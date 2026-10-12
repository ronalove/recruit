// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Panes composed into one screen: each one's cells in its frame (`chrome.rs`), the bar on the last row, the cursor of
//! the pane with the focus, a notice over it all. The frame is then sent by difference (`canvas::Painter`).

use super::chrome::{self, Header, Look, Notice, Part, Tab, Target};
use super::engine::Surface;
use super::select::Point;
use crate::ui::Rect;
use crate::ui::canvas::{Canvas, Cursor, Style};
use crate::ui::look;
use crossterm::style::Color;

/// What shows over the team, everything else dimmed, the bar included.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Over<'a> {
    Nothing,
    /// The last of the views: the menu.
    LastView,
    /// A choice to make (quit, compact a member), in the middle of the team's area ([`chrome::choice_area`]).
    Choice(&'a chrome::Choice),
}

/// Where a frame can be clicked. What is dimmed takes no click: nothing of the bar while something shows over the
/// team, and only the parts of the headers in what stays clear. The notice is over everything else: it is looked
/// at first.
#[derive(Debug, Default)]
pub(crate) struct Zones {
    pub bar: Vec<(Rect, Target)>,
    /// A choice's options, by their index.
    pub options: Vec<(Rect, usize)>,
    /// The parts of the headers, with their view's index in `views`.
    pub parts: Vec<(Rect, usize, Part)>,
    /// Where the notice shows.
    pub notice: Option<Rect>,
}

/// What is under a layer: as the menu dims what is behind its dialogs, in the palette of the moment.
fn dimmed() -> Style {
    Style { fg: Some(look::palette().faded), ..Style::PLAIN }
}

/// A pane as the screen shows it.
pub(crate) struct View<'a> {
    /// The pane, as the server names them: [`Screen`] keeps its cells from one frame to the next by it.
    pub id: &'a str,
    pub engine: &'a dyn Surface,
    /// The pane's place, its frame included: its cells inside ([`content`]).
    pub area: Rect,
    /// What its frame says; how far up its history it shows is filled in (`Header::scroll`).
    pub header: Header<'a>,
    pub focused: bool,
    /// Its selected text (`select::Selection::range`), shown in reverse video.
    pub selection: Option<(Point, Point)>,
    /// No frame: `area` is exactly the place of its cells, and it has no header, so no clickable parts. For a pane
    /// that draws its own frame (the menu's dropdown lists).
    pub bare: bool,
}

impl View<'_> {
    /// Where the view's cells go: all of `area` when bare, else inside its frame ([`content`]).
    pub(crate) fn inside(&self) -> Rect {
        if self.bare { self.area } else { content(self.area) }
    }
}

/// Where a pane's cells go in its `area`: inside its frame. Its engine and its PTY have this size.
pub(crate) fn content(area: Rect) -> Rect {
    chrome::inside(area)
}

/// The team's place on a `width` × `height` screen: all of it but the bar.
fn team_area(width: usize, height: usize, bar: Rect) -> Rect {
    Rect { x: 0, y: 0, width, height: if bar.height > 0 { bar.y } else { height } }
}

