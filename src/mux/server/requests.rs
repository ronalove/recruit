// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The commands of the team's other processes (`proto::Request`) and their answers.

use std::sync::atomic::Ordering;

use anyhow::Result;

use super::Server;
use super::lifecycle::log;
use super::status::look_state;
use super::tabs::Tab;
use crate::mux::engine::{Modes, MouseTracking};
use crate::mux::input::{self, Event};
use crate::mux::proto::{Bye, Captured, ClientInfo, PaneInfo, PaneModes, PaneSpec, PaneStats, Reply, Request, Stats};
use crate::t;
use crate::team::state::Snapshot;
use crate::ui::Rect;
use crate::ui::canvas::Canvas;
use crate::ui::look;

/// A result as a command's answer.
fn reply_of<T: serde::Serialize>(result: Result<T>) -> Reply {
    match result {
        Ok(value) => Reply::Ok(serde_json::to_value(value).unwrap_or_default()),
        Err(error) => Reply::Err(format!("{error:#}")),
    }
}

/// A canvas as text, a line per row, the blanks at the end cut.
fn captured(canvas: &Canvas, modes: Option<PaneModes>) -> Captured {
    let lines = (0..canvas.height()).map(|y| canvas.row(y)).collect();
    Captured { lines, history: Vec::new(), cursor: canvas.cursor().map(|c| (c.x, c.y)), modes }
}

impl Server {
    /// A command's answer. The members' order, which gives their colors, is read again after a request that may
    /// follow a change of the team (`team/launch.rs`, `team/live.rs`): here, never on the way of a frame.
    pub(super) fn request(&mut self, request: Request) -> Reply {
        // Not after `SetMember`: `team/live.rs` sends it before it writes team.json, and it renames in place.
        let changes = matches!(request, Request::Build { .. } | Request::OpenWindow { .. } | Request::Arrange { .. });
        let reply = self.dispatch(request);
        if changes {
            self.read_order();
        }
        reply
    }

    /// The members in the team's order (`team.json`), as the dashboard and the journal color them.
    fn read_order(&mut self) {
        match Snapshot::read(&self.state) {
            Ok(snapshot) => self.order = snapshot.members.into_iter().map(|member| member.name).collect(),
            Err(error) => log!("team.json: {error:#}"),
        }
        self.dirty = true;
    }

