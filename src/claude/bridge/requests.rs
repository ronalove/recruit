// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What recruit asks of a member's session through its mod: a compaction, « Reprendre »; and how each went.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::files::{compactions, resumes};
use crate::sys::write_atomic;
use crate::{board, t, ui::look};

/// Asks a member's session to compact, from the dashboard: its mod takes the request at its next tick, within
/// ten seconds, or the request lapses. Tried once: a session at work refuses it.
pub fn request_compaction(state: &Path, member: &str) -> Result<()> {
    write_request(&compactions(state), member, json!({})).map(drop)
}

/// Asks a member's session to compact on its own, from the server: at rest for
/// `config::AUTO_COMPACT_IDLE` seconds, its context at `percent` of its window, at or over the team's
/// `auto_compact`. As [`request_compaction`], and besides: skipped while a subagent or a teammate of the session
/// works, the compaction told to keep the details of the work last done, and the compaction that follows marked
/// `auto`, for the dashboard. Returns the request's id: the member's report tells how it went
/// (`Compaction.id`), or nothing if the request lapsed.
pub fn request_auto_compaction(state: &Path, member: &str, percent: u8) -> Result<String> {
    write_request(&compactions(state), member, json!({ "auto": true, "percent": percent }))
}

/// Asks a member's session to take up its work after a failed turn (« Reprendre »): its mod
/// sends « continue » at its next tick, unless the prompt box holds a draft or the member works; the request lapses as
/// a compaction's. How it went: [`resumption`].
pub fn request_resume(state: &Path, member: &str) -> Result<()> {
    write_request(&resumes(state), member, json!({})).map(drop)
}

/// Writes a request for `member` in `dir` (`compact/`, `resume/`): a new id, good for [`COMPACTION_LAPSE`], and the
/// fields of `more`. Its id.
fn write_request(dir: &Path, member: &str, more: Value) -> Result<String> {
    let id = request_id();
    let mut request = json!({ "id": id, "until": crate::sys::now() + COMPACTION_LAPSE });
    if let (Some(request), Value::Object(more)) = (request.as_object_mut(), more) {
        request.extend(more);
    }
    fs::create_dir_all(dir)?;
    write_atomic(&dir.join(member), &request.to_string())?;
    Ok(id)
}

/// How the last « Reprendre » asked for a member went, once its mod answered, or once it lapsed without an answer;
/// none before, nor before any.
pub fn resumption(state: &Path, member: &str) -> Option<Resumption> {
    resumption_at(state, member, crate::sys::now())
}

/// What a member's mod did with « Reprendre ».
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resumption {
    pub id: String,
    /// « continue » sent.
    #[serde(default)]
    pub sent: bool,
    /// Not sent: the prompt box held a draft.
    #[serde(default)]
    pub draft: bool,
    /// Not sent, why, as a sentence to show ([`resumption`] says the known reasons in the user's language): the member
    /// busy (at work, or waiting for an answer), a Claude Code without the calls, no answer in time, or what the
    /// session said.
    #[serde(default)]
    pub error: Option<String>,
    /// When the mod answered, in seconds since the epoch; for a request that lapsed, when it lapsed.
    #[serde(default)]
    pub at: i64,
}

/// The reasons the mod and the tick give for a « continue » not sent, by code: said in the user's language when read.
const RESUME_BUSY: &str = "busy";

const RESUME_UNSUPPORTED: &str = "unsupported";

/// How long « Reprendre », once handed to the mod, waits for its answer past the request's lapse, in seconds: the
/// mod answers at the tick after the prompt entered, a few seconds at most.
pub(super) const RESUME_ANSWER: i64 = 20;

/// A request to a member's mod: a compaction, « Reprendre ».
#[derive(Deserialize)]
struct Request {
    id: String,
    until: i64,
}

fn read_request(file: &Path) -> Option<Request> {
    serde_json::from_str(&fs::read_to_string(file).ok()?).ok()
}

/// A request's id: unique enough for one member's requests, made a second apart at least by a person.
pub(super) fn request_id() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis()).to_string()
}

/// « Reprendre » asked for a member, taken: its id, unless the dashboard sees the member at work or waiting for an
/// answer, where « continue » would land in the middle of a turn, or wait for its end: the request is then answered as
/// refused. Not seen (no dashboard looking: `dashboard = false`, or stopped), handed over all the same: the mod's own
/// guard refuses it while a turn runs, a question to answer included. Lapsed, it is left for [`resumption`] to tell.
pub(super) fn take_resume(state: &Path, member: &str, now: i64) -> Result<Option<String>> {
    let file = resumes(state).join(member);
    if read_request(&file).is_none_or(|r| r.until < now) {
        return Ok(None);
    }
    let seen = board::fresh_states(state).and_then(|s| s.members.get(member).map(|m| m.state));
    // Moved before it is read: one asked meanwhile is the one taken, never lost nor taken twice.
    let taken = resumes(state).join(format!("{member}.taken"));
    if fs::rename(&file, &taken).is_err() {
        return Ok(None);
    }
    let Some(request) = read_request(&taken) else { return Ok(None) };
    if !matches!(seen, Some(look::State::Working | look::State::Waiting)) {
        return Ok(Some(request.id));
    }
    let refused = Resumption { id: request.id, error: Some(RESUME_BUSY.into()), at: now, ..Default::default() };
    resumed(state, member, &refused)?;
    Ok(None)
}

