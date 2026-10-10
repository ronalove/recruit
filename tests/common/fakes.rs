// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The fake programs the tests put in panes: the test binary started again with `common::CHILD` set to one of
//! these roles (see `common::child_words`). Each writes known sequences, so that a screen can be compared as text.
//!
//! - `echo`: raw mode; after each byte it reads, writes `K` and the count of bytes so far (`K000001`) at the top
//!   left. The latency measures watch for it.
//! - `flood`: lines of text as fast as the PTY takes them; or `FLOOD_RATE` bytes per second, in slices of 16 ms
//!   between `BSU` and `ESU`, as Claude Code streams; or synchronized updates of `FLOOD_FRAME` bytes each,
//!   `FLOOD_PAUSE_MS` apart. With `FLOOD_COUNT_DIR`, it says there how much it wrote (`flooded`).
//! - `idle`: nothing, until killed.
//! - `fill`: `FILL_LINES` lines of history (100 000 by default), as fast as the PTY takes them, then nothing: a
//!   pane whose history is full, at rest (the engine compresses it, then must sleep).
//! - `relay`: what a program hands on to the real terminal, once: OSC 52 (clipboard), an OSC 8 link, OSC 9, 777
//!   and 99 notifications, BEL, OSC 9;4 progress, an OSC 2 title; then `RELAY DONE`, and nothing more.
//! - `sync`: `SYNC_ROUNDS` screens, each in one synchronized update (BSU, the pane filled row by row with one
//!   Greek letter, in ten pieces 5 ms apart, ESU), a new letter each round; then `SYNC DONE`.
//! - `keylog`: raw mode; appends each read to `KEYLOG_FILE`, one line per read (microseconds since start, then the
//!   bytes in hex), after a first line `ready <pid>`. With `KEYLOG_MODES=claude`, first asks for what Claude Code 2.1.295 asks for: bracketed paste
//!   (2004), focus events (1004), the kitty keyboard protocol (`KEYLOG_KITTY` flags, 1 by default). With
//!   `KEYLOG_PROBE`, writes 👍🏽 at the start of a line and asks where the cursor is.
//! - `one-of`: the first pane to create the file `ONE_OF_LOCK` runs `echo`, the others `ONE_OF_OTHERS` (`flood` or
//!   `idle`): the multiplexer runs the same command in every pane.
//! - `script`: raw mode; clears the screen, writes the variables named in `SCRIPT_ENV` (comma-separated, one `NAME=value` line each),
//!   then the bytes of `SCRIPT_HEX` (in hex), once; then logs each read to `SCRIPT_LOG`, as `keylog` does (after
//!   `ready <pid>`), and writes nothing more. The screen tests give it the sequences they check.
//! - `claude`: a Claude Code for the team tests, started by the shim of `common::team` (which answers `--version`
//!   and `agents` itself), its own arguments after `--`. It notes its session as `claude agents --json` gives it
//!   (`$CLAUDE_CONFIG_DIR/fake/run/<pid>.json`: `-r <id>`, else a new id), a line in that conversation's file so
//!   that it can be resumed, and its command line (`fake/argv.log`, `<name>\t<arguments>`); shows
//!   `FAKE CLAUDE <name> <session>` and `❯`; then reads lines: `Q` ends it (0), `X` crashes it (1).
//!
//! Owner: testeur.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use super::pty::raw_stdin;

/// Runs the fake `role`, if it is one; never returns then.
pub fn run(role: &str) -> bool {
    match role {
        "echo" => echo(),
        "flood" => flood(),
        "idle" => idle(),
        "relay" => relay(),
        "sync" => sync(),
        "keylog" => keylog(),
        "fill" => fill(),
        "script" => script(),
        "claude" => claude(),
        "one-of" => {
            let lock = std::env::var("ONE_OF_LOCK").expect("ONE_OF_LOCK");
            if std::fs::OpenOptions::new().write(true).create_new(true).open(&lock).is_ok() {
                echo()
            }
            match std::env::var("ONE_OF_OTHERS").as_deref() {
                Ok("flood") => flood(),
                Ok("fill") => fill(),
                _ => idle(),
            }
        }
        _ => false,
    }
}

/// The marker `echo` shows after `n` bytes.
pub fn echo_marker(n: usize) -> String {
    format!("K{n:06}")
}

fn echo() -> ! {
    raw_stdin();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "\x1b[2J\x1b[H{}", echo_marker(0));
    let _ = out.flush();
    let mut stdin = std::io::stdin().lock();
    let mut buf = [0u8; 4096];
    let mut n = 0;
    loop {
        match stdin.read(&mut buf) {
            Ok(0) | Err(_) => std::process::exit(0),
            Ok(read) => {
                n += read;
                let _ = write!(out, "\x1b[H{}", echo_marker(n));
                let _ = out.flush();
            }
        }
    }
}

