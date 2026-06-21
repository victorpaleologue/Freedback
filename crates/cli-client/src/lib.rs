//! Freedback basic client (component 4): read / write / sync over endpoints and
//! files, on **native and wasm32**.
//!
//! The client distinguishes **collection points** (where aggregates are read)
//! from **publication points** (where annotations are POSTed) as distinct
//! types, and abstracts I/O behind the [`Transport`] trait so the same code path
//! works against a file fixture and a live endpoint. On wasm, `reqwest`
//! delegates to the browser Fetch API; filesystem access is native-only.

use async_trait::async_trait;
use freedback_protocol::Annotation;
use serde_json::Value;
use thiserror::Error;

/// Client errors.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("http error: {0}")]
    Http(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protocol: {0}")]
    Protocol(#[from] freedback_protocol::Error),
    #[error("io: {0}")]
    Io(String),
    #[error("unsupported in this build: {0}")]
    Unsupported(&'static str),
}

/// Client result alias.
pub type Result<T> = std::result::Result<T, ClientError>;

/// HTTP transport. `?Send` so the same trait works with browser futures.
#[async_trait(?Send)]
pub trait Transport {
    /// GET a URL and return the response body as text.
    async fn get(&self, url: &str) -> Result<String>;
    /// POST a JSON body (as `application/ld+json`) and return the response text.
    async fn post_json(&self, url: &str, body: &str, bearer: Option<&str>) -> Result<String>;
}

/// A `reqwest`-backed transport (native + wasm/Fetch).
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl ReqwestTransport {
    /// Create a transport with a default client.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait(?Send)]
impl Transport for ReqwestTransport {
    async fn get(&self, url: &str) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        if !status.is_success() {
            return Err(ClientError::Http(format!("{status}: {text}")));
        }
        Ok(text)
    }

    async fn post_json(&self, url: &str, body: &str, bearer: Option<&str>) -> Result<String> {
        let mut req = self
            .client
            .post(url)
            .header("content-type", "application/ld+json")
            .body(body.to_string());
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        if !status.is_success() {
            return Err(ClientError::Http(format!("{status}: {text}")));
        }
        Ok(text)
    }
}

/// A point from which aggregates are READ (a feedback server or a collection
/// server's index).
#[derive(Debug, Clone)]
pub struct CollectionPoint {
    /// URL that accepts `?target=` and returns an `AnnotationPage` or array.
    pub annotations_url: String,
    /// URL of the `/sync` cursor (empty if not a feedback server).
    pub sync_url: String,
}

impl CollectionPoint {
    /// Build from a feedback-server base URL (`http://host:port`).
    pub fn from_server(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            annotations_url: format!("{base}/annotations/"),
            sync_url: format!("{base}/sync"),
        }
    }
    /// Build from a collection-server index URL (no sync cursor).
    pub fn from_index(url: &str) -> Self {
        Self {
            annotations_url: url.to_string(),
            sync_url: String::new(),
        }
    }
}

/// A point to which annotations are PUBLISHED (POSTed).
#[derive(Debug, Clone)]
pub struct PublicationPoint {
    /// The POST-to-container URL.
    pub url: String,
}

impl PublicationPoint {
    /// Build from a feedback-server base URL.
    pub fn from_server(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            url: format!("{base}/annotations/"),
        }
    }
}

/// Where to read from.
pub enum Source {
    /// A live collection point.
    Endpoint(CollectionPoint),
    /// A local file (native only).
    File(String),
}

/// Where to write to.
pub enum Dest {
    /// A live publication point (optionally with an OAuth bearer token).
    Endpoint {
        point: PublicationPoint,
        bearer: Option<String>,
    },
    /// A local file (native only): appends to a JSON array.
    File(String),
}

/// The Freedback client, generic over a [`Transport`].
pub struct Client<T: Transport> {
    transport: T,
}

impl<T: Transport> Client<T> {
    /// Build a client over a transport.
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Read annotations for `target` from a source. The same call works against
    /// a file fixture and a live endpoint.
    pub async fn read(&self, target: &str, source: &Source) -> Result<Vec<Annotation>> {
        let text = match source {
            Source::Endpoint(point) => {
                let url = format!("{}?target={}", point.annotations_url, urlencode(target));
                self.transport.get(&url).await?
            }
            Source::File(path) => read_file(path)?,
        };
        parse_annotations(&text)
    }

