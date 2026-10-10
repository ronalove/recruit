// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The test's own terminal: a PTY whose master the test holds, a program on its slave (the multiplexer, a fake, a
//! tmux client), and an alacritty `Term` that reads what the program writes, answers its queries as a terminal
//! would (DA1, DSR and CPR, `CSI ? u`, OSC 10 and 11) and gives the screen back as text. A reader thread feeds it,
//! so the program is never held back by the test; it notes when a text it is told to watch for shows up.
//!
//! Owner: testeur.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{NamedColor, Processor, Rgb};

/// What the test terminal answers to OSC 10 and 11: a dark theme, its text of a color no engine has by default
/// (libghostty-vt's is d8d8d8), so that an answer passed on is told from one made up.
const FG: Rgb = Rgb { r: 0xc0, g: 0xc1, b: 0xc2 };
const BG: Rgb = Rgb { r: 0x1e, g: 0x1e, b: 0x1e };

/// How the test terminal behaves, to imitate a real one.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    /// Answers `CSI ? u` (the kitty keyboard protocol), as Ghostty, kitty, WezTerm and iTerm2 do.
    pub kitty_keyboard: bool,
    /// Keeps every byte the program writes, for `TestTerm::raw`.
    pub capture: bool,
    /// Answers XTVERSION (`CSI > q`) with this name, as Ghostty (`ghostty 1.3.1`), kitty or WezTerm do; alacritty
    /// itself does not.
    pub xtversion: Option<&'static str>,
}

impl Default for Profile {
    fn default() -> Profile {
        Profile { kitty_keyboard: true, capture: false, xtversion: None }
    }
}

/// What a terminal, a multiplexer or a Claude Code session puts in the environment to say what it is: the programs
/// the tests start must not inherit it (they run in the tester's own pane, in the user's tmux, in a Claude Code
/// session). The same list as `SCRUB` in `src/mux/server/pane.rs`: a Claude in a pane that kept `TMUX` would take
/// itself for a pane of that tmux, and one that kept `CLAUDE_CODE_CHILD_SESSION` would save no transcript.
pub const SCRUB: &[&str] = &[
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
    "KITTY_LISTEN_ON",
    "TERMINAL_EMULATOR",
    // Some shells export the size, which some programs prefer to TIOCGWINSZ.
    "COLUMNS",
    "LINES",
    // The marks of the Claude Code session recruit was started from (its Bash tool, a `!` command): inherited, a
    // member's Claude takes itself for that session's child, and saves no transcript (2.1.295). Not the user's own
    // settings (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`, `CLAUDE_CODE_USE_BEDROCK`…).
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
    // The multiplexer recruit was launched with (the user's `RECRUIT_BACKEND=native`): the tests say which they
    // want (`common::team`), and a comparison with tmux must not start natively.
    "RECRUIT_BACKEND",
];

#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if matches!(event, Event::PtyWrite(_) | Event::ColorRequest(..)) {
            lock(&self.0).push(event);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the reader thread and the test share.
struct Shared {
    term: Term<Listener>,
    parser: Processor,
    /// Bytes the program wrote, in all.
    bytes: u64,
    /// Synchronized-update starts seen (`CSI ? 2026 h`): the multiplexer's frames.
    frames: u64,
    /// The text watched for, and when it was first seen on the screen.
    watch: Option<(String, Option<Instant>)>,
    /// The program closed its side.
    closed: bool,
    /// Every byte, with `Profile::capture`.
    raw: Option<Vec<u8>>,
    xtversion: Option<&'static str>,
}

/// A program on a PTY of the test's, and the screen it draws.
pub struct TestTerm {
    child: Child,
    writer: File,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    reader: Option<JoinHandle<()>>,
    cols: usize,
    rows: usize,
}

