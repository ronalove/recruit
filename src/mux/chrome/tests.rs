// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use crossterm::style::Color;

use super::*;
use crate::t;
use crate::ui::Rect;
use crate::ui::canvas::{Canvas, Style, columns};
use crate::ui::look::{ALT, Glyphs, Pressure, State};

const UNICODE: Look = Look { glyphs: Glyphs::Unicode, frame: 2 };

fn rows(canvas: &Canvas) -> Vec<String> {
    (0..canvas.height()).map(|y| canvas.row(y)).collect()
}

fn member<'a>(name: &'a str, state: State) -> Header<'a> {
    Header { name, color: Some(Color::Magenta), state, ..Header::default() }
}

/// A pane `width` columns wide and three rows high, framed.
fn framed(header: &Header<'_>, width: usize, focused: bool) -> Vec<String> {
    let mut canvas = Canvas::new(width, 3);
    frame(&mut canvas, Rect { x: 0, y: 0, width, height: 3 }, header, focused, UNICODE);
    rows(&canvas)
}

/// A choice's rows as drawn: `inner` columns between the borders.
fn choice_rows_of(choice: &Choice, inner: usize) -> Vec<String> {
    let row = |text: String| format!("│{text}{}│", " ".repeat(inner - columns(&text)));
    let title = format!("╭─ {} ", choice.title);
    let mut rows = vec![format!("{title}{}╮", "─".repeat(inner + 1 - columns(&title)))];
    if let Some(note) = &choice.note {
        rows.push(row(format!(" {note}")));
    }
    let last = choice.options.len() - 1;
    for (i, (key, label)) in choice.options.iter().enumerate() {
        if i == last {
            rows.push(format!("├{}┤", "─".repeat(inner)));
        }
        let gap = inner - 1 - columns(label) - 2;
        rows.push(row(format!(" {label}{}{key} ", " ".repeat(gap))));
    }
    rows.push(format!("╰{}╯", "─".repeat(inner)));
    rows
}

#[test]
fn the_quit_choice_in_the_team_s_words() {
    let choice = quit_choice();
    let cancel = if t!("a", "c") == "a" { 'a' } else { 'c' };
    assert_eq!(
        choice.options,
        [('d', t!("Détacher", "Detach")), ('q', t!("Quitter", "Quit")), (cancel, t!("Annuler", "Cancel"))]
    );
    assert_eq!(choice.selected, 2, "opens on the cancel");
    assert_eq!((choice.title.as_str(), choice.note), ("recruit", None));
    let choice = compact_choice("dev-cli", Some(88));
    let compact = if t!("c", "o") == "c" { 'c' } else { 'o' };
    assert_eq!(choice.options, [(compact, t!("Compacter", "Compact")), (cancel, t!("Annuler", "Cancel"))]);
    assert_eq!(choice.title, t!("Compacter {} ?", "Compact {}?", "dev-cli"));
    let note = t!(
        "Résume sa conversation pour libérer du contexte ({} %).",
        "Summarizes its conversation to free context ({}%).",
        88
    );
    assert_eq!(choice.note, Some(note));
    assert_eq!(choice.selected, 1, "opens on the cancel");
    let unknown = compact_choice("dev-cli", None).note.unwrap();
    let bare = t!("Résume sa conversation pour libérer du contexte.", "Summarizes its conversation to free context.");
    assert_eq!(unknown, bare);
}

#[test]
fn a_choice_at_the_center_with_its_rule_and_keys() {
    let choice = quit_choice();
    let screen = Rect { x: 0, y: 0, width: 60, height: 20 };
    let area = choice_area(screen, &choice);
    let inner = choice_inner(&choice);
    assert_eq!((area.width, area.height), (inner + 2, 6));
    assert_eq!((area.x, area.y), ((60 - area.width) / 2, (20 - 6) / 2));
    let mut canvas = Canvas::new(60, 20);
    canvas.fill(0, 0, 60, 20, Style::PLAIN.dim());
    canvas.put(0, area.y + 2, &"x".repeat(60), Style::PLAIN.dim());
    let zones = super::choice(&mut canvas, area, &choice);
    let drawn: Vec<String> =
        (area.y..area.y + area.height).map(|y| canvas.row(y).chars().skip(area.x).take(area.width).collect()).collect();
    assert_eq!(drawn, choice_rows_of(&choice, inner));
    // Each option where it shows, the rule skipped.
    let rows: Vec<(usize, usize)> = zones.iter().map(|(rect, i)| (rect.y - area.y, *i)).collect();
    assert_eq!(rows, [(1, 0), (2, 1), (4, 2)]);
    assert!(zones.iter().all(|(rect, _)| rect.x == area.x + 1 && rect.width == inner));
    // The selected one in reverse video, the others plain with their key in cyan; the frame grey.
    let selected = Style::PLAIN.reverse().bold();
    assert_eq!(canvas.style(area.x + 2, area.y + 4), selected);
    assert_eq!(canvas.style(area.x + inner - 1, area.y + 4), selected, "its key reversed too, not cyan");
    assert_eq!(canvas.style(area.x + 2, area.y + 2), Style::PLAIN);
    assert_eq!(canvas.style(area.x + inner - 1, area.y + 2), Style::fg(Color::DarkCyan).bold());
    assert_eq!(canvas.style(area.x, area.y + 1), Style::fg(Color::DarkGrey));
    // The compaction's note, dimmed.
    let compact = compact_choice("dev", Some(40));
    let area = choice_area(screen, &compact);
    super::choice(&mut canvas, area, &compact);
    assert!(canvas.style(area.x + 2, area.y + 1).dim, "the note dimmed");
    // What was under it is gone, what is around it stays.
    assert_eq!(canvas.row(area.y + 2).chars().take(area.x).collect::<String>(), "x".repeat(area.x));
}

#[test]
fn a_choice_no_larger_than_the_screen() {
    let choice = compact_choice("un-nom-de-membre-vraiment-très-long", Some(88));
    let screen = Rect { x: 0, y: 0, width: 24, height: 4 };
    let area = choice_area(screen, &choice);
    assert_eq!((area.x, area.width, area.height), (0, 24, 4));
    let mut canvas = Canvas::new(24, 4);
    let zones = super::choice(&mut canvas, area, &choice);
    // Two rows inside: the note goes, then the rule; both options stay, the cancel selected.
    assert_eq!(zones.iter().map(|(rect, i)| (rect.y, *i)).collect::<Vec<_>>(), [(1, 0), (2, 1)]);
    assert!(canvas.style(2, 2).reverse, "the cancel, selected");
    assert_eq!(canvas.row(3), format!("╰{}╯", "─".repeat(22)));
}

/// Reviewer: on a low screen, the choice never loses the cancel it opens on.
#[test]
fn a_low_screen_keeps_the_cancel() {
    let choice = quit_choice();
    let rows = |height: usize| {
        let screen = Rect { x: 0, y: 0, width: 30, height };
        let area = choice_area(screen, &choice);
        let mut canvas = Canvas::new(30, height);
        super::choice(&mut canvas, area, &choice).into_iter().map(|(_, i)| i).collect::<Vec<_>>()
    };
    assert_eq!(rows(6), [0, 1, 2], "all, with the rule");
    assert_eq!(rows(5), [0, 1, 2], "the rule gone");
    assert_eq!(rows(4), [0, 2], "the cancel kept, then the first");
    assert_eq!(rows(3), [2], "the cancel alone");
    // The selected one kept as well, when it is not the cancel.
    let quit = Choice { selected: 1, ..quit_choice() };
    assert_eq!(fitted(&quit, 2).0, [1, 2]);
    assert_eq!(fitted(&quit, 1).0, [2]);
}

#[test]
fn the_frame_takes_a_cell_all_around() {
    assert_eq!(inside(Rect { x: 0, y: 0, width: 10, height: 5 }), Rect { x: 1, y: 1, width: 8, height: 3 });
    assert_eq!(inside(Rect { x: 0, y: 0, width: 1, height: 1 }).width, 0);
}

#[test]
fn thick_for_the_focus_thin_otherwise() {
    let header = member("dev-rendu", State::Working);
    assert_eq!(
        framed(&header, 24, true),
        ["┏━ ⠹ dev-rendu ━━━━━━━━┓", "┃                      ┃", "┗━━━━━━━━━━━━━━━━━━━━━━┛"]
    );
    assert_eq!(
        framed(&header, 24, false),
        ["╭─ ⠹ dev-rendu ────────╮", "│                      │", "╰──────────────────────╯"]
    );
}

#[test]
fn inside_left_as_it_is() {
    let mut canvas = Canvas::new(12, 3);
    canvas.put(1, 1, "claude", Style::PLAIN);
    frame(&mut canvas, Rect { x: 0, y: 0, width: 12, height: 3 }, &member("a", State::Idle), false, UNICODE);
    assert_eq!(canvas.row(1), "│claude    │");
}

#[test]
fn colors_of_the_state() {
    let mut canvas = Canvas::new(30, 3);
    let area = Rect { x: 0, y: 0, width: 30, height: 3 };
    // Without the focus: grey at work and at rest, red waiting.
    for (state, color) in
        [(State::Working, Color::DarkGrey), (State::Waiting, Color::Red), (State::Idle, Color::DarkGrey)]
    {
        frame(&mut canvas, area, &member("dev-saisie", state), false, UNICODE);
        assert_eq!(canvas.style(0, 1), Style::fg(color), "{state:?}");
        // The sign in the state's color, the name in the member's: dimmed at rest, bold otherwise.
        assert_eq!(canvas.style(3, 0), sign_style(state));
        let name = canvas.style(5, 0);
        assert_eq!(name.fg, Some(Color::Magenta), "{state:?}");
        assert_eq!((name.bold, name.dim), (state != State::Idle, state == State::Idle));
    }
}

