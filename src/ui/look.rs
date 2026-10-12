// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! How a member looks, on the dashboard and in the menu: its state's color and sign, its effort's, its model's
//! family, its own color, and the stroke down the left of its card. Nothing here reads or writes a file.

use std::time::Duration;

use crossterm::style::Color;

mod effort;
mod error;
mod palette;

pub(crate) use effort::*;
pub(crate) use error::{error_label, error_short, without_code};
pub(crate) use palette::*;

/// How the Alt key is written before a key in what recruit shows: the Option symbol on macOS, `Alt+` elsewhere.
pub const ALT: &str = if cfg!(target_os = "macos") { "⌥" } else { "Alt+" };

/// How often the dashboard redraws while a member works, for the spinners: ten images a second, one turn.
pub(crate) const FRAME: Duration = Duration::from_millis(100);
/// A working member's spinner, one image per frame.
pub(crate) const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    Working,
    Idle,
    /// Waiting for the user, in its terminal.
    Waiting,
    /// Its turn ended on an API error (OSC 7501 `state=error`), until its next turn: orange, a state of its own.
    Error,
    /// Not known: the default.
    #[default]
    Other,
}

impl State {
    pub(crate) fn of(status: &str) -> Self {
        match status {
            "busy" => State::Working,
            "idle" => State::Idle,
            "waiting" => State::Waiting,
            _ => State::Other,
        }
    }

    /// Its name in `states.json` and in the server's list of panes: `working`, `idle`, `waiting` or `other`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Idle => "idle",
            State::Waiting => "waiting",
            State::Error => "error",
            State::Other => "other",
        }
    }

    /// The state [`State::name`] gives; `Other` for any other name.
    pub(crate) fn named(name: &str) -> Self {
        match name {
            "working" => State::Working,
            "idle" => State::Idle,
            "waiting" => State::Waiting,
            "error" => State::Error,
            _ => State::Other,
        }
    }

    /// What a pane's program last said of its state, as the server lists it: none when it said nothing, or a name
    /// this recruit does not know (a later server's).
    pub(crate) fn reported(name: Option<&str>) -> Option<Self> {
        name.map(State::named).filter(|state| *state != State::Other)
    }

    pub(crate) fn color(self) -> Color {
        match self {
            State::Working => palette().working,
            State::Idle => Color::DarkGrey,
            State::Waiting => Color::Red,
            State::Error => palette().orange,
            State::Other => Color::DarkGrey,
        }
    }

    /// Its sign on a card: the spinner's `frame`-th image while working.
    pub(crate) fn icon(self, glyphs: Glyphs, frame: usize) -> char {
        match (self, glyphs) {
            (State::Working, _) => SPINNER[frame % SPINNER.len()],
            // nf-md-clock_outline
            (State::Idle, Glyphs::Nerd) => '\u{F0150}',
            (State::Idle, Glyphs::Unicode) => '◷',
            // nf-md-bell_ring
            (State::Waiting, Glyphs::Nerd) => '\u{F009E}',
            (State::Waiting, Glyphs::Unicode) => '⚑',
            // nf-md-alert_circle_outline (in the Nerd Fonts, Ghostty's own included)
            (State::Error, Glyphs::Nerd) => '\u{F05D6}',
            (State::Error, Glyphs::Unicode) => '✗',
            (State::Other, _) => '◌',
        }
    }

    /// Its sign in the header: a robot for the working ones in Nerd Font, else as on a card.
    pub(crate) fn badge(self, glyphs: Glyphs, frame: usize) -> char {
        match (self, glyphs) {
            // nf-md-robot
            (State::Working, Glyphs::Nerd) => '\u{F06A9}',
            _ => self.icon(glyphs, frame),
        }
    }
}

/// How the states' signs are drawn: Nerd Font icons when every client's terminal carries them, plain Unicode
/// otherwise; one column wide either way, and no emoji.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Glyphs {
    #[default]
    Unicode,
    Nerd,
}

impl Glyphs {
    /// The sign of a subagent, there for a while: an hourglass, one column wide (not ⏳, which takes two).
    pub(crate) fn hourglass(self) -> char {
        match self {
            // nf-md-timer_sand
            Glyphs::Nerd => '\u{F051F}',
            Glyphs::Unicode => '⧗',
        }
    }

    /// The sign of a member at rest while a command it started still runs: a terminal's prompt, one column wide (not
    /// ⌨, which may be drawn as an emoji).
    pub(crate) fn console(self) -> char {
        match self {
            // nf-md-console
            Glyphs::Nerd => '\u{F018D}',
            Glyphs::Unicode => '❯',
        }
    }

    /// The sign of a teammate of the session's own team: a person, one column wide (a pawn: ☺ and ♟ may be drawn as
    /// emoji, two columns).
    pub(crate) fn person(self) -> char {
        match self {
            // nf-md-account
            Glyphs::Nerd => '\u{F0004}',
            Glyphs::Unicode => '♙',
        }
    }
}

