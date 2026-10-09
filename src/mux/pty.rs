// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A program on a pseudo-terminal (spec §5.2): started in its own session and process group, resized with
//! `TIOCSWINSZ`, stopped with its whole group, always reaped. Descriptors are `CLOEXEC`, and nothing but
//! async-signal-safe calls runs between `fork` and `exec`.
//!
//! Threads, for each program: a reader, which hands the program's output to `output` and queues its replies; the
//! only writer, which empties the queue into the terminal (a tty takes about 1 KiB of input under macOS: a reader
//! that wrote would wait on a program waiting on it); a waiter, which reaps the program and wakes the reader, so that
//! it ends even when a descendant keeps the terminal open.
//!
//! Owner: dev-terminal.

use std::collections::VecDeque;
use std::ffi::{CStr, CString, OsStr, OsString, c_char, c_int};
use std::fs::File;
use std::io::{self, PipeReader, PipeWriter, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::t;

/// How long a program asked to end (SIGHUP) has before its group is killed.
const GRACE: Duration = Duration::from_secs(3);

/// Bytes read from the program at once.
const CHUNK: usize = 64 * 1024;

/// How long what is left is read once the program has ended: a descendant may go on writing.
const DRAIN: Duration = Duration::from_millis(100);

/// What to start, and where.
#[derive(Clone, Debug, Default)]
pub(crate) struct Spawn {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Taken out of the environment inherited (`TMUX`, `TMUX_PANE`, the real terminal's own variables).
    pub env_remove: Vec<OsString>,
    /// Set in it, after the removals (`TERM`, `COLORTERM`, `TERM_PROGRAM`…).
    pub env: Vec<(OsString, OsString)>,
    pub cols: u16,
    pub rows: u16,
}

/// A running program and its terminal's master side. Dropped, the program's group is stopped and reaped.
pub(crate) struct Pty {
    master: Arc<File>,
    child: Arc<Child>,
    input: PtyInput,
    /// Wakes the writer if it waits for the program to read: it ends.
    stop: PipeWriter,
}

impl Pty {
    /// Starts `spawn` on a new pseudo-terminal. `output` runs on a reader thread of the PTY's own with each chunk
    /// the program writes; what it leaves in its second argument is written back to the program (an engine's
    /// replies), ahead of later input. `exited` runs once, after the last output, with the exit code (`None`: ended
    /// by a signal).
    pub(crate) fn spawn(
        spawn: &Spawn,
        output: impl FnMut(&[u8], &mut Vec<u8>) + Send + 'static,
        exited: impl FnOnce(Option<i32>) + Send + 'static,
    ) -> io::Result<Pty> {
        // Everything the child needs is made here: after `fork`, it only reads it.
        let exec = Exec::new(spawn)?;
        let (master, slave) = open(spawn.cols, spawn.rows)?;
        let (woken, wake) = io::pipe()?;
        let (stopped, stop) = io::pipe()?;
        // SAFETY: the child runs `Exec::run` only, which makes async-signal-safe calls on memory prepared above.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            // SAFETY: in the child, right after `fork`.
            unsafe { exec.run(slave.as_raw_fd()) }
        }
        // The program's end shows on the master only once every copy of its side is closed.
        drop(slave);
        let child =
            Arc::new(Child { pid, reaped: Mutex::new(false), done: Condvar::new(), killing: AtomicBool::new(false) });

        let (status_tx, status) = mpsc::channel();
        let waiter = Arc::clone(&child);
        if let Err(error) =
            thread::Builder::new().name(format!("pty-wait-{pid}")).spawn(move || wait(&waiter, wake, &status_tx))
        {
            // Nobody would reap it: done here.
            child.signal(libc::SIGKILL);
            child.reap();
            return Err(error);
        }

        let master = Arc::new(master);
        let queue = Arc::new(Queue::default());
        let pty = Pty { master: Arc::clone(&master), child, input: PtyInput { queue: Arc::clone(&queue) }, stop };
        // From here, an error drops `pty`, which stops the program; the waiter reaps it.
        // Non-blocking: the writer must not wait forever on a program that no longer reads. The reader polls first.
        set_nonblocking(master.as_raw_fd())?;
        let (writer, replies) = (Arc::clone(&master), Arc::clone(&queue));
        thread::Builder::new().name(format!("pty-write-{pid}")).spawn(move || write(&writer, &queue, &stopped))?;
        thread::Builder::new()
            .name(format!("pty-read-{pid}"))
            .spawn(move || read(&master, &woken, &replies, output, exited, &status))?;
        Ok(pty)
    }

    /// Where to send the program's input from any thread.
    pub(crate) fn input(&self) -> PtyInput {
        self.input.clone()
    }

    pub(crate) fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        set_size(self.master.as_raw_fd(), cols, rows)
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.pid as u32
    }

    /// Asks the program's group to end (SIGHUP, as when a terminal closes), then forces it after a short delay.
    /// Returns at once; `exited` says when it is over.
    pub(crate) fn kill(&self) {
        // SIGCONT too, as the kernel does on a hangup: a stopped program would not see the SIGHUP.
        self.child.signal(libc::SIGHUP);
        self.child.signal(libc::SIGCONT);
        if self.child.killing.swap(true, Ordering::AcqRel) {
            return;
        }
        let child = Arc::clone(&self.child);
        let forced = thread::Builder::new().name(format!("pty-kill-{}", child.pid)).spawn(move || {
            let reaped = child.lock();
            let (reaped, _) =
                child.done.wait_timeout_while(reaped, GRACE, |reaped| !*reaped).unwrap_or_else(PoisonError::into_inner);
            drop(reaped);
            child.signal(libc::SIGKILL);
        });
        if forced.is_err() {
            self.child.signal(libc::SIGKILL);
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // The writer ends, even if someone keeps a `PtyInput`, or if the program does not read.
        self.input.queue.stop();
        let _ = (&self.stop).write_all(&[0]);
        self.kill();
    }
}

/// The program's input. Sending never blocks: a writer thread empties the queue as fast as the program reads (a
/// paste of several hundred kilobytes must not stop the screen).
#[derive(Clone)]
pub(crate) struct PtyInput {
    queue: Arc<Queue>,
}

impl PtyInput {
    pub(crate) fn send(&self, bytes: Vec<u8>) {
        self.queue.push(bytes, false);
    }
}

/// What waits to be written to the program: the terminal's replies first, then its input, each message whole (a
/// reply never goes in the middle of a paste).
#[derive(Default)]
struct Queue {
    state: Mutex<Waiting>,
    ready: Condvar,
}

#[derive(Default)]
struct Waiting {
    replies: VecDeque<Vec<u8>>,
    input: VecDeque<Vec<u8>>,
    /// The PTY was dropped, or the program can no longer be written to: what comes is dropped.
    stopped: bool,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, Waiting> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, bytes: Vec<u8>, reply: bool) {
        let mut waiting = self.lock();
        if bytes.is_empty() || waiting.stopped {
            return;
        }
        if reply { &mut waiting.replies } else { &mut waiting.input }.push_back(bytes);
        self.ready.notify_one();
    }

    /// The next message to write, replies first; `None` once stopped.
    fn pop(&self) -> Option<Vec<u8>> {
        let mut waiting = self.lock();
        loop {
            if waiting.stopped {
                return None;
            }
            if let Some(bytes) = waiting.replies.pop_front().or_else(|| waiting.input.pop_front()) {
                return Some(bytes);
            }
            waiting = self.ready.wait(waiting).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn stop(&self) {
        let mut waiting = self.lock();
        waiting.stopped = true;
        waiting.replies.clear();
        waiting.input.clear();
        self.ready.notify_one();
    }
}

/// The program, as long as it has not been reaped.
struct Child {
    pid: libc::pid_t,
    /// Set under the lock once the program is reaped: its id may then be another's, and no signal is sent to it.
    reaped: Mutex<bool>,
    /// Told when `reaped` is set.
    done: Condvar,
    /// A forced end is already on its way.
    killing: AtomicBool,
}

impl Child {
    fn lock(&self) -> MutexGuard<'_, bool> {
        self.reaped.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends `signal` to the program's group, or to the program if it has not made its group yet.
    ///
    /// Nothing once the program is reaped, by choice: its descendants got the kernel's SIGHUP when it ended (it led
    /// the session), and those that ignore it are left alone, as tmux does. POSIX keeps a group's id from another
    /// process while the group has members, but an empty group's id may come back, and the group is not ours then.
    fn signal(&self, signal: c_int) {
        let reaped = self.lock();
        if !*reaped {
            // SAFETY: signals to the program this PTY started, not reaped yet: its id is still its own.
            unsafe {
                if libc::kill(-self.pid, signal) != 0 {
                    libc::kill(self.pid, signal);
                }
            }
        }
    }

    /// Reaps the program, waiting for its end if need be. Its exit code, `None` when a signal ended it.
    ///
    /// Once `waitid` has seen the end, this does not wait, and the lock keeps `signal` off the id the whole time. If
    /// `waitid` failed, the wait is made without the lock, so that `signal` (and `kill`) never block on it; a signal
    /// could then reach the id just after it is freed.
    fn reap(&self) -> Option<i32> {
        let mut status = 0;
        let mut reaped = self.lock();
        let mut pid = waitpid(self.pid, &mut status, libc::WNOHANG);
        if pid == 0 {
            drop(reaped);
            pid = waitpid(self.pid, &mut status, 0);
            reaped = self.lock();
        }
        *reaped = true;
        self.done.notify_all();
        (pid == self.pid && libc::WIFEXITED(status)).then(|| libc::WEXITSTATUS(status))
    }
}

/// `waitpid`, again when a signal interrupts it.
fn waitpid(pid: libc::pid_t, status: &mut c_int, options: c_int) -> libc::pid_t {
    loop {
        // SAFETY: waits for a child of this process, into an out parameter of the right type.
        let waited = unsafe { libc::waitpid(pid, status, options) };
        if waited >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return waited;
        }
    }
}

/// The waiter: waits for the program to end without reaping it, then reaps it under the lock (no signal may reach an
/// id that is no longer the program's), hands its exit code to the reader and wakes it.
fn wait(child: &Child, wake: PipeWriter, status: &Sender<Option<i32>>) {
    loop {
        // SAFETY: an out parameter of the right type, for this PTY's own child.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let waited =
            unsafe { libc::waitid(libc::P_PID, child.pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) };
        if waited == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
    let _ = status.send(child.reap());
    let _ = (&wake).write_all(&[0]);
}

/// The reader: the program's output to `output`, its replies queued, until the terminal is closed on the program's
/// side or, once the program has ended, until nothing is left to read. Then `exited`, with the exit code.
fn read(
    master: &File,
    woken: &PipeReader,
    queue: &Queue,
    mut output: impl FnMut(&[u8], &mut Vec<u8>),
    exited: impl FnOnce(Option<i32>),
    status: &Receiver<Option<i32>>,
) {
    let mut buffer = vec![0; CHUNK];
    let mut replies = Vec::new();
    // Once the program has ended: until when what is left is read.
    let mut ended: Option<Instant> = None;
    loop {
        if ended.is_some_and(|until| Instant::now() >= until) {
            break;
        }
        let mut fds = [
            libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: woken.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // Once the program has ended, only what is already there is read: a descendant may keep its side open.
        let (count, timeout) = if ended.is_some() { (1, 0) } else { (2, -1) };
        // SAFETY: `count` descriptors that outlive the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), count, timeout) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if ready == 0 {
            break;
        }
        if count == 2 && fds[1].revents != 0 {
            ended = Some(Instant::now() + DRAIN);
        }
        let master_ready = fds[0].revents;
        if master_ready & libc::POLLNVAL != 0 {
            break;
        }
        if master_ready & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
            continue;
        }
        match (&*master).read(&mut buffer) {
            // macOS reads nothing once the program's side is closed, Linux fails with EIO.
            Ok(0) => break,
            Ok(read) => {
                output(&buffer[..read], &mut replies);
                if !replies.is_empty() {
                    queue.push(std::mem::take(&mut replies), true);
                }
            }
            Err(error) if matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) => {}
            Err(_) => break,
        }
    }
    exited(status.recv().unwrap_or(None));
}

