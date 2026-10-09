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
use std::collections::VecDeque;
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
use self::pane::{Broken, Command, Gate, Pane, Wires};
use super::chrome::{self, Target};
use super::engine::{MouseTracking, Relay, State};
use super::input::{self, Button, Event, Mods, Mouse, MouseKind};
use super::keys::{self, Action, Shortcut};
use super::proto::{
    self, Bye, Captured, ClientInfo, ClientMsg, Hello, Kind, MenuOpened, PaneInfo, PaneModes, PaneSpec, PaneStats,
    Refusal, Reply, Request, ServerMsg, Stats, TabSpec, Welcome,
};
use super::screen::{self, View};
use super::select::{self, Selection};
use super::socket::{self, Info};
use super::{Caps, Rect};
use crate::canvas::{Canvas, Painter};
use crate::layout::{self, SidePanes};
use crate::look::{self, Glyphs};
use crate::state::Snapshot;
use crate::{board, t};

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
            Relay::Status(_) => {}
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
}

/// The panels' column: the dashboard over the journal, by pane ids.
#[derive(Clone, Default)]
struct Side {
    dashboard: Option<String>,
    journal: Option<String>,
}

impl Tab {
    fn new(title: String, panes: Vec<String>) -> Tab {
        Tab { title, panes, side: None, last: None }
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
/// one starts on (as `tmux.rs`'s `REDUCED_ROWS`).
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
    socket_id: (u64, u64),
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
    /// A choice open over the team (⌥q, a compaction's confirmation): it takes the keys and the clicks.
    choice: Option<Open>,
    /// Quit was picked: the team stops at the end of this turn.
    quitting: bool,
    /// Frames sent to clients, and bytes written to them, since the server started: `_ctl stats`.
    frames: u64,
    /// The members in the team's order, for their colors (`member_color`).
    order: Vec<String>,
    written: u64,
}

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
    let server = Server::new(state, session, (path, socket_id), tx, welcome);
    Ok((server, rx, lock))
}

