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

use crate::backend::{self, Backend};
use crate::cli::{Cli, Command, NewArgs};
use crate::config::{
    self, Catalog, Found, Member, Scope, Team, TmuxSettings, tilde, validate_member_name, validate_team_name,
};
use crate::launch::{self, Options};
use crate::state::Snapshot;
use crate::templates::Templates;
use crate::tmux::{self, Tmux};
use crate::ui::{self, Choice};
use crate::{board, bridge, claude, generate, i18n, live, member, menu, mux, t, wizard};

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
        Some(Command::Click { state, pane }) => click(&state, &pane),
        Some(Command::Menu { state, client: Some(client), popup: true, .. }) => live::popup(&state, &client),
        Some(Command::Menu { state, client, nerd, member, field, .. }) => {
            menu::run(&state, client.as_deref(), nerd, member.as_deref(), field.as_deref())
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
    let outcome = wizard::run(cwd, catalog, prefill)?;
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

    let name = args.name.clone().context(t!(
        "le nom de l'équipe manque : recruit new <nom> …",
        "the team name is missing: recruit new <name> …"
    ))?;
    validate_team_name(&name).map_err(anyhow::Error::msg)?;
    let templates = Templates::load();
    let catalog = Catalog::load(cwd)?;
    let root = catalog.local.as_ref().map_or_else(|| cwd.to_path_buf(), |(root, _)| root.clone());

    let mut team = if let Some(kind) = &args.template {
        templates.build(kind, args.size.as_deref().unwrap_or("small"))?
    } else if let Some(description) = &args.describe {
        let settings = catalog.local.as_ref().map(|(_, s)| s.claude.clone()).unwrap_or_default();
        let claude = claude::require(settings.command())?;
        generate::compose(
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
    team.permission_mode = args.permission_mode.or(team.permission_mode);
    team.model = args.model.or(team.model);

    let file = if args.global {
        config::save_global(&name, &team, args.force)?
    } else {
        config::save_local(&root, &name, &team, args.force)?
    };
    println!("{}", t!("Équipe « {} » enregistrée dans {}.", "Team \"{}\" saved in {}.", name, tilde(&file)));
    ui::print_team(&team);
    if args.launch {
        let scope = if args.global { Scope::Global } else { Scope::Local };
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
    let settings = local.first().or(global.first()).map(|f| f.tmux.clone()).unwrap_or_default();
    let running: Vec<_> = backend::running(&settings).into_iter().map(|(_, team)| team).collect();
    let state = |name: &str| running.iter().find(|r| r.session == tmux::session_name(name));
    let trial =
        |name: &str| running.iter().any(|r| r.session == format!("{}{}", tmux::session_name(name), launch::DRY_RUN));
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
    let known: Vec<String> = local
        .iter()
        .chain(&global)
        .flat_map(|f| [tmux::session_name(&f.name), format!("{}{}", tmux::session_name(&f.name), launch::DRY_RUN)])
        .collect();
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

/// The session `attach` and `stop` act on, and what runs it: the named team, this project's team, or the only one
/// running.
fn target(cwd: &Path, team: Option<&str>) -> Result<(Box<dyn Backend>, String, String)> {
    let catalog = Catalog::load(cwd)?;
    let found = match team {
        Some(name) => catalog.find(name),
        None => catalog.default_local(),
    };
    let settings = found.as_ref().map(|f| f.tmux.clone()).unwrap_or_else(TmuxSettings::default);
    if let Some(name) = team.or(found.as_ref().map(|f| f.name.as_str())) {
        let session = tmux::session_name(name);
        let trial = format!("{session}{}", launch::DRY_RUN);
        for session in [session, trial] {
            let (_, backend) = backend::for_session(&session, &settings)?;
            if backend.has_session(&session) {
                return Ok((backend, session, name.to_string()));
            }
        }
        bail!(t!(
            "l'équipe « {} » ne tourne pas. Pour la lancer : recruit {}",
            "team \"{}\" is not running. To launch it: recruit {}",
            name,
            name
        ));
    }
    let mut running = backend::running(&settings);
    let (kind, session, team) = match running.len() {
        0 => bail!(t!("aucune équipe ne tourne", "no team is running")),
        1 => {
            let (kind, r) = running.remove(0);
            (kind, r.session, r.team)
        }
        _ if ui::interactive() => {
            let choices = running
                .into_iter()
                .map(|(kind, r)| Choice::new((kind, r.session.clone(), r.team.clone()), r.session))
                .collect();
            ui::select(&t!("Quelle équipe ?", "Which team?"), choices)?
        }
        _ => {
            let names: Vec<String> = running.into_iter().map(|(_, r)| r.session).collect();
            bail!(t!(
                "plusieurs équipes tournent ({}) : précise laquelle",
                "several teams are running ({}): say which",
                names.join(", ")
            ))
        }
    };
    Ok((backend::new(kind, &settings, &session)?, session, team))
}

fn attach(cwd: &Path, team: Option<&str>) -> Result<()> {
    let (backend, session, _) = target(cwd, team)?;
    backend.attach(&session)
}

fn stop(cwd: &Path, team: Option<&str>, yes: bool) -> Result<()> {
    let (backend, session, name) = target(cwd, team)?;
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
    backend.stop(&session)?;
    println!("{}", t!("Équipe « {} » arrêtée.", "Team \"{}\" stopped.", name));
    Ok(())
}

/// A click on a panel, bound in recruit's tmux server: the member clicked gets the focus, in its tab. A click that
/// falls on no one does nothing.
fn click(state: &Path, pane: &str) -> Result<()> {
    let snapshot = Snapshot::read(state)?;
    let tmux = Tmux::new(&TmuxSettings { socket: Some(snapshot.socket.clone()), ..Default::default() })?;
    let Some(click) = tmux.click(pane)? else { return Ok(()) };
    let panel = match click.role.as_str() {
        tmux::DASHBOARD => board::Kind::Dashboard,
        tmux::JOURNAL => board::Kind::Journal,
        _ => return Ok(()),
    };
    match board::clicked(&snapshot, state, panel, click.x, click.y, click.columns, &click.line) {
        Some(board::Clicked::Compact(member)) => {
            compact(state, &member, |choice| tmux.confirm(&click.client, pane, choice))
        }
        Some(board::Clicked::Show(member)) => tmux.focus(&snapshot.session, &member).map(drop),
        None => Ok(()),
    }
}

/// Offers to compact a member's context, in a menu over the dashboard: the member's model reads it all again, which
/// the user sees before choosing. Once confirmed, the member's mod takes the request; a member gone back to work by
/// then refuses it.
/// `confirm` asks the user the choice recruit's own multiplexer shows too (`mux::chrome::compact_choice`): true for
/// its first option.
fn compact(state: &Path, member: &str, confirm: impl FnOnce(&mux::chrome::Choice) -> Result<bool>) -> Result<()> {
    // The name makes a file name: a member's, nothing else.
    validate_member_name(member).map_err(anyhow::Error::msg)?;
    if confirm(&mux::chrome::compact_choice(member, board::context_percent(state, member)))? {
        bridge::request_compaction(state, member)?;
    }
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
    fn compaction_asked_once_confirmed() {
        let state = tempfile::tempdir().unwrap();
        let asked = |name: &str| state.path().join("compact").join(name).exists();
        // Not a member's name: refused before asking, nothing written.
        for name in ["../escape", "dev cli", "-dev", "a#{pane_id}", ""] {
            assert!(compact(state.path(), name, |_| panic!("asked for {name:?}")).is_err(), "{name:?}");
        }
        assert!(!state.path().join("compact").exists());
        // Cancelled, then confirmed.
        compact(state.path(), "dev-cli", |_| Ok(false)).unwrap();
        assert!(!asked("dev-cli"));
        let mut shown = None;
        compact(state.path(), "dev-cli", |choice| {
            shown = Some(choice.clone());
            Ok(true)
        })
        .unwrap();
        assert!(asked("dev-cli"));
        // The native multiplexer's words, its context unknown here.
        assert_eq!(shown, Some(mux::chrome::compact_choice("dev-cli", None)));
    }

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
