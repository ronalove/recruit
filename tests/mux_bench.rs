// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The native multiplexer's bench (spec §9, "Performance"): reproducible figures, compared with tmux in the same
//! run. Ignored by `cargo test`; run in release, one test at a time:
//!
//! ```sh
//! CARGO_PROFILE_RELEASE_LTO=off cargo test --release --test mux_bench -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Each measure runs in a child process of its own (this binary started again, see `common::child`), so that one
//! measure's allocations never weigh on the next. Each prints a markdown table, for `specs/multiplexeur-compat.md`.
//!
//! Owner: testeur.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::time::{Duration, Instant};

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

use common::pty::{Profile, TestTerm};
use common::server::Server;
use common::{Usage, fakes, mib};

/// Rows of every pane in the memory measures: a tall pane, as on a laptop screen.
const ROWS: usize = 50;

/// The history every pane keeps, as tmux's `history-limit` does today.
const HISTORY: usize = 100_000;

/// The entry point of every child: does what `common::CHILD` names, then exits.
#[test]
#[ignore = "started by the other tests only"]
fn child() {
    match common::role().as_deref() {
        Some("engine-memory") => engine_memory_child(),
        Some("history") => history_child(),
        Some(role) if common::fakes::run(role) => {}
        Some(other) => panic!("unknown child role {other}"),
        None => {}
    }
}

