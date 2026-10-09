// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! recruit's own multiplexer behind the `Backend` trait: each method a request to the team's server
//! (specs/multiplexeur-serveur.md §3.5 and §4), by its socket. A team is known by its state folder
//! (`state::dir(session)`), where its server's lock and `server.json` are.
//!
//! Owner: dev-serveur (the trait: architecte).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;

use super::client;
use super::proto::{ClientInfo, MenuOpened as Opened, PaneInfo, PaneSpec, Reply, ReplyError, Request, TabSpec};
use super::socket;
use crate::backend::{Backend, JournalSize, MenuOpened, Pane, PaneState, Plan, RunningTeam};
use crate::layout::Tab;
use crate::{state, t};

/// How long `Build` may take: it starts every pane.
const BUILD_WAIT: Duration = Duration::from_secs(30);

/// How long `Stop` may take: the panes have a few seconds to end.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// How long a server that does not answer `Stop` has after SIGTERM, before SIGKILL.
const KILL_WAIT: Duration = Duration::from_secs(3);

/// The size of the screen of a team launched without a terminal.
const DETACHED_SIZE: (u16, u16) = (200, 50);

/// The native backend, for the team whose state is in `state` (or for all teams, for `running`).
pub(crate) struct Native {
    pub state: PathBuf,
}

impl Native {
    /// The state folder of `session`: this backend's own, or another one's (a `--dry-run` session).
    fn state_of(&self, session: &str) -> PathBuf {
        if self.state.file_name().is_some_and(|name| name == session) {
            self.state.clone()
        } else {
            state::dir(session)
        }
    }

    /// `request` to the server, its answer read as `T`.
    fn ask<T: DeserializeOwned>(&self, state: &Path, request: Request, wait: Duration) -> Result<T> {
        let reply = client::request(state, &request, wait)?.with_context(|| not_running(state))?;
        value(reply)
    }

    fn ask_unit(&self, request: Request) -> Result<()> {
        self.ask::<()>(&self.state, request, client::ANSWER)
    }
}

fn not_running(state: &Path) -> String {
    t!("l'équipe ne tourne pas (pas de serveur pour {})", "the team is not running (no server for {})", state.display())
}

