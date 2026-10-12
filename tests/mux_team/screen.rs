// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

/// The parts of a member's header that show on `row`: the name, the context, the sign of the effort, the effort's
/// name, the model's whole name, the time in the state, the zoom sign, the model's initial.
const NAME: usize = 0;

const CONTEXT: usize = 1;

const SIGN: usize = 2;

const WORD: usize = 3;

const MODEL: usize = 4;

const TIME: usize = 5;

const INITIAL: usize = 7;

// (index 6 is the zoom sign, kept apart: no constant, nothing reads it by name)

fn header_parts(row: &str) -> [bool; 8] {
    let has_time = row.split_whitespace().any(|w| {
        w.len() >= 2
            && w.ends_with(['s', 'm', 'h'])
            && w[..w.len() - 1].chars().all(|c| c.is_ascii_digit() || c == 'h' || c == 'm')
    });
    [
        row.contains(" lead"),
        row.contains('%'),
        row.contains('\u{25c9}'),
        row.contains("extra"),
        row.contains("Opus"),
        has_time,
        row.contains('\u{2922}'),
        // The model reduced to its initial, a word of its own between the state's sign, the name and the rest.
        row.split_whitespace().any(|w| w == "O"),
    ]
}

/// The parts that give up one after the other as the header narrows, in the order they stay: the name, the context, the
/// effort's sign, the model (its whole name or its initial), the time, the model's whole name, the effort's name.
/// Each implies the ones before it (the zoom sign is kept apart).
fn header_chain(p: &[bool; 8]) -> [bool; 7] {
    [p[NAME], p[CONTEXT], p[SIGN], p[MODEL] || p[INITIAL], p[TIME], p[MODEL], p[WORD]]
}

/// F1: a member's header says its state, name, model, effort, context, time in the state and the zoom sign, and
/// gives up what does not fit in the planned order: the effort's name, the model's name (its initial stays), the time,
/// the initial, the effort's sign; the context last, then the name is cut. The six forms of the decision: the whole
/// header, without the effort's name, the model as its initial (before the time goes), without the time, without the
/// initial (before the sign goes), without the sign.
#[test]
fn header_gives_up_in_order() {
    let tables = tables("head", &["lead"], &[], "dashboard = false");
    let mut team = Team::new("head", &tables);
    team.launch(&["lead"]);
    report(&team, "lead", "claude-opus-5-5", "xhigh", 85.0);
    says(&team, "lead", "idle");
    let mut term = client(&team, "lead");
    // The data is read by the server every two seconds.
    let header = |term: &TestTerm| term.screen().first().cloned().unwrap_or_default();
    assert!(
        wait(Duration::from_secs(6), || header(&term).contains("Opus")),
        "no model in the header: {}",
        header(&term)
    );
    let mut checks = Checks::default();
    let full = header(&term);
    let parts = header_parts(&full);
    checks.check("F1 everything fits at 120 columns", parts[..7].iter().all(|p| *p) && !parts[INITIAL], || {
        format!("{parts:?} {full}")
    });
    checks.check(
        "F1 the state's sign before the name",
        full.contains("\u{25f7} lead") || full.contains("\u{25cc} lead"),
        || full.clone(),
    );
    // Narrower and narrower: what stays at each width implies what is less urgent.
    // name ≥ context ≥ sign ≥ time ≥ model ≥ word (the zoom sign is kept apart).
    let mut seen_all = false;
    let mut last: Option<[bool; 8]> = None;
    // The forms met on the way down: the model whole and the effort named, then without the effort's name, then the
    // model as its initial with the time, then without the time, then without the initial, then without the sign.
    let mut forms = std::collections::BTreeSet::new();
    for cols in (14..=120u16).rev().step_by(2) {
        term.resize(cols, 20);
        // Drawn again for the new size: the frame's corner at the last column.
        let drawn = wait(Duration::from_secs(2), || {
            let row = header(&term);
            term.size().0 == cols as usize && row.chars().count() == cols as usize && row.ends_with(['┓', '╮'])
        });
        if !drawn {
            continue;
        }
        let row = header(&term);
        let p = header_parts(&row);
        let chain = header_chain(&p);
        let ordered = chain.windows(2).all(|w| w[0] || !w[1]);
        if p[NAME] && p[CONTEXT] {
            forms.insert(match (p[SIGN], p[MODEL], p[INITIAL], p[TIME], p[WORD]) {
                (true, true, _, true, true) => "whole",
                (true, true, _, true, false) => "no effort name",
                (true, false, true, true, _) => "initial, with the time",
                (true, false, true, false, _) => "initial, no time",
                (true, false, false, false, _) => "no model, sign kept",
                (false, false, false, false, _) => "no sign",
                _ => "other",
            });
        }
        checks.check(&format!("F1 order of giving up at {cols} columns"), ordered, || format!("{p:?} {row}"));
        if last.is_some_and(|before| before != p) {
            println!("{cols:3} columns: {p:?}");
        }
        seen_all |= p[..7].iter().all(|x| *x);
        last = Some(p);
    }
    checks.check("F1 the whole header at some width", seen_all, String::new);
    for form in
        ["whole", "no effort name", "initial, with the time", "initial, no time", "no model, sign kept", "no sign"]
    {
        checks.check(&format!("F1 the form « {form} » is met on the way down"), forms.contains(form), || {
            format!("{forms:?}")
        });
    }
    checks.check("F1 no header of another form", !forms.contains("other"), || format!("{forms:?}"));
    checks
        .check("F1 the name survives to the narrowest width tried", last.is_some_and(|p| p[0]), || format!("{last:?}"));
    checks.done();
    team.stop();
}

