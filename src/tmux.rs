// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Terminal backends. Only tmux for now: recruit runs its own tmux server (`tmux -L recruit`) with its own
//! configuration, so a team outlives the terminal window and can be attached from anywhere.

use std::fs;
use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::{TmuxSettings, cache_dir, home};
use crate::i18n::Lang;
use crate::layout::{self, Host, SidePanes, Tab, column_heights};
use crate::t;

/// What a backend is asked to open.
#[derive(Debug, Clone)]
pub struct Plan {
    pub session: String,
    pub team: String,
    pub dir: PathBuf,
    pub columns: usize,
    pub tabs: Vec<TabPlan>,
    /// Where the team's panels find it (`recruit _panel … <state>`).
    pub state: PathBuf,
    /// Shell command bound to Alt+j: takes the journal to its next size.
    pub toggle: String,
    /// Shell command run by a click on a panel: goes to the member clicked.
    pub click: String,
    /// Shell command that opens the team's menu in a popup: Alt+r and the bar's button.
    pub menu: String,
    /// The team's language, for the quit menu.
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
    /// Shown in the pane's border: the member's name, or the panel's title.
    pub member: String,
    /// A panel's kind (`DASHBOARD`, `JOURNAL`); none for a member.
    pub role: Option<&'static str>,
    /// Run directly, without a shell in between.
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub const DASHBOARD: &str = "dashboard";
pub const JOURNAL: &str = "journal";
/// How the Alt key is written before a key in what recruit shows: the Option symbol on macOS, `Alt+` elsewhere.
pub const ALT: &str = if cfg!(target_os = "macos") { "⌥" } else { "Alt+" };
/// Width of the panels' column: `layout::SIDE_PERCENT`.
const SIDE_WIDTH: &str = "35%";
/// Rows of the reduced journal: its last three messages, two lines each with none between them, then the line the
/// next one starts on.
const REDUCED_ROWS: usize = 3 * 2 + 1;
/// Marks the pane of a reduced journal.
const REDUCED: &str = "@recruit_reduced";

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
    /// Full, reduced, hidden, then full again.
    pub fn next(self) -> Self {
        match self {
            JournalSize::Full => JournalSize::Reduced,
            JournalSize::Reduced => JournalSize::Hidden,
            JournalSize::Hidden => JournalSize::Full,
        }
    }

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

/// A pane of a running team, as tmux lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneState {
    pub id: String,
    /// The member's name, or the panel's title.
    pub member: String,
    /// A panel's kind; empty for a member.
    pub role: String,
    /// Its window's id (`@3`).
    pub window: String,
}

/// A window of a running team, as tmux lists it.
#[derive(Debug, Clone, PartialEq)]
struct WindowState {
    id: String,
    title: String,
    width: usize,
    height: usize,
}

/// A click on a panel, as the binding left it in the pane's options.
#[derive(Debug, Clone, PartialEq)]
pub struct Click {
    /// The panel's kind (`DASHBOARD`, `JOURNAL`).
    pub role: String,
    /// The pane's width.
    pub columns: usize,
    /// Column and row in the pane, from 0.
    pub x: usize,
    pub y: usize,
    /// The tmux client clicked in, where to show a menu.
    pub client: String,
    /// The text of the row clicked, without its styles: what the members wrote, never given to a shell.
    pub line: String,
}

#[derive(Debug, Clone)]
pub struct RunningTeam {
    pub session: String,
    pub team: String,
    pub dir: String,
    pub attached: bool,
}

pub trait Backend {
    fn running(&self) -> Result<Vec<RunningTeam>>;
    fn launch(&self, plan: &Plan) -> Result<()>;
    /// Takes over the terminal; returns only on error or when switching from inside the same server.
    fn attach(&self, session: &str) -> Result<()>;
    fn stop(&self, session: &str) -> Result<()>;
}

pub struct Tmux {
    socket: String,
    settings: TmuxSettings,
}

/// tmux 3.5: the menus' `-M` and `-O` (mouse in a menu opened from a key or from `recruit _click`), after
/// `#[range=user]` in the status line (3.4), `allow-passthrough` (3.3), `extended-keys` and `new-session -e` (3.2).
/// `display-popup` runs its command through `default-shell` since 3.5: the popup's is a plain command line.
const MIN_VERSION: (u32, u32) = (3, 5);

impl Tmux {
    /// recruit's tmux server for a team already running: its version was checked when it was launched, and
    /// asking it again (`tmux -V`) would cost a few milliseconds before the menu shows.
    pub fn running(settings: &TmuxSettings) -> Self {
        Tmux { socket: settings.socket().to_string(), settings: settings.clone() }
    }

    pub fn new(settings: &TmuxSettings) -> Result<Self> {
        let output = Command::new("tmux").arg("-V").output().map_err(|_| {
            anyhow::anyhow!(t!(
                "tmux est introuvable. Installe-le : brew install tmux (macOS), apt install tmux (Debian, Ubuntu)…",
                "tmux not found. Install it: brew install tmux (macOS), apt install tmux (Debian, Ubuntu)…"
            ))
        })?;
        let version = String::from_utf8_lossy(&output.stdout);
        if let Some(found) = parse_version(&version)
            && found < MIN_VERSION
        {
            bail!(t!(
                "{} est trop ancien : recruit demande tmux {}.{} ou plus récent",
                "{} is too old: recruit needs tmux {}.{} or newer",
                version.trim(),
                MIN_VERSION.0,
                MIN_VERSION.1
            ));
        }
        Ok(Tmux { socket: settings.socket().to_string(), settings: settings.clone() })
    }

    fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.args(["-L", &self.socket]).env_remove("TMUX");
        command
    }

    fn run<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = self.command();
        command.args(args);
        let output = command.output().context("tmux")?;
        if !output.status.success() {
            bail!("tmux: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
    }

    pub fn has_session(&self, session: &str) -> bool {
        self.command()
            .args(["has-session", "-t", &exact(session)])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// recruit's tmux configuration, read once when its server starts.
    pub fn config(&self) -> String {
        let mut lines: Vec<String> = [
            "set -g base-index 1",
            "setw -g pane-base-index 1",
            "set -g renumber-windows on",
            "set -g history-limit 100000",
            "set -sg escape-time 10",
            "set -g focus-events on",
            "set -g set-titles on",
            "set -g set-titles-string '#S · #W'",
            "set -g status-left '#[bold] #S #[default] '",
            "set -g status-left-length 40",
            "setw -g pane-border-status top",
            "setw -g pane-border-format ' #{?#{@recruit_member},#{@recruit_member},#{pane_title}} '",
            // Tabs without the prefix: Alt+Shift+←/→ for the previous or next one (plain Alt+←/→ is word motion
            // in most terminals), Alt+1…9 for one in particular.
            "bind -n M-S-Left previous-window",
            "bind -n M-S-Right next-window",
        ]
        .map(String::from)
        .to_vec();
        lines.push(format!("set -g status-right '{VERSION} '"));
        lines.extend((1..=9).map(|n| format!("bind -n M-{n} select-window -t :={n}")));

        if self.settings.user_config.unwrap_or(true) {
            for file in [home().join(".tmux.conf"), home().join(".config/tmux/tmux.conf")] {
                if file.is_file() {
                    lines.push(format!("source-file -q {}", quote(&file.to_string_lossy())));
                }
            }
        }

        // What Claude Code needs, after the user's configuration so that it holds: Shift+Enter, notifications
        // and progress reaching the outer terminal, colours, the clipboard, member names that stay put.
        let mouse = if self.settings.mouse.unwrap_or(true) { "on" } else { "off" };
        lines.extend([
            format!("set -g mouse {mouse}"),
            "set -g allow-passthrough on".into(),
            "set -s extended-keys on".into(),
            "set -as terminal-features 'xterm*:extkeys'".into(),
            "set -as terminal-features 'xterm*:RGB'".into(),
            "set -g set-clipboard on".into(),
            format!("set -g default-terminal {}", default_terminal()),
            "setw -g allow-rename off".into(),
            "setw -g automatic-rename off".into(),
        ]);
        lines.extend(self.settings.options.iter().cloned());
        lines.join("\n") + "\n"
    }

    /// Creates the pane of `pane` in a new session, window or split; returns its id (`%12`).
    fn open(&self, how: &[&str], dir: &Path, pane: &Pane) -> Result<String> {
        let mut args: Vec<String> = how.iter().map(|s| s.to_string()).collect();
        args.extend(["-c".into(), dir.to_string_lossy().into_owned(), "-P".into(), "-F".into(), "#{pane_id}".into()]);
        for (key, value) in &pane.env {
            args.extend(["-e".into(), format!("{key}={value}")]);
        }
        args.extend(pane.argv.iter().cloned());
        let id = self.run(&args)?;
        self.run(["set-option", "-p", "-t", &id, "@recruit_member", &pane.member])?;
        if let Some(role) = pane.role {
            self.run(["set-option", "-p", "-t", &id, "@recruit_role", role])?;
        }
        Ok(id)
    }

    /// The panes of a session, tab by tab.
    pub fn panes(&self, session: &str) -> Result<Vec<PaneState>> {
        let format = "#{pane_id}\t#{window_id}\t#{@recruit_role}\t#{@recruit_member}";
        let list = self.run(["list-panes", "-s", "-t", &exact(session), "-F", format])?;
        Ok(list
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(4, '\t');
                Some(PaneState {
                    id: fields.next()?.to_string(),
                    window: fields.next().unwrap_or_default().to_string(),
                    role: fields.next().unwrap_or_default().to_string(),
                    member: fields.next().unwrap_or_default().to_string(),
                })
            })
            .collect())
    }

    /// The windows of a session, in order.
    fn windows(&self, session: &str) -> Result<Vec<WindowState>> {
        let format = "#{window_id}\t#{window_width}\t#{window_height}\t#{window_name}";
        let list = self.run(["list-windows", "-t", &exact(session), "-F", format])?;
        Ok(list
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(4, '\t');
                Some(WindowState {
                    id: fields.next()?.to_string(),
                    width: fields.next()?.parse().ok()?,
                    height: fields.next()?.parse().ok()?,
                    title: fields.next().unwrap_or_default().to_string(),
                })
            })
            .collect())
    }

    /// The panes of a window, in tmux's order.
    fn window_panes(&self, window: &str) -> Result<Vec<String>> {
        Ok(self.run(["list-panes", "-t", window, "-F", "#{pane_id}"])?.lines().map(String::from).collect())
    }

    /// The terminals of the clients attached to a session, one per line: their `TERM`, a tab, then the terminal's
    /// name and version when it told tmux (`ghostty 1.3.1`). Empty when tmux cannot tell.
    pub fn client_terminals(&self, session: &str) -> String {
        self.run(["list-clients", "-t", &exact(session), "-F", "#{client_termname}\t#{client_termtype}"])
            .unwrap_or_default()
    }

    /// The last click on a panel, left by the click binding in the pane's options, and the pane's width.
    pub fn click(&self, pane: &str) -> Result<Option<Click>> {
        let format = format!(
            "#{{@recruit_role}}\t#{{pane_width}}\t#{{{CLICK_X}}}\t#{{{CLICK_Y}}}\t#{{{CLICK_CLIENT}}}\t#{{{CLICK_LINE}}}"
        );
        Ok(parse_click(&self.run(["display-message", "-p", "-t", pane, &format])?))
    }

    /// Asks the user in a menu at the centre of `client`, over `pane`: `action` or `cancel`, `note` in grey between
    /// them. Waits for the answer, a menu opened from outside a client returning once it closes: true when the action
    /// was chosen.
    /// A menu already open on the client (another click's) stays alone: tmux returns at once without showing this
    /// one, which then counts as cancelled. Each menu answers with its own token, so that one never takes another's.
    pub fn confirm(&self, client: &str, pane: &str, action: &str, note: &str, cancel: &str) -> Result<bool> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let token = format!("{}-{now}", std::process::id());
        self.run(confirm_menu(client, pane, &token, action, note, cancel))?;
        Ok(self.run(["display-message", "-p", "-t", pane, &format!("#{{{CONFIRMED}}}")])? == token)
    }

    /// Gives the focus to a member's pane, in its tab. False when the session has no pane for it.
    pub fn focus(&self, session: &str, member: &str) -> Result<bool> {
        let panes = self.panes(session)?;
        let Some(pane) = panes.iter().find(|p| p.role.is_empty() && p.member == member) else { return Ok(false) };
        self.run(["select-window", "-t", &pane.id])?;
        self.run(["select-pane", "-t", &pane.id])?;
        Ok(true)
    }

    /// Takes the journal to its next size: full, reduced, hidden, full again. Returns its new size.
    pub fn toggle_journal(&self, session: &str, dir: &Path, journal: &Pane) -> Result<JournalSize> {
        let panes = self.panes(session)?;
        let open = panes.iter().find(|p| p.role == JOURNAL);
        let size = match open {
            None => JournalSize::Hidden,
            Some(pane) if self.reduced(&pane.id) => JournalSize::Reduced,
            Some(_) => JournalSize::Full,
        };
        let next = size.next();
        match (next, open) {
            (JournalSize::Reduced, Some(pane)) => self.reduce_journal(&pane.id)?,
            (JournalSize::Hidden, Some(pane)) => {
                let _ = self.run(["set-hook", "-uw", "-t", &pane.id, "window-resized"]);
                self.run(["kill-pane", "-t", &pane.id])?;
            }
            _ => {
                let dashboard = panes.iter().find(|p| p.role == DASHBOARD).context(t!(
                    "le tableau de bord est fermé : relancer recruit le rouvre",
                    "the dashboard is closed: running recruit again opens it"
                ))?;
                self.open(&["split-window", "-v", "-d", "-t", &dashboard.id, "-l", "50%"], dir, journal)?;
            }
        }
        Ok(next)
    }

    /// True when the journal in pane `id` is reduced.
    fn reduced(&self, id: &str) -> bool {
        self.run(["display-message", "-p", "-t", id, &format!("#{{{REDUCED}}}")]).is_ok_and(|v| v == "1")
    }

    /// Gives the journal in pane `id` its reduced height, and keeps it: tmux shares a window's new size out among its
    /// panes, the journal gets its rows back each time. Without window hooks (an older tmux), it only holds until then.
    fn reduce_journal(&self, id: &str) -> Result<()> {
        let rows = REDUCED_ROWS.to_string();
        self.run(["resize-pane", "-t", id, "-y", &rows])?;
        self.run(["set-option", "-p", "-t", id, REDUCED, "1"])?;
        let _ = self.run(["set-hook", "-w", "-t", id, "window-resized", &format!("resize-pane -t {id} -y {rows}")]);
        Ok(())
    }

    /// Opens a member's pane in a window of its own, at the end; returns its id.
    pub fn open_window(&self, session: &str, dir: &Path, pane: &Pane) -> Result<String> {
        let window = format!("{}:", exact(session));
        self.open(&["new-window", "-d", "-t", &window, "-n", &pane.member], dir, pane)
    }

    /// Closes a pane and what runs in it.
    pub fn kill_pane(&self, id: &str) -> Result<()> {
        self.run(["kill-pane", "-t", id]).map(drop)
    }

    /// The name a pane shows in its border, and goes by.
    pub fn set_member(&self, id: &str, member: &str) -> Result<()> {
        self.run(["set-option", "-p", "-t", id, "@recruit_member", member]).map(drop)
    }

    /// Opens the panels' column on the right of pane `beside`: the dashboard over the reduced journal.
    pub fn open_panels(&self, dir: &Path, beside: &str, dashboard: &Pane, journal: &Pane) -> Result<()> {
        let dashboard = self.open(&["split-window", "-h", "-d", "-t", beside, "-l", SIDE_WIDTH], dir, dashboard)?;
        let journal = self.open(&["split-window", "-v", "-d", "-t", &dashboard, "-l", "50%"], dir, journal)?;
        self.reduce_journal(&journal)
    }

    /// Closes the dashboard and the journal.
    pub fn close_panels(&self, session: &str) -> Result<()> {
        for pane in self.panes(session)?.iter().filter(|p| p.role == DASHBOARD || p.role == JOURNAL) {
            if pane.role == JOURNAL {
                let _ = self.run(["set-hook", "-uw", "-t", &pane.id, "window-resized"]);
            }
            self.kill_pane(&pane.id)?;
        }
        Ok(())
    }

    /// Takes the members' panes to `tabs` without stopping them (`layout::moves`), puts the windows in their order
    /// with their titles, then gives each tab its grid as `launch` builds it, the panels beside the first.
    pub fn arrange(&self, session: &str, tabs: &[Tab], columns: usize) -> Result<()> {
        let panes = self.panes(session)?;
        let windows = self.windows(session)?;
        let current: Vec<layout::Window> = windows
            .iter()
            .map(|w| layout::Window {
                id: w.id.clone(),
                title: w.title.clone(),
                members: panes
                    .iter()
                    .filter(|p| p.window == w.id && p.role.is_empty())
                    .map(|p| p.member.clone())
                    .collect(),
                panels: panes.iter().any(|p| p.window == w.id && !p.role.is_empty()),
            })
            .collect();
        let pane_of = |member: &str| {
            panes.iter().find(|p| p.role.is_empty() && p.member == member).map(|p| p.id.clone()).with_context(|| {
                t!("le panneau de « {} » est introuvable", "the pane of \"{}\" cannot be found", member)
            })
        };
        let moves = layout::moves(&current, tabs);
        let mut hosts = Vec::new();
        for (host, tab) in moves.hosts.iter().zip(tabs) {
            hosts.push(match host {
                Host::Window(id) => id.clone(),
                Host::Break(member) => {
                    let pane = pane_of(member)?;
                    self.run(["break-pane", "-d", "-s", &pane, "-n", &tab.title, "-P", "-F", "#{window_id}"])?
                }
            });
        }
        for (member, tab) in &moves.joins {
            self.run(["join-pane", "-d", "-s", &pane_of(member)?, "-t", &hosts[*tab]])?;
            // Shared out again at once: each join halves the pane it lands on.
            self.run(["select-layout", "-t", &hosts[*tab], "tiled"])?;
        }
        for (host, tab) in hosts.iter().zip(tabs) {
            self.run(["rename-window", "-t", host, &tab.title])?;
        }
        // The tabs first, in their order, then any other window.
        let mut order: Vec<String> = self.windows(session)?.into_iter().map(|w| w.id).collect();
        let wanted: Vec<String> =
            hosts.iter().cloned().chain(order.iter().filter(|w| !hosts.contains(w)).cloned()).collect();
        for k in 0..order.len() {
            if order[k] != wanted[k] {
                let at = order.iter().position(|w| *w == wanted[k]).expect("the same windows");
                self.run(["swap-window", "-d", "-s", &wanted[k], "-t", &order[k]])?;
                order.swap(k, at);
            }
        }
        let windows = self.windows(session)?;
        let panes = self.panes(session)?;
        for (i, (host, tab)) in hosts.iter().zip(tabs).enumerate() {
            let members = tab.members.iter().map(|m| pane_of(m)).collect::<Result<Vec<_>>>()?;
            let panel = |role: &str| panes.iter().find(|p| p.window == *host && p.role == role).map(|p| p.id.clone());
            let side = (i == 0)
                .then(|| SidePanes {
                    dashboard: panel(DASHBOARD),
                    journal: panel(JOURNAL).map(|id| {
                        let rows = self.reduced(&id).then_some(REDUCED_ROWS);
                        (id, rows)
                    }),
                })
                .filter(|s| s.dashboard.is_some() || s.journal.is_some());
            let Some(window) = windows.iter().find(|w| w.id == *host) else { continue };
            let (layout, cells) = layout::tmux_layout(window.width, window.height, &members, columns, side.as_ref());
            let mut list = self.window_panes(host)?;
            let mut sorted_list = list.clone();
            sorted_list.sort();
            let mut sorted_cells = cells.clone();
            sorted_cells.sort();
            if sorted_list != sorted_cells {
                // A pane that is not the team's: tmux shares the window out itself.
                self.run(["select-layout", "-t", host, "tiled"])?;
                continue;
            }
            // tmux gives its cells to the window's panes in their order.
            for k in 0..cells.len() {
                if list[k] != cells[k] {
                    let at = list.iter().position(|p| *p == cells[k]).expect("the same panes");
                    self.run(["swap-pane", "-d", "-s", &cells[k], "-t", &list[k]])?;
                    list.swap(k, at);
                }
            }
            self.run(["select-layout", "-t", host, &layout])?;
        }
        Ok(())
    }

    /// Detaches a client from its session.
    pub fn detach(&self, client: &str) -> Result<()> {
        self.run(["detach-client", "-t", client]).map(drop)
    }

    /// The client that shows a member's pane, else one that shows its tab, else any client of the session; and the
    /// pane. None when no client shows the session.
    pub fn client_of(&self, session: &str, member: &str) -> Result<Option<(String, String)>> {
        let panes = self.panes(session)?;
        let Some(pane) = panes.iter().find(|p| p.role.is_empty() && p.member == member) else { return Ok(None) };
        let format = "#{client_name}\t#{pane_id}\t#{window_id}";
        let clients: Vec<(String, String, String)> = self
            .run(["list-clients", "-t", &exact(session), "-F", format])
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, '\t');
                Some((fields.next()?.to_string(), fields.next()?.to_string(), fields.next()?.to_string()))
            })
            .collect();
        let client = clients
            .iter()
            .find(|c| c.1 == pane.id)
            .or_else(|| clients.iter().find(|c| c.2 == pane.window))
            .or(clients.first());
        Ok(client.map(|(client, _, _)| (client.clone(), pane.id.clone())))
    }

    /// A client's size, and its terminal as `client_terminals` gives it (`TERM`, a tab, the terminal's name and
    /// version), in one question.
    pub fn client_look(&self, client: &str) -> Result<(usize, usize, String)> {
        let format = "#{client_width}\t#{client_height}\t#{client_termname}\t#{client_termtype}";
        let out = self.run(["display-message", "-p", "-c", client, format])?;
        let mut parts = out.splitn(3, '\t');
        let number = |p: Option<&str>| p.and_then(|p| p.trim().parse().ok()).unwrap_or(0);
        let (width, height) = (number(parts.next()), number(parts.next()));
        Ok((width, height, parts.next().unwrap_or_default().to_string()))
    }

    /// Opens the team's menu in a popup on `client`, `size` cells (columns, rows), running `command`, and returns
    /// without waiting for it to close.
    pub fn popup(&self, client: &str, size: &(String, String), command: &str) -> Result<()> {
        let args = popup(command, size);
        // A popup opened from outside a client waits for it to close: not the caller.
        self.command()
            .args(&args[..1])
            .args(["-c", client])
            .args(&args[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .context("tmux display-popup")?;
        Ok(())
    }

    /// Starts a pane's command again, in place.
    pub fn respawn(&self, id: &str, dir: &Path, pane: &Pane) -> Result<()> {
        let mut args: Vec<String> = ["respawn-pane", "-k", "-t", id, "-c"].map(String::from).to_vec();
        args.push(dir.to_string_lossy().into_owned());
        for (key, value) in &pane.env {
            args.extend(["-e".into(), format!("{key}={value}")]);
        }
        args.extend(pane.argv.iter().cloned());
        self.run(&args).map(drop)
    }

    /// Opens the dashboard again: above the journal, which keeps its size, else on the right of the first member.
    pub fn restore_dashboard(&self, session: &str, dir: &Path, dashboard: &Pane) -> Result<()> {
        let panes = self.panes(session)?;
        if let Some(journal) = panes.iter().find(|p| p.role == JOURNAL) {
            self.open(&["split-window", "-v", "-b", "-d", "-t", &journal.id, "-l", "50%"], dir, dashboard)?;
            if self.reduced(&journal.id) {
                self.reduce_journal(&journal.id)?;
            }
        } else if let Some(first) = panes.iter().find(|p| p.role.is_empty()) {
            self.open(&["split-window", "-h", "-d", "-t", &first.id, "-l", SIDE_WIDTH], dir, dashboard)?;
        }
        Ok(())
    }

    /// Opens the session, its tabs and their panes.
    fn build(&self, plan: &Plan) -> Result<()> {
        let conf = cache_dir().join(format!("tmux-{}.conf", self.socket));
        fs::create_dir_all(cache_dir())?;
        fs::write(&conf, self.config())?;
        let conf = conf.to_string_lossy().into_owned();
        let (width, height) = crossterm::terminal::size().unwrap_or((200, 50));
        let (width, height) = (width.to_string(), height.saturating_sub(1).max(10).to_string());
        let session = exact(&plan.session);
        let window = format!("{session}:");

        let mut first = None;
        for tab in &plan.tabs {
            let Some(head) = tab.panes.first() else { continue };
            let top = if first.is_none() {
                // -f is only read by a server that is not running yet.
                let how = [
                    "-f",
                    &conf,
                    "new-session",
                    "-d",
                    "-s",
                    &plan.session,
                    "-n",
                    &tab.title,
                    "-x",
                    &width,
                    "-y",
                    &height,
                ];
                let id = self.open(&how, &plan.dir, head)?;
                self.run(["set-option", "-t", &window, "@recruit_team", &plan.team])?;
                self.run(["set-option", "-t", &window, "@recruit_dir", &plan.dir.to_string_lossy()])?;
                self.run(["set-option", "-t", &window, "@recruit_state", &plan.state.to_string_lossy()])?;
                self.run(["set-option", "-t", &window, "@recruit_lang", plan.lang.code()])?;
                first = Some(id.clone());
                id
            } else {
                self.open(&["new-window", "-d", "-t", &window, "-n", &tab.title], &plan.dir, head)?
            };

            if let Some(side) = &tab.side {
                // The panels' column first, so that it takes the whole height.
                let how = ["split-window", "-h", "-d", "-t", &top, "-l", SIDE_WIDTH];
                let dashboard = self.open(&how, &plan.dir, &side.dashboard)?;
                let how = ["split-window", "-v", "-d", "-t", &dashboard, "-l", "50%"];
                let journal = self.open(&how, &plan.dir, &side.journal)?;
                self.reduce_journal(&journal)?;
            }

            // Columns first, each one as wide as the others, then the rows of each column.
            let columns = if tab.side.is_some() { 1 } else { plan.columns };
            let heights = column_heights(tab.panes.len(), columns);
            let cols = heights.len();
            let mut tops = vec![top];
            for c in 1..cols {
                let size = format!("{}%", 100 * (cols - c) / (cols - c + 1));
                let target = tops[c - 1].clone();
                tops.push(self.open(&["split-window", "-h", "-t", &target, "-l", &size], &plan.dir, &tab.panes[c])?);
            }
            for (c, &h) in heights.iter().enumerate() {
                let mut target = tops[c].clone();
                for r in 1..h {
                    let size = format!("{}%", 100 * (h - r) / (h - r + 1));
                    let pane = &tab.panes[r * cols + c];
                    target = self.open(&["split-window", "-v", "-t", &target, "-l", &size], &plan.dir, pane)?;
                }
            }
        }

        if let Some(first) = first {
            self.run(["select-window", "-t", &first])?;
            self.run(["select-pane", "-t", &first])?;
        }
        // Bound at each launch rather than in the configuration: the path of recruit may have changed since the
        // server started. Same for a click on a panel, which goes to the member clicked.
        self.run(["bind-key", "-n", "M-j", "run-shell", "-b", &plan.toggle])?;
        self.run(click_binding(&plan.click))?;

        // The team's menu: Alt+r, or its button in the status line. Leaving in one go: Alt+q, or the button on the
        // right, opens the quit menu. A click elsewhere on the status line does what it does by default.
        let open = ["run-shell".to_string(), "-b".into(), plan.menu.clone()];
        self.run(["bind-key", "-n", "M-r"].into_iter().map(String::from).chain(open.iter().cloned()))?;
        let menu = quit_menu(plan.lang);
        self.run(["bind-key", "-n", "M-q"].into_iter().map(String::from).chain(menu.iter().cloned()))?;
        self.run(status_click(&open, &menu))?;
        // The right of the status line, and its length: a shorter one in the user's configuration would cut the
        // button off.
        let status = status_right(plan.lang);
        self.run(["set-option", "-t", &window, "status-right", &status])?;
        self.run(["set-option", "-t", &window, "status-right-length", &status_width(&status).to_string()])?;
        Ok(())
    }
}

impl Backend for Tmux {
    fn running(&self) -> Result<Vec<RunningTeam>> {
        let output = self
            .command()
            .args(["list-sessions", "-F", "#{session_name}\t#{@recruit_team}\t#{@recruit_dir}\t#{session_attached}"])
            .output()
            .context("tmux")?;
        if !output.status.success() {
            // No server running: no team either.
            return Ok(Vec::new());
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let session = fields.next()?.to_string();
                let team = fields.next().filter(|t| !t.is_empty()).unwrap_or(&session).to_string();
                let dir = fields.next().unwrap_or_default().to_string();
                let attached = fields.next().is_some_and(|a| a != "0");
                Some(RunningTeam { session, team, dir, attached })
            })
            .collect())
    }

    fn launch(&self, plan: &Plan) -> Result<()> {
        let result = self.build(plan);
        if result.is_err() {
            // No half-built team left behind.
            let _ = self.stop(&plan.session);
        }
        result
    }

    fn attach(&self, session: &str) -> Result<()> {
        if let Ok(outer) = std::env::var("TMUX") {
            let socket = outer.split(',').next().map(Path::new).and_then(Path::file_name);
            if socket.is_some_and(|s| s == self.socket.as_str()) {
                // Already inside recruit's server: switch this client over.
                let status = Command::new("tmux").args(["switch-client", "-t", &exact(session)]).status()?;
                if !status.success() {
                    bail!("tmux switch-client");
                }
                return Ok(());
            }
            eprintln!(
                "{}",
                t!(
                    "Note : tu es déjà dans tmux, l'équipe s'ouvre dans un tmux imbriqué.",
                    "Note: you are already inside tmux, the team opens in a nested tmux."
                )
            );
        }
        if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
            bail!(t!("pas de terminal pour s'attacher à l'équipe", "no terminal to attach the team to"));
        }
        let error = self.command().args(["attach-session", "-t", &exact(session)]).exec();
        Err(error).context("tmux attach-session")
    }

    fn stop(&self, session: &str) -> Result<()> {
        self.run(["kill-session", "-t", &exact(session)]).map(drop)
    }
}