/// The active pane: thick, bold, in the terminal's text color at rest and at work, red waiting; its header keeps
/// the state's styles.
#[test]
fn the_focus_in_the_terminal_s_text_color() {
    let mut canvas = Canvas::new(30, 3);
    let area = Rect { x: 0, y: 0, width: 30, height: 3 };
    for (state, style) in [
        (State::Idle, Style::PLAIN.bold()),
        (State::Working, Style::PLAIN.bold()),
        (State::Waiting, Style::fg(Color::Red).bold()),
    ] {
        frame(&mut canvas, area, &member("dev-saisie", state), true, UNICODE);
        assert_eq!(canvas.style(0, 1), style, "{state:?}");
        assert_eq!(canvas.row(1).chars().next(), Some('┃'));
        assert_eq!(canvas.style(3, 0), sign_style(state), "{state:?}");
    }
    // At rest, its name dimmed; at work and waiting, in its color and bold; waiting, ⚑ red.
    frame(&mut canvas, area, &member("dev-saisie", State::Idle), true, UNICODE);
    assert_eq!(canvas.style(5, 0), Style::fg(Color::Magenta).dim());
    frame(&mut canvas, area, &member("dev-saisie", State::Working), true, UNICODE);
    assert_eq!(canvas.style(5, 0), Style::fg(Color::Magenta).bold());
    frame(&mut canvas, area, &member("dev-saisie", State::Waiting), true, UNICODE);
    assert_eq!(canvas.style(3, 0), Style::fg(Color::Red).bold());
    assert_eq!(canvas.style(5, 0), Style::fg(Color::Magenta).bold());
    // A panel with the focus stays grey, thick.
    let panel = Header { name: "Journal", ..Header::default() };
    frame(&mut canvas, area, &panel, true, UNICODE);
    assert_eq!(canvas.style(0, 1), Style::fg(Color::DarkGrey).bold());
}

/// A pane gone wrong: red with the focus or without.
#[test]
fn an_error_red_even_with_the_focus() {
    let mut canvas = Canvas::new(30, 3);
    let area = Rect { x: 0, y: 0, width: 30, height: 3 };
    let header = Header { note: Some("terminé"), ..member("dev-saisie", State::Idle) };
    frame(&mut canvas, area, &header, false, UNICODE);
    assert_eq!(canvas.style(0, 1), Style::fg(Color::Red));
    frame(&mut canvas, area, &header, true, UNICODE);
    assert_eq!(canvas.style(0, 1), Style::fg(Color::Red).bold());
    assert_eq!(canvas.row(1).chars().next(), Some('┃'));
}

/// A member whose turn failed: an orange frame, thin without the focus and thick with it; ✗ and the
/// name bold, then the reason in orange after the whole name: whole, else « erreur d'API », else nothing, the
/// right part giving its room first. Its form holds no one back. A pane gone wrong keeps its red note instead.
#[test]
fn a_failed_turn_in_orange() {
    let orange = crate::ui::look::palette().orange;
    let label = crate::ui::look::error_label(Some("API Error: 529 overloaded"));
    let header = Header { error: Some(&label), ..rich("dev-saisie", State::Error) };
    let mut canvas = Canvas::new(100, 3);
    let area = |width| Rect { x: 0, y: 0, width, height: 3 };
    frame(&mut canvas, area(100), &header, false, UNICODE);
    assert_eq!((canvas.style(0, 1), canvas.row(1).chars().next()), (Style::fg(orange), Some('│')));
    frame(&mut canvas, area(100), &header, true, UNICODE);
    assert_eq!((canvas.style(0, 1), canvas.row(1).chars().next()), (Style::fg(orange).bold(), Some('┃')));
    let row = canvas.row(0);
    let at = |text: &str| row[..row.find(text).unwrap()].chars().count();
    assert!(row.starts_with(&format!("┏━ ✗ dev-saisie · {label} ")), "{row}");
    assert_eq!(canvas.style(at("✗"), 0), Style::fg(orange).bold());
    assert_eq!(canvas.style(at(&label), 0), Style::fg(orange));
    assert!(row.contains(&format!("Opus {}", high())), "{row}");
    // Narrower: the right part goes before the reason, then the short reason, then none; the name stays whole.
    let short = t!("erreur d'API", "API error");
    let widths: Vec<(usize, String)> = (20..100)
        .map(|width| {
            let mut canvas = Canvas::new(width, 3);
            frame(&mut canvas, area(width), &header, false, UNICODE);
            (width, canvas.row(0))
        })
        .collect();
    let whole = widths.iter().find(|(_, row)| row.contains(&label)).map(|(w, _)| *w).unwrap();
    let bare_reason = t!("API surchargée", "API overloaded");
    let coded = widths.iter().find(|(_, row)| row.contains(&format!("· {bare_reason} "))).map(|(w, _)| *w).unwrap();
    let reduced = widths.iter().find(|(_, row)| row.contains(&format!("· {short} "))).map(|(w, _)| *w).unwrap();
    assert!(reduced < coded && coded < whole, "{reduced} {coded} {whole}");
    let (_, at_whole) = &widths[whole - 20];
    assert!(!at_whole.contains("Opus") && !at_whole.contains('●'), "the right part first: {at_whole}");
    let (_, bare) = &widths[0];
    assert!(bare.contains("✗ dev-saisie") && !bare.contains('·'), "{bare}");
    // Its form holds no one back; a pane gone wrong says its note, not the reason.
    assert_eq!(form(&header, 40), FORMS);
    let ended = Header { note: Some("terminé"), ..header };
    frame(&mut canvas, area(100), &ended, false, UNICODE);
    assert!(!canvas.row(0).contains(&label) && canvas.style(0, 1) == Style::fg(Color::Red));
}

/// A failed turn's notice: ✗ and its frame in orange; one member by name, several counted, the reason
/// without its code.
#[test]
fn a_failed_turn_s_notice() {
    let label = crate::ui::look::error_label(Some("API Error: 529 overloaded"));
    let reason = t!("API surchargée", "API overloaded");
    assert_eq!(
        error_notice_text(&["dev-saisie"], &label),
        t!("dev-saisie a raté son tour : {}", "dev-saisie's turn failed: {}", reason)
    );
    assert_eq!(
        error_notice_text(&["dev-saisie", "testeur", "reviewer"], &label),
        t!("3 membres ont raté leur tour : {}", "3 members' turns failed: {}", reason)
    );
    assert_eq!(crate::ui::look::without_code("erreur d'API"), "erreur d'API");
    assert_eq!(crate::ui::look::without_code("a (b)"), "a (b)");
    let notice = Notice { state: State::Error, text: error_notice_text(&["dev-saisie"], &label), ..saisie() };
    let screen = Rect { x: 0, y: 0, width: 140, height: 39 };
    let area = notice_area(screen, &notice);
    let mut canvas = Canvas::new(140, 40);
    super::notice(&mut canvas, area, &notice, UNICODE);
    let row = canvas.row(2);
    let x = row[..row.find('✗').unwrap()].chars().count();
    assert_eq!(canvas.style(x, 2), Style::fg(crate::ui::look::palette().orange).bold());
    assert_eq!(canvas.style(area.x, 2), Style::fg(crate::ui::look::palette().orange));
    assert_eq!(canvas.style(row[..row.find("dev-saisie").unwrap()].chars().count(), 2), Style::fg(Color::Blue).bold());
}

/// No name, no header in the border: the menu's layer, whose first line says « recruit ».
#[test]
fn no_name_no_header() {
    let header = Header::default();
    assert_eq!(framed(&header, 20, true), ["┏━━━━━━━━━━━━━━━━━━┓", "┃                  ┃", "┗━━━━━━━━━━━━━━━━━━┛"]);
    assert_eq!(framed(&header, 20, false)[0], "╭──────────────────╮");
    let mut canvas = Canvas::new(20, 3);
    assert!(frame(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 3 }, &header, true, UNICODE).is_empty());
}

#[test]
fn a_panel_grey_and_plain() {
    let header = Header { name: "Journal", state: State::Working, ..Header::default() };
    assert_eq!(framed(&header, 20, false)[0], "╭─ Journal ────────╮");
    let mut canvas = Canvas::new(20, 3);
    frame(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 3 }, &header, false, UNICODE);
    assert_eq!(canvas.style(0, 0), Style::fg(Color::DarkGrey));
    assert_eq!(canvas.style(3, 0), Style::PLAIN);
}

/// The journal's key at the right of its border, while the title leaves room.
#[test]
fn the_journal_s_key_in_its_border() {
    let header = Header { name: "Journal", hint: Some(Hint::JournalSize), ..Header::default() };
    let hint = t!("{}j taille", "{}j size", ALT);
    let top = |width: usize| framed(&header, width, false).remove(0);
    let head = "╭─ Journal ";
    let dashes = 40 - columns(head) - columns(&hint) - 2 - 2;
    assert_eq!(top(40), format!("{head}{} {hint} ─╮", "─".repeat(dashes)));
    // Dimmed: its last letter, before the blank, the dash and the corner.
    let mut canvas = Canvas::new(40, 3);
    frame(&mut canvas, Rect { x: 0, y: 0, width: 40, height: 3 }, &header, false, UNICODE);
    assert_eq!(canvas.row(0).chars().nth(40 - 4), hint.chars().last());
    assert_eq!(canvas.style(40 - 4, 0), Style::PLAIN.dim());
    // At the limit, a single dash between the title and it; a column less, the title alone.
    let limit = columns(head) + 1 + columns(&hint) + 2 + 2;
    assert_eq!(top(limit), format!("{head}─ {hint} ─╮"));
    assert_eq!(top(limit - 1), format!("{head}{}╮", "─".repeat(limit - 1 - columns(head) - 1)));
}

