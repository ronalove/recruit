// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A pane of the server: a program on its PTY, the engine it writes to, and what ties them to the loop.
//!
//! The PTY's reader feeds the engine slice by slice, behind its mutex, and tells the loop once per frame at most;
//! the loop holds the `Gate` while it asks the engines anything, so that a flood never keeps it waiting. A panic of
//! an engine marks it broken: the loop gives the pane a new one, and gives the pane up after three in 10 s.
//!
//! Owner: dev-serveur.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::Msg;
use crate::mux::engine::{self, Engine, State};
use crate::mux::pty::{Pty, PtyInput, Spawn};
use crate::mux::{Caps, Rect, caught};

/// Lines of history kept by each pane (spec §5.5).
pub(super) const HISTORY: usize = 100_000;

/// What a reader feeds its engine at once: the screen waits for one slice at most.
const SLICE: usize = 4 * 1024;

/// Engine failures this close together give a pane up: its program would only make it fail again.
const FAILURES: usize = 3;
const FAILURES_WITHIN: Duration = Duration::from_secs(10);

/// What the real terminal, or a multiplexer, puts in the environment to say what it is: a pane's program must not
/// believe it runs there (spec §5.2). `TERM`, `COLORTERM` and `TERM_PROGRAM` are set again. And `RECRUIT_BACKEND`,
/// which chose the multiplexer before 2.0: nothing reads it any more, and a value left in the user's environment goes
/// no further than the server.
const SCRUB: &[&str] = &[
    "RECRUIT_BACKEND",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "ITERM_SESSION_ID",
    "ITERM_PROFILE",
    "KITTY_WINDOW_ID",
    "KITTY_PID",
    "KITTY_PUBLIC_KEY",
    "KITTY_LISTEN_ON",
    "WEZTERM_PANE",
    "WEZTERM_UNIX_SOCKET",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_EXECUTABLE_DIR",
    "WEZTERM_CONFIG_DIR",
    "WEZTERM_CONFIG_FILE",
    "GHOSTTY_RESOURCES_DIR",
    "GHOSTTY_BIN_DIR",
    "GHOSTTY_SHELL_FEATURES",
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
    "ALACRITTY_LOG",
    "VTE_VERSION",
    "WT_SESSION",
    "WINDOWID",
    "TERMINAL_EMULATOR",
    "COLUMNS",
    "LINES",
    // The marks of a Claude Code session recruit may have been started from: inherited, a member's Claude takes
    // itself for that session's child, and saves no transcript (2.1.295).
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
];

/// What the panes' programs are told of their terminal (spec §11, step 0's findings in specs/multiplexeur-prototype.md).
const TERM: &str = "xterm-256color";

/// What a pane runs, and where.
#[derive(Clone, Debug, Default)]
pub(super) struct Command {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// Set in the environment, after the removals.
    pub env: Vec<(String, String)>,
}

fn lock(engine: &Mutex<Box<dyn Engine>>) -> MutexGuard<'_, Box<dyn Engine>> {
    engine.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An engine that panicked: its state can no longer be trusted, and nothing calls it until it is replaced.
#[derive(Default)]
pub(super) struct Broken {
    set: AtomicBool,
    why: Mutex<String>,
    /// Given up: what the program writes is no longer read into an engine.
    dead: AtomicBool,
}

impl Broken {
    pub(super) fn is(&self) -> bool {
        self.set.load(Ordering::Acquire)
    }

    pub(super) fn mark(&self, why: String) {
        *self.why.lock().unwrap_or_else(PoisonError::into_inner) = why;
        self.set.store(true, Ordering::Release);
    }
}

/// Runs `f` on `engine` unless it is broken; a panic in it marks it broken. The lock is taken outside the panic: the
/// mutex is never poisoned.
fn guarded<T>(engine: &Mutex<Box<dyn Engine>>, broken: &Broken, f: impl FnOnce(&mut dyn Engine) -> T) -> Option<T> {
    let mut engine = lock(engine);
    if broken.is() {
        return None;
    }
    caught(|| f(engine.as_mut())).map_err(|why| broken.mark(why)).ok()
}

