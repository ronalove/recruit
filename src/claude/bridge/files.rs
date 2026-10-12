// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A member's files in the team's state folder: where each is, and what follows a member renamed or removed.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::prompt::{Prompted, prompted_file, prompted_of, write_prompted};
use crate::board;

pub(super) fn reports(state: &Path) -> PathBuf {
    state.join("members")
}

pub(super) fn doings(state: &Path) -> PathBuf {
    state.join("doing")
}

pub(super) fn compactions(state: &Path) -> PathBuf {
    state.join("compact")
}

/// « Reprendre » for each member: the request (`<member>`), the one taken (`<member>.taken`), the mod's answer
/// (`<member>.json`).
pub(super) fn resumes(state: &Path) -> PathBuf {
    state.join("resume")
}

/// What each conversation heard of its member's place in the team, by session: kept from one launch to the next, a
/// conversation may be resumed.
pub(super) fn prompted(state: &Path) -> PathBuf {
    state.join("prompted")
}

/// A member's own files, by name: its report, the line on what it does, the compaction and « Reprendre » asked for
/// it.
fn member_files(state: &Path, member: &str) -> [PathBuf; 6] {
    [
        reports(state).join(format!("{member}.json")),
        doings(state).join(format!("{member}.json")),
        compactions(state).join(member),
        resumes(state).join(member),
        resumes(state).join(format!("{member}.taken")),
        resumes(state).join(format!("{member}.json")),
    ]
}

/// A member renamed: its files follow it.
pub fn rename(state: &Path, from: &str, to: &str) -> Result<()> {
    for (old, new) in member_files(state, from).into_iter().zip(member_files(state, to)) {
        if old.exists() {
            fs::rename(&old, &new).with_context(|| old.display().to_string())?;
        }
    }
    for (session, prompted) in prompted_of(state, from) {
        write_prompted(state, &session, &Prompted { member: Some(to.to_string()), ..prompted })?;
    }
    Ok(())
}

/// A member removed: its files go with it.
pub fn forget(state: &Path, member: &str) -> Result<()> {
    let conversations =
        prompted_of(state, member).into_iter().filter_map(|(session, _)| prompted_file(state, &session));
    for file in member_files(state, member).into_iter().chain(conversations) {
        if file.exists() {
            fs::remove_file(&file).with_context(|| file.display().to_string())?;
        }
    }
    Ok(())
}

/// A new launch starts from the settings: no report, no session noted, no line on what a member does, no compaction
/// nor « Reprendre » asked, no zone of the dashboard, left from the last one.
pub fn reset(state: &Path) -> Result<()> {
    for dir in [reports(state), state.join("sessions"), doings(state), compactions(state), resumes(state)] {
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| dir.display().to_string())?;
        }
    }
    let zones = state.join(board::ZONES);
    if zones.exists() {
        fs::remove_file(&zones).with_context(|| zones.display().to_string())?;
    }
    Ok(())
}
