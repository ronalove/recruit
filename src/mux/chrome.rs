// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The chrome around the panes, direction B « Cadres » (CLAUDE.md, decision of 2026-10-09; mock-up
//! https://claude.ai/artifact/MbjmHDAzeN3mNK9jdqmXQ7): each pane in a frame, its header written in the top border.
//! The pane with the focus has a thick frame in bold, in the terminal's own text color, at work as at rest; the
//! others thin and grey. Only red stays, for a member waiting and for a pane gone wrong, which ask for an action:
//! thick with the focus, thin without (the active pane, 2026-10-09, option A of
//! https://claude.ai/artifact/Vy6KJsgaxqDUwNuXMLFrt5, as the user corrected it). The header keeps the state's
//! styles: the spinner by the name of a member at work, ⚑ in red for one waiting, the name in the member's color.
//! At its right, the model, the effort, the context and the time, then ⤢ in the corner (step 3, F1); each part a
//! click reaches comes back from [`frame`], the one under the mouse underlined (F3). Then the bar on the screen's
//! last row: the team, the tabs (the current one in reverse video, each after the sign of its most urgent member: ⚑
//! for one waiting), then the menu and quit buttons. A notice at the top right when a member out of sight waits
//! (F5); the choices at the center.
//!
//! Owner: dev-interface. The signatures are the interface with `screen.rs` (dev-rendu) and the server (dev-serveur).

use crossterm::style::Color;

use super::Rect;
use crate::canvas::{Canvas, Style, columns, fit};
use crate::look::ALT;
use crate::look::{Glyphs, Pressure, State};
use crate::t;

/// How the chrome draws its signs: Nerd Font or Unicode, and the spinner's image (`look::frame`) for the members at
/// work. While a header or a tab shows one at work, the screen is drawn again every `look::FRAME`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Look {
    pub glyphs: Glyphs,
    pub frame: usize,
}

/// What a pane's frame says of it. `Default` for the fields a caller leaves out: `..Header::default()`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Header<'a> {
    /// The member's name, or the panel's title.
    pub name: &'a str,
    /// The member's color, as on the dashboard and in the journal; `None` for a panel (dashboard, journal): its
    /// title plain, its frame grey, no state.
    pub color: Option<Color>,
    /// Its member's state: from its program (OSC 7501) when it says it, else from `claude agents`.
    pub state: State,
    /// What went wrong with the pane, after the name, in red, its frame red too: ended, engine failed.
    pub note: Option<&'a str>,
    /// How far up its history it shows, and how long that is (`screen::compose` fills it), after the note, dimmed.
    pub scroll: Option<(usize, usize)>,
    /// A key that acts on the pane, at the right of the top border, dimmed (mock-up B1), when the rest leaves room.
    pub hint: Option<Hint>,
    /// The member's model, by its family as Claude Code names it (Opus, Sonnet, Haiku, Fable), dimmed.
    pub model: Option<&'a str>,
    /// Its effort (low … max): its sign and word, in Claude Code's colors.
    pub effort: Option<&'a str>,
    /// Its context, in percent of its window, as its card shows it.
    pub context: Option<u8>,
    /// How near it is to compacting on its own: its context orange, then red.
    pub pressure: Pressure,
    /// At rest with its context known: « ⟳ » before the context, which a click compacts.
    pub compactable: bool,
    /// Seconds in its state, written short (12s, 22m, 1h05); with a command running, since the command started.
    pub since: Option<u64>,
    /// At rest while a command it started still runs (decision of 2026-10-09, as on its card): a terminal's sign
    /// instead of the state's, dimmed. Work, when it comes back, prevails.
    pub shell: bool,
    /// The zoom's sign in the corner, ⤢ or ⤡; `None` for a pane that is not a member's.
    pub zoom: Option<Zoom>,
    /// The part under the mouse: the name underlined and « réglages › » after it; another part underlined.
    pub hover: Option<Part>,
    /// The richest form its right part may take, from 0 (all of it) to 5 (none): the poorest that [`form`] gives
    /// the members' panes as wide as its own, for their columns to line up (designer, 2026-10-10). 0 by default.
    pub form: usize,
}

/// A part of a header that a click reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    /// The name, and « réglages › » while hovered: the member's sheet in the menu.
    Name,
    /// The model: the sheet, on its model.
    Model,
    /// The effort: the sheet, on its effort.
    Effort,
    /// The context of a member at rest (« ⟳ »): the compaction's confirmation.
    Context,
    /// ⤢ or ⤡: the zoom, and back.
    Zoom,
}

/// The zoom's sign in a member's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Zoom {
    /// ⤢: the pane can take its tab's room.
    In,
    /// ⤡: it does; back to the grid.
    Out,
}

/// What a panel's border says at its right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hint {
    /// The journal: ⌥j takes it to its next size.
    JournalSize,
    /// The dashboard: how many members work, wait for the user and rest (« ⠹ 5  ⚑ 1  ◷ 3 », mock-up B1).
    Counts { working: usize, waiting: usize, idle: usize },
}

impl Hint {
    /// Its pieces, each in its style: the key dimmed; each count's sign and figure in its state's color, none for a
    /// state no one is in.
    fn pieces(self, look: Look) -> Vec<(String, Style)> {
        match self {
            Hint::JournalSize => vec![(t!("{}j taille", "{}j size", ALT), Style::PLAIN.dim())],
            Hint::Counts { working, waiting, idle } => {
                let mut pieces = Vec::new();
                for (state, count) in [(State::Working, working), (State::Waiting, waiting), (State::Idle, idle)] {
                    if count == 0 {
                        continue;
                    }
                    if !pieces.is_empty() {
                        pieces.push(("  ".to_string(), Style::PLAIN));
                    }
                    let color = Style::fg(state.color());
                    pieces.push((state.badge(look.glyphs, look.frame).to_string(), color));
                    pieces.push((format!(" {count}"), color));
                }
                pieces
            }
        }
    }
}

/// A tab in the bar.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Tab<'a> {
    pub title: &'a str,
    pub active: bool,
    /// The most urgent of its members' states: waiting, else working, else at rest.
    pub state: State,
    /// The member whose pane takes the tab's room, zoomed: « ⤢ name » after the title.
    pub zoomed: Option<&'a str>,
}

/// What a click on the bar reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Tab(usize),
    Menu,
    Quit,
}

/// Rows and columns the frame takes from a pane: its border all around.
pub(crate) const FRAME: usize = 1;

/// A pane's cells inside its frame: its engine and its PTY have this size.
pub(crate) fn inside(area: Rect) -> Rect {
    Rect {
        x: area.x + FRAME,
        y: area.y + FRAME,
        width: area.width.saturating_sub(2 * FRAME),
        height: area.height.saturating_sub(2 * FRAME),
    }
}

/// The corners and lines of a frame: round and thin, or square and thick.
struct Lines {
    top_left: char,
    top_right: char,
    bottom_left: char,
    bottom_right: char,
    across: char,
    down: char,
}

const THIN: Lines =
    Lines { top_left: '╭', top_right: '╮', bottom_left: '╰', bottom_right: '╯', across: '─', down: '│' };
const THICK: Lines =
    Lines { top_left: '┏', top_right: '┓', bottom_left: '┗', bottom_right: '┛', across: '━', down: '┃' };

/// Columns kept at the right of the top border for the zoom's sign (« ⤢ » between two blanks, then a dash and the
/// corner): the header never runs into them, so that it stays put when the zoom comes (step 3).
const CORNER: usize = 5;

/// A frame's style: red for a pane gone wrong and for a member waiting, with the focus or without (they ask for an
/// action: the focus shows by the thickness); else the terminal's text color for the focus; grey for the rest, the
/// dashboard and the journal included. Bold with the focus.
fn frame_style(header: &Header<'_>, focused: bool) -> Style {
    let waiting = header.color.is_some() && header.state == State::Waiting;
    let style = if header.note.is_some() || waiting {
        Style::fg(Color::Red)
    } else if focused && header.color.is_some() {
        Style::PLAIN
    } else {
        Style::fg(Color::DarkGrey)
    };
    if focused { style.bold() } else { style }
}

/// The sign of a state, as on the dashboard: the spinner in yellow, ⚑ in bold red, the clock in grey.
fn sign_style(state: State) -> Style {
    match state {
        State::Working => Style::fg(Color::Yellow),
        State::Waiting => Style::fg(Color::Red).bold(),
        State::Idle | State::Other => Style::fg(Color::DarkGrey),
    }
}

/// A pane's frame around `area`, its header in the top border; thick when it has the focus. Inside is left as it is:
/// the pane's cells go there ([`inside`]).
pub(crate) fn frame(
    canvas: &mut Canvas,
    area: Rect,
    header: &Header<'_>,
    focused: bool,
    look: Look,
) -> Vec<(Rect, Part)> {
    let Rect { x, y, width, height } = area;
    if width < 2 || height < 2 {
        return Vec::new();
    }
    let lines = if focused { THICK } else { THIN };
    let style = frame_style(header, focused);
    let (right, bottom) = (x + width - 1, y + height - 1);
    canvas.hline(x + 1, y, width - 2, lines.across, style);
    canvas.hline(x + 1, bottom, width - 2, lines.across, style);
    canvas.vline(x, y + 1, height - 2, lines.down, style);
    canvas.vline(right, y + 1, height - 2, lines.down, style);
    for (col, row, corner) in [
        (x, y, lines.top_left),
        (right, y, lines.top_right),
        (x, bottom, lines.bottom_left),
        (right, bottom, lines.bottom_right),
    ] {
        canvas.put(col, row, corner.encode_utf8(&mut [0; 4]), style);
    }
    title(canvas, area, header, look)
}

/// The header in the top border, from its third column: « ⠹ name · alert · ↑ 214 sur 3000 », each part between
/// blanks that cut the line; at the right « Opus ▆ high · ⟳ 22 % · 18m », then ⤢ in the corner (mock-up B2, B7).
/// An alert comes first, whole, else short (what comes before its « : »), the name cut for it down to three columns,
/// and cut itself last; then the name, whole while it holds. What is left goes, short of room, in turn (designer,
/// 2026-10-09): the effort's word, the model, the time; the scroll, whole then short (« ↑ 214 »); the effort's sign;
/// the context. Under three columns of the name, the border goes without a header. Returns the parts a click reaches.
fn title(canvas: &mut Canvas, area: Rect, header: &Header<'_>, look: Look) -> Vec<(Rect, Part)> {
    let mut parts = Vec::new();
    let y = area.y;
    let right = area.x + area.width - 1;
    let metas = metas(header, look);
    let plan = plan(area.width, header, &metas);
    if plan.corner
        && let Some(zoom) = header.zoom
    {
        let on = header.hover == Some(Part::Zoom);
        let style = if on { Style::fg(Color::DarkCyan).bold() } else { Style::fg(Color::DarkGrey) };
        canvas.put(right - 4, y, " ", Style::PLAIN);
        canvas.put(right - 3, y, zoom_sign(zoom, look.glyphs).encode_utf8(&mut [0; 4]), style);
        canvas.put(right - 2, y, " ", Style::PLAIN);
        parts.push((Rect { x: right - 4, y, width: 3, height: 1 }, Part::Zoom));
    }
    let Some(name) = &plan.name else { return parts };
    let mut x = canvas.put(area.x + 2, y, " ", Style::PLAIN);
    let name_style = match header.color {
        Some(color) => {
            // Ended or failed, nothing runs there: a cross rather than the state's sign.
            let (sign, style) = match header.note {
                Some(_) => (failed(look.glyphs), Style::fg(Color::Red)),
                None if header.shell && header.state != State::Working => {
                    (look.glyphs.console(), sign_style(State::Idle))
                }
                None => (header.state.icon(look.glyphs, look.frame), sign_style(header.state)),
            };
            x = canvas.put(x, y, sign.encode_utf8(&mut [0; 4]), style);
            x = canvas.put(x, y, " ", Style::PLAIN);
            match header.state {
                State::Idle | State::Other => Style::fg(color).dim(),
                State::Working | State::Waiting => Style::fg(color).bold(),
            }
        }
        None => Style::PLAIN,
    };
    let hovered = |part: Part, style: Style| if header.hover == Some(part) { underlined(style) } else { style };
    let name_at = x;
    let mut name_end = canvas.put(x, y, name, hovered(Part::Name, name_style));
    x = canvas.put(name_end, y, " ", Style::PLAIN);
    if plan.settings {
        name_end = canvas.put(x, y, &settings(), Style::fg(Color::DarkCyan));
        x = canvas.put(name_end, y, " ", Style::PLAIN);
    }
    if header.color.is_some() && name_end > name_at {
        parts.push((Rect { x: name_at, y, width: name_end - name_at, height: 1 }, Part::Name));
    }
    if let Some(text) = &plan.alert {
        x = put_part(canvas, x, y, text, Style::fg(Color::Red));
    }
    if let Some(text) = &plan.scroll {
        x = put_part(canvas, x, y, text, Style::PLAIN.dim());
    }
    if let Some(meta) = plan.meta.map(|i| &metas[i]) {
        let width: usize = meta.iter().map(|(text, ..)| columns(text)).sum();
        let mut at = canvas.put(area.x + plan.stop - width - 1, y, " ", Style::PLAIN);
        for (text, style, part) in meta {
            let from = at;
            let style = part.map_or(*style, |part| hovered(part, *style));
            at = canvas.put(at, y, text, style);
            if let Some(part) = part {
                match parts.last_mut() {
                    // A part in pieces (the effort's sign and word, the context's ⟳ and figure): one zone.
                    Some((rect, last)) if last == part && rect.x + rect.width == from => rect.width += at - from,
                    _ => parts.push((Rect { x: from, y, width: at - from, height: 1 }, *part)),
                }
            }
        }
        canvas.put(at, y, " ", Style::PLAIN);
    }
    // The hint at the right between blanks, a dash before the corner, after a dash at least.
    if let Some(hint) = header.hint {
        let pieces = hint.pieces(look);
        let width = pieces.iter().map(|(text, _)| columns(text)).sum::<usize>() + 2;
        let start = right.saturating_sub(1 + width);
        if !pieces.is_empty() && start > x {
            let mut at = canvas.put(start, y, " ", Style::PLAIN);
            for (text, style) in pieces {
                at = canvas.put(at, y, &text, style);
            }
            canvas.put(at, y, " ", Style::PLAIN);
        }
    }
    parts
}

