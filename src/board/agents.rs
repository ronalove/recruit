// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The agents the members' sessions started (subagents, teammates), as cards of their own under the members.

use crossterm::style::Color;

use crate::claude::bridge::{Helper, HelperStatus};
use crate::t;
use crate::ui::canvas::{columns, fit};
use crate::ui::look::{self, State, duration};

use super::arrange::*;
use super::card::*;
use super::paint::*;
use super::zones::*;
use super::*;

// -- The agents the sessions started, as cards of their own ------------------------------------------------------
//
// Under the members, each kind in a section under a rule with its title (« ♙ teammates · 3 », « ⧗ subagents · 2 »):
// cards of the members' shape, the name in the clear, at the right the member that started them, faded in its color,
// no context. Short of room a section keeps fewer lines, then becomes a list of names, then a count: never at the
// expense of a member. A click on any of them leads to the member that started it.

/// An agent a member's session started, with that member.
pub(super) struct Agent<'a> {
    pub(super) helper: &'a Helper,
    pub(super) member: &'a Card,
}

/// The subagents and the teammates the members started and the dashboard still shows: at work first, then the others
/// from the latest; the ones whose status held too long (ghosts) last.
pub(super) fn agents(b: &Board) -> [Vec<Agent<'_>>; 2] {
    let mut subagents = Vec::new();
    let mut teammates = Vec::new();
    for member in ordered(&b.cards) {
        for helper in &member.helpers {
            let agent = Agent { helper, member };
            if helper.teammate.is_some() { teammates.push(agent) } else { subagents.push(agent) }
        }
    }
    let rank = |a: &Agent| match a.helper.status {
        _ if a.helper.unknown(b.now) => 3,
        HelperStatus::Running => 0,
        HelperStatus::Failed => 1,
        HelperStatus::Idle | HelperStatus::Done => 2,
    };
    for list in [&mut subagents, &mut teammates] {
        list.sort_by_key(|a| (rank(a), std::cmp::Reverse(a.helper.since), a.helper.id.clone()));
    }
    [subagents, teammates]
}

impl Agent<'_> {
    pub(super) fn teammate(&self) -> bool {
        self.helper.teammate.is_some()
    }

    /// The state its card shows: at work, failed in orange, else at rest; unknown for a ghost.
    pub(super) fn state(&self, now: i64) -> State {
        match self.helper.status {
            _ if self.helper.unknown(now) => State::Other,
            HelperStatus::Running => State::Working,
            HelperStatus::Failed => State::Error,
            HelperStatus::Idle | HelperStatus::Done => State::Idle,
        }
    }

    /// Its name on the card: a teammate's, a subagent's type.
    pub(super) fn name(&self) -> String {
        self.helper.teammate.clone().unwrap_or_else(|| self.helper.kind.clone())
    }

    /// What it was asked, for the line under its name.
    pub(super) fn text(&self) -> String {
        match self.helper.description.trim() {
            "" if self.teammate() => self.helper.kind.clone(),
            "" => String::new(),
            text => text.to_string(),
        }
    }

    /// Its sign: the state's for a teammate (the spinner at work, `?` for a ghost), an hourglass for a subagent at
    /// work, ✓ once over; ✗ once failed.
    pub(super) fn sign(&self, b: &Board) -> char {
        let h = self.helper;
        match (self.teammate(), h.status) {
            (true, _) if h.unknown(b.now) => '?',
            (true, HelperStatus::Done) => '✓',
            (true, _) => self.state(b.now).icon(b.glyphs, b.frame),
            (false, HelperStatus::Done) => '✓',
            (false, HelperStatus::Failed) => '✗',
            (false, _) => b.glyphs.hourglass(),
        }
    }

    /// The seconds on its card: a teammate's in its status; a subagent's since it started, until it ended.
    pub(super) fn time(&self, now: i64) -> String {
        let h = self.helper;
        let secs = if self.teammate() {
            now - h.since
        } else {
            (if h.status == HelperStatus::Running { now } else { h.since }) - h.started
        };
        duration(secs.max(0) as u64)
    }

    /// The sign of its kind, for the rule and the right of its card.
    fn kind_sign(&self, b: &Board) -> char {
        if self.teammate() { b.glyphs.person() } else { b.glyphs.hourglass() }
    }

    /// How it paints, as a card does: (sign, name, time).
    pub(super) fn paints(&self, now: i64) -> (Paint, Paint, Paint) {
        let color = self.state(now).color();
        match self.state(now) {
            State::Working => (Paint::Color(color), Paint::Bold(Color::Reset), Paint::Plain),
            State::Error => (Paint::Bold(color), Paint::Bold(Color::Reset), Paint::Color(color)),
            _ => (Paint::Dim, Paint::Dim, Paint::Dim),
        }
    }
}

