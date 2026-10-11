// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A running team's panels, in a column on the right of its contacts: the dashboard (who works, who rests, who
//! waits for the user, with each one's activity over the last half hour) over the journal (the messages the
//! members send each other). Both read what Claude Code already records: `claude agents --json` and the
//! conversation files; the mod adds the context and the account's usage.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::style::{Color, Stylize};
use crossterm::{cursor, queue, terminal};
use serde::{Deserialize, Serialize};

use crate::backend::Pane;
use crate::bridge::{Helper, HelperStatus, Report, Task};
use crate::canvas::{Canvas, Features, Painter, PanicGuard, Style};
use crate::claude::{self, Running};
use crate::config::write_atomic;
use crate::i18n::Lang;
use crate::look::{self, FRAME, Glyphs, Pressure, State, duration, family, frame};
use crate::member;
use crate::state::Snapshot;
use crate::{backend, bridge, t};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    Dashboard,
    Journal,
    /// Hides the journal, or shows it again.
    Toggle,
}

pub fn run(kind: Kind, state: &Path) -> Result<()> {
    let snapshot = Snapshot::read(state)?;
    match kind {
        Kind::Dashboard => dashboard(&snapshot, state),
        Kind::Journal => journal(snapshot, state),
        Kind::Toggle => {
            println!("{}", toggle(&snapshot)?.said());
            Ok(())
        }
    }
}

pub fn dashboard_title(lang: Lang) -> &'static str {
    match lang {
        Lang::Fr => "Tableau de bord",
        Lang::En => "Dashboard",
    }
}

pub fn journal_title(_: Lang) -> &'static str {
    "Journal"
}

/// How often the panels look again: `claude agents --json`, the mod's reports, the activity samples.
const TICK: Duration = Duration::from_secs(1);

/// The key that asks for the whole board again, as everywhere in a terminal.
const CTRL_L: u8 = 0x0c;

/// Asks the multiplexer for the colors of the client's terminal, and for a notice each time they change (the color
/// scheme reports, `CSI ? 996 n` and `CSI ? 2031 h`); the answer and the notices are `CSI ? 997 ; 1 n` for a dark
/// background, `CSI ? 997 ; 2 n` for a light one.
const ASK_SCHEME: &str = "\x1b[?996n\x1b[?2031h";
const STOP_SCHEME: &str = "\x1b[?2031l";

/// The color scheme reports in `bytes`, as `true` for a light background; what is left of one cut in two stays in
/// `bytes` for the next read, and everything else is dropped.
fn scheme_reports(bytes: &mut Vec<u8>) -> Vec<bool> {
    const HEAD: &[u8] = b"\x1b[?997;";
    let mut found = Vec::new();
    while let Some(at) = bytes.windows(HEAD.len()).position(|w| w == HEAD) {
        let rest = &bytes[at + HEAD.len()..];
        match rest {
            [kind, b'n', ..] => {
                found.push(*kind == b'2');
                bytes.drain(..at + HEAD.len() + 2);
            }
            // Not whole yet, or not one of ours: the next read says.
            [_] | [] => {
                bytes.drain(..at);
                return found;
            }
            _ => {
                bytes.drain(..at + HEAD.len());
            }
        }
    }
    // The tail that may begin a report.
    let keep =
        (1..HEAD.len()).rev().find(|&n| bytes.len() >= n && HEAD.starts_with(&bytes[bytes.len() - n..])).unwrap_or(0);
    bytes.drain(..bytes.len() - keep);
    found
}

/// A panel's keyboard, kept quiet: what is typed in its pane is neither echoed nor gathered into lines, so that
/// nothing moves the screen (an Enter echoed on the last line would scroll every line up one, and the board, written
/// by difference, would stay shifted). What comes is read on the side, and dropped; Ctrl-L asks for the whole board
/// again (`redraw`). Signals stay as they were: Ctrl-C stops the panel. Nothing when the input is no terminal. The
/// terminal is given back as it was once the guard goes, a panic included (a panel run by hand in a terminal).
fn quiet_input(redraw: Arc<AtomicBool>) -> Option<QuietInput> {
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut term) } != 0 {
        return None;
    }
    let _ = ORIGINAL.set(term);
    term.c_lflag &= !(libc::ECHO | libc::ICANON);
    term.c_cc[libc::VMIN] = 1;
    term.c_cc[libc::VTIME] = 0;
    if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term) } != 0 {
        return None;
    }
    let guard = QuietInput { _panic: PanicGuard::install(leave) };
    // Stopped by a signal (Ctrl-C, the pane closed): given back too, then stopped as the signal would.
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
    }
    // The client's colors: dark until the answer comes, the first image not waiting for it.
    {
        let mut out = std::io::stdout();
        let _ = out.write_all(ASK_SCHEME.as_bytes());
        let _ = out.flush();
    }
    std::thread::spawn(move || {
        let mut bytes = [0u8; 64];
        let mut pending: Vec<u8> = Vec::new();
        let mut input = std::io::stdin();
        while let Ok(read) = input.read(&mut bytes) {
            if read == 0 {
                break;
            }
            if bytes[..read].contains(&CTRL_L) {
                redraw.store(true, Ordering::Relaxed);
            }
            pending.extend_from_slice(&bytes[..read]);
            for light in scheme_reports(&mut pending) {
                // Only when the colors changed: all the board again, in the others.
                if look::is_light() != light {
                    look::set_light(light);
                    redraw.store(true, Ordering::Relaxed);
                }
            }
        }
    });
    Some(guard)
}

/// The terminal's settings before [`quiet_input`].
static ORIGINAL: OnceLock<libc::termios> = OnceLock::new();

/// The terminal's settings as they were before [`quiet_input`].
fn loud_input() {
    if let Some(term) = ORIGINAL.get() {
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, term) };
    }
}

/// The terminal as it was: its settings given back, the color scheme notices off. For a panic and for a signal, so
/// it only calls what a signal handler may (`tcsetattr`, `write`).
fn leave() {
    loud_input();
    unsafe { libc::write(libc::STDOUT_FILENO, STOP_SCHEME.as_ptr().cast(), STOP_SCHEME.len()) };
}