/// « réglages › », after a name under the mouse.
fn settings() -> String {
    t!("réglages ›", "settings ›")
}

/// Where a header goes in a pane `width` columns wide, its columns counted from the pane's left edge.
#[derive(Debug, Default)]
struct Plan {
    /// The zoom's corner kept: ⤢ there if the header has one.
    corner: bool,
    /// Where the right part ends, its last blank excluded.
    stop: usize,
    /// The name as it shows, cut if need be; none when not even three columns of it hold.
    name: Option<String>,
    /// « réglages › » after it.
    settings: bool,
    alert: Option<String>,
    scroll: Option<String>,
    /// The form of the right part shown, by its place in `metas`.
    meta: Option<usize>,
}

/// The header's layout in a pane `width` columns wide, with the forms `metas` of its right part: what [`title`]
/// draws, without drawing it.
fn plan(width: usize, header: &Header<'_>, metas: &[Vec<Piece>; 5]) -> Plan {
    let right = width.saturating_sub(1);
    let start = 2;
    let lead = match header.color {
        Some(_) => 3,
        None => 1,
    };
    // The header's columns end before the zoom's corner, or a dash before the frame's corner in a narrow pane.
    // An alert takes the zoom's corner too: the zoom goes first when the alert lacks room, whole beside the whole
    // name (zooming on a pane gone wrong helps read what it said).
    let corner = width >= 16
        && header.note.is_none_or(|note| {
            header.zoom.is_some() && start + lead + columns(header.name) + 1 + part(note) <= right + 1 - CORNER
        });
    let end = if corner { right + 1 - CORNER } else { right.saturating_sub(1) };
    // The right part's last blank, then a dash: before the zoom's blank, or before the frame's corner.
    let stop = if corner { end - 2 } else { end.saturating_sub(1) };
    let mut plan = Plan { corner, stop, ..Plan::default() };
    // At least three columns of the name, and a blank; no name, no header: the menu's layer, whose first line says
    // « recruit » already (architect, 2026-10-10).
    if end < start + lead + 3 + 1 || header.name.is_empty() {
        return plan;
    }
    let mut x = start + lead;
    let alert = header.note.and_then(|note| alert(note, columns(header.name) + 1, end - x));
    let alert_room = alert.as_ref().map_or(0, |text| part(text));
    // Beside the zoom, the name's blank is the zoom's own: one blank between them, not two.
    let limit = if corner && header.zoom.is_some() && alert.is_none() { end + 1 } else { end - alert_room };
    let name = fit(header.name, limit.saturating_sub(x + 1));
    x += columns(&name) + 1;
    plan.name = Some(name);
    // Hovered, « réglages › » after the name, while it leaves the rest some room (mock-up B2).
    if header.hover == Some(Part::Name) && header.note.is_none() && x + columns(&settings()) + 1 + SETTINGS_ROOM <= stop
    {
        plan.settings = true;
        x += columns(&settings()) + 1;
    }
    x += alert.as_ref().map_or(0, |text| part(text));
    // The scroll only beside a whole alert, if any: a shortened one leaves it no room.
    let whole_alert = alert.as_deref().is_none_or(|text| Some(text) == header.note);
    let scrolls: Vec<String> = match header.scroll.filter(|_| whole_alert) {
        Some((up, of)) => vec![t!("↑ {} sur {}", "↑ {} of {}", up, of), format!("↑ {up}")],
        None => Vec::new(),
    };
    let (meta, scroll) = layout(x, end, stop, metas, &scrolls, header.form);
    plan.alert = alert;
    plan.meta = meta;
    plan.scroll = scroll.map(|i| scrolls[i].clone());
    plan
}

/// The form the right part of `header` takes on its own in a pane `width` columns wide, from 0 (all of it) to 5
/// (none), neither hovered nor scrolled. 0 for a header with nothing to show there: it holds whole. The panes as wide
/// share their [`common_form`].
pub(crate) fn form(header: &Header<'_>, width: usize) -> usize {
    let alone = Header { hover: None, scroll: None, form: 0, ..*header };
    let metas = metas(&alone, Look::default());
    if metas.iter().all(Vec::is_empty) {
        return 0;
    }
    plan(width, &alone, &metas).meta.unwrap_or(5)
}

/// The [`Header::form`] of the members' panes as wide, from their own [`form`]s: the poorest, for their columns to
/// line up as the dashboard's cards of a width do; but a pane with nothing left at the right (5, a very long name)
/// holds no one back, and gets by alone (architect, 2026-10-10).
pub(crate) fn common_form(forms: impl IntoIterator<Item = usize>) -> usize {
    forms.into_iter().filter(|&form| form < 5).max().unwrap_or(0)
}

/// Whether a header shows its time in the state in a pane `width` columns wide (its whole area, as [`frame`] gets
/// it): while it does under a minute, its seconds change, and the screen must be drawn again each second.
pub(crate) fn shows_since(header: &Header<'_>, width: usize) -> bool {
    header.since.is_some() && plan(width, header, &metas(header, Look::default())).meta.is_some_and(|form| form < 3)
}

/// Columns « réglages › » leaves at least for the rest of the header, in the mock-up.
const SETTINGS_ROOM: usize = 12;

/// A piece of the header's right part: its text, its style, and the part a click on it reaches.
type Piece = (String, Style, Option<Part>);

/// The forms of the header's right part, from the richest: all; without the effort's word; without the model; without
/// the time; the context alone. Each empty for a pane that has none of what it shows.
fn metas(header: &Header<'_>, look: Look) -> [Vec<Piece>; 5] {
    let sep = || (" · ".to_string(), Style::fg(Color::DarkGrey), None);
    let resting = matches!(header.state, State::Idle | State::Other);
    let model = header.model.map(|model| (model.to_string(), Style::PLAIN.dim(), Some(Part::Model)));
    let effort = |word: bool| -> Vec<Piece> {
        let Some(level) = header.effort else { return Vec::new() };
        let sign = crate::look::effort_sign(level);
        let text = if word { format!("{sign} {level}") } else { sign.to_string() };
        match crate::look::effort_color(level) {
            Some(color) => vec![(text, Style::fg(color), Some(Part::Effort))],
            // `max` in a rainbow, turning with the spinners, as on the dashboard.
            None => text
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    // Still out of work: an image drawn for something else does not change its colors.
                    let turn = if header.state == State::Working { look.frame / 2 } else { 0 };
                    let color = crate::look::RAINBOW[(i + turn) % crate::look::RAINBOW.len()];
                    (c.to_string(), Style::fg(color), Some(Part::Effort))
                })
                .collect(),
        }
    };
    let context: Vec<Piece> = match header.context {
        Some(percent) => {
            let style = match header.pressure {
                Pressure::Warning => Style::fg(Color::Red).bold(),
                Pressure::Near => Style::fg(crate::look::ORANGE),
                Pressure::Calm if resting => Style::PLAIN.dim(),
                Pressure::Calm => Style::PLAIN,
            };
            let part = header.compactable.then_some(Part::Context);
            let mut pieces = Vec::new();
            if header.compactable {
                pieces.push(("⟳ ".to_string(), Style::PLAIN.dim(), part));
            }
            pieces.push((format!("{percent} %"), style, part));
            pieces
        }
        None => Vec::new(),
    };
    let time = header.since.map(|secs| {
        let style = if header.state == State::Waiting { Style::fg(Color::Red) } else { Style::PLAIN.dim() };
        vec![(crate::look::duration(secs), style, None)]
    });
    // Groups joined by « · »: the model and the effort, the context, the time.
    let join = |with_model: bool, word: bool, sign: bool, time_too: bool| -> Vec<Piece> {
        let mut first: Vec<Piece> = Vec::new();
        if with_model && let Some(model) = &model {
            first.push(model.clone());
        }
        if sign {
            let effort = effort(word);
            if !effort.is_empty() && !first.is_empty() {
                first.push((" ".to_string(), Style::PLAIN, None));
            }
            first.extend(effort);
        }
        let groups = [first, context.clone(), if time_too { time.clone().unwrap_or_default() } else { Vec::new() }];
        let mut out = Vec::new();
        for group in groups.into_iter().filter(|g| !g.is_empty()) {
            if !out.is_empty() {
                out.push(sep());
            }
            out.extend(group);
        }
        out
    };
    [
        join(true, true, true, true),
        join(true, false, true, true),
        join(false, false, true, true),
        join(false, false, true, false),
        join(false, false, false, false),
    ]
}

/// Which form of the right part (`metas`) and of the scroll (`scrolls`: whole, short) hold together after the left
/// part ends at `x` (its blank included): the scroll before `end`, the right part ending at `stop` with a blank on
/// each side and at least a dash before it. In turn: the right part loses its effort's word, its model, its time;
/// then the scroll shortens and goes; then the right part loses its effort's sign, then itself, and the scroll comes
/// back if it holds alone.
fn layout(
    x: usize,
    end: usize,
    stop: usize,
    metas: &[Vec<Piece>; 5],
    scrolls: &[String],
    richest: usize,
) -> (Option<usize>, Option<usize>) {
    let (whole, short) = ((!scrolls.is_empty()).then_some(0), (scrolls.len() > 1).then_some(1));
    // No richer than `richest`: a form before it stands for it.
    let meta = |i: usize| Some(i.max(richest)).filter(|&i| i < metas.len() && !metas[i].is_empty());
    let mut tries: Vec<(Option<usize>, Option<usize>)> = (0..4).map(|i| (meta(i), whole)).collect();
    tries.extend([(meta(3), short), (meta(3), None), (meta(4), None), (None, whole), (None, short), (None, None)]);
    let width = |i: usize| metas[i].iter().map(|(text, ..)| columns(text)).sum::<usize>();
    tries
        .into_iter()
        .find(|&(meta, scroll)| {
            let after = x + scroll.map_or(0, |i| part(&scrolls[i]));
            match meta {
                Some(i) => after + 2 + width(i) <= stop,
                None => after <= end,
            }
        })
        .unwrap_or((None, None))
}

/// The style underlined: a part under the mouse.
fn underlined(style: Style) -> Style {
    Style { underline: true, ..style }
}

/// The zoom's sign: ⤢ to zoom, ⤡ back; Nerd Font's fullscreen icons when the terminal carries them.
fn zoom_sign(zoom: Zoom, glyphs: Glyphs) -> char {
    match (zoom, glyphs) {
        // nf-md-fullscreen, nf-md-fullscreen_exit
        (Zoom::In, Glyphs::Nerd) => '\u{F0293}',
        (Zoom::Out, Glyphs::Nerd) => '\u{F0294}',
        (Zoom::In, Glyphs::Unicode) => '⤢',
        (Zoom::Out, Glyphs::Unicode) => '⤡',
    }
}

/// Columns a part after the name takes: « · », its text, a blank.
fn part(text: &str) -> usize {
    2 + columns(text) + 1
}

/// « · text » and a blank, from `x`; returns the column after the blank.
fn put_part(canvas: &mut Canvas, x: usize, y: usize, text: &str, style: Style) -> usize {
    let x = canvas.put(x, y, "· ", Style::PLAIN.dim());
    let x = canvas.put(x, y, text, style);
    canvas.put(x, y, " ", Style::PLAIN)
}

/// How an alert shows in `room` columns beside a name `name` columns wide (its blank included): whole, else short
/// (« moteur en échec » for « moteur en échec : … »), else shortest (« en échec »), beside the whole name if one
/// holds, else beside three columns of it; failing that, the shortest one cut, or nothing rather than « … » alone.
fn alert(note: &str, name: usize, room: usize) -> Option<String> {
    let short = note.split(':').next().unwrap_or(note).trim_end();
    // Shorter still, without its first word: « en échec », « failed ».
    let shortest = short.split_once(' ').map_or(short, |(_, rest)| rest.trim_start());
    let forms = [note, short, shortest];
    let fits = |beside: usize| forms.into_iter().find(|text| beside + part(text) <= room);
    if let Some(text) = fits(name).or_else(|| fits(3 + 1)) {
        return Some(text.to_string());
    }
    let cut = fit(shortest, room.saturating_sub(3 + 1 + part("")));
    (cut.chars().count() > 1).then_some(cut)
}

/// The sign of a pane that ended or failed: a cross, one column wide.
fn failed(glyphs: Glyphs) -> char {
    match glyphs {
        // nf-md-close_circle
        Glyphs::Nerd => '\u{F0159}',
        Glyphs::Unicode => '✗',
    }
}

/// How much a tab says: all of it, or only its number when it is not the current one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Labels {
    Whole,
    Numbers,
}

/// A tab's label: its number, which `Alt+1`…`Alt+9` reach, and its title.
/// The current one zoomed: « 2 Agents (1)  ⤢ dev-saisie » (mock-up B4).
fn label(i: usize, tab: &Tab<'_>, labels: Labels, glyphs: Glyphs) -> String {
    if !tab.active && labels == Labels::Numbers {
        return (i + 1).to_string();
    }
    match tab.zoomed {
        Some(member) => format!("{} {}  {} {member}", i + 1, tab.title, zoom_sign(Zoom::In, glyphs)),
        None => format!("{} {}", i + 1, tab.title),
    }
}

