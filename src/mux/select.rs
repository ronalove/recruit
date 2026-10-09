// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A pane's text selected with the mouse, and copied (spec §5.5): a press starts it, a drag extends it, a double
//! click takes a word, a triple click a line; the release copies it (OSC 52, by `Relay::Clipboard`). The selection
//! stays shown until the next press or key. And the history scrolled with the keyboard.
//!
//! The server holds one selection for the screen, and routes the mouse to it when the pane's program does not take
//! the mouse, or with Shift. It clears it on a press elsewhere, a key sent to the pane, a resize (the text is wrapped
//! anew), a switch to or from the alternate screen. Points count lines from the first line the pane had
//! ([`Engine::top`]): they stay on their text while the program writes and the view scrolls.
//!
//! Owner: dev-rendu.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::Rect;
use super::engine::{Engine, Line, Relay};
use super::input::{Key, KeyCode, Mods};

/// A cell of a pane's whole text, history included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Point {
    pub line: u64,
    pub col: usize,
}

/// What a press, and the drag after it, select by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Cell,
    Word,
    Line,
}

/// Two presses this close in time, on the same cell, make a double click; three, a triple.
const MULTI: Duration = Duration::from_millis(400);

/// The most a copy carries: beyond, terminals refuse OSC 52 or choke on it.
const COPY: usize = 1 << 20;

/// Cells that end a word, besides blanks: quotes, brackets, separators, and the lines Claude Code draws its boxes
/// with. « / . - _ : @ » stay in words: a path or a URL is taken whole by a double click.
fn separator(cell: &str) -> bool {
    let mut chars = cell.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return false };
    matches!(
        c,
        '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '|' | ',' | ';' | '\u{2500}'..='\u{257f}'
    )
}

/// A cell's kind, for words: runs of the same kind go together, a separator alone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Blank,
    Separator,
    Word,
}

fn kind(cell: &str) -> Kind {
    if cell.trim().is_empty() {
        Kind::Blank
    } else if separator(cell) {
        Kind::Separator
    } else {
        Kind::Word
    }
}

/// The point under the screen's cell `col`, `row`, in a pane whose cells are `cells`: brought within the pane when
/// outside (a drag past its edge).
pub(crate) fn point(engine: &dyn Engine, cells: Rect, col: usize, row: usize) -> Point {
    let col = col.clamp(cells.x, cells.x + cells.width.saturating_sub(1)) - cells.x;
    let row = row.clamp(cells.y, cells.y + cells.height.saturating_sub(1)) - cells.y;
    Point { line: engine.top() + row as u64, col }
}

/// Lines to scroll while a drag is past the pane's top (up into the history: 1) or bottom (back down: -1).
pub(crate) fn edge(cells: Rect, row: usize) -> isize {
    if row < cells.y {
        1
    } else if row >= cells.y + cells.height {
        -1
    } else {
        0
    }
}

/// What a key does to a pane's history, if it scrolls it: Shift with Page Up, Page Down (a screen less a line),
/// Home (the oldest line), End (the live screen); `rows` the pane's. Not on the alternate screen, which has no
/// history: the key goes to the program.
pub(crate) fn scroll_key(key: &Key, rows: usize, alt_screen: bool) -> Option<isize> {
    if key.mods != Mods::SHIFT || alt_screen || !key.is_press() {
        return None;
    }
    let page = rows.saturating_sub(1).max(1) as isize;
    match key.code {
        KeyCode::PageUp => Some(page),
        KeyCode::PageDown => Some(-page),
        KeyCode::Home => Some(isize::MAX),
        KeyCode::End => Some(isize::MIN),
        _ => None,
    }
}

/// A selection in one pane.
#[derive(Clone, Debug)]
pub(crate) struct Selection {
    /// The pane, as the server names them.
    pub pane: String,
    /// Where the press was, and where the pointer is.
    anchor: Point,
    head: Point,
    unit: Unit,
    /// When the press was, and how many in a row on that cell: for the next one.
    pressed: Instant,
    clicks: u8,
    /// What is selected, from its first cell to its last (`usize::MAX`: the end of the line); none for a click alone.
    range: Option<(Point, Point)>,
}

/// Lines a double or a triple click looks at, up and down from where it is: a word or a line longer than that
/// (base64, minified JSON, a line of a megabyte wrapped on thousands) is cut there. The work stays short, on the
/// screen's loop with the engine's lock held.
const SPAN: u64 = 256;

