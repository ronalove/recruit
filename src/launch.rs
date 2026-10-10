// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Launching a team: one tmux session, the contacts' tab then the working agents' tabs, a Claude session per pane.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::config::{Found, Scope, Team, tilde};
use crate::i18n::Lang;
use crate::state::{self, Snapshot};
use crate::tmux::{self, Backend, Pane, Plan, Side, TabPlan, Tmux};
use crate::{board, bridge, claude, i18n, layout, prompt, t, ui};

#[derive(Debug, Default, Clone)]
pub struct Options {
    /// Each member picks up the last conversation that carries its name.
    pub resume: bool,
    /// The layout only: each pane shows its member and the command it would run.
    pub dry_run: bool,
    /// Do not attach once launched.
    pub detach: bool,
    /// Stop the team first if it is running.
    pub restart: bool,
    /// Print the plan, launch nothing.
    pub print: bool,
}

pub fn launch(found: &Found, cwd: &Path, options: &Options) -> Result<()> {
    let mut options = options.clone();
    let team = &found.team;
    if team.members.is_empty() {
        bail!(t!("l'équipe « {} » n'a aucun membre", "team \"{}\" has no members", found.name));
    }
    let dir = found.work_dir(cwd);
    let mut session = tmux::session_name(&found.name);
    if options.dry_run {
        session.push_str(DRY_RUN);
    }
    let tmux = Tmux::new(&found.tmux)?;

    let mut stopped = false;
    if !options.print && tmux.has_session(&session) {
        let running = tmux.running()?.into_iter().find(|r| r.session == session);
        let running_dir = running.map(|r| r.dir).unwrap_or_default();
        if !running_dir.is_empty() && Path::new(&running_dir) != dir {
            // Two copies would share member names, and messages by name would not know which one to reach.
            bail!(t!(
                "l'équipe « {0} » tourne déjà dans {1}, et une équipe ne tourne qu'à un endroit à la fois. Rejoins-la (recruit attach {0}) ou arrête-la (recruit stop {0}).",
                "team \"{0}\" is already running in {1}, and a team runs in one place at a time. Join it (recruit attach {0}) or stop it (recruit stop {0}).",
                found.name,
                tilde(Path::new(&running_dir))
            ));
        }
        if options.restart || options.dry_run {
            tmux.stop(&session)?;
            stopped = true;
        } else {
            if options.resume {
                eprintln!(
                    "{}",
                    t!(
                        "L'équipe tourne déjà : --resume est ignoré.",
                        "The team is already running: --resume is ignored."
                    )
                );
            }
            println!(
                "{}",
                t!(
                    "L'équipe « {} » tourne déjà : je m'y rattache.",
                    "Team \"{}\" is already running: attaching.",
                    found.name
                )
            );
            if !repair(&tmux, found, &session)? {
                return attach_or_hint(&tmux, &session, &found.name, options.detach);
            }
            // Panes are gone: the team is built again, each member on its conversation.
            tmux.stop(&session)?;
            stopped = true;
            options.resume = true;
        }
    }

    let command = found.claude.command();
    let config_dir = found.claude.config_dir();
    let claude = if options.dry_run || options.print {
        claude::find_executable(command).unwrap_or_else(|| command.into())
    } else {
        let claude = claude::require(command)?;
        if let Some(config_dir) = &config_dir
            && !config_dir.is_dir()
        {
            // Claude Code would make it a new profile, and every member would ask to log in.
            // A local team's settings merge two files: name their folder.
            let source = match found.scope {
                Scope::Local => found.file.parent().unwrap_or(&found.file),
                Scope::Global => &found.file,
            };
            bail!(t!(
                "le profil Claude {} n'existe pas ([claude] config_dir dans {}). Lance claude une fois avec CLAUDE_CONFIG_DIR={} pour le créer.",
                "Claude profile {} does not exist ([claude] config_dir in {}). Run claude once with CLAUDE_CONFIG_DIR={} to create it.",
                tilde(config_dir),
                tilde(source),
                tilde(config_dir)
            ));
        }
        check_duplicates(&claude, found, &dir, config_dir.as_deref(), stopped)?;
        check_trust(&dir, config_dir.as_deref())?;
        claude
    };

    // The mod, when Claude Code can load it: the dashboard's model, context and usage, /equipe and /recruit.
    let plugin_dir = match bridge::claude_version(&claude) {
        Some(version) if version >= bridge::MIN_CLAUDE => Some(bridge::install()?),
        _ => None,
    };

    let resume = if options.resume { claude::last_conversations(&dir, config_dir.as_deref())? } else { HashMap::new() };
    if options.resume {
        for name in team.members.keys().filter(|n| !resume.contains_key(*n)) {
            eprintln!(
                "{}",
                t!(
                    "{} : aucune conversation à reprendre, nouvelle session.",
                    "{}: no conversation to resume, new session.",
                    name
                )
            );
        }
    }

    if !options.print {
        // The prompts of the team's last launch: none of its conversations runs any more.
        let _ = fs::remove_dir_all(prompt::folder(&session));
    }
    let state = state::dir(&session);
    let exe = std::env::current_exe().context("recruit")?.to_string_lossy().into_owned();
    let lang = team.lang.unwrap_or_else(i18n::lang);
    let panel = |kind: &str| panel(&exe, lang, kind, &state);
    let mut tabs = Vec::new();
    let mut commands = Vec::new();
    let mut argvs = HashMap::new();
    let start = claude::Launch {
        claude: &claude,
        team,
        plugin_dir: plugin_dir.as_deref(),
        status_line: claude::status_line(&dir, config_dir.as_deref()),
    };
    for (index, tab) in layout::tabs(team).into_iter().enumerate() {
        let mut panes = Vec::new();
        for name in &tab.members {
            let member = &team.members[name];
            let prompt_file = prompt::write(&session, name, &prompt::build(&found.name, &session, team, name))?;
            let resume = resume.get(name).map(String::as_str);
            let argv = start.argv(name, member, &prompt_file, resume);
            let line = shlex::try_join(argv.iter().map(String::as_str))?;
            // The profile goes through the pane's environment, the shell left after Claude keeps it; shown on the
            // command line all the same.
            let shown = match &config_dir {
                Some(dir) => format!("{}={} {line}", claude::CONFIG_DIR, quote(&dir.to_string_lossy())),
                None => line.clone(),
            };
            let script = if options.dry_run {
                let title = format!("{name} · {}", member.role.lines().next().unwrap_or_default());
                let note = t!(
                    "recruit --dry-run : Claude n'est pas lancé. recruit stop {} ferme l'essai.",
                    "recruit --dry-run: Claude is not started. recruit stop {} closes the trial.",
                    found.name
                );
                // No shell afterwards: its greeting would push the text away.
                format!(
                    "printf '%s\\n\\n%s\\n\\n%s\\n' {} {} {}; while :; do sleep 3600; done",
                    quote(&title),
                    quote(&format!("$ {shown}")),
                    quote(&note)
                )
            } else {
                member_script(&exe, &state, name, false)
            };
            commands.push((name.clone(), shown, prompt_file));
            argvs.insert(name.clone(), argv);
            let env = member_env(&found.name, name, &exe, &state, lang, config_dir.as_deref());
            let argv = vec!["/bin/sh".into(), "-c".into(), script];
            panes.push(Pane { member: name.clone(), role: None, argv, env });
        }
        // The panels go next to the contacts, in the first tab.
        let side = (index == 0 && team.dashboard != Some(false)).then(|| Side {
            dashboard: Pane {
                member: board::dashboard_title(lang).into(),
                role: Some(tmux::DASHBOARD),
                argv: panel("dashboard"),
                env: Vec::new(),
            },
            journal: Pane {
                member: board::journal_title(lang).into(),
                role: Some(tmux::JOURNAL),
                argv: panel("journal"),
                env: Vec::new(),
            },
        });
        tabs.push(TabPlan { title: tab.title, panes, side });
    }
    let plan = Plan {
        session: session.clone(),
        team: found.name.clone(),
        dir: dir.clone(),
        columns: layout::columns(team),
        tabs,
        state: state.clone(),
        // The session's own folder, read by tmux when the key is pressed: the binding serves every team.
        toggle: format!("{} _panel toggle '#{{@recruit_state}}' >/dev/null 2>&1", quote(&exe)),
        click: click_command(&exe),
        menu: menu_command(&exe),
        lang,
    };

    if options.print {
        print_plan(&plan, &commands);
        return Ok(());
    }
    let snapshot = Snapshot {
        team: found.name.clone(),
        session: session.clone(),
        socket: found.tmux.socket().to_string(),
        dir: dir.clone(),
        claude: claude.clone(),
        config_dir: config_dir.clone(),
        lang,
        members: state::members(team, &argvs),
        dashboard: if team.dashboard != Some(false) { panel("dashboard") } else { Vec::new() },
        journal: panel("journal"),
        origin: Some(found.origin()),
        plugin_dir: plugin_dir.clone(),
        status_line: start.status_line,
    };
    snapshot.write(&state)?;
    bridge::reset(&state)?;
    tmux.launch(&plan)?;
    println!(
        "{}",
        t!(
            "Équipe « {} » lancée dans {} : {}, {}.",
            "Team \"{}\" launched in {}: {}, {}.",
            found.name,
            tilde(&dir),
            i18n::count(team.members.len(), "membre", "member"),
            i18n::count(plan.tabs.len(), "onglet", "tab")
        )
    );
    attach_or_hint(&tmux, &session, &found.name, options.detach)
}

