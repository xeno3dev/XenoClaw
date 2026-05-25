//! File upload endpoint.
//!
//! POST /api/v1/uploads/{session_id} — multipart/form-data file upload.
//!
//! Files are stored under `{workspace}/uploads/{session_id}/{filename}` so the
//! agent can reference them (and the `view_image` tool can load images). The
//! response lists each stored file with its workspace-relative path, which the
//! web client echoes back in the chat message so the agent knows what to look at.

use axum::extract::{Multipart, Path, State};
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::Serialize;

use common::uploads::{display_path, image_media_type, sanitize_filename, session_upload_dir};

use crate::error::ApiError;
use crate::middleware::auth::RequestId;
use crate::state::AppState;

/// Metadata for one stored file.
#[derive(Debug, Serialize)]
pub struct UploadedFile {
    /// Sanitized filename as stored on disk.
    pub name: String,
    /// Workspace-relative path, e.g. `uploads/<session>/cat.png`.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// True if recognized as an image.
    pub is_image: bool,
    /// Image MIME type when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// Response listing all files saved by an upload request.
#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub session_id: String,
    pub files: Vec<UploadedFile>,
}

/// Per-request cap on total bytes accepted (25 MiB) to bound disk use.
const MAX_TOTAL_BYTES: usize = 25 * 1024 * 1024;

/// POST /api/v1/uploads/{session_id} — store uploaded files for a session.
async fn upload_files(
    State(state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(session_id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::bad_request(
            "session_id must not be empty.",
            req_id.0,
        ));
    }

    let dir = session_upload_dir(&state.workspace_dir, &session_id);
    tokio::fs::create_dir_all(&dir).await.map_err(|e| {
        ApiError::with_request_id(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "UPLOAD_DIR_FAILED",
            format!("Failed to create upload directory: {e}"),
            req_id.0.clone(),
        )
    })?;

    let mut files = Vec::new();
    let mut total: usize = 0;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                return Err(ApiError::bad_request(
                    format!("Malformed multipart upload: {e}"),
                    req_id.0.clone(),
                ));
            }
        };

        // Use the supplied filename; skip non-file fields without one.
        let original = match field.file_name() {
            Some(n) => n.to_string(),
            None => continue,
        };
        let filename = sanitize_filename(&original);

        let bytes = field.bytes().await.map_err(|e| {
            ApiError::bad_request(
                format!("Failed to read uploaded file '{filename}': {e}"),
                req_id.0.clone(),
            )
        })?;

        total += bytes.len();
        if total > MAX_TOTAL_BYTES {
            return Err(ApiError::bad_request(
                format!("Upload exceeds the {MAX_TOTAL_BYTES}-byte limit."),
                req_id.0.clone(),
            ));
        }

        let dest = dir.join(&filename);
        tokio::fs::write(&dest, &bytes).await.map_err(|e| {
            ApiError::with_request_id(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "UPLOAD_WRITE_FAILED",
                format!("Failed to write '{filename}': {e}"),
                req_id.0.clone(),
            )
        })?;

        let media_type = image_media_type(&filename).map(|m| m.to_string());
        files.push(UploadedFile {
            name: filename.clone(),
            path: display_path(&session_id, &filename),
            size: bytes.len() as u64,
            is_image: media_type.is_some(),
            media_type,
        });
    }

    if files.is_empty() {
        return Err(ApiError::bad_request(
            "No files found in the upload.",
            req_id.0,
        ));
    }

    tracing::info!(
        session_id = %session_id,
        count = files.len(),
        bytes = total,
        "Stored uploaded files"
    );

    Ok(Json(UploadResponse { session_id, files }))
}

/// Build upload routes.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/uploads/{session_id}", post(upload_files))
}
