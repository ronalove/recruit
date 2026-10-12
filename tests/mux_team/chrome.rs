// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

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
    // The server's state, not the screen alone: the menu clears its screen before its process ends, and a click in
    // between is swallowed (`Target::Quit if self.menu.is_some()`).
    check("R1 Esc closes the menu", close_menu(&team, &mut term), screen(&term));
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
    check("R1 Esc closes the menu opened by the key", close_menu(&team, &mut term), screen(&term));
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    team.stop();
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
