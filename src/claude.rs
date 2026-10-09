// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Everything recruit asks of Claude Code: where it is, which sessions run, the last conversation of each
//! name, and the command line of a member.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::{Member, Team, home};
use crate::t;

/// The executable, looked up in the PATH unless it is a path.
pub fn find_executable(command: &str) -> Option<PathBuf> {
    let command = crate::config::expand_home(command);
    if command.components().count() > 1 {
        return command.is_file().then_some(command);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(&command)).find(|p| p.is_file())
}

pub fn require(command: &str) -> Result<PathBuf> {
    find_executable(command).with_context(|| {
        t!(
            "« {} » est introuvable : Claude Code est-il installé ? (https://claude.com/claude-code)",
            "\"{}\" not found: is Claude Code installed? (https://claude.com/claude-code)",
            command
        )
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct Running {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// `busy`, `idle` or `waiting` (for the user, in its terminal).
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
}

/// The variable that picks a Claude Code profile: its configuration folder.
pub const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";

/// `claude`, in the profile `config_dir` when the settings name one.
pub fn command(claude: &Path, config_dir: Option<&Path>) -> Command {
    let mut command = Command::new(claude);
    if let Some(dir) = config_dir {
        command.env(CONFIG_DIR, dir);
    }
    command
}

/// The profile's folder: the settings' one, else the environment's. None: Claude Code's default.
fn profile(config_dir: Option<&Path>) -> Option<PathBuf> {
    config_dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os(CONFIG_DIR).filter(|d| !d.is_empty()).map(PathBuf::from))
}

/// Claude sessions open on this machine (`claude agents --json`).
pub fn running(claude: &Path, config_dir: Option<&Path>) -> Result<Vec<Running>> {
    let output = command(claude, config_dir).args(["agents", "--json"]).output()?;
    if !output.status.success() {
        bail!("claude agents --json: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// What a session's own file in the profile says of it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionFile {
    /// `busy`, `idle`, `shell` while a command it started runs past its turn…
    pub status: String,
    /// Since when, in seconds since the epoch, when it says it in a way read here.
    pub since: Option<i64>,
}

/// A session's own file in the profile (`sessions/<pid>.json`); None when missing or without a status. Its time
/// (`statusUpdatedAt`, in milliseconds, whole or not) may be missing: the status stands on its own.
pub fn session_status(config_dir: Option<&Path>, pid: u32) -> Option<SessionFile> {
    let file = profile_dir(config_dir).join("sessions").join(format!("{pid}.json"));
    let session: serde_json::Value = serde_json::from_str(&fs::read_to_string(file).ok()?).ok()?;
    let since =
        session["statusUpdatedAt"].as_f64().filter(|ms| ms.is_finite() && *ms > 0.0).map(|ms| (ms / 1000.0) as i64);
    Some(SessionFile { status: session["status"].as_str()?.to_string(), since })
}

/// The exact address ListAgents gives a session (`name [ref]`), from its own file in the profile: the first six hex
/// digits of SHA-256 over `session:` and its messaging socket's path (Claude Code 2.1.294). None when the file is
/// missing or says no socket: then no address, and nothing else changes.
pub fn session_ref(config_dir: Option<&Path>, pid: u32) -> Option<String> {
    let file = profile_dir(config_dir).join("sessions").join(format!("{pid}.json"));
    let session: serde_json::Value = serde_json::from_str(&fs::read_to_string(file).ok()?).ok()?;
    let socket = session["messagingSocketPath"].as_str().filter(|s| !s.is_empty())?;
    Some(ref_of(socket))
}

fn ref_of(socket: &str) -> String {
    sha256(format!("session:{socket}").as_bytes())[..3].iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 (FIPS 180-4), for the few bytes of a session's address: no crate for that.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (state, add) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *state = state.wrapping_add(add);
        }
    }
    let mut out = [0u8; 32];
    for (bytes, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *bytes = word.to_be_bytes();
    }
    out
}

/// The exact addresses of the team's `members` whose name more than one open session carries (another team, a
/// session opened by hand): member → ref. Each member's own session is the one `noted` for it (`recruit _member`
/// notes its session id), else the only one under its name in the team's folder `dir`. A member without a session
/// found, or without a ref (`reference`, from its session's file), is left out.
pub fn addresses(
    dir: &Path,
    members: &[&str],
    running: &[Running],
    noted: impl Fn(&str) -> Option<String>,
    reference: impl Fn(u32) -> Option<String>,
) -> std::collections::BTreeMap<String, String> {
    let carried = |name: &str| running.iter().filter(|r| r.name.as_deref() == Some(name)).count();
    members
        .iter()
        .filter(|name| carried(name) > 1)
        .filter_map(|&name| {
            let id = noted(name).filter(|id| !id.is_empty());
            let by_id = id.and_then(|id| running.iter().find(|r| r.session_id.as_deref() == Some(id.as_str())));
            let mut here = running.iter().filter(|r| {
                r.name.as_deref() == Some(name) && r.cwd.as_deref().is_some_and(|cwd| Path::new(cwd) == dir)
            });
            let only_here = match (here.next(), here.next()) {
                (Some(one), None) => Some(one),
                _ => None,
            };
            let session = by_id.or(only_here)?;
            Some((name.to_string(), reference(session.pid?)?))
        })
        .collect()
}

/// Whether Claude Code trusts this folder, as recorded in its global configuration: the folder or one of its
/// parents approved. None when the configuration cannot be read.
pub fn trusted(dir: &Path, config_dir: Option<&Path>) -> Option<bool> {
    let file = profile(config_dir).unwrap_or_else(home).join(".claude.json");
    let config: serde_json::Value = serde_json::from_slice(&fs::read(file).ok()?).ok()?;
    let projects = config.get("projects")?.as_object()?;
    Some(dir.ancestors().any(|d| {
        projects
            .get(d.to_string_lossy().as_ref())
            .and_then(|p| p.get("hasTrustDialogAccepted"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    }))
}

/// Where the profile keeps its conversations, among others.
fn profile_dir(config_dir: Option<&Path>) -> PathBuf {
    profile(config_dir).unwrap_or_else(|| home().join(".claude"))
}

/// The folder name Claude Code gives a project: every character that is not an ASCII letter or digit becomes a
/// dash, one per UTF-16 unit as in JavaScript.
pub fn project_key(dir: &Path) -> String {
    let mut key = String::new();
    for c in dir.to_string_lossy().chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c);
        } else {
            key.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    key
}

/// The conversation file of a session.
pub fn transcript(dir: &Path, config_dir: Option<&Path>, session_id: &str) -> PathBuf {
    profile_dir(config_dir).join("projects").join(project_key(dir)).join(format!("{session_id}.jsonl"))
}

/// For each session name, the most recent conversation that carries it in this directory: the last title given
/// in each conversation file, newest file first.
pub fn last_conversations(dir: &Path, config_dir: Option<&Path>) -> Result<HashMap<String, String>> {
    let folder = profile_dir(config_dir).join("projects").join(project_key(dir));
    let Ok(entries) = fs::read_dir(&folder) else { return Ok(HashMap::new()) };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));

    let mut found = HashMap::new();
    for (_, file) in files {
        let Some(title) = last_title(&file) else { continue };
        let Some(id) = file.file_stem().and_then(|s| s.to_str()) else { continue };
        found.entry(title).or_insert_with(|| id.to_string());
    }
    Ok(found)
}

fn last_title(file: &Path) -> Option<String> {
    let handle = fs::File::open(file).ok()?;
    if handle.metadata().ok()?.len() == 0 {
        return None;
    }
    // SAFETY: read-only map of a file Claude Code only appends to; a concurrent append does not touch the
    // bytes already mapped.
    let map = unsafe { memmap2::Mmap::map(&handle) }.ok()?;
    let at = memchr::memmem::rfind(&map, br#""type":"custom-title""#)?;
    let start = map[..at].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let end = memchr::memchr(b'\n', &map[at..]).map_or(map.len(), |i| at + i);
    let line: serde_json::Value = serde_json::from_slice(&map[start..end]).ok()?;
    line.get("customTitle")?.as_str().map(String::from)
}

/// The settings of a member other than the main contact, for its session only: a status line that prints nothing,
/// in place of the user's. The user's own settings stay as they are.
const QUIET: &str = r#"{"statusLine":{"type":"command","command":"true"}}"#;

/// Whether the settings of a session started in `dir` give a status line: the profile's, the project's, the local
/// ones (in `dir`, or at the root of its git repository). Claude Code keeps a row for any status line, even one
/// that prints nothing, in fullscreen (2.1.292), and `--settings` cannot take it away: QUIET only replaces one.
/// Managed settings are left out on purpose: theirs wins over `--settings`, QUIET could not replace it. A
/// `--setting-sources` in the arguments may leave the project's out: QUIET then adds an empty row (rare).
pub fn status_line(dir: &Path, config_dir: Option<&Path>) -> bool {
    let root = git_root(dir).unwrap_or_else(|| dir.to_path_buf());
    let files = [
        profile_dir(config_dir).join("settings.json"),
        dir.join(".claude/settings.json"),
        dir.join(".claude/settings.local.json"),
        root.join(".claude/settings.local.json"),
    ];
    files.iter().any(|file| {
        // A file that cannot be read gives none.
        let Ok(text) = fs::read(file) else { return false };
        // Not valid JSON: as if it gave one, a row too many rather than the user's status line.
        serde_json::from_slice::<serde_json::Value>(&text).map_or(true, |v| !v["statusLine"].is_null())
    })
}

/// The root of `dir`'s git repository, as Claude Code sees it: a worktree's is its main repository's.
fn git_root(dir: &Path) -> Option<PathBuf> {
    let root = dir.ancestors().find(|d| d.join(".git").exists())?;
    Some(main_repository(root).unwrap_or_else(|| root.to_path_buf()))
}

/// For a worktree, whose `.git` is a file (`gitdir: <its git directory>`): the main repository, as Claude Code
/// (2.1.293) finds it. The worktree's git directory must be `<common>/worktrees/<name>`, with a `gitdir` file
/// naming the worktree's `.git` back, and the common directory (its `commondir` file) must be a `.git`: a bare
/// repository's worktrees stay their own root. None otherwise.
fn main_repository(worktree: &Path) -> Option<PathBuf> {
    let link = fs::read_to_string(worktree.join(".git")).ok()?;
    let git_dir = worktree.join(link.strip_prefix("gitdir:")?.trim()).canonicalize().ok()?;
    let named = |file: &str| -> Option<PathBuf> {
        git_dir.join(fs::read_to_string(git_dir.join(file)).ok()?.trim()).canonicalize().ok()
    };
    let common = named("commondir")?;
    if git_dir.parent()? != common.join("worktrees")
        || named("gitdir")? != worktree.join(".git").canonicalize().ok()?
        || common.file_name()? != ".git"
    {
        return None;
    }
    Some(common.parent()?.to_path_buf())
}

/// Whether a member's command line puts QUIET in place of the user's status line: every member but the main contact,
/// unless the user's arguments give settings of their own (two `--settings` would fight); and only when the settings
/// give a status line (`status_line`): without one, QUIET would add the row of its own.
pub fn quiet_settings(status_line: bool, team: &Team, name: &str, member: &Member) -> bool {
    let own = |args: &[String]| args.iter().any(|a| a == "--settings" || a.starts_with("--settings="));
    status_line && team.quiet(name) && !own(&team.args) && !own(&member.args)
}

/// Whether a command line puts QUIET in place of the user's status line.
pub fn quieted(argv: &[String]) -> bool {
    argv.iter().any(|a| a == QUIET)
}

/// The value `flag` takes on a command line, `--flag value` or `--flag=value`: the last one, as Claude Code reads it.
pub fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let mut found = None;
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        if arg == flag {
            found = args.next().map(String::as_str);
        } else if let Some(value) = arg.strip_prefix(flag).and_then(|rest| rest.strip_prefix('=')) {
            found = Some(value);
        }
    }
    found
}

