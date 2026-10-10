// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The native multiplexer's server and client (step 1; grid `specs/multiplexeur-compat.md`, lines Sv): a server of
//! the test's own (`common::server`), fake programs in its panes, real clients on test terminals. Part of
//! `cargo test`, and so of the CI; `ssh_attach` and `attach_existing` are ignored: by hand, on a team already
//! running.
//!
//! Owner: testeur.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use common::fakes;
use common::server::{NOT_RUNNING, Server, UNKNOWN_PANE, leftovers};

#[test]
#[ignore = "started by the other tests only"]
fn child() {
    match common::role().as_deref() {
        Some(role) if common::fakes::run(role) => {}
        Some(other) => panic!("unknown child role {other}"),
        None => {}
    }
}

/// What a client's terminal shows, and the end of what it was sent, for a failed check.
fn shown(term: &common::pty::TestTerm) -> String {
    let raw = term.raw();
    let tail = String::from_utf8_lossy(&raw[raw.len().saturating_sub(400)..]).escape_debug().to_string();
    format!("screen: {:#?}\nend of output: {tail}", term.screen())
}

/// A pane opened by `_ctl spawn` runs its program; `send` reaches it; `capture` shows it; an unknown pane is code
/// 3; `stop` leaves nothing behind.
#[test]
fn spawn_send_capture_stop() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut server = Server::start(dir.path());
    server.spawn_fake("alpha", "echo", &[]);
    assert!(
        server.wait_pane("alpha", &fakes::echo_marker(0), Duration::from_secs(10)),
        "{:?}",
        server.capture(Some("alpha"))
    );
    server.ctl(["send", "--pane", "alpha", "x"]);
    assert!(server.wait_pane("alpha", &fakes::echo_marker(1), Duration::from_secs(5)));
    let unknown = server.ctl_output(["capture", "--pane", "nobody"]);
    assert_eq!(unknown.status.code(), Some(UNKNOWN_PANE), "{}", String::from_utf8_lossy(&unknown.stderr));
    server.stop();
    let gone = server.ctl_output(["where"]);
    assert_eq!(gone.status.code(), Some(NOT_RUNNING));
}

/// Sv7: the socket's folder is the user's alone (0700), and its path fits in `sun_path` (104 bytes on macOS).
#[test]
fn socket_is_private_and_short() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = Server::start(dir.path());
    let socket = server.where_json()["socket"].as_str().expect("where: socket").to_string();
    assert!(socket.len() < 104, "socket path of {} bytes: {socket}", socket.len());
    let folder = std::path::Path::new(&socket).parent().expect("folder");
    let mode = std::fs::metadata(folder).expect("folder").permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "{} is {mode:o}", folder.display());
}

/// Sv1, Sv2: a client attached, then gone (its terminal closed: the client killed); the panes go on meanwhile;
/// a new client shows what they did.
#[test]
fn client_gone_then_back() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = Server::start(dir.path());
    server.spawn_fake("alpha", "echo", &[]);
    let mut first = server.attach(120, 40);
    assert!(first.wait_for(&fakes::echo_marker(0), Duration::from_secs(10)), "{}", shown(&first));
    assert_eq!(server.clients()["clients"].as_array().map(Vec::len), Some(1), "{}", server.clients());
    // The terminal closed: SIGHUP to the client.
    // SAFETY: a plain signal to the client this test started.
    unsafe { libc::kill(first.pid() as libc::pid_t, libc::SIGHUP) };
    assert!(first.wait_exit(Duration::from_secs(5)).is_some(), "the client outlived its terminal");
    // Meanwhile the pane goes on.
    server.ctl(["send", "--pane", "alpha", "xyz"]);
    assert!(server.wait_pane("alpha", &fakes::echo_marker(3), Duration::from_secs(5)));
    assert_eq!(server.clients()["clients"].as_array().map(Vec::len), Some(0), "{}", server.clients());
    let second = server.attach(120, 40);
    assert!(second.wait_for(&fakes::echo_marker(3), Duration::from_secs(10)), "{}", shown(&second));
}

/// Sv3: a second attach takes over: the first client ends (with a message), the second gets the screen, at its
/// own size.
#[test]
fn second_attach_takes_over() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = Server::start(dir.path());
    server.spawn_fake("alpha", "echo", &[]);
    let mut first = server.attach(100, 30);
    assert!(first.wait_for(&fakes::echo_marker(0), Duration::from_secs(10)), "{}", shown(&first));
    let second = server.attach(140, 45);
    assert!(second.wait_for(&fakes::echo_marker(0), Duration::from_secs(10)), "{}", shown(&second));
    let ended = first.wait_exit(Duration::from_secs(5));
    assert!(ended.is_some(), "the first client is still attached");
    let raw = String::from_utf8_lossy(&first.raw()).into_owned();
    println!(
        "Ce que voit le premier client à la fin : {:?}",
        raw.chars().rev().take(200).collect::<String>().chars().rev().collect::<String>()
    );
    let clients = server.clients();
    let list = clients["clients"].as_array().expect("clients");
    assert_eq!(list.len(), 1, "{clients}");
    assert_eq!((list[0]["cols"].as_u64(), list[0]["rows"].as_u64()), (Some(140), Some(45)), "{clients}");
}

