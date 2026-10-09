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
use crate::{claude, live, t};

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
    let mut argv = if resume { resumed(&snapshot, state, &info) } else { info.argv.clone() };
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
/// output) no longer counts. tmux ignores the sequence.
fn clear_status() {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(b"\x1b]7501;state=clear\x07");
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

/// Waits for Claude to stop, noting meanwhile which session it runs (`claude agents --json`, by name and folder):
/// the one to resume, even before it carries a title, and the latest one after a `/clear`.
fn watch(mut child: Child, snapshot: &Snapshot, state: &Path, info: &MemberInfo) -> std::io::Result<ExitStatus> {
    let started = Instant::now();
    // At 2 s, then less and less often, a minute apart at most.
    let (mut interval, mut next) = (Duration::from_secs(2), Duration::from_secs(2));
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() >= next {
            note_session(snapshot, state, info);
            interval = (interval * 2).min(Duration::from_secs(60));
            next = started.elapsed() + interval;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn note_session(snapshot: &Snapshot, state: &Path, info: &MemberInfo) {
    let Ok(running) = claude::running(&snapshot.claude, snapshot.config_dir.as_deref()) else { return };
    let here = |r: &&claude::Running| {
        r.name.as_deref() == Some(info.name.as_str()) && r.cwd.as_deref().map(Path::new) == Some(snapshot.dir.as_path())
    };
    if let Some(id) = running.iter().find(here).and_then(|r| r.session_id.as_deref()) {
        let file = session_file(state, &info.name);
        if fs::read_to_string(&file).ok().as_deref() != Some(id) {
            let _ = fs::create_dir_all(file.parent().expect("a file in a folder"));
            let _ = fs::write(&file, id);
        }
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
    fn resuming() {
        let fresh = strings(&["claude", "--model", "opus", "-n", "dev", "--append-system-prompt-file", "/p/02.md"]);
        assert_eq!(resume_argv(&fresh, "id-2"), strings(&["claude", "--model", "opus", "-n", "dev", "-r", "id-2"]));
        let resumed = strings(&["claude", "-n", "dev", "-r", "id-1"]);
        assert_eq!(resume_argv(&resumed, "id-2"), strings(&["claude", "-n", "dev", "-r", "id-2"]));
    }
}
