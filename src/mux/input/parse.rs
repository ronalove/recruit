// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The real terminal's bytes read into events: legacy keys, xterm's modified keys, the kitty keyboard protocol,
//! modifyOtherKeys, SGR and X10 mouse reports, bracketed pastes, focus. What the terminal writes in answer to a
//! query (DA, DECRPM, OSC, DCS, the kitty flags, CPR) is told apart: the probe reads it, and when it comes late,
//! nothing of it reaches a pane as keys.

use std::time::Duration;

use super::{Button, Event, Key, KeyCode, KeyKind, Mods, Mouse, MouseKind};

/// What a run of bytes was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Event(Event),
    Reply(Reply),
}

/// The terminal's answer to a query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    /// `CSI ? flags u`: the kitty keyboard protocol is there.
    Kitty(u32),
    /// DECRPM, `CSI ? mode ; value $ y`.
    Mode { mode: u32, value: u32 },
    /// An OSC string, without its frame (`11;rgb:…`).
    Osc(Vec<u8>),
    /// A DCS string, without its frame (`>|name`).
    Dcs(Vec<u8>),
    /// DA1, `CSI ? … c`: the last answer of the probe.
    Primary,
    /// Anything else a terminal answers (DA2, CPR…).
    Other,
}

/// Lone ESC, or ESC and one byte, at the end of what the terminal sent: a key (Esc, Alt+[…) if nothing follows
/// this soon. Without the kitty protocol, Esc comes this late at most (tmux's `escape-time` is 10 ms too).
const ESCAPE_TIME: Duration = Duration::from_millis(10);

/// A sequence begun and not finished: the rest is on its way (a slow link), or it is dropped.
const SEQUENCE_TIME: Duration = Duration::from_millis(100);

/// An OSC or DCS answer begun before DA1's: a slow link (SSH) may cut it for longer than a sequence typed.
const STRING_TIME: Duration = Duration::from_secs(1);

/// A paste whose end does not come: ended after this long without a byte, so that what follows is typed again.
const PASTE_TIME: Duration = Duration::from_secs(5);

/// A paste is handed on in pieces of this much at most.
const PASTE_MAX: usize = if cfg!(test) { 64 } else { 32 << 20 };

const PASTE_END: &[u8] = b"\x1b[201~";

/// Bytes in, tokens out. Keeps what it cannot read yet: the start of a sequence, of a character, a paste.
#[derive(Debug, Default)]
pub(crate) struct Parser {
    buf: Vec<u8>,
    /// Inside a bracketed paste: its text so far.
    paste: Option<Vec<u8>>,
    /// DA1's answer came: the probe is over, an OSC or DCS answer is no longer expected.
    answered: bool,
    /// Dropping the rest of an answer cut too long, up to its end (BEL or ST), or for [`STRING_TIME`].
    skip: bool,
}

/// `buf` starts like an OSC or DCS answer (see [`escape`]), maybe without its first digit yet.
fn string_start(buf: &[u8]) -> bool {
    match buf {
        [0x1b, b']'] | [0x1b, b'P'] => true,
        [0x1b, b']', b, ..] => b.is_ascii_digit(),
        [0x1b, b'P', b, ..] => b.is_ascii_digit() || *b == b'>',
        _ => false,
    }
}

enum Step {
    /// Not enough bytes to say.
    More,
    /// So many bytes read, and what they were, if anything.
    Read(usize, Parsed),
}

enum Parsed {
    Token(Token),
    PasteStart,
    /// Nothing anyone needs (an unknown sequence, a stray paste end).
    Nothing,
}

