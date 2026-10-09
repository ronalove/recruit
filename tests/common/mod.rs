// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the native multiplexer's integration tests and bench share: the test binary started again as a fake
//! program or a measured child, what a process costs (memory, CPU, wakeups), the lines the fakes write, and the
//! tester's own tmux server for the baselines (`-L rtest-testeur`, never the user's).
//!
//! Owner: testeur.

// Each test crate uses its own part of this module.
#![allow(dead_code)]

pub mod fakes;
pub mod pty;
pub mod server;
pub mod team;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The environment variable that turns the test binary into one of its children (a fake program, a measured
/// process): its value names the role, the test `child` reads it.
pub const CHILD: &str = "RECRUIT_TEST_CHILD";

/// The tester's tmux server, for the baselines.
pub const TMUX_SOCKET: &str = "rtest-testeur";

/// A command that runs this test binary again as the child `role`: only the test named `child` runs, and it hands
/// over to the role at once.
pub fn child(role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command.args(["child", "--exact", "--ignored", "--nocapture", "--test-threads=1", "-q"]).env(CHILD, role);
    command
}

/// The same command, as words for a shell or for tmux (`env CHILD=role exe child …`), plus `vars`.
pub fn child_words(role: &str, vars: &[(&str, String)]) -> Vec<OsString> {
    let mut words: Vec<OsString> = vec!["env".into(), format!("{CHILD}={role}").into()];
    words.extend(vars.iter().map(|(key, value)| OsString::from(format!("{key}={value}"))));
    words.push(std::env::current_exe().expect("the test binary").into());
    words.extend(["child", "--exact", "--ignored", "--nocapture", "--test-threads=1", "-q"].map(OsString::from));
    words
}

/// The role this process was started for, if any.
pub fn role() -> Option<String> {
    std::env::var(CHILD).ok()
}

/// A number from the environment, or `default`.
pub fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// The text of history line `i` for a terminal `cols` wide, as a program like Claude Code writes it: lengths
/// spread from empty to the full width (deterministic), a few words in color, ASCII only. With `full`, every line
/// fills the width.
pub fn history_line(i: usize, cols: usize, full: bool) -> String {
    const WORDS: [&str; 8] = ["the", "quick", "brown", "fox", "jumps", "over", "a", "lazy"];
    let len = if full { cols } else { (i * 37 + 11) % (cols + 1) };
    let mut out = format!("{i:06}");
    let mut visible = out.len().min(len);
    out.truncate(visible);
    let mut w = i;
    while visible < len {
        let word = WORDS[w % WORDS.len()];
        let take = word.len().min(len - visible - 1);
        if visible + 1 >= len {
            break;
        }
        out.push(' ');
        if w.is_multiple_of(5) {
            out.push_str("\x1b[38;2;215;119;87m");
            out.push_str(&word[..take]);
            out.push_str("\x1b[0m");
        } else {
            out.push_str(&word[..take]);
        }
        visible += 1 + take;
        w += 1;
    }
    out
}

/// What a process costs at one moment.
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    /// Physical footprint (macOS), or proportional set size (Linux): what the process really costs.
    pub footprint: u64,
    pub resident: u64,
    pub virtual_size: u64,
    /// CPU time, user and system, in nanoseconds.
    pub cpu_ns: u64,
    /// Wakeups (macOS: interrupt wakeups; Linux: voluntary context switches).
    pub wakeups: u64,
}

