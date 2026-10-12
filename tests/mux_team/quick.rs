// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

/// Waits until the quick list is gone: its pane closed and nothing of it on the screen.
fn wait_list_gone(team: &Team, term: &TestTerm) -> bool {
    wait(Duration::from_secs(5), || menu_text(team).is_none() && !shown(term).contains("défaut"))
}

/// Clicks one cell inside `text` in a header and waits for the list. The text is looked for now, not kept from before:
/// a header's time is right-aligned and grows ("9s", then "10s"), which moves what is left of it by a cell, and a click
/// at the old column then lands beside the word (the macOS CI of 7536dc9, "the list opens again").
fn open_on(team: &Team, term: &mut TestTerm, text: &str) -> Option<ListBox> {
    let (col, row) = find_in_header(term, text)?;
    click(term, (col + 1, row));
    wait_list(team, term)
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
    let open = |term: &mut TestTerm, text: &str| open_on(&team, term, text);

    // Opens under the text, at the size announced, the text in reverse, nothing dimmed.
    let Some(list) = open(&mut term, "Opus") else { panic!("no list: {}", shown(&term)) };
    let model = find_in_header(&term, "Opus").expect("the model, with the list open");
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
    if open(&mut term, "Opus").is_some() {
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
    if open(&mut term, "Opus").is_some() {
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
    if let Some(list) = open(&mut term, "Opus") {
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
    if let Some(list) = open(&mut term, "élevé") {
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
    if let Some(list) = open(&mut term, "Sonnet") {
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
    // Back to the default starts it again (↻): the question opens in the list, on Annuler.
    assert!(open_on(&team, &mut term, "Opus").is_some(), "no list: {}", shown(&term));
    checks.check(
        "3 the default row says ↻",
        shown(&term).lines().any(|r| r.contains("défaut") && r.contains('↻')),
        || shown(&term),
    );
    term.send(b"\x1b[A");
    term.send(b"\r");
    let asked = wait(Duration::from_secs(10), || shown(&term).contains("Annuler"));
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
    if open_on(&team, &mut term, "Opus").is_some() {
        term.send(b"\x1b[A");
        term.send(b"\r");
        let _ = wait(Duration::from_secs(10), || shown(&term).contains("Annuler"));
        term.send(b"r");
        let again = wait(Duration::from_secs(10), || team.launches("dev").len() == 2);
        checks.check("3 Relancer: dev starts again", again, || format!("{:?}", team.launches("dev")));
        if again {
            let second = team.launches("dev")[1].clone();
            checks.check("3 Relancer: on its conversation", second.ends_with(&format!("-n dev -r {session}")), || {
                second.clone()
            });
        }
        let written = wait(Duration::from_secs(10), || !team_file(&team).contains("model = \"opus\""));
        checks.check("3 Relancer: the default leaves the key out", written, || team_file(&team));
        checks.check("3 Relancer: the list closes", wait_list_gone(&team, &term), || shown(&term));
    } else {
        checks.check("3 the list opens again", false, || shown(&term));
    }
    // A write that is refused: the list stays, with the reason.
    let keys = alt(&term, '1');
    term.send(&keys);
    assert!(wait(Duration::from_secs(10), || has_header(&term, "lead")), "{}", shown(&term));
    let _read_only = ReadOnly::new(team.proj.join(".recruit"));
    if open_on(&team, &mut term, "Opus").is_some() {
        term.send(b"\x1b[B");
        term.send(b"\r");
        let refused = wait(Duration::from_secs(10), || shown(&term).contains("✗"));
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
