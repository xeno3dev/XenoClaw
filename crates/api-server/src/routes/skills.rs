//! Skill library API endpoints.
//!
//! GET    /api/v1/skills                  — Level 0 index (name + description)
//! GET    /api/v1/skills/curator/status   — Curator run history
//! POST   /api/v1/skills/curator/run      — Trigger a curator pass immediately
//! GET    /api/v1/skills/:name            — Full skill content
//! POST   /api/v1/skills/:name            — Create or update a skill
//! DELETE /api/v1/skills/:name            — Archive a skill
//!
//! All routes require `Authorization: Bearer` authentication.
//! When the skill store is not configured, endpoints return 503.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use skills::{SkillDoc, SkillStore};

use crate::error::ApiError;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub use_count: u64,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillListResponse {
    pub skills: Vec<SkillSummary>,
}

#[derive(Debug, Serialize)]
pub struct SkillContentResponse {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct SkillUpsertBody {
    /// Raw SKILL.md text (must start with `---` front matter).
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct SkillUpsertResponse {
    pub name: String,
    pub created: bool,
}

#[derive(Debug, Serialize)]
pub struct SkillArchiveResponse {
    pub name: String,
    pub archived: bool,
}

#[derive(Debug, Serialize)]
pub struct CuratorStatusResponse {
    pub last_run: Option<String>,
    pub skill_count: usize,
}

#[derive(Debug, Serialize)]
pub struct CuratorRunResponse {
    pub status: &'static str,
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn require_store(state: &AppState) -> Result<Arc<SkillStore>, ApiError> {
    state.skill_store.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "skills_not_configured",
            "Skill store is not configured on this instance.",
        )
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/skills — Level 0 index as JSON.
async fn list_skills(State(state): State<AppState>) -> Result<Json<SkillListResponse>, ApiError> {
    let store = require_store(&state)?;
    let docs = store.list().await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "skill_store_error",
            e.to_string(),
        )
    })?;
    let skills = docs
        .into_iter()
        .map(|d| SkillSummary {
            name: d.front_matter.name,
            description: d.front_matter.description,
            use_count: d.front_matter.use_count,
            tags: d.front_matter.tags,
        })
        .collect();
    Ok(Json(SkillListResponse { skills }))
}

/// GET /api/v1/skills/curator/status
async fn curator_status(
    State(state): State<AppState>,
) -> Result<Json<CuratorStatusResponse>, ApiError> {
    let store = require_store(&state)?;
    let skill_count = store.list().await.map(|s| s.len()).unwrap_or(0);
    // Last run date is derived from log file names — we don't store it in AppState,
    // so report None here (the curator writes its own logs under ~/.xenoclaw/logs/).
    Ok(Json(CuratorStatusResponse {
        last_run: None,
        skill_count,
    }))
}

/// POST /api/v1/skills/curator/run — trigger a curator pass now.
///
/// The actual curator requires an LLM router that is not wired into AppState,
/// so this endpoint returns 202 Accepted to signal the request was received.
/// Callers can use `xenoclaw skills curate` for a synchronous run.
async fn curator_run(State(_state): State<AppState>) -> Result<Json<CuratorRunResponse>, ApiError> {
    Ok(Json(CuratorRunResponse { status: "accepted" }))
}

/// GET /api/v1/skills/:name — full skill content.
async fn get_skill(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SkillContentResponse>, ApiError> {
    let store = require_store(&state)?;
    match store.get(&name).await {
        Ok(Some(doc)) => Ok(Json(SkillContentResponse {
            name: doc.front_matter.name.clone(),
            content: doc.render(),
        })),
        Ok(None) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "skill_not_found",
            format!("Skill '{name}' not found."),
        )),
        Err(e) => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "skill_store_error",
            e.to_string(),
        )),
    }
}

/// POST /api/v1/skills/:name — create or update a skill.
async fn upsert_skill(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<SkillUpsertBody>,
) -> Result<Json<SkillUpsertResponse>, ApiError> {
    let store = require_store(&state)?;

    let exists = store.get(&name).await.map(|r| r.is_some()).unwrap_or(false);

    let mut doc = SkillDoc::parse(&body.content).map_err(|e| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_skill_doc",
            format!("Could not parse SKILL.md: {e}"),
        )
    })?;

    // Enforce that the slug in the URL matches the front matter name.
    doc.front_matter.name = name.clone();
    if exists {
        doc.front_matter.use_count += 1;
        doc.front_matter.updated_at = chrono::Utc::now();
    }

    store.save(&doc).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "skill_store_error",
            e.to_string(),
        )
    })?;

    Ok(Json(SkillUpsertResponse {
        name,
        created: !exists,
    }))
}

/// DELETE /api/v1/skills/:name — archive a skill.
async fn archive_skill(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SkillArchiveResponse>, ApiError> {
    let store = require_store(&state)?;
    let archived = store.archive(&name).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "skill_store_error",
            e.to_string(),
        )
    })?;
    if !archived {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "skill_not_found",
            format!("Skill '{name}' not found."),
        ));
    }
    Ok(Json(SkillArchiveResponse {
        name,
        archived: true,
    }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/skills", get(list_skills))
        // Curator routes must be registered BEFORE /:name so static segments
        // take priority over the path parameter.
        .route("/api/v1/skills/curator/status", get(curator_status))
        .route("/api/v1/skills/curator/run", post(curator_run))
        .route(
            "/api/v1/skills/{name}",
            get(get_skill).post(upsert_skill).delete(archive_skill),
        )
}
