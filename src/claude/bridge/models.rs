// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The model and effort of a member's requests: what the team's settings give, and what the mod puts on them.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::report::{Report, report};
use crate::claude;
use crate::team::state::MemberInfo;

/// The model and effort of a member's requests.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Override {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// The efforts Claude Code knows, from the lightest to the heaviest: the values of the team's files.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// A model Claude Code offers, by family: how a team's settings name it, how recruit shows it, and the full id a
/// request names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// The name a team's settings give it (`opus`).
    pub alias: &'static str,
    /// The name recruit shows (`Opus`).
    pub label: &'static str,
    /// The id a request names: the mod has no way to resolve an alias.
    pub id: &'static str,
}

/// The models, in the order the menu lists them. To bring up to date when new models come out; a full `claude-…` id
/// a team's settings give is taken as it is.
pub const MODELS: [Model; 4] = [
    Model { alias: "opus", label: "Opus", id: "claude-opus-5-5" },
    Model { alias: "sonnet", label: "Sonnet", id: "claude-sonnet-5-5" },
    Model { alias: "haiku", label: "Haiku", id: "claude-haiku-5-5" },
    Model { alias: "fable", label: "Fable", id: "claude-fable-5-1" },
];

impl Model {
    /// The model a team's settings call `alias`.
    pub fn of_alias(alias: &str) -> Option<&'static Model> {
        MODELS.iter().find(|model| model.alias == alias)
    }

    /// The model whose family `name` says, wherever it puts it: an id, a Bedrock profile, an ARN.
    pub fn of_name(name: &str) -> Option<&'static Model> {
        let name = name.to_lowercase();
        MODELS.iter().find(|model| name.contains(model.alias))
    }
}

/// What the mod puts on each of a member's requests: the model and effort the team's settings give it now, where
/// they differ from what its Claude was launched with. A model as a full id, a request names no alias. Back to Claude
/// Code's default, or a model with no id known here (`opus[1m]`), waits for the member's next launch: there is no
/// telling a request to use the default.
pub fn override_of(member: &MemberInfo) -> Override {
    let launched = |flag: &str| claude::flag_value(&member.argv, flag);
    let model = member.model.as_deref().filter(|m| launched("--model") != Some(*m)).and_then(model_id);
    let effort = member.effort.as_deref().filter(|e| launched("--effort") != Some(*e) && EFFORTS.contains(e));
    Override { model, effort: effort.map(String::from) }
}

/// Whether a member whose Claude was launched with `argv` gets the model and effort its settings now give (`model`,
/// `effort`) without starting again: they are those it was launched with, or the mod puts them on each of its
/// requests. Not Claude Code's default once launched with another (a request cannot ask for the default), nor a model
/// with no id known here (`opus[1m]`), nor an unknown effort: the member must start again, on its conversation.
pub fn applies_live(model: Option<&str>, effort: Option<&str>, argv: &[String]) -> bool {
    let model = model == claude::flag_value(argv, "--model") || model.and_then(model_id).is_some();
    let effort = effort == claude::flag_value(argv, "--effort") || effort.is_some_and(|e| EFFORTS.contains(&e));
    model && effort
}

/// `opus` → `claude-opus-5-5`; a `claude-…` id as it is; anything else unknown.
pub fn model_id(model: &str) -> Option<String> {
    match Model::of_alias(model) {
        Some(model) => Some(model.id.to_string()),
        None => model.starts_with("claude-").then(|| model.to_string()),
    }
}

/// What a member's next requests use: the model and effort recruit puts on them, else the session's own, with the
/// effort its last request had on that model; or, before any (or without the mod), those its settings give.
pub fn model_and_effort(state: &Path, member: &MemberInfo) -> (Option<String>, Option<String>) {
    model_and_effort_in(&report(state, &member.name).unwrap_or_default(), member)
}

/// The same, from the member's report already read.
pub fn model_and_effort_in(report: &Report, member: &MemberInfo) -> (Option<String>, Option<String>) {
    let set = override_of(member);
    // Just after a change of model in the session (`/model`), the last request still had the one before.
    let same = report.own.model.is_none() || report.own.model == report.model;
    let configured = member.effort.clone().filter(|e| EFFORTS.contains(&e.as_str()));
    let effort = set.effort.or(report.own.effort.clone().filter(|_| same)).or(configured);
    // Without the mod (an older Claude Code), the one its settings give.
    (set.model.or(report.model.clone()).or(report.own.model.clone()).or(member.model.clone()), effort)
}
