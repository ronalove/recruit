// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A whole screen drawn in the terminal's cells, for the menu: each frame is drawn into a [`Canvas`], then
//! [`Painter`] sends only the cells that changed since the frame before, in one write. And the terminal while such a
//! screen is on it ([`Session`]), given back as it was however the program ends, a panic included ([`PanicGuard`]).
//!
//! tmux keeps the synchronized updates of a program in a pane (3.7c), not in a popup (3.5a, 3.7c): no frame relies on
//! them, each goes out in a single write instead.

use std::io::{self, Stdout, Write};
use std::panic::{self, PanicHookInfo};
use std::sync::Arc;

use crossterm::style::Color;
use crossterm::{cursor, event, queue, terminal};
use unicode_width::UnicodeWidthChar;

/// How a cell is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub reverse: bool,
    pub underline: bool,
}

impl Style {
    pub(crate) const PLAIN: Style =
        Style { fg: None, bg: None, bold: false, dim: false, reverse: false, underline: false };

    pub(crate) fn fg(color: Color) -> Style {
        Style { fg: Some(color), ..Style::PLAIN }
    }

    pub(crate) fn bold(self) -> Style {
        Style { bold: true, ..self }
    }

    pub(crate) fn dim(self) -> Style {
        Style { dim: true, ..self }
    }

    pub(crate) fn reverse(self) -> Style {
        Style { reverse: true, ..self }
    }

    pub(crate) fn on(self, bg: Color) -> Style {
        Style { bg: Some(bg), ..self }
    }
}

/// Where a cell stands in a character two columns wide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Wide {
    #[default]
    One,
    /// Its first column: the character is drawn from here.
    Lead,
    /// Its second column, drawn with the first.
    Tail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    ch: char,
    style: Style,
    wide: Wide,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', style: Style::PLAIN, wide: Wide::One }
    }
}

/// A screen's worth of cells. Text is measured in columns (unicode-width), one character after the other: a
/// character of no width (a combining mark, a joiner) is left out, so that every cell holds what it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Canvas {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
}

/// Columns of `text` as a canvas draws it.
pub(crate) fn columns(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// `text` in `room` columns at most, with « … » when it is cut.
pub(crate) fn fit(text: &str, room: usize) -> String {
    if columns(text) <= room {
        return text.to_string();
    }
    let mut cut = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > room {
            break;
        }
        cut.push(c);
        used += w;
    }
    if room > 0 {
        cut.truncate(cut.trim_end().len());
        cut.push('…');
    }
    cut
}

