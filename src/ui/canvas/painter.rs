// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The frames sent by difference with the one on screen.

use std::io::Write;

use crossterm::style::Color;
use unicode_width::UnicodeWidthChar;

#[cfg(test)]
mod tests;

use super::{Canvas, Cell, Cursor, CursorShape, Features, Glyph, Style, Wide};

/// Whether cell `a` of `canvas` shows what cell `b` of `other` does: the same text, style and link, the links by
/// their URIs and the graphemes by their text, since each canvas numbers its own.
fn same(canvas: &Canvas, a: &Cell, other: &Canvas, b: &Cell) -> bool {
    let plain = |c: &Cell| matches!(c.glyph, Glyph::Char(_)) && c.style.link.is_none();
    if plain(a) && plain(b) {
        return a == b;
    }
    let unlinked = |c: &Cell| Style { link: None, ..c.style };
    if a.wide != b.wide || unlinked(a) != unlinked(b) {
        return false;
    }
    let links = match (a.style.link, b.style.link) {
        (None, None) => true,
        (Some(x), Some(y)) => canvas.uri(x) == other.uri(y),
        _ => false,
    };
    let (mut x, mut y) = ([0; 4], [0; 4]);
    links && canvas.text(a.glyph, &mut x) == other.text(b.glyph, &mut y)
}

/// Sends frames by difference with the one on screen.
#[derive(Default)]
pub(crate) struct Painter {
    shown: Option<Canvas>,
    /// The terminal's current style, when known. Its link is one of the frame being sent: none is left open between
    /// two frames.
    style: Option<Style>,
    features: Features,
    /// Whether the terminal shows its cursor: hidden at first, as the screens that use a painter set it up
    /// ([`Session`], `mux::input::Terminal`).
    visible: bool,
    /// The cursor's shape, when the painter set it.
    shape: Option<(CursorShape, bool)>,
}

/// Unchanged cells this close between two changed ones, in the same style, are written again rather than skipped.
const GAP: usize = 4;

/// Blanks in a row this many or more are erased (ECH, `CSI n X`: 4 to 6 bytes, and a move after it of 6 to 8) rather
/// than written.
const ERASE: usize = 10;

/// Whether a cell erased shows as `cell` does: a blank on the terminal's own background, with nothing that shows on a
/// blank (reverse video, underline, strike, a link). Erased cells take the current background (BCE) in some
/// terminals and the default one in others: only the default one is the same in all.
fn erasable(cell: &Cell, style: Style) -> bool {
    cell.glyph == Glyph::Char(' ')
        && cell.wide == Wide::One
        && style.bg.is_none()
        && !style.reverse
        && !style.underline
        && !style.strike
        && style.link.is_none()
}

/// The longest URI a link keeps; beyond, its text stays without it.
const URI: usize = 4096;

impl Painter {
    /// A painter for a terminal that can do `features`.
    pub(crate) fn new(features: Features) -> Painter {
        Painter { features, ..Painter::default() }
    }

    /// What is on screen is unknown (another program wrote on it): the next frame is sent whole.
    pub(crate) fn forget(&mut self) {
        *self = Painter::new(self.features);
    }

