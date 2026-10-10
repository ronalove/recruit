// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The libghostty-vt engine (spec §5.2): Ghostty's terminal, with grapheme clusters measured as Claude Code measures
//! them (mode 2027) and its history compressed. Ghostty answers the queries itself, through callbacks; the sniffer
//! still finds what it lets pass (program statuses, notifications, progress). A synchronized update (mode 2026) does
//! not hold the bytes back: the frame last drawn is shown again until it ends, or until its deadline.
//!
//! Owner: dev-terminal.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::style::Color as Paint;
use libghostty_vt::Terminal;
use libghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RenderState, RowIterator};
use libghostty_vt::screen::{CellContentTag, CellWide, Screen, TrackedGridRef};
use libghostty_vt::style::{Palette, RgbColor, StyleColor, Underline};
use libghostty_vt::terminal::{
    ClipboardLocation, ColorScheme, CompressionActivity, CompressionMode, CompressionResult, CursorStyle, Mode, Point,
    PointCoordinate, PointSpace, ScrollViewport, SizeReportSize,
};

use super::sniff::{Sniffed, Sniffer};
use super::{Engine, Line, Modes, MouseTracking, Relay, State};
use crate::canvas::{self, Canvas, Cursor, Style};
use crate::mux::{Rect, Rgb};

/// What the terminal answers when the real one's colors are unknown: a dark theme, as most are.
const DEFAULT_FG: Rgb = (0xd8, 0xd8, 0xd8);
const DEFAULT_BG: Rgb = (0x00, 0x00, 0x00);

/// The size of a cell in pixels, for `CSI 14 t` and `CSI 16 t`: the real one is not known.
const CELL_PIXELS: (u32, u32) = (8, 16);

/// What XTVERSION reports.
const VERSION: &str = concat!("recruit ", env!("CARGO_PKG_VERSION"));

/// How long a synchronized update may hold the frame, from its start, against a program that never ends it: Ghostty's
/// value (`sync_reset_ms`). Shorter, an update that takes its time to come through (a slow machine, a reader late
/// behind a small PTY buffer) shows half made, frame after frame, while its program is fine.
const SYNC_TIMEOUT: Duration = Duration::from_millis(1000);

/// Bytes written between two steps of history compression while a program writes.
const COMPRESS_EVERY: usize = 64 * 1024;
/// How long a pane stays quiet (no output, no scrolling) before its history compression is finished.
const QUIET: Duration = Duration::from_secs(1);
/// Between two steps of that finishing work.
const STEP: Duration = Duration::from_millis(10);

/// The 16 system colors, as crossterm names them.
const NAMED: [Paint; 16] = [
    Paint::Black,
    Paint::DarkRed,
    Paint::DarkGreen,
    Paint::DarkYellow,
    Paint::DarkBlue,
    Paint::DarkMagenta,
    Paint::DarkCyan,
    Paint::Grey,
    Paint::DarkGrey,
    Paint::Red,
    Paint::Green,
    Paint::Yellow,
    Paint::Blue,
    Paint::Magenta,
    Paint::Cyan,
    Paint::White,
];

/// What Ghostty's callbacks leave for the engine, during a write.
#[derive(Default)]
struct Shared {
    replies: Vec<u8>,
    relays: Vec<Relay>,
    /// The real terminal's background, for the color scheme report.
    bg: Option<Rgb>,
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) struct Ghostty {
    term: Terminal<'static, 'static>,
    view: RefCell<View>,
    shared: Arc<Mutex<Shared>>,
    sniffer: Sniffer,
    relays: Vec<Relay>,
    /// The program asked whether program statuses are taken (`OSC 7501 ; ?`) and was told yes: its statuses count
    /// until it clears them, or until a reset. A shell replaying old output does not set a status.
    armed: bool,
    /// While the program holds a synchronized update: until when the frame stays as it was.
    sync_until: Option<Instant>,
    /// The update outlived its deadline: frames go on until the program ends it.
    sync_expired: bool,
    /// When the history compression goes on a step, if it has work left.
    compress_due: Option<Instant>,
    /// The compression's activity token as last seen: it changes when there is something new to compress.
    activity: Option<CompressionActivity>,
    /// Bytes written since the last compression step.
    written: usize,
    /// Lines that left the top of the primary screen's history, for [`Engine::top`]: the line numbers never go back.
    evicted: u64,
    /// The first row of the active area as of the last write, followed as lines scroll away, and its row then. Never
    /// the last: Ghostty keeps a row that holds a tracked reference, so a pane shrinking would push its text into the
    /// history rather than drop the blank rows below it.
    anchor: Option<TrackedGridRef>,
    anchor_y: u32,
}

// SAFETY: a `Ghostty` moves between threads (the PTY's reader feeds it, the screen draws it), one at a time behind the
// pane's mutex, never shared (it is not Sync). Checked in the revisions pinned (Ghostty 22d13172 built by Zig 0.16.0,
// libghostty-rs 8953a740):
// 1. The library built has thread-local variables, all from Zig's standard library, none holding a terminal's state
//    (scripts/ghostty.sh lists them in each libghostty-vt.a and stops on any other):
//    - `Io.Threaded.Thread.current`: set only in the workers of an `Io.Threaded` pool. Each terminal of the C API has
//      its own `Io.Threaded` in `init_single_threaded` (src/terminal/c/terminal.zig: no async, no concurrency, so no
//      worker): it is null on every thread, read call by call, never kept.
//    - `debug.panic_stage`: only in Zig's panic and segfault handlers, which end the process.
//    - `Thread.maybeAttachSignalStack.global.signal_stack`: only at the start of a Zig executable or of a thread Zig
//      spawns; libghostty-vt does neither.
//    - `Thread.LinuxThreadImpl.tls_thread_id` (Linux): the calling thread's own id, cached by the thread that reads
//      it; right on any thread by construction.
//    The C++ `thread_local`s of its sources (highway's profiler, thread pool and vqsort, stb_image) are not in the
//    library built; nor is the `threadlocal` of Ghostty's crash reporter (src/crash), outside it.
// 2. Its process-wide state: the kitty images' generation counter, atomic and meant for terminals on different
//    threads; the sys callbacks (PNG decoding, logs), set once by an embedder, never by recruit.
// 3. Its default allocator is libc's malloc, safe from any thread.
// 4. The crate's only thread-local is the kitty graphics PNG decoder, left out (no `kitty-graphics` feature).
// 5. The callbacks registered here only touch `Shared` through its mutex, and run on the thread calling `vt_write`.
// 6. The C API is not thread-safe in that a terminal must not be used by two threads at once: the pane's mutex sees
//    to it, and render state and iterators live in the same value.
unsafe impl Send for Ghostty {}

/// The terminal as last drawn, and what draws it.
struct View {
    state: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    frame: Frame,
    /// A row read again goes here, its buffers reused, then is compared with the one it replaces.
    spare: Row,
}

/// A frame of the viewport, row by row: a row is read again from Ghostty only when it says the row changed, and
/// nothing at all when nothing did (a pane at rest costs no reading). Shown again as is while a synchronized update
/// holds it.
#[derive(Default)]
struct Frame {
    rows: Vec<Row>,
    cursor: Option<Cursor>,
    /// The default colors the rows were read with (`Colors::fg`, `Colors::bg`): Ghostty marks nothing dirty when the
    /// program changes them, the palette only.
    defaults: (Option<Paint>, Option<Paint>),
    /// [`Engine::generation`]: a fresh one each time a row read again differs from what it was (Ghostty marks the
    /// cursor's rows dirty when it only moves).
    generation: u64,
}

