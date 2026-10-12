// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The tabs in « focus »: the places of the panes, the cards of the others, and what holds them in place.

use std::time::{Duration, Instant};

use super::Server;
use super::frame::member_color;
use super::lifecycle::now;
use super::status::{pane_since, pane_state};
use super::tabs::Tab;
use crate::board;
use crate::mux::cards;
use crate::mux::chrome;
use crate::mux::focus::{self, Focus, Seen, Standing};
use crate::mux::layout;
use crate::ui::Rect;
use crate::ui::look;

/// How long a tab in « focus » stays as it is after the pointer moved over a pane on the right or the cards.
pub(super) const POINTER_HOLD: Duration = Duration::from_secs(5);

/// The id of the cards' frame among the views: no pane has it.
pub(super) const CARDS: &str = "cards";

/// Two presses on a header this close are a double click.
pub(super) const DOUBLE_CLICK: Duration = Duration::from_millis(400);

impl Server {
    /// Whether tab `t` shows its members in « focus »: a tab of agents (no panels beside it) of two members or more,
    /// the team's choice or ⌥l's, and a screen with room for it (`layout::focus_room`).
    fn in_focus(&self, t: usize) -> bool {
        let Some(tab) = self.tabs.get(t) else { return false };
        let (width, height) = self.tab_room();
        tab.side.is_none()
            && tab.panes.len() >= 2
            && tab.mode.unwrap_or(self.focus_layout)
            && layout::focus_room(width, height).is_some()
    }

    /// The panes of tab `t` as the places of « focus » see them: how well placed each one is, and since when.
    pub(super) fn focus_seen(&self, t: usize, now: Instant) -> Vec<Seen> {
        let epoch = self::now();
        let Some(tab) = self.tabs.get(t) else { return Vec::new() };
        tab.panes
            .iter()
            .filter_map(|id| self.pane(id))
            .map(|pane| {
                let standing = match pane_state(pane, &self.looks) {
                    look::State::Waiting => Standing::Waiting,
                    look::State::Error => Standing::Failed,
                    look::State::Working => Standing::Working,
                    look::State::Idle | look::State::Other => Standing::Resting,
                };
                let since = pane_since(pane, &self.looks)
                    .and_then(|since| now.checked_sub(Duration::from_secs(epoch.saturating_sub(since))))
                    .unwrap_or(self.pacing.born);
                Seen { id: pane.id.clone(), standing, since }
            })
            .collect()
    }

    /// The places of every tab in « focus » brought up to date with its panes, and dropped for a tab in its grid.
    /// Only what the panes' coming and going and the room force: nothing is reordered here ([`Server::update_focus`]).
    /// The pane with the focus is always one of those shown: a card that gets it comes to the left.
    pub(super) fn ensure_focus(&mut self) {
        let (width, height) = self.tab_room();
        let room = layout::focus_room(width, height).unwrap_or(0);
        let now = Instant::now();
        for t in 0..self.tabs.len() {
            if !self.in_focus(t) {
                self.tabs[t].focus = None;
                continue;
            }
            let seen = self.focus_seen(t, now);
            let focused = self.focus.clone();
            let Tab { last, panes, focus, .. } = &mut self.tabs[t];
            // A tab that opens in « focus » fills its right column at once; after, only what is forced.
            let opening = focus.is_none();
            let focus = focus.get_or_insert_with(|| {
                Focus::new(last.clone().filter(|id| panes.contains(id)).or_else(|| panes.first().cloned()))
            });
            focus.update(&seen, room, now, !opening);
            if let Some(id) = focused.filter(|id| focus.places().others.contains(id)) {
                focus.bring(&id, now);
            }
        }
    }

    /// The tabs in « focus » look at their places again: a member that began to wait, to work, a place that came due
    /// (`Focus::deadline`). Nothing moves in the tab shown while the pointer is on it, a key was just typed, or a click
    /// was just made; the places catch up when that ends. Moved places lay the tab out again.
    pub(super) fn update_focus(&mut self) {
        self.update_focus_at(Instant::now());
    }