fn flood() -> ! {
    let cols = super::env_usize("FLOOD_COLS", 120);
    let rate = super::env_usize("FLOOD_RATE", 0);
    let frame = super::env_usize("FLOOD_FRAME", 0);
    let pause = Duration::from_millis(super::env_usize("FLOOD_PAUSE_MS", 16) as u64);
    let mut counter = Counter::new();
    let mut out = std::io::stdout().lock();
    let mut i = 0;
    let mut lines = |chunk: &mut String, bytes: usize| {
        let start = chunk.len();
        while chunk.len() - start < bytes {
            chunk.push_str(&super::history_line(i, cols, false));
            chunk.push_str("\r\n");
            i += 1;
        }
    };
    loop {
        let mut chunk = String::new();
        if frame > 0 {
            // A big synchronized update (`FLOOD_FRAME` bytes), then `FLOOD_PAUSE_MS`.
            chunk.push_str("\x1b[?2026h");
            lines(&mut chunk, frame);
            chunk.push_str("\x1b[?2026l");
        } else if rate > 0 {
            // A slice every 16 ms, of rate × 16 ms bytes, as one synchronized update.
            chunk.push_str("\x1b[?2026h");
            lines(&mut chunk, (rate * 16 / 1000).max(1));
            chunk.push_str("\x1b[?2026l");
        } else {
            lines(&mut chunk, 8 * 1024);
        }
        if out.write_all(chunk.as_bytes()).and_then(|()| out.flush()).is_err() {
            std::process::exit(0);
        }
        counter.add(chunk.len());
        if frame > 0 {
            std::thread::sleep(pause);
        } else if rate > 0 {
            std::thread::sleep(Duration::from_millis(16));
        }
    }
}

/// With `FLOOD_COUNT_DIR`, how many bytes this flood has written so far, in a file of its own (its pid), rewritten
/// every half second: what the terminal above it really took.
struct Counter {
    path: Option<std::path::PathBuf>,
    bytes: u64,
    last: Instant,
}

impl Counter {
    fn new() -> Counter {
        let path =
            std::env::var_os("FLOOD_COUNT_DIR").map(|d| std::path::Path::new(&d).join(std::process::id().to_string()));
        Counter { path, bytes: 0, last: Instant::now() }
    }

    fn add(&mut self, n: usize) {
        self.bytes += n as u64;
        if let Some(path) = &self.path
            && self.last.elapsed() >= Duration::from_millis(500)
        {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, self.bytes.to_string()).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
            self.last = Instant::now();
        }
    }
}

/// What all the floods counting in `dir` have written, in all.
pub fn flooded(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_none())
        .filter_map(|e| std::fs::read_to_string(e.path()).ok()?.trim().parse::<u64>().ok())
        .sum()
}

fn keylog() -> ! {
    let path = std::env::var("KEYLOG_FILE").expect("KEYLOG_FILE");
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("key log");
    // The first line says the fake is up, and which process it is.
    let _ = writeln!(log, "ready {}", std::process::id());
    raw_stdin();
    let mut out = std::io::stdout().lock();
    if std::env::var("KEYLOG_MODES").as_deref() == Ok("claude") {
        let flags = super::env_usize("KEYLOG_KITTY", 1);
        let _ = write!(out, "\x1b[?2004h\x1b[?1004h\x1b[>{flags}u");
    }
    // With `KEYLOG_TITLE`: the window's title, by which the keys test knows its window is the key one.
    if let Ok(title) = std::env::var("KEYLOG_TITLE") {
        let _ = write!(out, "\x1b]2;{title}\x07");
    }
    let _ = write!(out, "\x1b[2J\x1b[HKEYLOG READY\r\n");
    // With `KEYLOG_PROBE`: an emoji with a skin tone, then a cursor position request; the answer comes in as the
    // first bytes logged. Its column says how many cells the terminal (or the engine above it) gave the emoji.
    if std::env::var_os("KEYLOG_PROBE").is_some() {
        let _ = write!(out, "\r\n\u{1f44d}\u{1f3fd}\x1b[6n");
    }
    let _ = out.flush();
    let start = Instant::now();
    let mut stdin = std::io::stdin().lock();
    let mut buf = [0u8; 65536];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) | Err(_) => std::process::exit(0),
            Ok(n) => {
                let hex: String = buf[..n].iter().map(|b| format!("{b:02x}")).collect();
                let _ = writeln!(log, "{} {hex}", start.elapsed().as_micros());
                let _ = write!(out, "{n} ");
                let _ = out.flush();
            }
        }
    }
}

