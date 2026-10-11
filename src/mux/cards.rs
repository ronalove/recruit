// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The cards of the members left out of a tab in « focus » (maquette https://claude.ai/artifact/H4tpNaZ2RAnxHErZHyztYe,
//! direction B): the dashboard's own cards (`board::card_lines`, one drawing), in a grey frame titled « others » with
//! the members' counts by state. From 120 columns, one column of cards with what the member does, its curve and its
//! context; narrower, two by two in their short form (title and curve). What does not fit gives way to « … and N more ».
//!
//! The frame is a pane's, drawn by the screen (`screen::View`, with [`title`] and [`counts`] in its header); the cards
//! are what a [`Panel`] draws inside it, and [`place`] says where each one is, to draw it and to hit-test a click.
//! No terminal, no files, no clock: the server gives the cards' data and the time.
//!
//! Owner: dev-rendu.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use super::chrome::Hint;
use super::engine::{Engine, Modes, Relay};
use super::{Rect, Rgb};
use crate::board::{CardData, Clock, card_lines, card_rows};
use crate::canvas::{Canvas, Cursor, Style};
use crate::look::State;
use crate::t;

/// Columns between two cards side by side.
const GAP: usize = 2;
/// Lines of what a member does, on a card from 120 columns.
const DOING_LINES: usize = 1;
/// Narrower than this, two cards side by side would be too narrow to read: one column of short cards.
const MIN_PAIR: usize = 24;

/// The frame's title.
pub(crate) fn title() -> String {
    t!("autres", "others")
}

/// The counts at the right of the frame's top border, as on the dashboard's.
pub(crate) fn counts(cards: &[CardData]) -> Hint {
    let (mut working, mut waiting, mut error, mut idle) = (0, 0, 0, 0);
    for card in cards {
        match card.state {
            State::Working => working += 1,
            State::Waiting => waiting += 1,
            State::Error => error += 1,
            State::Idle => idle += 1,
            State::Other => {}
        }
    }
    Hint::Counts { working, waiting, error, idle }
}

/// Where the cards go in `inside` (the frame's cells): each shown card by its index in `cards` with its rectangle, and
/// how many do not fit (the last row then says so). `wide`: the window has 120 columns or more.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Placed {
    pub shown: Vec<(usize, Rect)>,
    pub more: usize,
}

pub(crate) fn place(inside: Rect, cards: &[CardData], wide: bool) -> Placed {
    let all = fit(inside, cards, wide, inside.height);
    if all.len() == cards.len() || inside.height == 0 {
        return Placed { shown: all, more: 0 };
    }
    // Some are left out: the last row is for saying how many.
    let shown = fit(inside, cards, wide, inside.height - 1);
    let more = cards.len() - shown.len();
    Placed { shown, more }
}

/// As many cards as fit in `rows` rows of `inside`, in order.
fn fit(inside: Rect, cards: &[CardData], wide: bool, rows: usize) -> Vec<(usize, Rect)> {
    let mut shown = Vec::new();
    if inside.width == 0 {
        return shown;
    }
    let pairs = !wide && inside.width >= MIN_PAIR;
    if !pairs {
        let lines = if wide { DOING_LINES } else { 0 };
        let mut y = 0;
        for (i, card) in cards.iter().enumerate() {
            let height = card_rows(card, inside.width, lines);
            if y + height > rows {
                break;
            }
            shown.push((i, Rect { x: inside.x, y: inside.y + y, width: inside.width, height }));
            y += height + 1;
        }
        return shown;
    }
    let half = (inside.width - GAP) / 2;
    let mut y = 0;
    for (row, pair) in cards.chunks(2).enumerate() {
        let height = pair.iter().map(|card| card_rows(card, half, 0)).max().unwrap_or(0);
        if y + height > rows {
            break;
        }
        for k in 0..pair.len() {
            let x = inside.x + k * (half + GAP);
            shown.push((row * 2 + k, Rect { x, y: inside.y + y, width: half, height }));
        }
        y += height + 1;
    }
    shown
}

/// The cards, as the engine of the frame's pane: [`Engine::draw`] puts them in the frame's cells.
pub(crate) struct Panel<'a> {
    pub cards: &'a [CardData],
    /// The window has 120 columns or more.
    pub wide: bool,
    pub clock: Clock,
}

impl Panel<'_> {
    /// What the panel draws, as a number: the cards' fields that show (the curve follows the clock), the clock to the
    /// second, and the spinner's image only while a card is at work. The same number, the same cells: the screen then
    /// keeps them from one frame to the next. A hash, so that nothing is copied or drawn to know it.
    fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        (self.wide, self.clock.now, self.clock.glyphs as u8).hash(&mut hasher);
        if self.cards.iter().any(|card| card.state == State::Working) {
            self.clock.frame.hash(&mut hasher);
        }
        for card in self.cards {
            (&card.name, card.color, card.state as u8, card.since, card.shell).hash(&mut hasher);
            (card.context.map(f64::to_bits), card.pressure as u8, &card.model, &card.effort).hash(&mut hasher);
            (card.doing.as_ref().map(|task| (&task.now, &task.done)), &card.failed, card.compacted).hash(&mut hasher);
            (card.activity.seen, card.activity.work.len(), card.activity.work.last()).hash(&mut hasher);
        }
        hasher.finish()
    }
}