/// The status line's ranges that open the team's menu and the quit menu.
const MENU_RANGE: &str = "recruit-menu";
const QUIT_RANGE: &str = "recruit-quit";

/// `display-popup` arguments for the team's menu, `size` cells (columns, rows): closed when the menu ends.
fn popup(command: &str, size: &(String, String)) -> Vec<String> {
    ["display-popup", "-E", "-w", &size.0, "-h", &size.1, "-T", " recruit ", command].map(String::from).to_vec()
}

/// The menu's popup on a client `width` × `height`: 100 × 28 inside its borders, as the mockups draw it, less a
/// border's width around it in a smaller client, so that a client of 54 columns still gets the 50 the menu needs; never
/// larger than the client, which tmux refuses without a word. A size unknown: 80 % of it.
pub fn popup_size(width: usize, height: usize) -> (String, String) {
    if width == 0 || height == 0 {
        return ("80%".into(), "80%".into());
    }
    let fit = |wanted: usize, room: usize| wanted.min(room.saturating_sub(2)).max(1).min(room);
    (fit(102, width).to_string(), fit(30, height).to_string())
}

/// `bind-key` arguments for a click on the status line: on the menu's button what opens the team's menu, on the quit
/// button the quit menu, elsewhere what tmux does by default.
fn status_click(menu: &[String], quit: &[String]) -> Vec<String> {
    let command = |words: &[String]| words.iter().map(|w| tmux_quote(w)).collect::<Vec<_>>().join(" ");
    let on = |range: &str| format!("#{{==:#{{mouse_status_range}},{range}}}");
    let otherwise = format!(
        "if-shell -F {} {} {}",
        tmux_quote(&on(QUIT_RANGE)),
        tmux_quote(&command(quit)),
        tmux_quote("switch-client -t =")
    );
    ["bind-key", "-n", "MouseDown1Status", "if-shell", "-F", &on(MENU_RANGE), &command(menu), &otherwise]
        .map(String::from)
        .to_vec()
}

