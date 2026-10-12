// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The panes of the team: opened, started again, closed, arranged in tabs, and kept in order (the panels, the engines).

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use anyhow::{Result, bail};

use super::Server;
use super::lifecycle::log;
use super::pane::{Command, Pane, Role, Wires};
use super::tabs::{Side, Tab};
use crate::mux::Caps;
use crate::mux::proto::{JournalSize, PaneSpec, TabSpec};
use crate::t;

impl Server {
    /// Engines' deadlines due: a synchronized update that never ended, spare work.
    pub(super) fn expire(&mut self) {
        let now = Instant::now();
        self.pacing.deadline = None;
        let held = self.gate.hold();
        for pane in &self.panes {
            let done = pane.with(|engine| {
                let changed = engine.deadline().is_some_and(|at| at <= now) && engine.expire(now);
                (changed, engine.deadline())
            });
            if let Some((changed, deadline)) = done {
                self.dirty |= changed;
                self.pacing.deadline = [self.pacing.deadline, deadline].into_iter().flatten().min();
            }
        }
        drop(held);
    }

    /// Broken engines replaced.
    pub(super) fn repair(&mut self) {
        let caps = self.caps();
        for pane in self.panes.iter_mut().filter(|pane| pane.broken.is()) {
            if let Err(error) = pane.repair(&caps) {
                log!("pane {}: {error:#}", pane.id);
            }
            self.dirty = true;
        }
    }

    /// Opens a pane in tab `tab` (made if missing; the active one when `None`), and gives it the focus if it is the
    /// first.
    pub(super) fn open(&mut self, spec: PaneSpec, tab: Option<String>) -> Result<String> {
        let id = self.spawn(spec)?;
        let at = match tab {
            Some(title) => match self.tabs.iter().position(|tab| tab.title == title) {
                Some(at) => at,
                None => {
                    self.tabs.push(Tab::new(title, Vec::new()));
                    self.tabs.len() - 1
                }
            },
            None if self.tabs.is_empty() => {
                self.tabs.push(Tab::new("1".into(), Vec::new()));
                0
            }
            None => self.active,
        };
        self.tabs[at].panes.push(id.clone());
        if self.focus.is_none() {
            self.focus = Some(id.clone());
            self.active = at;
        }
        self.layout();
        Ok(id)
    }

    /// Starts a pane as `spec` says, in no tab yet: its id.
    pub(super) fn spawn(&mut self, spec: PaneSpec) -> Result<String> {
        let command = self.command_of(&spec)?;
        self.next_pane += 1;
        let id = format!("p{}", self.next_pane);
        let (caps, wires) = (self.caps(), self.wires());
        // Its size comes with the layout; a first guess meanwhile.
        let size = (self.size.0.max(3) - 2, self.size.1.saturating_sub(3).max(1));
        let pane = Pane::open(id.clone(), spec.member, Role::of(&spec.role), command, size, &caps, &wires)?;
        self.panes.push(pane);
        self.started = true;
        Ok(id)
    }

    /// What `spec` runs, in an absolute folder, the variables the last client gave over its environment.
    fn command_of(&self, spec: &PaneSpec) -> Result<Command> {
        if !Path::new(&spec.cwd).is_absolute() {
            bail!(t!("dossier relatif : {}", "relative directory: {}", spec.cwd));
        }
        let mut command = Command { argv: spec.argv.clone(), cwd: PathBuf::from(&spec.cwd), env: spec.env.clone() };
        for (name, value) in &self.env {
            command.env.retain(|(key, _)| key != name);
            if let Some(value) = value {
                command.env.push((name.clone(), value.clone()));
            }
        }
        Ok(command)
    }

    /// The terminal of the client, as the engines of new panes are told; the default without one.
    fn caps(&self) -> Caps {
        self.client.as_ref().map(|client| client.caps.clone()).unwrap_or_default()
    }

    /// What a pane's threads share with the loop.
    fn wires(&self) -> Wires {
        Wires { tx: self.tx.clone(), gate: Arc::clone(&self.gate) }
    }

