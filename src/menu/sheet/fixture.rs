// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A team for the tests, without files.

use crate::config::{self, Edit, Layered, Member, MemberLayers, Origin, Source, TeamLayers};

use super::*;

pub(crate) fn layered(
    value: Option<&str>,
    source: Source,
    fallback: Option<(Option<&str>, Source)>,
) -> Layered<Option<String>> {
    Layered { value: value.map(String::from), source, fallback: fallback.map(|(v, s)| (v.map(String::from), s)) }
}

fn unset() -> Layered<Option<String>> {
    layered(None, Source::Default, Some((None, Source::Default)))
}

pub(crate) fn layers(name: &str, contact: bool) -> MemberLayers {
    MemberLayers {
        role: Layered { value: format!("Rôle de {name}"), source: Source::Shared, fallback: None },
        instructions: layered(None, Source::Default, None),
        contact: Layered { value: contact, source: Source::Shared, fallback: Some((false, Source::Default)) },
        tab: unset(),
        permission_mode: unset(),
        // Opus from the team.
        model: layered(Some("opus"), Source::Team, Some((Some("opus"), Source::Team))),
        effort: unset(),
    }
}

pub(crate) fn team(names: &[(&str, bool)]) -> Team {
    let people = names
        .iter()
        .enumerate()
        .map(|(i, (name, contact))| Person {
            name: name.to_string(),
            contact: *contact,
            color: crate::ui::look::member_color(i),
            gone: false,
            model: Some("Opus".into()),
            effort: Some("high".into()),
            role: format!("Rôle de {name}"),
            layers: Some(layers(name, *contact)),
            tab_now: Some("Agents".into()),
            live: true,
        })
        .collect();
    let members = names
        .iter()
        .map(|(name, contact)| {
            (name.to_string(), Member { role: format!("Rôle de {name}"), contact: *contact, ..Default::default() })
        })
        .collect();
    Team {
        name: "essai".into(),
        origin: Origin::Local { root: "/p".into() },
        people,
        config: config::Team { members, ..Default::default() },
        dashboard: true,
        layers: Some(TeamLayers {
            dashboard: Layered { value: true, source: Source::Default, fallback: None },
            auto_compact: Layered { value: Some(70), source: Source::Default, fallback: None },
            columns: Layered { value: 3, source: Source::Default, fallback: None },
            rows: Layered { value: 2, source: Source::Default, fallback: None },
            layout: Layered { value: config::Layout::Auto, source: Source::Default, fallback: None },
        }),
        detach: true,
        file: ".recruit/settings.toml".into(),
        unreadable: None,
    }
}

/// The running team, as the tests want it: `restarts` started again by any edit.
pub(crate) struct Fake {
    pub restarts: Vec<String>,
    pub conversation: bool,
}

impl Fake {
    pub(crate) fn quiet() -> Self {
        Fake { restarts: Vec::new(), conversation: true }
    }

    pub(crate) fn restarting(names: &[&str]) -> Self {
        Fake { restarts: names.iter().map(|n| n.to_string()).collect(), conversation: true }
    }
}

impl Env for Fake {
    fn restarts(&self, _: &[Edit]) -> Vec<String> {
        self.restarts.clone()
    }

    fn has_conversation(&self, _: &str) -> bool {
        self.conversation
    }
}

/// The sheet of a team whose states are seen: none at work.
pub(crate) fn seen(team: Team) -> Sheet {
    let mut sheet = Sheet::new(team);
    sheet.states_known = true;
    sheet
}
