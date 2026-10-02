//! Iframe support for browser_oxide.
//!
//! Each iframe with `srcdoc` gets its own DOM tree, V8 runtime, and event loop.
//! Communication between parent and child is via serialized postMessage.

use crate::dom::node::{NodeData, NodeId};
use crate::dom::Dom;
use crate::event_loop::BrowserEventLoop;
use crate::js_runtime::runtime::BrowserRuntimeOptions;
use crate::js_runtime::BrowserJsRuntime;
use std::time::Duration;
use tracing;

/// loading -> interactive (DOMContentLoaded) -> complete (load), fired from
/// a zero-delay timer so async handlers run inside the event loop.
/// Watches the frame document for nested <iframe> elements that gain a
/// src after insertion (Turnstile's inner widget frame is created
/// src-less and pointed at its challenge URL from JS). Each such frame
/// raises a request to the page layer, which materializes it — this
/// realm cannot load frames itself.
const FRAME_WATCH_JS: &str = r#"
    (() => {
        globalThis.__fwMut = 0;
        globalThis.__fwReq = 0;
        const req = (frameEl) => {
            globalThis.__fwReq += 1;
            try {
                const src = frameEl.getAttribute('src');
                if (!src || src.startsWith('javascript:')) return;
                const selfId = globalThis.__oxSelfNodeId || 0;
                const idFn = globalThis.__browser_oxide && globalThis.__browser_oxide._getNodeId;
                const fid = idFn ? idFn(frameEl) : 0;
                if (globalThis.Deno && Deno.core && Deno.core.ops && Deno.core.ops.op_nested_frame_request) {
                    Deno.core.ops.op_nested_frame_request(selfId, fid, src);
                }
            } catch (_e) {}
        };
        new MutationObserver((muts) => {
            globalThis.__fwMut += muts.length;
            for (const m of muts) {
                if (m.type === 'attributes' && m.target && m.target.tagName === 'IFRAME') req(m.target);
                if (m.type === 'childList') {
                    m.addedNodes.forEach((n) => {
                        if (n.nodeType !== 1) return;
                        if (n.tagName === 'IFRAME') { req(n); return; }
                        // Subtree: innerHTML/container insertions carry the
                        // frame as a descendant, not as the added node.
                        try { n.querySelectorAll && n.querySelectorAll('iframe').forEach(req); } catch (_e) {}
                    });
                }
            }
        }).observe(document, { childList: true, subtree: true, attributes: true, attributeFilter: ['src'] });
        // Periodic scan: catch iframes created in DETACHED containers that
        // the MutationObserver misses (the container enters the document
        // after the iframe already has its src).
        let scanCount = 0;
        setInterval(() => {
            scanCount++;
            if (scanCount % 4 === 1) console.log('[FW-SCAN] scan=' + scanCount + ' iframes=' + document.querySelectorAll('iframe').length + ' withSrc=' + document.querySelectorAll('iframe[src]').length);
            try {
                document.querySelectorAll('iframe[src]').forEach((f) => {
                    const src = f.getAttribute('src');
                    if (src && !f.__fwProcessed) {
                        f.__fwProcessed = true;
                        req(f);
                    }
                });
            } catch (_e) {}
        }, 500);
    })();
"#;

const DOCUMENT_LIFECYCLE_JS: &str = r#"
setTimeout(() => {
    try { globalThis._browser_oxide.__documentReadyState = 'interactive'; } catch (_e) {}
    document.dispatchEvent(new Event('readystatechange'));
    document.dispatchEvent(new Event('DOMContentLoaded', {bubbles: true}));
    window.dispatchEvent(new Event('DOMContentLoaded', {bubbles: true}));
    try { globalThis._browser_oxide.__documentReadyState = 'complete'; } catch (_e) {}
    document.dispatchEvent(new Event('readystatechange'));
    window.dispatchEvent(new Event('load'));
}, 0);
"#;

/// Info about an iframe found in the DOM.
pub struct IframeInfo {
    pub node_id: NodeId,
    pub srcdoc: Option<String>,
    pub src: Option<String>,
}

