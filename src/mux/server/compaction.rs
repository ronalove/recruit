// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The automatic compaction of the members at rest: who is due, the requests, and their outcomes.

use std::path::Path;
use std::sync::PoisonError;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use super::lifecycle::{log, now};
use super::status::{note_status, pane_since, pane_state};
use super::{Msg, Server};
use crate::claude::bridge;
use crate::ui::look;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// How long before a compaction asked is asked again, when it was refused for a subagent or a teammate at work, or got
/// no answer.
pub(super) const AUTO_AGAIN: Duration = Duration::from_secs(5 * 60);

/// How long a request is waited for, in seconds' worth of time, before it counts as unanswered: it lapses after
/// `COMPACTION_LAPSE` (10 s), its mod answers at its next tick, and `watch_looks` reads the report every [`LOOKS`].
pub(super) const AUTO_PENDING: Duration = Duration::from_secs(30);

/// How long a member stays at rest after it was asked to compact on its own, at most, whatever its program says:
/// its answer, `done`, `skip` or `error`, ends it sooner.
pub(super) const COMPACTING: Duration = Duration::from_secs(3 * 60);

/// A member asked to compact on its own, in the rest that began at `since`.
pub(super) struct Asked {
    /// The rest it was asked in: a new turn starts another.
    pub(super) since: u64,
    /// The request's id, once written: its report's `compaction` answers with it.
    pub(super) id: Option<String>,
    /// When it was asked last.
    pub(super) at: Instant,
    /// Its program said another state than `working` since: the compaction is over, a `working` is a turn's again.
    pub(super) ended: bool,
    /// The earlier requests of this rest that got no answer, failed, or were refused for another reason than a
    /// subagent or a teammate at work (those do not count).
    pub(super) failures: u8,
}

/// Set, in seconds, the time a member rests before its context is compacted on its own (tests). Not documented.
pub(super) const AUTO_IDLE: &str = "RECRUIT_AUTO_COMPACT_IDLE";

/// A compaction skipped because the member was not in a state to be compacted (a subagent or a teammate at work, a turn
/// running): not a failed try, the member is asked again later. Any other skip (a hook that refuses) is one.
fn not_a_failure(outcome: &bridge::Compaction) -> bool {
    matches!(outcome.skip.as_deref(), Some(bridge::SKIP_HELPERS | bridge::SKIP_BUSY))
}

/// Writes a member's automatic compaction request (`bridge::request_auto_compaction`: a small file, which its mod takes
/// at its next tick) and tells the loop its id, or why not.
pub(super) fn ask_compaction(state: &Path, tx: &Sender<Msg>, member: &str, percent: u8, since: u64) {
    let id = bridge::request_auto_compaction(state, member, percent).map_err(|error| format!("{error:#}"));
    let _ = tx.send(Msg::Requested(member.to_string(), since, id));
}

impl Server {
    /// The members to compact on their own, as of `now` (seconds since the epoch), and how long before looking again
    /// for the rest not long enough yet, or the request to renew. A member qualifies when the
    /// team has the compaction on, it is at rest (by its program, else the dashboard) since `auto_idle` seconds with no
    /// command running, its context is at the team's share, it is on no client's screen (the panes of the tab shown, or
    /// the zoomed one alone), and [`Server::renewal`] allows a request. Whether a subagent or a teammate works, the
    /// bridge says. What else changes it (a state, the looks, a tab shown) comes through the loop, which looks again.
    pub(super) fn compaction_due(&self, now: u64) -> (Vec<(String, u8, u64)>, Option<Duration>) {
        let Some(share) = self.auto.auto_compact else { return (Vec::new(), None) };
        let shown = if self.client.is_some() { self.shown_ids() } else { Vec::new() };
        let mut due = Vec::new();
        let mut wait: Option<Duration> = None;
        let mut later = |left: Duration| wait = Some(wait.map_or(left, |wait| wait.min(left)));
        for pane in self.panes.iter().filter(|pane| pane.is_member() && !pane.exited) {
            if pane_state(pane, &self.looks) != look::State::Idle {
                continue;
            }
            let Some(look) = self.looks.get(&pane.member) else { continue };
            let Some(context) = look.context.filter(|context| *context >= share) else { continue };
            let Some(since) = pane_since(pane, &self.looks) else { continue };
            if look.shell {
                continue;
            }
            let Some(left) = self.renewal(&pane.member, since) else { continue };
            let rested = now.saturating_sub(since);
            if rested < self.auto.auto_idle {
                later(Duration::from_secs(self.auto.auto_idle - rested).max(left));
            } else if !left.is_zero() {
                later(left);
            } else if !shown.contains(&pane.id) {
                due.push((pane.member.clone(), context, since));
            }
        }
        (due, wait)
    }

