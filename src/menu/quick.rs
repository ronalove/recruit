// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The menu as a list only (`recruit _menu … --member m --field model|effort --list`, direction A of the mockups of
//! 2026-10-11): the values of one field under its pane's header, in a frame of the text's color; « default: … » first,
//! « current » on the value in place, ↻ on a value that starts the member again, a question under the values when the
//! member is at work, a text typed in place for another model. The pane is exactly its size, which the menu tells the
//! server (`CSI 8 ; rows ; columns t`) whenever it changes.

use super::draw::{Drawn, Look, Zone, button, input, muted, put_effort_text, selected, wrap};
use super::sheet::{Confirm, Dropdown, Overlay, Said, Sheet, Target, Typing};
use crate::canvas::{Canvas, Style, columns, fit};
use crate::t;
use crossterm::style::Color;

/// The narrowest inside of the frame, and the one with a question or a refusal in it.
const MIN_INSIDE: usize = 22;
const ASKED_INSIDE: usize = 44;
/// The widest a text typed in place makes the list.
const TYPED_INSIDE: usize = 44;

/// What a row of the list shows.
struct Row {
    /// Its text, its effort in colors apart.
    text: String,
    /// The effort level when the row is one: its sign and its word in their colors.
    effort: Option<String>,
    /// Beside it, on the right: « current » or ↻.
    right: Option<String>,
    /// A default or the way to another value: written as it is.
    plain: bool,
}

/// The list's rows, as the sheet stands. `short`: « default » without where it comes from.
fn rows(list: &Dropdown, short: bool) -> Vec<Row> {
    let current = t!("actuel", "current");
    list.items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let default = item.value.is_none() && !item.other;
            let mut text = item.label.clone();
            if default && short {
                // « default: Opus (settings.toml) » → « default: Opus ».
                if let Some(at) = text.rfind(" (")
                    && text.ends_with(')')
                {
                    text.truncate(at);
                }
            }
            // An effort's level: its label (sign and name) in its colors, wherever it is in the text.
            let effort = item.level.clone();
            let right =
                if list.current == Some(i) { Some(current.clone()) } else { item.restarts.then(|| "↻".to_string()) };
            Row { text, effort, right, plain: default || item.other }
        })
        .collect()
}

/// The list, the question under it, the text typed in place: which of them the sheet has open.
struct Parts<'a> {
    list: &'a Dropdown,
    confirm: Option<&'a Confirm>,
    typing: Option<&'a Typing>,
}

fn parts(sheet: &Sheet) -> Option<Parts<'_>> {
    let quick = sheet.quick.as_ref()?;
    Some(match &sheet.overlay {
        Overlay::List(list) => Parts { list, confirm: None, typing: None },
        Overlay::Confirm(confirm) => Parts { list: &quick.list, confirm: Some(confirm), typing: None },
        Overlay::Typing(typing) => Parts { list: &quick.list, confirm: None, typing: Some(typing) },
        Overlay::None => Parts { list: &quick.list, confirm: None, typing: None },
    })
}

/// What the question says under the values, on rows `inside` wide: its title, its notes, those it interrupts.
fn question(confirm: &Confirm, inside: usize, look: &Look) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    for line in &confirm.lines {
        for part in wrap(line, inside, 4) {
            out.push((part, Style::PLAIN));
        }
    }
    for (state, said) in &confirm.busy {
        let sign = state.icon(look.glyphs, look.frame);
        for (i, part) in wrap(said, inside.saturating_sub(2), 3).into_iter().enumerate() {
            out.push((if i == 0 { format!("{sign} {part}") } else { format!("  {part}") }, Style::fg(state.color())));
        }
    }
    out
}

/// A path or any word too long for a row, cut from its start (« …/settings.toml »): its end is what tells.
fn shorten(word: &str, room: usize) -> String {
    let chars: Vec<char> = word.chars().collect();
    if chars.len() <= room || room < 2 {
        return word.to_string();
    }
    format!("…{}", chars[chars.len() - (room - 1)..].iter().collect::<String>())
}

