//! Run the Apostate fingerprint collector in this engine and print its JSON.
//!
//!   cargo run --release -p browser_oxide --example collect_probe -- <collector.js>
//!
//! The page is an empty document; the collector source is evaluated first, then
//! a runner collects and stores the JSON on `__probe_result`, which we poll.

use std::time::{Duration, Instant};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let collector_path = args.next().expect("usage: collect_probe <collector.js>");
    let collector = std::fs::read_to_string(collector_path).expect("collector js");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let mut page = browser_oxide::Page::from_html(
                "<html><head></head><body></body></html>",
                Some(browser_oxide::stealth::presets::chrome_148_macos()),
            )
            .await
            .expect("page");

            if let Err(e) = page.evaluate(&collector) {
                eprintln!("COLLECTOR LOAD ERROR: {e}");
                std::process::exit(1);
            }

            let runner = r#"
                (async () => {
                    try {
                        const r = await window.ApostateCollector.collect();
                        globalThis.__probe_result = JSON.stringify(
                            Object.fromEntries(Object.entries(r.probes).map(([k, v]) =>
                                [k, v.ok ? v.value : { __error: String(v.error) }])));
                    } catch (e) {
                        globalThis.__probe_result = "ERR:" + (e && e.message || e);
                    }
                })();
            "#;
            if let Err(e) = page.evaluate(runner) {
                eprintln!("RUNNER ERROR: {e}");
                std::process::exit(1);
            }

            let t0 = Instant::now();
            loop {
                let _ = page.event_loop().run_until_idle(Duration::from_millis(200)).await;
                if let Ok(s) = page.evaluate("globalThis.__probe_result") {
                    if s != "undefined" && !s.is_empty() {
                        println!("{}", s.trim_matches('"'));
                        break;
                    }
                }
                if t0.elapsed() > Duration::from_secs(60) {
                    eprintln!("TIMEOUT waiting for collect()");
                    std::process::exit(2);
                }
            }
            page.consume_and_print_logs();
        })
        .await;
}
