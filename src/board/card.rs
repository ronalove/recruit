// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A member's card, as the dashboard draws it and as the multiplexer draws the cards of the others.

use crossterm::style::Color;

use crate::claude::bridge::Task;
use crate::t;
use crate::ui::canvas::{self, Long, Wrap, columns, fit};
use crate::ui::look::{self, Glyphs, Pressure, State, duration};

use super::paint::*;
use super::states::*;
use super::*;

/// How far back the activity curves go, in seconds.
pub(super) const HISTORY: i64 = 30 * 60;
/// From no activity to all the time.
const LEVELS: [char; 5] = ['⣀', '⣠', '⣤', '⣶', '⣿'];

/// What the titles show on the right of the names, the same on every card of a width and in columns from one to the
/// next: the model, the effort's sign and name, the time. Each column as wide as its widest; the richest form that
/// every card holds: `Opus   ◉ extra`, else `Opus   ◉`, else `O ◉`, else the time alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Right {
    pub(super) model: usize,
    /// The model's initial rather than its name: one column.
    pub(super) initial: bool,
    /// One for the sign alone, more for the sign and the name, none for neither.
    pub(super) effort: usize,
    pub(super) time: usize,
}

impl Right {
    /// For cards, each with its width.
    pub(super) fn of<'a>(list: impl IntoIterator<Item = (&'a CardData, usize)>) -> Self {
        let list: Vec<(&CardData, usize)> = list.into_iter().collect();
        let widest =
            |each: &dyn Fn(&CardData) -> Option<usize>| list.iter().filter_map(|(c, _)| each(c)).max().unwrap_or(0);
        let time = widest(&|c| Some(columns(&duration(c.since))));
        let model = widest(&|c| c.model.as_ref().map(|m| columns(m)));
        // The name's column is as wide as the longest name of the language, a level's or `max`.
        let level = widest(&|c| c.effort.as_ref().map(|_| 2 + look::effort_name_width()));
        let forms = [
            Right { model, initial: false, effort: level, time },
            Right { model, initial: false, effort: level.min(1), time },
            Right { model: model.min(1), initial: true, effort: level.min(1), time },
        ];
        let bare = Right { model: 0, initial: false, effort: 0, time };
        forms
            .into_iter()
            .filter(|right| right.width() > 0)
            .find(|right| list.iter().all(|(c, width)| right.width() + 3 <= right.rest(c, *width)))
            .unwrap_or(bare)
    }

    pub(super) fn width(&self) -> usize {
        self.model + if self.effort > 0 && self.model > 0 { 1 } else { 0 } + self.effort
    }

    /// What a card `width` wide leaves on its title past its sign, its name and the time.
    pub(super) fn rest(&self, c: &CardData, width: usize) -> usize {
        let room = width.saturating_sub(3);
        room.saturating_sub(2 + columns(&name_in(c, room, self.time)) + self.time)
    }
}

/// The member's name on a title of `room` columns, the time `time` wide.
fn name_in(c: &CardData, room: usize, time: usize) -> String {
    fit(&c.name, room.saturating_sub(3 + time).max(1))
}

/// The context's field, at the right of the curve: `⟳ 88 %` for a member at rest, which a click compacts.
pub(super) const CONTEXT: usize = 7;

/// How many columns of a card's context a click compacts the member on: at rest or its turn failed, its context known.
pub(super) fn compaction_cells(c: &CardData) -> Option<usize> {
    let percent = c.context.filter(|_| matches!(c.state, State::Idle | State::Error))?;
    Some(2 + columns(&format!("{percent:.0} %")))
}

/// A member's card for a caller that is not the dashboard (the « others » of the focus layout, drawn by the server):
/// what the dashboard's card shows, from what the server knows of the member.
#[derive(Debug, Clone)]
pub(crate) struct CardData {
    pub name: String,
    pub color: Color,
    pub state: State,
    /// Seconds in the state.
    pub since: u64,
    /// At rest while a command it started still runs.
    pub shell: bool,
    /// Of the model's window, in percent.
    pub context: Option<f64>,
    pub pressure: Pressure,
    /// The model's family (`Opus`), and the effort's level.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// What it does, in progress and once done (the request's short form).
    pub doing: Option<Task>,
    /// Its last half hour, as `MemberLook::activity` has it: the curve is drawn from its stretches.
    pub activity: Activity,
    /// Its turn failed: why, short (`look::error_label`).
    pub failed: Option<String>,
    /// When it was last compacted (`MemberLook::compacted`): a ⟳ in its curve.
    pub compacted: Option<i64>,
}

/// When and how a card is drawn: the clock, the spinners' image, the icons.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Clock {
    /// Seconds since the epoch.
    pub now: i64,
    pub frame: usize,
    pub glyphs: Glyphs,
}