    /// What each request does. The ones that open, close or move panes are in `panes.rs`; the others are small.
    pub(super) fn dispatch(&mut self, request: Request) -> Reply {
        match request {
            // Never here: the loop takes `Stop` first (and `stop` answers the commands that come after it).
            Request::Stop => done(),
            Request::Panes => value(self.panes_info()),
            Request::KillPane { id } => match self.find(&id).map(|pane| pane.id.clone()) {
                Some(id) => {
                    self.close_pane(&id);
                    done()
                }
                None => unknown_pane(&id),
            },
            Request::SetMember { id, member } => self.set_member(&id, member),
            Request::Detach { client } => {
                if self.client.as_ref().is_some_and(|c| c.id == client) {
                    let _ = self.drop_client(Some(Bye::Detached));
                    done()
                } else {
                    Reply::Err(t!("client inconnu : {}", "unknown client: {}", client))
                }
            }
            Request::Clients => value(self.clients_info()),
            Request::Stats => value(self.stats()),
            Request::Capture { pane, history } => self.capture(pane, history),
            Request::Send { pane, bytes } => match self.find(&pane) {
                Some(pane) => {
                    pane.send(bytes);
                    done()
                }
                None => unknown_pane(&pane),
            },
            Request::Key { pane: None, event } => {
                self.input(event);
                done()
            }
            Request::Key { pane: Some(which), event } => match self.find(&which) {
                Some(pane) => {
                    pane.send(encoded(&event, &pane.modes()));
                    done()
                }
                None => unknown_pane(&which),
            },
            Request::Spawn { member, argv, env, cwd, tab } => {
                if self.panes.iter().any(|pane| pane.member == member) {
                    return Reply::Err(t!("le panneau « {} » existe déjà", "pane \"{}\" already exists", member));
                }
                let spec = PaneSpec { member, role: String::new(), argv, env, cwd };
                reply_of(self.open(spec, tab))
            }
            Request::Build { team, dir, columns, tabs, cols, rows, exe, lang, focus } => {
                self.focus_layout = focus;
                self.exe = Some(exe).filter(|exe| !exe.is_empty());
                if !lang.is_empty() {
                    self.lang = lang;
                }
                reply_of(self.build(team, dir, columns, tabs, (cols, rows)))
            }
            Request::OpenWindow { pane } => {
                let title = pane.member.clone();
                let opened = self.spawn(pane);
                if let Ok(id) = &opened {
                    self.tabs.push(Tab::new(title, vec![id.clone()]));
                    self.layout();
                }
                reply_of(opened)
            }
            Request::Respawn { id, pane } => reply_of(self.respawn(&id, pane)),
            Request::Arrange { tabs, columns, focus } => {
                self.focus_layout = focus;
                self.arrange(tabs, columns);
                done()
            }
            Request::OpenPanels { beside, dashboard, journal } => {
                reply_of(self.open_panels(&beside, dashboard, journal))
            }
            Request::ClosePanels => {
                self.close_panels();
                done()
            }
            Request::RestoreDashboard { dashboard } => reply_of(self.restore_dashboard(dashboard)),
            Request::ToggleJournal { journal } => reply_of(self.toggle_journal(journal)),
            // The member names the client (one only, policy A); with a field, the sheet too.
            Request::OpenMenu { member, client, field } => {
                let sheet = member.as_deref().zip(field).map(|(member, field)| (member, Some(field)));
                reply_of(self.open_menu(client.as_deref(), sheet))
            }
        }
    }

    /// The tabs' panes, then the menu's, floating, in no tab (`_ctl` finds it by its member, « recruit »); whoever
    /// looks for members goes by their role.
    fn panes_info(&self) -> Vec<PaneInfo> {
        let menu = self.menu.as_ref().map(|(menu, _)| (None, menu.clone()));
        self.tabs
            .iter()
            .flat_map(|tab| tab.ids().into_iter().map(move |id| (Some(tab), id)))
            .chain(menu)
            .filter_map(|(tab, id)| self.pane(&id).map(|pane| (tab, pane)))
            .map(|(tab, pane)| PaneInfo {
                id: pane.id.clone(),
                member: pane.member.clone(),
                role: pane.role.name().to_string(),
                tab: tab.map(|tab| tab.title.clone()).unwrap_or_default(),
                pid: pane.pty.pid(),
                cols: pane.size.0,
                rows: pane.size.1,
                // Its program's own, never the dashboard's (`pane_state`): the dashboard reads it, and would read back
                // its own echo.
                status: Some(look_state(pane.state))
                    .filter(|state| *state != look::State::Other)
                    .map(|state| state.name().to_string()),
                since: pane.since.filter(|_| look_state(pane.state) != look::State::Other),
                error: pane.error.as_ref().and_then(|error| error.message.clone()),
            })
            .collect()
    }

    fn clients_info(&self) -> Vec<ClientInfo> {
        self.client
            .iter()
            .map(|client| ClientInfo {
                id: client.id.clone(),
                term: client.term.clone(),
                name: client.caps.name.clone(),
                cols: client.size.0,
                rows: client.size.1,
                since: client.since,
            })
            .collect()
    }

    fn stats(&self) -> Stats {
        let panes = self
            .panes
            .iter()
            .map(|pane| PaneStats {
                id: pane.id.clone(),
                member: pane.member.clone(),
                pid: pane.pty.pid(),
                cols: pane.size.0,
                rows: pane.size.1,
                history: pane.with(|engine| engine.history()).unwrap_or(0),
                read: pane.read.load(Ordering::Relaxed),
            })
            .collect();
        Stats { pid: std::process::id(), panes, frames: self.pacing.frames, written: self.pacing.written }
    }

