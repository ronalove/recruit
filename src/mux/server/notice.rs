// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The notice of a member gone waiting, or whose turn failed, out of sight.

use std::time::{Duration, Instant};

use super::Server;
use super::frame::member_color;
use super::status::pane_state;
use crate::mux::chrome;
use crate::ui::Rect;
use crate::ui::look;

/// How long a notice shows.
const NOTICE: Duration = Duration::from_secs(6);

/// Failed turns this close to the first one noticed share its notice (an API outage often hits the whole team).
pub(super) const BURST: Duration = Duration::from_secs(10);

impl Server {
    /// The notices of members gone waiting or failed out of sight, in turn: a wait last, which wins.
    pub(super) fn call(&mut self, mut called: Vec<(String, look::State)>) {
        called.sort_by_key(|(_, state)| *state == look::State::Waiting);
        for (id, state) in called {
            match state {
                look::State::Waiting => self.notify(&id),
                _ => self.notify_error(&id),
            }
        }
    }

    /// The notice of a member gone waiting out of sight, for [`NOTICE`]: the latest only; the others keep their tab's
    /// badge (⚑). None under a layer (the menu, a choice): nothing covers it, the menu shows each member's state, and
    /// the notice is forgotten, not shown once the layer goes.
    pub(super) fn notify(&mut self, id: &str) {
        // Without a client, no one to tell: no notice, nor its wake-up.
        if self.client.is_none() || self.menu.is_some() || self.choice.is_some() {
            return;
        }
        let Some(pane) = self.pane(id) else { return };
        let Some(tab) = self.tabs.iter().find(|tab| tab.has(id)) else { return };
        let notice = chrome::Notice {
            member: pane.member.clone(),
            state: look::State::Waiting,
            color: Some(member_color(&self.order, &pane.member)),
            text: chrome::notice_text(&pane.member),
            tab: tab.title.clone(),
        };
        self.notices.notice = Some((notice, Instant::now() + NOTICE));
        self.dirty = true;
    }

    /// The notice of a member whose turn failed out of sight, orange: one for the failures within
    /// [`BURST`] of the first, which the later ones join while it shows (« 3 membres ont raté leur tour »), and none
    /// else. Never over a wait's notice, nor under a layer, as a wait's (`notify`).
    fn notify_error(&mut self, id: &str) {
        if self.client.is_none() || self.menu.is_some() || self.choice.is_some() {
            return;
        }
        if self.notices.notice.as_ref().is_some_and(|(notice, _)| notice.state == look::State::Waiting) {
            return;
        }
        let Some(pane) = self.pane(id) else { return };
        let Some(tab) = self.tabs.iter().find(|tab| tab.has(id)) else { return };
        let label = pane.error.as_ref().map_or(self.notices.unknown_error.clone(), |error| error.label.clone());
        let (member, tab) = (pane.member.clone(), tab.title.clone());
        let now = Instant::now();
        let members = match &mut self.notices.burst {
            Some((since, members)) if now.duration_since(*since) < BURST => {
                if !members.contains(&member) {
                    members.push(member.clone());
                }
                // Its notice gone already (its time, a layer): no other.
                if !self.notices.notice.as_ref().is_some_and(|(notice, _)| notice.state == look::State::Error) {
                    return;
                }
                members.clone()
            }
            _ => {
                self.notices.burst = Some((now, vec![member.clone()]));
                vec![member.clone()]
            }
        };
        let names: Vec<&str> = members.iter().map(String::as_str).collect();
        let notice = chrome::Notice {
            state: look::State::Error,
            color: Some(member_color(&self.order, &member)),
            text: chrome::error_notice_text(&names, &label),
            member,
            tab,
        };
        // Joined, it keeps its time.
        let until = match &self.notices.notice {
            Some((shown, until)) if shown.state == look::State::Error => *until,
            _ => now + NOTICE,
        };
        self.notices.notice = Some((notice, until));
        self.dirty = true;
    }

    /// The notice gone once its time is up, once a layer opens, or once its members are all in sight or no longer
    /// in the state it tells.
    pub(super) fn check_notice(&mut self) {
        let Some((notice, until)) = &self.notices.notice else { return };
        let members = match &self.notices.burst {
            Some((_, members)) if notice.state == look::State::Error => members.clone(),
            _ => vec![notice.member.clone()],
        };
        let shown = self.in_view_ids();
        let calling = self
            .panes
            .iter()
            .filter(|pane| pane.is_member() && members.contains(&pane.member))
            .any(|pane| pane_state(pane, &self.looks) == notice.state && !shown.contains(&pane.id));
        let over = *until <= Instant::now() || self.menu.is_some() || self.choice.is_some() || !calling;
        if over {
            self.notices.notice = None;
            self.dirty = true;
        }
    }
}

/// The notice of a member gone waiting out of sight, or whose turn failed.
pub(super) struct Notices {
    /// A member gone waiting out of sight, or whose turn failed: its notice, until when it shows.
    pub(super) notice: Option<(chrome::Notice, Instant)>,
    /// The failed turns told by one notice: since when, and whose. Others within [`BURST`] join it, none other shows.
    pub(super) burst: Option<(Instant, Vec<String>)>,
    /// A failed turn's reason when its text is not known (as the dashboard saw it): « erreur d'API ».
    pub(super) unknown_error: String,
    /// Where the notice can be clicked, as last drawn.
    pub(super) notice_zone: Option<Rect>,
}

impl Notices {
    pub(super) fn new() -> Notices {
        Notices { notice: None, burst: None, unknown_error: look::error_label(None), notice_zone: None }
    }
}
