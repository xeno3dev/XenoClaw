//! Authentication routes — login with username/password or API key.
//!
//! POST /api/v1/auth/login — Authenticate with username + password
//! Returns a session token that can be used as a Bearer token.

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::{Deserialize, Serialize};

use security_layer::auth::PasswordAuthenticator;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct LoginError {
    pub error: String,
}

/// Build auth routes (unauthenticated — login is how you GET a token).
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/auth/login", post(login))
}

/// Handle login with username + password.
///
/// Validates credentials against the config's admin_username and
/// admin_password_hash (bcrypt). On success, generates a session token.
async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, Json<LoginError>)> {
    // Check username matches
    if body.username != state.admin_username {
        // Dummy verify to prevent timing attacks
        let _ = bcrypt::verify(
            &body.password,
            "$2b$12$000000000000000000000uGTWvMPqHJB2LTnGOEHOV0bMFfGPkkS",
        );
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(LoginError {
                error: "Invalid username or password".to_string(),
            }),
        ));
    }

    // Check if password login is enabled
    if state.admin_password_hash.is_empty() {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(LoginError {
                error: "Password login is disabled. Use an API key.".to_string(),
            }),
        ));
    }

    // Verify password against stored bcrypt hash
    let password_auth = PasswordAuthenticator::new();
    match password_auth.verify_password(&body.password, &state.admin_password_hash) {
        Ok(()) => {
            // Generate a session token
            let token = state.authenticator.generate_key(64);
            Ok(Json(LoginResponse { token }))
        }
        Err(_) => Err((
            StatusCode::UNAUTHORIZED,
            Json(LoginError {
                error: "Invalid username or password".to_string(),
            }),
        )),
    }
}
