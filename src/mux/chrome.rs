// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The chrome around the panes, direction B « Cadres » (CLAUDE.md, decision of 2026-10-09; mock-up
//! https://claude.ai/artifact/MbjmHDAzeN3mNK9jdqmXQ7): each pane in a frame, its header written in the top border.
//! The pane with the focus has a thick frame in bold, in the terminal's own text color, at work as at rest; the
//! others thin and grey. Only red stays, for a member waiting and for a pane gone wrong, which ask for an action:
//! thick with the focus, thin without (the active pane, 2026-10-09, option A of
//! https://claude.ai/artifact/Vy6KJsgaxqDUwNuXMLFrt5, as the user corrected it). The header keeps the state's
//! styles: the spinner by the name of a member at work, ⚑ in red for one waiting, the name in the member's color.
//! Then the bar on the screen's last row: the team, the tabs (the current one in reverse video, each after the sign
//! of its most urgent member: ⚑ for one waiting), then the menu and quit buttons. Step 1: state, name, an alert and
//! the scroll; model, effort, context and time come with the dashboard's data (steps 2, 3), right-aligned in the
//! border before the zoom's corner.
//!
//! Owner: dev-interface. The signatures are the interface with `screen.rs` (dev-rendu) and the server (dev-serveur).

use crossterm::style::Color;

use super::Rect;
use crate::canvas::{Canvas, Style, columns, fit};
use crate::look::{Glyphs, State};
use crate::t;
use crate::tmux::ALT;

/// How the chrome draws its signs: Nerd Font or Unicode, and the spinner's image (`look::frame`) for the members at
/// work. While a header or a tab shows one at work, the screen is drawn again every `look::FRAME`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Look {
    pub glyphs: Glyphs,
    pub frame: usize,
}

/// What a pane's frame says of it.
#[derive(Clone, Copy, Debug)]
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
}

/// A key a panel's border reminds of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hint {
    /// The journal: ⌥j takes it to its next size.
    JournalSize,
}

impl Hint {
    fn text(self) -> String {
        match self {
            Hint::JournalSize => t!("{}j taille", "{}j size", ALT),
        }
    }
}

