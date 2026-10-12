// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The mod's `start` and `tick`: what the session tells, and what recruit asks of it.

use std::fs;
use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use super::command::{MENU_COMMAND, team_command};
use super::doing::follow;
use super::files::reports;
use super::helpers::{helpers_now, restarted, working};
use super::models::override_of;
use super::prompt::{compacted_away, started};
use super::report::{Compaction, ContextUse, Report, report};
use super::requests::{
    AUTO_COMPACT_INSTRUCTIONS, Asked, COMPACTED_AFTER, Resumption, SKIP_HELPERS, request_id, resumed, take_compaction,
    take_resume,
};
use crate::sys::write_atomic;
use crate::t;
use crate::team::state::Snapshot;

pub(super) fn start(snapshot: &Snapshot, state: &Path, member: &str, input: &Value) -> Result<Value> {
    let known: Vec<&str> = input["commands"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let specs = [
        json!({
            "name": team_command(snapshot.lang),
            "description": t!(
                "Fait passer le journal de complet à réduit, puis masqué",
                "Takes the journal from full to reduced, then hidden"
            ),
        }),
        json!({
            "name": MENU_COMMAND,
            "description": t!(
                "Menu de l'équipe : membres, modèles, rôles, relances",
                "The team's menu: members, models, roles, restarts"
            ),
        }),
    ];
    // A command of the same name (the user's, a plugin's) keeps its place.
    let register: Vec<Value> =
        specs.into_iter().filter(|s| !known.contains(&s["name"].as_str().unwrap_or_default())).collect();
    let info = snapshot.members.iter().find(|m| m.name == member);
    if let (Some(info), Some(session)) = (info, input["session"].as_str()) {
        // At best: the mod would not start at all otherwise.
        let _ = started(snapshot, state, session, info);
    }
    let _ = restarted(state, member, crate::sys::now());
    Ok(json!({ "register": register, "quiet": info.is_some_and(|m| m.quiet) }))
}

/// A tick's input that does not read, said once in the server's log (`server.log` of the team's state folder, where the
/// server writes its own): nothing else shows it, the mod's tick goes on.
fn unreadable(state: &Path, member: &str, error: &str) {
    use std::io::Write;
    let line = format!(
        "{} recruit[{}]: bridge: the tick of {member} does not read ({error})\n",
        crate::sys::now(),
        std::process::id()
    );
    if let Ok(mut log) = fs::OpenOptions::new().create(true).append(true).open(state.join("server.log")) {
        let _ = log.write_all(line.as_bytes());
    }
}

/// How often the context's breakdown is asked, in seconds: estimated by the session itself, but over all its
/// context.
const BREAKDOWN_EVERY: i64 = 300;

pub(super) fn tick(snapshot: &Snapshot, state: &Path, member: &str, input: &Value) -> Result<Value> {
    let now = crate::sys::now();
    let before = report(state, member).unwrap_or_default();
    let (mut report, readable) = read_report(state, member, input, &before);
    report.at = if readable { now } else { before.at };
    report.helpers = helpers_now(&before, input, now);
    // A compaction just made: one asked for (the mod's answer), or the person's own `/compact` (`typed`, that the mod
    // saw go by).
    let typed = input.get("typed").filter(|t| t.is_object());
    let compacted = input["compaction"]["done"] == true || typed.is_some();
    estimate_context(&mut report, &before, compacted);
    keep_older(&mut report, &before, input, now);
    let compact = take_request(&mut report, &before, state, member, now);
    trace_compaction(&mut report, &before, typed, compacted, input, now);
    let breakdown = ask_breakdown(&mut report, &before, input, compacted, now);
    fs::create_dir_all(reports(state))?;
    write_atomic(&reports(state).join(format!("{member}.json")), &serde_json::to_string(&report)?)?;

    if compacted || input["compacted"] == true {
        compacted_away(state, input["session"].as_str().unwrap_or_default())?;
    }
    let mut reply = reply_to(snapshot, member, compact);
    if breakdown {
        // Just compacted, counted with the token-count API (less than a second): the session's own estimate counts a
        // fifth too many. Else the estimate, for the threshold alone.
        reply["breakdown"] = json!(if compacted { "full" } else { "summary" });
    }
    if let Some(answer) = input.get("resumption").and_then(|r| serde_json::from_value::<Resumption>(r.clone()).ok()) {
        resumed(state, member, &Resumption { at: now, ..answer })?;
    }
    if let Some(id) = take_resume(state, member, now)? {
        reply["resume"] = json!(id);
    }
    if let Some(ask) = follow(snapshot, state, member, input)? {
        reply["summarize"] = ask;
    }
    Ok(reply)
}

/// The report a tick's input makes, and whether the input read. An input that does not read leaves the report as it
/// was, and says why in it, instead of starting an empty one: as it was also means as old as it was (its time, its
/// measure's and its compaction's), nothing in it is news.
fn read_report(state: &Path, member: &str, input: &Value, before: &Report) -> (Report, bool) {
    match serde_json::from_value::<Report>(input.clone()) {
        Ok(report) => (report, true),
        Err(error) => {
            if before.input_error.is_none() {
                unreadable(state, member, &error.to_string());
            }
            (Report { input_error: Some(error.to_string()), ..before.clone() }, false)
        }
    }
}

/// Just compacted, the session tells no context until it answers again: the breakdown's count stands in, asked at
/// once, until the session answers.
fn estimate_context(report: &mut Report, before: &Report, compacted: bool) {
    let counted = report.threshold.as_ref().and_then(|t| t.used);
    let measured = report.context.as_ref().is_some_and(|c| c.percent.is_some());
    if !measured && (compacted || before.context.as_ref().is_some_and(|c| c.estimated)) {
        let window = report.context.as_ref().and_then(|c| c.window).filter(|w| *w > 0);
        let waiting = ContextUse { window, estimated: true, ..Default::default() };
        report.context = Some(match (counted, window) {
            (Some(used), Some(window)) => ContextUse {
                percent: Some(used as f64 * 100.0 / window as f64),
                tokens: Some(used),
                window: Some(window),
                estimated: true,
            },
            _ if compacted => waiting,
            _ => before.context.clone().unwrap_or(waiting),
        });
    }
}

/// The threshold and the compaction are stamped when the input carries them; else they are the report's own, with its
/// own time.
fn keep_older(report: &mut Report, before: &Report, input: &Value, now: i64) {
    match &mut report.threshold {
        Some(threshold) if input["threshold"].is_object() => threshold.at = now,
        Some(_) => {}
        None => report.threshold = before.threshold.clone(),
    }
    match &mut report.compaction {
        Some(compaction) if input["compaction"].is_object() => compaction.at = now,
        Some(_) => {}
        None => report.compaction = before.compaction.clone(),
    }
}

/// The compaction asked for the member, taken: its id, whether the server asked, the share of the window its context
/// held. An automatic request is refused while an agent of the session works, the refusal told in the report as a
/// hook's veto would be; otherwise it is handed to the mod (the returned request) with what to keep.
fn take_request(
    report: &mut Report,
    before: &Report,
    state: &Path,
    member: &str,
    now: i64,
) -> Option<(String, bool, Option<u8>)> {
    let Asked { id, auto, percent } = take_compaction(state, member, now)?;
    // A request by hand says nothing of the context: the one the report knows now.
    let percent = percent.or_else(|| {
        let known = report.context.as_ref().and_then(|c| c.percent).or(before.context.as_ref()?.percent);
        known.map(|p| p.round().clamp(0.0, 100.0) as u8)
    });
    if auto && working(&report.helpers, now) {
        report.compaction =
            Some(Compaction { id, skip: Some(SKIP_HELPERS.to_string()), at: now, auto, percent, ..Default::default() });
        return None;
    }
    Some((id, auto, percent))
}

/// The last compaction made, kept apart from the last request (`Report::compacted`), and its context after.
fn trace_compaction(
    report: &mut Report,
    before: &Report,
    typed: Option<&Value>,
    compacted: bool,
    input: &Value,
    now: i64,
) {
    report.compacted = match &report.compaction {
        Some(c) if input["compaction"]["done"] == true && c.done => Some(c.clone()),
        _ => before.compacted.clone(),
    };
    // A `/compact` typed in the panel is traced as one by hand: the context before is the last figure of the report
    // (the header's own), else the conversation's size the compaction recorded against the window.
    if let Some(typed) = typed {
        let window = [report.context.as_ref(), before.context.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|c| c.window)
            .filter(|w| *w > 0);
        let known = before
            .context
            .as_ref()
            .and_then(|c| c.percent)
            .or_else(|| Some(typed["tokensBefore"].as_u64()? as f64 * 100.0 / window? as f64));
        report.compacted = Some(Compaction {
            id: format!("typed-{}", request_id()),
            done: true,
            at: now,
            percent: known.map(|p| p.round().clamp(0.0, 100.0) as u8),
            ..Default::default()
        });
    }
    // Its context after: the first figure the session gives once it answers again, within a minute (later, the
    // conversation has grown).
    if !compacted {
        let after = report.context.as_ref().and_then(|c| c.percent).map(|p| p.round().clamp(0.0, 100.0) as u8);
        if let Some(c) = report.compacted.as_mut().filter(|c| c.after.is_none() && now - c.at <= COMPACTED_AFTER) {
            c.after = after;
        }
    }
}

/// Whether the context's breakdown is asked this time, noted in the report.
fn ask_breakdown(report: &mut Report, before: &Report, input: &Value, compacted: bool, now: i64) -> bool {
    let due = report.threshold.as_ref().is_none_or(|t| {
        now - t.at >= BREAKDOWN_EVERY || (t.model.is_some() && report.model.is_some() && t.model != report.model)
    });
    // A session that does not answer it (an older Claude Code) is not asked at each tick.
    let measured_now = input["threshold"].is_object();
    let waiting = Some(before.breakdown_asked).filter(|at| *at > 0 && !measured_now);
    let breakdown = compacted || (due && waiting.is_none_or(|at| now - at >= BREAKDOWN_EVERY));
    report.breakdown_asked = if breakdown { now } else { waiting.unwrap_or(0) };
    breakdown
}

/// What the tick answers: the model and effort the settings give, whether the member keeps its prompt alone, and the
/// compaction to make, if any.
fn reply_to(snapshot: &Snapshot, member: &str, compact: Option<(String, bool, Option<u8>)>) -> Value {
    let info = snapshot.member(member);
    let quiet = info.is_some_and(|m| m.quiet);
    let mut reply = json!({ "override": info.map(override_of).unwrap_or_default(), "quiet": quiet });
    if let Some((id, auto, percent)) = compact {
        reply["compact"] = json!(id);
        // What the mod tells back with its answer (`auto`, `percent`), and, for an automatic one, what to keep.
        let mut ask = serde_json::Map::new();
        if auto {
            ask.insert("auto".into(), json!(true));
            ask.insert("instructions".into(), json!(AUTO_COMPACT_INSTRUCTIONS));
        }
        if let Some(percent) = percent {
            ask.insert("percent".into(), json!(percent));
        }
        if !ask.is_empty() {
            reply["compactAsk"] = Value::Object(ask);
        }
    }
    reply
}