/// A row of the frame.
#[derive(Default, PartialEq)]
struct Row {
    /// The cells' graphemes one after the other.
    text: String,
    cells: Vec<Drawn>,
    links: Vec<String>,
}

#[derive(PartialEq)]
struct Drawn {
    x: u16,
    /// The grapheme, in `Row::text`.
    start: u32,
    end: u32,
    width: u8,
    /// Its link, in `Row::links`, given to the canvas when drawn.
    link: Option<u32>,
    style: Style,
}

/// The colors a frame is drawn with: the palette, and the default colors if the program changed them.
struct Colors {
    palette: Palette,
    standard: Palette,
    fg: Option<Paint>,
    bg: Option<Paint>,
}

impl Ghostty {
    pub(super) fn new(cols: u16, rows: u16, history: usize) -> Result<Ghostty> {
        let mut term = Terminal::new(cols.max(1), rows.max(1)).context("libghostty-vt")?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        term.set_scrollback_max_bytes(None)?.set_scrollback_max_lines(Some(history))?;
        // Grapheme clusters measured whole, as Claude Code does (Bun.stringWidth).
        term.set_mode(Mode::GRAPHEME_CLUSTER, true)?;
        // DECSCUSR cannot ask for it: what the cursor is when the program asked for nothing.
        term.set_default_cursor_style(Some(CursorStyle::BlockHollow))?;
        term.set_default_fg_color(Some(rgb_color(DEFAULT_FG)))?;
        term.set_default_bg_color(Some(rgb_color(DEFAULT_BG)))?;
        let to = Arc::clone(&shared);
        term.on_pty_write(move |_, data| lock(&to).replies.extend_from_slice(data))?;
        term.on_xtversion(|_| Some(VERSION))?;
        let to = Arc::clone(&shared);
        term.on_color_scheme(move |_| {
            let bg = lock(&to).bg.unwrap_or(DEFAULT_BG);
            Some(if luminance(bg) < 0.5 { ColorScheme::Dark } else { ColorScheme::Light })
        })?;
        term.on_size(|term| {
            Some(SizeReportSize {
                rows: term.rows().unwrap_or(1),
                columns: term.cols().unwrap_or(1),
                cell_width: CELL_PIXELS.0,
                cell_height: CELL_PIXELS.1,
            })
        })?;
        let to = Arc::clone(&shared);
        term.on_title_changed(move |term| {
            let title = term.title().unwrap_or_default().to_string();
            lock(&to).relays.push(Relay::Title(title));
        })?;
        let to = Arc::clone(&shared);
        term.on_bell(move |_| lock(&to).relays.push(Relay::Bell))?;
        let to = Arc::clone(&shared);
        term.on_clipboard_write(move |_, write| {
            if write.location() == ClipboardLocation::Standard
                && let Some(text) = write.contents().find(|content| content.mime.starts_with("text/plain"))
            {
                lock(&to).relays.push(Relay::Clipboard(String::from_utf8_lossy(text.data).into_owned()));
            }
            Ok(())
        })?;
        let view = View {
            state: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            frame: Frame { generation: super::fresh_generation(), ..Frame::default() },
            spare: Row::default(),
        };
        Ok(Ghostty {
            term,
            view: RefCell::new(view),
            shared,
            sniffer: Sniffer::default(),
            relays: Vec::new(),
            armed: false,
            sync_until: None,
            sync_expired: false,
            compress_due: None,
            activity: None,
            written: 0,
            evicted: 0,
            anchor: None,
            anchor_y: 0,
        })
    }

    /// What the callbacks left: replies to `replies`, the rest to the relays, in the order they came.
    fn collect(&mut self, replies: &mut Vec<u8>) {
        let mut shared = lock(&self.shared);
        replies.append(&mut shared.replies);
        self.relays.append(&mut shared.relays);
    }

    fn handle(&mut self, found: Sniffed, replies: &mut Vec<u8>) {
        match found {
            // Ghostty answers these itself (callbacks).
            Sniffed::Version | Sniffed::ColorScheme => {}
            Sniffed::Notify { title, body } => self.relays.push(Relay::Notify { title, body }),
            Sniffed::Progress(progress) => self.relays.push(Relay::Progress(progress)),
            // Program statuses are taken: Claude Code then says when it works, waits and is done.
            Sniffed::StatusQuery => {
                replies.extend(b"\x1b]7501;?\x1b\\");
                self.armed = true;
            }
            Sniffed::Status(status) if self.armed => {
                self.armed = status.state != State::Clear;
                self.relays.push(Relay::Status(status));
            }
            Sniffed::Status(_) => {}
            Sniffed::Reset => self.armed = false,
            Sniffed::SyncEnd => self.sync_ended(),
        }
    }

    /// The program ended a synchronized update, maybe in the middle of a read: the frame it made is shown whole, and
    /// an update that begins further in the same read gets a deadline of its own ([`Ghostty::written`] sees the mode
    /// at the end of a read only). Otherwise, when the reads come late (a busy machine), each one holds the end of an
    /// update and the start of the next: the next would keep the old deadline, or none after an expiry, and show half
    /// made; and the frames between would never show.
    fn sync_ended(&mut self) {
        if self.term.mode(Mode::SYNC_OUTPUT).unwrap_or(false) {
            return;
        }
        self.sync_until = None;
        self.sync_expired = false;
        let mut view = self.view.borrow_mut();
        self.capture(&mut view);
    }

    fn syncing(&self) -> bool {
        self.sync_until.is_some()
    }

    /// After a write: the synchronized update's deadline, the line numbers, the history compression.
    fn written(&mut self, bytes: usize, now: Instant) {
        if self.term.mode(Mode::SYNC_OUTPUT).unwrap_or(false) {
            if self.sync_until.is_none() && !self.sync_expired {
                self.sync_until = Some(now + SYNC_TIMEOUT);
            }
        } else {
            self.sync_until = None;
            self.sync_expired = false;
        }
        self.follow();
        self.written += bytes;
        if self.written >= COMPRESS_EVERY {
            self.written = 0;
            let _ = self.term.compress(CompressionMode::Incremental);
        }
        self.busy(now);
    }

    /// Something happened that may leave history to compress: the rest is done once the pane is quiet.
    fn busy(&mut self, now: Instant) {
        let activity = self.term.compression_activity().ok();
        if activity != self.activity {
            self.activity = activity;
            self.compress_due = Some(now + QUIET);
        }
    }

    /// Counts the lines that left the top of the primary screen's history since the last write (history full, CSI 3J,
    /// RIS), by where the anchor went, and anchors again on the first row of the active area.
    fn follow(&mut self) {
        if self.term.active_screen().unwrap_or_default() != Screen::Primary {
            return;
        }
        if let Some(anchor) = &self.anchor {
            match anchor.point(PointSpace::Screen) {
                Ok(Some(point)) => self.evicted += u64::from(self.anchor_y.saturating_sub(point.y)),
                // The anchor itself is gone: everything above it with it, at least.
                _ => self.evicted += u64::from(self.anchor_y) + 1,
            }
        }
        let point = Point::Active(PointCoordinate { x: 0, y: 0 });
        match self.anchor.as_mut() {
            Some(anchor) => {
                let _ = anchor.set(&mut self.term, point);
            }
            None => self.anchor = self.term.track_grid_ref(point).ok(),
        }
        self.anchor_y = self
            .anchor
            .as_ref()
            .and_then(|anchor| anchor.point(PointSpace::Screen).ok().flatten())
            .map_or(0, |point| point.y);
    }

