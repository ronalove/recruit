// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A gallery of the native multiplexer's screens with their styles, for the design review: a team of fakes in the
//! states of the design (at work, waiting, at rest, ended), a client on a test terminal at several sizes, the pointer
//! over the parts of a header and of the bar, the zoom, the notice. Writes `<name>.json` (the styled cells) and
//! `<name>.txt` into `target/night/gallery/` (or `GALLERY_OUT`), then `python3 -I tests/tools/gallery.py <folder>`
//! draws the PNG files.
//!
//! Run by hand, in a pty of its own, no window: `cargo test --test mux_gallery -- --ignored --nocapture`.
//!
//! Owner: testeur.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::pty::{Profile, TestTerm};
use common::team::{Team, wait};

#[test]
#[ignore = "started by the other tests only"]
fn child() {
    match common::role().as_deref() {
        Some(role) if common::fakes::run(role) => {}
        Some(other) => panic!("unknown child role {other}"),
        None => {}
    }
}

fn out_dir() -> PathBuf {
    let dir = std::env::var_os("GALLERY_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/night/gallery"));
    std::fs::create_dir_all(&dir).expect("gallery folder");
    dir
}

fn alt(term: &TestTerm, key: char) -> Vec<u8> {
    if term.kitty_keyboard() {
        format!("\x1b[{};3u", key as u32).into_bytes()
    } else {
        format!("\x1b{key}").into_bytes()
    }
}

/// Where `text` first shows: column (in cells) and row.
fn find(term: &TestTerm, text: &str) -> Option<(usize, usize)> {
    term.screen().iter().enumerate().find_map(|(row, line)| {
        let at = line.find(text)?;
        Some((unicode_width::UnicodeWidthStr::width(&line[..at]), row))
    })
}

/// Where `text` first shows on a row that is a pane's top border (a header), not the dashboard's cards.
fn find_in_header(term: &TestTerm, text: &str) -> Option<(usize, usize)> {
    term.screen().iter().enumerate().find_map(|(row, line)| {
        if !line.chars().any(|c| matches!(c, '┏' | '╭')) {
            return None;
        }
        let at = line.find(text)?;
        Some((unicode_width::UnicodeWidthStr::width(&line[..at]), row))
    })
}

/// The pointer at `(col, row)`, no button (SGR motion).
fn hover(term: &mut TestTerm, (col, row): (usize, usize)) {
    term.send(format!("\x1b[<35;{};{}M", col + 1, row + 1).as_bytes());
}

fn report(team: &Team, member: &str, model: &str, effort: &str, percent: f64) {
    let dir = team.state().join("members");
    std::fs::create_dir_all(&dir).expect("members folder");
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let report = serde_json::json!({
        "session": "s", "model": model, "own": {"model": model, "effort": effort},
        "context": {"percent": percent, "tokens": (percent * 2000.0) as u64, "window": 200000}, "at": at,
    });
    std::fs::write(dir.join(format!("{member}.json")), report.to_string()).expect("report");
}

fn says(team: &Team, member: &str, state: &str) {
    team.ctl(["send", "--pane", member, &format!("osc {state}\\r")]);
}

struct Gallery {
    dir: PathBuf,
    shots: Vec<String>,
}

impl Gallery {
    /// The screen of `term` once it settled, as `<name>`, with `what` it should show.
    fn shot(&mut self, team: &Team, term: &TestTerm, name: &str, what: &str) {
        std::thread::sleep(Duration::from_millis(600));
        // One look at the screen for both files: the text is read from the styled cells, so that the two always say the
        // same (a second look, a redraw later, once made them differ).
        let styled = term.styled();
        let text = screen_text(&styled);
        std::fs::write(self.dir.join(format!("{name}.json")), &styled).expect("json");
        std::fs::write(self.dir.join(format!("{name}.txt")), format!("{what}\n\n{text}\n")).expect("txt");
        let dashboard = team.panes().iter().any(|p| p["role"] == "dashboard");
        println!("shot {name}: {what} (dashboard pane alive: {dashboard})");
        self.shots.push(format!("{name}: {what}"));
    }
}

