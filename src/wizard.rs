// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Interactive team creation: guided (a type and a size, or Claude composing from a description) or manual
//! (a name and a role per member), then where to save the team, then whether to launch it.

use std::path::Path;

use anyhow::Result;

use crate::config::{
    self, Catalog, ClaudeSettings, Member, Scope, Team, tilde, validate_member_name, validate_team_name,
};
use crate::i18n;
use crate::templates::Templates;
use crate::ui::{self, Choice};
use crate::{claude, generate, t};

pub struct Outcome {
    pub name: String,
    pub scope: Scope,
    pub launch: bool,
}

enum Mode {
    Guided,
    Manual,
}

pub fn run(cwd: &Path, catalog: &Catalog, prefill: Option<&str>) -> Result<Outcome> {
    let templates = Templates::load();
    let root = catalog.local.as_ref().map_or_else(|| cwd.to_path_buf(), |(root, _)| root.clone());
    println!(
        "{}\n",
        t!("recruit · création d'une équipe d'agents Claude Code", "recruit · creating a team of Claude Code agents")
    );

    let mode = ui::select(
        &t!("Comment veux-tu composer ton équipe ?", "How do you want to build your team?"),
        vec![
            Choice::new(
                Mode::Guided,
                t!(
                    "Guidé : je choisis un type de projet et une taille d'équipe",
                    "Guided: I pick a project type and a team size"
                ),
            ),
            Choice::new(
                Mode::Manual,
                t!(
                    "Manuel : je donne le nom et le rôle de chaque membre",
                    "Manual: I give each member's name and role"
                ),
            ),
        ],
    )?;
    let mut team = match mode {
        Mode::Guided => {
            let claude = catalog.local.as_ref().map(|(_, s)| s.claude.clone()).unwrap_or_default();
            guided(&templates, &root, &claude)?
        }
        Mode::Manual => manual(&templates)?,
    };
    review(&mut team)?;
    contacts(&mut team)?;
    permissions(&mut team)?;

    let suggested = prefill.map(String::from).unwrap_or_else(|| suggest_name(&root));
    let name = ui::text(&t!("Nom de l'équipe", "Team name"), Some(&suggested), validate_team_name)?;

    let local_file = root.join(config::LOCAL_DIR).join(config::SETTINGS);
    let global_file = config::global_file(&name);
    let scope = ui::select(
        &t!("Où enregistrer l'équipe ?", "Where should the team be saved?"),
        vec![
            Choice::new(
                Scope::Local,
                t!("Dans ce projet : {} (à commiter)", "In this project: {} (to commit)", tilde(&local_file)),
            ),
            Choice::new(
                Scope::Global,
                t!(
                    "Dans mon profil : {} (lançable depuis n'importe quel dossier)",
                    "In my profile: {} (can be launched from any directory)",
                    tilde(&global_file)
                ),
            ),
        ],
    )?;

    let exists = match scope {
        Scope::Local => catalog.local_teams().iter().any(|f| f.name == name),
        Scope::Global => global_file.exists(),
    };
    let replace = exists
        && ui::confirm(
            &t!(
                "L'équipe « {} » existe déjà là. La remplacer ?",
                "Team \"{}\" already exists there. Replace it?",
                name
            ),
            false,
        )?;
    if exists && !replace {
        return Err(ui::Cancelled.into());
    }

    let file = match scope {
        Scope::Local => config::save_local(&root, &name, &team, replace)?,
        Scope::Global => config::save_global(&name, &team, replace)?,
    };
    println!();
    ui::print_paragraph(&t!("Équipe « {} » enregistrée dans {}.", "Team \"{}\" saved in {}.", name, tilde(&file)));
    if scope == Scope::Local {
        ui::print_paragraph(&t!(
            "Commite .recruit/settings.toml ; tes réglages personnels vont dans .recruit/settings.local.toml, ignoré par git.",
            "Commit .recruit/settings.toml; your personal settings go in .recruit/settings.local.toml, ignored by git."
        ));
    }

    let launch = ui::confirm(&t!("Lancer l'équipe maintenant ?", "Launch the team now?"), true)?;
    if !launch {
        let how = match scope {
            Scope::Local => t!("tape recruit dans ce dossier", "type recruit in this directory"),
            Scope::Global => format!("recruit {name}"),
        };
        println!("{}", t!("Pour la lancer plus tard : {}.", "To launch it later: {}.", how));
    }
    Ok(Outcome { name, scope, launch })
}

