// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What a program writes that the engine's library does not take care of, found in its output before the engine
//! parses it: the program status query (`OSC 7501 ; ?`), which wants an answer; desktop notifications (OSC 9, 777,
//! 99) and progress (OSC 9;4), which go to the real terminal and the interface; the program's status (OSC 7501), the
//! size it asks for (XTWINOPS 8), RIS, the end of a synchronized update (mode 2026) and mode 2031 turned on, for the
//! engine and the interface. What the library answers or reports itself (XTVERSION, the color scheme query, the
//! clipboard, the title, the bell) is not looked for.
//!
//! Only escape sequences are looked at: text is skipped with `memchr`, the sequences that matter are kept, the
//! others are followed without being stored (an OSC 52 of several megabytes costs nothing). A sequence may be cut
//! between two reads.

use super::base64;
use crate::mux::engine::{Progress, State, Status};

/// What was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Sniffed {
    Notify {
        title: Option<String>,
        body: String,
    },
    Progress(Progress),
    /// `OSC 7501 ; ?`: whether the terminal takes program statuses.
    StatusQuery,
    Status(Status),
    /// RIS (`ESC c`): the terminal back to its initial state, program status included.
    Reset,
    /// `CSI ? 2026 l` (mode 2026 among others, maybe): the end of a synchronized update, wherever it is in a read.
    SyncEnd,
    /// `CSI ? 2031 h` (mode 2031 among others, maybe): the program asks to be told when the theme changes.
    ThemeReports,
    /// `CSI 8 ; rows ; cols t` (XTWINOPS): the program asks for a screen of this size.
    Resize {
        rows: u16,
        cols: u16,
    },
}

/// Bytes of a CSI kept, enough for those looked for: a mode reset that lists 2026 among a dozen others too. Past it,
/// what is kept is cut, and a cut parameter that would read 2026 does no harm (the engine checks the mode itself).
const CSI_KEPT: usize = 64;

/// The largest side, in cells, that XTWINOPS 8 is read for: a request for more (or for 0, which means "as now") is
/// not one a list under a header makes.
const RESIZE_MAX: u16 = 1000;

/// Bytes of an OSC kept, past which a notification is dropped.
const OSC_KEPT: usize = 64 * 1024;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Scan {
    #[default]
    Ground,
    Escape,
    Csi,
    /// An OSC, kept while it may be one of ours.
    Osc,
    /// An OSC that is not, followed to its end.
    OscSkip,
    /// DCS, SOS, PM, APC: followed to their end.
    String,
    /// ESC in one of the above: `\` ends it, anything else starts another sequence.
    OscEscape,
    OscSkipEscape,
    StringEscape,
}

#[derive(Default)]
pub(super) struct Sniffer {
    state: Scan,
    kept: Vec<u8>,
    /// The kitty notification being put together (OSC 99 with `d=0`): its id, title and body.
    kitty: Option<(String, String, String)>,
    /// The byte just stepped on starts the next sequence (it followed the ESC that ended a string, and was not `\`):
    /// [`Sniffer::scan`] reads it again, so that what ends there is found.
    replay: bool,
}

impl Sniffer {
    /// Reads `bytes` up to the end of the first sequence found, if one is. Returns how many bytes were read, and what
    /// was found at their end.
    pub(super) fn scan(&mut self, bytes: &[u8]) -> (usize, Option<Sniffed>) {
        let mut at = 0;
        while at < bytes.len() {
            if self.state == Scan::Ground {
                match memchr::memchr(ESC, &bytes[at..]) {
                    Some(escape) => {
                        at += escape + 1;
                        self.state = Scan::Escape;
                        continue;
                    }
                    None => return (bytes.len(), None),
                }
            }
            let byte = bytes[at];
            at += 1;
            let found = self.step(byte);
            if std::mem::take(&mut self.replay) {
                at -= 1;
            }
            if let Some(found) = found {
                return (at, Some(found));
            }
        }
        (at, None)
    }