/// A signal that stops a panel: the terminal as it was, then the signal's own way.
extern "C" fn on_signal(signal: libc::c_int) {
    leave();
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// While it lives, the keyboard stays quiet (see [`quiet_input`]).
struct QuietInput {
    _panic: PanicGuard,
}

impl Drop for QuietInput {
    fn drop(&mut self) {
        // A panic: its hook did it already.
        if !std::thread::panicking() {
            leave();
        }
    }
}

/// However the pane may have moved without the board knowing (a terminal that drew it again, the computer asleep),
/// the whole board is written again this often, and whenever the clock jumps further than `CLOCK_JUMP`.
const WHOLE_EVERY: Duration = Duration::from_secs(30);
const CLOCK_JUMP: i64 = 5;

/// The members' sessions by name, and every session's name by process: replies are addressed to a process.
#[derive(Default, Clone)]
struct Sessions {
    members: HashMap<String, Running>,
    by_pid: HashMap<u32, String>,
    /// Sessions open elsewhere under a member's name, with the folder they work in: messages by name could go astray.
    elsewhere: Vec<(String, Option<String>)>,
    /// The exact addresses of the members whose name another session carries too (`claude::addresses`).
    refs: BTreeMap<String, String>,
    /// The members at rest while a command they started still runs (see [`at_rest_with_a_command`]), and since when,
    /// when their file says it.
    shell: HashMap<String, Option<i64>>,
    /// The states the members' panes tell (OSC 7501), and since when in seconds since the epoch: they come before
    /// `claude agents`, which says them only once asked.
    told: HashMap<String, (State, Option<u64>)>,
    /// The text of the error each member's turn failed on, as its pane last said it (OSC 7501 `state=error`).
    failed: HashMap<String, String>,
    /// Until `claude agents` first answers: the states file as it was, whose exact addresses, sessions and processes
    /// are written again rather than emptied (a dashboard started again would change the team's note for a second).
    kept: Option<Box<States>>,
    error: Option<String>,
}

/// The states file to keep writing until `claude agents` first answers: only one a dashboard wrote moments ago (one
/// started again); not an older one, a launch before's, whose sessions and addresses are no longer.
fn kept(state: &Path) -> Option<Box<States>> {
    read_states(state).filter(|states| states.fresh(now())).map(Box::new)
}

/// How often the dashboard asks `claude agents` while each member's pane tells its state: for the sessions (the
/// journal's transcripts, the session a member resumes), the names in use elsewhere and the processes.
const AGENTS_EVERY: Duration = Duration::from_secs(5);

/// The members' states as their panes tell them (OSC 7501), by name; none when the server could not be asked.
fn told(panes: Option<&[backend::PaneState]>) -> HashMap<String, (State, Option<u64>)> {
    let members = panes.unwrap_or_default().iter().filter(|pane| pane.role.is_empty());
    members.filter_map(|pane| Some((pane.member.clone(), (pane.status?, pane.since)))).collect()
}

/// The text of the error each member's pane says its turn failed on, by name.
fn failed(panes: Option<&[backend::PaneState]>) -> HashMap<String, String> {
    let members = panes.unwrap_or_default().iter().filter(|pane| pane.role.is_empty());
    members
        .filter(|pane| pane.status == Some(State::Error))
        .filter_map(|p| Some((p.member.clone(), p.error.clone()?)))
        .collect()
}

/// A member whose pane says it is at rest may have a command still running (its pane says `idle` then, its session's
/// file `shell`): read in its file, by the process `claude agents` last gave. Its pane says it works: no command shown.
fn with_a_command(found: &mut Sessions, status: impl Fn(u32) -> Option<claude::SessionFile>) {
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
/// carries too when `refs` gives the team's state folder (the dashboard, for `STATES`; the journal has no use for them).
fn sessions(s: &Snapshot, refs: Option<&Path>) -> Sessions {
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
fn at_rest_with_a_command(found: &mut Sessions, status: impl Fn(u32) -> Option<claude::SessionFile>) {
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
fn printable(text: &str) -> String {
    let formatting = |c: char| {
        matches!(c,
            '\u{AD}' | '\u{61C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
    };
    text.chars().filter(|&c| !c.is_control() && !formatting(c)).collect()
}

/// The sessions `claude agents --json` gives, sorted out for the team.
fn sorted(s: &Snapshot, running: Vec<Running>) -> Sessions {
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

impl Sessions {
    /// `states` with these sessions: each member's, every one's name by process, and the exact addresses of the
    /// members whose name another session carries too; until `claude agents` answers, those of the file as it was.
    fn written(&self, states: States) -> States {
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
    /// one's name by process. None from a dashboard older than 2.0.1, which wrote members without their sessions.
    fn of(states: States) -> Option<Sessions> {
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

/// How far back the activity curves go, in seconds.
const HISTORY: i64 = 30 * 60;
/// From no activity to all the time.
const LEVELS: [char; 5] = ['⣀', '⣠', '⣤', '⣶', '⣿'];
/// Narrower than this, cards go one per line.
const MIN_CARD: usize = 30;

fn member_color(s: &Snapshot, name: &str) -> Color {
    s.members.iter().position(|m| m.name == name).map_or(Color::Reset, look::member_color)
}

/// A member whose Claude runs, as its card shows it.
#[derive(Debug, Clone)]
struct Card {
    name: String,
    color: Color,
    contact: bool,
    state: State,
    /// Seconds in this state.
    since: u64,
    /// Of the model's window.
    context: Option<f64>,
    /// How near the session is to compacting on its own.
    pressure: Pressure,
    /// The model's family (`Opus`), and the effort's level its requests go with.
    model: Option<String>,
    effort: Option<String>,
    /// What it does, in progress and once done.
    doing: Option<Task>,
    /// Its state each second, as (time, working).
    samples: Vec<(i64, bool)>,
    /// Its last half hour as stretches at work, instead of `samples`: the cards the server draws, which borrow nothing
    /// they would copy at each image.
    work: Option<Activity>,
    /// The agents its session started: subagents, teammates.
    helpers: Vec<Helper>,
    /// At rest while a command it started still runs.
    shell: bool,
    /// Its turn failed (`State::Error`): why, short (`look::error_label`), and the error's text as its pane gave it.
    failed: Option<(String, Option<String>)>,
    /// When it was last compacted, in seconds since the epoch: a ⟳ in its curve, at that time, until the curve's
    /// half hour is over.
    compacted: Option<i64>,
}

/// What the dashboard draws.
#[derive(Debug, Clone, Default)]
struct Board {
    now: i64,
    error: Option<String>,
    /// The account's windows: label, percent used, seconds before it resets.
    usage: Vec<(String, f64, Option<u64>)>,
    cards: Vec<Card>,
    absent: Vec<String>,
    /// The spinners' image.
    frame: usize,
    glyphs: Glyphs,
    /// Sessions open elsewhere under a member's name, with their folder.
    elsewhere: Vec<(String, Option<String>)>,
}

/// The team as it stands, `team.json` read again: members added, removed or renamed while it runs. As it was, when the
/// file cannot be read (written again at this very moment).
fn reread(s: &mut Snapshot, state: &Path) {
    if let Ok(now) = Snapshot::read(state) {
        *s = now;
    }
}

fn dashboard(first: &Snapshot, state: &Path) -> Result<()> {
    let mut out = std::io::stdout();
    queue!(out, cursor::Hide)?;
    let redraw = Arc::new(AtomicBool::new(false));
    let _quiet = quiet_input(Arc::clone(&redraw));
    // Whether a member's pane does not tell its state: `claude agents` is then asked each second.
    let eager = &AtomicBool::new(true);
    std::thread::scope(|scope| {
        // `claude agents --json` takes a tenth of a second or more, and far more on a loaded machine: asked on the side,
        // it stalls neither the spinners nor the looks of each second.
        let (found, agents) = mpsc::channel();
        scope.spawn(move || {
            let mut s = first.clone();
            loop {
                reread(&mut s, state);
                let asked = Instant::now();
                if found.send(sessions(&s, Some(state))).is_err() {
                    break;
                }
                // Again in a while; each second while a member's pane does not tell its state.
                while asked.elapsed() < AGENTS_EVERY && !eager.load(Ordering::Relaxed) {
                    std::thread::sleep(TICK);
                }
                if eager.load(Ordering::Relaxed) {
                    std::thread::sleep(TICK);
                }
            }
        });
        // Each second, the states as the members' panes tell them (one question to the server), with what `claude
        // agents` last said: the states file is written each second, however long `claude agents` takes.
        let (sender, news) = mpsc::channel();
        scope.spawn(move || {
            // The clients asked at each look: one can attach again from another terminal.
            let backend = backend::of(first);
            let clients = || backend.client_terminals(&first.session);
            let mut s = first.clone();
            // The first look waits a moment for `claude agents`, no more: the panes' states are enough to draw.
            let mut last = match agents.recv_timeout(TICK) {
                Ok(found) => found,
                Err(RecvTimeoutError::Timeout) => Sessions { kept: kept(state), ..Default::default() },
                Err(RecvTimeoutError::Disconnected) => return,
            };
            loop {
                reread(&mut s, state);
                while let Ok(found) = agents.try_recv() {
                    last = found;
                }
                let panes = backend.panes(&s.session).ok();
                let (told, failed) = (told(panes.as_deref()), failed(panes.as_deref()));
                let mut sessions = Sessions { told, failed, ..last.clone() };
                eager.store(s.members.iter().any(|m| !sessions.told.contains_key(&m.name)), Ordering::Relaxed);
                with_a_command(&mut sessions, |pid| claude::session_status(s.config_dir.as_deref(), pid));
                if sender.send((s.clone(), (sessions, Glyphs::of(&clients())))).is_err() {
                    break;
                }
                std::thread::sleep(TICK);
            }
        });
        // The states written on the side too: the drawing never waits for the disk.
        let (states, written) = mpsc::channel::<States>();
        scope.spawn(move || {
            for states in written {
                // Not written, the menu asks `claude agents` itself, once the file is old.
                let _ = save_states(state, &states);
            }
        });
        let start = Instant::now();
        let mut memory = Memory::default();
        let mut board = Board::default();
        let mut screen = Screen::default();
        // What the zones file says: nothing yet, it may be the last launch's.
        let mut saved: Option<Vec<Zone>> = None;
        let mut fresh = news.recv().ok();
        let mut whole = Instant::now();
        let mut clock = now();
        loop {
            if let Some((s, (sessions, glyphs))) = fresh.take() {
                board = Board { glyphs, ..memory.board(&s, state, &sessions) };
                // A look that failed says nothing of the states: left to age, the file sends the menu to ask itself.
                if board.error.is_none() {
                    let seen = memory.states(&board, SystemTime::now(), Instant::now());
                    let _ = states.send(sessions.written(seen));
                }
            }
            for card in &mut board.cards {
                if let Some((_, at)) = memory.since.get(&card.name) {
                    card.since = at.elapsed().as_secs();
                }
            }
            board.now = now();
            board.frame = frame(start.elapsed());
            let jumped = (board.now - clock).abs() > CLOCK_JUMP;
            clock = board.now;
            if redraw.swap(false, Ordering::Relaxed) || jumped || whole.elapsed() >= WHOLE_EVERY {
                screen.forget();
                whole = Instant::now();
            }
            screen.draw(&mut out, &board)?;
            if saved.as_ref() != Some(&screen.zones) {
                // Not written, it is only the clicks that miss; tried again when the layout changes.
                let _ = save_zones(state, &screen.zones);
                saved = Some(screen.zones.clone());
            }
            // The spinners turn while someone works; otherwise only the clock moves, redrawn on the second.
            let working = board.cards.iter().any(|c| c.state == State::Working);
            fresh = match news.recv_timeout(if working { FRAME } else { next_second() }) {
                Ok(look) => Some(look),
                Err(RecvTimeoutError::Timeout) => None,
                // Only a panic stops the looking, and the scope passes it on.
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            };
        }
    })
}

/// What the dashboard remembers from one look to the next: since when each member is in its state, and what it did.
#[derive(Default)]
struct Memory {
    since: HashMap<String, (State, Instant)>,
    activity: HashMap<String, VecDeque<(i64, bool)>>,
    /// The name each session last showed under, and when: a member renamed on its conversation keeps its time and
    /// its curve, across the seconds it takes to start again.
    sessions: HashMap<String, (String, Instant)>,
}

/// Older than this, in seconds, a member's report says nothing of the agents it started: its mod ticks every 2 s.
const FRESH_REPORT: i64 = 10;

/// The agents a member's session started that its card shows at `now`: while its mod reports (a member stopped took
/// them along), those still shown.
fn helpers_shown(report: &Report, now: i64) -> Vec<Helper> {
    if now - report.at > FRESH_REPORT {
        return Vec::new();
    }
    report.helpers.iter().filter(|h| h.shown(now)).cloned().collect()
}

/// How long the dashboard remembers a session it no longer sees, under its last name.
const KEEP: Duration = Duration::from_secs(60);

impl Memory {
    /// The states of `board`, just made, for `STATES`, at `now` on the clock, which is `seen` on the dashboard's:
    /// since when in seconds since the epoch, counted back in milliseconds, so that it holds from one look to the next.
    fn states(&self, board: &Board, now: SystemTime, seen: Instant) -> States {
        let at = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let members = board.cards.iter().filter_map(|card| {
            let (state, since) = self.since.get(&card.name)?;
            let since = (at - seen.saturating_duration_since(*since).as_millis() as i64).div_euclid(1000);
            Some((card.name.clone(), MemberState { state: *state, since, shell: card.shell }))
        });
        States {
            at: at.div_euclid(1000),
            members: members.collect(),
            absent: board.absent.clone(),
            refs: BTreeMap::new(),
            sessions: BTreeMap::new(),
            pids: BTreeMap::new(),
            errors: board
                .cards
                .iter()
                .filter_map(|card| Some((card.name.clone(), card.failed.as_ref()?.1.clone()?)))
                .collect(),
            cards: board
                .cards
                .iter()
                .map(|card| {
                    (card.name.clone(), CardLook { doing: card.doing.clone(), activity: Activity::of(&card.samples) })
                })
                .collect(),
        }
    }

    /// The board for the sessions just found.
    fn board(&mut self, s: &Snapshot, state: &Path, sessions: &Sessions) -> Board {
        let now = now();
        // One moment for the whole look: members seen in the same state at once keep the file's order.
        let seen = Instant::now();
        let mut board =
            Board { now, error: sessions.error.clone(), elsewhere: sessions.elsewhere.clone(), ..Default::default() };
        // Hidden for screenshots (scripts/screenshots.sh): the user's account is not the demo's.
        if std::env::var_os(NO_USAGE).is_none() {
            board.usage = usage(state, s, now);
        }
        // The contacts first, as in the tabs.
        let members = s.members.iter().filter(|m| m.contact).chain(s.members.iter().filter(|m| !m.contact));
        // Since when a member's pane says its state, counted back from this moment.
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        for member in members {
            let session = sessions.members.get(&member.name);
            let told = sessions.told.get(&member.name);
            if session.is_none() && told.is_none() {
                board.absent.push(member.name.clone());
                continue;
            }
            if let Some(id) = session.and_then(|session| session.session_id.as_ref()) {
                if let Some((old, _)) = self.sessions.get(id).filter(|(old, _)| *old != member.name) {
                    let old = old.clone();
                    if let Some(since) = self.since.remove(&old) {
                        self.since.insert(member.name.clone(), since);
                    }
                    if let Some(activity) = self.activity.remove(&old) {
                        self.activity.insert(member.name.clone(), activity);
                    }
                }
                self.sessions.insert(id.clone(), (member.name.clone(), seen));
            }
            // As its pane tells it, else as `claude agents` last said.
            let st = match told {
                Some((st, _)) => *st,
                None => State::of(session.and_then(|s| s.status.as_deref()).unwrap_or_default()),
            };
            let entry = self.since.entry(member.name.clone()).or_insert((st, seen));
            if entry.0 != st {
                *entry = (st, seen);
            }
            if let Some((_, Some(since))) = told {
                let back = Duration::from_millis(at.saturating_sub(since.saturating_mul(1000)));
                entry.1 = seen.checked_sub(back).unwrap_or(seen);
            }
            // At rest with a command running: since its file says, a panel opened again included.
            if let Some(Some(at)) = sessions.shell.get(&member.name).filter(|_| st == State::Idle) {
                let back = Duration::from_secs(now.saturating_sub(*at).max(0) as u64);
                entry.1 = seen.checked_sub(back).unwrap_or(seen);
            }
            let samples = self.activity.entry(member.name.clone()).or_default();
            samples.push_back((now, st == State::Working));
            while samples.front().is_some_and(|(at, _)| *at <= now - HISTORY) {
                samples.pop_front();
            }
            let report = bridge::report(state, &member.name).unwrap_or_default();
            let (model, effort) = bridge::model_and_effort(state, member);
            let context = report.context.as_ref();
            let helpers = helpers_shown(&report, now);
            board.cards.push(Card {
                name: member.name.clone(),
                color: member_color(s, &member.name),
                contact: member.contact,
                state: st,
                since: entry.1.elapsed().as_secs(),
                context: context.and_then(|c| c.percent),
                pressure: Pressure::of(context.and_then(|c| c.tokens), report.compacts_at()),
                model: model.map(|m| family(&m)),
                effort: effort.filter(|e| bridge::EFFORTS.contains(&e.as_str())),
                doing: bridge::doing(state, &member.name),
                samples: samples.iter().copied().collect(),
                work: None,
                helpers,
                shell: sessions.shell.contains_key(&member.name),
                failed: (st == State::Error).then(|| {
                    let text = sessions.failed.get(&member.name);
                    (look::error_label(text.map(String::as_str)), text.cloned())
                }),
                compacted: report.compacted.as_ref().filter(|c| c.done).map(|c| c.at),
            });
        }
        // Those no longer in the team, forgotten; but for a while those whose session may come back under another name.
        self.sessions.retain(|_, (_, at)| at.elapsed() < KEEP);
        let kept = |name: &String| {
            s.members.iter().any(|m| &m.name == name) || self.sessions.values().any(|(last, _)| last == name)
        };
        let gone: Vec<String> = self.since.keys().chain(self.activity.keys()).filter(|n| !kept(n)).cloned().collect();
        for name in gone {
            self.since.remove(&name);
            self.activity.remove(&name);
        }
        // The working agents at rest, the latest first, by the very moment: two in the same second keep their order
        // from one second to the next, and so does the dashboard (see `ordered`).
        board.cards.sort_by_key(|c| {
            (!c.contact && resting(c)).then(|| std::cmp::Reverse(self.since.get(&c.name).map(|(_, at)| *at)))
        });
        board
    }
}

/// What the pane shows, sent by difference cell by cell ([`Painter`]): with the spinners turning ten times a second,
/// a frame sends the few cells that changed, not the lines they are on. Each frame a synchronized update: the
/// multiplexer, which may read a whole one in several goes, shows it only once it is all there.
struct Screen {
    /// The frame being drawn, in the same buffers from one to the next.
    canvas: Canvas,
    painter: Painter,
    zones: Vec<Zone>,
}

impl Default for Screen {
    fn default() -> Self {
        Screen {
            canvas: Canvas::default(),
            painter: Painter::new(Features { sync: true, ..Features::default() }),
            zones: Vec::new(),
        }
    }
}

impl Screen {
    /// Forgets what the pane shows: the next frame is sent whole.
    fn forget(&mut self) {
        self.painter.forget();
    }

    /// The board over the pane, only what changed.
    fn draw(&mut self, out: &mut impl Write, board: &Board) -> Result<()> {
        let changes = self.changes(terminal::size().unwrap_or((80, 24)), board);
        if !changes.is_empty() {
            out.write_all(&changes)?;
            out.flush()?;
        }
        Ok(())
    }

    /// What to write for `board` on a pane of `size`: the cells that changed, in one write; nothing if none did.
    fn changes(&mut self, size: (u16, u16), board: &Board) -> Vec<u8> {
        let (width, height) = (size.0 as usize, size.1 as usize);
        let Drawn { lines, zones } = render(board, width, height);
        self.canvas.reset(width, height);
        for (y, line) in lines.iter().enumerate() {
            line.draw(&mut self.canvas, y);
        }
        self.zones = zones;
        self.painter.frame(&self.canvas)
    }
}

/// The time before the clock's next second, and a little more so that it has moved on.
fn next_second() -> Duration {
    let into = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_millis());
    Duration::from_millis(u64::from(1000 - into) + 5)
}

/// Set, the dashboard leaves the account's usage out (screenshots). Not documented.
const NO_USAGE: &str = "RECRUIT_NO_USAGE";

/// The account's 5-hour and 7-day windows, from the members' reports.
fn usage(state: &Path, s: &Snapshot, now: i64) -> Vec<(String, f64, Option<u64>)> {
    let reports: Vec<Report> = s.members.iter().filter_map(|m| bridge::report(state, &m.name)).collect();
    windows(&reports, now)
}

/// Each window as the reports tell it: label, percent used, seconds before it resets. A report keeps what its session
/// last heard, old news for a member at rest however recent the report: per window, the one that resets last (a new
/// window resets later than the one before), and of those the highest use (it only goes up in a window). Without a
/// reset time, a report counts only when no other tells one.
fn windows(reports: &[Report], now: i64) -> Vec<(String, f64, Option<u64>)> {
    [("five_hour", t!("5 h", "5 h")), ("seven_day", t!("7 j", "7 d"))]
        .into_iter()
        .filter_map(|(kind, label)| {
            let (resets, percent) = reports
                .iter()
                .flat_map(|r| &r.rate_limits)
                .filter(|l| l.kind == kind)
                .map(|l| (l.resets_at.as_deref().and_then(iso_epoch), l.percent_used))
                .max_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))?;
            match resets {
                // Over, and no session has heard of the next one yet: nothing used in it so far.
                Some(at) if at <= now => Some((label, 0.0, None)),
                _ => Some((label, percent, resets.map(|at| (at - now) as u64))),
            }
        })
        .collect()
}

/// The dashboard's lines for a pane of `width` × `height`: the cards from the top, each in the form the room leaves
/// it (see [`arrange`]), the account's usage at the bottom; no header, its frame says the counts and the bar the keys
/// (mock-up B1). An error first, if any.
fn render(b: &Board, width: usize, height: usize) -> Drawn {
    let mut top = Vec::new();
    if let Some(error) = &b.error {
        top.extend([Line::of(fit(error, width), Paint::Color(Color::DarkRed)), Line::default()]);
    }
    let mut bottom = Vec::new();
    if !b.usage.is_empty() {
        bottom.extend([Line::default(), usage_line(&b.usage, width)]);
    }
    let absent = names(&t!("absent", "absent"), &b.absent, width, Color::DarkGrey, usize::MAX).spaced();
    let room = height.saturating_sub(1 + top.len() + bottom.len() + absent.len());
    let layout = arrange(&ordered(&b.cards), width, room);

    let mut middle = cards(&layout.rows, width, b);
    if layout.more > 0 {
        middle.lines.push(Line::of(t!("… et {} de plus", "… and {} more", layout.more), Paint::Dim));
    }
    middle.append(resting_names(&layout.named, width, layout.whole));
    // The agents the sessions started, in the room the members leave.
    let left = room.saturating_sub(middle.len());
    middle.append(helper_block(b, width, left));
    middle.append(absent);
    // Sessions open elsewhere under a member's name, over the usage: in the room the cards leave, if any.
    if let Some(line) = elsewhere_line(&b.elsewhere, width) {
        let mut with = if bottom.is_empty() { vec![Line::default()] } else { bottom.clone() };
        with.insert(1, line);
        if 1 + top.len() + middle.len() + with.len() <= height {
            bottom = with;
        }
    }

    let filler = height.saturating_sub(1 + top.len() + middle.len() + bottom.len());
    let mut drawn = Drawn::from(top);
    drawn.append(middle);
    drawn.append(Drawn::from(vec![Line::default(); filler]));
    drawn.append(Drawn::from(bottom));
    // A pane too low for all that: the usage at the bottom only, if it holds.
    let rows = height.saturating_sub(1);
    if drawn.len() > rows {
        let mut low = Vec::new();
        if !b.usage.is_empty() && rows >= 1 {
            low.extend(vec![Line::default(); rows - 1]);
            low.push(usage_line(&b.usage, width));
        }
        drawn = Drawn::from(low);
    }
    drawn
}

/// Lines of the dashboard, and where the members show in them, rows counted from the first line.
#[derive(Debug, Default)]
struct Drawn {
    lines: Vec<Line>,
    zones: Vec<Zone>,
}

impl From<Vec<Line>> for Drawn {
    fn from(lines: Vec<Line>) -> Self {
        Drawn { lines, zones: Vec::new() }
    }
}

impl Drawn {
    fn len(&self) -> usize {
        self.lines.len()
    }

    /// `below` under these lines.
    fn append(&mut self, below: Drawn) {
        let rows = self.lines.len();
        self.zones.extend(below.zones.into_iter().map(|zone| Zone { row: zone.row + rows, ..zone }));
        self.lines.extend(below.lines);
    }

    /// After an empty line, unless there is nothing.
    fn spaced(self) -> Drawn {
        if self.lines.is_empty() {
            return self;
        }
        let mut spaced = Drawn::from(vec![Line::default()]);
        spaced.append(self);
        spaced
    }
}

/// Where a member shows on the dashboard, its card or its name in a list: a click there leads to its pane; or the
/// context of a member at rest, which a click offers to compact. Rows and columns of the pane, from 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Zone {
    member: String,
    #[serde(default)]
    kind: ZoneKind,
    row: usize,
    col: usize,
    rows: usize,
    cols: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ZoneKind {
    #[default]
    Member,
    Compact,
}

impl Zone {
    fn contains(&self, x: usize, y: usize) -> bool {
        (self.col..self.col + self.cols).contains(&x) && (self.row..self.row + self.rows).contains(&y)
    }
}

/// The zones as the dashboard last drew them, in the team's folder.
pub const ZONES: &str = "zones.json";

/// Written in one go: a click never reads half a file.
fn save_zones(state: &Path, zones: &[Zone]) -> Result<()> {
    write_atomic(&state.join(ZONES), &serde_json::to_string(zones)?)
}

fn zones(state: &Path) -> Vec<Zone> {
    fs::read(state.join(ZONES)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

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
    /// transcripts) and the members' panes (`member.rs`, the session to resume), which no longer ask `claude agents`
    /// themselves while the dashboard looks (2.0.1). Not as fresh as `at`: up to about 8 s old when `at` is (asked
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
}

impl Activity {
    /// The stretches at work in `samples` (one a second, oldest first): a sample at work and the next second's join.
    fn of(samples: &[(i64, bool)]) -> Activity {
        let mut work: Vec<(i64, i64)> = Vec::new();
        for &(at, _) in samples.iter().filter(|(_, working)| *working) {
            match work.last_mut() {
                Some((_, to)) if at <= *to => *to = (at + 1).max(*to),
                _ => work.push((at, at + 1)),
            }
        }
        Activity { seen: samples.first().map_or(0, |(at, _)| *at), work }
    }

    /// One sample a second from when it was first seen up to `now`, at work where a stretch says so: what `curve` takes.
    #[cfg(test)]
    pub(crate) fn samples(&self, now: i64) -> Vec<(i64, bool)> {
        if self.seen == 0 {
            return Vec::new();
        }
        let from = self.seen.max(now - HISTORY);
        let mut stretches = self.work.iter().peekable();
        (from..=now)
            .map(|at| {
                while stretches.next_if(|(_, to)| *to <= at).is_some() {}
                (at, stretches.peek().is_some_and(|(start, to)| (*start..*to).contains(&at)))
            })
            .collect()
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
    /// At rest while a command it started still runs (decision of 2026-10-09): shown with a terminal's sign.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shell: bool,
}

/// A state by its name in `STATES`: `working`, `idle`, `waiting` or `other`.
mod state_name {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::look::State;

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
fn save_states(state: &Path, states: &States) -> Result<()> {
    write_atomic(&state.join(STATES), &serde_json::to_string(states)?)
}

/// How a piece of a line is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Paint {
    Plain,
    Dim,
    Color(Color),
    Bold(Color),
    /// Colored and dimmed.
    Faint(Color),
}

impl Paint {
    /// As a cell of a canvas is drawn.
    fn style(self) -> Style {
        let fg = |color: Color| Style { fg: (color != Color::Reset).then_some(color), ..Style::PLAIN };
        match self {
            Paint::Plain => Style::PLAIN,
            Paint::Dim => Style::PLAIN.dim(),
            Paint::Color(color) => fg(color),
            Paint::Bold(color) => fg(color).bold(),
            Paint::Faint(color) => fg(color).dim(),
        }
    }

    /// `text` as a terminal prints it, in this paint.
    fn apply(self, text: &str) -> String {
        match self {
            Paint::Plain => text.to_string(),
            Paint::Dim => text.dim().to_string(),
            Paint::Color(color) => text.with(color).to_string(),
            Paint::Bold(color) => text.with(color).bold().to_string(),
            Paint::Faint(color) => text.with(color).dim().to_string(),
        }
    }
}

/// A line of a panel: pieces of text one after the other, each in its paint.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Line(Vec<(String, Paint)>);

impl Line {
    fn of(text: impl Into<String>, paint: Paint) -> Line {
        let mut line = Line::default();
        line.push(text, paint);
        line
    }

    fn push(&mut self, text: impl Into<String>, paint: Paint) {
        let text = text.into();
        if !text.is_empty() {
            self.0.push((text, paint));
        }
    }

    /// `other` after this one.
    fn append(&mut self, other: Line) {
        self.0.extend(other.0);
    }

    /// Its text, without its paints.
    #[cfg(test)]
    fn text(&self) -> String {
        self.0.iter().map(|(text, _)| text.as_str()).collect()
    }

    /// On row `y` of `canvas`, from its first column.
    fn draw(&self, canvas: &mut Canvas, y: usize) {
        let mut x = 0;
        for (text, paint) in &self.0 {
            x = canvas.put(x, y, text, paint.style());
        }
    }

    /// On row `y` of `canvas`, from column `x`, cut before column `end`; returns the column after it.
    pub(crate) fn draw_at(&self, canvas: &mut Canvas, x: usize, y: usize, end: usize) -> usize {
        let mut x = x;
        for (text, paint) in &self.0 {
            x = canvas.put_in(x, y, end, text, paint.style());
        }
        x
    }

    /// As a terminal prints it: for the journal, which writes its lines as they come.
    fn printed(&self) -> String {
        self.0.iter().map(|(text, paint)| paint.apply(text)).collect()
    }
}

/// `pieces` one after the other, each in its paint, cut to `width` as `fit` cuts their text joined.
fn painted(pieces: &[(impl AsRef<str>, Paint)], width: usize) -> Line {
    let joined: String = pieces.iter().map(|(text, _)| text.as_ref()).collect();
    let mut rest: Vec<char> = fit(&joined, width).chars().collect();
    let mut line = Line::default();
    for (text, paint) in pieces {
        let taken: String = rest.drain(..text.as_ref().chars().count().min(rest.len())).collect();
        line.push(taken, *paint);
    }
    line
}

/// ` 5 h ▰▰▰▰▱▱▱▱ 41 % ↻ 4h27   7 j ▰▱▱▱▱▱▱▱ 5 % ↻ 6j11h`, the bars as long as the line allows. Too narrow, the reset
/// times go first, then the bars.
fn usage_line(usage: &[(String, f64, Option<u64>)], width: usize) -> Line {
    const MIN_CELLS: usize = 5;
    const MAX_CELLS: usize = 20;
    // What follows each bar: the share used, and the time before the window resets.
    let tails = |resets: bool| -> Vec<(String, String)> {
        usage
            .iter()
            .map(|(_, percent, left)| {
                let left = left.filter(|_| resets).map(|secs| format!(" ↻ {}", remaining(secs)));
                (format!("{percent:.0} %"), left.unwrap_or_default())
            })
            .collect()
    };
    // The line but the bars, with a space after each.
    let windows = usage.len().max(1);
    let plain = |tails: &[(String, String)]| {
        let texts: usize = usage
            .iter()
            .zip(tails)
            .map(|((label, ..), (used, left))| 2 + label.chars().count() + used.chars().count() + left.chars().count())
            .sum();
        texts + windows + 2 * (windows - 1)
    };
    let (tails, cells) = [true, false]
        .into_iter()
        .map(|resets| {
            let tails = tails(resets);
            let cells = (width.saturating_sub(plain(&tails)) / windows).min(MAX_CELLS);
            (tails, cells)
        })
        .find(|(_, cells)| *cells >= MIN_CELLS)
        .unwrap_or_else(|| (tails(false), 0));
    let mut line = Line::default();
    for (i, ((label, percent, _), (used, left))) in usage.iter().zip(tails).enumerate() {
        line.push(format!("{} {label} ", if i == 0 { "" } else { "  " }), Paint::Plain);
        if cells > 0 {
            let filled = ((percent / 100.0 * cells as f64).round() as usize).min(cells);
            let color = match percent {
                p if *p >= 90.0 => Color::Red,
                p if *p >= 70.0 => look::palette().working,
                _ => Color::Green,
            };
            line.push("▰".repeat(filled), Paint::Color(color));
            line.push("▱".repeat(cells - filled), Paint::Color(Color::DarkGrey));
            line.push(" ", Paint::Plain);
        }
        line.push(used, Paint::Plain);
        line.push(left, Paint::Dim);
    }
    line
}

/// `au repos  a · b · c`, wrapped under its label over at most `max` lines, each name a zone; the names left over
/// counted at the end, `… et 4 autres`.
fn names(label: &str, list: &[String], width: usize, color: Color, max: usize) -> Drawn {
    if list.is_empty() || max == 0 {
        return Drawn::default();
    }
    const INDENT: usize = 10;
    // Each line's names: the member, as shown, from which column.
    let mut rows: Vec<Vec<(&String, String, usize)>> = vec![Vec::new()];
    let mut used = INDENT;
    for name in list {
        // Alone on its line and still too wide: cut, the zone still names the member.
        let shown = fit(name, width.saturating_sub(INDENT).max(1));
        let len = shown.chars().count();
        if rows.last().is_some_and(|row| !row.is_empty()) && used + 3 + len > width {
            rows.push(Vec::new());
            used = INDENT;
        }
        let row = rows.last_mut().expect("one row at least");
        if !row.is_empty() {
            used += 3;
        }
        row.push((name, shown, used));
        used += len;
    }
    let mut left = 0;
    if rows.len() > max {
        left = rows[max..].iter().map(Vec::len).sum();
        rows.truncate(max);
        let last = rows.last_mut().expect("one row at least");
        // Room for the count after the last name kept.
        while let Some((_, shown, col)) = last.last() {
            if col + shown.chars().count() + 1 + others(left).chars().count() <= width {
                break;
            }
            last.pop();
            left += 1;
        }
    }
    let mut drawn = Drawn::default();
    for (i, row) in rows.iter().enumerate() {
        let head = if i == 0 { format!("{label:<INDENT$}") } else { " ".repeat(INDENT) };
        let shown: Vec<&str> = row.iter().map(|(_, shown, _)| shown.as_str()).collect();
        let mut line = Line::of(head, Paint::Dim);
        line.push(shown.join(" · "), Paint::Color(color));
        if left > 0 && i + 1 == rows.len() {
            line.push(if row.is_empty() { "" } else { " " }, Paint::Plain);
            line.push(others(left), Paint::Dim);
        }
        drawn.lines.push(line);
        drawn.zones.extend(row.iter().map(|(name, shown, col)| Zone {
            member: name.to_string(),
            kind: ZoneKind::Member,
            row: i,
            col: *col,
            rows: 1,
            cols: shown.chars().count(),
        }));
    }
    drawn
}

/// `… et 4 autres`: names left out of a list.
fn others(count: usize) -> String {
    if count == 1 { t!("… et 1 autre", "… and 1 more") } else { t!("… et {} autres", "… and {} more", count) }
}

/// ` nom en double ailleurs : coordinateur (shop)`, dimmed: sessions open elsewhere under a member's name, which
/// messages by name could reach.
fn elsewhere_line(list: &[(String, Option<String>)], width: usize) -> Option<Line> {
    if list.is_empty() {
        return None;
    }
    let names: Vec<String> = list
        .iter()
        .map(|(name, folder)| match folder {
            Some(folder) => format!("{name} ({folder})"),
            None => name.clone(),
        })
        .collect();
    let names = names.join(", ");
    let text = if list.len() == 1 {
        t!(" nom en double ailleurs : {}", " name in use elsewhere: {}", names)
    } else {
        t!(" noms en double ailleurs : {}", " names in use elsewhere: {}", names)
    };
    Some(Line::of(fit(&text, width), Paint::Dim))
}

/// The working agents at rest that have no card: `au repos  a · b · c`, after an empty line; on one line, those that
/// hold and how many more, unless `whole`.
fn resting_names(list: &[&Card], width: usize, whole: bool) -> Drawn {
    let list: Vec<String> = list.iter().map(|c| c.name.clone()).collect();
    names(&t!("au repos", "idle"), &list, width, State::Idle.color(), if whole { usize::MAX } else { 1 }).spaced()
}

/// The columns cards go in, side by side: the right one a column wider on an odd width.
fn column_widths(width: usize, columns: usize) -> Vec<usize> {
    if columns == 1 { vec![width] } else { vec![width / 2, width - width / 2] }
}

/// The lines a card `width` wide takes: its title and its curve, and in between what the member does over at most
/// `lines` lines.
fn card_height(c: &Card, width: usize, lines: usize) -> usize {
    2 + if lines == 0 { 0 } else { doing_lines(c, width, lines).len() }
}

/// The dashboard's parts, in order: the contacts, the working agents at work or waiting for the user, those at rest.
fn section(c: &Card) -> usize {
    if c.contact {
        0
    } else if resting(c) {
        2
    } else {
        1
    }
}

/// The cards in the order the dashboard shows them: the contacts; the agents waiting for the user, then those at
/// work; then those at rest, the latest first. Otherwise, as they come (the file's order, and for members at rest
/// since the same second, the latest first: see [`Memory::board`]).
fn ordered(cards: &[Card]) -> Vec<&Card> {
    let mut list: Vec<&Card> = cards.iter().collect();
    list.sort_by_key(|c| match section(c) {
        // Waiting for the user, then failed, then at work (mock-up C).
        1 => (
            1,
            match c.state {
                State::Waiting => 0,
                State::Error => 1,
                _ => 2,
            },
        ),
        2 => (2, c.since),
        s => (s, 0),
    });
    list
}

/// How a card is drawn: across the pane or beside another, with what the member does over at most so many lines; or,
/// for a working agent at rest, only its name in a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Card { half: bool, lines: usize },
    Named,
}

/// A card's forms as room runs out: what the member does over two lines; then over one line; then beside another, when
/// two hold side by side; then without what it does; a working agent at rest ends in the list of names.
fn forms(c: &Card, two: bool) -> Vec<Form> {
    let card = |half, lines| Form::Card { half, lines };
    let mut forms = vec![card(false, 2), card(false, 1)];
    if two {
        forms.extend([card(true, 1), card(true, 0)]);
    } else {
        forms.push(card(false, 0));
    }
    if !c.contact && resting(c) {
        forms.push(Form::Named);
    }
    forms
}

/// Where the cards go: rows of one card across the pane or two side by side, each card with the lines it gives what
/// the member does; the working agents at rest only named, all of them or on one line; how many cards found no room
/// at all.
#[derive(Debug, Default)]
struct Layout<'a> {
    rows: Vec<Vec<(&'a Card, usize)>>,
    named: Vec<&'a Card>,
    whole: bool,
    more: usize,
}

/// How the working agents at rest without a card show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Names {
    /// All of them, over as many lines as it takes.
    Whole,
    /// Those that hold on one line, and how many more.
    Line,
    /// Only counted, with the cards that found no room.
    Counted,
}

/// A card of the list by its place in it, with the lines it gives what the member does.
type Placed = (usize, usize);

/// The cards of `list`, in its order, over `room` lines of a pane `width` wide, each in its own form. `list` goes from
/// what matters most to what matters least: the contacts, the agents waiting for the user, at work, at rest from the
/// latest. In that order, each card takes the richest form that still leaves room for all those after it in their
/// barest one: across the pane with what the member does over two lines, then one; then beside the next card when two
/// hold side by side, then without what it does; a working agent at rest, at last, only named. Those named go on one
/// line while the cards choose; all of them if they hold once the cards have. A card with nothing to say of what the
/// member does only goes across the pane after that, with the room left. Even all in their barest form too many: the
/// first cards that hold, the agents at rest only counted with the others while the cards choose, then named if they
/// still hold. The agents the sessions started have their own cards under the members, in the room left (see
/// [`helper_block`]): they never take what the members do.
///
/// The same cards always get the same layout: it changes only with them, their states or the pane.
fn arrange<'a>(list: &[&'a Card], width: usize, room: usize) -> Layout<'a> {
    let two = width >= 2 * MIN_CARD;
    let ladders: Vec<Vec<Form>> = list.iter().map(|c| forms(c, two)).collect();
    // Each card's height across the pane and in the narrower column, for each number of lines, measured once.
    let half = column_widths(width, 2)[0];
    let heights: Vec<[[usize; 3]; 2]> =
        list.iter().map(|c| [width, half].map(|w| [0, 1, 2].map(|lines| card_height(c, w, lines)))).collect();
    let tall = |i: usize, half: bool, lines: usize| heights[i][usize::from(half)][lines];

    // The layout for each card's form, the first `shown` cards only; and the lines it takes.
    let build = |levels: &[usize], shown: usize, names: Names| -> (Layout<'a>, usize) {
        let mut layout = Layout { whole: names == Names::Whole, ..Default::default() };
        let mut rows: Vec<(Vec<Placed>, usize)> = Vec::new();
        let mut placed = 0;
        // Cards side by side, two by two in their order; one left over goes across.
        let mut run: Vec<Placed> = Vec::new();
        let pair = |run: &mut Vec<Placed>, rows: &mut Vec<(Vec<Placed>, usize)>| {
            for two in run.chunks(2) {
                let height = match two {
                    [(i, lines)] => tall(*i, false, *lines),
                    _ => two.iter().map(|&(i, lines)| tall(i, true, lines)).max().unwrap_or(0),
                };
                rows.push((two.to_vec(), height));
            }
            run.clear();
        };
        for (i, c) in list.iter().enumerate() {
            if run.first().is_some_and(|&(j, ..)| section(list[j]) != section(c)) {
                pair(&mut run, &mut rows);
            }
            match ladders[i][levels[i]] {
                Form::Named if names == Names::Counted => layout.more += 1,
                Form::Named => layout.named.push(c),
                Form::Card { .. } if placed == shown => layout.more += 1,
                Form::Card { half, lines } => {
                    placed += 1;
                    if half {
                        run.push((i, lines));
                    } else {
                        pair(&mut run, &mut rows);
                        rows.push((vec![(i, lines)], tall(i, false, lines)));
                    }
                }
            }
        }
        pair(&mut run, &mut rows);
        let mut height = rows.iter().map(|(_, h)| h + 1).sum::<usize>().saturating_sub(1);
        height += usize::from(layout.more > 0) + resting_names(&layout.named, width, layout.whole).len();
        layout.rows =
            rows.into_iter().map(|(row, _)| row.into_iter().map(|(i, lines)| (list[i], lines)).collect()).collect();
        (layout, height)
    };
    let fits = |levels: &[usize], shown: usize, names: Names| build(levels, shown, names).1 <= room;

    let barest: Vec<usize> = ladders.iter().map(|forms| forms.len() - 1).collect();
    let mut levels = barest.clone();
    let all = list.len();
    let cut = !fits(&levels, all, Names::Line);
    // Cut short, as many cards as hold, the names only counted with the others while the cards choose.
    let (shown, names) = if cut {
        match (0..all).rev().find(|&shown| fits(&levels, shown, Names::Counted)) {
            Some(shown) => (shown, Names::Counted),
            // Not even how many: nothing.
            None => return Layout::default(),
        }
    } else {
        (all, Names::Line)
    };
    // Each card in turn takes the richest form that holds, from `from`.
    let choose = |levels: &mut Vec<usize>, names: Names, from: &dyn Fn(usize) -> usize| {
        // Cut short, the agents at rest stay named: a card would go past the cards shown.
        for i in (0..all).filter(|&i| !cut || section(list[i]) != 2) {
            let was = levels[i];
            levels[i] = (from(i)..was)
                .find(|&form| {
                    levels[i] = form;
                    fits(levels, shown, names)
                })
                .unwrap_or(was);
        }
    };
    // What the members do first. Across the pane, a card that says nothing more than beside another: it waits for the
    // names.
    let plain = |i: usize| list[i].doing.is_none() && list[i].failed.is_none();
    let beside = |i: usize| ladders[i].iter().position(|f| matches!(f, Form::Card { half: true, .. }));
    choose(&mut levels, names, &|i| if plain(i) { beside(i).unwrap_or(0) } else { 0 });
    // Then the names: all of them if they hold, else on one line, else only counted.
    let names = [Names::Whole, Names::Line].into_iter().find(|&names| fits(&levels, shown, names)).unwrap_or(names);
    // The room left, to the cards that say nothing more across the pane.
    choose(&mut levels, names, &|_| 0);
    build(&levels, shown, names).0
}

/// Rows of cards, one across the pane or two side by side, an empty line between two rows, each as tall as its tallest
/// card; each card a zone, and the context of a member at rest another, to compact it. The model, the effort and the
/// time in columns from one card to the next of the same width.
fn cards(rows: &[Vec<(&Card, usize)>], width: usize, b: &Board) -> Drawn {
    let placed: Vec<Vec<(&Card, usize, usize)>> = rows
        .iter()
        .map(|row| {
            let widths = column_widths(width, row.len());
            row.iter().zip(widths).map(|(&(c, lines), w)| (c, w, lines)).collect()
        })
        .collect();
    let right = |n: usize| Right::of(placed.iter().filter(|row| row.len() == n).flatten().map(|&(c, w, _)| (c, w)));
    let rights = [right(1), right(2)];
    let mut drawn = Drawn::default();
    for (i, row) in placed.iter().enumerate() {
        if i > 0 {
            drawn.lines.push(Line::default());
        }
        let top = drawn.len();
        let right = &rights[row.len() - 1];
        let row: Vec<(&Card, usize, Vec<Line>)> =
            row.iter().map(|&(c, w, lines)| (c, w, card(c, w, lines, right, b))).collect();
        let mut col = 0;
        for (c, w, card) in &row {
            let zone = |kind, row, col, rows, cols| Zone { member: c.name.clone(), kind, row, col, rows, cols };
            drawn.zones.push(zone(ZoneKind::Member, top, col, card.len(), *w));
            // A card too narrow for its context shows none of it: nothing to compact there.
            if let Some(cols) = compaction_cells(c).filter(|cols| cols < w)
                && let Some(last) = card.len().checked_sub(1)
            {
                drawn.zones.push(zone(ZoneKind::Compact, top + last, col + w - 1 - cols, 1, cols));
            }
            col += w;
        }
        let height = row.iter().map(|(.., card)| card.len()).max().unwrap_or(0);
        for i in 0..height {
            let mut line = Line::default();
            for (j, (_, w, card)) in row.iter().enumerate() {
                match card.get(i) {
                    Some(text) => line.append(text.clone()),
                    // Under a shorter card, room for the one on its right.
                    None if j + 1 < row.len() => line.push(" ".repeat(*w), Paint::Plain),
                    None => {}
                }
            }
            drawn.lines.push(line);
        }
    }
    drawn
}

/// What the titles show on the right of the names, the same on every card of a width and in columns from one to the
/// next: the model, the effort's sign and name, the time. Each column as wide as its widest; the richest form that
/// every card holds: `Opus   ◉ extra`, else `Opus   ◉`, else `O ◉`, else the time alone (decision of 2026-10-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Right {
    model: usize,
    /// The model's initial rather than its name: one column.
    initial: bool,
    /// One for the sign alone, more for the sign and the name, none for neither.
    effort: usize,
    time: usize,
}

impl Right {
    /// For cards, each with its width.
    fn of<'a>(list: impl IntoIterator<Item = (&'a Card, usize)>) -> Self {
        let list: Vec<(&Card, usize)> = list.into_iter().collect();
        let widest =
            |each: &dyn Fn(&Card) -> Option<usize>| list.iter().filter_map(|(c, _)| each(c)).max().unwrap_or(0);
        let time = widest(&|c| Some(duration(c.since).chars().count()));
        let model = widest(&|c| c.model.as_ref().map(|m| m.chars().count()));
        // The name's column is as wide as the longest name of the language, a level's or `max`.
        let level = widest(&|c| c.effort.as_ref().map(|_| 2 + look::effort_name_width()));
        let forms = [
            Right { model, initial: false, effort: level, time },
            Right { model, initial: false, effort: level.min(1), time },
            Right { model: model.min(1), initial: true, effort: level.min(1), time },
        ];
        let bare = Right { model: 0, initial: false, effort: 0, time };
        forms
            .into_iter()
            .filter(|right| right.width() > 0)
            .find(|right| list.iter().all(|(c, width)| right.width() + 3 <= right.rest(c, *width)))
            .unwrap_or(bare)
    }

    fn width(&self) -> usize {
        self.model + if self.effort > 0 && self.model > 0 { 1 } else { 0 } + self.effort
    }

    /// What a card `width` wide leaves on its title past its sign, its name and the time.
    fn rest(&self, c: &Card, width: usize) -> usize {
        let room = width.saturating_sub(3);
        room.saturating_sub(2 + name_in(c, room, self.time).chars().count() + self.time)
    }
}

