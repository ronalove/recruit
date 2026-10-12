// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What each key does in a list of values or a confirmation open over the sheet.

use crate::config::{Edit, Field};

use super::*;

impl Sheet {
    pub(super) fn confirm_key(&mut self, mut confirm: Confirm, key: Key) -> Vec<Effect> {
        match key {
            Key::Char(c) if confirm.letters.is_some_and(|(yes, _)| yes == c) => return confirm.then,
            Key::Char(c) if confirm.letters.is_some_and(|(_, no)| no == c) => return Vec::new(),
            Key::Left | Key::Right | Key::Tab => confirm.at = 1 - confirm.at,
            Key::Enter if confirm.at == 0 => return confirm.then,
            Key::Enter | Key::Esc => return Vec::new(),
            _ => {}
        }
        self.overlay = Overlay::Confirm(confirm);
        Vec::new()
    }

    pub(super) fn list_key(&mut self, mut list: Dropdown, key: Key, env: &dyn Env) -> Vec<Effect> {
        let last = list.items.len().saturating_sub(1);
        match key {
            Key::Up => list.at = list.at.saturating_sub(1),
            Key::Down => list.at = (list.at + 1).min(last),
            Key::Home | Key::PageUp => list.at = 0,
            Key::End | Key::PageDown => list.at = last,
            Key::Esc => {
                if !matches!(list.purpose, Purpose::Value) {
                    self.said = None;
                }
                return Vec::new();
            }
            Key::Enter => return self.picked(list, env),
            _ => {}
        }
        self.overlay = Overlay::List(list);
        Vec::new()
    }

    /// The item highlighted in an open list, taken.
    pub(super) fn picked(&mut self, list: Dropdown, env: &dyn Env) -> Vec<Effect> {
        if let Some(quick) = self.quick.as_mut() {
            quick.list = list.clone();
        }
        let Some(choice) = list.items.get(list.at).cloned() else { return Vec::new() };
        match list.purpose {
            Purpose::Value => list.field.map_or_else(Vec::new, |field| self.apply_choice(field, &choice, env)),
            Purpose::Grid => self.apply_grid(&choice),
            Purpose::Compaction => self.apply_compaction(&choice),
            Purpose::Replacement { leaving, removed } => {
                self.said = None;
                let other = choice.value.unwrap_or_default();
                if removed {
                    self.confirm_remove(leaving, Some(other));
                    return Vec::new();
                }
                let edits = vec![
                    Edit::Set { member: leaving, field: Field::Contact(false) },
                    Edit::Set { member: other, field: Field::Contact(true) },
                ];
                let restarts = env.restarts(&edits);
                self.confirmed(edits, &restarts, Done::Contacts, None, Fid::Contact)
            }
        }
    }
}
