// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Real keystrokes in real terminals (spec §6, "Clavier", "Collage"; grid `specs/multiplexeur-compat.md`): each
//! terminal opens a window on the `keylog` fake, once alone (the reference), once through a copy of recruit: a
//! native server of the test's own with the fake in its one pane and the real client (`_ctl attach`); System
//! Events types the keys; the fake logs the bytes its pane
//! gets. The table compares the two. Run before each release, by hand:
//!
//! ```sh
//! KEYS_CONSENT=$(date +%F) cargo test --release --test mux_keys -- --ignored --nocapture --test-threads=1 real_keys
//! KEYS_CONSENT=$(date +%F) KEYS_TERMINALS=kitty KEYS_VARIANTS=installed KEYS_MODES=claude cargo test --release --test mux_keys -- --ignored --nocapture real_keys
//! ```
//!
//! It needs, for the application macOS holds responsible for this process (the terminal it runs in): Accessibility
//! ("Contrôle de l'appareil et accès aux données" in macOS 27), and Automation of System Events. Keys go to the
//! frontmost application's key window, whichever it is: nobody types during a run, and a run starts only with the
//! user's go. Guards, since keys once went to a restored shell window instead of the test's (2026-10-09):
//!
//! - no saved window restored (Ghostty `--window-save-state=never`; kitty and WezTerm without configuration);
//!   Terminal.app and iTerm2, single-instance applications, are skipped when already running;
//! - before each key, the frontmost process must be the terminal this test opened, and its key window must bear the
//!   fake's own title (OSC 2);
//! - a harmless witness key (`z`) first, and before each dangerous key, must reach the fake within 500 ms;
//! - dangerous keys (Enter, Ctrl+C, Ctrl+D, Shift+Tab, pastes, the multiplexer's shortcuts) only with
//!   `KEYS_DANGEROUS=1`;
//! - the terminal opened is ended by its pid, checked first to still be that terminal; and every process of the
//!   application that a launch added is ended when its window is done, by SIGKILL if SIGTERM is not enough (a
//!   window of that application the user would open during the run too: nobody touches anything during a run);
//! - nothing at all without `KEYS_CONSENT=<today's date>` (YYYY-MM-DD), given with the user's go for that day,
//!   and `real_keys` named on the command line: `--ignored` alone (a whole `cargo test -- --ignored`, the CI)
//!   types nothing, even with the variable still exported.
//!
//! macOS 14 and later activate applications "cooperatively": a background process cannot bring another
//! application to the front while the active one (the user's own terminal, where they work) keeps it. A second
//! instance of the user's terminal (Ghostty here) never comes to the front, and the run stops with nothing typed.
//! Hence terminals the user does not have open: Terminal.app, iTerm2, kitty, WezTerm, which a fresh launch may
//! bring to the front. `KEYS_WAIT` gives the test window longer (the user would click it; not chosen, 2026-10-09).
//!
//! The clipboard (the pastes go through it) is saved first, every type of every item, and put back at the end,
//! after a failure too, and on Ctrl-C. The saved copy is on disk, in clear, in the run's temporary folder, for the
//! length of the run (kept afterwards only if it could not be put back).
//!
//! Owner: testeur.
#![cfg(target_os = "macos")]

mod common;

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set by SIGINT (Ctrl-C): the whole run stops at the next key, and the clipboard is put back.
static STOP: AtomicBool = AtomicBool::new(false);

#[test]
#[ignore = "started by the other tests only"]
fn child() {
    match common::role().as_deref() {
        Some(role) if common::fakes::run(role) => {}
        Some(other) => panic!("unknown child role {other}"),
        None => {}
    }
}

/// A key, as System Events types it, and whether it must reach the pane through the multiplexer.
struct Stroke {
    name: &'static str,
    /// The AppleScript after `tell application "System Events" to`.
    script: String,
    /// False for the multiplexer's own shortcuts: through it, the pane must get nothing.
    reaches_pane: bool,
    /// What would do harm in a window that is not the test's (a shell, a Claude session, one of the team's): Enter,
    /// Escape (interrupts a turn), Ctrl+B (sends a command to the background), Ctrl+O, Ctrl+C, Ctrl+D, Shift+Tab,
    /// pastes, the multiplexer's shortcuts. Typed only with `KEYS_DANGEROUS=1`, and each after
    /// a witness key.
    dangerous: bool,
}

fn keystroke(name: &'static str, what: &str, using: &str) -> Stroke {
    let using = if using.is_empty() { String::new() } else { format!(" using {{{using}}}") };
    Stroke { name, script: format!("{what}{using}"), reaches_pane: true, dangerous: false }
}

/// The same, harmful in another window.
fn dangerous(name: &'static str, what: &str, using: &str) -> Stroke {
    Stroke { dangerous: true, ..keystroke(name, what, using) }
}

