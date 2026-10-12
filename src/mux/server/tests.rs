// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The server's tests: the loop, the panes, the screen, the compaction, the places of « focus ».

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::choice::{Open, Purpose};
use super::clients::{Client, NOTIFICATIONS, Outbox, Relays};
use super::compaction::{AUTO_AGAIN, AUTO_PENDING, Asked, COMPACTING, ask_compaction};
use super::events::next_member;
use super::focus_layout::DOUBLE_CLICK;
use super::frame::{common_forms, member_color, tick, urgent};
use super::lifecycle::{now, threads};
use super::menu::{LIST_SIZE, list_area};
use super::notice::BURST;
use super::pane::{Pane, TurnError};
use super::status::{last_words, note_status, one_line, pane_state};
use super::tabs::{Side, Tab};
use crate::mux::Caps;
use crate::mux::cards;
use crate::mux::chrome::{self, Target};
use crate::mux::engine::{self, Relay, State};
use crate::mux::focus::{self, Places};
use crate::mux::input::{self, Button, Mods, Mouse, MouseKind};
use crate::mux::keys::Action;
use crate::mux::layout;
use crate::mux::proto::{ClientMsg, PaneInfo, PaneSpec, Reply, Request, TabSpec, Welcome};
use crate::mux::screen::View;
use crate::mux::socket;
use crate::team::state::Snapshot;
use crate::ui::Rect;
use crate::ui::canvas::Painter;
use crate::ui::look;
use crate::{board, claude::bridge};

use super::*;

#[test]
fn threads_are_counted() {
    let before = threads().expect("the system says");
    assert!(before >= 1);
    let (go, wait) = mpsc::channel::<()>();
    let other = std::thread::spawn(move || {
        let _ = wait.recv();
    });
    // Tests run side by side: others may come and go meanwhile, but this one is there.
    assert!(threads().unwrap() >= 2);
    drop(go);
    other.join().unwrap();
}

#[test]
fn relays_are_bounded() {
    let mut relays = Relays::default();
    for i in 0..10 {
        relays.add(Relay::Clipboard(format!("copy {i}")));
        relays.add(Relay::Title(format!("title {i}")));
        relays.add(Relay::Bell);
    }
    for i in 0..100 {
        relays.add(Relay::Notify { title: None, body: format!("n{i}") });
    }
    let bytes = String::from_utf8(relays.take()).unwrap();
    // The last clipboard and title only, 64 notifications, one bell.
    assert_eq!(bytes.matches("]52;").count(), 1);
    assert_eq!(bytes.matches("]2;").count(), 1);
    assert!(bytes.contains("title 9"));
    assert_eq!(bytes.matches("]9;").count(), NOTIFICATIONS);
    assert!(bytes.contains("n99") && !bytes.contains("n35\x07"));
    assert_eq!(bytes.matches('\x07').count(), 1 + 1 + NOTIFICATIONS + 1);
    assert!(relays.take().is_empty());
}

#[test]
fn the_most_urgent_state_shows() {
    use look::State::*;
    assert_eq!(urgent([Idle, Working, Waiting].into_iter()), Waiting);
    assert_eq!(urgent([Idle, Working].into_iter()), Working);
    assert_eq!(urgent([Other, Idle].into_iter()), Idle);
    assert_eq!(urgent(std::iter::empty()), Other);
}

#[test]
fn the_next_pane_is_a_members() {
    let members: Vec<String> = ["p1", "p2", "p3"].map(String::from).to_vec();
    assert_eq!(next_member(&members, Some("p1")).as_deref(), Some("p2"));
    assert_eq!(next_member(&members, Some("p3")).as_deref(), Some("p1"));
    // From the dashboard or the journal, which ⌥n never reaches: the first member.
    assert_eq!(next_member(&members, Some("p9")).as_deref(), Some("p1"));
    assert_eq!(next_member(&members, None).as_deref(), Some("p1"));
    // One member: nothing to do.
    let alone = vec!["p1".to_string()];
    assert_eq!(next_member(&alone, Some("p1")), None);
    assert_eq!(next_member(&[], Some("p1")), None);
}

/// A pane that runs a little: enough for the requests below.
fn sleeper(member: &str) -> PaneSpec {
    PaneSpec {
        member: member.into(),
        role: String::new(),
        argv: vec!["/bin/sleep".into(), "30".into()],
        env: Vec::new(),
        cwd: "/".into(),
    }
}

/// A server for a test, built with `tabs` on an 80 × 24 screen, its members `names` in team.json.
fn built(state: &Path, names: &[&str], tabs: Vec<TabSpec>) -> Server {
    let members =
        names.iter().map(|name| crate::team::state::MemberInfo { name: name.to_string(), ..Default::default() });
    Snapshot { members: members.collect(), ..Default::default() }.write(state).unwrap();
    let (tx, _rx) = mpsc::channel();
    let welcome = Arc::new(Mutex::new(Welcome::default()));
    let mut server = Server::new(state, "t", (state.join("s.sock"), socket::FileId::default()), tx, welcome);
    let build = Request::Build {
        team: "t".into(),
        dir: "/".into(),
        columns: 3,
        tabs,
        cols: 80,
        rows: 24,
        exe: String::new(),
        lang: String::new(),
        focus: false,
    };
    assert!(matches!(server.request(build), Reply::Ok(_)));
    server
}

/// A client attached to `server`, whose frames nothing reads: the notices are for a client only.
fn attached(server: &mut Server) -> UnixStream {
    let (stream, other) = UnixStream::pair().unwrap();
    let caps = Caps::default();
    server.client = Some(Client {
        id: "c1".into(),
        stream,
        out: Arc::new(Outbox::default()),
        painter: Painter::new(caps.output),
        caps,
        term: "xterm".into(),
        size: (80, 24),
        since: 0,
        busy: false,
        late: false,
        motion: false,
        relays: Relays::default(),
    });
    other
}

fn id_of(server: &Server, member: &str) -> String {
    server.panes.iter().find(|pane| pane.member == member).unwrap().id.clone()
}

#[test]
fn a_failed_panes_last_words_are_logged_clean() {
    let state = tempfile::tempdir().unwrap();
    let mut said = sleeper("dev-a");
    let script = "printf 'thread main panicked at src/board.rs:9:\\n\\033]0;x\\007over\\302\\233[2J\\342\\200\\256flow\\n'; sleep 30";
    said.argv = ["/bin/sh", "-c", script].map(String::from).to_vec();
    let tabs = vec![TabSpec { title: "1".into(), panes: vec![said], side: None }];
    let server = built(state.path(), &["dev-a"], tabs);
    let rows = server.panes[0].size.1;
    let until = Instant::now() + Duration::from_secs(5);
    let mut words = String::new();
    while !words.contains("flow") && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
        words = server.panes[0].with(|engine| last_words(engine, rows)).unwrap_or_default();
    }
    assert_eq!(words, "thread main panicked at src/board.rs:9: ⏎ over[2Jflow");
}

/// What a pane's program has written, as `last_words` reads it.
fn said(server: &Server, i: usize) -> String {
    let rows = server.panes[i].size.1;
    server.panes[i].with(|engine| last_words(engine, rows)).unwrap_or_default()
}

/// Waits, five seconds at most, for what the pane's program wrote to contain `text`.
fn wait_said(server: &Server, i: usize, text: &str) -> String {
    let until = Instant::now() + Duration::from_secs(5);
    let mut words = said(server, i);
    while !words.contains(text) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
        words = said(server, i);
    }
    words
}

/// A pane whose engine panicked gets a blank one, and its program a SIGWINCH to draw itself again (the dashboard
/// relies on it: it writes by difference, and would stay blank).
#[test]
fn a_pane_whose_engine_broke_is_drawn_again_by_its_program() {
    let state = tempfile::tempdir().unwrap();
    let mut drawn = sleeper("dev-a");
    let script = "trap 'echo redrawn' WINCH; echo ready; while :; do sleep 0.05; done";
    drawn.argv = ["/bin/sh", "-c", script].map(String::from).to_vec();
    let tabs = vec![TabSpec { title: "1".into(), panes: vec![drawn], side: None }];
    let mut server = built(state.path(), &["dev-a"], tabs);
    assert!(wait_said(&server, 0, "ready").contains("ready"));
    server.panes[0].broken.mark("boom".into());
    assert!(server.panes[0].broken.is());
    server.repair();
    let pane = &server.panes[0];
    assert!(!pane.broken.is(), "a new engine in its place");
    assert_eq!(pane.failure.as_deref(), Some("boom"));
    // The new engine is blank: what shows now is what the program wrote after the repair, on its SIGWINCH.
    let words = wait_said(&server, 0, "redrawn");
    assert!(words.contains("redrawn"), "{words:?}");
    assert!(!words.contains("ready"), "the old text is not in the new engine: {words:?}");
    assert!(server.dirty, "a frame is asked for");
}

#[test]
fn a_time_shown_ticks_when_its_text_changes() {
    assert_eq!(tick(12, 0), Duration::from_secs(1));
    assert_eq!(tick(12, 250_000_000), Duration::from_millis(750));
    // 2m: 2m30s now, 3m in 30s.
    assert_eq!(tick(150, 0), Duration::from_secs(30));
    assert_eq!(tick(3600 + 5 * 60 + 59, 900_000_000), Duration::from_millis(100));
    assert!(tick(59, 999_999_999) > Duration::ZERO, "never at once");
}

#[test]
fn the_clock_ticks_for_a_time_drawn_only() {
    let state = tempfile::tempdir().unwrap();
    let tabs = vec![TabSpec { title: "1".into(), panes: vec![sleeper("dev-a")], side: None }];
    let mut server = built(state.path(), &["dev-a"], tabs);
    // Its program says its state: its time is since then.
    server.panes[0].state = Some(State::Idle);
    server.panes[0].since = Some(now() - 5);
    assert!(server.compose());
    let next = server.pacing.clock.expect("seconds shown").saturating_duration_since(Instant::now());
    assert!(next <= Duration::from_secs(1), "{next:?}");
    // Too narrow for the time: no wake-up each second.
    server.size = (12, 24);
    server.layout();
    assert!(server.compose());
    assert_eq!(server.pacing.clock, None);
    // Under a minute no more: a wake-up a minute, whatever the room.
    server.panes[0].since = Some(now() - 600);
    assert!(server.compose());
    assert!(server.pacing.clock.is_some());
}

