// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! recruit's side of the mod it puts in each member (`mod/`, a Claude Code plugin of function hooks). The mod
//! decides nothing: it calls `recruit _mod <event> <state> <member> [command]`, what the session knows as JSON on
//! the standard input, and does what the JSON answer says.
//!
//! - `start`: the slash commands to declare, `/equipe` (`/team` in English) and `/recruit`, and whether the member
//!   keeps its prompt alone, without the hint line under it (all but the main contact).
//! - `tick`, every two seconds: the session's model, context and usage, kept for the dashboard; the answer
//!   holds the model and effort the team's settings give the member now, which the mod puts on each of its
//!   requests, whether it keeps its prompt alone (the main contact may change), and what recruit asks of the
//!   session: a line on what the member does (a completion it asks the model for), a compaction the dashboard
//!   asked, the context's breakdown now and then. The mod hands the answers over at the next tick.
//! - `submit`, for each prompt: a note joined to it when the member's place in the team changed since its
//!   conversation last heard of it (see [`submit`]).
//! - `command`: what one of these commands does and prints.

use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{cache_dir, write_atomic};
use crate::i18n::Lang;
use crate::state::{MemberInfo, Snapshot};
use crate::{board, claude, live, menu, t};

/// Claude Code loads mods from 2.1.287 on.
pub const MIN_CLAUDE: (u32, u32, u32) = (2, 1, 287);

const FILES: [(&str, &str); 3] = [
    (".claude-plugin/plugin.json", include_str!("../mod/.claude-plugin/plugin.json")),
    ("hooks/hooks.json", include_str!("../mod/hooks/hooks.json")),
    ("hooks/register.ts", include_str!("../mod/hooks/register.ts")),
];

/// Writes the mod under `~/.cache/recruit/mod/<version>/`, for `claude --plugin-dir`. One folder per version of
/// recruit: a team still running keeps the one it was launched with.
pub fn install() -> Result<PathBuf> {
    let dir = cache_dir().join("mod").join(env!("CARGO_PKG_VERSION"));
    for (name, text) in FILES {
        let file = dir.join(name);
        if fs::read_to_string(&file).ok().as_deref() != Some(text) {
            fs::create_dir_all(file.parent().expect("a file in a folder"))?;
            fs::write(&file, text).with_context(|| t!("écriture de {}", "writing {}", file.display()))?;
        }
    }
    Ok(dir)
}

/// The version of Claude Code (`claude --version`: "2.1.292 (Claude Code)").
pub fn claude_version(claude: &Path) -> Option<(u32, u32, u32)> {
    let output = Command::new(claude).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace().next()?.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Event {
    Start,
    Tick,
    Submit,
    Command,
}

/// The model and effort of a member's requests.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Override {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// What a member's session last told: kept for the dashboard.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Report {
    /// The session's own model.
    #[serde(default)]
    pub model: Option<String>,
    /// The model and effort of its last request, as the session made it, before recruit's.
    #[serde(default, alias = "last")]
    pub own: Override,
    #[serde(default)]
    pub context: Option<ContextUse>,
    #[serde(default, rename = "rateLimits")]
    pub rate_limits: Vec<RateLimit>,
    /// When, in seconds since the epoch.
    #[serde(default)]
    pub at: i64,
    /// When the session compacts on its own, as last measured: now and then only, kept in between.
    #[serde(default)]
    pub threshold: Option<Threshold>,
    /// The last compaction the dashboard asked, and how it went; kept until the next one.
    #[serde(default)]
    pub compaction: Option<Compaction>,
    /// When the breakdown was asked, while no answer came: asked again only after a while. 0 when none waits.
    #[serde(default)]
    pub breakdown_asked: i64,
}

impl Report {
    /// How many tokens of context make the session compact on its own: as measured, or, with auto-compaction off,
    /// where it would (the window, less what Claude Code keeps for the reply and its buffer). None before a
    /// measure.
    pub fn compacts_at(&self) -> Option<u64> {
        let threshold = self.threshold.as_ref()?;
        match threshold.tokens {
            Some(tokens) if threshold.enabled => Some(tokens),
            _ => threshold.window.map(|window| window.saturating_sub(REPLY_RESERVE + COMPACT_BUFFER)),
        }
    }
}