/// The dashboard's counts in its border, each in its state's color; none for a state no one is in.
#[test]
fn the_dashboard_s_counts_in_its_border() {
    let title = t!("Tableau de bord", "Dashboard");
    let header = Header {
        name: &title,
        hint: Some(Hint::Counts { working: 5, waiting: 1, error: 1, idle: 3 }),
        ..Header::default()
    };
    let mut canvas = Canvas::new(60, 3);
    frame(&mut canvas, Rect { x: 0, y: 0, width: 60, height: 3 }, &header, false, UNICODE);
    let head = format!("╭─ {title} ");
    let counts = " ⠹ 5  ⚑ 1  ✗ 1  ◷ 3 ─╮";
    assert_eq!(canvas.row(0), format!("{head}{}{counts}", "─".repeat(60 - columns(&head) - columns(counts))));
    let at = |sign: char| canvas.row(0).chars().position(|c| c == sign).unwrap();
    assert_eq!(canvas.style(at('⠹'), 0), Style::fg(Color::Yellow));
    assert_eq!(canvas.style(at('⚑') + 2, 0), Style::fg(Color::Red));
    assert_eq!(canvas.style(at('✗'), 0), Style::fg(crate::ui::look::palette().orange));
    assert_eq!(canvas.style(at('◷'), 0), Style::fg(Color::DarkGrey));
    // No one waiting: no ⚑; no one at all: nothing.
    let header = Header { hint: Some(Hint::Counts { working: 2, waiting: 0, error: 0, idle: 0 }), ..header };
    assert!(framed(&header, 60, false)[0].ends_with("─ ⠹ 2 ─╮"));
    let header = Header { hint: Some(Hint::Counts { working: 0, waiting: 0, error: 0, idle: 0 }), ..header };
    assert_eq!(framed(&header, 30, false)[0], format!("{head}{}╮", "─".repeat(30 - columns(&head) - 1)));
    // Nerd Font: the robot at work, as on the dashboard.
    let header = Header { hint: Some(Hint::Counts { working: 2, waiting: 0, error: 0, idle: 0 }), ..header };
    frame(
        &mut canvas,
        Rect { x: 0, y: 0, width: 60, height: 3 },
        &header,
        false,
        Look { glyphs: Glyphs::Nerd, frame: 0 },
    );
    assert!(canvas.row(0).ends_with("\u{F06A9} 2 ─╮"), "{}", canvas.row(0));
}

/// The header in panes of 72, 52, 40, 34, 26 and 20 columns, then narrower.
#[test]
fn the_header_as_wide_as_the_pane() {
    let header = Header { scroll: Some((214, 3000)), ..member("dev-interface", State::Waiting) };
    let top = |width| framed(&header, width, false).remove(0);
    let up = t!("↑ {} sur {}", "↑ {} of {}", 214, 3000);
    let line = |head: &str, width: usize| format!("{head} {}╮", "─".repeat(width - columns(head) - 2));
    for width in [72, 52, 41] {
        assert_eq!(top(width), line(&format!("╭─ ⚑ dev-interface · {up}"), width));
    }
    // The scroll short, then gone.
    assert_eq!(top(34), "╭─ ⚑ dev-interface · ↑ 214 ──────╮");
    assert_eq!(top(26), "╭─ ⚑ dev-interface ──────╮");
    assert_eq!(top(20), "╭─ ⚑ dev-inte… ────╮");
    // Narrower than the zoom's corner allows: the header up to a dash before the frame's corner.
    assert_eq!(top(12), "╭─ ⚑ dev… ─╮");
    assert_eq!(top(11), "╭─ ⚑ de… ─╮");
    // Too narrow for three columns of the name: the border alone.
    assert_eq!(top(10), "╭────────╮");
    assert_eq!(top(6), "╭────╮");
    assert_eq!(top(2), "╭╮");
}

/// An alert in red, the frame red too: whole, else short, the name cut for it, then the alert itself.
#[test]
fn an_alert_before_the_rest() {
    let header = Header {
        note: Some("moteur en échec : panique"),
        scroll: Some((3, 9)),
        ..member("dev-saisie", State::Working)
    };
    let top = |width| framed(&header, width, false).remove(0);
    let up = t!("↑ {} sur {}", "↑ {} of {}", 3, 9);
    assert!(top(72).starts_with(&format!("╭─ ✗ dev-saisie · moteur en échec : panique · {up} ─")), "{}", top(72));
    // The scroll goes before the alert is shortened, and stays gone while it is.
    assert_eq!(top(49), "╭─ ✗ dev-saisie · moteur en échec : panique ────╮");
    assert_eq!(top(45), "╭─ ✗ dev-saisie · moteur en échec ──────────╮");
    // Shorter still, the name whole; then the name cut, then the alert itself.
    assert_eq!(top(35), "╭─ ✗ dev-saisie · en échec ───────╮");
    assert_eq!(top(30), "╭─ ✗ dev-saisie · en échec ──╮");
    assert_eq!(top(24), "╭─ ✗ dev-… · en échec ─╮");
    assert_eq!(top(20), "╭─ ✗ de… · en éc… ─╮");
    let mut canvas = Canvas::new(45, 3);
    frame(&mut canvas, Rect { x: 0, y: 0, width: 45, height: 3 }, &header, false, UNICODE);
    assert_eq!(canvas.style(0, 1), Style::fg(Color::Red));
    // The cross in red, and the alert.
    assert_eq!(canvas.style(3, 0), Style::fg(Color::Red));
    assert_eq!(canvas.style(18, 0), Style::fg(Color::Red));
    let nerd = Look { glyphs: Glyphs::Nerd, frame: 0 };
    frame(&mut canvas, Rect { x: 0, y: 0, width: 45, height: 3 }, &header, false, nerd);
    assert!(canvas.row(0).starts_with("╭─ \u{F0159} dev-saisie"), "{}", canvas.row(0));
    // In English, the same forms: « engine failed », then « failed ».
    let english = Header { note: Some("engine failed: panic"), ..header };
    assert_eq!(framed(&english, 40, false)[0], "╭─ ✗ dev-saisie · engine failed ───────╮");
    assert_eq!(framed(&english, 30, false)[0], "╭─ ✗ dev-saisie · failed ────╮");
    // A one-word alert has no shorter form.
    let header = Header { note: Some("terminé"), scroll: None, ..member("dev-saisie", State::Idle) };
    assert_eq!(framed(&header, 31, false)[0], "╭─ ✗ dev-saisie · terminé ────╮");
    assert_eq!(framed(&header, 30, false)[0], "╭─ ✗ dev-saisie · terminé ───╮");
    assert_eq!(framed(&header, 24, false)[0], "╭─ ✗ dev-s… · terminé ─╮");
}

/// A member at work with all its header: Opus, high, 29 %, six minutes, the zoom.
fn rich<'a>(name: &'a str, state: State) -> Header<'a> {
    Header {
        model: Some("Opus"),
        effort: Some("high"),
        context: Some(29),
        since: Some(360),
        zoom: Some(Zoom::In),
        ..member(name, state)
    }
}

/// A header's top border: `left`, dashes, then « meta » if any before the zoom's corner.
fn bordered(left: &str, meta: &str, width: usize) -> String {
    let right = if meta.is_empty() { " ⤢ ─╮".to_string() } else { format!(" {meta} ─ ⤢ ─╮") };
    format!("{left}{}{right}", "─".repeat(width - columns(left) - columns(&right)))
}

/// What goes when the pane narrows, in turn: the effort's name, the model's name
/// for its initial, the time, the initial, the effort's sign, the context; then the name is cut.
#[test]
fn the_full_header_as_wide_as_the_pane() {
    let header = rich("dev-rendu", State::Working);
    let top = |width| framed(&header, width, false).remove(0);
    let left = "╭─ ⠹ dev-rendu ";
    // French names the effort with one more column.
    let extra = columns(&high()) - columns("● high");
    let full = format!("Opus {} · 29 % · 6m", high());
    for (widths, meta) in [
        ([72, 47 + extra], full.as_str()),
        ([46 + extra, 42], "Opus ● · 29 % · 6m"),
        ([41, 39], "O ● · 29 % · 6m"),
        ([38, 34], "O ● · 29 %"),
        ([33, 32], "● · 29 %"),
        ([31, 28], "29 %"),
        ([27, 20], ""),
    ] {
        for width in widths {
            assert_eq!(top(width), bordered(left, meta, width), "{width}");
        }
    }
    // Beside the zoom, a single blank.
    assert_eq!(top(19), "╭─ ⠹ dev-rendu ⤢ ─╮");
    assert_eq!(top(18), "╭─ ⠹ dev-ren… ⤢ ─╮");
    // Narrower than the zoom's corner allows: no zoom, the name cut as before.
    assert_eq!(top(15), "╭─ ⠹ dev-re… ─╮");
    // Thick with the focus, the same header.
    assert_eq!(
        framed(&header, 72, true)[0],
        bordered(left, &format!("Opus {} · 29 % · 6m", high()), 72)
            .replace('─', "━")
            .replace('╭', "┏")
            .replace('╮', "┓")
    );
}