fn strokes() -> Vec<Stroke> {
    let text = |c: &str| format!("keystroke \"{c}\"");
    let code = |n: u32| format!("key code {n}");
    let mut list = vec![
        // Not a key: the fake's probe (👍🏽 then a cursor position request), answered by the terminal (direct) or by
        // the multiplexer's engine. Column 3: two cells; column 5: four.
        Stroke {
            name: "U3 : 👍🏽, colonne du curseur après",
            script: String::new(),
            reaches_pane: true,
            dangerous: false,
        },
        dangerous("Shift+Entrée", &code(36), "shift down"),
        keystroke("⌥b", &text("b"), "option down"),
        keystroke("⌥f", &text("f"), "option down"),
        dangerous("⌥Entrée", &code(36), "option down"),
        dangerous("Ctrl+B", &text("b"), "control down"),
        keystroke("Ctrl+R", &text("r"), "control down"),
        dangerous("Ctrl+O", &text("o"), "control down"),
        dangerous("Ctrl+C", &text("c"), "control down"),
        dangerous("Ctrl+D", &text("d"), "control down"),
        dangerous("Échap", &code(53), ""),
        keystroke("Tab", &code(48), ""),
        dangerous("Shift+Tab", &code(48), "shift down"),
        keystroke("←", &code(123), ""),
        keystroke("→", &code(124), ""),
        keystroke("↓", &code(125), ""),
        keystroke("↑", &code(126), ""),
        keystroke("é", &text("é"), ""),
        keystroke("è", &text("è"), ""),
        keystroke("à", &text("à"), ""),
        keystroke("ç", &text("ç"), ""),
        // « and » by their keys (Option+\\ and Option+Shift+\\ on a US layout): System Events types a character
        // missing from the layout as another one ("a" on the user's, 2026-10-09). What a layout puts there varies:
        // the check is that the multiplexer passes the same bytes as the terminal.
        keystroke("⌥\\ (« en US)", &code(42), "option down"),
        keystroke("⌥⇧\\ (» en US)", &code(42), "option down, shift down"),
        dangerous("Collage, 3 lignes", &text("v"), "command down"),
        dangerous("Collage, 150 Ko", &text("v"), "command down"),
    ];
    // The multiplexer's own keys, last: ⌥q ends it.
    for (name, what, using) in [("⌥1", text("1"), "option down"), ("⌥⇧→", code(124), "option down, shift down")]
    {
        let mut stroke = dangerous(name, &what, using);
        stroke.reaches_pane = false;
        list.push(stroke);
    }
    list
}

/// What the pastes put in the clipboard.
fn paste_text(name: &str) -> Option<String> {
    match name {
        "Collage, 3 lignes" => Some("une\nde\u{e9}ux\ttrois\n".to_string()),
        "Collage, 150 Ko" => Some((0..3000).map(|i| format!("{i:05} {}\n", "x".repeat(43))).collect()),
        _ => None,
    }
}

/// How a terminal is opened on a script, and the variants played.
#[derive(Clone, Copy)]
struct TerminalKind {
    name: &'static str,
    app: &'static str,
    /// Variants: as installed, then Option as Alt when the command line can set it.
    variants: &'static [&'static str],
}

const TERMINALS: &[TerminalKind] = &[
    TerminalKind { name: "ghostty", app: "/Applications/Ghostty.app", variants: &["installed", "option-as-alt"] },
    TerminalKind { name: "kitty", app: "/Applications/kitty.app", variants: &["installed", "option-as-alt"] },
    // WezTerm sends the left Option as Alt by default (`send_composed_key_when_left_alt_is_pressed = false`).
    TerminalKind { name: "wezterm", app: "/Applications/WezTerm.app", variants: &["installed"] },
    TerminalKind { name: "terminal", app: "/System/Applications/Utilities/Terminal.app", variants: &["installed"] },
    TerminalKind { name: "iterm2", app: "/Applications/iTerm.app", variants: &["installed"] },
];

