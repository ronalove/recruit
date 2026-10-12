// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! recruit's own multiplexer (specs/multiplexeur.md): a server per team (`recruit _server`) that runs the panes, the
//! clients that show it, and the handle the rest of recruit uses to talk to the server.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};

use crate::ui::Rgb;

pub(crate) mod cards;
pub(crate) mod chrome;
pub(crate) mod client;
pub(crate) mod ctl;
pub(crate) mod engine;
pub(crate) mod focus;
pub(crate) mod handle;
pub(crate) mod input;
pub(crate) mod keys;
pub(crate) mod layout;
pub(crate) mod plan;
pub(crate) mod proto;
pub(crate) mod pty;
pub(crate) mod screen;
pub(crate) mod select;
pub(crate) mod server;
pub(crate) mod socket;

pub(crate) use handle::{Mux, running};

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

/// What the real terminal can do, found once by the client when it starts (spec §5.3), before it reads any event:
/// queries written together, DA1 last since every terminal answers it, replies read until DA1's or a short delay.
/// Filled by `input::Terminal::open`; read by the painter and the engines.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Caps {
    /// What the painter may send: synchronized updates, 24-bit colors, OSC 8 links.
    pub output: crate::ui::canvas::Features,
    /// The kitty keyboard protocol (`CSI ? u` answered): pushed, keys come in unambiguous.
    pub kitty_keyboard: bool,
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
