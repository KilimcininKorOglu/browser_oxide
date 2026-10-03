//! Tests for getComputedStyle wired to actual DOM inline styles.

use browser_oxide::Page;

fn html(body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head></head><body>{}</body></html>",
        body
    )
}

#[tokio::test]
async fn inline_style_color() {
    let mut page = Page::from_html(
        &html(r#"<div id="el" style="color: red"></div>"#),
        None::<browser_oxide::stealth::StealthProfile>,
    )
    .await
    .unwrap();
    let val = page
        .evaluate("getComputedStyle(document.getElementById('el')).color")
        .unwrap();
    assert_eq!(val, "rgb(255, 0, 0)", "should return inline style color");
}

#[tokio::test]
async fn inline_style_font_size() {
    let mut page = Page::from_html(
        &html(r#"<div id="el" style="font-size: 20px"></div>"#),
        None::<browser_oxide::stealth::StealthProfile>,
    )
    .await
    .unwrap();
    let val = page
        .evaluate("getComputedStyle(document.getElementById('el')).fontSize")
        .unwrap();
    assert_eq!(val, "20px");
}

#[tokio::test]
async fn inline_style_multiple_properties() {
    let mut page = Page::from_html(
        &html(r#"<div id="el" style="color: blue; opacity: 0.5; display: flex"></div>"#),
        None::<browser_oxide::stealth::StealthProfile>,
    )
    .await
    .unwrap();
    assert_eq!(
        page.evaluate("getComputedStyle(document.getElementById('el')).color")
            .unwrap(),
        "rgb(0, 0, 255)"
    );
    assert_eq!(
        page.evaluate("getComputedStyle(document.getElementById('el')).opacity")
            .unwrap(),
        "0.5"
    );
    assert_eq!(
        page.evaluate("getComputedStyle(document.getElementById('el')).display")
            .unwrap(),
        "flex"
    );
}

#[tokio::test]
async fn no_inline_style_returns_default() {
    let mut page = Page::from_html(
        &html(r#"<div id="el"></div>"#),
        None::<browser_oxide::stealth::StealthProfile>,
    )
    .await
    .unwrap();
    // No inline style — should return CSS defaults
    let display = page
        .evaluate("getComputedStyle(document.getElementById('el')).display")
        .unwrap();
    assert_eq!(display, "block");
    let vis = page
        .evaluate("getComputedStyle(document.getElementById('el')).visibility")
        .unwrap();
    assert_eq!(vis, "visible");
}

#[tokio::test]
async fn get_property_value_method() {
    let mut page = Page::from_html(
        &html(r#"<div id="el" style="margin-top: 10px"></div>"#),
        None::<browser_oxide::stealth::StealthProfile>,
    )
    .await
    .unwrap();
    let val = page
        .evaluate("getComputedStyle(document.getElementById('el')).getPropertyValue('margin-top')")
        .unwrap();
    assert_eq!(val, "10px");
}

#[tokio::test]
async fn js_style_mutation_reflected() {
    let mut page = Page::from_html(
        &html(r#"<div id="el"></div>"#),
        None::<browser_oxide::stealth::StealthProfile>,
    )
    .await
    .unwrap();
    page.evaluate("document.getElementById('el').style.backgroundColor = 'green'")
        .unwrap();
    let val = page
        .evaluate("getComputedStyle(document.getElementById('el')).backgroundColor")
        .unwrap();
    assert_eq!(val, "rgb(0, 128, 0)");
}

// --- Style block tests (these need CSS cascade wiring) ---

#[tokio::test]
async fn style_block_color() {
    let html = r#"<!DOCTYPE html><html><head><style>.red { color: red; }</style></head>
    <body><div id="el" class="red"></div></body></html>"#;
    let mut page = Page::from_html(html, None::<browser_oxide::stealth::StealthProfile>)
        .await
        .unwrap();
    let val = page
        .evaluate("getComputedStyle(document.getElementById('el')).color")
        .unwrap();
    assert_eq!(val, "rgb(255, 0, 0)");
}

#[tokio::test]
async fn style_block_specificity_id_beats_class() {
    let html = r#"<!DOCTYPE html><html><head><style>
        .blue { color: blue; }
        #el { color: green; }
    </style></head>
    <body><div id="el" class="blue"></div></body></html>"#;
    let mut page = Page::from_html(html, None::<browser_oxide::stealth::StealthProfile>)
        .await
        .unwrap();
    assert_eq!(
        page.evaluate("getComputedStyle(document.getElementById('el')).color")
            .unwrap(),
        "rgb(0, 128, 0)"
    );
}

#[tokio::test]
async fn inline_style_beats_style_block() {
    let html = r#"<!DOCTYPE html><html><head><style>#el { color: blue; }</style></head>
    <body><div id="el" style="color: red"></div></body></html>"#;
    let mut page = Page::from_html(html, None::<browser_oxide::stealth::StealthProfile>)
        .await
        .unwrap();
    assert_eq!(
        page.evaluate("getComputedStyle(document.getElementById('el')).color")
            .unwrap(),
        "rgb(255, 0, 0)"
    );
}

#[tokio::test]
async fn style_block_font_size() {
    let html = r#"<!DOCTYPE html><html><head><style>
        .big { font-size: 24px; }
    </style></head>
    <body><div id="el" class="big"></div></body></html>"#;
    let mut page = Page::from_html(html, None::<browser_oxide::stealth::StealthProfile>)
        .await
        .unwrap();
    assert_eq!(
        page.evaluate("getComputedStyle(document.getElementById('el')).fontSize")
            .unwrap(),
        "24px"
    );
}

#[tokio::test]
async fn multiple_rules_last_wins_same_specificity() {
    let html = r#"<!DOCTYPE html><html><head><style>
        div { color: red; }
        div { color: blue; }
    </style></head>
    <body><div id="el"></div></body></html>"#;
    let mut page = Page::from_html(html, None::<browser_oxide::stealth::StealthProfile>)
        .await
        .unwrap();
    assert_eq!(
        page.evaluate("getComputedStyle(document.getElementById('el')).color")
            .unwrap(),
        "rgb(0, 0, 255)"
    );
}

/// `getComputedStyle` takes an `Element`. Chrome throws a `TypeError` for a
/// text node, a comment, a Document or a DocumentFragment — measured on Chrome
/// 154 — and a probe that walks a subtree calling it on every node sees the
/// difference immediately: handed a declaration instead of an exception it
/// reports the subtree as fully measurable.
#[tokio::test]
async fn non_element_arguments_throw_a_type_error() {
    let mut page = Page::from_html(&html(""), None::<browser_oxide::stealth::StealthProfile>)
        .await
        .unwrap();
    let each = r#"
        (function (expr) {
            try { getComputedStyle(eval(expr)); return "ok"; }
            catch (e) { return e.name; }
        })"#;
    for (expr, want) in [
        ("document.createTextNode('x')", "TypeError"),
        ("document.createComment('c')", "TypeError"),
        ("document", "TypeError"),
        ("document.createDocumentFragment()", "TypeError"),
        ("document.body", "ok"),
    ] {
        let got = page.evaluate(&format!("{each}({expr})")).unwrap();
        assert_eq!(got, want, "getComputedStyle({expr})");
    }
}

/// An element that is not in a document has no computed style. Chrome answers
/// an empty declaration: `length` 0, every read "", and the numeric keys gone
/// from the enumeration — 739 named keys against 1223 attached, a difference of
/// exactly the length (measured on Chrome 154).
#[tokio::test]
async fn a_detached_element_has_an_empty_computed_style() {
    let mut page = Page::from_html(
        &html(r#"<div id="in"></div>"#),
        None::<browser_oxide::stealth::StealthProfile>,
    )
    .await
    .unwrap();
    let got = page
        .evaluate(
            r#"(() => {
                const cs = getComputedStyle(document.createElement('div'));
                return [cs.length, cs.item(0), cs.display, cs.color,
                        cs.getPropertyValue('color'),
                        Object.keys(cs).length].join('|');
            })()"#,
        )
        .unwrap();
    assert_eq!(got, "0|||||739", "a detached element reads as empty");

    // Attached, the same call answers a full declaration.
    let attached = page
        .evaluate(
            r#"(() => {
                const cs = getComputedStyle(document.getElementById('in'));
                return [cs.length > 0, cs.display, cs.color].join('|');
            })()"#,
        )
        .unwrap();
    assert_eq!(attached, "true|block|rgb(0, 0, 0)");
}

/// `PublicKeyCredential.getClientCapabilities()` is a detection surface, not a
/// convenience: the `extension:*` block is what a site reads to decide which
/// WebAuthn extensions it may offer, and `conditionalCreate` gates the whole
/// conditional-mediation branch. Measured on Chrome 154 (macOS) — the engine
/// reported `conditionalCreate: false` and none of the fourteen extensions.
#[tokio::test]
async fn webauthn_client_capabilities_match_chrome() {
    let mut page = Page::from_html_with_url(
        &html(""),
        "https://site.example/",
        Some(browser_oxide::stealth::presets::chrome_153_macos()),
    )
    .await
    .unwrap();
    page.evaluate_async(
        "globalThis.__caps = null;\
         PublicKeyCredential.getClientCapabilities()\
           .then((c) => { globalThis.__caps = c; })\
           .catch((e) => { globalThis.__caps = 'ERR ' + e.name; });",
        std::time::Duration::from_secs(5),
    )
    .await
    .ok();
    let got = page
        .evaluate(
            "globalThis.__caps ? JSON.stringify(Object.keys(globalThis.__caps).sort()) : String(globalThis.__caps)",
        )
        .unwrap();
    let want = r#"["conditionalCreate","conditionalGet","extension:appid","extension:appidExclude","extension:cmtgKey","extension:credBlob","extension:credProps","extension:credentialProtectionPolicy","extension:crossDeviceFallbackUrl","extension:enforceCredentialProtectionPolicy","extension:getCredBlob","extension:hmacCreateSecret","extension:largeBlob","extension:minPinLength","extension:payment","extension:prf","hybridTransport","immediateGet","passkeyPlatformAuthenticator","relatedOrigins","signalAllAcceptedCredentials","signalCurrentUserDetails","signalUnknownCredential","userVerifyingPlatformAuthenticator"]"#;
    assert_eq!(got, want);

    // Chrome reports two extensions as unimplemented.
    let off = page
        .evaluate("JSON.stringify([globalThis.__caps['extension:cmtgKey'], globalThis.__caps['extension:crossDeviceFallbackUrl']])")
        .unwrap();
    assert_eq!(off, "[false,false]");
}
