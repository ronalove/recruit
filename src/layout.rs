// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Tabs: the members the user talks to first, then the working agents, in tabs of `columns` × `rows` at most (six by default); how a tab is cut
//! into a grid; and, for a running team whose members changed, how its panes move to their new tabs.

use crate::config::{Layout, Team};
use crate::i18n::{self, Lang};
use crate::mux::Rect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub title: String,
    /// Member names, in reading order: left to right, then top to bottom.
    pub members: Vec<String>,
}

/// Most columns in a tab: 3 by default.
pub fn columns(team: &Team) -> usize {
    team.columns.unwrap_or(3).max(1)
}

/// Most panes in a tab: 3 columns × 2 rows by default.
fn per_tab(team: &Team) -> usize {
    columns(team) * team.rows.unwrap_or(2).max(1)
}

pub fn tabs(team: &Team) -> Vec<Tab> {
    let contacts = team.contacts();
    let workers: Vec<&String> = team.members.keys().filter(|n| !contacts.contains(&n.as_str())).collect();
    if team.layout == Some(Layout::Tabs) {
        return contacts.into_iter().chain(workers.iter().map(|n| n.as_str())).map(own_tab).collect();
    }

    let (contacts_title, agents_title) = match team.lang.unwrap_or_else(i18n::lang) {
        Lang::Fr => ("Interlocuteurs", "Agents"),
        Lang::En => ("Contacts", "Agents"),
    };
    let mut tabs = split(contacts_title, contacts.into_iter().map(String::from).collect(), per_tab(team));

    // Working agents grouped by their `tab`, in order of first appearance; those without one under "Agents".
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for name in workers {
        let title = team.members[name].tab.clone().unwrap_or_else(|| agents_title.to_string());
        match groups.iter_mut().find(|(t, _)| *t == title) {
            Some((_, members)) => members.push(name.clone()),
            None => groups.push((title, vec![name.clone()])),
        }
    }
    for (title, members) in groups {
        tabs.extend(split(&title, members, per_tab(team)));
    }
    tabs
}

/// The titles of the tabs recruit makes that a member's `tab` cannot take, in both languages: "Interlocuteurs",
/// "Contacts", "Agents (2)"… They would make two windows of one title. "Agents" alone is the default group, as with no
/// `tab`: it goes with the working agents that have none.
pub fn reserved_title(title: &str) -> bool {
    let title = title.trim();
    let numbered = title
        .strip_prefix("Agents (")
        .and_then(|rest| rest.strip_suffix(')'))
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    numbered || ["Interlocuteurs", "Contacts"].contains(&title)
}

fn own_tab(name: &str) -> Tab {
    Tab { title: name.to_string(), members: vec![name.to_string()] }
}

/// `members` over as few tabs of at most `max` as needed, as even as possible (8 → 4 + 4, 7 → 4 + 3), titled
/// "Agents", or "Agents (1)", "Agents (2)"… when there are several.
fn split(title: &str, members: Vec<String>, max: usize) -> Vec<Tab> {
    if members.is_empty() {
        return Vec::new();
    }
    let count = members.len().div_ceil(max.max(1));
    let (base, extra) = (members.len() / count, members.len() % count);
    let mut members = members.into_iter();
    (0..count)
        .map(|i| Tab {
            title: if count == 1 { title.to_string() } else { format!("{title} ({})", i + 1) },
            members: members.by_ref().take(base + usize::from(i < extra)).collect(),
        })
        .collect()
}

/// The grid of a tab of `count` panes with at most `max_columns` columns: as few rows as possible, then as few
/// columns as these rows need (4 panes make 2 × 2, 5 make 3 + 2). Returns the number of panes in each column;
/// pane `i` goes in column `i % columns`, reading order.
pub fn column_heights(count: usize, max_columns: usize) -> Vec<usize> {
    let rows = count.div_ceil(max_columns.max(1)).max(1);
    let cols = count.div_ceil(rows).max(1);
    let mut heights = vec![0; cols];
    for i in 0..count {
        heights[i % cols] += 1;
    }
    heights
}