/// Sv5: a client killed outright (SIGKILL) changes nothing for the server.
#[test]
fn client_killed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = Server::start(dir.path());
    server.spawn_fake("alpha", "echo", &[]);
    let mut client = server.attach(120, 40);
    assert!(client.wait_for(&fakes::echo_marker(0), Duration::from_secs(10)), "{}", shown(&client));
    // SAFETY: a plain signal to the client this test started.
    unsafe { libc::kill(client.pid() as libc::pid_t, libc::SIGKILL) };
    let _ = client.wait_exit(Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(500));
    server.ctl(["send", "--pane", "alpha", "x"]);
    assert!(server.wait_pane("alpha", &fakes::echo_marker(1), Duration::from_secs(5)));
    assert_eq!(server.clients()["clients"].as_array().map(Vec::len), Some(0));
}

/// Sv4: the server killed outright (SIGKILL): its panes' programs end too (no orphan), and `_ctl` says the team is
/// not running (code 2), the dead socket notwithstanding.
#[test]
fn server_killed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = Server::start(dir.path());
    server.spawn_fake("alpha", "echo", &[]);
    server.spawn_fake("beta", "flood", &[("FLOOD_RATE", "20000".into())]);
    assert!(server.wait_pane("alpha", &fakes::echo_marker(0), Duration::from_secs(10)));
    let pid = server.pid();
    // SAFETY: a plain signal to the server this test started.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut left = leftovers(dir.path());
    while !left.is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        left = leftovers(dir.path());
    }
    assert!(left.is_empty(), "left after the server's death: {left:?}");
    let gone = server.ctl_output(["where"]);
    assert_eq!(gone.status.code(), Some(NOT_RUNNING), "{}", String::from_utf8_lossy(&gone.stderr));
    // Nothing to stop any more.
    server.gone();
}

/// What a program wrote stays on its pane's screen when the pane is resized (a client attaching at another
/// size): a program that does not redraw on SIGWINCH (a finished command's output, a log) must not lose it.
#[test]
fn resize_keeps_the_screen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = Server::start(dir.path());
    server.spawn_fake("alpha", "echo", &[]);
    assert!(server.wait_pane("alpha", &fakes::echo_marker(0), Duration::from_secs(10)));
    // A plain log too: three lines, then nothing.
    server.ctl(["spawn", "beta", "--", "sh", "-c", "printf 'ligne-1\\nligne-2\\nligne-3\\n'; sleep 60"]);
    assert!(server.wait_pane("beta", "ligne-3", Duration::from_secs(10)));
    let before = server.capture(Some("alpha"));
    let client = server.attach(100, 30);
    std::thread::sleep(Duration::from_secs(1));
    let after = server.capture(Some("alpha"));
    let log = server.capture(Some("beta"));
    println!("beta after the resize: {:?}", &log[..4.min(log.len())]);
    let log_kept = log.iter().any(|r| r.contains("ligne-1")) && log.iter().any(|r| r.contains("ligne-3"));
    let kept = after.iter().any(|r| r.contains(&fakes::echo_marker(0)));
    println!("alpha kept: {kept}; beta kept: {log_kept}");
    assert!(log_kept, "the log lost on resize: {log:?}");
    assert!(kept, "lost on resize; before: {:?}\nafter: {:?}\nclient: {}", &before[..3], &after[..3], shown(&client));
}

