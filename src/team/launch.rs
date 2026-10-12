// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Launching a team: its server, the contacts' tab then the working agents' tabs, a Claude session per pane.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::config::{Found, Scope, Team};
use crate::i18n::Lang;
use crate::mux::plan::{self, Pane, Plan, Side, TabPlan};
use crate::mux::{self, Mux, socket};
use crate::sys::tilde;
use crate::team::state::{self, Snapshot};
use crate::{board, claude, claude::bridge, i18n, t, team::layout, team::prompt, ui};

/// In a team's state folder: when its members last resumed after a crash, in seconds since the epoch.
const RECOVERED: &str = "recovered";
/// A crash this soon after a recovery asks before resuming again.
const AGAIN: Duration = Duration::from_secs(10 * 60);

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
    let session = if options.dry_run { trial_session(&found.name) } else { found.name.clone() };
    let state = state::dir(&session);
    // The team's multiplexer stopped without being asked to: the members pick up their conversations (user's
    // decision, spec §11), unless a new start is what is asked for.
    let crashed = !options.print && !options.restart && !options.dry_run && socket::crashed(&state);
    let mux = Mux::of(&session);
    let mut stopped = false;
    if !options.print && mux.is_running() {
        if !stop_or_join(&mux, found, &session, &dir, &mut options)? {
            return Ok(());
        }
        stopped = true;
    }
    if crashed {
        recover(&found.name, &state, &mut options, ui::interactive())?;
    }

    let config_dir = found.claude.config_dir();
    let claude = claude_for(found, &dir, config_dir.as_deref(), &options, stopped)?;
    // The mod, when Claude Code can load it: the dashboard's model, context and usage, /equipe and /recruit.
    let plugin_dir = match claude::claude_version(&claude) {
        Some(version) if version >= claude::MIN_CLAUDE => Some(bridge::install()?),
        _ => None,
    };
    let resume = conversations(team, &dir, config_dir.as_deref(), options.resume)?;
    if !options.print {
        // The prompts of the team's last launch: none of its conversations runs any more.
        let _ = fs::remove_dir_all(prompt::folder(&session));
    }
    let exe = crate::sys::exe()?.to_string_lossy().into_owned();
    let lang = team.language();
    let panes = Panes { exe: &exe, state: &state, team: &found.name, lang, config_dir: config_dir.as_deref() };
    let start = claude::Launch {
        claude: &claude,
        team,
        plugin_dir: plugin_dir.as_deref(),
        status_line: claude::status_line(&dir, config_dir.as_deref()),
    };
    let Built { plan, commands, argvs } = build(found, &session, &dir, &panes, &start, &resume, options.dry_run)?;
    if options.print {
        print_plan(&plan, &commands);
        return Ok(());
    }

    let snapshot = Snapshot {
        team: found.name.clone(),
        session: session.clone(),
        dir: dir.clone(),
        claude: claude.clone(),
        config_dir: config_dir.clone(),
        lang,
        members: state::members(team, &argvs),
        dashboard: if team.dashboard != Some(false) { panes.panel(plan::DASHBOARD).argv } else { Vec::new() },
        journal: panes.panel(plan::JOURNAL).argv,
        origin: found.origin(),
        plugin_dir: plugin_dir.clone(),
        status_line: start.status_line,
        auto_compact: team.auto_compact_at(),
    };
    snapshot.write(&state)?;
    bridge::reset(&state)?;
    // What the last launch's dashboard saw: its sessions, processes and exact addresses are not this launch's.
    let _ = fs::remove_file(state.join(board::STATES));
    mux.launch(&plan)?;
    if crashed && options.resume {
        note_recovery(&state);
    }
    socket::recovered(&state);
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
    attach_or_hint(&mux, &found.name, options.detach)
}

/// The team runs already. In another folder: refused. With --restart, or for a trial: stopped. Otherwise put back in
/// shape and joined, or, when panes are gone and the user agrees, stopped to be built again on its conversations.
/// True when it was stopped, false once joined.
fn stop_or_join(mux: &Mux, found: &Found, session: &str, dir: &Path, options: &mut Options) -> Result<bool> {
    let running = mux::running().into_iter().find(|r| r.session == session);
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
        mux.stop()?;
        return Ok(true);
    }
    if options.resume {
        eprintln!(
            "{}",
            t!("L'équipe tourne déjà : --resume est ignoré.", "The team is already running: --resume is ignored.")
        );
    }
    println!(
        "{}",
        t!("L'équipe « {} » tourne déjà : je m'y rattache.", "Team \"{}\" is already running: attaching.", found.name)
    );
    if !repair(mux, found, session)? {
        attach_or_hint(mux, &found.name, options.detach)?;
        return Ok(false);
    }
    // Panes are gone: the team is built again, each member on its conversation.
    mux.stop()?;
    options.resume = true;
    Ok(true)
}