/// What `relay` puts in the clipboard (OSC 52, base64 of "recruit-osc52").
pub const OSC52: &str = "\x1b]52;c;cmVjcnVpdC1vc2M1Mg==\x07";
pub const LINK_URL: &str = "https://example.com/recruit-osc8";

fn relay() -> ! {
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "{OSC52}");
    let _ = write!(out, "\x1b]8;;{LINK_URL}\x1b\\le lien\x1b]8;;\x1b\\\r\n");
    let _ = write!(out, "\x1b]9;recruit-osc9\x07");
    let _ = write!(out, "\x1b]777;notify;recruit;osc777\x07");
    let _ = write!(out, "\x1b]99;;recruit-osc99\x1b\\");
    let _ = write!(out, "\x07");
    let _ = write!(out, "\x1b]9;4;1;42\x07");
    let _ = write!(out, "\x1b]2;recruit-title\x07");
    let _ = write!(out, "RELAY DONE\r\n");
    let _ = out.flush();
    idle()
}

/// The letters `sync` fills its rounds with: none of them can be in a pane's title.
pub const SYNC_LETTERS: &str = "αβγδεζηθικλμνξοπρστυ";

fn sync() -> ! {
    let mut size = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: a size read from stdout, the pane's terminal.
    unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &raw mut size) };
    let (rows, cols) = (size.ws_row.max(1) as usize, size.ws_col.max(1) as usize);
    let rounds = super::env_usize("SYNC_ROUNDS", 20);
    let mut out = std::io::stdout().lock();
    std::thread::sleep(Duration::from_millis(500));
    for letter in SYNC_LETTERS.chars().cycle().take(rounds) {
        let line: String = std::iter::repeat_n(letter, cols).collect();
        let _ = write!(out, "\x1b[?2026h");
        for row in 0..rows {
            let _ = write!(out, "\x1b[{};1H{line}", row + 1);
            if (row + 1) % (rows / 10).max(1) == 0 {
                let _ = out.flush();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let _ = write!(out, "\x1b[?2026l");
        let _ = out.flush();
        std::thread::sleep(Duration::from_millis(30));
    }
    let _ = write!(out, "\x1b[H\x1b[2JSYNC DONE");
    let _ = out.flush();
    idle()
}

fn fill() -> ! {
    let cols = super::env_usize("FLOOD_COLS", 120);
    let lines = super::env_usize("FILL_LINES", 100_000);
    let mut out = std::io::stdout().lock();
    let mut chunk = String::new();
    for i in 0..lines {
        chunk.push_str(&super::history_line(i, cols, false));
        chunk.push_str("\r\n");
        if chunk.len() > 64 * 1024 {
            let _ = out.write_all(chunk.as_bytes());
            chunk.clear();
        }
    }
    let _ = out.write_all(chunk.as_bytes());
    let _ = out.flush();
    drop(out);
    idle()
}

fn script() -> ! {
    let mut log = std::env::var_os("SCRIPT_LOG")
        .map(|path| std::fs::OpenOptions::new().create(true).append(true).open(path).expect("script log"));
    if let Some(log) = &mut log {
        let _ = writeln!(log, "ready {}", std::process::id());
    }
    raw_stdin();
    let mut out = std::io::stdout().lock();
    // A clean screen: the bytes start at the top left, below nothing of libtest's.
    let _ = write!(out, "\x1b[2J\x1b[H");
    for name in std::env::var("SCRIPT_ENV").unwrap_or_default().split(',').filter(|n| !n.is_empty()) {
        let value = std::env::var(name).unwrap_or_else(|_| "(unset)".into());
        let _ = write!(out, "{name}={value}\r\n");
    }
    let hex = std::env::var("SCRIPT_HEX").unwrap_or_default();
    let bytes: Vec<u8> =
        (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect();
    let _ = out.write_all(&bytes);
    let _ = out.flush();
    drop(out);
    let start = Instant::now();
    let mut stdin = std::io::stdin().lock();
    let mut buf = [0u8; 65536];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) | Err(_) => std::process::exit(0),
            Ok(n) => {
                if let Some(log) = &mut log {
                    let hex: String = buf[..n].iter().map(|b| format!("{b:02x}")).collect();
                    let _ = writeln!(log, "{} {hex}", start.elapsed().as_micros());
                }
            }
        }
    }
}