fn guided(templates: &Templates, root: &Path, claude: &ClaudeSettings) -> Result<Team> {
    let mut kinds: Vec<Choice<Option<String>>> = templates
        .types()
        .into_iter()
        .map(|(id, label)| Choice::new(Some(id.to_string()), format!("{} · {}", label.label, label.hint)))
        .collect();
    kinds.push(Choice::new(
        None,
        t!(
            "Autre : je décris mon projet, Claude compose l'équipe",
            "Other: I describe my project, Claude builds the team"
        ),
    ));
    let kind = ui::select(&t!("Quel type de projet ?", "What kind of project?"), kinds)?;

    let mut sizes: Vec<Choice<Option<String>>> = templates
        .sizes()
        .into_iter()
        .map(|(id, label)| Choice::new(Some(id.to_string()), format!("{} · {}", label.label, label.hint)))
        .collect();
    if kind.is_none() {
        sizes.push(Choice::new(None, t!("À Claude de juger", "Let Claude decide")));
    }
    let size = ui::select(&t!("Quelle taille d'équipe ?", "What team size?"), sizes)?;

    match kind {
        Some(kind) => templates.build(&kind, size.as_deref().unwrap_or("small")),
        None => {
            let description = ui::text(
                &t!("Décris ton projet en quelques phrases", "Describe your project in a few sentences"),
                None,
                ui::required,
            )?;
            let exe = claude::require(claude.command())?;
            generate::compose(&exe, claude.config_dir().as_deref(), root, &description, size.as_deref(), templates)
        }
    }
}

fn manual(templates: &Templates) -> Result<Team> {
    let mut team = Team { lang: Some(i18n::lang()), ..Default::default() };
    println!(
        "{}",
        t!(
            "Donne le nom et le rôle de chaque membre ; tu choisiras ensuite tes interlocuteurs. Laisse le nom vide pour terminer.",
            "Give each member's name and role; you will then pick your contacts. Leave the name empty to finish."
        )
    );
    while add_member(&mut team)? {}
    let shared = ui::confirm(
        &t!(
            "Ajouter les règles communes de recruit (point d'entrée, zones, dépôt git partagé…) ?",
            "Add recruit's shared rules (point of contact, areas, shared git repository…)?"
        ),
        true,
    )?;
    if shared {
        team.instructions = Some(templates.team_instructions().trim().to_string());
    }
    Ok(team)
}

/// Asks for one more member; false when the user leaves the name empty (allowed once there is one member).
fn add_member(team: &mut Team) -> Result<bool> {
    let taken: Vec<String> = team.members.keys().cloned().collect();
    let first = taken.is_empty();
    let number = taken.len() + 1;
    let name = ui::text(&t!("Membre {} : nom", "Member {}: name", number), None, move |answer: &str| {
        if answer.is_empty() {
            return if first {
                Err(t!("Il faut au moins un membre.", "At least one member is needed."))
            } else {
                Ok(())
            };
        }
        if taken.iter().any(|n| n == answer) {
            return Err(t!("Ce nom est déjà pris.", "This name is already taken."));
        }
        validate_member_name(answer)
    })?;
    if name.is_empty() {
        return Ok(false);
    }
    let role = ui::text(
        &t!("Rôle de {} (une ligne, vue par ses coéquipiers)", "Role of {} (one line, seen by teammates)", name),
        None,
        ui::required,
    )?;
    let instructions = ui::text(
        &t!("Instructions particulières pour {} (facultatif)", "Specific instructions for {} (optional)", name),
        None,
        ui::optional,
    )?;
    team.members.insert(
        name,
        Member { role, instructions: Some(instructions).filter(|i| !i.is_empty()), ..Default::default() },
    );
    Ok(true)
}