    /// [`Server::update_focus`] as of `now`.
    fn update_focus_at(&mut self, now: Instant) {
        if self.tabs.iter().all(|tab| tab.focus.is_none()) {
            self.focus_wake = None;
            return;
        }
        let (width, height) = self.tab_room();
        let room = layout::focus_room(width, height).unwrap_or(0);
        let (held, ends) = self.holding(now);
        let mut changed = false;
        let mut wake: Option<Instant> = ends;
        for t in 0..self.tabs.len() {
            if self.tabs[t].focus.is_none() {
                continue;
            }
            let seen = self.focus_seen(t, now);
            let hold = held && t == self.active;
            if let Some(focus) = self.tabs[t].focus.as_mut() {
                changed |= focus.update(&seen, room, now, hold);
                wake = [wake, focus.deadline()].into_iter().flatten().min();
            }
        }
        self.focus_wake = wake;
        if changed {
            self.layout();
        }
    }

    /// Whether the tab shown is held in place, and when the hold that has an end ends: the pointer is on a pane on the
    /// right or on the cards and moved less than [`POINTER_HOLD`] ago, a key was typed or a click made less than a
    /// second ago (nothing tells that either is over: the wake-up is for the end of the hold).
    pub(super) fn holding(&self, now: Instant) -> (bool, Option<Instant>) {
        let mut ends: Vec<Instant> = [self.pointer.last_key, self.pointer.last_click]
            .into_iter()
            .flatten()
            .map(|at| at + focus::HOLD_AFTER_CLICK)
            .collect();
        if let (Some((col, row)), Some(at)) = (self.pointer.at, self.pointer.moved)
            && self.client.is_some()
            && self.under_pointer(col, row)
        {
            ends.push(at + POINTER_HOLD);
        }
        let ends: Vec<Instant> = ends.into_iter().filter(|end| *end > now).collect();
        (!ends.is_empty(), ends.into_iter().max())
    }

    /// Whether `col`, `row` is on a pane on the right of the tab shown in « focus », or on its cards.
    fn under_pointer(&self, col: usize, row: usize) -> bool {
        let Some(focus) = self.tabs.get(self.active).and_then(|tab| tab.focus.as_ref()) else { return false };
        let right = focus.places().right.iter().filter_map(|id| self.pane(id));
        right.into_iter().any(|pane| pane.area.contains(col, row))
            || self.cards_rect().is_some_and(|rect| rect.contains(col, row))
    }

    /// Where the cards of the members left out go in the tab shown in « focus »: their frame, in the room that
    /// `layout::focus_rects` leaves under the panes on the right. The same room, the same call, as `layout`.
    pub(super) fn cards_rect(&self) -> Option<Rect> {
        let tab = self.tabs.get(self.active).filter(|tab| tab.zoomed.is_none())?;
        let places = tab.focus.as_ref()?.places();
        let (width, height) = self.tab_room();
        layout::focus_rects(width, height, places.left.as_deref()?, &places.right, places.others.len()).1
    }

