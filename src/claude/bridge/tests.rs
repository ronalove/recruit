// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The bridge's tests: the mod's events as it sends them, and what each leaves in the team's state folder.

use std::fs;
use std::path::Path;

use anyhow::bail;
use serde_json::{Value, json};

use super::command::{command_text, menu_said};
use super::doing::{LINE_CHARS, REQUEST_CHARS, SUMMARY_MODEL, Task, doing, task_of, turn_lines, turn_request};
use super::files::{compactions, forget, prompted, rename, reports, reset, resumes};
use super::helpers::{Helper, HelperStatus, TEAMMATE_DROPPED, TEAMMATE_UNKNOWN, helpers, helpers_now, restarted};
use super::install::FILES;
use super::models::{Override, applies_live, model_and_effort, model_id, override_of};
use super::prompt::{prompted_file, prompted_of, started, submit};
use super::report::{Compaction, Report, Threshold, report};
use super::requests::{
    COMPACTED_AFTER, COMPACTION_LAPSE, RESUME_ANSWER, SKIP_HELPERS, request_auto_compaction, request_compaction,
    request_resume, resumption, resumption_at,
};
use super::tick::{start, tick};
use crate::i18n::Lang;
use crate::mux::proto::MenuOpened;
use crate::sys::write_atomic;
use crate::team::state::{MemberInfo, Snapshot};
use crate::{board, claude, t, ui::look};

fn snapshot(lang: Lang) -> Snapshot {
    Snapshot {
        team: "web".into(),
        session: "web".into(),
        dir: "/tmp/web".into(),
        claude: "/bin/claude".into(),
        config_dir: None,
        lang,
        members: ["lead", "dev"]
            .map(|n| MemberInfo {
                name: n.into(),
                role: "r".into(),
                contact: n == "lead",
                quiet: false,
                argv: Vec::new(),
                ..Default::default()
            })
            .to_vec(),
        dashboard: Vec::new(),
        journal: Vec::new(),
        ..Default::default()
    }
}

#[test]
fn menu_said_as_the_server_tells() {
    assert_eq!(menu_said(MenuOpened::Opened), t!("Menu de l'équipe ouvert.", "Team menu opened."));
    assert_eq!(
        menu_said(MenuOpened::AlreadyOpen),
        t!("Le menu de l'équipe est déjà ouvert.", "The team menu is already open.")
    );
    let none = menu_said(MenuOpened::NoClient);
    assert!(!none.contains("tmux") && none.contains("recruit attach"), "{none}");
}

#[test]
fn commands_in_the_team_language() {
    let dir = tempfile::tempdir().unwrap();
    let start = |lang, input: Value| start(&snapshot(lang), dir.path(), "lead", &input).unwrap();
    let names = |reply: Value| -> Vec<String> {
        reply["register"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(names(start(Lang::Fr, json!({ "commands": ["help"] }))), ["equipe", "recruit"]);
    assert_eq!(names(start(Lang::En, json!({}))), ["team", "recruit"]);
    // A command of the same name keeps its place.
    assert_eq!(names(start(Lang::Fr, json!({ "commands": ["recruit"] }))), ["equipe"]);
    // /regler and /tune are gone.
    let s = snapshot(Lang::Fr);
    for gone in ["regler", "tune"] {
        assert!(command_text(&s, dir.path(), "lead", gone).is_err());
    }
    assert!(command_text(&s, dir.path(), "lead", "team").is_err());
}

#[test]
fn quiet_members_keep_their_prompt_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = snapshot(Lang::Fr);
    s.members[1].quiet = true;
    let quiet = |member: &str| start(&s, dir.path(), member, &json!({})).unwrap()["quiet"].clone();
    assert_eq!(quiet("dev"), true);
    assert_eq!(quiet("lead"), false);
    // Not a member: nothing hidden.
    assert_eq!(quiet("qa"), false);
    // At each tick too: the main contact may change while the team runs.
    assert_eq!(tick(&s, dir.path(), "dev", &json!({})).unwrap()["quiet"], true);
    s.members[1].quiet = false;
    assert_eq!(tick(&s, dir.path(), "dev", &json!({})).unwrap()["quiet"], false);
}

#[test]
fn settings_on_each_request() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let mut s = snapshot(Lang::Fr);
    let argv = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    s.members[1].argv = argv(&["claude", "--model", "opus", "--effort", "high", "-n", "dev"]);
    let tick = |s: &Snapshot| tick(s, state, "dev", &json!({})).unwrap()["override"].clone();

    // As launched: the session's own, nothing put on its requests.
    s.members[1].model = Some("opus".into());
    s.members[1].effort = Some("high".into());
    assert_eq!(tick(&s), json!({}));
    // Changed since: the model as a full id.
    s.members[1].model = Some("sonnet".into());
    s.members[1].effort = Some("max".into());
    assert_eq!(tick(&s), json!({"model": "claude-sonnet-5-5", "effort": "max"}));
    s.members[1].model = Some("claude-opus-4-1".into());
    assert_eq!(tick(&s)["model"], "claude-opus-4-1");
    // Back to the default, or a model with no id known: at the next launch.
    s.members[1].model = None;
    assert_eq!(tick(&s), json!({"effort": "max"}));
    s.members[1].model = Some("opus[1m]".into());
    s.members[1].effort = Some("rapide".into());
    assert_eq!(tick(&s), json!({}));
    // `--model=…`, and the last one given, as Claude Code reads it.
    s.members[1].argv = argv(&["claude", "--model", "opus", "--model=sonnet"]);
    s.members[1].model = Some("sonnet".into());
    assert_eq!(tick(&s), json!({}));
    // A team launched before the command lines were kept: what the settings give.
    s.members[1].argv = Vec::new();
    assert_eq!(tick(&s), json!({"model": "claude-sonnet-5-5"}));
    // Another member's settings are its own.
    assert_eq!(super::tick(&s, state, "lead", &json!({})).unwrap()["override"], json!({}));

    // What requests cannot carry: the member starts again.
    let launched = argv(&["claude", "--model", "opus", "--effort", "high"]);
    let live = |model, effort| applies_live(model, effort, &launched);
    assert!(live(Some("opus"), Some("high")) && live(Some("sonnet"), Some("high")) && live(Some("opus"), Some("max")));
    assert!(live(Some("claude-opus-4-1"), Some("low")));
    // Back to Claude Code's default.
    assert!(!live(None, Some("high")) && !live(Some("opus"), None));
    assert!(!live(Some("opus[1m]"), Some("high")) && !live(Some("opus"), Some("rapide")));
    // Launched with Claude Code's defaults, and still on them.
    assert!(applies_live(None, None, &[]) && applies_live(Some("sonnet"), Some("low"), &[]));
}