/// At rest with a command running: the terminal's sign, dimmed as at rest, its time that of the command; work,
/// when it comes back, prevails.
#[test]
fn a_command_running() {
    let header = Header { shell: true, ..rich("dev", State::Idle) };
    let mut canvas = Canvas::new(40, 3);
    let area = Rect { x: 0, y: 0, width: 40, height: 3 };
    frame(&mut canvas, area, &header, false, UNICODE);
    assert_eq!(canvas.cell(3, 0), Glyphs::Unicode.console().to_string());
    assert_eq!(canvas.style(3, 0), sign_style(State::Idle));
    assert_eq!(canvas.style(5, 0), Style::fg(Color::Magenta).dim(), "the name at rest");
    assert!(canvas.row(0).contains("6m"), "{}", canvas.row(0));
    frame(&mut canvas, area, &header, false, Look { glyphs: Glyphs::Nerd, frame: 0 });
    assert_eq!(canvas.cell(3, 0), Glyphs::Nerd.console().to_string());
    let working = Header { state: State::Working, ..header };
    frame(&mut canvas, area, &working, false, UNICODE);
    assert_eq!(canvas.cell(3, 0), "⠹");
    // Gone wrong, the cross all the same.
    let failed = Header { note: Some("terminé"), ..header };
    frame(&mut canvas, area, &failed, false, UNICODE);
    assert_eq!(canvas.cell(3, 0), "✗");
}

/// Missing pieces leave no separator behind.
#[test]
fn only_what_is_known() {
    let left = "╭─ ◷ dev ";
    let header = Header { model: None, since: None, ..rich("dev", State::Idle) };
    // The model not known yet, an effort to say beside it: « ? ».
    assert_eq!(framed(&header, 40, false)[0], bordered(left, &format!("? {} · 29 %", high()), 40));
    let header = Header { effort: None, context: None, ..rich("dev", State::Idle) };
    assert_eq!(framed(&header, 40, false)[0], bordered(left, "Opus · 6m", 40));
    let header = Header { context: None, since: None, ..rich("dev", State::Idle) };
    assert_eq!(framed(&header, 40, false)[0], bordered(left, &format!("Opus {}", high()), 40));
    // Without the zoom (a pane that is not a member's), dashes in the corner.
    let header = Header { zoom: None, ..member("dev", State::Idle) };
    assert_eq!(framed(&header, 20, false)[0], "╭─ ◷ dev ──────────╮");
    // Zoomed: ⤡.
    let header = Header { zoom: Some(Zoom::Out), ..member("dev", State::Idle) };
    assert_eq!(framed(&header, 20, false)[0], "╭─ ◷ dev ────── ⤡ ─╮");
    let nerd = Look { glyphs: Glyphs::Nerd, frame: 0 };
    let mut canvas = Canvas::new(20, 3);
    frame(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 3 }, &header, false, nerd);
    assert_eq!(canvas.cell(16, 0), "\u{F0294}");
}

#[test]
fn the_header_s_colors() {
    let mut canvas = Canvas::new(72, 3);
    let area = Rect { x: 0, y: 0, width: 72, height: 3 };
    let at = |canvas: &Canvas, text: &str| {
        let row = canvas.row(0);
        let byte = row.find(text).unwrap_or_else(|| panic!("{text:?} in {row}"));
        row[..byte].chars().count()
    };
    frame(&mut canvas, area, &rich("dev-rendu", State::Working), false, UNICODE);
    // The model dimmed, the effort in Claude Code's color, the separators grey, the context plain at work, the
    // time dimmed, the zoom grey.
    assert_eq!(canvas.style(at(&canvas, "Opus"), 0), Style::PLAIN.dim());
    assert_eq!(canvas.style(at(&canvas, "●"), 0), Style::fg(Color::AnsiValue(141)));
    assert_eq!(canvas.style(at(&canvas, &t!("élevé", "high")), 0), Style::fg(Color::AnsiValue(141)));
    assert_eq!(canvas.style(at(&canvas, "·"), 0), Style::fg(Color::DarkGrey));
    assert_eq!(canvas.style(at(&canvas, "29"), 0), Style::PLAIN);
    assert_eq!(canvas.style(at(&canvas, "6m"), 0), Style::PLAIN.dim());
    assert_eq!(canvas.style(at(&canvas, "⤢"), 0), Style::fg(Color::DarkGrey));
    // Near compaction orange, where Claude Code warns red; at rest, dimmed, ⟳ before it when it compacts.
    let near = Header { pressure: Pressure::Near, ..rich("dev-rendu", State::Working) };
    frame(&mut canvas, area, &near, false, UNICODE);
    assert_eq!(canvas.style(at(&canvas, "29"), 0), Style::fg(crate::ui::look::palette().orange));
    let warned = Header { pressure: Pressure::Warning, ..rich("dev-rendu", State::Working) };
    frame(&mut canvas, area, &warned, false, UNICODE);
    assert_eq!(canvas.style(at(&canvas, "29"), 0), Style::fg(Color::Red).bold());
    let resting = Header { compactable: true, ..rich("dev-rendu", State::Idle) };
    frame(&mut canvas, area, &resting, false, UNICODE);
    assert!(canvas.row(0).contains(&format!(" Opus {} · ⟳ 29 % · 6m ", high())), "{}", canvas.row(0));
    assert_eq!(canvas.style(at(&canvas, "⟳"), 0), Style::PLAIN.dim());
    assert_eq!(canvas.style(at(&canvas, "29"), 0), Style::PLAIN.dim());
    // Waiting, the time in red.
    frame(&mut canvas, area, &rich("dev-rendu", State::Waiting), false, UNICODE);
    assert_eq!(canvas.style(at(&canvas, "6m"), 0), Style::fg(Color::Red));
    // `max` in a rainbow that turns with the spinner.
    let max = Header { effort: Some("max"), ..rich("dev-rendu", State::Working) };
    frame(&mut canvas, area, &max, false, UNICODE);
    let sign = at(&canvas, "◈");
    assert_eq!(canvas.style(sign, 0), Style::fg(crate::ui::look::palette().rainbow[1]));
    // Its name letter by letter, from the second color.
    assert_eq!(canvas.style(sign + 2, 0), Style::fg(crate::ui::look::palette().rainbow[2]));
    assert_eq!(canvas.style(sign + 3, 0), Style::fg(crate::ui::look::palette().rainbow[3]));
    frame(&mut canvas, area, &max, false, Look { frame: 4, ..UNICODE });
    assert_eq!(canvas.style(sign, 0), Style::fg(crate::ui::look::palette().rainbow[2]));
    // At rest, still.
    let resting = Header { state: State::Idle, ..max };
    frame(&mut canvas, area, &resting, false, Look { frame: 4, ..UNICODE });
    assert_eq!(canvas.style(at(&canvas, "◈"), 0), Style::fg(crate::ui::look::palette().rainbow[0]));
}

/// Where each part can be clicked; the context only when it compacts.
#[test]
fn the_header_s_parts() {
    let mut canvas = Canvas::new(72, 3);
    let area = Rect { x: 0, y: 0, width: 72, height: 3 };
    let text = |canvas: &Canvas, rect: Rect| canvas.row(0).chars().skip(rect.x).take(rect.width).collect::<String>();
    let parts = frame(&mut canvas, area, &rich("dev-rendu", State::Working), false, UNICODE);
    let shown: Vec<(Part, String)> = parts.iter().map(|(rect, part)| (*part, text(&canvas, *rect))).collect();
    assert_eq!(
        shown,
        [
            (Part::Zoom, " ⤢ ".into()),
            (Part::Name, "dev-rendu".into()),
            (Part::Model, "Opus".into()),
            (Part::Effort, high()),
        ]
    );
    assert!(parts.iter().all(|(rect, _)| rect.y == 0 && rect.height == 1));
    let resting = Header { compactable: true, ..rich("dev-rendu", State::Idle) };
    let parts = frame(&mut canvas, area, &resting, false, UNICODE);
    let context = parts.iter().find(|(_, part)| *part == Part::Context).expect("the context");
    assert_eq!(text(&canvas, context.0), "⟳ 29 %");
    // A rainbow in pieces, one zone.
    let max = Header { effort: Some("max"), ..rich("dev-rendu", State::Working) };
    let parts = frame(&mut canvas, area, &max, false, UNICODE);
    let effort: Vec<_> = parts.iter().filter(|(_, part)| *part == Part::Effort).collect();
    assert_eq!(effort.len(), 1);
    assert_eq!(text(&canvas, effort[0].0), "◈ max");
    // A panel: no part.
    let panel = Header { name: "Journal", ..Header::default() };
    assert!(frame(&mut canvas, area, &panel, false, UNICODE).is_empty());
    // Too narrow for a header: nothing to click but the zoom.
    let parts = frame(&mut canvas, Rect { width: 16, ..area }, &rich("dev-rendu", State::Working), false, UNICODE);
    assert_eq!(parts.iter().map(|(_, p)| *p).collect::<Vec<_>>(), [Part::Zoom, Part::Name]);
}