/// An agent's card, `width` wide: its title, what it was asked when `text`, its curve; the member's shape, with its
/// time `time` wide. The curve is made of the one change of status it knows of.
pub(super) fn agent_card(a: &Agent, width: usize, text: bool, time: usize, b: &Board) -> Vec<Line> {
    let state = a.state(b.now);
    let color = state.color();
    let height = 2 + usize::from(text);
    let line = if state == State::Error { Paint::Bold(color) } else { Paint::Color(color) };
    let bar = |row: usize| (look::stroke(row, height).to_string(), line);
    let (sign, name, time_paint) = a.paints(b.now);
    let room = width.saturating_sub(3);
    let at = a.time(b.now);
    // At the right, the member that started it: `♙ dev-interface`, else the sign alone, else nothing.
    let member = a.member.data.name.as_str();
    let tail = 2 + time;
    let full = format!("{} {member}", a.kind_sign(b));
    let name_room = |extra: usize| room.saturating_sub(2 + extra + tail);
    let shown = fit(&a.name(), name_room(0).max(1));
    let right = [full, a.kind_sign(b).to_string(), String::new()]
        .into_iter()
        .find(|r| 2 + columns(&shown) + columns(r) + usize::from(!r.is_empty()) + tail <= room)
        .unwrap_or_default();
    let gap = room.saturating_sub(2 + columns(&shown) + columns(&right) + tail);
    let mut middle = Vec::new();
    if !right.is_empty() {
        middle.push((right, Paint::Faint(a.member.data.color)));
        middle.push(("  ".into(), Paint::Plain));
    }
    let time = (format!("{at:>time$}"), time_paint);
    let mut out = vec![title_line(bar(0), (a.sign(b).to_string(), sign), (shown, name), gap, middle, time, width)];
    if text {
        let paint = match state {
            State::Working => Paint::Plain,
            State::Error => Paint::Color(color),
            _ => Paint::Dim,
        };
        let line = vec![bar(1), ("   ".into(), Paint::Plain), (fit(&a.text(), width.saturating_sub(4)), paint)];
        out.push(painted(&line, width));
    }
    let h = a.helper;
    let until = if h.status == HelperStatus::Running { b.now } else { h.since };
    let cells = width.saturating_sub(6 + CONTEXT);
    let bottom = [
        bar(height - 1),
        ("   ".into(), Paint::Plain),
        (work_curve(h.started, &[(h.started, until)], b.now, b.now, cells), Paint::Color(color)),
        (" ".into(), Paint::Plain),
    ];
    out.push(painted(&bottom, width));
    out
}

/// A section's rule and title: `── ♙ teammates · 3 ───────`, dimmed.
pub(super) fn rule(sign: char, title: &str, count: usize, width: usize) -> Line {
    let text = format!(" {sign} {title} · {count} ");
    let rest = width.saturating_sub(1 + columns(&text));
    Line(vec![("─".into(), Paint::Dim), (text, Paint::Dim), ("─".repeat(rest), Paint::Dim)])
}