/// A child iframe with its own V8 runtime and DOM.
pub struct ChildIframe {
    pub node_id: NodeId,
    pub event_loop: BrowserEventLoop,
    /// Frames nested INSIDE this frame's own document. Turnstile hosts its
    /// inner widget frame here; without materializing this level the inner
    /// VM never loads and the round stalls.
    pub children: Vec<ChildIframe>,
    pub depth: usize,
}

impl ChildIframe {
    /// Create a child iframe from srcdoc HTML.
    pub async fn from_srcdoc(
        node_id: NodeId,
        html: &str,
        profile: &crate::stealth::StealthProfile,
    ) -> Result<Self, deno_core::error::AnyError> {
        let dom = crate::html_parser::parse_html(html);
        let scripts = crate::script_runner::find_scripts(&dom);

        let runtime = BrowserJsRuntime::with_options(
            dom,
            BrowserRuntimeOptions {
                stealth_profile: Some(profile.clone()),
                ..Default::default()
            },
        );
        let mut event_loop = BrowserEventLoop::new(runtime);

        // Execute scripts in the child's own V8 context. W2.7 — Chrome
        // reports `about:srcdoc` for srcdoc iframe stack frames.
        for (i, script) in scripts.iter().enumerate() {
            if script.src.is_some() {
                continue;
            } // Skip external scripts in srcdoc
            if script.code.trim().is_empty() {
                continue;
            }
            if let Err(e) = event_loop.execute_script_with_name(&script.code, "about:srcdoc") {
                tracing::warn!(script_index = i, error = %e, "iframe script error");
            }
        }

        // Run child event loop
        event_loop.run_until_idle(Duration::from_secs(5)).await?;

        Ok(Self {
            node_id,
            event_loop,
            children: Vec::new(),
            depth: 0,
        })
    }