#[test]
fn panes_as_wide_share_a_form_and_the_dashboard_counts() {
    let state = tempfile::tempdir().unwrap();
    let tabs = vec![TabSpec { title: "1".into(), panes: vec![sleeper("dev-a"), sleeper("dev-b")], side: None }];
    let mut server = built(state.path(), &["dev-a", "dev-b"], tabs);
    let rich = board::MemberLook {
        model: Some("Sonnet".into()),
        effort: Some("xhigh".into()),
        context: Some(100),
        compactable: true,
        ..Default::default()
    };
    server.looks.insert("dev-a".into(), rich);
    server.looks.insert("dev-b".into(), board::MemberLook { model: Some("Opus".into()), ..Default::default() });

    // Narrow enough that dev-a's right part cannot show all of it.
    server.size = (60, 24);
    server.layout();
    let panes: Vec<&Pane> = server.panes.iter().collect();
    let engines: Vec<_> = panes.iter().map(|pane| pane.engine.lock().unwrap()).collect();
    let mut views: Vec<View<'_>> = panes
        .iter()
        .zip(&engines)
        .map(|(pane, engine)| View {
            id: &pane.id,
            engine: engine.as_ref(),
            area: pane.area,
            header: server.header(pane, None, None, now()),
            focused: false,
            selection: None,
            bare: false,
        })
        .collect();
    let width = views[0].area.width;
    assert_eq!(views[1].area.width, width);
    let alone: Vec<usize> = views.iter().map(|view| chrome::form(&view.header, width)).collect();
    assert!(alone[0] > alone[1], "{alone:?}: dev-a needs a poorer form");
    common_forms(&mut views);
    assert_eq!([views[0].header.form, views[1].header.form], [alone[0]; 2]);
    // A header with nothing on its right yet fits whole: it holds no one back.
    views[1].header = chrome::Header { model: None, ..views[1].header };
    views[0].header.form = 0;
    common_forms(&mut views);
    assert_eq!(views[0].header.form, alone[0]);
    drop(views);
    drop(engines);
    // The dashboard's border counts the members by their programs' states.
    server.panes[0].state = Some(State::Working);
    server.panes[1].state = Some(State::Blocked);
    assert_eq!(server.counts(), chrome::Hint::Counts { working: 1, waiting: 1, error: 0, idle: 0 });
    // A program that says nothing: as the dashboard saw it; the program, once it says, first.
    server.panes[1].state = None;
    assert_eq!(server.counts(), chrome::Hint::Counts { working: 1, waiting: 0, error: 0, idle: 0 }, "seen nowhere yet");
    server.looks.get_mut("dev-b").unwrap().state = Some(look::State::Idle);
    assert_eq!(server.counts(), chrome::Hint::Counts { working: 1, waiting: 0, error: 0, idle: 1 });
    server.looks.get_mut("dev-a").unwrap().state = Some(look::State::Idle);
    assert_eq!(server.counts(), chrome::Hint::Counts { working: 1, waiting: 0, error: 0, idle: 1 }, "its program says");
}

#[test]
fn a_command_running_shows_as_on_its_card() {
    let state = tempfile::tempdir().unwrap();
    let tabs = vec![TabSpec { title: "1".into(), panes: vec![sleeper("dev-a")], side: None }];
    let mut server = built(state.path(), &["dev-a"], tabs);
    let now = now();
    let running = board::MemberLook {
        state: Some(look::State::Idle),
        shell: true,
        since: Some(now as i64 - 600),
        ..Default::default()
    };
    server.looks.insert("dev-a".into(), running);
    server.panes[0].since = Some(now - 5);
    let header = |server: &Server| {
        let header = server.header(&server.panes[0], None, None, now);
        (header.state, header.shell, header.since)
    };
    // At rest by its program, the command running by `claude agents`: since the command started.
    server.panes[0].state = Some(State::Idle);
    assert_eq!(header(&server), (look::State::Idle, true, Some(600)));
    server.panes[0].state = Some(State::Done);
    assert_eq!(header(&server), (look::State::Idle, true, Some(600)));
    // Its program says nothing: the same.
    server.panes[0].state = None;
    assert!(header(&server).1);
    // At work, or waiting: its program wins.
    server.panes[0].state = Some(State::Working);
    assert_eq!(header(&server), (look::State::Working, false, Some(5)));
    server.panes[0].state = Some(State::Blocked);
    assert_eq!(header(&server), (look::State::Waiting, false, Some(5)));
    // The dashboard's border counts it at rest, as its cards do.
    server.panes[0].state = Some(State::Idle);
    assert_eq!(server.counts(), chrome::Hint::Counts { working: 0, waiting: 0, error: 0, idle: 1 });
}

#[test]
fn a_zoomed_pane_takes_its_tabs_room() {
    let state = tempfile::tempdir().unwrap();
    let panes = vec![sleeper("dev-a"), sleeper("dev-b")];
    let mut server = built(state.path(), &["dev-a", "dev-b"], vec![TabSpec { title: "1".into(), panes, side: None }]);
    let (a, b) = (id_of(&server, "dev-a"), id_of(&server, "dev-b"));
    let grid = server.pane(&b).unwrap().area;
    assert!(grid.width > 0 && grid.width < 80);
    server.zoom(&a);
    assert_eq!(server.pane(&a).unwrap().area, Rect { x: 0, y: 0, width: 80, height: 23 }, "the bar left out");
    assert_eq!(server.pane(&b).unwrap().area, Rect::default(), "hidden");
    assert_eq!(server.pane(&b).unwrap().size, (grid.width as u16 - 2, grid.height as u16 - 2), "not resized");
    assert_eq!(server.shown_ids(), [a.as_str()]);
    assert_eq!(server.focus.as_deref(), Some(a.as_str()));
    // Again: the grid.
    server.zoom(&a);
    assert_eq!(server.pane(&b).unwrap().area, grid);
    // Another pane of the tab focused: the grid too, as in tmux.
    server.zoom(&a);
    server.focus_on(&b);
    assert_eq!(server.pane(&b).unwrap().area, grid);
    assert_eq!(server.tabs[0].zoomed, None);
    // Arranged again (any change from the menu): kept while the pane stays in its tab, else the grid.
    server.zoom(&a);
    let arrange = |tabs: &[(&str, &[&str])]| Request::Arrange {
        tabs: tabs
            .iter()
            .map(|(title, names)| (title.to_string(), names.iter().map(|n| n.to_string()).collect()))
            .collect(),
        columns: 3,
        focus: false,
    };
    assert!(matches!(server.request(arrange(&[("1", &["dev-a", "dev-b"])])), Reply::Ok(_)));
    assert_eq!(server.tabs[0].zoomed.as_deref(), Some(a.as_str()));
    assert_eq!(server.pane(&b).unwrap().area, Rect::default());
    assert!(matches!(server.request(arrange(&[("1", &["dev-b"]), ("2", &["dev-a"])])), Reply::Ok(_)));
    assert!(server.tabs.iter().all(|tab| tab.zoomed.is_none()));
    assert!(matches!(server.request(arrange(&[("1", &["dev-a", "dev-b"])])), Reply::Ok(_)));
    // The zoomed pane gone: the grid.
    server.zoom(&b);
    server.close_pane(&b);
    assert_eq!(server.tabs[0].zoomed, None);
    assert_eq!(server.shown_ids(), [a]);
}

#[test]
fn next_pane_keeps_the_zoom_and_zoom_gives_the_grid() {
    let state = tempfile::tempdir().unwrap();
    let panes = vec![sleeper("dev-a"), sleeper("dev-b"), sleeper("dev-c")];
    let mut server =
        built(state.path(), &["dev-a", "dev-b", "dev-c"], vec![TabSpec { title: "1".into(), panes, side: None }]);
    let (a, b, c) = (id_of(&server, "dev-a"), id_of(&server, "dev-b"), id_of(&server, "dev-c"));
    // The grid: the focus goes on, the grid stays.
    server.focus_on(&a);
    server.act(Action::NextPane);
    assert_eq!((server.focus.as_deref(), server.tabs[0].zoomed.as_deref()), (Some(b.as_str()), None));
    // Zoomed: the next member takes the room, the others hidden.
    server.zoom(&b);
    server.act(Action::NextPane);
    assert_eq!((server.focus.as_deref(), server.tabs[0].zoomed.as_deref()), (Some(c.as_str()), Some(c.as_str())));
    assert_eq!(server.shown_ids(), [c.as_str()]);
    assert_eq!(server.pane(&c).unwrap().area, Rect { x: 0, y: 0, width: 80, height: 23 });
    assert_eq!(server.pane(&b).unwrap().area, Rect::default());
    // Round the tab, zoom kept.
    server.act(Action::NextPane);
    assert_eq!(server.tabs[0].zoomed.as_deref(), Some(a.as_str()));
    // ⌥z gives the grid back, ⤡ too.
    server.act(Action::Zoom);
    assert_eq!(server.tabs[0].zoomed, None);
    assert_eq!(server.focus.as_deref(), Some(a.as_str()));
    server.zoom(&a);
    server.part_click(&a, chrome::Part::Zoom);
    assert_eq!(server.tabs[0].zoomed, None);
    // A tab of one member: nothing to go to, the zoom stays.
    let alone = vec![sleeper("dev-a")];
    let state = tempfile::tempdir().unwrap();
    let mut server = built(state.path(), &["dev-a"], vec![TabSpec { title: "1".into(), panes: alone, side: None }]);
    let a = id_of(&server, "dev-a");
    server.zoom(&a);
    server.act(Action::NextPane);
    assert_eq!(server.tabs[0].zoomed.as_deref(), Some(a.as_str()));
}

/// Two members at rest for `rested` seconds, context `context` %, the team compacting from 70 %.
fn resting(state: &Path, rested: u64, context: u8) -> Server {
    let panes = vec![sleeper("dev-a"), sleeper("dev-b")];
    let mut server = built(state, &["dev-a", "dev-b"], vec![TabSpec { title: "1".into(), panes, side: None }]);
    server.auto.auto_compact = Some(70);
    server.auto.auto_idle = 900;
    for (i, name) in ["dev-a", "dev-b"].into_iter().enumerate() {
        server.panes[i].state = Some(State::Idle);
        server.panes[i].since = Some(now() - rested);
        let look = board::MemberLook { context: Some(context), ..Default::default() };
        server.looks.insert(name.into(), look);
    }
    server
}

fn due(server: &Server) -> Vec<String> {
    server.compaction_due(now()).0.into_iter().map(|(member, _, _)| member).collect()
}

