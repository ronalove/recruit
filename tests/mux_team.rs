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
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let report = serde_json::json!({
        "session": "s", "model": model, "own": {"model": model, "effort": effort},
        "context": {"percent": percent, "tokens": (percent * 2000.0) as u64, "window": 200000}, "at": at,
    });
    std::fs::write(dir.join(format!("{member}.json")), report.to_string()).expect("report");
}

/// The parts of a member's header that show on `row`: the name, the context, the sign of the effort, the effort's
/// word, the model, the time in the state, the zoom sign.
fn header_parts(row: &str) -> [bool; 7] {
    let has_time = row.split_whitespace().any(|w| {
        w.len() >= 2
            && w.ends_with(['s', 'm', 'h'])
            && w[..w.len() - 1].chars().all(|c| c.is_ascii_digit() || c == 'h' || c == 'm')
    });
    [
        row.contains(" lead"),
        row.contains('%'),
        row.contains('\u{2588}'),
        row.contains("xhigh"),
        row.contains("Opus"),
        has_time,
        row.contains('\u{2922}'),
    ]
}

/// F1: a member's header says its state, name, model, effort, context, time in the state and the zoom sign, and
/// gives up what does not fit in the planned order: the effort's word, the model, the time, the effort's sign; the
/// context last, then the name is cut.
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
    checks.check("F1 everything fits at 120 columns", parts.iter().all(|p| *p), || format!("{parts:?} {full}"));
    checks.check(
        "F1 the state's sign before the name",
        full.contains("\u{25f7} lead") || full.contains("\u{25cc} lead"),
        || full.clone(),
    );
    // Narrower and narrower: what stays at each width implies what is less urgent.
    // name ≥ context ≥ sign ≥ time ≥ model ≥ word (the zoom sign is kept apart).
    let mut seen_all = false;
    let mut last: Option<[bool; 7]> = None;
    for cols in (14..=120u16).rev().step_by(7) {
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
        let chain = [p[0], p[1], p[2], p[5], p[4], p[3]];
        let ordered = chain.windows(2).all(|w| w[0] || !w[1]);
        checks.check(&format!("F1 order of giving up at {cols} columns"), ordered, || format!("{p:?} {row}"));
        if last.is_some_and(|before| before != p) {
            println!("{cols:3} columns: {p:?}");
        }
        seen_all |= p.iter().all(|x| *x);
        last = Some(p);
    }
    checks.check("F1 the whole header at some width", seen_all, String::new);
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
    let focused = |lines: &[String], label: &str| lines.iter().any(|row| row.contains(label) && row.contains('‹'));
    for (member, tab_key, part, label) in [
        ("lead", '1', "Opus", "Modèle"),
        ("lead", '1', "xhigh", "Effort"),
        ("dev", '2', "Sonnet", "Modèle"),
        ("dev", '2', "high", "Effort"),
    ] {
        let keys = alt(&term, tab_key);
        term.send(&keys);
        // The member's own header first: the other tab's shows « xhigh » when « high » is looked for.
        let there = wait(Duration::from_secs(3), || has_header(&term, member) && find_in_header(&term, part).is_some());
        assert!(there, "{part}: {}", shown(&term));
        let at = find_in_header(&term, part).expect("part");
        click(&mut term, at);
        let opened = wait(Duration::from_secs(8), || menu_text(&team).is_some());
        checks.check(&format!("F2 a click on {member}'s {part} opens the menu"), opened, || shown(&term));
        let lines = menu_text(&team).unwrap_or_default();
        let _ = wait(Duration::from_secs(3), || menu_text(&team).is_some_and(|l| focused(&l, label)));
        let lines = menu_text(&team).unwrap_or(lines);
        checks.check(&format!("F2 {member}'s sheet"), sheet(&lines, member), || lines.join("\n"));
        checks.check(&format!("F2 the field {label} in focus"), focused(&lines, label), || lines.join("\n"));
        let layer = wait(Duration::from_secs(3), || shown(&term).contains("Nouvel agent"));
        checks.check("F2 the menu is a layer over the team", layer, || shown(&term));
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
        "F3 20 changes of element: 10 to 25 frames, the clocks apart",
        (10..=25 + still + SLACK).contains(&changes),
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
