//! The `display` value Chrome's UA stylesheet gives each HTML element.

/// Chrome's UA `display` for an element named `tag`, `none` when it
/// carries the `hidden` attribute. Every value was measured in Chrome 146.
pub fn ua_display(tag: &str, hidden: bool) -> &'static str {
    if hidden {
        return "none";
    }
    let tag = tag.to_ascii_lowercase();
    UA_DISPLAY
        .iter()
        .find(|(_, tags)| tags.contains(&tag.as_str()))
        .map(|(display, _)| *display)
        .unwrap_or("inline")
}

/// Chrome's UA `display` for `elem`.
pub fn element_ua_display(elem: &crate::dom::node::ElementData) -> &'static str {
    let hidden = elem
        .attrs
        .iter()
        .any(|a| a.name.local.eq_ignore_ascii_case("hidden"));
    ua_display(&elem.name.local, hidden)
}

const UA_DISPLAY: &[(&str, &[&str])] = &[
    (
        "block",
        &[
            "address",
            "article",
            "aside",
            "blockquote",
            "body",
            "center",
            "dd",
            "details",
            "dir",
            "div",
            "dl",
            "dt",
            "fieldset",
            "figcaption",
            "figure",
            "footer",
            "form",
            "frame",
            "frameset",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "header",
            "hgroup",
            "hr",
            "html",
            "legend",
            "listing",
            "main",
            "menu",
            "nav",
            "ol",
            "optgroup",
            "option",
            "p",
            "plaintext",
            "pre",
            "search",
            "section",
            "summary",
            "ul",
            "xmp",
        ],
    ),
    (
        "none",
        &[
            "audio", "base", "datalist", "dialog", "head", "link", "meta", "param", "rp", "script",
            "style", "template", "title",
        ],
    ),
    (
        "inline-block",
        &[
            "button", "input", "marquee", "meter", "progress", "select", "textarea",
        ],
    ),
    ("list-item", &["li"]),
    ("ruby", &["ruby"]),
    ("contents", &["slot"]),
    ("table", &["table"]),
    ("table-caption", &["caption"]),
    ("table-column", &["col"]),
    ("table-column-group", &["colgroup"]),
    ("table-row-group", &["tbody"]),
    ("table-header-group", &["thead"]),
    ("table-footer-group", &["tfoot"]),
    ("table-row", &["tr"]),
    ("table-cell", &["td", "th"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_take_chrome_ua_display() {
        let got: Vec<&str> = ["div", "SPAN", "head", "button", "td", "custom-el"]
            .iter()
            .map(|t| ua_display(t, false))
            .collect();
        assert_eq!(
            got,
            [
                "block",
                "inline",
                "none",
                "inline-block",
                "table-cell",
                "inline"
            ]
        );
        assert_eq!(ua_display("div", true), "none");
    }
}