#[test]
fn a_member_at_rest_with_a_high_context_is_compacted_once_off_screen() {
    let state = tempfile::tempdir().unwrap();
    let mut server = resting(state.path(), 901, 71);
    // No client attached: nothing on any screen.
    assert_eq!(due(&server), ["dev-a", "dev-b"]);
    let (_, wait) = server.compaction_due(now());
    assert_eq!(wait, None);

    // Not long enough: the wait is what is left of the shortest rest.
    let short = resting(state.path(), 600, 71);
    let (asked, wait) = short.compaction_due(now());
    assert!(asked.is_empty());
    assert!(wait.is_some_and(|wait| (298..=300).contains(&wait.as_secs())), "{wait:?}");

    // The context below the share, the compaction off, a command running, not at rest: not asked.
    assert!(due(&resting(state.path(), 901, 69)).is_empty());
    let mut off = resting(state.path(), 901, 90);
    off.auto.auto_compact = None;
    assert!(due(&off).is_empty());
    let mut busy = resting(state.path(), 901, 90);
    busy.looks.get_mut("dev-a").unwrap().shell = true;
    assert_eq!(due(&busy), ["dev-b"]);
    let state_path = state.path();
    for state in [State::Working, State::Blocked, State::Error] {
        let mut other = resting(state_path, 901, 90);
        other.panes[0].state = Some(state);
        assert_eq!(due(&other), ["dev-b"], "{state:?}");
    }
    let mut unknown = resting(state.path(), 901, 90);
    unknown.looks.get_mut("dev-a").unwrap().context = None;
    assert_eq!(due(&unknown), ["dev-b"]);

    // On a client's screen: the tab shown, but the zoomed pane alone.
    let _client = attached(&mut server);
    assert!(due(&server).is_empty(), "the tab is shown");
    let b = id_of(&server, "dev-b");
    server.zoom(&b);
    assert_eq!(due(&server), ["dev-a"], "a hidden pane of a zoomed tab");

    // Asked: not again in this rest, but after a new turn.
    server.plan_compaction(now());
    assert!(due(&server).is_empty());
    assert_eq!(server.auto.asked.get("dev-a").map(|asked| Some(asked.since)), Some(server.panes[0].since));
    server.panes[0].since = Some(now() - 1000);
    assert_eq!(due(&server), ["dev-a"], "a new rest");
}

fn outcome(id: &str, done: bool, skip: Option<&str>, error: Option<&str>) -> bridge::Compaction {
    bridge::Compaction {
        id: id.into(),
        done,
        skip: skip.map(String::from),
        error: error.map(String::from),
        auto: true,
        ..Default::default()
    }
}

/// `dev-a` asked `ago` seconds back, after `failures` requests that failed, its id (7) written.
fn asked_ago(server: &mut Server, ago: u64, failures: u8) -> u64 {
    let since = server.panes[0].since.unwrap();
    let at = Instant::now() - Duration::from_secs(ago);
    server.auto.asked.insert("dev-a".into(), Asked { since, id: Some("7".into()), at, ended: false, failures });
    since
}

fn within(left: Option<Duration>, low: u64, high: u64) -> bool {
    left.is_some_and(|left| (low..=high).contains(&left.as_secs()))
}

#[test]
fn a_request_refused_or_unanswered_is_asked_again_later() {
    let state = tempfile::tempdir().unwrap();
    let mut server = resting(state.path(), 1000, 80);
    let since = server.panes[0].since.unwrap();
    let answer = |server: &mut Server, answer: Option<bridge::Compaction>| {
        server.auto.outcomes.clear();
        if let Some(answer) = answer {
            server.auto.outcomes.insert("dev-a".into(), answer);
        }
    };

    // Not asked: now.
    assert_eq!(server.renewal("dev-a", since), Some(Duration::ZERO));
    // Done: the rest is closed, whenever.
    asked_ago(&mut server, 400, 0);
    answer(&mut server, Some(outcome("7", true, None, None)));
    assert_eq!(server.renewal("dev-a", since), None);
    // A subagent or a teammate at work: again 5 minutes after, as long as it lasts, failures or not.
    answer(&mut server, Some(outcome("7", false, Some(bridge::SKIP_HELPERS), None)));
    for (failures, skip) in [(0, bridge::SKIP_HELPERS), (1, bridge::SKIP_BUSY), (200, bridge::SKIP_HELPERS)] {
        answer(&mut server, Some(outcome("7", false, Some(skip), None)));
        asked_ago(&mut server, 100, failures);
        assert!(within(server.renewal("dev-a", since), 199, 200), "{failures}");
        assert!(!due(&server).contains(&"dev-a".to_string()));
        assert!(server.compaction_due(now()).1.is_some_and(|wait| wait.as_secs() <= 200));
        asked_ago(&mut server, 301, failures);
        assert_eq!(server.renewal("dev-a", since), Some(Duration::ZERO), "{failures}");
    }
    assert_eq!(due(&server), ["dev-a", "dev-b"]);
    // No answer yet, or in progress, for a little while: the rest stays open, and the one more try waits.
    for pending in [None, Some(outcome("7", false, None, None))] {
        answer(&mut server, pending);
        asked_ago(&mut server, 10, 0);
        assert!(within(server.renewal("dev-a", since), 289, 290));
        asked_ago(&mut server, 10, 1);
        assert!(within(server.renewal("dev-a", since), 19, 20), "open, watched, not asked");
        assert!(!due(&server).contains(&"dev-a".to_string()));
    }
    // No answer for its id (lapsed; the answer is an older one's), an error, another skip: once more, then closed.
    for said in [
        None,
        Some(outcome("6", true, None, None)),
        Some(outcome("7", false, None, Some("x"))),
        Some(outcome("7", false, Some("precompact"), None)),
    ] {
        answer(&mut server, said);
        asked_ago(&mut server, 100, 0);
        assert!(server.renewal("dev-a", since).is_some_and(|left| left > Duration::ZERO));
        asked_ago(&mut server, 301, 0);
        assert_eq!(server.renewal("dev-a", since), Some(Duration::ZERO));
        asked_ago(&mut server, 301, 1);
        assert_eq!(server.renewal("dev-a", since), None);
    }
    // A new rest starts afresh.
    server.panes[0].since = Some(now() - 950);
    assert_eq!(due(&server), ["dev-a", "dev-b"]);
}

#[test]
fn a_state_a_program_tells_makes_the_compaction_look_again() {
    let state = tempfile::tempdir().unwrap();
    let mut server = resting(state.path(), 10, 80);
    server.look_again = false;
    // Nothing to take: nothing to look at again.
    server.take_relays();
    assert!(!server.look_again);
    // Its state, as `frame` takes it (even with no client): the next turn of the loop looks.
    let mut replies = Vec::new();
    server.panes[0]
        .with(|engine| engine.feed(b"\x1b]7501;?\x1b\\\x1b]7501;state=idle:app=claude-code\x1b\\", &mut replies));
    server.take_relays();
    assert!(server.look_again);
    assert!(server.panes[0].since.is_some());
}

/// Feeds pane 0's program the statuses `says` (`working`, `idle`…), as it would write them, and takes them.
fn tells(server: &mut Server, says: &[&str]) {
    let mut replies = Vec::new();
    for state in says {
        let osc = format!("\x1b]7501;state={state}:app=claude-code\x1b\\");
        server.panes[0].with(|engine| engine.feed(osc.as_bytes(), &mut replies));
        server.take_relays();
    }
}

/// A tab of `names` in « focus » on an 80 × 24 screen, a client attached.
fn focused(state: &Path, names: &[&str]) -> (Server, UnixStream) {
    let panes = names.iter().map(|name| sleeper(name)).collect();
    let mut server = built(state, names, vec![TabSpec { title: "1".into(), panes, side: None }]);
    let client = attached(&mut server);
    server.focus_layout = true;
    server.layout();
    (server, client)
}

fn places(server: &Server) -> Places {
    server.tabs[0].focus.as_ref().expect("in focus").places().clone()
}

#[test]
fn a_tab_in_focus_has_the_active_member_on_the_left_and_the_best_placed_on_the_right() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, _client) = focused(state.path(), &["dev-a", "dev-b", "dev-c", "dev-d"]);
    let id = |server: &Server, name: &str| id_of(server, name);
    let (a, b, c, d) = (id(&server, "dev-a"), id(&server, "dev-b"), id(&server, "dev-c"), id(&server, "dev-d"));
    // 80 × 23: two panes on the right; the active one (the first) on the left; a card for the last.
    let at = places(&server);
    assert_eq!(at.left.as_deref(), Some(a.as_str()));
    assert_eq!(at.right.len(), 2);
    assert_eq!(at.others.len(), 1);
    assert_eq!(server.shown_ids().len(), 3);
    let area = |server: &Server, id: &str| server.pane(id).unwrap().area;
    let left = area(&server, &a);
    assert!(left.x == 0 && left.height == 23 && left.width < 80);
    for id in &at.right {
        let right = area(&server, id);
        assert!(right.x == left.width && right.x + right.width == 80, "{right:?}");
    }
    assert_eq!(area(&server, &at.others[0]), Rect::default(), "a card is hidden");
    assert!(server.compose());

    // The focus on a card: it comes to the left.
    let card = at.others[0].clone();
    server.focus_on(&card);
    assert_eq!(places(&server).left.as_deref(), Some(card.as_str()));
    assert!(server.shown_ids().contains(&card));
    // The focus on a pane on the right: only the focus, the left stays.
    let right = places(&server).right[0].clone();
    server.focus_on(&right);
    assert_eq!(server.focus.as_deref(), Some(right.as_str()));
    assert_eq!(places(&server).left.as_deref(), Some(card.as_str()));

    // ⌥n: the next member of the tab, in reading order, to the left, with the focus.
    server.tabs[0].focus = None;
    server.focus_on(&a);
    server.layout();
    assert_eq!(places(&server).left.as_deref(), Some(a.as_str()));
    server.act(Action::NextPane);
    assert_eq!((places(&server).left.as_deref(), server.focus.as_deref()), (Some(b.as_str()), Some(b.as_str())));
    server.act(Action::NextPane);
    assert_eq!(places(&server).left.as_deref(), Some(c.as_str()));
    assert_eq!(server.focus.as_deref(), Some(c.as_str()));
    assert!(server.shown_ids().len() == 3 && server.shown_ids().contains(&c));
    let _ = d;

    // ⌥l: the grid for this tab, and back; zoom keeps its way.
    server.act(Action::ToggleLayout);
    assert!(server.tabs[0].focus.is_none());
    assert_eq!(server.shown_ids().len(), 4);
    server.act(Action::ToggleLayout);
    assert!(server.tabs[0].focus.is_some());
    assert_eq!(places(&server).left.as_deref(), Some(c.as_str()), "the active one on the left again");
    let zoomed = c.clone();
    server.zoom(&zoomed);
    assert_eq!(server.shown_ids(), std::slice::from_ref(&c));
    server.zoom(&zoomed);
    // The client leaves: the choice of ⌥l goes with it.
    server.act(Action::ToggleLayout);
    assert!(server.tabs[0].focus.is_none());
    let _ = server.drop_client(None);
    assert!(server.tabs[0].mode.is_none() && server.tabs[0].focus.is_some());

    // A screen too narrow for it: the grid.
    let _client = attached(&mut server);
    server.resize((70, 24));
    assert!(server.tabs[0].focus.is_none());
    assert_eq!(server.shown_ids().len(), 4);
    server.resize((100, 30));
    assert!(server.tabs[0].focus.is_some());
    // The team in its grid: ⌥l asks for the focus.
    server.focus_layout = false;
    server.layout();
    assert!(server.tabs[0].focus.is_none());
    server.act(Action::ToggleLayout);
    assert!(server.tabs[0].focus.is_some());
}