impl Engine for Panel<'_> {
    fn feed(&mut self, _: &[u8], _: &mut Vec<u8>) {}
    fn deadline(&self) -> Option<Instant> {
        None
    }
    fn expire(&mut self, _: Instant, _: &mut Vec<u8>) -> bool {
        false
    }
    fn relays(&mut self, _: &mut Vec<Relay>) {}
    fn resize(&mut self, _: u16, _: u16) {}
    fn set_colors(&mut self, _: Option<Rgb>, _: Option<Rgb>) {}
    fn modes(&self) -> Modes {
        Modes::default()
    }

    fn draw(&self, canvas: &mut Canvas, area: Rect) {
        let placed = place(area, self.cards, self.wide);
        // The cards of a column line up with each other, as on the dashboard: one call per column, over references.
        let lines = if self.wide { DOING_LINES } else { 0 };
        let first = area.x;
        let mut columns: [Vec<&(usize, Rect)>; 2] = [Vec::new(), Vec::new()];
        for placed in &placed.shown {
            columns[usize::from(!self.wide && placed.1.x != first)].push(placed);
        }
        for column in columns {
            let Some((_, top)) = column.first() else { continue };
            let data: Vec<&CardData> = column.iter().map(|(i, _)| &self.cards[*i]).collect();
            for ((_, rect), drawn) in column.iter().zip(card_lines(&data, top.width, lines, self.clock)) {
                for (row, line) in drawn.iter().enumerate().take(rect.height) {
                    line.draw_at(canvas, rect.x, rect.y + row, rect.x + rect.width);
                }
            }
        }
        if placed.more > 0 && area.height > 0 {
            let text = t!("… et {} autres", "… and {} more", placed.more);
            canvas.put_in(area.x + 3, area.y + area.height - 1, area.x + area.width, &text, Style::PLAIN.dim());
        }
    }

    fn generation(&self) -> Option<u64> {
        Some(self.fingerprint())
    }
    fn cursor(&self) -> Option<Cursor> {
        None
    }
    fn history(&self) -> usize {
        0
    }
    fn scroll(&mut self, _: isize) {}
    fn scrolled(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Activity, Clock};
    use crate::look::{Glyphs, Pressure};
    use crossterm::style::Color;

    fn card(name: &str, state: State) -> CardData {
        CardData {
            name: name.into(),
            color: Color::Cyan,
            state,
            since: 90,
            shell: false,
            context: Some(41.0),
            pressure: Pressure::Calm,
            model: Some("Opus".into()),
            effort: Some("xhigh".into()),
            doing: None,
            activity: Activity::default(),
            failed: None,
            compacted: None,
        }
    }

    fn clock() -> Clock {
        Clock { now: 1000, frame: 0, glyphs: Glyphs::Unicode }
    }

    fn rows(canvas: &Canvas, height: usize) -> Vec<String> {
        (0..height).map(|y| canvas.row(y)).collect()
    }

    #[test]
    fn counts_by_state() {
        let cards =
            [card("a", State::Working), card("b", State::Idle), card("c", State::Idle), card("d", State::Error)];
        assert_eq!(counts(&cards), Hint::Counts { working: 1, waiting: 0, error: 1, idle: 2 });
    }

    #[test]
    fn wide_cards_in_one_column() {
        let cards = [card("dev-a", State::Idle), card("dev-b", State::Idle)];
        let inside = Rect { x: 1, y: 1, width: 48, height: 7 };
        let placed = place(inside, &cards, true);
        assert_eq!(placed.more, 0);
        assert_eq!(placed.shown.len(), 2);
        // No text to say: a card of two rows, a gap of one.
        assert_eq!(placed.shown[0].1, Rect { x: 1, y: 1, width: 48, height: 2 });
        assert_eq!(placed.shown[1].1.y, 4);
        let mut canvas = Canvas::new(50, 9);
        Panel { cards: &cards, wide: true, clock: clock() }.draw(&mut canvas, inside);
        let text = rows(&canvas, 9);
        assert!(text[1].contains("dev-a") && text[1].contains("Opus"), "{text:?}");
        assert!(text[4].contains("dev-b"), "{text:?}");
        assert!(text[2].contains("41 %"), "{text:?}");
    }

    #[test]
    fn narrow_cards_two_by_two() {
        let cards = [card("dev-a", State::Idle), card("dev-b", State::Idle), card("dev-c", State::Idle)];
        let inside = Rect { x: 0, y: 0, width: 34, height: 5 };
        let placed = place(inside, &cards, false);
        let at: Vec<_> = placed.shown.iter().map(|(i, r)| (*i, r.x, r.y)).collect();
        assert_eq!(at, [(0, 0, 0), (1, 16 + GAP, 0), (2, 0, 3)]);
        assert!(placed.shown.iter().all(|(_, r)| r.width == 16 && r.height == 2));
        let mut canvas = Canvas::new(34, 5);
        Panel { cards: &cards, wide: false, clock: clock() }.draw(&mut canvas, inside);
        let text = rows(&canvas, 5);
        assert!(text[0].contains("dev-a") && text[0].contains("dev-b"), "{text:?}");
        assert!(text[3].contains("dev-c") && !text[3].contains("dev-b"), "{text:?}");
    }

    #[test]
    fn what_does_not_fit_is_counted() {
        let cards: Vec<_> = (0..5).map(|i| card(&format!("m{i}"), State::Idle)).collect();
        // Rows for two cards (2 + gap + 2), one more for the count: three are left out.
        let inside = Rect { x: 0, y: 0, width: 40, height: 6 };
        let placed = place(inside, &cards, true);
        assert_eq!((placed.shown.len(), placed.more), (2, 3), "{placed:?}");
        let mut canvas = Canvas::new(40, 6);
        Panel { cards: &cards, wide: true, clock: clock() }.draw(&mut canvas, inside);
        let text = rows(&canvas, 6);
        assert!(text[5].contains(&t!("… et 3 autres", "… and 3 more")), "{text:?}");
        // One card more fits with its row, but not the fifth, and the count still takes a row.
        let placed = place(Rect { height: 12, ..inside }, &cards, true);
        assert_eq!((placed.shown.len(), placed.more), (4, 1));
        // All five fit: no count.
        let placed = place(Rect { height: 14, ..inside }, &cards, true);
        assert_eq!((placed.shown.len(), placed.more), (5, 0));
    }

    #[test]
    fn the_panel_is_kept_while_nothing_it_shows_changes() {
        let panel = |cards: &[CardData], clock: Clock| Panel { cards, wide: true, clock }.generation();
        let mut cards = [card("a", State::Idle), card("b", State::Idle)];
        let first = panel(&cards, clock());
        assert!(first.is_some());
        assert_eq!(panel(&cards, clock()), first);
        // The spinner's image only matters while a card is at work.
        assert_eq!(panel(&cards, Clock { frame: 7, ..clock() }), first);
        // The clock moves the curves, a second at a time.
        assert_ne!(panel(&cards, Clock { now: 1001, ..clock() }), first);
        // What a card shows.
        cards[1].context = Some(42.0);
        let changed = panel(&cards, clock());
        assert_ne!(changed, first);
        cards[1].state = State::Working;
        let working = panel(&cards, clock());
        assert_ne!(working, changed);
        assert_ne!(panel(&cards, Clock { frame: 7, ..clock() }), working);
        // The window's width changes the cards' form.
        assert_ne!(Panel { cards: &cards, wide: false, clock: clock() }.generation(), working);
    }

    #[test]
    #[ignore = "a measure, not a test: cargo test --release cards::tests::cost -- --ignored --nocapture"]
    fn cost_of_a_frame() {
        let cards: Vec<_> = (0..6).map(|i| card(&format!("member-{i}"), State::Working)).collect();
        let panel = Panel { cards: &cards, wide: true, clock: clock() };
        let inside = Rect { x: 1, y: 1, width: 48, height: 40 };
        let mut canvas = Canvas::new(60, 42);
        let n = 2000;
        let start = Instant::now();
        for _ in 0..n {
            panel.draw(&mut canvas, inside);
        }
        eprintln!("draw {:?}/frame", start.elapsed() / n);
        let start = Instant::now();
        for _ in 0..n {
            std::hint::black_box(panel.generation());
        }
        eprintln!("generation {:?}", start.elapsed() / n);
        let start = Instant::now();
        for _ in 0..n {
            std::hint::black_box(place(inside, &cards, true));
        }
        eprintln!("place {:?}", start.elapsed() / n);
    }

    #[test]
    fn nothing_in_no_room() {
        let cards = [card("a", State::Idle)];
        assert!(place(Rect { x: 0, y: 0, width: 0, height: 5 }, &cards, true).shown.is_empty());
        let placed = place(Rect { x: 0, y: 0, width: 30, height: 0 }, &cards, true);
        assert_eq!(placed, Placed { shown: Vec::new(), more: 0 });
        assert_eq!(place(Rect { x: 0, y: 0, width: 30, height: 5 }, &[], true), Placed::default());
    }
}