/// The loop's turn at the engines. A reader passes the gate before each slice it feeds; while the loop holds it, the
/// readers finish the slice they are on and wait. Without it, a reader of a flood takes its engine's lock again as
/// soon as it lets it go (the lock is not fair), and the loop waits tens of milliseconds for it.
#[derive(Default)]
pub(super) struct Gate {
    wanted: AtomicUsize,
    lock: Mutex<()>,
    open: Condvar,
}

impl Gate {
    pub(super) fn hold(&self) -> Held<'_> {
        self.wanted.fetch_add(1, Ordering::SeqCst);
        Held(self)
    }

    /// Waits while the loop holds the gate.
    fn pass(&self) {
        if self.wanted.load(Ordering::SeqCst) == 0 {
            return;
        }
        let mut guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        while self.wanted.load(Ordering::SeqCst) > 0 {
            guard = self.open.wait(guard).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

pub(super) struct Held<'a>(&'a Gate);

impl Drop for Held<'_> {
    fn drop(&mut self) {
        if self.0.wanted.fetch_sub(1, Ordering::SeqCst) == 1 {
            // Under the lock: a reader between its look at `wanted` and its wait is not missed.
            let _guard = self.0.lock.lock().unwrap_or_else(PoisonError::into_inner);
            self.0.open.notify_all();
        }
    }
}

/// A pane of the team.
pub(super) struct Pane {
    /// `p1`, `p2`…: never given twice in the server's life.
    pub id: String,
    /// The member's name, or the panel's title.
    pub member: String,
    /// A panel's kind (`dashboard`, `journal`); empty for a member.
    pub role: String,
    pub command: Command,
    pub engine: Arc<Mutex<Box<dyn Engine>>>,
    /// Set by the reader when it says the engine has news, cleared just before a frame: one message per pane and per
    /// frame at most, and none lost.
    pub told: Arc<AtomicBool>,
    pub broken: Arc<Broken>,
    pub pty: Pty,
    input: PtyInput,
    /// Where it is on the screen, frame included; empty when its tab is not shown.
    pub area: Rect,
    /// The size of its cells, which its engine and its program have.
    pub size: (u16, u16),
    pub exited: bool,
    /// The last time its engine failed, and why.
    pub failure: Option<String>,
    failures: Vec<Instant>,
    gate: Arc<Gate>,
    /// What its program last said of its state (OSC 7501).
    pub state: Option<State>,
    /// Since when it is in that state, as its header shows it (waiting, working, at rest), in seconds since the
    /// epoch; `None` until its program says it.
    pub since: Option<u64>,
    /// Bytes its programs wrote since it opened, for `_ctl stats`.
    pub read: Arc<AtomicU64>,
}

/// What a pane's threads share with the loop.
pub(super) struct Wires {
    pub tx: Sender<Msg>,
    pub gate: Arc<Gate>,
}

impl Pane {
    /// Starts `command` in a new pane `id`, `size` cells, its engine told the real terminal's colors.
    pub(super) fn open(
        id: String,
        member: String,
        role: String,
        command: Command,
        size: (u16, u16),
        caps: &Caps,
        wires: &Wires,
    ) -> Result<Pane> {
        let (cols, rows) = size;
        let engine = new_engine(size, caps)?;
        let engine = Arc::new(Mutex::new(engine));
        let (told, broken) = (Arc::new(AtomicBool::new(false)), Arc::new(Broken::default()));
        let read = Arc::new(AtomicU64::new(0));
        let pty = start(&id, &command, size, (&engine, &told, &broken, &read), wires)?;
        let input = pty.input();
        let _ = (cols, rows);
        Ok(Pane {
            id,
            member,
            role,
            command,
            engine,
            told,
            broken,
            pty,
            input,
            area: Rect::default(),
            size,
            exited: false,
            failure: None,
            failures: Vec::new(),
            gate: Arc::clone(&wires.gate),
            state: None,
            since: None,
            read,
        })
    }

