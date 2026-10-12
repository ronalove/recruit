// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

/// The server gives a pane a new engine, blank, after a panic in the old one, and tells its program its size again
/// (SIGWINCH twice, so that it changes). The dashboard writes by difference: at rest, it writes the whole board
/// again on that signal, the size unchanged, and not 30 s later.
#[test]
fn the_dashboard_draws_again_on_sigwinch() {
    let mut team = Team::new("winch", &tables("winch", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    report(&team, "lead", "claude-opus-5-5", "xhigh", 85.0);
    says(&team, "lead", "idle");
    let state = team.state().to_string_lossy().to_string();
    let term = team.in_terminal(&["_panel", "dashboard", &state], 70, 24, Profile::default());
    let drawn = wait(Duration::from_secs(5), || shown(&term).contains("lead"));
    assert!(drawn, "{}", shown(&term));
    // The first image: all that was written until the output paused for 300 ms.
    let mut first = term.counters().0;
    let mut still = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while still < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        let now = term.counters().0;
        still = if now == first { still + 1 } else { 0 };
        first = now;
    }
    // At rest: then only the times that move. Waited for, not guessed: two seconds in a row (one-second windows) each
    // writing less than a quarter of the first image, 20 s at most.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut calm = 0;
    while calm < 2 && Instant::now() < deadline {
        let so_far = term.counters().0;
        std::thread::sleep(Duration::from_secs(1));
        calm = if (term.counters().0 - so_far) * 4 < first { calm + 1 } else { 0 };
    }
    assert!(calm == 2 && first > 400, "{first} bytes to draw, the output never calmed down: {}", shown(&term));
    // SAFETY: a signal to the dashboard's process, which the test terminal started.
    assert_eq!(unsafe { libc::kill(term.pid() as libc::pid_t, libc::SIGWINCH) }, 0);
    let before = term.counters().0;
    let again = wait(Duration::from_secs(4), || term.counters().0 - before >= first / 2);
    assert!(again, "{} bytes after SIGWINCH, {first} to draw it: {}", term.counters().0 - before, shown(&term));
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