/// How far a section of `n` agents is shown, from the barest: level 0, only their names on a line; levels `1..=n`, that
/// many cards (the others named under them); levels `n + 1..=2n`, all the cards, that many more with the text of what
/// they were asked. Each level takes more lines than the one before: the room fills card by card.
fn section_rows(level: usize, n: usize) -> usize {
    match level {
        0 => 1,
        // The rule, the cards (2 lines, a blank between), a blank and the names of the others.
        l if l < n => 1 + (3 * l - 1) + 2,
        l if l == n => 1 + (3 * n - 1),
        l => 1 + (3 * n - 1) + (l - n),
    }
}

/// A section's names on one line, `  ♙  ⠹ ux-critic  ⠹ perf  ◷ a11y`, the ones that hold and how many more; each name
/// leads to the member that started it.
pub(super) fn agent_names(list: &[Agent], kind: char, width: usize, b: &Board) -> Drawn {
    let mut line = Line(vec![("  ".into(), Paint::Plain), (kind.to_string(), Paint::Dim), ("  ".into(), Paint::Plain)]);
    let mut col = 2 + 1 + 2;
    let mut zones = Vec::new();
    let mut shown = 0;
    for a in list {
        let (sign, name, _) = a.paints(b.now);
        let len = 2 + columns(&a.name());
        let left = list.len() - shown - 1;
        let more = if left > 0 { columns(&others(left)) + 3 } else { 0 };
        if col + len + more > width && shown > 0 {
            break;
        }
        if shown > 0 {
            line.push("  ", Paint::Plain);
            col += 2;
        }
        line.push(a.sign(b).to_string(), sign);
        line.push(" ", Paint::Plain);
        let label = fit(&a.name(), width.saturating_sub(col + 2).max(1));
        zones.push(Zone {
            member: a.member.data.name.clone(),
            kind: ZoneKind::Member,
            row: 0,
            col,
            rows: 1,
            cols: 2 + columns(&label),
        });
        col += 2 + columns(&label);
        line.push(label, name);
        shown += 1;
    }
    if shown < list.len() {
        line.push("  ", Paint::Plain);
        line.push(others(list.len() - shown), Paint::Dim);
    }
    Drawn { lines: vec![line], zones }
}

/// A section's noun for `n` of them: « teammate », « teammates »; « sous-agent », « sous-agents ».
pub(super) fn noun(teammates: bool, n: usize) -> String {
    match (teammates, n) {
        (true, 1) => t!("teammate", "teammate"),
        (true, _) => t!("teammates", "teammates"),
        (false, 1) => t!("sous-agent", "subagent"),
        (false, _) => t!("sous-agents", "subagents"),
    }
}

/// `  ♙ 3 teammates`: yellow when one works, orange when one failed.
pub(super) fn agent_count(list: &[Agent], kind: char, b: &Board) -> Drawn {
    let paint = if list.iter().any(|a| a.state(b.now) == State::Working) {
        Paint::Color(State::Working.color())
    } else if list.iter().any(|a| a.state(b.now) == State::Error) {
        Paint::Color(State::Error.color())
    } else {
        Paint::Dim
    };
    let teammates = list.first().is_some_and(Agent::teammate);
    Drawn::from(vec![Line::of(format!("  {kind} {} {}", list.len(), noun(teammates, list.len())), paint)])
}

/// A section drawn to `level` (see [`section_rows`]): its rule, then its cards, the names of the others.
fn agent_section(list: &[Agent], level: usize, width: usize, b: &Board) -> Drawn {
    let Some(first) = list.first() else { return Drawn::default() };
    let n = list.len();
    let kind = first.kind_sign(b);
    if level == 0 {
        // Names; a width that holds hardly two of them: a count.
        let names = agent_names(list, kind, width, b);
        let shown = names.zones.len();
        return if shown < 2 && n > 1 { agent_count(list, kind, b) } else { names };
    }
    let (cards, texts) = if level <= n { (level, 0) } else { (n, level - n) };
    let mut drawn = Drawn::from(vec![rule(kind, &noun(first.teammate(), n), n, width)]);
    // The times line up from one card to the next.
    let time = list.iter().map(|a| columns(&a.time(b.now))).max().unwrap_or(0).max(3);
    for (i, a) in list.iter().take(cards).enumerate() {
        if i > 0 {
            drawn.lines.push(Line::default());
        }
        let card = agent_card(a, width, i < texts, time, b);
        drawn.zones.push(Zone {
            member: a.member.data.name.clone(),
            kind: ZoneKind::Member,
            row: drawn.len(),
            col: 0,
            rows: card.len(),
            cols: width,
        });
        drawn.lines.extend(card);
    }
    if cards < n {
        drawn.lines.push(Line::default());
        drawn.append(agent_names(&list[cards..], kind, width, b));
    }
    drawn
}