/// A client that is resized keeps what the tab shows: the dashboard and the journal beside the contacts, at 140 × 40,
/// at 80 × 24 and back.
#[test]
fn resizing_keeps_the_dashboard() {
    let mut team = Team::new("resize", &tables("resize", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let mut term = client(&team, "lead");
    let mut checks = Checks::default();
    for (cols, rows) in [(140u16, 40u16), (80, 24), (140, 40), (100, 30), (80, 24)] {
        term.resize(cols, rows);
        let drawn = wait(Duration::from_secs(3), || {
            term.size() == (cols as usize, rows as usize)
                && term.screen().last().is_some_and(|bar| bar.contains("quitter") || bar.contains("quit"))
        });
        let both = wait(Duration::from_secs(3), || {
            let shown = shown(&term);
            shown.contains("Tableau de bord") && shown.contains("Journal")
        });
        checks.check(&format!("the dashboard and the journal at {cols} x {rows}"), drawn && both, || shown(&term));
    }
    checks.done();
    team.stop();
}

/// The sequence that lost the dashboard in the gallery (case 14): zoom in the second tab, narrower and narrower, the
/// zoom undone, 80 × 24, then the first tab: the dashboard is there.
#[test]
fn resizing_a_zoomed_tab_keeps_the_dashboard() {
    let mut team = Team::new("rezoom", &tables("rezoom", &["lead"], &[("dev1", None), ("dev2", None)], ""));
    team.launch(&["lead", "dev1", "dev2"]);
    let mut term = client(&team, "lead");
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(wait(Duration::from_secs(3), || has_header(&term, "dev1")), "{}", shown(&term));
    let zoom = alt(&term, 'z');
    term.send(&zoom);
    for cols in [72u16, 52, 40, 34, 26, 20] {
        term.resize(cols, 24);
        wait(Duration::from_secs(2), || term.size().0 == cols as usize);
        std::thread::sleep(Duration::from_millis(100));
    }
    term.send(&zoom);
    term.resize(80, 24);
    wait(Duration::from_secs(2), || term.size() == (80, 24));
    let keys = alt(&term, '1');
    term.send(&keys);
    let there =
        wait(Duration::from_secs(4), || shown(&term).contains("Tableau de bord") && shown(&term).contains("Journal"));
    let panes = team.panes();
    assert!(there, "no dashboard in the first tab after the resizes\n{}\n{:?}", shown(&term), panes);
    team.stop();
}

/// The dashboard's program in a pane of any width: it draws what it can and does not end (a pane whose program ends
/// is closed, and the dashboard with it, for good).
#[test]
fn the_dashboard_survives_a_narrow_pane() {
    let mut team = Team::new("narrow", &tables("narrow", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    report(&team, "lead", "claude-opus-5-5", "xhigh", 85.0);
    says(&team, "lead", "idle");
    let state = team.state().to_string_lossy().to_string();
    let mut checks = Checks::default();
    // All the widths at once: each program is given a moment to draw, and to fail.
    let widths = [1u16, 5, 6, 7, 20];
    let mut terms: Vec<(u16, TestTerm)> = widths
        .iter()
        .map(|&cols| (cols, team.in_terminal(&["_panel", "dashboard", &state], cols, 24, Profile::default())))
        .collect();
    std::thread::sleep(Duration::from_millis(600));
    for (cols, term) in &mut terms {
        let status = term.wait_exit(Duration::from_millis(10));
        checks.check(&format!("the dashboard at {cols} columns"), status.is_none(), || {
            format!("ended {status:?}: {}", term.screen().join(" ").split_whitespace().collect::<Vec<_>>().join(" "))
        });
    }
    checks.done();
    drop(terms);
    team.stop();
}

/// F8 (and the badge of F5): on a narrow window the inactive tabs shrink to their number and their state, the buttons
/// to their key; the ⚑ of a member that waits stays as long as there is room for a tab at all.
#[test]
fn narrow_bar() {
    let mut team = Team::new("bar", &tables("bar", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let mut term = client(&team, "lead");
    says(&team, "dev", "blocked:kind=permission");
    let bar = |term: &TestTerm| term.screen().last().cloned().unwrap_or_default();
    let sign = if cfg!(target_os = "macos") { "\u{2325}" } else { "Alt+" };
    let mut checks = Checks::default();
    let at = |term: &mut TestTerm, cols: u16, rows: u16| {
        term.resize(cols, rows);
        wait(Duration::from_secs(3), || term.size() == (cols as usize, rows as usize));
        std::thread::sleep(Duration::from_millis(300));
    };
    at(&mut term, 100, 24);
    let full = wait(Duration::from_secs(3), || bar(&term).contains("2 Agents") && bar(&term).contains("menu ("));
    checks.check("F8 wide: tabs and buttons in full", full, || bar(&term));
    at(&mut term, 40, 24);
    let b = bar(&term);
    checks.check(
        "F8 narrow: no title of an inactive tab, no word on the buttons, their keys stay",
        !b.contains("Agents")
            && !b.contains("menu (")
            && b.contains(&format!("{sign}r"))
            && b.contains(&format!("{sign}q")),
        || b.clone(),
    );
    let badge = wait(Duration::from_secs(3), || bar(&term).contains("\u{2691}2") || bar(&term).contains("\u{2691} 2"));
    checks.check("F8 narrow: the badge of the waiting member stays", badge, || bar(&term));
    at(&mut term, 24, 24);
    let badge = wait(Duration::from_secs(3), || bar(&term).contains("\u{2691}2") || bar(&term).contains("\u{2691} 2"));
    checks.check("F8 24 columns: the badge stays", badge, || bar(&term));
    at(&mut term, 100, 24);
    let back = wait(Duration::from_secs(3), || bar(&term).contains("2 Agents") && bar(&term).contains("menu ("));
    checks.check("F8 wide again: in full", back, || bar(&term));
    checks.done();
    team.stop();
}

/// F5: a member out of view that passes to waiting shows a notice for six seconds, at the top right, then only the
/// badge stays; a click on the notice goes to the member.
#[test]
#[ignore = "8 s: the notice lasts six seconds; run with --ignored"]
fn notice_for_six_seconds() {
    let mut team = Team::new("notice", &tables("notice", &["lead"], &[("dev1", None), ("dev2", None)], ""));
    team.launch(&["lead", "dev1", "dev2"]);
    let mut term = client(&team, "lead");
    let mut checks = Checks::default();
    let text = "dev1 attend ta réponse";
    says(&team, "dev1", "blocked:kind=permission");
    let shown_at = std::time::Instant::now();
    let there = wait(Duration::from_secs(4), || shown(&term).contains(text));
    checks.check("F5 the notice appears", there, || shown(&term));
    let at = find(&term, text);
    checks.check("F5 at the top right", at.is_some_and(|(col, row)| row <= 4 && col > term.size().0 / 2), || {
        format!("{at:?} {}", shown(&term))
    });
    // Gone after about six seconds, the badge stays.
    let gone = wait(Duration::from_secs(9), || !shown(&term).contains(text));
    let lasted = shown_at.elapsed();
    checks.check(
        "F5 six seconds, give or take",
        gone && lasted >= Duration::from_secs(5) && lasted <= Duration::from_secs(8),
        || format!("{lasted:?}"),
    );
    checks.check("F5 the badge stays", term.screen().last().is_some_and(|b| b.contains('\u{2691}')), || shown(&term));
    // A click on the notice of dev2 goes to dev2.
    says(&team, "dev2", "blocked:kind=permission");
    let text = "dev2 attend ta réponse";
    let there = wait(Duration::from_secs(4), || shown(&term).contains(text));
    checks.check("F5 the next notice appears", there, || shown(&term));
    if let Some(at) = find(&term, text) {
        click(&mut term, at);
        let went = wait(Duration::from_secs(3), || active(&term, &["dev1", "dev2"]) == ["dev2"]);
        checks.check("F5 a click on the notice goes to the member", went, || shown(&term));
    }
    checks.done();
    team.stop();
}

/// F3: the pointer moving over the screen redraws it only when the element under it changes (name, model, tab,
/// button: lit); moves inside one element, or over the text of a pane, send no frame at all. Counted by the server
/// (`_ctl stats`: frames sent to the client).
#[test]
fn hover_redraws_only_on_change() {
    let mut team = Team::new("hover", &tables("hover", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    report(&team, "lead", "claude-opus-5-5", "xhigh", 85.0);
    says(&team, "lead", "idle");
    let mut term = client(&team, "lead");
    assert!(wait(Duration::from_secs(6), || find_in_header(&term, "Opus").is_some()), "{}", shown(&term));
    let frames = |team: &Team| {
        let stats: serde_json::Value = serde_json::from_str(&team.ctl(["stats", "--json"])).expect("stats");
        stats["frames"].as_u64().unwrap_or_default()
    };
    let motion = |term: &mut TestTerm, (col, row): (usize, usize)| {
        term.send(format!("\x1b[<35;{};{}M", col + 1, row + 1).as_bytes());
    };
    // The clocks (the seconds in the header and on the cards, a few phases a second) draw frames of their own, and on a
    // slow machine a second that turns lands in a run now and then (a CI run saw 3 frames against 0). A defect draws a
    // frame for the moves in every run; a clock only in some. So each measure is made several times, short, and what
    // counts is the quietest run: the moves are not to cost more than a frame or two over the quietest run of the
    // pointer standing still.
    const RUN: Duration = Duration::from_millis(250);
    const RUNS: usize = 8;
    const SLACK: u64 = 2;
    let measure_once = |term: &mut TestTerm, moves: &mut dyn FnMut(&mut TestTerm), span: Duration| {
        let before = frames(&team);
        let begun = std::time::Instant::now();
        moves(term);
        std::thread::sleep(span.saturating_sub(begun.elapsed()));
        frames(&team) - before
    };
    // The frames of each of `runs` runs.
    let measure = |term: &mut TestTerm, moves: &mut dyn FnMut(&mut TestTerm), span: Duration, runs: usize| {
        (0..runs).map(|_| measure_once(term, moves, span)).collect::<Vec<u64>>()
    };
    let quietest = |runs: &[u64]| runs.iter().copied().min().unwrap_or_default();
    let name = find_in_header(&term, " lead ").map(|(c, r)| (c + 1, r)).expect("name");
    let model = find_in_header(&term, "Opus").expect("model");
    let inside = (5, 10);
    let mut checks = Checks::default();
    let still = measure(&mut term, &mut |_| {}, RUN, RUNS);
    // Over the text of the pane: nothing to light.
    let idle_moves = measure(
        &mut term,
        &mut |term| {
            for i in 0..100 {
                motion(term, (inside.0 + i % 20, inside.1 + i % 5));
            }
        },
        RUN,
        RUNS,
    );
    checks.check(
        "F3 100 moves over a pane's text: no more frames than standing still (quietest runs)",
        quietest(&idle_moves) <= quietest(&still) + SLACK,
        || format!("runs of moves {idle_moves:?} against standing still {still:?}"),
    );
    // On one element: a frame to light it (waited for), none after.
    motion(&mut term, name);
    assert!(wait(Duration::from_secs(3), || shown(&term).contains("réglages")), "the name never lit: {}", shown(&term));
    let still = measure(&mut term, &mut |_| {}, RUN, RUNS);
    let same = measure(
        &mut term,
        &mut |term| {
            for _ in 0..100 {
                motion(term, name);
            }
        },
        RUN,
        RUNS,
    );
    checks.check(
        "F3 100 moves on the same name: no more frames than standing still (quietest runs)",
        quietest(&same) <= quietest(&still) + SLACK,
        || format!("runs of moves {same:?} against standing still {still:?}"),
    );
    // From one element to the other, twenty times: about a frame a change. A run lasts 0.8 s, longer than a second's
    // clocks: the least a run draws is a frame or so a change, the quietest run no more than a frame a change.
    let still = measure(&mut term, &mut |_| {}, Duration::from_millis(800), 3);
    let changes = measure(
        &mut term,
        &mut |term| {
            for i in 0..20 {
                motion(term, if i % 2 == 0 { model } else { name });
                std::thread::sleep(Duration::from_millis(40));
            }
        },
        Duration::from_millis(800),
        3,
    );
    println!("F3 frames per run: still {still:?}, changes {changes:?}, same {same:?}, idle moves {idle_moves:?}");
    checks.check(
        "F3 20 changes of element: at least 6 frames in every run, at most 25 in the quietest (moves merge under load)",
        changes.iter().all(|c| *c >= 6) && quietest(&changes) <= 25 + quietest(&still) + SLACK,
        || format!("runs of changes {changes:?} against standing still {still:?}"),
    );
    // The name lit and unlit shows on the screen.
    motion(&mut term, name);
    let lit = wait(Duration::from_secs(2), || shown(&term).contains("réglages"));
    checks.check("F3 the name lit says « réglages »", lit, || shown(&term));
    motion(&mut term, inside);
    let unlit = wait(Duration::from_secs(2), || !shown(&term).contains("réglages"));
    checks.check("F3 off it, it goes out", unlit, || shown(&term));
    checks.done();
    team.stop();
}