/// The cards `width` wide, each over at most `lines` lines of what the member does (none: the short form, its title and
/// its curve), as the dashboard draws them: one capsule on the left in the state's color. The model, the effort and the
/// time line up from one card to the next, as on the dashboard.
pub(crate) fn card_lines(data: &[&CardData], width: usize, lines: usize, clock: Clock) -> Vec<Vec<Line>> {
    let right = Right::of(data.iter().map(|&d| (d, width)));
    let board = Board { now: clock.now, frame: clock.frame, glyphs: clock.glyphs, ..Board::default() };
    data.iter().map(|&d| card(d, width, lines, &right, &board)).collect()
}

/// The rows a card takes at `width` over at most `lines` lines: what `card_lines` gives, without drawing it.
pub(crate) fn card_rows(data: &CardData, width: usize, lines: usize) -> usize {
    let doing = || doing_lines_of(data.failed.as_deref(), data.doing.as_ref(), data.state, width, lines).len();
    2 + if lines == 0 { 0 } else { doing() }
}

/// The title line of a card: its capsule's top, the state's `sign`, the `name`, `gap` blanks, the `middle` pieces (at the
/// right of the name, before the time), the `time` and a blank; cut to `width`.
pub(super) fn title_line(
    bar: (String, Paint),
    sign: (String, Paint),
    name: (String, Paint),
    gap: usize,
    middle: Vec<(String, Paint)>,
    time: (String, Paint),
    width: usize,
) -> Line {
    let mut title = vec![bar, (" ".into(), Paint::Plain), sign, (" ".into(), Paint::Plain), name];
    title.push((" ".repeat(gap), Paint::Plain));
    title.extend(middle);
    title.push(time);
    title.push((" ".into(), Paint::Plain));
    painted(&title, width)
}

/// A card, `width` wide: its title, what the member does over at most `lines` lines, its curve and context. A
/// capsule on the left in the state's color, on all its lines; the state read by its intensity: yellow and bold at
/// work, red waiting for the user, faded at rest.
///
/// ```text
/// ╻ ⠹ dev-mod            Opus █ xhigh   3m
/// ┃   Je relis la maquette des cartes
/// ╹   ⣀⣀⣠⣤⣶⣿⣿⣶⣤⣀⣀                   74 %
/// ```
pub(super) fn card(c: &CardData, width: usize, lines: usize, right: &Right, b: &Board) -> Vec<Line> {
    let doing = if lines == 0 { Vec::new() } else { doing_lines(c, width, lines) };
    let height = 2 + doing.len();
    let color = c.state.color();
    let line = if matches!(c.state, State::Waiting | State::Error) { Paint::Bold(color) } else { Paint::Color(color) };
    // From halfway down the first line to halfway down the last: two cards never touch.
    let bar = |row: usize| (look::stroke(row, height).to_string(), line);
    let (sign, name, time) = match c.state {
        State::Working => (Paint::Color(color), Paint::Bold(c.color), Paint::Plain),
        State::Waiting => (Paint::Color(color), Paint::Bold(c.color), Paint::Color(color)),
        State::Error => (Paint::Bold(color), Paint::Bold(c.color), Paint::Color(color)),
        _ => (Paint::Dim, Paint::Faint(c.color), Paint::Dim),
    };

    let room = width.saturating_sub(3);
    let shown = name_in(c, room, right.time);
    let meta = if right.width() > 0 { right.width() + 2 } else { 0 };
    let gap = room.saturating_sub(2 + columns(&shown) + meta + right.time);
    // A failed turn says more than a command running.
    let icon = if c.shell && c.state != State::Error { b.glyphs.console() } else { c.state.icon(b.glyphs, b.frame) };
    let mut middle = Vec::new();
    if meta > 0 {
        middle.extend(model_and_effort(c, right, b.frame));
        middle.push(("  ".into(), Paint::Plain));
    }
    let time = (format!("{:>w$}", duration(c.since), w = right.time), time);
    let mut out = vec![title_line(bar(0), (icon.to_string(), sign), (shown, name), gap, middle, time, width)];
    for (i, (text, paint)) in doing.into_iter().enumerate() {
        // As wide as the card, for the one on its right.
        let pad = " ".repeat(width.saturating_sub(4 + columns(&text)));
        out.push(painted(&[bar(1 + i), ("   ".into(), Paint::Plain), (text, paint), (pad, Paint::Plain)], width));
    }
    let cells = width.saturating_sub(6 + CONTEXT);
    let mut bottom = vec![bar(height - 1), ("   ".into(), Paint::Plain)];
    // A compaction in the last half hour: a ⟳ in the curve, in the slice it fell in.
    match c.compacted.and_then(|at| compaction_cell(at, b.now, cells)) {
        Some(at) => {
            let curve: Vec<char> = c.activity.curve(b.now, cells).chars().collect();
            let (before, after) = (curve[..at].iter().collect::<String>(), curve[at + 1..].iter().collect::<String>());
            bottom.extend([(before, Paint::Color(color)), ("⟳".into(), Paint::Plain), (after, Paint::Color(color))]);
        }
        None => bottom.push((c.activity.curve(b.now, cells), Paint::Color(color))),
    }
    bottom.push((" ".into(), Paint::Plain));
    bottom.extend(context(c));
    bottom.push((" ".into(), Paint::Plain));
    out.push(painted(&bottom, width));
    out
}

