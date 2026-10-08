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

    #[test]
    fn project_keys() {
        assert_eq!(
            project_key(Path::new("/Users/ronan/Developer/gitea/omnidex")),
            "-Users-ronan-Developer-gitea-omnidex"
        );
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
