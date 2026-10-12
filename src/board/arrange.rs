// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! How the dashboard's cards are arranged in the room a pane leaves: the form of each, the lists of names, and the
//! zones of a click.

use crossterm::style::Color;

use crate::t;
use crate::ui::canvas::{columns, fit};
use crate::ui::look::State;

use super::card::*;
use super::paint::*;
use super::zones::*;
use super::*;

/// Narrower than this, cards go one per line.
const MIN_CARD: usize = 30;

/// `au repos  a · b · c`, wrapped under its label over at most `max` lines, each name a zone; the names left over
/// counted at the end, `… et 4 autres`.
pub(super) fn names(label: &str, list: &[String], width: usize, color: Color, max: usize) -> Drawn {
    if list.is_empty() || max == 0 {
        return Drawn::default();
    }
    const INDENT: usize = 10;
    // Each line's names: the member, as shown, from which column.
    let mut rows: Vec<Vec<(&String, String, usize)>> = vec![Vec::new()];
    let mut used = INDENT;
    for name in list {
        // Alone on its line and still too wide: cut, the zone still names the member.
        let shown = fit(name, width.saturating_sub(INDENT).max(1));
        let len = columns(&shown);
        if rows.last().is_some_and(|row| !row.is_empty()) && used + 3 + len > width {
            rows.push(Vec::new());
            used = INDENT;
        }
        let row = rows.last_mut().expect("one row at least");
        if !row.is_empty() {
            used += 3;
        }
        row.push((name, shown, used));
        used += len;
    }
    let mut left = 0;
    if rows.len() > max {
        left = rows[max..].iter().map(Vec::len).sum();
        rows.truncate(max);
        let last = rows.last_mut().expect("one row at least");
        // Room for the count after the last name kept.
        while let Some((_, shown, col)) = last.last() {
            if col + columns(shown) + 1 + columns(&others(left)) <= width {
                break;
            }
            last.pop();
            left += 1;
        }
    }
    let mut drawn = Drawn::default();
    for (i, row) in rows.iter().enumerate() {
        let head = if i == 0 { format!("{label:<INDENT$}") } else { " ".repeat(INDENT) };
        let shown: Vec<&str> = row.iter().map(|(_, shown, _)| shown.as_str()).collect();
        let mut line = Line::of(head, Paint::Dim);
        line.push(shown.join(" · "), Paint::Color(color));
        if left > 0 && i + 1 == rows.len() {
            line.push(if row.is_empty() { "" } else { " " }, Paint::Plain);
            line.push(others(left), Paint::Dim);
        }
        drawn.lines.push(line);
        drawn.zones.extend(row.iter().map(|(name, shown, col)| Zone {
            member: name.to_string(),
            kind: ZoneKind::Member,
            row: i,
            col: *col,
            rows: 1,
            cols: columns(shown),
        }));
    }
    drawn
}

/// `… et 4 autres`: names left out of a list.
pub(super) fn others(count: usize) -> String {
    if count == 1 { t!("… et 1 autre", "… and 1 more") } else { t!("… et {} autres", "… and {} more", count) }
}

/// ` nom en double ailleurs : coordinateur (shop)`, dimmed: sessions open elsewhere under a member's name, which
/// messages by name could reach.
pub(super) fn elsewhere_line(list: &[(String, Option<String>)], width: usize) -> Option<Line> {
    if list.is_empty() {
        return None;
    }
    let names: Vec<String> = list
        .iter()
        .map(|(name, folder)| match folder {
            Some(folder) => format!("{name} ({folder})"),
            None => name.clone(),
        })
        .collect();
    let names = names.join(", ");
    let text = if list.len() == 1 {
        t!(" nom en double ailleurs : {}", " name in use elsewhere: {}", names)
    } else {
        t!(" noms en double ailleurs : {}", " names in use elsewhere: {}", names)
    };
    Some(Line::of(fit(&text, width), Paint::Dim))
}

