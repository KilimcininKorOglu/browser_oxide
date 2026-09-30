//! Chrome surface parity report — diffs this engine's global `window`
//! surface against a real Chrome capture. The fixture
//! (`fixtures/chrome_surface.json`) holds the window own-property names and
//! kinds read off a real Chrome over CDP; refresh it with the same capture
//! script when Chrome moves.
//!
//! Not a pass/fail gate: the gap is expected to be non-zero. Run it to get
//! the sorted list of what a challenge script would find missing or
//! mistyped, then fix by priority:
//!
//!   cargo test -p browser_oxide --test chrome_surface_parity -- --ignored --nocapture

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const FIXTURE: &str = include_str!("fixtures/chrome_surface.json");

#[tokio::test]
#[ignore]
async fn chrome_surface_gap_report() {
    let surface: serde_json::Value = serde_json::from_str(FIXTURE).expect("surface fixture");

    // window entries are [name, kind(f/v/g/gs), …]
    let expected: BTreeMap<String, String> = surface["window"]
        .as_array()
        .expect("window array")
        .iter()
        .filter_map(|e| {
            let a = e.as_array()?;
            let name = a.first()?.as_str()?.to_string();
            let kind = a.get(1)?.as_str()?.to_string();
            Some((name, kind))
        })
        .collect();
    eprintln!(
        "capture: ua={} window props={}",
        surface["capture"].as_str().unwrap_or("?"),
        expected.len(),
    );

    // Secure context: the capture comes from an https page, and the engine
    // strips [SecureContext] globals on insecure ones.
    let mut page = browser_oxide::Page::from_html_with_url(
        "<html><head></head><body></body></html>",
        "https://odeme.com.tr/",
        Some(browser_oxide::stealth::presets::chrome_148_macos()),
    )
    .await
    .expect("page");

    let _ = page
        .event_loop()
        .run_until_idle(Duration::from_secs(5))
        .await;

    let actual_raw = page
        .evaluate(
            "JSON.stringify({ names: Object.getOwnPropertyNames(window).sort(), \
             sample: { webdriver: navigator.webdriver, tz: Intl.DateTimeFormat().resolvedOptions().timeZone } })",
        )
        .expect("enumerate window");

    let actual: serde_json::Value =
        serde_json::from_str(actual_raw.trim_matches('"')).expect("actual json");
    let actual_names: BTreeSet<String> = actual["names"]
        .as_array()
        .expect("names")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let missing: Vec<&String> = expected
        .keys()
        .filter(|n| !actual_names.contains(*n))
        .collect();
    let expected_names: BTreeSet<String> = expected.keys().cloned().collect();
    let extra: Vec<&String> = actual_names.difference(&expected_names).collect();

    eprintln!("\n=== WINDOW SURFACE GAP ===");
    eprintln!(
        "expected {} | actual {} | missing {} | extra {}",
        expected.len(),
        actual_names.len(),
        missing.len(),
        extra.len()
    );

    let mut by_kind: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for name in &missing {
        if let Some(kind) = expected.get(*name) {
            by_kind.entry(kind.as_str()).or_default().push(name);
        }
    }
    for (kind, names) in &by_kind {
        eprintln!("\n-- missing {} ({}):", kind, names.len());
        for n in names.iter().take(60) {
            eprintln!("   {n}");
        }
        if names.len() > 60 {
            eprintln!("   … (+{} more)", names.len() - 60);
        }
    }

    eprintln!("\n-- extra engine-only globals ({}):", extra.len());
    for n in extra.iter().take(40) {
        eprintln!("   +{n}");
    }
    if extra.len() > 40 {
        eprintln!("   … (+{} more)", extra.len() - 40);
    }

    eprintln!(
        "\nsample: webdriver={} tz={}",
        actual["sample"]["webdriver"], actual["sample"]["tz"]
    );

    // The report is informational — the gap is expected to be non-zero.
    // Nothing here hard-fails; the output above is the deliverable.
}