/// The share of time at work in each of `width` slices of the last half hour, as `curve` draws them, for something
/// seen since `from` and at work in the stretches `work` (`[from, to)` in seconds, oldest first): the slices are cut on
/// the clock too.
pub(super) fn work_curve(from: i64, work: &[(i64, i64)], now: i64, until: i64, width: usize) -> String {
    let slice = |t: i64| (t * width as i64).div_euclid(HISTORY);
    // The first second of slice `k`: the least `t` with `slice(t) >= k`.
    let start = |k: i64| (k * HISTORY + width as i64 - 1).div_euclid(width as i64);
    (0..width)
        .map(|i| {
            let k = slice(now) - (width - 1 - i) as i64;
            let (lo, hi) = (start(k), start(k + 1));
            let overlap = |a: i64, z: i64| (z.min(hi) - a.max(lo)).max(0);
            // Seen up to `until` only.
            let seen = overlap(from, until + 1);
            if seen == 0 {
                return ' ';
            }
            let busy: i64 = work.iter().map(|&(a, z)| overlap(a.max(from), z.min(until + 1))).sum();
            LEVELS[((busy.min(seen) as f64 / seen as f64) * 4.0).round() as usize]
        })
        .collect()
}

/// What the member does, over at most `lines` lines of a card `width` wide, each in its paint: the task in progress;
/// at rest, the task done after ✓, faded, the lines after the first under its text; in a state the dashboard does not
/// know, the task as it was. Nothing before the model has said. Its turn failed: « Tour raté : API surchargée (529) »
/// in orange, then the task it stopped in, after ↳, faded.
pub(super) fn doing_lines(c: &CardData, width: usize, lines: usize) -> Vec<(String, Paint)> {
    doing_lines_of(c.failed.as_deref(), c.doing.as_ref(), c.state, width, lines)
}

/// The same from what it takes: why the turn failed, the task, the state: no card to build for the rows it takes.
fn doing_lines_of(
    failed: Option<&str>,
    doing: Option<&Task>,
    state: State,
    width: usize,
    lines: usize,
) -> Vec<(String, Paint)> {
    if let Some(label) = failed {
        let room = width.saturating_sub(5);
        let mut out =
            vec![(fit(&t!("Tour raté : {}", "Turn failed: {}", label), room), Paint::Color(State::Error.color()))];
        if let Some(task) = doing.filter(|task| lines > 1 && !task.now.trim().is_empty()) {
            out.push((fit(&format!("↳ {}", task.now), room), Paint::Dim));
        }
        return out;
    }
    let Some(task) = doing else { return Vec::new() };
    let paint = if matches!(state, State::Working | State::Waiting | State::Error) { Paint::Plain } else { Paint::Dim };
    let (mark, text) = match &task.done {
        Some(done) if state == State::Idle => ("✓ ", done),
        _ if state == State::Idle => ("✓ ", &task.now),
        _ => ("", &task.now),
    };
    let indent = columns(mark);
    wrap(text, width.saturating_sub(5 + indent), lines)
        .into_iter()
        .enumerate()
        .map(|(i, line)| if i == 0 { format!("{mark}{line}") } else { format!("{}{line}", " ".repeat(indent)) })
        .map(|line| (line, paint))
        .collect()
}

/// `text` over at most `lines` lines of `width` columns, cut between words, a word longer than a line where it must;
/// an ellipsis ends the last line when the text goes on. One line at least, empty for an empty text.
pub(super) fn wrap(text: &str, width: usize, lines: usize) -> Vec<String> {
    canvas::wrap(text, width.max(1), Wrap::new(Long::Split).lines(lines))
}

/// The model, faded (its initial short of room, `?` while it is not known), then the effort's sign and name in Claude
/// Code's colors; each padded to its column.
pub(super) fn model_and_effort(c: &CardData, right: &Right, frame: usize) -> Vec<(String, Paint)> {
    let model = match (&c.model, right.initial) {
        (_, true) => look::model_initial(c.model.as_deref()).to_string(),
        (Some(model), false) => model.clone(),
        (None, false) => "?".to_string(),
    };
    let mut pieces = Vec::new();
    if right.model > 0 {
        pieces.push((format!("{model:<w$}", w = right.model), Paint::Dim));
    }
    if right.effort == 0 {
        return pieces;
    }
    if right.model > 0 {
        pieces.push((" ".into(), Paint::Plain));
    }
    let mut used = 0;
    if let Some(level) = &c.effort {
        // `max` in a rainbow, turning with the spinners.
        // A name that is not known, longer than the column, is cut at it.
        for (ch, color) in look::effort_cells(level, right.effort > 1, frame / 2).into_iter().take(right.effort) {
            pieces.push((ch.to_string(), color.map_or(Paint::Plain, Paint::Color)));
            used += 1;
        }
    }
    pieces.push((" ".repeat(right.effort.saturating_sub(used)), Paint::Plain));
    pieces
}

