// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A native multiplexer server of the test's own (step 1): `recruit _server <state>` on a copy of the binary, with
//! `XDG_*` and `RECRUIT_TMPDIR` in a temporary folder; driven by `recruit _ctl <state> …` (where, spawn, capture,
//! send, key, clients, kill, stop) and seen through real clients (`_ctl <state> attach`) on test terminals.
//! Stopped, and checked to leave no process behind, when dropped.
//!
//! Owner: testeur.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use super::pty::{Profile, TestTerm};

/// `_ctl`'s exit codes (agreed with dev-serveur).
pub const NOT_RUNNING: i32 = 2;
pub const UNKNOWN_PANE: i32 = 3;

pub struct Server {
    recruit: PathBuf,
    root: PathBuf,
    state: PathBuf,
    /// `RECRUIT_TMPDIR`, short: the socket's path must fit in 103 bytes, which a folder under `/var/folders` or a
    /// scratchpad does not leave room for.
    run: tempfile::TempDir,
    stopped: bool,
}

impl Server {
    /// Starts a server for a team whose state lives in `root/state`; returns once it is ready.
    pub fn start(root: &Path) -> Server {
        let recruit = super::recruit_copy(root);
        let state = root.join("state");
        std::fs::create_dir_all(&state).expect("state dir");
        let run = tempfile::Builder::new().prefix("rt.").tempdir_in("/tmp").expect("short run dir");
        let server = Server { recruit, root: root.to_path_buf(), state, run, stopped: false };
        // The language given: detecting it starts CoreFoundation's threads under macOS, and `_server` refuses to
        // fork with threads running.
        // The test's language, English by default as in the CI (which runs without a locale).
        let lang = std::env::var("RECRUIT_LANG").ok().filter(|l| l == "fr").unwrap_or_else(|| "en".into());
        let out = super::retry_busy(|| {
            server.command().args(["--lang", &lang, "_server"]).arg(&server.state).stdin(Stdio::null()).output()
        })
        .expect("_server");
        assert!(out.status.success(), "_server: {}", String::from_utf8_lossy(&out.stderr));
        server
    }

