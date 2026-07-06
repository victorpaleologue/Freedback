//! Parse & resolve user input into a canonical feedback target URI.
//!
//! Accepted forms:
//! - **GTIN** (EAN-13, UPC-A/GTIN-12, EAN-8, GTIN-14) with a valid GS1 mod-10
//!   check digit → canonicalized as a GS1 Digital Link
//!   `https://id.gs1.org/01/<gtin14>`.
//! - **ISBN-13** (a 978/979 "Bookland" EAN — simultaneously an ISBN *and* a
//!   GTIN) → resolved as the GS1 Digital Link, with the ISBN noted as an
//!   equivalence (`urn:isbn:` is a job for the collection server's
//!   equivalence index, component 7).
//! - **ISBN-10** (mod-11 check, `X` allowed) → converted to its ISBN-13 /
//!   EAN and resolved the same way.
//! - **http(s) URLs** → passed through unchanged.
//! - **Free text containing a bare GTIN/ISBN** (e.g. a shared product
//!   snippet) → the first valid embedded code is extracted.
//! - Anything else → a typed error.

use serde::Serialize;
use thiserror::Error;

/// Errors from input resolution. `PartialEq` so tests can match exactly.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InputError {
    /// The digits look like a GTIN but the GS1 mod-10 check digit is wrong —
    /// almost always a mistyped barcode.
    #[error("check digit mismatch in {code:?}: expected {expected}, found {found}")]
    CheckDigit {
        code: String,
        expected: char,
        found: char,
    },
    /// The characters look like an ISBN-10 but the mod-11 checksum fails.
    #[error("invalid ISBN-10 checksum in {0:?}")]
    Isbn10Checksum(String),
    /// Nothing recognizable (barcode, ISBN, or URL) in the input.
    #[error("no barcode, ISBN, or URL recognized in {0:?}")]
    Unrecognized(String),
}

/// A successfully resolved input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resolved {
    /// A GTIN, canonicalized as a GS1 Digital Link.
    Gtin {
        /// The zero-padded 14-digit GTIN.
        gtin14: String,
        /// The canonical target URI (`https://id.gs1.org/01/<gtin14>`).
        uri: String,
        /// When the code is ALSO an ISBN (978/979 EAN): its ISBN-13 form.
        /// The GS1 Digital Link stays the canonical target; this records the
        /// equivalence for display and for the collection server's index.
        #[serde(skip_serializing_if = "Option::is_none")]
        isbn13: Option<String>,
    },
    /// An http(s) URL, passed through unchanged.
    Url { uri: String },
}

impl Resolved {
    /// The canonical target URI (what feedback is filed under).
    pub fn uri(&self) -> &str {
        match self {
            Resolved::Gtin { uri, .. } => uri,
            Resolved::Url { uri } => uri,
        }
    }

    /// A short human label for journals and lists.
    pub fn label(&self) -> String {
        match self {
            Resolved::Gtin {
                gtin14,
                isbn13: Some(isbn),
                ..
            } => format!("ISBN {isbn} (GTIN {gtin14})"),
            Resolved::Gtin { gtin14, .. } => format!("GTIN {gtin14}"),
            Resolved::Url { uri } => uri.clone(),
        }
    }
}

/// Resolve any user input to a canonical target. See the module docs for the
/// accepted forms.
pub fn resolve(input: &str) -> Result<Resolved, InputError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(InputError::Unrecognized(input.to_string()));
    }

    // 1. http(s) URLs pass through unchanged.
    if is_http_url(s) {
        return Ok(Resolved::Url { uri: s.to_string() });
    }

    // 2. The whole input is a single code (digits, ISBN hyphens/spaces, or a
    //    trailing X)? Then a bad check digit is an ERROR, not "unrecognized" —
    //    the user clearly meant a code.
    if let Some(code) = normalize_code(s) {
        return resolve_code(&code);
    }

    // 3. Free text: extract the first embedded candidate that validates.
    //    Invalid candidates are skipped (free text is noisy).
    for candidate in extract_candidates(s) {
        if let Ok(resolved) = resolve_code(&candidate) {
            return Ok(resolved);
        }
    }

    Err(InputError::Unrecognized(input.to_string()))
}

/// A conservative http(s)-URL test: correct scheme and no whitespace.
fn is_http_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && !s.chars().any(char::is_whitespace)
        && s.len() > "https://".len()
}

/// Normalize a string that is entirely one code: strip ISBN separators
/// (spaces, hyphens), uppercase, and keep it only if it has a code shape
/// (8/12/13/14 digits, or an ISBN-10: 9 digits + digit/X).
fn normalize_code(s: &str) -> Option<String> {
    let mut cleaned = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '0'..='9' => cleaned.push(c),
            'x' | 'X' => cleaned.push('X'),
            ' ' | '-' | '\u{2010}'..='\u{2015}' => {} // separators
            _ => return None,
        }
    }
    has_code_shape(&cleaned).then_some(cleaned)
}

