// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The slash commands of the mod, and what each answers.

use std::path::Path;

use anyhow::{Result, bail};

use crate::i18n::Lang;
use crate::mux::Mux;
use crate::mux::plan::{self, Pane};
use crate::mux::proto::{JournalSize, MenuOpened};
use crate::team::state::Snapshot;
use crate::{board, t, team::live};

/// The journal's command, in the team's language.
pub(super) fn team_command(lang: Lang) -> String {
    t!(in lang; "equipe", "team")
}

/// The menu's command, the same in every language.
pub(super) const MENU_COMMAND: &str = "recruit";

pub(super) fn command_text(snapshot: &Snapshot, state: &Path, member: &str, command: &str) -> Result<String> {
    if command == team_command(snapshot.lang) {
        return Ok(journal_said(toggle_journal(snapshot, state)?));
    }
    if command != MENU_COMMAND {
        bail!("recruit _mod: {command}");
    }
    Ok(menu_said(live::open_menu(state, member)?))
}

/// Takes the journal to its next size: full, reduced, hidden, full again.
fn toggle_journal(s: &Snapshot, state: &Path) -> Result<JournalSize> {
    let journal = Pane {
        member: board::journal_title().to_string(),
        role: Some(plan::JOURNAL),
        argv: s.journal.clone(),
        env: Vec::new(),
    };
    Mux::at(state).toggle_journal(&s.dir, &journal)
}

/// What `/equipe` answers once the journal took its next size.
fn journal_said(size: JournalSize) -> String {
    match size {
        JournalSize::Full => t!("Journal complet.", "Journal at full size."),
        JournalSize::Reduced => t!("Journal réduit à ses derniers messages.", "Journal reduced to its last messages."),
        JournalSize::Hidden => t!("Journal masqué.", "Journal hidden."),
    }
}

/// What `/recruit` answers once the menu was asked for: the server says at once whether it opened it.
pub(super) fn menu_said(opened: MenuOpened) -> String {
    match opened {
        MenuOpened::AlreadyOpen => t!("Le menu de l'équipe est déjà ouvert.", "The team menu is already open."),
        MenuOpened::Opened => t!("Menu de l'équipe ouvert.", "Team menu opened."),
        MenuOpened::NoClient => t!(
            "Aucun terminal n'affiche l'équipe : `recruit attach` la rouvre, puis {}r ouvre le menu.",
            "No terminal shows the team: `recruit attach` opens it again, then {}r opens the menu.",
            crate::ui::look::ALT
        ),
    }
}
