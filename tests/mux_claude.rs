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
            let profile = Profile { kitty_keyboard: true, capture: true, xtversion: None };
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