#[test]
fn model_and_effort_as_shown() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let mut s = snapshot(Lang::Fr);
    let haiku = "claude-haiku-5-5";
    tick(&s, state, "dev", &json!({"model": haiku, "own": {"model": haiku, "effort": "low"}})).unwrap();
    assert_eq!(model_and_effort(state, &s.members[1]), (Some(haiku.into()), Some("low".into())));
    // What recruit puts on the requests, before any of them.
    s.members[1].model = Some("opus".into());
    s.members[1].effort = Some("xhigh".into());
    assert_eq!(model_and_effort(state, &s.members[1]), (Some("claude-opus-5-5".into()), Some("xhigh".into())));
    // Just after `/model` in the session: the last request's effort was for the model before.
    s.members[1].model = None;
    s.members[1].effort = None;
    let sonnet = "claude-sonnet-5-5";
    tick(&s, state, "dev", &json!({"model": sonnet, "own": {"model": haiku, "effort": "low"}})).unwrap();
    assert_eq!(model_and_effort(state, &s.members[1]), (Some(sonnet.into()), None));
    // Before any request, the effort its settings give.
    s.members[0].argv = ["claude", "--effort", "high"].map(String::from).to_vec();
    s.members[0].effort = Some("high".into());
    assert_eq!(override_of(&s.members[0]), Override::default());
    assert_eq!(model_and_effort(state, &s.members[0]), (None, Some("high".into())));
    // A report from an older mod: `last`.
    tick(&s, state, "lead", &json!({"model": haiku, "last": {"model": haiku, "effort": "medium"}})).unwrap();
    assert_eq!(model_and_effort(state, &s.members[0]).1.as_deref(), Some("medium"));
}

#[test]
fn place_in_the_team_told_once() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = Snapshot { config_dir: Some(dir.path().join("profile")), ..snapshot(Lang::Fr) };
    let file = state.join("02.md");
    fs::write(&file, "Tu es « dev ».\n").unwrap();
    let mut dev = s.members[1].clone();
    dev.argv = ["claude", "-n", "dev", "--append-system-prompt-file"]
        .iter()
        .map(|a| a.to_string())
        .chain([file.to_string_lossy().into_owned()])
        .collect();
    let submit = |session: &str, text: &str, now: &str| {
        submit(state, "dev", session, text, Lang::Fr, || Ok(now.to_string())).unwrap()
    };

    // A new conversation got its prompt on the command line.
    started(&s, state, "s1", &dev).unwrap();
    assert_eq!(submit("s1", "Publie la 0.7.3", "Tu es « dev ».\n"), None);
    // The team changed: told at the next prompt, once.
    let note = submit("s1", "Publie la 0.7.3", "Tu es « dev-rust ».\n").unwrap();
    assert!(note.contains("recruit") && note.ends_with("\n\nTu es « dev-rust »."), "{note}");
    assert_eq!(submit("s1", "Et la suite ?", "Tu es « dev-rust ».\n"), None);
    // A command: the model may never read it.
    assert_eq!(submit("s1", "  /compact", "Tu es « dev-rust », encore.\n"), None);
    assert!(submit("s1", "ok", "Tu es « dev-rust », encore.\n").is_some());
    // Compacted: the note may be gone with the summary, told again; unless the starting prompt says it all.
    tick(&s, state, "dev", &json!({"session": "s1", "compacted": true})).unwrap();
    assert!(submit("s1", "ok", "Tu es « dev-rust », encore.\n").is_some());
    tick(&s, state, "dev", &json!({"session": "s1", "compaction": {"id": "1", "done": true}})).unwrap();
    assert_eq!(submit("s1", "ok", "Tu es « dev ».\n"), None);

    // Resumed (renamed, restarted): its command line names the prompt a new conversation would get, its
    // conversation keeps the one it started with. As it was left.
    let conversation = claude::transcript(&s.dir, s.config_dir.as_deref(), "s1");
    fs::create_dir_all(conversation.parent().unwrap()).unwrap();
    fs::write(&conversation, "{}\n").unwrap();
    let renamed = state.join("03.md");
    fs::write(&renamed, "Tu es « dev-rust ».\n").unwrap();
    let mut resumed = dev.clone();
    *resumed.argv.last_mut().unwrap() = renamed.to_string_lossy().into_owned();
    started(&s, state, "s1", &resumed).unwrap();
    assert!(submit("s1", "ok", "Tu es « dev-rust ».\n").is_some());
    // Never noted (/clear, an older recruit): told at its first prompt, whatever changed.
    assert!(submit("s2", "ok", "Tu es « dev ».\n").is_some());
    assert_eq!(submit("s2", "ok", "Tu es « dev ».\n"), None);
    // Not an id: nothing written outside the folder.
    assert_eq!(submit("../s3", "ok", "x"), None);
    assert!(!state.join("s3.json").exists());
    // The prompt that cannot be made: the prompt goes bare, by the mod's leave.
    assert!(super::submit(state, "dev", "s4", "ok", Lang::Fr, || bail!("no team")).is_err());
}

/// The note joins the team's prompt, written in the team's language: the interface's may be another.
#[test]
fn the_note_is_in_the_teams_language() {
    let dir = tempfile::tempdir().unwrap();
    let note = |session: &str, lang: Lang| {
        submit(dir.path(), "dev", session, "ok", lang, || Ok("Tu es « dev ».\n".into())).unwrap().unwrap()
    };
    assert!(note("s1", Lang::Fr).starts_with("Note de recruit"));
    assert!(note("s2", Lang::En).starts_with("Note from recruit"));
}

#[test]
fn what_conversations_heard_goes_with_them() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = Snapshot { config_dir: Some(dir.path().join("profile")), ..snapshot(Lang::Fr) };
    let file = state.join("02.md");
    fs::write(&file, "Tu es « dev ».\n").unwrap();
    let mut dev = s.members[1].clone();
    dev.argv = vec!["--append-system-prompt-file".into(), file.to_string_lossy().into_owned()];
    let noted = |session: &str| prompted_file(state, session).unwrap().exists();
    started(&s, state, "s1", &dev).unwrap();
    submit(state, "dev", "s2", "ok", Lang::Fr, || Ok("x".into())).unwrap();
    submit(state, "lead", "s9", "ok", Lang::Fr, || Ok("x".into())).unwrap();
    assert!(noted("s1") && noted("s2") && noted("s9"));
    // Renamed: still its own.
    rename(state, "dev", "dev-rust").unwrap();
    assert_eq!(prompted_of(state, "dev-rust").len(), 2);
    // A new conversation: the earlier ones are left for good.
    dev.name = "dev-rust".into();
    started(&s, state, "s3", &dev).unwrap();
    assert!(!noted("s1") && !noted("s2") && noted("s3") && noted("s9"));
    // Removed: with the member.
    forget(state, "dev-rust").unwrap();
    assert!(!noted("s3") && noted("s9"));
}

