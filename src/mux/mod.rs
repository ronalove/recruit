// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! recruit's own multiplexer, to replace tmux (specs/multiplexeur.md): a server per team (`recruit _server`), the
//! clients that show it, and `Native`, the [`crate::backend::Backend`] that drives it. `RECRUIT_BACKEND=native` until
//! it is the default (step 4).
//!
//! Who owns what:
//! - `pty.rs`, `engine.rs` and `engine/`: the terminal each pane runs in (dev-terminal). Nothing of the VT engine's
//!   own types leaves `engine/`: the rest of the code sees the [`engine::Engine`] trait only.
//! - `input.rs` and `input/`, `keys.rs`: the real terminal's events read, the shortcuts, and what goes to a pane
//!   encoded as its modes ask (dev-saisie).
//! - `screen.rs`, with `canvas.rs`, and `select.rs`: panes composed into one screen, sent by difference; selection
//!   and copy (dev-rendu).
//! - `chrome.rs`: the frames, their headers and the bar, direction « Cadres » (dev-interface).
//! - `proto.rs`, `socket.rs`, `server.rs` and `server/`, `client.rs`, `ctl.rs`, `native.rs`: the server, its clients,
//!   what they say to each other, and the backend (dev-serveur).

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};

pub(crate) mod chrome;
pub(crate) mod client;
pub(crate) mod ctl;
pub(crate) mod engine;
pub(crate) mod input;
pub(crate) mod keys;
pub(crate) mod native;
pub(crate) mod proto;
pub(crate) mod pty;
pub(crate) mod screen;
pub(crate) mod select;
pub(crate) mod server;
pub(crate) mod socket;

thread_local! {
    /// Set while [`caught`] runs: a panic then is handled where it happens.
    static CATCHING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f`, and gives back the message of a panic in it, for an engine that must fail alone (spec §5.2): the
/// pane is given a new one, the rest goes on.
pub(crate) fn caught<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    let before = CATCHING.replace(true);
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    CATCHING.set(before);
    result.map_err(|panic| {
        panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    })
}

/// Whether the panic under way is one [`caught`] handles: a panic hook must then neither give the terminal back
/// nor write on it.
pub(crate) fn catching() -> bool {
    CATCHING.get()
}

/// A rectangle of the screen, in cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Rect {
    pub(crate) fn contains(&self, col: usize, row: usize) -> bool {
        (self.x..self.x + self.width).contains(&col) && (self.y..self.y + self.height).contains(&row)
    }
}

/// A color as the real terminal reports it (OSC 10, 11).
pub(crate) type Rgb = (u8, u8, u8);

/// What the real terminal can do, found once by the client when it starts (spec §5.3), before it reads any event:
/// queries written together, DA1 last since every terminal answers it, replies read until DA1's or a short delay.
/// Filled by `input::Terminal::open` (dev-saisie); read by the painter (dev-rendu) and the engines (dev-terminal).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Caps {
    /// What the painter may send: synchronized updates, 24-bit colors, OSC 8 links.
    pub output: crate::canvas::Features,
    /// The kitty keyboard protocol (`CSI ? u` answered): pushed, keys come in unambiguous.
    pub kitty_keyboard: bool,
    /// Mode 2027 (grapheme clusters measured as a whole) reported as settable.
    pub graphemes: bool,
    /// The terminal's default colors (OSC 10, 11), for the panes that ask theirs: Claude Code picks its light or
    /// dark theme from the background.
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    /// Its name and version (XTVERSION), when it gives them.
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_caught_says_why() {
        assert_eq!(caught(|| 2), Ok(2));
        assert_eq!(caught(catching), Ok(true), "hooks know");
        // The hook is left as it is: tests run side by side, and others panic on purpose.
        assert_eq!(caught(|| -> () { panic!("engine {}", 7) }), Err("engine 7".to_string()));
        assert_eq!(caught(|| -> () { panic!("engine") }), Err("engine".to_string()));
        assert!(!catching());
    }
}