    /// Whether `member`'s compaction, asked on its own, is under way: asked in the rest it is in, not answered (`done`,
    /// `skip`, `error` for its request's id), its program not back to another state than `working`, and not for more
    /// than [`COMPACTING`]. The `working` that Claude Code says for it is no turn: the member stays at rest.
    pub(super) fn compacting(&self, member: &str, since: Option<u64>) -> bool {
        let Some(asked) = self.auto.asked.get(member).filter(|asked| Some(asked.since) == since) else { return false };
        let answered =
            asked.id.as_ref().and_then(|id| self.auto.outcomes.get(member).filter(|outcome| outcome.id == *id));
        let answered =
            answered.is_some_and(|outcome| outcome.done || outcome.skip.is_some() || outcome.error.is_some());
        !asked.ended && !answered && asked.at.elapsed() < COMPACTING
    }

    /// The `working` still held back for a member whose compaction is over (no other state said since): told as a
    /// turn's. When the compaction was not made (refused, failed, no answer in time), since the `working` began. When
    /// it was, the program said no end of it (its `done` comes after the turn queued behind it, which begins right
    /// after the compaction: Claude Code 2.1.296, a message received meanwhile): the turn began when the mod answered,
    /// and the member stays at work until its next state. A compaction on its own says `done` at its end, before the
    /// mod answers: nothing is held then, and nothing changes here.
    pub(super) fn settle_compactions(&mut self) {
        for i in 0..self.panes.len() {
            let pane = &self.panes[i];
            if pane.held.is_none() || self.compacting(&pane.member, pane.since) {
                continue;
            }
            let answered = self.auto.outcomes.get(&pane.member).zip(self.auto.asked.get(&pane.member)).and_then(
                |(outcome, asked)| {
                    (asked.id.as_deref() == Some(outcome.id.as_str()) && outcome.done).then_some(outcome.at)
                },
            );
            if let Some((status, when)) = self.panes[i].held.take() {
                note_status(&mut self.panes[i], &status);
                let began = answered.and_then(|at| u64::try_from(at).ok()).filter(|at| *at > 0);
                self.panes[i].since = Some(began.unwrap_or(when));
            }
            self.dirty = true;
        }
    }

    /// Whether a member at rest since `since` may be asked to compact, and when: `None` when the rest is closed,
    /// else the wait before it may be asked (zero: now). Not asked yet in this rest: now. Asked: closed once the
    /// compaction is done; asked again, 5 minutes after, while a subagent or a teammate worked (as often as that
    /// lasts, those refusals count for nothing); a request that got no answer (lapsed, its mod not running), failed or
    /// was refused for another reason is asked once more, 5 minutes after, then the rest is closed. The answer to the
    /// last request is waited for [`AUTO_PENDING`]: the rest stays open, and watched, until it is read.
    pub(super) fn renewal(&self, member: &str, since: u64) -> Option<Duration> {
        let Some(asked) = self.auto.asked.get(member).filter(|asked| asked.since == since) else {
            return Some(Duration::ZERO);
        };
        let elapsed = asked.at.elapsed();
        let again = AUTO_AGAIN.saturating_sub(elapsed);
        let answer =
            asked.id.as_ref().and_then(|id| self.auto.outcomes.get(member).filter(|outcome| outcome.id == *id));
        let waiting = answer.is_none_or(|a| !a.done && a.skip.is_none() && a.error.is_none());
        match answer {
            Some(outcome) if outcome.done => None,
            Some(outcome) if not_a_failure(outcome) => Some(again),
            _ if waiting && elapsed < AUTO_PENDING => {
                Some(if asked.failures == 0 { again } else { AUTO_PENDING - elapsed })
            }
            // Failed, refused, or unanswered: the one more try is left to the first of them.
            _ => (asked.failures == 0).then_some(again),
        }
    }

    /// Whether the request of `asked` was refused for a subagent or a teammate at work.
    fn refused_for_helpers(&self, member: &str, asked: &Asked) -> bool {
        let answer =
            asked.id.as_ref().and_then(|id| self.auto.outcomes.get(member).filter(|outcome| outcome.id == *id));
        answer.is_some_and(not_a_failure)
    }

