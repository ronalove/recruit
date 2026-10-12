// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The agents a member's session started: its subagents at work, and its teammates.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::files::reports;
use super::report::{Report, report};
use crate::sys::write_atomic;

/// An agent a member's session started, as its mod last listed it (`$.agent.list()`): a subagent, or a teammate of
/// a team of Claude Code's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Helper {
    pub id: String,
    /// Its type: `Explore`, `general-purpose`…; a teammate's may be its role.
    #[serde(rename = "type")]
    pub kind: String,
    /// The few words of the call that started it.
    pub description: String,
    /// A teammate's name in its team; none for a subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teammate: Option<String>,
    pub status: HelperStatus,
    /// When it was first seen, and since when in this status, in seconds since the epoch.
    pub started: i64,
    pub since: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperStatus {
    /// Started or at work; for a subagent, held or between two turns too.
    Running,
    /// A teammate waiting for a message, or held.
    Idle,
    Done,
    /// Failed, or stopped.
    Failed,
}

/// How long a subagent stays on the dashboard once done, and once failed or stopped, in seconds.
pub const SUBAGENT_DONE_SHOWN: i64 = 10;

pub const SUBAGENT_FAILED_SHOWN: i64 = 30;

/// A teammate in the same status this long may be a ghost: one in a pane of its own keeps the word it last wrote
/// when that pane dies. Shown as unknown after the first, dropped after the second.
pub const TEAMMATE_UNKNOWN: i64 = 30 * 60;

pub const TEAMMATE_DROPPED: i64 = 2 * 60 * 60;

impl Helper {
    /// Still shown at `now`: a subagent at work, or over since less than a while, longer for one that failed; a
    /// teammate unless it stayed in its status too long.
    pub fn shown(&self, now: i64) -> bool {
        let over = now - self.since;
        match (self.teammate.is_some(), self.status) {
            (true, _) => over < TEAMMATE_DROPPED,
            (false, HelperStatus::Done) => over < SUBAGENT_DONE_SHOWN,
            (false, HelperStatus::Failed) => over < SUBAGENT_FAILED_SHOWN,
            (false, _) => true,
        }
    }

    /// A teammate that may be a ghost (see [`TEAMMATE_UNKNOWN`]).
    pub fn unknown(&self, now: i64) -> bool {
        self.teammate.is_some() && now - self.since >= TEAMMATE_UNKNOWN
    }
}

/// The agents a member's session started, from a tick's `input` and the report the last one left (`before`). Its
/// `agents`: a list (see [`helpers`]); `null`, the session could not list them this time, and they stay as they were;
/// none at all, an older mod, which never lists them: none. A session other than the last one (the member started
/// again on another conversation) left them behind (see [`left_behind`]); started again on the same one, its mod's
/// start did (see [`start`]).
pub(super) fn helpers_now(before: &Report, input: &Value, now: i64) -> Vec<Helper> {
    let session = input["session"].as_str();
    let changed = before.session.is_some() && before.session.as_deref() != session;
    let kept = if changed { left_behind(&before.helpers, now) } else { before.helpers.clone() };
    match input.get("agents") {
        None => Vec::new(),
        Some(Value::Array(agents)) => helpers(&kept, agents, now),
        Some(_) => kept,
    }
}

/// The agents a session's process left behind when it ended, the member started again on its conversation or on
/// another: its subagents at work or held, stopped with it; its teammates, gone with it; those over, as they were.
fn left_behind(helpers: &[Helper], now: i64) -> Vec<Helper> {
    helpers
        .iter()
        .filter(|h| h.teammate.is_none())
        .map(|h| match h.status {
            HelperStatus::Running | HelperStatus::Idle => {
                Helper { status: HelperStatus::Failed, since: now, ..h.clone() }
            }
            _ => h.clone(),
        })
        .collect()
}

/// A member's process starts again: the agents the last one started, in its report, are left behind (see
/// [`left_behind`]). Its conversation may be the same, so its session too: a tick alone could not tell.
pub(super) fn restarted(state: &Path, member: &str, now: i64) -> Result<()> {
    let Some(mut report) = report(state, member).filter(|r| !r.helpers.is_empty()) else { return Ok(()) };
    report.helpers = left_behind(&report.helpers, now);
    write_atomic(&reports(state).join(format!("{member}.json")), &serde_json::to_string(&report)?)
}

/// The agents a member's session started now, from what its mod lists (`agents`, `$.agent.list()` as is) and what the
/// last tick kept (`before`): each with the moment it was first seen and the moment its status was. A teammate has a
/// `teammateId`, and leaves when the session no longer lists it; a subagent no longer listed is over (the session
/// drops it a while after its end), and shows a while once over.
pub fn helpers(before: &[Helper], agents: &[Value], now: i64) -> Vec<Helper> {
    let text = |agent: &Value, key: &str| agent[key].as_str().unwrap_or_default().to_string();
    let mut list: Vec<Helper> = agents
        .iter()
        .filter_map(|agent| {
            let id = agent["id"].as_str().filter(|id| !id.is_empty())?;
            let teammate = agent["teammateId"].as_str().filter(|t| !t.is_empty()).map(|address| {
                // `<name>@<team>`, when it has no name of its own.
                agent["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(address.split('@').next().unwrap_or(address))
            });
            let status = match (agent["status"].as_str().unwrap_or_default(), teammate.is_some()) {
                ("completed", _) => HelperStatus::Done,
                ("failed" | "killed", _) => HelperStatus::Failed,
                ("idle" | "waiting", true) => HelperStatus::Idle,
                _ => HelperStatus::Running,
            };
            let was = before.iter().find(|h| h.id == id);
            Some(Helper {
                id: id.to_string(),
                kind: text(agent, "type"),
                description: text(agent, "description"),
                teammate: teammate.map(String::from),
                status,
                started: was.map_or(now, |h| h.started),
                since: was.filter(|h| h.status == status).map_or(now, |h| h.since),
            })
        })
        .collect();
    let over: Vec<Helper> = before
        .iter()
        .filter(|h| h.teammate.is_none() && !list.iter().any(|listed| listed.id == h.id))
        .cloned()
        .map(|h| match h.status {
            HelperStatus::Running | HelperStatus::Idle => Helper { status: HelperStatus::Done, since: now, ..h },
            _ => h,
        })
        .collect();
    list.extend(over);
    list.retain(|h| h.shown(now));
    list.sort_by(|a, b| (a.started, &a.id).cmp(&(b.started, &b.id)));
    list
}

/// A subagent or a teammate of the session works, as its mod last listed them: the summary a compaction makes would
/// leave out what they are about to hand back. A teammate that stayed `Running` for [`TEAMMATE_UNKNOWN`] (30 minutes)
/// is taken for a ghost (see [`Helper::unknown`]) and no longer blocks.
pub(super) fn working(helpers: &[Helper], now: i64) -> bool {
    helpers.iter().any(|h| h.status == HelperStatus::Running && !h.unknown(now))
}
