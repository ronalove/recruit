// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Input. On the client's side, the real terminal set up after asking what it can do, and its bytes read into our
//! own events ([`Terminal`], [`Event`]); on the server's, each event encoded for the pane it goes to, as that pane's
//! modes ask ([`key`], [`mouse`], [`paste`], [`focus`]). The events are plain data, which the protocol carries.
//!
//! We read the terminal's bytes ourselves rather than through crossterm's events: crossterm reads a late answer to
//! a query as keys (`ESC ]` as Alt+]), and loses what a pane may need (0x08 read as Backspace, the kitty protocol's
//! event types).

mod encode;
mod parse;
mod probe;
mod terminal;

pub(crate) use encode::{focus, key, mouse, paste};
#[cfg(test)]
use parse::{Parser, Token};
use serde::{Deserialize, Serialize};
pub(crate) use terminal::Terminal;

/// The events `bytes` read into, as the reader would once nothing more came; for the tests of what uses them.
#[cfg(test)]
pub(crate) fn read_bytes(bytes: &[u8]) -> Vec<Event> {
    let mut parser = Parser::default();
    let mut tokens = Vec::new();
    parser.feed(bytes, &mut tokens);
    parser.flush(&mut tokens);
    tokens
        .into_iter()
        .filter_map(|token| match token {
            Token::Event(event) => Some(event),
            Token::Reply(_) => None,
        })
        .collect()
}

/// What the real terminal sends, read. With its parts, carried by the protocol as they are (serde's default form,
/// in JSON): a change of their shape is a change of the protocol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Event {
    Key(Key),
    Mouse(Mouse),
    Paste(String),
    /// The terminal's window gained (true) or lost the focus.
    Focus(bool),
    /// Its new size, columns and rows.
    Resize(u16, u16),
    /// No event will come any more, and why: the terminal is gone, or reading it failed. The last one sent.
    Closed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Key {
    /// A character key holds its text (`A` for Shift+a typed as text), or its unshifted key when it came as an
    /// escape code with other modifiers (`a` with Ctrl and Shift).
    pub code: KeyCode,
    pub mods: Mods,
    pub kind: KeyKind,
}

impl Key {
    pub(crate) fn new(code: KeyCode, mods: Mods) -> Key {
        Key { code, mods, kind: KeyKind::Press }
    }

    /// A press, or a repeat: a key that does something.
    pub(crate) fn is_press(&self) -> bool {
        self.kind != KeyKind::Release
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum KeyCode {
    Char(char),
    Enter,
    /// Shift+Tab is Tab with [`Mods::SHIFT`].
    Tab,
    Backspace,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1 to F35.
    F(u8),
}

/// Modifiers, with the kitty protocol's bits: shift 1, alt 2, ctrl 4, super 8, hyper 16, meta 32. Caps Lock and
/// Num Lock are not modifiers here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Mods(pub u8);

impl Mods {
    pub(crate) const NONE: Mods = Mods(0);
    pub(crate) const SHIFT: Mods = Mods(1);
    pub(crate) const ALT: Mods = Mods(2);
    pub(crate) const CTRL: Mods = Mods(4);
    pub(crate) const SUPER: Mods = Mods(8);

    pub(crate) fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Mods {
    type Output = Mods;

    fn bitor(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }
}

/// Only presses come while the real terminal is asked for nothing more than the kitty protocol's first flag; the
/// kinds are there for the panes that ask for them, and for a later choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum KeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Mouse {
    pub kind: MouseKind,
    /// Cell of the screen, from 0.
    pub col: u16,
    pub row: u16,
    pub mods: Mods,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MouseKind {
    Down(Button),
    Up(Button),
    /// A move with that button down.
    Drag(Button),
    /// A move without a button: the real terminal is not asked for these (1003), see [`Terminal::open`].
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Button {
    Left,
    Middle,
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_go_through_the_protocol_unchanged() {
        let back = |event: &Event| -> Event {
            let json = serde_json::to_string(event).unwrap();
            serde_json::from_str(&json).unwrap_or_else(|e| panic!("{json}: {e}"))
        };
        // Every byte a terminal sends alone, and with Alt: read, carried, and sent to a pane as it came.
        let legacy = crate::mux::engine::Modes::default();
        for b in 0u8..=0x7f {
            for bytes in [vec![b], vec![0x1b, b]] {
                let events = read_bytes(&bytes);
                if bytes == [0x1b, b'['] || bytes == [0x1b, b'O'] || bytes == [0x1b, b']'] || bytes == [0x1b, b'P'] {
                    // Alt+[, Alt+O, Alt+], Alt+P: the start of a sequence, read apart from a key (see parse.rs).
                    continue;
                }
                assert_eq!(events.len(), 1, "{bytes:?}: {events:?}");
                let carried = back(&events[0]);
                assert_eq!(carried, events[0], "{bytes:?}");
                let Event::Key(key) = carried else { panic!("{bytes:?}: {carried:?}") };
                assert_eq!(super::key(&key, &legacy), bytes, "{bytes:?}: {key:?}");
            }
        }
        // And the rest of what can come.
        let table = [
            Event::Key(Key { code: KeyCode::Char('é'), mods: Mods::CTRL | Mods::SHIFT, kind: KeyKind::Release }),
            Event::Key(Key { code: KeyCode::F(35), mods: Mods(0x3f), kind: KeyKind::Repeat }),
            Event::Key(Key::new(KeyCode::Enter, Mods::SHIFT)),
            Event::Key(Key::new(KeyCode::Char('"'), Mods::NONE)),
            Event::Key(Key::new(KeyCode::Char('\\'), Mods::NONE)),
            Event::Mouse(Mouse { kind: MouseKind::Drag(Button::Right), col: 65535, row: 0, mods: Mods::ALT }),
            Event::Mouse(Mouse { kind: MouseKind::ScrollLeft, col: 3, row: 4, mods: Mods::NONE }),
            Event::Mouse(Mouse { kind: MouseKind::Moved, col: 0, row: 0, mods: Mods::NONE }),
            Event::Paste("a\x1b[201~\r\n\u{0}é😀\u{9b}".into()),
            Event::Focus(false),
            Event::Resize(80, 24),
            Event::Closed("the terminal closed".into()),
        ];
        for event in &table {
            assert_eq!(&back(event), event);
        }
        // The form: serde's default, which the protocol's own test pins down.
        let json = serde_json::to_string(&Event::Key(Key::new(KeyCode::Char('a'), Mods::CTRL))).unwrap();
        assert_eq!(json, r#"{"Key":{"code":{"Char":"a"},"mods":4,"kind":"Press"}}"#);
    }
}
