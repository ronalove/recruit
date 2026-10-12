// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! `recruit new` with its options (`--template`, `--describe`, `--member`…): a team made and saved without a question.

use std::path::Path;

use anyhow::{Context, Result, bail};

use super::compose;
use super::templates::Templates;
use crate::cli::NewArgs;
use crate::config::{self, Catalog, Member, Scope, Team, validate_member_name, validate_team_name};
use crate::sys::tilde;
use crate::{claude, i18n, t, ui};

/// Makes the team `args` describe, saves it and shows it: its name, and where it was saved.
pub fn make(cwd: &Path, args: &NewArgs) -> Result<(String, Scope)> {
    let name = args.name.clone().context(t!(
        "le nom de l'équipe manque : recruit new <nom> …",
        "the team name is missing: recruit new <name> …"
    ))?;
    validate_team_name(&name).map_err(anyhow::Error::msg)?;
    let templates = Templates::load();
    let catalog = Catalog::load(cwd)?;
    let root = catalog.root(cwd);

    let mut team = if let Some(kind) = &args.template {
        templates.build(kind, args.size.as_deref().unwrap_or("small"))?
    } else if let Some(description) = &args.describe {
        let settings = catalog.claude();
        let claude = claude::require(settings.command())?;
        compose::compose(
            &claude,
            settings.config_dir().as_deref(),
            &root,
            description,
            args.size.as_deref(),
            &templates,
        )?
    } else {
        Team {
            lang: Some(i18n::lang()),
            instructions: Some(templates.team_instructions().trim().to_string()),
            ..Default::default()
        }
    };
    for spec in &args.members {
        let (member, role) =
            spec.split_once(':').map(|(n, r)| (n.trim(), r.trim())).filter(|(_, r)| !r.is_empty()).with_context(
                || t!("--member attend « nom:rôle », pas « {} »", "--member expects \"name:role\", not \"{}\"", spec),
            )?;
        validate_member_name(member).map_err(|e| anyhow::anyhow!("{member}: {e}"))?;
        team.members.entry(member.to_string()).or_insert_with(Member::default).role = role.to_string();
    }
    if team.members.is_empty() {
        bail!(t!("l'équipe n'a aucun membre", "the team has no members"));
    }
    if args.contacts.len() > config::MAX_CONTACTS {
        bail!(t!("--contact : {} interlocuteurs au plus", "--contact: {} contacts at most", config::MAX_CONTACTS));
    }
    if !args.contacts.is_empty() {
        if let Some(unknown) = args.contacts.iter().find(|c| !team.members.contains_key(*c)) {
            bail!(t!("--contact : aucun membre « {} »", "--contact: no member \"{}\"", unknown));
        }
        for (name, member) in team.members.iter_mut() {
            member.contact = args.contacts.contains(name);
        }
    }
    team.permission_mode = args.permission_mode.clone().or(team.permission_mode);
    team.model = args.model.clone().or(team.model);

    let file = if args.global {
        config::save_global(&name, &team, args.force)?
    } else {
        config::save_local(&root, &name, &team, args.force)?
    };
    println!("{}", t!("Équipe « {} » enregistrée dans {}.", "Team \"{}\" saved in {}.", name, tilde(&file)));
    ui::print_team(&team);
    Ok((name, if args.global { Scope::Global } else { Scope::Local }))
}
