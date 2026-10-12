// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A tab of members in the « focus » layout (`Layout::Focus`; not the pane that has the keyboard focus): the main
//! member on the left; on the right, the best placed of the others (waiting, then failed, then at work, then the most
//! recently at rest); the rest as cards. Places change slowly: a member that starts waiting or fails its turn comes
//! in at once, in the place of the least well placed, whatever its guard, but never in that of a pane that itself
//! waits or failed; one that starts working comes in after [`ENTER_AFTER`], in the place of the least well placed
//! that has stayed [`STAY_AT_LEAST`] (the guard against coming and going); nothing moves while the server holds the
//! tab (the pointer over it, a key typed, a click less than [`HOLD_AFTER_CLICK`] ago), which comes before everything.
//! No terminal, no clock of its own: the server gives the time, the states and the room (`layout::focus_room`), and
//! lays out the places (`layout::focus_rects`).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A member at work comes to the right after this long at work.
pub(crate) const ENTER_AFTER: Duration = Duration::from_secs(30);
/// A pane that came to the right stays at least this long.
pub(crate) const STAY_AT_LEAST: Duration = Duration::from_secs(60);
/// Nothing moves for this long after a click.
pub(crate) const HOLD_AFTER_CLICK: Duration = Duration::from_secs(1);

/// What places a member, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Standing {
    Waiting,
    Failed,
    Working,
    Resting,
}

/// A member's pane of the tab, as the server sees it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Seen {
    pub id: String,
    pub standing: Standing,
    /// Since when it is in this standing.
    pub since: Instant,
}

/// Where each pane of the tab goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Places {
    pub left: Option<String>,
    /// Top to bottom.
    pub right: Vec<String>,
    /// As cards, best placed first.
    pub others: Vec<String>,
}

/// How well placed a member is: its standing first; among the waiting, the failed and those at work, the longest in it
/// first (so that a place does not change hands for nothing); among those at rest, the most recent first. The tab's
/// reading order settles the rest. Smaller is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rank {
    standing: Standing,
    since: Instant,
    index: usize,
}

impl Ord for Rank {
    fn cmp(&self, other: &Self) -> Ordering {
        self.standing
            .cmp(&other.standing)
            .then_with(|| match self.standing {
                Standing::Resting => other.since.cmp(&self.since),
                _ => self.since.cmp(&other.since),
            })
            .then(self.index.cmp(&other.index))
    }
}

impl PartialOrd for Rank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The places of one tab, kept from one call to the next.
#[derive(Debug, Default)]
pub(crate) struct Focus {
    places: Places,
    /// When each pane on the right took its place.
    placed: HashMap<String, Instant>,
    /// The panes as the last `update` saw them, and when, with the room it had.
    seen: Vec<Seen>,
    now: Option<Instant>,
    room: usize,
    /// When a gesture (⌥n, a click on a card) last moved the places: nothing moves by itself right after.
    gesture: Option<Instant>,
}

impl Focus {
    /// `left` on the left to begin with: the pane that has the focus, else the first.
    pub(crate) fn new(left: Option<String>) -> Focus {
        Focus { places: Places { left, ..Places::default() }, ..Focus::default() }
    }

    pub(crate) fn places(&self) -> &Places {
        &self.places
    }

    fn rank(&self, id: &str) -> Option<Rank> {
        let index = self.seen.iter().position(|p| p.id == id)?;
        let seen = &self.seen[index];
        Some(Rank { standing: seen.standing, since: seen.since, index })
    }

    /// The worst placed of `ids` (largest rank), or the best.
    fn worst(&self, ids: &[String]) -> Option<usize> {
        (0..ids.len()).max_by_key(|&i| self.rank(&ids[i]))
    }

    fn best(&self, ids: &[String]) -> Option<usize> {
        (0..ids.len()).min_by_key(|&i| self.rank(&ids[i]))
    }

    fn gesture_end(&self) -> Option<Instant> {
        self.gesture.map(|at| at + HOLD_AFTER_CLICK)
    }

