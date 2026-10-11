// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A whole team on the native multiplexer, launched by `recruit` with a fake `claude` (`common::team`): the parity
//! checks of the spec's §7.1 (grid, « Parité ») that need no window. Part of `cargo test`.
//!
//! Owner: testeur.

mod common;

use std::time::{Duration, Instant};

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

/// `[teams.<name>]` with `contacts` and `agents` (`(name, tab)`), roles made up.
fn tables(name: &str, contacts: &[&str], agents: &[(&str, Option<&str>)], extra: &str) -> String {
    let mut out = format!("[teams.{name}]\ndescription = \"test\"\n{extra}\n");
    for c in contacts {
        out.push_str(&format!("[teams.{name}.members.{c}]\nrole = \"Contact {c}\"\ncontact = true\n"));
    }
    for (a, tab) in agents {
        out.push_str(&format!("[teams.{name}.members.{a}]\nrole = \"Agent {a}\"\n"));
        if let Some(tab) = tab {
            out.push_str(&format!("tab = \"{tab}\"\n"));
        }
    }
    out
}

/// D1: the contacts' tab first (with the dashboard and the journal), then a tab per `tab`, then the others in
/// « Agents », groups of 6 at most shared out evenly, small teams included.
#[test]
fn tabs_as_planned() {
    let agents: Vec<(String, Option<&str>)> = (1..=9)
        .map(|i| (format!("a{i}"), None))
        .chain([("q1".to_string(), Some("qa")), ("q2".to_string(), Some("qa"))])
        .collect();
    let agents: Vec<(&str, Option<&str>)> = agents.iter().map(|(n, t)| (n.as_str(), *t)).collect();
    let mut team = Team::new("tabs", &tables("tabs", &["lead"], &agents, ""));
    let mut members: Vec<&str> = vec!["lead"];
    members.extend(agents.iter().map(|(n, _)| *n));
    team.launch(&members);
    let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let want = vec![
        ("Interlocuteurs".to_string(), names(&["lead", "Tableau de bord", "Journal"])),
        ("Agents (1)".to_string(), names(&["a1", "a2", "a3", "a4", "a5"])),
        ("Agents (2)".to_string(), names(&["a6", "a7", "a8", "a9"])),
        ("qa".to_string(), names(&["q1", "q2"])),
    ];
    assert_eq!(team.tabs(), want);
    team.stop();
}

/// D1 for a small team (one « Agents » tab), D5: `dashboard = false`, no dashboard nor journal, the two contacts
/// side by side (each half the width, the whole height); L3: `recruit list` names the team.
#[test]
fn small_team_without_dashboard() {
    let mut team = Team::new("small", &tables("small", &["lead", "pm"], &[("dev", None)], "dashboard = false"));
    team.launch(&["lead", "pm", "dev"]);
    let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        team.tabs(),
        vec![("Interlocuteurs".to_string(), names(&["lead", "pm"])), ("Agents".to_string(), names(&["dev"]))]
    );
    let panes = team.panes();
    let size = |member: &str| {
        let pane = panes.iter().find(|p| p["member"] == member).expect("pane");
        (pane["cols"].as_u64().unwrap(), pane["rows"].as_u64().unwrap())
    };
    let (lead, pm, dev) = (size("lead"), size("pm"), size("dev"));
    assert!(lead.1 == dev.1 && pm.1 == dev.1, "not the whole height: lead {lead:?}, pm {pm:?}, dev {dev:?}");
    assert!(lead.0 + pm.0 <= dev.0 && lead.0 + pm.0 + 4 >= dev.0, "not side by side: {lead:?} {pm:?} {dev:?}");
    let list = team.recruit(["list"]);
    let list = String::from_utf8_lossy(&list.stdout);
    assert!(list.contains("small"), "recruit list: {list}");
    team.stop();
}

/// The session `member` runs, once `member.rs` noted it (2 s after its start).
fn noted(team: &Team, member: &str) -> String {
    let file = team.state().join("sessions").join(member);
    assert!(wait(Duration::from_secs(8), || file.exists()), "{member}'s session never noted");
    std::fs::read_to_string(file).expect("session").trim().to_string()
}

