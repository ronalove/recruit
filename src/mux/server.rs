// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A team's server, `recruit _server <state>` (specs/multiplexeur-serveur.md): it owns the panes (their PTYs and
//! engines), draws the screen for the client, and answers the commands of the team's other processes, on a socket.
//! It outlives its client: the terminal closed, SSH cut, the team goes on.
//!
//! Started detached: `_server` forks twice and returns once the server is ready, or with its error (§1.1). The lock
//! of the state folder is taken in the server only, after the forks: its descriptor never leaves it.
//!
//! Threads (§6): this loop, alone to touch the state; per pane, the PTY's reader, writer and waiter (`pty.rs`); one
//! that waits for signals; one that accepts connections; per connection, one that reads it, and per client one that
//! writes to it. Nothing wakes the loop but a message, a frame due, or an engine's deadline.
//!
//! Owner: dev-serveur.

mod choice;
mod pane;

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use self::choice::{Open, Outcome, Purpose};
use self::pane::{Broken, Command, Gate, Pane, TurnError, Wires};
use super::cards::{self, Panel};
use super::chrome::{self, Target};
use super::engine::{self, MouseTracking, Relay, State};
use super::focus::{self, Focus, Places, Seen, Standing};
use super::input::{self, Button, Event, Mods, Mouse, MouseKind};
use super::keys::{self, Action, Shortcut};
use super::proto::{
    self, Bye, Captured, ClientInfo, ClientMsg, Hello, Kind, MenuField, MenuOpened, PaneInfo, PaneModes, PaneSpec,
    PaneStats, Refusal, Reply, Request, ServerMsg, Stats, TabSpec, Welcome,
};
use super::screen::{self, View};
use super::select::{self, Selection};
use super::socket::{self, Info};
use super::{Caps, Rect};
use crate::canvas::{Canvas, Painter};
use crate::layout::{self, SidePanes};
use crate::look::{self, Glyphs};
use crate::state::Snapshot;
use crate::{board, bridge, t};

/// One frame at most per this long (spec §5.3).
const FRAME: Duration = Duration::from_millis(16);

/// How long after a key or a paste the focused pane's answer is still its echo: drawn at once rather than at the next
/// frame.
const ECHO: Duration = Duration::from_millis(250);

/// Lines a notch of the wheel scrolls.
const WHEEL: isize = 3;

/// How long a connection may take to say what it wants.
const HELLO_TIME: Duration = Duration::from_secs(5);

/// How long the panes have to end when the team stops, before the server goes all the same: longer than `pty.rs`'s
/// grace, after which it kills them, so that its SIGKILL is sent before the server leaves.
const STOP_TIME: Duration = Duration::from_secs(4);

/// How long a client let go has to read its goodbye before its connection is shut: a client behind a dead SSH
/// connection would keep its writer in a write for good.
const BYE_TIME: Duration = Duration::from_secs(1);

/// How long the server waits, when it stops, for the answer to `Stop` to be written.
const ANSWER_TIME: Duration = Duration::from_secs(1);

/// Notifications kept for a client that does not read; the oldest go first.
const NOTIFICATIONS: usize = 64;

/// The size of the screen before any client tells its own.
const DEFAULT_SIZE: (u16, u16) = (200, 50);

/// The exit code after a panic outside a pane's engine (§1.4).
const PANICKED: i32 = 70;

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

/// A client's connection, once it said what it is.
struct Attaching {
    id: String,
    stream: UnixStream,
    out: Arc<Outbox>,
    caps: Caps,
    size: (u16, u16),
    term: String,
    env: Vec<(String, String)>,
}

/// What goes to a client, for its writer.
enum Out {
    Json(Vec<u8>),
    Output(Vec<u8>),
    /// The last: the connection is shut once what comes before is written.
    Close,
}

#[derive(Default)]
struct Outbox {
    queue: Mutex<VecDeque<Out>>,
    ready: Condvar,
    /// Set by the writer when it has left: all written, or the connection lost.
    closed: Mutex<bool>,
    left: Condvar,
}

impl Outbox {
    /// Waits, `time` at most, for the writer to leave. Whether it did.
    fn wait_closed(&self, time: Duration) -> bool {
        let closed = self.closed.lock().unwrap_or_else(PoisonError::into_inner);
        let (closed, _) =
            self.left.wait_timeout_while(closed, time, |closed| !*closed).unwrap_or_else(PoisonError::into_inner);
        *closed
    }

    fn set_closed(&self) {
        *self.closed.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.left.notify_all();
    }

    fn push(&self, out: Out) {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).push_back(out);
        self.ready.notify_one();
    }

    fn message(&self, message: &ServerMsg) {
        if let Ok(json) = serde_json::to_vec(message) {
            self.push(Out::Json(json));
        }
    }
}

/// A client attached.
struct Client {
    id: String,
    stream: UnixStream,
    out: Arc<Outbox>,
    caps: Caps,
    term: String,
    size: (u16, u16),
    since: u64,
    painter: Painter,
    /// Its writer has a frame to write.
    busy: bool,
    /// A frame came due while it was busy: the next one goes when it is ready.
    late: bool,
    /// What the focused pane, or the chrome, asks of the mouse, as last told.
    motion: Option<bool>,
    relays: Relays,
}

/// What the programs asked to hand on to the real terminal, until the client's next frame: bounded by nature (§6.2).
#[derive(Default)]
struct Relays {
    clipboard: Option<Relay>,
    title: Option<Relay>,
    progress: Option<Relay>,
    bell: bool,
    notifications: VecDeque<Relay>,
}

impl Relays {
    fn add(&mut self, relay: Relay) {
        match relay {
            Relay::Clipboard(_) => self.clipboard = Some(relay),
            Relay::Title(_) => self.title = Some(relay),
            Relay::Progress(_) => self.progress = Some(relay),
            Relay::Bell => self.bell = true,
            Relay::Notify { .. } => {
                if self.notifications.len() == NOTIFICATIONS {
                    self.notifications.pop_front();
                }
                self.notifications.push_back(relay);
            }
            Relay::Status(_) | Relay::Resize { .. } => {}
        }
    }

    fn take(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let relays = [self.clipboard.take(), self.title.take(), self.progress.take()];
        for relay in relays.into_iter().flatten().chain(self.notifications.drain(..)) {
            bytes.extend(relay.encode().unwrap_or_default());
        }
        if std::mem::take(&mut self.bell) {
            bytes.extend(Relay::Bell.encode().unwrap_or_default());
        }
        bytes
    }
}

/// A tab: its title, its members' panes in reading order, and the panels' column beside them.
struct Tab {
    title: String,
    panes: Vec<String>,
    side: Option<Side>,
    /// The pane that had the focus when the tab was last shown: it gets it back, as a tmux window does.
    last: Option<String>,
    /// The member's pane that takes the tab's room (⌥z, ⤢), the others hidden.
    zoomed: Option<String>,
    /// ⌥l: the tab in « focus » (true) or in its grid (false) whatever the team says, until the client leaves.
    mode: Option<bool>,
    /// The places of a tab in « focus », kept from one look to the next; none for a tab in its grid.
    focus: Option<Focus>,
}

/// The panels' column: the dashboard over the journal, by pane ids.
#[derive(Clone, Default)]
struct Side {
    dashboard: Option<String>,
    journal: Option<String>,
}

impl Tab {
    fn new(title: String, panes: Vec<String>) -> Tab {
        Tab { title, panes, side: None, last: None, zoomed: None, mode: None, focus: None }
    }

    /// Every pane of the tab: the members', then the panels'.
    fn ids(&self) -> Vec<String> {
        let side = self.side.iter().flat_map(|side| [side.dashboard.clone(), side.journal.clone()]).flatten();
        self.panes.iter().cloned().chain(side).collect()
    }

    fn has(&self, id: &str) -> bool {
        self.ids().iter().any(|pane| pane == id)
    }

    /// Takes pane `id` out of the tab.
    fn forget(&mut self, id: &str) {
        self.panes.retain(|pane| pane != id);
        if self.zoomed.as_deref() == Some(id) {
            self.zoomed = None;
        }
        if let Some(side) = self.side.as_mut() {
            if side.dashboard.as_deref() == Some(id) {
                side.dashboard = None;
            }
            if side.journal.as_deref() == Some(id) {
                side.journal = None;
            }
            if side.dashboard.is_none() && side.journal.is_none() {
                self.side = None;
            }
        }
    }
}

/// The reduced journal's rows, its frame included: its last three messages, two lines each, then the line the next
/// one starts on.
const JOURNAL_ROWS: usize = 3 * 2 + 1 + 2 * chrome::FRAME;

/// The kinds of the panes that are not members'.
const DASHBOARD: &str = "dashboard";
const JOURNAL: &str = "journal";
const MENU: &str = "menu";

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
    /// The pane a button went down in: its drag and its release go there.
    grab: Option<String>,
    /// The button that went down in it.
    grab_button: Button,
    /// Where the pointer last was in it, its cells from 0: at its press, then at each drag.
    grab_at: (u16, u16),
    /// A click settled a choice: the events of its button, until its release, are swallowed.
    swallowing: bool,
    dirty: bool,
    last_frame: Option<Instant>,
    typed: Option<Instant>,
    echo: bool,
    /// The earliest deadline of the engines.
    deadline: Option<Instant>,
    /// When the spinner of a member at work shown needs its next image; `None` when none is shown.
    animate: Option<Instant>,
    /// When the server started: the spinner's clock.
    born: Instant,
    /// The team's name, for the bar.
    team: String,
    /// Most columns of members in a tab.
    columns: usize,
    /// The journal shows its last messages only.
    reduced: bool,
    /// What opens the journal again once hidden (⌥j), as `launch` gave it.
    journal: Option<PaneSpec>,
    /// The screen's selection, in one pane; whether its pane was on its alternate screen when it began.
    selection: Option<(Selection, bool)>,
    /// The left button went down to select: its drag and its release select too.
    selecting: bool,
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
    /// Frames sent to clients, and bytes written to them, since the server started: `_ctl stats`.
    frames: u64,
    /// The members in the team's order, for their colors (`member_color`).
    order: Vec<String>,
    written: u64,
    /// What each member's header shows besides its state (model, effort, context), as `watch_looks` last read it.
    looks: BTreeMap<String, board::MemberLook>,
    /// When a time in a header shown next changes (12s, 13s; 22m, 23m); `None` when none shows.
    clock: Option<Instant>,
    /// A member gone waiting out of sight (F5), or whose turn failed: its notice, until when it shows.
    notice: Option<(chrome::Notice, Instant)>,
    /// The failed turns told by one notice: since when, and whose. Others within [`BURST`] join it, none other shows.
    burst: Option<(Instant, Vec<String>)>,
    /// A failed turn's reason when its text is not known (as the dashboard saw it): « erreur d'API ».
    unknown_error: String,
    /// Where the notice can be clicked, as last drawn.
    notice_zone: Option<Rect>,
    /// Where the headers' parts can be clicked, by pane, as last drawn.
    parts: Vec<(Rect, String, chrome::Part)>,
    /// What the pointer is over, among what a click reaches (F3).
    hover: Option<Hover>,
    /// The share of its window, in percent, from which a member at rest is compacted on its own; `None`: off, or not
    /// read yet (`watch_looks` tells it).
    auto_compact: Option<u8>,
    /// How long a member rests before that, in seconds: [`crate::config::AUTO_COMPACT_IDLE`], or [`AUTO_IDLE`].
    auto_idle: u64,
    /// The members asked to compact in their rest, by member (see [`Asked`]).
    asked: BTreeMap<String, Asked>,
    /// How those requests went, as the members' reports last said it.
    outcomes: BTreeMap<String, bridge::Compaction>,
    /// The members whose outcome `watch_looks` reads: those asked and not closed.
    watched: Arc<Mutex<Vec<String>>>,
    /// Something the automatic compaction and the tabs in « focus » depend on may have changed (a state, the looks, an
    /// answer, a client, a tab, a key, the pointer): they look again at the end of this turn, not at each frame.
    look_again: bool,
    /// The client's terminal has a light background (its OSC 11 answer), so the light palette: as of its attach.
    light: bool,
    /// The team's tabs of agents are in « focus » (`layout = "focus"`), as `Build` and `Arrange` say.
    focus_layout: bool,
    /// The cards of the members left out of the tab shown in « focus », best placed first, with their panes and the
    /// time each began its state (`since` ticks in them); rebuilt when the looks, a state or the places change.
    cards: Vec<board::CardData>,
    card_ids: Vec<String>,
    card_epoch: Vec<Option<u64>>,
    cards_stale: bool,
    /// When the tabs in « focus » next look at their places, for a place that comes due; `None` when none does.
    focus_wake: Option<Instant>,
    /// Where the pointer last was, on the screen, and when it last moved: nothing moves in a tab under it ([`POINTER_HOLD`]).
    pointer: Option<(usize, usize)>,
    pointer_at: Option<Instant>,
    /// The last key typed, the last click: nothing moves for a second after them.
    last_key: Option<Instant>,
    last_click: Option<Instant>,
    /// The last press on the header of a pane on the right of a tab in « focus »: a second one brings it to the left.
    header_click: Option<(Instant, String)>,
    /// When the automatic compaction next looks, for the rest that is not long enough yet; `None` when none waits.
    compaction: Option<Instant>,
}

/// The menu in its list mode (lot of 2026-10-11, direction A): the values of a member's model or effort, in a pane
/// floating under the text of the header clicked, nothing dimmed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct List {
    /// The member's pane whose header was clicked.
    pane: String,
    /// The part of its header: the model or the effort. Kept in inverse video while the list is open.
    part: chrome::Part,
    /// Where that text is on the screen.
    anchor: Rect,
    /// The list's size, its own frame included (it draws it): estimated at first, then as the menu tells it (`CSI 8 ; rows ; cols t`).
    size: (usize, usize),
}

/// What the pointer can be over, lit (F3).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Hover {
    /// A part of a pane's header, by its pane.
    Part(String, chrome::Part),
    /// A tab, a button of the bar.
    Bar(Target),
}

/// How long before a compaction asked is asked again, when it was refused for a subagent or a teammate at work, or got
/// no answer (decision of 2026-10-11).
const AUTO_AGAIN: Duration = Duration::from_secs(5 * 60);

/// How long a request is waited for, in seconds' worth of time, before it counts as unanswered: it lapses after
/// `COMPACTION_LAPSE` (10 s), its mod answers at its next tick, and `watch_looks` reads the report every [`LOOKS`].
const AUTO_PENDING: Duration = Duration::from_secs(30);

/// How long a member stays at rest after it was asked to compact on its own, at most, whatever its program says
/// (decision of 2026-10-11): its answer, `done`, `skip` or `error`, ends it sooner.
const COMPACTING: Duration = Duration::from_secs(3 * 60);

/// A member asked to compact on its own, in the rest that began at `since`.
struct Asked {
    /// The rest it was asked in: a new turn starts another.
    since: u64,
    /// The request's id, once written: its report's `compaction` answers with it.
    id: Option<String>,
    /// When it was asked last.
    at: Instant,
    /// Its program said another state than `working` since: the compaction is over, a `working` is a turn's again.
    ended: bool,
    /// The earlier requests of this rest that got no answer, failed, or were refused for another reason than a
    /// subagent or a teammate at work (those do not count).
    failures: u8,
}

/// Set, in seconds, the time a member rests before its context is compacted on its own (tests). Not documented.
const AUTO_IDLE: &str = "RECRUIT_AUTO_COMPACT_IDLE";

/// A list's size before the menu tells its own: six values and the frame, columns and rows.
const LIST_SIZE: (usize, usize) = (26, 8);

/// How often the members' looks are read for their headers.
const LOOKS: Duration = Duration::from_secs(2);

/// How long a tab in « focus » stays as it is after the pointer moved over a pane on the right or the cards.
const POINTER_HOLD: Duration = Duration::from_secs(5);

/// The id of the cards' frame among the views: no pane has it.
const CARDS: &str = "cards";

/// Two presses on a header this close are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How long a notice shows (F5).
const NOTICE: Duration = Duration::from_secs(6);

/// Failed turns this close to the first one noticed share its notice (an API outage often hits the whole team).
const BURST: Duration = Duration::from_secs(10);

/// `recruit _server <state>`: starts the team's server, detached, and returns once it is ready.
pub(crate) fn run(state: &Path) -> Result<()> {
    fs::create_dir_all(state).with_context(|| t!("création de {}", "creating {}", state.display()))?;
    // Absolute: the server leaves for `/`.
    let state = state.canonicalize().with_context(|| state.display().to_string())?;
    let session = state
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .context(t!("dossier d'état sans nom", "state directory without a name"))?;
    // A fork with another thread running could leave a lock held for good in the child (the allocator's, one of
    // std's): refused rather than risked.
    if let Some(count) = threads()
        && count > 1
    {
        bail!(t!(
            "recruit _server : {} fils tournent déjà, le serveur ne peut pas se détacher sans risque (sous macOS, donner la langue : recruit --lang fr _server …)",
            "recruit _server: {} threads already run, the server cannot detach safely (on macOS, give the language: recruit --lang en _server …)",
            count
        ));
    }
    close_inherited();
    let (mut ready, told) = io::pipe().context("pipe")?;
    // SAFETY: no thread runs yet (main.rs starts none before `_server`): what runs after fork is this thread alone.
    match unsafe { libc::fork() } {
        -1 => Err(io::Error::last_os_error()).context("fork"),
        0 => {
            drop(ready);
            daemon(&state, &session, told)
        }
        child => {
            drop(told);
            let mut said = String::new();
            let _ = ready.read_to_string(&mut said);
            let mut status = 0;
            // SAFETY: reaps the middle process, which leaves at once.
            unsafe { libc::waitpid(child, &mut status, 0) };
            match said.trim_end() {
                "ok" => Ok(()),
                "" => bail!(t!(
                    "le serveur de l'équipe s'est arrêté au démarrage : {}",
                    "the team's server stopped while starting: {}",
                    log_tail(&state)
                )),
                error => bail!("{error}"),
            }
        }
    }
}

/// The threads of this process, when the system says.
fn threads() -> Option<usize> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: proc_pidinfo fills a proc_taskinfo of the size given.
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        let pid = std::process::id() as libc::c_int;
        let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, (&raw mut info).cast(), size) };
        (got == size).then_some(info.pti_threadnum as usize)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(fs::read_dir("/proc/self/task").ok()?.count())
    }
}

/// The last lines of the server's log, for an error.
fn log_tail(state: &Path) -> String {
    let text = fs::read_to_string(state.join("server.log")).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// Closes the descriptors from 3 up inherited from the launcher (§1.1): the server keeps none of its terminal's, its
/// pipes, its sockets.
fn close_inherited() {
    let dir = if Path::new("/dev/fd").is_dir() { "/dev/fd" } else { "/proc/self/fd" };
    let Ok(entries) = fs::read_dir(dir) else { return };
    let fds: Vec<libc::c_int> =
        entries.filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok()).filter(|fd| *fd > 2).collect();
    // The listing's own descriptor is closed by now: closing it again only fails.
    for fd in fds {
        // SAFETY: nothing of this process uses these descriptors: they are the launcher's.
        unsafe { libc::close(fd) };
    }
}

/// In the first child: a new session, then the server in a grandchild, which never leads a session and so can never
/// get a controlling terminal. Never returns.
fn daemon(state: &Path, session: &str, told: io::PipeWriter) -> ! {
    // SAFETY: still a single thread.
    unsafe {
        libc::setsid();
        match libc::fork() {
            -1 => {
                let _ = writeln!(&told, "fork: {}", io::Error::last_os_error());
                libc::_exit(1)
            }
            0 => {}
            _ => libc::_exit(0),
        }
    }
    std::process::exit(start(state, session, told))
}

/// The server: set up, said ready on `told`, then the loop until the team stops; its exit code. An error before it
/// is ready goes on `told` only, for `_server` to say: the server's stderr may still be the launcher's.
fn start(state: &Path, session: &str, told: io::PipeWriter) -> i32 {
    let ready = match prepare(state, session) {
        Ok(ready) => ready,
        Err(error) => {
            let _ = writeln!(&told, "{error:#}");
            return 1;
        }
    };
    let (server, rx, _lock) = ready;
    let _ = writeln!(&told, "ok");
    drop(told);
    log(&format!("ready, pid {}, socket {}", std::process::id(), server.socket.display()));
    match server.run(rx) {
        Ok(()) => 0,
        Err(error) => {
            log(&format!("{error:#}"));
            1
        }
    }
}

type Ready = (Server, Receiver<Msg>, File);

