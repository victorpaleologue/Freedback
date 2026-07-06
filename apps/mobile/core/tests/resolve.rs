//! UX paths for input resolution: everything the Home screen's input field
//! (and the share sheet) must handle. Pure — no server.

use freedback_app_core::input::{resolve, InputError, Resolved};
use freedback_app_core::share;

fn gtin(resolved: &Resolved) -> (&str, &str, Option<&str>) {
    match resolved {
        Resolved::Gtin {
            gtin14,
            uri,
            isbn13,
        } => (gtin14.as_str(), uri.as_str(), isbn13.as_deref()),
        other => panic!("expected a GTIN resolution, got {other:?}"),
    }
}

// --- barcodes ---------------------------------------------------------------

#[test]
fn ean13_resolves_to_gs1_digital_link() {
    let r = resolve("3017620422003").unwrap();
    let (gtin14, uri, isbn) = gtin(&r);
    assert_eq!(gtin14, "03017620422003");
    assert_eq!(uri, "https://id.gs1.org/01/03017620422003");
    assert_eq!(isbn, None, "a food EAN is not an ISBN");
}

#[test]
fn upc_a_is_zero_padded_to_gtin14() {
    let r = resolve("036000291452").unwrap();
    let (gtin14, uri, _) = gtin(&r);
    assert_eq!(gtin14, "00036000291452");
    assert_eq!(uri, "https://id.gs1.org/01/00036000291452");
}

#[test]
fn ean8_resolves() {
    let r = resolve("96385074").unwrap();
    let (gtin14, uri, _) = gtin(&r);
    assert_eq!(gtin14, "00000096385074");
    assert_eq!(uri, "https://id.gs1.org/01/00000096385074");
}

#[test]
fn gtin14_resolves_as_is() {
    // EAN-13 3017620422003 with indicator digit 1 → fresh check digit 0.
    let r = resolve("13017620422000").unwrap();
    let (gtin14, uri, _) = gtin(&r);
    assert_eq!(gtin14, "13017620422000");
    assert_eq!(uri, "https://id.gs1.org/01/13017620422000");
}

#[test]
fn surrounding_whitespace_is_tolerated() {
    let r = resolve("  3017620422003\n").unwrap();
    assert_eq!(r.uri(), "https://id.gs1.org/01/03017620422003");
}

// --- ISBN --------------------------------------------------------------------

#[test]
fn isbn13_is_both_isbn_and_gtin() {
    let r = resolve("9780306406157").unwrap();
    let (gtin14, uri, isbn) = gtin(&r);
    assert_eq!(gtin14, "09780306406157");
    assert_eq!(
        uri, "https://id.gs1.org/01/09780306406157",
        "GS1 DL stays canonical"
    );
    assert_eq!(isbn, Some("9780306406157"), "the ISBN equivalence is noted");
}

#[test]
fn isbn13_with_hyphens_resolves() {
    let r = resolve("978-0-306-40615-7").unwrap();
    assert_eq!(r.uri(), "https://id.gs1.org/01/09780306406157");
}

#[test]
fn isbn10_converts_to_isbn13() {
    // 0-306-40615-2 is the ISBN-10 form of 978-0-306-40615-7.
    let r = resolve("0306406152").unwrap();
    let (gtin14, uri, isbn) = gtin(&r);
    assert_eq!(gtin14, "09780306406157");
    assert_eq!(uri, "https://id.gs1.org/01/09780306406157");
    assert_eq!(isbn, Some("9780306406157"));
}

#[test]
fn isbn10_with_x_check_digit() {
    // 0-9752298-0-X (mod-11 check character X = 10).
    let r = resolve("097522980X").unwrap();
    let (_, _, isbn) = gtin(&r);
    assert_eq!(isbn, Some("9780975229804"));
}

#[test]
fn isbn10_lowercase_x_and_hyphens() {
    let r = resolve("0-9752298-0-x").unwrap();
    let (_, _, isbn) = gtin(&r);
    assert_eq!(isbn, Some("9780975229804"));
}

