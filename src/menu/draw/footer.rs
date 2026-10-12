// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The foot of the menu: the line under the sheet, the result of the last change, the team's buttons and their keys.

use crossterm::style::Color;

use super::super::sheet::{Action, Fid, Overlay, Said, Target, Team};
use crate::config::Layout;
use crate::t;
use crate::ui::canvas::{Style, columns};

use super::*;

impl Drawer<'_> {
    /// The line under the sheet, cut between words over two rows at most, `room` columns wide: where the value comes
    /// from in a narrow window, what « default » gives, when a change takes effect.
    pub(super) fn hint_rows(&self, room: usize) -> Vec<String> {
        let sheet = self.sheet;
        let mut text = sheet.hint().unwrap_or_default();
        if !self.wide
            && let Some(fid) = sheet.field()
            && let Some(origin) = sheet.origin(fid)
        {
            text = if text.is_empty() { origin } else { format!("{origin} · {text}") };
        }
        if text.is_empty() {
            return Vec::new();
        }
        wrap(&text, room.max(1), 2)
    }

    /// The hint's `rows` from row `y`, after « ↳ », the second under the first's text.
    pub(super) fn hint(&mut self, rows: &[String], x: usize, y: usize, end: usize) {
        for (i, row) in rows.iter().enumerate() {
            let at = if i == 0 { self.c.put(x, y, "↳ ", muted()) } else { x + 2 };
            self.c.put_in(at, y + i, end, row, muted());
        }
    }

    /// What the last change said, on rows of the screen's width: three at most, the last one cut.
    /// A result signed and colored, a question as it is.
    pub(super) fn said(&self, width: usize) -> Vec<(String, Style)> {
        let (sign, text, style) = match &self.sheet.said {
            Some(Said::Done(text)) => ("✓ ", text, Style::fg(Color::Green)),
            Some(Said::Failed(text)) => ("✗ ", text, Style::fg(Color::Red)),
            Some(Said::Asked(text)) => ("", text, Style::PLAIN),
            None => return Vec::new(),
        };
        let indent = " ".repeat(columns(sign));
        let rows = wrap(text, width.saturating_sub(2 + columns(sign)), 3);
        rows.into_iter()
            .enumerate()
            .map(|(i, row)| (format!("{}{row}", if i == 0 { sign } else { indent.as_str() }), style))
            .collect()
    }

    pub(super) fn message(&mut self, y: usize, said: &[(String, Style)]) {
        for (i, (row, style)) in said.iter().enumerate() {
            self.c.put(1, y + i, row, *style);
        }
    }

    pub(super) fn actions(&self) -> Vec<Action> {
        self.sheet.actions()
    }

    /// The team's actions, as buttons with their key.
    pub(super) fn buttons(&mut self, y: usize, width: usize, actions: &[Action]) {
        let mut x = 1;
        // Short of room for the whole row, the grid and the share keep their value only: « g 3 × 2 », « c ⟳ 70 % »;
        // still short, the others keep a word.
        let label = |stage: usize, action: Action| match (stage, action) {
            (1.., Action::Grid | Action::AutoCompact) => value_label(action, &self.sheet.team),
            (2.., Action::New | Action::Dashboard | Action::Layout | Action::RestartAll) => {
                action_label(action, &self.sheet.team, false)
            }
            _ => action_label(action, &self.sheet.team, true),
        };
        let wanted = |stage: usize| actions.iter().map(|a| 2 + columns(&label(stage, *a)) + 2 + 1).sum::<usize>() + 1;
        let stage = (0..=2).find(|stage| wanted(*stage) <= width).unwrap_or(2);
        for &action in actions {
            let text = label(stage, action);
            let w = 2 + columns(&text) + 2;
            if x + w >= width {
                break;
            }
            let start = x;
            x = self.c.put(x, y, &format!(" {}", action.key()), Style::fg(Color::Cyan).bold().on(button()));
            x = self.c.put(x, y, &format!(" {text} "), Style::PLAIN.on(button()));
            self.zone(start, y, x - start, 1, Target::Action(action));
            x += 1;
        }
    }

    /// The team's actions in a narrow window: their key and a word.
    pub(super) fn action_keys(&mut self, y: usize, width: usize, without_settings: bool) {
        // Short of room, the least needed go first: detaching and stopping stay.
        let mut actions = self.actions();
        actions.retain(|a| !(without_settings && a.is_setting()));
        let room = |actions: &[Action]| -> usize {
            actions.iter().map(|a| columns(&action_label(*a, &self.sheet.team, false)) + 4).sum::<usize>() + 1
        };
        for spare in
            [Action::Layout, Action::Grid, Action::AutoCompact, Action::RestartAll, Action::Dashboard, Action::New]
        {
            if room(&actions) <= width {
                break;
            }
            actions.retain(|a| *a != spare);
        }
        let mut x = 1;
        for action in actions {
            let text = action_label(action, &self.sheet.team, false);
            let w = columns(&text) + 2;
            if x + w >= width {
                break;
            }
            let start = x;
            x = self.c.put(x, y, &action.key().to_string(), key_style());
            x = self.c.put(x, y, &format!(" {text}"), muted());
            self.zone(start, y, x - start, 1, Target::Action(action));
            x += 2;
        }
    }

    /// The team's settings on a row of their own in a narrow window: a key and the value, nothing else:
    /// `t tableau oui  l grille  g 3 × 2  c ⟳ 70 %`.
    pub(super) fn setting_keys(&mut self, y: usize, width: usize) {
        let team = &self.sheet.team;
        let mut x = 1;
        for action in self.actions().into_iter().filter(|a| a.is_setting()) {
            let text = match action {
                Action::Dashboard if team.dashboard => t!("tableau oui", "dashboard yes"),
                Action::Dashboard => t!("tableau non", "dashboard no"),
                Action::Layout => match team.layers.as_ref().map(|l| l.layout.value) {
                    Some(Layout::Focus) => "focus".to_string(),
                    _ => t!("grille", "grid"),
                },
                _ => value_label(action, team),
            };
            if x + columns(&text) + 2 >= width {
                break;
            }
            let start = x;
            x = self.c.put(x, y, &action.key().to_string(), key_style());
            x = self.c.put(x, y, &format!(" {text}"), muted());
            self.zone(start, y, x - start, 1, Target::Action(action));
            x += 2;
        }
    }

    /// The keys that act now.
    pub(super) fn keys(&mut self, y: usize, width: usize) {
        let word = if self.look.option { "⌥⌫" } else { "Alt+⌫" };
        let esc = t!("Échap", "Esc");
        let list: Vec<(String, String)> = match &self.sheet.overlay {
            Overlay::List(_) => vec![
                ("↑↓".into(), t!("choisir", "choose")),
                ("⏎".into(), t!("valider", "apply")),
                (esc, t!("ferme la liste", "closes the list")),
            ],
            Overlay::Typing(_) => vec![
                ("⏎".into(), t!("valider", "apply")),
                (esc, t!("annuler", "cancel")),
                (word.into(), t!("efface un mot", "erases a word")),
            ],
            Overlay::Confirm(_) => Vec::new(),
            Overlay::None => match self.sheet.field() {
                // Nothing to change: the files do not read.
                None if self.sheet.focusable().is_empty() => {
                    vec![("↑↓".into(), t!("membre", "member")), (esc, t!("ferme", "closes"))]
                }
                None => vec![
                    ("↑↓".into(), t!("membre", "member")),
                    ("⏎".into(), t!("sa fiche", "its sheet")),
                    (esc, t!("ferme", "closes")),
                ],
                Some(fid) if self.sheet.pending.is_some_and(|(f, _)| f == fid) => vec![
                    ("⏎".into(), t!("valider", "apply")),
                    ("←→".into(), t!("valeur", "value")),
                    (esc, t!("abandonner", "give up")),
                ],
                Some(fid) if fid.cycles() => vec![
                    ("↑↓".into(), t!("champ", "field")),
                    ("←→".into(), t!("valeur", "value")),
                    ("⏎".into(), t!("liste des valeurs", "list of values")),
                    (esc, t!("retour à la liste", "back to the list")),
                ],
                Some(Fid::Restart) => vec![
                    ("↑↓".into(), t!("champ", "field")),
                    ("←→".into(), t!("choisir", "choose")),
                    ("⏎".into(), t!("relancer", "restart")),
                    (esc, t!("retour à la liste", "back to the list")),
                ],
                Some(_) => vec![
                    ("↑↓".into(), t!("champ", "field")),
                    ("⏎".into(), t!("modifier", "change")),
                    (esc, t!("retour à la liste", "back to the list")),
                ],
            },
        };
        let mut x = 1;
        for (key, text) in list {
            if x + columns(&key) + 1 + columns(&text) >= width {
                break;
            }
            x = self.c.put(x, y, &key, key_style());
            x = self.c.put(x, y, &format!(" {text}"), muted()) + 3;
        }
    }
}