#[test]
fn the_members_left_out_are_cards_that_a_click_brings_to_the_left() {
    let state = tempfile::tempdir().unwrap();
    let names = ["dev-a", "dev-b", "dev-c", "dev-d", "dev-e"];
    let (mut server, _client) = focused(state.path(), &names);
    let at = places(&server);
    assert_eq!((at.right.len(), at.others.len()), (2, 2));
    // The same room for the panes and the cards: every member is a pane shown or a card, none lost, none twice.
    let rect = server.cards_rect().expect("a frame for the cards");
    let mut seen: Vec<String> = server.shown_ids();
    seen.extend(server.cards.ids.clone());
    seen.sort();
    let mut all: Vec<String> = names.iter().map(|name| id_of(&server, name)).collect();
    all.sort();
    assert_eq!(seen, all);
    let (width, height) = server.tab_room();
    assert!(rect.x + rect.width <= width && rect.y + rect.height <= height, "{rect:?}");
    for id in server.shown_ids() {
        let pane = server.pane(&id).unwrap().area;
        assert!(pane.x + pane.width <= width && pane.y + pane.height <= height);
        let apart = pane.x + pane.width <= rect.x || rect.x + rect.width <= pane.x || pane.y + pane.height <= rect.y;
        assert!(apart, "{pane:?} over the cards {rect:?}");
    }
    // Drawn: the frame titled, the names of the cards.
    assert!(server.compose());
    let rows: Vec<String> = (0..height).map(|y| server.screen.borrow().canvas().row(y)).collect();
    let text = rows.join("\n");
    assert!(text.contains(&cards::title()), "{text}");
    // Two by two in this narrow screen, the names cut to what holds: the cards' capsules are there.
    assert_eq!(text.matches('\u{2577}').count() + text.matches('\u{257B}').count(), 2, "{text}");
    // A card that begins to wait gives no notice: it is in view, red.
    let waiting = server.cards.data[0].name.clone();
    let looks = BTreeMap::from([(
        waiting,
        board::MemberLook { state: Some(look::State::Waiting), since: Some(now() as i64), ..Default::default() },
    )]);
    server.message(Msg::Looks(looks));
    assert!(server.notices.notice.is_none());
    server.refresh_cards();
    assert_eq!(server.cards.data.iter().filter(|card| card.state == look::State::Waiting).count(), 1, "red");
    // A click on a card brings its member to the left, the left one takes a place on the right.
    let wide = width >= layout::FOCUS_WIDE;
    let placed = cards::place(chrome::inside(rect), &server.cards.data, wide);
    let (index, card) = placed.shown[0];
    let member = server.cards.ids[index].clone();
    let left = at.left.clone().unwrap();
    let (col, row) = (card.x as u16 + 1, card.y as u16);
    server.mouse(&Mouse { kind: MouseKind::Down(Button::Left), col, row, mods: Mods::NONE });
    server.mouse(&Mouse { kind: MouseKind::Up(Button::Left), col, row, mods: Mods::NONE });
    assert_eq!(places(&server).left.as_deref(), Some(member.as_str()));
    assert_eq!(server.focus.as_deref(), Some(member.as_str()));
    assert!(places(&server).right.contains(&left) || places(&server).others.contains(&left));
    assert_eq!(server.cards.ids.len(), 2, "the cards made again");
    // A press in the frame but on no card: nothing, and nothing leaks to a pane.
    let before = places(&server);
    let corner = (rect.x as u16, rect.y as u16);
    server.mouse(&Mouse { kind: MouseKind::Down(Button::Left), col: corner.0, row: corner.1, mods: Mods::NONE });
    assert_eq!(places(&server), before);
    // Zoomed or in the grid: no cards.
    server.act(Action::ToggleLayout);
    assert!(server.cards_rect().is_none());
}

#[test]
fn the_bars_layout_button_follows_the_tab_and_toggles_it() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, _client) = focused(state.path(), &["dev-a", "dev-b", "dev-c"]);
    assert_eq!(server.switch(0), Some(chrome::Switch::ToGrid));
    let button = |server: &mut Server| {
        assert!(server.compose());
        server.zones.iter().find(|(_, target)| *target == Target::Layout).map(|(zone, _)| *zone)
    };
    let at = button(&mut server).expect("the button is drawn");
    let click = |server: &mut Server, at: Rect| {
        let (col, row) = (at.x as u16 + 1, at.y as u16);
        server.mouse(&Mouse { kind: MouseKind::Down(Button::Left), col, row, mods: Mods::NONE });
        server.mouse(&Mouse { kind: MouseKind::Up(Button::Left), col, row, mods: Mods::NONE });
    };
    click(&mut server, at);
    assert!(server.tabs[0].focus.is_none());
    assert_eq!(server.switch(0), Some(chrome::Switch::ToFocus));
    let at = button(&mut server).expect("the button, the other way");
    click(&mut server, at);
    assert!(server.tabs[0].focus.is_some());
    // A screen too small for it: no button.
    server.resize((70, 24));
    assert_eq!(server.switch(0), None);
    assert!(button(&mut server).is_none());
}

#[test]
fn a_double_click_on_the_header_of_a_pane_on_the_right_brings_it_to_the_left() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, _client) = focused(state.path(), &["dev-a", "dev-b", "dev-c"]);
    let left = places(&server).left.clone().unwrap();
    let right = places(&server).right[0].clone();
    assert!(server.compose());
    let area = server.pane(&right).unwrap().area;
    // On the border row, off the header's parts.
    let at = (area.x + area.width - 1, area.y);
    let click = |server: &mut Server| {
        let (col, row) = (at.0 as u16, at.1 as u16);
        server.mouse(&Mouse { kind: MouseKind::Down(Button::Left), col, row, mods: Mods::NONE });
        server.mouse(&Mouse { kind: MouseKind::Up(Button::Left), col, row, mods: Mods::NONE });
    };
    // One press: the focus, and the left stays.
    click(&mut server);
    assert_eq!((server.focus.as_deref(), places(&server).left.as_deref()), (Some(right.as_str()), Some(left.as_str())));
    // Two: it is the left, the other takes its place.
    click(&mut server);
    assert_eq!(places(&server).left.as_deref(), Some(right.as_str()));
    assert!(places(&server).right.contains(&left));
    assert_eq!(server.focus.as_deref(), Some(right.as_str()));
    // Apart in time: two single presses.
    let other = places(&server).right[0].clone();
    server.pointer.header_click = None;
    let area = server.pane(&other).unwrap().area;
    let (col, row) = ((area.x + area.width - 1) as u16, area.y as u16);
    server.mouse(&Mouse { kind: MouseKind::Down(Button::Left), col, row, mods: Mods::NONE });
    server.pointer.header_click.as_mut().unwrap().0 -= DOUBLE_CLICK;
    server.mouse(&Mouse { kind: MouseKind::Down(Button::Left), col, row, mods: Mods::NONE });
    assert_eq!(places(&server).left.as_deref(), Some(right.as_str()));
}

#[test]
fn nothing_moves_in_a_tab_in_focus_under_the_pointer_or_after_a_key_or_a_click() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, _client) = focused(state.path(), &["dev-a", "dev-b", "dev-c", "dev-d"]);
    let before = places(&server);
    let epoch = now() as i64;
    let tick = Instant::now();
    assert!(!server.holding(tick).0);
    // The pointer: held only over a pane on the right, and for five seconds after it moved.
    let right = server.pane(&before.right[0]).unwrap().area;
    let left = server.pane(before.left.as_ref().unwrap()).unwrap().area;
    server.pointer.at = Some((right.x + 2, right.y + 1));
    server.pointer.moved = Some(tick);
    let (held, end) = server.holding(tick);
    assert!(held && end.is_some_and(|end| end > tick + Duration::from_secs(4)));
    server.pointer.moved = Some(tick - Duration::from_secs(6));
    assert!(!server.holding(tick).0, "it has not moved for a while");
    server.pointer.moved = Some(tick);
    server.pointer.at = Some((left.x + 2, left.y + 1));
    assert!(!server.holding(tick).0, "over the left pane, the places may move");
    server.pointer.at = Some((5, 23));
    assert!(!server.holding(tick).0, "on the bar too");
    // A key, a click: a second.
    server.pointer.at = None;
    server.pointer.last_key = Some(tick);
    let (held, end) = server.holding(tick);
    assert!(held && end.is_some_and(|end| end > tick && end <= tick + Duration::from_secs(1)));
    server.pointer.last_key = Some(tick - Duration::from_secs(2));
    server.pointer.last_click = Some(tick);
    assert!(server.holding(tick).0);
    server.pointer.last_click = None;
    server.pointer.last_key = None;
    // A member that waits or failed goes past the pane's minute: it comes in as soon as nothing holds the tab. One
    // at work respects it: the server wakes for the end of the minute.
    let d = id_of(&server, "dev-d");
    let look = |state, since| board::MemberLook { state: Some(state), since: Some(since), ..Default::default() };
    server.looks.insert("dev-d".into(), look(look::State::Waiting, epoch));
    server.pointer.last_key = Some(Instant::now());
    server.update_focus();
    assert_eq!(places(&server), before, "held");
    assert!(server.focus_wake.is_some(), "woken when the hold ends");
    server.pointer.last_key = None;
    server.update_focus();
    assert!(places(&server).right.contains(&d), "{:?}", places(&server));
    assert!(server.shown_ids().contains(&d));
    assert!(server.focus_wake.is_none(), "nothing more is due");

    // At work: held, then the minute of the pane it would replace.
    let (mut server, _client) = focused(state.path(), &["dev-a", "dev-b", "dev-c", "dev-d"]);
    let d = id_of(&server, "dev-d");
    // Two minutes at work already: its own 30 seconds are past.
    server.looks.insert("dev-d".into(), look(look::State::Working, epoch - 120));
    server.pointer.last_key = Some(Instant::now());
    server.update_focus();
    assert_eq!(places(&server), before, "held");
    server.pointer.last_key = None;
    server.update_focus();
    assert_eq!(places(&server), before, "the minute is not over");
    let wake = server.focus_wake.expect("a place comes due");
    let left = wake.saturating_duration_since(Instant::now()).as_secs();
    assert!((55..=60).contains(&left), "{left}");
    // A minute later its turn has come: it is on the right.
    let later = Instant::now() + focus::STAY_AT_LEAST + Duration::from_secs(1);
    let seen = server.focus_seen(0, later);
    let focus = server.tabs[0].focus.as_mut().unwrap();
    assert!(focus.update(&seen, 2, later, false));
    assert!(focus.places().right.contains(&d), "{:?}", focus.places());
    // The pane with the focus is never lost: dropped to a card, it comes back to the left.
    assert!(server.shown_ids().contains(server.focus.as_ref().unwrap()));
}