    fn colors(&self) -> Colors {
        let (fg, bg) = self.defaults();
        Colors {
            palette: self.term.color_palette().unwrap_or(Palette([RgbColor::default(); 256])),
            standard: self.term.default_color_palette().unwrap_or(Palette([RgbColor::default(); 256])),
            fg,
            bg,
        }
    }

    /// The default foreground and background, if the program changed them.
    fn defaults(&self) -> (Option<Paint>, Option<Paint>) {
        let changed = |now: Option<RgbColor>, default: Option<RgbColor>| {
            now.filter(|now| Some(*now) != default).map(|c| Paint::Rgb { r: c.r, g: c.g, b: c.b })
        };
        (
            changed(self.term.fg_color().ok().flatten(), self.term.default_fg_color().ok().flatten()),
            changed(self.term.bg_color().ok().flatten(), self.term.default_bg_color().ok().flatten()),
        )
    }

    /// The viewport as it is now, into `frame`: the cursor every time, the rows Ghostty says changed since the last
    /// time (all of them after a resize, a scroll, a change of colors).
    fn capture(&self, view: &mut View) {
        let View { state, rows, cells, frame, spare } = view;
        let Ok(snapshot) = state.update(&self.term) else { return };
        frame.cursor = None;
        if snapshot.cursor_visible().unwrap_or(false)
            && let Ok(Some(at)) = snapshot.cursor_viewport()
        {
            let shape = match snapshot.cursor_visual_style() {
                Ok(CursorVisualStyle::Block) => canvas::CursorShape::Block,
                Ok(CursorVisualStyle::Underline) => canvas::CursorShape::Underline,
                Ok(CursorVisualStyle::Bar) => canvas::CursorShape::Bar,
                _ => canvas::CursorShape::Default,
            };
            let x = if at.at_wide_tail { at.x.saturating_sub(1) } else { at.x };
            frame.cursor = Some(Cursor {
                x: usize::from(x),
                y: usize::from(at.y),
                shape,
                blink: snapshot.cursor_blinking().unwrap_or(false),
            });
        }
        let height = usize::from(snapshot.rows().unwrap_or(0));
        let dirty = snapshot.dirty().unwrap_or(Dirty::Full);
        let defaults = self.defaults();
        let full = dirty == Dirty::Full || frame.rows.len() != height || frame.defaults != defaults;
        if dirty == Dirty::Clean && !full {
            return;
        }
        let mut changed = frame.rows.len() != height;
        frame.rows.resize_with(height, Row::default);
        let colors = self.colors();
        // A cell without styling of its own takes the default colors, the program's if it changed them.
        let plain = Style { fg: colors.fg, bg: colors.bg, ..Style::PLAIN };
        let Ok(mut row_it) = rows.update(&snapshot) else { return };
        // A link's URI read into this buffer; the row's last link is reused while the next cells carry the same
        // (the C API gives no link id to compare instead).
        let mut uri = Vec::new();
        // Ghostty writes a cell's grapheme from the start of the buffer: one for the cell, then copied.
        let mut grapheme = String::new();
        let mut y = 0u16;
        // Every row read: Ghostty may forget what was dirty.
        let mut complete = true;
        while let Some(row) = row_it.next() {
            let Some(out) = frame.rows.get_mut(usize::from(y)) else { break };
            if full || row.dirty().unwrap_or(true) {
                std::mem::swap(out, spare);
                out.text.clear();
                out.cells.clear();
                out.links.clear();
                // Whether some cell of the row may have a style or a link (never wrong when false): if not, no cell
                // is asked.
                let flags = row.raw_row().ok();
                let styled = flags.is_none_or(|flags| flags.is_styled().unwrap_or(true));
                let linked = flags.is_none_or(|flags| flags.has_hyperlink().unwrap_or(true));
                let Ok(mut cell_it) = cells.update(row) else {
                    // The row kept as it was, and read again next time.
                    std::mem::swap(out, spare);
                    complete = false;
                    break;
                };
                let mut x = 0u16;
                while let Some(cell) = cell_it.next() {
                    let Ok(raw) = cell.raw_cell() else {
                        x += 1;
                        continue;
                    };
                    let wide = raw.wide().unwrap_or(CellWide::Narrow);
                    if wide == CellWide::SpacerTail {
                        x += 1;
                        continue;
                    }
                    // Most cells are blank and plain: neither their style nor their text is asked for.
                    let style = if styled && raw.has_styling().unwrap_or(true) { cell.style().ok() } else { None };
                    let start = out.text.len() as u32;
                    let has_text = raw.has_text().unwrap_or(true);
                    let shown = wide != CellWide::SpacerHead && !style.is_some_and(|style| style.invisible) && has_text;
                    if shown && cell.graphemes_utf8(&mut grapheme).is_ok() && !grapheme.is_empty() {
                        out.text.push_str(&grapheme);
                    } else {
                        out.text.push(' ');
                    }
                    let link = (linked && raw.has_hyperlink().unwrap_or(false)).then(|| {
                        if uri.is_empty() {
                            uri.resize(4096, 0);
                        }
                        let grid = self.term.grid_ref(Point::Viewport(PointCoordinate { x, y: u32::from(y) })).ok()?;
                        let len = grid.hyperlink_uri(&mut uri).ok()?;
                        let read = &uri[..len];
                        if out.links.last().is_none_or(|last| last.as_bytes() != read) {
                            out.links.push(String::from_utf8_lossy(read).into_owned());
                        }
                        Some(out.links.len() as u32 - 1)
                    });
                    out.cells.push(Drawn {
                        x,
                        start,
                        end: out.text.len() as u32,
                        width: if wide == CellWide::Wide { 2 } else { 1 },
                        link: link.flatten(),
                        style: match style {
                            Some(style) => paint_style(&style, &colors),
                            // A blank cell may still have a background of its own, without a style: one erased with
                            // a background color set (`style()` says nothing of it).
                            None if !has_text => match raw.content_tag() {
                                Ok(CellContentTag::BgColorPalette) => Style {
                                    bg: raw.bg_color_palette().ok().map(|index| palette_paint(index.0, &colors)),
                                    ..plain
                                },
                                Ok(CellContentTag::BgColorRgb) => Style {
                                    bg: raw.bg_color_rgb().ok().map(|c| Paint::Rgb { r: c.r, g: c.g, b: c.b }),
                                    ..plain
                                },
                                _ => plain,
                            },
                            None => plain,
                        },
                    });
                    x += 1;
                }
                let _ = row.set_dirty(false);
                changed |= out != spare;
            }
            y += 1;
        }
        if changed {
            frame.generation = super::fresh_generation();
        }
        if complete {
            frame.defaults = defaults;
            let _ = snapshot.set_dirty(Dirty::Clean);
        }
    }
}

