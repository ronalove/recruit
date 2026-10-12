// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Team files.
//!
//! - Local: `.recruit/settings.toml` (committed) and `.recruit/settings.local.toml` (personal, not committed),
//!   found in the current directory or one of its parents. The local file is merged over the shared one.
//! - Global: `~/.config/recruit/<team>.toml`, one file per team, same format.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::sys::{expand_home, home, tilde, write_atomic};
use crate::t;

mod check;
mod edit;

pub use edit::{
    Edit, Field, Layered, MemberLayers, Source, TeamLayers, check_new_name, edit, member_layers, preview, team_layers,
};

pub const LOCAL_DIR: &str = ".recruit";
pub const SETTINGS: &str = "settings.toml";
pub const SETTINGS_LOCAL: &str = "settings.local.toml";

/// The members the user talks to share the first tab, next to the team's dashboard: two at most.
pub const MAX_CONTACTS: usize = 2;

use check::{Problem, problem};
pub use check::{reserved_title, validate_member_name, validate_team_name};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Team launched by a bare `recruit` when the file holds several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "ClaudeSettings::is_empty")]
    pub claude: ClaudeSettings,
    #[serde(default)]
    pub teams: IndexMap<String, Team>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeSettings {
    /// The Claude Code executable, `claude` from the PATH by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The Claude Code profile: its configuration folder, given to the members as `CLAUDE_CONFIG_DIR`. The
    /// environment's by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
}

impl ClaudeSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn command(&self) -> &str {
        self.command.as_deref().unwrap_or("claude")
    }

    /// `config_dir`, `~` expanded. Always absolute: `parse_settings` refuses the others.
    pub fn config_dir(&self) -> Option<PathBuf> {
        self.config_dir.as_deref().map(expand_home)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Team {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Language of the prompt recruit writes around the members' instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<Lang>,
    /// Rules shared by every member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Extra arguments given to claude for every member.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
    /// Grid size of a tab shared by several members: 3 columns × 2 rows by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
    /// The dashboard and the journal, in a column on the right of the contacts. On by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<bool>,
    /// A member at rest for a while compacted when its context is high: from 70 % of its window by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact: Option<AutoCompact>,
    #[serde(default)]
    pub members: IndexMap<String, Member>,
}

/// `auto_compact`: `false` turns it off, `true` is the default share, a number is the share of the window, in
/// percent, from which a member at rest for [`AUTO_COMPACT_IDLE`] seconds is compacted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum AutoCompact {
    Switch(bool),
    At(i64),
}

