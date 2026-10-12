// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;
use crate::t;
use crate::ui::canvas::{CursorShape, Features, Painter, Style};
use crate::ui::look::State;

/// A frame drawn whole into `canvas`: a screen with nothing kept, which draws everything. [`Screen::compose`] on a
/// kept one must come to the same.
fn compose(canvas: &mut Canvas, views: &[View<'_>], scene: &Scene<'_>) -> Zones {
    let mut screen = Screen::default();
    let zones = screen.compose(canvas.width(), canvas.height(), views, scene);
    canvas.clone_from(screen.canvas());
    zones
}

/// An engine that shows lines of text: `history` above the screen, `screen` on it.
struct Lines {
    history: Vec<&'static str>,
    screen: Vec<&'static str>,
    cursor: Option<Cursor>,
    scroll: usize,
    /// What `generation` says; `None`, like an engine that does not tell.
    generation: Option<u64>,
}

impl Lines {
    fn scroll(&mut self, lines: isize) {
        self.scroll = self.scroll.saturating_add_signed(lines).min(self.history.len());
    }
}

impl Surface for Lines {
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
    Header { name, state: State::Working, ..Header::default() }
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
            bare: false,
        },
        View {
            id: "two",
            engine: b,
            area: Rect { x: 10, y: 0, width: 10, height: 4 },
            header: header("two"),
            focused: focus == 1,
            selection: None,
            bare: false,
        },
    ]
}

const BAR: Rect = Rect { x: 0, y: 4, width: 20, height: 1 };

