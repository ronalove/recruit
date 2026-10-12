// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! From `recruit _server` to the server's end: detaching, the lock, the log, the signals, and the team's stop.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use super::clients::accept;
use super::status::watch_looks;
use super::{Asker, BYE_TIME, Msg, Server};
use crate::mux::proto::{self, Bye, Reply, Welcome};
use crate::mux::socket::{self, Info};
use crate::t;

/// A line in `server.log`: `log!("pane {id} ended")`.
macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("{} recruit[{}]: {}", $crate::mux::server::lifecycle::now(), std::process::id(), format_args!($($arg)*))
    };
}
pub(super) use log;

/// How long the panes have to end when the team stops, before the server goes all the same: longer than `pty.rs`'s
/// grace, after which it kills them, so that its SIGKILL is sent before the server leaves.
const STOP_TIME: Duration = Duration::from_secs(4);

/// How long the server waits, when it stops, for the answer to `Stop` to be written.
const ANSWER_TIME: Duration = Duration::from_secs(1);

/// The exit code after a panic outside a pane's engine.
const PANICKED: i32 = 70;

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
pub(super) fn threads() -> Option<usize> {
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

/// Closes the descriptors from 3 up inherited from the launcher: the server keeps none of its terminal's, its
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
pub(super) fn start(state: &Path, session: &str, told: io::PipeWriter) -> i32 {
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
    log!("ready, pid {}, socket {}", std::process::id(), server.socket.display());
    match server.run(rx) {
        Ok(()) => 0,
        Err(error) => {
            log!("{error:#}");
            1
        }
    }
}

pub(super) type Ready = (Server, Receiver<Msg>, File);

/// Everything before the loop.
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
        log!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
        // An engine's panic is caught where it happens, and that pane only is repaired.
        if !crate::mux::catching() {
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
    watch_looks(state.to_path_buf(), tx, Arc::clone(&server.auto.watched))?;
    Ok((server, rx, lock))
}

/// The state folder's lock, waited for a little: a server that stops lets it go as it leaves.
pub(super) fn lock(state: &Path) -> Result<File> {
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

/// Seconds since the epoch, as the server counts them.
pub(super) fn now() -> u64 {
    crate::sys::now() as u64
}

/// The signals the server takes: blocked in every thread, waited for by one.
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

impl Server {
    /// The socket again, gone from its folder (SIGURG): a new listener and its thread. The old thread stays
    /// in `accept` on the old one (macOS does not wake it).
    pub(super) fn reopen_socket(&mut self) {
        if fs::symlink_metadata(&self.socket).is_ok() {
            return;
        }
        match socket::listen(&self.socket) {
            Ok((listener, id)) => {
                self.socket_id = id;
                if let Err(error) = accept(listener, self.tx.clone(), Arc::clone(&self.welcome)) {
                    log!("accept: {error}");
                }
                log!("socket made again");
            }
            Err(error) => log!("socket: {error:#}"),
        }
    }

    /// The team stops: the client told, every program asked to end and waited for, the socket and
    /// `server.json` removed, then the answer to whoever asked, written before the server goes.
    pub(super) fn stop(mut self, rx: &Receiver<Msg>, bye: Bye, asker: Option<Asker>) {
        log!("stopping: {bye:?}");
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
        log!("stopped");
    }
}
