// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The bar on the screen's last row: the team, the tabs, the menu and quit buttons.

use super::Zoom;
use crossterm::style::Color;

use super::{Look, sign_style, underlined, zoom_sign};
use crate::t;
use crate::ui::Rect;
use crate::ui::canvas::{Canvas, Style, columns, fit};
use crate::ui::look::{ALT, Glyphs, State};

/// A tab in the bar.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Tab<'a> {
    pub title: &'a str,
    pub active: bool,
    /// The most urgent of its members' states: waiting, else working, else at rest.
    pub state: State,
    /// The member whose pane takes the tab's room, zoomed: « ⤢ name » after the title.
    pub zoomed: Option<&'a str>,
    /// What the layout button offers while this tab is the current one: none for a tab that
    /// has no « focus » to go to or come back from.
    pub switch: Option<Switch>,
}

/// What the bar's layout button does to the current tab, and says by the layout it goes to: « grille (⌥l) » from the
/// « focus », « focus (⌥l) » from the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Switch {
    ToGrid,
    ToFocus,
}

impl Switch {
    fn word(self) -> String {
        match self {
            Switch::ToGrid => t!("grille", "grid"),
            Switch::ToFocus => "focus".to_string(),
        }
    }
}

/// What a click on the bar reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Tab(usize),
    /// The layout button: the current tab goes to the other layout until the client detaches.
    Layout,
    Menu,
    Quit,
}

/// How much a tab says: all of it, or only its number when it is not the current one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Labels {
    Whole,
    Numbers,
}

/// A tab's label: its number, which `Alt+1`…`Alt+9` reach, and its title.
/// The current one zoomed: « 2 Agents (1)  ⤢ dev-saisie ».
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
fn buttons(shown: Buttons, glyphs: Glyphs, switch: Option<Switch>) -> Vec<(String, Target)> {
    let (menu, quit) = match glyphs {
        // nf-md-menu, nf-md-logout
        Glyphs::Nerd => ("\u{F035C} ", "\u{F0343} "),
        Glyphs::Unicode => ("", ""),
    };
    // The layout button first, before the menu's.
    let layout = switch.map(|switch| match shown {
        Buttons::Long => (format!(" {} ({ALT}l) ", switch.word()), Target::Layout),
        _ => (format!(" {ALT}l "), Target::Layout),
    });
    let rest = match shown {
        Buttons::Long => vec![
            (format!(" {menu}menu ({ALT}r) "), Target::Menu),
            (format!(" {quit}{} ({ALT}q) ", t!("quitter", "quit")), Target::Quit),
        ],
        Buttons::Keys => vec![(format!(" {ALT}r "), Target::Menu), (format!(" {ALT}q "), Target::Quit)],
        Buttons::None => Vec::new(),
    };
    layout.filter(|_| shown != Buttons::None).into_iter().chain(rest).collect()
}

