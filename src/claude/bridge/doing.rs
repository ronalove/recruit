// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The line on what a member does, from the requests that open its turns.

use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::files::doings;
use crate::sys::write_atomic;
use crate::team::state::Snapshot;
use crate::{claude, t};

/// What a member does, in a line, from the request that opened its turn: and how far its conversation was read.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Doing {
    session: String,
    offset: u64,
    /// The last request that opened a turn: its prompt id and its text.
    prompt: Option<String>,
    request: String,
    /// The request the model was asked about, once: answered or not, not asked again.
    asked: Option<String>,
    task: Option<Task>,
}

/// What a member does, as the model put the request that opened its last turn: in progress (« Je publie la version
/// 0.6.2 »), and once done (« Version 0.6.2 publiée »), when the model gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub now: String,
    #[serde(default)]
    pub done: Option<String>,
}

pub fn doing(state: &Path, member: &str) -> Option<Task> {
    read_doing(state, member).task
}

fn read_doing(state: &Path, member: &str) -> Doing {
    fs::read_to_string(doings(state).join(format!("{member}.json")))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The model the line on what a member does is asked of, through the member's own session.
pub(super) const SUMMARY_MODEL: &str = "haiku";

/// Of a request, what the model reads.
pub(super) const REQUEST_CHARS: usize = 2000;

/// A line on what a member does, at most: the model is asked for 60 characters.
pub(super) const LINE_CHARS: usize = 80;

/// Reads on in the member's conversation for the requests that open its turns, typed by the user or sent by a
/// teammate; takes the model's answer about the last one; and, when a new one came, what to ask the model.
pub(super) fn follow(snapshot: &Snapshot, state: &Path, member: &str, input: &Value) -> Result<Option<Value>> {
    let mut doing = read_doing(state, member);
    let before = doing.clone();
    let answer = &input["summary"];
    if answer["id"].as_str().is_some_and(|id| doing.asked.as_deref() == Some(id))
        && let Some(task) = answer["text"].as_str().and_then(task_of)
    {
        doing.task = Some(task);
    }
    if let Some(session) = input["session"].as_str().filter(|s| !s.is_empty()) {
        if session != doing.session {
            doing = Doing { session: session.to_string(), task: doing.task, ..Default::default() };
        }
        let file = claude::transcript(&snapshot.dir, snapshot.config_dir.as_deref(), session);
        for line in turn_lines(&file, &mut doing.offset) {
            if let Some((id, text)) = turn_request(&line) {
                doing.prompt = Some(id);
                doing.request = text;
            }
        }
    }
    let ask = match &doing.prompt {
        Some(prompt) if doing.asked.as_ref() != Some(prompt) => {
            doing.asked = Some(prompt.clone());
            Some(summary_ask(prompt, doing.task.as_ref(), &doing.request))
        }
        _ => None,
    };
    if doing != before {
        fs::create_dir_all(doings(state))?;
        write_atomic(&doings(state).join(format!("{member}.json")), &serde_json::to_string(&doing)?)?;
    }
    Ok(ask)
}

/// The whole lines `file` gained past `offset` that may open a turn; `offset` moves past every whole line, from the
/// start again when the file shrank. Read line by line: a resumed conversation may weigh megabytes.
pub(super) fn turn_lines(file: &Path, offset: &mut u64) -> Vec<String> {
    let Ok(mut handle) = fs::File::open(file) else { return Vec::new() };
    if handle.metadata().map_or(0, |m| m.len()) < *offset {
        *offset = 0;
    }
    if handle.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut reader = BufReader::new(handle);
    let mut found = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(read) if read > 0 && line.ends_with(b"\n") => *offset += read as u64,
            // The end, or a line still being written.
            _ => break,
        }
        let text = String::from_utf8_lossy(&line);
        if text.contains("\"turnOrigin\"") {
            found.push(text.trim_end().to_string());
        }
    }
    found
}