impl Frame {
    fn paint(&self, canvas: &mut Canvas, area: Rect) {
        for (y, row) in self.rows.iter().enumerate().take(area.height) {
            for cell in &row.cells {
                let x = usize::from(cell.x);
                if x >= area.width {
                    break;
                }
                let mut style = cell.style;
                style.link = cell.link.map(|link| canvas.link(&row.links[link as usize]));
                // Half of a wide character cut by the pane's edge: blank.
                let (text, width) = if cell.width == 2 && x + 1 >= area.width {
                    (" ", 1)
                } else {
                    (&row.text[cell.start as usize..cell.end as usize], usize::from(cell.width))
                };
                canvas.put_cell(area.x + x, area.y + y, text, width, style);
            }
        }
    }
}

/// A cell's style for the canvas: its colors as the real terminal names them (`None` for its own default, so that
/// its theme stays), unless the program changed them.
fn paint_style(style: &libghostty_vt::style::Style, colors: &Colors) -> Style {
    let paint = |color: StyleColor, default: Option<Paint>| match color {
        StyleColor::None => default,
        StyleColor::Rgb(c) => Some(Paint::Rgb { r: c.r, g: c.g, b: c.b }),
        StyleColor::Palette(index) => Some(palette_paint(index.0, colors)),
    };
    Style {
        fg: paint(style.fg_color, colors.fg),
        bg: paint(style.bg_color, colors.bg),
        bold: style.bold,
        dim: style.faint,
        reverse: style.inverse,
        underline: style.underline != Underline::None,
        italic: style.italic,
        strike: style.strikethrough,
        link: None,
    }
}

/// A color of the palette for the canvas: by its index, unless the program changed it.
fn palette_paint(index: u8, colors: &Colors) -> Paint {
    let i = usize::from(index);
    let (now, standard) = (colors.palette.0[i], colors.standard.0[i]);
    if now != standard {
        Paint::Rgb { r: now.r, g: now.g, b: now.b }
    } else if i < NAMED.len() {
        NAMED[i]
    } else {
        Paint::AnsiValue(index)
    }
}

impl Engine for Ghostty {
    fn feed(&mut self, bytes: &[u8], replies: &mut Vec<u8>) {
        let now = Instant::now();
        if self.deadline().is_some_and(|at| at <= now) {
            self.expire(now, replies);
        }
        let mut rest = bytes;
        while !rest.is_empty() {
            let (read, found) = self.sniffer.scan(rest);
            self.term.vt_write(&rest[..read]);
            self.collect(replies);
            if let Some(found) = found {
                self.handle(found, replies);
            }
            rest = &rest[read..];
        }
        self.written(bytes.len(), now);
    }

    fn deadline(&self) -> Option<Instant> {
        [self.sync_until, self.compress_due].into_iter().flatten().min()
    }

    fn expire(&mut self, now: Instant, _replies: &mut Vec<u8>) -> bool {
        let mut changed = false;
        if self.sync_until.is_some_and(|at| at <= now) {
            self.sync_until = None;
            self.sync_expired = true;
            changed = true;
        }
        if self.compress_due.is_some_and(|at| at <= now) {
            self.compress_due = match self.term.compress(CompressionMode::Incremental) {
                Ok(CompressionResult::Pending) => Some(now + STEP),
                _ => {
                    self.activity = self.term.compression_activity().ok();
                    None
                }
            };
        }
        changed
    }

    fn relays(&mut self, out: &mut Vec<Relay>) {
        out.append(&mut self.relays);
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        // Ghostty ends a synchronized update on a resize, so that it shows at once: not an expiry (`sync_expired` stays
        // as it was), the program starts a new one with its next BSU.
        let _ = self.term.resize(cols.max(1), rows.max(1), CELL_PIXELS.0, CELL_PIXELS.1);
        self.sync_until = None;
        self.follow();
    }

    fn set_colors(&mut self, fg: Option<Rgb>, bg: Option<Rgb>) {
        let _ = self.term.set_default_fg_color(Some(rgb_color(fg.unwrap_or(DEFAULT_FG))));
        let _ = self.term.set_default_bg_color(Some(rgb_color(bg.unwrap_or(DEFAULT_BG))));
        lock(&self.shared).bg = bg;
    }

    fn modes(&self) -> Modes {
        let on = |mode| self.term.mode(mode).unwrap_or(false);
        let mouse = if on(Mode::ANY_MOUSE) {
            Some(MouseTracking::Motion)
        } else if on(Mode::BUTTON_MOUSE) {
            Some(MouseTracking::Drag)
        // X10 (mode 9) reports presses only; it comes as Click, which sends releases too: no program of a pane asks
        // for it today.
        } else if on(Mode::NORMAL_MOUSE) || on(Mode::X10_MOUSE) {
            Some(MouseTracking::Click)
        } else {
            None
        };
        Modes {
            app_cursor: on(Mode::DECCKM),
            app_keypad: on(Mode::KEYPAD_KEYS),
            bracketed_paste: on(Mode::BRACKETED_PASTE),
            focus: on(Mode::FOCUS_EVENT),
            mouse,
            mouse_sgr: on(Mode::SGR_MOUSE),
            alternate_scroll: on(Mode::ALT_SCROLL),
            alt_screen: self.term.active_screen().unwrap_or_default() == Screen::Alternate,
            kitty: self.term.kitty_keyboard_flags().map_or(0, |flags| flags.bits()),
        }
    }

    fn draw(&self, canvas: &mut Canvas, area: Rect) {
        let mut view = self.view.borrow_mut();
        // While the program holds a synchronized update, the frame stays as it was.
        if !self.syncing() {
            self.capture(&mut view);
        }
        view.frame.paint(canvas, area);
    }

    fn generation(&self) -> Option<u64> {
        let mut view = self.view.borrow_mut();
        if !self.syncing() {
            self.capture(&mut view);
        }
        Some(view.frame.generation)
    }

    fn cursor(&self) -> Option<Cursor> {
        self.view.borrow().frame.cursor
    }

    fn history(&self) -> usize {
        self.term.scrollback_rows().unwrap_or(0)
    }

    fn scroll(&mut self, lines: isize) {
        let scroll =
            if lines == isize::MIN { ScrollViewport::Bottom } else { ScrollViewport::Delta(lines.saturating_neg()) };
        self.term.scroll_viewport(scroll);
        // Pages read from the history were decompressed: compressed again once the pane is quiet.
        self.busy(Instant::now());
    }

    fn scrolled(&self) -> usize {
        self.term.scrollbar().map_or(0, |bar| bar.total.saturating_sub(bar.offset + bar.len) as usize)
    }

    fn top(&self) -> u64 {
        let first = Point::Viewport(PointCoordinate { x: 0, y: 0 });
        let y = self
            .term
            .grid_ref(first)
            .ok()
            .and_then(|grid| self.term.point_from_grid_ref(&grid, PointSpace::Screen).ok().flatten())
            .map_or(0, |point| u64::from(point.y));
        y + self.oldest()
    }

    fn oldest(&self) -> u64 {
        match self.term.active_screen().unwrap_or_default() {
            Screen::Primary => self.evicted,
            Screen::Alternate => 0,
        }
    }