/// Terminals that carry the Nerd Font symbols themselves, whatever font the user picked.
pub(crate) const NERD_TERMINALS: [&str; 3] = ["ghostty", "kitty", "wezterm"];

impl Glyphs {
    /// From the clients attached to the team: their `TERM`, and the terminal's name and version when it told it
    /// (`ghostty 1.3.1`, under a `TERM` that may well be `xterm-256color`), empty otherwise.
    pub(crate) fn of<'a>(clients: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let nerd = |text: &str| {
            let text = text.to_lowercase();
            NERD_TERMINALS.iter().any(|t| text.contains(t))
        };
        let clients: Vec<(&str, &str)> =
            clients.into_iter().filter(|(term, name)| !term.trim().is_empty() || !name.trim().is_empty()).collect();
        if !clients.is_empty() && clients.iter().all(|(term, name)| nerd(term) || nerd(name)) {
            Glyphs::Nerd
        } else {
            Glyphs::Unicode
        }
    }
}

/// The spinner's image `elapsed` after the dashboard started.
pub(crate) fn frame(elapsed: Duration) -> usize {
    (elapsed.as_millis() / FRAME.as_millis()) as usize
}

/// The stroke down the left of a card `height` rows high, at `row`: from halfway down the first row to halfway down
/// the last, so that two cards never touch.
pub(crate) fn stroke(row: usize, height: usize) -> &'static str {
    match row {
        0 => "╻",
        r if r + 1 == height => "╹",
        _ => "┃",
    }
}

/// How near a session is to compacting on its own: its context turns orange, then red.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pressure {
    /// Far from it, or not measured yet.
    #[default]
    Calm,
    Near,
    /// Where Claude Code warns.
    Warning,
}

/// From this share of the tokens at which a session compacts on its own, its context turns orange.
const NEAR_COMPACTION: f64 = 0.8;
/// So many tokens before them, Claude Code warns, and the context turns red.
const COMPACTION_WARNING: u64 = 20_000;

impl Pressure {
    /// From the context's tokens and those at which the session compacts.
    pub(crate) fn of(tokens: Option<u64>, compacts_at: Option<u64>) -> Self {
        let (Some(tokens), Some(at)) = (tokens, compacts_at) else { return Pressure::Calm };
        if tokens >= at.saturating_sub(COMPACTION_WARNING) {
            Pressure::Warning
        } else if tokens as f64 >= at as f64 * NEAR_COMPACTION {
            Pressure::Near
        } else {
            Pressure::Calm
        }
    }
}

/// "12s", "22m", "1h05": the time in a state, short.
pub(crate) fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h{:02}", secs / 3600, secs % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_from_the_time() {
        assert_eq!(frame(Duration::ZERO), 0);
        assert_eq!(frame(Duration::from_millis(99)), 0);
        assert_eq!(frame(Duration::from_millis(250)), 2);
        assert_eq!(State::Working.icon(Glyphs::Unicode, frame(Duration::from_millis(250))), '⠹');
        // One turn a second, in either set of glyphs.
        assert_eq!(State::Working.icon(Glyphs::Nerd, frame(Duration::from_millis(1000))), '⠋');
        assert_eq!(State::Idle.icon(Glyphs::Unicode, 3), '◷');
        assert_eq!(State::Waiting.icon(Glyphs::Nerd, 3), '\u{F009E}');
    }

    #[test]
    fn helpers_signs_one_column_wide() {
        use unicode_width::UnicodeWidthChar;
        for glyphs in [Glyphs::Unicode, Glyphs::Nerd] {
            assert_eq!(glyphs.hourglass().width(), Some(1), "{glyphs:?}");
            assert_eq!(glyphs.person().width(), Some(1), "{glyphs:?}");
            assert_eq!(glyphs.console().width(), Some(1), "{glyphs:?}");
        }
    }

    #[test]
    fn nerd_font_only_where_every_terminal_has_it() {
        assert_eq!(Glyphs::of([("xterm-256color", "ghostty 1.3.1")]), Glyphs::Nerd);
        assert_eq!(Glyphs::of([("xterm-kitty", "")]), Glyphs::Nerd);
        assert_eq!(Glyphs::of([("xterm-256color", "WezTerm 20240203-110809-5046fc22")]), Glyphs::Nerd);
        assert_eq!(
            Glyphs::of([("xterm-ghostty", "ghostty 1.3.1"), ("xterm-256color", "iTerm2 3.5.4")]),
            Glyphs::Unicode
        );
        assert_eq!(Glyphs::of([("xterm-256color", "")]), Glyphs::Unicode);
        assert_eq!(Glyphs::of([("", "")]), Glyphs::Unicode);
        assert_eq!(Glyphs::of([]), Glyphs::Unicode);
    }

    #[test]
    fn short_durations() {
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(754), "12m");
        assert_eq!(duration(7500), "2h05");
    }

    #[test]
    fn strokes() {
        assert_eq!([0, 1, 2].map(|row| stroke(row, 3)), ["╻", "┃", "╹"]);
        assert_eq!([0, 1].map(|row| stroke(row, 2)), ["╻", "╹"]);
    }
}
