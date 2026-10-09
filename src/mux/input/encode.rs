// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Each event written for the pane it goes to, as a terminal would for the modes that pane's program asked for:
//! xterm's keys (the cursor keys in application mode), the kitty keyboard protocol and its flags, the mouse
//! (1000, 1002, 1003; SGR or X10; the wheel as cursor keys with 1007), bracketed pastes, focus.

use super::{Button, Key, KeyCode, KeyKind, Mods, Mouse, MouseKind};
use crate::mux::engine::{Modes, MouseTracking};

/// The kitty protocol's flags (`CSI > flags u`).
const DISAMBIGUATE: u8 = 1;
const EVENT_TYPES: u8 = 2;
const ALTERNATES: u8 = 4;
const ALL_AS_ESCAPES: u8 = 8;
const TEXT: u8 = 16;

/// The bytes `key` sends to a pane in `modes`. Empty when the pane does not take that key (a release it did not ask
/// for). Shift+Enter is a new line in Claude Code whatever the pane's modes: `CSI 13;2u` with the kitty protocol,
/// LF without it (Claude Code takes Shift+Enter in any form only once the terminal has said it speaks the kitty
/// protocol, LF always).
pub(crate) fn key(key: &Key, modes: &Modes) -> Vec<u8> {
    if modes.kitty & (DISAMBIGUATE | ALL_AS_ESCAPES) != 0 {
        kitty(key, modes)
    } else if key.kind == KeyKind::Release {
        Vec::new()
    } else {
        legacy(key, modes)
    }
}

/// xterm's modifier parameter: 1, plus shift 1, alt 2, ctrl 4, meta (super) 8.
fn xterm_mods(mods: Mods) -> u8 {
    1 + (mods.0 & 0x0f)
}

/// The control character Ctrl and `c` give, as xterm sends it.
fn control(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' | 'A'..='Z' => c as u8 & 0x1f,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' | '~' => 0x1e,
        '_' | '7' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// The final letter of a cursor key, or of Home and End.
fn cursor_letter(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Up => b'A',
        KeyCode::Down => b'B',
        KeyCode::Right => b'C',
        KeyCode::Left => b'D',
        KeyCode::Home => b'H',
        KeyCode::End => b'F',
        _ => return None,
    })
}

/// The number of a key sent as `CSI n ~`.
fn tilde_number(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Insert => 2,
        KeyCode::Delete => 3,
        KeyCode::PageUp => 5,
        KeyCode::PageDown => 6,
        KeyCode::F(n @ 5..=20) => [15, 17, 18, 19, 20, 21, 23, 24, 25, 26, 28, 29, 31, 32, 33, 34][n as usize - 5],
        _ => return None,
    })
}

fn legacy(key: &Key, modes: &Modes) -> Vec<u8> {
    let mods = key.mods;
    let (shift, alt, ctrl) = (mods.contains(Mods::SHIFT), mods.contains(Mods::ALT), mods.contains(Mods::CTRL));
    let mut out = Vec::new();
    if alt && matches!(key.code, KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::Esc) {
        out.push(0x1b);
    }
    let m = xterm_mods(mods);
    match key.code {
        KeyCode::Char(c) => match (ctrl, control(c)) {
            (true, Some(byte)) => out.push(byte),
            _ => {
                let c = if shift { upper(c).unwrap_or(c) } else { c };
                out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            }
        },
        KeyCode::Enter if shift && !ctrl && !alt => out.push(b'\n'),
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab if shift => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::Backspace => out.push(if ctrl { 0x08 } else { 0x7f }),
        KeyCode::Esc => out.push(0x1b),
        code @ (KeyCode::F(1..=4) | KeyCode::Up | KeyCode::Down | KeyCode::Right | KeyCode::Left)
        | code @ (KeyCode::Home | KeyCode::End) => {
            let (letter, ss3) = match code {
                KeyCode::F(n) => (b'P' + n - 1, true),
                _ => (cursor_letter(code).unwrap_or(b'A'), modes.app_cursor),
            };
            if m > 1 {
                out.extend_from_slice(format!("\x1b[1;{m}").as_bytes());
            } else {
                out.extend_from_slice(if ss3 { b"\x1bO" } else { b"\x1b[" });
            }
            out.push(letter);
        }
        code => match tilde_number(code) {
            Some(n) if m > 1 => out.extend_from_slice(format!("\x1b[{n};{m}~").as_bytes()),
            Some(n) => out.extend_from_slice(format!("\x1b[{n}~").as_bytes()),
            None => {}
        },
    }
    out
}