/// How the bar's buttons show: whole, their keys, or not at all on the narrowest screens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Buttons {
    Long,
    Keys,
    None,
}

/// The buttons on the right of the bar, as `shown`.
fn buttons(shown: Buttons, glyphs: Glyphs) -> Vec<(String, Target)> {
    let (menu, quit) = match glyphs {
        // nf-md-menu, nf-md-logout
        Glyphs::Nerd => ("\u{F035C} ", "\u{F0343} "),
        Glyphs::Unicode => ("", ""),
    };
    match shown {
        Buttons::Long => vec![
            (format!(" {menu}menu ({ALT}r) "), Target::Menu),
            (format!(" {quit}{} ({ALT}q) ", t!("quitter", "quit")), Target::Quit),
        ],
        Buttons::Keys => vec![(format!(" {ALT}r "), Target::Menu), (format!(" {ALT}q "), Target::Quit)],
        Buttons::None => Vec::new(),
    }
}

/// The bar on `row` (the screen's last): the team, the tabs between rules, then the buttons on the right, a column
/// from the edge. Returns where each can be clicked. When it is short of room, in turn: the other tabs keep only
/// their numbers, the team goes (the terminal's title has it), the buttons keep only their keys (the only help in
/// sight for a newcomer), the current tab's title is cut down to its number; then the blanks go (« │⠴1│⚑2│⚑3│ »),
/// then the buttons. The tabs' signs always stay: they are the badges (F8, mock-up B6; designer, 2026-10-10). `hover`, the tab or button under the mouse, brighter: an other tab no longer dimmed and bold, a button bold.
pub(crate) fn bar(
    canvas: &mut Canvas,
    row: Rect,
    team: &str,
    tabs: &[Tab<'_>],
    look: Look,
    hover: Option<Target>,
) -> Vec<(Rect, Target)> {
    let end = row.x + row.width;
    let team = format!(" {team} ");
    let number = |i: usize| (i + 1).to_string();
    // The least a tab says: its number, and ⤢ after it for the current one zoomed (« 1 ⤢ », packed « 1⤢ »).
    let least = |i: usize, tab: &Tab<'_>, packed: bool| match tab.zoomed.filter(|_| tab.active) {
        Some(_) if packed => format!("{}{}", i + 1, zoom_sign(Zoom::In, look.glyphs)),
        Some(_) => format!("{} {}", i + 1, zoom_sign(Zoom::In, look.glyphs)),
        None => number(i),
    };
    // Each tab: a rule, then « ⠹ label » between blanks, or « ⠹1 » packed; a rule after the last.
    let tabs_width = |labels: Labels, packed: bool, active_cut: bool| -> usize {
        let each = |(i, tab): (usize, &Tab<'_>)| match (packed, tab.active && active_cut) {
            (true, _) => 2 + columns(&least(i, tab, true)),
            (false, true) => 5 + columns(&least(i, tab, false)),
            (false, false) => 5 + columns(&label(i, tab, labels, look.glyphs)),
        };
        tabs.iter().enumerate().map(each).sum::<usize>() + 1
    };
    let buttons_width = |shown: Buttons| -> usize {
        buttons(shown, look.glyphs).iter().map(|(text, _)| columns(text) + 1).sum::<usize>()
    };
    // The forms from the richest, and the first that leaves a blank before the buttons: the other tabs to their
    // numbers, the team gone, the buttons to their keys (the current title then cut down to its number), the blanks
    // gone, the buttons gone. The tabs' signs never go.
    let forms = [
        (true, Labels::Whole, Buttons::Long, false, false),
        (true, Labels::Numbers, Buttons::Long, false, false),
        (false, Labels::Numbers, Buttons::Long, false, false),
        (false, Labels::Numbers, Buttons::Keys, false, true),
        (false, Labels::Numbers, Buttons::Keys, true, true),
    ];
    let (with_team, labels, shown, packed) = forms
        .into_iter()
        .find(|&(with_team, labels, shown, packed, active_cut)| {
            let team = if with_team { columns(&team) } else { 0 };
            team + tabs_width(labels, packed, active_cut) + 1 + buttons_width(shown) <= row.width
        })
        .map_or((false, Labels::Numbers, Buttons::None, true), |(team, labels, shown, packed, _)| {
            (team, labels, shown, packed)
        });

    let mut zones = Vec::new();
    // The buttons first, from the right: the tabs end before them.
    let mut right = end.saturating_sub(1);
    let mut placed = Vec::new();
    for (text, target) in buttons(shown, look.glyphs).into_iter().rev() {
        let start = right.saturating_sub(columns(&text)).max(row.x);
        // Hovered, brighter: bold.
        let style = if hover == Some(target) { Style::PLAIN.reverse().bold() } else { Style::PLAIN.reverse() };
        canvas.put_in(start, row.y, right, &text, style);
        placed.push((Rect { x: start, y: row.y, width: right - start, height: 1 }, target));
        right = start.saturating_sub(1);
    }
    let limit = if placed.is_empty() { end } else { right.max(row.x) };

    let mut x = row.x;
    if with_team {
        x = canvas.put_in(x, row.y, limit, &team, Style::PLAIN.bold());
    }
    let rule = Style::fg(Color::DarkGrey);
    // What the other tabs take after the current one, for its title to leave them their room.
    let after_active = |from: usize| -> usize {
        tabs[from..]
            .iter()
            .enumerate()
            .map(|(k, tab)| 1 + 4 + columns(&label(from + k, tab, labels, look.glyphs)))
            .sum::<usize>()
            + 1
    };
    for (i, tab) in tabs.iter().enumerate() {
        x = canvas.put_in(x, row.y, limit, "│", rule);
        let mut text = if packed { least(i, tab, true) } else { label(i, tab, labels, look.glyphs) };
        if tab.active && !packed {
            let room = limit.saturating_sub(x + 4 + after_active(i + 1));
            // Zoomed, who shows says more than the tab's title: the title goes first.
            if let Some(member) = tab.zoomed.filter(|_| columns(&text) > room) {
                text = format!("{} {} {member}", i + 1, zoom_sign(Zoom::In, look.glyphs));
            }
            // Down to its number (and ⤢ zoomed), never less: « 1 … » would say nothing more.
            let least = least(i, tab, false);
            text = if room < columns(&least) + 3 { least } else { fit(&text, room) };
        }
        let start = x;
        let sign = tab.state.icon(look.glyphs, look.frame).to_string();
        let sign = sign.as_str();
        // Hovered, its label underlined as what a click reaches; another than the current one brighter too: no
        // longer dimmed, bold.
        let hovered = hover == Some(Target::Tab(i));
        let (base, sign_style, text_style) = if tab.active {
            let on = Style::PLAIN.reverse();
            (on, on, on.bold())
        } else if hovered {
            (Style::PLAIN, sign_style(tab.state), Style::PLAIN.bold())
        } else {
            (Style::PLAIN, sign_style(tab.state), Style::PLAIN.dim())
        };
        let text_style = if hovered { underlined(text_style) } else { text_style };
        let blank = if tab.active { base.bold() } else { base };
        if !packed {
            x = canvas.put_in(x, row.y, limit, " ", base);
        }
        x = canvas.put_in(x, row.y, limit, sign, sign_style);
        if !packed {
            x = canvas.put_in(x, row.y, limit, " ", blank);
        }
        x = canvas.put_in(x, row.y, limit, &text, text_style);
        if !packed {
            x = canvas.put_in(x, row.y, limit, " ", blank);
        }
        if x > start {
            zones.push((Rect { x: start, y: row.y, width: x - start, height: 1 }, Target::Tab(i)));
        }
    }
    canvas.put_in(x, row.y, limit, "│", rule);
    zones.extend(placed.into_iter().rev());
    zones
}

/// A member gone waiting out of sight, for six seconds in a layer at the top right, over the rest without dimming it
/// (mock-up B1, B4): « ⚑ dev-saisie attend ta réponse · Agents (1)   ⌥g y aller ». A click on it goes to the member.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notice {
    pub member: String,
    /// The member's color, for its name.
    pub color: Option<Color>,
    /// What it says, from [`notice_text`].
    pub text: String,
    /// The tab the member is in, dimmed after the text.
    pub tab: String,
}

/// What a notice says of `member`.
pub(crate) fn notice_text(member: &str) -> String {
    t!("{} attend ta réponse", "{} needs your answer", member)
}

/// Columns between a notice and the screen's right edge (mock-up B1).
const NOTICE_MARGIN: usize = 2;

/// What a notice says inside its frame, in `room` columns: « ⚑ dev-saisie attend ta réponse · Agents (1)   ⌥g y
/// aller », its name in its color; short of room, ⌥g goes, then the tab, then the text is cut.
fn notice_pieces(notice: &Notice, glyphs: Glyphs, room: usize) -> Vec<(String, Style)> {
    let sign = format!(" {} ", State::Waiting.icon(glyphs, 0));
    let mut said = vec![(sign, Style::fg(Color::Red).bold())];
    match notice.text.strip_prefix(notice.member.as_str()).filter(|_| !notice.member.is_empty()) {
        Some(rest) => {
            let name = notice.color.map_or(Style::PLAIN, Style::fg).bold();
            said.extend([(notice.member.clone(), name), (rest.to_string(), Style::PLAIN.bold())]);
        }
        None => said.push((notice.text.clone(), Style::PLAIN.bold())),
    }
    let tab = (!notice.tab.is_empty()).then(|| (format!(" · {}", notice.tab), Style::PLAIN.dim()));
    let go = [
        ("   ".to_string(), Style::PLAIN),
        (format!("{ALT}g"), Style::fg(Color::DarkCyan)),
        (format!(" {}", t!("y aller", "go")), Style::PLAIN.dim()),
    ];
    let end = (" ".to_string(), Style::PLAIN);
    let forms: [Vec<(String, Style)>; 3] = [
        said.iter().cloned().chain(tab.clone()).chain(go).chain([end.clone()]).collect(),
        said.iter().cloned().chain(tab).chain([end.clone()]).collect(),
        said.iter().cloned().chain([end.clone()]).collect(),
    ];
    let width = |pieces: &[(String, Style)]| pieces.iter().map(|(text, _)| columns(text)).sum::<usize>();
    if let Some(form) = forms.iter().find(|form| width(form) <= room) {
        return form.clone();
    }
    // Cut: the sign, then what is left of the text, a blank at the end.
    let mut left = room.saturating_sub(1);
    let mut out = Vec::new();
    for (text, style) in &forms[2][..forms[2].len() - 1] {
        if left == 0 {
            break;
        }
        let cut = if columns(text) <= left { text.clone() } else { fit(text, left) };
        left -= columns(&cut);
        out.push((cut, *style));
    }
    out.push(end);
    out
}

/// Where `notice` goes on a screen whose team shows in `screen` (the bar left out): three rows at the top right, on
/// the panes' first row, two columns from the edge (mock-up B1, B4); no wider than the screen allows, empty on a
/// screen too small for it.
pub(crate) fn notice_area(screen: Rect, notice: &Notice) -> Rect {
    let room = screen.width.saturating_sub(NOTICE_MARGIN + 2);
    if screen.height < 4 || room < 8 {
        return Rect { x: screen.x, y: screen.y, width: 0, height: 0 };
    }
    // Its size does not depend on the glyphs: the sign is one column wide either way.
    let inner: usize = notice_pieces(notice, Glyphs::Unicode, room).iter().map(|(text, _)| columns(text)).sum();
    let width = inner + 2;
    Rect { x: screen.x + screen.width - NOTICE_MARGIN - width, y: screen.y + 1, width, height: 3 }
}

/// Draws `notice` in `area` (from [`notice_area`]), over what the screen drew, which stays as it is around it: a red
/// frame, the text inside. A click anywhere in `area` goes to the member.
pub(crate) fn notice(canvas: &mut Canvas, area: Rect, notice: &Notice, look: Look) {
    let Rect { x, y, width, height } = area;
    if width < 3 || height < 3 {
        return;
    }
    canvas.frame(x, y, width, height, Style::fg(Color::Red));
    let end = x + width - 1;
    let mut at = x + 1;
    for (text, style) in notice_pieces(notice, look.glyphs, width - 2) {
        at = canvas.put_in(at, y + 1, end, &text, style);
    }
}

/// A choice over the team, in a layer at the center, the rest dimmed (direction « Cadres »): the quit choice of ⌥q
/// and the bar's « quitter », a confirmation. Step 2: drawn here (dev-interface), placed and dimmed by the screen
/// (dev-rendu), kept, answered and acted on by the server (dev-serveur). See specs/multiplexeur.md, step 2.
///
/// - The screen dims the rest before it calls [`choice`], which draws its box only.
/// - An option's key matches whatever its case, with no modifier but Shift (« Q » under Caps Lock picks « q »); Esc
///   picks the cancel, the last option. ⏎ picks the selected one; ←→ and ↑↓ move it. The server answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Choice {
    pub title: String,
    /// A line under the title, dimmed: what the choice costs.
    pub note: Option<String>,
    /// Each option with the key that picks it, both shown; in their order on screen.
    pub options: Vec<(char, String)>,
    /// The option ⏎ picks, highlighted: ←→ (or ↑↓) move it. A confirmation opens on its cancel.
    pub selected: usize,
}

/// The key that cancels a choice: « a » for Annuler, « c » for Cancel.
fn cancel_key() -> char {
    match crate::i18n::lang() {
        crate::i18n::Lang::Fr => 'a',
        crate::i18n::Lang::En => 'c',
    }
}

