// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

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

/// A dashboard of this build in front of a server of the previous version (`OLD_RECRUIT=<its binary>`; one whose
/// `Panes` has no `status`, the 2.0.0, tests the fallback): it must not break, and fall back to asking `claude agents` once a second, no more. The server has no
/// dashboard of its own (`dashboard = false`), so the only `_panel dashboard` is this one.
#[test]
#[ignore = "needs OLD_RECRUIT=<the previous version's binary>: run with --ignored"]
fn the_dashboard_in_front_of_the_previous_server() {
    let old = std::env::var_os("OLD_RECRUIT").expect("OLD_RECRUIT: the path of the previous version's binary");
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
    let now = common::now_secs();
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

/// The passage the release notes promise: a team running under the previous version (`OLD_RECRUIT=<its binary>`), its
/// members having noted their sessions, then `recruit --restart --resume` with this build. The old server ends, a new
/// one runs, and each member is on its conversation again (`-r <id>`), the previous `states.json` still there.
#[test]
#[ignore = "needs OLD_RECRUIT=<the previous version's binary>: run with --ignored"]
fn a_team_of_the_previous_version_passes_to_this_build() {
    let old = std::env::var_os("OLD_RECRUIT").expect("OLD_RECRUIT: the path of the previous version's binary");
    let mut team = Team::new("passage", &tables("passage", &["lead"], &[("dev", None)], ""));
    team.use_binary(std::path::Path::new(&old));
    team.launch(&["lead", "dev"]);
    let sessions: Vec<(&str, String)> = ["lead", "dev"].iter().map(|m| (*m, noted(&team, m))).collect();
    let before = team.ctl(["where"]);
    let states = std::fs::read_to_string(team.state().join("states.json")).unwrap_or_default();
    println!("previous server: {}\nstates.json from the previous version: {} bytes", before.trim(), states.len());
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
