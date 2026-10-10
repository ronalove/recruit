// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A whole team of the test's own, launched by `recruit` (its own multiplexer): a
//! project folder with its `.recruit/settings.toml`, a fake `claude` (a shim that answers `--version` and
//! `agents`, then the fake `claude` of `common::fakes`), a Claude Code profile of its own (`[claude] config_dir`:
//! recruit approves the team's folder there, never in the user's `~/.claude.json`), `XDG_*` and `RECRUIT_TMPDIR`
//! in temporary folders. Stopped by `recruit stop` when dropped, and checked to leave no process behind, and the
//! user's `~/.claude.json` and `~/.claude/settings.json` untouched.
//!
//! Owner: testeur.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use super::pty::{Profile, TestTerm};
use super::server::{leftovers, short};

pub struct Team {
    /// The test's folder, its path resolved (`/private/var/…` under macOS): recruit and the fakes see it so.
    pub root: PathBuf,
    _dir: tempfile::TempDir,
    pub proj: PathBuf,
    pub profile: PathBuf,
    pub name: String,
    /// The language the team is launched in, for the texts the tests read.
    pub lang: &'static str,
    recruit: PathBuf,
    run: tempfile::TempDir,
    settings: Option<Vec<u8>>,
    stopped: bool,
    /// A real Claude Code (the user's profile, and its `~/.claude.json` told the folder): only `settings.json` is
    /// checked untouched.
    real: bool,
}

/// The fake `claude`: answers `--version` (a version that takes the mod) and `agents` (the fakes running, from
/// their `fake/run/<pid>.json`) itself, then becomes the test binary's fake `claude`, its own arguments after `--`.
fn shim(root: &Path) -> String {
    let exe = std::env::current_exe().expect("the test binary");
    format!(
        r#"#!/bin/sh
D="${{CLAUDE_CONFIG_DIR:-/nonexistent}}/fake"
case "$1" in
  --version) echo "2.1.295 (Claude Code)"; exit 0;;
  agents)
    printf '['; sep=''
    for f in "$D"/run/*.json; do
      [ -f "$f" ] || continue
      p=${{f##*/}}; p=${{p%.json}}
      kill -0 "$p" 2>/dev/null || continue
      printf '%s' "$sep"; cat "$f"; sep=','
    done
    printf ']\n'; exit 0;;
esac
exec env {child}=claude '{exe}' child --exact --ignored --nocapture --test-threads=1 -q '{root}' -- "$@"
"#,
        child = super::CHILD,
        exe = exe.display(),
        root = root.display(),
    )
}

fn user_file(path: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(path)
}

impl Team {
    /// A team `name` whose tables (`[teams.<name>]`, its members…) are `tables`, in French.
    pub fn new(name: &str, tables: &str) -> Team {
        Team::in_lang(name, tables, "fr")
    }

    /// A team of real Claude Code sessions: no fake, the user's profile. recruit approves the folder in the user's
    /// `~/.claude.json`, as it does for any team: the folder stays there, to tidy up.
    pub fn real(name: &str, tables: &str, lang: &'static str) -> Team {
        let mut team = Team::build(name, tables, lang, true);
        team.real = true;
        team
    }

    pub fn in_lang(name: &str, tables: &str, lang: &'static str) -> Team {
        Team::build(name, tables, lang, false)
    }