/// The member's name on a title of `room` columns, the time `time` wide.
fn name_in(c: &Card, room: usize, time: usize) -> String {
    fit(&c.name, room.saturating_sub(3 + time).max(1))
}

/// At rest, or in a state the dashboard does not know: the card fades.
fn resting(c: &Card) -> bool {
    !matches!(c.state, State::Working | State::Waiting | State::Error)
}

/// The context's field, at the right of the curve: `⟳ 88 %` for a member at rest, which a click compacts.
const CONTEXT: usize = 7;

/// How many columns of a card's context a click compacts the member on: at rest or its turn failed, its context known.
fn compaction_cells(c: &Card) -> Option<usize> {
    let percent = c.context.filter(|_| matches!(c.state, State::Idle | State::Error))?;
    Some(2 + format!("{percent:.0} %").chars().count())
}

/// A member's card for a caller that is not the dashboard (the « others » of the focus layout, drawn by the server): what
/// the dashboard's card shows, from what the server knows of the member.
#[derive(Debug, Clone)]
pub(crate) struct CardData {
    pub name: String,
    pub color: Color,
    pub state: State,
    /// Seconds in the state.
    pub since: u64,
    /// At rest while a command it started still runs.
    pub shell: bool,
    /// Of the model's window, in percent.
    pub context: Option<f64>,
    pub pressure: Pressure,
    /// The model's family (`Opus`), and the effort's level.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// What it does, in progress and once done (the request's short form).
    pub doing: Option<Task>,
    /// Its last half hour, as `MemberLook::activity` has it: the curve is drawn from its stretches.
    pub activity: Activity,
    /// Its turn failed: why, short (`look::error_label`).
    pub failed: Option<String>,
    /// When it was last compacted (`MemberLook::compacted`): a ⟳ in its curve.
    pub compacted: Option<i64>,
}

/// When and how a card is drawn: the clock, the spinners' image, the icons.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Clock {
    /// Seconds since the epoch.
    pub now: i64,
    pub frame: usize,
    pub glyphs: Glyphs,
}

impl CardData {
    /// As the dashboard's own card: no agents under it, not a contact.
    fn card(&self) -> Card {
        Card {
            name: self.name.clone(),
            color: self.color,
            contact: false,
            state: self.state,
            since: self.since,
            context: self.context,
            pressure: self.pressure,
            model: self.model.clone(),
            effort: self.effort.clone(),
            doing: self.doing.clone(),
            samples: Vec::new(),
            work: Some(self.activity.clone()),
            helpers: Vec::new(),
            shell: self.shell,
            failed: self.failed.clone().map(|label| (label, None)),
            compacted: self.compacted,
        }
    }
}

/// The cards `width` wide, each over at most `lines` lines of what the member does (none: the short form, its title and
/// its curve), as the dashboard draws them: one capsule on the left in the state's color. The model, the effort and the
/// time line up from one card to the next, as on the dashboard.
pub(crate) fn card_lines(data: &[&CardData], width: usize, lines: usize, clock: Clock) -> Vec<Vec<Line>> {
    let cards: Vec<Card> = data.iter().map(|data| data.card()).collect();
    let right = Right::of(cards.iter().map(|c| (c, width)));
    let board = Board { now: clock.now, frame: clock.frame, glyphs: clock.glyphs, ..Board::default() };
    cards.iter().map(|c| card(c, width, lines, &right, &board)).collect()
}

/// The rows a card takes at `width` over at most `lines` lines: what `card_lines` gives, without drawing it.
pub(crate) fn card_rows(data: &CardData, width: usize, lines: usize) -> usize {
    let doing = || doing_lines_of(data.failed.as_deref(), data.doing.as_ref(), data.state, width, lines).len();
    2 + if lines == 0 { 0 } else { doing() }
}

/// A card, `width` wide: its title, what the member does over at most `lines` lines, its curve and context. A capsule on the left in the state's color, on all its lines; the state
/// read by its intensity: yellow and bold at work, red waiting for the user, faded at rest.
///
/// ```text
/// ╻ ⠹ dev-mod            Opus █ xhigh   3m
/// ┃   Je relis la maquette des cartes
/// ╹   ⣀⣀⣠⣤⣶⣿⣿⣶⣤⣀⣀                   74 %
/// ```
fn card(c: &Card, width: usize, lines: usize, right: &Right, b: &Board) -> Vec<Line> {
    let doing = if lines == 0 { Vec::new() } else { doing_lines(c, width, lines) };
    let height = 2 + doing.len();
    let color = c.state.color();
    let line = if matches!(c.state, State::Waiting | State::Error) { Paint::Bold(color) } else { Paint::Color(color) };
    // From halfway down the first line to halfway down the last: two cards never touch.
    let bar = |row: usize| (look::stroke(row, height).to_string(), line);
    let (sign, name, time) = match c.state {
        State::Working => (Paint::Color(color), Paint::Bold(c.color), Paint::Plain),
        State::Waiting => (Paint::Color(color), Paint::Bold(c.color), Paint::Color(color)),
        State::Error => (Paint::Bold(color), Paint::Bold(c.color), Paint::Color(color)),
        _ => (Paint::Dim, Paint::Faint(c.color), Paint::Dim),
    };

    let room = width.saturating_sub(3);
    let shown = name_in(c, room, right.time);
    let meta = if right.width() > 0 { right.width() + 2 } else { 0 };
    let gap = room.saturating_sub(2 + shown.chars().count() + meta + right.time);
    let mut title = vec![
        bar(0),
        (" ".into(), Paint::Plain),
        // A failed turn says more than a command running.
        (
            if c.shell && c.state != State::Error { b.glyphs.console() } else { c.state.icon(b.glyphs, b.frame) }
                .to_string(),
            sign,
        ),
        (" ".into(), Paint::Plain),
        (shown, name),
        (" ".repeat(gap), Paint::Plain),
    ];
    if meta > 0 {
        title.extend(model_and_effort(c, right, b.frame));
        title.push(("  ".into(), Paint::Plain));
    }
    title.push((format!("{:>w$}", duration(c.since), w = right.time), time));
    title.push((" ".into(), Paint::Plain));

    let mut out = vec![painted(&title, width)];
    for (i, (text, paint)) in doing.into_iter().enumerate() {
        // As wide as the card, for the one on its right.
        let pad = " ".repeat(width.saturating_sub(4 + text.chars().count()));
        out.push(painted(&[bar(1 + i), ("   ".into(), Paint::Plain), (text, paint), (pad, Paint::Plain)], width));
    }
    let cells = width.saturating_sub(6 + CONTEXT);
    let mut bottom = vec![bar(height - 1), ("   ".into(), Paint::Plain)];
    // A compaction in the last half hour: a ⟳ in the curve, in the slice it fell in.
    match c.compacted.and_then(|at| compaction_cell(at, b.now, cells)) {
        Some(at) => {
            let curve: Vec<char> = c.curve(b.now, cells).chars().collect();
            let (before, after) = (curve[..at].iter().collect::<String>(), curve[at + 1..].iter().collect::<String>());
            bottom.extend([(before, Paint::Color(color)), ("⟳".into(), Paint::Plain), (after, Paint::Color(color))]);
        }
        None => bottom.push((c.curve(b.now, cells), Paint::Color(color))),
    }
    bottom.push((" ".into(), Paint::Plain));
    bottom.extend(context(c));
    bottom.push((" ".into(), Paint::Plain));
    out.push(painted(&bottom, width));
    out
}

// -- The agents the sessions started, as cards of their own ------------------------------------------------------
//
// Under the members, each kind in a section under a rule with its title (« ♙ teammates · 3 », « ⧗ subagents · 2 »):
// cards of the members' shape, the name in the clear, at the right the member that started them, faded in its color,
// no context. Short of room a section keeps fewer lines, then becomes a list of names, then a count: never at the
// expense of a member. A click on any of them leads to the member that started it.

/// An agent a member's session started, with that member.
struct Agent<'a> {
    helper: &'a Helper,
    member: &'a Card,
}

/// The subagents and the teammates the members started and the dashboard still shows: at work first, then the others
/// from the latest; the ones whose status held too long (ghosts) last.
fn agents(b: &Board) -> [Vec<Agent<'_>>; 2] {
    let mut subagents = Vec::new();
    let mut teammates = Vec::new();
    for member in ordered(&b.cards) {
        for helper in &member.helpers {
            let agent = Agent { helper, member };
            if helper.teammate.is_some() { teammates.push(agent) } else { subagents.push(agent) }
        }
    }
    let rank = |a: &Agent| match a.helper.status {
        _ if a.helper.unknown(b.now) => 3,
        HelperStatus::Running => 0,
        HelperStatus::Failed => 1,
        HelperStatus::Idle | HelperStatus::Done => 2,
    };
    for list in [&mut subagents, &mut teammates] {
        list.sort_by_key(|a| (rank(a), std::cmp::Reverse(a.helper.since), a.helper.id.clone()));
    }
    [subagents, teammates]
}

impl Agent<'_> {
    fn teammate(&self) -> bool {
        self.helper.teammate.is_some()
    }

    /// The state its card shows: at work, failed in orange, else at rest; unknown for a ghost.
    fn state(&self, now: i64) -> State {
        match self.helper.status {
            _ if self.helper.unknown(now) => State::Other,
            HelperStatus::Running => State::Working,
            HelperStatus::Failed => State::Error,
            HelperStatus::Idle | HelperStatus::Done => State::Idle,
        }
    }

    /// Its name on the card: a teammate's, a subagent's type.
    fn name(&self) -> String {
        self.helper.teammate.clone().unwrap_or_else(|| self.helper.kind.clone())
    }

    /// What it was asked, for the line under its name.
    fn text(&self) -> String {
        match self.helper.description.trim() {
            "" if self.teammate() => self.helper.kind.clone(),
            "" => String::new(),
            text => text.to_string(),
        }
    }

    /// Its sign: the state's for a teammate (the spinner at work, `?` for a ghost), an hourglass for a subagent at
    /// work, ✓ once over; ✗ once failed.
    fn sign(&self, b: &Board) -> char {
        let h = self.helper;
        match (self.teammate(), h.status) {
            (true, _) if h.unknown(b.now) => '?',
            (true, HelperStatus::Done) => '✓',
            (true, _) => self.state(b.now).icon(b.glyphs, b.frame),
            (false, HelperStatus::Done) => '✓',
            (false, HelperStatus::Failed) => '✗',
            (false, _) => b.glyphs.hourglass(),
        }
    }

    /// The seconds on its card: a teammate's in its status; a subagent's since it started, until it ended.
    fn time(&self, now: i64) -> String {
        let h = self.helper;
        let secs = if self.teammate() {
            now - h.since
        } else {
            (if h.status == HelperStatus::Running { now } else { h.since }) - h.started
        };
        duration(secs.max(0) as u64)
    }

    /// The sign of its kind, for the rule and the right of its card.
    fn kind_sign(&self, b: &Board) -> char {
        if self.teammate() { b.glyphs.person() } else { b.glyphs.hourglass() }
    }

    /// How it paints, as a card does: (sign, name, time).
    fn paints(&self, now: i64) -> (Paint, Paint, Paint) {
        let color = self.state(now).color();
        match self.state(now) {
            State::Working => (Paint::Color(color), Paint::Bold(Color::Reset), Paint::Plain),
            State::Error => (Paint::Bold(color), Paint::Bold(Color::Reset), Paint::Color(color)),
            _ => (Paint::Dim, Paint::Dim, Paint::Dim),
        }
    }
}

/// The share of time at work in each of `width` slices of the last half hour, as `curve` draws them, for something
/// seen since `from` and at work in the stretches `work` (`[from, to)` in seconds, oldest first): the slices are cut on
/// the clock too.
fn work_curve(from: i64, work: &[(i64, i64)], now: i64, width: usize) -> String {
    let slice = |t: i64| (t * width as i64).div_euclid(HISTORY);
    // The first second of slice `k`: the least `t` with `slice(t) >= k`.
    let start = |k: i64| (k * HISTORY + width as i64 - 1).div_euclid(width as i64);
    (0..width)
        .map(|i| {
            let k = slice(now) - (width - 1 - i) as i64;
            let (lo, hi) = (start(k), start(k + 1));
            let overlap = |a: i64, z: i64| (z.min(hi) - a.max(lo)).max(0);
            // Seen up to now only.
            let seen = overlap(from, now + 1);
            if seen == 0 {
                return ' ';
            }
            let busy: i64 = work.iter().map(|&(a, z)| overlap(a.max(from), z.min(now + 1))).sum();
            LEVELS[((busy.min(seen) as f64 / seen as f64) * 4.0).round() as usize]
        })
        .collect()
}

/// An agent's card, `width` wide: its title, what it was asked when `text`, its curve; the member's shape, with its
/// time `time` wide. The curve is made of the one change of status it knows of.
fn agent_card(a: &Agent, width: usize, text: bool, time: usize, b: &Board) -> Vec<Line> {
    let state = a.state(b.now);
    let color = state.color();
    let height = 2 + usize::from(text);
    let line = if state == State::Error { Paint::Bold(color) } else { Paint::Color(color) };
    let bar = |row: usize| (look::stroke(row, height).to_string(), line);
    let (sign, name, time_paint) = a.paints(b.now);
    let room = width.saturating_sub(3);
    let at = a.time(b.now);
    // At the right, the member that started it: `♙ dev-interface`, else the sign alone, else nothing.
    let member = a.member.name.as_str();
    let tail = 2 + time;
    let full = format!("{} {member}", a.kind_sign(b));
    let name_room = |extra: usize| room.saturating_sub(2 + extra + tail);
    let shown = fit(&a.name(), name_room(0).max(1));
    let right = [full, a.kind_sign(b).to_string(), String::new()]
        .into_iter()
        .find(|r| 2 + shown.chars().count() + r.chars().count() + usize::from(!r.is_empty()) + tail <= room)
        .unwrap_or_default();
    let gap = room.saturating_sub(2 + shown.chars().count() + right.chars().count() + tail);
    let mut title = vec![
        bar(0),
        (" ".into(), Paint::Plain),
        (a.sign(b).to_string(), sign),
        (" ".into(), Paint::Plain),
        (shown, name),
        (" ".repeat(gap), Paint::Plain),
    ];
    if !right.is_empty() {
        title.push((right, Paint::Faint(a.member.color)));
        title.push(("  ".into(), Paint::Plain));
    }
    title.push((format!("{at:>time$}"), time_paint));
    title.push((" ".into(), Paint::Plain));
    let mut out = vec![painted(&title, width)];
    if text {
        let paint = match state {
            State::Working => Paint::Plain,
            State::Error => Paint::Color(color),
            _ => Paint::Dim,
        };
        let line = vec![bar(1), ("   ".into(), Paint::Plain), (fit(&a.text(), width.saturating_sub(4)), paint)];
        out.push(painted(&line, width));
    }
    let h = a.helper;
    let until = if h.status == HelperStatus::Running { b.now } else { h.since };
    let cells = width.saturating_sub(6 + CONTEXT);
    let bottom = [
        bar(height - 1),
        ("   ".into(), Paint::Plain),
        (work_curve(h.started, &[(h.started, until)], b.now, cells), Paint::Color(color)),
        (" ".into(), Paint::Plain),
    ];
    out.push(painted(&bottom, width));
    out
}

/// A section's rule and title: `── ♙ teammates · 3 ───────`, dimmed.
fn rule(sign: char, title: &str, count: usize, width: usize) -> Line {
    let text = format!(" {sign} {title} · {count} ");
    let rest = width.saturating_sub(1 + text.chars().count());
    Line(vec![("─".into(), Paint::Dim), (text, Paint::Dim), ("─".repeat(rest), Paint::Dim)])
}

/// How far a section of `n` agents is shown, from the barest: level 0, only their names on a line; levels `1..=n`, that
/// many cards (the others named under them); levels `n + 1..=2n`, all the cards, that many more with the text of what
/// they were asked. Each level takes more lines than the one before: the room fills card by card.
fn section_rows(level: usize, n: usize) -> usize {
    match level {
        0 => 1,
        // The rule, the cards (2 lines, a blank between), a blank and the names of the others.
        l if l < n => 1 + (3 * l - 1) + 2,
        l if l == n => 1 + (3 * n - 1),
        l => 1 + (3 * n - 1) + (l - n),
    }
}

/// A section's names on one line, `  ♙  ⠹ ux-critic  ⠹ perf  ◷ a11y`, the ones that hold and how many more; each name
/// leads to the member that started it.
fn agent_names(list: &[Agent], kind: char, width: usize, b: &Board) -> Drawn {
    let mut line = Line(vec![("  ".into(), Paint::Plain), (kind.to_string(), Paint::Dim), ("  ".into(), Paint::Plain)]);
    let mut col = 2 + 1 + 2;
    let mut zones = Vec::new();
    let mut shown = 0;
    for a in list {
        let (sign, name, _) = a.paints(b.now);
        let len = 2 + a.name().chars().count();
        let left = list.len() - shown - 1;
        let more = if left > 0 { others(left).chars().count() + 3 } else { 0 };
        if col + len + more > width && shown > 0 {
            break;
        }
        if shown > 0 {
            line.push("  ", Paint::Plain);
            col += 2;
        }
        line.push(a.sign(b).to_string(), sign);
        line.push(" ", Paint::Plain);
        let label = fit(&a.name(), width.saturating_sub(col + 2).max(1));
        zones.push(Zone {
            member: a.member.name.clone(),
            kind: ZoneKind::Member,
            row: 0,
            col,
            rows: 1,
            cols: 2 + label.chars().count(),
        });
        col += 2 + label.chars().count();
        line.push(label, name);
        shown += 1;
    }
    if shown < list.len() {
        line.push("  ", Paint::Plain);
        line.push(others(list.len() - shown), Paint::Dim);
    }
    Drawn { lines: vec![line], zones }
}

/// A section's noun for `n` of them: « teammate », « teammates »; « sous-agent », « sous-agents ».
fn noun(teammates: bool, n: usize) -> String {
    match (teammates, n) {
        (true, 1) => t!("teammate", "teammate"),
        (true, _) => t!("teammates", "teammates"),
        (false, 1) => t!("sous-agent", "subagent"),
        (false, _) => t!("sous-agents", "subagents"),
    }
}

/// `  ♙ 3 teammates`: yellow when one works, orange when one failed.
fn agent_count(list: &[Agent], kind: char, b: &Board) -> Drawn {
    let paint = if list.iter().any(|a| a.state(b.now) == State::Working) {
        Paint::Color(State::Working.color())
    } else if list.iter().any(|a| a.state(b.now) == State::Error) {
        Paint::Color(State::Error.color())
    } else {
        Paint::Dim
    };
    let teammates = list.first().is_some_and(Agent::teammate);
    Drawn::from(vec![Line::of(format!("  {kind} {} {}", list.len(), noun(teammates, list.len())), paint)])
}