/// Everything before the loop (§1.1, points 4 to 9).
fn prepare(state: &Path, session: &str) -> Result<Ready> {
    std::env::set_current_dir("/").context("chdir /")?;
    redirect(0, Path::new("/dev/null"), false)?;
    let lock = lock(state)?;
    // A dead server's leftovers: `server.json` then names no live server, the lock being ours. It died without
    // stopping: the mark is kept for the launch (`socket::mark_crashed`).
    if let Some(info) = socket::read_info(state) {
        socket::mark_crashed(state, &info);
    }
    let _ = fs::remove_file(socket::info_file(state));
    let journal = state.join("server.log");
    if fs::metadata(&journal).is_ok_and(|meta| meta.len() > 1 << 20) {
        let _ = fs::rename(&journal, state.join("server.log.1"));
    }
    redirect(1, &journal, true)?;
    redirect(2, &journal, true)?;
    block_signals();
    std::panic::set_hook(Box::new(|info| {
        log(&format!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture()));
        // An engine's panic is caught where it happens, and that pane only is repaired (§1.4).
        if !super::catching() {
            std::process::exit(PANICKED);
        }
    }));

    let dir = socket::dir()?;
    let path = socket::path(&dir, session, state)?;
    let (listener, socket_id) = socket::listen(&path)?;
    let info = Info {
        pid: std::process::id(),
        socket: path.clone(),
        proto: proto::PROTO,
        version: proto::VERSION.to_string(),
        started: now(),
    };
    socket::write_info(state, &info)?;

    let (tx, rx) = mpsc::channel();
    let welcome = Arc::new(Mutex::new(Welcome {
        proto: proto::PROTO,
        version: proto::VERSION.to_string(),
        session: session.to_string(),
        pid: std::process::id(),
        ..Welcome::default()
    }));
    let signals = tx.clone();
    std::thread::Builder::new().name("signals".into()).spawn(move || wait_signals(&signals))?;
    accept(listener, tx.clone(), Arc::clone(&welcome))?;
    let server = Server::new(state, session, (path, socket_id), tx.clone(), welcome);
    watch_looks(state.to_path_buf(), tx, Arc::clone(&server.watched))?;
    Ok((server, rx, lock))
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
            grab: None,
            grab_button: Button::Left,
            grab_at: (0, 0),
            swallowing: false,
            dirty: false,
            last_frame: None,
            typed: None,
            echo: false,
            deadline: None,
            animate: None,
            born: Instant::now(),
            team: session.to_string(),
            columns: COLUMNS,
            reduced: true,
            journal: None,
            selection: None,
            selecting: false,
            exe: None,
            lang: crate::i18n::lang().code().to_string(),
            menu: None,
            list: None,
            choice: None,
            quitting: false,
            frames: 0,
            order: Vec::new(),
            written: 0,
            looks: BTreeMap::new(),
            clock: None,
            notice: None,
            burst: None,
            unknown_error: look::error_label(None),
            notice_zone: None,
            parts: Vec::new(),
            hover: None,
            auto_compact: None,
            auto_idle: std::env::var(AUTO_IDLE)
                .ok()
                .and_then(|secs| secs.parse().ok())
                .unwrap_or(crate::config::AUTO_COMPACT_IDLE),
            asked: BTreeMap::new(),
            outcomes: BTreeMap::new(),
            watched: Arc::new(Mutex::new(Vec::new())),
            look_again: false,
            cards: Vec::new(),
            card_ids: Vec::new(),
            card_epoch: Vec::new(),
            cards_stale: false,
            light: false,
            focus_layout: false,
            focus_wake: None,
            pointer: None,
            pointer_at: None,
            last_key: None,
            last_click: None,
            header_click: None,
            compaction: None,
        }
    }
}

/// The state folder's lock, waited for a little: a server that stops lets it go as it leaves.
fn lock(state: &Path) -> Result<File> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(lock) = socket::try_lock(state)? {
            return Ok(lock);
        }
        if Instant::now() >= until {
            bail!(t!("l'équipe tourne déjà", "the team is already running"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Puts `file` on descriptor `fd`: read, or appended to.
fn redirect(fd: libc::c_int, file: &Path, append: bool) -> Result<()> {
    let handle = if append { fs::OpenOptions::new().create(true).append(true).open(file) } else { File::open(file) }
        .with_context(|| file.display().to_string())?;
    // SAFETY: dup2 onto a standard descriptor; the copy is not CLOEXEC, as a standard descriptor should be.
    if unsafe { libc::dup2(handle.as_raw_fd(), fd) } < 0 {
        return Err(io::Error::last_os_error()).context("dup2");
    }
    Ok(())
}

/// Seconds since the epoch.
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Every [`LOOKS`], the members' looks for their headers (`board::looks`: a few small files), to the loop when they
/// changed: never on the way to a frame, and no message while nothing changes. The team's automatic compaction goes
/// with them (`team.json`, which a change from the menu rewrites).
fn watch_looks(state: PathBuf, tx: Sender<Msg>, watched: Arc<Mutex<Vec<String>>>) -> io::Result<()> {
    std::thread::Builder::new().name("looks".into()).spawn(move || {
        let mut last = BTreeMap::new();
        let mut compact = None;
        let mut outcomes = BTreeMap::new();
        loop {
            // A panic in its reading would end the server (the hook): caught, and tried again later.
            match super::caught(|| (board::looks(&state), Snapshot::read(&state).ok().map(|s| s.auto_compact))) {
                Ok((looks, share)) => {
                    if share.is_some() && share != compact {
                        compact = share;
                        if tx.send(Msg::AutoCompact(share.flatten())).is_err() {
                            return;
                        }
                    }
                    if looks != last {
                        if tx.send(Msg::Looks(looks.clone())).is_err() {
                            return;
                        }
                        last = looks;
                    }
                    // How the compactions asked went: their reports, for the members asked only.
                    let asked = watched.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    let said: BTreeMap<_, _> = asked
                        .into_iter()
                        .filter_map(|member| Some((member.clone(), bridge::report(&state, &member)?.compaction?)))
                        .collect();
                    if said != outcomes {
                        if tx.send(Msg::Outcomes(said.clone())).is_err() {
                            return;
                        }
                        outcomes = said;
                    }
                }
                Err(why) => log(&format!("looks: {why}")),
            }
            std::thread::sleep(LOOKS);
        }
    })?;
    Ok(())
}

/// A line in `server.log`.
fn log(text: &str) {
    eprintln!("{} recruit[{}]: {text}", now(), std::process::id());
}

/// Where a list goes on a screen of `screen` columns and rows (the bar left out), under the text at `anchor`, its
/// values aligned on it (they start four columns inside the frame: the frame goes four columns to the left, the
/// mock-up's): moved left just enough to end at the screen's edge; above the text when there is no room below; never
/// out of the screen.
fn list_area(anchor: Rect, size: (usize, usize), screen: (usize, usize)) -> Rect {
    let width = size.0.min(screen.0).max(1);
    let x = anchor.x.saturating_sub(4).min(screen.0.saturating_sub(width));
    let below = screen.1.saturating_sub(anchor.y + 1);
    let above = anchor.y.min(screen.1);
    let (y, height) = if size.1 <= below {
        (anchor.y + 1, size.1)
    } else if size.1 <= above {
        (anchor.y - size.1, size.1)
    } else if below >= above {
        (anchor.y + 1, below.max(1))
    } else {
        (anchor.y - above, above.max(1))
    };
    let height = height.min(screen.1).max(1);
    Rect { x, y: y.min(screen.1.saturating_sub(height)), width, height }
}

/// A compaction skipped because the member was not in a state to be compacted (a subagent or a teammate at work, a turn
/// running): not a failed try, the member is asked again later. Any other skip (a hook that refuses) is one.
fn not_a_failure(outcome: &bridge::Compaction) -> bool {
    matches!(outcome.skip.as_deref(), Some(bridge::SKIP_HELPERS | bridge::SKIP_BUSY))
}

/// Writes a member's automatic compaction request (`bridge::request_auto_compaction`: a small file, which its mod takes
/// at its next tick) and tells the loop its id, or why not.
fn ask_compaction(state: &Path, tx: &Sender<Msg>, member: &str, percent: u8, since: u64) {
    let id = bridge::request_auto_compaction(state, member, percent).map_err(|error| format!("{error:#}"));
    let _ = tx.send(Msg::Requested(member.to_string(), since, id));
}

/// The signals the server takes (§1.5): blocked in every thread, waited for by one.
const SIGNALS: [libc::c_int; 4] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGURG];

fn signal_set() -> libc::sigset_t {
    // SAFETY: a set filled by sigemptyset and sigaddset.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in SIGNALS {
            libc::sigaddset(&mut set, signal);
        }
        set
    }
}

/// Blocks the server's signals, before any thread: every thread inherits the mask.
fn block_signals() {
    let set = signal_set();
    // SAFETY: the mask of this thread, the only one.
    unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
}

/// Hands each signal to the loop.
fn wait_signals(tx: &Sender<Msg>) {
    let set = signal_set();
    loop {
        let mut signal = 0;
        // SAFETY: sigwait on blocked signals.
        if unsafe { libc::sigwait(&set, &mut signal) } != 0 {
            continue;
        }
        if tx.send(Msg::Signal(signal)).is_err() {
            return;
        }
    }
}

/// Accepts connections on `listener`, each served by a thread of its own.
fn accept(listener: UnixListener, tx: Sender<Msg>, welcome: Arc<Mutex<Welcome>>) -> io::Result<()> {
    static CLIENTS: AtomicU64 = AtomicU64::new(0);
    std::thread::Builder::new().name("accept".into()).spawn(move || {
        // Under macOS, `accept` then `FD_CLOEXEC` is not atomic: a connection accepted while a pane forks may leak
        // into it until it execs. Rare, and the program gets a socket it does not know of.
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(error) => {
                    // A lasting one (EMFILE, ENFILE) would spin: said, then a pause.
                    log(&format!("accept: {error}"));
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            if !socket::same_user(&stream) {
                log(&format!("connection refused: uid {:?}", socket::peer_uid(&stream).ok()));
                continue;
            }
            let (tx, welcome) = (tx.clone(), Arc::clone(&welcome));
            let spawned = std::thread::Builder::new().name("connection".into()).spawn(move || {
                if let Err(error) = connection(stream, &tx, &welcome, &CLIENTS) {
                    log(&format!("connection: {error}"));
                }
            });
            if let Err(error) = spawned {
                log(&format!("connection: {error}"));
            }
        }
    })?;
    Ok(())
}

/// A connection, from its `Hello` to its end.
fn connection(
    mut stream: UnixStream,
    tx: &Sender<Msg>,
    welcome: &Mutex<Welcome>,
    clients: &AtomicU64,
) -> io::Result<()> {
    stream.set_read_timeout(Some(HELLO_TIME))?;
    let Some(hello) = proto::recv::<Hello>(&mut stream)? else { return Ok(()) };
    let answer = welcome.lock().unwrap_or_else(PoisonError::into_inner).clone();
    proto::send(&mut stream, &answer)?;
    let ours = hello.proto == proto::PROTO;
    match hello.kind {
        Kind::Command => {
            let request = match proto::read_frame(&mut stream)? {
                Some(proto::Frame::Json(json)) => serde_json::from_slice::<Request>(&json),
                _ => return Ok(()),
            };
            // Another protocol: only `Stop`, frozen, is taken.
            let reply = match request {
                Ok(request) if ours || request == Request::Stop => return command(stream, tx, request),
                Ok(_) | Err(_) if !ours => Reply::Refused(Refusal::Proto),
                Ok(_) => unreachable!("taken above"),
                Err(error) => Reply::Err(format!("request: {error}")),
            };
            proto::send(&mut stream, &reply)
        }
        Kind::Attach if !ours => proto::send(&mut stream, &ServerMsg::Refused(Refusal::Proto)),
        Kind::Attach => {
            let Some(ClientMsg::Attach { caps, cols, rows, term, env }) = proto::recv::<ClientMsg>(&mut stream)? else {
                return Ok(());
            };
            stream.set_read_timeout(None)?;
            socket::set_buffer(&stream, libc::SO_SNDBUF, socket::BUFFER);
            let id = format!("c{}", clients.fetch_add(1, Ordering::Relaxed) + 1);
            let out = Arc::new(Outbox::default());
            let writer = stream.try_clone()?;
            let (writer_out, writer_tx, writer_id) = (Arc::clone(&out), tx.clone(), id.clone());
            std::thread::Builder::new()
                .name(format!("write-{id}"))
                .spawn(move || write_client(writer, &writer_out, &writer_tx, &writer_id))?;
            let attaching =
                Attaching { id: id.clone(), stream: stream.try_clone()?, out, caps, size: (cols, rows), term, env };
            if tx.send(Msg::Attach(Box::new(attaching))).is_err() {
                return Ok(());
            }
            // What the client says, until it goes.
            loop {
                match proto::recv::<ClientMsg>(&mut stream) {
                    Ok(Some(message)) => {
                        if tx.send(Msg::Client(id.clone(), message)).is_err() {
                            return Ok(());
                        }
                    }
                    Ok(None) | Err(_) => {
                        let _ = tx.send(Msg::Gone(id));
                        return Ok(());
                    }
                }
            }
        }
        Kind::Other => proto::send(&mut stream, &Reply::Refused(Refusal::Other)),
    }
}

/// A command handed to the loop; its answer written, then said written.
fn command(mut stream: UnixStream, tx: &Sender<Msg>, request: Request) -> io::Result<()> {
    let (reply_tx, reply) = mpsc::channel();
    let (written_tx, written) = mpsc::channel();
    if tx.send(Msg::Command(request, reply_tx, written)).is_err() {
        return proto::send(&mut stream, &Reply::Err(t!("le serveur s'arrête", "the server is stopping")));
    }
    let reply = reply.recv().unwrap_or_else(|_| Reply::Err(t!("le serveur s'arrête", "the server is stopping")));
    let result = proto::send(&mut stream, &reply);
    let _ = written_tx.send(());
    result
}

/// A client's writer: what the loop gives it, in order; it says when it has written all.
fn write_client(stream: UnixStream, out: &Outbox, tx: &Sender<Msg>, id: &str) {
    write_all(stream, out, tx, id);
    out.set_closed();
}