impl<'de> Deserialize<'de> for AutoCompact {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = AutoCompact;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(&t!(
                    "true, false ou un pourcentage de 1 à 100",
                    "true, false or a percentage from 1 to 100"
                ))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<AutoCompact, E> {
                Ok(AutoCompact::Switch(v))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<AutoCompact, E> {
                Ok(AutoCompact::At(v))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<AutoCompact, E> {
                Ok(AutoCompact::At(i64::try_from(v).unwrap_or(i64::MAX)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// The share of its window, in percent, from which a member at rest is compacted, when the team says nothing.
pub const AUTO_COMPACT_AT: u8 = 70;

/// How long a member stays at rest, in seconds, before its context is looked at for a compaction.
pub const AUTO_COMPACT_IDLE: u64 = 15 * 60;

/// The grid of a tab, when the team says nothing: 3 columns × 2 rows.
pub const COLUMNS: usize = 3;
pub const ROWS: usize = 2;

/// The tab of the working agents without a `tab` of their own, numbered when there are several.
pub const AGENTS: &str = "Agents";

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    /// One line: what the member is responsible for. Teammates see it.
    #[serde(default)]
    pub role: String,
    /// One of the user's contacts: the user talks to them, in the first tab. The others are working agents.
    #[serde(default, skip_serializing_if = "is_false")]
    pub contact: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Working agents with the same tab share it, in a grid; the others share "Agents" tabs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// The contacts in a first tab, then the working agents in tabs of `columns` × `rows` at most. "grid", which the
    /// menu writes, is the same.
    #[default]
    #[serde(alias = "grid")]
    Auto,
    /// One tab per member, contacts first.
    Tabs,
    /// As `Auto`, a tab of members shown in « focus »: the main one on the left, the best placed on the right, the
    /// others as cards (`mux::focus`). The contacts' tab with the panels keeps its grid.
    Focus,
}

impl Team {
    /// The language the team speaks: its own, else the interface's.
    pub fn language(&self) -> Lang {
        self.lang.unwrap_or_else(crate::i18n::lang)
    }

    /// The share of its window, in percent, from which a member at rest is compacted; None when it is turned off.
    pub fn auto_compact_at(&self) -> Option<u8> {
        match self.auto_compact {
            None | Some(AutoCompact::Switch(true)) => Some(AUTO_COMPACT_AT),
            Some(AutoCompact::Switch(false)) => None,
            Some(AutoCompact::At(at)) => Some(at.clamp(1, 100) as u8),
        }
    }

    /// The members the user talks to: those marked `contact`, or else the first one.
    pub fn contacts(&self) -> Vec<&str> {
        let marked: Vec<&str> = self.members.iter().filter(|(_, m)| m.contact).map(|(n, _)| n.as_str()).collect();
        if marked.is_empty() { self.members.keys().take(1).map(String::as_str).collect() } else { marked }
    }

    /// True for every member but the main contact, the first of the contacts: Claude Code's bars at the bottom of
    /// its pane are left out.
    pub fn quiet(&self, name: &str) -> bool {
        self.contacts().first() != Some(&name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Local,
    Global,
}

/// A team and where it comes from.
#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    pub scope: Scope,
    /// The file the team is written in.
    pub file: PathBuf,
    /// Local team: the project root, parent of `.recruit`.
    pub root: Option<PathBuf>,
    pub team: Team,
    pub claude: ClaudeSettings,
}

impl Found {
    /// Where the members work: a local team at its project's root, a global team wherever it is launched.
    pub fn work_dir(&self, cwd: &Path) -> PathBuf {
        self.root.clone().unwrap_or_else(|| cwd.to_path_buf())
    }

    /// Where the team is written, to change it and read it again.
    pub fn origin(&self) -> Origin {
        match (&self.root, self.scope) {
            (Some(root), Scope::Local) => Origin::Local { root: root.clone() },
            _ => Origin::Global { file: self.file.clone() },
        }
    }
}

/// Where a team is written: a local team in its project's `.recruit/settings.toml` and `settings.local.toml`, a
/// global one in its own file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "lowercase")]
pub enum Origin {
    Local { root: PathBuf },
    Global { file: PathBuf },
}

impl Origin {
    /// The team as its files say now.
    pub fn load(&self, team: &str) -> Result<Found> {
        let settings = match self {
            Origin::Local { root } => load_local(root)?,
            Origin::Global { file } => parse_settings(read_table(file)?, file)?,
        };
        self.found(&settings, team)
    }

    /// The team in `settings`, read from this origin's files.
    fn found(&self, settings: &Settings, team: &str) -> Result<Found> {
        let (scope, file, root) = match self {
            Origin::Local { root } => (Scope::Local, root.join(LOCAL_DIR).join(SETTINGS), Some(root)),
            Origin::Global { file } => (Scope::Global, file.clone(), None),
        };
        teams_of(settings, scope, &file, root).into_iter().find(|f| f.name == team).with_context(|| {
            t!("l'équipe « {} » n'est plus dans {}", "team \"{}\" is no longer in {}", team, tilde(&self.place()))
        })
    }

    /// For messages: the project's `.recruit` folder, or the global file.
    fn place(&self) -> PathBuf {
        match self {
            Origin::Local { root } => root.join(LOCAL_DIR),
            Origin::Global { file } => file.clone(),
        }
    }
}

/// Every team visible from a directory.
#[derive(Debug, Default)]
pub struct Catalog {
    /// The project root and its merged settings.
    pub local: Option<(PathBuf, Settings)>,
    /// Global files, sorted by name.
    pub global: Vec<(PathBuf, Settings)>,
}

impl Catalog {
    /// The project's root, else `cwd`: where a new local team is written, and where Claude composes a team from.
    pub fn root(&self, cwd: &Path) -> PathBuf {
        self.local.as_ref().map_or_else(|| cwd.to_path_buf(), |(root, _)| root.clone())
    }

    /// The project's `[claude]` settings; the defaults outside a project.
    pub fn claude(&self) -> ClaudeSettings {
        self.local.as_ref().map(|(_, s)| s.claude.clone()).unwrap_or_default()
    }

    pub fn load(cwd: &Path) -> Result<Self> {
        let local = match find_local_root(cwd) {
            Some(root) => {
                let settings = load_local(&root)?;
                Some((root, settings))
            }
            None => None,
        };
        Ok(Catalog { local, global: load_global()? })
    }

    /// A team by name: local first, then global (spec: the local one wins).
    pub fn find(&self, name: &str) -> Option<Found> {
        self.local_teams().into_iter().chain(self.global_teams()).find(|f| f.name == name)
    }

    pub fn local_teams(&self) -> Vec<Found> {
        let Some((root, settings)) = &self.local else { return Vec::new() };
        let file = root.join(LOCAL_DIR).join(SETTINGS);
        teams_of(settings, Scope::Local, &file, Some(root))
    }

    pub fn global_teams(&self) -> Vec<Found> {
        let mut seen = std::collections::HashSet::new();
        self.global
            .iter()
            .flat_map(|(file, settings)| teams_of(settings, Scope::Global, file, None))
            .filter(|f| seen.insert(f.name.clone()))
            .collect()
    }

    /// The local team a bare `recruit` launches: the only one, or `default`. None when it must be chosen.
    pub fn default_local(&self) -> Option<Found> {
        let teams = self.local_teams();
        if teams.len() == 1 {
            return teams.into_iter().next();
        }
        let default = self.local.as_ref()?.1.default.as_ref()?;
        teams.into_iter().find(|f| &f.name == default)
    }
}

fn teams_of(settings: &Settings, scope: Scope, file: &Path, root: Option<&PathBuf>) -> Vec<Found> {
    settings
        .teams
        .iter()
        .map(|(name, team)| Found {
            name: name.clone(),
            scope,
            file: file.to_path_buf(),
            root: root.cloned(),
            team: team.clone(),
            claude: settings.claude.clone(),
        })
        .collect()
}

/// A project's two files: `.recruit/settings.toml`, then `.recruit/settings.local.toml`, merged over it.
pub fn local_files(root: &Path) -> [PathBuf; 2] {
    let dir = root.join(LOCAL_DIR);
    [dir.join(SETTINGS), dir.join(SETTINGS_LOCAL)]
}

/// The nearest directory, from `start` up, that holds one of a project's files.
pub fn find_local_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| local_files(dir).iter().any(|file| file.is_file())).map(Path::to_path_buf)
}

/// The file of team `name`, as `Catalog::find` gives it, found in the plain TOML without checking the settings: for
/// `recruit edit` when the team files do not read. The project's `settings.toml` when one of its files has the team,
/// else the global file that has it, else the global file of its name.
pub fn team_file_unchecked(cwd: &Path, name: &str) -> Option<PathBuf> {
    if let Some(root) = find_local_root(cwd) {
        let [shared, personal] = local_files(&root);
        if defines(&shared, name) || defines(&personal, name) {
            return Some(shared);
        }
    }
    global_files().into_iter().find(|f| defines(f, name)).or_else(|| Some(global_file(name)).filter(|f| f.is_file()))
}

/// Whether `file` has team `name`, read as plain TOML, without checking the settings.
fn defines(file: &Path, name: &str) -> bool {
    read_table(file).is_ok_and(|t| t.get("teams").and_then(toml::Value::as_table).is_some_and(|t| t.contains_key(name)))
}

/// `settings.toml`, with `settings.local.toml` merged over it.
pub fn load_local(root: &Path) -> Result<Settings> {
    let mut tables = Vec::new();
    for file in local_files(root) {
        if file.is_file() {
            tables.push((read_table(&file)?, file));
        }
    }
    merge_local(&root.join(LOCAL_DIR), tables)
}

/// The settings of a project's files, the shared one first, as `load_local` reads them.
fn merge_local(dir: &Path, tables: Vec<(toml::Table, PathBuf)>) -> Result<Settings> {
    let mut merged = toml::Table::new();
    let mut files = Vec::new();
    for (table, file) in tables {
        // Each file on its own first, so that an error names the file it is in. A member may still lack its role
        // here: the other file can give it.
        files.push((deserialize(table.clone(), &file)?, file));
        merge(&mut merged, table);
    }
    let settings = deserialize(merged, &dir.join(SETTINGS))?;
    // After the merge: two members a case apart may come one from each file.
    let Some(problem) = problem(&settings) else { return Ok(settings) };
    let defines = |s: &Settings, team: &str, member: Option<&str>| {
        s.teams.get(team).is_some_and(|t| member.is_none_or(|m| t.members.contains_key(m)))
    };
    let file = match &problem {
        // The file that defines the team or the member, the shared one first.
        Problem::Name(bad) => files.iter().find(|(s, _)| defines(s, bad.team, bad.member)),
        // The file that defines the member, the shared one first: there is one.
        Problem::NoRole { team, member } => files.iter().find(|(s, _)| defines(s, team, Some(member))),
        // The last file that marks one of them.
        Problem::Contacts { team, .. } => {
            files.iter().rev().find(|(s, _)| s.teams.get(*team).is_some_and(|t| t.members.values().any(|m| m.contact)))
        }
    };
    Err(problem.error(&file.map_or_else(|| dir.join(SETTINGS), |(_, f)| f.clone())))
}

pub fn global_dir() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("recruit"),
        _ => home().join(".config").join("recruit"),
    }
}

pub fn global_file(team: &str) -> PathBuf {
    global_dir().join(format!("{team}.toml"))
}

/// The global teams' files, in order.
fn global_files() -> Vec<PathBuf> {
    files_in(&global_dir())
}

/// The TOML files of `dir`, in order.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml") && p.is_file())
        .collect();
    files.sort();
    files
}

fn load_global() -> Result<Vec<(PathBuf, Settings)>> {
    global_files()
        .into_iter()
        .map(|file| {
            let settings = parse_settings(read_table(&file)?, &file)?;
            Ok((file, settings))
        })
        .collect()
}

fn read_table(file: &Path) -> Result<toml::Table> {
    let text = fs::read_to_string(file).with_context(|| t!("lecture de {}", "reading {}", file.display()))?;
    text.parse::<toml::Table>()
        .with_context(|| t!("{} n'est pas un TOML valide", "{} is not valid TOML", file.display()))
}

fn parse_settings(table: toml::Table, file: &Path) -> Result<Settings> {
    let settings = deserialize(table, file)?;
    match problem(&settings) {
        Some(problem) => Err(problem.error(file)),
        None => Ok(settings),
    }
}

/// The settings of one file, every member's role aside.
fn deserialize(table: toml::Table, file: &Path) -> Result<Settings> {
    let settings: Settings = toml::Value::Table(table)
        .try_into()
        .with_context(|| t!("configuration invalide dans {}", "invalid configuration in {}", file.display()))?;
    // Relative, it would not name the same folder for recruit, launched from a subfolder, and for the members,
    // launched at the project's root.
    if let Some(dir) = &settings.claude.config_dir
        && !expand_home(dir).is_absolute()
    {
        bail!(t!(
            "{} : [claude] config_dir doit être un chemin absolu ou commencer par ~/ (« {} »)",
            "{}: [claude] config_dir must be an absolute path or start with ~/ (\"{}\")",
            file.display(),
            dir
        ));
    }
    for (name, team) in &settings.teams {
        if let Some(AutoCompact::At(at)) = team.auto_compact
            && !(1..=100).contains(&at)
        {
            bail!(t!(
                "{} : auto_compact de l'équipe « {} » doit être true, false ou un pourcentage de 1 à 100 (« {} »)",
                "{}: auto_compact of team \"{}\" must be true, false or a percentage from 1 to 100 (\"{}\")",
                file.display(),
                name,
                at
            ));
        }
        // A tab with the title of the ones recruit makes would make two windows of one title.
        if let Some((member, tab)) = team
            .members
            .iter()
            .find_map(|(m, member)| member.tab.as_deref().filter(|tab| reserved_title(tab)).map(|tab| (m, tab)))
        {
            bail!(t!(
                "{} : l'onglet « {} » du membre « {} » de l'équipe « {} » porte le titre d'un onglet de recruit : choisis-en un autre",
                "{}: tab \"{}\" of member \"{}\" of team \"{}\" has the title of one of recruit's tabs: pick another",
                file.display(),
                tab,
                member,
                name
            ));
        }
    }
    Ok(settings)
}

/// Deep merge: tables merge key by key, any other value from `over` replaces the one in `base`.
pub fn merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Writes a team into the project's `.recruit/settings.toml`, created if needed with its `.gitignore`.
pub fn save_local(root: &Path, name: &str, team: &Team, replace: bool) -> Result<PathBuf> {
    let dir = root.join(LOCAL_DIR);
    fs::create_dir_all(&dir).with_context(|| t!("création de {}", "creating {}", dir.display()))?;
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, format!("{SETTINGS_LOCAL}\n"))?;
    }
    let header = t!(
        "# Équipes recruit de ce projet. Ce fichier se commite.\n# Réglages personnels, non commités : .recruit/settings.local.toml (ils priment sur celui-ci).\n",
        "# recruit teams of this project. Commit this file.\n# Personal settings, not committed: .recruit/settings.local.toml (they take precedence over this one).\n"
    );
    let file = dir.join(SETTINGS);
    write_team(&file, &header, name, team, replace)?;
    Ok(file)
}