    /// Starts its program again, in place, with `command`: a new engine, blank. The old program is asked to end.
    pub(super) fn respawn(&mut self, command: Command, caps: &Caps, wires: &Wires) -> Result<()> {
        let engine = Arc::new(Mutex::new(new_engine(self.size, caps)?));
        let (told, broken) = (Arc::new(AtomicBool::new(false)), Arc::new(Broken::default()));
        let pty = start(&self.id, &command, self.size, (&engine, &told, &broken, &self.read), wires)?;
        // The old one, dropped, is stopped with its group; its `Exited` names a pid no longer this pane's.
        self.input = pty.input();
        self.pty = pty;
        self.engine = engine;
        self.told = told;
        self.broken = broken;
        self.command = command;
        self.exited = false;
        self.failure = None;
        self.failures.clear();
        self.state = None;
        self.since = None;
        Ok(())
    }

    /// Runs `f` on its engine, the readers held at the gate; `None` if the engine is broken.
    pub(super) fn with<T>(&self, f: impl FnOnce(&mut dyn Engine) -> T) -> Option<T> {
        let _held = self.gate.hold();
        guarded(&self.engine, &self.broken, f)
    }

    pub(super) fn modes(&self) -> engine::Modes {
        self.with(|engine| engine.modes()).unwrap_or_default()
    }

    pub(super) fn send(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() && !self.exited {
            self.input.send(bytes);
        }
    }

    /// Gives the pane its place, `area` its frame and `inside` its cells: its engine and its program take their
    /// size. An empty `inside` (a tab not shown) keeps the size it had.
    pub(super) fn place(&mut self, area: Rect, inside: Rect) {
        self.area = area;
        if inside.width == 0 || inside.height == 0 {
            return;
        }
        let size = (clamp(inside.width), clamp(inside.height));
        if size == self.size {
            return;
        }
        self.size = size;
        self.with(|engine| engine.resize(size.0, size.1));
        // A program that has just ended has no size to take.
        let _ = self.pty.resize(size.0, size.1);
    }

    /// Back to the live screen, when the pane shows its history.
    pub(super) fn unscroll(&self) -> bool {
        self.with(|engine| {
            let scrolled = engine.scrolled() > 0;
            if scrolled {
                engine.scroll(isize::MIN);
            }
            scrolled
        })
        .unwrap_or(false)
    }

    /// A new engine, blank, in place of a broken one; the program is told its size again, so that it draws its screen
    /// anew. Broken too often, the pane is given up: its program is stopped.
    pub(super) fn repair(&mut self, caps: &Caps) -> Result<()> {
        let fresh = new_engine(self.size, caps)?;
        let now = Instant::now();
        self.failures.retain(|at| now.duration_since(*at) < FAILURES_WITHIN);
        self.failures.push(now);
        let dead = self.failures.len() >= FAILURES;
        let mut engine = lock(&self.engine);
        *engine = fresh;
        let why = std::mem::take(&mut *self.broken.why.lock().unwrap_or_else(PoisonError::into_inner));
        eprintln!("recruit: pane {} ({}): engine failed: {why}", self.id, self.member);
        self.failure = Some(why);
        if dead {
            self.broken.dead.store(true, Ordering::Release);
        }
        self.broken.set.store(false, Ordering::Release);
        drop(engine);
        if dead {
            eprintln!("recruit: pane {} ({}): given up", self.id, self.member);
            self.pty.kill();
            return Ok(());
        }
        let (cols, rows) = self.size;
        // The same size twice sends no SIGWINCH: one row less (or more), then back.
        let _ = self.pty.resize(cols, if rows > 1 { rows - 1 } else { rows + 1 });
        let _ = self.pty.resize(cols, rows);
        Ok(())
    }
}