/// A write refused, as the last rows of the frame (three at most): a long word cut from its start, a long text from
/// its beginning, so that the reason, at the end, is always there.
fn refusal(sheet: &Sheet, inside: usize) -> Vec<String> {
    let Some(Said::Failed(error)) = &sheet.said else { return Vec::new() };
    // Two columns of sign or indent, one of blank on each side of the text, the frame.
    let room = inside.saturating_sub(4);
    let words: Vec<String> = error.split_whitespace().map(|word| shorten(word, room)).collect();
    let mut rows = wrap(&words.join(" "), room, usize::MAX / 2);
    if rows.len() > 3 {
        rows = rows.split_off(rows.len() - 3);
        rows[0] = fit(&format!("… {}", rows[0]), room);
    }
    rows.into_iter().enumerate().map(|(i, r)| if i == 0 { format!("✗ {r}") } else { format!("  {r}") }).collect()
}

/// Where everything goes.
struct Plan {
    inside: usize,
    rows: Vec<Row>,
    question: Vec<(String, Style)>,
    refusal: Vec<String>,
    typing_error: bool,
}

impl Plan {
    fn of(sheet: &Sheet, look: &Look, short: bool) -> Option<Plan> {
        let parts = parts(sheet)?;
        let rows = rows(parts.list, short);
        let label = rows.iter().map(|r| columns(&r.text)).max().unwrap_or(0);
        let right = rows.iter().filter_map(|r| r.right.as_deref().map(columns)).max().unwrap_or(0);
        // « › », the text, a gap, the right column, a blank on each side.
        let mut inside = (3 + label + if right > 0 { 2 + right } else { 0 } + 1).max(MIN_INSIDE);
        if let Some(typing) = parts.typing {
            inside = inside.max((3 + columns(&typing.text) + 2).min(TYPED_INSIDE));
        }
        if parts.confirm.is_some() || sheet.said.as_ref().is_some_and(|s| matches!(s, Said::Failed(_))) {
            inside = inside.max(ASKED_INSIDE);
        }
        let question = parts.confirm.map(|c| question(c, inside - 2, look)).unwrap_or_default();
        Some(Plan {
            inside,
            rows,
            question,
            refusal: refusal(sheet, inside),
            typing_error: parts.typing.is_some_and(|t| t.error.is_some()),
        })
    }

    /// Rows and columns, the frame included.
    fn size(&self, sheet: &Sheet) -> (usize, usize) {
        let confirm = sheet.quick.as_ref().and(match &sheet.overlay {
            Overlay::Confirm(_) => Some(()),
            _ => None,
        });
        let asked = if confirm.is_some() {
            // The separator, the title, the notes, a blank, the buttons.
            1 + 1 + self.question.len() + 1 + 1
        } else {
            0
        };
        let rows = 2 + self.rows.len() + asked + usize::from(self.typing_error) + self.refusal.len();
        (rows, self.inside + 2)
    }
}

/// The rows and columns the pane wants, frame included; None when the sheet is not a quick list.
pub(crate) fn size(sheet: &Sheet, look: &Look, short: bool) -> Option<(usize, usize)> {
    Plan::of(sheet, look, short).map(|plan| plan.size(sheet))
}

/// The columns the pane wants with all that it can say, and without where the default comes from.
#[cfg(test)]
pub(crate) fn widths(sheet: &Sheet, look: &Look) -> Option<(usize, usize)> {
    Some((size(sheet, look, false)?.1, size(sheet, look, true)?.1))
}