/// Suffix of the tmux session of a `--dry-run`, which may run next to the real team.
pub const DRY_RUN: &str = "-dry-run";

/// Once the member's supervisor gives up, the pane keeps a shell open in the same directory.
const SHELL: &str = r#"exec "${SHELL:-/bin/sh}" -l"#;

/// What Alt+r and the bar's button run, read by tmux when the key is pressed or the button clicked: the session's
/// folder and language, the client to show the menu on. `display-popup` reads no format in its command (tmux 3.7c),
/// `run-shell` does: recruit opens the popup itself, with these values.
fn menu_command(exe: &str) -> String {
    format!(
        "{} --lang '#{{@recruit_lang}}' _menu '#{{@recruit_state}}' --client '#{{client_name}}' --popup >/dev/null 2>&1",
        quote(exe)
    )
}

/// The menu's command line in its popup, for one client, `nerd` when its terminal carries the Nerd Font symbols. The
/// popup runs it through the user's shell (`default-shell`): a plain command line.
pub fn menu_line(exe: &str, lang: Lang, state: &Path, client: &str, nerd: bool) -> String {
    format!(
        "{} --lang {} _menu {} --client {}{}",
        quote(exe),
        lang.code(),
        quote(&state.to_string_lossy()),
        quote(client),
        if nerd { " --nerd" } else { "" }
    )
}