    fn step(&mut self, byte: u8) -> Option<Sniffed> {
        match self.state {
            Scan::Ground => {
                if byte == ESC {
                    self.state = Scan::Escape;
                }
            }
            Scan::Escape => {
                self.kept.clear();
                self.state = match byte {
                    b'[' => Scan::Csi,
                    b']' => Scan::Osc,
                    b'P' | b'X' | b'^' | b'_' => Scan::String,
                    ESC => Scan::Escape,
                    b'c' => {
                        self.state = Scan::Ground;
                        return Some(Sniffed::Reset);
                    }
                    _ => Scan::Ground,
                };
            }
            Scan::Csi => match byte {
                ESC => self.state = Scan::Escape,
                CAN | SUB => self.state = Scan::Ground,
                0x20..=0x3f => {
                    if self.kept.len() < CSI_KEPT {
                        self.kept.push(byte);
                    }
                }
                0x40..=0x7e => {
                    self.state = Scan::Ground;
                    return match (&self.kept[..], byte) {
                        (kept, b'l') if lists_mode(kept, b"2026") => Some(Sniffed::SyncEnd),
                        (kept, b'h') if lists_mode(kept, b"2031") => Some(Sniffed::ThemeReports),
                        // A cut sequence may have lost the parameters that made it something else.
                        (kept, b't') if kept.len() < CSI_KEPT => window_size(kept),
                        _ => None,
                    };
                }
                // C0 controls run inside a CSI without ending it; the rest is malformed and ends it.
                0x00..=0x1f => {}
                _ => self.state = Scan::Ground,
            },
            Scan::Osc | Scan::OscSkip => match byte {
                BEL => return self.end_osc(),
                ESC => self.state = if self.state == Scan::Osc { Scan::OscEscape } else { Scan::OscSkipEscape },
                CAN | SUB => self.state = Scan::Ground,
                _ if self.state == Scan::Osc => {
                    self.kept.push(byte);
                    if self.kept.len() > OSC_KEPT || !self.may_be_ours() {
                        self.kept.clear();
                        self.state = Scan::OscSkip;
                    }
                }
                _ => {}
            },
            Scan::OscEscape | Scan::OscSkipEscape => {
                let found = self.end_osc();
                self.next_sequence(byte);
                return found;
            }
            Scan::String => {
                if byte == ESC {
                    self.state = Scan::StringEscape;
                }
            }
            Scan::StringEscape => self.next_sequence(byte),
        }
        None
    }

    /// The ESC that ended a string was followed by `byte`: `\` ends it for good, anything else starts the next
    /// sequence, which reads `byte` as its first.
    fn next_sequence(&mut self, byte: u8) {
        if byte == b'\\' {
            self.state = Scan::Ground;
        } else {
            self.state = Scan::Escape;
            self.replay = true;
        }
    }

    /// Whether the OSC kept so far may still be 9, 777 or 99.
    fn may_be_ours(&self) -> bool {
        let number = self.kept.split(|&b| b == b';').next().unwrap_or_default();
        let complete = self.kept.len() > number.len();
        [&b"9"[..], b"777", b"99", b"7501"]
            .iter()
            .any(|ours| if complete { number == *ours } else { ours.starts_with(number) })
    }

    fn end_osc(&mut self) -> Option<Sniffed> {
        let kept = std::mem::take(&mut self.kept);
        let ours = self.state == Scan::Osc || self.state == Scan::OscEscape;
        self.state = Scan::Ground;
        if !ours {
            return None;
        }
        let text = String::from_utf8_lossy(&kept);
        let (number, rest) = text.split_once(';').unwrap_or((&text, ""));
        match number {
            "9" => osc_9(rest),
            "777" => {
                let mut fields = rest.splitn(3, ';');
                if fields.next() != Some("notify") {
                    return None;
                }
                let title = fields.next().unwrap_or_default();
                let body = fields.next().unwrap_or_default();
                Some(Sniffed::Notify { title: (!title.is_empty()).then(|| title.to_string()), body: body.to_string() })
            }
            "99" => self.osc_99(rest),
            "7501" => osc_7501(rest),
            _ => None,
        }
    }

