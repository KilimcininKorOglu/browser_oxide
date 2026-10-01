//! Live Turnstile probe — loads a page that embeds a Cloudflare Turnstile
//! widget, lets the challenge run (iframe + worker), and polls
//! `turnstile.getResponse()` until a token is minted or the budget runs out.
//!
//!   cargo run --release -p browser --example turnstile_probe -- <url> [profile] [budget_secs]
//!
//! Prints one status line per second: iframe count, widget state, token
//! length. Exits 0 when a token was minted, 2 when the budget expired.

use std::time::{Duration, Instant};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .expect("usage: turnstile_probe <url> [profile] [budget_secs]");
    let profile_name = args
        .next()
        .unwrap_or_else(|| "chrome_148_macos".to_string());
    let budget_secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);

    let profile = match profile_name.as_str() {
        "chrome_148_macos" => browser_oxide::stealth::presets::chrome_148_macos(),
        "chrome_148_windows" => browser_oxide::stealth::presets::chrome_148_windows(),
        other => panic!("unknown profile {other}"),
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let t0 = Instant::now();
            let diag_init = r##"
                globalThis.__evts = {};
                globalThis.__entryVals = {};
                setTimeout(() => {
                    const E = globalThis.__entryVals;
                    try { const cs = getComputedStyle(document.body); let h = 0; for (let i = 0; i < cs.length; i++) h = (h * 31 + cs.getPropertyValue(cs[i]).length) | 0; E.csHash = cs.length + ":" + h; } catch(e) { E.csHash = "ERR"; }
                    try { let n = 0; for (const s of document.styleSheets) { try { n += s.cssRules.length; } catch(e) {} } E.cssRules = n; } catch(e) { E.cssRules = "ERR"; }
                    try { E.docTitle = document.title.length; } catch(e) { E.docTitle = "ERR"; }
                    try { const c = document.createElement("canvas").getContext("2d"); c.font = "14px sans-serif"; E.emoji = String(c.measureText("\u{1F600}").width); } catch(e) { E.emoji = "ERR"; }
                    try { const c = document.createElement("canvas").getContext("2d"); c.font = "14px Arial"; E.font = String(c.measureText("mmmmmmmmmmlli").width); } catch(e) { E.font = "ERR"; }
                    try { E.tz = Intl.DateTimeFormat().resolvedOptions().timeZone; } catch(e) { E.tz = "ERR"; }
                    try { E.lang = navigator.language; } catch(e) { E.lang = "ERR"; }
                    try { E.uad = navigator.userAgentData.brands.map(b => b.brand + ":" + b.version).join(","); } catch(e) { E.uad = "ERR"; }
                }, 3000);
                addEventListener('message', (e) => {
                    try { const ev = e.data && e.data.event; if (ev) globalThis.__evts[ev] = (globalThis.__evts[ev] || 0) + 1; } catch (_) {}
                });
            "##;
            let mut page = match browser_oxide::Page::navigate_with_init(&url, profile.clone(), 3, vec![diag_init.to_string()]).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[navigate] ERROR: {e}");
                    std::process::exit(1);
                }
            };
            eprintln!(
                "[navigate] {} ms, body_len={}, iframes={}",
                t0.elapsed().as_millis(),
                page.content().len(),
                page.child_iframe_count()
            );

            let _ = page
                .event_loop()
                .run_until_idle(Duration::from_secs(8))
                .await;

            let poll = "(() => { \
                if (typeof turnstile === 'undefined') return 'no-turnstile'; \
                if (typeof turnstile.getResponse !== 'function') return 'no-getResponse'; \
                const t = turnstile.getResponse(); \
                return t ? ('TOKEN:' + t.length) : 'empty'; \
            })()";
            let frame_state = |n: usize| {
                format!(
                    "(() => {{ const f = globalThis.__ox_frame_info; \
                     return f && f({n}) ? JSON.stringify(f({n})) : 'none'; }})()"
                )
            };

            let mut minted: Option<String> = None;
            let mut last_iframes = 0usize;
            let mut last_delivered = 0usize;
            while t0.elapsed() < Duration::from_secs(budget_secs) {
                let client = browser_oxide::net::HttpClient::shared(&profile).expect("client");
                let delivered = page
                    .pump_frames_ctx(
                        Duration::from_millis(400),
                        Some(&client),
                        Some(&profile),
                    )
                    .await;
                let iframes = page.child_iframe_count();
                if iframes != last_iframes {
                    eprintln!("[iframes] {iframes}");
                    last_iframes = iframes;
                }

                // Per-frame diagnostics: URL, readyState, console output.
                for i in 0..iframes {
                    let href = {
                        let child = page.child_iframe(i).expect("frame");
                        child
                            .evaluate("location.href + ' | rs=' + document.readyState")
                            .unwrap_or_else(|e| format!("eval-err: {e}"))
                    };
                    let logs = {
                        let child = page.child_iframe(i).expect("frame");
                        let runtime = child.event_loop.runtime_mut().inner();
                        let state = runtime.op_state();
                        let mut state = state.borrow_mut();
                        let dom_state =
                            state.borrow_mut::<browser_oxide::js_runtime::state::DomState>();
                        std::mem::take(&mut dom_state.console_output)
                    };
                    for log in logs {
                        let prefix = match log.level {
                            browser_oxide::js_runtime::state::ConsoleLevel::Log => "[F{i} LOG]",
                            browser_oxide::js_runtime::state::ConsoleLevel::Warn => "[F{i} WARN]",
                            browser_oxide::js_runtime::state::ConsoleLevel::Error => "[F{i} ERROR]",
                            _ => "[F{i} INFO]",
                        };
                        eprintln!(
                            "{} {}",
                            prefix.replace("{i}", &i.to_string()),
                            log.args.join(" ")
                        );
                    }
                    if delivered != last_delivered {
                        eprintln!("[frame {i}] {} msgs={}", href.trim_matches('"'), delivered);
                    }
                }
                last_delivered = delivered;

                match page.evaluate(poll) {
                    Ok(s) if s.starts_with("\"TOKEN:") || s.contains("TOKEN:") => {
                        minted = Some(s.trim_matches('"').to_string());
                        break;
                    }
                    Ok(s) => {
                        if iframes > 0 {
                            let mut states = Vec::new();
                            for i in 0..iframes {
                                let js = frame_state(i);
                                states.push(
                                    page.evaluate(&js)
                                        .unwrap_or_else(|e| format!("eval-err:{e}")),
                                );
                            }
                            eprintln!(
                                "[{:>5.1}s] widget={} frames={}",
                                t0.elapsed().as_secs_f32(),
                                s.trim_matches('"'),
                                states.join(" | ")
                            );
                        } else {
                            eprintln!(
                                "[{:>5.1}s] widget={} frames=0",
                                t0.elapsed().as_secs_f32(),
                                s.trim_matches('"')
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("[{:>5.1}s] evaluate error: {e}", t0.elapsed().as_secs_f32())
                    }
                }
                tokio::time::sleep(Duration::from_millis(600)).await;
            }

            match minted {
                Some(tok) => {
                    println!("MINTED {} at {:.1}s", tok, t0.elapsed().as_secs_f32());
                    println!("OK");
                }
                None => {
                    println!("NO-TOKEN after {:.1}s", t0.elapsed().as_secs_f32());
                    println!("iframes={}", page.child_iframe_count());
                    if let Ok(v) = page.evaluate("JSON.stringify(globalThis.__evts || {})") {
                        eprintln!("[EVENTS] {}", v);
                    }
                    if let Ok(v) = page.evaluate("JSON.stringify(globalThis.__entryVals || {})") {
                        eprintln!("[ENTRY-VALS] {}", v);
                    }
                    page.consume_and_print_logs();
                    std::process::exit(2);
                }
            }
        })
        .await;
}
