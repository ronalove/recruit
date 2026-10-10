// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A running team, changed from its menu: what `config::edit` writes is applied at once to the team's panes and to
//! its `team.json`. Panes move between tabs without their Claude stopping; a member starts again only when it must,
//! renamed or with another permission mode, or when asked to. Whatever the change, the running team then follows
//! its files: a member they add gets its pane, one they no longer have loses it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::backend::Pane;
use crate::backend::{self, Backend, MenuOpened};
use crate::config::{self, Edit, Field, Found, Origin, Team};
use crate::state::{self, Snapshot};
use crate::{board, bridge, claude, launch, layout, prompt, t};

/// A running team, from its folder under `~/.cache/recruit/teams/`.
pub struct Running {
    pub state: PathBuf,
    pub snapshot: Snapshot,
    backend: Box<dyn Backend>,
}

/// How a member starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// On its conversation (`recruit _member --resume`).
    Resume,
    /// On a new conversation, with its prompt as the team's files give it now.
    Fresh,
}

impl Running {
    pub fn open(state: &Path) -> Result<Running> {
        let snapshot = Snapshot::read(state)?;
        let backend = backend::of(&snapshot);
        Ok(Running { state: state.to_path_buf(), snapshot, backend })
    }

    /// A `--dry-run` trial starts no Claude: nothing to change in it.
    fn guard(&self) -> Result<()> {
        if self.snapshot.session.ends_with(launch::DRY_RUN) {
            bail!(t!(
                "un essai --dry-run ne lance pas Claude : lance l'équipe pour la modifier d'ici",
                "a --dry-run trial starts no Claude: launch the team to change it from here"
            ));
        }
        Ok(())
    }

    fn origin(&self) -> Result<Origin> {
        self.snapshot.origin.clone().with_context(|| {
            t!(
                "l'équipe « {} » a été lancée par une version plus ancienne de recruit : relance-la pour la modifier d'ici",
                "team \"{}\" was launched by an older recruit: launch it again to change it from here",
                self.snapshot.team
            )
        })
    }

    /// The team as its files say now.
    pub fn config(&self) -> Result<Found> {
        self.origin()?.load(&self.snapshot.team)
    }

    /// Writes `edits` into the team's files (`config::edit`), then applies them to the running team.
    pub fn apply(&mut self, edits: &[Edit]) -> Result<()> {
        self.guard()?;
        let _lock = state::lock(&self.state)?;
        self.snapshot = Snapshot::read(&self.state)?;
        let origin = self.origin()?;
        let before = origin.load(&self.snapshot.team)?;
        let named = Named::by(edits);
        let new_names: Vec<&str> =
            named.added.iter().chain(named.renames.iter().map(|(_, to)| to)).map(String::as_str).collect();
        self.check_open(&new_names)?;
        let found = config::edit(&origin, &self.snapshot.team, edits)?;
        let starts: Vec<(String, Start)> = resumed(&self.snapshot, &before.team, &found.team, &named.renames)
            .into_iter()
            .map(|name| (name, Start::Resume))
            .collect();
        self.sync(&found, &named, &starts)
    }

    /// The members `apply` starts again for these edits, by their names before them: a renamed one, on its
    /// conversation under its new name; one whose permission mode changes; the main contact and the one that takes
    /// its place, when the user's settings give a status line (only the main contact shows it); one whose model or
    /// effort changes to one its requests cannot carry (`bridge::applies_live`; any, when Claude Code runs without
    /// the mod). Nothing written. Edits that `apply` would refuse: every
    /// member they rename or give a permission mode, a model or an effort.
    pub fn restarts(&self, edits: &[Edit]) -> Vec<String> {
        let renames = Named::by(edits).renames;
        let after = self.origin().and_then(|origin| {
            let before = origin.load(&self.snapshot.team)?;
            let after = config::preview(&origin, &self.snapshot.team, edits)?;
            Ok(resumed(&self.snapshot, &before.team, &after.team, &renames))
        });
        match after {
            Ok(after) => before_names(after, &renames),
            Err(_) => touched(edits),
        }
    }

    /// Starts a member again: on its conversation, or on a new one (`fresh`), its command line built again from the
    /// team's files.
    pub fn restart(&mut self, member: &str, fresh: bool) -> Result<()> {
        self.guard()?;
        let _lock = state::lock(&self.state)?;
        self.snapshot = Snapshot::read(&self.state)?;
        let found = self.config()?;
        if !found.team.members.contains_key(member) {
            bail!(t!("l'équipe n'a pas de membre « {} »", "the team has no member \"{}\"", member));
        }
        let start = if fresh { Start::Fresh } else { Start::Resume };
        let named = Named { changed: vec![member.to_string()], ..Default::default() };
        self.sync(&found, &named, &[(member.to_string(), start)])
    }