/// The panels' column of the first tab, by pane ids: the dashboard, and the journal under it with its height when
/// it is reduced (half the column otherwise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidePanes {
    pub dashboard: Option<String>,
    pub journal: Option<(String, Option<usize>)>,
}

/// Width of the panels' column, as a share of the window.
pub const SIDE_PERCENT: usize = 35;

/// Where each pane of a tab goes in the multiplexer, frame included: a grid of at most `columns` columns, the frames
/// side by side with no line between them (direction « Cadres », CLAUDE.md), in a `width` × `height` area from its
/// top left corner. Returns the panes (ids, as given) with their rectangles.
pub(crate) fn rects(
    width: usize,
    height: usize,
    panes: &[String],
    columns: usize,
    side: Option<&SidePanes>,
) -> Vec<(String, Rect)> {
    let mut out = Vec::new();
    tree(width, height, panes, columns, side, 0).place(Rect { x: 0, y: 0, width, height }, 0, &mut out);
    out
}

fn tree(width: usize, height: usize, panes: &[String], columns: usize, side: Option<&SidePanes>, gap: usize) -> Node {
    let side_panes: Vec<(String, Option<usize>)> = side
        .map(|s| {
            s.dashboard
                .iter()
                .map(|d| (d.clone(), None))
                .chain(s.journal.iter().map(|(j, r)| (j.clone(), *r)))
                .collect()
        })
        .unwrap_or_default();
    let columns = if side_panes.is_empty() { columns } else { 1 };
    let heights = column_heights(panes.len(), columns);
    let cols = heights.len();
    let grid: Vec<Vec<String>> =
        (0..cols).map(|c| (0..heights[c]).map(|r| panes[r * cols + c].clone()).collect()).collect();
    let grid_node = |w: usize, h: usize| -> Node {
        let column = |members: &[String]| -> Node {
            let sizes = shares(h, members.len(), gap);
            Node::split(false, members.iter().zip(sizes).map(|(m, s)| (s, Node::Pane(m.clone()))).collect())
        };
        let sizes = shares(w, cols, gap);
        Node::split(true, grid.iter().zip(sizes).map(|(members, s)| (s, column(members))).collect())
    };
    if side_panes.is_empty() {
        return grid_node(width, height);
    }
    let side_width = width * SIDE_PERCENT / 100;
    let left = width.saturating_sub(side_width + gap);
    let column = match side_panes.as_slice() {
        [(dashboard, _), (journal, rows)] => {
            let journal_rows = rows.unwrap_or(height / 2).min(height.saturating_sub(2));
            let dashboard_rows = height.saturating_sub(journal_rows + gap);
            Node::split(
                false,
                vec![(dashboard_rows, Node::Pane(dashboard.clone())), (journal_rows, Node::Pane(journal.clone()))],
            )
        }
        [(only, _)] => Node::Pane(only.clone()),
        _ => unreachable!("one or two panels"),
    };
    Node::split(true, vec![(left, grid_node(left, height)), (side_width, column)])
}

/// Narrower than this, the cards of a tab in « focus » go two by two, in their short form (maquette).
pub(crate) const FOCUS_WIDE: usize = 120;
/// Shorter than this, a tab in « focus » shows two panes on the right instead of three.
const FOCUS_TALL: usize = 36;
/// Fewest columns of a tab in « focus »; narrower, it keeps its grid.
const FOCUS_MIN_WIDTH: usize = 80;
/// Fewest rows of a tab in « focus » (two panes of three rows and the cards' frame); shorter, it keeps its grid.
const FOCUS_MIN_HEIGHT: usize = 14;
/// The fewest rows of a pane on the right.
const FOCUS_PANE_ROWS: usize = 3;

