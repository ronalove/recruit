// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A team's multiplexer seen from outside: a handle on its server, each method a request to it, by its socket. A team
//! is known by its state folder (`state::dir(session)`), where its server's lock and `server.json` are.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;

use super::client;
use super::plan::{Pane, Plan};
use super::proto::{ClientInfo, JournalSize, Kind, MenuOpened, PaneInfo, PaneSpec, Request, TabSpec};
use super::socket;
use crate::team::layout::Tab;
use crate::{t, team::state};

/// How long `Build` may take: it starts every pane.
const BUILD_WAIT: Duration = Duration::from_secs(30);

/// How long `Stop` may take: the panes have a few seconds to end.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// How long a server that does not answer `Stop` has after SIGTERM, before SIGKILL.
const KILL_WAIT: Duration = Duration::from_secs(3);

/// The size of the screen of a team launched without a terminal.
const DETACHED_SIZE: (u16, u16) = (200, 50);

/// A team running, as its server says it.
#[derive(Debug, Clone)]
pub(crate) struct RunningTeam {
    pub session: String,
    pub team: String,
    pub dir: String,
    pub attached: bool,
}

/// The teams whose state folder has a `server.json` and whose server answers. Without any, nothing but a look at the
/// folders: cheap, and safe without a server.
pub(crate) fn running() -> Vec<RunningTeam> {
    let teams = crate::sys::cache_dir().join("teams");
    let Ok(entries) = fs::read_dir(&teams) else { return Vec::new() };
    let mut running = Vec::new();
    for entry in entries.flatten() {
        let state = entry.path();
        if socket::read_info(&state).is_none() {
            continue;
        }
        let Ok(Some((_, welcome))) = client::hello(&state, Kind::Command) else { continue };
        let session = entry.file_name().to_string_lossy().into_owned();
        let team = if welcome.team.is_empty() { session.clone() } else { welcome.team };
        running.push(RunningTeam { session, team, dir: welcome.dir, attached: welcome.attached });
    }
    running.sort_by(|a, b| a.session.cmp(&b.session));
    running
}

/// The multiplexer of the team whose state is in `state`.
pub(crate) struct Mux {
    state: PathBuf,
}

impl Mux {
    /// The team whose state folder is `state`, when the caller already has it.
    pub(crate) fn at(state: &Path) -> Mux {
        Mux { state: state.to_path_buf() }
    }

    /// The team of `session`, by its state folder.
    pub(crate) fn of(session: &str) -> Mux {
        Mux::at(&state::dir(session))
    }

    /// `request` to the server, its answer read as `T`, within `wait`.
    fn ask_within<T: DeserializeOwned>(&self, request: Request, wait: Duration) -> Result<T> {
        let reply = client::request(&self.state, &request, wait)?.with_context(|| not_running(&self.state))?;
        reply.value()
    }

    /// [`Mux::ask_within`], within the usual time.
    fn ask<T: DeserializeOwned>(&self, request: Request) -> Result<T> {
        self.ask_within(request, client::ANSWER)
    }

    /// False at once without `server.json`; a dead server's leftovers are cleaned on the way (the lock is free). A
    /// server that holds its lock without answering (stopped, stuck) still runs: `stop` must be able to end it.
    pub(crate) fn is_running(&self) -> bool {
        if socket::read_info(&self.state).is_none() {
            return false;
        }
        match client::hello(&self.state, Kind::Command) {
            Ok(found) => found.is_some(),
            Err(_) => socket::try_lock(&self.state).is_ok_and(|lock| lock.is_none()),
        }
    }

    pub(crate) fn launch(&self, plan: &Plan) -> Result<()> {
        let mut tabs = Vec::new();
        for tab in &plan.tabs {
            let side = match &tab.side {
                Some(side) => Some((spec(&side.dashboard, &plan.dir)?, spec(&side.journal, &plan.dir)?)),
                None => None,
            };
            let panes = tab.panes.iter().map(|pane| spec(pane, &plan.dir)).collect::<Result<_>>()?;
            tabs.push(TabSpec { title: tab.title.clone(), panes, side });
        }
        // The plan checked first: no server is left without a team.
        start_server(&self.state)?;
        let (cols, rows) = crossterm::terminal::size().unwrap_or(DETACHED_SIZE);
        let build = Request::Build {
            team: plan.team.clone(),
            dir: plan.dir.to_string_lossy().into_owned(),
            columns: plan.columns,
            tabs,
            cols,
            rows,
            exe: crate::sys::exe()?.to_string_lossy().into_owned(),
            lang: plan.lang.code().to_string(),
            focus: plan.focus,
        };
        let built = self.ask_within::<()>(build, BUILD_WAIT);
        if built.is_err() {
            // No half-built team left behind.
            let _ = client::request(&self.state, &Request::Stop, STOP_WAIT);
        }
        built
    }

    /// Takes over the terminal; returns only on error, or when the client leaves.
    pub(crate) fn attach(&self) -> Result<()> {
        client::attach(&self.state)
    }