/// The Claude Code executable the members run. For a launch, checked first: its profile exists, none of the team's
/// members is open in `dir` already, and Claude Code trusts the folder. A trial or a printed plan only looks it up.
fn claude_for(
    found: &Found,
    dir: &Path,
    config_dir: Option<&Path>,
    options: &Options,
    stopped: bool,
) -> Result<PathBuf> {
    let command = found.claude.command();
    if options.dry_run || options.print {
        return Ok(claude::find_executable(command).unwrap_or_else(|| command.into()));
    }
    let claude = claude::require(command)?;
    if let Some(config_dir) = config_dir
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
    check_duplicates(&claude, found, dir, config_dir, stopped)?;
    check_trust(dir, config_dir)?;
    Ok(claude)
}

/// The conversation each member picks up, when `resume`; said for those without one, which start a new session.
fn conversations(team: &Team, dir: &Path, config_dir: Option<&Path>, resume: bool) -> Result<HashMap<String, String>> {
    if !resume {
        return Ok(HashMap::new());
    }
    let found = claude::last_conversations(dir, config_dir)?;
    for name in team.members.keys().filter(|n| !found.contains_key(*n)) {
        eprintln!(
            "{}",
            t!(
                "{} : aucune conversation à reprendre, nouvelle session.",
                "{}: no conversation to resume, new session.",
                name
            )
        );
    }
    Ok(found)
}

/// What a launch builds: its plan, each member's command line as shown (`--print`, `--dry-run`) with its prompt, and
/// the command line of each member's Claude.
struct Built {
    plan: Plan,
    commands: Vec<(String, String, PathBuf)>,
    argvs: HashMap<String, Vec<String>>,
}

/// The tabs of the team, a pane for each member (a trial's shows its command line instead of running it), the
/// panels next to the contacts; each member's prompt written for the session.
fn build(
    found: &Found,
    session: &str,
    dir: &Path,
    panes: &Panes,
    start: &claude::Launch,
    resume: &HashMap<String, String>,
    dry_run: bool,
) -> Result<Built> {
    let team = &found.team;
    let (mut tabs, mut commands, mut argvs) = (Vec::new(), Vec::new(), HashMap::new());
    for (index, tab) in layout::tabs(team).into_iter().enumerate() {
        let mut members = Vec::new();
        for name in &tab.members {
            let member = &team.members[name];
            let prompt_file = prompt::write(session, name, &prompt::build(&found.name, team, name))?;
            let argv = start.argv(name, member, &prompt_file, resume.get(name).map(String::as_str));
            let line = shlex::try_join(argv.iter().map(String::as_str))?;
            // The profile goes through the pane's environment, the shell left after Claude keeps it; shown on the
            // command line all the same.
            let shown = match panes.config_dir {
                Some(dir) => format!("{}={} {line}", claude::CONFIG_DIR, quote(&dir.to_string_lossy())),
                None => line.clone(),
            };
            let pane = if dry_run {
                let title = format!("{name} · {}", member.role.lines().next().unwrap_or_default());
                let note = t!(
                    "recruit --dry-run : Claude n'est pas lancé. recruit stop {} ferme l'essai.",
                    "recruit --dry-run: Claude is not started. recruit stop {} closes the trial.",
                    found.name
                );
                // No shell afterwards: its greeting would push the text away.
                let script = format!(
                    "printf '%s\\n\\n%s\\n\\n%s\\n' {} {} {}; while :; do sleep 3600; done",
                    quote(&title),
                    quote(&format!("$ {shown}")),
                    quote(&note)
                );
                panes.with_script(name, script)
            } else {
                panes.member(name, false)
            };
            commands.push((name.clone(), shown, prompt_file));
            argvs.insert(name.clone(), argv);
            members.push(pane);
        }
        // The panels go next to the contacts, in the first tab.
        let side = (index == 0 && team.dashboard != Some(false))
            .then(|| Side { dashboard: panes.panel(plan::DASHBOARD), journal: panes.panel(plan::JOURNAL) });
        tabs.push(TabPlan { title: tab.title, panes: members, side });
    }
    let plan = Plan {
        team: found.name.clone(),
        dir: dir.to_path_buf(),
        columns: layout::columns(team),
        focus: team.layout == Some(crate::config::Layout::Focus),
        tabs,
        lang: panes.lang,
    };
    Ok(Built { plan, commands, argvs })
}

/// Suffix of the session of a `--dry-run`, which may run next to the real team.
pub const DRY_RUN: &str = "-dry-run";

