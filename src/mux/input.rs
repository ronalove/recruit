// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Input (spec §5.4). On the client's side, the real terminal set up after asking what it can do, and its bytes
//! read into our own events ([`Terminal`], [`Event`]); on the server's, each event encoded for the pane it goes to,
//! as that pane's modes ask ([`key`], [`mouse`], [`paste`], [`focus`]). The events are plain data, for the protocol
//! to carry at step 1.
//!
//! We read the terminal's bytes ourselves rather than through crossterm's events: crossterm reads a late answer to
//! a query as keys (`ESC ]` as Alt+]), and loses what a pane may need (0x08 read as Backspace, the kitty protocol's
//! event types).
//!
//! Owner: dev-saisie.

mod encode;
mod parse;

use std::fs::File;
use std::io::{self, Read, Stdout, Write};
use std::os::fd::{AsFd, AsRawFd, IntoRawFd, OwnedFd, RawFd};
use std::panic::{self, PanicHookInfo};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) use encode::{focus, key, mouse, paste};
use parse::{Parser, Reply, Token};
use serde::{Deserialize, Serialize};

use super::engine::MouseTracking;
use super::{Caps, Rgb};
use crate::canvas::Features;
use crate::t;

/// The events `bytes` read into, as the reader would once nothing more came; for the tests of what uses them.
#[cfg(test)]
pub(crate) fn read_bytes(bytes: &[u8]) -> Vec<Event> {
    let mut parser = Parser::default();
    let mut tokens = Vec::new();
    parser.feed(bytes, &mut tokens);
    parser.flush(&mut tokens);
    tokens
        .into_iter()
        .filter_map(|token| match token {
            Token::Event(event) => Some(event),
            Token::Reply(_) => None,
        })
        .collect()
}

/// What the real terminal sends, read. With its parts, carried by the protocol as they are (serde's default form,
/// in JSON): a change of their shape is a change of the protocol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Event {
    Key(Key),
    Mouse(Mouse),
    Paste(String),
    /// The terminal's window gained (true) or lost the focus.
    Focus(bool),
    /// Its new size, columns and rows.
    Resize(u16, u16),
    /// No event will come any more, and why: the terminal is gone, or reading it failed. The last one sent.
    Closed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Key {
    /// A character key holds its text (`A` for Shift+a typed as text), or its unshifted key when it came as an
    /// escape code with other modifiers (`a` with Ctrl and Shift).
    pub code: KeyCode,
    pub mods: Mods,
    pub kind: KeyKind,
}

impl Key {
    pub(crate) fn new(code: KeyCode, mods: Mods) -> Key {
        Key { code, mods, kind: KeyKind::Press }
    }