    /// The cards of the tab shown, made again from the looks, the panes' states and the places: a card has what the
    /// dashboard last wrote of its member (what it does, its curve), and none of it with no dashboard.
    pub(super) fn refresh_cards(&mut self) {
        let (mut cards, mut ids, mut epochs) = (Vec::new(), Vec::new(), Vec::new());
        if self.cards_rect().is_some() {
            let places = self.tabs[self.active].focus.as_ref().map(|focus| focus.places().clone()).unwrap_or_default();
            let epoch = now();
            for pane in places.others.iter().filter_map(|id| self.pane(id)) {
                let state = pane_state(pane, &self.looks);
                let look = self.looks.get(&pane.member);
                let began = pane_since(pane, &self.looks);
                cards.push(board::CardData {
                    name: pane.member.clone(),
                    color: member_color(&self.order, &pane.member),
                    state,
                    since: began.map_or(0, |began| epoch.saturating_sub(began)),
                    shell: look.is_some_and(|look| look.shell)
                        && !matches!(state, look::State::Working | look::State::Waiting | look::State::Error),
                    context: look.and_then(|look| look.context).map(f64::from),
                    pressure: look.map(|look| look.pressure).unwrap_or_default(),
                    model: look.and_then(|look| look.model.clone()),
                    effort: look.and_then(|look| look.effort.clone()),
                    doing: look.and_then(|look| look.doing.clone()),
                    activity: look.map(|look| look.activity.clone()).unwrap_or_default(),
                    failed: (state == look::State::Error).then(|| {
                        pane.error.as_ref().map_or(self.notices.unknown_error.clone(), |error| error.label.clone())
                    }),
                    compacted: look.and_then(|look| look.compacted),
                });
                ids.push(pane.id.clone());
                epochs.push(began);
            }
        }
        if !cards.is_empty() || !self.cards.data.is_empty() {
            self.dirty = true;
        }
        (self.cards.data, self.cards.ids, self.cards.epoch) = (cards, ids, epochs);
    }

    /// A press on the cards' frame: on a card, that member comes to the left. True when the press was in the frame.
    pub(super) fn card_click(&mut self, col: usize, row: usize) -> bool {
        let Some(rect) = self.cards_rect().filter(|rect| rect.contains(col, row)) else { return false };
        let wide = self.tab_room().0 >= layout::FOCUS_WIDE;
        let placed = cards::place(chrome::inside(rect), &self.cards.data, wide);
        let hit = placed.shown.iter().find(|(_, at)| at.contains(col, row)).map(|(i, _)| *i);
        if let Some(id) = hit.and_then(|i| self.cards.ids.get(i).cloned()) {
            if let Some(focus) = self.active_focus_mut() {
                focus.bring(&id, Instant::now());
            }
            self.focus_on(&id);
            self.layout();
        }
        true
    }

    /// The bar's layout button for tab `t`: where ⌥l would take it. None for a tab it cannot go to « focus » in (panels
    /// beside it, a single member, a screen too small).
    pub(super) fn switch(&self, t: usize) -> Option<chrome::Switch> {
        let tab = self.tabs.get(t)?;
        let (width, height) = self.tab_room();
        if tab.side.is_some() || tab.panes.len() < 2 || layout::focus_room(width, height).is_none() {
            return None;
        }
        Some(if self.in_focus(t) { chrome::Switch::ToGrid } else { chrome::Switch::ToFocus })
    }

    /// ⌥l and the bar's button: the tab shown goes from its grid to « focus », or back, until the client leaves.
    pub(super) fn toggle_layout(&mut self) {
        let t = self.active;
        let Some(tab) = self.tabs.get(t) else { return };
        if tab.side.is_some() || tab.panes.len() < 2 {
            return;
        }
        let focus = self.in_focus(t);
        self.tabs[t].mode = Some(!focus);
        self.unselect();
        self.layout();
    }

    /// The pane on the right of the tab shown whose header row is at `col`, `row`.
    pub(super) fn right_header_at(&self, col: usize, row: usize) -> Option<String> {
        let places = self.tabs.get(self.active)?.focus.as_ref()?.places();
        places
            .right
            .iter()
            .find(|id| self.pane(id).is_some_and(|pane| row == pane.area.y && pane.area.contains(col, row)))
            .cloned()
    }
}

/// The cards of the members left out of the tab shown in « focus ».
#[derive(Default)]
pub(super) struct Cards {
    /// The cards of the members left out of the tab shown in « focus », best placed first, with their panes and the
    /// time each began its state (`since` ticks in them); rebuilt when the looks, a state or the places change.
    pub(super) data: Vec<board::CardData>,
    pub(super) ids: Vec<String>,
    pub(super) epoch: Vec<Option<u64>>,
    pub(super) stale: bool,
}
