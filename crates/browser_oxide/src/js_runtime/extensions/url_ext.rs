//! WHATWG URL parsing for the JS `URL` class.
//!
//! The `url` crate implements the URL Standard's parser and setters, so the
//! JS class holds no parsing logic of its own. Both ops answer the URL as
//! eleven strings in the order of [`parts`].

use deno_core::op2;
use url::Url;

/// href, origin, protocol, username, password, host, hostname, port,
/// pathname, search, hash: the order the JS class reads them in.
fn parts(u: &Url) -> Vec<String> {
    let hostname = u.host_str().unwrap_or("").to_string();
    let port = u.port().map(|p| p.to_string()).unwrap_or_default();
    let host = if port.is_empty() {
        hostname.clone()
    } else {
        format!("{hostname}:{port}")
    };
    vec![
        u.as_str().to_string(),
        u.origin().ascii_serialization(),
        format!("{}:", u.scheme()),
        u.username().to_string(),
        u.password().unwrap_or("").to_string(),
        host,
        hostname,
        port,
        u.path().to_string(),
        prefixed('?', u.query()),
        prefixed('#', u.fragment()),
    ]
}

/// A query or fragment as the URL API shows it: empty when missing or empty.
fn prefixed(mark: char, value: Option<&str>) -> String {
    match value {
        Some(v) if !v.is_empty() => format!("{mark}{v}"),
        _ => String::new(),
    }
}

/// The part of a setter value before the first character that ends a host.
fn host_prefix(value: &str) -> &str {
    let end = value.find(['/', '?', '#', '\\']).unwrap_or(value.len());
    &value[..end]
}

/// Splits `host[:port]`, keeping the brackets of an IPv6 host whole.
fn split_host_port(value: &str) -> (&str, &str) {
    let value = host_prefix(value);
    let colon = match value.rfind(']') {
        Some(close) => value[close..].find(':').map(|i| i + close),
        None => value.find(':'),
    };
    match colon {
        Some(i) => (&value[..i], &value[i + 1..]),
        None => (value, ""),
    }
}

fn set_port(u: &mut Url, value: &str) {
    if value.is_empty() {
        let _ = u.set_port(None);
        return;
    }
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    if let Ok(port) = digits.parse::<u16>() {
        let _ = u.set_port(Some(port));
    }
}

fn set_host(u: &mut Url, value: &str) {
    let (host, port) = split_host_port(value);
    if u.set_host(Some(host)).is_ok() && !port.is_empty() {
        set_port(u, port);
    }
}

fn set_hostname(u: &mut Url, value: &str) {
    let host = split_host_port(value).0;
    let _ = u.set_host(Some(host));
}

fn set_password(u: &mut Url, value: &str) {
    let _ = u.set_password(if value.is_empty() { None } else { Some(value) });
}

fn set_query(u: &mut Url, value: &str) {
    let query = value.strip_prefix('?').unwrap_or(value);
    u.set_query(if query.is_empty() { None } else { Some(query) });
}

fn set_fragment(u: &mut Url, value: &str) {
    let fragment = value.strip_prefix('#').unwrap_or(value);
    u.set_fragment(if fragment.is_empty() {
        None
    } else {
        Some(fragment)
    });
}

/// Applies one URL API setter. A value the Standard rejects leaves the URL as it was.
fn apply_setter(u: &mut Url, name: &str, value: &str) {
    match name {
        "protocol" => {
            let _ = u.set_scheme(value.split(':').next().unwrap_or(""));
        }
        "username" => {
            let _ = u.set_username(value);
        }
        "password" => set_password(u, value),
        "host" => set_host(u, value),
        "hostname" => set_hostname(u, value),
        "port" => set_port(u, value),
        "pathname" => u.set_path(value),
        "search" => set_query(u, value),
        "hash" => set_fragment(u, value),
        _ => {}
    }
}

/// Parses `input`, against `base` when `has_base` is set. `None` is a failure.
#[op2]
#[serde]
pub fn op_url_parse(
    #[string] input: String,
    #[string] base: String,
    has_base: bool,
) -> Option<Vec<String>> {
    let parsed = if has_base {
        Url::parse(&base).ok()?.join(&input)
    } else {
        Url::parse(&input)
    };
    parsed.ok().map(|u| parts(&u))
}

/// Sets one component of an already parsed `href` and answers the result.
#[op2]
#[serde]
pub fn op_url_set(
    #[string] href: String,
    #[string] name: String,
    #[string] value: String,
) -> Option<Vec<String>> {
    let mut url = Url::parse(&href).ok()?;
    apply_setter(&mut url, &name, &value);
    Some(parts(&url))
}

deno_core::extension!(url_extension, ops = [op_url_parse, op_url_set],);

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Vec<String> {
        parts(&Url::parse(input).unwrap())
    }

    fn set(input: &str, name: &str, value: &str) -> String {
        let mut url = Url::parse(input).unwrap();
        apply_setter(&mut url, name, value);
        url.to_string()
    }

    #[test]
    fn parts_follow_the_url_api() {
        let p = parse("https://u:p@X.com:8080/a b?q=1#h");
        assert_eq!(p[0], "https://u:p@x.com:8080/a%20b?q=1#h");
        assert_eq!(p[1], "https://x.com:8080");
        assert_eq!(p[2], "https:");
        assert_eq!((p[3].as_str(), p[4].as_str()), ("u", "p"));
        assert_eq!(
            (p[5].as_str(), p[6].as_str(), p[7].as_str()),
            ("x.com:8080", "x.com", "8080")
        );
        assert_eq!(
            (p[8].as_str(), p[9].as_str(), p[10].as_str()),
            ("/a%20b", "?q=1", "#h")
        );
    }

    #[test]
    fn an_opaque_url_has_a_null_origin_and_no_host() {
        let p = parse("data:text/plain,hi");
        assert_eq!(
            (p[1].as_str(), p[2].as_str(), p[5].as_str()),
            ("null", "data:", "")
        );
    }

    #[test]
    fn setters_apply_the_standards_rules() {
        let base = "https://x.com:8080/a?q=1#h";
        assert_eq!(set(base, "host", "y.org:99"), "https://y.org:99/a?q=1#h");
        assert_eq!(set(base, "host", "y.org"), "https://y.org:8080/a?q=1#h");
        assert_eq!(
            set(base, "hostname", "y.org:99"),
            "https://y.org:8080/a?q=1#h"
        );
        assert_eq!(set(base, "port", ""), "https://x.com/a?q=1#h");
        assert_eq!(set(base, "port", "443x"), "https://x.com/a?q=1#h");
        assert_eq!(set(base, "search", ""), "https://x.com:8080/a#h");
        assert_eq!(set(base, "search", "k=2"), "https://x.com:8080/a?k=2#h");
        assert_eq!(set(base, "hash", ""), "https://x.com:8080/a?q=1");
        assert_eq!(set(base, "protocol", "http:"), "http://x.com:8080/a?q=1#h");
        assert_eq!(set(base, "protocol", "data"), base);
    }
}
