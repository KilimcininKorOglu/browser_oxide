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
        })
    }

    /// Create a child iframe by fetching src URL via HTTP client.
    pub async fn from_url(
        node_id: NodeId,
        url: &str,
        client: &crate::net::HttpClient,
        stealth_profile: Option<&crate::stealth::StealthProfile>,
        parent_origin: Option<&str>,
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
            .get(url)
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

        // Run child event loop briefly. A frame that keeps timers alive would
        // hold the parent here for the whole budget, while in a browser both
        // documents run side by side; Page::pump_frames drives it afterwards.
        event_loop
            .run_until_idle(Duration::from_millis(300))
            .await?;

        Ok(Self {
            node_id,
            event_loop,
        })
    }

    /// Evaluate JS in the child's V8 context.
    pub fn evaluate(&mut self, js: &str) -> Result<String, deno_core::error::AnyError> {
        self.event_loop.execute_script(js)
    }

    /// Query the child's DOM for text content of a selector match.
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
