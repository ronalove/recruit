// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The sessions of the team's members and of the others, as `claude agents` and the members' panes tell them.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::claude::{self, Running};
use crate::mux::proto::PaneInfo;
use crate::sys::now;
use crate::team::member;
use crate::team::state::Snapshot;
use crate::ui::look::State;

use super::states::*;

/// The members' sessions by name, and every session's name by process: replies are addressed to a process.
#[derive(Default, Clone)]
pub(super) struct Sessions {
    pub(super) members: HashMap<String, Running>,
    pub(super) by_pid: HashMap<u32, String>,
    /// Sessions open elsewhere under a member's name, with the folder they work in: messages by name could go astray.
    pub(super) elsewhere: Vec<(String, Option<String>)>,
    /// The exact addresses of the members whose name another session carries too (`claude::addresses`).
    pub(super) refs: BTreeMap<String, String>,
    /// The members at rest while a command they started still runs (see [`at_rest_with_a_command`]), and since when,
    /// when their file says it.
    pub(super) shell: HashMap<String, Option<i64>>,
    /// The states the members' panes tell (OSC 7501), and since when in seconds since the epoch: they come before
    /// `claude agents`, which says them only once asked.
    pub(super) told: HashMap<String, (State, Option<u64>)>,
    /// The text of the error each member's turn failed on, as its pane last said it (OSC 7501 `state=error`).
    pub(super) failed: HashMap<String, String>,
    /// Until `claude agents` first answers: the states file as it was, whose exact addresses, sessions and processes
    /// are written again rather than emptied (a dashboard started again would change the team's note for a second).
    pub(super) kept: Option<Box<States>>,
    pub(super) error: Option<String>,
}

/// The states file to keep writing until `claude agents` first answers: only one a dashboard wrote moments ago (one
/// started again); not an older one, a launch before's, whose sessions and addresses are no longer.
pub(super) fn kept(state: &Path) -> Option<Box<States>> {
    read_states(state).filter(|states| states.fresh(now())).map(Box::new)
}

/// The members' states as their panes tell them (OSC 7501), by name; none when the server could not be asked.
pub(super) fn told(panes: Option<&[PaneInfo]>) -> HashMap<String, (State, Option<u64>)> {
    let members = panes.unwrap_or_default().iter().filter(|pane| pane.role.is_empty());
    members
        .filter_map(|pane| Some((pane.member.clone(), (State::reported(pane.status.as_deref())?, pane.since))))
        .collect()
}

/// The text of the error each member's pane says its turn failed on, by name.
pub(super) fn failed(panes: Option<&[PaneInfo]>) -> HashMap<String, String> {
    let members = panes.unwrap_or_default().iter().filter(|pane| pane.role.is_empty());
    members
        .filter(|pane| State::reported(pane.status.as_deref()) == Some(State::Error))
        .filter_map(|p| Some((p.member.clone(), p.error.clone()?)))
        .collect()
}

/// A member whose pane says it is at rest may have a command still running (its pane says `idle` then, its session's
/// file `shell`): read in its file, by the process `claude agents` last gave. Its pane says it works: no command shown.
pub(super) fn with_a_command(found: &mut Sessions, status: impl Fn(u32) -> Option<claude::SessionFile>) {
    for (name, (state, _)) in &found.told {
        if *state != State::Idle {
            found.shell.remove(name);
            continue;
        }
        let file = found.members.get(name).and_then(|session| session.pid).and_then(&status);
        match file.filter(|file| file.status == "shell") {
            Some(file) => found.shell.insert(name.clone(), file.since),
            None => found.shell.remove(name),
        };
    }
}

/// The sessions as `claude agents` gives them; with the exact addresses of the members whose name another session
/// carries too when `refs` gives the team's state folder (the dashboard, for `STATES`; the journal has no use for
/// them).
pub(super) fn sessions(s: &Snapshot, refs: Option<&Path>) -> Sessions {
    match claude::running(&s.claude, s.config_dir.as_deref()) {
        Ok(running) => {
            let refs = refs.map_or_else(BTreeMap::new, |state| {
                let names: Vec<&str> = s.members.iter().map(|m| m.name.as_str()).collect();
                claude::addresses(
                    &s.dir,
                    &names,
                    &running,
                    |member| member::noted(state, member),
                    |pid| claude::session_ref(s.config_dir.as_deref(), pid),
                )
            });
            let mut found = Sessions { refs, ..sorted(s, running) };
            at_rest_with_a_command(&mut found, |pid| claude::session_status(s.config_dir.as_deref(), pid));
            found
        }
        Err(error) => Sessions { error: Some(format!("{error:#}")), ..Default::default() },
    }
}

