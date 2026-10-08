// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Tabs: the members the user talks to first, then the working agents, in tabs of six at most; how a tab is cut
//! into a grid; and, for a running team whose members changed, how its panes move to their new tabs.

use crate::config::{Layout, Team};
use crate::i18n::{self, Lang};

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

/// A window of a running team, as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: String,
    pub title: String,
    /// The members whose panes it holds.
    pub members: Vec<String>,
    /// It holds the dashboard or the journal: the first tab's.
    pub panels: bool,
}

/// The window that takes a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    Window(String),
    /// A new one, made of this member's pane, taken out of its window.
    Break(String),
}

/// How the panes of a running team go to their new tabs, none of them closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moves {
    /// The window of each tab, in order.
    pub hosts: Vec<Host>,
    /// The panes that go to another window: the member and its tab.
    pub joins: Vec<(String, usize)>,
}

/// From `current` to `tabs`: each tab takes a window that holds one of its members and that no tab before took,
/// the one with its title first, then the one with most of its members; the first tab, the panels' window. A tab
/// whose members all sit in windows already taken gets a new window. A window left with no pane closes on its own.
/// Every member of `tabs` has a pane in `current`.
pub fn moves(current: &[Window], tabs: &[Tab]) -> Moves {
    let mut taken: Vec<usize> = Vec::new();
    let mut hosts = Vec::new();
    for (i, tab) in tabs.iter().enumerate() {
        let mine = |w: &Window| w.members.iter().filter(|m| tab.members.contains(m)).count();
        let panels = if i == 0 { current.iter().position(|w| w.panels) } else { None };
        let host = panels.or_else(|| {
            current
                .iter()
                .enumerate()
                .filter(|&(j, w)| !taken.contains(&j) && !w.panels && mine(w) > 0)
                .max_by_key(|&(j, w)| (w.title == tab.title, mine(w), std::cmp::Reverse(j)))
                .map(|(j, _)| j)
        });
        match host {
            Some(j) => {
                taken.push(j);
                hosts.push(Host::Window(current[j].id.clone()));
            }
            None => hosts.push(Host::Break(tab.members[0].clone())),
        }
    }
    let window_of = |member: &str| current.iter().find(|w| w.members.iter().any(|m| m == member)).map(|w| &w.id);
    let mut joins = Vec::new();
    for (i, tab) in tabs.iter().enumerate() {
        for member in &tab.members {
            let home = match &hosts[i] {
                Host::Window(id) => window_of(member) == Some(id),
                Host::Break(first) => first == member,
            };
            if !home {
                joins.push((member.clone(), i));
            }
        }
    }
    Moves { hosts, joins }
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

/// The layout tmux gives a tab as `launch` builds it, for `select-layout`: the members' panes (ids, reading order) in
/// a grid of at most `columns` columns, or in one column beside the panels'. Returns the layout and the panes in the
/// order tmux assigns them to its cells, which the window's panes must follow.
pub fn tmux_layout(
    width: usize,
    height: usize,
    panes: &[String],
    columns: usize,
    side: Option<&SidePanes>,
) -> (String, Vec<String>) {
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
            let sizes = shares(h, members.len());
            Node::split(false, members.iter().zip(sizes).map(|(m, s)| (s, Node::Pane(m.clone()))).collect())
        };
        let sizes = shares(w, cols);
        Node::split(true, grid.iter().zip(sizes).map(|(members, s)| (s, column(members))).collect())
    };
    let root = if side_panes.is_empty() {
        grid_node(width, height)
    } else {
        let side_width = width * SIDE_PERCENT / 100;
        let left = width.saturating_sub(side_width + 1);
        let column = match side_panes.as_slice() {
            [(dashboard, _), (journal, rows)] => {
                let journal_rows = rows.unwrap_or(height / 2).min(height.saturating_sub(2));
                let dashboard_rows = height.saturating_sub(journal_rows + 1);
                Node::split(
                    false,
                    vec![(dashboard_rows, Node::Pane(dashboard.clone())), (journal_rows, Node::Pane(journal.clone()))],
                )
            }
            [(only, _)] => Node::Pane(only.clone()),
            _ => unreachable!("one or two panels"),
        };
        Node::split(true, vec![(left, grid_node(left, height)), (side_width, column)])
    };
    let mut body = String::new();
    let mut order = Vec::new();
    root.render(width, height, 0, 0, &mut body, &mut order);
    (format!("{:04x},{body}", checksum(&body)), order)
}

