//! Task management endpoints.
//!
//! POST   /api/v1/tasks          — Create a scheduled task
//! GET    /api/v1/tasks          — List tasks
//! GET    /api/v1/tasks/:id/history — Get task execution history
//! DELETE /api/v1/tasks/:id      — Cancel a task

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::auth::RequestId;
use crate::state::AppState;

/// Request body for creating a task.
#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    pub trigger: serde_json::Value,
    pub action: serde_json::Value,
    pub timeout_seconds: Option<u32>,
}

/// Response after creating a task.
#[derive(Debug, Serialize)]
pub struct CreateTaskResponse {
    pub task_id: String,
    pub name: String,
    pub status: &'static str,
}

/// Task summary for listing.
#[derive(Debug, Serialize)]
pub struct TaskSummary {
    pub id: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
}

/// Task list response.
#[derive(Debug, Serialize)]
pub struct TaskListResponse {
    pub tasks: Vec<TaskSummary>,
}

/// Task run entry for history.
#[derive(Debug, Serialize)]
pub struct TaskRunEntry {
    pub id: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub duration_ms: Option<u64>,
    pub status: String,
    pub error: Option<String>,
    pub attempt: u8,
}

/// Task history response.
#[derive(Debug, Serialize)]
pub struct TaskHistoryResponse {
    pub task_id: String,
    pub runs: Vec<TaskRunEntry>,
}

/// Delete task response.
#[derive(Debug, Serialize)]
pub struct DeleteTaskResponse {
    pub task_id: String,
    pub status: &'static str,
}

/// POST /api/v1/tasks — Create a new scheduled task.
async fn create_task(
    State(_state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Json(body): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<CreateTaskResponse>), ApiError> {
    // Validate required fields
    if body.name.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Task name must not be empty.",
            req_id.0,
        ));
    }

    // In a full implementation, this would create the task via Task Scheduler.
    let response = CreateTaskResponse {
        task_id: Uuid::new_v4().to_string(),
        name: body.name,
        status: "active",
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// GET /api/v1/tasks — List all tasks.
async fn list_tasks(State(_state): State<AppState>) -> Json<TaskListResponse> {
    // In a full implementation, this would query the Task Scheduler.
    Json(TaskListResponse { tasks: Vec::new() })
}

/// GET /api/v1/tasks/:id/history — Get task execution history.
async fn get_task_history(
    State(_state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<TaskHistoryResponse>, ApiError> {
    // Validate task ID format
    Uuid::parse_str(&id).map_err(|_| {
        ApiError::bad_request(
            "Invalid task ID format. Must be a valid UUID.",
            req_id.0.clone(),
        )
    })?;

    // In a full implementation, this would query the Task Scheduler.
    Ok(Json(TaskHistoryResponse {
        task_id: id,
        runs: Vec::new(),
    }))
}

/// DELETE /api/v1/tasks/:id — Cancel a task.
async fn cancel_task(
    State(_state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<DeleteTaskResponse>, ApiError> {
    // Validate task ID format
    Uuid::parse_str(&id).map_err(|_| {
        ApiError::bad_request(
            "Invalid task ID format. Must be a valid UUID.",
            req_id.0.clone(),
        )
    })?;

    // In a full implementation, this would cancel via Task Scheduler.
    Ok(Json(DeleteTaskResponse {
        task_id: id,
        status: "cancelled",
    }))
}

/// Build task routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/tasks", post(create_task).get(list_tasks))
        .route("/api/v1/tasks/{id}/history", get(get_task_history))
        .route("/api/v1/tasks/{id}", delete(cancel_task))
}
