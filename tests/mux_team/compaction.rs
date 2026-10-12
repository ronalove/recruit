// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

/// What the member's mod says of the compaction `id` it was asked (`compact/<member>`): `outcome` is
/// `{"done": true}`, `{"skip": "helpers"}` or `{"error": "…"}`, in its report (`bridge::Compaction`).
fn answer_compaction(team: &Team, member: &str, id: &str, outcome: serde_json::Value) {
    let file = team.state().join("members").join(format!("{member}.json"));
    let mut report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("a report to complete")).expect("report: JSON");
    let at = common::now_secs();
    let mut compaction = serde_json::json!({"id": id, "auto": true, "at": at});
    for (key, value) in outcome.as_object().expect("an outcome") {
        compaction[key] = value.clone();
    }
    report["compaction"] = compaction;
    std::fs::write(file, report.to_string()).expect("report");
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
    let began = common::now_secs();
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
    let now = common::now_secs;
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