/// Waits for the parent's go-ahead: a line on stdin.
fn wait_parent() {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

/// What `history_child` writes: `lines` history lines, `\r\n` after each.
fn history_bytes(lines: usize, cols: usize, full: bool) -> Vec<u8> {
    // `BENCH_REPLAY`: a recorded terminal output instead (a real session's), played twice so that the history
    // fills at every width.
    if let Some(path) = std::env::var_os("BENCH_REPLAY") {
        let bytes = std::fs::read(path).expect("BENCH_REPLAY");
        return [bytes.as_slice(), b"\r\n", bytes.as_slice()].concat();
    }
    let mut out = Vec::new();
    for i in 0..lines {
        out.extend_from_slice(common::history_line(i, cols, full).as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// `panes` alacritty terminals, `cols` × `ROWS`, each fed `lines` lines in chunks of 64 KiB as a PTY reader
/// would; the parent measures this process before and after.
fn engine_memory_child() {
    let cols = common::env_usize("BENCH_COLS", 120);
    let panes = common::env_usize("BENCH_PANES", 1);
    let lines = common::env_usize("BENCH_LINES", HISTORY + ROWS);
    let full = common::env_usize("BENCH_FULL", 0) == 1;
    let bytes = history_bytes(lines, cols, full);
    println!("ready");
    wait_parent();
    let mut terms = Vec::new();
    for _ in 0..panes {
        let config = Config { scrolling_history: HISTORY, kitty_keyboard: true, ..Config::default() };
        let mut term = Term::new(config, &TermSize::new(cols, ROWS), VoidListener);
        let mut parser: Processor = Processor::new();
        for chunk in bytes.chunks(64 * 1024) {
            parser.advance(&mut term, chunk);
        }
        terms.push((term, parser));
    }
    drop(bytes);
    println!("fed {}", terms.len());
    wait_parent();
}

/// Writes `BENCH_LINES` history lines to stdout, says so on the last line, then stays quiet until killed: the
/// fake program of the tmux baseline.
fn history_child() {
    let cols = common::env_usize("BENCH_COLS", 120);
    let lines = common::env_usize("BENCH_LINES", HISTORY + ROWS);
    let full = common::env_usize("BENCH_FULL", 0) == 1;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&history_bytes(lines, cols, full));
    let _ = out.write_all(b"history-done");
    let _ = out.flush();
    drop(out);
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// One line of the memory table.
struct Memory {
    footprint: u64,
    resident: u64,
    virtual_size: u64,
}

/// The engine alone: `panes` panes of `cols` columns, 100 000 lines each. What the process gained.
fn engine_memory(cols: usize, panes: usize, full: bool) -> Memory {
    let mut command = common::child("engine-memory");
    command
        .env("BENCH_COLS", cols.to_string())
        .env("BENCH_PANES", panes.to_string())
        .env("BENCH_FULL", if full { "1" } else { "0" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = command.spawn().expect("child");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut expect = |want: &str| {
        let mut line = String::new();
        loop {
            line.clear();
            assert!(stdout.read_line(&mut line).expect("child output") > 0, "the child ended before {want}");
            if line.starts_with(want) {
                break;
            }
        }
    };
    expect("ready");
    let before = Usage::of(child.id()).expect("child usage");
    writeln!(stdin).expect("go");
    expect("fed");
    let after = Usage::of(child.id()).expect("child usage");
    writeln!(stdin).expect("go");
    let _ = child.wait();
    Memory {
        footprint: after.footprint.saturating_sub(before.footprint),
        resident: after.resident.saturating_sub(before.resident),
        virtual_size: after.virtual_size.saturating_sub(before.virtual_size),
    }
}

/// tmux 3.5 or later, with `history-limit 100000`: `panes` panes of `cols` columns, each running the history
/// fake. What the tmux server gained over a server with one idle pane.
fn tmux_memory(cols: usize, panes: usize, full: bool) -> Memory {
    let _ = common::tmux().arg("kill-server").stderr(Stdio::null()).status();
    // A server that is still exiting would refuse the new session.
    std::thread::sleep(Duration::from_millis(300));
    let size = [cols.to_string(), ROWS.to_string()];
    // A server still exiting refuses the session now and then: three tries.
    let mut tries = 0;
    loop {
        let output = common::tmux()
            .args(["new-session", "-d", "-s", "bench", "-x", &size[0], "-y", &size[1], "sleep 3600"])
            .stdin(Stdio::null())
            .output()
            .expect("tmux");
        if output.status.success() {
            break;
        }
        tries += 1;
        assert!(tries < 3, "tmux new-session: {}", String::from_utf8_lossy(&output.stderr).trim());
        std::thread::sleep(Duration::from_secs(1));
    }
    common::tmux_run(["set-option", "-g", "history-limit", &HISTORY.to_string()]);
    let server: u32 = common::tmux_run(["display-message", "-p", "#{pid}"]).parse().expect("tmux pid");
    std::thread::sleep(Duration::from_millis(300));
    let before = Usage::of(server).expect("tmux usage");
    let vars = [("BENCH_COLS", cols.to_string()), ("BENCH_FULL", if full { "1" } else { "0" }.to_string())];
    let words: Vec<String> =
        common::child_words("history", &vars).iter().map(|w| w.to_string_lossy().into_owned()).collect();
    let command = shlex::try_join(words.iter().map(String::as_str)).expect("command");
    for _ in 0..panes {
        // One window per pane, each of the same size: the server keeps every window's history.
        common::tmux_run(["new-window", "-d", "-t", "=bench", &command]);
    }
    // tmux drops a tenth of the history when it is full: the fake's last line says when it is done.
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let windows = common::tmux_run(["list-windows", "-t", "=bench", "-F", "#{window_index}"]);
        let done = windows
            .lines()
            .filter(|w| *w != "0")
            .filter(|w| common::tmux_run(["capture-pane", "-p", "-t", &format!("=bench:{w}")]).contains("history-done"))
            .count();
        if done >= panes {
            break;
        }
        assert!(Instant::now() < deadline, "tmux panes still filling: {done} of {panes} done");
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(500));
    let after = Usage::of(server).expect("tmux usage");
    let _ = common::tmux().arg("kill-server").status();
    Memory {
        footprint: after.footprint.saturating_sub(before.footprint),
        resident: after.resident.saturating_sub(before.resident),
        virtual_size: after.virtual_size.saturating_sub(before.virtual_size),
    }
}

/// Memory per pane with 100 000 lines of history (spec §8, step 0's measures): alacritty's engine alone, and
/// tmux, for 80, 120 and 200 columns, lines of spread lengths ("typical") and full lines. Per pane: the slope
/// between 1 and 10 panes, so that what a process costs once does not count.
#[test]
#[ignore = "bench: run in release, see the module's doc"]
fn memory_per_pane() {
    let tmux = common::has_tmux();
    println!();
    println!(
        "| Lines | Columns | Engine | 1 pane: footprint | 10 panes: footprint | Per pane: footprint | Per pane: resident | Per pane: virtual |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    // With `BENCH_REPLAY`, the recorded output only (its lines are what they are: `full` does not apply).
    let kinds: &[bool] = if std::env::var_os("BENCH_REPLAY").is_some() { &[false] } else { &[false, true] };
    let replay = std::env::var_os("BENCH_REPLAY").is_some();
    for &full in kinds {
        for cols in [80, 120, 200] {
            let mut rows = vec![("alacritty 0.26", engine_memory(cols, 1, full), engine_memory(cols, 10, full))];
            if tmux {
                rows.push(("tmux", tmux_memory(cols, 1, full), tmux_memory(cols, 10, full)));
            }
            for (engine, one, ten) in rows {
                let per = |a: u64, b: u64| mib(b.saturating_sub(a)) / 9.0;
                println!(
                    "| {} | {cols} | {engine} | {:.1} Mio | {:.1} Mio | {:.1} Mio | {:.1} Mio | {:.1} Mio |",
                    if replay {
                        "replay"
                    } else if full {
                        "full"
                    } else {
                        "typical"
                    },
                    mib(one.footprint),
                    mib(ten.footprint),
                    per(one.footprint, ten.footprint),
                    per(one.resident, ten.resident),
                    per(one.virtual_size, ten.virtual_size),
                );
            }
        }
    }
}

/// The outer terminal of the latency measures, as a laptop's full screen.
const OUTER: (u16, u16) = (200, 50);

/// Samples per latency condition, and the pause between two, drawn between these bounds so that the samples do
/// not lock onto the multiplexer's frame rate.
const SAMPLES: usize = 500;
const PAUSE_MS: (u64, u64) = (20, 50);

/// A small deterministic generator for the pauses (the same run after run).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, low: u64, high: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        low + (self.0 >> 33) % (high - low + 1)
    }
}

/// Percentiles of a set of durations, in milliseconds.
struct Spread {
    /// Bytes and frames per second sent to the outer terminal, and the CPU of the measured process (the
    /// multiplexer, or tmux's server) in percent of one core, over the samples.
    out_mib_s: f64,
    frames_s: f64,
    cpu: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    missed: usize,
}

fn spread(mut samples: Vec<Duration>, missed: usize) -> Spread {
    samples.sort();
    let at = |q: f64| {
        let i = ((samples.len() as f64 - 1.0) * q).round() as usize;
        samples.get(i).map_or(f64::NAN, |d| d.as_secs_f64() * 1000.0)
    };
    Spread {
        out_mib_s: 0.0,
        frames_s: 0.0,
        cpu: 0.0,
        p50: at(0.50),
        p95: at(0.95),
        p99: at(0.99),
        max: at(1.0),
        missed,
    }
}

/// The key typed in the latency measures.
#[derive(Clone, Copy)]
enum Key {
    X,
    Escape,
}

impl Key {
    /// What a terminal sends for it: Escape as `CSI 27 u` once the kitty protocol was pushed to it.
    fn bytes(self, kitty: bool) -> &'static [u8] {
        match (self, kitty) {
            (Key::X, _) => b"x",
            (Key::Escape, false) => b"\x1b",
            (Key::Escape, true) => b"\x1b[27u",
        }
    }
}

/// What runs on the outer terminal for one condition.
enum Setup {
    /// The echo fake alone, on the test's PTY: the reference.
    Direct,
    /// A native server of the test's own (`common::server`) with `panes` panes, the echo first (it has the
    /// focus), the others `others`, and a real client on the test's terminal.
    Mux { panes: usize, others: &'static str },
    /// tmux, likewise.
    Tmux { panes: usize, others: &'static str },
}

/// Starts `setup` on a test terminal and gives the focus to the echo pane; returns the terminal and the count of
/// bytes the echo has seen.
/// `extra` goes in the fakes' environment (`FLOOD_RATE`, `FLOOD_FRAME`, `FLOOD_COUNT_DIR`…).
fn start(
    setup: &Setup,
    profile: Profile,
    dir: &std::path::Path,
    extra: &[(&str, String)],
) -> (TestTerm, usize, Option<Server>) {
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let run = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let lock = dir.join(format!("echo-{run}.lock"));
    let _ = std::fs::remove_file(&lock);
    let (panes, others) = match setup {
        Setup::Direct => (1, "idle"),
        Setup::Mux { panes, others } | Setup::Tmux { panes, others } => (*panes, *others),
    };
    let mut vars = vec![
        ("ONE_OF_LOCK", lock.to_string_lossy().into_owned()),
        ("ONE_OF_OTHERS", others.to_string()),
        ("FLOOD_COLS", "120".to_string()),
    ];
    vars.extend(extra.iter().cloned());
    let fake = common::child_words("one-of", &vars);
    let (cols, rows) = OUTER;
    let mut term = match setup {
        Setup::Direct => TestTerm::spawn(&fake[0], &fake[1..], &[], cols, rows, profile),
        Setup::Mux { .. } => {
            let root = dir.join(format!("mux-{run}"));
            std::fs::create_dir_all(&root).expect("server root");
            let server = Server::start(&root);
            let mut vars: Vec<(&str, String)> = vec![("FLOOD_COLS", "120".to_string())];
            vars.extend(extra.iter().cloned());
            server.spawn_fake("echo", "echo", &vars);
            for i in 1..panes {
                server.spawn_fake(&format!("p{i}"), others, &vars);
            }
            // Its output kept, to show what it said if it ends.
            let mut term = server.attach_as(cols, rows, Profile { capture: true, ..profile });
            assert!(term.wait_for(&fakes::echo_marker(0), Duration::from_secs(10)), "no echo: {:#?}", term.screen());
            term.send(b"x");
            assert!(term.wait_for(&fakes::echo_marker(1), Duration::from_secs(5)), "{:#?}", term.screen());
            return (term, 1, Some(server));
        }
        Setup::Tmux { .. } => {
            let _ = common::tmux().arg("kill-server").stderr(Stdio::null()).status();
            std::thread::sleep(Duration::from_millis(300));
            let command = shlex::try_join(fake.iter().map(|w| w.to_str().expect("UTF-8"))).expect("command");
            // As recruit's own tmux configuration has it (escape-time 10), and without a status line. In a file:
            // set by commands after `new-session`, escape-time did not reach the client it attached.
            let conf = dir.join("tmux.conf");
            std::fs::write(&conf, "set -s escape-time 10\nset -g status off\n").expect("tmux.conf");
            let conf = conf.to_string_lossy().into_owned();
            let mut args: Vec<String> =
                ["-L", common::TMUX_SOCKET, "-f", &conf, "new-session", "-s", "lat"].map(String::from).to_vec();
            args.push(command.clone());
            for _ in 1..panes {
                args.extend([";".into(), "split-window".into(), command.clone()]);
                args.extend([";".into(), "select-layout".into(), "tiled".into()]);
            }
            TestTerm::spawn(std::ffi::OsStr::new("tmux"), &args, &[], cols, rows, profile)
        }
    };
    assert!(term.wait_for(&fakes::echo_marker(0), Duration::from_secs(10)), "no echo pane: {:#?}", term.screen());
    if matches!(setup, Setup::Tmux { .. }) && std::env::var_os("BENCH_DEBUG").is_some() {
        println!("tmux : {}", common::tmux_run(["show-options", "-s", "escape-time"]));
    }
    // Find the pane that echoes: a probe byte, then the next pane, until the echo counts it. The count is read
    // on the screen each time: a probe that reached the echo late still counts.
    for _ in 0..panes {
        let seen = echo_count(&term).unwrap_or(0);
        term.send(b"x");
        if term.wait_for(&fakes::echo_marker(seen + 1), Duration::from_secs(2)) {
            return (term, seen + 1, None);
        }
        if let Setup::Tmux { .. } = setup {
            common::tmux_run(["select-pane", "-t", "=lat:.+"]);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    panic!("the echo pane never got the focus: {:#?}", term.screen());
}

/// What `pid` costs so far; said, and zero, if it cannot be read (a process gone, a wrong pid).
fn usage(pid: u32) -> Usage {
    Usage::of(pid).unwrap_or_else(|| {
        println!("No usage readable for pid {pid}: the figures of this line are wrong.");
        Usage::default()
    })
}

/// The processes whose cost is measured: the multiplexer's server and its client, tmux's server, or the fake
/// alone.
fn measured(setup: &Setup, term: &TestTerm, server: Option<&Server>) -> Vec<u32> {
    match (setup, server) {
        (Setup::Tmux { .. }, _) => {
            vec![common::tmux_run(["display-message", "-p", "#{pid}"]).parse().expect("tmux pid")]
        }
        (Setup::Mux { .. }, Some(server)) => vec![server.pid(), term.pid()],
        _ => vec![term.pid()],
    }
}

/// What `pids` cost so far, added up.
fn usage_of(pids: &[u32]) -> Usage {
    pids.iter().map(|p| usage(*p)).fold(Usage::default(), |a, u| Usage {
        footprint: a.footprint + u.footprint,
        resident: a.resident + u.resident,
        virtual_size: a.virtual_size + u.virtual_size,
        cpu_ns: a.cpu_ns + u.cpu_ns,
        wakeups: a.wakeups + u.wakeups,
    })
}

/// The count the echo shows now (`K000012`), if it is on the screen.
fn echo_count(term: &TestTerm) -> Option<usize> {
    term.screen().iter().find_map(|row| {
        let i = row.find('K')?;
        row.get(i + 1..i + 7)?.parse().ok()
    })
}

/// Ends `setup`'s programs.
fn stop(setup: &Setup, term: TestTerm, server: Option<Server>) {
    match setup {
        Setup::Mux { .. } => {
            drop(term);
            if let Some(mut server) = server {
                server.stop();
            }
        }
        Setup::Tmux { .. } => {
            let _ = common::tmux().arg("kill-server").status();
        }
        Setup::Direct => {}
    }
}

/// The time from a key typed on the outer terminal to its echo on the outer screen, `SAMPLES` times: `byte` is
/// what is typed.
fn latency(setup: &Setup, key: Key, profile: Profile, dir: &std::path::Path, extra: &[(&str, String)]) -> Spread {
    let (mut term, mut count, server) = start(setup, profile, dir, extra);
    // Let the floods reach their pace.
    std::thread::sleep(Duration::from_secs(1));
    let mut rng = Lcg(0x5eed);
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut missed = 0;
    let pids = measured(setup, &term, server.as_ref());
    let (bytes0, frames0) = term.counters();
    let usage0 = usage_of(&pids);
    let began = Instant::now();
    for _ in 0..SAMPLES {
        std::thread::sleep(Duration::from_millis(rng.next(PAUSE_MS.0, PAUSE_MS.1)));
        // Whatever the outer terminal sends, the pane gets one byte: the echo does not ask for the kitty protocol.
        let want = fakes::echo_marker(count + 1);
        let bytes = key.bytes(term.kitty_keyboard());
        match term.round_trip(bytes, &want, Duration::from_secs(2)) {
            Some(took) => {
                samples.push(took);
                count += 1;
            }
            None => {
                missed += 1;
                if let Some(status) = term.wait_exit(Duration::ZERO) {
                    let raw = term.raw();
                    let tail =
                        String::from_utf8_lossy(&raw[raw.len().saturating_sub(600)..]).escape_debug().to_string();
                    println!("The program ended during the samples ({status}); the end of its output: {tail}");
                    break;
                }
                // Resynchronise on whatever the echo shows now.
                count = echo_count(&term).unwrap_or(count);
                if missed >= 20 {
                    println!("20 keys missed: stopped; the screen: {:#?}", term.screen());
                    break;
                }
            }
        }
    }
    let secs = began.elapsed().as_secs_f64();
    let (bytes1, frames1) = term.counters();
    let usage1 = usage_of(&pids);
    stop(setup, term, server);
    let mut out = spread(samples, missed);
    out.out_mib_s = mib(bytes1 - bytes0) / secs;
    out.frames_s = (frames1 - frames0) as f64 / secs;
    out.cpu = (usage1.cpu_ns.saturating_sub(usage0.cpu_ns)) as f64 / 1e9 / secs * 100.0;
    out
}

/// Latency added to a key (spec §8, step 0's measures): typed on the outer terminal, echoed by a fake in the
/// focused pane, read back on the outer screen. The reference is the fake alone on the test's terminal; the
/// multiplexer and tmux each with 1 pane, then 10 (9 of them flooding). Then Escape alone, which a terminal
/// reader might hold back to see whether a sequence follows.
#[test]
#[ignore = "bench: run in release, see the module's doc"]
fn latency_added() {
    let dir = tempfile::tempdir().expect("temp dir");
    // Big synchronized updates, as Claude Code's full-screen frames, only bigger: the engine parses each at its end.
    let frames = || vec![("FLOOD_FRAME", "120000".to_string()), ("FLOOD_PAUSE_MS", "50".to_string())];
    let mut setups: Vec<(&str, Setup, Env)> = vec![
        ("direct", Setup::Direct, vec![]),
        ("recruit, 1 pane", Setup::Mux { panes: 1, others: "idle" }, vec![]),
        ("recruit, 10 panes, 9 flooding", Setup::Mux { panes: 10, others: "flood" }, vec![]),
        ("recruit, 10 panes, 9 in 120 KB frames", Setup::Mux { panes: 10, others: "flood" }, frames()),
    ];
    if common::has_tmux() {
        setups.push(("tmux, 1 pane", Setup::Tmux { panes: 1, others: "idle" }, vec![]));
        setups.push(("tmux, 10 panes, 9 flooding", Setup::Tmux { panes: 10, others: "flood" }, vec![]));
        setups.push(("tmux, 10 panes, 9 in 120 KB frames", Setup::Tmux { panes: 10, others: "flood" }, frames()));
    }
    // `LATENCY_ONLY=mux` (or `tmux`, `direct`) keeps the setups whose name starts so; `LATENCY_KEYS=x`.
    if let Ok(only) = std::env::var("LATENCY_ONLY") {
        setups.retain(|(name, ..)| name.starts_with(&only) || name.contains(&only));
    }
    let keys_wanted = std::env::var("LATENCY_KEYS").unwrap_or_default();
    println!();
    println!(
        "| Setup | Key | Terminal | p50 | p95 | p99 | max | Missed | Added p50 | Added p99 | Out | Frames | CPU |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    let legacy = Profile { kitty_keyboard: false, ..Profile::default() };
    let kitty = Profile { kitty_keyboard: true, ..Profile::default() };
    let runs = [
        ("x", Key::X, "kitty", kitty),
        ("Escape", Key::Escape, "legacy", legacy),
        ("Escape", Key::Escape, "kitty", kitty),
    ];
    for (key, which, terminal, profile) in runs {
        if !keys_wanted.is_empty() && !keys_wanted.split(',').any(|k| k == key) {
            continue;
        }
        let mut reference = None;
        for (name, setup, extra) in &setups {
            let s = latency(setup, which, profile, dir.path(), extra);
            let (p50, p99) = *reference.get_or_insert((s.p50, s.p99));
            println!(
                "| {name} | {key} | {terminal} | {:.2} ms | {:.2} ms | {:.2} ms | {:.2} ms | {} | {:+.2} ms | {:+.2} ms | {:.2} Mio/s | {:.0}/s | {:.0} % |",
                s.p50,
                s.p95,
                s.p99,
                s.max,
                s.missed,
                s.p50 - p50,
                s.p99 - p99,
                s.out_mib_s,
                s.frames_s,
                s.cpu,
            );
        }
    }
}

/// What the measured processes (the multiplexer's server and client, or tmux's server) cost over `secs` seconds.
struct Cost {
    cpu: f64,
    wakeups_s: f64,
    out_bytes: u64,
    frames_s: f64,
    /// CPU time per frame sent, in microseconds.
    per_frame_us: f64,
    /// What the floods wrote, in Mio/s, and the CPU seconds per Mio of it.
    in_mib_s: f64,
    cpu_per_mib: f64,
}

fn cost(setup: &Setup, term: &TestTerm, server: Option<&Server>, secs: u64, counted: Option<&std::path::Path>) -> Cost {
    let flooded = |dir: Option<&std::path::Path>| dir.map_or(0, fakes::flooded);
    let consumed0 = flooded(counted);
    let pids = measured(setup, term, server);
    let (bytes0, frames0) = term.counters();
    let usage0 = usage_of(&pids);
    let began = Instant::now();
    std::thread::sleep(Duration::from_secs(secs));
    let elapsed = began.elapsed().as_secs_f64();
    let (bytes1, frames1) = term.counters();
    let usage1 = usage_of(&pids);
    let consumed = flooded(counted).saturating_sub(consumed0);
    let cpu_s = usage1.cpu_ns.saturating_sub(usage0.cpu_ns) as f64 / 1e9;
    let frames = frames1 - frames0;
    Cost {
        cpu: cpu_s / elapsed * 100.0,
        wakeups_s: usage1.wakeups.saturating_sub(usage0.wakeups) as f64 / elapsed,
        out_bytes: bytes1 - bytes0,
        frames_s: frames as f64 / elapsed,
        per_frame_us: if frames > 0 { cpu_s * 1e6 / frames as f64 } else { f64::NAN },
        in_mib_s: mib(consumed) / elapsed,
        cpu_per_mib: if consumed > 0 { cpu_s / mib(consumed) } else { f64::NAN },
    }
}

/// One line of the CPU table: its name, what runs, the fakes' extra environment, the seconds measured.
type Run = (String, Setup, Env, u64);

/// What goes in the fakes' environment on top of their role's.
type Env = Vec<(&'static str, String)>;

/// Two real Claude Code at rest, in the multiplexer or tmux: what the multiplexer itself costs (Claude's own CPU
/// is not counted). The sessions run in the tester's folder for real sessions (see `tests/mux_claude.rs`).
fn claude_at_rest(mux: bool, dir: &std::path::Path, secs: u64) -> Cost {
    let work = std::env::temp_dir().join("recruit-testeur-claude");
    std::fs::create_dir_all(&work).expect("work dir");
    let claude = ["claude", "-n", "testeur-compat"];
    let (cols, rows) = OUTER;
    let (setup, term, server) = if mux {
        let root = dir.join("claude-at-rest");
        std::fs::create_dir_all(&root).expect("server root");
        let server = Server::start(&root);
        server.spawn("c1", &work, &[], &claude);
        server.spawn("c2", &work, &[], &claude);
        let term = server.attach_as(cols, rows, Profile::default());
        (Setup::Mux { panes: 2, others: "idle" }, term, Some(server))
    } else {
        let _ = common::tmux().arg("kill-server").stderr(Stdio::null()).status();
        std::thread::sleep(Duration::from_millis(300));
        let conf = dir.join("tmux.conf");
        std::fs::write(&conf, "set -s escape-time 10\nset -g status off\n").expect("tmux.conf");
        let command = claude.join(" ");
        let conf = conf.to_string_lossy().into_owned();
        let args = [
            "-L",
            common::TMUX_SOCKET,
            "-f",
            &conf,
            "new-session",
            "-s",
            "lat",
            &command,
            ";",
            "split-window",
            "-h",
            &command,
        ];
        let term =
            TestTerm::spawn_in(Some(&work), std::ffi::OsStr::new("tmux"), args, &[], cols, rows, Profile::default());
        (Setup::Tmux { panes: 2, others: "idle" }, term, None)
    };
    // Both up, then their first frames done.
    let _ = term.wait_for("❯", Duration::from_secs(30));
    std::thread::sleep(Duration::from_secs(10));
    let c = cost(&setup, &term, server.as_ref(), secs, None);
    stop(&setup, term, server);
    c
}

/// CPU at rest and in a flood (spec §8, step 0's measures), the multiplexer against tmux:
/// - at rest: 2 real Claude Code, then 10 fakes (`CPU_SECS` seconds, 60 by default, after 5 s to settle);
/// - at Claude's pace: 1 and 10 panes writing 20 KB/s each, in synchronized slices every 16 ms;
/// - flat out: 1 and 10 panes as fast as they can, with what they really wrote (the floods count it), and the CPU
///   per Mio of it.
///
/// Wakeups: interrupt wakeups (macOS) or voluntary context switches (Linux) of the measured process. Frames:
/// synchronized updates sent to the outer terminal (tmux sends them too).
#[test]
#[ignore = "bench: run in release, see the module's doc"]
fn cpu_rest_and_flood() {
    let dir = tempfile::tempdir().expect("temp dir");
    let secs = common::env_usize("CPU_SECS", 60) as u64;
    let flood_secs = (secs / 3).max(10);
    let only = std::env::var("CPU_ONLY").ok();
    let wanted = |name: &str| only.as_deref().is_none_or(|o| name.contains(o));
    println!();
    println!(
        "| Setup | Seconds | CPU | Wakeups/s | Bytes out | Frames/s | CPU per frame | Floods wrote | CPU per Mio |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    let print = |name: &str, secs: u64, c: &Cost| {
        println!(
            "| {name} | {secs} | {:.2} % | {:.1} | {} | {:.1} | {:.0} µs | {:.2} Mio/s | {:.3} s |",
            c.cpu, c.wakeups_s, c.out_bytes, c.frames_s, c.per_frame_us, c.in_mib_s, c.cpu_per_mib
        );
    };
    for (tool, mux) in [("recruit", true), ("tmux", false)] {
        if !mux && !common::has_tmux() {
            continue;
        }
        let name = format!("{tool}, 2 Claude at rest");
        // Real sessions: only with `CLAUDE_SESSIONS` (see `common::consented`).
        if wanted(&name)
            && common::consented("CLAUDE_SESSIONS", "the line with real Claude Code sessions", "cpu_rest_and_flood")
        {
            print(&name, secs, &claude_at_rest(mux, dir.path(), secs));
        }
        let setup = |panes: usize, others: &'static str| {
            if mux { Setup::Mux { panes, others } } else { Setup::Tmux { panes, others } }
        };
        let runs: Vec<Run> = vec![
            (format!("{tool}, 10 fakes at rest"), setup(10, "idle"), vec![], secs),
            // 9 panes that wrote 100 000 lines each, then nothing: measured once their compression is over.
            (format!("{tool}, 9 full histories at rest"), setup(10, "fill"), vec![("SETTLE", "30".into())], secs),
            (format!("{tool}, 1 pane at 20 KB/s"), setup(2, "flood"), vec![("FLOOD_RATE", "20000".into())], flood_secs),
            (
                format!("{tool}, 9 panes at 20 KB/s"),
                setup(10, "flood"),
                vec![("FLOOD_RATE", "20000".into())],
                flood_secs,
            ),
            (format!("{tool}, 1 pane flat out"), setup(2, "flood"), vec![], flood_secs),
            (format!("{tool}, 9 panes flat out"), setup(10, "flood"), vec![], flood_secs),
        ];
        for (n, (name, setup, mut extra, secs)) in runs.into_iter().enumerate() {
            if !wanted(&name) {
                continue;
            }
            let counted = dir.path().join(format!("counts-{tool}-{n}").replace(' ', "-"));
            std::fs::create_dir_all(&counted).expect("counts");
            extra.push(("FLOOD_COUNT_DIR", counted.to_string_lossy().into_owned()));
            let settle = extra.iter().find(|(k, _)| *k == "SETTLE").and_then(|(_, v)| v.parse().ok()).unwrap_or(5);
            let (term, _, server) = start(&setup, Profile::default(), dir.path(), &extra);
            std::thread::sleep(Duration::from_secs(settle));
            let c = cost(&setup, &term, server.as_ref(), secs, Some(&counted));
            stop(&setup, term, server);
            print(&name, secs, &c);
        }
    }
}

/// Memory of one pane through the multiplexer itself: the footprint of a native server whose one pane holds 100 000
/// lines (`BENCH_REPLAY` for a recorded output), less that of the same server with an empty pane, at 80, 120 and
/// 200 columns of pane (a client attached, a little bigger than the pane).
#[test]
#[ignore = "bench: run in release, see the module's doc"]
fn memory_through_mux() {
    let dir = tempfile::tempdir().expect("temp dir");
    println!();
    println!("| Pane columns | Empty pane | 100 000 lines | Per pane | Virtual, per pane |");
    println!("|---|---|---|---|---|");
    for cols in [80usize, 120, 200] {
        let measure = |role: &str| -> Usage {
            let root = dir.path().join(format!("mem-{cols}-{role}"));
            std::fs::create_dir_all(&root).expect("server root");
            let mut server = Server::start(&root);
            // The pane's frame and the bar take a few cells: a client a little bigger than the pane.
            let term = server.attach_as(cols as u16 + 2, ROWS as u16 + 3, Profile::default());
            std::thread::sleep(Duration::from_millis(500));
            server.spawn_fake("history", role, &[("BENCH_COLS", cols.to_string())]);
            if role == "history" {
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    server.wait_pane("history", "history-done", Duration::from_secs(300))
                }));
                assert!(matches!(done, Ok(true)), "the server's log:\n{}", server.log_tail());
            } else {
                std::thread::sleep(Duration::from_secs(2));
            }
            // Time for the history's compression, which runs once the pane is quiet (`MEM_SETTLE` seconds).
            std::thread::sleep(Duration::from_secs(common::env_usize("MEM_SETTLE", 10) as u64));
            let usage = usage(server.pid());
            drop(term);
            server.stop();
            usage
        };
        let empty = measure("idle");
        let full = measure("history");
        println!(
            "| {cols} | {:.1} Mio | {:.1} Mio | {:.1} Mio | {:.1} Mio |",
            mib(empty.footprint),
            mib(full.footprint),
            mib(full.footprint.saturating_sub(empty.footprint)),
            mib(full.virtual_size.saturating_sub(empty.virtual_size)),
        );
    }
}