/// What the mod did with « Reprendre », or why the tick did not hand it over: kept until the next one.
pub(super) fn resumed(state: &Path, member: &str, resumption: &Resumption) -> Result<()> {
    fs::create_dir_all(resumes(state))?;
    write_atomic(&resumes(state).join(format!("{member}.json")), &serde_json::to_string(resumption)?)
}

pub(super) fn resumption_at(state: &Path, member: &str, now: i64) -> Option<Resumption> {
    let lapsed = |request: Request, at: i64| Resumption {
        id: request.id,
        error: Some(t!("pas de réponse de sa session", "no answer from its session")),
        at,
        ..Default::default()
    };
    let dir = resumes(state);
    // Asked and not taken yet: under way, or lapsed.
    if let Some(request) = read_request(&dir.join(member)) {
        let until = request.until;
        return (until < now).then(|| lapsed(request, until));
    }
    // Taken: the answer to that request, not to one before.
    let taken = read_request(&dir.join(format!("{member}.taken")))?;
    let answer = fs::read_to_string(dir.join(format!("{member}.json"))).ok();
    let answer = answer.and_then(|text| serde_json::from_str::<Resumption>(&text).ok()).filter(|a| a.id == taken.id);
    let Some(mut answer) = answer else {
        let until = taken.until + RESUME_ANSWER;
        return (until < now).then(|| lapsed(taken, until));
    };
    answer.error = answer.error.map(|error| match error.as_str() {
        RESUME_BUSY => t!("le membre est occupé", "the member is busy"),
        RESUME_UNSUPPORTED => t!(
            "cette version de Claude Code ne permet pas d'envoyer « continue »",
            "this version of Claude Code does not let recruit send \"continue\""
        ),
        _ => error,
    });
    Some(answer)
}

/// How long after a compaction its context after is still taken from the session, in seconds.
pub(super) const COMPACTED_AFTER: i64 = 60;

/// How long a compaction request waits for the member's mod, in seconds.
pub(super) const COMPACTION_LAPSE: i64 = 10;

/// What a compaction asked for a member says: its id, and, for the server's own (`request_auto_compaction`), the share
/// of the window its context held.
pub(super) struct Asked {
    pub(super) id: String,
    pub(super) auto: bool,
    pub(super) percent: Option<u8>,
}

/// The compaction asked for a member, taken: unless it lapsed.
pub(super) fn take_compaction(state: &Path, member: &str, now: i64) -> Option<Asked> {
    let file = compactions(state).join(member);
    let request: Value = serde_json::from_str(&fs::read_to_string(&file).ok()?).ok()?;
    let _ = fs::remove_file(&file);
    let id = request["id"].as_str()?.to_string();
    if request["until"].as_i64()? < now {
        return None;
    }
    let percent = request["percent"].as_u64().and_then(|p| u8::try_from(p).ok());
    Some(Asked { id, auto: request["auto"] == true, percent })
}

/// Why an automatic compaction is not made: an agent of the session works (see [`working`]). Said in a
/// [`Compaction`]'s `skip`, as a hook's own reason would be.
pub const SKIP_HELPERS: &str = "helpers";

/// Why a compaction is not made when the mod answers it: a turn runs (the member went back to work). Not a failure.
pub const SKIP_BUSY: &str = "busy";

/// What an automatic compaction tells the summarizer to keep (the model reads it, the user does not): the member is
/// idle, and carries on from the summary alone when its next message comes.
pub(super) const AUTO_COMPACT_INSTRUCTIONS: &str = "\
This is an automatic compaction: the session was idle, and its context grew large. The member is idle, waiting for \
its next message (from the user or a teammate); when it comes, it carries on from this summary alone. Say what it \
last handed back, to whom, and whether it was waiting on someone. Keep in detail what the last stretch of work \
holds, and drop what is settled and needs no more than a line:
- the files touched and why (paths, functions, what changed), and the commands that mattered with their results;
- the choices made and the reasons given, and what was ruled out;
- the open points: what is half done, what fails, what was left for later, what was not checked;
- who asked for what (the user, a teammate), the exact request, the answer promised and to whom, and any \
instruction or constraint given on the way that still holds.
Do not tidy these into generalities: keep names, paths, identifiers and figures as they were.";