    /// The bytes that turn the screen into `next`: nothing when it shows what is on screen already. The painter keeps
    /// its own copy of what it sent, in the same buffers from one frame to the next: a canvas can be painted for
    /// several terminals.
    pub(crate) fn frame(&mut self, next: &Canvas) -> Vec<u8> {
        let shown = self.shown.take();
        let whole = shown.as_ref().is_none_or(|shown| (shown.width, shown.height) != (next.width, next.height));
        let mut cells = Vec::new();
        if whole {
            cells.extend_from_slice(b"\x1b[0m\x1b[2J");
            self.style = Some(Style::PLAIN);
        }
        // A later frame of the canvas sent before: only what changed since in each row is compared.
        // Only for a canvas advanced since (`Canvas::advance`): one written again in place, without, is compared whole.
        let since = shown
            .as_ref()
            .filter(|shown| !whole && shown.lineage == next.lineage && shown.clock < next.clock)
            .map(|shown| shown.clock);
        let spans = |y: usize| match since {
            None => Some((0, next.width)),
            Some(since) => {
                let damage = next.damage[y];
                if damage.at <= since {
                    None
                } else if damage.before <= since {
                    // From the first half of a wide character whose second half changed: it is drawn from there.
                    let mut from = (damage.from as usize).min(next.width);
                    if from > 0 && from < next.width && next.cells[y * next.width + from].wide == Wide::Tail {
                        from -= 1;
                    }
                    Some((from, (damage.to as usize).min(next.width)))
                } else {
                    Some((0, next.width))
                }
            }
        };
        self.cells(&mut cells, next, shown.as_ref().filter(|_| !whole), &spans);

        let mut out = Vec::with_capacity(cells.len() + 32);
        if self.features.sync {
            out.extend_from_slice(b"\x1b[?2026h");
        } else if !cells.is_empty() && self.visible {
            // Without synchronized updates, the cursor would be seen running through the changes.
            out.extend_from_slice(b"\x1b[?25l");
            self.visible = false;
        }
        let drew = !cells.is_empty();
        out.extend_from_slice(&cells);
        self.cursor(&mut out, next, shown.as_ref().and_then(|shown| shown.cursor), drew);
        if self.features.sync {
            if out.len() == b"\x1b[?2026h".len() {
                out.clear();
            } else {
                out.extend_from_slice(b"\x1b[?2026l");
            }
        }
        let mut kept = shown.unwrap_or_else(|| Canvas::new(0, 0));
        if since.is_some() {
            kept.catch_up(next, spans);
        } else {
            kept.copy_from(next);
        }
        self.shown = Some(kept);
        out
    }