    /// Kitty's notifications: `99 ; key=value:… ; payload`, the payload a title (by default) or a body, put
    /// together over several sequences of the same id while `d=0`.
    fn osc_99(&mut self, rest: &str) -> Option<Sniffed> {
        let (metadata, payload) = rest.split_once(';').unwrap_or((rest, ""));
        let (mut id, mut done, mut part, mut encoded) = ("", true, "title", false);
        for pair in metadata.split(':') {
            match pair.split_once('=') {
                Some(("i", value)) => id = value,
                Some(("d", value)) => done = value != "0",
                Some(("p", value)) => part = value,
                Some(("e", value)) => encoded = value == "1",
                _ => {}
            }
        }
        if !matches!(part, "title" | "body") {
            return None;
        }
        let payload =
            if encoded { String::from_utf8_lossy(&base64::decode(payload)?).into_owned() } else { payload.into() };
        let mut note = match self.kitty.take() {
            Some(note) if note.0 == id => note,
            _ => (id.to_string(), String::new(), String::new()),
        };
        if note.1.len() + note.2.len() + payload.len() > OSC_KEPT {
            return None;
        }
        if part == "title" { &mut note.1 } else { &mut note.2 }.push_str(&payload);
        if !done {
            self.kitty = Some(note);
            return None;
        }
        let (_, title, body) = note;
        Some(if body.is_empty() {
            Sniffed::Notify { title: None, body: title }
        } else {
            Sniffed::Notify { title: (!title.is_empty()).then_some(title), body }
        })
    }
}

/// Whether the parameters of a CSI (`?1000;2031`) are those of a private mode list that has `mode` among them.
fn lists_mode(params: &[u8], mode: &[u8]) -> bool {
    params.strip_prefix(b"?").is_some_and(|modes| modes.split(|&byte| byte == b';').any(|listed| listed == mode))
}

/// `8 ; rows ; cols` (the parameters of a CSI ending with `t`): exactly these three, plain decimal numbers, the last
/// two from 1 to [`RESIZE_MAX`]. Other XTWINOPS (`18`, `22 ; 0`…), and any that is not well formed, are left alone.
fn window_size(params: &[u8]) -> Option<Sniffed> {
    let mut fields = params.split(|&byte| byte == b';');
    let number = |field: Option<&[u8]>| -> Option<u16> {
        let field = field.filter(|field| !field.is_empty() && field.iter().all(u8::is_ascii_digit))?;
        std::str::from_utf8(field).ok()?.parse().ok()
    };
    if fields.next() != Some(b"8") {
        return None;
    }
    let (rows, cols) = (number(fields.next())?, number(fields.next())?);
    let in_range = |side: u16| (1..=RESIZE_MAX).contains(&side);
    (fields.next().is_none() && in_range(rows) && in_range(cols)).then_some(Sniffed::Resize { rows, cols })
}

/// `7501 ; ?` asks; `7501 ; key=value:…` tells. A state unknown here is left alone.
fn osc_7501(rest: &str) -> Option<Sniffed> {
    if rest.starts_with('?') {
        return Some(Sniffed::StatusQuery);
    }
    let mut status = Status { state: State::Clear, message: None, id: None, app: None };
    let mut state = None;
    let decoded = |text: &str| base64::decode(text).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    for field in rest.split(':') {
        let Some((key, value)) = field.split_once('=') else { continue };
        match key {
            "state" => {
                state = match value {
                    "idle" => Some(State::Idle),
                    "working" => Some(State::Working),
                    "blocked" => Some(State::Blocked),
                    "done" => Some(State::Done),
                    // A turn that failed on an API error (Claude Code 2.1.296), the failure in `msg`.
                    "error" => Some(State::Error),
                    "clear" => Some(State::Clear),
                    _ => None,
                }
            }
            "id" => status.id = Some(value.to_string()),
            "app" => status.app = Some(value.to_string()),
            "msg" => status.message = decoded(value),
            _ => {}
        }
    }
    status.state = state?;
    Some(Sniffed::Status(status))
}