    /// Create a child iframe by fetching src URL via HTTP client.
    ///
    /// `parent_origin` is the creating document's origin (its
    /// `sec-fetch-site` parent side); `parent_url` is that document's full
    /// URL, which is what a subframe request names in its `Referer`.
    pub async fn from_url(
        node_id: NodeId,
        url: &str,
        client: &crate::net::HttpClient,
        stealth_profile: Option<&crate::stealth::StealthProfile>,
        parent_origin: Option<&str>,
        parent_url: Option<&str>,
    ) -> Result<Self, deno_core::error::AnyError> {
        // CSP `frame-src` enforcement (falls back to child-src then
        // default-src). Real Chrome refuses to navigate iframes whose
        // src violates the parent's CSP, surfacing the same network-
        // error shape we return on op_fetch blocks.
        if let Ok(parsed_url) = url::Url::parse(url) {
            if let Err(violated) = crate::js_runtime::extensions::fetch_ext::check_csp(
                crate::net::csp::Directive::FrameSrc,
                &parsed_url,
                None,
                false,
            ) {
                eprintln!(
                    "[csp] Refused to frame '{}' because it violates the following Content Security Policy directive: \"{}\".",
                    url, violated
                );
                return Err(deno_core::error::AnyError::msg(format!(
                    "iframe blocked by CSP: {}",
                    url
                )));
            }
        }

        let resp = client
            // A subframe load, not a top-level navigation: the frame's own
            // document request must carry `sec-fetch-dest: iframe`, the
            // parent relationship and a Referer. `parent_url` falls back to
            // the origin when only that is known.
            .frame_get(url, parent_url.or(parent_origin))
            .await
            .map_err(|e| deno_core::error::AnyError::msg(format!("iframe fetch error: {}", e)))?;

        if !resp.ok() {
            return Err(deno_core::error::AnyError::msg(format!(
                "iframe fetch {} returned {}",
                url, resp.status
            )));
        }

        let html = resp.text();
        // Skip if response looks like non-HTML (binary, error page)
        if html.trim().is_empty() {
            return Self::from_srcdoc(
                node_id,
                "<html><body></body></html>",
                stealth_profile.unwrap(),
            )
            .await;
        }

        let dom = crate::html_parser::parse_html(&html);
        let scripts = crate::script_runner::find_scripts(&dom);
        let stylesheet_entries = crate::stylesheet_collector::find_stylesheets(&dom);
        // The frame document's own CSP, from its response and its meta tags.
        let csp_headers: Vec<&str> = resp
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-security-policy"))
            .map(|(_, v)| v.as_str())
            .collect();
        let require_trusted_types = crate::csp_collector::collect_csp(&csp_headers, &dom)
            .requires_trusted_types_for_script()
            && stealth_profile.is_none_or(|p| p.enforce_csp)
            && std::env::var("BROWSER_OXIDE_CSP_BYPASS").is_err();

        // Fetch external stylesheets
        let mut stylesheets = Vec::new();
        for entry in &stylesheet_entries {
            match entry {
                crate::stylesheet_collector::StylesheetEntry::Inline(_) => {}
                crate::stylesheet_collector::StylesheetEntry::External(href) => {
                    let full_url = if href.starts_with("http") {
                        href.clone()
                    } else if href.starts_with('/') {
                        if let Ok(base) = url::Url::parse(url) {
                            format!(
                                "{}://{}{}",
                                base.scheme(),
                                base.host_str().unwrap_or(""),
                                href
                            )
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    };
                    if let Ok(resp) = client.get(&full_url).await {
                        if resp.ok() {
                            let text = resp.text();
                            if !text.trim_start().starts_with("<!") {
                                stylesheets.push(text);
                            }
                        }
                    }
                }
            }
        }

        let mut options = BrowserRuntimeOptions {
            external_stylesheets: stylesheets,
            is_secure_context: crate::page::is_secure_url(url),
            frame_parent_origin: parent_origin.map(str::to_string),
            // The document URL comes from the runtime state; assigning
            // location.href after start-up would queue a navigation instead.
            base_url: url::Url::parse(url).ok(),
            require_trusted_types,
            ..Default::default()
        };
        if let Some(profile) = stealth_profile {
            options.stealth_profile = Some(profile.clone());
        }

        let runtime = BrowserJsRuntime::with_options(dom, options);
        let mut event_loop = BrowserEventLoop::new(runtime);

        // Set location.href (URL-state setup, not a real navigation), the
        // same way Page::from_html_with_url does, and drop the navigation
        // the assignment queued so the frame's event loop keeps running.
        let url_js = serde_json::Value::String(url.to_string());
        event_loop
            .execute_script(&format!(
                "location.href = {url_js}; delete globalThis.__pendingNavigation;"
            ))
            .ok();
        event_loop.reset_nav_pending();

        // Execute scripts, fetching external ones
        for (i, script) in scripts.iter().enumerate() {
            let code = if let Some(src) = &script.src {
                let full_url = if src.starts_with("http") {
                    src.clone()
                } else if src.starts_with('/') {
                    if let Ok(base) = url::Url::parse(url) {
                        format!(
                            "{}://{}{}",
                            base.scheme(),
                            base.host_str().unwrap_or(""),
                            src
                        )
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };
                match client.get(&full_url).await {
                    Ok(resp) if resp.ok() => {
                        let text = resp.text();
                        if text.trim_start().starts_with("<!") {
                            continue;
                        }
                        text
                    }
                    _ => continue,
                }
            } else {
                script.code.clone()
            };

            if code.trim().is_empty() {
                continue;
            }
            // W2.7 — name scripts by their actual URL (external src or
            // the iframe document URL for inline). Chrome stack frames
            // are URL-tagged, not anonymous.
            let name = if let Some(src) = &script.src {
                src.clone()
            } else {
                url.to_string()
            };
            if let Err(e) = event_loop.execute_script_with_name(&code, &name) {
                tracing::warn!(script_index = i, error = %e, "iframe script error");
            }
        }

        // Advance the frame document's lifecycle the way Page's build path
        // does for a top-level document: a challenge script that waits for
        // DOMContentLoaded or load otherwise never starts.
        if let Err(e) = event_loop.execute_script(DOCUMENT_LIFECYCLE_JS) {
            tracing::warn!(error = %e, "iframe lifecycle script error");
        }
        let _ = event_loop.execute_script(FRAME_WATCH_JS);
        // TEMP DIAG: capture ALL errors and rejections in the child realm
        let _ = event_loop.execute_script(
            r#"
            globalThis.__childDiag = [];
            addEventListener('error', (e) => { __childDiag.push('ERR:' + (e.message || '?') + ' @ ' + String(e.filename||'').slice(-30) + ':' + (e.lineno||0)); });
            addEventListener('unhandledrejection', (e) => { __childDiag.push('REJ:' + String(e.reason && (e.reason.stack || e.reason.message || e.reason) || '?').slice(0, 150)); });
            "#,
        );

        // Run child event loop briefly. A frame that keeps timers alive would
        // hold the parent here for the whole budget, while in a browser both
        // documents run side by side; Page::pump_frames drives it afterwards.
        event_loop
            .run_until_idle(Duration::from_millis(300))
            .await?;

        Ok(Self {
            node_id,
            event_loop,
            children: Vec::new(),
            depth: 0,
        })
    }

    /// Evaluate JS in the child's V8 context.
    pub fn evaluate(&mut self, js: &str) -> Result<String, deno_core::error::AnyError> {
        self.event_loop.execute_script(js)
    }

    /// Query the child's DOM for text content of a selector match.
    /// Scan THIS frame's document for nested <iframe> elements and
    /// materialize each as a full child realm (one level per call). The
    /// challenge flow nests its widget frame inside the challenge frame;
    /// a never-loaded nested frame stalls the whole round.
    pub async fn materialize_children(
        &mut self,
        client: &crate::net::HttpClient,
        profile: &crate::stealth::StealthProfile,
        base_url: &str,
    ) -> usize {
        if self.depth >= 3 {
            return 0;
        }
        let iframes = {
            let dom = self.event_loop.runtime_mut().inner();
            let state = dom.op_state();
            let state = state.borrow();
            let dom_state = state.borrow::<crate::js_runtime::state::DomState>();
            find_iframes(&dom_state.dom)
        };
        let already: Vec<_> = self.children.iter().map(|c| c.node_id).collect();
        let mut materialized = 0usize;
        for info in &iframes {
            if already.contains(&info.node_id) {
                continue;
            }
            let child = if let Some(srcdoc) = &info.srcdoc {
                ChildIframe::from_srcdoc(info.node_id, srcdoc, profile).await
            } else if let Some(src) = &info.src {
                if src.is_empty() || src.starts_with("javascript:") {
                    continue;
                }
                let full = if src.starts_with("http") {
                    src.clone()
                } else if let Ok(base) = url::Url::parse(base_url) {
                    base.join(src)
                        .map(|u| u.to_string())
                        .unwrap_or_else(|_| src.clone())
                } else {
                    src.clone()
                };
                ChildIframe::from_url(
                    info.node_id,
                    &full,
                    client,
                    Some(profile),
                    cross_origin_parent(base_url, &full).as_deref(),
                    Some(base_url),
                )
                .await
            } else {
                continue;
            };
            if let Ok(mut child) = child {
                child.depth = self.depth + 1;
                // Recurse: the nested frame may itself contain frames.
                let inner = Box::pin(child.materialize_children(client, profile, base_url)).await;
                materialized += 1 + inner;
                self.children.push(child);
            }
        }
        materialized
    }

    /// Route frame messages between this realm and its nested children
    /// (mirror of the page-level pump).
    pub async fn pump_frames(&mut self, budget: Duration) -> usize {
        const TO_PARENT: &str = "(() => { const b = globalThis[Symbol.for('__ox_frames')]; return b && b.drainToParent ? b.drainToParent() : '[]'; })()";
        const TO_CHILDREN: &str = "(() => { const b = globalThis[Symbol.for('__ox_frames')]; return b && b.drainToChildren ? b.drainToChildren() : '[]'; })()";
        let mut delivered = 0usize;
        for child in self.children.iter_mut() {
            if let Err(e) = child.event_loop.run_until_idle(budget).await {
                tracing::warn!(error = %e, "nested frame event loop error");
            }
            let queued = child.evaluate(TO_PARENT).unwrap_or_default();
            for msg in parse_frame_messages(&queued) {
                let js = format!(
                    "globalThis[Symbol.for('__ox_frames')].deliverFromChild({}, {}, {})",
                    child.node_id.to_raw(),
                    serde_json::Value::String(msg.data),
                    serde_json::Value::String(msg.origin)
                );
                if self.event_loop.execute_script(&js).is_ok() {
                    delivered += 1;
                }
            }
        }
        let queued = self
            .event_loop
            .execute_script(TO_CHILDREN)
            .unwrap_or_default();
        for msg in parse_frame_messages(&queued) {
            if let Some(node) = msg.node {
                if let Some(child) = self
                    .children
                    .iter_mut()
                    .find(|c| c.node_id.to_raw() == node)
                {
                    let js = format!(
                        "globalThis[Symbol.for('__ox_frames')].deliverFromParent({}, {})",
                        serde_json::Value::String(msg.data),
                        serde_json::Value::String(msg.origin)
                    );
                    if child.evaluate(&js).is_ok() {
                        delivered += 1;
                    }
                }
            }
        }
        delivered
    }

    pub fn materialize_grandchildren_sync(&mut self) {
        // placeholder to keep borrow checker simple at call sites
    }

    pub fn query_text(&mut self, selector: &str) -> Option<String> {
        self.evaluate(&format!(
            r#"(() => {{ const el = document.querySelector("{}"); return el ? el.textContent : ""; }})()"#,
            selector.replace('"', "\\\"")
        )).ok().filter(|s| !s.is_empty())
    }
}

/// One postMessage queued by a frame bridge, with its JSON-encoded data.
pub struct FrameMessage {
    /// Target iframe node id; set only on parent->child messages.
    pub node: Option<u32>,
    pub data: String,
    pub origin: String,
}

/// Parse the JSON array a frame bridge's `drain()` returns.
pub fn parse_frame_messages(queued: &str) -> Vec<FrameMessage> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str(queued) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|m| {
            Some(FrameMessage {
                node: m.get("node").and_then(|n| n.as_u64()).map(|n| n as u32),
                data: m.get("data")?.as_str()?.to_string(),
                origin: m.get("origin")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// The embedding document's origin when `child_url` is cross-origin to
/// `parent_url`; `None` for a same-origin frame or an unparsable URL.
pub fn cross_origin_parent(parent_url: &str, child_url: &str) -> Option<String> {
    let parent = url::Url::parse(parent_url).ok()?.origin();
    let child = url::Url::parse(child_url).ok()?.origin();
    (parent != child).then(|| parent.ascii_serialization())
}

/// Find all `<iframe>` elements in the DOM.
pub fn find_iframes(dom: &Dom) -> Vec<IframeInfo> {
    let mut iframes = Vec::new();
    collect_iframes(dom, NodeId::DOCUMENT, &mut iframes);
    iframes
}

fn collect_iframes(dom: &Dom, node_id: NodeId, iframes: &mut Vec<IframeInfo>) {
    let children = dom.children(node_id);
    for child_id in children {
        if let Some(node) = dom.get(child_id) {
            if let NodeData::Element(elem) = &node.data {
                if elem.name.local.eq_ignore_ascii_case("iframe") {
                    let srcdoc = elem
                        .attrs
                        .iter()
                        .find(|a| a.name.local == "srcdoc")
                        .map(|a| a.value.clone());
                    let src = elem
                        .attrs
                        .iter()
                        .find(|a| a.name.local == "src")
                        .map(|a| a.value.clone());
                    iframes.push(IframeInfo {
                        node_id: child_id,
                        srcdoc,
                        src,
                    });
                }
                // Widgets such as Turnstile put their iframe inside a
                // (closed) shadow root; walk it like the light DOM.
                if let Some(shadow) = elem.shadow_root {
                    collect_iframes(dom, shadow, iframes);
                }
            }
            collect_iframes(dom, child_id, iframes);
        }
    }
}
