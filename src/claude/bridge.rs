// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! recruit's side of the mod it puts in each member (`mod/`, a Claude Code plugin of function hooks). The mod
//! decides nothing: it calls `recruit _mod <event> <state> <member> [command]`, what the session knows as JSON on
//! the standard input, and does what the JSON answer says.
//!
//! - `start`: the slash commands to declare, `/equipe` (`/team` in English) and `/recruit`, and whether the member
//!   keeps its prompt alone, without the hint line under it (all but the main contact).
//! - `tick`, every two seconds: the session's model, context and usage, kept for the dashboard; the answer
//!   holds the model and effort the team's settings give the member now, which the mod puts on each of its
//!   requests, whether it keeps its prompt alone (the main contact may change), and what recruit asks of the
//!   session: a line on what the member does (a completion it asks the model for), a compaction the dashboard
//!   asked, « continue » the menu asked after a failed turn, the context's breakdown now and then. The mod hands the
//!   answers over at the next tick.
//! - `submit`, for each prompt: a note joined to it when the member's place in the team changed since its
//!   conversation last heard of it (see [`submit`]).
//! - `command`: what one of these commands does and prints.

mod command;
mod doing;
mod files;
mod helpers;
mod install;
mod models;
mod prompt;
mod report;
mod requests;
#[cfg(test)]
mod tests;
mod tick;

use std::io::Read;
use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use self::command::command_text;
use self::prompt::{compacted_away, submit};
use self::tick::{start, tick};
use crate::team::live;
use crate::team::state::Snapshot;

pub use self::doing::{Task, doing};
pub use self::files::{forget, rename, reset};
pub use self::helpers::{Helper, HelperStatus};
pub use self::install::install;
pub use self::models::{EFFORTS, MODELS, Model, applies_live, model_and_effort, model_and_effort_in, model_id};
pub use self::report::{Compaction, Report, report};
pub use self::requests::{
    Resumption, SKIP_BUSY, SKIP_HELPERS, request_auto_compaction, request_compaction, request_resume, resumption,
};
#[cfg(test)]
pub use self::{helpers::TEAMMATE_UNKNOWN, report::RateLimit};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Event {
    Start,
    Tick,
    Submit,
    Command,
}

pub fn run(event: Event, state: &Path, member: &str, command: Option<&str>) -> Result<()> {
    let snapshot = Snapshot::read(state)?;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let input: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let reply = match event {
        Event::Start => start(&snapshot, state, member, &input)?,
        Event::Tick => tick(&snapshot, state, member, &input)?,
        Event::Submit => {
            let session = input["session"].as_str().unwrap_or_default();
            let text = input["text"].as_str().unwrap_or_default();
            // Compacted just before: told with the prompt, the next tick would come after it.
            if input["compacted"] == true {
                compacted_away(state, session)?;
            }
            match submit(state, member, session, text, snapshot.lang, || live::prompt(state, member))? {
                Some(note) => json!({ "context": note }),
                None => json!({}),
            }
        }
        Event::Command => {
            json!({ "text": command_text(&snapshot, state, member, command.unwrap_or_default())? })
        }
    };
    println!("{reply}");
    Ok(())
}