impl Parser {
    pub(crate) fn feed(&mut self, bytes: &[u8], out: &mut Vec<Token>) {
        self.buf.extend_from_slice(bytes);
        let mut at = 0;
        loop {
            if self.skip {
                let rest = &self.buf[at..];
                match (rest.iter().position(|&b| b == 0x07), find(rest, b"\x1b\\")) {
                    (Some(bel), st) if st.is_none_or(|st| bel < st) => at += bel + 1,
                    (_, Some(st)) => at += st + 2,
                    // All of it, but an ESC that may start the ST.
                    _ => {
                        at = self.buf.len() - usize::from(rest.ends_with(b"\x1b"));
                        break;
                    }
                }
                self.skip = false;
                continue;
            }
            if let Some(paste) = &mut self.paste {
                let rest = &self.buf[at..];
                match find(rest, PASTE_END) {
                    Some(end) => {
                        paste.extend_from_slice(&rest[..end]);
                        let text = String::from_utf8_lossy(paste).into_owned();
                        out.push(Token::Event(Event::Paste(text)));
                        self.paste = None;
                        at += end + PASTE_END.len();
                    }
                    None => {
                        // Keeps what may be the start of the end.
                        let keep = (1..PASTE_END.len().min(rest.len() + 1))
                            .rev()
                            .find(|&n| rest.ends_with(&PASTE_END[..n]))
                            .unwrap_or(0);
                        paste.extend_from_slice(&rest[..rest.len() - keep]);
                        at = self.buf.len() - keep;
                        if paste.len() >= PASTE_MAX {
                            // Handed on cut at a character, the rest kept for the next piece. Each piece reaches the
                            // pane as a paste of its own, framed: the program sees several pastes, which at this size
                            // is better than holding them all.
                            let cut = (0..paste.len()).rev().find(|&i| paste[i] & 0xc0 != 0x80).unwrap_or(0);
                            let rest = paste.split_off(if cut == 0 { paste.len() } else { cut });
                            let text = String::from_utf8_lossy(paste).into_owned();
                            out.push(Token::Event(Event::Paste(text)));
                            *paste = rest;
                        }
                        break;
                    }
                }
                continue;
            }
            if at >= self.buf.len() {
                break;
            }
            match step(&self.buf[at..]) {
                Step::More => break,
                Step::Read(n, parsed) => {
                    at += n;
                    match parsed {
                        Parsed::Token(token) => {
                            self.answered |= token == Token::Reply(Reply::Primary);
                            out.push(token);
                        }
                        Parsed::PasteStart => self.paste = Some(Vec::new()),
                        Parsed::Nothing => {}
                    }
                }
            }
        }
        self.buf.drain(..at);
    }

    /// How long to wait for the rest of what is pending before [`Parser::flush`]; `None`: nothing to wait for.
    pub(crate) fn pending(&self) -> Option<Duration> {
        match self.buf.as_slice() {
            _ if self.paste.is_some() => Some(PASTE_TIME),
            _ if self.skip => Some(STRING_TIME),
            [] => None,
            buf if !self.answered && string_start(buf) => Some(STRING_TIME),
            [0x1b] | [0x1b, _] => Some(ESCAPE_TIME),
            _ => Some(SEQUENCE_TIME),
        }
    }

