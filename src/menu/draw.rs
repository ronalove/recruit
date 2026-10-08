// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The menu on a screen of a given size, drawn into a [`Canvas`], and where each click lands: the members on the left
//! as on the dashboard, the chosen one's sheet on the right, the team's actions at the bottom (direction A of the
//! mockups). Narrower than [`WIDE`], the list keeps the names only and the origin of a value goes under the sheet.

use crossterm::style::Color;

use super::sheet::{Action, Confirm, Dropdown, Entry, Fid, Overlay, Pane, Person, Said, Sheet, Target, Typing};
use crate::canvas::{Canvas, Style, columns, fit};
use crate::look::{self, Glyphs, RAINBOW, State};
use crate::t;

/// From this width, the wide layout.
pub(crate) const WIDE: usize = 90;
/// Below this, the menu says the window is too small.
pub(crate) const MIN_WIDTH: usize = 50;
pub(crate) const MIN_HEIGHT: usize = 14;

const SELECTED: Color = Color::AnsiValue(237);
/// The chosen card while the sheet has the focus.
const CHOSEN: Color = Color::AnsiValue(235);
const BUTTON: Color = Color::AnsiValue(236);
const INPUT: Color = Color::AnsiValue(233);
const FADED: Color = Color::AnsiValue(239);
const MUTED: Color = Color::DarkGrey;

/// How things look, beside the sheet: the icons, the spinners' image, the time, the keys' sign.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Look {
    pub glyphs: Glyphs,
    pub frame: usize,
    /// Seconds since the epoch.
    pub now: i64,
    /// ⌥ rather than Alt+ (macOS).
    pub option: bool,
}

/// Where a click lands.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Zone {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub target: Target,
}

pub(crate) struct Drawn {
    pub canvas: Canvas,
    /// The last drawn first.
    pub zones: Vec<Zone>,
    /// The list's width: the wheel left of it moves in the list.
    pub split: usize,
}

impl Drawn {
    pub(crate) fn hit(&self, x: usize, y: usize) -> Option<Target> {
        self.zones
            .iter()
            .find(|z| (z.x..z.x + z.width).contains(&x) && (z.y..z.y + z.height).contains(&y))
            .map(|z| z.target.clone())
    }
}

fn muted() -> Style {
    Style::fg(MUTED)
}

fn key_style() -> Style {
    Style::fg(Color::Cyan)
}

pub(crate) fn draw(sheet: &Sheet, look: &Look, width: usize, height: usize) -> Drawn {
    let mut d = Drawer { c: Canvas::new(width, height), zones: Vec::new(), sheet, look: *look, wide: width >= WIDE };
    if width < MIN_WIDTH || height < MIN_HEIGHT {
        let text =
            t!("Fenêtre trop petite : {} × {} au moins", "Window too small: {} × {} at least", MIN_WIDTH, MIN_HEIGHT);
        let text = fit(&text, width);
        let x = width.saturating_sub(columns(&text)) / 2;
        d.c.put(x, height / 2, &text, muted());
        return Drawn { canvas: d.c, zones: Vec::new(), split: width };
    }
    let split = if d.wide { 34 } else { 18 };
    d.header(width);
    d.c.hline(0, 1, width, '─', muted());
    d.c.put(split, 1, "┬", muted());
    // What the last change said, on up to three rows: then the team's actions, then the keys.
    let said = d.said(width);
    let bottom = height - 3 - said.len().max(1);
    d.c.vline(split, 2, bottom - 2, '│', muted());
    d.c.hline(0, bottom, width, '─', muted());
    d.c.put(split, bottom, "┴", muted());
    d.list(split, 2, bottom);
    let hint_row = bottom - 1;
    d.sheet_rows(split + 1, 2, width, hint_row);
    d.hint(split + 2, hint_row, width - 1);
    d.message(bottom + 1, &said);
    if d.wide {
        d.buttons(height - 2, width);
    } else {
        d.action_keys(height - 2, width);
    }
    d.keys(height - 1, width);
    match &sheet.overlay {
        Overlay::List(list) => d.dropdown(list, split, bottom),
        Overlay::Confirm(confirm) => d.confirm(confirm, width, height),
        _ => {}
    }
    Drawn { canvas: d.c, zones: d.zones, split }
}

struct Drawer<'a> {
    c: Canvas,
    /// The last drawn first: an overlay's zones before what is under it.
    zones: Vec<Zone>,
    sheet: &'a Sheet,
    look: Look,
    wide: bool,
}

