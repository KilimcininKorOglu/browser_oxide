//! CSSOM rule lists: splitting a sheet into rules and serializing them
//! the way Chrome's `CSSRule.cssText` does.

use crate::css_parser::ast::{Declaration, QualifiedRule, Rule};
use crate::css_parser::parse_stylesheet;
use crate::js_runtime::utils::tokens_to_string;

/// One rule as `CSSStyleSheet.cssRules` shows it.
#[derive(Debug, serde::Serialize)]
pub struct CSSRuleJson {
    pub selector_text: String,
    pub css_text: String,
    pub rule_type: u8,
}

/// The rules of a sheet, each as the CSS text of one rule.
pub fn split_rules(css: &str) -> Vec<String> {
    let (sheet, _errors) = parse_stylesheet(css);
    let starts: Vec<usize> = sheet.rules.iter().map(rule_start).collect();
    sheet
        .rules
        .iter()
        .enumerate()
        .filter_map(|(i, rule)| {
            let end = starts.get(i + 1).copied().unwrap_or(css.len());
            rule_text(rule, css.get(starts[i]..end).unwrap_or(""))
        })
        .collect()
}

/// The CSS text of `text` when it holds exactly one rule. Chrome's
/// `insertRule` refuses anything else with a SyntaxError.
pub fn parse_one_rule(text: &str) -> Option<String> {
    let (sheet, _errors) = parse_stylesheet(text);
    match sheet.rules.as_slice() {
        [rule] => rule_text(rule, text),
        _ => None,
    }
}

/// Describe one rule held as CSS text.
pub fn rule_json(text: &str) -> CSSRuleJson {
    let (sheet, _errors) = parse_stylesheet(text);
    match sheet.rules.first() {
        Some(Rule::Qualified(qr)) => CSSRuleJson {
            selector_text: tokens_to_string(&qr.prelude).trim().to_string(),
            css_text: text.to_string(),
            rule_type: 1,
        },
        Some(Rule::At(at)) => CSSRuleJson {
            selector_text: String::new(),
            css_text: text.to_string(),
            rule_type: at_rule_type(at.name),
        },
        None => CSSRuleJson {
            selector_text: String::new(),
            css_text: text.to_string(),
            rule_type: 0,
        },
    }
}

fn rule_start(rule: &Rule) -> usize {
    match rule {
        Rule::Qualified(qr) => qr.loc.offset,
        Rule::At(at) => at.loc.offset,
    }
}

/// A qualified rule is serialized; an at-rule keeps its source text.
fn rule_text(rule: &Rule, source: &str) -> Option<String> {
    match rule {
        Rule::Qualified(qr) => serialize_style_rule(qr),
        Rule::At(_) => Some(source.trim().to_string()),
    }
}

/// `a { color: red; }`, or `b { }` for an empty block.
fn serialize_style_rule(qr: &QualifiedRule) -> Option<String> {
    let selector = tokens_to_string(&qr.prelude).trim().to_string();
    if selector.is_empty() {
        return None;
    }
    let decls: Vec<String> = qr.declarations.iter().map(serialize_declaration).collect();
    if decls.is_empty() {
        return Some(format!("{selector} {{ }}"));
    }
    Some(format!("{selector} {{ {} }}", decls.join(" ")))
}

fn serialize_declaration(d: &Declaration) -> String {
    let value = tokens_to_string(&d.value).trim().to_string();
    if d.important {
        format!("{}: {value} !important;", d.name)
    } else {
        format!("{}: {value};", d.name)
    }
}

/// The legacy `CSSRule.type` constant of an at-rule.
fn at_rule_type(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "import" => 3,
        "media" => 4,
        "font-face" => 5,
        "page" => 6,
        "keyframes" | "-webkit-keyframes" => 7,
        "namespace" => 10,
        "supports" => 12,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_serialized_like_chrome() {
        let rules = split_rules("a { color: red }  .q{margin:0} b{}");
        assert_eq!(rules, ["a { color: red; }", ".q { margin: 0; }", "b { }"]);
    }

    #[test]
    fn at_rules_keep_their_source() {
        let rules = split_rules("@media (min-width: 1px) { a { top: 1px } }\ni { left: 2px }");
        assert_eq!(rules[0], "@media (min-width: 1px) { a { top: 1px } }");
        assert_eq!(rules[1], "i { left: 2px; }");
        assert_eq!(rule_json(&rules[0]).rule_type, 4);
    }

    #[test]
    fn insert_rule_takes_exactly_one_rule() {
        assert_eq!(
            parse_one_rule(".x2{top:1px}").as_deref(),
            Some(".x2 { top: 1px; }")
        );
        assert_eq!(parse_one_rule("i{} j{}"), None);
        assert_eq!(parse_one_rule("this is bad"), None);
        assert_eq!(parse_one_rule(""), None);
    }
}