    /// Starts every member again on a new conversation, the team as its files say now.
    pub fn restart_all(&mut self) -> Result<()> {
        self.guard()?;
        let _lock = state::lock(&self.state)?;
        self.snapshot = Snapshot::read(&self.state)?;
        let found = self.config()?;
        let starts: Vec<(String, Start)> = found.team.members.keys().map(|n| (n.clone(), Start::Fresh)).collect();
        let named = Named::all(&self.snapshot, &found.team);
        self.sync(&found, &named, &starts)
    }

    /// Closes the pane of a member the team's files no longer have, as for a member removed: its conversation stops,
    /// its session and its reports are forgotten, the panes left share the tabs again. Refused for a member the files
    /// still have: removing it goes through `apply`.
    pub fn dismiss(&mut self, member: &str) -> Result<()> {
        self.guard()?;
        let _lock = state::lock(&self.state)?;
        self.snapshot = Snapshot::read(&self.state)?;
        let found = self.config()?;
        if !gone(&self.snapshot, &found.team).iter().any(|m| m == member) {
            bail!(t!(
                "« {} » n'est pas un membre lancé que les fichiers de l'équipe n'ont plus",
                "\"{}\" is not a running member that the team's files no longer have",
                member
            ));
        }
        let named = Named { removed: vec![member.to_string()], ..Default::default() };
        self.sync(&found, &named, &[])
    }

    /// Detaches a client: the team keeps running.
    pub fn detach(&self, client: &str) -> Result<()> {
        self.backend.detach(client)
    }

    /// Stops the team: its Claude sessions are closed, the menu with them.
    pub fn stop(&self) -> Result<()> {
        self.backend.stop(&self.snapshot.session)
    }

