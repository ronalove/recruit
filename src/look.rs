// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! How a member looks, on the dashboard and in the menu: its state's color and sign, its effort's, its model's
//! family, its own color, and the stroke down the left of its card. Nothing here reads or writes anything.

use std::time::Duration;

use crossterm::style::Color;

use crate::t;

/// How the Alt key is written before a key in what recruit shows: the Option symbol on macOS, `Alt+` elsewhere.
pub const ALT: &str = if cfg!(target_os = "macos") { "⌥" } else { "Alt+" };

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
    /// Its turn ended on an API error (OSC 7501 `state=error`), until its next turn: orange, a state of its own
    /// (decision of 2026-10-10, mock-up C).
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

    pub(crate) fn color(self) -> Color {
        match self {
            State::Working => Color::Yellow,
            State::Idle => Color::DarkGrey,
            State::Waiting => Color::Red,
            State::Error => ORANGE,
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
            // nf-md-alert_circle_outline (mock-up C; in the Nerd Fonts, Ghostty's own included)
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

/// A failed turn's reason, short, for a header, a card or a notice, from the text of the error Claude Code gave
/// (mock-up C): « API surchargée (529) », « connexion perdue », « erreur du serveur (500) », « limite de débit (429) »,
/// « limite d'usage, jusqu'à 15 h » (the hour only when the text gives it); else « erreur d'API », with its status
/// code when the text has one.
pub(crate) fn error_label(message: Option<&str>) -> String {
    let text = message.unwrap_or_default();
    let lower = text.to_lowercase();
    let code = status_code(text);
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&["usage limit", "limit reached", "hit your limit", "limite d'usage"]) {
        return match reset_hour(text) {
            Some(hour) => t!("limite d'usage, jusqu'à {}", "usage limit until {}", hour),
            None => t!("limite d'usage", "usage limit"),
        };
    }
    if code == Some(529) || has(&["overloaded"]) {
        return t!("API surchargée (529)", "API overloaded (529)");
    }
    if code == Some(429) || has(&["rate_limit", "rate limit"]) {
        return t!("limite de débit (429)", "rate limited (429)");
    }
    if let Some(code) = code.filter(|code| (500..600).contains(code)) {
        return t!("erreur du serveur ({})", "server error ({})", code);
    }
    if code.is_none()
        && has(&[
            "connection",
            "econnreset",
            "econnrefused",
            "etimedout",
            "enotfound",
            "socket hang up",
            "fetch failed",
            "network",
            "timed out",
            "timeout",
        ])
    {
        return t!("connexion perdue", "connection lost");
    }
    match code {
        Some(code) => format!("{} ({code})", error_short()),
        None => error_short(),
    }
}

/// A failed turn's reason without the status code at its end: « API surchargée » for « API surchargée (529) ».
pub(crate) fn without_code(label: &str) -> &str {
    match label.strip_suffix(')').and_then(|rest| rest.rsplit_once(" (")) {
        Some((reason, code)) if !code.is_empty() && code.bytes().all(|b| b.is_ascii_digit()) => reason,
        _ => label,
    }
}

/// A failed turn's reason at its shortest: « erreur d'API ».
pub(crate) fn error_short() -> String {
    t!("erreur d'API", "API error")
}

/// The HTTP status in an error's text: « API Error: 529 {…} », or « status 500 ».
fn status_code(text: &str) -> Option<u16> {
    // Positions found in the lowered text cut that text, never the original: lowering changes the length of some
    // characters (« İ »), and only ASCII digits are read after the marker.
    let lower = text.to_lowercase();
    let after = ["api error:", "status code", "status:", "status"]
        .iter()
        .find_map(|marker| lower.find(marker).map(|at| &lower[at + marker.len()..]))?;
    let digits: String = after.trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|code| (400..600).contains(code))
}

