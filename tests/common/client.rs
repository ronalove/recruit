// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the tests that drive a team through a client on a test terminal share: keys and mouse reports as the terminal
//! sends them, where a text shows on the screen, and the states a fake member is told (OSC 7501, the mod's report).

use super::pty::TestTerm;
use super::team::Team;

/// The key Alt+`key` as the client's terminal sends it: kitty's keyboard protocol when it speaks it, else ESC.
pub fn alt(term: &TestTerm, key: char) -> Vec<u8> {
    if term.kitty_keyboard() {
        format!("\x1b[{};3u", key as u32).into_bytes()
    } else {
        format!("\x1b{key}").into_bytes()
    }
}

/// Where `text` first shows on the screen of `term`: column (in cells) and row.
pub fn find(term: &TestTerm, text: &str) -> Option<(usize, usize)> {
    term.screen().iter().enumerate().find_map(|(row, line)| {
        let at = line.find(text)?;
        Some((unicode_width::UnicodeWidthStr::width(&line[..at]), row))
    })
}

/// Where `text` first shows on a row that is a pane's top border (a header), not a card of the dashboard.
pub fn find_in_header(term: &TestTerm, text: &str) -> Option<(usize, usize)> {
    term.screen().iter().enumerate().find_map(|(row, line)| {
        if !line.chars().any(|c| matches!(c, '┏' | '╭')) {
            return None;
        }
        let at = line.find(text)?;
        Some((unicode_width::UnicodeWidthStr::width(&line[..at]), row))
    })
}

/// A click (press and release, SGR) on the cell `(col, row)`.
pub fn click(term: &mut TestTerm, (col, row): (usize, usize)) {
    term.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 1, row + 1, col + 1, row + 1).as_bytes());
}

/// A mouse move to `(col, row)`, as the terminal reports it with every motion on (`?1003`).
pub fn hover(term: &mut TestTerm, (col, row): (usize, usize)) {
    term.send(format!("\x1b[<35;{};{}M", col + 1, row + 1).as_bytes());
}

/// Whether the header on `row` of `name` has the thick frame of the active pane: the corner on the name's left is
/// `┏`, not `╭`, whatever stands between (the state's icon…).
pub fn thick(row: &str, name: &str) -> bool {
    let chars: Vec<char> = row.chars().collect();
    let name: Vec<char> = format!(" {name} ").chars().collect();
    (0..chars.len().saturating_sub(name.len() - 1))
        .filter(|&i| chars[i..i + name.len()] == name[..])
        .any(|i| chars[..i].iter().rev().find(|c| matches!(c, '┏' | '╭' | '┓' | '╮')).is_some_and(|c| *c == '┏'))
}

/// A member says what it does (OSC 7501), as Claude Code does: `working`, `blocked:kind=permission`, `idle`.
pub fn says(team: &Team, member: &str, state: &str) {
    team.ctl(["send", "--pane", member, &format!("osc {state}\\r")]);
}

/// What the mod last reported for `member` (`<state>/members/<member>.json`), as `board::looks` reads it.
pub fn report(team: &Team, member: &str, model: &str, effort: &str, percent: f64) {
    let dir = team.state().join("members");
    std::fs::create_dir_all(&dir).expect("members folder");
    let file = dir.join(format!("{member}.json"));
    let mut report = serde_json::json!({
        "session": "s", "model": model, "own": {"model": model, "effort": effort},
        "context": {"percent": percent, "tokens": (percent * 2000.0) as u64, "window": 200000}, "at": super::now_secs(),
    });
    // The mod's answer to a compaction stays through the reports told again.
    let kept = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|old| old.get("compaction").cloned());
    if let Some(compaction) = kept {
        report["compaction"] = compaction;
    }
    std::fs::write(file, report.to_string()).expect("report");
}

/// Standard base64, for the `msg` of an error state.
pub fn base64(text: &str) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { DIGITS[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// The member's turn failed on `message`, as Claude Code 2.1.296 says it (`state=error:app=claude-code:msg=<base64>`).
pub fn fails(team: &Team, member: &str, message: &str) {
    says(team, member, &format!("error:app=claude-code:msg={}", base64(message)));
}