/// The context's field, `CONTEXT` wide: the share of the window, orange near compaction, red where Claude Code
/// warns; `⟳` before it for a member at rest.
pub(super) fn context(c: &CardData) -> Vec<(String, Paint)> {
    let Some(percent) = c.context else { return vec![(" ".repeat(CONTEXT), Paint::Plain)] };
    let text = format!("{percent:.0} %");
    let paint = match c.pressure {
        Pressure::Warning => Paint::Bold(Color::Red),
        Pressure::Near => Paint::Color(look::palette().orange),
        Pressure::Calm if resting(c) => Paint::Dim,
        Pressure::Calm => Paint::Plain,
    };
    let compacts = compaction_cells(c);
    let mut pieces = vec![(" ".repeat(CONTEXT.saturating_sub(compacts.unwrap_or(columns(&text)))), Paint::Plain)];
    if compacts.is_some() {
        pieces.push(("⟳ ".into(), Paint::Dim));
    }
    pieces.push((text, paint));
    pieces
}

impl Activity {
    /// The curve of its stretches, `width` cells: a cell is blank before it was seen.
    pub(super) fn curve(&self, now: i64, width: usize) -> String {
        if self.seen == 0 {
            return " ".repeat(width);
        }
        // Known up to the last second noted, whatever the clock says: the second under way is not yet at rest.
        let until = if self.last == 0 { now } else { now.min(self.last) };
        work_curve(self.seen, &self.work, now, until, width)
    }
}

/// The cell of a curve `width` cells wide that a compaction at `at` falls in, if it is still in the half hour it shows:
/// the slices are cut on the clock, as the curve's own.
pub(super) fn compaction_cell(at: i64, now: i64, width: usize) -> Option<usize> {
    let slice = |t: i64| (t * width as i64).div_euclid(HISTORY);
    let back = slice(now) - slice(at);
    (width > 0 && (0..width as i64).contains(&back)).then(|| width - 1 - back as usize)
}

#[cfg(test)]
mod tests {

    use std::time::{Instant, SystemTime};

    use crossterm::style::{Color, Stylize};

    use crate::claude::bridge::Task;

    use crate::mux::proto::PaneInfo;
    use crate::sys::now;

    use crate::t;
    use crate::ui::canvas::Canvas;
    use crate::ui::look::{self, Glyphs, Pressure, State};

    use super::*;

    use crate::board::arrange::*;

    use crate::board::dashboard::*;
    use crate::board::fixture::*;

    use crate::board::sessions::*;

    use crate::board::{Board, Card};

    /// The cards the server draws for the focus layout are the dashboard's own: the same lines, lined up the same way.
    #[test]
    fn the_cards_for_others_are_the_dashboards_own() {
        // Oldest first, as the dashboard keeps them.
        let samples = || -> Vec<(i64, bool)> { (0..600).rev().map(|i| (1000 - i, i % 3 == 0)).collect() };
        let data = |name: &str, model: &str, state| CardData {
            name: name.into(),
            color: Color::Cyan,
            state,
            since: 90,
            shell: false,
            context: Some(41.0),
            pressure: Pressure::Calm,
            model: Some(model.into()),
            effort: Some("xhigh".into()),
            doing: Some(Task { now: "Je relis la maquette des cartes".into(), done: None }),
            activity: activity_of(&samples()),
            failed: None,
            compacted: None,
        };
        let list = [data("dev-mod", "Opus", State::Working), data("ops", "Haiku", State::Idle)];
        let list: Vec<&CardData> = list.iter().collect();
        let clock = Clock { now: 1000, frame: 3, glyphs: Glyphs::Unicode };
        let drawn = card_lines(&list, 44, 2, clock);
        // The dashboard's card for the first, with the second beside it for the columns.
        let right = Right::of(list.iter().map(|&d| (d, 44)));
        let board = Board { now: 1000, frame: 3, glyphs: Glyphs::Unicode, ..Board::default() };
        assert_eq!(drawn[0], card(list[0], 44, 2, &right, &board));
        let text: Vec<String> = drawn[0].iter().map(Line::text).collect();
        assert!(
            text[0].contains("dev-mod") && text[0].contains("Opus") && text[0].contains(&look::effort_label("xhigh")),
            "{text:?}"
        );
        assert!(text[1].contains("Je relis la maquette"), "{text:?}");
        assert!(text.last().unwrap().contains("41 %"), "{text:?}");
        // The columns line up from one card to the next: the second one's effort starts where the first's does.
        let at = |lines: &[Line]| lines[0].text().find(&look::effort_label("xhigh"));
        assert_eq!(drawn[1][0].text().chars().count(), drawn[0][0].text().chars().count());
        assert!(at(&drawn[0]).is_some() && at(&drawn[1]).is_some());
        // The short form: its title and its curve.
        let short = card_lines(&list, 44, 0, clock);
        assert_eq!(short[0].len(), 2);
        assert_eq!(card_rows(list[0], 44, 0), 2);
        assert_eq!(card_rows(list[0], 44, 2), drawn[0].len());
        // Nothing seen, a blank curve.
        assert_eq!(Activity::default().curve(1000, 5), "     ");
        // A line on a canvas, cut where it must.
        let mut canvas = Canvas::new(30, 2);
        let end = drawn[0][0].draw_at(&mut canvas, 2, 0, 20);
        assert_eq!(end, 20);
        assert!(canvas.row(0).starts_with("  ╻ "), "{}", canvas.row(0));
    }