/// The sections of agents under the members, in `room` lines, an empty line before them and between them: the
/// teammates, which last, then the subagents, which come and go with every tool, at the very end. The teammates matter
/// most. A section that does not hold even as names is left out, the subagents first. Then one chain of states, each a
/// little richer than the one before: the teammates one card at a time, to all their text, then the subagents in the
/// room that is left. The richest state that holds is drawn: so no section ever gets richer as the room shrinks, and
/// the room is filled card by card.
pub(super) fn helper_block(b: &Board, width: usize, room: usize) -> Drawn {
    let [subagents, teammates] = agents(b);
    // In the order they show, and the order that matters: the teammates, which last, then the subagents, which come and
    // go at the very end.
    let sections: Vec<&Vec<Agent>> = [&teammates, &subagents].into_iter().filter(|l| !l.is_empty()).collect();
    if sections.is_empty() || width == 0 {
        return Drawn::default();
    }
    // The lines of the levels chosen, an empty line before the first and between: a section left out has none.
    let rows = |levels: &[Option<usize>]| {
        let held: Vec<usize> =
            levels.iter().zip(&sections).filter_map(|(level, l)| Some(section_rows((*level)?, l.len()))).collect();
        if held.is_empty() { 0 } else { 1 + held.iter().sum::<usize>() + held.len() - 1 }
    };
    // The barest for each in turn, the teammates first, while it holds; those that do not go.
    let mut levels: Vec<Option<usize>> = vec![None; sections.len()];
    for i in 0..sections.len() {
        levels[i] = Some(0);
        if rows(&levels) > room {
            levels[i] = None;
        }
    }
    // The chain: the teammates' levels one by one, then the subagents'.
    let order: Vec<usize> = (0..sections.len()).filter(|&i| levels[i].is_some()).collect();
    'chain: for &i in &order {
        for level in 1..=2 * sections[i].len() {
            levels[i] = Some(level);
            if rows(&levels) > room {
                levels[i] = Some(level - 1);
                break 'chain;
            }
        }
    }
    let mut drawn = Drawn::from(vec![Line::default()]);
    let mut first = true;
    for (list, level) in sections.iter().zip(&levels) {
        let Some(level) = level else { continue };
        if !std::mem::take(&mut first) {
            drawn.lines.push(Line::default());
        }
        drawn.append(agent_section(list, *level, width, b));
    }
    if first { Drawn::default() } else { drawn }
}

#[cfg(test)]
mod tests {

    use crate::claude::bridge::{Helper, HelperStatus};

    use crate::ui::look::State;
    use crate::{claude::bridge, t};

    use super::*;

    use crate::board::dashboard::*;
    use crate::board::fixture::*;

    use crate::board::Card;

    /// The contact at work over two lines, with what its session started: two subagents, one at work and one done,
    /// and a teammate waiting for a message; the agent at rest with its task done.
    fn with_helpers() -> Vec<Card> {
        let helper = |id: &str, kind: &str, description: &str, teammate: Option<&str>, status, started, since| Helper {
            id: id.into(),
            kind: kind.into(),
            description: description.into(),
            teammate: teammate.map(String::from),
            status,
            started,
            since,
        };
        let mut cards = long_and_short();
        cards[0].helpers = vec![
            helper("a", "Explore", "Lire le tableau de bord", None, HelperStatus::Running, 988, 988),
            helper("b", "Plan", "Découper la tâche", None, HelperStatus::Done, 900, 996),
            helper("m", "reviewer", "", Some("critic"), HelperStatus::Idle, 800, 940),
        ];
        cards
    }