/// A panel's command line.
pub fn panel(exe: &str, lang: Lang, kind: &str, state: &Path) -> Vec<String> {
    let state = state.to_string_lossy().into_owned();
    [exe, "--lang", lang.code(), "_panel", kind, &state].map(String::from).to_vec()
}

/// What a click on a panel runs, read by tmux when it happens: the session's folder and language, the pane clicked.
/// The binding serves every team; the language is the team's, for the menu a click may open. Nothing the click
/// landed on: the binding leaves it in the pane's options.
fn click_command(exe: &str) -> String {
    format!(
        "RECRUIT_LANG='#{{@recruit_lang}}' {} _click '#{{@recruit_state}}' '#{{pane_id}}' >/dev/null 2>&1",
        quote(exe)
    )
}

/// What a member's pane runs: `recruit _member`, which starts Claude again when it stops on its own, then the
/// user's shell if it gives up.
/// The script catches SIGINT and does nothing with it: a Ctrl-C during the pause before a restart reaches the whole
/// foreground group, and dash or busybox ash (`/bin/sh` on Debian, Alpine) would end there, the pane with them, rather
/// than go on to the shell. A caught signal goes back to its default in what the script runs, unlike an ignored one:
/// `_member` and Claude get their Ctrl-C as before.
pub fn member_script(exe: &str, state: &Path, name: &str, resume: bool) -> String {
    let resume = if resume { " --resume" } else { "" };
    format!("trap : INT; {} _member{resume} {} {}; {SHELL}", quote(exe), quote(&state.to_string_lossy()), quote(name))
}

