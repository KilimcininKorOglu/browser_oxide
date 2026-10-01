//! Evaluate a JS snippet on a real https page with the macOS preset.
use std::time::Duration;
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().expect("usage: eval_secure <url> <js-file>");
    let js = std::fs::read_to_string(args.next().expect("js file")).expect("js");
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        let profile = browser_oxide::stealth::presets::chrome_148_macos();
        let mut page = browser_oxide::Page::navigate(&url, profile, 3).await.expect("navigate");
        let _ = page.event_loop().run_until_idle(Duration::from_secs(3)).await;
        // The snippet may return a promise; settle it into
        // __probe_result and poll, then print.
        let wrapped = format!(
            "globalThis.__probe_result = undefined;\n\
             Promise.resolve().then(() => ({js}))\n\
             .then(r => {{ globalThis.__probe_result = typeof r === 'string' ? r : JSON.stringify(r); }})\n\
             .catch(e => {{ globalThis.__probe_result = 'ERR:' + (e && e.message || e); }})"
        );
        let _ = page.evaluate_async(&wrapped, Duration::from_secs(30)).await;
        let t0 = std::time::Instant::now();
        loop {
            let _ = page.event_loop().run_until_idle(Duration::from_millis(200)).await;
            match page.evaluate("globalThis.__probe_result") {
                Ok(s) if s != "undefined" && !s.is_empty() => {
                    println!("{}", s.trim_matches('"'));
                    break;
                }
                _ => {}
            }
            if t0.elapsed() > Duration::from_secs(35) {
                eprintln!("TIMEOUT");
                break;
            }
        }
        page.consume_and_print_logs();
    }).await;
}