/// The writer: the queue into the terminal, until the PTY is dropped or the program is gone. The master does not
/// block: when the program does not read, the writer waits for room, or for `stopped`.
fn write(master: &File, queue: &Queue, stopped: &PipeReader) {
    'queue: while let Some(bytes) = queue.pop() {
        let mut rest = &bytes[..];
        while !rest.is_empty() {
            match (&*master).write(rest) {
                Ok(0) => break 'queue,
                Ok(written) => rest = &rest[written..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if !room(master, stopped) {
                        break 'queue;
                    }
                }
                Err(_) => break 'queue,
            }
        }
    }
    queue.stop();
}

/// Waits until the program can take more input; false if the PTY was dropped or the terminal is gone.
fn room(master: &File, stopped: &PipeReader) -> bool {
    loop {
        let mut fds = [
            libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLOUT, revents: 0 },
            libc::pollfd { fd: stopped.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: two descriptors that outlive the call.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return false;
        }
        if fds[1].revents != 0 {
            return false;
        }
        if fds[0].revents & libc::POLLOUT != 0 {
            return true;
        }
        if fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return false;
        }
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: flags of a descriptor this module opened.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// A new pseudo-terminal, `cols` × `rows`: its master and the program's side, both `CLOEXEC` from the start.
fn open(cols: u16, rows: u16) -> io::Result<(File, OwnedFd)> {
    // SAFETY: calls on descriptors this function owns, and on out parameters of the right types.
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if master < 0 {
            return Err(io::Error::last_os_error());
        }
        let master = File::from(OwnedFd::from_raw_fd(master));
        if libc::grantpt(master.as_raw_fd()) != 0 || libc::unlockpt(master.as_raw_fd()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let name = slave_name(master.as_raw_fd())?;
        let slave = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if slave < 0 {
            return Err(io::Error::last_os_error());
        }
        let slave = OwnedFd::from_raw_fd(slave);
        // UTF-8 input: a backspace in a line being edited takes a whole character back.
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(slave.as_raw_fd(), &mut termios) == 0 {
            termios.c_iflag |= libc::IUTF8;
            libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios);
        }
        set_size(master.as_raw_fd(), cols, rows)?;
        Ok((master, slave))
    }
}