/// The session of a team's `--dry-run`.
pub fn trial_session(team: &str) -> String {
    format!("{team}{DRY_RUN}")
}

/// Once the member's supervisor gives up, the pane keeps a shell open in the same directory.
const SHELL: &str = r#"exec "${SHELL:-/bin/sh}" -l"#;

/// A team whose multiplexer crashed: each member resumes its conversation, said so. A second crash soon after the last
/// recovery ([`note_recovery`]) asks first, when `interactive` (a conversation may be what brings the multiplexer
/// down); without a terminal, it stops there.
fn recover(team: &str, state: &Path, options: &mut Options, interactive: bool) -> Result<()> {
    let now = crate::sys::now() as u64;
    let last = fs::read_to_string(state.join(RECOVERED)).ok().and_then(|text| text.trim().parse::<u64>().ok());
    let again = last.is_some_and(|last| now.saturating_sub(last) < AGAIN.as_secs());
    if !options.resume {
        if again {
            if !interactive {
                bail!(t!(
                    "l'équipe « {0} » s'est de nouveau arrêtée brutalement, peu après la dernière reprise. Relance-la depuis un terminal pour choisir, ou avec --resume pour reprendre les conversations.",
                    "team \"{0}\" stopped abruptly again, soon after the last recovery. Launch it from a terminal to choose, or with --resume to resume the conversations.",
                    team
                ));
            }
            options.resume = ui::confirm(
                &t!(
                    "L'équipe « {} » s'est de nouveau arrêtée brutalement, peu après la dernière reprise. Reprendre encore les conversations ?",
                    "Team \"{}\" stopped abruptly again, soon after the last recovery. Resume the conversations again?",
                    team
                ),
                true,
            )?;
        } else {
            println!(
                "{}",
                t!(
                    "L'équipe « {} » s'est arrêtée brutalement : chaque membre reprend sa conversation.",
                    "Team \"{}\" stopped abruptly: each member resumes its conversation.",
                    team
                )
            );
            options.resume = true;
        }
    }
    Ok(())
}

/// The team is running again on its conversations after a crash: noted for the next one, once launched (a launch
/// that fails is not a recovery). The time in the file, not the file's: it may come before now on Linux (CLAUDE.md).
fn note_recovery(state: &Path) {
    let now = crate::sys::now() as u64;
    let _ = crate::sys::write_atomic(&state.join(RECOVERED), &now.to_string());
}

/// A panel's command line.
pub fn panel(exe: &str, lang: Lang, kind: &str, state: &Path) -> Vec<String> {
    let state = state.to_string_lossy().into_owned();
    [exe, "--lang", lang.code(), "_panel", kind, &state].map(String::from).to_vec()
}

/// What a member's pane runs: `recruit _member`, which starts Claude again when it stops on its own, then the
/// user's shell if it gives up. The script catches SIGINT and does nothing with it: a Ctrl-C during the pause before
/// a restart reaches the whole foreground group, and dash or busybox ash (`/bin/sh` on Debian, Alpine) would end
/// there, the pane with them, rather than go on to the shell. A caught signal goes back to its default in what the
/// script runs, unlike an ignored one: `_member` and Claude get their Ctrl-C as before.
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

/// What a team's panes are started with: recruit's executable, the team's state folder, its name, its language and
/// its Claude profile.
pub struct Panes<'a> {
    pub exe: &'a str,
    pub state: &'a Path,
    pub team: &'a str,
    pub lang: Lang,
    pub config_dir: Option<&'a Path>,
}

impl<'a> Panes<'a> {
    /// The panes of a running team.
    pub fn of(snapshot: &'a Snapshot, exe: &'a str, state: &'a Path) -> Self {
        Panes { exe, state, team: &snapshot.team, lang: snapshot.lang, config_dir: snapshot.config_dir.as_deref() }
    }

    /// A member's pane: `recruit _member`, on its conversation when `resume` (`member_script`).
    pub fn member(&self, name: &str, resume: bool) -> Pane {
        self.with_script(name, member_script(self.exe, self.state, name, resume))
    }

    /// A member's pane that runs `script` (a trial's), with the member's environment.
    fn with_script(&self, name: &str, script: String) -> Pane {
        Pane {
            member: name.to_string(),
            role: None,
            argv: vec!["/bin/sh".into(), "-c".into(), script],
            env: member_env(self.team, name, self.exe, self.state, self.lang, self.config_dir),
        }
    }

    /// The pane of a panel: `plan::DASHBOARD` or `plan::JOURNAL`.
    pub fn panel(&self, role: &'static str) -> Pane {
        let title =
            if role == plan::DASHBOARD { board::dashboard_title(self.lang) } else { board::journal_title().into() };
        Pane { member: title, role: Some(role), argv: panel(self.exe, self.lang, role, self.state), env: Vec::new() }
    }
}

