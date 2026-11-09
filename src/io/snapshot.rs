//! Platform-selected HTML rasterization for the interactive graphics pipeline.

use std::path::Path;

use anyhow::Result;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use anyhow::bail;

/// Renders an HTML document into the lossless raster consumed by `GraphicPage`.
pub fn render_html_to_raster(
    html: &str,
    base_dir: &Path,
    width: u32,
    output_width_px: Option<u32>,
) -> Result<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        crate::io::webkit_snapshot::render_html_to_tiff(html, base_dir, width, output_width_px)
    }

    #[cfg(target_os = "windows")]
    {
        let _ = output_width_px;
        crate::io::chromium_snapshot::render_html_to_png(html, base_dir, width)
            .map(|result| result.png_bytes)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (html, base_dir, width, output_width_px);
        bail!("interactive HTML rasterization is only supported on macOS and Windows");
    }
}
