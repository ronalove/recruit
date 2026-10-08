// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Changes to a team's files, made from a running team's menu: comments and layout kept, the changes of one call
//! written together, and only once the configuration they give reads back.
//!
//! A member's setting is read in layers, the first that gives it wins: the member in `settings.local.toml`, the
//! member in `settings.toml` (or in a global team's file), the team itself for the model, the effort and the
//! permission mode, then recruit's or Claude Code's default. A change goes into the file that gives the value now:
//! the personal file if it has the key, else the shared one if it has it, else the member's file, the shared one
//! first. When the value asked is what the layers under that file give, the key leaves the file, and a table left
//! empty goes with it; otherwise the value is written there. No other file is touched. The team's `dashboard` goes
//! the same way, the team's file in place of the member's. Renaming and removing a member act on every file that
//! defines it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use toml_edit::{DocumentMut, InlineTable, Item, Key, Table, TableLike, Value};

use super::{Found, LOCAL_DIR, Member, Origin, SETTINGS, SETTINGS_LOCAL, Team, merge_local, parse_settings, tilde};
use super::{validate_member_name, write_atomic, write_temporary};
use crate::t;

/// A member's setting and the value asked for it. `None` (and `false` for `Contact`) asks for the default: what the
/// layers under the file that gives the value now give.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Role(String),
    Instructions(Option<String>),
    /// The `contact` key, not whether the member is one of the contacts: with no member marked, the first one is.
    Contact(bool),
    Tab(Option<String>),
    PermissionMode(Option<String>),
    Model(Option<String>),
    Effort(Option<String>),
}

/// A change to a team, as `edit` takes them; in JSON for `recruit _edit`: `{"set": {"member": "dev", "field":
/// {"model": "opus"}}}`, `{"rename": {"from": "a", "to": "b"}}`, `{"add": {"name": "x", "member": {"role": "…"}}}`,
/// `{"remove": "dev"}`, `{"dashboard": false}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edit {
    Set {
        member: String,
        field: Field,
    },
    /// Keeps the member's place among the others, and its comments.
    Rename {
        from: String,
        to: String,
    },
    /// Into the file that defines the team, the shared one first.
    Add {
        name: String,
        member: Member,
    },
    Remove(String),
    /// The dashboard and the journal; on by default.
    Dashboard(bool),
}

/// The layer a value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The project's personal file, `.recruit/settings.local.toml`.
    Personal,
    /// The project's shared file, `.recruit/settings.toml`, or a global team's file.
    Shared,
    /// The team's own value, `[teams.<team>]`: model, effort and permission mode only.
    Team,
    /// recruit's default, or Claude Code's.
    Default,
}

/// A setting as the menu shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Layered<T> {
    pub value: T,
    pub source: Source,
    /// What the default would give: the value under the layer that gives it now, and its layer. None for a role
    /// written in one file only: it has no default.
    pub fallback: Option<(T, Source)>,
}

impl<T> Layered<T> {
    fn map<U>(self, f: impl Fn(T) -> U) -> Layered<U> {
        Layered { value: f(self.value), source: self.source, fallback: self.fallback.map(|(v, s)| (f(v), s)) }
    }
}

/// Each of a member's settings, layered.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberLayers {
    pub role: Layered<String>,
    pub instructions: Layered<Option<String>>,
    pub contact: Layered<bool>,
    pub tab: Layered<Option<String>>,
    pub permission_mode: Layered<Option<String>>,
    pub model: Layered<Option<String>>,
    pub effort: Layered<Option<String>>,
}

/// Applies `edits` in turn, then writes the files they changed, each in one go, once the configuration they give
/// reads back; nothing is written otherwise, nor when nothing changes. Returns the team as written.
pub fn edit(origin: &Origin, team: &str, edits: &[Edit]) -> Result<Found> {
    let mut files = Files::read(origin)?;
    for edit in edits {
        files.apply(team, edit)?;
    }
    files.save(team, |from, to| fs::rename(from, to))?;
    origin.load(team)
}

/// The team as `edit` would write it, checked the same way; nothing written.
pub fn preview(origin: &Origin, team: &str, edits: &[Edit]) -> Result<Found> {
    let mut files = Files::read(origin)?;
    for edit in edits {
        files.apply(team, edit)?;
    }
    let (settings, _) = files.check(team)?;
    origin.found(&settings, team)
}

/// A member's settings, where each one comes from, and what its default would be.
pub fn member_layers(origin: &Origin, team: &str, member: &str) -> Result<MemberLayers> {
    let files = Files::read(origin)?;
    files.known(team, member)?;
    let spot = Spot::Member(member);
    let text = |kind| -> Result<Layered<Option<String>>> { Ok(files.layered(team, &spot, kind)?.map(text_of)) };
    Ok(MemberLayers {
        role: text(Kind::Role)?.map(Option::unwrap_or_default),
        instructions: text(Kind::Instructions)?,
        contact: files.layered(team, &spot, Kind::Contact)?.map(|v| v == Some(Plain::Bool(true))),
        tab: text(Kind::Tab)?,
        permission_mode: text(Kind::PermissionMode)?,
        model: text(Kind::Model)?,
        effort: text(Kind::Effort)?,
    })
}

/// Whether the team has its dashboard, where that comes from, and what its default would be.
pub fn dashboard_layers(origin: &Origin, team: &str) -> Result<Layered<bool>> {
    let files = Files::read(origin)?;
    Ok(files.layered(team, &Spot::Team, Kind::Dashboard)?.map(|v| v != Some(Plain::Bool(false))))
}

/// A new member's name: a valid one, that no member has yet, whatever the case.
pub fn check_new_name(team: &Team, name: &str) -> Result<(), String> {
    check_name(team.members.keys().map(String::as_str), name, None)
}

/// `name` among `names`, except the member it replaces.
fn check_name<'a>(names: impl IntoIterator<Item = &'a str>, name: &str, except: Option<&str>) -> Result<(), String> {
    validate_member_name(name)?;
    let lower = name.to_lowercase();
    match names.into_iter().find(|n| Some(*n) != except && n.to_lowercase() == lower) {
        Some(taken) if taken == name => Err(t!("« {} » est déjà dans l'équipe", "\"{}\" is already in the team", name)),
        Some(taken) => Err(t!(
            "« {} » ne diffère de « {} » que par la casse",
            "\"{}\" differs from \"{}\" by case only",
            name,
            taken
        )),
        None => Ok(()),
    }
}