/// The working agents at rest that have no card: `au repos  a · b · c`, after an empty line; on one line, those that
/// hold and how many more, unless `whole`.
pub(super) fn resting_names(list: &[&Card], width: usize, whole: bool) -> Drawn {
    let list: Vec<String> = list.iter().map(|c| c.data.name.clone()).collect();
    names(&t!("au repos", "idle"), &list, width, State::Idle.color(), if whole { usize::MAX } else { 1 }).spaced()
}

/// The columns cards go in, side by side: the right one a column wider on an odd width.
fn column_widths(width: usize, columns: usize) -> Vec<usize> {
    if columns == 1 { vec![width] } else { vec![width / 2, width - width / 2] }
}

/// The lines a card `width` wide takes: its title and its curve, and in between what the member does over at most
/// `lines` lines.
fn card_height(c: &Card, width: usize, lines: usize) -> usize {
    2 + if lines == 0 { 0 } else { doing_lines(&c.data, width, lines).len() }
}

/// The dashboard's parts, in order: the contacts, the working agents at work or waiting for the user, those at rest.
pub(super) fn section(c: &Card) -> usize {
    if c.contact {
        0
    } else if resting(&c.data) {
        2
    } else {
        1
    }
}

/// The cards in the order the dashboard shows them: the contacts; the agents waiting for the user, then those at
/// work; then those at rest, the latest first. Otherwise, as they come (the file's order, and for members at rest
/// since the same second, the latest first: see [`Memory::board`]).
pub(super) fn ordered(cards: &[Card]) -> Vec<&Card> {
    let mut list: Vec<&Card> = cards.iter().collect();
    list.sort_by_key(|c| match section(c) {
        // Waiting for the user, then failed, then at work.
        1 => (
            1,
            match c.data.state {
                State::Waiting => 0,
                State::Error => 1,
                _ => 2,
            },
        ),
        2 => (2, c.data.since),
        s => (s, 0),
    });
    list
}

/// How a card is drawn: across the pane or beside another, with what the member does over at most so many lines; or,
/// for a working agent at rest, only its name in a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Form {
    Card { half: bool, lines: usize },
    Named,
}

/// A card's forms as room runs out: what the member does over two lines; then over one line; then beside another, when
/// two hold side by side; then without what it does; a working agent at rest ends in the list of names.
pub(super) fn forms(c: &Card, two: bool) -> Vec<Form> {
    let card = |half, lines| Form::Card { half, lines };
    let mut forms = vec![card(false, 2), card(false, 1)];
    if two {
        forms.extend([card(true, 1), card(true, 0)]);
    } else {
        forms.push(card(false, 0));
    }
    if !c.contact && resting(&c.data) {
        forms.push(Form::Named);
    }
    forms
}

/// Where the cards go: rows of one card across the pane or two side by side, each card with the lines it gives what
/// the member does; the working agents at rest only named, all of them or on one line; how many cards found no room
/// at all.
#[derive(Debug, Default)]
pub(super) struct Layout<'a> {
    pub(super) rows: Vec<Vec<(&'a Card, usize)>>,
    pub(super) named: Vec<&'a Card>,
    pub(super) whole: bool,
    pub(super) more: usize,
}

/// How the working agents at rest without a card show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Names {
    /// All of them, over as many lines as it takes.
    Whole,
    /// Those that hold on one line, and how many more.
    Line,
    /// Only counted, with the cards that found no room.
    Counted,
}

/// A card of the list by its place in it, with the lines it gives what the member does.
type Placed = (usize, usize);

