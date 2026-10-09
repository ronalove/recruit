// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What a program writes that an engine may let pass without a word, found in its output before the engine parses
//! it: XTVERSION, the color scheme query (`CSI ? 996 n`) and the program status query (`OSC 7501 ; ?`), which want
//! an answer; desktop notifications (OSC 9, 777, 99) and progress (OSC 9;4), which go to the real terminal and the
//! interface; the program's status (OSC 7501), for the interface.
//!
//! Only escape sequences are looked at: text is skipped with `memchr`, the sequences that matter are kept, the
//! others are followed without being stored (an OSC 52 of several megabytes costs nothing). A sequence may be cut
//! between two reads.
//!
//! Owner: dev-terminal.

use crate::mux::engine::{Progress, State, Status};

/// What was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Sniffed {
    /// `CSI > q`, `CSI > 0 q`: the terminal's name and version.
    Version,
    /// `CSI ? 996 n`: dark or light.
    ColorScheme,
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
}

/// Bytes of a CSI kept, enough for those looked for.
const CSI_KEPT: usize = 16;

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
            if let Some(found) = self.step(byte) {
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
                        (b">" | b">0", b'q') => Some(Sniffed::Version),
                        (b"?996", b'n') => Some(Sniffed::ColorScheme),
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
                self.state = Scan::Ground;
                if byte != b'\\' {
                    self.step(ESC);
                    self.step(byte);
                }
                return found;
            }
            Scan::String => {
                if byte == ESC {
                    self.state = Scan::StringEscape;
                }
            }
            Scan::StringEscape => {
                self.state = Scan::Ground;
                if byte != b'\\' {
                    self.step(ESC);
                    self.step(byte);
                }
            }
        }
        None
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
            if encoded { String::from_utf8_lossy(&base64_decode(payload)?).into_owned() } else { payload.into() };
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

/// `7501 ; ?` asks; `7501 ; key=value:…` tells. A state unknown here is left alone.
fn osc_7501(rest: &str) -> Option<Sniffed> {
    if rest.starts_with('?') {
        return Some(Sniffed::StatusQuery);
    }
    let mut status = Status { state: State::Clear, kind: None, progress: None, title: None, message: None, id: None };
    let mut state = None;
    let decoded = |text: &str| base64_decode(text).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    for field in rest.split(':') {
        let Some((key, value)) = field.split_once('=') else { continue };
        match key {
            "state" => {
                state = match value {
                    "idle" => Some(State::Idle),
                    "working" => Some(State::Working),
                    "blocked" => Some(State::Blocked),
                    "done" => Some(State::Done),
                    "clear" => Some(State::Clear),
                    _ => None,
                }
            }
            "kind" => status.kind = Some(value.to_string()),
            "id" => status.id = Some(value.to_string()),
            "progress" => status.progress = value.parse::<u32>().ok().map(|p| p.min(100) as u8),
            "title" => status.title = decoded(value),
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

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(super) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { BASE64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Standard base64, padded or not; `None` when it is not.
pub(super) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut bits, mut count) = (0u32, 0);
    for byte in text.trim_end_matches('=').bytes() {
        let value = BASE64.iter().position(|&b| b == byte)? as u32;
        bits = bits << 6 | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Some(out)
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
    fn queries_are_found() {
        assert_eq!(
            sniff(&[b"text\x1b[>q more \x1b[>0q\x1b[?996n\x1b[>1q\x1b[q"]),
            [Sniffed::Version, Sniffed::Version, Sniffed::ColorScheme]
        );
        // Cut anywhere.
        assert_eq!(sniff(&[b"\x1b", b"[", b">", b"0", b"q"]), [Sniffed::Version]);
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
        let msg = base64_encode("approve Write: /tmp/note.txt".as_bytes());
        let blocked = format!("\x1b]7501;state=blocked:app=claude-code:kind=permission:msg={msg}\x07");
        let working = b"\x1b]7501;state=working:app=claude-code:id=7:progress=140:title=Q2xhdWRl\x1b\\";
        let status = |state| Status { state, kind: None, progress: None, title: None, message: None, id: None };
        assert_eq!(
            sniff(&[blocked.as_bytes(), working, b"\x1b]7501;state=sleeping\x07\x1b]7501;state=clear\x07"]),
            [
                Sniffed::Status(Status {
                    kind: Some("permission".into()),
                    message: Some("approve Write: /tmp/note.txt".into()),
                    ..status(State::Blocked)
                }),
                Sniffed::Status(Status {
                    progress: Some(100),
                    title: Some("Claude".into()),
                    id: Some("7".into()),
                    ..status(State::Working)
                }),
                Sniffed::Status(status(State::Clear)),
            ]
        );
    }

    #[test]
    fn other_sequences_are_followed_not_kept() {
        let mut sniffer = Sniffer::default();
        let big = [b"\x1b]52;c;".as_slice(), &vec![b'A'; 1 << 20], b"\x07"].concat();
        assert_eq!(sniffer.scan(&big), (big.len(), None));
        assert!(sniffer.kept.capacity() < 64);
        // A string's content is not read as sequences (BEL does not end it); an ESC ends it, as in vte.
        assert_eq!(sniff(&[b"\x1bP]9;no\x07\x1b\\\x1b]2;title\x07\x1b_x\x1b[>q"]), [Sniffed::Version]);
        // An OSC ended by another sequence.
        assert_eq!(
            sniff(&[b"\x1b]9;hi\x1b[>q"]),
            [Sniffed::Notify { title: None, body: "hi".into() }, Sniffed::Version]
        );
    }

    #[test]
    fn base64_both_ways() {
        for text in ["", "f", "fo", "foo", "foob", "fooba", "foobar", "été ✓"] {
            assert_eq!(base64_decode(&base64_encode(text.as_bytes())).unwrap(), text.as_bytes());
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_decode("not base64!"), None);
    }
}
