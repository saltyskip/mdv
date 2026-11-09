use std::{
    io::Cursor,
    time::{Duration, Instant},
};

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};

use crate::ui::page_graphics::build_graphic_page;

use super::*;

const METRICS: CellMetrics = CellMetrics { width_px: 8.0, height_px: 2.0 };
const ROWS: usize = 4;
const STRIDE_PX: u32 = 16;
const TILE_BYTES: u64 = (STRIDE_PX as u64 + 8) * 8 * 3;

fn page(height_px: u32) -> GraphicPage {
    let image = ImageBuffer::from_fn(8, height_px, |x, y| {
        Rgba([x as u8, (y % 256) as u8, (y / 256) as u8, 255])
    });
    let mut png = Vec::new();
    DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .unwrap_or_else(|error| panic!("fixture should encode: {error}"));
    build_graphic_page(&png, 16, 8)
        .unwrap_or_else(|error| panic!("fixture page should decode: {error}"))
}

fn graphics(transfer: Transfer, budget_bytes: u64) -> PageGraphics {
    let mut graphics = PageGraphics::new(transfer, budget_bytes);
    graphics.stride_px = STRIDE_PX;
    graphics
}

fn settle(graphics: &mut PageGraphics) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut commands = Vec::new();
    while graphics.has_pending_work() {
        assert!(Instant::now() < deadline, "tile preparation did not finish");
        commands.extend(
            graphics
                .maintenance()
                .unwrap_or_else(|error| panic!("maintenance should succeed: {error}")),
        );
        thread::yield_now();
    }
    commands
}

fn count(commands: &[String], needle: &str) -> usize {
    commands.iter().filter(|command| command.contains(needle)).count()
}

fn resident(graphics: &PageGraphics) -> Vec<usize> {
    graphics
        .page
        .as_ref()
        .map(|page| {
            page.tiles
                .iter()
                .enumerate()
                .filter(|(_, tile)| tile.resident)
                .map(|(index, _)| index)
                .collect()
        })
        .unwrap_or_default()
}

fn png_payload(command: &str) -> RgbaImage {
    let (_, payload) =
        command.split_once(';').unwrap_or_else(|| panic!("transmit should carry a payload"));
    let bytes = STANDARD
        .decode(payload.trim_end_matches("\u{1b}\\"))
        .unwrap_or_else(|error| panic!("payload should be base64: {error}"));
    image::load_from_memory(&bytes)
        .unwrap_or_else(|error| panic!("payload should be a png: {error}"))
        .to_rgba8()
}

fn placement_field(command: &str, field: &str) -> u32 {
    command
        .split([',', ';'])
        .find_map(|part| part.strip_prefix(&format!("{field}=")))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("placement should carry {field}: {command}"))
}

#[test]
fn every_viewport_falls_inside_one_tile_and_shows_the_page_pixels() {
    let page = page(100);
    let mut graphics = graphics(Transfer::Direct, u64::MAX);
    let mut transmitted = std::collections::BTreeMap::new();
    let mut visible = Vec::new();

    for tenth in 0..=460 {
        let scroll = f64::from(tenth) / 10.0;
        let (commands, placements) = graphics
            .commands(&page, scroll, ROWS, METRICS, &visible)
            .unwrap_or_else(|error| panic!("viewport renders: {error}"));
        let mut all = commands;
        all.extend(settle(&mut graphics));
        for command in all.iter().filter(|command| command.contains("a=t")) {
            transmitted.insert(placement_field(command, "i"), png_payload(command));
        }
        let place = all
            .iter()
            .find(|command| command.contains("a=p"))
            .unwrap_or_else(|| panic!("a placement"));
        let tile = &transmitted[&placement_field(place, "i")];
        let (y, h) = (placement_field(place, "y"), placement_field(place, "h"));
        assert!(y + h <= tile.height(), "scroll {scroll} overruns its tile");
        let slice = viewport_slice_at(&page, scroll, ROWS, METRICS.height_px);
        assert_eq!(
            image::imageops::crop_imm(tile, 0, y, 8, h).to_image(),
            image::imageops::crop_imm(page.image.as_ref(), 0, slice.source_y_px, 8, h).to_image(),
            "scroll {scroll} shows the wrong pixels"
        );
        visible = placements;
    }
}

#[test]
fn neighbours_load_in_the_background_up_to_the_budget() {
    let page = page(160);
    let mut graphics = graphics(Transfer::Direct, TILE_BYTES * 3);

    let (first, _) = graphics
        .commands(&page, 0.0, ROWS, METRICS, &[])
        .unwrap_or_else(|error| panic!("first viewport: {error}"));
    let prefetched = settle(&mut graphics);

    assert_eq!(count(&first, "a=t"), 1, "only the visible tile blocks the first frame");
    assert_eq!(count(&prefetched, "a=t"), 2);
    assert!(!prefetched.iter().any(|command| command.contains("a=p")));
    assert_eq!(resident(&graphics), vec![0, 1, 2]);
}

#[test]
fn scrolling_across_resident_tiles_sends_only_placements() {
    let page = page(160);
    let mut graphics = graphics(Transfer::Direct, u64::MAX);
    let (_, mut visible) = graphics
        .commands(&page, 0.0, ROWS, METRICS, &[])
        .unwrap_or_else(|error| panic!("first: {error}"));
    let _ = settle(&mut graphics);

    for scroll in [3.0, 9.5, 20.0, 41.25, 76.0, 12.0, 0.0] {
        let (commands, placements) = graphics
            .commands(&page, scroll, ROWS, METRICS, &visible)
            .unwrap_or_else(|error| panic!("viewport renders: {error}"));
        assert_eq!(count(&commands, "a=t"), 0, "scroll {scroll} re-uploaded a tile");
        assert_eq!(count(&commands, "a=p"), 1);
        visible = placements;
    }
}