/// Of the window, what Claude Code keeps for the model's reply (its output limit, at most this) and the margin
/// it compacts before: 2.1.292 compacts at the window less both.
const REPLY_RESERVE: u64 = 20_000;
const COMPACT_BUFFER: u64 = 13_000;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
/// How full the session's context is.
pub struct ContextUse {
    /// Of the model's window.
    #[serde(default)]
    pub percent: Option<f64>,
    #[serde(default)]
    pub tokens: Option<u64>,
    /// The model's window, in tokens.
    #[serde(default)]
    pub window: Option<u64>,
    /// Counted by the breakdown, not by the API: the session has not answered since it compacted (or started).
    #[serde(default)]
    pub estimated: bool,
}

/// Where the session compacts on its own (`/context`'s breakdown).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Threshold {
    /// Tokens of context at which it compacts; none with auto-compaction off.
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(default)]
    pub enabled: bool,
    /// The window it measures against: the model's, or a smaller one set for compaction.
    #[serde(default)]
    pub window: Option<u64>,
    /// The model it was measured for: measured again once the session changes model.
    #[serde(default)]
    pub model: Option<String>,
    /// The tokens of context the breakdown counted, an estimate.
    #[serde(default)]
    pub used: Option<u64>,
    #[serde(default)]
    pub at: i64,
}

/// A compaction asked from the dashboard: done, skipped by a hook, or refused (a turn running).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compaction {
    pub id: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub skip: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub at: i64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    /// `five_hour`, `seven_day`.
    pub kind: String,
    #[serde(rename = "percentUsed")]
    pub percent_used: f64,
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<String>,
}

pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// The aliases a team's settings may give, as model ids: a request names a full id, and the mod has no way to resolve
/// an alias. To bring up to date when new models come out; a full `claude-…` id is taken as it is.
pub const ALIASES: [(&str, &str); 4] = [
    ("opus", "claude-opus-5-5"),
    ("sonnet", "claude-sonnet-5-5"),
    ("haiku", "claude-haiku-5-5"),
    ("fable", "claude-fable-5-1"),
];

fn reports(state: &Path) -> PathBuf {
    state.join("members")
}

fn doings(state: &Path) -> PathBuf {
    state.join("doing")
}

fn compactions(state: &Path) -> PathBuf {
    state.join("compact")
}

/// What each conversation heard of its member's place in the team, by session: kept from one launch to the next, a
/// conversation may be resumed.
fn prompted(state: &Path) -> PathBuf {
    state.join("prompted")
}

/// A member's own files, by name: its report, the line on what it does, the compaction asked for it.
fn member_files(state: &Path, member: &str) -> [PathBuf; 3] {
    [
        reports(state).join(format!("{member}.json")),
        doings(state).join(format!("{member}.json")),
        compactions(state).join(member),
    ]
}

/// A member renamed: its files follow it.
pub fn rename(state: &Path, from: &str, to: &str) -> Result<()> {
    for (old, new) in member_files(state, from).into_iter().zip(member_files(state, to)) {
        if old.exists() {
            fs::rename(&old, &new).with_context(|| old.display().to_string())?;
        }
    }
    for (session, prompted) in prompted_of(state, from) {
        write_prompted(state, &session, &Prompted { member: Some(to.to_string()), ..prompted })?;
    }
    Ok(())
}

/// A member removed: its files go with it.
pub fn forget(state: &Path, member: &str) -> Result<()> {
    let conversations =
        prompted_of(state, member).into_iter().filter_map(|(session, _)| prompted_file(state, &session));
    for file in member_files(state, member).into_iter().chain(conversations) {
        if file.exists() {
            fs::remove_file(&file).with_context(|| file.display().to_string())?;
        }
    }
    Ok(())
}

/// A new launch starts from the settings: no report, no session noted, no line on what a member does, no compaction
/// asked, no zone of the dashboard, left from the last one.
pub fn reset(state: &Path) -> Result<()> {
    for dir in [reports(state), state.join("sessions"), doings(state), compactions(state)] {
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| dir.display().to_string())?;
        }
    }
    let zones = state.join(board::ZONES);
    if zones.exists() {
        fs::remove_file(&zones).with_context(|| zones.display().to_string())?;
    }
    Ok(())
}

pub fn report(state: &Path, member: &str) -> Option<Report> {
    serde_json::from_str(&fs::read_to_string(reports(state).join(format!("{member}.json"))).ok()?).ok()
}