/// A team setting's value alone, for a button short of room: the grid, the share (⟳ before it).
fn value_label(action: Action, team: &Team) -> String {
    let Some(layers) = &team.layers else { return String::new() };
    match action {
        Action::Grid => format!("{} × {}", layers.columns.value, layers.rows.value),
        _ => match layers.auto_compact.value {
            Some(at) => t!("⟳ {} %", "⟳ {}%", at),
            None => t!("⟳ non", "⟳ off"),
        },
    }
}

fn action_label(action: Action, team: &Team, long: bool) -> String {
    match (action, long) {
        (Action::New, true) => t!("Nouvel agent", "New agent"),
        (Action::New, false) => t!("agent", "agent"),
        (Action::Dashboard, true) if team.dashboard => t!("Tableau de bord : oui", "Dashboard: yes"),
        (Action::Dashboard, true) => t!("Tableau de bord : non", "Dashboard: no"),
        (Action::Dashboard, false) => t!("tableau", "dashboard"),
        (Action::Layout, true) => match team.layers.as_ref().map(|l| l.layout.value) {
            Some(Layout::Focus) => t!("Disposition : focus", "Layout: focus"),
            _ => t!("Disposition : grille", "Layout: grid"),
        },
        (Action::Layout, false) => t!("disposition", "layout"),
        (Action::Grid, true) => match &team.layers {
            Some(l) => t!("Grille : {} × {}", "Grid: {} × {}", l.columns.value, l.rows.value),
            None => t!("Grille", "Grid"),
        },
        (Action::Grid, false) => t!("grille", "grid"),
        (Action::AutoCompact, true) => match team.layers.as_ref().map(|l| l.auto_compact.value) {
            Some(Some(at)) => t!("Compaction auto : {} %", "Auto-compaction: {}%", at),
            Some(None) => t!("Compaction auto : non", "Auto-compaction: off"),
            None => t!("Compaction auto", "Auto-compaction"),
        },
        (Action::AutoCompact, false) => t!("compaction", "compaction"),
        (Action::RestartAll, true) => t!("Réinitialiser", "Reset"),
        (Action::RestartAll, false) => t!("réinitialiser", "reset"),
        (Action::Detach, true) => t!("Détacher", "Detach"),
        (Action::Detach, false) => t!("détacher", "detach"),
        (Action::Stop, true) => t!("Quitter", "Quit"),
        (Action::Stop, false) => t!("quitter", "quit"),
    }
}
