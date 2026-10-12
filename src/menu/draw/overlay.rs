// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What opens over the sheet: a list of values under its row, and the confirmations at the center.

use crossterm::style::Color;

use super::super::sheet::{Action, Confirm, Dropdown, Purpose, Target};
use crate::t;
use crate::ui::canvas::{Style, columns, fit};

use super::*;

impl Drawer<'_> {
    /// The values of a row, open under it, or above it when there is no room below.
    pub(super) fn dropdown(&mut self, list: &Dropdown, split: usize, bottom: usize) {
        let width = self.c.width();
        let x0 = split + 2;
        let value_x = x0 + if self.wide { 15 } else { 14 };
        let label_width = list.items.iter().map(|i| columns(&i.label)).max().unwrap_or(0);
        let detail_width = list.items.iter().filter_map(|i| i.detail.as_deref().map(columns)).max().unwrap_or(0);
        let current = t!("actuel", "current");
        let wanted = 6 + label_width + if detail_width > 0 { 2 + detail_width } else { 0 } + 2 + columns(&current) + 2;
        let x = value_x.saturating_sub(2).min(width.saturating_sub(wanted.min(width - split - 2) + 1)).max(split + 1);
        let box_width = wanted.min(width - x - 1);
        // A team setting opens above its button; a row of the sheet, under it or above it.
        let button = match list.purpose {
            Purpose::Grid => Some(Action::Grid),
            Purpose::Compaction => Some(Action::AutoCompact),
            _ => None,
        };
        let button = button.and_then(|a| self.zones.iter().find(|z| z.target == Target::Action(a))).map(|z| (z.x, z.y));
        let field_row = self
            .zones
            .iter()
            .find(|z| matches!(z.target, Target::Row(i) if self.sheet.focusable().get(i) == list.field.as_ref()))
            .map_or(2, |z| z.y);
        let rows = list.items.len().min(bottom.saturating_sub(4));
        let height = rows + 2;
        let (x, y) = match button {
            Some((bx, by)) => (bx.min(width.saturating_sub(box_width + 1)).max(1), by.saturating_sub(height).max(2)),
            None if field_row + 1 + height <= bottom => (x, field_row + 1),
            None => (x, field_row.saturating_sub(height).max(2)),
        };
        self.c.frame(x, y, box_width, height, muted());
        let first = list.at.saturating_sub(rows.saturating_sub(1));
        for (row, (i, item)) in list.items.iter().enumerate().skip(first).take(rows).enumerate() {
            let iy = y + 1 + row;
            let on = i == list.at;
            let inner_end = x + box_width - 1;
            if on {
                self.c.tint(x + 1, iy, box_width - 2, selected());
                self.c.put(x + 2, iy, "›", key_style());
            }
            let style = if on {
                Style::PLAIN.bold()
            } else if item.value.is_none() && !item.other {
                Style::PLAIN.dim()
            } else {
                Style::PLAIN
            };
            match &item.level {
                Some(level) => {
                    put_effort_text(&mut self.c, x + 4, iy, inner_end, &item.label, level, style);
                }
                None => {
                    self.c.put_in(x + 4, iy, inner_end, &item.label, style);
                }
            }
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
                self.c.tint(x + 1, iy, box_width - 2, selected());
            }
            self.zone(x + 1, iy, box_width - 2, 1, Target::Item(i));
        }
    }

    /// A question at the centre, the rest faded.
    pub(super) fn confirm(&mut self, confirm: &Confirm, width: usize, height: usize) {
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
            Style::fg(yes_color).on(button())
        };
        let no_style = if confirm.at == 1 { Style::PLAIN.bold().on(selected()) } else { Style::PLAIN.on(button()) };
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
        self.c.fade_outside(x, y, box_width, box_height, Style::fg(faded()));
        // Nothing under the dialog takes a click.
        self.zones.clear();
        self.zone(x + 3, buttons_y, after_yes - (x + 3), 1, Target::Yes);
        self.zone(after_yes + 2, buttons_y, after_no - (after_yes + 2), 1, Target::No);
    }
}