    /// Publish an annotation to a destination. Returns the stored annotation
    /// (with its server id) for endpoints.
    pub async fn write(&self, ann: &Annotation, dest: &Dest) -> Result<Annotation> {
        match dest {
            Dest::Endpoint { point, bearer } => {
                let body = serde_json::to_string(ann)?;
                let text = self
                    .transport
                    .post_json(&point.url, &body, bearer.as_deref())
                    .await?;
                Ok(serde_json::from_str(&text)?)
            }
            Dest::File(path) => {
                append_file(path, ann)?;
                Ok(ann.clone())
            }
        }
    }

    /// One round of NIP-77-style negentropy reconciliation against a server's
    /// `POST /negentropy`. Posts the client's range message for `target` and
    /// returns the server's reply. The server is read-only over its set, so the
    /// round is a plain stateless HTTP batch call (INVARIANT 7).
    pub async fn negentropy_round(
        &self,
        point: &CollectionPoint,
        target: &str,
        message: &freedback_protocol::Message,
    ) -> Result<freedback_protocol::Message> {
        let url = negentropy_url(&point.sync_url)?;
        let body = serde_json::to_string(&serde_json::json!({
            "target": target,
            "message": message,
        }))?;
        let text = self.transport.post_json(&url, &body, None).await?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Bulk-fetch annotations by dedup id from a server's `POST
    /// /annotations/by-id`. The reconcile path fetches only the `need` ids
    /// negentropy identified, keeping the transfer O(diff).
    pub async fn fetch_by_id(
        &self,
        point: &CollectionPoint,
        ids: &[String],
    ) -> Result<Vec<Annotation>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let url = by_id_url(&point.annotations_url);
        let body = serde_json::to_string(&serde_json::json!({ "ids": ids }))?;
        let text = self.transport.post_json(&url, &body, None).await?;
        parse_annotations(&text)
    }

    /// Incremental sync against a feedback server's `/sync` cursor.
    pub async fn sync(
        &self,
        point: &CollectionPoint,
        target: &str,
        gt_iat: i64,
        latest_edits_only: bool,
    ) -> Result<Vec<Annotation>> {
        if point.sync_url.is_empty() {
            return Err(ClientError::Unsupported(
                "collection point has no /sync cursor",
            ));
        }
        let url = format!(
            "{}?target={}&gt_iat={}&latest_edits_only={}",
            point.sync_url,
            urlencode(target),
            gt_iat,
            latest_edits_only
        );
        let text = self.transport.get(&url).await?;
        parse_annotations(&text)
    }
}

/// Parse either an `AnnotationPage` (`{items:[...]}`) or a bare array.
fn parse_annotations(text: &str) -> Result<Vec<Annotation>> {
    let value: Value = serde_json::from_str(text)?;
    let items = match value {
        Value::Array(arr) => arr,
        Value::Object(mut obj) => match obj.remove("items") {
            Some(Value::Array(arr)) => arr,
            _ => return Err(ClientError::Http("response had no items array".into())),
        },
        _ => return Err(ClientError::Http("unexpected response shape".into())),
    };
    items
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(ClientError::from))
        .collect()
}

/// Derive the `/negentropy` endpoint from a feedback server's `/sync` cursor
/// URL (they share a base). Returns `Unsupported` for a collection point that
/// has no sync cursor (and therefore no negentropy endpoint).
fn negentropy_url(sync_url: &str) -> Result<String> {
    let base = sync_url
        .strip_suffix("/sync")
        .ok_or(ClientError::Unsupported(
            "collection point has no /negentropy endpoint",
        ))?;
    Ok(format!("{base}/negentropy"))
}

/// Derive the bulk `by-id` endpoint from the collection's annotations URL
/// (which ends in `/annotations/`).
fn by_id_url(annotations_url: &str) -> String {
    format!("{}/by-id", annotations_url.trim_end_matches('/'))
}

/// Minimal percent-encoding for query values.
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

#[cfg(not(target_arch = "wasm32"))]
fn read_file(path: &str) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| ClientError::Io(e.to_string()))
}

#[cfg(target_arch = "wasm32")]
fn read_file(_path: &str) -> Result<String> {
    Err(ClientError::Unsupported("file IO is not available on wasm"))
}

#[cfg(not(target_arch = "wasm32"))]
fn append_file(path: &str, ann: &Annotation) -> Result<()> {
    let mut items: Vec<Annotation> = match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s)?,
        _ => Vec::new(),
    };
    items.push(ann.clone());
    let json = serde_json::to_string_pretty(&items)?;
    std::fs::write(path, json).map_err(|e| ClientError::Io(e.to_string()))
}

#[cfg(target_arch = "wasm32")]
fn append_file(_path: &str, _ann: &Annotation) -> Result<()> {
    Err(ClientError::Unsupported("file IO is not available on wasm"))
}