/// recruit and its version, dimmed, on the right of the status line.
const VERSION: &str = concat!("#[dim]recruit ", env!("CARGO_PKG_VERSION"), "#[nodim]");

/// The right of the status line: recruit's version, then the buttons that open the team's menu and the quit menu,
/// each alone in its range.
fn status_right(lang: Lang) -> String {
    let quit = match lang {
        Lang::Fr => format!("quitter ({ALT}q)"),
        Lang::En => format!("quit ({ALT}q)"),
    };
    let menu = format!("menu ({ALT}r)");
    let button = |range: &str, text: &str| format!("#[range=user|{range}]#[reverse] {text} #[noreverse]#[norange]");
    format!("{VERSION}  {} {} ", button(MENU_RANGE, &menu), button(QUIT_RANGE, &quit))
}

/// The quit menu, as `display-menu` arguments: detach, or stop the team. `-O` keeps it open when the click that
/// opened it from the status line is released: without it, a plain click shows the menu and closes it at once.
/// `-M` lets the mouse choose an item when Alt+q opened it: a menu opened from a key ignores the mouse, a click on
/// an item would close it unchosen.
fn quit_menu(lang: Lang) -> Vec<String> {
    let (detach, stop, stop_key, cancel) = match lang {
        Lang::Fr => ("Détacher : l'équipe continue", "Arrêter l'équipe", "a", "Annuler"),
        Lang::En => ("Detach: the team keeps running", "Stop the team", "s", "Cancel"),
    };
    let title = "#[align=centre] recruit ";
    ["display-menu", "-M", "-O", "-T", title, "-x", "C", "-y", "C"]
        .into_iter()
        .chain([detach, "d", "detach-client", stop, stop_key, "kill-session"])
        .chain(["", cancel, "q", ""])
        .map(String::from)
        .collect()
}

