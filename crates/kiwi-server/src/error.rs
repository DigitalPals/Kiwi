use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

/// An API error rendered as `{ "error": { "code", "message" } }`. Messages are shown to the
/// user as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
    pub fn csrf(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "csrf", message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }
    /// The device belongs to someone other than the owner.
    pub fn not_owner(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "not_owner", message)
    }
    /// Kiwi Builds is turned off in Settings.
    pub fn builds_off() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "builds_off",
            "Kiwi Builds is off. Turn it on in Settings.",
        )
    }
    pub fn signing_unavailable(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "signing_unavailable",
            message,
        )
    }
    /// A request proxied from the tailnet whose peer `tailscale whois` could not resolve.
    pub fn device_unidentified() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "device_unidentified",
            "Couldn't identify this device through Tailscale. Try again.",
        )
    }
    /// A request for a host name Kiwi does not serve (e.g. DNS rebinding).
    pub fn bad_host() -> Self {
        Self::new(
            StatusCode::MISDIRECTED_REQUEST,
            "bad_host",
            "This host name is not served by Kiwi.",
        )
    }
    pub fn internal(message: impl std::fmt::Display) -> Self {
        tracing::error!("internal error: {message}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("Internal error: {message}"),
        )
    }
    /// Maps a kiwi-source error by its stage.
    pub fn source(error: kiwi_source::Error) -> Self {
        let message = error.to_string();
        match error.stage {
            "Source parse" | "IPA validation" => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "source_parse", message)
            }
            "URL" | "Source URL" => Self::bad_request(message),
            _ => Self::new(StatusCode::BAD_GATEWAY, "source_fetch", message),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::internal(format!("database: {e}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": { "code": self.code, "message": self.message } });
        (self.status, Json(body)).into_response()
    }
}