fn write_all(mut stream: UnixStream, out: &Outbox, tx: &Sender<Msg>, id: &str) {
    loop {
        let batch: Vec<Out> = {
            let mut queue = out.queue.lock().unwrap_or_else(PoisonError::into_inner);
            while queue.is_empty() {
                queue = out.ready.wait(queue).unwrap_or_else(PoisonError::into_inner);
            }
            queue.drain(..).collect()
        };
        for item in batch {
            let written = match item {
                Out::Json(json) => proto::send_json(&mut stream, &json),
                Out::Output(bytes) => proto::send_output(&mut stream, &bytes),
                Out::Close => {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return;
                }
            };
            if written.is_err() {
                // The reader sees the end too, and says the client gone.
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
        }
        if tx.send(Msg::Ready(id.to_string())).is_err() {
            return;
        }
    }
}

impl Server {
    fn run(mut self, rx: Receiver<Msg>) -> Result<()> {
        loop {
            let now = Instant::now();
            let frame_due =
                self.dirty.then(|| if self.echo { now } else { self.last_frame.map_or(now, |at| at + FRAME) });
            let notice = self.notice.as_ref().map(|(_, until)| *until);
            let due = [frame_due, self.deadline, self.animate, self.clock, notice, self.compaction, self.focus_wake];
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
                    Msg::Signal(signal) => log(&format!("signal {signal}")),
                    other => self.message(other),
                }
            }
            drop(held);
            if stop.is_none() && std::mem::take(&mut self.quitting) {
                stop = Some((Bye::Stopped, None));
            }
            if stop.is_none() && self.started && !self.panes.iter().any(|pane| pane.role.is_empty()) {
                stop = Some((Bye::Ended, None));
            }
            if let Some((bye, asker)) = stop {
                self.stop(&rx, bye, asker);
                return Ok(());
            }
            let now = Instant::now();
            if self.animate.is_some_and(|at| at <= now) {
                self.animate = None;
                self.dirty = true;
            }
            if self.clock.is_some_and(|at| at <= now) {
                self.clock = None;
                self.dirty = true;
            }
            self.check_notice();
            self.expire();
            self.repair();
            self.frame();
            // After the frame: the programs' states are noted as it takes their relays (`take_relays`).
            let again = std::mem::take(&mut self.look_again);
            if again || self.compaction.is_some_and(|at| at <= Instant::now()) {
                self.auto_compaction();
            }
            if again || self.focus_wake.is_some_and(|at| at <= Instant::now()) {
                self.update_focus();
            }
            if std::mem::take(&mut self.cards_stale) {
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
                if self.focus.as_deref() == Some(id.as_str()) && self.typed.take().is_some_and(|at| at.elapsed() < ECHO)
                {
                    self.echo = true;
                }
                self.dirty = true;
            }
            Msg::Exited(id, which, code) => {
                let Some(i) = self.panes.iter().position(|pane| pane.id == id && Arc::ptr_eq(&pane.broken, &which))
                else {
                    return;
                };
                log(&format!("pane {id} ({}) ended: {code:?}", self.panes[i].member));
                // Failed: what it said last (a panic's message), before its pane goes with it.
                if code != Some(0) {
                    let rows = self.panes[i].size.1;
                    let said = self.panes[i].with(|engine| last_words(engine, rows)).unwrap_or_default();
                    if !said.is_empty() {
                        log(&format!("pane {id} said: {said}"));
                    }
                }
                self.panes[i].exited = true;
                self.close_pane(&id);
            }
            Msg::Broken => self.dirty = true,
            Msg::Looks(looks) => {
                self.cards_stale = true;
                // A member whose program says nothing, gone waiting or failed out of sight by the dashboard: a notice,
                // as by its program (`take_relays`).
                let shown = self.in_view_ids();
                let mut called = Vec::new();
                for pane in self.panes.iter().filter(|pane| pane.role.is_empty() && !shown.contains(&pane.id)) {
                    let state = pane_state(pane, &looks);
                    if calls(state) && pane_state(pane, &self.looks) != state {
                        called.push((pane.id.clone(), state));
                    }
                }
                self.looks = looks;
                self.dirty = true;
                self.call(called);
            }
            Msg::AutoCompact(share) => self.auto_compact = share,
            Msg::Requested(member, since, id) => {
                if let Some(asked) = self.asked.get_mut(&member).filter(|asked| asked.since == since) {
                    match id {
                        Ok(id) => asked.id = Some(id),
                        Err(error) => log(&format!("auto-compaction: {member}: {error}")),
                    }
                }
            }
            Msg::Outcomes(outcomes) => self.outcomes = outcomes,
            Msg::Attach(attaching) => self.attach(*attaching),
            Msg::Client(id, message) => {
                if self.client.as_ref().is_some_and(|client| client.id == id) {
                    self.client_message(message);
                }
            }
            Msg::Gone(id) => {
                if self.client.as_ref().is_some_and(|client| client.id == id) {
                    log(&format!("client {id} gone"));
                    let _ = self.drop_client(None);
                }
            }
            Msg::Ready(id) => {
                if let Some(client) = self.client.as_mut().filter(|client| client.id == id) {
                    client.busy = false;
                    if std::mem::take(&mut client.late) {
                        self.dirty = true;
                        self.last_frame = None;
                    }
                }
            }
            Msg::Command(..) | Msg::Signal(_) => {}
        }
    }

    /// A new client: it takes the team over from the one that had it (spec §10, question 3: A).
    fn attach(&mut self, attaching: Attaching) {
        let Attaching { id, stream, out, caps, size, term, env } = attaching;
        if self.client.is_some() {
            let _ = self.drop_client(Some(Bye::Replaced));
        }
        log(&format!("client {id} attached: {term}, {}x{}", size.0, size.1));
        self.env = proto::UPDATE_ENV
            .iter()
            .map(|name| (name.to_string(), env.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone())))
            .collect();
        // Its background tells the palette (dark when it said none), and the programs that ask it the theme (`CSI ? 996 n`,
        // mode 2031): told the one they do not know yet.
        let light = caps.bg.is_some_and(look::light_background);
        for pane in &self.panes {
            let mut replies = Vec::new();
            pane.with(|engine| {
                engine.set_colors(caps.fg, caps.bg);
                engine.color_scheme(light, &mut replies);
            });
            if !replies.is_empty() {
                pane.send(replies);
            }
        }
        look::set_light(light);
        self.theme(light);
        out.message(&ServerMsg::Attached { client: id.clone() });
        let painter = Painter::new(caps.output);
        self.client = Some(Client {
            id,
            stream,
            out,
            caps,
            term,
            size,
            since: now(),
            painter,
            busy: false,
            late: false,
            motion: None,
            relays: Relays::default(),
        });
        self.set_attached();
        self.resize(size);
    }

    /// The client's theme is `light` (or dark). Changed since the client before: the whole image is drawn again, the
    /// cells kept by the screen were painted with the other palette. True when it changed.
    fn theme(&mut self, light: bool) -> bool {
        if light == self.light {
            return false;
        }
        self.light = light;
        self.screen.borrow_mut().redraw();
        self.dirty = true;
        true
    }

    /// Lets the client go: told why, when it is still there to hear it.
    /// Returns its outbox, to wait for its writer to leave.
    fn drop_client(&mut self, bye: Option<Bye>) -> Option<Arc<Outbox>> {
        // The menu, and a choice, were opened for this client: they would act for one that is gone.
        if let Some((menu, _)) = self.menu.clone() {
            self.close_pane(&menu);
        }
        self.choice = None;
        // Its pointer with it, and what it chose with ⌥l: the tabs go back to what the team says.
        self.hover = None;
        self.pointer = None;
        if self.tabs.iter().any(|tab| tab.mode.is_some()) {
            for tab in &mut self.tabs {
                tab.mode = None;
            }
            self.layout();
        }
        let client = self.client.take()?;
        self.set_attached();
        let Client { out, stream, .. } = client;
        match bye {
            Some(bye) => {
                out.message(&ServerMsg::Bye(bye));
                out.push(Out::Close);
                // Shut if it does not read its goodbye in time: its writer may be stuck in a write (a dead SSH).
                let watched = Arc::clone(&out);
                let spawned = std::thread::Builder::new().name("bye".into()).spawn(move || {
                    if !watched.wait_closed(BYE_TIME) {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                });
                if let Err(error) = spawned {
                    log(&format!("bye: {error}"));
                }
            }
            // Gone: its writer may be stuck in a write; the shut wakes it.
            None => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                out.push(Out::Close);
            }
        }
        Some(out)
    }

    fn set_attached(&self) {
        let mut welcome = self.welcome.lock().unwrap_or_else(PoisonError::into_inner);
        welcome.attached = self.client.is_some();
    }

    fn client_message(&mut self, message: ClientMsg) {
        match message {
            ClientMsg::Attach { .. } => {}
            ClientMsg::Input(event) => self.input(event),
            ClientMsg::Redraw => {
                if let Some(client) = self.client.as_mut() {
                    client.painter.forget();
                }
                self.dirty = true;
            }
            ClientMsg::Detach => {
                let _ = self.drop_client(Some(Bye::Detached));
            }
        }
    }

    fn focused(&self) -> Option<&Pane> {
        let id = self.focus.as_deref()?;
        self.panes.iter().find(|pane| pane.id == id)
    }

    fn pane(&self, id: &str) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    /// The member whose pane has the focus; `None` for a panel or the menu.
    fn focused_member(&self) -> Option<&str> {
        self.focused().filter(|pane| pane.role.is_empty()).map(|pane| pane.member.as_str())
    }

    /// An event from the client: the server's keys first, then the focused pane's.
    fn input(&mut self, event: Event) {
        // A choice open takes the keys and the clicks; nothing reaches the panes meanwhile.
        if self.choice.is_some() {
            match event {
                Event::Key(key) => {
                    let outcome = self.choice.as_mut().and_then(|open| open.key(&key));
                    self.dirty = true;
                    if let Some(outcome) = outcome {
                        self.decide(outcome);
                    }
                    return;
                }
                Event::Mouse(mouse) => {
                    let (col, row) = (mouse.col as usize, mouse.row as usize);
                    match mouse.kind {
                        MouseKind::Down(_) => {
                            let outcome = self.choice.as_ref().map(|open| open.click(col, row));
                            if let Some(outcome) = outcome {
                                self.decide(outcome);
                                // Its release, and what comes before it, belong to this click.
                                self.swallowing = true;
                            }
                        }
                        // Over an option, the selection follows the pointer, as in tmux's menus.
                        MouseKind::Moved | MouseKind::Drag(_)
                            if self.choice.as_mut().is_some_and(|open| open.hover(col, row)) =>
                        {
                            self.dirty = true;
                        }
                        _ => {}
                    }
                    return;
                }
                Event::Paste(_) => return,
                Event::Focus(_) | Event::Resize(..) | Event::Closed(_) => {}
            }
        }
        match event {
            Event::Key(key) => {
                self.last_key = Some(Instant::now());
                match keys::shortcut(&key) {
                    Some(Shortcut::Run(action)) => {
                        self.act(action);
                        self.dirty = true;
                        return;
                    }
                    Some(Shortcut::Swallow) => return,
                    None => {}
                }
                let Some(pane) = self.focused() else { return };
                let modes = pane.modes();
                // Shift with Page Up, Page Down, Home, End scroll the history rather than go to the program.
                let rows = self.cells(&pane.id, pane.area).height.max(1);
                if let Some(lines) = select::scroll_key(&key, rows, modes.alt_screen) {
                    pane.with(|engine| engine.scroll(lines));
                    self.dirty = true;
                    return;
                }
                let bytes = input::key(&key, &modes);
                let typed = !bytes.is_empty();
                if typed {
                    self.unselect_in(self.focus.clone());
                }
                let Some(pane) = self.focused() else { return };
                // A pane that writes on and on has said so already: its next output, the echo most likely, must tell.
                let unscrolled = typed && pane.unscroll();
                if typed {
                    pane.told.store(false, Ordering::Release);
                }
                pane.send(bytes);
                self.dirty |= unscrolled;
                if typed {
                    self.typed = Some(Instant::now());
                }
            }
            Event::Paste(text) => {
                self.unselect_in(self.focus.clone());
                let Some(pane) = self.focused() else { return };
                let unscrolled = pane.unscroll();
                pane.told.store(false, Ordering::Release);
                pane.send(input::paste(&text, &pane.modes()));
                self.dirty |= unscrolled;
                self.typed = Some(Instant::now());
            }
            Event::Mouse(mouse) => self.mouse(&mouse),
            Event::Focus(gained) => {
                if let Some(pane) = self.focused() {
                    pane.send(input::focus(gained, &pane.modes()).unwrap_or_default());
                }
            }
            Event::Resize(cols, rows) => self.resize((cols, rows)),
            Event::Closed(_) => {
                let _ = self.drop_client(None);
            }
        }
    }

    /// What a shortcut does (`keys.rs`). While the menu is open, only the menu's own keys do something: the rest
    /// would take the focus from it.
    fn act(&mut self, action: Action) {
        if self.menu.is_some() && !matches!(action, Action::Menu | Action::Quit) {
            return;
        }
        match action {
            Action::Tab(tab) => self.show_tab(tab),
            Action::PreviousTab | Action::NextTab => {
                let count = self.tabs.len().max(1);
                let next = if action == Action::NextTab { 1 } else { count - 1 };
                self.show_tab((self.active + next) % count);
            }
            // The members' panes only: the dashboard and the journal are reached by a click (user's decision).
            Action::NextPane => {
                let Some(tab) = self.tabs.get(self.active) else { return };
                // In « focus »: the next member of the tab comes to the left, with the focus.
                if tab.focus.is_some() && tab.zoomed.is_none() {
                    let order = tab.panes.clone();
                    let left = self.tabs[self.active].focus.as_mut().and_then(|focus| {
                        focus.next(&order, Instant::now());
                        focus.places().left.clone()
                    });
                    if let Some(left) = left {
                        self.focus_on(&left);
                    }
                    self.unselect();
                    self.layout();
                    return;
                }
                if let Some(id) = next_member(&tab.panes, self.focus.as_deref()) {
                    // A zoomed tab keeps its zoom: on the next member (F4, decision of 2026-10-11). ⌥z and ⤡ give
                    // the grid back.
                    if tab.zoomed.is_some() {
                        self.tabs[self.active].zoomed = Some(id.clone());
                        self.unselect();
                    }
                    self.focus_on(&id);
                }
            }
            Action::Journal => {
                if let Some(journal) = self.journal.clone()
                    && let Err(error) = self.toggle_journal(journal)
                {
                    log(&format!("journal: {error:#}"));
                }
            }
            Action::Menu => self.menu_here(),
            // The menu open has its own way out.
            Action::Quit if self.menu.is_some() => {}
            Action::Quit => self.ask_quit(),
            Action::Zoom => {
                if let Some(id) = self.focused_member().and(self.focus.clone()) {
                    self.zoom(&id);
                }
            }
            Action::Waiting => self.go_waiting(),
            Action::ToggleLayout => self.toggle_layout(),
        }
    }

    /// What the pointer is over at `col`, `row`, among what a click reaches: lit when it changes (F3). The notice
    /// first, which hides what is under it, then the bar, then the headers.
    fn hover_at(&mut self, col: usize, row: usize) {
        let hover = if self.notice_zone.is_some_and(|zone| zone.contains(col, row)) {
            None
        } else if let Some((_, target)) = self.zones.iter().find(|(zone, _)| zone.contains(col, row)) {
            Some(Hover::Bar(*target))
        } else {
            let part = self.parts.iter().find(|(zone, _, _)| zone.contains(col, row));
            part.map(|(_, id, part)| Hover::Part(id.clone(), *part))
        };
        if hover != self.hover {
            self.hover = hover;
            self.dirty = true;
        }
    }

    /// A press on a part of pane `id`'s header (F2): the name, the model, the effort open the member's sheet in the
    /// menu, on that field; the context of a member at rest offers to compact it; ⤢ and ⤡ zoom.
    fn part_click(&mut self, id: &str, part: chrome::Part) {
        let Some(member) = self.pane(id).filter(|pane| pane.role.is_empty()).map(|pane| pane.member.clone()) else {
            return;
        };
        let field = match part {
            chrome::Part::Name => MenuField::Name,
            chrome::Part::Model | chrome::Part::Effort => {
                let field = if part == chrome::Part::Model { MenuField::Model } else { MenuField::Effort };
                // Under the text clicked, a list of the values; the whole menu if the text is not found.
                let anchor = self.parts.iter().find(|(_, of, p)| of == id && *p == part).map(|(zone, _, _)| *zone);
                if let Some(anchor) = anchor {
                    let list = List { pane: id.to_string(), part, anchor, size: LIST_SIZE };
                    if let Err(error) = self.open_menu_as(None, Some((&member, Some(field))), Some(list)) {
                        log(&format!("menu: {error:#}"));
                    }
                    return;
                }
                field
            }
            chrome::Part::Context => {
                // As the header shows it: no file read here.
                let percent = self.looks.get(&member).and_then(|looks| looks.context);
                self.open_choice(Open::new(chrome::compact_choice(&member, percent), Purpose::Compact { member }));
                return;
            }
            chrome::Part::Zoom => {
                self.zoom(id);
                return;
            }
        };
        if let Err(error) = self.open_menu(None, Some((&member, Some(field)))) {
            log(&format!("menu: {error:#}"));
        }
    }

    /// Member pane `id` takes its tab's room, with the focus; zoomed already, the grid comes back (F4).
    fn zoom(&mut self, id: &str) {
        if !self.pane(id).is_some_and(|pane| pane.role.is_empty()) {
            return;
        }
        let Some(tab) = self.tabs.iter().position(|tab| tab.panes.iter().any(|pane| pane == id)) else { return };
        if self.tabs[tab].zoomed.as_deref() == Some(id) {
            self.tabs[tab].zoomed = None;
        } else {
            self.focus_on(id);
            self.tabs[tab].zoomed = Some(id.to_string());
        }
        self.unselect();
        self.layout();
    }

    /// To the member who has waited the longest (⌥g, F6), else to the one whose turn failed the longest ago (mock-up
    /// C); nothing when none waits nor failed.
    fn go_waiting(&mut self) {
        let looks = &self.looks;
        let rank = |pane: &&Pane| match pane_state(pane, looks) {
            look::State::Waiting => Some(0),
            look::State::Error => Some(1),
            _ => None,
        };
        let calling = self.panes.iter().filter(|pane| pane.role.is_empty()).filter(|pane| rank(pane).is_some());
        let first = calling.min_by_key(|pane| (rank(pane), pane_since(pane, looks).unwrap_or(u64::MAX)));
        if let Some(id) = first.map(|pane| pane.id.clone()) {
            self.focus_on(&id);
        }
    }

    /// A press on the dashboard or the journal, at `x`, `y` of its cells (`columns` wide): the member clicked gets the
    /// focus, in its tab; a member's context offers to compact it, after confirmation. The team's state is read here,
    /// once per click, not on the way of a frame.
    fn panel_click(&mut self, id: &str, x: usize, y: usize, columns: usize) {
        let Some(pane) = self.pane(id) else { return };
        let kind = if pane.role == DASHBOARD { board::Kind::Dashboard } else { board::Kind::Journal };
        // The row clicked as the panel wrote it: a recipient's name is read in it.
        let line = pane
            .with(|engine| engine.line(engine.top() + y as u64).map(|line| line.text.trim_end().to_string()))
            .flatten()
            .unwrap_or_default();
        let snapshot = match Snapshot::read(&self.state) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                log(&format!("click: {error:#}"));
                return;
            }
        };
        match board::clicked(&snapshot, &self.state, kind, x, y, columns, &line) {
            Some(board::Clicked::Show(member)) => {
                let target = self.panes.iter().find(|pane| pane.role.is_empty() && pane.member == member);
                if let Some(target) = target.map(|pane| pane.id.clone()) {
                    self.focus_on(&target);
                }
            }
            Some(board::Clicked::Compact(member)) => {
                self.open_choice(Open::new(
                    chrome::compact_choice(&member, board::context_percent(&self.state, &member)),
                    Purpose::Compact { member },
                ));
            }
            None => {}
        }
        self.dirty = true;
    }

    /// The quit choice, for the client: detach it, stop the team, or cancel.
    fn ask_quit(&mut self) {
        let Some(client) = self.client.as_ref().map(|client| client.id.clone()) else { return };
        self.open_choice(Open::new(chrome::quit_choice(&self.team), Purpose::Quit { client }));
    }

    /// Opens a choice over the team. A pane whose program took a press still down gets its release now: the choice
    /// takes the real one, and the program would stay in a drag.
    fn open_choice(&mut self, open: Open) {
        if let Some(id) = self.grab.take()
            && let Some(pane) = self.pane(&id)
            && let Some(bytes) = choice::release(self.grab_button, self.grab_at, &pane.modes())
        {
            pane.send(bytes);
        }
        self.unselect();
        self.choice = Some(open);
        self.dirty = true;
    }

    /// What a choice's end does: its option, by its index in the choice's order.
    fn decide(&mut self, outcome: Outcome) {
        let Some(open) = self.choice.take() else { return };
        self.dirty = true;
        let Outcome::Picked(index) = outcome else { return };
        match (open.purpose, index) {
            // Detach, quit, cancel (`chrome::quit_choice`).
            (Purpose::Quit { client }, 0) => {
                if self.client.as_ref().is_some_and(|attached| attached.id == client) {
                    let _ = self.drop_client(Some(Bye::Detached));
                }
            }
            (Purpose::Quit { .. }, 1) => self.quitting = true,
            // Compact, cancel (`chrome::compact_choice`): the member's mod takes the request; one gone back to work
            // by then refuses it.
            (Purpose::Compact { member }, 0) => {
                if let Err(error) = crate::bridge::request_compaction(&self.state, &member) {
                    log(&format!("compaction of {member}: {error:#}"));
                }
            }
            _ => {}
        }
    }

    /// Where pane `id`'s cells are in its `area`: inside its frame, but for the menu's list, which draws its own.
    fn cells(&self, id: &str, area: Rect) -> Rect {
        if self.list.is_some() && self.menu.as_ref().is_some_and(|(menu, _)| menu == id) {
            area
        } else {
            screen::content(area)
        }
    }

    /// The list under a header's text closed, the focus back where it was.
    fn close_list(&mut self) {
        if self.list.is_some()
            && let Some(menu) = self.menu.as_ref().map(|(menu, _)| menu.clone())
        {
            self.close_pane(&menu);
        }
        self.list = None;
        self.dirty = true;
    }

    /// The list asks for `rows` × `cols`, its own frame included (`CSI 8 ; rows ; cols t`): its place changes with it,
    /// within the screen.
    fn list_resize(&mut self, rows: u16, cols: u16) {
        let Some(list) = self.list.as_mut() else { return };
        let size = (usize::from(cols), usize::from(rows));
        if list.size != size {
            list.size = size;
            self.layout();
        }
    }

    /// Gives the focus to pane `id`, showing its tab; the panes that asked are told (1004).
    fn focus_on(&mut self, id: &str) {
        if let Some(tab) = self.tabs.iter().position(|tab| tab.has(id)) {
            self.active = tab;
            self.tabs[tab].last = Some(id.to_string());
            // Another pane of a zoomed tab: the grid back, as in tmux.
            if self.tabs[tab].zoomed.as_deref().is_some_and(|zoomed| zoomed != id) {
                self.tabs[tab].zoomed = None;
            }
        }
        if self.focus.as_deref() == Some(id) {
            return;
        }
        if let Some(old) = self.focused() {
            old.send(input::focus(false, &old.modes()).unwrap_or_default());
        }
        self.focus = Some(id.to_string());
        if let Some(new) = self.focused() {
            new.send(input::focus(true, &new.modes()).unwrap_or_default());
        }
        self.layout();
    }

    fn show_tab(&mut self, tab: usize) {
        let Some(first) = self.tabs.get(tab).and_then(|tab| tab.ids().first().cloned()) else { return };
        // The pane that had the focus in it, else the first: one gone since is forgotten.
        let focus = self.tabs[tab].last.clone().filter(|id| self.tabs[tab].has(id));
        self.focus_on(&focus.unwrap_or(first));
        self.active = tab;
        self.layout();
    }

    /// A mouse event: the bar's parts, else the pane under the pointer, or the one a button went down in until it
    /// goes up; a press in a pane gives it the focus. To its program if it takes the mouse, else the wheel scrolls
    /// its history.
    fn mouse(&mut self, mouse: &Mouse) {
        if choice::swallowed(&mut self.swallowing, mouse.kind) {
            return;
        }
        let (col, row) = (mouse.col as usize, mouse.row as usize);
        // Nothing moves in a tab under the pointer, nor right after a click (« focus »).
        self.pointer = Some((col, row));
        self.pointer_at = Some(Instant::now());
        if let MouseKind::Down(_) = mouse.kind {
            self.last_click = Some(Instant::now());
        }
        if mouse.kind == MouseKind::Moved {
            self.hover_at(col, row);
        }
        // A list under a header's text: a press outside it closes it (on the same text too: a second click), and is
        // all it does; inside, it is the list's, whatever header part lies under it.
        let in_list = self.list.is_some() && self.menu_area().contains(col, row);
        if let MouseKind::Down(_) = mouse.kind
            && self.list.is_some()
            && !in_list
        {
            self.close_list();
            self.swallowing = true;
            return;
        }
        // The notice over everything: a click on it goes to its member.
        if let MouseKind::Down(_) = mouse.kind
            && self.notice_zone.is_some_and(|zone| zone.contains(col, row))
        {
            self.unselect();
            if let Some((notice, _)) = self.notice.take() {
                let target = self.panes.iter().find(|pane| pane.role.is_empty() && pane.member == notice.member);
                if let Some(target) = target.map(|pane| pane.id.clone()) {
                    self.focus_on(&target);
                }
            }
            // Its release is over the pane under it: not for its program.
            self.swallowing = true;
            self.dirty = true;
            return;
        }
        if let MouseKind::Down(Button::Left) = mouse.kind
            && !in_list
            && let Some((id, part)) =
                self.parts.iter().find(|(zone, _, _)| zone.contains(col, row)).map(|(_, id, part)| (id.clone(), *part))
        {
            self.unselect();
            self.part_click(&id, part);
            self.swallowing = true;
            self.dirty = true;
            return;
        }
        if let MouseKind::Down(_) = mouse.kind
            && let Some(target) = self.zones.iter().find(|(zone, _)| zone.contains(col, row)).map(|(_, t)| *t)
        {
            self.unselect();
            match target {
                Target::Tab(tab) => self.show_tab(tab),
                Target::Layout if self.menu.is_some() => {}
                Target::Layout => self.toggle_layout(),
                Target::Menu => self.menu_here(),
                Target::Quit if self.menu.is_some() => {}
                Target::Quit => self.ask_quit(),
            }
            self.dirty = true;
            return;
        }
        // The cards of the members left out of a tab in « focus »: a card brings its member to the left.
        if let MouseKind::Down(Button::Left) = mouse.kind
            && !in_list
            && self.card_click(col, row)
        {
            self.unselect();
            self.swallowing = true;
            self.dirty = true;
            return;
        }
        // The header of a pane on the right of a tab in « focus »: a press gives it the focus, a second one brings it to
        // the left.
        if let MouseKind::Down(Button::Left) = mouse.kind
            && !in_list
            && let Some(id) = self.right_header_at(col, row)
        {
            self.unselect();
            let now = Instant::now();
            let again = self.header_click.take().is_some_and(|(at, prev)| prev == id && at.elapsed() < DOUBLE_CLICK);
            if again {
                if let Some(focus) = self.tabs.get_mut(self.active).and_then(|tab| tab.focus.as_mut()) {
                    focus.bring(&id, now);
                }
            } else {
                self.header_click = Some((now, id.clone()));
            }
            self.focus_on(&id);
            self.layout();
            self.swallowing = true;
            self.dirty = true;
            return;
        }
        let mut shown = self.shown_ids();
        // The menu over the others: first under the pointer, and the only one while it is open.
        if let Some((menu, _)) = &self.menu {
            shown = vec![menu.clone()];
        }
        let under = self
            .panes
            .iter()
            .filter(|pane| shown.contains(&pane.id))
            .find(|pane| self.cells(&pane.id, pane.area).contains(col, row))
            .map(|pane| pane.id.clone());
        let held = matches!(mouse.kind, MouseKind::Drag(_) | MouseKind::Up(_));
        let Some(id) = (if held { self.grab.clone().or(under) } else { under }) else { return };
        if matches!(mouse.kind, MouseKind::Up(_)) {
            self.grab = None;
        }
        if let MouseKind::Down(_) = mouse.kind {
            self.grab = Some(id.clone());
            if let MouseKind::Down(button) = mouse.kind {
                self.grab_button = button;
            }
            if self.focus.as_deref() != Some(id.as_str()) {
                self.focus_on(&id);
                self.dirty = true;
            }
        }
        let Some(area) = self.pane(&id).map(|pane| pane.area) else { return };
        let content = self.cells(&id, area);
        let x = col.clamp(content.x, content.x + content.width.saturating_sub(1)) - content.x;
        let y = row.clamp(content.y, content.y + content.height.saturating_sub(1)) - content.y;
        if matches!(mouse.kind, MouseKind::Down(_) | MouseKind::Drag(_)) && self.grab.as_deref() == Some(id.as_str()) {
            self.grab_at = (x as u16, y as u16);
        }
        let Some(pane) = self.pane(&id) else { return };
        // The dashboard and the journal ask for no mouse (`recruit _panel`): a press goes to the member it lands on.
        if let MouseKind::Down(Button::Left) = mouse.kind
            && (pane.role == DASHBOARD || pane.role == JOURNAL)
        {
            self.unselect();
            self.panel_click(&id, x, y, content.width);
            return;
        }
        let left = matches!(
            mouse.kind,
            MouseKind::Down(Button::Left) | MouseKind::Drag(Button::Left) | MouseKind::Up(Button::Left)
        );
        // With Shift, the left button selects even in a program that takes the mouse, as in terminals.
        let program = if left && (self.selecting || mouse.mods.contains(Mods::SHIFT)) {
            None
        } else {
            input::mouse(mouse, x as u16, y as u16, &pane.modes())
        };
        match program {
            Some(bytes) => {
                pane.send(bytes);
                if let MouseKind::Down(_) = mouse.kind {
                    self.unselect();
                }
            }
            None if left => self.select(&id, mouse, content),
            None => {
                let lines = match mouse.kind {
                    MouseKind::ScrollUp => WHEEL,
                    MouseKind::ScrollDown => -WHEEL,
                    _ => return,
                };
                self.dirty |= pane
                    .with(|engine| {
                        let before = engine.scrolled();
                        engine.scroll(lines);
                        engine.scrolled() != before
                    })
                    .unwrap_or(false);
            }
        }
    }

    /// The left button selecting in pane `id`, whose cells are `cells`: a press starts a selection (a word on a
    /// double click, a line on a triple), a drag extends it (the history scrolling past the pane's top or bottom), a
    /// release copies it, to the client's terminal (OSC 52).
    fn select(&mut self, id: &str, mouse: &Mouse, cells: Rect) {
        let (col, row) = (mouse.col as usize, mouse.row as usize);
        let previous = self.selection.take();
        let Some(pane) = self.pane(id) else { return };
        let alt = pane.modes().alt_screen;
        let now = Instant::now();
        let done = pane.with(|engine| match mouse.kind {
            MouseKind::Down(_) => {
                let at = select::point(engine, cells, col, row);
                let previous = previous.as_ref().map(|(selection, _)| selection);
                (Some((Selection::press(previous, id, at, now, engine), alt)), None)
            }
            MouseKind::Drag(_) => match previous.filter(|(selection, _)| selection.pane == id) {
                Some((mut selection, alt)) => {
                    let lines = select::edge(cells, row);
                    if lines != 0 {
                        engine.scroll(lines);
                    }
                    selection.drag(select::point(engine, cells, col, row), engine);
                    (Some((selection, alt)), None)
                }
                None => (None, None),
            },
            MouseKind::Up(_) => {
                let copied = previous.as_ref().and_then(|(selection, _)| selection.copy(engine));
                (previous, copied)
            }
            _ => (previous, None),
        });
        if let Some((selection, copied)) = done {
            self.selection = selection;
            if let (Some(relay), Some(client)) = (copied, self.client.as_mut()) {
                client.relays.add(relay);
            }
        }
        self.selecting = matches!(mouse.kind, MouseKind::Down(_) | MouseKind::Drag(_));
        self.dirty = true;
    }

    fn unselect(&mut self) {
        if self.selection.take().is_some() {
            self.dirty = true;
        }
        self.selecting = false;
    }

    /// The selection gone if it is in pane `id`: what it showed is about to change.
    fn unselect_in(&mut self, id: Option<String>) {
        if self.selection.as_ref().is_some_and(|(selection, _)| Some(&selection.pane) == id.as_ref()) {
            self.unselect();
        }
    }

    /// The selection gone if its lines left the history, or its pane changed screens.
    fn check_selection(&mut self) {
        let Some((selection, alt)) = &self.selection else { return };
        let gone = match self.pane(&selection.pane) {
            Some(pane) => {
                pane.with(|engine| engine.modes().alt_screen != *alt || !selection.kept(engine)).unwrap_or(true)
            }
            None => true,
        };
        if gone {
            self.unselect();
        }
    }

    fn resize(&mut self, size: (u16, u16)) {
        self.unselect();
        // A list under a header's text would stay where the text was: closed, as by a click outside.
        self.close_list();
        self.size = size;
        if let Some(client) = self.client.as_mut() {
            client.size = size;
        }
        self.layout();
    }

    /// The room of a tab: the screen less the bar.
    fn tab_room(&self) -> (usize, usize) {
        (self.size.0 as usize, (self.size.1 as usize).saturating_sub(1))
    }

    /// Whether tab `t` shows its members in « focus »: a tab of agents (no panels beside it) of two members or more,
    /// the team's choice or ⌥l's, and a screen with room for it (`layout::focus_room`).
    fn in_focus(&self, t: usize) -> bool {
        let Some(tab) = self.tabs.get(t) else { return false };
        let (width, height) = self.tab_room();
        tab.side.is_none()
            && tab.panes.len() >= 2
            && tab.mode.unwrap_or(self.focus_layout)
            && layout::focus_room(width, height).is_some()
    }

    /// The panes of tab `t` as the places of « focus » see them: how well placed each one is, and since when.
    fn focus_seen(&self, t: usize, now: Instant) -> Vec<Seen> {
        let epoch = self::now();
        let Some(tab) = self.tabs.get(t) else { return Vec::new() };
        tab.panes
            .iter()
            .filter_map(|id| self.pane(id))
            .map(|pane| {
                let standing = match pane_state(pane, &self.looks) {
                    look::State::Waiting => Standing::Waiting,
                    look::State::Error => Standing::Failed,
                    look::State::Working => Standing::Working,
                    look::State::Idle | look::State::Other => Standing::Resting,
                };
                let since = pane_since(pane, &self.looks)
                    .and_then(|since| now.checked_sub(Duration::from_secs(epoch.saturating_sub(since))))
                    .unwrap_or(self.born);
                Seen { id: pane.id.clone(), standing, since }
            })
            .collect()
    }

    /// The places of every tab in « focus » brought up to date with its panes, and dropped for a tab in its grid.
    /// Only what the panes' coming and going and the room force: nothing is reordered here ([`Server::update_focus`]).
    /// The pane with the focus is always one of those shown: a card that gets it comes to the left.
    fn ensure_focus(&mut self) {
        let (width, height) = self.tab_room();
        let room = layout::focus_room(width, height).unwrap_or(0);
        let now = Instant::now();
        for t in 0..self.tabs.len() {
            if !self.in_focus(t) {
                self.tabs[t].focus = None;
                continue;
            }
            let seen = self.focus_seen(t, now);
            let focused = self.focus.clone();
            let Tab { last, panes, focus, .. } = &mut self.tabs[t];
            // A tab that opens in « focus » fills its right column at once; after, only what is forced.
            let opening = focus.is_none();
            let focus = focus.get_or_insert_with(|| {
                Focus::new(last.clone().filter(|id| panes.contains(id)).or_else(|| panes.first().cloned()))
            });
            focus.update(&seen, room, now, !opening);
            if let Some(id) = focused.filter(|id| focus.places().others.contains(id)) {
                focus.bring(&id, now);
            }
        }
    }

    /// The tabs in « focus » look at their places again: a member that began to wait, to work, a place that came due
    /// (`Focus::deadline`). Nothing moves in the tab shown while the pointer is on it, a key was just typed, or a click
    /// was just made; the places catch up when that ends. Moved places lay the tab out again.
    fn update_focus(&mut self) {
        self.update_focus_at(Instant::now());
    }

    /// [`Server::update_focus`] as of `now`.
    fn update_focus_at(&mut self, now: Instant) {
        if self.tabs.iter().all(|tab| tab.focus.is_none()) {
            self.focus_wake = None;
            return;
        }
        let (width, height) = self.tab_room();
        let room = layout::focus_room(width, height).unwrap_or(0);
        let (held, ends) = self.holding(now);
        let mut changed = false;
        let mut wake: Option<Instant> = ends;
        for t in 0..self.tabs.len() {
            if self.tabs[t].focus.is_none() {
                continue;
            }
            let seen = self.focus_seen(t, now);
            let hold = held && t == self.active;
            if let Some(focus) = self.tabs[t].focus.as_mut() {
                changed |= focus.update(&seen, room, now, hold);
                wake = [wake, focus.deadline()].into_iter().flatten().min();
            }
        }
        self.focus_wake = wake;
        if changed {
            self.layout();
        }
    }

    /// Whether the tab shown is held in place, and when the hold that has an end ends: the pointer is on a pane on the
    /// right or on the cards and moved less than [`POINTER_HOLD`] ago, a key was typed or a click made less than a
    /// second ago (nothing tells that either is over: the wake-up is for the end of the hold).
    fn holding(&self, now: Instant) -> (bool, Option<Instant>) {
        let mut ends: Vec<Instant> =
            [self.last_key, self.last_click].into_iter().flatten().map(|at| at + focus::HOLD_AFTER_CLICK).collect();
        if let (Some((col, row)), Some(at)) = (self.pointer, self.pointer_at)
            && self.client.is_some()
            && self.under_pointer(col, row)
        {
            ends.push(at + POINTER_HOLD);
        }
        let ends: Vec<Instant> = ends.into_iter().filter(|end| *end > now).collect();
        (!ends.is_empty(), ends.into_iter().max())
    }

    /// Whether `col`, `row` is on a pane on the right of the tab shown in « focus », or on its cards.
    fn under_pointer(&self, col: usize, row: usize) -> bool {
        let Some(focus) = self.tabs.get(self.active).and_then(|tab| tab.focus.as_ref()) else { return false };
        let right = focus.places().right.iter().filter_map(|id| self.pane(id));
        right.into_iter().any(|pane| pane.area.contains(col, row))
            || self.cards_rect().is_some_and(|rect| rect.contains(col, row))
    }

    /// Where the cards of the members left out go in the tab shown in « focus »: their frame, in the room that
    /// `layout::focus_rects` leaves under the panes on the right. The same room, the same call, as `layout`.
    fn cards_rect(&self) -> Option<Rect> {
        let tab = self.tabs.get(self.active).filter(|tab| tab.zoomed.is_none())?;
        let places = tab.focus.as_ref()?.places();
        let (width, height) = self.tab_room();
        layout::focus_rects(width, height, places.left.as_deref()?, &places.right, places.others.len()).1
    }

    /// The cards of the tab shown, made again from the looks, the panes' states and the places: a card has what the
    /// dashboard last wrote of its member (what it does, its curve), and none of it with no dashboard.
    fn refresh_cards(&mut self) {
        let (mut cards, mut ids, mut epochs) = (Vec::new(), Vec::new(), Vec::new());
        if self.cards_rect().is_some() {
            let places = self.tabs[self.active].focus.as_ref().map(|focus| focus.places().clone()).unwrap_or_default();
            let epoch = now();
            for pane in places.others.iter().filter_map(|id| self.pane(id)) {
                let state = pane_state(pane, &self.looks);
                let look = self.looks.get(&pane.member);
                let began = pane_since(pane, &self.looks);
                cards.push(board::CardData {
                    name: pane.member.clone(),
                    color: member_color(&self.order, &pane.member),
                    state,
                    since: began.map_or(0, |began| epoch.saturating_sub(began)),
                    shell: look.is_some_and(|look| look.shell)
                        && !matches!(state, look::State::Working | look::State::Waiting | look::State::Error),
                    context: look.and_then(|look| look.context).map(f64::from),
                    pressure: look.map(|look| look.pressure).unwrap_or_default(),
                    model: look.and_then(|look| look.model.clone()),
                    effort: look.and_then(|look| look.effort.clone()),
                    doing: look.and_then(|look| look.doing.clone()),
                    activity: look.map(|look| look.activity.clone()).unwrap_or_default(),
                    failed: (state == look::State::Error)
                        .then(|| pane.error.as_ref().map_or(self.unknown_error.clone(), |error| error.label.clone())),
                    compacted: look.and_then(|look| look.compacted),
                });
                ids.push(pane.id.clone());
                epochs.push(began);
            }
        }
        if !cards.is_empty() || !self.cards.is_empty() {
            self.dirty = true;
        }
        (self.cards, self.card_ids, self.card_epoch) = (cards, ids, epochs);
    }

    /// A press on the cards' frame: on a card, that member comes to the left. True when the press was in the frame.
    fn card_click(&mut self, col: usize, row: usize) -> bool {
        let Some(rect) = self.cards_rect().filter(|rect| rect.contains(col, row)) else { return false };
        let wide = self.tab_room().0 >= layout::FOCUS_WIDE;
        let placed = cards::place(chrome::inside(rect), &self.cards, wide);
        let hit = placed.shown.iter().find(|(_, at)| at.contains(col, row)).map(|(i, _)| *i);
        if let Some(id) = hit.and_then(|i| self.card_ids.get(i).cloned()) {
            if let Some(focus) = self.tabs.get_mut(self.active).and_then(|tab| tab.focus.as_mut()) {
                focus.bring(&id, Instant::now());
            }
            self.focus_on(&id);
            self.layout();
        }
        true
    }

    /// The bar's layout button for tab `t`: where ⌥l would take it. None for a tab it cannot go to « focus » in (panels
    /// beside it, a single member, a screen too small).
    fn switch(&self, t: usize) -> Option<chrome::Switch> {
        let tab = self.tabs.get(t)?;
        let (width, height) = self.tab_room();
        if tab.side.is_some() || tab.panes.len() < 2 || layout::focus_room(width, height).is_none() {
            return None;
        }
        Some(if self.in_focus(t) { chrome::Switch::ToGrid } else { chrome::Switch::ToFocus })
    }

    /// ⌥l and the bar's button: the tab shown goes from its grid to « focus », or back, until the client leaves.
    fn toggle_layout(&mut self) {
        let t = self.active;
        let Some(tab) = self.tabs.get(t) else { return };
        if tab.side.is_some() || tab.panes.len() < 2 {
            return;
        }
        let focus = self.in_focus(t);
        self.tabs[t].mode = Some(!focus);
        self.unselect();
        self.layout();
    }

    /// The pane on the right of the tab shown whose header row is at `col`, `row`.
    fn right_header_at(&self, col: usize, row: usize) -> Option<String> {
        let places = self.tabs.get(self.active)?.focus.as_ref()?.places();
        places
            .right
            .iter()
            .find(|id| self.pane(id).is_some_and(|pane| row == pane.area.y && pane.area.contains(col, row)))
            .cloned()
    }

    /// Every tab laid out on the screen, less the bar: a shown tab's panes get their place, the others their size.
    fn layout(&mut self) {
        self.ensure_focus();
        if let Some((menu, _)) = self.menu.clone() {
            let area = self.menu_area();
            // A list draws its own frame: its cells are all its place.
            let inside = if self.list.is_some() { area } else { screen::content(area) };
            if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == menu) {
                pane.place(area, inside);
            }
        }
        let (width, height) = (self.size.0 as usize, (self.size.1 as usize).saturating_sub(1));
        for (t, tab) in self.tabs.iter().enumerate() {
            let side = tab.side.as_ref().map(|side| SidePanes {
                dashboard: side.dashboard.clone(),
                journal: side.journal.clone().map(|id| (id, self.reduced.then_some(JOURNAL_ROWS))),
            });
            // Zoomed, one pane takes the tab's room, the others keep their size, hidden (their programs are not
            // resized for nothing); in a tab not shown too, so that going back to it resizes nothing.
            let zoomed = tab.zoomed.as_deref();
            // In « focus »: the left pane, those on the right, and the room of the cards (left empty for now).
            let places = tab.focus.as_ref().map(Focus::places).filter(|places| places.left.is_some());
            let rects = match places {
                Some(Places { left: Some(left), right, others }) => {
                    layout::focus_rects(width, height, left, right, others.len()).0
                }
                _ => layout::rects(width, height, &tab.panes, self.columns, side.as_ref()),
            };
            // A card is no pane on the screen: hidden, its size kept.
            for id in tab.panes.iter().filter(|id| !rects.iter().any(|(placed, _)| placed == *id)) {
                if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == *id) {
                    pane.area = Rect::default();
                }
            }
            for (id, area) in rects {
                let area = if zoomed == Some(id.as_str()) { Rect { x: 0, y: 0, width, height } } else { area };
                if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) {
                    let shown = t == self.active && zoomed.is_none_or(|zoomed| zoomed == id);
                    pane.place(area, screen::content(area));
                    pane.area = if shown { area } else { Rect::default() };
                }
            }
        }
        self.refresh_cards();
        self.dirty = true;
    }

    /// Engines' deadlines due: a synchronized update that never ended, spare work.
    fn expire(&mut self) {
        let now = Instant::now();
        self.deadline = None;
        let held = self.gate.hold();
        for pane in &self.panes {
            let done = pane.with(|engine| {
                let mut replies = Vec::new();
                let changed = engine.deadline().is_some_and(|at| at <= now) && engine.expire(now, &mut replies);
                (replies, changed, engine.deadline())
            });
            if let Some((replies, changed, deadline)) = done {
                self.dirty |= changed || !replies.is_empty();
                pane.send(replies);
                self.deadline = [self.deadline, deadline].into_iter().flatten().min();
            }
        }
        drop(held);
    }

    /// Broken engines replaced.
    fn repair(&mut self) {
        let caps = self.client.as_ref().map(|client| client.caps.clone()).unwrap_or_default();
        for pane in self.panes.iter_mut().filter(|pane| pane.broken.is()) {
            if let Err(error) = pane.repair(&caps) {
                log(&format!("pane {}: {error:#}", pane.id));
            }
            self.dirty = true;
        }
    }

    /// The relays of every engine taken: kept for the client, or dropped without one (a clipboard filled hours
    /// later would surprise); the states noted, and since when; a member gone waiting out of sight noticed (F5).
    fn take_relays(&mut self) {
        let focus = self.focus.clone();
        let shown = self.in_view_ids();
        let mut relays = Vec::new();
        let mut called = Vec::new();
        let mut resized = None;
        // The members compacting on their own, by pane: their `working` is held back.
        let compacting: Vec<String> = self
            .panes
            .iter()
            .filter(|pane| pane.role.is_empty() && self.compacting(&pane.member, pane.since))
            .map(|pane| pane.id.clone())
            .collect();
        let mut ended = Vec::new();
        let list_pane = self.list.as_ref().and(self.menu.as_ref()).map(|(menu, _)| menu.as_str());
        let held = self.gate.hold();
        let looks = &self.looks;
        for pane in &mut self.panes {
            relays.clear();
            let gate_free = pane.with(|engine| engine.relays(&mut relays));
            if gate_free.is_none() {
                continue;
            }
            for relay in relays.drain(..) {
                // The size a list asks for: honoured for the menu's list alone, relayed to no one.
                if let Relay::Resize { rows, cols } = relay {
                    if list_pane == Some(pane.id.as_str()) {
                        resized = Some((rows, cols));
                    }
                    continue;
                }
                if let Relay::Status(status) = &relay {
                    // Compacting on its own: a `working` is not a turn, the member stays at rest; another state is
                    // the end of the compaction (or of the turn it was in).
                    if status.id.is_none() && compacting.contains(&pane.id) {
                        if status.state == State::Working {
                            pane.held = Some((status.clone(), now()));
                            continue;
                        }
                        ended.push(pane.member.clone());
                    }
                    if status.id.is_none() {
                        pane.held = None;
                    }
                    let state = look_state(Some(status.state));
                    // So already, by its program or by the dashboard: no second notice.
                    if calls(state)
                        && pane_state(pane, looks) != state
                        && pane.role.is_empty()
                        && !shown.contains(&pane.id)
                    {
                        called.push((pane.id.clone(), state));
                    }
                    note_status(pane, status);
                    self.look_again = true;
                    self.cards_stale = true;
                }
                if matches!(relay, Relay::Title(_)) && focus.as_deref() != Some(pane.id.as_str()) {
                    continue;
                }
                if let Some(client) = self.client.as_mut() {
                    client.relays.add(relay);
                }
            }
        }
        drop(held);
        for member in ended {
            if let Some(asked) = self.asked.get_mut(&member) {
                asked.ended = true;
            }
        }
        if let Some((rows, cols)) = resized {
            self.list_resize(rows, cols);
        }
        self.call(called);
    }

    /// The notices of members gone waiting or failed out of sight, in turn: a wait last, which wins.
    fn call(&mut self, mut called: Vec<(String, look::State)>) {
        called.sort_by_key(|(_, state)| *state == look::State::Waiting);
        for (id, state) in called {
            match state {
                look::State::Waiting => self.notify(&id),
                _ => self.notify_error(&id),
            }
        }
    }

    /// The notice of a member gone waiting out of sight, for [`NOTICE`]: the latest only; the others keep their tab's
    /// badge (⚑). None under a layer (the menu, a choice): nothing covers it, the menu shows each member's state, and
    /// the notice is forgotten, not shown once the layer goes (architect's decision).
    fn notify(&mut self, id: &str) {
        // Without a client, no one to tell: no notice, nor its wake-up.
        if self.client.is_none() || self.menu.is_some() || self.choice.is_some() {
            return;
        }
        let Some(pane) = self.pane(id) else { return };
        let Some(tab) = self.tabs.iter().find(|tab| tab.has(id)) else { return };
        let notice = chrome::Notice {
            member: pane.member.clone(),
            state: look::State::Waiting,
            color: Some(member_color(&self.order, &pane.member)),
            text: chrome::notice_text(&pane.member),
            tab: tab.title.clone(),
        };
        self.notice = Some((notice, Instant::now() + NOTICE));
        self.dirty = true;
    }

    /// The notice of a member whose turn failed out of sight (mock-up C), orange: one for the failures within
    /// [`BURST`] of the first, which the later ones join while it shows (« 3 membres ont raté leur tour »), and none
    /// else. Never over a wait's notice, nor under a layer, as a wait's (`notify`).
    fn notify_error(&mut self, id: &str) {
        if self.client.is_none() || self.menu.is_some() || self.choice.is_some() {
            return;
        }
        if self.notice.as_ref().is_some_and(|(notice, _)| notice.state == look::State::Waiting) {
            return;
        }
        let Some(pane) = self.pane(id) else { return };
        let Some(tab) = self.tabs.iter().find(|tab| tab.has(id)) else { return };
        let label = pane.error.as_ref().map_or(self.unknown_error.clone(), |error| error.label.clone());
        let (member, tab) = (pane.member.clone(), tab.title.clone());
        let now = Instant::now();
        let members = match &mut self.burst {
            Some((since, members)) if now.duration_since(*since) < BURST => {
                if !members.contains(&member) {
                    members.push(member.clone());
                }
                // Its notice gone already (its time, a layer): no other.
                if !self.notice.as_ref().is_some_and(|(notice, _)| notice.state == look::State::Error) {
                    return;
                }
                members.clone()
            }
            _ => {
                self.burst = Some((now, vec![member.clone()]));
                vec![member.clone()]
            }
        };
        let names: Vec<&str> = members.iter().map(String::as_str).collect();
        let notice = chrome::Notice {
            state: look::State::Error,
            color: Some(member_color(&self.order, &member)),
            text: chrome::error_notice_text(&names, &label),
            member,
            tab,
        };
        // Joined, it keeps its time.
        let until = match &self.notice {
            Some((shown, until)) if shown.state == look::State::Error => *until,
            _ => now + NOTICE,
        };
        self.notice = Some((notice, until));
        self.dirty = true;
    }

    /// The notice gone once its time is up, once a layer opens, or once its members are all in sight or no longer
    /// in the state it tells.
    fn check_notice(&mut self) {
        let Some((notice, until)) = &self.notice else { return };
        let members = match &self.burst {
            Some((_, members)) if notice.state == look::State::Error => members.clone(),
            _ => vec![notice.member.clone()],
        };
        let shown = self.in_view_ids();
        let calling = self
            .panes
            .iter()
            .filter(|pane| pane.role.is_empty() && members.contains(&pane.member))
            .any(|pane| pane_state(pane, &self.looks) == notice.state && !shown.contains(&pane.id));
        let over = *until <= Instant::now() || self.menu.is_some() || self.choice.is_some() || !calling;
        if over {
            self.notice = None;
            self.dirty = true;
        }
    }

    /// The panes in a client's view, for the notices: those the tab shown shows, and its cards (a card turns red, then
    /// its member comes in: no notice). Not for the compaction, to which a card is no pane on the screen.
    fn in_view_ids(&self) -> Vec<String> {
        let mut ids = self.shown_ids();
        ids.extend(self.card_ids.iter().cloned());
        ids
    }

    /// The panes the active tab shows: the zoomed one alone, else all of them.
    fn shown_ids(&self) -> Vec<String> {
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        match (&tab.zoomed, tab.focus.as_ref()) {
            (Some(id), _) => vec![id.clone()],
            // In « focus »: the left pane and those on the right; the others are cards.
            (None, Some(focus)) => {
                let places = focus.places();
                places.left.iter().chain(&places.right).cloned().collect()
            }
            (None, None) => tab.ids(),
        }
    }

    /// The members to compact on their own, as of `now` (seconds since the epoch), and how long before looking again
    /// for the rest not long enough yet, or the request to renew (decision of 2026-10-11). A member qualifies when the
    /// team has the compaction on, it is at rest (by its program, else the dashboard) since `auto_idle` seconds with no
    /// command running, its context is at the team's share, it is on no client's screen (the panes of the tab shown, or
    /// the zoomed one alone), and [`Server::renewal`] allows a request. Whether a subagent or a teammate works, the
    /// bridge says. What else changes it (a state, the looks, a tab shown) comes through the loop, which looks again.
    fn compaction_due(&self, now: u64) -> (Vec<(String, u8, u64)>, Option<Duration>) {
        let Some(share) = self.auto_compact else { return (Vec::new(), None) };
        let shown = if self.client.is_some() { self.shown_ids() } else { Vec::new() };
        let mut due = Vec::new();
        let mut wait: Option<Duration> = None;
        let mut later = |left: Duration| wait = Some(wait.map_or(left, |wait| wait.min(left)));
        for pane in self.panes.iter().filter(|pane| pane.role.is_empty() && !pane.exited) {
            if pane_state(pane, &self.looks) != look::State::Idle {
                continue;
            }
            let Some(look) = self.looks.get(&pane.member) else { continue };
            let Some(context) = look.context.filter(|context| *context >= share) else { continue };
            let Some(since) = pane_since(pane, &self.looks) else { continue };
            if look.shell {
                continue;
            }
            let Some(left) = self.renewal(&pane.member, since) else { continue };
            let rested = now.saturating_sub(since);
            if rested < self.auto_idle {
                later(Duration::from_secs(self.auto_idle - rested).max(left));
            } else if !left.is_zero() {
                later(left);
            } else if !shown.contains(&pane.id) {
                due.push((pane.member.clone(), context, since));
            }
        }
        (due, wait)
    }

    /// Whether `member`'s compaction, asked on its own, is under way: asked in the rest it is in, not answered (`done`,
    /// `skip`, `error` for its request's id), its program not back to another state than `working`, and not for more
    /// than [`COMPACTING`]. The `working` that Claude Code says for it is no turn: the member stays at rest.
    fn compacting(&self, member: &str, since: Option<u64>) -> bool {
        let Some(asked) = self.asked.get(member).filter(|asked| Some(asked.since) == since) else { return false };
        let answered = asked.id.as_ref().and_then(|id| self.outcomes.get(member).filter(|outcome| outcome.id == *id));
        let answered =
            answered.is_some_and(|outcome| outcome.done || outcome.skip.is_some() || outcome.error.is_some());
        !asked.ended && !answered && asked.at.elapsed() < COMPACTING
    }

    /// The `working` still held back for a member whose compaction is over (no other state said since): told as a
    /// turn's. When the compaction was not made (refused, failed, no answer in time), since the `working` began. When
    /// it was, the program said no end of it (its `done` comes after the turn queued behind it, which begins right
    /// after the compaction: Claude Code 2.1.296, a message received meanwhile): the turn began when the mod answered,
    /// and the member stays at work until its next state. A compaction on its own says `done` at its end, before the
    /// mod answers: nothing is held then, and nothing changes here.
    fn settle_compactions(&mut self) {
        for i in 0..self.panes.len() {
            let pane = &self.panes[i];
            if pane.held.is_none() || self.compacting(&pane.member, pane.since) {
                continue;
            }
            let answered =
                self.outcomes.get(&pane.member).zip(self.asked.get(&pane.member)).and_then(|(outcome, asked)| {
                    (asked.id.as_deref() == Some(outcome.id.as_str()) && outcome.done).then_some(outcome.at)
                });
            if let Some((status, when)) = self.panes[i].held.take() {
                note_status(&mut self.panes[i], &status);
                let began = answered.and_then(|at| u64::try_from(at).ok()).filter(|at| *at > 0);
                self.panes[i].since = Some(began.unwrap_or(when));
            }
            self.dirty = true;
        }
    }

    /// Whether a member at rest since `since` may be asked to compact, and when: `None` when the rest is closed,
    /// else the wait before it may be asked (zero: now). Not asked yet in this rest: now. Asked: closed once the
    /// compaction is done; asked again, 5 minutes after, while a subagent or a teammate worked (as often as that
    /// lasts, those refusals count for nothing); a request that got no answer (lapsed, its mod not running), failed or
    /// was refused for another reason is asked once more, 5 minutes after, then the rest is closed. The answer to the
    /// last request is waited for [`AUTO_PENDING`]: the rest stays open, and watched, until it is read.
    fn renewal(&self, member: &str, since: u64) -> Option<Duration> {
        let Some(asked) = self.asked.get(member).filter(|asked| asked.since == since) else {
            return Some(Duration::ZERO);
        };
        let elapsed = asked.at.elapsed();
        let again = AUTO_AGAIN.saturating_sub(elapsed);
        let answer = asked.id.as_ref().and_then(|id| self.outcomes.get(member).filter(|outcome| outcome.id == *id));
        let waiting = answer.is_none_or(|a| !a.done && a.skip.is_none() && a.error.is_none());
        match answer {
            Some(outcome) if outcome.done => None,
            Some(outcome) if not_a_failure(outcome) => Some(again),
            _ if waiting && elapsed < AUTO_PENDING => {
                Some(if asked.failures == 0 { again } else { AUTO_PENDING - elapsed })
            }
            // Failed, refused, or unanswered: the one more try is left to the first of them.
            _ => (asked.failures == 0).then_some(again),
        }
    }

    /// Whether the request of `asked` was refused for a subagent or a teammate at work.
    fn refused_for_helpers(&self, member: &str, asked: &Asked) -> bool {
        let answer = asked.id.as_ref().and_then(|id| self.outcomes.get(member).filter(|outcome| outcome.id == *id));
        answer.is_some_and(not_a_failure)
    }

    /// Asks the members that [`Server::compaction_due`] names: logged, the request written off the loop
    /// ([`ask_compaction`]), its id told back to the loop.
    fn auto_compaction(&mut self) {
        self.settle_compactions();
        for (member, percent, since) in self.plan_compaction(now()) {
            let (state, tx) = (self.state.clone(), self.tx.clone());
            let name = member.clone();
            let spawned = std::thread::Builder::new()
                .name("compaction".into())
                .spawn(move || ask_compaction(&state, &tx, &name, percent, since));
            if let Err(error) = spawned {
                log(&format!("auto-compaction: {member}: {error}"));
            }
        }
    }

    /// What [`Server::auto_compaction`] decides as of `now` (seconds since the epoch): the members to ask, with the
    /// context seen and the start of their rest, noted as asked (and logged); the next look due; the reports to read.
    fn plan_compaction(&mut self, now: u64) -> Vec<(String, u8, u64)> {
        if self.auto_compact.is_none() {
            self.compaction = None;
            self.asked.clear();
            self.watched.lock().unwrap_or_else(PoisonError::into_inner).clear();
            return Vec::new();
        }
        let (due, mut wait) = self.compaction_due(now);
        // A compaction under way ends within [`COMPACTING`] at the latest: the `working` held back is told then.
        for pane in self.panes.iter().filter(|pane| pane.held.is_some() && self.compacting(&pane.member, pane.since)) {
            let left = self
                .asked
                .get(&pane.member)
                .map_or(Duration::ZERO, |asked| COMPACTING.saturating_sub(asked.at.elapsed()));
            wait = Some(wait.map_or(left, |wait| wait.min(left)));
        }
        self.compaction = wait.map(|wait| Instant::now() + wait);
        for (member, percent, since) in &due {
            let before = self.asked.get(member).filter(|asked| asked.since == *since);
            let failures = before
                .map_or(0, |asked| asked.failures.saturating_add(u8::from(!self.refused_for_helpers(member, asked))));
            log(&format!(
                "auto-compaction: {member} asked (after {failures} failed), context at {percent} %, at rest since {since}"
            ));
            let asked = Asked { since: *since, id: None, at: Instant::now(), ended: false, failures };
            self.asked.insert(member.clone(), asked);
        }
        // The outcomes of the requests still open are the reports to read.
        let open: Vec<String> = self
            .asked
            .iter()
            .filter(|(member, asked)| {
                self.looks.contains_key(*member)
                    && (self.renewal(member, asked.since).is_some() || self.compacting(member, Some(asked.since)))
            })
            .map(|(member, _)| member.clone())
            .collect();
        let mut watched = self.watched.lock().unwrap_or_else(PoisonError::into_inner);
        if *watched != open {
            *watched = open;
        }
        due
    }

    /// The next frame, if one is due and the client can take it.
    fn frame(&mut self) {
        if !self.dirty || !(self.echo || self.last_frame.is_none_or(|at| at.elapsed() >= FRAME)) {
            return;
        }
        for pane in &self.panes {
            pane.told.store(false, Ordering::Release);
        }
        self.take_relays();
        let Some(client) = self.client.as_ref() else {
            self.dirty = false;
            self.echo = false;
            return;
        };
        if client.busy {
            if let Some(client) = self.client.as_mut() {
                client.late = true;
            }
            self.dirty = false;
            return;
        }
        let started = Instant::now();
        self.check_selection();
        if !self.compose() {
            return;
        }
        // Every move, always: the headers' parts, the tabs and the buttons light up under the pointer (F3), a choice's
        // selection follows it, the focused pane may ask. Only a change of what is lit draws again.
        let motion = true;
        let Some(client) = self.client.as_mut() else { return };
        let relays = client.relays.take();
        self.written += relays.len() as u64;
        if !relays.is_empty() {
            client.out.push(Out::Output(relays));
        }
        if client.motion != Some(motion) {
            client.motion = Some(motion);
            client.out.message(&ServerMsg::Motion(motion));
        }
        let bytes = client.painter.frame(self.screen.borrow().canvas());
        self.dirty = false;
        // Nothing changed on screen, the cursor included (the pointer lit what was lit already): no frame, no round
        // trip with the client, and the next one is not held back.
        if bytes.is_empty() {
            self.echo = false;
            return;
        }
        self.frames += 1;
        self.written += bytes.len() as u64;
        client.out.push(Out::Output(bytes));
        client.busy = true;
        self.last_frame = Some(started);
        self.echo = false;
    }

    /// A pane's header: a member's state, since when, its model, effort and context (`looks`), its zoom, and the
    /// part under the pointer; a panel's title (the dashboard, the journal, the menu). `epoch`: now, in seconds.
    fn header<'a>(
        &'a self,
        pane: &'a Pane,
        note: Option<&'a str>,
        zoomed: Option<&str>,
        epoch: u64,
    ) -> chrome::Header<'a> {
        let hint = match pane.role.as_str() {
            JOURNAL => Some(chrome::Hint::JournalSize),
            DASHBOARD => Some(self.counts()),
            _ => None,
        };
        let base = chrome::Header {
            // The menu names itself on its first line: its frame has no title.
            name: if pane.role == MENU { "" } else { &pane.member },
            state: pane_state(pane, &self.looks),
            note,
            hint,
            ..chrome::Header::default()
        };
        if !pane.role.is_empty() {
            return base;
        }
        let looks = self.looks.get(&pane.member);
        let context = looks.and_then(|looks| looks.context);
        // Its program says its state: at rest by it. Else (none said, or `Clear`) as the dashboard last saw it.
        let told = look_state(pane.state) != look::State::Other;
        // At rest, or its turn failed (architect's decision): a click on its context compacts it.
        let compactable = if told {
            matches!(base.state, look::State::Idle | look::State::Error) && context.is_some()
        } else {
            looks.is_some_and(|looks| looks.compactable)
        };
        // At rest by its program, a command still running by `claude agents` (`"status": "shell"`, which OSC 7501 does
        // not tell): the command's sign, and since when it runs, as its card (architect's decision). At work, or
        // waiting, its program wins.
        let shell = looks.is_some_and(|looks| looks.shell)
            && !matches!(base.state, look::State::Working | look::State::Waiting | look::State::Error);
        // Why its turn failed, short: as its program said it, else unknown (as the dashboard saw it).
        let error = (base.state == look::State::Error)
            .then(|| pane.error.as_ref().map_or(self.unknown_error.as_str(), |error| error.label.as_str()));
        let seen = || looks?.since.and_then(|since| u64::try_from(since).ok());
        let own = pane.since.filter(|_| told);
        let since = if shell { seen().or(own) } else { own.or_else(seen) };
        let hover = match &self.hover {
            Some(Hover::Part(id, part)) if *id == pane.id => Some(*part),
            _ => None,
        };
        chrome::Header {
            // A member's color, as on the dashboard; none for the dashboard, the journal, the menu.
            color: Some(member_color(&self.order, &pane.member)),
            model: looks.and_then(|looks| looks.model.as_deref()),
            effort: looks.and_then(|looks| looks.effort.as_deref()),
            context,
            pressure: looks.map(|looks| looks.pressure).unwrap_or_default(),
            compactable,
            shell,
            error,
            since: since.map(|since| epoch.saturating_sub(since)),
            zoom: Some(if zoomed == Some(pane.id.as_str()) { chrome::Zoom::Out } else { chrome::Zoom::In }),
            hover,
            // The text a list is open under stays lit.
            open: self.list.as_ref().filter(|list| list.pane == pane.id).map(|list| list.part),
            ..base
        }
    }

    /// How many members work, wait for the user, failed their turn and rest, for the dashboard's border (mock-ups B1
    /// and C), as their programs say it; for one whose program says nothing (no OSC 7501, or `Clear`), as the
    /// dashboard last saw it (`looks`).
    fn counts(&self) -> chrome::Hint {
        let (mut working, mut waiting, mut error, mut idle) = (0, 0, 0, 0);
        for pane in self.panes.iter().filter(|pane| pane.role.is_empty()) {
            match pane_state(pane, &self.looks) {
                look::State::Working => working += 1,
                look::State::Waiting => waiting += 1,
                look::State::Error => error += 1,
                look::State::Idle => idle += 1,
                look::State::Other => {}
            }
        }
        chrome::Hint::Counts { working, waiting, error, idle }
    }

    /// The screen as it is, in `self.screen`: the shown tab's panes in their frames, the bar. False while an engine
    /// is broken.
    fn compose(&mut self) -> bool {
        let (width, height) = (self.size.0 as usize, self.size.1 as usize);
        // The cards' times in their states tick.
        let secs = now();
        for (card, began) in self.cards.iter_mut().zip(&self.card_epoch) {
            if let Some(began) = began {
                card.since = secs.saturating_sub(*began);
            }
        }
        let held = self.gate.hold();
        let mut shown: Vec<&Pane> = self.shown_ids().into_iter().filter_map(|id| self.pane(&id)).collect();
        let zoomed = self.tabs.get(self.active).and_then(|tab| tab.zoomed.as_deref());
        // The menu last: drawn over the others.
        shown.extend(self.menu.as_ref().and_then(|(menu, _)| self.pane(menu)));
        let engines: Vec<_> =
            shown.iter().map(|pane| pane.engine.lock().unwrap_or_else(PoisonError::into_inner)).collect();
        if shown.iter().any(|pane| pane.broken.is()) {
            return false;
        }
        let notes: Vec<Option<String>> = shown.iter().map(|pane| note(pane)).collect();
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let mut views: Vec<View<'_>> = shown
            .iter()
            .zip(&engines)
            .zip(&notes)
            .map(|((pane, engine), note)| View {
                id: &pane.id,
                engine: engine.as_ref(),
                area: pane.area,
                header: self.header(pane, note.as_deref(), zoomed, epoch.as_secs()),
                focused: self.focus.as_deref() == Some(pane.id.as_str()),
                selection: self
                    .selection
                    .as_ref()
                    .filter(|(selection, _)| selection.pane == pane.id)
                    .and_then(|(selection, _)| selection.range()),
                // The menu's list draws its own frame.
                bare: self.list.is_some() && self.menu.as_ref().is_some_and(|(menu, _)| *menu == pane.id),
            })
            .collect();
        common_forms(&mut views);
        // The cards' frame, under the menu if it is open (the menu is the last view).
        let glyphs = self.client.as_ref().map_or(Glyphs::Unicode, glyphs);
        let card_clock = board::Clock { now: secs as i64, frame: look::frame(self.born.elapsed()), glyphs };
        let panel = Panel { cards: &self.cards, wide: width >= layout::FOCUS_WIDE, clock: card_clock };
        let title = cards::title();
        if let Some(area) = self.cards_rect().filter(|_| !self.cards.is_empty()) {
            let at = views.len() - usize::from(self.menu.is_some());
            let header = chrome::Header { name: &title, hint: Some(cards::counts(&self.cards)), ..Default::default() };
            let frame = View { id: CARDS, engine: &panel, area, header, focused: false, selection: None, bare: false };
            views.insert(at, frame);
        }
        let tabs: Vec<chrome::Tab<'_>> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let states = tab.panes.iter().filter_map(|id| self.pane(id)).map(|pane| pane_state(pane, &self.looks));
                let zoomed = tab.zoomed.as_deref().and_then(|id| self.pane(id)).map(|pane| pane.member.as_str());
                chrome::Tab {
                    title: &tab.title,
                    active: i == self.active,
                    state: urgent(states),
                    zoomed,
                    switch: if i == self.active { self.switch(i) } else { None },
                }
            })
            .collect();
        // The spinner turns while a member at work shows, in a header or a tab.
        let working = views.iter().map(|view| view.header.state).chain(tabs.iter().map(|tab| tab.state));
        let working = working.into_iter().any(|state| state == look::State::Working)
            || views.iter().any(|view| view.id == CARDS)
                && self.cards.iter().any(|card| card.state == look::State::Working);
        let bar = Rect { x: 0, y: height.saturating_sub(1), width, height: usize::from(height > 0) };
        let look = chrome::Look { glyphs, frame: look::frame(self.born.elapsed()) };
        let team = self.team.as_str();
        let screen = &self.screen;
        // Over the team, the rest dimmed: a choice, else the menu (the last view).
        // A list under a header's text dims nothing: the team stays as it is around it.
        let over = match (&self.choice, &self.menu) {
            (Some(open), _) => screen::Over::Choice(&open.choice),
            (None, Some(_)) if self.list.is_none() => screen::Over::LastView,
            _ => screen::Over::Nothing,
        };
        let hover = match &self.hover {
            Some(Hover::Bar(target)) => Some(*target),
            _ => None,
        };
        let notice = self.notice.as_ref().map(|(notice, _)| notice);
        let composed = super::caught(|| {
            screen.borrow_mut().compose(width, height, &views, team, &tabs, bar, hover, look, over, notice)
        });
        // The times shown, ticking: the next image when the first of them changes (not under a layer, dimmed). Under a
        // minute, only those a header has room for (`shows_since` lays the header out: asked of these only); beyond,
        // a wake-up a minute costs nothing worth asking.
        let covered = self.choice.is_some() || self.menu.is_some();
        let shows = |view: &View<'_>| {
            view.header.since.filter(|secs| *secs >= 60 || chrome::shows_since(&view.header, view.area.width))
        };
        let ticks = views.iter().filter_map(shows).map(|secs| tick(secs, epoch.subsec_nanos()));
        // The cards' times change every second, the spinner aside.
        let cards = self.cards.iter().filter(|_| views.iter().any(|view| view.id == CARDS));
        let ticks = ticks.chain(cards.map(|card| tick(card.since, epoch.subsec_nanos())));
        let clock = if covered { None } else { ticks.min().map(|after| Instant::now() + after) };
        let ids: Vec<String> = views.iter().map(|view| view.id.to_string()).collect();
        // The zones, or whether an engine is to blame.
        let zones = match composed {
            Ok(zones) => Ok(zones),
            Err(why) => {
                // Which engine: each one asked alone what compose asks of it.
                let mut found = false;
                for (pane, view) in shown.iter().zip(views.iter().filter(|view| view.id != CARDS)) {
                    let mut alone = Canvas::new(width, height);
                    let replay = super::caught(|| {
                        view.engine.draw(&mut alone, view.inside());
                        (view.engine.cursor(), view.engine.scrolled(), view.engine.history())
                    });
                    if let Err(why) = replay {
                        pane.broken.mark(why);
                        found = true;
                    }
                }
                log(&format!("screen: {why}"));
                Err(found)
            }
        };
        drop(views);
        drop(engines);
        drop(shown);
        drop(held);
        let zones = match zones {
            Ok(zones) => zones,
            Err(found) => {
                // No engine to blame: the screen would fail again at each frame. The next change asks again.
                if !found {
                    self.dirty = false;
                }
                return false;
            }
        };
        self.zones = zones.bar;
        self.parts =
            zones.parts.into_iter().filter_map(|(at, i, part)| Some((at, ids.get(i)?.clone(), part))).collect();
        self.notice_zone = zones.notice;
        self.clock = clock;
        if let Some(open) = self.choice.as_mut() {
            open.zones = zones.options;
        }
        // Under a layer (the menu, a choice), the spinner shows dimmed: not worth a frame each `look::FRAME`.
        self.animate = (working && !covered).then(|| Instant::now() + look::FRAME);
        true
    }

    /// Opens a pane in tab `tab` (made if missing; the active one when `None`), and gives it the focus if it is the
    /// first.
    fn open(&mut self, spec: PaneSpec, tab: Option<String>) -> Result<String> {
        let id = self.spawn(spec)?;
        let at = match tab {
            Some(title) => match self.tabs.iter().position(|tab| tab.title == title) {
                Some(at) => at,
                None => {
                    self.tabs.push(Tab::new(title, Vec::new()));
                    self.tabs.len() - 1
                }
            },
            None if self.tabs.is_empty() => {
                self.tabs.push(Tab::new("1".into(), Vec::new()));
                0
            }
            None => self.active,
        };
        self.tabs[at].panes.push(id.clone());
        if self.focus.is_none() {
            self.focus = Some(id.clone());
            self.active = at;
        }
        self.layout();
        Ok(id)
    }

    /// Starts a pane as `spec` says, in no tab yet: its id.
    fn spawn(&mut self, spec: PaneSpec) -> Result<String> {
        if !Path::new(&spec.cwd).is_absolute() {
            bail!(t!("dossier relatif : {}", "relative directory: {}", spec.cwd));
        }
        self.next_pane += 1;
        let id = format!("p{}", self.next_pane);
        let caps = self.client.as_ref().map(|client| client.caps.clone()).unwrap_or_default();
        let wires = Wires { tx: self.tx.clone(), gate: Arc::clone(&self.gate) };
        let command = self.with_env(Command { argv: spec.argv, cwd: PathBuf::from(spec.cwd), env: spec.env });
        // Its size comes with the layout; a first guess meanwhile.
        let size = (self.size.0.max(3) - 2, self.size.1.saturating_sub(3).max(1));
        let pane = Pane::open(id.clone(), spec.member, spec.role, command, size, &caps, &wires)?;
        self.panes.push(pane);
        self.started = true;
        Ok(id)
    }

    /// The variables the last client gave, over the command's environment.
    fn with_env(&self, mut command: Command) -> Command {
        for (name, value) in &self.env {
            command.env.retain(|(key, _)| key != name);
            if let Some(value) = value {
                command.env.push((name.clone(), value.clone()));
            }
        }
        command
    }

    /// Removes pane `id`, its program asked to end if it still runs.
    fn close_pane(&mut self, id: &str) {
        let Some(i) = self.panes.iter().position(|pane| pane.id == id) else { return };
        // Dropped: its program's group is stopped.
        self.panes.remove(i);
        // The menu gives the focus back to the pane that had it, if it is still there.
        let mut back = None;
        if self.menu.as_ref().is_some_and(|(menu, _)| menu == id) {
            back = self.menu.take().and_then(|(_, before)| before);
            self.list = None;
        }
        for tab in &mut self.tabs {
            tab.forget(id);
        }
        let before = self.tabs.len();
        self.tabs.retain(|tab| !tab.ids().is_empty());
        if self.tabs.len() < before {
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        }
        if self.focus.as_deref() == Some(id) {
            // Gone: no focus out to tell. Else the pane that had it, or the first of the tab shown; told (1004).
            self.focus = None;
            let first = self.tabs.get(self.active).and_then(|tab| tab.ids().first().cloned());
            if let Some(next) = back.filter(|back| self.pane(back).is_some()).or(first) {
                self.focus_on(&next);
            }
        }
        if self.grab.as_deref() == Some(id) {
            self.grab = None;
        }
        self.layout();
    }

    /// A pane by its id, else by its member's name, else by the role of a pane that is no member's (« menu »,
    /// « dashboard », « journal »).
    fn find(&self, which: &str) -> Option<&Pane> {
        self.pane(which)
            .or_else(|| self.panes.iter().find(|pane| pane.member == which))
            .or_else(|| self.panes.iter().find(|pane| !pane.role.is_empty() && pane.role == which))
    }

    /// A command's answer. The members' order, which gives their colors, is read again after a request that may
    /// follow a change of the team (`launch`, `live.rs`): here, never on the way of a frame.
    fn request(&mut self, request: Request) -> Reply {
        // Not after `SetMember`: `live.rs` sends it before it writes team.json, and it renames in place.
        let changes = matches!(request, Request::Build { .. } | Request::OpenWindow { .. } | Request::Arrange { .. });
        let reply = self.answer(request);
        if changes {
            self.read_order();
        }
        reply
    }

    /// The members in the team's order (`team.json`), as the dashboard and the journal color them.
    fn read_order(&mut self) {
        match Snapshot::read(&self.state) {
            Ok(snapshot) => self.order = snapshot.members.into_iter().map(|member| member.name).collect(),
            Err(error) => log(&format!("team.json: {error:#}")),
        }
        self.dirty = true;
    }

    fn answer(&mut self, request: Request) -> Reply {
        let ok = |value: serde_json::Value| Reply::Ok(value);
        let unknown = |which: &str| Reply::Err(t!("panneau inconnu : {}", "unknown pane: {}", which));
        match request {
            Request::Stop => Reply::Ok(serde_json::Value::Null),
            // The tabs' panes, then the menu's, floating, in no tab (`_ctl` finds it by its member, « recruit »);
            // whoever looks for members goes by their role.
            Request::Panes => {
                let menu = self.menu.as_ref().map(|(menu, _)| (None, menu.clone()));
                let list: Vec<PaneInfo> = self
                    .tabs
                    .iter()
                    .flat_map(|tab| tab.ids().into_iter().map(move |id| (Some(tab), id)))
                    .chain(menu)
                    .filter_map(|(tab, id)| self.pane(&id).map(|pane| (tab, pane)))
                    .map(|(tab, pane)| PaneInfo {
                        id: pane.id.clone(),
                        member: pane.member.clone(),
                        role: pane.role.clone(),
                        tab: tab.map(|tab| tab.title.clone()).unwrap_or_default(),
                        pid: pane.pty.pid(),
                        cols: pane.size.0,
                        rows: pane.size.1,
                        // Its program's own, never the dashboard's (`pane_state`): the dashboard reads it, and would
                        // read back its own echo.
                        status: Some(look_state(pane.state))
                            .filter(|state| *state != look::State::Other)
                            .map(|state| state.name().to_string()),
                        since: pane.since.filter(|_| look_state(pane.state) != look::State::Other),
                        error: pane.error.as_ref().and_then(|error| error.message.clone()),
                    })
                    .collect();
                ok(serde_json::to_value(list).unwrap_or_default())
            }
            Request::KillPane { id } => match self.find(&id).map(|pane| pane.id.clone()) {
                Some(id) => {
                    self.close_pane(&id);
                    ok(serde_json::Value::Null)
                }
                None => unknown(&id),
            },
            Request::SetMember { id, member } => match self.panes.iter_mut().find(|pane| pane.id == id) {
                Some(pane) => {
                    // Its color follows it: `live.rs` writes team.json only after this.
                    if let Some(name) = self.order.iter_mut().find(|name| **name == pane.member) {
                        name.clone_from(&member);
                    }
                    pane.member = member;
                    self.dirty = true;
                    ok(serde_json::Value::Null)
                }
                None => unknown(&id),
            },
            Request::Focus { member } => {
                let id =
                    self.panes.iter().find(|pane| pane.member == member && pane.role.is_empty()).map(|p| p.id.clone());
                if let Some(id) = &id {
                    self.focus_on(id);
                }
                ok(serde_json::Value::Bool(id.is_some()))
            }
            Request::Detach { client } => {
                if self.client.as_ref().is_some_and(|c| c.id == client) {
                    let _ = self.drop_client(Some(Bye::Detached));
                    ok(serde_json::Value::Null)
                } else {
                    Reply::Err(t!("client inconnu : {}", "unknown client: {}", client))
                }
            }
            Request::Clients => {
                let list: Vec<ClientInfo> = self
                    .client
                    .iter()
                    .map(|client| ClientInfo {
                        id: client.id.clone(),
                        term: client.term.clone(),
                        name: client.caps.name.clone(),
                        cols: client.size.0,
                        rows: client.size.1,
                        since: client.since,
                    })
                    .collect();
                ok(serde_json::to_value(list).unwrap_or_default())
            }
            Request::Stats => {
                let panes = self
                    .panes
                    .iter()
                    .map(|pane| PaneStats {
                        id: pane.id.clone(),
                        member: pane.member.clone(),
                        pid: pane.pty.pid(),
                        cols: pane.size.0,
                        rows: pane.size.1,
                        history: pane.with(|engine| engine.history()).unwrap_or(0),
                        read: pane.read.load(Ordering::Relaxed),
                    })
                    .collect();
                let stats = Stats { pid: std::process::id(), panes, frames: self.frames, written: self.written };
                ok(serde_json::to_value(stats).unwrap_or_default())
            }
            Request::Capture { pane, styles: _, history } => match pane {
                None => match self.compose() {
                    true => ok(serde_json::to_value(captured(self.screen.borrow().canvas(), None)).unwrap_or_default()),
                    false => Reply::Err(t!("un moteur est en échec", "an engine failed")),
                },
                Some(which) => match self.find(&which) {
                    Some(pane) => {
                        let (cols, rows) = (pane.size.0 as usize, pane.size.1 as usize);
                        let mut canvas = Canvas::new(cols, rows);
                        let area = Rect { x: 0, y: 0, width: cols, height: rows };
                        let drawn = pane.with(|engine| {
                            engine.draw(&mut canvas, area);
                            let modes = engine.modes();
                            let mouse = match modes.mouse {
                                None => 0,
                                Some(MouseTracking::Click) => 1000,
                                Some(MouseTracking::Drag) => 1002,
                                Some(MouseTracking::Motion) => 1003,
                            };
                            let shown = engine.scrolled() == 0;
                            let cursor = engine.cursor().filter(|_| shown).map(|c| (c.x, c.y));
                            // The history above the live screen: its last `history` lines still kept.
                            let live = engine.top() + engine.scrolled() as u64;
                            let first = live.saturating_sub(history as u64).max(engine.oldest());
                            let above: Vec<String> = (first..live)
                                .filter_map(|line| engine.line(line))
                                .map(|line| line.text.trim_end().to_string())
                                .collect();
                            let modes = PaneModes {
                                kitty: modes.kitty,
                                bracketed_paste: modes.bracketed_paste,
                                focus: modes.focus,
                                mouse,
                                mouse_sgr: modes.mouse_sgr,
                                alt_screen: modes.alt_screen,
                                app_cursor: modes.app_cursor,
                                scrolled: engine.scrolled(),
                                history: engine.history(),
                            };
                            (cursor, modes, above)
                        });
                        match drawn {
                            Some((cursor, modes, above)) => {
                                let mut captured = captured(&canvas, Some(modes));
                                captured.cursor = cursor;
                                captured.history = above;
                                ok(serde_json::to_value(captured).unwrap_or_default())
                            }
                            None => Reply::Err(t!("le moteur du panneau est en échec", "the pane's engine failed")),
                        }
                    }
                    None => unknown(&which),
                },
            },
            Request::Send { pane, bytes } => match self.find(&pane) {
                Some(pane) => {
                    pane.send(bytes);
                    ok(serde_json::Value::Null)
                }
                None => unknown(&pane),
            },
            Request::Key { pane: None, event } => {
                self.input(event);
                ok(serde_json::Value::Null)
            }
            Request::Key { pane: Some(which), event } => match self.find(&which) {
                Some(pane) => {
                    let modes = pane.modes();
                    let bytes = match &event {
                        Event::Key(key) => input::key(key, &modes),
                        Event::Paste(text) => input::paste(text, &modes),
                        Event::Focus(gained) => input::focus(*gained, &modes).unwrap_or_default(),
                        Event::Mouse(mouse) => input::mouse(mouse, mouse.col, mouse.row, &modes).unwrap_or_default(),
                        Event::Resize(..) | Event::Closed(_) => Vec::new(),
                    };
                    pane.send(bytes);
                    ok(serde_json::Value::Null)
                }
                None => unknown(&which),
            },
            Request::Spawn { member, argv, env, cwd, tab } => {
                if self.panes.iter().any(|pane| pane.member == member) {
                    return Reply::Err(t!("le panneau « {} » existe déjà", "pane \"{}\" already exists", member));
                }
                let spec = PaneSpec { member, role: String::new(), argv, env, cwd };
                answer(self.open(spec, tab))
            }
            Request::Build { team, dir, columns, tabs, cols, rows, exe, lang, focus } => {
                self.focus_layout = focus;
                self.exe = Some(exe).filter(|exe| !exe.is_empty());
                if !lang.is_empty() {
                    self.lang = lang;
                }
                answer(self.build(team, dir, columns, tabs, (cols, rows)))
            }
            Request::OpenWindow { pane } => {
                let title = pane.member.clone();
                let opened = self.spawn(pane);
                if let Ok(id) = &opened {
                    self.tabs.push(Tab::new(title, vec![id.clone()]));
                    self.layout();
                }
                answer(opened)
            }
            Request::Respawn { id, pane } => answer(self.respawn(&id, pane)),
            Request::Arrange { tabs, columns, focus } => {
                self.focus_layout = focus;
                self.arrange(tabs, columns);
                ok(serde_json::Value::Null)
            }
            Request::OpenPanels { beside, dashboard, journal } => answer(self.open_panels(&beside, dashboard, journal)),
            Request::ClosePanels => {
                self.close_panels();
                ok(serde_json::Value::Null)
            }
            Request::RestoreDashboard { dashboard } => answer(self.restore_dashboard(dashboard)),
            Request::ToggleJournal { journal } => answer(self.toggle_journal(journal)),
            // The member names the client (one only, policy A); with a field, the sheet too.
            Request::OpenMenu { member, client, field } => {
                let sheet = member.as_deref().zip(field).map(|(member, field)| (member, Some(field)));
                answer(self.open_menu(client.as_deref(), sheet))
            }
        }
    }

    /// The team as `launch` plans it, in a server still empty.
    fn build(&mut self, team: String, dir: String, columns: usize, tabs: Vec<TabSpec>, size: (u16, u16)) -> Result<()> {
        if self.started {
            bail!(t!("l'équipe est déjà construite", "the team is already built"));
        }
        self.team = team.clone();
        {
            let mut welcome = self.welcome.lock().unwrap_or_else(PoisonError::into_inner);
            welcome.team = team;
            welcome.dir = dir;
        }
        self.columns = columns.max(1);
        if self.client.is_none() && size.0 > 0 && size.1 > 0 {
            self.size = size;
        }
        for spec in tabs {
            let mut panes = Vec::new();
            for pane in spec.panes {
                panes.push(self.spawn(pane)?);
            }
            let mut tab = Tab::new(spec.title, panes);
            if let Some((dashboard, journal)) = spec.side {
                self.journal = Some(journal.clone());
                tab.side = Some(Side { dashboard: Some(self.spawn(dashboard)?), journal: Some(self.spawn(journal)?) });
            }
            self.tabs.push(tab);
        }
        self.reduced = true;
        self.active = 0;
        self.focus = self.tabs.first().and_then(|tab| tab.ids().first().cloned());
        self.layout();
        Ok(())
    }

    /// Pane `id`'s program started again, in place.
    fn respawn(&mut self, id: &str, spec: PaneSpec) -> Result<()> {
        if !Path::new(&spec.cwd).is_absolute() {
            bail!(t!("dossier relatif : {}", "relative directory: {}", spec.cwd));
        }
        let command = self.with_env(Command { argv: spec.argv, cwd: PathBuf::from(spec.cwd), env: spec.env });
        let caps = self.client.as_ref().map(|client| client.caps.clone()).unwrap_or_default();
        let wires = Wires { tx: self.tx.clone(), gate: Arc::clone(&self.gate) };
        let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) else {
            bail!(t!("panneau inconnu : {}", "unknown pane: {}", id));
        };
        pane.respawn(command, &caps, &wires)?;
        self.dirty = true;
        Ok(())
    }

    /// The members' panes put in `tabs`, none stopped; the panels' column beside the first. A member's pane in none
    /// of them keeps a tab of its own, at the end.
    fn arrange(&mut self, tabs: Vec<(String, Vec<String>)>, columns: usize) {
        self.columns = columns.max(1);
        let side = self.tabs.iter_mut().find_map(|tab| tab.side.take());
        let member_pane = |name: &str| {
            self.panes.iter().find(|pane| pane.role.is_empty() && pane.member == name).map(|pane| pane.id.clone())
        };
        let mut arranged: Vec<Tab> = tabs
            .iter()
            .map(|(title, members)| Tab::new(title.clone(), members.iter().filter_map(|m| member_pane(m)).collect()))
            .filter(|tab| !tab.panes.is_empty())
            .collect();
        let placed: Vec<String> = arranged.iter().flat_map(|tab| tab.panes.clone()).collect();
        for pane in self.panes.iter().filter(|pane| pane.role.is_empty() && !placed.contains(&pane.id)) {
            arranged.push(Tab::new(pane.member.clone(), vec![pane.id.clone()]));
        }
        match arranged.first_mut() {
            Some(first) => first.side = side,
            None => {
                if let Some(side) = side {
                    arranged.push(Tab { side: Some(side), ..Tab::new(String::new(), Vec::new()) });
                }
            }
        }
        // Each tab keeps the pane it last had the focus on, wherever that pane went; and its zoom, if the pane zoomed
        // stays in a tab of the same title (any change from the menu ends here: a new effort keeps the zoom).
        let lasts: Vec<String> = self.tabs.iter().filter_map(|tab| tab.last.clone()).collect();
        let zooms: Vec<(&str, &str)> =
            self.tabs.iter().filter_map(|tab| Some((tab.title.as_str(), tab.zoomed.as_deref()?))).collect();
        for tab in &mut arranged {
            tab.last = lasts.iter().find(|last| tab.has(last)).cloned();
            let zoom = zooms.iter().find(|(title, id)| *title == tab.title && tab.panes.iter().any(|pane| pane == id));
            tab.zoomed = zoom.map(|(_, id)| id.to_string());
        }
        // ⌥l and the places of a tab in « focus » stay with a tab of the same title.
        for tab in &mut arranged {
            if let Some(old) = self.tabs.iter_mut().find(|old| old.title == tab.title) {
                tab.mode = old.mode;
                tab.focus = old.focus.take();
            }
        }
        self.tabs = arranged;
        self.active = self.focus.as_deref().and_then(|id| self.tabs.iter().position(|tab| tab.has(id))).unwrap_or(0);
        self.layout();
    }

    /// The dashboard over the reduced journal, beside pane `beside`.
    fn open_panels(&mut self, beside: &str, dashboard: PaneSpec, journal: PaneSpec) -> Result<()> {
        let Some(at) = self.tabs.iter().position(|tab| tab.has(beside)) else {
            bail!(t!("panneau inconnu : {}", "unknown pane: {}", beside));
        };
        self.journal = Some(journal.clone());
        let side = Side { dashboard: Some(self.spawn(dashboard)?), journal: Some(self.spawn(journal)?) };
        self.tabs[at].side = Some(side);
        self.reduced = true;
        self.layout();
        Ok(())
    }

    /// The team's menu (`recruit _menu`) in a pane floating over the team, on the client: `client`, else the one there
    /// is. A menu already open stays: one at a time.
    fn open_menu(&mut self, client: Option<&str>, sheet: Option<(&str, Option<MenuField>)>) -> Result<MenuOpened> {
        self.open_menu_as(client, sheet, None)
    }

    /// [`Server::open_menu`], as a `list` under a header's text if given: `recruit _menu … --list`.
    fn open_menu_as(
        &mut self,
        client: Option<&str>,
        sheet: Option<(&str, Option<MenuField>)>,
        list: Option<List>,
    ) -> Result<MenuOpened> {
        let Some(attached) = self.client.as_ref().filter(|c| client.is_none_or(|id| c.id == id)) else {
            return Ok(MenuOpened::NoClient);
        };
        if self.menu.is_some() {
            return Ok(MenuOpened::AlreadyOpen);
        }
        let Some(exe) = self.exe.clone() else {
            bail!(t!("le menu n'est pas disponible pour cette équipe", "the menu is not available for this team"));
        };
        let mut argv = vec![exe, "--lang".into(), self.lang.clone(), "_menu".into()];
        argv.push(self.state.to_string_lossy().into_owned());
        argv.extend(["--client".into(), attached.id.clone()]);
        if glyphs(attached) == Glyphs::Nerd {
            argv.push("--nerd".into());
        }
        if self.light {
            argv.push("--light".into());
        }
        if let Some((member, field)) = sheet {
            argv.extend(["--member".into(), member.to_string()]);
            if let Some(field) = field.and_then(MenuField::arg) {
                argv.extend(["--field".into(), field.into()]);
            }
        }
        if list.is_some() {
            argv.push("--list".into());
        }
        let cwd = self.welcome.lock().unwrap_or_else(PoisonError::into_inner).dir.clone();
        let cwd = if cwd.is_empty() { "/".to_string() } else { cwd };
        let spec = PaneSpec { member: "recruit".into(), role: MENU.into(), argv, env: Vec::new(), cwd };
        let id = self.spawn(spec)?;
        let before = self.focus.clone();
        self.menu = Some((id.clone(), before));
        self.list = list;
        self.focus_on(&id);
        Ok(MenuOpened::Opened)
    }

    /// The menu's place: 100 × 28 inside its frame, as the mock-ups draw it, centered; less in a smaller screen.
    fn menu_area(&self) -> Rect {
        if let Some(list) = &self.list {
            return list_area(list.anchor, list.size, (self.size.0 as usize, (self.size.1 as usize).saturating_sub(1)));
        }
        let (width, height) = (self.size.0 as usize, (self.size.1 as usize).saturating_sub(1));
        let fit = |wanted: usize, room: usize| wanted.min(room.saturating_sub(2)).max(1).min(room);
        let (w, h) = (fit(102, width), fit(30, height));
        Rect { x: (width - w) / 2, y: (height - h) / 2, width: w, height: h }
    }

    /// The menu, on the client: on the sheet of the active pane's member (⌥r and the bar's button, F7), else on the
    /// menu's first sheet (the dashboard or the journal active).
    fn menu_here(&mut self) {
        let member = self.focused_member().map(str::to_string);
        if let Err(error) = self.open_menu(None, member.as_deref().map(|member| (member, None))) {
            log(&format!("menu: {error:#}"));
        }
    }

    /// The dashboard and the journal closed.
    fn close_panels(&mut self) {
        let panels: Vec<String> = self
            .panes
            .iter()
            .filter(|pane| pane.role == DASHBOARD || pane.role == JOURNAL)
            .map(|pane| pane.id.clone())
            .collect();
        for id in panels {
            self.close_pane(&id);
        }
        self.journal = None;
    }

    /// The dashboard opened again: over the journal, which keeps its size, else beside the first member.
    fn restore_dashboard(&mut self, dashboard: PaneSpec) -> Result<()> {
        let at = self.tabs.iter().position(|tab| tab.side.is_some()).unwrap_or(0);
        if self.tabs.is_empty() {
            bail!(t!("l'équipe n'a aucun panneau", "the team has no pane"));
        }
        let id = self.spawn(dashboard)?;
        self.tabs[at].side.get_or_insert_with(Side::default).dashboard = Some(id);
        self.layout();
        Ok(())
    }

    /// The journal to its next size: full, reduced, hidden (closed), full again.
    fn toggle_journal(&mut self, journal: PaneSpec) -> Result<&'static str> {
        self.journal = Some(journal.clone());
        let open = self.tabs.iter().position(|tab| tab.side.as_ref().is_some_and(|side| side.journal.is_some()));
        let next = match (open, self.reduced) {
            (None, _) => "full",
            (Some(_), false) => "reduced",
            (Some(_), true) => "hidden",
        };
        match next {
            "reduced" => self.reduced = true,
            "hidden" => {
                let id = open.and_then(|at| self.tabs[at].side.as_ref()?.journal.clone());
                if let Some(id) = id {
                    self.close_pane(&id);
                }
            }
            _ => {
                let Some(at) = self.tabs.iter().position(|tab| tab.side.is_some()) else {
                    bail!(t!(
                        "le tableau de bord est fermé : relancer recruit le rouvre",
                        "the dashboard is closed: running recruit again opens it"
                    ));
                };
                let id = self.spawn(journal)?;
                self.tabs[at].side.get_or_insert_with(Side::default).journal = Some(id);
                self.reduced = false;
            }
        }
        self.layout();
        Ok(next)
    }

    /// The socket again, gone from its folder (SIGURG, §2.4): a new listener and its thread. The old thread stays
    /// in `accept` on the old one (macOS does not wake it).
    fn reopen_socket(&mut self) {
        if fs::symlink_metadata(&self.socket).is_ok() {
            return;
        }
        match socket::listen(&self.socket) {
            Ok((listener, id)) => {
                self.socket_id = id;
                if let Err(error) = accept(listener, self.tx.clone(), Arc::clone(&self.welcome)) {
                    log(&format!("accept: {error}"));
                }
                log("socket made again");
            }
            Err(error) => log(&format!("socket: {error:#}")),
        }
    }

    /// The team stops (§1.3): the client told, every program asked to end and waited for, the socket and
    /// `server.json` removed, then the answer to whoever asked, written before the server goes.
    fn stop(mut self, rx: &Receiver<Msg>, bye: Bye, asker: Option<Asker>) {
        log(&format!("stopping: {bye:?}"));
        let client = self.drop_client(Some(bye));
        for pane in &self.panes {
            pane.pty.kill();
        }
        let until = Instant::now() + STOP_TIME;
        let mut running = self.panes.iter().filter(|pane| !pane.exited).count();
        while running > 0 {
            match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(Msg::Exited(..)) => running -= 1,
                Ok(Msg::Command(_, reply, _)) => {
                    let _ = reply.send(Reply::Err(t!("l'équipe s'arrête", "the team is stopping")));
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let _ = fs::remove_file(socket::info_file(&self.state));
        socket::remove_own(&self.socket, self.socket_id);
        // Dropped: what still runs is forced (`Pty`).
        self.panes.clear();
        // The client hears why before the server leaves, else it would take a normal stop for a crash.
        if let Some(out) = client {
            out.wait_closed(BYE_TIME);
        }
        if let Some((reply, written)) = asker {
            // The server leaves only once the answer is written: whoever asked then knows the team stopped.
            if reply.send(Reply::Ok(serde_json::Value::Null)).is_ok() {
                let _ = written.recv_timeout(ANSWER_TIME);
            }
        }
        log("stopped");
    }
}

/// A result as a command's answer.
fn answer<T: serde::Serialize>(result: Result<T>) -> Reply {
    match result {
        Ok(value) => Reply::Ok(serde_json::to_value(value).unwrap_or_default()),
        Err(error) => Reply::Err(format!("{error:#}")),
    }
}

/// The signs the client's terminal can show.
fn glyphs(client: &Client) -> Glyphs {
    let name = client.caps.name.clone().unwrap_or_default();
    Glyphs::of(&format!("{}\t{name}", client.term))
}

/// A pane's state, as the chrome draws it.
fn look_state(state: Option<State>) -> look::State {
    match state {
        Some(State::Working) => look::State::Working,
        Some(State::Blocked) => look::State::Waiting,
        Some(State::Idle | State::Done) => look::State::Idle,
        Some(State::Error) => look::State::Error,
        Some(State::Clear) | None => look::State::Other,
    }
}

/// What its program says of its state, noted in a pane: since when (from its first state, then at each change), and
/// why its turn failed, kept until another state.
fn note_status(pane: &mut Pane, status: &engine::Status) {
    if pane.since.is_none() || look_state(pane.state) != look_state(Some(status.state)) {
        pane.since = Some(now());
    }
    pane.state = Some(status.state);
    pane.error = (status.state == State::Error).then(|| TurnError {
        message: status.message.clone(),
        label: one_line(&look::error_label(status.message.as_deref())),
    });
}

/// A text from a pane's program on one line, for a header or a notice: its line breaks and tabs as spaces.
fn one_line(text: &str) -> String {
    text.chars().map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c }).collect()
}

