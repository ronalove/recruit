// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A running team's panels, programs that run in panes of the multiplexer: the dashboard (who works, who rests, who
//! waits for the user, with each one's activity over the last half hour) over the journal (the messages the
//! members send each other). Both read what Claude Code already records: `claude agents --json` and the
//! conversation files; the mod adds the context and the account's usage.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use crossterm::style::Color;

use crate::claude::bridge::Helper;
use crate::i18n::Lang;
use crate::t;
use crate::team::state::Snapshot;
use crate::ui::look::{self, Glyphs, State};

mod agents;
mod arrange;
mod card;
mod clock;
mod dashboard;
#[cfg(test)]
mod fixture;
mod journal;
mod keyboard;
mod paint;
mod sessions;
mod states;
mod zones;

pub(crate) use card::{CardData, Clock, card_lines, card_rows};
pub(crate) use sessions::members_of;
#[cfg(test)]
pub use states::{Activity, FRESH, MemberState, Session};
pub(crate) use states::{MemberLook, context_percent, looks};
pub use states::{STATES, States, fresh_states, read_states};
pub use zones::{Clicked, ZONES, clicked};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    Dashboard,
    Journal,
}

pub fn run(kind: Kind, state: &Path) -> Result<()> {
    let snapshot = Snapshot::read(state)?;
    match kind {
        Kind::Dashboard => dashboard::dashboard(&snapshot, state),
        Kind::Journal => journal::journal(snapshot, state),
    }
}

pub fn dashboard_title(lang: Lang) -> String {
    t!(in lang; "Tableau de bord", "Dashboard")
}

pub fn journal_title() -> &'static str {
    "Journal"
}

/// How often the panels look again: `claude agents --json`, the mod's reports, the activity samples.
const TICK: Duration = Duration::from_secs(1);

fn member_color(s: &Snapshot, name: &str) -> Color {
    s.members.iter().position(|m| m.name == name).map_or(Color::Reset, look::member_color)
}

/// A member whose Claude runs, as its card shows it: what the card shows (`CardData`), with what only the dashboard
/// has of it.
#[derive(Debug, Clone)]
struct Card {
    data: CardData,
    contact: bool,
    /// The agents its session started: subagents, teammates.
    helpers: Vec<Helper>,
    /// The text of the error its turn failed on, as its pane gave it (`data.failed` has the short reason).
    error: Option<String>,
}

/// What the dashboard draws.
#[derive(Debug, Clone, Default)]
struct Board {
    now: i64,
    error: Option<String>,
    /// The account's windows: label, percent used, seconds before it resets.
    usage: Vec<(String, f64, Option<u64>)>,
    cards: Vec<Card>,
    absent: Vec<String>,
    /// The spinners' image.
    frame: usize,
    glyphs: Glyphs,
    /// Sessions open elsewhere under a member's name, with their folder.
    elsewhere: Vec<(String, Option<String>)>,
}

/// The team as it stands, `team.json` read again: members added, removed or renamed while it runs. As it was, when the
/// file cannot be read (written again at this very moment).
fn reread(s: &mut Snapshot, state: &Path) {
    if let Ok(now) = Snapshot::read(state) {
        *s = now;
    }
}

/// At rest, or in a state the dashboard does not know: the card fades.
fn resting(c: &CardData) -> bool {
    !matches!(c.state, State::Working | State::Waiting | State::Error)
}