    fn build(name: &str, tables: &str, lang: &'static str, real: bool) -> Team {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().canonicalize().expect("canonical temp dir");
        let proj = root.join("proj");
        let profile = root.join("profile");
        for d in [proj.join(".recruit"), profile.clone(), root.join("config"), root.join("cache")] {
            std::fs::create_dir_all(d).expect("folders");
        }
        let claude = root.join("claude");
        std::fs::write(&claude, shim(&root)).expect("shim");
        std::fs::set_permissions(&claude, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
        let settings = if real {
            tables.to_string()
        } else {
            format!("[claude]\ncommand = \"{}\"\nconfig_dir = \"{}\"\n\n{tables}", claude.display(), profile.display())
        };
        std::fs::write(proj.join(".recruit/settings.toml"), settings).expect("settings.toml");
        let recruit = super::recruit_copy(&root);
        let run = tempfile::Builder::new().prefix("rt.").tempdir_in("/tmp").expect("short run dir");
        let settings = std::fs::read(user_file(".claude/settings.json")).ok();
        Team { root, _dir: dir, proj, profile, name: name.into(), lang, recruit, run, settings, stopped: false, real }
    }

    /// The copy of recruit, in the project, with the test's environment.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.recruit);
        for key in super::pty::SCRUB {
            command.env_remove(key);
        }
        command
            .current_dir(&self.proj)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("RECRUIT_TMPDIR", self.run.path())
            .env("RECRUIT_LANG", self.lang)
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null());
        command
    }

    /// `bash -c <script>` in the project with the test's environment (`XDG_*`, `RECRUIT_*`) and `RECRUIT` the copy of
    /// recruit under test: its output, and whether it succeeded.
    pub fn bash(&self, script: &str) -> (bool, String) {
        let mut command = Command::new("bash");
        command.arg("-c").arg(script).current_dir(&self.proj);
        for (key, value) in self.command().get_envs() {
            match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        command.env("RECRUIT", &self.recruit).env("TEAM_STATE", self.state()).env("TEAM_NAME", &self.name);
        let out = command.output().expect("bash");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }

    /// `recruit <args…>` in the project, whatever its code.
    pub fn recruit<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_os_string()).collect();
        super::retry_busy(|| self.command().args(&args).output()).expect("recruit")
    }

    /// `recruit --detach`, which must succeed; then every member's fake up.
    pub fn launch(&self, members: &[&str]) {
        let out = self.recruit(["--detach"]);
        assert!(out.status.success(), "recruit --detach: {}{}", self.text(&out.stdout), self.text(&out.stderr));
        for member in members {
            assert!(
                self.wait_pane(member, "FAKE CLAUDE", Duration::from_secs(10)),
                "{member} never started: {:?}",
                self.capture(member)
            );
        }
    }

    fn text(&self, bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// The team's folder, where its server keeps its state.
    pub fn state(&self) -> PathBuf {
        self.root.join("cache/recruit/teams").join(&self.name)
    }

    /// `recruit _ctl <state> <args…>`, whatever its code.
    pub fn ctl_output<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_os_string()).collect();
        super::retry_busy(|| self.command().arg("_ctl").arg(self.state()).args(&args).output()).expect("_ctl")
    }

    /// `recruit _ctl <state> <args…>`, which must succeed; its stdout.
    pub fn ctl<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_os_string()).collect();
        let out = self.ctl_output(&args);
        assert!(out.status.success(), "_ctl {args:?} ({}): {}", out.status, self.text(&out.stderr));
        self.text(&out.stdout)
    }

    /// The panes, as `_ctl panes --json` gives them (id, member, tab, pid, cols, rows…).
    pub fn panes(&self) -> Vec<serde_json::Value> {
        serde_json::from_str(&self.ctl(["panes", "--json"])).expect("panes: JSON")
    }

    /// The tabs in order, each with its panes' names in order.
    pub fn tabs(&self) -> Vec<(String, Vec<String>)> {
        let mut tabs: Vec<(String, Vec<String>)> = Vec::new();
        for pane in self.panes() {
            let tab = pane["tab"].as_str().unwrap_or_default().to_string();
            let member = pane["member"].as_str().unwrap_or_default().to_string();
            match tabs.iter_mut().find(|(t, _)| *t == tab) {
                Some((_, members)) => members.push(member),
                None => tabs.push((tab, vec![member])),
            }
        }
        tabs
    }

    /// The text of `pane` (an id or a member), or nothing when it is gone.
    pub fn capture(&self, pane: &str) -> Vec<String> {
        let out = self.ctl_output(["capture", "--pane", pane]);
        self.text(&out.stdout).lines().map(|l| l.trim_end().to_string()).collect()
    }

    /// Waits until `text` shows in `pane`, `timeout` at most.
    pub fn wait_pane(&self, pane: &str, text: &str, timeout: Duration) -> bool {
        wait(timeout, || self.capture(pane).iter().any(|r| r.contains(text)))
    }

    /// The command lines the fake `claude` was started with for `member`, in order.
    pub fn launches(&self, member: &str) -> Vec<String> {
        let log = std::fs::read_to_string(self.profile.join("fake/argv.log")).unwrap_or_default();
        log.lines().filter_map(|l| l.strip_prefix(&format!("{member}\t")).map(str::to_string)).collect()
    }

    /// Whether `member`'s `recruit _member` still runs (the loop that starts its Claude again).
    pub fn member_runs(&self, member: &str) -> bool {
        let state = self.state();
        let wanted = format!("_member {} {member}", state.display());
        let resumed = format!("_member --resume {} {member}", state.display());
        leftovers(&self.root).iter().any(|l| l.contains(&wanted) || l.contains(&resumed))
    }

    /// A real client (`recruit attach <team>`) on a new test terminal.
    pub fn attach(&self, cols: u16, rows: u16, profile: Profile) -> TestTerm {
        self.in_terminal(&["attach", &self.name], cols, rows, profile)
    }

    /// `recruit <args…>` on a new test terminal of the team's environment, in the project.
    pub fn in_terminal(&self, args: &[&str], cols: u16, rows: u16, profile: Profile) -> TestTerm {
        let env = [
            ("XDG_CONFIG_HOME", self.root.join("config").into_os_string()),
            ("XDG_CACHE_HOME", self.root.join("cache").into_os_string()),
            ("RECRUIT_TMPDIR", self.run.path().as_os_str().to_os_string()),
            ("RECRUIT_LANG", self.lang.into()),
            ("SHELL", "/bin/sh".into()),
        ];
        let env: Vec<(&str, &OsStr)> = env.iter().map(|(k, v)| (*k, v.as_os_str())).collect();
        TestTerm::spawn_in(Some(&self.proj), self.recruit.as_os_str(), args, &env, cols, rows, profile)
    }

    /// `recruit stop <team>`, then: nothing left running, the user's files untouched.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let out = self.recruit(["stop", &self.name]);
        assert!(out.status.success(), "recruit stop: {}", self.text(&out.stderr));
        let gone = wait(Duration::from_secs(5), || leftovers(&self.root).is_empty());
        assert!(gone, "processes left after stop: {:?}", short(&leftovers(&self.root)));
        self.check_user_files();
    }

    /// The team was stopped from inside (the choice's « Quitter »): nothing left running, the user's files untouched.
    pub fn stopped_by_the_user(&mut self) {
        self.stopped = true;
        let gone = wait(Duration::from_secs(5), || leftovers(&self.root).is_empty());
        assert!(gone, "processes left after the team's quit: {:?}", short(&leftovers(&self.root)));
        self.check_user_files();
    }

    /// Whether the team's server answers.
    pub fn running(&self) -> bool {
        self.ctl_output(["where"]).status.success()
    }

    /// The user's `~/.claude.json` never names the test's folder, and `~/.claude/settings.json` did not change.
    pub fn check_user_files(&self) {
        let root = self.root.to_string_lossy();
        if !self.real {
            let claude_json = std::fs::read_to_string(user_file(".claude.json")).unwrap_or_default();
            assert!(!claude_json.contains(root.as_ref()), "~/.claude.json names the test's folder");
        }
        let settings = std::fs::read(user_file(".claude/settings.json")).ok();
        assert!(settings == self.settings, "~/.claude/settings.json changed during the test");
    }
}

impl Drop for Team {
    fn drop(&mut self) {
        if !self.stopped && !std::thread::panicking() {
            self.stop();
        } else if !self.stopped {
            // A failed test: stop without checking, never a second panic.
            self.stopped = true;
            let _ = self.recruit(["stop", &self.name]);
            // `recruit stop` reads the team's files: with those broken it stops nothing, and the server would stay.
            let _ = self.ctl_output(["stop"]);
        }
    }
}

/// Waits until `check` holds, `timeout` at most.
pub fn wait(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
