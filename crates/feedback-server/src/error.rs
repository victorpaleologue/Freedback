//! API error responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Errors surfaced to HTTP clients.
#[derive(Debug)]
pub enum ApiError {
    /// A plain status + message (401, 404, 400, 500…).
    Status(StatusCode, String),
    /// SHACL validation failed → 422 with the report (INVARIANT 3).
    Validation(Vec<String>),
}

impl ApiError {
    /// 400 Bad Request.
    pub fn bad_request(msg: impl Into<String>) -> Self {
        ApiError::Status(StatusCode::BAD_REQUEST, msg.into())
    }
    /// 401 Unauthorized.
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        ApiError::Status(StatusCode::UNAUTHORIZED, msg.into())
    }
    /// 404 Not Found.
    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError::Status(StatusCode::NOT_FOUND, msg.into())
    }
    /// 406 Not Acceptable (content negotiation failed).
    pub fn not_acceptable(msg: impl Into<String>) -> Self {
        ApiError::Status(StatusCode::NOT_ACCEPTABLE, msg.into())
    }
    /// 500 Internal Server Error.
    pub fn internal(msg: impl Into<String>) -> Self {
        ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, msg.into())
    }

    /// Render this error as a `(status, json-object)` pair for inclusion as one
    /// item in a multi-status batch response. The object mirrors the standalone
    /// error body: a SHACL failure carries its `report`, others a flat `error`.
    pub fn as_item(&self) -> (StatusCode, serde_json::Value) {
        match self {
            ApiError::Status(code, msg) => (*code, json!({ "error": msg })),
            ApiError::Validation(violations) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "error": "SHACL validation failed",
                    "conformsTo": "https://freedback.net/profile/1",
                    "report": { "conforms": false, "violations": violations },
                }),
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Status(code, msg) => (code, Json(json!({ "error": msg }))).into_response(),
            ApiError::Validation(violations) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": "SHACL validation failed",
                    "conformsTo": "https://freedback.net/profile/1",
                    "report": { "conforms": false, "violations": violations },
                })),
            )
                .into_response(),
        }
    }
}

impl From<freedback_storage::StoreError> for ApiError {
    fn from(e: freedback_storage::StoreError) -> Self {
        ApiError::internal(format!("storage: {e}"))
    }
}

impl From<freedback_protocol::Error> for ApiError {
    fn from(e: freedback_protocol::Error) -> Self {
        ApiError::bad_request(format!("protocol: {e}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::internal(format!("json: {e}"))
    }
}
