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
        match page.evaluate(&js) {
            Ok(s) => println!("{}", s.trim_matches('"')),
            Err(e) => eprintln!("EVAL ERROR: {e}"),
        }
        page.consume_and_print_logs();
    }).await;
}
