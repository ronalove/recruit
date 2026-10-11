// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The compatibility grid with a real Claude Code (spec §6; `specs/multiplexeur-compat.md`), on the test's own
//! terminal (`common::pty`): no window, nothing typed into the user's screen. Real sessions, short, with nothing
//! asked of the model: they only start, take keys and quit. `<profile>/settings.json` is compared before and after.
//!
//! ```sh
//! CLAUDE_SESSIONS=$(date +%F) cargo test --release --test mux_claude -- --ignored --nocapture --test-threads=1 claude_modes
//! ```
//!
//! Nothing runs without `CLAUDE_SESSIONS` set to today's date and the test named on the command line: real
//! sessions use the user's quota, and a whole `cargo test -- --ignored` must not start them.
//!
//! - `claude_modes`: Claude alone, for each `TERM_PROGRAM` the grid plays, default and full screen: what it asks of
//!   its terminal (kitty keyboard flags, synchronized updates, mouse, focus, bracketed paste…).
//! - `claude_through_mux`: two Claude in a native server of the test's own, a real client attached, for each
//!   announced terminal: the screen is clean, keys reach the focused pane, resizing, focus, stopping.
//!
//! The sessions run in `$TMPDIR/recruit-testeur-claude`, a folder of the tester's own: Claude Code asks once
//! whether to trust it, and the test answers yes.
//!
//! Owner: testeur.

mod common;

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::time::Duration;

use common::pty::{Profile, TestTerm};

#[test]
#[ignore = "started by the other tests only"]
fn child() {
    match common::role().as_deref() {
        Some(role) if common::fakes::run(role) => {}
        Some(other) => panic!("unknown child role {other}"),
        None => {}
    }
}

/// The sessions' name: a named session opens on the conversation, as a member's does; an unnamed one may open on
/// the agent view (2.1.295).
const SESSION: &str = "testeur-compat";

/// Where the sessions run.
fn workdir() -> PathBuf {
    let dir = std::env::temp_dir().join("recruit-testeur-claude");
    std::fs::create_dir_all(&dir).expect("work dir");
    dir
}

/// Claude Code's user settings, which no test may change.
fn settings() -> (PathBuf, Option<Vec<u8>>) {
    let profile = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".claude"));
    let path = profile.join("settings.json");
    let bytes = std::fs::read(&path).ok();
    (path, bytes)
}

/// Fails if the settings changed since `before`.
fn check_settings(before: &(PathBuf, Option<Vec<u8>>)) {
    let after = settings();
    assert!(after.1 == before.1, "{} changed during the test", before.0.display());
    println!("{} inchangé.", before.0.display());
}

/// A terminal the grid announces to Claude Code: `TERM_PROGRAM`, its version, `TERM`.
struct Announced {
    name: &'static str,
    program: Option<&'static str>,
    version: Option<&'static str>,
    term: &'static str,
}

const ANNOUNCED: &[Announced] = &[
    Announced {
        name: "recruit",
        program: Some("recruit"),
        version: Some(env!("CARGO_PKG_VERSION")),
        term: "xterm-256color",
    },
    Announced { name: "aucun", program: None, version: None, term: "xterm-256color" },
    Announced { name: "ghostty", program: Some("ghostty"), version: None, term: "xterm-256color" },
    Announced { name: "ghostty 1.2.3", program: Some("ghostty"), version: Some("1.2.3"), term: "xterm-256color" },
    Announced { name: "tmux", program: Some("tmux"), version: Some("3.8"), term: "tmux-256color" },
];

/// Waits for Claude's prompt on `term` (`❯`), answering the folder-trust question if it comes, then lets the
/// first frames settle. True if it came.
///
/// Each key of the answer is chosen from a screen that has been still for 500 ms, never sent blind: at startup
/// Claude Code reads the first keys itself (`startCapturingEarlyInput`, 2.1.295) to fill its prompt, an Enter there
/// turns into a space and arrows are dropped, and that reader races Ink's for about 0.1 s after the question shows
/// (dev-terminal, under tmux and natively). A down arrow sent again after a lost Enter went round to « No, exit ».
fn wait_prompt(term: &mut TestTerm) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let screen = settled(term, Duration::from_millis(500), deadline);
        if screen.iter().any(|r| r.contains("trust this folder")) {
            let on_yes = screen.iter().any(|r| r.contains('❯') && r.contains("Yes, I trust"));
            if on_yes {
                term.send(b"\r");
                // Gone, or the Enter was lost and the next still screen says so.
                let _ = wait_gone(term, "trust this folder", Duration::from_secs(3));
            } else {
                // The choice shown is not « Yes, I trust »: one step down, then what the screen shows decides again.
                term.send(b"\x1b[B");
                let _ = term.wait_for("❯ Yes, I trust", Duration::from_secs(2));
            }
            continue;
        }
        if screen.iter().any(|r| r.contains('❯') && !r.contains("No, exit")) {
            std::thread::sleep(Duration::from_secs(3));
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// The screen of `term` once it has not changed for `quiet` (or at `deadline`).
fn settled(term: &TestTerm, quiet: Duration, deadline: std::time::Instant) -> Vec<String> {
    let mut last = term.screen();
    let mut since = std::time::Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let now = term.screen();
        if now != last {
            last = now;
            since = std::time::Instant::now();
        } else if since.elapsed() >= quiet || std::time::Instant::now() >= deadline {
            return last;
        }
    }
}