/// What the mod puts on each of a member's requests: the model and effort the team's settings give it now, where
/// they differ from what its Claude was launched with. A model as a full id, a request names no alias. Back to Claude
/// Code's default, or a model with no id known here (`opus[1m]`), waits for the member's next launch: there is no
/// telling a request to use the default.
pub fn override_of(member: &MemberInfo) -> Override {
    let launched = |flag: &str| claude::flag_value(&member.argv, flag);
    let model = member.model.as_deref().filter(|m| launched("--model") != Some(*m)).and_then(model_id);
    let effort = member.effort.as_deref().filter(|e| launched("--effort") != Some(*e) && EFFORTS.contains(e));
    Override { model, effort: effort.map(String::from) }
}

/// Whether a member whose Claude was launched with `argv` gets the model and effort its settings now give (`model`,
/// `effort`) without starting again: they are those it was launched with, or the mod puts them on each of its
/// requests. Not Claude Code's default once launched with another (a request cannot ask for the default), nor a model
/// with no id known here (`opus[1m]`), nor an unknown effort: the member must start again, on its conversation.
pub fn applies_live(model: Option<&str>, effort: Option<&str>, argv: &[String]) -> bool {
    let model = model == claude::flag_value(argv, "--model") || model.and_then(model_id).is_some();
    let effort = effort == claude::flag_value(argv, "--effort") || effort.is_some_and(|e| EFFORTS.contains(&e));
    model && effort
}

/// `opus` → `claude-opus-5-5`; a `claude-…` id as it is; anything else unknown.
pub fn model_id(model: &str) -> Option<String> {
    match ALIASES.iter().find(|(alias, _)| *alias == model) {
        Some((_, id)) => Some(id.to_string()),
        None => model.starts_with("claude-").then(|| model.to_string()),
    }
}

/// The journal's command, in the team's language.
fn team_command(lang: Lang) -> &'static str {
    match lang {
        Lang::Fr => "equipe",
        Lang::En => "team",
    }
}

/// The menu's command, the same in every language.
const MENU_COMMAND: &str = "recruit";

pub fn run(event: Event, state: &Path, member: &str, command: Option<&str>) -> Result<()> {
    let snapshot = Snapshot::read(state)?;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let input: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let reply = match event {
        Event::Start => start(&snapshot, state, member, &input)?,
        Event::Tick => tick(&snapshot, state, member, &input)?,
        Event::Submit => {
            let session = input["session"].as_str().unwrap_or_default();
            let text = input["text"].as_str().unwrap_or_default();
            // Compacted just before: told with the prompt, the next tick would come after it.
            if input["compacted"] == true {
                compacted_away(state, session)?;
            }
            match submit(state, member, session, text, || live::prompt(state, member))? {
                Some(note) => json!({ "context": note }),
                None => json!({}),
            }
        }
        Event::Command => {
            json!({ "text": command_text(&snapshot, state, member, command.unwrap_or_default())? })
        }
    };
    println!("{reply}");
    Ok(())
}