/// A setting's value in a file.
#[derive(Debug, Clone, PartialEq)]
enum Plain {
    Text(String),
    Bool(bool),
}

fn text_of(value: Option<Plain>) -> Option<String> {
    match value {
        Some(Plain::Text(text)) => Some(text),
        _ => None,
    }
}

impl Plain {
    fn read(item: &Item) -> Option<Plain> {
        item.as_str().map(|s| Plain::Text(s.to_string())).or_else(|| item.as_bool().map(Plain::Bool))
    }

    fn value(&self) -> Value {
        match self {
            Plain::Text(text) => Value::from(text.as_str()),
            Plain::Bool(b) => Value::from(*b),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Role,
    Instructions,
    Contact,
    Tab,
    PermissionMode,
    Model,
    Effort,
    Dashboard,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Role => "role",
            Kind::Instructions => "instructions",
            Kind::Contact => "contact",
            Kind::Tab => "tab",
            Kind::PermissionMode => "permission_mode",
            Kind::Model => "model",
            Kind::Effort => "effort",
            Kind::Dashboard => "dashboard",
        }
    }

    /// The team gives one for its members.
    fn given_by_team(self) -> bool {
        matches!(self, Kind::PermissionMode | Kind::Model | Kind::Effort)
    }

    /// Whether two values of this setting come to the same: a tab of "Agents" is the default group, as no tab.
    fn same(self, a: Option<&Plain>, b: Option<&Plain>) -> bool {
        let agents = Plain::Text("Agents".into());
        let plain = |v: Option<&Plain>| v.filter(|v| !(self == Kind::Tab && **v == agents)).cloned();
        plain(a) == plain(b)
    }

    /// recruit's default; None for a setting left to Claude Code, or absent.
    fn default(self) -> Option<Plain> {
        match self {
            Kind::Contact => Some(Plain::Bool(false)),
            Kind::Dashboard => Some(Plain::Bool(true)),
            _ => None,
        }
    }
}

impl Field {
    /// The setting, and the value asked: None for the default. Blank text is no text.
    fn split(&self) -> (Kind, Option<Plain>) {
        let text = |value: &Option<String>| {
            value.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(|v| Plain::Text(v.to_string()))
        };
        match self {
            Field::Role(role) => (Kind::Role, text(&Some(role.clone()))),
            Field::Instructions(v) => (Kind::Instructions, text(v)),
            Field::Contact(contact) => (Kind::Contact, Some(Plain::Bool(*contact))),
            Field::Tab(v) => (Kind::Tab, text(v)),
            Field::PermissionMode(v) => (Kind::PermissionMode, text(v)),
            Field::Model(v) => (Kind::Model, text(v)),
            Field::Effort(v) => (Kind::Effort, text(v)),
        }
    }
}

/// Where a setting is written: in a member's table, or in the team's.
enum Spot<'a> {
    Member(&'a str),
    Team,
}

impl Spot<'_> {
    fn path<'a>(&'a self, team: &'a str) -> Vec<&'a str> {
        match self {
            Spot::Member(member) => vec!["teams", team, "members", member],
            Spot::Team => vec!["teams", team],
        }
    }
}

struct File {
    path: PathBuf,
    source: Source,
    /// The text as read, to write only what changed.
    before: String,
    doc: DocumentMut,
}

/// A team's files, the shared one first.
struct Files {
    origin: Origin,
    list: Vec<File>,
}

impl Files {
    fn read(origin: &Origin) -> Result<Files> {
        let candidates = match origin {
            Origin::Local { root } => {
                let dir = root.join(LOCAL_DIR);
                vec![(dir.join(SETTINGS), Source::Shared), (dir.join(SETTINGS_LOCAL), Source::Personal)]
            }
            Origin::Global { file } => vec![(file.clone(), Source::Shared)],
        };
        let mut list = Vec::new();
        for (path, source) in candidates {
            if !path.is_file() {
                continue;
            }
            let before =
                fs::read_to_string(&path).with_context(|| t!("lecture de {}", "reading {}", path.display()))?;
            let doc = before
                .parse::<DocumentMut>()
                .with_context(|| t!("{} n'est pas un TOML valide", "{} is not valid TOML", path.display()))?;
            list.push(File { path, source, before, doc });
        }
        Ok(Files { origin: origin.clone(), list })
    }

    fn table(&self, i: usize, path: &[&str]) -> Option<&dyn TableLike> {
        let mut table: &dyn TableLike = self.list[i].doc.as_table();
        for key in path {
            table = table.get(key)?.as_table_like()?;
        }
        Some(table)
    }

    fn table_mut(&mut self, i: usize, path: &[&str]) -> Option<&mut dyn TableLike> {
        table_at(&mut self.list[i].doc, path)
    }

    fn get(&self, i: usize, team: &str, spot: &Spot, key: &str) -> Option<Plain> {
        self.table(i, &spot.path(team))?.get(key).and_then(Plain::read)
    }

    fn defines(&self, i: usize, team: &str, spot: &Spot) -> bool {
        self.table(i, &spot.path(team)).is_some()
    }

    /// The team's members, in the merged order: the shared file's, then those the personal file adds.
    fn members(&self, team: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for i in 0..self.list.len() {
            if let Some(members) = self.table(i, &["teams", team, "members"]) {
                for (name, _) in members.iter() {
                    if !names.iter().any(|n| n == name) {
                        names.push(name.to_string());
                    }
                }
            }
        }
        names
    }

    fn known(&self, team: &str, member: &str) -> Result<()> {
        if !self.members(team).iter().any(|m| m == member) {
            bail!(t!(
                "l'équipe « {} » n'a pas de membre « {} » ({})",
                "team \"{}\" has no member \"{}\" ({})",
                team,
                member,
                tilde(&self.origin.place())
            ));
        }
        Ok(())
    }

