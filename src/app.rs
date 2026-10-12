// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What each command does.
//!
//! A bare `recruit`:
//! 1. a team in this project (`.recruit/`): launch it at once;
//! 2. none here but global teams exist: offer them, or creating one; nothing is launched without asking;
//! 3. no team at all: interactive creation.
//!
//! `recruit <name>` launches the local team of that name, else the global one, else creates it.

use std::path::Path;
use std::process::Command as Process;

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::cli::{Cli, Command, NewArgs};
use crate::config::{self, Catalog, Found, Scope, validate_team_name};
use crate::create::templates::Templates;
use crate::mux::Mux;
use crate::sys::tilde;
use crate::team::launch::{self, Options};

use crate::ui::{self, Choice};
use crate::{board, claude::bridge, create, i18n, menu, mux, t, team::live, team::member};

pub fn run(cli: Cli) -> Result<()> {
    let cwd = std::env::current_dir().context("current directory")?;
    if cli.command.is_some() && (cli.team.is_some() || cli.launch != Default::default()) {
        bail!(t!(
            "un nom d'équipe ou une option de lancement ne va pas avec une sous-commande",
            "a team name or a launch option does not go with a subcommand"
        ));
    }
    match cli.command {
        Some(Command::New(args)) => new(&cwd, args),
        Some(Command::List { json }) => list(&cwd, json),
        Some(Command::Attach { team }) => attach(&cwd, team.as_deref()),
        Some(Command::Stop { team, yes }) => stop(&cwd, team.as_deref(), yes),
        Some(Command::Edit { team, local }) => edit(&cwd, team.as_deref(), local),
        Some(Command::Templates) => templates(),
        Some(Command::Panel { kind, state }) => board::run(kind, &state),
        Some(Command::Mod { event, state, member, command }) => bridge::run(event, &state, &member, command.as_deref()),
        Some(Command::Server { state }) => mux::server::run(&state),
        Some(Command::Ctl { state, action }) => mux::ctl::run(&state, action),
        Some(Command::Member { state, member, resume }) => member::run(&state, &member, resume),
        Some(Command::Menu { state, client, nerd, light, member, field, list }) => {
            crate::ui::look::set_light(light);
            menu::run(&state, client.as_deref(), nerd, member.as_deref(), field.as_deref(), list)
        }
        Some(Command::EditRunning { state, edits, restart, fresh, restart_all, dismiss }) => {
            edit_running(&state, edits.as_deref(), restart.as_deref(), fresh, restart_all, dismiss.as_deref())
        }
        None => {
            let options: Options = cli.launch.into();
            match cli.team {
                Some(name) => by_name(&cwd, &name, &options),
                None => default(&cwd, &options),
            }
        }
    }
}

fn by_name(cwd: &Path, name: &str, options: &Options) -> Result<()> {
    let catalog = Catalog::load(cwd)?;
    if let Some(found) = catalog.find(name) {
        return launch::launch(&found, cwd, options);
    }
    if !ui::interactive() {
        bail!(t!(
            "aucune équipe « {} ». Pour la créer : recruit new {} --template … ou --member nom:rôle",
            "no team \"{}\". To create it: recruit new {} --template … or --member name:role",
            name,
            name
        ));
    }
    validate_team_name(name).map_err(anyhow::Error::msg)?;
    println!(
        "{}\n",
        t!(
            "L'équipe « {} » n'existe pas encore : créons-la.",
            "Team \"{}\" does not exist yet: let's create it.",
            name
        )
    );
    create(cwd, &catalog, Some(name), options)
}

