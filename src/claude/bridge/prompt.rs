// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What each conversation heard of its member's place in the team, and the note that tells it again.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::files::prompted;
use crate::i18n::Lang;
use crate::sys::write_atomic;
use crate::team::state::{MemberInfo, Snapshot};
use crate::{claude, t};

/// What a conversation heard of its member's place in the team: the prompt it started with, when known, and the
/// last one it was given, at its start or in a note. Both whole, as `prompt::build` writes them.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Prompted {
    /// Whose conversation it is: forgotten with the member, or once a new conversation replaces it.
    #[serde(default)]
    pub(super) member: Option<String>,
    #[serde(default)]
    pub(super) base: Option<String>,
    #[serde(default)]
    pub(super) seen: Option<String>,
}

/// A session id as a file name: an id as Claude Code makes them, nothing else.
pub(super) fn prompted_file(state: &Path, session: &str) -> Option<PathBuf> {
    let fit = !session.is_empty() && session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    fit.then(|| prompted(state).join(format!("{session}.json")))
}

fn read_prompted(state: &Path, session: &str) -> Option<Prompted> {
    serde_json::from_str(&fs::read_to_string(prompted_file(state, session)?).ok()?).ok()
}

pub(super) fn write_prompted(state: &Path, session: &str, value: &Prompted) -> Result<()> {
    let Some(file) = prompted_file(state, session) else { return Ok(()) };
    fs::create_dir_all(prompted(state))?;
    write_atomic(&file, &serde_json::to_string(value)?)
}

/// How long what a conversation heard is kept, unless it goes on: resumed later, it is told its place again.
const PROMPTED_DAYS: u64 = 30;

/// A member's session just started. A new conversation got its prompt on the command line: what it holds is known.
/// A resumed one keeps the prompt it started with, as noted then if it was, whatever the command line says (it names
/// the prompt a new conversation would get); a resumed one never noted (before this recruit, or long ago) is told its
/// place at its first prompt. Resumed, a conversation has its file already: a new one gets it at its first message.
pub(super) fn started(snapshot: &Snapshot, state: &Path, session: &str, member: &MemberInfo) -> Result<()> {
    forget_old_prompts(state);
    if claude::transcript(&snapshot.dir, snapshot.config_dir.as_deref(), session).exists() {
        return Ok(());
    }
    // A new conversation: the member's earlier ones are left, never to be resumed (recruit resumes the latest).
    for (other, _) in prompted_of(state, &member.name) {
        if let Some(file) = prompted_file(state, &other) {
            let _ = fs::remove_file(file);
        }
    }
    let Some(file) = claude::flag_value(&member.argv, "--append-system-prompt-file") else { return Ok(()) };
    let Ok(text) = fs::read_to_string(file) else { return Ok(()) };
    let prompted = Prompted { member: Some(member.name.clone()), base: Some(text.clone()), seen: Some(text) };
    write_prompted(state, session, &prompted)
}

/// The conversations noted as `member`'s, by session.
pub(super) fn prompted_of(state: &Path, member: &str) -> Vec<(String, Prompted)> {
    let Ok(entries) = fs::read_dir(prompted(state)) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let session = entry.file_name().to_string_lossy().strip_suffix(".json")?.to_string();
            let prompted = read_prompted(state, &session)?;
            (prompted.member.as_deref() == Some(member)).then_some((session, prompted))
        })
        .collect()
}

fn forget_old_prompts(state: &Path) {
    let Ok(entries) = fs::read_dir(prompted(state)) else { return };
    let limit = std::time::Duration::from_secs(PROMPTED_DAYS * 24 * 3600);
    for entry in entries.flatten() {
        let age = entry.metadata().and_then(|m| m.modified()).ok().and_then(|at| at.elapsed().ok());
        if age.is_some_and(|age| age > limit) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// For a prompt about to reach a member: the note to join to it, when the member's place in the team (`current`, its
/// prompt as recruit would write it now) is not what its conversation last heard. Told once: the next prompts go
/// bare until the team changes again, in the team's language, as the prompt it joins. A command is left alone, the
/// model may never read it.
pub fn submit(
    state: &Path,
    member: &str,
    session: &str,
    text: &str,
    lang: Lang,
    current: impl FnOnce() -> Result<String>,
) -> Result<Option<String>> {
    if text.trim_start().starts_with('/') || prompted_file(state, session).is_none() {
        return Ok(None);
    }
    let mut prompted = read_prompted(state, session).unwrap_or_default();
    prompted.member = Some(member.to_string());
    let current = current()?;
    if prompted.seen.as_deref() == Some(current.as_str()) {
        return Ok(None);
    }
    let note = format!(
        "{}\n\n{}",
        t!(in lang;
            "Note de recruit, que l'utilisateur ne voit pas : voici ta place dans l'équipe telle qu'elle est maintenant. Elle prime sur ce qu'en dit ton prompt système (ton nom, ton rôle, tes interlocuteurs, les membres, les règles communes), qui peut dater d'une composition plus ancienne de l'équipe.",
            "Note from recruit, which the user does not see: here is your place in the team as it stands now. It prevails over what your system prompt says of it (your name, your role, your contacts, the members, the shared rules), which may date from an earlier line-up of the team."
        ),
        current.trim_end()
    );
    prompted.seen = Some(current);
    write_prompted(state, session, &prompted)?;
    Ok(Some(note))
}

/// The conversation was compacted: the notes it was given may have gone with what it summed up, and only its starting
/// prompt is sure to remain. The next prompt tells its place again, unless that prompt says it all.
pub(super) fn compacted_away(state: &Path, session: &str) -> Result<()> {
    match read_prompted(state, session) {
        Some(prompted) => write_prompted(state, session, &Prompted { seen: prompted.base.clone(), ..prompted }),
        None => Ok(()),
    }
}