    /// The places again, from the panes of the tab (reading order) and the room on the right: a pane gone leaves its
    /// place, a new one comes as a card. With `held`, nothing moves but for panes gone or new (and for a room that
    /// shrank). A pane at rest fills a free place on the right at once (the tab's opening), but never takes the place
    /// of another; a waiting or failed one takes the place of the worst placed at once, one at work after
    /// [`ENTER_AFTER`], and neither takes the place of a pane that came less than [`STAY_AT_LEAST`] ago. True when a
    /// place changed.
    pub(crate) fn update(&mut self, panes: &[Seen], room: usize, now: Instant, held: bool) -> bool {
        let before = self.places.clone();
        self.seen = panes.to_vec();
        self.now = Some(now);
        self.room = room;
        let known = |id: &String| panes.iter().any(|p| &p.id == id);
        self.places.right.retain(known);
        self.places.others.retain(known);
        self.placed.retain(|id, _| known(id));
        if self.places.left.as_ref().is_some_and(|id| !known(id)) {
            self.places.left = None;
        }
        for pane in panes {
            let id = &pane.id;
            let placed = self.places.left.as_ref() == Some(id)
                || self.places.right.contains(id)
                || self.places.others.contains(id);
            if !placed {
                self.places.others.push(id.clone());
            }
        }
        // The left one gone: the best placed takes its place.
        if self.places.left.is_none() {
            let (right, others) = (self.places.right.clone(), self.places.others.clone());
            if let Some(i) = self.best(&right) {
                self.places.left = Some(self.places.right.remove(i));
            } else if let Some(i) = self.best(&others) {
                self.places.left = Some(self.places.others.remove(i));
            }
        }
        // A room that shrank: the worst placed go back to cards, held or not.
        while self.places.right.len() > room {
            let right = self.places.right.clone();
            let Some(i) = self.worst(&right) else { break };
            let id = self.places.right.remove(i);
            self.placed.remove(&id);
            self.places.others.push(id);
        }
        let held = held || self.gesture_end().is_some_and(|end| now < end);
        if !held {
            self.fill(now);
            while self.swap(now) {}
        }
        self.sort_others();
        self.places != before
    }

    /// Free places on the right, filled with the best placed cards.
    fn fill(&mut self, now: Instant) {
        while self.places.right.len() < self.room {
            let others = self.places.others.clone();
            let Some(i) = self.best(&others) else { break };
            let id = self.places.others.remove(i);
            self.placed.insert(id.clone(), now);
            self.places.right.push(id);
        }
    }

    /// Whether `card` may take the place of a pane on the right by itself, and when it may: a waiting or failed
    /// member at once, one at work after [`ENTER_AFTER`], one at rest never.
    fn enters_at(&self, card: &Rank, now: Instant) -> Option<Instant> {
        match card.standing {
            Standing::Waiting | Standing::Failed => Some(now),
            Standing::Working => Some(card.since + ENTER_AFTER),
            Standing::Resting => None,
        }
    }

    /// When the pane `id` on the right, ranked `pane`, can give its place to a card ranked `card`: none while it is
    /// ranked better; at once for a card that waits or failed, and never if the pane itself waits or failed (attention
    /// comes before the minute's guard); else once it has stayed [`STAY_AT_LEAST`].
    fn free_at(&self, id: &str, pane: &Rank, card: &Rank) -> Option<Instant> {
        if pane <= card {
            return None;
        }
        let urgent = |r: &Rank| matches!(r.standing, Standing::Waiting | Standing::Failed);
        if urgent(card) {
            return if urgent(pane) { None } else { self.now };
        }
        self.placed.get(id).map(|at| *at + STAY_AT_LEAST).or(self.now)
    }

    /// One card takes the place of the worst placed pane on the right that gives way to it, if one may: true when it
    /// did. The best placed card that may enter goes first.
    fn swap(&mut self, now: Instant) -> bool {
        let mut cards: Vec<(String, Rank)> =
            self.places.others.iter().filter_map(|id| Some((id.clone(), self.rank(id)?))).collect();
        cards.sort_by_key(|(_, rank)| *rank);
        for (card, rank) in cards {
            if self.enters_at(&rank, now).is_none_or(|at| at > now) {
                continue;
            }
            let victim = (0..self.places.right.len())
                .filter(|&i| {
                    let id = &self.places.right[i];
                    self.rank(id).is_some_and(|pane| self.free_at(id, &pane, &rank).is_some_and(|at| at <= now))
                })
                .max_by_key(|&i| self.rank(&self.places.right[i]));
            let Some(v) = victim else { continue };
            let victim = self.places.right[v].clone();
            self.places.others.retain(|id| *id != card);
            self.places.others.push(victim.clone());
            self.placed.remove(&victim);
            self.placed.insert(card.clone(), now);
            self.places.right[v] = card;
            return true;
        }
        false
    }