/// The bar on `row` (the screen's last): the team, the tabs between rules, then the buttons on the right, a column
/// from the edge. Returns where each can be clicked. When it is short of room, in turn: the other tabs keep only
/// their numbers, the team goes (the terminal's title has it), the buttons keep only their keys (the only help in
/// sight for a newcomer), the current tab's title is cut down to its number; then the blanks go (« │⠴1│⚑2│⚑3│ »),
/// then the buttons. The tabs' signs always stay: they are the badges. `hover`, the tab or button under the mouse,
/// brighter: an other tab no longer dimmed and bold, a button bold.
pub(crate) fn bar(
    canvas: &mut Canvas,
    row: Rect,
    team: &str,
    tabs: &[Tab<'_>],
    look: Look,
    hover: Option<Target>,
) -> Vec<(Rect, Target)> {
    let team = format!(" {team} ");
    let switch = tabs.iter().find(|tab| tab.active).and_then(|tab| tab.switch);
    let form = choose(row.width, &team, tabs, look, switch);
    // The buttons first, from the right: the tabs end before them.
    let (placed, limit) = put_buttons(canvas, row, form.shown, switch, look, hover);
    let mut zones = Vec::new();
    let mut x = row.x;
    if form.with_team {
        x = canvas.put_in(x, row.y, limit, &team, Style::PLAIN.bold());
    }
    let tabs_row = Tabs { tabs, form, look, hover, y: row.y, limit };
    for i in 0..tabs.len() {
        x = canvas.put_in(x, row.y, limit, "│", Style::fg(Color::DarkGrey));
        let start = x;
        x = tabs_row.put_tab(canvas, x, i);
        if x > start {
            zones.push((Rect { x: start, y: row.y, width: x - start, height: 1 }, Target::Tab(i)));
        }
    }
    canvas.put_in(x, row.y, limit, "│", Style::fg(Color::DarkGrey));
    zones.extend(placed.into_iter().rev());
    zones
}

/// How the bar is laid out when short of room.
struct Form {
    with_team: bool,
    labels: Labels,
    shown: Buttons,
    /// No blanks around the tabs (« │⠴1│⚑2│ »).
    packed: bool,
}

/// The least a tab says: its number, and ⤢ after it for the current one zoomed (« 1 ⤢ », packed « 1⤢ »).
fn least(i: usize, tab: &Tab<'_>, packed: bool, glyphs: Glyphs) -> String {
    match tab.zoomed.filter(|_| tab.active) {
        Some(_) if packed => format!("{}{}", i + 1, zoom_sign(Zoom::In, glyphs)),
        Some(_) => format!("{} {}", i + 1, zoom_sign(Zoom::In, glyphs)),
        None => (i + 1).to_string(),
    }
}

/// The forms from the richest, and the first that leaves a blank before the buttons in `width` columns: the other
/// tabs to their numbers, the team gone, the buttons to their keys (the current title then cut down to its number),
/// the blanks gone, the buttons gone. The tabs' signs never go.
fn choose(width: usize, team: &str, tabs: &[Tab<'_>], look: Look, switch: Option<Switch>) -> Form {
    // Each tab: a rule, then « ⠹ label » between blanks, or « ⠹1 » packed; a rule after the last.
    let tabs_width = |labels: Labels, packed: bool, active_cut: bool| -> usize {
        let each = |(i, tab): (usize, &Tab<'_>)| match (packed, tab.active && active_cut) {
            (true, _) => 2 + columns(&least(i, tab, true, look.glyphs)),
            (false, true) => 5 + columns(&least(i, tab, false, look.glyphs)),
            (false, false) => 5 + columns(&label(i, tab, labels, look.glyphs)),
        };
        tabs.iter().enumerate().map(each).sum::<usize>() + 1
    };
    let buttons_width = |shown: Buttons| -> usize {
        buttons(shown, look.glyphs, switch).iter().map(|(text, _)| columns(text) + 1).sum::<usize>()
    };
    let forms = [
        (true, Labels::Whole, Buttons::Long, false, false),
        (true, Labels::Numbers, Buttons::Long, false, false),
        (false, Labels::Numbers, Buttons::Long, false, false),
        (false, Labels::Numbers, Buttons::Keys, false, true),
        (false, Labels::Numbers, Buttons::Keys, true, true),
    ];
    forms
        .into_iter()
        .find(|&(with_team, labels, shown, packed, active_cut)| {
            let team = if with_team { columns(team) } else { 0 };
            team + tabs_width(labels, packed, active_cut) + 1 + buttons_width(shown) <= width
        })
        .map_or(Form { with_team: false, labels: Labels::Numbers, shown: Buttons::None, packed: true }, |form| Form {
            with_team: form.0,
            labels: form.1,
            shown: form.2,
            packed: form.3,
        })
}

/// The buttons on the right of `row`, from a column before its edge; where each can be clicked, and the column the
/// tabs must end before.
fn put_buttons(
    canvas: &mut Canvas,
    row: Rect,
    shown: Buttons,
    switch: Option<Switch>,
    look: Look,
    hover: Option<Target>,
) -> (Vec<(Rect, Target)>, usize) {
    let end = row.x + row.width;
    let mut right = end.saturating_sub(1);
    let mut placed = Vec::new();
    for (text, target) in buttons(shown, look.glyphs, switch).into_iter().rev() {
        let start = right.saturating_sub(columns(&text)).max(row.x);
        // Hovered, brighter: bold.
        let style = if hover == Some(target) { Style::PLAIN.reverse().bold() } else { Style::PLAIN.reverse() };
        canvas.put_in(start, row.y, right, &text, style);
        placed.push((Rect { x: start, y: row.y, width: right - start, height: 1 }, target));
        right = start.saturating_sub(1);
    }
    let limit = if placed.is_empty() { end } else { right.max(row.x) };
    (placed, limit)
}

/// What drawing the tabs takes: the tabs, how the bar is laid out, and the row they go on, cut before `limit`.
struct Tabs<'a, 'b> {
    tabs: &'a [Tab<'b>],
    form: Form,
    look: Look,
    hover: Option<Target>,
    y: usize,
    limit: usize,
}

impl Tabs<'_, '_> {
    /// Tab `i` from column `x`: its sign, its label; the column after it.
    fn put_tab(&self, canvas: &mut Canvas, mut x: usize, i: usize) -> usize {
        let Tabs { tabs, look, hover, y, limit, .. } = *self;
        let tab = &tabs[i];
        let (labels, packed) = (self.form.labels, self.form.packed);
        let mut text = if packed { least(i, tab, true, look.glyphs) } else { label(i, tab, labels, look.glyphs) };
        if tab.active && !packed {
            // What the other tabs take after the current one, for its title to leave them their room.
            let after_active: usize = tabs[i + 1..]
                .iter()
                .enumerate()
                .map(|(k, tab)| 1 + 4 + columns(&label(i + 1 + k, tab, labels, look.glyphs)))
                .sum::<usize>()
                + 1;
            let room = limit.saturating_sub(x + 4 + after_active);
            // Zoomed, who shows says more than the tab's title: the title goes first.
            if let Some(member) = tab.zoomed.filter(|_| columns(&text) > room) {
                text = format!("{} {} {member}", i + 1, zoom_sign(Zoom::In, look.glyphs));
            }
            // Down to its number (and ⤢ zoomed), never less: « 1 … » would say nothing more.
            let least = least(i, tab, false, look.glyphs);
            text = if room < columns(&least) + 3 { least } else { fit(&text, room) };
        }
        let sign = tab.state.icon(look.glyphs, look.frame).to_string();
        let sign = sign.as_str();
        // Hovered, its label underlined as what a click reaches; another than the current one brighter too: no longer
        // dimmed, bold.
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
            x = canvas.put_in(x, y, limit, " ", base);
        }
        x = canvas.put_in(x, y, limit, sign, sign_style);
        if !packed {
            x = canvas.put_in(x, y, limit, " ", blank);
        }
        x = canvas.put_in(x, y, limit, &text, text_style);
        if !packed {
            x = canvas.put_in(x, y, limit, " ", blank);
        }
        x
    }
}
