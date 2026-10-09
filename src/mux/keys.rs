// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The multiplexer's own keys (spec §5.4): no prefix, ⌥ only (Alt elsewhere than macOS), so that every Ctrl key
//! reaches Claude Code. A table without state: a key in, what to do out; the server's loop does it.
//!
//! Owner: dev-saisie.

use super::input::{Key, KeyCode, KeyKind, Mods};

/// What one of the multiplexer's keys asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// ⌥1 to ⌥9: that tab, from 0.
    Tab(usize),
    /// ⌥⇧←, ⌥⇧→.
    PreviousTab,
    NextTab,
    /// ⌥n: the next pane of the tab (⌥o is Claude Code's).
    NextPane,
    /// ⌥j: the journal to its next size, full, reduced, hidden.
    Journal,
    /// ⌥r: the menu.
    Menu,
    /// ⌥q: detach, quit or cancel (the menu itself at step 1).
    Quit,
}

/// A key the multiplexer keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shortcut {
    Run(Action),
    /// The release of one of its keys, or the repeat of one that toggles or opens: nothing, and nothing for the pane
    /// either.
    Swallow,
}

/// What `key` does, if it is one of the multiplexer's; `None`: it goes to the focused pane. With ⌥ and nothing but
/// ⌥ (⇧ too for the arrows): ⌥ with Ctrl, or a capital letter, is the pane's.
pub(crate) fn shortcut(key: &Key) -> Option<Shortcut> {
    let action = match (key.mods, key.code) {
        (Mods::ALT, KeyCode::Char(digit @ '1'..='9')) => Action::Tab(digit as usize - '1' as usize),
        (Mods::ALT, KeyCode::Char('n')) => Action::NextPane,
        (Mods::ALT, KeyCode::Char('j')) => Action::Journal,
        (Mods::ALT, KeyCode::Char('r')) => Action::Menu,
        (Mods::ALT, KeyCode::Char('q')) => Action::Quit,
        (mods, KeyCode::Left) if mods == Mods::ALT | Mods::SHIFT => Action::PreviousTab,
        (mods, KeyCode::Right) if mods == Mods::ALT | Mods::SHIFT => Action::NextTab,
        _ => return None,
    };
    // Held down, the moves go on as a terminal repeats them; a toggle or a menu would flicker.
    let repeats = matches!(action, Action::PreviousTab | Action::NextTab | Action::NextPane);
    Some(match key.kind {
        KeyKind::Press => Shortcut::Run(action),
        KeyKind::Repeat if repeats => Shortcut::Run(action),
        KeyKind::Repeat | KeyKind::Release => Shortcut::Swallow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Mods = Mods::ALT;
    const S: Mods = Mods::SHIFT;
    const C: Mods = Mods::CTRL;
    const N: Mods = Mods::NONE;

    fn key(code: KeyCode, mods: Mods) -> Key {
        Key::new(code, mods)
    }

    #[test]
    fn the_table() {
        use Action::*;
        use KeyCode::{Char, Left, Right};
        let table = [
            (key(Char('1'), A), Some(Tab(0))),
            (key(Char('5'), A), Some(Tab(4))),
            (key(Char('9'), A), Some(Tab(8))),
            (key(Left, A | S), Some(PreviousTab)),
            (key(Right, A | S), Some(NextTab)),
            (key(Char('n'), A), Some(NextPane)),
            (key(Char('j'), A), Some(Journal)),
            (key(Char('r'), A), Some(Menu)),
            (key(Char('q'), A), Some(Quit)),
            // The pane's.
            (key(Char('0'), A), None),
            (key(Char('z'), A), None),
            (key(Char('o'), A), None),
            (key(Char('j'), N), None),
            (key(Char('J'), A), None),
            (key(Char('j'), A | S), None),
            (key(Char('j'), A | C), None),
            (key(Char('q'), C), None),
            (key(Left, A), None),
            (key(Right, S), None),
            (key(Left, A | S | C), None),
            (key(Char('1'), A | Mods::SUPER), None),
        ];
        for (key, action) in table {
            assert_eq!(shortcut(&key), action.map(Shortcut::Run), "{key:?}");
        }
    }

    #[test]
    fn claude_code_keeps_its_keys() {
        // Spec §6: Ctrl+B, Ctrl+R, Ctrl+O, Ctrl+C, Ctrl+D, Tab, Shift+Tab, Esc, Shift+Enter, Alt+Enter, arrows.
        let keys = [
            key(KeyCode::Char('b'), C),
            key(KeyCode::Char('r'), C),
            key(KeyCode::Char('o'), C),
            key(KeyCode::Char('c'), C),
            key(KeyCode::Char('d'), C),
            key(KeyCode::Tab, N),
            key(KeyCode::Tab, S),
            key(KeyCode::Esc, N),
            key(KeyCode::Enter, S),
            key(KeyCode::Enter, A),
            key(KeyCode::Up, N),
            key(KeyCode::Left, A),
            key(KeyCode::Char('b'), A),
            key(KeyCode::Char('f'), A),
        ];
        for key in keys {
            assert_eq!(shortcut(&key), None, "{key:?}");
        }
    }

    #[test]
    fn releases_are_swallowed_and_moves_repeat() {
        use Action::*;
        let table = [
            (key(KeyCode::Left, A | S), PreviousTab, true),
            (key(KeyCode::Right, A | S), NextTab, true),
            (key(KeyCode::Char('n'), A), NextPane, true),
            (key(KeyCode::Char('j'), A), Journal, false),
            (key(KeyCode::Char('r'), A), Menu, false),
            (key(KeyCode::Char('q'), A), Quit, false),
            (key(KeyCode::Char('1'), A), Tab(0), false),
            (key(KeyCode::Char('9'), A), Tab(8), false),
        ];
        for (pressed, action, repeats) in table {
            let held = Key { kind: KeyKind::Repeat, ..pressed };
            let expected = if repeats { Shortcut::Run(action) } else { Shortcut::Swallow };
            assert_eq!(shortcut(&held), Some(expected), "{held:?}");
            let released = Key { kind: KeyKind::Release, ..pressed };
            assert_eq!(shortcut(&released), Some(Shortcut::Swallow), "{released:?}");
        }
        // The pane's keys stay the pane's, released or not.
        for kind in [KeyKind::Release, KeyKind::Repeat] {
            let other = Key { kind, ..key(KeyCode::Char('j'), N) };
            assert_eq!(shortcut(&other), None);
        }
    }

    #[test]
    fn as_the_terminal_sends_them() {
        // ⌥ read from the bytes a terminal sends: ESC first (legacy), xterm's modifiers, the kitty protocol.
        let read = |bytes: &[u8]| match crate::mux::input::read_bytes(bytes).as_slice() {
            [crate::mux::input::Event::Key(key)] => *key,
            other => panic!("{bytes:?}: {other:?}"),
        };
        let table: [(&[u8], Option<Action>); 8] = [
            (b"\x1b1", Some(Action::Tab(0))),
            (b"\x1b[49;3u", Some(Action::Tab(0))),
            (b"\x1bj", Some(Action::Journal)),
            (b"\x1b[106;3u", Some(Action::Journal)),
            (b"\x1b[1;4D", Some(Action::PreviousTab)),
            (b"\x1b[1;4C", Some(Action::NextTab)),
            (b"\x1bJ", None),
            (b"\x1b[1;3C", None),
        ];
        for (bytes, action) in table {
            assert_eq!(shortcut(&read(bytes)), action.map(Shortcut::Run), "{bytes:?}");
        }
    }
}
