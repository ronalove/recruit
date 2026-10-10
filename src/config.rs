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
use crate::t;

mod edit;

pub use edit::{
    Edit, Field, Layered, MemberLayers, Source, check_new_name, dashboard_layers, edit, member_layers, preview,
};

pub const LOCAL_DIR: &str = ".recruit";
pub const SETTINGS: &str = "settings.toml";
pub const SETTINGS_LOCAL: &str = "settings.local.toml";

/// The members the user talks to share the first tab, next to the team's dashboard: two at most.
pub const MAX_CONTACTS: usize = 2;

/// Subcommand names: a team cannot take them, `recruit <name>` would never reach it.
pub const RESERVED: &[&str] = &[
    "new",
    "list",
    "attach",
    "stop",
    "edit",
    "templates",
    "help",
    "_panel",
    "_mod",
    "_member",
    "_click",
    "_menu",
    "_edit",
];

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Team launched by a bare `recruit` when the file holds several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "ClaudeSettings::is_empty")]
    pub claude: ClaudeSettings,
    #[serde(default, skip_serializing_if = "TmuxSettings::is_empty")]
    pub tmux: TmuxSettings,
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
/// The `[tmux]` section of recruit 1, which ran its teams in tmux: its keys are accepted and have no effect, with a
/// warning at launch. Only `socket` still serves, to find a team still running in recruit 1's tmux server.
pub struct TmuxSettings {
    /// Name of recruit 1's tmux server (`tmux -L <socket>`), `recruit` by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    /// Mouse support in recruit 1's tmux server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse: Option<bool>,
    /// Whether recruit 1's tmux server read the user's tmux configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_config: Option<bool>,
    /// Extra tmux commands for recruit 1's tmux server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

impl TmuxSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn socket(&self) -> &str {
        self.socket.as_deref().unwrap_or("recruit")
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
    #[serde(default)]
    pub members: IndexMap<String, Member>,
}

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
    /// The contacts in a first tab, then the working agents in tabs of six at most. "grid", from 0.1, is the same.
    #[default]
    #[serde(alias = "grid")]
    Auto,
    /// One tab per member, contacts first.
    Tabs,
}

impl Team {
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
    pub tmux: TmuxSettings,
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
            tmux: settings.tmux.clone(),
        })
        .collect()
}

/// The nearest directory, from `start` up, that holds `.recruit/settings.toml` or `.recruit/settings.local.toml`.
pub fn find_local_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        let local = dir.join(LOCAL_DIR);
        (local.join(SETTINGS).is_file() || local.join(SETTINGS_LOCAL).is_file()).then(|| dir.to_path_buf())
    })
}

/// The file of team `name`, as `Catalog::find` gives it, found in the plain TOML without checking the settings: for
/// `recruit edit` when the team files do not read. The project's `settings.toml` when one of its files has the team,
/// else the global file that has it, else the global file of its name.
pub fn team_file_unchecked(cwd: &Path, name: &str) -> Option<PathBuf> {
    let has = |file: &Path| {
        read_table(file)
            .is_ok_and(|t| t.get("teams").and_then(toml::Value::as_table).is_some_and(|t| t.contains_key(name)))
    };
    if let Some(root) = find_local_root(cwd) {
        let dir = root.join(LOCAL_DIR);
        if has(&dir.join(SETTINGS)) || has(&dir.join(SETTINGS_LOCAL)) {
            return Some(dir.join(SETTINGS));
        }
    }
    global_files().into_iter().find(|f| has(f)).or_else(|| Some(global_file(name)).filter(|f| f.is_file()))
}

/// `settings.toml`, with `settings.local.toml` merged over it.
pub fn load_local(root: &Path) -> Result<Settings> {
    let dir = root.join(LOCAL_DIR);
    let mut tables = Vec::new();
    for file in [dir.join(SETTINGS), dir.join(SETTINGS_LOCAL)] {
        if file.is_file() {
            tables.push((read_table(&file)?, file));
        }
    }
    merge_local(&dir, tables)
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
    if let Some(bad) = bad_name(&settings) {
        // The file that defines the team or the member, the shared one first.
        let file = files
            .iter()
            .find(|(s, _)| s.teams.get(bad.team).is_some_and(|t| bad.member.is_none_or(|m| t.members.contains_key(m))))
            .map_or_else(|| dir.join(SETTINGS), |(_, f)| f.clone());
        return Err(bad.error(&file));
    }
    if let Some((team, member)) = missing_role(&settings) {
        // The file that defines the member, the shared one first.
        let (_, file) = files
            .iter()
            .find(|(s, _)| s.teams.get(team).is_some_and(|t| t.members.contains_key(member)))
            .expect("a merged member comes from one of the files");
        return Err(no_role(file, team, member));
    }
    if let Some((team, count)) = too_many_contacts(&settings) {
        // The last file that marks one of them.
        let file = files
            .iter()
            .rev()
            .find(|(s, _)| s.teams.get(team).is_some_and(|t| t.members.values().any(|m| m.contact)))
            .map_or_else(|| dir.join(SETTINGS), |(_, f)| f.clone());
        return Err(contacts_error(&file, team, count));
    }
    Ok(settings)
}