/// The text of a screen given as `TestTerm::styled` does: one line a row, the wide characters' spacers left out, the
/// right blanks cut.
fn screen_text(styled: &str) -> String {
    let screen: serde_json::Value = serde_json::from_str(styled).expect("styled screen");
    screen["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let line: String = row
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|cell| !cell[3].as_str().unwrap_or_default().contains('w'))
                        .map(|cell| cell[0].as_str().unwrap_or(" "))
                        .collect();
                    line.trim_end().to_string()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The design's team: two contacts, four agents in « work » (at work, waiting, at rest, done), two in « qa ».
fn gallery_team() -> Team {
    gallery_team_in("fr", false)
}

/// The same in `lang`; `work_model`: `work` has a model in the file (`opus`), so that its list has a value in place
/// that a return to the default would start again.
fn gallery_team_in(lang: &'static str, work_model: bool) -> Team {
    let tables = "\
[teams.gal]
description = \"gallery\"
[teams.gal.members.lead]
role = \"Contact\"
contact = true
[teams.gal.members.pm]
role = \"Contact\"
contact = true
[teams.gal.members.work]
role = \"Agent\"
tab = \"work\"
[teams.gal.members.waits]
role = \"Agent\"
tab = \"work\"
[teams.gal.members.rests]
role = \"Agent\"
tab = \"work\"
[teams.gal.members.done]
role = \"Agent\"
tab = \"work\"
[teams.gal.members.q1]
role = \"Agent\"
tab = \"qa\"
[teams.gal.members.q2]
role = \"Agent\"
tab = \"qa\"
";
    let tables = if work_model {
        tables.replace(
            "[teams.gal.members.work]\nrole = \"Agent\"\n",
            "[teams.gal.members.work]\nrole = \"Agent\"\nmodel = \"opus\"\n",
        )
    } else {
        tables.to_string()
    };
    let team = Team::in_lang("gal", &tables, lang);
    team.launch(&["lead", "pm", "work", "waits", "rests", "done", "q1", "q2"]);
    for (member, model, effort, percent) in [
        ("lead", "claude-fable-5-1", "max", 12.0),
        ("pm", "claude-opus-5-5", "medium", 30.0),
        ("work", "claude-opus-5-5", "xhigh", 40.0),
        ("waits", "claude-sonnet-5-5", "high", 62.0),
        ("rests", "claude-haiku-5-5", "low", 91.0),
        ("done", "claude-opus-5-5", "high", 55.0),
        ("q1", "claude-sonnet-5-5", "medium", 20.0),
        ("q2", "claude-haiku-5-5", "low", 5.0),
    ] {
        report(&team, member, model, effort, percent);
    }
    for (member, state) in [
        ("lead", "idle"),
        ("pm", "working"),
        ("work", "working"),
        ("waits", "blocked:kind=permission"),
        ("rests", "idle"),
        ("done", "done"),
        ("q1", "idle"),
        ("q2", "working"),
    ] {
        says(&team, member, state);
    }
    std::thread::sleep(Duration::from_secs(3));
    team
}

#[test]
#[ignore = "by hand: the design review's gallery"]
fn gallery() {
    let mut team = gallery_team();
    let mut gallery = Gallery { dir: out_dir(), shots: Vec::new() };
    let mut term = team.attach(140, 40, Profile::default());
    assert!(term.wait_for("lead", Duration::from_secs(5)));
    gallery.shot(&team, &term, "01-contacts-140x40", "tab 1: two contacts, the dashboard and the journal");
    let keys = alt(&term, '2');
    term.send(&keys);
    wait(Duration::from_secs(3), || find_in_header(&term, "work").is_some());
    println!("panes at 140x40: {}", team.ctl(["panes", "--json"]));
    gallery.shot(
        &team,
        &term,
        "02-work-tab-140x40",
        "tab 2: at work, waiting, at rest, done \
         (a pane whose program ended is closed at once: no « ended » frame to show)",
    );
    // The pointer over the parts of a header and of the bar.
    if let Some(at) = find_in_header(&term, "work") {
        hover(&mut term, at);
        gallery.shot(&team, &term, "03-hover-name", "the pointer on the name of the member at work");
    }
    if let Some(at) = find_in_header(&term, "Opus") {
        hover(&mut term, at);
        gallery.shot(&team, &term, "04-hover-model", "the pointer on the model");
    }
    if let Some(at) = find_in_header(&term, "extra").or_else(|| find_in_header(&term, "\u{25c9}")) {
        hover(&mut term, at);
        gallery.shot(&team, &term, "05-hover-effort", "the pointer on the effort");
    }
    if let Some(at) = find_in_header(&term, "\u{27f3}") {
        hover(&mut term, at);
        gallery.shot(
            &team,
            &term,
            "06-hover-context",
            "the pointer on the context of the member at rest (compactable)",
        );
    }
    if let Some(at) = find(&term, "1 Interlocuteurs") {
        hover(&mut term, at);
        gallery.shot(&team, &term, "07-hover-tab", "the pointer on a tab of the bar");
    }
    if let Some(at) = find(&term, "quitter") {
        hover(&mut term, at);
        gallery.shot(&team, &term, "08-hover-button", "the pointer on the quit button");
    }
    hover(&mut term, (0, 0));
    // The notice and the badge: a member out of view goes waiting.
    says(&team, "q1", "blocked:kind=permission");
    std::thread::sleep(Duration::from_millis(800));
    gallery.shot(
        &team,
        &term,
        "09-notice-and-badge",
        "q1 (tab qa, out of view) waits: the notice at the top right, the badge on the tab",
    );
    std::thread::sleep(Duration::from_secs(7));
    gallery.shot(&team, &term, "10-badge-after-notice", "seven seconds later: the notice is gone, the badge stays");
    // Zoom, and the header at the widths of the design.
    let keys = alt(&term, 'z');
    term.send(&keys);
    gallery.shot(&team, &term, "11-zoom-140x40", "the member at work zoomed: the bar says so");
    for cols in [72u16, 52, 40, 34, 26, 20] {
        term.resize(cols, 24);
        wait(Duration::from_secs(2), || term.size().0 == cols as usize);
        gallery.shot(&team, &term, &format!("12-zoom-{cols}x24"), &format!("the zoomed header at {cols} columns"));
    }
    // 80 × 24 and back: the bar and the tabs.
    term.send(&keys);
    term.resize(80, 24);
    wait(Duration::from_secs(2), || term.size() == (80, 24));
    gallery.shot(&team, &term, "13-work-80x24", "tab 2 at 80 x 24");
    let keys1 = alt(&term, '1');
    term.send(&keys1);
    gallery.shot(&team, &term, "14-contacts-80x24", "tab 1 at 80 x 24: the bar's narrow form");
    let keys_g = alt(&term, 'g');
    term.send(&keys_g);
    gallery.shot(&team, &term, "15-alt-g-80x24", "after Alt+g: the member who waits the longest");
    term.resize(140, 40);
    wait(Duration::from_secs(2), || term.size() == (140, 40));
    gallery.shot(&team, &term, "16-after-alt-g-140x40", "back at 140 x 40, on the member who waits");
    std::fs::write(gallery.dir.join("INDEX.txt"), gallery.shots.join("\n") + "\n").expect("index");
    team.stop();
}

/// Narrow screens: the first tab started at 80 × 24 straight (no resize), then the bar at 30, 24 and 20 columns with
/// a tab zoomed and two badges (« work » and « qa » have a member who waits).
#[test]
#[ignore = "by hand: the design review's gallery"]
fn gallery_narrow() {
    let mut team = gallery_team();
    let mut gallery = Gallery { dir: out_dir(), shots: Vec::new() };
    let term = team.attach(80, 24, Profile::default());
    assert!(term.wait_for("lead", Duration::from_secs(5)));
    gallery.shot(
        &team,
        &term,
        "20-contacts-started-at-80x24",
        "tab 1, the client started at 80 x 24 (no resize before)",
    );
    drop(term);
    let mut term = team.attach(30, 24, Profile::default());
    assert!(term.wait_for("lead", Duration::from_secs(5)));
    says(&team, "q1", "blocked:kind=permission");
    std::thread::sleep(Duration::from_secs(1));
    let keys = alt(&term, 'z');
    term.send(&keys);
    gallery.shot(
        &team,
        &term,
        "21-bar-30-zoomed-two-badges",
        "30 columns, tab 1 with lead zoomed, badges on tabs 2 and 3",
    );
    for cols in [24u16, 20] {
        term.resize(cols, 24);
        wait(Duration::from_secs(2), || term.size().0 == cols as usize);
        gallery.shot(&team, &term, &format!("22-bar-{cols}-zoomed-two-badges"), &format!("{cols} columns, same"));
    }
    std::fs::write(gallery.dir.join("INDEX-narrow.txt"), gallery.shots.join("\n") + "\n").expect("index");
    team.stop();
}

/// Standard base64, for the `msg` of an error state.
fn base64(text: &str) -> String {
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

/// The member's turn failed, as Claude Code 2.1.296 says it (`state=error:app=claude-code:msg=<base64>`).
fn fails(team: &Team, member: &str) {
    let message =
        "API Error: 529 {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}";
    says(team, member, &format!("error:app=claude-code:msg={}", base64(message)));
}

/// A failed turn (« tour raté ») for the design review, at 140 × 40 and at 64 × 19 (the mock-up's): the first tab with
/// the dashboard, the member's card and the notice; the member's tab with its frame and header; the member's sheet
/// in the menu (⌥r on its pane).
#[test]
#[ignore = "by hand: the design review's gallery of the failed turn"]
fn gallery_failed_turn() {
    let mut team = gallery_team();
    let mut gallery = Gallery { dir: out_dir(), shots: Vec::new() };
    for (size, (cols, rows)) in [("140x40", (140u16, 40u16)), ("64x19", (64, 19))] {
        let mut term = team.attach(cols, rows, Profile::default());
        // The server kept the tab the last client was on: back to the first.
        let keys = alt(&term, '1');
        term.send(&keys);
        assert!(term.wait_for("lead", Duration::from_secs(5)), "{}", term.screen().join("\n"));
        // `work` was at work; its turn fails (out of view: on tab 2).
        fails(&team, "work");
        std::thread::sleep(Duration::from_millis(1500));
        gallery.shot(
            &team,
            &term,
            &format!("30-failed-tab1-notice-{size}"),
            "tab 1: the dashboard with the card of `work` (failed), the notice at the top right, the badge on tab 2",
        );
        let keys = alt(&term, '2');
        term.send(&keys);
        wait(Duration::from_secs(3), || find_in_header(&term, "work").is_some());
        gallery.shot(
            &team,
            &term,
            &format!("31-failed-tab-work-{size}"),
            "tab 2: `work`'s pane with the failed turn's frame and header (✗, reason after the name)",
        );
        let keys = alt(&term, 'r');
        term.send(&keys);
        wait(Duration::from_secs(5), || team.panes().iter().any(|p| p["role"] == "menu"));
        std::thread::sleep(Duration::from_millis(1200));
        gallery.shot(
            &team,
            &term,
            &format!("32-failed-sheet-{size}"),
            "the menu on `work`'s sheet (the whole text of the error)",
        );
        term.send(b"\x1b");
        std::thread::sleep(Duration::from_millis(800));
        drop(term);
        // The next size starts from a member at work again, and past the ten seconds that merge two notices.
        says(&team, "work", "working");
        std::thread::sleep(Duration::from_secs(11));
    }
    std::fs::write(gallery.dir.join("INDEX-failed.txt"), gallery.shots.join("\n") + "\n").expect("index");
    team.stop();
}

/// A click (press and release, SGR) on the cell `(col, row)`.
fn click(term: &mut TestTerm, (col, row): (usize, usize)) {
    term.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 1, row + 1, col + 1, row + 1).as_bytes());
}