    /// Asks the members that [`Server::compaction_due`] names: logged, the request written off the loop
    /// ([`ask_compaction`]), its id told back to the loop.
    pub(super) fn auto_compaction(&mut self) {
        self.settle_compactions();
        for (member, percent, since) in self.plan_compaction(now()) {
            let (state, tx) = (self.state.clone(), self.tx.clone());
            let name = member.clone();
            let spawned = std::thread::Builder::new()
                .name("compaction".into())
                .spawn(move || ask_compaction(&state, &tx, &name, percent, since));
            if let Err(error) = spawned {
                log!("auto-compaction: {member}: {error}");
            }
        }
    }

    /// What [`Server::auto_compaction`] decides as of `now` (seconds since the epoch): the members to ask, with the
    /// context seen and the start of their rest, noted as asked (and logged); the next look due; the reports to read.
    pub(super) fn plan_compaction(&mut self, now: u64) -> Vec<(String, u8, u64)> {
        if self.auto.auto_compact.is_none() {
            self.auto.compaction = None;
            self.auto.asked.clear();
            self.auto.watched.lock().unwrap_or_else(PoisonError::into_inner).clear();
            return Vec::new();
        }
        let (due, mut wait) = self.compaction_due(now);
        // A compaction under way ends within [`COMPACTING`] at the latest: the `working` held back is told then.
        for pane in self.panes.iter().filter(|pane| pane.held.is_some() && self.compacting(&pane.member, pane.since)) {
            let left = self
                .auto
                .asked
                .get(&pane.member)
                .map_or(Duration::ZERO, |asked| COMPACTING.saturating_sub(asked.at.elapsed()));
            wait = Some(wait.map_or(left, |wait| wait.min(left)));
        }
        self.auto.compaction = wait.map(|wait| Instant::now() + wait);
        for (member, percent, since) in &due {
            let before = self.auto.asked.get(member).filter(|asked| asked.since == *since);
            let failures = before
                .map_or(0, |asked| asked.failures.saturating_add(u8::from(!self.refused_for_helpers(member, asked))));
            log!(
                "auto-compaction: {member} asked (after {failures} failed), context at {percent} %, at rest since {since}"
            );
            let asked = Asked { since: *since, id: None, at: Instant::now(), ended: false, failures };
            self.auto.asked.insert(member.clone(), asked);
        }
        // The outcomes of the requests still open are the reports to read.
        let open: Vec<String> = self
            .auto
            .asked
            .iter()
            .filter(|(member, asked)| {
                self.looks.contains_key(*member)
                    && (self.renewal(member, asked.since).is_some() || self.compacting(member, Some(asked.since)))
            })
            .map(|(member, _)| member.clone())
            .collect();
        let mut watched = self.auto.watched.lock().unwrap_or_else(PoisonError::into_inner);
        if *watched != open {
            *watched = open;
        }
        due
    }
}

/// The automatic compaction of the members at rest.
pub(super) struct Auto {
    /// The share of its window, in percent, from which a member at rest is compacted on its own; `None`: off, or not
    /// read yet (`watch_looks` tells it).
    pub(super) auto_compact: Option<u8>,
    /// How long a member rests before that, in seconds: [`crate::config::AUTO_COMPACT_IDLE`], or [`AUTO_IDLE`].
    pub(super) auto_idle: u64,
    /// The members asked to compact in their rest, by member (see [`Asked`]).
    pub(super) asked: BTreeMap<String, Asked>,
    /// How those requests went, as the members' reports last said it.
    pub(super) outcomes: BTreeMap<String, bridge::Compaction>,
    /// The members whose outcome `watch_looks` reads: those asked and not closed.
    pub(super) watched: Arc<Mutex<Vec<String>>>,
    /// When the automatic compaction next looks, for the rest that is not long enough yet; `None` when none waits.
    pub(super) compaction: Option<Instant>,
}

impl Auto {
    pub(super) fn new() -> Auto {
        Auto {
            auto_compact: None,
            auto_idle: std::env::var(AUTO_IDLE)
                .ok()
                .and_then(|secs| secs.parse().ok())
                .unwrap_or(crate::config::AUTO_COMPACT_IDLE),
            asked: BTreeMap::new(),
            outcomes: BTreeMap::new(),
            watched: Arc::new(Mutex::new(Vec::new())),
            compaction: None,
        }
    }
}
