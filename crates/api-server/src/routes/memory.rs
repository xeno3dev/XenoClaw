//! Memory store endpoints.
//!
//! GET    /api/v1/memory/search    — Search memory store
//! POST   /api/v1/memory/knowledge — Store knowledge entry
//! DELETE /api/v1/memory/:id       — Delete memory entry

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::auth::RequestId;
use crate::state::AppState;

/// Query parameters for memory search.
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// The search query string.
    pub q: Option<String>,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

/// A search result entry.
#[derive(Debug, Serialize)]
pub struct SearchResultEntry {
    pub id: String,
    pub content: String,
    pub relevance_score: f32,
    pub source: String,
}

/// Search response.
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResultEntry>,
    pub query: String,
}

/// Request body for storing a knowledge entry.
#[derive(Debug, Deserialize)]
pub struct StoreKnowledgeRequest {
    pub title: String,
    pub content: String,
    pub tags: Option<Vec<String>>,
}

/// Response after storing knowledge.
#[derive(Debug, Serialize)]
pub struct StoreKnowledgeResponse {
    pub id: String,
    pub title: String,
    pub status: &'static str,
}

/// Response after deleting a memory entry.
#[derive(Debug, Serialize)]
pub struct DeleteMemoryResponse {
    pub id: String,
    pub status: &'static str,
}

/// Maximum allowed content size for knowledge entries (10,000 characters).
const MAX_KNOWLEDGE_CONTENT_SIZE: usize = 10_000;

/// GET /api/v1/memory/search — Search the memory store.
async fn search_memory(
    State(_state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    let query = params.q.unwrap_or_default();

    if query.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Search query parameter 'q' must not be empty.",
            req_id.0,
        ));
    }

    // In a full implementation, this would query the Memory Store.
    Ok(Json(SearchResponse {
        results: Vec::new(),
        query,
    }))
}

/// POST /api/v1/memory/knowledge — Store a knowledge entry.
async fn store_knowledge(
    State(_state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Json(body): Json<StoreKnowledgeRequest>,
) -> Result<(StatusCode, Json<StoreKnowledgeResponse>), ApiError> {
    // Validate title
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Knowledge entry title must not be empty.",
            req_id.0,
        ));
    }

    // Validate content size
    if body.content.len() > MAX_KNOWLEDGE_CONTENT_SIZE {
        return Err(ApiError::bad_request(
            format!(
                "Content exceeds maximum size of {} characters (got {}).",
                MAX_KNOWLEDGE_CONTENT_SIZE,
                body.content.len()
            ),
            req_id.0,
        ));
    }

    if body.content.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Knowledge entry content must not be empty.",
            req_id.0,
        ));
    }

    // In a full implementation, this would store via Memory Store.
    let response = StoreKnowledgeResponse {
        id: Uuid::new_v4().to_string(),
        title: body.title,
        status: "stored",
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// DELETE /api/v1/memory/:id — Delete a memory entry.
async fn delete_memory(
    State(_state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<DeleteMemoryResponse>, ApiError> {
    // Validate ID format
    Uuid::parse_str(&id).map_err(|_| {
        ApiError::bad_request(
            "Invalid memory entry ID format. Must be a valid UUID.",
            req_id.0.clone(),
        )
    })?;

    // In a full implementation, this would delete via Memory Store.
    Ok(Json(DeleteMemoryResponse {
        id,
        status: "deleted",
    }))
}

/// Build memory routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/memory/search", get(search_memory))
        .route("/api/v1/memory/knowledge", post(store_knowledge))
        .route("/api/v1/memory/{id}", delete(delete_memory))
}