    /// The cells of `next` that differ from `shown` (all of them without it), looked for in the columns `spans` gives
    /// for each row (none: the row is as it was).
    fn cells(
        &mut self,
        out: &mut Vec<u8>,
        next: &Canvas,
        shown: Option<&Canvas>,
        spans: &dyn Fn(usize) -> Option<(usize, usize)>,
    ) {
        let mut at: Option<(usize, usize)> = None;
        for y in 0..next.height {
            let Some((from, to)) = spans(y) else { continue };
            let span = y * next.width..(y + 1) * next.width;
            let row = &next.cells[span.clone()];
            let old = shown.map(|shown| (shown, &shown.cells[span.clone()]));
            let changed = |x: usize| old.is_none_or(|(shown, old)| !same(next, &row[x], shown, &old[x]));
            let mut x = from;
            while x < to {
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
                // After a grapheme of several characters, the terminal may not have measured it as its engine did:
                // the next cell is placed rather than written where the cursor is.
                let mut lost = false;
                loop {
                    let c = row[end];
                    let mut step = 1;
                    if c.wide != Wide::Tail {
                        if lost {
                            let _ = write!(out, "\x1b[{};{}H", y + 1, end + 1);
                        }
                        let style = self.sendable(next, c.style);
                        self.sgr(out, style, next);
                        // Blanks on the default background, enough of them: erased (ECH) rather than written. The
                        // cursor stays: the next cell is placed.
                        let blanks = if erasable(&c, style) {
                            row[end..].iter().take_while(|other| **other == c).count()
                        } else {
                            0
                        };
                        if blanks >= ERASE {
                            let _ = write!(out, "\x1b[{blanks}X");
                            step = blanks;
                            lost = true;
                        } else {
                            let mut buf = [0; 4];
                            let text = next.text(c.glyph, &mut buf);
                            match c.glyph {
                                Glyph::Cluster { .. } if !self.features.graphemes => {
                                    narrowed(out, text, if c.wide == Wide::Lead { 2 } else { 1 })
                                }
                                _ => out.extend_from_slice(text.as_bytes()),
                            }
                            lost = matches!(c.glyph, Glyph::Cluster { .. });
                        }
                    }
                    end += step;
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
                            if row[end..i].iter().all(|c| {
                                c.wide == Wide::One
                                    && matches!(c.glyph, Glyph::Char(_))
                                    && Some(self.sendable(next, c.style)) == self.style
                            }) && row[end..i]
                                .iter()
                                .map(|c| match c.glyph {
                                    Glyph::Char(ch) => ch.len_utf8(),
                                    Glyph::Cluster { len, .. } => len as usize,
                                })
                                .sum::<usize>()
                                < 6 =>
                        {
                            continue;
                        }
                        _ => break,
                    }
                }
                at = (!lost).then_some((end, y));
                x = end;
            }
        }
        // No link stays open: the next frame numbers its own.
        if let Some(style) = self.style
            && style.link.is_some()
        {
            out.extend_from_slice(b"\x1b]8;;\x1b\\");
            self.style = Some(Style { link: None, ..style });
        }
    }

    /// `style` as this terminal gets it: without its link if it cannot have links, or if its URI could not go out
    /// as it is (a control character, which would end the sequence early).
    fn sendable(&self, canvas: &Canvas, style: Style) -> Style {
        match style.link {
            Some(link) if self.features.links => {
                let uri = canvas.uri(link);
                if uri.is_empty() || uri.len() > URI || uri.chars().any(char::is_control) {
                    Style { link: None, ..style }
                } else {
                    style
                }
            }
            Some(_) => Style { link: None, ..style },
            None => style,
        }
    }

    /// Shows `next`'s cursor, or hides it; `before` is where the frame on screen had it, `drew` whether this one
    /// moved the terminal's cursor since.
    fn cursor(&mut self, out: &mut Vec<u8>, next: &Canvas, before: Option<Cursor>, drew: bool) {
        match next.cursor.filter(|c| c.x < next.width && c.y < next.height) {
            None => {
                if self.visible {
                    out.extend_from_slice(b"\x1b[?25l");
                    self.visible = false;
                }
            }
            Some(c) => {
                if drew || !self.visible || before.is_none_or(|b| (b.x, b.y) != (c.x, c.y)) {
                    let _ = write!(out, "\x1b[{};{}H", c.y + 1, c.x + 1);
                }
                if self.shape != Some((c.shape, c.blink)) {
                    let steady = usize::from(!c.blink);
                    let code = match c.shape {
                        CursorShape::Default => 0,
                        CursorShape::Block => 1 + steady,
                        CursorShape::Underline => 3 + steady,
                        CursorShape::Bar => 5 + steady,
                    };
                    let _ = write!(out, "\x1b[{code} q");
                    self.shape = Some((c.shape, c.blink));
                }
                if !self.visible {
                    out.extend_from_slice(b"\x1b[?25h");
                    self.visible = true;
                }
            }
        }
    }

    /// Switches the terminal to `style`, saying only what changes; its link is one of `canvas`.
    fn sgr(&mut self, out: &mut Vec<u8>, style: Style, canvas: &Canvas) {
        if self.style == Some(style) {
            return;
        }
        let link = self.style.and_then(|old| old.link);
        // Written as they come, no allocation: the drawing's path.
        let mut params = Params { out, any: false };
        let from = match self.style {
            // An attribute to turn off: from scratch.
            Some(old)
                if !(old.bold && !style.bold
                    || old.dim && !style.dim
                    || old.reverse && !style.reverse
                    || old.underline && !style.underline
                    || old.italic && !style.italic
                    || old.strike && !style.strike) =>
            {
                old
            }
            _ => {
                params.number(0);
                Style::PLAIN
            }
        };
        for (on, was, code) in [
            (style.bold, from.bold, 1),
            (style.dim, from.dim, 2),
            (style.italic, from.italic, 3),
            (style.underline, from.underline, 4),
            (style.reverse, from.reverse, 7),
            (style.strike, from.strike, 9),
        ] {
            if on && !was {
                params.number(code);
            }
        }
        let truecolor = self.features.truecolor;
        if style.fg != from.fg {
            match style.fg {
                Some(c) => color(&mut params, c, false, truecolor),
                None => params.number(39),
            }
        }
        if style.bg != from.bg {
            match style.bg {
                Some(c) => color(&mut params, c, true, truecolor),
                None => params.number(49),
            }
        }
        let out = params.end();
        // SGR 0 leaves links alone.
        if style.link != link {
            match style.link {
                Some(link) => {
                    let _ = write!(out, "\x1b]8;;{}\x1b\\", canvas.uri(link));
                }
                None => out.extend_from_slice(b"\x1b]8;;\x1b\\"),
            }
        }
        self.style = Some(style);
    }
}