/// Where the click binding leaves a click, in the pane's options.
const CLICK_X: &str = "@recruit_click_x";
const CLICK_Y: &str = "@recruit_click_y";
const CLICK_LINE: &str = "@recruit_click_line";
const CLICK_CLIENT: &str = "@recruit_click_client";

/// `bind-key` arguments for a click on a panel, run when the button is released: a drag that selects text ends
/// otherwise (`MouseDragEnd1Pane`) and does not run it. The click is left in the pane's options, then `command`
/// runs with nothing but the team's folder and the pane: `#{mouse_line}` holds what the members wrote and never
/// goes through a shell. The pane gets the click all the same, the panels' as every other.
fn click_binding(command: &str) -> Vec<String> {
    let panel = format!("#{{||:#{{==:#{{@recruit_role}},{DASHBOARD}}},#{{==:#{{@recruit_role}},{JOURNAL}}}}}");
    let mut then = vec!["send-keys -M".to_string()];
    let click = [
        (CLICK_X, "#{mouse_x}"),
        (CLICK_Y, "#{mouse_y}"),
        (CLICK_CLIENT, "#{client_name}"),
        (CLICK_LINE, "#{mouse_line}"),
    ];
    for (option, value) in click {
        then.push(format!("set-option -p -t = -F {option} {}", tmux_quote(value)));
    }
    then.push(format!("run-shell -b -t = {}", tmux_quote(command)));
    let then = then.join(" ; ");
    ["bind-key", "-n", "MouseUp1Pane", "if-shell", "-F", "-t", "=", &panel, &then, "send-keys -M"]
        .map(String::from)
        .to_vec()
}