    /// The agents a session started have cards of their own, under the members,
    /// each kind in a section under a rule; nothing is left under the member's card. A click on one leads to the
    /// member that started it.
    #[test]
    fn agents_as_cards_under_the_members() {
        let drawn = render(&board(with_helpers()), 70, 40);
        let lines: Vec<String> = drawn.lines.iter().map(Line::text).collect();
        let top = title(&lines, "coordinateur").unwrap();
        // Its card: the title, two lines of what it does, its curve; the next one is not a helper.
        assert!(lines[top + 1].starts_with("┃   Je relis") && lines[top + 2].starts_with("┃   droite"), "{lines:#?}");
        assert!(lines[top + 3].starts_with('╹'), "{lines:#?}");
        let rule = |sign: char, noun: &str, n: usize| {
            lines.iter().position(|l| l.starts_with(&format!("─ {sign} {noun} · {n} ─"))).expect(noun)
        };
        let subs = rule('⧗', &t!("sous-agents", "subagents"), 2);
        let mates = rule('♙', "teammate", 1);
        assert!(
            mates < subs && lines[subs].chars().count() == 70,
            "the teammates first, the subagents last: {lines:#?}"
        );
        // Cards in the members' shape: the name in the clear, the member that started it at the right, the time, what
        // it was asked, the curve; the subagents' type with its hourglass, ✓ once done.
        let card = |at: usize| lines[at..at + 3].iter().map(|l| l.trim_end().to_string()).collect::<Vec<_>>();
        let first = card(subs + 1);
        assert!(first[0].starts_with("╻ ⧗ Explore") && first[0].contains("⧗ coordinateur"), "{first:#?}");
        assert!(first[0].ends_with(" 12s") && first[1] == "┃   Lire le tableau de bord", "{first:#?}");
        assert!(first[2].starts_with("╹   "), "{first:#?}");
        let second = card(subs + 5);
        assert!(second[0].starts_with("╻ ✓ Plan") && second[0].ends_with(" 1m"), "{second:#?}");
        let mate = card(mates + 1);
        assert!(mate[0].contains("critic") && mate[0].contains("♙ coordinateur"), "{mate:#?}");
        // A click goes to the member that started them.
        let zone = |row: usize| drawn.zones.iter().find(|z| z.row == row && z.kind == ZoneKind::Member).unwrap();
        for row in [subs + 1, subs + 5, mates + 1] {
            assert_eq!(zone(row).member, "coordinateur", "row {row}");
            assert_eq!((zone(row).col, zone(row).rows), (0, 3));
        }
        // No agents, no sections.
        let lines: Vec<String> = render(&board(long_and_short()), 70, 40).lines.iter().map(Line::text).collect();
        assert!(!lines.iter().any(|l| l.starts_with("─ ")), "{lines:#?}");
    }