/// What a turn of the loop does after the messages: what looks again after them, as `run` does it.
fn end_of_turn(server: &mut Server) {
    let again = std::mem::take(&mut server.look_again);
    if again || server.auto.compaction.is_some_and(|at| at <= Instant::now()) {
        server.auto_compaction();
    }
    if again || server.focus_wake.is_some_and(|at| at <= Instant::now()) {
        server.update_focus();
    }
    if std::mem::take(&mut server.cards.stale) {
        server.refresh_cards();
    }
}

#[test]
fn a_mouse_move_that_lights_nothing_asks_for_no_frame() {
    for focus in [false, true] {
        let state = tempfile::tempdir().unwrap();
        let names = ["dev-a", "dev-b", "dev-c", "dev-d", "dev-e"];
        let panes = names.iter().map(|name| sleeper(name)).collect();
        let mut server = built(state.path(), &names, vec![TabSpec { title: "1".into(), panes, side: None }]);
        let _client = attached(&mut server);
        server.focus_layout = focus;
        server.layout();
        assert_eq!(server.tabs[0].focus.is_some(), focus);
        assert!(server.compose());
        end_of_turn(&mut server);
        server.dirty = false;
        // Over a pane's text, over the right panes and the cards, and on the bar: the same place again and again.
        for (col, row) in [(5u16, 5u16), (70, 3), (70, 20), (3, 23)] {
            let event = input::Event::Mouse(Mouse { kind: MouseKind::Moved, col, row, mods: Mods::NONE });
            // The first move may light a part or a button; a frame for that is right.
            server.message(Msg::Client("c1".into(), ClientMsg::Input(event.clone())));
            end_of_turn(&mut server);
            server.dirty = false;
            for _ in 0..5 {
                server.message(Msg::Client("c1".into(), ClientMsg::Input(event.clone())));
                end_of_turn(&mut server);
                assert!(!server.dirty, "focus {focus}: at {col},{row}: the same place again asked for a frame");
            }
        }
    }
}

#[test]
fn a_client_with_another_theme_has_the_whole_screen_drawn_again() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, _client) = focused(state.path(), &["dev-a", "dev-b"]);
    assert!(server.compose());
    assert_eq!(server.screen.borrow().canvas().width(), 80);
    // The same theme as the client before: the cells kept are good.
    assert!(!server.theme(false));
    assert_eq!(server.screen.borrow().canvas().width(), 80);
    // Another: the next frame is drawn whole.
    server.dirty = false;
    assert!(server.theme(true) && server.light);
    assert!(server.dirty);
    assert!(server.compose());
    assert_eq!(server.screen.borrow().canvas().width(), 80);
    assert!(!server.theme(true));
    assert!(server.theme(false) && !server.light);
    // A background that says light: luminance over a half.
    assert!(look::light_background((250, 250, 250)) && !look::light_background((40, 44, 52)));
}

#[test]
fn a_member_compacting_on_its_own_stays_at_rest() {
    let state = tempfile::tempdir().unwrap();
    let mut server = resting(state.path(), 1000, 80);
    server.auto.auto_idle = 0;
    let mut replies = Vec::new();
    server.panes[0].with(|engine| engine.feed(b"\x1b]7501;?\x1b\\", &mut replies));
    tells(&mut server, &["idle"]);
    let since = server.panes[0].since.unwrap();
    let shown = |server: &Server| server.header(&server.panes[0], None, None, now()).state;
    // dev-a is asked; the writer tells the id back.
    let planned = server.plan_compaction(now());
    assert!(planned.iter().any(|(member, _, _)| member == "dev-a"));
    server.message(Msg::Requested("dev-a".into(), since, Ok("c1".into())));
    assert!(server.compacting("dev-a", Some(since)));

    // Claude Code says `working` for the compaction: nothing changes on the screen, nor the time in the state.
    tells(&mut server, &["working"]);
    assert_eq!((shown(&server), server.panes[0].since), (look::State::Idle, Some(since)));
    assert_eq!(server.panes[0].state, Some(State::Idle));
    assert!(server.panes[0].held.is_some());
    assert_eq!(server.counts(), chrome::Hint::Counts { working: 0, waiting: 0, error: 0, idle: 2 });
    // The dashboard reads the panes: at rest, since the same time.
    let Reply::Ok(list) = server.dispatch(Request::Panes) else { panic!("panes") };
    let list: Vec<PaneInfo> = serde_json::from_value(list).unwrap();
    let info = list.iter().find(|info| info.member == "dev-a").unwrap();
    assert_eq!((info.status.as_deref(), info.since), (Some("idle"), Some(since)));
    // Its end: the state of the last turn again; the compaction is over, a `working` is a turn's.
    tells(&mut server, &["idle"]);
    assert!(server.panes[0].held.is_none() && server.auto.asked["dev-a"].ended);
    assert!(!server.compacting("dev-a", Some(since)));
    assert_eq!((shown(&server), server.panes[0].since), (look::State::Idle, Some(since)));
    tells(&mut server, &["working"]);
    assert_eq!(shown(&server), look::State::Working);

    // Made (`done`), the program said no other state (a message received meanwhile: its turn queued behind the
    // compaction begins right after it): the `working` held is that turn's, since the mod's answer.
    let mut server = resting(state.path(), 1000, 80);
    server.auto.auto_idle = 0;
    server.panes[0].with(|engine| engine.feed(b"\x1b]7501;?\x1b\\", &mut replies));
    tells(&mut server, &["idle"]);
    let since = server.panes[0].since.unwrap();
    server.plan_compaction(now());
    server.message(Msg::Requested("dev-a".into(), since, Ok("c2".into())));
    tells(&mut server, &["working"]);
    assert_eq!((shown(&server), server.panes[0].since), (look::State::Idle, Some(since)));
    let answered = now() as i64 + 3;
    let done = bridge::Compaction { done: true, at: answered, ..outcome("c2", true, None, None) };
    server.message(Msg::Outcomes(BTreeMap::from([("dev-a".to_string(), done)])));
    server.settle_compactions();
    assert!(server.panes[0].held.is_none());
    assert_eq!((shown(&server), server.panes[0].since), (look::State::Working, Some(answered as u64)));
    // It stays at work until the program says the end of the turn.
    server.settle_compactions();
    assert_eq!(shown(&server), look::State::Working);
    tells(&mut server, &["idle"]);
    assert_eq!(shown(&server), look::State::Idle);

    // A compaction on its own: `done` at its end, before the mod answers: the answer changes nothing.
    let mut server = resting(state.path(), 1000, 80);
    server.auto.auto_idle = 0;
    server.panes[0].with(|engine| engine.feed(b"\x1b]7501;?\x1b\\", &mut replies));
    tells(&mut server, &["idle"]);
    let since = server.panes[0].since.unwrap();
    server.plan_compaction(now());
    server.message(Msg::Requested("dev-a".into(), since, Ok("c5".into())));
    tells(&mut server, &["working", "idle"]);
    assert_eq!((shown(&server), server.panes[0].since), (look::State::Idle, Some(since)));
    let done = bridge::Compaction { done: true, at: now() as i64 + 2, ..outcome("c5", true, None, None) };
    server.message(Msg::Outcomes(BTreeMap::from([("dev-a".to_string(), done)])));
    server.settle_compactions();
    assert_eq!((shown(&server), server.panes[0].since), (look::State::Idle, Some(since)));

    // Refused (a helper at work, or an error): not compacting after all, the `working` told since it began.
    for answer in [outcome("c3", false, Some(bridge::SKIP_HELPERS), None), outcome("c3", false, None, Some("x"))] {
        let mut server = resting(state.path(), 1000, 80);
        server.auto.auto_idle = 0;
        server.panes[0].with(|engine| engine.feed(b"\x1b]7501;?\x1b\\", &mut replies));
        tells(&mut server, &["idle"]);
        let since = server.panes[0].since.unwrap();
        server.plan_compaction(now());
        server.message(Msg::Requested("dev-a".into(), since, Ok("c3".into())));
        tells(&mut server, &["working"]);
        let began = server.panes[0].held.as_ref().map(|(_, when)| *when).unwrap();
        assert_eq!(shown(&server), look::State::Idle, "held while the answer is waited for");
        server.message(Msg::Outcomes(BTreeMap::from([("dev-a".to_string(), answer)])));
        server.settle_compactions();
        assert_eq!((shown(&server), server.panes[0].since), (look::State::Working, Some(began)));
    }

    // No answer in time: the same.
    let mut server = resting(state.path(), 1000, 80);
    server.auto.auto_idle = 0;
    server.panes[0].with(|engine| engine.feed(b"\x1b]7501;?\x1b\\", &mut replies));
    tells(&mut server, &["idle"]);
    let since = server.panes[0].since.unwrap();
    server.plan_compaction(now());
    server.message(Msg::Requested("dev-a".into(), since, Ok("c4".into())));
    tells(&mut server, &["working"]);
    server.settle_compactions();
    assert_eq!(shown(&server), look::State::Idle);
    server.auto.asked.get_mut("dev-a").unwrap().at = Instant::now() - COMPACTING;
    server.settle_compactions();
    assert_eq!(shown(&server), look::State::Working);
    // A member never asked: its `working` is a turn at once.
    let mut other = resting(state.path(), 1000, 80);
    other.panes[0].with(|engine| engine.feed(b"\x1b]7501;?\x1b\\", &mut replies));
    tells(&mut other, &["idle", "working"]);
    assert_eq!(other.header(&other.panes[0], None, None, now()).state, look::State::Working);
}