/// Does a cleaned string have the shape of a GTIN or ISBN-10?
fn has_code_shape(cleaned: &str) -> bool {
    let digits = cleaned.chars().filter(char::is_ascii_digit).count();
    let xs = cleaned.chars().filter(|c| *c == 'X').count();
    match cleaned.len() {
        8 | 12 | 13 | 14 => xs == 0 && digits == cleaned.len(),
        // ISBN-10: nine digits then a digit or X check character.
        10 => digits >= 9 && cleaned[..9].chars().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

/// Candidate codes embedded in free text: maximal runs of digits, `X`/`x`,
/// and separators, cleaned and shape-checked.
fn extract_candidates(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    for c in s.chars().chain(std::iter::once('\n')) {
        match c {
            '0'..='9' | 'x' | 'X' | '-' | ' ' | '\u{2010}'..='\u{2015}' => run.push(c),
            _ => {
                push_candidates_from_run(&run, &mut out);
                run.clear();
            }
        }
    }
    out
}

/// A run may contain leading/trailing separators or stray words' `x`s; try
/// the trimmed run as a whole, then each space-separated piece.
fn push_candidates_from_run(run: &str, out: &mut Vec<String>) {
    let trimmed = run.trim_matches(|c: char| c == ' ' || c == '-');
    if trimmed.is_empty() {
        return;
    }
    if let Some(code) = normalize_code(trimmed) {
        out.push(code);
        return;
    }
    for piece in trimmed.split(' ') {
        if let Some(code) = normalize_code(piece.trim_matches('-')) {
            out.push(code);
        }
    }
}

/// Resolve a cleaned code (digits, possibly a trailing `X` for ISBN-10).
fn resolve_code(code: &str) -> Result<Resolved, InputError> {
    match code.len() {
        10 => resolve_isbn10(code),
        8 | 12 | 13 | 14 if code.chars().all(|c| c.is_ascii_digit()) => resolve_gtin(code),
        _ => Err(InputError::Unrecognized(code.to_string())),
    }
}

/// Validate the GS1 mod-10 check digit and build the GS1 Digital Link.
fn resolve_gtin(digits: &str) -> Result<Resolved, InputError> {
    let expected = gs1_check_digit(&digits[..digits.len() - 1]);
    let found = digits.chars().last().expect("non-empty");
    if found != expected {
        return Err(InputError::CheckDigit {
            code: digits.to_string(),
            expected,
            found,
        });
    }
    let gtin14 = format!("{digits:0>14}");
    // A 978/979 EAN is simultaneously an ISBN-13 and a GTIN ("Bookland").
    // Strip the GTIN-14 zero padding (and a `0` indicator digit) to test.
    let ean13 = gtin14.trim_start_matches('0');
    let isbn13 = (ean13.len() == 13 && (ean13.starts_with("978") || ean13.starts_with("979")))
        .then(|| ean13.to_string());
    Ok(Resolved::Gtin {
        uri: gs1_digital_link(&gtin14),
        gtin14,
        isbn13,
    })
}

/// Validate an ISBN-10 (mod 11, `X` = 10) and convert it to its ISBN-13 /
/// EAN form (`978` + first nine digits + fresh GS1 check digit).
fn resolve_isbn10(code: &str) -> Result<Resolved, InputError> {
    let mut sum: u32 = 0;
    for (i, c) in code.chars().enumerate() {
        let value = match c {
            '0'..='9' => c as u32 - '0' as u32,
            // `X` (=10) is only legal as the final check character.
            'X' if i == 9 => 10,
            _ => return Err(InputError::Isbn10Checksum(code.to_string())),
        };
        sum += (10 - i as u32) * value;
    }
    if sum % 11 != 0 {
        return Err(InputError::Isbn10Checksum(code.to_string()));
    }
    let payload = format!("978{}", &code[..9]);
    let check = gs1_check_digit(&payload);
    let isbn13 = format!("{payload}{check}");
    resolve_gtin(&isbn13)
}

/// The GS1 mod-10 check digit for a data string (the code WITHOUT its check
/// digit): from the rightmost data digit leftwards, weights alternate 3, 1.
fn gs1_check_digit(data: &str) -> char {
    let sum: u32 = data
        .chars()
        .rev()
        .enumerate()
        .map(|(i, c)| {
            let d = c as u32 - '0' as u32;
            if i % 2 == 0 {
                d * 3
            } else {
                d
            }
        })
        .sum();
    char::from_digit((10 - sum % 10) % 10, 10).expect("mod 10 is a digit")
}

/// The canonical GS1 Digital Link for a 14-digit GTIN.
fn gs1_digital_link(gtin14: &str) -> String {
    format!("https://id.gs1.org/01/{gtin14}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gs1_check_digit_known_values() {
        assert_eq!(gs1_check_digit("301762042200"), '3'); // EAN-13
        assert_eq!(gs1_check_digit("03600029145"), '2'); // UPC-A
        assert_eq!(gs1_check_digit("9638507"), '4'); // EAN-8
        assert_eq!(gs1_check_digit("978030640615"), '7'); // ISBN-13 EAN
    }

    #[test]
    fn free_text_candidates_are_extracted() {
        let candidates = extract_candidates("EAN: 3017620422003, batch 42.");
        assert!(candidates.contains(&"3017620422003".to_string()));
    }

    #[test]
    fn free_text_isbn_with_hyphens_is_extracted() {
        let candidates = extract_candidates("see ISBN 978-0-306-40615-7 for details");
        assert!(candidates.contains(&"9780306406157".to_string()));
    }

    #[test]
    fn short_or_odd_digit_runs_are_not_candidates() {
        assert!(extract_candidates("call me at 123 tomorrow").is_empty());
        assert!(extract_candidates("no digits at all").is_empty());
    }

    #[test]
    fn code_shape_rejects_x_in_gtin() {
        assert!(normalize_code("30176204220X3").is_none());
    }
}
