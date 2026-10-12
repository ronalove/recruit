// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The tabs: which panes each holds, which one has the focus, zoom, and where each pane goes on the screen.

use super::Server;
use crate::mux::chrome;
use crate::mux::focus::{Focus, Places};
use crate::mux::input;
use crate::mux::layout::{self, SidePanes};
use crate::mux::screen;
use crate::ui::Rect;

/// A tab: its title, its members' panes in reading order, and the panels' column beside them.
pub(super) struct Tab {
    pub(super) title: String,
    pub(super) panes: Vec<String>,
    pub(super) side: Option<Side>,
    /// The pane that had the focus when the tab was last shown: it gets it back, as a tmux window does.
    pub(super) last: Option<String>,
    /// The member's pane that takes the tab's room (⌥z, ⤢), the others hidden.
    pub(super) zoomed: Option<String>,
    /// ⌥l: the tab in « focus » (true) or in its grid (false) whatever the team says, until the client leaves.
    pub(super) mode: Option<bool>,
    /// The places of a tab in « focus », kept from one look to the next; none for a tab in its grid.
    pub(super) focus: Option<Focus>,
}

/// The panels' column: the dashboard over the journal, by pane ids.
#[derive(Clone, Default)]
pub(super) struct Side {
    pub(super) dashboard: Option<String>,
    pub(super) journal: Option<String>,
}

impl Tab {
    pub(super) fn new(title: String, panes: Vec<String>) -> Tab {
        Tab { title, panes, side: None, last: None, zoomed: None, mode: None, focus: None }
    }

    /// Every pane of the tab: the members', then the panels'.
    pub(super) fn ids(&self) -> Vec<String> {
        let side = self.side.iter().flat_map(|side| [side.dashboard.clone(), side.journal.clone()]).flatten();
        self.panes.iter().cloned().chain(side).collect()
    }

    pub(super) fn has(&self, id: &str) -> bool {
        self.ids().iter().any(|pane| pane == id)
    }

    /// Takes pane `id` out of the tab.
    pub(super) fn forget(&mut self, id: &str) {
        self.panes.retain(|pane| pane != id);
        if self.zoomed.as_deref() == Some(id) {
            self.zoomed = None;
        }
        if let Some(side) = self.side.as_mut() {
            if side.dashboard.as_deref() == Some(id) {
                side.dashboard = None;
            }
            if side.journal.as_deref() == Some(id) {
                side.journal = None;
            }
            if side.dashboard.is_none() && side.journal.is_none() {
                self.side = None;
            }
        }
    }
}

/// The reduced journal's rows, its frame included: its last three messages, two lines each, then the line the next
/// one starts on.
const JOURNAL_ROWS: usize = 3 * 2 + 1 + 2 * chrome::FRAME;

impl Server {
    /// Member pane `id` takes its tab's room, with the focus; zoomed already, the grid comes back.
    pub(super) fn zoom(&mut self, id: &str) {
        if !self.pane(id).is_some_and(|pane| pane.is_member()) {
            return;
        }
        let Some(tab) = self.tabs.iter().position(|tab| tab.panes.iter().any(|pane| pane == id)) else { return };
        if self.tabs[tab].zoomed.as_deref() == Some(id) {
            self.tabs[tab].zoomed = None;
        } else {
            self.focus_on(id);
            self.tabs[tab].zoomed = Some(id.to_string());
        }
        self.unselect();
        self.layout();
    }

    /// Gives the focus to pane `id`, showing its tab; the panes that asked are told (1004).
    pub(super) fn focus_on(&mut self, id: &str) {
        if let Some(tab) = self.tabs.iter().position(|tab| tab.has(id)) {
            self.active = tab;
            self.tabs[tab].last = Some(id.to_string());
            // Another pane of a zoomed tab: the grid back, as in tmux.
            if self.tabs[tab].zoomed.as_deref().is_some_and(|zoomed| zoomed != id) {
                self.tabs[tab].zoomed = None;
            }
        }
        if self.focus.as_deref() == Some(id) {
            return;
        }
        if let Some(old) = self.focused() {
            old.send(input::focus(false, &old.modes()).unwrap_or_default());
        }
        self.focus = Some(id.to_string());
        if let Some(new) = self.focused() {
            new.send(input::focus(true, &new.modes()).unwrap_or_default());
        }
        self.layout();
    }

