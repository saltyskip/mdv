use std::{
    cmp,
    collections::BTreeSet,
    io::{self, Write},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::Result;
use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEventKind,
    },
    execute, queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{
        self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate,
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    },
};
mod graphics;
mod highlight;
mod layout;
mod scroll;
#[cfg(test)]
mod tests;

use self::{
    graphics::{GraphicsProtocol, PageGraphics, collect_graphics_commands, resize_graphic_space},
    highlight::highlight_code_terminal,
    layout::{
        PageGeometry, ZOOM_PERCENT_RANGE, article_origin, article_wrap_width, current_cell_metrics,
        fit_to_width, render_graphic_page, truncate_visible,
    },
    scroll::ScrollMotion,
};
use crate::render::text::render_document;
use crate::{
    cli::ScrollStyle,
    core::{config::AppConfig, document::Document, theme::ThemeTokens},
    io::fs::FileSystemDocumentSource,
    io::kitty_graphics::{DeleteCommand, encode_delete},
    io::{browser, image_decoder::ImageDecoder, mermaid_cli::MermaidCliRenderer},
    render::{
        markdown::parse_document,
        text::{
            RenderedDocument, RenderedGraphicContent, RenderedLine, RenderedLineKind,
            scaled_graphic_height,
        },
    },
    ui::page_graphics::{GraphicPage, total_rows},
};

const ZOOM_STEP_PERCENT: u16 = 10;
const RESIZE_SETTLE: Duration = Duration::from_millis(150);
const NOTICE_DURATION: Duration = Duration::from_millis(1500);
const DEFAULT_ZOOM_PERCENT: u16 = 100;

#[derive(Clone, Copy)]
enum ScrollAnchor {
    Proportional,
    Row,
}

#[derive(Clone, Copy)]
struct GraphicViewport {
    scroll: usize,
    content_height: usize,
    article_x: u16,
    article_width: u16,
    viewport_width: u16,
    cell_metrics: CellMetrics,
}

#[derive(Clone, Copy)]
struct CellMetrics {
    width_px: f32,
    height_px: f32,
}

pub struct TerminalViewer {
    config: AppConfig,
    source: FileSystemDocumentSource,
    document: Document,
    source_text: String,
    rendered: Option<RenderedDocument>,
    graphic_page: Option<GraphicPage>,
    scroll: usize,
    scroll_motion: ScrollMotion,
    page_graphics: PageGraphics,
    pending_image_deletes: Vec<String>,
    clear_text: bool,
    last_status: Option<String>,
    last_modified: Option<SystemTime>,
    warning: Option<String>,
    needs_redraw: bool,
    pending_layout_refresh: bool,
    clear_all_graphics: bool,
    visible_placements: Vec<(u32, u32)>,
    transmitted_graphics: BTreeSet<u32>,
    mermaid_renderer: MermaidCliRenderer,
    cell_metrics: CellMetrics,
    geometry: PageGeometry,
    layout_requested_at: Instant,
    pending_render: Option<PendingRender>,
    show_status: bool,
    notice: Option<(String, Instant)>,
    status_rows_drawn: u16,
}

struct PendingRender {
    geometry: PageGeometry,
    anchor: ScrollAnchor,
    announce: bool,
    receiver: Receiver<Result<GraphicPage>>,
}

#[must_use]
pub fn is_supported_terminal(term_program: Option<&str>, term: Option<&str>) -> bool {
    graphics_protocol(term_program, term).is_some()
}

fn graphics_protocol(term_program: Option<&str>, term: Option<&str>) -> Option<GraphicsProtocol> {
    if term_program.is_some_and(|value| value.eq_ignore_ascii_case("wezterm")) {
        Some(GraphicsProtocol::Inline)
    } else if term_program.is_some_and(|value| {
        value.eq_ignore_ascii_case("ghostty") || value.eq_ignore_ascii_case("rex")
    }) || term.is_some_and(|value| value.contains("kitty"))
    {
        Some(GraphicsProtocol::Kitty)
    } else {
        None
    }
}

