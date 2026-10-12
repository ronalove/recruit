// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What recruit refuses in a team's files: names it would not give (the rules of the names it asks for), tab titles
//! of its own, a member without a role, a team with too many contacts.

use std::path::Path;

use super::{MAX_CONTACTS, Settings};
use crate::t;

/// Subcommand names: a team cannot take them, `recruit <name>` would never reach it.
const RESERVED: &[&str] = &[
    "new",
    "list",
    "attach",
    "stop",
    "edit",
    "templates",
    "help",
    "_panel",
    "_mod",
    "_member",
    "_menu",
    "_edit",
    "_server",
    "_ctl",
];

/// The titles of the tabs recruit makes that a member's `tab` cannot take, in both languages: "Interlocuteurs",
/// "Contacts", "Agents (2)"… They would make two windows of one title. "Agents" alone is the default group, as with no
/// `tab`: it goes with the working agents that have none.
pub fn reserved_title(title: &str) -> bool {
    let title = title.trim();
    let numbered = title
        .strip_prefix("Agents (")
        .and_then(|rest| rest.strip_suffix(')'))
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    numbered || ["Interlocuteurs", "Contacts"].contains(&title)
}

pub fn validate_team_name(name: &str) -> Result<(), String> {
    let ok_chars = name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    if name.is_empty() || name.chars().count() > 40 || !ok_chars || name.starts_with('-') {
        return Err(t!(
            "lettres, chiffres, - et _ seulement, sans - au début, 40 caractères au plus",
            "letters, digits, - and _ only, not starting with -, at most 40 characters"
        ));
    }
    if RESERVED.contains(&name) {
        return Err(t!("« {} » est une commande de recruit", "\"{}\" is a recruit command", name));
    }
    Ok(())
}

/// Member names are the addresses teammates write to.
pub fn validate_member_name(name: &str) -> Result<(), String> {
    let ok_chars = name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if name.is_empty() || name.chars().count() > 40 || !ok_chars || name.starts_with(['-', '.']) {
        return Err(t!(
            "lettres, chiffres, -, _ et . seulement, sans espace, sans - ni . au début, 40 caractères au plus",
            "letters, digits, -, _ and . only, no spaces, not starting with - or ., at most 40 characters"
        ));
    }
    Ok(())
}

/// What recruit refuses in settings read from its files, once each file reads: a name it would not give, a member
/// without a role, a team with too many contacts.
pub(super) enum Problem<'a> {
    Name(BadName<'a>),
    NoRole { team: &'a str, member: &'a str },
    Contacts { team: &'a str, count: usize },
}

/// The first problem of `settings`, in that order.
pub(super) fn problem(settings: &Settings) -> Option<Problem<'_>> {
    if let Some(bad) = bad_name(settings) {
        return Some(Problem::Name(bad));
    }
    if let Some((team, member)) = missing_role(settings) {
        return Some(Problem::NoRole { team, member });
    }
    too_many_contacts(settings).map(|(team, count)| Problem::Contacts { team, count })
}

impl Problem<'_> {
    /// The problem said, `file` named.
    pub(super) fn error(&self, file: &Path) -> anyhow::Error {
        match self {
            Problem::Name(bad) => bad.error(file),
            Problem::NoRole { team, member } => no_role(file, team, member),
            Problem::Contacts { team, count } => contacts_error(file, team, *count),
        }
    }
}

/// A team or member name recruit would refuse when it asks for one: the first in the settings, and why.
pub(super) struct BadName<'a> {
    pub(super) team: &'a str,
    pub(super) member: Option<&'a str>,
    why: String,
}

impl BadName<'_> {
    fn error(&self, file: &Path) -> anyhow::Error {
        match self.member {
            None => anyhow::anyhow!(t!(
                "{} : l'équipe « {} » ne peut pas porter ce nom : {}. Renomme-la dans le fichier.",
                "{}: team \"{}\" cannot have this name: {}. Rename it in the file.",
                file.display(),
                self.team,
                self.why
            )),
            Some(member) => anyhow::anyhow!(t!(
                "{} : le membre « {} » de l'équipe « {} » ne peut pas porter ce nom : {}. Renomme-le dans le fichier.",
                "{}: member \"{}\" of team \"{}\" cannot have this name: {}. Rename it in the file.",
                file.display(),
                member,
                self.team,
                self.why
            )),
        }
    }
}

/// The rules of the names recruit asks for, on the names of a file written by hand: a member named `-x` could not
/// start (`recruit _member … -x`), `a/b` or `..` would put its files outside its team's folder, `Dev` and `dev` would
/// share theirs on a filesystem that ignores case, and a team named `list` could not be launched by name.
fn bad_name(settings: &Settings) -> Option<BadName<'_>> {
    for (team, members) in settings.teams.iter().map(|(name, team)| (name.as_str(), &team.members)) {
        if let Err(why) = validate_team_name(team) {
            return Some(BadName { team, member: None, why });
        }
        let mut seen: Vec<(String, &str)> = Vec::new();
        for member in members.keys().map(String::as_str) {
            let lower = member.to_lowercase();
            let why = match (validate_member_name(member), seen.iter().find(|(l, _)| *l == lower)) {
                (Err(why), _) => why,
                (Ok(()), Some((_, twin))) => t!(
                    "« {} » ne diffère de « {} » que par la casse",
                    "\"{}\" differs from \"{}\" by case only",
                    member,
                    twin
                ),
                (Ok(()), None) => {
                    seen.push((lower, member));
                    continue;
                }
            };
            return Some(BadName { team, member: Some(member), why });
        }
    }
    None
}

/// The first member without a role, as (team, member).
fn missing_role(settings: &Settings) -> Option<(&str, &str)> {
    settings.teams.iter().find_map(|(name, team)| {
        team.members.iter().find(|(_, m)| m.role.trim().is_empty()).map(|(member, _)| (name.as_str(), member.as_str()))
    })
}

fn no_role(file: &Path, team: &str, member: &str) -> anyhow::Error {
    anyhow::anyhow!(t!(
        "{} : le membre « {} » de l'équipe « {} » n'a pas de rôle",
        "{}: member \"{}\" of team \"{}\" has no role",
        file.display(),
        member,
        team
    ))
}

/// The first team with more than `MAX_CONTACTS` contacts, and how many it has.
fn too_many_contacts(settings: &Settings) -> Option<(&str, usize)> {
    settings.teams.iter().find_map(|(name, team)| {
        let count = team.contacts().len();
        (count > MAX_CONTACTS).then_some((name.as_str(), count))
    })
}

fn contacts_error(file: &Path, team: &str, count: usize) -> anyhow::Error {
    anyhow::anyhow!(t!(
        "{} : l'équipe « {} » a {} interlocuteurs (contact = true), {} au plus. Les autres peuvent devenir des agents de travail.",
        "{}: team \"{}\" has {} contacts (contact = true), {} at most. The others can become working agents.",
        file.display(),
        team,
        count,
        MAX_CONTACTS
    ))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::RESERVED;
    use crate::cli::Cli;

    #[test]
    fn reserved_names_are_the_subcommands() {
        let command = Cli::command();
        let mut commands: Vec<&str> = command.get_subcommands().map(clap::Command::get_name).collect();
        commands.push("help");
        commands.sort_unstable();
        let mut reserved = RESERVED.to_vec();
        reserved.sort_unstable();
        assert_eq!(reserved, commands, "a team cannot take the name of a subcommand: keep RESERVED up to date");
    }
}