    pub(super) fn show_tab(&mut self, tab: usize) {
        let Some(first) = self.tabs.get(tab).and_then(|tab| tab.ids().first().cloned()) else { return };
        // The pane that had the focus in it, else the first: one gone since is forgotten.
        let focus = self.tabs[tab].last.clone().filter(|id| self.tabs[tab].has(id));
        self.focus_on(&focus.unwrap_or(first));
        self.active = tab;
        self.layout();
    }

    pub(super) fn resize(&mut self, size: (u16, u16)) {
        self.unselect();
        // A list under a header's text would stay where the text was: closed, as by a click outside.
        self.close_list();
        self.size = size;
        if let Some(client) = self.client.as_mut() {
            client.size = size;
        }
        self.layout();
    }

    /// The places of the tab shown, when it is in « focus ».
    pub(super) fn active_focus_mut(&mut self) -> Option<&mut Focus> {
        self.tabs.get_mut(self.active).and_then(|tab| tab.focus.as_mut())
    }

    /// The room of a tab: the screen less the bar.
    pub(super) fn tab_room(&self) -> (usize, usize) {
        (self.size.0 as usize, (self.size.1 as usize).saturating_sub(1))
    }

    /// Every tab laid out on the screen, less the bar: a shown tab's panes get their place, the others their size.
    pub(super) fn layout(&mut self) {
        self.ensure_focus();
        if let Some((menu, _)) = self.menu.clone() {
            let area = self.menu_area();
            // A list draws its own frame: its cells are all its place.
            let inside = if self.list.is_some() { area } else { screen::content(area) };
            if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == menu) {
                pane.place(area, inside);
            }
        }
        let (width, height) = self.tab_room();
        for (t, tab) in self.tabs.iter().enumerate() {
            let side = tab.side.as_ref().map(|side| SidePanes {
                dashboard: side.dashboard.clone(),
                journal: side.journal.clone().map(|id| (id, self.reduced.then_some(JOURNAL_ROWS))),
            });
            // Zoomed, one pane takes the tab's room, the others keep their size, hidden (their programs are not
            // resized for nothing); in a tab not shown too, so that going back to it resizes nothing.
            let zoomed = tab.zoomed.as_deref();
            // In « focus »: the left pane, those on the right, and the room of the cards (left empty for now).
            let places = tab.focus.as_ref().map(Focus::places).filter(|places| places.left.is_some());
            let rects = match places {
                Some(Places { left: Some(left), right, others }) => {
                    layout::focus_rects(width, height, left, right, others.len()).0
                }
                _ => layout::rects(width, height, &tab.panes, self.columns, side.as_ref()),
            };
            // A card is no pane on the screen: hidden, its size kept.
            for id in tab.panes.iter().filter(|id| !rects.iter().any(|(placed, _)| placed == *id)) {
                if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == *id) {
                    pane.area = Rect::default();
                }
            }
            for (id, area) in rects {
                let area = if zoomed == Some(id.as_str()) { Rect { x: 0, y: 0, width, height } } else { area };
                if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) {
                    let shown = t == self.active && zoomed.is_none_or(|zoomed| zoomed == id);
                    pane.place(area, screen::content(area));
                    pane.area = if shown { area } else { Rect::default() };
                }
            }
        }
        self.refresh_cards();
        self.dirty = true;
    }

    /// The panes in a client's view, for the notices: those the tab shown shows, and its cards (a card turns red, then
    /// its member comes in: no notice). Not for the compaction, to which a card is no pane on the screen.
    pub(super) fn in_view_ids(&self) -> Vec<String> {
        let mut ids = self.shown_ids();
        ids.extend(self.cards.ids.iter().cloned());
        ids
    }

    /// The panes the active tab shows: the zoomed one alone, else all of them.
    pub(super) fn shown_ids(&self) -> Vec<String> {
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        match (&tab.zoomed, tab.focus.as_ref()) {
            (Some(id), _) => vec![id.clone()],
            // In « focus »: the left pane and those on the right; the others are cards.
            (None, Some(focus)) => {
                let places = focus.places();
                places.left.iter().chain(&places.right).cloned().collect()
            }
            (None, None) => tab.ids(),
        }
    }
}
