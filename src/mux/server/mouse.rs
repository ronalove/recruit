// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The client's mouse: the bar, the headers, the panels, the cards, and the selection in a pane.

use std::time::Instant;

use super::choice::{Open, Purpose};
use super::focus_layout::DOUBLE_CLICK;
use super::lifecycle::log;
use super::menu::{LIST_SIZE, List};
use super::pane::Role;
use super::{Hover, Server, choice};
use crate::board;
use crate::mux::chrome::{self, Target};
use crate::mux::input::{self, Button, Mods, Mouse, MouseKind};
use crate::mux::proto::MenuField;
use crate::mux::screen;
use crate::mux::select::{self, Selection};
use crate::team::state::Snapshot;
use crate::ui::Rect;

/// Lines a notch of the wheel scrolls.
const WHEEL: isize = 3;

impl Server {
    /// What the pointer is over at `col`, `row`, among what a click reaches: lit when it changes. The notice
    /// first, which hides what is under it, then the bar, then the headers.
    fn hover_at(&mut self, col: usize, row: usize) {
        let hover = if self.notices.notice_zone.is_some_and(|zone| zone.contains(col, row)) {
            None
        } else if let Some((_, target)) = self.zones.iter().find(|(zone, _)| zone.contains(col, row)) {
            Some(Hover::Bar(*target))
        } else {
            let part = self.parts.iter().find(|(zone, _, _)| zone.contains(col, row));
            part.map(|(_, id, part)| Hover::Part(id.clone(), *part))
        };
        if hover != self.pointer.hover {
            self.pointer.hover = hover;
            self.dirty = true;
        }
    }

    /// A press on a part of pane `id`'s header: the name, the model, the effort open the member's sheet in the
    /// menu, on that field; the context of a member at rest offers to compact it; ⤢ and ⤡ zoom.
    pub(super) fn part_click(&mut self, id: &str, part: chrome::Part) {
        let Some(member) = self.pane(id).filter(|pane| pane.is_member()).map(|pane| pane.member.clone()) else {
            return;
        };
        let field = match part {
            chrome::Part::Name => MenuField::Name,
            chrome::Part::Model | chrome::Part::Effort => {
                let field = if part == chrome::Part::Model { MenuField::Model } else { MenuField::Effort };
                // Under the text clicked, a list of the values; the whole menu if the text is not found.
                let anchor = self.parts.iter().find(|(_, of, p)| of == id && *p == part).map(|(zone, _, _)| *zone);
                if let Some(anchor) = anchor {
                    let list = List { pane: id.to_string(), part, anchor, size: LIST_SIZE };
                    if let Err(error) = self.open_menu_as(None, Some((&member, Some(field))), Some(list)) {
                        log!("menu: {error:#}");
                    }
                    return;
                }
                field
            }
            chrome::Part::Context => {
                // As the header shows it: no file read here.
                let percent = self.looks.get(&member).and_then(|looks| looks.context);
                self.open_choice(Open::new(chrome::compact_choice(&member, percent), Purpose::Compact { member }));
                return;
            }
            chrome::Part::Zoom => {
                self.zoom(id);
                return;
            }
        };
        if let Err(error) = self.open_menu(None, Some((&member, Some(field)))) {
            log!("menu: {error:#}");
        }
    }

