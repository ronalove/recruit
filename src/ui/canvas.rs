// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A whole screen drawn in the terminal's cells, for the menu and the multiplexer: each frame is drawn into a
//! [`Canvas`], then [`Painter`] sends only the cells that changed since the frame before, in one write.
//!
//! No frame relies on synchronized updates, which not every terminal keeps: each goes out in a single write. A terminal
//! that has them ([`Features::sync`]) gets each frame framed by BSU and ESU as well.

mod painter;
#[cfg(test)]
mod tests;
mod text;

use crossterm::style::Color;

pub(crate) use painter::Painter;
pub(crate) use text::{columns, fit, is_direction_mark};
use text::{detached, left_out, width};

/// How a cell is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub reverse: bool,
    pub underline: bool,
    pub italic: bool,
    pub strike: bool,
    /// An OSC 8 link, from [`Canvas::link`] of the same canvas.
    pub link: Option<LinkId>,
}

/// A link of a canvas (OSC 8), what [`Canvas::link`] gives for its URI. It means something in that canvas only: two
/// frames are compared by their links' URIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LinkId(pub u32);

/// The cursor a frame shows, where the program of the focused pane left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub x: usize,
    pub y: usize,
    pub shape: CursorShape,
    pub blink: bool,
}

/// DECSCUSR's shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CursorShape {
    /// The one the user set in their terminal (DECSCUSR 0): never overridden unless the program asks.
    #[default]
    Default,
    Block,
    Underline,
    Bar,
}

/// What the terminal a painter writes to can do beyond what they all do. By default: 24-bit colors, neither
/// synchronized updates nor links.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Features {
    /// Mode 2026: each frame framed by BSU and ESU.
    pub sync: bool,
    /// 24-bit colors; otherwise, the nearest of the 256.
    pub truecolor: bool,
    /// OSC 8; otherwise, links are left out and their text stays.
    pub links: bool,
    /// Grapheme clusters measured whole (mode 2027 on); otherwise, one of several characters is cut down to what
    /// fits its cells measured character by character (👍🏽 to 👍, a ZWJ family to its first member): the line stays
    /// aligned.
    pub graphemes: bool,
}

impl Default for Features {
    fn default() -> Self {
        Features { sync: false, truecolor: true, links: false, graphemes: false }
    }
}

impl Style {
    pub(crate) const PLAIN: Style = Style {
        fg: None,
        bg: None,
        bold: false,
        dim: false,
        reverse: false,
        underline: false,
        italic: false,
        strike: false,
        link: None,
    };

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

/// What a cell shows: one character, the common case, or a grapheme of several (a letter and its accents, an emoji
/// sequence), `len` bytes of its canvas's `clusters` from `at`. A cell stays small: it is the drawing path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glyph {
    Char(char),
    Cluster { at: u32, len: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    glyph: Glyph,
    style: Style,
    wide: Wide,
}

impl Cell {
    fn blank(style: Style) -> Cell {
        Cell { glyph: Glyph::Char(' '), style, wide: Wide::One }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell::blank(Style::PLAIN)
    }
}

/// Links a canvas keeps before [`Canvas::compact`] keeps only those shown.
const LINKS: usize = 256;

/// Bytes a grapheme keeps at most: enough for the longest emoji sequences, not for a pile of accents.
const CLUSTER: usize = 64;

impl Default for Canvas {
    fn default() -> Self {
        Canvas::new(0, 0)
    }
}

/// A screen's worth of cells. Text is measured in columns (unicode-width), one character after the other. A
/// character of no width goes with the one before it in its cell (an accent), unless it would join that one to the
/// next or change its width from one terminal to another (ZWJ, variation selectors, format characters): those are
/// left out, so that what a cell shows takes the columns it was given everywhere. The panes' cells come whole from
/// their engine ([`Canvas::put_cell`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Canvas {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
    /// Shown at the end of the frame; hidden when `None`.
    cursor: Option<Cursor>,
    /// URIs of the links, by [`LinkId`].
    links: Vec<String>,
    /// The text of the graphemes of more than one character, one after the other.
    clusters: String,
    /// What changed in each row, by the clock: what a painter that sent this canvas before compares again.
    damage: Vec<Damage>,
    /// Advanced at each frame drawn in a canvas kept from one to the next ([`Canvas::advance`]).
    clock: u64,
    /// The cells and stores this canvas descends from: a new one at each start from scratch (`new`, `reset`) and
    /// each renumbering of its stores (`compact`). A painter compares by row only within one.
    lineage: u64,
}

/// What changed in a row: at which clock last, at which before it, and the columns changed at the last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Damage {
    at: u64,
    before: u64,
    from: u32,
    to: u32,
}

