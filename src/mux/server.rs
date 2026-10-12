// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A team's server, `recruit _server <state>`, one per team: it owns the panes (their PTYs and
//! engines), draws the screen for the client, and answers the commands of the team's other processes, on a socket.
//! It outlives its client: the terminal closed, SSH cut, the team goes on.
//!
//! Started detached: `_server` forks twice and returns once the server is ready, or with its error. The lock
//! of the state folder is taken in the server only, after the forks: its descriptor never leaves it.
//!
//! Threads: this loop, alone to touch the state; per pane, the PTY's reader, writer and waiter (`pty.rs`); one
//! that waits for signals; one that accepts connections; per connection, one that reads it, and per client one that
//! writes to it. Nothing wakes the loop but a message, a frame due, or an engine's deadline.

mod choice;
mod clients;
mod compaction;
mod events;
mod focus_layout;
mod frame;
mod lifecycle;
mod menu;
mod mouse;
mod notice;
mod pane;
mod panes;
mod requests;
mod status;
mod tabs;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use self::choice::Open;
use self::clients::{Attaching, Client};
use self::compaction::Auto;
use self::focus_layout::Cards;
use self::frame::Pacing;
use self::lifecycle::log;
use self::menu::List;
use self::mouse::Pointer;
use self::notice::Notices;
use self::pane::{Broken, Gate, Pane};
use self::status::{calls, last_words, pane_state};
use self::tabs::Tab;
use super::chrome::{self, Target};
use super::proto::{Bye, ClientMsg, PaneSpec, Reply, Request, Welcome};
use super::screen;
use super::socket;
use crate::ui::Rect;
use crate::{board, claude::bridge};

pub(crate) use self::lifecycle::run;

/// One frame at most per this long (16 ms).
const FRAME: Duration = Duration::from_millis(16);

/// How long after a key or a paste the focused pane's answer is still its echo: drawn at once rather than at the next
/// frame.
const ECHO: Duration = Duration::from_millis(250);

/// How long a client let go has to read its goodbye before its connection is shut: a client behind a dead SSH
/// connection would keep its writer in a write for good.
const BYE_TIME: Duration = Duration::from_secs(1);

/// The size of the screen before any client tells its own.
const DEFAULT_SIZE: (u16, u16) = (200, 50);

/// What the loop is told.
enum Msg {
    /// A pane's program wrote: its engine has news.
    Output(String),
    /// A pane's program ended; `Broken` tells which program, for a pane started again since.
    Exited(String, Arc<Broken>, Option<i32>),
    /// An engine panicked on what its program wrote.
    Broken,
    Signal(libc::c_int),
    /// A client asks for the team.
    Attach(Box<Attaching>),
    Client(String, ClientMsg),
    /// A client's connection ended.
    Gone(String),
    /// A client's writer has written all it was given.
    Ready(String),
    /// A command, where its answer goes, and where the connection says it wrote it.
    Command(Request, Sender<Reply>, Receiver<()>),
    /// The members' looks for their headers, changed (`watch_looks`).
    Looks(BTreeMap<String, board::MemberLook>),
    /// The team's share of the window from which a member at rest is compacted, as `team.json` says it now (`None`:
    /// off).
    AutoCompact(Option<u8>),
    /// A member's automatic compaction request was written (its id), or could not be (`Err`): `auto_compaction`.
    Requested(String, u64, Result<String, String>),
    /// How the requests asked went, by member, as their reports say it (`watch_looks`).
    Outcomes(BTreeMap<String, bridge::Compaction>),
}

/// Who asked the team to stop: where its answer goes, and where the connection says it wrote it.
type Asker = (Sender<Reply>, Receiver<()>);

/// Most columns of members in a tab, until `Build` says the team's.
const COLUMNS: usize = 3;