    #[test]
    fn what_they_do_on_each_card() {
        let doing = |name: &str, state, now: &str, done: Option<&str>| {
            sample_card(name, false, state).tweak(|c| {
                c.data.doing = Some(Task { now: now.into(), done: done.map(String::from) });
            })
        };
        let cards = vec![
            doing("coordinateur", State::Working, "Je relis la maquette des cartes", Some("Maquette relue")),
            doing("dev", State::Idle, "Je publie la version 0.6.2", Some("Version 0.6.2 publiée")),
            sample_card("ops", false, State::Waiting),
        ];
        let lines: Vec<String> = padded(&board(cards.clone()), 50, 23).lines.iter().map(Line::text).collect();
        // The one waiting for the user first; nothing said yet, no line for it.
        assert!(lines[2].starts_with("╻ ⚑ ops ") && lines[3].starts_with("╹ "));
        assert!(lines[5].starts_with("╻ ⠋ coordinateur "));
        assert_eq!(lines[6].trim_end(), "┃   Je relis la maquette des cartes");
        assert!(lines[7].starts_with("╹   ") && lines[8].is_empty());
        // At rest, the last task, done.
        assert_eq!(lines[10].trim_end(), "┃   ✓ Version 0.6.2 publiée");
        assert!(!lines.iter().any(|l| l.trim_end() == "┃"), "{lines:#?}");
        // In a state the dashboard does not know: the task as it was, faded, nothing done.
        let unknown = doing("ops", State::Other, "Je publie la version 0.6.2", Some("Version 0.6.2 publiée"));
        let right = Right { model: 4, initial: false, effort: 8, time: 2 };
        let line = card(&unknown.data, 50, 1, &right, &board(Vec::new()))[1].clone();
        assert_eq!(visible(&line).trim_end(), "┃   Je publie la version 0.6.2");
        assert!(line.printed().contains(&"Je publie la version 0.6.2".dim().to_string()));
        // The model gave no form for it done: as it was.
        let undone = doing("review", State::Idle, "Je relis le clic", None);
        let right = Right { model: 4, initial: false, effort: 8, time: 2 };
        assert_eq!(
            visible(&card(&undone.data, 50, 1, &right, &board(Vec::new()))[1]).trim_end(),
            "┃   ✓ Je relis le clic"
        );
        // Short of room, the agent at rest goes first, into the list of names; the one at work keeps its task.
        let lines: Vec<String> = padded(&board(cards), 50, 15).lines.iter().map(Line::text).collect();
        assert!(lines.iter().any(|l| l.trim_end() == "┃   Je relis la maquette des cartes"), "{lines:#?}");
        assert_eq!(lines.iter().filter(|l| l.starts_with('╻')).count(), 2);
        assert!(lines.iter().any(|l| l.starts_with(&t!("au repos", "idle")) && l.ends_with(" dev")), "{lines:#?}");
    }

    #[test]
    fn wrapped_between_words() {
        let text = "Je publie la version 0.6.2 de recruit";
        assert_eq!(wrap(text, 20, 2), ["Je publie la version", "0.6.2 de recruit"]);
        assert_eq!(wrap(text, 40, 2), [text]);
        // On one line, cut.
        assert_eq!(wrap(text, 20, 1), ["Je publie la versio…"]);
        // Past two lines, the second ends with an ellipsis.
        assert_eq!(wrap("un deux trois quatre cinq six sept huit", 10, 2), ["un deux", "trois qua…"]);
        // A word longer than a line, cut where it must.
        assert_eq!(wrap("abcdefghij", 6, 2), ["abcdef", "ghij"]);
        assert_eq!(wrap("abcdefghijklmnop qr", 6, 2), ["abcdef", "ghijk…"]);
        assert_eq!(wrap("ok", 6, 2), ["ok"]);
        assert_eq!(wrap("", 6, 2), [""]);
        // Accents at the edge of a line: counted as characters, not bytes.
        assert_eq!(wrap("Équipe réglée près", 7, 2), ["Équipe", "réglée…"]);
        assert_eq!(wrap("é é é é", 3, 2), ["é é", "é é"]);
        // Wide characters count two columns each: a task of them wraps on two lines of the width, where it used to
        // run past it.
        assert_eq!(wrap("日本語 日本語", 6, 2), ["日本語", "日本語"]);
        assert_eq!(wrap("日本語日本語日本語", 8, 3), ["日本語日", "本語日本", "語"]);
        assert_eq!(wrap("日本語日本語日本語", 8, 2), ["日本語日", "本語日…"]);
        assert_eq!(wrap("日本", 1, 2), ["日", "本"]);
    }