/// An answer's value, or its error.
fn value<T: DeserializeOwned>(reply: Reply) -> Result<T> {
    reply.value().map_err(|error| match error {
        ReplyError::Failed(message) => anyhow::anyhow!(message),
        ReplyError::Refused(refusal) => anyhow::anyhow!("refused: {refusal:?}"),
        ReplyError::Invalid(why) => anyhow::anyhow!("answer: {why}"),
    })
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
    let exe = std::env::current_exe().context("recruit")?;
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

impl Backend for Native {
    /// The teams whose state folder has a `server.json` and whose server answers. Without any, nothing but a look
    /// at the folders: cheap, and safe without a server.
    fn running(&self) -> Result<Vec<RunningTeam>> {
        let teams = crate::config::cache_dir().join("teams");
        let Ok(entries) = fs::read_dir(&teams) else { return Ok(Vec::new()) };
        let mut running = Vec::new();
        for entry in entries.flatten() {
            let state = entry.path();
            if socket::read_info(&state).is_none() {
                continue;
            }
            let Ok(Some((_, welcome))) = client::hello(&state, super::proto::Kind::Command) else { continue };
            let session = entry.file_name().to_string_lossy().into_owned();
            let team = if welcome.team.is_empty() { session.clone() } else { welcome.team };
            running.push(RunningTeam { session, team, dir: welcome.dir, attached: welcome.attached });
        }
        running.sort_by(|a, b| a.session.cmp(&b.session));
        Ok(running)
    }

    /// False at once without `server.json`; a dead server's leftovers are cleaned on the way (the lock is free). A
    /// server that holds its lock without answering (stopped, stuck) still runs: `stop` must be able to end it.
    fn has_session(&self, session: &str) -> bool {
        let state = self.state_of(session);
        if socket::read_info(&state).is_none() {
            return false;
        }
        match client::hello(&state, super::proto::Kind::Command) {
            Ok(found) => found.is_some(),
            Err(_) => socket::try_lock(&state).is_ok_and(|lock| lock.is_none()),
        }
    }

    fn launch(&self, plan: &Plan) -> Result<()> {
        let state = self.state_of(&plan.session);
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
        start_server(&state)?;
        let (cols, rows) = crossterm::terminal::size().unwrap_or(DETACHED_SIZE);
        let build = Request::Build {
            team: plan.team.clone(),
            dir: plan.dir.to_string_lossy().into_owned(),
            columns: plan.columns,
            tabs,
            cols,
            rows,
            exe: std::env::current_exe().context("recruit")?.to_string_lossy().into_owned(),
            lang: plan.lang.code().to_string(),
        };
        let built = self.ask::<()>(&state, build, BUILD_WAIT);
        if built.is_err() {
            // No half-built team left behind.
            let _ = client::request(&state, &Request::Stop, STOP_WAIT);
        }
        built
    }

    fn attach(&self, session: &str) -> Result<()> {
        client::attach(&self.state_of(session))
    }

    /// Stops the team, and returns once its panes have ended. A server that does not answer gets SIGTERM, then
    /// SIGKILL, only while it holds its lock: the pid it noted is then surely its own.
    fn stop(&self, session: &str) -> Result<()> {
        let state = self.state_of(session);
        let error = match client::request(&state, &Request::Stop, STOP_WAIT) {
            Ok(None) => return Ok(()),
            Ok(Some(reply)) => return value(reply),
            Err(error) => error,
        };
        let Some(info) = socket::read_info(&state) else { return Err(error) };
        if socket::try_lock(&state)?.is_some() {
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
        if !gone(pid) && socket::try_lock(&state)?.is_none() {
            // SAFETY: as above, the lock still held.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        // Stopped on purpose: what it leaves goes without a crash's mark, under the lock, once it let it go.
        let until = Instant::now() + KILL_WAIT;
        loop {
            if let Some(_lock) = socket::try_lock(&state)? {
                socket::clean_stopped(&state, &info);
                return Ok(());
            }
            if Instant::now() >= until {
                return Err(error);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn panes(&self, session: &str) -> Result<Vec<PaneState>> {
        let panes: Vec<PaneInfo> = self.ask(&self.state_of(session), Request::Panes, client::ANSWER)?;
        Ok(panes
            .into_iter()
            .map(|pane| PaneState { id: pane.id, member: pane.member, role: pane.role, window: pane.tab })
            .collect())
    }

    fn open_window(&self, session: &str, dir: &Path, pane: &Pane) -> Result<String> {
        self.ask(&self.state_of(session), Request::OpenWindow { pane: spec(pane, dir)? }, client::ANSWER)
    }

    fn kill_pane(&self, id: &str) -> Result<()> {
        self.ask_unit(Request::KillPane { id: id.to_string() })
    }

    fn set_member(&self, id: &str, member: &str) -> Result<()> {
        self.ask_unit(Request::SetMember { id: id.to_string(), member: member.to_string() })
    }

    fn respawn(&self, id: &str, dir: &Path, pane: &Pane) -> Result<()> {
        self.ask_unit(Request::Respawn { id: id.to_string(), pane: spec(pane, dir)? })
    }

    fn arrange(&self, session: &str, tabs: &[Tab], columns: usize) -> Result<()> {
        let tabs = tabs.iter().map(|tab| (tab.title.clone(), tab.members.clone())).collect();
        self.ask(&self.state_of(session), Request::Arrange { tabs, columns }, client::ANSWER)
    }

    fn open_panels(&self, dir: &Path, beside: &str, dashboard: &Pane, journal: &Pane) -> Result<()> {
        let request = Request::OpenPanels {
            beside: beside.to_string(),
            dashboard: spec(dashboard, dir)?,
            journal: spec(journal, dir)?,
        };
        self.ask_unit(request)
    }

    fn close_panels(&self, session: &str) -> Result<()> {
        self.ask(&self.state_of(session), Request::ClosePanels, client::ANSWER)
    }

    fn restore_dashboard(&self, session: &str, dir: &Path, dashboard: &Pane) -> Result<()> {
        let request = Request::RestoreDashboard { dashboard: spec(dashboard, dir)? };
        self.ask(&self.state_of(session), request, client::ANSWER)
    }

    fn toggle_journal(&self, session: &str, dir: &Path, journal: &Pane) -> Result<JournalSize> {
        let size: String =
            self.ask(&self.state_of(session), Request::ToggleJournal { journal: spec(journal, dir)? }, client::ANSWER)?;
        Ok(match size.as_str() {
            "full" => JournalSize::Full,
            "reduced" => JournalSize::Reduced,
            _ => JournalSize::Hidden,
        })
    }

    fn focus(&self, session: &str, member: &str) -> Result<bool> {
        self.ask(&self.state_of(session), Request::Focus { member: member.to_string() }, client::ANSWER)
    }

    fn detach(&self, client: &str) -> Result<()> {
        self.ask_unit(Request::Detach { client: client.to_string() })
    }

    fn open_menu(&self, state: &Path, session: &str, member: Option<&str>, client: Option<&str>) -> Result<MenuOpened> {
        let _ = session;
        let request = Request::OpenMenu { member: member.map(String::from), client: client.map(String::from) };
        let opened: Opened = self.ask(state, request, client::ANSWER)?;
        Ok(match opened {
            Opened::Opened => MenuOpened::Opened,
            Opened::NoClient => MenuOpened::NoClient,
            Opened::AlreadyOpen => MenuOpened::AlreadyOpen,
        })
    }

    /// The terminals the team shows in, one per line: `TERM`, a tab, the terminal's name and version.
    fn client_terminals(&self, session: &str) -> String {
        let clients: Vec<ClientInfo> =
            self.ask(&self.state_of(session), Request::Clients, client::ANSWER).unwrap_or_default();
        clients
            .iter()
            .map(|client| format!("{}\t{}", client.term, client.name.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