impl Drawer<'_> {
    fn zone(&mut self, x: usize, y: usize, width: usize, height: usize, target: Target) {
        self.zones.insert(0, Zone { x, y, width, height, target });
    }

    fn header(&mut self, width: usize) {
        let x = self.c.put(1, 0, "recruit", Style::PLAIN.bold());
        let team = if self.wide {
            t!(" · équipe « {} »", " · team \"{}\"", self.sheet.team.name)
        } else {
            t!(" · « {} »", " · \"{}\"", self.sheet.team.name)
        };
        self.c.put(x, 0, &team, Style::PLAIN);
        let mut end = width - 1;
        if self.wide {
            let close = t!(" ferme", " closes");
            let start = self.c.put_right(end, 0, &close, muted());
            end = self.c.put_right(start, 0, &t!("Échap", "Esc"), key_style()) - 3;
        }
        // The members by state, as at the top of the dashboard.
        let mut counts: Vec<(State, usize)> = Vec::new();
        for state in [State::Working, State::Waiting, State::Idle] {
            let n = self
                .sheet
                .team
                .people
                .iter()
                .filter(|p| self.sheet.state(&p.name).map(|(s, _)| s) == Some(state))
                .count();
            counts.push((state, n));
        }
        if self.sheet.states.is_empty() {
            return;
        }
        for (state, n) in counts.into_iter().rev() {
            let number = format!(" {n}");
            let start = self.c.put_right(end, 0, &number, Style::PLAIN);
            // Turning only while someone works: a frame sends only what changed.
            let frame = if n > 0 { self.look.frame } else { 0 };
            let icon = state.badge(self.look.glyphs, frame).to_string();
            end = self.c.put_right(start, 0, &icon, Style::fg(state.color())) - 3;
        }
    }

    /// The members, then the new agent, from `top` to `bottom` (excluded); scrolled to keep the chosen one in sight.
    fn list(&mut self, split: usize, top: usize, bottom: usize) {
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
                        self.card(person, split, row, chosen.then_some(if focus_here { SELECTED } else { CHOSEN }));
                        self.zone(0, row, split, 2, Target::Entry(Entry::Member(person.name.clone())));
                    }
                    Item::New => {
                        let chosen = self.sheet.entry == Entry::New;
                        let x = self.c.put(1, row, "+", key_style().bold());
                        self.c.put_in(x, row, split, &t!(" Nouvel agent", " New agent"), key_style());
                        if chosen {
                            self.c.tint(0, row, split, if focus_here { SELECTED } else { CHOSEN });
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
    fn card(&mut self, p: &Person, split: usize, y: usize, tint: Option<Color>) {
        let state = self.sheet.state(&p.name);
        let color = state.map_or(MUTED, |(s, _)| s.color());
        self.c.put(1, y, look::stroke(0, 2), Style::fg(color));
        self.c.put(1, y + 1, look::stroke(1, 2), Style::fg(color));
        if let Some((s, _)) = state {
            let sign = if s == State::Waiting { Style::fg(color).bold() } else { Style::fg(color) };
            self.c.put(3, y, &s.icon(self.look.glyphs, self.look.frame).to_string(), sign);
        }
        let name_style = match state.map(|(s, _)| s) {
            Some(State::Working | State::Waiting) => Style::fg(p.color).bold(),
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

    /// An effort: its sign, and its level when `word`, in Claude Code's colors; `max` in a rainbow, still (turning, it
    /// would be sent again at each frame).
    fn effort(&mut self, x: usize, y: usize, end: usize, level: &str, word: bool) -> usize {
        let text =
            if word { format!("{} {level}", look::effort_sign(level)) } else { look::effort_sign(level).to_string() };
        let mut x = x;
        for (i, ch) in text.chars().enumerate() {
            let color = look::effort_color(level).unwrap_or(RAINBOW[i % RAINBOW.len()]);
            x = self.c.put_in(x, y, end, &ch.to_string(), Style::fg(color));
        }
        x
    }

    /// The sheet of the chosen entry, from `left` (the column after the list's line) to the right edge; returns
    /// the rows that take the focus with the row each is on.
    fn sheet_rows(&mut self, left: usize, top: usize, width: usize, last: usize) {
        let x0 = left + 1;
        let right = width - 1;
        let sheet = self.sheet;
        let mut y = top;
        match sheet.person() {
            Some(p) => {
                self.c.put(x0, y, &fit(&p.name, right - x0), Style::PLAIN.bold());
                if let Some((state, since)) = sheet.state(&p.name) {
                    let time = look::duration(self.look.now.saturating_sub(since).max(0) as u64);
                    let word = if self.wide { format!(" {} · {time}", state_word(state)) } else { format!(" {time}") };
                    let start = self.c.put_right(right, y, &word, muted());
                    self.c.put_right(
                        start,
                        y,
                        &state.icon(self.look.glyphs, self.look.frame).to_string(),
                        Style::fg(state.color()),
                    );
                }
                y += 1;
                let role = p.layers.as_ref().map_or(p.role.as_str(), |l| l.role.value.as_str());
                let lines = if self.wide { 2 } else { 1 };
                for line in wrap(role.trim(), right - x0, lines) {
                    self.c.put(x0, y, &line, Style::PLAIN);
                    y += 1;
                }
                y = top + 1 + lines + 1;
                // The team's files do not read: what it runs with, and nothing to change.
                if self.sheet.team.unreadable.is_some() {
                    self.c.put(x0, y, &t!("Modèle", "Model"), muted());
                    self.c.put(x0 + 15, y, p.model.as_deref().unwrap_or("—"), Style::PLAIN);
                    self.c.put(x0, y + 1, &t!("Effort", "Effort"), muted());
                    match &p.effort {
                        Some(effort) => {
                            self.effort(x0 + 15, y + 1, right, effort, true);
                        }
                        None => {
                            self.c.put(x0 + 15, y + 1, "—", Style::PLAIN);
                        }
                    }
                    let note = t!(
                        "Réglages en lecture seule tant que les fichiers de l'équipe ne se lisent pas.",
                        "Settings read only while the team's files do not read."
                    );
                    for (i, line) in wrap(&note, right - x0, 2).into_iter().enumerate() {
                        self.c.put(x0, y + 3 + i, &line, muted());
                    }
                }
                if p.gone {
                    let text = t!(
                        "« {} » n'est plus dans les fichiers de l'équipe, mais son panneau tourne encore.",
                        "\"{}\" is no longer in the team's files, but its pane still runs.",
                        p.name
                    );
                    for line in wrap(&text, right - x0, 3) {
                        self.c.put(x0, y, &line, Style::PLAIN);
                        y += 1;
                    }
                    y += 1;
                }
            }
            None => {
                self.c.put(x0, y, &t!("Nouvel agent", "New agent"), Style::PLAIN.bold());
                y = top + 2;
            }
        }
        let value_x = x0 + if self.wide { 15 } else { 14 };
        let focused = sheet.field();
        let typing = match &sheet.overlay {
            Overlay::Typing(typing) => Some(typing),
            _ => None,
        };
        // Short of room: the gaps between groups go first, then the rows scroll to keep the focused one in sight.
        let typing_error = typing.is_some_and(|t| t.error.is_some()) as usize;
        let room = last.saturating_sub(y).saturating_sub(typing_error);
        let mut rows = sheet.rows();
        if rows.len() > room {
            rows.retain(Option::is_some);
        }
        let focus_at = rows.iter().position(|r| r.is_some() && *r == focused).unwrap_or(0);
        let skip = if rows.len() > room { (focus_at + 1).saturating_sub(room).min(rows.len() - room) } else { 0 };
        let mut index = 0;
        for (n, row) in rows.into_iter().enumerate() {
            let Some(fid) = row else {
                if n >= skip {
                    y += 1;
                }
                continue;
            };
            if n < skip || y >= last {
                index += 1;
                continue;
            }
            let on = focused == Some(fid);
            if on {
                self.c.tint(left, y, right - left, SELECTED);
            }
            self.zone(left, y, right - left, 1, Target::Row(index));
            let label_style = if on { Style::PLAIN.bold() } else { Style::PLAIN };
            self.c.put_in(x0, y, value_x - 1, &label(fid), label_style);
            // Room for the origin on the right, in a wide window, for the rows that have one.
            let end = if self.wide && sheet.origin(fid).is_some() {
                right.saturating_sub(22).max(value_x + 10)
            } else {
                right
            };
            match typing {
                Some(typing) if typing.field == fid => {
                    y = self.input(typing, value_x, y, right, last);
                    index += 1;
                    y += 1;
                    continue;
                }
                _ => {}
            }
            self.value(fid, index, value_x, y, end, on);
            if self.wide
                && let Some(origin) = sheet.origin(fid)
            {
                self.c.put_right(right - 1, y, &fit(&origin, 20), muted());
            }
            if on {
                self.c.tint(left, y, right - left, SELECTED);
            }
            index += 1;
            y += 1;
        }
        if skip > 0 {
            self.c.put(right - 1, top, "▲", muted());
        }
    }

    /// A row's value, from `x` to `end`.
    fn value(&mut self, fid: Fid, index: usize, x: usize, y: usize, end: usize, on: bool) {
        let sheet = self.sheet;
        let pending = sheet.pending.is_some_and(|(f, _)| f == fid);
        match fid {
            Fid::Restart => {
                let mut x = x;
                let resume = if self.wide {
                    t!(" sur sa conversation ", " on its conversation ")
                } else {
                    t!(" reprise ", " resumed ")
                };
                for (fresh, text) in [(false, resume), (true, t!(" à neuf ", " afresh "))] {
                    let chosen = on && sheet.fresh == fresh;
                    let style =
                        if chosen { Style::fg(Color::Black).on(Color::Cyan).bold() } else { Style::PLAIN.on(BUTTON) };
                    let start = x;
                    x = self.c.put_in(x, y, end, &text, style);
                    self.zone(start, y, x - start, 1, Target::Restart(fresh));
                    x += 1;
                }
            }
            Fid::Remove => {
                let text = t!(
                    "« {} » quitte l'équipe…",
                    "\"{}\" leaves the team…",
                    sheet.person().map_or("", |p| p.name.as_str())
                );
                self.c.put_in(x, y, end, &text, Style::fg(Color::Red));
            }
            Fid::Close => {
                self.c.put_in(
                    x,
                    y,
                    end,
                    &t!(" Fermer son panneau ", " Close its pane "),
                    Style::fg(Color::Red).on(BUTTON),
                );
            }
            Fid::Compose => {
                if sheet.draft.composing.is_some() {
                    let sign = look::SPINNER[self.look.frame % look::SPINNER.len()].to_string();
                    let x = self.c.put_in(x, y, end, &sign, Style::fg(Color::Yellow));
                    let text = t!(
                        " Claude compose le nouvel agent… Échap annule",
                        " Claude is composing the new agent… Esc cancels"
                    );
                    self.c.put_in(x, y, end, &text, Style::fg(Color::Yellow));
                } else {
                    self.c.put_in(x, y, end, &t!(" Composer ", " Compose "), Style::PLAIN.on(BUTTON));
                }
            }
            Fid::Add => {
                let text = t!(" ⏎ Ajouter et lancer ", " ⏎ Add and launch ");
                self.c.put_in(x, y, end, &text, Style::fg(Color::Black).on(Color::Cyan).bold());
            }
            Fid::Instructions | Fid::NewInstructions => {
                let after = self.c.put_in(x, y, end, &sheet.value(fid), Style::PLAIN);
                self.c.put_in(after + 3, y, end, &t!("⏎ éditeur", "⏎ editor"), muted());
            }
            _ => {
                let text = sheet.value(fid);
                let start = x;
                let shown = if fid == Fid::Effort
                    && !pending
                    && sheet.person().and_then(|p| p.layers.as_ref()).is_some_and(|l| l.effort.value.is_some())
                {
                    let level = text.clone();
                    self.effort(start, y, end, &level, true)
                } else {
                    let style = if fid == Fid::NewName && config_error(sheet).is_some() {
                        Style::fg(Color::Red)
                    } else {
                        Style::PLAIN
                    };
                    let style = if pending { style.bold() } else { style };
                    self.c.put_in(start, y, end.saturating_sub(2), &fit(&text, end.saturating_sub(start + 2)), style)
                };
                if on && fid.cycles() {
                    self.step_signs(index, start, y, shown, end);
                    if pending {
                        self.c.put_in(shown + 3, y, end, &t!("à valider ⏎", "to apply ⏎"), Style::fg(Color::Yellow));
                    }
                }
            }
        }
    }

    /// ‹ before a value at `start` and › after it at `after`, each a click to the value before or after.
    fn step_signs(&mut self, index: usize, start: usize, y: usize, after: usize, end: usize) {
        if start >= 2 {
            self.c.put(start - 2, y, "‹", key_style());
            self.zone(start - 2, y, 1, 1, Target::Step(index, false));
        }
        if after + 1 < end {
            self.c.put(after + 1, y, "›", key_style());
            self.zone(after + 1, y, 1, 1, Target::Step(index, true));
        }
    }

    /// A text typed in place, its cursor as a reversed cell; what is wrong with it on the row under. Returns the last
    /// row it takes.
    fn input(&mut self, typing: &Typing, x: usize, y: usize, right: usize, last: usize) -> usize {
        let room = right.saturating_sub(x + 1);
        self.c.fill(x, y, room + 1, 1, Style::PLAIN.on(INPUT));
        // The part of the text that keeps the cursor in sight.
        let chars: Vec<char> = typing.text.chars().collect();
        let mut start = 0;
        while columns(&chars[start..typing.cursor].iter().collect::<String>()) + 1 > room && start < typing.cursor {
            start += 1;
        }
        let mut col = x;
        for (i, ch) in chars.iter().enumerate().skip(start) {
            let w = columns(&ch.to_string());
            if col + w > x + room {
                break;
            }
            let style = if i == typing.cursor { Style::PLAIN.reverse() } else { Style::PLAIN.on(INPUT) };
            col = self.c.put(col, y, &ch.to_string(), style);
        }
        if typing.cursor >= chars.len() && col < x + room + 1 {
            self.c.put(col, y, " ", Style::PLAIN.reverse());
        }
        match &typing.error {
            Some(error) if y + 1 < last => {
                self.c.put_in(x, y + 1, right, &fit(&format!("✗ {error}"), right - x), Style::fg(Color::Red));
                y + 1
            }
            _ => y,
        }
    }

    /// The line under the sheet: where the value comes from in a narrow window, what « default » gives, when a change
    /// takes effect.
    fn hint(&mut self, x: usize, y: usize, end: usize) {
        let sheet = self.sheet;
        let mut text = sheet.hint().unwrap_or_default();
        if !self.wide
            && let Some(fid) = sheet.field()
            && let Some(origin) = sheet.origin(fid)
        {
            text = if text.is_empty() { origin } else { format!("{origin} · {text}") };
        }
        if text.is_empty() {
            return;
        }
        let x = self.c.put(x, y, "↳ ", muted());
        self.c.put_in(x, y, end, &fit(&text, end.saturating_sub(x)), muted());
    }

    /// What the last change said, on rows of the screen's width: three at most, the last one cut.
    /// A result signed and colored, a question as it is.
    fn said(&self, width: usize) -> Vec<(String, Style)> {
        let (sign, text, style) = match &self.sheet.said {
            Some(Said::Done(text)) => ("✓ ", text, Style::fg(Color::Green)),
            Some(Said::Failed(text)) => ("✗ ", text, Style::fg(Color::Red)),
            Some(Said::Asked(text)) => ("", text, Style::PLAIN),
            None => return Vec::new(),
        };
        let indent = " ".repeat(columns(sign));
        let rows = wrap(text, width.saturating_sub(2 + columns(sign)), 3);
        rows.into_iter()
            .enumerate()
            .map(|(i, row)| (format!("{}{row}", if i == 0 { sign } else { indent.as_str() }), style))
            .collect()
    }

    fn message(&mut self, y: usize, said: &[(String, Style)]) {
        for (i, (row, style)) in said.iter().enumerate() {
            self.c.put(1, y + i, row, *style);
        }
    }

    fn actions(&self) -> Vec<Action> {
        self.sheet.actions()
    }

    /// The team's actions, as buttons with their key.
    fn buttons(&mut self, y: usize, width: usize) {
        let mut x = 1;
        for action in self.actions() {
            let text = action_label(action, self.sheet.team.dashboard, true);
            let w = 2 + columns(&text) + 2;
            if x + w >= width {
                break;
            }
            let start = x;
            x = self.c.put(x, y, &format!(" {}", action.key()), Style::fg(Color::Cyan).bold().on(BUTTON));
            x = self.c.put(x, y, &format!(" {text} "), Style::PLAIN.on(BUTTON));
            self.zone(start, y, x - start, 1, Target::Action(action));
            x += 1;
        }
    }

    /// The team's actions in a narrow window: their key and a word.
    fn action_keys(&mut self, y: usize, width: usize) {
        // Short of room, the least needed go first: detaching and stopping stay.
        let mut actions = self.actions();
        let room = |actions: &[Action]| -> usize {
            actions.iter().map(|a| columns(&action_label(*a, self.sheet.team.dashboard, false)) + 4).sum::<usize>() + 1
        };
        for spare in [Action::RestartAll, Action::Dashboard, Action::New] {
            if room(&actions) <= width {
                break;
            }
            actions.retain(|a| *a != spare);
        }
        let mut x = 1;
        for action in actions {
            let text = action_label(action, self.sheet.team.dashboard, false);
            let w = columns(&text) + 2;
            if x + w >= width {
                break;
            }
            let start = x;
            x = self.c.put(x, y, &action.key().to_string(), key_style());
            x = self.c.put(x, y, &format!(" {text}"), muted());
            self.zone(start, y, x - start, 1, Target::Action(action));
            x += 2;
        }
    }

    /// The keys that act now.
    fn keys(&mut self, y: usize, width: usize) {
        let word = if self.look.option { "⌥⌫" } else { "Alt+⌫" };
        let esc = t!("Échap", "Esc");
        let list: Vec<(String, String)> = match &self.sheet.overlay {
            Overlay::List(_) => vec![
                ("↑↓".into(), t!("choisir", "choose")),
                ("⏎".into(), t!("valider", "apply")),
                (esc, t!("ferme la liste", "closes the list")),
            ],
            Overlay::Typing(_) => vec![
                ("⏎".into(), t!("valider", "apply")),
                (esc, t!("annuler", "cancel")),
                (word.into(), t!("efface un mot", "erases a word")),
            ],
            Overlay::Confirm(_) => Vec::new(),
            Overlay::None => match self.sheet.field() {
                // Nothing to change: the files do not read.
                None if self.sheet.focusable().is_empty() => {
                    vec![("↑↓".into(), t!("membre", "member")), (esc, t!("ferme", "closes"))]
                }
                None => vec![
                    ("↑↓".into(), t!("membre", "member")),
                    ("⏎".into(), t!("sa fiche", "its sheet")),
                    (esc, t!("ferme", "closes")),
                ],
                Some(fid) if self.sheet.pending.is_some_and(|(f, _)| f == fid) => vec![
                    ("⏎".into(), t!("valider", "apply")),
                    ("←→".into(), t!("valeur", "value")),
                    (esc, t!("abandonner", "give up")),
                ],
                Some(fid) if fid.cycles() => vec![
                    ("↑↓".into(), t!("champ", "field")),
                    ("←→".into(), t!("valeur", "value")),
                    ("⏎".into(), t!("liste des valeurs", "list of values")),
                    (esc, t!("retour à la liste", "back to the list")),
                ],
                Some(Fid::Restart) => vec![
                    ("↑↓".into(), t!("champ", "field")),
                    ("←→".into(), t!("choisir", "choose")),
                    ("⏎".into(), t!("relancer", "restart")),
                    (esc, t!("retour à la liste", "back to the list")),
                ],
                Some(_) => vec![
                    ("↑↓".into(), t!("champ", "field")),
                    ("⏎".into(), t!("modifier", "change")),
                    (esc, t!("retour à la liste", "back to the list")),
                ],
            },
        };
        let mut x = 1;
        for (key, text) in list {
            if x + columns(&key) + 1 + columns(&text) >= width {
                break;
            }
            x = self.c.put(x, y, &key, key_style());
            x = self.c.put(x, y, &format!(" {text}"), muted()) + 3;
        }
    }

    /// The values of a row, open under it, or above it when there is no room below.
    fn dropdown(&mut self, list: &Dropdown, split: usize, bottom: usize) {
        let width = self.c.width();
        let x0 = split + 2;
        let value_x = x0 + if self.wide { 15 } else { 14 };
        let label_width = list.items.iter().map(|i| columns(&i.label)).max().unwrap_or(0);
        let detail_width = list.items.iter().filter_map(|i| i.detail.as_deref().map(columns)).max().unwrap_or(0);
        let current = t!("actuel", "current");
        let wanted = 6 + label_width + if detail_width > 0 { 2 + detail_width } else { 0 } + 2 + columns(&current) + 2;
        let x = value_x.saturating_sub(2).min(width.saturating_sub(wanted.min(width - split - 2) + 1)).max(split + 1);
        let box_width = wanted.min(width - x - 1);
        let field_row = self
            .zones
            .iter()
            .find(|z| matches!(z.target, Target::Row(i) if self.sheet.focusable().get(i) == Some(&list.field)))
            .map_or(2, |z| z.y);
        let rows = list.items.len().min(bottom.saturating_sub(4));
        let height = rows + 2;
        let y = if field_row + 1 + height <= bottom { field_row + 1 } else { field_row.saturating_sub(height).max(2) };
        self.c.frame(x, y, box_width, height, muted());
        let first = list.at.saturating_sub(rows.saturating_sub(1));
        for (row, (i, item)) in list.items.iter().enumerate().skip(first).take(rows).enumerate() {
            let iy = y + 1 + row;
            let on = i == list.at;
            let inner_end = x + box_width - 1;
            if on {
                self.c.tint(x + 1, iy, box_width - 2, SELECTED);
                self.c.put(x + 2, iy, "›", key_style());
            }
            let style = if on {
                Style::PLAIN.bold()
            } else if item.value.is_none() && !item.other {
                Style::PLAIN.dim()
            } else {
                Style::PLAIN
            };
            self.c.put_in(x + 4, iy, inner_end, &item.label, style);
            if let Some(detail) = &item.detail {
                // Its own column, and room for « current » on the current one's row.
                let detail_x = x + 6 + label_width;
                let marked = if list.current == Some(i) { columns(&current) + 2 } else { 0 };
                let room = inner_end.saturating_sub(detail_x + 1 + marked);
                self.c.put_in(detail_x, iy, inner_end, &fit(detail, room), muted());
            }
            if list.current == Some(i) {
                self.c.put_right(inner_end - 1, iy, &current, muted());
            }
            if on {
                self.c.tint(x + 1, iy, box_width - 2, SELECTED);
            }
            self.zone(x + 1, iy, box_width - 2, 1, Target::Item(i));
        }
    }

    /// A question at the centre, the rest faded.
    fn confirm(&mut self, confirm: &Confirm, width: usize, height: usize) {
        let box_width = 64.min(width - 4);
        let inner = box_width - 6;
        let mut lines: Vec<(String, Style)> = Vec::new();
        for line in &confirm.lines {
            for part in wrap(line, inner, 4) {
                lines.push((part, Style::PLAIN));
            }
        }
        // Those it interrupts, each state in its sign and color: yellow at work, red waiting for the user.
        if !confirm.busy.is_empty() {
            lines.push((String::new(), Style::PLAIN));
        }
        for (state, said) in &confirm.busy {
            let sign = state.icon(self.look.glyphs, self.look.frame);
            for (i, part) in wrap(said, inner - 2, 3).into_iter().enumerate() {
                let text = if i == 0 { format!("{sign} {part}") } else { format!("  {part}") };
                lines.push((text, Style::fg(state.color())));
            }
        }
        let box_height = (lines.len() + 7).min(height - 2);
        let x = (width - box_width) / 2;
        let y = (height - box_height) / 2;
        self.c.frame(x, y, box_width, box_height, muted());
        let title = fit(&format!(" {} ", confirm.title), box_width - 4);
        self.c.put(x + 2, y, &title, Style::PLAIN.bold());
        for (i, (line, style)) in lines.iter().enumerate() {
            if y + 2 + i >= y + box_height - 4 {
                break;
            }
            self.c.put_in(x + 3, y + 2 + i, x + box_width - 2, line, *style);
        }
        let buttons_y = y + box_height - 4;
        let yes_color = if confirm.danger { Color::Red } else { Color::Cyan };
        let yes_style = if confirm.at == 0 {
            Style::fg(Color::Black).on(yes_color).bold()
        } else {
            Style::fg(yes_color).on(BUTTON)
        };
        let no_style = if confirm.at == 1 { Style::PLAIN.bold().on(SELECTED) } else { Style::PLAIN.on(BUTTON) };
        let end = x + box_width - 2;
        let yes = fit(&format!(" {} ", confirm.yes), box_width / 2);
        let after_yes = self.c.put_in(x + 3, buttons_y, end, &yes, yes_style);
        let no = t!(" Annuler ", " Cancel ");
        let after_no = self.c.put_in(after_yes + 2, buttons_y, end, &no, no_style);
        let esc = t!("Échap", "Esc");
        let mut kx = x + 3;
        for (key, text) in [
            ("←→".to_string(), t!("choisir", "choose")),
            ("⏎".into(), t!("valider", "apply")),
            (esc, t!("annule", "cancels")),
        ] {
            kx = self.c.put_in(kx, buttons_y + 2, end, &key, key_style());
            kx = self.c.put_in(kx, buttons_y + 2, end, &format!(" {text}"), muted()) + 3;
        }
        self.c.fade_outside(x, y, box_width, box_height, Style::fg(FADED));
        // Nothing under the dialog takes a click.
        self.zones.clear();
        self.zone(x + 3, buttons_y, after_yes - (x + 3), 1, Target::Yes);
        self.zone(after_yes + 2, buttons_y, after_no - (after_yes + 2), 1, Target::No);
    }
}

fn config_error(sheet: &Sheet) -> Option<String> {
    if sheet.draft.name.is_empty() {
        return None;
    }
    crate::config::check_new_name(&sheet.team.config, &sheet.draft.name).err()
}

fn state_word(state: State) -> String {
    match state {
        State::Working => t!("au travail", "at work"),
        State::Waiting => t!("en attente", "waiting"),
        State::Idle => t!("au repos", "at rest"),
        State::Other => t!("état inconnu", "unknown state"),
    }
}

fn label(fid: Fid) -> String {
    match fid {
        Fid::Model => t!("Modèle", "Model"),
        Fid::Effort => t!("Effort", "Effort"),
        Fid::Mode => t!("Permission", "Permission"),
        Fid::Contact => t!("Interlocuteur", "Contact"),
        Fid::Tab => t!("Onglet", "Tab"),
        Fid::Name | Fid::NewName => t!("Nom", "Name"),
        Fid::Role | Fid::NewRole => t!("Rôle", "Role"),
        Fid::Instructions | Fid::NewInstructions => t!("Instructions", "Instructions"),
        Fid::Restart => t!("Relancer", "Restart"),
        Fid::Remove => t!("Retirer", "Remove"),
        Fid::Close => t!("Panneau", "Pane"),
        Fid::How => t!("Comment", "How"),
        Fid::Builtin => t!("Rôle intégré", "Built-in role"),
        Fid::Request => t!("Demande", "Request"),
        Fid::Compose => String::new(),
        Fid::Add => String::new(),
    }
}

fn action_label(action: Action, dashboard: bool, long: bool) -> String {
    match (action, long) {
        (Action::New, true) => t!("Nouvel agent", "New agent"),
        (Action::New, false) => t!("agent", "agent"),
        (Action::Dashboard, true) if dashboard => t!("Tableau de bord : oui", "Dashboard: yes"),
        (Action::Dashboard, true) => t!("Tableau de bord : non", "Dashboard: no"),
        (Action::Dashboard, false) => t!("tableau", "dashboard"),
        (Action::RestartAll, true) => t!("Réinitialiser", "Reset"),
        (Action::RestartAll, false) => t!("réinitialiser", "reset"),
        (Action::Detach, true) => t!("Détacher", "Detach"),
        (Action::Detach, false) => t!("détacher", "detach"),
        (Action::Stop, true) => t!("Quitter", "Quit"),
        (Action::Stop, false) => t!("quitter", "quit"),
    }
}

/// `text` on rows of `width` columns at most, broken between words, `lines` at most (the last one cut with « … »).
fn wrap(text: &str, width: usize, lines: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    for word in text.split_whitespace() {
        let wanted = if row.is_empty() { columns(word) } else { columns(&row) + 1 + columns(word) };
        if wanted <= width || row.is_empty() {
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
        } else {
            rows.push(std::mem::take(&mut row));
            row.push_str(word);
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    if rows.len() > lines {
        let rest = rows[lines - 1..].join(" ");
        rows.truncate(lines - 1);
        rows.push(fit(&rest, width));
    }
    rows.into_iter().map(|r| fit(&r, width)).collect()
}

#[cfg(test)]
mod tests {
    use super::super::sheet::fixture::{Fake, team};
    use super::super::sheet::{Key, Sheet};
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

    #[test]
    fn wide() {
        let mut s = sheet();
        s.key(Key::Down, &Fake::quiet());
        let drawn = draw(&s, &look(), 100, 28);
        let text = rows(&drawn);
        assert!(text[0].starts_with(" recruit · "), "{}", text[0]);
        assert!(text[0].ends_with(&t!("Échap ferme", "Esc closes")));
        assert!(text[0].contains("⠋ 1   ⚑ 1   ◷ 1"), "the states counted: {}", text[0]);
        assert_eq!(text[1].chars().nth(34), Some('┬'), "the list 34 wide");
        assert!(text[3].contains("⠋ coordinateur") && text[3].ends_with("") && text[3].contains("3m"), "{}", text[3]);
        assert!(text[4].contains("Opus   ▆ high"), "the model, then the effort: {}", text[4]);
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
        assert!(
            text[20].contains("n ") && text[20].contains(&t!("tableau", "dashboard")),
            "the team's keys: {}",
            text[20]
        );
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
        assert!(!reset || keys.contains(&t!("tableau", "dashboard")), "the least needed went first: {keys}");
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
        assert_eq!(drawn.canvas.style(1, 0).fg, Some(FADED), "the rest faded");
        assert!(
            drawn.zones.iter().all(|z| matches!(z.target, Target::Yes | Target::No)),
            "only its buttons take a click"
        );
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
        assert!(text[23].starts_with(" ✗ mot") && text[24].starts_with("   mot"), "{}", text[23]);
        assert!(text[25].ends_with('…'), "the last row cut: {}", text[25]);
        assert!(text[22].starts_with('─'), "the line over it");
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
}
