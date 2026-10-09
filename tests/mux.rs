// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The native multiplexer's integration tests without a window (spec §9): a server of the test's own
//! (`common::server`), a fake program in its pane (`common::fakes`), a real client on the test's terminal
//! (`common::pty`); what reaches that terminal is checked. Part of `cargo test`, and so of the CI.
//!
//! Owner: testeur.

mod common;

use std::path::Path;
use std::time::Duration;

use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use common::fakes;
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

/// A server of the test's own in `dir`, a real client attached on a test terminal of `cols` × `rows` (answering
/// XTVERSION with `xtversion`, every byte kept), then a pane running the fake `role`, at the client's size.
fn served(
    dir: &Path,
    role: &str,
    vars: &[(&str, String)],
    cols: u16,
    rows: u16,
    xtversion: Option<&'static str>,
) -> (Server, TestTerm) {
    let server = Server::start(dir);
    let term = server.attach_as(cols, rows, Profile { capture: true, xtversion, ..Profile::default() });
    assert!(wait_attached(&server), "the client never attached: {}", server.clients());
    server.spawn_fake("alpha", role, vars);
    (server, term)
}

/// Waits until the server counts a client, 10 s at most.
fn wait_attached(server: &Server) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if server.clients()["clients"].as_array().is_some_and(|c| !c.is_empty()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// What a pane hands on to the real terminal reaches it (spec §5.6): the clipboard (OSC 52), links (OSC 8),
/// notifications (OSC 9, 777, 99, BEL), progress (OSC 9;4). The title is checked to be either relayed or kept.
#[test]
fn relays_reach_the_terminal() {
    relays(None);
}

/// The same, on a terminal that says it is Ghostty.
#[test]
fn relays_reach_ghostty() {
    relays(Some("ghostty 1.3.1"));
}

fn relays(xtversion: Option<&'static str>) {
    let dir = tempfile::tempdir().expect("temp dir");
    let (mut server, term) = served(dir.path(), "relay", &[], 120, 30, xtversion);
    assert!(term.wait_for("RELAY DONE", Duration::from_secs(10)), "{:#?}", term.screen());
    std::thread::sleep(Duration::from_millis(300));
    let raw = String::from_utf8_lossy(&term.raw()).into_owned();
    let mut missing = Vec::new();
    for (what, needle) in [
        ("OSC 52", fakes::OSC52.trim_end_matches('\x07')),
        ("OSC 8", &format!("\x1b]8;;{}", fakes::LINK_URL)[..]),
        ("OSC 9", "\x1b]9;recruit-osc9"),
        // The engine hands every notification on as OSC 9 ("title: body").
        ("OSC 777 (en OSC 9)", "\x1b]9;recruit: osc777"),
        ("OSC 99 (en OSC 9)", "\x1b]9;recruit-osc99"),
        ("OSC 9;4", "\x1b]9;4;1;42"),
    ] {
        // A terminal that does not say what it is gets no links: their text stays (checked below).
        if what == "OSC 8" && xtversion.is_none() {
            continue;
        }
        if !raw.contains(needle) {
            missing.push(what);
        }
    }
    for word in ["osc777", "osc99", "recruit-osc9"] {
        let shown: Vec<String> = raw
            .match_indices(word)
            .map(|(i, _)| raw[i.saturating_sub(24)..i + word.len()].escape_debug().to_string())
            .collect();
        println!("{word} dans la sortie : {shown:?}");
    }
    let bell = raw.contains('\x07') && !raw.replace(fakes::OSC52, "").is_empty();
    println!(
        "{xtversion:?} : relais absents : {missing:?} ; BEL vu : {bell} ; titre relayé : {}",
        raw.contains("recruit-title")
    );
    assert!(term.screen().iter().any(|r| r.contains("le lien")), "the link's text is gone: {:#?}", term.screen());
    drop(term);
    server.stop();
    assert!(missing.is_empty(), "not relayed: {missing:?}");
}

/// A pane in the middle of a synchronized update is never drawn half-way (spec §5.3): every frame the
/// multiplexer sends is played on a terminal of its own, frame by frame, and no frame may show two of the fake's
/// letters at once.
#[test]
fn no_partial_frame() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (cols, rows) = (100u16, 30u16);
    let rounds = 20;
    let (mut server, term) = served(dir.path(), "sync", &[("SYNC_ROUNDS", rounds.to_string())], cols, rows, None);
    assert!(term.wait_for("SYNC DONE", Duration::from_secs(30)), "{:#?}", term.screen());
    let raw = term.raw();
    drop(term);
    server.stop();

    // Replay, cut after each end of synchronized update (the multiplexer's frames), or after each read when it
    // sends none.
    let end = b"\x1b[?2026l";
    let mut replay = Term::new(Config::default(), &TermSize::new(cols as usize, rows as usize), common_listener());
    let mut parser: Processor = Processor::new();
    let (mut frames, mut partial, mut seen) = (0, 0, std::collections::BTreeSet::new());
    let mut start = 0;
    let mut i = 0;
    while i + end.len() <= raw.len() {
        if &raw[i..i + end.len()] == end {
            parser.advance(&mut replay, &raw[start..i + end.len()]);
            start = i + end.len();
            frames += 1;
            let letters = letters_on(&replay);
            seen.extend(letters.iter().copied());
            if letters.len() > 1 {
                partial += 1;
                if partial <= 3 {
                    println!("Image {frames} partielle : {letters:?}");
                }
            }
            i = start;
        } else {
            i += 1;
        }
    }
    println!("{frames} images, {partial} partielles, {} lettres vues sur {rounds}", seen.len());
    assert!(frames > 0, "no synchronized frame at all");
    assert_eq!(partial, 0, "{partial} partial frames out of {frames}");
}

/// The fake's letters that show on `term`'s screen.
fn letters_on<T>(term: &Term<T>) -> std::collections::BTreeSet<char> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    let grid = term.grid();
    let mut found = std::collections::BTreeSet::new();
    for line in 0..grid.screen_lines() {
        let row = &grid[Line(line as i32)];
        for col in 0..grid.columns() {
            let c = row[Column(col)].c;
            if fakes::SYNC_LETTERS.contains(c) {
                found.insert(c);
            }
        }
    }
    found
}

fn common_listener() -> alacritty_terminal::event::VoidListener {
    alacritty_terminal::event::VoidListener
}