/// A section drawn to `level` (see [`section_rows`]): its rule, then its cards, the names of the others.
fn agent_section(list: &[Agent], level: usize, width: usize, b: &Board) -> Drawn {
    let Some(first) = list.first() else { return Drawn::default() };
    let n = list.len();
    let kind = first.kind_sign(b);
    if level == 0 {
        // Names; a width that holds hardly two of them: a count.
        let names = agent_names(list, kind, width, b);
        let shown = names.zones.len();
        return if shown < 2 && n > 1 { agent_count(list, kind, b) } else { names };
    }
    let (cards, texts) = if level <= n { (level, 0) } else { (n, level - n) };
    let mut drawn = Drawn::from(vec![rule(kind, &noun(first.teammate(), n), n, width)]);
    // The times line up from one card to the next.
    let time = list.iter().map(|a| a.time(b.now).chars().count()).max().unwrap_or(0).max(3);
    for (i, a) in list.iter().take(cards).enumerate() {
        if i > 0 {
            drawn.lines.push(Line::default());
        }
        let card = agent_card(a, width, i < texts, time, b);
        drawn.zones.push(Zone {
            member: a.member.name.clone(),
            kind: ZoneKind::Member,
            row: drawn.len(),
            col: 0,
            rows: card.len(),
            cols: width,
        });
        drawn.lines.extend(card);
    }
    if cards < n {
        drawn.lines.push(Line::default());
        drawn.append(agent_names(&list[cards..], kind, width, b));
    }
    drawn
}

/// The sections of agents under the members, in `room` lines, an empty line before them and between them: the teammates,
/// which last, then the subagents, which come and go with every tool, at the very end. The teammates matter most. A
/// section that does not hold even as names is left out, the subagents first. Then one chain of states, each a little richer than the one before:
/// the teammates one card at a time, to all their text, then the subagents in the room that is left. The richest state
/// that holds is drawn: so no section ever gets richer as the room shrinks, and the room is filled card by card.
fn helper_block(b: &Board, width: usize, room: usize) -> Drawn {
    let [subagents, teammates] = agents(b);
    // In the order they show, and the order that matters: the teammates, which last, then the subagents, which come and
    // go at the very end.
    let sections: Vec<&Vec<Agent>> = [&teammates, &subagents].into_iter().filter(|l| !l.is_empty()).collect();
    if sections.is_empty() || width == 0 {
        return Drawn::default();
    }
    // The lines of the levels chosen, an empty line before the first and between: a section left out has none.
    let rows = |levels: &[Option<usize>]| {
        let held: Vec<usize> =
            levels.iter().zip(&sections).filter_map(|(level, l)| Some(section_rows((*level)?, l.len()))).collect();
        if held.is_empty() { 0 } else { 1 + held.iter().sum::<usize>() + held.len() - 1 }
    };
    // The barest for each in turn, the teammates first, while it holds; those that do not go.
    let mut levels: Vec<Option<usize>> = vec![None; sections.len()];
    for i in 0..sections.len() {
        levels[i] = Some(0);
        if rows(&levels) > room {
            levels[i] = None;
        }
    }
    // The chain: the teammates' levels one by one, then the subagents'.
    let order: Vec<usize> = (0..sections.len()).filter(|&i| levels[i].is_some()).collect();
    'chain: for &i in &order {
        for level in 1..=2 * sections[i].len() {
            levels[i] = Some(level);
            if rows(&levels) > room {
                levels[i] = Some(level - 1);
                break 'chain;
            }
        }
    }
    let mut drawn = Drawn::from(vec![Line::default()]);
    let mut first = true;
    for (list, level) in sections.iter().zip(&levels) {
        let Some(level) = level else { continue };
        if !std::mem::take(&mut first) {
            drawn.lines.push(Line::default());
        }
        drawn.append(agent_section(list, *level, width, b));
    }
    if first { Drawn::default() } else { drawn }
}

/// What the member does, over at most `lines` lines of a card `width` wide, each in its paint: the task in progress;
/// at rest, the task done after ✓, faded, the lines after the first under its text; in a state the dashboard does not
/// know, the task as it was. Nothing before the model has said. Its turn failed: « Tour raté : API surchargée (529) »
/// in orange, then the task it stopped in, after ↳, faded (mock-up C).
fn doing_lines(c: &Card, width: usize, lines: usize) -> Vec<(String, Paint)> {
    doing_lines_of(c.failed.as_ref().map(|(label, _)| label.as_str()), c.doing.as_ref(), c.state, width, lines)
}

/// The same from what it takes: why the turn failed, the task, the state: no card to build for the rows it takes.
fn doing_lines_of(
    failed: Option<&str>,
    doing: Option<&Task>,
    state: State,
    width: usize,
    lines: usize,
) -> Vec<(String, Paint)> {
    if let Some(label) = failed {
        let room = width.saturating_sub(5);
        let mut out =
            vec![(fit(&t!("Tour raté : {}", "Turn failed: {}", label), room), Paint::Color(State::Error.color()))];
        if let Some(task) = doing.filter(|task| lines > 1 && !task.now.trim().is_empty()) {
            out.push((fit(&format!("↳ {}", task.now), room), Paint::Dim));
        }
        return out;
    }
    let Some(task) = doing else { return Vec::new() };
    let paint = if matches!(state, State::Working | State::Waiting | State::Error) { Paint::Plain } else { Paint::Dim };
    let (mark, text) = match &task.done {
        Some(done) if state == State::Idle => ("✓ ", done),
        _ if state == State::Idle => ("✓ ", &task.now),
        _ => ("", &task.now),
    };
    let indent = mark.chars().count();
    wrap(text, width.saturating_sub(5 + indent), lines)
        .into_iter()
        .enumerate()
        .map(|(i, line)| if i == 0 { format!("{mark}{line}") } else { format!("{}{line}", " ".repeat(indent)) })
        .map(|line| (line, paint))
        .collect()
}

/// `text` over at most `lines` lines of `width` characters, cut between words, a word longer than a line where it
/// must; an ellipsis ends the last line when the text goes on. One line at least, empty for an empty text.
fn wrap(text: &str, width: usize, lines: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = line.chars().count();
            if used > 0 && used + 1 + word.len() <= width {
                line.push(' ');
                line.extend(&word);
                break;
            }
            if used > 0 {
                out.push(std::mem::take(&mut line));
                continue;
            }
            if word.len() <= width {
                line.extend(&word);
                break;
            }
            // Longer than a line: cut, the rest on the next.
            out.push(word.drain(..width).collect());
        }
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    let lines = lines.max(1);
    if out.len() > lines {
        let rest = out.split_off(lines - 1).join(" ");
        out.push(fit(&rest, width));
    }
    out
}

/// The model, faded (its initial short of room, `?` while it is not known), then the effort's sign and name in Claude
/// Code's colors; each padded to its column.
fn model_and_effort(c: &Card, right: &Right, frame: usize) -> Vec<(String, Paint)> {
    let model = match (&c.model, right.initial) {
        (_, true) => look::model_initial(c.model.as_deref()).to_string(),
        (Some(model), false) => model.clone(),
        (None, false) => "?".to_string(),
    };
    let mut pieces = Vec::new();
    if right.model > 0 {
        pieces.push((format!("{model:<w$}", w = right.model), Paint::Dim));
    }
    if right.effort == 0 {
        return pieces;
    }
    if right.model > 0 {
        pieces.push((" ".into(), Paint::Plain));
    }
    let mut used = 0;
    if let Some(level) = &c.effort {
        // `max` in a rainbow, turning with the spinners.
        // A name that is not known, longer than the column, is cut at it.
        for (ch, color) in look::effort_cells(level, right.effort > 1, frame / 2).into_iter().take(right.effort) {
            pieces.push((ch.to_string(), color.map_or(Paint::Plain, Paint::Color)));
            used += 1;
        }
    }
    pieces.push((" ".repeat(right.effort.saturating_sub(used)), Paint::Plain));
    pieces
}

/// The context's field, `CONTEXT` wide: the share of the window, orange near compaction, red where Claude Code
/// warns; `⟳` before it for a member at rest.
fn context(c: &Card) -> Vec<(String, Paint)> {
    let Some(percent) = c.context else { return vec![(" ".repeat(CONTEXT), Paint::Plain)] };
    let text = format!("{percent:.0} %");
    let paint = match c.pressure {
        Pressure::Warning => Paint::Bold(Color::Red),
        Pressure::Near => Paint::Color(look::palette().orange),
        Pressure::Calm if resting(c) => Paint::Dim,
        Pressure::Calm => Paint::Plain,
    };
    let compacts = compaction_cells(c);
    let mut pieces = vec![(" ".repeat(CONTEXT.saturating_sub(compacts.unwrap_or(text.chars().count()))), Paint::Plain)];
    if compacts.is_some() {
        pieces.push(("⟳ ".into(), Paint::Dim));
    }
    pieces.push((text, paint));
    pieces
}

/// The share of time spent working in each of `width` slices of the last half hour, oldest first, as braille
/// bars; blank where nothing was seen yet. The slices are cut on the clock, not counted back from `now`: one past
/// stays as it is, only the current one fills, and the curve moves one slice at a time.
fn curve(samples: &[(i64, bool)], now: i64, width: usize) -> String {
    let (mut seen, mut busy) = (vec![0u32; width], vec![0u32; width]);
    let slice = |at: i64| (at * width as i64).div_euclid(HISTORY);
    for &(at, working) in samples {
        let back = slice(now) - slice(at);
        if !(0..width as i64).contains(&back) {
            continue;
        }
        let i = width - 1 - back as usize;
        seen[i] += 1;
        busy[i] += u32::from(working);
    }
    (0..width)
        .map(|i| match seen[i] {
            0 => ' ',
            n => LEVELS[((busy[i] as f64 / n as f64) * 4.0).round() as usize],
        })
        .collect()
}

impl Card {
    /// Its curve, `width` cells: from the stretches at work when it has them, else from its samples.
    fn curve(&self, now: i64, width: usize) -> String {
        match &self.work {
            Some(activity) => activity.curve(now, width),
            None => curve(&self.samples, now, width),
        }
    }
}

impl Activity {
    /// The curve of its stretches, `width` cells, as `curve` draws samples: a cell is blank before it was seen.
    fn curve(&self, now: i64, width: usize) -> String {
        if self.seen == 0 {
            return " ".repeat(width);
        }
        work_curve(self.seen, &self.work, now, width)
    }
}

/// The cell of a curve `width` cells wide that a compaction at `at` falls in, if it is still in the half hour it shows:
/// the slices are cut on the clock, as the curve's own.
fn compaction_cell(at: i64, now: i64, width: usize) -> Option<usize> {
    let slice = |t: i64| (t * width as i64).div_euclid(HISTORY);
    let back = slice(now) - slice(at);
    (width > 0 && (0..width as i64).contains(&back)).then(|| width - 1 - back as usize)
}

/// "42min", "4h27", "6j12h": the time before a window resets, short.
fn remaining(secs: u64) -> String {
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes}min"),
        (0, _) => format!("{hours}h{minutes:02}"),
        _ => t!("{}j{}h", "{}d{}h", days, hours),
    }
}

/// What the journal writes, by the time it happened.
enum Entry {
    Message(Message),
    Compacted(Compacted),
}

impl Entry {
    fn at(&self) -> i64 {
        match self {
            Entry::Message(m) => m.at,
            Entry::Compacted(c) => c.at,
        }
    }
}

/// A message one member sent another.
#[derive(Debug, Clone, PartialEq)]
struct Message {
    at: i64,
    from: String,
    to: String,
    text: String,
}

/// The messages a line of `from`'s conversation sends.
fn messages(from: &str, line: &str) -> Vec<Message> {
    if !line.contains(r#""name":"SendMessage""#) {
        return Vec::new();
    }
    let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else { return Vec::new() };
    let at = entry.get("timestamp").and_then(|t| t.as_str()).and_then(iso_epoch).unwrap_or_else(now);
    let Some(blocks) = entry.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .filter(|b| b.get("name").and_then(|n| n.as_str()) == Some("SendMessage"))
        .filter_map(|b| b.get("input"))
        .map(|input| {
            let text = |key: &str| match input.get(key) {
                Some(serde_json::Value::String(s)) => s.trim().to_string(),
                Some(other) if !other.is_null() => other.to_string(),
                _ => String::new(),
            };
            let summary = text("summary");
            let body = if summary.is_empty() { text("message") } else { summary };
            Message {
                at,
                from: from.to_string(),
                to: printable(&text("to")),
                text: printable(body.lines().next().unwrap_or_default()),
            }
        })
        .collect()
}

/// A recipient by name: replies go to `uds:/tmp/cc-socks/<pid>.sock`, the sender's process; a name two sessions share
/// comes with the one meant, `coordinateur [ceeb11]`, which `claude agents --json` does not give: only the name is
/// kept.
fn recipient(to: &str, by_pid: &HashMap<u32, String>) -> String {
    let pid = to.strip_prefix("uds:").and_then(|path| Path::new(path).file_stem()?.to_str()?.parse::<u32>().ok());
    let named = || {
        let reference = to.strip_suffix(']').and_then(|rest| rest.rsplit_once(" ["));
        match reference {
            Some((name, id)) if !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit()) => name.to_string(),
            _ => to.to_string(),
        }
    };
    pid.and_then(|pid| by_pid.get(&pid).cloned()).unwrap_or_else(named)
}

/// What a conversation file gained since the last read, in whole lines.
#[derive(Default)]
struct Tail {
    offset: u64,
    rest: Vec<u8>,
}

impl Tail {
    fn read(&mut self, file: &Path) -> Vec<String> {
        let Ok(mut handle) = fs::File::open(file) else { return Vec::new() };
        let len = handle.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            *self = Tail::default();
        }
        let mut bytes = std::mem::take(&mut self.rest);
        if handle.seek(SeekFrom::Start(self.offset)).is_err() || handle.read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        self.offset = len.max(self.offset);
        let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        self.rest = bytes.split_off(complete);
        String::from_utf8_lossy(&bytes).lines().map(String::from).collect()
    }
}

/// Earlier messages shown when the journal opens.
const BACKLOG: usize = 200;

/// How long the journal waits for the context a compaction left (`Compaction::after`) before it writes the line
/// without it: the mod tells within a minute.
const COMPACTION_GRACE: i64 = 65;

/// What the journal writes in its stream besides messages: a member's compaction, made or failed on its own.
#[derive(Debug, Clone, PartialEq)]
struct Compacted {
    at: i64,
    member: String,
    /// Made by the server, not by hand.
    auto: bool,
    /// Its context before and after, in percent of its window.
    before: Option<u8>,
    after: Option<u8>,
    /// An automatic one that failed: why, as the member's mod said it.
    failed: Option<String>,
}

/// The compactions of `member`'s report not written yet: its last one made, once the context after is known (or the
/// wait is over), and an automatic one that failed. `printed` keeps what was written, by member, request and kind.
fn compactions(state: &Path, member: &str, now: i64, printed: &mut HashSet<String>) -> Vec<Compacted> {
    let Some(report) = bridge::report(state, member) else { return Vec::new() };
    let mut found = Vec::new();
    if let Some(c) = report.compacted.as_ref().filter(|c| c.done) {
        let key = format!("{member}/{}/{}", c.id, c.at);
        if (c.after.is_some() || now - c.at > COMPACTION_GRACE) && printed.insert(key) {
            found.push(Compacted {
                at: c.at,
                member: member.to_string(),
                auto: c.auto,
                before: c.percent,
                after: c.after,
                failed: None,
            });
        }
    }
    // Refused because an agent was at work: nothing, it will come back. Refused for any other reason, an automatic
    // one: a line, once.
    if let Some(c) = report.compaction.as_ref().filter(|c| c.auto && !c.done && c.skip.is_none()) {
        let key = format!("{member}/{}/{}/failed", c.id, c.at);
        if let Some(error) = c.error.as_ref().filter(|_| printed.insert(key)) {
            found.push(Compacted {
                at: c.at,
                member: member.to_string(),
                auto: true,
                before: c.percent,
                after: None,
                failed: Some(error.clone()),
            });
        }
    }
    found
}

/// `text` in `room` characters, cut between words with « … » when it does not hold; None when not even one whole word
/// holds (half of one would say nothing).
fn fit_words(text: &str, room: usize) -> Option<String> {
    if text.chars().count() <= room {
        return Some(text.to_string());
    }
    let mut kept = String::new();
    for word in text.split_whitespace() {
        let wanted = kept.chars().count() + usize::from(!kept.is_empty()) + word.chars().count();
        if wanted + 1 > room {
            break;
        }
        if !kept.is_empty() {
            kept.push(' ');
        }
        kept.push_str(word);
    }
    (!kept.is_empty()).then(|| format!("{kept}…"))
}

/// How a compaction event lays out in a pane `width` wide: its head (` 14:32  ⟳ dev-rendu`) and its `words`, whole ones
/// (an atom such as `· 74 → 9 %` never breaks), over one line or two: the words that hold after the head, then the
/// rest under it, from the name's column. None when the rest does not hold on the second line either.
fn lay_out(head: usize, indent: usize, words: &[String], width: usize) -> Option<(usize, Vec<String>)> {
    let mut used = head;
    let mut first = 0;
    for word in words {
        let wanted = used + 1 + word.chars().count();
        if wanted > width {
            break;
        }
        used = wanted;
        first += 1;
    }
    let rest: Vec<String> = words[first..].to_vec();
    let second = rest.join(" ");
    (indent + second.chars().count() <= width).then_some((first, rest))
}

/// A compaction event in the journal, on two lines at most, cut between words like a message (decision of 2026-10-11):
/// ` 14:32  ⟳ dev-rendu compacté auto · 74 → 9 %`, dimmed but for the name, or, short of room,
/// ` 14:32  ⟳ dev-rendu compacté` then `          auto · 74 → 9 %`; an automatic one that failed,
/// ` 14:32  ✗ dev-rendu compaction auto ratée · why`, in orange, as `compaction` then `auto ratée · API…`. Words go
/// only past two lines: the figures first, then « auto », never in the middle of one.
fn compaction_lines(s: &Snapshot, time: &str, c: &Compacted, width: usize) -> Vec<String> {
    let piece = |text: &str, paint: Paint| (text.to_string(), paint);
    let indent = 1 + time.chars().count() + 2 + 2;
    let orange = Paint::Color(look::palette().orange);
    let (sign, sign_paint, name_paint, word_paint) = match &c.failed {
        Some(_) => ("✗ ", orange, Paint::Bold(look::palette().orange), orange),
        None => ("⟳ ", Paint::Dim, Paint::Bold(member_color(s, &c.member)), Paint::Dim),
    };
    let head_pieces = vec![
        piece(" ", Paint::Plain),
        piece(time, Paint::Dim),
        piece("  ", Paint::Plain),
        piece(sign, sign_paint),
        piece(&c.member, name_paint),
    ];
    let head = indent + c.member.chars().count();
    // What it says, word by word, the richest first.
    let figures = match (c.before, c.after) {
        (Some(before), Some(after)) => Some(t!("· {} → {} %", "· {} → {}%", before, after)),
        (Some(before), None) => Some(t!("· {} %", "· {}%", before)),
        _ => None,
    };
    let words = |text: &str| -> Vec<String> { text.split_whitespace().map(String::from).collect() };
    let candidates: Vec<Vec<String>> = match &c.failed {
        None => {
            let whole = if c.auto { t!("compacté auto", "auto-compacted") } else { t!("compacté", "compacted") };
            let mut all = words(&whole);
            let mut sets = Vec::new();
            if let Some(figures) = figures {
                // The figures stay together: « · 74 → 9 % » is one word to the layout.
                all.push(figures);
                sets.push(all.clone());
                all.pop();
            }
            sets.extend([all, words(&t!("compacté", "compacted")), Vec::new()]);
            sets
        }
        Some(error) => {
            let label = words(&t!("compaction auto ratée", "auto-compaction failed"));
            let short = words(&t!("compaction ratée", "compaction failed"));
            let why = error.lines().next().unwrap_or_default();
            // The reason takes what is left of the last line, cut between words; none when too little remains.
            let with_why = |label: &Vec<String>| {
                let (first, rest) = lay_out(head, indent, label, width)?;
                let last = if rest.is_empty() {
                    head + label[..first].iter().map(|w| 1 + w.chars().count()).sum::<usize>()
                } else {
                    indent + rest.join(" ").chars().count()
                };
                let room = width.saturating_sub(last + 3);
                // No whole word of it fits: no reason, rather than half of one.
                let why = fit_words(why, room).filter(|_| room >= 6 && !why.is_empty())?;
                let mut all = label.clone();
                all.push(format!("· {why}"));
                Some(all)
            };
            [with_why(&label), Some(label.clone()), Some(short), Some(Vec::new())].into_iter().flatten().collect()
        }
    };
    for candidate in &candidates {
        if let Some((first, rest)) = lay_out(head, indent, candidate, width) {
            let text = |words: &[String]| words.iter().map(|w| format!(" {w}")).collect::<String>();
            let mut first_line = head_pieces.clone();
            first_line.push(piece(&text(&candidate[..first]), word_paint));
            let mut lines = vec![painted(&first_line, width).printed()];
            if !rest.is_empty() {
                let second = vec![piece(&" ".repeat(indent), Paint::Plain), piece(&rest.join(" "), word_paint)];
                lines.push(painted(&second, width).printed());
            }
            return lines;
        }
    }
    // Not even the head holds: the name, cut.
    vec![painted(&head_pieces, width).printed()]
}

/// What the journal wrote, kept to write again when the pane changes width: its lines are cut to the width they were
/// written at, and a pane that was narrower for a moment (before the multiplexer sized it) would keep them short.
#[derive(Debug, Clone)]
enum Written {
    Message { time: String, from: String, to: String, text: String },
    Compacted { time: String, compaction: Compacted },
}

/// How many entries the journal keeps to write again.
const KEPT: usize = 2 * BACKLOG;

/// `written` as the journal writes it in a pane `width` wide: a message on two lines, a compaction on one.
fn write_entry(out: &mut impl Write, s: &Snapshot, written: &Written, width: usize) -> std::io::Result<()> {
    match written {
        Written::Message { time, from, to, text } => {
            writeln!(out, "{}", message_head(s, time, from, to, width))?;
            writeln!(out, "{}{}", "        ╰─ ".dark_grey(), fit(text, width.saturating_sub(11)))
        }
        Written::Compacted { time, compaction } => {
            for line in compaction_lines(s, time, compaction, width) {
                writeln!(out, "{line}")?;
            }
            Ok(())
        }
    }
}

fn journal(mut s: Snapshot, state: &Path) -> Result<()> {
    let mut out = std::io::stdout();
    queue!(out, cursor::Hide)?;
    // What is typed there shows nowhere either (an Enter would leave an empty line between two messages).
    let _quiet = quiet_input(Arc::new(AtomicBool::new(false)));
    let mut tails: HashMap<PathBuf, Tail> = HashMap::new();
    let mut printed: HashSet<String> = HashSet::new();
    let mut history: VecDeque<Written> = VecDeque::new();
    let mut at_width: Option<usize> = None;
    let mut first = true;
    loop {
        reread(&mut s, state);
        // As the dashboard last saw them while it looks: `claude agents` asked once for the team.
        let sessions = fresh_states(state).and_then(Sessions::of).unwrap_or_else(|| sessions(&s, None));
        let mut fresh = Vec::new();
        for (name, session) in &sessions.members {
            let Some(id) = &session.session_id else { continue };
            let file = claude::transcript(&s.dir, s.config_dir.as_deref(), id);
            for line in tails.entry(file.clone()).or_default().read(&file) {
                fresh.extend(messages(name, &line).into_iter().map(Entry::Message));
            }
        }
        // Their compactions, among the messages by the time they were made.
        let now = now();
        for member in &s.members {
            fresh.extend(compactions(state, &member.name, now, &mut printed).into_iter().map(Entry::Compacted));
        }
        fresh.sort_by_key(Entry::at);
        if first {
            fresh.drain(..fresh.len().saturating_sub(BACKLOG));
            first = false;
        }
        let width = terminal::size().map_or(80, |(w, _)| w as usize);
        // The pane changed width: all of what is kept again, at the new one, over a clean screen.
        if at_width.is_some_and(|before| before != width) {
            write!(out, "\x1b[H\x1b[2J\x1b[3J")?;
            for written in &history {
                write_entry(&mut out, &s, written, width)?;
            }
        }
        at_width = Some(width);
        for entry in fresh {
            let time = clock(entry.at()).chars().take(5).collect::<String>();
            let written = match entry {
                Entry::Message(m) => {
                    Written::Message { time, from: m.from, to: recipient(&m.to, &sessions.by_pid), text: m.text }
                }
                Entry::Compacted(compaction) => Written::Compacted { time, compaction },
            };
            write_entry(&mut out, &s, &written, width)?;
            history.push_back(written);
            if history.len() > KEPT {
                history.pop_front();
            }
        }
        out.flush()?;
        std::thread::sleep(TICK);
    }
}

/// Between a message's sender and its recipient in the journal.
const ARROW: &str = "──▶";

/// A message's first line in the journal, ` 14:32  dev-cli ──▶ coordinateur`, cut to `width` rather than wrapped:
/// a click reaches only the line it lands on, and a name split over two would read as another one.
fn message_head(s: &Snapshot, time: &str, from: &str, to: &str, width: usize) -> String {
    let (sender, recipient) = (member_color(s, from), member_color(s, to));
    let pieces = [
        (" ", Paint::Plain),
        (time, Paint::Dim),
        ("  ", Paint::Plain),
        (from, Paint::Bold(sender)),
        (" ", Paint::Plain),
        (ARROW, Paint::Dim),
        (" ", Paint::Plain),
        (to, Paint::Color(recipient)),
    ];
    painted(&pieces, width).printed()
}