fn claude() -> ! {
    let args: Vec<String> = std::env::args().skip_while(|a| a != "--").skip(1).collect();
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let name = after("-n").unwrap_or_default();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let session = after("-r").unwrap_or_else(|| format!("fake-{pid}-{nanos}"));
    let profile = std::path::PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").expect("CLAUDE_CONFIG_DIR"));
    let fake = profile.join("fake");
    let cwd = std::env::current_dir().and_then(|d| d.canonicalize()).expect("cwd");
    let _ = std::fs::create_dir_all(fake.join("run"));
    if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(fake.join("argv.log")) {
        // One write per line: the fakes of a team start together, appending to the same file.
        let _ = log.write_all(format!("{name}\t{}\n", args.join(" ")).as_bytes());
    }
    let running = serde_json::json!({
        "name": name, "cwd": cwd, "status": "idle", "sessionId": session, "pid": pid,
    });
    let run = fake.join("run").join(format!("{pid}.json"));
    let _ = std::fs::write(&run, running.to_string());
    // The conversation's file, where recruit looks for it (`claude::transcript`): a session that said nothing is
    // not resumed.
    let key: String = cwd
        .to_string_lossy()
        .chars()
        .flat_map(|c| if c.is_ascii_alphanumeric() { vec![c] } else { vec!['-'; c.len_utf16()] })
        .collect();
    let conversation = profile.join("projects").join(key);
    let _ = std::fs::create_dir_all(&conversation);
    if let Ok(mut file) =
        std::fs::OpenOptions::new().create(true).append(true).open(conversation.join(format!("{session}.jsonl")))
    {
        let _ = writeln!(file, "{{\"type\":\"fake\",\"pid\":{pid}}}");
        // The title Claude Code gives a conversation started with `-n <name>`: `claude::last_conversations` reads it.
        let _ = writeln!(file, "{{\"type\":\"custom-title\",\"customTitle\":\"{name}\",\"sessionId\":\"{session}\"}}");
    }
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "\x1b[2J\x1b[HFAKE CLAUDE {name} {session}\r\n\u{276f} ");
    let _ = out.flush();
    drop(out);
    no_echo();
    let mut asked = false;
    let code = loop {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => break 0,
            Ok(_) => match line.rsplit("\x1b\\").next().unwrap_or_default().trim() {
                "Q" => break 0,
                "X" => break 1,
                // `osc working`, `osc blocked:kind=permission`…: what Claude Code says of its state (OSC 7501).
                command if command.starts_with("osc ") => {
                    let mut out = std::io::stdout().lock();
                    // The terminal must have said it takes statuses first (`OSC 7501 ; ?`, answered at once): the
                    // engine ignores them before. Its answer reaches the program's input; `line` drops it.
                    if !asked {
                        asked = true;
                        let _ = out.write_all(b"\x1b]7501;?\x1b\\");
                        let _ = out.flush();
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    let _ = write!(out, "\x1b]7501;state={}\x07", &command[4..]);
                    let _ = out.flush();
                }
                // `msg <to> <text>`: the conversation records a message to a teammate (a `SendMessage` of the model), for
                // the journal.
                command if command.starts_with("msg ") => {
                    let (to, text) = command[4..].split_once(' ').unwrap_or((&command[4..], "hello"));
                    let stamp =
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    let line = serde_json::json!({
                        "type": "assistant", "timestamp": iso(stamp),
                        "message": {"content": [{"type": "tool_use", "name": "SendMessage",
                            "input": {"to": to, "summary": text, "message": text}}]},
                    });
                    if let Ok(mut file) =
                        std::fs::OpenOptions::new().append(true).open(conversation.join(format!("{session}.jsonl")))
                    {
                        let _ = writeln!(file, "{line}");
                    }
                }
                // `agent busy`, `agent idle`, `agent waiting`: what `claude agents` says of it.
                command if command.starts_with("agent ") => {
                    let running = serde_json::json!({
                        "name": name, "cwd": cwd, "status": &command[6..], "sessionId": session, "pid": pid,
                    });
                    let _ = std::fs::write(&run, running.to_string());
                }
                _ => {}
            },
        }
    };
    let _ = std::fs::remove_file(&run);
    std::process::exit(code)
}

/// `2026-10-07T10:28:51.000Z` for seconds since the epoch.
fn iso(secs: u64) -> String {
    let (days, rest) = (secs / 86400, secs % 86400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// Nothing written back for what is typed or answered: the answer to `OSC 7501 ; ?` would show as `^[]7501;?^[\`.
fn no_echo() {
    // SAFETY: plain termios calls on the program's own terminal; a failure (no terminal) leaves it as it is.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut termios) == 0 {
            termios.c_lflag &= !(libc::ECHO | libc::ECHOCTL);
            libc::tcsetattr(0, libc::TCSANOW, &termios);
        }
    }
}

fn idle() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