impl TestTerm {
    /// Starts `program` with `args` on a new PTY of `cols` × `rows`, with `env` on top of this environment (less
    /// `TMUX` and `TMUX_PANE`, and with `TERM=xterm-256color`, `COLORTERM=truecolor`).
    pub fn spawn<I, S>(
        program: &OsStr,
        args: I,
        env: &[(&str, &OsStr)],
        cols: u16,
        rows: u16,
        profile: Profile,
    ) -> TestTerm
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        TestTerm::spawn_in(None, program, args, env, cols, rows, profile)
    }

    /// The same, in the directory `dir`.
    pub fn spawn_in<I, S>(
        dir: Option<&std::path::Path>,
        program: &OsStr,
        args: I,
        env: &[(&str, &OsStr)],
        cols: u16,
        rows: u16,
        profile: Profile,
    ) -> TestTerm
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let (master, slave) = openpty(cols, rows);
        let mut command = Command::new(program);
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        command.args(args);
        for key in SCRUB {
            command.env_remove(key);
        }
        command.env("TERM", "xterm-256color").env("COLORTERM", "truecolor");
        for (key, value) in env {
            command.env(key, value);
        }
        let slave_fd = slave.as_raw_fd();
        command
            .stdin(Stdio::from(slave.try_clone().expect("slave")))
            .stdout(Stdio::from(slave.try_clone().expect("slave")))
            .stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = super::retry_busy(|| command.spawn()).expect("program on the test PTY");
        let master = File::from(master);
        let writer = master.try_clone().expect("master");
        let config = Config { kitty_keyboard: profile.kitty_keyboard, scrolling_history: 0, ..Config::default() };
        let events = Listener::default();
        let term = Term::new(config, &TermSize::new(cols as usize, rows as usize), events.clone());
        let shared = Arc::new((
            Mutex::new(Shared {
                term,
                parser: Processor::new(),
                bytes: 0,
                frames: 0,
                watch: None,
                closed: false,
                raw: profile.capture.then(Vec::new),
                xtversion: profile.xtversion,
            }),
            Condvar::new(),
        ));
        let reader = {
            let shared = Arc::clone(&shared);
            let replies = master.try_clone().expect("master");
            std::thread::spawn(move || read(master, replies, &shared, &events))
        };
        TestTerm { child, writer, shared, reader: Some(reader), cols: cols as usize, rows: rows as usize }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Types `bytes`, as a terminal sends them; false once the program is gone.
    pub fn send(&mut self, bytes: &[u8]) -> bool {
        self.writer.write_all(bytes).is_ok()
    }

    /// The screen, one string per row, trailing blanks dropped.
    pub fn screen(&self) -> Vec<String> {
        let (mutex, _) = &*self.shared;
        render(&lock(mutex).term)
    }

    /// Bytes and frames the program wrote so far.
    pub fn counters(&self) -> (u64, u64) {
        let (mutex, _) = &*self.shared;
        let shared = lock(mutex);
        (shared.bytes, shared.frames)
    }

    /// Waits until `text` is on the screen, `timeout` at most; true if it is.
    pub fn wait_for(&self, text: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (mutex, condvar) = &*self.shared;
        let mut shared = lock(mutex);
        loop {
            if render(&shared.term).iter().any(|row| row.contains(text)) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline || shared.closed {
                return false;
            }
            shared = condvar.wait_timeout(shared, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Types `bytes` and measures how long until `text` shows on the screen, `timeout` at most. The time starts
    /// just before the write; the reader thread notes the first chunk after which the screen shows `text`.
    pub fn round_trip(&mut self, bytes: &[u8], text: &str, timeout: Duration) -> Option<Duration> {
        let (mutex, condvar) = &*self.shared;
        lock(mutex).watch = Some((text.to_string(), None));
        let start = Instant::now();
        if self.writer.write_all(bytes).is_err() {
            lock(mutex).watch = None;
            return None;
        }
        let deadline = start + timeout;
        let mut shared = lock(mutex);
        loop {
            if let Some((_, Some(seen))) = shared.watch {
                shared.watch = None;
                return Some(seen.saturating_duration_since(start));
            }
            let now = Instant::now();
            if now >= deadline || shared.closed {
                shared.watch = None;
                return None;
            }
            shared = condvar.wait_timeout(shared, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Whether the program has ended, after `timeout` at most.
    pub fn wait_exit(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether the program pushed the kitty keyboard protocol to this terminal (its "disambiguate" flag).
    pub fn kitty_keyboard(&self) -> bool {
        let (mutex, _) = &*self.shared;
        lock(mutex).term.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES)
    }

    /// Every byte the program wrote so far (with `Profile::capture`).
    pub fn raw(&self) -> Vec<u8> {
        let (mutex, _) = &*self.shared;
        lock(mutex).raw.clone().unwrap_or_default()
    }

    /// Resizes the terminal: the PTY (the program gets SIGWINCH) and the screen.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        // The screen first: the program redraws for the new size as soon as it hears of it.
        let (mutex, _) = &*self.shared;
        lock(mutex).term.resize(TermSize::new(cols as usize, rows as usize));
        let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: the master's descriptor, and a size that outlives the call.
        unsafe { libc::ioctl(self.writer.as_raw_fd(), libc::TIOCSWINSZ, &raw const size) };
        self.cols = cols as usize;
        self.rows = rows as usize;
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// The cursor: column, row (from 0), shown, and its shape as DECSCUSR sets it (`Block`, `Underline`, `Beam`…,
    /// and whether it blinks).
    pub fn cursor(&self) -> (usize, usize, bool, String) {
        let (mutex, _) = &*self.shared;
        let shared = lock(mutex);
        let point = shared.term.grid().cursor.point;
        let shown = shared.term.mode().contains(TermMode::SHOW_CURSOR);
        let style = shared.term.cursor_style();
        let shape = format!("{:?}{}", style.shape, if style.blinking { ", blinking" } else { "" });
        (point.column.0, point.line.0 as usize, shown, shape)
    }

    /// The whole screen with its styles, as JSON for `tests/gallery.py`: `rows` of cells `[text, fg, bg, flags]`, a
    /// color written `rgb:r,g,b`, `idx:n` or `named:Name`, the flags a string of `b`old, `d`im, `i`talic, `u`nderline,
    /// `r`everse, `s`trike.
    pub fn styled(&self) -> String {
        use alacritty_terminal::vte::ansi::Color;
        let paint = |color: Color| match color {
            Color::Spec(Rgb { r, g, b }) => format!("rgb:{r},{g},{b}"),
            Color::Indexed(n) => format!("idx:{n}"),
            Color::Named(name) => format!("named:{name:?}"),
        };
        let (mutex, _) = &*self.shared;
        let shared = lock(mutex);
        let grid = shared.term.grid();
        let rows: Vec<Vec<(String, String, String, String)>> = (0..self.rows)
            .map(|row| {
                (0..self.cols)
                    .map(|col| {
                        let cell = &grid[Line(row as i32)][Column(col)];
                        let mut text = cell.c.to_string();
                        text.extend(cell.zerowidth().into_iter().flatten());
                        let mut flags = String::new();
                        for (flag, letter) in [
                            (Flags::BOLD, 'b'),
                            (Flags::DIM, 'd'),
                            (Flags::ITALIC, 'i'),
                            (Flags::UNDERLINE, 'u'),
                            (Flags::INVERSE, 'r'),
                            (Flags::STRIKEOUT, 's'),
                            (Flags::WIDE_CHAR_SPACER, 'w'),
                        ] {
                            if cell.flags.contains(flag) {
                                flags.push(letter);
                            }
                        }
                        (text, paint(cell.fg), paint(cell.bg), flags)
                    })
                    .collect()
            })
            .collect();
        serde_json::json!({ "cols": self.cols, "rows": rows }).to_string()
    }

    /// Whether the cell at `col`, `row` is drawn in reverse video.
    pub fn inverse(&self, col: usize, row: usize) -> bool {
        let (mutex, _) = &*self.shared;
        let shared = lock(mutex);
        shared.term.grid()[Line(row as i32)][Column(col)].flags.contains(alacritty_terminal::term::cell::Flags::INVERSE)
    }

    /// The cell at `col`, `row` (from 0): its text (with what combines into it), foreground and background, as
    /// alacritty keeps them (`Spec(Rgb { … })` for 24 bits, `Indexed(n)` for the 256-color palette…).
    pub fn cell(&self, col: usize, row: usize) -> (String, String, String) {
        let (mutex, _) = &*self.shared;
        let shared = lock(mutex);
        let cell = &shared.term.grid()[Line(row as i32)][Column(col)];
        let mut text = cell.c.to_string();
        text.extend(cell.zerowidth().into_iter().flatten());
        (text, format!("{:?}", cell.fg), format!("{:?}", cell.bg))
    }
}

impl Drop for TestTerm {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // The program's whole session: it leads one (setsid).
            // SAFETY: a plain signal to a process group the test started.
            unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
            let _ = self.child.wait();
        }
        // The reader ends when the last holder of the slave is gone.
        if let Some(reader) = self.reader.take() {
            let (mutex, _) = &*self.shared;
            if lock(mutex).closed {
                let _ = reader.join();
            }
        }
    }
}

/// The screen of `term` as text.
fn render(term: &Term<Listener>) -> Vec<String> {
    let grid = term.grid();
    (0..grid.screen_lines())
        .map(|line| {
            let row = &grid[Line(line as i32)];
            let mut text = String::new();
            for col in 0..grid.columns() {
                let cell = &row[Column(col)];
                // A wide character's second cell holds nothing of its own.
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                text.push(cell.c);
                text.extend(cell.zerowidth().into_iter().flatten());
            }
            text.trim_end().to_string()
        })
        .collect()
}

/// The reader thread: what the program writes, into the `Term`; the `Term`'s answers, back to the program.
fn read(mut master: File, mut replies: File, shared: &(Mutex<Shared>, Condvar), events: &Listener) {
    let (mutex, condvar) = shared;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match master.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut guard = lock(mutex);
        let state = &mut *guard;
        state.bytes += n as u64;
        if let Some(raw) = &mut state.raw {
            raw.extend_from_slice(&buf[..n]);
        }
        state.frames += count(&buf[..n], b"\x1b[?2026h");
        state.parser.advance(&mut state.term, &buf[..n]);
        if let Some((text, seen @ None)) = &mut state.watch
            && render(&state.term).iter().any(|row| row.contains(text.as_str()))
        {
            *seen = Some(Instant::now());
        }
        let xtversion = state
            .xtversion
            .filter(|_| count(&buf[..n], b"\x1b[>q") + count(&buf[..n], b"\x1b[>0q") > 0)
            .map(|name| format!("\x1bP>|{name}\x1b\\"));
        let pending: Vec<Event> = std::mem::take(&mut *lock(&events.0));
        drop(guard);
        if let Some(answer) = xtversion {
            let _ = replies.write_all(answer.as_bytes());
        }
        for event in pending {
            let answer = match event {
                Event::PtyWrite(text) => text,
                // OSC 10 asks for `NamedColor::Foreground` (256), OSC 11 for `Background` (257).
                Event::ColorRequest(index, format) => {
                    format(if index == NamedColor::Background as usize { BG } else { FG })
                }
                _ => continue,
            };
            let _ = replies.write_all(answer.as_bytes());
        }
        condvar.notify_all();
    }
    lock(mutex).closed = true;
    condvar.notify_all();
}

/// How many times `needle` appears in `haystack` (a frame start split across two reads is missed: rare, and
/// only the frame counts suffer).
fn count(haystack: &[u8], needle: &[u8]) -> u64 {
    haystack.windows(needle.len()).filter(|w| *w == needle).count() as u64
}

/// A new PTY pair of `cols` × `rows`, both ends close-on-exec.
fn openpty(cols: u16, rows: u16) -> (OwnedFd, OwnedFd) {
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: out-parameters for two descriptors, and a size that outlives the call.
    let rc =
        unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &raw mut size) };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    for fd in [master, slave] {
        // SAFETY: a descriptor this function owns.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // SAFETY: two fresh descriptors, owned from here on.
    unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
}

/// Puts this process's terminal (stdin) in raw mode, for the fakes that read keys.
pub fn raw_stdin() {
    // SAFETY: a termios read, changed and written back on stdin.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut termios) == 0 {
            libc::cfmakeraw(&mut termios);
            libc::tcsetattr(0, libc::TCSANOW, &termios);
        }
    }
}