/// `grapheme` for a terminal that measures it character by character, in `width` columns: its characters as long as
/// they fit, its accents with them, up to a joiner (which would join the next one); blanks for the columns left.
fn narrowed(out: &mut Vec<u8>, grapheme: &str, width: usize) {
    let mut used = 0;
    let mut buf = [0; 4];
    for c in grapheme.chars() {
        let w = c.width().unwrap_or(0);
        if c == '\u{200d}' || used + w > width {
            break;
        }
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        used += w;
    }
    out.extend(std::iter::repeat_n(b' ', width.saturating_sub(used)));
}

/// An SGR sequence written as its parameters come: `ESC [` before the first, `;` between them, `m` after the last;
/// nothing without any.
struct Params<'a> {
    out: &'a mut Vec<u8>,
    any: bool,
}

impl<'a> Params<'a> {
    fn number(&mut self, n: u8) {
        self.out.extend_from_slice(if self.any { b";" } else { b"\x1b[" });
        self.any = true;
        decimal(self.out, n);
    }

    /// Several numbers, one parameter each: `38;2;r;g;b`.
    fn numbers(&mut self, numbers: &[u8]) {
        for &n in numbers {
            self.number(n);
        }
    }

    fn end(self) -> &'a mut Vec<u8> {
        if self.any {
            self.out.push(b'm');
        }
        self.out
    }
}

/// `n` in decimal, without the formatting machinery.
fn decimal(out: &mut Vec<u8>, n: u8) {
    if n >= 100 {
        out.push(b'0' + n / 100);
    }
    if n >= 10 {
        out.push(b'0' + n / 10 % 10);
    }
    out.push(b'0' + n % 10);
}

/// A color's SGR parameters, the sixteen named ones by their short codes; a 24-bit one brought down to the nearest
/// of the 256 for a terminal without them.
fn color(params: &mut Params<'_>, color: Color, bg: bool, truecolor: bool) {
    let base = |code: u8| if bg { code + 10 } else { code };
    let (extended, indexed) = if bg { (48, 5) } else { (38, 5) };
    match color {
        Color::Reset => params.number(base(39)),
        Color::Black => params.number(base(30)),
        Color::DarkRed => params.number(base(31)),
        Color::DarkGreen => params.number(base(32)),
        Color::DarkYellow => params.number(base(33)),
        Color::DarkBlue => params.number(base(34)),
        Color::DarkMagenta => params.number(base(35)),
        Color::DarkCyan => params.number(base(36)),
        Color::Grey => params.number(base(37)),
        Color::DarkGrey => params.number(base(90)),
        Color::Red => params.number(base(91)),
        Color::Green => params.number(base(92)),
        Color::Yellow => params.number(base(93)),
        Color::Blue => params.number(base(94)),
        Color::Magenta => params.number(base(95)),
        Color::Cyan => params.number(base(96)),
        Color::White => params.number(base(97)),
        Color::AnsiValue(n) => params.numbers(&[extended, indexed, n]),
        Color::Rgb { r, g, b } if !truecolor => params.numbers(&[extended, indexed, ansi256(r, g, b)]),
        Color::Rgb { r, g, b } => params.numbers(&[extended, 2, r, g, b]),
    }
}

/// The nearest of the 256 colors: in the 6×6×6 cube, or on the grey ramp when closer.
fn ansi256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    let step = |v: u8| match v {
        0..48 => 0,
        48..115 => 1,
        _ => (v - 35) / 40,
    };
    let (qr, qg, qb) = (step(r), step(g), step(b));
    let cube = 16 + 36 * qr + 6 * qg + qb;
    let (cr, cg, cb) = (LEVELS[qr as usize], LEVELS[qg as usize], LEVELS[qb as usize]);
    if (cr, cg, cb) == (r, g, b) {
        return cube;
    }
    let average = (r as u32 + g as u32 + b as u32) / 3;
    let grey_at = if average > 238 { 23 } else { average.saturating_sub(3) / 10 };
    let grey = (8 + 10 * grey_at) as u8;
    let distance = |x: u8, y: u8, z: u8| {
        let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    if distance(grey, grey, grey) < distance(cr, cg, cb) { 232 + grey_at as u8 } else { cube }
}
