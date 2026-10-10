// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! How a member looks, on the dashboard and in the menu: its state's color and sign, its effort's, its model's
//! family, its own color, and the stroke down the left of its card. Nothing here reads or writes anything.

use std::time::Duration;

use crossterm::style::Color;

/// How often the dashboard redraws while a member works, for the spinners: ten images a second, one turn.
pub(crate) const FRAME: Duration = Duration::from_millis(100);
/// A working member's spinner, one image per frame.
pub(crate) const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The members' own colors, the journal's too: no yellow, green or red, which tell the states.
pub(crate) const COLORS: [Color; 8] = [
    Color::Cyan,
    Color::Magenta,
    Color::Blue,
    Color::AnsiValue(209),
    Color::AnsiValue(147),
    Color::AnsiValue(80),
    Color::AnsiValue(218),
    Color::AnsiValue(111),
];

/// The `index`-th member's own color, in the order of the team.
pub(crate) fn member_color(index: usize) -> Color {
    COLORS[index % COLORS.len()]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    Working,
    Idle,
    /// Waiting for the user, in its terminal.
    Waiting,
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

    pub(crate) fn color(self) -> Color {
        match self {
            State::Working => Color::Yellow,
            State::Idle => Color::DarkGrey,
            State::Waiting => Color::Red,
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
    /// From the clients attached to the team, one per line as `Tmux::client_terminals` gives them: their `TERM`, then
    /// the terminal's name and version when it told tmux (`ghostty 1.3.1`, under a `TERM` that may well be
    /// `xterm-256color`).
    pub(crate) fn of(clients: &str) -> Self {
        let nerd = |client: &str| {
            let client = client.to_lowercase();
            NERD_TERMINALS.iter().any(|t| client.contains(t))
        };
        let clients: Vec<&str> = clients.lines().filter(|l| !l.trim().is_empty()).collect();
        if !clients.is_empty() && clients.iter().all(|c| nerd(c)) { Glyphs::Nerd } else { Glyphs::Unicode }
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

/// A context near compaction.
pub(crate) const ORANGE: Color = Color::AnsiValue(208);

/// An effort's sign of level.
pub(crate) fn effort_sign(level: &str) -> char {
    match level {
        "low" => '▂',
        "medium" => '▄',
        "high" => '▆',
        _ => '█',
    }
}

/// An effort's color, as in Claude Code; none for `max`, in a rainbow.
pub(crate) fn effort_color(level: &str) -> Option<Color> {
    match level {
        "low" => Some(Color::AnsiValue(179)),
        "medium" => Some(Color::AnsiValue(114)),
        "high" | "xhigh" => Some(Color::AnsiValue(141)),
        _ => None,
    }
}

pub(crate) const RAINBOW: [Color; 6] = [
    Color::AnsiValue(203),
    Color::AnsiValue(209),
    Color::AnsiValue(221),
    Color::AnsiValue(114),
    Color::AnsiValue(75),
    Color::AnsiValue(141),
];

/// The families Claude Code names its models by.
pub(crate) const FAMILIES: [&str; 4] = ["haiku", "sonnet", "opus", "fable"];

/// `claude-opus-5-5` → `Opus`, `us.anthropic.claude-sonnet-4-5-…` → `Sonnet`: the model's family, as Claude Code names
/// it, wherever the id puts it (a Bedrock profile, an ARN); else the id's first word.
pub(crate) fn family(model: &str) -> String {
    let id = model.to_lowercase();
    let word = match FAMILIES.into_iter().find(|family| id.contains(family)) {
        Some(family) => family,
        None => {
            let name = id.strip_prefix("claude-").unwrap_or(&id);
            name.split(|c: char| !c.is_alphanumeric()).next().unwrap_or(name)
        }
    };
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
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
    fn model_families() {
        assert_eq!(family("claude-opus-5-5"), "Opus");
        assert_eq!(family("claude-haiku-4-5-20251001"), "Haiku");
        assert_eq!(family("claude-sonnet-5-5[1m]"), "Sonnet");
        assert_eq!(family("fable"), "Fable");
        // Wherever the id puts it: a Bedrock profile, an ARN.
        assert_eq!(family("us.anthropic.claude-sonnet-4-5-20250929-v1:0"), "Sonnet");
        assert_eq!(
            family("arn:aws:bedrock:us-east-1::foundation-model/anthropic.claude-opus-4-1-20250805-v1:0"),
            "Opus"
        );
        // No family known: the first word.
        assert_eq!(family("mistral-large-2"), "Mistral");
    }

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
        assert_eq!(Glyphs::of("xterm-256color\tghostty 1.3.1\n"), Glyphs::Nerd);
        assert_eq!(Glyphs::of("xterm-kitty\t\n"), Glyphs::Nerd);
        assert_eq!(Glyphs::of("xterm-256color\tWezTerm 20240203-110809-5046fc22\n"), Glyphs::Nerd);
        assert_eq!(Glyphs::of("xterm-ghostty\tghostty 1.3.1\nxterm-256color\tiTerm2 3.5.4\n"), Glyphs::Unicode);
        assert_eq!(Glyphs::of("xterm-256color\t\n"), Glyphs::Unicode);
        assert_eq!(Glyphs::of(""), Glyphs::Unicode);
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
