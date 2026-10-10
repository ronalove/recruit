// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What a running team leaves for its panels: the team as it was launched, in `~/.cache/recruit/teams/<session>/`.

use std::collections::HashMap;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{Origin, Team, cache_dir, write_atomic};
use crate::i18n::Lang;
use crate::t;

const FILE: &str = "team.json";
const LOCK: &str = "team.lock";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub team: String,
    pub session: String,
    /// Where the members work.
    pub dir: PathBuf,
    /// The Claude Code executable, and its profile when the settings name one.
    pub claude: PathBuf,
    #[serde(default)]
    pub config_dir: Option<PathBuf>,
    pub lang: Lang,
    pub members: Vec<MemberInfo>,
    /// The dashboard pane's command line, to open it again if it closed; empty without a dashboard.
    #[serde(default)]
    pub dashboard: Vec<String>,
    /// The journal pane's command line, to open it again once hidden.
    pub journal: Vec<String>,
    /// Where the team is written, to change it from its menu and read it again. None in a team launched before it.
    #[serde(default)]
    pub origin: Option<Origin>,
    /// The mod's folder, when Claude Code loads it: what a member's command line is built with again.
    #[serde(default)]
    pub plugin_dir: Option<PathBuf>,
    /// The members' settings give a status line (`claude::status_line`).
    #[serde(default)]
    pub status_line: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemberInfo {
    pub name: String,
    /// The first line of the member's role.
    pub role: String,
    pub contact: bool,
    /// Without Claude Code's bars at the bottom: every member but the main contact. False in a team launched
    /// before it, whose members keep them.
    #[serde(default)]
    pub quiet: bool,
    /// Its Claude command line, as its running Claude was started, a new conversation's form: `-r <id>` in place
    /// of the prompt when it resumed one at launch. Built again from the team's files each time the member starts.
    #[serde(default)]
    pub argv: Vec<String>,
    /// The model and the effort its settings give, the member's own or else the team's: when they differ from those
    /// of `argv`, the mod puts them on each request, until the member starts again.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// Its tab, as its settings gave it when it last changed: the place it keeps once its files no longer have it.
    #[serde(default)]
    pub tab: Option<String>,
}

/// The members of a team, in its order, its contacts marked, with their command lines.
pub fn members(team: &Team, argvs: &HashMap<String, Vec<String>>) -> Vec<MemberInfo> {
    let contacts = team.contacts();
    team.members
        .iter()
        .map(|(name, m)| MemberInfo {
            name: name.clone(),
            role: m.role.lines().next().unwrap_or_default().trim().to_string(),
            contact: contacts.contains(&name.as_str()),
            quiet: team.quiet(name),
            argv: argvs.get(name).cloned().unwrap_or_default(),
            model: m.model.clone().or_else(|| team.model.clone()),
            effort: m.effort.clone().or_else(|| team.effort.clone()),
            tab: m.tab.clone(),
        })
        .collect()
}

/// Keeps the other writers of a running team's files out while held: its menu, open twice, or a member starting
/// again. Readers need nothing, team.json is written in one go.
pub struct Lock {
    _file: fs::File,
}

/// Waits for the team's lock.
pub fn lock(dir: &Path) -> Result<Lock> {
    fs::create_dir_all(dir).with_context(|| t!("création de {}", "creating {}", dir.display()))?;
    let file = dir.join(LOCK);
    let handle = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&file)
        .with_context(|| t!("ouverture de {}", "opening {}", file.display()))?;
    // SAFETY: flock on a descriptor this function owns; released when the file closes.
    if unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| file.display().to_string());
    }
    Ok(Lock { _file: handle })
}

/// The folder of a running team, by its session name (`backend::session_name`).
pub fn dir(session: &str) -> PathBuf {
    cache_dir().join("teams").join(session)
}

impl Snapshot {
    /// In one go: the panels read it at any time. A writer that read it first holds the team's `lock`.
    pub fn write(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir).with_context(|| t!("création de {}", "creating {}", dir.display()))?;
        write_atomic(&dir.join(FILE), &serde_json::to_string_pretty(self)?)
    }

    pub fn member(&self, name: &str) -> Option<&MemberInfo> {
        self.members.iter().find(|m| m.name == name)
    }

    pub fn read(dir: &Path) -> Result<Self> {
        let file = dir.join(FILE);
        let text = fs::read_to_string(&file).with_context(|| t!("lecture de {}", "reading {}", file.display()))?;
        serde_json::from_str(&text).with_context(|| file.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Member;

    #[test]
    fn written_and_read_back() {
        let mut team = Team { model: Some("opus".into()), ..Default::default() };
        team.members.insert("lead".into(), Member { role: "Pilote\nen détail".into(), ..Default::default() });
        team.members.insert(
            "dev".into(),
            Member {
                role: "Code".into(),
                model: Some("sonnet".into()),
                effort: Some("high".into()),
                ..Default::default()
            },
        );
        let snapshot = Snapshot {
            team: "web".into(),
            session: "web".into(),
            dir: "/tmp/web".into(),
            claude: "/bin/claude".into(),
            config_dir: None,
            lang: Lang::Fr,
            members: members(&team, &HashMap::from([("lead".to_string(), vec!["claude".to_string()])])),
            dashboard: Vec::new(),
            journal: vec!["recruit".into(), "_panel".into()],
            origin: Some(Origin::Local { root: "/tmp/web".into() }),
            plugin_dir: None,
            status_line: true,
        };
        let lead = MemberInfo {
            name: "lead".into(),
            role: "Pilote".into(),
            contact: true,
            quiet: false,
            argv: vec!["claude".into()],
            model: Some("opus".into()),
            effort: None,
            tab: None,
        };
        assert_eq!(snapshot.members[0], lead);
        assert!(!snapshot.members[1].contact && snapshot.members[1].quiet);
        assert_eq!(
            (snapshot.members[1].model.as_deref(), snapshot.members[1].effort.as_deref()),
            (Some("sonnet"), Some("high"))
        );
        let dir = tempfile::tempdir().unwrap();
        snapshot.write(dir.path()).unwrap();
        assert_eq!(Snapshot::read(dir.path()).unwrap(), snapshot);

        // A team.json from before `quiet`, the origin and the configured model: its members keep their bars.
        let mut old: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join(FILE)).unwrap()).unwrap();
        for key in ["origin", "plugin_dir", "status_line"] {
            old.as_object_mut().unwrap().remove(key);
        }
        for member in old["members"].as_array_mut().unwrap() {
            for key in ["quiet", "model", "effort"] {
                member.as_object_mut().unwrap().remove(key);
            }
        }
        fs::write(dir.path().join(FILE), old.to_string()).unwrap();
        let old = Snapshot::read(dir.path()).unwrap();
        assert!(old.members.iter().all(|m| !m.quiet && m.model.is_none()));
        assert_eq!(old.origin, None);
        // Written in one go, nothing left beside it.
        let _lock = lock(dir.path()).unwrap();
        snapshot.write(dir.path()).unwrap();
        let mut names: Vec<String> =
            fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, [FILE, LOCK]);
    }
}
