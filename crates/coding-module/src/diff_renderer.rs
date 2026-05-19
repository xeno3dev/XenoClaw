//! Diff Renderer — renders unified diffs as SVG images.
//!
//! Provides:
//! - `DiffRenderer` — renders diffs as SVG images with syntax highlighting
//! - `RenderError` — errors that can occur during rendering
//!
//! The renderer produces SVG markup with:
//! - File name header at the top
//! - Line numbers on the left margin
//! - Green background (#e6ffec) for addition lines
//! - Red background (#ffebe9) for removal lines
//! - White/neutral background for context lines
//! - Monospace font for code content
//! - Blue/purple hunk headers (`@@`)

use std::fmt::Write;

use thiserror::Error;

use crate::diff_tracker::{DiffHunk, DiffLine, UnifiedDiff};

// =============================================================================
// Configuration constants
// =============================================================================

/// Font size in pixels for code content.
const FONT_SIZE: u32 = 14;

/// Line height in pixels.
const LINE_HEIGHT: u32 = 20;

/// Left padding for the entire image.
const PADDING_LEFT: u32 = 10;

/// Top padding for the entire image.
const PADDING_TOP: u32 = 10;

/// Width of the line number gutter.
const GUTTER_WIDTH: u32 = 70;

/// Height of the file name header.
const HEADER_HEIGHT: u32 = 32;

/// Horizontal padding inside the code area.
const CODE_PADDING_LEFT: u32 = 8;

/// Default image width.
const IMAGE_WIDTH: u32 = 900;

/// Spacing between files in combined view.
const FILE_SEPARATOR_HEIGHT: u32 = 16;

// Colors
const COLOR_BG: &str = "#ffffff";
const COLOR_HEADER_BG: &str = "#f6f8fa";
const COLOR_HEADER_TEXT: &str = "#24292f";
const COLOR_ADDITION_BG: &str = "#e6ffec";
const COLOR_ADDITION_TEXT: &str = "#1a7f37";
const COLOR_REMOVAL_BG: &str = "#ffebe9";
const COLOR_REMOVAL_TEXT: &str = "#cf222e";
const COLOR_CONTEXT_TEXT: &str = "#24292f";
const COLOR_HUNK_HEADER_BG: &str = "#ddf4ff";
const COLOR_HUNK_HEADER_TEXT: &str = "#0550ae";
const COLOR_LINE_NUMBER: &str = "#6e7781";
const COLOR_BORDER: &str = "#d0d7de";
const COLOR_BINARY_TEXT: &str = "#57606a";

// =============================================================================
// RenderError
// =============================================================================

/// Errors that can occur during diff image rendering.
#[derive(Debug, Error)]
pub enum RenderError {
    /// The diff contains no content to render.
    #[error("diff contains no content to render")]
    EmptyDiff,

    /// An internal formatting error occurred.
    #[error("internal formatting error: {0}")]
    FormatError(#[from] std::fmt::Error),
}

// =============================================================================
// DiffRenderer
// =============================================================================

/// Renders unified diffs as SVG images.
///
/// Produces SVG markup suitable for display in web UIs, messaging platforms,
/// or conversion to PNG via external tools.
#[derive(Debug, Clone)]
pub struct DiffRenderer {
    /// Width of the rendered image in pixels.
    pub width: u32,
    /// Font family for code content.
    pub font_family: String,
}

impl Default for DiffRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl DiffRenderer {
    /// Create a new DiffRenderer with default settings.
    pub fn new() -> Self {
        Self {
            width: IMAGE_WIDTH,
            font_family: "monospace".to_string(),
        }
    }

    /// Create a new DiffRenderer with a custom width.
    pub fn with_width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    /// Create a new DiffRenderer with a custom font family.
    pub fn with_font_family(mut self, font_family: impl Into<String>) -> Self {
        self.font_family = font_family.into();
        self
    }

    /// Render a single file diff as an SVG image.
    ///
    /// Returns the SVG content as bytes (UTF-8 encoded).
    pub fn render_diff(&self, diff: &UnifiedDiff) -> Result<Vec<u8>, RenderError> {
        let svg = self.render_single_diff_svg(diff)?;
        Ok(svg.into_bytes())
    }