    /// As the pane gets lower, a section keeps fewer lines: cards with their text, without, two by two, only their
    /// names, only counted, left out; the members' cards never lose a line to them.
    #[test]
    fn agents_give_up_lines_before_the_members_do() {
        let mut shapes: Vec<&str> = Vec::new();
        let members = |lines: &[String]| -> Option<Vec<String>> {
            let top = title(lines, "coordinateur")?;
            Some(lines[top..top + 4].to_vec())
        };
        let without = {
            let mut cards = with_helpers();
            cards[0].helpers.clear();
            cards
        };
        for height in (12..=40).rev() {
            let lines: Vec<String> = render(&board(with_helpers()), 70, height).lines.iter().map(Line::text).collect();
            // The members are drawn as they are without any agents.
            let alone: Vec<String> = render(&board(without.clone()), 70, height).lines.iter().map(Line::text).collect();
            assert_eq!(members(&lines), members(&alone), "{height}: {lines:#?}");
            let has = |text: &str| lines.iter().any(|l| l.contains(text));
            let shape = if has("┃   Lire le tableau") {
                "text"
            } else if lines.iter().any(|l| l.starts_with("╻ ⧗ Explore")) {
                "cards"
            } else if has(&format!("⧗ {}", t!("2 sous-agents", "2 subagents"))) {
                "counted"
            } else if lines.iter().any(|l| l.starts_with("  ⧗  ")) {
                "names"
            } else {
                "none"
            };
            if shapes.last() != Some(&shape) {
                shapes.push(shape);
            }
        }
        assert_eq!(shapes[0], "text", "{shapes:?}");
        assert_eq!(shapes.last(), Some(&"none"), "{shapes:?}");
        assert!(shapes.contains(&"counted") || shapes.contains(&"names"), "{shapes:?}");
    }

    /// A scene with three subagents and three teammates, swept from a tall pane to a low one: the room fills card by
    /// card, and no section ever gets richer as the pane shrinks; the teammates keep
    /// their place longer than the subagents.
    #[test]
    fn agents_never_get_richer_as_the_pane_shrinks() {
        let agent = |id: &str, teammate: Option<&str>, status| Helper {
            id: id.into(),
            kind: if teammate.is_some() { "reviewer".into() } else { format!("SubK{id}") },
            description: format!("text-of-{id}"),
            teammate: teammate.map(String::from),
            status,
            started: 900,
            since: 950,
        };
        let mut lead = sample_card("coordinateur", true, State::Working);
        lead.helpers = vec![
            agent("1", None, HelperStatus::Running),
            agent("2", None, HelperStatus::Running),
            agent("3", None, HelperStatus::Done),
            agent("a", Some("mate-a"), HelperStatus::Running),
            agent("b", Some("mate-b"), HelperStatus::Idle),
            agent("c", Some("mate-c"), HelperStatus::Idle),
        ];
        // How much of a section shows: names alone 1, each card 10, each text 1 more.
        let score = |lines: &[String], card: &str, text: &str| -> usize {
            let cards = lines.iter().filter(|l| l.starts_with('╻') && l.contains(card)).count();
            let texts = lines.iter().filter(|l| l.contains(text)).count();
            if cards + texts > 0 { 10 * cards + texts } else { usize::from(lines.iter().any(|l| l.contains(card))) }
        };
        for width in [60, 70, 140] {
            let mut before: Option<(usize, usize, usize)> = None;
            let mut seen = Vec::new();
            for height in (10..=70).rev() {
                // One member alone: its card does not change with the height, so the room the agents get only shrinks.
                let lines: Vec<String> =
                    render(&board(vec![lead.clone()]), width, height).lines.iter().map(Line::text).collect();
                let (subs, mates) = (score(&lines, "SubK", "text-of-1"), score(&lines, "mate-", "text-of-a"));
                // The teammates' text: the three share a marker apart.
                let mates =
                    mates.max(lines.iter().filter(|l| l.contains("text-of-b") || l.contains("text-of-c")).count());
                if let Some((was_height, was_subs, was_mates)) = before {
                    assert!(
                        subs <= was_subs && mates <= was_mates,
                        "width {width}: richer at {height} than at {was_height}: subagents {was_subs} → {subs}, \
                         teammates {was_mates} → {mates}\n{lines:#?}"
                    );
                }
                before = Some((height, subs, mates));
                seen.push((subs, mates));
            }
            // From all of it down to nothing, the teammates holding on longer.
            assert!(seen.first().is_some_and(|&(s, m)| s > 10 && m > 10), "{width}: {:?}", seen.first());
            let last_mates = seen.iter().rposition(|&(_, m)| m > 0).unwrap();
            let last_subs = seen.iter().rposition(|&(s, _)| s > 0).unwrap();
            assert!(last_mates >= last_subs, "{width}: the teammates outlast the subagents");
        }
    }