/// The model's initial where its name does not fit, and `?` for a model not known yet, beside a known effort; a click
/// on the initial reaches the model like its name The effort's name in the user's words.
#[test]
fn the_model_as_its_initial() {
    let area = Rect { x: 0, y: 0, width: 40, height: 3 };
    let initial = |model: Option<&'static str>| {
        let mut canvas = Canvas::new(40, 3);
        let header = Header { model, ..rich("dev-rendu", State::Idle) };
        let parts = frame(&mut canvas, area, &header, false, UNICODE);
        let (rect, _) = parts.iter().find(|(_, part)| *part == Part::Model).expect("the model's zone");
        (canvas.row(0).chars().skip(rect.x).take(rect.width).collect::<String>(), canvas.style(rect.x, 0).dim)
    };
    for (model, expected) in [(Some("Opus"), "O"), (Some("Sonnet"), "S"), (Some("Haiku"), "H"), (Some("Fable"), "F")] {
        assert_eq!(initial(model), (expected.to_string(), true));
    }
    // A family Claude Code does not know: its first letter in capitals; none yet: `?`, dimmed too.
    assert_eq!(initial(Some("mistral")), ("M".to_string(), true));
    assert_eq!(initial(None), ("?".to_string(), true));
    // The levels in the user's words, extra for xhigh, with Claude Code's signs.
    let wide = Rect { width: 90, ..area };
    let mut canvas = Canvas::new(90, 3);
    for (level, text) in [
        ("low", t!("○ faible", "○ low")),
        ("medium", t!("◐ moyen", "◐ medium")),
        ("high", t!("● élevé", "● high")),
        ("xhigh", t!("◉ extra", "◉ extra")),
        ("max", "◈ max".to_string()),
    ] {
        let header = Header { effort: Some(level), ..rich("dev-rendu", State::Idle) };
        let parts = frame(&mut canvas, wide, &header, false, UNICODE);
        let (rect, _) = parts.iter().find(|(_, part)| *part == Part::Effort).unwrap();
        assert_eq!(canvas.row(0).chars().skip(rect.x).take(rect.width).collect::<String>(), text);
    }
}

/// The name hovered underlined, « réglages › » after it while the rest keeps some room; another
/// part underlined; the zoom in bold cyan.
/// The part whose list is open keeps its colors, on the list cursor's background.
#[test]
fn the_header_with_its_list_open() {
    let area = Rect { x: 0, y: 0, width: 80, height: 5 };
    for (part, text) in [(Part::Model, "Opus"), (Part::Effort, "high")] {
        let mut canvas = Canvas::new(80, 5);
        let open = Header { open: Some(part), effort: Some("high"), ..rich("dev-rendu", State::Working) };
        let parts = frame(&mut canvas, area, &open, false, UNICODE);
        let (rect, _) = parts.iter().find(|(_, p)| *p == part).expect("the part is there");
        assert!(
            (rect.x..rect.x + rect.width)
                .all(|x| canvas.style(x, 0).bg == Some(crate::ui::look::palette().selected)
                    && !canvas.style(x, 0).reverse),
            "{text}: on the cursor's background over its zone"
        );
        // The blank on each side is on it too; the model is not dimmed there.
        assert_eq!(
            canvas.style(rect.x - 1, 0).bg,
            Some(crate::ui::look::palette().selected),
            "{text}: the blank before"
        );
        assert_eq!(
            canvas.style(rect.x + rect.width, 0).bg,
            Some(crate::ui::look::palette().selected),
            "{text}: the blank after"
        );
        assert!(!canvas.style(rect.x, 0).dim, "{text}: in its normal color");
        let other = if part == Part::Model { Part::Effort } else { Part::Model };
        let (rect, _) = parts.iter().find(|(_, p)| *p == other).expect("the other part");
        assert!((rect.x..rect.x + rect.width).all(|x| canvas.style(x, 0).bg.is_none()), "the other one is not");
        // Not open, the model stays dimmed.
        if part == Part::Effort {
            assert!(canvas.style(rect.x, 0).dim);
        }
    }
}

/// In a light terminal the part whose list is open is on the light palette's selection (253).
#[test]
fn the_header_with_its_list_open_in_the_light_palette() {
    let area = Rect { x: 0, y: 0, width: 80, height: 3 };
    let mut canvas = Canvas::new(80, 3);
    crate::ui::look::set_light(true);
    let open = Header { open: Some(Part::Model), ..rich("dev-rendu", State::Idle) };
    let parts = frame(&mut canvas, area, &open, false, UNICODE);
    crate::ui::look::set_light(false);
    let (rect, _) = parts.iter().find(|(_, p)| *p == Part::Model).unwrap();
    assert_eq!(canvas.style(rect.x, 0).bg, Some(Color::AnsiValue(253)));
}

#[test]
fn the_header_hovered() {
    let mut canvas = Canvas::new(72, 3);
    let area = Rect { x: 0, y: 0, width: 72, height: 3 };
    let settings = t!("réglages ›", "settings ›");
    let hovered = Header { hover: Some(Part::Name), ..rich("dev-terminal", State::Working) };
    let parts = frame(&mut canvas, area, &hovered, false, UNICODE);
    let row = canvas.row(0);
    assert!(row.starts_with(&format!("╭─ ⠹ dev-terminal {settings} ─")), "{row}");
    assert!(canvas.style(5, 0).underline && !canvas.style(4, 0).underline, "the name underlined");
    assert_eq!(canvas.style(5 + 13, 0), Style::fg(Color::DarkCyan));
    // The name's zone takes « réglages › » too: the mouse stays on it.
    let name = parts.iter().find(|(_, part)| *part == Part::Name).unwrap().0;
    assert_eq!((name.x, name.width), (5, 12 + 1 + columns(&settings)));
    // The rest still there, shorter if need be.
    assert_eq!(row, bordered(&format!("╭─ ⠹ dev-terminal {settings} "), &format!("Opus {} · 29 % · 6m", high()), 72));
    // Narrow: no room for it, the name underlined alone.
    let parts = frame(&mut canvas, Rect { width: 40, ..area }, &hovered, false, UNICODE);
    assert!(!canvas.row(0).contains(&settings), "{}", canvas.row(0));
    assert_eq!(parts.iter().find(|(_, part)| *part == Part::Name).unwrap().0.width, 12);
    // The model, the effort.
    let hovered = Header { hover: Some(Part::Effort), ..rich("dev-rendu", State::Working) };
    frame(&mut canvas, area, &hovered, false, UNICODE);
    let at = canvas.row(0).chars().position(|c| c == '●').unwrap();
    assert!(canvas.style(at, 0).underline && canvas.style(at + 5, 0).underline);
    assert!(!canvas.style(at - 2, 0).underline, "not the model");
    // The context that compacts, underlined whole as its zone: « ⟳ 29 % ».
    let hovered = Header { hover: Some(Part::Context), compactable: true, ..rich("dev-rendu", State::Idle) };
    frame(&mut canvas, area, &hovered, false, UNICODE);
    let at = canvas.row(0).chars().position(|c| c == '⟳').unwrap();
    assert!((at..at + 6).all(|x| canvas.style(x, 0).underline), "{}", canvas.row(0));
    assert!(!canvas.style(at - 1, 0).underline && !canvas.style(at + 6, 0).underline);
    let hovered = Header { hover: Some(Part::Zoom), ..rich("dev-rendu", State::Working) };
    frame(&mut canvas, area, &hovered, false, UNICODE);
    assert_eq!(canvas.style(72 - 4, 0), Style::fg(Color::DarkCyan).bold());
}

/// The scroll keeps its place after the model, the effort's word and the time are gone, before the effort's sign
/// and the context go.
#[test]
fn the_scroll_among_the_rest() {
    let header = Header { scroll: Some((214, 3000)), ..rich("dev-rendu", State::Working) };
    let up = t!("↑ {} sur {}", "↑ {} of {}", 214, 3000);
    let left = format!("╭─ ⠹ dev-rendu · {up} ");
    let short = "╭─ ⠹ dev-rendu · ↑ 214 ";
    let top = |width| framed(&header, width, false).remove(0);
    let full = format!("Opus {} · 29 % · 6m", high());
    let wide = columns(&left) + columns(&full) + 2 + 7;
    assert_eq!(top(wide), bordered(&left, &full, wide));
    assert_eq!(top(wide - 1), bordered(&left, "Opus ● · 29 % · 6m", wide - 1));
    let at = columns(&left) + 10 + 7;
    assert_eq!(top(at), bordered(&left, "● · 29 %", at));
    assert_eq!(top(at - 1), bordered(short, "● · 29 %", at - 1));
    let at = columns(short) + 10 + 7;
    assert_eq!(top(at - 1), bordered("╭─ ⠹ dev-rendu ", "● · 29 %", at - 1));
    assert_eq!(top(30), bordered("╭─ ⠹ dev-rendu ", "29 %", 30));
}