/// Puts a running team back in shape before joining it: a member whose Claude no longer runs starts again on its
/// conversation, a closed dashboard opens again. True when members' panes are gone and the team should be built
/// again, which the user agreed to.
fn repair(mux: &Mux, found: &Found, session: &str) -> Result<bool> {
    let state = state::dir(session);
    // Without its team.json, nothing to go by.
    let Ok(snapshot) = Snapshot::read(&state) else { return Ok(false) };
    let panes = mux.panes()?;
    let running = claude::running(&snapshot.claude, snapshot.config_dir.as_deref()).unwrap_or_default();
    let alive = |name: &str| {
        running
            .iter()
            .any(|r| r.name.as_deref() == Some(name) && r.cwd.as_deref().map(Path::new) == Some(&snapshot.dir))
    };
    let exe = crate::sys::exe()?.to_string_lossy().into_owned();
    let starts = Panes::of(&snapshot, &exe, &state);
    let (mut restarted, mut missing) = (Vec::new(), Vec::new());
    for member in &snapshot.members {
        match panes.iter().find(|p| p.role.is_empty() && p.member == member.name) {
            None => missing.push(member.name.as_str()),
            Some(pane) if !alive(&member.name) && !member.argv.is_empty() => {
                mux.respawn(&pane.id, &snapshot.dir, &starts.member(&member.name, true))?;
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
    if !snapshot.dashboard.is_empty() && !panes.iter().any(|p| p.role == plan::DASHBOARD) {
        // The command line it was launched with.
        let dashboard = Pane { argv: snapshot.dashboard.clone(), ..starts.panel(plan::DASHBOARD) };
        mux.restore_dashboard(&snapshot.dir, &dashboard)?;
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

pub fn attach_or_hint(mux: &Mux, team: &str, detach: bool) -> Result<()> {
    if detach || !ui::interactive() {
        println!("{}", t!("Pour la rejoindre : recruit attach {}", "To join it: recruit attach {}", team));
        return Ok(());
    }
    mux.attach()
}

/// A member's session is the one with its name in its directory (member.rs): two there could not be told apart.
/// Elsewhere, recruit gives each member the exact addresses of its teammates whose name another session carries
/// (`live::addresses`), and the dashboard names the duplicates when it has room.
fn check_duplicates(
    claude: &Path,
    found: &Found,
    dir: &Path,
    config_dir: Option<&Path>,
    just_stopped: bool,
) -> Result<()> {
    // A team just stopped, by --restart or to be built again, takes a moment to close its sessions.
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
        let here = open_in(&running, dir, |name| found.team.members.contains_key(name));
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

/// The names of the Claude sessions open in `dir` that `taken` says are taken: a team's members, or the names a
/// change gives.
pub fn open_in<'a>(running: &'a [claude::Running], dir: &Path, taken: impl Fn(&str) -> bool) -> Vec<&'a str> {
    running
        .iter()
        .filter(|s| s.cwd.as_deref().is_some_and(|cwd| Path::new(cwd) == dir))
        .filter_map(|s| s.name.as_deref().filter(|n| taken(n)))
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

fn print_plan(plan: &Plan, commands: &[(String, String, PathBuf)]) {
    println!("{}", t!("Équipe « {} » (dossier {})", "Team \"{}\" (directory {})", plan.team, tilde(&plan.dir)));
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
    fn a_crash_resumes_then_asks() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        let mut options = Options::default();
        recover("x", state, &mut options, false).unwrap();
        assert!(options.resume, "the first crash resumes without asking");
        assert!(!state.join(RECOVERED).exists(), "noted once launched only");
        note_recovery(state);
        assert!(fs::read_to_string(state.join(RECOVERED)).unwrap().parse::<u64>().is_ok());
        // Soon after: asked, and without a terminal to ask on, nothing starts.
        let mut options = Options::default();
        assert!(recover("x", state, &mut options, false).is_err());
        // Unless --resume says so.
        let mut options = Options { resume: true, ..Default::default() };
        recover("x", state, &mut options, false).unwrap();
        assert!(options.resume);
        // Long after: as the first time.
        fs::write(state.join(RECOVERED), "1000").unwrap();
        let mut options = Options::default();
        recover("x", state, &mut options, false).unwrap();
        assert!(options.resume);
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
        let taken = |name: &str| team.members.contains_key(name);
        assert_eq!(open_in(&running, here, taken), ["dev"]);
        assert!(open_in(&running[1..], here, taken).is_empty());
    }
}