    #[test]
    fn capsule_over_four_lines() {
        let right = Right { model: 4, initial: false, effort: 8, time: 2 };
        let lines: Vec<String> =
            card(&long_and_short()[0].data, 50, 2, &right, &board(Vec::new())).iter().map(Line::text).collect();
        let bars: Vec<char> = lines.iter().map(|l| l.chars().next().unwrap()).collect();
        assert_eq!(bars, ['╻', '┃', '┃', '╹']);
        assert_eq!(lines[1].trim_end(), "┃   Je relis la maquette des cartes et je corrige");
        assert_eq!(lines[2].trim_end(), "┃   les colonnes de droite");
        // At rest, the lines after the first under the text, past its ✓.
        let done = sample_card("dev", false, State::Idle).tweak(|c| {
            c.data.doing =
                Some(Task { now: String::new(), done: Some("Version 0.6.2 publiée et poussée dans le tap".into()) });
        });
        let lines: Vec<String> = card(&done.data, 40, 2, &right, &board(Vec::new())).iter().map(Line::text).collect();
        assert_eq!(lines[1].trim_end(), "┃   ✓ Version 0.6.2 publiée et poussée");
        assert_eq!(lines[2].trim_end(), "┃     dans le tap");
    }

    #[test]
    fn the_right_of_the_titles_in_columns() {
        let cards = vec![
            sample_card("coordinateur", true, State::Working),
            sample_card("dev", false, State::Idle).tweak(|c| {
                c.data.model = Some("Sonnet".into());
                c.data.effort = Some("low".into());
                c.data.since = 4000;
            }),
            sample_card("ops", false, State::Waiting).tweak(|c| {
                c.data.model = None;
                c.data.effort = None;
            }),
        ];
        let titles = |width: usize| -> Vec<String> {
            let lines = render(&board(cards.clone()), width, 23).lines;
            lines.iter().map(Line::text).filter(|l| l.starts_with('╻')).collect()
        };
        let ends = |titles: Vec<String>, ends: [&str; 3]| {
            assert!(
                titles
                    .iter()
                    .zip(ends)
                    .all(|(t, end)| t.ends_with(end) && t.chars().count() == titles[0].chars().count()),
                "{titles:#?}"
            );
        };
        // The richest form they all hold, each column as wide as its widest (the effort's the widest name of the language,
        // two columns more than a sign), the time on the right: coordinateur holds the model and the effort's name from 39
        // columns on, its sign from 32, the model's initial from 27; a model not known yet is « ? » in its column.
        let name = |level: &str| format!("{:<8}", look::effort_label(level));
        ends(
            titles(39),
            [
                &format!(" Opus   {}    1m ", name("xhigh")),
                &format!(" ?      {}    1m ", " ".repeat(8)),
                &format!(" Sonnet {}  1h06 ", name("low")),
            ],
        );
        ends(titles(38), [" Opus   ◉    1m ", " ?           1m ", " Sonnet ○  1h06 "]);
        ends(titles(32), [" Opus   ◉    1m ", " ?           1m ", " Sonnet ○  1h06 "]);
        ends(titles(31), [" O ◉    1m ", " ?      1m ", " S ○  1h06 "]);
        ends(titles(27), [" O ◉    1m ", " ?      1m ", " S ○  1h06 "]);
        let bare = titles(26);
        assert!(bare.iter().all(|t| !t.contains("Opus") && !t.contains('◉') && t.chars().count() == 26));
        assert!(bare[2].ends_with(" 1h06 "));
    }

    /// An effort no one knows (a later Claude Code's, a slip in a file): « ? » in grey, its name cut at the column, never
    /// past it.
    #[test]
    fn an_unknown_effort_stays_in_its_column() {
        let odd = sample_card("dev", false, State::Working).tweak(|c| {
            c.data.effort = Some("ultra-turbo-extreme".into());
        });
        let right = Right { model: 4, initial: false, effort: 8, time: 2 };
        let title = card(&odd.data, 50, 0, &right, &board(Vec::new()))[0].clone();
        let text = title.text();
        assert!(text.contains("? ultra-") && !text.contains("ultra-t"), "{text}");
        assert_eq!(text.chars().count(), 50, "{text}");
        let grey = "?".with(Color::DarkGrey).to_string();
        assert!(title.printed().contains(&grey), "grey, not a rainbow");
    }

    #[test]
    fn model_faded_effort_in_its_colors() {
        let right = Right { model: 6, initial: false, effort: 8, time: 2 };
        let title = card(&sample_card("dev", false, State::Working).data, 50, 0, &right, &board(Vec::new()))[0].clone();
        let dim = "\x1b[2m";
        let title = title.printed();
        assert!(title.contains(&format!("{dim}Opus")));
        let violet = "◉".with(Color::AnsiValue(141)).to_string();
        assert!(!title.contains(&format!("{dim}◉")) && title.contains(&violet));
    }