/// A click as `Tmux::click` reads it: the panel's kind, the pane's width, the column, the row, the client and the
/// line, separated by tabs.
fn parse_click(text: &str) -> Option<Click> {
    let mut fields = text.splitn(6, '\t');
    let role = fields.next()?.to_string();
    let columns = fields.next()?.parse().ok()?;
    let x = fields.next()?.parse().ok()?;
    let y = fields.next()?.parse().ok()?;
    // tmux's output is trimmed: an empty row leaves no tab after the client, nor an unknown client after the row.
    let client = fields.next().unwrap_or_default().to_string();
    let line = fields.next().unwrap_or_default().to_string();
    Some(Click { role, columns, x, y, client, line })
}

/// Where the confirmation menu leaves its answer, in the pane's options.
const CONFIRMED: &str = "@recruit_confirmed";

/// `display-menu` arguments for a confirmation at the centre of `client`: `action` (key c), a `note` in grey that
/// cannot be chosen, then `cancel` (key q). Choosing the action only sets an option on `pane` to `token` (digits and
/// a dash): the caller reads it once the menu closes, nothing else it was given goes through a command. The menu
/// reads its texts as formats.
///
/// Opened from outside a mouse binding, a menu ignores the mouse unless told (`-M`): a click on an item would close
/// it unchosen. With the mouse, tmux reports every motion, and one outside the menu, where the pointer starts, would
/// close it too: `-O` keeps it open until a click, on an item or outside.
fn confirm_menu(client: &str, pane: &str, token: &str, action: &str, note: &str, cancel: &str) -> Vec<String> {
    let text = |t: &str| t.replace('#', "##");
    let chosen = format!("set-option -p -t {} {CONFIRMED} {}", tmux_quote(pane), tmux_quote(token));
    let head = ["display-menu", "-M", "-O", "-c", client, "-t", pane, "-T", "#[align=centre] recruit "];
    let head = head.into_iter().chain(["-x", "C", "-y", "C"]);
    let mut args: Vec<String> = head.map(String::from).collect();
    // An item whose name starts with `-` is shown dim and cannot be chosen.
    args.extend([text(action), "c".into(), chosen, format!("-{}", text(note)), String::new(), String::new()]);
    args.extend([String::new(), text(cancel), "q".into(), String::new()]);
    args
}