    /// A press, or a repeat: a key that does something.
    pub(crate) fn is_press(&self) -> bool {
        self.kind != KeyKind::Release
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum KeyCode {
    Char(char),
    Enter,
    /// Shift+Tab is Tab with [`Mods::SHIFT`].
    Tab,
    Backspace,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1 to F35.
    F(u8),
}

/// Modifiers, with the kitty protocol's bits: shift 1, alt 2, ctrl 4, super 8, hyper 16, meta 32. Caps Lock and
/// Num Lock are not modifiers here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Mods(pub u8);

impl Mods {
    pub(crate) const NONE: Mods = Mods(0);
    pub(crate) const SHIFT: Mods = Mods(1);
    pub(crate) const ALT: Mods = Mods(2);
    pub(crate) const CTRL: Mods = Mods(4);
    pub(crate) const SUPER: Mods = Mods(8);

    pub(crate) fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Mods {
    type Output = Mods;

    fn bitor(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }
}

/// Only presses come while the real terminal is asked for nothing more than the kitty protocol's first flag; the
/// kinds are there for the panes that ask for them, and for a later choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum KeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Mouse {
    pub kind: MouseKind,
    /// Cell of the screen, from 0.
    pub col: u16,
    pub row: u16,
    pub mods: Mods,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MouseKind {
    Down(Button),
    Up(Button),
    /// A move with that button down.
    Drag(Button),
    /// A move without a button: the real terminal is not asked for these (1003), see [`Terminal::open`].
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Button {
    Left,
    Middle,
    Right,
}

/// The queries of the probe, written together: the kitty keyboard protocol's flags, DECRQM for synchronized updates
/// (2026) and grapheme clusters (2027), the default colors (OSC 10, 11), the name (XTVERSION); DA1 last, which
/// every terminal answers, so that its answer ends the probe.
const QUERIES: &[u8] = b"\x1b[?u\x1b[?2026$p\x1b[?2027$p\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[>0q\x1b[c";

/// The longest the probe waits for DA1's answer; a terminal answers in a few milliseconds, a few tens by SSH.
const PROBE_TIME: Duration = Duration::from_millis(500);

/// The modes the multiplexer sets on the real terminal: alternate screen, no cursor (the painter shows the focused
/// pane's), mouse reports (presses, releases, the wheel and drags, in SGR form; not the moves without a button,
/// which would wake us at each one), bracketed pastes, focus reports.
const ENTER: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?1004h";

/// The kitty keyboard protocol, flag 1 only (disambiguate): Esc comes as `CSI 27 u`, at once and never mistaken
/// for the start of a sequence; Alt and Ctrl combinations, Shift+Enter and Shift+Tab come apart; text stays text,
/// so that what a layout, dead keys or an input method compose arrives whole (flag 8 would send key codes, and the
/// text only with 16). Not 2: Claude Code asks for no releases. Not 4: the shifted key is told by the text.
/// Pushed on the alternate screen, which has a stack of its own.
const KITTY_PUSH: &[u8] = b"\x1b[>1u";

/// Everything given back as it was: the kitty flags popped while still on the alternate screen, the reports off, a
/// synchronized update or a link the painter left open closed, cursor shown with the terminal's own shape, colors
/// reset, the main screen back.
const LEAVE: &[u8] = b"\x1b[?1004l\x1b[?2004l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?2026l\x1b]8;;\x1b\\\x1b[0m\x1b[0 q\x1b[?25h\x1b[?1049l";

/// Without the kitty protocol, xterm's modifyOtherKeys at level 1: the keys that have no legacy form of their own
/// (Shift+Enter, Ctrl+Enter) come as `CSI 27 ; mods ; code ~` or `CSI code ; mods u`, the rest as before. xterm and
/// tmux (`extended-keys`) do it; the others leave it.
const MODIFY_OTHER_KEYS: &[u8] = b"\x1b[>4;1m";

/// Which of the two was asked for: undone only then (`CSI < u` reads as something else elsewhere).
static KITTY_PUSHED: AtomicBool = AtomicBool::new(false);
static MODIFY_SET: AtomicBool = AtomicBool::new(false);

/// Whether mode 2027 (grapheme clusters) was on, and turned off: turned on again only then.
static GRAPHEMES_OFF: AtomicBool = AtomicBool::new(false);
/// Whether it was off, and turned on: turned off again only then.
static GRAPHEMES_ON: AtomicBool = AtomicBool::new(false);

/// The real terminal while the multiplexer is on it: raw, alternate screen, cursor hidden, mouse (presses, drags,
/// wheel; SGR), bracketed paste, focus changes, the kitty keyboard protocol when it has it. Dropping it gives the
/// terminal back as it was; a panic of the thread that opened it does it before its message. A panic of another
/// thread writes nothing on the screen: its message is told once the terminal is given back; the reading thread's
/// own ends with [`Event::Closed`].
pub(crate) struct Terminal {
    out: Stdout,
    input: File,
    /// What the probe read past the answers: keys typed meanwhile, the start of a sequence.
    parser: Parser,
    early: Vec<Event>,
    /// Wakes the reading thread so that it ends.
    stop: Option<OwnedFd>,
    /// Every move reported (1003), for the focused pane that asks for them.
    motion: bool,
    /// How it was set up, for [`Terminal::resume`].
    setup: Option<Setup>,
    /// Given back for a while ([`Terminal::suspend`]).
    suspended: bool,
    _panic: Guard,
}

impl Terminal {
    /// Sets the terminal up, after asking what it can do (see [`Caps`]); grapheme clusters (2027) off, see
    /// [`Terminal::open_with`].
    pub(crate) fn open() -> io::Result<(Terminal, Caps)> {
        Terminal::open_with(false)
    }

    /// The same, with grapheme clusters (mode 2027) measured as the engine measures them, where the terminal knows the
    /// mode: on when `graphemes` (an engine that measures them whole), off otherwise (Ghostty has them on by
    /// default: an engine that measures code point by code point gives 👍🏽 two wide cells, which the terminal would
    /// join into one, and the rest of the line would move left). Given back as it was.
    pub(crate) fn open_with(graphemes: bool) -> io::Result<(Terminal, Caps)> {
        let input = tty()?;
        crossterm::terminal::enable_raw_mode()?;
        // From here, Drop puts the terminal back.
        let mut terminal = Terminal {
            out: io::stdout(),
            input,
            parser: Parser::default(),
            early: Vec::new(),
            stop: None,
            motion: false,
            setup: None,
            suspended: false,
            _panic: Guard::install(),
        };
        let answers = terminal.probe()?;
        let caps = caps(&answers, |name| std::env::var(name).ok());
        // Hidden: what the probe heard, for the terminals' grid (specs/multiplexeur-compat.md).
        if let Some(path) = std::env::var_os("RECRUIT_MUX_CAPS") {
            let _ = std::fs::write(path, report(&answers, &caps, |name| std::env::var(name).ok()));
        }
        let setup = setup(&answers, &caps, graphemes);
        let mut caps = caps;
        caps.output.graphemes = setup.graphemes_active;
        setup.arm();
        terminal.write(&setup.bytes)?;
        terminal.setup = Some(setup);
        Ok((terminal, caps))
    }

    /// Gives the terminal back as it was, as on drop, for a while (SIGTSTP): the reading thread goes on, the probe's
    /// findings stay. Nothing must be written until [`Terminal::resume`].
    pub(crate) fn suspend(&mut self) -> io::Result<()> {
        if self.suspended {
            return Ok(());
        }
        self.suspended = true;
        leave();
        Ok(())
    }

    /// Sets the terminal up again as [`Terminal::open`] did, without probing it again, the mouse mode included.
    /// What was on the screen is gone: the next frame must be whole.
    pub(crate) fn resume(&mut self) -> io::Result<()> {
        if !self.suspended {
            return Ok(());
        }
        crossterm::terminal::enable_raw_mode()?;
        self.suspended = false;
        let Some(setup) = &self.setup else { return Ok(()) };
        setup.arm();
        let mut bytes = setup.bytes.clone();
        if self.motion {
            bytes.extend_from_slice(mouse_change(true));
        }
        self.write(&bytes)
    }

    /// Writes the queries and reads until DA1's answer, or [`PROBE_TIME`].
    fn probe(&mut self) -> io::Result<Answers> {
        self.write(QUERIES)?;
        let start = Instant::now();
        let deadline = start + PROBE_TIME;
        let mut answers = Answers::default();
        let mut buf = [0; 4096];
        let mut tokens = Vec::new();
        while !answers.done {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut fds = [pollfd(self.input.as_raw_fd())];
            if poll(&mut fds, Some(left))? == 0 {
                break;
            }
            let n = match self.input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            answers.raw.extend_from_slice(&buf[..n]);
            self.parser.feed(&buf[..n], &mut tokens);
            for token in tokens.drain(..) {
                match token {
                    Token::Reply(reply) => answers.note(reply),
                    Token::Event(event) => self.early.push(event),
                }
            }
        }
        answers.took = start.elapsed();
        Ok(answers)
    }

    /// The terminal's size, columns and rows, as its window has it now; later changes come as [`Event::Resize`].
    pub(crate) fn size(&self) -> Option<(u16, u16)> {
        size(self.input.as_raw_fd())
    }

    /// Follows the mouse mode of the pane that has the focus: the moves without a button (1003) are asked of the
    /// terminal only while that pane takes them, since each one wakes us. Presses, drags and the wheel always come,
    /// for the interface. Writes only on a change.
    pub(crate) fn set_mouse(&mut self, tracking: Option<MouseTracking>) -> io::Result<()> {
        let motion = tracking == Some(MouseTracking::Motion);
        if motion == self.motion {
            return Ok(());
        }
        self.motion = motion;
        // Suspended, [`Terminal::resume`] asks for it.
        if self.suspended {
            return Ok(());
        }
        self.write(mouse_change(motion))
    }

    /// Writes a frame, or a relay, in one write.
    pub(crate) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.out.write_all(bytes)?;
        self.out.flush()
    }

    /// Reads the terminal's events on a thread of its own and hands each to `send`, until it returns false or the
    /// terminal is dropped; when the terminal goes, or reading it fails or panics, [`Event::Closed`] last. Once.
    pub(crate) fn read_events(&mut self, mut send: impl FnMut(Event) -> bool + Send + 'static) -> io::Result<()> {
        let input = self.input.try_clone()?;
        let (stop, stop_write) = pipe()?;
        let winch = winch()?;
        self.stop = Some(stop_write);
        let mut parser = std::mem::take(&mut self.parser);
        let early = std::mem::take(&mut self.early);
        thread::Builder::new().name("input".into()).spawn(move || {
            for event in early {
                if !send(event) {
                    return;
                }
            }
            let reason = match super::caught(|| read(input, stop, winch, &mut parser, &mut send)) {
                Ok(Ok(End::Stopped)) => return,
                Ok(Ok(End::Gone)) => t!("le terminal est fermé", "the terminal closed"),
                Ok(Err(error)) => t!("lecture du terminal : {}", "reading the terminal: {}", error),
                Err(panic) => t!("lecture du terminal : {}", "reading the terminal: {}", panic),
            };
            send(Event::Closed(reason));
        })?;
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = File::from(stop).write_all(b"x");
        }
        // A panic of this thread: its hook did it already. Suspended: given back already.
        if !thread::panicking() {
            if !self.suspended {
                leave();
            }
            tell_panics();
        }
    }
}

/// The panics of other threads than the terminal's, which nothing caught: kept while the screen is ours, told on
/// the terminal given back.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn tell_panics() {
    let panics = std::mem::take(&mut *PANICS.lock().unwrap_or_else(|e| e.into_inner()));
    for panic in panics {
        eprintln!("{panic}");
    }
}

/// Every move reported, or back to presses and drags: xterm keeps one mouse mode, which turning 1003 off turns off
/// whole, so 1002 is asked again.
fn mouse_change(motion: bool) -> &'static [u8] {
    if motion { b"\x1b[?1003h" } else { b"\x1b[?1003l\x1b[?1002h" }
}