/// What the command lines of one launch's members share.
pub struct Launch<'a> {
    pub claude: &'a Path,
    pub team: &'a Team,
    /// The mod's folder, when Claude Code can load it.
    pub plugin_dir: Option<&'a Path>,
    /// The members' settings give a status line (see [`status_line`]).
    pub status_line: bool,
}

impl Launch<'_> {
    /// `claude [--plugin-dir <mod>] [--permission-mode …] [--model …] [--effort …] [--settings <quiet>] [args…]
    /// -n <name> (-r <id> | --append-system-prompt-file <file>)`. A resumed conversation keeps the prompt it started
    /// with, so none is passed again.
    pub fn argv(&self, name: &str, member: &Member, prompt: &Path, resume: Option<&str>) -> Vec<String> {
        let team = self.team;
        let mut argv = vec![self.claude.to_string_lossy().into_owned()];
        if let Some(dir) = self.plugin_dir {
            argv.extend(["--plugin-dir".to_string(), dir.to_string_lossy().into_owned()]);
        }
        let pick = |m: &Option<String>, t: &Option<String>| m.clone().or_else(|| t.clone());
        for (flag, value) in [
            ("--permission-mode", pick(&member.permission_mode, &team.permission_mode)),
            ("--model", pick(&member.model, &team.model)),
            ("--effort", pick(&member.effort, &team.effort)),
        ] {
            if let Some(value) = value {
                argv.extend([flag.to_string(), value]);
            }
        }
        if quiet_settings(self.status_line, team, name, member) {
            argv.extend(["--settings".to_string(), QUIET.to_string()]);
        }
        argv.extend(team.args.iter().cloned());
        argv.extend(member.args.iter().cloned());
        argv.extend(["-n".to_string(), name.to_string()]);
        match resume {
            Some(id) => argv.extend(["-r".to_string(), id.to_string()]),
            None => argv.extend(["--append-system-prompt-file".to_string(), prompt.to_string_lossy().into_owned()]),
        }
        argv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha256_as_published() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // Two blocks: the length no longer fits after the message.
        assert_eq!(
            hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(hex(&sha256(&[b'a'; 1000])), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }

    /// As ListAgents showed them (Claude Code 2.1.294).
    #[test]
    fn refs_as_list_agents_shows_them() {
        assert_eq!(ref_of("/tmp/cc-socks/72947.sock"), "551272");
        assert_eq!(ref_of("/tmp/cc-socks/72848.sock"), "8dd997");
        assert_eq!(ref_of("/tmp/cc-socks/72860.sock"), "3779a7");
    }

    #[test]
    fn session_ref_from_its_file() {
        let profile = tempfile::tempdir().unwrap();
        let sessions = profile.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("72947.json"), r#"{"pid":72947,"messagingSocketPath":"/tmp/cc-socks/72947.sock"}"#)
            .unwrap();
        assert_eq!(session_ref(Some(profile.path()), 72947).as_deref(), Some("551272"));
        // Missing, empty, or another format: no address.
        assert_eq!(session_ref(Some(profile.path()), 1), None);
        fs::write(sessions.join("2.json"), r#"{"pid":2,"messagingSocketPath":""}"#).unwrap();
        assert_eq!(session_ref(Some(profile.path()), 2), None);
        fs::write(sessions.join("3.json"), r#"{"pid":3,"messaging":{"socket":"/tmp/x.sock"}}"#).unwrap();
        assert_eq!(session_ref(Some(profile.path()), 3), None);
        fs::write(sessions.join("4.json"), "not json").unwrap();
        assert_eq!(session_ref(Some(profile.path()), 4), None);
    }

    fn session(name: &str, cwd: &str, id: &str, pid: u32) -> Running {
        Running {
            name: Some(name.into()),
            cwd: Some(cwd.into()),
            status: None,
            session_id: Some(id.into()),
            pid: Some(pid),
        }
    }

    #[test]
    fn addresses_only_for_names_carried_twice() {
        let running = [
            session("dev", "/team", "s1", 1),
            session("dev", "/other", "s2", 2),
            session("lead", "/team", "s3", 3),
            session("qa", "/team", "s4", 4),
            session("qa", "/team", "s5", 5),
            session("ops", "/team", "s6", 6),
            session("ops", "/other", "s7", 7),
        ];
        let noted = |member: &str| match member {
            "qa" => Some("s5".to_string()),
            "ops" => Some("gone".to_string()),
            _ => None,
        };
        let reference = |pid: u32| (pid != 6).then(|| format!("ref{pid}"));
        let found = addresses(Path::new("/team"), &["dev", "lead", "qa", "ops"], &running, noted, reference);
        // dev: its only session in the team's folder; qa: the one noted, though both are here; lead: alone with its
        // name; ops: found, but its file gives no address.
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            [("dev".into(), "ref1".into()), ("qa".into(), "ref5".into())]
        );
        // Two in the team's folder and none noted: no way to tell, no address.
        let found = addresses(Path::new("/team"), &["qa"], &running, |_| None, |pid| Some(pid.to_string()));
        assert!(found.is_empty());
    }

    #[test]
    fn session_status_from_its_file() {
        let profile = tempfile::tempdir().unwrap();
        let sessions = profile.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("42.json"), r#"{"pid":42,"status":"shell","statusUpdatedAt":1791530838682}"#).unwrap();
        let file = |status: &str, since| Some(SessionFile { status: status.into(), since });
        assert_eq!(session_status(Some(profile.path()), 42), file("shell", Some(1_791_530_838)));
        // Its time written otherwise, or not at all: the status alone.
        fs::write(sessions.join("45.json"), r#"{"pid":45,"status":"shell","statusUpdatedAt":1791530838682.5}"#)
            .unwrap();
        assert_eq!(session_status(Some(profile.path()), 45), file("shell", Some(1_791_530_838)));
        fs::write(sessions.join("46.json"), r#"{"pid":46,"status":"shell","statusUpdatedAt":"soon"}"#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 46), file("shell", None));
        fs::write(sessions.join("47.json"), r#"{"pid":47,"status":"shell"}"#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 47), file("shell", None));
        // Missing, or not as expected: nothing.
        assert_eq!(session_status(Some(profile.path()), 7), None);
        fs::write(sessions.join("43.json"), r#"{"pid":43,"status":3}"#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 43), None);
        fs::write(sessions.join("44.json"), r#"{"pid":44,"status":"idle""#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 44), None);
    }

    #[test]
    fn project_keys() {
        assert_eq!(project_key(Path::new("/home/user/code/my-project")), "-home-user-code-my-project");
        assert_eq!(project_key(Path::new("/tmp/a.b_c/é")), "-tmp-a-b-c--");
    }

    #[test]
    fn titles_from_conversation_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc.jsonl");
        fs::write(
            &file,
            concat!(
                r#"{"type":"user","message":"x"}"#,
                "\n",
                r#"{"type":"custom-title","customTitle":"ancien"}"#,
                "\n",
                r#"{"type":"custom-title","customTitle":"opérateur"}"#,
                "\n",
                r#"{"type":"assistant"}"#,
                "\n"
            ),
        )
        .unwrap();
        assert_eq!(last_title(&file).as_deref(), Some("opérateur"));
        fs::write(&file, "").unwrap();
        assert_eq!(last_title(&file), None);
    }

    #[test]
    fn profile_folder() {
        let profile = tempfile::tempdir().unwrap();
        let project = Path::new("/tmp/projet");
        fs::write(profile.path().join(".claude.json"), r#"{"projects":{"/tmp":{"hasTrustDialogAccepted":true}}}"#)
            .unwrap();
        assert_eq!(trusted(project, Some(profile.path())), Some(true));
        assert_eq!(trusted(Path::new("/opt/ailleurs"), Some(profile.path())), Some(false));

        let folder = profile.path().join("projects").join(project_key(project));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("id-1.jsonl"), concat!(r#"{"type":"custom-title","customTitle":"dev"}"#, "\n")).unwrap();
        let found = last_conversations(project, Some(profile.path())).unwrap();
        assert_eq!(found.get("dev").map(String::as_str), Some("id-1"));

        let env = |c: &Command| c.get_envs().find(|(k, _)| *k == CONFIG_DIR).and_then(|(_, v)| v).map(PathBuf::from);
        assert_eq!(env(&command(Path::new("claude"), Some(profile.path()))), Some(profile.path().to_path_buf()));
        assert_eq!(env(&command(Path::new("claude"), None)), None);
    }

    #[test]
    fn transcripts() {
        assert_eq!(
            transcript(Path::new("/tmp/p"), Some(Path::new("/profil")), "id"),
            Path::new("/profil/projects/-tmp-p/id.jsonl")
        );
    }

    #[test]
    fn member_command_line() {
        let mut team = Team { permission_mode: Some("auto".into()), model: Some("opus".into()), ..Default::default() };
        team.members.insert("lead".into(), Member { contact: true, ..Default::default() });
        let member = Member {
            role: "r".into(),
            model: Some("sonnet".into()),
            args: vec!["--verbose".into()],
            ..Default::default()
        };
        let claude = Path::new("/bin/claude");
        let prompt = Path::new("/tmp/p.md");
        let launch = Launch { claude, team: &team, plugin_dir: None, status_line: true };
        assert_eq!(
            launch.argv("dev", &member, prompt, None),
            [
                "/bin/claude",
                "--permission-mode",
                "auto",
                "--model",
                "sonnet",
                "--settings",
                r#"{"statusLine":{"type":"command","command":"true"}}"#,
                "--verbose",
                "-n",
                "dev",
                "--append-system-prompt-file",
                "/tmp/p.md"
            ]
        );
        assert!(launch.argv("dev", &member, prompt, Some("id-1")).ends_with(&["-r".into(), "id-1".into()]));
        let with_mod = Launch { plugin_dir: Some(Path::new("/m")), ..launch }.argv("dev", &member, prompt, None);
        assert_eq!(with_mod[1..3], ["--plugin-dir", "/m"]);

        // The main contact keeps its bars.
        let settings = |argv: Vec<String>| argv.iter().filter(|a| a.starts_with("--settings")).count();
        assert_eq!(settings(launch.argv("lead", &member, prompt, None)), 0);
        // Resumed, a member stays quiet: its pane starts it again with the same arguments.
        assert_eq!(settings(launch.argv("dev", &member, prompt, Some("id-1"))), 1);
        // No status line to replace: nothing, QUIET would add a row.
        assert_eq!(settings(Launch { status_line: false, ..launch }.argv("dev", &member, prompt, None)), 0);
        // Settings of the user's own, the member's or the team's: only theirs.
        let own = Member { args: vec!["--settings".into(), "/x.json".into()], ..member.clone() };
        assert_eq!(settings(launch.argv("dev", &own, prompt, None)), 1);
        team.args = vec!["--settings=/y.json".into()];
        let launch = Launch { claude, team: &team, plugin_dir: None, status_line: true };
        assert_eq!(settings(launch.argv("dev", &member, prompt, None)), 1);
        assert!(!launch.argv("dev", &member, prompt, None).iter().any(|a| a.contains("statusLine")));
        let quiet = Launch { claude, team: &Team::default(), plugin_dir: None, status_line: true };
        assert!(quieted(&quiet.argv("dev", &member, prompt, None)));
        let argv = ["claude", "--model", "opus", "--effort=high", "--model=sonnet"].map(String::from);
        assert_eq!((flag_value(&argv, "--model"), flag_value(&argv, "--effort")), (Some("sonnet"), Some("high")));
        assert_eq!(flag_value(&argv, "--permission-mode"), None);
    }

    #[test]
    fn status_lines_in_settings() {
        let profile = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path().join("sous-dossier");
        fs::create_dir_all(dir.join(".claude")).unwrap();
        fs::create_dir_all(repo.path().join(".git")).unwrap();
        fs::create_dir_all(repo.path().join(".claude")).unwrap();
        let line = r#"{"statusLine":{"type":"command","command":"x"}}"#;
        assert!(!status_line(&dir, Some(profile.path())));

        fs::write(profile.path().join("settings.json"), r#"{"model":"opus","statusLine":null}"#).unwrap();
        assert!(!status_line(&dir, Some(profile.path())));
        fs::write(profile.path().join("settings.json"), line).unwrap();
        assert!(status_line(&dir, Some(profile.path())));
        fs::remove_file(profile.path().join("settings.json")).unwrap();

        // The repository's root counts for the local settings only.
        fs::write(repo.path().join(".claude/settings.json"), line).unwrap();
        assert!(!status_line(&dir, Some(profile.path())));
        fs::write(repo.path().join(".claude/settings.local.json"), line).unwrap();
        assert!(status_line(&dir, Some(profile.path())));
        fs::remove_file(repo.path().join(".claude/settings.local.json")).unwrap();

        fs::write(dir.join(".claude/settings.local.json"), line).unwrap();
        assert!(status_line(&dir, Some(profile.path())));
        fs::remove_file(dir.join(".claude/settings.local.json")).unwrap();

        fs::write(dir.join(".claude/settings.json"), line).unwrap();
        assert!(status_line(&dir, Some(profile.path())));
        // Not valid JSON: as if they gave one.
        fs::write(dir.join(".claude/settings.json"), "{").unwrap();
        assert!(status_line(&dir, Some(profile.path())));
    }

    /// A worktree as git adds it: `<common>/worktrees/<name>` with its `commondir` and `gitdir`, and the worktree's
    /// `.git` file. Its git directory.
    fn add_worktree(common: &Path, worktree: &Path) -> PathBuf {
        let git_dir = common.join("worktrees").join(worktree.file_name().unwrap());
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(worktree).unwrap();
        fs::write(git_dir.join("commondir"), "../..\n").unwrap();
        fs::write(git_dir.join("gitdir"), format!("{}\n", worktree.join(".git").display())).unwrap();
        fs::write(worktree.join(".git"), format!("gitdir: {}\n", git_dir.display())).unwrap();
        git_dir
    }

    #[test]
    fn worktrees_take_their_main_repository_as_root() {
        let profile = tempfile::tempdir().unwrap();
        let top = tempfile::tempdir().unwrap();
        let main = top.path().join("projet");
        let worktree = top.path().join("essai");
        let git_dir = add_worktree(&main.join(".git"), &worktree);
        let dir = worktree.join("sous-dossier");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(git_root(&dir), Some(main.canonicalize().unwrap()));

        // The main repository's local settings count, the worktree's root's do not.
        fs::create_dir_all(worktree.join(".claude")).unwrap();
        fs::write(worktree.join(".claude/settings.local.json"), r#"{"statusLine":{}}"#).unwrap();
        assert!(!status_line(&dir, Some(profile.path())));
        fs::create_dir_all(main.join(".claude")).unwrap();
        fs::write(main.join(".claude/settings.local.json"), r#"{"statusLine":{}}"#).unwrap();
        assert!(status_line(&dir, Some(profile.path())));

        // Relative links, as `worktree.useRelativePaths` writes them.
        fs::write(worktree.join(".git"), "gitdir: ../projet/.git/worktrees/essai\n").unwrap();
        fs::write(git_dir.join("gitdir"), "../../../../essai/.git\n").unwrap();
        assert_eq!(git_root(&dir), Some(main.canonicalize().unwrap()));
        // A git directory that names another worktree back: its own folder.
        fs::write(git_dir.join("gitdir"), format!("{}\n", main.join(".git").display())).unwrap();
        assert_eq!(git_root(&dir), Some(worktree.clone()));
        fs::write(git_dir.join("gitdir"), "../../../../essai/.git\n").unwrap();
        // A `.git` file without `commondir` behind it (a submodule): its own folder.
        fs::remove_file(git_dir.join("commondir")).unwrap();
        assert_eq!(git_root(&dir), Some(worktree.clone()));
        // An ordinary repository.
        assert_eq!(git_root(&main.join(".claude")), Some(main));
    }

    #[test]
    fn worktrees_of_a_bare_repository_are_their_own_root() {
        let top = tempfile::tempdir().unwrap();
        let worktree = top.path().join("projet/main");
        add_worktree(&top.path().join("projet/.bare"), &worktree);
        assert_eq!(git_root(&worktree.join("src")), Some(worktree.clone()));
        // The same, with a `.git` common directory: the main repository.
        let worktree = top.path().join("autre/main");
        add_worktree(&top.path().join("autre/.git"), &worktree);
        assert_eq!(git_root(&worktree), Some(top.path().join("autre").canonicalize().unwrap()));
    }
}