/// A look at a team already running (a real one, launched by hand for the grid): a client attached for a few
/// seconds on a test terminal of `ATTACH_SIZE` (`200x50`), its screen printed, then detached (SIGTERM). The team
/// is named by `ATTACH_STATE` (its state folder), with `ATTACH_BIN` (the copy of recruit that runs it) and
/// `RECRUIT_TMPDIR`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` as it was launched with.
#[test]
#[ignore = "by hand, on a team already running"]
fn attach_existing() {
    let (Some(state), Some(bin)) = (std::env::var_os("ATTACH_STATE"), std::env::var_os("ATTACH_BIN")) else {
        println!("ATTACH_STATE and ATTACH_BIN not set: nothing to do");
        return;
    };
    let size = std::env::var("ATTACH_SIZE").unwrap_or_else(|_| "200x50".into());
    let (cols, rows) =
        size.split_once('x').map(|(c, r)| (c.parse().unwrap_or(200), r.parse().unwrap_or(50))).unwrap_or((200, 50));
    let args: [&std::ffi::OsStr; 3] = ["_ctl".as_ref(), state.as_os_str(), "attach".as_ref()];
    let profile = common::pty::Profile { capture: true, ..Default::default() };
    let mut term = common::pty::TestTerm::spawn(&bin, args, &[], cols, rows, profile);
    std::thread::sleep(Duration::from_secs(common::env_usize("ATTACH_SECS", 4) as u64));
    for row in term.screen() {
        println!("{row}");
    }
    // SAFETY: a plain signal to the client this test started.
    unsafe { libc::kill(term.pid() as libc::pid_t, libc::SIGTERM) };
    let status = term.wait_exit(Duration::from_secs(5));
    println!("client ended: {status:?}");
}