fn start(snapshot: &Snapshot, state: &Path, member: &str, input: &Value) -> Result<Value> {
    let known: Vec<&str> = input["commands"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let specs = [
        json!({
            "name": team_command(snapshot.lang),
            "description": t!(
                "Fait passer le journal de complet à réduit, puis masqué",
                "Takes the journal from full to reduced, then hidden"
            ),
        }),
        json!({
            "name": MENU_COMMAND,
            "description": t!(
                "Menu de l'équipe : membres, modèles, rôles, relances",
                "The team's menu: members, models, roles, restarts"
            ),
        }),
    ];
    // A command of the same name (the user's, a plugin's) keeps its place.
    let register: Vec<Value> =
        specs.into_iter().filter(|s| !known.contains(&s["name"].as_str().unwrap_or_default())).collect();
    let info = snapshot.members.iter().find(|m| m.name == member);
    if let (Some(info), Some(session)) = (info, input["session"].as_str()) {
        // At best: the mod would not start at all otherwise.
        let _ = started(snapshot, state, session, info);
    }
    Ok(json!({ "register": register, "quiet": info.is_some_and(|m| m.quiet) }))
}

/// How often the context's breakdown is asked, in seconds: estimated by the session itself, but over all its
/// context.
const BREAKDOWN_EVERY: i64 = 300;

fn tick(snapshot: &Snapshot, state: &Path, member: &str, input: &Value) -> Result<Value> {
    let now = board::now();
    let before = report(state, member).unwrap_or_default();
    let mut report: Report = serde_json::from_value(input.clone()).unwrap_or_default();
    report.at = now;
    // Just compacted, the session tells no context until it answers again: the breakdown's count stands in, asked at
    // once, until the session answers.
    let compacted = input["compaction"]["done"] == true;
    let measured_now = report.threshold.is_some();
    let counted = report.threshold.as_ref().and_then(|t| t.used);
    let measured = report.context.as_ref().is_some_and(|c| c.percent.is_some());
    if !measured && (compacted || before.context.as_ref().is_some_and(|c| c.estimated)) {
        let window = report.context.as_ref().and_then(|c| c.window).filter(|w| *w > 0);
        let waiting = ContextUse { window, estimated: true, ..Default::default() };
        report.context = Some(match (counted, window) {
            (Some(used), Some(window)) => ContextUse {
                percent: Some(used as f64 * 100.0 / window as f64),
                tokens: Some(used),
                window: Some(window),
                estimated: true,
            },
            _ if compacted => waiting,
            _ => before.context.clone().unwrap_or(waiting),
        });
    }
    match &mut report.threshold {
        Some(threshold) => threshold.at = now,
        None => report.threshold = before.threshold,
    }
    match &mut report.compaction {
        Some(compaction) => compaction.at = now,
        None => report.compaction = before.compaction,
    }
    let due = report.threshold.as_ref().is_none_or(|t| {
        now - t.at >= BREAKDOWN_EVERY || (t.model.is_some() && report.model.is_some() && t.model != report.model)
    });
    // A session that does not answer it (an older Claude Code) is not asked at each tick.
    let waiting = Some(before.breakdown_asked).filter(|at| *at > 0 && !measured_now);
    let breakdown = compacted || (due && waiting.is_none_or(|at| now - at >= BREAKDOWN_EVERY));
    report.breakdown_asked = if breakdown { now } else { waiting.unwrap_or(0) };
    fs::create_dir_all(reports(state))?;
    write_atomic(&reports(state).join(format!("{member}.json")), &serde_json::to_string(&report)?)?;

    if compacted || input["compacted"] == true {
        compacted_away(state, input["session"].as_str().unwrap_or_default())?;
    }
    let info = snapshot.member(member);
    let quiet = info.is_some_and(|m| m.quiet);
    let mut reply = json!({ "override": info.map(override_of).unwrap_or_default(), "quiet": quiet });
    if breakdown {
        // Just compacted, counted with the token-count API (less than a second): the session's own estimate counts a
        // fifth too many. Else the estimate, for the threshold alone.
        reply["breakdown"] = json!(if compacted { "full" } else { "summary" });
    }
    if let Some(id) = take_compaction(state, member, now) {
        reply["compact"] = json!(id);
    }
    if let Some(ask) = follow(snapshot, state, member, input)? {
        reply["summarize"] = ask;
    }
    Ok(reply)
}

/// Asks a member's session to compact, from the dashboard: its mod takes the request at its next tick, within
/// ten seconds, or the request lapses. Tried once: a session at work refuses it.
pub fn request_compaction(state: &Path, member: &str) -> Result<()> {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let request = json!({ "id": millis.to_string(), "until": board::now() + COMPACTION_LAPSE });
    fs::create_dir_all(compactions(state))?;
    let file = compactions(state).join(member);
    fs::write(&file, request.to_string()).with_context(|| file.display().to_string())
}

/// How long a compaction request waits for the member's mod, in seconds.
const COMPACTION_LAPSE: i64 = 10;

/// The compaction asked for a member, taken: its id, unless it lapsed.
fn take_compaction(state: &Path, member: &str, now: i64) -> Option<String> {
    let file = compactions(state).join(member);
    let request: Value = serde_json::from_str(&fs::read_to_string(&file).ok()?).ok()?;
    let _ = fs::remove_file(&file);
    let id = request["id"].as_str()?;
    (request["until"].as_i64()? >= now).then(|| id.to_string())
}

/// What a conversation heard of its member's place in the team: the prompt it started with, when known, and the
/// last one it was given, at its start or in a note. Both whole, as `prompt::build` writes them.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Prompted {
    /// Whose conversation it is: forgotten with the member, or once a new conversation replaces it.
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    seen: Option<String>,
}

/// A session id as a file name: an id as Claude Code makes them, nothing else.
fn prompted_file(state: &Path, session: &str) -> Option<PathBuf> {
    let fit = !session.is_empty() && session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    fit.then(|| prompted(state).join(format!("{session}.json")))
}