/// The columns a status line format takes: its text without the `#[…]` styles.
fn status_width(format: &str) -> usize {
    let mut width = 0;
    let mut rest = format;
    while let Some(start) = rest.find("#[") {
        width += rest[..start].chars().count();
        rest = rest[start..].split_once(']').map_or("", |(_, after)| after);
    }
    width + rest.chars().count()
}

/// A word of a tmux command line: as it is when plain, double-quoted otherwise.
fn tmux_quote(word: &str) -> String {
    let plain = !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "-_=./:".contains(c));
    if plain {
        return word.to_string();
    }
    let escaped = word.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$");
    format!("\"{escaped}\"")
}

/// `=name` makes tmux match the session name exactly instead of by prefix.
fn exact(session: &str) -> String {
    format!("={session}")
}

/// tmux refuses `.` and `:` in session names.
pub fn session_name(team: &str) -> String {
    team.replace(['.', ':'], "_")
}

fn parse_version(text: &str) -> Option<(u32, u32)> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let mut parts = text[start..].split(|c: char| !c.is_ascii_digit());
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// `tmux-256color` where the system knows it (most Linux), `screen-256color` otherwise (older macOS ncurses).
fn default_terminal() -> &'static str {
    let known = Command::new("infocmp")
        .arg("tmux-256color")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if known { "tmux-256color" } else { "screen-256color" }
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_shortcuts() {
        let config = Tmux { socket: "t".into(), settings: TmuxSettings::default() }.config();
        assert!(config.contains("bind -n M-S-Left previous-window\n"));
        assert!(config.contains("bind -n M-9 select-window -t :=9\n"));
    }

    #[test]
    fn quit_menu_words() {
        let menu = quit_menu(Lang::Fr);
        // The mouse handled, whatever opened it, and the menu kept open by the click's release or a motion.
        assert_eq!(menu[..3], ["display-menu", "-M", "-O"]);
        assert!(menu.contains(&"kill-session".to_string()) && menu.contains(&"detach-client".to_string()));
        assert_eq!(tmux_quote("detach-client"), "detach-client");
        assert_eq!(tmux_quote(""), "\"\"");
        assert_eq!(tmux_quote("Arrêter l'équipe"), "\"Arrêter l'équipe\"");
        assert_eq!(tmux_quote(r#"a "b" $c"#), r#""a \"b\" \$c""#);
    }

    #[test]
    fn status_line() {
        let version = format!("recruit {}", env!("CARGO_PKG_VERSION"));
        let config = Tmux { socket: "t".into(), settings: TmuxSettings::default() }.config();
        assert!(config.contains(&format!("set -g status-right '#[dim]{version}#[nodim] '\n")));

        let status = status_right(Lang::Fr);
        assert!(status.starts_with(&format!("#[dim]{version}#[nodim]  #[range=")));
        // Each button's range holds the button and nothing else, the menu's on the left of the quit one.
        let (_, range) = status.split_once(&format!("#[range=user|{MENU_RANGE}]")).unwrap();
        let (range, after) = range.split_once("#[norange]").unwrap();
        assert_eq!(range, format!("#[reverse] menu ({ALT}r) #[noreverse]"));
        assert_eq!(after, format!(" #[range=user|{QUIT_RANGE}]#[reverse] quitter ({ALT}q) #[noreverse]#[norange] "));
        assert!(status_right(Lang::En).contains(&format!("#[reverse] quit ({ALT}q) #[noreverse]#[norange]")));
        let buttons = format!(" menu ({ALT}r)   quitter ({ALT}q) ");
        assert_eq!(status_width(&status), version.len() + 2 + buttons.chars().count() + 1);
        assert_eq!(SIDE_WIDTH, format!("{}%", layout::SIDE_PERCENT));
        assert_eq!(status_width("#[reverse] ⌥q #[noreverse]"), 4);
        assert_eq!(status_width("#[bold]é#[default]"), 1);
    }

    #[test]
    fn menu_in_a_popup() {
        let command = "/opt/recruit --lang fr _menu /s --client /dev/ttys004";
        let popup = popup(command, &("102".into(), "30".into()));
        assert_eq!(popup[..6], ["display-popup", "-E", "-w", "102", "-h", "30"]);
        assert_eq!(popup.last().unwrap(), command);
        let size = |w: usize, h: usize| {
            let (w, h) = popup_size(w, h);
            (w.parse::<usize>().unwrap(), h.parse::<usize>().unwrap())
        };
        assert_eq!(size(200, 60), (102, 30));
        assert_eq!(size(103, 31), (101, 29), "a border's width less");
        assert_eq!(size(54, 18), (52, 16), "the menu's 50 × 14 inside");
        assert_eq!(size(15, 10), (13, 8), "never larger than the client");
        assert_eq!(size(1, 1), (1, 1));
        assert_eq!(popup_size(0, 0), ("80%".into(), "80%".into()), "a size unknown");
        let open =
            "'/opt/re cruit' --lang '#{@recruit_lang}' _menu '#{@recruit_state}' --client '#{client_name}' --popup";
        let binding = status_click(&["run-shell".into(), "-b".into(), open.into()], &quit_menu(Lang::En));
        assert_eq!(
            binding[..6],
            ["bind-key", "-n", "MouseDown1Status", "if-shell", "-F", "#{==:#{mouse_status_range},recruit-menu}"]
        );
        assert_eq!(binding[6], format!("run-shell -b {}", tmux_quote(open)));
        assert!(
            binding[7].starts_with(r##"if-shell -F "#{==:#{mouse_status_range},recruit-quit}" "display-menu -M -O"##)
        );
        assert!(binding[7].ends_with(r##" "switch-client -t =""##), "{}", binding[7]);
    }

    #[test]
    fn journal_sizes_in_turn() {
        let mut size = JournalSize::Reduced;
        let turn: Vec<JournalSize> = (0..4)
            .map(|_| {
                size = size.next();
                size
            })
            .collect();
        assert_eq!(turn, [JournalSize::Hidden, JournalSize::Full, JournalSize::Reduced, JournalSize::Hidden]);
        // Three messages of two lines, the journal writing none between them, and the line after the last one.
        assert_eq!(REDUCED_ROWS, 7);
        let said = [JournalSize::Full, JournalSize::Reduced, JournalSize::Hidden].map(JournalSize::said);
        assert!(said.iter().all(|s| s.starts_with("Journal")) && said[0] != said[1] && said[1] != said[2]);
    }

    #[test]
    fn click_on_panels_only() {
        let command = "'/opt/re cruit' _click '#{@recruit_state}' '#{pane_id}' >/dev/null 2>&1";
        let binding = click_binding(command);
        assert_eq!(binding[..7], ["bind-key", "-n", "MouseUp1Pane", "if-shell", "-F", "-t", "="]);
        let [condition, then, otherwise] = &binding[7..] else { panic!("{binding:?}") };
        assert_eq!(condition, "#{||:#{==:#{@recruit_role},dashboard},#{==:#{@recruit_role},journal}}");
        // Elsewhere, and on the panels too, the pane gets the click as without the binding.
        assert_eq!(otherwise, "send-keys -M");
        let then: Vec<&str> = then.split(" ; ").collect();
        assert_eq!(then[0], "send-keys -M");
        assert!(then.contains(&r##"set-option -p -t = -F @recruit_click_line "#{mouse_line}""##));
        assert!(then.contains(&r##"set-option -p -t = -F @recruit_click_client "#{client_name}""##));
        // The shell gets the team's folder and the pane, nothing the members wrote.
        let shell = then.last().unwrap();
        assert_eq!(*shell, format!("run-shell -b -t = {}", tmux_quote(command)));
        assert!(!shell.contains("mouse_"));
        assert_eq!(then.iter().filter(|c| c.contains("mouse_line")).count(), 1);
    }

    #[test]
    fn click_read_back_as_written() {
        let line = r#"  dev ──▶ "review" '$(touch x)' `id` #{pane_id} #[fg=red] ; run-shell 'rm' \"#;
        let click = parse_click(&format!("journal\t48\t12\t3\t/dev/ttys004\t{line}")).unwrap();
        let client = "/dev/ttys004".to_string();
        assert_eq!(click, Click { role: "journal".into(), columns: 48, x: 12, y: 3, client, line: line.into() });
        // A tab in the line belongs to the line.
        assert_eq!(parse_click("dashboard\t48\t0\t0\tc\ta\tb").unwrap().line, "a\tb");
        let click = parse_click("dashboard\t48\t4\t7\t/dev/ttys004").unwrap();
        assert_eq!((click.client.as_str(), click.line.as_str()), ("/dev/ttys004", ""));
        assert_eq!(parse_click("dashboard\t48\t4\t7").unwrap().client, "");
        // No click left yet.
        assert_eq!(parse_click("journal\t48\t\t\t\t"), None);
        assert_eq!(parse_click(""), None);
    }

    #[test]
    fn confirmation_menu() {
        let menu = confirm_menu(
            "/dev/ttys004",
            "%3",
            "812-1791390000",
            "Compacter dev-cli",
            "Relit tout #son contexte",
            "Annuler",
        );
        // The mouse handled, and the menu kept open by a motion outside it.
        let head = ["display-menu", "-M", "-O", "-c", "/dev/ttys004", "-t", "%3", "-T", "#[align=centre] recruit "];
        assert_eq!(menu[..9], head);
        assert_eq!(menu[9..13], ["-x", "C", "-y", "C"]);
        let items = &menu[13..];
        let chosen = r#"set-option -p -t "%3" @recruit_confirmed 812-1791390000"#;
        assert_eq!(items[..3], ["Compacter dev-cli", "c", chosen]);
        // The note, dim and not to be chosen: its `#` shown as is, not read as a format.
        assert_eq!(items[3..6], ["-Relit tout ##son contexte", "", ""]);
        // A line, then cancelling, which does nothing.
        assert_eq!(items[6..], ["", "Annuler", "q", ""]);
        // Choosing sets an option and runs nothing else: no text given goes into a command.
        assert!(menu.iter().filter(|a| a.contains("set-option")).all(|a| !a.contains("dev-cli")));
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("tmux 3.7c"), Some((3, 7)));
        assert_eq!(parse_version("tmux next-3.6"), Some((3, 6)));
        assert_eq!(parse_version("tmux 2.9a\n"), Some((2, 9)));
        assert_eq!(parse_version("tmux master"), None);
    }

    #[test]
    fn session_names() {
        assert_eq!(session_name("omni.dex:1"), "omni_dex_1");
    }
}
