use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{
    ExtendedColorType, ImageEncoder, RgbaImage,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};

use crate::{
    io::kitty_graphics::{
        DeleteCommand, KittyImagePlacement, encode_delete, encode_place, encode_transmit_png,
    },
    ui::{
        page_graphics::{GraphicPage, ViewportSlice, viewport_slice_at},
        terminal::CellMetrics,
    },
};

use super::ansi_cursor_move;

const TILE_STRIDE_PX: u32 = 4096;
const TILE_IMAGE_ID_BASE: u32 = 0x4d44_0000;
const TILE_IMAGE_ID_BANK: u32 = 0x0001_0000;
const DEFAULT_BUDGET_BYTES: u64 = 120 * 1024 * 1024;

type GraphicCommands = (Vec<String>, Vec<(u32, u32)>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::ui::terminal) enum Transfer {
    File(PathBuf),
    Direct,
}

impl Transfer {
    pub(in crate::ui::terminal) fn from_env() -> Self {
        let remote =
            std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
        match std::env::var("MDV_GRAPHICS_TRANSFER").ok().as_deref() {
            Some("direct") => Self::Direct,
            Some("file") => Self::in_temp_dir(),
            _ if remote => Self::Direct,
            _ => Self::in_temp_dir(),
        }
    }

    fn in_temp_dir() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let instance = NEXT.fetch_add(1, Ordering::Relaxed);
        Self::File(
            std::env::temp_dir().join(format!("mdv-tiles-{}-{instance}", std::process::id())),
        )
    }
}

