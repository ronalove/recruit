// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::super::sheet::fixture::{Fake, team};
use super::super::sheet::{Entry, Key, Said, Sheet};
use super::sheet::paragraphs;
use super::*;

fn look() -> Look {
    Look { glyphs: Glyphs::Unicode, frame: 0, now: 1000, option: true }
}

fn rows(drawn: &Drawn) -> Vec<String> {
    (0..drawn.canvas.height()).map(|y| drawn.canvas.row(y)).collect()
}

fn sheet() -> Sheet {
    let mut s = Sheet::new(team(&[("coordinateur", true), ("dev-cli", false), ("ops", false)]));
    s.states.insert("coordinateur".into(), (State::Working, 820));
    s.states.insert("dev-cli".into(), (State::Idle, 160));
    s.states.insert("ops".into(), (State::Waiting, 960));
    s
}

/// No size, however small, makes the menu panic, in any of its states: the list, the sheet, a dropdown, a
/// confirmation, the new agent's form, a name being typed, a team whose files cannot be read (reviewer's sweep,
/// kept, with the typing and the unreadable team it had not drawn).
#[test]
fn no_size_too_small() {
    use super::super::sheet::{Action as A, Fid};
    let env = Fake::quiet();
    let typed = |fid: Fid, text: &str| {
        let mut s = sheet();
        s.key(Key::Down, &env);
        s.key(Key::Enter, &env);
        s.row = s.focusable().iter().position(|f| *f == fid).expect("the row");
        s.key(Key::Enter, &env);
        for c in text.chars() {
            s.key(Key::Char(c), &env);
        }
        s
    };
    let after = |keys: &[Key]| {
        let mut s = sheet();
        for key in keys {
            s.key(key.clone(), &env);
        }
        s
    };
    let mut unreadable = Sheet::new(team(&[("coordinateur", true), ("dev-cli", false)]));
    unreadable.team.unreadable = Some(format!("{}: {}", "settings.toml", "valeur inattendue ".repeat(12)));
    unreadable.reload(unreadable.team.clone(), None);
    let states = [
        sheet(),
        after(&[Key::Down]),
        after(&[Key::Enter]),
        after(&[Key::Enter, Key::Enter]),
        after(&[Key::Char(A::Stop.key())]),
        after(&[Key::Char(A::New.key())]),
        after(&[Key::Down, Key::Down, Key::Down, Key::Down, Key::Down, Key::Down, Key::Enter]),
        typed(Fid::Name, "un-nom-de-membre-vraiment-très-long-日本語"),
        typed(Fid::Role, &"Un rôle écrit à la main, long ".repeat(4)),
        unreadable,
    ];
    // The states drawn are those meant.
    use super::super::sheet::{Overlay, Said};
    assert!(matches!(states[3].overlay, Overlay::List(_)), "{:?}", states[3].overlay);
    assert!(matches!(states[4].overlay, Overlay::Confirm(_)), "{:?}", states[4].overlay);
    assert!(states[7..9].iter().all(|s| matches!(s.overlay, Overlay::Typing(_))));
    assert!(matches!(states[9].said, Some(Said::Failed(_))));
    let failed = std::sync::Mutex::new(Vec::new());
    // Every small size, which carry nearly all the overflows, then a step.
    let widths: Vec<usize> = (0..=60).chain((66..=120).step_by(6)).collect();
    let heights: Vec<usize> = (0..=20).chain((25..=40).step_by(5)).collect();
    std::thread::scope(|scope| {
        for chunk in widths.chunks(widths.len().div_ceil(8)) {
            let (states, heights, failed) = (&states, &heights, &failed);
            scope.spawn(move || {
                for &width in chunk {
                    for &height in heights {
                        for (i, s) in states.iter().enumerate() {
                            for option in [true, false] {
                                let look = Look { option, ..look() };
                                if std::panic::catch_unwind(|| draw(s, &look, width, height)).is_err() {
                                    failed.lock().unwrap().push(format!("state {i} at {width} × {height}"));
                                }
                            }
                        }
                    }
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    assert!(failed.is_empty(), "{} failed: {}", failed.len(), failed.join(", "));
}

/// The server's version after « recruit », and the arrow after it only when a newer one is out; the team's name gives
/// way before either when the room is short.
#[test]
fn version_and_update_in_the_header() {
    let mut s = sheet();
    s.version = "2.3.1".into();
    let plain = rows(&draw(&s, &look(), 100, 28))[0].clone();
    assert!(plain.starts_with(&format!(" recruit 2.3.1{}", t!(" · équipe", " · team"))), "{plain}");
    for arrow in ['↑', '\u{F005D}'] {
        assert!(!plain.contains(arrow), "{plain}");
    }
    s.update = Some("2.4.0".into());
    let unicode = rows(&draw(&s, &look(), 100, 28))[0].clone();
    assert!(unicode.starts_with(&format!(" recruit 2.3.1 ↑{}", t!(" · équipe", " · team"))), "{unicode}");
    let nerd = Look { glyphs: Glyphs::Nerd, ..look() };
    let nerd = rows(&draw(&s, &nerd, 100, 28))[0].clone();
    assert!(nerd.starts_with(" recruit 2.3.1 \u{F005D} "), "{nerd}");
    // Narrow: the version and the arrow stay, the team's name is cut.
    s.team.name = "une-équipe-au-nom-très-long".into();
    let narrow = rows(&draw(&s, &look(), 50, 14))[0].clone();
    assert!(narrow.starts_with(" recruit 2.3.1 ↑ · "), "{narrow}");
    assert!(narrow.contains('…') && narrow.contains("⠋ 1"), "{narrow}");
    assert!(columns(&narrow) <= 50, "{narrow}");
}

#[test]
fn wide() {
    let mut s = sheet();
    s.key(Key::Down, &Fake::quiet());
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    assert!(text[0].starts_with(&format!(" recruit {} · ", s.version)), "{}", text[0]);
    assert!(text[0].ends_with(&t!("Échap ferme", "Esc closes")));
    assert!(text[0].contains("⠋ 1   ⚑ 1   ◷ 1"), "the states counted: {}", text[0]);
    assert_eq!(text[1].chars().nth(34), Some('┬'), "the list 34 wide");
    assert!(text[3].contains("⠋ coordinateur") && text[3].ends_with("") && text[3].contains("3m"), "{}", text[3]);
    assert!(
        text[4].contains(&format!("Opus   {}", look::effort_label("high"))),
        "the model, then the effort, its sign and its name: {}",
        text[4]
    );
    assert!(text[2].contains("dev-cli") && text[2].contains("◷"), "the sheet's title: {}", text[2]);
    let model = text.iter().find(|r| r.contains(&t!("Modèle", "Model"))).unwrap();
    assert!(model.contains("Opus") && model.trim_end().ends_with(&t!("équipe", "team")), "{model}");
    assert!(text[26].contains(&t!(" n Nouvel agent ", " n New agent ")), "the team's buttons: {}", text[26]);
    assert!(text[27].contains("↑↓"), "the keys");
    // A click on a card, on a button.
    assert_eq!(drawn.hit(5, 3), Some(Target::Entry(Entry::Member("coordinateur".into()))));
    assert!(matches!(drawn.hit(3, 26), Some(Target::Action(Action::New))));
    assert_eq!(drawn.split, 34);
}

#[test]
fn the_team_settings_have_a_row() {
    let mut s = sheet();
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    let (grid, share) = (t!("Grille : 3 × 2", "Grid: 3 × 2"), t!("Compaction auto : 70 %", "Auto-compaction: 70%"));
    let row = &text[25];
    assert!(row.contains(&t!("Tableau de bord : oui", "Dashboard: yes")), "{row}");
    assert!(row.contains(&grid) && row.contains(&share), "{row}");
    assert!(row.contains(&t!("Disposition : grille", "Layout: grid")), "{row}");
    assert!(!text[26].contains(&grid) && text[26].contains(&t!("Détacher", "Detach")), "{}", text[26]);
    let x = row[..row.find(&grid).unwrap()].chars().count() + 2;
    assert!(matches!(drawn.hit(x, 25), Some(Target::Action(Action::Grid))));
    // The list opens above its button, the question under the rows.
    s.key(Key::Char(Action::Grid.key()), &Fake::quiet());
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    let top = text.iter().position(|r| r.contains("3 × 2") && r.contains('›')).expect("the list is drawn");
    assert!(top < 25, "above the buttons: {top}");
    assert!(text.iter().any(|r| r.contains("2 × 1")));
    assert!(matches!(drawn.hit(x, top + 1), Some(Target::Item(2))), "{:?}", drawn.hit(x, top + 1));
    // Short of height, the settings share the actions' row, last.
    let short = rows(&draw(&sheet(), &look(), 100, 18));
    let row = &short[16];
    assert!(row.contains(&t!("Détacher", "Detach")) && row.contains(&t!("Quitter", "Quit")), "{row}");
    // The grid keeps its value before anything is cut; the share, last, goes first if the row is still short.
    assert!(row.contains(" g 3 × 2"), "the short form before cutting: {row}");
}

/// In a light terminal the menu's own backgrounds are the light palette's: the buttons 252, the chosen card 254.
#[test]
fn the_menu_in_the_light_palette() {
    let button = |drawn: &Drawn| {
        let zone = drawn.zones.iter().find(|z| z.target == Target::Action(Action::New)).unwrap();
        drawn.canvas.style(zone.x + 3, zone.y).bg
    };
    let s = sheet();
    assert_eq!(button(&draw(&s, &look(), 100, 28)), Some(Color::AnsiValue(236)));
    look::set_light(true);
    assert_eq!(button(&draw(&s, &look(), 100, 28)), Some(Color::AnsiValue(252)));
    look::set_light(false);
}

/// At 80 × 24 the row over the actions, free while nothing is said, holds the team's settings: a key and its value.
#[test]
fn narrow_settings_on_the_free_row() {
    let mut s = sheet();
    let drawn = draw(&s, &look(), 80, 24);
    let text = rows(&drawn);
    let row = text.iter().position(|r| r.contains(&t!("tableau oui", "dashboard yes"))).expect("the settings row");
    assert_eq!(row, 21, "the row above the actions: {text:#?}");
    let line = text[row].trim_end();
    let expected = t!(" t tableau oui  l grille  g 3 × 2  c ⟳ 70 %", " t dashboard yes  l grid  g 3 × 2  c ⟳ 70%");
    assert_eq!(line, expected);
    // They click, and the actions' row has no setting left.
    let at = text[row][..text[row].find(" g 3").unwrap()].chars().count() + 1;
    assert!(matches!(drawn.hit(at, row), Some(Target::Action(Action::Grid))));
    assert!(!text[22].contains(&t!("grille", "grid")) && !text[22].contains("disposition"), "{}", text[22]);
    assert!(text[22].contains("n ") && text[22].contains("q "), "{}", text[22]);
    // Something said takes the row: the settings go back to the actions' row, the first to go.
    s.said = Some(super::super::sheet::Said::Done("ok".into()));
    let text = rows(&draw(&s, &look(), 80, 24));
    assert!(!text.iter().any(|r| r.contains(&t!("tableau oui", "dashboard yes"))), "{text:#?}");
}

/// The sheet's effort row in colors, the applied level of Claude Code's own default included: « ● élevé » at the left,
/// « Claude Code » at the right (not « défaut de Claude Code » and « défaut »).
#[test]
fn the_sheet_shows_the_applied_effort_in_its_colors() {
    let s = sheet();
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    let y = text.iter().position(|r| r.contains(&t!("Effort", "Effort"))).unwrap();
    let label = look::effort_label("high");
    assert!(text[y].contains(&label) && text[y].trim_end().ends_with("Claude Code"), "{}", text[y]);
    let at = text[y][..text[y].find(&label).unwrap()].chars().count();
    assert_eq!(drawn.canvas.style(at, y).fg, look::effort_color("high"), "the sign in its color");
    assert_eq!(drawn.canvas.style(at + 2, y).fg, look::effort_color("high"), "the name too");
}

#[test]
fn narrow() {
    let mut s = sheet();
    s.key(Key::Enter, &Fake::quiet());
    let drawn = draw(&s, &look(), 64, 22);
    let text = rows(&drawn);
    assert_eq!(drawn.split, 18);
    assert!(text[0].contains("« essai »") || text[0].contains("\"essai\""));
    assert!(!text[3].contains("3m"), "no times in the list: {}", text[3]);
    let hint = text.iter().find(|r| r.contains("↳")).unwrap();
    assert!(hint.contains(&t!("équipe", "team")), "the origin under the sheet: {hint}");
    // The actions on their row, the settings, a key and a value each, on the free one above.
    assert!(text[20].contains("n ") && text[20].contains("q "), "the team's keys: {}", text[20]);
    assert!(text[19].contains(&t!("tableau oui", "dashboard yes")), "the team's settings: {}", text[19]);
    assert!(text.iter().all(|r| columns(r) <= 64));
}

#[test]
fn stopping_stays_in_sight() {
    // At the narrowest the menu draws, quitting and detaching always show. Short of room (in French, whose words
    // are longer), resetting, the least needed, gives its place first.
    let text = rows(&draw(&sheet(), &look(), 50, 14));
    let keys = &text[12];
    assert!(keys.contains(&t!("quitter", "quit")) && keys.contains(&t!("détacher", "detach")), "{keys}");
    let reset = keys.contains(&t!("réinitialiser", "reset"));
    assert!(!reset || keys.contains("n "), "the least needed went first: {keys}");
    assert!(columns(keys) <= 50, "{keys}");
}

#[test]
fn a_question_is_no_result() {
    let mut s = sheet();
    s.said = Some(Said::Asked("Qui le devient à sa place ?".into()));
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    let y = text.iter().position(|r| r.contains("Qui le devient")).unwrap();
    assert_eq!(text[y], " Qui le devient à sa place ?", "no sign");
    assert_eq!(drawn.canvas.style(1, y), Style::PLAIN, "in the plain color");
}

#[test]
fn too_small() {
    let drawn = draw(&sheet(), &look(), 40, 10);
    assert!(rows(&drawn)[5].contains("50 × 14"));
    assert!(drawn.zones.is_empty());
}

#[test]
fn a_list_under_its_row() {
    let mut s = sheet();
    let env = Fake::quiet();
    s.key(Key::Enter, &env);
    s.key(Key::Enter, &env);
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    let first = text.iter().position(|r| r.contains("╭")).expect("a box");
    assert!(
        text[first + 1].contains("› ") && text[first + 1].contains(&t!("actuel", "current")),
        "{}",
        text[first + 1]
    );
    assert!(text[first + 2].contains("Sonnet"), "Opus is the default's");
    assert!(matches!(drawn.hit(60, first + 2), Some(Target::Item(1))));
}

#[test]
fn a_dialog_fades_the_rest() {
    let mut s = sheet();
    s.key(Key::Char(super::super::sheet::Action::Stop.key()), &Fake::quiet());
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    assert!(text.iter().any(|r| r.contains(&t!("Quitter l'équipe ?", "Quit the team?"))));
    assert_eq!(drawn.canvas.style(1, 0).fg, Some(faded()), "the rest faded");
    assert!(drawn.zones.iter().all(|z| matches!(z.target, Target::Yes | Target::No)), "only its buttons take a click");
}

#[test]
fn one_waiting_in_red() {
    let mut s = sheet();
    let env = Fake::quiet();
    s.choose(Entry::Member("ops".into()));
    s.key(Key::Enter, &env);
    while s.field() != Some(Fid::Restart) {
        s.key(Key::Down, &env);
    }
    s.key(Key::Enter, &env);
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    let y = text.iter().position(|r| r.contains("⚑ « ops »") || r.contains("⚑ \"ops\"")).expect("the waiting line");
    let x = text[y].find('⚑').map(|b| text[y][..b].chars().count()).unwrap();
    assert_eq!(drawn.canvas.style(x, y).fg, Some(Color::Red));
}

/// A member whose turn failed: ✗ in orange in the list; on its sheet, since when and the error whole,
/// cut at its own line breaks then between words, then « Reprendre » chosen.
#[test]
fn a_failed_turn_on_its_sheet() {
    let mut s = sheet();
    s.states.insert("dev-cli".into(), (State::Error, 760));
    let error =
        "API Error: 529\n{\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}";
    s.failed([("dev-cli".to_string(), error.to_string())].into());
    s.open_on("dev-cli", None);
    let drawn = draw(&s, &look(), 100, 30);
    let text = rows(&drawn);
    let failed = t!("Tour raté il y a {}", "Turn failed {} ago", "4m");
    let y = text.iter().position(|r| r.contains(&failed)).expect("the failure");
    let x = text[y].find('✗').map(|b| text[y][..b].chars().count()).unwrap();
    assert_eq!(drawn.canvas.style(x, y).fg, Some(State::Error.color()));
    assert!(text[y + 1].trim_end().ends_with("API Error: 529"), "{:?}", text[y + 1]);
    assert!(text[y + 2].contains("{\"type\":\"error\""), "{:?}", text[y + 2]);
    let resume = text.iter().position(|r| r.contains(&t!("Reprendre", "Resume"))).expect("the row");
    assert!(resume > y + 2 && text[resume].contains("continue"), "{:#?}", text);
    // Its help whole, over two rows, cut between words.
    let help = text.iter().position(|r| r.contains("↳")).expect("the help");
    let after = |row: &str, mark: char| row[row.find(mark).unwrap() + mark.len_utf8()..].trim().to_string();
    let joined = format!("{} {}", after(&text[help], '↳'), after(&text[help + 1], '│'));
    let said = t!(
        "Lui envoie « continue » ; il reprend où il s'est arrêté. Rien n'est envoyé si sa saisie contient du texte.",
        "Sends it “continue”; it picks up where it stopped. Nothing is sent if its prompt holds a draft."
    );
    assert_eq!(joined, said, "{:#?}", &text[help..help + 2]);
    // In the list, ✗ in orange.
    let row = text.iter().position(|r| r.contains("✗ dev-cli")).expect("the list");
    let x = text[row].find('✗').map(|b| text[row][..b].chars().count()).unwrap();
    let style = drawn.canvas.style(x, row);
    assert_eq!((style.fg, style.bold), (Some(State::Error.color()), true));
    // Cut at its line breaks, then between words; « … » when it goes on.
    assert_eq!(paragraphs("a b\nc", 10, 2), ["a b", "c"]);
    assert_eq!(paragraphs("a\tb", 10, 2), ["a b"]);
    assert_eq!(paragraphs("a b\nc\nd", 10, 2), ["a b", "c…"]);
    assert_eq!(paragraphs("un deux trois", 7, 1), ["un deu…"]);
    assert!(paragraphs("", 7, 2).is_empty());
}

/// The focused row always shows, whatever the field and the size: on a member whose turn
/// failed, the blank under its error gives way first, then its error, then the role. At 64 × 19, the menu's
/// layer of the capture (62 × 15) and the smallest it takes.
#[test]
fn the_focused_row_always_shows() {
    let env = Fake::quiet();
    let mut s = sheet();
    s.states.insert("dev-cli".into(), (State::Error, 995));
    s.failed([("dev-cli".to_string(), "API Error: 529\n{\"type\":\"error\",\"error\":{}}".to_string())].into());
    s.open_on("dev-cli", None);
    let resume = t!("Reprendre", "Resume");
    for (w, h) in [(64, 19), (62, 15), (MIN_WIDTH, MIN_HEIGHT), (100, 14), (140, 40)] {
        let text = rows(&draw(&s, &look(), w, h));
        assert!(text.iter().any(|r| r.contains(&resume)), "{w} × {h}: {text:#?}");
        assert!(text.iter().any(|r| r.contains(&t!("Tour raté", "Turn failed"))), "{w} × {h}: {text:#?}");
    }
    // At 64 × 19, the error still there, only its blank given up.
    let text = rows(&draw(&s, &look(), 64, 19));
    assert!(text.iter().any(|r| r.contains("API Error: 529")), "{text:#?}");
    // Every field, every size: its row drawn, a click on it there.
    for name in ["dev-cli", "ops"] {
        s.open_on(name, None);
        s.key(Key::Right, &env);
        let count = s.focusable().len();
        for row in 0..count {
            s.pane = super::super::sheet::Pane::Sheet;
            s.row = row;
            for h in MIN_HEIGHT..24 {
                for w in [MIN_WIDTH, 64, WIDE, 120] {
                    let drawn = draw(&s, &look(), w, h);
                    assert!(
                        drawn.zones.iter().any(|z| z.target == Target::Row(row)),
                        "{name}, row {row} ({:?}) at {w} × {h}: {:#?}",
                        s.field(),
                        rows(&drawn)
                    );
                }
            }
        }
    }
}

#[test]
fn a_value_prepared_says_enter() {
    let mut s = sheet();
    let env = Fake::restarting(&["coordinateur"]);
    s.key(Key::Enter, &env);
    s.key(Key::Down, &env);
    s.key(Key::Down, &env);
    s.key(Key::Right, &env);
    let text = rows(&draw(&s, &look(), 100, 28));
    assert!(text[27].starts_with(&format!(" ⏎ {}", t!("valider", "apply"))), "{}", text[27]);
}

#[test]
fn a_long_message_on_three_rows() {
    let mut s = sheet();
    s.said = Some(Said::Failed("mot ".repeat(80)));
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    assert!(text[22].starts_with(" ✗ mot") && text[23].starts_with("   mot"), "{}", text[22]);
    assert!(text[24].ends_with('…'), "the last row cut: {}", text[24]);
    assert!(text[21].starts_with('─'), "the line over it");
    assert!(text[25].contains(&t!(" t Tableau", " t Dashboard")), "then the team's settings");
    assert!(text[26].contains(&t!(" n Nouvel agent ", " n New agent ")), "then the buttons");
}

#[test]
fn files_that_do_not_read() {
    let mut t = team(&[("coordinateur", true), ("dev", false)]);
    t.unreadable = Some("settings.toml".into());
    for p in &mut t.people {
        p.layers = None;
    }
    let s = Sheet::new(t);
    let text = rows(&draw(&s, &look(), 100, 28));
    assert!(text.iter().any(|r| r.contains(&t!("lecture seule", "read only"))));
    assert!(!text.iter().any(|r| r.contains(&t!("Nouvel agent", "New agent"))), "nothing to add");
    let buttons = text.iter().find(|r| r.contains(&t!("Quitter", "Quit"))).unwrap();
    assert!(buttons.contains(&t!("Détacher", "Detach")) && !buttons.contains(&t!("Tableau", "Dashboard")));
}

#[test]
fn typing_shows_what_is_wrong() {
    let mut s = sheet();
    let env = Fake::quiet();
    s.key(Key::Down, &env);
    s.key(Key::Enter, &env);
    while s.field() != Some(Fid::Name) {
        s.key(Key::Down, &env);
    }
    s.key(Key::Enter, &env);
    s.key(Key::EraseAll, &env);
    s.key(Key::Char('O'), &env);
    s.key(Key::Char('p'), &env);
    s.key(Key::Char('s'), &env);
    let drawn = draw(&s, &look(), 100, 28);
    let text = rows(&drawn);
    let at = text.iter().position(|r| r.contains("Ops")).expect("the text");
    assert!(text[at + 1].contains("✗"), "under it: {}", text[at + 1]);
    let x = text[at].find("Ops").map(|b| text[at][..b].chars().count()).unwrap() + 3;
    assert!(drawn.canvas.style(x, at).reverse, "the cursor after the text");
}

#[test]
fn a_long_list_scrolls_to_the_chosen() {
    let names: Vec<(String, bool)> = (0..12).map(|i| (format!("agent-{i}"), i == 0)).collect();
    let names: Vec<(&str, bool)> = names.iter().map(|(n, c)| (n.as_str(), *c)).collect();
    let mut s = Sheet::new(team(&names));
    let env = Fake::quiet();
    for _ in 0..11 {
        s.key(Key::Down, &env);
    }
    let drawn = draw(&s, &look(), 100, 24);
    let text = rows(&drawn);
    assert!(text.iter().any(|r| r.contains("agent-11")), "the chosen one in sight");
    assert!(text[2].contains('▲'), "more above");
    assert!(!text.iter().any(|r| r.contains("agent-1 ")), "the first ones scrolled away");
}