    /// Read cell by cell where it is kept: only the page it is on is decompressed, if it was compressed.
    fn line(&self, line: u64) -> Option<Line> {
        let y = u32::try_from(line.checked_sub(self.oldest())?).ok()?;
        if u64::from(y) >= self.term.total_rows().ok()? as u64 {
            return None;
        }
        let mut out = Line::default();
        // A grapheme of any length: the buffer grows when Ghostty says how many code points it needs.
        let mut chars = vec!['\0'; 16];
        let mut grapheme = String::new();
        for x in 0..self.term.cols().ok()? {
            let grid = self.term.grid_ref(Point::Screen(PointCoordinate { x, y })).ok()?;
            if x == 0 {
                out.wrapped = grid.row().and_then(|row| row.is_wrapped()).unwrap_or(false);
            }
            match grid.cell().and_then(|cell| cell.wide()) {
                Ok(CellWide::SpacerTail) => out.push(""),
                Ok(CellWide::SpacerHead) => out.push(" "),
                _ => {
                    let count = match grid.graphemes(&mut chars) {
                        Ok(count) => count,
                        Err(libghostty_vt::Error::OutOfSpace { required }) => {
                            chars.resize(required.max(chars.len() * 4), '\0');
                            grid.graphemes(&mut chars).unwrap_or(0)
                        }
                        Err(_) => 0,
                    }
                    .min(chars.len());
                    grapheme.clear();
                    grapheme.extend(&chars[..count]);
                    out.push(if grapheme.is_empty() { " " } else { &grapheme });
                }
            }
        }
        Some(out)
    }
}

fn rgb_color((r, g, b): Rgb) -> RgbColor {
    RgbColor { r, g, b }
}

