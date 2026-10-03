//! CSSOM rule lists: splitting a sheet into rules and serializing them
//! the way Chrome's `CSSRule.cssText` does.

use crate::css_parser::ast::{AtRule, Declaration, QualifiedRule, Rule};
use crate::css_parser::parse_stylesheet;
use crate::js_runtime::utils::tokens_to_string;

/// One rule as `CSSStyleSheet.cssRules` shows it.
#[derive(Debug, serde::Serialize)]
pub struct CSSRuleJson {
    pub selector_text: String,
    pub css_text: String,
    pub rule_type: u8,
    /// The `@keyframes` steps, empty for every other rule.
    ///
    /// Chrome lists them through `CSSKeyframesRule.cssRules`, and a sheet
    /// made only of `@keyframes` — which is what a challenge widget ships —
    /// is unreadable without them: the walk stops on the first `undefined`.
    #[serde(default)]
    pub keyframes: Vec<CSSKeyframeJson>,
}

/// One `@keyframes` step, as `CSSKeyframesRule.cssRules` shows it.
#[derive(Debug, serde::Serialize)]
pub struct CSSKeyframeJson {
    /// The selector: `"from"`, `"to"` or a percentage list.
    pub key_text: String,
    pub css_text: String,
    pub declarations: Vec<(String, String)>,
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
            keyframes: Vec::new(),
        },
        Some(Rule::At(at)) => CSSRuleJson {
            selector_text: String::new(),
            css_text: text.to_string(),
            rule_type: at_rule_type(at.name),
            keyframes: keyframe_steps(at),
        },
        None => CSSRuleJson {
            selector_text: String::new(),
            css_text: text.to_string(),
            rule_type: 0,
            keyframes: Vec::new(),
        },
    }
}

/// `from` / `to` become the percentages Chrome reports.
fn normalize_key_text(raw: &str) -> String {
    raw.split(',')
        .map(|part| match part.trim().to_ascii_lowercase().as_str() {
            "from" => "0%".to_string(),
            "to" => "100%".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Chrome serializes a value with a space after every top-level comma:
/// `scale3d(1,1,1)` reads back as `scale3d(1, 1, 1)`.
fn normalize_value(raw: &str) -> String {
    let trimmed = raw.trim();
    let mut out = String::with_capacity(trimmed.len() + 8);
    let mut in_string: Option<char> = None;
    let mut prev = '\0';
    for ch in trimmed.chars() {
        match in_string {
            Some(q) => {
                out.push(ch);
                if ch == q && prev != '\\' {
                    in_string = None;
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    in_string = Some(ch);
                    out.push(ch);
                } else if ch == ',' {
                    out.push(',');
                    if !out.ends_with(", ") && !trimmed[out.len()..].starts_with(' ') {
                        out.push(' ');
                    }
                } else {
                    out.push(ch);
                }
            }
        }
        prev = ch;
    }
    out.trim_end().to_string()
}

/// True for the at-rules whose block holds `@keyframes` steps.
fn is_keyframes(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "keyframes" | "-webkit-keyframes"
    )
}

/// The steps of a `@keyframes` rule, in source order.
///
/// Chrome serializes each one the CSSOM way — the selector as written and the
/// declarations normalized and terminated — and that text is what a rule walk
/// collects, so the raw source slice is not enough here.
fn keyframe_steps(at: &AtRule<'_>) -> Vec<CSSKeyframeJson> {
    if !is_keyframes(at.name) {
        return Vec::new();
    }
    let Some(crate::css_parser::ast::Block::RuleList(rules)) = &at.block else {
        return Vec::new();
    };
    rules
        .iter()
        .filter_map(|rule| match rule {
            Rule::Qualified(qr) => {
                let raw_key = tokens_to_string(&qr.prelude).trim().to_string();
                if raw_key.is_empty() {
                    return None;
                }
                // Chrome reports the percentages, never the keywords: a step
                // written `from` comes back as `0%` and `to` as `100%`.
                let key_text = normalize_key_text(&raw_key);
                let declarations: Vec<(String, String)> = qr
                    .declarations
                    .iter()
                    .map(|d| {
                        (
                            d.name.to_string(),
                            normalize_value(&tokens_to_string(&d.value)),
                        )
                    })
                    .collect();
                let body: Vec<String> = declarations
                    .iter()
                    .map(|(n, v)| format!("{n}: {v};"))
                    .collect();
                let css_text = if body.is_empty() {
                    format!("{key_text} {{ }}")
                } else {
                    format!("{key_text} {{ {} }}", body.join(" "))
                };
                Some(CSSKeyframeJson {
                    key_text,
                    css_text,
                    declarations,
                })
            }
            Rule::At(_) => None,
        })
        .collect()
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
    fn keyframes_expose_their_steps() {
        let r = rule_json("@keyframes spin{from{opacity:0}to{opacity:1}}");
        assert_eq!(r.rule_type, 7);
        let steps: Vec<&str> = r.keyframes.iter().map(|k| k.key_text.as_str()).collect();
        assert_eq!(steps, ["0%", "100%"], "keywords become percentages");
        assert_eq!(r.keyframes[1].key_text, "100%");
        assert_eq!(r.keyframes[1].css_text, "100% { opacity: 1; }");
        assert_eq!(r.keyframes[0].key_text, "0%");
        assert_eq!(
            r.keyframes[0].declarations,
            [("opacity".to_string(), "0".to_string())]
        );
        assert!(rule_json("@media screen{a{top:0}}").keyframes.is_empty());
        // Chrome spaces the arguments of a function value.
        let r = rule_json("@keyframes s{50%{transform:scale3d(1,1,1)}}");
        assert_eq!(
            r.keyframes[0].css_text,
            "50% { transform: scale3d(1, 1, 1); }"
        );
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
