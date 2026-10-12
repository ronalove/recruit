// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the probe asks the real terminal when the client starts, and what it makes of the answers: the kitty
//! keyboard protocol, synchronized updates, grapheme clusters, the default colors, the name; then [`Caps`].

use std::time::Duration;

use super::parse::Reply;
use crate::mux::Caps;
use crate::ui::Rgb;
use crate::ui::canvas::Features;

/// The queries of the probe, written together: the kitty keyboard protocol's flags, DECRQM for synchronized updates
/// (2026) and grapheme clusters (2027), the default colors (OSC 10, 11), the name (XTVERSION); DA1 last, which
/// every terminal answers, so that its answer ends the probe.
pub(super) const QUERIES: &[u8] = b"\x1b[?u\x1b[?2026$p\x1b[?2027$p\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[>0q\x1b[c";

/// The longest the probe waits for DA1's answer; a terminal answers in a few milliseconds, a few tens by SSH.
pub(super) const PROBE_TIME: Duration = Duration::from_millis(500);

/// What the probe heard.
#[derive(Debug, Default)]
pub(super) struct Answers {
    pub(super) kitty: Option<u32>,
    /// DECRPM: mode and its state.
    pub(super) modes: Vec<(u32, u32)>,
    pub(super) fg: Option<Rgb>,
    pub(super) bg: Option<Rgb>,
    pub(super) name: Option<String>,
    /// DA1 came: nothing more will.
    pub(super) done: bool,
    /// Everything read, and how long it took, for [`report`].
    pub(super) raw: Vec<u8>,
    pub(super) took: Duration,
}

/// What the probe heard, written where `RECRUIT_MUX_CAPS` says (hidden; read when the native client opens the
/// terminal, `client.rs`), for the terminals' grid: the terminal's environment, its answers escaped, a sequence a
/// line, and what was made of them.
pub(super) fn report(answers: &Answers, caps: &Caps, env: impl Fn(&str) -> Option<String>) -> String {
    let escaped = |bytes: &[u8]| bytes.escape_ascii().to_string();
    let mut out = format!(
        "recruit {} terminal probe\ntook: {} ms; DA1 answered: {}\n\nenvironment:\n",
        env!("CARGO_PKG_VERSION"),
        answers.took.as_millis(),
        if answers.done { "yes" } else { "no" },
    );
    for name in ["TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "COLORTERM", "VTE_VERSION", "LC_TERMINAL", "TMUX"] {
        out += &format!("  {name}={}\n", env(name).unwrap_or_else(|| "(unset)".into()));
    }
    out += &format!("  over SSH: {}\n", if env("SSH_TTY").is_some() { "yes" } else { "no" });
    out += &format!("\nqueries:\n  {}\n\nanswers:\n", escaped(QUERIES));
    let mut starts: Vec<usize> = answers
        .raw
        .iter()
        .enumerate()
        .filter(|&(i, &b)| b == 0x1b && answers.raw.get(i + 1) != Some(&b'\\'))
        .map(|(i, _)| i)
        .collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(answers.raw.len());
        if start < end {
            out += &format!("  {}\n", escaped(&answers.raw[start..end]));
        }
    }
    out += &format!("\ncaps: {caps:#?}\n");
    out
}

impl Answers {
    pub(super) fn note(&mut self, reply: Reply) {
        match reply {
            Reply::Kitty(flags) => self.kitty = Some(flags),
            Reply::Mode { mode, value } => self.modes.push((mode, value)),
            Reply::Osc(body) => {
                let body = String::from_utf8_lossy(&body);
                match body.split_once(';') {
                    Some(("10", spec)) => self.fg = rgb(spec),
                    Some(("11", spec)) => self.bg = rgb(spec),
                    _ => {}
                }
            }
            Reply::Dcs(body) => {
                if let Some(name) = body.strip_prefix(b">|") {
                    let name = String::from_utf8_lossy(name).trim().to_string();
                    self.name = (!name.is_empty()).then_some(name);
                }
            }
            Reply::Primary => self.done = true,
            Reply::Other => {}
        }
    }

