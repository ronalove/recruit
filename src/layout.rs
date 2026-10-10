// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Tabs: the members the user talks to first, then the working agents, in tabs of six at most; how a tab is cut
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
}
