// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Panes composed into one screen (spec §5.3): each one's cells in its frame (`chrome.rs`, direction B « Cadres »),
//! the bar on the last row, the cursor of the pane with the focus. The frame is then sent by difference
//! (`canvas::Painter`).
//!
//! Owner: dev-rendu.

use super::Rect;
use super::chrome::{self, Header, Look, Tab, Target};
use super::engine::Engine;
use super::select::Point;
use crate::canvas::{Canvas, Cursor, Style};
use crossterm::style::Color;

/// What shows over the team, everything else dimmed, the bar included (direction « Cadres »).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Over<'a> {
    Nothing,
    /// The last of the views: the menu.
    LastView,
    /// A choice to make (quit, compact a member), in the middle of the team's area ([`chrome::choice_area`]).
    Choice(&'a chrome::Choice),
}

/// Where a frame can be clicked: the bar's parts, and a choice's options by their index. Nothing of the bar while
/// something shows over the team: what is dimmed takes no click.
#[derive(Debug, Default)]
pub(crate) struct Zones {
    pub bar: Vec<(Rect, Target)>,
    pub options: Vec<(Rect, usize)>,
}

/// What is under a layer: as the menu dims what is behind its dialogs.
const DIMMED: Style = Style { fg: Some(Color::AnsiValue(239)), ..Style::PLAIN };

/// A pane as the screen shows it.
pub(crate) struct View<'a> {
    /// The pane, as the server names them: [`Screen`] keeps its cells from one frame to the next by it.
    pub id: &'a str,
    pub engine: &'a dyn Engine,
    /// The pane's place, its frame included: its cells inside ([`content`]).
    pub area: Rect,
    /// What its frame says; how far up its history it shows is filled in (`Header::scroll`).
    pub header: Header<'a>,
    pub focused: bool,
    /// Its selected text (`select::Selection::range`), shown in reverse video.
    pub selection: Option<(Point, Point)>,
}

/// Where a pane's cells go in its `area`: inside its frame. Its engine and its PTY have this size.
pub(crate) fn content(area: Rect) -> Rect {
    chrome::inside(area)
}