    /// Removes pane `id`, its program asked to end if it still runs.
    pub(super) fn close_pane(&mut self, id: &str) {
        let Some(i) = self.panes.iter().position(|pane| pane.id == id) else { return };
        // Dropped: its program's group is stopped.
        self.panes.remove(i);
        // The menu gives the focus back to the pane that had it, if it is still there.
        let mut back = None;
        if self.menu.as_ref().is_some_and(|(menu, _)| menu == id) {
            back = self.menu.take().and_then(|(_, before)| before);
            self.list = None;
        }
        for tab in &mut self.tabs {
            tab.forget(id);
        }
        let before = self.tabs.len();
        self.tabs.retain(|tab| !tab.ids().is_empty());
        if self.tabs.len() < before {
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        }
        if self.focus.as_deref() == Some(id) {
            // Gone: no focus out to tell. Else the pane that had it, or the first of the tab shown; told (1004).
            self.focus = None;
            let first = self.tabs.get(self.active).and_then(|tab| tab.ids().first().cloned());
            if let Some(next) = back.filter(|back| self.pane(back).is_some()).or(first) {
                self.focus_on(&next);
            }
        }
        if self.pointer.grab.as_deref() == Some(id) {
            self.pointer.grab = None;
        }
        self.layout();
    }

    /// A pane by its id, else by its member's name, else by the role of a pane that is no member's (« menu »,
    /// « dashboard », « journal »).
    pub(super) fn find(&self, which: &str) -> Option<&Pane> {
        self.pane(which)
            .or_else(|| self.panes.iter().find(|pane| pane.member == which))
            .or_else(|| self.panes.iter().find(|pane| !pane.is_member() && pane.role.name() == which))
    }

    /// The team as `launch` plans it, in a server still empty.
    pub(super) fn build(
        &mut self,
        team: String,
        dir: String,
        columns: usize,
        tabs: Vec<TabSpec>,
        size: (u16, u16),
    ) -> Result<()> {
        if self.started {
            bail!(t!("l'équipe est déjà construite", "the team is already built"));
        }
        self.team = team.clone();
        {
            let mut welcome = self.welcome.lock().unwrap_or_else(PoisonError::into_inner);
            welcome.team = team;
            welcome.dir = dir;
        }
        self.columns = columns.max(1);
        if self.client.is_none() && size.0 > 0 && size.1 > 0 {
            self.size = size;
        }
        for spec in tabs {
            let mut panes = Vec::new();
            for pane in spec.panes {
                panes.push(self.spawn(pane)?);
            }
            let mut tab = Tab::new(spec.title, panes);
            if let Some((dashboard, journal)) = spec.side {
                self.journal = Some(journal.clone());
                tab.side = Some(Side { dashboard: Some(self.spawn(dashboard)?), journal: Some(self.spawn(journal)?) });
            }
            self.tabs.push(tab);
        }
        self.reduced = true;
        self.active = 0;
        self.focus = self.tabs.first().and_then(|tab| tab.ids().first().cloned());
        self.layout();
        Ok(())
    }

