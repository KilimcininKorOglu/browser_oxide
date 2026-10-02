//! Navigate to a page, let it run, and dump the RAW document HTML.
//!
//!   cargo run --release -p browser_oxide --example botcheck_probe -- <url> [wait_secs]
//!
//! DUMP_TO=<path> writes the raw HTML there; without it the HTML prints.

use std::time::{Duration, Instant};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .expect("usage: botcheck_probe <url> [wait_secs]");
    let wait: u64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(25);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let profile = browser_oxide::stealth::presets::chrome_153_macos();
            let mut page = match browser_oxide::Page::navigate(&url, profile, 3).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("NAV ERROR: {e}");
                    std::process::exit(1);
                }
            };
            let t0 = Instant::now();
            loop {
                let _ = page
                    .event_loop()
                    .run_until_idle(Duration::from_millis(250))
                    .await;
                if t0.elapsed() > Duration::from_secs(wait) {
                    break;
                }
            }
            // RAW: the full document HTML exactly as the engine rendered it.
            match page.evaluate("document.documentElement.outerHTML") {
                Ok(s) => {
                    let unescaped = s
                        .trim_matches('"')
                        .replace("\\n", "\n")
                        .replace("\\t", "\t")
                        .replace("\\\"", "\"")
                        .replace("\\\\", "\\");
                    if let Ok(dest) = std::env::var("DUMP_TO") {
                        std::fs::write(&dest, &unescaped).expect("write dump");
                        eprintln!("[dump] {} bytes -> {}", unescaped.len(), dest);
                    } else {
                        println!("{unescaped}");
                    }
                }
                Err(e) => eprintln!("EVAL ERROR: {e}"),
            }
            page.consume_and_print_logs();
        })
        .await;
}
