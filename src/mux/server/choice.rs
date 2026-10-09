// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A choice open over the team (the quit choice of ⌥q, the confirmation of a compaction): what each key and each
//! click does to it, without the screen. While one is open, nothing reaches the panes.
//!
//! Owner: dev-serveur.

use crate::mux::Rect;
use crate::mux::chrome::Choice;
use crate::mux::engine::Modes;
use crate::mux::input::{self, Button, Key, KeyCode, Mods, Mouse, MouseKind};

/// What the choice is for: what its options do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Purpose {
    /// ⌥q, the bar's « quitter », for this client: detach it, quit (stop the team), cancel.
    Quit { client: String },
    /// A click on a member's context: compact it, cancel.
    Compact { member: String },
}

/// How a choice ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// The option of that index, in the choice's order.
    Picked(usize),
    Cancelled,
}

/// A choice on screen.
pub(super) struct Open {
    pub choice: Choice,
    pub purpose: Purpose,
    /// Where each option can be clicked, by its index, as last drawn.
    pub zones: Vec<(Rect, usize)>,
}

impl Open {
    pub(super) fn new(choice: Choice, purpose: Purpose) -> Open {
        Open { choice, purpose, zones: Vec::new() }
    }

    /// A key: an option's own letter picks it, ←→ and ↑↓ (and Tab) move the selection around, ⏎ picks it, Échap
    /// cancels. `None`: the choice stays, perhaps with another option selected. A release or another key does
    /// nothing.
    pub(super) fn key(&mut self, key: &Key) -> Option<Outcome> {
        if !key.is_press() {
            return None;
        }
        let count = self.choice.options.len();
        if count == 0 {
            return Some(Outcome::Cancelled);
        }
        let plain = key.mods == Mods::NONE || key.mods == Mods::SHIFT;
        match key.code {
            KeyCode::Esc => Some(Outcome::Cancelled),
            KeyCode::Enter if plain => Some(Outcome::Picked(self.choice.selected.min(count - 1))),
            KeyCode::Left | KeyCode::Up => {
                self.choice.selected = (self.choice.selected + count - 1) % count;
                None
            }
            KeyCode::Right | KeyCode::Down => {
                self.choice.selected = (self.choice.selected + 1) % count;
                None
            }
            KeyCode::Tab if key.mods == Mods::SHIFT => {
                self.choice.selected = (self.choice.selected + count - 1) % count;
                None
            }
            KeyCode::Tab => {
                self.choice.selected = (self.choice.selected + 1) % count;
                None
            }
            KeyCode::Char(c) if plain => {
                let wanted = c.to_lowercase().next().unwrap_or(c);
                let at = self.choice.options.iter().position(|(key, _)| key.to_lowercase().next() == Some(wanted));
                at.map(Outcome::Picked)
            }
            _ => None,
        }
    }

    /// The pointer moved to `col`, `row`: over an option, it becomes the selected one. Whether the selection changed.
    pub(super) fn hover(&mut self, col: usize, row: usize) -> bool {
        match self.zones.iter().find(|(zone, _)| zone.contains(col, row)) {
            Some((_, index)) if *index != self.choice.selected => {
                self.choice.selected = *index;
                true
            }
            _ => false,
        }
    }

    /// A press of a button at `col`, `row`: on an option, it is picked; anywhere else, the choice is cancelled.
    pub(super) fn click(&self, col: usize, row: usize) -> Outcome {
        match self.zones.iter().find(|(zone, _)| zone.contains(col, row)) {
            Some((_, index)) => Outcome::Picked(*index),
            None => Outcome::Cancelled,
        }
    }
}

/// After a click that settled a choice, its button is still down: the events that follow, until its release, belong
/// to that click, and none may reach a pane (a program that takes the mouse would get a release without its press).
/// Whether `kind` is one of them; `swallowing` ends with the release, or with a new press (a release the terminal
/// never sent).
pub(super) fn swallowed(swallowing: &mut bool, kind: MouseKind) -> bool {
    if !*swallowing {
        return false;
    }
    match kind {
        MouseKind::Up(_) => {
            *swallowing = false;
            true
        }
        MouseKind::Drag(_) | MouseKind::Moved => true,
        MouseKind::Down(_) => {
            *swallowing = false;
            false
        }
        MouseKind::ScrollUp | MouseKind::ScrollDown | MouseKind::ScrollLeft | MouseKind::ScrollRight => false,
    }
}