    #[test]
    fn context_near_compaction() {
        assert_eq!(Pressure::of(Some(150_000), None), Pressure::Calm);
        assert_eq!(Pressure::of(None, Some(167_000)), Pressure::Calm);
        assert_eq!(Pressure::of(Some(133_000), Some(167_000)), Pressure::Calm);
        assert_eq!(Pressure::of(Some(133_600), Some(167_000)), Pressure::Near);
        assert_eq!(Pressure::of(Some(147_000), Some(167_000)), Pressure::Warning);
        assert_eq!(Pressure::of(Some(773_600), Some(967_000)), Pressure::Near);
        let near = sample_card("dev", false, State::Idle).tweak(|c| {
            c.data.pressure = Pressure::Near;
        });
        let field: String = context(&near.data).iter().map(|(text, paint)| paint.apply(text)).collect();
        assert!(field.contains(&"41 %".with(look::palette().orange).to_string()));
        assert_eq!(visible(&field), " ⟳ 41 %");
        // At work, no compacting.
        assert_eq!(compaction_cells(&sample_card("dev", false, State::Working).data), None);
    }

    /// In a light terminal the board is drawn in the light palette: the yellow at work gives way
    /// to 136, the orange to 166, the members' own colors to theirs; back in the dark, as before.
    #[test]
    fn a_board_in_the_light_palette() {
        let first = |c: &Card| card(&c.data, 44, 0, &Right::of([(&c.data, 44)]), &board(Vec::new()))[0].0[0].1;
        let working = sample_card("dev", false, State::Working);
        let failed = sample_card("dev", false, State::Error).tweak(|c| {
            c.data.failed = Some("API".into());
            c.error = None;
        });
        assert_eq!(first(&working), Paint::Color(Color::Yellow));
        look::set_light(true);
        assert_eq!(first(&working), Paint::Color(Color::AnsiValue(136)));
        assert_eq!(first(&failed), Paint::Bold(Color::AnsiValue(166)));
        assert_eq!(look::member_color(3), Color::AnsiValue(167));
        // The gauge's warning color follows.
        let usage = usage_line(&[("5 h".into(), 75.0, None)], 40);
        assert!(usage.0.iter().any(|(_, paint)| *paint == Paint::Color(Color::AnsiValue(136))), "{usage:?}");
        look::set_light(false);
        assert_eq!(first(&working), Paint::Color(Color::Yellow));
        assert_eq!(look::member_color(3), Color::AnsiValue(209));
    }

    /// A member whose turn failed: its card after those waiting and before those at work, an orange
    /// capsule, ✗, « Tour raté : <raison> » then the task it stopped in after ↳, faded; its context compactable; the
    /// error's text written for the menu, the reason read from its pane's error.
    #[test]
    fn a_failed_turn_on_its_card() {
        let label = look::error_label(Some("API Error: 529 overloaded"));
        let failed = sample_card("dev-saisie", false, State::Error).tweak(|c| {
            c.data.doing = Some(Task { now: "Encodeur des touches kitty".into(), done: None });
            c.data.failed = Some(label.clone());
            c.error = Some("API Error: 529 overloaded".into());
        });
        let cards = vec![
            sample_card("dev-terminal", false, State::Working),
            failed.clone(),
            sample_card("testeur", false, State::Waiting),
        ];
        assert_eq!(
            ordered(&cards).iter().map(|c| c.data.name.as_str()).collect::<Vec<_>>(),
            ["testeur", "dev-saisie", "dev-terminal"]
        );
        let b = board(vec![failed.clone()]);
        let right = Right::of([(&failed.data, 60)]);
        let lines = card(&failed.data, 60, 2, &right, &b);
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert!(text[0].starts_with("╻ ✗ dev-saisie"), "{text:#?}");
        assert_eq!(text[1].trim_end(), format!("┃   {}", t!("Tour raté : {}", "Turn failed: {}", label)));
        assert_eq!(text[2].trim_end(), "┃   ↳ Encodeur des touches kitty");
        assert!(text[3].ends_with(" ⟳ 41 % "), "{text:#?}");
        let orange = Paint::Color(State::Error.color());
        assert_eq!(lines[0].0[0], ("╻".to_string(), Paint::Bold(State::Error.color())));
        assert!(
            lines[1]
                .0
                .iter()
                .any(|(text, paint)| text.starts_with(&t!("Tour raté", "Turn failed")) && *paint == orange)
        );
        assert!(lines[2].0.iter().any(|(text, paint)| text.starts_with('↳') && *paint == Paint::Dim));
        // On one line: the reason only. With a command still running: the failure's sign.
        let one = card(&failed.data, 60, 1, &right, &b);
        assert_eq!(one.len(), 3);
        let busy = failed.clone().tweak(|c| {
            c.data.shell = true;
        });
        assert!(card(&busy.data, 60, 0, &right, &b)[0].text().starts_with("╻ ✗"));

        // The states for the menu: the error's text.
        let mut memory = Memory::default();
        memory.since.insert("dev-saisie".into(), (State::Error, Instant::now()));
        let states = memory.states(&b, SystemTime::now(), Instant::now());
        assert_eq!(states.members["dev-saisie"].state, State::Error);
        assert_eq!(states.errors["dev-saisie"], "API Error: 529 overloaded");

        // From the panes: the state and the error's text.
        let pane = PaneInfo {
            id: "%1".into(),
            member: "dev-saisie".into(),
            role: String::new(),
            tab: "Moteur".into(),
            pid: 0,
            cols: 80,
            rows: 24,
            status: Some(State::Error.name().into()),
            since: Some(now() as u64 - 240),
            error: Some("API Error: 529 overloaded".into()),
        };
        let sessions = Sessions {
            told: told(Some(std::slice::from_ref(&pane))),
            failed: crate::board::sessions::failed(Some(std::slice::from_ref(&pane))),
            ..Default::default()
        };
        let state = tempfile::tempdir().unwrap();
        let seen = Memory::default().board(&snapshot(&["dev-saisie"]), state.path(), &sessions);
        assert_eq!(seen.cards[0].data.failed, Some(label));
        assert_eq!(seen.cards[0].error.as_deref(), Some("API Error: 529 overloaded"));
        assert!((240..243).contains(&seen.cards[0].data.since));
    }