fn default(cwd: &Path, options: &Options) -> Result<()> {
    let catalog = Catalog::load(cwd)?;

    let local = catalog.local_teams();
    if !local.is_empty() {
        let found = match catalog.default_local() {
            Some(found) => found,
            None if ui::interactive() => {
                let choices = local.into_iter().map(|f| Choice::new(f.clone(), team_line(&f))).collect();
                ui::select(&t!("Quelle équipe lancer ?", "Which team should be launched?"), choices)?
            }
            None => bail!(t!(
                "plusieurs équipes dans ce projet : choisis-en une (recruit <nom>) ou fixe `default` dans .recruit/settings.toml",
                "several teams in this project: pick one (recruit <name>) or set `default` in .recruit/settings.toml"
            )),
        };
        return launch::launch(&found, cwd, options);
    }

    let global = catalog.global_teams();
    if !global.is_empty() {
        let names: Vec<&str> = global.iter().map(|f| f.name.as_str()).collect();
        if !ui::interactive() {
            bail!(t!(
                "aucune équipe dans ce dossier. Équipes globales : {}. Lance-en une avec recruit <nom>",
                "no team in this directory. Global teams: {}. Launch one with recruit <name>",
                names.join(", ")
            ));
        }
        let mut choices: Vec<Choice<Option<Found>>> = global
            .into_iter()
            .map(|f| {
                let label = t!("Lancer l'équipe globale {}", "Launch the global team {}", team_line(&f));
                Choice::new(Some(f), label)
            })
            .collect();
        choices.push(Choice::new(None, t!("Créer une nouvelle équipe", "Create a new team")));
        let choice = ui::select(
            &t!(
                "Aucune équipe dans ce dossier. Que veux-tu faire ?",
                "No team in this directory. What do you want to do?"
            ),
            choices,
        )?;
        return match choice {
            Some(found) => launch::launch(&found, cwd, options),
            None => create(cwd, &catalog, None, options),
        };
    }

    if !ui::interactive() {
        bail!(t!(
            "aucune équipe trouvée. Lance recruit dans un terminal pour en créer une, ou : recruit new <nom> --template …",
            "no team found. Run recruit in a terminal to create one, or: recruit new <name> --template …"
        ));
    }
    create(cwd, &catalog, None, options)
}

fn create(cwd: &Path, catalog: &Catalog, prefill: Option<&str>, options: &Options) -> Result<()> {
    let outcome = create::run(cwd, catalog, prefill)?;
    if !outcome.launch {
        return Ok(());
    }
    launch_created(cwd, &outcome.name, outcome.scope, options)
}

fn launch_created(cwd: &Path, name: &str, scope: Scope, options: &Options) -> Result<()> {
    let catalog = Catalog::load(cwd)?;
    let teams = match scope {
        Scope::Local => catalog.local_teams(),
        Scope::Global => catalog.global_teams(),
    };
    let found = teams.into_iter().find(|f| f.name == name).context("team just saved")?;
    launch::launch(&found, cwd, options)
}

fn new(cwd: &Path, args: NewArgs) -> Result<()> {
    let scripted = args.template.is_some() || args.describe.is_some() || !args.members.is_empty();
    if !scripted {
        if !ui::interactive() {
            bail!(t!(
                "sans terminal, précise --template, --describe ou --member",
                "without a terminal, give --template, --describe or --member"
            ));
        }
        let catalog = Catalog::load(cwd)?;
        if let Some(name) = &args.name {
            validate_team_name(name).map_err(anyhow::Error::msg)?;
        }
        return create(cwd, &catalog, args.name.as_deref(), &Options::default());
    }

    let (name, scope) = create::new::make(cwd, &args)?;
    if args.launch {
        launch_created(cwd, &name, scope, &Options::default())?;
    }
    Ok(())
}

fn team_line(found: &Found) -> String {
    let members: Vec<&str> = found.team.members.keys().map(String::as_str).collect();
    format!("{} ({})", found.name, members.join(", "))
}