    /// Refuses names that a Claude session open in the team's folder already has: messages by name would not know
    /// which one to reach (`launch::check_duplicates`).
    fn check_open(&self, names: &[&str]) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let running = claude::running(&self.snapshot.claude, self.snapshot.config_dir.as_deref())?;
        let open: Vec<&str> = running
            .iter()
            .filter(|r| r.cwd.as_deref().map(Path::new) == Some(self.snapshot.dir.as_path()))
            .filter_map(|r| r.name.as_deref().filter(|n| names.contains(n)))
            .collect();
        if !open.is_empty() {
            bail!(t!(
                "une session Claude porte déjà ce nom dans ce dossier : {}. Ferme-la d'abord, ou choisis un autre nom.",
                "a Claude session already has this name in this directory: {}. Close it first, or pick another name.",
                open.join(", ")
            ));
        }
        Ok(())
    }

    /// Makes the running team follow `found` for the members `named` names: renamed ones follow their new names,
    /// removed ones lose their panes, added ones get theirs, the dashboard opens or closes when the edits ask, panes
    /// move to their tabs, and the members of `starts` start again. The others keep running as they are (`view`).
    /// team.json is written before any of them starts: `recruit _member` reads it.
    fn sync(&mut self, found: &Found, named: &Named, starts: &[(String, Start)]) -> Result<()> {
        let state = self.state.clone();
        let session = self.snapshot.session.clone();
        let dir = self.snapshot.dir.clone();
        let exe = std::env::current_exe().context("recruit")?.to_string_lossy().into_owned();
        let panes = self.backend.panes(&session)?;
        let mut pane_of: HashMap<String, String> =
            panes.iter().filter(|p| p.role.is_empty()).map(|p| (p.member.clone(), p.id.clone())).collect();

        for (from, to) in &named.renames {
            let Some(info) = self.snapshot.members.iter_mut().find(|m| m.name == *from) else { continue };
            if !found.team.members.contains_key(to) {
                continue;
            }
            info.name = to.clone();
            let (old, new) = (session_file(&state, from), session_file(&state, to));
            if old.exists() {
                fs::rename(&old, &new).with_context(|| old.display().to_string())?;
            }
            bridge::rename(&state, from, to)?;
            if let Some(pane) = pane_of.remove(from) {
                self.backend.set_member(&pane, to)?;
                pane_of.insert(to.clone(), pane);
            }
        }

        let running: Vec<String> = self.snapshot.members.iter().map(|m| m.name.clone()).collect();
        let removed: Vec<String> = named.removed.iter().filter(|n| running.contains(n)).cloned().collect();
        let view = view(&self.snapshot, &found.team, named);
        let argvs: HashMap<String, Vec<String>> = self.snapshot.members.drain(..).map(|m| (m.name, m.argv)).collect();
        self.snapshot.members = state::members(&view, &argvs);

        // Who starts, and how: the new members on a new conversation.
        let mut starting: Vec<(String, Start)> = named
            .added
            .iter()
            .filter(|n| !running.contains(n) && view.members.contains_key(*n))
            .map(|n| (n.clone(), Start::Fresh))
            .collect();
        for (name, start) in starts.iter().filter(|(n, _)| view.members.contains_key(n)) {
            match starting.iter_mut().find(|(n, _)| n == name) {
                Some(entry) => entry.1 = (*start).max_fresh(entry.1),
                None => starting.push((name.clone(), *start)),
            }
        }
        for (name, start) in &starting {
            let argv = fresh_argv(&self.snapshot, found, name)?;
            if let Some(info) = self.snapshot.members.iter_mut().find(|m| m.name == *name) {
                info.argv = argv;
            }
            if *start == Start::Fresh {
                let _ = fs::remove_file(session_file(&state, name));
            }
        }

        let lang = self.snapshot.lang;
        let panels = found.team.dashboard != Some(false);
        if named.dashboard {
            self.snapshot.dashboard = if panels { launch::panel(&exe, lang, "dashboard", &state) } else { Vec::new() };
            self.snapshot.journal = launch::panel(&exe, lang, "journal", &state);
        }
        self.snapshot.write(&state)?;

        for name in &removed {
            if let Some(pane) = pane_of.remove(name) {
                // Its supervisor gets the hang-up, as when the team stops: nothing starts again.
                self.backend.kill_pane(&pane)?;
            }
            let _ = fs::remove_file(session_file(&state, name));
            bridge::forget(&state, name)?;
        }
        let member_pane = |name: &str, start: Start| Pane {
            member: name.to_string(),
            role: None,
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                launch::member_script(&exe, &state, name, start == Start::Resume),
            ],
            env: launch::member_env(&self.snapshot.team, name, &exe, &state, lang, self.snapshot.config_dir.as_deref()),
        };
        let mut started = Vec::new();
        for (name, start) in &starting {
            if !pane_of.contains_key(name) {
                let pane = self.backend.open_window(&session, &dir, &member_pane(name, *start))?;
                pane_of.insert(name.clone(), pane);
                started.push(name.clone());
            }
        }

        // A member whose pane was closed by hand stays without one: `recruit` offers to build the team again.
        let mut tabs = layout::tabs(&view);
        for tab in &mut tabs {
            tab.members.retain(|m| pane_of.contains_key(m));
        }
        tabs.retain(|t| !t.members.is_empty());
        if named.dashboard {
            let has_panels = self.backend.panes(&session)?.iter().any(|p| p.role == backend::DASHBOARD);
            if !panels {
                self.backend.close_panels(&session)?;
            } else if !has_panels && let Some(first) = tabs.first().and_then(|t| pane_of.get(&t.members[0])) {
                let panel = |kind: &str, role, title: &str| Pane {
                    member: title.to_string(),
                    role: Some(role),
                    argv: launch::panel(&exe, lang, kind, &state),
                    env: Vec::new(),
                };
                self.backend.close_panels(&session)?;
                self.backend.open_panels(
                    &dir,
                    first,
                    &panel("dashboard", backend::DASHBOARD, board::dashboard_title(lang)),
                    &panel("journal", backend::JOURNAL, board::journal_title(lang)),
                )?;
            }
        }
        self.backend.arrange(&session, &tabs, layout::columns(&view))?;

        for (name, start) in &starting {
            if started.contains(name) {
                continue;
            }
            if let Some(pane) = pane_of.get(name) {
                self.backend.respawn(pane, &dir, &member_pane(name, *start))?;
            }
        }
        Ok(())
    }
}

/// The members a change names, which the running team changes for. The others keep running as they are, whatever
/// their files say now (changed by hand, or by a `git checkout`): one gone from the files keeps its pane and its
/// conversation, one new in them waits for the next launch.
#[derive(Debug, Default, PartialEq)]
struct Named {
    /// From the name a member runs under to its new one.
    renames: Vec<(String, String)>,
    /// Taken out of the team, by the names they run under.
    removed: Vec<String>,
    /// New in the team, by their names in the files.
    added: Vec<String>,
    /// Every member named, by its name in the files: those to take as the files say.
    changed: Vec<String>,
    /// The dashboard follows the files.
    dashboard: bool,
}

