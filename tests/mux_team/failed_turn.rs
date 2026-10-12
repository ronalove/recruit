// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

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