/// `9 ; 4 ; state ; percent` is progress (ConEmu); `9 ; text` a notification (iTerm2); ConEmu's other commands
/// (`9 ; number ; …`) are left alone.
fn osc_9(rest: &str) -> Option<Sniffed> {
    let mut fields = rest.split(';');
    let first = fields.next().unwrap_or_default();
    if first == "4" {
        let state = fields.next().unwrap_or_default();
        let percent = fields.next().and_then(|p| p.trim().parse::<u32>().ok()).map(|p| p.min(100) as u8);
        return Some(Sniffed::Progress(match state {
            "1" => Progress::Set(percent.unwrap_or(0)),
            "2" => Progress::Error(percent),
            "3" => Progress::Indeterminate,
            "4" => Progress::Paused(percent),
            _ => Progress::Clear,
        }));
    }
    if !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit()) && rest.len() > first.len() {
        return None;
    }
    (!rest.is_empty()).then(|| Sniffed::Notify { title: None, body: rest.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything found in `chunks`, fed one after the other.
    fn sniff(chunks: &[&[u8]]) -> Vec<Sniffed> {
        let mut sniffer = Sniffer::default();
        let mut found = Vec::new();
        for chunk in chunks {
            let mut rest = *chunk;
            while !rest.is_empty() {
                let (used, item) = sniffer.scan(rest);
                found.extend(item);
                rest = &rest[used..];
            }
        }
        found
    }

    #[test]
    fn what_the_library_answers_is_not_looked_for() {
        // XTVERSION and the color scheme query: answered by libghostty-vt's callbacks.
        assert_eq!(sniff(&[b"text\x1b[>q more \x1b[>0q\x1b[?996n\x1b[>1q\x1b[q"]), []);
    }

    #[test]
    fn ends_of_synchronized_updates_are_found() {
        assert_eq!(
            sniff(&[b"\x1b[?2026h a \x1b[?2026l b \x1b[?1000;2026l\x1b[?20260l\x1b[2026l\x1b[?2026h"]),
            [Sniffed::SyncEnd, Sniffed::SyncEnd]
        );
        assert_eq!(sniff(&[b"\x1b[?20", b"26", b"l"]), [Sniffed::SyncEnd]);
        // At the end of a long list of modes, cut anywhere.
        assert_eq!(sniff(&[b"\x1b[?1000;1002;1006;2026l"]), [Sniffed::SyncEnd]);
        assert_eq!(sniff(&[b"\x1b[?1000;1002;10", b"06;1004;2004;2026l"]), [Sniffed::SyncEnd]);
    }

    #[test]
    fn modes_turned_on_for_the_theme_are_found() {
        assert_eq!(
            sniff(&[b"a\x1b[?2031h b \x1b[?1000;2031;1006h\x1b[?2031l\x1b[2031h\x1b[?20310h\x1b[?2026h\x1b[?203h"]),
            [Sniffed::ThemeReports, Sniffed::ThemeReports]
        );
        // Cut anywhere.
        assert_eq!(sniff(&[b"\x1b[?20", b"31", b"h"]), [Sniffed::ThemeReports]);
        assert_eq!(sniff(&[b"\x1b[?1000;1002;10", b"06;1004;2004;2031h"]), [Sniffed::ThemeReports]);
    }

    #[test]
    fn window_sizes_are_found() {
        let size = |rows, cols| Sniffed::Resize { rows, cols };
        assert_eq!(
            sniff(&[b"text\x1b[8;24;80t more \x1b[8;1;1000t\x1b[8;030;0100t"]),
            [size(24, 80), size(1, 1000), size(30, 100)]
        );
        // Cut anywhere.
        assert_eq!(sniff(&[b"\x1b", b"[", b"8;2", b"4;", b"80", b"t"]), [size(24, 80)]);
        // Missing, empty, or surplus parameters: not a request (0 would mean "as now" anyway).
        for bad in ["\x1b[8t", "\x1b[8;24t", "\x1b[8;24;t", "\x1b[8;;80t", "\x1b[8;;t", "\x1b[8;24;80;1t"] {
            assert_eq!(sniff(&[bad.as_bytes()]), [], "{bad:?}");
        }
        // Out of bounds, or not plain numbers.
        for bad in [
            "\x1b[8;0;80t",
            "\x1b[8;24;0t",
            "\x1b[8;1001;80t",
            "\x1b[8;24;1001t",
            "\x1b[8;99999;80t",
            "\x1b[8;24;65536t",
            "\x1b[8;-1;80t",
            "\x1b[8;24;+80t",
            "\x1b[8;2.4;80t",
            "\x1b[8:24:80t",
            "\x1b[?8;24;80t",
            "\x1b[>8;24;80t",
            "\x1b[8;24;80 t",
            "\x1b[8;24;80T",
            "\x1b[8;24;80u",
        ] {
            assert_eq!(sniff(&[bad.as_bytes()]), [], "{bad:?}");
        }
        // The other XTWINOPS: not ours.
        for other in ["\x1b[18t", "\x1b[19t", "\x1b[14t", "\x1b[22;0t", "\x1b[23;0t", "\x1b[9;1t", "\x1b[48;24;80t"] {
            assert_eq!(sniff(&[other.as_bytes()]), [], "{other:?}");
        }
        // Digits after a very long run of zeros are not read as if the run was the whole number.
        let long = format!("\x1b[8;24;{}80t", "0".repeat(100));
        assert_eq!(sniff(&[long.as_bytes()]), []);
        // Aborted by CAN, or by another sequence: nothing; the other sequence still counts.
        assert_eq!(sniff(&[b"\x1b[8;24;\x1880t"]), []);
        assert_eq!(sniff(&[b"\x1b[8;24;\x1b[?2031h"]), [Sniffed::ThemeReports]);
        // A C0 control inside it does not end it.
        assert_eq!(sniff(&[b"\x1b[8;24;\n80t"]), [size(24, 80)]);
    }

    #[test]
    fn notifications_and_progress() {
        let note =
            |title: Option<&str>, body: &str| Sniffed::Notify { title: title.map(Into::into), body: body.into() };
        assert_eq!(sniff(&[b"\x1b]9;Claude needs you\x07"]), [note(None, "Claude needs you")]);
        assert_eq!(sniff(&[b"\x1b]777;notify;Claude;Done\x1b\\"]), [note(Some("Claude"), "Done")]);
        assert_eq!(
            sniff(&[b"\x1b]9;4;1;42\x1b\\\x1b]9;4;0\x07\x1b]9;4;3;\x07\x1b]9;4;2\x07"]),
            [
                Sniffed::Progress(Progress::Set(42)),
                Sniffed::Progress(Progress::Clear),
                Sniffed::Progress(Progress::Indeterminate),
                Sniffed::Progress(Progress::Error(None)),
            ]
        );
        // ConEmu's other commands are not notifications.
        assert_eq!(sniff(&[b"\x1b]9;9;/tmp\x07"]), []);
        // Kitty's, in two parts, the body in base64.
        assert_eq!(
            sniff(&[b"\x1b]99;i=1:d=0;Claude\x1b\\", b"\x1b]99;i=1:p=body:e=1;", b"RG9uZQ==\x1b\\"]),
            [note(Some("Claude"), "Done")]
        );
        assert_eq!(sniff(&[b"\x1b]99;;Hello\x1b\\"]), [note(None, "Hello")]);
    }

    #[test]
    fn program_statuses() {
        assert_eq!(sniff(&[b"\x1b]7501;?\x1b\\", b"\x1b]7501;?\x07"]), [Sniffed::StatusQuery, Sniffed::StatusQuery]);
        assert_eq!(sniff(&[b"text\x1bc", b"\x1b[c"]), [Sniffed::Reset]);
        let msg = base64::encode("approve Write: /tmp/note.txt".as_bytes());
        // Its other keys (`kind`, `progress`, `title`) are not read.
        let blocked = format!("\x1b]7501;state=blocked:app=claude-code:kind=permission:msg={msg}\x07");
        let working = b"\x1b]7501;state=working:app=claude-code:id=7:progress=140:title=Q2xhdWRl\x1b\\";
        let status = |state| Status { state, message: None, id: None, app: None };
        let claude = |state| Status { app: Some("claude-code".into()), ..status(state) };
        assert_eq!(
            sniff(&[blocked.as_bytes(), working, b"\x1b]7501;state=sleeping\x07\x1b]7501;state=clear\x07"]),
            [
                Sniffed::Status(Status {
                    message: Some("approve Write: /tmp/note.txt".into()),
                    ..claude(State::Blocked)
                }),
                Sniffed::Status(Status { id: Some("7".into()), ..claude(State::Working) }),
                Sniffed::Status(status(State::Clear)),
            ]
        );
        // As Claude Code 2.1.296 writes them: a sub-agent's end (no `app`), then its own state; and what
        // `recruit _member` writes once its Claude stopped.
        assert_eq!(
            sniff(&[
                b"\x1b]7501;state=clear:id=a1b2\x1b\\\x1b]7501;state=working:app=claude-code\x1b\\",
                b"\x1b]7501;state=clear:app=recruit\x07"
            ]),
            [
                Sniffed::Status(Status { id: Some("a1b2".into()), ..status(State::Clear) }),
                Sniffed::Status(claude(State::Working)),
                Sniffed::Status(Status { app: Some("recruit".into()), ..status(State::Clear) }),
            ]
        );
        // A turn that failed, its failure told; then the next turn.
        let failed = format!("\x1b]7501;state=error:app=claude-code:msg={}\x1b\\", base64::encode(b"API Error: 529"));
        assert_eq!(
            sniff(&[failed.as_bytes(), b"\x1b]7501;state=working:app=claude-code\x1b\\"]),
            [
                Sniffed::Status(Status { message: Some("API Error: 529".into()), ..claude(State::Error) }),
                Sniffed::Status(claude(State::Working)),
            ]
        );
    }

    /// The ESC that ends a string (an OSC, a DCS, an APC…) and is not followed by `\` starts the next sequence: a reset
    /// right after it is found all the same, wherever the reads are cut.
    #[test]
    fn a_reset_right_after_a_string_is_found() {
        let note = Sniffed::Notify { title: None, body: "x".into() };
        let strings: [(&[u8], &[Sniffed]); 5] = [
            (b"\x1b]9;x", &[note]),
            (b"\x1b]52;c;AAAA", &[]),
            (b"\x1b]7501;?", &[Sniffed::StatusQuery]),
            (b"\x1bPq", &[]),
            (b"\x1b_x", &[]),
        ];
        for (string, before) in strings {
            let bytes = [string, b"\x1bc"].concat();
            let expected: Vec<Sniffed> = before.iter().cloned().chain([Sniffed::Reset]).collect();
            assert_eq!(sniff(&[&bytes]), expected, "{bytes:?}");
            // Cut after the ESC that ends the string, and before it.
            for cut in [string.len(), string.len() + 1] {
                assert_eq!(sniff(&[&bytes[..cut], &bytes[cut..]]), expected, "{bytes:?} cut at {cut}");
            }
        }
        // Not a reset: the string is ended by `ESC \`, and the `c` after it is text.
        assert_eq!(sniff(&[b"\x1bPq\x1b\\c", b"\x1b]9;x\x1b\\c"]), [Sniffed::Notify { title: None, body: "x".into() }]);
    }

    #[test]
    fn other_sequences_are_followed_not_kept() {
        let mut sniffer = Sniffer::default();
        let big = [b"\x1b]52;c;".as_slice(), &vec![b'A'; 1 << 20], b"\x07"].concat();
        assert_eq!(sniffer.scan(&big), (big.len(), None));
        assert!(sniffer.kept.capacity() < 64);
        // A string's content is not read as sequences (BEL does not end it); an ESC ends it, as in vte.
        assert_eq!(sniff(&[b"\x1bP]9;no\x07\x1b\\\x1b]2;title\x07\x1b_x\x1b[?2031h"]), [Sniffed::ThemeReports]);
        // An OSC ended by another sequence.
        assert_eq!(
            sniff(&[b"\x1b]9;hi\x1b[?2031h"]),
            [Sniffed::Notify { title: None, body: "hi".into() }, Sniffed::ThemeReports]
        );
    }
}