/// The server's state, the loop's alone.
struct Server {
    state: PathBuf,
    socket: PathBuf,
    socket_id: socket::FileId,
    tx: Sender<Msg>,
    gate: Arc<Gate>,
    welcome: Arc<Mutex<Welcome>>,
    panes: Vec<Pane>,
    tabs: Vec<Tab>,
    active: usize,
    focus: Option<String>,
    client: Option<Client>,
    /// The screen's size: the client's, else the last one's.
    size: (u16, u16),
    /// What the programs started from now on get from the last client (`proto::UPDATE_ENV`).
    env: Vec<(String, Option<String>)>,
    next_pane: u64,
    /// A pane ever opened: the team ends with its last one.
    started: bool,
    /// Where the bar's parts can be clicked, as last drawn.
    zones: Vec<(Rect, Target)>,
    /// The last frame composed, kept to compose the next: a pane that drew nothing new keeps its cells.
    screen: RefCell<screen::Screen>,
    dirty: bool,
    /// The team's name, for the bar.
    team: String,
    /// Most columns of members in a tab.
    columns: usize,
    /// The journal shows its last messages only.
    reduced: bool,
    /// What opens the journal again once hidden (⌥j), as `launch` gave it.
    journal: Option<PaneSpec>,
    /// The recruit and the language the server runs its own programs with, as `Build` gave them.
    exe: Option<String>,
    lang: String,
    /// The menu's floating pane, over the team, and the pane that had the focus before it.
    menu: Option<(String, Option<String>)>,
    /// The menu is a list under a header's text, not the whole menu.
    list: Option<List>,
    /// A choice open over the team (⌥q, a compaction's confirmation): it takes the keys and the clicks.
    choice: Option<Open>,
    /// Quit was picked: the team stops at the end of this turn.
    quitting: bool,
    /// The members in the team's order, for their colors (`member_color`).
    order: Vec<String>,
    /// What each member's header shows besides its state (model, effort, context), as `watch_looks` last read it.
    looks: BTreeMap<String, board::MemberLook>,
    /// Where the headers' parts can be clicked, by pane, as last drawn.
    parts: Vec<(Rect, String, chrome::Part)>,
    /// Something the automatic compaction and the tabs in « focus » depend on may have changed (a state, the looks, an
    /// answer, a client, a tab, a key, the pointer): they look again at the end of this turn, not at each frame.
    look_again: bool,
    /// The client's terminal has a light background (its OSC 11 answer), so the light palette: as of its attach.
    light: bool,
    /// The team's tabs of agents are in « focus » (`layout = "focus"`), as `Build` and `Arrange` say.
    focus_layout: bool,
    /// When the tabs in « focus » next look at their places, for a place that comes due; `None` when none does.
    focus_wake: Option<Instant>,
    /// When the next frame is due, and what is sent.
    pacing: Pacing,
    /// What the client did last: its pointer, its buttons, its keys, the selection.
    pointer: Pointer,
    /// The notice of a member gone waiting out of sight, or whose turn failed.
    notices: Notices,
    /// The automatic compaction of the members at rest.
    auto: Auto,
    /// The cards of the members left out of the tab shown in « focus ».
    cards: Cards,
}

/// What the pointer can be over, lit.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Hover {
    /// A part of a pane's header, by its pane.
    Part(String, chrome::Part),
    /// A tab, a button of the bar.
    Bar(Target),
}

impl Server {
    /// A server for the team whose state is in `state`, before any pane: its socket (path, and what tells the file
    /// apart), where its threads write to it, and what it welcomes connections with.
    fn new(
        state: &Path,
        session: &str,
        socket: (PathBuf, socket::FileId),
        tx: Sender<Msg>,
        welcome: Arc<Mutex<Welcome>>,
    ) -> Server {
        Server {
            state: state.to_path_buf(),
            socket: socket.0,
            socket_id: socket.1,
            tx,
            gate: Arc::new(Gate::default()),
            welcome,
            panes: Vec::new(),
            tabs: Vec::new(),
            active: 0,
            focus: None,
            client: None,
            size: DEFAULT_SIZE,
            env: Vec::new(),
            next_pane: 0,
            started: false,
            zones: Vec::new(),
            screen: RefCell::default(),
            dirty: false,
            team: session.to_string(),
            columns: COLUMNS,
            reduced: true,
            journal: None,
            exe: None,
            lang: crate::i18n::lang().code().to_string(),
            menu: None,
            list: None,
            choice: None,
            quitting: false,
            order: Vec::new(),
            looks: BTreeMap::new(),
            parts: Vec::new(),
            look_again: false,
            light: false,
            focus_layout: false,
            focus_wake: None,
            pacing: Pacing::new(),
            pointer: Pointer::new(),
            notices: Notices::new(),
            auto: Auto::new(),
            cards: Cards::default(),
        }
    }
}

