//! Platform-selected HTML rasterization for the interactive graphics pipeline.

use std::path::Path;

use anyhow::Result;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use anyhow::bail;

/// Renders an HTML document into the lossless raster consumed by `GraphicPage`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotSize {
    pub css_width: u32,
    pub output_width_px: Option<u32>,
    pub max_css_height: Option<u32>,
}

impl SnapshotSize {
    #[must_use]
    pub const fn css_width(css_width: u32) -> Self {
        Self { css_width, output_width_px: None, max_css_height: None }
    }
}

pub fn render_html_to_raster(html: &str, base_dir: &Path, size: SnapshotSize) -> Result<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        crate::io::webkit_snapshot::render_html_to_tiff(html, base_dir, size)
    }

    #[cfg(target_os = "windows")]
    {
        crate::io::chromium_snapshot::render_html_to_png(html, base_dir, size.css_width)
            .map(|result| result.png_bytes)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (html, base_dir, size);
        bail!("interactive HTML rasterization is only supported on macOS and Windows");
    }
}