/// S1, S2: a member that stops on its own comes back 2 s later on its conversation (`-r <session>`); stopped twice
/// just after starting, it is not started again; and the team's stop starts nothing again.
#[test]
#[ignore = "5 s: the 2 s before member.rs notes a session, and the 2 s before it starts Claude again"]
fn a_stopped_member_comes_back() {
    let mut team = Team::new("back", &tables("back", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let session = noted(&team, "dev");
    team.ctl(["send", "--pane", "dev", "Q\\r"]);
    let stopped = Instant::now();
    assert!(
        wait(Duration::from_secs(8), || team.launches("dev").len() == 2),
        "dev never came back: {:?}",
        team.capture("dev")
    );
    let after = stopped.elapsed();
    let again = team.launches("dev")[1].clone();
    assert!(after >= Duration::from_millis(1900), "back after {after:?}, before the 2 s");
    assert!(again.ends_with(&format!("-n dev -r {session}")), "not on its conversation: {again}");
    // Stopped again at once: twice just after starting, it stays stopped.
    assert!(team.wait_pane("dev", "FAKE CLAUDE", Duration::from_secs(5)));
    team.ctl(["send", "--pane", "dev", "Q\\r"]);
    assert!(team.wait_pane("dev", "deux fois", Duration::from_secs(5)), "{:?}", team.capture("dev"));
    // Nothing left to start it again: its loop is over.
    assert!(wait(Duration::from_secs(3), || !team.member_runs("dev")), "dev's loop still runs");
    assert_eq!(team.launches("dev").len(), 2, "started a third time");
    let lead = team.launches("lead").len();
    team.stop();
    assert_eq!(team.launches("lead").len(), lead, "the team's stop started lead again");
}

/// S2: Ctrl-C in the 2 s before a member comes back leaves a shell instead, and nothing is started again.
#[test]
fn ctrl_c_leaves_a_shell() {
    let mut team = Team::new("shell", &tables("shell", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    team.ctl(["send", "--pane", "dev", "Q\\r"]);
    // Ctrl-C again and again, as one would: one that comes while Claude still runs is forgotten (`member.rs` clears
    // it before its pause), one in the pause leaves the shell. Reading the pane to time it misses the 2 s when the
    // machine is busy.
    let cancelled = wait(Duration::from_secs(6), || {
        team.ctl(["send", "--pane", "dev", "\\x03"]);
        std::thread::sleep(Duration::from_millis(150));
        team.capture("dev").iter().any(|r| r.contains("Reprise annulée"))
    });
    assert!(cancelled, "{:?}\nlaunches: {:?}", team.capture("dev"), team.launches("dev"));
    team.ctl(["send", "--pane", "dev", "echo SHELL-$((40+2))\\r"]);
    assert!(team.wait_pane("dev", "SHELL-42", Duration::from_secs(5)), "no shell: {:?}", team.capture("dev"));
    assert!(wait(Duration::from_secs(3), || !team.member_runs("dev")), "dev's loop still runs after Ctrl-C");
    assert_eq!(team.launches("dev").len(), 1, "started again after Ctrl-C");
    team.stop();
}

/// H1: what the menu changes on a running team (`recruit _edit`, as the menu does): a member moved to another tab
/// keeps running; one added starts; one removed goes; one renamed starts again on its conversation.
#[test]
fn hot_edits() {
    let mut team = Team::new("hot", &tables("hot", &["lead"], &[("dev", None), ("ops", None)], ""));
    team.launch(&["lead", "dev", "ops"]);
    let state = team.state();
    let edit = |json: &str| {
        let out = team.recruit([std::ffi::OsStr::new("_edit"), state.as_os_str(), std::ffi::OsStr::new(json)]);
        assert!(out.status.success(), "_edit {json}: {}", String::from_utf8_lossy(&out.stderr));
    };
    let pid =
        |team: &Team, member: &str| team.panes().iter().find(|p| p["member"] == member).and_then(|p| p["pid"].as_u64());
    let dev = pid(&team, "dev");
    edit(r#"[{"set": {"member": "dev", "field": {"tab": "qa"}}}]"#);
    assert!(
        wait(Duration::from_secs(5), || team.tabs().iter().any(|(t, m)| t == "qa" && m == &["dev".to_string()])),
        "dev not moved: {:?}",
        team.tabs()
    );
    assert_eq!(pid(&team, "dev"), dev, "dev was started again to move");
    assert_eq!(team.launches("dev").len(), 1, "dev's Claude was started again to move");
    edit(r#"[{"add": {"name": "new", "member": {"role": "New"}}}]"#);
    assert!(team.wait_pane("new", "FAKE CLAUDE", Duration::from_secs(8)), "new never started");
    edit(r#"[{"remove": "ops"}]"#);
    assert!(
        wait(Duration::from_secs(5), || !team.panes().iter().any(|p| p["member"] == "ops")),
        "ops still there: {:?}",
        team.tabs()
    );
    let session = noted(&team, "dev");
    edit(r#"[{"rename": {"from": "dev", "to": "dev2"}}]"#);
    assert!(
        wait(Duration::from_secs(8), || !team.launches("dev2").is_empty()),
        "dev2 never started: {:?}",
        team.tabs()
    );
    let renamed = team.launches("dev2")[0].clone();
    assert!(renamed.ends_with(&format!("-n dev2 -r {session}")), "not on dev's conversation: {renamed}");
    team.stop();
}

/// The key Alt+`key` as the client's terminal sends it: kitty's keyboard protocol when it speaks it, else ESC.
fn alt(term: &TestTerm, key: char) -> Vec<u8> {
    if term.kitty_keyboard() {
        format!("\x1b[{};3u", key as u32).into_bytes()
    } else {
        format!("\x1b{key}").into_bytes()
    }
}

/// Where `text` first shows on `term`: column (in cells) and row.
fn find(term: &TestTerm, text: &str) -> Option<(usize, usize)> {
    term.screen().iter().enumerate().find_map(|(row, line)| {
        let at = line.find(text)?;
        Some((unicode_width::UnicodeWidthStr::width(&line[..at]), row))
    })
}

/// A click (press and release, SGR) on the cell `(col, row)`.
fn click(term: &mut TestTerm, (col, row): (usize, usize)) {
    term.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 1, row + 1, col + 1, row + 1).as_bytes());
}

/// A client attached to `team`, once it shows its first member.
fn client(team: &Team, member: &str) -> TestTerm {
    let term = team.attach(120, 40, Profile::default());
    assert!(term.wait_for(member, Duration::from_secs(5)), "no client: {:?}", term.screen());
    term
}

/// Q1: ⌥q opens « Detach, Quit, Cancel » on Cancel; Enter, the letters, Esc and a click close it or choose; `d` ends
/// this client and the team stays, `q` stops the team and leaves nothing.
fn quit_choice(lang: &'static str) {
    let name = format!("quit-{lang}");
    let mut team = Team::in_lang(&name, &tables(&name, &["lead"], &[("dev", None)], ""), lang);
    team.launch(&["lead", "dev"]);
    let (detach, quit, cancel) =
        if lang == "fr" { ("Détacher", "Quitter", "Annuler") } else { ("Detach", "Quit", "Cancel") };
    let mut term = client(&team, "lead");
    let open = |term: &mut TestTerm| {
        let keys = alt(term, 'q');
        term.send(&keys);
        assert!(term.wait_for(cancel, Duration::from_secs(3)), "the choice never opened: {:?}", term.screen());
        for option in [detach, quit] {
            assert!(find(term, option).is_some(), "no {option}: {:?}", term.screen());
        }
    };
    let closed = |term: &TestTerm| wait(Duration::from_secs(3), || find(term, cancel).is_none());
    // Opened on Cancel: Enter closes it and nothing else happens.
    open(&mut term);
    term.send(b"\r");
    assert!(closed(&term), "Enter on Cancel did not close it: {:?}", term.screen());
    // Esc, then the letter of Cancel, then a click on it.
    open(&mut term);
    term.send(b"\x1b");
    assert!(closed(&term), "Esc did not close it: {:?}", term.screen());
    open(&mut term);
    term.send(if lang == "fr" { b"a" } else { b"c" });
    assert!(closed(&term), "the letter of Cancel did not close it: {:?}", term.screen());
    open(&mut term);
    let at = find(&term, cancel).expect("Cancel");
    click(&mut term, at);
    assert!(closed(&term), "a click on Cancel did not close it: {:?}", term.screen());
    assert!(term.wait_exit(Duration::from_millis(300)).is_none(), "the client left on a cancel");
    assert!(team.running(), "the team stopped on a cancel");
    // Detach: this client ends, the team stays.
    open(&mut term);
    term.send(b"d");
    assert!(term.wait_exit(Duration::from_secs(5)).is_some(), "the client stayed after Detach: {:?}", term.screen());
    assert!(team.running(), "Detach stopped the team");
    assert_eq!(team.panes().len(), 4, "panes after Detach: {:?}", team.tabs());
    // A click on Detach, from a new client.
    let mut term = client(&team, "lead");
    open(&mut term);
    let at = find(&term, detach).expect("Detach");
    click(&mut term, at);
    assert!(term.wait_exit(Duration::from_secs(5)).is_some(), "a click on Detach left the client there");
    assert!(team.running(), "a click on Detach stopped the team");
    // Quit stops the team, whatever is left.
    let mut term = client(&team, "lead");
    open(&mut term);
    term.send(b"q");
    assert!(term.wait_exit(Duration::from_secs(5)).is_some(), "the client stayed after Quit");
    assert!(wait(Duration::from_secs(5), || !team.running()), "the team still runs after Quit");
    team.stopped_by_the_user();
}

#[test]
fn quit_choice_fr() {
    quit_choice("fr");
}

#[test]
fn quit_choice_en() {
    quit_choice("en");
}

/// What a client's keys and clicks do, on one team: A1 tabs by key, A2 the Alt sign, D3 the dashboard and the journal
/// beside and under the contacts, D4 ⌥j's four sizes, K4 the bar's buttons, R1 the menu over the team.
#[test]
fn client_keys_and_buttons() {
    let mut team = Team::new("keys", &tables("keys", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let mut term = client(&team, "lead");
    let mut failed: Vec<String> = Vec::new();
    let mut check = |what: &str, ok: bool, got: String| {
        if !ok {
            failed.push(format!("{what}: {got}"));
        }
    };
    let screen = |term: &TestTerm| term.screen().join("\n");
    let sign = if cfg!(target_os = "macos") { "\u{2325}" } else { "Alt+" };
    // The first frames may come before the panes are placed (a loaded machine): the layout is waited for.
    let placed = |term: &TestTerm| {
        let (lead, board, journal) = (find(term, "lead"), find(term, "Tableau de bord"), find(term, "Journal"));
        matches!(
            (lead, board, journal),
            (Some(l), Some(b), Some(j)) if b.0 > l.0 && b.1 == l.1 && j.0 == b.0 && j.1 > b.1
        )
    };
    let _ = wait(Duration::from_secs(5), || placed(&term) && screen(&term).contains(&format!("menu ({sign}r)")));
    check("A2 the bar names the shortcuts", screen(&term).contains(&format!("menu ({sign}r)")), screen(&term));
    // D3: the dashboard to the right of the contact, on the top row; the journal under it.
    let (lead, board, journal) = (find(&term, "lead"), find(&term, "Tableau de bord"), find(&term, "Journal"));
    check(
        "D3 dashboard right of the contact, journal under the dashboard",
        matches!(
            (lead, board, journal),
            (Some(l), Some(b), Some(j)) if b.0 > l.0 && b.1 == l.1 && j.0 == b.0 && j.1 > b.1
        ),
        format!("{lead:?} {board:?} {journal:?}"),
    );
    // D4: reduced at launch, then hidden, full, reduced.
    let journal_rows = |team: &Team| {
        let panes = team.panes();
        panes.iter().find(|p| p["role"] == "journal").map(|p| p["rows"].as_u64().unwrap())
    };
    let mut sizes = vec![journal_rows(&team)];
    for _ in 0..3 {
        let keys = alt(&term, 'j');
        term.send(&keys);
        let before = sizes.last().cloned().flatten();
        wait(Duration::from_secs(3), || journal_rows(&team) != before);
        sizes.push(journal_rows(&team));
    }
    let ok = match sizes[..] {
        [Some(reduced), None, Some(full), Some(again)] => reduced <= 10 && full > reduced && again == reduced,
        _ => false,
    };
    check("D4 reduced, hidden, full, reduced", ok, format!("{sizes:?}"));
    // A1: ⌥2, ⌥1, ⌥⇧→, ⌥⇧←; the current tab reversed in the bar.
    let bar_row = |term: &TestTerm| term.screen().len() - 1;
    let reversed = |term: &TestTerm, label: &str| {
        let at = find(term, label)?;
        Some(term.inverse(at.0, bar_row(term)))
    };
    for (what, keys, shows, tab) in [
        ("A1 ⌥2", alt(&term, '2'), "FAKE CLAUDE dev", "2 Agents"),
        ("A1 ⌥1", alt(&term, '1'), "FAKE CLAUDE lead", "1 Interlocuteurs"),
        ("A1 ⌥⇧→", b"\x1b[1;4C".to_vec(), "FAKE CLAUDE dev", "2 Agents"),
        ("A1 ⌥⇧←", b"\x1b[1;4D".to_vec(), "FAKE CLAUDE lead", "1 Interlocuteurs"),
    ] {
        term.send(&keys);
        let got = wait(Duration::from_secs(3), || screen(&term).contains(shows));
        let other = if tab.starts_with('1') { "2 Agents" } else { "1 Interlocuteurs" };
        let ok = reversed(&term, tab) == Some(true) && reversed(&term, other) == Some(false);
        check(what, got && ok, screen(&term));
    }
    // K4: the bar's buttons.
    let at = find(&term, &format!("menu ({sign}r)")).expect("menu button");
    click(&mut term, at);
    let opened = wait(Duration::from_secs(5), || screen(&term).contains("Nouvel agent"));
    check(
        "K4 the menu button opens the menu (R1: the members, over the team)",
        opened && screen(&term).contains("dev"),
        screen(&term),
    );
    term.send(b"\x1b");
    check(
        "R1 Esc closes the menu",
        wait(Duration::from_secs(3), || !screen(&term).contains("Nouvel agent")),
        screen(&term),
    );
    let at = find(&term, &format!("quitter ({sign}q)")).expect("quit button");
    click(&mut term, at);
    let opened = wait(Duration::from_secs(3), || screen(&term).contains("Annuler"));
    check("K4 the quit button opens the choice", opened, screen(&term));
    term.send(b"\x1b");
    assert!(wait(Duration::from_secs(3), || !screen(&term).contains("Annuler")), "the choice stays: {}", screen(&term));
    // R1 by the key.
    let keys = alt(&term, 'r');
    term.send(&keys);
    check(
        "R1 ⌥r opens the menu",
        wait(Duration::from_secs(5), || screen(&term).contains("Nouvel agent")),
        screen(&term),
    );
    term.send(b"\x1b");
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    team.stop();
}

/// Whether the header on `row` of `name` has the thick frame of the active pane.
fn thick(row: &str, name: &str) -> bool {
    let chars: Vec<char> = row.chars().collect();
    let name: Vec<char> = format!(" {name} ").chars().collect();
    (0..chars.len().saturating_sub(name.len() - 1))
        .filter(|&i| chars[i..i + name.len()] == name[..])
        .any(|i| chars[..i].iter().rev().find(|c| matches!(c, '┏' | '╭' | '┓' | '╮')).is_some_and(|c| *c == '┏'))
}

/// Where `text` first shows on a row that is a pane's top border (a header), not a card of the dashboard.
fn find_in_header(term: &TestTerm, text: &str) -> Option<(usize, usize)> {
    term.screen().iter().enumerate().find_map(|(row, line)| {
        if !line.chars().any(|c| matches!(c, '┏' | '╭')) {
            return None;
        }
        let at = line.find(text)?;
        Some((unicode_width::UnicodeWidthStr::width(&line[..at]), row))
    })
}

/// Whether a pane's header names `name` on `term`: a frame's top border, not the dashboard's card.
fn has_header(term: &TestTerm, name: &str) -> bool {
    term.screen().iter().any(|row| {
        row.contains(&format!(" {name} ")) && row.chars().any(|c| matches!(c, '┏' | '╭')) && row.contains(['━', '─'])
    })
}

/// The members, among `members`, whose header is thick on the screen of `term`: the active pane.
fn active(term: &TestTerm, members: &[&str]) -> Vec<String> {
    let screen = term.screen();
    members
        .iter()
        .filter(|m| screen.iter().any(|row| row.contains(&format!(" {m} ")) && thick(row, m)))
        .map(|m| m.to_string())
        .collect()
}

/// A member says what it does (OSC 7501), as Claude Code does: `working`, `blocked:kind=permission`, `idle`.
fn says(team: &Team, member: &str, state: &str) {
    team.ctl(["send", "--pane", member, &format!("osc {state}\\r")]);
}

/// Collects the failed checks of a test, to report them all at once.
#[derive(Default)]
struct Checks(Vec<String>);

impl Checks {
    fn check(&mut self, what: &str, ok: bool, got: impl FnOnce() -> String) {
        if !ok {
            self.0.push(format!("{what}: {}", got()));
        }
    }

    fn done(self) {
        let more = self.0.len().saturating_sub(8);
        assert!(
            self.0.is_empty(),
            "{} failed:\n{}\n({more} more)",
            self.0.len(),
            self.0[..self.0.len() - more].join("\n")
        );
    }
}

fn shown(term: &TestTerm) -> String {
    term.screen().join("\n")
}

/// F6: ⌥g goes to the member who has waited the longest, and nowhere when none waits. Also F5's badge: ⚑ in the bar
/// on the tab of a member that waits, out of view, and the states in the bar by OSC 7501.
#[test]
fn alt_g_goes_to_who_waits() {
    let mut team = Team::new("waits", &tables("waits", &["lead"], &[("dev1", None), ("dev2", None)], ""));
    team.launch(&["lead", "dev1", "dev2"]);
    let mut term = client(&team, "lead");
    let members = ["lead", "dev1", "dev2"];
    let go = |term: &mut TestTerm| {
        let keys = alt(term, 'g');
        term.send(&keys);
    };
    let mut checks = Checks::default();
    // Nobody waits: ⌥g stays where it is.
    go(&mut term);
    std::thread::sleep(Duration::from_millis(300));
    let now = active(&term, &members);
    checks.check("F6 nobody waits: the focus stays", now == ["lead"], || format!("{now:?}"));
    // dev2 waits first, dev1 a while later (the states are told in seconds): the longest waiting, dev2, comes first.
    says(&team, "dev2", "blocked:kind=permission");
    let badge = wait(Duration::from_secs(5), || shown(&term).contains("\u{2691} 2 Agents"));
    checks.check("F5 the badge of a waiting member out of view", badge, || {
        term.screen().last().cloned().unwrap_or_default()
    });
    std::thread::sleep(Duration::from_millis(1300));
    says(&team, "dev1", "blocked:kind=permission");
    std::thread::sleep(Duration::from_millis(500));
    go(&mut term);
    let got = wait(Duration::from_secs(3), || active(&term, &members) == ["dev2"]);
    checks.check("F6 to the longest waiting", got, || format!("{:?}\n{}", active(&term, &members), shown(&term)));
    // dev2 answered: dev1 is next.
    says(&team, "dev2", "working");
    std::thread::sleep(Duration::from_millis(500));
    let keys = alt(&term, '1');
    term.send(&keys);
    wait(Duration::from_secs(3), || active(&term, &members) == ["lead"]);
    go(&mut term);
    let got = wait(Duration::from_secs(3), || active(&term, &members) == ["dev1"]);
    checks.check("F6 then to the next", got, || format!("{:?}\n{}", active(&term, &members), shown(&term)));
    checks.done();
    team.stop();
}

/// F4: ⌥z gives the active member's pane the whole tab and the bar names it; ⌥z again, or a click on ⤡, brings the
/// grid back; a click on ⤢ zooms.
#[test]
fn zoom() {
    let mut team = Team::new("zoom", &tables("zoom", &["lead"], &[("dev1", None), ("dev2", None)], ""));
    team.launch(&["lead", "dev1", "dev2"]);
    let mut term = client(&team, "lead");
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(
        wait(Duration::from_secs(3), || has_header(&term, "dev1") && has_header(&term, "dev2")),
        "{}",
        shown(&term)
    );
    let mut checks = Checks::default();
    let focused = active(&term, &["dev1", "dev2"]);
    checks.check("F4 one active member", focused.len() == 1, || format!("{focused:?}"));
    let name = focused.first().cloned().unwrap_or_default();
    let other = if name == "dev1" { "dev2" } else { "dev1" };
    let keys = alt(&term, 'z');
    term.send(&keys);
    let zoomed = wait(Duration::from_secs(3), || has_header(&term, &name) && !has_header(&term, other));
    checks.check("F4 ⌥z: alone in its tab", zoomed, || shown(&term));
    let width = team.panes().iter().find(|p| p["member"] == name.as_str()).map(|p| p["cols"].as_u64().unwrap());
    checks.check("F4 it has the tab's width", width.is_some_and(|w| w >= 110), || format!("{width:?}"));
    term.send(&keys);
    let back = wait(Duration::from_secs(3), || has_header(&term, other));
    checks.check("F4 ⌥z again: the grid is back", back, || shown(&term));
    // The signs in the header: ⤢ zooms, ⤡ brings the grid back.
    match find(&term, "\u{2922}") {
        Some(at) => {
            click(&mut term, at);
            let zoomed = wait(Duration::from_secs(3), || !has_header(&term, other));
            checks.check("F4 a click on ⤢ zooms", zoomed, || shown(&term));
            match find(&term, "\u{2921}") {
                Some(at) => {
                    click(&mut term, at);
                    let back = wait(Duration::from_secs(3), || has_header(&term, other));
                    checks.check("F4 a click on ⤡ brings the grid back", back, || shown(&term));
                }
                None => checks.check("F4 ⤡ in the zoomed header", false, || shown(&term)),
            }
        }
        None => checks.check("F4 ⤢ in a header", false, || shown(&term)),
    }
    checks.done();
    team.stop();
}

/// F4: the zoomed tab says so in the bar, « ⤢ name » after its title.
#[test]
fn zoom_named_in_the_bar() {
    let mut team = Team::new("zoombar", &tables("zoombar", &["lead"], &[("dev1", None), ("dev2", None)], ""));
    team.launch(&["lead", "dev1", "dev2"]);
    let mut term = client(&team, "lead");
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(
        wait(Duration::from_secs(3), || has_header(&term, "dev1") && has_header(&term, "dev2")),
        "{}",
        shown(&term)
    );
    let name = active(&term, &["dev1", "dev2"]).pop().expect("an active member");
    let keys = alt(&term, 'z');
    term.send(&keys);
    let bar = |term: &TestTerm| term.screen().last().cloned().unwrap_or_default();
    let mark = format!("\u{2922} {name}");
    assert!(wait(Duration::from_secs(3), || bar(&term).contains(&mark)), "{}", bar(&term));
    term.send(&keys);
    assert!(wait(Duration::from_secs(3), || !bar(&term).contains(&mark)), "{}", bar(&term));
    team.stop();
}

/// What the mod last reported for `member` (`<state>/members/<member>.json`), as `board::looks` reads it.
fn report(team: &Team, member: &str, model: &str, effort: &str, percent: f64) {
    let dir = team.state().join("members");
    std::fs::create_dir_all(&dir).expect("members folder");
    let file = dir.join(format!("{member}.json"));
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut report = serde_json::json!({
        "session": "s", "model": model, "own": {"model": model, "effort": effort},
        "context": {"percent": percent, "tokens": (percent * 2000.0) as u64, "window": 200000}, "at": at,
    });
    // The mod's answer to a compaction (`answer_compaction`) stays through the reports told again.
    let kept = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|old| old.get("compaction").cloned());
    if let Some(compaction) = kept {
        report["compaction"] = compaction;
    }
    std::fs::write(file, report.to_string()).expect("report");
}

/// What the member's mod says of the compaction `id` it was asked (`compact/<member>`): `outcome` is
/// `{"done": true}`, `{"skip": "helpers"}` or `{"error": "…"}`, in its report (`bridge::Compaction`).
fn answer_compaction(team: &Team, member: &str, id: &str, outcome: serde_json::Value) {
    let file = team.state().join("members").join(format!("{member}.json"));
    let mut report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("a report to complete")).expect("report: JSON");
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut compaction = serde_json::json!({"id": id, "auto": true, "at": at});
    for (key, value) in outcome.as_object().expect("an outcome") {
        compaction[key] = value.clone();
    }
    report["compaction"] = compaction;
    std::fs::write(file, report.to_string()).expect("report");
}

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

/// The text of the menu's pane (the server names it « recruit »), or nothing while there is none.
fn menu_text(team: &Team) -> Option<Vec<String>> {
    let out = team.ctl_output(["capture", "--pane", "recruit"]);
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect())
}

/// Closes the menu from wherever it is: Esc until its pane is gone.
fn close_menu(team: &Team, term: &mut TestTerm) -> bool {
    wait(Duration::from_secs(8), || {
        if menu_text(team).is_none() {
            // And the client's screen without it: a click sent before is lost.
            return wait(Duration::from_secs(3), || !shown(term).contains("Nouvel agent"));
        }
        term.send(b"\x1b");
        std::thread::sleep(Duration::from_millis(300));
        false
    })
}

/// F2: a click on the name, the model or the effort of a header opens the real `_menu` on that member's sheet, on
/// that field; F7: ⌥r opens it on the sheet of the active pane's member.
#[test]
#[ignore = "8 s, the menu is started six times: run with --ignored"]
fn header_clicks_open_the_sheet() {
    let mut team = Team::new("sheet", &tables("sheet", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    report(&team, "lead", "claude-opus-5-5", "xhigh", 85.0);
    report(&team, "dev", "claude-sonnet-5-5", "high", 40.0);
    says(&team, "lead", "idle");
    says(&team, "dev", "idle");
    let mut term = client(&team, "lead");
    // The headers learn the models and efforts within two seconds.
    assert!(wait(Duration::from_secs(6), || find_in_header(&term, "Opus").is_some()), "{}", shown(&term));
    let mut checks = Checks::default();
    // The sheet of `member` on screen: its name at the top of the right half; the field in focus: « ‹ › » round its
    // value, or the line that says what ⏎ does.
    let sheet = |lines: &[String], member: &str| {
        lines.iter().any(|row| row.contains(&format!("│ {member} ")) || row.contains(&format!("┃ {member} ")))
            || lines.iter().any(|row| row.split('│').any(|cell| cell.trim() == member))
    };
    for (member, tab_key, part, label) in [
        ("lead", '1', "Opus", "Modèle"),
        ("lead", '1', "extra", "Effort"),
        ("dev", '2', "Sonnet", "Modèle"),
        ("dev", '2', "élevé", "Effort"),
    ] {
        let keys = alt(&term, tab_key);
        term.send(&keys);
        // The member's own header first: the other tab's shows « extra » when « élevé » is looked for.
        let there = wait(Duration::from_secs(3), || has_header(&term, member) && find_in_header(&term, part).is_some());
        assert!(there, "{part}: {}", shown(&term));
        let at = find_in_header(&term, part).expect("part");
        let kept = untouched(&term);
        click(&mut term, at);
        // The model and the effort open their list under the text (lot of 2026-10-11, `quick_list_*`), not the sheet.
        let opened = wait(Duration::from_secs(8), || menu_text(&team).is_some());
        checks.check(&format!("F2 a click on {member}'s {part} opens the menu"), opened, || shown(&term));
        let listed = wait_list(&team, &term).is_some();
        checks.check(&format!("F2 {member}'s {part} opens the list of {label}"), listed, || shown(&term));
        let lines = menu_text(&team).unwrap_or_default();
        checks.check(&format!("F2 {member}'s {part}: the values only, no sheet"), !sheet(&lines, member), || {
            lines.join("\n")
        });
        checks.check("F2 the list leaves the team as it was", untouched(&term) == kept, || shown(&term));
        checks.check("the menu closes", close_menu(&team, &mut term), || shown(&term));
    }
    // The name: the field to rename.
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(wait(Duration::from_secs(3), || find_in_header(&term, " dev ").is_some()), "{}", shown(&term));
    let (col, row) = find_in_header(&term, " dev ").expect("name");
    click(&mut term, (col + 1, row));
    let renaming =
        wait(Duration::from_secs(8), || menu_text(&team).is_some_and(|l| l.iter().any(|r| r.contains("renommer"))));
    checks.check("F2 a click on the name opens the Nom field", renaming, || {
        menu_text(&team).unwrap_or_default().join("\n")
    });
    checks.check("the menu closes", close_menu(&team, &mut term), || shown(&term));
    // F7: ⌥r on the active pane (dev, in the second tab): its sheet; then in the first tab: lead's.
    for (member, tab_key) in [("dev", '2'), ("lead", '1')] {
        let keys = alt(&term, tab_key);
        term.send(&keys);
        assert!(wait(Duration::from_secs(3), || find_in_header(&term, member).is_some()));
        let keys = alt(&term, 'r');
        term.send(&keys);
        let opened = wait(Duration::from_secs(8), || menu_text(&team).is_some());
        checks.check(&format!("F7 ⌥r opens the menu on {member}"), opened, || shown(&term));
        let lines = menu_text(&team).unwrap_or_default();
        let _ = wait(Duration::from_secs(3), || menu_text(&team).is_some_and(|l| sheet(&l, member)));
        let lines = menu_text(&team).unwrap_or(lines);
        checks.check(&format!("F7 the sheet is {member}'s"), sheet(&lines, member), || lines.join("\n"));
        checks.check("the menu closes", close_menu(&team, &mut term), || shown(&term));
    }
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

/// K1: a click on a member's card in the dashboard goes to its pane (its tab, its thick frame). K3: a click on the
/// context of a member at rest opens « Compacter ? » on Annuler, the rest dimmed; Esc and Annuler do nothing;
/// Compacter asks the member (`compact/<member>` in the team's folder).
#[test]
fn dashboard_clicks() {
    let mut team = Team::new("cards", &tables("cards", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    report(&team, "lead", "claude-opus-5-5", "xhigh", 12.0);
    report(&team, "dev", "claude-sonnet-5-5", "high", 40.0);
    says(&team, "lead", "idle");
    says(&team, "dev", "idle");
    let mut term = client(&team, "lead");
    let card_of = |term: &TestTerm, name: &str| {
        term.screen().iter().enumerate().find_map(|(row, line)| {
            let at = line.find('╻')?;
            let after = &line[at..];
            let name_at = after.find(&format!(" {name} "))?;
            Some((unicode_width::UnicodeWidthStr::width(&line[..at + name_at]) + 1, row))
        })
    };
    let mut checks = Checks::default();
    assert!(wait(Duration::from_secs(8), || card_of(&term, "dev").is_some()), "no card: {}", shown(&term));
    // K1
    let at = card_of(&term, "dev").expect("dev's card");
    click(&mut term, at);
    let there = wait(Duration::from_secs(3), || active(&term, &["lead", "dev"]) == ["dev"]);
    checks.check("K1 a click on dev's card goes to dev", there, || shown(&term));
    let keys = alt(&term, '1');
    term.send(&keys);
    wait(Duration::from_secs(3), || active(&term, &["lead", "dev"]) == ["lead"]);
    // K3: the context, on dev's second card row (« ⟳ 40 % »).
    let context = wait(Duration::from_secs(8), || find(&term, "40 %").is_some());
    checks.check("K3 the card shows the context", context, || shown(&term));
    let compactions = team.state().join("compact");
    if let Some((col, row)) = find(&term, "40 %") {
        for how in ["Esc", "Annuler"] {
            click(&mut term, (col, row));
            let open = wait(Duration::from_secs(3), || shown(&term).contains("Compacter dev"));
            checks.check(&format!("K3 the choice opens ({how})"), open, || shown(&term));
            // Opens on Annuler: Enter alone does nothing; Esc closes; so does a click on Annuler.
            if how == "Esc" {
                term.send(b"\x1b");
            } else if let Some(at) = find(&term, "Annuler") {
                click(&mut term, at);
            }
            let closed = wait(Duration::from_secs(3), || !shown(&term).contains("Compacter dev"));
            checks.check(&format!("K3 {how} closes it"), closed, || shown(&term));
            checks.check(&format!("K3 {how} asks nothing"), !compactions.join("dev").exists(), || "a request".into());
        }
        click(&mut term, (col, row));
        wait(Duration::from_secs(3), || shown(&term).contains("Compacter dev"));
        term.send(b"c");
        let asked = wait(Duration::from_secs(3), || compactions.join("dev").exists());
        checks.check("K3 « c » asks dev to compact", asked, || shown(&term));
    }
    // F2: the same from the header of a member at rest (lead, in the first tab).
    term.send(b"\x1b");
    let sign = find_in_header(&term, "\u{27f3}");
    checks.check("F2 the header of a member at rest shows its context as compactable", sign.is_some(), || shown(&term));
    if let Some(at) = sign {
        click(&mut term, at);
        let open = wait(Duration::from_secs(3), || shown(&term).contains("Compacter lead"));
        checks.check("F2 a click on the header's context opens the choice", open, || shown(&term));
        term.send(b"\x1b");
        let closed = wait(Duration::from_secs(3), || !shown(&term).contains("Compacter lead"));
        checks.check("F2 Esc closes it", closed, || shown(&term));
    }
    checks.done();
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

/// The native half of `scripts/screenshots.sh` (`scripts/shots-backend.sh`): what the script asks of the multiplexer,
/// answered for a team of fakes, without vhs, a window or a real Claude. C1.
#[test]
#[ignore = "needs bash, jq and python3; run with --ignored"]
fn screenshots_backend_native() {
    let mut team = Team::new("shots", &tables("shots", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let mut term = client(&team, "lead");
    let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/shots-backend.sh");
    let prelude = format!(
        "set -euo pipefail; source '{}'; backend=native; session=$TEAM_NAME; state=$TEAM_STATE; \
         OWN_RECRUIT=$RECRUIT; SOCKET=x; ",
        lib.display()
    );
    let sh = |script: &str| {
        let (ok, text) = team.bash(&format!("{prelude}{script}"));
        assert!(ok, "{script}\n{text}");
        text.trim().to_string()
    };
    let mut checks = Checks::default();
    for role in ["lead", "dev", "dashboard", "journal"] {
        let id = sh(&format!("pane_of {role}"));
        checks.check(&format!("pane_of {role}"), id.starts_with('p'), || id.clone());
    }
    let members = sh("member_panes | wc -l | tr -d ' '");
    checks.check("member_panes counts the members only", members == "2", || members.clone());
    let all = sh("shown_panes | wc -l | tr -d ' '");
    checks.check("shown_panes counts the panels too", all == "4", || all.clone());
    let text = sh("pane_text \"$(pane_of lead)\"");
    checks.check("pane_text", text.contains("FAKE CLAUDE lead"), || text.clone());
    checks.check("state_of_team", sh("state_of_team") == team.state().to_string_lossy(), || sh("state_of_team"));
    checks.check("has_second_tab", sh("has_second_tab && echo yes") == "yes", String::new);
    checks.check("attach_command names the team", sh("attach_command").ends_with("attach shots"), || {
        sh("attach_command")
    });
    // A client is attached: the tabs and the menu, by `_ctl key`.
    checks.check("has_client", sh("has_client && echo yes") == "yes", String::new);
    sh("select_second");
    let second = wait(Duration::from_secs(3), || has_header(&term, "dev"));
    checks.check("select_second", second, || shown(&term));
    sh("select_first");
    let first = wait(Duration::from_secs(3), || has_header(&term, "lead"));
    checks.check("select_first", first, || shown(&term));
    sh("open_menu");
    let menu = wait(Duration::from_secs(8), || shown(&term).contains("Nouvel agent"));
    checks.check("open_menu", menu, || shown(&term));
    close_menu(&team, &mut term);
    // The focus goes to a member of the second tab, by ⌥n from the first.
    sh("select_second");
    wait(Duration::from_secs(3), || has_header(&term, "dev"));
    sh("focus_member dev");
    checks.check("focus_member", wait(Duration::from_secs(3), || active(&term, &["dev"]) == ["dev"]), || shown(&term));
    sh("select_first");
    wait(Duration::from_secs(3), || has_header(&term, "lead"));
    // The header and the card of each member say the same state: told by the fakes (OSC 7501 and `claude agents`).
    says(&team, "lead", "working");
    team.ctl(["send", "--pane", "lead", "agent busy\\r"]);
    says(&team, "dev", "idle");
    wait(Duration::from_secs(6), || {
        let (ok, _) = team.bash(&format!("{prelude}states_agree"));
        ok
    });
    let (agree, why) = team.bash(&format!("{prelude}states_agree"));
    checks.check("states_agree: header and card alike", agree, || why.clone());
    // And apart: the header says it works, the card (`claude agents`) says it rests.
    team.ctl(["send", "--pane", "lead", "agent idle\\r"]);
    let (apart, said) = {
        let mut last = (true, String::new());
        wait(Duration::from_secs(8), || {
            last = team.bash(&format!("{prelude}states_agree"));
            !last.0
        });
        last
    };
    checks.check("states_agree: tells them apart", !apart && said.contains("lead"), || said.clone());
    // A text typed as a user would reach the pane's program: `osc working` makes the fake say it works.
    sh("pane_type \"$(pane_of lead)\" 'osc working'");
    let working = wait(Duration::from_secs(4), || {
        let header = term.screen().into_iter().find(|r| r.contains(" lead ") && r.contains('┏'));
        header.is_some_and(|h| h.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)))
    });
    checks.check("pane_type", working, || shown(&term));
    // The crop of the dashboard: a frame at the right of the first tab.
    let rect = sh("pane_rect \"$(pane_of dashboard)\" 'Tableau de bord'");
    let numbers: Vec<usize> = rect.split_whitespace().filter_map(|n| n.parse().ok()).collect();
    checks.check(
        "pane_rect finds the dashboard's frame",
        numbers.len() == 4 && numbers[0] > 20 && numbers[2] > 20 && numbers[3] > 10,
        || rect.clone(),
    );
    drop(term);
    sh("stop_team");
    let stopped = wait(Duration::from_secs(5), || !team.running());
    checks.check("stop_team", stopped, String::new);
    checks.done();
    team.stopped_by_the_user();
}

/// D2: `layout = "tabs"` gives each member a tab of its own; `columns` and `rows` set the grid of a shared tab (their
/// product is the members a tab takes) and the panes sit in it that way, seen on the client's screen.
#[test]
fn layout_settings_are_followed() {
    let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut tabs =
        Team::new("laytabs", &tables("laytabs", &["lead"], &[("a", None), ("b", None)], "layout = \"tabs\""));
    tabs.launch(&["lead", "a", "b"]);
    let want = vec![
        ("lead".to_string(), names(&["lead", "Tableau de bord", "Journal"])),
        ("a".to_string(), names(&["a"])),
        ("b".to_string(), names(&["b"])),
    ];
    assert_eq!(tabs.tabs(), want, "layout = \"tabs\"");
    tabs.stop();
    let agents: Vec<(&str, Option<&str>)> = vec![("a1", None), ("a2", None), ("a3", None), ("a4", None)];
    let mut grid = Team::new("laygrid", &tables("laygrid", &["lead"], &agents, "columns = 2\nrows = 1"));
    grid.launch(&["lead", "a1", "a2", "a3", "a4"]);
    let tabs = grid.tabs();
    let titles: Vec<&str> = tabs.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(titles, ["Interlocuteurs", "Agents (1)", "Agents (2)"], "two members a tab: {tabs:?}");
    assert_eq!(tabs[1].1, names(&["a1", "a2"]));
    let mut term = client(&grid, "lead");
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(wait(Duration::from_secs(3), || has_header(&term, "a1") && has_header(&term, "a2")), "{}", shown(&term));
    let a1 = find_in_header(&term, " a1 ").expect("a1");
    let a2 = find_in_header(&term, " a2 ").expect("a2");
    assert!(a1.1 == a2.1 && a2.0 > a1.0, "two columns, one row: a1 {a1:?}, a2 {a2:?}");
    drop(term);
    grid.stop();
    // Two columns, two rows: four in one tab, in a square.
    let mut four = Team::new("lay4", &tables("lay4", &["lead"], &agents, "columns = 2\nrows = 2"));
    four.launch(&["lead", "a1", "a2", "a3", "a4"]);
    assert_eq!(four.tabs()[1].1, names(&["a1", "a2", "a3", "a4"]), "{:?}", four.tabs());
    let mut term = client(&four, "lead");
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(
        wait(Duration::from_secs(3), || ["a1", "a2", "a3", "a4"].iter().all(|m| has_header(&term, m))),
        "{}",
        shown(&term)
    );
    let at = |name: &str| find_in_header(&term, &format!(" {name} ")).expect("header");
    let (a1, a2, a3, a4) = (at("a1"), at("a2"), at("a3"), at("a4"));
    assert!(
        a1.1 == a2.1 && a3.1 == a4.1 && a3.1 > a1.1 && a1.0 == a3.0 && a2.0 == a4.0 && a2.0 > a1.0,
        "{a1:?} {a2:?} {a3:?} {a4:?}"
    );
    drop(term);
    four.stop();
}

/// 1A: the grid changed on a running team (`recruit _edit '[{"grid": …}]'`, as the menu does) rearranges the tabs
/// and starts nobody again: every pane keeps its process, and no Claude is launched a second time.
#[test]
fn grid_changes_rearrange_without_restart() {
    let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let agents: Vec<(&str, Option<&str>)> = vec![("a1", None), ("a2", None), ("a3", None), ("a4", None)];
    let mut team = Team::new("gridedit", &tables("gridedit", &["lead"], &agents, ""));
    team.launch(&["lead", "a1", "a2", "a3", "a4"]);
    let pids = |team: &Team| -> Vec<(String, Option<u64>)> {
        let mut all: Vec<_> = team
            .panes()
            .iter()
            .map(|p| (p["member"].as_str().unwrap_or_default().to_string(), p["pid"].as_u64()))
            .collect();
        all.sort();
        all
    };
    assert_eq!(team.tabs()[1].1, names(&["a1", "a2", "a3", "a4"]), "3 × 2 by default: {:?}", team.tabs());
    let before = pids(&team);
    assert!(before.iter().all(|(_, pid)| pid.is_some()), "a pane without a pid: {before:?}");
    let state = team.state();
    let edit = |json: &str| {
        let out = team.recruit([std::ffi::OsStr::new("_edit"), state.as_os_str(), std::ffi::OsStr::new(json)]);
        assert!(out.status.success(), "_edit {json}: {}", String::from_utf8_lossy(&out.stderr));
    };
    // 2 × 1: two members a tab.
    edit(r#"[{"grid": {"columns": 2, "rows": 1}}]"#);
    let two = names(&["a1", "a2"]);
    assert!(
        wait(Duration::from_secs(8), || {
            let tabs = team.tabs();
            tabs.len() == 3 && tabs[1].1 == two && tabs[2].1 == names(&["a3", "a4"])
        }),
        "tabs not rearranged to 2 × 1: {:?}",
        team.tabs()
    );
    assert_eq!(pids(&team), before, "a pane was started again by the grid change");
    // A client sees them side by side.
    let mut term = client(&team, "lead");
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(wait(Duration::from_secs(3), || has_header(&term, "a1") && has_header(&term, "a2")), "{}", shown(&term));
    let a1 = find_in_header(&term, " a1 ").expect("a1");
    let a2 = find_in_header(&term, " a2 ").expect("a2");
    assert!(a1.1 == a2.1 && a2.0 > a1.0, "two columns, one row: a1 {a1:?}, a2 {a2:?}");
    // Back to 3 × 2 (the default: the keys leave the file): one tab again, a client attached meanwhile follows.
    edit(r#"[{"grid": {"columns": 3, "rows": 2}}]"#);
    assert!(
        wait(Duration::from_secs(8), || {
            let tabs = team.tabs();
            tabs.len() == 2 && tabs[1].1 == names(&["a1", "a2", "a3", "a4"])
        }),
        "tabs not back to 3 × 2: {:?}",
        team.tabs()
    );
    assert_eq!(pids(&team), before, "a pane was started again by the grid change back");
    for member in ["lead", "a1", "a2", "a3", "a4"] {
        assert_eq!(team.launches(member).len(), 1, "{member}'s Claude was started again");
    }
    assert!(
        wait(Duration::from_secs(3), || ["a1", "a2", "a3", "a4"].iter().all(|m| has_header(&term, m))),
        "the client did not follow: {}",
        shown(&term)
    );
    drop(term);
    team.stop();
}

/// 1C: in a zoomed tab, ⌥n zooms the next member of the tab (the zoom stays, the bar names the new one), round the
/// tab and back; ⌥z brings the grid back.
#[test]
fn alt_n_keeps_the_zoom() {
    let mut team =
        Team::new("zoomn", &tables("zoomn", &["lead"], &[("dev1", None), ("dev2", None), ("dev3", None)], ""));
    team.launch(&["lead", "dev1", "dev2", "dev3"]);
    let mut term = client(&team, "lead");
    let members = ["dev1", "dev2", "dev3"];
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(wait(Duration::from_secs(3), || members.iter().all(|m| has_header(&term, m))), "{}", shown(&term));
    let mut checks = Checks::default();
    let start = active(&term, &members).pop().expect("an active member");
    let keys = alt(&term, 'z');
    term.send(&keys);
    let bar = |term: &TestTerm| term.screen().last().cloned().unwrap_or_default();
    let only = |term: &TestTerm, name: &str| {
        has_header(term, name) && members.iter().filter(|m| **m != name).all(|m| !has_header(term, m))
    };
    let zoomed = wait(Duration::from_secs(3), || only(&term, &start));
    checks.check("1C ⌥z: alone in its tab", zoomed, || shown(&term));
    // ⌥n round the tab: the next one each time, then the first again.
    let from = members.iter().position(|m| *m == start).unwrap();
    let next = alt(&term, 'n');
    for step in 1..=members.len() {
        let want = members[(from + step) % members.len()];
        term.send(&next);
        let got = wait(Duration::from_secs(3), || only(&term, want));
        checks.check(&format!("1C ⌥n #{step}: {want} alone"), got, || shown(&term));
        let named = wait(Duration::from_secs(2), || bar(&term).contains(&format!("\u{2922} {want}")));
        checks.check(&format!("1C ⌥n #{step}: the bar names {want}"), named, || bar(&term));
        let sized = team.panes().iter().find(|p| p["member"] == want).map(|p| p["cols"].as_u64().unwrap());
        checks.check(&format!("1C ⌥n #{step}: {want} has the tab's width"), sized.is_some_and(|w| w >= 110), || {
            format!("{sized:?}")
        });
        let sole = active(&term, &members);
        checks.check(&format!("1C ⌥n #{step}: {want} is the active one"), sole == [want], || format!("{sole:?}"));
    }
    // ⌥z: the grid, with the member that was zoomed active.
    let keys = alt(&term, 'z');
    term.send(&keys);
    let back = wait(Duration::from_secs(3), || members.iter().all(|m| has_header(&term, m)));
    checks.check("1C ⌥z after ⌥n: the grid is back", back, || shown(&term));
    let last = members[(from + members.len()) % members.len()];
    let sole = active(&term, &members);
    checks.check("1C the grid keeps the member last zoomed active", sole == [last], || format!("{sole:?}"));
    // Not zoomed, ⌥n only moves the focus (the grid stays).
    term.send(&next);
    std::thread::sleep(Duration::from_millis(300));
    checks
        .check("1C ⌥n out of the zoom keeps the grid", members.iter().all(|m| has_header(&term, m)), || shown(&term));
    checks.done();
    team.stop();
}

/// The reports the mod would write (`report`), told again and again: a report older than a few seconds may be
/// ignored, and the waits below last longer.
fn contexts(team: &Team, list: &[(&str, f64)]) {
    for (member, percent) in list {
        report(team, member, "opus", "high", *percent);
    }
}

/// The automatic compaction the server asked of `member` (`<state>/compact/<member>`), if its mod has not taken it.
fn compaction_asked(team: &Team, member: &str) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(team.state().join("compact").join(member)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The members whose compaction is asked.
fn compactions_asked(team: &Team, members: &[&str]) -> Vec<String> {
    members.iter().filter(|m| compaction_asked(team, m).is_some()).map(|m| m.to_string()).collect()
}

/// Waits for `check`, telling the contexts again at each try.
fn wait_with_contexts(team: &Team, list: &[(&str, f64)], timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    wait(timeout, || {
        contexts(team, list);
        check()
    })
}

/// Lets `span` pass, telling the contexts again, and says whether `check` held all along.
fn holds_with_contexts(team: &Team, list: &[(&str, f64)], span: Duration, mut check: impl FnMut() -> bool) -> bool {
    !wait_with_contexts(team, list, span, || !check())
}

/// 4: a member at rest for the idle time (`RECRUIT_AUTO_COMPACT_IDLE`, 2 s here) whose context reaches the share gets
/// one request, `"auto": true`, in `compact/<member>`; not one at the screen of a client, working, waiting, failed,
/// running a command, or under the share; and one only per period of rest.
#[test]
fn auto_compaction_is_asked_once_at_rest() {
    let mut team = Team::new(
        "autoc",
        &tables(
            "autoc",
            &["lead"],
            &[("dev1", None), ("dev2", None), ("dev3", None), ("dev4", None), ("dev5", None), ("dev6", None)],
            "",
        ),
    );
    team.env("RECRUIT_AUTO_COMPACT_IDLE", "2");
    let all = ["lead", "dev1", "dev2", "dev3", "dev4", "dev5", "dev6"];
    team.launch(&all);
    let devs = &all[1..];
    // The client looks at the first tab: lead, the dashboard and the journal.
    let mut term = client(&team, "lead");
    let mut high = [
        ("lead", 90.0),
        ("dev1", 80.0),
        ("dev2", 90.0),
        ("dev3", 90.0),
        ("dev4", 90.0),
        ("dev5", 50.0),
        ("dev6", 50.0),
    ];
    says(&team, "lead", "idle");
    says(&team, "dev1", "idle");
    says(&team, "dev2", "working");
    says(&team, "dev3", "blocked:kind=permission");
    fails(&team, "dev4", OVERLOADED);
    // The contexts come once the states are in the server's hands: a pane that has told nothing yet takes the
    // dashboard's state, which is rest, and a slow machine would ask it.
    let states = team.state().join("states.json");
    let told = wait(Duration::from_secs(15), || {
        let Some(v) =
            std::fs::read_to_string(&states).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            return false;
        };
        v["members"]["dev2"]["state"] == "working"
            && v["members"]["dev3"]["state"] == "waiting"
            && v["members"]["dev4"]["state"] == "error"
    });
    assert!(told, "the states never came: {:?}", std::fs::read_to_string(&states));
    // The command is seen by the dashboard, which polls `claude agents` every few seconds, only once the pane is at
    // rest: until then dev5's context is under the share, so the rest's clock is not what is tested.
    says(&team, "dev5", "idle");
    agent_status(&team, "dev5", "shell");
    let known = wait_with_contexts(&team, &high, Duration::from_secs(15), || {
        std::fs::read_to_string(&states)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .is_some_and(|v| v["members"]["dev5"]["shell"] == true)
    });
    assert!(known, "the dashboard never saw dev5's command: {:?}", std::fs::read_to_string(&states));
    high[5].1 = 90.0;
    says(&team, "dev6", "idle");
    let mut checks = Checks::default();
    // dev1 alone, with the share it was seen at.
    let asked = wait_with_contexts(&team, &high, Duration::from_secs(12), || compaction_asked(&team, "dev1").is_some());
    checks.check("4 dev1 at rest, 80 %, out of view: asked", asked, || format!("{:?}", compactions_asked(&team, &all)));
    if let Some(request) = compaction_asked(&team, "dev1") {
        checks.check("4 the request is automatic", request["auto"] == true, || request.to_string());
        let percent = request["percent"].as_u64();
        checks.check("4 the request says the share seen", percent == Some(80), || request.to_string());
        checks.check("4 the request lapses later", request["until"].as_i64().is_some(), || request.to_string());
    }
    // Everyone else had the same time: none is asked.
    let others = holds_with_contexts(&team, &high, Duration::from_secs(5), || {
        compactions_asked(&team, &all).iter().all(|m| m == "dev1")
    });
    let got = compactions_asked(&team, &all);
    checks.check(
        "4 lead (on screen), dev2 (works), dev3 (waits), dev4 (failed), dev5 (command), dev6 (50 %): not asked",
        others,
        || {
            format!(
                "{got:?}\nstates.json: {}",
                std::fs::read_to_string(team.state().join("states.json")).unwrap_or_default()
            )
        },
    );
    // Once per period of rest: the mod takes the request (the file goes) and makes the compaction, and nothing comes
    // back.
    let id = compaction_asked(&team, "dev1").and_then(|r| r["id"].as_str().map(String::from)).unwrap_or_default();
    let _ = std::fs::remove_file(team.state().join("compact/dev1"));
    answer_compaction(&team, "dev1", &id, serde_json::json!({"done": true}));
    let again = holds_with_contexts(&team, &high, Duration::from_secs(6), || compaction_asked(&team, "dev1").is_none());
    checks.check("4 dev1 asked once in its rest", again, || format!("{:?}", compaction_asked(&team, "dev1")));
    // A new rest, but its pane is on a client's screen: not asked; on the screen of none: asked.
    says(&team, "dev1", "working");
    std::thread::sleep(Duration::from_millis(1500));
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(wait(Duration::from_secs(3), || devs.iter().all(|m| has_header(&term, m))), "{}", shown(&term));
    says(&team, "dev1", "idle");
    let seen = holds_with_contexts(&team, &high, Duration::from_secs(6), || compaction_asked(&team, "dev1").is_none());
    checks.check("4 dev1 new rest, on a client's screen: not asked", seen, || {
        format!("{:?}", compaction_asked(&team, "dev1"))
    });
    let keys = alt(&term, '1');
    term.send(&keys);
    let gone = wait_with_contexts(&team, &high, Duration::from_secs(8), || compaction_asked(&team, "dev1").is_some());
    checks.check("4 dev1 new rest, the client left: asked", gone, || format!("{:?}", compactions_asked(&team, &all)));
    // The lead, once the client leaves for another tab, is no longer on screen: asked.
    let keys = alt(&term, '2');
    term.send(&keys);
    let lead = wait_with_contexts(&team, &high, Duration::from_secs(8), || compaction_asked(&team, "lead").is_some());
    checks.check("4 lead no longer on screen: asked", lead, || format!("{:?}", compactions_asked(&team, &all)));
    // dev2 stops working, out of view again: a rest begins; its share is 90 %.
    let keys = alt(&term, '1');
    term.send(&keys);
    says(&team, "dev2", "idle");
    let dev2 = wait_with_contexts(&team, &high, Duration::from_secs(8), || compaction_asked(&team, "dev2").is_some());
    checks.check("4 dev2 at rest after its work: asked", dev2, || format!("{:?}", compactions_asked(&team, &all)));
    checks.done();
    drop(term);
    team.stop();
}

/// 4: `auto_compact = false` asks nothing, whatever the context; `auto_compact = 85` waits for 85 %; and the switch
/// changed on the running team (`recruit _edit`) is followed.
#[test]
fn auto_compaction_follows_the_settings() {
    let mut off = Team::new("autooff", &tables("autooff", &["lead"], &[("dev", None)], "auto_compact = false"));
    off.env("RECRUIT_AUTO_COMPACT_IDLE", "2");
    off.launch(&["lead", "dev"]);
    let high = [("lead", 95.0), ("dev", 95.0)];
    contexts(&off, &high);
    says(&off, "dev", "idle");
    says(&off, "lead", "idle");
    let quiet = holds_with_contexts(&off, &high, Duration::from_secs(7), || {
        compactions_asked(&off, &["lead", "dev"]).is_empty()
    });
    assert!(quiet, "auto_compact = false, asked: {:?}", compactions_asked(&off, &["lead", "dev"]));
    // Switched on while they rest: the share applies from the next try.
    let state = off.state();
    let out = off.recruit([
        std::ffi::OsStr::new("_edit"),
        state.as_os_str(),
        std::ffi::OsStr::new(r#"[{"auto_compact": 70}]"#),
    ]);
    assert!(out.status.success(), "_edit: {}", String::from_utf8_lossy(&out.stderr));
    let asked = wait_with_contexts(&off, &high, Duration::from_secs(10), || compaction_asked(&off, "dev").is_some());
    assert!(asked, "switched on at 70 %, nothing asked: {:?}", compactions_asked(&off, &["lead", "dev"]));
    off.stop();

    let mut share = Team::new("autoshare", &tables("autoshare", &["lead"], &[("dev", None)], "auto_compact = 85"));
    share.env("RECRUIT_AUTO_COMPACT_IDLE", "2");
    share.launch(&["lead", "dev"]);
    let under = [("lead", 10.0), ("dev", 80.0)];
    contexts(&share, &under);
    says(&share, "dev", "idle");
    let quiet =
        holds_with_contexts(&share, &under, Duration::from_secs(6), || compaction_asked(&share, "dev").is_none());
    assert!(quiet, "80 % under auto_compact = 85, asked: {:?}", compaction_asked(&share, "dev"));
    let over = [("lead", 10.0), ("dev", 86.0)];
    let asked =
        wait_with_contexts(&share, &over, Duration::from_secs(10), || compaction_asked(&share, "dev").is_some());
    assert!(asked, "86 % over auto_compact = 85, nothing asked");
    let request = compaction_asked(&share, "dev").expect("request");
    assert_eq!(request["percent"].as_u64(), Some(86), "{request}");
    share.stop();
}

/// Where the quick list (a click on the model or the effort of a header) is on the client's screen: its top left cell
/// and its size, which the menu in list mode announced (`CSI 8 ; rows ; columns t`) and the server gave its pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListBox {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl ListBox {
    fn right(&self) -> usize {
        self.x + self.w - 1
    }

    fn bottom(&self) -> usize {
        self.y + self.h - 1
    }
}

/// The quick list as drawn: found by its first row (« défaut : … »), its frame checked against the size
/// its pane has. None while it is not (all) there.
fn list_box(team: &Team, term: &TestTerm) -> Option<ListBox> {
    let panes = team.panes();
    let menu = panes.iter().find(|p| p["role"] == "menu")?;
    let (w, h) = (menu["cols"].as_u64()? as usize, menu["rows"].as_u64()? as usize);
    let screen = term.screen();
    let (row, line) = screen.iter().enumerate().find(|(_, line)| line.contains("défaut"))?;
    let label = unicode_width::UnicodeWidthStr::width(&line[..line.find("défaut")?]);
    // « │ › défaut »: the frame, a blank, the cursor, a blank.
    let found = ListBox { x: label.checked_sub(4)?, y: row.checked_sub(1)?, w, h };
    let corner = |col: usize, row: usize| term.cell(col, row).0;
    let framed = corner(found.x, found.y) == "╭"
        && corner(found.right(), found.y) == "╮"
        && corner(found.x, found.bottom()) == "╰"
        && corner(found.right(), found.bottom()) == "╯";
    framed.then_some(found)
}

/// Waits for the quick list, 8 s at most.
fn wait_list(team: &Team, term: &TestTerm) -> Option<ListBox> {
    let mut seen = None;
    wait(Duration::from_secs(8), || {
        seen = list_box(team, term);
        seen.is_some()
    });
    seen
}

/// Waits until the quick list is gone: its pane closed and nothing of it on the screen.
fn wait_list_gone(team: &Team, term: &TestTerm) -> bool {
    wait(Duration::from_secs(5), || menu_text(team).is_none() && !shown(term).contains("défaut"))
}

/// A mouse move to `(col, row)`, as the terminal reports it with every motion on (`?1003`).
fn hover(term: &mut TestTerm, (col, row): (usize, usize)) {
    term.send(format!("\x1b[<35;{};{}M", col + 1, row + 1).as_bytes());
}

/// What a cell looks like: its text color, its background and its attributes.
fn look_of(term: &TestTerm, (col, row): (usize, usize)) -> (String, String, String) {
    let cell = term.cell(col, row);
    (cell.1, cell.2, term.flags(col, row))
}

/// The looks of what the quick list must leave as it is: lead's text, the dashboard's title, the bar's first cells.
fn untouched(term: &TestTerm) -> Vec<(String, String, String)> {
    let mut cells = Vec::new();
    for (text, len) in [("FAKE CLAUDE", 11), ("Tableau de bord", 15)] {
        if let Some((col, row)) = (0..term.screen().len()).find_map(|r| {
            let line = term.screen()[r].clone();
            line.find(text).map(|at| (unicode_width::UnicodeWidthStr::width(&line[..at]), r))
        }) {
            cells.extend((0..len).map(|i| look_of(term, (col + i, row))));
        }
    }
    let bar = term.screen().len() - 1;
    cells.extend((0..8).map(|i| look_of(term, (i, bar))));
    cells
}

/// The row of the list where the cursor `›` is, among the rows of `list`.
fn list_cursor(term: &TestTerm, list: ListBox) -> Option<usize> {
    let screen = term.screen();
    (list.y + 1..list.bottom()).find(|row| term.cell(list.x + 2, *row).0 == "›" && screen[*row].contains('›'))
}

/// The team's project settings file, as it is now.
fn team_file(team: &Team) -> String {
    std::fs::read_to_string(team.proj.join(".recruit/settings.toml")).expect("settings.toml")
}

/// The team with `lead` and `dev` (in the second tab), their models and efforts told as the mod would, and a client
/// of 120 × 40 that has seen the headers learn them.
fn quick_team(name: &str, extra: &str) -> (Team, TestTerm) {
    let team = Team::new(name, &tables(name, &["lead"], &[("dev", None)], extra));
    team.launch(&["lead", "dev"]);
    report(&team, "lead", "claude-opus-5-5", "high", 40.0);
    report(&team, "dev", "claude-opus-5-5", "high", 40.0);
    says(&team, "lead", "idle");
    says(&team, "dev", "idle");
    let term = client(&team, "lead");
    assert!(wait(Duration::from_secs(8), || find_in_header(&term, "Opus").is_some()), "{}", shown(&term));
    (team, term)
}

/// 3: a click on the model or the effort of a header opens, under the text, the list of its values, at the size the
/// menu announces, the text clicked in reverse video and nothing dimmed; Esc, a click outside or a second click on the
/// text close it with nothing changed and the focus where it was; ↑↓ and ⏎ (or a click on a value) write the value in
/// the file that gives it and the header follows; the pointer moving over the values moves the cursor.
#[test]
fn quick_list_chooses_a_model_and_an_effort() {
    let (team, mut term) = quick_team("quick", "");
    let members = ["lead", "dev"];
    let mut checks = Checks::default();
    let before_file = team_file(&team);
    let before_active = active(&term, &members);
    let before_untouched = untouched(&term);
    let model = find_in_header(&term, "Opus").expect("the model");
    let model_before = look_of(&term, model);
    let open = |term: &mut TestTerm, at: (usize, usize)| {
        click(term, at);
        wait_list(&team, term)
    };

    // Opens under the text, at the size announced, the text in reverse, nothing dimmed.
    let Some(list) = open(&mut term, model) else { panic!("no list: {}", shown(&term)) };
    checks.check("3 under the model: its row is the next", list.y == model.1 + 1, || format!("{list:?} {model:?}"));
    checks.check("3 under the model: the values start at its column", list.x + 4 == model.0, || {
        format!("{list:?} {model:?}")
    });
    println!(
        "CLICKED STYLES: {:?}",
        (0..4).map(|i| (term.cell(model.0 + i, model.1), term.flags(model.0 + i, model.1))).collect::<Vec<_>>()
    );
    println!(
        "HEADER STYLES before? other cells: {:?}",
        (0..4).map(|i| (term.cell(model.0 - 8 + i, model.1), term.flags(model.0 - 8 + i, model.1))).collect::<Vec<_>>()
    );
    let neighbor = find_in_header(&term, "élevé").expect("the effort beside the model");
    checks.check(
        "3 the model lit: not styled as the text beside it",
        (0..4).all(|i| look_of(&term, (model.0 + i, model.1)) != look_of(&term, (neighbor.0 + i, neighbor.1))),
        || format!("{:?} against {:?}", look_of(&term, model), look_of(&term, neighbor)),
    );
    checks.check("3 the model lit: not as it was", look_of(&term, model) != model_before, || {
        format!("{:?}", model_before)
    });
    checks.check("3 nothing dimmed: the rest looks as it did", untouched(&term) == before_untouched, || {
        format!("{:?}\n{:?}", untouched(&term), before_untouched)
    });
    let body = shown(&term);
    for value in ["défaut", "Opus", "Sonnet", "Haiku", "Fable", "Autre…", "actuel"] {
        checks.check(&format!("3 the list says {value}"), body.contains(value), || body.clone());
    }
    // Hover: the cursor goes to the value under the pointer (rows y+1.. are the values).
    let rows: Vec<usize> = (list.y + 1..list.bottom()).collect();
    for (n, row) in rows.iter().enumerate().skip(1).take(3) {
        hover(&mut term, (list.x + 8, *row));
        let moved = wait(Duration::from_secs(2), || list_cursor(&term, list) == Some(*row));
        checks.check(&format!("3 hover #{n}: the cursor follows"), moved, || {
            format!("at {:?} for {row}\n{}", list_cursor(&term, list), shown(&term))
        });
    }
    // Esc closes: nothing changed, the focus back, no reverse video.
    term.send(b"\x1b");
    checks.check("3 Esc closes", wait_list_gone(&team, &term), || shown(&term));
    checks.check("3 Esc: the file is as it was", team_file(&team) == before_file, || team_file(&team));
    checks.check(
        "3 Esc: the focus is back",
        wait(Duration::from_secs(2), || active(&term, &members) == before_active),
        || format!("{:?}", active(&term, &members)),
    );
    checks.check(
        "3 Esc: no reverse video left",
        wait(Duration::from_secs(2), || look_of(&term, model) == model_before),
        || shown(&term),
    );
    // A click outside, in lead's own pane.
    if open(&mut term, model).is_some() {
        click(&mut term, (10, 20));
        checks.check("3 a click outside closes", wait_list_gone(&team, &term), || shown(&term));
        checks.check("3 outside: the file is as it was", team_file(&team) == before_file, || team_file(&team));
        checks.check(
            "3 outside: the focus is back",
            wait(Duration::from_secs(2), || active(&term, &members) == before_active),
            || format!("{:?}", active(&term, &members)),
        );
    } else {
        checks.check("3 the list opens again", false, || shown(&term));
    }
    // A second click on the text.
    if open(&mut term, model).is_some() {
        click(&mut term, (model.0 + 1, model.1));
        checks.check("3 a second click on the text closes", wait_list_gone(&team, &term), || shown(&term));
        checks.check("3 second click: the file is as it was", team_file(&team) == before_file, || team_file(&team));
        checks.check(
            "3 second click: the focus is back",
            wait(Duration::from_secs(2), || active(&term, &members) == before_active),
            || format!("{:?}", active(&term, &members)),
        );
    } else {
        checks.check("3 the list opens a third time", false, || shown(&term));
    }
    // ↓↓ ⏎: « default » is in place, Opus then Sonnet.
    if let Some(list) = open(&mut term, model) {
        term.send(b"\x1b[B\x1b[B");
        let down = wait(Duration::from_secs(2), || list_cursor(&term, list) == Some(list.y + 3));
        checks.check("3 ↓↓ goes to the third value", down, || format!("{:?}", list_cursor(&term, list)));
        term.send(b"\r");
        checks.check("3 ⏎ closes", wait_list_gone(&team, &term), || shown(&term));
        let written = wait(Duration::from_secs(3), || team_file(&team).contains("model = \"sonnet\""));
        checks.check("3 ⏎ writes model = \"sonnet\" for lead", written, || team_file(&team));
        let lead = team_file(&team);
        let table = lead.split("[teams.quick.members.").find(|t| t.starts_with("lead]")).unwrap_or_default();
        checks.check("3 the value is in lead's table", table.contains("model = \"sonnet\""), || lead.clone());
        checks.check(
            "3 dev's table is left alone",
            !lead.split("members.dev]").nth(1).unwrap_or("").contains("model"),
            || lead.clone(),
        );
        let followed = wait(Duration::from_secs(5), || {
            term.screen().first().is_some_and(|r| r.contains("Sonnet") && !r.contains("Opus"))
        });
        checks.check("3 the header follows: Sonnet", followed, || shown(&term));
        checks.check(
            "3 ⏎: the focus is back",
            wait(Duration::from_secs(2), || active(&term, &members) == before_active),
            || format!("{:?}", active(&term, &members)),
        );
    } else {
        checks.check("3 the list opens for the choice", false, || shown(&term));
    }
    // The effort: its own list, with the levels.
    let effort = find_in_header(&term, "élevé").expect("the effort");
    let effort_before = look_of(&term, effort);
    if let Some(list) = open(&mut term, effort) {
        let body = shown(&term);
        for level in ["faible", "moyen", "élevé", "extra", "max"] {
            checks.check(&format!("3 the effort list says {level}"), body.contains(level), || body.clone());
        }
        checks.check("3 under the effort: its row is the next", list.y == effort.1 + 1, || {
            format!("{list:?} {effort:?}")
        });
        checks.check("3 the effort lit", look_of(&term, effort) != effort_before, || shown(&term));
        term.send(b"\x1b[B");
        term.send(b"\r");
        let written = wait(Duration::from_secs(3), || team_file(&team).contains("effort = \"low\""));
        checks.check("3 ⏎ writes effort = \"low\"", written, || team_file(&team));
        let followed = wait(Duration::from_secs(5), || term.screen().first().is_some_and(|r| r.contains("faible")));
        checks.check("3 the header follows: low", followed, || shown(&term));
    } else {
        checks.check("3 the effort list opens", false, || shown(&term));
    }
    // A click on a value chooses it.
    let model = find_in_header(&term, "Sonnet").expect("the model");
    if let Some(list) = open(&mut term, model) {
        let haiku = (list.y + 1..list.bottom()).find(|row| term.screen()[*row].contains("Haiku"));
        match haiku {
            Some(row) => {
                click(&mut term, (list.x + 8, row));
                let written = wait(Duration::from_secs(3), || team_file(&team).contains("model = \"haiku\""));
                checks.check("3 a click on a value writes it", written, || team_file(&team));
            }
            None => checks.check("3 Haiku in the list", false, || shown(&term)),
        }
    }
    checks.done();
    drop(term);
    let mut team = team;
    team.stop();
}

/// 3: the list slides left to stay on the screen (the model of the last of three columns), and opens above the text
/// when there is no room below (the header of the bottom pane).
#[test]
fn quick_list_stays_on_the_screen() {
    let mut checks = Checks::default();
    // Three columns of a 120-column screen: the third one's effort is near the right edge.
    let mut team = Team::new(
        "quickedge",
        &tables("quickedge", &["lead"], &[("a1", None), ("a2", None), ("a3", None)], "columns = 3\nrows = 1"),
    );
    team.launch(&["lead", "a1", "a2", "a3"]);
    for m in ["lead", "a1", "a2", "a3"] {
        report(&team, m, "claude-opus-5-5", "high", 40.0);
        says(&team, m, "idle");
    }
    let mut term = team.attach(120, 24, Profile::default());
    assert!(term.wait_for("lead", Duration::from_secs(5)));
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(
        wait(Duration::from_secs(8), || ["a1", "a2", "a3"].iter().all(|m| has_header(&term, m))),
        "{}",
        shown(&term)
    );
    assert!(wait(Duration::from_secs(8), || term.screen()[0].matches("Opus").count() == 3), "{}", shown(&term));
    let row0 = term.screen()[0].clone();
    // The columns are narrow: the effort shows as its sign only.
    let at = row0.rfind('\u{25cf}').expect("the last effort");
    let effort = (unicode_width::UnicodeWidthStr::width(&row0[..at]), 0);
    let effort_before = look_of(&term, effort);
    click(&mut term, effort);
    match wait_list(&team, &term) {
        Some(list) => {
            checks
                .check("3 near the right edge: ends at the screen's edge", list.right() == 119, || format!("{list:?}"));
            checks.check("3 near the right edge: slid left of the text", list.x + 4 < effort.0, || {
                format!("{list:?} {effort:?}")
            });
            checks.check("3 near the right edge: below the text", list.y == 1, || format!("{list:?}"));
            checks
                .check("3 near the right edge: the text lit", look_of(&term, effort) != effort_before, || shown(&term));
        }
        None => checks.check("3 the list opens at the right edge", false, || shown(&term)),
    }
    term.send(b"\x1b");
    checks.check("3 the list at the edge closes", wait_list_gone(&team, &term), || shown(&term));
    drop(term);
    team.stop();

    // One column of three panes in 20 rows: the bottom pane's header has no room below.
    let mut team = Team::new(
        "quickbottom",
        &tables("quickbottom", &["lead"], &[("a1", None), ("a2", None), ("a3", None)], "columns = 1\nrows = 3"),
    );
    team.launch(&["lead", "a1", "a2", "a3"]);
    for m in ["lead", "a1", "a2", "a3"] {
        report(&team, m, "claude-opus-5-5", "high", 40.0);
        says(&team, m, "idle");
    }
    let mut term = team.attach(140, 20, Profile::default());
    assert!(term.wait_for("lead", Duration::from_secs(5)));
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(
        wait(Duration::from_secs(8), || ["a1", "a2", "a3"].iter().all(|m| has_header(&term, m))),
        "{}",
        shown(&term)
    );
    let bottom = wait(Duration::from_secs(8), || {
        term.screen().iter().filter(|r| r.contains("Opus") && r.contains(" a")).count() == 3
    });
    assert!(bottom, "{}", shown(&term));
    let screen = term.screen();
    let row = screen.iter().rposition(|r| r.contains("Opus") && r.contains(" a3 ")).expect("a3's header");
    let col = unicode_width::UnicodeWidthStr::width(&screen[row][..screen[row].find("Opus").unwrap()]);
    let text_before = look_of(&term, (col, row));
    click(&mut term, (col, row));
    match wait_list(&team, &term) {
        Some(list) => {
            checks.check("3 at the bottom: the list ends just above the text", list.bottom() + 1 == row, || {
                format!("{list:?} header on row {row}")
            });
            checks.check(
                "3 at the bottom: the values start at the text's column, or end at the screen's edge",
                list.x + 4 == col || list.right() == 139,
                || format!("{list:?} col {col}"),
            );
            checks.check("3 at the bottom: the text lit", look_of(&term, (col, row)) != text_before, || shown(&term));
        }
        None => checks.check("3 the list opens at the bottom", false, || shown(&term)),
    }
    term.send(b"\x1b");
    checks.check("3 the list at the bottom closes", wait_list_gone(&team, &term), || shown(&term));
    drop(term);
    team.stop();
    checks.done();
}

/// Makes `dir` read-only, and writable again when dropped: the test's folder goes away either way.
struct ReadOnly(std::path::PathBuf);

impl ReadOnly {
    fn new(dir: std::path::PathBuf) -> ReadOnly {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).expect("read-only");
        ReadOnly(dir)
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

/// 3: a value that starts a member that works again asks first, inside the list, on « Annuler »; Annuler changes
/// nothing, « Relancer » writes the value and starts the member again on its conversation; a write that is refused
/// leaves the list open with the reason.
#[test]
fn quick_list_asks_and_refuses() {
    let toml = tables("quickask", &["lead"], &[("dev", None)], "")
        .replace("role = \"Agent dev\"\n", "role = \"Agent dev\"\nmodel = \"opus\"\n");
    let mut team = Team::new("quickask", &toml);
    team.launch(&["lead", "dev"]);
    report(&team, "lead", "claude-opus-5-5", "high", 40.0);
    report(&team, "dev", "claude-opus-5-5", "high", 40.0);
    says(&team, "lead", "idle");
    says(&team, "dev", "working");
    let mut term = client(&team, "lead");
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(
        wait(Duration::from_secs(8), || has_header(&term, "dev") && find_in_header(&term, "Opus").is_some()),
        "{}",
        shown(&term)
    );
    let session = noted(&team, "dev");
    let mut checks = Checks::default();
    let before_file = team_file(&team);
    let model = find_in_header(&term, "Opus").expect("dev's model");
    // Back to the default starts it again (↻): the question opens in the list, on Annuler.
    click(&mut term, model);
    assert!(wait_list(&team, &term).is_some(), "no list: {}", shown(&term));
    checks.check(
        "3 the default row says ↻",
        shown(&term).lines().any(|r| r.contains("défaut") && r.contains('↻')),
        || shown(&term),
    );
    term.send(b"\x1b[A");
    term.send(b"\r");
    let asked = wait(Duration::from_secs(3), || shown(&term).contains("Annuler"));
    checks.check("3 working + ↻: the question opens", asked, || shown(&term));
    let at = find(&term, "Annuler");
    if let Some(at) = at {
        let inside = list_box(&team, &term).is_some_and(|l| at.0 > l.x && at.0 < l.right() && at.1 < l.y + l.h);
        checks.check("3 the question is in the list", inside, || shown(&term));
        checks.check("3 Annuler is the chosen button", term.flags(at.0 + 2, at.1).contains('b'), || {
            term.flags(at.0 + 2, at.1)
        });
        let yes = shown(&term).lines().find(|r| r.contains("Relancer (r)")).map(String::from);
        checks.check("3 the other button is not", yes.is_some(), || shown(&term));
    }
    term.send(b"\r");
    checks.check("3 ⏎ on Annuler closes", wait_list_gone(&team, &term), || shown(&term));
    checks.check("3 Annuler: the file is as it was", team_file(&team) == before_file, || team_file(&team));
    checks.check("3 Annuler: dev was not started again", team.launches("dev").len() == 1, || {
        format!("{:?}", team.launches("dev"))
    });
    // Again, and « Relancer ».
    click(&mut term, model);
    if wait_list(&team, &term).is_some() {
        term.send(b"\x1b[A");
        term.send(b"\r");
        let _ = wait(Duration::from_secs(3), || shown(&term).contains("Annuler"));
        term.send(b"r");
        let again = wait(Duration::from_secs(10), || team.launches("dev").len() == 2);
        checks.check("3 Relancer: dev starts again", again, || format!("{:?}", team.launches("dev")));
        if again {
            let second = team.launches("dev")[1].clone();
            checks.check("3 Relancer: on its conversation", second.ends_with(&format!("-n dev -r {session}")), || {
                second.clone()
            });
        }
        let written = wait(Duration::from_secs(3), || !team_file(&team).contains("model = \"opus\""));
        checks.check("3 Relancer: the default leaves the key out", written, || team_file(&team));
        checks.check("3 Relancer: the list closes", wait_list_gone(&team, &term), || shown(&term));
    } else {
        checks.check("3 the list opens again", false, || shown(&term));
    }
    // A write that is refused: the list stays, with the reason.
    let keys = alt(&term, '1');
    term.send(&keys);
    assert!(wait(Duration::from_secs(5), || has_header(&term, "lead")), "{}", shown(&term));
    let lead = find_in_header(&term, "Opus").expect("lead's model");
    let _read_only = ReadOnly::new(team.proj.join(".recruit"));
    click(&mut term, lead);
    if wait_list(&team, &term).is_some() {
        term.send(b"\x1b[B");
        term.send(b"\r");
        let refused = wait(Duration::from_secs(5), || shown(&term).contains("✗"));
        checks.check("3 refused: the reason shows", refused, || shown(&term));
        checks.check("3 refused: the list stays open", list_box(&team, &term).is_some(), || shown(&term));
        checks.check("3 refused: the file is as it was", team_file(&team).contains("[teams.quickask]"), || {
            team_file(&team)
        });
        term.send(b"\x1b");
        checks.check("3 refused: Esc closes", wait_list_gone(&team, &term), || shown(&term));
    } else {
        checks.check("3 the list opens for a refusal", false, || shown(&term));
    }
    drop(_read_only);
    checks.done();
    drop(term);
    team.stop();
}

/// What `_ctl panes` says of `member`'s pane: the state its program told (`status`) and since when (`since`, seconds).
fn pane_status(team: &Team, member: &str) -> (Option<String>, Option<u64>) {
    let panes = team.panes();
    let pane = panes.iter().find(|p| p["member"] == member);
    (pane.and_then(|p| p["status"].as_str().map(String::from)), pane.and_then(|p| p["since"].as_u64()))
}

/// The spinner of a member at work, in any of its steps.
fn spinning(text: &str) -> bool {
    text.chars().any(|c| "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(c))
}

/// 4: the `working` that Claude Code says while it compacts is no turn: the member stays at rest (header, bar, `_ctl
/// panes` with the same `since`) until its mod answers. Made: the `working` is dropped, and the `idle` that follows
/// changes nothing. Refused for helpers, or failed: the `working` is a turn after all, told since it began.
#[test]
fn a_compaction_holds_the_working() {
    let mut team =
        Team::new("autohold", &tables("autohold", &["lead"], &[("dev1", None), ("dev2", None), ("dev3", None)], ""));
    team.env("RECRUIT_AUTO_COMPACT_IDLE", "2");
    let devs = ["dev1", "dev2", "dev3"];
    team.launch(&["lead", "dev1", "dev2", "dev3"]);
    let high = [("lead", 10.0), ("dev1", 85.0), ("dev2", 85.0), ("dev3", 85.0)];
    for member in ["lead", "dev1", "dev2", "dev3"] {
        says(&team, member, "idle");
    }
    let told =
        wait(Duration::from_secs(15), || devs.iter().all(|m| pane_status(&team, m).0.as_deref() == Some("idle")));
    assert!(told, "the states never came: {:?}", team.panes());
    contexts(&team, &high);
    let asked = wait_with_contexts(&team, &high, Duration::from_secs(15), || {
        devs.iter().all(|m| compaction_asked(&team, m).is_some())
    });
    assert!(asked, "not all asked: {:?}", compactions_asked(&team, &devs));
    let id = |member: &str| compaction_asked(&team, member).and_then(|r| r["id"].as_str().map(String::from)).unwrap();
    let ids: Vec<String> = devs.iter().map(|m| id(m)).collect();
    // The mod takes the requests.
    for member in devs {
        let _ = std::fs::remove_file(team.state().join("compact").join(member));
    }
    let rest: Vec<Option<u64>> = devs.iter().map(|m| pane_status(&team, m).1).collect();
    assert!(rest.iter().all(Option::is_some), "no rest start: {:?}", team.panes());
    // Claude Code compacts: `working`, which is no turn.
    let began = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    for member in devs {
        says(&team, member, "working");
    }
    let mut checks = Checks::default();
    let held = holds_with_contexts(&team, &high, Duration::from_millis(2500), || {
        devs.iter().zip(&rest).all(|(m, since)| {
            let (status, at) = pane_status(&team, m);
            status.as_deref() == Some("idle") && at == *since
        })
    });
    checks
        .check("4 compacting: `_ctl panes` stay at rest, with the same since", held, || format!("{:?}", team.panes()));
    // The screen: no spinner in the headers, the bar counts them at rest.
    let mut term = team.attach(140, 40, Profile::default());
    assert!(term.wait_for("lead", Duration::from_secs(5)));
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(wait(Duration::from_secs(5), || devs.iter().all(|m| has_header(&term, m))), "{}", shown(&term));
    let headers: Vec<String> = term
        .screen()
        .into_iter()
        .filter(|row| {
            row.chars().any(|c| matches!(c, '╭' | '┏')) && devs.iter().any(|m| row.contains(&format!(" {m} ")))
        })
        .collect();
    checks.check("4 compacting: no spinner in the headers", headers.len() == 1 && !spinning(&headers[0]), || {
        shown(&term)
    });
    let bar = bar_of(&term);
    checks.check("4 compacting: no spinner in the bar", !spinning(&bar), || bar.clone());
    // dev1: made. The held `working` is dropped; the `idle` that follows changes nothing.
    answer_compaction(&team, "dev1", &ids[0], serde_json::json!({"done": true}));
    says(&team, "dev1", "idle");
    // dev2: refused for helpers; dev3: failed. Their `working` was a turn: told since it began.
    answer_compaction(&team, "dev2", &ids[1], serde_json::json!({"skip": "helpers"}));
    answer_compaction(&team, "dev3", &ids[2], serde_json::json!({"error": "boom"}));
    let turn = wait_with_contexts(&team, &high, Duration::from_secs(10), || {
        ["dev2", "dev3"].iter().all(|m| pane_status(&team, m).0.as_deref() == Some("working"))
    });
    checks.check("4 refused or failed: the held working applies", turn, || format!("{:?}", team.panes()));
    for member in ["dev2", "dev3"] {
        let since = pane_status(&team, member).1.unwrap_or_default();
        checks.check(
            &format!("4 {member}: the working is told since it began"),
            since >= began && since <= began + 4,
            || format!("since {since}, working told at {began}"),
        );
    }
    std::thread::sleep(Duration::from_millis(1500));
    let (status, since) = pane_status(&team, "dev1");
    checks.check(
        "4 made: dev1 stays at rest, since unchanged",
        status.as_deref() == Some("idle") && since == rest[0],
        || format!("{status:?} {since:?} (rest since {:?})", rest[0]),
    );
    let working =
        wait(Duration::from_secs(8), || term.screen().iter().any(|row| row.contains(" dev2 ") && spinning(row)));
    checks.check("4 refused: the header shows the work", working, || shown(&term));
    checks.done();
    drop(term);
    team.stop();
}

/// 4, found with real Claude Code on 2026-10-11 (`claude_auto_compaction`): a message typed while a member compacts is
/// queued, and its turn starts as the compaction ends, with no `done` and no `working` of its own: Claude Code says
/// `working` once at the compaction's start and `done` once at the turn's end. The mod answers the compaction (`done`)
/// in between. The member must then show at work until that `done` (with a `since` between the compaction's start and
/// its answer); a compaction alone, whose `done` comes before the mod's answer, stays at rest all along.
#[test]
fn a_turn_queued_during_a_compaction_shows_at_work() {
    let mut team = Team::new("autoqueue", &tables("autoqueue", &["lead"], &[("dev1", None), ("dev2", None)], ""));
    team.env("RECRUIT_AUTO_COMPACT_IDLE", "2");
    let devs = ["dev1", "dev2"];
    team.launch(&["lead", "dev1", "dev2"]);
    let high = [("lead", 10.0), ("dev1", 85.0), ("dev2", 85.0)];
    for member in ["lead", "dev1", "dev2"] {
        says(&team, member, "idle");
    }
    let told =
        wait(Duration::from_secs(15), || devs.iter().all(|m| pane_status(&team, m).0.as_deref() == Some("idle")));
    assert!(told, "the states never came: {:?}", team.panes());
    contexts(&team, &high);
    let asked = wait_with_contexts(&team, &high, Duration::from_secs(15), || {
        devs.iter().all(|m| compaction_asked(&team, m).is_some())
    });
    assert!(asked, "not all asked: {:?}", compactions_asked(&team, &devs));
    let id = |member: &str| compaction_asked(&team, member).and_then(|r| r["id"].as_str().map(String::from)).unwrap();
    let ids: Vec<String> = devs.iter().map(|m| id(m)).collect();
    for member in devs {
        let _ = std::fs::remove_file(team.state().join("compact").join(member));
    }
    let rest: Vec<Option<u64>> = devs.iter().map(|m| pane_status(&team, m).1).collect();
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // Both compact: `working`, once.
    let began = now();
    for member in devs {
        says(&team, member, "working");
    }
    let mut checks = Checks::default();
    let held = holds_with_contexts(&team, &high, Duration::from_millis(2000), || {
        devs.iter().zip(&rest).all(|(m, since)| {
            let (status, at) = pane_status(&team, m);
            status.as_deref() == Some("idle") && at == *since
        })
    });
    checks.check("4 compacting: both at rest", held, || format!("{:?}", team.panes()));
    // dev2, a compaction alone: its `done` comes first, the mod's answer after. At rest all along.
    says(&team, "dev2", "done");
    std::thread::sleep(Duration::from_millis(600));
    answer_compaction(&team, "dev2", &ids[1], serde_json::json!({"done": true}));
    let alone = holds_with_contexts(&team, &high, Duration::from_millis(3500), || {
        let (status, at) = pane_status(&team, "dev2");
        status.as_deref() != Some("working") && at == rest[1] || status.as_deref() == Some("idle")
    });
    checks.check("4 a compaction alone stays at rest", alone, || format!("{:?}", team.panes()));
    // dev1, a turn queued behind it: the mod answers, no `done` has come: at work until it does.
    let answered = now();
    answer_compaction(&team, "dev1", &ids[0], serde_json::json!({"done": true}));
    let works = wait_with_contexts(&team, &high, Duration::from_secs(8), || {
        pane_status(&team, "dev1").0.as_deref() == Some("working")
    });
    checks.check("4 queued turn: at work once the compaction is answered", works, || format!("{:?}", team.panes()));
    let since = pane_status(&team, "dev1").1.unwrap_or_default();
    checks.check(
        "4 queued turn: a since between the compaction's start and a moment after its answer",
        since >= began && since <= answered + 4,
        || format!("since {since}, started {began}, answered {answered}"),
    );
    // The header of the member shows the work.
    let mut term = team.attach(140, 40, Profile::default());
    assert!(term.wait_for("lead", Duration::from_secs(5)));
    let keys = alt(&term, '2');
    term.send(&keys);
    assert!(wait(Duration::from_secs(5), || devs.iter().all(|m| has_header(&term, m))), "{}", shown(&term));
    let spinner = wait(Duration::from_secs(5), || {
        term.screen()
            .iter()
            .any(|row| row.contains(" dev1 ") && row.chars().any(|c| matches!(c, '╭' | '┏')) && spinning(row))
    });
    checks.check("4 queued turn: the header shows the work", spinner, || shown(&term));
    // It stays so, for as long as the turn lasts.
    let stays = holds_with_contexts(&team, &high, Duration::from_millis(3000), || {
        pane_status(&team, "dev1").0.as_deref() == Some("working")
    });
    checks.check("4 queued turn: still at work 3 s later", stays, || format!("{:?}", team.panes()));
    // Its `done`: at rest, since now.
    let ended = now();
    says(&team, "dev1", "done");
    let rested = wait_with_contexts(&team, &high, Duration::from_secs(5), || {
        pane_status(&team, "dev1").0.as_deref() == Some("idle")
    });
    checks.check("4 queued turn: its done brings the rest", rested, || format!("{:?}", team.panes()));
    let (_, after) = pane_status(&team, "dev1");
    checks.check(
        "4 queued turn: the rest begins with that done",
        after.is_some_and(|a| a >= ended.saturating_sub(1)),
        || format!("{after:?} against {ended}"),
    );
    checks.done();
    drop(term);
    team.stop();
}

/// The tab in « focus » (`layout = "focus"`, ⌥l): six agents, the active one on the left, three panels on the right, the
/// two others as cards in « others ». A member in a card that waits, or whose turn failed, enters at once, over the
/// minute a panel keeps its place; the one it takes the place of goes to the cards. (Decision of 2026-10-11 : a member
/// at work respects the minute: `focus_a_member_at_work_waits_its_turn`, which is long.)
#[test]
fn focus_a_member_that_waits_or_failed_enters_at_once() {
    let tables = tables(
        "focus",
        &["lead"],
        &[
            ("a1", Some("crew")),
            ("a2", Some("crew")),
            ("a3", Some("crew")),
            ("a4", Some("crew")),
            ("a5", Some("crew")),
            ("a6", Some("crew")),
        ],
        "layout = \"focus\"",
    );
    let mut team = Team::new("focus", &tables);
    let members = ["lead", "a1", "a2", "a3", "a4", "a5", "a6"];
    team.launch(&members);
    for member in members {
        report(&team, member, "claude-opus-5-5", "high", 40.0);
    }
    for (member, state) in [
        ("lead", "idle"),
        ("a1", "working"),
        ("a2", "working"),
        ("a3", "working"),
        ("a4", "idle"),
        ("a5", "idle"),
        ("a6", "idle"),
    ] {
        says(&team, member, state);
    }
    let mut term = client(&team, "lead");
    let keys = alt(&term, '2');
    term.send(&keys);
    let shown_panels = |term: &TestTerm| -> Vec<String> {
        ["a1", "a2", "a3", "a4", "a5", "a6"].iter().filter(|m| has_header(term, m)).map(|m| m.to_string()).collect()
    };
    // Four panels (the active one and three), two cards.
    let laid = wait(Duration::from_secs(8), || shown_panels(&term).len() == 4);
    assert!(laid, "focus: four panels expected: {:?}\n{}", shown_panels(&term), shown(&term));
    let panels = shown_panels(&term);
    let card = ["a1", "a2", "a3", "a4", "a5", "a6"].into_iter().find(|m| !panels.contains(&m.to_string())).unwrap();
    let mut checks = Checks::default();
    // A card that waits enters at once.
    says(&team, card, "blocked:kind=permission");
    let entered = wait(Duration::from_secs(4), || has_header(&term, card));
    checks.check(&format!("focus: {card}, a card, waits: its panel shows within 4 s"), entered, || {
        format!("{:?}\n{}", shown_panels(&term), shown(&term))
    });
    checks.check("focus: four panels still", shown_panels(&term).len() == 4, || format!("{:?}", shown_panels(&term)));
    // A card whose turn failed, the same.
    says(&team, card, "idle");
    std::thread::sleep(Duration::from_millis(800));
    let panels = shown_panels(&term);
    if let Some(other) = ["a1", "a2", "a3", "a4", "a5", "a6"].into_iter().find(|m| !panels.contains(&m.to_string())) {
        fails(&team, other, OVERLOADED);
        let entered = wait(Duration::from_secs(4), || has_header(&term, other));
        checks.check(&format!("focus: {other}, a card, failed: its panel shows within 4 s"), entered, || {
            format!("{:?}\n{}", shown_panels(&term), shown(&term))
        });
    } else {
        checks.check("focus: a card left to fail", false, || shown(&term));
    }
    checks.done();
    drop(term);
    team.stop();
}

/// What `member`'s program heard of the color scheme, from `fake/scheme.log` (`CSI ? 997 ; N n`: `1` dark, `2` light), once
/// the fake has read a line (`flush`).
fn heard_schemes(team: &Team) -> Vec<String> {
    std::fs::read_to_string(team.profile.join("fake/scheme.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// A line to the fake, which hands what it received before it to its log.
fn flush(team: &Team, member: &str) {
    team.ctl(["send", "--pane", member, "noop\\r"]);
}

/// The color scheme reaches the programs that ask for it (`CSI ? 2031 h`), when it changes and not otherwise: a client
/// of a light terminal attaches, and the program receives `CSI ? 997 ; 2 n`; a new attach with the same scheme tells it
/// nothing; a client of a dark terminal tells it `; 1 n`. (The terminal answers OSC 10 and 11, as the client asks at
/// each attach.)
#[test]
fn a_program_hears_the_color_scheme_when_it_changes() {
    let mut team = Team::new("scheme", &tables("scheme", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    team.ctl(["send", "--pane", "dev", "scheme\\r"]);
    std::thread::sleep(Duration::from_millis(500));
    let mut checks = Checks::default();
    let light = Profile { light: true, ..Profile::default() };
    let hears = |team: &Team, want: &[&str], within: Duration| {
        wait(within, || {
            flush(team, "dev");
            std::thread::sleep(Duration::from_millis(150));
            heard_schemes(team) == want
        })
    };
    // A light client attaches.
    let term = team.attach(120, 40, light);
    assert!(term.wait_for("lead", Duration::from_secs(5)), "{}", shown(&term));
    let got = hears(&team, &["2"], Duration::from_secs(6));
    checks.check("a light client: the program hears `997 ; 2 n`", got, || format!("{:?}", heard_schemes(&team)));
    drop(term);
    std::thread::sleep(Duration::from_millis(500));
    // The same scheme again: nothing more.
    let term = team.attach(120, 40, light);
    assert!(term.wait_for("lead", Duration::from_secs(5)), "{}", shown(&term));
    let quiet = !wait(Duration::from_secs(3), || {
        flush(&team, "dev");
        std::thread::sleep(Duration::from_millis(150));
        heard_schemes(&team) != ["2"]
    });
    checks.check("the same scheme again: nothing more is told", quiet, || format!("{:?}", heard_schemes(&team)));
    drop(term);
    std::thread::sleep(Duration::from_millis(500));
    // A dark client: `; 1 n`.
    let term = team.attach(120, 40, Profile::default());
    assert!(term.wait_for("lead", Duration::from_secs(5)), "{}", shown(&term));
    let got = hears(&team, &["2", "1"], Duration::from_secs(6));
    checks.check("a dark client after: the program hears `997 ; 1 n`", got, || format!("{:?}", heard_schemes(&team)));
    drop(term);
    checks.done();
    team.stop();
}

/// L2: `--dry-run` in native: the plan shown in the panes (the command each would run), no Claude started, no login
/// shell at the end, `recruit list` says a trial runs, and `recruit stop` closes it.
#[test]
fn dry_run_starts_no_claude() {
    let mut team = Team::new("dry", &tables("dry", &["lead"], &[("dev", None)], ""));
    let out = team.recruit(["--detach", "--dry-run"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // The trial is a team of its own beside the real one: `<team>-dry-run`.
    let state = team.root.join("cache/recruit/teams/dry-dry-run");
    let capture = |member: &str| {
        let out = team.command().arg("_ctl").arg(&state).args(["capture", "--pane", member]).output().expect("_ctl");
        String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim_end().to_string()).collect::<Vec<_>>().join("")
    };
    let mut checks = Checks::default();
    for member in ["lead", "dev"] {
        let shown = wait(Duration::from_secs(5), || capture(member).contains("Claude n'est pas lancé"));
        checks.check(&format!("L2 {member}'s pane says Claude is not started"), shown, || capture(member));
        let text = capture(member);
        checks.check(
            &format!("L2 {member}'s pane shows the command"),
            text.contains("--plugin-dir") && text.contains(&format!("-n {member}")),
            || text.clone(),
        );
    }
    let started = team.launches("lead").len() + team.launches("dev").len();
    checks.check("L2 no Claude was started", started == 0, || format!("{started} launches"));
    let claude = team.root.join("claude").to_string_lossy().to_string();
    let running = common::server::leftovers(&team.root)
        .into_iter()
        .filter(|l| l.split_whitespace().nth(1) == Some(claude.as_str()))
        .count();
    checks.check("L2 no process of Claude", running == 0, || format!("{running}"));
    let list = String::from_utf8_lossy(&team.recruit(["list"]).stdout).to_string();
    checks.check("L2 `recruit list` says a trial runs", list.contains("--dry-run"), || list.clone());
    checks.done();
    team.stop();
}

/// S3: coming back to a team that runs starts again the members that stopped (a shell where Claude was) and opens the
/// dashboard again if it is gone; then the client is attached.
#[test]
fn coming_back_repairs_the_team() {
    let mut team = Team::new("back3", &tables("back3", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let mut checks = Checks::default();
    // The dashboard is closed, dev stopped by Ctrl-C (a shell stays).
    team.ctl(["kill", "--pane", "dashboard"]);
    team.ctl(["send", "--pane", "dev", "Q\\r"]);
    let cancelled = wait(Duration::from_secs(6), || {
        team.ctl(["send", "--pane", "dev", "\\x03"]);
        std::thread::sleep(Duration::from_millis(150));
        team.capture("dev").iter().any(|r| r.contains("Reprise annulée"))
    });
    assert!(cancelled, "dev did not stop: {:?}", team.capture("dev"));
    assert!(!team.panes().iter().any(|p| p["role"] == "dashboard"), "the dashboard is still there");
    assert_eq!(team.launches("dev").len(), 1);
    // `recruit back3` again, on a terminal.
    let mut term = team.in_terminal(&["back3"], 120, 40, Profile::default());
    let started = wait(Duration::from_secs(10), || team.launches("dev").len() == 2);
    checks.check("S3 dev's Claude started again", started, || format!("{:?}\n{}", team.launches("dev"), shown(&term)));
    let board = wait(Duration::from_secs(10), || team.panes().iter().any(|p| p["role"] == "dashboard"));
    checks.check("S3 the dashboard is open again", board, || format!("{:?}\n{}", team.tabs(), shown(&term)));
    let attached = wait(Duration::from_secs(10), || has_header(&term, "lead"));
    checks.check("S3 the client shows the team", attached, || shown(&term));
    let _ = term.wait_exit(Duration::from_millis(10));
    drop(term);
    checks.done();
    team.stop();
}

/// S3: when panes are missing, coming back asks whether to rebuild the team: « n » keeps it as it is (the client still
/// attaches), « O » opens them again, each member on its conversation.
#[test]
fn coming_back_asks_to_rebuild() {
    let mut team = Team::new("miss", &tables("miss", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let session = {
        assert!(wait(Duration::from_secs(8), || team.state().join("sessions/dev").exists()));
        std::fs::read_to_string(team.state().join("sessions/dev")).expect("session").trim().to_string()
    };
    team.ctl(["kill", "--pane", "dev"]);
    assert!(wait(Duration::from_secs(3), || !team.panes().iter().any(|p| p["member"] == "dev")));
    let mut checks = Checks::default();
    let mut term = team.in_terminal(&["miss"], 120, 30, Profile::default());
    let asked = wait(Duration::from_secs(8), || {
        shown(&term).contains("Panneaux fermés : dev") && shown(&term).contains("Reconstruire")
    });
    checks.check("S3 the question names the closed pane", asked, || shown(&term));
    term.send(b"n\r");
    let attached = wait(Duration::from_secs(8), || has_header(&term, "lead"));
    checks.check("S3 « n »: the client attaches", attached, || shown(&term));
    checks.check("S3 « n »: nothing rebuilt", !team.panes().iter().any(|p| p["member"] == "dev"), || {
        format!("{:?}", team.tabs())
    });
    drop(term);
    let mut term = team.in_terminal(&["miss"], 120, 30, Profile::default());
    wait(Duration::from_secs(8), || shown(&term).contains("Reconstruire"));
    term.send(b"o\r");
    // Rebuilding starts the team again: for a moment, no server answers.
    let has_dev = || {
        let out = team.ctl_output(["panes", "--json"]);
        out.status.success() && String::from_utf8_lossy(&out.stdout).contains("\"member\":\"dev\"")
    };
    let rebuilt = wait(Duration::from_secs(15), || has_dev() && team.launches("dev").len() >= 2);
    checks.check("S3 « O »: dev is back", rebuilt, || {
        format!("{:?}\n{:?}\n{}", team.tabs(), team.launches("dev"), shown(&term))
    });
    let again = team.launches("dev").last().cloned().unwrap_or_default();
    checks.check("S3 « O »: on its conversation", again.contains(&format!("-r {session}")), || {
        format!("{session}: {again}")
    });
    drop(term);
    checks.done();
    team.stop();
}

/// L1: with team files that no longer read, the menu is read-only (members as launched, a message naming the file),
/// offers only « Détacher » and « Quitter », and « Quitter » still stops the team.
#[test]
fn unreadable_files_open_a_read_only_menu() {
    let mut team = Team::new("ro", &tables("ro", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let mut term = client(&team, "lead");
    std::fs::write(team.proj.join(".recruit/settings.toml"), "this is = not [valid toml\n").expect("break the file");
    let keys = alt(&term, 'r');
    term.send(&keys);
    let opened = wait(Duration::from_secs(8), || shown(&term).contains("lecture seule"));
    let mut checks = Checks::default();
    checks.check("L1 the menu says it is read-only", opened, || shown(&term));
    let text = shown(&term);
    checks.check("L1 the members as launched", text.contains("lead") && text.contains("dev"), || text.clone());
    checks.check("L1 it names the file", text.contains("settings.toml") && text.contains("TOML"), || text.clone());
    checks.check(
        "L1 Détacher and Quitter only",
        text.contains("d Détacher")
            && text.contains("q Quitter")
            && !text.contains("Nouvel agent")
            && !text.contains("Réinitialiser"),
        || text.clone(),
    );
    term.send(b"\x1b");
    let closed = wait(Duration::from_secs(5), || !shown(&term).contains("lecture seule"));
    checks.check("L1 Esc closes it", closed, || shown(&term));
    let keys = alt(&term, 'r');
    term.send(&keys);
    wait(Duration::from_secs(8), || shown(&term).contains("lecture seule"));
    term.send(b"q");
    let asked = wait(Duration::from_secs(5), || shown(&term).contains("Quitter l'équipe ?"));
    checks.check("L1 « q » asks to quit the team", asked, || shown(&term));
    let tmux_said = shown(&term).contains("tmux");
    checks.check("L1 the question does not speak of tmux", !tmux_said, || shown(&term));
    // It opens on Annuler: back to Quitter, then Enter.
    term.send(b"\x1b[D");
    std::thread::sleep(Duration::from_millis(300));
    term.send(b"\r");
    let stopped = wait(Duration::from_secs(8), || !team.running());
    checks.check("L1 Quitter stops the team", stopped, || shown(&term));
    drop(term);
    checks.done();
    team.stopped_by_the_user();
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
    // The clocks (the seconds in the header and on the dashboard's cards) draw frames of their own: each run of moves
    // is measured against a run of the same length with the pointer still, and may cost only a frame or two more.
    const RUN: Duration = Duration::from_millis(400);
    const SLACK: u64 = 2;
    let measure = |term: &mut TestTerm, moves: &mut dyn FnMut(&mut TestTerm)| {
        let before = frames(&team);
        let begun = std::time::Instant::now();
        moves(term);
        std::thread::sleep(RUN.saturating_sub(begun.elapsed()));
        frames(&team) - before
    };
    let name = find_in_header(&term, " lead ").map(|(c, r)| (c + 1, r)).expect("name");
    let model = find_in_header(&term, "Opus").expect("model");
    let inside = (5, 10);
    let mut checks = Checks::default();
    let still = measure(&mut term, &mut |_| {});
    // Over the text of the pane: nothing to light.
    let idle_moves = measure(&mut term, &mut |term| {
        for i in 0..100 {
            motion(term, (inside.0 + i % 20, inside.1 + i % 5));
        }
    });
    checks.check(
        "F3 100 moves over a pane's text: no more frames than standing still",
        idle_moves <= still + SLACK,
        || format!("{idle_moves} frames against {still}"),
    );
    // On one element: a frame to light it (waited for), none after.
    motion(&mut term, name);
    assert!(wait(Duration::from_secs(3), || shown(&term).contains("réglages")), "the name never lit: {}", shown(&term));
    let still = measure(&mut term, &mut |_| {});
    let same = measure(&mut term, &mut |term| {
        for _ in 0..100 {
            motion(term, name);
        }
    });
    checks.check("F3 100 moves on the same name: no more frames than standing still", same <= still + SLACK, || {
        format!("{same} frames against {still}")
    });
    // From one element to the other, twenty times: about a frame a change.
    let still = measure(&mut term, &mut |_| {});
    let changes = measure(&mut term, &mut |term| {
        for i in 0..20 {
            motion(term, if i % 2 == 0 { model } else { name });
            std::thread::sleep(Duration::from_millis(40));
        }
    });
    checks.check(
        "F3 20 changes of element: 6 to 25 frames, the clocks apart (under load, moves merge into one frame)",
        (6..=25 + still + SLACK).contains(&changes),
        || format!("{changes} frames against {still} standing still"),
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

/// What the mod hands over at each tick (`mod/hooks/register.ts`, `tick`: `session`, `model`, `own`, `context`,
/// `rateLimits`, `threshold`, `compacted`, `agents`) is kept in the member's report (`<state>/members/<member>.json`),
/// the `compacted: false` it always carries included. Found by a real session on 2026-10-11: with `compacted` a flag
/// in the input and a compaction in the report, the whole input failed to read, and every report came back empty (no
/// model, no context, nothing for the compaction to measure).
#[test]
fn a_tick_from_the_mod_is_kept() {
    use std::io::Write;
    let mut team = Team::new("tick", &tables("tick", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let state = team.state();
    let input = serde_json::json!({
        "session": "11111111-2222-3333-4444-555555555555",
        "model": "claude-haiku-5-5",
        "own": {"model": "claude-haiku-5-5", "effort": "high"},
        "context": {"tokens": 24000, "window": 200000, "percent": 12.0},
        "rateLimits": [{"kind": "five_hour", "percentUsed": 21.0}],
        "threshold": {"tokens": 167000, "enabled": true, "window": 200000, "model": "claude-haiku-5-5", "used": 23000},
        "compacted": false,
        "agents": [],
    });
    let mut child = team
        .command()
        .args(["_mod", "tick"])
        .arg(&state)
        .arg("lead")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("_mod tick");
    child.stdin.take().expect("stdin").write_all(input.to_string().as_bytes()).expect("input");
    let out = child.wait_with_output().expect("tick");
    assert!(out.status.success(), "_mod tick: {}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(state.join("members/lead.json")).expect("the report the tick wrote"),
    )
    .expect("report: JSON");
    let mut checks = Checks::default();
    checks.check("a tick keeps the session", report["session"] == "11111111-2222-3333-4444-555555555555", || {
        report.to_string()
    });
    checks.check("a tick keeps the model", report["model"] == "claude-haiku-5-5", || report.to_string());
    checks.check("a tick keeps the context", report["context"]["percent"] == 12.0, || report.to_string());
    checks.check("a tick keeps the threshold", report["threshold"]["tokens"] == 167000, || report.to_string());
    checks.check("a tick keeps the limits", report["rateLimits"].as_array().is_some_and(|l| l.len() == 1), || {
        report.to_string()
    });
    checks.done();
    team.stop();
}

/// R2: `/recruit` (the mod calls `recruit _mod command <state> <member> recruit`): with a client, opens the same
/// menu and says so; with the menu already open, says so; with no client, says how to get one. In words that name no
/// tmux.
#[test]
fn slash_recruit_opens_the_menu() {
    let mut team = Team::new("slash", &tables("slash", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let state = team.state();
    let slash = |team: &Team| {
        let out = team.recruit([
            std::ffi::OsStr::new("_mod"),
            std::ffi::OsStr::new("command"),
            state.as_os_str(),
            std::ffi::OsStr::new("lead"),
            std::ffi::OsStr::new("recruit"),
        ]);
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    };
    let mut checks = Checks::default();
    // No client yet.
    let none = slash(&team);
    checks.check(
        "R2 no client: says how to get one",
        none.contains("recruit attach") && !none.contains("tmux"),
        || none.clone(),
    );
    let mut term = client(&team, "lead");
    let first = slash(&team);
    let open = wait(Duration::from_secs(8), || shown(&term).contains("Nouvel agent"));
    checks
        .check("R2 /recruit opens the menu", open && first.contains("ouvert"), || format!("{first}\n{}", shown(&term)));
    checks.check("R2 it names no tmux", !first.contains("tmux"), || first.clone());
    let again = slash(&team);
    checks.check("R2 already open: says so", again.contains("déjà ouvert") && !again.contains("tmux"), || {
        again.clone()
    });
    term.send(b"\x1b");
    let closed = wait(Duration::from_secs(5), || menu_text(&team).is_none());
    checks.check("R2 the menu closes", closed, || shown(&term));
    drop(term);
    checks.done();
    team.stop();
}

/// K2: a click on the sender or the recipient of a message in the journal goes to that member's pane.
#[test]
fn journal_clicks_go_to_the_member() {
    let mut team = Team::new("jclick", &tables("jclick", &["lead", "pm"], &[("dev", None)], ""));
    team.launch(&["lead", "pm", "dev"]);
    let mut term = client(&team, "lead");
    team.ctl(["send", "--pane", "lead", "msg pm Peux-tu cadrer la demande ?\\r"]);
    team.ctl(["send", "--pane", "pm", "msg dev Voici le besoin.\\r"]);
    // The journal writes a line a message: « 01:33  lead ──▶ pm ».
    let row_of = |term: &TestTerm, from: &str, to: &str| {
        term.screen().iter().enumerate().find_map(|(row, line)| {
            let arrow = line.find("──▶")?;
            let before = line[..arrow].split_whitespace().last()? == from;
            let after = line[arrow + "──▶".len()..].split_whitespace().next()? == to;
            (before && after).then_some((row, arrow))
        })
    };
    let mut checks = Checks::default();
    let there =
        wait(Duration::from_secs(8), || row_of(&term, "lead", "pm").is_some() && row_of(&term, "pm", "dev").is_some());
    checks.check("K2 the journal shows both messages", there, || shown(&term));
    let word_at = |term: &TestTerm, row: usize, arrow: usize, to: bool, word: &str| {
        let line = &term.screen()[row];
        let at = if to {
            arrow + "──▶".len() + line[arrow + "──▶".len()..].find(word)?
        } else {
            line[..arrow].rfind(word)?
        };
        Some((unicode_width::UnicodeWidthStr::width(&line[..at]), row))
    };
    let members = ["lead", "pm", "dev"];
    if let Some((row, arrow)) = row_of(&term, "lead", "pm") {
        if let Some(at) = word_at(&term, row, arrow, true, "pm") {
            click(&mut term, at);
            let went = wait(Duration::from_secs(3), || active(&term, &members) == ["pm"]);
            checks.check("K2 a click on the recipient goes to pm", went, || shown(&term));
        }
        if let Some((row, arrow)) = row_of(&term, "lead", "pm")
            && let Some(at) = word_at(&term, row, arrow, false, "lead")
        {
            click(&mut term, at);
            let went = wait(Duration::from_secs(3), || active(&term, &members) == ["lead"]);
            checks.check("K2 a click on the sender goes to lead", went, || shown(&term));
        }
    }
    if let Some((row, arrow)) = row_of(&term, "pm", "dev")
        && let Some(at) = word_at(&term, row, arrow, true, "dev")
    {
        click(&mut term, at);
        let went = wait(Duration::from_secs(3), || active(&term, &members) == ["dev"]);
        checks.check("K2 a click on a recipient in another tab goes there", went, || shown(&term));
    }
    checks.done();
    team.stop();
}

// 2.0.1: sobriety of the dashboard and the OSC state that is lost. Guards from end to end, measured before the fixes
// (`--ignored`) and normal tests once the fixes are in.

/// `claude agents --json` is what costs most of a team's CPU (0.16 to 0.29 s and ~160 MB a call): with every member
/// giving its state by OSC 7501, a team of six asks it 0.3 times a second at most (2.0.0: ~1.9). Counted by the
/// fake `claude`'s shim over 12 s, once the team has settled; the margin is two calls and no more load makes it
/// ask more.
#[test]
fn claude_agents_is_asked_seldom() {
    const WINDOW: u64 = 12;
    const RATE: f64 = 0.3;
    let agents: Vec<(String, Option<&str>)> = (1..=5).map(|i| (format!("dev{i}"), None)).collect();
    let agents: Vec<(&str, Option<&str>)> = agents.iter().map(|(n, t)| (n.as_str(), *t)).collect();
    let mut team = Team::new("seldom", &tables("seldom", &["lead"], &agents, ""));
    let members = ["lead", "dev1", "dev2", "dev3", "dev4", "dev5"];
    team.launch(&members);
    for member in members {
        says(&team, member, "idle");
    }
    // The start-up settles: the first reads of every member, the dashboard's first states.
    std::thread::sleep(Duration::from_secs(3));
    let begun = team.agent_calls().len();
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(WINDOW));
    let calls = team.agent_calls();
    let calls = &calls[begun.min(calls.len())..];
    let secs = started.elapsed().as_secs_f64();
    let limit = (RATE * secs).ceil() as usize + 2;
    let mut by_caller = std::collections::BTreeMap::<&str, usize>::new();
    for (_, _, caller) in calls {
        *by_caller.entry(caller.as_str()).or_default() += 1;
    }
    println!(
        "claude agents: {} calls in {secs:.1} s = {:.2}/s (limit {limit}); by caller: {by_caller:?}",
        calls.len(),
        calls.len() as f64 / secs
    );
    assert!(calls.len() <= limit, "{} calls in {secs:.1} s (at most {limit}); by caller: {by_caller:?}", calls.len());
    team.stop();
}

/// What a pane has read of its program since it opened (`_ctl stats`), for the pane called `member`.
fn pane_read(team: &Team, member: &str) -> u64 {
    let stats: serde_json::Value = serde_json::from_str(&team.ctl(["stats", "--json"])).expect("stats");
    stats["panes"]
        .as_array()
        .and_then(|panes| panes.iter().find(|p| p["member"] == member))
        .and_then(|p| p["read"].as_u64())
        .unwrap_or_else(|| panic!("no pane {member} in {stats}"))
}

/// What `claude agents` says of a member: `busy`, `idle`, `waiting`.
fn agent_status(team: &Team, member: &str, status: &str) {
    team.ctl(["send", "--pane", member, &format!("agent {status}\\r")]);
}

/// The dashboard writes little while three members work: the spinners turn in the cards, and only what changed
/// is drawn again (2.0.0: the whole title line of each working card ten times a second, ~7 KB/s for twelve
/// members). At most 1 KB/s, read from the pane's own count of bytes its program wrote, over 5 s.
#[test]
fn the_dashboard_writes_little() {
    const WINDOW: u64 = 5;
    const PER_SECOND: u64 = 1024;
    let mut team = Team::new("quiet", &tables("quiet", &["lead"], &[("dev1", None), ("dev2", None)], ""));
    let members = ["lead", "dev1", "dev2"];
    team.launch(&members);
    // Told both ways, as Claude Code does: the OSC state and what `claude agents` says.
    for member in members {
        says(&team, member, "working");
        agent_status(&team, member, "busy");
    }
    // The cards take the new states (the server and the dashboard read them every few seconds).
    std::thread::sleep(Duration::from_secs(3));
    let dashboard = "Tableau de bord";
    let (dash, journal) = (pane_read(&team, dashboard), pane_read(&team, "Journal"));
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(WINDOW));
    let secs = started.elapsed().as_secs_f64();
    let (dash, journal) = (pane_read(&team, dashboard) - dash, pane_read(&team, "Journal") - journal);
    println!(
        "dashboard: {dash} bytes in {secs:.1} s = {:.0} B/s (limit {PER_SECOND}); journal: {journal} bytes",
        dash as f64 / secs
    );
    assert!(
        dash as f64 <= PER_SECOND as f64 * secs,
        "the dashboard wrote {dash} bytes in {secs:.1} s ({:.0} B/s, at most {PER_SECOND})",
        dash as f64 / secs
    );
    team.stop();
}

/// The sign before `name` on its header row on `term` (the frame's top border, not a card of the dashboard).
fn header_sign(term: &TestTerm, name: &str) -> Option<char> {
    let wanted = format!(" {name} ");
    term.screen().iter().find_map(|row| {
        if !row.chars().any(|c| matches!(c, '┏' | '╭')) || !row.contains(['━', '─']) {
            return None;
        }
        row[..row.find(&wanted)?].chars().last()
    })
}

/// Whether `sign` is an image of the spinner (Braille), the sign of a member at work.
fn is_spinner(sign: Option<char>) -> bool {
    sign.is_some_and(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
}

/// Waits until the header of `name` on `term` shows (or no longer shows) a member at work.
fn header_works(term: &TestTerm, name: &str, works: bool, timeout: Duration) -> bool {
    wait(timeout, || is_spinner(header_sign(term, name)) == works)
}

/// Claude Code 2.1.296 writes `state=working:app=claude-code:id=<id>:title=<b64>` for a sub-agent and
/// `state=clear:id=<id>` when it ends (the `clear` of an id is that sub-agent's, not the member's), and a bare
/// `state=clear` when `/clear` starts a new conversation: the header must keep showing the member's own state,
/// not « ◌ ». The fake writes no new `OSC 7501 ; ?` after the clear, as Claude Code does not.
#[test]
fn the_state_survives_a_clear() {
    let mut team = Team::new("cleared", &tables("cleared", &["lead", "co"], &[], ""));
    team.launch(&["lead", "co"]);
    for member in ["lead", "co"] {
        says(&team, member, "idle");
    }
    let term = client(&team, "lead");
    let mut checks = Checks::default();
    let calm = header_works(&term, "lead", false, Duration::from_secs(5))
        && header_works(&term, "co", false, Duration::from_secs(5));
    checks.check("both idle before", calm, || shown(&term));
    let fails = |term: &TestTerm, member: &str| format!("sign {:?}\n{}", header_sign(term, member), shown(term));
    // A sub-agent starts and ends, with its id: the member stays at work, at every moment.
    says(&team, "lead", "working");
    says(&team, "lead", "working:app=claude-code:id=a1b2");
    let works = header_works(&term, "lead", true, Duration::from_secs(5));
    checks.check("lead: at work", works, || fails(&term, "lead"));
    says(&team, "lead", "clear:id=a1b2");
    let mut steady = true;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(100));
        steady &= is_spinner(header_sign(&term, "lead"));
    }
    checks.check("lead: still at work after the sub-agent's clear (with its id)", steady, || fails(&term, "lead"));
    // The states still count after that: done, then at work again.
    says(&team, "lead", "done");
    let rests = header_works(&term, "lead", false, Duration::from_secs(5));
    checks.check("lead: at rest after `done`", rests, || fails(&term, "lead"));
    says(&team, "lead", "working");
    let again = header_works(&term, "lead", true, Duration::from_secs(5));
    checks.check("lead: at work again", again, || fails(&term, "lead"));
    // A status with an id alone changes nothing: from `done`, `working:id=` stays at rest.
    says(&team, "lead", "done");
    header_works(&term, "lead", false, Duration::from_secs(5));
    says(&team, "lead", "working:id=a1b2");
    std::thread::sleep(Duration::from_millis(800));
    checks.check("lead: a status with an id alone changes nothing", !is_spinner(header_sign(&term, "lead")), || {
        fails(&term, "lead")
    });
    // The member's own `/clear` (no id): its states come after, and count.
    says(&team, "co", "clear");
    std::thread::sleep(Duration::from_millis(500));
    says(&team, "co", "working");
    let works = header_works(&term, "co", true, Duration::from_secs(5));
    checks.check("co: at work after a bare clear", works, || fails(&term, "co"));
    // And it follows the states after that: waiting, then idle again.
    says(&team, "co", "blocked:kind=permission");
    let waits = wait(Duration::from_secs(5), || header_sign(&term, "co") == Some('\u{2691}'));
    checks.check("co: waiting after the clear", waits, || fails(&term, "co"));
    says(&team, "co", "done");
    let idle = header_works(&term, "co", false, Duration::from_secs(5));
    checks.check("co: at rest again", idle, || fails(&term, "co"));
    drop(term);
    checks.done();
    team.stop();
}

/// A pane that gives no state by OSC 7501 (a Claude Code that never speaks it, a shell) takes its state from the
/// dashboard (`claude agents`): at work when it says `busy`, idle when it does not.
#[test]
fn a_pane_without_a_state_takes_the_dashboards() {
    let mut team = Team::new("fallback", &tables("fallback", &["lead", "co"], &[], ""));
    team.launch(&["lead", "co"]);
    let term = client(&team, "lead");
    let mut checks = Checks::default();
    // Neither speaks OSC 7501 (the fakes never ask for it): `claude agents` is all there is.
    for member in ["lead", "co"] {
        agent_status(&team, member, "busy");
    }
    for member in ["lead", "co"] {
        let works = header_works(&term, member, true, Duration::from_secs(12));
        checks.check(&format!("{member}: at work by the dashboard's state"), works, || {
            format!("sign {:?}\n{}", header_sign(&term, member), shown(&term))
        });
    }
    for member in ["lead", "co"] {
        agent_status(&team, member, "idle");
    }
    for member in ["lead", "co"] {
        let idle = header_works(&term, member, false, Duration::from_secs(12));
        checks.check(&format!("{member}: idle by the dashboard's state"), idle, || {
            format!("sign {:?}\n{}", header_sign(&term, member), shown(&term))
        });
    }
    drop(term);
    checks.done();
    team.stop();
}

/// A bare `clear` (OSC 7501 `state=clear`, no id) means the program gives no state any more: the pane takes the
/// dashboard's (`claude agents`), and shows « ◌ » only when the dashboard knows nothing of the member either.
#[test]
fn a_bare_clear_hands_over_to_the_dashboard() {
    let mut team = Team::new("handover", &tables("handover", &["lead", "co"], &[], ""));
    team.launch(&["lead", "co"]);
    let term = client(&team, "lead");
    let mut checks = Checks::default();
    let fails = |member: &str| format!("sign {:?}\n{}", header_sign(&term, member), shown(&term));
    for member in ["lead", "co"] {
        says(&team, member, "working");
        let works = header_works(&term, member, true, Duration::from_secs(5));
        checks.check(&format!("{member}: at work by its own state"), works, || fails(member));
    }
    // `lead`: `claude agents` says idle: the clear hands the pane over to it.
    agent_status(&team, "lead", "idle");
    says(&team, "lead", "clear");
    let rests = wait(Duration::from_secs(12), || header_sign(&term, "lead") == Some('\u{25f7}'));
    checks.check("lead: a bare clear, the dashboard says idle: at rest", rests, || fails("lead"));
    // `co`: `claude agents` no longer lists it (its file is gone): nothing says anything.
    let runs = std::fs::read_dir(team.profile.join("fake/run")).expect("run folder");
    let run = runs
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains("\"name\":\"co\"")))
        .expect("co's run file");
    std::fs::remove_file(run).expect("remove co's run file");
    says(&team, "co", "clear");
    let unknown = wait(Duration::from_secs(12), || header_sign(&term, "co") == Some('\u{25cc}'));
    checks.check("co: a bare clear, the dashboard knows nothing: \u{25cc}", unknown, || fails("co"));
    drop(term);
    checks.done();
    team.stop();
}

/// The session files of the fake `claude` (`<profile>/sessions/<pid>.json`, as Claude Code writes them, for
/// `member.rs`): the keys it reads, the status following `claude agents`, and none when the team goes without.
#[test]
fn the_fake_writes_its_session_file() {
    let sessions = |team: &Team, name: &str| -> Option<serde_json::Value> {
        let files = std::fs::read_dir(team.profile.join("sessions")).ok()?;
        files
            .filter_map(|e| {
                serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(e.ok()?.path()).ok()?).ok()
            })
            .find(|v| v["name"] == name)
    };
    let mut team = Team::new("sessfile", &tables("sessfile", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let file = sessions(&team, "dev").expect("dev's session file");
    let (pid, session) = (file["pid"].as_u64().unwrap_or_default(), file["sessionId"].as_str().unwrap_or_default());
    assert!(pid > 1 && session.starts_with("fake-"), "{file}");
    assert!(file["cwd"].is_string() && file["status"] == "idle" && file["statusUpdatedAt"].is_u64(), "{file}");
    assert!(team.profile.join(format!("sessions/{pid}.json")).exists());
    agent_status(&team, "dev", "busy");
    assert!(wait(Duration::from_secs(3), || sessions(&team, "dev").is_some_and(|f| f["status"] == "busy")));
    team.stop();
    let mut team = Team::new("nosess", &tables("nosess", &["lead"], &[], ""));
    team.without_session_files();
    team.launch(&["lead"]);
    assert!(sessions(&team, "lead").is_none(), "a session file despite the marker");
    team.stop();
}

/// `member.rs` notes its Claude's session (`<state>/sessions/<member>`) from Claude Code's own file in the profile
/// (`sessions/<pid>.json`), asking `claude agents` for nothing; without that file it falls back to `claude agents`.
#[test]
fn a_member_notes_its_session() {
    let from_members = |team: &Team| team.agent_calls().iter().filter(|c| c.2.starts_with("_member")).count();
    // With the file: noted, and no call of `claude agents` from any `_member`.
    let mut team = Team::new("noted", &tables("noted", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    assert!(wait(Duration::from_secs(8), || team.state().join("sessions/dev").exists()), "dev's session never noted");
    let session = std::fs::read_to_string(team.state().join("sessions/dev")).unwrap_or_default();
    assert!(session.trim().starts_with("fake-"), "{session:?}");
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(from_members(&team), 0, "{:?}", team.agent_calls());
    team.stop();
    // Without it: noted all the same, through `claude agents`.
    let mut team = Team::new("fallbk", &tables("fallbk", &["lead"], &[("dev", None)], ""));
    team.without_session_files();
    team.launch(&["lead", "dev"]);
    assert!(wait(Duration::from_secs(10), || team.state().join("sessions/dev").exists()), "dev's session never noted");
    let session = std::fs::read_to_string(team.state().join("sessions/dev")).unwrap_or_default();
    assert!(session.trim().starts_with("fake-"), "{session:?}");
    team.stop();
}

/// A dashboard of this build in front of a server of the 2.0.0 (`OLD_RECRUIT=<a 2.0.0 binary>`: its `Panes` has no
/// `status`): it must not break, and fall back to asking `claude agents` once a second, no more. The server has no
/// dashboard of its own (`dashboard = false`), so the only `_panel dashboard` is this one.
#[test]
#[ignore = "needs OLD_RECRUIT=<a 2.0.0 binary>: run with --ignored"]
fn the_dashboard_in_front_of_an_old_server() {
    let old = std::env::var_os("OLD_RECRUIT").expect("OLD_RECRUIT: the path of a 2.0.0 binary");
    let mut team = Team::new("oldsrv", &tables("oldsrv", &["lead"], &[("dev", None)], "dashboard = false"));
    team.use_binary(std::path::Path::new(&old));
    team.launch(&["lead", "dev"]);
    // Members that say their state by OSC: with a new server nothing would be asked every second.
    says(&team, "lead", "working");
    says(&team, "dev", "idle");
    let new = team.root.join("recruit-new");
    std::fs::copy(env!("CARGO_BIN_EXE_recruit"), &new).expect("copy of this build");
    let state = team.state().to_string_lossy().into_owned();
    let term = team.in_terminal_of(&new, &["_panel", "dashboard", &state], 60, 20, Profile::default());
    let started = Instant::now();
    assert!(term.wait_for("lead", Duration::from_secs(8)), "the dashboard shows no member: {:?}", term.screen());
    std::thread::sleep(Duration::from_secs(8));
    let secs = started.elapsed().as_secs_f64();
    let mine: Vec<_> = team.agent_calls().into_iter().filter(|c| c.2 == "_panel dashboard").collect();
    println!(
        "old server: {} calls of `_panel dashboard` in {secs:.1} s = {:.2}/s",
        mine.len(),
        mine.len() as f64 / secs
    );
    assert!(mine.len() as f64 <= secs * 1.3 + 2.0, "more than once a second: {} in {secs:.1} s", mine.len());
    assert!(mine.len() >= 3, "far less than once a second: {} in {secs:.1} s", mine.len());
    assert!(term.screen().iter().any(|r| r.contains("lead")), "{:?}", term.screen());
    drop(term);
    team.stop();
}

/// A launch that resumes (`--resume`): a member whose Claude stops at once, before it writes its own session file
/// (a failed start), is started again on the same conversation, not afresh.
#[test]
fn a_resumed_member_that_stops_at_once_comes_back_on_its_conversation() {
    let mut team = Team::new("resumes", &tables("resumes", &["lead"], &[("dev", None)], ""));
    let said = serde_json::json!({"type": "user", "message": {"content": "earlier"}}).to_string();
    team.seed_conversation("lead", "old-lead", std::slice::from_ref(&said));
    team.seed_conversation("dev", "old-dev", std::slice::from_ref(&said));
    team.die_once("dev");
    team.launch_with(&["--detach", "--resume"], &["lead"]);
    assert!(
        wait(Duration::from_secs(12), || team.launches("dev").len() >= 2),
        "dev never came back: {:?} {:?}",
        team.launches("dev"),
        team.capture("dev")
    );
    let starts = team.launches("dev");
    assert!(starts[0].ends_with("-n dev -r old-dev") || starts[0].contains("-r old-dev"), "first start: {}", starts[0]);
    assert!(starts[1].contains("-n dev") && starts[1].contains("-r old-dev"), "not on its conversation: {}", starts[1]);
    assert!(team.wait_pane("dev", "FAKE CLAUDE dev old-dev", Duration::from_secs(5)), "{:?}", team.capture("dev"));
    assert!(team.launches("lead")[0].contains("-r old-lead"), "{:?}", team.launches("lead"));
    team.stop();
}

/// A `states.json` left by the launch before (still fresh, as after `recruit --restart` of a team that was
/// running), with sessions, processes and addresses of members that are gone, and a transcript with a message:
/// none of it survives the launch, in the members' starts, the dashboard's file or the journal.
#[test]
fn a_states_file_from_before_is_not_kept() {
    let mut team = Team::new("yesterday", &tables("yesterday", &["lead"], &[("dev", None)], ""));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let stale = serde_json::json!({"type": "assistant", "timestamp": "2026-10-09T10:00:00.000Z",
        "message": {"content": [{"type": "tool_use", "name": "SendMessage",
            "input": {"to": "lead", "summary": "stale-message-xyz", "message": "stale-message-xyz"}}]}})
    .to_string();
    team.seed_conversation("ghost", "ghost-session", &[stale]);
    let before = serde_json::json!({
        "at": now, "members": {}, "absent": [],
        "refs": {"dev": "dev [ghost-ref]"},
        "sessions": {"dev": {"id": "ghost-session", "pid": 99999}, "lead": {"id": "ghost-session", "pid": 99998}},
        "pids": {"99999": "dev", "99998": "lead"},
    });
    std::fs::create_dir_all(team.state()).expect("state folder");
    std::fs::write(team.state().join("states.json"), before.to_string()).expect("old states.json");
    team.launch(&["lead", "dev"]);
    for member in ["lead", "dev"] {
        let starts = team.launches(member);
        assert!(!starts[0].contains("ghost-session"), "{member} started on the old session: {}", starts[0]);
    }
    // The dashboard writes its own file; wait for one that is not the old one's.
    let file = team.state().join("states.json");
    let rewritten = wait(Duration::from_secs(10), || {
        std::fs::read_to_string(&file).is_ok_and(|t| {
            serde_json::from_str::<serde_json::Value>(&t).is_ok_and(|v| v["at"].as_u64().unwrap_or(0) > now + 1)
        })
    });
    assert!(rewritten, "the dashboard never wrote its states: {:?}", std::fs::read_to_string(&file));
    std::thread::sleep(Duration::from_secs(6));
    let text = std::fs::read_to_string(&file).expect("states.json");
    for old in ["ghost-session", "ghost-ref", "99999", "99998"] {
        assert!(!text.contains(old), "states.json still carries {old}: {text}");
    }
    let journal = team.capture("Journal").join("\n");
    assert!(!journal.contains("stale-message-xyz"), "{journal}");
    team.stop();
}

/// A reading, not a guard: the composed text of the dashboard's pane for a team of fakes in a fixed state, written to
/// `$DUMP_OUT` (run it with two binaries, `RECRUIT_BIN`, and compare, the times apart).
#[test]
#[ignore = "a reading: DUMP_OUT=<file> cargo test --test mux_team dashboard_capture_dump -- --ignored"]
fn dashboard_capture_dump() {
    let out = std::env::var_os("DUMP_OUT").expect("DUMP_OUT");
    let mut team = Team::new("dump", &tables("dump", &["lead"], &[("dev1", None), ("dev2", None), ("dev3", None)], ""));
    team.launch(&["lead", "dev1", "dev2", "dev3"]);
    for (member, osc, agent) in [
        ("lead", "working", "busy"),
        ("dev1", "idle", "idle"),
        ("dev2", "blocked:kind=permission", "waiting"),
        ("dev3", "idle", "idle"),
    ] {
        says(&team, member, osc);
        agent_status(&team, member, agent);
    }
    let _term = client(&team, "lead");
    std::thread::sleep(Duration::from_secs(9));
    let text = team.capture("Tableau de bord").join("\n");
    std::fs::write(out, format!("{text}\n")).expect("dump");
    team.stop();
}

/// The passage the release notes promise: a team running under the 2.0.0 (`OLD_RECRUIT=<a 2.0.0 binary>`), its members
/// having noted their sessions, then `recruit --restart --resume` with this build. The old server ends, a new one
/// runs, and each member is on its conversation again (`-r <id>`), the 2.0.0's `states.json` still there.
#[test]
#[ignore = "needs OLD_RECRUIT=<a 2.0.0 binary>: run with --ignored"]
fn a_team_of_the_2_0_0_passes_to_this_build() {
    let old = std::env::var_os("OLD_RECRUIT").expect("OLD_RECRUIT: the path of a 2.0.0 binary");
    let mut team = Team::new("passage", &tables("passage", &["lead"], &[("dev", None)], ""));
    team.use_binary(std::path::Path::new(&old));
    team.launch(&["lead", "dev"]);
    let sessions: Vec<(&str, String)> = ["lead", "dev"].iter().map(|m| (*m, noted(&team, m))).collect();
    let before = team.ctl(["where"]);
    let states = std::fs::read_to_string(team.state().join("states.json")).unwrap_or_default();
    println!("2.0.0 server: {}\nstates.json from the 2.0.0: {} bytes", before.trim(), states.len());
    // This build now.
    team.use_binary(std::path::Path::new(env!("CARGO_BIN_EXE_recruit")));
    let out = team.recruit(["--restart", "--resume", "--detach"]);
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let after = team.ctl(["where"]);
    println!("new server: {}", after.trim());
    assert_ne!(before.trim(), after.trim(), "the server is the old one");
    for (member, id) in &sessions {
        assert!(
            wait(Duration::from_secs(12), || team.launches(member).len() >= 2),
            "{member} not started again: {:?}",
            team.launches(member)
        );
        let again = team.launches(member).last().cloned().unwrap_or_default();
        assert!(again.contains(&format!("-r {id}")), "{member} not on {id}: {again}");
        println!("{member}: {again}");
    }
    team.stop();
}

// A failed turn (« tour raté »): Claude Code 2.1.296 writes `state=error:app=claude-code:msg=<base64>` when a turn ends
// on an API error. The state lasts until the member's next one.

/// Standard base64, for the `msg` of an error state.
fn base64(text: &str) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(DIGITS[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The member's turn failed on `message`, as Claude Code says it.
fn fails(team: &Team, member: &str, message: &str) {
    says(team, member, &format!("error:app=claude-code:msg={}", base64(message)));
}

const OVERLOADED: &str =
    "API Error: 529 {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}";

/// The bar (the screen's last row).
fn bar_of(term: &TestTerm) -> String {
    term.screen().last().cloned().unwrap_or_default()
}

/// The `errors` of the dashboard's `states.json` (member → the error's whole text).
fn state_errors(team: &Team) -> serde_json::Value {
    std::fs::read_to_string(team.state().join("states.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map_or(serde_json::Value::Null, |v| v["errors"].clone())
}

#[test]
fn base64_of_the_fakes_is_standard() {
    assert_eq!(base64("API Error: 529"), "QVBJIEVycm9yOiA1Mjk=");
    assert_eq!(base64("ab"), "YWI=");
    assert_eq!(base64("abc"), "YWJj");
    assert_eq!(base64("a"), "YQ==");
}

/// The header of a member whose turn failed: ✗ in place of its state's sign, its name, then the reason in short (« API
/// surchargée (529) », else « erreur d'API »); the whole text goes to `states.json` (`errors`, for the menu); the
/// member's next state, `working`, clears both.
#[test]
fn a_failed_turn_shows_in_the_header_and_in_states_json() {
    let mut team = Team::new("failed", &tables("failed", &["lead", "co"], &[], ""));
    team.launch(&["lead", "co"]);
    let term = client(&team, "lead");
    let mut checks = Checks::default();
    let row_of = |member: &str| -> String {
        let wanted = format!(" {member} ");
        term.screen()
            .iter()
            .find(|r| r.contains(&wanted) && r.chars().any(|c| matches!(c, '┏' | '╭')) && r.contains(['━', '─']))
            .cloned()
            .unwrap_or_default()
    };
    // Each member works first, as Claude Code always does before an `error` (an `error` as a pane's very first status
    // was seen leaving the header on « ◌ », left as it is).
    says(&team, "lead", "working");
    assert!(header_works(&term, "lead", true, Duration::from_secs(5)), "{}", shown(&term));
    fails(&team, "lead", OVERLOADED);
    says(&team, "co", "working");
    std::thread::sleep(Duration::from_millis(600));
    fails(&team, "co", "API Error: the request failed in a way nobody foresaw");
    let shows = wait(Duration::from_secs(5), || header_sign(&term, "lead") == Some('\u{2717}'));
    checks.check("lead: ✗ in the header after the failed turn", shows, || row_of("lead"));
    wait(Duration::from_secs(5), || header_sign(&term, "co") == Some('\u{2717}'));
    std::thread::sleep(Duration::from_millis(300));
    let (lead, co) = (row_of("lead"), row_of("co"));
    let after = |row: &str, member: &str, label: &str| {
        row.find(&format!(" {member} ")).zip(row.find(label)).is_some_and(|(name, label)| label > name)
    };
    checks.check(
        "lead: « API surchargée (529) » after the name",
        after(&lead, "lead", "API surchargée (529)"),
        || lead.clone(),
    );
    checks.check("co: « erreur d'API » after the name", after(&co, "co", "erreur d'API"), || co.clone());
    // The whole text for the menu.
    let errors = wait(Duration::from_secs(8), || {
        let errors = state_errors(&team);
        errors["lead"].as_str().is_some_and(|t| t.contains("529")) && errors["co"].is_string()
    });
    checks.check("states.json: errors holds lead's text and co's", errors, || state_errors(&team).to_string());
    // The next state clears it.
    says(&team, "lead", "working");
    let cleared = wait(Duration::from_secs(5), || {
        is_spinner(header_sign(&term, "lead")) && !row_of("lead").contains("API surchargée")
    });
    checks.check("lead: working clears the error from the header", cleared, || row_of("lead"));
    let gone =
        wait(Duration::from_secs(6), || state_errors(&team)["lead"].is_null() && state_errors(&team)["co"].is_string());
    checks.check("states.json: lead's error is gone, co's stays", gone, || state_errors(&team).to_string());
    drop(term);
    checks.done();
    team.stop();
}

/// The bar: ✗ on the tab of a member whose turn failed. A tab keeps one sign, its most urgent member's (waiting, then
/// failed, then working, then at rest): ⚑ alone while another waits on the same tab, ✗ again once it does not.
#[test]
fn a_failed_turn_shows_on_its_tab() {
    let mut team = Team::new("tabfail", &tables("tabfail", &["lead"], &[("dev1", None), ("dev2", None)], ""));
    team.launch(&["lead", "dev1", "dev2"]);
    let term = client(&team, "lead");
    let mut checks = Checks::default();
    fails(&team, "dev2", OVERLOADED);
    let alone = wait(Duration::from_secs(5), || bar_of(&term).contains('\u{2717}'));
    checks.check("✗ on the tab of the failed member", alone, || bar_of(&term));
    says(&team, "dev1", "blocked:kind=permission");
    let waiting = wait(Duration::from_secs(5), || {
        let bar = bar_of(&term);
        bar.contains('\u{2691}') && !bar.contains('\u{2717}')
    });
    checks.check("⚑ alone on the tab with one waiting and one failed", waiting, || bar_of(&term));
    says(&team, "dev1", "working");
    let back = wait(Duration::from_secs(5), || {
        let bar = bar_of(&term);
        bar.contains('\u{2717}') && !bar.contains('\u{2691}')
    });
    checks.check("✗ again once the one waiting works", back, || bar_of(&term));
    says(&team, "dev2", "working");
    let cleared =
        wait(Duration::from_secs(5), || !bar_of(&term).contains('\u{2717}') && !bar_of(&term).contains('\u{2691}'));
    checks.check("both gone once they work", cleared, || bar_of(&term));
    drop(term);
    checks.done();
    team.stop();
}

/// The notice (top right) of a member out of view whose turn failed, naming it with ✗; two failures less than ten
/// seconds apart make one notice, not two.
#[test]
fn a_failed_turn_out_of_view_makes_one_notice() {
    let mut team = Team::new("noticefail", &tables("noticefail", &["lead"], &[("dev1", None), ("dev2", None)], ""));
    team.launch(&["lead", "dev1", "dev2"]);
    let term = client(&team, "lead");
    let mut checks = Checks::default();
    // dev1 and dev2 are out of view (the contact's tab). A notice is a row at the top with ✗ and « ⌥g y aller »: the
    // dashboard's cards of the two also carry ✗ and their names, but no key.
    let notice = |term: &TestTerm| -> Option<String> {
        term.screen()
            .iter()
            .take(6)
            .find(|r| r.contains('\u{2717}') && (r.contains("\u{2325}g") || r.contains("Alt+g")))
            .cloned()
    };
    fails(&team, "dev1", OVERLOADED);
    let began = Instant::now();
    let there = wait(Duration::from_secs(4), || notice(&term).is_some());
    checks.check("a notice for the failed member", there, || shown(&term));
    std::thread::sleep(Duration::from_millis(1500));
    fails(&team, "dev2", OVERLOADED);
    // Watch for ten seconds: the notice goes out once, and nothing comes back after it.
    let mut ended: Option<Duration> = None;
    let mut again = false;
    while began.elapsed() < Duration::from_secs(11) {
        let up = notice(&term).is_some();
        match (ended, up) {
            (None, false) => ended = Some(began.elapsed()),
            (Some(_), true) => again = true,
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    checks.check("the notice ends (six seconds or so)", ended.is_some_and(|e| e < Duration::from_secs(10)), || {
        format!("{ended:?}")
    });
    checks.check("no second notice for the second failure", !again, || shown(&term));
    checks.check("the badge stays on the tab", bar_of(&term).contains('\u{2717}'), || bar_of(&term));
    drop(term);
    checks.done();
    team.stop();
}

/// ⌥g goes to the members who wait first (the longest first), then to those whose turn failed (the longest first).
#[test]
fn alt_g_goes_to_who_waits_then_to_who_failed() {
    let agents: Vec<(&str, Option<&str>)> = vec![("dev1", None), ("dev2", None), ("dev3", None)];
    let mut team = Team::new("gofail", &tables("gofail", &["lead"], &agents, ""));
    team.launch(&["lead", "dev1", "dev2", "dev3"]);
    let mut term = client(&team, "lead");
    let members = ["lead", "dev1", "dev2", "dev3"];
    let go = |term: &mut TestTerm| {
        let keys = alt(term, 'g');
        term.send(&keys);
    };
    let home = |term: &mut TestTerm| {
        let keys = alt(term, '1');
        term.send(&keys);
        wait(Duration::from_secs(3), || active(term, &members) == ["lead"]);
    };
    let mut checks = Checks::default();
    // dev1 failed first, then dev3, then dev2 waits (later than both): waiting comes before failing.
    fails(&team, "dev1", OVERLOADED);
    std::thread::sleep(Duration::from_millis(1200));
    fails(&team, "dev3", OVERLOADED);
    std::thread::sleep(Duration::from_millis(1200));
    says(&team, "dev2", "blocked:kind=permission");
    std::thread::sleep(Duration::from_millis(500));
    go(&mut term);
    let got = wait(Duration::from_secs(3), || active(&term, &members) == ["dev2"]);
    checks.check("F6 first to who waits", got, || format!("{:?}\n{}", active(&term, &members), shown(&term)));
    says(&team, "dev2", "working");
    std::thread::sleep(Duration::from_millis(500));
    home(&mut term);
    go(&mut term);
    let got = wait(Duration::from_secs(3), || active(&term, &members) == ["dev1"]);
    checks
        .check("F6 then to who failed longest ago", got, || format!("{:?}\n{}", active(&term, &members), shown(&term)));
    says(&team, "dev1", "working");
    std::thread::sleep(Duration::from_millis(500));
    home(&mut term);
    go(&mut term);
    let got = wait(Duration::from_secs(3), || active(&term, &members) == ["dev3"]);
    checks.check("F6 then to the next who failed", got, || format!("{:?}\n{}", active(&term, &members), shown(&term)));
    drop(term);
    checks.done();
    team.stop();
}

/// An error whose text changes length when lowercased (« İ », which becomes two characters) must not bring anything
/// down: the server answers (the same one), the header shows the failed turn, and the dashboard and the journal are
/// still there.
#[test]
fn an_error_text_with_odd_letters_panics_nothing() {
    let mut team = Team::new("odd", &tables("odd", &["lead", "co"], &[], ""));
    team.launch(&["lead", "co"]);
    let term = client(&team, "lead");
    let pid = |team: &Team| {
        serde_json::from_str::<serde_json::Value>(team.ctl(["where"]).trim()).expect("where")["pid"].clone()
    };
    let before = pid(&team);
    let mut checks = Checks::default();
    for member in ["lead", "co"] {
        says(&team, member, "working");
    }
    assert!(header_works(&term, "lead", true, Duration::from_secs(5)), "{}", shown(&term));
    fails(&team, "lead", "\u{130} status 500");
    fails(&team, "co", "API Error: \u{130}\u{130}\u{130} \u{1F4A5} \u{FB03} 529 \u{130}");
    for member in ["lead", "co"] {
        let shows = wait(Duration::from_secs(5), || header_sign(&term, member) == Some('\u{2717}'));
        checks.check(&format!("{member}: the header shows the failed turn"), shows, || shown(&term));
    }
    // Long enough for the dashboard, the journal and the menu to have read it.
    std::thread::sleep(Duration::from_secs(4));
    checks.check("the same server answers", team.running() && pid(&team) == before, || {
        format!("{before} {:?}", team.ctl_output(["where"]))
    });
    let roles: Vec<String> = team.panes().iter().filter_map(|p| p["role"].as_str().map(str::to_string)).collect();
    checks.check(
        "the dashboard and the journal are still there",
        roles.iter().any(|r| r == "dashboard") && roles.iter().any(|r| r == "journal"),
        || format!("{roles:?}"),
    );
    let errors = state_errors(&team);
    checks.check("states.json carries both texts", errors["lead"].is_string() && errors["co"].is_string(), || {
        errors.to_string()
    });
    drop(term);
    checks.done();
    team.stop();
}