    /// Stops the team, and returns once its panes have ended. A server that does not answer gets SIGTERM, then
    /// SIGKILL, only while it holds its lock: the pid it noted is then surely its own.
    pub(crate) fn stop(&self) -> Result<()> {
        let state = &self.state;
        let error = match client::request(state, &Request::Stop, STOP_WAIT) {
            Ok(None) => return Ok(()),
            Ok(Some(reply)) => return reply.value(),
            Err(error) => error,
        };
        let Some(info) = socket::read_info(state) else { return Err(error) };
        if socket::try_lock(state)?.is_some() {
            // No server after all: it ended meanwhile.
            return Ok(());
        }
        let pid = info.pid as libc::pid_t;
        // SAFETY: a signal to the process that holds the state folder's lock.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let until = Instant::now() + KILL_WAIT;
        while !gone(pid) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        if !gone(pid) && socket::try_lock(state)?.is_none() {
            // SAFETY: as above, the lock still held.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        // Stopped on purpose: what it leaves goes without a crash's mark, under the lock, once it let it go.
        let until = Instant::now() + KILL_WAIT;
        loop {
            if let Some(_lock) = socket::try_lock(state)? {
                socket::clean_stopped(state, &info);
                return Ok(());
            }
            if Instant::now() >= until {
                return Err(error);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The panes of the team, tab by tab.
    pub(crate) fn panes(&self) -> Result<Vec<PaneInfo>> {
        self.ask(Request::Panes)
    }

    /// Opens `pane` in a tab of its own; returns its id.
    pub(crate) fn open_window(&self, dir: &Path, pane: &Pane) -> Result<String> {
        self.ask(Request::OpenWindow { pane: spec(pane, dir)? })
    }

    pub(crate) fn kill_pane(&self, id: &str) -> Result<()> {
        self.ask(Request::KillPane { id: id.to_string() })
    }

    /// The name a pane shows, after a rename.
    pub(crate) fn set_member(&self, id: &str, member: &str) -> Result<()> {
        self.ask(Request::SetMember { id: id.to_string(), member: member.to_string() })
    }

    /// Starts `pane`'s program again in pane `id`.
    pub(crate) fn respawn(&self, id: &str, dir: &Path, pane: &Pane) -> Result<()> {
        self.ask(Request::Respawn { id: id.to_string(), pane: spec(pane, dir)? })
    }

    /// Puts the panes in `tabs`, in their order, without stopping any. With `focus`, the tabs of members without the
    /// panels beside them in « focus » (`layout = "focus"`).
    pub(crate) fn arrange(&self, tabs: &[Tab], columns: usize, focus: bool) -> Result<()> {
        let tabs = tabs.iter().map(|tab| (tab.title.clone(), tab.members.clone())).collect();
        self.ask(Request::Arrange { tabs, columns, focus })
    }

    pub(crate) fn open_panels(&self, dir: &Path, beside: &str, dashboard: &Pane, journal: &Pane) -> Result<()> {
        self.ask(Request::OpenPanels {
            beside: beside.to_string(),
            dashboard: spec(dashboard, dir)?,
            journal: spec(journal, dir)?,
        })
    }

    pub(crate) fn close_panels(&self) -> Result<()> {
        self.ask(Request::ClosePanels)
    }

    pub(crate) fn restore_dashboard(&self, dir: &Path, dashboard: &Pane) -> Result<()> {
        self.ask(Request::RestoreDashboard { dashboard: spec(dashboard, dir)? })
    }

    pub(crate) fn toggle_journal(&self, dir: &Path, journal: &Pane) -> Result<JournalSize> {
        self.ask(Request::ToggleJournal { journal: spec(journal, dir)? })
    }

    pub(crate) fn detach(&self, client: &str) -> Result<()> {
        self.ask(Request::Detach { client: client.to_string() })
    }

    /// Opens the team's menu: on the terminal that shows `member`, or on `client`, or on the only one.
    pub(crate) fn open_menu(&self, member: Option<&str>, client: Option<&str>) -> Result<MenuOpened> {
        self.ask(Request::OpenMenu { member: member.map(String::from), client: client.map(String::from), field: None })
    }

    /// The terminals the team shows in, for the icons (Nerd Font or not); none when the server does not answer.
    pub(crate) fn clients(&self) -> Vec<ClientInfo> {
        self.ask(Request::Clients).unwrap_or_default()
    }
}

fn not_running(state: &Path) -> String {
    t!("l'équipe ne tourne pas (pas de serveur pour {})", "the team is not running (no server for {})", state.display())
}

/// A pane of the plan, as the server opens it, in `dir`.
fn spec(pane: &Pane, dir: &Path) -> Result<PaneSpec> {
    // Refused rather than changed: a folder whose name is not UTF-8 would be another folder once replaced.
    let cwd = dir.to_str().with_context(|| {
        t!("le nom du dossier n'est pas en UTF-8 : {}", "the directory's name is not UTF-8: {}", dir.display())
    })?;
    Ok(PaneSpec {
        member: pane.member.clone(),
        role: pane.role.unwrap_or_default().to_string(),
        argv: pane.argv.clone(),
        env: pane.env.clone(),
        cwd: cwd.to_string(),
    })
}

/// Starts the server of the state folder `state`, and returns once it is ready, or with what stopped it.
fn start_server(state: &Path) -> Result<()> {
    let exe = crate::sys::exe()?;
    // The language given: found from the system, it would start CoreFoundation's threads before the server forks.
    // Its stderr read to the end: the server puts its own on `server.log` before it says it is ready, so that the
    // end comes then; a descriptor 2 kept by the server would keep the launcher waiting.
    let output = Command::new(&exe)
        .args(["--lang", crate::i18n::lang().code()])
        .arg("_server")
        .arg(state)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("recruit _server")?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        bail!("{}", said.trim().trim_start_matches("recruit: "));
    }
    Ok(())
}

/// Whether `pid`'s process is gone.
fn gone(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks.
    unsafe { libc::kill(pid, 0) != 0 }
}
