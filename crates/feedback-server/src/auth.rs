//! Dual-identity auth (INVARIANT 4).
//!
//! A write is authorized either by **self-signed** annotations (every annotation
//! carries a valid ES256 signature — federates) **or** by a valid **OAuth**
//! bearer token mapped to an app-scoped `(app_id, user_id)` identity (does not
//! federate). Self-signature is verified cryptographically; OAuth is a
//! pluggable token map (a real OAuth integration replaces the map).

use std::collections::HashMap;

use axum::http::HeaderMap;
use freedback_protocol::{verify_annotation, Annotation, Creator};

use crate::error::ApiError;

/// OAuth bearer-token registry: token → `(app_id, user_id)`.
#[derive(Default)]
pub struct OAuth {
    tokens: HashMap<String, (String, String)>,
}

impl OAuth {
    /// Build from a token map.
    pub fn new(tokens: HashMap<String, (String, String)>) -> Self {
        Self { tokens }
    }
    /// Resolve a bearer token to an identity.
    pub fn lookup(&self, token: &str) -> Option<&(String, String)> {
        self.tokens.get(token)
    }
}

/// The authenticated identity behind a write.
#[derive(Debug, Clone)]
pub enum Authz {
    /// Each annotation is self-signed; the issuer is each annotation's `kid`.
    SelfSigned,
    /// App-managed OAuth identity (siloed, non-federating).
    OAuth { app_id: String, user_id: String },
}

impl Authz {
    /// The app-scoped `creator.id` stamped onto OAuth-authored annotations.
    pub fn oauth_creator(&self) -> Option<Creator> {
        match self {
            Authz::OAuth { app_id, user_id } => Some(Creator::new(format!(
                "urn:freedback:oauth:{app_id}:{user_id}"
            ))),
            Authz::SelfSigned => None,
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Resolve an OAuth bearer (if present) to an [`Authz`].
///
/// * `Ok(Some(authz))` — a valid bearer token.
/// * `Ok(None)` — no bearer header at all (the caller should fall back to
///   self-signature checks).
/// * `Err(..)` — a bearer header is present but the token is unknown (fatal).
pub fn oauth_authz(oauth: &OAuth, headers: &HeaderMap) -> Result<Option<Authz>, ApiError> {
    match bearer(headers) {
        Some(token) => match oauth.lookup(token) {
            Some((app_id, user_id)) => Ok(Some(Authz::OAuth {
                app_id: app_id.clone(),
                user_id: user_id.clone(),
            })),
            None => Err(ApiError::unauthorized("invalid bearer token")),
        },
        None => Ok(None),
    }
}

/// Authorize a single annotation by its own self-signature (no bearer).
/// Used by the batch path so one bad signature fails only its own item.
pub fn authorize_one_self_signed(ann: &Annotation) -> Result<Authz, ApiError> {
    if ann.signature.is_none() {
        return Err(ApiError::unauthorized(
            "no bearer token and annotation is unsigned",
        ));
    }
    verify_annotation(ann).map_err(|_| ApiError::unauthorized("signature verification failed"))?;
    Ok(Authz::SelfSigned)
}

/// Authorize a batch of annotations. OAuth bearer takes precedence; otherwise
/// every annotation must carry a valid self-signature. This is the all-or-
/// nothing gate used by the single-item POST path; the batch path authorizes
/// per item (see [`oauth_authz`] / [`authorize_one_self_signed`]).
pub fn authorize(
    oauth: &OAuth,
    headers: &HeaderMap,
    anns: &[Annotation],
) -> Result<Authz, ApiError> {
    if let Some(authz) = oauth_authz(oauth, headers)? {
        return Ok(authz);
    }
    for ann in anns {
        authorize_one_self_signed(ann)?;
    }
    Ok(Authz::SelfSigned)
}