/// The kind of each column of `line`; the second column of a wide character takes the first's.
fn kinds(line: &Line) -> Vec<Kind> {
    let mut kinds = Vec::with_capacity(line.width());
    let mut last = Kind::Blank;
    for col in 0..line.width() {
        let cell = line.cell(col);
        if !cell.is_empty() {
            last = kind(cell);
        }
        kinds.push(last);
    }
    kinds
}

/// Lines read from an engine, by number, kept while a selection is worked out.
struct Text<'a> {
    engine: &'a dyn Engine,
    lines: HashMap<u64, Option<Line>>,
}

impl<'a> Text<'a> {
    fn new(engine: &'a dyn Engine) -> Self {
        Text { engine, lines: HashMap::new() }
    }

    fn line(&mut self, line: u64) -> Option<&Line> {
        let engine = self.engine;
        self.lines.entry(line).or_insert_with(|| engine.line(line)).as_ref()
    }

    fn wrapped(&mut self, line: u64) -> bool {
        self.line(line).is_some_and(|line| line.wrapped)
    }

    fn kinds(&mut self, line: u64) -> Vec<Kind> {
        self.line(line).map(kinds).unwrap_or_default()
    }

    /// The word, the run of blanks or the separator at `at`, across wrapped lines' ends, [`SPAN`] lines at most
    /// each way.
    fn word(&mut self, at: Point) -> (Point, Point) {
        let mut kinds = self.kinds(at.line);
        let Some(&kind) = kinds.get(at.col.min(kinds.len().saturating_sub(1))) else { return (at, at) };
        let col = at.col.min(kinds.len() - 1);
        if kind == Kind::Separator {
            return (Point { col, ..at }, Point { col, ..at });
        }
        let (mut line, mut first) = (at.line, col);
        loop {
            while first > 0 && kinds[first - 1] == kind {
                first -= 1;
            }
            if first > 0 || line <= self.engine.oldest() || at.line - line >= SPAN || !self.wrapped(line - 1) {
                break;
            }
            let up = self.kinds(line - 1);
            if up.last() != Some(&kind) {
                break;
            }
            first = up.len() - 1;
            (line, kinds) = (line - 1, up);
        }
        let start = Point { line, col: first };
        let (mut line, mut kinds, mut last) = (at.line, self.kinds(at.line), col);
        loop {
            while last + 1 < kinds.len() && kinds[last + 1] == kind {
                last += 1;
            }
            if last + 1 < kinds.len() || line - at.line >= SPAN || !self.wrapped(line) {
                break;
            }
            let down = self.kinds(line + 1);
            if down.first() != Some(&kind) {
                break;
            }
            (line, kinds, last) = (line + 1, down, 0);
        }
        (start, Point { line, col: last })
    }

    /// The whole line at `line`, its wrapped parts included, [`SPAN`] lines at most each way.
    fn line_of(&mut self, line: u64) -> (Point, Point) {
        let mut start = line;
        while start > self.engine.oldest() && line - start < SPAN && self.wrapped(start - 1) {
            start -= 1;
        }
        let mut end = line;
        while end - line < SPAN && self.wrapped(end) {
            end += 1;
        }
        (Point { line: start, col: 0 }, Point { line: end, col: usize::MAX })
    }
}

