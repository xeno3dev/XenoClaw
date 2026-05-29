//! Authentication middleware using Bearer token API keys.
//!
//! Extracts the API key from the `Authorization: Bearer <key>` header,
//! validates it against stored keys, and enforces rate limits.

use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

use common::types::ApiKeyId;

use crate::error::ApiError;
use crate::state::AppState;

/// Key used to store the authenticated API key ID in request extensions.
#[derive(Debug, Clone, Copy)]
pub struct AuthenticatedKey(pub ApiKeyId);

/// Axum middleware layer type alias for convenience.
pub type AuthLayer = axum::middleware::FromFnLayer<
    fn(
        State<AppState>,
        Request<axum::body::Body>,
        Next,
    )
        -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, ApiError>> + Send>>,
    AppState,
    (State<AppState>, Request<axum::body::Body>, Next),
>;

/// Authentication and rate-limiting middleware function.
///
/// This function:
/// 1. Generates a request ID (or uses X-Request-Id if provided)
/// 2. Extracts the Bearer token from the Authorization header
/// 3. Validates the API key against stored keys
/// 4. Checks rate limits for the authenticated key
/// 5. Injects the authenticated key ID into request extensions
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    // Generate or extract request ID
    let request_id = request
        .headers()
        .get("X-Request-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Extract Authorization header
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(header_value) => {
            if let Some(token) = header_value.strip_prefix("Bearer ") {
                token.to_string()
            } else {
                return Err(ApiError::unauthorized(request_id));
            }
        }
        None => {
            return Err(ApiError::unauthorized(request_id));
        }
    };

    // Validate API key format and authenticate.
    // First try pre-configured API keys; if that fails, check whether this is
    // a session token issued by the /auth/login endpoint.
    let (key_id, key_rate_limit) = match state.authenticator.authenticate(&token, &state.api_keys) {
        Ok(id) => {
            let rate_limit = state
                .api_keys
                .iter()
                .find(|k| k.id == id)
                .map(|k| k.rate_limit);
            (id, rate_limit)
        }
        Err(_) => {
            // Fall back to login-issued session tokens.
            let is_login_token = state.login_tokens.read().await.contains(&token);
            if is_login_token {
                // Session tokens are not rate-limited per-key; pass None so
                // the limiter uses its global default.
                (state.admin_session_key_id, None)
            } else {
                return Err(ApiError::with_request_id(
                    StatusCode::UNAUTHORIZED,
                    "INVALID_CREDENTIALS",
                    "Invalid API key.",
                    request_id,
                ));
            }
        }
    };

    // Check rate limit
    if let Err(err) = state
        .rate_limiter
        .check_rate_limit(key_id, key_rate_limit)
        .await
    {
        return Err(ApiError::from_security_error(err, request_id));
    }

    // Store authenticated key and request ID in extensions
    request.extensions_mut().insert(AuthenticatedKey(key_id));
    request.extensions_mut().insert(RequestId(request_id));

    Ok(next.run(request).await)
}

/// Request ID stored in request extensions for use in handlers.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);