impl Usage {
    /// The usage of process `pid`, or `None` once it is gone.
    #[cfg(target_os = "macos")]
    pub fn of(pid: u32) -> Option<Usage> {
        let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        // SAFETY: the buffer is a `rusage_info_v4`, the flavor asked for.
        let rc = unsafe {
            libc::proc_pid_rusage(
                pid as libc::c_int,
                libc::RUSAGE_INFO_V4,
                (&raw mut info).cast::<libc::rusage_info_t>(),
            )
        };
        if rc != 0 {
            return None;
        }
        let mut task: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        // SAFETY: the buffer is a `proc_taskinfo`, of the size given.
        let got =
            unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTASKINFO, 0, (&raw mut task).cast(), size) };
        let virtual_size = if got == size { task.pti_virtual_size } else { 0 };
        // ri_user_time and ri_system_time are in Mach absolute time units (125/3 ns on Apple silicon).
        let (numer, denom) = timebase();
        let ticks = info.ri_user_time + info.ri_system_time;
        let cpu_ns = (u128::from(ticks) * u128::from(numer) / u128::from(denom.max(1))) as u64;
        Some(Usage {
            footprint: info.ri_phys_footprint,
            resident: info.ri_resident_size,
            virtual_size,
            cpu_ns,
            wakeups: info.ri_interrupt_wkups,
        })
    }

    #[cfg(target_os = "linux")]
    pub fn of(pid: u32) -> Option<Usage> {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let field = |name: &str| -> u64 {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };
        let pss = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("Pss:"))
                    .and_then(|r| r.split_whitespace().next())
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .unwrap_or_else(|| field("VmRSS:"));
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The fields after the command name, which is in parentheses and may hold spaces.
        let after = &stat[stat.rfind(')')? + 2..];
        let fields: Vec<&str> = after.split_whitespace().collect();
        let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
        // SAFETY: a plain query.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        Some(Usage {
            footprint: pss * 1024,
            resident: field("VmRSS:") * 1024,
            virtual_size: field("VmSize:") * 1024,
            cpu_ns: ticks * 1_000_000_000 / hz,
            wakeups: field("voluntary_ctxt_switches:"),
        })
    }
}

/// The Mach timebase: libc marks it deprecated in favor of the mach2 crate, a dependency the tests can do without.
#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn timebase() -> (u32, u32) {
    let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
    // SAFETY: a plain out-parameter.
    unsafe { libc::mach_timebase_info(&raw mut info) };
    (info.numer, info.denom)
}

/// Mebibytes, for the tables.
pub fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The tester's tmux, without any configuration and without the `TMUX` of the session the tests run in.
pub fn tmux() -> Command {
    let mut command = Command::new("tmux");
    command.args(["-L", TMUX_SOCKET, "-f", "/dev/null"]).env_remove("TMUX").env_remove("TMUX_PANE");
    command
}

/// Runs a tmux command, and returns its output (trimmed), or panics with its error.
pub fn tmux_run<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = tmux().args(args).stdin(Stdio::null()).output().expect("tmux");
    assert!(output.status.success(), "tmux: {}", String::from_utf8_lossy(&output.stderr).trim());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Whether tmux is there at all.
pub fn has_tmux() -> bool {
    Command::new("tmux").arg("-V").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Whether the variable `var` holds today's date (`date +%F`): the lock on what a whole `cargo test -- --ignored`
/// must never set off by itself (real keystrokes, real Claude Code sessions, which use the user's quota). Says why
/// when it does not.
///
/// The test must also be named on the command line (`test`, among `std::env::args()`): a variable exported for one
/// run stays set all day, and a later, general `--ignored` must not open the lock with it.
pub fn consented(var: &str, what: &str, test: &str) -> bool {
    let today = Command::new("date").arg("+%F").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let given = std::env::var(var).ok();
    let named = std::env::args().skip(1).any(|a| a == test);
    if !named {
        println!("Skipped: {what}. Name this test on the command line ({test}), with {var} set to today's date.");
        return false;
    }
    let ok = matches!((&given, &today), (Some(given), Ok(today)) if given == today);
    if !ok {
        println!(
            "Skipped: {what}. Set {var}={} (today) once the user has said go.",
            today.as_deref().unwrap_or("YYYY-MM-DD")
        );
    }
    ok
}

/// Runs `command`'s `run` (`output`, `spawn`), again for a moment while it fails with "text file busy" (ETXTBSY):
/// under Linux, a copy of recruit just written by one test cannot be run while a process another test forks
/// at that moment still holds it open for writing (until its own `exec`).
pub fn retry_busy<T>(mut run: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut tries = 0;
    loop {
        match run() {
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 40 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// A copy of the recruit binary in `dir`: others rebuild `target/` while the tests run.
pub fn recruit_copy(dir: &Path) -> PathBuf {
    let copy = dir.join("recruit");
    std::fs::copy(env!("CARGO_BIN_EXE_recruit"), &copy).expect("copy of the recruit binary");
    copy
}
