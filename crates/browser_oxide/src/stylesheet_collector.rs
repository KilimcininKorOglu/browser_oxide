use crate::dom::node::{NodeData, NodeId};
use crate::dom::Dom;

/// A stylesheet entry — either inline CSS or an external URL to fetch.
#[derive(Debug, Clone)]
pub enum StylesheetEntry {
    /// CSS text from a `<style>` block.
    Inline(String),
    /// href URL from `<link rel="stylesheet" href="...">`.
    External(String),
}

/// Find all stylesheets in the DOM: `<style>` blocks and `<link rel="stylesheet">` tags.
/// Returns entries in document order. Mirrors the `script_runner::find_scripts` pattern.
pub fn find_stylesheets(dom: &Dom) -> Vec<StylesheetEntry> {
    let mut entries = Vec::new();
    collect_stylesheets(dom, NodeId::DOCUMENT, &mut entries);
    entries
}

/// The connected `<style>` elements that carry CSS, in document order.
/// Each owns a stylesheet, empty ones included.
pub fn style_elements(dom: &Dom) -> Vec<NodeId> {
    let mut found = Vec::new();
    collect_style_elements(dom, NodeId::DOCUMENT, &mut found);
    found
}

fn collect_style_elements(dom: &Dom, node_id: NodeId, found: &mut Vec<NodeId>) {
    for child_id in dom.children(node_id) {
        if let Some(NodeData::Element(elem)) = dom.get(child_id).map(|n| &n.data) {
            if is_css_style(elem) {
                found.push(child_id);
            }
        }
        collect_style_elements(dom, child_id, found);
    }
}

/// A `<style>` element whose type is CSS.
fn is_css_style(elem: &crate::dom::node::ElementData) -> bool {
    if !elem.name.local.eq_ignore_ascii_case("style") {
        return false;
    }
    let type_attr = elem
        .attrs
        .iter()
        .find(|a| a.name.local == "type")
        .map(|a| a.value.as_str());
    matches!(type_attr, None | Some("text/css") | Some(""))
}

fn collect_stylesheets(dom: &Dom, node_id: NodeId, entries: &mut Vec<StylesheetEntry>) {
    let children = dom.children(node_id);
    for child_id in children {
        if let Some(node) = dom.get(child_id) {
            if let NodeData::Element(elem) = &node.data {
                // <style> blocks
                if is_css_style(elem) {
                    let css = dom.text_content(child_id);
                    if !css.trim().is_empty() {
                        entries.push(StylesheetEntry::Inline(css));
                    }
                }

                // <link rel="stylesheet" href="...">
                if elem.name.local.eq_ignore_ascii_case("link") {
                    let is_stylesheet = elem.attrs.iter().any(|a| {
                        a.name.local.eq_ignore_ascii_case("rel")
                            && a.value.to_lowercase().contains("stylesheet")
                    });
                    if is_stylesheet {
                        if let Some(href) = elem
                            .attrs
                            .iter()
                            .find(|a| a.name.local.eq_ignore_ascii_case("href"))
                            .map(|a| a.value.clone())
                        {
                            if !href.trim().is_empty() {
                                entries.push(StylesheetEntry::External(href));
                            }
                        }
                    }
                }
            }
            collect_stylesheets(dom, child_id, entries);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_inline_style() {
        let dom = crate::html_parser::parse_html(
            "<html><head><style>.a { color: red; }</style></head><body></body></html>",
        );
        let entries = find_stylesheets(&dom);
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], StylesheetEntry::Inline(css) if css.contains("color: red")));
    }

    #[test]
    fn finds_link_stylesheet() {
        let dom = crate::html_parser::parse_html(
            r#"<html><head><link rel="stylesheet" href="/style.css"></head><body></body></html>"#,
        );
        let entries = find_stylesheets(&dom);
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], StylesheetEntry::External(href) if href == "/style.css"));
    }

    #[test]
    fn finds_both_in_order() {
        let dom = crate::html_parser::parse_html(
            r#"<html><head>
                <link rel="stylesheet" href="/a.css">
                <style>.b { color: blue; }</style>
                <link rel="stylesheet" href="/c.css">
            </head><body></body></html>"#,
        );
        let entries = find_stylesheets(&dom);
        assert_eq!(entries.len(), 3);
        assert!(matches!(&entries[0], StylesheetEntry::External(h) if h == "/a.css"));
        assert!(matches!(&entries[1], StylesheetEntry::Inline(_)));
        assert!(matches!(&entries[2], StylesheetEntry::External(h) if h == "/c.css"));
    }

    #[test]
    fn ignores_non_stylesheet_links() {
        let dom = crate::html_parser::parse_html(
            r#"<html><head><link rel="icon" href="/favicon.ico"></head><body></body></html>"#,
        );
        let entries = find_stylesheets(&dom);
        assert!(entries.is_empty());
    }

    #[test]
    fn style_elements_keeps_empty_css_styles_only() {
        let dom = crate::html_parser::parse_html(
            r#"<html><head><style></style><style type="text/x-template">a{}</style>
                <style type="text/css">.b { color: blue }</style></head><body></body></html>"#,
        );
        let nodes = style_elements(&dom);
        assert_eq!(nodes.len(), 2);
        assert_eq!(dom.text_content(nodes[0]), "");
        assert!(dom.text_content(nodes[1]).contains("color: blue"));
    }
}