/// How the terminal is set up, and what to undo.
#[derive(Debug, PartialEq, Eq)]
struct Setup {
    bytes: Vec<u8>,
    kitty: bool,
    modify: bool,
    graphemes_off: bool,
    graphemes_on: bool,
    /// Mode 2027 on once set up: the terminal measures grapheme clusters whole (`Features::graphemes`).
    graphemes_active: bool,
}

impl Setup {
    /// Says what [`leave`] will have to undo.
    fn arm(&self) {
        KITTY_PUSHED.store(self.kitty, Ordering::SeqCst);
        MODIFY_SET.store(self.modify, Ordering::SeqCst);
        GRAPHEMES_OFF.store(self.graphemes_off, Ordering::SeqCst);
        GRAPHEMES_ON.store(self.graphemes_on, Ordering::SeqCst);
    }
}

fn setup(answers: &Answers, caps: &Caps, graphemes: bool) -> Setup {
    let mut bytes = ENTER.to_vec();
    let kitty = caps.kitty_keyboard;
    bytes.extend_from_slice(if kitty { KITTY_PUSH } else { MODIFY_OTHER_KEYS });
    // Measured as the engine measures: turned off when set (1) and on when reset (2); set or reset for good (3, 4),
    // or unknown, it stays.
    let graphemes_off = !graphemes && answers.modes.contains(&(2027, 1));
    let graphemes_on = graphemes && answers.modes.contains(&(2027, 2));
    if graphemes_off {
        bytes.extend_from_slice(b"\x1b[?2027l");
    }
    if graphemes_on {
        bytes.extend_from_slice(b"\x1b[?2027h");
    }
    let graphemes_active = answers.modes.contains(&(2027, 3))
        || (graphemes && (answers.modes.contains(&(2027, 1)) || answers.modes.contains(&(2027, 2))));
    Setup { bytes, kitty, modify: !kitty, graphemes_off, graphemes_on, graphemes_active }
}