#[test]
fn a_member_is_asked_every_five_minutes_while_helpers_work_and_twice_otherwise() {
    let state = tempfile::tempdir().unwrap();
    let mut server = resting(state.path(), 1000, 80);
    let asked_now = |server: &mut Server| {
        let planned = server.plan_compaction(now());
        planned.into_iter().map(|(member, _, _)| member).filter(|member| member == "dev-a").count()
    };
    let watched = |server: &Server| server.auto.watched.lock().unwrap().contains(&"dev-a".to_string());
    let write = |server: &mut Server, id: &str| {
        let since = server.auto.asked["dev-a"].since;
        server.message(Msg::Requested("dev-a".into(), since, Ok(id.into())));
    };
    let wait_again = |server: &mut Server| server.auto.asked.get_mut("dev-a").unwrap().at = Instant::now() - AUTO_AGAIN;

    assert_eq!(asked_now(&mut server), 1);
    assert_eq!(server.auto.asked["dev-a"].failures, 0);
    assert!(watched(&server));
    assert_eq!(asked_now(&mut server), 0, "not twice in a row");
    assert!(server.auto.compaction.is_some());
    // Three refusals for a subagent or a teammate in a row: asked each time, none counts, always watched.
    for round in 0..3 {
        let id = format!("h{round}");
        write(&mut server, &id);
        assert_eq!(server.auto.asked["dev-a"].id.as_deref(), Some(id.as_str()));
        let refused = BTreeMap::from([("dev-a".to_string(), outcome(&id, false, Some(bridge::SKIP_HELPERS), None))]);
        server.message(Msg::Outcomes(refused));
        assert!(watched(&server), "round {round}");
        assert_eq!(asked_now(&mut server), 0, "5 minutes not passed");
        wait_again(&mut server);
        assert_eq!(asked_now(&mut server), 1, "round {round}");
        assert_eq!(server.auto.asked["dev-a"].failures, 0, "round {round}");
        assert!(watched(&server), "the new request is waited for");
    }
    // Then it goes unanswered: once more 5 minutes after, then closed and no longer watched.
    write(&mut server, "u1");
    server.auto.asked.get_mut("dev-a").unwrap().at = Instant::now() - AUTO_PENDING;
    assert_eq!(asked_now(&mut server), 0, "5 minutes not passed");
    wait_again(&mut server);
    assert_eq!(asked_now(&mut server), 1);
    assert_eq!(server.auto.asked["dev-a"].failures, 1);
    // The last request is waited for, watched and not asked, while its answer may come.
    assert!(watched(&server));
    assert_eq!(asked_now(&mut server), 0);
    write(&mut server, "u2");
    server.auto.asked.get_mut("dev-a").unwrap().at = Instant::now() - AUTO_AGAIN;
    assert_eq!(asked_now(&mut server), 0, "closed");
    assert!(!watched(&server));
    // A stale writer's id (another rest) is not kept.
    let since = server.auto.asked["dev-a"].since;
    server.message(Msg::Requested("dev-a".into(), since - 1, Ok("old".into())));
    assert_eq!(server.auto.asked["dev-a"].id.as_deref(), Some("u2"));
    // The compaction off: nothing asked nor watched.
    server.auto.auto_compact = None;
    assert_eq!(asked_now(&mut server), 0);
    assert!(server.auto.watched.lock().unwrap().is_empty());
    assert!(server.auto.asked.is_empty());
}

#[test]
fn an_automatic_compaction_is_requested_in_the_state_folder() {
    let state = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    ask_compaction(state.path(), &tx, "dev-a", 75, 1234);
    let request: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state.path().join("compact/dev-a")).unwrap()).unwrap();
    assert_eq!((request["auto"].as_bool(), request["percent"].as_u64()), (Some(true), Some(75)));
    let Ok(Msg::Requested(member, since, Ok(id))) = rx.try_recv() else { panic!("the id comes back") };
    assert_eq!((member.as_str(), since, request["id"].as_str()), ("dev-a", 1234, Some(id.as_str())));
}

#[test]
fn a_member_waiting_out_of_sight_is_noticed_and_reached() {
    let state = tempfile::tempdir().unwrap();
    // It asks first, as Claude Code does: an engine relays the states of a program that asked.
    let mut asking = sleeper("dev-b");
    asking.argv = ["/bin/sh", "-c", "printf '\\033]7501;?\\007\\033]7501;state=blocked\\007'; sleep 30"]
        .map(String::from)
        .to_vec();
    let tabs = vec![
        TabSpec { title: "1".into(), panes: vec![sleeper("dev-a")], side: None },
        TabSpec { title: "2".into(), panes: vec![asking], side: None },
    ];
    let mut server = built(state.path(), &["dev-a", "dev-b"], tabs);
    let _client = attached(&mut server);
    let b = id_of(&server, "dev-b");
    let until = Instant::now() + Duration::from_secs(5);
    while server.notices.notice.is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
        server.take_relays();
    }
    let (notice, _) = server.notices.notice.clone().expect("noticed");
    assert_eq!((notice.member.as_str(), notice.tab.as_str()), ("dev-b", "2"));
    assert_eq!(notice.text, chrome::notice_text("dev-b"));
    assert!(server.pane(&b).unwrap().since.is_some());
    // Under a layer: gone, and none comes.
    server.choice = Some(Open::new(chrome::quit_choice(), Purpose::Quit { client: "c1".into() }));
    server.check_notice();
    assert!(server.notices.notice.is_none());
    server.notify(&b);
    assert!(server.notices.notice.is_none(), "none under a layer");
    server.choice = None;
    server.check_notice();
    assert!(server.notices.notice.is_none(), "not shown once the layer goes");
    server.notify(&b);
    assert!(server.notices.notice.is_some());
    // Nor without a client.
    let client = server.client.take();
    server.notices.notice = None;
    server.notify(&b);
    assert!(server.notices.notice.is_none(), "no one to tell");
    server.client = client;
    server.notify(&b);
    // ⌥g: to it, and the notice goes once it is in sight.
    server.go_waiting();
    assert_eq!((server.active, server.focus.as_deref()), (1, Some(b.as_str())));
    server.check_notice();
    assert!(server.notices.notice.is_none());
}

fn status(state: State, message: Option<&str>) -> engine::Status {
    engine::Status { state, message: message.map(String::from), id: None, app: None }
}

/// One member a tab, out of sight but `dev-a`'s, a client attached: the notices are for a client only.
fn four_tabs() -> (tempfile::TempDir, Server, UnixStream) {
    let state = tempfile::tempdir().unwrap();
    let names = ["dev-a", "dev-b", "dev-c", "dev-d"];
    let tabs = names
        .iter()
        .enumerate()
        .map(|(i, name)| TabSpec { title: (i + 1).to_string(), panes: vec![sleeper(name)], side: None })
        .collect();
    let mut server = built(state.path(), &names, tabs);
    let client = attached(&mut server);
    (state, server, client)
}

#[test]
fn a_failed_turn_is_kept_until_another_state() {
    let (_state, mut server, _client) = four_tabs();
    let overloaded = r#"API Error: 529 {"type":"error","error":{"type":"overloaded_error"}}"#;
    note_status(&mut server.panes[1], &status(State::Working, None));
    note_status(&mut server.panes[1], &status(State::Error, Some(overloaded)));
    let error = server.panes[1].error.clone().expect("kept");
    assert_eq!(error, TurnError { message: Some(overloaded.into()), label: look::error_label(Some(overloaded)) });
    // Its header: the state, its reason short; `Panes`: the text whole.
    let header = server.header(&server.panes[1], None, None, now());
    assert_eq!((header.state, header.error), (look::State::Error, Some(error.label.as_str())));
    let Reply::Ok(list) = server.dispatch(Request::Panes) else { panic!("panes") };
    let list: Vec<PaneInfo> = serde_json::from_value(list).unwrap();
    let info = list.iter().find(|info| info.member == "dev-b").unwrap();
    assert_eq!((info.status.as_deref(), info.error.as_deref()), (Some(look::State::Error.name()), Some(overloaded)));
    // Its next turn: gone, everywhere.
    note_status(&mut server.panes[1], &status(State::Working, None));
    assert_eq!(server.panes[1].error, None);
    assert_eq!(server.header(&server.panes[1], None, None, now()).error, None);
    // Any other state too, `Clear` included.
    note_status(&mut server.panes[1], &status(State::Error, None));
    assert_eq!(
        server.panes[1].error.as_ref().map(|error| error.label.as_str()),
        Some(server.notices.unknown_error.as_str())
    );
    note_status(&mut server.panes[1], &status(State::Clear, None));
    assert_eq!(server.panes[1].error, None);
    // Failed as the dashboard saw it (no state of its own): its reason unknown, said short.
    server.looks.insert("dev-b".into(), board::MemberLook { state: Some(look::State::Error), ..Default::default() });
    let header = server.header(&server.panes[1], None, None, now());
    assert_eq!((header.state, header.error), (look::State::Error, Some(server.notices.unknown_error.as_str())));
    // Never for a member at work.
    assert_eq!(server.header(&server.panes[0], None, None, now()).error, None);
    // Failed: still compactable (its context known), and its failure wins over a command running.
    server.looks.insert(
        "dev-b".into(),
        board::MemberLook { context: Some(40), shell: true, since: Some(1), ..Default::default() },
    );
    note_status(&mut server.panes[1], &status(State::Error, Some("API Error:\n\t500")));
    let header = server.header(&server.panes[1], None, None, now());
    assert_eq!((header.state, header.compactable, header.shell), (look::State::Error, true, false));
    // On one line, whatever its program wrote.
    assert!(!header.error.unwrap().contains(['\n', '\t', '\r']), "{:?}", header.error);
    assert_eq!(one_line("a\nb\tc\r"), "a b c ");
}

#[test]
fn a_failed_turn_ranks_after_a_wait_on_tabs_counts_and_go() {
    let (_state, mut server, _client) = four_tabs();
    let now = now();
    let set = |server: &mut Server, i: usize, state: State, ago: u64| {
        server.panes[i].state = Some(state);
        server.panes[i].since = Some(now - ago);
    };
    // The tabs: waiting, else failed, else working, else at rest.
    use look::State::{Error, Idle, Waiting, Working};
    assert_eq!(urgent([Working, Error, Idle].into_iter()), Error);
    assert_eq!(urgent([Error, Waiting].into_iter()), Waiting);
    assert_eq!(urgent([Idle, Working].into_iter()), Working);
    set(&mut server, 0, State::Working, 5);
    set(&mut server, 1, State::Error, 600);
    set(&mut server, 2, State::Error, 60);
    set(&mut server, 3, State::Idle, 5);
    assert_eq!(server.counts(), chrome::Hint::Counts { working: 1, waiting: 0, error: 2, idle: 1 });
    // ⌥g: the turn that failed the longest ago, while none waits.
    server.go_waiting();
    assert_eq!(server.focus.as_deref(), Some(server.panes[1].id.as_str()));
    // One waiting, even since less long: it first.
    set(&mut server, 3, State::Blocked, 1);
    server.go_waiting();
    assert_eq!(server.focus.as_deref(), Some(server.panes[3].id.as_str()));
    assert_eq!(server.counts(), chrome::Hint::Counts { working: 1, waiting: 1, error: 2, idle: 0 });
}