/// The lineages of the canvases of this process, one after the other.
static LINEAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn lineage() -> u64 {
    LINEAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Canvas {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        Canvas {
            width,
            height,
            cells: vec![Cell::default(); width * height],
            cursor: None,
            links: Vec::new(),
            clusters: String::new(),
            damage: vec![Damage::default(); height],
            clock: 1,
            lineage: lineage(),
        }
    }

    /// The next frame, drawn over this one: what it changes is told apart from what the frames before changed.
    pub(crate) fn advance(&mut self) {
        self.clock += 1;
    }

    /// Columns `from..to` of row `y` changed.
    fn touch(&mut self, y: usize, from: usize, to: usize) {
        let clock = self.clock;
        if let Some(damage) = self.damage.get_mut(y) {
            if damage.at == clock {
                damage.from = damage.from.min(from as u32);
                damage.to = damage.to.max(to as u32);
            } else {
                *damage = Damage { at: clock, before: damage.at, from: from as u32, to: to as u32 };
            }
        }
    }

    /// Blank again, `width` × `height`, in the buffers it has.
    pub(crate) fn reset(&mut self, width: usize, height: usize) {
        (self.width, self.height) = (width, height);
        self.cells.clear();
        self.cells.resize(width * height, Cell::default());
        self.cursor = None;
        self.links.clear();
        self.clusters.clear();
        self.damage.clear();
        self.damage.resize(height, Damage::default());
        self.lineage = lineage();
    }

    /// The graphemes of more than one character still shown, alone in their store, once it outgrows `room` bytes;
    /// and the links still shown, once there are more than [`LINKS`]: a canvas kept from one frame to the next
    /// stores each grapheme written, however often, and each link ever shown.
    pub(crate) fn compact(&mut self, room: usize) {
        if self.clusters.len() > room {
            let mut kept = String::new();
            let clusters = &self.clusters;
            for cell in &mut self.cells {
                if let Glyph::Cluster { at, len } = cell.glyph {
                    let at_now = kept.len() as u32;
                    kept.push_str(&clusters[at as usize..at as usize + len as usize]);
                    cell.glyph = Glyph::Cluster { at: at_now, len };
                }
            }
            self.clusters = kept;
            self.lineage = lineage();
        }
        if self.links.len() > LINKS {
            // Old number to new, for the links still shown.
            let mut renumbered: Vec<Option<u32>> = vec![None; self.links.len()];
            let mut kept: Vec<String> = Vec::new();
            for cell in &mut self.cells {
                let Some(LinkId(old)) = cell.style.link else { continue };
                let new = match renumbered.get(old as usize).copied().flatten() {
                    Some(new) => new,
                    None => {
                        kept.push(std::mem::take(&mut self.links[old as usize]));
                        let new = kept.len() as u32 - 1;
                        renumbered[old as usize] = Some(new);
                        new
                    }
                };
                cell.style.link = Some(LinkId(new));
            }
            self.links = kept;
            self.lineage = lineage();
        }
    }

    /// Sets the cell at `x`, `y` to `grapheme`, `width` columns wide (1 or 2) as the engine that measured it says;
    /// a wide one takes the cell after it too. For the panes: their program measured its text, and the canvas does
    /// not measure it again. What could show it otherwise is left out ([`left_out`]), and a ZWJ at its end, which
    /// would join it to the next cell in a terminal that groups graphemes (an engine that measures character by
    /// character puts a ZWJ with the character before it); nothing left, the cell is blank.
    pub(crate) fn put_cell(&mut self, x: usize, y: usize, grapheme: &str, width: usize, style: Style) {
        let glyph = if grapheme.ends_with('\u{200d}') || grapheme.chars().any(|c| left_out(c, width)) {
            let mut clean: String = grapheme.chars().filter(|&c| !left_out(c, width)).collect();
            clean.truncate(clean.trim_end_matches('\u{200d}').len());
            self.glyph(&clean)
        } else {
            self.glyph(grapheme)
        };
        if width < 2 {
            self.set(x, y, Cell { glyph, style, wide: Wide::One });
        } else if x + 1 < self.width {
            self.set(x, y, Cell { glyph, style, wide: Wide::Lead });
            self.set(x + 1, y, Cell { glyph: Glyph::Char(' '), style, wide: Wide::Tail });
        } else {
            // In the last column, the terminal would wrap it to the next row.
            self.set(x, y, Cell::blank(style));
        }
    }

    /// The cursor the frame shows, or none.
    pub(crate) fn set_cursor(&mut self, cursor: Option<Cursor>) {
        self.cursor = cursor;
    }

    /// The link to give cells for `uri` (OSC 8): the same for the same URI in this canvas.
    /// Called for each cell of a link: the last URI given is looked at first, and a screen holds few links.
    pub(crate) fn link(&mut self, uri: &str) -> LinkId {
        if self.links.last().is_some_and(|last| last == uri) {
            return LinkId(self.links.len() as u32 - 1);
        }
        let at = self.links.iter().rposition(|known| known == uri).unwrap_or_else(|| {
            self.links.push(uri.to_string());
            self.links.len() - 1
        });
        LinkId(at as u32)
    }

    /// Becomes a copy of `other`, in the buffers it has: no allocation once they are large enough.
    fn copy_from(&mut self, other: &Canvas) {
        self.width = other.width;
        self.height = other.height;
        self.cells.clone_from(&other.cells);
        self.cursor = other.cursor;
        self.links.clone_from(&other.links);
        self.clusters.clone_from(&other.clusters);
        self.damage.clone_from(&other.damage);
        (self.clock, self.lineage) = (other.clock, other.lineage);
    }

    /// Becomes `other` again, a copy of an earlier frame of the same lineage: only the columns `spans` gives for each
    /// row are copied (`None`: the row is as it was), and what the stores gained since.
    fn catch_up(&mut self, other: &Canvas, spans: impl Fn(usize) -> Option<(usize, usize)>) {
        for y in 0..other.height {
            if let Some((from, to)) = spans(y) {
                let at = y * other.width;
                self.cells[at + from..at + to].copy_from_slice(&other.cells[at + from..at + to]);
            }
        }
        self.cursor = other.cursor;
        // The stores only grow within a lineage.
        let links = self.links.len();
        self.links.extend(other.links[links.min(other.links.len())..].iter().cloned());
        let clusters = self.clusters.len();
        self.clusters.push_str(other.clusters.get(clusters..).unwrap_or(""));
        self.damage.clone_from(&other.damage);
        self.clock = other.clock;
    }

    fn uri(&self, link: LinkId) -> &str {
        self.links.get(link.0 as usize).map_or("", String::as_str)
    }

    /// The glyph for `text`, its first [`CLUSTER`] bytes; a blank for nothing.
    fn glyph(&mut self, text: &str) -> Glyph {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (None, _) => Glyph::Char(' '),
            (Some(c), None) => Glyph::Char(c),
            _ => {
                let mut len = text.len().min(CLUSTER);
                while !text.is_char_boundary(len) {
                    len -= 1;
                }
                let at = self.clusters.len() as u32;
                self.clusters.push_str(&text[..len]);
                Glyph::Cluster { at, len: len as u16 }
            }
        }
    }

    /// What `glyph` shows, `buf` holding it when it is one character.
    fn text<'a>(&'a self, glyph: Glyph, buf: &'a mut [u8; 4]) -> &'a str {
        match glyph {
            Glyph::Char(c) => c.encode_utf8(buf),
            Glyph::Cluster { at, len } => &self.clusters[at as usize..at as usize + len as usize],
        }
    }

    /// Adds `mark`, of no width, to what the cell at `i` shows, unless it holds its [`CLUSTER`] bytes already.
    fn attach(&mut self, i: usize, mark: char) {
        let mut buf = [0; 4];
        let shown = self.text(self.cells[i].glyph, &mut buf);
        if shown.len() + mark.len_utf8() > CLUSTER {
            return;
        }
        let mut grapheme = shown.to_string();
        grapheme.push(mark);
        self.cells[i].glyph = self.glyph(&grapheme);
        let (x, y) = (i % self.width, i / self.width);
        self.touch(y, x, x + 1);
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }

    pub(crate) fn height(&self) -> usize {
        self.height
    }

    fn at(&self, x: usize, y: usize) -> Option<usize> {
        (x < self.width && y < self.height).then(|| y * self.width + x)
    }

    /// Sets a cell, and blanks what remains of a wide character it cuts in two.
    fn set(&mut self, x: usize, y: usize, cell: Cell) {
        let Some(i) = self.at(x, y) else { return };
        // Written again as it is: nothing changed, nothing for the painters to compare.
        if self.cells[i] == cell {
            return;
        }
        self.touch(y, x.saturating_sub(1), (x + 2).min(self.width));
        match self.cells[i].wide {
            Wide::Lead if cell.wide != Wide::Lead => {
                if let Some(next) = self.at(x + 1, y) {
                    self.cells[next] = Cell { glyph: Glyph::Char(' '), wide: Wide::One, ..self.cells[next] };
                }
            }
            // The second half of another wide character in its place: the first half is the new one's.
            Wide::Tail if x > 0 && cell.wide != Wide::Tail => {
                let before = i - 1;
                self.cells[before] = Cell { glyph: Glyph::Char(' '), wide: Wide::One, ..self.cells[before] };
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
        // The cell the last character went into, for the accents after it.
        let mut last = None;
        for c in text.chars() {
            // A control character: nothing to show.
            let Some(w) = width(c) else { continue };
            if w == 0 {
                if let Some(i) = last
                    && !detached(c)
                {
                    self.attach(i, c);
                }
                continue;
            }
            if x + w > end {
                break;
            }
            if w == 2 {
                self.set(x, y, Cell { glyph: Glyph::Char(c), style, wide: Wide::Lead });
                self.set(x + 1, y, Cell { glyph: Glyph::Char(' '), style, wide: Wide::Tail });
            } else {
                self.set(x, y, Cell { glyph: Glyph::Char(c), style, wide: Wide::One });
            }
            last = self.at(x, y);
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
        let end = (x + width).min(self.width);
        if x >= end {
            return;
        }
        for row in y..(y + height).min(self.height) {
            // The edges cell by cell, for the wide characters they cut; the rest at once, the panes' path.
            self.set(x, row, Cell::blank(style));
            self.set(end - 1, row, Cell::blank(style));
            if end - x > 2 {
                let at = row * self.width;
                let blank = Cell::blank(style);
                let mut changed: Option<(usize, usize)> = None;
                for (col, cell) in self.cells[at + x + 1..at + end - 1].iter_mut().enumerate() {
                    if *cell != blank {
                        *cell = blank;
                        let col = x + 1 + col;
                        changed = Some(changed.map_or((col, col + 1), |(from, _)| (from, col + 1)));
                    }
                }
                if let Some((from, to)) = changed {
                    self.touch(row, from, to);
                }
            }
        }
    }

    /// Puts `bg` behind `width` cells of row `y`, keeping what they show.
    pub(crate) fn tint(&mut self, x: usize, y: usize, width: usize, bg: Color) {
        let end = (x + width).min(self.width);
        for col in x..end {
            if let Some(i) = self.at(col, y) {
                self.cells[i].style.bg = Some(bg);
            }
        }
        if x < end {
            self.touch(y, x, end);
        }
    }

    /// Shows `width` cells of row `y` in reverse video, or as they were: a selection.
    pub(crate) fn invert(&mut self, x: usize, y: usize, width: usize) {
        let end = (x + width).min(self.width);
        for col in x..end {
            if let Some(i) = self.at(col, y) {
                self.cells[i].style.reverse = !self.cells[i].style.reverse;
            }
        }
        if x < end && y < self.height {
            self.touch(y, x, end);
        }
    }

    /// A line of `ch` from `x`, `width` long.
    pub(crate) fn hline(&mut self, x: usize, y: usize, width: usize, ch: char, style: Style) {
        for col in x..(x + width).min(self.width) {
            self.set(col, y, Cell { glyph: Glyph::Char(ch), style, wide: Wide::One });
        }
    }

    /// A column of `ch` from `y`, `height` long.
    pub(crate) fn vline(&mut self, x: usize, y: usize, height: usize, ch: char, style: Style) {
        for row in y..(y + height).min(self.height) {
            self.set(x, row, Cell { glyph: Glyph::Char(ch), style, wide: Wide::One });
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
            self.set(col, row, Cell { glyph: Glyph::Char(corner), style, wide: Wide::One });
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
                if self.cells[i].style != faded {
                    self.cells[i].style = faded;
                    self.touch(row, col, col + 1);
                }
            }
        }
    }

    /// Row `y`'s text, its wide characters once, without the blanks at its end: for tests, and the server's captures.
    pub(crate) fn row(&self, y: usize) -> String {
        let mut row = String::new();
        for cell in &self.cells[y * self.width..(y + 1) * self.width] {
            if cell.wide != Wide::Tail {
                let mut buf = [0; 4];
                row.push_str(self.text(cell.glyph, &mut buf));
            }
        }
        row.truncate(row.trim_end().len());
        row
    }

    /// What the cell at `x`, `y` shows; nothing for the second column of a wide character.
    #[cfg(test)]
    pub(crate) fn cell(&self, x: usize, y: usize) -> String {
        let cell = self.cells[y * self.width + x];
        if cell.wide == Wide::Tail {
            return String::new();
        }
        self.text(cell.glyph, &mut [0; 4]).to_string()
    }

    /// The style of the cell at `x`, `y`.
    #[cfg(test)]
    pub(crate) fn style(&self, x: usize, y: usize) -> Style {
        self.cells[y * self.width + x].style
    }

    /// The URI of the cell at `x`, `y`'s link.
    #[cfg(test)]
    pub(crate) fn link_at(&self, x: usize, y: usize) -> Option<&str> {
        self.cells[y * self.width + x].style.link.map(|link| self.uri(link))
    }

    pub(crate) fn cursor(&self) -> Option<Cursor> {
        self.cursor
    }
}

/// The random writes of the tests that compare a painter with a whole redraw, here and in the screen's: a xorshift, so
/// that a failing run comes back with its seed.
#[cfg(test)]
pub(crate) mod rng {
    pub(crate) struct Rng(pub(crate) u64);

    impl Rng {
        pub(crate) fn n(&mut self, m: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % m as u64) as usize
        }
    }
}
