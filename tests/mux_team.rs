// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A whole team on the native multiplexer, launched by `recruit` with a fake `claude` (`common::team`): the parity
//! checks of the spec's §7.1 (grid, « Parité ») that need no window. Part of `cargo test`.
//!
//! Owner: testeur.

mod common;

use std::time::{Duration, Instant};

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
#[ignore = "5 s, the 2 s before member.rs notes a session and the 2 s before it starts Claude again: run with --ignored"]
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
