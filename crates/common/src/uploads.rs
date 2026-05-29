//! Shared helpers for storing user-uploaded files under the workspace.
//!
//! Uploads land at `{workspace}/uploads/{session_id}/{filename}`. Both the web
//! upload endpoint and the messaging bridges route through these helpers so
//! path-sanitization and the directory layout stay consistent.

use std::path::{Path, PathBuf};

/// Image file extensions the agent knows how to view.
const IMAGE_EXTENSIONS: &[(&str, &str)] = &[
    (".png", "image/png"),
    (".jpg", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".gif", "image/gif"),
    (".webp", "image/webp"),
    (".bmp", "image/bmp"),
];

/// The uploads directory for a session: `{workspace}/uploads/{session_id}`.
/// The session id is sanitized to a single path component.
pub fn session_upload_dir(workspace: &Path, session_id: &str) -> PathBuf {
    workspace
        .join("uploads")
        .join(sanitize_component(session_id))
}

/// Relative display path shown to the model, e.g. `uploads/{session}/foo.png`.
/// Uses forward slashes regardless of platform.
pub fn display_path(session_id: &str, filename: &str) -> String {
    format!(
        "uploads/{}/{}",
        sanitize_component(session_id),
        sanitize_filename(filename)
    )
}

/// Reduce an arbitrary client-supplied filename to a safe basename: strips any
/// directory components, control characters, and leading/trailing dots so a
/// crafted name can't escape the uploads directory via `../` or absolute paths.
pub fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim_matches('.')
        .trim()
        .to_string();
    if cleaned.is_empty() || cleaned == ".." {
        "file".to_string()
    } else {
        cleaned
    }
}

/// Sanitize a path component (session id). Same rules as filenames but never
/// empty (falls back to "session").
fn sanitize_component(s: &str) -> String {
    let cleaned = sanitize_filename(s);
    if cleaned == "file" && s != "file" {
        "session".to_string()
    } else {
        cleaned
    }
}

/// Whether a filename looks like an image (by extension).
pub fn is_image_filename(name: &str) -> bool {
    image_media_type(name).is_some()
}

/// Guess the image MIME type from a filename extension. `None` for non-images.
pub fn image_media_type(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    IMAGE_EXTENSIONS
        .iter()
        .find(|(ext, _)| lower.ends_with(ext))
        .map(|(_, mime)| *mime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("/abs/foo.png"), "foo.png");
        assert_eq!(sanitize_filename("a\\b\\c.rs"), "c.rs");
        assert_eq!(sanitize_filename(".."), "file");
        assert_eq!(sanitize_filename(""), "file");
        assert_eq!(sanitize_filename("normal.txt"), "normal.txt");
    }

    #[test]
    fn upload_dir_layout() {
        let dir = session_upload_dir(Path::new("/ws"), "sess-1");
        assert_eq!(dir, PathBuf::from("/ws/uploads/sess-1"));
    }

    #[test]
    fn image_detection() {
        assert_eq!(image_media_type("cat.PNG"), Some("image/png"));
        assert_eq!(image_media_type("a.jpeg"), Some("image/jpeg"));
        assert!(is_image_filename("x.webp"));
        assert!(!is_image_filename("notes.rs"));
        assert_eq!(image_media_type("notes.rs"), None);
    }

    #[test]
    fn display_path_uses_forward_slashes() {
        assert_eq!(display_path("s1", "a.png"), "uploads/s1/a.png");
    }
}