/// The list on a screen of `width` × `height`, from its top left corner.
pub(crate) fn draw(sheet: &Sheet, look: &Look, short: bool, width: usize, height: usize) -> Drawn {
    let mut canvas = Canvas::new(width.max(1), height.max(1));
    let mut zones = Vec::new();
    let Some(plan) = Plan::of(sheet, look, short) else { return Drawn { canvas, zones, split: width } };
    let parts = parts(sheet).expect("a plan comes from parts");
    let (wanted, columns_) = plan.size(sheet);
    // Short of height, the values far from the cursor go; the question, its buttons and the frame stay.
    let kept = plan.rows.len().saturating_sub(wanted.saturating_sub(height.max(1))).max(1);
    let rows = wanted - plan.rows.len().saturating_sub(kept);
    canvas.frame(0, 0, columns_, rows, Style::PLAIN);
    let (left, right) = (1, columns_ - 1);
    let mut y = 1;
    // The cursor stays on the value chosen under a question, but is not shown moving.
    let at = parts.list.at;
    let first = at.saturating_sub(kept / 2).min(plan.rows.len().saturating_sub(kept));
    for (i, row) in plan.rows.iter().enumerate().skip(first).take(kept) {
        let on = i == at;
        let typed = parts.typing.filter(|_| parts.list.items.get(i).is_some_and(|item| item.other));
        if let Some(typing) = typed {
            // The text, in place of « Other… ».
            canvas.put(left + 1, y, "›", super::draw::key_style());
            let mut x = left + 3;
            for (n, ch) in typing.text.chars().enumerate() {
                let style = if n == typing.cursor { Style::PLAIN.reverse() } else { Style::PLAIN.on(input()) };
                x = canvas.put_in(x, y, right - 1, &ch.to_string(), style);
            }
            if typing.cursor >= typing.text.chars().count() {
                canvas.put_in(x, y, right - 1, " ", Style::PLAIN.reverse());
            }
        } else {
            if on {
                canvas.put(left + 1, y, "›", super::draw::key_style());
            }
            let style = if row.plain && !on {
                Style::PLAIN
            } else if on {
                Style::PLAIN.bold()
            } else {
                Style::PLAIN
            };
            let end = right - 1;
            match &row.effort {
                Some(level) => {
                    // The sign and the name in their colors, the rest as the line is.
                    put_effort_text(&mut canvas, left + 3, y, end, &row.text, level, style);
                }
                None => {
                    canvas.put_in(left + 3, y, end, &row.text, style);
                }
            }
            if let Some(text) = &row.right {
                let muted_right = if text == "↻" { Style::PLAIN } else { muted() };
                canvas.put_right(right - 1, y, text, muted_right);
            }
        }
        // The background over the whole line inside, the text written: the cursor's, the typed text's.
        if typed.is_some() {
            canvas.tint(left, y, right - left, input());
        } else if on {
            canvas.tint(left, y, right - left, selected());
        }
        zones.insert(0, Zone { x: left, y, width: right - left, height: 1, target: Target::Item(i) });
        y += 1;
    }
    if let Some(confirm) = parts.confirm {
        canvas.hline(1, y, columns_ - 2, '─', Style::PLAIN);
        canvas.put(0, y, "├", Style::PLAIN);
        canvas.put(columns_ - 1, y, "┤", Style::PLAIN);
        y += 1;
        canvas.put_in(left + 1, y, right - 1, &fit(&confirm.title, plan.inside - 2), Style::PLAIN.bold());
        y += 1;
        for (line, style) in &plan.question {
            canvas.put_in(left + 1, y, right - 1, line, *style);
            y += 1;
        }
        y += 1;
        buttons(&mut canvas, &mut zones, confirm, left + 1, y, right - 1);
        y += 1;
    }
    if let Some(error) = parts.typing.and_then(|t| t.error.as_deref()) {
        canvas.put_in(left + 1, y, right - 1, &fit(&format!("✗ {error}"), plan.inside - 2), Style::fg(Color::Red));
        y += 1;
    }
    for line in &plan.refusal {
        canvas.put_in(left + 1, y, right - 1, line, Style::fg(Color::Red));
        y += 1;
    }
    Drawn { canvas, zones, split: width }
}

/// « Restart (r) » and « Cancel (c) »: the one chosen reversed.
fn buttons(canvas: &mut Canvas, zones: &mut Vec<Zone>, confirm: &Confirm, x: usize, y: usize, end: usize) {
    let (yes_key, no_key) = confirm.letters.unwrap_or(('r', 'a'));
    let yes = format!(" {} ({yes_key}) ", confirm.yes);
    let no = format!(" {} ({no_key}) ", t!("Annuler", "Cancel"));
    let color = if confirm.danger { Color::Red } else { Color::Cyan };
    let yes_style =
        if confirm.at == 0 { Style::fg(Color::Black).on(color).bold() } else { Style::fg(color).on(button()) };
    let no_style = if confirm.at == 1 { Style::PLAIN.bold().on(selected()) } else { Style::PLAIN.on(button()) };
    let after_yes = canvas.put_in(x, y, end, &yes, yes_style);
    let after_no = canvas.put_in(after_yes + 2, y, end, &no, no_style);
    zones.insert(0, Zone { x, y, width: after_yes - x, height: 1, target: Target::Yes });
    zones.insert(0, Zone { x: after_yes + 2, y, width: after_no - (after_yes + 2), height: 1, target: Target::No });
}