/// The upper case of a letter that has a single one.
fn upper(c: char) -> Option<char> {
    let mut up = c.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) if u != c => Some(u),
        _ => None,
    }
}

/// The lower case of a letter that has a single one.
fn lower(c: char) -> Option<char> {
    let mut low = c.to_lowercase();
    match (low.next(), low.next()) {
        (Some(l), None) if l != c => Some(l),
        _ => None,
    }
}

/// `;mods[:type]`, or nothing when both are the default.
fn mods_param(mods: u8, kind: u8) -> String {
    match (mods, kind) {
        (0, 1) => String::new(),
        (m, 1) => format!(";{}", m + 1),
        (m, k) => format!(";{}:{k}", m + 1),
    }
}

fn kitty(key: &Key, modes: &Modes) -> Vec<u8> {
    let flags = modes.kitty;
    let all = flags & ALL_AS_ESCAPES != 0;
    let kind = match key.kind {
        _ if flags & EVENT_TYPES == 0 => 1,
        KeyKind::Press => 1,
        KeyKind::Repeat => 2,
        KeyKind::Release => 3,
    };
    if key.kind == KeyKind::Release && flags & EVENT_TYPES == 0 {
        return Vec::new();
    }
    let mut mods = key.mods.0;
    let csi = |body: String| format!("\x1b[{body}").into_bytes();
    match key.code {
        KeyCode::Char(c) => {
            // A text key: its unshifted key, and the shifted one when shift gave it.
            let (base, shifted) = match lower(c) {
                Some(l) if upper(l) == Some(c) => {
                    mods |= Mods::SHIFT.0;
                    (l, Some(c))
                }
                _ => (c, if mods & Mods::SHIFT.0 != 0 { upper(c) } else { None }),
            };
            let text_only = mods & !Mods::SHIFT.0 == 0;
            if !all && text_only {
                // Text stays text; its release is not reported (spec: "Event types").
                if kind == 3 {
                    return Vec::new();
                }
                let text = if mods & Mods::SHIFT.0 != 0 { shifted.unwrap_or(c) } else { c };
                return text.to_string().into_bytes();
            }
            let mut body = (base as u32).to_string();
            if flags & ALTERNATES != 0
                && let Some(shifted) = shifted
            {
                body += &format!(":{}", shifted as u32);
            }
            let mut params = mods_param(mods, kind);
            if flags & TEXT != 0 && text_only && kind != 3 {
                let text = shifted.filter(|_| mods & Mods::SHIFT.0 != 0).unwrap_or(c);
                if params.is_empty() {
                    params = ";1".into();
                }
                params += &format!(";{}", text as u32);
            }
            csi(format!("{body}{params}u"))
        }
        KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace if !all && mods == 0 => {
            // Kept as in legacy mode (spec: "Disambiguate escape codes"), and without their release.
            match (kind, key.code) {
                (3, _) => Vec::new(),
                (_, KeyCode::Enter) => b"\r".to_vec(),
                (_, KeyCode::Tab) => b"\t".to_vec(),
                _ => b"\x7f".to_vec(),
            }
        }
        KeyCode::Enter => csi(format!("13{}u", mods_param(mods, kind))),
        KeyCode::Tab => csi(format!("9{}u", mods_param(mods, kind))),
        KeyCode::Backspace => csi(format!("127{}u", mods_param(mods, kind))),
        KeyCode::Esc => csi(format!("27{}u", mods_param(mods, kind))),
        KeyCode::F(3) => csi(format!("13{}~", mods_param(mods, kind))),
        KeyCode::F(n @ 13..=35) => csi(format!("{}{}u", 57376 + n as u32 - 13, mods_param(mods, kind))),
        code => {
            let letter = match code {
                KeyCode::F(n @ (1 | 2 | 4)) => Some(b'P' + n - 1),
                _ => cursor_letter(code),
            };
            match (letter, tilde_number(code)) {
                (Some(letter), _) if mods == 0 && kind == 1 => {
                    // F1, F2 and F4 as `CSI P`, `CSI Q`, `CSI S` (the spec's "legacy functional keys").
                    let ss3 = modes.app_cursor && !matches!(code, KeyCode::F(_));
                    let mut out = if ss3 { b"\x1bO".to_vec() } else { csi(String::new()) };
                    out.push(letter);
                    out
                }
                (Some(letter), _) => {
                    let mut out = csi(format!("1{}", mods_param(mods, kind)));
                    out.push(letter);
                    out
                }
                (None, Some(n)) => csi(format!("{n}{}~", mods_param(mods, kind))),
                (None, None) => Vec::new(),
            }
        }
    }
}

