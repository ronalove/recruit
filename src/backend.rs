// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What runs a team: recruit's own multiplexer (specs/multiplexeur.md), a server per team, which the [`Backend`] trait
//! drives; and what a launch asks of it ([`Plan`]).

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::i18n::Lang;
use crate::layout::Tab;
use crate::mux::native::Native;
use crate::mux::socket;
use crate::state::{self, Snapshot};
use crate::t;

/// What a launch asks the multiplexer to open.
#[derive(Debug, Clone)]
pub struct Plan {
    pub session: String,
    pub team: String,
    pub dir: PathBuf,
    pub columns: usize,
    pub tabs: Vec<TabPlan>,
    /// The team's language, for what the multiplexer writes itself.
    pub lang: Lang,
}

#[derive(Debug, Clone)]
pub struct TabPlan {
    pub title: String,
    pub panes: Vec<Pane>,
    /// The dashboard over the journal, in a column on the right; the members then stack in one column.
    pub side: Option<Side>,
}

#[derive(Debug, Clone)]
pub struct Side {
    pub dashboard: Pane,
    pub journal: Pane,
}

#[derive(Debug, Clone)]
pub struct Pane {
    /// Shown in the pane's frame: the member's name, or the panel's title.
    pub member: String,
    /// A panel's kind (`DASHBOARD`, `JOURNAL`); none for a member.
    pub role: Option<&'static str>,
    /// Run directly, without a shell in between.
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub const DASHBOARD: &str = "dashboard";
pub const JOURNAL: &str = "journal";

/// The journal's sizes, which Alt+j goes through in turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalSize {
    /// Half of the panels' column.
    Full,
    /// Its last messages only, the rest of the column to the dashboard. The journal opens so.
    Reduced,
    Hidden,
}

impl JournalSize {
    /// What the user is told once the journal took this size.
    pub fn said(self) -> String {
        match self {
            JournalSize::Full => t!("Journal complet.", "Journal at full size."),
            JournalSize::Reduced => {
                t!("Journal réduit à ses derniers messages.", "Journal reduced to its last messages.")
            }
            JournalSize::Hidden => t!("Journal masqué.", "Journal hidden."),
        }
    }
}

/// A pane of a running team, as its server lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneState {
    pub id: String,
    /// The member's name, or the panel's title.
    pub member: String,
    /// A panel's kind; empty for a member.
    pub role: String,
    /// Its tab.
    pub window: String,
}

#[derive(Debug, Clone)]
pub struct RunningTeam {
    pub session: String,
    pub team: String,
    pub dir: String,
    pub attached: bool,
}

/// A team's session name, which names its state folder and its server's socket: the team's name, `.` and `:` made
/// `_` (the names of recruit 1's tmux sessions, kept so that a team keeps its state).
pub fn session_name(team: &str) -> String {
    team.replace(['.', ':'], "_")
}

/// Whether the menu could be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuOpened {
    Opened,
    /// It was open already, on that terminal.
    AlreadyOpen,
    /// No terminal shows the team.
    NoClient,
}

pub trait Backend {
    // Teams.
    fn running(&self) -> Result<Vec<RunningTeam>>;
    fn has_session(&self, session: &str) -> bool;
    fn launch(&self, plan: &Plan) -> Result<()>;
    /// Takes over the terminal; returns only on error, or when the client leaves.
    fn attach(&self, session: &str) -> Result<()>;
    fn stop(&self, session: &str) -> Result<()>;

    // The members' panes.
    fn panes(&self, session: &str) -> Result<Vec<PaneState>>;
    /// Opens `pane` in a tab of its own; returns its id.
    fn open_window(&self, session: &str, dir: &Path, pane: &Pane) -> Result<String>;
    fn kill_pane(&self, id: &str) -> Result<()>;
    /// The name a pane shows, after a rename.
    fn set_member(&self, id: &str, member: &str) -> Result<()>;
    /// Starts `pane`'s program again in pane `id`.
    fn respawn(&self, id: &str, dir: &Path, pane: &Pane) -> Result<()>;
    /// Puts the panes in `tabs`, in their order, without stopping any.
    fn arrange(&self, session: &str, tabs: &[Tab], columns: usize) -> Result<()>;

    // The dashboard and the journal, programs in panes.
    fn open_panels(&self, dir: &Path, beside: &str, dashboard: &Pane, journal: &Pane) -> Result<()>;
    fn close_panels(&self, session: &str) -> Result<()>;
    fn restore_dashboard(&self, session: &str, dir: &Path, dashboard: &Pane) -> Result<()>;
    fn toggle_journal(&self, session: &str, dir: &Path, journal: &Pane) -> Result<JournalSize>;

    // Clients.
    fn detach(&self, client: &str) -> Result<()>;
    /// Opens the team's menu (its state in `state`): on the terminal that shows `member`, or on `client`, or on the
    /// only one.
    fn open_menu(&self, state: &Path, session: &str, member: Option<&str>, client: Option<&str>) -> Result<MenuOpened>;
    /// The terminals the team shows in, for the icons (Nerd Font or not).
    fn client_terminals(&self, session: &str) -> String;
}

/// Whether the multiplexer ran `session` and crashed (itself, or the machine): a server removes `server.json` when it
/// stops, even when `stop` has to kill it; a dead one's is left, and becomes the `crashed` mark once cleaned. Dead,
/// not slow: its lock is free.
pub fn crashed(session: &str) -> bool {
    let state = state::dir(session);
    let left = socket::read_info(&state).is_some() || socket::crashed_file(&state).exists();
    left && matches!(socket::try_lock(&state), Ok(Some(_)))
}

/// The team is running again: a crash before is no longer one.
pub fn recovered(session: &str) {
    let _ = std::fs::remove_file(socket::crashed_file(&state::dir(session)));
}

/// What runs `session`, or will.
pub fn for_session(session: &str) -> Box<dyn Backend> {
    Box::new(Native { state: state::dir(session) })
}

/// The teams running.
pub fn running() -> Vec<RunningTeam> {
    Native { state: PathBuf::new() }.running().unwrap_or_default()
}

/// What runs a team, as it was launched.
pub fn of(snapshot: &Snapshot) -> Box<dyn Backend> {
    for_session(&snapshot.session)
}
