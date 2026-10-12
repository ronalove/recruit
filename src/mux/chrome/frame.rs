// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A pane's frame and its header: the parts of the header, the forms it takes as the pane narrows, and the places a
//! click reaches.

use crossterm::style::Color;

use super::{Look, sign_style, underlined, zoom_sign};
use crate::t;
use crate::ui::Rect;
use crate::ui::canvas::{Canvas, Style, columns, fit};
use crate::ui::look::{ALT, Glyphs, Pressure, State};

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
    /// Why its member's turn failed, short (`look::error_label`): after the whole name, in orange, while its state is
    /// `State::Error`; « erreur d'API » short of room, then nothing.
    pub error: Option<&'a str>,
    /// How far up its history it shows, and how long that is (`screen::compose` fills it), after the note, dimmed.
    pub scroll: Option<(usize, usize)>,
    /// A key that acts on the pane, at the right of the top border, dimmed, when the rest leaves room.
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
    /// At rest while a command it started still runs (as on its card): a terminal's sign
    /// instead of the state's, dimmed. Work, when it comes back, prevails.
    pub shell: bool,
    /// The zoom's sign in the corner, ⤢ or ⤡; `None` for a pane that is not a member's.
    pub zoom: Option<Zoom>,
    /// The part under the mouse: the name underlined and « réglages › » after it; another part underlined.
    pub hover: Option<Part>,
    /// The part whose list of values is open under it (the model or the effort): on the list cursor's background while
    /// it is, its colors kept.
    pub open: Option<Part>,
    /// The richest form its right part may take, from 0 (all of it) to 6 (none): the poorest that [`form`] gives
    /// the members' panes as wide as its own, for their columns to line up. 0 by default.
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
    /// The dashboard: how many members work, wait for the user, failed their turn and rest (« ⠹ 5  ⚑ 1  ✗ 1  ◷ 3 »).
    Counts { working: usize, waiting: usize, error: usize, idle: usize },
}