/// The bytes a mouse event at `col`, `row` of a pane (from 0) sends to it in `modes`. `None` when the pane does not
/// take it: then a wheel scrolls the history. On the alternate screen with 1007 and no mouse mode, the wheel
/// becomes cursor keys.
pub(crate) fn mouse(event: &Mouse, col: u16, row: u16, modes: &Modes) -> Option<Vec<u8>> {
    let Some(tracking) = modes.mouse else {
        let code = match event.kind {
            MouseKind::ScrollUp => KeyCode::Up,
            MouseKind::ScrollDown => KeyCode::Down,
            _ => return None,
        };
        if !(modes.alt_screen && modes.alternate_scroll) {
            return None;
        }
        return Some(key(&Key::new(code, Mods::NONE), modes));
    };
    let button = |b: Button| match b {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
    };
    let (mut cb, release) = match event.kind {
        MouseKind::Down(b) => (button(b), false),
        MouseKind::Up(b) => (if modes.mouse_sgr { button(b) } else { 3 }, true),
        MouseKind::Drag(b) if tracking != MouseTracking::Click => (32 + button(b), false),
        MouseKind::Moved if tracking == MouseTracking::Motion => (35, false),
        MouseKind::ScrollUp => (64, false),
        MouseKind::ScrollDown => (65, false),
        MouseKind::ScrollLeft => (66, false),
        MouseKind::ScrollRight => (67, false),
        MouseKind::Drag(_) | MouseKind::Moved => return None,
    };
    for (m, bit) in [(Mods::SHIFT, 4), (Mods::ALT, 8), (Mods::CTRL, 16)] {
        if event.mods.contains(m) {
            cb += bit;
        }
    }
    if modes.mouse_sgr {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{cb};{};{}{end}", col as u32 + 1, row as u32 + 1).into_bytes());
    }
    // X10: one byte each, 32 more than the value, cells from 1; beyond column 223 there is nothing to send.
    let (x, y) = (col as u32 + 33, row as u32 + 33);
    if x > 255 || y > 255 {
        return Some(Vec::new());
    }
    Some(vec![0x1b, b'[', b'M', 32 + cb, x as u8, y as u8])
}

/// A paste, framed when the pane asked for it (2004). Line ends become CR, as a terminal sends them; the control
/// characters but tab go, so that nothing in a clipboard ends the frame early or reads as keys.
pub(crate) fn paste(text: &str, modes: &Modes) -> Vec<u8> {
    let mut clean = String::with_capacity(text.len() + 12);
    if modes.bracketed_paste {
        clean.push_str("\x1b[200~");
    }
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                clean.push('\r');
            }
            '\n' => clean.push('\r'),
            '\t' => clean.push('\t'),
            c if c.is_control() => {}
            c => clean.push(c),
        }
    }
    if modes.bracketed_paste {
        clean.push_str("\x1b[201~");
    }
    clean.into_bytes()
}