/// The cards of `list`, in its order, over `room` lines of a pane `width` wide, each in its own form. `list` goes from
/// what matters most to what matters least: the contacts, the agents waiting for the user, at work, at rest from the
/// latest. In that order, each card takes the richest form that still leaves room for all those after it in their
/// barest one: across the pane with what the member does over two lines, then one; then beside the next card when two
/// hold side by side, then without what it does; a working agent at rest, at last, only named. Those named go on one
/// line while the cards choose; all of them if they hold once the cards have. A card with nothing to say of what the
/// member does only goes across the pane after that, with the room left. Even all in their barest form too many: the
/// first cards that hold, the agents at rest only counted with the others while the cards choose, then named if they
/// still hold. The agents the sessions started have their own cards under the members, in the room left (see
/// [`helper_block`]): they never take what the members do.
///
/// The same cards always get the same layout: it changes only with them, their states or the pane.
pub(super) fn arrange<'a>(list: &[&'a Card], width: usize, room: usize) -> Layout<'a> {
    let two = width >= 2 * MIN_CARD;
    let ladders: Vec<Vec<Form>> = list.iter().map(|c| forms(c, two)).collect();
    // Each card's height across the pane and in the narrower column, for each number of lines, measured once.
    let half = column_widths(width, 2)[0];
    let heights: Vec<[[usize; 3]; 2]> =
        list.iter().map(|c| [width, half].map(|w| [0, 1, 2].map(|lines| card_height(c, w, lines)))).collect();
    let tall = |i: usize, half: bool, lines: usize| heights[i][usize::from(half)][lines];

    // The layout for each card's form, the first `shown` cards only; and the lines it takes.
    let build = |levels: &[usize], shown: usize, names: Names| -> (Layout<'a>, usize) {
        let mut layout = Layout { whole: names == Names::Whole, ..Default::default() };
        let mut rows: Vec<(Vec<Placed>, usize)> = Vec::new();
        let mut placed = 0;
        // Cards side by side, two by two in their order; one left over goes across.
        let mut run: Vec<Placed> = Vec::new();
        let pair = |run: &mut Vec<Placed>, rows: &mut Vec<(Vec<Placed>, usize)>| {
            for two in run.chunks(2) {
                let height = match two {
                    [(i, lines)] => tall(*i, false, *lines),
                    _ => two.iter().map(|&(i, lines)| tall(i, true, lines)).max().unwrap_or(0),
                };
                rows.push((two.to_vec(), height));
            }
            run.clear();
        };
        for (i, c) in list.iter().enumerate() {
            if run.first().is_some_and(|&(j, ..)| section(list[j]) != section(c)) {
                pair(&mut run, &mut rows);
            }
            match ladders[i][levels[i]] {
                Form::Named if names == Names::Counted => layout.more += 1,
                Form::Named => layout.named.push(c),
                Form::Card { .. } if placed == shown => layout.more += 1,
                Form::Card { half, lines } => {
                    placed += 1;
                    if half {
                        run.push((i, lines));
                    } else {
                        pair(&mut run, &mut rows);
                        rows.push((vec![(i, lines)], tall(i, false, lines)));
                    }
                }
            }
        }
        pair(&mut run, &mut rows);
        let mut height = rows.iter().map(|(_, h)| h + 1).sum::<usize>().saturating_sub(1);
        height += usize::from(layout.more > 0) + resting_names(&layout.named, width, layout.whole).len();
        layout.rows =
            rows.into_iter().map(|(row, _)| row.into_iter().map(|(i, lines)| (list[i], lines)).collect()).collect();
        (layout, height)
    };
    let fits = |levels: &[usize], shown: usize, names: Names| build(levels, shown, names).1 <= room;

    let barest: Vec<usize> = ladders.iter().map(|forms| forms.len() - 1).collect();
    let mut levels = barest.clone();
    let all = list.len();
    let cut = !fits(&levels, all, Names::Line);
    // Cut short, as many cards as hold, the names only counted with the others while the cards choose.
    let (shown, names) = if cut {
        match (0..all).rev().find(|&shown| fits(&levels, shown, Names::Counted)) {
            Some(shown) => (shown, Names::Counted),
            // Not even how many: nothing.
            None => return Layout::default(),
        }
    } else {
        (all, Names::Line)
    };
    // Each card in turn takes the richest form that holds, from `from`.
    let choose = |levels: &mut Vec<usize>, names: Names, from: &dyn Fn(usize) -> usize| {
        // Cut short, the agents at rest stay named: a card would go past the cards shown.
        for i in (0..all).filter(|&i| !cut || section(list[i]) != 2) {
            let was = levels[i];
            levels[i] = (from(i)..was)
                .find(|&form| {
                    levels[i] = form;
                    fits(levels, shown, names)
                })
                .unwrap_or(was);
        }
    };
    // What the members do first. Across the pane, a card that says nothing more than beside another: it waits for the
    // names.
    let plain = |i: usize| list[i].data.doing.is_none() && list[i].data.failed.is_none();
    let beside = |i: usize| ladders[i].iter().position(|f| matches!(f, Form::Card { half: true, .. }));
    choose(&mut levels, names, &|i| if plain(i) { beside(i).unwrap_or(0) } else { 0 });
    // Then the names: all of them if they hold, else on one line, else only counted.
    let names = [Names::Whole, Names::Line].into_iter().find(|&names| fits(&levels, shown, names)).unwrap_or(names);
    // The room left, to the cards that say nothing more across the pane.
    choose(&mut levels, names, &|_| 0);
    build(&levels, shown, names).0
}