pub fn global_dir() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("recruit"),
        _ => home().join(".config").join("recruit"),
    }
}

/// Generated files: the members' prompts, the mod, the running teams' state.
pub fn cache_dir() -> PathBuf {
    match std::env::var_os("XDG_CACHE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("recruit"),
        _ => home().join(".cache").join("recruit"),
    }
}

pub fn global_file(team: &str) -> PathBuf {
    global_dir().join(format!("{team}.toml"))
}

/// The global teams' files, in order.
fn global_files() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(global_dir()) else { return Vec::new() };
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
    if let Some(bad) = bad_name(&settings) {
        return Err(bad.error(file));
    }
    if let Some((team, member)) = missing_role(&settings) {
        return Err(no_role(file, team, member));
    }
    if let Some((team, count)) = too_many_contacts(&settings) {
        return Err(contacts_error(file, team, count));
    }
    Ok(settings)
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
    // A tab with the title of the ones recruit makes would make two windows of one title.
    for (name, team) in &settings.teams {
        if let Some((member, tab)) = team.members.iter().find_map(|(m, member)| {
            member.tab.as_deref().filter(|tab| crate::layout::reserved_title(tab)).map(|tab| (m, tab))
        }) {
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

/// The first member without a role, as (team, member).
/// A team or member name recruit would refuse when it asks for one: the first in the settings, and why.
struct BadName<'a> {
    team: &'a str,
    member: Option<&'a str>,
    why: String,
}

impl BadName<'_> {
    fn error(&self, file: &Path) -> anyhow::Error {
        match self.member {
            None => anyhow::anyhow!(t!(
                "{} : l'équipe « {} » ne peut pas porter ce nom : {}. Renomme-la dans le fichier.",
                "{}: team \"{}\" cannot have this name: {}. Rename it in the file.",
                file.display(),
                self.team,
                self.why
            )),
            Some(member) => anyhow::anyhow!(t!(
                "{} : le membre « {} » de l'équipe « {} » ne peut pas porter ce nom : {}. Renomme-le dans le fichier.",
                "{}: member \"{}\" of team \"{}\" cannot have this name: {}. Rename it in the file.",
                file.display(),
                member,
                self.team,
                self.why
            )),
        }
    }
}

/// The rules of the names recruit asks for, on the names of a file written by hand: a member named `-x` could not
/// start (`recruit _member … -x`), `a/b` or `..` would put its files outside its team's folder, `Dev` and `dev` would
/// share theirs on a filesystem that ignores case, and a team named `list` could not be launched by name.
fn bad_name(settings: &Settings) -> Option<BadName<'_>> {
    for (team, members) in settings.teams.iter().map(|(name, team)| (name.as_str(), &team.members)) {
        if let Err(why) = validate_team_name(team) {
            return Some(BadName { team, member: None, why });
        }
        let mut seen: Vec<(String, &str)> = Vec::new();
        for member in members.keys().map(String::as_str) {
            let lower = member.to_lowercase();
            let why = match (validate_member_name(member), seen.iter().find(|(l, _)| *l == lower)) {
                (Err(why), _) => why,
                (Ok(()), Some((_, twin))) => t!(
                    "« {} » ne diffère de « {} » que par la casse",
                    "\"{}\" differs from \"{}\" by case only",
                    member,
                    twin
                ),
                (Ok(()), None) => {
                    seen.push((lower, member));
                    continue;
                }
            };
            return Some(BadName { team, member: Some(member), why });
        }
    }
    None
}

fn missing_role(settings: &Settings) -> Option<(&str, &str)> {
    settings.teams.iter().find_map(|(name, team)| {
        team.members.iter().find(|(_, m)| m.role.trim().is_empty()).map(|(member, _)| (name.as_str(), member.as_str()))
    })
}