/// The time shows while the header holds it: the screen draws again each second only then.
/// Panes as wide take the same form, the poorest of theirs, for their columns to line up.
#[test]
fn one_form_for_panes_as_wide() {
    // Panes as wide take the poorest form of theirs: pm as dev-interface.
    let short = rich("pm", State::Working);
    let long = rich("dev-interface", State::Working);
    let longest = rich("un-nom-de-membre-vraiment-très-long", State::Working);
    assert_eq!([form(&short, 45), form(&long, 45), form(&longest, 45)], [0, 2, FORMS]);
    // The very long name holds no one back.
    let common = common_form([&short, &long, &longest].map(|h| form(h, 45)));
    assert_eq!(common, 2);
    let top = |header: Header<'_>| framed(&Header { form: common, ..header }, 45, false).remove(0);
    assert_eq!(top(short), bordered("╭─ ⠹ pm ", "O ● · 29 % · 6m", 45));
    assert_eq!(top(long), bordered("╭─ ⠹ dev-interface ", "O ● · 29 % · 6m", 45));
    assert!(top(longest).starts_with("╭─ ⠹ un-nom-de-membre-vraiment-tr") && !top(longest).contains('%'));
    // None but a very long name, or none at all: no ceiling.
    assert_eq!(common_form([FORMS, FORMS]), 0);
    assert_eq!(common_form([]), 0);
    // Narrower than its form allows, a pane takes a poorer one all the same.
    let capped = Header { form: 1, ..long };
    assert_eq!(framed(&capped, 40, false)[0], bordered("╭─ ⠹ dev-interface ", "O ● · 29 %", 40));
    // Hovered or scrolled, the form is that of the pane at rest.
    let hovered = Header { hover: Some(Part::Name), scroll: Some((3, 9)), ..long };
    assert_eq!(form(&hovered, 45), 2);
    // Nothing at the right: whole, binding no other; too narrow for anything, none.
    assert_eq!(form(&Header { model: None, effort: None, context: None, since: None, ..long }, 45), 0);
    assert_eq!(form(&long, 20), FORMS);
}

#[test]
fn whether_the_time_shows() {
    let header = rich("dev-rendu", State::Working);
    let shown = |width| framed(&header, width, false)[0].contains("6m");
    for width in [72, 47, 46, 42, 41, 39, 38, 37, 34, 32, 20] {
        assert_eq!(shows_since(&header, width), shown(width), "{width}");
    }
    assert!(shows_since(&header, 39) && !shows_since(&header, 38));
    assert!(!shows_since(&Header { since: None, ..header }, 72));
    // The scroll goes after the time.
    let scrolled = Header { scroll: Some((3, 9)), ..header };
    assert_eq!(shows_since(&scrolled, 40), framed(&scrolled, 40, false)[0].contains("6m"));
}

/// A pane gone wrong keeps its zoom while the whole alert holds beside the whole name; it gives its corner first.
#[test]
fn the_zoom_beside_an_alert() {
    let header = Header { note: Some("moteur en échec : panique"), ..rich("dev", State::Idle) };
    let left = "╭─ ✗ dev · moteur en échec : panique ";
    let top = |width| framed(&header, width, false).remove(0);
    let at = columns(left) + 5;
    assert_eq!(top(60), bordered(left, "O ● · 29 %", 60));
    assert_eq!(top(at), bordered(left, "", at));
    // A column less: the zoom goes, the alert whole.
    assert_eq!(top(at - 1), format!("{left}───╮"));
    assert_eq!(top(at - 3), format!("{left}─╮"));
    // Then the alert shortens, as before.
    assert!(top(at - 4).starts_with("╭─ ✗ dev · moteur en échec ─"), "{}", top(at - 4));
    // No zoom without a member's pane.
    let header = Header { zoom: None, ..header };
    assert!(!framed(&header, 60, false)[0].contains('⤢'));
}

fn saisie() -> Notice {
    Notice {
        member: "dev-saisie".into(),
        state: State::Waiting,
        color: Some(Color::Blue),
        text: notice_text("dev-saisie"),
        tab: "Agents (1)".into(),
    }
}

/// At the top right, on the panes' first row, two columns from the edge, in a red frame.
#[test]
fn a_notice_at_the_top_right() {
    let notice = saisie();
    let text = notice_text("dev-saisie");
    let go = t!("y aller", "go");
    let inner = format!(" ⚑ {text} · Agents (1)   {ALT}g {go} ");
    let screen = Rect { x: 0, y: 0, width: 140, height: 39 };
    let area = notice_area(screen, &notice);
    let width = columns(&inner) + 2;
    assert_eq!(area, Rect { x: 140 - 2 - width, y: 1, width, height: 3 });
    let mut canvas = Canvas::new(140, 40);
    canvas.fill(0, 0, 140, 40, Style::PLAIN);
    canvas.put(0, 2, &"x".repeat(140), Style::PLAIN);
    super::notice(&mut canvas, area, &notice, UNICODE);
    let drawn = |y: usize| canvas.row(y).chars().skip(area.x).take(area.width).collect::<String>();
    assert_eq!(drawn(1), format!("╭{}╮", "─".repeat(width - 2)));
    assert_eq!(drawn(2), format!("│{inner}│"));
    assert_eq!(drawn(3), format!("╰{}╯", "─".repeat(width - 2)));
    // What is around it stays, not dimmed.
    assert_eq!(canvas.row(2).chars().take(area.x).collect::<String>(), "x".repeat(area.x));
    assert_eq!(canvas.row(2).chars().skip(area.x + width).collect::<String>(), "xx");
    assert_eq!(canvas.style(area.x, 2), Style::fg(Color::Red));
    // ⚑ in bold red, the name in its color, the text bold, the tab dimmed, the key in cyan.
    let at = |text: &str| {
        let row = canvas.row(2);
        row[..row.find(text).unwrap()].chars().count()
    };
    assert_eq!(canvas.style(at("⚑"), 2), Style::fg(Color::Red).bold());
    assert_eq!(canvas.style(at("dev-saisie"), 2), Style::fg(Color::Blue).bold());
    assert_eq!(canvas.style(at("dev-saisie") + 11, 2), Style::PLAIN.bold());
    assert_eq!(canvas.style(at("Agents"), 2), Style::PLAIN.dim());
    assert_eq!(canvas.style(at(&format!("{ALT}g")), 2), Style::fg(Color::DarkCyan));
}

/// A narrow screen: ⌥g goes, then the tab, then the text is cut; too small, no notice.
#[test]
fn a_notice_on_a_narrow_screen() {
    let notice = saisie();
    let text = notice_text("dev-saisie");
    let inner = |screen: usize| {
        let area = notice_area(Rect { x: 0, y: 0, width: screen, height: 24 }, &notice);
        let mut canvas = Canvas::new(screen, 24);
        super::notice(&mut canvas, area, &notice, UNICODE);
        let row = canvas.row(2).chars().skip(area.x + 1).take(area.width.saturating_sub(2)).collect::<String>();
        (row, area)
    };
    let with_tab = format!(" ⚑ {text} · Agents (1) ");
    let (row, area) = inner(columns(&with_tab) + 4);
    assert_eq!((row.as_str(), area.x), (with_tab.as_str(), 0));
    let bare = format!(" ⚑ {text} ");
    assert_eq!(inner(columns(&with_tab) + 3).0, bare);
    assert_eq!(inner(columns(&bare) + 4).0, bare);
    let (row, area) = inner(20);
    assert!(columns(&row) <= 16, "{row:?}");
    assert!(row.starts_with(" ⚑ dev-saisie") && row.ends_with("… "), "{row:?}");
    assert_eq!(area.x + area.width, 18);
    assert_eq!(inner(11).1.width, 0, "too narrow");
    assert_eq!(notice_area(Rect { x: 0, y: 0, width: 80, height: 3 }, &notice).width, 0, "too low");
    // Nerd Font: its bell.
    let area = notice_area(Rect { x: 0, y: 0, width: 140, height: 39 }, &notice);
    let mut canvas = Canvas::new(140, 40);
    super::notice(&mut canvas, area, &notice, Look { glyphs: Glyphs::Nerd, frame: 0 });
    assert_eq!(canvas.cell(area.x + 2, 2), "\u{F009E}");
}