#[test]
fn files_follow_their_member() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    tick(&s, state, "dev", &json!({"model": "claude-opus-5-5"})).unwrap();
    request_compaction(state, "dev").unwrap();
    request_resume(state, "dev").unwrap();
    rename(state, "dev", "dev-rust").unwrap();
    assert!(report(state, "dev").is_none());
    assert_eq!(report(state, "dev-rust").unwrap().model.as_deref(), Some("claude-opus-5-5"));
    assert!(compactions(state).join("dev-rust").exists());
    assert!(!resumes(state).join("dev").exists() && resumes(state).join("dev-rust").exists());
    forget(state, "dev-rust").unwrap();
    assert!(report(state, "dev-rust").is_none() && !compactions(state).join("dev-rust").exists());
    assert!(!resumes(state).join("dev-rust").exists());
    // Nothing to move or remove: nothing done.
    rename(state, "qa", "test").unwrap();
    forget(state, "qa").unwrap();

    fs::write(state.join(board::ZONES), "[]").unwrap();
    tick(&s, state, "dev", &json!({})).unwrap();
    fs::create_dir_all(prompted(state)).unwrap();
    request_resume(state, "dev").unwrap();
    reset(state).unwrap();
    assert!(report(state, "dev").is_none() && !resumes(state).exists());
    assert!(!state.join(board::ZONES).exists());
    // What conversations heard stays: one may be resumed.
    assert!(prompted(state).exists());
}

#[test]
fn flags_on_a_command_line() {
    let argv: Vec<String> = ["claude", "--model", "opus", "--effort=low", "--model"].map(String::from).to_vec();
    assert_eq!(claude::flag_value(&argv, "--effort"), Some("low"));
    // A flag with no value after it: none.
    assert_eq!(claude::flag_value(&argv, "--model"), None);
    assert_eq!(claude::flag_value(&argv, "--model-x"), None);
    assert_eq!(model_id("haiku").as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(model_id("sonnet[1m]"), None);
}

/// A line of a conversation that opens a turn: typed by the user, or a teammate's message.
fn opening(prompt: &str, origin: &str, text: &str) -> String {
    let row = match origin {
        "peer" => json!({
            "type": "user", "isMeta": true, "promptId": prompt, "turnOrigin": "peer",
            "origin": { "kind": "peer", "name": "coordinateur", "body": text },
            "message": { "role": "user", "content": format!("Another Claude session sent a message:\n<cross-session-message>\n{text}\n</cross-session-message>") },
        }),
        _ => json!({
            "type": "user", "promptId": prompt, "turnOrigin": origin, "promptSource": "typed",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
        }),
    };
    format!("{row}\n")
}

#[test]
fn requests_that_open_a_turn() {
    assert_eq!(turn_request(&opening("p1", "peer", "Publie la 0.6.2")), Some(("p1".into(), "Publie la 0.6.2".into())));
    assert_eq!(
        turn_request(&opening("p2", "human", "  corrige le test \n")),
        Some(("p2".into(), "corrige le test".into()))
    );
    let typed = json!({"type": "user", "promptId": "p3", "turnOrigin": "human", "message": {"content": "vas-y"}});
    assert_eq!(turn_request(&typed.to_string()), Some(("p3".into(), "vas-y".into())));
    // A tool's result belongs to the turn, a task's notice opens none.
    let result =
        json!({"type": "user", "promptId": "p3", "message": {"content": [{"type": "tool_result", "content": "ok"}]}});
    assert_eq!(turn_request(&result.to_string()), None);
    assert_eq!(turn_request(&opening("p4", "task_notification", "<task-notification>")), None);
    assert_eq!(turn_request(&opening("p5", "human", "   ")), None);
    let long = turn_request(&opening("p6", "peer", &"a".repeat(5000))).unwrap();
    assert_eq!(long.1.chars().count(), REQUEST_CHARS);
}

#[test]
fn conversation_read_on_by_whole_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("c.jsonl");
    let first = opening("p1", "peer", "un");
    fs::write(&file, format!("{{\"type\":\"assistant\"}}\n{first}{}", &opening("p2", "human", "deux")[..20])).unwrap();
    let mut offset = 0;
    let lines = turn_lines(&file, &mut offset);
    assert_eq!(lines, [first.trim_end()]);
    // The line being written is read once whole.
    fs::write(&file, format!("{{\"type\":\"assistant\"}}\n{first}{}", opening("p2", "human", "deux"))).unwrap();
    assert_eq!(turn_lines(&file, &mut offset).len(), 1);
    assert!(turn_lines(&file, &mut offset).is_empty());
    // Shorter: another conversation in its place, read from the start.
    fs::write(&file, opening("p9", "human", "neuf")).unwrap();
    assert_eq!(turn_lines(&file, &mut offset).len(), 1);
}

#[test]
fn answers_as_tasks() {
    let task = |now: &str, done: Option<&str>| Some(Task { now: now.into(), done: done.map(String::from) });
    assert_eq!(
        task_of("« Je publie la version 0.6.2. »\nVersion 0.6.2 publiée.\n"),
        task("Je publie la version 0.6.2", Some("Version 0.6.2 publiée"))
    );
    // Labels and bullets the model was asked not to put.
    assert_eq!(
        task_of("\n- En cours : \"I am fixing the test\"\n\nDone: Test fixed"),
        task("I am fixing the test", Some("Test fixed"))
    );
    // One line only: no form once done.
    assert_eq!(task_of("Je relis la maquette"), task("Je relis la maquette", None));
    assert_eq!(task_of(" \n "), None);
    assert_eq!(task_of(&"x".repeat(200)).unwrap().now.chars().count(), LINE_CHARS);
    // A colon of the line itself stays.
    assert_eq!(task_of("Je règle le seuil : 80 %"), task("Je règle le seuil : 80 %", None));
    // No control character: no escape sequence, no clipboard write (OSC 52).
    let trapped = "Je publie\x1b[2J la version\x1b]52;c;cm0gLXJmIH4=\x07\nVersion\u{9b}31m publiée";
    assert_eq!(task_of(trapped), task("Je publie[2J la version]52;c;cm0gLXJmIH4=", Some("Version31m publiée")));
}

