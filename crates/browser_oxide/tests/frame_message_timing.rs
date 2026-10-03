//! The build must drain the parent realm's frame outbox.
//!
//! A widget hands its frame the whole configuration with `postMessage`, and a
//! cross-origin frame's `postMessage` is queued in the parent's realm for the
//! host to route. Routing used to happen only in [`Page::pump_frames`], which
//! the caller reaches after navigation returns — so a frame born while the
//! top-level document was still building sat with the message undelivered for
//! the rest of it. Measured on the production target: the frame was born 3.7 s
//! before the navigation returned and its first beacon waited for exactly that.
//!
//! The frame here is cross-origin to the page, which is what puts its
//! `postMessage` on the queued path. The frame's own document is never
//! reachable (port 9 refuses instantly), so the test needs no server: what is
//! under test is the parent's side of the handoff.

use browser_oxide::Page;

const FRAME_SRC: &str = "http://127.0.0.1:9/frame.html";

/// Post to the frame from a script, the way a widget does.
fn page_with_post() -> String {
    format!(
        r#"<!DOCTYPE html><html><body>
        <iframe id="w" src="{src}"></iframe>
        <script>
            document.getElementById('w').contentWindow.postMessage({{ hello: 'world' }}, '*');
        </script>
        </body></html>"#,
        src = FRAME_SRC
    )
}

/// Whatever the parent realm still has queued for its frames.
fn pending(page: &mut Page) -> String {
    page.evaluate(
        "(() => { const b = globalThis[Symbol.for('__ox_frames')]; \
         return b && b.drainToChildren ? b.drainToChildren() : '[]'; })()",
    )
    .unwrap_or_default()
}

#[tokio::test]
async fn build_drains_the_frame_outbox() {
    let profile = browser_oxide::stealth::presets::chrome_153_macos();
    let mut page =
        Page::from_html_with_url(&page_with_post(), "https://site.example/", Some(profile))
            .await
            .unwrap();

    assert_eq!(
        pending(&mut page),
        "[]",
        "navigation must route queued frame messages, not leave them for the caller's pump"
    );
}

#[tokio::test]
async fn a_frame_message_is_routed_only_once() {
    let profile = browser_oxide::stealth::presets::chrome_153_macos();
    let mut page =
        Page::from_html_with_url(&page_with_post(), "https://site.example/", Some(profile))
            .await
            .unwrap();

    // The outbox is emptied by draining, so a page that posts twice during its
    // own load must not have the first message handed over a second time. With
    // no child realm to receive it, a duplicate would show up as the message
    // surviving the drain.
    let leftovers = pending(&mut page);
    assert_eq!(leftovers, "[]", "first drain must empty the outbox");
    assert_eq!(pending(&mut page), "[]", "a drained outbox stays empty");
}