#[test]
fn failed_turns_within_ten_seconds_share_one_orange_notice() {
    let (_state, mut server, _client) = four_tabs();
    let fail = |server: &mut Server, i: usize| {
        note_status(&mut server.panes[i], &status(State::Error, Some("API Error: 529 overloaded")));
    };
    let label = look::error_label(Some("API Error: 529 overloaded"));
    let (b, c, d) = (id_of(&server, "dev-b"), id_of(&server, "dev-c"), id_of(&server, "dev-d"));
    fail(&mut server, 1);
    server.call(vec![(b.clone(), look::State::Error)]);
    let (notice, until) = server.notices.notice.clone().expect("noticed");
    assert_eq!((notice.state, notice.member.as_str(), notice.tab.as_str()), (look::State::Error, "dev-b", "2"));
    assert_eq!(notice.text, chrome::error_notice_text(&["dev-b"], &label));
    // Another within ten seconds: the same notice, both told, its time kept.
    fail(&mut server, 2);
    server.call(vec![(c.clone(), look::State::Error)]);
    let (notice, kept) = server.notices.notice.clone().expect("still");
    assert_eq!(notice.text, chrome::error_notice_text(&["dev-b", "dev-c"], &label));
    assert_eq!((notice.member.as_str(), kept), ("dev-c", until));
    // Its time up within the ten seconds: none other.
    server.notices.notice = None;
    fail(&mut server, 3);
    server.call(vec![(d.clone(), look::State::Error)]);
    assert!(server.notices.notice.is_none(), "one for the burst");
    // Ten seconds on: a new one.
    server.notices.burst.as_mut().unwrap().0 -= BURST;
    server.call(vec![(d.clone(), look::State::Error)]);
    assert_eq!(server.notices.notice.as_ref().map(|(notice, _)| notice.member.as_str()), Some("dev-d"));
    // A wait wins over it, and keeps its place.
    server.panes[1].state = Some(State::Blocked);
    server.call(vec![(c.clone(), look::State::Error), (b.clone(), look::State::Waiting)]);
    let (notice, _) = server.notices.notice.clone().expect("waiting");
    assert_eq!((notice.state, notice.member.as_str()), (look::State::Waiting, "dev-b"));
    server.notices.burst = None;
    server.call(vec![(c.clone(), look::State::Error)]);
    assert_eq!(server.notices.notice.as_ref().map(|(notice, _)| notice.state), Some(look::State::Waiting));
    // None under a layer, nor without a client.
    server.notices.notice = None;
    server.choice = Some(Open::new(chrome::quit_choice(), Purpose::Quit { client: "c1".into() }));
    server.call(vec![(c.clone(), look::State::Error)]);
    assert!(server.notices.notice.is_none(), "under a layer");
    server.choice = None;
    let client = server.client.take();
    server.call(vec![(c.clone(), look::State::Error)]);
    assert!(server.notices.notice.is_none(), "no one to tell");
    server.client = client;
    // Gone once none of its members is failed out of sight any more.
    server.notices.burst = None;
    server.call(vec![(c.clone(), look::State::Error)]);
    fail(&mut server, 3);
    server.call(vec![(d.clone(), look::State::Error)]);
    note_status(&mut server.panes[3], &status(State::Working, None));
    server.check_notice();
    assert!(server.notices.notice.is_some(), "dev-c still failed");
    server.focus_on(&c);
    server.check_notice();
    assert!(server.notices.notice.is_none(), "dev-c in sight");
}

#[test]
fn a_member_failed_by_the_dashboard_is_noticed_too() {
    let (_state, mut server, _client) = four_tabs();
    // Its program says nothing; the dashboard saw its turn fail.
    let failed = board::MemberLook { state: Some(look::State::Error), ..Default::default() };
    server.message(Msg::Looks(BTreeMap::from([("dev-c".into(), failed.clone())])));
    let (notice, _) = server.notices.notice.clone().expect("noticed");
    assert_eq!((notice.state, notice.member.as_str()), (look::State::Error, "dev-c"));
    assert_eq!(notice.text, chrome::error_notice_text(&["dev-c"], &server.notices.unknown_error));
    // The same again: no second one.
    server.notices.notice = None;
    server.notices.burst = None;
    server.message(Msg::Looks(BTreeMap::from([("dev-c".into(), failed)])));
    assert!(server.notices.notice.is_none());
}

#[test]
fn a_member_whose_program_says_nothing_or_clear_is_as_the_dashboard_saw_it() {
    let state = tempfile::tempdir().unwrap();
    let tabs = vec![
        TabSpec { title: "1".into(), panes: vec![sleeper("dev-a")], side: None },
        TabSpec { title: "2".into(), panes: vec![sleeper("dev-b")], side: None },
    ];
    let mut server = built(state.path(), &["dev-a", "dev-b"], tabs);
    let _client = attached(&mut server);
    let b = id_of(&server, "dev-b");
    let now = now();
    // dev-b's program said `Clear` long ago (the omnidex coordinator, 2.0.0): its own state and time are no more.
    server.panes[1].state = Some(State::Clear);
    server.panes[1].since = Some(now - 5);
    server.panes[0].state = Some(State::Idle);
    let seen = |state| board::MemberLook { state: Some(state), since: Some(now as i64 - 300), ..Default::default() };
    let looks =
        BTreeMap::from([("dev-a".into(), seen(look::State::Working)), ("dev-b".into(), seen(look::State::Waiting))]);
    server.message(Msg::Looks(looks));
    // Out of sight, waiting by the dashboard: its notice, its tab's ⚑.
    assert_eq!(server.notices.notice.as_ref().map(|(notice, _)| notice.member.as_str()), Some("dev-b"));
    let header = server.header(&server.panes[1], None, None, now);
    assert_eq!((header.state, header.since), (look::State::Waiting, Some(300)));
    // dev-a's program says it rests: it, not the dashboard.
    assert_eq!(server.header(&server.panes[0], None, None, now).state, look::State::Idle);
    assert_eq!(server.counts(), chrome::Hint::Counts { working: 0, waiting: 1, error: 0, idle: 1 });
    let tab = |server: &Server, i: usize| {
        urgent(server.tabs[i].ids().iter().filter_map(|id| server.pane(id)).map(|pane| pane_state(pane, &server.looks)))
    };
    assert_eq!(tab(&server, 1), look::State::Waiting);
    // `Panes` gives its program's own: none. The dashboard reads it, not its own echo.
    let Reply::Ok(list) = server.dispatch(Request::Panes) else { panic!("panes") };
    let list: Vec<PaneInfo> = serde_json::from_value(list).unwrap();
    let info = list.iter().find(|info| info.id == b).unwrap();
    assert_eq!((info.status.as_deref(), info.since), (None, None));
    // ⌥g reaches it.
    server.go_waiting();
    assert_eq!(server.focus.as_deref(), Some(b.as_str()));
    // Its program speaks again: it wins over the dashboard.
    server.panes[1].state = Some(State::Working);
    assert_eq!(server.header(&server.panes[1], None, None, now).state, look::State::Working);
    // Never told, never seen: nothing.
    server.panes[1].state = None;
    server.looks.remove("dev-b");
    assert_eq!(server.header(&server.panes[1], None, None, now).state, look::State::Other);
    // Panels keep their own (none): the dashboard's states are the members'.
    assert!(server.panes.iter().all(|pane| pane.is_member() || pane_state(pane, &server.looks) == look::State::Other));
}

#[test]
fn the_headers_parts_and_the_notice_answer_the_pointer() {
    let state = tempfile::tempdir().unwrap();
    let tabs = vec![
        TabSpec { title: "1".into(), panes: vec![sleeper("dev-a"), sleeper("dev-b")], side: None },
        TabSpec { title: "2".into(), panes: vec![sleeper("dev-c")], side: None },
    ];
    let mut server = built(state.path(), &["dev-a", "dev-b", "dev-c"], tabs);
    let _client = attached(&mut server);
    let (a, b, c) = (id_of(&server, "dev-a"), id_of(&server, "dev-b"), id_of(&server, "dev-c"));
    let look = board::MemberLook {
        model: Some("Opus".into()),
        effort: Some("high".into()),
        context: Some(40),
        compactable: true,
        ..Default::default()
    };
    server.looks.insert("dev-a".into(), look);
    assert!(server.compose());
    let part = |server: &Server, id: &str, part: chrome::Part| {
        let found = server.parts.iter().find(|(_, of, p)| of == id && *p == part);
        found.map(|(at, _, _)| (at.x as u16, at.y as u16)).unwrap_or_else(|| panic!("{part:?} of {id}"))
    };
    let press = |server: &mut Server, (col, row): (u16, u16), kind: MouseKind| {
        server.mouse(&Mouse { kind, col, row, mods: Mods::NONE });
    };
    // Hovered: lit, once.
    let name = part(&server, &a, chrome::Part::Name);
    server.dirty = false;
    press(&mut server, name, MouseKind::Moved);
    assert_eq!(server.pointer.hover, Some(Hover::Part(a.clone(), chrome::Part::Name)));
    assert!(server.dirty);
    server.dirty = false;
    press(&mut server, name, MouseKind::Moved);
    assert!(!server.dirty, "the same part: nothing to draw");
    // Its context: the compaction's confirmation.
    let context = part(&server, &a, chrome::Part::Context);
    press(&mut server, context, MouseKind::Down(Button::Left));
    assert_eq!(server.choice.as_ref().map(|open| &open.purpose), Some(&Purpose::Compact { member: "dev-a".into() }));
    server.choice = None;
    press(&mut server, context, MouseKind::Up(Button::Left));
    assert!(!server.pointer.swallowing, "its release swallowed");
    // ⤢: zoomed, with the focus.
    assert!(server.compose());
    let zoom = part(&server, &b, chrome::Part::Zoom);
    press(&mut server, zoom, MouseKind::Down(Button::Left));
    assert_eq!(server.tabs[0].zoomed.as_deref(), Some(b.as_str()));
    assert_eq!(server.focus.as_deref(), Some(b.as_str()));
    press(&mut server, zoom, MouseKind::Up(Button::Left));
    // A member out of sight waiting: its notice, and a click on it goes to it.
    server.notify(&c);
    assert!(server.compose());
    let at = server.notices.notice_zone.expect("drawn");
    press(&mut server, (at.x as u16 + 1, at.y as u16), MouseKind::Down(Button::Left));
    assert_eq!((server.active, server.focus.as_deref()), (1, Some(c.as_str())));
    assert!(server.notices.notice.is_none());
}