    /// A press on the dashboard or the journal, at `x`, `y` of its cells (`columns` wide): the member clicked gets the
    /// focus, in its tab; a member's context offers to compact it, after confirmation. The team's state is read here,
    /// once per click, not on the way of a frame.
    fn panel_click(&mut self, id: &str, x: usize, y: usize, columns: usize) {
        let Some(pane) = self.pane(id) else { return };
        let kind = if pane.role == Role::Dashboard { board::Kind::Dashboard } else { board::Kind::Journal };
        // The row clicked as the panel wrote it: a recipient's name is read in it.
        let line = pane
            .with(|engine| engine.line(engine.top() + y as u64).map(|line| line.text.trim_end().to_string()))
            .flatten()
            .unwrap_or_default();
        let snapshot = match Snapshot::read(&self.state) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                log!("click: {error:#}");
                return;
            }
        };
        match board::clicked(&snapshot, &self.state, kind, x, y, columns, &line) {
            Some(board::Clicked::Show(member)) => {
                if let Some(target) = self.member_pane(&member).map(|pane| pane.id.clone()) {
                    self.focus_on(&target);
                }
            }
            Some(board::Clicked::Compact(member)) => {
                self.open_choice(Open::new(
                    chrome::compact_choice(&member, board::context_percent(&self.state, &member)),
                    Purpose::Compact { member },
                ));
            }
            None => {}
        }
        self.dirty = true;
    }

    /// Where pane `id`'s cells are in its `area`: inside its frame, but for the menu's list, which draws its own.
    pub(super) fn cells(&self, id: &str, area: Rect) -> Rect {
        if self.list.is_some() && self.menu.as_ref().is_some_and(|(menu, _)| menu == id) {
            area
        } else {
            screen::content(area)
        }
    }

    /// A mouse event: the bar's parts, else the pane under the pointer, or the one a button went down in until it
    /// goes up; a press in a pane gives it the focus. To its program if it takes the mouse, else the wheel scrolls
    /// its history.
    pub(super) fn mouse(&mut self, mouse: &Mouse) {
        if choice::swallowed(&mut self.pointer.swallowing, mouse.kind) {
            return;
        }
        let (col, row) = (mouse.col as usize, mouse.row as usize);
        // Nothing moves in a tab under the pointer, nor right after a click (« focus »).
        self.pointer.at = Some((col, row));
        self.pointer.moved = Some(Instant::now());
        if let MouseKind::Down(_) = mouse.kind {
            self.pointer.last_click = Some(Instant::now());
        }
        if mouse.kind == MouseKind::Moved {
            self.hover_at(col, row);
        }
        // A list under a header's text: a press outside it closes it (on the same text too: a second click), and is
        // all it does; inside, it is the list's, whatever header part lies under it.
        let in_list = self.list.is_some() && self.menu_area().contains(col, row);
        if self.mouse_on_layer(mouse, in_list) || self.mouse_on_chrome(mouse, in_list) {
            return;
        }
        self.mouse_in_pane(mouse);
    }

    /// A press on a layer over the team: outside the list under a header's text, which it closes, or on the notice,
    /// which brings its member in. Whether it was one (and is all it did).
    fn mouse_on_layer(&mut self, mouse: &Mouse, in_list: bool) -> bool {
        let (col, row) = (mouse.col as usize, mouse.row as usize);
        if let MouseKind::Down(_) = mouse.kind
            && self.list.is_some()
            && !in_list
        {
            self.close_list();
            self.pointer.swallowing = true;
            return true;
        }
        // The notice over everything: a click on it goes to its member.
        if let MouseKind::Down(_) = mouse.kind
            && self.notices.notice_zone.is_some_and(|zone| zone.contains(col, row))
        {
            self.unselect();
            if let Some((notice, _)) = self.notices.notice.take()
                && let Some(target) = self.member_pane(&notice.member).map(|pane| pane.id.clone())
            {
                self.focus_on(&target);
            }
            // Its release is over the pane under it: not for its program.
            self.pointer.swallowing = true;
            self.dirty = true;
            return true;
        }
        false
    }

    /// A press on what the team draws: a part of a header, the bar, a card, the header of a pane on the right of a tab
    /// in « focus ». Whether it was one (and is all it did).
    fn mouse_on_chrome(&mut self, mouse: &Mouse, in_list: bool) -> bool {
        let (col, row) = (mouse.col as usize, mouse.row as usize);
        if let MouseKind::Down(Button::Left) = mouse.kind
            && !in_list
            && let Some((id, part)) =
                self.parts.iter().find(|(zone, _, _)| zone.contains(col, row)).map(|(_, id, part)| (id.clone(), *part))
        {
            self.unselect();
            self.part_click(&id, part);
            self.pointer.swallowing = true;
            self.dirty = true;
            return true;
        }
        if let MouseKind::Down(_) = mouse.kind
            && let Some(target) = self.zones.iter().find(|(zone, _)| zone.contains(col, row)).map(|(_, t)| *t)
        {
            self.unselect();
            match target {
                Target::Tab(tab) => self.show_tab(tab),
                Target::Layout if self.menu.is_some() => {}
                Target::Layout => self.toggle_layout(),
                Target::Menu => self.menu_here(),
                Target::Quit if self.menu.is_some() => {}
                Target::Quit => self.ask_quit(),
            }
            self.dirty = true;
            return true;
        }
        // The cards of the members left out of a tab in « focus »: a card brings its member to the left.
        if let MouseKind::Down(Button::Left) = mouse.kind
            && !in_list
            && self.card_click(col, row)
        {
            self.unselect();
            self.pointer.swallowing = true;
            self.dirty = true;
            return true;
        }
        // The header of a pane on the right of a tab in « focus »: a press gives it the focus, a second one brings it
        // to the left.
        if let MouseKind::Down(Button::Left) = mouse.kind
            && !in_list
            && let Some(id) = self.right_header_at(col, row)
        {
            self.unselect();
            let now = Instant::now();
            let again =
                self.pointer.header_click.take().is_some_and(|(at, prev)| prev == id && at.elapsed() < DOUBLE_CLICK);
            if again {
                if let Some(focus) = self.active_focus_mut() {
                    focus.bring(&id, now);
                }
            } else {
                self.pointer.header_click = Some((now, id.clone()));
            }
            self.focus_on(&id);
            self.layout();
            self.pointer.swallowing = true;
            self.dirty = true;
            return true;
        }
        false
    }

    /// The mouse in the pane under the pointer, or the one a button went down in until it goes up; a press gives it the
    /// focus.
    fn mouse_in_pane(&mut self, mouse: &Mouse) {
        let (col, row) = (mouse.col as usize, mouse.row as usize);
        let mut shown = self.shown_ids();
        // The menu over the others: first under the pointer, and the only one while it is open.
        if let Some((menu, _)) = &self.menu {
            shown = vec![menu.clone()];
        }
        let under = self
            .panes
            .iter()
            .filter(|pane| shown.contains(&pane.id))
            .find(|pane| self.cells(&pane.id, pane.area).contains(col, row))
            .map(|pane| pane.id.clone());
        let held = matches!(mouse.kind, MouseKind::Drag(_) | MouseKind::Up(_));
        let Some(id) = (if held { self.pointer.grab.clone().or(under) } else { under }) else { return };
        if matches!(mouse.kind, MouseKind::Up(_)) {
            self.pointer.grab = None;
        }
        if let MouseKind::Down(_) = mouse.kind {
            self.pointer.grab = Some(id.clone());
            if let MouseKind::Down(button) = mouse.kind {
                self.pointer.grab_button = button;
            }
            if self.focus.as_deref() != Some(id.as_str()) {
                self.focus_on(&id);
                self.dirty = true;
            }
        }
        let Some(area) = self.pane(&id).map(|pane| pane.area) else { return };
        let content = self.cells(&id, area);
        let x = col.clamp(content.x, content.x + content.width.saturating_sub(1)) - content.x;
        let y = row.clamp(content.y, content.y + content.height.saturating_sub(1)) - content.y;
        if matches!(mouse.kind, MouseKind::Down(_) | MouseKind::Drag(_))
            && self.pointer.grab.as_deref() == Some(id.as_str())
        {
            self.pointer.grab_at = (x as u16, y as u16);
        }
        let Some(pane) = self.pane(&id) else { return };
        // The dashboard and the journal ask for no mouse (`recruit _panel`): a press goes to the member it lands on.
        if let MouseKind::Down(Button::Left) = mouse.kind
            && matches!(pane.role, Role::Dashboard | Role::Journal)
        {
            self.unselect();
            self.panel_click(&id, x, y, content.width);
            return;
        }
        let left = matches!(
            mouse.kind,
            MouseKind::Down(Button::Left) | MouseKind::Drag(Button::Left) | MouseKind::Up(Button::Left)
        );
        // With Shift, the left button selects even in a program that takes the mouse, as in terminals.
        let program = if left && (self.pointer.selecting || mouse.mods.contains(Mods::SHIFT)) {
            None
        } else {
            input::mouse(mouse, x as u16, y as u16, &pane.modes())
        };
        match program {
            Some(bytes) => {
                pane.send(bytes);
                if let MouseKind::Down(_) = mouse.kind {
                    self.unselect();
                }
            }
            None if left => self.select(&id, mouse, content),
            None => {
                let lines = match mouse.kind {
                    MouseKind::ScrollUp => WHEEL,
                    MouseKind::ScrollDown => -WHEEL,
                    _ => return,
                };
                self.dirty |= pane
                    .with(|engine| {
                        let before = engine.scrolled();
                        engine.scroll(lines);
                        engine.scrolled() != before
                    })
                    .unwrap_or(false);
            }
        }
    }

    /// The left button selecting in pane `id`, whose cells are `cells`: a press starts a selection (a word on a
    /// double click, a line on a triple), a drag extends it (the history scrolling past the pane's top or bottom), a
    /// release copies it, to the client's terminal (OSC 52).
    pub(super) fn select(&mut self, id: &str, mouse: &Mouse, cells: Rect) {
        let (col, row) = (mouse.col as usize, mouse.row as usize);
        let previous = self.pointer.selection.take();
        let Some(pane) = self.pane(id) else { return };
        let alt = pane.modes().alt_screen;
        let now = Instant::now();
        let done = pane.with(|engine| match mouse.kind {
            MouseKind::Down(_) => {
                let at = select::point(engine, cells, col, row);
                let previous = previous.as_ref().map(|(selection, _)| selection);
                (Some((Selection::press(previous, id, at, now, engine), alt)), None)
            }
            MouseKind::Drag(_) => match previous.filter(|(selection, _)| selection.pane == id) {
                Some((mut selection, alt)) => {
                    let lines = select::edge(cells, row);
                    if lines != 0 {
                        engine.scroll(lines);
                    }
                    selection.drag(select::point(engine, cells, col, row), engine);
                    (Some((selection, alt)), None)
                }
                None => (None, None),
            },
            MouseKind::Up(_) => {
                let copied = previous.as_ref().and_then(|(selection, _)| selection.copy(engine));
                (previous, copied)
            }
            _ => (previous, None),
        });
        if let Some((selection, copied)) = done {
            self.pointer.selection = selection;
            if let (Some(relay), Some(client)) = (copied, self.client.as_mut()) {
                client.relays.add(relay);
            }
        }
        self.pointer.selecting = matches!(mouse.kind, MouseKind::Down(_) | MouseKind::Drag(_));
        self.dirty = true;
    }

    pub(super) fn unselect(&mut self) {
        if self.pointer.selection.take().is_some() {
            self.dirty = true;
        }
        self.pointer.selecting = false;
    }

    /// The selection gone if it is in pane `id`: what it showed is about to change.
    pub(super) fn unselect_in(&mut self, id: Option<String>) {
        if self.pointer.selection.as_ref().is_some_and(|(selection, _)| Some(&selection.pane) == id.as_ref()) {
            self.unselect();
        }
    }

    /// The selection gone if its lines left the history, or its pane changed screens.
    pub(super) fn check_selection(&mut self) {
        let Some((selection, alt)) = &self.pointer.selection else { return };
        let gone = match self.pane(&selection.pane) {
            Some(pane) => {
                pane.with(|engine| engine.modes().alt_screen != *alt || !selection.kept(engine)).unwrap_or(true)
            }
            None => true,
        };
        if gone {
            self.unselect();
        }
    }
}