/// No size, however small, makes the chrome panic: every header, the bar, a notice and the choices, from 0 to
/// 120 columns and 0 to 40 rows (reviewer's sweep, kept).
#[test]
fn no_size_too_small() {
    let long = "un-nom-de-membre-de-quarante-cinq-caractères";
    let mut headers = Vec::new();
    for name in ["a", "dev-rendu", "日本語の名前の長いもの", long] {
        for form in 0..=5 {
            headers.push(Header { form, ..rich(name, State::Working) });
        }
        headers.extend([
            Header { hover: Some(Part::Name), scroll: Some((214, 3000)), ..rich(name, State::Waiting) },
            Header { note: Some("moteur en échec : panique"), compactable: true, ..rich(name, State::Idle) },
            Header { effort: Some("max"), zoom: Some(Zoom::Out), hover: Some(Part::Zoom), ..rich(name, State::Idle) },
        ]);
    }
    let counts = Hint::Counts { working: 3, waiting: 12, error: 4, idle: 140 };
    headers.extend([
        Header { name: "Journal", hint: Some(Hint::JournalSize), ..Header::default() },
        Header { name: "Tableau de bord", hint: Some(counts), ..Header::default() },
        Header::default(),
    ]);
    let titles = ["Interlocuteurs", "Agents (1)", "日本語", "x"];
    let bars: Vec<Vec<Tab<'_>>> = (0..=9)
        .flat_map(|n| {
            let tab = move |i: usize, zoomed| Tab {
                title: titles[i % titles.len()],
                active: i == 0,
                state: [State::Working, State::Waiting, State::Idle][i % 3],
                zoomed,
                // Every width, with the layout button on the current tab: nothing overflows.
                switch: (i == 0).then_some(Switch::ToFocus),
            };
            [(0..n).map(|i| tab(i, None)).collect(), (0..n).map(|i| tab(i, Some(long))).collect()]
        })
        .collect();
    let notices = [saisie(), Notice { member: String::new(), text: notice_text(""), ..saisie() }];
    let choices = [quit_choice(), compact_choice(long, Some(88)), compact_choice("dev", None)];
    let looks = [UNICODE, Look { glyphs: Glyphs::Nerd, frame: 7 }];
    // Every small width, which carry nearly all the overflows, then a step; a thread for each share of them.
    let widths: Vec<usize> = (0..=60).chain((64..=120).step_by(4)).collect();
    std::thread::scope(|scope| {
        for chunk in widths.chunks(widths.len().div_ceil(8)) {
            let (headers, bars, notices, choices) = (&headers, &bars, &notices, &choices);
            scope.spawn(move || {
                // A column and a row more than the widest area: what is drawn out of its area shows there.
                let mut screen = Canvas::new(121, 42);
                for look in looks {
                    for &width in chunk {
                        // A header depends on the width; the frame's height on a few rows only.
                        for height in 0..=4 {
                            let area = Rect { x: 0, y: 0, width, height };
                            for header in headers {
                                within(&mut screen, area, |canvas| {
                                    frame(canvas, area, header, height % 2 == 0, look);
                                });
                            }
                        }
                        for header in headers {
                            let _ = (form(header, width), shows_since(header, width));
                        }
                        let row = Rect { x: 0, y: 40, width, height: 1 };
                        for tabs in bars {
                            for hover in [None, Some(Target::Tab(1)), Some(Target::Quit)] {
                                within(&mut screen, row, |canvas| {
                                    bar(canvas, row, "mux", tabs, look, hover);
                                });
                            }
                        }
                        for height in (0..=20).chain((24..=40).step_by(4)) {
                            let area = Rect { x: 0, y: 0, width, height };
                            for shown in notices {
                                let at = notice_area(area, shown);
                                let inside = at.x + at.width <= width && at.y + at.height <= height.max(at.y);
                                assert!(inside, "{at:?} in {area:?}");
                                within(&mut screen, at, |canvas| notice(canvas, at, shown, look));
                            }
                            for asked in choices {
                                let at = choice_area(area, asked);
                                assert!(at.x + at.width <= width && at.y + at.height <= height, "{at:?} in {area:?}");
                                within(&mut screen, at, |canvas| {
                                    choice(canvas, at, asked);
                                });
                            }
                        }
                    }
                }
            });
        }
    });
    // A frame as high as the screen, for its sides.
    let mut screen = Canvas::new(121, 42);
    for height in 0..=40 {
        let area = Rect { x: 0, y: 0, width: 120, height };
        within(&mut screen, area, |canvas| {
            frame(canvas, area, &headers[0], false, UNICODE);
        });
    }
}

/// The witnesses see a cell drawn out of the area.
#[test]
#[should_panic(expected = "out of")]
fn a_stray_cell_seen() {
    let mut canvas = Canvas::new(10, 5);
    let area = Rect { x: 0, y: 0, width: 4, height: 2 };
    within(&mut canvas, area, |canvas| {
        canvas.put(0, 0, "abcde", Style::PLAIN);
    });
}

/// Even a stray sign the same as the witness's: its style tells it.
#[test]
#[should_panic(expected = "out of")]
fn a_stray_witness_seen() {
    let mut canvas = Canvas::new(10, 5);
    let area = Rect { x: 0, y: 0, width: 4, height: 2 };
    within(&mut canvas, area, |canvas| {
        canvas.put(4, 1, "¤", Style::PLAIN);
    });
}

/// Draws with `draw` in `area` of `canvas`, and checks that nothing went out of it to the right or below: the
/// column after it and the row under it, each a cell longer, kept as they were.
fn within(canvas: &mut Canvas, area: Rect, draw: impl FnOnce(&mut Canvas)) {
    let (right, bottom) = (area.x + area.width, area.y + area.height);
    let witnesses: Vec<(usize, usize)> = (area.y..=bottom)
        .map(|y| (right, y))
        .chain((area.x..right).map(|x| (x, bottom)))
        .filter(|&(x, y)| x < canvas.width() && y < canvas.height())
        .collect();
    // A sign and a style the chrome never draws: a stray separator or blank shows as well.
    let witness = Style { strike: true, italic: true, ..Style::PLAIN };
    for &(x, y) in &witnesses {
        canvas.put(x, y, "¤", witness);
    }
    draw(canvas);
    for &(x, y) in &witnesses {
        let kept = canvas.cell(x, y) == "¤" && canvas.style(x, y) == witness;
        assert!(kept, "drawn at {x}, {y}, out of {area:?}");
    }
}

#[test]
fn a_wide_name_cut_whole() {
    let header = member("日本語の名前", State::Idle);
    assert_eq!(framed(&header, 22, false)[0], "╭─ ◷ 日本語の名… ────╮");
    // A character two columns wide never cut in half: one column short, it goes whole.
    assert_eq!(framed(&header, 21, false)[0], "╭─ ◷ 日本語の… ─────╮");
}

/// « ● high » in the language of the run: « ● élevé » in French, a column wider.
fn high() -> String {
    t!("● élevé", "● high")
}

fn tabs(state: [State; 3]) -> [Tab<'static>; 3] {
    [
        Tab { title: "Interlocuteurs", active: true, state: state[0], zoomed: None, switch: None },
        Tab { title: "Agents (1)", active: false, state: state[1], zoomed: None, switch: None },
        Tab { title: "Agents (2)", active: false, state: state[2], zoomed: None, switch: None },
    ]
}