    /// Render multiple file diffs as a single combined SVG image.
    ///
    /// Each file's diff is rendered sequentially with a separator between them.
    /// Returns the SVG content as bytes (UTF-8 encoded).
    pub fn render_combined(&self, diffs: &[UnifiedDiff]) -> Result<Vec<u8>, RenderError> {
        if diffs.is_empty() {
            return Err(RenderError::EmptyDiff);
        }

        let mut svg = String::new();
        let mut total_height = PADDING_TOP;

        // Calculate total height by summing all diff heights
        let mut section_heights = Vec::new();
        for diff in diffs {
            let h = self.calculate_diff_height(diff);
            section_heights.push(h);
            total_height += h + FILE_SEPARATOR_HEIGHT;
        }
        // Remove the last separator
        if !diffs.is_empty() {
            total_height -= FILE_SEPARATOR_HEIGHT;
        }
        total_height += PADDING_TOP; // bottom padding

        // SVG header
        write_svg_header(&mut svg, self.width, total_height)?;

        // Background
        write!(
            &mut svg,
            r#"<rect width="{}" height="{}" fill="{}"/>"#,
            self.width, total_height, COLOR_BG
        )?;

        // Render each diff
        let mut y_offset = PADDING_TOP;
        for (i, diff) in diffs.iter().enumerate() {
            self.render_diff_section(&mut svg, diff, y_offset)?;
            y_offset += section_heights[i] + FILE_SEPARATOR_HEIGHT;
        }

        // SVG footer
        svg.push_str("</svg>");

        Ok(svg.into_bytes())
    }

    /// Render a single diff as a complete SVG document.
    fn render_single_diff_svg(&self, diff: &UnifiedDiff) -> Result<String, RenderError> {
        let mut svg = String::new();
        let height = self.calculate_diff_height(diff) + PADDING_TOP * 2;

        // SVG header
        write_svg_header(&mut svg, self.width, height)?;

        // Background
        write!(
            &mut svg,
            r#"<rect width="{}" height="{}" fill="{}"/>"#,
            self.width, height, COLOR_BG
        )?;

        // Render the diff content
        self.render_diff_section(&mut svg, diff, PADDING_TOP)?;

        // SVG footer
        svg.push_str("</svg>");

        Ok(svg)
    }

    /// Render a diff section at the given y offset.
    fn render_diff_section(
        &self,
        svg: &mut String,
        diff: &UnifiedDiff,
        y_offset: u32,
    ) -> Result<(), RenderError> {
        let mut y = y_offset;

        // File name header
        self.render_file_header(svg, diff, y)?;
        y += HEADER_HEIGHT;

        if diff.is_binary {
            // Binary file indicator
            self.render_binary_indicator(svg, y)?;
            return Ok(());
        }

        // Render each hunk
        for hunk in &diff.hunks {
            // Hunk header (@@ ... @@)
            self.render_hunk_header(svg, hunk, y)?;
            y += LINE_HEIGHT;

            // Render lines
            let mut old_line = hunk.old_start;
            let mut new_line = hunk.new_start;

            for line in &hunk.lines {
                self.render_diff_line(svg, line, old_line, new_line, y)?;

                match line {
                    DiffLine::Context(_) => {
                        old_line += 1;
                        new_line += 1;
                    }
                    DiffLine::Addition(_) => {
                        new_line += 1;
                    }
                    DiffLine::Removal(_) => {
                        old_line += 1;
                    }
                }

                y += LINE_HEIGHT;
            }
        }

        // Bottom border
        write!(
            svg,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="1"/>"#,
            PADDING_LEFT,
            y,
            self.width - PADDING_LEFT,
            y,
            COLOR_BORDER
        )?;

        Ok(())
    }

    /// Render the file name header bar.
    fn render_file_header(
        &self,
        svg: &mut String,
        diff: &UnifiedDiff,
        y: u32,
    ) -> Result<(), RenderError> {
        let file_name = diff.new_path.display().to_string();
        let stats = if diff.is_binary {
            "Binary file".to_string()
        } else {
            format!("+{} -{}", diff.lines_added, diff.lines_removed)
        };

        // Header background
        write!(
            svg,
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" stroke="{}" stroke-width="1" rx="4" ry="4"/>"#,
            PADDING_LEFT,
            y,
            self.width - PADDING_LEFT * 2,
            HEADER_HEIGHT,
            COLOR_HEADER_BG,
            COLOR_BORDER
        )?;

        // File name text
        write!(
            svg,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}" font-weight="bold" fill="{}">{}</text>"#,
            PADDING_LEFT + CODE_PADDING_LEFT,
            y + HEADER_HEIGHT / 2 + FONT_SIZE / 3,
            self.font_family,
            FONT_SIZE,
            COLOR_HEADER_TEXT,
            escape_xml(&file_name)
        )?;