    /// The file a change of `kind` goes into: the last that has the key, else the first that defines the member or
    /// the team.
    fn target(&self, team: &str, spot: &Spot, kind: Kind) -> Option<usize> {
        let len = self.list.len();
        (0..len)
            .rev()
            .find(|&i| self.get(i, team, spot, kind.key()).is_some())
            .or_else(|| (0..len).find(|&i| self.defines(i, team, spot)))
    }

    /// What the layers under file `i` give, and the layer that gives it. None for a role: it has no default.
    fn below(&self, i: usize, team: &str, spot: &Spot, kind: Kind) -> Option<(Option<Plain>, Source)> {
        let key = kind.key();
        if let Some(j) = (0..i).rev().find(|&j| self.get(j, team, spot, key).is_some()) {
            return Some((self.get(j, team, spot, key), self.list[j].source));
        }
        if matches!(spot, Spot::Member(_)) && kind.given_by_team() {
            let team_value = (0..self.list.len()).rev().find_map(|j| self.get(j, team, &Spot::Team, key));
            if team_value.is_some() {
                return Some((team_value, Source::Team));
            }
        }
        (kind != Kind::Role).then(|| (kind.default(), Source::Default))
    }

    fn layered(&self, team: &str, spot: &Spot, kind: Kind) -> Result<Layered<Option<Plain>>> {
        let target = self.target(team, spot, kind).with_context(|| self.unknown(team, spot))?;
        let fallback = self.below(target, team, spot, kind);
        Ok(match self.get(target, team, spot, kind.key()) {
            Some(value) => Layered { value: Some(value), source: self.list[target].source, fallback },
            None => {
                let (value, source) = fallback.clone().unwrap_or((None, Source::Default));
                Layered { value, source, fallback }
            }
        })
    }

    fn unknown(&self, team: &str, spot: &Spot) -> String {
        let place = tilde(&self.origin.place());
        match spot {
            Spot::Member(member) => t!(
                "l'équipe « {} » n'a pas de membre « {} » ({})",
                "team \"{}\" has no member \"{}\" ({})",
                team,
                member,
                place
            ),
            Spot::Team => t!("aucune équipe « {} » dans {}", "no team \"{}\" in {}", team, place),
        }
    }

    fn apply(&mut self, team: &str, edit: &Edit) -> Result<()> {
        let place = tilde(&self.origin.place());
        match edit {
            Edit::Set { member, field } => {
                self.known(team, member)?;
                let (kind, value) = field.split();
                if kind == Kind::Role && value.is_none() {
                    bail!(t!("le rôle de « {} » ne peut pas être vide", "the role of \"{}\" cannot be empty", member));
                }
                self.set(team, &Spot::Member(member), kind, value)
            }
            Edit::Dashboard(on) => self.set(team, &Spot::Team, Kind::Dashboard, Some(Plain::Bool(*on))),
            Edit::Rename { from, to } => {
                self.known(team, from)?;
                if from == to {
                    return Ok(());
                }
                let members = self.members(team);
                check_name(members.iter().map(String::as_str), to, Some(from)).map_err(anyhow::Error::msg)?;
                for file in &mut self.list {
                    if let Some(members) = item_at(&mut file.doc, &["teams", team, "members"]) {
                        rename_key(members, from, to);
                    }
                }
                Ok(())
            }
            Edit::Add { name, member } => {
                let members = self.members(team);
                check_name(members.iter().map(String::as_str), name, None).map_err(anyhow::Error::msg)?;
                if member.role.trim().is_empty() {
                    bail!(t!("le rôle de « {} » ne peut pas être vide", "the role of \"{}\" cannot be empty", name));
                }
                let i = (0..self.list.len())
                    .find(|&i| self.defines(i, team, &Spot::Team))
                    .with_context(|| self.unknown(team, &Spot::Team))?;
                let team_table = self.table_mut(i, &["teams", team]).expect("the file defines the team");
                add_member(team_table, name, member);
                Ok(())
            }
            Edit::Remove(member) => {
                self.known(team, member)?;
                if self.members(team).len() == 1 {
                    bail!(t!(
                        "« {} » est le dernier membre de l'équipe « {} » ({}) : il reste",
                        "\"{}\" is the last member of team \"{}\" ({}): it stays",
                        member,
                        team,
                        place
                    ));
                }
                for file in &mut self.list {
                    let path = ["teams", team, "members"];
                    if remove_entry(&mut file.doc, &path, member) {
                        prune(&mut file.doc, &path);
                    }
                }
                Ok(())
            }
        }
    }

    /// Writes `value` (None: the default) where the setting is read now, or takes the key away there when the
    /// layers under it give that value.
    fn set(&mut self, team: &str, spot: &Spot, kind: Kind, value: Option<Plain>) -> Result<()> {
        let target = self.target(team, spot, kind).with_context(|| self.unknown(team, spot))?;
        let below = self.below(target, team, spot, kind);
        let key = kind.key();
        let path = spot.path(team);
        let value = match value {
            Some(value) if below.as_ref().is_none_or(|(b, _)| !kind.same(b.as_ref(), Some(&value))) => value,
            _ => {
                if remove_entry(&mut self.list[target].doc, &path, key) {
                    prune(&mut self.list[target].doc, &path);
                }
                return Ok(());
            }
        };
        let table = self.table_mut(target, &path).expect("the target defines the member or the team");
        match table.get_mut(key) {
            Some(item) if Plain::read(item).as_ref() == Some(&value) => {}
            Some(Item::Value(old)) => {
                // The comment at the end of its line stays.
                let decor = old.decor().clone();
                *old = value.value();
                *old.decor_mut() = decor;
            }
            Some(item) => *item = Item::Value(value.value()),
            None => {
                table.insert(key, Item::Value(value.value()));
            }
        }
        Ok(())
    }

    /// Writes the files that changed, once the configuration they give reads back: all of them or none (`commit`).
    /// `rename` puts a temporary file over its file.
    fn save(&self, team: &str, rename: impl Fn(&Path, &Path) -> std::io::Result<()>) -> Result<()> {
        if self.list.iter().all(|f| f.before == f.doc.to_string()) {
            return Ok(());
        }
        let (_, texts) = self.check(team)?;
        let changes: Vec<(&Path, &str, &str)> = self
            .list
            .iter()
            .zip(&texts)
            .filter(|(file, text)| file.before != **text)
            .map(|(file, text)| (file.path.as_path(), text.as_str(), file.before.as_str()))
            .collect();
        commit(&changes, rename)
    }