/// What the client did last: its pointer, its buttons, its keys, and the selection.
pub(super) struct Pointer {
    /// The pane a button went down in: its drag and its release go there.
    pub(super) grab: Option<String>,
    /// The button that went down in it.
    pub(super) grab_button: Button,
    /// Where the pointer last was in it, its cells from 0: at its press, then at each drag.
    pub(super) grab_at: (u16, u16),
    /// A click settled a choice: the events of its button, until its release, are swallowed.
    pub(super) swallowing: bool,
    /// The screen's selection, in one pane; whether its pane was on its alternate screen when it began.
    pub(super) selection: Option<(Selection, bool)>,
    /// The left button went down to select: its drag and its release select too.
    pub(super) selecting: bool,
    /// What the pointer is over, among what a click reaches.
    pub(super) hover: Option<Hover>,
    /// Where the pointer last was, on the screen, and when it last moved: nothing moves in a tab under it
    /// ([`POINTER_HOLD`]).
    pub(super) at: Option<(usize, usize)>,
    pub(super) moved: Option<Instant>,
    /// The last key typed, the last click: nothing moves for a second after them.
    pub(super) last_key: Option<Instant>,
    pub(super) last_click: Option<Instant>,
    /// The last press on the header of a pane on the right of a tab in « focus »: a second one brings it to the left.
    pub(super) header_click: Option<(Instant, String)>,
}

impl Pointer {
    pub(super) fn new() -> Pointer {
        Pointer {
            grab: None,
            grab_button: Button::Left,
            grab_at: (0, 0),
            swallowing: false,
            selection: None,
            selecting: false,
            hover: None,
            at: None,
            moved: None,
            last_key: None,
            last_click: None,
            header_click: None,
        }
    }
}