impl Named {
    /// The members `edits` name, one edit after the other.
    fn by(edits: &[Edit]) -> Named {
        // Each name the edits touch, as it is now, and the name it runs under (None: added by the edits).
        let mut now: Vec<(String, Option<String>)> = Vec::new();
        let mut named = Named::default();
        for edit in edits {
            let at = |now: &[(String, Option<String>)], name: &str| now.iter().position(|(n, _)| n == name);
            match edit {
                Edit::Add { name, .. } => now.push((name.clone(), None)),
                Edit::Rename { from, to } => match at(&now, from) {
                    Some(i) => now[i].0 = to.clone(),
                    None => now.push((to.clone(), Some(from.clone()))),
                },
                Edit::Remove(name) => match at(&now, name) {
                    Some(i) => named.removed.extend(now.remove(i).1),
                    None => named.removed.push(name.clone()),
                },
                Edit::Set { member, .. } => {
                    if at(&now, member).is_none() {
                        now.push((member.clone(), Some(member.clone())));
                    }
                }
                Edit::Dashboard(_) => named.dashboard = true,
            }
        }
        for (name, running) in now {
            match running {
                Some(running) if running != name => named.renames.push((running, name.clone())),
                Some(_) => {}
                None => named.added.push(name.clone()),
            }
            named.changed.push(name);
        }
        named
    }

    /// Every member, the team following its files whole: `restart_all`.
    fn all(snapshot: &Snapshot, team: &Team) -> Named {
        let running: Vec<&str> = snapshot.members.iter().map(|m| m.name.as_str()).collect();
        Named {
            renames: Vec::new(),
            removed: running.iter().filter(|n| !team.members.contains_key(**n)).map(|n| n.to_string()).collect(),
            added: team.members.keys().filter(|n| !running.contains(&n.as_str())).cloned().collect(),
            changed: team.members.keys().cloned().collect(),
            dashboard: true,
        }
    }
}

/// The team as it runs once a change is made: the running members, renamed, the removed ones out, the added ones
/// in, in the files' order (one the files no longer have after the member it ran after). A member the change names
/// as its files say; another as it runs, its contact mark aside, read in the files while they have it (who is a
/// contact depends on the whole team). The tabs' shape, as the files say.
fn view(snapshot: &Snapshot, team: &Team, named: &Named) -> Team {
    let in_files = |name: &str| team.members.get_index_of(name);
    let renamed = |name: &str| -> String {
        let to = named.renames.iter().find(|(from, to)| from == name && team.members.contains_key(to));
        to.map_or(name, |(_, to)| to).to_string()
    };
    let mut members: Vec<((usize, usize), String, config::Member)> = Vec::new();
    let mut after = 0;
    for (i, info) in snapshot.members.iter().enumerate() {
        if named.removed.contains(&info.name) {
            continue;
        }
        let name = renamed(&info.name);
        let key = match in_files(&name) {
            Some(index) => {
                after = index + 1;
                (index + 1, 0)
            }
            None => (after, i + 1),
        };
        let member = match team.members.get(&name) {
            Some(member) if named.changed.contains(&name) => as_set(team, member),
            files => config::Member {
                role: info.role.clone(),
                contact: files.map_or(info.contact, |m| m.contact),
                tab: info.tab.clone(),
                model: info.model.clone(),
                effort: info.effort.clone(),
                ..Default::default()
            },
        };
        members.push((key, name, member));
    }
    for name in &named.added {
        if let Some(index) = in_files(name).filter(|_| !members.iter().any(|(_, n, _)| n == name)) {
            members.push(((index + 1, 0), name.clone(), as_set(team, &team.members[name])));
        }
    }
    members.sort_by_key(|(key, _, _)| *key);
    Team {
        lang: team.lang,
        layout: team.layout,
        columns: team.columns,
        rows: team.rows,
        members: members.into_iter().map(|(_, name, member)| (name, member)).collect(),
        ..Default::default()
    }
}

/// The running members `team` does not have (removed by hand, or by a `git checkout`): `apply` leaves them be,
/// `Running::dismiss` closes them.
fn gone(snapshot: &Snapshot, team: &Team) -> Vec<String> {
    snapshot.members.iter().filter(|m| !team.members.contains_key(&m.name)).map(|m| m.name.clone()).collect()
}

