// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Tabs: the members the user talks to first, then the working agents, in tabs of `columns` × `rows` at most (six by
//! default). How a tab is cut into a grid, or into the places of the « focus » layout, is the multiplexer's
//! (`mux::layout`).

use crate::config::{self, Layout, Team};
use crate::t;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tab {
    pub(crate) title: String,
    /// Member names, in reading order: left to right, then top to bottom.
    pub(crate) members: Vec<String>,
}

/// Most columns in a tab.
pub(crate) fn columns(team: &Team) -> usize {
    team.columns.unwrap_or(config::COLUMNS).max(1)
}

/// Most panes in a tab.
fn per_tab(team: &Team) -> usize {
    columns(team) * team.rows.unwrap_or(config::ROWS).max(1)
}

pub(crate) fn tabs(team: &Team) -> Vec<Tab> {
    let contacts = team.contacts();
    let workers: Vec<&String> = team.members.keys().filter(|n| !contacts.contains(&n.as_str())).collect();
    if team.layout == Some(Layout::Tabs) {
        return contacts.into_iter().chain(workers.iter().map(|n| n.as_str())).map(own_tab).collect();
    }

    let contacts_title = t!(in team.language(); "Interlocuteurs", "Contacts");
    let agents_title = config::AGENTS;
    let mut tabs = split(&contacts_title, contacts.into_iter().map(String::from).collect(), per_tab(team));

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Member;
    use crate::i18n::Lang;

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
}