#[test]
fn panes_in_their_frames() {
    let (a, b) = (left(), right());
    let mut canvas = Canvas::new(20, 5);
    let tabs = [Tab { title: "A", active: true, state: State::Working, zoomed: None, switch: None }];
    let zones = compose(
        &mut canvas,
        &two(&a, &b, 0),
        &Scene {
            team: "team",
            tabs: &tabs,
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
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
    compose(
        &mut canvas,
        &two(&a, &b, 1),
        &Scene {
            team: "team",
            tabs: &tabs,
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert_eq!(canvas.cursor(), Some(Cursor { x: 18, y: 1, ..CURSOR }));
    // No bar: nothing drawn on the last row, nothing to click.
    let mut canvas = Canvas::new(20, 5);
    assert!(
        compose(
            &mut canvas,
            &two(&a, &b, 1),
            &Scene {
                team: "team",
                tabs: &tabs,
                bar: Rect::default(),
                hover: None,
                look: Look::default(),
                over: Over::Nothing,
                notice: None
            }
        )
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
    compose(
        &mut canvas,
        &views,
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
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
    compose(
        &mut canvas,
        &views,
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
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
    compose(
        &mut canvas,
        &views,
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    let up = t!("↑ {} sur {}", "↑ {} of {}", 1, 2);
    assert!(canvas.row(0).contains(&up), "the scroll in the header: {:?}", canvas.row(0));
    assert_eq!(canvas.row(1).chars().skip(1).take(5).collect::<String>(), "old 2");
    // Scrolled, the focused pane shows no cursor.
    assert_eq!(canvas.cursor(), None);
    // After the pane's own note.
    views[0].header.note = Some("exited");
    let mut canvas = Canvas::new(50, 5);
    compose(
        &mut canvas,
        &views,
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert!(canvas.row(0).contains(&format!("exited · {up}")), "{:?}", canvas.row(0));
    // At the bottom of its history, nothing.
    a.scroll(-1);
    let views = two(&a, &b, 0);
    let mut canvas = Canvas::new(50, 5);
    compose(
        &mut canvas,
        &views,
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
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
    screen.compose(
        20,
        5,
        &two(&a, &b, 0),
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert_eq!(first(screen.canvas()), "$ ls   ");
    // Its text changed, its generation did not: the cells drawn before stay (this is what tells it was kept).
    a.screen = vec!["changed"];
    screen.compose(
        20,
        5,
        &two(&a, &b, 0),
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert_eq!(first(screen.canvas()), "$ ls   ");
    // The cursor is set at every frame all the same.
    assert_eq!(screen.canvas().cursor(), Some(Cursor { x: 3, y: 2, ..CURSOR }));
    // A new generation: drawn again, the old text gone.
    a.generation = Some(2);
    screen.compose(
        20,
        5,
        &two(&a, &b, 0),
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert_eq!(first(screen.canvas()), "changed");
    assert_eq!(screen.canvas().row(2).chars().skip(1).take(3).collect::<String>(), "   ");
    // Another selection: drawn again.
    let mut views = two(&a, &b, 0);
    views[0].selection = Some((Point { line: 2, col: 0 }, Point { line: 2, col: 1 }));
    screen.compose(
        20,
        5,
        &views,
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert!(screen.canvas().style(1, 1).reverse);
    // An engine that does not tell: drawn every time.
    let mut b = right();
    screen.compose(
        20,
        5,
        &two(&a, &b, 0),
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    b.screen = vec!["> bye"];
    screen.compose(
        20,
        5,
        &two(&a, &b, 0),
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
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
            bare: false,
        },
        View {
            id: "menu",
            engine: layer,
            area: Rect { x: 2, y: 0, width: 8, height: 3 },
            header: header("menu"),
            focused: false,
            selection: None,
            bare: false,
        },
    ]
}

/// The same frame as one drawn whole, on a new canvas.
fn as_whole(screen: &Screen, views: &[View<'_>]) {
    let mut fresh = Canvas::new(20, 5);
    compose(
        &mut fresh,
        views,
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
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
    screen.compose(
        20,
        5,
        &layered(&pane, &layer),
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert!(screen.canvas().row(1).contains("MENU"));
    // The pane under the layer draws something new; the layer, unchanged, is drawn over it again.
    (pane.screen, pane.generation) = (vec!["xxxxxxxx"], Some(2));
    screen.compose(
        20,
        5,
        &layered(&pane, &layer),
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
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
    let tabs = [Tab { title: "a long tab title", active: true, state: State::Working, zoomed: None, switch: None }];
    screen.compose(
        20,
        5,
        &two(&a, &b, 0),
        &Scene {
            team: "team",
            tabs: &tabs,
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    // One pane alone, wider: nothing left of the other, nor of the longer tab.
    let short = [Tab { title: "t", active: true, state: State::Working, zoomed: None, switch: None }];
    screen.compose(
        20,
        5,
        &alone(&a),
        &Scene {
            team: "team",
            tabs: &short,
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    let mut fresh = Canvas::new(20, 5);
    compose(
        &mut fresh,
        &alone(&a),
        &Scene {
            team: "team",
            tabs: &short,
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    for y in 0..5 {
        assert_eq!(screen.canvas().row(y), fresh.row(y), "row {y}");
    }
    // A panic in the middle of a frame: the next one is drawn whole.
    screen.composing = true;
    a.screen = vec!["after"];
    screen.compose(
        20,
        5,
        &alone(&a),
        &Scene {
            team: "team",
            tabs: &short,
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert_eq!(first(screen.canvas()), "after  ");
}

/// An engine whose every cell shows one character, with a fixed generation.
struct Fill(char, Option<u64>);

impl Surface for Fill {
    fn draw(&self, canvas: &mut Canvas, area: Rect) {
        for y in 0..area.height {
            for x in 0..area.width {
                canvas.put_cell(area.x + x, area.y + y, self.0.encode_utf8(&mut [0; 4]), 1, Style::PLAIN);
            }
        }
    }
    fn generation(&self) -> Option<u64> {
        self.1
    }
}

/// Two panes side by side, the menu over both: the border between them crosses the menu's cells.
fn under_a_menu<'a>(a: &'a Fill, c: &'a Fill, menu: &'a Fill) -> [View<'a>; 3] {
    let view = |id: &'a str, engine: &'a dyn Surface, area, focused| View {
        id,
        engine,
        area,
        header: Header { name: id, state: State::Idle, ..Header::default() },
        focused,
        selection: None,
        bare: false,
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
    screen.compose(
        20,
        11,
        &under_a_menu(&a, &c, &menu),
        &Scene { team: "t", tabs: &[], bar, hover: None, look: Look::default(), over: Over::Nothing, notice: None },
    );
    let first = screen.canvas().row(4);
    // Nothing changed: the frames under the menu drawn again, the menu over them.
    screen.compose(
        20,
        11,
        &under_a_menu(&a, &c, &menu),
        &Scene { team: "t", tabs: &[], bar, hover: None, look: Look::default(), over: Over::Nothing, notice: None },
    );
    assert_eq!(screen.canvas().row(4), first, "the frames under the menu must not show through it");
    // A pane under it drew something new: the menu over it all the same.
    let a = Fill('a', Some(2));
    screen.compose(
        20,
        11,
        &under_a_menu(&a, &c, &menu),
        &Scene { team: "t", tabs: &[], bar, hover: None, look: Look::default(), over: Over::Nothing, notice: None },
    );
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
    screen.compose(
        20,
        5,
        &selected(&a, &b),
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert!(screen.canvas().style(1, 1).reverse && !screen.canvas().style(1, 2).reverse);
    // The view moved a line up, its generation as it was: the selection a row lower all the same.
    a.scroll = 1;
    screen.compose(
        20,
        5,
        &selected(&a, &b),
        &Scene {
            team: "team",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert!(!screen.canvas().style(1, 1).reverse && screen.canvas().style(1, 2).reverse);
}

/// The frame as one drawn whole, on a new canvas, with `over`.
fn same_as_whole(screen: &Screen, views: &[View<'_>], tabs: &[Tab<'_>], bar: Rect, over: Over<'_>) {
    same_as_whole_with(screen, views, tabs, bar, over, None);
}

/// The same, with a notice.
fn same_as_whole_with(
    screen: &Screen,
    views: &[View<'_>],
    tabs: &[Tab<'_>],
    bar: Rect,
    over: Over<'_>,
    notice: Option<&Notice>,
) {
    let (width, height) = (screen.canvas().width(), screen.canvas().height());
    let mut fresh = Canvas::new(width, height);
    compose(&mut fresh, views, &Scene { team: "t", tabs, bar, hover: None, look: Look::default(), over, notice });
    for y in 0..height {
        assert_eq!(screen.canvas().row(y), fresh.row(y), "row {y}");
        for x in 0..width {
            assert_eq!(screen.canvas().style(x, y), fresh.style(x, y), "column {x}, row {y}");
        }
    }
    assert_eq!(screen.canvas().cursor(), fresh.cursor());
}

#[test]
fn under_the_menu_dimmed() {
    let (a, c, menu) = (Fill('a', Some(1)), Fill('c', Some(1)), Fill('m', Some(1)));
    let bar = Rect { x: 0, y: 10, width: 20, height: 1 };
    let tabs = [Tab { title: "t", active: true, state: State::Idle, zoomed: None, switch: None }];
    let mut screen = Screen::default();
    let zones = screen.compose(
        20,
        11,
        &under_a_menu(&a, &c, &menu),
        &Scene { team: "t", tabs: &tabs, bar, hover: None, look: Look::default(), over: Over::LastView, notice: None },
    );
    // The panes and the bar dimmed, the menu clear; nothing of the bar to click.
    assert_eq!(screen.canvas().style(1, 1), dimmed());
    assert_eq!(screen.canvas().style(0, 10), dimmed());
    assert_ne!(screen.canvas().style(7, 4), dimmed());
    assert_eq!(screen.canvas().row(4).chars().skip(6).take(8).collect::<String>(), "mmmmmmmm");
    assert!(zones.bar.is_empty() && zones.options.is_empty());
    same_as_whole(&screen, &under_a_menu(&a, &c, &menu), &tabs, bar, Over::LastView);
    // The menu closed, the panes' generations as they were: nothing stays dimmed.
    let [a_view, c_view, _] = under_a_menu(&a, &c, &menu);
    let views = [a_view, c_view];
    let zones = screen.compose(
        20,
        11,
        &views,
        &Scene { team: "t", tabs: &tabs, bar, hover: None, look: Look::default(), over: Over::Nothing, notice: None },
    );
    assert_ne!(screen.canvas().style(1, 1), dimmed());
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
        bare: false,
    };
    let mut screen = Screen::default();
    screen.compose(
        20,
        5,
        &[one, two_, layer],
        &Scene {
            team: "t",
            tabs: &[],
            bar: BAR,
            hover: None,
            look: Look::default(),
            over: Over::LastView,
            notice: None,
        },
    );
    assert_eq!(screen.canvas().cursor(), None, "the focused pane is under the menu");
}

#[test]
fn a_choice_over_the_team() {
    let (a, c) = (Fill('a', Some(1)), Fill('c', Some(1)));
    let [a_view, c_view, _] = under_a_menu(&a, &c, &a);
    let views = [a_view, c_view];
    let choice = chrome::quit_choice();
    let bar = Rect { x: 0, y: 10, width: 20, height: 1 };
    let mut screen = Screen::default();
    let zones = screen.compose(
        20,
        11,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar,
            hover: None,
            look: Look::default(),
            over: Over::Choice(&choice),
            notice: None,
        },
    );
    let area = chrome::choice_area(Rect { x: 0, y: 0, width: 20, height: 10 }, &choice);
    // An option to click for each, within the choice; the rest dimmed, the bar too.
    assert_eq!(zones.options.len(), choice.options.len());
    assert!(zones.options.iter().all(|(rect, _)| overlap(rect, &area)));
    assert!(zones.bar.is_empty());
    assert_eq!(screen.canvas().style(0, 10), dimmed());
    assert_eq!(screen.canvas().cursor(), None);
    same_as_whole(&screen, &views, &[], bar, Over::Choice(&choice));
    // Gone: the team as it was.
    screen.compose(
        20,
        11,
        &views,
        &Scene { team: "t", tabs: &[], bar, hover: None, look: Look::default(), over: Over::Nothing, notice: None },
    );
    same_as_whole(&screen, &views, &[], bar, Over::Nothing);
}

#[test]
fn erased_blanks_show_as_written() {
    // Frames of a kept canvas, long blank runs among them (erased by ECH), painted for a terminal that is Ghostty's
    // own engine: it shows each frame's text, and the default background where the canvas has it.
    use crate::ui::canvas::rng::Rng;
    use crossterm::style::Color;
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let backgrounds = [None, None, Some(Color::AnsiValue(236)), Some(Color::Rgb { r: 20, g: 40, b: 60 })];
    for _ in 0..60 {
        let (w, h) = (20 + r.n(60), 1 + r.n(8));
        let area = Rect { x: 0, y: 0, width: w, height: h };
        let mut canvas = Canvas::new(w, h);
        let mut painter = Painter::new(Features { sync: true, links: true, graphemes: true, ..Features::default() });
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
                    assert_eq!(shows(got), shows(want), "column {x}, row {y} of {w}×{h}: {want:?} {got:?} {ops:#?}");
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
        compose(
            &mut canvas,
            &two(&a, &b, focus),
            &Scene {
                team: "team",
                tabs: &[],
                bar: BAR,
                hover: None,
                look: Look::default(),
                over: Over::Nothing,
                notice: None,
            },
        );
        canvas
    };
    let mut painter = Painter::default();
    painter.frame(&draw(0));
    assert!(painter.frame(&draw(0)).is_empty(), "nothing changed, nothing sent");
    // The focus moves: the frames and the cursor, not the panes' text.
    let moved = String::from_utf8(painter.frame(&draw(1))).unwrap();
    assert!(!moved.is_empty() && !moved.contains("ls") && !moved.contains("hello"), "{moved:?}");
}

fn waiting(member: &str) -> Notice {
    Notice {
        member: member.into(),
        state: crate::ui::look::State::Waiting,
        color: None,
        text: chrome::notice_text(member),
        tab: "Agents (1)".into(),
    }
}

/// Two panes side by side on a 160 × 12 screen, the bar on its last row; the notice over the right one.
const WIDE_BAR: Rect = Rect { x: 0, y: 11, width: 160, height: 1 };

fn wide<'a>(a: &'a dyn Surface, b: &'a dyn Surface, focus: usize) -> [View<'a>; 2] {
    let view = |id: &'a str, engine: &'a dyn Surface, x: usize, focused: bool| View {
        id,
        engine,
        area: Rect { x, y: 0, width: 80, height: 11 },
        header: Header { name: id, color: Some(Color::Cyan), state: State::Idle, ..Header::default() },
        focused,
        selection: None,
        bare: false,
    };
    [view("a", a, 0, focus == 0), view("b", b, 80, focus == 1)]
}

/// Where the notice goes on the wide screen.
fn notice_at(notice: &Notice) -> Rect {
    chrome::notice_area(team_area(160, 12, WIDE_BAR), notice)
}

/// The rows a painter's bytes write to (by their cursor positions).
fn rows_written(bytes: &[u8]) -> Vec<usize> {
    let text = String::from_utf8_lossy(bytes);
    let mut rows = Vec::new();
    for part in text.split("\x1b[").skip(1) {
        let Some(end) = part.find(|c: char| !(c.is_ascii_digit() || c == ';')) else { continue };
        if part[end..].starts_with('H')
            && let Some((row, _)) = part[..end].split_once(';')
            && let Ok(row) = row.parse::<usize>()
        {
            rows.push(row - 1);
        }
    }
    rows.sort_unstable();
    rows.dedup();
    rows
}

#[test]
fn a_notice_over_everything() {
    let (a, b) = (Fill('a', Some(1)), Fill('b', Some(1)));
    let notice = waiting("dev-saisie");
    let area = notice_at(&notice);
    let tabs = [Tab { title: "t", active: true, state: State::Idle, zoomed: None, switch: None }];
    let mut screen = Screen::default();
    let views = wide(&a, &b, 0);
    let zones = screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &tabs,
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: Some(&notice),
        },
    );
    assert_eq!(zones.notice, Some(area));
    assert!(area.y + area.height <= WIDE_BAR.y, "over the team, not the bar: {area:?}");
    assert!(screen.canvas().row(area.y + 1).contains("dev-saisie"), "{:?}", screen.canvas().row(area.y + 1));
    same_as_whole_with(&screen, &views, &tabs, WIDE_BAR, Over::Nothing, Some(&notice));
    // Over a choice, and the menu, as clear: never dimmed.
    let choice = chrome::quit_choice();
    for over in [Over::Choice(&choice), Over::LastView] {
        let zones = screen.compose(
            160,
            12,
            &views,
            &Scene {
                team: "t",
                tabs: &tabs,
                bar: WIDE_BAR,
                hover: None,
                look: Look::default(),
                over,
                notice: Some(&notice),
            },
        );
        assert_eq!(zones.notice, Some(area));
        let dimmed = (area.x..area.x + area.width)
            .flat_map(|x| (area.y..area.y + area.height).map(move |y| (x, y)))
            .filter(|&(x, y)| screen.canvas().style(x, y) == dimmed())
            .count();
        assert!(dimmed < area.width * area.height / 2, "{dimmed} dimmed cells in the notice under {over:?}");
        same_as_whole_with(&screen, &views, &tabs, WIDE_BAR, over, Some(&notice));
    }
}

#[test]
fn a_notice_that_goes_draws_again_only_what_it_covered() {
    let (a, b) = (Fill('a', Some(1)), Fill('b', Some(1)));
    let notice = waiting("dev-saisie");
    let area = notice_at(&notice);
    assert!(!overlap(&area, &wide(&a, &b, 0)[0].area), "the notice over the right pane only: {area:?}");
    let mut screen = Screen::default();
    let mut painter = Painter::new(Features { sync: true, ..Features::default() });
    screen.compose(
        160,
        12,
        &wide(&a, &b, 0),
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: Some(&notice),
        },
    );
    painter.frame(screen.canvas());
    // Nothing changed: nothing sent, the notice kept as it was.
    screen.compose(
        160,
        12,
        &wide(&a, &b, 0),
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: Some(&notice),
        },
    );
    assert!(painter.frame(screen.canvas()).is_empty());
    // Gone: the team as it is, and only the notice's rows sent.
    let zones = screen.compose(
        160,
        12,
        &wide(&a, &b, 0),
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert_eq!(zones.notice, None);
    same_as_whole_with(&screen, &wide(&a, &b, 0), &[], WIDE_BAR, Over::Nothing, None);
    let rows = rows_written(&painter.frame(screen.canvas()));
    assert!(!rows.is_empty() && rows.iter().all(|row| (area.y..area.y + area.height).contains(row)), "{rows:?}");
    // The pane it did not cover kept its cells (they say what its engine drew before), the other drawn again.
    screen.compose(
        160,
        12,
        &wide(&a, &b, 0),
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: Some(&notice),
        },
    );
    let (a, b) = (Fill('A', Some(1)), Fill('B', Some(1)));
    screen.compose(
        160,
        12,
        &wide(&a, &b, 0),
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert_eq!(screen.canvas().row(5).chars().nth(1), Some('a'));
    assert_eq!(screen.canvas().row(5).chars().nth(81), Some('B'));
    // Over no view (an empty tab): blank again.
    let mut screen = Screen::default();
    screen.compose(
        160,
        12,
        &[],
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: Some(&notice),
        },
    );
    screen.compose(
        160,
        12,
        &[],
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    same_as_whole_with(&screen, &[], &[], WIDE_BAR, Over::Nothing, None);
}

#[test]
fn a_notice_that_moves() {
    let (a, b) = (Fill('a', Some(1)), Fill('b', Some(1)));
    let (long, short) = (waiting("a-very-long-member-name"), waiting("x"));
    assert_ne!(notice_at(&long), notice_at(&short));
    let mut screen = Screen::default();
    let views = wide(&a, &b, 0);
    screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: Some(&long),
        },
    );
    let zones = screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: Some(&short),
        },
    );
    assert_eq!(zones.notice, Some(notice_at(&short)));
    same_as_whole_with(&screen, &views, &[], WIDE_BAR, Over::Nothing, Some(&short));
    // Under a menu too, dimmed but for it and the notice.
    screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::LastView,
            notice: Some(&long),
        },
    );
    screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::LastView,
            notice: Some(&short),
        },
    );
    same_as_whole_with(&screen, &views, &[], WIDE_BAR, Over::LastView, Some(&short));
    screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::LastView,
            notice: None,
        },
    );
    same_as_whole_with(&screen, &views, &[], WIDE_BAR, Over::LastView, None);
}

#[test]
fn no_cursor_under_the_notice() {
    let notice = waiting("dev-saisie");
    let area = notice_at(&notice);
    let a = Fill('a', Some(1));
    // The right pane's cells start at column 81, row 1: its cursor in the notice's first cell.
    let mut b = right();
    b.cursor = Some(Cursor { x: area.x - 81, y: area.y - 1, ..CURSOR });
    let mut screen = Screen::default();
    screen.compose(
        160,
        12,
        &wide(&a, &b, 1),
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    assert_eq!(screen.canvas().cursor().map(|c| (c.x, c.y)), Some((area.x, area.y)));
    screen.compose(
        160,
        12,
        &wide(&a, &b, 1),
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: Some(&notice),
        },
    );
    assert_eq!(screen.canvas().cursor(), None);
}

#[test]
fn a_hover_sends_its_rows_only() {
    let (a, b) = (Fill('a', Some(1)), Fill('b', Some(1)));
    let tabs = [
        Tab { title: "one", active: true, state: State::Idle, zoomed: None, switch: None },
        Tab { title: "two", active: false, state: State::Idle, zoomed: None, switch: None },
    ];
    let frame = |screen: &mut Screen, painter: &mut Painter, part: Option<Part>, target: Option<Target>| {
        let mut views = wide(&a, &b, 0);
        views[1].header.hover = part;
        screen.compose(
            160,
            12,
            &views,
            &Scene {
                team: "t",
                tabs: &tabs,
                bar: WIDE_BAR,
                hover: target,
                look: Look::default(),
                over: Over::Nothing,
                notice: None,
            },
        );
        painter.frame(screen.canvas())
    };
    let mut screen = Screen::default();
    let mut painter = Painter::new(Features { sync: true, ..Features::default() });
    frame(&mut screen, &mut painter, None, None);
    // The pointer moved over nothing new: nothing sent.
    assert!(frame(&mut screen, &mut painter, None, None).is_empty());
    // Over a name: its header's row; over a tab: the bar's.
    let name = rows_written(&frame(&mut screen, &mut painter, Some(Part::Name), None));
    assert_eq!(name, [0]);
    assert!(frame(&mut screen, &mut painter, Some(Part::Name), None).is_empty());
    // Off the name, onto a tab: the header as it was, the bar.
    let bar = rows_written(&frame(&mut screen, &mut painter, None, Some(Target::Tab(1))));
    assert_eq!(bar, [0, WIDE_BAR.y]);
    assert!(frame(&mut screen, &mut painter, None, Some(Target::Tab(1))).is_empty());
}

#[test]
fn the_parts_of_the_headers_by_their_view() {
    let (a, b) = (Fill('a', Some(1)), Fill('b', Some(1)));
    let mut views = wide(&a, &b, 0);
    for view in &mut views {
        view.header = Header {
            model: Some("Opus"),
            effort: Some("high"),
            context: Some(40),
            compactable: true,
            zoom: Some(chrome::Zoom::In),
            ..view.header
        };
    }
    let mut screen = Screen::default();
    let zones = screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    // Each view's, in its top border.
    for (i, view) in views.iter().enumerate() {
        let parts: Vec<_> = zones.parts.iter().filter(|(_, index, _)| *index == i).collect();
        assert!(parts.iter().any(|(_, _, part)| *part == Part::Name), "{:?}", zones.parts);
        assert!(parts.iter().all(|(rect, _, _)| rect.y == view.area.y && within(rect, &view.area)));
    }
    // Dimmed under a choice: none to click; under the menu, only the menu's own.
    let choice = chrome::quit_choice();
    let zones = screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Choice(&choice),
            notice: None,
        },
    );
    assert!(zones.parts.is_empty());
    let zones = screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::LastView,
            notice: None,
        },
    );
    assert!(!zones.parts.is_empty() && zones.parts.iter().all(|(_, index, _)| *index == 1), "{:?}", zones.parts);
}

#[test]
fn a_bare_view_is_all_cells() {
    let (a, b) = (Fill('a', Some(1)), Fill('b', Some(1)));
    let mut views = wide(&a, &b, 0);
    // The second one, over the first, with no frame: its `area` is exactly its cells.
    views[1].area = Rect { x: 10, y: 2, width: 6, height: 3 };
    views[1].bare = true;
    let mut screen = Screen::default();
    let zones = screen.compose(
        160,
        12,
        &views,
        &Scene {
            team: "t",
            tabs: &[],
            bar: WIDE_BAR,
            hover: None,
            look: Look::default(),
            over: Over::Nothing,
            notice: None,
        },
    );
    let canvas = screen.canvas();
    for (x, y) in [(10, 2), (15, 2), (10, 4), (15, 4)] {
        assert_eq!(canvas.cell(x, y), "b", "({x}, {y})");
    }
    // Just outside it, the first view's.
    for (x, y) in [(9, 2), (16, 2), (10, 5), (10, 1)] {
        assert_ne!(canvas.cell(x, y), "b", "({x}, {y})");
    }
    assert!(zones.parts.iter().all(|(_, index, _)| *index == 0));
    assert_eq!(views[1].inside(), views[1].area);
}

/// An engine that counts how many times it is drawn, with a fixed generation.
struct Counted(std::cell::Cell<usize>);

impl Surface for Counted {
    fn draw(&self, _: &mut Canvas, _: Rect) {
        self.0.set(self.0.get() + 1);
    }
    fn generation(&self) -> Option<u64> {
        Some(1)
    }
}

#[test]
fn a_new_theme_or_a_redraw_draws_the_kept_cells_again() {
    let engine = Counted(std::cell::Cell::new(0));
    let bar = Rect { x: 0, y: 10, width: 20, height: 1 };
    let view = || {
        [View {
            id: "a",
            engine: &engine,
            area: Rect { x: 0, y: 0, width: 20, height: 10 },
            header: header("a"),
            focused: false,
            selection: None,
            bare: false,
        }]
    };
    let mut screen = Screen::default();
    let frame = |screen: &mut Screen| {
        screen.compose(
            20,
            11,
            &view(),
            &Scene { team: "t", tabs: &[], bar, hover: None, look: Look::default(), over: Over::Nothing, notice: None },
        );
        engine.0.get()
    };
    assert_eq!(frame(&mut screen), 1);
    // Nothing changed: kept.
    assert_eq!(frame(&mut screen), 1);
    // The dimmed color of another theme: everything is drawn, for the cells kept under a layer.
    screen.faded = Some(Color::AnsiValue(1));
    assert_eq!(frame(&mut screen), 2);
    assert_eq!(frame(&mut screen), 2);
    // Asked for.
    screen.redraw();
    assert_eq!(frame(&mut screen), 3);
    assert_eq!(frame(&mut screen), 3);
}
