//! Web Annotation Protocol collection paging: build an `AnnotationPage` plus the
//! `Link` / `ETag` headers a polite client (and the collection server) rely on.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use axum::http::header::{CACHE_CONTROL, ETAG, LAST_MODIFIED, LINK};
use axum::http::{HeaderMap, HeaderValue};
use freedback_protocol::Annotation;
use serde_json::{json, Value};

use crate::httpdate;

/// A rendered collection page: JSON body + response headers.
pub struct PageView {
    pub body: Value,
    pub headers: HeaderMap,
}

fn collection_url(base: &str, target: Option<&str>) -> String {
    match target {
        Some(t) => format!("{base}/annotations/?target={}", urlencode(t)),
        None => format!("{base}/annotations/"),
    }
}

fn page_url(base: &str, target: Option<&str>, page: usize) -> String {
    let sep = if target.is_some() { "&" } else { "?" };
    format!("{}{sep}page={page}", collection_url(base, target))
}

/// Build the page view (body + `Link`/`ETag` headers).
pub fn build_page(
    base: &str,
    target: Option<&str>,
    page: usize,
    page_size: usize,
    total: usize,
    items: &[Annotation],
    cache_max_age: u64,
) -> PageView {
    let start_index = page.saturating_mul(page_size);
    let canonical = page_url(base, target, page);
    let collection = collection_url(base, target);

    let has_next = page_size > 0 && start_index + items.len() < total;
    let has_prev = page > 0;

    let mut body = json!({
        "@context": "http://www.w3.org/ns/anno.jsonld",
        "id": canonical,
        "type": "AnnotationPage",
        "partOf": { "id": collection, "total": total },
        "startIndex": start_index,
        "items": items,
    });
    if has_next {
        body["next"] = json!(page_url(base, target, page + 1));
    }
    if has_prev {
        body["prev"] = json!(page_url(base, target, page - 1));
    }

    // ETag over (total, ids on this page) — stable across identical content.
    let etag = weak_etag(total, items);

    let mut links = vec![
        format!("<{canonical}>; rel=\"canonical\""),
        "<http://www.w3.org/ns/ldp#Page>; rel=\"type\"".to_string(),
    ];
    if has_next {
        links.push(format!(
            "<{}>; rel=\"next\"",
            page_url(base, target, page + 1)
        ));
    }
    if has_prev {
        links.push(format!(
            "<{}>; rel=\"prev\"",
            page_url(base, target, page - 1)
        ));
    }

    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&links.join(", ")) {
        headers.insert(LINK, v);
    }
    if let Ok(v) = HeaderValue::from_str(&etag) {
        headers.insert(ETAG, v);
    }
    // Freshness: a polite aggregator may reuse the page without revalidating
    // for `max-age` seconds.
    if let Ok(v) = HeaderValue::from_str(&format!("max-age={cache_max_age}")) {
        headers.insert(CACHE_CONTROL, v);
    }
    // Validator: the newest item on the page is the representation's mtime, so a
    // conditional `If-Modified-Since` can earn a cheap 304 even without an ETag.
    if let Some(v) = last_modified(items).and_then(|s| HeaderValue::from_str(&s).ok()) {
        headers.insert(LAST_MODIFIED, v);
    }

    PageView { body, headers }
}

/// The most recent `created` time among `items`, as an HTTP-date — the page's
/// `Last-Modified`. `None` for an empty page (nothing to date).
fn last_modified(items: &[Annotation]) -> Option<String> {
    let newest = items.iter().filter_map(Annotation::iat).max()?;
    httpdate::format(newest)
}

fn weak_etag(total: usize, items: &[Annotation]) -> String {
    let mut h = DefaultHasher::new();
    total.hash(&mut h);
    for a in items {
        a.id.hash(&mut h);
    }
    format!("W/\"{:x}\"", h.finish())
}

/// Minimal percent-encoding for the few characters that break a query value.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
