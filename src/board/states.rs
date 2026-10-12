// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! `states.json`, what the dashboard writes each second for the menu, the journal, the members and the server, and what
//! the server reads of it.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::claude::bridge;
use crate::claude::bridge::Task;
use crate::sys::{now, write_atomic};
use crate::team::state::Snapshot;
use crate::ui::look::{self, Pressure, State};

use super::card::HISTORY;

/// The members' states as the dashboard last saw them, in the team's folder: for the menu, which shows them at once
/// without asking `claude agents`.
pub const STATES: &str = "states.json";

/// What `STATES` holds: written at each look, every second, even when nothing changed; older than a few seconds, the
/// dashboard no longer looks (stopped, or `dashboard = false`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct States {
    /// When the dashboard looked, in seconds since the epoch.
    pub at: i64,
    /// The members with a session open in the team's folder.
    pub members: BTreeMap<String, MemberState>,
    /// Those without.
    pub absent: Vec<String>,
    /// The exact addresses (`name [ref]`) of the members whose name another session carries too, for the team's note
    /// (`live::prompt`); none most of the time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub refs: BTreeMap<String, String>,
    /// The session each member runs in the team's folder, as `claude agents` last said it: for the journal (the
    /// transcripts) and the members' panes (`team/member.rs`, the session to resume), which no longer ask `claude
    /// agents` themselves while the dashboard looks. Not as fresh as `at`: up to about 8 s old when `at` is (asked
    /// every 5 s, the time it takes, then `FRESH`); a reader that acts on a process checks it is still there.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sessions: BTreeMap<String, Session>,
    /// Every session open on this machine by its process, members or not: the names messages go to, for the journal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pids: BTreeMap<u32, String>,
    /// The text of the error each member in `State::Error` failed on, whole: for the menu, which shows it on top of
    /// the member's sheet.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub errors: BTreeMap<String, String>,
    /// What each member's card shows beyond its state, for the cards the multiplexer draws itself (the « others » of
    /// the focus layout): what it does and its curve.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cards: BTreeMap<String, CardLook>,
}

/// What a card shows of a member beyond its state, as `STATES` keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardLook {
    /// What it does, in progress and once done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doing: Option<Task>,
    #[serde(default)]
    pub activity: Activity,
}

/// A member's last half hour, as its curve draws it: the stretches at work, not a sample a second.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// When the dashboard first saw the member, in seconds since the epoch: before it, nothing is known.
    #[serde(default)]
    pub seen: i64,
    /// The stretches at work, `[from, to)` in seconds since the epoch, oldest first.
    #[serde(default)]
    pub work: Vec<(i64, i64)>,
    /// The last second the dashboard noted: after it, nothing is known yet.
    #[serde(default)]
    pub last: i64,
}

impl Activity {
    /// Notes the member at work or not in the second `now`: a second at work joins the stretch that ends where it
    /// begins; what is older than the curve's half hour is forgotten.
    pub(super) fn note(&mut self, now: i64, working: bool) {
        if self.seen == 0 {
            self.seen = now;
        }
        if working {
            // A second between two looks at work, skipped by their period: at work too.
            let one_skipped = self.last != 0 && now == self.last + 2;
            match self.work.last_mut() {
                Some((_, to)) if now <= *to => *to = (now + 1).max(*to),
                Some((_, to)) if one_skipped && *to == self.last + 1 => *to = now + 1,
                _ => self.work.push((now, now + 1)),
            }
        }
        self.last = self.last.max(now);
        let oldest = now - HISTORY + 1;
        self.seen = self.seen.max(oldest);
        self.work.retain(|&(_, to)| to > oldest);
    }
}

/// A member's session in [`States::sessions`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Its id, the name of its transcript.
    pub id: String,
    /// Its Claude's process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MemberState {
    #[serde(with = "state_name")]
    pub state: State,
    /// Since when, in seconds since the epoch, as long as the dashboard has seen it so; for a member renamed on its
    /// conversation, since before its new name. At rest with a command running, since the command started.
    pub since: i64,
    /// At rest while a command it started still runs: shown with a terminal's sign.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shell: bool,
}

/// A state by its name in `STATES`: `working`, `idle`, `waiting` or `other`.
mod state_name {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::ui::look::State;