fn drawn_bar(width: usize, tabs: &[Tab<'_>], look: Look) -> (String, Vec<(Rect, Target)>, Canvas) {
    let mut canvas = Canvas::new(width, 1);
    let zones = bar(&mut canvas, Rect { x: 0, y: 0, width, height: 1 }, "mux", tabs, look, None);
    (canvas.row(0), zones, canvas)
}

#[test]
fn the_bar_in_a_wide_window() {
    let tabs = tabs([State::Working, State::Waiting, State::Idle]);
    let (row, zones, canvas) = drawn_bar(100, &tabs, UNICODE);
    let left = " mux │ ⠹ 1 Interlocuteurs │ ⚑ 2 Agents (1) │ ◷ 3 Agents (2) │";
    let menu = format!(" menu ({ALT}r) ");
    let quit = format!(" {} ({ALT}q) ", t!("quitter", "quit"));
    let gap = 100 - columns(left) - columns(&menu) - 1 - columns(&quit) - 1;
    assert_eq!(row, format!("{left}{}{menu} {}", " ".repeat(gap), quit.trim_end()));
    // The current tab in reverse video, ⚑ in red before the one waiting, the others dimmed.
    assert!(canvas.style(7, 0).reverse && canvas.style(9, 0).bold);
    assert_eq!(canvas.style(28, 0), sign_style(State::Waiting));
    assert!(canvas.style(30, 0).dim);
    assert_eq!(
        zones,
        [
            (Rect { x: 6, y: 0, width: 20, height: 1 }, Target::Tab(0)),
            (Rect { x: 27, y: 0, width: 16, height: 1 }, Target::Tab(1)),
            (Rect { x: 44, y: 0, width: 16, height: 1 }, Target::Tab(2)),
            (
                Rect { x: 99 - columns(&quit) - 1 - columns(&menu), y: 0, width: columns(&menu), height: 1 },
                Target::Menu
            ),
            (Rect { x: 99 - columns(&quit), y: 0, width: columns(&quit), height: 1 }, Target::Quit),
        ]
    );
    // Each zone's text: its tab or its button.
    let text = |r: Rect| row.chars().skip(r.x).take(r.width).collect::<String>();
    assert_eq!(text(zones[1].0), " ⚑ 2 Agents (1) ");
    assert_eq!(text(zones[4].0), quit.trim_end());
}

/// The layout button of the current tab: « grille (⌥l) » on a tab in « focus », « focus (⌥l) »
/// on one in the grid, before the menu's; nothing for a tab with no focus. Narrow, its key alone; clickable.
#[test]
fn the_layout_button() {
    let mut tabs = tabs([State::Working, State::Idle, State::Idle]);
    let (row, zones, _) = drawn_bar(100, &tabs, UNICODE);
    assert!(!row.contains("(⌥l)") && !zones.iter().any(|(_, t)| *t == Target::Layout), "{row}");
    for (switch, word) in [(Switch::ToGrid, t!("grille", "grid")), (Switch::ToFocus, "focus".to_string())] {
        tabs[0].switch = Some(switch);
        let (row, zones, canvas) = drawn_bar(100, &tabs, UNICODE);
        let button = format!(" {word} ({ALT}l) ");
        let at = row.find(&button).unwrap_or_else(|| panic!("{button:?} in {row:?}"));
        let menu = row.find(&format!(" menu ({ALT}r) ")).unwrap();
        assert!(at < menu, "before the menu's: {row}");
        let (rect, _) = zones.iter().find(|(_, t)| *t == Target::Layout).unwrap();
        let x = row[..at].chars().count();
        assert_eq!((rect.x, rect.width), (x, columns(&button)));
        assert!(canvas.style(x + 2, 0).reverse, "drawn as the other buttons");
        // Not on the current tab: not shown.
        let mut other = tabs;
        other[0].active = false;
        other[1].active = true;
        assert!(!drawn_bar(100, &other, UNICODE).0.contains("(⌥l)"));
    }
    // Hovered, bold; narrow, its key alone.
    let mut canvas = Canvas::new(100, 1);
    bar(&mut canvas, Rect { x: 0, y: 0, width: 100, height: 1 }, "mux", &tabs, UNICODE, Some(Target::Layout));
    let at = canvas.row(0).find("focus (").unwrap();
    assert!(canvas.style(canvas.row(0)[..at].chars().count(), 0).bold);
    let (row, zones, _) = drawn_bar(60, &tabs, UNICODE);
    assert!(row.contains(&format!(" {ALT}l ")) && !row.contains("focus ("), "{row}");
    assert!(zones.iter().any(|(_, t)| *t == Target::Layout));
}

/// Narrower: the other tabs keep their numbers, the team goes, the buttons keep their keys, the
/// current title is cut.
#[test]
fn the_bar_in_a_narrow_window() {
    let tabs = tabs([State::Working, State::Waiting, State::Idle]);
    let menu = format!(" menu ({ALT}r) ");
    let quit = format!(" {} ({ALT}q) ", t!("quitter", "quit"));
    let long = format!("{menu} {quit}");
    let numbers = " mux │ ⠹ 1 Interlocuteurs │ ⚑ 2 │ ◷ 3 │";
    let (row, ..) = drawn_bar(columns(numbers) + 1 + columns(&long) + 1, &tabs, UNICODE);
    assert_eq!(row, format!("{numbers} {}", long.trim_end()));

    let without = "│ ⠹ 1 Interlocuteurs │ ⚑ 2 │ ◷ 3 │";
    let (row, zones, _) = drawn_bar(columns(without) + 1 + columns(&long) + 1, &tabs, UNICODE);
    assert_eq!(row, format!("{without} {}", long.trim_end()));
    assert_eq!(
        zones.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
        [Target::Tab(0), Target::Tab(1), Target::Tab(2), Target::Menu, Target::Quit]
    );

    let keys = format!(" {ALT}r   {ALT}q ");
    let (row, ..) = drawn_bar(columns(without) + 1 + columns(&keys) + 1, &tabs, UNICODE);
    assert_eq!(row, format!("{without} {}", keys.trim_end()));
    let (row, ..) = drawn_bar(columns(without) + 1 + columns(&keys) + 1 - 6, &tabs, UNICODE);
    assert_eq!(row, format!("│ ⠹ 1 Interlo… │ ⚑ 2 │ ◷ 3 │ {}", keys.trim_end()));
}

/// The current tab says who is zoomed; its title cut before the others lose their room.
#[test]
fn the_zoom_in_the_current_tab() {
    let mut tabs = tabs([State::Working, State::Waiting, State::Idle]);
    tabs[0].zoomed = Some("dev-saisie");
    let (row, zones, _) = drawn_bar(140, &tabs, UNICODE);
    assert!(row.starts_with(" mux │ ⠹ 1 Interlocuteurs  ⤢ dev-saisie │ ⚑ 2 Agents (1) │"), "{row}");
    assert_eq!(zones[0].0.width, columns(" ⠹ 1 Interlocuteurs  ⤢ dev-saisie "));
    let nerd = Look { glyphs: Glyphs::Nerd, frame: 2 };
    assert!(drawn_bar(140, &tabs, nerd).0.contains("Interlocuteurs  \u{F0293} dev-saisie"));
    // Narrow, the buttons to their keys (⌥r, or Alt+r off macOS): the others to their numbers first.
    let keys = format!(" {ALT}r   {ALT}q ");
    let room = |tabs: &str| columns(tabs) + 1 + columns(&keys) + 1;
    let numbers = "│ ⠹ 1 Interlocuteurs  ⤢ dev-saisie │ ⚑ 2 │ ◷ 3 │";
    let (row, ..) = drawn_bar(room(numbers), &tabs, UNICODE);
    assert_eq!(row, format!("{numbers} {}", keys.trim_end()));
    // Narrower: the current title goes before who is zoomed, then that is cut, then only ⤢ stays; packed, it
    // stays too, before the buttons go.
    let least = "│ ⠹ 1 ⤢ │ ⚑ 2 │ ◷ 3 │";
    let (row, ..) = drawn_bar(room(least), &tabs, UNICODE);
    assert_eq!(row, format!("{least} {}", keys.trim_end()));
    let (row, zones, _) = drawn_bar(room(least) - 1, &tabs, UNICODE);
    assert!(row.starts_with("│⠹1⤢│⚑2│◷3│") && row.ends_with(keys.trim_end()), "{row}");
    assert_eq!(zones[0].0.width, 3);
    let (row, ..) = drawn_bar(columns("│⠹1⤢│⚑2│◷3│") + 1 + columns(&keys) - 1, &tabs, UNICODE);
    assert_eq!(row.trim_end(), "│⠹1⤢│⚑2│◷3│");
    let zoomed = "│ ⠹ 1 ⤢ dev-saisie │ ⚑ 2 │ ◷ 3 │";
    let (row, ..) = drawn_bar(room(zoomed), &tabs, UNICODE);
    assert_eq!(row, format!("{zoomed} {}", keys.trim_end()));
    let (row, ..) = drawn_bar(room(zoomed) - 4, &tabs, UNICODE);
    assert!(row.starts_with("│ ⠹ 1 ⤢ dev-s") && row.contains("… │ ⚑ 2 │ ◷ 3 │"), "{row}");
}

/// The tab or button under the mouse, brighter.
#[test]
fn the_bar_hovered() {
    let tabs = tabs([State::Working, State::Waiting, State::Idle]);
    let mut canvas = Canvas::new(100, 1);
    let row = Rect { x: 0, y: 0, width: 100, height: 1 };
    let zones = bar(&mut canvas, row, "mux", &tabs, UNICODE, Some(Target::Tab(1)));
    let label = zones[1].0.x + 3;
    assert_eq!(canvas.style(label, 0), underlined(Style::PLAIN.bold()));
    assert_eq!(canvas.style(label - 1, 0), Style::PLAIN, "the blank before it as it was");
    assert_eq!(canvas.style(zones[2].0.x + 3, 0), Style::PLAIN.dim(), "the others as they were");
    assert_eq!(canvas.style(zones[1].0.x + 1, 0), sign_style(State::Waiting), "its sign as it was");
    let zones = bar(&mut canvas, row, "mux", &tabs, UNICODE, Some(Target::Quit));
    assert_eq!(canvas.style(zones[4].0.x + 1, 0), Style::PLAIN.reverse().bold());
    assert_eq!(canvas.style(zones[3].0.x + 1, 0), Style::PLAIN.reverse());
    // The current tab hovered stays as it is.
    let zones = bar(&mut canvas, row, "mux", &tabs, UNICODE, Some(Target::Tab(0)));
    assert_eq!(canvas.style(zones[0].0.x + 3, 0), underlined(Style::PLAIN.reverse().bold()));
    assert_eq!(canvas.style(zones[0].0.x + 2, 0), Style::PLAIN.reverse().bold());
}

/// Narrower still: the blanks go, then the buttons; a sign never.
#[test]
fn the_bar_packed() {
    let tabs = tabs([State::Working, State::Waiting, State::Waiting]);
    let keys = format!(" {ALT}r   {ALT}q ");
    let spaced = "│ ⠹ 1 │ ⚑ 2 │ ⚑ 3 │";
    let (row, ..) = drawn_bar(columns(spaced) + 1 + columns(&keys) + 1, &tabs, UNICODE);
    assert_eq!(row, format!("{spaced} {}", keys.trim_end()));
    let packed = "│⠹1│⚑2│⚑3│";
    let (row, zones, canvas) = drawn_bar(columns(spaced) + columns(&keys) + 1, &tabs, UNICODE);
    assert!(row.starts_with(packed) && row.ends_with(keys.trim_end()), "{row}");
    assert_eq!(zones[1].0, Rect { x: 4, y: 0, width: 2, height: 1 });
    assert!(canvas.style(1, 0).reverse && canvas.style(2, 0).reverse, "the current one, reversed");
    assert_eq!(canvas.style(4, 0), sign_style(State::Waiting));
    // Without the buttons, at last.
    let (row, zones, _) = drawn_bar(columns(packed) + 1 + columns(&keys), &tabs, UNICODE);
    assert_eq!(row.trim_end(), packed);
    assert_eq!(zones.len(), 3);
    for width in [12, 10] {
        assert!(drawn_bar(width, &tabs, UNICODE).0.starts_with(packed), "{width}");
    }
    // Narrower than the signs: cut at the edge, the first ones kept.
    assert_eq!(drawn_bar(6, &tabs, UNICODE).0, "│⠹1│⚑2");
}

#[test]
fn nerd_font_signs() {
    let look = Look { glyphs: Glyphs::Nerd, frame: 0 };
    let tabs = [Tab { title: "Agents", state: State::Waiting, ..Tab::default() }];
    let (row, ..) = drawn_bar(60, &tabs, look);
    assert!(row.starts_with(" mux │ \u{F009E} 1 Agents │"), "{row}");
    assert!(row.contains("\u{F035C} menu") && row.contains("\u{F0343} "), "{row}");
}

#[test]
fn the_spinner_turns() {
    let tabs = [Tab { title: "A", active: true, state: State::Working, zoomed: None, switch: None }];
    let first = drawn_bar(60, &tabs, Look { frame: 0, ..UNICODE }).0;
    let next = drawn_bar(60, &tabs, Look { frame: 1, ..UNICODE }).0;
    assert!(first.starts_with(" mux │ ⠋ 1 A │") && next.starts_with(" mux │ ⠙ 1 A │"));
}