/// The path of the program's side of `master`. `ptsname` writes into a buffer of its own: only this function calls
/// it, under a lock.
fn slave_name(master: RawFd) -> io::Result<CString> {
    static PTSNAME: Mutex<()> = Mutex::new(());
    let _guard = PTSNAME.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: a master this module opened; the name is copied before the lock is released.
    unsafe {
        let name = libc::ptsname(master);
        if name.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(CStr::from_ptr(name).to_owned())
    }
}

/// Gives the terminal its size, which sends SIGWINCH to the program.
fn set_size(master: RawFd, cols: u16, rows: u16) -> io::Result<()> {
    let size = libc::winsize { ws_row: rows.max(1), ws_col: cols.max(1), ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: a `winsize` for TIOCSWINSZ, on a terminal this module opened.
    if unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Everything `exec` needs, made before `fork`: the child must not allocate.
struct Exec {
    path: CString,
    _argv: Vec<CString>,
    argv: Vec<*const c_char>,
    _envp: Vec<CString>,
    envp: Vec<*const c_char>,
    cwd: Option<CString>,
    /// Written to the terminal, before the error number, if the program cannot be started after all.
    failed: Vec<u8>,
}

impl Exec {
    fn new(spawn: &Spawn) -> io::Result<Exec> {
        let env = environment(spawn);
        let cwd = (!spawn.cwd.as_os_str().is_empty()).then_some(spawn.cwd.as_path());
        if let Some(cwd) = cwd
            && !cwd.is_dir()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                t!("{} : dossier introuvable", "{}: no such directory", cwd.display()),
            ));
        }
        let path = resolve(&spawn.program, cwd.unwrap_or(Path::new("")), &env)?;
        let argv: Vec<CString> =
            std::iter::once(&spawn.program).chain(&spawn.args).map(|arg| c_string(arg)).collect::<io::Result<_>>()?;
        let envp: Vec<CString> = env
            .iter()
            .map(|(key, value)| {
                let mut pair = key.as_bytes().to_vec();
                pair.push(b'=');
                pair.extend(value.as_bytes());
                c_string(OsStr::from_bytes(&pair))
            })
            .collect::<io::Result<_>>()?;
        let failed = t!(
            "recruit : impossible de lancer {} (erreur ",
            "recruit: cannot start {} (error ",
            spawn.program.to_string_lossy()
        );
        Ok(Exec {
            path: c_string(path.as_os_str())?,
            argv: argv.iter().map(|arg| arg.as_ptr()).chain([std::ptr::null()]).collect(),
            _argv: argv,
            envp: envp.iter().map(|pair| pair.as_ptr()).chain([std::ptr::null()]).collect(),
            _envp: envp,
            cwd: cwd.map(|cwd| c_string(cwd.as_os_str())).transpose()?,
            failed: failed.into_bytes(),
        })
    }

    /// The child's side, between `fork` and `exec`: default signals, the terminal as its standard descriptors (first,
    /// so that a failure is told there), a session of its own with the terminal as its controlling one, its
    /// directory, then the program. Only async-signal-safe calls.
    ///
    /// SAFETY: to be called in the child, right after `fork`.
    unsafe fn run(&self, slave: RawFd) -> ! {
        // SAFETY: async-signal-safe calls on memory made before `fork`.
        unsafe {
            // Ignored signals stay ignored across `exec` (Rust ignores SIGPIPE): the program gets the defaults back.
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            for signal in 1..32 {
                if signal != libc::SIGKILL && signal != libc::SIGSTOP {
                    libc::signal(signal, libc::SIG_DFL);
                }
            }
            // Above the standard descriptors first: `dup2` onto itself would leave it `CLOEXEC`.
            let slave = if slave < 3 { libc::fcntl(slave, libc::F_DUPFD, 3) } else { slave };
            if slave < 0 {
                self.fail();
            }
            for fd in 0..3 {
                if libc::dup2(slave, fd) < 0 {
                    self.fail();
                }
            }
            libc::close(slave);
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                self.fail();
            }
            if let Some(cwd) = &self.cwd
                && libc::chdir(cwd.as_ptr()) < 0
            {
                self.fail();
            }
            libc::execve(self.path.as_ptr(), self.argv.as_ptr(), self.envp.as_ptr());
            self.fail()
        }
    }

    /// Says why on the terminal, without allocating, and ends the child as a shell would.
    unsafe fn fail(&self) -> ! {
        // SAFETY: async-signal-safe calls, on memory made before `fork` and on the stack.
        unsafe {
            let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            let mut digits = [0u8; 12];
            let mut at = digits.len();
            let mut n = errno.unsigned_abs();
            loop {
                at -= 1;
                digits[at] = b'0' + (n % 10) as u8;
                n /= 10;
                if n == 0 {
                    break;
                }
            }
            libc::write(2, self.failed.as_ptr().cast(), self.failed.len());
            libc::write(2, digits[at..].as_ptr().cast(), digits.len() - at);
            libc::write(2, b")\r\n".as_ptr().cast(), 3);
            libc::_exit(127)
        }
    }
}