    /// DECRPM says the mode can be set: set (1), reset (2) or set for good (3); 0 is unknown, 4 reset for good.
    pub(super) fn settable(&self, mode: u32) -> bool {
        self.modes.iter().any(|&(m, value)| m == mode && (1..=3).contains(&value))
    }
}

/// An X11 color spec as terminals answer OSC 10 and 11: `rgb:r/g/b`, 1 to 4 hex digits each.
fn rgb(spec: &str) -> Option<Rgb> {
    let parts = spec.strip_prefix("rgb:").or_else(|| spec.strip_prefix("rgba:"))?;
    let mut channels = parts.split('/').map(|part| {
        let value = u32::from_str_radix(part, 16).ok().filter(|_| (1..=4).contains(&part.len()))?;
        let max = (1u32 << (4 * part.len())) - 1;
        Some(((value * 255 + max / 2) / max) as u8)
    });
    Some((channels.next()??, channels.next()??, channels.next()??))
}

/// Terminals known to show 24-bit colors and OSC 8 links, by the start of their name (XTVERSION, `TERM_PROGRAM`,
/// `TERM`), lower case. tmux takes both and passes them on, or not, as its own terminal can.
const KNOWN: &[&str] = &[
    "ghostty",
    "kitty",
    "xterm-kitty",
    "wezterm",
    "iterm",
    "foot",
    "tmux",
    "alacritty",
    "contour",
    "rio",
    "vscode",
    "warpterminal",
    "konsole",
];