fn read_prompted(state: &Path, session: &str) -> Option<Prompted> {
    serde_json::from_str(&fs::read_to_string(prompted_file(state, session)?).ok()?).ok()
}

fn write_prompted(state: &Path, session: &str, value: &Prompted) -> Result<()> {
    let Some(file) = prompted_file(state, session) else { return Ok(()) };
    fs::create_dir_all(prompted(state))?;
    write_atomic(&file, &serde_json::to_string(value)?)
}

/// How long what a conversation heard is kept, unless it goes on: resumed later, it is told its place again.
const PROMPTED_DAYS: u64 = 30;

/// A member's session just started. A new conversation got its prompt on the command line: what it holds is known.
/// A resumed one keeps the prompt it started with, as noted then if it was, whatever the command line says (it names
/// the prompt a new conversation would get); a resumed one never noted (before this recruit, or long ago) is told its
/// place at its first prompt. Resumed, a conversation has its file already: a new one gets it at its first message.
fn started(snapshot: &Snapshot, state: &Path, session: &str, member: &MemberInfo) -> Result<()> {
    forget_old_prompts(state);
    if claude::transcript(&snapshot.dir, snapshot.config_dir.as_deref(), session).exists() {
        return Ok(());
    }
    // A new conversation: the member's earlier ones are left, never to be resumed (recruit resumes the latest).
    for (other, _) in prompted_of(state, &member.name) {
        if let Some(file) = prompted_file(state, &other) {
            let _ = fs::remove_file(file);
        }
    }
    let Some(file) = claude::flag_value(&member.argv, "--append-system-prompt-file") else { return Ok(()) };
    let Ok(text) = fs::read_to_string(file) else { return Ok(()) };
    let prompted = Prompted { member: Some(member.name.clone()), base: Some(text.clone()), seen: Some(text) };
    write_prompted(state, session, &prompted)
}

/// The conversations noted as `member`'s, by session.
fn prompted_of(state: &Path, member: &str) -> Vec<(String, Prompted)> {
    let Ok(entries) = fs::read_dir(prompted(state)) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let session = entry.file_name().to_string_lossy().strip_suffix(".json")?.to_string();
            let prompted = read_prompted(state, &session)?;
            (prompted.member.as_deref() == Some(member)).then_some((session, prompted))
        })
        .collect()
}

fn forget_old_prompts(state: &Path) {
    let Ok(entries) = fs::read_dir(prompted(state)) else { return };
    let limit = std::time::Duration::from_secs(PROMPTED_DAYS * 24 * 3600);
    for entry in entries.flatten() {
        let age = entry.metadata().and_then(|m| m.modified()).ok().and_then(|at| at.elapsed().ok());
        if age.is_some_and(|age| age > limit) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// For a prompt about to reach a member: the note to join to it, when the member's place in the team (`current`, its
/// prompt as recruit would write it now) is not what its conversation last heard. Told once: the next prompts go
/// bare until the team changes again. A command is left alone, the model may never read it.
pub fn submit(
    state: &Path,
    member: &str,
    session: &str,
    text: &str,
    current: impl FnOnce() -> Result<String>,
) -> Result<Option<String>> {
    if text.trim_start().starts_with('/') || prompted_file(state, session).is_none() {
        return Ok(None);
    }
    let mut prompted = read_prompted(state, session).unwrap_or_default();
    prompted.member = Some(member.to_string());
    let current = current()?;
    if prompted.seen.as_deref() == Some(current.as_str()) {
        return Ok(None);
    }
    let note = format!(
        "{}\n\n{}",
        t!(
            "Note de recruit, que l'utilisateur ne voit pas : voici ta place dans l'équipe telle qu'elle est maintenant. Elle prime sur ce qu'en dit ton prompt système (ton nom, ton rôle, tes interlocuteurs, les membres, les règles communes), qui peut dater d'une composition plus ancienne de l'équipe.",
            "Note from recruit, which the user does not see: here is your place in the team as it stands now. It prevails over what your system prompt says of it (your name, your role, your contacts, the members, the shared rules), which may date from an earlier line-up of the team."
        ),
        current.trim_end()
    );
    prompted.seen = Some(current);
    write_prompted(state, session, &prompted)?;
    Ok(Some(note))
}

/// The conversation was compacted: the notes it was given may have gone with what it summed up, and only its starting
/// prompt is sure to remain. The next prompt tells its place again, unless that prompt says it all.
fn compacted_away(state: &Path, session: &str) -> Result<()> {
    match read_prompted(state, session) {
        Some(prompted) => write_prompted(state, session, &Prompted { seen: prompted.base.clone(), ..prompted }),
        None => Ok(()),
    }
}

/// What a member does, in a line, from the request that opened its turn: and how far its conversation was read.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Doing {
    session: String,
    offset: u64,
    /// The last request that opened a turn: its prompt id and its text.
    prompt: Option<String>,
    request: String,
    /// The request the model was asked about, once: answered or not, not asked again.
    asked: Option<String>,
    task: Option<Task>,
}

/// What a member does, as the model put the request that opened its last turn: in progress (« Je publie la version
/// 0.6.2 »), and once done (« Version 0.6.2 publiée »), when the model gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub now: String,
    #[serde(default)]
    pub done: Option<String>,
}