pub(in crate::ui::terminal) fn budget_from_env() -> u64 {
    std::env::var("MDV_IMAGE_BUDGET_MB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|megabytes| *megabytes > 0)
        .map_or(DEFAULT_BUDGET_BYTES, |megabytes| megabytes * 1024 * 1024)
}

#[derive(Clone, Debug)]
enum Payload {
    File(PathBuf),
    Png(Vec<u8>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PageKey {
    image_ptr: usize,
    image_width_px: u32,
    image_height_px: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TileSpan {
    source_y_px: u32,
    source_height_px: u32,
}

struct Tile {
    span: TileSpan,
    payload: Option<Payload>,
    resident: bool,
}

struct TiledPage {
    key: PageKey,
    overlap_px: u32,
    id_base: u32,
    image: Arc<RgbaImage>,
    tiles: Vec<Tile>,
    current: usize,
    direction: i8,
}

impl TiledPage {
    fn image_id(&self, index: usize) -> u32 {
        self.id_base + index as u32
    }
}

struct Pending {
    generation: u64,
    index: usize,
    receiver: Receiver<Result<Payload>>,
}

pub(in crate::ui::terminal) struct PageGraphics {
    transfer: Transfer,
    budget_bytes: u64,
    stride_px: u32,
    generation: u64,
    page: Option<TiledPage>,
    pending: Option<Pending>,
}

impl Default for PageGraphics {
    fn default() -> Self {
        Self::new(Transfer::Direct, DEFAULT_BUDGET_BYTES)
    }
}

impl Drop for PageGraphics {
    fn drop(&mut self) {
        self.remove_tile_files();
    }
}

impl PageGraphics {
    pub(in crate::ui::terminal) fn new(transfer: Transfer, budget_bytes: u64) -> Self {
        Self {
            transfer,
            budget_bytes,
            stride_px: TILE_STRIDE_PX,
            generation: 0,
            page: None,
            pending: None,
        }
    }

    pub(in crate::ui::terminal) fn commands(
        &mut self,
        page: &GraphicPage,
        scroll_rows: f64,
        content_height: usize,
        cell_metrics: CellMetrics,
        previous_visible_placements: &[(u32, u32)],
    ) -> Result<GraphicCommands> {
        let slice = viewport_slice_at(page, scroll_rows, content_height, cell_metrics.height_px);
        if slice.rows == 0 || slice.source_height_px == 0 {
            return Ok((delete_hidden_placements(previous_visible_placements, &[]), Vec::new()));
        }

        let overlap_px = viewport_slice_at(page, 0.0, content_height, cell_metrics.height_px)
            .source_height_px
            .max(slice.source_height_px);
        let stale_images = self.invalidate_for_page(page, overlap_px);
        let index = self.focus(slice)?;
        let mut commands = self.make_resident_now(index)?;
        commands.extend(self.prefetch()?);

        let tiled = self.tiled()?;
        let image_id = tiled.image_id(index);
        let span = tiled.tiles[index].span;
        let visible_placements = vec![(image_id, image_id)];
        let placement = KittyImagePlacement {
            image_id,
            placement_id: image_id,
            columns: page.width_cells,
            rows: slice.rows,
            source_x_px: 0,
            source_y_px: slice.source_y_px - span.source_y_px,
            source_width_px: page.image_width_px,
            source_height_px: slice.source_height_px,
            cursor_x: 0,
            cursor_y: 0,
            z_index: -1,
        };
        commands.push(format!(
            "{}{}",
            ansi_cursor_move(page.left_cells, 0),
            encode_place(&placement)
        ));
        commands.extend(delete_hidden_placements(previous_visible_placements, &visible_placements));
        commands.extend(stale_images);

        Ok((commands, visible_placements))
    }

    pub(in crate::ui::terminal) fn has_pending_work(&self) -> bool {
        self.pending.is_some() || self.next_candidate().is_some()
    }

    pub(in crate::ui::terminal) fn maintenance(&mut self) -> Result<Vec<String>> {
        let mut commands = Vec::new();
        if let Some(pending) = &self.pending {
            match pending.receiver.try_recv() {
                Ok(result) => {
                    let (generation, index) = (pending.generation, pending.index);
                    self.pending = None;
                    let payload = result?;
                    if generation == self.generation
                        && let Some(page) = self.page.as_mut()
                    {
                        page.tiles[index].payload = Some(payload);
                        commands.extend(self.admit(index).unwrap_or_default());
                    }
                }
                Err(TryRecvError::Empty) => return Ok(commands),
                Err(TryRecvError::Disconnected) => self.pending = None,
            }
        }
        commands.extend(self.prefetch()?);
        Ok(commands)
    }

    pub(in crate::ui::terminal) fn reset(&mut self) -> Vec<String> {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        self.remove_tile_files();
        self.page
            .take()
            .map(|page| {
                page.tiles
                    .iter()
                    .enumerate()
                    .filter(|(_, tile)| tile.resident)
                    .map(|(index, _)| {
                        encode_delete(DeleteCommand::Image { image_id: page.image_id(index) })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn remove_tile_files(&self) {
        if let Transfer::File(dir) = &self.transfer {
            let _ = fs::remove_dir_all(dir);
        }
    }

    fn tiled(&self) -> Result<&TiledPage> {
        self.page.as_ref().ok_or_else(|| anyhow!("no page is tiled"))
    }

    fn invalidate_for_page(&mut self, page: &GraphicPage, needed_overlap_px: u32) -> Vec<String> {
        let key = PageKey {
            image_ptr: page.image.as_ref().as_raw().as_ptr() as usize,
            image_width_px: page.image_width_px,
            image_height_px: page.image_height_px,
        };
        if self
            .page
            .as_ref()
            .is_some_and(|tiled| tiled.key == key && tiled.overlap_px >= needed_overlap_px)
        {
            return Vec::new();
        }
        let overlap_px = needed_overlap_px + needed_overlap_px / 4;

        let deletes = self.reset();
        self.page = Some(TiledPage {
            key,
            overlap_px,
            id_base: TILE_IMAGE_ID_BASE + (self.generation % 2) as u32 * TILE_IMAGE_ID_BANK,
            image: Arc::clone(&page.image),
            tiles: tile_spans(page.image_height_px, self.stride_px, overlap_px)
                .into_iter()
                .map(|span| Tile { span, payload: None, resident: false })
                .collect(),
            current: 0,
            direction: 1,
        });
        deletes
    }

    fn focus(&mut self, slice: ViewportSlice) -> Result<usize> {
        let stride_px = self.stride_px;
        let page = self.page.as_mut().ok_or_else(|| anyhow!("no page is tiled"))?;
        let index = ((slice.source_y_px / stride_px) as usize).min(page.tiles.len() - 1);
        if index != page.current {
            page.direction = if index > page.current { 1 } else { -1 };
            page.current = index;
        }
        Ok(index)
    }

    fn make_resident_now(&mut self, index: usize) -> Result<Vec<String>> {
        let page = self.page.as_ref().ok_or_else(|| anyhow!("no page is tiled"))?;
        if page.tiles[index].resident {
            return Ok(Vec::new());
        }
        if page.tiles[index].payload.is_none() {
            let payload = match self.pending.take_if(|pending| pending.index == index) {
                Some(pending) => pending
                    .receiver
                    .recv()
                    .context("tile preparation stopped before it finished")??,
                None => prepare_tile(
                    &page.image,
                    page.tiles[index].span,
                    &self.transfer,
                    self.generation,
                    index,
                )?,
            };
            if let Some(page) = self.page.as_mut() {
                page.tiles[index].payload = Some(payload);
            }
        }
        self.admit_evicting_anything(index)
    }

    fn admit_evicting_anything(&mut self, index: usize) -> Result<Vec<String>> {
        let evictions =
            self.evictions_for(index).unwrap_or_else(|| self.all_other_residents(index));
        self.apply_admission(index, evictions)
    }

    fn admit(&mut self, index: usize) -> Option<Vec<String>> {
        let evictions = self.evictions_for(index)?;
        self.apply_admission(index, evictions).ok()
    }

    fn apply_admission(&mut self, index: usize, evictions: Vec<usize>) -> Result<Vec<String>> {
        let page = self.page.as_mut().ok_or_else(|| anyhow!("no page is tiled"))?;
        let width_px = page.key.image_width_px;
        let tile = &page.tiles[index];
        let payload =
            tile.payload.as_ref().ok_or_else(|| anyhow!("tile {index} has no payload"))?;
        let transmit =
            transmit(page.image_id(index), payload, width_px, tile.span.source_height_px);

        let mut commands: Vec<String> = evictions
            .into_iter()
            .map(|evicted| {
                page.tiles[evicted].resident = false;
                encode_delete(DeleteCommand::Image { image_id: page.image_id(evicted) })
            })
            .collect();
        page.tiles[index].resident = true;
        commands.push(transmit);
        Ok(commands)
    }

    fn evictions_for(&self, index: usize) -> Option<Vec<usize>> {
        let page = self.page.as_ref()?;
        let width_px = page.key.image_width_px;
        let incoming = tile_bytes(page.tiles[index].span, width_px);
        let distance = page.current.abs_diff(index);
        let mut resident_bytes: u64 = page
            .tiles
            .iter()
            .filter(|tile| tile.resident)
            .map(|tile| tile_bytes(tile.span, width_px))
            .sum();

        let mut farther: Vec<usize> = page
            .tiles
            .iter()
            .enumerate()
            .filter(|(other, tile)| tile.resident && page.current.abs_diff(*other) > distance)
            .map(|(other, _)| other)
            .collect();
        farther.sort_by_key(|other| std::cmp::Reverse(page.current.abs_diff(*other)));

        let mut evictions = Vec::new();
        let mut farther = farther.into_iter();
        while resident_bytes + incoming > self.budget_bytes {
            let evicted = farther.next()?;
            resident_bytes -= tile_bytes(page.tiles[evicted].span, width_px);
            evictions.push(evicted);
        }
        Some(evictions)
    }

    fn all_other_residents(&self, index: usize) -> Vec<usize> {
        self.page
            .as_ref()
            .map(|page| {
                page.tiles
                    .iter()
                    .enumerate()
                    .filter(|(other, tile)| tile.resident && *other != index)
                    .map(|(other, _)| other)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn next_candidate(&self) -> Option<usize> {
        let page = self.page.as_ref()?;
        let pending_index = self.pending.as_ref().map(|pending| pending.index);
        let mut candidates: Vec<usize> = (0..page.tiles.len())
            .filter(|index| !page.tiles[*index].resident && Some(*index) != pending_index)
            .collect();
        candidates.sort_by_key(|index| {
            let ahead = (*index as i64 - page.current as i64).signum() == i64::from(page.direction);
            (page.current.abs_diff(*index), !ahead)
        });
        candidates.into_iter().next().filter(|index| self.evictions_for(*index).is_some())
    }

    fn prefetch(&mut self) -> Result<Vec<String>> {
        if self.pending.is_some() {
            return Ok(Vec::new());
        }
        let Some(index) = self.next_candidate() else {
            return Ok(Vec::new());
        };
        let page = self.tiled()?;
        if page.tiles[index].payload.is_some() {
            return Ok(self.admit(index).unwrap_or_default());
        }

        let image = Arc::clone(&page.image);
        let span = page.tiles[index].span;
        let transfer = self.transfer.clone();
        let generation = self.generation;
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("mdv-page-tiles".to_string())
            .spawn(move || {
                let _ = sender.send(prepare_tile(&image, span, &transfer, generation, index));
            })
            .context("failed to start page tile preparation")?;
        self.pending = Some(Pending { generation, index, receiver });
        Ok(Vec::new())
    }
}

fn tile_spans(image_height_px: u32, stride_px: u32, overlap_px: u32) -> Vec<TileSpan> {
    let count = image_height_px.saturating_sub(1) / stride_px + 1;
    (0..count)
        .map(|index| {
            let source_y_px = index * stride_px;
            TileSpan {
                source_y_px,
                source_height_px: (stride_px + overlap_px)
                    .min(image_height_px.saturating_sub(source_y_px))
                    .max(1),
            }
        })
        .collect()
}

fn tile_bytes(span: TileSpan, width_px: u32) -> u64 {
    u64::from(span.source_height_px) * u64::from(width_px) * 3
}

fn tile_pixels(image: &RgbaImage, span: TileSpan) -> Vec<u8> {
    let row_bytes = image.width() as usize * 4;
    let start = span.source_y_px as usize * row_bytes;
    image.as_raw()[start..start + span.source_height_px as usize * row_bytes]
        .chunks_exact(4)
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect()
}

fn prepare_tile(
    image: &RgbaImage,
    span: TileSpan,
    transfer: &Transfer,
    generation: u64,
    index: usize,
) -> Result<Payload> {
    let pixels = tile_pixels(image, span);
    match transfer {
        Transfer::File(dir) => {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
            let path = dir.join(format!("{generation}-{index}.rgba"));
            fs::write(&path, &pixels)
                .with_context(|| format!("failed to write {}", path.display()))?;
            Ok(Payload::File(path))
        }
        Transfer::Direct => {
            let mut png = Vec::new();
            PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
                .write_image(&pixels, image.width(), span.source_height_px, ExtendedColorType::Rgb8)
                .context("failed to encode page tile")?;
            Ok(Payload::Png(png))
        }
    }
}

fn transmit(image_id: u32, payload: &Payload, width_px: u32, height_px: u32) -> String {
    match payload {
        Payload::File(path) => format!(
            "\u{1b}_Ga=t,t=f,f=24,s={width_px},v={height_px},i={image_id},q=2;{}\u{1b}\\",
            STANDARD.encode(path.as_os_str().as_encoded_bytes())
        ),
        Payload::Png(bytes) => encode_transmit_png(image_id, bytes),
    }
}

fn delete_hidden_placements(
    previous_visible_placements: &[(u32, u32)],
    visible_placements: &[(u32, u32)],
) -> Vec<String> {
    previous_visible_placements
        .iter()
        .copied()
        .filter(|placement| !visible_placements.contains(placement))
        .map(|(image_id, placement_id)| {
            encode_delete(DeleteCommand::Placement { image_id, placement_id })
        })
        .collect()
}

#[cfg(test)]
#[path = "resident_tests.rs"]
mod tests;