#[test]
fn a_list_goes_under_its_text_within_the_screen() {
    let screen = (80, 23);
    let text = |x, y| Rect { x, y, width: 6, height: 1 };
    // Under the text, its content aligned on it (the frame one column to the left).
    assert_eq!(list_area(text(10, 0), (26, 8), screen), Rect { x: 6, y: 1, width: 26, height: 8 });
    // Too far right: moved left just enough to end at the edge.
    assert_eq!(list_area(text(70, 0), (26, 8), screen), Rect { x: 54, y: 1, width: 26, height: 8 });
    // No room below: above the text, its bottom edge on the row over it.
    assert_eq!(list_area(text(10, 20), (26, 8), screen), Rect { x: 6, y: 12, width: 26, height: 8 });
    // Room neither below nor above: the larger side, cut to it. Never out of the screen.
    assert_eq!(list_area(text(10, 11), (26, 40), screen), Rect { x: 6, y: 12, width: 26, height: 11 });
    assert_eq!(list_area(text(10, 14), (90, 40), screen), Rect { x: 0, y: 0, width: 80, height: 14 });
    assert_eq!(list_area(text(10, 3), (26, 20), (20, 5)), Rect { x: 0, y: 0, width: 20, height: 3 });
    // A screen of one row: the list there, not under it.
    assert_eq!(list_area(text(10, 0), (26, 8), (80, 1)), Rect { x: 6, y: 0, width: 26, height: 1 });
    // Near the left edge.
    assert_eq!(list_area(text(2, 0), (26, 8), screen).x, 0);
}

#[test]
fn a_click_on_the_model_or_effort_opens_a_list_that_a_click_outside_closes() {
    let state = tempfile::tempdir().unwrap();
    let tabs = vec![TabSpec { title: "1".into(), panes: vec![sleeper("dev-a"), sleeper("dev-b")], side: None }];
    let mut server = built(state.path(), &["dev-a", "dev-b"], tabs);
    server.exe = Some("/bin/cat".into());
    let _client = attached(&mut server);
    let (a, b) = (id_of(&server, "dev-a"), id_of(&server, "dev-b"));
    let look = board::MemberLook {
        model: Some("Opus".into()),
        effort: Some("high".into()),
        context: Some(40),
        ..Default::default()
    };
    server.looks.insert("dev-a".into(), look.clone());
    server.looks.insert("dev-b".into(), look);
    server.focus_on(&a);
    assert!(server.compose());
    let part = |server: &Server, id: &str, part: chrome::Part| {
        let found = server.parts.iter().find(|(_, of, p)| of == id && *p == part);
        found.map(|(at, _, _)| *at).unwrap_or_else(|| panic!("{part:?} of {id}"))
    };
    let click = |server: &mut Server, col: usize, row: usize| {
        server.mouse(&Mouse {
            kind: MouseKind::Down(Button::Left),
            col: col as u16,
            row: row as u16,
            mods: Mods::NONE,
        });
        server.mouse(&Mouse { kind: MouseKind::Up(Button::Left), col: col as u16, row: row as u16, mods: Mods::NONE });
    };

    // The effort of dev-b: a list under it, the focus on it, the team not dimmed.
    let effort = part(&server, &b, chrome::Part::Effort);
    click(&mut server, effort.x, effort.y);
    let list = server.list.clone().expect("a list");
    assert_eq!((list.pane.as_str(), list.part, list.anchor), (b.as_str(), chrome::Part::Effort, effort));
    let (menu, before) = server.menu.clone().expect("the menu's pane");
    assert_eq!(before.as_deref(), Some(a.as_str()));
    assert_eq!(server.focus.as_deref(), Some(menu.as_str()));
    assert_eq!(server.pane(&menu).unwrap().area, list_area(effort, LIST_SIZE, (80, 23)));
    assert!(server.compose());
    assert!(server.choice.is_none() && server.notices.notice.is_none());

    // The menu tells the size it needs, its frame included: the pane takes exactly that.
    let mut replies = Vec::new();
    server.panes.iter().find(|pane| pane.id == menu).unwrap().with(|engine| engine.feed(b"\x1b[8;5;30t", &mut replies));
    server.take_relays();
    let area = server.pane(&menu).unwrap().area;
    assert_eq!((area.width, area.height), (30, 5), "the size announced, no frame added");
    assert_eq!(area, list_area(effort, (30, 5), (80, 23)));
    // Another pane's size request is no one's business.
    server.panes.iter().find(|pane| pane.id == a).unwrap().with(|engine| engine.feed(b"\x1b[8;5;9t", &mut replies));
    server.take_relays();
    assert_eq!(server.pane(&a).unwrap().size, server.pane(&a).unwrap().size);
    assert_eq!(server.pane(&menu).unwrap().area, area);

    // A press inside the list is the list's, even over a header part it covers: nothing else opens.
    click(&mut server, area.x + 2, area.y + 2);
    assert!(server.list.is_some() && server.choice.is_none());
    // A press outside closes it and does nothing more; the focus is where it was.
    let name = part(&server, &a, chrome::Part::Name);
    click(&mut server, name.x, name.y);
    assert!(server.list.is_none() && server.menu.is_none());
    assert!(server.pane(&menu).is_none(), "its pane is gone");
    assert_eq!(server.focus.as_deref(), Some(a.as_str()));
    assert!(server.compose());

    // A second click on the same text closes it, and does not open another.
    let model = part(&server, &b, chrome::Part::Model);
    click(&mut server, model.x, model.y);
    assert!(server.list.as_ref().is_some_and(|list| list.part == chrome::Part::Model));
    click(&mut server, model.x, model.y);
    assert!(server.list.is_none() && server.menu.is_none());
    assert_eq!(server.focus.as_deref(), Some(a.as_str()));

    // The window resized meanwhile: closed, the text it hung under has moved.
    click(&mut server, model.x, model.y);
    assert!(server.list.is_some());
    server.resize((100, 30));
    assert!(server.list.is_none() && server.menu.is_none());
    assert_eq!(server.focus.as_deref(), Some(a.as_str()));
    assert!(server.compose());
    let model = part(&server, &b, chrome::Part::Model);
    // Ended by itself (Échap): closed the same.
    click(&mut server, model.x, model.y);
    let (menu, _) = server.menu.clone().unwrap();
    server.close_pane(&menu);
    assert!(server.list.is_none() && server.menu.is_none());
    assert_eq!(server.focus.as_deref(), Some(a.as_str()));
    // The name still opens the whole menu, centered.
    let name = part(&server, &b, chrome::Part::Name);
    click(&mut server, name.x, name.y);
    assert!(server.list.is_none());
    let (menu, _) = server.menu.clone().expect("the menu");
    assert_eq!(server.pane(&menu).unwrap().area, server.menu_area());
}

#[test]
fn the_colors_follow_the_team() {
    let state = tempfile::tempdir().unwrap();
    let write = |names: &[&str]| {
        let members =
            names.iter().map(|name| crate::team::state::MemberInfo { name: name.to_string(), ..Default::default() });
        Snapshot { members: members.collect(), ..Default::default() }.write(state.path()).unwrap();
    };
    write(&["chef", "dev-a", "dev-b"]);
    let (tx, _rx) = mpsc::channel();
    let welcome = Arc::new(Mutex::new(Welcome::default()));
    let socket = (state.path().join("s.sock"), socket::FileId::default());
    let mut server = Server::new(state.path(), "t", socket, tx, welcome);
    let tabs = vec![TabSpec { title: "1".into(), panes: vec![sleeper("dev-a")], side: None }];
    let build = Request::Build {
        team: "t".into(),
        dir: "/".into(),
        columns: 3,
        tabs,
        cols: 80,
        rows: 24,
        exe: String::new(),
        lang: String::new(),
        focus: false,
    };
    assert!(matches!(server.request(build), Reply::Ok(_)));
    assert_eq!(server.order, ["chef", "dev-a", "dev-b"]);
    // Renamed: in place, before team.json says it (`team/live.rs`).
    let id = server.panes[0].id.clone();
    assert!(matches!(server.request(Request::SetMember { id, member: "dev-z".into() }), Reply::Ok(_)));
    assert_eq!(server.order, ["chef", "dev-z", "dev-b"]);
    // Added, then removed: as team.json says once written.
    write(&["chef", "dev-z", "dev-b", "dev-c"]);
    assert!(matches!(server.request(Request::OpenWindow { pane: sleeper("dev-c") }), Reply::Ok(_)));
    assert_eq!(server.order, ["chef", "dev-z", "dev-b", "dev-c"]);
    write(&["chef", "dev-z", "dev-c"]);
    let arrange =
        Request::Arrange { tabs: vec![("1".into(), vec!["dev-z".into(), "dev-c".into()])], columns: 3, focus: false };
    assert!(matches!(server.request(arrange), Reply::Ok(_)));
    assert_eq!(server.order, ["chef", "dev-z", "dev-c"]);
    assert_eq!(member_color(&server.order, "dev-c"), look::member_color(2));
}

#[test]
fn members_take_the_dashboards_colors() {
    let order: Vec<String> = ["chef", "dev-a", "dev-b"].map(String::from).to_vec();
    assert_eq!(member_color(&order, "chef"), look::member_color(0));
    assert_eq!(member_color(&order, "dev-b"), look::member_color(2));
    assert_eq!(member_color(&order, "inconnu"), look::member_color(3), "after the others");
    assert_ne!(member_color(&order, "chef"), member_color(&order, "dev-a"));
}

#[test]
fn a_tab_forgets_its_panels() {
    let mut tab = Tab::new("t".into(), vec!["p1".into(), "p2".into()]);
    tab.side = Some(Side { dashboard: Some("p3".into()), journal: Some("p4".into()) });
    assert_eq!(tab.ids(), ["p1", "p2", "p3", "p4"]);
    tab.forget("p4");
    assert_eq!(tab.ids(), ["p1", "p2", "p3"]);
    tab.forget("p3");
    assert!(tab.side.is_none());
    tab.forget("p1");
    assert!(tab.has("p2") && !tab.has("p1"));
}