fn clamp(cells: usize) -> u16 {
    cells.clamp(1, u16::MAX as usize) as u16
}

fn new_engine(size: (u16, u16), caps: &Caps) -> Result<Box<dyn Engine>> {
    let mut engine = engine::new(size.0, size.1, HISTORY)?;
    engine.set_colors(caps.fg, caps.bg);
    Ok(engine)
}

/// How `command` starts on a terminal `size` cells: its environment cleaned (`SCRUB`), then given what a pane's
/// program is told of its terminal. `None` without a program.
fn spawn_of(command: &Command, size: (u16, u16)) -> Option<Spawn> {
    let (program, args) = command.argv.split_first()?;
    let mut env: Vec<(OsString, OsString)> = vec![
        ("TERM".into(), TERM.into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "recruit".into()),
        ("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into()),
    ];
    env.extend(command.env.iter().map(|(key, value)| (key.into(), value.into())));
    Some(Spawn {
        program: program.into(),
        args: args.iter().map(OsString::from).collect(),
        cwd: command.cwd.clone(),
        env_remove: SCRUB.iter().map(OsString::from).collect(),
        env,
        cols: size.0,
        rows: size.1,
    })
}

/// What a PTY's reader shares with its pane: the engine it feeds, whether it said so, whether the engine broke, and
/// the bytes it read.
type Feed<'a> = (&'a Arc<Mutex<Box<dyn Engine>>>, &'a Arc<AtomicBool>, &'a Arc<Broken>, &'a Arc<AtomicU64>);

/// Starts `command` on a new PTY, its output fed to the engine.
fn start(id: &str, command: &Command, size: (u16, u16), feed: Feed<'_>, wires: &Wires) -> Result<Pty> {
    let (engine, told, broken, read) = feed;
    let Some(spawn) = spawn_of(command, size) else {
        anyhow::bail!("pane {id}: no command");
    };
    let program = spawn.program.to_string_lossy().into_owned();
    let output = {
        let (engine, told, broken) = (Arc::clone(engine), Arc::clone(told), Arc::clone(broken));
        let (tx, gate, id, read) = (wires.tx.clone(), Arc::clone(&wires.gate), id.to_string(), Arc::clone(read));
        move |bytes: &[u8], replies: &mut Vec<u8>| {
            read.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            for slice in bytes.chunks(SLICE) {
                // A panic in the engine stops this pane, not the server; what comes until it is replaced is lost.
                if broken.is() || broken.dead.load(Ordering::Acquire) {
                    return;
                }
                gate.pass();
                if guarded(&engine, &broken, |engine| engine.feed(slice, replies)).is_none() {
                    let _ = tx.send(Msg::Broken);
                    return;
                }
            }
            if !told.swap(true, Ordering::AcqRel) {
                let _ = tx.send(Msg::Output(id.clone()));
            }
        }
    };
    let exited = {
        let (tx, id, broken) = (wires.tx.clone(), id.to_string(), Arc::clone(broken));
        move |code: Option<i32>| {
            // `broken` tells this program apart from the one a respawn put in its place.
            let _ = tx.send(Msg::Exited(id, broken, code));
        }
    };
    Pty::spawn(&spawn, output, exited).with_context(|| program.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_does_not_inherit_the_teams_multiplexer() {
        let command = Command { argv: vec!["/bin/sh".into()], cwd: "/".into(), env: Vec::new() };
        let spawn = spawn_of(&command, (80, 24)).unwrap();
        let removed = |name: &str| spawn.env_remove.iter().any(|removed| removed == name);
        assert!(removed("RECRUIT_BACKEND"), "a value from before 2.0 goes no further");
        assert!(removed("TMUX") && removed("TMUX_PANE"));
        // What the members need to reach their team stays.
        assert!(!removed("RECRUIT_TMPDIR") && !removed("RECRUIT_STATE"));
        assert!(spawn_of(&Command::default(), (80, 24)).is_none());
    }
}
