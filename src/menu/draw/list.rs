// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The members' list on the left: a card for each, its state, its model and its effort.

use crossterm::style::Color;

use super::super::sheet::{Entry, Overlay, Pane, Person, Target};
use crate::t;
use crate::ui::canvas::{Style, columns, fit};
use crate::ui::look::{self, State};

use super::*;

impl Drawer<'_> {
    /// The members, then the new agent, from `top` to `bottom` (excluded); scrolled to keep the chosen one in sight.
    pub(super) fn list(&mut self, split: usize, top: usize, bottom: usize) {
        // Each entry as rows of its own: a label row, a card, a gap.
        enum Item<'p> {
            Label(String),
            Card(&'p Person),
            Gap,
            New,
        }
        let team = &self.sheet.team;
        let order = team.order();
        let mut items: Vec<Item> = Vec::new();
        let contacts: Vec<&Person> = order.iter().map(|&i| &team.people[i]).filter(|p| p.contact).collect();
        let agents: Vec<&Person> = order.iter().map(|&i| &team.people[i]).filter(|p| !p.contact).collect();
        for (label, group) in [(t!("Interlocuteurs", "Contacts"), contacts), (t!("Agents", "Agents"), agents)] {
            if group.is_empty() {
                continue;
            }
            items.push(Item::Label(label));
            for person in group {
                items.push(Item::Card(person));
                if self.wide {
                    items.push(Item::Gap);
                }
            }
            if !self.wide {
                items.push(Item::Gap);
            }
        }
        if self.sheet.entries().contains(&Entry::New) {
            items.push(Item::New);
        }
        let height = |item: &Item| match item {
            Item::Card(_) => 2,
            _ => 1,
        };
        let room = bottom - top;
        let chosen_at = items
            .iter()
            .position(|item| match (item, &self.sheet.entry) {
                (Item::Card(p), Entry::Member(name)) => &p.name == name,
                (Item::New, Entry::New) => true,
                _ => false,
            })
            .unwrap_or(0);
        let start_of = |i: usize| items[..i].iter().map(&height).sum::<usize>();
        let total = start_of(items.len());
        let chosen_bottom = start_of(chosen_at) + height(&items[chosen_at]);
        let scroll = if total <= room { 0 } else { chosen_bottom.saturating_sub(room).min(total - room) };
        let focus_here = self.sheet.pane == Pane::List && matches!(self.sheet.overlay, Overlay::None);
        let mut y = top as isize - scroll as isize;
        for item in &items {
            let h = height(item) as isize;
            let visible = y >= top as isize && y + h <= bottom as isize;
            if visible {
                let row = y as usize;
                match item {
                    Item::Label(label) => {
                        self.c.put(1, row, label, muted());
                    }
                    Item::Gap => {}
                    Item::Card(person) => {
                        let chosen = self.sheet.entry == Entry::Member(person.name.clone());
                        self.card(
                            person,
                            split,
                            row,
                            chosen.then_some(if focus_here { selected() } else { chosen_card() }),
                        );
                        self.zone(0, row, split, 2, Target::Entry(Entry::Member(person.name.clone())));
                    }
                    Item::New => {
                        let chosen = self.sheet.entry == Entry::New;
                        let x = self.c.put(1, row, "+", key_style().bold());
                        self.c.put_in(x, row, split, &t!(" Nouvel agent", " New agent"), key_style());
                        if chosen {
                            self.c.tint(0, row, split, if focus_here { selected() } else { chosen_card() });
                        }
                        self.zone(0, row, split, 1, Target::Entry(Entry::New));
                    }
                }
            }
            y += h;
        }
        if scroll > 0 {
            self.c.put(split - 2, top, "▲", muted());
        }
        if total > room && scroll < total - room {
            self.c.put(split - 2, bottom - 1, "▼", muted());
        }
    }

    /// A member's card: its state's stroke and sign, its name in its color, the time in its state; then its model,
    /// dimmed, and its effort.
    pub(super) fn card(&mut self, p: &Person, split: usize, y: usize, tint: Option<Color>) {
        let state = self.sheet.state(&p.name);
        let color = state.map_or(MUTED, |(s, _)| s.color());
        self.c.put(1, y, look::stroke(0, 2), Style::fg(color));
        self.c.put(1, y + 1, look::stroke(1, 2), Style::fg(color));
        if let Some((s, _)) = state {
            let sign =
                if matches!(s, State::Waiting | State::Error) { Style::fg(color).bold() } else { Style::fg(color) };
            self.c.put(3, y, &s.icon(self.look.glyphs, self.look.frame).to_string(), sign);
        }
        let name_style = match state.map(|(s, _)| s) {
            Some(State::Working | State::Waiting | State::Error) => Style::fg(p.color).bold(),
            Some(_) => Style::fg(p.color).dim(),
            None => Style::fg(p.color),
        };
        let time = state
            .filter(|_| self.wide)
            .map(|(_, since)| look::duration(self.look.now.saturating_sub(since).max(0) as u64));
        let time_width = time.as_deref().map_or(0, columns);
        let name_room = split.saturating_sub(5 + 1 + time_width + 1);
        let name = if p.gone { format!("{} ⚠", p.name) } else { p.name.clone() };
        self.c.put(5, y, &fit(&name, name_room), name_style);
        if let Some(time) = &time {
            self.c.put_right(split - 1, y, time, muted());
        }
        let model = p.model.clone().unwrap_or_default();
        let x = if self.wide {
            self.c.put_in(5, y + 1, split, &format!("{model:<7}"), Style::PLAIN.dim())
        } else {
            self.c.put_in(5, y + 1, split, &format!("{model} "), Style::PLAIN.dim())
        };
        if let Some(effort) = &p.effort {
            self.effort(x, y + 1, split, effort, self.wide);
        }
        if p.gone {
            self.c.put_in(5, y + 1, split, &t!("absent des fichiers", "not in the files"), Style::fg(Color::Red));
        }
        if let Some(bg) = tint {
            self.c.tint(0, y, split, bg);
            self.c.tint(0, y + 1, split, bg);
        }
    }

    /// An effort: its sign, and its name when `word`, in Claude Code's colors; `max` in a rainbow, still (turning, it
    /// would be sent again at each frame).
    pub(super) fn effort(&mut self, x: usize, y: usize, end: usize, level: &str, word: bool) -> usize {
        let mut x = x;
        for (ch, color) in look::effort_cells(level, word, 0) {
            x = self.c.put_in(x, y, end, &ch.to_string(), color.map_or(Style::PLAIN, Style::fg));
        }
        x
    }
}
