// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What runs in a member's pane: its Claude session, started again on the same conversation when it stops on its
//! own (`/exit`, a crash), so that the member keeps its name, its role and its history. Two seconds first, in
//! which Ctrl-C leaves a shell instead; nothing again when the team is stopped, nor after two failed starts, nor
//! once the member left the team. Each start again takes the member's settings as the team's files give them then.

use std::fs;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::state::{self, MemberInfo, Snapshot};
use crate::{board, claude, live, t};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static HUNG_UP: AtomicBool = AtomicBool::new(false);

/// A Claude that stops this soon after it started did not get going.
const QUICK: Duration = Duration::from_secs(10);
/// The time to choose a shell instead.
const PAUSE: Duration = Duration::from_secs(2);

extern "C" fn on_signal(signal: libc::c_int) {
    let flag = if signal == libc::SIGINT { &INTERRUPTED } else { &HUNG_UP };
    flag.store(true, Ordering::SeqCst);
}

/// Caught rather than ignored: Claude, started afterwards, gets the default dispositions back.
fn catch_signals() {
    for signal in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
        // SAFETY: the handler only stores into atomics.
        unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
    }
}

/// `resume`: start on the member's last conversation, as when its pane is started again.
pub fn run(state: &Path, member: &str, resume: bool) -> Result<()> {
    let snapshot = Snapshot::read(state)?;
    let mut info = snapshot
        .members
        .iter()
        .find(|m| m.name == member && !m.argv.is_empty())
        .cloned()
        .with_context(|| t!("aucun membre « {} » dans {}", "no member \"{}\" in {}", member, state.display()))?;
    catch_signals();
    // The first start as recruit just wrote it: launched, or started again from the menu.
    let mut argv = first_argv(&snapshot, state, &info, resume);
    let mut quick = 0;
    loop {
        let started = Instant::now();
        let status =
            Command::new(&argv[0]).args(&argv[1..]).spawn().and_then(|child| watch(child, &snapshot, state, &info));
        clear_status();
        let ended_by_team =
            status.as_ref().is_ok_and(|s| matches!(s.signal(), Some(libc::SIGHUP | libc::SIGTERM | libc::SIGKILL)));
        if HUNG_UP.load(Ordering::SeqCst) || ended_by_team {
            return Ok(());
        }
        if let Err(error) = status {
            eprintln!("recruit: {}: {error}", argv[0]);
            return Ok(());
        }
        quick = if started.elapsed() < QUICK { quick + 1 } else { 0 };
        if quick >= 2 {
            eprintln!(
                "\n{}",
                t!(
                    "Claude s'est arrêté deux fois juste après son démarrage : je ne le relance plus. Une fois la cause corrigée, recruit {} --restart --resume relance l'équipe.",
                    "Claude stopped twice just after starting: not starting it again. Once the cause is fixed, recruit {} --restart --resume starts the team again.",
                    snapshot.team
                )
            );
            return Ok(());
        }
        INTERRUPTED.store(false, Ordering::SeqCst);
        println!(
            "\n{}",
            t!(
                "Claude s'est arrêté : reprise de la conversation de {} dans 2 s. Ctrl-C pour un shell à la place.",
                "Claude stopped: resuming {}'s conversation in 2 s. Ctrl-C for a shell instead.",
                member
            )
        );
        if !pause() {
            println!(
                "{}",
                t!(
                    "Reprise annulée. Relancer recruit remet en route les membres arrêtés.",
                    "Resume cancelled. Running recruit again restarts the stopped members."
                )
            );
            return Ok(());
        }
        let Some(next) = prepare(state, member) else {
            println!("{}", t!("{} ne fait plus partie de l'équipe.", "{} is no longer in the team.", member));
            return Ok(());
        };
        info = next;
        argv = resumed(&snapshot, state, &info);
    }
}

/// Ends the program status Claude gave its terminal (OSC 7501), whatever stopped it, a crash included: the native
/// multiplexer then shows the pane without a state, and a status that a later program writes (a shell replaying old
/// output) no longer counts.
fn clear_status() {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(b"\x1b]7501;state=clear:app=recruit\x07");
    let _ = out.flush();
}