    /// The copy of recruit, with the test's environment.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.recruit);
        command
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("RECRUIT_TMPDIR", self.run.path());
        for key in super::pty::SCRUB {
            command.env_remove(key);
        }
        command
    }

    /// `recruit _ctl <state> <args…>`, its output whatever its code.
    pub fn ctl_output<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.try_ctl(args).expect("_ctl")
    }

    /// `recruit _ctl <state> <args…>`, or why it could not run.
    fn try_ctl<I, S>(&self, args: I) -> std::io::Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_os_string()).collect();
        super::retry_busy(|| self.command().arg("_ctl").arg(&self.state).args(&args).stdin(Stdio::null()).output())
    }

    /// `recruit _ctl <state> <args…>`, which must succeed; its stdout.
    pub fn ctl<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_os_string()).collect();
        let out = self.ctl_output(&args);
        assert!(out.status.success(), "_ctl {args:?} ({}): {}", out.status, String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `server.json`, as `_ctl where` gives it.
    pub fn where_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.ctl(["where"])).expect("where: JSON")
    }

    /// Whether the server of this state folder still lives, from outside: its lock file is held (`flock` fails for
    /// us). Cheap, and true only while a server holds it: a process that took the server's pid since is no proof.
    pub fn alive(&self) -> impl Fn() -> bool + Send + Sync + 'static {
        let lock = self.state.join("server.lock");
        move || holds_lock(&lock)
    }

    /// The server's pid.
    pub fn pid(&self) -> u32 {
        self.where_json()["pid"].as_u64().expect("where: pid") as u32
    }

    /// Opens a pane for `member` running a fake of the tests (`role`, with `vars`).
    pub fn spawn_fake(&self, member: &str, role: &str, vars: &[(&str, String)]) {
        let mut args: Vec<OsString> = vec!["spawn".into(), member.into(), "--".into()];
        args.extend(super::child_words(role, vars));
        // The test's folder on the fake's command line, so that `leftovers` finds it (`ps` may show no process's
        // environment, macOS 27 in a test): one more filter for libtest, which names no test.
        args.push(self.root.clone().into_os_string());
        self.ctl(args);
        // The check that nothing is left proves something only if it sees the fakes: it must see this one now.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !leftovers(&self.root).iter().any(|l| l.contains(" child --exact")) {
            assert!(
                std::time::Instant::now() < deadline,
                "leftovers() does not see the fake {member}: it would prove nothing; it sees {:?}",
                short(&leftovers(&self.root))
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Opens a pane for `member` running `command` in `cwd`, with `env` on top (`--env K=V`).
    pub fn spawn(&self, member: &str, cwd: &Path, env: &[(&str, &str)], command: &[&str]) {
        let mut args: Vec<OsString> = vec!["spawn".into(), member.into(), "--cwd".into(), cwd.into()];
        for (key, value) in env {
            args.extend(["--env".into(), format!("{key}={value}").into()]);
        }
        args.push("--".into());
        args.extend(command.iter().map(OsString::from));
        self.ctl(args);
    }

    /// `capture --pane <member> --json`, parsed.
    pub fn capture_json(&self, member: &str) -> serde_json::Value {
        serde_json::from_str(&self.ctl(["capture", "--pane", member, "--json"])).expect("capture: JSON")
    }

    /// The text of `member`'s pane (or of the composed screen, `None`).
    pub fn capture(&self, member: Option<&str>) -> Vec<String> {
        let mut args: Vec<&str> = vec!["capture"];
        if let Some(member) = member {
            args.extend(["--pane", member]);
        }
        self.ctl(args).lines().map(|l| l.trim_end().to_string()).collect()
    }

    /// The last `lines` of `member`'s history, then its screen (`capture --history`).
    pub fn capture_history(&self, member: &str, lines: usize) -> Vec<String> {
        let lines = lines.to_string();
        self.ctl(["capture", "--pane", member, "--history", &lines]).lines().map(|l| l.trim_end().to_string()).collect()
    }

    /// Waits until `text` shows in `member`'s pane, `timeout` at most.
    pub fn wait_pane(&self, member: &str, text: &str, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.capture(Some(member)).iter().any(|r| r.contains(text)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    /// The clients attached, as `_ctl clients --json` gives them.
    pub fn clients(&self) -> serde_json::Value {
        serde_json::from_str(&self.ctl(["clients", "--json"])).expect("clients: JSON")
    }

    /// A real client on a new test terminal of `cols` × `rows`.
    pub fn attach(&self, cols: u16, rows: u16) -> TestTerm {
        self.attach_as(cols, rows, Profile::default())
    }

    /// The same, on a test terminal that behaves as `profile` says (with or without the kitty protocol…).
    pub fn attach_as(&self, cols: u16, rows: u16, profile: Profile) -> TestTerm {
        self.attach_with(cols, rows, profile, &[])
    }

    /// The same, with `extra` in the client's environment (`COLORTERM` empty for a terminal without 24 bits…).
    pub fn attach_with(&self, cols: u16, rows: u16, profile: Profile, extra: &[(&str, &str)]) -> TestTerm {
        let mut env = vec![
            ("XDG_CONFIG_HOME", self.root.join("config").into_os_string()),
            ("XDG_CACHE_HOME", self.root.join("cache").into_os_string()),
            ("RECRUIT_TMPDIR", self.run.path().as_os_str().to_os_string()),
        ];
        env.extend(extra.iter().map(|(k, v)| (*k, OsString::from(v))));
        let env: Vec<(&str, &OsStr)> = env.iter().map(|(k, v)| (*k, v.as_os_str())).collect();
        let args: [&OsStr; 3] = [OsStr::new("_ctl"), self.state.as_os_str(), OsStr::new("attach")];
        let profile = Profile { capture: true, ..profile };
        TestTerm::spawn(self.recruit.as_os_str(), args, &env, cols, rows, profile)
    }

    /// Stops the server (`_ctl stop`), and checks that nothing it started is left.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let pid = self.where_json()["pid"].as_u64().map(|p| p as u32);
        let out = self.ctl_output(["stop"]);
        assert!(out.status.success(), "_ctl stop ({}): {}", out.status, String::from_utf8_lossy(&out.stderr));
        if let Some(pid) = pid {
            std::thread::sleep(Duration::from_millis(300));
            assert!(super::Usage::of(pid).is_none(), "the server ({pid}) is still there after stop");
        }
        let left = leftovers(&self.root);
        assert!(left.is_empty(), "processes left after stop: {:?}", short(&left));
    }

    /// For a server that died on its own (killed by the test): nothing to stop when dropped, but its folders are
    /// still removed.
    pub fn gone(mut self) {
        self.stopped = true;
    }

    /// The end of the server's log (`server.log` in its state folder), for a failed check.
    pub fn log_tail(&self) -> String {
        let log = std::fs::read_to_string(self.state.join("server.log")).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        lines[lines.len().saturating_sub(30)..].join("\n")
    }

    pub fn state(&self) -> &Path {
        &self.state
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if !self.stopped && !std::thread::panicking() {
            self.stop();
        } else if !self.stopped {
            // A failed test: stop without checking, so that nothing is left running; and never a second panic.
            self.stopped = true;
            let _ = self.try_ctl(["stop"]);
        }
    }
}

/// The processes still running whose command line (or, under Linux, environment) names `root` (the test's own
/// folder): the server, its client, the fakes (`spawn_fake` puts the folder on their command line). Each as "pid
/// command line environment".
pub fn leftovers(root: &Path) -> Vec<String> {
    let root = root.to_string_lossy();
    processes()
        .into_iter()
        .filter(|l| l.contains(root.as_ref()))
        // An MCP server's sidecar of the user's (`ha_mcp`) leaves its Claude's process group and outlives it, under
        // tmux as well (2026-10-09): said, not counted.
        .filter(|l| {
            let sidecar = l.contains("stdio_settings_sidecar");
            if sidecar {
                println!("Ignored, outlives its Claude under tmux too: {}", l.chars().take(100).collect::<String>());
            }
            !sidecar
        })
        .map(|l| l.trim().to_string())
        .collect()
}

/// Lines of `leftovers`, cut for a message (the environment makes them long).
pub fn short(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| l.chars().take(200).collect()).collect()
}

/// Every process but this one and its `ps`, with its command line and environment: from /proc under Linux (where
/// `ps` does not show the environment), from `ps -axeww` under macOS (which shows it for one's own processes).
fn processes() -> Vec<String> {
    let me = std::process::id();
    #[cfg(target_os = "linux")]
    {
        let read =
            |path: std::path::PathBuf| std::fs::read(path).map(|b| String::from_utf8_lossy(&b).replace('\0', " ")).ok();
        std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| *pid != me)
            .filter_map(|pid| {
                let cmdline = read(format!("/proc/{pid}/cmdline").into())?;
                let environ = read(format!("/proc/{pid}/environ").into()).unwrap_or_default();
                Some(format!("{pid} {cmdline} {environ}"))
            })
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = Command::new("ps").args(["-axeww", "-o", "pid=,command="]).output().expect("ps");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.contains("ps -axeww"))
            .filter(|l| l.split_whitespace().next().and_then(|p| p.parse::<u32>().ok()) != Some(me))
            .map(String::from)
            .collect()
    }
}