#[test]
fn isbn979_ean_is_an_isbn_too() {
    // 979-10-90636-07-1 (a real 979 ISBN); recompute nothing — trust the code.
    let r = resolve("9791090636071").unwrap();
    let (_, _, isbn) = gtin(&r);
    assert_eq!(isbn, Some("9791090636071"));
}

// --- URLs ---------------------------------------------------------------------

#[test]
fn https_url_passes_through_unchanged() {
    let r = resolve("https://example.com/item/1?a=b#frag").unwrap();
    assert_eq!(
        r,
        Resolved::Url {
            uri: "https://example.com/item/1?a=b#frag".into()
        }
    );
}

#[test]
fn http_url_passes_through() {
    let r = resolve("http://example.com/x").unwrap();
    assert_eq!(r.uri(), "http://example.com/x");
}

// --- free text ------------------------------------------------------------------

#[test]
fn text_with_embedded_gtin_is_extracted() {
    let r = resolve("Just scanned Nutella, EAN 3017620422003 — thoughts?").unwrap();
    assert_eq!(r.uri(), "https://id.gs1.org/01/03017620422003");
}

#[test]
fn text_with_embedded_hyphenated_isbn_is_extracted() {
    let r = resolve("reading ISBN 978-0-306-40615-7 right now").unwrap();
    assert_eq!(r.uri(), "https://id.gs1.org/01/09780306406157");
}

#[test]
fn text_with_an_invalid_code_keeps_scanning() {
    // The first run (a phone number) fails the checksum; the later EAN wins.
    let r = resolve("call 0612345678, product 3017620422003").unwrap();
    assert_eq!(r.uri(), "https://id.gs1.org/01/03017620422003");
}

// --- errors -----------------------------------------------------------------------

#[test]
fn invalid_check_digit_is_a_typed_error() {
    let err = resolve("3017620422004").unwrap_err();
    assert_eq!(
        err,
        InputError::CheckDigit {
            code: "3017620422004".into(),
            expected: '3',
            found: '4',
        }
    );
}

#[test]
fn invalid_isbn10_checksum_is_a_typed_error() {
    let err = resolve("0306406153").unwrap_err();
    assert_eq!(err, InputError::Isbn10Checksum("0306406153".into()));
}

#[test]
fn garbage_is_unrecognized() {
    assert!(matches!(
        resolve("hello world"),
        Err(InputError::Unrecognized(_))
    ));
    assert!(matches!(resolve(""), Err(InputError::Unrecognized(_))));
    assert!(matches!(resolve("   "), Err(InputError::Unrecognized(_))));
    // Too short for any code.
    assert!(matches!(resolve("1234"), Err(InputError::Unrecognized(_))));
}

#[test]
fn non_http_schemes_are_not_urls() {
    assert!(matches!(
        resolve("ftp://example.com/file"),
        Err(InputError::Unrecognized(_))
    ));
}

// --- share normalization ------------------------------------------------------------

#[test]
fn share_deep_link_with_barcode_resolves() {
    let r = share::normalize("freedback://share?text=3017620422003").unwrap();
    assert_eq!(r.uri(), "https://id.gs1.org/01/03017620422003");
}

#[test]
fn share_deep_link_with_urlencoded_url_resolves() {
    let r =
        share::normalize("freedback://share?text=https%3A%2F%2Fexample.com%2Fitem%2F1").unwrap();
    assert_eq!(r.uri(), "https://example.com/item/1");
}

#[test]
fn lookup_app_link_resolves() {
    let r = share::normalize("https://freedback.net/l?q=9780306406157").unwrap();
    assert_eq!(r.uri(), "https://id.gs1.org/01/09780306406157");
}

#[test]
fn plain_shared_text_falls_through_to_resolve() {
    let r = share::normalize("ISBN 978-0-306-40615-7, so good").unwrap();
    assert_eq!(r.uri(), "https://id.gs1.org/01/09780306406157");
}

#[test]
fn share_labels_are_human() {
    let r = resolve("9780306406157").unwrap();
    assert_eq!(r.label(), "ISBN 9780306406157 (GTIN 09780306406157)");
    let r = resolve("3017620422003").unwrap();
    assert_eq!(r.label(), "GTIN 03017620422003");
}