/// The environment of a member's pane: who it is; for the mod, who to call, where the team is, which language it
/// speaks; and the Claude profile.
pub fn member_env(
    team: &str,
    name: &str,
    exe: &str,
    state: &Path,
    lang: Lang,
    config_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("RECRUIT_TEAM".into(), team.to_string()),
        ("RECRUIT_MEMBER".into(), name.to_string()),
        ("RECRUIT_EXE".into(), exe.to_string()),
        ("RECRUIT_STATE".into(), state.to_string_lossy().into_owned()),
        ("RECRUIT_LANG".into(), lang.code().into()),
    ];
    if let Some(dir) = config_dir {
        env.push((claude::CONFIG_DIR.into(), dir.to_string_lossy().into_owned()));
    }
    env
}

/// Puts a running team back in shape before joining it: a member whose Claude no longer runs starts again on its
/// conversation, a closed dashboard opens again. True when members' panes are gone and the team should be built
/// again, which the user agreed to.
fn repair(tmux: &Tmux, found: &Found, session: &str) -> Result<bool> {
    let state = state::dir(session);
    // A team launched by an older recruit left nothing to go by.
    let Ok(snapshot) = Snapshot::read(&state) else { return Ok(false) };
    let panes = tmux.panes(session)?;
    let running = claude::running(&snapshot.claude, snapshot.config_dir.as_deref()).unwrap_or_default();
    let alive = |name: &str| {
        running
            .iter()
            .any(|r| r.name.as_deref() == Some(name) && r.cwd.as_deref().map(Path::new) == Some(&snapshot.dir))
    };
    let exe = std::env::current_exe().context("recruit")?.to_string_lossy().into_owned();
    let (mut restarted, mut missing) = (Vec::new(), Vec::new());
    for member in &snapshot.members {
        match panes.iter().find(|p| p.role.is_empty() && p.member == member.name) {
            None => missing.push(member.name.as_str()),
            Some(pane) if !alive(&member.name) && !member.argv.is_empty() => {
                let script = member_script(&exe, &state, &member.name, true);
                let env = member_env(
                    &snapshot.team,
                    &member.name,
                    &exe,
                    &state,
                    snapshot.lang,
                    snapshot.config_dir.as_deref(),
                );
                let argv = vec!["/bin/sh".into(), "-c".into(), script];
                tmux.respawn(&pane.id, &snapshot.dir, &Pane { member: member.name.clone(), role: None, argv, env })?;
                restarted.push(member.name.as_str());
            }
            Some(_) => {}
        }
    }
    if !restarted.is_empty() {
        println!(
            "{}",
            t!(
                "Relancés sur leur conversation : {}.",
                "Started again on their conversations: {}.",
                restarted.join(", ")
            )
        );
    }
    if !snapshot.dashboard.is_empty() && !panes.iter().any(|p| p.role == tmux::DASHBOARD) {
        let dashboard = Pane {
            member: board::dashboard_title(snapshot.lang).into(),
            role: Some(tmux::DASHBOARD),
            argv: snapshot.dashboard.clone(),
            env: Vec::new(),
        };
        tmux.restore_dashboard(session, &snapshot.dir, &dashboard)?;
        println!("{}", t!("Tableau de bord rouvert.", "Dashboard opened again."));
    }
    if missing.is_empty() {
        return Ok(false);
    }
    eprintln!("{}", t!("Panneaux fermés : {}.", "Closed panes: {}.", missing.join(", ")));
    if !ui::interactive() {
        eprintln!(
            "{}",
            t!(
                "Pour reconstruire l'équipe en reprenant les conversations : recruit {} --restart --resume",
                "To build the team again on its conversations: recruit {} --restart --resume",
                found.name
            )
        );
        return Ok(false);
    }
    ui::confirm(
        &t!(
            "Reconstruire l'équipe ? Chaque membre reprend sa conversation.",
            "Build the team again? Each member resumes its conversation."
        ),
        true,
    )
}