enum Review {
    Accept,
    Remove,
    Add,
}

fn review(team: &mut Team) -> Result<()> {
    loop {
        println!("\n{}", t!("L'équipe ({}) :", "The team ({}):", i18n::count(team.members.len(), "membre", "member")));
        ui::print_team(team);
        println!();
        let action = ui::select(
            &t!("Elle te convient ?", "Does it suit you?"),
            vec![
                Choice::new(Review::Accept, t!("Oui, on garde cette équipe", "Yes, keep this team")),
                Choice::new(Review::Remove, t!("Retirer des membres", "Remove members")),
                Choice::new(Review::Add, t!("Ajouter un membre", "Add a member")),
            ],
        )?;
        match action {
            Review::Accept => break,
            Review::Remove => {
                let choices = team.members.keys().map(|n| Choice::new(n.clone(), n.clone())).collect();
                let removed = ui::multi_select(&t!("Membres à retirer", "Members to remove"), choices, &[], None)?;
                if removed.len() >= team.members.len() {
                    println!("{}", t!("Il faut garder au moins un membre.", "At least one member must stay."));
                    continue;
                }
                team.members.retain(|name, _| !removed.contains(name));
            }
            Review::Add => {
                add_member(team)?;
            }
        }
    }
    Ok(())
}

/// Which members the user talks to; the others are working agents.
fn contacts(team: &mut Team) -> Result<()> {
    let current = team.contacts().into_iter().map(String::from).collect::<Vec<_>>();
    let checked: Vec<usize> =
        team.members.keys().enumerate().filter(|(_, n)| current.contains(n)).map(|(i, _)| i).collect();
    let choices = team.members.keys().map(|n| Choice::new(n.clone(), n.clone())).collect();
    let chosen = ui::multi_select(
        &t!(
            "Qui sont tes interlocuteurs (deux au plus) ? Ils seront dans le premier onglet, à côté du tableau de bord ; les autres sont des agents de travail, que tu peux voir et interrompre.",
            "Who are your contacts (two at most)? They go in the first tab, next to the dashboard; the others are working agents, which you can watch and step in on."
        ),
        choices,
        &checked,
        Some(config::MAX_CONTACTS),
    )?;
    for (name, member) in team.members.iter_mut() {
        member.contact = chosen.contains(name);
    }
    // At least one contact: the first member when none was ticked.
    if chosen.is_empty()
        && let Some(first) = team.members.values_mut().next()
    {
        first.contact = true;
    }
    Ok(())
}

fn permissions(team: &mut Team) -> Result<()> {
    let mode = ui::select(
        &t!("Permissions des agents", "Agents' permissions"),
        vec![
            Choice::new(
                None,
                t!("Réglage habituel de Claude Code (rien d'imposé)", "Claude Code's usual setting (nothing forced)"),
            ),
            Choice::new(
                Some("acceptEdits"),
                t!(
                    "acceptEdits : les modifications de fichiers sont acceptées d'office",
                    "acceptEdits: file edits are accepted automatically"
                ),
            ),
            Choice::new(
                Some("auto"),
                t!(
                    "auto : Claude Code approuve seul ce qui est sans risque",
                    "auto: Claude Code approves safe actions on its own"
                ),
            ),
            Choice::new(
                Some("bypassPermissions"),
                t!(
                    "bypassPermissions : aucune demande (comme --dangerously-skip-permissions)",
                    "bypassPermissions: never asks (like --dangerously-skip-permissions)"
                ),
            ),
        ],
    )?;
    team.permission_mode = mode.map(String::from);
    Ok(())
}

/// The project directory's name, made into a valid team name.
fn suggest_name(root: &Path) -> String {
    let base = root.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let name: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(40)
        .collect();
    if validate_team_name(&name).is_ok() { name } else { t!("equipe", "team") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_names() {
        assert_eq!(suggest_name(Path::new("/home/me/My Project.rs")), "my-project-rs");
        assert_eq!(suggest_name(Path::new("/home/me/omnidex")), "omnidex");
        assert!(validate_team_name(&suggest_name(Path::new("/home/me/list"))).is_ok());
    }
}