/// A pane gaining or losing the focus, if it asked to be told (1004).
pub(crate) fn focus(gained: bool, modes: &Modes) -> Option<Vec<u8>> {
    modes.focus.then(|| if gained { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    use KeyCode::*;
    const S: Mods = Mods::SHIFT;
    const A: Mods = Mods::ALT;
    const C: Mods = Mods::CTRL;
    const N: Mods = Mods::NONE;

    fn modes() -> Modes {
        Modes::default()
    }

    fn kitty_modes(flags: u8) -> Modes {
        Modes { kitty: flags, ..Modes::default() }
    }

    fn check(modes: &Modes, table: &[(Key, &[u8])]) {
        for (k, bytes) in table {
            assert_eq!(key(k, modes), *bytes, "{k:?} in {modes:?}: {:?}", String::from_utf8_lossy(&key(k, modes)));
        }
    }

    fn k(code: KeyCode, mods: Mods) -> Key {
        Key::new(code, mods)
    }

    #[test]
    fn legacy_keys() {
        // xterm, ctlseqs: "PC-Style Function Keys", "Alt and Meta Keys" (metaSendsEscape), control characters.
        check(
            &modes(),
            &[
                (k(Char('a'), N), b"a"),
                (k(Char('A'), N), b"A"),
                (k(Char('a'), S), b"A"),
                (k(Char('é'), N), "é".as_bytes()),
                (k(Char('a'), A), b"\x1ba"),
                (k(Char('b'), C), b"\x02"),
                (k(Char('r'), C), b"\x12"),
                (k(Char('o'), C), b"\x0f"),
                (k(Char('c'), C), b"\x03"),
                (k(Char('d'), C), b"\x04"),
                (k(Char('h'), C), b"\x08"),
                (k(Char('j'), C), b"\n"),
                (k(Char('a'), C | S), b"\x01"),
                (k(Char('x'), C | A), b"\x1b\x18"),
                (k(Char(' '), C), b"\x00"),
                (k(Char('['), C), b"\x1b"),
                (k(Char('\\'), C), b"\x1c"),
                (k(Char(']'), C), b"\x1d"),
                (k(Char('_'), C), b"\x1f"),
                (k(Char('/'), C), b"\x1f"),
                (k(Char('?'), C), b"\x7f"),
                (k(Char('é'), C), "é".as_bytes()),
                (k(Enter, N), b"\r"),
                (k(Enter, A), b"\x1b\r"),
                (k(Enter, S), b"\n"),
                (k(Enter, C), b"\r"),
                (k(Tab, N), b"\t"),
                (k(Tab, S), b"\x1b[Z"),
                (k(Tab, A), b"\x1b\t"),
                (k(Backspace, N), b"\x7f"),
                (k(Backspace, A), b"\x1b\x7f"),
                (k(Backspace, C), b"\x08"),
                (k(Esc, N), b"\x1b"),
                (k(Esc, A), b"\x1b\x1b"),
                (k(Up, N), b"\x1b[A"),
                (k(Down, N), b"\x1b[B"),
                (k(Right, N), b"\x1b[C"),
                (k(Left, N), b"\x1b[D"),
                (k(Home, N), b"\x1b[H"),
                (k(End, N), b"\x1b[F"),
                (k(Up, S), b"\x1b[1;2A"),
                (k(Left, A), b"\x1b[1;3D"),
                (k(Right, C), b"\x1b[1;5C"),
                (k(End, C | S), b"\x1b[1;6F"),
                (k(Insert, N), b"\x1b[2~"),
                (k(Delete, N), b"\x1b[3~"),
                (k(Delete, C), b"\x1b[3;5~"),
                (k(PageUp, N), b"\x1b[5~"),
                (k(PageDown, S), b"\x1b[6;2~"),
                (k(F(1), N), b"\x1bOP"),
                (k(F(4), N), b"\x1bOS"),
                (k(F(1), S), b"\x1b[1;2P"),
                (k(F(5), N), b"\x1b[15~"),
                (k(F(10), N), b"\x1b[21~"),
                (k(F(11), N), b"\x1b[23~"),
                (k(F(12), C), b"\x1b[24;5~"),
                (k(F(13), N), b"\x1b[25~"),
                (k(F(21), N), b""),
            ],
        );
        // A release never reaches a pane that did not ask for them.
        let release = Key { kind: KeyKind::Release, ..k(Char('a'), N) };
        assert_eq!(key(&release, &modes()), b"");
        let repeat = Key { kind: KeyKind::Repeat, ..k(Char('a'), N) };
        assert_eq!(key(&repeat, &modes()), b"a");
    }

    #[test]
    fn application_cursor_keys() {
        // DECCKM: SS3 without modifiers, CSI with them.
        let app = Modes { app_cursor: true, ..modes() };
        check(
            &app,
            &[
                (k(Up, N), b"\x1bOA"),
                (k(Left, N), b"\x1bOD"),
                (k(Home, N), b"\x1bOH"),
                (k(End, N), b"\x1bOF"),
                (k(Up, C), b"\x1b[1;5A"),
                (k(PageUp, N), b"\x1b[5~"),
            ],
        );
    }

    #[test]
    fn kitty_disambiguate() {
        // https://sw.kovidgoyal.net/kitty/keyboard-protocol/, flag 1: text stays text, Enter, Tab and Backspace stay
        // legacy without modifiers; Esc and anything with a modifier but shift go as `CSI … u`.
        check(
            &kitty_modes(DISAMBIGUATE),
            &[
                (k(Char('a'), N), b"a"),
                (k(Char('A'), N), b"A"),
                (k(Char('a'), S), b"A"),
                (k(Char('é'), N), "é".as_bytes()),
                (k(Char('a'), A), b"\x1b[97;3u"),
                (k(Char('b'), C), b"\x1b[98;5u"),
                (k(Char('r'), C), b"\x1b[114;5u"),
                (k(Char('o'), C), b"\x1b[111;5u"),
                (k(Char('c'), C), b"\x1b[99;5u"),
                (k(Char('a'), C | S), b"\x1b[97;6u"),
                (k(Char('A'), C), b"\x1b[97;6u"),
                (k(Char(' '), C), b"\x1b[32;5u"),
                (k(Esc, N), b"\x1b[27u"),
                (k(Esc, A), b"\x1b[27;3u"),
                (k(Enter, N), b"\r"),
                (k(Enter, S), b"\x1b[13;2u"),
                (k(Enter, C), b"\x1b[13;5u"),
                (k(Enter, A), b"\x1b[13;3u"),
                (k(Tab, N), b"\t"),
                (k(Tab, S), b"\x1b[9;2u"),
                (k(Backspace, N), b"\x7f"),
                (k(Backspace, C), b"\x1b[127;5u"),
                (k(Up, N), b"\x1b[A"),
                (k(Up, S), b"\x1b[1;2A"),
                (k(Home, N), b"\x1b[H"),
                (k(F(1), N), b"\x1b[P"),
                (k(F(1), C), b"\x1b[1;5P"),
                (k(F(3), N), b"\x1b[13~"),
                (k(F(4), N), b"\x1b[S"),
                (k(F(5), N), b"\x1b[15~"),
                (k(F(13), N), b"\x1b[57376u"),
                (k(Delete, A), b"\x1b[3;3~"),
            ],
        );
    }

    #[test]
    fn kitty_alternates_events_all_and_text() {
        // Flag 1 | 4, what Claude Code pushes (`CSI > 5 u`): the shifted key after a colon.
        check(
            &kitty_modes(DISAMBIGUATE | ALTERNATES),
            &[
                (k(Char('a'), C | S), b"\x1b[97:65;6u"),
                (k(Char('A'), A), b"\x1b[97:65;4u"),
                (k(Char('a'), C), b"\x1b[97;5u"),
                (k(Enter, S), b"\x1b[13;2u"),
                (k(Char('A'), N), b"A"),
            ],
        );
        // Flag 1 | 2: event types; text keys and plain Enter report no release.
        let events = kitty_modes(DISAMBIGUATE | EVENT_TYPES);
        let kind = |key: Key, kind| Key { kind, ..key };
        check(
            &events,
            &[
                (kind(k(Esc, N), KeyKind::Release), b"\x1b[27;1:3u"),
                (kind(k(Esc, N), KeyKind::Repeat), b"\x1b[27;1:2u"),
                (kind(k(Up, C), KeyKind::Release), b"\x1b[1;5:3A"),
                (kind(k(Up, N), KeyKind::Release), b"\x1b[1;1:3A"),
                (kind(k(Char('b'), C), KeyKind::Release), b"\x1b[98;5:3u"),
                (kind(k(Char('a'), N), KeyKind::Release), b""),
                (kind(k(Enter, N), KeyKind::Release), b""),
                (kind(k(Char('a'), N), KeyKind::Repeat), b"a"),
            ],
        );
        // Without flag 2, a release reaches no pane.
        assert_eq!(key(&kind(k(Esc, N), KeyKind::Release), &kitty_modes(DISAMBIGUATE)), b"");
        // Flag 8: every key an escape code, Enter and its friends included; 16 adds the text.
        check(
            &kitty_modes(DISAMBIGUATE | ALL_AS_ESCAPES),
            &[
                (k(Char('a'), N), b"\x1b[97u"),
                (k(Char('A'), N), b"\x1b[97;2u"),
                (k(Enter, N), b"\x1b[13u"),
                (k(Tab, N), b"\x1b[9u"),
                (k(Backspace, N), b"\x1b[127u"),
            ],
        );
        check(
            &kitty_modes(ALL_AS_ESCAPES | TEXT | ALTERNATES),
            &[
                (k(Char('a'), N), b"\x1b[97;1;97u"),
                (k(Char('A'), N), b"\x1b[97:65;2;65u"),
                (k(Char('é'), N), b"\x1b[233;1;233u"),
                (k(Char('a'), C), b"\x1b[97;5u"),
            ],
        );
    }

    #[test]
    fn claude_code_keys_arrive() {
        // Spec §6: whatever the pane asked for, these reach Claude Code as the keys it knows.
        for modes in [modes(), kitty_modes(DISAMBIGUATE), kitty_modes(DISAMBIGUATE | ALTERNATES)] {
            let kitty = modes.kitty != 0;
            let expect: &[(Key, &[u8])] = &[
                (k(Enter, S), if kitty { b"\x1b[13;2u" } else { b"\n" }),
                (k(Char('b'), C), if kitty { b"\x1b[98;5u" } else { b"\x02" }),
                (k(Char('r'), C), if kitty { b"\x1b[114;5u" } else { b"\x12" }),
                (k(Char('o'), C), if kitty { b"\x1b[111;5u" } else { b"\x0f" }),
                (k(Tab, N), b"\t"),
                (k(Tab, S), if kitty { b"\x1b[9;2u" } else { b"\x1b[Z" }),
                (k(Esc, N), if kitty { b"\x1b[27u" } else { b"\x1b" }),
            ];
            check(&modes, expect);
        }
    }

    fn m(kind: MouseKind, mods: Mods) -> Mouse {
        Mouse { kind, col: 0, row: 0, mods }
    }

    #[test]
    fn mouse_reports() {
        use MouseKind::*;
        let sgr = |tracking| Modes { mouse: Some(tracking), mouse_sgr: true, ..modes() };
        let x10 = |tracking| Modes { mouse: Some(tracking), ..modes() };
        // xterm ctlseqs, "Mouse Tracking": button, +4 shift, +8 meta, +16 ctrl, +32 motion, 64 and up the wheel.
        type Case<'a> = (Modes, Mouse, u16, u16, Option<&'a [u8]>);
        let table: &[Case] = &[
            (sgr(MouseTracking::Click), m(Down(Button::Left), N), 0, 0, Some(b"\x1b[<0;1;1M")),
            (sgr(MouseTracking::Click), m(Up(Button::Left), N), 0, 0, Some(b"\x1b[<0;1;1m")),
            (sgr(MouseTracking::Click), m(Down(Button::Right), N), 9, 4, Some(b"\x1b[<2;10;5M")),
            (sgr(MouseTracking::Click), m(Up(Button::Middle), N), 9, 4, Some(b"\x1b[<1;10;5m")),
            (sgr(MouseTracking::Click), m(ScrollUp, N), 3, 2, Some(b"\x1b[<64;4;3M")),
            (sgr(MouseTracking::Click), m(ScrollDown, N), 3, 2, Some(b"\x1b[<65;4;3M")),
            (sgr(MouseTracking::Click), m(ScrollLeft, N), 0, 0, Some(b"\x1b[<66;1;1M")),
            (sgr(MouseTracking::Click), m(Down(Button::Left), S | C), 0, 0, Some(b"\x1b[<20;1;1M")),
            (sgr(MouseTracking::Click), m(Down(Button::Left), A), 0, 0, Some(b"\x1b[<8;1;1M")),
            (sgr(MouseTracking::Click), m(Drag(Button::Left), N), 1, 1, None),
            (sgr(MouseTracking::Click), m(Moved, N), 1, 1, None),
            (sgr(MouseTracking::Drag), m(Drag(Button::Left), N), 1, 1, Some(b"\x1b[<32;2;2M")),
            (sgr(MouseTracking::Drag), m(Drag(Button::Right), N), 1, 1, Some(b"\x1b[<34;2;2M")),
            (sgr(MouseTracking::Drag), m(Moved, N), 1, 1, None),
            (sgr(MouseTracking::Motion), m(Moved, N), 1, 1, Some(b"\x1b[<35;2;2M")),
            (sgr(MouseTracking::Motion), m(Drag(Button::Left), N), 1, 1, Some(b"\x1b[<32;2;2M")),
            (sgr(MouseTracking::Click), m(Down(Button::Left), N), 300, 1, Some(b"\x1b[<0;301;2M")),
            (x10(MouseTracking::Click), m(Down(Button::Left), N), 0, 0, Some(b"\x1b[M !!")),
            (x10(MouseTracking::Click), m(Up(Button::Left), N), 0, 0, Some(b"\x1b[M#!!")),
            (x10(MouseTracking::Click), m(ScrollUp, N), 9, 10, Some(b"\x1b[M`*+")),
            (x10(MouseTracking::Drag), m(Drag(Button::Left), C), 0, 0, Some(b"\x1b[MP!!")),
            (x10(MouseTracking::Click), m(Down(Button::Left), N), 222, 0, Some(b"\x1b[M \xff!")),
            (x10(MouseTracking::Click), m(Down(Button::Left), N), 223, 0, Some(b"")),
        ];
        for (modes, event, col, row, bytes) in table {
            assert_eq!(mouse(event, *col, *row, modes).as_deref(), *bytes, "{event:?} at {col},{row} in {modes:?}");
        }
    }

    #[test]
    fn the_wheel_without_mouse_mode() {
        let wheel = m(MouseKind::ScrollUp, N);
        // To the history.
        assert_eq!(mouse(&wheel, 0, 0, &modes()), None);
        assert_eq!(mouse(&wheel, 0, 0, &Modes { alternate_scroll: true, ..modes() }), None);
        assert_eq!(mouse(&m(MouseKind::Down(Button::Left), N), 0, 0, &modes()), None);
        // 1007 on the alternate screen: cursor keys, in the pane's cursor mode.
        let alt = Modes { alt_screen: true, alternate_scroll: true, ..modes() };
        assert_eq!(mouse(&wheel, 0, 0, &alt).as_deref(), Some(&b"\x1b[A"[..]));
        let down = m(MouseKind::ScrollDown, N);
        assert_eq!(mouse(&down, 0, 0, &Modes { app_cursor: true, ..alt }).as_deref(), Some(&b"\x1bOB"[..]));
        assert_eq!(mouse(&m(MouseKind::Down(Button::Left), N), 0, 0, &alt), None);
    }

    #[test]
    fn pastes() {
        let framed = Modes { bracketed_paste: true, ..modes() };
        let table: &[(&str, &[u8], &[u8])] = &[
            ("ab", b"ab", b"\x1b[200~ab\x1b[201~"),
            ("a\nb", b"a\rb", b"\x1b[200~a\rb\x1b[201~"),
            ("a\r\nb\r", b"a\rb\r", b"\x1b[200~a\rb\r\x1b[201~"),
            ("a\tb", b"a\tb", b"\x1b[200~a\tb\x1b[201~"),
            ("é😀", "é😀".as_bytes(), "\x1b[200~é😀\x1b[201~".as_bytes()),
            // Nothing in the text ends the frame or reads as keys.
            ("a\x1b[201~\x03b", b"a[201~b", b"\x1b[200~a[201~b\x1b[201~"),
            ("\u{9b}201~", b"201~", b"\x1b[200~201~\x1b[201~"),
        ];
        for &(text, plain, bracketed) in table {
            assert_eq!(paste(text, &modes()), plain, "{text:?}");
            assert_eq!(paste(text, &framed), bracketed, "{text:?}");
        }
        let big = "x".repeat(200_000) + "\n";
        assert_eq!(paste(&big, &framed).len(), 200_000 + 1 + 12);
    }

    #[test]
    fn focus_reports() {
        assert_eq!(focus(true, &modes()), None);
        let on = Modes { focus: true, ..modes() };
        assert_eq!(focus(true, &on).as_deref(), Some(&b"\x1b[I"[..]));
        assert_eq!(focus(false, &on).as_deref(), Some(&b"\x1b[O"[..]));
    }
}