    #[test]
    fn curves() {
        let all: Vec<(i64, bool)> = (1..=HISTORY).map(|t| (t, true)).collect();
        assert_eq!(activity_of(&all).curve(HISTORY, 10), "⣿".repeat(10));
        // The last minute, over the end of a slice and the start of the next one.
        let recent: Vec<(i64, bool)> = (HISTORY - 59..=HISTORY).map(|t| (t, false)).collect();
        assert_eq!(activity_of(&recent).curve(HISTORY, 10), format!("{}⣀⣀", " ".repeat(8)));

        // Slices of three minutes, working one, resting the next, since `base`.
        let base = 200 * 180;
        let samples =
            |until: i64| -> Vec<(i64, bool)> { (base..=until).map(|t| (t, (t - base) / 180 % 2 == 0)).collect() };
        let now = base + 1800;
        assert_eq!(activity_of(&samples(now)).curve(now, 10), "⣀⣿⣀⣿⣀⣿⣀⣿⣀⣿");
        // Half a slice later, the past ones have not moved.
        assert_eq!(activity_of(&samples(now + 90)).curve(now + 90, 10), "⣀⣿⣀⣿⣀⣿⣀⣿⣀⣿");
        // A new slice: the curve moves by one, the slice nothing was noted in blank.
        assert_eq!(activity_of(&samples(now + 90)).curve(now + 180, 10), "⣿⣀⣿⣀⣿⣀⣿⣀⣿ ");
        // The second under way, not noted yet, is not at rest: the last cell of a member at work stays full.
        let mut working = Activity::default();
        for t in 1..=HISTORY {
            working.note(t, true);
        }
        assert_eq!(working.curve(HISTORY + 1, 10), "⣿".repeat(10));
        assert_eq!(working.curve(HISTORY + 1, 10), working.curve(HISTORY, 10));
    }

    /// A name or a task in wide characters (two columns each) is cut by columns: a card is as wide as its pane, where
    /// it used to run past it by one column for each wide character.
    #[test]
    fn wide_characters_are_counted_by_columns() {
        let b = board(Vec::new());
        let width = 40;
        for name in ["开发者前端工程师后端开发者前端工程师后端", "日本語"] {
            let c = sample_card(name, false, State::Idle);
            let right = Right::of([(&c.data, width)]);
            let title = visible(&card(&c.data, width, 0, &right, &b)[0]);
            assert_eq!(columns(&title), width, "{title:?}");
        }
        // The name cut keeps its ellipsis in the pane; a short one is whole.
        let long = sample_card("开发者前端工程师后端开发者前端工程师后端", false, State::Idle);
        let right = Right::of([(&long.data, width)]);
        assert!(visible(&card(&long.data, width, 0, &right, &b)[0]).contains('…'));
        // The task a failed turn stopped in, long and wide: every line of the card holds in the pane.
        let failed = sample_card("dev", false, State::Error).tweak(|c| {
            c.data.failed = Some("API".into());
            c.data.doing = Some(Task { now: "日本語".repeat(20), done: None });
        });
        let right = Right::of([(&failed.data, 44)]);
        let lines = card(&failed.data, 44, 2, &right, &b);
        assert!(
            lines.iter().all(|l| columns(&visible(l)) <= 44),
            "{:#?}",
            lines.iter().map(Line::text).collect::<Vec<_>>()
        );
        assert!(visible(&lines[2]).starts_with("┃   ↳ 日本語") && visible(&lines[2]).contains('…'));
        // A wide task at work, over two lines: each holds in the card.
        let working = sample_card("dev", false, State::Working).tweak(|c| {
            c.data.doing = Some(Task { now: "日本語の長い仕事 ".repeat(8), done: None });
        });
        let right = Right::of([(&working.data, 44)]);
        let lines = card(&working.data, 44, 2, &right, &b);
        assert!(
            lines.iter().all(|l| columns(&visible(l)) <= 44),
            "{:#?}",
            lines.iter().map(Line::text).collect::<Vec<_>>()
        );
        assert_eq!(lines.len(), 4);
        // The first line is whole, the second ends the text with its ellipsis: nothing cut in the middle.
        assert!(!visible(&lines[1]).contains('…'), "{:?}", visible(&lines[1]));
        assert!(visible(&lines[2]).trim_end().ends_with('…'), "{:?}", visible(&lines[2]));
    }
}