impl Server {
    /// A server for the team whose state is in `state`, before any pane: its socket (path, and what tells the file
    /// apart), where its threads write to it, and what it welcomes connections with.
    fn new(
        state: &Path,
        session: &str,
        socket: (PathBuf, (u64, u64)),
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
            choice: None,
            quitting: false,
            frames: 0,
            order: Vec::new(),
            written: 0,
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

/// A line in `server.log`.
fn log(text: &str) {
    eprintln!("{} recruit[{}]: {text}", now(), std::process::id());
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
            let first = match [frame_due, self.deadline, self.animate].into_iter().flatten().min() {
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
            if self.animate.is_some_and(|at| at <= Instant::now()) {
                self.animate = None;
                self.dirty = true;
            }
            self.expire();
            self.repair();
            self.frame();
        }
    }

    /// A message that is not a command nor a signal.
    fn message(&mut self, msg: Msg) {
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
                self.panes[i].exited = true;
                self.close_pane(&id);
            }
            Msg::Broken => self.dirty = true,
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
        for pane in &self.panes {
            pane.with(|engine| engine.set_colors(caps.fg, caps.bg));
        }
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

    /// Lets the client go: told why, when it is still there to hear it.
    /// Returns its outbox, to wait for its writer to leave.
    fn drop_client(&mut self, bye: Option<Bye>) -> Option<Arc<Outbox>> {
        // The menu, and a choice, were opened for this client: they would act for one that is gone.
        if let Some((menu, _)) = self.menu.clone() {
            self.close_pane(&menu);
        }
        self.choice = None;
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
                let rows = screen::content(pane.area).height.max(1);
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
                if let Some(id) = next_member(&tab.panes, self.focus.as_deref()) {
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

    /// Gives the focus to pane `id`, showing its tab; the panes that asked are told (1004).
    fn focus_on(&mut self, id: &str) {
        if let Some(tab) = self.tabs.iter().position(|tab| tab.has(id)) {
            self.active = tab;
            self.tabs[tab].last = Some(id.to_string());
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
        if let MouseKind::Down(_) = mouse.kind
            && let Some(target) = self.zones.iter().find(|(zone, _)| zone.contains(col, row)).map(|(_, t)| *t)
        {
            self.unselect();
            match target {
                Target::Tab(tab) => self.show_tab(tab),
                Target::Menu => self.menu_here(),
                Target::Quit if self.menu.is_some() => {}
                Target::Quit => self.ask_quit(),
            }
            self.dirty = true;
            return;
        }
        let mut shown = self.tabs.get(self.active).map(Tab::ids).unwrap_or_default();
        // The menu over the others: first under the pointer, and the only one while it is open.
        if let Some((menu, _)) = &self.menu {
            shown = vec![menu.clone()];
        }
        let under = self
            .panes
            .iter()
            .filter(|pane| shown.contains(&pane.id))
            .find(|pane| screen::content(pane.area).contains(col, row))
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
        let content = screen::content(area);
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
        self.size = size;
        if let Some(client) = self.client.as_mut() {
            client.size = size;
        }
        self.layout();
    }

    /// Every tab laid out on the screen, less the bar: a shown tab's panes get their place, the others their size.
    fn layout(&mut self) {
        if let Some((menu, _)) = self.menu.clone() {
            let area = self.menu_area();
            if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == menu) {
                pane.place(area, screen::content(area));
            }
        }
        let (width, height) = (self.size.0 as usize, (self.size.1 as usize).saturating_sub(1));
        for (t, tab) in self.tabs.iter().enumerate() {
            let side = tab.side.as_ref().map(|side| SidePanes {
                dashboard: side.dashboard.clone(),
                journal: side.journal.clone().map(|id| (id, self.reduced.then_some(JOURNAL_ROWS))),
            });
            for (id, area) in layout::rects(width, height, &tab.panes, self.columns, side.as_ref()) {
                if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) {
                    let shown = if t == self.active { area } else { Rect::default() };
                    pane.place(area, screen::content(area));
                    pane.area = shown;
                }
            }
        }
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
    /// later would surprise); the states noted.
    fn take_relays(&mut self) {
        let focus = self.focus.clone();
        let mut relays = Vec::new();
        let held = self.gate.hold();
        for pane in &mut self.panes {
            relays.clear();
            let gate_free = pane.with(|engine| engine.relays(&mut relays));
            if gate_free.is_none() {
                continue;
            }
            for relay in relays.drain(..) {
                if let Relay::Status(status) = &relay {
                    pane.state = Some(status.state);
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
        // Every move: for the focused pane that asks, and for a choice, whose selection follows the pointer.
        let motion = self.choice.is_some()
            || self.focused().is_some_and(|pane| pane.modes().mouse == Some(MouseTracking::Motion));
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
        self.frames += 1;
        self.written += bytes.len() as u64;
        client.out.push(Out::Output(bytes));
        client.busy = true;
        self.last_frame = Some(started);
        self.dirty = false;
        self.echo = false;
    }

    /// The screen as it is, in `self.screen`: the shown tab's panes in their frames, the bar. False while an engine
    /// is broken.
    fn compose(&mut self) -> bool {
        let (width, height) = (self.size.0 as usize, self.size.1 as usize);
        let held = self.gate.hold();
        let mut shown: Vec<&Pane> = match self.tabs.get(self.active) {
            Some(tab) => tab.ids().into_iter().filter_map(|id| self.pane(&id)).collect(),
            None => Vec::new(),
        };
        // The menu last: drawn over the others.
        shown.extend(self.menu.as_ref().and_then(|(menu, _)| self.pane(menu)));
        let engines: Vec<_> =
            shown.iter().map(|pane| pane.engine.lock().unwrap_or_else(PoisonError::into_inner)).collect();
        if shown.iter().any(|pane| pane.broken.is()) {
            return false;
        }
        let notes: Vec<Option<String>> = shown.iter().map(|pane| note(pane)).collect();
        let views: Vec<View<'_>> = shown
            .iter()
            .zip(&engines)
            .zip(&notes)
            .map(|((pane, engine), note)| View {
                id: &pane.id,
                engine: engine.as_ref(),
                area: pane.area,
                header: chrome::Header {
                    name: &pane.member,
                    // A member's color, as on the dashboard; none for the dashboard, the journal, the menu.
                    color: pane.role.is_empty().then(|| member_color(&self.order, &pane.member)),
                    state: look_state(pane.state),
                    note: note.as_deref(),
                    scroll: None,
                    hint: (pane.role == JOURNAL).then_some(chrome::Hint::JournalSize),
                },
                focused: self.focus.as_deref() == Some(pane.id.as_str()),
                selection: self
                    .selection
                    .as_ref()
                    .filter(|(selection, _)| selection.pane == pane.id)
                    .and_then(|(selection, _)| selection.range()),
            })
            .collect();
        let tabs: Vec<chrome::Tab<'_>> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let states = tab.panes.iter().filter_map(|id| self.pane(id)).map(|pane| look_state(pane.state));
                chrome::Tab { title: &tab.title, active: i == self.active, state: urgent(states) }
            })
            .collect();
        // The spinner turns while a member at work shows, in a header or a tab.
        let working = views.iter().map(|view| view.header.state).chain(tabs.iter().map(|tab| tab.state));
        let working = working.into_iter().any(|state| state == look::State::Working);
        let glyphs = self.client.as_ref().map_or(Glyphs::Unicode, glyphs);
        let bar = Rect { x: 0, y: height.saturating_sub(1), width, height: usize::from(height > 0) };
        let look = chrome::Look { glyphs, frame: look::frame(self.born.elapsed()) };
        let team = self.team.as_str();
        let screen = &self.screen;
        // Over the team, the rest dimmed: a choice, else the menu (the last view).
        let over = match (&self.choice, &self.menu) {
            (Some(open), _) => screen::Over::Choice(&open.choice),
            (None, Some(_)) => screen::Over::LastView,
            (None, None) => screen::Over::Nothing,
        };
        let composed =
            super::caught(|| screen.borrow_mut().compose(width, height, &views, team, &tabs, bar, look, over));
        // The zones, or whether an engine is to blame.
        let zones = match composed {
            Ok(zones) => Ok(zones),
            Err(why) => {
                // Which engine: each one asked alone what compose asks of it.
                let mut found = false;
                for (pane, view) in shown.iter().zip(&views) {
                    let mut alone = Canvas::new(width, height);
                    let replay = super::caught(|| {
                        view.engine.draw(&mut alone, screen::content(view.area));
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
        if let Some(open) = self.choice.as_mut() {
            open.zones = zones.options;
        }
        // Under a layer (the menu, a choice), the spinner shows dimmed: not worth a frame each `look::FRAME`.
        let covered = self.choice.is_some() || self.menu.is_some();
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

    /// A pane by its id, else by its member's name.
    fn find(&self, which: &str) -> Option<&Pane> {
        self.pane(which).or_else(|| self.panes.iter().find(|pane| pane.member == which))
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
            Request::Panes => {
                let list: Vec<PaneInfo> = self
                    .tabs
                    .iter()
                    .flat_map(|tab| tab.ids().into_iter().map(move |id| (tab, id)))
                    .filter_map(|(tab, id)| self.pane(&id).map(|pane| (tab, pane)))
                    .map(|(tab, pane)| PaneInfo {
                        id: pane.id.clone(),
                        member: pane.member.clone(),
                        role: pane.role.clone(),
                        tab: tab.title.clone(),
                        pid: pane.pty.pid(),
                        cols: pane.size.0,
                        rows: pane.size.1,
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
            Request::Build { team, dir, columns, tabs, cols, rows, exe, lang } => {
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
            Request::Arrange { tabs, columns } => {
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
            Request::OpenMenu { member: _, client } => answer(self.open_menu(client.as_deref())),
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
        // Each tab keeps the pane it last had the focus on, wherever that pane went.
        let lasts: Vec<String> = self.tabs.iter().filter_map(|tab| tab.last.clone()).collect();
        for tab in &mut arranged {
            tab.last = lasts.iter().find(|last| tab.has(last)).cloned();
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
    fn open_menu(&mut self, client: Option<&str>) -> Result<MenuOpened> {
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
        let cwd = self.welcome.lock().unwrap_or_else(PoisonError::into_inner).dir.clone();
        let cwd = if cwd.is_empty() { "/".to_string() } else { cwd };
        let spec = PaneSpec { member: "recruit".into(), role: MENU.into(), argv, env: Vec::new(), cwd };
        let id = self.spawn(spec)?;
        let before = self.focus.clone();
        self.menu = Some((id.clone(), before));
        self.focus_on(&id);
        Ok(MenuOpened::Opened)
    }

    /// The menu's place: 100 × 28 inside its frame, as the mock-ups draw it, centered; less in a smaller screen.
    fn menu_area(&self) -> Rect {
        let (width, height) = (self.size.0 as usize, (self.size.1 as usize).saturating_sub(1));
        let fit = |wanted: usize, room: usize| wanted.min(room.saturating_sub(2)).max(1).min(room);
        let (w, h) = (fit(102, width), fit(30, height));
        Rect { x: (width - w) / 2, y: (height - h) / 2, width: w, height: h }
    }

    /// The menu, on the client.
    fn menu_here(&mut self) {
        if let Err(error) = self.open_menu(None) {
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
        Some(State::Clear) | None => look::State::Other,
    }
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

/// A member's own color, by its place in the team's order, as `board` gives it; one the order does not know yet
/// (a pane opened by `_ctl`) comes after the others.
fn member_color(order: &[String], member: &str) -> crossterm::style::Color {
    look::member_color(order.iter().position(|name| name == member).unwrap_or(order.len()))
}

/// The most urgent of states: waiting, else working, else at rest.
fn urgent(states: impl Iterator<Item = look::State>) -> look::State {
    let states: Vec<look::State> = states.collect();
    [look::State::Waiting, look::State::Working, look::State::Idle]
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
        let socket = (state.path().join("s.sock"), (0, 0));
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
        let arrange = Request::Arrange { tabs: vec![("1".into(), vec!["dev-z".into(), "dev-c".into()])], columns: 3 };
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