pub fn doing(state: &Path, member: &str) -> Option<Task> {
    read_doing(state, member).task
}

fn read_doing(state: &Path, member: &str) -> Doing {
    fs::read_to_string(doings(state).join(format!("{member}.json")))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The model the line on what a member does is asked of, through the member's own session.
const SUMMARY_MODEL: &str = "haiku";
/// Of a request, what the model reads.
const REQUEST_CHARS: usize = 2000;
/// A line on what a member does, at most: the model is asked for 60 characters.
const LINE_CHARS: usize = 80;

/// Reads on in the member's conversation for the requests that open its turns, typed by the user or sent by a
/// teammate; takes the model's answer about the last one; and, when a new one came, what to ask the model.
fn follow(snapshot: &Snapshot, state: &Path, member: &str, input: &Value) -> Result<Option<Value>> {
    let mut doing = read_doing(state, member);
    let before = doing.clone();
    let answer = &input["summary"];
    if answer["id"].as_str().is_some_and(|id| doing.asked.as_deref() == Some(id))
        && let Some(task) = answer["text"].as_str().and_then(task_of)
    {
        doing.task = Some(task);
    }
    if let Some(session) = input["session"].as_str().filter(|s| !s.is_empty()) {
        if session != doing.session {
            doing = Doing { session: session.to_string(), task: doing.task, ..Default::default() };
        }
        let file = claude::transcript(&snapshot.dir, snapshot.config_dir.as_deref(), session);
        for line in turn_lines(&file, &mut doing.offset) {
            if let Some((id, text)) = turn_request(&line) {
                doing.prompt = Some(id);
                doing.request = text;
            }
        }
    }
    let ask = match &doing.prompt {
        Some(prompt) if doing.asked.as_ref() != Some(prompt) => {
            doing.asked = Some(prompt.clone());
            Some(summary_ask(prompt, doing.task.as_ref(), &doing.request))
        }
        _ => None,
    };
    if doing != before {
        fs::create_dir_all(doings(state))?;
        write_atomic(&doings(state).join(format!("{member}.json")), &serde_json::to_string(&doing)?)?;
    }
    Ok(ask)
}

/// The whole lines `file` gained past `offset` that may open a turn; `offset` moves past every whole line, from the
/// start again when the file shrank. Read line by line: a resumed conversation may weigh megabytes.
fn turn_lines(file: &Path, offset: &mut u64) -> Vec<String> {
    let Ok(mut handle) = fs::File::open(file) else { return Vec::new() };
    if handle.metadata().map_or(0, |m| m.len()) < *offset {
        *offset = 0;
    }
    if handle.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut reader = BufReader::new(handle);
    let mut found = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(read) if read > 0 && line.ends_with(b"\n") => *offset += read as u64,
            // The end, or a line still being written.
            _ => break,
        }
        let text = String::from_utf8_lossy(&line);
        if text.contains("\"turnOrigin\"") {
            found.push(text.trim_end().to_string());
        }
    }
    found
}

/// The request a line of a conversation opens a turn with, typed by the user or sent by a teammate: its prompt id
/// and its text, the teammate's message without its envelope.
fn turn_request(line: &str) -> Option<(String, String)> {
    let row: Value = serde_json::from_str(line).ok()?;
    let origin = row["turnOrigin"].as_str()?;
    if row["type"] != "user" || !["human", "peer"].contains(&origin) {
        return None;
    }
    let id = row["promptId"].as_str()?.to_string();
    let content = &row["message"]["content"];
    let text = match row["origin"]["body"].as_str() {
        Some(body) if origin == "peer" => body.to_string(),
        _ => match content {
            Value::String(text) => text.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        },
    };
    let text = text.trim();
    (!text.is_empty()).then(|| (id, text.chars().take(REQUEST_CHARS).collect()))
}