/// Opens `kind` (variant `variant`) on `script`. The user's own configuration is left out where the command line
/// allows it (Ghostty, kitty, WezTerm); Terminal.app and iTerm2 open with their own settings.
fn open(kind: &TerminalKind, variant: &str, script: &Path) {
    let alt = variant == "option-as-alt";
    let script = script.to_string_lossy().into_owned();
    let mut command = match kind.name {
        "ghostty" => {
            let mut c = Command::new("open");
            // No saved window restored: a restored shell took the keys once (2026-10-09).
            c.args(["-na", kind.app, "--args", "--config-default-files=false", "--window-save-state=never"]);
            c.arg("--quit-after-last-window-closed=true");
            c.arg("--confirm-close-surface=false");
            if alt {
                c.arg("--macos-option-as-alt=left");
            }
            c.args(["-e", &script]);
            c
        }
        "kitty" => {
            let mut c = Command::new(format!("{}/Contents/MacOS/kitty", kind.app));
            c.args([
                "--config",
                "NONE",
                "-o",
                "macos_quit_when_last_window_closed=yes",
                "-o",
                "confirm_os_window_close=0",
            ]);
            if alt {
                c.args(["-o", "macos_option_as_alt=left"]);
            }
            c.arg(&script);
            c
        }
        "wezterm" => {
            let mut c = Command::new(format!("{}/Contents/MacOS/wezterm", kind.app));
            c.args(["-n", "start", "--always-new-process", "--", &script]);
            c
        }
        "iterm2" => {
            // iTerm2 runs no script it is given to open: its window runs the test's profile (see `ItermProfile`),
            // whose command is this run's script.
            let profile = serde_json::json!({ "Profiles": [{
                "Name": ITERM_PROFILE,
                "Guid": ITERM_PROFILE,
                "Custom Command": "Yes",
                "Command": format!("/bin/sh {}", shlex::try_quote(&script).expect("path")),
                "Close Sessions On End": true,
            }]});
            std::fs::write(iterm_profile_file(), profile.to_string()).expect("iTerm2 profile");
            let mut c = Command::new("open");
            // Without asking whether to check for updates: on a first launch, that dialog keeps iTerm2 from opening
            // any window until answered (seen 2026-10-09, 3.7.4). Nor whether to paste several lines: that dialog
            // took the key window from the test's (seen 2026-10-09). Arguments, for this launch only.
            c.args(["-a", kind.app, "--args", "-ApplePersistenceIgnoreState", "YES", "-SUEnableAutomaticChecks", "NO"]);
            c.args(["-NoSyncDoNotWarnBeforeMultilinePaste", "YES"]);
            c.args(["-NoSyncDoNotWarnBeforePastingOneLineEndingInNewlineAtShellPrompt", "YES"]);
            c
        }
        _ => {
            // Without the saved windows: the final SIGTERM would close the user's ones with them.
            let mut c = Command::new("open");
            c.args(["-a", kind.app, &script, "--args", "-ApplePersistenceIgnoreState", "YES"]);
            c
        }
    };
    // kitty and WezTerm are started by this process: none of its terminal's or session's marks for them.
    for key in common::pty::SCRUB {
        command.env_remove(key);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = command.spawn().expect("terminal");
    // `open` returns at once; kitty and WezTerm run until their window closes: reaped aside either way.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// iTerm2 copies a dynamic profile into its own (`New Bookmarks`) when it quits, and keeps it after the file is
/// gone (seen 2026-10-09, 3.7.4): the copy is taken out again, iTerm2 closed, through `defaults` (export, edit,
/// import; never the plist itself, which cfprefsd caches). What it did, or why it could not.
fn forget_test_profile() -> Result<String, String> {
    if Command::new("pgrep").args(["-x", "iTerm2"]).output().is_ok_and(|o| o.status.success()) {
        return Err("iTerm2 still running".into());
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let file = dir.path().join("iterm2.plist");
    let file = file.to_str().ok_or("path")?;
    let run = |program: &str, args: &[&str]| -> Result<String, String> {
        let out = Command::new(program).args(args).output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() { Ok(text) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
    };
    run("defaults", &["export", "com.googlecode.iterm2", file])?;
    let buddy = "/usr/libexec/PlistBuddy";
    let mut removed = 0;
    let mut i = 0;
    while let Ok(guid) = run(buddy, &["-c", &format!("Print ':New Bookmarks:{i}:Guid'"), file]) {
        if guid == ITERM_PROFILE {
            run(buddy, &["-c", &format!("Delete ':New Bookmarks:{i}'"), file])?;
            removed += 1;
        } else {
            i += 1;
        }
    }
    if removed == 0 {
        return Ok("aucune copie dans les profils".into());
    }
    run("defaults", &["import", "com.googlecode.iterm2", file])?;
    Ok(format!("{removed} copie(s) retirée(s) des profils"))
}

/// The name and Guid of the test's iTerm2 profile.
const ITERM_PROFILE: &str = "recruit-keys-test";

/// Where the test's iTerm2 profile lives while a run uses it (a dynamic profile, read by iTerm2 at launch).
fn iterm_profile_file() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    home.join("Library/Application Support/iTerm2/DynamicProfiles").join(format!("{ITERM_PROFILE}.json"))
}

/// iTerm2's default profile made the test's for the length of the run (decision of the user, 2026-10-09): the
/// previous `Default Bookmark Guid` noted, put back at the end, after a failure too, and checked; the profile's
/// file removed.
struct ItermProfile {
    previous: Option<String>,
    restored: bool,
}

impl ItermProfile {
    fn install() -> ItermProfile {
        let out = Command::new("defaults").args(["read", "com.googlecode.iterm2", "Default Bookmark Guid"]).output();
        let previous =
            out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        println!("iTerm2 : profil par défaut d'avant : {previous:?}");
        let dir = iterm_profile_file().parent().expect("dir").to_path_buf();
        std::fs::create_dir_all(dir).expect("DynamicProfiles");
        let status = Command::new("defaults")
            .args(["write", "com.googlecode.iterm2", "Default Bookmark Guid", "-string", ITERM_PROFILE])
            .status()
            .expect("defaults");
        assert!(status.success(), "defaults write failed");
        ItermProfile { previous, restored: false }
    }

    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = std::fs::remove_file(iterm_profile_file());
        let args: Vec<&str> = match &self.previous {
            Some(guid) => vec!["write", "com.googlecode.iterm2", "Default Bookmark Guid", "-string", guid],
            None => vec!["delete", "com.googlecode.iterm2", "Default Bookmark Guid"],
        };
        let _ = Command::new("defaults").args(&args).status();
        let copied = forget_test_profile();
        let now = Command::new("defaults").args(["read", "com.googlecode.iterm2", "Default Bookmark Guid"]).output();
        let now =
            now.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let file_gone = !iterm_profile_file().exists();
        if now == self.previous && file_gone && copied.is_ok() {
            println!(
                "iTerm2 rendu tel qu'avant : profil par défaut {now:?}, fichier du profil de test retiré, {}.",
                copied.unwrap_or_default()
            );
        } else {
            println!(
                "iTerm2 PAS rendu tel qu'avant : profil par défaut {now:?} (avant {:?}), fichier retiré : {file_gone}, copie dans les profils : {copied:?} ; à remettre : defaults write com.googlecode.iterm2 \"Default Bookmark Guid\" -string <avant>",
                self.previous
            );
        }
    }
}

impl Drop for ItermProfile {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Runs an AppleScript (`osascript -e`), or a JavaScript for Automation file with arguments; its output.
fn osascript(args: &[&str]) -> Result<String, String> {
    let output = Command::new("osascript").args(args).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn system_events(script: &str) -> Result<String, String> {
    osascript(&["-e", &format!("tell application \"System Events\" to {script}")])
}

/// The clipboard as it was: every item, every type, base64, in a JSON file.
struct Clipboard {
    saved: PathBuf,
    restored: bool,
}

const SAVE_JS: &str = r#"ObjC.import('AppKit'); ObjC.import('Foundation');
function run(argv) {
  var items = $.NSPasteboard.generalPasteboard.pasteboardItems, out = [];
  for (var i = 0; i < items.count; i++) {
    var item = items.objectAtIndex(i), types = item.types, o = {};
    for (var j = 0; j < types.count; j++) {
      var t = types.objectAtIndex(j).js, d = item.dataForType(t);
      if (d) o[t] = d.base64EncodedStringWithOptions(0).js;
    }
    out.push(o);
  }
  $.NSString.alloc.initWithUTF8String(JSON.stringify(out)).writeToFileAtomicallyEncodingError(argv[0], true, $.NSUTF8StringEncoding, null);
  return out.length;
}"#;

const RESTORE_JS: &str = r#"ObjC.import('AppKit'); ObjC.import('Foundation');
function run(argv) {
  var text = $.NSString.stringWithContentsOfFileEncodingError(argv[0], $.NSUTF8StringEncoding, null).js;
  var data = JSON.parse(text), pb = $.NSPasteboard.generalPasteboard, items = $.NSMutableArray.alloc.init;
  pb.clearContents;
  data.forEach(function (o) {
    var item = $.NSPasteboardItem.alloc.init;
    for (var t in o) item.setDataForType($.NSData.alloc.initWithBase64EncodedStringOptions(o[t], 0), t);
    items.addObject(item);
  });
  if (items.count > 0) pb.writeObjects(items);
  return items.count;
}"#;

impl Clipboard {
    fn save(dir: &Path) -> Clipboard {
        let script = dir.join("save.js");
        std::fs::write(&script, SAVE_JS).expect("save.js");
        let saved = dir.join("clipboard.json");
        let items = osascript(&["-l", "JavaScript", &script.to_string_lossy(), &saved.to_string_lossy()])
            .expect("clipboard saved");
        println!("Presse-papiers sauvé : {items} élément(s), dans {}", saved.display());
        Clipboard { saved, restored: false }
    }

    /// Puts the clipboard back; false if it could not (the saved copy is then kept on disk).
    fn restore(&mut self) -> bool {
        if self.restored {
            return true;
        }
        let script = self.saved.with_file_name("restore.js");
        let _ = std::fs::write(&script, RESTORE_JS);
        match osascript(&["-l", "JavaScript", &script.to_string_lossy(), &self.saved.to_string_lossy()]) {
            Ok(items) => {
                println!("Presse-papiers rendu : {items} élément(s).");
                self.restored = true;
                true
            }
            Err(error) => {
                println!(
                    "Presse-papiers PAS rendu ({error}) : osascript -l JavaScript {} {}",
                    script.display(),
                    self.saved.display()
                );
                false
            }
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

extern "C" fn on_sigint(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// The log's lines past `from`, and the new length.
fn read_log(path: &Path, from: usize) -> (Vec<String>, usize) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(String::from).collect();
    let len = lines.len();
    (lines.into_iter().skip(from).collect(), len)
}

/// The bytes of log lines (`<µs> <hex>`), joined.
fn bytes_of(lines: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        if let Some(hex) = line.split_whitespace().nth(1) {
            out.extend((0..hex.len()).step_by(2).filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()));
        }
    }
    out
}

/// Bytes made readable: `⎋` for Escape, `^X` for a control, the text otherwise; long ones as a length.
fn readable(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(rien)".to_string();
    }
    if bytes.len() > 40 {
        let text = String::from_utf8_lossy(bytes);
        let framed = text.starts_with("\x1b[200~") && text.ends_with("\x1b[201~");
        return format!("{} octets{}", bytes.len(), if framed { ", encadrés" } else { "" });
    }
    let mut out = String::new();
    for c in String::from_utf8_lossy(bytes).chars() {
        match c {
            '\x1b' => out.push('⎋'),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "^{}", ((c as u8) + 0x40) as char);
            }
            '\x7f' => out.push_str("^?"),
            '|' => out.push_str("\\|"),
            c => out.push(c),
        }
    }
    out
}

/// The pid of the terminal application that runs the fake `fake_pid`: up its parents, the first inside `app`.
fn terminal_pid(fake_pid: u32, app: &str) -> Option<u32> {
    let mut pid = fake_pid;
    for _ in 0..20 {
        let out = Command::new("ps").args(["-o", "ppid=,comm=", "-p", &pid.to_string()]).output().ok()?;
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let (ppid, comm) = line.split_once(char::is_whitespace)?;
        if comm.trim().starts_with(app) {
            return Some(pid);
        }
        pid = ppid.trim().parse().ok()?;
        if pid <= 1 {
            return None;
        }
    }
    None
}

/// Types `what` (an AppleScript after `tell application "System Events" to`) only if the frontmost process is
/// `pid` and its key window's title holds `title`: the check and the key in one script, so that no window can come
/// to the front between the two.
fn guarded(pid: u32, title: &str, what: &str) -> Result<(), String> {
    let title = title.replace('"', "");
    let script = format!(
        r#"tell application "System Events"
  set p to first process whose frontmost is true
  if (unix id of p) is not {pid} then return "front " & (unix id of p)
  set n to name of front window of p
  if n does not contain "{title}" then return "window " & n
  {what}
  return "typed"
end tell"#
    );
    answer(osascript(&["-e", &script]))
}

/// Brings the application `pid` to the front by its pid (AppKit, not System Events: `set frontmost` left the
/// user's own Ghostty in front once, 2026-10-09), then waits, 3 s at most, until its key window is the test's;
/// nothing is typed meanwhile.
fn activate(pid: u32, title: &str) -> Result<(), String> {
    const ACTIVATE: &str = r#"ObjC.import('AppKit');
function run(argv) {
  var app = $.NSRunningApplication.runningApplicationWithProcessIdentifier(parseInt(argv[0]));
  if (!app || app.isNil()) return "none";
  return app.activateWithOptions($.NSApplicationActivateAllWindows) ? "asked" : "refused";
}"#;
    let asked = osascript(&["-l", "JavaScript", "-e", ACTIVATE, &pid.to_string()]);
    let title = title.replace('"', "");
    let check = format!(
        r#"tell application "System Events"
  set p to first process whose frontmost is true
  if (unix id of p) is not {pid} then return "front " & (unix id of p)
  set n to name of front window of p
  if n does not contain "{title}" then return "window " & n
  return "typed"
end tell"#
    );
    // `KEYS_WAIT` seconds (3 by default) for it to come to the front; macOS (14 and later) may refuse the activation
    // while the user's own window is active: then the user clicks the test's window, told so here.
    let wait = common::env_usize("KEYS_WAIT", 3) as u64;
    if wait > 3 {
        println!(">>> Clique la fenêtre « {title} » (KEYLOG READY), puis ne touche plus à rien ; {wait} s au plus.");
    }
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        match answer(osascript(&["-e", &check])) {
            Ok(()) => return Ok(()),
            Err(why) if Instant::now() >= deadline => return Err(format!("{why} (activation : {asked:?})")),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// What the guarded scripts answer: `typed` when the check passed.
fn answer(out: Result<String, String>) -> Result<(), String> {
    match out {
        Ok(answer) if answer == "typed" => Ok(()),
        Ok(answer) => Err(match answer.split_once(' ') {
            Some(("front", other)) => format!("l'application au premier plan n'est plus la sienne (pid {other})"),
            Some(("window", name)) => format!("la fenêtre clé n'est pas la sienne (« {name} »)"),
            _ => format!("réponse inattendue : {answer}"),
        }),
        Err(error) => Err(format!("System Events : {error}")),
    }
}

/// Types `z` (guarded, see `guarded`) and waits for the fake to log it, 500 ms at most.
fn witness(pid: u32, title: &str, log: &Path, seen: &mut usize) -> Result<(), String> {
    guarded(pid, title, "keystroke \"z\"")?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        std::thread::sleep(Duration::from_millis(25));
        let (new, len) = read_log(log, *seen);
        if !new.is_empty() {
            *seen = len;
            let bytes = bytes_of(&new);
            // `z`, or its kitty form when the fake asked for the protocol (`CSI 122 u`).
            return if bytes == b"z" || bytes == b"\x1b[122u" {
                Ok(())
            } else {
                Err(format!("reçue autrement : {}", readable(&bytes)))
            };
        }
    }
    Err("pas reçue en 500 ms".to_string())
}

/// For an application that runs as a single instance (Terminal.app, iTerm2), the pid of one already running.
fn single_instance(kind: &TerminalKind) -> Option<u32> {
    let name = match kind.name {
        "terminal" => "Terminal",
        "iterm2" => "iTerm2",
        _ => return None,
    };
    let out = Command::new("pgrep").args(["-x", name]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse().ok()
}

/// The pids of the processes of the application `app` (their executable inside its bundle), and for iTerm2 its
/// session servers (`iTermServer-<version>`, outside the bundle): they outlive the application, keep the session
/// running, and would have it restored at the next launch (seen 2026-10-09).
fn app_pids(app: &str) -> Vec<u32> {
    let mut prefixes = vec![app.to_string()];
    if app.ends_with("/iTerm.app")
        && let Some(home) = std::env::var_os("HOME")
    {
        prefixes.push(format!("{}/Library/Application Support/iTerm2/iTermServer", home.to_string_lossy()));
    }
    let out = Command::new("ps").args(["-ax", "-o", "pid=,comm="]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (pid, comm) = l.trim().split_once(char::is_whitespace)?;
            let comm = comm.trim();
            prefixes.iter().any(|p| comm.starts_with(p.as_str())).then(|| pid.parse().ok()).flatten()
        })
        .collect()
}

/// The processes of an application started by one launch of the test: those that were not there before it. Ended
/// when dropped (SIGTERM, then SIGKILL after 3 s for any left), each checked to still be that application's.
struct Launched {
    app: &'static str,
    before: Vec<u32>,
}

impl Launched {
    fn new(app: &'static str) -> Launched {
        Launched { app, before: app_pids(app) }
    }

    fn ours(&self) -> Vec<u32> {
        app_pids(self.app).into_iter().filter(|p| !self.before.contains(p)).collect()
    }
}

impl Drop for Launched {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_millis(500));
        for pid in self.ours() {
            // SAFETY: a plain signal to a process this launch started (checked above to be the application's).
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.ours().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        for pid in self.ours() {
            println!("{} (pid {pid}) still there after SIGTERM: SIGKILL", self.app);
            // SAFETY: as above.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
    }
}

/// Whether `pid` is still a process of the application `app` (and not a reused pid).
fn still_ours(pid: u32, app: &str) -> bool {
    Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim().starts_with(app))
}

/// What one window got for each stroke.
type Got = Vec<Option<Vec<u8>>>;

/// One window: `kind` on the fake (`through_mux`: inside a copy of `recruit _mux`), the strokes typed, what the
/// pane got for each (`None`: not typed, the frontmost window was not the terminal's).
fn play(kind: &TerminalKind, variant: &str, through_mux: bool, modes: &str, dir: &Path, recruit: &Path) -> Got {
    let run = dir.join(format!("{}-{variant}-{}", kind.name, if through_mux { "mux" } else { "direct" }));
    std::fs::create_dir_all(&run).expect("run dir");
    let log = run.join("keys.log");
    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_file(run.join("client.pid"));
    // A title no other window has: the keys go only while the key window bears it.
    let title = format!("recruit-keys-{}-{}", std::process::id(), run.file_name().expect("name").to_string_lossy());
    let vars = [
        ("KEYLOG_FILE", log.to_string_lossy().into_owned()),
        ("KEYLOG_MODES", modes.to_string()),
        ("KEYLOG_PROBE", "1".to_string()),
        ("KEYLOG_TITLE", title.clone()),
    ];
    let fake = common::child_words("keylog", &vars);
    let fake = shlex::try_join(fake.iter().map(|w| w.to_str().expect("UTF-8"))).expect("command");
    let mut script = String::from("#!/bin/sh\n");
    // The native server's socket needs a short folder (103 bytes), removed with this guard.
    let short = tempfile::Builder::new().prefix("rt.").tempdir_in("/tmp").expect("short run dir");
    let quote = |p: &Path| shlex::try_quote(&p.to_string_lossy()).expect("path").into_owned();
    if through_mux {
        // The real thing: a native server of the test's own, the fake in its one pane, the real client attached.
        // The server stops once its last pane has ended (the fake, killed at the end).
        let state = run.join("state");
        let _ = writeln!(
            script,
            "export XDG_CONFIG_HOME={0}/config XDG_CACHE_HOME={0}/cache RECRUIT_TMPDIR={1}",
            quote(&run),
            quote(short.path())
        );
        let _ = writeln!(script, "{} --lang fr _server {} || exit 1", quote(recruit), quote(&state));
        let _ = writeln!(script, "{} _ctl {} spawn keylog -- {fake} || exit 1", quote(recruit), quote(&state));
        // The client's pid (the shell's, kept by `exec`): the fake runs under the server, not under the terminal.
        let _ = writeln!(script, "echo $$ > {}", quote(&run.join("client.pid")));
        let _ = writeln!(script, "exec {} _ctl {} attach", quote(recruit), quote(&state));
    } else {
        let _ = writeln!(script, "exec {fake}");
    }
    let path = run.join("run.command");
    std::fs::write(&path, script).expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    // Every process of the application that this launch adds is ended when this window is done, whatever way
    // it ends (`Launched`).
    let _launched = Launched::new(kind.app);
    open(kind, variant, &path);

    // The fake says it is up: its first log line.
    let deadline = Instant::now() + Duration::from_secs(20);
    let fake_pid = loop {
        let (lines, _) = read_log(&log, 0);
        if let Some(pid) = lines.first().and_then(|l| l.strip_prefix("ready ")).and_then(|p| p.parse::<u32>().ok()) {
            break Some(pid);
        }
        if Instant::now() > deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let strokes = strokes();
    let Some(fake_pid) = fake_pid else {
        println!("{} ({variant}) : le factice n'a pas démarré", kind.name);
        return vec![None; strokes.len()];
    };
    // Up from the fake, or from the native client when there is one.
    let below =
        std::fs::read_to_string(run.join("client.pid")).ok().and_then(|p| p.trim().parse().ok()).unwrap_or(fake_pid);
    let Some(app_pid) = terminal_pid(below, kind.app) else {
        println!("{} ({variant}) : application introuvable au-dessus du pid {below}", kind.name);
        return vec![None; strokes.len()];
    };
    // A guard that fails stops this window only (the next terminal is played); Ctrl-C (`STOP`) stops everything.
    let mut halted = false;
    if let Err(why) = activate(app_pid, &title) {
        println!("{} ({variant}) : la fenêtre de test n'est pas venue au premier plan : {why} ; rien tapé", kind.name);
        halted = true;
    }
    let allow_dangerous = std::env::var("KEYS_DANGEROUS").as_deref() == Ok("1");
    let mut got = Vec::new();
    let mut seen = 1;
    let mut witnessed = false;
    for stroke in &strokes {
        if STOP.load(Ordering::SeqCst) || halted || (stroke.dangerous && !allow_dangerous) {
            got.push(None);
            continue;
        }
        if stroke.script.is_empty() {
            // The probe: what came in since the fake started.
            std::thread::sleep(Duration::from_millis(800));
            let (new, len) = read_log(&log, seen);
            seen = len;
            got.push(Some(bytes_of(&new)));
            continue;
        }
        // A harmless witness key first, and again before each dangerous one: the fake must have it within
        // 500 ms, or nothing more is typed.
        if !witnessed || stroke.dangerous {
            match witness(app_pid, &title, &log, &mut seen) {
                Ok(()) => witnessed = true,
                Err(why) => {
                    println!("{} ({variant}) : touche témoin {why} ; arrêt avant « {} »", kind.name, stroke.name);
                    halted = true;
                    got.push(None);
                    continue;
                }
            }
        }
        if let Some(text) = paste_text(stroke.name) {
            let mut pbcopy = Command::new("pbcopy").stdin(Stdio::piped()).spawn().expect("pbcopy");
            std::io::Write::write_all(pbcopy.stdin.as_mut().expect("stdin"), text.as_bytes()).expect("pbcopy");
            let _ = pbcopy.wait();
        }
        if let Err(why) = guarded(app_pid, &title, &stroke.script) {
            println!("{} ({variant}) : {why} ; « {} » pas tapée, arrêt", kind.name, stroke.name);
            halted = true;
            got.push(None);
            continue;
        }
        // Until the pane has been quiet for 400 ms (a big paste comes in pieces), 3 s at most.
        let start = Instant::now();
        let mut quiet_since = Instant::now();
        let mut lines = Vec::new();
        while start.elapsed() < Duration::from_secs(3) && quiet_since.elapsed() < Duration::from_millis(400) {
            std::thread::sleep(Duration::from_millis(50));
            let (new, len) = read_log(&log, seen);
            if !new.is_empty() {
                quiet_since = Instant::now();
                lines.extend(new);
                seen = len;
            }
        }
        got.push(Some(bytes_of(&lines)));
    }
    // The fake (and the multiplexer above it) end, then the terminal this test opened, by its pid, after
    // checking that the pid is still that terminal (a pid can be reused).
    // SAFETY: plain signals to processes this test started.
    unsafe { libc::kill(fake_pid as libc::pid_t, libc::SIGTERM) };
    std::thread::sleep(Duration::from_millis(500));
    if still_ours(app_pid, kind.app) {
        unsafe { libc::kill(app_pid as libc::pid_t, libc::SIGTERM) };
    }
    got
}

/// Real keystrokes, direct and through the multiplexer, in each terminal (`KEYS_TERMINALS` to choose them),
/// with the fake asking for nothing, then for what Claude Code asks for (`KEYS_MODES=plain|claude|both`).
#[test]
#[ignore = "real keystrokes: needs Accessibility and Automation, nobody typing; see the module's doc"]
fn real_keys() {
    // A lock `--ignored` alone does not open (it ran by mistake once, 2026-10-09): real keystrokes go only with
    // `KEYS_CONSENT` set to today's date, given with the user's go for that day.
    if !common::consented("KEYS_CONSENT", "real keystrokes go to the frontmost window", "real_keys") {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let recruit = common::recruit_copy(dir.path());
    // SAFETY: the handler only stores to an atomic.
    unsafe { libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t) };
    assert_eq!(system_events("get UI elements enabled").as_deref(), Ok("true"), "Accessibility not granted");
    let mut clipboard = Clipboard::save(dir.path());
    let chosen = std::env::var("KEYS_TERMINALS").unwrap_or_default();
    // iTerm2's profile, only if iTerm2 is played and not running (it is skipped then).
    let iterm = TERMINALS.iter().find(|k| k.name == "iterm2").expect("iterm2");
    let mut iterm_profile = (chosen.is_empty() || chosen.split(',').any(|c| c == "iterm2"))
        .then(|| single_instance(iterm).is_none().then(ItermProfile::install))
        .flatten();
    let modes: Vec<&str> = match std::env::var("KEYS_MODES").as_deref() {
        Ok("plain") => vec!["plain"],
        Ok("claude") => vec!["claude"],
        _ => vec!["plain", "claude"],
    };
    let strokes = strokes();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for kind in TERMINALS.iter().filter(|k| chosen.is_empty() || chosen.split(',').any(|c| c == k.name)) {
            // Terminal.app and iTerm2 run as one instance: one already open may hold the user's windows.
            if let Some(process) = single_instance(kind) {
                println!(
                    "{} sauté : l'application tourne déjà (pid {process}), elle peut porter des fenêtres de l'utilisateur",
                    kind.name
                );
                continue;
            }
            let variants = std::env::var("KEYS_VARIANTS").unwrap_or_default();
            for variant in kind.variants.iter().filter(|v| variants.is_empty() || variants.split(',').any(|w| w == **v))
            {
                for mode in &modes {
                    if STOP.load(Ordering::SeqCst) {
                        return;
                    }
                    let direct = play(kind, variant, false, mode, dir.path(), &recruit);
                    let mux = play(kind, variant, true, mode, dir.path(), &recruit);
                    println!();
                    println!("### {} ({variant}), factice {mode}", kind.name);
                    println!();
                    println!("| Touche | Direct | Via le mux | Verdict |");
                    println!("|---|---|---|---|");
                    for (i, stroke) in strokes.iter().enumerate() {
                        let (d, m) = (&direct[i], &mux[i]);
                        let verdict = match (d, m) {
                            (None, None) if stroke.dangerous => "non passé (dangereuse, KEYS_DANGEROUS=1)",
                            (None, _) | (_, None) => "non passé",
                            (Some(_), Some(m)) if !stroke.reaches_pane => {
                                if m.is_empty() {
                                    "ok (pris par le mux)"
                                } else {
                                    "ko : arrivé au panneau"
                                }
                            }
                            (Some(d), Some(m)) if d == m => "ok",
                            (Some(d), Some(_)) if d.is_empty() => "terminal : rien envoyé",
                            _ => "diffère",
                        };
                        let show = |b: &Option<Vec<u8>>| b.as_deref().map_or("—".to_string(), readable);
                        println!("| {} | {} | {} | {verdict} |", stroke.name, show(d), show(m));
                    }
                }
            }
        }
    }));
    if let Some(profile) = &mut iterm_profile {
        profile.restore();
    }
    if !clipboard.restore() {
        // The saved copy stays on disk, with the commands that put it back printed above.
        let kept = dir.keep();
        println!("Dossier du passage gardé : {}", kept.display());
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// The layer-0 windows on screen or not: `pid id title` per line (CGWindowListCopyWindowInfo, read only).
const WINDOWS_JS: &str = r#"ObjC.import("CoreGraphics"); ObjC.import("Foundation");
var a = ObjC.castRefToObject($.CGWindowListCopyWindowInfo($.kCGWindowListOptionAll, 0));
var out = [];
for (var i = 0; i < a.count; i++) {
  var d = a.objectAtIndex(i);
  if (ObjC.unwrap(d.objectForKey("kCGWindowLayer")) != 0) continue;
  out.push(ObjC.unwrap(d.objectForKey("kCGWindowOwnerPID")) + " " + ObjC.unwrap(d.objectForKey("kCGWindowNumber"))
    + " " + (ObjC.unwrap(d.objectForKey("kCGWindowName")) || ""));
}
out.join("\n")"#;

/// The window of one of `pids` whose title holds `title`, or else their only window: its number.
fn window_of(pids: &[u32], title: &str) -> Option<u32> {
    let list = osascript(&["-l", "JavaScript", "-e", WINDOWS_JS]).ok()?;
    let ours: Vec<(u32, String)> = list
        .lines()
        .filter_map(|l| {
            let mut words = l.splitn(3, ' ');
            let pid: u32 = words.next()?.parse().ok()?;
            let id: u32 = words.next()?.parse().ok()?;
            pids.contains(&pid).then(|| (id, words.next().unwrap_or_default().to_string()))
        })
        .collect();
    ours.iter()
        .find(|(_, name)| name.contains(title))
        .or(if ours.len() == 1 { ours.first() } else { None })
        .map(|w| w.0)
}

/// How many different byte values the pixels of `png` take (converted to BMP by `sips`): 1 or 2 for an image all
/// black, or all one color (no right to record the screen, or nothing drawn yet).
fn colors_in(png: &Path) -> Result<usize, String> {
    let bmp = png.with_extension("bmp");
    let out = Command::new("sips")
        .args(["-s", "format", "bmp"])
        .arg(png)
        .arg("--out")
        .arg(&bmp)
        .stdout(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let bytes = std::fs::read(&bmp).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&bmp);
    let start = bytes.get(10..14).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or("BMP")?;
    let mut seen = [false; 256];
    for b in bytes.get(start..).unwrap_or_default() {
        seen[*b as usize] = true;
    }
    Ok(seen.iter().filter(|s| **s).count())
}

/// A file shown in each terminal and its window captured (`screencapture -l`), for what only a real terminal
/// draws (dev-rendu's ECH, 2026-10-09). No key typed; each window opened on `cat <file>; sleep`, found by its
/// title, captured, then closed by its pid; iTerm2 through its temporary profile, put back after. Nothing at all
/// without `SHOTS_CONSENT=<today's date>`, given with the user's go (the windows show on their screen, and some
/// come to the front), and `window_shots` named:
///
/// ```sh
/// SHOTS_CONSENT=$(date +%F) SHOTS_FILE=<file> SHOTS_OUT=<folder> cargo test --test mux_keys -- --ignored --nocapture --test-threads=1 window_shots
/// ```
///
/// `SHOTS_TERMINALS` (comma-separated, all by default), `SHOTS_SECS` (how long each window stays, 8 by default).
/// A capture all of one color (no right to record the screen) stops the run: nothing is worked around.
#[test]
#[ignore = "opens windows on the user's screen: needs SHOTS_CONSENT, see its doc"]
fn window_shots() {
    if !common::consented("SHOTS_CONSENT", "windows open on the user's screen", "window_shots") {
        return;
    }
    let file = PathBuf::from(std::env::var("SHOTS_FILE").expect("SHOTS_FILE: the file to show"));
    let out = PathBuf::from(std::env::var("SHOTS_OUT").expect("SHOTS_OUT: where the captures go"));
    std::fs::create_dir_all(&out).expect("SHOTS_OUT");
    let secs = common::env_usize("SHOTS_SECS", 8);
    let dir = tempfile::tempdir().expect("temp dir");
    let chosen = std::env::var("SHOTS_TERMINALS").unwrap_or_default();
    let wanted = |name: &str| chosen.is_empty() || chosen.split(',').any(|c| c == name);
    let iterm = TERMINALS.iter().find(|k| k.name == "iterm2").expect("iterm2");
    let mut iterm_profile =
        wanted("iterm2").then(|| single_instance(iterm).is_none().then(ItermProfile::install)).flatten();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for kind in TERMINALS.iter().filter(|k| wanted(k.name)) {
            if let Some(process) = single_instance(kind) {
                println!("{} sauté : l'application tourne déjà (pid {process})", kind.name);
                continue;
            }
            let title = format!("recruit-shots-{}", kind.name);
            let script = dir.path().join(format!("{}.command", kind.name));
            let body = format!(
                "#!/bin/sh\nprintf '\\033]2;{title}\\007'\ncat {}\nsleep {secs}\n",
                shlex::try_quote(&file.to_string_lossy()).expect("path")
            );
            std::fs::write(&script, body).expect("script");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            let launched = Launched::new(kind.app);
            open(kind, "installed", &script);
            let deadline = Instant::now() + Duration::from_secs(15);
            let window = loop {
                if let Some(id) = window_of(&launched.ours(), &title) {
                    break Some(id);
                }
                if Instant::now() > deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(250));
            };
            let Some(window) = window else {
                println!("{} : fenêtre introuvable en 15 s, rien capturé", kind.name);
                continue;
            };
            // Drawn, and the title given.
            std::thread::sleep(Duration::from_millis(1500));
            let png = out.join(format!("{}.png", kind.name));
            let status = Command::new("screencapture").args(["-x", "-o", &format!("-l{window}")]).arg(&png).status();
            drop(launched);
            match (status, colors_in(&png)) {
                (Ok(s), Ok(colors)) if s.success() && colors > 2 => {
                    println!("{} : {} ({colors} valeurs d'octet)", kind.name, png.display())
                }
                (status, colors) => {
                    println!(
                        "{} : capture vide ou refusée ({status:?}, {colors:?}) : arrêt, sans rien contourner (droit d'enregistrer l'écran ?)",
                        kind.name
                    );
                    return;
                }
            }
        }
    }));
    if let Some(profile) = &mut iterm_profile {
        profile.restore();
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
