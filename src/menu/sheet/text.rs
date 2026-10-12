// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The sentences and short texts the sheet is made of.

use crate::config::{self, Origin, Source};
use crate::ui::canvas::fit;
use crate::{claude::bridge, i18n, t};

use super::*;

/// What the screen says while the team's files do not read.
pub(super) fn unreadable(error: &str) -> String {
    t!(
        "Les fichiers de l'équipe ne se lisent pas ({}) : réglages en lecture seule ; détacher et quitter restent possibles.",
        "The team's files do not read ({}): settings read only; detaching and quitting still work.",
        error
    )
}

pub(crate) fn source_name(source: Source, origin: &Origin) -> String {
    match source {
        Source::Personal => config::SETTINGS_LOCAL.to_string(),
        Source::Shared => match origin {
            Origin::Local { .. } => config::SETTINGS.to_string(),
            Origin::Global { file } => file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        },
        Source::Team => t!("équipe", "team"),
        Source::Default => t!("défaut", "default"),
    }
}

/// `opus` → `Opus`; an id as it is.
pub(crate) fn model_label(model: &str) -> String {
    bridge::Model::of_alias(model).map_or_else(|| model.to_string(), |m| m.label.to_string())
}

pub(super) fn mode_detail(mode: &str) -> String {
    match mode {
        "default" => t!("demande avant d'agir", "asks before acting"),
        "acceptEdits" => {
            t!("les modifications de fichiers sont acceptées d'office", "file edits are accepted automatically")
        }
        "auto" => {
            t!("Claude Code approuve seul ce qui est sans risque", "Claude Code approves safe actions on its own")
        }
        "plan" => t!("prépare un plan sans rien modifier", "makes a plan, changes nothing"),
        "bypassPermissions" => t!(
            "aucune demande (comme --dangerously-skip-permissions)",
            "never asks (like --dangerously-skip-permissions)"
        ),
        _ => String::new(),
    }
}

pub(crate) fn instructions_count(instructions: Option<&str>) -> String {
    match instructions.map(|i| i.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0) {
        0 => t!("aucune", "none"),
        lines => i18n::count(lines, "ligne", "line"),
    }
}

/// The first line of `text`, cut to `width` columns.
pub(crate) fn first_line(text: &str, width: usize) -> String {
    fit(text.trim().lines().next().unwrap_or_default(), width)
}

/// « a », « b » et « c ».
pub(crate) fn quoted(names: &[impl AsRef<str>]) -> String {
    i18n::joined(i18n::lang(), names, &t!("et", "and"))
}

/// What a change that starts members again would interrupt, for the members `quoted` (one of them, or several) in
/// `state`: at work, waiting for the user, or in a state not known.
pub(super) fn interruption(state: State, one: bool, quoted: &str) -> String {
    match (state, one) {
        (State::Working, true) => t!(
            "{} est au travail : sa tâche en cours sera interrompue.",
            "{} is at work: its current task will be interrupted.",
            quoted
        ),
        (State::Working, false) => t!(
            "{} sont au travail : leurs tâches en cours seront interrompues.",
            "{} are at work: their current tasks will be interrupted.",
            quoted
        ),
        (State::Waiting, true) => t!(
            "{} attend ta réponse (permission ou plan) : sa demande sera interrompue.",
            "{} is waiting for your answer (permission or plan): its request will be interrupted.",
            quoted
        ),
        (State::Waiting, false) => t!(
            "{} attendent ta réponse (permission ou plan) : leurs demandes seront interrompues.",
            "{} are waiting for your answer (permission or plan): their requests will be interrupted.",
            quoted
        ),
        (_, true) => t!("{} : état inconnu, peut-être au travail.", "{}: state unknown, perhaps at work.", quoted),
        (_, false) => t!("{} : états inconnus, peut-être au travail.", "{}: states unknown, perhaps at work.", quoted),
    }
}

/// Why a change to `fid` is carried out by starting the member again.
pub(super) fn restart_reason(fid: Fid) -> String {
    match fid {
        Fid::Mode => {
            t!("Le mode de permission ne change qu'à la relance :", "The permission mode changes only at a restart:")
        }
        Fid::Name => {
            t!("Un membre renommé repart sur sa conversation :", "A renamed member starts again on its conversation:")
        }
        Fid::Contact => {
            t!("Les interlocuteurs changent de barre en repartant :", "The contacts change bars as they start again:")
        }
        _ => t!("Ce changement ne passe pas par ses requêtes :", "This change cannot go through its requests:"),
    }
}

/// The one note of a restart's confirmation in the quick list, by the state of the member it interrupts.
pub(super) fn quick_note(state: Option<State>) -> String {
    match state {
        Some(State::Waiting) => t!(
            "Il attend ta réponse : sa demande sera interrompue. Il repartira sur sa conversation.",
            "It waits for your answer: its request will be interrupted. It starts again on its conversation."
        ),
        Some(State::Working) => t!(
            "Il est au travail : sa tâche en cours sera interrompue. Il repartira sur sa conversation.",
            "It is at work: its current task will be interrupted. It starts again on its conversation."
        ),
        _ => t!(
            "Son état est inconnu : sa tâche en cours peut être interrompue. Il repartira sur sa conversation.",
            "Its state is unknown: its current task may be interrupted. It starts again on its conversation."
        ),
    }
}