/// How many panes the right column of a tab in « focus » shows in a `width` × `height` area (decision of
/// 2026-10-11): three, two under 120 × 36; None under 80 columns, where the tab keeps its grid (and under 14 rows,
/// where the right column has no room for two panes and the cards).
pub(crate) fn focus_room(width: usize, height: usize) -> Option<usize> {
    if width < FOCUS_MIN_WIDTH || height < FOCUS_MIN_HEIGHT {
        None
    } else if width < FOCUS_WIDE || height < FOCUS_TALL {
        Some(2)
    } else {
        Some(3)
    }
}

/// The rows the frame of `cards` cards takes in a `width` columns area, as on the maquette: a card is 4 rows, with its
/// line of text and the gap under it; two by two, in the short form, 3 rows a row of cards; the frame's two borders.
pub(crate) fn focus_cards_rows(width: usize, cards: usize) -> usize {
    match cards {
        0 => 0,
        n if width < FOCUS_WIDE => 1 + 3 * n.div_ceil(2),
        n => 1 + 4 * n,
    }
}

/// Where the panes of a tab in « focus » go, frames included, in a `width` × `height` area from its top left corner
/// (maquette https://claude.ai/artifact/H4tpNaZ2RAnxHErZHyztYe, direction B): `left` on the left, as the contacts
/// beside the panels; `right` stacked on the right, top to bottom; under them, when there are `cards` cards of the
/// members left out, the area of their frame ([`focus_cards_rows`] rows, fewer when the panes need the room),
/// returned apart. The widths follow `SIDE_PERCENT`. With nothing on the right, the cards' frame takes the whole
/// column; with no cards either, the left pane takes the whole width.
pub(crate) fn focus_rects(
    width: usize,
    height: usize,
    left: &str,
    right: &[String],
    cards: usize,
) -> (Vec<(String, Rect)>, Option<Rect>) {
    let side = if right.is_empty() && cards == 0 { 0 } else { width * SIDE_PERCENT / 100 };
    let main = width - side;
    let mut out = vec![(left.to_string(), Rect { x: 0, y: 0, width: main, height })];
    if side == 0 {
        return (out, None);
    }
    let reserved = FOCUS_PANE_ROWS * right.len();
    let frame =
        if right.is_empty() { height } else { focus_cards_rows(width, cards).min(height.saturating_sub(reserved)) };
    let frame = if cards > 0 && frame >= 3 { frame } else { 0 };
    let panes = height - frame;
    if !right.is_empty() {
        let mut y = 0;
        for (id, rows) in right.iter().zip(shares(panes, right.len(), 0)) {
            out.push((id.clone(), Rect { x: main, y, width: side, height: rows }));
            y += rows;
        }
    }
    let cards = (frame > 0).then_some(Rect { x: main, y: panes, width: side, height: frame });
    (out, cards)
}

/// `count` cells sharing `total` columns or rows, `gap` between each two: each cell its share of what is left, the
/// first ones rounded down; without a gap, even shares.
fn shares(total: usize, count: usize, gap: usize) -> Vec<usize> {
    let count = count.max(1);
    if gap == 0 {
        return (0..count).map(|i| (i + 1) * total / count - i * total / count).collect();
    }
    let mut sizes = Vec::new();
    let mut rest = total;
    for k in 1..count {
        let percent = 100 * (count - k) / (count - k + 1);
        let split = rest * percent / 100;
        sizes.push(rest.saturating_sub(split + gap));
        rest = split;
    }
    sizes.push(rest);
    sizes
}

/// A cell of a tab's layout.
enum Node {
    Pane(String),
    /// Side by side when `across`, stacked otherwise; each child with its width or height.
    Split {
        across: bool,
        children: Vec<(usize, Node)>,
    },
}

impl Node {
    fn split(across: bool, mut children: Vec<(usize, Node)>) -> Node {
        if children.len() == 1 { children.remove(0).1 } else { Node::Split { across, children } }
    }

