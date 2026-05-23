//! Messaging-bridge status endpoint.
//!
//! GET /api/v1/messaging — Report which third-party messaging providers
//! have credentials configured.
//!
//! This is a deliberately tiny read-only surface so the Settings UI can
//! render connection status without ever fetching the actual bot tokens
//! over the network. Configuration changes go through the wizard
//! (`xenoclaw -s`) or by editing config.toml directly — the API does not
//! expose write access, since accepting bot tokens over HTTP would mean
//! they travel through reverse proxies, browser memory, and access logs.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct ProviderStatus {
    pub configured: bool,
}

#[derive(Debug, Serialize)]
pub struct MessagingStatusResponse {
    pub telegram: ProviderStatus,
    pub discord: ProviderStatus,
    pub whatsapp: ProviderStatus,
}

async fn get_messaging_status(State(state): State<AppState>) -> Json<MessagingStatusResponse> {
    let s = state.messaging_status;
    Json(MessagingStatusResponse {
        telegram: ProviderStatus {
            configured: s.telegram_configured,
        },
        discord: ProviderStatus {
            configured: s.discord_configured,
        },
        whatsapp: ProviderStatus {
            configured: s.whatsapp_configured,
        },
    })
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/messaging", get(get_messaging_status))
}