/// A key of `client_keys_reach_pane`: its name, what a kitty terminal sends, what a legacy one sends, what the pane
/// must get from each (`None`: nothing).
type Key = (&'static str, &'static [u8], &'static [u8], Option<&'static [u8]>, Option<&'static [u8]>);

/// Keys through the real client, without a window (grid K, K15): a test terminal that speaks the kitty protocol
/// types into a client attached to a native server; the `keylog` fake in the pane logs what it gets. Each key must
/// reach the pane as a legacy terminal would send it (the fake asks for no mode); the server's own shortcuts
/// (⌥1, ⌥⇧→) must not reach it.
#[test]
fn client_keys_reach_pane() {
    keys_reach_pane(common::pty::Profile::default());
}

/// The same from a terminal without the kitty protocol (Terminal.app): the legacy encodings.
#[test]
fn client_keys_reach_pane_legacy() {
    keys_reach_pane(common::pty::Profile { kitty_keyboard: false, ..Default::default() });
}

fn keys_reach_pane(profile: common::pty::Profile) {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = Server::start(dir.path());
    let log = dir.path().join("keys.log");
    server.spawn_fake("alpha", "keylog", &[("KEYLOG_FILE", log.to_string_lossy().into_owned())]);
    assert!(server.wait_pane("alpha", "KEYLOG READY", Duration::from_secs(10)));
    let mut client = server.attach_as(120, 40, profile);
    assert!(client.wait_for("KEYLOG READY", Duration::from_secs(10)), "{}", shown(&client));
    std::thread::sleep(Duration::from_millis(500));
    let kitty = client.kitty_keyboard();
    println!("The client pushed the kitty protocol to its terminal: {kitty}");
    let keys: [Key; 11] = [
        ("x", b"x", b"x", Some(b"x"), Some(b"x")),
        ("é", "é".as_bytes(), "é".as_bytes(), Some("é".as_bytes()), Some("é".as_bytes())),
        // A legacy terminal cannot tell Shift+Enter: Terminal.app sends ESC CR, passed on as such (Claude Code
        // takes it for a new line too); from a kitty terminal, the multiplexer sends LF.
        ("Shift+Entrée", b"\x1b[13;2u", b"\x1b\r", Some(b"\n"), Some(b"\x1b\r")),
        ("Ctrl+B", b"\x1b[98;5u", b"\x02", Some(b"\x02"), Some(b"\x02")),
        ("Ctrl+C", b"\x1b[99;5u", b"\x03", Some(b"\x03"), Some(b"\x03")),
        ("Échap", b"\x1b[27u", b"\x1b", Some(b"\x1b"), Some(b"\x1b")),
        ("⌥b", b"\x1b[98;3u", b"\x1bb", Some(b"\x1bb"), Some(b"\x1bb")),
        ("↑", b"\x1b[A", b"\x1b[A", Some(b"\x1b[A"), Some(b"\x1b[A")),
        ("Shift+Tab", b"\x1b[9;2u", b"\x1b[Z", Some(b"\x1b[Z"), Some(b"\x1b[Z")),
        ("⌥1", b"\x1b[49;3u", b"\x1b1", None, None),
        ("⌥⇧→", b"\x1b[1;4C", b"\x1b[1;4C", None, None),
    ];
    let read = |from: usize| -> (Vec<u8>, usize) {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let mut bytes = Vec::new();
        for line in lines[from.min(lines.len())..].iter().filter(|l| !l.starts_with("ready ")) {
            if let Some(hex) = line.split_whitespace().nth(1) {
                bytes.extend((0..hex.len()).step_by(2).filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()));
            }
        }
        (bytes, lines.len())
    };
    // What the pane got since `seen`, once something came, 3 s at most (no fixed pause: a busy runner is slow).
    let wait_new = |seen: usize| -> (Vec<u8>, usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let (got, _) = read(seen);
            if !got.is_empty() || std::time::Instant::now() >= deadline {
                // A key may come in two reads: a moment for the rest.
                std::thread::sleep(Duration::from_millis(100));
                return read(seen);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let (_, mut seen) = read(0);
    let mut bad = Vec::new();
    for (name, as_kitty, as_legacy, want_kitty, want_legacy) in keys {
        let want = if kitty { want_kitty } else { want_legacy };
        client.send(if kitty { as_kitty } else { as_legacy });
        if want.is_none() {
            // A key that must not reach the pane: a witness behind it, which alone must arrive.
            client.send(b"z");
        }
        let (got, len) = wait_new(seen);
        seen = len;
        let ok = match want {
            Some(want) => got == want,
            None => got == b"z",
        };
        println!("| {name} | {:?} | {} |", String::from_utf8_lossy(&got), if ok { "ok" } else { "ko" });
        if !ok {
            bad.push(name);
        }
    }
    assert!(bad.is_empty(), "keys that went wrong: {bad:?}");
}

/// By SSH (grid Sv9, P3), on a team already launched on another machine (by hand, see the grid): a client attached
/// through `ssh -tt` on a test terminal; a pane added there that writes OSC 52, which must reach this terminal;
/// the SSH session cut (its process killed), the server still running; a second attach. `SSH_HOST`, `SSH_ROOT` (the
/// remote temporary folder: the copy of recruit, `config`, `cache`, `r` for `RECRUIT_TMPDIR`, `proj`), `SSH_TEAM`.
#[test]
#[ignore = "by hand, on a team launched on another machine"]
fn ssh_attach() {
    let (Ok(host), Ok(root)) = (std::env::var("SSH_HOST"), std::env::var("SSH_ROOT")) else {
        println!("SSH_HOST and SSH_ROOT not set: nothing to do");
        return;
    };
    let team = std::env::var("SSH_TEAM").unwrap_or_else(|_| "nat".into());
    let env = format!(
        "cd {root}/proj && env XDG_CONFIG_HOME={root}/config XDG_CACHE_HOME={root}/cache RECRUIT_TMPDIR={root}/r"
    );
    let remote = |command: &str| -> String {
        let out = std::process::Command::new("ssh")
            .args(["-o", "BatchMode=yes", &host, &format!("{env} {command}")])
            .output()
            .expect("ssh");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    };
    let attach = || {
        let args = [
            "-tt".to_string(),
            "-o".into(),
            "BatchMode=yes".into(),
            host.clone(),
            format!("{env} {root}/recruit attach {team}"),
        ];
        let profile = common::pty::Profile { capture: true, ..Default::default() };
        common::pty::TestTerm::spawn(std::ffi::OsStr::new("ssh"), &args, &[], 200, 50, profile)
    };
    let mut first = attach();
    let up = first.wait_for("chef", Duration::from_secs(15));
    println!("first attach: up {up}");
    for row in first.screen().iter().take(6) {
        println!("  {row}");
    }
    // OSC 52 from a pane over there, to this terminal.
    let state = format!("{root}/cache/recruit/teams/{team}-dry-run");
    println!(
        "{}",
        remote(&format!(
            "{root}/recruit _ctl {state} spawn osc -- sh -c \"printf '\\033]52;c;cmVjcnVpdC1vc2M1Mg==\\007'; sleep 60\""
        ))
    );
    std::thread::sleep(Duration::from_secs(2));
    let raw = String::from_utf8_lossy(&first.raw()).into_owned();
    println!("OSC 52 through SSH: {}", raw.contains("\x1b]52;c;cmVjcnVpdC1vc2M1Mg=="));
    // The SSH session cut: the ssh process killed outright.
    // SAFETY: a plain signal to the ssh this test started.
    unsafe { libc::kill(first.pid() as libc::pid_t, libc::SIGKILL) };
    let _ = first.wait_exit(Duration::from_secs(5));
    std::thread::sleep(Duration::from_secs(2));
    println!("after the cut, where: {}", remote(&format!("{root}/recruit _ctl {state} where")).trim());
    println!("after the cut, clients: {}", remote(&format!("{root}/recruit _ctl {state} clients --json")).trim());
    let mut second = attach();
    let back = second.wait_for("chef", Duration::from_secs(15));
    println!("second attach: up {back}");
    // SAFETY: a plain signal to the ssh this test started.
    unsafe { libc::kill(second.pid() as libc::pid_t, libc::SIGTERM) };
    let _ = second.wait_exit(Duration::from_secs(5));
}