        // Stats text (right-aligned)
        write!(
            svg,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}" fill="{}" text-anchor="end">{}</text>"#,
            self.width - PADDING_LEFT - CODE_PADDING_LEFT,
            y + HEADER_HEIGHT / 2 + FONT_SIZE / 3,
            self.font_family,
            FONT_SIZE - 2,
            COLOR_LINE_NUMBER,
            escape_xml(&stats)
        )?;

        Ok(())
    }

    /// Render a hunk header line (@@ -old,count +new,count @@).
    fn render_hunk_header(
        &self,
        svg: &mut String,
        hunk: &DiffHunk,
        y: u32,
    ) -> Result<(), RenderError> {
        let header_text = format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        );

        // Background
        write!(
            svg,
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
            PADDING_LEFT,
            y,
            self.width - PADDING_LEFT * 2,
            LINE_HEIGHT,
            COLOR_HUNK_HEADER_BG
        )?;

        // Text
        write!(
            svg,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}" fill="{}">{}</text>"#,
            PADDING_LEFT + CODE_PADDING_LEFT,
            y + LINE_HEIGHT - (LINE_HEIGHT - FONT_SIZE) / 2 - 2,
            self.font_family,
            FONT_SIZE - 1,
            COLOR_HUNK_HEADER_TEXT,
            escape_xml(&header_text)
        )?;

        Ok(())
    }

    /// Render a single diff line (context, addition, or removal).
    fn render_diff_line(
        &self,
        svg: &mut String,
        line: &DiffLine,
        old_line_num: usize,
        new_line_num: usize,
        y: u32,
    ) -> Result<(), RenderError> {
        let (bg_color, text_color, prefix, content, left_num, right_num) = match line {
            DiffLine::Context(text) => (
                COLOR_BG,
                COLOR_CONTEXT_TEXT,
                " ",
                text.as_str(),
                format!("{}", old_line_num),
                format!("{}", new_line_num),
            ),
            DiffLine::Addition(text) => (
                COLOR_ADDITION_BG,
                COLOR_ADDITION_TEXT,
                "+",
                text.as_str(),
                String::new(),
                format!("{}", new_line_num),
            ),
            DiffLine::Removal(text) => (
                COLOR_REMOVAL_BG,
                COLOR_REMOVAL_TEXT,
                "-",
                text.as_str(),
                format!("{}", old_line_num),
                String::new(),
            ),
        };

        // Line background
        write!(
            svg,
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
            PADDING_LEFT,
            y,
            self.width - PADDING_LEFT * 2,
            LINE_HEIGHT,
            bg_color
        )?;

        // Left line number (old)
        write!(
            svg,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}" fill="{}" text-anchor="end">{}</text>"#,
            PADDING_LEFT + GUTTER_WIDTH / 2 - 4,
            y + LINE_HEIGHT - (LINE_HEIGHT - FONT_SIZE) / 2 - 2,
            self.font_family,
            FONT_SIZE - 2,
            COLOR_LINE_NUMBER,
            left_num
        )?;

        // Right line number (new)
        write!(
            svg,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}" fill="{}" text-anchor="end">{}</text>"#,
            PADDING_LEFT + GUTTER_WIDTH - 4,
            y + LINE_HEIGHT - (LINE_HEIGHT - FONT_SIZE) / 2 - 2,
            self.font_family,
            FONT_SIZE - 2,
            COLOR_LINE_NUMBER,
            right_num
        )?;

        // Prefix (+, -, or space)
        let prefix_x = PADDING_LEFT + GUTTER_WIDTH + CODE_PADDING_LEFT;
        write!(
            svg,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}" fill="{}">{}</text>"#,
            prefix_x,
            y + LINE_HEIGHT - (LINE_HEIGHT - FONT_SIZE) / 2 - 2,
            self.font_family,
            FONT_SIZE,
            text_color,
            prefix
        )?;

        // Code content (truncated to fit width)
        let code_x = prefix_x + FONT_SIZE; // offset past the prefix character
        let max_chars = ((self.width - code_x - PADDING_LEFT) / (FONT_SIZE * 6 / 10)) as usize;
        let display_content = if content.len() > max_chars {
            &content[..max_chars.min(content.len())]
        } else {
            content
        };

        write!(
            svg,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}" fill="{}">{}</text>"#,
            code_x,
            y + LINE_HEIGHT - (LINE_HEIGHT - FONT_SIZE) / 2 - 2,
            self.font_family,
            FONT_SIZE,
            text_color,
            escape_xml(display_content)
        )?;

        Ok(())
    }

    /// Render a binary file change indicator.
    fn render_binary_indicator(&self, svg: &mut String, y: u32) -> Result<(), RenderError> {
        write!(
            svg,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}" fill="{}" font-style="italic">Binary file changed (no line-level diff available)</text>"#,
            PADDING_LEFT + CODE_PADDING_LEFT,
            y + LINE_HEIGHT - (LINE_HEIGHT - FONT_SIZE) / 2 - 2,
            self.font_family,
            FONT_SIZE,
            COLOR_BINARY_TEXT
        )?;

        Ok(())
    }

    /// Calculate the total height needed for a diff section.
    fn calculate_diff_height(&self, diff: &UnifiedDiff) -> u32 {
        let mut height = HEADER_HEIGHT; // file header

        if diff.is_binary {
            height += LINE_HEIGHT; // binary indicator line
            return height;
        }

        for hunk in &diff.hunks {
            height += LINE_HEIGHT; // hunk header
            height += (hunk.lines.len() as u32) * LINE_HEIGHT; // diff lines
        }

        // If no hunks (no changes), show a minimal height
        if diff.hunks.is_empty() {
            height += LINE_HEIGHT;
        }

        height
    }
}