/// `claude agents` says `busy` of a session whose turn is over while a command it started still runs (a server, or
/// one that hangs): its own file says `shell` (2.1.294). Such a member is at rest, its card says the command runs.
/// Back at work meanwhile, its file says `busy` again. No file, or not as expected: as `claude agents` says.
pub(super) fn at_rest_with_a_command(found: &mut Sessions, status: impl Fn(u32) -> Option<claude::SessionFile>) {
    for (name, session) in &mut found.members {
        let Some(pid) = session.pid.filter(|_| session.status.as_deref() == Some("busy")) else { continue };
        if let Some(file) = status(pid).filter(|file| file.status == "shell") {
            session.status = Some("idle".into());
            found.shell.insert(name.clone(), file.since);
        }
    }
}

/// `text` without what printing it as it is would let it do: no control character (an escape sequence, a clipboard
/// write), no formatting one (text turned right to left, invisible characters).
pub(super) fn printable(text: &str) -> String {
    let formatting = |c: char| {
        matches!(c,
            '\u{AD}' | '\u{61C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
    };
    text.chars().filter(|&c| !c.is_control() && !formatting(c)).collect()
}

/// The sessions `claude agents --json` gives, sorted out for the team.
pub(super) fn sorted(s: &Snapshot, running: Vec<Running>) -> Sessions {
    let mut found = Sessions::default();
    for session in running {
        let Some(name) = session.name.clone() else { continue };
        if let Some(pid) = session.pid {
            found.by_pid.insert(pid, name.clone());
        }
        if !s.members.iter().any(|m| m.name == name) {
            continue;
        }
        let here = session.cwd.as_deref().is_some_and(|cwd| Path::new(cwd) == s.dir);
        if here {
            found.members.insert(name, session);
            continue;
        }
        let folder = session.cwd.as_deref().and_then(|cwd| Path::new(cwd).file_name());
        let double = (name, folder.map(|f| printable(&f.to_string_lossy())));
        if !found.elsewhere.contains(&double) {
            found.elsewhere.push(double);
        }
    }
    // In the file's order, whatever order `claude agents` gives: the line stays as it is from one look to the next.
    let order = |name: &String| s.members.iter().position(|m| &m.name == name);
    found.elsewhere.sort_by(|a, b| order(&a.0).cmp(&order(&b.0)).then_with(|| a.1.cmp(&b.1)));
    found
}

/// The sessions of `running` that are the team's members', by name: those open in the team's folder.
pub(crate) fn members_of(s: &Snapshot, running: Vec<Running>) -> HashMap<String, Running> {
    sorted(s, running).members
}

impl Sessions {
    /// `states` with these sessions: each member's, every one's name by process, and the exact addresses of the
    /// members whose name another session carries too; until `claude agents` answers, those of the file as it was.
    pub(super) fn written(&self, states: States) -> States {
        if let Some(kept) = &self.kept {
            let (refs, sessions, pids) = (kept.refs.clone(), kept.sessions.clone(), kept.pids.clone());
            return States { refs, sessions, pids, ..states };
        }
        let sessions = self.members.iter().filter_map(|(name, running)| {
            let id = running.session_id.clone()?;
            Some((name.clone(), Session { id, pid: running.pid }))
        });
        States {
            sessions: sessions.collect(),
            pids: self.by_pid.iter().map(|(pid, name)| (*pid, name.clone())).collect(),
            refs: self.refs.clone(),
            ..states
        }
    }

    /// The sessions as the dashboard wrote them in `states`, for the journal: the members' ids and processes, and every
    /// one's name by process. None while the dashboard has members but no sessions yet (`claude agents` has not
    /// answered it): the journal then asks `claude agents` itself.
    pub(super) fn of(states: States) -> Option<Sessions> {
        if states.sessions.is_empty() && !states.members.is_empty() {
            return None;
        }
        let members = states.sessions.into_iter().map(|(name, session)| {
            let running = Running {
                name: Some(name.clone()),
                cwd: None,
                status: None,
                session_id: Some(session.id),
                pid: session.pid,
            };
            (name, running)
        });
        Some(Sessions { members: members.collect(), by_pid: states.pids.into_iter().collect(), ..Default::default() })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use std::time::{Instant, SystemTime};

    use crate::claude::{self, Running};

    use crate::mux::proto::PaneInfo;
    use crate::sys::now;

    use crate::ui::look::{Glyphs, State};

    use super::*;

    use crate::board::card::*;

    use crate::board::dashboard::*;
    use crate::board::fixture::*;

    #[test]
    fn at_rest_while_a_command_runs() {
        let session = |status: &str, pid| Running {
            name: None,
            cwd: None,
            status: Some(status.into()),
            session_id: None,
            pid: Some(pid),
        };
        let mut found = Sessions {
            members: [
                ("server", session("busy", 1)),
                ("thinking", session("busy", 2)),
                ("unknown", session("busy", 3)),
                ("resting", session("idle", 4)),
            ]
            .map(|(name, session)| (name.to_string(), session))
            .into(),
            ..Default::default()
        };
        // Its own file: a command running past the turn, at work, none; one at rest is not asked.
        let files = |pid| match pid {
            1 | 4 => Some(claude::SessionFile { status: "shell".into(), since: Some(now() - 600) }),
            2 => Some(claude::SessionFile { status: "busy".into(), since: None }),
            _ => None,
        };
        at_rest_with_a_command(&mut found, files);
        let status = |name: &str| found.members[name].status.clone().unwrap();
        assert_eq!(
            [status("server"), status("thinking"), status("unknown"), status("resting")],
            ["idle", "busy", "busy", "idle"]
        );
        assert_eq!(found.shell.keys().collect::<Vec<_>>(), ["server"]);

        // On the board, and for the menu: at rest, under a terminal's sign.
        let s = snapshot(&["server", "thinking", "unknown", "resting"]);
        let state = tempfile::tempdir().unwrap();
        let mut memory = Memory::default();
        let seen = memory.board(&s, state.path(), &found);
        let server = seen.cards.iter().find(|c| c.data.name == "server").unwrap();
        assert_eq!((server.data.state, server.data.shell), (State::Idle, true));
        // Since its file says, ten minutes: not since the panel looked.
        assert!((600..610).contains(&server.data.since), "{}", server.data.since);
        let states = memory.states(&seen, SystemTime::now(), Instant::now());
        assert_eq!(states.members["server"].state, State::Idle);
        let right = Right::of([(&server.data, 50)]);
        let title = visible(&card(&server.data, 50, 0, &right, &board(Vec::new()))[0]);
        assert!(title.starts_with(&format!("╻ {} server", Glyphs::Unicode.console())), "{title:?}");
    }

    /// The states the members' panes tell (OSC 7501) come before `claude agents`, with since when they say; a member
    /// told by its pane has a card before `claude agents` sees it; at rest by its pane, a command still running is read
    /// in its session's file.
    #[test]
    fn states_told_by_the_panes() {
        let pane = |member: &str, role: &str, status: Option<State>, since| PaneInfo {
            id: member.into(),
            member: member.into(),
            role: role.into(),
            tab: "Agents".into(),
            pid: 0,
            cols: 80,
            rows: 24,
            status: status.map(|s| s.name().to_string()),
            since,
            error: None,
        };
        let at = now() as u64;
        let panes = [
            pane("lead", "", Some(State::Working), Some(at - 120)),
            pane("server", "", Some(State::Idle), Some(at - 30)),
            pane("new", "", Some(State::Waiting), None),
            pane("quiet", "", None, None),
            pane("Dashboard", crate::mux::plan::DASHBOARD, Some(State::Working), None),
        ];
        let told = told(Some(&panes));
        assert_eq!(
            told.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
            ["lead", "new", "server"].into()
        );
        assert!(crate::board::sessions::told(None).is_empty());

        let session = |status: &str, pid| Running {
            name: None,
            cwd: None,
            status: Some(status.into()),
            session_id: None,
            pid: Some(pid),
        };
        let mut found = Sessions {
            // As `claude agents` said a while ago: the lead at rest, the server at work, `quiet` waiting.
            members: [("lead", session("idle", 1)), ("server", session("busy", 2)), ("quiet", session("waiting", 3))]
                .map(|(name, session)| (name.to_string(), session))
                .into(),
            shell: HashMap::from([("lead".to_string(), None)]),
            told,
            ..Default::default()
        };
        let files = |pid| (pid == 2).then(|| claude::SessionFile { status: "shell".into(), since: Some(now() - 600) });
        with_a_command(&mut found, files);
        assert_eq!(found.shell.keys().collect::<Vec<_>>(), ["server"]);

        let s = snapshot(&["lead", "server", "new", "quiet", "gone"]);
        let state = tempfile::tempdir().unwrap();
        let seen = Memory::default().board(&s, state.path(), &found);
        let card = |name: &str| seen.cards.iter().find(|c| c.data.name == name).unwrap();
        assert_eq!(card("lead").data.state, State::Working);
        assert!((120..123).contains(&card("lead").data.since), "{}", card("lead").data.since);
        assert_eq!((card("server").data.state, card("server").data.shell), (State::Idle, true));
        assert!((600..603).contains(&card("server").data.since), "{}", card("server").data.since);
        assert_eq!((card("new").data.state, card("new").data.since), (State::Waiting, 0));
        assert_eq!(card("quiet").data.state, State::Waiting);
        assert_eq!(seen.absent, ["gone"]);
    }

    #[test]
    fn sessions_of_the_team_and_elsewhere() {
        let s = snapshot(&["coordinateur", "dev"]);
        let running: Vec<Running> = serde_json::from_value(serde_json::json!([
            {"name": "dev", "cwd": "/tmp/autre", "pid": 4},
            {"name": "coordinateur", "cwd": "/tmp/web", "status": "busy", "pid": 1},
            {"name": "coordinateur", "cwd": "/home/user/code/shop", "pid": 2},
            {"name": "dev", "cwd": "/tmp/autre", "pid": 5},
            {"name": "dev", "pid": 6},
            {"name": "inconnu", "cwd": "/tmp/ailleurs", "pid": 7},
            {"name": "dev", "cwd": "/tmp/pi\u{1b}[31mège", "pid": 8},
            {"name": "dev", "cwd": "/tmp/\u{202e}gpj.exe\u{200b}", "pid": 9},
        ]))
        .unwrap();
        let found = sorted(&s, running);
        assert_eq!(found.members.keys().collect::<Vec<_>>(), ["coordinateur"]);
        assert_eq!(found.by_pid.get(&7).map(String::as_str), Some("inconnu"));
        // In the file's order, each once; the folder printed without its control or formatting characters.
        let elsewhere: Vec<(&str, Option<&str>)> =
            found.elsewhere.iter().map(|(n, f)| (n.as_str(), f.as_deref())).collect();
        assert_eq!(
            elsewhere,
            [
                ("coordinateur", Some("shop")),
                ("dev", None),
                ("dev", Some("autre")),
                ("dev", Some("gpj.exe")),
                ("dev", Some("pi[31mège"))
            ]
        );
    }

    /// The sessions written in the states for the journal and the members' panes, and read back.
    #[test]
    fn sessions_in_the_states() {
        let s = snapshot(&["coordinateur", "dev"]);
        let running: Vec<Running> = serde_json::from_value(serde_json::json!([
            {"name": "coordinateur", "cwd": "/tmp/web", "status": "busy", "pid": 1, "sessionId": "s1"},
            {"name": "dev", "cwd": "/tmp/web", "status": "idle", "pid": 3},
            {"name": "inconnu", "cwd": "/tmp/ailleurs", "pid": 7, "sessionId": "s7"},
        ]))
        .unwrap();
        let states = sorted(&s, running).written(States { at: 100, ..Default::default() });
        assert_eq!(
            states.sessions,
            BTreeMap::from([("coordinateur".into(), Session { id: "s1".into(), pid: Some(1) })])
        );
        assert_eq!(states.pids, BTreeMap::from([(1, "coordinateur".into()), (3, "dev".into()), (7, "inconnu".into())]));
        let text = serde_json::to_string(&states).unwrap();
        let read = Sessions::of(serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(read.members["coordinateur"].session_id.as_deref(), Some("s1"));
        assert_eq!(read.by_pid.get(&7).map(String::as_str), Some("inconnu"));
        // Before `claude agents` first answers: the file's addresses, sessions and processes written again, not emptied.
        let refs = BTreeMap::from([("coordinateur".to_string(), "coordinateur [ceeb11]".to_string())]);
        let file = States { refs: refs.clone(), ..states.clone() };
        let waiting = Sessions { kept: Some(Box::new(file.clone())), ..Default::default() };
        let again = waiting.written(States { at: 101, ..Default::default() });
        assert_eq!((again.at, &again.refs, &again.sessions, &again.pids), (101, &refs, &file.sessions, &file.pids));
        // Once it answered, its own: the addresses included.
        let answered = Sessions { refs: BTreeMap::new(), ..sorted(&s, Vec::new()) };
        assert!(answered.written(States { at: 102, ..Default::default() }).refs.is_empty());
        let named = Sessions { refs: refs.clone(), ..Default::default() };
        assert_eq!(named.written(States::default()).refs, refs);
        // Kept from the file only when a dashboard wrote it moments ago: not a launch before's.
        let folder = tempfile::tempdir().unwrap();
        assert!(kept(folder.path()).is_none(), "no file");
        save_states(folder.path(), &States { at: now(), ..file.clone() }).unwrap();
        assert_eq!(kept(folder.path()).map(|k| k.refs), Some(refs.clone()));
        save_states(folder.path(), &States { at: now() - FRESH - 2, ..file.clone() }).unwrap();
        assert!(kept(folder.path()).is_none(), "older than FRESH");
        // Members but no sessions yet (`claude agents` has not answered): the journal asks `claude agents` itself.
        let member = MemberState { state: State::Idle, since: 90, shell: false };
        let old = States { at: 100, members: BTreeMap::from([("dev".into(), member)]), ..Default::default() };
        assert!(Sessions::of(old).is_none());
        // No member running: nothing to read, nothing to ask.
        assert!(
            Sessions::of(States { at: 100, absent: vec!["dev".into()], ..Default::default() })
                .unwrap()
                .members
                .is_empty()
        );
    }
}