/// ⌥q and the bar's « quitter »: Détacher (d), Quitter (q), Annuler (a); Detach (d), Quit (q), Cancel (c) in English
/// (CLAUDE.md, 2026-10-08), with nothing more in the labels, under « recruit ». Opens on the cancel. Its options' order is the server's: detach, quit, cancel.
pub(crate) fn quit_choice(_team: &str) -> Choice {
    Choice {
        title: "recruit".into(),
        note: None,
        options: vec![
            ('d', t!("Détacher", "Detach")),
            ('q', t!("Quitter", "Quit")),
            (cancel_key(), t!("Annuler", "Cancel")),
        ],
        selected: 2,
    }
}

/// The confirmation of a click on a member's context on the dashboard (user's choice, 2026-10-09): « Compacter <membre> ? », what it does with the member's context as the
/// dashboard shows it (`percent`, left out unknown), then Compacter (c) and Annuler (a); Compact (o) and Cancel (c)
/// in English. Opens on the cancel. Its options' order is the server's: compact, cancel.
pub(crate) fn compact_choice(member: &str, percent: Option<u8>) -> Choice {
    let note = match percent {
        Some(percent) => t!(
            "Résume sa conversation pour libérer du contexte ({} %).",
            "Summarizes its conversation to free context ({}%).",
            percent
        ),
        None => t!("Résume sa conversation pour libérer du contexte.", "Summarizes its conversation to free context."),
    };
    let compact = match crate::i18n::lang() {
        crate::i18n::Lang::Fr => 'c',
        crate::i18n::Lang::En => 'o',
    };
    Choice {
        title: t!("Compacter {} ?", "Compact {}?", member),
        note: Some(note),
        options: vec![(compact, t!("Compacter", "Compact")), (cancel_key(), t!("Annuler", "Cancel"))],
        selected: 1,
    }
}

/// Columns inside a choice's frame: the title in the border, the note, each option with its key at the right.
fn choice_inner(choice: &Choice) -> usize {
    let title = columns(&choice.title) + 5;
    let note = choice.note.as_deref().map_or(0, |note| columns(note) + 2);
    let options = choice.options.iter().map(|(_, label)| columns(label) + 5).max().unwrap_or(0);
    title.max(note).max(options)
}

/// Rows a choice takes: its frame, its note, its options, and the rule before the last one (the cancel).
fn choice_rows(choice: &Choice) -> usize {
    2 + usize::from(choice.note.is_some()) + choice.options.len() + usize::from(choice.options.len() > 1)
}

/// Where `choice` goes on a screen whose team shows in `screen` (the bar left out): its size from its text, centered;
/// no larger than the screen.
pub(crate) fn choice_area(screen: Rect, choice: &Choice) -> Rect {
    let width = (choice_inner(choice) + 2).min(screen.width);
    let height = choice_rows(choice).min(screen.height);
    Rect { x: screen.x + (screen.width - width) / 2, y: screen.y + (screen.height - height) / 2, width, height }
}

/// Draws `choice` in `area` (from [`choice_area`]), over what the screen drew and dimmed: a grey frame as the
/// panels', its title in the top border, the note dimmed, one option a row with its key at the right in cyan, a rule
/// before the last one (the cancel), the selected one in reverse video. Returns where each option can be clicked, by
/// its index.
pub(crate) fn choice(canvas: &mut Canvas, area: Rect, choice: &Choice, _look: Look) -> Vec<(Rect, usize)> {
    let Rect { x, y, width, height } = area;
    if width < 4 || height < 3 {
        return Vec::new();
    }
    canvas.frame(x, y, width, height, Style::fg(Color::DarkGrey));
    let inner = width - 2;
    let right = x + width - 1;
    let title = fit(&choice.title, inner.saturating_sub(4));
    canvas.put_in(x + 2, y, right, &format!(" {title} "), Style::PLAIN.bold());
    let (shown, note, rule) = fitted(choice, height - 2);
    let mut row = y + 1;
    if let Some(note) = choice.note.as_deref().filter(|_| note) {
        canvas.put_in(x + 2, row, right - 1, &fit(note, inner.saturating_sub(2)), Style::PLAIN.dim());
        row += 1;
    }
    let mut zones = Vec::new();
    let last = choice.options.len().saturating_sub(1);
    for i in shown {
        let (key, label) = &choice.options[i];
        if i == last && rule {
            canvas.put(x, row, "├", Style::fg(Color::DarkGrey));
            canvas.hline(x + 1, row, inner, '─', Style::fg(Color::DarkGrey));
            canvas.put(right, row, "┤", Style::fg(Color::DarkGrey));
            row += 1;
        }
        let on = i == choice.selected;
        let base = if on { Style::PLAIN.reverse() } else { Style::PLAIN };
        canvas.fill(x + 1, row, inner, 1, base);
        let key_at = right.saturating_sub(2);
        let key_style = if on { base.bold() } else { Style::fg(Color::DarkCyan).bold() };
        canvas.put_in(key_at, row, right, &key.to_string(), key_style);
        let label = fit(label, key_at.saturating_sub(x + 3));
        canvas.put_in(x + 2, row, key_at, &label, if on { base.bold() } else { base });
        zones.push((Rect { x: x + 1, y: row, width: inner, height: 1 }, i));
        row += 1;
    }
    zones
}

