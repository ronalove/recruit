// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A whole screen drawn in the terminal's cells, for the menu and the multiplexer: each frame is drawn into a
//! [`Canvas`], then [`Painter`] sends only the cells that changed since the frame before, in one write. And the
//! terminal while such a screen is on it ([`Session`]), given back as it was however the program ends, a panic
//! included ([`PanicGuard`]).
//!
//! No frame relies on synchronized updates, which not every terminal keeps (tmux drops them in a popup): each goes out
//! in a single write. A terminal that has them ([`Features::sync`]) gets each frame framed by BSU and ESU as well.

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

/// What the terminal a painter writes to can do beyond what they all do. By default, what the menu has always
/// assumed: 24-bit colors, neither synchronized updates nor links.
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

/// Columns of `text` as a canvas draws it.
pub(crate) fn columns(text: &str) -> usize {
    text.chars().map(|c| width(c).unwrap_or(0)).sum()
}

/// Columns of `c` as a canvas draws it: none for what it leaves out ([`detached`]), some of which unicode-width
/// gives one; `None` for a control character.
fn width(c: char) -> Option<usize> {
    if detached(c) { Some(0) } else { c.width() }
}

/// `text` in `room` columns at most, with « … » when it is cut.
pub(crate) fn fit(text: &str, room: usize) -> String {
    if columns(text) <= room {
        return text.to_string();
    }
    let mut cut = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = width(c).unwrap_or(0);
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

/// The characters of no width that [`Canvas::put`] leaves out rather than add to the character before: joiners and
/// non-joiners, variation selectors, direction marks and controls, invisible operators, format characters and tags.
fn detached(c: char) -> bool {
    bidi(c)
        || matches!(c,
            '\u{ad}'
            | '\u{61c}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2060}'..='\u{2064}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e007f}'
            | '\u{e0100}'..='\u{e01ef}')
}

/// The controls of the writing direction: a terminal that follows them would reorder what a pane shows.
fn bidi(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// What [`Canvas::put_cell`] leaves out of a grapheme `width` columns wide: control characters and direction
/// controls; VS16 when it is one column wide, since the terminals that heed it draw the character on two.
fn left_out(c: char, width: usize) -> bool {
    c.is_control() || bidi(c) || (c == '\u{fe0f}' && width < 2)
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

/// The nearest of the 256 colors: in the 6×6×6 cube, or on the grey ramp when closer (as tmux does).
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
        // An accent goes with the letter before it; a joiner or a variation selector is left out, and an accent
        // with nothing before it too.
        let mut canvas = Canvas::new(10, 1);
        assert_eq!(canvas.put(0, 0, "\u{301}e\u{301}\u{200d}x\u{fe0f}", Style::PLAIN), 2);
        assert_eq!(canvas.row(0), "e\u{301}x");
        assert_eq!(columns("e\u{301}\u{200d}x\u{fe0f}"), 2);
        // Format characters left out as well, direction controls among them.
        let mut canvas = Canvas::new(10, 1);
        canvas.put(0, 0, "a\u{61c}\u{fff9}\u{1d173}\u{13430}\u{202e}\u{2066}b", Style::PLAIN);
        assert_eq!(canvas.row(0), "ab");
        assert!(canvas.clusters.is_empty());
        assert_eq!(columns("a\u{fff9}\u{ad}b"), 2, "measured as drawn");
        // A pile of accents: the cell full, nothing more kept.
        let mut canvas = Canvas::new(2, 1);
        canvas.put(0, 0, &format!("e{}", "\u{301}".repeat(500)), Style::PLAIN);
        assert_eq!(canvas.row(0), format!("e{}", "\u{301}".repeat(31)));
        assert!(canvas.clusters.len() < 2 * CLUSTER * CLUSTER, "{}", canvas.clusters.len());
    }

    #[test]
    fn a_canvas_kept_from_frame_to_frame() {
        let mut canvas = Canvas::new(8, 2);
        // A fill whose edges cut wide characters: their other halves go blank.
        canvas.put(0, 0, "日本語x", Style::PLAIN);
        canvas.fill(1, 0, 4, 1, Style::PLAIN);
        assert_eq!(canvas.row(0), "      x");
        canvas.put(0, 1, "abcdefgh", Style::PLAIN);
        canvas.fill(2, 1, 3, 1, Style::PLAIN);
        assert_eq!(canvas.row(1), "ab   fgh");
        // Graphemes written again and again: their store keeps the ones shown only, once compacted.
        let family = "\u{1f468}\u{200d}\u{1f469}";
        for _ in 0..100 {
            canvas.put_cell(0, 0, family, 2, Style::PLAIN);
        }
        let stored = canvas.clusters.len();
        canvas.compact(stored);
        assert_eq!(canvas.clusters.len(), stored, "within its room: left alone");
        canvas.compact(0);
        assert_eq!(canvas.clusters, family);
        assert!(canvas.row(0).starts_with(family));
        // Links ever shown: past LINKS, only those still shown stay, renumbered.
        let mut canvas = Canvas::new(2, 1);
        for i in 0..=LINKS {
            let link = canvas.link(&format!("https://e.example/{i}"));
            canvas.put(0, 0, "a", Style { link: Some(link), ..Style::PLAIN });
        }
        let first = canvas.link("https://e.example/0");
        canvas.put(1, 0, "b", Style { link: Some(first), ..Style::PLAIN });
        canvas.compact(usize::MAX);
        assert_eq!(canvas.links.len(), 2);
        assert_eq!(canvas.link_at(0, 0), Some(format!("https://e.example/{LINKS}").as_str()));
        assert_eq!(canvas.link_at(1, 0), Some("https://e.example/0"));
        // Blank again, at another size.
        canvas.reset(4, 1);
        assert_eq!((canvas.width(), canvas.height(), canvas.row(0)), (4, 1, String::new()));
        assert!(canvas.clusters.is_empty());
    }

    #[test]
    fn a_kept_canvas_painted_by_its_changes_only() {
        // Random writes on a canvas kept from frame to frame: a painter that compares the changed columns only sends
        // what one comparing everything sends, frames skipped included.
        struct Rng(u64);
        impl Rng {
            fn n(&mut self, m: usize) -> usize {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                (self.0 % m as u64) as usize
            }
        }
        // Run longer on other seeds while working on the painter: 20 000 rounds on six seeds passed.
        let mut r = Rng(0x2545_f491_4f6c_dd1d);
        let words = ["abc", "日本", "x", "  ", "──", "é\u{301}", "a日b"];
        let colors = [None, Some(Color::Cyan), Some(Color::AnsiValue(141)), Some(Color::Rgb { r: 215, g: 119, b: 87 })];
        let family = "\u{1f468}\u{200d}\u{1f469}";
        for _ in 0..500 {
            let (mut w, mut h) = (1 + r.n(30), 1 + r.n(6));
            let mut canvas = Canvas::new(w, h);
            let features =
                Features { sync: r.n(2) == 1, truecolor: r.n(2) == 1, links: r.n(4) != 0, graphemes: r.n(2) == 1 };
            let (mut by_changes, mut whole) = (Painter::new(features), Painter::new(features));
            let mut ops: Vec<String> = Vec::new();
            for _ in 0..14 {
                canvas.advance();
                ops.push("--".into());
                for _ in 0..r.n(6) {
                    let (x, y) = (r.n(w), r.n(h));
                    let style = Style { fg: colors[r.n(4)], bg: colors[r.n(4)], bold: r.n(2) == 1, ..Style::PLAIN };
                    let op = r.n(11);
                    ops.push(format!("{op} at {x},{y}"));
                    match op {
                        0 => canvas.fill(x, y, r.n(w) + 1, r.n(h) + 1, style),
                        1 => canvas.tint(x, y, r.n(w) + 1, Color::AnsiValue(236)),
                        2 => canvas.invert(x, y, r.n(w) + 1),
                        3 => canvas.fade_outside(x, y, r.n(w), r.n(h), Style::fg(Color::AnsiValue(239))),
                        4 => canvas.put_cell(x, y, family, 2, style),
                        5 => {
                            let link = canvas.link(["https://a.example", "https://b.example"][r.n(2)]);
                            canvas.put(x, y, words[r.n(words.len())], Style { link: Some(link), ..style });
                        }
                        6 => canvas.set_cursor(Some(Cursor { x, y, shape: CursorShape::Bar, blink: false })),
                        7 => canvas.compact(r.n(2) * 64),
                        // Links past LINKS, each to its own URI: the next compaction renumbers them.
                        8 => {
                            for i in 0..LINKS + 1 + r.n(32) {
                                let link = canvas.link(&format!("https://e.example/{}", r.n(1000) * 1000 + i));
                                let (x, y) = (r.n(w), r.n(h));
                                canvas.put(x, y, "l", Style { link: Some(link), ..style });
                            }
                            canvas.compact(usize::MAX);
                        }
                        // Blank again, at the same size or another.
                        9 => {
                            if r.n(2) == 0 {
                                (w, h) = (1 + r.n(30), 1 + r.n(6));
                            }
                            canvas.reset(w, h);
                        }
                        _ => {
                            canvas.put(x, y, words[r.n(words.len())], style);
                        }
                    }
                }
                // Some frames are not sent (a client busy): the next one carries their changes.
                if r.n(4) == 0 {
                    continue;
                }
                let mut copy = canvas.clone();
                copy.lineage = lineage();
                let (a, b) = (by_changes.frame(&canvas), whole.frame(&copy));
                assert_eq!(text(&a), text(&b), "{w}×{h} {ops:?}");
            }
        }
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
        let first = text(&painter.frame(&canvas));
        assert!(first.starts_with("\x1b[0m\x1b[2J"), "the first frame whole");
        assert!(painter.frame(&canvas).is_empty(), "nothing changed, nothing sent");

        canvas.put(4, 1, "x", Style::fg(Color::Cyan));
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;5H\x1b[96mx");
        // The same style again: no SGR. A short gap in another style: moved across.
        canvas.put(4, 1, "y", Style::fg(Color::Cyan));
        canvas.put(7, 1, "z", Style::fg(Color::Cyan));
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;5Hy\x1b[2;8Hz");
        // In the same style: written over.
        canvas.put(4, 1, "abcd", Style::fg(Color::Cyan));
        painter.frame(&canvas);
        canvas.put(4, 1, "e", Style::fg(Color::Cyan));
        canvas.put(7, 1, "f", Style::fg(Color::Cyan));
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;5Hebcf");
        // A long gap: moved across.
        canvas.put(0, 2, "a", Style::PLAIN);
        canvas.put(15, 2, "b", Style::PLAIN);
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[3;1H\x1b[39ma\x1b[3;16Hb");
    }

    #[test]
    fn styles_say_only_what_changes() {
        let mut painter = Painter::default();
        let mut out = Vec::new();
        let canvas = Canvas::new(0, 0);
        painter.style = Some(Style::PLAIN);
        painter.sgr(&mut out, Style::fg(Color::AnsiValue(141)).bold(), &canvas);
        painter.sgr(&mut out, Style::fg(Color::AnsiValue(141)).bold().on(Color::AnsiValue(236)), &canvas);
        painter.sgr(&mut out, Style::fg(Color::Yellow), &canvas);
        painter.sgr(&mut out, Style::fg(Color::Yellow), &canvas);
        assert_eq!(text(&out), "\x1b[1;38;5;141m\x1b[48;5;236m\x1b[0;93m");
        // Italic and struck through: on, then off from scratch.
        let mut out = Vec::new();
        painter.sgr(&mut out, Style { italic: true, strike: true, ..Style::fg(Color::Yellow) }, &canvas);
        painter.sgr(&mut out, Style { strike: true, ..Style::fg(Color::Yellow) }, &canvas);
        assert_eq!(text(&out), "\x1b[3;9m\x1b[0;9;93m");
    }

    /// A frame of one row, `cells` given as the engine would.
    fn pane_row(width: usize, cells: &[(&str, usize)]) -> Canvas {
        let mut canvas = Canvas::new(width, 1);
        let mut x = 0;
        for (grapheme, w) in cells {
            canvas.put_cell(x, 0, grapheme, *w, Style::PLAIN);
            x += w;
        }
        canvas
    }

    #[test]
    fn graphemes_go_whole() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        let canvas = pane_row(8, &[("a", 1), (family, 2), ("e\u{301}", 1), ("\x1bx", 1), ("\u{7}", 1)]);
        assert_eq!(canvas.row(0), format!("a{family}e\u{301}x"), "control characters left out");
        // Placed again after a grapheme of several characters, which the terminal may measure otherwise.
        let mut painter = Painter::new(Features { graphemes: true, ..Features::default() });
        painter.frame(&Canvas::new(8, 1));
        assert_eq!(text(&painter.frame(&canvas)), format!("\x1b[1;1Ha{family}\x1b[1;4He\u{301}\x1b[1;5Hx"));
        // Another canvas, its graphemes stored elsewhere: the same text, nothing to send.
        let mut again = Canvas::new(8, 1);
        again.clusters.push_str("zzzzz");
        again.put(0, 0, "a", Style::PLAIN);
        again.put_cell(1, 0, family, 2, Style::PLAIN);
        again.put(3, 0, "e\u{301}x", Style::PLAIN);
        assert!(painter.frame(&again).is_empty());
        // A terminal that measures character by character: what fits the cells, up to a joiner; the accent stays.
        let thumb = "\u{1f44d}\u{1f3fd}";
        let canvas = pane_row(8, &[(family, 2), (thumb, 2), ("e\u{301}", 1), ("1\u{fe0f}\u{20e3}", 2)]);
        let mut painter = Painter::default();
        painter.frame(&Canvas::new(8, 1));
        assert_eq!(
            text(&painter.frame(&canvas)),
            "\x1b[1;1H\u{1f468}\x1b[1;3H\u{1f44d}\x1b[1;5He\u{301}\x1b[1;6H1\u{fe0f}\u{20e3} "
        );
        // Wide in the last column: the terminal would wrap it, so a blank.
        let mut canvas = Canvas::new(3, 1);
        canvas.put_cell(1, 0, "日", 2, Style::PLAIN);
        canvas.put_cell(2, 0, "本", 2, Style::PLAIN);
        assert_eq!(canvas.row(0), "");
        // VS16 in one column: left out, the terminals that heed it would draw two; in two, kept.
        let mut canvas = Canvas::new(4, 1);
        canvas.put_cell(0, 0, "\u{26a0}\u{fe0f}", 1, Style::PLAIN);
        canvas.put_cell(1, 0, "\u{2764}\u{fe0f}", 2, Style::PLAIN);
        assert_eq!(canvas.row(0), "\u{26a0}\u{2764}\u{fe0f}");
        assert_eq!(canvas.cells[0].glyph, Glyph::Char('\u{26a0}'));
        // A family as an engine that measures character by character gives it, a ZWJ with each of the first two: the
        // ZWJ left out, each one alone.
        let canvas = pane_row(6, &[("\u{1f468}\u{200d}", 2), ("\u{1f469}\u{200d}", 2), ("\u{1f467}", 2)]);
        assert_eq!(canvas.row(0), "\u{1f468}\u{1f469}\u{1f467}");
        assert!(canvas.clusters.is_empty());
        // Direction controls left out: a pane is never reordered.
        let canvas = pane_row(3, &[("a\u{202e}", 1), ("\u{2067}", 1), ("b\u{301}\u{2069}", 1)]);
        assert_eq!(canvas.row(0), "a b\u{301}");
        // A grapheme past its bytes is cut on a character.
        let mut canvas = Canvas::new(2, 1);
        canvas.put_cell(0, 0, &format!("e{}", "\u{301}".repeat(40)), 1, Style::PLAIN);
        assert_eq!(canvas.row(0), format!("e{}", "\u{301}".repeat(31)));
    }

    #[test]
    fn links_by_their_uri() {
        let linked = |uris: &[&str], uri: &str| {
            let mut canvas = Canvas::new(6, 1);
            for other in uris {
                canvas.link(other);
            }
            let link = canvas.link(uri);
            canvas.put(0, 0, "ab", Style { link: Some(link), ..Style::PLAIN });
            canvas.put(2, 0, "c", Style::PLAIN);
            canvas
        };
        let mut painter = Painter::new(Features { links: true, ..Features::default() });
        painter.frame(&Canvas::new(6, 1));
        let first = linked(&[], "https://a.example");
        assert_eq!(first.link_at(1, 0), Some("https://a.example"));
        assert_eq!(text(&painter.frame(&first)), "\x1b[1;1H\x1b]8;;https://a.example\x1b\\ab\x1b]8;;\x1b\\c");
        // Another id for the same URI: the same frame.
        assert!(painter.frame(&linked(&["https://b.example"], "https://a.example")).is_empty());
        // The same id for another URI: sent again, and the link closed at the end.
        assert_eq!(
            text(&painter.frame(&linked(&[], "https://b.example"))),
            "\x1b[1;1H\x1b]8;;https://b.example\x1b\\ab\x1b]8;;\x1b\\"
        );
        // A URI that would end the sequence early goes without its link.
        assert_eq!(text(&painter.frame(&linked(&[], "https://c\x1b\\x"))), "\x1b[1;1Hab");
        // A terminal without links: the text only.
        let mut painter = Painter::default();
        painter.frame(&Canvas::new(6, 1));
        assert_eq!(text(&painter.frame(&linked(&[], "https://a.example"))), "\x1b[1;1Habc");
    }

    #[test]
    fn the_cursor_last() {
        let mut painter = Painter::default();
        let mut canvas = Canvas::new(4, 2);
        // No cursor: it stays hidden, as the screen was set up; nothing said.
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[0m\x1b[2J\x1b[1;1H    \x1b[2;1H    ");
        assert!(painter.frame(&canvas).is_empty());
        let bar = Cursor { x: 2, y: 1, shape: CursorShape::Bar, blink: true };
        canvas.set_cursor(Some(bar));
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;3H\x1b[5 q\x1b[?25h");
        assert!(painter.frame(&canvas).is_empty(), "still there, nothing sent");
        // Cells drawn: the cursor hidden meanwhile, put back after.
        canvas.put(0, 0, "x", Style::PLAIN);
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[?25l\x1b[1;1Hx\x1b[2;3H\x1b[?25h");
        // Moved, steady: only that.
        canvas.set_cursor(Some(Cursor { x: 0, blink: false, ..bar }));
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;1H\x1b[6 q");
        canvas.set_cursor(Some(Cursor { x: 9, ..bar }));
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[?25l", "off the canvas: hidden");
        // The user's own shape: DECSCUSR 0, whatever the blink.
        canvas.set_cursor(Some(Cursor { shape: CursorShape::Default, ..bar }));
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;3H\x1b[0 q\x1b[?25h");
    }

    #[test]
    fn synchronized_frames() {
        let mut painter = Painter::new(Features { sync: true, ..Features::default() });
        let mut canvas = Canvas::new(4, 1);
        canvas.set_cursor(Some(Cursor { x: 0, y: 0, shape: CursorShape::Block, blink: false }));
        painter.frame(&canvas);
        assert!(painter.frame(&canvas).is_empty(), "nothing to frame");
        canvas.put(1, 0, "y", Style::PLAIN);
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[?2026h\x1b[1;2Hy\x1b[1;1H\x1b[?2026l");
    }

    #[test]
    fn colors_for_a_terminal_without_24_bits() {
        assert_eq!(ansi256(0, 0, 0), 16);
        assert_eq!(ansi256(0xff, 0xff, 0xff), 231);
        assert_eq!(ansi256(0xaf, 0x87, 0xff), 141);
        assert_eq!(ansi256(0x80, 0x80, 0x80), 244);
        assert_eq!(ansi256(0xd7, 0x77, 0x57), 173);
        let mut painter = Painter::new(Features { truecolor: false, ..Features::default() });
        let mut canvas = Canvas::new(1, 1);
        canvas.put(0, 0, "x", Style::fg(Color::Rgb { r: 0xaf, g: 0x87, b: 0xff }));
        assert!(text(&painter.frame(&canvas)).contains("\x1b[38;5;141mx"));
        let mut painter = Painter::default();
        assert!(text(&painter.frame(&canvas)).contains("\x1b[38;2;175;135;255mx"));
    }

    #[test]
    fn wide_characters_go_whole() {
        let mut painter = Painter::default();
        let mut canvas = Canvas::new(6, 1);
        canvas.put(0, 0, "ab", Style::PLAIN);
        painter.frame(&canvas);
        canvas.put(0, 0, "日", Style::PLAIN);
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[1;1H日");
        canvas.put(1, 0, "c", Style::PLAIN);
        assert_eq!(text(&painter.frame(&canvas)), "\x1b[1;1H c");
    }

    #[test]
    fn a_new_size_or_a_lent_terminal_sends_it_all() {
        let mut painter = Painter::default();
        let canvas = Canvas::new(4, 1);
        painter.frame(&canvas);
        assert!(text(&painter.frame(&Canvas::new(5, 1))).starts_with("\x1b[0m\x1b[2J"));
        painter.forget();
        assert!(text(&painter.frame(&canvas)).starts_with("\x1b[0m\x1b[2J"));
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