/// What a pane that took a press of `button` gets when a choice opens before its release: the release, so that it
/// does not stay in a drag (the choice takes the real one), where the pointer last was in the pane (`at`, its
/// cells): a program that selects with the mouse ends its selection there. `None` when its program does not take
/// the mouse.
pub(super) fn release(button: Button, at: (u16, u16), modes: &Modes) -> Option<Vec<u8>> {
    let up = Mouse { kind: MouseKind::Up(button), col: at.0, row: at.1, mods: Mods::NONE };
    input::mouse(&up, at.0, at.1, modes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quit() -> Open {
        let choice = Choice {
            title: "mux".into(),
            note: None,
            options: vec![('d', "Détacher".into()), ('q', "Quitter".into()), ('a', "Annuler".into())],
            selected: 2,
        };
        Open::new(choice, Purpose::Quit { client: "c1".into() })
    }

    fn press(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    #[test]
    fn the_click_that_settles_a_choice_keeps_its_release() {
        let mut swallowing = true;
        assert!(swallowed(&mut swallowing, MouseKind::Drag(Button::Left)));
        assert!(swallowed(&mut swallowing, MouseKind::Up(Button::Left)), "its release");
        assert!(!swallowing);
        assert!(!swallowed(&mut swallowing, MouseKind::Up(Button::Left)), "the next one passes");
        // A release the terminal never sent: the next press passes, and ends it.
        let mut swallowing = true;
        assert!(!swallowed(&mut swallowing, MouseKind::Down(Button::Left)));
        assert!(!swallowing);
        let mut swallowing = true;
        assert!(!swallowed(&mut swallowing, MouseKind::ScrollUp), "the wheel is no button");
        assert!(swallowing);
    }

    #[test]
    fn a_pane_pressed_gets_its_release_when_a_choice_opens() {
        let sgr = Modes { mouse: Some(crate::mux::engine::MouseTracking::Drag), mouse_sgr: true, ..Modes::default() };
        // Where the pointer last was, in the pane's cells from 0 (SGR counts from 1).
        assert_eq!(release(Button::Left, (8, 3), &sgr), Some(b"\x1b[<0;9;4m".to_vec()));
        assert_eq!(release(Button::Right, (0, 0), &sgr), Some(b"\x1b[<2;1;1m".to_vec()));
        // A program that does not take the mouse gets nothing.
        assert_eq!(release(Button::Left, (8, 3), &Modes::default()), None);
    }

    #[test]
    fn keys_pick_move_and_cancel() {
        let mut open = quit();
        assert_eq!(open.key(&press(KeyCode::Enter)), Some(Outcome::Picked(2)), "opens on the cancel");
        assert_eq!(open.key(&press(KeyCode::Left)), None);
        assert_eq!(open.choice.selected, 1);
        assert_eq!(open.key(&press(KeyCode::Up)), None);
        assert_eq!(open.key(&press(KeyCode::Up)), None);
        assert_eq!(open.choice.selected, 2, "around");
        assert_eq!(open.key(&press(KeyCode::Right)), None);
        assert_eq!(open.choice.selected, 0, "around");
        assert_eq!(open.key(&Key::new(KeyCode::Tab, Mods::SHIFT)), None);
        assert_eq!(open.choice.selected, 2);
        assert_eq!(open.key(&press(KeyCode::Char('q'))), Some(Outcome::Picked(1)));
        assert_eq!(open.key(&press(KeyCode::Char('D'))), Some(Outcome::Picked(0)), "either case");
        assert_eq!(open.key(&press(KeyCode::Esc)), Some(Outcome::Cancelled));
        assert_eq!(open.key(&press(KeyCode::Char('x'))), None, "another key does nothing");
        assert_eq!(open.key(&Key::new(KeyCode::Char('q'), Mods::ALT)), None, "nor with Alt");
        let mut release = press(KeyCode::Enter);
        release.kind = crate::mux::input::KeyKind::Release;
        assert_eq!(open.key(&release), None);
    }

    #[test]
    fn a_click_picks_or_cancels() {
        let mut open = quit();
        open.zones = vec![
            (Rect { x: 10, y: 5, width: 8, height: 1 }, 0),
            (Rect { x: 20, y: 5, width: 7, height: 1 }, 1),
            (Rect { x: 29, y: 5, width: 7, height: 1 }, 2),
        ];
        assert_eq!(open.click(12, 5), Outcome::Picked(0));
        assert_eq!(open.click(21, 5), Outcome::Picked(1));
        assert_eq!(open.click(19, 5), Outcome::Cancelled, "between two options");
        assert_eq!(open.click(0, 0), Outcome::Cancelled, "outside");
        // The selection follows the pointer over the options, and stays when it leaves them.
        assert!(open.hover(21, 5));
        assert_eq!(open.choice.selected, 1);
        assert!(!open.hover(22, 5), "the same option");
        assert!(!open.hover(0, 0));
        assert_eq!(open.choice.selected, 1);
    }
}
