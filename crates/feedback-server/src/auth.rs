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

/// Authorize a batch of annotations. OAuth bearer takes precedence; otherwise
/// every annotation must carry a valid self-signature.
pub fn authorize(
    oauth: &OAuth,
    headers: &HeaderMap,
    anns: &[Annotation],
) -> Result<Authz, ApiError> {
    if let Some(token) = bearer(headers) {
        return match oauth.lookup(token) {
            Some((app_id, user_id)) => Ok(Authz::OAuth {
                app_id: app_id.clone(),
                user_id: user_id.clone(),
            }),
            None => Err(ApiError::unauthorized("invalid bearer token")),
        };
    }

    for ann in anns {
        if ann.signature.is_none() {
            return Err(ApiError::unauthorized(
                "no bearer token and annotation is unsigned",
            ));
        }
        verify_annotation(ann)
            .map_err(|_| ApiError::unauthorized("signature verification failed"))?;
    }
    Ok(Authz::SelfSigned)
}
