// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The libghostty-vt engine (spec §5.2): Ghostty's terminal, with grapheme clusters measured as Claude Code measures
//! them (mode 2027) and its history compressed. Ghostty answers the queries itself, through callbacks; the sniffer
//! still finds what it lets pass (program statuses, notifications, progress). A synchronized update (mode 2026) does
//! not hold the bytes back: the frame last drawn is shown again until it ends, or until its deadline.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use libghostty_vt::Terminal;
use libghostty_vt::screen::{CellWide, Screen, TrackedGridRef};
use libghostty_vt::style::RgbColor;
use libghostty_vt::terminal::{
    ClipboardLocation, ColorScheme, CompressionActivity, CompressionMode, CompressionResult, CursorStyle, Mode, Point,
    PointCoordinate, PointSpace, ScrollViewport, SizeReportSize,
};

use self::frame::View;
use super::sniff::{Sniffed, Sniffer};
use super::{Engine, Line, Modes, MouseTracking, Relay, State, Surface};
use crate::ui::canvas::{Canvas, Cursor};
use crate::ui::look::light_background;
use crate::ui::{Rect, Rgb};

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

/// What Ghostty's callbacks leave for the engine, during a write.
#[derive(Default)]
struct Shared {
    replies: Vec<u8>,
    relays: Vec<Relay>,
    /// Whether the real terminal's theme is light, for the color scheme report (`CSI ? 996 n`): that of the client
    /// attached now ([`Engine::set_colors`], [`Engine::color_scheme`]); `None` while no client told its background
    /// (a dark one is answered, and nothing is said unasked).
    light: Option<bool>,
    /// The theme the program was last told, by an answer to `CSI ? 996 n` or a report (mode 2031); `None` before
    /// any.
    told: Option<bool>,
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
    /// until `recruit _member` says it left (`state=clear:app=recruit`), or until a reset. A shell replaying old
    /// output does not set a status.
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

impl Ghostty {
    pub(super) fn new(cols: u16, rows: u16, history: usize) -> Result<Ghostty> {
        let mut term = Terminal::new(cols.max(1), rows.max(1)).context("libghostty-vt")?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        term.set_scrollback_max_bytes(None)?.set_scrollback_max_lines(Some(history))?;
        // Grapheme clusters measured whole, as Claude Code does (Bun.stringWidth) and as the real terminal draws them:
        // the default, so that a full reset (RIS) goes back to it rather than to a cell-by-cell measure.
        term.set_default_mode(Mode::GRAPHEME_CLUSTER, true)?;
        // DECSCUSR cannot ask for it: what the cursor is when the program asked for nothing.
        term.set_default_cursor_style(Some(CursorStyle::BlockHollow))?;
        term.set_default_fg_color(Some(rgb_color(DEFAULT_FG)))?;
        term.set_default_bg_color(Some(rgb_color(DEFAULT_BG)))?;
        let to = Arc::clone(&shared);
        term.on_pty_write(move |_, data| lock(&to).replies.extend_from_slice(data))?;
        term.on_xtversion(|_| Some(VERSION))?;
        let to = Arc::clone(&shared);
        term.on_color_scheme(move |_| {
            let mut shared = lock(&to);
            let light = shared.light.unwrap_or(false);
            shared.told = Some(light);
            Some(scheme(light))
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
        let view = View::new()?;
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
            Sniffed::Notify { title, body } => self.relays.push(Relay::Notify { title, body }),
            Sniffed::Progress(progress) => self.relays.push(Relay::Progress(progress)),
            // Program statuses are taken: Claude Code then says when it works, waits and is done.
            Sniffed::StatusQuery => {
                replies.extend(b"\x1b]7501;?\x1b\\");
                self.armed = true;
            }
            // One of its tasks (Claude Code: a sub-agent, `clear` when it ends): the pane's state is the program's.
            Sniffed::Status(status) if status.id.is_some() => {}
            Sniffed::Status(status) if self.armed => {
                // Claude Code's own `clear` (no `app`) may come with its statuses going on after it.
                let left = status.state == State::Clear && status.app.as_deref() == Some("recruit");
                self.armed = !left;
                if left {
                    self.forget_theme();
                }
                self.relays.push(Relay::Status(status));
            }
            Sniffed::Status(_) => {}
            Sniffed::Reset => {
                self.armed = false;
                self.forget_theme();
            }
            Sniffed::SyncEnd => {
                self.sync_ended();
                // The list that ended the update may have turned graphemes off too ([`Sniffed::GraphemesOff`]).
                self.keep_graphemes();
            }
            Sniffed::ThemeReports => self.theme_asked(replies),
            Sniffed::GraphemesOff => self.keep_graphemes(),
            // The engine does nothing with it: the server decides.
            Sniffed::Resize { rows, cols } => self.relays.push(Relay::Resize { rows, cols }),
        }
    }

    /// The program turned grapheme clusters (mode 2027) off, or may have: they stay on. The real terminal draws them
    /// whole whatever a program asks (the client keeps its mode 2027 on, or turns it on where it is known, and never
    /// follows a pane), so the engine, whose cursor positions and cell widths the program reads back and the screen is
    /// composed from, measures as it draws: a pane that went cell by cell would shift the rest of the line, and the
    /// frame around it, by the width the real terminal gives to an emoji with a skin tone or a flag. Setting what is
    /// already set does nothing, and this runs right after the sequence, before the bytes that follow.
    fn keep_graphemes(&mut self) {
        let _ = self.term.set_mode(Mode::GRAPHEME_CLUSTER, true);
    }

    /// The program turned mode 2031 on: if it was never told the theme (it did not ask with `CSI ? 996 n`) and the
    /// client's is known, it is told now, where the mode was set in what it wrote. Later changes come with
    /// [`Engine::color_scheme`].
    fn theme_asked(&mut self, replies: &mut Vec<u8>) {
        let mut shared = lock(&self.shared);
        if let Some(light) = shared.light
            && shared.told.is_none()
            && self.term.mode(Mode::COLOR_SCHEME_REPORT).unwrap_or(false)
        {
            tell_theme(&mut shared, light, replies);
        }
    }

    /// The program that was told the theme is gone (a reset, or `recruit _member` saying its Claude stopped): the next
    /// one knows nothing, and is told where the theme stands when it turns mode 2031 on, even if it never asks. Its
    /// mode goes with it: a Claude that was killed did not turn it off, and the shell that takes the pane over must
    /// never be sent a report it did not ask for (a reset has already turned it off).
    fn forget_theme(&mut self) {
        lock(&self.shared).told = None;
        let _ = self.term.set_mode(Mode::COLOR_SCHEME_REPORT, false);
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
}

impl Engine for Ghostty {
    fn feed(&mut self, bytes: &[u8], replies: &mut Vec<u8>) {
        let now = Instant::now();
        if self.deadline().is_some_and(|at| at <= now) {
            self.expire(now);
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

    fn expire(&mut self, now: Instant) -> bool {
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
        // The theme follows the background, by the interface's own measure.
        lock(&self.shared).light = bg.map(light_background);
    }

    fn color_scheme(&mut self, light: bool, replies: &mut Vec<u8>) {
        let mut shared = lock(&self.shared);
        shared.light = Some(light);
        // Told only to a program that asked for it (mode 2031), and only a change: an attach that finds the theme it
        // left tells nothing.
        if shared.told == Some(light) || !self.term.mode(Mode::COLOR_SCHEME_REPORT).unwrap_or(false) {
            return;
        }
        tell_theme(&mut shared, light, replies);
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
            bracketed_paste: on(Mode::BRACKETED_PASTE),
            focus: on(Mode::FOCUS_EVENT),
            mouse,
            mouse_sgr: on(Mode::SGR_MOUSE),
            alternate_scroll: on(Mode::ALT_SCROLL),
            alt_screen: self.term.active_screen().unwrap_or_default() == Screen::Alternate,
            kitty: self.term.kitty_keyboard_flags().map_or(0, |flags| flags.bits()),
        }
    }

    fn scroll(&mut self, lines: isize) {
        let scroll =
            if lines == isize::MIN { ScrollViewport::Bottom } else { ScrollViewport::Delta(lines.saturating_neg()) };
        self.term.scroll_viewport(scroll);
        // Pages read from the history were decompressed: compressed again once the pane is quiet.
        self.busy(Instant::now());
    }
}

impl Surface for Ghostty {
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

fn scheme(light: bool) -> ColorScheme {
    if light { ColorScheme::Light } else { ColorScheme::Dark }
}

/// The report of a theme (`CSI ? 997 ; 1|2 n`), appended to `replies`: the program now knows it.
fn tell_theme(shared: &mut Shared, light: bool, replies: &mut Vec<u8>) {
    let mut report = [0u8; 16];
    if let Ok(len) = scheme(light).encode_report(&mut report) {
        replies.extend_from_slice(&report[..len]);
        shared.told = Some(light);
    }
}

mod frame;
#[cfg(test)]
mod tests;