/// A member as its files say, its model and effort the team's when it has none of its own.
fn as_set(team: &Team, member: &config::Member) -> config::Member {
    config::Member {
        model: member.model.clone().or_else(|| team.model.clone()),
        effort: member.effort.clone().or_else(|| team.effort.clone()),
        ..member.clone()
    }
}

impl Start {
    /// A new conversation wins over resuming one.
    fn max_fresh(self, other: Start) -> Start {
        if self == Start::Fresh || other == Start::Fresh { Start::Fresh } else { Start::Resume }
    }
}

/// The members of `after`, by their names in it, that must start again for the running team to follow it from
/// `before`: see `Running::restarts`. `snapshot` gives the command line each one runs.
fn resumed(snapshot: &Snapshot, before: &Team, after: &Team, renames: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> =
        renames.iter().filter(|(_, to)| after.members.contains_key(to)).map(|(_, to)| to.clone()).collect();
    let model = |team: &Team, m: &config::Member| m.model.clone().or_else(|| team.model.clone());
    let effort = |team: &Team, m: &config::Member| m.effort.clone().or_else(|| team.effort.clone());
    for (name, member) in &after.members {
        let old = renames.iter().find(|(_, to)| to == name).map_or(name.as_str(), |(from, _)| from.as_str());
        let Some(was) = before.members.get(old) else { continue };
        let argv = snapshot.member(old).map(|m| m.argv.as_slice()).unwrap_or_default();
        // A permission mode is given at start only.
        let permission = permission_mode(before, was) != permission_mode(after, member);
        // So is the status line: the main contact's goes to another (`claude::quiet_settings`).
        let quiet = before.quiet(old) != after.quiet(name)
            && claude::quieted(argv) != claude::quiet_settings(snapshot.status_line, after, name, member);
        // A model or an effort the mod cannot put on each request; with no mod, any but those it was started with.
        let (new_model, new_effort) = (model(after, member), effort(after, member));
        let tuned = new_model != model(before, was) || new_effort != effort(before, was);
        let carried = match snapshot.plugin_dir {
            Some(_) => bridge::applies_live(new_model.as_deref(), new_effort.as_deref(), argv),
            None => {
                new_model.as_deref() == claude::flag_value(argv, "--model")
                    && new_effort.as_deref() == claude::flag_value(argv, "--effort")
            }
        };
        if (permission || quiet || (tuned && !carried)) && !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

/// Names after `renames`, given back as they were before.
fn before_names(names: Vec<String>, renames: &[(String, String)]) -> Vec<String> {
    names
        .into_iter()
        .map(|name| renames.iter().find(|(_, to)| *to == name).map_or(name, |(from, _)| from.clone()))
        .collect()
}

/// The members edits rename or give a permission mode, a model or an effort: those `apply` may start again.
fn touched(edits: &[Edit]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for edit in edits {
        let name = match edit {
            Edit::Rename { from, to } if from != to => from,
            Edit::Set { member, field: Field::PermissionMode(_) | Field::Model(_) | Field::Effort(_) } => member,
            _ => continue,
        };
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

fn permission_mode<'a>(team: &'a Team, member: &'a config::Member) -> Option<&'a str> {
    member.permission_mode.as_deref().or(team.permission_mode.as_deref())
}

/// Where `recruit _member` notes the session a member runs.
fn session_file(state: &Path, member: &str) -> PathBuf {
    state.join("sessions").join(member)
}

/// A member's command line for a new conversation, from the team as its files say now; its prompt written, if new,
/// in a file of its own (`prompt::write`).
pub fn fresh_argv(snapshot: &Snapshot, found: &Found, member: &str) -> Result<Vec<String>> {
    let info = found
        .team
        .members
        .get(member)
        .with_context(|| t!("l'équipe n'a pas de membre « {} »", "the team has no member \"{}\"", member))?;
    let text = prompt::build(&found.name, &found.team, member);
    let file = prompt::write(&snapshot.session, member, &text)?;
    let launch = claude::Launch {
        claude: &snapshot.claude,
        team: &found.team,
        plugin_dir: snapshot.plugin_dir.as_deref(),
        status_line: snapshot.status_line,
    };
    Ok(launch.argv(member, info, &file, None))
}

/// A member's prompt as a new conversation would get it now, the team's files read again: the text `launch` and
/// `fresh_argv` write.
pub fn prompt(state: &Path, member: &str) -> Result<String> {
    let snapshot = Snapshot::read(state)?;
    let origin = snapshot
        .origin
        .as_ref()
        .context(t!("équipe lancée par une version plus ancienne de recruit", "team launched by an older recruit"))?;
    let found = origin.load(&snapshot.team)?;
    if !found.team.members.contains_key(member) {
        bail!(t!("l'équipe n'a pas de membre « {} »", "the team has no member \"{}\"", member));
    }
    let mut text = prompt::build(&found.name, &found.team, member);
    // The exact addresses of the teammates whose name another session carries too: they change with each restart, and
    // the note goes again when they do.
    let refs: Vec<(String, String)> = addresses(state)
        .into_iter()
        .filter(|(name, _)| name != member && found.team.members.contains_key(name))
        .collect();
    if let Some(addresses) = prompt::addresses(&found.team, &refs) {
        text.push('\n');
        text.push_str(&addresses);
        text.push('\n');
    }
    Ok(text)
}

/// Older than this, in seconds, what the dashboard wrote is no longer what runs: the refs change rarely, at a
/// member's restart.
const STALE: i64 = 10;

/// The exact addresses of the members whose name another session carries too, as the dashboard saw them a moment ago;
/// none when it did not lately. Never `claude agents` here: this runs at each message a member is sent, where nothing
/// may be slow. So a team without a dashboard (`dashboard = false`) gets no exact addresses: its members ask (the
/// prompt's rule).
fn addresses(state: &Path) -> std::collections::BTreeMap<String, String> {
    board::read_states(state).filter(|s| (board::now() - s.at).abs() <= STALE).map(|s| s.refs).unwrap_or_default()
}

/// Opens the team's menu over a member's pane, on the client that shows it, without waiting for it to close: for
/// `/recruit` typed in its session.
pub fn open_menu(state: &Path, member: &str) -> Result<MenuOpened> {
    let snapshot = Snapshot::read(state)?;
    backend::of(&snapshot).open_menu(state, &snapshot.session, Some(member), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename(from: &str, to: &str) -> Edit {
        Edit::Rename { from: from.into(), to: to.into() }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn set(member: &str, field: Field) -> Edit {
        Edit::Set { member: member.into(), field }
    }

    fn add(name: &str) -> Edit {
        Edit::Add { name: name.into(), member: config::Member { role: "r".into(), ..Default::default() } }
    }

    #[test]
    fn members_named_one_edit_after_the_other() {
        let pairs =
            |list: &[(&str, &str)]| list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        assert_eq!(Named::by(&[rename("a", "b"), rename("b", "c")]).renames, pairs(&[("a", "c")]));
        assert_eq!(Named::by(&[rename("a", "b"), rename("b", "a")]).renames, pairs(&[]));
        assert_eq!(Named::by(&[rename("a", "a"), rename("x", "y")]).renames, pairs(&[("x", "y")]));
        let named = Named::by(&[
            add("new"),
            rename("new", "ops"),
            rename("dev", "back"),
            Edit::Remove("back".into()),
            add("tmp"),
            Edit::Remove("tmp".into()),
            set("qa", Field::Tab(None)),
            Edit::Remove("lead".into()),
        ]);
        assert_eq!(
            named,
            Named {
                renames: Vec::new(),
                removed: strings(&["dev", "lead"]),
                added: strings(&["ops"]),
                changed: strings(&["ops", "qa"]),
                dashboard: false,
            }
        );
        assert!(Named::by(&[Edit::Dashboard(false)]).dashboard);
    }

    /// Running: lead, dev (tab "Code"), qa, old (gone from the files by hand); the files: lead, dev (tab moved by
    /// hand), qa, extra (added by hand).
    fn running_and_files() -> (Snapshot, Team) {
        let info = |name: &str, contact: bool, tab: Option<&str>| state::MemberInfo {
            name: name.into(),
            role: format!("rôle de {name}"),
            contact,
            tab: tab.map(String::from),
            model: Some("opus".into()),
            argv: vec!["claude".into()],
            ..Default::default()
        };
        let snapshot = Snapshot {
            members: vec![
                info("lead", true, None),
                info("dev", false, Some("Code")),
                info("old", false, None),
                info("qa", false, None),
            ],
            ..Default::default()
        };
        let mut team = Team { model: Some("sonnet".into()), ..Default::default() };
        for (name, tab) in [("lead", None), ("dev", Some("Ailleurs")), ("qa", None), ("extra", None)] {
            let member = config::Member {
                role: format!("{name} dans le fichier"),
                tab: tab.map(String::from),
                ..Default::default()
            };
            team.members.insert(name.into(), member);
        }
        (snapshot, team)
    }

    #[test]
    fn hand_changes_are_left_alone() {
        let (snapshot, mut team) = running_and_files();
        team.members["qa"].tab = Some("Revue".into());
        // An edit of qa: qa as its files say, the others as they run.
        let named = Named::by(&[set("qa", Field::Tab(Some("Revue".into())))]);
        let view = view(&snapshot, &team, &named);
        assert_eq!(view.members.keys().collect::<Vec<_>>(), ["lead", "dev", "old", "qa"]);
        assert_eq!(view.members["qa"].tab.as_deref(), Some("Revue"));
        assert_eq!(view.members["qa"].model.as_deref(), Some("sonnet"));
        assert_eq!(view.members["dev"].tab.as_deref(), Some("Code"));
        assert_eq!(view.members["dev"].role, "rôle de dev");
        assert_eq!(view.members["old"].role, "rôle de old");
        assert_eq!(view.contacts(), ["lead"]);
        // `old`, gone from the files by hand, is not removed: no pane closed, no session forgotten.
        assert!(named.removed.is_empty());
        // Removed and added by the edits: those only.
        let named = Named::by(&[Edit::Remove("dev".into()), add("ops")]);
        let mut with_ops = team.clone();
        with_ops.members.shift_remove("dev");
        with_ops.members.insert("ops".into(), config::Member { role: "r".into(), ..Default::default() });
        let view = super::view(&snapshot, &with_ops, &named);
        assert_eq!(view.members.keys().collect::<Vec<_>>(), ["lead", "old", "qa", "ops"]);
        assert_eq!(named.removed, ["dev"]);
        // Following the files whole (restart_all): the files' members, `old` out, `extra` in.
        let named = Named::all(&snapshot, &team);
        assert_eq!((named.removed.clone(), named.added.clone()), (strings(&["old"]), strings(&["extra"])));
        let view = super::view(&snapshot, &team, &named);
        assert_eq!(view.members.keys().collect::<Vec<_>>(), ["lead", "dev", "qa", "extra"]);
        assert_eq!(view.members["dev"].tab.as_deref(), Some("Ailleurs"));
    }

    #[test]
    fn gone_from_the_files_then_dismissed() {
        let (snapshot, team) = running_and_files();
        assert_eq!(gone(&snapshot, &team), ["old"]);
        let view = view(&snapshot, &team, &Named { removed: strings(&["old"]), ..Default::default() });
        assert_eq!(view.members.keys().collect::<Vec<_>>(), ["lead", "dev", "qa"]);
        assert_eq!(view.members["dev"].tab.as_deref(), Some("Code"));
    }

    #[test]
    fn renamed_in_place() {
        let (snapshot, mut team) = running_and_files();
        let dev = team.members.shift_remove_full("dev").unwrap();
        team.members.shift_insert(dev.0, "back".into(), dev.2);
        let view = view(&snapshot, &team, &Named::by(&[rename("dev", "back")]));
        assert_eq!(view.members.keys().collect::<Vec<_>>(), ["lead", "back", "old", "qa"]);
        assert_eq!(view.members["back"].tab.as_deref(), Some("Ailleurs"));
    }

    #[test]
    fn restarted_by_their_edits() {
        let edits = [
            set("dev", Field::Model(Some("opus".into()))),
            set("qa", Field::PermissionMode(Some("auto".into()))),
            rename("ops", "release"),
            rename("lead", "lead"),
            set("qa", Field::PermissionMode(None)),
        ];
        assert_eq!(touched(&edits), ["dev", "qa", "ops"]);
    }

    /// A team of `lead` and `dev`, its model `team_model`, `dev`'s own `model` and `effort`.
    fn team(team_model: Option<&str>, model: Option<&str>, effort: Option<&str>) -> Team {
        let mut team = Team { model: team_model.map(String::from), ..Default::default() };
        team.members.insert("lead".into(), config::Member { role: "r".into(), ..Default::default() });
        let dev = config::Member {
            role: "r".into(),
            model: model.map(String::from),
            effort: effort.map(String::from),
            ..Default::default()
        };
        team.members.insert("dev".into(), dev);
        team
    }

    /// `dev` launched with `--model opus --effort high`, with the mod.
    fn snapshot() -> Snapshot {
        let argv = ["claude", "--model", "opus", "--effort", "high", "-n", "dev"].map(String::from).to_vec();
        let dev = state::MemberInfo { name: "dev".into(), argv, ..Default::default() };
        let lead = state::MemberInfo { name: "lead".into(), argv: vec!["claude".into()], ..Default::default() };
        Snapshot { members: vec![lead, dev], plugin_dir: Some("/mod".into()), ..Default::default() }
    }

    #[test]
    fn started_again_when_requests_cannot_carry_it() {
        let before = team(None, Some("opus"), Some("high"));
        let resumed = |after: &Team| resumed(&snapshot(), &before, after, &[]);
        // Carried by each request: a known alias or id, a known effort; or back to what it was launched with.
        assert!(resumed(&team(None, Some("sonnet"), Some("high"))).is_empty());
        assert!(resumed(&team(None, Some("claude-opus-5-5"), Some("max"))).is_empty());
        assert!(resumed(&team(Some("opus"), None, Some("high"))).is_empty());
        // Claude Code's default once launched with another, a model with no id known here, an unknown effort.
        assert_eq!(resumed(&team(None, None, Some("high"))), ["dev"]);
        assert_eq!(resumed(&team(None, Some("opus[1m]"), Some("high"))), ["dev"]);
        assert_eq!(resumed(&team(None, Some("opus"), Some("turbo"))), ["dev"]);
        // Unchanged, even if it could not be carried: nothing.
        let odd = team(None, Some("opus[1m]"), Some("high"));
        assert!(super::resumed(&snapshot(), &odd, &odd, &[]).is_empty());
        // The permission mode, given at start only.
        let mut auto = before.clone();
        auto.members["dev"].permission_mode = Some("auto".into());
        assert_eq!(resumed(&auto), ["dev"]);
        auto.members.swap_remove("dev");
        auto.permission_mode = Some("auto".into());
        assert_eq!(resumed(&auto), ["lead"]);
    }

    #[test]
    fn without_the_mod_any_model_or_effort_starts_again() {
        let before = team(None, Some("opus"), Some("high"));
        let without = Snapshot { plugin_dir: None, ..snapshot() };
        let resumed = |after: &Team| resumed(&without, &before, after, &[]);
        assert_eq!(resumed(&team(None, Some("sonnet"), Some("high"))), ["dev"]);
        assert_eq!(resumed(&team(None, Some("opus"), Some("max"))), ["dev"]);
        // Back to what it was started with: nothing to do.
        let launched = team(None, Some("sonnet"), Some("high"));
        assert!(super::resumed(&without, &launched, &before, &[]).is_empty());
    }

    #[test]
    fn status_line_follows_the_main_contact() {
        // lead is the main contact; dev becomes it, lead marked no more.
        let before = team(None, Some("opus"), Some("high"));
        let mut after = before.clone();
        after.members["dev"].contact = true;
        let quiet = r#"{"statusLine":{"type":"command","command":"true"}}"#;
        let line = |snapshot: &Snapshot| {
            let mut snapshot = snapshot.clone();
            snapshot.members[1].argv.extend(["--settings".into(), quiet.into()]);
            snapshot
        };
        let with_line = Snapshot { status_line: true, ..line(&snapshot()) };
        assert_eq!(resumed(&with_line, &before, &after, &[]), ["lead", "dev"]);
        // No status line in the user's settings: no restart, the bars come with the next start.
        let without_line = Snapshot { status_line: false, ..snapshot() };
        assert!(resumed(&without_line, &before, &after, &[]).is_empty());
        // Another contact that is not the main one: nothing changes for the status line.
        let mut second = before.clone();
        second.members["lead"].contact = true;
        let mut both = second.clone();
        both.members["dev"].contact = true;
        assert!(resumed(&with_line, &second, &both, &[]).is_empty());
    }

    #[test]
    fn renamed_ones_by_their_names_before() {
        let before = team(None, Some("opus"), Some("high"));
        let mut after = before.clone();
        let dev = after.members.shift_remove("dev").unwrap();
        after.members.insert("back".into(), config::Member { model: None, ..dev });
        let renames = vec![("dev".to_string(), "back".to_string())];
        // Renamed: started again once, under its new name, its model left to Claude Code's default.
        let names = resumed(&snapshot(), &before, &after, &renames);
        assert_eq!(names, ["back"]);
        assert_eq!(before_names(names, &renames), ["dev"]);
    }
}