    /// A list of names, then a count: `  ♙  ⠹ ux-critic  ⠹ perf  ◷ a11y`, `  ♙ 3 teammates`, in yellow when one works,
    /// in orange when one failed, else faded.
    #[test]
    fn agents_by_name_and_counted() {
        let mate =
            |name: &str, status| Helper { id: name.into(), teammate: Some(name.into()), ..helper(name, None, status) };
        let mut lead = sample_card("coordinateur", true, State::Working);
        lead.helpers = vec![
            mate("ux-critic", HelperStatus::Running),
            mate("perf", HelperStatus::Running),
            mate("a11y", HelperStatus::Idle),
        ];
        let b = board(vec![lead]);
        let [_, teammates] = agents(&b);
        let names = agent_names(&teammates, '♙', 60, &b);
        let text = names.lines[0].text();
        assert!(text.starts_with("  ♙  ⠋ perf  ⠋ ux-critic  ◷ a11y"), "{text}");
        assert_eq!(names.zones.len(), 3);
        assert!(names.zones.iter().all(|z| z.member == "coordinateur" && z.rows == 1));
        // Too narrow for all of them: the ones that hold and how many more.
        let text = agent_names(&teammates, '♙', 28, &b).lines[0].text();
        assert!(text.contains("perf") && text.ends_with(&others(2)) && !text.contains("ux-critic"), "{text}");
        let count = agent_count(&teammates, '♙', &b);
        assert_eq!(count.lines[0].text(), "  ♙ 3 teammates");
        // Each kind in its own words, in the plural from two.
        let sub = |n: usize| {
            let mut lead = sample_card("coordinateur", true, State::Working);
            lead.helpers = (0..n).map(|i| helper(&format!("s{i}"), None, HelperStatus::Running)).collect();
            let b = board(vec![lead]);
            let [subagents, _] = agents(&b);
            agent_count(&subagents, '⧗', &b).lines[0].text()
        };
        assert_eq!(sub(1), format!("  ⧗ 1 {}", t!("sous-agent", "subagent")));
        assert_eq!(sub(2), format!("  ⧗ 2 {}", t!("sous-agents", "subagents")));
        assert_eq!(count.lines[0].0[0].1, Paint::Color(State::Working.color()));
        let mut idle = teammates;
        idle.retain(|a| a.helper.status == HelperStatus::Idle);
        assert_eq!(agent_count(&idle, '♙', &b).lines[0].text(), "  ♙ 1 teammate");
        assert_eq!(agent_count(&idle, '♙', &b).lines[0].0[0].1, Paint::Dim);
    }

    /// A failure in orange, not in red: the capsule, the sign and the time.
    #[test]
    fn a_failed_subagent_in_orange() {
        let mut lead = sample_card("coordinateur", true, State::Working);
        lead.helpers = vec![Helper { since: 996, ..helper("a", None, HelperStatus::Failed) }];
        let b = board(vec![lead]);
        let [subagents, _] = agents(&b);
        let card = agent_card(&subagents[0], 50, true, 3, &b);
        let orange = State::Error.color();
        assert_eq!(card[0].0[0].1, Paint::Bold(orange));
        assert!(card[0].text().starts_with("╻ ✗ Explore"), "{}", card[0].text());
        assert_eq!(card[1].0[2].1, Paint::Color(orange));
    }

    #[test]
    fn teammate_gone_quiet_shows_unknown() {
        let mut cards = with_helpers();
        cards[0].helpers.clear();
        cards[0].helpers.push(Helper {
            id: "m".into(),
            kind: "reviewer".into(),
            description: String::new(),
            teammate: Some("critic".into()),
            status: HelperStatus::Running,
            started: 0,
            since: 1000 - bridge::TEAMMATE_UNKNOWN,
        });
        let lines: Vec<String> = render(&board(cards), 70, 30).lines.iter().map(Line::text).collect();
        assert!(lines.iter().any(|l| l.starts_with("╻ ? critic")), "{lines:#?}");
    }
}