#[test]
fn a_line_on_what_a_member_does() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let s = Snapshot { config_dir: Some(dir.path().join("profile")), ..snapshot(Lang::Fr) };
    let conversation = claude::transcript(&s.dir, s.config_dir.as_deref(), "sess");
    fs::create_dir_all(conversation.parent().unwrap()).unwrap();
    let tick = |input: Value| tick(&s, &state, "dev", &input).unwrap();

    // Nothing asked before a request.
    fs::write(&conversation, "{\"type\":\"assistant\"}\n").unwrap();
    assert!(tick(json!({"session": "sess"})).get("summarize").is_none());
    let append = |line: String| {
        let mut text = fs::read_to_string(&conversation).unwrap();
        text.push_str(&line);
        fs::write(&conversation, text).unwrap();
    };
    append(opening("p1", "peer", "Publie la version 0.6.2"));
    let ask = tick(json!({"session": "sess"}))["summarize"].clone();
    assert_eq!((ask["id"].as_str(), ask["model"].as_str()), (Some("p1"), Some(SUMMARY_MODEL)));
    assert!(ask["prompt"].as_str().unwrap().contains("Publie la version 0.6.2"));
    // Asked once.
    assert!(tick(json!({"session": "sess"})).get("summarize").is_none());
    // An answer to another question is not taken.
    tick(json!({"session": "sess", "summary": {"id": "p0", "text": "Je fais autre chose"}}));
    assert_eq!(doing(&state, "dev"), None);
    tick(
        json!({"session": "sess", "summary": {"id": "p1", "text": "Je publie la version 0.6.2.\nVersion 0.6.2 publiée"}}),
    );
    let published = Task { now: "Je publie la version 0.6.2".into(), done: Some("Version 0.6.2 publiée".into()) };
    assert_eq!(doing(&state, "dev"), Some(published.clone()));

    // The next request: the line from before goes with it; an error keeps the line.
    append(opening("p2", "human", "Merci !"));
    let ask = tick(json!({"session": "sess"}))["summarize"].clone();
    assert!(ask["prompt"].as_str().unwrap().contains("Je publie la version 0.6.2\nVersion 0.6.2 publiée"));
    tick(json!({"session": "sess", "summary": {"id": "p2", "error": "api-error"}}));
    assert_eq!(doing(&state, "dev"), Some(published.clone()));
    assert!(tick(json!({"session": "sess"})).get("summarize").is_none());

    // Another session (/clear): read from its start, the line kept meanwhile.
    let other = conversation.with_file_name("autre.jsonl");
    fs::write(&other, opening("q1", "human", "Écris les tests")).unwrap();
    assert_eq!(tick(json!({"session": "autre"}))["summarize"]["id"], "q1");
    assert_eq!(doing(&state, "dev"), Some(published));
    reset(&state).unwrap();
    assert_eq!(doing(&state, "dev"), None);
}

#[test]
fn context_counted_after_a_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let tick = |input: Value| tick(&s, state, "dev", &input).unwrap();
    let context = || report(state, "dev").unwrap().context.unwrap();
    // A session that has not answered yet: no count made up for it.
    let counted = json!({"tokens": 167_000, "enabled": true, "window": 200_000, "used": 6_000});
    tick(json!({"context": {"window": 200_000}, "threshold": counted}));
    assert_eq!(context().percent, None);
    tick(json!({"context": {"percent": 40.0, "tokens": 80_000, "window": 200_000}}));
    // Done: the breakdown asked at once; the API tells nothing until the session answers.
    assert_eq!(
        tick(json!({"context": {"window": 200_000}, "compaction": {"id": "1", "done": true}}))["breakdown"],
        "full"
    );
    assert_eq!(context().percent, None);
    tick(json!({"context": {"window": 200_000}, "threshold": counted}));
    assert_eq!((context().percent, context().tokens, context().estimated), (Some(3.0), Some(6_000), true));
    // Kept until the API tells again.
    tick(json!({"context": {"window": 200_000}}));
    assert_eq!(context().percent, Some(3.0));
    tick(json!({"context": {"percent": 5.0, "tokens": 10_000, "window": 200_000}}));
    assert_eq!((context().percent, context().estimated), (Some(5.0), false));
    // Without the API's figure nor a count, nothing is made up.
    tick(json!({"context": {"window": 200_000}}));
    assert_eq!(context().percent, None);
}

#[test]
fn compaction_asked_from_the_dashboard() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let tick = |member: &str, input: Value| tick(&s, state, member, &input).unwrap();
    request_compaction(state, "dev").unwrap();
    let id = tick("dev", json!({}))["compact"].as_str().unwrap().to_string();
    // Taken once.
    assert!(tick("dev", json!({})).get("compact").is_none());
    // How it went, kept until the next one.
    // A manual one carries no instructions, whatever the agents do.
    request_compaction(state, "dev").unwrap();
    let busy = json!([{"id": "a", "type": "Explore", "description": "look", "status": "running"}]);
    let reply = tick("dev", json!({"agents": busy}));
    assert!(reply["compact"].is_string() && reply["compactAsk"]["instructions"].is_null());
    tick("dev", json!({"compaction": {"id": id, "error": "a turn is running"}}));
    tick("dev", json!({}));
    let kept = report(state, "dev").unwrap().compaction.unwrap();
    assert_eq!((kept.id, kept.done, kept.error.as_deref()), (id, false, Some("a turn is running")));
    // Lapsed: dropped.
    fs::write(compactions(state).join("dev"), json!({"id": "1", "until": crate::sys::now() - 1}).to_string()).unwrap();
    assert!(tick("dev", json!({})).get("compact").is_none());
    assert!(!compactions(state).join("dev").exists());
    // Another member's request stays for it.
    request_compaction(state, "lead").unwrap();
    assert!(tick("dev", json!({})).get("compact").is_none());
    assert!(tick("lead", json!({})).get("compact").is_some());
}

#[test]
fn auto_compaction_refused_while_an_agent_works() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let tick = |input: Value| tick(&s, state, "dev", &input).unwrap();
    let agent = |status: &str| json!({"id": "a", "type": "Explore", "description": "look", "status": status});
    let teammate = |status: &str| json!({"id": "t", "teammateId": "scout@crew", "name": "scout", "status": status});

    // A subagent at work: refused, said as a hook's veto, and nothing handed to the mod.
    request_auto_compaction(state, "dev", 82).unwrap();
    let reply = tick(json!({"agents": [agent("running")]}));
    assert!(reply.get("compact").is_none() && reply.get("compactAsk").is_none());
    let kept = report(state, "dev").unwrap().compaction.unwrap();
    assert_eq!((kept.skip.as_deref(), kept.done, kept.auto, kept.percent), (Some(SKIP_HELPERS), false, true, Some(82)));
    assert!(!compactions(state).join("dev").exists(), "taken, not asked again");
    // The refusal stays in the report until the next compaction.
    tick(json!({"agents": [agent("running")]}));
    assert_eq!(report(state, "dev").unwrap().compaction.unwrap().skip.as_deref(), Some(SKIP_HELPERS));

    // A teammate at work too; one waiting for a message, or a subagent over, does not count.
    request_auto_compaction(state, "dev", 82).unwrap();
    assert!(tick(json!({"agents": [teammate("running")]})).get("compact").is_none());
    for agents in [json!([teammate("idle")]), json!([agent("completed")]), json!([agent("failed")]), json!([])] {
        request_auto_compaction(state, "dev", 75).unwrap();
        let reply = tick(json!({ "agents": agents }));
        assert!(reply["compact"].is_string(), "{agents}");
        assert_eq!((&reply["compactAsk"]["percent"], &reply["compactAsk"]["auto"]), (&json!(75), &json!(true)));
        let said = reply["compactAsk"]["instructions"].as_str().unwrap();
        assert!(said.contains("files touched") && said.contains("open points") && said.contains("who asked for what"));
    }
    // An agents list the session could not give leaves those it had, and a mod that lists none asks nothing.
    request_auto_compaction(state, "dev", 75).unwrap();
    tick(json!({"agents": [agent("running")]}));
    request_auto_compaction(state, "dev", 75).unwrap();
    assert!(tick(json!({"agents": null})).get("compact").is_none());
    request_auto_compaction(state, "dev", 75).unwrap();
    assert!(tick(json!({})).get("compact").is_some());
}