/// Whether a member gone into `state` out of sight gets a notice: waiting for the user, or its turn failed.
fn calls(state: look::State) -> bool {
    matches!(state, look::State::Waiting | look::State::Error)
}

/// A pane's state, as its program says it (OSC 7501); for a member whose program says none, or `Clear`, as the
/// dashboard last saw it (`looks`): everywhere a member's state is shown (headers, counts, tabs, ⌥g, notices); never
/// in `Panes`, which the dashboard reads.
fn pane_state(pane: &Pane, looks: &BTreeMap<String, board::MemberLook>) -> look::State {
    match look_state(pane.state) {
        look::State::Other if pane.role.is_empty() => {
            looks.get(&pane.member).and_then(|looks| looks.state).unwrap_or(look::State::Other)
        }
        state => state,
    }
}

/// Since when a pane is in its [`pane_state`], in seconds since the epoch: by its program, else by the dashboard.
fn pane_since(pane: &Pane, looks: &BTreeMap<String, board::MemberLook>) -> Option<u64> {
    if look_state(pane.state) != look::State::Other || !pane.role.is_empty() {
        return pane.since;
    }
    looks.get(&pane.member)?.since.and_then(|since| u64::try_from(since).ok())
}

/// The member's pane after `focus` among a tab's `members`, in turn; the first when the focus is elsewhere (a panel).
/// None when it would stay where it is: a tab of one member.
fn next_member(members: &[String], focus: Option<&str>) -> Option<String> {
    let next = match members.iter().position(|id| Some(id.as_str()) == focus) {
        Some(at) => members.get((at + 1) % members.len()),
        None => members.first(),
    }?;
    (Some(next.as_str()) != focus).then(|| next.clone())
}