    /// Nothing more came: a lone ESC is Esc, ESC and a key is Alt and that key, the rest is dropped. A paste ends
    /// with what came. An answer cut before DA1's is dropped, and the rest of it when it comes; after, it was Alt+]
    /// or Alt+P and keys typed fast.
    pub(crate) fn flush(&mut self, out: &mut Vec<Token>) {
        if let Some(paste) = self.paste.take() {
            if !paste.is_empty() {
                out.push(Token::Event(Event::Paste(String::from_utf8_lossy(&paste).into_owned())));
            }
            self.buf.clear();
            return;
        }
        if self.skip {
            self.skip = false;
            self.buf.clear();
            return;
        }
        if string_start(&self.buf) && (self.buf.len() > 2 || !self.answered) {
            let rest = self.buf.split_off(2);
            let alt = Key::new(KeyCode::Char(self.buf[1] as char), Mods::ALT);
            self.buf.clear();
            if self.answered {
                out.push(Token::Event(Event::Key(alt)));
                self.feed(&rest, out);
            } else {
                self.skip = true;
            }
            return;
        }
        let key = match self.buf.as_slice() {
            [0x1b] => Some(Key::new(KeyCode::Esc, Mods::NONE)),
            [0x1b, 0x1b] => Some(Key::new(KeyCode::Esc, Mods::ALT)),
            &[0x1b, b] if (0x20..0x7f).contains(&b) => Some(Key::new(KeyCode::Char(b as char), Mods::ALT)),
            _ => None,
        };
        out.extend(key.map(|key| Token::Event(Event::Key(key))));
        self.buf.clear();
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn key(code: KeyCode, mods: Mods) -> Parsed {
    Parsed::Token(Token::Event(Event::Key(Key::new(code, mods))))
}

fn reply(reply: Reply) -> Parsed {
    Parsed::Token(Token::Reply(reply))
}

/// Reads one thing at the start of `buf` (not empty).
fn step(buf: &[u8]) -> Step {
    let b = buf[0];
    match b {
        0x1b => escape(buf),
        b'\r' => Step::Read(1, key(KeyCode::Enter, Mods::NONE)),
        b'\t' => Step::Read(1, key(KeyCode::Tab, Mods::NONE)),
        0x7f => Step::Read(1, key(KeyCode::Backspace, Mods::NONE)),
        0 => Step::Read(1, key(KeyCode::Char(' '), Mods::CTRL)),
        // Ctrl+H and Ctrl+J included: 0x08 and 0x0a go back to the pane as they came.
        0x01..=0x1a => Step::Read(1, key(KeyCode::Char((b'a' + b - 1) as char), Mods::CTRL)),
        0x1c..=0x1f => Step::Read(1, key(KeyCode::Char(b"\\]^_"[(b - 0x1c) as usize] as char), Mods::CTRL)),
        0x20..=0x7e => Step::Read(1, key(KeyCode::Char(b as char), Mods::NONE)),
        _ => utf8(buf),
    }
}

fn utf8(buf: &[u8]) -> Step {
    let len = match buf[0] {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Step::Read(1, Parsed::Nothing),
    };
    if buf.len() < len {
        // A character cut in two: only continuation bytes may follow.
        return if buf[1..].iter().all(|b| b & 0xc0 == 0x80) { Step::More } else { Step::Read(1, Parsed::Nothing) };
    }
    match std::str::from_utf8(&buf[..len]).ok().and_then(|s| s.chars().next()) {
        Some(c) => Step::Read(len, key(KeyCode::Char(c), Mods::NONE)),
        None => Step::Read(1, Parsed::Nothing),
    }
}

fn escape(buf: &[u8]) -> Step {
    let Some(&next) = buf.get(1) else { return Step::More };
    match next {
        b'[' => csi(buf),
        b'O' => match buf.get(2) {
            None => Step::More,
            Some(&b) => match ss3(b) {
                Some(code) => Step::Read(3, key(code, Mods::NONE)),
                None => Step::Read(2, key(KeyCode::Char('O'), Mods::ALT)),
            },
        },
        // An OSC answer starts with its number, a DCS one with `>|` (XTVERSION) or a digit (DECRQSS); otherwise
        // these are Alt+] and Alt+P.
        b']' | b'P' => match buf.get(2) {
            None => Step::More,
            Some(&b) if b.is_ascii_digit() || (next == b'P' && b == b'>') => string(buf, next),
            Some(_) => Step::Read(2, key(KeyCode::Char(next as char), Mods::ALT)),
        },
        _ => match step(&buf[1..]) {
            Step::More => Step::More,
            Step::Read(n, Parsed::Token(Token::Event(Event::Key(mut key)))) => {
                key.mods = key.mods | Mods::ALT;
                Step::Read(n + 1, Parsed::Token(Token::Event(Event::Key(key))))
            }
            Step::Read(n, other) => Step::Read(n + 1, other),
        },
    }
}

/// An OSC or DCS string, up to BEL or ST.
fn string(buf: &[u8], kind: u8) -> Step {
    let body = &buf[2..];
    let (end, len) = match (body.iter().position(|&b| b == 0x07), find(body, b"\x1b\\")) {
        (Some(bel), Some(st)) if st < bel => (st, 2),
        (Some(bel), _) => (bel, 1),
        (None, Some(st)) => (st, 2),
        (None, None) => return Step::More,
    };
    let content = body[..end].to_vec();
    Step::Read(2 + end + len, reply(if kind == b']' { Reply::Osc(content) } else { Reply::Dcs(content) }))
}

fn ss3(b: u8) -> Option<KeyCode> {
    Some(match b {
        b'A' => KeyCode::Up,
        b'B' => KeyCode::Down,
        b'C' => KeyCode::Right,
        b'D' => KeyCode::Left,
        b'H' => KeyCode::Home,
        b'F' => KeyCode::End,
        b'P'..=b'S' => KeyCode::F(b - b'P' + 1),
        // The keypad in application mode.
        b'M' => KeyCode::Enter,
        b'X' => KeyCode::Char('='),
        b'j'..=b'y' => KeyCode::Char(b"*+,-./0123456789"[(b - b'j') as usize] as char),
        _ => return None,
    })
}

/// The parameters of a CSI sequence: `;` between them, `:` between the parts of one; a missing number is `None`.
fn params(body: &[u8]) -> Vec<Vec<Option<u32>>> {
    if body.is_empty() {
        return Vec::new();
    }
    body.split(|&b| b == b';')
        .map(|param| {
            param.split(|&b| b == b':').map(|n| std::str::from_utf8(n).ok().and_then(|n| n.parse().ok())).collect()
        })
        .collect()
}

fn get(params: &[Vec<Option<u32>>], i: usize, j: usize) -> Option<u32> {
    params.get(i).and_then(|param| param.get(j).copied().flatten())
}

/// Modifiers and event type from the `mods:type` parameter; the lock keys (Caps Lock, Num Lock) left out.
fn mods_kind(params: &[Vec<Option<u32>>], i: usize) -> (Mods, KeyKind) {
    let mods = get(params, i, 0).unwrap_or(1).saturating_sub(1) as u8 & 0x3f;
    let kind = match get(params, i, 1) {
        Some(2) => KeyKind::Repeat,
        Some(3) => KeyKind::Release,
        _ => KeyKind::Press,
    };
    (Mods(mods), kind)
}

fn csi(buf: &[u8]) -> Step {
    let Some(&first) = buf.get(2) else { return Step::More };
    if first == b'M' {
        // X10 mouse: three bytes, each 32 more than its value.
        if buf.len() < 6 {
            return Step::More;
        }
        let (cb, x, y) = (buf[3].wrapping_sub(32), buf[4].saturating_sub(33), buf[5].saturating_sub(33));
        return Step::Read(6, mouse(cb as u32, x as u16, y as u16, false));
    }
    let Some(end) = buf[2..].iter().position(|b| !(0x20..=0x3f).contains(b)).map(|i| i + 2) else {
        return Step::More;
    };
    let fin = buf[end];
    if !(0x40..=0x7e).contains(&fin) {
        // Broken: dropped up to the odd byte, which is read again on its own.
        return Step::Read(end, Parsed::Nothing);
    }
    let body = &buf[2..end];
    Step::Read(end + 1, csi_body(body, fin))
}

fn csi_body(body: &[u8], fin: u8) -> Parsed {
    match body.first() {
        Some(b'<') => {
            let params = params(&body[1..]);
            if !matches!(fin, b'M' | b'm') {
                return Parsed::Nothing;
            }
            let (Some(cb), Some(x), Some(y)) = (get(&params, 0, 0), get(&params, 1, 0), get(&params, 2, 0)) else {
                return Parsed::Nothing;
            };
            let at = |n: u32| n.saturating_sub(1).min(u16::MAX as u32) as u16;
            return mouse(cb, at(x), at(y), fin == b'm');
        }
        Some(b'?') => {
            let digits: Vec<u8> = body[1..].iter().copied().filter(|&b| b != b'$').collect();
            let params = params(&digits);
            return reply(match fin {
                b'u' => Reply::Kitty(get(&params, 0, 0).unwrap_or(0)),
                b'c' => Reply::Primary,
                b'y' if body.ends_with(b"$") => match (get(&params, 0, 0), get(&params, 1, 0)) {
                    (Some(mode), Some(value)) => Reply::Mode { mode, value },
                    _ => Reply::Other,
                },
                _ => Reply::Other,
            });
        }
        Some(b'>' | b'=') => return reply(Reply::Other),
        _ => {}
    }
    if body.iter().any(|b| !(b'0'..=b';').contains(b)) {
        // Intermediate bytes: nothing a key sends.
        return Parsed::Nothing;
    }
    let params = params(body);
    match fin {
        b'I' if body.is_empty() => Parsed::Token(Token::Event(Event::Focus(true))),
        b'O' if body.is_empty() => Parsed::Token(Token::Event(Event::Focus(false))),
        b'u' => {
            let Some(code) = get(&params, 0, 0).and_then(kitty_code) else { return Parsed::Nothing };
            let (mods, kind) = mods_kind(&params, 1);
            Parsed::Token(Token::Event(Event::Key(Key { code, mods, kind })))
        }
        b'~' => {
            let n = get(&params, 0, 0).unwrap_or(0);
            if n == 200 {
                return Parsed::PasteStart;
            }
            if n == 27 {
                // modifyOtherKeys: `CSI 27 ; mods ; code ~`.
                let Some(code) = get(&params, 2, 0).and_then(kitty_code) else { return Parsed::Nothing };
                let (mods, kind) = mods_kind(&params, 1);
                return Parsed::Token(Token::Event(Event::Key(Key { code, mods, kind })));
            }
            let code = match n {
                1 | 7 => KeyCode::Home,
                2 => KeyCode::Insert,
                3 => KeyCode::Delete,
                4 | 8 => KeyCode::End,
                5 => KeyCode::PageUp,
                6 => KeyCode::PageDown,
                11..=15 => KeyCode::F((n - 10) as u8),
                17..=21 => KeyCode::F((n - 11) as u8),
                23..=26 => KeyCode::F((n - 12) as u8),
                28 | 29 => KeyCode::F((n - 13) as u8),
                31..=34 => KeyCode::F((n - 14) as u8),
                _ => return Parsed::Nothing,
            };
            let (mods, kind) = mods_kind(&params, 1);
            Parsed::Token(Token::Event(Event::Key(Key { code, mods, kind })))
        }
        // A cursor position report, which nobody asked for any more; `CSI 1 ; mods R` is F3.
        b'R' if get(&params, 0, 0).is_some_and(|row| row != 1) => reply(Reply::Other),
        _ => {
            let code = match fin {
                b'A' => KeyCode::Up,
                b'B' => KeyCode::Down,
                b'C' => KeyCode::Right,
                b'D' => KeyCode::Left,
                b'H' => KeyCode::Home,
                b'F' => KeyCode::End,
                b'P'..=b'S' => KeyCode::F(fin - b'P' + 1),
                b'Z' => return key(KeyCode::Tab, Mods::SHIFT),
                _ => return Parsed::Nothing,
            };
            let (mods, kind) = mods_kind(&params, 1);
            Parsed::Token(Token::Event(Event::Key(Key { code, mods, kind })))
        }
    }
}

/// A key of the kitty protocol, by its number: a character, or one of the functional keys in the private use
/// area (the keypad's own keys read as the others). The keys we do not carry (media, the modifiers on their own,
/// the lock keys) are `None`.
fn kitty_code(n: u32) -> Option<KeyCode> {
    Some(match n {
        13 | 57414 => KeyCode::Enter,
        9 => KeyCode::Tab,
        8 | 127 => KeyCode::Backspace,
        27 => KeyCode::Esc,
        57376..=57398 => KeyCode::F((n - 57376 + 13) as u8),
        57399..=57416 => KeyCode::Char(b"0123456789./*-+\0=,"[(n - 57399) as usize] as char),
        57417 => KeyCode::Left,
        57418 => KeyCode::Right,
        57419 => KeyCode::Up,
        57420 => KeyCode::Down,
        57421 => KeyCode::PageUp,
        57422 => KeyCode::PageDown,
        57423 => KeyCode::Home,
        57424 => KeyCode::End,
        57425 => KeyCode::Insert,
        57426 => KeyCode::Delete,
        0xe000..=0xf8ff => return None,
        _ => KeyCode::Char(char::from_u32(n).filter(|c| !c.is_control())?),
    })
}

/// A mouse report: `cb` as xterm writes it (button in the low bits, then shift 4, alt 8, ctrl 16, move 32,
/// wheel 64); `release` for SGR's `m`.
fn mouse(cb: u32, col: u16, row: u16, release: bool) -> Parsed {
    let mut mods = Mods::NONE;
    for (bit, m) in [(4, Mods::SHIFT), (8, Mods::ALT), (16, Mods::CTRL)] {
        if cb & bit != 0 {
            mods = mods | m;
        }
    }
    let button = match cb & 3 {
        0 => Some(Button::Left),
        1 => Some(Button::Middle),
        2 => Some(Button::Right),
        _ => None,
    };
    let kind = if cb & 128 != 0 {
        // Buttons 8 to 11.
        return Parsed::Nothing;
    } else if cb & 64 != 0 {
        [MouseKind::ScrollUp, MouseKind::ScrollDown, MouseKind::ScrollLeft, MouseKind::ScrollRight][(cb & 3) as usize]
    } else if cb & 32 != 0 {
        button.map_or(MouseKind::Moved, MouseKind::Drag)
    } else if release {
        MouseKind::Up(button.unwrap_or(Button::Left))
    } else {
        // X10 says which button went up only as "a button".
        button.map_or(MouseKind::Up(Button::Left), MouseKind::Down)
    };
    Parsed::Token(Token::Event(Event::Mouse(Mouse { kind, col, row, mods })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(bytes: &[u8]) -> Vec<Token> {
        let mut parser = Parser::default();
        let mut out = Vec::new();
        parser.feed(bytes, &mut out);
        parser.flush(&mut out);
        out
    }

    fn keys(bytes: &[u8]) -> Vec<Key> {
        tokens(bytes)
            .into_iter()
            .map(|token| match token {
                Token::Event(Event::Key(key)) => key,
                other => panic!("{bytes:?}: {other:?}"),
            })
            .collect()
    }

    fn one(bytes: &[u8]) -> Key {
        let keys = keys(bytes);
        assert_eq!(keys.len(), 1, "{bytes:?}: {keys:?}");
        keys[0]
    }

    use KeyCode::*;
    const S: Mods = Mods::SHIFT;
    const A: Mods = Mods::ALT;
    const C: Mods = Mods::CTRL;
    const N: Mods = Mods::NONE;

    #[test]
    fn legacy_keys() {
        // What xterm sends without any mode (ctlseqs, "PC-Style Function Keys").
        let table: &[(&[u8], KeyCode, Mods)] = &[
            (b"a", Char('a'), N),
            (b"A", Char('A'), N),
            ("é".as_bytes(), Char('é'), N),
            ("€".as_bytes(), Char('€'), N),
            ("😀".as_bytes(), Char('😀'), N),
            (b"\r", Enter, N),
            (b"\t", Tab, N),
            (b"\x7f", Backspace, N),
            (b"\x08", Char('h'), C),
            (b"\n", Char('j'), C),
            (b"\x02", Char('b'), C),
            (b"\x12", Char('r'), C),
            (b"\x0f", Char('o'), C),
            (b"\x00", Char(' '), C),
            (b"\x1c", Char('\\'), C),
            (b"\x1f", Char('_'), C),
            (b"\x1ba", Char('a'), A),
            (b"\x1b\r", Enter, A),
            (b"\x1b\x02", Char('b'), C | A),
            ("\x1bé".as_bytes(), Char('é'), A),
            (b"\x1b[A", Up, N),
            (b"\x1bOA", Up, N),
            (b"\x1b[1;5D", Left, C),
            (b"\x1b[1;2C", Right, S),
            (b"\x1b[1;3B", Down, A),
            (b"\x1b[H", Home, N),
            (b"\x1bOF", End, N),
            (b"\x1b[1~", Home, N),
            (b"\x1b[4~", End, N),
            (b"\x1b[2~", Insert, N),
            (b"\x1b[3;5~", Delete, C),
            (b"\x1b[5~", PageUp, N),
            (b"\x1b[6;2~", PageDown, S),
            (b"\x1bOP", F(1), N),
            (b"\x1b[1;2Q", F(2), S),
            (b"\x1b[1;5R", F(3), C),
            (b"\x1b[15~", F(5), N),
            (b"\x1b[24;5~", F(12), C),
            (b"\x1b[Z", Tab, S),
            (b"\x1bOM", Enter, N),
            (b"\x1bOp", Char('0'), N),
            // modifyOtherKeys 2.
            (b"\x1b[27;2;13~", Enter, S),
            (b"\x1b[27;5;105~", Char('i'), C),
        ];
        for &(bytes, code, mods) in table {
            assert_eq!(one(bytes), Key::new(code, mods), "{bytes:?}");
        }
    }

    #[test]
    fn kitty_keys() {
        // https://sw.kovidgoyal.net/kitty/keyboard-protocol/: `CSI code[:shifted[:base]] ; mods[:type] ; text u`.
        let table: &[(&[u8], KeyCode, Mods, KeyKind)] = &[
            (b"\x1b[27u", Esc, N, KeyKind::Press),
            (b"\x1b[13;2u", Enter, S, KeyKind::Press),
            (b"\x1b[9;2u", Tab, S, KeyKind::Press),
            (b"\x1b[127;5u", Backspace, C, KeyKind::Press),
            (b"\x1b[98;5u", Char('b'), C, KeyKind::Press),
            (b"\x1b[97;6u", Char('a'), S | C, KeyKind::Press),
            (b"\x1b[97:65;6u", Char('a'), S | C, KeyKind::Press),
            (b"\x1b[93;3u", Char(']'), A, KeyKind::Press),
            (b"\x1b[97;1:3u", Char('a'), N, KeyKind::Release),
            (b"\x1b[97;1:2u", Char('a'), N, KeyKind::Repeat),
            (b"\x1b[97;1;97u", Char('a'), N, KeyKind::Press),
            // Caps Lock (64) and Num Lock (128) are not modifiers.
            (b"\x1b[98;69u", Char('b'), C, KeyKind::Press),
            (b"\x1b[57414u", Enter, N, KeyKind::Press),
            (b"\x1b[57399u", Char('0'), N, KeyKind::Press),
            (b"\x1b[57376u", F(13), N, KeyKind::Press),
            (b"\x1b[13~", F(3), N, KeyKind::Press),
            (b"\x1b[1;5:3A", Up, C, KeyKind::Release),
        ];
        for &(bytes, code, mods, kind) in table {
            assert_eq!(one(bytes), Key { code, mods, kind }, "{bytes:?}");
        }
        // Media keys and modifiers on their own are not carried.
        assert_eq!(tokens(b"\x1b[57428u\x1b[57441;2u"), []);
    }

    #[test]
    fn escape_alone_is_esc_at_once_or_after_a_short_wait() {
        let mut parser = Parser::default();
        let mut out = Vec::new();
        parser.feed(b"\x1b", &mut out);
        assert!(out.is_empty());
        assert_eq!(parser.pending(), Some(ESCAPE_TIME));
        parser.flush(&mut out);
        assert_eq!(out, [Token::Event(Event::Key(Key::new(Esc, N)))]);
        // ESC then the rest of a sequence, read apart.
        out.clear();
        parser.feed(b"\x1b", &mut out);
        parser.feed(b"[A", &mut out);
        assert_eq!(out, [Token::Event(Event::Key(Key::new(Up, N)))]);
        assert_eq!(parser.pending(), None);
        // ESC and a key, nothing after: Alt and that key.
        assert_eq!(keys(b"\x1b["), [Key::new(Char('['), A)]);
        assert_eq!(keys(b"\x1bO"), [Key::new(Char('O'), A)]);
        // Before DA1's answer, `ESC ]` alone may be an answer cut: see answers_cut_by_a_slow_link.
        assert_eq!(keys(b"\x1b]"), []);
        assert_eq!(keys(b"\x1b\x1b"), [Key::new(Esc, A)]);
        assert_eq!(keys(b"\x1b]x"), [Key::new(Char(']'), A), Key::new(Char('x'), N)]);
        assert_eq!(keys(b"\x1bPx"), [Key::new(Char('P'), A), Key::new(Char('x'), N)]);
    }

    #[test]
    fn sequences_and_characters_cut_in_two() {
        let mut parser = Parser::default();
        let mut out = Vec::new();
        for chunk in [&b"\x1b[1"[..], b";5", b"D\xc3", b"\xa9\x1b[<0;1", b"0;5M"] {
            parser.feed(chunk, &mut out);
        }
        let mouse = Mouse { kind: MouseKind::Down(Button::Left), col: 9, row: 4, mods: N };
        assert_eq!(
            out,
            [
                Token::Event(Event::Key(Key::new(Left, C))),
                Token::Event(Event::Key(Key::new(Char('é'), N))),
                Token::Event(Event::Mouse(mouse)),
            ]
        );
        // Begun and never finished: dropped.
        parser.feed(b"\x1b[1;", &mut out);
        assert_eq!(parser.pending(), Some(SEQUENCE_TIME));
        out.clear();
        parser.flush(&mut out);
        assert!(out.is_empty());
        parser.feed(b"x", &mut out);
        assert_eq!(out, [Token::Event(Event::Key(Key::new(Char('x'), N)))]);
    }

    #[test]
    fn answers_cut_by_a_slow_link() {
        let key = |c| Token::Event(Event::Key(Key::new(Char(c), N)));
        // Before DA1's answer: an OSC answer may wait a second; cut longer, it is dropped, and so is its rest.
        let mut parser = Parser::default();
        let mut out = Vec::new();
        parser.feed(b"\x1b]11;rgb:00", &mut out);
        assert_eq!(parser.pending(), Some(STRING_TIME));
        parser.flush(&mut out);
        assert!(out.is_empty());
        assert_eq!(parser.pending(), Some(STRING_TIME));
        parser.feed(b"00/0000/0000\x1b", &mut out);
        parser.feed(b"\\b", &mut out);
        assert_eq!(out, [key('b')]);
        // Cut right after its introducer, too.
        out.clear();
        parser.feed(b"\x1bP", &mut out);
        assert_eq!(parser.pending(), Some(STRING_TIME));
        parser.flush(&mut out);
        parser.feed(b">|tmux 3.8\x07c", &mut out);
        assert_eq!(out, [key('c')]);
        // A rest that never comes: the next quiet second ends the dropping.
        out.clear();
        parser.feed(b"\x1b]10;", &mut out);
        parser.flush(&mut out);
        parser.flush(&mut out);
        parser.feed(b"d", &mut out);
        assert_eq!(out, [key('d')]);
        // After DA1's answer, no answer is expected: Alt+] and keys typed fast.
        out.clear();
        parser.feed(b"\x1b[?62c\x1b]1", &mut out);
        assert_eq!(parser.pending(), Some(SEQUENCE_TIME));
        parser.flush(&mut out);
        assert_eq!(out, [Token::Reply(Reply::Primary), Token::Event(Event::Key(Key::new(Char(']'), A))), key('1')]);
        out.clear();
        parser.feed(b"\x1b]", &mut out);
        assert_eq!(parser.pending(), Some(ESCAPE_TIME));
        parser.flush(&mut out);
        assert_eq!(out, [Token::Event(Event::Key(Key::new(Char(']'), A)))]);
    }

    #[test]
    fn pastes_without_an_end_and_long_ones() {
        // No end: ended after a quiet while, with what came; what follows is typed again.
        let mut parser = Parser::default();
        let mut out = Vec::new();
        parser.feed(b"\x1b[200~abc\x1b[20", &mut out);
        assert_eq!(parser.pending(), Some(PASTE_TIME));
        parser.flush(&mut out);
        parser.feed(b"x", &mut out);
        assert_eq!(out, [Token::Event(Event::Paste("abc".into())), Token::Event(Event::Key(Key::new(Char('x'), N)))]);
        // Long: in pieces of PASTE_MAX at most, never cut inside a character.
        out.clear();
        let text = "é".repeat(100);
        parser.feed(b"\x1b[200~", &mut out);
        for chunk in text.as_bytes().chunks(7) {
            parser.feed(chunk, &mut out);
        }
        parser.feed(b"\x1b[201~", &mut out);
        let pieces: Vec<String> = out
            .iter()
            .map(|token| match token {
                Token::Event(Event::Paste(piece)) => piece.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(pieces.len() > 2 && pieces.iter().all(|piece| piece.len() <= PASTE_MAX + 7), "{pieces:?}");
        assert_eq!(pieces.concat(), text);
    }

    #[test]
    fn mouse_reports() {
        let m = |kind, col, row, mods| Token::Event(Event::Mouse(Mouse { kind, col, row, mods }));
        use MouseKind::*;
        let table: &[(&[u8], Token)] = &[
            (b"\x1b[<0;1;1M", m(Down(Button::Left), 0, 0, N)),
            (b"\x1b[<0;1;1m", m(Up(Button::Left), 0, 0, N)),
            (b"\x1b[<2;80;24M", m(Down(Button::Right), 79, 23, N)),
            (b"\x1b[<1;3;4M", m(Down(Button::Middle), 2, 3, N)),
            (b"\x1b[<32;5;6M", m(Drag(Button::Left), 4, 5, N)),
            (b"\x1b[<35;5;6M", m(Moved, 4, 5, N)),
            (b"\x1b[<64;1;1M", m(ScrollUp, 0, 0, N)),
            (b"\x1b[<65;1;1M", m(ScrollDown, 0, 0, N)),
            (b"\x1b[<66;1;1M", m(ScrollLeft, 0, 0, N)),
            (b"\x1b[<67;1;1M", m(ScrollRight, 0, 0, N)),
            (b"\x1b[<20;1;1M", m(Down(Button::Left), 0, 0, S | C)),
            (b"\x1b[<8;300;1M", m(Down(Button::Left), 299, 0, A)),
            (b"\x1b[M !!", m(Down(Button::Left), 0, 0, N)),
            (b"\x1b[M#!!", m(Up(Button::Left), 0, 0, N)),
            (b"\x1b[M`*+", m(ScrollUp, 9, 10, N)),
        ];
        for (bytes, token) in table {
            assert_eq!(tokens(bytes), std::slice::from_ref(token), "{bytes:?}");
        }
    }

    #[test]
    fn pastes_focus_and_replies() {
        assert_eq!(tokens(b"\x1b[I\x1b[O"), [Token::Event(Event::Focus(true)), Token::Event(Event::Focus(false))]);
        // A paste is text, whatever it holds, even in pieces; the end may come cut.
        let mut parser = Parser::default();
        let mut out = Vec::new();
        for chunk in [&b"x\x1b[200~a\x1b[A\r"[..], "é\x1b[20".as_bytes(), b"1~y"] {
            parser.feed(chunk, &mut out);
        }
        assert_eq!(
            out,
            [
                Token::Event(Event::Key(Key::new(Char('x'), N))),
                Token::Event(Event::Paste("a\x1b[A\ré".into())),
                Token::Event(Event::Key(Key::new(Char('y'), N))),
            ]
        );
        assert_eq!(parser.pending(), None);
        // The answers to the probe, and the same come late: none is a key.
        let table: &[(&[u8], Reply)] = &[
            (b"\x1b[?1u", Reply::Kitty(1)),
            (b"\x1b[?2026;2$y", Reply::Mode { mode: 2026, value: 2 }),
            (b"\x1b[?2027;0$y", Reply::Mode { mode: 2027, value: 0 }),
            (b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\", Reply::Osc(b"11;rgb:1e1e/1e1e/2e2e".to_vec())),
            (b"\x1b]10;rgb:ffff/ffff/ffff\x07", Reply::Osc(b"10;rgb:ffff/ffff/ffff".to_vec())),
            (b"\x1bP>|ghostty 1.2.0\x1b\\", Reply::Dcs(b">|ghostty 1.2.0".to_vec())),
            (b"\x1bP1$r0m\x1b\\", Reply::Dcs(b"1$r0m".to_vec())),
            (b"\x1b[?62;22;52c", Reply::Primary),
            (b"\x1b[>1;10;0c", Reply::Other),
            (b"\x1b[12;40R", Reply::Other),
        ];
        for (bytes, reply) in table {
            assert_eq!(tokens(bytes), [Token::Reply(reply.clone())], "{bytes:?}");
        }
        // An OSC answer cut by a slow link waits for its end, typed keys around it stay.
        let mut parser = Parser::default();
        let mut out = Vec::new();
        parser.feed(b"a\x1b]11;rgb:00", &mut out);
        assert_eq!(parser.pending(), Some(STRING_TIME));
        parser.feed(b"00/0000/0000\x1b\\b", &mut out);
        assert_eq!(
            out,
            [
                Token::Event(Event::Key(Key::new(Char('a'), N))),
                Token::Reply(Reply::Osc(b"11;rgb:0000/0000/0000".to_vec())),
                Token::Event(Event::Key(Key::new(Char('b'), N))),
            ]
        );
    }
}