/// Rows of cards, one across the pane or two side by side, an empty line between two rows, each as tall as its tallest
/// card; each card a zone, and the context of a member at rest another, to compact it. The model, the effort and the
/// time in columns from one card to the next of the same width.
pub(super) fn cards(rows: &[Vec<(&Card, usize)>], width: usize, b: &Board) -> Drawn {
    let placed: Vec<Vec<(&Card, usize, usize)>> = rows
        .iter()
        .map(|row| {
            let widths = column_widths(width, row.len());
            row.iter().zip(widths).map(|(&(c, lines), w)| (c, w, lines)).collect()
        })
        .collect();
    let right =
        |n: usize| Right::of(placed.iter().filter(|row| row.len() == n).flatten().map(|&(c, w, _)| (&c.data, w)));
    let rights = [right(1), right(2)];
    let mut drawn = Drawn::default();
    for (i, row) in placed.iter().enumerate() {
        if i > 0 {
            drawn.lines.push(Line::default());
        }
        let top = drawn.len();
        let right = &rights[row.len() - 1];
        let row: Vec<(&Card, usize, Vec<Line>)> =
            row.iter().map(|&(c, w, lines)| (c, w, card(&c.data, w, lines, right, b))).collect();
        let mut col = 0;
        for (c, w, card) in &row {
            let zone = |kind, row, col, rows, cols| Zone { member: c.data.name.clone(), kind, row, col, rows, cols };
            drawn.zones.push(zone(ZoneKind::Member, top, col, card.len(), *w));
            // A card too narrow for its context shows none of it: nothing to compact there.
            if let Some(cols) = compaction_cells(&c.data).filter(|cols| cols < w)
                && let Some(last) = card.len().checked_sub(1)
            {
                drawn.zones.push(zone(ZoneKind::Compact, top + last, col + w - 1 - cols, 1, cols));
            }
            col += w;
        }
        let height = row.iter().map(|(.., card)| card.len()).max().unwrap_or(0);
        for i in 0..height {
            let mut line = Line::default();
            for (j, (_, w, card)) in row.iter().enumerate() {
                match card.get(i) {
                    Some(text) => line.append(text.clone()),
                    // Under a shorter card, room for the one on its right.
                    None if j + 1 < row.len() => line.push(" ".repeat(*w), Paint::Plain),
                    None => {}
                }
            }
            drawn.lines.push(line);
        }
    }
    drawn
}

#[cfg(test)]
mod tests {

    use crossterm::style::Color;

    use crate::claude::bridge::Task;