/// The members' panes as wide as each other take the same form of header, the poorest that one of them needs (but one
/// with nothing left at the right: `chrome::common_form`), for their columns to line up (designer, 2026-10-10): a
/// layout of each header on its own, as `chrome::frame` makes it for its drawing.
fn common_forms(views: &mut [View<'_>]) {
    let forms: Vec<Option<(usize, usize)>> = views
        .iter()
        .map(|view| view.header.zoom.map(|_| (view.area.width, chrome::form(&view.header, view.area.width))))
        .collect();
    for (view, form) in views.iter_mut().zip(&forms) {
        let Some((width, _)) = form else { continue };
        view.header.form =
            chrome::common_form(forms.iter().flatten().filter(|(w, _)| w == width).map(|(_, form)| *form));
    }
}

/// What a pane's program wrote last, for the log when it fails: its last lines up to the live screen's last row (its
/// `rows` rows),
/// a line wrapped joined to the next, the others by « ⏎ », at most [`LAST_WORDS`] characters.
fn last_words(engine: &mut dyn super::engine::Engine, rows: u16) -> String {
    const LINES: u64 = 40;
    let end = engine.top() + engine.scrolled() as u64 + u64::from(rows);
    let mut text = String::new();
    for line in end.saturating_sub(LINES).max(engine.oldest())..end {
        let Some(line) = engine.line(line) else { continue };
        if line.wrapped {
            text.push_str(&line.text);
        } else if !line.text.trim().is_empty() {
            text.push_str(line.text.trim_end());
            text.push_str(" ⏎ ");
        }
    }
    // What a program wrote goes to the log: no control (C0, C1) nor direction mark (as `canvas::bidi`), which would
    // act on whoever reads it in a terminal.
    let text: String = text.chars().filter(|&c| !c.is_control() && !direction(c)).collect();
    let text = text.trim_end_matches(" ⏎ ").trim();
    let skip = text.chars().count().saturating_sub(LAST_WORDS);
    text.chars().skip(skip).collect()
}

/// A mark that changes the direction of the text after it, as `canvas::bidi` knows them.
fn direction(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// The most of a failed pane's last words the log keeps.
const LAST_WORDS: usize = 600;

/// How long until a time shown as `secs` (12s, 22m, 1h05) reads otherwise: the next second under a minute, else the
/// next minute; `nanos`: how far into its second now is.
fn tick(secs: u64, nanos: u32) -> Duration {
    let into = Duration::from_nanos(u64::from(nanos));
    let whole = if secs < 60 { 1 } else { 60 - secs % 60 };
    Duration::from_secs(whole).saturating_sub(into).max(Duration::from_millis(1))
}

/// A member's own color, by its place in the team's order, as `board` gives it; one the order does not know yet
/// (a pane opened by `_ctl`) comes after the others.
fn member_color(order: &[String], member: &str) -> crossterm::style::Color {
    look::member_color(order.iter().position(|name| name == member).unwrap_or(order.len()))
}

/// The most urgent of states: waiting, else failed, else working, else at rest (mock-up C).
fn urgent(states: impl Iterator<Item = look::State>) -> look::State {
    let states: Vec<look::State> = states.collect();
    [look::State::Waiting, look::State::Error, look::State::Working, look::State::Idle]
        .into_iter()
        .find(|state| states.contains(state))
        .unwrap_or(look::State::Other)
}

/// What happened to a pane, for its header.
fn note(pane: &Pane) -> Option<String> {
    let mut notes = Vec::new();
    if let Some(why) = &pane.failure {
        notes.push(t!("moteur en échec : {}", "engine failed: {}", why));
    }
    if pane.exited {
        notes.push(t!("terminé", "exited"));
    }
    (!notes.is_empty()).then(|| notes.join(" · "))
}

/// A canvas as text, a line per row, the blanks at the end cut.
fn captured(canvas: &Canvas, modes: Option<PaneModes>) -> Captured {
    let lines = (0..canvas.height()).map(|y| canvas.row(y)).collect();
    Captured { lines, history: Vec::new(), cursor: canvas.cursor().map(|c| (c.x, c.y)), modes }
}

#[cfg(test)]
mod tests {
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
            names.iter().map(|name| crate::state::MemberInfo { name: name.to_string(), ..Default::default() });
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
            motion: None,
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
        let next = server.clock.expect("seconds shown").saturating_duration_since(Instant::now());
        assert!(next <= Duration::from_secs(1), "{next:?}");
        // Too narrow for the time: no wake-up each second.
        server.size = (12, 24);
        server.layout();
        assert!(server.compose());
        assert_eq!(server.clock, None);
        // Under a minute no more: a wake-up a minute, whatever the room.
        server.panes[0].since = Some(now() - 600);
        assert!(server.compose());
        assert!(server.clock.is_some());
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
        assert_eq!(
            server.counts(),
            chrome::Hint::Counts { working: 1, waiting: 0, error: 0, idle: 0 },
            "seen nowhere yet"
        );
        server.looks.get_mut("dev-b").unwrap().state = Some(look::State::Idle);
        assert_eq!(server.counts(), chrome::Hint::Counts { working: 1, waiting: 0, error: 0, idle: 1 });
        server.looks.get_mut("dev-a").unwrap().state = Some(look::State::Idle);
        assert_eq!(
            server.counts(),
            chrome::Hint::Counts { working: 1, waiting: 0, error: 0, idle: 1 },
            "its program says"
        );
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
        let mut server =
            built(state.path(), &["dev-a", "dev-b"], vec![TabSpec { title: "1".into(), panes, side: None }]);
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
        server.auto_compact = Some(70);
        server.auto_idle = 900;
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
        off.auto_compact = None;
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
        assert_eq!(server.asked.get("dev-a").map(|asked| Some(asked.since)), Some(server.panes[0].since));
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
        server.asked.insert("dev-a".into(), Asked { since, id: Some("7".into()), at, ended: false, failures });
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
            server.outcomes.clear();
            if let Some(answer) = answer {
                server.outcomes.insert("dev-a".into(), answer);
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
        seen.extend(server.card_ids.clone());
        seen.sort();
        let mut all: Vec<String> = names.iter().map(|name| id_of(&server, name)).collect();
        all.sort();
        assert_eq!(seen, all);
        let (width, height) = server.tab_room();
        assert!(rect.x + rect.width <= width && rect.y + rect.height <= height, "{rect:?}");
        for id in server.shown_ids() {
            let pane = server.pane(&id).unwrap().area;
            assert!(pane.x + pane.width <= width && pane.y + pane.height <= height);
            let apart =
                pane.x + pane.width <= rect.x || rect.x + rect.width <= pane.x || pane.y + pane.height <= rect.y;
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
        let waiting = server.cards[0].name.clone();
        let looks = BTreeMap::from([(
            waiting,
            board::MemberLook { state: Some(look::State::Waiting), since: Some(now() as i64), ..Default::default() },
        )]);
        server.message(Msg::Looks(looks));
        assert!(server.notice.is_none());
        server.refresh_cards();
        assert_eq!(server.cards.iter().filter(|card| card.state == look::State::Waiting).count(), 1, "red");
        // A click on a card brings its member to the left, the left one takes a place on the right.
        let wide = width >= layout::FOCUS_WIDE;
        let placed = cards::place(chrome::inside(rect), &server.cards, wide);
        let (index, card) = placed.shown[0];
        let member = server.card_ids[index].clone();
        let left = at.left.clone().unwrap();
        let (col, row) = (card.x as u16 + 1, card.y as u16);
        server.mouse(&Mouse { kind: MouseKind::Down(Button::Left), col, row, mods: Mods::NONE });
        server.mouse(&Mouse { kind: MouseKind::Up(Button::Left), col, row, mods: Mods::NONE });
        assert_eq!(places(&server).left.as_deref(), Some(member.as_str()));
        assert_eq!(server.focus.as_deref(), Some(member.as_str()));
        assert!(places(&server).right.contains(&left) || places(&server).others.contains(&left));
        assert_eq!(server.card_ids.len(), 2, "the cards made again");
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
        assert_eq!(
            (server.focus.as_deref(), places(&server).left.as_deref()),
            (Some(right.as_str()), Some(left.as_str()))
        );
        // Two: it is the left, the other takes its place.
        click(&mut server);
        assert_eq!(places(&server).left.as_deref(), Some(right.as_str()));
        assert!(places(&server).right.contains(&left));
        assert_eq!(server.focus.as_deref(), Some(right.as_str()));
        // Apart in time: two single presses.
        let other = places(&server).right[0].clone();
        server.header_click = None;
        let area = server.pane(&other).unwrap().area;
        let (col, row) = ((area.x + area.width - 1) as u16, area.y as u16);
        server.mouse(&Mouse { kind: MouseKind::Down(Button::Left), col, row, mods: Mods::NONE });
        server.header_click.as_mut().unwrap().0 -= DOUBLE_CLICK;
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
        server.pointer = Some((right.x + 2, right.y + 1));
        server.pointer_at = Some(tick);
        let (held, end) = server.holding(tick);
        assert!(held && end.is_some_and(|end| end > tick + Duration::from_secs(4)));
        server.pointer_at = Some(tick - Duration::from_secs(6));
        assert!(!server.holding(tick).0, "it has not moved for a while");
        server.pointer_at = Some(tick);
        server.pointer = Some((left.x + 2, left.y + 1));
        assert!(!server.holding(tick).0, "over the left pane, the places may move");
        server.pointer = Some((5, 23));
        assert!(!server.holding(tick).0, "on the bar too");
        // A key, a click: a second.
        server.pointer = None;
        server.last_key = Some(tick);
        let (held, end) = server.holding(tick);
        assert!(held && end.is_some_and(|end| end > tick && end <= tick + Duration::from_secs(1)));
        server.last_key = Some(tick - Duration::from_secs(2));
        server.last_click = Some(tick);
        assert!(server.holding(tick).0);
        server.last_click = None;
        server.last_key = None;
        // A member that waits or failed goes past the pane's minute: it comes in as soon as nothing holds the tab. One
        // at work respects it: the server wakes for the end of the minute.
        let d = id_of(&server, "dev-d");
        let look = |state, since| board::MemberLook { state: Some(state), since: Some(since), ..Default::default() };
        server.looks.insert("dev-d".into(), look(look::State::Waiting, epoch));
        server.last_key = Some(Instant::now());
        server.update_focus();
        assert_eq!(places(&server), before, "held");
        assert!(server.focus_wake.is_some(), "woken when the hold ends");
        server.last_key = None;
        server.update_focus();
        assert!(places(&server).right.contains(&d), "{:?}", places(&server));
        assert!(server.shown_ids().contains(&d));
        assert!(server.focus_wake.is_none(), "nothing more is due");

        // At work: held, then the minute of the pane it would replace.
        let (mut server, _client) = focused(state.path(), &["dev-a", "dev-b", "dev-c", "dev-d"]);
        let d = id_of(&server, "dev-d");
        // Two minutes at work already: its own 30 seconds are past.
        server.looks.insert("dev-d".into(), look(look::State::Working, epoch - 120));
        server.last_key = Some(Instant::now());
        server.update_focus();
        assert_eq!(places(&server), before, "held");
        server.last_key = None;
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
        server.auto_idle = 0;
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
        let Reply::Ok(list) = server.answer(Request::Panes) else { panic!("panes") };
        let list: Vec<PaneInfo> = serde_json::from_value(list).unwrap();
        let info = list.iter().find(|info| info.member == "dev-a").unwrap();
        assert_eq!((info.status.as_deref(), info.since), (Some("idle"), Some(since)));
        // Its end: the state of the last turn again; the compaction is over, a `working` is a turn's.
        tells(&mut server, &["idle"]);
        assert!(server.panes[0].held.is_none() && server.asked["dev-a"].ended);
        assert!(!server.compacting("dev-a", Some(since)));
        assert_eq!((shown(&server), server.panes[0].since), (look::State::Idle, Some(since)));
        tells(&mut server, &["working"]);
        assert_eq!(shown(&server), look::State::Working);

        // Made (`done`), the program said no other state (a message received meanwhile: its turn queued behind the
        // compaction begins right after it): the `working` held is that turn's, since the mod's answer.
        let mut server = resting(state.path(), 1000, 80);
        server.auto_idle = 0;
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
        server.auto_idle = 0;
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
            server.auto_idle = 0;
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
        server.auto_idle = 0;
        server.panes[0].with(|engine| engine.feed(b"\x1b]7501;?\x1b\\", &mut replies));
        tells(&mut server, &["idle"]);
        let since = server.panes[0].since.unwrap();
        server.plan_compaction(now());
        server.message(Msg::Requested("dev-a".into(), since, Ok("c4".into())));
        tells(&mut server, &["working"]);
        server.settle_compactions();
        assert_eq!(shown(&server), look::State::Idle);
        server.asked.get_mut("dev-a").unwrap().at = Instant::now() - COMPACTING;
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
        let watched = |server: &Server| server.watched.lock().unwrap().contains(&"dev-a".to_string());
        let write = |server: &mut Server, id: &str| {
            let since = server.asked["dev-a"].since;
            server.message(Msg::Requested("dev-a".into(), since, Ok(id.into())));
        };
        let wait_again = |server: &mut Server| server.asked.get_mut("dev-a").unwrap().at = Instant::now() - AUTO_AGAIN;

        assert_eq!(asked_now(&mut server), 1);
        assert_eq!(server.asked["dev-a"].failures, 0);
        assert!(watched(&server));
        assert_eq!(asked_now(&mut server), 0, "not twice in a row");
        assert!(server.compaction.is_some());
        // Three refusals for a subagent or a teammate in a row: asked each time, none counts, always watched.
        for round in 0..3 {
            let id = format!("h{round}");
            write(&mut server, &id);
            assert_eq!(server.asked["dev-a"].id.as_deref(), Some(id.as_str()));
            let refused =
                BTreeMap::from([("dev-a".to_string(), outcome(&id, false, Some(bridge::SKIP_HELPERS), None))]);
            server.message(Msg::Outcomes(refused));
            assert!(watched(&server), "round {round}");
            assert_eq!(asked_now(&mut server), 0, "5 minutes not passed");
            wait_again(&mut server);
            assert_eq!(asked_now(&mut server), 1, "round {round}");
            assert_eq!(server.asked["dev-a"].failures, 0, "round {round}");
            assert!(watched(&server), "the new request is waited for");
        }
        // Then it goes unanswered: once more 5 minutes after, then closed and no longer watched.
        write(&mut server, "u1");
        server.asked.get_mut("dev-a").unwrap().at = Instant::now() - AUTO_PENDING;
        assert_eq!(asked_now(&mut server), 0, "5 minutes not passed");
        wait_again(&mut server);
        assert_eq!(asked_now(&mut server), 1);
        assert_eq!(server.asked["dev-a"].failures, 1);
        // The last request is waited for, watched and not asked, while its answer may come.
        assert!(watched(&server));
        assert_eq!(asked_now(&mut server), 0);
        write(&mut server, "u2");
        server.asked.get_mut("dev-a").unwrap().at = Instant::now() - AUTO_AGAIN;
        assert_eq!(asked_now(&mut server), 0, "closed");
        assert!(!watched(&server));
        // A stale writer's id (another rest) is not kept.
        let since = server.asked["dev-a"].since;
        server.message(Msg::Requested("dev-a".into(), since - 1, Ok("old".into())));
        assert_eq!(server.asked["dev-a"].id.as_deref(), Some("u2"));
        // The compaction off: nothing asked nor watched.
        server.auto_compact = None;
        assert_eq!(asked_now(&mut server), 0);
        assert!(server.watched.lock().unwrap().is_empty());
        assert!(server.asked.is_empty());
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
        while server.notice.is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            server.take_relays();
        }
        let (notice, _) = server.notice.clone().expect("noticed");
        assert_eq!((notice.member.as_str(), notice.tab.as_str()), ("dev-b", "2"));
        assert_eq!(notice.text, chrome::notice_text("dev-b"));
        assert!(server.pane(&b).unwrap().since.is_some());
        // Under a layer: gone, and none comes.
        server.choice = Some(Open::new(chrome::quit_choice("t"), Purpose::Quit { client: "c1".into() }));
        server.check_notice();
        assert!(server.notice.is_none());
        server.notify(&b);
        assert!(server.notice.is_none(), "none under a layer");
        server.choice = None;
        server.check_notice();
        assert!(server.notice.is_none(), "not shown once the layer goes");
        server.notify(&b);
        assert!(server.notice.is_some());
        // Nor without a client.
        let client = server.client.take();
        server.notice = None;
        server.notify(&b);
        assert!(server.notice.is_none(), "no one to tell");
        server.client = client;
        server.notify(&b);
        // ⌥g: to it, and the notice goes once it is in sight.
        server.go_waiting();
        assert_eq!((server.active, server.focus.as_deref()), (1, Some(b.as_str())));
        server.check_notice();
        assert!(server.notice.is_none());
    }

    fn status(state: State, message: Option<&str>) -> engine::Status {
        engine::Status {
            state,
            kind: None,
            progress: None,
            title: None,
            message: message.map(String::from),
            id: None,
            app: None,
        }
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
        let Reply::Ok(list) = server.answer(Request::Panes) else { panic!("panes") };
        let list: Vec<PaneInfo> = serde_json::from_value(list).unwrap();
        let info = list.iter().find(|info| info.member == "dev-b").unwrap();
        assert_eq!(
            (info.status.as_deref(), info.error.as_deref()),
            (Some(look::State::Error.name()), Some(overloaded))
        );
        // Its next turn: gone, everywhere.
        note_status(&mut server.panes[1], &status(State::Working, None));
        assert_eq!(server.panes[1].error, None);
        assert_eq!(server.header(&server.panes[1], None, None, now()).error, None);
        // Any other state too, `Clear` included.
        note_status(&mut server.panes[1], &status(State::Error, None));
        assert_eq!(
            server.panes[1].error.as_ref().map(|error| error.label.as_str()),
            Some(server.unknown_error.as_str())
        );
        note_status(&mut server.panes[1], &status(State::Clear, None));
        assert_eq!(server.panes[1].error, None);
        // Failed as the dashboard saw it (no state of its own): its reason unknown, said short.
        server
            .looks
            .insert("dev-b".into(), board::MemberLook { state: Some(look::State::Error), ..Default::default() });
        let header = server.header(&server.panes[1], None, None, now());
        assert_eq!((header.state, header.error), (look::State::Error, Some(server.unknown_error.as_str())));
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
        let (notice, until) = server.notice.clone().expect("noticed");
        assert_eq!((notice.state, notice.member.as_str(), notice.tab.as_str()), (look::State::Error, "dev-b", "2"));
        assert_eq!(notice.text, chrome::error_notice_text(&["dev-b"], &label));
        // Another within ten seconds: the same notice, both told, its time kept.
        fail(&mut server, 2);
        server.call(vec![(c.clone(), look::State::Error)]);
        let (notice, kept) = server.notice.clone().expect("still");
        assert_eq!(notice.text, chrome::error_notice_text(&["dev-b", "dev-c"], &label));
        assert_eq!((notice.member.as_str(), kept), ("dev-c", until));
        // Its time up within the ten seconds: none other.
        server.notice = None;
        fail(&mut server, 3);
        server.call(vec![(d.clone(), look::State::Error)]);
        assert!(server.notice.is_none(), "one for the burst");
        // Ten seconds on: a new one.
        server.burst.as_mut().unwrap().0 -= BURST;
        server.call(vec![(d.clone(), look::State::Error)]);
        assert_eq!(server.notice.as_ref().map(|(notice, _)| notice.member.as_str()), Some("dev-d"));
        // A wait wins over it, and keeps its place.
        server.panes[1].state = Some(State::Blocked);
        server.call(vec![(c.clone(), look::State::Error), (b.clone(), look::State::Waiting)]);
        let (notice, _) = server.notice.clone().expect("waiting");
        assert_eq!((notice.state, notice.member.as_str()), (look::State::Waiting, "dev-b"));
        server.burst = None;
        server.call(vec![(c.clone(), look::State::Error)]);
        assert_eq!(server.notice.as_ref().map(|(notice, _)| notice.state), Some(look::State::Waiting));
        // None under a layer, nor without a client.
        server.notice = None;
        server.choice = Some(Open::new(chrome::quit_choice("t"), Purpose::Quit { client: "c1".into() }));
        server.call(vec![(c.clone(), look::State::Error)]);
        assert!(server.notice.is_none(), "under a layer");
        server.choice = None;
        let client = server.client.take();
        server.call(vec![(c.clone(), look::State::Error)]);
        assert!(server.notice.is_none(), "no one to tell");
        server.client = client;
        // Gone once none of its members is failed out of sight any more.
        server.burst = None;
        server.call(vec![(c.clone(), look::State::Error)]);
        fail(&mut server, 3);
        server.call(vec![(d.clone(), look::State::Error)]);
        note_status(&mut server.panes[3], &status(State::Working, None));
        server.check_notice();
        assert!(server.notice.is_some(), "dev-c still failed");
        server.focus_on(&c);
        server.check_notice();
        assert!(server.notice.is_none(), "dev-c in sight");
    }

    #[test]
    fn a_member_failed_by_the_dashboard_is_noticed_too() {
        let (_state, mut server, _client) = four_tabs();
        // Its program says nothing; the dashboard saw its turn fail.
        let failed = board::MemberLook { state: Some(look::State::Error), ..Default::default() };
        server.message(Msg::Looks(BTreeMap::from([("dev-c".into(), failed.clone())])));
        let (notice, _) = server.notice.clone().expect("noticed");
        assert_eq!((notice.state, notice.member.as_str()), (look::State::Error, "dev-c"));
        assert_eq!(notice.text, chrome::error_notice_text(&["dev-c"], &server.unknown_error));
        // The same again: no second one.
        server.notice = None;
        server.burst = None;
        server.message(Msg::Looks(BTreeMap::from([("dev-c".into(), failed)])));
        assert!(server.notice.is_none());
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
        let seen =
            |state| board::MemberLook { state: Some(state), since: Some(now as i64 - 300), ..Default::default() };
        let looks = BTreeMap::from([
            ("dev-a".into(), seen(look::State::Working)),
            ("dev-b".into(), seen(look::State::Waiting)),
        ]);
        server.message(Msg::Looks(looks));
        // Out of sight, waiting by the dashboard: its notice, its tab's ⚑.
        assert_eq!(server.notice.as_ref().map(|(notice, _)| notice.member.as_str()), Some("dev-b"));
        let header = server.header(&server.panes[1], None, None, now);
        assert_eq!((header.state, header.since), (look::State::Waiting, Some(300)));
        // dev-a's program says it rests: it, not the dashboard.
        assert_eq!(server.header(&server.panes[0], None, None, now).state, look::State::Idle);
        assert_eq!(server.counts(), chrome::Hint::Counts { working: 0, waiting: 1, error: 0, idle: 1 });
        let tab = |server: &Server, i: usize| {
            urgent(
                server.tabs[i]
                    .ids()
                    .iter()
                    .filter_map(|id| server.pane(id))
                    .map(|pane| pane_state(pane, &server.looks)),
            )
        };
        assert_eq!(tab(&server, 1), look::State::Waiting);
        // `Panes` gives its program's own: none. The dashboard reads it, not its own echo.
        let Reply::Ok(list) = server.answer(Request::Panes) else { panic!("panes") };
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
        assert!(
            server
                .panes
                .iter()
                .all(|pane| pane.role.is_empty() || pane_state(pane, &server.looks) == look::State::Other)
        );
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
        assert_eq!(server.hover, Some(Hover::Part(a.clone(), chrome::Part::Name)));
        assert!(server.dirty);
        server.dirty = false;
        press(&mut server, name, MouseKind::Moved);
        assert!(!server.dirty, "the same part: nothing to draw");
        // Its context: the compaction's confirmation.
        let context = part(&server, &a, chrome::Part::Context);
        press(&mut server, context, MouseKind::Down(Button::Left));
        assert_eq!(
            server.choice.as_ref().map(|open| &open.purpose),
            Some(&Purpose::Compact { member: "dev-a".into() })
        );
        server.choice = None;
        press(&mut server, context, MouseKind::Up(Button::Left));
        assert!(!server.swallowing, "its release swallowed");
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
        let at = server.notice_zone.expect("drawn");
        press(&mut server, (at.x as u16 + 1, at.y as u16), MouseKind::Down(Button::Left));
        assert_eq!((server.active, server.focus.as_deref()), (1, Some(c.as_str())));
        assert!(server.notice.is_none());
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
            server.mouse(&Mouse {
                kind: MouseKind::Up(Button::Left),
                col: col as u16,
                row: row as u16,
                mods: Mods::NONE,
            });
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
        assert!(server.choice.is_none() && server.notice.is_none());

        // The menu tells the size it needs, its frame included: the pane takes exactly that.
        let mut replies = Vec::new();
        server
            .panes
            .iter()
            .find(|pane| pane.id == menu)
            .unwrap()
            .with(|engine| engine.feed(b"\x1b[8;5;30t", &mut replies));
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
                names.iter().map(|name| crate::state::MemberInfo { name: name.to_string(), ..Default::default() });
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
        // Renamed: in place, before team.json says it (`live.rs`).
        let id = server.panes[0].id.clone();
        assert!(matches!(server.request(Request::SetMember { id, member: "dev-z".into() }), Reply::Ok(_)));
        assert_eq!(server.order, ["chef", "dev-z", "dev-b"]);
        // Added, then removed: as team.json says once written.
        write(&["chef", "dev-z", "dev-b", "dev-c"]);
        assert!(matches!(server.request(Request::OpenWindow { pane: sleeper("dev-c") }), Reply::Ok(_)));
        assert_eq!(server.order, ["chef", "dev-z", "dev-b", "dev-c"]);
        write(&["chef", "dev-z", "dev-c"]);
        let arrange = Request::Arrange {
            tabs: vec![("1".into(), vec!["dev-z".into(), "dev-c".into()])],
            columns: 3,
            focus: false,
        };
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
}
