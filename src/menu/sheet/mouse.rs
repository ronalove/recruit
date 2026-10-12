// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What a click, a move of the pointer or the wheel does.

use super::*;

impl Sheet {
    /// A click on what `target` names.
    pub(crate) fn click(&mut self, target: Target, env: &dyn Env) -> Vec<Effect> {
        let effects = self.click_in(target, env);
        self.settled(effects)
    }

    /// The pointer over `target`: in an open list, its cursor follows; under a question, so does the button.
    pub(crate) fn hover(&mut self, target: Option<Target>) {
        match (&mut self.overlay, target) {
            (Overlay::List(list), Some(Target::Item(at))) if at < list.items.len() => list.at = at,
            (Overlay::Confirm(confirm), Some(Target::Yes)) => confirm.at = 0,
            (Overlay::Confirm(confirm), Some(Target::No)) => confirm.at = 1,
            _ => {}
        }
    }

    fn click_in(&mut self, target: Target, env: &dyn Env) -> Vec<Effect> {
        match (&self.overlay, target) {
            (Overlay::Confirm(_), Target::Yes) => {
                if let Overlay::Confirm(confirm) = std::mem::replace(&mut self.overlay, Overlay::None) {
                    return confirm.then;
                }
                Vec::new()
            }
            (Overlay::Confirm(_), Target::No) => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            // A dialog stays until answered.
            (Overlay::Confirm(_), _) => Vec::new(),
            (Overlay::List(_), Target::Item(at)) => {
                if let Overlay::List(mut list) = std::mem::replace(&mut self.overlay, Overlay::None) {
                    list.at = at.min(list.items.len().saturating_sub(1));
                    return self.picked(list, env);
                }
                Vec::new()
            }
            // A click in the text being typed keeps it.
            (Overlay::Typing(typing), Target::Row(row)) if self.focusable().get(row) == Some(&typing.field) => {
                Vec::new()
            }
            (_, target) => self.click_elsewhere(target, env),
        }
    }

    /// A click on something else than the dialog or the list open: it closes the list or gives up the text, then does
    /// what the click means.
    fn click_elsewhere(&mut self, target: Target, env: &dyn Env) -> Vec<Effect> {
        // A click elsewhere closes a list or gives up a text.
        self.overlay = Overlay::None;
        match target {
            Target::Entry(entry) => {
                self.choose(entry);
                self.pane = Pane::List;
            }
            Target::Row(row) => {
                if self.pane == Pane::Sheet
                    && self.row == row
                    && let Some(&fid) = self.focusable().get(row)
                {
                    return self.enter(fid, env);
                }
                self.pending = None;
                self.pane = Pane::Sheet;
                self.row = row;
            }
            Target::Step(row, forward) => {
                if self.row != row {
                    self.pending = None;
                }
                self.pane = Pane::Sheet;
                self.row = row;
                if let Some(&fid) = self.focusable().get(row) {
                    return self.step(fid, forward, env);
                }
            }
            Target::Restart(fresh) => {
                self.pending = None;
                self.pane = Pane::Sheet;
                self.row = self.focusable().iter().position(|f| *f == Fid::Restart).unwrap_or(self.row);
                self.fresh = fresh;
                return self.restart();
            }
            Target::Action(action) => return self.action(action),
            Target::Item(_) | Target::Yes | Target::No => {}
        }
        Vec::new()
    }

    /// The wheel over the list (`list`) or the sheet: up or down one; the value prepared is given up.
    pub(crate) fn wheel(&mut self, list: bool, down: bool, env: &dyn Env) -> Vec<Effect> {
        let key = if down { Key::Down } else { Key::Up };
        match &self.overlay {
            Overlay::List(_) => self.key(key, env),
            Overlay::None => {
                self.pending = None;
                self.pane = if list || self.focusable().is_empty() { Pane::List } else { Pane::Sheet };
                self.key(key, env)
            }
            _ => Vec::new(),
        }
    }
}