pub fn save_global(name: &str, team: &Team, replace: bool) -> Result<PathBuf> {
    let dir = global_dir();
    if let Some(other) = defined_before(&files_in(&dir), &global_file(name), name) {
        bail!(t!(
            "l'équipe « {0} » est déjà définie dans {1} : choisis un autre nom, ou modifie ce fichier (recruit edit {0})",
            "team \"{0}\" is already defined in {1}: pick another name, or edit that file (recruit edit {0})",
            name,
            tilde(&other)
        ));
    }
    fs::create_dir_all(&dir).with_context(|| t!("création de {}", "creating {}", dir.display()))?;
    let header = t!(
        "# Équipe recruit globale « {name} ». Lancement : recruit {name}\n",
        "# Global recruit team \"{name}\". Launch it with: recruit {name}\n"
    );
    let file = global_file(name);
    write_team(&file, &header, name, team, replace)?;
    Ok(file)
}

/// Another of the global `files` that defines team `name`, written to `file`: the global teams are read file by file,
/// the first one kept, so the team written there would be hidden, or would hide this one.
fn defined_before(files: &[PathBuf], file: &Path, name: &str) -> Option<PathBuf> {
    files.iter().find(|f| f.as_path() != file && defines(f, name)).cloned()
}

/// Appends `[teams.<name>]` to the file and leaves the rest as it was, comments included.
fn write_team(file: &Path, header: &str, name: &str, team: &Team, replace: bool) -> Result<()> {
    let mut text = if file.is_file() { fs::read_to_string(file)? } else { header.to_string() };
    let mut doc: toml_edit::DocumentMut =
        text.parse().with_context(|| t!("{} n'est pas un TOML valide", "{} is not valid TOML", file.display()))?;
    let exists = doc.get("teams").and_then(|t| t.get(name)).is_some();
    if exists {
        if !replace {
            bail!(t!("l'équipe « {} » existe déjà dans {}", "team \"{}\" already exists in {}", name, file.display()));
        }
        doc["teams"].as_table_like_mut().expect("teams is a table").remove(name);
        text = doc.to_string();
    }
    if !text.is_empty() && !text.ends_with("\n\n") {
        text.push_str(if text.ends_with('\n') { "\n" } else { "\n\n" });
    }
    text.push_str(&team_toml(name, team)?);
    // Never leave a file recruit could not read back.
    let check = text.parse::<toml::Table>().context("generated TOML")?;
    parse_settings(check, file)?;
    write_atomic(file, &text)
}