/// Relative luminance, 0 to 1.
fn luminance((r, g, b): Rgb) -> f64 {
    (0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Ghostty {
        Ghostty::new(20, 5, 100).unwrap()
    }

    /// What the engine answers to `bytes`.
    fn answer(engine: &mut Ghostty, bytes: &[u8]) -> String {
        let mut replies = Vec::new();
        engine.feed(bytes, &mut replies);
        String::from_utf8(replies).unwrap()
    }

    fn rows(engine: &Ghostty) -> Vec<String> {
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        (0..5).map(|y| canvas.row(y)).collect()
    }

    fn relays(engine: &mut Ghostty) -> Vec<Relay> {
        let mut relays = Vec::new();
        engine.relays(&mut relays);
        relays
    }

    #[test]
    fn queries_are_answered() {
        let mut engine = engine();
        assert_eq!(answer(&mut engine, b"\x1b[c"), "\x1b[?62;22c");
        assert_eq!(answer(&mut engine, b"\x1b[5n"), "\x1b[0n");
        assert_eq!(answer(&mut engine, b"\x1b[3;7H\x1b[6n"), "\x1b[3;7R");
        // Synchronized updates known, graphemes measured whole (set by the engine).
        assert_eq!(answer(&mut engine, b"\x1b[?2026$p"), "\x1b[?2026;2$y");
        assert_eq!(answer(&mut engine, b"\x1b[?2027$p"), "\x1b[?2027;1$y");
        // The kitty keyboard protocol: none, then pushed, then popped.
        assert_eq!(answer(&mut engine, b"\x1b[?u"), "\x1b[?0u");
        assert_eq!(answer(&mut engine, b"\x1b[>1u\x1b[?u"), "\x1b[?1u");
        assert_eq!(engine.modes().kitty, 1);
        answer(&mut engine, b"\x1b[<u");
        assert_eq!(engine.modes().kitty, 0);
        // Claude Code's probe, together: XTVERSION, kitty, program status, then DA1 last, which ends it.
        let version = format!("\x1bP>|recruit {}\x1b\\", env!("CARGO_PKG_VERSION"));
        assert_eq!(
            answer(&mut engine, b"\x1b[>0q\x1b[?u\x1b]7501;?\x1b\\\x1b[c"),
            format!("{version}\x1b[?0u\x1b]7501;?\x1b\\\x1b[?62;22c")
        );
        assert_eq!(answer(&mut engine, b"\x1b[18t"), "\x1b[8;5;20t");
    }

    #[test]
    fn colors_are_the_real_terminals() {
        let mut engine = engine();
        assert!(answer(&mut engine, b"\x1b]11;?\x1b\\").contains("11;rgb:0000/0000/0000"));
        assert_eq!(answer(&mut engine, b"\x1b[?996n"), "\x1b[?997;1n");
        engine.set_colors(Some((0x10, 0x20, 0x30)), Some((0xfa, 0xfb, 0xfc)));
        assert!(answer(&mut engine, b"\x1b]10;?\x1b\\").contains("10;rgb:1010/2020/3030"));
        assert!(answer(&mut engine, b"\x1b]11;?\x1b\\").contains("11;rgb:fafa/fbfb/fcfc"));
        assert_eq!(answer(&mut engine, b"\x1b[?996n"), "\x1b[?997;2n");
        // What the program sets wins, and is drawn.
        answer(&mut engine, b"\x1b]11;rgb:01/02/03\x1b\\ ");
        assert!(answer(&mut engine, b"\x1b]11;?\x1b\\").contains("11;rgb:0101/0202/0303"));
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        assert_eq!(canvas.style(0, 0).bg, Some(Paint::Rgb { r: 1, g: 2, b: 3 }));
    }

    #[test]
    fn relayed() {
        let mut engine = engine();
        answer(&mut engine, b"\x1b]52;c;aGVsbG8=\x07\x1b]2;Claude\x07\x07\x1b]9;Done\x07\x1b]9;4;1;50\x07");
        assert_eq!(
            relays(&mut engine),
            [
                Relay::Clipboard("hello".into()),
                Relay::Title("Claude".into()),
                Relay::Bell,
                Relay::Notify { title: None, body: "Done".into() },
                Relay::Progress(super::super::Progress::Set(50)),
            ]
        );
        assert!(relays(&mut engine).is_empty(), "given once");
    }

    #[test]
    fn program_statuses_count_once_asked() {
        let mut engine = engine();
        let working = b"\x1b]7501;state=working:app=claude-code\x1b\\";
        // Not asked: a shell replaying old output sets nothing.
        answer(&mut engine, working);
        assert!(relays(&mut engine).is_empty());
        answer(&mut engine, b"\x1b]7501;?\x1b\\");
        answer(&mut engine, working);
        assert!(matches!(&relays(&mut engine)[..], [Relay::Status(s)] if s.state == State::Working));
        // Cleared (member.rs writes it when its Claude stops): then nothing until asked again.
        answer(&mut engine, b"\x1b]7501;state=clear\x07");
        answer(&mut engine, working);
        assert!(matches!(&relays(&mut engine)[..], [Relay::Status(s)] if s.state == State::Clear));
        // A reset too.
        answer(&mut engine, b"\x1b]7501;?\x1b\\\x1bc");
        answer(&mut engine, working);
        assert!(relays(&mut engine).is_empty());
    }

    #[test]
    fn an_update_may_take_its_time() {
        // A slow machine: the update comes through in pieces, well past 150 ms; held all the same, up to a second.
        let mut engine = engine();
        answer(&mut engine, b"\x1b[Hold");
        rows(&engine);
        let start = Instant::now();
        answer(&mut engine, b"\x1b[?2026h\x1b[Hne");
        let mut replies = Vec::new();
        assert!(!engine.expire(start + Duration::from_millis(500), &mut replies));
        assert_eq!(rows(&engine)[0], "old");
        answer(&mut engine, b"w\x1b[?2026l");
        assert_eq!(rows(&engine)[0], "new");
        // A program that never ends one: shown after a second.
        answer(&mut engine, b"\x1b[?2026h\x1b[Hstuck");
        assert_eq!(rows(&engine)[0], "new");
        assert!(engine.expire(Instant::now() + SYNC_TIMEOUT, &mut replies));
        assert_eq!(rows(&engine)[0], "stuck");
    }

    #[test]
    fn an_update_that_ends_and_begins_in_one_read() {
        // Output read late: the end of one update and the start of the next come in one read.
        let mut engine = engine();
        answer(&mut engine, b"\x1b[?2026h\x1b[2J\x1b[Hone");
        let first = engine.sync_until.expect("a deadline");
        std::thread::sleep(Duration::from_millis(2));
        answer(&mut engine, b"\x1b[?2026l\x1b[?2026h\x1b[2J\x1b[Htwo");
        // The next update has a deadline of its own, and holds the frame: the first update, whole.
        assert!(engine.sync_until.is_some_and(|next| next > first), "the first update's deadline kept");
        assert_eq!(rows(&engine)[0], "one");
        // Past its deadline, frames go on; then it ends, and the next begins, in one read: held all the same.
        let mut replies = Vec::new();
        assert!(engine.expire(engine.sync_until.unwrap(), &mut replies));
        assert_eq!(rows(&engine)[0], "two");
        answer(&mut engine, b"\x1b[?2026l\x1b[?2026h\x1b[2J\x1b[Hthr");
        assert!(engine.sync_until.is_some(), "the next update not held");
        assert_eq!(rows(&engine)[0], "two", "half of the next update shown");
        // Its end shows it whole.
        answer(&mut engine, b"ee\x1b[?2026l");
        assert_eq!(rows(&engine)[0], "three");
    }

    #[test]
    fn every_whole_frame_of_a_late_read_shows() {
        // A read that holds several updates, the last one unfinished: the last whole frame shows, not the first one,
        // nor the unfinished one.
        let mut engine = engine();
        rows(&engine);
        answer(&mut engine, b"\x1b[?2026h\x1b[Ha\x1b[?2026l\x1b[?2026h\x1b[Hb\x1b[?2026l\x1b[?2026h\x1b[Hc");
        assert!(engine.sync_until.is_some());
        assert_eq!(rows(&engine)[0], "b");
        // A mode 2026 reset among others ends it too.
        answer(&mut engine, b"\x1b[?1000;2026l");
        assert_eq!(rows(&engine)[0], "c");
        assert!(engine.sync_until.is_none());
    }

    #[test]
    fn synchronized_updates_hold_the_frame() {
        let mut engine = engine();
        answer(&mut engine, b"before");
        assert_eq!(rows(&engine)[0], "before");
        // Held, write after write: the frame stays, and the queries are answered at once.
        assert_eq!(answer(&mut engine, b"\x1b[?2026h\x1b[2J\x1b[Hafter\x1b[c"), "\x1b[?62;22c");
        assert_eq!(answer(&mut engine, b" more"), "");
        assert_eq!(rows(&engine)[0], "before");
        let deadline = engine.deadline().expect("a deadline");
        assert!(deadline <= Instant::now() + SYNC_TIMEOUT);
        // Its end shows it all.
        answer(&mut engine, b"\x1b[?2026l");
        assert_eq!(rows(&engine)[0], "after more");
        // Never ended: shown at the deadline, not before.
        answer(&mut engine, b"\x1b[?2026h\x1b[2J\x1b[Hlate");
        let deadline = engine.sync_until.unwrap();
        let mut replies = Vec::new();
        assert!(!engine.expire(deadline - Duration::from_millis(1), &mut replies));
        assert_eq!(rows(&engine)[0], "after more");
        assert!(engine.expire(deadline, &mut replies));
        assert_eq!(rows(&engine)[0], "late");
        // Still in the update past its deadline: frames go on.
        answer(&mut engine, b" too");
        assert_eq!(rows(&engine)[0], "late too");
    }

    #[test]
    fn cells_are_drawn_as_claude_measures_them() {
        let mut engine = engine();
        // Each takes the columns Bun.stringWidth gives it: 👍🏽, ❤️ and 🇫🇷 two, e + accent one.
        answer(&mut engine, "a界b e\u{301} 👍🏽 ❤\u{fe0f}🇫🇷|".as_bytes());
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        assert_eq!(canvas.row(0), "a界b e\u{301} 👍🏽 ❤\u{fe0f}🇫🇷|");
        assert_eq!(engine.cursor().unwrap().x, 15);
        answer(&mut engine, b"\r\n\x1b[1;31mR\x1b[0m\x1b[38;2;1;2;3mT\x1b[0m\x1b[38;5;200mI\x1b[0mx");
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        let at = |x| canvas.style(x, 1);
        assert_eq!((at(0).fg, at(0).bold), (Some(Paint::DarkRed), true));
        assert_eq!(at(1).fg, Some(Paint::Rgb { r: 1, g: 2, b: 3 }));
        assert_eq!(at(2).fg, Some(Paint::AnsiValue(200)));
        assert_eq!(at(3).fg, None, "the real terminal's own color");
        // Cells erased with a background color set: that background, without text or style of their own.
        answer(&mut engine, b"\r\n\x1b[41m\x1b[K\x1b[0m\r\n\x1b[48;2;4;5;6m\x1b[2X\x1b[0m");
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        assert_eq!((canvas.style(0, 2).bg, canvas.style(19, 2).bg), (Some(Paint::DarkRed), Some(Paint::DarkRed)));
        assert_eq!((canvas.style(1, 3).bg, canvas.style(2, 3).bg), (Some(Paint::Rgb { r: 4, g: 5, b: 6 }), None));
        // A wide character cut by the pane's edge is not drawn half.
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 2, height: 1 });
        assert_eq!(canvas.row(0), "a");
    }

    #[test]
    fn links_and_hidden_text() {
        let mut engine = engine();
        answer(&mut engine, b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\ \x1b[8msecret");
        rows(&engine);
        assert_eq!(engine.view.borrow().frame.rows[0].links, ["https://example.com"], "one link for its four cells");
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        assert_eq!(canvas.link_at(0, 0), Some("https://example.com"));
        assert_eq!(canvas.link_at(5, 0), None);
        assert_eq!(canvas.row(0), "link");
    }

    #[test]
    fn the_cursor_as_the_program_left_it() {
        let mut engine = engine();
        answer(&mut engine, b"\x1b[2;4H");
        rows(&engine);
        let cursor = engine.cursor().unwrap();
        assert_eq!((cursor.x, cursor.y, cursor.shape), (3, 1, canvas::CursorShape::Default));
        answer(&mut engine, b"\x1b[6 q");
        rows(&engine);
        assert_eq!(engine.cursor().unwrap().shape, canvas::CursorShape::Bar);
        answer(&mut engine, b"\x1b[0 q\x1b[?25l");
        rows(&engine);
        assert_eq!(engine.cursor(), None);
        answer(&mut engine, b"\x1b[?25h");
        rows(&engine);
        assert_eq!(engine.cursor().unwrap().shape, canvas::CursorShape::Default);
    }

    #[test]
    fn the_view_stays_on_its_lines() {
        let mut engine = engine();
        for i in 0..10 {
            answer(&mut engine, format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(engine.history(), 6);
        engine.scroll(2);
        assert_eq!(engine.scrolled(), 2);
        assert_eq!(rows(&engine)[0], "line 4");
        // The program writes: the view keeps showing the same lines.
        answer(&mut engine, b"line 10\r\n");
        assert_eq!((engine.scrolled(), rows(&engine)[0].as_str()), (3, "line 4"));
        engine.scroll(100);
        assert_eq!(engine.scrolled(), engine.history());
        engine.scroll(-1);
        assert_eq!(engine.scrolled(), engine.history() - 1);
        engine.scroll(isize::MIN);
        assert_eq!(engine.scrolled(), 0);
        assert_eq!(rows(&engine)[3], "line 10");
    }

    #[test]
    fn lines_keep_their_numbers() {
        let mut engine = Ghostty::new(20, 5, 10).unwrap();
        let text = |line: Option<Line>| line.map(|line| line.text.trim_end().to_string());
        for i in 0..8 {
            answer(&mut engine, format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(engine.oldest(), 0);
        assert_eq!(text(engine.line(0)).as_deref(), Some("line 0"));
        assert_eq!(engine.top(), 4, "lines 0 to 3 above the screen");
        let wide = engine.line(2).unwrap();
        assert_eq!((wide.width(), wide.cell(0), wide.wrapped), (20, "l", false));
        // The program writes on: Ghostty drops the history by whole pages (thousands of lines here, past the limit of
        // 10), and every line keeps its number.
        let mut written = 8;
        while engine.oldest() == 0 {
            answer(&mut engine, format!("line {written}\r\n").as_bytes());
            written += 1;
            assert!(written < 50_000, "the history never turned");
        }
        let oldest = engine.oldest();
        assert_eq!(engine.line(oldest - 1), None);
        assert_eq!(text(engine.line(oldest)), Some(format!("line {oldest}")));
        assert_eq!(text(engine.line(written - 1)), Some(format!("line {}", written - 1)));
        // CSI 3J empties the history: the oldest line jumps forward, never back.
        answer(&mut engine, b"\x1b[3J");
        assert!(engine.oldest() > oldest);
        assert_eq!(text(engine.line(engine.top())), Some(format!("line {}", written - 4)));
        // Not written yet: none.
        assert_eq!(engine.line(engine.top() + 10), None);
        // Soft wraps are said.
        answer(&mut engine, &[b'x'; 25]);
        // 20 on the next-to-last row, which goes on; the last 5 below it.
        let first = engine.top() + 3;
        assert_eq!((engine.line(first).unwrap().text, engine.line(first).unwrap().wrapped), ("x".repeat(20), true));
        assert!(!engine.line(first + 1).unwrap().wrapped);
        // The alternate screen counts its own lines, from 0.
        answer(&mut engine, b"\x1b[?1049h\x1b[Halt");
        assert_eq!((engine.oldest(), engine.top()), (0, 0));
        assert_eq!(text(engine.line(0)).as_deref(), Some("alt"));
    }

    #[test]
    fn modes_for_the_input() {
        let mut engine = engine();
        let start = engine.modes();
        assert_eq!(start, Modes { alternate_scroll: start.alternate_scroll, ..Modes::default() });
        answer(&mut engine, b"\x1b[?1h\x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1006h\x1b[?1049h\x1b=\x1b[>11u");
        assert_eq!(
            engine.modes(),
            Modes {
                app_cursor: true,
                app_keypad: true,
                bracketed_paste: true,
                focus: true,
                mouse: Some(MouseTracking::Drag),
                mouse_sgr: true,
                alternate_scroll: start.alternate_scroll,
                alt_screen: true,
                kitty: 11,
            }
        );
    }

    #[test]
    fn history_compressed_once_quiet_then_nothing() {
        let mut engine = Ghostty::new(80, 24, 100_000).unwrap();
        let line = format!("{}\r\n", "compressible text ".repeat(4));
        let chunk = line.repeat(1000);
        let mut replies = Vec::new();
        for _ in 0..30 {
            engine.feed(chunk.as_bytes(), &mut replies);
        }
        let quiet = engine.deadline().expect("compression to finish");
        assert!(quiet > Instant::now() + QUIET / 2);
        // Nothing before the pane is quiet; then steps until done, then no deadline at all.
        assert!(!engine.expire(quiet - Duration::from_millis(1), &mut replies));
        assert_eq!(engine.deadline(), Some(quiet));
        let mut now = quiet;
        let mut steps = 0;
        while let Some(at) = engine.deadline() {
            now = now.max(at);
            assert!(!engine.expire(now, &mut replies), "compression does not change the screen");
            steps += 1;
            assert!(steps < 100_000);
        }
        assert!(steps >= 1);
        // The text is still there, read back from compressed pages.
        let oldest = engine.oldest();
        assert!(engine.line(oldest).unwrap().text.starts_with("compressible text"));
    }

    #[test]
    fn resize_keeps_the_screen_and_the_history() {
        let screen = |engine: &Ghostty, (w, h): (u16, u16)| {
            let mut canvas = Canvas::new(usize::from(w), usize::from(h));
            engine.draw(&mut canvas, Rect { x: 0, y: 0, width: usize::from(w), height: usize::from(h) });
            (0..usize::from(h)).map(|y| canvas.row(y)).collect::<Vec<_>>()
        };
        // Text at the top: smaller, larger, narrower, the blank rows below go, the text stays where it was (a
        // program that does not redraw on SIGWINCH, the shell after Claude, keeps its screen).
        let mut engine = Ghostty::new(80, 24, 1000).unwrap();
        answer(&mut engine, b"ligne-1\r\nligne-2\r\nligne-3\r\n");
        for size in [(80, 20), (100, 29), (60, 10), (200, 50), (79, 24)] {
            engine.resize(size.0, size.1);
            assert_eq!(screen(&engine, size)[..4], ["ligne-1", "ligne-2", "ligne-3", ""], "{size:?}");
            assert_eq!(engine.history(), 0, "{size:?}");
            rows(&engine);
            assert_eq!(engine.cursor().map(|c| (c.x, c.y)), Some((0, 3)), "{size:?}");
        }
        // A cleared screen written at the top, likewise.
        answer(&mut engine, b"\x1b[2J\x1b[Hen haut");
        engine.resize(70, 12);
        assert_eq!(screen(&engine, (70, 12))[0], "en haut");
        // Smaller than the text: what does not fit goes into the history, nothing is lost.
        let mut engine = Ghostty::new(20, 10, 1000).unwrap();
        for i in 0..8 {
            answer(&mut engine, format!("line {i}\r\n").as_bytes());
        }
        engine.resize(20, 4);
        let shown = screen(&engine, (20, 4));
        assert!(shown.contains(&"line 7".to_string()), "{shown:?}");
        assert!(engine.history() > 0);
        let kept = |engine: &Ghostty| {
            let first = engine.oldest();
            (first..first + 8)
                .filter_map(|line| engine.line(line))
                .map(|line| line.text.trim_end().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(kept(&engine), (0..8).map(|i| format!("line {i}")).collect::<Vec<_>>());
        engine.scroll(100);
        assert_eq!(rows(&engine)[0], "line 0");
        engine.scroll(isize::MIN);
        // Larger again: everything is still there, on the screen or in the history.
        engine.resize(20, 12);
        let all = kept(&engine);
        assert_eq!(all, (0..8).map(|i| format!("line {i}")).collect::<Vec<_>>());
    }

    #[test]
    fn long_graphemes_are_copied_whole() {
        let mut engine = engine();
        // A family of five joined by ZWJ: nine code points in one cell.
        let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦\u{200d}👦";
        answer(&mut engine, format!("{family}|").as_bytes());
        let line = engine.line(engine.top()).unwrap();
        assert_eq!((line.cell(0), line.cell(1), line.cell(2)), (family, "", "|"));
    }

    #[test]
    fn frames_follow_every_change_though_rows_are_read_again_only_when_dirty() {
        let mut engine = engine();
        answer(&mut engine, b"one\r\ntwo\r\n\x1b[31mred\x1b[0m\r\nfour");
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        assert_eq!(rows(&engine)[..4], ["one", "two", "red", "four"]);
        // A change in one row, the rest kept; then nothing new.
        answer(&mut engine, b"\x1b[2;1HTWO");
        assert_eq!(rows(&engine)[..4], ["one", "TWO", "red", "four"]);
        assert_eq!(rows(&engine)[..4], ["one", "TWO", "red", "four"]);
        // The cursor moves alone.
        answer(&mut engine, b"\x1b[5;3H");
        rows(&engine);
        assert_eq!(engine.cursor().map(|c| (c.x, c.y)), Some((2, 4)));
        // Colors changed by the program after a frame: every cell drawn with them shows it.
        answer(&mut engine, b"\x1b]4;1;rgb:00/ff/00\x1b\\");
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        assert_eq!(canvas.style(0, 2).fg, Some(Paint::Rgb { r: 0, g: 0xff, b: 0 }));
        answer(&mut engine, b"\x1b]11;rgb:01/02/03\x1b\\");
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        assert_eq!(canvas.style(10, 0).bg, Some(Paint::Rgb { r: 1, g: 2, b: 3 }));
        // The view moves up into the history: the frame follows.
        for i in 0..10 {
            answer(&mut engine, format!("\r\nmore {i}").as_bytes());
        }
        rows(&engine);
        engine.scroll(3);
        let shown = rows(&engine);
        engine.scroll(isize::MIN);
        assert_ne!(shown, rows(&engine));
        assert_eq!(rows(&engine)[4], "more 9");
    }

    #[test]
    fn generation_changes_with_the_cells_drawn_only() {
        let mut engine = engine();
        let mut seen = engine.generation().unwrap();
        let mut step = |engine: &mut Ghostty, bytes: &[u8]| {
            answer(engine, bytes);
            let now = engine.generation().unwrap();
            let changed = now != seen;
            seen = now;
            changed
        };
        assert!(step(&mut engine, b"hello\r\nworld"));
        assert!(!step(&mut engine, b""), "nothing new");
        assert!(!step(&mut engine, b"\x1b[3;5H"), "the cursor alone");
        assert_eq!(engine.cursor().map(|c| (c.x, c.y)), Some((4, 2)), "given up to date");
        assert!(!step(&mut engine, b"\x1b[1;1Hhello"), "the same text written again");
        // Drawn by someone else in between (a capture): still seen as changed by the screen.
        answer(&mut engine, b"!");
        let mut canvas = Canvas::new(20, 5);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
        assert!(step(&mut engine, b""));
        assert!(step(&mut engine, b"\x1b]11;rgb:01/02/03\x1b\\"), "the background");
        assert!(step(&mut engine, b"\x1b]4;1;rgb:00/ff/00\x1b\\\x1b[31mred\x1b[0m"));
        assert!(step(&mut engine, b"\x1b]4;1;rgb:00/00/ff\x1b\\"), "a color of the palette in use");
        // Held by a synchronized update, then shown.
        assert!(!step(&mut engine, b"\x1b[?2026hsome\r\nmore"));
        assert!(step(&mut engine, b"\x1b[?2026l"));
        // The view moves into the history and back.
        for i in 0..10 {
            answer(&mut engine, format!("\r\nline {i}").as_bytes());
        }
        step(&mut engine, b"");
        engine.scroll(2);
        assert!(step(&mut engine, b""));
        engine.scroll(isize::MIN);
        assert!(step(&mut engine, b""));
        engine.resize(30, 6);
        assert!(step(&mut engine, b""));
        // A new engine for the same pane: never a number the old one gave.
        let again = super::super::new(20, 5, 100).unwrap();
        assert_ne!(again.generation(), Some(seen));
        assert_ne!(again.generation(), self::engine().generation());
    }

    /// Where a frame's time goes, with Claude-like lines (colored bullet, bold, a path): `cargo test --release
    /// draw_costs -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measure, not a check"]
    fn draw_costs() {
        let (cols, rows) = (120u16, 50u16);
        let area = Rect { x: 0, y: 0, width: usize::from(cols), height: usize::from(rows) };
        let line = |i: usize| {
            format!(
                "\x1b[38;2;215;119;87m⏺\x1b[0m Line {i} of a reply, with \x1b[1mbold\x1b[0m text and a path src/mux/pty.rs:{i}\r\n"
            )
        };
        let mut engine = Ghostty::new(cols, rows, 100_000).unwrap();
        for i in 0..2000 {
            answer(&mut engine, line(i).as_bytes());
        }
        let mut painter = crate::canvas::Painter::new(crate::canvas::Features::default());
        let (mut capture, mut paint, mut painted, mut colors) = (0u128, 0u128, 0u128, 0u128);
        let frames = 500;
        for (i, changing) in (0..frames * 2).map(|i| (i, i < frames)) {
            if changing {
                answer(&mut engine, line(i).as_bytes());
            } else {
                // An indicator that turns, as Claude's does: one cell of one row.
                answer(&mut engine, format!("\x1b7\x1b[10;1H{}\x1b8", ['◐', '◓', '◑', '◒'][i % 4]).as_bytes());
            }
            let started = Instant::now();
            let _ = std::hint::black_box(engine.colors());
            colors += started.elapsed().as_nanos();
            let started = Instant::now();
            engine.capture(&mut engine.view.borrow_mut());
            capture += started.elapsed().as_nanos();
            let mut canvas = Canvas::new(area.width, area.height);
            let started = Instant::now();
            engine.view.borrow().frame.paint(&mut canvas, area);
            paint += started.elapsed().as_nanos();
            let started = Instant::now();
            let _ = std::hint::black_box(painter.frame(&canvas));
            painted += started.elapsed().as_nanos();
            if i == frames - 1 || i == frames * 2 - 1 {
                let n = frames as u128 * 1000;
                println!(
                    "{}: colors {} µs, capture {} µs (colors included), paint {} µs, Painter::frame {} µs",
                    if changing { "a line a frame (all rows move)" } else { "one cell a frame" },
                    colors / n,
                    capture / n,
                    paint / n,
                    painted / n
                );
                (capture, paint, painted, colors) = (0, 0, 0, 0);
            }
        }
    }

    #[test]
    fn resized_down_to_nothing() {
        let mut engine = engine();
        answer(&mut engine, "界界界界界界界界界界界\r\n".repeat(10).as_bytes());
        for (cols, rows) in [(1, 1), (0, 0), (3, 2), (200, 60), (1, 30)] {
            engine.resize(cols, rows);
            answer(&mut engine, "界a\u{301}\r\n".repeat(3).as_bytes());
            let mut canvas = Canvas::new(200, 60);
            engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 200, height: 60 });
            let _ = (engine.top(), engine.line(engine.oldest()));
        }
    }
}