/// What of `choice` holds in `rows` rows inside its frame: the options shown, by index in their order, whether the
/// note shows and whether the rule does. Short of rows, the note goes first, then the rule, then the options but the
/// cancel (the last) and the selected one, which always stay while a row is left; the others then show in their
/// order as the rows allow.
fn fitted(choice: &Choice, rows: usize) -> (Vec<usize>, bool, bool) {
    let count = choice.options.len();
    let rule = count > 1;
    let note = choice.note.is_some() && rows > count + usize::from(rule);
    let rule = rule && rows > count;
    if rows >= count {
        return ((0..count).collect(), note, rule);
    }
    let last = count.saturating_sub(1);
    let mut kept: Vec<usize> = [last, choice.selected].into_iter().filter(|&i| i < count).collect();
    kept.dedup();
    kept.truncate(rows);
    for i in 0..count {
        if kept.len() >= rows {
            break;
        }
        if !kept.contains(&i) {
            kept.push(i);
        }
    }
    kept.sort_unstable();
    (kept, false, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNICODE: Look = Look { glyphs: Glyphs::Unicode, frame: 2 };

    fn rows(canvas: &Canvas) -> Vec<String> {
        (0..canvas.height()).map(|y| canvas.row(y)).collect()
    }

    fn member<'a>(name: &'a str, state: State) -> Header<'a> {
        Header { name, color: Some(Color::Magenta), state, ..Header::default() }
    }

    /// A pane `width` columns wide and three rows high, framed.
    fn framed(header: &Header<'_>, width: usize, focused: bool) -> Vec<String> {
        let mut canvas = Canvas::new(width, 3);
        frame(&mut canvas, Rect { x: 0, y: 0, width, height: 3 }, header, focused, UNICODE);
        rows(&canvas)
    }

    /// A choice's rows as drawn: `inner` columns between the borders.
    fn choice_rows_of(choice: &Choice, inner: usize) -> Vec<String> {
        let row = |text: String| format!("│{text}{}│", " ".repeat(inner - columns(&text)));
        let title = format!("╭─ {} ", choice.title);
        let mut rows = vec![format!("{title}{}╮", "─".repeat(inner + 1 - columns(&title)))];
        if let Some(note) = &choice.note {
            rows.push(row(format!(" {note}")));
        }
        let last = choice.options.len() - 1;
        for (i, (key, label)) in choice.options.iter().enumerate() {
            if i == last {
                rows.push(format!("├{}┤", "─".repeat(inner)));
            }
            let gap = inner - 1 - columns(label) - 2;
            rows.push(row(format!(" {label}{}{key} ", " ".repeat(gap))));
        }
        rows.push(format!("╰{}╯", "─".repeat(inner)));
        rows
    }

    #[test]
    fn the_quit_choice_in_the_team_s_words() {
        let choice = quit_choice("mux");
        let cancel = if t!("a", "c") == "a" { 'a' } else { 'c' };
        assert_eq!(
            choice.options,
            [('d', t!("Détacher", "Detach")), ('q', t!("Quitter", "Quit")), (cancel, t!("Annuler", "Cancel"))]
        );
        assert_eq!(choice.selected, 2, "opens on the cancel");
        assert_eq!((choice.title.as_str(), choice.note), ("recruit", None));
        let choice = compact_choice("dev-cli", Some(88));
        let compact = if t!("c", "o") == "c" { 'c' } else { 'o' };
        assert_eq!(choice.options, [(compact, t!("Compacter", "Compact")), (cancel, t!("Annuler", "Cancel"))]);
        assert_eq!(choice.title, t!("Compacter {} ?", "Compact {}?", "dev-cli"));
        let note = t!(
            "Résume sa conversation pour libérer du contexte ({} %).",
            "Summarizes its conversation to free context ({}%).",
            88
        );
        assert_eq!(choice.note, Some(note));
        assert_eq!(choice.selected, 1, "opens on the cancel");
        let unknown = compact_choice("dev-cli", None).note.unwrap();
        let bare =
            t!("Résume sa conversation pour libérer du contexte.", "Summarizes its conversation to free context.");
        assert_eq!(unknown, bare);
    }

    #[test]
    fn a_choice_at_the_center_with_its_rule_and_keys() {
        let choice = quit_choice("mux");
        let screen = Rect { x: 0, y: 0, width: 60, height: 20 };
        let area = choice_area(screen, &choice);
        let inner = choice_inner(&choice);
        assert_eq!((area.width, area.height), (inner + 2, 6));
        assert_eq!((area.x, area.y), ((60 - area.width) / 2, (20 - 6) / 2));
        let mut canvas = Canvas::new(60, 20);
        canvas.fill(0, 0, 60, 20, Style::PLAIN.dim());
        canvas.put(0, area.y + 2, &"x".repeat(60), Style::PLAIN.dim());
        let zones = super::choice(&mut canvas, area, &choice, UNICODE);
        let drawn: Vec<String> = (area.y..area.y + area.height)
            .map(|y| canvas.row(y).chars().skip(area.x).take(area.width).collect())
            .collect();
        assert_eq!(drawn, choice_rows_of(&choice, inner));
        // Each option where it shows, the rule skipped.
        let rows: Vec<(usize, usize)> = zones.iter().map(|(rect, i)| (rect.y - area.y, *i)).collect();
        assert_eq!(rows, [(1, 0), (2, 1), (4, 2)]);
        assert!(zones.iter().all(|(rect, _)| rect.x == area.x + 1 && rect.width == inner));
        // The selected one in reverse video, the others plain with their key in cyan; the frame grey.
        let selected = Style::PLAIN.reverse().bold();
        assert_eq!(canvas.style(area.x + 2, area.y + 4), selected);
        assert_eq!(canvas.style(area.x + inner - 1, area.y + 4), selected, "its key reversed too, not cyan");
        assert_eq!(canvas.style(area.x + 2, area.y + 2), Style::PLAIN);
        assert_eq!(canvas.style(area.x + inner - 1, area.y + 2), Style::fg(Color::DarkCyan).bold());
        assert_eq!(canvas.style(area.x, area.y + 1), Style::fg(Color::DarkGrey));
        // The compaction's note, dimmed.
        let compact = compact_choice("dev", Some(40));
        let area = choice_area(screen, &compact);
        super::choice(&mut canvas, area, &compact, UNICODE);
        assert!(canvas.style(area.x + 2, area.y + 1).dim, "the note dimmed");
        // What was under it is gone, what is around it stays.
        assert_eq!(canvas.row(area.y + 2).chars().take(area.x).collect::<String>(), "x".repeat(area.x));
    }

    #[test]
    fn a_choice_no_larger_than_the_screen() {
        let choice = compact_choice("un-nom-de-membre-vraiment-très-long", Some(88));
        let screen = Rect { x: 0, y: 0, width: 24, height: 4 };
        let area = choice_area(screen, &choice);
        assert_eq!((area.x, area.width, area.height), (0, 24, 4));
        let mut canvas = Canvas::new(24, 4);
        let zones = super::choice(&mut canvas, area, &choice, UNICODE);
        // Two rows inside: the note goes, then the rule; both options stay, the cancel selected.
        assert_eq!(zones.iter().map(|(rect, i)| (rect.y, *i)).collect::<Vec<_>>(), [(1, 0), (2, 1)]);
        assert!(canvas.style(2, 2).reverse, "the cancel, selected");
        assert_eq!(canvas.row(3), format!("╰{}╯", "─".repeat(22)));
    }

    /// Reviewer: on a low screen, the choice never loses the cancel it opens on.
    #[test]
    fn a_low_screen_keeps_the_cancel() {
        let choice = quit_choice("mux");
        let rows = |height: usize| {
            let screen = Rect { x: 0, y: 0, width: 30, height };
            let area = choice_area(screen, &choice);
            let mut canvas = Canvas::new(30, height);
            super::choice(&mut canvas, area, &choice, UNICODE).into_iter().map(|(_, i)| i).collect::<Vec<_>>()
        };
        assert_eq!(rows(6), [0, 1, 2], "all, with the rule");
        assert_eq!(rows(5), [0, 1, 2], "the rule gone");
        assert_eq!(rows(4), [0, 2], "the cancel kept, then the first");
        assert_eq!(rows(3), [2], "the cancel alone");
        // The selected one kept as well, when it is not the cancel.
        let quit = Choice { selected: 1, ..quit_choice("mux") };
        assert_eq!(fitted(&quit, 2).0, [1, 2]);
        assert_eq!(fitted(&quit, 1).0, [2]);
    }

    #[test]
    fn the_frame_takes_a_cell_all_around() {
        assert_eq!(inside(Rect { x: 0, y: 0, width: 10, height: 5 }), Rect { x: 1, y: 1, width: 8, height: 3 });
        assert_eq!(inside(Rect { x: 0, y: 0, width: 1, height: 1 }).width, 0);
    }

    #[test]
    fn thick_for_the_focus_thin_otherwise() {
        let header = member("dev-rendu", State::Working);
        assert_eq!(
            framed(&header, 24, true),
            ["┏━ ⠹ dev-rendu ━━━━━━━━┓", "┃                      ┃", "┗━━━━━━━━━━━━━━━━━━━━━━┛"]
        );
        assert_eq!(
            framed(&header, 24, false),
            ["╭─ ⠹ dev-rendu ────────╮", "│                      │", "╰──────────────────────╯"]
        );
    }

    #[test]
    fn inside_left_as_it_is() {
        let mut canvas = Canvas::new(12, 3);
        canvas.put(1, 1, "claude", Style::PLAIN);
        frame(&mut canvas, Rect { x: 0, y: 0, width: 12, height: 3 }, &member("a", State::Idle), false, UNICODE);
        assert_eq!(canvas.row(1), "│claude    │");
    }

    #[test]
    fn colors_of_the_state() {
        let mut canvas = Canvas::new(30, 3);
        let area = Rect { x: 0, y: 0, width: 30, height: 3 };
        // Without the focus: grey at work and at rest, red waiting.
        for (state, color) in
            [(State::Working, Color::DarkGrey), (State::Waiting, Color::Red), (State::Idle, Color::DarkGrey)]
        {
            frame(&mut canvas, area, &member("dev-saisie", state), false, UNICODE);
            assert_eq!(canvas.style(0, 1), Style::fg(color), "{state:?}");
            // The sign in the state's color, the name in the member's: dimmed at rest, bold otherwise.
            assert_eq!(canvas.style(3, 0), sign_style(state));
            let name = canvas.style(5, 0);
            assert_eq!(name.fg, Some(Color::Magenta), "{state:?}");
            assert_eq!((name.bold, name.dim), (state != State::Idle, state == State::Idle));
        }
    }

    /// The active pane: thick, bold, in the terminal's text color at rest and at work, red waiting; its header keeps
    /// the state's styles.
    #[test]
    fn the_focus_in_the_terminal_s_text_color() {
        let mut canvas = Canvas::new(30, 3);
        let area = Rect { x: 0, y: 0, width: 30, height: 3 };
        for (state, style) in [
            (State::Idle, Style::PLAIN.bold()),
            (State::Working, Style::PLAIN.bold()),
            (State::Waiting, Style::fg(Color::Red).bold()),
        ] {
            frame(&mut canvas, area, &member("dev-saisie", state), true, UNICODE);
            assert_eq!(canvas.style(0, 1), style, "{state:?}");
            assert_eq!(canvas.row(1).chars().next(), Some('┃'));
            assert_eq!(canvas.style(3, 0), sign_style(state), "{state:?}");
        }
        // At rest, its name dimmed; at work and waiting, in its color and bold; waiting, ⚑ red.
        frame(&mut canvas, area, &member("dev-saisie", State::Idle), true, UNICODE);
        assert_eq!(canvas.style(5, 0), Style::fg(Color::Magenta).dim());
        frame(&mut canvas, area, &member("dev-saisie", State::Working), true, UNICODE);
        assert_eq!(canvas.style(5, 0), Style::fg(Color::Magenta).bold());
        frame(&mut canvas, area, &member("dev-saisie", State::Waiting), true, UNICODE);
        assert_eq!(canvas.style(3, 0), Style::fg(Color::Red).bold());
        assert_eq!(canvas.style(5, 0), Style::fg(Color::Magenta).bold());
        // A panel with the focus stays grey, thick.
        let panel = Header { name: "Journal", ..Header::default() };
        frame(&mut canvas, area, &panel, true, UNICODE);
        assert_eq!(canvas.style(0, 1), Style::fg(Color::DarkGrey).bold());
    }

    /// A pane gone wrong: red with the focus or without.
    #[test]
    fn an_error_red_even_with_the_focus() {
        let mut canvas = Canvas::new(30, 3);
        let area = Rect { x: 0, y: 0, width: 30, height: 3 };
        let header = Header { note: Some("terminé"), ..member("dev-saisie", State::Idle) };
        frame(&mut canvas, area, &header, false, UNICODE);
        assert_eq!(canvas.style(0, 1), Style::fg(Color::Red));
        frame(&mut canvas, area, &header, true, UNICODE);
        assert_eq!(canvas.style(0, 1), Style::fg(Color::Red).bold());
        assert_eq!(canvas.row(1).chars().next(), Some('┃'));
    }

    /// No name, no header in the border: the menu's layer, whose first line says « recruit ».
    #[test]
    fn no_name_no_header() {
        let header = Header::default();
        assert_eq!(framed(&header, 20, true), ["┏━━━━━━━━━━━━━━━━━━┓", "┃                  ┃", "┗━━━━━━━━━━━━━━━━━━┛"]);
        assert_eq!(framed(&header, 20, false)[0], "╭──────────────────╮");
        let mut canvas = Canvas::new(20, 3);
        assert!(frame(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 3 }, &header, true, UNICODE).is_empty());
    }

    #[test]
    fn a_panel_grey_and_plain() {
        let header = Header { name: "Journal", state: State::Working, ..Header::default() };
        assert_eq!(framed(&header, 20, false)[0], "╭─ Journal ────────╮");
        let mut canvas = Canvas::new(20, 3);
        frame(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 3 }, &header, false, UNICODE);
        assert_eq!(canvas.style(0, 0), Style::fg(Color::DarkGrey));
        assert_eq!(canvas.style(3, 0), Style::PLAIN);
    }

    /// Mock-up B1: the journal's key at the right of its border, while the title leaves room.
    #[test]
    fn the_journal_s_key_in_its_border() {
        let header = Header { name: "Journal", hint: Some(Hint::JournalSize), ..Header::default() };
        let hint = t!("{}j taille", "{}j size", ALT);
        let top = |width: usize| framed(&header, width, false).remove(0);
        let head = "╭─ Journal ";
        let dashes = 40 - columns(head) - columns(&hint) - 2 - 2;
        assert_eq!(top(40), format!("{head}{} {hint} ─╮", "─".repeat(dashes)));
        // Dimmed: its last letter, before the blank, the dash and the corner.
        let mut canvas = Canvas::new(40, 3);
        frame(&mut canvas, Rect { x: 0, y: 0, width: 40, height: 3 }, &header, false, UNICODE);
        assert_eq!(canvas.row(0).chars().nth(40 - 4), hint.chars().last());
        assert_eq!(canvas.style(40 - 4, 0), Style::PLAIN.dim());
        // At the limit, a single dash between the title and it; a column less, the title alone.
        let limit = columns(head) + 1 + columns(&hint) + 2 + 2;
        assert_eq!(top(limit), format!("{head}─ {hint} ─╮"));
        assert_eq!(top(limit - 1), format!("{head}{}╮", "─".repeat(limit - 1 - columns(head) - 1)));
    }

    /// Mock-up B1: the dashboard's counts in its border, each in its state's color; none for a state no one is in.
    #[test]
    fn the_dashboard_s_counts_in_its_border() {
        let title = t!("Tableau de bord", "Dashboard");
        let header =
            Header { name: &title, hint: Some(Hint::Counts { working: 5, waiting: 1, idle: 3 }), ..Header::default() };
        let mut canvas = Canvas::new(60, 3);
        frame(&mut canvas, Rect { x: 0, y: 0, width: 60, height: 3 }, &header, false, UNICODE);
        let head = format!("╭─ {title} ");
        let counts = " ⠹ 5  ⚑ 1  ◷ 3 ─╮";
        assert_eq!(canvas.row(0), format!("{head}{}{counts}", "─".repeat(60 - columns(&head) - columns(counts))));
        let at = |sign: char| canvas.row(0).chars().position(|c| c == sign).unwrap();
        assert_eq!(canvas.style(at('⠹'), 0), Style::fg(Color::Yellow));
        assert_eq!(canvas.style(at('⚑') + 2, 0), Style::fg(Color::Red));
        assert_eq!(canvas.style(at('◷'), 0), Style::fg(Color::DarkGrey));
        // No one waiting: no ⚑; no one at all: nothing.
        let header = Header { hint: Some(Hint::Counts { working: 2, waiting: 0, idle: 0 }), ..header };
        assert!(framed(&header, 60, false)[0].ends_with("─ ⠹ 2 ─╮"));
        let header = Header { hint: Some(Hint::Counts { working: 0, waiting: 0, idle: 0 }), ..header };
        assert_eq!(framed(&header, 30, false)[0], format!("{head}{}╮", "─".repeat(30 - columns(&head) - 1)));
        // Nerd Font: the robot at work, as on the dashboard.
        let header = Header { hint: Some(Hint::Counts { working: 2, waiting: 0, idle: 0 }), ..header };
        frame(
            &mut canvas,
            Rect { x: 0, y: 0, width: 60, height: 3 },
            &header,
            false,
            Look { glyphs: Glyphs::Nerd, frame: 0 },
        );
        assert!(canvas.row(0).ends_with("\u{F06A9} 2 ─╮"), "{}", canvas.row(0));
    }

    /// Mock-up B7: the header in panes of 72, 52, 40, 34, 26 and 20 columns, then narrower.
    #[test]
    fn the_header_as_wide_as_the_pane() {
        let header = Header { scroll: Some((214, 3000)), ..member("dev-interface", State::Waiting) };
        let top = |width| framed(&header, width, false).remove(0);
        let up = t!("↑ {} sur {}", "↑ {} of {}", 214, 3000);
        let line = |head: &str, width: usize| format!("{head} {}╮", "─".repeat(width - columns(head) - 2));
        for width in [72, 52, 41] {
            assert_eq!(top(width), line(&format!("╭─ ⚑ dev-interface · {up}"), width));
        }
        // The scroll short, then gone.
        assert_eq!(top(34), "╭─ ⚑ dev-interface · ↑ 214 ──────╮");
        assert_eq!(top(26), "╭─ ⚑ dev-interface ──────╮");
        assert_eq!(top(20), "╭─ ⚑ dev-inte… ────╮");
        // Narrower than the zoom's corner allows: the header up to a dash before the frame's corner.
        assert_eq!(top(12), "╭─ ⚑ dev… ─╮");
        assert_eq!(top(11), "╭─ ⚑ de… ─╮");
        // Too narrow for three columns of the name: the border alone.
        assert_eq!(top(10), "╭────────╮");
        assert_eq!(top(6), "╭────╮");
        assert_eq!(top(2), "╭╮");
    }

    /// An alert in red, the frame red too: whole, else short, the name cut for it, then the alert itself.
    #[test]
    fn an_alert_before_the_rest() {
        let header = Header {
            note: Some("moteur en échec : panique"),
            scroll: Some((3, 9)),
            ..member("dev-saisie", State::Working)
        };
        let top = |width| framed(&header, width, false).remove(0);
        let up = t!("↑ {} sur {}", "↑ {} of {}", 3, 9);
        assert!(top(72).starts_with(&format!("╭─ ✗ dev-saisie · moteur en échec : panique · {up} ─")), "{}", top(72));
        // The scroll goes before the alert is shortened, and stays gone while it is.
        assert_eq!(top(49), "╭─ ✗ dev-saisie · moteur en échec : panique ────╮");
        assert_eq!(top(45), "╭─ ✗ dev-saisie · moteur en échec ──────────╮");
        // Shorter still, the name whole; then the name cut, then the alert itself.
        assert_eq!(top(35), "╭─ ✗ dev-saisie · en échec ───────╮");
        assert_eq!(top(30), "╭─ ✗ dev-saisie · en échec ──╮");
        assert_eq!(top(24), "╭─ ✗ dev-… · en échec ─╮");
        assert_eq!(top(20), "╭─ ✗ de… · en éc… ─╮");
        let mut canvas = Canvas::new(45, 3);
        frame(&mut canvas, Rect { x: 0, y: 0, width: 45, height: 3 }, &header, false, UNICODE);
        assert_eq!(canvas.style(0, 1), Style::fg(Color::Red));
        // The cross in red, and the alert.
        assert_eq!(canvas.style(3, 0), Style::fg(Color::Red));
        assert_eq!(canvas.style(18, 0), Style::fg(Color::Red));
        let nerd = Look { glyphs: Glyphs::Nerd, frame: 0 };
        frame(&mut canvas, Rect { x: 0, y: 0, width: 45, height: 3 }, &header, false, nerd);
        assert!(canvas.row(0).starts_with("╭─ \u{F0159} dev-saisie"), "{}", canvas.row(0));
        // In English, the same forms: « engine failed », then « failed ».
        let english = Header { note: Some("engine failed: panic"), ..header };
        assert_eq!(framed(&english, 40, false)[0], "╭─ ✗ dev-saisie · engine failed ───────╮");
        assert_eq!(framed(&english, 30, false)[0], "╭─ ✗ dev-saisie · failed ────╮");
        // A one-word alert has no shorter form.
        let header = Header { note: Some("terminé"), scroll: None, ..member("dev-saisie", State::Idle) };
        assert_eq!(framed(&header, 31, false)[0], "╭─ ✗ dev-saisie · terminé ────╮");
        assert_eq!(framed(&header, 30, false)[0], "╭─ ✗ dev-saisie · terminé ───╮");
        assert_eq!(framed(&header, 24, false)[0], "╭─ ✗ dev-s… · terminé ─╮");
    }

    /// A member at work with all its header: Opus, high, 29 %, six minutes, the zoom.
    fn rich<'a>(name: &'a str, state: State) -> Header<'a> {
        Header {
            model: Some("Opus"),
            effort: Some("high"),
            context: Some(29),
            since: Some(360),
            zoom: Some(Zoom::In),
            ..member(name, state)
        }
    }

    /// A header's top border: `left`, dashes, then « meta » if any before the zoom's corner.
    fn bordered(left: &str, meta: &str, width: usize) -> String {
        let right = if meta.is_empty() { " ⤢ ─╮".to_string() } else { format!(" {meta} ─ ⤢ ─╮") };
        format!("{left}{}{right}", "─".repeat(width - columns(left) - columns(&right)))
    }

    /// Mock-up B7: what goes when the pane narrows, in turn: the effort's word, the model, the time, the effort's sign,
    /// the context; then the name is cut.
    #[test]
    fn the_full_header_as_wide_as_the_pane() {
        let header = rich("dev-rendu", State::Working);
        let top = |width| framed(&header, width, false).remove(0);
        let left = "╭─ ⠹ dev-rendu ";
        for (widths, meta) in [
            ([72, 47], "Opus ▆ high · 29 % · 6m"),
            ([46, 42], "Opus ▆ · 29 % · 6m"),
            ([41, 37], "▆ · 29 % · 6m"),
            ([36, 32], "▆ · 29 %"),
            ([31, 28], "29 %"),
            ([27, 20], ""),
        ] {
            for width in widths {
                assert_eq!(top(width), bordered(left, meta, width), "{width}");
            }
        }
        // Beside the zoom, a single blank.
        assert_eq!(top(19), "╭─ ⠹ dev-rendu ⤢ ─╮");
        assert_eq!(top(18), "╭─ ⠹ dev-ren… ⤢ ─╮");
        // Narrower than the zoom's corner allows: no zoom, the name cut as before.
        assert_eq!(top(15), "╭─ ⠹ dev-re… ─╮");
        // Thick with the focus, the same header.
        assert_eq!(
            framed(&header, 72, true)[0],
            bordered(left, "Opus ▆ high · 29 % · 6m", 72).replace('─', "━").replace('╭', "┏").replace('╮', "┓")
        );
    }

    /// At rest with a command running: the terminal's sign, dimmed as at rest, its time that of the command; work,
    /// when it comes back, prevails.
    #[test]
    fn a_command_running() {
        let header = Header { shell: true, ..rich("dev", State::Idle) };
        let mut canvas = Canvas::new(40, 3);
        let area = Rect { x: 0, y: 0, width: 40, height: 3 };
        frame(&mut canvas, area, &header, false, UNICODE);
        assert_eq!(canvas.cell(3, 0), Glyphs::Unicode.console().to_string());
        assert_eq!(canvas.style(3, 0), sign_style(State::Idle));
        assert_eq!(canvas.style(5, 0), Style::fg(Color::Magenta).dim(), "the name at rest");
        assert!(canvas.row(0).contains("6m"), "{}", canvas.row(0));
        frame(&mut canvas, area, &header, false, Look { glyphs: Glyphs::Nerd, frame: 0 });
        assert_eq!(canvas.cell(3, 0), Glyphs::Nerd.console().to_string());
        let working = Header { state: State::Working, ..header };
        frame(&mut canvas, area, &working, false, UNICODE);
        assert_eq!(canvas.cell(3, 0), "⠹");
        // Gone wrong, the cross all the same.
        let failed = Header { note: Some("terminé"), ..header };
        frame(&mut canvas, area, &failed, false, UNICODE);
        assert_eq!(canvas.cell(3, 0), "✗");
    }

    /// Missing pieces leave no separator behind.
    #[test]
    fn only_what_is_known() {
        let left = "╭─ ◷ dev ";
        let header = Header { model: None, since: None, ..rich("dev", State::Idle) };
        assert_eq!(framed(&header, 40, false)[0], bordered(left, "▆ high · 29 %", 40));
        let header = Header { effort: None, context: None, ..rich("dev", State::Idle) };
        assert_eq!(framed(&header, 40, false)[0], bordered(left, "Opus · 6m", 40));
        let header = Header { context: None, since: None, ..rich("dev", State::Idle) };
        assert_eq!(framed(&header, 40, false)[0], bordered(left, "Opus ▆ high", 40));
        // Without the zoom (a pane that is not a member's), dashes in the corner.
        let header = Header { zoom: None, ..member("dev", State::Idle) };
        assert_eq!(framed(&header, 20, false)[0], "╭─ ◷ dev ──────────╮");
        // Zoomed: ⤡.
        let header = Header { zoom: Some(Zoom::Out), ..member("dev", State::Idle) };
        assert_eq!(framed(&header, 20, false)[0], "╭─ ◷ dev ────── ⤡ ─╮");
        let nerd = Look { glyphs: Glyphs::Nerd, frame: 0 };
        let mut canvas = Canvas::new(20, 3);
        frame(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 3 }, &header, false, nerd);
        assert_eq!(canvas.cell(16, 0), "\u{F0294}");
    }

    #[test]
    fn the_header_s_colors() {
        let mut canvas = Canvas::new(72, 3);
        let area = Rect { x: 0, y: 0, width: 72, height: 3 };
        let at = |canvas: &Canvas, text: &str| {
            let row = canvas.row(0);
            let byte = row.find(text).unwrap_or_else(|| panic!("{text:?} in {row}"));
            row[..byte].chars().count()
        };
        frame(&mut canvas, area, &rich("dev-rendu", State::Working), false, UNICODE);
        // The model dimmed, the effort in Claude Code's color, the separators grey, the context plain at work, the
        // time dimmed, the zoom grey.
        assert_eq!(canvas.style(at(&canvas, "Opus"), 0), Style::PLAIN.dim());
        assert_eq!(canvas.style(at(&canvas, "▆"), 0), Style::fg(Color::AnsiValue(141)));
        assert_eq!(canvas.style(at(&canvas, "high"), 0), Style::fg(Color::AnsiValue(141)));
        assert_eq!(canvas.style(at(&canvas, "·"), 0), Style::fg(Color::DarkGrey));
        assert_eq!(canvas.style(at(&canvas, "29"), 0), Style::PLAIN);
        assert_eq!(canvas.style(at(&canvas, "6m"), 0), Style::PLAIN.dim());
        assert_eq!(canvas.style(at(&canvas, "⤢"), 0), Style::fg(Color::DarkGrey));
        // Near compaction orange, where Claude Code warns red; at rest, dimmed, ⟳ before it when it compacts.
        let near = Header { pressure: Pressure::Near, ..rich("dev-rendu", State::Working) };
        frame(&mut canvas, area, &near, false, UNICODE);
        assert_eq!(canvas.style(at(&canvas, "29"), 0), Style::fg(crate::look::ORANGE));
        let warned = Header { pressure: Pressure::Warning, ..rich("dev-rendu", State::Working) };
        frame(&mut canvas, area, &warned, false, UNICODE);
        assert_eq!(canvas.style(at(&canvas, "29"), 0), Style::fg(Color::Red).bold());
        let resting = Header { compactable: true, ..rich("dev-rendu", State::Idle) };
        frame(&mut canvas, area, &resting, false, UNICODE);
        assert!(canvas.row(0).contains(" Opus ▆ high · ⟳ 29 % · 6m "), "{}", canvas.row(0));
        assert_eq!(canvas.style(at(&canvas, "⟳"), 0), Style::PLAIN.dim());
        assert_eq!(canvas.style(at(&canvas, "29"), 0), Style::PLAIN.dim());
        // Waiting, the time in red.
        frame(&mut canvas, area, &rich("dev-rendu", State::Waiting), false, UNICODE);
        assert_eq!(canvas.style(at(&canvas, "6m"), 0), Style::fg(Color::Red));
        // `max` in a rainbow that turns with the spinner.
        let max = Header { effort: Some("max"), ..rich("dev-rendu", State::Working) };
        frame(&mut canvas, area, &max, false, UNICODE);
        let sign = at(&canvas, "█");
        assert_eq!(canvas.style(sign, 0), Style::fg(crate::look::RAINBOW[1]));
        assert_eq!(canvas.style(sign + 2, 0), Style::fg(crate::look::RAINBOW[3]));
        frame(&mut canvas, area, &max, false, Look { frame: 4, ..UNICODE });
        assert_eq!(canvas.style(sign, 0), Style::fg(crate::look::RAINBOW[2]));
        // At rest, still.
        let resting = Header { state: State::Idle, ..max };
        frame(&mut canvas, area, &resting, false, Look { frame: 4, ..UNICODE });
        assert_eq!(canvas.style(at(&canvas, "█"), 0), Style::fg(crate::look::RAINBOW[0]));
    }

    /// F2: where each part can be clicked; the context only when it compacts.
    #[test]
    fn the_header_s_parts() {
        let mut canvas = Canvas::new(72, 3);
        let area = Rect { x: 0, y: 0, width: 72, height: 3 };
        let text =
            |canvas: &Canvas, rect: Rect| canvas.row(0).chars().skip(rect.x).take(rect.width).collect::<String>();
        let parts = frame(&mut canvas, area, &rich("dev-rendu", State::Working), false, UNICODE);
        let shown: Vec<(Part, String)> = parts.iter().map(|(rect, part)| (*part, text(&canvas, *rect))).collect();
        assert_eq!(
            shown,
            [
                (Part::Zoom, " ⤢ ".into()),
                (Part::Name, "dev-rendu".into()),
                (Part::Model, "Opus".into()),
                (Part::Effort, "▆ high".into()),
            ]
        );
        assert!(parts.iter().all(|(rect, _)| rect.y == 0 && rect.height == 1));
        let resting = Header { compactable: true, ..rich("dev-rendu", State::Idle) };
        let parts = frame(&mut canvas, area, &resting, false, UNICODE);
        let context = parts.iter().find(|(_, part)| *part == Part::Context).expect("the context");
        assert_eq!(text(&canvas, context.0), "⟳ 29 %");
        // A rainbow in pieces, one zone.
        let max = Header { effort: Some("max"), ..rich("dev-rendu", State::Working) };
        let parts = frame(&mut canvas, area, &max, false, UNICODE);
        let effort: Vec<_> = parts.iter().filter(|(_, part)| *part == Part::Effort).collect();
        assert_eq!(effort.len(), 1);
        assert_eq!(text(&canvas, effort[0].0), "█ max");
        // A panel: no part.
        let panel = Header { name: "Journal", ..Header::default() };
        assert!(frame(&mut canvas, area, &panel, false, UNICODE).is_empty());
        // Too narrow for a header: nothing to click but the zoom.
        let parts = frame(&mut canvas, Rect { width: 16, ..area }, &rich("dev-rendu", State::Working), false, UNICODE);
        assert_eq!(parts.iter().map(|(_, p)| *p).collect::<Vec<_>>(), [Part::Zoom, Part::Name]);
    }

    /// F3, mock-up B2: the name hovered underlined, « réglages › » after it while the rest keeps some room; another
    /// part underlined; the zoom in bold cyan.
    #[test]
    fn the_header_hovered() {
        let mut canvas = Canvas::new(72, 3);
        let area = Rect { x: 0, y: 0, width: 72, height: 3 };
        let settings = t!("réglages ›", "settings ›");
        let hovered = Header { hover: Some(Part::Name), ..rich("dev-terminal", State::Working) };
        let parts = frame(&mut canvas, area, &hovered, false, UNICODE);
        let row = canvas.row(0);
        assert!(row.starts_with(&format!("╭─ ⠹ dev-terminal {settings} ─")), "{row}");
        assert!(canvas.style(5, 0).underline && !canvas.style(4, 0).underline, "the name underlined");
        assert_eq!(canvas.style(5 + 13, 0), Style::fg(Color::DarkCyan));
        // The name's zone takes « réglages › » too: the mouse stays on it.
        let name = parts.iter().find(|(_, part)| *part == Part::Name).unwrap().0;
        assert_eq!((name.x, name.width), (5, 12 + 1 + columns(&settings)));
        // The rest still there, shorter if need be.
        assert_eq!(row, bordered(&format!("╭─ ⠹ dev-terminal {settings} "), "Opus ▆ high · 29 % · 6m", 72));
        // Narrow: no room for it, the name underlined alone.
        let parts = frame(&mut canvas, Rect { width: 40, ..area }, &hovered, false, UNICODE);
        assert!(!canvas.row(0).contains(&settings), "{}", canvas.row(0));
        assert_eq!(parts.iter().find(|(_, part)| *part == Part::Name).unwrap().0.width, 12);
        // The model, the effort.
        let hovered = Header { hover: Some(Part::Effort), ..rich("dev-rendu", State::Working) };
        frame(&mut canvas, area, &hovered, false, UNICODE);
        let at = canvas.row(0).chars().position(|c| c == '▆').unwrap();
        assert!(canvas.style(at, 0).underline && canvas.style(at + 5, 0).underline);
        assert!(!canvas.style(at - 2, 0).underline, "not the model");
        // The context that compacts, underlined whole as its zone: « ⟳ 29 % ».
        let hovered = Header { hover: Some(Part::Context), compactable: true, ..rich("dev-rendu", State::Idle) };
        frame(&mut canvas, area, &hovered, false, UNICODE);
        let at = canvas.row(0).chars().position(|c| c == '⟳').unwrap();
        assert!((at..at + 6).all(|x| canvas.style(x, 0).underline), "{}", canvas.row(0));
        assert!(!canvas.style(at - 1, 0).underline && !canvas.style(at + 6, 0).underline);
        let hovered = Header { hover: Some(Part::Zoom), ..rich("dev-rendu", State::Working) };
        frame(&mut canvas, area, &hovered, false, UNICODE);
        assert_eq!(canvas.style(72 - 4, 0), Style::fg(Color::DarkCyan).bold());
    }

    /// The scroll keeps its place after the model, the effort's word and the time are gone, before the effort's sign
    /// and the context go.
    #[test]
    fn the_scroll_among_the_rest() {
        let header = Header { scroll: Some((214, 3000)), ..rich("dev-rendu", State::Working) };
        let up = t!("↑ {} sur {}", "↑ {} of {}", 214, 3000);
        let left = format!("╭─ ⠹ dev-rendu · {up} ");
        let short = "╭─ ⠹ dev-rendu · ↑ 214 ";
        let top = |width| framed(&header, width, false).remove(0);
        let wide = columns(&left) + 25 + 7;
        assert_eq!(top(wide), bordered(&left, "Opus ▆ high · 29 % · 6m", wide));
        assert_eq!(top(wide - 1), bordered(&left, "Opus ▆ · 29 % · 6m", wide - 1));
        let at = columns(&left) + 10 + 7;
        assert_eq!(top(at), bordered(&left, "▆ · 29 %", at));
        assert_eq!(top(at - 1), bordered(short, "▆ · 29 %", at - 1));
        let at = columns(short) + 10 + 7;
        assert_eq!(top(at - 1), bordered("╭─ ⠹ dev-rendu ", "▆ · 29 %", at - 1));
        assert_eq!(top(30), bordered("╭─ ⠹ dev-rendu ", "29 %", 30));
    }

    /// The time shows while the header holds it: the screen draws again each second only then.
    /// Panes as wide take the same form, the poorest of theirs, for their columns to line up.
    #[test]
    fn one_form_for_panes_as_wide() {
        // Panes as wide take the poorest form of theirs: pm as dev-interface.
        let short = rich("pm", State::Working);
        let long = rich("dev-interface", State::Working);
        let longest = rich("un-nom-de-membre-vraiment-très-long", State::Working);
        assert_eq!([form(&short, 45), form(&long, 45), form(&longest, 45)], [0, 2, 5]);
        // The very long name holds no one back.
        let common = common_form([&short, &long, &longest].map(|h| form(h, 45)));
        assert_eq!(common, 2);
        let top = |header: Header<'_>| framed(&Header { form: common, ..header }, 45, false).remove(0);
        assert_eq!(top(short), bordered("╭─ ⠹ pm ", "▆ · 29 % · 6m", 45));
        assert_eq!(top(long), bordered("╭─ ⠹ dev-interface ", "▆ · 29 % · 6m", 45));
        assert!(top(longest).starts_with("╭─ ⠹ un-nom-de-membre-vraiment-tr") && !top(longest).contains('%'));
        // None but a very long name, or none at all: no ceiling.
        assert_eq!(common_form([5, 5]), 0);
        assert_eq!(common_form([]), 0);
        // Narrower than its form allows, a pane takes a poorer one all the same.
        let capped = Header { form: 1, ..long };
        assert_eq!(framed(&capped, 40, false)[0], bordered("╭─ ⠹ dev-interface ", "▆ · 29 %", 40));
        // Hovered or scrolled, the form is that of the pane at rest.
        let hovered = Header { hover: Some(Part::Name), scroll: Some((3, 9)), ..long };
        assert_eq!(form(&hovered, 45), 2);
        // Nothing at the right: whole, binding no other; too narrow for anything, none.
        assert_eq!(form(&Header { model: None, effort: None, context: None, since: None, ..long }, 45), 0);
        assert_eq!(form(&long, 20), 5);
    }

    #[test]
    fn whether_the_time_shows() {
        let header = rich("dev-rendu", State::Working);
        let shown = |width| framed(&header, width, false)[0].contains("6m");
        for width in [72, 47, 46, 42, 41, 37, 36, 32, 20] {
            assert_eq!(shows_since(&header, width), shown(width), "{width}");
        }
        assert!(shows_since(&header, 37) && !shows_since(&header, 36));
        assert!(!shows_since(&Header { since: None, ..header }, 72));
        // The scroll goes after the time.
        let scrolled = Header { scroll: Some((3, 9)), ..header };
        assert_eq!(shows_since(&scrolled, 40), framed(&scrolled, 40, false)[0].contains("6m"));
    }

    /// A pane gone wrong keeps its zoom while the whole alert holds beside the whole name; it gives its corner first.
    #[test]
    fn the_zoom_beside_an_alert() {
        let header = Header { note: Some("moteur en échec : panique"), ..rich("dev", State::Idle) };
        let left = "╭─ ✗ dev · moteur en échec : panique ";
        let top = |width| framed(&header, width, false).remove(0);
        let at = columns(left) + 5;
        assert_eq!(top(60), bordered(left, "▆ · 29 % · 6m", 60));
        assert_eq!(top(at), bordered(left, "", at));
        // A column less: the zoom goes, the alert whole.
        assert_eq!(top(at - 1), format!("{left}───╮"));
        assert_eq!(top(at - 3), format!("{left}─╮"));
        // Then the alert shortens, as before.
        assert!(top(at - 4).starts_with("╭─ ✗ dev · moteur en échec ─"), "{}", top(at - 4));
        // No zoom without a member's pane.
        let header = Header { zoom: None, ..header };
        assert!(!framed(&header, 60, false)[0].contains('⤢'));
    }

    fn saisie() -> Notice {
        Notice {
            member: "dev-saisie".into(),
            color: Some(Color::Blue),
            text: notice_text("dev-saisie"),
            tab: "Agents (1)".into(),
        }
    }

    /// F5, mock-up B1: at the top right, on the panes' first row, two columns from the edge, in a red frame.
    #[test]
    fn a_notice_at_the_top_right() {
        let notice = saisie();
        let text = notice_text("dev-saisie");
        let go = t!("y aller", "go");
        let inner = format!(" ⚑ {text} · Agents (1)   {ALT}g {go} ");
        let screen = Rect { x: 0, y: 0, width: 140, height: 39 };
        let area = notice_area(screen, &notice);
        let width = columns(&inner) + 2;
        assert_eq!(area, Rect { x: 140 - 2 - width, y: 1, width, height: 3 });
        let mut canvas = Canvas::new(140, 40);
        canvas.fill(0, 0, 140, 40, Style::PLAIN);
        canvas.put(0, 2, &"x".repeat(140), Style::PLAIN);
        super::notice(&mut canvas, area, &notice, UNICODE);
        let drawn = |y: usize| canvas.row(y).chars().skip(area.x).take(area.width).collect::<String>();
        assert_eq!(drawn(1), format!("╭{}╮", "─".repeat(width - 2)));
        assert_eq!(drawn(2), format!("│{inner}│"));
        assert_eq!(drawn(3), format!("╰{}╯", "─".repeat(width - 2)));
        // What is around it stays, not dimmed.
        assert_eq!(canvas.row(2).chars().take(area.x).collect::<String>(), "x".repeat(area.x));
        assert_eq!(canvas.row(2).chars().skip(area.x + width).collect::<String>(), "xx");
        assert_eq!(canvas.style(area.x, 2), Style::fg(Color::Red));
        // ⚑ in bold red, the name in its color, the text bold, the tab dimmed, the key in cyan.
        let at = |text: &str| {
            let row = canvas.row(2);
            row[..row.find(text).unwrap()].chars().count()
        };
        assert_eq!(canvas.style(at("⚑"), 2), Style::fg(Color::Red).bold());
        assert_eq!(canvas.style(at("dev-saisie"), 2), Style::fg(Color::Blue).bold());
        assert_eq!(canvas.style(at("dev-saisie") + 11, 2), Style::PLAIN.bold());
        assert_eq!(canvas.style(at("Agents"), 2), Style::PLAIN.dim());
        assert_eq!(canvas.style(at(&format!("{ALT}g")), 2), Style::fg(Color::DarkCyan));
    }

    /// A narrow screen: ⌥g goes, then the tab, then the text is cut; too small, no notice.
    #[test]
    fn a_notice_on_a_narrow_screen() {
        let notice = saisie();
        let text = notice_text("dev-saisie");
        let inner = |screen: usize| {
            let area = notice_area(Rect { x: 0, y: 0, width: screen, height: 24 }, &notice);
            let mut canvas = Canvas::new(screen, 24);
            super::notice(&mut canvas, area, &notice, UNICODE);
            let row = canvas.row(2).chars().skip(area.x + 1).take(area.width.saturating_sub(2)).collect::<String>();
            (row, area)
        };
        let with_tab = format!(" ⚑ {text} · Agents (1) ");
        let (row, area) = inner(columns(&with_tab) + 4);
        assert_eq!((row.as_str(), area.x), (with_tab.as_str(), 0));
        let bare = format!(" ⚑ {text} ");
        assert_eq!(inner(columns(&with_tab) + 3).0, bare);
        assert_eq!(inner(columns(&bare) + 4).0, bare);
        let (row, area) = inner(20);
        assert!(columns(&row) <= 16, "{row:?}");
        assert!(row.starts_with(" ⚑ dev-saisie") && row.ends_with("… "), "{row:?}");
        assert_eq!(area.x + area.width, 18);
        assert_eq!(inner(11).1.width, 0, "too narrow");
        assert_eq!(notice_area(Rect { x: 0, y: 0, width: 80, height: 3 }, &notice).width, 0, "too low");
        // Nerd Font: its bell.
        let area = notice_area(Rect { x: 0, y: 0, width: 140, height: 39 }, &notice);
        let mut canvas = Canvas::new(140, 40);
        super::notice(&mut canvas, area, &notice, Look { glyphs: Glyphs::Nerd, frame: 0 });
        assert_eq!(canvas.cell(area.x + 2, 2), "\u{F009E}");
    }

    /// No size, however small, makes the chrome panic: every header, the bar, a notice and the choices, from 0 to
    /// 120 columns and 0 to 40 rows (reviewer's sweep, kept).
    #[test]
    fn no_size_too_small() {
        let long = "un-nom-de-membre-de-quarante-cinq-caractères";
        let mut headers = Vec::new();
        for name in ["a", "dev-rendu", "日本語の名前の長いもの", long] {
            for form in 0..=5 {
                headers.push(Header { form, ..rich(name, State::Working) });
            }
            headers.extend([
                Header { hover: Some(Part::Name), scroll: Some((214, 3000)), ..rich(name, State::Waiting) },
                Header { note: Some("moteur en échec : panique"), compactable: true, ..rich(name, State::Idle) },
                Header {
                    effort: Some("max"),
                    zoom: Some(Zoom::Out),
                    hover: Some(Part::Zoom),
                    ..rich(name, State::Idle)
                },
            ]);
        }
        let counts = Hint::Counts { working: 3, waiting: 12, idle: 140 };
        headers.extend([
            Header { name: "Journal", hint: Some(Hint::JournalSize), ..Header::default() },
            Header { name: "Tableau de bord", hint: Some(counts), ..Header::default() },
            Header::default(),
        ]);
        let titles = ["Interlocuteurs", "Agents (1)", "日本語", "x"];
        let bars: Vec<Vec<Tab<'_>>> = (0..=9)
            .flat_map(|n| {
                let tab = move |i: usize, zoomed| Tab {
                    title: titles[i % titles.len()],
                    active: i == 0,
                    state: [State::Working, State::Waiting, State::Idle][i % 3],
                    zoomed,
                };
                [(0..n).map(|i| tab(i, None)).collect(), (0..n).map(|i| tab(i, Some(long))).collect()]
            })
            .collect();
        let notices = [saisie(), Notice { member: String::new(), text: notice_text(""), ..saisie() }];
        let choices = [quit_choice("mux"), compact_choice(long, Some(88)), compact_choice("dev", None)];
        let looks = [UNICODE, Look { glyphs: Glyphs::Nerd, frame: 7 }];
        // Every small width, which carry nearly all the overflows, then a step; a thread for each share of them.
        let widths: Vec<usize> = (0..=60).chain((64..=120).step_by(4)).collect();
        std::thread::scope(|scope| {
            for chunk in widths.chunks(widths.len().div_ceil(8)) {
                let (headers, bars, notices, choices) = (&headers, &bars, &notices, &choices);
                scope.spawn(move || {
                    // A column and a row more than the widest area: what is drawn out of its area shows there.
                    let mut screen = Canvas::new(121, 42);
                    for look in looks {
                        for &width in chunk {
                            // A header depends on the width; the frame's height on a few rows only.
                            for height in 0..=4 {
                                let area = Rect { x: 0, y: 0, width, height };
                                for header in headers {
                                    within(&mut screen, area, |canvas| {
                                        frame(canvas, area, header, height % 2 == 0, look);
                                    });
                                }
                            }
                            for header in headers {
                                let _ = (form(header, width), shows_since(header, width));
                            }
                            let row = Rect { x: 0, y: 40, width, height: 1 };
                            for tabs in bars {
                                for hover in [None, Some(Target::Tab(1)), Some(Target::Quit)] {
                                    within(&mut screen, row, |canvas| {
                                        bar(canvas, row, "mux", tabs, look, hover);
                                    });
                                }
                            }
                            for height in (0..=20).chain((24..=40).step_by(4)) {
                                let area = Rect { x: 0, y: 0, width, height };
                                for shown in notices {
                                    let at = notice_area(area, shown);
                                    let inside = at.x + at.width <= width && at.y + at.height <= height.max(at.y);
                                    assert!(inside, "{at:?} in {area:?}");
                                    within(&mut screen, at, |canvas| notice(canvas, at, shown, look));
                                }
                                for asked in choices {
                                    let at = choice_area(area, asked);
                                    assert!(
                                        at.x + at.width <= width && at.y + at.height <= height,
                                        "{at:?} in {area:?}"
                                    );
                                    within(&mut screen, at, |canvas| {
                                        choice(canvas, at, asked, look);
                                    });
                                }
                            }
                        }
                    }
                });
            }
        });
        // A frame as high as the screen, for its sides.
        let mut screen = Canvas::new(121, 42);
        for height in 0..=40 {
            let area = Rect { x: 0, y: 0, width: 120, height };
            within(&mut screen, area, |canvas| {
                frame(canvas, area, &headers[0], false, UNICODE);
            });
        }
    }

    /// The witnesses see a cell drawn out of the area.
    #[test]
    #[should_panic(expected = "out of")]
    fn a_stray_cell_seen() {
        let mut canvas = Canvas::new(10, 5);
        let area = Rect { x: 0, y: 0, width: 4, height: 2 };
        within(&mut canvas, area, |canvas| {
            canvas.put(0, 0, "abcde", Style::PLAIN);
        });
    }

    /// Even a stray sign the same as the witness's: its style tells it.
    #[test]
    #[should_panic(expected = "out of")]
    fn a_stray_witness_seen() {
        let mut canvas = Canvas::new(10, 5);
        let area = Rect { x: 0, y: 0, width: 4, height: 2 };
        within(&mut canvas, area, |canvas| {
            canvas.put(4, 1, "¤", Style::PLAIN);
        });
    }

    /// Draws with `draw` in `area` of `canvas`, and checks that nothing went out of it to the right or below: the
    /// column after it and the row under it, each a cell longer, kept as they were.
    fn within(canvas: &mut Canvas, area: Rect, draw: impl FnOnce(&mut Canvas)) {
        let (right, bottom) = (area.x + area.width, area.y + area.height);
        let witnesses: Vec<(usize, usize)> = (area.y..=bottom)
            .map(|y| (right, y))
            .chain((area.x..right).map(|x| (x, bottom)))
            .filter(|&(x, y)| x < canvas.width() && y < canvas.height())
            .collect();
        // A sign and a style the chrome never draws: a stray separator or blank shows as well.
        let witness = Style { strike: true, italic: true, ..Style::PLAIN };
        for &(x, y) in &witnesses {
            canvas.put(x, y, "¤", witness);
        }
        draw(canvas);
        for &(x, y) in &witnesses {
            let kept = canvas.cell(x, y) == "¤" && canvas.style(x, y) == witness;
            assert!(kept, "drawn at {x}, {y}, out of {area:?}");
        }
    }

    #[test]
    fn a_wide_name_cut_whole() {
        let header = member("日本語の名前", State::Idle);
        assert_eq!(framed(&header, 22, false)[0], "╭─ ◷ 日本語の名… ────╮");
        // A character two columns wide never cut in half: one column short, it goes whole.
        assert_eq!(framed(&header, 21, false)[0], "╭─ ◷ 日本語の… ─────╮");
    }

    fn tabs(state: [State; 3]) -> [Tab<'static>; 3] {
        [
            Tab { title: "Interlocuteurs", active: true, state: state[0], zoomed: None },
            Tab { title: "Agents (1)", active: false, state: state[1], zoomed: None },
            Tab { title: "Agents (2)", active: false, state: state[2], zoomed: None },
        ]
    }

    fn drawn_bar(width: usize, tabs: &[Tab<'_>], look: Look) -> (String, Vec<(Rect, Target)>, Canvas) {
        let mut canvas = Canvas::new(width, 1);
        let zones = bar(&mut canvas, Rect { x: 0, y: 0, width, height: 1 }, "mux", tabs, look, None);
        (canvas.row(0), zones, canvas)
    }

    #[test]
    fn the_bar_in_a_wide_window() {
        let tabs = tabs([State::Working, State::Waiting, State::Idle]);
        let (row, zones, canvas) = drawn_bar(100, &tabs, UNICODE);
        let left = " mux │ ⠹ 1 Interlocuteurs │ ⚑ 2 Agents (1) │ ◷ 3 Agents (2) │";
        let menu = format!(" menu ({ALT}r) ");
        let quit = format!(" {} ({ALT}q) ", t!("quitter", "quit"));
        let gap = 100 - columns(left) - columns(&menu) - 1 - columns(&quit) - 1;
        assert_eq!(row, format!("{left}{}{menu} {}", " ".repeat(gap), quit.trim_end()));
        // The current tab in reverse video, ⚑ in red before the one waiting, the others dimmed.
        assert!(canvas.style(7, 0).reverse && canvas.style(9, 0).bold);
        assert_eq!(canvas.style(28, 0), sign_style(State::Waiting));
        assert!(canvas.style(30, 0).dim);
        assert_eq!(
            zones,
            [
                (Rect { x: 6, y: 0, width: 20, height: 1 }, Target::Tab(0)),
                (Rect { x: 27, y: 0, width: 16, height: 1 }, Target::Tab(1)),
                (Rect { x: 44, y: 0, width: 16, height: 1 }, Target::Tab(2)),
                (
                    Rect { x: 99 - columns(&quit) - 1 - columns(&menu), y: 0, width: columns(&menu), height: 1 },
                    Target::Menu
                ),
                (Rect { x: 99 - columns(&quit), y: 0, width: columns(&quit), height: 1 }, Target::Quit),
            ]
        );
        // Each zone's text: its tab or its button.
        let text = |r: Rect| row.chars().skip(r.x).take(r.width).collect::<String>();
        assert_eq!(text(zones[1].0), " ⚑ 2 Agents (1) ");
        assert_eq!(text(zones[4].0), quit.trim_end());
    }

    /// Mock-up B6, then narrower: the other tabs keep their numbers, the team goes, the buttons keep their keys, the
    /// current title is cut.
    #[test]
    fn the_bar_in_a_narrow_window() {
        let tabs = tabs([State::Working, State::Waiting, State::Idle]);
        let menu = format!(" menu ({ALT}r) ");
        let quit = format!(" {} ({ALT}q) ", t!("quitter", "quit"));
        let long = format!("{menu} {quit}");
        let numbers = " mux │ ⠹ 1 Interlocuteurs │ ⚑ 2 │ ◷ 3 │";
        let (row, ..) = drawn_bar(columns(numbers) + 1 + columns(&long) + 1, &tabs, UNICODE);
        assert_eq!(row, format!("{numbers} {}", long.trim_end()));

        let without = "│ ⠹ 1 Interlocuteurs │ ⚑ 2 │ ◷ 3 │";
        let (row, zones, _) = drawn_bar(columns(without) + 1 + columns(&long) + 1, &tabs, UNICODE);
        assert_eq!(row, format!("{without} {}", long.trim_end()));
        assert_eq!(
            zones.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
            [Target::Tab(0), Target::Tab(1), Target::Tab(2), Target::Menu, Target::Quit]
        );

        let keys = format!(" {ALT}r   {ALT}q ");
        let (row, ..) = drawn_bar(columns(without) + 1 + columns(&keys) + 1, &tabs, UNICODE);
        assert_eq!(row, format!("{without} {}", keys.trim_end()));
        let (row, ..) = drawn_bar(columns(without) + 1 + columns(&keys) + 1 - 6, &tabs, UNICODE);
        assert_eq!(row, format!("│ ⠹ 1 Interlo… │ ⚑ 2 │ ◷ 3 │ {}", keys.trim_end()));
    }

    /// Mock-up B4: the current tab says who is zoomed; its title cut before the others lose their room.
    #[test]
    fn the_zoom_in_the_current_tab() {
        let mut tabs = tabs([State::Working, State::Waiting, State::Idle]);
        tabs[0].zoomed = Some("dev-saisie");
        let (row, zones, _) = drawn_bar(140, &tabs, UNICODE);
        assert!(row.starts_with(" mux │ ⠹ 1 Interlocuteurs  ⤢ dev-saisie │ ⚑ 2 Agents (1) │"), "{row}");
        assert_eq!(zones[0].0.width, columns(" ⠹ 1 Interlocuteurs  ⤢ dev-saisie "));
        let nerd = Look { glyphs: Glyphs::Nerd, frame: 2 };
        assert!(drawn_bar(140, &tabs, nerd).0.contains("Interlocuteurs  \u{F0293} dev-saisie"));
        // Narrow, the buttons to their keys (⌥r, or Alt+r off macOS): the others to their numbers first.
        let keys = format!(" {ALT}r   {ALT}q ");
        let room = |tabs: &str| columns(tabs) + 1 + columns(&keys) + 1;
        let numbers = "│ ⠹ 1 Interlocuteurs  ⤢ dev-saisie │ ⚑ 2 │ ◷ 3 │";
        let (row, ..) = drawn_bar(room(numbers), &tabs, UNICODE);
        assert_eq!(row, format!("{numbers} {}", keys.trim_end()));
        // Narrower: the current title goes before who is zoomed, then that is cut, then only ⤢ stays; packed, it
        // stays too, before the buttons go.
        let least = "│ ⠹ 1 ⤢ │ ⚑ 2 │ ◷ 3 │";
        let (row, ..) = drawn_bar(room(least), &tabs, UNICODE);
        assert_eq!(row, format!("{least} {}", keys.trim_end()));
        let (row, zones, _) = drawn_bar(room(least) - 1, &tabs, UNICODE);
        assert!(row.starts_with("│⠹1⤢│⚑2│◷3│") && row.ends_with(keys.trim_end()), "{row}");
        assert_eq!(zones[0].0.width, 3);
        let (row, ..) = drawn_bar(columns("│⠹1⤢│⚑2│◷3│") + 1 + columns(&keys) - 1, &tabs, UNICODE);
        assert_eq!(row.trim_end(), "│⠹1⤢│⚑2│◷3│");
        let zoomed = "│ ⠹ 1 ⤢ dev-saisie │ ⚑ 2 │ ◷ 3 │";
        let (row, ..) = drawn_bar(room(zoomed), &tabs, UNICODE);
        assert_eq!(row, format!("{zoomed} {}", keys.trim_end()));
        let (row, ..) = drawn_bar(room(zoomed) - 4, &tabs, UNICODE);
        assert!(row.starts_with("│ ⠹ 1 ⤢ dev-s") && row.contains("… │ ⚑ 2 │ ◷ 3 │"), "{row}");
    }

    /// F3: the tab or button under the mouse, brighter.
    #[test]
    fn the_bar_hovered() {
        let tabs = tabs([State::Working, State::Waiting, State::Idle]);
        let mut canvas = Canvas::new(100, 1);
        let row = Rect { x: 0, y: 0, width: 100, height: 1 };
        let zones = bar(&mut canvas, row, "mux", &tabs, UNICODE, Some(Target::Tab(1)));
        let label = zones[1].0.x + 3;
        assert_eq!(canvas.style(label, 0), underlined(Style::PLAIN.bold()));
        assert_eq!(canvas.style(label - 1, 0), Style::PLAIN, "the blank before it as it was");
        assert_eq!(canvas.style(zones[2].0.x + 3, 0), Style::PLAIN.dim(), "the others as they were");
        assert_eq!(canvas.style(zones[1].0.x + 1, 0), sign_style(State::Waiting), "its sign as it was");
        let zones = bar(&mut canvas, row, "mux", &tabs, UNICODE, Some(Target::Quit));
        assert_eq!(canvas.style(zones[4].0.x + 1, 0), Style::PLAIN.reverse().bold());
        assert_eq!(canvas.style(zones[3].0.x + 1, 0), Style::PLAIN.reverse());
        // The current tab hovered stays as it is.
        let zones = bar(&mut canvas, row, "mux", &tabs, UNICODE, Some(Target::Tab(0)));
        assert_eq!(canvas.style(zones[0].0.x + 3, 0), underlined(Style::PLAIN.reverse().bold()));
        assert_eq!(canvas.style(zones[0].0.x + 2, 0), Style::PLAIN.reverse().bold());
    }

    /// Narrower still: the blanks go, then the buttons; a sign never.
    #[test]
    fn the_bar_packed() {
        let tabs = tabs([State::Working, State::Waiting, State::Waiting]);
        let keys = format!(" {ALT}r   {ALT}q ");
        let spaced = "│ ⠹ 1 │ ⚑ 2 │ ⚑ 3 │";
        let (row, ..) = drawn_bar(columns(spaced) + 1 + columns(&keys) + 1, &tabs, UNICODE);
        assert_eq!(row, format!("{spaced} {}", keys.trim_end()));
        let packed = "│⠹1│⚑2│⚑3│";
        let (row, zones, canvas) = drawn_bar(columns(spaced) + columns(&keys) + 1, &tabs, UNICODE);
        assert!(row.starts_with(packed) && row.ends_with(keys.trim_end()), "{row}");
        assert_eq!(zones[1].0, Rect { x: 4, y: 0, width: 2, height: 1 });
        assert!(canvas.style(1, 0).reverse && canvas.style(2, 0).reverse, "the current one, reversed");
        assert_eq!(canvas.style(4, 0), sign_style(State::Waiting));
        // Without the buttons, at last.
        let (row, zones, _) = drawn_bar(columns(packed) + 1 + columns(&keys), &tabs, UNICODE);
        assert_eq!(row.trim_end(), packed);
        assert_eq!(zones.len(), 3);
        for width in [12, 10] {
            assert!(drawn_bar(width, &tabs, UNICODE).0.starts_with(packed), "{width}");
        }
        // Narrower than the signs: cut at the edge, the first ones kept.
        assert_eq!(drawn_bar(6, &tabs, UNICODE).0, "│⠹1│⚑2");
    }

    #[test]
    fn nerd_font_signs() {
        let look = Look { glyphs: Glyphs::Nerd, frame: 0 };
        let tabs = [Tab { title: "Agents", state: State::Waiting, ..Tab::default() }];
        let (row, ..) = drawn_bar(60, &tabs, look);
        assert!(row.starts_with(" mux │ \u{F009E} 1 Agents │"), "{row}");
        assert!(row.contains("\u{F035C} menu") && row.contains("\u{F0343} "), "{row}");
    }

    #[test]
    fn the_spinner_turns() {
        let tabs = [Tab { title: "A", active: true, state: State::Working, zoomed: None }];
        let first = drawn_bar(60, &tabs, Look { frame: 0, ..UNICODE }).0;
        let next = drawn_bar(60, &tabs, Look { frame: 1, ..UNICODE }).0;
        assert!(first.starts_with(" mux │ ⠋ 1 A │") && next.starts_with(" mux │ ⠙ 1 A │"));
    }
}