// =============================================================================
// Helper functions
// =============================================================================

/// Write the SVG document header.
fn write_svg_header(svg: &mut String, width: u32, height: u32) -> Result<(), std::fmt::Error> {
    write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"#,
        width, height, width, height
    )
}

/// Escape special XML characters in text content.
fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::diff_tracker::{generate_unified_diff, DiffHunk, DiffLine, UnifiedDiff};
    use std::path::Path;

    fn sample_diff() -> UnifiedDiff {
        generate_unified_diff(
            Path::new("src/main.rs"),
            Path::new("src/main.rs"),
            "fn main() {\n    println!(\"Hello\");\n}\n",
            "fn main() {\n    println!(\"Hello, world!\");\n    eprintln!(\"debug\");\n}\n",
            3,
        )
    }

    fn binary_diff() -> UnifiedDiff {
        UnifiedDiff {
            old_path: PathBuf::from("image.png"),
            new_path: PathBuf::from("image.png"),
            is_binary: true,
            hunks: Vec::new(),
            lines_added: 0,
            lines_removed: 0,
        }
    }

    #[test]
    fn test_render_single_diff_produces_valid_svg() {
        let renderer = DiffRenderer::new();
        let diff = sample_diff();
        let result = renderer.render_diff(&diff);

        assert!(result.is_ok());
        let svg_bytes = result.unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        // Verify it's valid SVG structure
        assert!(svg_str.starts_with("<svg"));
        assert!(svg_str.ends_with("</svg>"));
        assert!(svg_str.contains("xmlns=\"http://www.w3.org/2000/svg\""));
    }

    #[test]
    fn test_render_diff_contains_file_name() {
        let renderer = DiffRenderer::new();
        let diff = sample_diff();
        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        assert!(svg_str.contains("src/main.rs"));
    }

    #[test]
    fn test_render_diff_contains_line_numbers() {
        let renderer = DiffRenderer::new();
        let diff = sample_diff();
        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        // Should contain line numbers
        assert!(svg_str.contains(">1<"));
        assert!(svg_str.contains(">2<"));
    }

    #[test]
    fn test_render_diff_has_addition_highlighting() {
        let renderer = DiffRenderer::new();
        let diff = sample_diff();
        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        // Should have green background for additions
        assert!(svg_str.contains(COLOR_ADDITION_BG));
    }

    #[test]
    fn test_render_diff_has_removal_highlighting() {
        let renderer = DiffRenderer::new();
        let diff = sample_diff();
        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        // Should have red background for removals
        assert!(svg_str.contains(COLOR_REMOVAL_BG));
    }

    #[test]
    fn test_render_diff_has_hunk_header() {
        let renderer = DiffRenderer::new();
        let diff = sample_diff();
        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        // Should have hunk header with blue background
        assert!(svg_str.contains(COLOR_HUNK_HEADER_BG));
        assert!(svg_str.contains("@@"));
    }

    #[test]
    fn test_render_binary_diff() {
        let renderer = DiffRenderer::new();
        let diff = binary_diff();
        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        assert!(svg_str.contains("Binary file changed"));
        assert!(svg_str.contains("image.png"));
    }

    #[test]
    fn test_render_combined_multiple_files() {
        let renderer = DiffRenderer::new();
        let diff1 = generate_unified_diff(
            Path::new("file_a.rs"),
            Path::new("file_a.rs"),
            "fn a() {}\n",
            "fn a() { todo!() }\n",
            3,
        );
        let diff2 = generate_unified_diff(
            Path::new("file_b.rs"),
            Path::new("file_b.rs"),
            "fn b() {}\n",
            "fn b() { todo!() }\n",
            3,
        );

        let result = renderer.render_combined(&[diff1, diff2]);
        assert!(result.is_ok());

        let svg_str = String::from_utf8(result.unwrap()).unwrap();
        assert!(svg_str.contains("file_a.rs"));
        assert!(svg_str.contains("file_b.rs"));
    }

    #[test]
    fn test_render_combined_empty_returns_error() {
        let renderer = DiffRenderer::new();
        let result = renderer.render_combined(&[]);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RenderError::EmptyDiff));
    }

    #[test]
    fn test_render_combined_single_file() {
        let renderer = DiffRenderer::new();
        let diff = sample_diff();
        let result = renderer.render_combined(&[diff]);
        assert!(result.is_ok());

        let svg_str = String::from_utf8(result.unwrap()).unwrap();
        assert!(svg_str.starts_with("<svg"));
        assert!(svg_str.ends_with("</svg>"));
    }

    #[test]
    fn test_render_diff_escapes_xml_special_chars() {
        let renderer = DiffRenderer::new();
        let diff = generate_unified_diff(
            Path::new("test.html"),
            Path::new("test.html"),
            "<div>old</div>\n",
            "<div>new & improved</div>\n",
            3,
        );

        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        // XML special chars should be escaped
        assert!(svg_str.contains("&lt;div&gt;"));
        assert!(svg_str.contains("&amp;"));
        // Should NOT contain unescaped angle brackets in text content
        assert!(!svg_str.contains("<div>old</div>"));
    }

    #[test]
    fn test_render_diff_with_custom_width() {
        let renderer = DiffRenderer::new().with_width(1200);
        let diff = sample_diff();
        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        assert!(svg_str.contains("width=\"1200\""));
    }

    #[test]
    fn test_render_no_changes_diff() {
        let renderer = DiffRenderer::new();
        let diff = generate_unified_diff(
            Path::new("unchanged.rs"),
            Path::new("unchanged.rs"),
            "same content\n",
            "same content\n",
            3,
        );

        let result = renderer.render_diff(&diff);
        assert!(result.is_ok());

        let svg_str = String::from_utf8(result.unwrap()).unwrap();
        assert!(svg_str.contains("unchanged.rs"));
        assert!(svg_str.contains("+0 -0"));
    }

    #[test]
    fn test_render_new_file_diff() {
        let renderer = DiffRenderer::new();
        let diff = generate_unified_diff(
            Path::new("new_file.rs"),
            Path::new("new_file.rs"),
            "",
            "fn new() {}\n",
            3,
        );

        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        assert!(svg_str.contains("new_file.rs"));
        assert!(svg_str.contains(COLOR_ADDITION_BG));
    }

    #[test]
    fn test_render_deleted_file_diff() {
        let renderer = DiffRenderer::new();
        let diff = generate_unified_diff(
            Path::new("deleted.rs"),
            Path::new("deleted.rs"),
            "fn old() {}\n",
            "",
            3,
        );

        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        assert!(svg_str.contains("deleted.rs"));
        assert!(svg_str.contains(COLOR_REMOVAL_BG));
    }

    #[test]
    fn test_render_combined_with_binary_and_text() {
        let renderer = DiffRenderer::new();
        let text_diff = sample_diff();
        let bin_diff = binary_diff();

        let result = renderer.render_combined(&[text_diff, bin_diff]);
        assert!(result.is_ok());

        let svg_str = String::from_utf8(result.unwrap()).unwrap();
        assert!(svg_str.contains("src/main.rs"));
        assert!(svg_str.contains("image.png"));
        assert!(svg_str.contains("Binary file changed"));
    }

    #[test]
    fn test_diff_renderer_default() {
        let renderer = DiffRenderer::default();
        assert_eq!(renderer.width, IMAGE_WIDTH);
        assert_eq!(renderer.font_family, "monospace");
    }

    #[test]
    fn test_render_diff_stats_in_header() {
        let renderer = DiffRenderer::new();
        let diff = generate_unified_diff(
            Path::new("stats.rs"),
            Path::new("stats.rs"),
            "a\nb\nc\n",
            "a\nx\ny\nz\nc\n",
            3,
        );

        let svg_bytes = renderer.render_diff(&diff).unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();

        // Should show +3 -1 (removed b, added x, y, z)
        assert!(svg_str.contains("+3 -1"));
    }
}