impl Hint {
    /// Its pieces, each in its style: the key dimmed; each count's sign and figure in its state's color, none for a
    /// state no one is in.
    fn pieces(self, look: Look) -> Vec<(String, Style)> {
        match self {
            Hint::JournalSize => vec![(t!("{}j taille", "{}j size", ALT), Style::PLAIN.dim())],
            Hint::Counts { working, waiting, error, idle } => {
                let mut pieces = Vec::new();
                let counts =
                    [(State::Working, working), (State::Waiting, waiting), (State::Error, error), (State::Idle, idle)];
                for (state, count) in counts {
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
/// corner): the header never runs into them, so that it stays put when the zoom comes.
const CORNER: usize = 5;

/// A frame's style: red for a pane gone wrong and for a member waiting, orange for a member whose turn failed, with
/// the focus or without (they ask for an action: the focus shows by the thickness); else the terminal's text color for
/// the focus; grey for the rest, the dashboard and the journal included. Bold with the focus.
fn frame_style(header: &Header<'_>, focused: bool) -> Style {
    let member = |state: State| header.color.is_some() && header.state == state;
    let style = if header.note.is_some() || member(State::Waiting) {
        Style::fg(Color::Red)
    } else if member(State::Error) {
        Style::fg(crate::ui::look::palette().orange)
    } else if focused && header.color.is_some() {
        Style::PLAIN
    } else {
        Style::fg(Color::DarkGrey)
    };
    if focused { style.bold() } else { style }
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
/// blanks that cut the line; at the right « Opus ● high · ⟳ 22 % · 18m », then ⤢ in the corner.
/// An alert comes first, whole, else short (what comes before its « : »), the name cut for it down to three columns,
/// and cut itself last; then the name, whole while it holds. What is left goes, short of room, in turn:
/// the effort's word, the model, the time; the scroll, whole then short (« ↑ 214 »); the effort's sign;
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
        parts.push(put_zoom(canvas, right, y, zoom, header, look));
    }
    let Some(name) = &plan.name else { return parts };
    let mut x = put_name(canvas, area, &plan, name, header, look, &mut parts);
    if let Some(text) = &plan.reason {
        x = put_part(canvas, x, y, text, Style::fg(crate::ui::look::palette().orange));
    }
    if let Some(text) = &plan.alert {
        x = put_part(canvas, x, y, text, Style::fg(Color::Red));
    }
    if let Some(text) = &plan.scroll {
        x = put_part(canvas, x, y, text, Style::PLAIN.dim());
    }
    if let Some(meta) = plan.meta.map(|i| &metas[i]) {
        put_meta(canvas, area, plan.stop, meta, header, &mut parts);
    }
    put_hint(canvas, right, y, x, header, look);
    parts
}

/// `style`, underlined when `part` is under the mouse.
fn hovered(header: &Header<'_>, part: Part, style: Style) -> Style {
    if header.hover == Some(part) { underlined(style) } else { style }
}

/// The zoom's sign in the corner, between two blanks; where a click on it lands.
fn put_zoom(canvas: &mut Canvas, right: usize, y: usize, zoom: Zoom, header: &Header<'_>, look: Look) -> (Rect, Part) {
    let on = header.hover == Some(Part::Zoom);
    let style = if on { Style::fg(Color::DarkCyan).bold() } else { Style::fg(Color::DarkGrey) };
    canvas.put(right - 4, y, " ", Style::PLAIN);
    canvas.put(right - 3, y, zoom_sign(zoom, look.glyphs).encode_utf8(&mut [0; 4]), style);
    canvas.put(right - 2, y, " ", Style::PLAIN);
    (Rect { x: right - 4, y, width: 3, height: 1 }, Part::Zoom)
}

/// The state's sign and the name, then « réglages › » when the plan has it; the column after them. The name is a
/// part a click reaches, for a member.
fn put_name(
    canvas: &mut Canvas,
    area: Rect,
    plan: &Plan,
    name: &str,
    header: &Header<'_>,
    look: Look,
    parts: &mut Vec<(Rect, Part)>,
) -> usize {
    let y = area.y;
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
                State::Working | State::Waiting | State::Error => Style::fg(color).bold(),
            }
        }
        None => Style::PLAIN,
    };
    let name_at = x;
    let mut name_end = canvas.put(x, y, name, hovered(header, Part::Name, name_style));
    x = canvas.put(name_end, y, " ", Style::PLAIN);
    if plan.settings {
        name_end = canvas.put(x, y, &settings(), Style::fg(Color::DarkCyan));
        x = canvas.put(name_end, y, " ", Style::PLAIN);
    }
    if header.color.is_some() && name_end > name_at {
        parts.push((Rect { x: name_at, y, width: name_end - name_at, height: 1 }, Part::Name));
    }
    x
}

/// The right part (model, effort, context, time) ending before column `stop` of `area`, each piece in its style; the
/// parts a click reaches are added to `parts`, the one whose list is open on the list's background.
fn put_meta(
    canvas: &mut Canvas,
    area: Rect,
    stop: usize,
    meta: &[Piece],
    header: &Header<'_>,
    parts: &mut Vec<(Rect, Part)>,
) {
    let y = area.y;
    let width: usize = meta.iter().map(|(text, ..)| columns(text)).sum();
    let mut at = canvas.put(area.x + stop - width - 1, y, " ", Style::PLAIN);
    for (text, style, part) in meta {
        let from = at;
        let style = part.map_or(*style, |part| {
            let style = hovered(header, part, *style);
            // The part whose list is open, on the background of the list's cursor, in its colors: the model no longer
            // dimmed (dim on that background is grey on grey).
            if header.open == Some(part) {
                Style { dim: false, ..style }.on(crate::ui::look::palette().selected)
            } else {
                style
            }
        });
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
    // Its background reaches the blank on each side: a sign that is a full block (█) hides it, and the part reads as
    // one target.
    if let Some((rect, _)) = parts.iter().find(|(_, part)| Some(*part) == header.open) {
        // What is there is always a blank: the one before the right part, a separator, the one after it.
        for x in [rect.x.checked_sub(1), Some(rect.x + rect.width)].into_iter().flatten() {
            canvas.tint(x, y, 1, crate::ui::look::palette().selected);
        }
    }
}

/// The hint at the right between blanks, a dash before the corner, after a dash at least; `x` is the column the rest
/// of the header ends at.
fn put_hint(canvas: &mut Canvas, right: usize, y: usize, x: usize, header: &Header<'_>, look: Look) {
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
    /// Why its member's turn failed, whole or short.
    reason: Option<String>,
    alert: Option<String>,
    scroll: Option<String>,
    /// The form of the right part shown, by its place in `metas`.
    meta: Option<usize>,
}

/// The header's layout in a pane `width` columns wide, with the forms `metas` of its right part: what [`title`]
/// draws, without drawing it.
fn plan(width: usize, header: &Header<'_>, metas: &[Vec<Piece>; FORMS]) -> Plan {
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
    // « recruit » already.
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
    let whole_name = name == header.name;
    plan.name = Some(name);
    // A failed turn's reason after the whole name: whole, else short, else none. The right part makes
    // room for it.
    plan.reason =
        reasons(header).into_iter().find(|text| whole_name && x + part(text) <= end).inspect(|text| x += part(text));
    // Hovered, « réglages › » after the name, while it leaves the rest some room.
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

/// The form the right part of `header` takes on its own in a pane `width` columns wide, from 0 (all of it) to `FORMS`
/// (none), neither hovered nor scrolled. 0 for a header with nothing to show there: it holds whole. The panes as wide
/// share their [`common_form`]. `FORMS` for a member whose turn failed: its reason takes room from its own pane only,
/// and holds no one back.
pub(crate) fn form(header: &Header<'_>, width: usize) -> usize {
    if !reasons(header).is_empty() {
        return FORMS;
    }
    let alone = Header { hover: None, scroll: None, form: 0, ..*header };
    let metas = metas(&alone, Look::default());
    if metas.iter().all(Vec::is_empty) {
        return 0;
    }
    plan(width, &alone, &metas).meta.unwrap_or(FORMS)
}

/// The [`Header::form`] of the members' panes as wide, from their own [`form`]s: the poorest, for their columns to line
/// up as the dashboard's cards of a width do; but a pane with nothing left at the right (`FORMS`, a very long name)
/// holds no one back, and gets by alone.
pub(crate) fn common_form(forms: impl IntoIterator<Item = usize>) -> usize {
    forms.into_iter().filter(|&form| form < FORMS).max().unwrap_or(0)
}

/// Whether a header shows its time in the state in a pane `width` columns wide (its whole area, as [`frame`] gets
/// it): while it does under a minute, its seconds change, and the screen must be drawn again each second.
pub(crate) fn shows_since(header: &Header<'_>, width: usize) -> bool {
    header.since.is_some() && plan(width, header, &metas(header, Look::default())).meta.is_some_and(|form| form < 3)
}

/// The forms of a failed turn's reason in a member's header, from the richest: as given (« API surchargée (529) »),
/// without its code as the notice says it (« API surchargée »), then « erreur d'API »; none
/// out of `State::Error`, or for a pane gone wrong, whose note says more.
fn reasons(header: &Header<'_>) -> Vec<String> {
    let Some(label) = header.error.filter(|_| header.state == State::Error && header.note.is_none()) else {
        return Vec::new();
    };
    let mut forms = vec![label.to_string()];
    for form in [crate::ui::look::without_code(label).to_string(), crate::ui::look::error_short()] {
        if !forms.contains(&form) {
            forms.push(form);
        }
    }
    forms
}

/// Columns « réglages › » leaves at least for the rest of the header.
const SETTINGS_ROOM: usize = 12;

/// A piece of the header's right part: its text, its style, and the part a click on it reaches.
type Piece = (String, Style, Option<Part>);

/// How many forms the right part has, from the richest (0) to the context alone: past them, nothing.
pub(super) const FORMS: usize = 6;

/// The forms of the header's right part, from the richest: `Opus ◉ extra · 40 % · 12m`; without
/// the effort's name; the model as its initial; without the time; without the initial; the context alone. Each empty
/// for a pane that has none of what it shows.
fn metas(header: &Header<'_>, look: Look) -> [Vec<Piece>; FORMS] {
    let context = context_pieces(header);
    let time = time_pieces(header);
    // Groups joined by « · »: the model and the effort, the context, the time.
    let join = |model_form: Option<bool>, effort_form: Option<bool>, context_too: bool, time_too: bool| -> Vec<Piece> {
        let mut first: Vec<Piece> = Vec::new();
        if let Some(initial) = model_form
            && let Some(model) = model_piece(header, initial)
        {
            first.push(model);
        }
        if let Some(word) = effort_form {
            let effort = effort_pieces(header, look, word);
            if !effort.is_empty() && !first.is_empty() {
                first.push((" ".to_string(), Style::PLAIN, None));
            }
            first.extend(effort);
        }
        let groups = [
            first,
            if context_too { context.clone() } else { Vec::new() },
            if time_too { time.clone() } else { Vec::new() },
        ];
        let mut out = Vec::new();
        for group in groups.into_iter().filter(|g| !g.is_empty()) {
            if !out.is_empty() {
                out.push((" · ".to_string(), Style::fg(Color::DarkGrey), None));
            }
            out.extend(group);
        }
        out
    };
    [
        join(Some(false), Some(true), true, true),
        join(Some(false), Some(false), true, true),
        join(Some(true), Some(false), true, true),
        join(Some(true), Some(false), true, false),
        join(None, Some(false), true, false),
        join(None, None, true, false),
    ]
}

/// The model, its name or its initial, dimmed; `?` while it is not known, but with an effort to say beside it (a pane
/// with nothing known shows nothing).
fn model_piece(header: &Header<'_>, initial: bool) -> Option<Piece> {
    if header.model.is_none() && header.effort.is_none() {
        return None;
    }
    let text = match header.model {
        Some(model) if !initial => model.to_string(),
        model => crate::ui::look::model_initial(model).to_string(),
    };
    Some((text, Style::PLAIN.dim(), Some(Part::Model)))
}

/// The effort's sign, with its name when `word`, each in its color.
fn effort_pieces(header: &Header<'_>, look: Look, word: bool) -> Vec<Piece> {
    let Some(level) = header.effort else { return Vec::new() };
    // Still out of work: an image drawn for something else does not change its colors.
    let turn = if header.state == State::Working { look.frame / 2 } else { 0 };
    crate::ui::look::effort_cells(level, word, turn)
        .into_iter()
        .map(|(c, color)| (c.to_string(), color.map_or(Style::PLAIN, Style::fg), Some(Part::Effort)))
        .collect()
}

/// The context's figure, with ⟳ before it when a click compacts the member.
fn context_pieces(header: &Header<'_>) -> Vec<Piece> {
    let Some(percent) = header.context else { return Vec::new() };
    let resting = matches!(header.state, State::Idle | State::Other);
    let style = match header.pressure {
        Pressure::Warning => Style::fg(Color::Red).bold(),
        Pressure::Near => Style::fg(crate::ui::look::palette().orange),
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

/// The time in the state, red for a member that waits.
fn time_pieces(header: &Header<'_>) -> Vec<Piece> {
    let Some(secs) = header.since else { return Vec::new() };
    let style = if header.state == State::Waiting { Style::fg(Color::Red) } else { Style::PLAIN.dim() };
    vec![(crate::ui::look::duration(secs), style, None)]
}

/// Which form of the right part (`metas`) and of the scroll (`scrolls`: whole, short) hold together after the left
/// part ends at `x` (its blank included): the scroll before `end`, the right part ending at `stop` with a blank on
/// each side and at least a dash before it. In turn: the right part loses its effort's name, its model's name for its
/// initial, its time, its initial; then the scroll shortens and goes; then the right part loses its effort's sign, then
/// itself, and the scroll comes back if it holds alone.
fn layout(
    x: usize,
    end: usize,
    stop: usize,
    metas: &[Vec<Piece>; FORMS],
    scrolls: &[String],
    richest: usize,
) -> (Option<usize>, Option<usize>) {
    let (whole, short) = ((!scrolls.is_empty()).then_some(0), (scrolls.len() > 1).then_some(1));
    // No richer than `richest`: a form before it stands for it.
    let meta = |i: usize| Some(i.max(richest)).filter(|&i| i < metas.len() && !metas[i].is_empty());
    let mut tries: Vec<(Option<usize>, Option<usize>)> = (0..=4).map(|i| (meta(i), whole)).collect();
    tries.extend([(meta(4), short), (meta(4), None), (meta(5), None), (None, whole), (None, short), (None, None)]);
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