#[cfg(test)]
mod tests {
    use super::super::sheet::fixture::{Fake, team};
    use super::super::sheet::{Effect, Fid, Key};
    use super::*;
    use crate::look::{Glyphs, State};

    fn look() -> Look {
        Look { glyphs: Glyphs::Unicode, frame: 0, now: 1000, option: true }
    }

    /// `dev`'s `field` as a quick list.
    fn opened(field: Fid, env: &Fake) -> Sheet {
        let mut s = Sheet::new(team(&[("lead", true), ("dev", false)]));
        s.open_list("dev", field, env);
        s
    }

    fn text(s: &Sheet, short: bool) -> Vec<String> {
        let (rows, cols) = size(s, &look(), short).expect("a quick list");
        let drawn = draw(s, &look(), short, cols, rows);
        (0..rows).map(|y| drawn.canvas.row(y)).collect()
    }

    #[test]
    fn the_values_alone_in_a_frame() {
        let s = opened(Fid::Effort, &Fake::quiet());
        let rows = text(&s, false);
        assert!(rows[0].starts_with('╭') && rows.last().unwrap().starts_with('╰'), "{rows:#?}");
        // No layer gives an effort: what Claude Code applies, as the member's mod reported it.
        assert!(
            rows[1].contains('›')
                && rows[1].contains(&t!(
                    "défaut : {} (Claude Code)",
                    "default: {} (Claude Code)",
                    crate::look::effort_label("high")
                ))
        );
        assert!(rows[1].contains(&t!("actuel", "current")), "the value in place: {}", rows[1]);
        for (row, level) in rows[2..].iter().zip(["low", "medium", "high", "xhigh", "max"]) {
            assert!(row.contains(&crate::look::effort_label(level)), "{row}");
        }
        // Nothing else: no help, no title.
        assert_eq!(rows.len(), 2 + 6);
        let (rows_, cols) = size(&s, &look(), false).unwrap();
        assert_eq!(rows_, rows.len());
        assert!(rows.iter().all(|r| columns(r) <= cols));
    }

    #[test]
    fn the_effort_in_its_colors() {
        let s = opened(Fid::Effort, &Fake::quiet());
        let (rows, cols) = size(&s, &look(), false).unwrap();
        let drawn = draw(&s, &look(), false, cols, rows);
        // « ● high »: the sign and the word in the same color.
        let line = (0..rows).find(|y| drawn.canvas.row(*y).contains('●')).unwrap();
        let sign = drawn.canvas.row(line).chars().position(|c| c == '●').unwrap();
        assert_eq!(drawn.canvas.style(sign, line).fg, crate::look::effort_color("high"));
        assert_eq!(drawn.canvas.style(sign + 2, line).fg, crate::look::effort_color("high"));
    }

    #[test]
    fn a_value_that_restarts_is_marked() {
        let env = Fake::restarting(&["dev"]);
        let mut s = opened(Fid::Effort, &env);
        assert!(s.unmarked(), "read from the files after the first image");
        assert!(!text(&s, false).iter().any(|r| r.contains('↻')));
        s.mark_restarts(&env);
        assert!(!s.unmarked());
        let rows = text(&s, false);
        assert!(rows[2..rows.len() - 1].iter().all(|r| r.contains('↻')), "{rows:#?}");
        let quiet = text(&opened(Fid::Effort, &Fake::quiet()), false);
        assert!(!quiet.iter().any(|r| r.contains('↻')));
    }

    #[test]
    fn escape_ends_it_and_a_choice_too() {
        let env = Fake::quiet();
        let mut s = opened(Fid::Effort, &env);
        assert_eq!(s.key(Key::Esc, &env), [Effect::Quit]);
        let mut s = opened(Fid::Effort, &env);
        s.key(Key::Down, &env);
        let effects = s.key(Key::Enter, &env);
        assert!(matches!(&effects[..], [Effect::Apply { .. }]), "{effects:?}");
        // A click on a value chooses it.
        let mut s = opened(Fid::Effort, &env);
        let effects = s.click(Target::Item(3), &env);
        assert!(matches!(&effects[..], [Effect::Apply { .. }]), "{effects:?}");
        // The pointer over a value moves the cursor.
        let mut s = opened(Fid::Effort, &env);
        s.hover(Some(Target::Item(4)));
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        assert_eq!(list.at, 4);
    }