/// `[teams.<name>]` and its members, as TOML text.
pub fn team_toml(name: &str, team: &Team) -> Result<String> {
    let mut settings = Settings::default();
    settings.teams.insert(name.to_string(), team.clone());
    Ok(toml::to_string_pretty(&settings)?)
}

/// Team names become file names, and the names of their sessions and sockets.
#[cfg(test)]
mod tests {
    use super::*;

    fn member(role: &str) -> Member {
        Member { role: role.into(), ..Default::default() }
    }

    #[test]
    fn local_overrides_shared() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(LOCAL_DIR);
        fs::create_dir_all(&local).unwrap();
        fs::write(
            local.join(SETTINGS),
            r#"
[teams.perso]
model = "opus"
[teams.perso.members.coordinateur]
role = "Coordonne"
instructions = "partagé"
[teams.perso.members.dev]
role = "Code"
"#,
        )
        .unwrap();
        fs::write(
            local.join(SETTINGS_LOCAL),
            r#"
[teams.perso]
model = "sonnet"
[teams.perso.members.coordinateur]
instructions = "personnel"
[teams.perso.members.designer]
role = "Maquettes"
"#,
        )
        .unwrap();
        let sub = dir.path().join("src/deep");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_local_root(&sub).unwrap(), dir.path());

        let settings = load_local(dir.path()).unwrap();
        let team = &settings.teams["perso"];
        assert_eq!(team.model.as_deref(), Some("sonnet"));
        let names: Vec<_> = team.members.keys().map(String::as_str).collect();
        assert_eq!(names, ["coordinateur", "dev", "designer"]);
        assert_eq!(team.members["coordinateur"].role, "Coordonne");
        assert_eq!(team.members["coordinateur"].instructions.as_deref(), Some("personnel"));
    }

    #[test]
    fn unknown_keys_and_missing_roles_are_errors() {
        let bad = "[teams.a]\ncolour = 'red'\n".parse::<toml::Table>().unwrap();
        assert!(parse_settings(bad, Path::new("x")).is_err());
        let no_role = "[teams.a.members.b]\nmodel = 'opus'\n".parse::<toml::Table>().unwrap();
        assert!(parse_settings(no_role, Path::new("x")).is_err());
    }

    #[test]
    fn a_global_team_another_file_defines_is_not_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let (mine, own) = (dir.path().join("perso.toml"), dir.path().join("web.toml"));
        fs::write(&mine, "[teams.web.members.lead]\nrole = 'Lead'\n").unwrap();
        let files = files_in(dir.path());
        assert_eq!(defined_before(&files, &own, "web"), Some(mine.clone()));
        assert_eq!(defined_before(&files, &own, "api"), None);
        // Its own file is the one to replace.
        fs::rename(&mine, &own).unwrap();
        assert_eq!(defined_before(&files_in(dir.path()), &own, "web"), None);
    }

    #[test]
    fn a_tmux_section_is_an_unknown_key() {
        // recruit 1's section: refused like any other, its file named.
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(LOCAL_DIR);
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(SETTINGS), "[tmux]\nsocket = 'x'\n\n[teams.a.members.b]\nrole = 'B'\n").unwrap();
        let error = format!("{:#}", load_local(dir.path()).unwrap_err());
        assert!(error.contains(&local.join(SETTINGS).display().to_string()) && error.contains("tmux"), "{error}");
    }

    #[test]
    fn errors_name_their_file() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(LOCAL_DIR);
        fs::create_dir_all(&local).unwrap();
        let error = |shared: &str, personal: &str| {
            fs::write(local.join(SETTINGS), shared).unwrap();
            fs::write(local.join(SETTINGS_LOCAL), personal).unwrap();
            format!("{:#}", load_local(dir.path()).unwrap_err())
        };
        let team = "[teams.a.members.b]\nrole = 'B'\n";
        let shared = local.join(SETTINGS).display().to_string();
        let personal = local.join(SETTINGS_LOCAL).display().to_string();

        let relative = error(team, "[claude]\nconfig_dir = 'perso'\n");
        assert!(relative.contains(&personal), "{relative}");
        let unknown = error(team, "[teams.a]\ncolour = 'red'\n");
        assert!(unknown.contains(&personal), "{unknown}");
        let added = error(team, "[teams.a.members.c]\nmodel = 'opus'\n");
        assert!(added.contains(&personal), "{added}");
        let unknown = error("[teams.a]\ncolour = 'red'\n", team);
        assert!(unknown.contains(&shared) && !unknown.contains(&personal), "{unknown}");
        let roleless = error("[teams.a.members.b]\nmodel = 'opus'\n", "[teams.a.members.b]\neffort = 'high'\n");
        assert!(roleless.contains(&shared) && !roleless.contains(&personal), "{roleless}");
    }

    #[test]
    fn names_written_by_hand_follow_the_rules() {
        let read = |text: &str| parse_settings(text.parse::<toml::Table>().unwrap(), Path::new("/x/web.toml"));
        let refused = |text: &str| format!("{:#}", read(text).unwrap_err());
        let quoted = |name: &str| t!("« {} »", "\"{}\"", name);
        let with_member =
            |name: &str| format!("[teams.web.members.lead]\nrole = 'L'\n[teams.web.members.\"{name}\"]\nrole = 'R'\n");

        for name in ["-x", ".x", "a/b", "../../evil", "dev back", ""] {
            let error = refused(&with_member(name));
            assert!(error.starts_with("/x/web.toml"), "{error}");
            assert!(error.contains(&quoted(name)) && error.contains(&quoted("web")), "{name}: {error}");
        }
        let twins = refused("[teams.web.members.Dev]\nrole = 'D'\n[teams.web.members.dev]\nrole = 'd'\n");
        assert!(twins.contains(&quoted("dev")) && twins.contains(&quoted("Dev")), "the second, and its twin: {twins}");
        for team in ["list", "_panel", "-web", "my team", "a/b", "a.b"] {
            let error = refused(&format!("[teams.\"{team}\".members.lead]\nrole = 'L'\n"));
            assert!(error.contains(&quoted(team)) && !error.contains("lead"), "{team}: {error}");
        }
        // What the rules let through.
        let fine = "[teams.\"équipe_2\".members.\"rédacteur\"]\nrole = 'R'\n[teams.\"équipe_2\".members.\"dev.api\"]\nrole = 'A'\n\
                    [teams.\"équipe_2\".members.Dev]\nrole = 'D'\n";
        assert!(read(fine).is_ok());

        // A member of each file, a case apart: the file that adds the second one is the one to change.
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(LOCAL_DIR);
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(SETTINGS), "[teams.web.members.Dev]\nrole = 'D'\n").unwrap();
        fs::write(local.join(SETTINGS_LOCAL), "[teams.web.members.dev]\nrole = 'd'\n").unwrap();
        let error = format!("{:#}", load_local(dir.path()).unwrap_err());
        assert!(error.starts_with(&local.join(SETTINGS_LOCAL).display().to_string()), "{error}");
    }

    #[test]
    fn main_contact_keeps_its_bars() {
        let team = |marks: &[(&str, bool)]| Team {
            members: marks
                .iter()
                .map(|&(n, contact)| (n.to_string(), Member { contact, ..Default::default() }))
                .collect(),
            ..Default::default()
        };
        let quiet = |t: &Team| t.members.keys().filter(|n| t.quiet(n)).cloned().collect::<Vec<_>>();
        // No contact marked: the first member is the one.
        assert_eq!(quiet(&team(&[("lead", false), ("dev", false), ("qa", false)])), ["dev", "qa"]);
        // The contact, wherever it stands.
        assert_eq!(quiet(&team(&[("dev", false), ("lead", true), ("qa", false)])), ["dev", "qa"]);
        // Two contacts: the first keeps its bars, the second goes quiet too.
        assert_eq!(quiet(&team(&[("dev", false), ("lead", true), ("pm", true)])), ["dev", "pm"]);
    }

    #[test]
    fn contacts_at_most_two() {
        let team = |marks: &str| format!("[teams.a]\ndashboard = false\n{marks}").parse::<toml::Table>().unwrap();
        let three = "[teams.a.members.x]\nrole='X'\ncontact=true\n[teams.a.members.y]\nrole='Y'\ncontact=true\n\
                     [teams.a.members.z]\nrole='Z'\ncontact=true\n";
        let error = format!("{:#}", parse_settings(team(three), Path::new("x")).unwrap_err());
        assert!(error.contains('3'), "{error}");
        let two = "[teams.a.members.x]\nrole='X'\ncontact=true\n[teams.a.members.y]\nrole='Y'\ncontact=true\n";
        let settings = parse_settings(team(two), Path::new("x")).unwrap();
        assert_eq!(settings.teams["a"].dashboard, Some(false));

        // Two shared, one more personal: the personal file is the one to change.
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(LOCAL_DIR);
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(SETTINGS), format!("[teams.a]\n{two}[teams.a.members.z]\nrole='Z'\n")).unwrap();
        fs::write(local.join(SETTINGS_LOCAL), "[teams.a.members.z]\ncontact = true\n").unwrap();
        let error = format!("{:#}", load_local(dir.path()).unwrap_err());
        assert!(error.contains(&local.join(SETTINGS_LOCAL).display().to_string()), "{error}");
    }

    #[test]
    fn tabs_of_recruit_are_refused() {
        for tab in ["Interlocuteurs", "Contacts", "Agents (2)", " Agents (12) "] {
            let file = format!("[teams.a.members.b]\nrole = 'B'\ntab = '{tab}'\n").parse::<toml::Table>().unwrap();
            let error = format!("{:#}", parse_settings(file, Path::new("/x/web.toml")).unwrap_err());
            assert!(error.starts_with("/x/web.toml") && error.contains(tab.trim()), "{error}");
        }
        for tab in ["Agents", "Agents (x)", "Code", "agents", "Agents (1) bis"] {
            let file = format!("[teams.a.members.b]\nrole = 'B'\ntab = '{tab}'\n").parse::<toml::Table>().unwrap();
            assert!(parse_settings(file, Path::new("x")).is_ok(), "{tab}");
        }
        // "Agents" is the default group: with the agents that have no tab, in one window.
        let team = "[teams.a.members.lead]\nrole = 'L'\n[teams.a.members.b]\nrole = 'B'\ntab = 'Agents'\n\
                    [teams.a.members.c]\nrole = 'C'\n";
        let settings = parse_settings(team.parse::<toml::Table>().unwrap(), Path::new("x")).unwrap();
        let tabs = crate::team::layout::tabs(&settings.teams["a"]);
        assert_eq!(tabs.len(), 2);
        assert_eq!((tabs[1].title.as_str(), tabs[1].members.len()), ("Agents", 2));
    }

    #[test]
    fn auto_compaction_share() {
        let team = |value: &str| {
            let file = format!("[teams.a]\n{value}\n[teams.a.members.b]\nrole = 'B'\n");
            parse_settings(file.parse::<toml::Table>().unwrap(), Path::new("/x/web.toml"))
        };
        for (value, at) in [("", Some(70)), ("auto_compact = true", Some(70)), ("auto_compact = false", None)] {
            assert_eq!(team(value).unwrap().teams["a"].auto_compact_at(), at, "{value}");
        }
        assert_eq!(team("auto_compact = 85").unwrap().teams["a"].auto_compact_at(), Some(85));
        for value in ["auto_compact = 0", "auto_compact = 101", "auto_compact = -5"] {
            let error = format!("{:#}", team(value).unwrap_err());
            assert!(error.starts_with("/x/web.toml") && error.contains("auto_compact"), "{error}");
        }
        for value in ["auto_compact = 'oui'", "auto_compact = 1.5"] {
            let error = format!("{:#}", team(value).unwrap_err());
            let expected = t!("true, false ou un pourcentage de 1 à 100", "true, false or a percentage from 1 to 100");
            assert!(error.contains(&expected) && !error.contains("untagged"), "{error}");
        }
    }

    #[test]
    fn claude_profile() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(LOCAL_DIR);
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(SETTINGS), "[claude]\ncommand = 'claude'\n[teams.a.members.b]\nrole = 'B'\n").unwrap();
        assert_eq!(load_local(dir.path()).unwrap().claude.config_dir(), None);

        fs::write(local.join(SETTINGS_LOCAL), "[claude]\nconfig_dir = '~/.claude-perso'\n").unwrap();
        let claude = load_local(dir.path()).unwrap().claude;
        assert_eq!(claude.command(), "claude");
        assert_eq!(claude.config_dir(), Some(home().join(".claude-perso")));

        let relative = "[claude]\nconfig_dir = '.claude-perso'\n".parse::<toml::Table>().unwrap();
        assert!(parse_settings(relative, Path::new("x")).is_err());
        let absolute = "[claude]\nconfig_dir = '/opt/claude'\n".parse::<toml::Table>().unwrap();
        let settings = parse_settings(absolute, Path::new("x")).unwrap();
        assert_eq!(settings.claude.config_dir(), Some(PathBuf::from("/opt/claude")));
    }

    #[test]
    fn save_appends_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let mut team = Team { description: Some("Petite équipe".into()), ..Default::default() };
        team.members.insert("coordinateur".into(), member("Coordonne\nsur deux lignes"));
        team.members.insert("développeur".into(), member("Code"));
        let file = save_local(dir.path(), "perso", &team, false).unwrap();
        assert!(dir.path().join(".recruit/.gitignore").is_file());

        let mut text = fs::read_to_string(&file).unwrap();
        text.push_str("# ma note\n");
        fs::write(&file, text).unwrap();

        let other = Team { members: [("x".to_string(), member("X"))].into_iter().collect(), ..Default::default() };
        save_local(dir.path(), "autre", &other, false).unwrap();
        assert!(save_local(dir.path(), "perso", &other, false).is_err());

        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("# ma note"));
        assert!(!text.contains("\n[teams]\n"), "{text}");
        let settings = load_local(dir.path()).unwrap();
        assert_eq!(settings.teams["perso"], team);
        assert_eq!(settings.teams["autre"], other);

        save_local(dir.path(), "perso", &other, true).unwrap();
        let settings = load_local(dir.path()).unwrap();
        assert_eq!(settings.teams["perso"], other);
        assert!(fs::read_to_string(&file).unwrap().contains("# ma note"));
    }

    #[test]
    fn names() {
        assert!(validate_team_name("omnidex").is_ok());
        assert!(validate_team_name("équipe_2").is_ok());
        assert!(validate_team_name("list").is_err());
        assert!(validate_team_name("a.b").is_err());
        assert!(validate_team_name("").is_err());
        assert!(validate_member_name("opérateur-ui-ux").is_ok());
        assert!(validate_member_name("dev back").is_err());
        assert!(validate_member_name("-x").is_err());
    }
}