/// What to ask the model about a request, in the team's language: what it asks to do, in progress and once done; or
/// the lines from before when it asks nothing.
fn summary_ask(id: &str, before: Option<&Task>, request: &str) -> Value {
    let system = t!(
        "Tu écris l'état d'un agent : ce que le message qu'il vient de recevoir lui demande de faire. Réponds par deux lignes en français, de 60 caractères au plus, sans guillemets, sans étiquette ni point final : d'abord la tâche en cours, à la première personne et au présent ; puis la même tâche une fois finie, au participe passé, sans sujet. Exemple :\nJe publie la version 0.6.2\nVersion 0.6.2 publiée",
        "You write an agent's status: what the message it just received asks it to do. Answer with two lines in English, of 60 characters at most, without quotes, labels or a final period: first the task in progress, in the first person and the present tense; then the same task once done, as a past participle, with no subject. Example:\nI am releasing version 0.6.2\nVersion 0.6.2 released"
    );
    let before = match before {
        Some(task) => format!("{}\n{}", task.now, task.done.as_deref().unwrap_or(&task.now)),
        None => t!("aucune", "none"),
    };
    let prompt = t!(
        "Message reçu :\n{}\n\nLignes actuelles :\n{}\n\nSi ce message demande un travail, écris les deux nouvelles lignes. S'il n'en demande aucun (un remerciement, un accord, une information sans demande), recopie les lignes actuelles. Réponds par les deux lignes seules.",
        "Message received:\n{}\n\nCurrent lines:\n{}\n\nIf this message asks for some work, write the two new lines. If it asks for none (thanks, an agreement, information with no request), copy the current lines. Answer with the two lines alone.",
        request,
        before
    );
    json!({ "id": id, "model": SUMMARY_MODEL, "system": system, "prompt": prompt, "maxTokens": 120 })
}

/// The model's answer as a task: its first line in progress, its second once done; the second may be missing.
fn task_of(text: &str) -> Option<Task> {
    let mut lines = text.lines().filter_map(clean_line);
    Some(Task { now: lines.next()?, done: lines.next() })
}

/// What the model may put before a line despite being asked not to.
const LABELS: [&str; 9] =
    ["en cours", "fini", "finie", "terminé", "terminée", "in progress", "ongoing", "done", "finished"];

/// A line of the model's answer, bare: no bullet, label or quotes around it, no final period, not too long; and no
/// control character, which the dashboard would print (an escape sequence asked for in a message, a clipboard write).
fn clean_line(line: &str) -> Option<String> {
    let line: String = line.chars().filter(|c| !c.is_control()).collect();
    let mut line = line.trim().trim_start_matches(['-', '•', '*']).trim_start();
    if let Some((label, rest)) = line.split_once(':')
        && LABELS.contains(&label.trim().to_lowercase().as_str())
    {
        line = rest;
    }
    let line = line.trim_matches(|c: char| "\"'«»“”".contains(c) || c.is_whitespace());
    let line = line.strip_suffix('.').unwrap_or(line).trim_end();
    (!line.is_empty()).then(|| line.chars().take(LINE_CHARS).collect())
}

