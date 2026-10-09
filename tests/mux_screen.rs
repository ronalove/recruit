// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What a pane draws, as the real terminal gets it through the native multiplexer (grid `R`, `Co`, `U`, `T`, `F`,
//! `M`): a server of the test's own, the fake `script` in its panes writing known sequences and logging what it
//! reads, a real client on the test's terminal, whose cells, colors and cursor are compared. Part of `cargo test`.
//!
//! Owner: testeur.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::pty::{Profile, TestTerm};
use common::server::Server;

#[test]
#[ignore = "started by the other tests only"]
fn child() {
    match common::role().as_deref() {
        Some(role) if common::fakes::run(role) => {}
        Some(other) => panic!("unknown child role {other}"),
        None => {}
    }
}

/// Bytes as `script` takes them (`SCRIPT_HEX`).
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What `script` has read so far, from its log.
fn logged(log: &Path) -> Vec<u8> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut bytes = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with("ready ")) {
        if let Some(hex) = line.split_whitespace().nth(1) {
            bytes.extend((0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()));
        }
    }
    bytes
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Waits until `check` holds, `timeout` at most.
fn wait(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// How many SGR mouse reports in `bytes` are presses (`CSI < … M`; a release ends in `m`).
fn sgr_presses(bytes: &[u8]) -> usize {
    let mut presses = 0;
    let mut i = 0;
    while let Some(at) = bytes[i..].windows(3).position(|w| w == b"\x1b[<") {
        let start = i + at + 3;
        let end = bytes[start..].iter().position(|b| !(b.is_ascii_digit() || *b == b';')).map(|p| start + p);
        if let Some(end) = end {
            presses += usize::from(bytes[end] == b'M');
            i = end + 1;
        } else {
            break;
        }
    }
    presses
}

/// Whether the frame whose header (in `top`, the screen's first row) names `name` is the thick one, the focused
/// pane's: the corner on the name's left is `┏`, not `╭`, whatever stands between (the state's icon…).
fn thick(top: &str, name: &str) -> bool {
    let chars: Vec<char> = top.chars().collect();
    let name: Vec<char> = format!(" {name} ").chars().collect();
    (0..chars.len().saturating_sub(name.len() - 1))
        .filter(|&i| chars[i..i + name.len()] == name[..])
        .any(|i| chars[..i].iter().rev().find(|c| matches!(c, '┏' | '╭' | '┓' | '╮')).is_some_and(|c| *c == '┏'))
}

/// Where `text` first shows on the screen of `term`: column (in cells) and row.
fn find(term: &TestTerm, text: &str) -> Option<(usize, usize)> {
    term.screen().iter().enumerate().find_map(|(row, line)| {
        let at = line.find(text)?;
        Some((unicode_width::UnicodeWidthStr::width(&line[..at]), row))
    })
}

fn shown(term: &TestTerm) -> String {
    term.screen().iter().enumerate().map(|(i, r)| format!("{i:2}|{r}\n")).collect()
}

/// A server, a client on a test terminal of 100 × 30 (`profile`, `env` on top), then one pane per `(member, bytes,
/// env vars to write)`, each logging what it reads to `<dir>/<member>.log`; their first rows waited for.
fn served(
    dir: &Path,
    profile: Profile,
    env: &[(&str, &str)],
    panes: &[(&str, &[u8], &str)],
    ready: &[&str],
) -> (Server, TestTerm) {
    let server = Server::start(dir);
    let term = server.attach_with(100, 30, Profile { capture: true, ..profile }, env);
    assert!(
        wait(Duration::from_secs(10), || server.clients()["clients"].as_array().is_some_and(|c| !c.is_empty())),
        "the client never attached"
    );
    for (member, bytes, vars) in panes {
        let log = dir.join(format!("{member}.log")).to_string_lossy().into_owned();
        server.spawn_fake(
            member,
            "script",
            &[("SCRIPT_HEX", hex(bytes)), ("SCRIPT_LOG", log), ("SCRIPT_ENV", (*vars).into())],
        );
    }
    for text in ready {
        assert!(term.wait_for(text, Duration::from_secs(10)), "{text} never showed:\n{}", shown(&term));
    }
    (server, term)
}

/// One pane drawn as written (grid R3, R9, R10, Co1, Co3, U4, U7, T1, T3): a scrolling region, 24-bit colors, a
/// decomposed accent, boxes and blocks, the cursor's place and shape; the queries the pane makes, answered with
/// the real terminal's colors; the variables it is given; and every frame the client sends in a synchronized
/// update. On a terminal that says it is Ghostty (synchronized updates, 24 bits).
#[test]
fn a_pane_as_written() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut bytes: Vec<u8> = Vec::new();
    // Rows 1 to 3: the variables. Row 5 above the region, 7 to 9 the region, 10 below it.
    bytes.extend(b"\x1b[5;1HABOVE\x1b[10;1HBELOW\x1b[7;9r\x1b[9;1HS1\r\nS2\r\nS3\r\nS4\r\nS5\x1b[r");
    bytes.extend(b"\x1b[12;1H\x1b[38;2;1;2;3mC\x1b[0m\x1b[48;2;4;5;6mD\x1b[0m");
    bytes.extend("\x1b[13;1He\u{301}x".as_bytes());
    bytes.extend("\x1b[14;1H┌─┐│└┘█▀▄░".as_bytes());
    bytes.extend(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[>c\x1b[?2027$p");
    bytes.extend(b"\x1b[15;1HSCREEN READY\x1b[16;7H\x1b[4 q");
    let profile = Profile { xtversion: Some("ghostty 1.3.1"), ..Profile::default() };
    let panes: &[(&str, &[u8], &str)] = &[("alpha", &bytes, "TERM,COLORTERM,TERM_PROGRAM")];
    let (mut server, term) = served(dir.path(), profile, &[], panes, &["SCREEN READY"]);
    let log = dir.path().join("alpha.log");
    assert!(wait(Duration::from_secs(5), || contains(&logged(&log), b"$y")), "no answer to DECRQM: {:?}", logged(&log));
    std::thread::sleep(Duration::from_millis(300));
    let (x, y) = find(&term, "TERM=").expect("TERM= shown");
    // A row of the pane, without the frame on its right.
    let row = |n: usize| {
        let line: String = term.screen()[y + n - 1].chars().skip(x).collect();
        line.trim_end_matches(['┃', '│']).trim_end().to_string()
    };
    let mut bad: Vec<String> = Vec::new();
    let mut check = |what: &str, ok: bool, got: String| {
        println!("| {what} | {} | {got} |", if ok { "ok" } else { "KO" });
        if !ok {
            bad.push(what.into());
        }
    };
    // T1.
    let vars = format!("{} / {} / {}", row(1), row(2), row(3));
    check(
        "T1 TERM, COLORTERM, TERM_PROGRAM",
        row(1) == "TERM=xterm-256color" && row(2) == "COLORTERM=truecolor" && row(3) == "TERM_PROGRAM=recruit",
        vars,
    );
    // R3.
    let region = [5, 7, 8, 9, 10].map(|n| row(n).trim_end().to_string());
    check("R3 scrolling region", region == ["ABOVE", "S3", "S4", "S5", "BELOW"], format!("{region:?}"));
    // Co1.
    let c = term.cell(x, y + 11);
    let d = term.cell(x + 1, y + 11);
    check(
        "Co1 24-bit colors",
        c.0 == "C" && c.1.contains("r: 1, g: 2, b: 3") && d.0 == "D" && d.2.contains("r: 4, g: 5, b: 6"),
        format!("{c:?} {d:?}"),
    );
    // U4.
    let e = term.cell(x, y + 12);
    let after = term.cell(x + 1, y + 12);
    check("U4 decomposed accent in one cell", e.0 == "e\u{301}" && after.0 == "x", format!("{e:?} {after:?}"));
    // U7.
    check("U7 boxes and blocks", row(14).trim_end() == "┌─┐│└┘█▀▄░", row(14));
    // R10.
    let cursor = term.cursor();
    check(
        "R10 cursor place and shape (steady: no blinking)",
        // DECSCUSR 4 is a steady underline: `TestTerm::cursor` would say "Underline, blinking" for 3.
        cursor.0 == x + 6 && cursor.1 == y + 15 && cursor.2 && cursor.3 == "Underline",
        format!("{cursor:?}, wanted ({}, {}, true, \"Underline\")", x + 6, y + 15),
    );
    // Co3, T3.
    let answers = String::from_utf8_lossy(&logged(&log)).into_owned();
    check(
        "Co3 OSC 10 and 11, the real terminal's",
        answers.contains("]10;rgb:c0c0/c1c1/c2c2") && answers.contains("]11;rgb:1e1e/1e1e/1e1e"),
        format!("{answers:?}"),
    );
    check("T3 DA2 answered", answers.contains("\x1b[>"), format!("{answers:?}"));
    check("T3 DECRQM 2027 answered", answers.contains("\x1b[?2027;"), format!("{answers:?}"));
    // R9.
    let raw = term.raw();
    let outside = outside_frames(&raw);
    check(
        "R9 every frame in a synchronized update",
        count(&raw, b"\x1b[?2026h") > 0 && outside.is_empty(),
        format!("{} frames, outside them: {:?}", count(&raw, b"\x1b[?2026h"), outside),
    );
    drop(term);
    server.stop();
    assert!(bad.is_empty(), "wrong: {bad:?}\n{}", answers);
}

/// The text the client wrote outside its synchronized updates (escape sequences aside), and an update never ended.
fn outside_frames(raw: &[u8]) -> String {
    let mut out = String::new();
    let mut inside = false;
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == 0x1b {
            let rest = &raw[i..];
            if rest.starts_with(b"\x1b[?2026h") {
                inside = true;
            } else if rest.starts_with(b"\x1b[?2026l") {
                inside = false;
            }
            i += escape_len(rest);
            continue;
        }
        if !inside && raw[i] >= 0x20 {
            out.push(raw[i] as char);
        }
        i += 1;
    }
    if inside {
        out.push_str(" (an update never ended)");
    }
    out
}

/// The length of the escape sequence at the start of `rest`: CSI to its final byte, OSC, DCS and APC to BEL or
/// ST, two bytes otherwise.
fn escape_len(rest: &[u8]) -> usize {
    match rest.get(1) {
        Some(b'[') => rest.iter().skip(2).position(|b| (0x40..=0x7e).contains(b)).map_or(rest.len(), |p| p + 3),
        Some(b']' | b'P' | b'_') => {
            let mut j = 2;
            while j < rest.len() {
                if rest[j] == 0x07 {
                    return j + 1;
                }
                if rest[j] == 0x1b && rest.get(j + 1) == Some(&b'\\') {
                    return j + 2;
                }
                j += 1;
            }
            rest.len()
        }
        Some(_) => 2,
        None => 1,
    }
}

/// Without 24 bits (no `COLORTERM`, a terminal not known by name), colors fall back to the 256 (grid Co4).
#[test]
fn colors_without_24_bits() {
    let dir = tempfile::tempdir().expect("temp dir");
    let bytes = b"\x1b[38;2;255;0;0mR\x1b[0m COLORS READY";
    let panes: &[(&str, &[u8], &str)] = &[("alpha", bytes, "")];
    let (mut server, term) = served(dir.path(), Profile::default(), &[("COLORTERM", "")], panes, &["COLORS READY"]);
    let (x, y) = find(&term, "R COLORS").expect("shown");
    let cell = term.cell(x, y);
    let raw = term.raw();
    drop(term);
    server.stop();
    assert!(cell.1.contains("Indexed(196)"), "red not as color 196: {cell:?}");
    assert!(!contains(&raw, b"38;2;"), "24-bit colors sent anyway");
}

/// What leaves the top of a pane goes to its history, in order, none lost (grid R4).
#[test]
fn history_in_order() {
    let dir = tempfile::tempdir().expect("temp dir");
    let lines: String = (1..=2000).map(|i| format!("L{i:04}\r\n")).collect::<String>() + "HISTORY DONE";
    let panes: &[(&str, &[u8], &str)] = &[("alpha", lines.as_bytes(), "")];
    let (mut server, term) = served(dir.path(), Profile::default(), &[], panes, &["HISTORY DONE"]);
    let captured = server.capture_history("alpha", 5000);
    drop(term);
    server.stop();
    let numbers: Vec<usize> = captured.iter().filter_map(|l| l.trim().strip_prefix('L')?.parse().ok()).collect();
    let want: Vec<usize> = (1..=2000).collect();
    assert!(
        numbers == want,
        "history: {} lines, first {:?}, last {:?}",
        numbers.len(),
        numbers.first(),
        numbers.last()
    );
}

/// Focus and clicks between two panes (grid F1, F2, M1, M5, R10): `alpha` asks for focus events, `beta` for
/// clicks (1000, SGR) and not for focus. The terminal's own focus goes to the active pane; a click in `beta` gives
/// it the focus and reaches it once, at its own coordinates; `alpha` hears it lost the focus, `beta` nothing; ⌥n
/// back to `alpha`, which hears it; the cursor is always the active pane's.
#[test]
fn focus_and_clicks() {
    let dir = tempfile::tempdir().expect("temp dir");
    let alpha = b"\x1b[?1004hALPHA READY";
    let beta = b"\x1b[?1000h\x1b[?1006hBETA READY";
    let panes: &[(&str, &[u8], &str)] = &[("alpha", alpha, ""), ("beta", beta, "")];
    let (mut server, mut term) = served(dir.path(), Profile::default(), &[], panes, &["ALPHA READY", "BETA READY"]);
    let (alpha_log, beta_log) = (dir.path().join("alpha.log"), dir.path().join("beta.log"));
    let mut bad: Vec<String> = Vec::new();
    let mut check = |what: &str, ok: bool, got: String| {
        println!("| {what} | {} | {got} |", if ok { "ok" } else { "KO" });
        if !ok {
            bad.push(what.into());
        }
    };
    let (ax, ay) = find(&term, "ALPHA READY").expect("alpha");
    let (bx, by) = find(&term, "BETA READY").expect("beta");
    let in_alpha = |c: &(usize, usize, bool, String)| c.0 >= ax && c.0 < bx - 1 && c.1 >= ay;
    // The frame that puts it there may come after the text.
    let _ = wait(Duration::from_secs(3), || {
        let c = term.cursor();
        in_alpha(&c) && c.2
    });
    let cursor = term.cursor();
    check("R10 cursor in the active pane (alpha)", in_alpha(&cursor) && cursor.2, format!("{cursor:?}"));
    // F2: the terminal loses the focus, then gets it back.
    let raw = term.raw();
    check("F2 the client asks its terminal for focus events", contains(&raw, b"\x1b[?1004h"), String::new());
    term.send(b"\x1b[O");
    term.send(b"\x1b[I");
    let got = wait(Duration::from_secs(3), || contains(&logged(&alpha_log), b"\x1b[O\x1b[I"));
    check("F2 relayed to the active pane", got, format!("{:?}", String::from_utf8_lossy(&logged(&alpha_log))));
    // M5, M1: a click in beta, at its 10th column and 4th row.
    let before = logged(&alpha_log).len();
    let (cx, cy) = (bx + 9, by + 3);
    term.send(format!("\x1b[<0;{};{}M", cx + 1, cy + 1).as_bytes());
    term.send(format!("\x1b[<0;{};{}m", cx + 1, cy + 1).as_bytes());
    let focused = wait(Duration::from_secs(3), || thick(&term.screen()[0], "beta"));
    check("M5 a click gives beta the focus", focused, term.screen()[0].clone());
    // Pressed then released: once the release is in, a press sent twice would be too.
    let release = format!("\x1b[<0;10;{}m", cy - by + 1);
    let _ = wait(Duration::from_secs(3), || contains(&logged(&beta_log), release.as_bytes()));
    let clicks = logged(&beta_log);
    let press = format!("\x1b[<0;10;{}M", cy - by + 1);
    let presses = count(&clicks, press.as_bytes());
    check(
        "M1/M5 the click reaches beta once, at its coordinates",
        presses == 1 && sgr_presses(&clicks) == 1,
        format!("{:?}, wanted {press:?} once", String::from_utf8_lossy(&clicks)),
    );
    let left = logged(&alpha_log)[before..].to_vec();
    check("F1 alpha hears it lost the focus", left == b"\x1b[O", format!("{:?}", String::from_utf8_lossy(&left)));
    check("F1 beta, which did not ask, hears nothing", !contains(&clicks, b"\x1b[I"), String::new());
    let cursor = term.cursor();
    check("R10 cursor in beta now", cursor.0 >= bx && cursor.2, format!("{cursor:?}"));
    // ⌥n back to alpha.
    let before = logged(&alpha_log).len();
    term.send(if term.kitty_keyboard() { b"\x1b[110;3u" } else { b"\x1bn" });
    let back = wait(Duration::from_secs(3), || thick(&term.screen()[0], "alpha"));
    let got = wait(Duration::from_secs(3), || logged(&alpha_log).len() > before);
    let heard = logged(&alpha_log)[before..].to_vec();
    check(
        "F1 ⌥n: alpha hears it has the focus",
        back && got && heard == b"\x1b[I",
        format!("{:?}", String::from_utf8_lossy(&heard)),
    );
    let cursor = term.cursor();
    check("R10 cursor back in alpha", in_alpha(&cursor), format!("{cursor:?}"));
    drop(term);
    server.stop();
    assert!(bad.is_empty(), "wrong: {bad:?}");
}
