// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The chrome around the panes: each pane in a frame, its header written in the top border.
//! The pane with the focus has a thick frame in bold, in the terminal's own text color, at work as at rest; the
//! others thin and grey. Only red stays, for a member waiting and for a pane gone wrong, which ask for an action:
//! thick with the focus, thin without. The header keeps the state's
//! styles: the spinner by the name of a member at work, ⚑ in red for one waiting, the name in the member's color.
//! At its right, the model, the effort, the context and the time, then ⤢ in the corner; each part a click reaches
//! comes back from [`frame`], the one under the mouse underlined. Then the bar on the screen's last row: the team,
//! the tabs (the current one in reverse video, each after the sign of its most urgent member: ⚑ for one waiting),
//! then the menu and quit buttons. A notice at the top right when a member out of sight waits; the choices at the
//! center. The signatures are the interface with `screen.rs` and the server.

use crossterm::style::Color;

use crate::ui::canvas::Style;
use crate::ui::look::{Glyphs, State};

mod bar;
mod choice;
mod frame;
mod notice;
#[cfg(test)]
mod tests;

pub(crate) use bar::*;
pub(crate) use choice::*;
pub(crate) use frame::*;
pub(crate) use notice::*;

/// How the chrome draws its signs: Nerd Font or Unicode, and the spinner's image (`look::frame`) for the members at
/// work. While a header or a tab shows one at work, the screen is drawn again every `look::FRAME`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Look {
    pub glyphs: Glyphs,
    pub frame: usize,
}

/// The sign of a state, as on the dashboard: the spinner in yellow, ⚑ in bold red, ✗ in bold orange, the clock in grey.
fn sign_style(state: State) -> Style {
    match state {
        State::Working => Style::fg(crate::ui::look::palette().working),
        State::Waiting => Style::fg(Color::Red).bold(),
        State::Error => Style::fg(crate::ui::look::palette().orange).bold(),
        State::Idle | State::Other => Style::fg(Color::DarkGrey),
    }
}

/// The style underlined: a part under the mouse.
fn underlined(style: Style) -> Style {
    Style { underline: true, ..style }
}

/// The zoom's sign: ⤢ to zoom, ⤡ back; Nerd Font's fullscreen icons when the terminal carries them.
fn zoom_sign(zoom: Zoom, glyphs: Glyphs) -> char {
    match (zoom, glyphs) {
        // nf-md-fullscreen, nf-md-fullscreen_exit
        (Zoom::In, Glyphs::Nerd) => '\u{F0293}',
        (Zoom::Out, Glyphs::Nerd) => '\u{F0294}',
        (Zoom::In, Glyphs::Unicode) => '⤢',
        (Zoom::Out, Glyphs::Unicode) => '⤡',
    }
}
