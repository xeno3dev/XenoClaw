//! Structured JSON error responses for the API server.
//!
//! All errors are returned as JSON with a consistent structure:
//! - `error_code`: A machine-readable error code string
//! - `message`: A human-readable error description
//! - `request_id`: The unique request identifier for tracing

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use uuid::Uuid;

use common::errors::SecurityError;

/// Structured error response body returned by all API endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub error_code: String,
    pub message: String,
    pub request_id: String,
}

/// API error type that maps to HTTP status codes and structured JSON responses.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub error_code: String,
    pub message: String,
    pub request_id: String,
    /// Optional Retry-After header value in seconds (for 429 responses).
    pub retry_after: Option<u64>,
}

impl ApiError {
    /// Create a new API error with a generated request ID.
    pub fn new(
        status: StatusCode,
        error_code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            error_code: error_code.into(),
            message: message.into(),
            request_id: Uuid::new_v4().to_string(),
            retry_after: None,
        }
    }

    /// Create a new API error with a specific request ID.
    pub fn with_request_id(
        status: StatusCode,
        error_code: impl Into<String>,
        message: impl Into<String>,
        request_id: String,
    ) -> Self {
        Self {
            status,
            error_code: error_code.into(),
            message: message.into(),
            request_id,
            retry_after: None,
        }
    }

    /// Set the Retry-After header value.
    pub fn with_retry_after(mut self, seconds: u64) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    // --- Common error constructors ---

    /// 400 Bad Request — validation error.
    pub fn bad_request(message: impl Into<String>, request_id: String) -> Self {
        Self::with_request_id(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            message,
            request_id,
        )
    }

    /// 401 Unauthorized — authentication required or failed.
    pub fn unauthorized(request_id: String) -> Self {
        Self::with_request_id(
            StatusCode::UNAUTHORIZED,
            "AUTHENTICATION_REQUIRED",
            "Valid API key required. Provide a Bearer token in the Authorization header.",
            request_id,
        )
    }

    /// 403 Forbidden — insufficient permissions.
    pub fn forbidden(message: impl Into<String>, request_id: String) -> Self {
        Self::with_request_id(
            StatusCode::FORBIDDEN,
            "INSUFFICIENT_PERMISSIONS",
            message,
            request_id,
        )
    }

    /// 404 Not Found.
    pub fn not_found(message: impl Into<String>, request_id: String) -> Self {
        Self::with_request_id(StatusCode::NOT_FOUND, "NOT_FOUND", message, request_id)
    }

    /// 429 Too Many Requests — rate limit exceeded.
    pub fn rate_limited(retry_after_secs: u64, request_id: String) -> Self {
        Self::with_request_id(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMIT_EXCEEDED",
            format!(
                "Rate limit exceeded. Retry after {} seconds.",
                retry_after_secs
            ),
            request_id,
        )
        .with_retry_after(retry_after_secs)
    }

    /// 500 Internal Server Error.
    pub fn internal(message: impl Into<String>, request_id: String) -> Self {
        Self::with_request_id(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            message,
            request_id,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorResponse {
            error_code: self.error_code,
            message: self.message,
            request_id: self.request_id,
        };

        let mut response = (self.status, axum::Json(body)).into_response();

        // Add Retry-After header for 429 responses
        if let Some(retry_after) = self.retry_after {
            response
                .headers_mut()
                .insert("Retry-After", retry_after.to_string().parse().unwrap());
        }

        response
    }
}

/// Convert a SecurityError into an ApiError.
impl ApiError {
    pub fn from_security_error(err: SecurityError, request_id: String) -> Self {
        match err {
            SecurityError::AuthenticationRequired => Self::unauthorized(request_id),
            SecurityError::InvalidCredentials => Self::with_request_id(
                StatusCode::UNAUTHORIZED,
                "INVALID_CREDENTIALS",
                "Invalid API key.",
                request_id,
            ),
            SecurityError::IpBlocked { until } => Self::with_request_id(
                StatusCode::FORBIDDEN,
                "IP_BLOCKED",
                format!("IP blocked until {}", until),
                request_id,
            ),
            SecurityError::InsufficientPermissions { required_role } => Self::forbidden(
                format!(
                    "Insufficient permissions. Required role: {:?}",
                    required_role
                ),
                request_id,
            ),
            SecurityError::RateLimitExceeded { retry_after } => {
                Self::rate_limited(retry_after.as_secs(), request_id)
            }
            SecurityError::SessionExpired => Self::with_request_id(
                StatusCode::UNAUTHORIZED,
                "SESSION_EXPIRED",
                "Session has expired. Please re-authenticate.",
                request_id,
            ),
            SecurityError::SandboxViolation { .. } => {
                Self::forbidden("Operation denied by security policy.", request_id)
            }
            SecurityError::NetworkBlocked { .. } => {
                Self::forbidden("Network access denied by security policy.", request_id)
            }
        }
    }
}