/// `count` cells sharing `total` columns or rows, one between each two for the border, as `launch` splits them:
/// each split gives the new pane its share of what is left (`-l 66%`, then `-l 50%`…), tmux rounding down.
fn shares(total: usize, count: usize) -> Vec<usize> {
    let mut sizes = Vec::new();
    let mut rest = total;
    for k in 1..count.max(1) {
        let percent = 100 * (count - k) / (count - k + 1);
        let split = rest * percent / 100;
        sizes.push(rest.saturating_sub(split + 1));
        rest = split;
    }
    sizes.push(rest);
    sizes
}

/// A cell of a tmux layout.
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

    fn render(&self, width: usize, height: usize, x: usize, y: usize, out: &mut String, order: &mut Vec<String>) {
        use std::fmt::Write;
        let _ = write!(out, "{width}x{height},{x},{y}");
        match self {
            Node::Pane(id) => {
                let _ = write!(out, ",{}", id.trim_start_matches('%'));
                order.push(id.clone());
            }
            Node::Split { across, children } => {
                out.push(if *across { '{' } else { '[' });
                let mut at = if *across { x } else { y };
                for (i, (size, child)) in children.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    let (w, h, cx, cy) = if *across { (*size, height, at, y) } else { (width, *size, x, at) };
                    child.render(w, h, cx, cy, out, order);
                    at += size + 1;
                }
                out.push(if *across { '}' } else { ']' });
            }
        }
    }
}

