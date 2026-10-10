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
        std::fs::write(self.dir.join(format!("{name}.json")), term.styled()).expect("json");
        std::fs::write(self.dir.join(format!("{name}.txt")), format!("{what}\n\n{}\n", term.screen().join("\n")))
            .expect("txt");
        let dashboard = team.panes().iter().any(|p| p["role"] == "dashboard");
        println!("shot {name}: {what} (dashboard pane alive: {dashboard})");
        self.shots.push(format!("{name}: {what}"));
    }
}

/// The design's team: two contacts, four agents in « work » (at work, waiting, at rest, done), two in « qa ».
fn gallery_team() -> Team {
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
    let team = Team::new("gal", tables);
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
    if let Some(at) = find_in_header(&term, "xhigh").or_else(|| find_in_header(&term, "\u{2588}")) {
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