/// What stays clear on a `width` × `height` screen when something is `over` the team: the menu's place, or the
/// choice's.
fn clear(width: usize, height: usize, views: &[View<'_>], bar: Rect, over: Over<'_>) -> Option<Rect> {
    match over {
        Over::Nothing => None,
        Over::LastView => views.last().map(|view| view.area),
        Over::Choice(choice) => Some(chrome::choice_area(team_area(width, height, bar), choice)),
    }
}

/// One view, the `index`-th: its cells when `cells` (else they are on the canvas already), its frame, whose parts
/// go to `parts`; its cursor on the screen when it has the focus and shows its live screen.
fn draw(
    canvas: &mut Canvas,
    view: &View<'_>,
    index: usize,
    cells: bool,
    look: Look,
    parts: &mut Vec<(Rect, usize, Part)>,
) -> Option<Cursor> {
    let area = view.area;
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let inside = view.inside();
    if cells && inside.width > 0 && inside.height > 0 {
        canvas.fill(inside.x, inside.y, inside.width, inside.height, Style::PLAIN);
        view.engine.draw(canvas, inside);
        if let Some(range) = view.selection {
            selected(canvas, inside, view.engine.top(), range);
        }
    }
    let scrolled = view.engine.scrolled();
    if !view.bare {
        let header =
            Header { scroll: (scrolled > 0).then(|| (scrolled, view.engine.history().max(scrolled))), ..view.header };
        let drawn = chrome::frame(canvas, area, &header, view.focused, look);
        parts.extend(drawn.into_iter().map(|(rect, part)| (rect, index, part)));
    }
    if !view.focused || scrolled > 0 {
        return None;
    }
    view.engine.cursor().filter(|c| c.x < inside.width && c.y < inside.height).map(|c| Cursor {
        x: inside.x + c.x,
        y: inside.y + c.y,
        ..c
    })
}

/// What a frame shows besides the panes: the bar on `bar` (the screen's last row; nothing when empty) with the team's
/// name, its tabs and the part of it `hover` is on, what is `over` the team, and last the `notice`, never dimmed.
pub(crate) struct Scene<'a> {
    pub team: &'a str,
    pub tabs: &'a [Tab<'a>],
    pub bar: Rect,
    pub hover: Option<Target>,
    pub look: Look,
    pub over: Over<'a>,
    pub notice: Option<&'a Notice>,
}