/// A tab in the bar.
pub(crate) struct Tab<'a> {
    pub title: &'a str,
    pub active: bool,
    /// The most urgent of its members' states: waiting, else working, else at rest.
    pub state: State,
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
pub(crate) fn frame(canvas: &mut Canvas, area: Rect, header: &Header<'_>, focused: bool, look: Look) {
    let Rect { x, y, width, height } = area;
    if width < 2 || height < 2 {
        return;
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
    title(canvas, area, header, look);
}

/// The header in the top border, from its third column: « ⠹ name · alert · ↑ 214 sur 3000 », each part between
/// blanks that cut the line. An alert comes first, whole, else short (what comes before its « : »), the name cut for
/// it down to three columns, and cut itself last; then the name; the scroll takes what they leave, whole, else short
/// (« ↑ 214 »), else nothing. Under three columns of the name, the border goes without a header.
fn title(canvas: &mut Canvas, area: Rect, header: &Header<'_>, look: Look) {
    let right = area.x + area.width - 1;
    let start = area.x + 2;
    // The header's columns end before the zoom's corner, or a dash before the frame's corner in a narrow pane.
    // An alert takes the zoom's corner too: the zoom goes before the alert is shortened.
    let end = if area.width >= 16 && header.note.is_none() { right + 1 - CORNER } else { right - 1 };
    let lead = match header.color {
        Some(_) => 3,
        None => 1,
    };
    // At least three columns of the name, and a blank.
    if end < start + lead + 3 + 1 {
        return;
    }
    let mut x = canvas.put(start, area.y, " ", Style::PLAIN);
    let name_style = match header.color {
        Some(color) => {
            // Ended or failed, nothing runs there: a cross rather than the state's sign.
            let (sign, style) = match header.note {
                Some(_) => (failed(look.glyphs), Style::fg(Color::Red)),
                None => (header.state.icon(look.glyphs, look.frame), sign_style(header.state)),
            };
            x = canvas.put(x, area.y, sign.encode_utf8(&mut [0; 4]), style);
            x = canvas.put(x, area.y, " ", Style::PLAIN);
            match header.state {
                State::Idle | State::Other => Style::fg(color).dim(),
                State::Working | State::Waiting => Style::fg(color).bold(),
            }
        }
        None => Style::PLAIN,
    };
    let room = end - x;
    let alert = header.note.and_then(|note| alert(note, columns(header.name) + 1, room));
    let alert_room = alert.as_ref().map_or(0, |text| part(text));
    x = put_name(canvas, x, area.y, end - alert_room, header.name, name_style);
    if let Some(text) = &alert {
        x = put_part(canvas, x, area.y, text, Style::fg(Color::Red));
    }
    // The scroll only beside a whole alert, if any: a shortened one leaves it no room.
    let whole_alert = alert.as_deref().is_none_or(|text| Some(text) == header.note);
    if let Some((up, of)) = header.scroll.filter(|_| whole_alert) {
        let whole = t!("↑ {} sur {}", "↑ {} of {}", up, of);
        let short = format!("↑ {up}");
        if let Some(text) = [whole, short].into_iter().find(|text| x + part(text) <= end) {
            x = put_part(canvas, x, area.y, &text, Style::PLAIN.dim());
        }
    }
    // The hint at the right, a dash before the corner, after a dash at least.
    if let Some(hint) = header.hint {
        let text = format!(" {} ", hint.text());
        let start = right.saturating_sub(1 + columns(&text));
        if start > x {
            canvas.put_in(start, area.y, right - 1, &text, Style::PLAIN.dim());
        }
    }
    // TODO(steps 2, 3): model, effort, context and time, right-aligned before `end`. What goes when the room lacks,
    // in turn (designer, 2026-10-09): the effort's word, the model, the time; then the scroll; then the effort's sign,
    // the context; the name is cut last. An alert stays before all of them.
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

/// The name, cut to fit before `end` with a blank after it; returns the column after the blank.
fn put_name(canvas: &mut Canvas, x: usize, y: usize, end: usize, name: &str, style: Style) -> usize {
    let room = end.saturating_sub(x + 1);
    let x = canvas.put_in(x, y, end, &fit(name, room), style);
    canvas.put_in(x, y, end, " ", Style::PLAIN)
}

/// How much a tab says: all of it, or only its number when it is not the current one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Labels {
    Whole,
    Numbers,
}

/// A tab's label: its number, which `Alt+1`…`Alt+9` reach, and its title.
fn label(i: usize, tab: &Tab<'_>, labels: Labels) -> String {
    if tab.active || labels == Labels::Whole { format!("{} {}", i + 1, tab.title) } else { (i + 1).to_string() }
}

/// The buttons on the right of the bar, long or only their keys.
fn buttons(long: bool, glyphs: Glyphs) -> [(String, Target); 2] {
    let (menu, quit) = match glyphs {
        // nf-md-menu, nf-md-logout
        Glyphs::Nerd => ("\u{F035C} ", "\u{F0343} "),
        Glyphs::Unicode => ("", ""),
    };
    if long {
        [
            (format!(" {menu}menu ({ALT}r) "), Target::Menu),
            (format!(" {quit}{} ({ALT}q) ", t!("quitter", "quit")), Target::Quit),
        ]
    } else {
        [(format!(" {ALT}r "), Target::Menu), (format!(" {ALT}q "), Target::Quit)]
    }
}

/// The bar on `row` (the screen's last): the team, the tabs between rules, then the buttons on the right, a column
/// from the edge. Returns where each can be clicked. When it is short of room, in turn: the other tabs keep only
/// their numbers, the team goes (the terminal's title has it), the buttons keep only their keys (the only help in
/// sight for a newcomer), the current tab's title is cut. The tabs' signs always stay: they are the badges.
pub(crate) fn bar(canvas: &mut Canvas, row: Rect, team: &str, tabs: &[Tab<'_>], look: Look) -> Vec<(Rect, Target)> {
    let end = row.x + row.width;
    let team = format!(" {team} ");
    let tabs_width = |labels: Labels| -> usize {
        // Each tab: a rule, then « ⠹ label » between blanks; a rule after the last.
        tabs.iter().enumerate().map(|(i, tab)| 1 + 4 + columns(&label(i, tab, labels))).sum::<usize>() + 1
    };
    let buttons_width =
        |long: bool| -> usize { buttons(long, look.glyphs).iter().map(|(text, _)| columns(text) + 1).sum::<usize>() };
    // The forms from the richest, and the first that leaves a blank before the buttons.
    let forms = [
        (true, Labels::Whole, true),
        (true, Labels::Numbers, true),
        (false, Labels::Numbers, true),
        (false, Labels::Numbers, false),
    ];
    let (with_team, labels, long) = forms
        .into_iter()
        .find(|&(with_team, labels, long)| {
            let team = if with_team { columns(&team) } else { 0 };
            team + tabs_width(labels) + 1 + buttons_width(long) <= row.width
        })
        .unwrap_or((false, Labels::Numbers, false));

    let mut zones = Vec::new();
    // The buttons first, from the right: the tabs end before them.
    let mut right = end.saturating_sub(1);
    let mut placed = Vec::new();
    for (text, target) in buttons(long, look.glyphs).into_iter().rev() {
        let start = right.saturating_sub(columns(&text)).max(row.x);
        canvas.put_in(start, row.y, right, &text, Style::PLAIN.reverse());
        placed.push((Rect { x: start, y: row.y, width: right - start, height: 1 }, target));
        right = start.saturating_sub(1);
    }
    let limit = right.max(row.x);

    let mut x = row.x;
    if with_team {
        x = canvas.put_in(x, row.y, limit, &team, Style::PLAIN.bold());
    }
    let rule = Style::fg(Color::DarkGrey);
    // What the other tabs take after the current one, for its title to leave them their room.
    let after_active = |from: usize| -> usize {
        tabs[from..].iter().enumerate().map(|(k, tab)| 1 + 4 + columns(&label(from + k, tab, labels))).sum::<usize>()
            + 1
    };
    for (i, tab) in tabs.iter().enumerate() {
        x = canvas.put_in(x, row.y, limit, "│", rule);
        let mut text = label(i, tab, labels);
        if tab.active {
            let room = limit.saturating_sub(x + 4 + after_active(i + 1));
            text = fit(&text, room.max(1));
        }
        let start = x;
        let sign = tab.state.icon(look.glyphs, look.frame).to_string();
        let sign = sign.as_str();
        if tab.active {
            let on = Style::PLAIN.reverse();
            x = canvas.put_in(x, row.y, limit, " ", on);
            x = canvas.put_in(x, row.y, limit, sign, on);
            x = canvas.put_in(x, row.y, limit, &format!(" {text} "), on.bold());
        } else {
            x = canvas.put_in(x, row.y, limit, " ", Style::PLAIN);
            x = canvas.put_in(x, row.y, limit, sign, sign_style(tab.state));
            x = canvas.put_in(x, row.y, limit, &format!(" {text} "), Style::PLAIN.dim());
        }
        if x > start {
            zones.push((Rect { x: start, y: row.y, width: x - start, height: 1 }, Target::Tab(i)));
        }
    }
    canvas.put_in(x, row.y, limit, "│", rule);
    zones.extend(placed.into_iter().rev());
    zones
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
/// (CLAUDE.md, 2026-10-08), with nothing more in the labels, under « recruit » as tmux's quit menu. Opens on the
/// cancel. Its options' order is the server's: detach, quit, cancel.
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

/// The confirmation of a click on a member's context on the dashboard, under tmux (`app::compact`) as in recruit's own
/// multiplexer (user's choice, 2026-10-09): « Compacter <membre> ? », what it does with the member's context as the
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
        let key_style = if on { base.bold() } else { Style::fg(Color::Cyan).bold() };
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
        Header { name, color: Some(Color::Magenta), state, note: None, scroll: None, hint: None }
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
        assert_eq!(canvas.style(area.x + inner - 1, area.y + 2), Style::fg(Color::Cyan).bold());
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
        let panel = Header { name: "Journal", color: None, state: State::Other, note: None, scroll: None, hint: None };
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

    #[test]
    fn a_panel_grey_and_plain() {
        let header =
            Header { name: "Journal", color: None, state: State::Working, note: None, scroll: None, hint: None };
        assert_eq!(framed(&header, 20, false)[0], "╭─ Journal ────────╮");
        let mut canvas = Canvas::new(20, 3);
        frame(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 3 }, &header, false, UNICODE);
        assert_eq!(canvas.style(0, 0), Style::fg(Color::DarkGrey));
        assert_eq!(canvas.style(3, 0), Style::PLAIN);
    }

    /// Mock-up B1: the journal's key at the right of its border, while the title leaves room.
    #[test]
    fn the_journal_s_key_in_its_border() {
        let header = Header {
            name: "Journal",
            color: None,
            state: State::Other,
            note: None,
            scroll: None,
            hint: Some(Hint::JournalSize),
        };
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

    #[test]
    fn a_wide_name_cut_whole() {
        let header = member("日本語の名前", State::Idle);
        assert_eq!(framed(&header, 22, false)[0], "╭─ ◷ 日本語の名… ────╮");
        // A character two columns wide never cut in half: one column short, it goes whole.
        assert_eq!(framed(&header, 21, false)[0], "╭─ ◷ 日本語の… ─────╮");
    }

    fn tabs(state: [State; 3]) -> [Tab<'static>; 3] {
        [
            Tab { title: "Interlocuteurs", active: true, state: state[0] },
            Tab { title: "Agents (1)", active: false, state: state[1] },
            Tab { title: "Agents (2)", active: false, state: state[2] },
        ]
    }

    fn drawn_bar(width: usize, tabs: &[Tab<'_>], look: Look) -> (String, Vec<(Rect, Target)>, Canvas) {
        let mut canvas = Canvas::new(width, 1);
        let zones = bar(&mut canvas, Rect { x: 0, y: 0, width, height: 1 }, "mux", tabs, look);
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

    #[test]
    fn nerd_font_signs() {
        let look = Look { glyphs: Glyphs::Nerd, frame: 0 };
        let tabs = [Tab { title: "Agents", active: false, state: State::Waiting }];
        let (row, ..) = drawn_bar(60, &tabs, look);
        assert!(row.starts_with(" mux │ \u{F009E} 1 Agents │"), "{row}");
        assert!(row.contains("\u{F035C} menu") && row.contains("\u{F0343} "), "{row}");
    }

    #[test]
    fn the_spinner_turns() {
        let tabs = [Tab { title: "A", active: true, state: State::Working }];
        let first = drawn_bar(60, &tabs, Look { frame: 0, ..UNICODE }).0;
        let next = drawn_bar(60, &tabs, Look { frame: 1, ..UNICODE }).0;
        assert!(first.starts_with(" mux │ ⠋ 1 A │") && next.starts_with(" mux │ ⠙ 1 A │"));
    }
}
