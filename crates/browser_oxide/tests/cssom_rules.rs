//! CSSOM surface tests: the rule objects a stylesheet walk collects.
//!
//! Every number here was measured against Chrome 154 on the same markup. The
//! shape is the measurement: a probe that walks `document.styleSheets` reads
//! rule types, `cssRules`, `keyText` and the declaration's own key set, and a
//! sheet made only of `@keyframes` — which is what a challenge widget ships —
//! is unreadable unless the nested steps exist.

use browser_oxide::Page;

const SHEET: &str = r#"<!DOCTYPE html><html><head><style>
@keyframes spin { from { opacity: 0 } 50% { opacity: .5; transform: scale(2) } to { opacity: 1 } }
@keyframes scale { 0%, 100% { transform: none } 50% { transform: scale3d(1,1,1) } }
#a { color: red; margin: 0; padding: 1px; border-width: 2px }
#b { top: 0 }
</style></head><body><div id="a"></div><div id="b"></div></body></html>"#;

async fn sheet_page() -> Page {
    Page::from_html_with_url(
        SHEET,
        "https://site.example/",
        Some(browser_oxide::stealth::presets::chrome_153_macos()),
    )
    .await
    .unwrap()
}

/// A `@keyframes` rule is a `CSSKeyframesRule` that names itself and lists its
/// steps. Chrome reports `from`/`to` as the percentages they stand for and
/// spaces a function's arguments.
#[tokio::test]
async fn keyframes_rules_list_their_steps() {
    let mut page = sheet_page().await;
    let got = page
        .evaluate(
            r#"(() => {
                const kf = document.styleSheets[0].cssRules[0];
                const steps = [];
                for (let i = 0; i < kf.cssRules.length; i++) {
                    const k = kf.cssRules[i];
                    steps.push([k.constructor.name, k.type, k.keyText, k.cssText,
                                k.style ? k.style.length : 'none'].join(':'));
                }
                return [kf.constructor.name, kf.type, kf.name, kf.cssRules.length,
                        steps.join('|')].join(' ; ');
            })()"#,
        )
        .unwrap();
    assert_eq!(
        got,
        "CSSKeyframesRule ; 7 ; spin ; 3 ; \
         CSSKeyframeRule:8:0%:0% { opacity: 0; }:1|\
CSSKeyframeRule:8:50%:50% { opacity: 0.5; transform: scale(2); }:2|\
CSSKeyframeRule:8:100%:100% { opacity: 1; }:1"
    );
}

#[tokio::test]
async fn keyframes_serialises_function_arguments_with_spaces() {
    let mut page = sheet_page().await;
    let got = page
        .evaluate(r#"String(document.styleSheets[0].cssRules[1].cssRules[0].cssText)"#)
        .unwrap();
    assert_eq!(got, "0%, 100% { transform: none; }");
    let got = page
        .evaluate(r#"String(document.styleSheets[0].cssRules[1].cssRules[1].cssText)"#)
        .unwrap();
    assert_eq!(got, "50% { transform: scale3d(1, 1, 1); }");
}

/// `cssRules` comes from the `CSSGroupingRule` mixin, so every rule answers it
/// — a style rule with an empty list, not `undefined`.
#[tokio::test]
async fn every_rule_answers_css_rules() {
    let mut page = sheet_page().await;
    let got = page
        .evaluate(
            r#"(() => {
                const rules = document.styleSheets[0].cssRules;
                const out = [];
                for (let i = 0; i < rules.length; i++) {
                    out.push((rules[i].cssRules !== undefined ? rules[i].cssRules.length : 'MISSING'));
                }
                return out.join(',');
            })()"#,
        )
        .unwrap();
    // spin, scale, #a (style rule → empty), #b (style rule → empty)
    assert_eq!(got, "3,2,0,0");
}

/// A declared declaration counts the EXPANDED longhands and enumerates the
/// same named properties a computed one does. Chrome answers 13 longhands and
/// 752 keys for `#a { color: red; margin: 0; padding: 1px; border-width: 2px }`
/// — 739 names plus one key per longhand.
#[tokio::test]
async fn a_declared_declaration_counts_longhands_and_lists_every_name() {
    let mut page = sheet_page().await;
    let got = page
        .evaluate(
            r#"(() => {
                const s = document.styleSheets[0].cssRules[2].style;
                return [s.length, Object.keys(s).length, s.item(0), s.item(1),
                        s.color, s.margin, s.getPropertyValue('padding'),
                        s.cssText].join('|');
            })()"#,
        )
        .unwrap();
    assert_eq!(
        got,
        "13|752|color|margin-top|red|0px|1px|color: red; margin: 0px; padding: 1px; border-width: 2px;"
    );
}

/// A rule that declares nothing still enumerates the names — the declaration
/// is not an empty object. Chrome answers 740 keys for `#b`'s single
/// declaration: 739 names plus one index.
#[tokio::test]
async fn a_single_declaration_still_lists_every_name() {
    let mut page = sheet_page().await;
    let got = page
        .evaluate(
            r#"(() => {
                const s = document.styleSheets[0].cssRules[3].style;
                return [Object.keys(s).length, s.length, s.item(0),
                        s.top, s.margin].join('|');
            })()"#,
        )
        .unwrap();
    // `top: 0` is a length, so it reads back with its unit.
    assert_eq!(got, "740|1|top|0px|0px");
}