    /// A pane renamed: its color follows it (`team/live.rs` writes team.json only after this).
    fn set_member(&mut self, id: &str, member: String) -> Reply {
        let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) else { return unknown_pane(id) };
        if let Some(name) = self.order.iter_mut().find(|name| **name == pane.member) {
            name.clone_from(&member);
        }
        pane.member = member;
        self.dirty = true;
        done()
    }

    /// A pane's text, or the whole screen as composed, for the tests and the captures.
    fn capture(&mut self, pane: Option<String>, history: usize) -> Reply {
        let Some(which) = pane else {
            return match self.compose() {
                true => value(captured(self.screen.borrow().canvas(), None)),
                false => Reply::Err(t!("un moteur est en échec", "an engine failed")),
            };
        };
        let Some(pane) = self.find(&which) else { return unknown_pane(&which) };
        let (cols, rows) = (pane.size.0 as usize, pane.size.1 as usize);
        let mut canvas = Canvas::new(cols, rows);
        let area = Rect { x: 0, y: 0, width: cols, height: rows };
        let drawn = pane.with(|engine| {
            engine.draw(&mut canvas, area);
            let modes = engine.modes();
            let mouse = match modes.mouse {
                None => 0,
                Some(MouseTracking::Click) => 1000,
                Some(MouseTracking::Drag) => 1002,
                Some(MouseTracking::Motion) => 1003,
            };
            let shown = engine.scrolled() == 0;
            let cursor = engine.cursor().filter(|_| shown).map(|c| (c.x, c.y));
            // The history above the live screen: its last `history` lines still kept.
            let live = engine.top() + engine.scrolled() as u64;
            let first = live.saturating_sub(history as u64).max(engine.oldest());
            let above: Vec<String> = (first..live)
                .filter_map(|line| engine.line(line))
                .map(|line| line.text.trim_end().to_string())
                .collect();
            let modes = PaneModes {
                kitty: modes.kitty,
                bracketed_paste: modes.bracketed_paste,
                focus: modes.focus,
                mouse,
                mouse_sgr: modes.mouse_sgr,
                alt_screen: modes.alt_screen,
                app_cursor: modes.app_cursor,
                scrolled: engine.scrolled(),
                history: engine.history(),
            };
            (cursor, modes, above)
        });
        match drawn {
            Some((cursor, modes, above)) => {
                let mut captured = captured(&canvas, Some(modes));
                captured.cursor = cursor;
                captured.history = above;
                value(captured)
            }
            None => Reply::Err(t!("le moteur du panneau est en échec", "the pane's engine failed")),
        }
    }
}

/// `Ok(null)`: done, nothing to say.
fn done() -> Reply {
    Reply::Ok(serde_json::Value::Null)
}

/// `Ok(value)`.
fn value<T: serde::Serialize>(value: T) -> Reply {
    Reply::Ok(serde_json::to_value(value).unwrap_or_default())
}

fn unknown_pane(which: &str) -> Reply {
    Reply::Err(t!("panneau inconnu : {}", "unknown pane: {}", which))
}

/// What an event from a client is, for a pane whose modes are `modes`.
fn encoded(event: &Event, modes: &Modes) -> Vec<u8> {
    match event {
        Event::Key(key) => input::key(key, modes),
        Event::Paste(text) => input::paste(text, modes),
        Event::Focus(gained) => input::focus(*gained, modes).unwrap_or_default(),
        Event::Mouse(mouse) => input::mouse(mouse, mouse.col, mouse.row, modes).unwrap_or_default(),
        Event::Resize(..) | Event::Closed(_) => Vec::new(),
    }
}