/// Waits for the menu's pane (a list or the menu), then lets it settle.
fn wait_menu(team: &Team) -> bool {
    let up = wait(Duration::from_secs(8), || team.panes().iter().any(|p| p["role"] == "menu"));
    std::thread::sleep(Duration::from_millis(900));
    up
}

/// Esc until the menu's pane is gone.
fn close_menu(team: &Team, term: &mut TestTerm) {
    wait(Duration::from_secs(8), || {
        if !team.panes().iter().any(|p| p["role"] == "menu") {
            return true;
        }
        term.send(b"\x1b");
        std::thread::sleep(Duration::from_millis(400));
        false
    });
    std::thread::sleep(Duration::from_millis(500));
}

/// The quick lists (a click on the model or the effort of a header) for the design review, at 140 × 40 and 80 × 24, in
/// French and in English: the model's list, the effort's, the question of a member at work, a write refused, the list
/// that slides left at the edge, and the list that opens above the text at the bottom. `GALLERY_OUT` or
/// `target/night/gallery/`.
#[test]
#[ignore = "by hand: the design review's gallery of the quick lists"]
fn gallery_quick_list() {
    let mut gallery = Gallery { dir: out_dir(), shots: Vec::new() };
    for lang in ["fr", "en"] {
        let mut team = gallery_team_in(if lang == "fr" { "fr" } else { "en" }, true);
        for (size, (cols, rows), light) in quick_sizes() {
            let mut term = team.attach(cols, rows, Profile { light, ..Profile::default() });
            let keys = alt(&term, '2');
            term.send(&keys);
            assert!(
                wait(Duration::from_secs(5), || find_in_header(&term, "work").is_some()),
                "{}",
                term.screen().join("\n")
            );
            // The first model in a header is `work`'s: Opus.
            let theme = if light { "-light" } else { "" };
            let name = |what: &str| format!("40-quick-{lang}-{size}{theme}-{what}");
            // Too narrow for the model in a header of the grid (80 columns): `work` zoomed, as one would to read it.
            let mut zoomed = false;
            if find_in_header(&term, "Opus").is_none() {
                let keys = alt(&term, 'z');
                term.send(&keys);
                zoomed = wait(Duration::from_secs(3), || find_in_header(&term, "Opus").is_some());
            }
            let model = find_in_header(&term, "Opus");
            let effort = find_in_header(&term, "extra").or_else(|| find_in_header(&term, "\u{25c9}"));
            match model {
                Some(at) => {
                    click(&mut term, at);
                    wait_menu(&team);
                    gallery.shot(
                        &team,
                        &term,
                        &name("model"),
                        "the model's list under `work`'s header (Opus in place)",
                    );
                    // The pointer over a value: the cursor goes to it.
                    if let Some((col, row)) = find(&term, "Haiku") {
                        hover(&mut term, (col + 1, row));
                        std::thread::sleep(Duration::from_millis(500));
                        gallery.shot(&team, &term, &name("hover"), "the pointer over « Haiku »: the cursor follows it");
                    }
                    close_menu(&team, &mut term);
                    click(&mut term, at);
                    wait_menu(&team);
                    // At work, back to the default starts it again: the question opens in the list.
                    term.send(b"\x1b[A");
                    std::thread::sleep(Duration::from_millis(300));
                    term.send(b"\r");
                    std::thread::sleep(Duration::from_millis(900));
                    gallery.shot(
                        &team,
                        &term,
                        &name("ask"),
                        "`work` is at work and the default starts it again: the question, on Cancel",
                    );
                    close_menu(&team, &mut term);
                }
                None => gallery.shots.push(format!("{}: no model in `work`'s header at this size", name("model"))),
            }
            match effort {
                Some(at) => {
                    click(&mut term, at);
                    wait_menu(&team);
                    gallery.shot(&team, &term, &name("effort"), "the effort's list under `work`'s header");
                    close_menu(&team, &mut term);
                }
                None => gallery.shots.push(format!("{}: no effort in `work`'s header at this size", name("effort"))),
            }
            // The right column's header: the list slides left to stay on the screen. Zoomed, the one pane's header.
            let right = find_in_header(&term, "Sonnet").or(if zoomed { model } else { None });
            if let Some(at) = right {
                click(&mut term, at);
                wait_menu(&team);
                if !zoomed {
                    gallery.shot(
                        &team,
                        &term,
                        &name("edge"),
                        "`waits`' model, in the right column: the list slides left",
                    );
                }
                // A write refused: the list stays with the reason.
                let dir = team.proj.join(".recruit");
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).expect("read-only");
                }
                term.send(b"\x1b[B");
                term.send(b"\r");
                std::thread::sleep(Duration::from_millis(1200));
                gallery.shot(&team, &term, &name("refused"), "a write refused: the list stays, with the reason");
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("writable");
                }
                close_menu(&team, &mut term);
            } else {
                gallery.shots.push(format!("{}: no Sonnet in a header at this size", name("edge")));
            }
            if zoomed {
                let keys = alt(&term, 'z');
                term.send(&keys);
                // The server keeps the zoom for the next client: it must have heard the key.
                std::thread::sleep(Duration::from_millis(800));
            }
            drop(term);
        }
        team.stop();
        // A column of five panes: the header of the bottom one has no room below.
        let tables = "[teams.galq]\ndescription = \"gallery\"\ncolumns = 1\nrows = 5\n[teams.galq.members.lead]\nrole = \"Contact\"\ncontact = true\n".to_string()
            + &(1..=5).map(|i| format!("[teams.galq.members.a{i}]\nrole = \"Agent\"\n")).collect::<String>();
        let mut team = Team::in_lang("galq", &tables, if lang == "fr" { "fr" } else { "en" });
        team.launch(&["lead", "a1", "a2", "a3", "a4", "a5"]);
        for member in ["lead", "a1", "a2", "a3", "a4", "a5"] {
            report(&team, member, "claude-opus-5-5", "high", 40.0);
            says(&team, member, "idle");
        }
        std::thread::sleep(Duration::from_secs(3));
        for (size, (cols, rows), light) in quick_sizes() {
            let mut term = team.attach(cols, rows, Profile { light, ..Profile::default() });
            let keys = alt(&term, '2');
            term.send(&keys);
            wait(Duration::from_secs(5), || find_in_header(&term, "a5").is_some());
            let screen = term.screen();
            let bottom = screen.iter().rposition(|r| r.contains("Opus") && r.contains(" a5 "));
            match bottom {
                Some(row) => {
                    let col = unicode_width::UnicodeWidthStr::width(&screen[row][..screen[row].find("Opus").unwrap()]);
                    click(&mut term, (col, row));
                    wait_menu(&team);
                    gallery.shot(
                        &team,
                        &term,
                        &format!("40-quick-{lang}-{size}{}-bottom", if light { "-light" } else { "" }),
                        "`a5`'s model, the bottom header of a column of five: the list opens above the text",
                    );
                    close_menu(&team, &mut term);
                }
                None => {
                    gallery.shots.push(format!("40-quick-{lang}-{size}-bottom: no Opus on a5's header (light {light})"))
                }
            }
            drop(term);
        }
        team.stop();
    }
    std::fs::write(gallery.dir.join("INDEX-quick.txt"), gallery.shots.join("\n") + "\n").expect("index");
}