/// The terminal as before: see [`LEAVE`]. Out of raw mode last.
fn leave() {
    let mut out = io::stdout();
    if KITTY_PUSHED.swap(false, Ordering::SeqCst) {
        let _ = out.write_all(b"\x1b[<u");
    }
    if MODIFY_SET.swap(false, Ordering::SeqCst) {
        let _ = out.write_all(b"\x1b[>4m");
    }
    if GRAPHEMES_OFF.swap(false, Ordering::SeqCst) {
        let _ = out.write_all(b"\x1b[?2027h");
    }
    if GRAPHEMES_ON.swap(false, Ordering::SeqCst) {
        let _ = out.write_all(b"\x1b[?2027l");
    }
    let _ = out.write_all(LEAVE);
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
}

/// The terminal to read: standard input if it is one, else the process's own (`/dev/tty`).
fn tty() -> io::Result<File> {
    let stdin = io::stdin();
    if unsafe { libc::isatty(stdin.as_raw_fd()) } == 1 {
        return Ok(File::from(stdin.as_fd().try_clone_to_owned()?));
    }
    File::options().read(true).write(true).open("/dev/tty")
}

fn pollfd(fd: RawFd) -> libc::pollfd {
    libc::pollfd { fd, events: libc::POLLIN, revents: 0 }
}