/// What the terminal can do. Synchronized updates and grapheme clusters, from DECRPM. 24-bit colors and links have
/// no query: a terminal known by name has them, or one that says so (`COLORTERM`, a `-direct` `TERM`; VTE from 0.50
/// for links); otherwise 256 colors and no links, which every terminal shows right.
pub(super) fn caps(answers: &Answers, env: impl Fn(&str) -> Option<String>) -> Caps {
    let names: Vec<String> = [answers.name.clone(), env("TERM_PROGRAM"), env("TERM")]
        .into_iter()
        .flatten()
        .map(|name| name.to_lowercase())
        .collect();
    let known = names.iter().any(|name| KNOWN.iter().any(|known| name.starts_with(known)));
    let colorterm = env("COLORTERM").is_some_and(|value| matches!(value.as_str(), "truecolor" | "24bit"));
    let direct = env("TERM").is_some_and(|term| term.ends_with("-direct"));
    let vte = env("VTE_VERSION").and_then(|v| v.parse::<u32>().ok()).is_some_and(|v| v >= 5000);
    Caps {
        output: Features {
            sync: answers.settable(2026),
            truecolor: known || colorterm || direct,
            links: known || vte,
            // Known once the terminal is set up: see `setup`.
            graphemes: false,
        },
        kitty_keyboard: answers.kitty.is_some(),
        fg: answers.fg,
        bg: answers.bg,
        name: answers.name.clone(),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::mux::input::parse::{Parser, Token};
    use crate::mux::input::{Event, Key, KeyCode, Mods};

    pub(crate) fn heard(bytes: &[u8]) -> (Answers, Vec<Event>) {
        let mut parser = Parser::default();
        let mut tokens = Vec::new();
        parser.feed(bytes, &mut tokens);
        let mut answers = Answers::default();
        let mut events = Vec::new();
        for token in tokens {
            match token {
                Token::Reply(reply) => answers.note(reply),
                Token::Event(event) => events.push(event),
            }
        }
        (answers, events)
    }

    pub(crate) fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    #[test]
    fn colors() {
        let table = [
            ("rgb:1e1e/1e1e/2e2e", Some((30, 30, 46))),
            ("rgb:ffff/ffff/ffff", Some((255, 255, 255))),
            ("rgb:00/80/ff", Some((0, 128, 255))),
            ("rgb:f/0/8", Some((255, 0, 136))),
            ("rgba:ffff/0000/0000/ffff", Some((255, 0, 0))),
            ("rgb:12345/0/0", None),
            ("#ffffff", None),
            ("rgb:ff/ff", None),
        ];
        for (spec, color) in table {
            assert_eq!(rgb(spec), color, "{spec}");
        }
    }

    #[test]
    fn ghostty_answers_everything() {
        // Ghostty 1.2's answers to QUERIES, with a key typed meanwhile.
        let (answers, events) = heard(
            b"\x1b[?0u\x1b[?2026;2$y\x1b[?2027;2$yx\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b]11;rgb:2828/2c2c/3434\x1b\\\
              \x1bP>|ghostty 1.2.0\x1b\\\x1b[?62;22;52c",
        );
        assert!(answers.done);
        assert_eq!(events, [Event::Key(Key::new(KeyCode::Char('x'), Mods::NONE))]);
        let caps = caps(&answers, env(&[]));
        assert_eq!(
            caps,
            Caps {
                output: Features { sync: true, truecolor: true, links: true, graphemes: false },
                kitty_keyboard: true,
                fg: Some((255, 255, 255)),
                bg: Some((40, 44, 52)),
                name: Some("ghostty 1.2.0".into()),
            }
        );
    }

    #[test]
    fn a_plain_terminal_answers_da1_only() {
        // Terminal.app: DA1, and nothing else it knows.
        let (answers, events) = heard(b"\x1b[?1;2c");
        assert!(answers.done && events.is_empty());
        let caps = caps(&answers, env(&[("TERM_PROGRAM", "Apple_Terminal"), ("TERM", "xterm-256color")]));
        assert_eq!(
            caps,
            Caps {
                output: Features { sync: false, truecolor: false, links: false, graphemes: false },
                kitty_keyboard: false,
                fg: None,
                bg: None,
                name: None,
            }
        );
        // It says it has 24-bit colors.
        assert!(super::caps(&answers, env(&[("COLORTERM", "truecolor")])).output.truecolor);
        assert!(super::caps(&answers, env(&[("TERM", "xterm-direct")])).output.truecolor);
        let vte = super::caps(&answers, env(&[("VTE_VERSION", "7600"), ("COLORTERM", "truecolor")]));
        assert!(vte.output.links && vte.output.truecolor);
        // Known by name, through SSH (no TERM_PROGRAM, but TERM and XTVERSION).
        assert!(super::caps(&answers, env(&[("TERM", "xterm-kitty")])).output.links);
        let (named, _) = heard(b"\x1bP>|WezTerm 20240203\x1b\\\x1b[?1;2c");
        assert!(super::caps(&named, env(&[])).output.links);
    }

    #[test]
    fn the_probe_report() {
        let (mut answers, _) = heard(b"\x1b[?0u\x1b[?1;2c");
        answers.raw = b"\x1b[?0u\x1b[?1;2c".to_vec();
        let caps = caps(&answers, env(&[]));
        let report = report(&answers, &caps, env(&[("TERM", "xterm-256color")]));
        assert!(report.contains("DA1 answered: yes"), "{report}");
        assert!(report.contains("  TERM=xterm-256color\n  TERM_PROGRAM=(unset)\n"), "{report}");
        assert!(report.contains("answers:\n  \\x1b[?0u\n  \\x1b[?1;2c\n"), "{report}");
        assert!(report.contains("kitty_keyboard: true"), "{report}");
    }

    #[test]
    fn decrpm_states() {
        let (answers, _) = heard(b"\x1b[?2026;0$y\x1b[?2027;4$y");
        assert!(!answers.settable(2026) && !answers.settable(2027));
        let (answers, _) = heard(b"\x1b[?2026;1$y\x1b[?2027;3$y");
        assert!(answers.settable(2026) && answers.settable(2027));
    }
}
