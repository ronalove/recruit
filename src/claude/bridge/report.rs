// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What a member's session last told, kept for the dashboard.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::files::reports;
use super::helpers::Helper;
use super::models::Override;
use crate::ui::look::Pressure;

/// What a member's session last told: kept for the dashboard.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Report {
    /// The session's id.
    #[serde(default)]
    pub session: Option<String>,
    /// The session's own model.
    #[serde(default)]
    pub model: Option<String>,
    /// The model and effort of its last request, as the session made it, before recruit's.
    #[serde(default, alias = "last")]
    pub own: Override,
    #[serde(default)]
    pub context: Option<ContextUse>,
    #[serde(default, rename = "rateLimits")]
    pub rate_limits: Vec<RateLimit>,
    /// When, in seconds since the epoch.
    #[serde(default)]
    pub at: i64,
    /// When the session compacts on its own, as last measured: now and then only, kept in between.
    #[serde(default)]
    pub threshold: Option<Threshold>,
    /// The last compaction the dashboard asked, and how it went; kept until the next one.
    #[serde(default)]
    pub compaction: Option<Compaction>,
    /// The last compaction made (`done`), automatic or by hand (`auto` tells), kept apart: no refusal, no error and no
    /// later request touches it, until the next one is made. What the dashboard's trace reads; `compaction` stays the
    /// outcome of the last request. Its name in the file is not `compacted`: the mod's input carries a flag of that
    /// name, and the input is read as a report (a flag read as a compaction made every tick fail).
    #[serde(default, rename = "last_compacted", skip_serializing_if = "Option::is_none")]
    pub compacted: Option<Compaction>,
    /// When the breakdown was asked, while no answer came: asked again only after a while. 0 when none waits.
    #[serde(default)]
    pub breakdown_asked: i64,
    /// The agents the session started: its subagents at work, and for a while those just over; its teammates, as long
    /// as it lists them (see [`helpers`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub helpers: Vec<Helper>,
    /// Why the last tick's input could not be read, if it could not: the report is then the one before, and this
    /// says so. Gone with the next tick that reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_error: Option<String>,
}

impl Report {
    /// How many tokens of context make the session compact on its own: as measured, or, with auto-compaction off,
    /// where it would (the window, less what Claude Code keeps for the reply and its buffer). None before a
    /// measure.
    pub fn compacts_at(&self) -> Option<u64> {
        let threshold = self.threshold.as_ref()?;
        match threshold.tokens {
            Some(tokens) if threshold.enabled => Some(tokens),
            _ => threshold.window.map(|window| window.saturating_sub(REPLY_RESERVE + COMPACT_BUFFER)),
        }
    }

    /// How close the context is to the compaction.
    pub fn pressure(&self) -> Pressure {
        Pressure::of(self.context.as_ref().and_then(|c| c.tokens), self.compacts_at())
    }
}

/// Of the window, what Claude Code keeps for the model's reply (its output limit, at most this) and the margin
/// it compacts before: 2.1.292 compacts at the window less both.
const REPLY_RESERVE: u64 = 20_000;

const COMPACT_BUFFER: u64 = 13_000;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
/// How full the session's context is.
pub struct ContextUse {
    /// Of the model's window.
    #[serde(default)]
    pub percent: Option<f64>,
    #[serde(default)]
    pub tokens: Option<u64>,
    /// The model's window, in tokens.
    #[serde(default)]
    pub window: Option<u64>,
    /// Counted by the breakdown, not by the API: the session has not answered since it compacted (or started).
    #[serde(default)]
    pub estimated: bool,
}

/// Where the session compacts on its own (`/context`'s breakdown).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Threshold {
    /// Tokens of context at which it compacts; none with auto-compaction off.
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(default)]
    pub enabled: bool,
    /// The window it measures against: the model's, or a smaller one set for compaction.
    #[serde(default)]
    pub window: Option<u64>,
    /// The model it was measured for: measured again once the session changes model.
    #[serde(default)]
    pub model: Option<String>,
    /// The tokens of context the breakdown counted, an estimate.
    #[serde(default)]
    pub used: Option<u64>,
    #[serde(default)]
    pub at: i64,
}

/// A compaction asked from the dashboard, or by the server on its own: done, skipped by a hook, or refused (a turn
/// running).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compaction {
    pub id: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub skip: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub at: i64,
    /// Asked by the server, the member at rest for a while (`request_auto_compaction`), not by a click.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto: bool,
    /// Its context before, in percent of its window: as the server saw it for an automatic one, as the report knew it
    /// when the request was taken for one asked by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// Its context after, in percent of its window, as soon as the session tells (see [`COMPACTED_AFTER`]); only in
    /// [`Report::compacted`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u8>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    /// `five_hour`, `seven_day`.
    pub kind: String,
    #[serde(rename = "percentUsed")]
    pub percent_used: f64,
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<String>,
}

pub fn report(state: &Path, member: &str) -> Option<Report> {
    serde_json::from_str(&fs::read_to_string(reports(state).join(format!("{member}.json"))).ok()?).ok()
}