    /// The configuration the files give as they are now, and their texts: an error when it would not read back.
    fn check(&self, team: &str) -> Result<(super::Settings, Vec<String>)> {
        let texts: Vec<String> = self.list.iter().map(|f| f.doc.to_string()).collect();
        let mut tables = Vec::new();
        for (file, text) in self.list.iter().zip(&texts) {
            let table = text.parse::<toml::Table>().with_context(|| {
                t!("{} : le TOML modifié est invalide", "{}: the changed TOML is invalid", file.path.display())
            })?;
            tables.push((table, file.path.clone()));
        }
        let settings = match &self.origin {
            Origin::Local { root } => merge_local(&root.join(LOCAL_DIR), tables)?,
            Origin::Global { file } => {
                let (table, _) = tables.pop().expect("a global team's file");
                parse_settings(table, file)?
            }
        };
        if settings.teams.get(team).is_none_or(|t| t.members.is_empty()) {
            bail!(t!(
                "l'équipe « {} » n'aurait plus de membre ({})",
                "team \"{}\" would have no members left ({})",
                team,
                tilde(&self.origin.place())
            ));
        }
        Ok((settings, texts))
    }
}

/// Writes files together, as (file, new text, text before): each new text in a temporary file beside its file first,
/// then each temporary file renamed over its file. When a rename fails, the files already changed get their text
/// back: a member renamed in one file only would be a member without a role in the other.
fn commit(changes: &[(&Path, &str, &str)], rename: impl Fn(&Path, &Path) -> std::io::Result<()>) -> Result<()> {
    let mut temporaries: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (file, text, _) in changes {
        match write_temporary(file, text) {
            Ok(temporary) => temporaries.push(temporary),
            Err(error) => {
                for (temporary, _) in &temporaries {
                    let _ = fs::remove_file(temporary);
                }
                return Err(error);
            }
        }
    }
    for (i, ((file, _, _), (temporary, target))) in changes.iter().zip(&temporaries).enumerate() {
        if let Err(error) = rename(temporary, target) {
            for (temporary, _) in &temporaries[i..] {
                let _ = fs::remove_file(temporary);
            }
            let mut error = anyhow::Error::new(error).context(t!("écriture de {}", "writing {}", file.display()));
            for (done, _, before) in &changes[..i] {
                if let Err(undo) = write_atomic(done, before) {
                    error = error.context(t!(
                        "{} est resté modifié : {:#}",
                        "{} was left changed: {:#}",
                        done.display(),
                        undo
                    ));
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

fn table_at<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut dyn TableLike> {
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for key in path {
        table = table.get_mut(key)?.as_table_like_mut()?;
    }
    Some(table)
}

fn item_at<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Item> {
    let (last, parents) = path.split_last()?;
    table_at(doc, parents)?.get_mut(last)
}

/// Takes away the table at `path` if it is empty, its comments with it, then each parent left empty: an empty
/// member's table would make a member without a role once the other file no longer has it.
fn prune(doc: &mut DocumentMut, path: &[&str]) {
    for depth in (1..=path.len()).rev() {
        let (last, parents) = path[..depth].split_last().expect("depth is at least 1");
        let Some(parent) = table_at(doc, parents) else { return };
        if !parent.get(last).and_then(Item::as_table_like).is_some_and(|t| t.is_empty()) {
            return;
        }
        remove_entry(doc, parents, last);
    }
}

/// Takes an entry out of the table at `path`. A table under its own header goes with the comments right above it;
/// those before a blank line belong to what comes before, the file's own heading for instance, and stay: they move
/// to the next table in the file, or to its end.
fn remove_entry(doc: &mut DocumentMut, path: &[&str], key: &str) -> bool {
    let Some(removed) = table_at(doc, path).and_then(|t| t.remove(key)) else { return false };
    let Item::Table(table) = removed else { return true };
    let prefix = table.decor().prefix().and_then(|p| p.as_str()).unwrap_or_default().to_string();
    let Some(blank) = prefix.rfind("\n\n") else { return true };
    let kept = &prefix[..blank + 2];
    if kept.trim().is_empty() {
        return true;
    }
    match next_table(doc, table.position()) {
        Some(path) => {
            let mut next = doc.as_table_mut();
            for key in &path {
                next = next.get_mut(key).and_then(Item::as_table_mut).expect("a table just found");
            }
            let own = next.decor().prefix().and_then(|p| p.as_str()).unwrap_or_default().to_string();
            next.decor_mut().set_prefix(format!("{kept}{}", own.trim_start_matches('\n')));
        }
        None => {
            let trailing = doc.trailing().as_str().unwrap_or_default().to_string();
            doc.set_trailing(format!("{trailing}{}\n", kept.trim_end()));
        }
    }
    true
}

/// The path of the table written right after position `after`, under its own header.
fn next_table(doc: &DocumentMut, after: Option<isize>) -> Option<Vec<String>> {
    fn visit(table: &Table, path: &mut Vec<String>, after: isize, best: &mut Option<(isize, Vec<String>)>) {
        for (key, item) in table.iter() {
            let Item::Table(table) = item else { continue };
            path.push(key.to_string());
            if let Some(position) =
                table.position().filter(|&p| p > after && !table.is_implicit() && !table.is_dotted())
                && best.as_ref().is_none_or(|(b, _)| position < *b)
            {
                *best = Some((position, path.clone()));
            }
            visit(table, path, after, best);
            path.pop();
        }
    }
    let mut best = None;
    visit(doc.as_table(), &mut Vec::new(), after?, &mut best);
    best.map(|(_, path)| path)
}

/// Renames a key in place: the entries after it are taken out and put back in their order. A table under a header
/// keeps its place in the file anyway, its comments with it.
fn rename_key(item: &mut Item, from: &str, to: &str) {
    let renamed =
        |key: Key| Key::new(to).with_leaf_decor(key.leaf_decor().clone()).with_dotted_decor(key.dotted_decor().clone());
    match item {
        Item::Table(table) => {
            let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
            let Some(at) = keys.iter().position(|k| k == from) else { return };
            let tail: Vec<(Key, Item)> = keys[at..].iter().filter_map(|k| table.remove_entry(k)).collect();
            for (key, item) in tail {
                let key = if key.get() == from { renamed(key) } else { key };
                table.insert_formatted(&key, item);
            }
        }
        Item::Value(Value::InlineTable(table)) => {
            let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
            let Some(at) = keys.iter().position(|k| k == from) else { return };
            let tail: Vec<(Key, Value)> = keys[at..].iter().filter_map(|k| table.remove_entry(k)).collect();
            for (key, value) in tail {
                let key = if key.get() == from { renamed(key) } else { key };
                table.insert_formatted(&key, value);
            }
        }
        _ => {}
    }
}

/// A member's keys and values, in the order recruit writes them.
fn member_values(member: &Member) -> Vec<(&'static str, Value)> {
    let mut values = vec![("role", Value::from(member.role.trim()))];
    if member.contact {
        values.push(("contact", Value::from(true)));
    }
    let texts = [
        ("instructions", &member.instructions),
        ("tab", &member.tab),
        ("permission_mode", &member.permission_mode),
        ("model", &member.model),
        ("effort", &member.effort),
    ];
    for (key, value) in texts {
        if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            values.push((key, Value::from(value)));
        }
    }
    if !member.args.is_empty() {
        values.push(("args", Value::Array(member.args.iter().map(String::as_str).collect())));
    }
    values
}

/// Adds a member to a team's table, written as the others are: under its own header, in dotted keys, or inline.
fn add_member(team: &mut dyn TableLike, name: &str, member: &Member) {
    let dotted = team.is_dotted();
    let members = team.entry("members").or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(true);
        table.set_dotted(dotted);
        Item::Table(table)
    });
    match members {
        Item::Value(Value::InlineTable(members)) => {
            let mut table = InlineTable::new();
            for (key, value) in member_values(member) {
                table.insert(key, value);
            }
            members.insert(name, Value::InlineTable(table));
        }
        Item::Table(members) => {
            let mut table = Table::new();
            table.set_dotted(members.is_dotted());
            for (key, value) in member_values(member) {
                table.insert(key, Item::Value(value));
            }
            members.insert(name, Item::Table(table));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project with its shared file and, if given, its personal one.
    fn project(shared: &str, personal: Option<&str>) -> (tempfile::TempDir, Origin) {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(LOCAL_DIR);
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(SETTINGS), shared).unwrap();
        if let Some(personal) = personal {
            fs::write(local.join(SETTINGS_LOCAL), personal).unwrap();
        }
        let origin = Origin::Local { root: dir.path().to_path_buf() };
        (dir, origin)
    }

    fn shared(dir: &Path) -> String {
        fs::read_to_string(dir.join(LOCAL_DIR).join(SETTINGS)).unwrap()
    }

    fn personal(dir: &Path) -> String {
        fs::read_to_string(dir.join(LOCAL_DIR).join(SETTINGS_LOCAL)).unwrap()
    }

    fn set(member: &str, field: Field) -> Edit {
        Edit::Set { member: member.into(), field }
    }

    const TEAM: &str = r#"# Équipe du projet
[teams.web]
model = "opus" # pour tous

# Le coordinateur
[teams.web.members.lead]
role = "Coordonne"
contact = true

# Le développeur, sur deux lignes
[teams.web.members.dev]
role = """
Code
et teste"""
effort = 'high' # assez

[teams.web.members.qa]
role = "Relit"

# Une autre équipe
[teams.autre.members.solo]
role = "Seul"
"#;

    #[test]
    fn set_keeps_comments_and_writes_only_changes() {
        let (dir, origin) = project(TEAM, None);
        let found = edit(&origin, "web", &[set("dev", Field::Effort(Some("max".into())))]).unwrap();
        assert_eq!(found.team.members["dev"].effort.as_deref(), Some("max"));
        let text = shared(dir.path());
        assert_eq!(text, TEAM.replace("effort = 'high' # assez", "effort = \"max\" # assez"));

        // The same value again: the file is not written (its literal string stays).
        let (dir, origin) = project(TEAM, None);
        let modified = |dir: &Path| fs::metadata(dir.join(LOCAL_DIR).join(SETTINGS)).unwrap().modified().unwrap();
        let before = modified(dir.path());
        std::thread::sleep(std::time::Duration::from_millis(20));
        edit(&origin, "web", &[set("dev", Field::Effort(Some("high".into())))]).unwrap();
        assert_eq!(modified(dir.path()), before);
        assert_eq!(shared(dir.path()), TEAM);
    }

    #[test]
    fn new_keys_go_into_the_members_table() {
        let (dir, origin) = project(TEAM, None);
        edit(
            &origin,
            "web",
            &[set("qa", Field::Tab(Some("Revue".into()))), set("qa", Field::Model(Some("sonnet".into())))],
        )
        .unwrap();
        let text = shared(dir.path());
        assert!(
            text.contains("[teams.web.members.qa]\nrole = \"Relit\"\ntab = \"Revue\"\nmodel = \"sonnet\"\n"),
            "{text}"
        );
        assert!(text.ends_with("# Une autre équipe\n[teams.autre.members.solo]\nrole = \"Seul\"\n"), "{text}");
    }

    #[test]
    fn defaults_take_the_key_away() {
        // The team's model under the member's: the member's goes, the team's stays.
        let (dir, origin) = project(&TEAM.replace("role = \"Relit\"", "role = \"Relit\"\nmodel = \"haiku\""), None);
        let found = edit(&origin, "web", &[set("qa", Field::Model(None))]).unwrap();
        assert_eq!(found.team.members["qa"].model, None);
        assert_eq!(shared(dir.path()), TEAM);
        // Asking for the team's value comes to the same.
        let (dir, origin) = project(&TEAM.replace("role = \"Relit\"", "role = \"Relit\"\nmodel = \"haiku\""), None);
        edit(&origin, "web", &[set("qa", Field::Model(Some("opus".into())))]).unwrap();
        assert_eq!(shared(dir.path()), TEAM);
        // Not a contact: the key goes, `contact = false` is never written.
        let (dir, origin) = project(TEAM, None);
        edit(&origin, "web", &[set("lead", Field::Contact(false))]).unwrap();
        assert_eq!(shared(dir.path()), TEAM.replace("contact = true\n", ""));
        // The dashboard back on.
        let (dir, origin) = project(&TEAM.replace("model = \"opus\" # pour tous\n", "dashboard = false\n"), None);
        assert!(!dashboard_layers(&origin, "web").unwrap().value);
        edit(&origin, "web", &[Edit::Dashboard(true)]).unwrap();
        assert!(!shared(dir.path()).contains("dashboard"));
        edit(&origin, "web", &[Edit::Dashboard(false)]).unwrap();
        assert!(shared(dir.path()).contains("[teams.web]\ndashboard = false\n"), "{}", shared(dir.path()));
    }

    /// The rule settled on 2026-10-07: the file that gives the value now, compared with the layers under it.
    #[test]
    fn personal_file_over_the_shared_one() {
        let personal_model = "[teams.web.members.dev]\nmodel = \"sonnet\"\n";
        // Both give a model: the default, or the shared value, takes the personal one away, with its table.
        for value in [None, Some("haiku")] {
            let shared_dev = TEAM.replace("effort = 'high' # assez", "model = \"haiku\"");
            let (dir, origin) = project(&shared_dev, Some(&format!("# perso\n\n# dev\n{personal_model}")));
            let layers = member_layers(&origin, "web", "dev").unwrap();
            assert_eq!((layers.model.value.as_deref(), layers.model.source), (Some("sonnet"), Source::Personal));
            assert_eq!(layers.model.fallback, Some((Some("haiku".into()), Source::Shared)));
            edit(&origin, "web", &[set("dev", Field::Model(value.map(String::from)))]).unwrap();
            assert_eq!(personal(dir.path()), "# perso\n", "{value:?}");
            assert_eq!(shared(dir.path()), shared_dev);
        }
        // Another value: written in the personal file, the shared one untouched.
        let (dir, origin) = project(TEAM, Some(personal_model));
        edit(&origin, "web", &[set("dev", Field::Model(Some("opus-5".into())))]).unwrap();
        assert_eq!(personal(dir.path()), "[teams.web.members.dev]\nmodel = \"opus-5\"\n");
        assert_eq!(shared(dir.path()), TEAM);
        // A key only the shared file has: changed there.
        edit(&origin, "web", &[set("dev", Field::Effort(None))]).unwrap();
        assert!(!shared(dir.path()).contains("effort"));
        assert_eq!(personal(dir.path()), "[teams.web.members.dev]\nmodel = \"opus-5\"\n");

        // Both mark the contact: the personal file says false.
        let (dir, origin) = project(TEAM, Some("[teams.web.members.lead]\ncontact = true\n"));
        edit(&origin, "web", &[set("lead", Field::Contact(false))]).unwrap();
        assert_eq!(personal(dir.path()), "[teams.web.members.lead]\ncontact = false\n");
        assert_eq!(shared(dir.path()), TEAM);
        let layers = member_layers(&origin, "web", "lead").unwrap();
        assert_eq!(
            layers.contact,
            Layered { value: false, source: Source::Personal, fallback: Some((true, Source::Shared)) }
        );

        // A team's key in the personal file only: the shared file's team is not touched.
        let (dir, origin) = project(TEAM, Some("[teams.web]\ndashboard = false\n"));
        edit(&origin, "web", &[Edit::Dashboard(true)]).unwrap();
        assert_eq!(personal(dir.path()), "");
        assert_eq!(shared(dir.path()), TEAM);
    }

    #[test]
    fn emptied_tables_go_with_their_comments_not_the_heading() {
        let personal_file = "# Réglages perso\n\n# dev en sonnet\n[teams.web.members.dev]\nmodel = \"sonnet\"\n\n\
                             [teams.web.members.qa]\neffort = \"low\"\n";
        let (dir, origin) = project(TEAM, Some(personal_file));
        edit(&origin, "web", &[set("dev", Field::Model(None))]).unwrap();
        assert_eq!(personal(dir.path()), "# Réglages perso\n\n[teams.web.members.qa]\neffort = \"low\"\n");
        edit(&origin, "web", &[set("qa", Field::Effort(None))]).unwrap();
        assert_eq!(personal(dir.path()), "# Réglages perso\n");
        // Its team's table, left empty, goes too: no member without a role once the shared file drops it.
        let (dir, origin) = project(TEAM, Some("[teams.web]\n[teams.web.members.qa]\nmodel = \"x\"\n"));
        edit(&origin, "web", &[set("qa", Field::Model(None))]).unwrap();
        assert_eq!(personal(dir.path()), "");
    }

    #[test]
    fn agents_tab_is_the_default() {
        let (dir, origin) = project(TEAM, None);
        edit(&origin, "web", &[set("qa", Field::Tab(Some("Agents".into())))]).unwrap();
        assert_eq!(shared(dir.path()), TEAM);
        edit(&origin, "web", &[set("qa", Field::Tab(Some("Code".into())))]).unwrap();
        assert!(shared(dir.path()).contains("tab = \"Code\""));
        edit(&origin, "web", &[set("qa", Field::Tab(Some(" Agents ".into())))]).unwrap();
        assert_eq!(shared(dir.path()), TEAM);

        // The shared file gives "Revue", the personal one "Code": "Agents" must give the default group, written.
        let revue = TEAM.replace("role = \"Relit\"", "role = \"Relit\"\ntab = \"Revue\"");
        let (dir, origin) = project(&revue, Some("[teams.web.members.qa]\ntab = \"Code\"\n"));
        let found = edit(&origin, "web", &[set("qa", Field::Tab(Some("Agents".into())))]).unwrap();
        assert_eq!(personal(dir.path()), "[teams.web.members.qa]\ntab = \"Agents\"\n");
        assert_eq!(shared(dir.path()), revue);
        let tabs = crate::layout::tabs(&found.team);
        assert!(tabs.iter().any(|t| t.title == "Agents" && t.members.contains(&"qa".to_string())), "{tabs:?}");
        // The default, then "Revue" again: the personal key goes.
        edit(&origin, "web", &[set("qa", Field::Tab(Some("Revue".into())))]).unwrap();
        assert_eq!(personal(dir.path()), "");
        // Under it, "Agents" already: the key goes too.
        let (dir, origin) =
            project(&revue.replace("\"Revue\"", "\"Agents\""), Some("[teams.web.members.qa]\ntab = \"Code\"\n"));
        edit(&origin, "web", &[set("qa", Field::Tab(Some("Agents".into())))]).unwrap();
        assert_eq!(personal(dir.path()), "");
    }

    #[test]
    fn layers_of_a_member() {
        let (_dir, origin) = project(TEAM, None);
        let layers = member_layers(&origin, "web", "dev").unwrap();
        assert_eq!(layers.role.value, "Code\net teste");
        assert_eq!(layers.role.fallback, None);
        assert_eq!(
            layers.model,
            Layered {
                value: Some("opus".into()),
                source: Source::Team,
                fallback: Some((Some("opus".into()), Source::Team))
            }
        );
        assert_eq!(layers.effort.source, Source::Shared);
        assert_eq!(layers.effort.fallback, Some((None, Source::Default)));
        assert_eq!(
            layers.contact,
            Layered { value: false, source: Source::Default, fallback: Some((false, Source::Default)) }
        );
        assert!(member_layers(&origin, "web", "inconnu").is_err());
        assert!(dashboard_layers(&origin, "web").unwrap().value);
    }

    #[test]
    fn rename_keeps_the_place_and_the_comments() {
        let (dir, origin) = project(TEAM, Some("[teams.web.members.dev]\nmodel = \"sonnet\" # perso\n"));
        let found = edit(&origin, "web", &[Edit::Rename { from: "dev".into(), to: "développeur".into() }]).unwrap();
        let names: Vec<&str> = found.team.members.keys().map(String::as_str).collect();
        assert_eq!(names, ["lead", "développeur", "qa"]);
        assert_eq!(found.team.members["développeur"].model.as_deref(), Some("sonnet"));
        let text = shared(dir.path());
        assert!(
            text.contains(
                "# Le développeur, sur deux lignes\n[teams.web.members.\"développeur\"]\nrole = \"\"\"\nCode"
            ),
            "{text}"
        );
        assert!(personal(dir.path()).contains("model = \"sonnet\" # perso"));

        // Inline tables and dotted keys keep their order too.
        for team in [
            "[teams.web]\nmembers = { lead = { role = 'L' }, dev = { role = 'D' }, qa = { role = 'Q' } }\n",
            "[teams.web]\nmembers.lead.role = 'L'\nmembers.dev.role = 'D'\nmembers.dev.model = 'm'\nmembers.qa.role = 'Q'\n",
        ] {
            let (_dir, origin) = project(team, None);
            let found = edit(&origin, "web", &[Edit::Rename { from: "dev".into(), to: "back".into() }]).unwrap();
            let names: Vec<&str> = found.team.members.keys().map(String::as_str).collect();
            assert_eq!(names, ["lead", "back", "qa"], "{team}");
        }
    }

    #[test]
    fn names_already_taken() {
        let (_dir, origin) = project(TEAM, None);
        let rename = |to: &str| edit(&origin, "web", &[Edit::Rename { from: "dev".into(), to: to.into() }]);
        assert!(rename("qa").is_err());
        assert!(rename("QA").is_err());
        assert!(rename("dev back").is_err());
        // Its own name in another case.
        assert!(rename("Dev").is_ok());
        let team = origin.load("web").unwrap().team;
        assert!(check_new_name(&team, "lead").is_err());
        // The case message, not "already in the team", in whatever language the tests run.
        assert!(check_new_name(&team, "Lead").unwrap_err().contains(&t!("que par la casse", "by case only")));
        assert!(check_new_name(&team, "ops").is_ok());
    }

    #[test]
    fn added_where_the_team_is_and_before_the_next_one() {
        let (dir, origin) = project(TEAM, Some("[teams.web]\nmodel = \"sonnet\"\n"));
        let member = Member { role: "Publie\nles versions".into(), model: Some("haiku".into()), ..Default::default() };
        let found = edit(&origin, "web", &[Edit::Add { name: "ops".into(), member }]).unwrap();
        let names: Vec<&str> = found.team.members.keys().map(String::as_str).collect();
        assert_eq!(names, ["lead", "dev", "qa", "ops"]);
        let text = shared(dir.path());
        let ops = text.find("[teams.web.members.ops]").unwrap();
        assert!(
            text.find("[teams.web.members.qa]").unwrap() < ops && ops < text.find("# Une autre équipe").unwrap(),
            "{text}"
        );
        assert_eq!(personal(dir.path()), "[teams.web]\nmodel = \"sonnet\"\n");
        assert!(
            edit(
                &origin,
                "web",
                &[Edit::Add { name: "Ops".into(), member: Member { role: "x".into(), ..Default::default() } }]
            )
            .is_err()
        );
        assert!(edit(&origin, "web", &[Edit::Add { name: "x".into(), member: Member::default() }]).is_err());
    }

    #[test]
    fn removed_from_every_file_never_the_last() {
        let (dir, origin) = project(TEAM, Some("# perso\n\n[teams.web.members.dev]\nmodel = \"sonnet\"\n"));
        let found = edit(&origin, "web", &[Edit::Remove("dev".into())]).unwrap();
        assert!(!found.team.members.contains_key("dev"));
        assert_eq!(personal(dir.path()), "# perso\n");
        assert!(!shared(dir.path()).contains("Le développeur"));
        edit(&origin, "web", &[Edit::Remove("qa".into())]).unwrap();
        let error = format!("{:#}", edit(&origin, "web", &[Edit::Remove("lead".into())]).unwrap_err());
        assert!(error.contains("dernier") || error.contains("last"), "{error}");
        assert!(shared(dir.path()).contains("[teams.web.members.lead]"));
    }

    #[test]
    fn invalid_results_write_nothing() {
        let three = TEAM.replace("role = \"Relit\"", "role = \"Relit\"\ncontact = true");
        let (dir, origin) = project(&three, None);
        let error = format!("{:#}", edit(&origin, "web", &[set("dev", Field::Contact(true))]).unwrap_err());
        assert!(error.contains(&dir.path().join(LOCAL_DIR).join(SETTINGS).display().to_string()), "{error}");
        assert_eq!(shared(dir.path()), three);
        // Together, the contact handed over: fine.
        let found =
            edit(&origin, "web", &[set("qa", Field::Contact(false)), set("dev", Field::Contact(true))]).unwrap();
        assert_eq!(found.team.contacts(), ["lead", "dev"]);
        // An edit that fails halfway leaves everything as it was.
        let before = shared(dir.path());
        assert!(
            edit(&origin, "web", &[set("qa", Field::Model(Some("x".into()))), set("nobody", Field::Tab(None))])
                .is_err()
        );
        assert_eq!(shared(dir.path()), before);
        assert!(edit(&origin, "web", &[set("qa", Field::Role("  ".into()))]).is_err());
    }

    /// The personal file's rename fails after the shared one's went through: the shared file gets its text back, no
    /// temporary file stays, and the team still reads.
    #[test]
    fn both_files_or_neither() {
        let personal_file = "[teams.web.members.dev]\nmodel = \"sonnet\"\n";
        let (dir, origin) = project(TEAM, Some(personal_file));
        let mut files = Files::read(&origin).unwrap();
        files.apply("web", &Edit::Rename { from: "dev".into(), to: "back".into() }).unwrap();
        let renames = std::cell::Cell::new(0);
        let error = files
            .save("web", |from, to| {
                renames.set(renames.get() + 1);
                if renames.get() == 2 { Err(std::io::Error::other("disque plein")) } else { fs::rename(from, to) }
            })
            .unwrap_err();
        assert_eq!(renames.get(), 2);
        let error = format!("{error:#}");
        assert!(error.contains(SETTINGS_LOCAL) && error.contains("disque plein"), "{error}");
        assert_eq!(shared(dir.path()), TEAM);
        assert_eq!(personal(dir.path()), personal_file);
        let left: Vec<String> = fs::read_dir(dir.path().join(LOCAL_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        assert!(origin.load("web").unwrap().team.members.contains_key("dev"));

        // The same rename, nothing failing: both files change.
        let found = edit(&origin, "web", &[Edit::Rename { from: "dev".into(), to: "back".into() }]).unwrap();
        assert_eq!(found.team.members["back"].model.as_deref(), Some("sonnet"));
        assert!(personal(dir.path()).contains("[teams.web.members.back]"));
    }

    #[test]
    fn previewed_not_written() {
        let (dir, origin) = project(TEAM, None);
        let found = preview(&origin, "web", &[set("dev", Field::Model(Some("haiku".into())))]).unwrap();
        assert_eq!(found.team.members["dev"].model.as_deref(), Some("haiku"));
        assert_eq!(found.origin(), origin);
        assert_eq!(shared(dir.path()), TEAM);
        assert!(preview(&origin, "web", &[set("dev", Field::Contact(true)), set("qa", Field::Contact(true))]).is_err());
    }

    /// Team files kept in a dotfiles repository and linked: the link stays, its target changes.
    #[test]
    fn links_stay_links() {
        let dotfiles = tempfile::tempdir().unwrap();
        let (dir, origin) = project(TEAM, None);
        let real = dotfiles.path().join("perso.toml");
        fs::write(&real, "# mes réglages\n[teams.web.members.dev]\nmodel = \"sonnet\"\n").unwrap();
        let link = dir.path().join(LOCAL_DIR).join(SETTINGS_LOCAL);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        edit(&origin, "web", &[set("dev", Field::Model(Some("haiku".into())))]).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "# mes réglages\n[teams.web.members.dev]\nmodel = \"haiku\"\n");
        assert_eq!(shared(dir.path()), TEAM);
        // Nothing left beside the link nor the file.
        for folder in [dir.path().join(LOCAL_DIR), dotfiles.path().to_path_buf()] {
            assert!(fs::read_dir(folder).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
        }

        // A global team's file, linked.
        let global = tempfile::tempdir().unwrap();
        let real = dotfiles.path().join("web.toml");
        fs::write(&real, TEAM).unwrap();
        let link = global.path().join("web.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        edit(&Origin::Global { file: link.clone() }, "web", &[set("qa", Field::Effort(Some("low".into())))]).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(fs::read_to_string(&real).unwrap().contains("role = \"Relit\"\neffort = \"low\""));
    }

    #[test]
    fn edits_in_json() {
        let edits: Vec<Edit> = serde_json::from_str(
            r#"[{"set": {"member": "dev", "field": {"model": null}}}, {"set": {"member": "qa", "field": {"contact": true}}},
                {"rename": {"from": "a", "to": "b"}}, {"add": {"name": "x", "member": {"role": "X", "model": "opus"}}},
                {"remove": "dev"}, {"dashboard": false}]"#,
        )
        .unwrap();
        assert_eq!(edits[0], set("dev", Field::Model(None)));
        assert_eq!(edits[1], set("qa", Field::Contact(true)));
        let Edit::Add { member, .. } = &edits[3] else { panic!() };
        assert_eq!(member.model.as_deref(), Some("opus"));
        assert_eq!(edits[5], Edit::Dashboard(false));
    }

    #[test]
    fn global_team() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("web.toml");
        fs::write(&file, TEAM).unwrap();
        let origin = Origin::Global { file: file.clone() };
        let found = edit(&origin, "web", &[set("qa", Field::Effort(Some("low".into())))]).unwrap();
        assert_eq!(found.origin(), origin);
        assert_eq!(found.team.members["qa"].effort.as_deref(), Some("low"));
        assert!(fs::read_to_string(&file).unwrap().contains("# Une autre équipe"));
        let layers = member_layers(&origin, "web", "qa").unwrap();
        assert_eq!(layers.effort.source, Source::Shared);
    }
}