/// tmux's checksum of a layout (`layout_checksum`).
fn checksum(layout: &str) -> u16 {
    layout.bytes().fold(0u16, |sum, b| ((sum >> 1) | ((sum & 1) << 15)).wrapping_add(u16::from(b)))
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// The windows of a team launched as `tabs`, the first with the panels, then a window per pane in `extra`.
    fn windows(tabs: &[Tab], extra: &[&str]) -> Vec<Window> {
        let launched = tabs.iter().enumerate().map(|(i, t)| Window {
            id: format!("@{i}"),
            title: t.title.clone(),
            members: t.members.clone(),
            panels: i == 0,
        });
        let added = extra.iter().enumerate().map(|(i, m)| Window {
            id: format!("@n{i}"),
            title: m.to_string(),
            members: vec![m.to_string()],
            panels: false,
        });
        launched.chain(added).collect()
    }

    fn joins(list: &[(&str, usize)]) -> Vec<(String, usize)> {
        list.iter().map(|(m, i)| (m.to_string(), *i)).collect()
    }

    fn hosts(list: &[&str]) -> Vec<Host> {
        list.iter().map(|h| Host::Window(h.to_string())).collect()
    }

    #[test]
    fn six_agents_then_seven_then_six() {
        // A seventh agent: two tabs of 4 and 3, the new pane's window takes the second.
        let current = windows(&tabs(&crew(6)), &["w7"]);
        let seven = tabs(&crew(7));
        assert_eq!(shape(&seven)[1..], [("Agents (1)".into(), 4), ("Agents (2)".into(), 3)]);
        let moves = super::moves(&current, &seven);
        assert_eq!(moves.hosts, hosts(&["@0", "@1", "@n0"]));
        assert_eq!(moves.joins, joins(&[("w5", 2), ("w6", 2)]));

        // Back to six: the second tab's panes join the first, its window closes on its own.
        let mut current = windows(&seven, &[]);
        current[2].members.retain(|m| m != "w7");
        let moves = super::moves(&current, &tabs(&crew(6)));
        assert_eq!(moves.hosts, hosts(&["@0", "@1"]));
        assert_eq!(moves.joins, joins(&[("w5", 1), ("w6", 1)]));
    }

    #[test]
    fn twelve_agents_then_thirteen() {
        let current = windows(&tabs(&crew(12)), &["w13"]);
        let thirteen = tabs(&crew(13));
        assert_eq!(thirteen.iter().map(|t| t.members.len()).collect::<Vec<_>>(), [1, 5, 4, 4]);
        let moves = super::moves(&current, &thirteen);
        assert_eq!(moves.hosts, hosts(&["@0", "@1", "@2", "@n0"]));
        assert_eq!(moves.joins, joins(&[("w6", 2), ("w10", 3), ("w11", 3), ("w12", 3)]));
    }

    #[test]
    fn nothing_moves_when_nothing_changed() {
        let launched = tabs(&crew(9));
        let moves = super::moves(&windows(&launched, &[]), &launched);
        assert_eq!(moves.hosts, hosts(&["@0", "@1", "@2"]));
        assert!(moves.joins.is_empty());
    }

    #[test]
    fn contacts_and_tabs_change() {
        // A working agent becomes a contact: its pane goes to the panels' window.
        let mut t = crew(3);
        let current = windows(&tabs(&t), &[]);
        t.members["w2"].contact = true;
        let moves = super::moves(&current, &tabs(&t));
        assert_eq!(moves.hosts, hosts(&["@0", "@1"]));
        assert_eq!(moves.joins, joins(&[("w2", 0)]));

        // Two agents get a tab of their own: their window is taken, a new one is made of the first.
        let mut t = crew(4);
        let current = windows(&tabs(&t), &[]);
        for name in ["w3", "w4"] {
            t.members[name].tab = Some("Code".into());
        }
        let moves = super::moves(&current, &tabs(&t));
        assert_eq!(moves.hosts, vec![Host::Window("@0".into()), Host::Window("@1".into()), Host::Break("w3".into())]);
        assert_eq!(moves.joins, joins(&[("w4", 2)]));

        // The tab is given back: the window titled "Agents" stays, "Code" empties.
        let current = windows(&tabs(&t), &[]);
        let moves = super::moves(&current, &tabs(&crew(4)));
        assert_eq!(moves.hosts, hosts(&["@0", "@1"]));
        assert_eq!(moves.joins, joins(&[("w3", 1), ("w4", 1)]));
    }

    #[test]
    fn layouts_as_tmux_writes_them() {
        let ids = |list: &[u32]| list.iter().map(|i| format!("%{i}")).collect::<Vec<_>>();
        // `#{window_layout}` of tabs split as `launch` splits them, 200 × 49, in tmux 3.7c.
        let (layout, order) = tmux_layout(200, 49, &ids(&[0, 1, 2, 3, 4]), 3, None);
        assert_eq!(
            layout,
            "ab4a,200x49,0,0{67x49,0,0[67x24,0,0,0,67x24,0,25,3],65x49,68,0[65x24,68,0,1,65x24,68,25,4],66x49,134,0,2}"
        );
        assert_eq!(order, ids(&[0, 3, 1, 4, 2]));
        let side = SidePanes { dashboard: Some("%6".into()), journal: Some(("%7".into(), Some(7))) };
        let (layout, order) = tmux_layout(200, 49, &ids(&[5, 8]), 3, Some(&side));
        assert_eq!(
            layout,
            "566a,200x49,0,0{129x49,0,0[129x24,0,0,5,129x24,0,25,8],70x49,130,0[70x41,130,0,6,70x7,130,42,7]}"
        );
        assert_eq!(order, ids(&[5, 8, 6, 7]));
        // One pane, the journal hidden, at half height.
        assert_eq!(tmux_layout(80, 24, &ids(&[9]), 3, None).0, format!("{:04x},80x24,0,0,9", checksum("80x24,0,0,9")));
        let dashboard = SidePanes { dashboard: Some("%6".into()), journal: None };
        assert!(tmux_layout(200, 49, &ids(&[5]), 3, Some(&dashboard)).0.ends_with("{129x49,0,0,5,70x49,130,0,6}"));
        let half = SidePanes { journal: Some(("%7".into(), None)), ..side };
        assert!(tmux_layout(200, 49, &ids(&[5]), 3, Some(&half)).0.ends_with("[70x24,130,0,6,70x24,130,25,7]}"));
        assert_eq!(shares(49, 3), [16, 15, 16]);
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