impl Canvas {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        Canvas { width, height, cells: vec![Cell::default(); width * height] }
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }

    fn at(&self, x: usize, y: usize) -> Option<usize> {
        (x < self.width && y < self.height).then(|| y * self.width + x)
    }

    /// Sets a cell, and blanks what remains of a wide character it cuts in two.
    fn set(&mut self, x: usize, y: usize, cell: Cell) {
        let Some(i) = self.at(x, y) else { return };
        match self.cells[i].wide {
            Wide::Lead if cell.wide != Wide::Lead => {
                if let Some(next) = self.at(x + 1, y) {
                    self.cells[next] = Cell { ch: ' ', wide: Wide::One, ..self.cells[next] };
                }
            }
            // The second half of another wide character in its place: the first half is the new one's.
            Wide::Tail if x > 0 && cell.wide != Wide::Tail => {
                let before = i - 1;
                self.cells[before] = Cell { ch: ' ', wide: Wide::One, ..self.cells[before] };
            }
            _ => {}
        }
        self.cells[i] = cell;
    }

    /// Writes `text` from `x` on row `y`, cut at the canvas's edge; returns the column after it.
    pub(crate) fn put(&mut self, x: usize, y: usize, text: &str, style: Style) -> usize {
        self.put_in(x, y, self.width, text, style)
    }

    /// Writes `text` from `x` on row `y`, cut before column `end`; returns the column after it.
    pub(crate) fn put_in(&mut self, mut x: usize, y: usize, end: usize, text: &str, style: Style) -> usize {
        let end = end.min(self.width);
        for c in text.chars() {
            let w = c.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            if x + w > end {
                break;
            }
            if w == 2 {
                self.set(x, y, Cell { ch: c, style, wide: Wide::Lead });
                self.set(x + 1, y, Cell { ch: ' ', style, wide: Wide::Tail });
            } else {
                self.set(x, y, Cell { ch: c, style, wide: Wide::One });
            }
            x += w;
        }
        x
    }

    /// Writes `text` so that it ends just before column `end`; returns the column it starts at.
    pub(crate) fn put_right(&mut self, end: usize, y: usize, text: &str, style: Style) -> usize {
        let start = end.saturating_sub(columns(text));
        self.put_in(start, y, end, text, style);
        start
    }

    /// Blanks a rectangle in `style`.
    pub(crate) fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, style: Style) {
        for row in y..(y + height).min(self.height) {
            for col in x..(x + width).min(self.width) {
                self.set(col, row, Cell { ch: ' ', style, wide: Wide::One });
            }
        }
    }

    /// Puts `bg` behind `width` cells of row `y`, keeping what they show.
    pub(crate) fn tint(&mut self, x: usize, y: usize, width: usize, bg: Color) {
        for col in x..(x + width).min(self.width) {
            if let Some(i) = self.at(col, y) {
                self.cells[i].style.bg = Some(bg);
            }
        }
    }

    /// A line of `ch` from `x`, `width` long.
    pub(crate) fn hline(&mut self, x: usize, y: usize, width: usize, ch: char, style: Style) {
        for col in x..(x + width).min(self.width) {
            self.set(col, y, Cell { ch, style, wide: Wide::One });
        }
    }

    /// A column of `ch` from `y`, `height` long.
    pub(crate) fn vline(&mut self, x: usize, y: usize, height: usize, ch: char, style: Style) {
        for row in y..(y + height).min(self.height) {
            self.set(x, row, Cell { ch, style, wide: Wide::One });
        }
    }

    /// A box with round corners, blank inside.
    pub(crate) fn frame(&mut self, x: usize, y: usize, width: usize, height: usize, style: Style) {
        if width < 2 || height < 2 {
            return;
        }
        self.fill(x, y, width, height, Style::PLAIN);
        let (right, bottom) = (x + width - 1, y + height - 1);
        self.hline(x + 1, y, width - 2, '─', style);
        self.hline(x + 1, bottom, width - 2, '─', style);
        self.vline(x, y + 1, height - 2, '│', style);
        self.vline(right, y + 1, height - 2, '│', style);
        for (col, row, corner) in [(x, y, '╭'), (right, y, '╮'), (x, bottom, '╰'), (right, bottom, '╯')] {
            self.set(col, row, Cell { ch: corner, style, wide: Wide::One });
        }
    }

    /// Everything outside the rectangle drawn in `faded`, as it is otherwise: what is behind a dialog.
    pub(crate) fn fade_outside(&mut self, x: usize, y: usize, width: usize, height: usize, faded: Style) {
        for row in 0..self.height {
            for col in 0..self.width {
                if (x..x + width).contains(&col) && (y..y + height).contains(&row) {
                    continue;
                }
                let i = row * self.width + col;
                self.cells[i].style = faded;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn height(&self) -> usize {
        self.height
    }

    /// Row `y`'s text, its wide characters once.
    #[cfg(test)]
    pub(crate) fn row(&self, y: usize) -> String {
        self.cells[y * self.width..(y + 1) * self.width]
            .iter()
            .filter(|c| c.wide != Wide::Tail)
            .map(|c| c.ch)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// The style of the cell at `x`, `y`.
    #[cfg(test)]
    pub(crate) fn style(&self, x: usize, y: usize) -> Style {
        self.cells[y * self.width + x].style
    }
}

/// Sends frames by difference with the one on screen.
#[derive(Default)]
pub(crate) struct Painter {
    shown: Option<Canvas>,
    /// The terminal's current style, when known.
    style: Option<Style>,
}

/// Unchanged cells this close between two changed ones, in the same style, are written again rather than skipped.
const GAP: usize = 4;

impl Painter {
    /// What is on screen is unknown (another program wrote on it): the next frame is sent whole.
    pub(crate) fn forget(&mut self) {
        *self = Painter::default();
    }

    /// The bytes that turn the screen into `next`.
    pub(crate) fn frame(&mut self, next: Canvas) -> Vec<u8> {
        let mut out = Vec::new();
        let shown = self.shown.take();
        let whole = shown.as_ref().is_none_or(|shown| (shown.width, shown.height) != (next.width, next.height));
        if whole {
            out.extend_from_slice(b"\x1b[0m\x1b[2J");
            self.style = Some(Style::PLAIN);
        }
        let mut at: Option<(usize, usize)> = None;
        for y in 0..next.height {
            let row = &next.cells[y * next.width..(y + 1) * next.width];
            let old = shown.as_ref().filter(|_| !whole).map(|shown| &shown.cells[y * next.width..(y + 1) * next.width]);
            let changed = |x: usize| old.is_none_or(|old| old[x] != row[x]);
            let mut x = 0;
            while x < next.width {
                let cell = row[x];
                // A wide character goes with both its columns.
                let differs = changed(x) || (cell.wide == Wide::Lead && x + 1 < next.width && changed(x + 1));
                if !differs || cell.wide == Wide::Tail {
                    x += 1;
                    continue;
                }
                if at != Some((x, y)) {
                    let _ = write!(out, "\x1b[{};{}H", y + 1, x + 1);
                }
                // The run: changed cells, and the unchanged ones in short gaps between them.
                let mut end = x;
                loop {
                    let c = row[end];
                    if c.wide != Wide::Tail {
                        self.sgr(&mut out, c.style);
                        let mut buf = [0; 4];
                        out.extend_from_slice(c.ch.encode_utf8(&mut buf).as_bytes());
                    }
                    end += 1;
                    if end >= next.width {
                        break;
                    }
                    if changed(end) {
                        continue;
                    }
                    // Written again, unchanged cells cost their bytes; skipped, a move costs about six.
                    let next_change = (end..next.width.min(end + GAP + 1)).find(|&i| changed(i));
                    match next_change {
                        Some(i)
                            if row[end..i].iter().all(|c| c.wide == Wide::One && Some(c.style) == self.style)
                                && row[end..i].iter().map(|c| c.ch.len_utf8()).sum::<usize>() < 6 =>
                        {
                            continue;
                        }
                        _ => break,
                    }
                }
                at = Some((end, y));
                x = end;
            }
        }
        self.shown = Some(next);
        out
    }

    /// Switches the terminal to `style`, saying only what changes.
    fn sgr(&mut self, out: &mut Vec<u8>, style: Style) {
        if self.style == Some(style) {
            return;
        }
        let mut params: Vec<String> = Vec::new();
        let from = match self.style {
            // An attribute to turn off: from scratch.
            Some(old)
                if !(old.bold && !style.bold
                    || old.dim && !style.dim
                    || old.reverse && !style.reverse
                    || old.underline && !style.underline) =>
            {
                old
            }
            _ => {
                params.push("0".into());
                Style::PLAIN
            }
        };
        if style.bold && !from.bold {
            params.push("1".into());
        }
        if style.dim && !from.dim {
            params.push("2".into());
        }
        if style.underline && !from.underline {
            params.push("4".into());
        }
        if style.reverse && !from.reverse {
            params.push("7".into());
        }
        if style.fg != from.fg {
            params.push(style.fg.map_or("39".into(), |c| color(c, false)));
        }
        if style.bg != from.bg {
            params.push(style.bg.map_or("49".into(), |c| color(c, true)));
        }
        if !params.is_empty() {
            let _ = write!(out, "\x1b[{}m", params.join(";"));
        }
        self.style = Some(style);
    }
}

/// A color's SGR parameters, the sixteen named ones by their short codes.
fn color(color: Color, bg: bool) -> String {
    let base = |code: u8| (if bg { code + 10 } else { code }).to_string();
    match color {
        Color::Reset => base(39),
        Color::Black => base(30),
        Color::DarkRed => base(31),
        Color::DarkGreen => base(32),
        Color::DarkYellow => base(33),
        Color::DarkBlue => base(34),
        Color::DarkMagenta => base(35),
        Color::DarkCyan => base(36),
        Color::Grey => base(37),
        Color::DarkGrey => base(90),
        Color::Red => base(91),
        Color::Green => base(92),
        Color::Yellow => base(93),
        Color::Blue => base(94),
        Color::Magenta => base(95),
        Color::Cyan => base(96),
        Color::White => base(97),
        Color::AnsiValue(n) => format!("{};5;{n}", if bg { 48 } else { 38 }),
        Color::Rgb { r, g, b } => format!("{};2;{r};{g};{b}", if bg { 48 } else { 38 }),
    }
}

/// Mouse reports: presses, releases, the wheel and drags, in SGR coordinates; not the moves without a button, which
/// would wake the program at each one.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1002l\x1b[?1000l";

/// The terminal while a full screen is on it: raw, on its alternate screen, without its cursor, with the mouse and
/// pastes reported. Dropping it gives everything back; a panic does it before its message.
pub(crate) struct Session {
    out: Stdout,
    _panic: PanicGuard,
}

impl Session {
    pub(crate) fn open() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // From here, Drop puts the terminal back.
        let mut session = Session { out: io::stdout(), _panic: PanicGuard::install(leave) };
        session.enter()?;
        Ok(session)
    }

    fn enter(&mut self) -> io::Result<()> {
        queue!(self.out, terminal::EnterAlternateScreen, cursor::Hide, event::EnableBracketedPaste)?;
        self.out.write_all(MOUSE_ON.as_bytes())?;
        self.out.flush()
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.out.write_all(bytes)?;
        self.out.flush()
    }

    /// Hands the terminal to another program (an editor) while `run` runs, then takes it back: the next frame must
    /// be sent whole.
    pub(crate) fn lend<T>(&mut self, run: impl FnOnce() -> T) -> io::Result<T> {
        leave();
        let result = run();
        terminal::enable_raw_mode()?;
        self.enter()?;
        Ok(result)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A panic: its hook did it already.
        if !std::thread::panicking() {
            leave();
        }
    }
}

/// The terminal as before a full screen: its own screen back, the cursor shown, nothing reported, out of raw mode.
fn leave() {
    let mut out = io::stdout();
    let _ = out.write_all(MOUSE_OFF.as_bytes());
    let _ = queue!(out, event::DisableBracketedPaste, cursor::Show, terminal::LeaveAlternateScreen);
    let _ = out.write_all(b"\x1b[0m");
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// While it lives, a panic runs `restore` before the hook in place when it was installed; dropping it puts that hook
/// back.
pub(crate) struct PanicGuard {
    before: Option<Arc<Hook>>,
}

impl PanicGuard {
    pub(crate) fn install(restore: fn()) -> Self {
        let before = Arc::new(panic::take_hook());
        let hook = Arc::clone(&before);
        panic::set_hook(Box::new(move |info| {
            restore();
            hook(info)
        }));
        PanicGuard { before: Some(before) }
    }
}

impl Drop for PanicGuard {
    fn drop(&mut self) {
        // A panicking thread cannot change the hook: this one stays, and puts the terminal back once more.
        if std::thread::panicking() {
            return;
        }
        // Dropping the list's hook leaves the one before alone in its Arc.
        drop(panic::take_hook());
        if let Some(before) = self.before.take().and_then(|before| Arc::try_unwrap(before).ok()) {
            panic::set_hook(before);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn text_in_columns() {
        let mut canvas = Canvas::new(10, 2);
        assert_eq!(canvas.put(1, 0, "日本x", Style::PLAIN), 6);
        assert_eq!(canvas.row(0), " 日本x");
        // Cut before a wide character that would not fit.
        assert_eq!(canvas.put(8, 1, "a日", Style::PLAIN), 9);
        assert_eq!(canvas.row(1), "        a");
        // Written over half a wide character: the other half goes blank.
        canvas.put(2, 0, "z", Style::PLAIN);
        assert_eq!(canvas.row(0), "  z本x");
        let mut canvas = Canvas::new(10, 1);
        canvas.put(0, 0, "日", Style::PLAIN);
        canvas.put(0, 0, "a", Style::PLAIN);
        assert_eq!(canvas.row(0), "a");
        // A wide character over another, or over one shifted by a column.
        let mut canvas = Canvas::new(10, 1);
        canvas.put(0, 0, "日本", Style::PLAIN);
        canvas.put(0, 0, "語", Style::PLAIN);
        assert_eq!(canvas.row(0), "語本");
        canvas.put(1, 0, "語", Style::PLAIN);
        assert_eq!(canvas.row(0), " 語");
        // A joiner or an accent on its own takes no cell.
        let mut canvas = Canvas::new(10, 1);
        assert_eq!(canvas.put(0, 0, "e\u{301}\u{200d}", Style::PLAIN), 1);
        assert_eq!(canvas.row(0), "e");
    }

    #[test]
    fn right_aligned_and_cut() {
        let mut canvas = Canvas::new(12, 1);
        assert_eq!(canvas.put_right(12, 0, "équipe", Style::PLAIN), 6);
        assert_eq!(canvas.row(0), "      équipe");
        assert_eq!(fit("settings.local.toml", 10), "settings.…");
        assert_eq!(fit("court", 10), "court");
        assert_eq!(fit("日本語のラベル", 7), "日本語…");
        assert_eq!(columns("日本x"), 5);
    }

    #[test]
    fn boxes_and_fades() {
        let mut canvas = Canvas::new(6, 4);
        canvas.put(0, 0, "abcdef", Style::fg(Color::Cyan));
        canvas.frame(1, 1, 4, 3, Style::fg(Color::DarkGrey));
        assert_eq!((canvas.row(1), canvas.row(2), canvas.row(3)), (" ╭──╮".into(), " │  │".into(), " ╰──╯".into()));
        let faded = Style::fg(Color::AnsiValue(239));
        canvas.fade_outside(1, 1, 4, 3, faded);
        assert_eq!(canvas.style(0, 0), faded);
        assert_eq!(canvas.style(1, 1), Style::fg(Color::DarkGrey));
        canvas.tint(0, 0, 2, Color::AnsiValue(236));
        assert_eq!(canvas.style(1, 0).bg, Some(Color::AnsiValue(236)));
        assert_eq!(canvas.row(0), "abcdef", "tinted, it shows the same");
    }

    #[test]
    fn only_what_changed() {
        let mut painter = Painter::default();
        let mut canvas = Canvas::new(20, 3);
        canvas.put(0, 0, "Que veux-tu changer ?", Style::PLAIN);
        let first = text(&painter.frame(canvas.clone()));
        assert!(first.starts_with("\x1b[0m\x1b[2J"), "the first frame whole");
        assert!(painter.frame(canvas.clone()).is_empty(), "nothing changed, nothing sent");

        canvas.put(4, 1, "x", Style::fg(Color::Cyan));
        assert_eq!(text(&painter.frame(canvas.clone())), "\x1b[2;5H\x1b[96mx");
        // The same style again: no SGR. A short gap in another style: moved across.
        canvas.put(4, 1, "y", Style::fg(Color::Cyan));
        canvas.put(7, 1, "z", Style::fg(Color::Cyan));
        assert_eq!(text(&painter.frame(canvas.clone())), "\x1b[2;5Hy\x1b[2;8Hz");
        // In the same style: written over.
        canvas.put(4, 1, "abcd", Style::fg(Color::Cyan));
        painter.frame(canvas.clone());
        canvas.put(4, 1, "e", Style::fg(Color::Cyan));
        canvas.put(7, 1, "f", Style::fg(Color::Cyan));
        assert_eq!(text(&painter.frame(canvas.clone())), "\x1b[2;5Hebcf");
        // A long gap: moved across.
        canvas.put(0, 2, "a", Style::PLAIN);
        canvas.put(15, 2, "b", Style::PLAIN);
        assert_eq!(text(&painter.frame(canvas.clone())), "\x1b[3;1H\x1b[39ma\x1b[3;16Hb");
    }

    #[test]
    fn styles_say_only_what_changes() {
        let mut painter = Painter::default();
        let mut out = Vec::new();
        painter.style = Some(Style::PLAIN);
        painter.sgr(&mut out, Style::fg(Color::AnsiValue(141)).bold());
        painter.sgr(&mut out, Style::fg(Color::AnsiValue(141)).bold().on(Color::AnsiValue(236)));
        painter.sgr(&mut out, Style::fg(Color::Yellow));
        painter.sgr(&mut out, Style::fg(Color::Yellow));
        assert_eq!(text(&out), "\x1b[1;38;5;141m\x1b[48;5;236m\x1b[0;93m");
    }

    #[test]
    fn wide_characters_go_whole() {
        let mut painter = Painter::default();
        let mut canvas = Canvas::new(6, 1);
        canvas.put(0, 0, "ab", Style::PLAIN);
        painter.frame(canvas.clone());
        canvas.put(0, 0, "日", Style::PLAIN);
        assert_eq!(text(&painter.frame(canvas.clone())), "\x1b[1;1H日");
        canvas.put(1, 0, "c", Style::PLAIN);
        assert_eq!(text(&painter.frame(canvas.clone())), "\x1b[1;1H c");
    }

    #[test]
    fn a_new_size_or_a_lent_terminal_sends_it_all() {
        let mut painter = Painter::default();
        let canvas = Canvas::new(4, 1);
        painter.frame(canvas.clone());
        assert!(text(&painter.frame(Canvas::new(5, 1))).starts_with("\x1b[0m\x1b[2J"));
        painter.forget();
        assert!(text(&painter.frame(canvas)).starts_with("\x1b[0m\x1b[2J"));
    }

    static RESTORED: AtomicBool = AtomicBool::new(false);

    #[test]
    fn a_panic_puts_the_terminal_back_before_its_message() {
        let guard = PanicGuard::install(|| RESTORED.store(true, Ordering::SeqCst));
        let _ = panic::catch_unwind(|| panic!("essai"));
        assert!(RESTORED.load(Ordering::SeqCst));
        drop(guard);
        RESTORED.store(false, Ordering::SeqCst);
        let _ = panic::catch_unwind(|| panic!("essai, la garde relâchée"));
        assert!(!RESTORED.load(Ordering::SeqCst), "the hook from before is back");
    }
}