impl Server {
    fn run(mut self, rx: Receiver<Msg>) -> Result<()> {
        loop {
            let now = Instant::now();
            let frame_due = self
                .dirty
                .then(|| if self.pacing.echo { now } else { self.pacing.last_frame.map_or(now, |at| at + FRAME) });
            let notice = self.notices.notice.as_ref().map(|(_, until)| *until);
            let due = [
                frame_due,
                self.pacing.deadline,
                self.pacing.animate,
                self.pacing.clock,
                notice,
                self.auto.compaction,
                self.focus_wake,
            ];
            let first = match due.into_iter().flatten().min() {
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(now)) {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                },
                None => match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return Ok(()),
                },
            };
            let gate = Arc::clone(&self.gate);
            let held = gate.hold();
            let mut stop: Option<(Bye, Option<Asker>)> = None;
            let pending: Vec<Msg> = first.into_iter().chain(rx.try_iter()).collect();
            for msg in pending {
                match msg {
                    Msg::Command(Request::Stop, reply, written) => {
                        stop = Some((Bye::Stopped, Some((reply, written))));
                    }
                    Msg::Command(request, reply, _) => {
                        self.look_again = true;
                        let answer = self.request(request);
                        let _ = reply.send(answer);
                    }
                    Msg::Signal(libc::SIGTERM | libc::SIGINT) => stop = Some((Bye::Stopped, None)),
                    Msg::Signal(libc::SIGURG) => self.reopen_socket(),
                    Msg::Signal(signal) => log!("signal {signal}"),
                    other => self.message(other),
                }
            }
            drop(held);
            if stop.is_none() && std::mem::take(&mut self.quitting) {
                stop = Some((Bye::Stopped, None));
            }
            if stop.is_none() && self.started && !self.panes.iter().any(|pane| pane.is_member()) {
                stop = Some((Bye::Ended, None));
            }
            if let Some((bye, asker)) = stop {
                self.stop(&rx, bye, asker);
                return Ok(());
            }
            let now = Instant::now();
            if self.pacing.animate.is_some_and(|at| at <= now) {
                self.pacing.animate = None;
                self.dirty = true;
            }
            if self.pacing.clock.is_some_and(|at| at <= now) {
                self.pacing.clock = None;
                self.dirty = true;
            }
            self.check_notice();
            self.expire();
            self.repair();
            self.frame();
            // After the frame: the programs' states are noted as it takes their relays (`take_relays`).
            let again = std::mem::take(&mut self.look_again);
            if again || self.auto.compaction.is_some_and(|at| at <= Instant::now()) {
                self.auto_compaction();
            }
            if again || self.focus_wake.is_some_and(|at| at <= Instant::now()) {
                self.update_focus();
            }
            if std::mem::take(&mut self.cards.stale) {
                self.refresh_cards();
            }
        }
    }

    /// A message that is not a command nor a signal.
    fn message(&mut self, msg: Msg) {
        // Output is the frames' business; all else may change who is to be compacted.
        self.look_again |= !matches!(msg, Msg::Output(_));
        match msg {
            Msg::Output(id) => {
                if self.focus.as_deref() == Some(id.as_str())
                    && self.pacing.typed.take().is_some_and(|at| at.elapsed() < ECHO)
                {
                    self.pacing.echo = true;
                }
                self.dirty = true;
            }
            Msg::Exited(id, which, code) => {
                let Some(i) = self.panes.iter().position(|pane| pane.id == id && Arc::ptr_eq(&pane.broken, &which))
                else {
                    return;
                };
                log!("pane {id} ({}) ended: {code:?}", self.panes[i].member);
                // Failed: what it said last (a panic's message), before its pane goes with it.
                if code != Some(0) {
                    let rows = self.panes[i].size.1;
                    let said = self.panes[i].with(|engine| last_words(engine, rows)).unwrap_or_default();
                    if !said.is_empty() {
                        log!("pane {id} said: {said}");
                    }
                }
                self.panes[i].exited = true;
                self.close_pane(&id);
            }
            Msg::Broken => self.dirty = true,
            Msg::Looks(looks) => {
                self.cards.stale = true;
                // A member whose program says nothing, gone waiting or failed out of sight by the dashboard: a notice,
                // as by its program (`take_relays`).
                let shown = self.in_view_ids();
                let mut called = Vec::new();
                for pane in self.panes.iter().filter(|pane| pane.is_member() && !shown.contains(&pane.id)) {
                    let state = pane_state(pane, &looks);
                    if calls(state) && pane_state(pane, &self.looks) != state {
                        called.push((pane.id.clone(), state));
                    }
                }
                self.looks = looks;
                self.dirty = true;
                self.call(called);
            }
            Msg::AutoCompact(share) => self.auto.auto_compact = share,
            Msg::Requested(member, since, id) => {
                if let Some(asked) = self.auto.asked.get_mut(&member).filter(|asked| asked.since == since) {
                    match id {
                        Ok(id) => asked.id = Some(id),
                        Err(error) => log!("auto-compaction: {member}: {error}"),
                    }
                }
            }
            Msg::Outcomes(outcomes) => self.auto.outcomes = outcomes,
            Msg::Attach(attaching) => self.attach(*attaching),
            Msg::Client(id, message) => {
                if self.client.as_ref().is_some_and(|client| client.id == id) {
                    self.client_message(message);
                }
            }
            Msg::Gone(id) => {
                if self.client.as_ref().is_some_and(|client| client.id == id) {
                    log!("client {id} gone");
                    let _ = self.drop_client(None);
                }
            }
            Msg::Ready(id) => {
                if let Some(client) = self.client.as_mut().filter(|client| client.id == id) {
                    client.busy = false;
                    if std::mem::take(&mut client.late) {
                        self.dirty = true;
                        self.pacing.last_frame = None;
                    }
                }
            }
            Msg::Command(..) | Msg::Signal(_) => {}
        }
    }

    fn focused(&self) -> Option<&Pane> {
        let id = self.focus.as_deref()?;
        self.panes.iter().find(|pane| pane.id == id)
    }

    fn pane(&self, id: &str) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    /// The pane of the member called `name`.
    fn member_pane(&self, name: &str) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.is_member() && pane.member == name)
    }

    /// The member whose pane has the focus; `None` for a panel or the menu.
    fn focused_member(&self) -> Option<&str> {
        self.focused().filter(|pane| pane.is_member()).map(|pane| pane.member.as_str())
    }
}