/// The sizes and themes of the quick lists' gallery: 140 × 40 and 80 × 24, dark then light.
fn quick_sizes() -> Vec<(&'static str, (u16, u16), bool)> {
    [false, true].into_iter().flat_map(|light| [("140x40", (140, 40), light), ("80x24", (80, 24), light)]).collect()
}

/// A report with more in it (`extra`'s keys, over the usual ones): a compaction made, the agents a session started.
fn report_extra(team: &Team, member: &str, model: &str, effort: &str, percent: f64, extra: serde_json::Value) {
    let dir = team.state().join("members");
    std::fs::create_dir_all(&dir).expect("members folder");
    let at = now_secs();
    let mut report = serde_json::json!({
        "session": "s", "model": model, "own": {"model": model, "effort": effort},
        "context": {"percent": percent, "tokens": (percent * 2000.0) as u64, "window": 200000}, "at": at,
    });
    for (key, value) in extra.as_object().expect("an object") {
        report[key] = value.clone();
    }
    std::fs::write(dir.join(format!("{member}.json")), report.to_string()).expect("report");
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The compaction traces and the agents a session started, as the mod would report them, on the design's team.
/// `many`: more teammates and subagents than the dashboard has room for.
fn lot_reports(team: &Team, many: bool, base: i64) {
    let now = now_secs();
    let made = |id: &str, auto: bool, percent: u64, after: u64, ago: i64| serde_json::json!({"id": id, "done": true, "auto": auto, "percent": percent, "after": after, "at": base - ago});
    report_extra(
        team,
        "work",
        "claude-opus-5-5",
        "xhigh",
        40.0,
        serde_json::json!({
            "last_compacted": made("c1", true, 74, 9, 300),
            "helpers": lot_helpers(now, many),
        }),
    );
    report_extra(
        team,
        "rests",
        "claude-haiku-5-5",
        "low",
        12.0,
        serde_json::json!({ "last_compacted": made("typed-1", false, 91, 12, 700) }),
    );
    report_extra(
        team,
        "waits",
        "claude-sonnet-5-5",
        "high",
        81.0,
        serde_json::json!({"compaction": {
            "id": "c2", "auto": true, "done": false, "percent": 81, "at": base - 120,
            "error": "API surchargée (529)",
        }}),
    );
    let teammates: Vec<serde_json::Value> = if many {
        ["ux-critic", "perf", "a11y", "i18n", "docs", "release"]
            .iter()
            .enumerate()
            .map(|(i, n)| {
                helper(
                    &format!("t{i}"),
                    "general-purpose",
                    n,
                    Some(n),
                    if i == 5 {
                        "failed"
                    } else if i % 2 == 0 {
                        "running"
                    } else {
                        "idle"
                    },
                    now - 900,
                    now - 60 * (i as i64 + 1),
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    if many {
        report_extra(team, "pm", "claude-opus-5-5", "medium", 30.0, serde_json::json!({ "helpers": teammates }));
    }
}

fn helper(
    id: &str,
    kind: &str,
    description: &str,
    teammate: Option<&str>,
    status: &str,
    started: i64,
    since: i64,
) -> serde_json::Value {
    let mut h = serde_json::json!({
        "id": id, "type": kind, "description": description, "status": status, "started": started, "since": since,
    });
    if let Some(t) = teammate {
        h["teammate"] = serde_json::json!(t);
    }
    h
}

/// What `work` started: subagents (two at work, one failed a moment ago) and teammates (one at work, one waiting for a
/// message, one failed); more with `many`.
fn lot_helpers(now: i64, many: bool) -> Vec<serde_json::Value> {
    let mut list = vec![
        helper("s1", "Explore", "Map the layout code", None, "running", now - 95, now - 95),
        helper("s2", "general-purpose", "Review the diff of the grid", None, "running", now - 40, now - 40),
        helper("s3", "Plan", "Draft the focus layout", None, "failed", now - 70, now - 4),
        helper("t1", "general-purpose", "UX critique", Some("ux-critic"), "running", now - 900, now - 130),
        helper("t2", "general-purpose", "Performance review", Some("perf"), "idle", now - 900, now - 400),
        helper("t3", "general-purpose", "Accessibility review", Some("a11y"), "failed", now - 900, now - 20),
    ];
    if many {
        for (i, kind) in ["Explore", "Explore", "general-purpose", "Plan"].iter().enumerate() {
            list.push(helper(
                &format!("m{i}"),
                kind,
                &format!("Task number {}", i + 4),
                None,
                "running",
                now - 20 * (i as i64 + 1),
                now - 20 * (i as i64 + 1),
            ));
        }
        for (i, name) in ["i18n", "docs", "release"].iter().enumerate() {
            list.push(helper(
                &format!("tm{i}"),
                "general-purpose",
                name,
                Some(name),
                "running",
                now - 900,
                now - 50 * (i as i64 + 1),
            ));
        }
    }
    list
}

/// ⌥`key`, as the client's terminal sends it, then a moment for the server.
fn press(term: &mut TestTerm, key: char) {
    let keys = alt(term, key);
    term.send(&keys);
    std::thread::sleep(Duration::from_millis(500));
}

/// The compaction traces (journal, ⟳ in the curve) and the sections of the agents a session started (2a), on the first
/// tab of the design's team, at 140 × 40 and at 80 × 24, in French and English, dark; light (140 × 40) to keep a trace.
/// `GALLERY_OUT` or `target/night/gallery/`.
#[test]
#[ignore = "by hand: the design review's gallery of the compaction trace and the agents' cards"]
fn gallery_trace_and_helpers() {
    let mut gallery = Gallery { dir: out_dir(), shots: Vec::new() };
    for lang in ["fr", "en"] {
        let mut team = gallery_team_in(lang, false);
        let base = now_secs();
        lot_reports(&team, false, base);
        for (size, (cols, rows), light) in
            [("140x40", (140u16, 40u16), false), ("80x24", (80, 24), false), ("140x40", (140, 40), true)]
        {
            let theme = if light { "-light" } else { "" };
            let mut term = team.attach(cols, rows, Profile { light, ..Profile::default() });
            assert!(term.wait_for("lead", Duration::from_secs(5)));
            press(&mut term, '1');
            // The traces and the agents are read by the journal and the dashboard within a few seconds.
            std::thread::sleep(Duration::from_secs(4));
            lot_reports(&team, false, base);
            std::thread::sleep(Duration::from_secs(3));
            gallery.shot(
                &team,
                &term,
                &format!("50-trace-{lang}-{size}{theme}"),
                "tab 1: the journal's compaction lines (auto, by hand, failed in orange), the ⟳ in the curve of the cards, \
                 and under the members the sections of subagents and teammates of `work`",
            );
            // The journal full: ⌥j twice (hidden, then full).
            press(&mut term, 'j');
            press(&mut term, 'j');
            std::thread::sleep(Duration::from_secs(1));
            gallery.shot(
                &team,
                &term,
                &format!("51-journal-full-{lang}-{size}{theme}"),
                "the journal full: the compaction lines among the messages",
            );
            press(&mut term, 'j');
            drop(term);
        }
        // More agents than room: the sections give up their shapes in turn.
        lot_reports(&team, true, base);
        for (size, (cols, rows)) in [("140x40", (140u16, 40u16)), ("100x31", (100, 31)), ("80x24", (80, 24))] {
            let mut term = team.attach(cols, rows, Profile::default());
            assert!(term.wait_for("lead", Duration::from_secs(5)));
            press(&mut term, '1');
            std::thread::sleep(Duration::from_secs(1));
            lot_reports(&team, true, base);
            std::thread::sleep(Duration::from_secs(4));
            gallery.shot(
                &team,
                &term,
                &format!("52-helpers-many-{lang}-{size}"),
                "tab 1: many subagents and teammates: the sections give up their shapes for lack of room",
            );
            drop(term);
        }
        team.stop();
    }
    std::fs::write(gallery.dir.join("INDEX-trace.txt"), gallery.shots.join("\n") + "\n").expect("index");
}

/// The team rows of the menu (« Disposition », « Grille », « Compaction auto »), their lists, and their short forms in
/// a low window: 140 × 40 and 80 × 24, French and English, dark.
#[test]
#[ignore = "by hand: the design review's gallery of the menu's team rows"]
fn gallery_menu_rows() {
    let mut gallery = Gallery { dir: out_dir(), shots: Vec::new() };
    for lang in ["fr", "en"] {
        let mut team = gallery_team_in(lang, false);
        for (size, (cols, rows), light) in [
            ("140x40", (140u16, 40u16), false),
            ("100x31", (100, 31), false),
            ("80x24", (80, 24), false),
            ("140x40", (140, 40), true),
        ] {
            let theme = if light { "-light" } else { "" };
            let mut term = team.attach(cols, rows, Profile { light, ..Profile::default() });
            assert!(term.wait_for("lead", Duration::from_secs(5)));
            press(&mut term, '1');
            press(&mut term, 'r');
            wait_menu(&team);
            gallery.shot(
                &team,
                &term,
                &format!("60-menu-{lang}-{size}{theme}"),
                "the menu on the first member's sheet: the team's rows",
            );
            for (key, what, name) in [
                ('g', "the grid's list (columns × rows)", "grid"),
                ('c', "the automatic compaction's list (off, 50 to 90 %)", "compaction"),
            ] {
                term.send(key.to_string().as_bytes());
                std::thread::sleep(Duration::from_millis(900));
                gallery.shot(&team, &term, &format!("61-{name}-{lang}-{size}{theme}"), what);
                term.send(b"\x1b");
                std::thread::sleep(Duration::from_millis(600));
            }
            // « Layout »: the key switches the tabs to « focus » and back; what the menu says of it each time.
            term.send(b"l");
            std::thread::sleep(Duration::from_millis(1200));
            gallery.shot(
                &team,
                &term,
                &format!("62-layout-focus-{lang}-{size}{theme}"),
                "after `l`: the layout row says focus",
            );
            term.send(b"l");
            std::thread::sleep(Duration::from_millis(1200));
            gallery.shot(
                &team,
                &term,
                &format!("63-layout-grid-{lang}-{size}{theme}"),
                "after `l` again: the layout row says grid",
            );
            close_menu(&team, &mut term);
            drop(term);
        }
        team.stop();
    }
    std::fs::write(gallery.dir.join("INDEX-menu.txt"), gallery.shots.join("\n") + "\n").expect("index");
}

/// The tabs in « focus » (`layout = "focus"`): 140 × 40 and 100 × 31, French and English, dark; the cards « others », a
/// member that enters (one of the cards waits), the bar's button « grid (⌥l) » and « focus (⌥l) ».
#[test]
#[ignore = "by hand: the design review's gallery of the focus layout"]
fn gallery_focus() {
    let mut gallery = Gallery { dir: out_dir(), shots: Vec::new() };
    for lang in ["fr", "en"] {
        let tables = "[teams.galf]\ndescription = \"gallery\"\nlayout = \"focus\"\n[teams.galf.members.lead]\nrole = \"Contact\"\ncontact = true\n".to_string()
            + &(1..=6).map(|i| format!("[teams.galf.members.a{i}]\nrole = \"Agent\"\ntab = \"crew\"\n")).collect::<String>();
        let mut team = Team::in_lang("galf", &tables, if lang == "fr" { "fr" } else { "en" });
        let members = ["lead", "a1", "a2", "a3", "a4", "a5", "a6"];
        team.launch(&members);
        let models = [
            ("lead", "claude-fable-5-1", "max", 12.0),
            ("a1", "claude-opus-5-5", "xhigh", 40.0),
            ("a2", "claude-sonnet-5-5", "high", 62.0),
            ("a3", "claude-opus-5-5", "medium", 55.0),
            ("a4", "claude-haiku-5-5", "low", 91.0),
            ("a5", "claude-sonnet-5-5", "medium", 20.0),
            ("a6", "claude-haiku-5-5", "low", 5.0),
        ];
        for (member, model, effort, percent) in models {
            report(&team, member, model, effort, percent);
        }
        for (member, state) in [
            ("lead", "idle"),
            ("a1", "working"),
            ("a2", "blocked:kind=permission"),
            ("a3", "working"),
            ("a4", "idle"),
            ("a5", "done"),
            ("a6", "idle"),
        ] {
            says(&team, member, state);
        }
        std::thread::sleep(Duration::from_secs(4));
        for (size, (cols, rows), light) in
            [("140x40", (140u16, 40u16), false), ("100x31", (100, 31), false), ("140x40", (140, 40), true)]
        {
            let theme = if light { "-light" } else { "" };
            let mut term = team.attach(cols, rows, Profile { light, ..Profile::default() });
            // The server kept the tab the last client was on: the bar, with the team's name, is there on any.
            assert!(term.wait_for("galf", Duration::from_secs(8)), "{}", term.screen().join("\n"));
            press(&mut term, '2');
            std::thread::sleep(Duration::from_secs(2));
            gallery.shot(
                &team,
                &term,
                &format!("70-focus-{lang}-{size}{theme}"),
                "the tab in focus: the active member on the left, the best placed on the right, the others as cards",
            );
            if !light {
                // A card's member waits: it enters at once, in place of the least well placed.
                says(&team, "a6", "blocked:kind=permission");
                std::thread::sleep(Duration::from_millis(1800));
                gallery.shot(
                    &team,
                    &term,
                    &format!("71-focus-enters-{lang}-{size}"),
                    "`a6`, a card, waits: it enters at once",
                );
                std::thread::sleep(Duration::from_secs(6));
                gallery.shot(
                    &team,
                    &term,
                    &format!("71b-focus-enters-later-{lang}-{size}"),
                    "`a6` still waits, six seconds later",
                );
            }
            says(&team, "a6", "idle");
            // The bar's button: ⌥l brings the grid; its label says what it brings back.
            press(&mut term, 'l');
            std::thread::sleep(Duration::from_secs(1));
            gallery.shot(
                &team,
                &term,
                &format!("72-focus-to-grid-{lang}-{size}{theme}"),
                "after ⌥l: the grid, the button says « focus (⌥l) »",
            );
            press(&mut term, 'l');
            std::thread::sleep(Duration::from_secs(1));
            drop(term);
        }
        team.stop();
    }
    std::fs::write(gallery.dir.join("INDEX-focus.txt"), gallery.shots.join("\n") + "\n").expect("index");
}

/// The dashboard with room for the rich form of its sections (2a): few members, two subagents and three teammates (one
/// at work, one waiting for a message, one failed), at 140 × 40, and lower (140 × 30, 140 × 24) where the cards
/// give way to cards side by side; French and English, dark, then light at 140 × 40.
#[test]
#[ignore = "by hand: the design review's gallery of the dashboard's sections"]
fn gallery_dashboard_rich() {
    let mut gallery = Gallery { dir: out_dir(), shots: Vec::new() };
    for lang in ["fr", "en"] {
        let tables = "[teams.gald]\ndescription = \"gallery\"\n[teams.gald.members.lead]\nrole = \"Contact\"\ncontact = true\n\
            [teams.gald.members.work]\nrole = \"Agent\"\n[teams.gald.members.waits]\nrole = \"Agent\"\n\
            [teams.gald.members.rests]\nrole = \"Agent\"\n";
        let mut team = Team::in_lang("gald", tables, if lang == "fr" { "fr" } else { "en" });
        let members = ["lead", "work", "waits", "rests"];
        team.launch(&members);
        for (member, state) in
            [("lead", "idle"), ("work", "working"), ("waits", "blocked:kind=permission"), ("rests", "idle")]
        {
            says(&team, member, state);
        }
        let write = |team: &Team| {
            let now = now_secs();
            let helpers = vec![
                helper("s1", "Explore", "Map the layout code", None, "running", now - 95, now - 95),
                helper("s2", "general-purpose", "Review the diff of the grid", None, "running", now - 40, now - 40),
                helper("t1", "general-purpose", "UX critique", Some("ux-critic"), "running", now - 900, now - 130),
                helper("t2", "general-purpose", "Performance review", Some("perf"), "idle", now - 900, now - 400),
                helper("t3", "general-purpose", "Accessibility review", Some("a11y"), "failed", now - 900, now - 20),
            ];
            report_extra(team, "lead", "claude-fable-5-1", "max", 12.0, serde_json::json!({}));
            report_extra(team, "work", "claude-opus-5-5", "xhigh", 40.0, serde_json::json!({ "helpers": helpers }));
            report_extra(team, "waits", "claude-sonnet-5-5", "high", 62.0, serde_json::json!({}));
            report_extra(team, "rests", "claude-haiku-5-5", "low", 31.0, serde_json::json!({}));
        };
        write(&team);
        std::thread::sleep(Duration::from_secs(3));
        for (size, (cols, rows), light) in [
            ("140x40", (140u16, 40u16), false),
            ("140x34", (140, 34), false),
            ("140x30", (140, 30), false),
            ("140x26", (140, 26), false),
            ("140x40", (140, 40), true),
        ] {
            let theme = if light { "-light" } else { "" };
            let mut term = team.attach(cols, rows, Profile { light, ..Profile::default() });
            assert!(term.wait_for("gald", Duration::from_secs(8)), "{}", term.screen().join("\n"));
            press(&mut term, '1');
            // The journal hidden (⌥j, until its pane is gone): the dashboard has the whole column.
            for _ in 0..3 {
                if !team.panes().iter().any(|p| p["role"] == "journal") {
                    break;
                }
                press(&mut term, 'j');
            }
            write(&team);
            std::thread::sleep(Duration::from_secs(4));
            write(&team);
            std::thread::sleep(Duration::from_secs(2));
            gallery.shot(
                &team,
                &term,
                &format!("80-dashboard-rich-{lang}-{size}{theme}"),
                "the dashboard with few members, the journal hidden: the rules « ⧗ subagents · 2 » and « ♙ teammates · 3 », their cards",
            );
            drop(term);
        }
        team.stop();
    }
    std::fs::write(gallery.dir.join("INDEX-dashboard.txt"), gallery.shots.join("\n") + "\n").expect("index");
}

/// The header of a member at many widths, to see the whole reduction (model initial, effort sign and name, time,
/// context, zoom sign): the first agent's pane zoomed, 140 down to 20 columns; French and English, dark and light.
/// Also the member's tab at 140 × 40.
#[test]
#[ignore = "by hand: the design review's gallery of the header's reduction"]
fn gallery_header_widths() {
    let mut gallery = Gallery { dir: out_dir(), shots: Vec::new() };
    for lang in ["fr", "en"] {
        let mut team = gallery_team_in(lang, false);
        for light in [false, true] {
            let theme = if light { "-light" } else { "" };
            let mut term = team.attach(140, 40, Profile { light, ..Profile::default() });
            assert!(term.wait_for("gal", Duration::from_secs(8)), "{}", term.screen().join("\n"));
            press(&mut term, '2');
            wait(Duration::from_secs(3), || find_in_header(&term, "work").is_some());
            gallery.shot(
                &team,
                &term,
                &format!("90-tab-work-{lang}-140x40{theme}"),
                "tab 2: the four agents (at work, waiting, at rest, done) with the new signs",
            );
            press(&mut term, 'z');
            for cols in [140u16, 100, 80, 72, 64, 56, 52, 46, 40, 34, 30, 26, 20] {
                term.resize(cols, 24);
                wait(Duration::from_secs(2), || term.size().0 == cols as usize);
                std::thread::sleep(Duration::from_millis(400));
                gallery.shot(
                    &team,
                    &term,
                    &format!("91-header-{lang}{theme}-{cols:03}"),
                    &format!("the zoomed header of `work` at {cols} columns"),
                );
            }
            press(&mut term, 'z');
            drop(term);
        }
        team.stop();
    }
    std::fs::write(gallery.dir.join("INDEX-header.txt"), gallery.shots.join("\n") + "\n").expect("index");
}