    /// The panes' rectangles in `area`, `gap` cells between two.
    fn place(&self, area: Rect, gap: usize, out: &mut Vec<(String, Rect)>) {
        match self {
            Node::Pane(id) => out.push((id.clone(), area)),
            Node::Split { across, children } => {
                let mut at = if *across { area.x } else { area.y };
                for (size, child) in children {
                    let cell = if *across {
                        Rect { x: at, width: *size, ..area }
                    } else {
                        Rect { y: at, height: *size, ..area }
                    };
                    child.place(cell, gap, out);
                    at += size + gap;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("%{i}")).collect()
    }

    #[test]
    fn native_rects_tile_the_area() {
        // Five members, three columns at most: 3 + 2, no line between the frames.
        let rects = rects(120, 40, &ids(5), 3, None);
        let at = |id: &str| rects.iter().find(|(i, _)| i == id).unwrap().1;
        assert_eq!(at("%0"), Rect { x: 0, y: 0, width: 40, height: 20 });
        assert_eq!(at("%3"), Rect { x: 0, y: 20, width: 40, height: 20 });
        assert_eq!(at("%2"), Rect { x: 80, y: 0, width: 40, height: 40 });
        let area: usize = rects.iter().map(|(_, r)| r.width * r.height).sum();
        assert_eq!(area, 120 * 40, "every cell has its pane, once");
        // Beside the panels: the members in one column, the dashboard over the journal on the right.
        let side = SidePanes { dashboard: Some("%d".into()), journal: Some(("%j".into(), Some(7))) };
        let rects = super::rects(100, 30, &ids(2), 3, Some(&side));
        let at = |id: &str| rects.iter().find(|(i, _)| i == id).unwrap().1;
        assert_eq!(at("%0"), Rect { x: 0, y: 0, width: 65, height: 15 });
        assert_eq!(at("%d"), Rect { x: 65, y: 0, width: 35, height: 23 });
        assert_eq!(at("%j"), Rect { x: 65, y: 23, width: 35, height: 7 });
    }
    use crate::config::Member;

    /// Members as (name, contact, tab).
    fn team(members: &[(&str, bool, Option<&str>)]) -> Team {
        let mut team = Team { lang: Some(Lang::Fr), ..Default::default() };
        for (name, contact, tab) in members {
            team.members.insert(
                name.to_string(),
                Member { role: "r".into(), contact: *contact, tab: tab.map(String::from), ..Default::default() },
            );
        }
        team
    }

    fn shape(tabs: &[Tab]) -> Vec<(String, usize)> {
        tabs.iter().map(|t| (t.title.clone(), t.members.len())).collect()
    }

    #[test]
    fn small_team() {
        let t = team(&[("coordinateur", true, None), ("développeur", false, None), ("designer", false, None)]);
        let tabs = tabs(&t);
        assert_eq!(tabs[0], Tab { title: "Interlocuteurs".into(), members: vec!["coordinateur".into()] });
        assert_eq!(tabs[1], Tab { title: "Agents".into(), members: vec!["développeur".into(), "designer".into()] });
        assert_eq!(tabs.len(), 2);
    }

    #[test]
    fn first_member_is_the_contact_by_default() {
        let t = team(&[("a", false, None), ("b", false, None)]);
        assert_eq!(shape(&tabs(&t)), [("Interlocuteurs".into(), 1), ("Agents".into(), 1)]);
        assert_eq!(tabs(&t)[0].members, ["a"]);
    }

    #[test]
    fn working_agents_split_evenly() {
        let mut members = vec![("lead".to_string(), true)];
        members.extend((1..=8).map(|i| (format!("w{i}"), false)));
        let list: Vec<(&str, bool, Option<&str>)> = members.iter().map(|(n, c)| (n.as_str(), *c, None)).collect();
        let contacts = ("Interlocuteurs".to_string(), 1);
        assert_eq!(shape(&tabs(&team(&list))), [contacts.clone(), ("Agents (1)".into(), 4), ("Agents (2)".into(), 4)]);
        assert_eq!(
            shape(&tabs(&team(&list[..8]))),
            [contacts.clone(), ("Agents (1)".into(), 4), ("Agents (2)".into(), 3)]
        );
        assert_eq!(shape(&tabs(&team(&list[..7]))), [contacts, ("Agents".into(), 6)]);
    }

    /// The examples the user gave (2026-10-06): contacts, working agents → panes per tab.
    #[test]
    fn user_examples() {
        for (contacts, agents, expected) in [
            (1, 2, vec![1, 2]),
            (2, 4, vec![2, 4]),
            (4, 6, vec![4, 6]),
            (3, 7, vec![3, 4, 3]),
            (1, 15, vec![1, 5, 5, 5]),
            (5, 1, vec![5, 1]),
        ] {
            let names: Vec<(String, bool)> = (0..contacts)
                .map(|i| (format!("i{i}"), true))
                .chain((0..agents).map(|i| (format!("a{i}"), false)))
                .collect();
            let list: Vec<(&str, bool, Option<&str>)> = names.iter().map(|(n, c)| (n.as_str(), *c, None)).collect();
            let sizes: Vec<usize> = tabs(&team(&list)).iter().map(|t| t.members.len()).collect();
            assert_eq!(sizes, expected, "{contacts} contacts, {agents} agents");
        }
    }

    #[test]
    fn explicit_tabs_and_several_contacts() {
        let t = team(&[
            ("coordinateur", true, Some("ignoré")),
            ("planificateur", false, Some("Coordination")),
            ("dev", false, Some("Code")),
            ("reviewer", false, Some("Coordination")),
            ("seul", false, None),
            ("opérateur", true, None),
        ]);
        let tabs = tabs(&t);
        assert_eq!(
            shape(&tabs),
            [("Interlocuteurs".into(), 2), ("Coordination".into(), 2), ("Code".into(), 1), ("Agents".into(), 1)]
        );
        assert_eq!(tabs[0].members, ["coordinateur", "opérateur"]);
        assert_eq!(tabs[1].members, ["planificateur", "reviewer"]);

        let mut each = t;
        each.layout = Some(Layout::Tabs);
        let titles: Vec<String> = super::tabs(&each).into_iter().map(|t| t.title).collect();
        assert_eq!(titles, ["coordinateur", "opérateur", "planificateur", "dev", "reviewer", "seul"]);
    }

    #[test]
    fn english_titles() {
        let mut t = team(&[("lead", true, None), ("dev", false, None)]);
        t.lang = Some(Lang::En);
        assert_eq!(shape(&tabs(&t)), [("Contacts".into(), 1), ("Agents".into(), 1)]);
    }

    /// One contact, then `workers` working agents w1, w2…
    fn crew(workers: usize) -> Team {
        let names: Vec<String> = (1..=workers).map(|i| format!("w{i}")).collect();
        let mut list = vec![("lead", true, None)];
        list.extend(names.iter().map(|n| (n.as_str(), false, None)));
        team(&list)
    }

    #[test]
    fn six_agents_then_seven_then_six() {
        // A seventh agent: two tabs of 4 and 3; back to six, one tab.
        assert_eq!(shape(&tabs(&crew(7)))[1..], [("Agents (1)".into(), 4), ("Agents (2)".into(), 3)]);
        assert_eq!(shape(&tabs(&crew(6)))[1..], [("Agents".into(), 6)]);
    }

    #[test]
    fn twelve_agents_then_thirteen() {
        assert_eq!(tabs(&crew(13)).iter().map(|t| t.members.len()).collect::<Vec<_>>(), [1, 5, 4, 4]);
    }

    #[test]
    fn grid_shapes() {
        assert_eq!(column_heights(1, 3), [1]);
        assert_eq!(column_heights(2, 3), [1, 1]);
        assert_eq!(column_heights(3, 3), [1, 1, 1]);
        assert_eq!(column_heights(4, 3), [2, 2]);
        assert_eq!(column_heights(5, 3), [2, 2, 1]);
        assert_eq!(column_heights(6, 3), [2, 2, 2]);
        assert_eq!(column_heights(7, 3), [3, 2, 2]);
        assert_eq!(column_heights(4, 2), [2, 2]);
    }

    #[test]
    fn the_room_on_the_right_follows_the_window() {
        let table = [
            ((140, 40), Some(3)),
            ((120, 36), Some(3)),
            ((119, 40), Some(2)),
            ((140, 35), Some(2)),
            ((100, 31), Some(2)),
            ((80, 24), Some(2)),
            ((79, 40), None),
            ((140, 13), None),
        ];
        for ((width, height), room) in table {
            assert_eq!(focus_room(width, height), room, "{width} × {height}");
        }
    }

    #[test]
    fn focus_rects_tile_the_area() {
        // 140 × 39 (the maquette's tab without its bar): three panes on the right, two cards of 4 rows and the frame.
        let right = ids(3);
        let (rects, cards) = focus_rects(140, 39, "left", &right, 2);
        let at = |id: &str| rects.iter().find(|(i, _)| i == id).unwrap().1;
        assert_eq!(at("left"), Rect { x: 0, y: 0, width: 91, height: 39 });
        let frame = cards.unwrap();
        assert_eq!(frame, Rect { x: 91, y: 30, width: 49, height: 9 });
        // The panes share what the frame leaves, in order, from the top, each as wide as the column.
        let heights: Vec<_> = right.iter().map(|id| at(id)).collect();
        assert_eq!(heights.iter().map(|r| r.height).sum::<usize>(), 30);
        assert_eq!(heights[0].y, 0);
        for pair in heights.windows(2) {
            assert_eq!(pair[1].y, pair[0].y + pair[0].height);
        }
        assert!(heights.iter().all(|r| r.x == 91 && r.width == 49));
        assert_eq!(rects.len(), 4);
    }

    #[test]
    fn the_cards_go_two_by_two_in_a_narrow_window() {
        assert_eq!(focus_cards_rows(140, 2), 9);
        assert_eq!(focus_cards_rows(119, 3), 7);
        assert_eq!(focus_cards_rows(100, 2), 4);
        assert_eq!(focus_cards_rows(140, 0), 0);
        let (_, cards) = focus_rects(100, 30, "l", &ids(2), 3);
        assert_eq!(cards, Some(Rect { x: 65, y: 23, width: 35, height: 7 }));
    }

    #[test]
    fn the_panes_keep_their_rows_before_the_cards() {
        // Six cards would take 25 rows: the frame gives way to two panes of three rows at least.
        let (rects, cards) = focus_rects(140, 20, "l", &ids(2), 6);
        let frame = cards.unwrap();
        assert_eq!(frame.height, 14);
        assert!(rects.iter().skip(1).all(|(_, r)| r.height == 3));
        // No room for even that: no frame.
        let (_, cards) = focus_rects(140, 8, "l", &ids(2), 6);
        assert_eq!(cards, None);
    }

    #[test]
    fn nothing_on_the_right() {
        let (rects, cards) = focus_rects(140, 30, "l", &[], 0);
        assert_eq!(rects, [("l".to_string(), Rect { x: 0, y: 0, width: 140, height: 30 })]);
        assert_eq!(cards, None);
        // Only cards: they have the column.
        let (rects, cards) = focus_rects(140, 30, "l", &[], 2);
        assert_eq!(rects.len(), 1);
        assert_eq!(cards, Some(Rect { x: 91, y: 0, width: 49, height: 30 }));
        // No cards: the panes have the column.
        let (rects, cards) = focus_rects(140, 30, "l", &ids(2), 0);
        assert_eq!(cards, None);
        assert_eq!(rects[1].1.height + rects[2].1.height, 30);
    }
}
