// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the client types: the shortcuts, the keys and the paste for the focused pane, the choices over the team.

use std::sync::atomic::Ordering;
use std::time::Instant;

use super::choice::{Open, Outcome, Purpose};
use super::lifecycle::log;
use super::pane::Pane;
use super::status::{pane_since, pane_state};
use super::{Server, choice};
use crate::mux::chrome;
use crate::mux::input::{self, Event, MouseKind};
use crate::mux::keys::{self, Action, Shortcut};
use crate::mux::proto::Bye;
use crate::ui::look;

/// The member's pane after `focus` among a tab's `members`, in turn; the first when the focus is elsewhere (a panel).
/// None when it would stay where it is: a tab of one member.
pub(super) fn next_member(members: &[String], focus: Option<&str>) -> Option<String> {
    let next = match members.iter().position(|id| Some(id.as_str()) == focus) {
        Some(at) => members.get((at + 1) % members.len()),
        None => members.first(),
    }?;
    (Some(next.as_str()) != focus).then(|| next.clone())
}

impl Server {
    /// An event from the client: the server's keys first, then the focused pane's.
    pub(super) fn input(&mut self, event: Event) {
        // A choice open takes the keys and the clicks; nothing reaches the panes meanwhile.
        if self.choice.is_some() && self.choice_takes(&event) {
            return;
        }
        match event {
            Event::Key(key) => {
                self.pointer.last_key = Some(Instant::now());
                match keys::shortcut(&key) {
                    Some(Shortcut::Run(action)) => {
                        self.act(action);
                        self.dirty = true;
                        return;
                    }
                    Some(Shortcut::Swallow) => return,
                    None => {}
                }
                let Some(pane) = self.focused() else { return };
                let modes = pane.modes();
                // Shift with Page Up, Page Down, Home, End scroll the history rather than go to the program.
                let rows = self.cells(&pane.id, pane.area).height.max(1);
                if let Some(lines) = keys::scroll_key(&key, rows, modes.alt_screen) {
                    pane.with(|engine| engine.scroll(lines));
                    self.dirty = true;
                    return;
                }
                let bytes = input::key(&key, &modes);
                let typed = !bytes.is_empty();
                if typed {
                    self.unselect_in(self.focus.clone());
                }
                let Some(pane) = self.focused() else { return };
                // A pane that writes on and on has said so already: its next output, the echo most likely, must tell.
                let unscrolled = typed && pane.unscroll();
                if typed {
                    pane.told.store(false, Ordering::Release);
                }
                pane.send(bytes);
                self.dirty |= unscrolled;
                if typed {
                    self.pacing.typed = Some(Instant::now());
                }
            }
            Event::Paste(text) => {
                self.unselect_in(self.focus.clone());
                let Some(pane) = self.focused() else { return };
                let unscrolled = pane.unscroll();
                pane.told.store(false, Ordering::Release);
                pane.send(input::paste(&text, &pane.modes()));
                self.dirty |= unscrolled;
                self.pacing.typed = Some(Instant::now());
            }
            Event::Mouse(mouse) => self.mouse(&mouse),
            Event::Focus(gained) => {
                if let Some(pane) = self.focused() {
                    pane.send(input::focus(gained, &pane.modes()).unwrap_or_default());
                }
            }
            Event::Resize(cols, rows) => self.resize((cols, rows)),
            Event::Closed(_) => {
                let _ = self.drop_client(None);
            }
        }
    }