/// The request a line of a conversation opens a turn with, typed by the user or sent by a teammate: its prompt id
/// and its text, the teammate's message without its envelope.
pub(super) fn turn_request(line: &str) -> Option<(String, String)> {
    let row: Value = serde_json::from_str(line).ok()?;
    let origin = row["turnOrigin"].as_str()?;
    if row["type"] != "user" || !["human", "peer"].contains(&origin) {
        return None;
    }
    let id = row["promptId"].as_str()?.to_string();
    let content = &row["message"]["content"];
    let text = match row["origin"]["body"].as_str() {
        Some(body) if origin == "peer" => body.to_string(),
        _ => match content {
            Value::String(text) => text.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        },
    };
    let text = text.trim();
    (!text.is_empty()).then(|| (id, text.chars().take(REQUEST_CHARS).collect()))
}

/// What to ask the model about a request, in the team's language: what it asks to do, in progress and once done; or
/// the lines from before when it asks nothing.
fn summary_ask(id: &str, before: Option<&Task>, request: &str) -> Value {
    let system = t!(
        "Tu écris l'état d'un agent : ce que le message qu'il vient de recevoir lui demande de faire. Réponds par deux lignes en français, de 60 caractères au plus, sans guillemets, sans étiquette ni point final : d'abord la tâche en cours, à la première personne et au présent ; puis la même tâche une fois finie, au participe passé, sans sujet. Exemple :\nJe publie la version 0.6.2\nVersion 0.6.2 publiée",
        "You write an agent's status: what the message it just received asks it to do. Answer with two lines in English, of 60 characters at most, without quotes, labels or a final period: first the task in progress, in the first person and the present tense; then the same task once done, as a past participle, with no subject. Example:\nI am releasing version 0.6.2\nVersion 0.6.2 released"
    );
    let before = match before {
        Some(task) => format!("{}\n{}", task.now, task.done.as_deref().unwrap_or(&task.now)),
        None => t!("aucune", "none"),
    };
    let prompt = t!(
        "Message reçu :\n{}\n\nLignes actuelles :\n{}\n\nSi ce message demande un travail, écris les deux nouvelles lignes. S'il n'en demande aucun (un remerciement, un accord, une information sans demande), recopie les lignes actuelles. Réponds par les deux lignes seules.",
        "Message received:\n{}\n\nCurrent lines:\n{}\n\nIf this message asks for some work, write the two new lines. If it asks for none (thanks, an agreement, information with no request), copy the current lines. Answer with the two lines alone.",
        request,
        before
    );
    json!({ "id": id, "model": SUMMARY_MODEL, "system": system, "prompt": prompt, "maxTokens": 120 })
}

/// The model's answer as a task: its first line in progress, its second once done; the second may be missing.
pub(super) fn task_of(text: &str) -> Option<Task> {
    let mut lines = text.lines().filter_map(clean_line);
    Some(Task { now: lines.next()?, done: lines.next() })
}

/// What the model may put before a line despite being asked not to.
const LABELS: [&str; 9] =
    ["en cours", "fini", "finie", "terminé", "terminée", "in progress", "ongoing", "done", "finished"];

/// A line of the model's answer, bare: no bullet, label or quotes around it, no final period, not too long; and no
/// control character, which the dashboard would print (an escape sequence asked for in a message, a clipboard write).
fn clean_line(line: &str) -> Option<String> {
    let line: String = line.chars().filter(|c| !c.is_control()).collect();
    let mut line = line.trim().trim_start_matches(['-', '•', '*']).trim_start();
    if let Some((label, rest)) = line.split_once(':')
        && LABELS.contains(&label.trim().to_lowercase().as_str())
    {
        line = rest;
    }
    let line = line.trim_matches(|c: char| "\"'«»“”".contains(c) || c.is_whitespace());
    let line = line.strip_suffix('.').unwrap_or(line).trim_end();
    (!line.is_empty()).then(|| line.chars().take(LINE_CHARS).collect())
}