/// How many of `fds` are ready, waiting `timeout` at most (`None`: as long as it takes).
fn poll(fds: &mut [libc::pollfd], timeout: Option<Duration>) -> io::Result<usize> {
    let ms = timeout.map_or(-1, |t| t.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32);
    loop {
        // SAFETY: `fds` is a valid, writable array of `fds.len()` pollfd.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
        if ready >= 0 {
            return Ok(ready as usize);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// A pipe, closed on exec (std's, `pipe2` where there is one), both ends non-blocking: (read, write).
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let (read, write) = io::pipe()?;
    let (read, write) = (OwnedFd::from(read), OwnedFd::from(write));
    for fd in [&read, &write] {
        // SAFETY: fcntl on a descriptor we own.
        let ok = unsafe {
            let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
            flags >= 0 && libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
        };
        if !ok {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read, write))
}

/// Where SIGWINCH's handler writes: the write end of [`WINCH`]'s pipe.
static WINCH_WRITE: AtomicI32 = AtomicI32::new(-1);

/// The read end of the pipe that says the terminal changed size, made once for the process.
static WINCH: Mutex<Option<RawFd>> = Mutex::new(None);

/// The SIGWINCH handler in place before ours (signal-hook's, if crossterm read events in this process), and its
/// flags: ours calls it after its own work.
static WINCH_BEFORE: AtomicUsize = AtomicUsize::new(libc::SIG_DFL);
static WINCH_BEFORE_FLAGS: AtomicI32 = AtomicI32::new(0);

#[cfg(target_os = "macos")]
fn errno() -> *mut libc::c_int {
    // SAFETY: the calling thread's errno.
    unsafe { libc::__error() }
}

#[cfg(not(target_os = "macos"))]
fn errno() -> *mut libc::c_int {
    // SAFETY: the calling thread's errno.
    unsafe { libc::__errno_location() }
}

extern "C" fn on_winch(signal: libc::c_int, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    // SAFETY: only async-signal-safe calls (write(2), the handler before); errno is left as it was.
    unsafe {
        let saved = *errno();
        let fd = WINCH_WRITE.load(Ordering::Relaxed);
        if fd >= 0 {
            // A full pipe already says it all.
            libc::write(fd, b"w".as_ptr().cast(), 1);
        }
        let before = WINCH_BEFORE.load(Ordering::Relaxed);
        if before != libc::SIG_DFL && before != libc::SIG_IGN {
            if WINCH_BEFORE_FLAGS.load(Ordering::Relaxed) & libc::SA_SIGINFO != 0 {
                let before: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                    std::mem::transmute(before);
                before(signal, info, context);
            } else {
                let before: extern "C" fn(libc::c_int) = std::mem::transmute(before);
                before(signal);
            }
        }
        *errno() = saved;
    }
}

fn winch() -> io::Result<RawFd> {
    let mut winch = WINCH.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(fd) = *winch {
        return Ok(fd);
    }
    let (read, write) = pipe()?;
    WINCH_WRITE.store(write.into_raw_fd(), Ordering::SeqCst);
    // SAFETY: zeroed sigactions are valid; the handler writes to a pipe and calls the one before.
    unsafe {
        let mut before: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGWINCH, std::ptr::null(), &mut before) != 0 {
            return Err(io::Error::last_os_error());
        }
        WINCH_BEFORE.store(before.sa_sigaction, Ordering::SeqCst);
        WINCH_BEFORE_FLAGS.store(before.sa_flags, Ordering::SeqCst);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_winch as extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) as usize;
        action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let fd = read.into_raw_fd();
    *winch = Some(fd);
    Ok(fd)
}

/// The size of the terminal behind `fd`, columns and rows.
fn size(fd: RawFd) -> Option<(u16, u16)> {
    // SAFETY: TIOCGWINSZ fills a winsize.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) } == 0;
    (ok && size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
}

/// How the reading ended.
#[derive(Debug, PartialEq, Eq)]
enum End {
    /// Asked to: the terminal dropped, or `send` said so.
    Stopped,
    /// The terminal is gone (end of file, hang-up).
    Gone,
}

/// The reading thread: the terminal's bytes, the size changes, the end.
fn read(
    mut input: File,
    stop: OwnedFd,
    winch: RawFd,
    parser: &mut Parser,
    send: &mut impl FnMut(Event) -> bool,
) -> io::Result<End> {
    let mut buf = vec![0; 1 << 16];
    let mut tokens = Vec::new();
    loop {
        let mut fds = [pollfd(input.as_raw_fd()), pollfd(stop.as_raw_fd()), pollfd(winch)];
        let ready = poll(&mut fds, parser.pending())?;
        if fds[1].revents != 0 {
            return Ok(End::Stopped);
        }
        if ready == 0 {
            parser.flush(&mut tokens);
        }
        if fds[2].revents & libc::POLLIN != 0 {
            let mut drain = [0; 64];
            // SAFETY: reads into a buffer of that size; the pipe does not block.
            while unsafe { libc::read(winch, drain.as_mut_ptr().cast(), drain.len()) } > 0 {}
            if let Some((cols, rows)) = size(input.as_raw_fd()) {
                tokens.push(Token::Event(Event::Resize(cols, rows)));
            }
        }
        if fds[0].revents & libc::POLLNVAL != 0 {
            return Ok(End::Gone);
        }
        if fds[0].revents != 0 {
            match input.read(&mut buf) {
                Ok(0) => return Ok(End::Gone),
                // What a terminal that went away answers.
                Err(e) if e.raw_os_error() == Some(libc::EIO) => return Ok(End::Gone),
                Ok(n) => parser.feed(&buf[..n], &mut tokens),
                Err(e) if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) => {}
                Err(e) => return Err(e),
            }
        }
        for token in tokens.drain(..) {
            // Late answers to the probe are dropped here.
            if let Token::Event(event) = token
                && !send(event)
            {
                return Ok(End::Stopped);
            }
        }
    }
}

/// What the probe heard.
#[derive(Debug, Default)]
struct Answers {
    kitty: Option<u32>,
    /// DECRPM: mode and its state.
    modes: Vec<(u32, u32)>,
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    name: Option<String>,
    /// DA1 came: nothing more will.
    done: bool,
    /// Everything read, and how long it took, for [`report`].
    raw: Vec<u8>,
    took: Duration,
}

/// What the probe heard, written where `RECRUIT_MUX_CAPS` says (hidden; read when the native client opens the
/// terminal, `client.rs`), for the terminals' grid: the terminal's environment, its answers escaped, a sequence a
/// line, and what was made of them.
fn report(answers: &Answers, caps: &Caps, env: impl Fn(&str) -> Option<String>) -> String {
    let escaped = |bytes: &[u8]| bytes.escape_ascii().to_string();
    let mut out = format!(
        "recruit {} terminal probe\ntook: {} ms; DA1 answered: {}\n\nenvironment:\n",
        env!("CARGO_PKG_VERSION"),
        answers.took.as_millis(),
        if answers.done { "yes" } else { "no" },
    );
    for name in ["TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "COLORTERM", "VTE_VERSION", "LC_TERMINAL", "TMUX"] {
        out += &format!("  {name}={}\n", env(name).unwrap_or_else(|| "(unset)".into()));
    }
    out += &format!("  over SSH: {}\n", if env("SSH_TTY").is_some() { "yes" } else { "no" });
    out += &format!("\nqueries:\n  {}\n\nanswers:\n", escaped(QUERIES));
    let mut starts: Vec<usize> = answers
        .raw
        .iter()
        .enumerate()
        .filter(|&(i, &b)| b == 0x1b && answers.raw.get(i + 1) != Some(&b'\\'))
        .map(|(i, _)| i)
        .collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(answers.raw.len());
        if start < end {
            out += &format!("  {}\n", escaped(&answers.raw[start..end]));
        }
    }
    out += &format!("\ncaps: {caps:#?}\n");
    out
}

impl Answers {
    fn note(&mut self, reply: Reply) {
        match reply {
            Reply::Kitty(flags) => self.kitty = Some(flags),
            Reply::Mode { mode, value } => self.modes.push((mode, value)),
            Reply::Osc(body) => {
                let body = String::from_utf8_lossy(&body);
                match body.split_once(';') {
                    Some(("10", spec)) => self.fg = rgb(spec),
                    Some(("11", spec)) => self.bg = rgb(spec),
                    _ => {}
                }
            }
            Reply::Dcs(body) => {
                if let Some(name) = body.strip_prefix(b">|") {
                    let name = String::from_utf8_lossy(name).trim().to_string();
                    self.name = (!name.is_empty()).then_some(name);
                }
            }
            Reply::Primary => self.done = true,
            Reply::Other => {}
        }
    }

    /// DECRPM says the mode can be set: set (1), reset (2) or set for good (3); 0 is unknown, 4 reset for good.
    fn settable(&self, mode: u32) -> bool {
        self.modes.iter().any(|&(m, value)| m == mode && (1..=3).contains(&value))
    }
}