    /// Pane `id`'s program started again, in place.
    pub(super) fn respawn(&mut self, id: &str, spec: PaneSpec) -> Result<()> {
        let command = self.command_of(&spec)?;
        let (caps, wires) = (self.caps(), self.wires());
        let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) else {
            bail!(t!("panneau inconnu : {}", "unknown pane: {}", id));
        };
        pane.respawn(command, &caps, &wires)?;
        self.dirty = true;
        Ok(())
    }

    /// The members' panes put in `tabs`, none stopped; the panels' column beside the first. A member's pane in none
    /// of them keeps a tab of its own, at the end.
    pub(super) fn arrange(&mut self, tabs: Vec<(String, Vec<String>)>, columns: usize) {
        self.columns = columns.max(1);
        let side = self.tabs.iter_mut().find_map(|tab| tab.side.take());
        let mut arranged: Vec<Tab> = tabs
            .iter()
            .map(|(title, members)| {
                Tab::new(
                    title.clone(),
                    members.iter().filter_map(|m| self.member_pane(m)).map(|pane| pane.id.clone()).collect(),
                )
            })
            .filter(|tab| !tab.panes.is_empty())
            .collect();
        let placed: Vec<String> = arranged.iter().flat_map(|tab| tab.panes.clone()).collect();
        for pane in self.panes.iter().filter(|pane| pane.is_member() && !placed.contains(&pane.id)) {
            arranged.push(Tab::new(pane.member.clone(), vec![pane.id.clone()]));
        }
        match arranged.first_mut() {
            Some(first) => first.side = side,
            None => {
                if let Some(side) = side {
                    arranged.push(Tab { side: Some(side), ..Tab::new(String::new(), Vec::new()) });
                }
            }
        }
        // Each tab keeps the pane it last had the focus on, wherever that pane went; and its zoom, if the pane zoomed
        // stays in a tab of the same title (any change from the menu ends here: a new effort keeps the zoom).
        let lasts: Vec<String> = self.tabs.iter().filter_map(|tab| tab.last.clone()).collect();
        let zooms: Vec<(&str, &str)> =
            self.tabs.iter().filter_map(|tab| Some((tab.title.as_str(), tab.zoomed.as_deref()?))).collect();
        for tab in &mut arranged {
            tab.last = lasts.iter().find(|last| tab.has(last)).cloned();
            let zoom = zooms.iter().find(|(title, id)| *title == tab.title && tab.panes.iter().any(|pane| pane == id));
            tab.zoomed = zoom.map(|(_, id)| id.to_string());
        }
        // ⌥l and the places of a tab in « focus » stay with a tab of the same title.
        for tab in &mut arranged {
            if let Some(old) = self.tabs.iter_mut().find(|old| old.title == tab.title) {
                tab.mode = old.mode;
                tab.focus = old.focus.take();
            }
        }
        self.tabs = arranged;
        self.active = self.focus.as_deref().and_then(|id| self.tabs.iter().position(|tab| tab.has(id))).unwrap_or(0);
        self.layout();
    }

    /// The dashboard over the reduced journal, beside pane `beside`.
    pub(super) fn open_panels(&mut self, beside: &str, dashboard: PaneSpec, journal: PaneSpec) -> Result<()> {
        let Some(at) = self.tabs.iter().position(|tab| tab.has(beside)) else {
            bail!(t!("panneau inconnu : {}", "unknown pane: {}", beside));
        };
        self.journal = Some(journal.clone());
        let side = Side { dashboard: Some(self.spawn(dashboard)?), journal: Some(self.spawn(journal)?) };
        self.tabs[at].side = Some(side);
        self.reduced = true;
        self.layout();
        Ok(())
    }

    /// The dashboard and the journal closed.
    pub(super) fn close_panels(&mut self) {
        let panels: Vec<String> = self
            .panes
            .iter()
            .filter(|pane| matches!(pane.role, Role::Dashboard | Role::Journal))
            .map(|pane| pane.id.clone())
            .collect();
        for id in panels {
            self.close_pane(&id);
        }
        self.journal = None;
    }

    /// The dashboard opened again: over the journal, which keeps its size, else beside the first member.
    pub(super) fn restore_dashboard(&mut self, dashboard: PaneSpec) -> Result<()> {
        let at = self.tabs.iter().position(|tab| tab.side.is_some()).unwrap_or(0);
        if self.tabs.is_empty() {
            bail!(t!("l'équipe n'a aucun panneau", "the team has no pane"));
        }
        let id = self.spawn(dashboard)?;
        self.tabs[at].side.get_or_insert_with(Side::default).dashboard = Some(id);
        self.layout();
        Ok(())
    }

    /// The journal to its next size: full, reduced, hidden (closed), full again.
    pub(super) fn toggle_journal(&mut self, journal: PaneSpec) -> Result<JournalSize> {
        self.journal = Some(journal.clone());
        let open = self.tabs.iter().position(|tab| tab.side.as_ref().is_some_and(|side| side.journal.is_some()));
        let next = match (open, self.reduced) {
            (None, _) => JournalSize::Full,
            (Some(_), false) => JournalSize::Reduced,
            (Some(_), true) => JournalSize::Hidden,
        };
        match next {
            JournalSize::Reduced => self.reduced = true,
            JournalSize::Hidden => {
                let id = open.and_then(|at| self.tabs[at].side.as_ref()?.journal.clone());
                if let Some(id) = id {
                    self.close_pane(&id);
                }
            }
            JournalSize::Full => {
                let Some(at) = self.tabs.iter().position(|tab| tab.side.is_some()) else {
                    bail!(t!(
                        "le tableau de bord est fermé : relancer recruit le rouvre",
                        "the dashboard is closed: running recruit again opens it"
                    ));
                };
                let id = self.spawn(journal)?;
                self.tabs[at].side.get_or_insert_with(Side::default).journal = Some(id);
                self.reduced = false;
            }
        }
        self.layout();
        Ok(next)
    }
}