    pub fn serialize<S: Serializer>(state: &State, to: S) -> Result<S::Ok, S::Error> {
        to.serialize_str(state.name())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<State, D::Error> {
        Ok(State::named(&String::deserialize(from)?))
    }
}

/// What the dashboard last wrote; none if it never did, or while the file cannot be read.
pub fn read_states(state: &Path) -> Option<States> {
    fs::read(state.join(STATES)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// Older than this, in seconds, `STATES` says the dashboard no longer looks: its readers ask for themselves.
pub const FRESH: i64 = 3;

/// What the dashboard last wrote, if it still looks (written less than [`FRESH`] seconds ago).
pub fn fresh_states(state: &Path) -> Option<States> {
    read_states(state).filter(|states| states.fresh(now()))
}

impl States {
    /// Written less than [`FRESH`] seconds from `now`, either way: the clocks of two processes.
    pub fn fresh(&self, now: i64) -> bool {
        (now - self.at).abs() <= FRESH
    }
}

/// Written in one go: the menu never reads half a file.
pub(super) fn save_states(state: &Path, states: &States) -> Result<()> {
    write_atomic(&state.join(STATES), &serde_json::to_string(states)?)
}

/// What a member's header shows in recruit's own multiplexer, as its card on the dashboard shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MemberLook {
    /// Its model's family: Opus, Sonnet, Haiku, Fable.
    pub model: Option<String>,
    /// Its effort, one of `bridge::EFFORTS`.
    pub effort: Option<String>,
    /// Its context, in percent of its window.
    pub context: Option<u8>,
    /// How near it is to compacting on its own.
    pub pressure: Pressure,
    /// At rest as the dashboard last saw it, its context known: a click on its context compacts it.
    pub compactable: bool,
    /// Since when it is in its state as the dashboard last saw it, in seconds since the epoch.
    pub since: Option<i64>,
    /// Its state as the dashboard last saw it: for a member whose program does not tell its own (no OSC 7501).
    pub state: Option<State>,
    /// At rest as the dashboard last saw it, a command it started still running: `since` is when the command started.
    pub shell: bool,
    /// What it does, as the dashboard last saw it: for its card among the « others ».
    pub doing: Option<Task>,
    /// Its last half hour, as the dashboard last wrote it: its curve. Stretches, which hold while nothing changes.
    pub activity: Activity,
    /// When its session was last compacted, by hand or on its own, in seconds since the epoch: a ⟳ in its curve.
    pub compacted: Option<i64>,
}

/// Each member's look, by name, from the team as launched, the mod's reports and what the dashboard last wrote
/// (`STATES`: `since` kept whatever its age, true while the state is; the state, and what follows from it, only while
/// the dashboard looks, so that a dashboard stopped leaves no state frozen in the headers). Reads a few small files:
/// for a thread of the server, every two seconds, never on the way to a frame. Empty if the team cannot be read.
pub(crate) fn looks(state: &Path) -> BTreeMap<String, MemberLook> {
    let Ok(snapshot) = Snapshot::read(state) else { return BTreeMap::new() };
    let states = read_states(state);
    let fresh = states.as_ref().is_some_and(|s| s.fresh(now()));
    snapshot
        .members
        .iter()
        .map(|member| {
            let report = bridge::report(state, &member.name).unwrap_or_default();
            let (model, effort) = look::shown(bridge::model_and_effort_in(&report, member));
            let context = report.context.as_ref();
            let percent =
                context.and_then(|c| c.percent).filter(|p| p.is_finite()).map(|p| p.round().clamp(0.0, 100.0) as u8);
            let seen = states.as_ref().and_then(|s| s.members.get(&member.name));
            let state_seen = seen.filter(|_| fresh);
            let card = states.as_ref().filter(|_| fresh).and_then(|s| s.cards.get(&member.name));
            let look = MemberLook {
                model,
                effort,
                context: percent,
                pressure: report.pressure(),
                compactable: percent.is_some()
                    && state_seen.is_some_and(|s| matches!(s.state, State::Idle | State::Error)),
                since: seen.map(|s| s.since),
                state: state_seen.map(|s| s.state),
                // A failed turn says more than a command running.
                shell: state_seen.is_some_and(|s| s.shell && s.state != State::Error),
                doing: card.and_then(|c| c.doing.clone()),
                activity: card.map(|c| c.activity.clone()).unwrap_or_default(),
                compacted: report.compacted.as_ref().filter(|c| c.done).map(|c| c.at),
            };
            (member.name.clone(), look)
        })
        .collect()
}

/// A member's context as its card shows it, in percent: for the compaction's confirmation. None while its mod has
/// not measured it.
pub fn context_percent(state: &Path, member: &str) -> Option<u8> {
    let percent = bridge::report(state, member)?.context?.percent?;
    percent.is_finite().then(|| percent.round().clamp(0.0, 100.0) as u8)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    use crate::claude::Running;
    use crate::claude::bridge::Task;

    use crate::sys::now;

    use crate::ui::look::State;

    use super::*;

    use crate::board::dashboard::*;
    use crate::board::fixture::*;

    use crate::board::sessions::*;

    use crate::board::Board;

    /// The looks of the headers: the state only while the dashboard looks; stopped, its last states are not shown
    /// as if they held (the pane says « ◌ »), the time and the rest stay.
    #[test]
    fn looks_without_a_stale_state() {
        let state = tempfile::tempdir().unwrap();
        snapshot(&["dev"]).write(state.path()).unwrap();
        let member = MemberState { state: State::Idle, since: 90, shell: true };
        let states = |at| States { at, members: BTreeMap::from([("dev".into(), member)]), ..Default::default() };
        save_states(state.path(), &states(now())).unwrap();
        let look = &looks(state.path())["dev"];
        assert_eq!((look.state, look.shell, look.since), (Some(State::Idle), true, Some(90)));
        save_states(state.path(), &states(now() - 60)).unwrap();
        let look = &looks(state.path())["dev"];
        assert_eq!((look.state, look.shell, look.compactable, look.since), (None, false, false, Some(90)));
    }

    /// What a card among the others shows of a member beyond its state: what it does and its
    /// curve, written by the dashboard as stretches at work, read by the server with the looks; only while the
    /// dashboard looks.
    #[test]
    fn the_cards_of_the_others_get_their_curve_and_task() {
        let now = now();
        // Working for 20 s, 10 s at rest, 30 s at work again, 5 s at rest: one sample a second.
        let samples: Vec<(i64, bool)> = (0..65).map(|i| (now - 64 + i, !(20..30).contains(&i) && i < 60)).collect();
        let activity = activity_of(&samples);
        assert_eq!(activity.seen, now - 64);
        assert_eq!(activity.work, [(now - 64, now - 44), (now - 34, now - 4)]);
        // A second skipped between two looks at work was at work too; two, or one noted at rest, were not.
        let mut skipping = Activity::default();
        skipping.note(10, true);
        skipping.note(12, true);
        assert_eq!((skipping.work.clone(), skipping.last), (vec![(10, 13)], 12));
        skipping.note(15, true);
        assert_eq!(skipping.work, [(10, 13), (15, 16)]);
        let mut resting = Activity::default();
        for (t, working) in [(10, true), (11, false), (12, true)] {
            resting.note(t, working);
        }
        assert_eq!(resting.work, [(10, 11), (12, 13)]);
        // What is older than the half hour is forgotten.
        let mut long = activity.clone();
        long.note(now + HISTORY, false);
        assert_eq!((long.seen, long.work.len()), (now + 1, 0));
        // Through the file and `looks`.
        let state = tempfile::tempdir().unwrap();
        snapshot(&["dev", "quiet"]).write(state.path()).unwrap();
        let member = MemberState { state: State::Working, since: 90, shell: false };
        let task = Task { now: "Je relis la maquette".into(), done: Some("Maquette relue".into()) };
        let mut states = States { at: now, members: BTreeMap::from([("dev".into(), member)]), ..Default::default() };
        states.cards.insert("dev".into(), CardLook { doing: Some(task.clone()), activity: activity.clone() });
        save_states(state.path(), &states).unwrap();
        let looks = looks(state.path());
        assert_eq!((looks["dev"].doing.as_ref(), &looks["dev"].activity), (Some(&task), &activity));
        assert_eq!((looks["quiet"].doing.as_ref(), &looks["quiet"].activity), (None, &Activity::default()));
        // Stale, it is not shown as if it held.
        save_states(state.path(), &States { at: now - 60, ..states }).unwrap();
        let looks = crate::board::states::looks(state.path());
        assert_eq!((looks["dev"].doing.as_ref(), &looks["dev"].activity), (None, &Activity::default()));
        // The dashboard writes it for each card, and an older file without it still reads.
        let board = Board { cards: vec![sample_card("dev", false, State::Working)], now, ..Default::default() };
        let mut memory = Memory::default();
        memory.since.insert("dev".into(), (State::Working, Instant::now()));
        let written = memory.states(&board, SystemTime::now(), Instant::now());
        assert_eq!(written.cards["dev"].activity, board.cards[0].data.activity);
        let old: States = serde_json::from_str(r#"{"at": 1, "members": {}, "absent": []}"#).unwrap();
        assert!(old.cards.is_empty());
    }

    #[test]
    fn exact_addresses_in_the_states_only_when_some() {
        let mut states = States { at: 5, ..Default::default() };
        assert!(!serde_json::to_string(&states).unwrap().contains("refs"));
        states.refs.insert("dev".into(), "a1b2c3".into());
        let written = serde_json::to_string(&states).unwrap();
        assert!(written.contains(r#""refs":{"dev":"a1b2c3"}"#), "{written}");
        // Written by a recruit before them: none.
        let older: States = serde_json::from_str(r#"{"at":5,"members":{},"absent":[]}"#).unwrap();
        assert!(older.refs.is_empty());
    }

    #[test]
    fn states_for_the_menu() {
        let state = tempfile::tempdir().unwrap();
        let running = |name: &str, status: &str, session: &str| {
            let session = Running {
                name: Some(name.into()),
                cwd: Some("/tmp/web".into()),
                status: Some(status.into()),
                session_id: Some(session.into()),
                pid: None,
            };
            (name.to_string(), session)
        };
        let look = |members: &[(&str, &str, &str)]| Sessions {
            members: members.iter().map(|(n, st, id)| running(n, st, id)).collect(),
            ..Default::default()
        };
        let mut memory = Memory::default();
        let s = snapshot(&["dev", "qa", "ops"]);
        let board = memory.board(&s, state.path(), &look(&[("dev", "busy", "s1"), ("qa", "waiting", "s2")]));
        // One moment for every look below: the same moment gives the same since.
        let (now, seen) = (SystemTime::now(), Instant::now());
        let first = memory.states(&board, now, seen);
        let at = now.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(first.at, at);
        assert_eq!(first.members["dev"].state, State::Working);
        assert!((at - 1..=at).contains(&first.members["dev"].since));
        assert_eq!(first.members["qa"].state, State::Waiting);
        assert_eq!(first.absent, ["ops"]);
        // A look later, the same moment.
        let board = memory.board(&s, state.path(), &look(&[("dev", "busy", "s1"), ("qa", "waiting", "s2")]));
        assert_eq!(memory.states(&board, now, seen).members, first.members);
        // Renamed on its conversation, it keeps its moment; a new state, a new one.
        let renamed = snapshot(&["dev-rust", "qa"]);
        let board = memory.board(&renamed, state.path(), &look(&[("dev-rust", "busy", "s1"), ("qa", "idle", "s2")]));
        let states = memory.states(&board, now, seen);
        assert_eq!(states.members["dev-rust"], first.members["dev"]);
        assert!(!states.members.contains_key("dev"));
        assert_eq!(states.members["qa"].state, State::Idle);
        assert!(states.members["qa"].since >= at - 1);

        // Written whole, read back as it was.
        assert_eq!(read_states(state.path()), None);
        save_states(state.path(), &states).unwrap();
        assert_eq!(read_states(state.path()), Some(states.clone()));
        let text = fs::read_to_string(state.path().join(STATES)).unwrap();
        assert!(text.contains(r#""qa":{"state":"idle","#), "{text}");
        // A state of a later recruit, as one it does not know.
        fs::write(state.path().join(STATES), r#"{"at":1,"members":{"a":{"state":"asleep","since":0}},"absent":[]}"#)
            .unwrap();
        let read = read_states(state.path()).unwrap();
        assert_eq!((read.members["a"].state, read.members["a"].shell), (State::Other, false));
        // A command running: written only then, read back.
        assert!(!text.contains("shell"), "{text}");
        let mut states = states;
        states.members.get_mut("qa").unwrap().shell = true;
        save_states(state.path(), &states).unwrap();
        assert!(fs::read_to_string(state.path().join(STATES)).unwrap().contains(r#""shell":true"#));
        assert_eq!(read_states(state.path()), Some(states));
    }
}