#[test]
fn compaction_traced_with_the_context_before_and_after() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let tick = |input: Value| tick(&s, state, "dev", &input).unwrap();
    let context = |percent: f64| json!({"context": {"percent": percent, "tokens": 148_000, "window": 200_000}});
    // By hand: the context the report knows when the request is taken, handed to the mod to tell back.
    tick(context(74.0));
    request_compaction(state, "dev").unwrap();
    let reply = tick(context(74.2));
    let id = reply["compact"].as_str().unwrap().to_string();
    assert_eq!(reply["compactAsk"], json!({"percent": 74}));
    // Not asked by the server: no `auto`.
    tick(json!({"compaction": {"id": id, "done": true, "percent": 74}, "context": {"window": 200_000}}));
    let made = report(state, "dev").unwrap().compacted.unwrap();
    assert_eq!((made.done, made.auto, made.percent, made.after), (true, false, Some(74), None));
    // The session tells nothing yet; then the breakdown's count stands in, and the context after is kept.
    tick(json!({"context": {"window": 200_000}}));
    assert_eq!(report(state, "dev").unwrap().compacted.unwrap().after, None);
    let used = json!({"context": {"window": 200_000}, "threshold": {"tokens": 167_000, "enabled": true, "window": 200_000, "used": 18_000}});
    tick(used.clone());
    let made = report(state, "dev").unwrap().compacted.unwrap();
    assert_eq!((made.percent, made.after), (Some(74), Some(9)));
    // Kept as is when the conversation goes on, and through a refusal or an error that follow.
    tick(context(30.0));
    request_auto_compaction(state, "dev", 80).unwrap();
    let id = tick(json!({}))["compact"].as_str().unwrap().to_string();
    tick(json!({"compaction": {"id": id, "error": "a turn is running", "auto": true, "percent": 80}}));
    let kept = report(state, "dev").unwrap();
    assert_eq!(kept.compacted.unwrap().after, Some(9));
    assert_eq!(kept.compaction.unwrap().error.as_deref(), Some("a turn is running"));
    // A later figure than a minute after is not "after" any more.
    let mut old = report(state, "dev").unwrap();
    let mut stale = old.compacted.take().unwrap();
    stale.after = None;
    stale.at = crate::sys::now() - COMPACTED_AFTER - 5;
    old.compacted = Some(stale);
    write_atomic(&reports(state).join("dev.json"), &serde_json::to_string(&old).unwrap()).unwrap();
    tick(context(55.0));
    assert_eq!(report(state, "dev").unwrap().compacted.unwrap().after, None);
}

#[test]
fn a_tick_as_the_mod_sends_it_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    // The input's keys as `tick` in `mod/hooks/register.ts` makes them, `compacted` (a flag) included, with what a
    // handover adds.
    let input = json!({
        "session": "s1",
        "model": "claude-haiku-5-5",
        "own": {"model": "claude-haiku-5-5", "effort": "high"},
        "context": {"tokens": 24_000, "window": 200_000, "percent": 12.0},
        "rateLimits": [{"kind": "five_hour", "percentUsed": 21.0}],
        "threshold": {"tokens": 167_000, "enabled": true, "window": 200_000, "model": "claude-haiku-5-5", "used": 23_000},
        "compacted": false,
        "agents": [],
        "summary": {"id": "x", "text": "t"},
        "compaction": {"id": "1", "done": true, "auto": true, "percent": 71},
        "resumption": {"id": "2", "sent": true},
        "typed": {"tokensBefore": 1},
    });
    for flag in [false, true] {
        let reply = tick(&s, state, "dev", &json!({ "compacted": flag, "session": "s1" }));
        assert!(reply.is_ok());
    }
    tick(&s, state, "dev", &input).unwrap();
    let kept = report(state, "dev").unwrap();
    assert!(kept.input_error.is_none(), "{:?}", kept.input_error);
    assert_eq!((kept.session.as_deref(), kept.model.as_deref()), (Some("s1"), Some("claude-haiku-5-5")));
    assert_eq!(kept.context.as_ref().and_then(|c| c.percent), Some(12.0));
    assert_eq!(kept.threshold.as_ref().and_then(|t| t.tokens), Some(167_000));
    assert_eq!(kept.rate_limits.len(), 1);
    assert!(kept.compacted.is_some());
    // The file names the last compaction apart from the flag.
    let file: Value = serde_json::from_str(&fs::read_to_string(reports(state).join("dev.json")).unwrap()).unwrap();
    assert!(file["last_compacted"]["done"] == true && file.get("compacted").is_none());
}

#[test]
fn an_unreadable_tick_is_no_news() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    // A report from a while ago: a failed automatic compaction, a measure of the threshold.
    let old = crate::sys::now() - 600;
    let failed =
        Compaction {
            id: "c1".into(), at: old, auto: true, error: Some("API surchargée".into()), ..Default::default()
        };
    let threshold = Threshold { tokens: Some(167_000), at: old, ..Default::default() };
    let before = Report {
        session: Some("s1".into()),
        at: old,
        compaction: Some(failed.clone()),
        threshold: Some(threshold.clone()),
        ..Default::default()
    };
    fs::create_dir_all(reports(state)).unwrap();
    write_atomic(&reports(state).join("dev.json"), &serde_json::to_string(&before).unwrap()).unwrap();
    for _ in 0..2 {
        tick(&s, state, "dev", &json!({"session": "s1", "context": "twelve"})).unwrap();
    }
    let kept = report(state, "dev").unwrap();
    // Nothing stamped as new: the dashboard sees it stale, and the journal's key (the request and its time) holds.
    assert_eq!((kept.at, kept.compaction, kept.threshold), (old, Some(failed), Some(threshold)));
    assert!(kept.input_error.is_some());
    // Said once in the log, at the first.
    let log = fs::read_to_string(state.join("server.log")).unwrap();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("tick of dev does not read"), "{log}");
    // A tick that reads is news again, and its compaction's time is its own.
    tick(&s, state, "dev", &json!({"session": "s1", "compaction": {"id": "c2", "error": "x"}})).unwrap();
    let fresh = report(state, "dev").unwrap();
    assert!(fresh.at >= old + 600 && fresh.input_error.is_none());
    assert_eq!(fresh.compaction.map(|c| (c.id, c.at >= old + 600)), Some(("c2".into(), true)));
}