fn list(cwd: &Path, as_json: bool) -> Result<()> {
    let catalog = Catalog::load(cwd)?;
    let local = catalog.local_teams();
    let global = catalog.global_teams();
    let running = mux::running();
    let state = |name: &str| running.iter().find(|r| r.session == name);
    let trial = |name: &str| running.iter().any(|r| r.session == launch::trial_session(name));
    let default = catalog.local.as_ref().and_then(|(_, s)| s.default.clone());

    if as_json {
        let rows: Vec<_> = local
            .iter()
            .chain(&global)
            .map(|f| {
                let run = state(&f.name);
                json!({
                    "name": f.name,
                    "scope": if f.scope == Scope::Local { "local" } else { "global" },
                    "file": f.file,
                    "members": f.team.members.keys().collect::<Vec<_>>(),
                    "running": run.is_some(),
                    "attached": run.is_some_and(|r| r.attached),
                    "dir": run.map(|r| r.dir.clone()),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    let print = |teams: &[Found]| {
        let width = teams.iter().map(|f| f.name.chars().count()).max().unwrap_or(0);
        for f in teams {
            let mark = if default.as_deref() == Some(f.name.as_str()) { "*" } else { " " };
            let status = match state(&f.name) {
                Some(r) if r.attached => t!("en cours, attachée", "running, attached"),
                Some(_) => t!("en cours", "running"),
                None if trial(&f.name) => t!("essai en cours (--dry-run)", "trial running (--dry-run)"),
                None => t!("arrêtée", "stopped"),
            };
            let pad = " ".repeat(width - f.name.chars().count());
            let members = i18n::count(f.team.members.len(), "membre", "member");
            println!("  {mark} {}{pad}  {members:>11}  {status}", f.name);
        }
    };
    if let Some((root, _)) = &catalog.local {
        println!(
            "{}",
            t!("Équipes de ce projet ({})", "Teams of this project ({})", tilde(&root.join(config::LOCAL_DIR)))
        );
        print(&local);
    }
    if !global.is_empty() {
        println!("{}", t!("Équipes globales ({})", "Global teams ({})", tilde(&config::global_dir())));
        print(&global);
    }
    if local.is_empty() && global.is_empty() {
        println!(
            "{}",
            t!("Aucune équipe. Tape recruit pour en créer une.", "No team yet. Type recruit to create one.")
        );
    }
    let known: Vec<String> =
        local.iter().chain(&global).flat_map(|f| [f.name.clone(), launch::trial_session(&f.name)]).collect();
    let others: Vec<String> =
        running
            .iter()
            .filter(|r| !known.contains(&r.session))
            .map(|r| {
                if r.dir.is_empty() {
                    r.session.clone()
                } else {
                    format!("{} ({})", r.session, tilde(Path::new(&r.dir)))
                }
            })
            .collect();
    if !others.is_empty() {
        println!("{}", t!("Autres équipes en cours : {}", "Other running teams: {}", others.join(", ")));
    }
    Ok(())
}

/// The team `attach` and `stop` act on, and its name: the named team (or its trial), this project's team, or the only
/// one running.
fn target(cwd: &Path, team: Option<&str>) -> Result<(Mux, String)> {
    let catalog = Catalog::load(cwd)?;
    let found = match team {
        Some(name) => catalog.find(name),
        None => catalog.default_local(),
    };
    if let Some(name) = team.or(found.as_ref().map(|f| f.name.as_str())) {
        let session = name.to_string();
        let trial = launch::trial_session(&session);
        for session in [session, trial] {
            let mux = Mux::of(&session);
            if mux.is_running() {
                return Ok((mux, name.to_string()));
            }
        }
        bail!(t!(
            "l'équipe « {} » ne tourne pas. Pour la lancer : recruit {}",
            "team \"{}\" is not running. To launch it: recruit {}",
            name,
            name
        ));
    }
    let mut running = mux::running();
    let (session, team) = match running.len() {
        0 => bail!(t!("aucune équipe ne tourne", "no team is running")),
        1 => {
            let r = running.remove(0);
            (r.session, r.team)
        }
        _ if ui::interactive() => {
            let choices =
                running.into_iter().map(|r| Choice::new((r.session.clone(), r.team.clone()), r.session)).collect();
            ui::select(&t!("Quelle équipe ?", "Which team?"), choices)?
        }
        _ => {
            let names: Vec<String> = running.into_iter().map(|r| r.session).collect();
            bail!(t!(
                "plusieurs équipes tournent ({}) : précise laquelle",
                "several teams are running ({}): say which",
                names.join(", ")
            ))
        }
    };
    Ok((Mux::of(&session), team))
}

fn attach(cwd: &Path, team: Option<&str>) -> Result<()> {
    let (mux, _) = target(cwd, team)?;
    mux.attach()
}

fn stop(cwd: &Path, team: Option<&str>, yes: bool) -> Result<()> {
    let (mux, name) = target(cwd, team)?;
    if !yes && ui::interactive() {
        let sure = ui::confirm(
            &t!(
                "Arrêter l'équipe « {} » ? Ses sessions Claude seront fermées (recruit {} --resume les reprendra).",
                "Stop team \"{}\"? Its Claude sessions will be closed (recruit {} --resume picks them up again).",
                name,
                name
            ),
            true,
        )?;
        if !sure {
            return Ok(());
        }
    }
    mux.stop()?;
    println!("{}", t!("Équipe « {} » arrêtée.", "Team \"{}\" stopped.", name));
    Ok(())
}

/// `recruit _edit`: what the menu does, from the command line.
fn edit_running(
    state: &Path,
    edits: Option<&str>,
    restart: Option<&str>,
    fresh: bool,
    all: bool,
    dismiss: Option<&str>,
) -> Result<()> {
    let mut running = live::Running::open(state)?;
    if let Some(member) = dismiss {
        running.dismiss(member)?;
    }
    if let Some(edits) = edits {
        let edits: Vec<config::Edit> = serde_json::from_str(edits).context("edits")?;
        running.apply(&edits)?;
    }
    if let Some(member) = restart {
        running.restart(member, fresh)?;
    }
    if all {
        running.restart_all()?;
    }
    Ok(())
}

fn edit(cwd: &Path, team: Option<&str>, personal: bool) -> Result<()> {
    let file = edit_target(cwd, team, personal)?;
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    let status = Process::new("/bin/sh").args(["-c", &format!("{editor} \"$1\""), "sh"]).arg(&file).status()?;
    if !status.success() {
        bail!("{editor}: {status}");
    }
    // Say at once if the file no longer reads.
    Catalog::load(cwd).map(drop)
}

/// The file `recruit edit` opens: found even when the team files do not read, which is when they need editing.
/// Without a name, the project's `settings.toml`; with `personal`, its `settings.local.toml`, created if needed.
fn edit_target(cwd: &Path, team: Option<&str>, personal: bool) -> Result<std::path::PathBuf> {
    let root = config::find_local_root(cwd);
    if personal {
        let root = root.context(t!("pas d'équipe dans ce projet", "no team in this project"))?;
        let file = root.join(config::LOCAL_DIR).join(config::SETTINGS_LOCAL);
        if !file.exists() {
            let header = t!(
                "# Réglages personnels de recruit, non commités. Ils priment sur settings.toml, clé par clé.\n# Exemple :\n# [teams.<équipe>.members.<membre>]\n# model = \"sonnet\"\n",
                "# Personal recruit settings, not committed. They take precedence over settings.toml, key by key.\n# Example:\n# [teams.<team>.members.<member>]\n# model = \"sonnet\"\n"
            );
            std::fs::write(&file, header)?;
        }
        return Ok(file);
    }
    let Some(name) = team else {
        let root = root.context(t!(
            "pas d'équipe dans ce projet : précise son nom (recruit edit <nom>)",
            "no team in this project: give its name (recruit edit <name>)"
        ))?;
        return Ok(root.join(config::LOCAL_DIR).join(config::SETTINGS));
    };
    match Catalog::load(cwd) {
        Ok(catalog) => {
            catalog.find(name).map(|f| f.file).with_context(|| t!("aucune équipe « {} »", "no team \"{}\"", name))
        }
        // The files do not read: the team found by its name alone, else the reason they do not read.
        Err(error) => config::team_file_unchecked(cwd, name).ok_or(error),
    }
}

fn templates() -> Result<()> {
    let templates = Templates::load();
    println!("{}", t!("Types de projet (--template) :", "Project types (--template):"));
    for (id, label) in templates.types() {
        println!("  {id:<10} {} · {}", label.label, label.hint);
        for (size, _) in templates.sizes() {
            let team = templates.build(id, size)?;
            let names: Vec<&str> = team.members.keys().map(String::as_str).collect();
            println!("    {size:<7} {}", names.join(", "));
        }
    }
    println!("\n{}", t!("Tailles (--size) :", "Sizes (--size):"));
    for (id, label) in templates.sizes() {
        println!("  {id:<10} {} · {}", label.label, label.hint);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_opens_files_that_do_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(config::LOCAL_DIR);
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        let shared = local.join(config::SETTINGS);
        let personal = local.join(config::SETTINGS_LOCAL);

        // A member name recruit refuses, written by hand.
        std::fs::write(&shared, "[teams.web.members.\"-dev\"]\nrole = 'Code'\n").unwrap();
        assert!(Catalog::load(&sub).is_err());
        assert_eq!(edit_target(&sub, None, false).unwrap(), shared);
        assert_eq!(edit_target(&sub, Some("web"), false).unwrap(), shared);
        assert_eq!(edit_target(&sub, None, true).unwrap(), personal);
        assert!(std::fs::read_to_string(&personal).unwrap().starts_with('#'), "created, with its header");
        // A team no file has: the reason the files do not read.
        let error = format!("{:#}", edit_target(&sub, Some("nope"), false).unwrap_err());
        assert!(error.contains("-dev"), "{error}");

        // The fault in the personal file: a member without a role.
        std::fs::write(&shared, "[teams.web.members.dev]\nrole = 'Code'\n").unwrap();
        std::fs::write(&personal, "[teams.web.members.qa]\nmodel = 'opus'\n").unwrap();
        assert!(Catalog::load(&sub).is_err());
        assert_eq!(edit_target(&sub, None, true).unwrap(), personal);
        assert_eq!(edit_target(&sub, Some("web"), false).unwrap(), shared);
    }
}