/// Waits until `text` is no longer on the screen of `term`, `timeout` at most; true if it went.
fn wait_gone(term: &TestTerm, text: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if !term.screen().iter().any(|r| r.contains(text)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Counts of the sequences that say what a program asks of its terminal.
fn modes(raw: &[u8]) -> Vec<(&'static str, String)> {
    let text = String::from_utf8_lossy(raw);
    let count = |needle: &str| text.matches(needle).count();
    let kitty: Vec<String> = text
        .match_indices("\x1b[>")
        .filter_map(|(i, _)| {
            let rest = &text[i + 3..];
            let end = rest.find(|c: char| !c.is_ascii_digit())?;
            (rest[end..].starts_with('u') && end > 0).then(|| rest[..end].to_string())
        })
        .collect();
    vec![
        ("kitty poussé (drapeaux)", if kitty.is_empty() { "non".into() } else { kitty.join(",") }),
        ("CSI ? u (question kitty)", count("\x1b[?u").to_string()),
        ("XTVERSION demandé", count("\x1b[>0q").to_string()),
        ("DECRQM 2026 demandé", count("\x1b[?2026$p").to_string()),
        ("BSU (images synchronisées)", count("\x1b[?2026h").to_string()),
        ("écran alterné 1049/1047", format!("{}/{}", count("\x1b[?1049h"), count("\x1b[?1047h"))),
        (
            "souris 1000/1002/1003/1006",
            format!(
                "{}/{}/{}/{}",
                count("\x1b[?1000h"),
                count("\x1b[?1002h"),
                count("\x1b[?1003h"),
                count("\x1b[?1006h")
            ),
        ),
        ("collage encadré 2004", count("\x1b[?2004h").to_string()),
        ("focus 1004", count("\x1b[?1004h").to_string()),
        ("2031 (thème)", count("\x1b[?2031h").to_string()),
        ("modifyOtherKeys", count("\x1b[>4;").to_string()),
        ("OSC 7501 (état)", count("\x1b]7501").to_string()),
        ("OSC 11 demandé", count("\x1b]11;?").to_string()),
        ("DA1 demandé", count("\x1b[c").to_string()),
    ]
}

/// Claude Code's environment for `announced`, in full screen or not.
fn claude_env(announced: &Announced, fullscreen: bool) -> Vec<(&'static str, OsString)> {
    let mut env = vec![("TERM", OsString::from(announced.term))];
    if let Some(program) = announced.program {
        env.push(("TERM_PROGRAM", program.into()));
    }
    if let Some(version) = announced.version {
        env.push(("TERM_PROGRAM_VERSION", version.into()));
    }
    env.push(display(fullscreen));
    env
}

/// The variable that picks Claude Code's renderer for this session only, whatever the user's `tui` setting says
/// (`/tui` itself would save the choice for every later session).
fn display(fullscreen: bool) -> (&'static str, OsString) {
    if fullscreen {
        ("CLAUDE_CODE_NO_FLICKER", "1".into())
    } else {
        ("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN", "1".into())
    }
}

/// What Claude Code asks of its terminal, for each announced terminal, in its default and full-screen modes.
#[test]
#[ignore = "real Claude Code sessions; see the module's doc"]
fn claude_modes() {
    if !common::consented("CLAUDE_SESSIONS", "real Claude Code sessions", "claude_modes") {
        return;
    }
    let before = settings();
    let dir = workdir();
    let mut columns: Vec<(String, Vec<(&'static str, String)>)> = Vec::new();
    let only = std::env::var("MODES_ONLY").ok();
    for announced in ANNOUNCED.iter().filter(|a| only.as_deref().is_none_or(|o| a.name == o)) {
        for fullscreen in [false, true] {
            let env = claude_env(announced, fullscreen);
            let env: Vec<(&str, &OsStr)> = env.iter().map(|(k, v)| (*k, v.as_os_str())).collect();
            let profile = Profile { kitty_keyboard: true, capture: true, xtversion: None, light: false };
            let mut term = TestTerm::spawn_in(
                Some(&dir),
                OsStr::new("claude"),
                ["--strict-mcp-config", "-n", SESSION],
                &env,
                120,
                40,
                profile,
            );
            let up = wait_prompt(&mut term);
            let name = format!("{}{}", announced.name, if fullscreen { ", plein écran" } else { "" });
            let mut found = modes(&term.raw());
            found.push(("invite affichée", if up { "oui" } else { "NON" }.into()));
            if !up || std::env::var_os("MUX_DEBUG").is_some() {
                println!("{name} : écran : {:#?}", term.screen());
            }
            columns.push((name, found));
            // Ctrl+C twice on an empty prompt quits.
            let ctrl_c: &[u8] = if term.kitty_keyboard() { b"\x1b[99;5u" } else { b"\x03" };
            term.send(ctrl_c);
            std::thread::sleep(Duration::from_millis(300));
            term.send(ctrl_c);
            let _ = term.wait_exit(Duration::from_secs(5));
        }
    }
    println!();
    print!("| Demande |");
    for (name, _) in &columns {
        print!(" {name} |");
    }
    println!();
    println!("|---|{}", "---|".repeat(columns.len()));
    for row in 0..columns[0].1.len() {
        print!("| {} |", columns[0].1[row].0);
        for (_, found) in &columns {
            print!(" {} |", found[row].1);
        }
        println!();
    }
    check_settings(&before);
}

/// One check through the multiplexer, and its result.
type Check = (&'static str, Result<(), String>);

/// The screen holds no escape sequence shown as text: what a terminal that missed a sequence leaves behind.
fn clean(screen: &[String]) -> Result<(), String> {
    for row in screen {
        for bad in ["\u{1b}", "[?", "]8;", "\u{fffd}", "2026$", ";5u", ";2u"] {
            if row.contains(bad) {
                return Err(format!("« {bad} » dans « {} »", row.trim()));
            }
        }
    }
    Ok(())
}

/// Whether `text` shows on `term` within `secs` seconds, as a check.
fn shows(term: &TestTerm, text: &str, secs: u64) -> Result<(), String> {
    if term.wait_for(text, Duration::from_secs(secs)) { Ok(()) } else { Err(format!("« {text} » absent")) }
}

/// The text of the rows that hold `needle`.
fn rows_with(term: &TestTerm, needle: &str) -> Vec<String> {
    term.screen().into_iter().filter(|r| r.contains(needle)).collect()
}

/// What the outer terminal sends for a key, depending on whether the multiplexer pushed the kitty protocol to it.
fn key(term: &TestTerm, kitty: &'static [u8], legacy: &'static [u8]) -> &'static [u8] {
    if term.kitty_keyboard() { kitty } else { legacy }
}

/// Two Claude Code in a copy of `recruit _mux`, for each announced terminal and in full screen: the grid's checks
/// that need no window.
#[test]
#[ignore = "real Claude Code sessions; see the module's doc"]
fn claude_through_mux() {
    if !common::consented("CLAUDE_SESSIONS", "real Claude Code sessions", "claude_through_mux") {
        return;
    }
    let before = settings();
    let dir = workdir();
    let temp = tempfile::tempdir().expect("temp dir");
    let mut runs: Vec<(String, &Announced, bool)> = ANNOUNCED.iter().map(|a| (a.name.to_string(), a, false)).collect();
    runs.push(("recruit, plein écran".into(), &ANNOUNCED[0], true));
    runs.push(("ghostty, plein écran".into(), &ANNOUNCED[2], true));
    if let Ok(only) = std::env::var("MUX_ONLY") {
        runs.retain(|(name, _, _)| name.contains(&only));
    }
    let mut table: Vec<(String, Vec<Check>)> = Vec::new();
    for (name, announced, fullscreen) in runs {
        // Each pane's Claude under `script`, which logs what it writes: what it asks of the multiplexer.
        let logs = temp.path().join(format!("logs-{}", table.len()));
        std::fs::create_dir_all(&logs).expect("logs");
        // With `MUX_SCRIPT=1`, each Claude under `script`, which logs what it writes. Not by default: macOS's
        // `script` does not pass size changes on to its inner PTY, and Claude would draw a pane that has narrowed at
        // its first width (cells left behind, letters wrapped along the edge).
        let command: Vec<&str> = if std::env::var_os("MUX_SCRIPT").is_some() {
            vec!["sh", "-c", "exec script -q -F \"$PANE_LOGS/pane.$$\" claude -n testeur-compat"]
        } else {
            vec!["claude", "--strict-mcp-config", "-n", SESSION]
        };
        // The terminal announced, over what the server gives a pane (an empty TERM_PROGRAM for none).
        let logs_text = logs.to_string_lossy().into_owned();
        let (display_key, display_value) = display(fullscreen);
        let display_value = display_value.to_string_lossy().into_owned();
        let mut env: Vec<(&str, &str)> = vec![
            ("PANE_LOGS", &logs_text),
            ("TERM", announced.term),
            ("TERM_PROGRAM", announced.program.unwrap_or("")),
            (display_key, &display_value),
        ];
        if let Some(version) = announced.version {
            env.push(("TERM_PROGRAM_VERSION", version));
        }
        let root = temp.path().join(format!("server-{}", table.len()));
        std::fs::create_dir_all(&root).expect("server root");
        let mut server = common::server::Server::start(&root);
        let mut term = server.attach_as(200, 50, Profile::default());
        std::thread::sleep(Duration::from_millis(500));
        server.spawn("gauche", &dir, &env, &command);
        server.spawn("droite", &dir, &env, &command);
        let mut checks: Vec<Check> = Vec::new();
        let up = wait_prompt(&mut term);
        checks.push(("Les deux Claude démarrent", if up { Ok(()) } else { Err(format!("{:#?}", term.screen())) }));
        std::thread::sleep(Duration::from_secs(2));
        if std::env::var_os("MUX_DEBUG").is_some() {
            println!("{name} : écran au départ : {:#?}", term.screen());
        }
        checks.push(("Écran sans séquence perdue", clean(&term.screen())));
        checks.push(("Kitty poussé au terminal", if term.kitty_keyboard() { Ok(()) } else { Err("non".into()) }));

        // Text with accents, CJK and an emoji.
        term.send("é à ç « » 漢字 🙂".as_bytes());
        checks.push(("Saisie : accents, CJK, emoji", shows(&term, "é à ç « » 漢字", 3)));
        // Shift+Enter, then more text: two lines in the prompt.
        term.send(key(&term, b"\x1b[13;2u", b"\x1b\r"));
        term.send(b"suite");
        let second = shows(&term, "suite", 3).and_then(|()| {
            let same = rows_with(&term, "suite").iter().any(|r| r.contains("漢字"));
            if same { Err("« suite » sur la même ligne : Shift+Entrée a été perdu".into()) } else { Ok(()) }
        });
        if second.is_err() {
            println!("{name} : Shift+Entrée, le panneau : {:#?}", tail(&server.capture(Some("gauche")), 14));
        }
        checks.push(("Shift+Entrée va à la ligne", second));
        // Ctrl+C empties the prompt.
        term.send(key(&term, b"\x1b[99;5u", b"\x03"));
        std::thread::sleep(Duration::from_millis(800));
        let emptied = if rows_with(&term, "suite").is_empty() { Ok(()) } else { Err("saisie toujours là".into()) };
        if emptied.is_err() {
            println!("{name} : Ctrl+C, le panneau : {:#?}", tail(&server.capture(Some("gauche")), 14));
        }
        checks.push(("Ctrl+C vide la saisie", emptied));
        // Shift+Tab changes the permission mode shown under the prompt.
        let footer = |term: &TestTerm| -> Vec<String> {
            term.screen().into_iter().filter(|r| r.contains("mode") || r.contains("⏵") || r.contains("⏸")).collect()
        };
        let mode_before = footer(&term);
        term.send(key(&term, b"\x1b[9;2u", b"\x1b[Z"));
        std::thread::sleep(Duration::from_millis(1000));
        let mode_after = footer(&term);
        checks.push((
            "Shift+Tab change le mode",
            if mode_after != mode_before { Ok(()) } else { Err(format!("{mode_before:?}")) },
        ));
        // Back to the first mode: at most five more.
        for _ in 0..5 {
            if footer(&term) == mode_before {
                break;
            }
            term.send(key(&term, b"\x1b[9;2u", b"\x1b[Z"));
            std::thread::sleep(Duration::from_millis(600));
        }
        // Resize smaller, then back: Claude redraws, nothing left behind.
        term.resize(150, 40);
        std::thread::sleep(Duration::from_secs(2));
        checks.push(("Redimensionné (150×40) : écran propre", clean(&term.screen())));
        term.resize(200, 50);
        std::thread::sleep(Duration::from_secs(2));
        checks.push(("Rendu sa taille : écran propre", clean(&term.screen())));
        // Alt+n gives the focus to the other pane: what is typed shows there, once.
        term.send(key(&term, b"\x1b[110;3u", b"\x1bn"));
        std::thread::sleep(Duration::from_millis(500));
        term.send(b"qwerty");
        let focus = shows(&term, "qwerty", 3).and_then(|()| {
            let n: usize = term.screen().iter().map(|r| r.matches("qwerty").count()).sum();
            if n == 1 { Ok(()) } else { Err(format!("vu {n} fois")) }
        });
        checks.push(("⌥n : la saisie va à l'autre panneau", focus));
        term.send(key(&term, b"\x1b[99;5u", b"\x03"));
        std::thread::sleep(Duration::from_millis(500));
        let server_modes = server.capture_json("gauche")["modes"].to_string();
        // The client gone, the server stopped: nothing left (`Server::stop` checks it).
        drop(term);
        server.stop();
        checks.push(("Arrêt : rien ne reste", Ok(())));
        let mut asked: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&logs).into_iter().flatten().flatten() {
            let raw = std::fs::read(entry.path()).unwrap_or_default();
            let found = modes(&raw);
            let pick = |label: &str| found.iter().find(|(l, _)| *l == label).map_or(String::new(), |(_, v)| v.clone());
            asked.push(format!(
                "kitty {} · XTVERSION {} · DECRQM 2026 {} · BSU {} · 1049 {}",
                pick("kitty poussé (drapeaux)"),
                pick("XTVERSION demandé"),
                pick("DECRQM 2026 demandé"),
                pick("BSU (images synchronisées)"),
                pick("écran alterné 1049/1047"),
            ));
        }
        if asked.is_empty() {
            // Without `script`: the modes the server's engine sees in the pane.
            asked.push(server_modes.clone());
        }
        println!("{name} : ce que demande Claude dans le panneau : {asked:?}");
        table.push((name, checks));
    }
    println!();
    print!("| Vérification |");
    for (name, _) in &table {
        print!(" {name} |");
    }
    println!();
    println!("|---|{}", "---|".repeat(table.len()));
    for row in 0..table[0].1.len() {
        print!("| {} |", table[0].1[row].0);
        for (_, checks) in &table {
            match &checks[row].1 {
                Ok(()) => print!(" ok |"),
                Err(why) => print!(
                    " ko : {} |",
                    why.replace('|', "\\|").replace('\n', " ").chars().take(120).collect::<String>()
                ),
            }
        }
        println!();
    }
    check_settings(&before);
}

/// Standard base64, decoded to text (what OSC 52 carries).
fn base64(text: &str) -> Option<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for byte in text.bytes().filter(|b| *b != b'=') {
        bits = (bits << 6) | ALPHABET.iter().position(|a| *a == byte)? as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    String::from_utf8(out).ok()
}

/// The last `n` non-blank rows of a screen.
fn tail(rows: &[String], n: usize) -> Vec<String> {
    let rows: Vec<String> = rows.iter().filter(|r| !r.trim().is_empty()).cloned().collect();
    rows[rows.len().saturating_sub(n)..].to_vec()
}

/// Where the text `title` (a pane's name, in its frame's top edge) starts on `term`'s screen: (row, column).
fn find(term: &TestTerm, title: &str) -> Option<(usize, usize)> {
    term.screen().iter().enumerate().find_map(|(row, line)| {
        let byte = line.find(title)?;
        Some((row, line[..byte].chars().count()))
    })
}

/// An SGR mouse report (1-based cells): `button` 0 left, 32 + left for a drag, 64/65 wheel up/down; +4 for Shift.
fn mouse(button: u32, col: usize, row: usize, press: bool) -> Vec<u8> {
    format!("\x1b[<{button};{};{}{}", col + 1, row + 1, if press { 'M' } else { 'm' }).into_bytes()
}

/// The mouse through the native server (spec §5.4, §5.5; grid M2, M3, P2): two real Claude Code side by side,
/// one in full screen (it takes the mouse), one in the classic mode. Shift and a drag select in the full-screen
/// one and copy (OSC 52 to the terminal); the wheel goes to Claude in full screen, to the pane's history in the
/// classic mode.
#[test]
#[ignore = "real Claude Code sessions; see the module's doc"]
fn claude_mouse_native() {
    if !common::consented("CLAUDE_SESSIONS", "real Claude Code sessions", "claude_mouse_native") {
        return;
    }
    let before = settings();
    let dir = tempfile::tempdir().expect("temp dir");
    let work = workdir();
    let server = common::server::Server::start(dir.path());
    let ask = "Écris les nombres de 1 à 80, un par ligne, puis le mot FINI-TEST seul sur la dernière ligne.";
    server.spawn("plein", &work, &[("CLAUDE_CODE_NO_FLICKER", "1")], &["claude", "--strict-mcp-config", "-n", SESSION]);
    server.spawn(
        "classique",
        &work,
        &[("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN", "1")],
        &["claude", "--strict-mcp-config", "-n", SESSION],
    );
    let mut term = server.attach(200, 50);
    for member in ["plein", "classique"] {
        assert!(server.wait_pane(member, "❯", Duration::from_secs(40)), "{member}: {:?}", server.capture(Some(member)));
    }
    for member in ["plein", "classique"] {
        server.ctl(["send", "--pane", member, ask]);
        std::thread::sleep(Duration::from_millis(300));
        server.ctl(["key", "--pane", member, "enter"]);
    }
    for member in ["plein", "classique"] {
        assert!(
            server.wait_pane(member, "80", Duration::from_secs(60)),
            "{member}: {:?}",
            server.capture(Some(member))
        );
    }
    std::thread::sleep(Duration::from_secs(3));
    let mut checks: Vec<(&str, Result<(), String>)> = Vec::new();
    let modes = |m: &str| server.capture_json(m)["modes"].clone();
    println!("modes plein : {}", modes("plein"));
    println!("modes classique : {}", modes("classique"));

    // The full-screen pane: its frame title; and the known word, somewhere in it, to select.
    let (top, left) = find(&term, "plein").expect("the full-screen pane's title");
    let (row, col) = term
        .screen()
        .iter()
        .enumerate()
        .find_map(|(r, line)| {
            let byte = line.find("FINI-TEST")?;
            let c = line[..byte].chars().count();
            (c >= left && c < left + 98).then_some((r, c))
        })
        .expect("FINI-TEST on the full-screen pane");
    // Shift + press on its first letter, drag to its last, release (without Shift: the selection begun with it
    // holds): the multiplexer's own selection, copied by OSC 52.
    let raw_before = term.raw().len();
    term.send(&mouse(4, col, row, true));
    for step in 1..=8 {
        term.send(&mouse(4 + 32, col + step, row, true));
        std::thread::sleep(Duration::from_millis(10));
    }
    term.send(&mouse(0, col + 8, row, false));
    std::thread::sleep(Duration::from_millis(800));
    let raw = term.raw();
    let after = String::from_utf8_lossy(&raw[raw_before..]).into_owned();
    let copied = after.find("\x1b]52;").and_then(|i| {
        let rest = &after[i + 5..];
        let data = rest.split_once(';')?.1;
        let end = data.find(['\x07', '\x1b']).unwrap_or(data.len());
        base64(&data[..end])
    });
    println!("OSC 52 reçu : {copied:?}");
    checks.push((
        "Maj+glisser en plein écran : « FINI-TEST » copié par OSC 52",
        match copied.as_deref() {
            Some(text) if text.trim() == "FINI-TEST" => Ok(()),
            Some(text) => Err(format!("copié : {text:?}")),
            None => Err("pas d'OSC 52".into()),
        },
    ));
    let row = top + 4;
    let col = left + 2;

    // The wheel over the full-screen pane: to Claude (the pane's history is not scrolled).
    for _ in 0..3 {
        term.send(&mouse(64, col, row, true));
    }
    std::thread::sleep(Duration::from_millis(800));
    let scrolled = modes("plein")["scrolled"].as_u64().unwrap_or(0);
    checks.push((
        "Molette en plein écran : à Claude",
        if scrolled == 0 { Ok(()) } else { Err(format!("historique défilé de {scrolled}")) },
    ));

    // The wheel over the classic pane: the pane's history scrolls.
    let (ctop, cleft) = find(&term, "classique").expect("the classic pane's title");
    for _ in 0..3 {
        term.send(&mouse(64, cleft + 2, ctop + 4, true));
    }
    std::thread::sleep(Duration::from_millis(800));
    let scrolled = modes("classique")["scrolled"].as_u64().unwrap_or(0);
    checks.push((
        "Molette en mode classique : l'historique",
        if scrolled > 0 { Ok(()) } else { Err("pas défilé".into()) },
    ));
    // Back down.
    for _ in 0..10 {
        term.send(&mouse(65, cleft + 2, ctop + 4, true));
    }
    std::thread::sleep(Duration::from_millis(500));

    for (what, result) in &checks {
        println!(
            "| {what} | {} |",
            match result {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("ko : {e}"),
            }
        );
    }
    drop(term);
    drop(server);
    check_settings(&before);
}

/// The glyph a pane's header shows before its member's name (`┏━ ◷ lead ━`), on the client's screen.
fn header_glyph(screen: &[String], name: &str) -> Option<char> {
    let key = format!(" {name} ");
    screen.iter().find_map(|row| {
        if !row.chars().any(|c| matches!(c, '┏' | '╭')) {
            return None;
        }
        let at = row.find(&key)?;
        row[..at].chars().rev().find(|c| !c.is_whitespace())
    })
}

/// The glyph the dashboard's card of `name` shows (`╻ ◷ lead`).
fn card_glyph(screen: &[String], name: &str) -> Option<char> {
    let key = format!(" {name} ");
    screen.iter().find_map(|row| {
        let from = row.find('╻')? + '╻'.len_utf8();
        let rest = &row[from..];
        let at = rest.find(&key)?;
        rest[..at].chars().rev().find(|c| !c.is_whitespace())
    })
}

/// A short code for a state glyph: W at work (a spinner), A waiting (⚑), R at rest (◷), O other (◌), else the glyph.
fn state_of(glyph: Option<char>) -> String {
    match glyph {
        None => "-".into(),
        Some('\u{2800}'..='\u{28ff}') => "W".into(),
        Some('\u{2691}') => "A".into(),
        Some('\u{25f7}') => "R".into(),
        Some('\u{25cc}') => "O".into(),
        Some(other) => other.to_string(),
    }
}

/// What the header says of two members (OSC 7501, from Claude Code) against what the dashboard's card says
/// (`claude agents --json`, every two seconds), through a work, a permission and a command left running. Two real
/// Claude Code sessions on Haiku, in a folder of the tester's own, in the test's terminal only: no window. The
/// timeline goes to `target/night/real-states.csv`: a row each 200 ms and an event column.
///
/// ```sh
/// CLAUDE_SESSIONS=$(date +%F) cargo test --test mux_claude -- --ignored --nocapture claude_header_against_card
/// ```
#[test]
#[ignore = "real Claude Code sessions (two, on Haiku, about 3 minutes): CLAUDE_SESSIONS and the test named"]
fn claude_header_against_card() {
    if !common::consented("CLAUDE_SESSIONS", "real Claude Code sessions", "claude_header_against_card") {
        return;
    }
    use std::time::Instant;
    let before = settings();
    let tables = "\
[teams.states]
description = \"test\"
args = [\"--strict-mcp-config\"]
[teams.states.members.lead]
role = \"Contact\"
contact = true
model = \"haiku\"
[teams.states.members.pm]
role = \"Contact\"
contact = true
model = \"haiku\"
";
    let mut team = common::team::Team::real("states", tables, "en");
    let out = team.recruit(["--detach"]);
    assert!(out.status.success(), "recruit --detach: {}", String::from_utf8_lossy(&out.stderr));
    let term = team.attach(150, 50, Profile::default());
    let screen = |term: &TestTerm| term.screen();
    let started = Instant::now();
    let mut rows: Vec<String> = vec!["t,lead_header,lead_card,pm_header,pm_card,event".into()];
    let mut event = String::new();
    // One sample: both members' header and card.
    let sample = |term: &TestTerm, event: &mut String, rows: &mut Vec<String>| {
        let s = screen(term);
        let row = format!(
            "{:.1},{},{},{},{},{}",
            started.elapsed().as_secs_f64(),
            state_of(header_glyph(&s, "lead")),
            state_of(card_glyph(&s, "lead")),
            state_of(header_glyph(&s, "pm")),
            state_of(card_glyph(&s, "pm")),
            event
        );
        event.clear();
        rows.push(row);
        s
    };
    let send = |member: &str, text: &str| {
        // Typed, then Enter a moment later: text and Enter in one burst is taken for a paste, and not sent.
        team.ctl(["send", "--pane", member, text]);
        std::thread::sleep(Duration::from_millis(700));
        team.ctl(["send", "--pane", member, "\\r"]);
    };
    // Until both are at rest (Claude Code says it by OSC 7501 once it is up).
    let mut trusted = std::collections::HashSet::new();
    let up = common::team::wait(Duration::from_secs(120), || {
        // Claude Code asks once whether it may trust a folder it has not seen, « No, exit » first: down, Enter.
        for member in ["lead", "pm"] {
            if !trusted.contains(member) && team.capture(member).iter().any(|r| r.contains("Yes, I trust this folder"))
            {
                trusted.insert(member);
                team.ctl(["send", "--pane", member, "\\x1b[B"]);
                std::thread::sleep(Duration::from_millis(500));
                team.ctl(["send", "--pane", member, "\\r"]);
                event = format!("{member}: trusted the folder");
            }
        }
        let s = sample(&term, &mut event, &mut rows);
        header_glyph(&s, "lead") == Some('\u{25f7}') && header_glyph(&s, "pm") == Some('\u{25f7}')
    });
    println!("members up: {up}, after {:.0} s", started.elapsed().as_secs_f64());
    // Whatever stands in the way (a question on startup) is shown, not guessed.
    if !up {
        for row in term.screen() {
            println!("{row}");
        }
    }
    let mut answered = std::collections::HashSet::new();
    let phase = |term: &TestTerm,
                 label: &str,
                 prompt: &str,
                 rest_after: Duration,
                 event: &mut String,
                 rows: &mut Vec<String>,
                 answered: &mut std::collections::HashSet<String>| {
        *event = format!("{label}: sent");
        for member in ["lead", "pm"] {
            send(member, &prompt.replace("{member}", member));
        }
        let begun = Instant::now();
        let mut rest_since: Option<Instant> = None;
        let mut worked = false;
        while begun.elapsed() < Duration::from_secs(150) {
            let s = sample(term, event, rows);
            for member in ["lead", "pm"] {
                let key = format!("{label}-{member}-{}", begun.elapsed().as_secs() / 10);
                if header_glyph(&s, member) == Some('\u{2691}') && answered.insert(key) {
                    std::thread::sleep(Duration::from_millis(700));
                    // The permission question, « 1. Yes » first: Enter allows it.
                    team.ctl(["send", "--pane", member, "\\r"]);
                    *event = format!("{label}: {member} allowed");
                }
            }
            let working = ["lead", "pm"].iter().any(|m| matches!(state_of(header_glyph(&s, m)).as_str(), "W" | "A"));
            worked |= working;
            if !worked && begun.elapsed() > Duration::from_secs(20) && !answered.contains(&format!("{label}-shown")) {
                answered.insert(format!("{label}-shown"));
                println!("{label}: nothing after 20 s, lead's pane:");
                for row in team.capture("lead") {
                    println!("  |{row}");
                }
            }
            if worked && !working {
                let since = *rest_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= rest_after {
                    break;
                }
            } else {
                rest_since = None;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    };
    phase(
        &term,
        "work",
        "Create the file note-{member}.txt containing the word hello, then run the shell command `sleep 6`, \
         then reply with the single word done.",
        Duration::from_secs(8),
        &mut event,
        &mut rows,
        &mut answered,
    );
    phase(
        &term,
        "background",
        "Start the shell command `sleep 25` in the background (run_in_background), \
         then reply with the single word started, and stop.",
        Duration::from_secs(35),
        &mut event,
        &mut rows,
        &mut answered,
    );
    std::fs::create_dir_all("target/night").ok();
    std::fs::write("target/night/real-states.csv", rows.join("\n") + "\n").expect("csv");
    let root = team.root.to_string_lossy().to_string();
    team.stop();
    check_settings(&before);
    let key: String = root.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let projects = PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".claude/projects");
    let left: Vec<String> = std::fs::read_dir(&projects)
        .map(|d| {
            d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with(&key)).collect()
        })
        .unwrap_or_default();
    println!("folders left under {}: {left:?}", projects.display());
    println!("timeline: target/night/real-states.csv ({} rows)", rows.len());
}

/// Where `script` records the raw output of each member's Claude (`raw-<member>.log`), and the wrapper that runs it.
fn compact_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("recruit-testeur-compact");
    std::fs::create_dir_all(&dir).expect("compact dir");
    dir
}

/// A `claude` that records the raw output of a member's session (`script -q -F`, a pty of its own between the pane and
/// Claude Code, the bytes unchanged), and runs every other command (`--version`, `agents`…) as it is.
fn compact_wrapper(dir: &std::path::Path) -> PathBuf {
    let real = std::process::Command::new("sh")
        .args(["-c", "command -v claude"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    assert!(!real.is_empty(), "no claude on the PATH");
    let wrapper = dir.join("claude-wrap");
    let script = format!(
        r#"#!/bin/sh
REAL='{real}'
case "$1" in --version|agents|plugin|mcp|doctor|config|-p|--print) exec "$REAL" "$@";; esac
name=x; prev=
for a in "$@"; do [ "$prev" = "-n" ] && name=$a; prev=$a; done
exec script -q -F '{dir}/raw-'"$name"'.log' "$REAL" "$@"
"#,
        dir = dir.display()
    );
    std::fs::write(&wrapper, script).expect("wrapper");
    std::fs::set_permissions(&wrapper, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    wrapper
}

/// The OSC 7501 sequences of `bytes` as text (`ESC`, `BEL`, `ST` spelt out), each with where it started.
fn osc_7501(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = bytes[at..].windows(6).position(|w| w == b"\x1b]7501") {
        let start = at + found;
        let rest = &bytes[start..];
        let end = rest
            .iter()
            .position(|b| *b == 0x07)
            .map(|i| (i + 1, "BEL"))
            .into_iter()
            .chain(rest.windows(2).position(|w| w == b"\x1b\\").map(|i| (i + 2, "ST")))
            .min_by_key(|(i, _)| *i);
        match end {
            Some((len, terminator)) => {
                let body = String::from_utf8_lossy(&rest[2..len - if terminator == "BEL" { 1 } else { 2 }]).to_string();
                out.push(format!("ESC ] {body} {terminator}"));
                at = start + len;
            }
            None => break,
        }
    }
    out
}

/// Samples a real team: each 300 ms or so the raw OSC 7501 each member's Claude wrote (from `script`'s log), what
/// `_ctl panes` says of each pane, the glyphs of the dashboard's cards, and the events the test names. Writes the
/// timeline and the raw sequences into `target/night/`.
struct Recorder<'a> {
    team: &'a common::team::Team,
    term: &'a TestTerm,
    dir: PathBuf,
    started: std::time::Instant,
    offsets: std::collections::HashMap<&'static str, u64>,
    rows: Vec<String>,
    osc: Vec<String>,
    event: String,
    /// Each member's status as `_ctl panes` last said, with since.
    status: std::collections::HashMap<&'static str, (String, u64)>,
}

impl<'a> Recorder<'a> {
    fn new(team: &'a common::team::Team, term: &'a TestTerm, dir: PathBuf) -> Recorder<'a> {
        Recorder {
            team,
            term,
            dir,
            started: std::time::Instant::now(),
            offsets: Default::default(),
            rows: vec!["t,lead_status,lead_since,dev_status,dev_since,lead_card,dev_card,compact_files,event".into()],
            osc: Vec::new(),
            event: String::new(),
            status: Default::default(),
        }
    }

    fn at(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn note(&mut self, event: &str) {
        println!("[{:6.1}] {event}", self.at());
        if !self.event.is_empty() {
            self.event.push_str(" / ");
        }
        self.event.push_str(&event.replace(',', ";"));
    }

    /// The raw sequences written since the last look, per member.
    fn raw(&mut self) -> Vec<(&'static str, String)> {
        let mut found = Vec::new();
        for member in ["lead", "dev"] {
            let Ok(bytes) = std::fs::read(self.dir.join(format!("raw-{member}.log"))) else { continue };
            let from = self.offsets.get(member).copied().unwrap_or(0) as usize;
            if bytes.len() <= from {
                continue;
            }
            // A sequence cut by the end of what was written is read at the next look.
            let chunk = &bytes[from..];
            let mut upto = chunk.len();
            if let Some(open) = chunk.windows(6).rposition(|w| w == b"\x1b]7501") {
                let tail = &chunk[open..];
                if !tail.contains(&0x07) && !tail.windows(2).any(|w| w == b"\x1b\\") {
                    upto = open;
                }
            }
            for sequence in osc_7501(&chunk[..upto]) {
                found.push((member, sequence));
            }
            self.offsets.insert(member, (from + upto) as u64);
        }
        found
    }

    /// One look: the screen, and a row of the timeline.
    fn sample(&mut self) -> Vec<String> {
        let t = self.at();
        for (member, sequence) in self.raw() {
            self.osc.push(format!("{t:7.2} {member:4} {sequence}"));
            println!("[{t:6.1}] raw {member}: {sequence}");
        }
        for pane in self.team.panes() {
            let name = match pane["member"].as_str() {
                Some("lead") => "lead",
                Some("dev") => "dev",
                _ => continue,
            };
            let status = pane["status"].as_str().unwrap_or("-").to_string();
            let since = pane["since"].as_u64().unwrap_or(0);
            if self.status.get(name).map(|(s, _)| s.as_str()) != Some(status.as_str()) {
                println!("[{t:6.1}] pane {name}: {status} (since {since})");
            }
            self.status.insert(name, (status, since));
        }
        let screen = self.term.screen();
        let card = |name: &str| state_of(card_glyph(&screen, name));
        let files: Vec<String> = ["lead", "dev"]
            .iter()
            .filter(|m| self.team.state().join("compact").join(m).exists())
            .map(|m| m.to_string())
            .collect();
        let status = |name: &str| self.status.get(name).cloned().unwrap_or_default();
        let (lead, dev) = (status("lead"), status("dev"));
        let row = format!(
            "{t:.1},{},{},{},{},{},{},{},{}",
            lead.0,
            lead.1,
            dev.0,
            dev.1,
            card("lead"),
            card("dev"),
            files.join("+"),
            self.event
        );
        self.event.clear();
        self.rows.push(row);
        screen
    }

    /// Samples until `check` holds on the screen, `timeout` at most.
    fn until(&mut self, timeout: Duration, mut check: impl FnMut(&mut Recorder<'a>, &[String]) -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let screen = self.sample();
            if check(self, &screen) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Samples for `span`.
    fn pass(&mut self, span: Duration) {
        self.until(span, |_, _| false);
    }

    fn status_of(&self, member: &str) -> String {
        self.status.get(member).map(|(s, _)| s.clone()).unwrap_or_default()
    }

    fn save(&self, name: &str) {
        std::fs::create_dir_all("target/night").ok();
        std::fs::write(format!("target/night/{name}.csv"), self.rows.join("\n") + "\n").expect("csv");
        std::fs::write(format!("target/night/{name}-osc.txt"), self.osc.join("\n") + "\n").expect("osc");
    }
}

/// One step of the folder-trust question in `member`'s pane, if it is shown and still: a key sent while the screen
/// moves is read by Claude Code's early-input reader (see `wait_prompt`), so each key goes to a screen that stood still
/// for 600 ms: Down while « No, exit » is chosen, then Enter on « Yes, I trust ». True if a key was sent.
fn answer_trust(team: &common::team::Team, member: &str) -> bool {
    let rows = team.capture(member);
    if !rows.iter().any(|r| r.contains("trust this folder")) {
        return false;
    }
    std::thread::sleep(Duration::from_millis(600));
    let again = team.capture(member);
    if again != rows {
        return false;
    }
    let on_yes = again.iter().any(|r| r.contains('❯') && r.contains("Yes, I trust"));
    team.ctl(["send", "--pane", member, if on_yes { "\\r" } else { "\\x1b[B" }]);
    true
}

/// Typed, then Enter a moment later: text and Enter in one burst is taken for a paste, and not sent.
fn type_into(team: &common::team::Team, member: &str, text: &str) {
    team.ctl(["send", "--pane", member, text]);
    std::thread::sleep(Duration::from_millis(700));
    team.ctl(["send", "--pane", member, "\\r"]);
}

/// A member's report (`<state>/members/<member>.json`), as the mod last wrote it.
fn member_report(team: &common::team::Team, member: &str) -> serde_json::Value {
    std::fs::read_to_string(team.state().join("members").join(format!("{member}.json")))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// The automatic compaction of a member that rests out of sight, with real Claude Code (2.1.296): what it writes in
/// OSC 7501 from the request to after its end, a message typed during it, the summary it makes, the report and the
/// trace, and a `/compact` typed in a pane. Two Haiku sessions in a folder of the tester's own, on the test's terminal
/// only. The timeline goes to `target/night/real-compact.csv` and the raw sequences (as `script` recorded them) to
/// `target/night/real-compact-osc.txt`.
///
/// ```sh
/// CLAUDE_SESSIONS=$(date +%F) cargo test --test mux_claude -- --ignored --nocapture claude_auto_compaction
/// ```
#[test]
#[ignore = "real Claude Code sessions (two, on Haiku, about 6 minutes): CLAUDE_SESSIONS and the test named"]
fn claude_auto_compaction() {
    if !common::consented("CLAUDE_SESSIONS", "real Claude Code sessions", "claude_auto_compaction") {
        return;
    }
    let before = settings();
    let dir = compact_dir();
    for member in ["lead", "dev"] {
        let _ = std::fs::remove_file(dir.join(format!("raw-{member}.log")));
    }
    let wrapper = compact_wrapper(&dir);
    let tables = format!(
        "[claude]\ncommand = \"{}\"\n\n[teams.compact]\ndescription = \"test\"\nargs = [\"--strict-mcp-config\"]\n\
         auto_compact = 1\n[teams.compact.members.lead]\nrole = \"Contact\"\ncontact = true\nmodel = \"haiku\"\n\
         [teams.compact.members.dev]\nrole = \"Agent\"\nmodel = \"haiku\"\n",
        wrapper.display()
    );
    let mut team = common::team::Team::real("compact", &tables, "en");
    team.env("RECRUIT_AUTO_COMPACT_IDLE", "6");
    let out = team.recruit(["--detach"]);
    assert!(out.status.success(), "recruit --detach: {}", String::from_utf8_lossy(&out.stderr));
    // The first tab: lead, the dashboard and the journal. `dev` is on the second: out of sight.
    let term = team.attach(150, 50, Profile::default());
    let mut rec = Recorder::new(&team, &term, dir.clone());
    let up = rec.until(Duration::from_secs(150), |rec, screen| {
        for member in ["lead", "dev"] {
            if answer_trust(rec.team, member) {
                rec.note(&format!("{member}: answered the folder question"));
            }
        }
        let _ = screen;
        rec.status_of("lead") == "idle" && rec.status_of("dev") == "idle"
    });
    println!("members up: {up}");
    if !up {
        for row in term.screen() {
            println!("{row}");
        }
    }
    assert!(up, "the members never came up");
    // Claude Code reads the first keys itself for a moment after it starts: nothing typed before it is settled.
    rec.pass(Duration::from_secs(4));
    // A turn ends with `done` (not `idle`): the pane's state says so once it does.
    let turn = |rec: &mut Recorder, member: &str, text: &str, what: &str| -> bool {
        rec.note(&format!("{member}: sends {what}"));
        type_into(rec.team, member, text);
        let worked = rec.until(Duration::from_secs(90), |rec, _| rec.status_of(member) == "working");
        let done = worked && rec.until(Duration::from_secs(120), |rec, _| rec.status_of(member) != "working");
        rec.note(&format!("{member}: {what} answered: {done}"));
        done
    };
    // Text only, no tool: a permission question would stop the turn (nobody answers it here).
    let dev1 = turn(
        &mut rec,
        "dev",
        "Write a four-line poem about the sea, then end your reply with exactly this line: The harbor code is ZEBRA-42.",
        "the first task",
    );
    let dev2 = turn(
        &mut rec,
        "dev",
        "Give three short tips for a long train trip, then end your reply with exactly this line: Last answer: tips delivered.",
        "the second task",
    );
    let lead1 = turn(&mut rec, "lead", "Reply with the single word ok.", "a word");
    println!("turns: dev {dev1} {dev2}, lead {lead1}");
    println!("reports after the turns: lead {}\n  dev {}", member_report(&team, "lead"), member_report(&team, "dev"));
    // The rest begins; the request comes after 6 s of it (the server, `compact/dev`), and the mod takes it at its next tick.
    rec.note("rest begins");
    let asked = rec.until(Duration::from_secs(60), |rec, _| rec.team.state().join("compact/dev").exists());
    rec.note(&format!("request for dev: {asked}"));
    if asked {
        println!("request: {}", std::fs::read_to_string(team.state().join("compact/dev")).unwrap_or_default());
    }
    // 1 and 2 (a): until the compaction is over: no message sent.
    let compacted = rec.until(Duration::from_secs(180), |rec, _| {
        let r = member_report(rec.team, "dev");
        r["last_compacted"]["done"] == true
    });
    rec.note(&format!("first compaction made: {compacted}"));
    rec.pass(Duration::from_secs(12));
    let report = member_report(&team, "dev");
    println!("dev report.compaction: {}", report["compaction"]);
    println!("dev report.last_compacted: {}", report["last_compacted"]);
    println!("dev report.context: {}", report["context"]);
    for (name, member) in [("dev", "dev"), ("lead", "lead")] {
        println!("{name} last_compacted: {}", member_report(&team, member)["last_compacted"]);
    }
    let shot = |name: &str, screen: &[String]| {
        std::fs::create_dir_all("target/night").ok();
        std::fs::write(format!("target/night/real-compact-{name}.txt"), screen.join("\n") + "\n").ok();
    };
    let screen = rec.sample();
    shot("after-first", &screen);
    for row in screen.iter().filter(|r| r.contains('\u{27f3}') || r.to_lowercase().contains("compact")) {
        println!("  screen: {row}");
    }
    // 2 (b): a second compaction; a message typed into the pane while it runs.
    let ping = turn(&mut rec, "dev", "Reply with the single word ping.", "a word, after the first compaction");
    println!("ping answered: {ping}");
    rec.note("second rest begins");
    let asked = rec.until(Duration::from_secs(60), |rec, _| rec.team.state().join("compact/dev").exists());
    rec.note(&format!("second request for dev: {asked}"));
    let mut sent_at = None;
    let mut working_seen = false;
    let mut message_started = false;
    let second = rec.until(Duration::from_secs(240), |rec, _| {
        let raw_working = rec.osc.iter().rev().take(6).any(|l| l.contains(" dev ") && l.contains("state=working"));
        if !working_seen && asked && !rec.team.state().join("compact/dev").exists() && raw_working {
            working_seen = true;
        }
        if working_seen && sent_at.is_none() {
            sent_at = Some(rec.at());
            rec.note("dev: typed a message during the compaction");
            type_into(rec.team, "dev", "Write about 400 words about trains, in plain text, no title.");
        }
        if sent_at.is_some() {
            message_started |=
                rec.osc.iter().rev().take(10).any(|l| l.contains(" dev ") && l.contains("state=working"));
        }
        let r = member_report(rec.team, "dev");
        sent_at.is_some()
            && r["last_compacted"]["done"] == true
            && r["last_compacted"]["at"] != report["last_compacted"]["at"]
    });
    let made_at = rec.at();
    rec.note(&format!("second compaction made: {second}; the message was typed at {sent_at:?}"));
    // The turn the message makes, and what the member shows during it.
    rec.pass(Duration::from_secs(45));
    // `_ctl panes` says `dev` at work from the start of the queued turn (the compaction made, the mod's answer a tick
    // later) to its `done`, and at rest after it: Claude Code says `working` once and `done` once for both.
    let done_at = rec
        .osc
        .iter()
        .filter(|l| l.contains(" dev ") && l.contains("state=done"))
        .filter_map(|l| l.split_whitespace().next()?.parse::<f64>().ok())
        .filter(|t| *t > sent_at.unwrap_or(f64::MAX))
        .fold(None, |first: Option<f64>, t| Some(first.map_or(t, |f| f.min(t))));
    match (sent_at, done_at) {
        (Some(_), Some(done)) => {
            let dev_at = |from: f64, to: f64| -> Vec<(f64, String)> {
                rec.rows
                    .iter()
                    .skip(1)
                    .filter_map(|row| {
                        let cells: Vec<&str> = row.split(',').collect();
                        let t: f64 = cells.first()?.parse().ok()?;
                        (t >= from && t <= to).then(|| (t, cells.get(3).unwrap_or(&"").to_string()))
                    })
                    .collect()
            };
            let during = dev_at(made_at + 3.0, done - 1.0);
            let after = dev_at(done + 1.5, done + 6.0);
            println!(
                "the queued turn ran until {done:.1} s; compaction made at {made_at:.1} s; rows at work: {}/{} in [{:.1}, {:.1}], at rest: {}/{} after",
                during.iter().filter(|(_, s)| s == "working").count(),
                during.len(),
                made_at + 3.0,
                done - 1.0,
                after.iter().filter(|(_, s)| s == "idle").count(),
                after.len(),
            );
            if done - made_at > 5.0 {
                assert!(
                    !during.is_empty() && during.iter().all(|(_, s)| s == "working"),
                    "dev not at work during the queued turn: {during:?}"
                );
            }
            assert!(
                !after.is_empty() && after.iter().all(|(_, s)| s == "idle"),
                "dev not at rest after its done: {after:?}"
            );
        }
        other => println!("no queued turn to check: {other:?}"),
    }
    let screen = rec.sample();
    shot("after-second", &screen);
    println!("message started: {message_started}");
    println!("dev report.last_compacted: {}", member_report(&team, "dev")["last_compacted"]);
    // 3: a typed `/compact` in lead's pane.
    rec.note("lead: types /compact");
    type_into(&team, "lead", "/compact");
    let typed =
        rec.until(Duration::from_secs(150), |rec, _| member_report(rec.team, "lead")["last_compacted"]["done"] == true);
    rec.note(&format!("lead compacted by hand: {typed}"));
    rec.pass(Duration::from_secs(15));
    println!("lead report.compaction: {}", member_report(&team, "lead")["compaction"]);
    println!("lead report.last_compacted: {}", member_report(&team, "lead")["last_compacted"]);
    let screen = rec.sample();
    shot("after-typed", &screen);
    for row in screen.iter().filter(|r| r.contains('\u{27f3}') || r.to_lowercase().contains("compact")) {
        println!("  screen: {row}");
    }
    rec.save("real-compact");
    // The summary dev's conversation holds: what the compaction kept.
    let session = std::fs::read_to_string(team.state().join("sessions/dev")).unwrap_or_default().trim().to_string();
    let root = team.root.to_string_lossy().to_string();
    let key: String = format!("{root}/proj").chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let transcript = home.join(".claude/projects").join(&key).join(format!("{session}.jsonl"));
    println!("dev's conversation: {}", transcript.display());
    let text = std::fs::read_to_string(&transcript).unwrap_or_default();
    let mut summaries = 0;
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let flat = entry.to_string();
        if entry["isCompactSummary"] == true
            || entry["subtype"] == "compact_boundary"
            || flat.contains("continued from a previous conversation")
        {
            summaries += 1;
            let body = entry["message"]["content"].as_str().map(String::from).unwrap_or_else(|| flat.clone());
            println!("--- compaction entry {summaries}: type {} subtype {}", entry["type"], entry["subtype"]);
            println!(
                "    ZEBRA-42: {}, tips delivered: {}, train: {}",
                body.contains("ZEBRA-42"),
                body.contains("tips delivered"),
                body.contains("train")
            );
            std::fs::write(format!("target/night/real-compact-summary-{summaries}.txt"), &body).ok();
        }
    }
    println!("compaction entries in the conversation: {summaries}");
    // The mod and the bridge agree: no report says it could not read what the mod sent.
    for member in ["lead", "dev"] {
        let report = member_report(&team, member);
        println!(
            "{member}: input_error {:?}, model {}, context {}",
            report["input_error"], report["model"], report["context"]
        );
        assert!(
            report["input_error"].is_null(),
            "{member}'s report: the bridge could not read the mod's tick: {report}"
        );
    }
    let log = std::fs::read_to_string(team.state().join("server.log")).unwrap_or_default();
    let unread: Vec<&str> = log.lines().filter(|l| l.contains("does not read")).collect();
    println!("server.log lines with « does not read »: {unread:?}");
    assert!(unread.is_empty(), "the server could not read what the mod wrote: {unread:?}");
    let leftovers = vec![format!("~/.claude/projects/{key}*"), format!("~/.claude.json: the entry for {root}/proj")];
    team.stop();
    check_settings(&before);
    println!("traces left to tidy when no Claude runs: {leftovers:?}");
}

/// What the mod reports of a real session (`<state>/members/<member>.json`: model, context…), with and without the
/// wrapper that records the raw output (`COMPACT_WRAP=1`): one Haiku member, one word asked.
///
/// ```sh
/// CLAUDE_SESSIONS=$(date +%F) [COMPACT_WRAP=1] cargo test --test mux_claude -- --ignored --nocapture claude_mod_report
/// ```
#[test]
#[ignore = "a real Claude Code session (one, on Haiku, about 1 minute): CLAUDE_SESSIONS and the test named"]
fn claude_mod_report() {
    if !common::consented("CLAUDE_SESSIONS", "a real Claude Code session", "claude_mod_report") {
        return;
    }
    let before = settings();
    let dir = compact_dir();
    let command = if std::env::var_os("COMPACT_WRAP").is_some() {
        format!("[claude]\ncommand = \"{}\"\n\n", compact_wrapper(&dir).display())
    } else {
        String::new()
    };
    let tables = format!(
        "{command}[teams.rep]\ndescription = \"test\"\nargs = [\"--strict-mcp-config\"]\n[teams.rep.members.lead]\n\
         role = \"Contact\"\ncontact = true\nmodel = \"haiku\"\n"
    );
    let team = common::team::Team::real("rep", &tables, "en");
    let out = team.recruit(["--detach"]);
    assert!(out.status.success(), "recruit --detach: {}", String::from_utf8_lossy(&out.stderr));
    let term = team.attach(120, 40, Profile::default());
    let up = common::team::wait(Duration::from_secs(120), || {
        answer_trust(&team, "lead");
        team.panes().iter().any(|p| p["member"] == "lead" && p["status"] == "idle")
    });
    assert!(up, "lead never came up: {:#?} {:?}", team.panes(), team.capture("lead"));
    std::thread::sleep(Duration::from_secs(3));
    type_into(&team, "lead", "Reply with the single word ok.");
    common::team::wait(Duration::from_secs(60), || {
        team.panes().iter().any(|p| p["member"] == "lead" && p["status"] == "working")
    });
    common::team::wait(Duration::from_secs(60), || {
        team.panes().iter().any(|p| p["member"] == "lead" && p["status"] != "working")
    });
    for _ in 0..4 {
        std::thread::sleep(Duration::from_secs(3));
        println!("report: {}", member_report(&team, "lead"));
    }
    let report = member_report(&team, "lead");
    assert!(report["input_error"].is_null(), "the bridge could not read the mod's tick: {report}");
    assert!(report["model"].is_string() && report["context"].is_object(), "an empty report: {report}");
    let log = std::fs::read_to_string(team.state().join("server.log")).unwrap_or_default();
    let unread: Vec<&str> = log.lines().filter(|l| l.contains("does not read")).collect();
    assert!(unread.is_empty(), "the server could not read what the mod wrote: {unread:?}");
    drop(term);
    let mut team = team;
    team.stop();
    check_settings(&before);
}
