use std::time::Duration;
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        let profile = browser_oxide::stealth::presets::chrome_153_macos();
        let mut page = browser_oxide::Page::from_html("<html><body>ok</body></html>", Some(profile)).await.unwrap();
        let expr = r#"globalThis.__audit = null; (async () => { globalThis.__audit = JSON.stringify({
            ua: navigator.userAgent,
            webdriver: navigator.webdriver,
            plugins: navigator.plugins.length,
            deviceMemory: navigator.deviceMemory,
            uaData: navigator.userAgentData ? JSON.stringify({brands: navigator.userAgentData.brands, platform: navigator.userAgentData.platform}) : null,
            webgl: (() => { const c = document.createElement('canvas'); const gl = c.getContext('webgl'); if (!gl) return null; const dbg = gl.getExtension('WEBGL_debug_renderer_info'); return dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER); })(),
            screen: [screen.width, screen.height, screen.availWidth, screen.availHeight, screen.colorDepth],
            dpr: devicePixelRatio,
            outer: [outerWidth, outerHeight, innerWidth, innerHeight],
            windowChromeKey: !!window.chrome && Object.keys(window.chrome).slice(0,6),
            permissionsQueryNotification: await (navigator.permissions && navigator.permissions.query({name:'notifications'}).then(p => p.state).catch(e => 'ERR')),
            notifPerm: (typeof Notification !== 'undefined') ? Notification.permission : 'no-ctor',
            storageQuota: await (navigator.storage && navigator.storage.estimate ? navigator.storage.estimate().then(e => e.quota) : 'no-storage'),
            mediaDevices: !!navigator.mediaDevices,
            rtt: navigator.connection ? navigator.connection.rtt : null,
            pdf: navigator.pdfViewerEnabled,
        }); })();"#;
        let _ = page.event_loop().run_until_idle(Duration::from_millis(300)).await;
        if let Err(e) = page.evaluate_async(expr, Duration::from_secs(10)).await {
            eprintln!("EVAL ERR: {e}");
        }
        for _ in 0..60 {
            let _ = page.event_loop().run_until_idle(Duration::from_millis(200)).await;
            if let Ok(s) = page.evaluate("String(globalThis.__audit)") {
                // the promise resolves to the JSON string; wait for it
                let v = s.trim_matches('"');
                if v.starts_with('{') { println!("{v}"); break; }
            }
        }
        page.consume_and_print_logs();
    }).await;
}
