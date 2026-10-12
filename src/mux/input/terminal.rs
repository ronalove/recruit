// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The real terminal while the multiplexer is on it: set up after the probe, given back as it was, read on a thread
//! of its own.

use std::fs::File;
use std::io::{self, Read, Stdout, Write};
use std::os::fd::{AsFd, AsRawFd, IntoRawFd, OwnedFd, RawFd};
use std::panic::{self, PanicHookInfo};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::Event;
use super::parse::{Parser, Token};
use super::probe::{Answers, PROBE_TIME, QUERIES, caps, report};
use crate::mux::Caps;
use crate::mux::engine::MouseTracking;
use crate::t;

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

/// Which of the two was asked for ([`KEYBOARD_KITTY`], [`KEYBOARD_MODIFY`]), or neither: undone only then (`CSI < u`
/// reads as something else elsewhere).
static KEYBOARD: AtomicU8 = AtomicU8::new(KEYBOARD_NONE);
const KEYBOARD_NONE: u8 = 0;
const KEYBOARD_KITTY: u8 = 1;
const KEYBOARD_MODIFY: u8 = 2;

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
            let reason = match crate::mux::caught(|| read(input, stop, winch, &mut parser, &mut send)) {
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
    graphemes_off: bool,
    graphemes_on: bool,
    /// Mode 2027 on once set up: the terminal measures grapheme clusters whole (`Features::graphemes`).
    graphemes_active: bool,
}

impl Setup {
    /// Says what [`leave`] will have to undo.
    fn arm(&self) {
        KEYBOARD.store(if self.kitty { KEYBOARD_KITTY } else { KEYBOARD_MODIFY }, Ordering::SeqCst);
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
    Setup { bytes, kitty, graphemes_off, graphemes_on, graphemes_active }
}

/// The terminal as before: see [`LEAVE`]. Out of raw mode last.
fn leave() {
    let mut out = io::stdout();
    match KEYBOARD.swap(KEYBOARD_NONE, Ordering::SeqCst) {
        KEYBOARD_KITTY => {
            let _ = out.write_all(b"\x1b[<u");
        }
        KEYBOARD_MODIFY => {
            let _ = out.write_all(b"\x1b[>4m");
        }
        _ => {}
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

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// While it lives, a panic of the thread that installed it gives the terminal back, then runs the hook in place
/// before. A panic that [`crate::mux::caught`] handles runs nothing: its message is shown elsewhere. The message of a
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
            if crate::mux::catching() {
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
    use crate::mux::input::probe::tests::{env, heard};
    use crate::mux::input::{Key, KeyCode, Mods};
    #[test]
    fn grapheme_clusters_turned_off_only_where_on() {
        let ends = |setup: &Setup, tail: &[u8]| setup.bytes.ends_with(tail);
        // Ghostty 1.3.1: 2027 on by default.
        let (ghostty, _) = heard(b"\x1b[?0u\x1b[?2026;2$y\x1b[?2027;1$y\x1b[?62;22;52c");
        let caps = caps(&ghostty, env(&[]));
        let off = setup(&ghostty, &caps, false);
        assert!(off.graphemes_off && off.kitty && !off.graphemes_active);
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
                assert!(!plain.kitty && ends(&plain, MODIFY_OTHER_KEYS), "{answer:?} {graphemes}");
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