/// An X11 color spec as terminals answer OSC 10 and 11: `rgb:r/g/b`, 1 to 4 hex digits each.
fn rgb(spec: &str) -> Option<Rgb> {
    let parts = spec.strip_prefix("rgb:").or_else(|| spec.strip_prefix("rgba:"))?;
    let mut channels = parts.split('/').map(|part| {
        let value = u32::from_str_radix(part, 16).ok().filter(|_| (1..=4).contains(&part.len()))?;
        let max = (1u32 << (4 * part.len())) - 1;
        Some(((value * 255 + max / 2) / max) as u8)
    });
    Some((channels.next()??, channels.next()??, channels.next()??))
}

/// Terminals known to show 24-bit colors and OSC 8 links, by the start of their name (XTVERSION, `TERM_PROGRAM`,
/// `TERM`), lower case. tmux takes both and passes them on, or not, as its own terminal can.
const KNOWN: &[&str] = &[
    "ghostty",
    "kitty",
    "xterm-kitty",
    "wezterm",
    "iterm",
    "foot",
    "tmux",
    "alacritty",
    "contour",
    "rio",
    "vscode",
    "warpterminal",
    "konsole",
];

/// What the terminal can do. Synchronized updates and grapheme clusters, from DECRPM. 24-bit colors and links have
/// no query: a terminal known by name has them, or one that says so (`COLORTERM`, a `-direct` `TERM`; VTE from 0.50
/// for links); otherwise 256 colors and no links, which every terminal shows right.
fn caps(answers: &Answers, env: impl Fn(&str) -> Option<String>) -> Caps {
    let names: Vec<String> = [answers.name.clone(), env("TERM_PROGRAM"), env("TERM")]
        .into_iter()
        .flatten()
        .map(|name| name.to_lowercase())
        .collect();
    let known = names.iter().any(|name| KNOWN.iter().any(|known| name.starts_with(known)));
    let colorterm = env("COLORTERM").is_some_and(|value| matches!(value.as_str(), "truecolor" | "24bit"));
    let direct = env("TERM").is_some_and(|term| term.ends_with("-direct"));
    let vte = env("VTE_VERSION").and_then(|v| v.parse::<u32>().ok()).is_some_and(|v| v >= 5000);
    Caps {
        output: Features {
            sync: answers.settable(2026),
            truecolor: known || colorterm || direct,
            links: known || vte,
            // Known once the terminal is set up: see `setup`.
            graphemes: false,
        },
        kitty_keyboard: answers.kitty.is_some(),
        graphemes: answers.settable(2027),
        fg: answers.fg,
        bg: answers.bg,
        name: answers.name.clone(),
    }
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// While it lives, a panic of the thread that installed it gives the terminal back, then runs the hook in place
/// before. A panic that [`super::caught`] handles runs nothing: its message is shown elsewhere. The message of a
/// panic of another thread is kept, and told when the terminal is given back: nothing is written over the screen.
/// Dropping it puts the hook before back.
struct Guard {
    before: Option<Arc<Hook>>,
}

impl Guard {
    fn install() -> Guard {
        let owner = thread::current().id();
        let before = Arc::new(panic::take_hook());
        let hook = Arc::clone(&before);
        panic::set_hook(Box::new(move |info| {
            if super::catching() {
                return;
            }
            let current = thread::current();
            if current.id() == owner {
                leave();
                tell_panics();
                hook(info);
            } else {
                let message = format!("thread '{}' {info}", current.name().unwrap_or("<unnamed>"));
                PANICS.lock().unwrap_or_else(|e| e.into_inner()).push(message);
            }
        }));
        Guard { before: Some(before) }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // A panicking thread cannot change the hook: this one stays.
        if thread::panicking() {
            return;
        }
        drop(panic::take_hook());
        if let Some(before) = self.before.take().and_then(|before| Arc::try_unwrap(before).ok()) {
            panic::set_hook(before);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(bytes: &[u8]) -> (Answers, Vec<Event>) {
        let mut parser = Parser::default();
        let mut tokens = Vec::new();
        parser.feed(bytes, &mut tokens);
        let mut answers = Answers::default();
        let mut events = Vec::new();
        for token in tokens {
            match token {
                Token::Reply(reply) => answers.note(reply),
                Token::Event(event) => events.push(event),
            }
        }
        (answers, events)
    }

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    #[test]
    fn colors() {
        let table = [
            ("rgb:1e1e/1e1e/2e2e", Some((30, 30, 46))),
            ("rgb:ffff/ffff/ffff", Some((255, 255, 255))),
            ("rgb:00/80/ff", Some((0, 128, 255))),
            ("rgb:f/0/8", Some((255, 0, 136))),
            ("rgba:ffff/0000/0000/ffff", Some((255, 0, 0))),
            ("rgb:12345/0/0", None),
            ("#ffffff", None),
            ("rgb:ff/ff", None),
        ];
        for (spec, color) in table {
            assert_eq!(rgb(spec), color, "{spec}");
        }
    }

    #[test]
    fn ghostty_answers_everything() {
        // Ghostty 1.2's answers to QUERIES, with a key typed meanwhile.
        let (answers, events) = heard(
            b"\x1b[?0u\x1b[?2026;2$y\x1b[?2027;2$yx\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b]11;rgb:2828/2c2c/3434\x1b\\\
              \x1bP>|ghostty 1.2.0\x1b\\\x1b[?62;22;52c",
        );
        assert!(answers.done);
        assert_eq!(events, [Event::Key(Key::new(KeyCode::Char('x'), Mods::NONE))]);
        let caps = caps(&answers, env(&[]));
        assert_eq!(
            caps,
            Caps {
                output: Features { sync: true, truecolor: true, links: true, graphemes: false },
                kitty_keyboard: true,
                graphemes: true,
                fg: Some((255, 255, 255)),
                bg: Some((40, 44, 52)),
                name: Some("ghostty 1.2.0".into()),
            }
        );
    }

    #[test]
    fn a_plain_terminal_answers_da1_only() {
        // Terminal.app: DA1, and nothing else it knows.
        let (answers, events) = heard(b"\x1b[?1;2c");
        assert!(answers.done && events.is_empty());
        let caps = caps(&answers, env(&[("TERM_PROGRAM", "Apple_Terminal"), ("TERM", "xterm-256color")]));
        assert_eq!(
            caps,
            Caps {
                output: Features { sync: false, truecolor: false, links: false, graphemes: false },
                kitty_keyboard: false,
                graphemes: false,
                fg: None,
                bg: None,
                name: None,
            }
        );
        // It says it has 24-bit colors.
        assert!(super::caps(&answers, env(&[("COLORTERM", "truecolor")])).output.truecolor);
        assert!(super::caps(&answers, env(&[("TERM", "xterm-direct")])).output.truecolor);
        let vte = super::caps(&answers, env(&[("VTE_VERSION", "7600"), ("COLORTERM", "truecolor")]));
        assert!(vte.output.links && vte.output.truecolor);
        // Known by name, through SSH (no TERM_PROGRAM, but TERM and XTVERSION).
        assert!(super::caps(&answers, env(&[("TERM", "xterm-kitty")])).output.links);
        let (named, _) = heard(b"\x1bP>|WezTerm 20240203\x1b\\\x1b[?1;2c");
        assert!(super::caps(&named, env(&[])).output.links);
    }

    #[test]
    fn the_probe_report() {
        let (mut answers, _) = heard(b"\x1b[?0u\x1b[?1;2c");
        answers.raw = b"\x1b[?0u\x1b[?1;2c".to_vec();
        let caps = caps(&answers, env(&[]));
        let report = report(&answers, &caps, env(&[("TERM", "xterm-256color")]));
        assert!(report.contains("DA1 answered: yes"), "{report}");
        assert!(report.contains("  TERM=xterm-256color\n  TERM_PROGRAM=(unset)\n"), "{report}");
        assert!(report.contains("answers:\n  \\x1b[?0u\n  \\x1b[?1;2c\n"), "{report}");
        assert!(report.contains("kitty_keyboard: true"), "{report}");
    }

    #[test]
    fn grapheme_clusters_turned_off_only_where_on() {
        let ends = |setup: &Setup, tail: &[u8]| setup.bytes.ends_with(tail);
        // Ghostty 1.3.1: 2027 on by default.
        let (ghostty, _) = heard(b"\x1b[?0u\x1b[?2026;2$y\x1b[?2027;1$y\x1b[?62;22;52c");
        let caps = caps(&ghostty, env(&[]));
        let off = setup(&ghostty, &caps, false);
        assert!(off.graphemes_off && off.kitty && !off.modify && !off.graphemes_active);
        assert!(ends(&off, b"\x1b[>1u\x1b[?2027l"), "{:?}", off.bytes.escape_ascii().to_string());
        // Kept on for an engine that measures them whole.
        let kept = setup(&ghostty, &caps, true);
        assert!(!kept.graphemes_off && !kept.graphemes_on && kept.graphemes_active && ends(&kept, KITTY_PUSH));
        // Known but off (2): turned on for such an engine, left off otherwise.
        let (known, _) = heard(b"\x1b[?2027;2$y");
        let known_caps = super::caps(&known, env(&[]));
        let on = setup(&known, &known_caps, true);
        assert!(on.graphemes_on && !on.graphemes_off && on.graphemes_active && ends(&on, b"\x1b[?2027h"));
        let left = setup(&known, &known_caps, false);
        assert!(!left.graphemes_on && !left.graphemes_off && !left.graphemes_active);
        assert!(ends(&left, MODIFY_OTHER_KEYS));
        // For good (3, 4), unknown (0), or not answered (Terminal.app): nothing to do, either way.
        for answer in [&b"\x1b[?2027;3$y"[..], b"\x1b[?2027;4$y", b"\x1b[?2027;0$y", b""] {
            let (answers, _) = heard(answer);
            for graphemes in [false, true] {
                let plain = setup(&answers, &super::caps(&answers, env(&[])), graphemes);
                assert!(!plain.graphemes_off && !plain.graphemes_on, "{answer:?} {graphemes}");
                assert!(plain.modify && ends(&plain, MODIFY_OTHER_KEYS), "{answer:?} {graphemes}");
                // On for good, the terminal measures them whole whatever we ask; off for good or unknown, not.
                assert_eq!(plain.graphemes_active, answer == b"\x1b[?2027;3$y", "{answer:?} {graphemes}");
            }
        }
    }

    #[test]
    fn every_move_only_for_a_pane_that_takes_them() {
        assert_eq!(mouse_change(true), b"\x1b[?1003h");
        assert_eq!(mouse_change(false), b"\x1b[?1003l\x1b[?1002h");
        // Given back without it, whatever the pane asked last.
        assert!(LEAVE.windows(8).any(|w| w == b"\x1b[?1003l"));
    }

    #[test]
    fn events_go_through_the_protocol_unchanged() {
        let back = |event: &Event| -> Event {
            let json = serde_json::to_string(event).unwrap();
            serde_json::from_str(&json).unwrap_or_else(|e| panic!("{json}: {e}"))
        };
        // Every byte a terminal sends alone, and with Alt: read, carried, and sent to a pane as it came.
        let legacy = crate::mux::engine::Modes::default();
        for b in 0u8..=0x7f {
            for bytes in [vec![b], vec![0x1b, b]] {
                let mut parser = Parser::default();
                let mut tokens = Vec::new();
                parser.feed(&bytes, &mut tokens);
                parser.flush(&mut tokens);
                let events: Vec<Event> = tokens
                    .into_iter()
                    .filter_map(|token| match token {
                        Token::Event(event) => Some(event),
                        Token::Reply(_) => None,
                    })
                    .collect();
                if bytes == [0x1b, b'['] || bytes == [0x1b, b'O'] || bytes == [0x1b, b']'] || bytes == [0x1b, b'P'] {
                    // Alt+[, Alt+O, Alt+], Alt+P: the start of a sequence, read apart from a key (see parse.rs).
                    continue;
                }
                assert_eq!(events.len(), 1, "{bytes:?}: {events:?}");
                let carried = back(&events[0]);
                assert_eq!(carried, events[0], "{bytes:?}");
                let Event::Key(key) = carried else { panic!("{bytes:?}: {carried:?}") };
                assert_eq!(super::key(&key, &legacy), bytes, "{bytes:?}: {key:?}");
            }
        }
        // And the rest of what can come.
        let table = [
            Event::Key(Key { code: KeyCode::Char('é'), mods: Mods::CTRL | Mods::SHIFT, kind: KeyKind::Release }),
            Event::Key(Key { code: KeyCode::F(35), mods: Mods(0x3f), kind: KeyKind::Repeat }),
            Event::Key(Key::new(KeyCode::Enter, Mods::SHIFT)),
            Event::Key(Key::new(KeyCode::Char('"'), Mods::NONE)),
            Event::Key(Key::new(KeyCode::Char('\\'), Mods::NONE)),
            Event::Mouse(Mouse { kind: MouseKind::Drag(Button::Right), col: 65535, row: 0, mods: Mods::ALT }),
            Event::Mouse(Mouse { kind: MouseKind::ScrollLeft, col: 3, row: 4, mods: Mods::NONE }),
            Event::Mouse(Mouse { kind: MouseKind::Moved, col: 0, row: 0, mods: Mods::NONE }),
            Event::Paste("a\x1b[201~\r\n\u{0}é😀\u{9b}".into()),
            Event::Focus(false),
            Event::Resize(80, 24),
            Event::Closed("the terminal closed".into()),
        ];
        for event in &table {
            assert_eq!(&back(event), event);
        }
        // The form: serde's default, which the protocol's own test pins down.
        let json = serde_json::to_string(&Event::Key(Key::new(KeyCode::Char('a'), Mods::CTRL))).unwrap();
        assert_eq!(json, r#"{"Key":{"code":{"Char":"a"},"mods":4,"kind":"Press"}}"#);
    }

    #[test]
    fn decrpm_states() {
        let (answers, _) = heard(b"\x1b[?2026;0$y\x1b[?2027;4$y");
        assert!(!answers.settable(2026) && !answers.settable(2027));
        let (answers, _) = heard(b"\x1b[?2026;1$y\x1b[?2027;3$y");
        assert!(answers.settable(2026) && answers.settable(2027));
    }

    #[test]
    fn the_reader_says_when_the_terminal_is_gone() {
        let (tty, keyboard) = pipe().unwrap();
        let (stop, _stop_write) = pipe().unwrap();
        let (winch, _winch_write) = pipe().unwrap();
        File::from(keyboard).write_all(b"a").unwrap();
        let mut events = Vec::new();
        let mut send = |event| {
            events.push(event);
            true
        };
        let end = read(File::from(tty), stop, winch.as_raw_fd(), &mut Parser::default(), &mut send).unwrap();
        assert_eq!(end, End::Gone);
        assert_eq!(events, [Event::Key(Key::new(KeyCode::Char('a'), Mods::NONE))]);
    }

    #[test]
    fn the_reader_ends_with_the_terminal() {
        // A pipe stands for the terminal: keys, a size change ignored (no terminal behind), then the end.
        let (tty, mut keyboard) = {
            let (read, write) = pipe().unwrap();
            (File::from(read), File::from(write))
        };
        let (stop, stop_write) = pipe().unwrap();
        let (winch, _winch_write) = pipe().unwrap();
        keyboard.write_all(b"a\x1b[13;2u\x1b").unwrap();
        let mut events = Vec::new();
        let mut parser = Parser::default();
        let handle = thread::spawn(move || {
            let mut send = |event| {
                events.push(event);
                events.len() < 3
            };
            assert_eq!(read(tty, stop, winch.as_raw_fd(), &mut parser, &mut send).unwrap(), End::Stopped);
            drop(winch);
            events
        });
        let events = handle.join().unwrap();
        drop(stop_write);
        assert_eq!(
            events,
            [
                Event::Key(Key::new(KeyCode::Char('a'), Mods::NONE)),
                Event::Key(Key::new(KeyCode::Enter, Mods::SHIFT)),
                // The lone ESC, after its short wait.
                Event::Key(Key::new(KeyCode::Esc, Mods::NONE)),
            ]
        );
    }
}
