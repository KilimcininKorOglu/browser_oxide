//! Web IDL gap probe.
//!
//! Reads a JSON map `{ "<Interface>": { "inherits": <name|null>, "members": [..] } }`
//! and reports which Window-exposed interfaces, and which members of them, the
//! engine does not expose. It runs offline against an `about:blank` page.
//!
//!   cargo run --release -p browser_oxide --example idl_gap_probe -- <idl.json>
//!
//! A path ending in `.js` is evaluated as a script instead and its result is printed.

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: idl_gap_probe <idl.json>");
    let idl = std::fs::read_to_string(&path).expect("cannot read the IDL json");
    let eval_only = path.ends_with(".js");
    let profile = browser_oxide::stealth::presets::chrome_148_macos();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let mut page = browser_oxide::Page::from_html("<html><body></body></html>", Some(profile))
                .await
                .expect("cannot create the page");
            if eval_only {
                match page.evaluate(&idl) {
                    Ok(out) => println!("{out}"),
                    Err(e) => println!("EVAL ERROR: {e}"),
                }
                return;
            }
            let script = format!(
                r#"(function () {{
                    var idl = {idl};
                    var out = {{ missingInterfaces: [], missingMembers: {{}} }};
                    Object.keys(idl).forEach(function (name) {{
                        var ctor = globalThis[name];
                        if (typeof ctor !== 'function') {{ out.missingInterfaces.push(name); return; }}
                        var miss = idl[name].members.filter(function (m) {{
                            return !(m in ctor.prototype) && !(m in ctor);
                        }});
                        if (miss.length) out.missingMembers[name] = miss;
                    }});
                    return JSON.stringify(out);
                }})()"#
            );
            match page.evaluate(&script) {
                Ok(json) => println!("{json}"),
                Err(e) => println!("EVAL ERROR: {e}"),
            }
        })
        .await;
}