fn command_text(snapshot: &Snapshot, state: &Path, member: &str, command: &str) -> Result<String> {
    if command == team_command(snapshot.lang) {
        return Ok(board::toggle(snapshot)?.said());
    }
    if command != MENU_COMMAND {
        bail!("recruit _mod: {command}");
    }
    let before = menu::last_opened(state);
    if !live::open_menu(state, member)? {
        return Ok(t!(
            "Aucun client tmux n'affiche ce panneau : {}r ouvre le menu depuis le terminal de l'équipe.",
            "No tmux client shows this pane: {}r opens the menu from the team's terminal.",
            crate::tmux::ALT
        ));
    }
    // tmux opens the window on the side, and says nothing when it refuses it: the menu, once started, says so.
    let until = std::time::Instant::now() + MENU_WAIT;
    while std::time::Instant::now() < until {
        if menu::opened_since(state, before.as_deref()) {
            return Ok(t!("Menu de l'équipe ouvert.", "Team menu opened."));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(t!(
        "tmux n'a pas ouvert le menu : une autre fenêtre est sans doute déjà ouverte sur ce terminal (Échap la ferme).",
        "tmux did not open the menu: another window is likely open on this terminal already (Esc closes it)."
    ))
}

/// How long `/recruit` waits for the menu to start in its window.
const MENU_WAIT: std::time::Duration = std::time::Duration::from_millis(2000);

/// What a member's next requests use: the model and effort recruit puts on them, else the session's own, with the
/// effort its last request had on that model; or, before any (or without the mod), those its settings give.
pub fn model_and_effort(state: &Path, member: &MemberInfo) -> (Option<String>, Option<String>) {
    let report = report(state, &member.name).unwrap_or_default();
    let set = override_of(member);
    // Just after a change of model in the session (`/model`), the last request still had the one before.
    let same = report.own.model.is_none() || report.own.model == report.model;
    let configured = member.effort.clone().filter(|e| EFFORTS.contains(&e.as_str()));
    let effort = set.effort.or(report.own.effort.filter(|_| same)).or(configured);
    // Without the mod (an older Claude Code), the one its settings give.
    (set.model.or(report.model).or(report.own.model).or(member.model.clone()), effort)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(lang: Lang) -> Snapshot {
        Snapshot {
            team: "web".into(),
            session: "web".into(),
            socket: "recruit".into(),
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
        assert!(
            live(Some("opus"), Some("high")) && live(Some("sonnet"), Some("high")) && live(Some("opus"), Some("max"))
        );
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
        let submit =
            |session: &str, text: &str, now: &str| submit(state, "dev", session, text, || Ok(now.to_string())).unwrap();

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
        assert!(super::submit(state, "dev", "s4", "ok", || bail!("no team")).is_err());
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
        submit(state, "dev", "s2", "ok", || Ok("x".into())).unwrap();
        submit(state, "lead", "s9", "ok", || Ok("x".into())).unwrap();
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
        rename(state, "dev", "dev-rust").unwrap();
        assert!(report(state, "dev").is_none());
        assert_eq!(report(state, "dev-rust").unwrap().model.as_deref(), Some("claude-opus-5-5"));
        assert!(compactions(state).join("dev-rust").exists());
        forget(state, "dev-rust").unwrap();
        assert!(report(state, "dev-rust").is_none() && !compactions(state).join("dev-rust").exists());
        // Nothing to move or remove: nothing done.
        rename(state, "qa", "test").unwrap();
        forget(state, "qa").unwrap();

        fs::write(state.join(board::ZONES), "[]").unwrap();
        tick(&s, state, "dev", &json!({})).unwrap();
        fs::create_dir_all(prompted(state)).unwrap();
        reset(state).unwrap();
        assert!(report(state, "dev").is_none());
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
        assert_eq!(
            turn_request(&opening("p1", "peer", "Publie la 0.6.2")),
            Some(("p1".into(), "Publie la 0.6.2".into()))
        );
        assert_eq!(
            turn_request(&opening("p2", "human", "  corrige le test \n")),
            Some(("p2".into(), "corrige le test".into()))
        );
        let typed = json!({"type": "user", "promptId": "p3", "turnOrigin": "human", "message": {"content": "vas-y"}});
        assert_eq!(turn_request(&typed.to_string()), Some(("p3".into(), "vas-y".into())));
        // A tool's result belongs to the turn, a task's notice opens none.
        let result = json!({"type": "user", "promptId": "p3", "message": {"content": [{"type": "tool_result", "content": "ok"}]}});
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
        fs::write(&file, format!("{{\"type\":\"assistant\"}}\n{first}{}", &opening("p2", "human", "deux")[..20]))
            .unwrap();
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
        tick("dev", json!({"compaction": {"id": id, "error": "a turn is running"}}));
        tick("dev", json!({}));
        let kept = report(state, "dev").unwrap().compaction.unwrap();
        assert_eq!((kept.id, kept.done, kept.error.as_deref()), (id, false, Some("a turn is running")));
        // Lapsed: dropped.
        fs::write(compactions(state).join("dev"), json!({"id": "1", "until": board::now() - 1}).to_string()).unwrap();
        assert!(tick("dev", json!({})).get("compact").is_none());
        assert!(!compactions(state).join("dev").exists());
        // Another member's request stays for it.
        request_compaction(state, "lead").unwrap();
        assert!(tick("dev", json!({})).get("compact").is_none());
        assert!(tick("lead", json!({})).get("compact").is_some());
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
        assert!(
            tick("lead", json!({}))["breakdown"] == "summary" && tick("lead", json!({})).get("breakdown").is_none()
        );
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
}