fn c_string(text: &OsStr) -> io::Result<CString> {
    CString::new(text.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, t!("octet nul dans {:?}", "NUL byte in {:?}", text)))
}

/// The program's environment: this one's, less `env_remove`, plus `env`.
fn environment(spawn: &Spawn) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> =
        std::env::vars_os().filter(|(key, _)| !spawn.env_remove.contains(key)).collect();
    for (key, value) in &spawn.env {
        env.retain(|(known, _)| known != key);
        env.push((key.clone(), value.clone()));
    }
    env
}

/// Where `program` is: as given if it has a slash (from `cwd` when relative, as `exec` will after `chdir`), else
/// the first executable file of that name in the program's `PATH`.
fn resolve(program: &OsStr, cwd: &Path, env: &[(OsString, OsString)]) -> io::Result<PathBuf> {
    let executable =
        |path: &Path| path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    let not_found =
        || io::Error::new(io::ErrorKind::NotFound, t!("{} : introuvable", "{}: not found", program.to_string_lossy()));
    if program.as_bytes().contains(&b'/') {
        return if executable(&cwd.join(program)) { Ok(program.into()) } else { Err(not_found()) };
    }
    if program.is_empty() {
        return Err(not_found());
    }
    let path = env.iter().find(|(key, _)| key == "PATH").map_or(OsStr::new("/usr/bin:/bin"), |(_, path)| path);
    std::env::split_paths(path)
        .map(|dir| cwd.join(dir).join(program))
        .find(|path| executable(path))
        .ok_or_else(not_found)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Instant;

    /// What a program wrote, and how it ended.
    struct Run {
        pty: Pty,
        output: Arc<Mutex<Vec<u8>>>,
        ended: Receiver<Option<i32>>,
    }

    fn start(program: &str, args: &[&str], reply: Option<(&'static [u8], &'static [u8])>) -> Run {
        let spawn = Spawn {
            program: program.into(),
            args: args.iter().map(OsString::from).collect(),
            cwd: std::env::temp_dir(),
            env_remove: vec!["HOME".into()],
            env: vec![("RECRUIT_TEST_SET".into(), "set".into())],
            cols: 100,
            rows: 30,
        };
        let output = Arc::new(Mutex::new(Vec::new()));
        let (tx, ended) = mpsc::channel();
        let seen = Arc::clone(&output);
        let mut answered = false;
        let pty = Pty::spawn(
            &spawn,
            move |bytes, replies| {
                let mut seen = seen.lock().unwrap();
                seen.extend_from_slice(bytes);
                if let Some((query, answer)) = reply
                    && !answered
                    && seen.windows(query.len()).any(|w| w == query)
                {
                    replies.extend_from_slice(answer);
                    answered = true;
                }
            },
            move |code| tx.send(code).unwrap(),
        )
        .unwrap();
        Run { pty, output, ended }
    }

    impl Run {
        fn wait(&self) -> Option<i32> {
            self.ended.recv_timeout(Duration::from_secs(10)).expect("the program ends")
        }

        fn text(&self) -> String {
            String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
        }
    }

    #[test]
    fn a_program_runs_on_its_own_terminal() {
        let run = start(
            "/bin/sh",
            &[
                "-c",
                "stty size; echo \"[$RECRUIT_TEST_SET:${HOME:-none}]\"; tty >/dev/null && echo tty; pwd -P; exit 7",
            ],
            None,
        );
        assert_eq!(run.wait(), Some(7));
        let text = run.text();
        assert!(text.contains("30 100"), "{text:?}");
        assert!(text.contains("[set:none]"), "{text:?}");
        assert!(text.contains("tty"), "{text:?}");
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        assert!(text.contains(&*tmp.to_string_lossy()), "{text:?}");
    }

    #[test]
    fn replies_and_input_reach_the_program() {
        // The program asks, reads the answer, says so, then reads a line of input.
        let run = start(
            "/bin/sh",
            &[
                "-c",
                "stty raw -echo; printf '?query'; a=$(dd bs=1 count=6 2>/dev/null); stty -raw; echo got; read b; echo \"<$a|$b>\"",
            ],
            Some((b"?query", b"answer")),
        );
        let started = Instant::now();
        while !run.text().contains("got") {
            assert!(started.elapsed() < Duration::from_secs(5), "{:?}", run.text());
            thread::sleep(Duration::from_millis(10));
        }
        run.pty.input().send(b"line\n".to_vec());
        assert_eq!(run.wait(), Some(0));
        let text = run.text();
        assert!(text.contains("<answer|line>"), "{text:?}");
    }

    #[test]
    fn replies_go_before_the_input_waiting() {
        let queue = Queue::default();
        queue.push(b"paste".to_vec(), false);
        queue.push(b"key".to_vec(), false);
        queue.push(b"reply".to_vec(), true);
        queue.push(Vec::new(), true);
        assert_eq!(queue.pop().unwrap(), b"reply");
        assert_eq!(queue.pop().unwrap(), b"paste");
        queue.push(b"reply 2".to_vec(), true);
        assert_eq!(queue.pop().unwrap(), b"reply 2");
        assert_eq!(queue.pop().unwrap(), b"key");
        queue.stop();
        queue.push(b"late".to_vec(), false);
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn resized_and_signalled() {
        let run = start("/bin/sh", &["-c", "trap 'stty size; exit 3' WINCH; while :; do sleep 0.05; done"], None);
        thread::sleep(Duration::from_millis(200));
        run.pty.resize(55, 12).unwrap();
        assert_eq!(run.wait(), Some(3));
        assert!(run.text().contains("12 55"), "{:?}", run.text());
    }

    #[test]
    fn killed_with_its_group() {
        // A child that ignores nothing, and a grandchild in the same group: both go.
        let run = start("/bin/sh", &["-c", "sleep 30 & echo $!; wait"], None);
        let started = Instant::now();
        let grandchild = loop {
            if let Some(pid) = run.text().lines().next().and_then(|line| line.trim().parse::<i32>().ok()) {
                break pid;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(20));
        };
        run.pty.kill();
        assert_eq!(run.wait(), None);
        thread::sleep(Duration::from_millis(100));
        // SAFETY: signal 0 only checks that the process exists.
        assert_ne!(unsafe { libc::kill(grandchild, 0) }, 0, "the grandchild is gone");
        // Reaped: no zombie left behind.
        assert!(*run.pty.child.lock());
    }

    #[test]
    fn forced_when_it_ignores_the_hangup() {
        let run = start("/bin/sh", &["-c", "trap '' HUP; echo ready; while :; do sleep 0.05; done"], None);
        while !run.text().contains("ready") {
            thread::sleep(Duration::from_millis(20));
        }
        let asked = Instant::now();
        drop(run.pty);
        assert_eq!(run.ended.recv_timeout(GRACE * 3).unwrap(), None);
        assert!(asked.elapsed() >= GRACE);
    }

    #[test]
    fn ends_even_if_a_descendant_keeps_the_terminal() {
        // A grandchild that ignores the hangup keeps the terminal open: the program's end is still seen.
        let run = start("/bin/sh", &["-c", "(trap '' HUP; sleep 3) & echo started; exit 5"], None);
        let started = Instant::now();
        assert_eq!(run.wait(), Some(5));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(run.text().contains("started"));
    }

    #[test]
    fn signals_ignored_here_are_not_ignored_there() {
        // The server will ignore SIGHUP and SIGPIPE: a program started from it must not. Checked in a process of its
        // own that starts with SIGHUP ignored, so that the other tests keep theirs.
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", "trap '' HUP; exec \"$0\" \"$@\""])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", "mux::pty::tests::hangup_ignored_by_the_parent", "--ignored", "--nocapture"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && text.contains("SIGHUP ignored here"), "{text}");
    }

    #[test]
    #[ignore = "run by signals_ignored_here_are_not_ignored_there, in a process that ignores SIGHUP"]
    fn hangup_ignored_by_the_parent() {
        // SAFETY: reads the disposition only.
        let ignored = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut action);
            action.sa_sigaction == libc::SIG_IGN
        };
        if !ignored {
            println!("SIGHUP not ignored in this process: nothing to check");
            return;
        }
        println!("SIGHUP ignored here");
        let run = start("/bin/sh", &["-c", "kill -HUP $$; echo survived"], None);
        assert_eq!(run.wait(), None);
        assert!(!run.text().contains("survived"), "{:?}", run.text());
    }

    #[test]
    fn a_descendant_writing_forever_does_not_hold_the_end() {
        let run = start("/bin/sh", &["-c", "(trap '' HUP; yes) & sleep 0.2; exit 4"], None);
        assert_eq!(run.wait(), Some(4));
        // SAFETY: the `yes` left behind, found by its group (the program's id).
        unsafe { libc::kill(-(run.pty.pid() as i32), libc::SIGKILL) };
    }

    #[test]
    fn the_writer_ends_with_the_pty_even_if_nobody_reads() {
        // The program never reads, and a descendant keeps the terminal open: the writer waits for room, and must
        // still end when the PTY is dropped, closing the master.
        let run = start("/bin/sh", &["-c", "(trap '' HUP; sleep 3) & sleep 30"], None);
        run.pty.input().send(vec![b'x'; 1 << 20]);
        thread::sleep(Duration::from_millis(300));
        let master = Arc::clone(&run.pty.master);
        drop(run.pty);
        assert_eq!(run.ended.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        let started = Instant::now();
        while Arc::strong_count(&master) > 1 {
            assert!(started.elapsed() < Duration::from_secs(2), "the writer still holds the master");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn unknown_programs_are_refused() {
        let spawn = Spawn { program: "recruit-no-such-program".into(), cols: 80, rows: 24, ..Spawn::default() };
        let error = Pty::spawn(&spawn, |_, _| {}, |_| {}).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        let spawn = Spawn { program: "/bin/sh".into(), cwd: "/no/such/dir".into(), ..Spawn::default() };
        assert_eq!(Pty::spawn(&spawn, |_, _| {}, |_| {}).err().unwrap().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn descriptors_are_not_inherited() {
        // The program has its three standard descriptors and nothing of ours (the master, the pipes). `test -e`
        // opens nothing, unlike `ls /dev/fd`.
        let run = start(
            "/bin/sh",
            &["-c", "for fd in $(seq 3 255); do [ -e /dev/fd/$fd ] && echo \"fd $fd\"; done; :"],
            None,
        );
        assert_eq!(run.wait(), Some(0));
        assert!(!run.text().contains("fd "), "{:?}", run.text());
    }
}
