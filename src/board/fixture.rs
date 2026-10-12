// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the tests of the panels share: a team's cards and board, and a line as it is seen.

use crossterm::style::Color;

use crate::claude::bridge::{Helper, HelperStatus, Task};
use crate::t;
use crate::team::state::Snapshot;
use crate::ui::look::{Pressure, State};

use super::dashboard::*;
use super::paint::*;
use super::*;
use crate::board::zones::*;

pub(super) trait Printed {
    fn printed(&self) -> String;
}

impl Printed for str {
    fn printed(&self) -> String {
        self.to_string()
    }
}

impl Printed for String {
    fn printed(&self) -> String {
        self.clone()
    }
}

impl Printed for Line {
    fn printed(&self) -> String {
        Line::printed(self)
    }
}

/// A line's text: a printed one without its SGR sequences.
pub(super) fn visible(line: &(impl Printed + ?Sized)) -> String {
    let line = line.printed();
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(super) fn sample_card(name: &str, contact: bool, state: State) -> Card {
    let data = CardData {
        name: name.into(),
        color: Color::Cyan,
        state,
        since: 90,
        shell: false,
        context: Some(41.0),
        pressure: Pressure::Calm,
        model: Some("Opus".into()),
        effort: Some("xhigh".into()),
        doing: None,
        activity: activity_of(&(0..600).rev().map(|i| (1000 - i, i % 3 == 0)).collect::<Vec<_>>()),
        failed: None,
        compacted: None,
    };
    Card { data, contact, helpers: Vec::new(), error: None }
}

impl Card {
    /// This card with `change` done to it.
    pub(super) fn tweak(mut self, change: impl FnOnce(&mut Card)) -> Card {
        change(&mut self);
        self
    }
}

pub(super) fn board(cards: Vec<Card>) -> Board {
    Board {
        now: 1000,
        usage: vec![("5 h".into(), 41.0, Some(4920)), ("7 j".into(), 5.0, Some(560_000))],
        cards,
        absent: vec!["ops".into()],
        ..Default::default()
    }
}

/// The team of a capture: thirteen members, the contact and nine agents at rest (two who just finished, some
/// without a task), two at work, one with a task over two lines.
pub(super) fn capture() -> Vec<Card> {
    let task = |now: &str, done: &str| Some(Task { now: now.into(), done: Some(done.into()) });
    let at = |name: &str, state, since, doing| {
        sample_card(name, false, state).tweak(|c| {
            c.data.since = since;
            c.data.doing = doing;
        })
    };
    let mut cards = vec![
        sample_card("coordinateur", true, State::Idle).tweak(|c| {
            c.data.doing = task("Je répartis les tâches", "Tâches réparties");
        }),
        at("opérateur", State::Idle, 4000, None),
        at("ui-ux", State::Idle, 3000, task("Je relis les écrans", "Écrans relus")),
        at("designer", State::Idle, 20, task("Je livre la maquette", "Maquette des cartes livrée")),
        at(
            "gpu-engine",
            State::Working,
            120,
            task("Je profile le rendu des tuiles et je corrige la fuite du cache de textures", "Fuite corrigée"),
        ),
        at("performance", State::Idle, 2500, None),
        at("dev-back", State::Working, 300, task("Je migre l'API des cartes vers le nouveau schéma", "API migrée")),
        at("sécurité", State::Idle, 1800, task("J'audite les dépendances", "Dépendances auditées")),
        at("devops", State::Idle, 900, None),
        at("traitement-cartes", State::Idle, 1200, task("Je découpe les cartes", "Cartes découpées en tuiles")),
        at("planificateur", State::Idle, 5000, None),
        at("reviewer", State::Idle, 45, task("Je relis la PR 42", "Relecture de la PR 42 terminée")),
    ];
    cards.insert(1, at("opérateur-ui-ux", State::Idle, 3500, None));
    cards
}

/// The lines of a dashboard: no more than the pane holds, no card with an empty line, every zone over what it names.
/// The dashboard in a pane `height` rows high as these tests measure it, from when it had a header of two rows:
/// those two rows blank, then the cards in the room they leave. Each test keeps its room and its rows.
pub(super) fn padded(b: &Board, width: usize, height: usize) -> Drawn {
    let mut drawn = Drawn::from(vec![Line::default(); 2.min(height.saturating_sub(1))]);
    drawn.append(render(b, width, height.saturating_sub(2)));
    drawn
}

/// Where a member's card starts, if it has one.
pub(super) fn title(lines: &[String], name: &str) -> Option<usize> {
    lines.iter().position(|l| l.contains(&format!(" {name} ")) && l.contains('╻'))
}

/// What a member's card says it does, line by line, its capsule first.
pub(super) fn task(lines: &[String], name: &str) -> Vec<String> {
    let top = title(lines, name).unwrap();
    let col = lines[top].find(&format!(" {name} ")).unwrap();
    let col = lines[top][..col].chars().count() - 3;
    let column = |l: &String| l.chars().skip(col).collect::<String>();
    let middles = lines[top + 1..].iter().map(column).take_while(|l| l.starts_with('┃'));
    // The card on the right of a pair ends at its own width.
    middles.map(|l| l.split(" ┃ ").next().unwrap_or_default().trim_end().to_string()).collect()
}

/// The agents at rest only named, as they show.
pub(super) fn named(lines: &[String]) -> Vec<String> {
    let rest = t!("au repos", "idle");
    let from = lines.iter().position(|l| l.starts_with(&rest)).unwrap_or(lines.len());
    let list = lines[from..].iter().take_while(|l| !l.is_empty()).map(|l| l.trim_start_matches(&rest));
    let list = list.map(|l| l.split(" … ").next().unwrap_or_default().to_string());
    list.flat_map(|l| l.split(" · ").map(|n| n.trim().to_string()).collect::<Vec<_>>()).collect()
}

/// Two members: one whose task goes over two lines in one column, one at rest whose task holds in one.
pub(super) fn long_and_short() -> Vec<Card> {
    let task = |now: &str, done: Option<&str>| Some(Task { now: now.into(), done: done.map(String::from) });
    vec![
        sample_card("coordinateur", true, State::Working).tweak(|c| {
            c.data.doing = task("Je relis la maquette des cartes et je corrige les colonnes de droite", None);
        }),
        sample_card("dev", false, State::Idle).tweak(|c| {
            c.data.doing = task("Je publie la version 0.6.2", Some("Version 0.6.2 publiée"));
        }),
    ]
}

pub(super) fn helper(id: &str, teammate: Option<&str>, status: HelperStatus) -> Helper {
    Helper {
        id: id.into(),
        kind: "Explore".into(),
        description: format!("Lire {id}"),
        teammate: teammate.map(String::from),
        status,
        started: 990,
        since: 990,
    }
}

/// Each zone over what it names: a whole card, in its capsule, with the member's name on top; the name itself; or
/// the context to compact.
pub(super) fn zones_match(drawn: &Drawn) {
    for zone in &drawn.zones {
        let line: Vec<char> = visible(&drawn.lines[zone.row]).chars().collect();
        let part: String = line[zone.col..zone.col + zone.cols].iter().collect();
        if zone.kind == ZoneKind::Compact {
            assert_eq!((zone.rows, part.chars().next()), (1, Some('⟳')), "{zone:?}");
            assert!(part.ends_with(" %"), "{zone:?}");
        } else if zone.rows > 1 {
            assert!(part.starts_with('╻') && part.contains(&zone.member), "{zone:?}");
            let bottom = visible(&drawn.lines[zone.row + zone.rows - 1]);
            assert!(bottom.chars().nth(zone.col) == Some('╹'), "{zone:?}");
        } else {
            // A name too wide for the line is cut; its zone still names the member.
            let shown = part
                .strip_suffix('…')
                .filter(|cut| zone.member.starts_with(cut))
                .map_or(part.as_str(), |_| &zone.member);
            assert_eq!((zone.rows, shown), (1, zone.member.as_str()), "{zone:?}");
        }
    }
}

pub(super) fn snapshot(names: &[&str]) -> Snapshot {
    let member = |name: &&str| crate::team::state::MemberInfo {
        name: name.to_string(),
        role: String::new(),
        contact: false,
        quiet: false,
        argv: Vec::new(),
        ..Default::default()
    };
    Snapshot {
        team: "web".into(),
        session: "web".into(),
        dir: "/tmp/web".into(),
        claude: "claude".into(),
        config_dir: None,
        lang: Lang::Fr,
        members: names.iter().map(member).collect(),
        dashboard: Vec::new(),
        journal: Vec::new(),
        ..Default::default()
    }
}

/// The activity of a member seen second by second: `samples` as (time, at work), oldest first.
pub(super) fn activity_of(samples: &[(i64, bool)]) -> Activity {
    let mut activity = Activity::default();
    for &(at, working) in samples {
        activity.note(at, working);
    }
    activity
}
