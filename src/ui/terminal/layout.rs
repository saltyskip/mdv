use std::path::Path;

use anyhow::Result;
use crossterm::terminal;
use tracing::info_span;

use crate::{
    core::{config::AppConfig, document::Document},
    io::snapshot::{SnapshotSize, render_html_to_raster},
    render::github_html::build_github_html_with_palette,
    ui::{
        page_graphics::{GraphicPage, build_graphic_page},
        terminal::CellMetrics,
    },
};

const CSS_LINE_HEIGHT_PX: f32 = 24.0;
pub(super) const ZOOM_PERCENT_RANGE: std::ops::RangeInclusive<u16> = 50..=300;
const MAX_ARTICLE_CSS_PX: f32 = 980.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PageGeometry {
    pub width_cells: u16,
    pub left_cells: u16,
    pub display_width_px: u32,
    pub zoom: f32,
}

impl PageGeometry {
    pub(super) fn current(config: &AppConfig, cell_metrics: CellMetrics) -> Self {
        let viewport_width = terminal::size().map_or(80, |(width, _)| width).max(1);
        let zoom =
            cell_metrics.height_px / CSS_LINE_HEIGHT_PX * f32::from(config.zoom_percent) / 100.0;
        let max_width_cells =
            (MAX_ARTICLE_CSS_PX * zoom / cell_metrics.width_px).floor().max(1.0) as u16;
        let width_cells = viewport_width.min(max_width_cells);
        Self {
            width_cells,
            left_cells: (viewport_width - width_cells) / 2,
            display_width_px: (f32::from(width_cells) * cell_metrics.width_px).round().max(1.0)
                as u32,
            zoom,
        }
    }

    pub(super) fn renders_like(self, other: Self) -> bool {
        Self { left_cells: 0, ..self } == Self { left_cells: 0, ..other }
    }
}

pub(super) fn render_graphic_page(
    config: &AppConfig,
    document: &Document,
    source_text: &str,
    geometry: PageGeometry,
    first_screen_px: Option<u32>,
) -> Result<GraphicPage> {
    let _span = info_span!("startup.render_graphic_page").entered();
    if cfg!(test) {
        anyhow::bail!("graphic mode disabled during tests");
    }

    let snapshot_width_px =
        ((geometry.display_width_px as f32) / geometry.zoom).round().max(1.0) as u32;
    let base_dir = document
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let html = {
        let _span = info_span!("startup.build_github_html").entered();
        build_github_html_with_palette(
            source_text,
            base_dir,
            config.theme,
            config.mermaid_mode,
            config.palette,
        )?
    };
    let snapshot = {
        let _span = info_span!("startup.render_html_to_raster").entered();
        render_html_to_raster(
            &html,
            base_dir,
            SnapshotSize {
                css_width: snapshot_width_px,
                output_width_px: Some(geometry.display_width_px),
                max_css_height: first_screen_px
                    .map(|px| (px as f32 / geometry.zoom).ceil().max(1.0) as u32),
            },
        )?
    };

    {
        let _span = info_span!("startup.build_graphic_page").entered();
        let page = build_graphic_page(&snapshot, geometry.width_cells, geometry.display_width_px)?;
        Ok(GraphicPage { left_cells: geometry.left_cells, ..page }.at_display_resolution())
    }
}

pub(super) fn fit_to_width(line: &str, width: usize) -> String {
    let mut fitted: String = line.chars().take(width).collect();
    let current = fitted.chars().count();
    if current < width {
        fitted.push_str(&" ".repeat(width - current));
    }
    fitted
}

pub(super) fn current_cell_metrics() -> CellMetrics {
    let Ok(window) = terminal::window_size() else {
        return CellMetrics { width_px: 8.0, height_px: 16.0 };
    };
    if window.columns == 0 || window.rows == 0 || window.width == 0 || window.height == 0 {
        return CellMetrics { width_px: 8.0, height_px: 16.0 };
    }

    let cell_width = f32::from(window.width) / f32::from(window.columns);
    let cell_height = f32::from(window.height) / f32::from(window.rows);
    if cell_width <= 0.0 || cell_height <= 0.0 {
        CellMetrics { width_px: 8.0, height_px: 16.0 }
    } else {
        CellMetrics { width_px: cell_width, height_px: cell_height }
    }
}

pub(super) fn article_origin(viewport_width: u16) -> u16 {
    viewport_width.saturating_sub(article_wrap_width(viewport_width)) / 2
}

pub(super) fn article_wrap_width(viewport_width: u16) -> u16 {
    let usable = viewport_width.saturating_sub(8);
    if usable >= 24 { usable.min(96) } else { viewport_width.saturating_sub(2).max(1) }
}

pub(super) fn truncate_visible(text: &str, width: usize) -> String {
    text.chars().take(width.max(1)).collect()
}