#[test]
fn a_far_jump_evicts_the_farthest_tiles_first() {
    let page = page(160);
    let mut graphics = graphics(Transfer::Direct, TILE_BYTES * 3);
    let (_, visible) = graphics
        .commands(&page, 0.0, ROWS, METRICS, &[])
        .unwrap_or_else(|error| panic!("first: {error}"));
    let _ = settle(&mut graphics);

    let (jump, _) = graphics
        .commands(&page, 70.0, ROWS, METRICS, &visible)
        .unwrap_or_else(|error| panic!("jump: {error}"));
    let _ = settle(&mut graphics);

    assert_eq!(count(&jump, "a=d,d=I"), 1);
    assert!(jump.iter().any(|command| command.contains(&format!("i={}", tile_image_id(0)))));
    assert_eq!(resident(&graphics), vec![7, 8, 9]);
}

#[test]
fn a_tile_larger_than_the_budget_still_shows() {
    let page = page(160);
    let mut graphics = graphics(Transfer::Direct, 1);

    let (commands, _) = graphics
        .commands(&page, 0.0, ROWS, METRICS, &[])
        .unwrap_or_else(|error| panic!("first: {error}"));

    assert_eq!(count(&commands, "a=t"), 1);
    assert_eq!(count(&commands, "a=p"), 1);
    assert!(!graphics.has_pending_work());
}

#[test]
fn file_transfer_sends_a_path_to_raw_pixels_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("temp dir: {error}"));
    let tiles = dir.path().join("tiles");
    let page = page(160);
    let mut graphics = graphics(Transfer::File(tiles.clone()), u64::MAX);

    let (commands, _) = graphics
        .commands(&page, 0.0, ROWS, METRICS, &[])
        .unwrap_or_else(|error| panic!("first: {error}"));
    let transmit = commands
        .iter()
        .find(|command| command.contains("a=t"))
        .unwrap_or_else(|| panic!("a transmit command"));
    assert!(transmit.contains("t=f,f=24,s=8,v=24"));
    let (_, encoded) = transmit.split_once(';').unwrap_or_else(|| panic!("a path payload"));
    let path = String::from_utf8(
        STANDARD
            .decode(encoded.trim_end_matches("\u{1b}\\"))
            .unwrap_or_else(|error| panic!("base64 path: {error}")),
    )
    .unwrap_or_else(|error| panic!("utf-8 path: {error}"));
    let pixels = fs::read(&path).unwrap_or_else(|error| panic!("tile file exists: {error}"));
    assert_eq!(
        pixels,
        DynamicImage::ImageRgba8(
            image::imageops::crop_imm(page.image.as_ref(), 0, 0, 8, 24).to_image()
        )
        .to_rgb8()
        .into_raw()
    );

    let _ = settle(&mut graphics);
    let deletes = graphics.reset();

    assert_eq!(deletes.len(), 10);
    assert!(!tiles.exists(), "reset removes the tile files");
}

#[test]
fn a_replaced_page_invalidates_every_tile() {
    let first_page = page(160);
    let second_page = page(160);
    let mut graphics = graphics(Transfer::Direct, TILE_BYTES * 2);
    let (_, visible) = graphics
        .commands(&first_page, 0.0, ROWS, METRICS, &[])
        .unwrap_or_else(|error| panic!("first: {error}"));
    let _ = settle(&mut graphics);

    let (commands, _) = graphics
        .commands(&second_page, 0.0, ROWS, METRICS, &visible)
        .unwrap_or_else(|error| panic!("replaced page: {error}"));

    assert_eq!(count(&commands, "a=d,d=I"), 2);
    assert_eq!(count(&commands, "a=t"), 1);
}

#[test]
fn reset_discards_an_inflight_preparation() {
    let page = page(160);
    let mut graphics = graphics(Transfer::Direct, u64::MAX);
    let _ = graphics
        .commands(&page, 0.0, ROWS, METRICS, &[])
        .unwrap_or_else(|error| panic!("first: {error}"));

    assert!(graphics.has_pending_work());
    let _ = graphics.reset();

    assert!(!graphics.has_pending_work());
    assert!(
        graphics
            .maintenance()
            .unwrap_or_else(|error| panic!("stale preparation is ignored: {error}"))
            .is_empty()
    );
}

#[test]
fn a_centered_page_is_placed_at_its_left_margin() {
    let page = GraphicPage { left_cells: 7, ..page(160) };
    let mut graphics = graphics(Transfer::Direct, u64::MAX);

    let (commands, _) = graphics
        .commands(&page, 0.0, ROWS, METRICS, &[])
        .unwrap_or_else(|error| panic!("first: {error}"));

    let place = commands
        .iter()
        .find(|command| command.contains("a=p"))
        .unwrap_or_else(|| panic!("a placement"));
    assert!(place.starts_with(&ansi_cursor_move(7, 0)));
}

#[test]
fn a_raster_wider_than_the_pane_is_stored_at_pane_resolution() {
    let image = ImageBuffer::from_pixel(16, 320, Rgba([10, 20, 30, 255]));
    let mut png = Vec::new();
    DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .unwrap_or_else(|error| panic!("fixture should encode: {error}"));

    let page = build_graphic_page(&png, 16, 8)
        .unwrap_or_else(|error| panic!("page should decode: {error}"))
        .at_display_resolution();

    assert_eq!((page.image_width_px, page.image_height_px), (8, 160));
    assert_eq!((page.display_width_px, page.display_height_px), (8, 160));
    assert_eq!(*page.image.get_pixel(4, 80), Rgba([10, 20, 30, 255]));
}