#[test]
fn an_unreadable_tick_keeps_the_report() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let tick = |input: Value| tick(&s, state, "dev", &input).unwrap();
    tick(json!({"session": "s1", "model": "claude-haiku-5-5", "context": {"percent": 12.0, "window": 200_000}}));
    // A context that is not one: the report stays, and says why.
    tick(json!({"session": "s1", "context": "twelve"}));
    let kept = report(state, "dev").unwrap();
    assert_eq!(kept.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(kept.context.as_ref().and_then(|c| c.percent), Some(12.0));
    assert!(kept.input_error.as_deref().is_some_and(|e| e.contains("context") || e.contains("invalid type")));
    // The next tick that reads clears it.
    tick(json!({"session": "s1", "model": "claude-haiku-5-5"}));
    assert!(report(state, "dev").unwrap().input_error.is_none());
}

#[test]
fn compaction_typed_in_the_panel_is_traced() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let tick = |input: Value| tick(&s, state, "dev", &input).unwrap();
    let context = |percent: f64| json!({"context": {"percent": percent, "tokens": 148_000, "window": 200_000}});
    // The person's `/compact`: the context before is the report's last figure; asked to count at once.
    tick(context(74.0));
    let reply = tick(json!({"typed": {"tokensBefore": 150_000, "tokensAfter": 20_000}, "compacted": true,
        "context": {"window": 200_000}}));
    assert_eq!(reply["breakdown"], "full");
    let made = report(state, "dev").unwrap().compacted.unwrap();
    assert!(made.id.starts_with("typed-"));
    assert_eq!((made.done, made.auto, made.percent, made.after), (true, false, Some(74), None));
    // The last request is not that: nothing asked.
    assert!(report(state, "dev").unwrap().compaction.is_none());
    // The context after, once the session counts.
    let used = json!({"context": {"window": 200_000}, "threshold": {"tokens": 167_000, "enabled": true, "window": 200_000, "used": 18_000}});
    tick(used);
    let made = report(state, "dev").unwrap().compacted.unwrap();
    assert_eq!((made.percent, made.after), (Some(74), Some(9)));
    // A report with no figure before: the size the compaction recorded, against the window.
    let dir = tempfile::tempdir().unwrap();
    let fresh = |input: Value| tick_fresh(&s, dir.path(), &input);
    fresh(json!({"typed": {"tokensBefore": 150_000}, "context": {"window": 200_000}}));
    let made = report(dir.path(), "dev").unwrap().compacted.unwrap();
    assert_eq!((made.percent, made.after), (Some(75), None));
    // Nothing to go by: no figure rather than a guess.
    let dir = tempfile::tempdir().unwrap();
    tick_fresh(&s, dir.path(), &json!({"typed": {}}));
    assert_eq!(report(dir.path(), "dev").unwrap().compacted.unwrap().percent, None);
}

fn tick_fresh(s: &Snapshot, state: &Path, input: &Value) -> Value {
    tick(s, state, "dev", input).unwrap()
}

#[test]
fn auto_compaction_told_to_the_dashboard() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let tick = |input: Value| tick(&s, state, "dev", &input).unwrap();
    request_auto_compaction(state, "dev", 71).unwrap();
    let id = tick(json!({}))["compact"].as_str().unwrap().to_string();
    // The mod's answer says it was automatic; the next ticks keep it.
    tick(json!({"compaction": {"id": id, "done": true, "auto": true, "percent": 71}}));
    tick(json!({}));
    let kept = report(state, "dev").unwrap().compaction.unwrap();
    assert_eq!((kept.id, kept.done, kept.auto, kept.percent), (id.clone(), true, true, Some(71)));
    let made = report(state, "dev").unwrap().compacted.unwrap();
    assert_eq!((made.id.as_str(), made.done, made.auto, made.percent), (id.as_str(), true, true, Some(71)));
    // A later refusal replaces the outcome of the last request, not the last compaction made.
    request_auto_compaction(state, "dev", 80).unwrap();
    let busy = json!([{"id": "a", "type": "Explore", "description": "look", "status": "running"}]);
    tick(json!({"agents": busy}));
    let after = report(state, "dev").unwrap();
    assert_eq!(after.compaction.unwrap().skip.as_deref(), Some(SKIP_HELPERS));
    assert_eq!(after.compacted, Some(made.clone()));
    // A compaction made by hand takes its place, with no `auto`.
    request_compaction(state, "dev").unwrap();
    let manual = tick(json!({}))["compact"].as_str().unwrap().to_string();
    tick(json!({"compaction": {"id": manual, "done": true}}));
    let by_hand = report(state, "dev").unwrap().compacted.unwrap();
    assert_eq!((by_hand.id.as_str(), by_hand.auto), (manual.as_str(), false));
    // As does a new automatic one.
    request_auto_compaction(state, "dev", 90).unwrap();
    let second = tick(json!({}))["compact"].as_str().unwrap().to_string();
    tick(json!({"compaction": {"id": second, "done": true, "auto": true, "percent": 90}}));
    let latest = report(state, "dev").unwrap().compacted.unwrap();
    assert_eq!((latest.id, latest.percent), (second, Some(90)));
    // Taken once.
    assert!(tick(json!({})).get("compact").is_none());
    // Lapsed: dropped.
    request_auto_compaction(state, "dev", 71).unwrap();
    let lapsed = json!({"id": "1", "until": crate::sys::now() - 1, "auto": true, "percent": 71});
    fs::write(compactions(state).join("dev"), lapsed.to_string()).unwrap();
    assert!(tick(json!({})).get("compact").is_none());
    assert!(!compactions(state).join("dev").exists());
}

/// The dashboard sees `member` in `state`, now.
fn seen(state: &Path, member: &str, seen: look::State) {
    let at = crate::sys::now();
    let member_state = board::MemberState { state: seen, since: at, shell: false };
    let states = board::States { at, members: [(member.to_string(), member_state)].into(), ..Default::default() };
    fs::write(state.join(board::STATES), serde_json::to_string(&states).unwrap()).unwrap();
}