/// The name under column `x` on a message's first line in the journal, ` 14:32  dev-cli ──▶ coordinateur`: its
/// sender or its recipient, nothing elsewhere nor on any other line. Columns counted in characters, as the journal
/// writes them. A recipient that reaches the last of the pane's `columns` may go on over the next line: the pane's
/// terminal wraps again the lines already written when it narrows, `dev-cli` then reads `dev`. It is no one's.
fn journal_name_at(line: &str, x: usize, columns: usize) -> Option<&str> {
    let (time, rest) = line.strip_prefix(' ')?.split_at_checked(5)?;
    let clock = time.bytes().enumerate().all(|(i, b)| if i == 2 { b == b':' } else { b.is_ascii_digit() });
    let rest = rest.strip_prefix("  ").filter(|_| clock)?;
    let width = |text: &str| text.chars().count();
    let from_at = 1 + 5 + 2;
    // A compaction: ` 14:32  ⟳ dev-rendu compacté auto · 74 → 9 %`, its name alone; or a failed one, with ✗.
    if let Some(after) = rest.strip_prefix("⟳ ").or_else(|| rest.strip_prefix("✗ ")) {
        let name = after.split(' ').next().filter(|name| !name.is_empty())?;
        let at = from_at + 2;
        return (at..at + width(name)).contains(&x).then_some(name);
    }
    let (from, to) = rest.split_once(&format!(" {ARROW} "))?;
    let to_at = from_at + width(from) + 1 + width(ARROW) + 1;
    let to = to.trim_end();
    let to = (to_at + width(to) < columns).then_some((to_at, to));
    [Some((from_at, from)), to]
        .into_iter()
        .flatten()
        .find(|(at, name)| (*at..at + width(name)).contains(&x))
        .map(|(_, name)| name)
}

/// The member under a click on a panel, at column `x` and row `y` of its pane from 0, the pane `columns` wide, `line`
/// the clicked line as the pane shows it: on the dashboard, a card or a name in a list, where the dashboard last drew
/// them; in the journal, the sender or the recipient on a message's first line, read from `line` alone (it may be
/// scrolled back).
pub fn member_at(
    s: &Snapshot,
    state: &Path,
    panel: Kind,
    x: usize,
    y: usize,
    columns: usize,
    line: &str,
) -> Option<String> {
    let name = match panel {
        Kind::Dashboard => {
            zones(state).into_iter().find(|zone| zone.kind == ZoneKind::Member && zone.contains(x, y))?.member
        }
        Kind::Journal => journal_name_at(line, x, columns)?.to_string(),
        Kind::Toggle => return None,
    };
    s.members.iter().any(|m| m.name == name).then_some(name)
}

/// The member at rest whose context a click on the dashboard falls on, at column `x` and row `y` of its pane: the
/// click offers to compact it. Asked before `member_at`, whose card holds the same place.
pub fn compaction_at(s: &Snapshot, state: &Path, x: usize, y: usize) -> Option<String> {
    let name = zones(state).into_iter().find(|zone| zone.kind == ZoneKind::Compact && zone.contains(x, y))?.member;
    s.members.iter().any(|m| m.name == name).then_some(name)
}

/// What a click on a panel reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clicked {
    /// A member: its pane, in its tab.
    Show(String),
    /// A member at rest, by its context on the dashboard: compacting it is offered, after confirmation.
    Compact(String),
}