/// The hour a usage limit resets at, written for the user: « 15 h », “3 pm”; from « resets 3pm », « resets 3:30pm »,
/// « resets 15:00 », or a time in seconds after a « | » (« usage limit reached|1760000000 »). None when it says none.
fn reset_hour(text: &str) -> Option<String> {
    let (hour, minute) = match text.split_once('|') {
        Some((_, epoch)) => local_time(epoch.trim().parse().ok()?)?,
        None => {
            let lower = text.to_lowercase();
            let rest = &lower[lower.find("reset")?..];
            let rest = rest.split_once(' ')?.1.trim_start();
            let rest = rest.strip_prefix("at ").unwrap_or(rest);
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == ':').collect();
            let (h, m) = digits.split_once(':').unwrap_or((&digits, "0"));
            let (mut hour, minute): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
            let suffix = rest[digits.len()..].trim_start();
            if suffix.starts_with("pm") && hour < 12 {
                hour += 12;
            } else if suffix.starts_with("am") && hour == 12 {
                hour = 0;
            }
            (hour, minute)
        }
    };
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(match crate::i18n::lang() {
        crate::i18n::Lang::Fr if minute == 0 => format!("{hour} h"),
        crate::i18n::Lang::Fr => format!("{hour} h {minute:02}"),
        crate::i18n::Lang::En => {
            let (shown, half) = (if hour % 12 == 0 { 12 } else { hour % 12 }, if hour < 12 { "am" } else { "pm" });
            if minute == 0 { format!("{shown} {half}") } else { format!("{shown}:{minute:02} {half}") }
        }
    })
}

/// The hour and minute of `epoch` (seconds) in local time.
// libc marks musl's `time_t` deprecated over its size on 32-bit targets; it is 64 bits on the ones recruit ships.
#[allow(deprecated)]
fn local_time(epoch: i64) -> Option<(u32, u32)> {
    let time = epoch as libc::time_t;
    // SAFETY: localtime_r only writes the `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return None;
    }
    Some((tm.tm_hour as u32, tm.tm_min as u32))
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
    /// From the clients attached to the team, one per line as `Backend::client_terminals` gives them: their `TERM`,
    /// a tab, then the terminal's name and version when it told it (`ghostty 1.3.1`, under a `TERM` that may well be
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

    /// A failed turn's reason, from the text Claude Code gives (mock-up C), in the language of the run.
    #[test]
    fn failed_turns_said_short() {
        let overloaded =
            r#"API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(error_label(Some(overloaded)), t!("API surchargée (529)", "API overloaded (529)"));
        assert_eq!(error_label(Some("Overloaded")), t!("API surchargée (529)", "API overloaded (529)"));
        let server =
            r#"API Error: 500 {"type":"error","error":{"type":"api_error","message":"Internal server error"}}"#;
        assert_eq!(error_label(Some(server)), t!("erreur du serveur (500)", "server error (500)"));
        assert_eq!(error_label(Some("API Error: 503 upstream")), t!("erreur du serveur (503)", "server error (503)"));
        assert_eq!(error_label(Some("API Error: Request timed out.")), t!("connexion perdue", "connection lost"));
        assert_eq!(error_label(Some("API Error: Connection error.")), t!("connexion perdue", "connection lost"));
        let rate = r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error"}}"#;
        assert_eq!(error_label(Some(rate)), t!("limite de débit (429)", "rate limited (429)"));
        assert_eq!(
            error_label(Some("You've hit your limit · resets 3pm (Europe/Paris)")),
            t!("limite d'usage, jusqu'à 15 h", "usage limit until 3 pm")
        );
        assert_eq!(
            error_label(Some("5-hour limit reached ∙ resets 3:30am")),
            t!("limite d'usage, jusqu'à 3 h 30", "usage limit until 3:30 am")
        );
        assert_eq!(error_label(Some("Claude usage limit reached")), t!("limite d'usage", "usage limit"));
        // Nothing to read: the shortest.
        assert_eq!(error_label(Some("")), t!("erreur d'API", "API error"));
        assert_eq!(error_label(None), t!("erreur d'API", "API error"));
        assert_eq!(error_label(Some("API Error: 400 bad request")), t!("erreur d'API (400)", "API error (400)"));
        assert_eq!(error_short(), t!("erreur d'API", "API error"));
        // Characters whose lowercase is longer or shorter: no panic, the code still read.
        assert_eq!(error_label(Some("İ status 500")), t!("erreur du serveur (500)", "server error (500)"));
        assert_eq!(error_label(Some("ȺȺ API Error: 529")), t!("API surchargée (529)", "API overloaded (529)"));
        assert_eq!(
            error_label(Some("İİ usage limit · resets 3pm")),
            t!("limite d'usage, jusqu'à 15 h", "usage limit until 3 pm")
        );
        assert_eq!(error_label(Some("ȺȺȺ resets İ limit reached")), t!("limite d'usage", "usage limit"));
    }

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