#[test]
fn resume_asked_from_the_menu() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let tick = |member: &str, input: Value| tick(&s, state, member, &input).unwrap();
    // None asked: nothing to tell.
    assert!(resumption(state, "dev").is_none());

    // Not seen by the dashboard (none looking): handed over, the mod's guard is enough.
    request_resume(state, "dev").unwrap();
    let unseen = tick("dev", json!({}))["resume"].as_str().unwrap().to_string();
    tick("dev", json!({"resumption": {"id": unseen, "sent": true}}));
    assert!(resumption(state, "dev").unwrap().sent);
    std::thread::sleep(std::time::Duration::from_millis(2));
    request_resume(state, "dev").unwrap();
    seen(state, "dev", look::State::Error);
    let id = tick("dev", json!({}))["resume"].as_str().unwrap().to_string();
    // Taken once; no answer yet.
    assert!(tick("dev", json!({})).get("resume").is_none());
    assert!(resumption(state, "dev").is_none());
    tick("dev", json!({"resumption": {"id": id, "sent": true}}));
    let done = resumption(state, "dev").unwrap();
    assert!(done.sent && !done.draft && done.error.is_none() && done.id == id);
    assert!(done.at >= crate::sys::now() - 1);
    // Kept until the next one, never told as the next one's answer.
    tick("dev", json!({}));
    assert!(resumption(state, "dev").unwrap().sent);
    std::thread::sleep(std::time::Duration::from_millis(2));
    request_resume(state, "dev").unwrap();
    assert!(resumption(state, "dev").is_none());
    // At rest is fine too; a draft in the prompt box, nothing sent.
    seen(state, "dev", look::State::Idle);
    let next = tick("dev", json!({}))["resume"].as_str().unwrap().to_string();
    assert_ne!(next, id);
    assert!(resumption(state, "dev").is_none());
    tick("dev", json!({"resumption": {"id": next, "draft": true}}));
    assert!(resumption(state, "dev").unwrap().draft);

    // The reasons known, said in the user's language; the session's own, as it said them.
    let failed = |code: &str| {
        std::thread::sleep(std::time::Duration::from_millis(2));
        request_resume(state, "dev").unwrap();
        let id = tick("dev", json!({}))["resume"].as_str().unwrap().to_string();
        tick("dev", json!({"resumption": {"id": id, "error": code}}));
        resumption(state, "dev").unwrap().error.unwrap()
    };
    assert_eq!(failed("busy"), t!("le membre est occupé", "the member is busy"));
    assert!(failed("unsupported").contains("Claude Code"));
    assert_eq!(failed("Error: refused"), "Error: refused");

    // At work, or waiting for an answer: refused at once, not handed over.
    for busy in [look::State::Working, look::State::Waiting] {
        seen(state, "dev", busy);
        std::thread::sleep(std::time::Duration::from_millis(2));
        request_resume(state, "dev").unwrap();
        assert!(tick("dev", json!({})).get("resume").is_none());
        let refused = resumption(state, "dev").unwrap();
        assert_eq!(refused.error.unwrap(), t!("le membre est occupé", "the member is busy"));
        assert!(!refused.sent);
    }

    // Another member's request stays for it.
    seen(state, "lead", look::State::Error);
    request_resume(state, "lead").unwrap();
    assert!(tick("dev", json!({})).get("resume").is_none());
    assert!(tick("lead", json!({})).get("resume").is_some());
}

#[test]
fn resume_lapses_without_an_answer() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let now = crate::sys::now();
    let lapsed = t!("pas de réponse de sa session", "no answer from its session");
    // Never taken (no mod): lapsed once its time is up, and no longer handed over.
    fs::create_dir_all(resumes(state)).unwrap();
    fs::write(resumes(state).join("dev"), json!({"id": "1", "until": now + 5}).to_string()).unwrap();
    assert!(resumption_at(state, "dev", now).is_none());
    let told = resumption_at(state, "dev", now + 6).unwrap();
    assert_eq!((told.id.as_str(), told.error.as_deref(), told.at), ("1", Some(lapsed.as_str()), now + 5));
    fs::write(resumes(state).join("dev"), json!({"id": "1", "until": now - 1}).to_string()).unwrap();
    assert!(tick(&s, state, "dev", &json!({})).unwrap().get("resume").is_none());
    assert!(resumption(state, "dev").unwrap().error.is_some());
    // Taken, and the mod never answered (its session gone): lapsed a while later.
    request_resume(state, "dev").unwrap();
    let id = tick(&s, state, "dev", &json!({})).unwrap()["resume"].as_str().unwrap().to_string();
    let until = now + COMPACTION_LAPSE;
    assert!(resumption_at(state, "dev", until + RESUME_ANSWER).is_none());
    let told = resumption_at(state, "dev", until + RESUME_ANSWER + 2).unwrap();
    assert_eq!((told.id, told.error), (id, Some(lapsed)));
}

#[test]
fn auto_compaction_measured_now_and_then() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let s = snapshot(Lang::Fr);
    let tick = |member: &str, input: Value| tick(&s, state, member, &input).unwrap();
    let opus = "claude-opus-5-5";
    // Not measured yet: asked.
    assert_eq!(tick("dev", json!({"model": opus}))["breakdown"], "summary");
    let threshold = json!({"tokens": 967_000, "enabled": true, "window": 1_000_000, "model": opus});
    assert!(tick("dev", json!({"model": opus, "threshold": threshold})).get("breakdown").is_none());
    // Kept from one tick to the next.
    assert!(tick("dev", json!({"model": opus})).get("breakdown").is_none());
    let report = report(state, "dev").unwrap();
    assert_eq!(report.compacts_at(), Some(967_000));
    // Another model: measured again.
    let haiku = "claude-haiku-4-5-20251001";
    assert_eq!(tick("dev", json!({"model": haiku}))["breakdown"], "summary");
    // Not answered (an older Claude Code): not asked again for a while.
    assert!(tick("dev", json!({"model": haiku})).get("breakdown").is_none());
    assert!(tick("lead", json!({}))["breakdown"] == "summary" && tick("lead", json!({})).get("breakdown").is_none());
    // Auto-compaction off: where it would compact.
    let off = Report {
        threshold: Some(Threshold { tokens: None, enabled: false, window: Some(200_000), ..Default::default() }),
        ..Default::default()
    };
    assert_eq!(off.compacts_at(), Some(167_000));
    assert_eq!(Report::default().compacts_at(), None);
}

#[test]
fn mod_files() {
    let manifest: Value = serde_json::from_str(FILES[0].1).unwrap();
    assert_eq!(manifest["name"], "recruit");
    assert!(FILES[2].1.contains("'_mod'"));
}