/// The panes drawn into `canvas`, each in its frame, the focused one's cursor when it shows its live screen, the bar
/// in `bar` (the screen's last row; nothing when empty) with the team's name and its tabs, and what is `over` them.
/// Returns where the frame can be clicked. Everything is drawn: [`Screen::compose`] keeps what did not change, and
/// is what the server calls; this one is the frame drawn whole that its tests compare with.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose(
    canvas: &mut Canvas,
    views: &[View<'_>],
    team: &str,
    tabs: &[Tab<'_>],
    bar: Rect,
    look: Look,
    over: Over<'_>,
) -> Zones {
    let mut cursor = None;
    for view in views {
        cursor = draw(canvas, view, true, look).or(cursor);
    }
    let clear = clear(canvas.width(), canvas.height(), views, bar, over);
    finish(canvas, cursor, team, tabs, bar, look, over, clear)
}

/// What stays clear on a `width` × `height` screen when something is `over` the team: the menu's place, or the
/// choice's.
fn clear(width: usize, height: usize, views: &[View<'_>], bar: Rect, over: Over<'_>) -> Option<Rect> {
    match over {
        Over::Nothing => None,
        Over::LastView => views.last().map(|view| view.area),
        Over::Choice(choice) => {
            let team = Rect { x: 0, y: 0, width, height: if bar.height > 0 { bar.y } else { height } };
            Some(chrome::choice_area(team, choice))
        }
    }
}

/// One view: its cells when `cells` (else they are on the canvas already), its frame; its cursor on the screen when
/// it has the focus and shows its live screen.
fn draw(canvas: &mut Canvas, view: &View<'_>, cells: bool, look: Look) -> Option<Cursor> {
    let area = view.area;
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let inside = content(area);
    if cells && inside.width > 0 && inside.height > 0 {
        canvas.fill(inside.x, inside.y, inside.width, inside.height, Style::PLAIN);
        view.engine.draw(canvas, inside);
        if let Some(range) = view.selection {
            selected(canvas, inside, view.engine.top(), range);
        }
    }
    let scrolled = view.engine.scrolled();
    let header =
        Header { scroll: (scrolled > 0).then(|| (scrolled, view.engine.history().max(scrolled))), ..view.header };
    chrome::frame(canvas, area, &header, view.focused, look);
    if !view.focused || scrolled > 0 {
        return None;
    }
    view.engine.cursor().filter(|c| c.x < inside.width && c.y < inside.height).map(|c| Cursor {
        x: inside.x + c.x,
        y: inside.y + c.y,
        ..c
    })
}

/// The cursor, the bar (its row blanked first, for a canvas kept from the frame before), then what is over the team:
/// everything outside `clear` dimmed, and the choice drawn. Under a layer, the cursor shows only in it.
#[allow(clippy::too_many_arguments)]
fn finish(
    canvas: &mut Canvas,
    cursor: Option<Cursor>,
    team: &str,
    tabs: &[Tab<'_>],
    bar: Rect,
    look: Look,
    over: Over<'_>,
    clear: Option<Rect>,
) -> Zones {
    let cursor = match over {
        Over::Nothing => cursor,
        Over::LastView => cursor.filter(|c| clear.is_some_and(|area| area.contains(c.x, c.y))),
        Over::Choice(_) => None,
    };
    canvas.set_cursor(cursor);
    let mut zones = Zones::default();
    if bar.width > 0 && bar.height > 0 {
        canvas.fill(bar.x, bar.y, bar.width, bar.height, Style::PLAIN);
        zones.bar = chrome::bar(canvas, bar, team, tabs, look);
    }
    if let Some(area) = clear {
        canvas.fade_outside(area.x, area.y, area.width, area.height, DIMMED);
        zones.bar.clear();
        if let Over::Choice(choice) = over {
            zones.options = chrome::choice(canvas, area, choice, look);
        }
    }
    zones
}

/// What a view's cells showed when they were last drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Drawn {
    id: String,
    area: Rect,
    generation: Option<u64>,
    selection: Option<(Point, Point)>,
    /// The line on its first row, when a selection shows: the view can move without its cells changing.
    top: Option<u64>,
}

/// Bytes of graphemes the canvas keeps before it is compacted.
const ROOM: usize = 64 * 1024;

/// The screen's canvas, kept from one frame to the next, and what each view's cells showed in it: a pane whose
/// engine drew nothing new since ([`Engine::generation`]), in the same place, with the same selection, and that
/// overlaps no view drawn before it (a layer is painted at every frame), keeps its cells rather than being painted
/// again (spec §2). A new
/// size, other panes, or another order: everything is drawn.
#[derive(Default)]
pub(crate) struct Screen {
    canvas: Canvas,
    drawn: Vec<Drawn>,
    /// Set while a frame is composed: one that panicked leaves it set, and the next is drawn whole.
    composing: bool,
    /// What stayed clear under a layer: when it changes, comes or goes, everything is drawn (a cell kept dimmed, or
    /// one kept from under the layer's old place, would stay so).
    clear: Option<Rect>,
}

impl Screen {
    /// The frame for a `width` × `height` screen: see [`compose`]; then in [`Screen::canvas`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compose(
        &mut self,
        width: usize,
        height: usize,
        views: &[View<'_>],
        team: &str,
        tabs: &[Tab<'_>],
        bar: Rect,
        look: Look,
        over: Over<'_>,
    ) -> Zones {
        let clear = clear(width, height, views, bar, over);
        let same = !self.composing
            && (self.canvas.width(), self.canvas.height()) == (width, height)
            && clear == self.clear
            && self.drawn.len() == views.len()
            && self.drawn.iter().zip(views).all(|(drawn, view)| drawn.id == view.id && drawn.area == view.area);
        self.composing = true;
        if same {
            self.canvas.compact(ROOM);
            self.canvas.advance();
        } else {
            self.canvas.reset(width, height);
            self.drawn.clear();
            self.drawn.resize(views.len(), Drawn::default());
        }
        self.clear = clear;
        // The areas of the views drawn before: their frames are drawn again at every frame, and a view over one of
        // them (a layer) is painted again too, or a frame would show through it.
        let mut before: Vec<Rect> = Vec::new();
        let mut cursor = None;
        for (view, drawn) in views.iter().zip(&mut self.drawn) {
            let generation = view.engine.generation();
            let top = view.selection.map(|_| view.engine.top());
            let keep = same
                && generation.is_some()
                && drawn.generation == generation
                && drawn.selection == view.selection
                && drawn.top == top
                && !before.iter().any(|area| overlap(area, &view.area));
            before.push(view.area);
            if !keep {
                if drawn.id != view.id {
                    drawn.id = view.id.to_string();
                }
                (drawn.area, drawn.generation, drawn.selection, drawn.top) =
                    (view.area, generation, view.selection, top);
            }
            cursor = draw(&mut self.canvas, view, !keep, look).or(cursor);
        }
        let zones = finish(&mut self.canvas, cursor, team, tabs, bar, look, over, clear);
        self.composing = false;
        zones
    }

    /// The last frame composed.
    pub(crate) fn canvas(&self) -> &Canvas {
        &self.canvas
    }
}

fn overlap(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

/// The cells of `range` a pane shows in `cells`, from line `top`, in reverse video.
fn selected(canvas: &mut Canvas, cells: Rect, top: u64, (start, end): (Point, Point)) {
    for row in 0..cells.height {
        let line = top + row as u64;
        if line < start.line || line > end.line {
            continue;
        }
        let from = if line == start.line { start.col } else { 0 };
        let to = if line == end.line { end.col.min(cells.width - 1) } else { cells.width - 1 };
        if from <= to {
            canvas.invert(cells.x + from, cells.y + row, to - from + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::canvas::{CursorShape, Features, Painter, Style};
    use crate::look::State;
    use crate::mux::Rgb;
    use crate::mux::engine::{Modes, Relay};
    use crate::t;

    /// An engine that shows lines of text: `history` above the screen, `screen` on it.
    struct Lines {
        history: Vec<&'static str>,
        screen: Vec<&'static str>,
        cursor: Option<Cursor>,
        scroll: usize,
        /// What `generation` says; `None`, like an engine that does not tell.
        generation: Option<u64>,
    }

    impl Engine for Lines {
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
        fn draw(&self, canvas: &mut Canvas, area: Rect) {
            let scroll = self.scroll.min(self.history.len());
            let lines = self.history[self.history.len() - scroll..].iter().chain(&self.screen);
            for (row, line) in lines.take(area.height).enumerate() {
                for (col, ch) in line.chars().take(area.width).enumerate() {
                    canvas.put_cell(area.x + col, area.y + row, ch.encode_utf8(&mut [0; 4]), 1, Style::PLAIN);
                }
            }
        }
        fn cursor(&self) -> Option<Cursor> {
            self.cursor
        }
        fn history(&self) -> usize {
            self.history.len()
        }
        fn scroll(&mut self, lines: isize) {
            self.scroll = self.scroll.saturating_add_signed(lines).min(self.history.len());
        }
        fn scrolled(&self) -> usize {
            self.scroll
        }
        fn top(&self) -> u64 {
            (self.history.len() - self.scroll) as u64
        }
        fn generation(&self) -> Option<u64> {
            self.generation
        }
    }

    fn rows(canvas: &Canvas) -> Vec<String> {
        (0..canvas.height()).map(|y| canvas.row(y)).collect()
    }

    const CURSOR: Cursor = Cursor { x: 2, y: 1, shape: CursorShape::Bar, blink: false };

    fn left() -> Lines {
        Lines {
            history: vec!["old 1", "old 2"],
            screen: vec!["$ ls", "a b"],
            cursor: Some(CURSOR),
            scroll: 0,
            generation: None,
        }
    }

    fn right() -> Lines {
        Lines {
            history: vec![],
            screen: vec!["> hello"],
            cursor: Some(Cursor { x: 7, y: 0, ..CURSOR }),
            scroll: 0,
            generation: None,
        }
    }

    fn header(name: &'static str) -> Header<'static> {
        Header { name, color: None, state: State::Working, note: None, scroll: None, hint: None }
    }

    /// Two panes side by side on a 20 × 5 screen, the bar on its last row.
    fn two<'a>(a: &'a Lines, b: &'a Lines, focus: usize) -> [View<'a>; 2] {
        [
            View {
                id: "one",
                engine: a,
                area: Rect { x: 0, y: 0, width: 10, height: 4 },
                header: header("one"),
                focused: focus == 0,
                selection: None,
            },
            View {
                id: "two",
                engine: b,
                area: Rect { x: 10, y: 0, width: 10, height: 4 },
                header: header("two"),
                focused: focus == 1,
                selection: None,
            },
        ]
    }

    const BAR: Rect = Rect { x: 0, y: 4, width: 20, height: 1 };

    #[test]
    fn panes_in_their_frames() {
        let (a, b) = (left(), right());
        let mut canvas = Canvas::new(20, 5);
        let tabs = [Tab { title: "A", active: true, state: State::Working }];
        let zones = compose(&mut canvas, &two(&a, &b, 0), "team", &tabs, BAR, Look::default(), Over::Nothing);
        // Each engine inside its frame, the frames side by side with no line between them.
        let rows = rows(&canvas);
        let cols = |row: &str, from: usize, to: usize| row.chars().skip(from).take(to - from).collect::<String>();
        assert_eq!(cols(&rows[1], 1, 9), "$ ls    ");
        assert_eq!(cols(&rows[1], 11, 19), "> hello ");
        assert_eq!(cols(&rows[2], 1, 4), "a b");
        assert!(rows[0].contains("one") && rows[0].contains("two"), "{rows:?}");
        // The focused pane's cursor, on the screen.
        assert_eq!(canvas.cursor(), Some(Cursor { x: 3, y: 2, ..CURSOR }));
        // The bar, and where its clicks go.
        assert_eq!(zones.bar.first().map(|(_, target)| *target), Some(Target::Tab(0)));
        assert!(zones.bar.iter().all(|(rect, _)| rect.y == BAR.y));

        let mut canvas = Canvas::new(20, 5);
        compose(&mut canvas, &two(&a, &b, 1), "team", &tabs, BAR, Look::default(), Over::Nothing);
        assert_eq!(canvas.cursor(), Some(Cursor { x: 18, y: 1, ..CURSOR }));
        // No bar: nothing drawn on the last row, nothing to click.
        let mut canvas = Canvas::new(20, 5);
        assert!(
            compose(&mut canvas, &two(&a, &b, 1), "team", &tabs, Rect::default(), Look::default(), Over::Nothing)
                .bar
                .is_empty()
        );
        assert_eq!(canvas.row(4), "");
    }

    #[test]
    fn the_selection_in_reverse_video() {
        let (mut a, b) = (left(), right());
        let mut views = two(&a, &b, 0);
        // "ls" on the first row of the live screen (line 2, after two of history), then the next line's start.
        views[0].selection = Some((Point { line: 2, col: 2 }, Point { line: 3, col: 0 }));
        let mut canvas = Canvas::new(20, 5);
        compose(&mut canvas, &views, "team", &[], BAR, Look::default(), Over::Nothing);
        let reversed = |x, y| canvas.style(x, y).reverse;
        // Inside the frame: the pane's cells from column 1, row 1.
        assert!(!reversed(2, 1) && reversed(3, 1) && reversed(8, 1), "to the end of the row");
        assert!(reversed(1, 2) && !reversed(2, 2));
        assert!(!reversed(11, 1), "the other pane untouched");
        // Scrolled up a line: the same text, a row lower.
        a.scroll(1);
        let mut views = two(&a, &b, 0);
        views[0].selection = Some((Point { line: 2, col: 2 }, Point { line: 2, col: 3 }));
        let mut canvas = Canvas::new(20, 5);
        compose(&mut canvas, &views, "team", &[], BAR, Look::default(), Over::Nothing);
        assert_eq!(canvas.row(2).chars().skip(1).take(4).collect::<String>(), "$ ls");
        assert!(canvas.style(3, 2).reverse && canvas.style(4, 2).reverse && !canvas.style(5, 2).reverse);
    }

    #[test]
    fn scrolled_up() {
        let (mut a, b) = (left(), right());
        a.scroll(1);
        let mut views = two(&a, &b, 0);
        // Wide enough for the note: the header is cut to the frame's width (chrome.rs).
        views[0].area.width = 40;
        views[1].area.x = 40;
        let mut canvas = Canvas::new(50, 5);
        compose(&mut canvas, &views, "team", &[], BAR, Look::default(), Over::Nothing);
        let up = t!("↑ {} sur {}", "↑ {} of {}", 1, 2);
        assert!(canvas.row(0).contains(&up), "the scroll in the header: {:?}", canvas.row(0));
        assert_eq!(canvas.row(1).chars().skip(1).take(5).collect::<String>(), "old 2");
        // Scrolled, the focused pane shows no cursor.
        assert_eq!(canvas.cursor(), None);
        // After the pane's own note.
        views[0].header.note = Some("exited");
        let mut canvas = Canvas::new(50, 5);
        compose(&mut canvas, &views, "team", &[], BAR, Look::default(), Over::Nothing);
        assert!(canvas.row(0).contains(&format!("exited · {up}")), "{:?}", canvas.row(0));
        // At the bottom of its history, nothing.
        a.scroll(-1);
        let views = two(&a, &b, 0);
        let mut canvas = Canvas::new(50, 5);
        compose(&mut canvas, &views, "team", &[], BAR, Look::default(), Over::Nothing);
        assert!(!canvas.row(0).contains('↑'), "{:?}", canvas.row(0));
    }

    /// What `text` shows in a Ghostty pane, painted for a terminal that is Ghostty too (libghostty-vt), with mode
    /// 2027 on or off: the screen it reads back, and the pane's.
    fn painted(text: &str, groups: bool) -> (Canvas, Canvas) {
        let (cols, rows) = (16, 6);
        let area = Rect { x: 0, y: 0, width: cols as usize, height: rows as usize };
        let mut replies = Vec::new();
        let mut pane = crate::mux::engine::new(cols, rows, 100).unwrap();
        pane.feed(text.as_bytes(), &mut replies);
        let mut shown = Canvas::new(area.width, area.height);
        pane.draw(&mut shown, area);
        let mut painter = Painter::new(Features { graphemes: groups, ..Features::default() });
        let mut outer = crate::mux::engine::new(cols, rows, 0).unwrap();
        outer.feed(if groups { b"\x1b[?2027h" } else { b"\x1b[?2027l" }, &mut replies);
        outer.feed(&painter.frame(&shown), &mut replies);
        let mut seen = Canvas::new(area.width, area.height);
        outer.draw(&mut seen, area);
        (shown, seen)
    }

    const GRAPHEMES: &str = "a\u{1f44d}\u{1f3fd}b\r\n\u{2764}\u{fe0f}c\r\n\u{1f1eb}\u{1f1f7}d\r\n\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}e\r\n\u{65e5}\u{672c}f";

    #[test]
    fn graphemes_whole_where_the_terminal_groups_them() {
        let (shown, seen) = painted(GRAPHEMES, true);
        for y in 0..5 {
            for x in 0..16 {
                assert_eq!(seen.cell(x, y), shown.cell(x, y), "column {x}, row {y}: {:?}", seen.row(y));
            }
        }
        // Whole, and what follows each one where the pane has it.
        assert_eq!(shown.cell(1, 0), "\u{1f44d}\u{1f3fd}");
        assert_eq!(seen.cell(3, 0), "b");
        assert_eq!(seen.cell(2, 1), "c");
        assert_eq!(seen.cell(2, 2), "d");
        assert_eq!(seen.cell(0, 3), "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}");
        assert_eq!(seen.cell(2, 3), "e");
        assert_eq!(seen.cell(4, 4), "f");
    }

    #[test]
    fn graphemes_cut_where_the_terminal_does_not_group_them() {
        let (shown, seen) = painted(GRAPHEMES, false);
        // Cut to their first character, the line aligned as in the pane.
        for (x, y, after) in [(3, 0, "b"), (2, 1, "c"), (2, 2, "d"), (2, 3, "e"), (4, 4, "f")] {
            assert_eq!(shown.cell(x, y), after);
            assert_eq!(seen.cell(x, y), after, "row {y}: {:?}", seen.row(y));
        }
        assert_eq!(seen.cell(1, 0), "\u{1f44d}");
        assert_eq!(seen.cell(0, 3), "\u{1f468}");
    }

    /// The pane's first cells, on the screen's second row (inside the frame).
    fn first(canvas: &Canvas) -> String {
        canvas.row(1).chars().skip(1).take(7).collect()
    }

    #[test]
    fn a_pane_that_drew_nothing_new_keeps_its_cells() {
        let (mut a, b) = (left(), right());
        a.generation = Some(1);
        let mut screen = Screen::default();
        screen.compose(20, 5, &two(&a, &b, 0), "team", &[], BAR, Look::default(), Over::Nothing);
        assert_eq!(first(screen.canvas()), "$ ls   ");
        // Its text changed, its generation did not: the cells drawn before stay (this is what tells it was kept).
        a.screen = vec!["changed"];
        screen.compose(20, 5, &two(&a, &b, 0), "team", &[], BAR, Look::default(), Over::Nothing);
        assert_eq!(first(screen.canvas()), "$ ls   ");
        // The cursor is set at every frame all the same.
        assert_eq!(screen.canvas().cursor(), Some(Cursor { x: 3, y: 2, ..CURSOR }));
        // A new generation: drawn again, the old text gone.
        a.generation = Some(2);
        screen.compose(20, 5, &two(&a, &b, 0), "team", &[], BAR, Look::default(), Over::Nothing);
        assert_eq!(first(screen.canvas()), "changed");
        assert_eq!(screen.canvas().row(2).chars().skip(1).take(3).collect::<String>(), "   ");
        // Another selection: drawn again.
        let mut views = two(&a, &b, 0);
        views[0].selection = Some((Point { line: 2, col: 0 }, Point { line: 2, col: 1 }));
        screen.compose(20, 5, &views, "team", &[], BAR, Look::default(), Over::Nothing);
        assert!(screen.canvas().style(1, 1).reverse);
        // An engine that does not tell: drawn every time.
        let mut b = right();
        screen.compose(20, 5, &two(&a, &b, 0), "team", &[], BAR, Look::default(), Over::Nothing);
        b.screen = vec!["> bye"];
        screen.compose(20, 5, &two(&a, &b, 0), "team", &[], BAR, Look::default(), Over::Nothing);
        assert_eq!(screen.canvas().row(1).chars().skip(11).take(7).collect::<String>(), "> bye  ");
    }

    /// A pane, and a layer over part of it (the menu).
    fn layered<'a>(pane: &'a Lines, layer: &'a Lines) -> [View<'a>; 2] {
        [
            View {
                id: "one",
                engine: pane,
                area: Rect { x: 0, y: 0, width: 10, height: 4 },
                header: header("one"),
                focused: true,
                selection: None,
            },
            View {
                id: "menu",
                engine: layer,
                area: Rect { x: 2, y: 0, width: 8, height: 3 },
                header: header("menu"),
                focused: false,
                selection: None,
            },
        ]
    }

    /// The same frame as one drawn whole, on a new canvas.
    fn as_whole(screen: &Screen, views: &[View<'_>]) {
        let mut fresh = Canvas::new(20, 5);
        compose(&mut fresh, views, "team", &[], BAR, Look::default(), Over::Nothing);
        for y in 0..5 {
            assert_eq!(screen.canvas().row(y), fresh.row(y), "row {y}");
        }
    }

    #[test]
    fn what_covers_a_pane_drawn_again_is_drawn_again() {
        let (mut pane, mut layer) = (left(), right());
        layer.screen = vec!["MENU"];
        (pane.generation, layer.generation) = (Some(1), Some(1));
        let mut screen = Screen::default();
        screen.compose(20, 5, &layered(&pane, &layer), "team", &[], BAR, Look::default(), Over::Nothing);
        assert!(screen.canvas().row(1).contains("MENU"));
        // The pane under the layer draws something new; the layer, unchanged, is drawn over it again.
        (pane.screen, pane.generation) = (vec!["xxxxxxxx"], Some(2));
        screen.compose(20, 5, &layered(&pane, &layer), "team", &[], BAR, Look::default(), Over::Nothing);
        assert!(screen.canvas().row(1).contains("MENU"), "{:?}", screen.canvas().row(1));
        as_whole(&screen, &layered(&pane, &layer));
    }

    /// One pane alone, as wide as the screen.
    fn alone(a: &Lines) -> [View<'_>; 1] {
        let [one, _] = two(a, a, 0);
        [View { area: Rect { x: 0, y: 0, width: 20, height: 4 }, ..one }]
    }

    #[test]
    fn another_layout_draws_it_all() {
        let (mut a, mut b) = (left(), right());
        (a.generation, b.generation) = (Some(1), Some(1));
        let mut screen = Screen::default();
        let tabs = [Tab { title: "a long tab title", active: true, state: State::Working }];
        screen.compose(20, 5, &two(&a, &b, 0), "team", &tabs, BAR, Look::default(), Over::Nothing);
        // One pane alone, wider: nothing left of the other, nor of the longer tab.
        let short = [Tab { title: "t", active: true, state: State::Working }];
        screen.compose(20, 5, &alone(&a), "team", &short, BAR, Look::default(), Over::Nothing);
        let mut fresh = Canvas::new(20, 5);
        compose(&mut fresh, &alone(&a), "team", &short, BAR, Look::default(), Over::Nothing);
        for y in 0..5 {
            assert_eq!(screen.canvas().row(y), fresh.row(y), "row {y}");
        }
        // A panic in the middle of a frame: the next one is drawn whole.
        screen.composing = true;
        a.screen = vec!["after"];
        screen.compose(20, 5, &alone(&a), "team", &short, BAR, Look::default(), Over::Nothing);
        assert_eq!(first(screen.canvas()), "after  ");
    }

    /// An engine whose every cell shows one character, with a fixed generation (reviewer's).
    struct Fill(char, Option<u64>);

    impl Engine for Fill {
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
        fn draw(&self, canvas: &mut Canvas, area: Rect) {
            for y in 0..area.height {
                for x in 0..area.width {
                    canvas.put_cell(area.x + x, area.y + y, self.0.encode_utf8(&mut [0; 4]), 1, Style::PLAIN);
                }
            }
        }
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
        fn generation(&self) -> Option<u64> {
            self.1
        }
    }

    /// Two panes side by side, the menu over both: the border between them crosses the menu's cells.
    fn under_a_menu<'a>(a: &'a Fill, c: &'a Fill, menu: &'a Fill) -> [View<'a>; 3] {
        let view = |id: &'a str, engine: &'a dyn Engine, area, focused| View {
            id,
            engine,
            area,
            header: Header { name: id, color: None, state: State::Idle, note: None, scroll: None, hint: None },
            focused,
            selection: None,
        };
        [
            view("a", a, Rect { x: 0, y: 0, width: 10, height: 10 }, false),
            view("c", c, Rect { x: 10, y: 0, width: 10, height: 10 }, false),
            view("m", menu, Rect { x: 5, y: 2, width: 10, height: 6 }, true),
        ]
    }

    #[test]
    fn a_layer_keeps_its_cells_over_the_frames_under_it() {
        let (a, c, menu) = (Fill('a', Some(1)), Fill('c', Some(1)), Fill('m', Some(1)));
        let bar = Rect { x: 0, y: 10, width: 20, height: 1 };
        let mut screen = Screen::default();
        screen.compose(20, 11, &under_a_menu(&a, &c, &menu), "t", &[], bar, Look::default(), Over::Nothing);
        let first = screen.canvas().row(4);
        // Nothing changed: the frames under the menu drawn again, the menu over them.
        screen.compose(20, 11, &under_a_menu(&a, &c, &menu), "t", &[], bar, Look::default(), Over::Nothing);
        assert_eq!(screen.canvas().row(4), first, "the frames under the menu must not show through it");
        // A pane under it drew something new: the menu over it all the same.
        let a = Fill('a', Some(2));
        screen.compose(20, 11, &under_a_menu(&a, &c, &menu), "t", &[], bar, Look::default(), Over::Nothing);
        assert_eq!(screen.canvas().row(4), first);
    }

    #[test]
    fn a_selection_follows_its_view() {
        let (mut a, b) = (left(), right());
        a.generation = Some(1);
        fn selected<'a>(a: &'a Lines, b: &'a Lines) -> [View<'a>; 2] {
            let mut views = two(a, b, 1);
            views[0].selection = Some((Point { line: 2, col: 0 }, Point { line: 2, col: 3 }));
            views
        }
        let mut screen = Screen::default();
        screen.compose(20, 5, &selected(&a, &b), "team", &[], BAR, Look::default(), Over::Nothing);
        assert!(screen.canvas().style(1, 1).reverse && !screen.canvas().style(1, 2).reverse);
        // The view moved a line up, its generation as it was: the selection a row lower all the same.
        a.scroll = 1;
        screen.compose(20, 5, &selected(&a, &b), "team", &[], BAR, Look::default(), Over::Nothing);
        assert!(!screen.canvas().style(1, 1).reverse && screen.canvas().style(1, 2).reverse);
    }

    /// The frame as one drawn whole, on a new canvas, with `over`.
    fn same_as_whole(screen: &Screen, views: &[View<'_>], tabs: &[Tab<'_>], bar: Rect, over: Over<'_>) {
        let (width, height) = (screen.canvas().width(), screen.canvas().height());
        let mut fresh = Canvas::new(width, height);
        compose(&mut fresh, views, "t", tabs, bar, Look::default(), over);
        for y in 0..height {
            assert_eq!(screen.canvas().row(y), fresh.row(y), "row {y}");
            for x in 0..width {
                assert_eq!(screen.canvas().style(x, y), fresh.style(x, y), "column {x}, row {y}");
            }
        }
    }

    #[test]
    fn under_the_menu_dimmed() {
        let (a, c, menu) = (Fill('a', Some(1)), Fill('c', Some(1)), Fill('m', Some(1)));
        let bar = Rect { x: 0, y: 10, width: 20, height: 1 };
        let tabs = [Tab { title: "t", active: true, state: State::Idle }];
        let mut screen = Screen::default();
        let zones =
            screen.compose(20, 11, &under_a_menu(&a, &c, &menu), "t", &tabs, bar, Look::default(), Over::LastView);
        // The panes and the bar dimmed, the menu clear; nothing of the bar to click.
        assert_eq!(screen.canvas().style(1, 1), DIMMED);
        assert_eq!(screen.canvas().style(0, 10), DIMMED);
        assert_ne!(screen.canvas().style(7, 4), DIMMED);
        assert_eq!(screen.canvas().row(4).chars().skip(6).take(8).collect::<String>(), "mmmmmmmm");
        assert!(zones.bar.is_empty() && zones.options.is_empty());
        same_as_whole(&screen, &under_a_menu(&a, &c, &menu), &tabs, bar, Over::LastView);
        // The menu closed, the panes' generations as they were: nothing stays dimmed.
        let [a_view, c_view, _] = under_a_menu(&a, &c, &menu);
        let views = [a_view, c_view];
        let zones = screen.compose(20, 11, &views, "t", &tabs, bar, Look::default(), Over::Nothing);
        assert_ne!(screen.canvas().style(1, 1), DIMMED);
        assert!(!zones.bar.is_empty());
        same_as_whole(&screen, &views, &tabs, bar, Over::Nothing);
    }

    #[test]
    fn under_a_layer_no_cursor_but_its_own() {
        let (a, b) = (left(), right());
        let menu = Fill('m', Some(1));
        let [one, two_] = two(&a, &b, 0);
        let layer = View {
            id: "m",
            engine: &menu,
            area: Rect { x: 12, y: 0, width: 8, height: 3 },
            header: header("m"),
            focused: false,
            selection: None,
        };
        let mut screen = Screen::default();
        screen.compose(20, 5, &[one, two_, layer], "t", &[], BAR, Look::default(), Over::LastView);
        assert_eq!(screen.canvas().cursor(), None, "the focused pane is under the menu");
    }

    #[test]
    fn a_choice_over_the_team() {
        let (a, c) = (Fill('a', Some(1)), Fill('c', Some(1)));
        let [a_view, c_view, _] = under_a_menu(&a, &c, &a);
        let views = [a_view, c_view];
        let choice = chrome::quit_choice("mux");
        let bar = Rect { x: 0, y: 10, width: 20, height: 1 };
        let mut screen = Screen::default();
        let zones = screen.compose(20, 11, &views, "t", &[], bar, Look::default(), Over::Choice(&choice));
        let area = chrome::choice_area(Rect { x: 0, y: 0, width: 20, height: 10 }, &choice);
        // An option to click for each, within the choice; the rest dimmed, the bar too.
        assert_eq!(zones.options.len(), choice.options.len());
        assert!(zones.options.iter().all(|(rect, _)| overlap(rect, &area)));
        assert!(zones.bar.is_empty());
        assert_eq!(screen.canvas().style(0, 10), DIMMED);
        assert_eq!(screen.canvas().cursor(), None);
        same_as_whole(&screen, &views, &[], bar, Over::Choice(&choice));
        // Gone: the team as it was.
        screen.compose(20, 11, &views, "t", &[], bar, Look::default(), Over::Nothing);
        same_as_whole(&screen, &views, &[], bar, Over::Nothing);
    }

    #[test]
    fn erased_blanks_show_as_written() {
        // Frames of a kept canvas, long blank runs among them (erased by ECH), painted for a terminal that is Ghostty's
        // own engine: it shows each frame's text, and the default background where the canvas has it.
        use crossterm::style::Color;
        struct Rng(u64);
        impl Rng {
            fn n(&mut self, m: usize) -> usize {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                (self.0 % m as u64) as usize
            }
        }
        let mut r = Rng(0x9e37_79b9_7f4a_7c15);
        let backgrounds = [None, None, Some(Color::AnsiValue(236)), Some(Color::Rgb { r: 20, g: 40, b: 60 })];
        for _ in 0..60 {
            let (w, h) = (20 + r.n(60), 1 + r.n(8));
            let area = Rect { x: 0, y: 0, width: w, height: h };
            let mut canvas = Canvas::new(w, h);
            let mut painter =
                Painter::new(Features { sync: true, links: true, graphemes: true, ..Features::default() });
            let mut terminal = crate::mux::engine::new(w as u16, h as u16, 0).unwrap();
            let mut ops: Vec<String> = Vec::new();
            for _ in 0..6 {
                canvas.advance();
                ops.push("--".into());
                for _ in 0..r.n(8) {
                    let (x, y) = (r.n(w), r.n(h));
                    let mut style = Style { bg: backgrounds[r.n(4)], reverse: r.n(6) == 0, ..Style::PLAIN };
                    // What shows on a blank: these are never erased.
                    match r.n(8) {
                        0 => style.underline = true,
                        1 => style.strike = true,
                        2 => style.link = Some(canvas.link("https://a.example")),
                        _ => {}
                    }
                    let op = r.n(5);
                    ops.push(format!("{op} {x},{y} bg {:?} rev {}", style.bg, style.reverse));
                    match op {
                        0 => canvas.fill(x, y, r.n(w) + 1, r.n(h) + 1, style),
                        1 => canvas.fill(x, y, r.n(w) + 1, 1, Style { bg: None, reverse: false, ..style }),
                        2 => canvas.invert(x, y, r.n(w) + 1),
                        _ => {
                            let text = ["word", "a longer line of text", "x", "日本"][r.n(4)];
                            canvas.put(x, y, text, Style { fg: Some(Color::Cyan), ..style });
                        }
                    }
                }
                terminal.feed(&painter.frame(&canvas), &mut Vec::new());
                let mut seen = Canvas::new(w, h);
                terminal.draw(&mut seen, area);
                for y in 0..h {
                    for x in 0..w {
                        let (want, got) = (canvas.style(x, y), seen.style(x, y));
                        assert_eq!(seen.cell(x, y), canvas.cell(x, y), "column {x}, row {y}: {:?}", seen.row(y));
                        // A wide character's second column shows in its first one's style.
                        if canvas.cell(x, y).is_empty() {
                            continue;
                        }
                        let shows = |s: Style| (s.bg.is_some(), s.reverse, s.underline, s.strike, s.link.is_some());
                        assert_eq!(
                            shows(got),
                            shows(want),
                            "column {x}, row {y} of {w}×{h}: {want:?} {got:?} {ops:#?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_frame_then_nothing() {
        let (a, b) = (left(), right());
        let draw = |focus: usize| {
            let mut canvas = Canvas::new(20, 5);
            compose(&mut canvas, &two(&a, &b, focus), "team", &[], BAR, Look::default(), Over::Nothing);
            canvas
        };
        let mut painter = Painter::default();
        painter.frame(&draw(0));
        assert!(painter.frame(&draw(0)).is_empty(), "nothing changed, nothing sent");
        // The focus moves: the frames and the cursor, not the panes' text.
        let moved = String::from_utf8(painter.frame(&draw(1))).unwrap();
        assert!(!moved.is_empty() && !moved.contains("ls") && !moved.contains("hello"), "{moved:?}");
    }
}
