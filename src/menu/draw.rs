// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The menu on a screen of a given size, drawn into a [`Canvas`], and where each click lands: the members on the left
//! as on the dashboard, the chosen one's sheet on the right, the team's actions at the bottom. Narrower than
//! [`WIDE`], the list keeps the names only and the origin of a value goes under the sheet.

use crossterm::style::Color;

use super::sheet::{Action, Fid, Overlay, Sheet, Target};
use crate::t;
use crate::ui::canvas::{Canvas, Style, columns, fit};
use crate::ui::look::{self, Glyphs, State};

/// From this width, the wide layout.
pub(crate) const WIDE: usize = 90;
/// Below this, the menu says the window is too small.
pub(crate) const MIN_WIDTH: usize = 50;
pub(crate) const MIN_HEIGHT: usize = 14;
/// The height from which the team's settings have a row of their own.
const TALL: usize = 20;

/// What dims: the terminal's own grey, which follows its theme.
const MUTED: Color = Color::DarkGrey;

// What recruit colors itself, for a dark terminal or a light one (`look::palette`).
/// The chosen line of a list.
pub(super) fn selected() -> Color {
    look::palette().selected
}

/// The chosen card while the sheet has the focus.
fn chosen_card() -> Color {
    look::palette().chosen
}

pub(super) fn button() -> Color {
    look::palette().button
}

/// A text edited in place.
pub(super) fn input() -> Color {
    look::palette().input
}

/// What a layer dims around it.
fn faded() -> Color {
    look::palette().faded
}

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

pub(super) fn muted() -> Style {
    Style::fg(MUTED)
}

/// `text` from `x` on row `y`, the effort `level`'s label (its sign and its name) in its colors wherever it is in the
/// text, the rest in `style`; the colors bold when `style` is. Returns the column after it.
pub(super) fn put_effort_text(
    c: &mut Canvas,
    x: usize,
    y: usize,
    end: usize,
    text: &str,
    level: &str,
    style: Style,
) -> usize {
    let cells = look::effort_cells(level, true, 0);
    let at = text.find(&look::effort_label(level)).map_or(usize::MAX, |byte| text[..byte].chars().count());
    let mut x = x;
    for (n, ch) in text.chars().enumerate() {
        let color = n.checked_sub(at).and_then(|k| cells.get(k)).and_then(|(_, color)| *color);
        let painted = color.map_or(style, |color| Style { fg: Some(color), ..style });
        x = c.put_in(x, y, end, &ch.to_string(), painted);
    }
    x
}

pub(super) fn key_style() -> Style {
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
    // The team's settings on a row of their own when there is room, above its actions.
    let settings = d.wide && height >= TALL && sheet.actions().iter().any(|a| a.is_setting());
    let bottom = height - 3 - usize::from(settings) - said.len().max(1);
    d.c.vline(split, 2, bottom - 2, '│', muted());
    d.c.hline(0, bottom, width, '─', muted());
    d.c.put(split, bottom, "┴", muted());
    d.list(split, 2, bottom);
    // The hint under the sheet, on two rows when it needs them (« Reprendre » explains what it writes).
    let hint = d.hint_rows(width - 1 - (split + 2 + 2));
    let hint_row = bottom - hint.len().max(1);
    d.sheet_rows(split + 1, 2, width, hint_row);
    d.hint(&hint, split + 2, hint_row, width - 1);
    d.message(bottom + 1, &said);
    // A narrow window with nothing said: the row the message would take holds the team's settings, a key and a value
    // each; with something said, they fall back to the actions' row, where they are the first to go.
    let free = !d.wide && said.is_empty() && sheet.actions().iter().any(|a| a.is_setting());
    if free {
        d.setting_keys(bottom + 1, width);
    }
    if settings {
        let (set, act): (Vec<Action>, Vec<Action>) = d.actions().into_iter().partition(|a| a.is_setting());
        d.buttons(height - 3, width, &set);
        d.buttons(height - 2, width, &act);
    } else if d.wide {
        // One row: the settings that do not fit are the first to go.
        let mut act = d.actions();
        act.sort_by_key(|a| matches!(a, Action::Grid | Action::AutoCompact));
        d.buttons(height - 2, width, &act);
    } else {
        d.action_keys(height - 2, width, free);
    }
    d.keys(height - 1, width);
    match &sheet.overlay {
        Overlay::List(list) => d.dropdown(list, split, bottom),
        Overlay::Confirm(confirm) => d.confirm(confirm, width, height),
        _ => {}
    }
    Drawn { canvas: d.c, zones: d.zones, split }
}

/// The rows a member's sheet shows above its fields: its role and the blank under it, a failed turn (its title, its
/// error, a blank), what it is gone and a blank.
struct Room {
    role: usize,
    role_gap: usize,
    failed: usize,
    error: usize,
    error_gap: usize,
    gone: usize,
    gone_gap: usize,
}

impl Room {
    fn used(&self) -> usize {
        self.role + self.role_gap + self.failed + self.error + self.error_gap + self.gone + self.gone_gap
    }

    /// Down to `rows` rows, giving way in turn: the blank under the error, the error from its end, the blank under
    /// what it is gone, that text from its end, the blank under the role, the role from its end. The failed turn's
    /// title stays.
    fn fit(&mut self, rows: usize) {
        let order: [fn(&mut Room) -> &mut usize; 6] = [
            |r| &mut r.error_gap,
            |r| &mut r.error,
            |r| &mut r.gone_gap,
            |r| &mut r.gone,
            |r| &mut r.role_gap,
            |r| &mut r.role,
        ];
        for part in order {
            while self.used() > rows && *part(self) > 0 {
                *part(self) -= 1;
            }
        }
    }
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
        for state in [State::Working, State::Waiting, State::Error, State::Idle] {
            let n = self
                .sheet
                .team
                .people
                .iter()
                .filter(|p| self.sheet.state(&p.name).map(|(s, _)| s) == Some(state))
                .count();
            // A failed turn, only while there is one.
            if state != State::Error || n > 0 {
                counts.push((state, n));
            }
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
}

fn label(fid: Fid) -> String {
    match fid {
        Fid::Resume => t!("Reprendre", "Resume"),
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

/// `text` on rows of `width` columns at most, broken between words, `lines` at most (the last one cut with « … »).
pub(super) fn wrap(text: &str, width: usize, lines: usize) -> Vec<String> {
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

mod footer;
mod list;
mod overlay;
mod sheet;
#[cfg(test)]
mod tests;