#[test]
fn subagents_from_launch_to_end() {
    let agent = |id: &str, kind: &str, status: &str| json!({ "id": id, "type": kind, "description": format!("{id} work"), "status": status });
    let list = |agents: Vec<Value>| agents;
    let states = |list: &[Helper]| list.iter().map(|h| (h.id.clone(), h.status, h.since)).collect::<Vec<_>>();
    // Started: seen from now on.
    let first = helpers(&[], &list(vec![agent("a", "Explore", "running")]), 100);
    assert_eq!((first[0].kind.as_str(), first[0].description.as_str()), ("Explore", "a work"));
    assert_eq!((first[0].teammate.as_ref(), first[0].started), (None, 100));
    // A second one, held, and the first between two turns: both at work; the first keeps its start.
    let both = helpers(&first, &list(vec![agent("a", "Explore", "idle"), agent("b", "Plan", "waiting")]), 104);
    assert_eq!(states(&both), [("a".into(), HelperStatus::Running, 100), ("b".into(), HelperStatus::Running, 104)]);
    // Done, stopped: over since the moment seen so, which stays.
    let over = helpers(&both, &list(vec![agent("a", "Explore", "completed"), agent("b", "Plan", "killed")]), 110);
    let later = helpers(&over, &list(vec![agent("a", "Explore", "completed")]), 115);
    assert_eq!(states(&later), [("a".into(), HelperStatus::Done, 110), ("b".into(), HelperStatus::Failed, 110)]);
    // Done ten seconds ago: gone; failed, still there a while.
    let gone = helpers(&later, &list(Vec::new()), 120);
    assert_eq!(states(&gone), [("b".into(), HelperStatus::Failed, 110)]);
    assert!(helpers(&gone, &list(Vec::new()), 140).is_empty());
    // One the session no longer lists while at work: over.
    let dropped = helpers(&first, &list(Vec::new()), 105);
    assert_eq!(states(&dropped), [("a".into(), HelperStatus::Done, 105)]);
}

#[test]
fn teammates_until_no_longer_listed() {
    let mate = |status: &str| {
        json!({ "id": "m1", "type": "reviewer", "description": "review", "status": status,
                "teammateId": "critic@web", "name": "critic" })
    };
    let one = |status: &str| vec![mate(status)];
    let first = helpers(&[], &one("running"), 100);
    assert_eq!((first[0].teammate.as_deref(), first[0].status), (Some("critic"), HelperStatus::Running));
    // Waiting for a message: idle, since then.
    let idle = helpers(&first, &one("idle"), 130);
    assert_eq!((idle[0].status, idle[0].since, idle[0].started), (HelperStatus::Idle, 130, 100));
    // Done, it stays while listed; once no longer listed, it leaves.
    let done = helpers(&idle, &one("completed"), 200);
    assert_eq!(helpers(&done, &one("completed"), 400)[0].status, HelperStatus::Done);
    assert!(helpers(&done, &[], 210).is_empty());
    // A ghost: the same word half an hour, unknown; two hours, gone.
    let still = helpers(&first, &one("running"), 100 + TEAMMATE_UNKNOWN);
    assert!(still[0].unknown(100 + TEAMMATE_UNKNOWN) && !first[0].unknown(101));
    assert!(helpers(&still, &one("running"), 100 + TEAMMATE_DROPPED).is_empty());
    // Without a name, the address's.
    let bare = json!({ "id": "m2", "type": "x", "description": "", "status": "running", "teammateId": "solo@web" });
    assert_eq!(helpers(&[], &[bare], 1)[0].teammate.as_deref(), Some("solo"));
}

#[test]
fn same_session_new_start() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path();
    let agent = |id: &str, status: &str| json!({ "id": id, "type": "Explore", "description": "", "status": status });
    let mate = json!({ "id": "m", "type": "x", "description": "", "status": "running", "teammateId": "critic@web" });
    let helpers = helpers(&[], &[agent("a", "running"), agent("b", "completed"), mate], 100);
    let before = Report { session: Some("s1".into()), at: 100, helpers, ..Default::default() };
    fs::create_dir_all(reports(state)).unwrap();
    write_atomic(&reports(state).join("dev.json"), &serde_json::to_string(&before).unwrap()).unwrap();
    // Started again on the same conversation: the subagent at work was stopped, the one done stays, the teammate
    // went; the next tick, same session, keeps it so.
    restarted(state, "dev", 106).unwrap();
    let after = report(state, "dev").unwrap();
    let states: Vec<(&str, HelperStatus, i64)> =
        after.helpers.iter().map(|h| (h.id.as_str(), h.status, h.since)).collect();
    assert_eq!(states, [("a", HelperStatus::Failed, 106), ("b", HelperStatus::Done, 100)]);
    let next = helpers_now(&after, &json!({ "session": "s1", "agents": [] }), 108);
    assert_eq!(next.iter().map(|h| h.status).collect::<Vec<_>>(), [HelperStatus::Failed, HelperStatus::Done]);
    // Nothing to leave behind: no report written.
    restarted(state, "nobody", 106).unwrap();
    assert!(report(state, "nobody").is_none());
}

#[test]
fn subagent_listed_again_once_done() {
    let agent = |status: &str| json!({ "id": "a", "type": "Explore", "description": "", "status": status });
    let done = helpers(&helpers(&[], &[agent("running")], 100), &[agent("completed")], 105);
    // Woken by a message, under the same id: at work again, since then, from its first start.
    let again = helpers(&done, &[agent("running")], 108);
    assert_eq!((again[0].status, again[0].started, again[0].since), (HelperStatus::Running, 100, 108));
}

#[test]
fn helpers_from_a_tick() {
    let agent = |id: &str, status: &str| json!({ "id": id, "type": "Explore", "description": "", "status": status });
    let report = |input: &Value, before: &Report, now| Report {
        session: input["session"].as_str().map(String::from),
        helpers: helpers_now(before, input, now),
        ..Default::default()
    };
    let first = report(
        &json!({ "session": "s1", "agents": [agent("a", "running"), agent("b", "completed")] }),
        &Report::default(),
        100,
    );
    assert_eq!(first.helpers.len(), 2);
    // The session could not list them: as they were, their times too.
    let failed = report(&json!({ "session": "s1", "agents": null }), &first, 104);
    assert_eq!(failed.helpers, first.helpers);
    // An older mod, which never lists them: none.
    assert!(report(&json!({ "session": "s1" }), &first, 104).helpers.is_empty());
    // The member started again: those at work were stopped with the session, those done stay done.
    let again = report(&json!({ "session": "s2", "agents": [] }), &first, 106);
    let states: Vec<(&str, HelperStatus, i64)> =
        again.helpers.iter().map(|h| (h.id.as_str(), h.status, h.since)).collect();
    assert_eq!(states, [("a", HelperStatus::Failed, 106), ("b", HelperStatus::Done, 100)]);
}