fn no_role(file: &Path, team: &str, member: &str) -> anyhow::Error {
    anyhow::anyhow!(t!(
        "{} : le membre « {} » de l'équipe « {} » n'a pas de rôle",
        "{}: member \"{}\" of team \"{}\" has no role",
        file.display(),
        member,
        team
    ))
}

/// The first team with more than `MAX_CONTACTS` contacts, and how many it has.
fn too_many_contacts(settings: &Settings) -> Option<(&str, usize)> {
    settings.teams.iter().find_map(|(name, team)| {
        let count = team.contacts().len();
        (count > MAX_CONTACTS).then_some((name.as_str(), count))
    })
}

fn contacts_error(file: &Path, team: &str, count: usize) -> anyhow::Error {
    anyhow::anyhow!(t!(
        "{} : l'équipe « {} » a {} interlocuteurs (contact = true), {} au plus. Les autres peuvent devenir des agents de travail.",
        "{}: team \"{}\" has {} contacts (contact = true), {} at most. The others can become working agents.",
        file.display(),
        team,
        count,
        MAX_CONTACTS
    ))
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
    fs::create_dir_all(&dir).with_context(|| t!("création de {}", "creating {}", dir.display()))?;
    let header = t!(
        "# Équipe recruit globale « {name} ». Lancement : recruit {name}\n",
        "# Global recruit team \"{name}\". Launch it with: recruit {name}\n"
    );
    let file = global_file(name);
    write_team(&file, &header, name, team, replace)?;
    Ok(file)
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

/// Writes a file in one go: a reader sees it as it was or as it is, never half written. A file that was there keeps
/// its permissions; a link stays a link, its target written (`write_temporary`).
pub fn write_atomic(file: &Path, text: &str) -> Result<()> {
    let (temporary, target) = write_temporary(file, text)?;
    fs::rename(&temporary, &target).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        anyhow::Error::new(error).context(t!("écriture de {}", "writing {}", file.display()))
    })
}

/// Writes `text` in a temporary file for a rename over `file`: beside the file it names, a link followed (the
/// dotfiles of a global team are often links), with that file's permissions if it exists. Returns the temporary file
/// and the one to rename it over.
pub fn write_temporary(file: &Path, text: &str) -> Result<(PathBuf, PathBuf)> {
    let target = fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = fs::write(&temporary, text).and_then(|()| match fs::metadata(&target) {
        Ok(meta) => fs::set_permissions(&temporary, meta.permissions()),
        Err(_) => Ok(()),
    });
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| t!("écriture de {}", "writing {}", file.display()));
    }
    Ok((temporary, target))
}

/// `[teams.<name>]` and its members, as TOML text.
pub fn team_toml(name: &str, team: &Team) -> Result<String> {
    let mut settings = Settings::default();
    settings.teams.insert(name.to_string(), team.clone());
    Ok(toml::to_string_pretty(&settings)?)
}

/// Team names become file names, and the names of their sessions and sockets.
pub fn validate_team_name(name: &str) -> Result<(), String> {
    let ok_chars = name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    if name.is_empty() || name.chars().count() > 40 || !ok_chars || name.starts_with('-') {
        return Err(t!(
            "lettres, chiffres, - et _ seulement, sans - au début, 40 caractères au plus",
            "letters, digits, - and _ only, not starting with -, at most 40 characters"
        ));
    }
    if RESERVED.contains(&name) {
        return Err(t!("« {} » est une commande de recruit", "\"{}\" is a recruit command", name));
    }
    Ok(())
}

/// Member names are the addresses teammates write to.
pub fn validate_member_name(name: &str) -> Result<(), String> {
    let ok_chars = name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if name.is_empty() || name.chars().count() > 40 || !ok_chars || name.starts_with(['-', '.']) {
        return Err(t!(
            "lettres, chiffres, -, _ et . seulement, sans espace, sans - ni . au début, 40 caractères au plus",
            "letters, digits, -, _ and . only, no spaces, not starting with - or ., at most 40 characters"
        ));
    }
    Ok(())
}

pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => PathBuf::from(path),
    }
}

/// `~/x` rather than `/Users/me/x`, for messages.
pub fn tilde(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

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
        let tabs = crate::layout::tabs(&settings.teams["a"]);
        assert_eq!(tabs.len(), 2);
        assert_eq!((tabs[1].title.as_str(), tabs[1].members.len()), ("Agents", 2));
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
