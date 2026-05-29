//! Image viewing tool.
//!
//! `view_image` lets the agent see an uploaded image. When the active model
//! supports vision, it returns an image sentinel that the agent core converts
//! into a multimodal message (the model receives the actual pixels). When the
//! model is text-only, it degrades gracefully to a textual description of the
//! file so the agent can still acknowledge it.

use std::path::{Path, PathBuf};

use agent_core::tool_registry::Tool;
use async_trait::async_trait;
use llm_router::types::IMAGE_SENTINEL_KEY;
use serde_json::{json, Value};

/// Maximum image size we'll inline to the model (5 MiB of raw bytes).
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Tool that loads an image file for the model to view.
pub struct ViewImageTool {
    /// Workspace root — all paths are resolved relative to / confined within this.
    workspace_dir: PathBuf,
    /// Whether the active model can accept image input. Computed once at startup
    /// from the primary provider's model; the fail-safe when false.
    vision_supported: bool,
}

impl ViewImageTool {
    pub fn new(workspace_dir: PathBuf, vision_supported: bool) -> Self {
        Self {
            workspace_dir,
            vision_supported,
        }
    }

    /// Resolve a user-supplied path to an absolute path confined to the
    /// workspace. Rejects anything that escapes the workspace root.
    fn resolve(&self, raw: &str) -> Result<PathBuf, String> {
        let candidate = Path::new(raw);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.workspace_dir.join(candidate)
        };

        // Canonicalize to collapse any `..` and verify containment. The file
        // must exist for canonicalize to succeed, which doubles as existence check.
        let canonical = joined
            .canonicalize()
            .map_err(|e| format!("Cannot access '{raw}': {e}"))?;
        let ws_canonical = self
            .workspace_dir
            .canonicalize()
            .unwrap_or_else(|_| self.workspace_dir.clone());
        if !canonical.starts_with(&ws_canonical) {
            return Err(format!(
                "Path '{raw}' is outside the workspace and cannot be read."
            ));
        }
        Ok(canonical)
    }
}

#[async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &str {
        "view_image"
    }

    fn description(&self) -> &str {
        "View an image file so you can see its contents. Provide `path` relative \
         to the workspace (e.g. uploads/<session>/photo.png) or an absolute path \
         inside the workspace. Use this for images the user has uploaded."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the image (relative to the workspace, e.g. uploads/<session>/cat.png)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let path_arg = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required 'path' argument".to_string())?;

        let resolved = self.resolve(path_arg)?;

        let media_type = common::uploads::image_media_type(&resolved.to_string_lossy())
            .ok_or_else(|| {
                format!("'{path_arg}' is not a recognised image (png, jpg, jpeg, gif, webp, bmp).")
            })?;

        let metadata =
            std::fs::metadata(&resolved).map_err(|e| format!("Cannot stat '{path_arg}': {e}"))?;
        let size = metadata.len();

        // Fail-safe: the model can't see images. Return a textual acknowledgement
        // instead of pixels so the agent can still reason about the file.
        if !self.vision_supported {
            return Ok(format!(
                "⚠️ The active model does not support image input, so I can't view \
                 the contents of '{path_arg}' ({media_type}, {size} bytes). It is saved \
                 in the workspace. Configure a vision-capable model (e.g. claude-opus, \
                 claude-sonnet, or gpt-4o) to analyze images."
            ));
        }

        if size > MAX_IMAGE_BYTES {
            return Ok(format!(
                "Image '{path_arg}' is {size} bytes, which exceeds the {MAX_IMAGE_BYTES}-byte \
                 limit for inline viewing. Ask the user for a smaller version."
            ));
        }

        let bytes =
            std::fs::read(&resolved).map_err(|e| format!("Failed to read '{path_arg}': {e}"))?;
        let data = base64_encode(&bytes);

        // Emit the sentinel the agent core understands. The `note` becomes the
        // text of the multimodal message accompanying the image.
        Ok(json!({
            IMAGE_SENTINEL_KEY: {
                "media_type": media_type,
                "data": data,
                "note": format!("Here is the image '{path_arg}'.")
            }
        })
        .to_string())
    }

    fn coding_only(&self) -> bool {
        // Available in General mode too — users upload images in plain chat.
        false
    }
}

/// Minimal standard base64 encoder (RFC 4648). Inlined to avoid adding a crate
/// dependency in a network-restricted build environment.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[tokio::test]
    async fn fail_safe_when_no_vision() {
        let tmp = std::env::temp_dir().join("xeno_view_image_test");
        let _ = std::fs::create_dir_all(&tmp);
        let img = tmp.join("a.png");
        std::fs::write(&img, b"\x89PNG\r\n\x1a\n").unwrap();

        let tool = ViewImageTool::new(tmp.clone(), false);
        let out = tool
            .execute(json!({ "path": img.to_string_lossy() }))
            .await
            .unwrap();
        assert!(out.contains("does not support image input"));
    }

    #[tokio::test]
    async fn emits_sentinel_when_vision() {
        let tmp = std::env::temp_dir().join("xeno_view_image_test2");
        let _ = std::fs::create_dir_all(&tmp);
        let img = tmp.join("b.png");
        std::fs::write(&img, b"\x89PNG\r\n\x1a\n").unwrap();

        let tool = ViewImageTool::new(tmp.clone(), true);
        let out = tool
            .execute(json!({ "path": img.to_string_lossy() }))
            .await
            .unwrap();
        assert!(out.contains(IMAGE_SENTINEL_KEY));
        assert!(out.contains("image/png"));
    }

    #[tokio::test]
    async fn rejects_non_image() {
        let tmp = tempfile::TempDir::new().unwrap();
        let f = tmp.path().join("notes.txt");
        std::fs::write(&f, b"hello").unwrap();

        let tool = ViewImageTool::new(tmp.path().to_path_buf(), true);
        let err = tool
            .execute(json!({ "path": f.to_string_lossy() }))
            .await
            .unwrap_err();
        assert!(err.contains("not a recognised image"));
    }
}