impl Selection {
    /// A press of the left button at `at` in pane `pane`: a new selection, a word on a double click (the previous
    /// selection pressed on the same cell or the next, a moment ago, and not dragged since), a line on a triple.
    pub(crate) fn press(
        previous: Option<&Selection>,
        pane: &str,
        at: Point,
        now: Instant,
        engine: &dyn Engine,
    ) -> Selection {
        // On the same cell or the one beside it: a hand that clicks twice moves a little.
        let near = |anchor: Point| anchor.line == at.line && anchor.col.abs_diff(at.col) <= 1;
        let clicks = match previous {
            Some(previous)
                if previous.pane == pane
                    && near(previous.anchor)
                    && now.saturating_duration_since(previous.pressed) <= MULTI =>
            {
                previous.clicks % 3 + 1
            }
            _ => 1,
        };
        let unit = match clicks {
            1 => Unit::Cell,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let mut selection =
            Selection { pane: pane.to_string(), anchor: at, head: at, unit, pressed: now, clicks, range: None };
        selection.extend(engine);
        selection
    }

    /// The pointer dragged to `at`, the button down.
    pub(crate) fn drag(&mut self, at: Point, engine: &dyn Engine) {
        if at != self.head {
            self.head = at;
            // A press after a drag starts anew, however close.
            self.clicks = 0;
            self.extend(engine);
        }
    }

    fn extend(&mut self, engine: &dyn Engine) {
        let (low, high) = (self.anchor.min(self.head), self.anchor.max(self.head));
        let mut text = Text::new(engine);
        self.range = match self.unit {
            Unit::Cell if low == high => None,
            Unit::Cell => {
                // From the first column of a wide character pressed on its second.
                let tail =
                    text.line(low.line).is_some_and(|line| low.col < line.width() && line.cell(low.col).is_empty());
                Some((if tail && low.col > 0 { Point { col: low.col - 1, ..low } } else { low }, high))
            }
            Unit::Word => Some((text.word(low).0, text.word(high).1)),
            Unit::Line => Some((text.line_of(low.line).0, text.line_of(high.line).1)),
        };
    }

    /// What is selected, from its first cell to its last (`usize::MAX` for the end of a line); none for a click.
    pub(crate) fn range(&self) -> Option<(Point, Point)> {
        self.range
    }

    /// Whether its text is still kept: lines past the history's limit are gone.
    pub(crate) fn kept(&self, engine: &dyn Engine) -> bool {
        self.range.is_none_or(|(start, _)| start.line >= engine.oldest())
    }

    /// The selected text: blanks at the ends of lines left out, a wrapped line joined to the next, the others ended
    /// by a new line; empty for a click.
    pub(crate) fn text(&self, engine: &dyn Engine) -> String {
        let Some((start, end)) = self.range else { return String::new() };
        let mut out = String::new();
        for number in start.line.max(engine.oldest())..=end.line {
            let Some(line) = engine.line(number) else { continue };
            let from = if number == start.line { start.col } else { 0 };
            let to = if number == end.line { end.col.saturating_add(1) } else { usize::MAX };
            let begun = out.len();
            out.push_str(line.span(from, to));
            // A wrapped line's blanks are its text, cut by the edge.
            let wrapped = line.wrapped && to >= line.width();
            if !wrapped {
                let kept = out[begun..].trim_end().len();
                out.truncate(begun + kept);
                if number != end.line {
                    out.push('\n');
                }
            }
            if out.len() > COPY {
                let mut cut = COPY;
                while !out.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.truncate(cut);
                break;
            }
        }
        out
    }

    /// What the release of the button copies: the selected text, for the real terminal's clipboard; nothing for a
    /// click.
    pub(crate) fn copy(&self, engine: &dyn Engine) -> Option<Relay> {
        let text = self.text(engine);
        (!text.is_empty()).then_some(Relay::Clipboard(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{Canvas, Cursor};
    use crate::mux::Rgb;
    use crate::mux::engine::Modes;

    /// A pane's text: lines of `width` cells at least, those ending in `\` wrapped into the next (and as wide as
    /// their text); the first `dropped` gone from the history, the view `top`.
    struct Fake {
        lines: Vec<&'static str>,
        width: usize,
        dropped: u64,
        top: u64,
    }

    impl Fake {
        fn new(lines: &[&'static str], width: usize) -> Fake {
            Fake { lines: lines.to_vec(), width, dropped: 0, top: 0 }
        }
    }

    impl Engine for Fake {
        fn feed(&mut self, _: &[u8], _: &mut Vec<u8>) {}
        fn deadline(&self) -> Option<Instant> {
            None
        }
        fn expire(&mut self, _: Instant, _: &mut Vec<u8>) -> bool {
            false
        }
        fn relays(&mut self, _: &mut Vec<Relay>) {}
        fn resize(&mut self, _: u16, _: u16) {}
        fn set_colors(&mut self, _: Option<Rgb>, _: Option<Rgb>) {}
        fn modes(&self) -> Modes {
            Modes::default()
        }
        fn draw(&self, _: &mut Canvas, _: Rect) {}
        fn cursor(&self) -> Option<Cursor> {
            None
        }
        fn history(&self) -> usize {
            0
        }
        fn scroll(&mut self, _: isize) {}
        fn scrolled(&self) -> usize {
            0
        }
        fn top(&self) -> u64 {
            self.top
        }
        fn oldest(&self) -> u64 {
            self.dropped
        }
        fn line(&self, line: u64) -> Option<Line> {
            if line < self.dropped {
                return None;
            }
            let text = self.lines.get(line as usize)?;
            let (text, wrapped) = match text.strip_suffix('\\') {
                Some(text) => (text, true),
                None => (*text, false),
            };
            let mut line = Line { wrapped, ..Line::default() };
            for c in text.chars() {
                line.push(c.encode_utf8(&mut [0; 4]));
                // 日 and 本 take two columns.
                if matches!(c, '日' | '本') {
                    line.push("");
                }
            }
            // A wrapped line fills the width: its text is written so in the tests.
            while !wrapped && line.width() < self.width {
                line.push(" ");
            }
            Some(line)
        }
    }

    fn at(line: u64, col: usize) -> Point {
        Point { line, col }
    }

    #[test]
    fn a_drag_selects_cells() {
        let fake = Fake::new(&["$ ls -la", "a.txt  b.txt", "done"], 12);
        let now = Instant::now();
        let mut selection = Selection::press(None, "0", at(0, 2), now, &fake);
        assert_eq!(selection.range(), None, "a click alone selects nothing");
        assert_eq!(selection.copy(&fake), None);
        selection.drag(at(1, 4), &fake);
        assert_eq!(selection.text(&fake), "ls -la\na.txt");
        // Dragged back before the press: from there to the press.
        selection.drag(at(0, 0), &fake);
        assert_eq!(selection.text(&fake), "$ l");
        // Blanks at the ends of lines left out, a blank line kept.
        selection.drag(at(2, 11), &fake);
        assert_eq!(selection.copy(&fake), Some(Relay::Clipboard("ls -la\na.txt  b.txt\ndone".into())));
    }

    #[test]
    fn double_and_triple_clicks() {
        let fake = Fake::new(&["open ~/src/main.rs:12 (\"x\")", "one long line that goes \\", "on here", "next"], 30);
        let now = Instant::now();
        let first = Selection::press(None, "0", at(0, 9), now, &fake);
        let double = Selection::press(Some(&first), "0", at(0, 9), now + Duration::from_millis(200), &fake);
        assert_eq!(double.text(&fake), "~/src/main.rs:12", "a path whole");
        // On a separator: itself; on a blank: the blanks.
        let quote = Selection::press(Some(&first), "0", at(0, 23), now, &fake);
        let quote = Selection::press(Some(&quote), "0", at(0, 23), now, &fake);
        assert_eq!(quote.text(&fake), "\"");
        // A word across a wrapped line's end: "goes" then the blank at the edge, then "on" on the next line.
        let word = Selection::press(None, "0", at(2, 0), now, &fake);
        let word = Selection::press(Some(&word), "0", at(2, 0), now, &fake);
        assert_eq!(word.text(&fake), "on");
        // A triple click: the whole line, its wrapped parts too.
        let triple = Selection::press(Some(&double), "0", at(0, 9), now + Duration::from_millis(400), &fake);
        assert_eq!(triple.text(&fake), "open ~/src/main.rs:12 (\"x\")");
        let line = Selection::press(None, "0", at(2, 3), now, &fake);
        let line = Selection::press(Some(&line), "0", at(2, 3), now, &fake);
        let line = Selection::press(Some(&line), "0", at(2, 3), now, &fake);
        assert_eq!(line.text(&fake), "one long line that goes on here");
        assert_eq!(line.range(), Some((at(1, 0), at(2, usize::MAX))));
        // Dragged by lines.
        let mut lines = line.clone();
        lines.drag(at(3, 0), &fake);
        assert_eq!(lines.text(&fake), "one long line that goes on here\nnext");
        // The cell beside: still a double click.
        let beside = Selection::press(Some(&first), "0", at(0, 10), now, &fake);
        assert_eq!(beside.text(&fake), "~/src/main.rs:12");
        // Too late, another pane, two cells away, another line, or after a drag: a new click.
        let late = Selection::press(Some(&first), "0", at(0, 9), now + Duration::from_millis(401), &fake);
        let other = Selection::press(Some(&first), "1", at(0, 9), now, &fake);
        let moved = Selection::press(Some(&first), "0", at(0, 11), now, &fake);
        let below = Selection::press(Some(&first), "0", at(1, 9), now, &fake);
        let mut dragged = first.clone();
        dragged.drag(at(0, 12), &fake);
        let after = Selection::press(Some(&dragged), "0", at(0, 9), now, &fake);
        assert!([late, other, moved, below, after].iter().all(|s| s.range().is_none()));
    }

    #[test]
    fn wide_characters_and_wrapped_lines() {
        let fake = Fake::new(&["日本 ok", "a wrapped line \\", "end"], 15);
        let now = Instant::now();
        // From the second column of 日: 日 whole.
        let mut selection = Selection::press(None, "0", at(0, 1), now, &fake);
        selection.drag(at(0, 2), &fake);
        assert_eq!(selection.text(&fake), "日本");
        // A wrapped line joined to the next, its blank at the edge kept.
        let mut selection = Selection::press(None, "0", at(1, 10), now, &fake);
        selection.drag(at(2, 2), &fake);
        assert_eq!(selection.text(&fake), "line end");
        // A double click on 本's second column: the word 日本.
        let word = Selection::press(None, "0", at(0, 3), now, &fake);
        let word = Selection::press(Some(&word), "0", at(0, 3), now, &fake);
        assert_eq!(word.text(&fake), "日本");
    }

    #[test]
    fn a_word_or_a_line_without_end_is_cut() {
        // A line of 10 000 lines wrapped, all of one word: a double or a triple click looks no further than SPAN
        // lines each way.
        let mut lines = vec!["xxxxxxxxxx\\"; 10_000];
        lines.push("xx");
        let fake = Fake::new(&lines, 10);
        let now = Instant::now();
        let click = Selection::press(None, "0", at(5_000, 4), now, &fake);
        let word = Selection::press(Some(&click), "0", at(5_000, 4), now, &fake);
        assert_eq!(word.range(), Some((at(5_000 - SPAN, 0), at(5_000 + SPAN, 9))));
        let line = Selection::press(Some(&word), "0", at(5_000, 4), now, &fake);
        assert_eq!(line.range(), Some((at(5_000 - SPAN, 0), at(5_000 + SPAN, usize::MAX))));
        assert_eq!(line.text(&fake).len(), (2 * SPAN as usize + 1) * 10);
    }

    #[test]
    fn points_on_screen_and_lines_gone() {
        let mut fake = Fake::new(&["zero", "one", "two", "three"], 8);
        fake.top = 2;
        let cells = Rect { x: 10, y: 5, width: 8, height: 2 };
        assert_eq!(point(&fake, cells, 12, 5), at(2, 2));
        // Past the edges: brought within the pane; the drag scrolls.
        assert_eq!(point(&fake, cells, 0, 99), at(3, 0));
        assert_eq!(point(&fake, cells, 99, 0), at(2, 7));
        assert_eq!((edge(cells, 4), edge(cells, 5), edge(cells, 6), edge(cells, 7)), (1, 0, 0, -1));
        let mut selection = Selection::press(None, "0", at(0, 0), Instant::now(), &fake);
        selection.drag(at(2, 2), &fake);
        assert!(selection.kept(&fake));
        // The first lines gone from the history: the rest only.
        fake.dropped = 1;
        assert!(!selection.kept(&fake));
        assert_eq!(selection.text(&fake), "one\ntwo");
    }

    #[test]
    fn keys_that_scroll() {
        let shift = |code| Key::new(code, Mods::SHIFT);
        assert_eq!(scroll_key(&shift(KeyCode::PageUp), 20, false), Some(19));
        assert_eq!(scroll_key(&shift(KeyCode::PageDown), 20, false), Some(-19));
        assert_eq!(scroll_key(&shift(KeyCode::Home), 20, false), Some(isize::MAX));
        assert_eq!(scroll_key(&shift(KeyCode::End), 20, false), Some(isize::MIN));
        assert_eq!(scroll_key(&shift(KeyCode::PageUp), 1, false), Some(1));
        // Without Shift, or on the alternate screen: the program's.
        assert_eq!(scroll_key(&Key::new(KeyCode::PageUp, Mods::NONE), 20, false), None);
        assert_eq!(scroll_key(&shift(KeyCode::PageUp), 20, true), None);
    }
}