/// What a member's header shows in recruit's own multiplexer (step 3, F1), as its card on the dashboard shows it.
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
/// the dashboard looks, so that a dashboard stopped leaves no state frozen in the headers). Reads a few small files: for a thread
/// of the server, every two seconds, never on the way to a frame. Empty if the team cannot be read.
pub(crate) fn looks(state: &Path) -> BTreeMap<String, MemberLook> {
    let Ok(snapshot) = Snapshot::read(state) else { return BTreeMap::new() };
    let states = read_states(state);
    let fresh = states.as_ref().is_some_and(|s| s.fresh(now()));
    snapshot
        .members
        .iter()
        .map(|member| {
            let report = bridge::report(state, &member.name).unwrap_or_default();
            let (model, effort) = bridge::model_and_effort_in(&report, member);
            let context = report.context.as_ref();
            let percent =
                context.and_then(|c| c.percent).filter(|p| p.is_finite()).map(|p| p.round().clamp(0.0, 100.0) as u8);
            let seen = states.as_ref().and_then(|s| s.members.get(&member.name));
            let state_seen = seen.filter(|_| fresh);
            let card = states.as_ref().filter(|_| fresh).and_then(|s| s.cards.get(&member.name));
            let look = MemberLook {
                model: model.map(|m| family(&m)),
                effort: effort.filter(|e| bridge::EFFORTS.contains(&e.as_str())),
                context: percent,
                pressure: Pressure::of(context.and_then(|c| c.tokens), report.compacts_at()),
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

/// What a click on a panel reaches, as recruit's own multiplexer (the server) asks: see [`compaction_at`], asked
/// first, then [`member_at`].
pub fn clicked(
    s: &Snapshot,
    state: &Path,
    panel: Kind,
    x: usize,
    y: usize,
    columns: usize,
    line: &str,
) -> Option<Clicked> {
    if panel == Kind::Dashboard
        && let Some(member) = compaction_at(s, state, x, y)
    {
        return Some(Clicked::Compact(member));
    }
    member_at(s, state, panel, x, y, columns, line).map(Clicked::Show)
}

/// Takes the journal to its next size: full, reduced, hidden, full again.
pub fn toggle(s: &Snapshot) -> Result<backend::JournalSize> {
    let journal = Pane {
        member: journal_title(s.lang).to_string(),
        role: Some(backend::JOURNAL),
        argv: s.journal.clone(),
        env: Vec::new(),
    };
    backend::of(s).toggle_journal(&s.session, &s.dir, &journal)
}

/// `text` cut to `width` characters, an ellipsis marking the cut.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    if width > 0 {
        cut.push('…');
    }
    cut
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Seconds since the epoch of an ISO 8601 time: `2026-10-07T10:28:51.123Z`, or with an offset (`+02:00`).
pub fn iso_epoch(text: &str) -> Option<i64> {
    let (date, time) = text.trim().split_once('T')?;
    let mut parts = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    let (time, offset) = match time.find(['Z', '+', '-']) {
        Some(at) if &time[at..] == "Z" => (&time[..at], 0),
        Some(at) => {
            let sign = if time[at..].starts_with('-') { -1 } else { 1 };
            let (h, m) = time[at + 1..].split_once(':')?;
            (&time[..at], sign * (h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60))
        }
        None => (time, 0),
    };
    let mut clock = time.splitn(3, ':');
    let hours: i64 = clock.next()?.parse().ok()?;
    let minutes: i64 = clock.next()?.parse().ok()?;
    let seconds: f64 = clock.next()?.parse().ok()?;
    Some(days_from_civil(year, month, day) * 86_400 + hours * 3600 + minutes * 60 + seconds as i64 - offset)
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `HH:MM:SS` in local time.
// libc marks musl's `time_t` deprecated over its size on 32-bit targets; it is 64 bits on the ones recruit ships.
#[allow(deprecated)]
fn clock(epoch: i64) -> String {
    let time = epoch as libc::time_t;
    // SAFETY: localtime_r only writes the `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::ALT;

    trait Printed {
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
    fn visible(line: &(impl Printed + ?Sized)) -> String {
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

    #[test]
    fn iso_times() {
        assert_eq!(iso_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_epoch("2026-10-07T10:28:51.123Z"), Some(1_791_368_931));
        assert_eq!(iso_epoch("2026-10-07T12:28:51+02:00"), Some(1_791_368_931));
        assert_eq!(iso_epoch("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(iso_epoch("hier"), None);
    }

    #[test]
    fn sent_messages() {
        let line = r#"{"type":"assistant","timestamp":"2026-10-07T10:28:51.123Z","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"SendMessage","input":{"to":"dev","summary":"API prête","message":"L'API est prête.\nDétails…"}},{"type":"tool_use","name":"SendMessage","input":{"to":"uds:/tmp/cc-socks/42.sock","message":"Bien reçu\nsuite"}}]}}"#;
        let found = messages("lead", line);
        assert_eq!(
            found[0],
            Message { at: 1_791_368_931, from: "lead".into(), to: "dev".into(), text: "API prête".into() }
        );
        assert_eq!(found[1].text, "Bien reçu");
        let by_pid = HashMap::from([(42, "coordinateur".to_string())]);
        assert_eq!(recipient(&found[1].to, &by_pid), "coordinateur");
        assert_eq!(recipient("dev", &by_pid), "dev");
        // The session meant, among those of one name: the name alone, the member's color and its click.
        assert_eq!(recipient("coordinateur [ceeb11]", &by_pid), "coordinateur");
        assert_eq!(recipient("coordinateur [x]", &by_pid), "coordinateur [x]");
        assert_eq!(recipient("[ceeb11]", &by_pid), "[ceeb11]");
        assert!(messages("lead", r#"{"type":"user","message":"SendMessage"}"#).is_empty());
        // Printed as it is, so no control character: no escape sequence, no clipboard write (OSC 52).
        let trapped = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "SendMessage",
            "input": {"to": "dev\u{1b}[31m", "message": "Fini\u{1b}]52;c;cm0gLXJmIH4=\u{7} \u{9b}2Jvoilà"}}]}});
        let found = messages("lead", &trapped.to_string());
        assert_eq!((found[0].to.as_str(), found[0].text.as_str()), ("dev[31m", "Fini]52;c;cm0gLXJmIH4= 2Jvoilà"));
        // Nor formatting: a text turned right to left, invisible characters.
        let turned = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "SendMessage",
            "input": {"to": "d\u{200b}ev", "message": "\u{202e}fdp.exe\u{2066} prêt"}}]}});
        let found = messages("lead", &turned.to_string());
        assert_eq!((found[0].to.as_str(), found[0].text.as_str()), ("dev", "fdp.exe prêt"));
    }

    #[test]
    fn tails_read_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.jsonl");
        fs::write(&file, "a\nb").unwrap();
        let mut tail = Tail::default();
        assert_eq!(tail.read(&file), ["a"]);
        fs::write(&file, "a\nbc\nd\n").unwrap();
        assert_eq!(tail.read(&file), ["bc", "d"]);
        assert!(tail.read(&file).is_empty());
        fs::write(&file, "x\n").unwrap();
        assert_eq!(tail.read(&file), ["x"]);
    }

    fn sample_card(name: &str, contact: bool, state: State) -> Card {
        Card {
            name: name.into(),
            color: Color::Cyan,
            contact,
            state,
            since: 90,
            context: Some(41.0),
            pressure: Pressure::Calm,
            model: Some("Opus".into()),
            effort: Some("xhigh".into()),
            doing: None,
            samples: (0..600).map(|i| (1000 - i, i % 3 == 0)).collect(),
            work: None,
            helpers: Vec::new(),
            shell: false,
            failed: None,
            compacted: None,
        }
    }

    /// The cards the server draws for the focus layout are the dashboard's own: the same lines, lined up the same way.
    #[test]
    fn the_cards_for_others_are_the_dashboards_own() {
        // Oldest first, as the dashboard keeps them.
        let samples = || -> Vec<(i64, bool)> { (0..600).rev().map(|i| (1000 - i, i % 3 == 0)).collect() };
        let data = |name: &str, model: &str, state| CardData {
            name: name.into(),
            color: Color::Cyan,
            state,
            since: 90,
            shell: false,
            context: Some(41.0),
            pressure: Pressure::Calm,
            model: Some(model.into()),
            effort: Some("xhigh".into()),
            doing: Some(Task { now: "Je relis la maquette des cartes".into(), done: None }),
            activity: Activity::of(&samples()),
            failed: None,
            compacted: None,
        };
        let list = [data("dev-mod", "Opus", State::Working), data("ops", "Haiku", State::Idle)];
        let list: Vec<&CardData> = list.iter().collect();
        let clock = Clock { now: 1000, frame: 3, glyphs: Glyphs::Unicode };
        let drawn = card_lines(&list, 44, 2, clock);
        // The dashboard's card for the first, with the second beside it for the columns.
        let cards: Vec<Card> = list.iter().map(|data| data.card()).collect();
        let right = Right::of(cards.iter().map(|c| (c, 44)));
        let board = Board { now: 1000, frame: 3, glyphs: Glyphs::Unicode, ..Board::default() };
        assert_eq!(drawn[0], card(&cards[0], 44, 2, &right, &board));
        let text: Vec<String> = drawn[0].iter().map(Line::text).collect();
        assert!(
            text[0].contains("dev-mod") && text[0].contains("Opus") && text[0].contains(&look::effort_label("xhigh")),
            "{text:?}"
        );
        assert!(text[1].contains("Je relis la maquette"), "{text:?}");
        assert!(text.last().unwrap().contains("41 %"), "{text:?}");
        // The columns line up from one card to the next: the second one's effort starts where the first's does.
        let at = |lines: &[Line]| lines[0].text().find(&look::effort_label("xhigh"));
        assert_eq!(drawn[1][0].text().chars().count(), drawn[0][0].text().chars().count());
        assert!(at(&drawn[0]).is_some() && at(&drawn[1]).is_some());
        // The short form: its title and its curve.
        let short = card_lines(&list, 44, 0, clock);
        assert_eq!(short[0].len(), 2);
        assert_eq!(card_rows(list[0], 44, 0), 2);
        assert_eq!(card_rows(list[0], 44, 2), drawn[0].len());
        // Its curve from the stretches is the one the samples give, cell by cell, at any width.
        for width in [1, 10, 37, 60, 100] {
            assert_eq!(Activity::of(&samples()).curve(1000, width), curve(&samples(), 1000, width), "{width} cells");
        }
        assert_eq!(Activity::default().curve(1000, 5), "     ");
        // A line on a canvas, cut where it must.
        let mut canvas = Canvas::new(30, 2);
        let end = drawn[0][0].draw_at(&mut canvas, 2, 0, 20);
        assert_eq!(end, 20);
        assert!(canvas.row(0).starts_with("  ╻ "), "{}", canvas.row(0));
    }

    fn board(cards: Vec<Card>) -> Board {
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
    fn capture() -> Vec<Card> {
        let task = |now: &str, done: &str| Some(Task { now: now.into(), done: Some(done.into()) });
        let at = |name: &str, state, since, doing| Card { since, doing, ..sample_card(name, false, state) };
        let mut cards = vec![
            Card {
                doing: task("Je répartis les tâches", "Tâches réparties"),
                ..sample_card("coordinateur", true, State::Idle)
            },
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
    fn padded(b: &Board, width: usize, height: usize) -> Drawn {
        let mut drawn = Drawn::from(vec![Line::default(); 2.min(height.saturating_sub(1))]);
        drawn.append(render(b, width, height.saturating_sub(2)));
        drawn
    }

    fn checked(b: &Board, width: usize, height: usize) -> Vec<String> {
        let drawn = padded(b, width, height);
        zones_match(&drawn);
        let lines: Vec<String> = drawn.lines.iter().map(Line::text).collect();
        assert!(lines.len() == height - 1 && lines.iter().all(|l| l.chars().count() <= width), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.trim_end().ends_with('┃')), "{lines:#?}");
        lines
    }

    /// Where a member's card starts, if it has one.
    fn title(lines: &[String], name: &str) -> Option<usize> {
        lines.iter().position(|l| l.contains(&format!(" {name} ")) && l.contains('╻'))
    }

    /// What a member's card says it does, line by line, its capsule first.
    fn task(lines: &[String], name: &str) -> Vec<String> {
        let top = title(lines, name).unwrap();
        let col = lines[top].find(&format!(" {name} ")).unwrap();
        let col = lines[top][..col].chars().count() - 3;
        let column = |l: &String| l.chars().skip(col).collect::<String>();
        let middles = lines[top + 1..].iter().map(column).take_while(|l| l.starts_with('┃'));
        // The card on the right of a pair ends at its own width.
        middles.map(|l| l.split(" ┃ ").next().unwrap_or_default().trim_end().to_string()).collect()
    }

    /// The agents at rest only named, as they show.
    fn named(lines: &[String]) -> Vec<String> {
        let rest = t!("au repos", "idle");
        let from = lines.iter().position(|l| l.starts_with(&rest)).unwrap_or(lines.len());
        let list = lines[from..].iter().take_while(|l| !l.is_empty()).map(|l| l.trim_start_matches(&rest));
        let list = list.map(|l| l.split(" … ").next().unwrap_or_default().to_string());
        list.flat_map(|l| l.split(" · ").map(|n| n.trim().to_string()).collect::<Vec<_>>()).collect()
    }

    #[test]
    fn capture_at_several_heights() {
        let at = |height: usize| checked(&board(capture()), 60, height);
        let gpu = ["┃   Je profile le rendu des tuiles et je corrige la fuite", "┃   du cache de textures"];
        // Those who just finished: across the pane, what they did whole.
        let finished = |lines: &[String]| {
            assert_eq!(task(lines, "designer"), ["┃   ✓ Maquette des cartes livrée"], "{lines:#?}");
            assert_eq!(task(lines, "reviewer"), ["┃   ✓ Relecture de la PR 42 terminée"], "{lines:#?}");
        };

        // The contact, the two at work across the pane, their task over two lines, uncut; then those at rest, the
        // latest first; the three oldest only named.
        let lines = at(44);
        let order = ["coordinateur", "gpu-engine", "dev-back", "designer", "reviewer", "devops", "sécurité", "ui-ux"];
        let tops: Vec<usize> = order.iter().map(|name| title(&lines, name).unwrap()).collect();
        assert!(tops.windows(2).all(|w| w[0] <= w[1]) && tops[0] == 2, "{lines:#?}");
        assert_eq!(task(&lines, "gpu-engine"), gpu);
        finished(&lines);
        // Beside one another as they come, the same height or not.
        assert_eq!(title(&lines, "performance"), title(&lines, "ui-ux"));
        assert_eq!(task(&lines, "ui-ux"), ["┃   ✓ Écrans relus"]);
        assert_eq!(named(&lines), ["opérateur-ui-ux", "opérateur", "planificateur"]);

        // Less room: fewer cards at rest, those who just finished keep theirs whole.
        for height in [38, 34, 30] {
            let lines = at(height);
            assert_eq!(task(&lines, "gpu-engine"), gpu);
            finished(&lines);
        }
        // The next ones side by side, what they did cut rather than left out.
        let lines = at(34);
        assert_eq!(title(&lines, "devops"), title(&lines, "traitement-cartes"));
        assert_eq!(task(&lines, "traitement-cartes"), ["┃   ✓ Cartes découpées en tu…"]);
        assert_eq!(named(&lines).len(), 6);
        // Named on one line, the others counted, rather than a card less.
        let lines = at(30);
        // As many names as the line holds, in either language: the others make eight.
        let shown = named(&lines);
        assert_eq!(shown[..2], ["devops", "traitement-cartes"]);
        let line = format!("{:<10}{} {}", t!("au repos", "idle"), shown.join(" · "), others(8 - shown.len()));
        assert!(lines.contains(&line), "{lines:#?}");

        // Those who just finished side by side, without what they did, before the ones at work give anything up.
        let lines = at(24);
        assert_eq!(task(&lines, "gpu-engine"), gpu);
        let pair = &lines[title(&lines, "designer").unwrap()];
        assert!(pair.contains(" reviewer ") && task(&lines, "designer").is_empty(), "{lines:#?}");

        // Then the ones at work side by side, their task on one line; the contact still across the pane.
        let lines = at(18);
        let pair = &lines[title(&lines, "gpu-engine").unwrap()];
        assert!(pair.contains(" dev-back ") && pair.matches('╻').count() == 2, "{lines:#?}");
        assert_eq!(task(&lines, "coordinateur"), ["┃   ✓ Tâches réparties"]);

        // At last, the contact alone, what it did whole, and how many more.
        let lines = at(12);
        assert_eq!(task(&lines, "coordinateur"), ["┃   ✓ Tâches réparties"]);
        assert!(lines.contains(&t!("… et {} de plus", "… and {} more", 12)), "{lines:#?}");
        assert!(named(&lines).is_empty());
    }

    #[test]
    fn a_contact_across_rather_than_two_cut() {
        // Two contacts, 6 lines for them: the one at work across the pane, its task on one line; the other without.
        let cards = vec![
            Card {
                doing: Some(Task {
                    now: "Je relis la maquette des cartes et je corrige les colonnes de droite".into(),
                    done: None,
                }),
                ..sample_card("coordinateur", true, State::Working)
            },
            Card {
                doing: Some(Task { now: String::new(), done: Some("Plan relu".into()) }),
                ..sample_card("architecte", true, State::Idle)
            },
        ];
        let lines = checked(&Board { absent: Vec::new(), ..board(cards) }, 60, 11);
        assert!(lines.iter().all(|l| l.matches('╻').count() <= 1), "{lines:#?}");
        assert_eq!(task(&lines, "coordinateur"), ["┃   Je relis la maquette des cartes et je corrige les colo…"]);
        assert!(task(&lines, "architecte").is_empty());
        assert_eq!(lines[2..8].iter().filter(|l| l.is_empty()).count(), 1, "{lines:#?}");
    }

    #[test]
    fn too_low_a_pane() {
        // Two rows more than with the header it had: from six, « … et 13 de plus » holds.
        for height in 0..=5 {
            let lines = render(&board(capture()), 60, height).lines;
            assert!(lines.len() <= height.saturating_sub(1), "{height}: {lines:#?}");
            assert!(!lines.iter().any(|l| visible(l).starts_with('…')), "{height}: {lines:#?}");
        }
        // The usage at the bottom only.
        let lines: Vec<String> = render(&board(capture()), 60, 3).lines.iter().map(Line::text).collect();
        assert!(lines[0].is_empty() && lines[1].contains("41 % ↻ 1h22") && lines.len() == 2, "{lines:#?}");
        assert!(render(&board(capture()), 60, 3).zones.is_empty());
        // Room for how many more only.
        let lines: Vec<String> =
            padded(&Board { absent: Vec::new(), ..board(capture()) }, 60, 6).lines.iter().map(Line::text).collect();
        assert_eq!(lines[2], t!("… et {} de plus", "… and {} more", 13));
    }

    #[test]
    fn names_on_one_line() {
        let list: Vec<String> =
            ["devops", "traitement-cartes", "sécurité", "performance", "ui-ux"].map(String::from).into();
        let drawn = names("au repos", &list, 50, Color::DarkGrey, 1);
        let lines: Vec<String> = drawn.lines.iter().map(Line::text).collect();
        assert_eq!(lines, [format!("au repos  devops · traitement-cartes {}", others(3))]);
        // The names shown are zones, the others are not.
        zones_match(&drawn);
        assert_eq!(drawn.zones.iter().map(|z| z.member.as_str()).collect::<Vec<_>>(), ["devops", "traitement-cartes"]);
        // One left over; none that holds.
        let lines = |width| {
            names("au repos", &list[..2], width, Color::DarkGrey, 1).lines.iter().map(Line::text).collect::<Vec<_>>()
        };
        assert_eq!(lines(30), [format!("au repos  devops {}", others(1))]);
        assert_eq!(lines(20), [format!("au repos  {}", others(2))]);
        // As many lines as it takes.
        assert_eq!(names("au repos", &list, 50, Color::DarkGrey, usize::MAX).len(), 2);
    }

    #[test]
    fn same_layout_from_one_second_to_the_next() {
        let later = |mut b: Board| {
            b.now += 1;
            b.cards.iter_mut().for_each(|c| c.since += 1);
            b
        };
        for height in [60, 44, 36, 30, 24, 18, 12] {
            let now = render(&board(capture()), 60, height);
            assert_eq!(now.zones, render(&later(board(capture())), 60, height).zones, "{height}");
        }
        // Two at rest since the same second: as they come, the latest first (see `Memory::board`).
        let cards = vec![
            Card { since: 30, ..sample_card("b", false, State::Idle) },
            Card { since: 30, ..sample_card("a", false, State::Idle) },
            Card { since: 10, ..sample_card("c", false, State::Idle) },
            sample_card("w", false, State::Working),
            sample_card("v", false, State::Waiting),
            sample_card("lead", true, State::Idle),
        ];
        let names: Vec<&str> = ordered(&cards).iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["lead", "v", "w", "c", "b", "a"]);
    }

    #[test]
    fn small_team_full_width() {
        let b = board(vec![
            sample_card("coordinateur", true, State::Working),
            sample_card("lanceur", false, State::Idle),
            sample_card("interface", false, State::Waiting),
        ]);
        let lines: Vec<String> = padded(&b, 70, 23).lines.iter().map(Line::text).collect();
        assert_eq!(lines.len(), 22);
        assert!(lines.iter().all(|l| l.chars().count() <= 70), "{lines:#?}");
        let tops: Vec<&String> = lines.iter().filter(|l| l.starts_with('╻')).collect();
        assert_eq!(tops.len(), 3);
        assert!(tops.iter().all(|l| l.chars().count() == 70));
        // The cards right under the header, on two lines, an empty one between two: the contact, the agent waiting
        // for the user, the one at rest; the usage at the bottom.
        assert!(
            lines[2].starts_with("╻ ⠋ coordinateur ")
                && lines[2].ends_with(&format!(" Opus {}   1m ", look::effort_label("xhigh")))
        );
        assert!(lines[3].starts_with("╹   ") && lines[3].ends_with("   41 % "));
        assert!(lines[4].is_empty() && lines[5].starts_with("╻ ⚑ interface "));
        assert!(lines[7].is_empty() && lines[8].starts_with("╻ ◷ lanceur "));
        // At rest, the context offers to compact.
        assert!(lines[9].ends_with(" ⟳ 41 % "));
        assert!(lines[21].contains("41 % ↻ 1h22") && lines[20].is_empty());
        assert!(lines.iter().any(|l| l.contains("ops")));
    }

    #[test]
    fn what_they_do_on_each_card() {
        let doing = |name: &str, state, now: &str, done: Option<&str>| Card {
            doing: Some(Task { now: now.into(), done: done.map(String::from) }),
            ..sample_card(name, false, state)
        };
        let cards = vec![
            doing("coordinateur", State::Working, "Je relis la maquette des cartes", Some("Maquette relue")),
            doing("dev", State::Idle, "Je publie la version 0.6.2", Some("Version 0.6.2 publiée")),
            sample_card("ops", false, State::Waiting),
        ];
        let lines: Vec<String> = padded(&board(cards.clone()), 50, 23).lines.iter().map(Line::text).collect();
        // The one waiting for the user first; nothing said yet, no line for it.
        assert!(lines[2].starts_with("╻ ⚑ ops ") && lines[3].starts_with("╹ "));
        assert!(lines[5].starts_with("╻ ⠋ coordinateur "));
        assert_eq!(lines[6].trim_end(), "┃   Je relis la maquette des cartes");
        assert!(lines[7].starts_with("╹   ") && lines[8].is_empty());
        // At rest, the last task, done.
        assert_eq!(lines[10].trim_end(), "┃   ✓ Version 0.6.2 publiée");
        assert!(!lines.iter().any(|l| l.trim_end() == "┃"), "{lines:#?}");
        // In a state the dashboard does not know: the task as it was, faded, nothing done.
        let unknown = doing("ops", State::Other, "Je publie la version 0.6.2", Some("Version 0.6.2 publiée"));
        let right = Right { model: 4, initial: false, effort: 8, time: 2 };
        let line = card(&unknown, 50, 1, &right, &board(Vec::new()))[1].clone();
        assert_eq!(visible(&line).trim_end(), "┃   Je publie la version 0.6.2");
        assert!(line.printed().contains(&"Je publie la version 0.6.2".dim().to_string()));
        // The model gave no form for it done: as it was.
        let undone = doing("review", State::Idle, "Je relis le clic", None);
        let right = Right { model: 4, initial: false, effort: 8, time: 2 };
        assert_eq!(visible(&card(&undone, 50, 1, &right, &board(Vec::new()))[1]).trim_end(), "┃   ✓ Je relis le clic");
        // Short of room, the agent at rest goes first, into the list of names; the one at work keeps its task.
        let lines: Vec<String> = padded(&board(cards), 50, 15).lines.iter().map(Line::text).collect();
        assert!(lines.iter().any(|l| l.trim_end() == "┃   Je relis la maquette des cartes"), "{lines:#?}");
        assert_eq!(lines.iter().filter(|l| l.starts_with('╻')).count(), 2);
        assert!(lines.iter().any(|l| l.starts_with(&t!("au repos", "idle")) && l.ends_with(" dev")), "{lines:#?}");
    }

    #[test]
    fn wrapped_between_words() {
        let text = "Je publie la version 0.6.2 de recruit";
        assert_eq!(wrap(text, 20, 2), ["Je publie la version", "0.6.2 de recruit"]);
        assert_eq!(wrap(text, 40, 2), [text]);
        // On one line, cut.
        assert_eq!(wrap(text, 20, 1), ["Je publie la versio…"]);
        // Past two lines, the second ends with an ellipsis.
        assert_eq!(wrap("un deux trois quatre cinq six sept huit", 10, 2), ["un deux", "trois qua…"]);
        // A word longer than a line, cut where it must.
        assert_eq!(wrap("abcdefghij", 6, 2), ["abcdef", "ghij"]);
        assert_eq!(wrap("abcdefghijklmnop qr", 6, 2), ["abcdef", "ghijk…"]);
        assert_eq!(wrap("ok", 6, 2), ["ok"]);
        assert_eq!(wrap("", 6, 2), [""]);
        // Accents at the edge of a line: counted as characters, not bytes.
        assert_eq!(wrap("Équipe réglée près", 7, 2), ["Équipe", "réglée…"]);
        assert_eq!(wrap("é é é é", 3, 2), ["é é", "é é"]);
    }

    /// Two members: one whose task goes over two lines in one column, one at rest whose task holds in one.
    fn long_and_short() -> Vec<Card> {
        let task = |now: &str, done: Option<&str>| Some(Task { now: now.into(), done: done.map(String::from) });
        vec![
            Card {
                doing: task("Je relis la maquette des cartes et je corrige les colonnes de droite", None),
                ..sample_card("coordinateur", true, State::Working)
            },
            Card {
                doing: task("Je publie la version 0.6.2", Some("Version 0.6.2 publiée")),
                ..sample_card("dev", false, State::Idle)
            },
        ]
    }

    /// The contact at work over two lines, with what its session started: two subagents, one at work and one done,
    /// and a teammate waiting for a message; the agent at rest with its task done.
    fn with_helpers() -> Vec<Card> {
        let helper = |id: &str, kind: &str, description: &str, teammate: Option<&str>, status, started, since| Helper {
            id: id.into(),
            kind: kind.into(),
            description: description.into(),
            teammate: teammate.map(String::from),
            status,
            started,
            since,
        };
        let mut cards = long_and_short();
        cards[0].helpers = vec![
            helper("a", "Explore", "Lire le tableau de bord", None, HelperStatus::Running, 988, 988),
            helper("b", "Plan", "Découper la tâche", None, HelperStatus::Done, 900, 996),
            helper("m", "reviewer", "", Some("critic"), HelperStatus::Idle, 800, 940),
        ];
        cards
    }

    /// The agents a session started have cards of their own (2a, direction A, subagents option 2), under the members,
    /// each kind in a section under a rule; nothing is left under the member's card. A click on one leads to the
    /// member that started it.
    #[test]
    fn agents_as_cards_under_the_members() {
        let drawn = render(&board(with_helpers()), 70, 40);
        let lines: Vec<String> = drawn.lines.iter().map(Line::text).collect();
        let top = title(&lines, "coordinateur").unwrap();
        // Its card: the title, two lines of what it does, its curve; the next one is not a helper.
        assert!(lines[top + 1].starts_with("┃   Je relis") && lines[top + 2].starts_with("┃   droite"), "{lines:#?}");
        assert!(lines[top + 3].starts_with('╹'), "{lines:#?}");
        let rule = |sign: char, noun: &str, n: usize| {
            lines.iter().position(|l| l.starts_with(&format!("─ {sign} {noun} · {n} ─"))).expect(noun)
        };
        let subs = rule('⧗', &t!("sous-agents", "subagents"), 2);
        let mates = rule('♙', "teammate", 1);
        assert!(
            mates < subs && lines[subs].chars().count() == 70,
            "the teammates first, the subagents last: {lines:#?}"
        );
        // Cards in the members' shape: the name in the clear, the member that started it at the right, the time, what
        // it was asked, the curve; the subagents' type with its hourglass, ✓ once done.
        let card = |at: usize| lines[at..at + 3].iter().map(|l| l.trim_end().to_string()).collect::<Vec<_>>();
        let first = card(subs + 1);
        assert!(first[0].starts_with("╻ ⧗ Explore") && first[0].contains("⧗ coordinateur"), "{first:#?}");
        assert!(first[0].ends_with(" 12s") && first[1] == "┃   Lire le tableau de bord", "{first:#?}");
        assert!(first[2].starts_with("╹   "), "{first:#?}");
        let second = card(subs + 5);
        assert!(second[0].starts_with("╻ ✓ Plan") && second[0].ends_with(" 1m"), "{second:#?}");
        let mate = card(mates + 1);
        assert!(mate[0].contains("critic") && mate[0].contains("♙ coordinateur"), "{mate:#?}");
        // A click goes to the member that started them.
        let zone = |row: usize| drawn.zones.iter().find(|z| z.row == row && z.kind == ZoneKind::Member).unwrap();
        for row in [subs + 1, subs + 5, mates + 1] {
            assert_eq!(zone(row).member, "coordinateur", "row {row}");
            assert_eq!((zone(row).col, zone(row).rows), (0, 3));
        }
        // No agents, no sections.
        let lines: Vec<String> = render(&board(long_and_short()), 70, 40).lines.iter().map(Line::text).collect();
        assert!(!lines.iter().any(|l| l.starts_with("─ ")), "{lines:#?}");
    }

    /// As the pane gets lower, a section keeps fewer lines: cards with their text, without, two by two, only their
    /// names, only counted, left out; the members' cards never lose a line to them.
    #[test]
    fn agents_give_up_lines_before_the_members_do() {
        let mut shapes: Vec<&str> = Vec::new();
        let members = |lines: &[String]| -> Option<Vec<String>> {
            let top = title(lines, "coordinateur")?;
            Some(lines[top..top + 4].to_vec())
        };
        let without = {
            let mut cards = with_helpers();
            cards[0].helpers.clear();
            cards
        };
        for height in (12..=40).rev() {
            let lines: Vec<String> = render(&board(with_helpers()), 70, height).lines.iter().map(Line::text).collect();
            // The members are drawn as they are without any agents.
            let alone: Vec<String> = render(&board(without.clone()), 70, height).lines.iter().map(Line::text).collect();
            assert_eq!(members(&lines), members(&alone), "{height}: {lines:#?}");
            let has = |text: &str| lines.iter().any(|l| l.contains(text));
            let shape = if has("┃   Lire le tableau") {
                "text"
            } else if lines.iter().any(|l| l.starts_with("╻ ⧗ Explore")) {
                "cards"
            } else if has(&format!("⧗ {}", t!("2 sous-agents", "2 subagents"))) {
                "counted"
            } else if lines.iter().any(|l| l.starts_with("  ⧗  ")) {
                "names"
            } else {
                "none"
            };
            if shapes.last() != Some(&shape) {
                shapes.push(shape);
            }
        }
        assert_eq!(shapes[0], "text", "{shapes:?}");
        assert_eq!(shapes.last(), Some(&"none"), "{shapes:?}");
        assert!(shapes.contains(&"counted") || shapes.contains(&"names"), "{shapes:?}");
    }

    /// A scene with three subagents and three teammates, swept from a tall pane to a low one: the room fills card by
    /// card, and no section ever gets richer as the pane shrinks (2a, designer's review, 2026-10-11); the teammates keep
    /// their place longer than the subagents.
    #[test]
    fn agents_never_get_richer_as_the_pane_shrinks() {
        let agent = |id: &str, teammate: Option<&str>, status| Helper {
            id: id.into(),
            kind: if teammate.is_some() { "reviewer".into() } else { format!("SubK{id}") },
            description: format!("text-of-{id}"),
            teammate: teammate.map(String::from),
            status,
            started: 900,
            since: 950,
        };
        let mut lead = sample_card("coordinateur", true, State::Working);
        lead.helpers = vec![
            agent("1", None, HelperStatus::Running),
            agent("2", None, HelperStatus::Running),
            agent("3", None, HelperStatus::Done),
            agent("a", Some("mate-a"), HelperStatus::Running),
            agent("b", Some("mate-b"), HelperStatus::Idle),
            agent("c", Some("mate-c"), HelperStatus::Idle),
        ];
        // How much of a section shows: names alone 1, each card 10, each text 1 more.
        let score = |lines: &[String], card: &str, text: &str| -> usize {
            let cards = lines.iter().filter(|l| l.starts_with('╻') && l.contains(card)).count();
            let texts = lines.iter().filter(|l| l.contains(text)).count();
            if cards + texts > 0 { 10 * cards + texts } else { usize::from(lines.iter().any(|l| l.contains(card))) }
        };
        for width in [60, 70, 140] {
            let mut before: Option<(usize, usize, usize)> = None;
            let mut seen = Vec::new();
            for height in (10..=70).rev() {
                // One member alone: its card does not change with the height, so the room the agents get only shrinks.
                let lines: Vec<String> =
                    render(&board(vec![lead.clone()]), width, height).lines.iter().map(Line::text).collect();
                let (subs, mates) = (score(&lines, "SubK", "text-of-1"), score(&lines, "mate-", "text-of-a"));
                // The teammates' text: the three share a marker apart.
                let mates =
                    mates.max(lines.iter().filter(|l| l.contains("text-of-b") || l.contains("text-of-c")).count());
                if let Some((was_height, was_subs, was_mates)) = before {
                    assert!(
                        subs <= was_subs && mates <= was_mates,
                        "width {width}: richer at {height} than at {was_height}: subagents {was_subs} → {subs}, \
                         teammates {was_mates} → {mates}\n{lines:#?}"
                    );
                }
                before = Some((height, subs, mates));
                seen.push((subs, mates));
            }
            // From all of it down to nothing, the teammates holding on longer.
            assert!(seen.first().is_some_and(|&(s, m)| s > 10 && m > 10), "{width}: {:?}", seen.first());
            let last_mates = seen.iter().rposition(|&(_, m)| m > 0).unwrap();
            let last_subs = seen.iter().rposition(|&(s, _)| s > 0).unwrap();
            assert!(last_mates >= last_subs, "{width}: the teammates outlast the subagents");
        }
    }

    /// A list of names, then a count: `  ♙  ⠹ ux-critic  ⠹ perf  ◷ a11y`, `  ♙ 3 teammates`, in yellow when one works,
    /// in orange when one failed, else faded.
    #[test]
    fn agents_by_name_and_counted() {
        let mate =
            |name: &str, status| Helper { id: name.into(), teammate: Some(name.into()), ..helper(name, None, status) };
        let mut lead = sample_card("coordinateur", true, State::Working);
        lead.helpers = vec![
            mate("ux-critic", HelperStatus::Running),
            mate("perf", HelperStatus::Running),
            mate("a11y", HelperStatus::Idle),
        ];
        let b = board(vec![lead]);
        let [_, teammates] = agents(&b);
        let names = agent_names(&teammates, '♙', 60, &b);
        let text = names.lines[0].text();
        assert!(text.starts_with("  ♙  ⠋ perf  ⠋ ux-critic  ◷ a11y"), "{text}");
        assert_eq!(names.zones.len(), 3);
        assert!(names.zones.iter().all(|z| z.member == "coordinateur" && z.rows == 1));
        // Too narrow for all of them: the ones that hold and how many more.
        let text = agent_names(&teammates, '♙', 28, &b).lines[0].text();
        assert!(text.contains("perf") && text.ends_with(&others(2)) && !text.contains("ux-critic"), "{text}");
        let count = agent_count(&teammates, '♙', &b);
        assert_eq!(count.lines[0].text(), "  ♙ 3 teammates");
        // Each kind in its own words, in the plural from two.
        let sub = |n: usize| {
            let mut lead = sample_card("coordinateur", true, State::Working);
            lead.helpers = (0..n).map(|i| helper(&format!("s{i}"), None, HelperStatus::Running)).collect();
            let b = board(vec![lead]);
            let [subagents, _] = agents(&b);
            agent_count(&subagents, '⧗', &b).lines[0].text()
        };
        assert_eq!(sub(1), format!("  ⧗ 1 {}", t!("sous-agent", "subagent")));
        assert_eq!(sub(2), format!("  ⧗ 2 {}", t!("sous-agents", "subagents")));
        assert_eq!(count.lines[0].0[0].1, Paint::Color(State::Working.color()));
        let mut idle = teammates;
        idle.retain(|a| a.helper.status == HelperStatus::Idle);
        assert_eq!(agent_count(&idle, '♙', &b).lines[0].text(), "  ♙ 1 teammate");
        assert_eq!(agent_count(&idle, '♙', &b).lines[0].0[0].1, Paint::Dim);
    }

    /// A failure in orange, not in red: the capsule, the sign and the time.
    #[test]
    fn a_failed_subagent_in_orange() {
        let mut lead = sample_card("coordinateur", true, State::Working);
        lead.helpers = vec![Helper { since: 996, ..helper("a", None, HelperStatus::Failed) }];
        let b = board(vec![lead]);
        let [subagents, _] = agents(&b);
        let card = agent_card(&subagents[0], 50, true, 3, &b);
        let orange = State::Error.color();
        assert_eq!(card[0].0[0].1, Paint::Bold(orange));
        assert!(card[0].text().starts_with("╻ ✗ Explore"), "{}", card[0].text());
        assert_eq!(card[1].0[2].1, Paint::Color(orange));
    }

    fn helper(id: &str, teammate: Option<&str>, status: HelperStatus) -> Helper {
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

    /// The journal's lines at the pane's real width, 46 columns beside the dashboard: whole there, short only when the
    /// room is short, and written again at the new width when the pane changes it. The forms, longest first: a success
    /// « compacté auto · 74 → 9 % », « compacté auto », « compacté »; a failure « compaction auto ratée · why »,
    /// « compaction auto ratée », « compaction ratée ».
    #[test]
    fn journal_lines_at_the_width_of_the_pane() {
        let s = snapshot(&["work", "waits"]);
        let plain = |line: String| String::from_utf8_lossy(&strip_ansi(line.as_bytes())).into_owned();
        let done = |name: &str| Compacted {
            at: 1000,
            member: name.into(),
            auto: true,
            before: Some(74),
            after: Some(9),
            failed: None,
        };
        let failed = Compacted { failed: Some("API surchargée, réessaie plus tard".into()), ..done("waits") };
        let auto = t!("compacté auto", "auto-compacted");
        let figures = t!("74 → 9 %", "74 → 9%");
        // 46 columns: all of it, on one line.
        assert_eq!(
            plain(compaction_head(&s, "14:32", &done("work"), 46)),
            format!(" 14:32  ⟳ work {auto} · {figures}")
        );
        let line = plain(compaction_head(&s, "14:32", &failed, 46));
        let label = t!("compaction auto ratée", "auto-compaction failed");
        // The reason takes what the line leaves, when it is enough to say a word of it.
        assert!(line.starts_with(&format!(" 14:32  ✗ waits {label}")) && line.chars().count() <= 46, "{line:?}");
        // A little less: the words that do not hold go on a second line, under the name, nothing taken away.
        let indent = " ".repeat(10);
        let at = |width| plain(compaction_head(&s, "14:32", &done("work"), width));
        assert_eq!(at(33), format!(" 14:32  ⟳ work {auto}\n{indent}· {figures}"));
        // The journal beside the dashboard at 80 × 24: 26 columns, 17 after the time and the sign.
        assert_eq!(
            at(26),
            t!(" 14:32  ⟳ work compacté\n          auto · 74 → 9 %", " 14:32  ⟳ work\n          auto-compacted")
        );
        let fails = |width| plain(compaction_head(&s, "14:32", &failed, width));
        assert_eq!(
            fails(26),
            t!(" 14:32  ✗ waits compaction\n          auto ratée", " 14:32  ✗ waits compaction\n          failed")
        );
        // What was asked is kept as long as two lines hold: a failure never loses its object.
        for width in 26..=60 {
            for text in [fails(width), at(width)] {
                let lines: Vec<&str> = text.lines().collect();
                assert!(lines.len() <= 2 && lines.iter().all(|l| l.chars().count() <= width), "{width}: {text:?}");
            }
            assert!(fails(width).contains(&t!("compaction", "compaction")), "{width}: {}", fails(width));
        }
        // An event of two lines fits the reduced journal as a message does: its three last messages, two lines each.
        assert!(compaction_lines(&s, "14:32", &done("work"), 26).len() <= 2);
        // Written again at the width the pane gets: short at 20, whole at 46.
        let history = [
            Written::Message { time: "14:30".into(), from: "work".into(), to: "waits".into(), text: "salut".into() },
            Written::Compacted { time: "14:32".into(), compaction: done("work") },
        ];
        let at_width = |width: usize| {
            let mut out = Vec::new();
            for written in &history {
                write_entry(&mut out, &s, written, width).unwrap();
            }
            plain(String::from_utf8(out).unwrap())
        };
        assert!(!at_width(20).contains("74"), "{}", at_width(20));
        assert!(at_width(46).contains(&format!("{auto} · {figures}")), "{}", at_width(46));
    }

    #[test]
    fn helpers_only_while_the_mod_reports() {
        let report = |at| Report {
            at,
            helpers: vec![
                helper("a", None, HelperStatus::Running),
                Helper { since: 900, ..helper("b", None, HelperStatus::Done) },
            ],
            ..Default::default()
        };
        // Done long ago: no longer shown.
        assert_eq!(helpers_shown(&report(995), 1000).iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(helpers_shown(&report(1000 - FRESH_REPORT), 1000).len(), 1);
        // A report older than that: the member stopped, they went with it.
        assert!(helpers_shown(&report(1000 - FRESH_REPORT - 1), 1000).is_empty());
    }

    #[test]
    fn forgotten_screen_written_whole() {
        let b = board(vec![sample_card("dev", false, State::Idle)]);
        let mut screen = Screen::default();
        let mut out = Vec::new();
        screen.draw(&mut out, &b).unwrap();
        let whole = out.len();
        assert!(whole > 0);
        // The same board: nothing to write.
        out.clear();
        screen.draw(&mut out, &b).unwrap();
        assert!(out.is_empty());
        // Forgotten, as after Ctrl-L or a while: all of it again.
        screen.forget();
        out.clear();
        screen.draw(&mut out, &b).unwrap();
        assert_eq!(out.len(), whole);
    }

    #[test]
    fn teammate_gone_quiet_shows_unknown() {
        let mut cards = with_helpers();
        cards[0].helpers.clear();
        cards[0].helpers.push(Helper {
            id: "m".into(),
            kind: "reviewer".into(),
            description: String::new(),
            teammate: Some("critic".into()),
            status: HelperStatus::Running,
            started: 0,
            since: 1000 - bridge::TEAMMATE_UNKNOWN,
        });
        let lines: Vec<String> = render(&board(cards), 70, 30).lines.iter().map(Line::text).collect();
        assert!(lines.iter().any(|l| l.starts_with("╻ ? critic")), "{lines:#?}");
    }

    #[test]
    fn capsule_over_four_lines() {
        let right = Right { model: 4, initial: false, effort: 8, time: 2 };
        let lines: Vec<String> =
            card(&long_and_short()[0], 50, 2, &right, &board(Vec::new())).iter().map(Line::text).collect();
        let bars: Vec<char> = lines.iter().map(|l| l.chars().next().unwrap()).collect();
        assert_eq!(bars, ['╻', '┃', '┃', '╹']);
        assert_eq!(lines[1].trim_end(), "┃   Je relis la maquette des cartes et je corrige");
        assert_eq!(lines[2].trim_end(), "┃   les colonnes de droite");
        // At rest, the lines after the first under the text, past its ✓.
        let done = Card {
            doing: Some(Task {
                now: String::new(), done: Some("Version 0.6.2 publiée et poussée dans le tap".into())
            }),
            ..sample_card("dev", false, State::Idle)
        };
        let lines: Vec<String> = card(&done, 40, 2, &right, &board(Vec::new())).iter().map(Line::text).collect();
        assert_eq!(lines[1].trim_end(), "┃   ✓ Version 0.6.2 publiée et poussée");
        assert_eq!(lines[2].trim_end(), "┃     dans le tap");
    }

    #[test]
    fn forms_given_up_in_turn() {
        // The contact at work over two lines, the agent at rest with its task done: 15 lines leave them 8.
        let at = |height: usize| -> Vec<String> {
            padded(&board(long_and_short()), 70, height).lines.iter().map(Line::text).collect()
        };
        let middles = |lines: &[String], name: &str| {
            let top = lines.iter().position(|l| l.contains(&format!(" {name} "))).unwrap();
            lines[top + 1..].iter().take_while(|l| l.starts_with('┃')).count()
        };
        let named =
            |lines: &[String]| lines.iter().any(|l| l.starts_with(&t!("au repos", "idle")) && l.contains("dev"));
        let cut = |lines: &[String]| lines.iter().any(|l| l.starts_with('┃') && l.trim_end().ends_with('…'));
        let lines = at(15);
        assert!(middles(&lines, "coordinateur") == 2 && middles(&lines, "dev") == 1 && !cut(&lines), "{lines:#?}");
        // The agent at rest first: without its task, then only named.
        let lines = at(14);
        assert!(middles(&lines, "coordinateur") == 2 && middles(&lines, "dev") == 0, "{lines:#?}");
        let lines = at(13);
        assert!(middles(&lines, "coordinateur") == 2 && named(&lines) && !cut(&lines), "{lines:#?}");
        // Then the contact: its task on one line, cut, then without it.
        let lines = at(12);
        assert!(middles(&lines, "coordinateur") == 1 && named(&lines) && cut(&lines), "{lines:#?}");
        let lines = at(11);
        assert!(middles(&lines, "coordinateur") == 0 && named(&lines), "{lines:#?}");
        // At last, rather than no card, no names: only how many more.
        let lines = at(10);
        assert!(middles(&lines, "coordinateur") == 0 && !named(&lines), "{lines:#?}");
        assert!(lines.iter().any(|l| l == &t!("… et {} de plus", "… and {} more", 1)), "{lines:#?}");
    }

    #[test]
    fn zones_over_cards_of_different_heights() {
        // One column: four lines, an empty one, three.
        let drawn = padded(&board(long_and_short()), 70, 15);
        zones_match(&drawn);
        let rows = |drawn: &Drawn| -> Vec<(String, ZoneKind, usize, usize, usize)> {
            drawn.zones.iter().map(|z| (z.member.clone(), z.kind, z.row, z.col, z.rows)).collect()
        };
        assert_eq!(
            rows(&drawn)[..3],
            [
                ("coordinateur".into(), ZoneKind::Member, 2, 0, 4),
                ("dev".into(), ZoneKind::Member, 7, 0, 3),
                ("dev".into(), ZoneKind::Compact, 9, 63, 1),
            ]
        );
        // Side by side, two at work: each its own height, the shorter one's curve on its own last line.
        let pair = || {
            let task = Some(Task { now: "Je relis le clic".into(), done: None });
            vec![
                Card { doing: task, ..sample_card("relecteur", false, State::Working) },
                sample_card("dev", false, State::Working),
            ]
        };
        let drawn = padded(&board(pair()), 70, 10);
        zones_match(&drawn);
        assert_eq!(
            rows(&drawn)[..2],
            [("relecteur".into(), ZoneKind::Member, 2, 0, 3), ("dev".into(), ZoneKind::Member, 2, 35, 2)]
        );
        // The shorter one on the left: room kept under it, the right one's capsule in its column on all its lines.
        let mut cards = pair();
        cards.reverse();
        let drawn = padded(&board(cards), 70, 10);
        zones_match(&drawn);
        assert_eq!(rows(&drawn)[0], ("dev".into(), ZoneKind::Member, 2, 0, 2));
        assert_eq!(rows(&drawn)[1], ("relecteur".into(), ZoneKind::Member, 2, 35, 3));
        let capsule: Vec<Option<char>> = drawn.lines[2..5].iter().map(|l| visible(l).chars().nth(35)).collect();
        assert_eq!(capsule, [Some('╻'), Some('┃'), Some('╹')]);
        assert_eq!(visible(&drawn.lines[4]).chars().position(|c| c == '╹'), Some(35));
    }

    #[test]
    fn the_right_of_the_titles_in_columns() {
        let cards = vec![
            sample_card("coordinateur", true, State::Working),
            Card {
                model: Some("Sonnet".into()),
                effort: Some("low".into()),
                since: 4000,
                ..sample_card("dev", false, State::Idle)
            },
            Card { model: None, effort: None, ..sample_card("ops", false, State::Waiting) },
        ];
        let titles = |width: usize| -> Vec<String> {
            let lines = render(&board(cards.clone()), width, 23).lines;
            lines.iter().map(Line::text).filter(|l| l.starts_with('╻')).collect()
        };
        let ends = |titles: Vec<String>, ends: [&str; 3]| {
            assert!(
                titles
                    .iter()
                    .zip(ends)
                    .all(|(t, end)| t.ends_with(end) && t.chars().count() == titles[0].chars().count()),
                "{titles:#?}"
            );
        };
        // The richest form they all hold, each column as wide as its widest (the effort's the widest name of the language,
        // two columns more than a sign), the time on the right: coordinateur holds the model and the effort's name from 39
        // columns on, its sign from 32, the model's initial from 27; a model not known yet is « ? » in its column.
        let name = |level: &str| format!("{:<8}", look::effort_label(level));
        ends(
            titles(39),
            [
                &format!(" Opus   {}    1m ", name("xhigh")),
                &format!(" ?      {}    1m ", " ".repeat(8)),
                &format!(" Sonnet {}  1h06 ", name("low")),
            ],
        );
        ends(titles(38), [" Opus   ◉    1m ", " ?           1m ", " Sonnet ○  1h06 "]);
        ends(titles(32), [" Opus   ◉    1m ", " ?           1m ", " Sonnet ○  1h06 "]);
        ends(titles(31), [" O ◉    1m ", " ?      1m ", " S ○  1h06 "]);
        ends(titles(27), [" O ◉    1m ", " ?      1m ", " S ○  1h06 "]);
        let bare = titles(26);
        assert!(bare.iter().all(|t| !t.contains("Opus") && !t.contains('◉') && t.chars().count() == 26));
        assert!(bare[2].ends_with(" 1h06 "));
    }

    /// An effort no one knows (a later Claude Code's, a slip in a file): « ? » in grey, its name cut at the column, never
    /// past it.
    #[test]
    fn an_unknown_effort_stays_in_its_column() {
        let odd = Card { effort: Some("ultra-turbo-extreme".into()), ..sample_card("dev", false, State::Working) };
        let right = Right { model: 4, initial: false, effort: 8, time: 2 };
        let title = card(&odd, 50, 0, &right, &board(Vec::new()))[0].clone();
        let text = title.text();
        assert!(text.contains("? ultra-") && !text.contains("ultra-t"), "{text}");
        assert_eq!(text.chars().count(), 50, "{text}");
        let grey = "?".with(Color::DarkGrey).to_string();
        assert!(title.printed().contains(&grey), "grey, not a rainbow");
    }

    #[test]
    fn model_faded_effort_in_its_colors() {
        let right = Right { model: 6, initial: false, effort: 8, time: 2 };
        let title = card(&sample_card("dev", false, State::Working), 50, 0, &right, &board(Vec::new()))[0].clone();
        let dim = "\x1b[2m";
        let title = title.printed();
        assert!(title.contains(&format!("{dim}Opus")));
        let violet = "◉".with(Color::AnsiValue(141)).to_string();
        assert!(!title.contains(&format!("{dim}◉")) && title.contains(&violet));
    }

    #[test]
    fn context_near_compaction() {
        assert_eq!(Pressure::of(Some(150_000), None), Pressure::Calm);
        assert_eq!(Pressure::of(None, Some(167_000)), Pressure::Calm);
        assert_eq!(Pressure::of(Some(133_000), Some(167_000)), Pressure::Calm);
        assert_eq!(Pressure::of(Some(133_600), Some(167_000)), Pressure::Near);
        assert_eq!(Pressure::of(Some(147_000), Some(167_000)), Pressure::Warning);
        assert_eq!(Pressure::of(Some(773_600), Some(967_000)), Pressure::Near);
        let near = Card { pressure: Pressure::Near, ..sample_card("dev", false, State::Idle) };
        let field: String = context(&near).iter().map(|(text, paint)| paint.apply(text)).collect();
        assert!(field.contains(&"41 %".with(look::palette().orange).to_string()));
        assert_eq!(visible(&field), " ⟳ 41 %");
        // At work, no compacting.
        assert_eq!(compaction_cells(&sample_card("dev", false, State::Working)), None);
    }

    #[test]
    fn big_team_two_columns_then_names() {
        let mut cards =
            vec![sample_card("coordinateur", true, State::Idle), sample_card("archi", true, State::Working)];
        cards.extend((0..4).map(|i| sample_card(&format!("w{i}"), false, State::Working)));
        cards.extend((0..9).map(|i| sample_card(&format!("repos{i}"), false, State::Idle)));
        let lines: Vec<String> = padded(&board(cards.clone()), 70, 23).lines.iter().map(Line::text).collect();
        assert!(lines.iter().all(|l| l.chars().count() <= 70), "{lines:#?}");
        // Nothing to say of what they do: two side by side rather than a name less. A contact at rest keeps its card,
        // the latest agents at rest too; the others are only named.
        assert!(lines[2].contains("╻ ◷ coordinateur ") && lines[2].contains("╻ ⠋ archi "), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("╻ ◷ repos0 ") && l.contains("╻ ◷ repos1 ")));
        assert_eq!(named(&lines), ["repos4", "repos5", "repos6", "repos7", "repos8"]);

        // Twice the height: every one in cards, two side by side.
        let tall: Vec<String> = padded(&board(cards.clone()), 70, 47).lines.iter().map(Line::text).collect();
        assert!(tall.iter().any(|l| l.contains("╻ ◷ repos0 ")));
        // Too narrow for two side by side.
        let narrow: Vec<String> = padded(&board(cards), 50, 47).lines.iter().map(Line::text).collect();
        assert!(narrow.iter().all(|l| l.matches('╻').count() <= 1 && l.chars().count() <= 50));
    }

    /// Each zone over what it names: a whole card, in its capsule, with the member's name on top; the name itself; or
    /// the context to compact.
    fn zones_match(drawn: &Drawn) {
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

    #[test]
    fn zones_over_cards_and_names() {
        let b = board(vec![
            sample_card("coordinateur", true, State::Working),
            sample_card("lanceur", false, State::Idle),
            sample_card("interface", false, State::Waiting),
        ]);
        let drawn = padded(&b, 70, 23);
        zones_match(&drawn);
        let at =
            |member: &str, kind| drawn.zones.iter().find(|z| z.member == member && z.kind == kind).cloned().unwrap();
        let zone =
            |member: &str, kind, row, col, rows, cols| Zone { member: member.into(), kind, row, col, rows, cols };
        assert_eq!(at("coordinateur", ZoneKind::Member), zone("coordinateur", ZoneKind::Member, 2, 0, 2, 70));
        assert_eq!(at("interface", ZoneKind::Member), zone("interface", ZoneKind::Member, 5, 0, 2, 70));
        // The context of a member at rest, to compact it.
        assert_eq!(at("lanceur", ZoneKind::Compact), zone("lanceur", ZoneKind::Compact, 9, 63, 1, 6));
        assert_eq!(drawn.zones.iter().filter(|z| z.kind == ZoneKind::Compact).count(), 1);
        // An absent member is named in a list, after its label.
        assert_eq!(at("ops", ZoneKind::Member), zone("ops", ZoneKind::Member, 11, 10, 1, 3));

        // Two side by side, the oldest agents at rest named.
        let mut cards =
            vec![sample_card("coordinateur", true, State::Idle), sample_card("archi", true, State::Working)];
        cards.extend((0..4).map(|i| sample_card(&format!("w{i}"), false, State::Working)));
        cards.extend((0..9).map(|i| sample_card(&format!("repos{i}"), false, State::Idle)));
        let drawn = padded(&board(cards), 70, 23);
        zones_match(&drawn);
        let archi = drawn.zones.iter().find(|z| z.member == "archi").unwrap();
        assert_eq!((archi.row, archi.col, archi.cols), (2, 35, 35));
        let w1 = drawn.zones.iter().find(|z| z.member == "w1").unwrap();
        assert_eq!((w1.row, w1.col, w1.cols), (5, 35, 35));
        let named =
            drawn.zones.iter().filter(|z| z.member.starts_with("repos") && z.rows == 1 && z.kind == ZoneKind::Member);
        let named: Vec<(&str, usize)> = named.map(|z| (z.member.as_str(), z.col)).collect();
        assert_eq!(named, [("repos4", 10), ("repos5", 19), ("repos6", 28), ("repos7", 37), ("repos8", 46)]);
    }

    #[test]
    fn zones_follow_what_moves_them() {
        let cards = || vec![sample_card("coordinateur", true, State::Working), sample_card("dev", false, State::Idle)];
        let rows = |b: &Board| padded(b, 70, 23).zones.iter().map(|z| z.row).collect::<Vec<_>>();
        // An error first, then a blank: everything two lines lower.
        let failing = Board { error: Some("claude agents: boom".into()), ..board(cards()) };
        zones_match(&padded(&failing, 70, 23));
        assert_eq!(rows(&failing), rows(&board(cards())).iter().map(|r| r + 2).collect::<Vec<_>>());

        // Two side by side on an odd width: the right one a column wider.
        let mut many: Vec<Card> = (0..6).map(|i| sample_card(&format!("w{i}"), false, State::Working)).collect();
        many.extend((0..4).map(|i| sample_card(&format!("v{i}"), false, State::Waiting)));
        let drawn = padded(&board(many), 71, 23);
        zones_match(&drawn);
        assert!(drawn.lines.iter().all(|l| visible(l).chars().count() <= 71));
        let w1 = drawn.zones.iter().find(|z| z.member == "w1").unwrap();
        assert_eq!((w1.col, w1.cols), (35, 36));

        // A name wider than its line: cut, its zone over what shows.
        let list = vec!["court".to_string(), "un-nom-bien-trop-long".to_string()];
        let drawn = names("au repos", &list, 20, Color::DarkGrey, usize::MAX);
        let lines: Vec<String> = drawn.lines.iter().map(Line::text).collect();
        assert_eq!(lines, ["au repos  court", "          un-nom-bi…"]);
        zones_match(&drawn);
        let cut = Zone { member: list[1].clone(), kind: ZoneKind::Member, row: 1, col: 10, rows: 1, cols: 10 };
        assert_eq!(drawn.zones[1], cut);
    }

    #[test]
    fn zone_bounds() {
        let zone = Zone { member: "dev".into(), kind: ZoneKind::Member, row: 2, col: 35, rows: 3, cols: 35 };
        assert!(zone.contains(35, 2) && zone.contains(69, 4));
        // Beside the card, above and under it.
        assert!(!zone.contains(34, 2) && !zone.contains(70, 2) && !zone.contains(35, 1) && !zone.contains(35, 5));
    }

    fn snapshot(names: &[&str]) -> Snapshot {
        let member = |name: &&str| crate::state::MemberInfo {
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
        let server = seen.cards.iter().find(|c| c.name == "server").unwrap();
        assert_eq!((server.state, server.shell), (State::Idle, true));
        // Since its file says, ten minutes: not since the panel looked.
        assert!((600..610).contains(&server.since), "{}", server.since);
        let states = memory.states(&seen, SystemTime::now(), Instant::now());
        assert_eq!(states.members["server"].state, State::Idle);
        let right = Right::of([(server, 50)]);
        let title = visible(&card(server, 50, 0, &right, &board(Vec::new()))[0]);
        assert!(title.starts_with(&format!("╻ {} server", Glyphs::Unicode.console())), "{title:?}");
    }

    /// The states the members' panes tell (OSC 7501) come before `claude agents`, with since when they say; a member
    /// told by its pane has a card before `claude agents` sees it; at rest by its pane, a command still running is read
    /// in its session's file (2.0.1).
    #[test]
    fn states_told_by_the_panes() {
        let pane = |member: &str, role: &str, status, since| backend::PaneState {
            id: member.into(),
            member: member.into(),
            role: role.into(),
            window: "Agents".into(),
            status,
            since,
            error: None,
        };
        let at = now() as u64;
        let panes = [
            pane("lead", "", Some(State::Working), Some(at - 120)),
            pane("server", "", Some(State::Idle), Some(at - 30)),
            pane("new", "", Some(State::Waiting), None),
            pane("quiet", "", None, None),
            pane("Dashboard", backend::DASHBOARD, Some(State::Working), None),
        ];
        let told = told(Some(&panes));
        assert_eq!(
            told.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
            ["lead", "new", "server"].into()
        );
        assert!(super::told(None).is_empty());

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
        let card = |name: &str| seen.cards.iter().find(|c| c.name == name).unwrap();
        assert_eq!(card("lead").state, State::Working);
        assert!((120..123).contains(&card("lead").since), "{}", card("lead").since);
        assert_eq!((card("server").state, card("server").shell), (State::Idle, true));
        assert!((600..603).contains(&card("server").since), "{}", card("server").since);
        assert_eq!((card("new").state, card("new").since), (State::Waiting, 0));
        assert_eq!(card("quiet").state, State::Waiting);
        assert_eq!(seen.absent, ["gone"]);
    }

    /// The looks of the headers: the state only while the dashboard looks; stopped, its last states are not shown
    /// as if they held (the pane says « ◌ »), the time and the rest stay (2.0.1).
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

    /// What a card among the others shows of a member beyond its state (lot of 2026-10-11): what it does and its
    /// curve, written by the dashboard as stretches at work, read by the server with the looks; only while the
    /// dashboard looks.
    #[test]
    fn the_cards_of_the_others_get_their_curve_and_task() {
        let now = now();
        // Working for 20 s, 10 s at rest, 30 s at work again, 5 s at rest: one sample a second.
        let samples: Vec<(i64, bool)> = (0..65).map(|i| (now - 64 + i, !(20..30).contains(&i) && i < 60)).collect();
        let activity = Activity::of(&samples);
        assert_eq!(activity.seen, now - 64);
        assert_eq!(activity.work, [(now - 64, now - 44), (now - 34, now - 4)]);
        // Back into samples, the same ones, a second at a time.
        assert_eq!(activity.samples(now), samples);
        assert!(Activity::default().samples(now).is_empty(), "nothing seen, nothing known");
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
        let looks = super::looks(state.path());
        assert_eq!((looks["dev"].doing.as_ref(), &looks["dev"].activity), (None, &Activity::default()));
        // The dashboard writes it for each card, and an older file without it still reads.
        let board = Board { cards: vec![sample_card("dev", false, State::Working)], now, ..Default::default() };
        let mut memory = Memory::default();
        memory.since.insert("dev".into(), (State::Working, Instant::now()));
        let written = memory.states(&board, SystemTime::now(), Instant::now());
        assert_eq!(written.cards["dev"].activity, Activity::of(&board.cards[0].samples));
        let old: States = serde_json::from_str(r#"{"at": 1, "members": {}, "absent": []}"#).unwrap();
        assert!(old.cards.is_empty());
    }

    /// A compaction leaves its trace (lot of 2026-10-11, mock-up C): a line in the journal at its time, dimmed but for the
    /// name, with « auto » when the server made it; an automatic one that failed, a line in orange; one put off for
    /// an agent at work, nothing. And a ⟳ in the card's curve, in the slice it fell in, until it leaves the half hour.
    #[test]
    fn a_compaction_leaves_a_trace() {
        let state = tempfile::tempdir().unwrap();
        fs::create_dir_all(state.path().join("members")).unwrap();
        let write = |report: &Report| {
            fs::write(state.path().join("members/dev.json"), serde_json::to_string(report).unwrap()).unwrap();
        };
        let done = |auto, after| bridge::Compaction {
            id: "c1".into(),
            done: true,
            at: 1000,
            auto,
            percent: Some(74),
            after,
            ..Default::default()
        };
        let mut printed = HashSet::new();
        // Made, the context after not known yet: the line waits for it, a minute at most.
        write(&Report { compacted: Some(done(true, None)), ..Default::default() });
        assert!(compactions(state.path(), "dev", 1010, &mut printed).is_empty());
        let found = compactions(state.path(), "dev", 1070, &mut printed);
        assert_eq!((found.len(), found[0].after), (1, None));
        // Once only.
        assert!(compactions(state.path(), "dev", 1080, &mut printed).is_empty());
        // With the context after, at once.
        let mut printed = HashSet::new();
        write(&Report { compacted: Some(done(false, Some(9))), ..Default::default() });
        let found = compactions(state.path(), "dev", 1001, &mut printed);
        assert_eq!((found[0].auto, found[0].before, found[0].after), (false, Some(74), Some(9)));
        // The lines.
        let s = snapshot(&["dev"]);
        let plain = |line: String| String::from_utf8_lossy(&strip_ansi(line.as_bytes())).into_owned();
        let auto = Compacted { auto: true, ..found[0].clone() };
        let (what, auto_what) = (t!("compacté", "compacted"), t!("compacté auto", "auto-compacted"));
        assert_eq!(
            plain(compaction_head(&s, "14:32", &auto, 80)),
            format!(" 14:32  ⟳ dev {auto_what} · {}", t!("74 → 9 %", "74 → 9%"))
        );
        assert_eq!(
            plain(compaction_head(&s, "14:32", &found[0], 80)),
            format!(" 14:32  ⟳ dev {what} · {}", t!("74 → 9 %", "74 → 9%"))
        );
        let half = Compacted { after: None, ..found[0].clone() };
        assert_eq!(
            plain(compaction_head(&s, "14:32", &half, 80)),
            format!(" 14:32  ⟳ dev {what} · {}", t!("74 %", "74%"))
        );
        // Its name is a click on that member, nowhere else.
        let line = plain(compaction_head(&s, "14:32", &found[0], 80));
        assert_eq!(journal_name_at(&line, 11, 80), Some("dev"));
        assert_eq!(journal_name_at(&line, 14, 80), None);
        // A narrow pane cuts between words over two lines, then drops some: the figures first, then « auto ».
        let words =
            ["14:32", "⟳", "dev", what.as_str(), "auto", "auto-compacted", "·", "74", "→", "9", "%", "9%", "74%"];
        for width in 14..60 {
            let text = plain(compaction_head(&s, "14:32", &auto, width));
            let lines: Vec<&str> = text.lines().collect();
            assert!(lines.len() <= 2 && lines.iter().all(|l| l.chars().count() <= width), "{width}: {text:?}");
            assert!(text.split_whitespace().all(|word| words.contains(&word)), "cut in a word at {width}: {text:?}");
        }
        // A failed automatic one: orange, once; put off for an agent at work: nothing.
        let mut printed = HashSet::new();
        let failed = bridge::Compaction {
            id: "c2".into(),
            at: 2000,
            auto: true,
            error: Some("API surchargée".into()),
            ..Default::default()
        };
        write(&Report { compaction: Some(failed.clone()), ..Default::default() });
        let found = compactions(state.path(), "dev", 2001, &mut printed);
        assert_eq!(found[0].failed.as_deref(), Some("API surchargée"));
        assert!(compaction_head(&s, "14:32", &found[0], 80).contains("\x1b[38;5;208m"), "orange");
        // Its reason cut between words, « … » after the last, and dropped before the rest is cut.
        let long = Compacted {
            failed: Some("API surchargée pour le moment, réessaie plus tard".into()),
            ..found[0].clone()
        };
        let line = plain(compaction_head(&s, "14:32", &long, 60));
        assert!(line.chars().count() <= 60 && line.ends_with('…') && !line.ends_with(" …"), "{line:?}");
        let text = plain(compaction_head(&s, "14:32", &long, 30));
        assert!(text.starts_with(" 14:32  ✗ dev") && text.lines().count() <= 2, "{text:?}");
        // A reason none of whose words holds is left out whole, not cut to « ECONNR… ».
        let word = Compacted { failed: Some("ECONNREFUSEDXYZ".into()), ..found[0].clone() };
        for width in 24..60 {
            let text = plain(compaction_head(&s, "14:32", &word, width));
            assert!(!text.contains("ECONN") || text.contains("ECONNREFUSEDXYZ"), "{width}: {text:?}");
        }
        assert!(!plain(compaction_head(&s, "14:32", &word, 40)).contains('…'));
        assert_eq!(fit_words("ECONNREFUSEDXYZ", 8), None);
        assert_eq!(fit_words("ECONNREFUSEDXYZ", 15).as_deref(), Some("ECONNREFUSEDXYZ"));
        assert!(compactions(state.path(), "dev", 2002, &mut printed).is_empty());
        let put_off = bridge::Compaction { skip: Some(bridge::SKIP_HELPERS.into()), error: None, ..failed };
        write(&Report { compaction: Some(put_off), ..Default::default() });
        assert!(compactions(state.path(), "dev", 2003, &mut HashSet::new()).is_empty());
        // The curve's ⟳: 30 minutes over 60 cells, 30 s each; at 1000 s, now 1100 s: the slice 3 cells back.
        assert_eq!(compaction_cell(1000, 1100, 60), Some(56));
        assert_eq!(compaction_cell(1100, 1100, 60), Some(59));
        assert_eq!(compaction_cell(1000, 1000 + HISTORY, 60), None, "left the half hour");
        let mut card = sample_card("dev", false, State::Idle);
        card.compacted = Some(1000);
        let lines = super::card(&card, 44, 0, &Right::of([(&card, 44)]), &board(Vec::new()));
        let curve = lines.last().unwrap().text();
        assert_eq!(curve.matches('⟳').count(), 2, "the curve's and the context's: {curve}");
        card.compacted = None;
        let lines = super::card(&card, 44, 0, &Right::of([(&card, 44)]), &board(Vec::new()));
        assert_eq!(lines.last().unwrap().text().matches('⟳').count(), 1, "only the context's (at rest)");
    }

    /// A compaction event's lines, one string.
    fn compaction_head(s: &Snapshot, time: &str, c: &Compacted, width: usize) -> String {
        compaction_lines(s, time, c, width).join("\n")
    }

    fn strip_ansi(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b {
                while i < bytes.len() && bytes[i] != b'm' {
                    i += 1;
                }
                i += 1;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        out
    }

    /// The multiplexer's answer to `CSI ? 996 n`, and its notices, however the reads cut them: the others are dropped.
    #[test]
    fn color_scheme_reports_cut_anywhere() {
        let mut bytes = b"abc\x1b[?997;".to_vec();
        assert!(scheme_reports(&mut bytes).is_empty());
        assert_eq!(bytes, b"\x1b[?997;");
        bytes.extend(b"2n typed \x1b[?99");
        assert_eq!(scheme_reports(&mut bytes), [true]);
        assert_eq!(bytes, b"\x1b[?99");
        bytes.extend(b"7;1n\x1b[?997;2n");
        assert_eq!(scheme_reports(&mut bytes), [false, true]);
        assert!(bytes.is_empty());
        // What is not one is never kept: another report, a key.
        bytes.extend(b"\x1b[?996n\x1b[?997;x1 q \x1b[A");
        assert!(scheme_reports(&mut bytes).is_empty());
        assert!(bytes.len() < 3, "{bytes:?}");
    }

    /// In a light terminal the board is drawn in the light palette (2026-10-11, validated): the yellow at work gives way
    /// to 136, the orange to 166, the members' own colors to theirs; back in the dark, as before.
    #[test]
    fn a_board_in_the_light_palette() {
        let first = |c: &Card| card(c, 44, 0, &Right::of([(c, 44)]), &board(Vec::new()))[0].0[0].1;
        let working = sample_card("dev", false, State::Working);
        let failed = Card { failed: Some(("API".into(), None)), ..sample_card("dev", false, State::Error) };
        assert_eq!(first(&working), Paint::Color(Color::Yellow));
        look::set_light(true);
        assert_eq!(first(&working), Paint::Color(Color::AnsiValue(136)));
        assert_eq!(first(&failed), Paint::Bold(Color::AnsiValue(166)));
        assert_eq!(look::member_color(3), Color::AnsiValue(167));
        // The gauge's warning color follows.
        let usage = usage_line(&[("5 h".into(), 75.0, None)], 40);
        assert!(usage.0.iter().any(|(_, paint)| *paint == Paint::Color(Color::AnsiValue(136))), "{usage:?}");
        look::set_light(false);
        assert_eq!(first(&working), Paint::Color(Color::Yellow));
        assert_eq!(look::member_color(3), Color::AnsiValue(209));
    }

    /// A member whose turn failed (mock-up C): its card after those waiting and before those at work, an orange
    /// capsule, ✗, « Tour raté : <raison> » then the task it stopped in after ↳, faded; its context compactable; the
    /// error's text written for the menu, the reason read from its pane's error.
    #[test]
    fn a_failed_turn_on_its_card() {
        let label = look::error_label(Some("API Error: 529 overloaded"));
        let failed = Card {
            doing: Some(Task { now: "Encodeur des touches kitty".into(), done: None }),
            failed: Some((label.clone(), Some("API Error: 529 overloaded".into()))),
            ..sample_card("dev-saisie", false, State::Error)
        };
        let cards = vec![
            sample_card("dev-terminal", false, State::Working),
            failed.clone(),
            sample_card("testeur", false, State::Waiting),
        ];
        assert_eq!(
            ordered(&cards).iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["testeur", "dev-saisie", "dev-terminal"]
        );
        let b = board(vec![failed.clone()]);
        let right = Right::of([(&failed, 60)]);
        let lines = card(&failed, 60, 2, &right, &b);
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert!(text[0].starts_with("╻ ✗ dev-saisie"), "{text:#?}");
        assert_eq!(text[1].trim_end(), format!("┃   {}", t!("Tour raté : {}", "Turn failed: {}", label)));
        assert_eq!(text[2].trim_end(), "┃   ↳ Encodeur des touches kitty");
        assert!(text[3].ends_with(" ⟳ 41 % "), "{text:#?}");
        let orange = Paint::Color(State::Error.color());
        assert_eq!(lines[0].0[0], ("╻".to_string(), Paint::Bold(State::Error.color())));
        assert!(
            lines[1]
                .0
                .iter()
                .any(|(text, paint)| text.starts_with(&t!("Tour raté", "Turn failed")) && *paint == orange)
        );
        assert!(lines[2].0.iter().any(|(text, paint)| text.starts_with('↳') && *paint == Paint::Dim));
        // On one line: the reason only. With a command still running: the failure's sign.
        let one = card(&failed, 60, 1, &right, &b);
        assert_eq!(one.len(), 3);
        let busy = Card { shell: true, ..failed.clone() };
        assert!(card(&busy, 60, 0, &right, &b)[0].text().starts_with("╻ ✗"));

        // The states for the menu: the error's text.
        let mut memory = Memory::default();
        memory.since.insert("dev-saisie".into(), (State::Error, Instant::now()));
        let states = memory.states(&b, SystemTime::now(), Instant::now());
        assert_eq!(states.members["dev-saisie"].state, State::Error);
        assert_eq!(states.errors["dev-saisie"], "API Error: 529 overloaded");

        // From the panes: the state and the error's text.
        let pane = backend::PaneState {
            id: "%1".into(),
            member: "dev-saisie".into(),
            role: String::new(),
            window: "Moteur".into(),
            status: Some(State::Error),
            since: Some(now() as u64 - 240),
            error: Some("API Error: 529 overloaded".into()),
        };
        let sessions = Sessions {
            told: told(Some(std::slice::from_ref(&pane))),
            failed: super::failed(Some(std::slice::from_ref(&pane))),
            ..Default::default()
        };
        let state = tempfile::tempdir().unwrap();
        let seen = Memory::default().board(&snapshot(&["dev-saisie"]), state.path(), &sessions);
        assert_eq!(seen.cards[0].failed, Some((label, Some("API Error: 529 overloaded".into()))));
        assert!((240..243).contains(&seen.cards[0].since));
    }

    #[test]
    fn at_rest_the_latest_first_by_the_moment() {
        let s = snapshot(&["a", "b", "c"]);
        let state = tempfile::tempdir().unwrap();
        let look = |busy: &[&str]| Sessions {
            members: ["a", "b", "c"]
                .map(|name| {
                    let status = if busy.contains(&name) { "busy" } else { "idle" };
                    let session = Running {
                        name: Some(name.into()),
                        cwd: Some("/tmp/web".into()),
                        status: Some(status.into()),
                        session_id: None,
                        pid: None,
                    };
                    (name.to_string(), session)
                })
                .into(),
            ..Default::default()
        };
        let mut memory = Memory::default();
        let mut order = |busy: &[&str]| -> Vec<String> {
            let board = memory.board(&s, state.path(), &look(busy));
            ordered(&board.cards).iter().map(|c| c.name.clone()).collect()
        };
        // Seen at rest in one look: the file's order.
        assert_eq!(order(&[]), ["a", "b", "c"]);
        // At rest again, the latest; in the same second as the others, all the same.
        assert_eq!(order(&["b"]), ["b", "a", "c"]);
        assert_eq!(order(&[]), ["b", "a", "c"]);
        // Two at rest again in one look: the file's order between them.
        assert_eq!(order(&["c", "a"]), ["a", "c", "b"]);
        assert_eq!(order(&[]), ["a", "c", "b"]);
    }

    #[test]
    fn renamed_on_its_conversation_keeps_its_curve() {
        let state = tempfile::tempdir().unwrap();
        let look = |name: &str, session: &str| Sessions {
            members: [(
                name.to_string(),
                Running {
                    name: Some(name.into()),
                    cwd: Some("/tmp/web".into()),
                    status: Some("busy".into()),
                    session_id: Some(session.into()),
                    pid: None,
                },
            )]
            .into(),
            ..Default::default()
        };
        let mut memory = Memory::default();
        let samples = |board: &Board| board.cards.first().map(|c| c.samples.len());
        memory.board(&snapshot(&["dev"]), state.path(), &look("dev", "s1"));
        assert_eq!(samples(&memory.board(&snapshot(&["dev"]), state.path(), &look("dev", "s1"))), Some(2));
        // Renamed: its Claude starts again, a few looks without it.
        let renamed = snapshot(&["dev-rust"]);
        assert!(memory.board(&renamed, state.path(), &Sessions::default()).cards.is_empty());
        assert_eq!(samples(&memory.board(&renamed, state.path(), &look("dev-rust", "s1"))), Some(3));
        assert!(!memory.activity.contains_key("dev"));
        // On a new conversation: a new curve.
        let other = snapshot(&["qa"]);
        assert_eq!(samples(&memory.board(&other, state.path(), &look("qa", "s2"))), Some(1));
    }

    /// The pane shows the lines as drawn, a line as wide as the pane with its last cell; and while members work, a
    /// frame of the spinners sends their cells only (2.0.1: the title lines whole before, 733 bytes a frame).
    #[test]
    fn a_spinner_frame_sends_its_cells_only() {
        let working = |name: &str| Card { since: 30, ..sample_card(name, false, State::Working) };
        let mut b = board(vec![working("dev-a"), working("dev-b"), working("dev-c")]);
        let (width, height) = (60, 23);
        let mut screen = Screen::default();
        let whole = screen.changes((width as u16, height as u16), &b);
        assert!(!whole.is_empty());
        let lines = render(&b, width, height).lines;
        assert!(lines.iter().any(|l| crate::canvas::columns(&l.text()) == width), "{lines:#?}");
        for (y, line) in lines.iter().enumerate() {
            assert_eq!(screen.canvas.row(y).trim_end(), line.text().trim_end());
        }
        // The same board: nothing; the next spinners' image: a few bytes for each.
        assert!(screen.changes((width as u16, height as u16), &b).is_empty());
        b.frame += 1;
        let spun = screen.changes((width as u16, height as u16), &b);
        assert!(!spun.is_empty() && spun.len() <= 3 * 24, "{} bytes: {:?}", spun.len(), String::from_utf8_lossy(&spun));
        // A second of it, the times moving on once: under a kilobyte (the goal of 2.0.1).
        let mut second = 0;
        for i in 0..10 {
            b.frame += 1;
            if i == 9 {
                b.cards.iter_mut().for_each(|c| c.since += 1);
                b.now += 1;
            }
            second += screen.changes((width as u16, height as u16), &b).len();
        }
        eprintln!("a frame: {} bytes, a second: {second} bytes", spun.len());
        assert!(second < 1000, "{second} bytes in a second");
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

    /// The sessions written in the states for the journal and the members' panes, and read back (2.0.1).
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
        // A dashboard older than 2.0.1: members, no sessions; the journal asks `claude agents` itself.
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

    #[test]
    fn names_in_use_elsewhere_over_the_usage() {
        let cards = || {
            vec![
                sample_card("coordinateur", true, State::Working),
                sample_card("lanceur", false, State::Idle),
                sample_card("interface", false, State::Waiting),
            ]
        };
        let b = Board { elsewhere: vec![("coordinateur".into(), Some("shop".into()))], ..board(cards()) };
        let lines = |height: usize| -> Vec<String> { padded(&b, 70, height).lines.iter().map(Line::text).collect() };
        let line = t!(" nom en double ailleurs : {}", " name in use elsewhere: {}", "coordinateur (shop)");
        // Right over the usage, dimmed.
        let tall = lines(23);
        assert_eq!(tall[20], line);
        assert!(tall[21].contains("41 % ↻ 1h22") && tall[19].is_empty());
        assert_eq!(padded(&b, 70, 23).lines[20], Line::of(line.clone(), Paint::Dim));
        // The cards first: no room left, no line.
        assert_eq!(lines(16)[13], line);
        assert!(!lines(15).contains(&line), "{:#?}", lines(15));
        assert_eq!(padded(&b, 70, 15).zones, padded(&board(cards()), 70, 15).zones);
        // Several, on one line at most.
        let b = Board {
            elsewhere: vec![("coordinateur".into(), Some("shop".into())), ("dev".into(), None)],
            ..board(cards())
        };
        let shown =
            padded(&b, 30, 23).lines.iter().map(Line::text).find(|l| l.contains("ailleurs") || l.contains("elsewhere"));
        assert_eq!(shown.map(|l| (l.chars().count(), l.ends_with('…'))), Some((30, true)));
    }

    #[test]
    fn click_on_the_dashboard() {
        let dir = tempfile::tempdir().unwrap();
        let s = snapshot(&["coordinateur", "dev"]);
        let at = |x, y| member_at(&s, dir.path(), Kind::Dashboard, x, y, 70, "");
        // Nothing drawn yet.
        assert_eq!(at(0, 2), None);
        let zone =
            |member: &str, kind, row, col, rows, cols| Zone { member: member.into(), kind, row, col, rows, cols };
        let zones = vec![
            zone("coordinateur", ZoneKind::Member, 2, 0, 3, 35),
            zone("dev", ZoneKind::Member, 2, 35, 3, 35),
            zone("dev", ZoneKind::Compact, 4, 62, 1, 6),
            zone("parti", ZoneKind::Member, 6, 10, 1, 5),
        ];
        save_zones(dir.path(), &zones).unwrap();
        assert_eq!(super::zones(dir.path()), zones);
        assert_eq!(at(0, 2).as_deref(), Some("coordinateur"));
        assert_eq!(at(40, 4).as_deref(), Some("dev"));
        // The header, past the cards, and a name that is no member.
        assert_eq!(at(3, 0), None);
        assert_eq!(at(3, 5), None);
        assert_eq!(at(12, 6), None);
        assert_eq!(member_at(&s, dir.path(), Kind::Toggle, 0, 2, 70, ""), None);
        // On the context of a member at rest: compacting it, not going to it; elsewhere on its card, no compacting.
        assert_eq!(compaction_at(&s, dir.path(), 63, 4).as_deref(), Some("dev"));
        assert_eq!(at(63, 4).as_deref(), Some("dev"));
        assert_eq!(compaction_at(&s, dir.path(), 40, 4), None);
        // Only the file is left, no partial one.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn click_on_the_journal() {
        let s = snapshot(&["coordinateur", "dev", "dev-cli", "rédac"]);
        let within =
            |line: &str, x, columns| member_at(&s, Path::new("/nonexistent"), Kind::Journal, x, 0, columns, line);
        let at = |line: &str, x| within(line, x, 80);
        let line = format!(" 14:32  dev-cli {ARROW} dev");
        let names: Vec<Option<String>> = (0..24).map(|x| at(&line, x)).collect();
        let expected = |x: usize| match x {
            8..15 => Some("dev-cli".to_string()),
            20..23 => Some("dev".to_string()),
            _ => None,
        };
        assert_eq!(names, (0..24).map(expected).collect::<Vec<_>>());
        // A name in another: the whole one under the click.
        let line = format!(" 14:32  dev {ARROW} dev-cli   ");
        assert_eq!(at(&line, 10).as_deref(), Some("dev"));
        assert_eq!(at(&line, 16).as_deref(), Some("dev-cli"));
        assert_eq!(at(&line, 23), None);
        // Columns, not bytes: `é` takes two bytes and one column.
        let line = format!(" 09:05  rédac {ARROW} coordinateur");
        assert_eq!(at(&line, 12).as_deref(), Some("rédac"));
        assert_eq!(at(&line, 13), None);
        assert_eq!(at(&line, 18).as_deref(), Some("coordinateur"));
        // A recipient that is no member: a process nobody was found for.
        assert_eq!(at(&format!(" 09:05  dev {ARROW} uds:/tmp/cc-socks/42.sock"), 16), None);
        // Cut to a narrow journal rather than wrapped: the cut name is no one's.
        let head = visible(&message_head(&s, "14:32", "coordinateur", "dev-cli", 28));
        assert_eq!(head, format!(" 14:32  coordinateur {ARROW} de…"));
        assert_eq!(at(&head, 8).as_deref(), Some("coordinateur"));
        assert!((20..28).all(|x| at(&head, x).is_none()));
        let wide = visible(&message_head(&s, "14:32", "coordinateur", "dev-cli", 40));
        assert_eq!(at(&wide, 25).as_deref(), Some("dev-cli"));
        // Written 40 wide, then wrapped again by the pane's terminal narrowed to 28: `dev` up to the last column, `-cli` on
        // the next line. The recipient may go on: no one; the sender still answers.
        let wrapped = format!(" 14:32  coordinateur {ARROW} dev");
        assert_eq!(wrapped.chars().count(), 28);
        assert!((25..28).all(|x| within(&wrapped, x, 28).is_none()));
        assert_eq!(within(&wrapped, 8, 28).as_deref(), Some("coordinateur"));
        // A column left after it: the whole name.
        assert_eq!(within(&wrapped, 25, 29).as_deref(), Some("dev"));
        assert_eq!(within(&wrapped, 27, 29).as_deref(), Some("dev"));
        // Not a message's first line: its text, the title, or no time in front.
        assert!((0..40).all(|x| at("        ╰─ dev-cli asks dev ──▶ coordinateur", x).is_none()));
        assert!((0..30).all(|x| at(&format!(" Journal          {ALT}j masque"), x).is_none()));
        assert!((0..20).all(|x| at(&format!(" ab:cd  dev {ARROW} dev-cli"), x).is_none()));
    }

    #[test]
    fn curves() {
        let all: Vec<(i64, bool)> = (0..HISTORY).map(|i| (HISTORY - i, true)).collect();
        assert_eq!(curve(&all, HISTORY, 10), "⣿".repeat(10));
        // The last minute, over the end of a slice and the start of the next one.
        let recent: Vec<(i64, bool)> = (0..60).map(|i| (HISTORY - i, false)).collect();
        assert_eq!(curve(&recent, HISTORY, 10), format!("{}⣀⣀", " ".repeat(8)));

        // Slices of three minutes, working one, resting the next, since `base`.
        let base = 200 * 180;
        let samples =
            |until: i64| -> Vec<(i64, bool)> { (base..=until).map(|t| (t, (t - base) / 180 % 2 == 0)).collect() };
        let now = base + 1800;
        assert_eq!(curve(&samples(now), now, 10), "⣀⣿⣀⣿⣀⣿⣀⣿⣀⣿");
        // Half a slice later, the past ones have not moved.
        assert_eq!(curve(&samples(now + 90), now + 90, 10), "⣀⣿⣀⣿⣀⣿⣀⣿⣀⣿");
        // A new slice: the curve moves by one.
        assert_eq!(curve(&samples(now + 90), now + 180, 10), "⣿⣀⣿⣀⣿⣀⣿⣀⣿ ");
    }

    #[test]
    fn usage_from_the_freshest_window() {
        let report = |at: i64, five_hour: f64, resets: Option<&str>| Report {
            rate_limits: vec![bridge::RateLimit {
                kind: "five_hour".into(),
                percent_used: five_hour,
                resets_at: resets.map(String::from),
            }],
            at,
            ..Default::default()
        };
        let now = iso_epoch("2026-10-07T12:00:00Z").unwrap();
        let five_hour = |reports: &[Report]| windows(reports, now)[0].clone();
        // A member at rest reports just now what it heard long ago; the other, earlier, a newer use.
        let stale = report(now, 3.0, Some("2026-10-07T14:00:00.000Z"));
        let fresh = report(now - 30, 4.0, Some("2026-10-07T14:00:00Z"));
        assert_eq!(five_hour(&[stale.clone(), fresh.clone()]), ("5 h".into(), 4.0, Some(7200)));
        assert_eq!(five_hour(&[fresh.clone(), stale.clone()]).1, 4.0);
        // A new window resets later: its use starts again from little, whatever the old one reached.
        let full = report(now, 97.0, Some("2026-10-07T15:00:00Z"));
        let new_window = report(now - 60, 1.0, Some("2026-10-07T20:00:00Z"));
        assert_eq!(five_hour(&[full.clone(), new_window]).1, 1.0);
        // Without a reset time, only when nothing else tells one.
        let unknown = report(now, 50.0, None);
        assert_eq!(five_hour(&[unknown.clone(), fresh.clone()]).1, 4.0);
        assert_eq!(five_hour(std::slice::from_ref(&unknown)), ("5 h".into(), 50.0, None));
        // The window is over and nobody has heard of the next one: nothing used yet, no reset time.
        let over = iso_epoch("2026-10-07T15:00:00Z").unwrap();
        assert_eq!(windows(&[full, stale], over)[0], ("5 h".into(), 0.0, None));
        assert!(windows(&[], now).is_empty());
    }

    #[test]
    fn names_keep_their_column() {
        let b = board(Vec::new());
        let column = |state| {
            let right = Right { model: 4, initial: false, effort: 8, time: 2 };
            let top = visible(&card(&sample_card("archi", false, state), 40, 0, &right, &b)[0]);
            assert_eq!(top.chars().count(), 40);
            top.split("archi").next().unwrap().chars().count()
        };
        for state in [State::Working, State::Idle, State::Waiting, State::Other] {
            assert_eq!(column(state), 4);
        }
    }

    #[test]
    fn no_size_too_small() {
        // Thirteen cards, the same with long names, twenty-six; with an error, none.
        let long: Vec<Card> = capture()
            .into_iter()
            .enumerate()
            .map(|(i, c)| Card { name: format!("{}-avec-un-nom-très-long-{i}", c.name), ..c })
            .collect();
        let many: Vec<Card> = (0..2)
            .flat_map(|k| capture().into_iter().map(move |c| Card { name: format!("{}{k}", c.name), ..c }))
            .collect();
        let boards = [
            board(capture()),
            Board { glyphs: Glyphs::Nerd, ..board(long) },
            board(many),
            Board { error: Some("claude agents: boom".into()), usage: Vec::new(), ..board(Vec::new()) },
            board(vec![
                Card {
                    failed: Some((look::error_label(Some("API Error: 529")), None)),
                    doing: Some(Task { now: "Encodeur des touches kitty".into(), done: None }),
                    ..sample_card("dev-saisie", false, State::Error)
                },
                sample_card("dev-terminal", false, State::Working),
            ]),
        ];
        let s = snapshot(&["coordinateur", "dev-cli"]);
        let failed = std::sync::Mutex::new(Vec::new());
        // Every small size, which carry nearly all the overflows, then a step.
        let widths: Vec<usize> = (0..=40).chain((44..=120).step_by(8)).collect();
        let heights: Vec<usize> = (0..=16).chain((20..=40).step_by(5)).collect();
        std::thread::scope(|scope| {
            for chunk in widths.chunks(widths.len().div_ceil(8)) {
                let (boards, heights, failed, s) = (&boards, &heights, &failed, &s);
                scope.spawn(move || {
                    for &width in chunk {
                        for &height in heights {
                            for (i, b) in boards.iter().enumerate() {
                                let size = (width as u16, height as u16);
                                let drawn = std::panic::catch_unwind(|| {
                                    Screen::default().changes(size, b);
                                    render(b, width, height)
                                });
                                if drawn.is_err() {
                                    failed.lock().unwrap().push(format!("the dashboard {i} at {width} × {height}"));
                                }
                            }
                        }
                        let journal = std::panic::catch_unwind(|| {
                            message_head(s, "14:32", "dev-cli", "coordinateur", width);
                            fit("Le texte d'un message", width.saturating_sub(11))
                        });
                        if journal.is_err() {
                            failed.lock().unwrap().push(format!("the journal at {width}"));
                        }
                    }
                });
            }
        });
        let failed = failed.into_inner().unwrap();
        assert!(failed.is_empty(), "{} failed: {}", failed.len(), failed.join(", "));
    }

    /// No header: its frame says the counts, the bar the keys (mock-up B1); the cards from the top.
    #[test]
    fn no_header() {
        let cards = vec![sample_card("a", true, State::Working), sample_card("b", false, State::Idle)];
        let drawn = render(&board(cards), 60, 20);
        assert!(visible(&drawn.lines[0]).contains(" a"), "{:?}", drawn.lines[0]);
        assert!(drawn.lines.iter().all(|line| !visible(line).contains(&format!("{ALT}r menu"))));
        // An error first, then a blank.
        let failing = render(&Board { error: Some("boom".into()), ..board(Vec::new()) }, 60, 20);
        assert_eq!((visible(&failing.lines[0]).as_str(), failing.lines[1].text().as_str()), ("boom", ""));
        // A pane too low: the usage at the bottom only.
        let low = render(&board(vec![sample_card("a", true, State::Working); 6]), 60, 4);
        assert_eq!(low.lines.len(), 3);
        assert!(visible(&low.lines[2]).contains("5 h"), "{:?}", low.lines);
    }

    #[test]
    fn usage_on_one_line() {
        let usage = board(Vec::new()).usage;
        let days = t!("6j11h", "6d11h");
        let (full, empty) = (|n| "▰".repeat(n), |n| "▱".repeat(n));
        let wide = visible(&usage_line(&usage, 70));
        assert_eq!(wide, format!(" 5 h {}{} 41 % ↻ 1h22   7 j ▰{} 5 % ↻ {days}", full(7), empty(10), empty(16)));
        assert_eq!(wide.chars().count(), 70);
        // Too narrow: the reset times go first, then the bars.
        assert_eq!(
            visible(&usage_line(&usage, 40)),
            format!(" 5 h {}{} 41 %   7 j {} 5 %", full(4), empty(5), empty(9))
        );
        assert_eq!(visible(&usage_line(&usage, 25)), " 5 h 41 %   7 j 5 %");
        // Never longer than 20 cells.
        assert_eq!(visible(&usage_line(&usage, 200)).matches(['▰', '▱']).count(), 40);
        assert_eq!(remaining(42 * 60), "42min");
        assert_eq!(remaining(3900), "1h05");
        assert_eq!(remaining(3 * 86_400 + 4 * 3600 + 59), t!("3j4h", "3d4h"));
    }

    #[test]
    fn short_texts() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 4), "abc");
    }
}
