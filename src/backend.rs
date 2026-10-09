// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What recruit asks of what runs a team: tmux, or its own multiplexer (specs/multiplexeur.md, `RECRUIT_BACKEND=native`
//! until it is the default). The trait grows by primitives until tmux leaves (step 4); the types it carries live in
//! `tmux.rs` until then.

use std::path::Path;

use anyhow::Result;

use crate::config::TmuxSettings;
use crate::layout::Tab;
use crate::mux::native::Native;
use crate::mux::socket;
use crate::state::{self, Snapshot};
use crate::tmux::Tmux;
pub use crate::tmux::{JournalSize, Pane, PaneState, Plan, RunningTeam};

/// Whether the menu could be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuOpened {
    Opened,
    /// It was open already, on that terminal: recruit's own multiplexer knows it (tmux opens nothing and says nothing).
    AlreadyOpen,
    /// No terminal shows the team.
    NoClient,
}

pub trait Backend {
    // Teams.
    fn running(&self) -> Result<Vec<RunningTeam>>;
    fn has_session(&self, session: &str) -> bool;
    fn launch(&self, plan: &Plan) -> Result<()>;
    /// Takes over the terminal; returns only on error, when the client leaves, or when switching from inside the
    /// same tmux server.
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

    // The dashboard and the journal, while they are programs in panes (until step 2).
    fn open_panels(&self, dir: &Path, beside: &str, dashboard: &Pane, journal: &Pane) -> Result<()>;
    fn close_panels(&self, session: &str) -> Result<()>;
    fn restore_dashboard(&self, session: &str, dir: &Path, dashboard: &Pane) -> Result<()>;
    fn toggle_journal(&self, session: &str, dir: &Path, journal: &Pane) -> Result<JournalSize>;

    // Clients.
    /// Shows `member`'s pane, in its tab; false when it has none.
    // No caller: under tmux, `recruit _click` focuses by tmux itself; recruit's server focuses within itself. Goes with
    // tmux, at step 4, unless something outside the server needs it by then.
    #[allow(dead_code)]
    fn focus(&self, session: &str, member: &str) -> Result<bool>;
    fn detach(&self, client: &str) -> Result<()>;
    /// Opens the team's menu (its state in `state`): on the terminal that shows `member`, or on `client`, or on the
    /// only one.
    fn open_menu(&self, state: &Path, session: &str, member: Option<&str>, client: Option<&str>) -> Result<MenuOpened>;
    /// The terminals the team shows in, for the icons (Nerd Font or not).
    fn client_terminals(&self, session: &str) -> String;
}

/// What runs a team.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// recruit's tmux server; the teams launched before the field existed.
    #[default]
    Tmux,
    /// recruit's own multiplexer.
    Native,
}

/// What a team launched now runs on: recruit's own multiplexer with `RECRUIT_BACKEND=native`, hidden, until it is the
/// default (step 4); tmux otherwise.
pub fn requested() -> Kind {
    match std::env::var("RECRUIT_BACKEND") {
        Ok(backend) if backend == "native" => Kind::Native,
        _ => Kind::Tmux,
    }
}

/// The backend of a team that may be running as `session`: the one it was launched on while it runs, whatever is
/// asked for now (a second copy of a running team must never start on the other one); else the one asked for now.
pub fn for_session(session: &str, settings: &TmuxSettings) -> Result<(Kind, Box<dyn Backend>)> {
    let launched = Snapshot::read(&state::dir(session)).map(|s| s.backend).unwrap_or_default();
    match launched {
        Kind::Native => {
            let native = Native { state: state::dir(session) };
            if native.has_session(session) {
                return Ok((Kind::Native, Box::new(native)));
            }
        }
        // Without a tmux recruit can use, no team of its runs there: recruit's own multiplexer needs none.
        Kind::Tmux => {
            if let Ok(tmux) = Tmux::new(settings)
                && tmux.has_session(session)
            {
                return Ok((Kind::Tmux, Box::new(tmux)));
            }
        }
    }
    let kind = requested();
    Ok((kind, new(kind, settings, session)?))
}

/// Whether recruit's own multiplexer ran `session` and crashed (itself, or the machine): a server removes
/// `server.json` when it stops, even when `stop` has to kill it; a dead one's is left, and becomes the `crashed` mark
/// once cleaned. Dead, not slow: its lock is free.
pub fn crashed(session: &str) -> bool {
    let state = state::dir(session);
    let left = socket::read_info(&state).is_some() || socket::crashed_file(&state).exists();
    left && matches!(socket::try_lock(&state), Ok(Some(_)))
}

/// The team is running again: a crash before is no longer one.
pub fn recovered(session: &str) {
    let _ = std::fs::remove_file(socket::crashed_file(&state::dir(session)));
}

/// A backend of `kind` for `session`; tmux's version is checked.
pub fn new(kind: Kind, settings: &TmuxSettings, session: &str) -> Result<Box<dyn Backend>> {
    Ok(match kind {
        Kind::Tmux => Box::new(Tmux::new(settings)?),
        Kind::Native => Box::new(Native { state: state::dir(session) }),
    })
}

/// The teams running, on either backend.
pub fn running(settings: &TmuxSettings) -> Vec<(Kind, RunningTeam)> {
    let mut teams: Vec<(Kind, RunningTeam)> = Tmux::new(settings)
        .and_then(|tmux| tmux.running())
        .unwrap_or_default()
        .into_iter()
        .map(|team| (Kind::Tmux, team))
        .collect();
    let native = Native { state: std::path::PathBuf::new() };
    teams.extend(native.running().unwrap_or_default().into_iter().map(|team| (Kind::Native, team)));
    teams
}

/// The backend of a running team, as it was launched.
pub fn of(snapshot: &Snapshot) -> Box<dyn Backend> {
    match snapshot.backend {
        Kind::Tmux => {
            Box::new(Tmux::running(&TmuxSettings { socket: Some(snapshot.socket.clone()), ..Default::default() }))
        }
        Kind::Native => Box::new(Native { state: state::dir(&snapshot.session) }),
    }
}
