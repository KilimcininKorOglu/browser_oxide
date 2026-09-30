//! Evaluate a JS snippet in a page context and print the result.
//!
//!   cargo run --release -p browser_oxide --example eval_probe -- <url-or-'-'> <js-file-or-'-stdin'>
//!
//! With `-` as the URL the page is an empty about:blank-equivalent.

use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().expect("usage: eval_probe <url|-> <js-file>");
    let js_file = args.next().expect("usage: eval_probe <url|-> <js-file>");
    let js = if js_file == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).expect("stdin");
        s
    } else {
        std::fs::read_to_string(&js_file).expect("js file")
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let mut page = if url == "-" {
                browser_oxide::Page::from_html(
                    "<html><head></head><body></body></html>",
                    None::<browser_oxide::stealth::StealthProfile>,
                )
                .await
                .expect("page")
            } else {
                let profile = browser_oxide::stealth::presets::chrome_148_macos();
                browser_oxide::Page::navigate(&url, profile, 3)
                    .await
                    .expect("navigate")
            };
            let _ = page
                .event_loop()
                .run_until_idle(Duration::from_secs(5))
                .await;
            match page.evaluate(&js) {
                Ok(s) => println!("{}", s.trim_matches('"')),
                Err(e) => {
                    eprintln!("EVAL ERROR: {e}");
                    std::process::exit(1);
                }
            }
            page.consume_and_print_logs();
        })
        .await;
}