    #[test]
    fn a_member_at_work_is_asked_under_the_values() {
        let env = Fake::restarting(&["dev"]);
        let mut s = opened(Fid::Effort, &env);
        s.states.insert("dev".into(), (State::Working, 1));
        s.states_known = true;
        let before = size(&s, &look(), false).unwrap();
        s.key(Key::Down, &env);
        assert!(s.key(Key::Enter, &env).is_empty());
        let rows = text(&s, false);
        // The values still there, the question under them, the answers with their keys.
        assert!(rows.iter().any(|r| r.contains(&crate::look::effort_name("high"))), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains(&t!("Relancer « dev » ?", "Restart \"dev\"?"))), "{rows:#?}");
        // One note, not the menu's two.
        let note = t!("Il est au travail", "It is at work");
        assert_eq!(rows.iter().filter(|r| r.contains(&note) || r.contains("interrup")).count().min(1), 1);
        assert!(
            !rows
                .iter()
                .any(|r| r.contains(&t!("repartira sur sa conversation.", "starts again on its conversation.")))
        );
        let buttons = rows.iter().find(|r| r.contains(&t!("Relancer (r)", "Restart (r)"))).expect("the buttons");
        assert!(buttons.contains(&t!("Annuler (a)", "Cancel (c)")), "{buttons}");
        assert!(size(&s, &look(), false).unwrap().0 > before.0, "the pane grows");
        // « Cancel » is the answer at first; its key ends the list without a write.
        let cancel = t!("a", "c").chars().next().unwrap();
        assert_eq!(s.key(Key::Char(cancel), &env), [Effect::Quit]);
        // The other key writes.
        let mut s = opened(Fid::Effort, &env);
        s.states.insert("dev".into(), (State::Working, 1));
        s.states_known = true;
        s.key(Key::Down, &env);
        s.key(Key::Enter, &env);
        let effects = s.key(Key::Char('r'), &env);
        assert!(matches!(&effects[..], [Effect::Apply { .. }]), "{effects:?}");
    }

    #[test]
    fn a_refused_write_keeps_the_list_and_says_why() {
        let env = Fake::quiet();
        let mut s = opened(Fid::Effort, &env);
        let before = size(&s, &look(), false).unwrap();
        s.key(Key::Down, &env);
        assert!(matches!(&s.key(Key::Enter, &env)[..], [Effect::Apply { .. }]));
        s.said = Some(super::super::sheet::Said::Failed("settings.toml : plus de place".into()));
        s.reopen_list(&env);
        assert!(matches!(s.overlay, Overlay::List(_)));
        let rows = text(&s, false);
        assert!(rows.last().unwrap().contains("✗") || rows[rows.len() - 2].contains("✗"), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("plus de place")));
        assert!(size(&s, &look(), false).unwrap().0 > before.0);
    }

    #[test]
    fn another_model_is_typed_in_place() {
        let env = Fake::quiet();
        let mut s = opened(Fid::Model, &env);
        // « Other… » is the last value.
        s.key(Key::End, &env);
        assert!(s.key(Key::Enter, &env).is_empty());
        for c in "opus[1m]".chars() {
            s.key(Key::Char(c), &env);
        }
        let rows = text(&s, false);
        assert!(rows.iter().any(|r| r.contains("› opus[1m]")), "{rows:#?}");
        assert_eq!(s.key(Key::Esc, &env), [Effect::Quit], "given up, it ends");
    }

    #[test]
    fn a_narrow_screen_drops_where_the_default_comes_from() {
        let s = opened(Fid::Model, &Fake::quiet());
        let full = text(&s, false);
        let short = text(&s, true);
        let origin = t!("équipe", "team");
        assert!(full[1].contains(&format!("({origin})")), "{}", full[1]);
        assert!(
            !short[1].contains(&origin) && short[1].contains(&t!("défaut : Opus", "default: Opus")),
            "{}",
            short[1]
        );
        let (a, b) = widths(&s, &look()).unwrap();
        assert!(b < a);
    }

    #[test]
    fn short_of_height_the_values_far_from_the_cursor_go() {
        let env = Fake::restarting(&["dev"]);
        let mut s = opened(Fid::Effort, &env);
        s.states.insert("dev".into(), (State::Working, 1));
        s.states_known = true;
        let (wanted, cols) = size(&s, &look(), false).unwrap();
        // Cut to 5 rows with the cursor in the middle: its row stays, and the frame.
        let drawn = draw(&s, &look(), false, cols, 5);
        let rows: Vec<String> = (0..5).map(|y| drawn.canvas.row(y)).collect();
        assert!(rows[0].starts_with('╭') && rows[4].starts_with('╰'), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains('›')), "the cursor is there: {rows:#?}");
        assert!(wanted > 5);
        // Under a question, the question and its buttons stay whatever the height.
        s.key(Key::Down, &env);
        s.key(Key::Down, &env);
        s.key(Key::Enter, &env);
        let (wanted, cols) = size(&s, &look(), false).unwrap();
        let height = wanted - 3;
        let drawn = draw(&s, &look(), false, cols, height);
        let rows: Vec<String> = (0..height).map(|y| drawn.canvas.row(y)).collect();
        assert!(rows.last().unwrap().starts_with('╰'), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains(&t!("Relancer (r)", "Restart (r)"))), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains('›')), "{rows:#?}");
    }

    #[test]
    fn a_refusal_keeps_its_reason_and_the_cursor() {
        let env = Fake::quiet();
        let mut s = opened(Fid::Effort, &env);
        for _ in 0..3 {
            s.key(Key::Down, &env);
        }
        assert!(matches!(&s.key(Key::Enter, &env)[..], [Effect::Apply { .. }]));
        let error = "écrire /Users/ronan/Developer/gitea/un-projet-au-nom-bien-long/.recruit/settings.local.toml : Permission denied (os error 13)";
        s.said = Some(super::super::sheet::Said::Failed(error.into()));
        s.reopen_list(&env);
        // The cursor on the value chosen, not back on the one in place.
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        assert_eq!(list.at, 3);
        let rows = text(&s, false);
        let last: String = rows[rows.len() - 4..rows.len() - 1].join(" ");
        assert!(last.contains("Permission denied (os error 13)"), "the reason, whole, at the end: {rows:#?}");
        assert!(!last.contains("/Users/ronan/Developer"), "the path cut from its start: {last}");
        assert!(last.contains("…") && last.contains("settings.local.toml"), "{last}");
        assert!(rows.iter().all(|r| columns(r) <= size(&s, &look(), false).unwrap().1));
    }

    /// In a light terminal, the cursor's line is on the light palette's selection (253).
    #[test]
    fn the_cursor_line_in_the_light_palette() {
        let s = opened(Fid::Effort, &Fake::quiet());
        let (rows, cols) = size(&s, &look(), false).unwrap();
        crate::look::set_light(true);
        let drawn = draw(&s, &look(), false, cols, rows);
        crate::look::set_light(false);
        assert_eq!(drawn.canvas.style(3, 1).bg, Some(Color::AnsiValue(253)));
    }

    /// The cursor's line is on its background from edge to edge inside the frame, its text in bold included.
    #[test]
    fn the_cursor_line_is_on_its_background_whole() {
        let mut s = opened(Fid::Effort, &Fake::quiet());
        s.key(Key::Down, &Fake::quiet());
        let (rows, cols) = size(&s, &look(), false).unwrap();
        let drawn = draw(&s, &look(), false, cols, rows);
        // The second value (« low »), the cursor on it.
        for y in [2] {
            for x in 1..cols - 1 {
                assert_eq!(drawn.canvas.style(x, y).bg, Some(selected()), "column {x}: {}", drawn.canvas.row(y));
            }
            assert!(drawn.canvas.row(y).contains(&crate::look::effort_name("low")));
        }
        // The line above it, not.
        assert!((1..cols - 1).all(|x| drawn.canvas.style(x, 1).bg.is_none()));
    }
}
