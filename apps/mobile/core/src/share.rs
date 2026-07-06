//! Normalize incoming shares and deep links into [`input::resolve`] results.
//!
//! The Android share bridge works like this (see `src-tauri/gen/android`):
//! `ShareActivity` receives `ACTION_SEND` / `ACTION_PROCESS_TEXT` text,
//! rewrites it as a `freedback://share?text=<urlencoded>` VIEW intent aimed at
//! `MainActivity`, and finishes. The `tauri-plugin-deep-link` plugin delivers
//! that URI to Rust, which stores the decoded text as the pending share and
//! emits a `share` event. Lookup App Links (`https://freedback.net/l?q=…`)
//! travel the same road.

use crate::input::{self, InputError, Resolved};

/// The deep-link scheme the Android share bridge uses.
pub const SHARE_SCHEME: &str = "freedback";
/// The https App-Link lookup prefix (`https://freedback.net/l`).
pub const LOOKUP_PREFIX: &str = "https://freedback.net/l";

/// Normalize any shared string — a `freedback://share?text=…` deep link, a
/// `https://freedback.net/l?q=…` lookup link, or plain shared text — and
/// resolve it to a canonical target.
pub fn normalize(input: &str) -> Result<Resolved, InputError> {
    let s = input.trim();
    if let Some(text) = extract_share_text(s) {
        return input::resolve(&text);
    }
    input::resolve(s)
}

/// If `url` is one of our deep-link forms, extract the shared/looked-up text.
/// Returns `None` for anything else (including plain shared text).
pub fn extract_share_text(url: &str) -> Option<String> {
    let s = url.trim();
    if let Some(rest) = s.strip_prefix("freedback://") {
        // freedback://share?text=<urlencoded>  (also tolerate ?q=)
        let query = rest.split_once('?').map(|(_, q)| q).unwrap_or("");
        return query_param(query, "text").or_else(|| query_param(query, "q"));
    }
    if let Some(rest) = strip_prefix_ci(s, LOOKUP_PREFIX) {
        // https://freedback.net/l?q=<urlencoded>  or  /l/<urlencoded-path>
        if let Some(query) = rest.split_once('?').map(|(_, q)| q) {
            if let Some(text) = query_param(query, "q").or_else(|| query_param(query, "text")) {
                return Some(text);
            }
        }
        let path = rest.split('?').next().unwrap_or("");
        let path = path.trim_start_matches('/');
        if !path.is_empty() {
            return Some(percent_decode(path));
        }
    }
    None
}

/// Case-insensitive prefix strip (URL schemes/hosts are case-insensitive).
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Find a query parameter and percent-decode its value.
fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| percent_decode(v))
    })
}

/// Decode %XX escapes and `+` (form-encoded spaces). Invalid escapes pass
/// through literally — shares are best-effort text, never fatal.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => match (hex_val(bytes.get(i + 1)), hex_val(bytes.get(i + 2))) {
                (Some(hi), Some(lo)) => {
                    out.push(hi * 16 + lo);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: Option<&u8>) -> Option<u8> {
    match b? {
        b @ b'0'..=b'9' => Some(b - b'0'),
        b @ b'a'..=b'f' => Some(b - b'a' + 10),
        b @ b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_roundtrips_typical_shares() {
        assert_eq!(percent_decode("3017620422003"), "3017620422003");
        assert_eq!(
            percent_decode("https%3A%2F%2Fexample.com%2Fitem%2F1"),
            "https://example.com/item/1"
        );
        assert_eq!(percent_decode("a+b%20c"), "a b c");
        assert_eq!(percent_decode("100%"), "100%"); // invalid escape passes through
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn extract_share_text_from_deep_link() {
        assert_eq!(
            extract_share_text("freedback://share?text=3017620422003").as_deref(),
            Some("3017620422003")
        );
        assert_eq!(
            extract_share_text("freedback://share?text=ISBN%20978-0-306-40615-7").as_deref(),
            Some("ISBN 978-0-306-40615-7")
        );
        assert_eq!(extract_share_text("freedback://share").as_deref(), None);
        assert_eq!(extract_share_text("plain text"), None);
        assert_eq!(extract_share_text("https://example.com/x"), None);
    }

    #[test]
    fn extract_share_text_from_lookup_app_link() {
        assert_eq!(
            extract_share_text("https://freedback.net/l?q=9780306406157").as_deref(),
            Some("9780306406157")
        );
        assert_eq!(
            extract_share_text("https://freedback.net/l/9780306406157").as_deref(),
            Some("9780306406157")
        );
    }
}