fn quote(text: &str) -> String {
    shlex::try_quote(text).map(|q| q.into_owned()).unwrap_or_else(|_| "''".into())
}

pub fn attach_or_hint(tmux: &Tmux, session: &str, team: &str, detach: bool) -> Result<()> {
    if detach || !ui::interactive() {
        println!("{}", t!("Pour la rejoindre : recruit attach {}", "To join it: recruit attach {}", team));
        return Ok(());
    }
    tmux.attach(session)
}

/// A member's session is the one with its name in its directory (member.rs): two there could not be told apart.
/// Elsewhere, the prompt tells members apart by their tmux session, and the dashboard names the duplicates when it
/// has room. ListAgents does not show the tmux socket, though: two teams with one session name on two servers, or a
/// tmux session of the user's with the team's name, still look alike.
fn check_duplicates(
    claude: &Path,
    found: &Found,
    dir: &Path,
    config_dir: Option<&Path>,
    just_stopped: bool,
) -> Result<()> {
    // A team just stopped by --restart takes a moment to close its sessions.
    let attempts = if just_stopped { 10 } else { 1 };
    for attempt in 0..attempts {
        let running = match claude::running(claude, config_dir) {
            Ok(running) => running,
            Err(error) => {
                eprintln!(
                    "{}",
                    t!(
                        "Avertissement : impossible de lister les sessions Claude ouvertes ({error:#}).",
                        "Warning: could not list open Claude sessions ({error:#})."
                    )
                );
                return Ok(());
            }
        };
        let here = open_here(&running, &found.team, dir);
        if here.is_empty() {
            return Ok(());
        }
        if attempt + 1 == attempts {
            bail!(t!(
                "déjà ouvertes dans ce dossier : {}. Ferme-les d'abord, sinon deux sessions porteraient le même nom.",
                "already open in this directory: {}. Close them first, otherwise two sessions would share a name.",
                here.join(", ")
            ));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok(())
}

/// The team's members already open in `dir`.
fn open_here<'a>(running: &'a [claude::Running], team: &Team, dir: &Path) -> Vec<&'a str> {
    running
        .iter()
        .filter(|s| s.cwd.as_deref().is_some_and(|cwd| Path::new(cwd) == dir))
        .filter_map(|s| s.name.as_deref().filter(|n| team.members.contains_key(*n)))
        .collect()
}

/// Claude Code asks every new session whether to trust a folder it has never been approved in, and its default
/// answer is to exit: with a whole team, better know before.
fn check_trust(dir: &Path, config_dir: Option<&Path>) -> Result<()> {
    if claude::trusted(dir, config_dir) != Some(false) {
        return Ok(());
    }
    eprintln!(
        "{}",
        t!(
            "Claude Code n'a pas encore approuvé ce dossier : chaque membre te demandera « Do you trust this folder? », et Entrée seule le ferme (« No, exit »). Choisis « Yes, I trust this folder » dans chaque onglet, ou lance d'abord claude une fois ici pour l'approuver.",
            "Claude Code has not approved this folder yet: every member will ask \"Do you trust this folder?\", and Enter alone closes it (\"No, exit\"). Pick \"Yes, I trust this folder\" in each tab, or run claude here once first to approve it."
        )
    );
    if ui::interactive() && !ui::confirm(&t!("Lancer l'équipe quand même ?", "Launch the team anyway?"), true)? {
        return Err(ui::Cancelled.into());
    }
    Ok(())
}