/// The cursor, the bar (its row blanked first, for a canvas kept from the frame before), then what is over the team:
/// everything outside `clear` dimmed, and the choice drawn; last the notice, in its place. Under a layer, the cursor
/// shows only in it; never under the notice.
fn finish(
    canvas: &mut Canvas,
    cursor: Option<Cursor>,
    mut parts: Vec<(Rect, usize, Part)>,
    scene: &Scene<'_>,
    clear: Option<Rect>,
    notice: Option<(&Notice, Rect)>,
) -> Zones {
    let Scene { look, over, .. } = *scene;
    let cursor = match over {
        Over::Nothing => cursor,
        Over::LastView => cursor.filter(|c| clear.is_some_and(|area| area.contains(c.x, c.y))),
        Over::Choice(_) => None,
    };
    canvas.set_cursor(cursor.filter(|c| !notice.is_some_and(|(_, area)| area.contains(c.x, c.y))));
    let mut zones = Zones::default();
    let row = scene.bar;
    if row.width > 0 && row.height > 0 {
        canvas.fill(row.x, row.y, row.width, row.height, Style::PLAIN);
        zones.bar = chrome::bar(canvas, row, scene.team, scene.tabs, look, scene.hover);
    }
    if let Some(area) = clear {
        canvas.fade_outside(area.x, area.y, area.width, area.height, dimmed());
        zones.bar.clear();
        match over {
            // The choice's box over whatever stays clear under it.
            Over::Choice(choice) => {
                parts.clear();
                zones.options = chrome::choice(canvas, area, choice);
            }
            _ => parts.retain(|(rect, _, _)| within(rect, &area)),
        }
    }
    zones.parts = parts;
    if let Some((notice, area)) = notice.filter(|(_, area)| area.width > 0 && area.height > 0) {
        chrome::notice(canvas, area, notice, look);
        zones.notice = Some(area);
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
/// engine drew nothing new since ([`Surface::generation`]), in the same place, with the same selection, and that
/// overlaps no view drawn before it (a layer is painted at every frame), keeps its cells rather than being painted
/// again. A new size, other panes, or another order: everything is drawn. A notice that goes or moves:
/// what it covered is drawn again, the rest kept.
#[derive(Default)]
pub(crate) struct Screen {
    canvas: Canvas,
    drawn: Vec<Drawn>,
    /// Set while a frame is composed: one that panicked leaves it set, and the next is drawn whole.
    composing: bool,
    /// What stayed clear under a layer: when it changes, comes or goes, everything is drawn (a cell kept dimmed, or
    /// one kept from under the layer's old place, would stay so).
    clear: Option<Rect>,
    /// Where the notice was drawn: kept cells there show it.
    notice: Option<Rect>,
    /// The color dimmed cells had in the last frame: another one (the theme changed) and everything is drawn, or the
    /// cells kept under a layer would keep the old one.
    faded: Option<Color>,
}

impl Screen {
    /// The frame for a `width` × `height` screen, the `views` drawn into [`Screen::canvas`] each in its frame, the
    /// focused one's cursor when it shows its live screen, and the `scene` around them. Returns where the frame can be
    /// clicked. Only what changed since the frame before is drawn again.
    pub(crate) fn compose(&mut self, width: usize, height: usize, views: &[View<'_>], scene: &Scene<'_>) -> Zones {
        let Scene { bar, look, over, notice, .. } = *scene;
        let clear = clear(width, height, views, bar, over);
        let notice = notice.map(|notice| (notice, chrome::notice_area(team_area(width, height, bar), notice)));
        let same = !self.composing
            && (self.canvas.width(), self.canvas.height()) == (width, height)
            && clear == self.clear
            && self.faded == Some(look::palette().faded)
            && self.drawn.len() == views.len()
            && self.drawn.iter().zip(views).all(|(drawn, view)| drawn.id == view.id && drawn.area == view.area);
        self.composing = true;
        // Where the notice was, when it went or moved: blanked, and the views under it painted again.
        let mut gone = None;
        if same {
            self.canvas.compact(ROOM);
            self.canvas.advance();
            gone = self.notice.filter(|&old| Some(old) != notice.map(|(_, area)| area));
            if let Some(old) = gone {
                self.canvas.fill(old.x, old.y, old.width, old.height, Style::PLAIN);
            }
        } else {
            self.canvas.reset(width, height);
            self.drawn.clear();
            self.drawn.resize(views.len(), Drawn::default());
        }
        self.clear = clear;
        self.faded = Some(look::palette().faded);
        self.notice = notice.map(|(_, area)| area);
        // The areas of the views drawn before: their frames are drawn again at every frame, and a view over one of
        // them (a layer) is painted again too, or a frame would show through it.
        let mut before: Vec<Rect> = Vec::new();
        let mut cursor = None;
        let mut parts = Vec::new();
        for (i, (view, drawn)) in views.iter().zip(&mut self.drawn).enumerate() {
            let generation = view.engine.generation();
            let top = view.selection.map(|_| view.engine.top());
            let keep = same
                && generation.is_some()
                && drawn.generation == generation
                && drawn.selection == view.selection
                && drawn.top == top
                && !gone.is_some_and(|old| overlap(&old, &view.area))
                && !before.iter().any(|area| overlap(area, &view.area));
            before.push(view.area);
            if !keep {
                if drawn.id != view.id {
                    drawn.id = view.id.to_string();
                }
                (drawn.area, drawn.generation, drawn.selection, drawn.top) =
                    (view.area, generation, view.selection, top);
            }
            cursor = draw(&mut self.canvas, view, i, !keep, look, &mut parts).or(cursor);
        }
        let zones = finish(&mut self.canvas, cursor, parts, scene, clear, notice);
        self.composing = false;
        zones
    }

    /// The next frame is drawn whole, whatever was kept: for what the screen cannot see change by itself.
    pub(crate) fn redraw(&mut self) {
        self.composing = true;
    }

    /// The last frame composed.
    pub(crate) fn canvas(&self) -> &Canvas {
        &self.canvas
    }
}

fn overlap(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

/// Whether `inner` is all inside `outer`.
fn within(inner: &Rect, outer: &Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.width <= outer.x + outer.width
        && inner.y + inner.height <= outer.y + outer.height
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
mod tests;
