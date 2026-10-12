// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The chosen member's sheet on the right: its head, its rows and their values.

use crossterm::style::Color;

use super::super::sheet::{Fid, Overlay, Person, Sheet, Target, Typing};
use crate::t;
use crate::ui::canvas::{Style, columns, fit};
use crate::ui::look::{self, State};

use super::*;

impl Drawer<'_> {
    /// The sheet of the chosen entry, from `left` (the column after the list's line) to the right edge, down to row
    /// `last`: the member's head, then its rows.
    pub(super) fn sheet_rows(&mut self, left: usize, top: usize, width: usize, last: usize) {
        let x0 = left + 1;
        let right = width - 1;
        let y = match self.sheet.person() {
            Some(p) => self.member_head(p, x0, right, top, last),
            None => {
                self.c.put(x0, top, &t!("Nouvel agent", "New agent"), Style::PLAIN.bold());
                top + 2
            }
        };
        self.field_rows(left, top, y, right, last);
    }

    /// A member's head from row `top`: its name and state, its role, why its turn failed, what it is gone, and what a
    /// team whose files do not read runs with; the row after it.
    fn member_head(&mut self, p: &Person, x0: usize, right: usize, top: usize, last: usize) -> usize {
        let sheet = self.sheet;
        self.put_title(p, x0, right, top);
        let mut y = top + 1;
        let role = p.layers.as_ref().map_or(p.role.as_str(), |l| l.role.value.as_str());
        // Its turn failed: since when, and the error whole, as Claude Code gave it.
        let failed = sheet.state(&p.name).filter(|(state, _)| *state == State::Error);
        let error = match (failed, sheet.errors.get(&p.name)) {
            (Some(_), Some(error)) => paragraphs(error, right.saturating_sub(x0 + 2), if self.wide { 2 } else { 1 }),
            _ => Vec::new(),
        };
        let gone = if p.gone {
            let text = t!(
                "« {} » n'est plus dans les fichiers de l'équipe, mais son panneau tourne encore.",
                "\"{}\" is no longer in the team's files, but its pane still runs.",
                p.name
            );
            wrap(&text, right - x0, 3)
        } else {
            Vec::new()
        };
        // What shows above the rows, and what gives way first so that the focused row always shows: the blank under
        // the error, the error's text from its end, the blank under what it is gone, its text, the blank under the
        // role, the role from its end.
        let mut room = Room {
            role: if self.wide { 2 } else { 1 },
            role_gap: 1,
            failed: usize::from(failed.is_some()),
            error: error.len(),
            error_gap: usize::from(failed.is_some()),
            gone: gone.len(),
            gone_gap: usize::from(p.gone),
        };
        let typing_error = matches!(&sheet.overlay, Overlay::Typing(t) if t.error.is_some()) as usize;
        let below = if sheet.rows().is_empty() { 0 } else { 1 + typing_error };
        room.fit(last.saturating_sub(y + below));
        for line in wrap(role.trim(), right - x0, room.role.max(1)).into_iter().take(room.role) {
            self.c.put(x0, y, &line, Style::PLAIN);
            y += 1;
        }
        y = top + 1 + room.role + room.role_gap;
        if let Some((_, since)) = failed {
            y = self.put_failure(x0, right, y, since, &error[..room.error]);
            y += room.error_gap;
        }
        // The team's files do not read: what it runs with, and nothing to change.
        if self.sheet.team.unreadable.is_some() {
            self.put_read_only(p, x0, right, y);
        }
        for line in gone.iter().take(room.gone) {
            self.c.put(x0, y, line, Style::PLAIN);
            y += 1;
        }
        y + room.gone_gap
    }

    /// The member's name on row `y`, and at the right its state with the time it has been in it.
    fn put_title(&mut self, p: &Person, x0: usize, right: usize, y: usize) {
        self.c.put(x0, y, &fit(&p.name, right - x0), Style::PLAIN.bold());
        if let Some((state, since)) = self.sheet.state(&p.name) {
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
    }

    /// « Tour raté il y a … » on row `y`, then the lines of the error; the row after them.
    fn put_failure(&mut self, x0: usize, right: usize, y: usize, since: i64, error: &[String]) -> usize {
        let mut y = y;
        let ago = look::duration(self.look.now.saturating_sub(since).max(0) as u64);
        let orange = Style::fg(State::Error.color()).bold();
        let x = self.c.put(x0, y, &State::Error.icon(self.look.glyphs, 0).to_string(), orange);
        self.c.put_in(x + 1, y, right, &t!("Tour raté il y a {}", "Turn failed {} ago", ago), orange);
        y += 1;
        for line in error {
            self.c.put_in(x0 + 2, y, right, line, muted());
            y += 1;
        }
        y
    }

    /// The model and effort a member runs with, and the note that its settings are read only, from row `y`.
    fn put_read_only(&mut self, p: &Person, x0: usize, right: usize, y: usize) {
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

    /// The rows of the fields from row `y`, down to `last`: a label, a value, an origin in a wide window; each a zone
    /// of a click. Short of room, the gaps between groups go first, then the rows scroll to keep the focused one in
    /// sight (▲ at the top right of the sheet, from `top`).
    fn field_rows(&mut self, left: usize, top: usize, y: usize, right: usize, last: usize) {
        let sheet = self.sheet;
        let x0 = left + 1;
        let mut y = y;
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
                self.c.tint(left, y, right - left, selected());
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
                self.c.tint(left, y, right - left, selected());
            }
            index += 1;
            y += 1;
        }
        if skip > 0 {
            self.c.put(right - 1, top, "▲", muted());
        }
    }

    /// A row's value, from `x` to `end`.
    pub(super) fn value(&mut self, fid: Fid, index: usize, x: usize, y: usize, end: usize, on: bool) {
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
                        if chosen { Style::fg(Color::Black).on(Color::Cyan).bold() } else { Style::PLAIN.on(button()) };
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
                    Style::fg(Color::Red).on(button()),
                );
            }
            Fid::Compose => {
                if sheet.draft.composing.is_some() {
                    let sign = look::SPINNER[self.look.frame % look::SPINNER.len()].to_string();
                    let x = self.c.put_in(x, y, end, &sign, Style::fg(look::palette().working));
                    let text = t!(
                        " Claude compose le nouvel agent… Échap annule",
                        " Claude is composing the new agent… Esc cancels"
                    );
                    self.c.put_in(x, y, end, &text, Style::fg(look::palette().working));
                } else {
                    self.c.put_in(x, y, end, &t!(" Composer ", " Compose "), Style::PLAIN.on(button()));
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
                let level = if fid == Fid::Effort { sheet.effort_level() } else { None };
                let shown = if let Some(level) = level {
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
                        self.c.put_in(
                            shown + 3,
                            y,
                            end,
                            &t!("à valider ⏎", "to apply ⏎"),
                            Style::fg(look::palette().working),
                        );
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
    pub(super) fn input(&mut self, typing: &Typing, x: usize, y: usize, right: usize, last: usize) -> usize {
        let room = right.saturating_sub(x + 1);
        self.c.fill(x, y, room + 1, 1, Style::PLAIN.on(input()));
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
            let style = if i == typing.cursor { Style::PLAIN.reverse() } else { Style::PLAIN.on(input()) };
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
        State::Error => t!("en erreur", "failed"),
        State::Other => t!("état inconnu", "unknown state"),
    }
}

/// `text` over at most `lines` rows of `width` columns: cut at its own line breaks, then between words, its tabs as
/// blanks; the last row ends with « … » when the text goes on. For a text from elsewhere, an error as Claude Code gave
/// it.
pub(super) fn paragraphs(text: &str, width: usize, lines: usize) -> Vec<String> {
    let text = text.replace('\t', " ");
    let mut rows: Vec<String> = Vec::new();
    for paragraph in text.lines().map(str::trim).filter(|p| !p.is_empty()) {
        rows.extend(wrap(paragraph, width, usize::MAX));
    }
    if rows.len() > lines {
        rows.truncate(lines);
        if let Some(last) = rows.last_mut() {
            // « … » after it, or in place of its end when the row is full.
            *last = if columns(last) < width { format!("{last}…") } else { fit(&format!("{last}…"), width) };
        }
    }
    rows
}