    /// When `update` should run again even if nothing comes: a member at work reaching [`ENTER_AFTER`], a place
    /// reaching [`STAY_AT_LEAST`] while a better placed one waits for it. None when nothing is pending, or when what is
    /// pending is held back by the server (the time is already past): it runs `update` when it lets go.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        let now = self.now?;
        let gesture = self.gesture_end().unwrap_or(now);
        let mut soonest: Option<Instant> = None;
        for card in self.places.others.iter().filter_map(|id| self.rank(id)) {
            let Some(enters) = self.enters_at(&card, now) else { continue };
            for pane in &self.places.right {
                let Some(free) = self.rank(pane).and_then(|rank| self.free_at(pane, &rank, &card)) else { continue };
                let at = enters.max(free).max(gesture);
                soonest = Some(soonest.map_or(at, |s| s.min(at)));
            }
        }
        soonest.filter(|at| *at > now)
    }

    /// ⌥n: the pane after the left one in `order` (the tab's reading order, wrapping) comes to the left. If it was on
    /// the right, the left one takes its place; if it was a card, as with a click on it ([`Focus::bring`]).
    pub(crate) fn next(&mut self, order: &[String], now: Instant) {
        let Some(left) = self.places.left.clone() else { return };
        let start = order.iter().position(|id| *id == left).map_or(0, |i| i + 1);
        let target = (0..order.len())
            .map(|k| &order[(start + k) % order.len()])
            .find(|id| **id != left && (self.places.right.contains(id) || self.places.others.contains(id)))
            .cloned();
        if let Some(id) = target {
            self.bring(&id, now);
        }
    }

    /// `id` comes to the left. From the right (a double click on its header), the left one takes its place. From a
    /// card (a click on it, or ⌥n), the left one takes a free place on the right if there is one, else the place of
    /// the least well placed, which becomes a card.
    pub(crate) fn bring(&mut self, id: &str, now: Instant) {
        if !self.places.others.iter().any(|other| other == id) {
            return self.exchange(id, now);
        }
        let Some(left) = self.places.left.clone() else { return };
        if self.places.right.len() < self.room {
            self.places.right.push(left.clone());
            self.placed.insert(left, now);
        } else {
            let right = self.places.right.clone();
            let Some(v) = self.worst(&right) else { return self.exchange(id, now) };
            let victim = std::mem::replace(&mut self.places.right[v], left.clone());
            self.placed.remove(&victim);
            self.placed.insert(left, now);
            self.places.others.push(victim);
        }
        self.places.others.retain(|other| other != id);
        self.places.left = Some(id.to_string());
        self.gesture = Some(now);
        self.sort_others();
    }

    /// `id` (on the right or a card) and the left one swap places.
    fn exchange(&mut self, id: &str, now: Instant) {
        let Some(left) = self.places.left.clone() else { return };
        if let Some(i) = self.places.right.iter().position(|other| other == id) {
            self.places.right[i] = left.clone();
            self.placed.insert(left, now);
            self.placed.remove(id);
        } else if let Some(i) = self.places.others.iter().position(|other| other == id) {
            self.places.others[i] = left;
        } else {
            return;
        }
        self.places.left = Some(id.to_string());
        self.gesture = Some(now);
        self.sort_others();
    }

    /// The cards, best placed first.
    fn sort_others(&mut self) {
        let mut others = std::mem::take(&mut self.places.others);
        others.sort_by_key(|id| self.rank(id));
        self.places.others = others;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use Standing::{Failed, Resting, Waiting, Working};

    fn secs(base: Instant, n: u64) -> Instant {
        base + Duration::from_secs(n)
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Panes `(id, standing, since in seconds from `base`)`.
    fn panes(base: Instant, list: &[(&str, Standing, u64)]) -> Vec<Seen> {
        list.iter().map(|&(id, standing, since)| Seen { id: id.into(), standing, since: secs(base, since) }).collect()
    }

    /// A tab: a at work on the left, b waiting, c at work, d at rest for 2 minutes, e and f at
    /// rest longer.
    fn opening(base: Instant) -> (Focus, Vec<Seen>) {
        let all = panes(
            base,
            &[
                ("a", Working, 0),
                ("b", Waiting, 0),
                ("c", Working, 0),
                ("d", Resting, 100),
                ("e", Resting, 50),
                ("f", Resting, 10),
            ],
        );
        let mut focus = Focus::new(Some("a".into()));
        focus.update(&all, 3, secs(base, 200), false);
        (focus, all)
    }

    #[test]
    fn the_opening_fills_the_right_by_rank() {
        let base = Instant::now();
        let (focus, _) = opening(base);
        let places = focus.places();
        assert_eq!(places.left.as_deref(), Some("a"));
        // Waiting, then at work, then the most recently at rest.
        assert_eq!(places.right, ids(&["b", "c", "d"]));
        assert_eq!(places.others, ids(&["e", "f"]));
    }

    #[test]
    fn a_smaller_room_sends_the_worst_placed_back_to_cards() {
        let base = Instant::now();
        let (mut focus, all) = opening(base);
        assert!(focus.update(&all, 2, secs(base, 201), true));
        assert_eq!(focus.places().right, ids(&["b", "c"]));
        assert_eq!(focus.places().others, ids(&["d", "e", "f"]));
        // A larger one takes the best placed card back, once nothing holds the tab.
        assert!(!focus.update(&all, 3, secs(base, 202), true));
        assert!(focus.update(&all, 3, secs(base, 203), false));
        assert_eq!(focus.places().right, ids(&["b", "c", "d"]));
    }

    /// Who enters, and when, for a card changing standing at `since` while the right holds b, c, d from t = 200.
    #[test]
    fn a_card_enters_by_its_standing() {
        // (standing of e from t = 300, when asked, whether e is on the right then)
        let table = [
            (Waiting, 300, true),
            (Failed, 300, true),
            (Working, 300, false),
            (Working, 329, false),
            (Working, 330, true),
            (Resting, 400, false),
        ];
        for (standing, at, enters) in table {
            let base = Instant::now();
            let (mut focus, mut all) = opening(base);
            // Everyone has stayed long enough by t = 300.
            all[4] = Seen { standing, since: secs(base, 300), ..all[4].clone() };
            focus.update(&all, 3, secs(base, at), false);
            assert_eq!(focus.places().right.contains(&"e".to_string()), enters, "{standing:?} at {at}");
        }
    }

    #[test]
    fn the_least_well_placed_makes_room_and_a_pane_stays_a_minute() {
        let base = Instant::now();
        let (mut focus, mut all) = opening(base);
        // e starts working at t = 200, when the right was filled: it may come at 230, but d, the worst placed, must stay
        // until 260.
        all[4] = Seen { standing: Working, since: secs(base, 200), ..all[4].clone() };
        assert!(!focus.update(&all, 3, secs(base, 230), false));
        assert_eq!(focus.deadline(), Some(secs(base, 260)));
        assert!(!focus.update(&all, 3, secs(base, 259), false));
        assert!(focus.update(&all, 3, secs(base, 260), false));
        // In d's place, nothing else moved; d is a card.
        assert_eq!(focus.places().right, ids(&["b", "c", "e"]));
        assert_eq!(focus.places().others, ids(&["d", "f"]));
        assert_eq!(focus.deadline(), None);
    }

    #[test]
    fn a_member_who_waits_or_failed_comes_in_at_once() {
        // The captures' case: three panes that just took their places, then a card that waits.
        for standing in [Waiting, Failed] {
            let base = Instant::now();
            let (mut focus, mut all) = opening(base);
            all[5] = Seen { standing, since: secs(base, 201), ..all[5].clone() };
            assert!(focus.update(&all, 3, secs(base, 201), false), "{standing:?}");
            // In the place of d, the worst placed, whatever its guard.
            assert_eq!(focus.places().right, ids(&["b", "c", "f"]), "{standing:?}");
            assert_eq!(focus.places().others, ids(&["d", "e"]));
            assert_eq!(focus.deadline(), None);
        }
    }

    #[test]
    fn attention_never_takes_the_place_of_attention() {
        let base = Instant::now();
        let all = panes(
            base,
            &[
                ("a", Working, 0),
                ("b", Waiting, 0),
                ("c", Failed, 0),
                ("d", Waiting, 0),
                ("e", Waiting, 201),
                ("f", Working, 0),
            ],
        );
        let mut focus = Focus::new(Some("a".into()));
        // The three panes on the right wait or failed: e, a card that waits, takes none of their places.
        let mut start = all.clone();
        start[4] = Seen { standing: Resting, since: secs(base, 100), ..start[4].clone() };
        focus.update(&start, 3, secs(base, 200), false);
        assert_eq!(focus.places().right, ids(&["b", "d", "c"]));
        focus.update(&all, 3, secs(base, 201), false);
        assert_eq!(focus.places().right, ids(&["b", "d", "c"]));
        assert_eq!(focus.deadline(), None);
        // A pane at work on the right gives way at once, though it came a second ago: e waits.
        let mut mixed = all.clone();
        mixed[3] = Seen { standing: Working, since: secs(base, 0), ..mixed[3].clone() };
        let mut focus = Focus::new(Some("a".into()));
        let mut start = mixed.clone();
        start[4] = Seen { standing: Resting, since: secs(base, 100), ..start[4].clone() };
        focus.update(&start, 3, secs(base, 200), false);
        assert_eq!(focus.places().right, ids(&["b", "c", "d"]));
        focus.update(&mixed, 3, secs(base, 201), false);
        assert_eq!(focus.places().right, ids(&["b", "c", "e"]));
    }

    #[test]
    fn a_member_at_work_waits_for_the_guard_of_a_pane() {
        let base = Instant::now();
        let (mut focus, mut all) = opening(base);
        // Attention first: held, or not, a pane at work does not shorten the guard.
        all[5] = Seen { standing: Working, since: secs(base, 201), ..all[5].clone() };
        focus.update(&all, 3, secs(base, 231), false);
        assert_eq!(focus.places().right, ids(&["b", "c", "d"]));
        assert_eq!(focus.deadline(), Some(secs(base, 260)));
    }

    #[test]
    fn a_member_at_work_has_a_deadline() {
        let base = Instant::now();
        let (mut focus, mut all) = opening(base);
        all[4] = Seen { standing: Working, since: secs(base, 300), ..all[4].clone() };
        focus.update(&all, 3, secs(base, 300), false);
        assert_eq!(focus.deadline(), Some(secs(base, 330)));
        // Nothing to wait for with only cards at rest.
        let (focus, _) = opening(base);
        assert_eq!(focus.deadline(), None);
    }

    #[test]
    fn a_held_tab_moves_only_for_panes_gone_or_new() {
        let base = Instant::now();
        let (mut focus, mut all) = opening(base);
        all[4] = Seen { standing: Waiting, since: secs(base, 300), ..all[4].clone() };
        assert!(!focus.update(&all, 3, secs(base, 400), true));
        assert_eq!(focus.places().right, ids(&["b", "c", "d"]));
        // Held and past: no deadline to spin on.
        assert_eq!(focus.deadline(), None);
        // A new pane is a card; one gone leaves its place.
        all.remove(2);
        all.push(Seen { id: "g".into(), standing: Resting, since: secs(base, 390) });
        assert!(focus.update(&all, 3, secs(base, 401), true));
        assert_eq!(focus.places().right, ids(&["b", "d"]));
        assert_eq!(focus.places().others, ids(&["e", "g", "f"]));
        // Let go: e waits, and takes the free place.
        focus.update(&all, 3, secs(base, 402), false);
        assert_eq!(focus.places().right, ids(&["b", "d", "e"]));
    }

    #[test]
    fn the_left_one_gone_the_best_placed_takes_its_place() {
        let base = Instant::now();
        let (mut focus, mut all) = opening(base);
        all.remove(0);
        focus.update(&all, 3, secs(base, 201), false);
        assert_eq!(focus.places().left.as_deref(), Some("b"));
        assert_eq!(focus.places().right, ids(&["c", "d", "e"]));
        assert_eq!(focus.places().others, ids(&["f"]));
    }

    #[test]
    fn next_walks_the_tab_as_a_click_would() {
        let base = Instant::now();
        let (mut focus, _) = opening(base);
        let order = ids(&["a", "b", "c", "d", "e", "f"]);
        // b is on the right: a takes its place.
        focus.next(&order, secs(base, 210));
        assert_eq!(focus.places().left.as_deref(), Some("b"));
        assert_eq!(focus.places().right, ids(&["a", "c", "d"]));
        // c, then d, on the right too.
        focus.next(&order, secs(base, 211));
        focus.next(&order, secs(base, 212));
        assert_eq!(focus.places().left.as_deref(), Some("d"));
        assert_eq!(focus.places().right, ids(&["a", "b", "c"]));
        // e is a card: d takes the place of the worst placed (c, at work as long as a, after it), which becomes a card.
        focus.next(&order, secs(base, 213));
        assert_eq!(focus.places().left.as_deref(), Some("e"));
        assert_eq!(focus.places().right, ids(&["a", "b", "d"]));
        assert_eq!(focus.places().others, ids(&["c", "f"]));
        // f, then wrapping to a.
        focus.next(&order, secs(base, 214));
        assert_eq!(focus.places().left.as_deref(), Some("f"));
        focus.next(&order, secs(base, 215));
        assert_eq!(focus.places().left.as_deref(), Some("a"));
    }

    #[test]
    fn next_to_a_card_takes_a_free_place_first() {
        let base = Instant::now();
        let (mut focus, all) = opening(base);
        // The room shrank to two, then grew back, held: one place is free, the best placed card not in it yet.
        focus.update(&all, 2, secs(base, 201), true);
        focus.update(&all, 3, secs(base, 202), true);
        assert_eq!(focus.places().right, ids(&["b", "c"]));
        focus.next(&ids(&["a", "d", "b"]), secs(base, 203));
        assert_eq!(focus.places().left.as_deref(), Some("d"));
        assert_eq!(focus.places().right, ids(&["b", "c", "a"]));
    }

    #[test]
    fn a_protected_pane_does_not_stop_the_others_from_giving_way() {
        let base = Instant::now();
        let mut all = panes(
            base,
            &[
                ("a", Working, 0),
                ("b", Resting, 100),
                ("c", Resting, 90),
                ("d", Resting, 80),
                ("e", Resting, 50),
                ("f", Resting, 10),
            ],
        );
        let mut focus = Focus::new(Some("a".into()));
        focus.update(&all, 3, secs(base, 200), false);
        assert_eq!(focus.places().right, ids(&["b", "c", "d"]));
        // d goes back to a card, and comes again at t = 255: protected until 315, the worst placed.
        focus.update(&all, 2, secs(base, 205), false);
        focus.update(&all, 3, secs(base, 255), false);
        assert_eq!(focus.places().right, ids(&["b", "c", "d"]));
        // f works from t = 256, and may come at 286. b and c have stayed since 260; c is the worst of them.
        all[5] = Seen { standing: Working, since: secs(base, 256), ..all[5].clone() };
        focus.update(&all, 3, secs(base, 285), false);
        assert_eq!(focus.places().right, ids(&["b", "c", "d"]));
        assert_eq!(focus.deadline(), Some(secs(base, 286)));
        assert!(focus.update(&all, 3, secs(base, 286), false));
        assert_eq!(focus.places().right, ids(&["b", "f", "d"]));
        assert_eq!(focus.places().others, ids(&["c", "e"]));
    }

    #[test]
    fn bring_from_a_card_replaces_the_worst_placed() {
        let base = Instant::now();
        let (mut focus, all) = opening(base);
        focus.bring("e", secs(base, 210));
        // e on the left; a in d's place, the worst placed, which is a card now.
        assert_eq!(focus.places().left.as_deref(), Some("e"));
        assert_eq!(focus.places().right, ids(&["b", "c", "a"]));
        assert_eq!(focus.places().others, ids(&["d", "f"]));
        // Nothing moves by itself for a second after a gesture.
        let mut waiting = all.clone();
        waiting[5] = Seen { standing: Waiting, since: secs(base, 210), ..waiting[5].clone() };
        focus.update(&waiting, 3, secs(base, 210) + Duration::from_millis(500), false);
        assert_eq!(focus.places().right, ids(&["b", "c", "a"]));
        // Held for a second after the gesture; then f, who waits, takes the place of the worst placed at once: c.
        assert_eq!(focus.deadline(), Some(secs(base, 211)));
        assert!(focus.update(&waiting, 3, secs(base, 211), false));
        assert_eq!(focus.places().right, ids(&["b", "f", "a"]));
    }

    #[test]
    fn bring_from_the_right_swaps_with_the_left() {
        let base = Instant::now();
        let (mut focus, _) = opening(base);
        focus.bring("c", secs(base, 210));
        assert_eq!(focus.places().left.as_deref(), Some("c"));
        assert_eq!(focus.places().right, ids(&["b", "a", "d"]));
        assert_eq!(focus.places().others, ids(&["e", "f"]));
        // Unknown, or already on the left: nothing.
        let before = focus.places().clone();
        focus.bring("zz", secs(base, 211));
        focus.bring("c", secs(base, 211));
        assert_eq!(*focus.places(), before);
    }

    #[test]
    fn bring_takes_a_free_place_before_a_pane() {
        let base = Instant::now();
        let all = panes(base, &[("a", Working, 0), ("b", Resting, 0), ("c", Resting, 0)]);
        let mut focus = Focus::new(None);
        focus.update(&all, 3, secs(base, 5), true);
        // Held, so no one is on the right yet; c is a card, and a is on the left, the first to come.
        assert_eq!(focus.places().left.as_deref(), Some("a"));
        focus.bring("c", secs(base, 6));
        assert_eq!(focus.places().left.as_deref(), Some("c"));
    }
}