fn print_plan(plan: &Plan, commands: &[(String, String, std::path::PathBuf)]) {
    println!(
        "{}",
        t!("Session tmux : {} (dossier {})", "tmux session: {} (directory {})", plan.session, tilde(&plan.dir))
    );
    for (i, tab) in plan.tabs.iter().enumerate() {
        let names: Vec<&str> = tab.panes.iter().map(|p| p.member.as_str()).collect();
        let side =
            tab.side.as_ref().map(|s| format!(" + {}, {}", s.dashboard.member, s.journal.member)).unwrap_or_default();
        println!("  {} {}: {}", t!("Onglet", "Tab"), i + 1, format_args!("{} [{}]{side}", tab.title, names.join(", ")));
    }
    for (name, line, prompt) in commands {
        println!("\n# {name}\n{line}\n# {}", t!("prompt : {}", "prompt: {}", tilde(prompt)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ctrl_c_in_the_pause_leaves_the_shell() {
        let script = member_script("/opt/re cruit", Path::new("/tmp/x"), "dev", true);
        assert_eq!(script, r#"trap : INT; '/opt/re cruit' _member --resume /tmp/x dev; exec "${SHELL:-/bin/sh}" -l"#);
        // dash behaves as busybox ash: a Ctrl-C to the group while it waits for `_member` (here a subshell that
        // sends it) ends the script before its shell, unless it is caught. In a process group of its own: the
        // signal reaches the script and what it runs, not the tests.
        if !Path::new("/bin/dash").exists() {
            return;
        }
        use std::os::unix::process::CommandExt;
        let script = "trap : INT; (kill -INT 0; sleep 1); echo after";
        let out = std::process::Command::new("/bin/dash").args(["-c", script]).process_group(0).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("after"));
    }

    #[test]
    fn the_menu_line_reads_back() {
        for nerd in [false, true] {
            let line = menu_line("/opt/re cruit", Lang::Fr, Path::new("/tmp/équipe x"), "/dev/ttys004", nerd);
            let words = shlex::split(&line).expect("a shell line");
            assert_eq!(words[0], "/opt/re cruit");
            let cli = <crate::cli::Cli as clap::Parser>::try_parse_from(&words).unwrap();
            let Some(crate::cli::Command::Menu { state, client, popup, nerd: read }) = cli.command else { panic!() };
            assert_eq!(state, Path::new("/tmp/équipe x"));
            assert_eq!((client.as_deref(), popup, read), (Some("/dev/ttys004"), false, nerd));
        }
    }
    #[test]
    fn click_command_as_bound() {
        let command = click_command("/opt/re cruit/recruit");
        assert_eq!(
            command,
            "RECRUIT_LANG='#{@recruit_lang}' '/opt/re cruit/recruit' _click '#{@recruit_state}' '#{pane_id}' >/dev/null 2>&1"
        );
        // What the members wrote never reaches the shell.
        assert!(!command.contains("mouse_"));
    }

    #[test]
    fn duplicates_only_in_this_directory() {
        let mut team = Team::default();
        team.members.insert("dev".into(), Default::default());
        let open = |name: &str, cwd: Option<&str>| claude::Running {
            name: Some(name.into()),
            cwd: cwd.map(String::from),
            status: None,
            session_id: None,
            pid: None,
        };
        let here = Path::new("/work/projet");
        let running = [
            open("dev", Some("/work/projet")),
            open("dev", Some("/work/autre")),
            open("dev", None),
            open("intrus", Some("/work/projet")),
        ];
        assert_eq!(open_here(&running, &team, here), ["dev"]);
        assert!(open_here(&running[1..], &team, here).is_empty());
    }
}