    use crate::t;
    use crate::ui::look::{self, State};

    use super::*;

    use crate::board::dashboard::*;
    use crate::board::fixture::*;

    use crate::board::Board;

    fn checked(b: &Board, width: usize, height: usize) -> Vec<String> {
        let drawn = padded(b, width, height);
        zones_match(&drawn);
        let lines: Vec<String> = drawn.lines.iter().map(Line::text).collect();
        assert!(lines.len() == height - 1 && lines.iter().all(|l| l.chars().count() <= width), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.trim_end().ends_with('┃')), "{lines:#?}");
        lines
    }

    #[test]
    fn capture_at_several_heights() {
        let at = |height: usize| checked(&board(capture()), 60, height);
        let gpu = ["┃   Je profile le rendu des tuiles et je corrige la fuite", "┃   du cache de textures"];
        // Those who just finished: across the pane, what they did whole.
        let finished = |lines: &[String]| {
            assert_eq!(task(lines, "designer"), ["┃   ✓ Maquette des cartes livrée"], "{lines:#?}");
            assert_eq!(task(lines, "reviewer"), ["┃   ✓ Relecture de la PR 42 terminée"], "{lines:#?}");
        };

        // The contact, the two at work across the pane, their task over two lines, uncut; then those at rest, the
        // latest first; the three oldest only named.
        let lines = at(44);
        let order = ["coordinateur", "gpu-engine", "dev-back", "designer", "reviewer", "devops", "sécurité", "ui-ux"];
        let tops: Vec<usize> = order.iter().map(|name| title(&lines, name).unwrap()).collect();
        assert!(tops.windows(2).all(|w| w[0] <= w[1]) && tops[0] == 2, "{lines:#?}");
        assert_eq!(task(&lines, "gpu-engine"), gpu);
        finished(&lines);
        // Beside one another as they come, the same height or not.
        assert_eq!(title(&lines, "performance"), title(&lines, "ui-ux"));
        assert_eq!(task(&lines, "ui-ux"), ["┃   ✓ Écrans relus"]);
        assert_eq!(named(&lines), ["opérateur-ui-ux", "opérateur", "planificateur"]);

        // Less room: fewer cards at rest, those who just finished keep theirs whole.
        for height in [38, 34, 30] {
            let lines = at(height);
            assert_eq!(task(&lines, "gpu-engine"), gpu);
            finished(&lines);
        }
        // The next ones side by side, what they did cut rather than left out.
        let lines = at(34);
        assert_eq!(title(&lines, "devops"), title(&lines, "traitement-cartes"));
        assert_eq!(task(&lines, "traitement-cartes"), ["┃   ✓ Cartes découpées en tu…"]);
        assert_eq!(named(&lines).len(), 6);
        // Named on one line, the others counted, rather than a card less.
        let lines = at(30);
        // As many names as the line holds, in either language: the others make eight.
        let shown = named(&lines);
        assert_eq!(shown[..2], ["devops", "traitement-cartes"]);
        let line = format!("{:<10}{} {}", t!("au repos", "idle"), shown.join(" · "), others(8 - shown.len()));
        assert!(lines.contains(&line), "{lines:#?}");

        // Those who just finished side by side, without what they did, before the ones at work give anything up.
        let lines = at(24);
        assert_eq!(task(&lines, "gpu-engine"), gpu);
        let pair = &lines[title(&lines, "designer").unwrap()];
        assert!(pair.contains(" reviewer ") && task(&lines, "designer").is_empty(), "{lines:#?}");

        // Then the ones at work side by side, their task on one line; the contact still across the pane.
        let lines = at(18);
        let pair = &lines[title(&lines, "gpu-engine").unwrap()];
        assert!(pair.contains(" dev-back ") && pair.matches('╻').count() == 2, "{lines:#?}");
        assert_eq!(task(&lines, "coordinateur"), ["┃   ✓ Tâches réparties"]);

        // At last, the contact alone, what it did whole, and how many more.
        let lines = at(12);
        assert_eq!(task(&lines, "coordinateur"), ["┃   ✓ Tâches réparties"]);
        assert!(lines.contains(&t!("… et {} de plus", "… and {} more", 12)), "{lines:#?}");
        assert!(named(&lines).is_empty());
    }

    #[test]
    fn a_contact_across_rather_than_two_cut() {
        // Two contacts, 6 lines for them: the one at work across the pane, its task on one line; the other without.
        let cards = vec![
            sample_card("coordinateur", true, State::Working).tweak(|c| {
                c.data.doing = Some(Task {
                    now: "Je relis la maquette des cartes et je corrige les colonnes de droite".into(),
                    done: None,
                });
            }),
            sample_card("architecte", true, State::Idle).tweak(|c| {
                c.data.doing = Some(Task { now: String::new(), done: Some("Plan relu".into()) });
            }),
        ];
        let lines = checked(&Board { absent: Vec::new(), ..board(cards) }, 60, 11);
        assert!(lines.iter().all(|l| l.matches('╻').count() <= 1), "{lines:#?}");
        assert_eq!(task(&lines, "coordinateur"), ["┃   Je relis la maquette des cartes et je corrige les colo…"]);
        assert!(task(&lines, "architecte").is_empty());
        assert_eq!(lines[2..8].iter().filter(|l| l.is_empty()).count(), 1, "{lines:#?}");
    }

    #[test]
    fn too_low_a_pane() {
        // Two rows more than with the header it had: from six, « … et 13 de plus » holds.
        for height in 0..=5 {
            let lines = render(&board(capture()), 60, height).lines;
            assert!(lines.len() <= height.saturating_sub(1), "{height}: {lines:#?}");
            assert!(!lines.iter().any(|l| visible(l).starts_with('…')), "{height}: {lines:#?}");
        }
        // The usage at the bottom only.
        let lines: Vec<String> = render(&board(capture()), 60, 3).lines.iter().map(Line::text).collect();
        assert!(lines[0].is_empty() && lines[1].contains("41 % ↻ 1h22") && lines.len() == 2, "{lines:#?}");
        assert!(render(&board(capture()), 60, 3).zones.is_empty());
        // Room for how many more only.
        let lines: Vec<String> =
            padded(&Board { absent: Vec::new(), ..board(capture()) }, 60, 6).lines.iter().map(Line::text).collect();
        assert_eq!(lines[2], t!("… et {} de plus", "… and {} more", 13));
    }

    #[test]
    fn names_on_one_line() {
        let list: Vec<String> =
            ["devops", "traitement-cartes", "sécurité", "performance", "ui-ux"].map(String::from).into();
        let drawn = names("au repos", &list, 50, Color::DarkGrey, 1);
        let lines: Vec<String> = drawn.lines.iter().map(Line::text).collect();
        assert_eq!(lines, [format!("au repos  devops · traitement-cartes {}", others(3))]);
        // The names shown are zones, the others are not.
        zones_match(&drawn);
        assert_eq!(drawn.zones.iter().map(|z| z.member.as_str()).collect::<Vec<_>>(), ["devops", "traitement-cartes"]);
        // One left over; none that holds.
        let lines = |width| {
            names("au repos", &list[..2], width, Color::DarkGrey, 1).lines.iter().map(Line::text).collect::<Vec<_>>()
        };
        assert_eq!(lines(30), [format!("au repos  devops {}", others(1))]);
        assert_eq!(lines(20), [format!("au repos  {}", others(2))]);
        // As many lines as it takes.
        assert_eq!(names("au repos", &list, 50, Color::DarkGrey, usize::MAX).len(), 2);
    }

    #[test]
    fn same_layout_from_one_second_to_the_next() {
        let later = |mut b: Board| {
            b.now += 1;
            b.cards.iter_mut().for_each(|c| c.data.since += 1);
            b
        };
        for height in [60, 44, 36, 30, 24, 18, 12] {
            let now = render(&board(capture()), 60, height);
            assert_eq!(now.zones, render(&later(board(capture())), 60, height).zones, "{height}");
        }
        // Two at rest since the same second: as they come, the latest first (see `Memory::board`).
        let cards = vec![
            sample_card("b", false, State::Idle).tweak(|c| {
                c.data.since = 30;
            }),
            sample_card("a", false, State::Idle).tweak(|c| {
                c.data.since = 30;
            }),
            sample_card("c", false, State::Idle).tweak(|c| {
                c.data.since = 10;
            }),
            sample_card("w", false, State::Working),
            sample_card("v", false, State::Waiting),
            sample_card("lead", true, State::Idle),
        ];
        let names: Vec<&str> = ordered(&cards).iter().map(|c| c.data.name.as_str()).collect();
        assert_eq!(names, ["lead", "v", "w", "c", "b", "a"]);
    }

    #[test]
    fn small_team_full_width() {
        let b = board(vec![
            sample_card("coordinateur", true, State::Working),
            sample_card("lanceur", false, State::Idle),
            sample_card("interface", false, State::Waiting),
        ]);
        let lines: Vec<String> = padded(&b, 70, 23).lines.iter().map(Line::text).collect();
        assert_eq!(lines.len(), 22);
        assert!(lines.iter().all(|l| l.chars().count() <= 70), "{lines:#?}");
        let tops: Vec<&String> = lines.iter().filter(|l| l.starts_with('╻')).collect();
        assert_eq!(tops.len(), 3);
        assert!(tops.iter().all(|l| l.chars().count() == 70));
        // The cards right under the header, on two lines, an empty one between two: the contact, the agent waiting
        // for the user, the one at rest; the usage at the bottom.
        assert!(
            lines[2].starts_with("╻ ⠋ coordinateur ")
                && lines[2].ends_with(&format!(" Opus {}   1m ", look::effort_label("xhigh")))
        );
        assert!(lines[3].starts_with("╹   ") && lines[3].ends_with("   41 % "));
        assert!(lines[4].is_empty() && lines[5].starts_with("╻ ⚑ interface "));
        assert!(lines[7].is_empty() && lines[8].starts_with("╻ ◷ lanceur "));
        // At rest, the context offers to compact.
        assert!(lines[9].ends_with(" ⟳ 41 % "));
        assert!(lines[21].contains("41 % ↻ 1h22") && lines[20].is_empty());
        assert!(lines.iter().any(|l| l.contains("ops")));
    }

    #[test]
    fn forms_given_up_in_turn() {
        // The contact at work over two lines, the agent at rest with its task done: 15 lines leave them 8.
        let at = |height: usize| -> Vec<String> {
            padded(&board(long_and_short()), 70, height).lines.iter().map(Line::text).collect()
        };
        let middles = |lines: &[String], name: &str| {
            let top = lines.iter().position(|l| l.contains(&format!(" {name} "))).unwrap();
            lines[top + 1..].iter().take_while(|l| l.starts_with('┃')).count()
        };
        let named =
            |lines: &[String]| lines.iter().any(|l| l.starts_with(&t!("au repos", "idle")) && l.contains("dev"));
        let cut = |lines: &[String]| lines.iter().any(|l| l.starts_with('┃') && l.trim_end().ends_with('…'));
        let lines = at(15);
        assert!(middles(&lines, "coordinateur") == 2 && middles(&lines, "dev") == 1 && !cut(&lines), "{lines:#?}");
        // The agent at rest first: without its task, then only named.
        let lines = at(14);
        assert!(middles(&lines, "coordinateur") == 2 && middles(&lines, "dev") == 0, "{lines:#?}");
        let lines = at(13);
        assert!(middles(&lines, "coordinateur") == 2 && named(&lines) && !cut(&lines), "{lines:#?}");
        // Then the contact: its task on one line, cut, then without it.
        let lines = at(12);
        assert!(middles(&lines, "coordinateur") == 1 && named(&lines) && cut(&lines), "{lines:#?}");
        let lines = at(11);
        assert!(middles(&lines, "coordinateur") == 0 && named(&lines), "{lines:#?}");
        // At last, rather than no card, no names: only how many more.
        let lines = at(10);
        assert!(middles(&lines, "coordinateur") == 0 && !named(&lines), "{lines:#?}");
        assert!(lines.iter().any(|l| l == &t!("… et {} de plus", "… and {} more", 1)), "{lines:#?}");
    }

    #[test]
    fn big_team_two_columns_then_names() {
        let mut cards =
            vec![sample_card("coordinateur", true, State::Idle), sample_card("archi", true, State::Working)];
        cards.extend((0..4).map(|i| sample_card(&format!("w{i}"), false, State::Working)));
        cards.extend((0..9).map(|i| sample_card(&format!("repos{i}"), false, State::Idle)));
        let lines: Vec<String> = padded(&board(cards.clone()), 70, 23).lines.iter().map(Line::text).collect();
        assert!(lines.iter().all(|l| l.chars().count() <= 70), "{lines:#?}");
        // Nothing to say of what they do: two side by side rather than a name less. A contact at rest keeps its card,
        // the latest agents at rest too; the others are only named.
        assert!(lines[2].contains("╻ ◷ coordinateur ") && lines[2].contains("╻ ⠋ archi "), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("╻ ◷ repos0 ") && l.contains("╻ ◷ repos1 ")));
        assert_eq!(named(&lines), ["repos4", "repos5", "repos6", "repos7", "repos8"]);

        // Twice the height: every one in cards, two side by side.
        let tall: Vec<String> = padded(&board(cards.clone()), 70, 47).lines.iter().map(Line::text).collect();
        assert!(tall.iter().any(|l| l.contains("╻ ◷ repos0 ")));
        // Too narrow for two side by side.
        let narrow: Vec<String> = padded(&board(cards), 50, 47).lines.iter().map(Line::text).collect();
        assert!(narrow.iter().all(|l| l.matches('╻').count() <= 1 && l.chars().count() <= 50));
    }

    #[test]
    fn names_keep_their_column() {
        let b = board(Vec::new());
        let column = |state| {
            let right = Right { model: 4, initial: false, effort: 8, time: 2 };
            let top = visible(&card(&sample_card("archi", false, state).data, 40, 0, &right, &b)[0]);
            assert_eq!(top.chars().count(), 40);
            top.split("archi").next().unwrap().chars().count()
        };
        for state in [State::Working, State::Idle, State::Waiting, State::Other] {
            assert_eq!(column(state), 4);
        }
    }

    /// A list of names in wide characters: each name is as wide as its zone says, and the line holds in the pane.
    #[test]
    fn wide_names_in_a_list() {
        let list: Vec<String> = ["日本語", "开发者前端工程师后端开发者前端工程师后端", "dev"].map(String::from).into();
        let drawn = names("au repos", &list, 30, Color::DarkGrey, usize::MAX);
        assert!(drawn.lines.iter().all(|l| columns(&l.text()) <= 30), "{:?}", drawn.lines);
        let line = |row: usize| drawn.lines[row].text();
        for zone in &drawn.zones {
            let cells: String = line(zone.row)
                .chars()
                .skip_while({
                    let mut used = 0;
                    move |c| {
                        let skip = used < zone.col;
                        used += columns(c.encode_utf8(&mut [0; 4]));
                        skip
                    }
                })
                .collect();
            assert!(cells.starts_with(zone.member.chars().next().unwrap()), "{zone:?} in {:?}", line(zone.row));
        }
        assert_eq!(drawn.zones[0].cols, 6);
    }
}