fn current_graphics_protocol() -> GraphicsProtocol {
    graphics_protocol(
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
    .unwrap_or_default()
}

impl TerminalViewer {
    pub fn try_new(
        config: AppConfig,
        source: FileSystemDocumentSource,
        document: Document,
        source_text: String,
    ) -> Result<Self> {
        let cell_metrics = current_cell_metrics();
        let geometry = PageGeometry::current(&config, cell_metrics);
        let last_modified = source.modified_at().ok();
        let first_screen_px = terminal::size()
            .map_or(40, |(_, rows)| rows)
            .saturating_mul(cell_metrics.height_px.ceil() as u16);
        let first_screen_px = u32::from(first_screen_px);
        let (graphic_page, rendered, warning) = match render_graphic_page(
            &config,
            &document,
            &source_text,
            geometry,
            Some(first_screen_px),
        ) {
            Ok(page) => (Some(page), None, None),
            Err(error) => (
                None,
                Some(render_document(
                    &document,
                    config.theme,
                    config.mermaid_mode,
                    article_wrap_width(terminal::size().map_or(80, |(cols, _)| cols)),
                    cell_metrics.width_px / cell_metrics.height_px,
                )),
                Some(format!("graphic mode unavailable: {error}")),
            ),
        };

        let mut viewer = Self {
            config,
            source,
            document,
            source_text,
            rendered,
            graphic_page,
            scroll: 0,
            scroll_motion: ScrollMotion::default(),
            page_graphics: PageGraphics::new(current_graphics_protocol()),
            pending_image_deletes: Vec::new(),
            clear_text: true,
            last_status: None,
            last_modified,
            warning,
            needs_redraw: true,
            pending_layout_refresh: false,
            clear_all_graphics: false,
            visible_placements: Vec::new(),
            transmitted_graphics: BTreeSet::new(),
            mermaid_renderer: MermaidCliRenderer::from_env(),
            cell_metrics,
            geometry,
            layout_requested_at: Instant::now(),
            pending_render: None,
            show_status: false,
            notice: None,
            status_rows_drawn: 0,
        };
        if viewer
            .graphic_page
            .as_ref()
            .is_some_and(|page| page.display_height_px >= first_screen_px)
        {
            viewer.start_render(geometry, ScrollAnchor::Row, false);
        }
        Ok(viewer)
    }

    #[cfg(test)]
    #[must_use]
    pub fn new_for_tests(
        config: AppConfig,
        source: FileSystemDocumentSource,
        document: Document,
        source_text: String,
    ) -> Self {
        let cell_metrics = current_cell_metrics();
        let geometry = PageGeometry::current(&config, cell_metrics);
        let last_modified = source.modified_at().ok();
        let (graphic_page, rendered, warning) =
            match render_graphic_page(&config, &document, &source_text, geometry, None) {
                Ok(page) => (Some(page), None, None),
                Err(error) => (
                    None,
                    Some(render_document(
                        &document,
                        config.theme,
                        config.mermaid_mode,
                        article_wrap_width(terminal::size().map_or(80, |(cols, _)| cols)),
                        cell_metrics.width_px / cell_metrics.height_px,
                    )),
                    Some(format!("graphic mode unavailable: {error}")),
                ),
            };

        Self {
            config,
            source,
            document,
            source_text,
            rendered,
            graphic_page,
            scroll: 0,
            scroll_motion: ScrollMotion::default(),
            page_graphics: PageGraphics::new(current_graphics_protocol()),
            pending_image_deletes: Vec::new(),
            clear_text: true,
            last_status: None,
            last_modified,
            warning,
            needs_redraw: true,
            pending_layout_refresh: false,
            clear_all_graphics: false,
            visible_placements: Vec::new(),
            transmitted_graphics: BTreeSet::new(),
            mermaid_renderer: MermaidCliRenderer::from_env(),
            cell_metrics,
            geometry,
            layout_requested_at: Instant::now(),
            pending_render: None,
            show_status: false,
            notice: None,
            status_rows_drawn: 0,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            cursor::Hide,
            EnableMouseCapture,
            Clear(ClearType::All)
        )?;

        let run_result = self.event_loop();

        disable_raw_mode()?;
        for command in self.release_page_images() {
            write!(io::stdout(), "{command}")?;
        }
        let clear_graphics = if self.page_graphics.is_inline() {
            String::new()
        } else {
            encode_delete(DeleteCommand::AllVisiblePlacements)
        };
        execute!(
            io::stdout(),
            EndSynchronizedUpdate,
            DisableMouseCapture,
            LeaveAlternateScreen,
            cursor::Show,
            Print(clear_graphics)
        )?;
        run_result
    }

    fn release_page_images(&mut self) -> Vec<String> {
        let mut deletes = std::mem::take(&mut self.pending_image_deletes);
        deletes.extend(self.page_graphics.reset());
        deletes
    }

    fn event_loop(&mut self) -> Result<()> {
        const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
        let following_frame = |previous: Instant, now: Instant| {
            let scheduled = previous + FRAME_INTERVAL;
            if scheduled > now { scheduled } else { now + FRAME_INTERVAL }
        };
        let mut next_frame = Instant::now();
        let mut last_motion_tick = Instant::now();
        loop {
            if event::poll(Duration::ZERO)? {
                let events = self.read_pending_events()?;
                if self.apply_event_batch(events) {
                    return Ok(());
                }
            }
            let now = Instant::now();
            self.needs_redraw |= self.poll_layout(now);
            self.needs_redraw |= self.scroll_motion.advance(now.duration_since(last_motion_tick));
            last_motion_tick = now;
            if self.needs_redraw && now >= next_frame {
                self.draw()?;
                self.needs_redraw = false;
                next_frame = following_frame(next_frame, now);
            } else if now >= next_frame && self.page_graphics.has_pending_work() {
                let commands = self.page_graphics.maintenance()?;
                if !commands.is_empty() {
                    let mut stdout = io::stdout().lock();
                    for command in commands {
                        stdout.write_all(command.as_bytes())?;
                    }
                    stdout.flush()?;
                }
                next_frame = following_frame(next_frame, now);
            }
            if self.config.watch && self.maybe_reload() {
                continue;
            }

            let wait = if self.needs_redraw
                || self.scroll_motion.active()
                || self.page_graphics.has_pending_work()
            {
                next_frame.saturating_duration_since(Instant::now()).min(FRAME_INTERVAL)
            } else {
                FRAME_INTERVAL
            };
            if !event::poll(wait)? {
                if self.render_next_visible_graphic() {
                    self.needs_redraw = true;
                }
                continue;
            }
        }
    }

    fn read_pending_events(&mut self) -> Result<Vec<Event>> {
        let mut events = vec![event::read()?];
        while events.len() < 256 && event::poll(Duration::ZERO)? {
            events.push(event::read()?);
        }
        Ok(events)
    }

    fn apply_event_batch<I>(&mut self, events: I) -> bool
    where
        I: IntoIterator<Item = Event>,
    {
        let events = events.into_iter().collect::<Vec<_>>();
        if events.iter().any(|event| matches!(event, Event::Key(key) if key.kind != KeyEventKind::Release && key.code == KeyCode::Char('q'))) {
            return true;
        }
        let mut next_scroll = self.scroll;
        let mut scroll_dirty = false;

        let flush_scroll = |viewer: &mut Self, next_scroll: &mut usize, scroll_dirty: &mut bool| {
            if !*scroll_dirty {
                return;
            }
            viewer.needs_redraw |= viewer.set_scroll(*next_scroll);
            *next_scroll = viewer.scroll;
            *scroll_dirty = false;
        };

        for event in events {
            match event {
                Event::Resize(_, _) => {
                    flush_scroll(self, &mut next_scroll, &mut scroll_dirty);
                    self.cell_metrics = current_cell_metrics();
                    self.pending_layout_refresh = true;
                    self.layout_requested_at = Instant::now();
                    self.needs_redraw = true;
                }
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('j') | KeyCode::Down => {
                            next_scroll =
                                cmp::min(next_scroll.saturating_add(1), self.max_scroll());
                            scroll_dirty = true;
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            next_scroll = next_scroll.saturating_sub(1);
                            scroll_dirty = true;
                        }
                        KeyCode::PageDown => {
                            next_scroll = cmp::min(
                                next_scroll.saturating_add(self.page_height()),
                                self.max_scroll(),
                            );
                            scroll_dirty = true;
                        }
                        KeyCode::PageUp => {
                            next_scroll = next_scroll.saturating_sub(self.page_height());
                            scroll_dirty = true;
                        }
                        KeyCode::Char('g') => {
                            next_scroll = 0;
                            scroll_dirty = true;
                        }
                        KeyCode::Char('G') => {
                            next_scroll = self.max_scroll();
                            scroll_dirty = true;
                        }
                        KeyCode::Char('o') => {
                            flush_scroll(self, &mut next_scroll, &mut scroll_dirty);
                            self.open_focused_link();
                            self.needs_redraw = true;
                        }
                        KeyCode::Char('r') => {
                            flush_scroll(self, &mut next_scroll, &mut scroll_dirty);
                            self.reload();
                            next_scroll = self.scroll;
                            self.needs_redraw = true;
                        }
                        KeyCode::Char('+' | '=') => {
                            self.set_zoom(
                                self.config.zoom_percent.saturating_add(ZOOM_STEP_PERCENT),
                            );
                        }
                        KeyCode::Char('-') => {
                            self.set_zoom(
                                self.config.zoom_percent.saturating_sub(ZOOM_STEP_PERCENT),
                            );
                        }
                        KeyCode::Char('0') => self.set_zoom(DEFAULT_ZOOM_PERCENT),
                        KeyCode::Char('s') => {
                            self.show_status = !self.show_status;
                            self.needs_redraw = true;
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        next_scroll = cmp::min(next_scroll.saturating_add(3), self.max_scroll());
                        scroll_dirty = true;
                    }
                    MouseEventKind::ScrollUp => {
                        next_scroll = next_scroll.saturating_sub(3);
                        scroll_dirty = true;
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        flush_scroll(self, &mut next_scroll, &mut scroll_dirty);
        false
    }

    fn draw(&mut self) -> Result<()> {
        let _span = tracing::info_span!("terminal.draw").entered();
        let mut frame = Vec::new();
        queue!(frame, BeginSynchronizedUpdate)?;
        let (width, height) = terminal::size()?;
        let status_rows = self.status_rows();
        let content_height = usize::from(height.saturating_sub(status_rows));
        let tokens = ThemeTokens::for_theme(self.config.theme);
        if status_rows < self.status_rows_drawn {
            queue!(
                frame,
                cursor::MoveTo(0, height.saturating_sub(1)),
                Clear(ClearType::CurrentLine)
            )?;
        }
        self.status_rows_drawn = status_rows;
        let article_width = article_wrap_width(width);
        let article_x = article_origin(width);

        if self.clear_text || self.rendered.is_some() {
            for row in 0..content_height {
                queue!(frame, cursor::MoveTo(0, row as u16), Clear(ClearType::CurrentLine))?;
                if let Some(rendered) = &self.rendered {
                    let visible_end =
                        rendered.lines.len().min(self.scroll.saturating_add(content_height));
                    let visible_lines = &rendered.lines[self.scroll..visible_end];
                    let Some(line) = visible_lines.get(row) else {
                        continue;
                    };
                    queue!(frame, cursor::MoveTo(article_x, row as u16))?;
                    self.draw_line(&mut frame, line, usize::from(article_width), tokens)?;
                }
            }
            self.clear_text = false;
            self.last_status = None;
        }

        let (graphic_commands, visible_placement_ids) = if let Some(page) = &self.graphic_page {
            self.page_graphics.commands(
                page,
                self.scroll_motion.position(),
                content_height,
                self.cell_metrics,
                &self.visible_placements,
            )?
        } else if let Some(rendered) = &self.rendered {
            collect_graphics_commands(
                rendered,
                GraphicViewport {
                    scroll: self.scroll,
                    content_height,
                    article_x,
                    article_width,
                    viewport_width: width,
                    cell_metrics: self.cell_metrics,
                },
                &self.visible_placements,
                &mut self.transmitted_graphics,
            )
        } else {
            (Vec::new(), Vec::new())
        };

        if self.clear_all_graphics {
            if !self.page_graphics.is_inline() {
                write!(frame, "{}", encode_delete(DeleteCommand::AllVisiblePlacements))?;
            }
            self.clear_all_graphics = false;
        }

        for command in self.pending_image_deletes.drain(..) {
            write!(frame, "{command}")?;
        }
        for command in graphic_commands {
            write!(frame, "{command}")?;
        }
        self.visible_placements = visible_placement_ids;

        if self.graphic_page.is_some() && self.page_graphics.is_inline() {
            self.last_status = None;
        }
        if status_rows == 0 {
            self.last_status = None;
        } else {
            let status = self.status_line(usize::from(width));
            if self.last_status.as_ref() != Some(&status) {
                let muted = self.config.palette.map_or(tokens.muted, |palette| {
                    let muted = palette.muted();
                    Color::Rgb { r: muted.red, g: muted.green, b: muted.blue }
                });
                queue!(
                    frame,
                    cursor::MoveTo(0, height.saturating_sub(1)),
                    ResetColor,
                    Clear(ClearType::CurrentLine),
                    SetForegroundColor(muted),
                    Print(&status),
                    ResetColor
                )?;
                self.last_status = Some(status);
            }
        }
        queue!(frame, EndSynchronizedUpdate)?;
        let mut stdout = io::stdout().lock();
        stdout.write_all(&frame)?;
        stdout.flush()?;
        Ok(())
    }

    fn page_height(&self) -> usize {
        terminal::size()
            .map(|(_, height)| usize::from(height.saturating_sub(self.status_rows())))
            .unwrap_or(20)
    }

    fn status_rows(&self) -> u16 {
        u16::from(
            self.show_status
                || self.notice.is_some()
                || self.warning.is_some()
                || self.pending_render.as_ref().is_some_and(|pending| pending.announce),
        )
    }

    fn show_notice(&mut self, message: String) {
        self.notice = Some((message, Instant::now() + NOTICE_DURATION));
        self.needs_redraw = true;
    }

    fn poll_layout(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if let Some(pending) = &self.pending_render {
            match pending.receiver.try_recv() {
                Ok(result) => {
                    let (geometry, anchor) = (pending.geometry, pending.anchor);
                    self.pending_render = None;
                    self.apply_render(geometry, result, anchor);
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.pending_render = None;
                    changed = true;
                }
            }
        }
        if self.pending_layout_refresh && now >= self.layout_requested_at + RESIZE_SETTLE {
            self.pending_layout_refresh = false;
            self.refresh_layout();
            changed = true;
        }
        if self.notice.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.notice = None;
            changed = true;
        }
        changed
    }

    fn refresh_layout(&mut self) {
        let geometry = PageGeometry::current(&self.config, self.cell_metrics);
        let target = self.pending_render.as_ref().map_or(self.geometry, |pending| pending.geometry);
        if !geometry.renders_like(target) {
            self.start_render(geometry, ScrollAnchor::Proportional, true);
            return;
        }
        if self.pending_render.is_none()
            && let Some(page) = &self.graphic_page
        {
            self.graphic_page =
                Some(GraphicPage { left_cells: geometry.left_cells, ..page.clone() });
            self.geometry = geometry;
            self.clear_text = true;
        }
        let max_scroll = self.max_scroll();
        let _ = self.set_scroll(self.scroll.min(max_scroll));
    }

    fn start_render(&mut self, geometry: PageGeometry, anchor: ScrollAnchor, announce: bool) {
        let (sender, receiver) = mpsc::channel();
        let config = self.config.clone();
        let document = self.document.clone();
        let source_text = self.source_text.clone();
        let spawned = thread::Builder::new().name("mdv-page-render".to_string()).spawn(move || {
            let _ =
                sender.send(render_graphic_page(&config, &document, &source_text, geometry, None));
        });
        match spawned {
            Ok(_) => {
                self.pending_render = Some(PendingRender { geometry, anchor, announce, receiver });
            }
            Err(error) => self.apply_render(
                geometry,
                Err(anyhow::Error::from(error).context("failed to start page rendering")),
                anchor,
            ),
        }
    }

    fn apply_render(
        &mut self,
        geometry: PageGeometry,
        result: Result<GraphicPage>,
        anchor: ScrollAnchor,
    ) {
        let position = self.scroll as f64 / self.max_scroll().max(1) as f64;
        match result {
            Ok(graphic_page) => {
                if self.rendered.take().is_some() {
                    self.transmitted_graphics.clear();
                    self.clear_all_graphics = true;
                }
                self.graphic_page = Some(graphic_page);
                self.warning = None;
            }
            Err(error) => {
                self.pending_image_deletes.extend(self.page_graphics.reset());
                self.graphic_page = None;
                self.rendered = Some(render_document(
                    &self.document,
                    self.config.theme,
                    self.config.mermaid_mode,
                    article_wrap_width(terminal::size().map_or(80, |(cols, _)| cols)),
                    self.cell_metrics.width_px / self.cell_metrics.height_px,
                ));
                self.warning = Some(format!("graphic render failed: {error}"));
                self.transmitted_graphics.clear();
                self.visible_placements.clear();
                self.clear_all_graphics = true;
            }
        }
        self.geometry = geometry;
        self.clear_text = true;
        self.last_status = None;
        let max_scroll = self.max_scroll();
        self.scroll = match anchor {
            ScrollAnchor::Proportional => (position * max_scroll as f64).round() as usize,
            ScrollAnchor::Row => self.scroll,
        }
        .min(max_scroll);
        self.scroll_motion.jump(self.scroll);
        self.needs_redraw = true;
    }

    fn max_scroll(&self) -> usize {
        if let Some(page) = &self.graphic_page {
            total_rows(page, self.cell_metrics.height_px).saturating_sub(self.page_height())
        } else if let Some(rendered) = &self.rendered {
            rendered.lines.len().saturating_sub(self.page_height())
        } else {
            0
        }
    }

    fn maybe_reload(&mut self) -> bool {
        let Ok(modified) = self.source.modified_at() else {
            return false;
        };

        let should_reload = self.last_modified.is_none_or(|current| modified > current);

        if should_reload {
            self.reload();
            self.last_modified = Some(modified);
            return true;
        }
        false
    }

    fn reload(&mut self) {
        match self.source.read_to_string() {
            Ok(content) => match parse_document(self.source.path().clone(), &content) {
                Ok(document) => {
                    self.document = document;
                    self.source_text = content;
                    self.rerender();
                    self.show_notice("reloaded".to_string());
                    let _ = self.set_scroll(cmp::min(self.scroll, self.max_scroll()));
                }
                Err(error) => {
                    self.warning = Some(format!("reload failed: {error}"));
                }
            },
            Err(error) => {
                self.warning = Some(format!("reload failed: {error}"));
            }
        }
    }

    fn open_focused_link(&mut self) {
        let Some(url) = self.document.meta.links.first() else {
            self.warning = Some("no link in document".to_string());
            return;
        };

        match browser::open_url(url) {
            Ok(()) => {
                self.warning = Some(format!("opened {url}"));
            }
            Err(error) => {
                self.warning = Some(format!("open failed: {error}"));
            }
        }
    }

    fn rerender(&mut self) {
        let geometry = PageGeometry::current(&self.config, self.cell_metrics);
        let result =
            render_graphic_page(&self.config, &self.document, &self.source_text, geometry, None);
        self.apply_render(geometry, result, ScrollAnchor::Row);
    }

    fn set_zoom(&mut self, percent: u16) {
        let percent = percent.clamp(*ZOOM_PERCENT_RANGE.start(), *ZOOM_PERCENT_RANGE.end());
        if percent == self.config.zoom_percent {
            return;
        }
        self.config.zoom_percent = percent;
        self.pending_layout_refresh = true;
        self.layout_requested_at = Instant::now() - RESIZE_SETTLE;
        self.show_notice(format!("zoom {percent}%"));
    }

    fn set_scroll(&mut self, next_scroll: usize) -> bool {
        if next_scroll == self.scroll {
            return false;
        }
        self.scroll = next_scroll;
        if self.graphic_page.is_some() && self.config.scroll == ScrollStyle::Smooth {
            self.scroll_motion.retarget(next_scroll, self.page_height() / 2);
        } else {
            self.scroll_motion.jump(next_scroll);
        }
        true
    }

    fn render_next_visible_graphic(&mut self) -> bool {
        if self.graphic_page.is_some() {
            return false;
        }

        let content_height = self.page_height();
        let Some(rendered) = self.rendered.as_mut() else {
            return false;
        };
        let visible_end = self.scroll.saturating_add(content_height);
        let Some((graphic_index, source)) =
            rendered.graphics.iter().enumerate().find_map(|(index, graphic)| {
                if graphic.line_index < self.scroll || graphic.line_index >= visible_end {
                    return None;
                }

                match &graphic.content {
                    RenderedGraphicContent::Mermaid { source, png_bytes: None, failed: false } => {
                        Some((index, source.clone()))
                    }
                    _ => None,
                }
            })
        else {
            return false;
        };

        let target_width_px = (f32::from(rendered.graphics[graphic_index].width_cells)
            * self.cell_metrics.width_px)
            .round()
            .max(1.0) as u32;

        match self.mermaid_renderer.render_png_sized(
            &source,
            Some(target_width_px),
            Some(2.0),
            self.config.theme,
        ) {
            Ok(rendered_png) => {
                let image_decoder = ImageDecoder::new();
                let natural_size = image_decoder.dimensions_from_png_bytes(&rendered_png).ok();
                let new_height = natural_size
                    .map(|(width, height)| {
                        scaled_graphic_height(
                            rendered.graphics[graphic_index].width_cells,
                            self.cell_metrics.width_px / self.cell_metrics.height_px,
                            width,
                            height,
                        )
                    })
                    .unwrap_or(rendered.graphics[graphic_index].height_cells);

                resize_graphic_space(rendered, graphic_index, new_height);
                if let RenderedGraphicContent::Mermaid { png_bytes, failed, .. } =
                    &mut rendered.graphics[graphic_index].content
                {
                    *png_bytes = Some(rendered_png);
                    *failed = false;
                }
                if let Some((width, height)) = natural_size {
                    rendered.graphics[graphic_index].natural_width_px = width;
                    rendered.graphics[graphic_index].natural_height_px = height;
                }
                self.warning = None;
            }
            Err(error) => {
                if let RenderedGraphicContent::Mermaid { failed, .. } =
                    &mut rendered.graphics[graphic_index].content
                {
                    *failed = true;
                }
                self.warning = Some(format!("mermaid render failed: {error}"));
            }
        }

        true
    }

    fn draw_line(
        &self,
        stdout: &mut impl Write,
        line: &RenderedLine,
        width: usize,
        tokens: ThemeTokens,
    ) -> Result<()> {
        match &line.kind {
            RenderedLineKind::Blank => {
                queue!(stdout, ResetColor, Print(" ".repeat(width)))?;
            }
            RenderedLineKind::Heading { level } => {
                let text = fit_to_width(&line.display_text, width);
                queue!(
                    stdout,
                    SetForegroundColor(tokens.accent),
                    SetAttribute(Attribute::Bold),
                    Print(match *level {
                        1 => text.to_uppercase(),
                        _ => text,
                    }),
                    SetAttribute(Attribute::Reset),
                    ResetColor
                )?;
            }
            RenderedLineKind::Paragraph | RenderedLineKind::List => {
                let text = fit_to_width(&line.display_text, width);
                queue!(stdout, SetForegroundColor(tokens.foreground), Print(text), ResetColor)?;
            }
            RenderedLineKind::Quote => {
                let text = fit_to_width(&line.display_text, width);
                queue!(stdout, SetForegroundColor(tokens.muted), Print(text), ResetColor)?;
            }
            RenderedLineKind::Callout { kind } => {
                let text = fit_to_width(&line.display_text, width.saturating_sub(2));
                let foreground = if matches!(
                    *kind,
                    crate::core::document::CalloutKind::Warning
                        | crate::core::document::CalloutKind::Caution
                ) {
                    tokens.warning
                } else {
                    tokens.accent
                };
                queue!(
                    stdout,
                    SetForegroundColor(foreground),
                    Print("▎ "),
                    SetBackgroundColor(tokens.subtle_background),
                    SetForegroundColor(tokens.foreground),
                    SetAttribute(Attribute::Bold),
                    Print(text),
                    SetAttribute(Attribute::Reset),
                    ResetColor
                )?;
            }
            RenderedLineKind::Code { language, is_fence_delimiter } => {
                self.draw_code_line(
                    stdout,
                    line,
                    language.as_deref(),
                    *is_fence_delimiter,
                    width,
                    tokens,
                )?;
            }
            RenderedLineKind::Table => {
                let text = fit_to_width(&line.display_text, width);
                queue!(stdout, SetForegroundColor(tokens.accent), Print(text), ResetColor)?;
            }
            RenderedLineKind::Rule => {
                let text = fit_to_width(&line.display_text, width);
                queue!(stdout, SetForegroundColor(tokens.muted), Print(text), ResetColor)?;
            }
            RenderedLineKind::Meta => {
                let text = fit_to_width(&line.display_text, width);
                queue!(
                    stdout,
                    SetForegroundColor(tokens.muted),
                    SetAttribute(Attribute::Italic),
                    Print(text),
                    SetAttribute(Attribute::Reset),
                    ResetColor
                )?;
            }
        }
        Ok(())
    }

    fn draw_code_line(
        &self,
        stdout: &mut impl Write,
        line: &RenderedLine,
        language: Option<&str>,
        is_fence_delimiter: bool,
        width: usize,
        tokens: ThemeTokens,
    ) -> Result<()> {
        let frame = if is_fence_delimiter {
            if line.display_text.is_empty() {
                format!("╰{}╯", "─".repeat(width.saturating_sub(2)))
            } else {
                let label = format!("─ {} ", line.display_text);
                let fill = "─".repeat(width.saturating_sub(label.chars().count() + 2));
                format!("╭{label}{fill}╮")
            }
        } else {
            let inner_width = width.saturating_sub(4).max(1);
            let raw = truncate_visible(&line.plain_text, inner_width);
            let padding = " ".repeat(inner_width.saturating_sub(raw.chars().count()));
            let highlighted = highlight_code_terminal(&raw, language, self.config.theme);
            format!("│ {highlighted}{padding} │")
        };

        queue!(
            stdout,
            SetForegroundColor(tokens.foreground),
            SetBackgroundColor(tokens.code_background),
            Print(frame),
            ResetColor
        )?;
        Ok(())
    }

    fn status_line(&self, width: usize) -> String {
        let name = self.source.path().file_name().map_or_else(
            || self.source.path().display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let message = self
            .warning
            .clone()
            .or_else(|| self.notice.as_ref().map(|(message, _)| message.clone()))
            .or_else(|| {
                self.pending_render
                    .as_ref()
                    .filter(|pending| pending.announce)
                    .map(|_| "rendering…".to_string())
            });
        let status = [Some(name), Some(format!("{}%", self.config.zoom_percent)), message]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        fit_to_width(&format!(" {status}"), width)
    }
}