    /// An event for the choice open over the team, if it takes it: the keys, the mouse and the paste do, the rest goes
    /// on to the panes.
    fn choice_takes(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => {
                let outcome = self.choice.as_mut().and_then(|open| open.key(key));
                self.dirty = true;
                if let Some(outcome) = outcome {
                    self.decide(outcome);
                }
                true
            }
            Event::Mouse(mouse) => {
                let (col, row) = (mouse.col as usize, mouse.row as usize);
                match mouse.kind {
                    MouseKind::Down(_) => {
                        let outcome = self.choice.as_ref().map(|open| open.click(col, row));
                        if let Some(outcome) = outcome {
                            self.decide(outcome);
                            // Its release, and what comes before it, belong to this click.
                            self.pointer.swallowing = true;
                        }
                    }
                    // Over an option, the selection follows the pointer.
                    MouseKind::Moved | MouseKind::Drag(_)
                        if self.choice.as_mut().is_some_and(|open| open.hover(col, row)) =>
                    {
                        self.dirty = true;
                    }
                    _ => {}
                }
                true
            }
            Event::Paste(_) => true,
            Event::Focus(_) | Event::Resize(..) | Event::Closed(_) => false,
        }
    }

    /// What a shortcut does (`keys.rs`). While the menu is open, only the menu's own keys do something: the rest
    /// would take the focus from it.
    pub(super) fn act(&mut self, action: Action) {
        if self.menu.is_some() && !matches!(action, Action::Menu | Action::Quit) {
            return;
        }
        match action {
            Action::Tab(tab) => self.show_tab(tab),
            Action::PreviousTab | Action::NextTab => {
                let count = self.tabs.len().max(1);
                let next = if action == Action::NextTab { 1 } else { count - 1 };
                self.show_tab((self.active + next) % count);
            }
            // The members' panes only: the dashboard and the journal are reached by a click.
            Action::NextPane => {
                let Some(tab) = self.tabs.get(self.active) else { return };
                // In « focus »: the next member of the tab comes to the left, with the focus.
                if tab.focus.is_some() && tab.zoomed.is_none() {
                    let order = tab.panes.clone();
                    let left = self.tabs[self.active].focus.as_mut().and_then(|focus| {
                        focus.next(&order, Instant::now());
                        focus.places().left.clone()
                    });
                    if let Some(left) = left {
                        self.focus_on(&left);
                    }
                    self.unselect();
                    self.layout();
                    return;
                }
                if let Some(id) = next_member(&tab.panes, self.focus.as_deref()) {
                    // A zoomed tab keeps its zoom: on the next member. ⌥z and ⤡ give
                    // the grid back.
                    if tab.zoomed.is_some() {
                        self.tabs[self.active].zoomed = Some(id.clone());
                        self.unselect();
                    }
                    self.focus_on(&id);
                }
            }
            Action::Journal => {
                if let Some(journal) = self.journal.clone()
                    && let Err(error) = self.toggle_journal(journal)
                {
                    log!("journal: {error:#}");
                }
            }
            Action::Menu => self.menu_here(),
            // The menu open has its own way out.
            Action::Quit if self.menu.is_some() => {}
            Action::Quit => self.ask_quit(),
            Action::Zoom => {
                if let Some(id) = self.focused_member().and(self.focus.clone()) {
                    self.zoom(&id);
                }
            }
            Action::Waiting => self.go_waiting(),
            Action::ToggleLayout => self.toggle_layout(),
        }
    }

    /// To the member who has waited the longest (⌥g), else to the one whose turn failed the longest ago; nothing
    /// when none waits nor failed.
    pub(super) fn go_waiting(&mut self) {
        let looks = &self.looks;
        let rank = |pane: &&Pane| match pane_state(pane, looks) {
            look::State::Waiting => Some(0),
            look::State::Error => Some(1),
            _ => None,
        };
        let calling = self.panes.iter().filter(|pane| pane.is_member()).filter(|pane| rank(pane).is_some());
        let first = calling.min_by_key(|pane| (rank(pane), pane_since(pane, looks).unwrap_or(u64::MAX)));
        if let Some(id) = first.map(|pane| pane.id.clone()) {
            self.focus_on(&id);
        }
    }

    /// The quit choice, for the client: detach it, stop the team, or cancel.
    pub(super) fn ask_quit(&mut self) {
        let Some(client) = self.client.as_ref().map(|client| client.id.clone()) else { return };
        self.open_choice(Open::new(chrome::quit_choice(), Purpose::Quit { client }));
    }

    /// Opens a choice over the team. A pane whose program took a press still down gets its release now: the choice
    /// takes the real one, and the program would stay in a drag.
    pub(super) fn open_choice(&mut self, open: Open) {
        if let Some(id) = self.pointer.grab.take()
            && let Some(pane) = self.pane(&id)
            && let Some(bytes) = choice::release(self.pointer.grab_button, self.pointer.grab_at, &pane.modes())
        {
            pane.send(bytes);
        }
        self.unselect();
        self.choice = Some(open);
        self.dirty = true;
    }

    /// What a choice's end does: its option, by its index in the choice's order.
    fn decide(&mut self, outcome: Outcome) {
        let Some(open) = self.choice.take() else { return };
        self.dirty = true;
        let Outcome::Picked(index) = outcome else { return };
        match (open.purpose, index) {
            // Detach, quit, cancel (`chrome::quit_choice`).
            (Purpose::Quit { client }, 0) => {
                if self.client.as_ref().is_some_and(|attached| attached.id == client) {
                    let _ = self.drop_client(Some(Bye::Detached));
                }
            }
            (Purpose::Quit { .. }, 1) => self.quitting = true,
            // Compact, cancel (`chrome::compact_choice`): the member's mod takes the request; one gone back to work
            // by then refuses it.
            (Purpose::Compact { member }, 0) => {
                if let Err(error) = crate::claude::bridge::request_compaction(&self.state, &member) {
                    log!("compaction of {member}: {error:#}");
                }
            }
            _ => {}
        }
    }
}