/// Before a member starts again: its command line built again from the team's files (its model, its effort, its
/// permission mode may have changed), noted in team.json for the mod. None once the member left the team. When the
/// files cannot be read, the command line it last started with.
fn prepare(state: &Path, member: &str) -> Option<MemberInfo> {
    let _lock = state::lock(state).ok();
    let mut snapshot = Snapshot::read(state).ok()?;
    let index = snapshot.members.iter().position(|m| m.name == member)?;
    let found = snapshot.origin.as_ref().and_then(|origin| origin.load(&snapshot.team).ok());
    if let Some(found) = found.filter(|f| f.team.members.contains_key(member))
        && let Ok(argv) = live::fresh_argv(&snapshot, &found, member)
    {
        let argvs = std::collections::HashMap::from([(member.to_string(), argv)]);
        let fresh = state::members(&found.team, &argvs).into_iter().find(|m| m.name == member)?;
        if snapshot.members[index] != fresh {
            snapshot.members[index] = fresh;
            let _ = snapshot.write(state);
        }
    }
    Some(snapshot.members[index].clone())
}

/// Waits `PAUSE`: false when Ctrl-C came, or the team stopped, meanwhile.
fn pause() -> bool {
    let until = Instant::now() + PAUSE;
    while Instant::now() < until {
        if INTERRUPTED.load(Ordering::SeqCst) || HUNG_UP.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// How often the session is read in what the dashboard wrote, while it looks: a file read, without backing off.
const READ: Duration = Duration::from_secs(2);

/// How far apart `claude agents` is asked at most, when the dashboard does not look.
const SLOWEST: Duration = Duration::from_secs(60);

/// How long a session may stay missing from what the dashboard writes before `claude agents` is asked all the same.
const MISSING: Duration = Duration::from_secs(60);

/// Waits for Claude to stop, noting meanwhile which session it runs: the one to resume, even before it carries a
/// title, and the latest one after a `/clear`.
fn watch(mut child: Child, snapshot: &Snapshot, state: &Path, info: &MemberInfo) -> std::io::Result<ExitStatus> {
    let started = Instant::now();
    let mut pace = Pace::new();
    let mut next = READ;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() >= next {
            note_session(snapshot, state, info, child.id(), &mut pace, started.elapsed());
            next = started.elapsed() + READ;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// What the dashboard says of a member's session.
#[derive(Debug, PartialEq)]
enum Seen {
    /// The session its Claude runs.
    Session(String),
    /// It looks, but has not noted that Claude's session: not seen yet, or the one before (a relaunch).
    Missing,
    /// It does not look: no file, or older than `board::FRESH` (`dashboard = false`, the dashboard stopped).
    Stale,
}

/// What `states` (fresh, if any) says of `member`'s session, run by Claude's process `pid`: an entry of the Claude
/// before it (a few seconds old at most) would resume the wrong conversation.
fn seen(states: Option<&board::States>, member: &str, pid: u32) -> Seen {
    let Some(states) = states else { return Seen::Stale };
    match states.sessions.get(member) {
        Some(session) if session.pid == Some(pid) => Seen::Session(session.id.clone()),
        _ => Seen::Missing,
    }
}

/// When to ask `claude agents --json` (0.2 s of CPU and 160 MB a call, a dozen members at once, 2.0.1): never while
/// the dashboard notes the session; while it does not look, at 2 s, then less and less often, a minute apart at
/// most; while it looks without noting it, once a minute.
struct Pace {
    /// The wait after the last ask, while the dashboard does not look.
    backoff: Duration,
    /// The next ask while it does not look, since the start.
    next: Duration,
    /// Since when the session is missing from a fresh file.
    missing: Option<Duration>,
}

impl Pace {
    fn new() -> Self {
        Pace { backoff: READ, next: READ, missing: None }
    }

    /// Whether to ask, at `at` since the start, after what the dashboard said.
    fn ask(&mut self, seen: &Seen, at: Duration) -> bool {
        match seen {
            Seen::Session(_) => {
                self.missing = None;
                false
            }
            Seen::Missing => {
                let since = *self.missing.get_or_insert(at);
                let ask = at.saturating_sub(since) >= MISSING;
                if ask {
                    self.missing = Some(at);
                }
                ask
            }
            Seen::Stale => {
                self.missing = None;
                let ask = at >= self.next;
                if ask {
                    self.backoff = (self.backoff * 2).min(SLOWEST);
                    self.next = at + self.backoff;
                }
                ask
            }
        }
    }
}

/// Notes the session the member's Claude (process `pid`) runs: read in Claude's own file in the profile, else in
/// what the dashboard last wrote while it looks, else asked to `claude agents` (by name and folder) when [`Pace`]
/// says so. True when it asked.
fn note_session(snapshot: &Snapshot, state: &Path, info: &MemberInfo, pid: u32, pace: &mut Pace, at: Duration) -> bool {
    // Claude's own file first (a file read, its process by construction), then what the dashboard wrote.
    let seen = match claude::session_id(snapshot.config_dir.as_deref(), pid) {
        Some(id) => Seen::Session(id),
        None => seen(board::fresh_states(state).as_ref(), &info.name, pid),
    };
    let ask = pace.ask(&seen, at);
    let id = match seen {
        Seen::Session(id) => Some(id),
        _ if ask => asked(snapshot, info),
        _ => None,
    };
    if let Some(id) = id {
        note(state, &info.name, &id);
    }
    ask
}

/// The session the member runs, as `claude agents` says it.
fn asked(snapshot: &Snapshot, info: &MemberInfo) -> Option<String> {
    let running = claude::running(&snapshot.claude, snapshot.config_dir.as_deref()).ok()?;
    let here = |r: &&claude::Running| {
        r.name.as_deref() == Some(info.name.as_str()) && r.cwd.as_deref().map(Path::new) == Some(snapshot.dir.as_path())
    };
    running.into_iter().find(|r| here(&r)).and_then(|r| r.session_id)
}

/// Notes `id` as the member's session, if it is not already.
fn note(state: &Path, member: &str, id: &str) {
    let file = session_file(state, member);
    if fs::read_to_string(&file).ok().as_deref() != Some(id) {
        let _ = fs::create_dir_all(file.parent().expect("a file in a folder"));
        let _ = fs::write(&file, id);
    }
}

/// Where the session a member last ran is noted.
fn session_file(state: &Path, member: &str) -> PathBuf {
    state.join("sessions").join(member)
}

/// The session a member last ran, as noted: none when it never ran, or the note is empty.
pub fn noted(state: &Path, member: &str) -> Option<String> {
    let id = fs::read_to_string(session_file(state, member)).ok()?;
    Some(id.trim().to_string()).filter(|id| !id.is_empty())
}

/// The command line of the first start: on the conversation last noted (`resume`), else as launched. Launched on a
/// conversation (`-r <id>`: a launch that resumes, after a crash, from recruit 1), that one is noted; launched
/// afresh, the noted one is forgotten. Either way, a Claude that stops before its own is noted starts again where it
/// started, not on an older conversation, nor afresh.
fn first_argv(snapshot: &Snapshot, state: &Path, info: &MemberInfo, resume: bool) -> Vec<String> {
    if resume {
        return resumed(snapshot, state, info);
    }
    match resumes(&info.argv) {
        Some(id) => note(state, &info.name, id),
        None => {
            let _ = fs::remove_file(session_file(state, &info.name));
        }
    }
    info.argv.clone()
}

/// The conversation a command line resumes: the `-r <id>` right after `-n <name>`, where `claude::Start::argv` and
/// [`resume_argv`] put it (one in the team's or the member's own arguments, before `-n`, is not recruit's).
fn resumes(argv: &[String]) -> Option<&str> {
    let n = argv.iter().rposition(|a| a == "-n")?;
    match argv.get(n + 2..n + 4)? {
        [flag, id] if flag == "-r" && !id.is_empty() => Some(id.as_str()),
        _ => None,
    }
}

/// The member's command line on the session it last ran; as launched when that session never got a word (no
/// conversation file), or is unknown.
fn resumed(snapshot: &Snapshot, state: &Path, info: &MemberInfo) -> Vec<String> {
    let id = fs::read_to_string(session_file(state, &info.name)).unwrap_or_default();
    let used = !id.is_empty()
        && claude::transcript(&snapshot.dir, snapshot.config_dir.as_deref(), &id).metadata().is_ok_and(|m| m.len() > 0);
    if used { resume_argv(&info.argv, &id) } else { info.argv.clone() }
}

/// The launch command line on conversation `id`: what follows `-n <name>` (the prompt, or an earlier `-r`) gives
/// way to `-r <id>`. A resumed conversation keeps its prompt.
pub fn resume_argv(argv: &[String], id: &str) -> Vec<String> {
    let end = argv.iter().rposition(|a| a == "-n").map_or(argv.len(), |i| (i + 2).min(argv.len()));
    let mut resumed = argv[..end].to_vec();
    resumed.extend(["-r".to_string(), id.to_string()]);
    resumed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_session_is_read_where_the_dashboard_wrote_it_for_this_claude() {
        let mut states = board::States::default();
        assert_eq!(seen(None, "dev", 7), Seen::Stale, "the dashboard does not look");
        assert_eq!(seen(Some(&states), "dev", 7), Seen::Missing, "not seen yet");
        states.sessions.insert("dev".into(), board::Session { id: "s-1".into(), pid: Some(7) });
        assert_eq!(seen(Some(&states), "dev", 7), Seen::Session("s-1".into()));
        assert_eq!(seen(Some(&states), "dev", 8), Seen::Missing, "the Claude before: not its conversation");
        assert_eq!(seen(Some(&states), "qa", 7), Seen::Missing);
        states.sessions.insert("dev".into(), board::Session { id: "s-1".into(), pid: None });
        assert_eq!(seen(Some(&states), "dev", 7), Seen::Missing, "no process said: not sure");
    }

    #[test]
    fn claude_agents_is_asked_only_when_the_dashboard_cannot_tell() {
        let s = Duration::from_secs;
        let session = Seen::Session("s".into());
        // The dashboard looks and notes it: a file read every 2 s, never an ask.
        let mut pace = Pace::new();
        assert!((1..=200).all(|i| !pace.ask(&session, s(2 * i))));
        // It does not look: at 2 s, then 4, 8, 16, 32, then a minute apart.
        let mut pace = Pace::new();
        let asks: Vec<u64> =
            (1..=100).map(|i| s(2 * i)).filter(|at| pace.ask(&Seen::Stale, *at)).map(|at| at.as_secs()).collect();
        assert_eq!(asks, [2, 6, 14, 30, 62, 122, 182]);
        // One stale read amid fresh ones does not ask at once: its own pace.
        let mut pace = Pace::new();
        assert!(pace.ask(&Seen::Stale, s(2)));
        assert!(!pace.ask(&session, s(4)));
        assert!(!pace.ask(&Seen::Stale, s(5)));
        assert!(pace.ask(&Seen::Stale, s(6)));
        // It looks without noting it: an ask once it has been missing a minute, then a minute later.
        let mut pace = Pace::new();
        let asks: Vec<u64> =
            (1..=70).map(|i| s(2 * i)).filter(|at| pace.ask(&Seen::Missing, *at)).map(|at| at.as_secs()).collect();
        assert_eq!(asks, [62, 122]);
        // Found meanwhile: the minute starts over.
        let mut pace = Pace::new();
        assert!(!pace.ask(&Seen::Missing, s(2)));
        assert!(!pace.ask(&session, s(50)));
        assert!(!pace.ask(&Seen::Missing, s(70)));
        assert!(pace.ask(&Seen::Missing, s(130)));
    }

    #[test]
    fn a_fresh_states_file_spares_claude_agents() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        // `claude agents` fails here (a missing executable): asked, it notes nothing. A profile of its own, without
        // Claude's file: the fallbacks.
        let profile = tempfile::tempdir().unwrap();
        let snapshot = Snapshot {
            claude: "/nonexistent/claude".into(),
            dir: "/".into(),
            config_dir: Some(profile.path().into()),
            ..Default::default()
        };
        let info = MemberInfo { name: "dev".into(), ..Default::default() };
        let mut pace = Pace::new();
        let at = Duration::from_secs(2);
        assert!(note_session(&snapshot, state, &info, 7, &mut pace, at), "no file: asked");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let mut states = board::States { at: now, ..Default::default() };
        states.sessions.insert("dev".into(), board::Session { id: "s-2".into(), pid: Some(7) });
        let write =
            |states: &board::States| fs::write(state.join(board::STATES), serde_json::to_string(states).unwrap());
        write(&states).unwrap();
        assert!(!note_session(&snapshot, state, &info, 7, &mut pace, at * 2));
        assert_eq!(noted(state, "dev").as_deref(), Some("s-2"));
        // Another Claude's entry: not noted.
        assert!(!note_session(&snapshot, state, &info, 8, &mut pace, at * 3));
        assert_eq!(noted(state, "dev").as_deref(), Some("s-2"));
        // Too old: the dashboard no longer looks; not noted from it.
        states.at = now - board::FRESH - 5;
        states.sessions.insert("dev".into(), board::Session { id: "s-3".into(), pid: Some(7) });
        write(&states).unwrap();
        note_session(&snapshot, state, &info, 7, &mut pace, at * 4);
        assert_eq!(noted(state, "dev").as_deref(), Some("s-2"), "kept");
        // Claude's own file: first, whatever the dashboard says, and never an ask.
        let sessions = profile.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("7.json"), r#"{"pid":7,"sessionId":"s-4","cwd":"/","name":"dev","status":"idle"}"#)
            .unwrap();
        assert!(!note_session(&snapshot, state, &info, 7, &mut pace, at * 50));
        assert_eq!(noted(state, "dev").as_deref(), Some("s-4"));
        // Another process's file: not its session.
        fs::write(sessions.join("9.json"), r#"{"pid":8,"sessionId":"s-5"}"#).unwrap();
        states.at = now;
        write(&states).unwrap();
        note_session(&snapshot, state, &info, 9, &mut pace, at * 51);
        assert_eq!(noted(state, "dev").as_deref(), Some("s-4"));
    }

    #[test]
    fn a_fresh_start_forgets_the_conversation_noted() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        let profile = tempfile::tempdir().unwrap();
        let snapshot = Snapshot { dir: "/w".into(), config_dir: Some(profile.path().into()), ..Default::default() };
        let launched = strings(&["claude", "-n", "dev", "--append-system-prompt-file", "/p/01.md"]);
        let info = MemberInfo { name: "dev".into(), argv: launched.clone(), ..Default::default() };
        // A conversation noted, that said something.
        note(state, "dev", "old");
        let transcript = claude::transcript(&snapshot.dir, snapshot.config_dir.as_deref(), "old");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(&transcript, "{}\n").unwrap();
        assert_eq!(first_argv(&snapshot, state, &info, true), strings(&["claude", "-n", "dev", "-r", "old"]));
        assert_eq!(noted(state, "dev").as_deref(), Some("old"), "resumed: kept");
        // Afresh: forgotten, so that a quick stop starts afresh again.
        assert_eq!(first_argv(&snapshot, state, &info, false), launched);
        assert_eq!(noted(state, "dev"), None);
        assert_eq!(resumed(&snapshot, state, &info), launched);
        // Launched on a conversation (`--resume`, after a crash): that one noted, so that a quick stop resumes it.
        let on = strings(&["claude", "--model", "opus", "-n", "dev", "-r", "old2"]);
        let info = MemberInfo { argv: on.clone(), ..info };
        let transcript = claude::transcript(&snapshot.dir, snapshot.config_dir.as_deref(), "old2");
        fs::write(&transcript, "{}\n").unwrap();
        assert_eq!(first_argv(&snapshot, state, &info, false), on);
        assert_eq!(noted(state, "dev").as_deref(), Some("old2"));
        assert_eq!(resumed(&snapshot, state, &info), on);
    }

    #[test]
    fn the_conversation_a_command_line_resumes() {
        let argv = |list: &[&str]| strings(list);
        assert_eq!(resumes(&argv(&["claude", "-n", "dev", "-r", "id-1"])), Some("id-1"));
        assert_eq!(
            resumes(&resume_argv(&argv(&["claude", "-n", "dev", "--append-system-prompt-file", "/p"]), "id-2")),
            Some("id-2")
        );
        // Afresh, a `-r` in the team's own arguments, a name without anything after, a bare `-r`: none.
        assert_eq!(resumes(&argv(&["claude", "-n", "dev", "--append-system-prompt-file", "/p"])), None);
        assert_eq!(resumes(&argv(&["claude", "-r", "x", "-n", "dev", "--append-system-prompt-file", "/p"])), None);
        assert_eq!(resumes(&argv(&["claude", "-n", "dev"])), None);
        assert_eq!(resumes(&argv(&["claude", "-n", "dev", "-r"])), None);
        assert_eq!(resumes(&argv(&["claude"])), None);
    }

    #[test]
    fn resuming() {
        let fresh = strings(&["claude", "--model", "opus", "-n", "dev", "--append-system-prompt-file", "/p/02.md"]);
        assert_eq!(resume_argv(&fresh, "id-2"), strings(&["claude", "--model", "opus", "-n", "dev", "-r", "id-2"]));
        let resumed = strings(&["claude", "-n", "dev", "-r", "id-1"]);
        assert_eq!(resume_argv(&resumed, "id-2"), strings(&["claude", "-n", "dev", "-r", "id-2"]));
    }
}
