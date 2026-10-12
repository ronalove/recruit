// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Everything recruit asks of Claude Code: where it is, which sessions run, the last conversation of each
//! name, and the command line of a member; and recruit's mod in Claude Code, with the bridge that does its work
//! ([`bridge`]).

mod args;
pub(crate) mod bridge;
mod session;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::sys::home;
use crate::t;

pub use self::args::{Launch, flag_value, quiet_settings, quieted, status_line};
pub use self::session::{
    SessionFile, addresses, last_conversations, session_id, session_ref, session_status, transcript,
};

/// The executable, looked up in the PATH unless it is a path: the first that can be run, as a shell finds it.
pub fn find_executable(command: &str) -> Option<PathBuf> {
    let command = crate::sys::expand_home(command);
    if command.components().count() > 1 {
        return command.is_file().then_some(command);
    }
    find_in(&std::env::var_os("PATH")?, &command)
}

/// The first file called `command` in the folders of `path` that the user can run.
fn find_in(path: &std::ffi::OsStr, command: &Path) -> Option<PathBuf> {
    std::env::split_paths(path).map(|dir| dir.join(command)).find(|file| file.is_file() && can_run(file))
}

/// Whether the user may run `file` (the `X_OK` check of `access`).
fn can_run(file: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(file.as_os_str().as_bytes()) else { return false };
    // SAFETY: a NUL-ended path that outlives the call.
    unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 }
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

/// Claude Code loads mods from 2.1.287 on.
pub const MIN_CLAUDE: (u32, u32, u32) = (2, 1, 287);

/// The version of Claude Code (`claude --version`: "2.1.292 (Claude Code)").
pub fn claude_version(claude: &Path) -> Option<(u32, u32, u32)> {
    let output = Command::new(claude).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace().next()?.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::session::project_key;

    /// A `claude` that cannot be run, earlier in the PATH, is passed over, as a shell does.
    #[test]
    fn the_first_file_that_can_be_run() {
        use std::os::unix::fs::PermissionsExt;
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for (dir, mode) in [(&first, 0o644), (&second, 0o755)] {
            let file = dir.path().join("claude");
            fs::write(&file, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        }
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();
        assert_eq!(find_in(&path, Path::new("claude")), Some(second.path().join("claude")));
        // None can be run: none found.
        let only = std::env::join_paths([first.path()]).unwrap();
        assert_eq!(find_in(&only, Path::new("claude")), None);
        assert_eq!(find_in(&path, Path::new("absent")), None);
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
}