/// Whether something holds the lock file `lock` (`flock`): trying to take it fails. A file that is not there, or
/// that cannot be opened, is no one's.
fn holds_lock(lock: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::OpenOptions::new().write(true).open(lock) else { return false };
    // SAFETY: `flock` on a descriptor we own; taken, it is let go with the file when this function ends.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) != 0 }
}

/// Stops a process now and then (SIGSTOP, then SIGCONT `pause` later), so that its reads come late, as on a slow
/// runner. Made to be harmless: before each SIGSTOP `alive` must say the process is still the one meant (a pid
/// may be taken by another once it is gone), a SIGCONT always follows a SIGSTOP, and dropping it (a failed
/// assertion included) ends the thread, waits for it and lets the process go.
pub struct Staller {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Staller {
    pub fn start(
        pid: u32,
        every: Duration,
        pause: Duration,
        alive: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Staller {
        use std::sync::atomic::Ordering;
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = done.clone();
        let pid = pid as libc::pid_t;
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                std::thread::sleep(every);
                // Checked at the last moment, and again for nothing else: no signal to what may not be the server.
                if flag.load(Ordering::Relaxed) || !alive() {
                    break;
                }
                // SAFETY: plain signals to the process the test started; the SIGCONT always follows.
                unsafe { libc::kill(pid, libc::SIGSTOP) };
                std::thread::sleep(pause);
                unsafe { libc::kill(pid, libc::SIGCONT) };
            }
        });
        Staller { done, thread: Some(thread) }
    }
}

impl Drop for Staller {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
