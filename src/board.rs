// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A running team's panels, in a column on the right of its contacts: the dashboard (who works, who rests, who
//! waits for the user, with each one's activity over the last half hour) over the journal (the messages the
//! members send each other). Both read what Claude Code already records: `claude agents --json` and the
//! conversation files; the mod adds the context and the account's usage.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::style::{Color, Stylize};
use crossterm::terminal::{self, ClearType};
use crossterm::{cursor, queue};
use serde::{Deserialize, Serialize};

use crate::bridge::{Helper, HelperStatus, Report, Task};
use crate::claude::{self, Running};
use crate::config::{TmuxSettings, write_atomic};
use crate::i18n::Lang;
use crate::look::{self, FRAME, Glyphs, RAINBOW, State, duration, effort_color, effort_sign, family, frame};
use crate::state::Snapshot;
use crate::tmux::{self, ALT, Pane, Tmux};
use crate::{bridge, t};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    Dashboard,
    Journal,
    /// Hides the journal, or shows it again.
    Toggle,
}

pub fn run(kind: Kind, state: &Path) -> Result<()> {
    let snapshot = Snapshot::read(state)?;
    match kind {
        Kind::Dashboard => dashboard(&snapshot, state),
        Kind::Journal => journal(snapshot, state),
        Kind::Toggle => {
            println!("{}", toggle(&snapshot)?.said());
            Ok(())
        }
    }
}

pub fn dashboard_title(lang: Lang) -> &'static str {
    match lang {
        Lang::Fr => "Tableau de bord",
        Lang::En => "Dashboard",
    }
}

pub fn journal_title(_: Lang) -> &'static str {
    "Journal"
}

/// How often the panels look again: `claude agents --json`, the mod's reports, the activity samples.
const TICK: Duration = Duration::from_secs(1);

/// The members' sessions by name, and every session's name by process: replies are addressed to a process.
#[derive(Default)]
struct Sessions {
    members: HashMap<String, Running>,
    by_pid: HashMap<u32, String>,
    /// Sessions open elsewhere under a member's name, with the folder they work in: messages by name could go astray.
    elsewhere: Vec<(String, Option<String>)>,
    error: Option<String>,
}

fn sessions(s: &Snapshot) -> Sessions {
    match claude::running(&s.claude, s.config_dir.as_deref()) {
        Ok(running) => sorted(s, running),
        Err(error) => Sessions { error: Some(format!("{error:#}")), ..Default::default() },
    }
}

/// `text` without what printing it as it is would let it do: no control character (an escape sequence, a clipboard
/// write), no formatting one (text turned right to left, invisible characters).
fn printable(text: &str) -> String {
    let formatting = |c: char| {
        matches!(c,
            '\u{AD}' | '\u{61C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
    };
    text.chars().filter(|&c| !c.is_control() && !formatting(c)).collect()
}

/// The sessions `claude agents --json` gives, sorted out for the team.
fn sorted(s: &Snapshot, running: Vec<Running>) -> Sessions {
    let mut found = Sessions::default();
    for session in running {
        let Some(name) = session.name.clone() else { continue };
        if let Some(pid) = session.pid {
            found.by_pid.insert(pid, name.clone());
        }
        if !s.members.iter().any(|m| m.name == name) {
            continue;
        }
        let here = session.cwd.as_deref().is_some_and(|cwd| Path::new(cwd) == s.dir);
        if here {
            found.members.insert(name, session);
            continue;
        }
        let folder = session.cwd.as_deref().and_then(|cwd| Path::new(cwd).file_name());
        let double = (name, folder.map(|f| printable(&f.to_string_lossy())));
        if !found.elsewhere.contains(&double) {
            found.elsewhere.push(double);
        }
    }
    // In the file's order, whatever order `claude agents` gives: the line stays as it is from one look to the next.
    let order = |name: &String| s.members.iter().position(|m| &m.name == name);
    found.elsewhere.sort_by(|a, b| order(&a.0).cmp(&order(&b.0)).then_with(|| a.1.cmp(&b.1)));
    found
}

/// How far back the activity curves go, in seconds.
const HISTORY: i64 = 30 * 60;
/// From no activity to all the time.
const LEVELS: [char; 5] = ['⣀', '⣠', '⣤', '⣶', '⣿'];
/// Narrower than this, cards go one per line.
const MIN_CARD: usize = 30;

fn member_color(s: &Snapshot, name: &str) -> Color {
    s.members.iter().position(|m| m.name == name).map_or(Color::Reset, look::member_color)
}

/// A member whose Claude runs, as its card shows it.
#[derive(Debug, Clone)]
struct Card {
    name: String,
    color: Color,
    contact: bool,
    state: State,
    /// Seconds in this state.
    since: u64,
    /// Of the model's window.
    context: Option<f64>,
    /// How near the session is to compacting on its own.
    pressure: Pressure,
    /// The model's family (`Opus`), and the effort's level its requests go with.
    model: Option<String>,
    effort: Option<String>,
    /// What it does, in progress and once done.
    doing: Option<Task>,
    /// Its state each second, as (time, working).
    samples: Vec<(i64, bool)>,
    /// The agents its session started: subagents, teammates.
    helpers: Vec<Helper>,
}

/// How near a session is to compacting on its own, which colors its context.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pressure {
    /// Far from it, or not measured yet.
    #[default]
    Calm,
    Near,
    /// Where Claude Code warns.
    Warning,
}

/// From this share of the tokens at which a session compacts on its own, its context turns orange.
const NEAR_COMPACTION: f64 = 0.8;
/// So many tokens before them, Claude Code warns, and the context turns red.
const COMPACTION_WARNING: u64 = 20_000;

impl Pressure {
    fn of(tokens: Option<u64>, compacts_at: Option<u64>) -> Self {
        let (Some(tokens), Some(at)) = (tokens, compacts_at) else { return Pressure::Calm };
        if tokens >= at.saturating_sub(COMPACTION_WARNING) {
            Pressure::Warning
        } else if tokens as f64 >= at as f64 * NEAR_COMPACTION {
            Pressure::Near
        } else {
            Pressure::Calm
        }
    }
}

/// What the dashboard draws.
#[derive(Debug, Clone, Default)]
struct Board {
    now: i64,
    error: Option<String>,
    /// The account's windows: label, percent used, seconds before it resets.
    usage: Vec<(String, f64, Option<u64>)>,
    cards: Vec<Card>,
    absent: Vec<String>,
    /// The spinners' image.
    frame: usize,
    glyphs: Glyphs,
    /// Sessions open elsewhere under a member's name, with their folder.
    elsewhere: Vec<(String, Option<String>)>,
}

/// The team as it stands, `team.json` read again: members added, removed or renamed while it runs. As it was, when the
/// file cannot be read (written again at this very moment).
fn reread(s: &mut Snapshot, state: &Path) {
    if let Ok(now) = Snapshot::read(state) {
        *s = now;
    }
}

fn dashboard(first: &Snapshot, state: &Path) -> Result<()> {
    let mut out = std::io::stdout();
    queue!(out, cursor::Hide)?;
    std::thread::scope(|scope| {
        // `claude agents --json` takes a tenth of a second or more: asked on the side, it does not stall the spinners.
        let (sender, news) = mpsc::channel();
        scope.spawn(move || {
            // The clients asked at each look: one can attach again from another terminal.
            let tmux = Tmux::new(&TmuxSettings { socket: Some(first.socket.clone()), ..Default::default() }).ok();
            let clients = || tmux.as_ref().map(|t| t.client_terminals(&first.session)).unwrap_or_default();
            let mut s = first.clone();
            loop {
                reread(&mut s, state);
                let look = (sessions(&s), Glyphs::of(&clients()));
                if sender.send((s.clone(), look)).is_err() {
                    break;
                }
                std::thread::sleep(TICK);
            }
        });
        // The states written on the side too: the drawing never waits for the disk.
        let (states, written) = mpsc::channel::<States>();
        scope.spawn(move || {
            for states in written {
                // Not written, the menu asks `claude agents` itself, once the file is old.
                let _ = save_states(state, &states);
            }
        });
        let start = Instant::now();
        let mut memory = Memory::default();
        let mut board = Board::default();
        let mut screen = Screen::default();
        // What the zones file says: nothing yet, it may be the last launch's.
        let mut saved: Option<Vec<Zone>> = None;
        let mut fresh = news.recv().ok();
        loop {
            if let Some((s, (sessions, glyphs))) = fresh.take() {
                board = Board { glyphs, ..memory.board(&s, state, &sessions) };
                // A look that failed says nothing of the states: left to age, the file sends the menu to ask itself.
                if board.error.is_none() {
                    let _ = states.send(memory.states(&board, SystemTime::now(), Instant::now()));
                }
            }
            for card in &mut board.cards {
                if let Some((_, at)) = memory.since.get(&card.name) {
                    card.since = at.elapsed().as_secs();
                }
            }
            board.now = now();
            board.frame = frame(start.elapsed());
            screen.draw(&mut out, &board)?;
            if saved.as_ref() != Some(&screen.zones) {
                // Not written, it is only the clicks that miss; tried again when the layout changes.
                let _ = save_zones(state, &screen.zones);
                saved = Some(screen.zones.clone());
            }
            // The spinners turn while someone works; otherwise only the clock moves, redrawn on the second.
            let working = board.cards.iter().any(|c| c.state == State::Working);
            fresh = match news.recv_timeout(if working { FRAME } else { next_second() }) {
                Ok(look) => Some(look),
                Err(RecvTimeoutError::Timeout) => None,
                // Only a panic stops the looking, and the scope passes it on.
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            };
        }
    })
}

/// What the dashboard remembers from one look to the next: since when each member is in its state, and what it did.
#[derive(Default)]
struct Memory {
    since: HashMap<String, (State, Instant)>,
    activity: HashMap<String, VecDeque<(i64, bool)>>,
    /// The name each session last showed under, and when: a member renamed on its conversation keeps its time and
    /// its curve, across the seconds it takes to start again.
    sessions: HashMap<String, (String, Instant)>,
}

/// Older than this, in seconds, a member's report says nothing of the agents it started: its mod ticks every 2 s.
const FRESH_REPORT: i64 = 10;

/// The agents a member's session started that its card shows at `now`: while its mod reports (a member stopped took
/// them along), those still shown.
fn helpers_shown(report: &Report, now: i64) -> Vec<Helper> {
    if now - report.at > FRESH_REPORT {
        return Vec::new();
    }
    report.helpers.iter().filter(|h| h.shown(now)).cloned().collect()
}

/// How long the dashboard remembers a session it no longer sees, under its last name.
const KEEP: Duration = Duration::from_secs(60);

impl Memory {
    /// The states of `board`, just made, for `STATES`, at `now` on the clock, which is `seen` on the dashboard's:
    /// since when in seconds since the epoch, counted back in milliseconds, so that it holds from one look to the next.
    fn states(&self, board: &Board, now: SystemTime, seen: Instant) -> States {
        let at = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let members = board.cards.iter().filter_map(|card| {
            let (state, since) = self.since.get(&card.name)?;
            let since = (at - seen.saturating_duration_since(*since).as_millis() as i64).div_euclid(1000);
            Some((card.name.clone(), MemberState { state: *state, since }))
        });
        States { at: at.div_euclid(1000), members: members.collect(), absent: board.absent.clone() }
    }

    /// The board for the sessions just found.
    fn board(&mut self, s: &Snapshot, state: &Path, sessions: &Sessions) -> Board {
        let now = now();
        // One moment for the whole look: members seen in the same state at once keep the file's order.
        let seen = Instant::now();
        let mut board =
            Board { now, error: sessions.error.clone(), elsewhere: sessions.elsewhere.clone(), ..Default::default() };
        // Hidden for screenshots (scripts/screenshots.sh): the user's account is not the demo's.
        if std::env::var_os(NO_USAGE).is_none() {
            board.usage = usage(state, s, now);
        }
        // The contacts first, as in the tabs.
        let members = s.members.iter().filter(|m| m.contact).chain(s.members.iter().filter(|m| !m.contact));
        for member in members {
            let Some(session) = sessions.members.get(&member.name) else {
                board.absent.push(member.name.clone());
                continue;
            };
            if let Some(id) = &session.session_id {
                if let Some((old, _)) = self.sessions.get(id).filter(|(old, _)| *old != member.name) {
                    let old = old.clone();
                    if let Some(since) = self.since.remove(&old) {
                        self.since.insert(member.name.clone(), since);
                    }
                    if let Some(activity) = self.activity.remove(&old) {
                        self.activity.insert(member.name.clone(), activity);
                    }
                }
                self.sessions.insert(id.clone(), (member.name.clone(), seen));
            }
            let st = State::of(session.status.as_deref().unwrap_or_default());
            let entry = self.since.entry(member.name.clone()).or_insert((st, seen));
            if entry.0 != st {
                *entry = (st, seen);
            }
            let samples = self.activity.entry(member.name.clone()).or_default();
            samples.push_back((now, st == State::Working));
            while samples.front().is_some_and(|(at, _)| *at <= now - HISTORY) {
                samples.pop_front();
            }
            let report = bridge::report(state, &member.name).unwrap_or_default();
            let (model, effort) = bridge::model_and_effort(state, member);
            let context = report.context.as_ref();
            let helpers = helpers_shown(&report, now);
            board.cards.push(Card {
                name: member.name.clone(),
                color: member_color(s, &member.name),
                contact: member.contact,
                state: st,
                since: entry.1.elapsed().as_secs(),
                context: context.and_then(|c| c.percent),
                pressure: Pressure::of(context.and_then(|c| c.tokens), report.compacts_at()),
                model: model.map(|m| family(&m)),
                effort: effort.filter(|e| bridge::EFFORTS.contains(&e.as_str())),
                doing: bridge::doing(state, &member.name),
                samples: samples.iter().copied().collect(),
                helpers,
            });
        }
        // Those no longer in the team, forgotten; but for a while those whose session may come back under another name.
        self.sessions.retain(|_, (_, at)| at.elapsed() < KEEP);
        let kept = |name: &String| {
            s.members.iter().any(|m| &m.name == name) || self.sessions.values().any(|(last, _)| last == name)
        };
        let gone: Vec<String> = self.since.keys().chain(self.activity.keys()).filter(|n| !kept(n)).cloned().collect();
        for name in gone {
            self.since.remove(&name);
            self.activity.remove(&name);
        }
        // The working agents at rest, the latest first, by the very moment: two in the same second keep their order
        // from one second to the next, and so does the dashboard (see `ordered`).
        board.cards.sort_by_key(|c| {
            (!c.contact && resting(c)).then(|| std::cmp::Reverse(self.since.get(&c.name).map(|(_, at)| *at)))
        });
        board
    }
}

/// What the pane shows, so that a frame writes only the lines that changed: with the spinners turning ten times a
/// second, rewriting the whole board each time would keep tmux busy for nothing.
#[derive(Default)]
struct Screen {
    size: (u16, u16),
    lines: Vec<String>,
    zones: Vec<Zone>,
}

impl Screen {
    /// The board over the pane, each changed line written over the old one rather than cleared, so that nothing
    /// flickers.
    fn draw(&mut self, out: &mut impl Write, board: &Board) -> Result<()> {
        let size = terminal::size().unwrap_or((80, 24));
        if size != self.size {
            *self = Screen { size, ..Default::default() };
        }
        let Drawn { lines, zones } = render(board, size.0 as usize, size.1 as usize);
        // All the changes in one write.
        let mut changes = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            if self.lines.get(row) != Some(line) {
                queue!(changes, cursor::MoveTo(0, row as u16))?;
                write!(changes, "{line}")?;
                queue!(changes, terminal::Clear(ClearType::UntilNewLine))?;
            }
        }
        if !changes.is_empty() {
            // Below the board, what was typed in the pane, if anything.
            queue!(changes, cursor::MoveTo(0, lines.len() as u16), terminal::Clear(ClearType::FromCursorDown))?;
            out.write_all(&changes)?;
            out.flush()?;
        }
        self.lines = lines;
        self.zones = zones;
        Ok(())
    }
}

/// The time before the clock's next second, and a little more so that it has moved on.
fn next_second() -> Duration {
    let into = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_millis());
    Duration::from_millis(u64::from(1000 - into) + 5)
}

/// Set, the dashboard leaves the account's usage out (screenshots). Not documented.
const NO_USAGE: &str = "RECRUIT_NO_USAGE";

/// The account's 5-hour and 7-day windows, from the members' reports.
fn usage(state: &Path, s: &Snapshot, now: i64) -> Vec<(String, f64, Option<u64>)> {
    let reports: Vec<Report> = s.members.iter().filter_map(|m| bridge::report(state, &m.name)).collect();
    windows(&reports, now)
}

/// Each window as the reports tell it: label, percent used, seconds before it resets. A report keeps what its session
/// last heard, old news for a member at rest however recent the report: per window, the one that resets last (a new
/// window resets later than the one before), and of those the highest use (it only goes up in a window). Without a
/// reset time, a report counts only when no other tells one.
fn windows(reports: &[Report], now: i64) -> Vec<(String, f64, Option<u64>)> {
    [("five_hour", t!("5 h", "5 h")), ("seven_day", t!("7 j", "7 d"))]
        .into_iter()
        .filter_map(|(kind, label)| {
            let (resets, percent) = reports
                .iter()
                .flat_map(|r| &r.rate_limits)
                .filter(|l| l.kind == kind)
                .map(|l| (l.resets_at.as_deref().and_then(iso_epoch), l.percent_used))
                .max_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))?;
            match resets {
                // Over, and no session has heard of the next one yet: nothing used in it so far.
                Some(at) if at <= now => Some((label, 0.0, None)),
                _ => Some((label, percent, resets.map(|at| (at - now) as u64))),
            }
        })
        .collect()
}

/// The dashboard's lines for a pane of `width` × `height`: the header, the cards right under it, each in the form
/// the room leaves it (see [`arrange`]), the account's usage at the bottom.
fn render(b: &Board, width: usize, height: usize) -> Drawn {
    let mut top = vec![header(b, width), String::new()];
    if let Some(error) = &b.error {
        top.insert(1, fit(error, width).red().to_string());
    }
    let mut bottom = Vec::new();
    if !b.usage.is_empty() {
        bottom.extend([String::new(), usage_line(&b.usage, width)]);
    }
    let absent = names(&t!("absent", "absent"), &b.absent, width, Color::DarkGrey, usize::MAX).spaced();
    let room = height.saturating_sub(1 + top.len() + bottom.len() + absent.len());
    let layout = arrange(&ordered(&b.cards), width, room);

    let mut middle = cards(&layout.rows, width, b);
    if layout.more > 0 {
        middle.lines.push(t!("… et {} de plus", "… and {} more", layout.more).dim().to_string());
    }
    middle.append(resting_names(&layout.named, width, layout.whole));
    middle.append(absent);
    // Sessions open elsewhere under a member's name, over the usage: in the room the cards leave, if any.
    if let Some(line) = elsewhere_line(&b.elsewhere, width) {
        let mut with = if bottom.is_empty() { vec![String::new()] } else { bottom.clone() };
        with.insert(1, line);
        if 1 + top.len() + middle.len() + with.len() <= height {
            bottom = with;
        }
    }

    let filler = height.saturating_sub(1 + top.len() + middle.len() + bottom.len());
    let mut drawn = Drawn::from(top);
    drawn.append(middle);
    drawn.append(Drawn::from(vec![String::new(); filler]));
    drawn.append(Drawn::from(bottom));
    // A pane too low for all that: the header, and the usage at the bottom if it holds.
    let rows = height.saturating_sub(1);
    if drawn.len() > rows {
        let mut low = vec![header(b, width)];
        if !b.usage.is_empty() && rows >= 2 {
            low.extend(vec![String::new(); rows - 2]);
            low.push(usage_line(&b.usage, width));
        }
        low.truncate(rows);
        drawn = Drawn::from(low);
    }
    drawn
}

/// Lines of the dashboard, and where the members show in them, rows counted from the first line.
#[derive(Debug, Default)]
struct Drawn {
    lines: Vec<String>,
    zones: Vec<Zone>,
}

impl From<Vec<String>> for Drawn {
    fn from(lines: Vec<String>) -> Self {
        Drawn { lines, zones: Vec::new() }
    }
}

impl Drawn {
    fn len(&self) -> usize {
        self.lines.len()
    }

    /// `below` under these lines.
    fn append(&mut self, below: Drawn) {
        let rows = self.lines.len();
        self.zones.extend(below.zones.into_iter().map(|zone| Zone { row: zone.row + rows, ..zone }));
        self.lines.extend(below.lines);
    }

    /// After an empty line, unless there is nothing.
    fn spaced(self) -> Drawn {
        if self.lines.is_empty() {
            return self;
        }
        let mut spaced = Drawn::from(vec![String::new()]);
        spaced.append(self);
        spaced
    }
}

/// Where a member shows on the dashboard, its card or its name in a list: a click there leads to its pane; or the
/// context of a member at rest, which a click offers to compact. Rows and columns of the pane, from 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Zone {
    member: String,
    #[serde(default)]
    kind: ZoneKind,
    row: usize,
    col: usize,
    rows: usize,
    cols: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ZoneKind {
    #[default]
    Member,
    Compact,
}

impl Zone {
    fn contains(&self, x: usize, y: usize) -> bool {
        (self.col..self.col + self.cols).contains(&x) && (self.row..self.row + self.rows).contains(&y)
    }
}

/// The zones as the dashboard last drew them, in the team's folder.
pub const ZONES: &str = "zones.json";

/// Written in one go: a click never reads half a file.
fn save_zones(state: &Path, zones: &[Zone]) -> Result<()> {
    write_atomic(&state.join(ZONES), &serde_json::to_string(zones)?)
}

fn zones(state: &Path) -> Vec<Zone> {
    fs::read(state.join(ZONES)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// The members' states as the dashboard last saw them, in the team's folder: for the menu, which shows them at once
/// without asking `claude agents`.
pub const STATES: &str = "states.json";

/// What `STATES` holds: written at each look, every second, even when nothing changed; older than a few seconds, the
/// dashboard no longer looks (stopped, or `dashboard = false`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct States {
    /// When the dashboard looked, in seconds since the epoch.
    pub at: i64,
    /// The members with a session open in the team's folder.
    pub members: BTreeMap<String, MemberState>,
    /// Those without.
    pub absent: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MemberState {
    #[serde(with = "state_name")]
    pub state: State,
    /// Since when, in seconds since the epoch, as long as the dashboard has seen it so; for a member renamed on its
    /// conversation, since before its new name.
    pub since: i64,
}

/// A state by its name in `STATES`: `working`, `idle`, `waiting` or `other`.
mod state_name {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::look::State;

    pub fn serialize<S: Serializer>(state: &State, to: S) -> Result<S::Ok, S::Error> {
        to.serialize_str(match state {
            State::Working => "working",
            State::Idle => "idle",
            State::Waiting => "waiting",
            State::Other => "other",
        })
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<State, D::Error> {
        Ok(match String::deserialize(from)?.as_str() {
            "working" => State::Working,
            "idle" => State::Idle,
            "waiting" => State::Waiting,
            _ => State::Other,
        })
    }
}

/// What the dashboard last wrote; none if it never did, or while the file cannot be read.
pub fn read_states(state: &Path) -> Option<States> {
    fs::read(state.join(STATES)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// Written in one go: the menu never reads half a file.
fn save_states(state: &Path, states: &States) -> Result<()> {
    write_atomic(&state.join(STATES), &serde_json::to_string(states)?)
}

/// The keys the header shows: the menu, the journal's size, quitting.
fn keys() -> String {
    t!("{ALT}r menu · {ALT}j journal · {ALT}q quitter", "{ALT}r menu · {ALT}j journal · {ALT}q quit")
}

/// ` 14:32:05  ⠹ 2  ⚑ 1  ◷ 3    ⌥r menu · ⌥j journal · ⌥q quitter`: the time, how many work, wait for the user and
/// rest, the keys; tmux's status line names the team. Too narrow, the time goes first, then the keys; the counts stay.
fn header(b: &Board, width: usize) -> String {
    let count = |state: State| b.cards.iter().filter(|c| c.state == state).count();
    let counts: Vec<(String, Color)> = [State::Working, State::Waiting, State::Idle]
        .into_iter()
        .filter(|state| count(*state) > 0)
        .map(|state| (format!("{} {}", state.badge(b.glyphs, b.frame), count(state)), state.color()))
        .collect();
    let counted: usize =
        counts.iter().map(|(text, _)| text.chars().count()).sum::<usize>() + 2 * counts.len().saturating_sub(1);
    let time = clock(b.now);
    let keys = keys();
    let mut used = 1 + counted;
    let with_time = time.chars().count() + if counts.is_empty() { 0 } else { 2 };
    let with_keys = 2 + keys.chars().count();
    let (show_time, show_keys) =
        if used + with_time + with_keys <= width { (true, true) } else { (false, used + with_keys <= width) };
    let mut parts = Vec::new();
    if show_time {
        parts.push((time, Paint::Dim));
        used += with_time;
    }
    parts.extend(counts.into_iter().map(|(text, color)| (text, Paint::Color(color))));
    let mut pieces = vec![(" ".to_string(), Paint::Plain)];
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            pieces.push(("  ".into(), Paint::Plain));
        }
        pieces.push(part);
    }
    if show_keys {
        pieces.push((" ".repeat(width - used - keys.chars().count()), Paint::Plain));
        pieces.push((keys, Paint::Dim));
    }
    // Narrower than the counts alone, cut rather than wrapped.
    painted(&pieces, width)
}

/// How a piece of a line is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Paint {
    Plain,
    Dim,
    Color(Color),
    Bold(Color),
    /// Colored and dimmed.
    Faint(Color),
}

impl Paint {
    fn apply(self, text: &str) -> String {
        match self {
            Paint::Plain => text.to_string(),
            Paint::Dim => text.dim().to_string(),
            Paint::Color(color) => text.with(color).to_string(),
            Paint::Bold(color) => text.with(color).bold().to_string(),
            Paint::Faint(color) => text.with(color).dim().to_string(),
        }
    }
}

/// `pieces` one after the other, each in its paint, cut to `width` as `fit` cuts their text joined.
fn painted(pieces: &[(impl AsRef<str>, Paint)], width: usize) -> String {
    let joined: String = pieces.iter().map(|(text, _)| text.as_ref()).collect();
    let mut rest: Vec<char> = fit(&joined, width).chars().collect();
    let mut line = String::new();
    for (text, paint) in pieces {
        let taken: String = rest.drain(..text.as_ref().chars().count().min(rest.len())).collect();
        if !taken.is_empty() {
            line.push_str(&paint.apply(&taken));
        }
    }
    line
}

/// ` 5 h ▰▰▰▰▱▱▱▱ 41 % ↻ 4h27   7 j ▰▱▱▱▱▱▱▱ 5 % ↻ 6j11h`, the bars as long as the line allows. Too narrow, the reset
/// times go first, then the bars.
fn usage_line(usage: &[(String, f64, Option<u64>)], width: usize) -> String {
    const MIN_CELLS: usize = 5;
    const MAX_CELLS: usize = 20;
    // What follows each bar: the share used, and the time before the window resets.
    let tails = |resets: bool| -> Vec<(String, String)> {
        usage
            .iter()
            .map(|(_, percent, left)| {
                let left = left.filter(|_| resets).map(|secs| format!(" ↻ {}", remaining(secs)));
                (format!("{percent:.0} %"), left.unwrap_or_default())
            })
            .collect()
    };
    // The line but the bars, with a space after each.
    let windows = usage.len().max(1);
    let plain = |tails: &[(String, String)]| {
        let texts: usize = usage
            .iter()
            .zip(tails)
            .map(|((label, ..), (used, left))| 2 + label.chars().count() + used.chars().count() + left.chars().count())
            .sum();
        texts + windows + 2 * (windows - 1)
    };
    let (tails, cells) = [true, false]
        .into_iter()
        .map(|resets| {
            let tails = tails(resets);
            let cells = (width.saturating_sub(plain(&tails)) / windows).min(MAX_CELLS);
            (tails, cells)
        })
        .find(|(_, cells)| *cells >= MIN_CELLS)
        .unwrap_or_else(|| (tails(false), 0));
    usage
        .iter()
        .zip(tails)
        .map(|((label, percent, _), (used, left))| {
            let bar = if cells == 0 {
                String::new()
            } else {
                let filled = ((percent / 100.0 * cells as f64).round() as usize).min(cells);
                let color = match percent {
                    p if *p >= 90.0 => Color::Red,
                    p if *p >= 70.0 => Color::Yellow,
                    _ => Color::Green,
                };
                format!("{}{} ", "▰".repeat(filled).with(color), "▱".repeat(cells - filled).dark_grey())
            };
            format!(" {label} {bar}{used}{}", left.dim())
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// `au repos  a · b · c`, wrapped under its label over at most `max` lines, each name a zone; the names left over
/// counted at the end, `… et 4 autres`.
fn names(label: &str, list: &[String], width: usize, color: Color, max: usize) -> Drawn {
    if list.is_empty() || max == 0 {
        return Drawn::default();
    }
    const INDENT: usize = 10;
    // Each line's names: the member, as shown, from which column.
    let mut rows: Vec<Vec<(&String, String, usize)>> = vec![Vec::new()];
    let mut used = INDENT;
    for name in list {
        // Alone on its line and still too wide: cut, the zone still names the member.
        let shown = fit(name, width.saturating_sub(INDENT).max(1));
        let len = shown.chars().count();
        if rows.last().is_some_and(|row| !row.is_empty()) && used + 3 + len > width {
            rows.push(Vec::new());
            used = INDENT;
        }
        let row = rows.last_mut().expect("one row at least");
        if !row.is_empty() {
            used += 3;
        }
        row.push((name, shown, used));
        used += len;
    }
    let mut left = 0;
    if rows.len() > max {
        left = rows[max..].iter().map(Vec::len).sum();
        rows.truncate(max);
        let last = rows.last_mut().expect("one row at least");
        // Room for the count after the last name kept.
        while let Some((_, shown, col)) = last.last() {
            if col + shown.chars().count() + 1 + others(left).chars().count() <= width {
                break;
            }
            last.pop();
            left += 1;
        }
    }
    let mut drawn = Drawn::default();
    for (i, row) in rows.iter().enumerate() {
        let head = if i == 0 { format!("{label:<INDENT$}") } else { " ".repeat(INDENT) };
        let shown: Vec<&str> = row.iter().map(|(_, shown, _)| shown.as_str()).collect();
        let mut line = format!("{}{}", head.dim(), shown.join(" · ").with(color));
        if left > 0 && i + 1 == rows.len() {
            let gap = if row.is_empty() { "" } else { " " };
            line.push_str(&format!("{gap}{}", others(left).dim()));
        }
        drawn.lines.push(line);
        drawn.zones.extend(row.iter().map(|(name, shown, col)| Zone {
            member: name.to_string(),
            kind: ZoneKind::Member,
            row: i,
            col: *col,
            rows: 1,
            cols: shown.chars().count(),
        }));
    }
    drawn
}

/// `… et 4 autres`: names left out of a list.
fn others(count: usize) -> String {
    if count == 1 { t!("… et 1 autre", "… and 1 more") } else { t!("… et {} autres", "… and {} more", count) }
}

/// ` nom en double ailleurs : coordinateur (shop)`, dimmed: sessions open elsewhere under a member's name, which
/// messages by name could reach.
fn elsewhere_line(list: &[(String, Option<String>)], width: usize) -> Option<String> {
    if list.is_empty() {
        return None;
    }
    let names: Vec<String> = list
        .iter()
        .map(|(name, folder)| match folder {
            Some(folder) => format!("{name} ({folder})"),
            None => name.clone(),
        })
        .collect();
    let names = names.join(", ");
    let text = if list.len() == 1 {
        t!(" nom en double ailleurs : {}", " name in use elsewhere: {}", names)
    } else {
        t!(" noms en double ailleurs : {}", " names in use elsewhere: {}", names)
    };
    Some(fit(&text, width).dim().to_string())
}

/// The working agents at rest that have no card: `au repos  a · b · c`, after an empty line; on one line, those that
/// hold and how many more, unless `whole`.
fn resting_names(list: &[&Card], width: usize, whole: bool) -> Drawn {
    let list: Vec<String> = list.iter().map(|c| c.name.clone()).collect();
    names(&t!("au repos", "idle"), &list, width, State::Idle.color(), if whole { usize::MAX } else { 1 }).spaced()
}

/// The columns cards go in, side by side: the right one a column wider on an odd width.
fn column_widths(width: usize, columns: usize) -> Vec<usize> {
    if columns == 1 { vec![width] } else { vec![width / 2, width - width / 2] }
}

/// The lines a card `width` wide takes: its title and its curve, and in between what the member does over at most
/// `lines` lines, then the agents its session started, as `helpers` says.
fn card_height(c: &Card, width: usize, lines: usize, helpers: Helpers) -> usize {
    2 + if lines == 0 { 0 } else { doing_lines(c, width, lines).len() } + helper_lines(c, helpers)
}

/// How the agents a member's session started show on its card: one line each, else counted on one line, else not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Helpers {
    Listed,
    Counted,
    Left,
}

/// The lines they take.
fn helper_lines(c: &Card, helpers: Helpers) -> usize {
    match helpers {
        _ if c.helpers.is_empty() => 0,
        Helpers::Listed => c.helpers.len(),
        Helpers::Counted => 1,
        Helpers::Left => 0,
    }
}

/// The dashboard's parts, in order: the contacts, the working agents at work or waiting for the user, those at rest.
fn section(c: &Card) -> usize {
    if c.contact {
        0
    } else if resting(c) {
        2
    } else {
        1
    }
}

/// The cards in the order the dashboard shows them: the contacts; the agents waiting for the user, then those at
/// work; then those at rest, the latest first. Otherwise, as they come (the file's order, and for members at rest
/// since the same second, the latest first: see [`Memory::board`]).
fn ordered(cards: &[Card]) -> Vec<&Card> {
    let mut list: Vec<&Card> = cards.iter().collect();
    list.sort_by_key(|c| match section(c) {
        1 => (1, u64::from(c.state != State::Waiting)),
        2 => (2, c.since),
        s => (s, 0),
    });
    list
}

/// How a card is drawn: across the pane or beside another, with what the member does over at most so many lines; or,
/// for a working agent at rest, only its name in a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Card { half: bool, lines: usize, helpers: Helpers },
    Named,
}

/// A card's forms as room runs out: with the agents its session started, one a line, then counted, then without them,
/// what the member does over two lines all along (it never gives it up for them); then over one line; then beside
/// another, when two hold side by side; then without what it does; a working agent at rest ends in the list of names.
fn forms(c: &Card, two: bool) -> Vec<Form> {
    let card = |half, lines, helpers| Form::Card { half, lines, helpers };
    let mut forms = Vec::new();
    if !c.helpers.is_empty() {
        forms.extend([card(false, 2, Helpers::Listed), card(false, 2, Helpers::Counted)]);
    }
    forms.extend([card(false, 2, Helpers::Left), card(false, 1, Helpers::Left)]);
    if two {
        forms.extend([card(true, 1, Helpers::Left), card(true, 0, Helpers::Left)]);
    } else {
        forms.push(card(false, 0, Helpers::Left));
    }
    if !c.contact && resting(c) {
        forms.push(Form::Named);
    }
    forms
}

/// Where the cards go: rows of one card across the pane or two side by side, each card with the lines it gives what
/// the member does; the working agents at rest only named, all of them or on one line; how many cards found no room
/// at all.
#[derive(Debug, Default)]
struct Layout<'a> {
    rows: Vec<Vec<(&'a Card, usize, Helpers)>>,
    named: Vec<&'a Card>,
    whole: bool,
    more: usize,
}

/// How the working agents at rest without a card show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Names {
    /// All of them, over as many lines as it takes.
    Whole,
    /// Those that hold on one line, and how many more.
    Line,
    /// Only counted, with the cards that found no room.
    Counted,
}

/// A card of the list by its place in it, with the lines it gives what the member does and how the agents its session
/// started show.
type Placed = (usize, usize, Helpers);

/// The cards of `list`, in its order, over `room` lines of a pane `width` wide, each in its own form. `list` goes from
/// what matters most to what matters least: the contacts, the agents waiting for the user, at work, at rest from the
/// latest. In that order, each card takes the richest form that still leaves room for all those after it in their
/// barest one: across the pane with what the member does over two lines, then one; then beside the next card when two
/// hold side by side, then without what it does; a working agent at rest, at last, only named. Those named go on one
/// line while the cards choose; all of them if they hold once the cards have. A card with nothing to say of what the
/// member does only goes across the pane after that, with the room left. Even all in their barest form too many: the
/// first cards that hold, the agents at rest only counted with the others while the cards choose, then named if they
/// still hold. The agents a member's session started, under its card, only get the room left then: listed, else
/// counted; they never take what the members do.
///
/// The same cards always get the same layout: it changes only with them, their states, the agents their sessions
/// started or the pane.
fn arrange<'a>(list: &[&'a Card], width: usize, room: usize) -> Layout<'a> {
    let two = width >= 2 * MIN_CARD;
    let ladders: Vec<Vec<Form>> = list.iter().map(|c| forms(c, two)).collect();
    // Each card's height across the pane and in the narrower column, for each number of lines, measured once.
    let half = column_widths(width, 2)[0];
    let heights: Vec<[[usize; 3]; 2]> = list
        .iter()
        .map(|c| [width, half].map(|w| [0, 1, 2].map(|lines| card_height(c, w, lines, Helpers::Left))))
        .collect();
    let tall = |i: usize, half: bool, lines: usize, helpers: Helpers| {
        heights[i][usize::from(half)][lines] + helper_lines(list[i], helpers)
    };

    // The layout for each card's form, the first `shown` cards only; and the lines it takes.
    let build = |levels: &[usize], shown: usize, names: Names| -> (Layout<'a>, usize) {
        let mut layout = Layout { whole: names == Names::Whole, ..Default::default() };
        let mut rows: Vec<(Vec<Placed>, usize)> = Vec::new();
        let mut placed = 0;
        // Cards side by side, two by two in their order; one left over goes across.
        let mut run: Vec<Placed> = Vec::new();
        let pair = |run: &mut Vec<Placed>, rows: &mut Vec<(Vec<Placed>, usize)>| {
            for two in run.chunks(2) {
                let height = match two {
                    [(i, lines, helpers)] => tall(*i, false, *lines, *helpers),
                    _ => two.iter().map(|&(i, lines, helpers)| tall(i, true, lines, helpers)).max().unwrap_or(0),
                };
                rows.push((two.to_vec(), height));
            }
            run.clear();
        };
        for (i, c) in list.iter().enumerate() {
            if run.first().is_some_and(|&(j, ..)| section(list[j]) != section(c)) {
                pair(&mut run, &mut rows);
            }
            match ladders[i][levels[i]] {
                Form::Named if names == Names::Counted => layout.more += 1,
                Form::Named => layout.named.push(c),
                Form::Card { .. } if placed == shown => layout.more += 1,
                Form::Card { half, lines, helpers } => {
                    placed += 1;
                    if half {
                        run.push((i, lines, helpers));
                    } else {
                        pair(&mut run, &mut rows);
                        rows.push((vec![(i, lines, helpers)], tall(i, false, lines, helpers)));
                    }
                }
            }
        }
        pair(&mut run, &mut rows);
        let mut height = rows.iter().map(|(_, h)| h + 1).sum::<usize>().saturating_sub(1);
        height += usize::from(layout.more > 0) + resting_names(&layout.named, width, layout.whole).len();
        layout.rows = rows
            .into_iter()
            .map(|(row, _)| row.into_iter().map(|(i, lines, helpers)| (list[i], lines, helpers)).collect())
            .collect();
        (layout, height)
    };
    let fits = |levels: &[usize], shown: usize, names: Names| build(levels, shown, names).1 <= room;

    let barest: Vec<usize> = ladders.iter().map(|forms| forms.len() - 1).collect();
    let mut levels = barest.clone();
    let all = list.len();
    let cut = !fits(&levels, all, Names::Line);
    // Cut short, as many cards as hold, the names only counted with the others while the cards choose.
    let (shown, names) = if cut {
        match (0..all).rev().find(|&shown| fits(&levels, shown, Names::Counted)) {
            Some(shown) => (shown, Names::Counted),
            // Not even how many: nothing.
            None => return Layout::default(),
        }
    } else {
        (all, Names::Line)
    };
    // Each card in turn takes the richest form that holds, from `from`.
    let choose = |levels: &mut Vec<usize>, names: Names, from: &dyn Fn(usize) -> usize| {
        // Cut short, the agents at rest stay named: a card would go past the cards shown.
        for i in (0..all).filter(|&i| !cut || section(list[i]) != 2) {
            let was = levels[i];
            levels[i] = (from(i)..was)
                .find(|&form| {
                    levels[i] = form;
                    fits(levels, shown, names)
                })
                .unwrap_or(was);
        }
    };
    // What the members do first, without the agents their sessions started: those of a card would take the room of
    // what the next ones do. Across the pane, a card that says nothing more than beside another: it waits for the
    // names.
    let plain = |i: usize| list[i].doing.is_none();
    let beside = |i: usize| ladders[i].iter().position(|f| matches!(f, Form::Card { half: true, .. }));
    let bare =
        |i: usize| ladders[i].iter().position(|f| matches!(f, Form::Card { helpers: Helpers::Left, .. })).unwrap_or(0);
    choose(&mut levels, names, &|i| if plain(i) { beside(i).unwrap_or(0) } else { bare(i) });
    // Then the names: all of them if they hold, else on one line, else only counted.
    let names = [Names::Whole, Names::Line].into_iter().find(|&names| fits(&levels, shown, names)).unwrap_or(names);
    // The room left, to the agents the sessions started and to the cards that say nothing more across the pane.
    choose(&mut levels, names, &|_| 0);
    build(&levels, shown, names).0
}

/// Rows of cards, one across the pane or two side by side, an empty line between two rows, each as tall as its tallest
/// card; each card a zone, and the context of a member at rest another, to compact it. The model, the effort and the
/// time in columns from one card to the next of the same width.
fn cards(rows: &[Vec<(&Card, usize, Helpers)>], width: usize, b: &Board) -> Drawn {
    let placed: Vec<Vec<(&Card, usize, usize, Helpers)>> = rows
        .iter()
        .map(|row| {
            let widths = column_widths(width, row.len());
            row.iter().zip(widths).map(|(&(c, lines, helpers), w)| (c, w, lines, helpers)).collect()
        })
        .collect();
    let right = |n: usize| Right::of(placed.iter().filter(|row| row.len() == n).flatten().map(|&(c, w, ..)| (c, w)));
    let rights = [right(1), right(2)];
    let mut drawn = Drawn::default();
    for (i, row) in placed.iter().enumerate() {
        if i > 0 {
            drawn.lines.push(String::new());
        }
        let top = drawn.len();
        let right = &rights[row.len() - 1];
        let row: Vec<(&Card, usize, Vec<String>)> =
            row.iter().map(|&(c, w, lines, helpers)| (c, w, card(c, w, lines, helpers, right, b))).collect();
        let mut col = 0;
        for (c, w, card) in &row {
            let zone = |kind, row, col, rows, cols| Zone { member: c.name.clone(), kind, row, col, rows, cols };
            drawn.zones.push(zone(ZoneKind::Member, top, col, card.len(), *w));
            if let Some(cols) = compaction_cells(c) {
                drawn.zones.push(zone(ZoneKind::Compact, top + card.len() - 1, col + w - 1 - cols, 1, cols));
            }
            col += w;
        }
        let height = row.iter().map(|(.., card)| card.len()).max().unwrap_or(0);
        for i in 0..height {
            let mut line = String::new();
            for (j, (_, w, card)) in row.iter().enumerate() {
                match card.get(i) {
                    Some(text) => line.push_str(text),
                    // Under a shorter card, room for the one on its right.
                    None if j + 1 < row.len() => line.push_str(&" ".repeat(*w)),
                    None => {}
                }
            }
            drawn.lines.push(line);
        }
    }
    drawn
}

/// What the titles show on the right of the names, the same on every card of a width and in columns from one to the
/// next: the model, the effort's sign and level, the time. Each column as wide as its widest; the richest form that
/// every card holds: `Opus █ xhigh`, else `Opus █`, else `Opus`, else the time alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Right {
    model: usize,
    /// One for the sign alone, more for the sign and the level, none for neither.
    effort: usize,
    time: usize,
}

impl Right {
    /// For cards, each with its width.
    fn of<'a>(list: impl IntoIterator<Item = (&'a Card, usize)>) -> Self {
        let list: Vec<(&Card, usize)> = list.into_iter().collect();
        let widest =
            |each: &dyn Fn(&Card) -> Option<usize>| list.iter().filter_map(|(c, _)| each(c)).max().unwrap_or(0);
        let time = widest(&|c| Some(duration(c.since).chars().count()));
        let model = widest(&|c| c.model.as_ref().map(|m| m.chars().count()));
        let level = widest(&|c| c.effort.as_ref().map(|e| 2 + e.chars().count()));
        let bare = Right { model: 0, effort: 0, time };
        [level, level.min(1), 0]
            .into_iter()
            .map(|effort| Right { model, effort, time })
            .filter(|right| right.width() > 0)
            .find(|right| list.iter().all(|(c, width)| right.width() + 3 <= right.rest(c, *width)))
            .unwrap_or(bare)
    }

    fn width(&self) -> usize {
        self.model + if self.effort > 0 { 1 + self.effort } else { 0 }
    }

    /// What a card `width` wide leaves on its title past its sign, its name and the time.
    fn rest(&self, c: &Card, width: usize) -> usize {
        let room = width.saturating_sub(3);
        room.saturating_sub(2 + name_in(c, room, self.time).chars().count() + self.time)
    }
}

/// The member's name on a title of `room` columns, the time `time` wide.
fn name_in(c: &Card, room: usize, time: usize) -> String {
    fit(&c.name, room.saturating_sub(3 + time).max(1))
}

/// At rest, or in a state the dashboard does not know: the card fades.
fn resting(c: &Card) -> bool {
    !matches!(c.state, State::Working | State::Waiting)
}

/// The context's field, at the right of the curve: `⟳ 88 %` for a member at rest, which a click compacts.
const CONTEXT: usize = 7;

/// How many columns of a card's context a click compacts the member on: at rest, its context known.
fn compaction_cells(c: &Card) -> Option<usize> {
    let percent = c.context.filter(|_| c.state == State::Idle)?;
    Some(2 + format!("{percent:.0} %").chars().count())
}

/// A card, `width` wide: its title, what the member does over at most `lines` lines, the agents its session started
/// as `helpers` says, its curve and context. A capsule on the left in the state's color, on all its lines; the state
/// read by its intensity: yellow and bold at work, red waiting for the user, faded at rest.
///
/// ```text
/// ╻ ⠹ dev-mod            Opus █ xhigh   3m
/// ┃   Je relis la maquette des cartes
/// ┃   ⧗ Explore · Lire le tableau de bord  12s
/// ╹   ⣀⣀⣠⣤⣶⣿⣿⣶⣤⣀⣀                   74 %
/// ```
fn card(c: &Card, width: usize, lines: usize, helpers: Helpers, right: &Right, b: &Board) -> Vec<String> {
    let doing = if lines == 0 { Vec::new() } else { doing_lines(c, width, lines) };
    let helpers = helpers_drawn(c, helpers, width.saturating_sub(4), b);
    let height = 2 + doing.len() + helpers.len();
    let color = c.state.color();
    let line = if c.state == State::Waiting { Paint::Bold(color) } else { Paint::Color(color) };
    // From halfway down the first line to halfway down the last: two cards never touch.
    let bar = |row: usize| (look::stroke(row, height).to_string(), line);
    let faded = resting(c);
    let (sign, name, time) = match c.state {
        State::Working => (Paint::Color(color), Paint::Bold(c.color), Paint::Plain),
        State::Waiting => (Paint::Color(color), Paint::Bold(c.color), Paint::Color(color)),
        _ => (Paint::Dim, Paint::Faint(c.color), Paint::Dim),
    };

    let room = width.saturating_sub(3);
    let shown = name_in(c, room, right.time);
    let meta = if right.width() > 0 { right.width() + 2 } else { 0 };
    let gap = room.saturating_sub(2 + shown.chars().count() + meta + right.time);
    let mut title = vec![
        bar(0),
        (" ".into(), Paint::Plain),
        (c.state.icon(b.glyphs, b.frame).to_string(), sign),
        (" ".into(), Paint::Plain),
        (shown, name),
        (" ".repeat(gap), Paint::Plain),
    ];
    if meta > 0 {
        title.extend(model_and_effort(c, right, b.frame));
        title.push(("  ".into(), Paint::Plain));
    }
    title.push((format!("{:>w$}", duration(c.since), w = right.time), time));
    title.push((" ".into(), Paint::Plain));

    let mut out = vec![painted(&title, width)];
    let paint = if faded { Paint::Dim } else { Paint::Plain };
    for (i, text) in doing.into_iter().enumerate() {
        // As wide as the card, for the one on its right.
        let pad = " ".repeat(width.saturating_sub(4 + text.chars().count()));
        out.push(painted(&[bar(1 + i), ("   ".into(), Paint::Plain), (text, paint), (pad, Paint::Plain)], width));
    }
    let above = out.len();
    for (i, pieces) in helpers.into_iter().enumerate() {
        let mut line = vec![bar(above + i), ("   ".into(), Paint::Plain)];
        line.extend(pieces);
        out.push(painted(&line, width));
    }
    let mut bottom = vec![
        bar(height - 1),
        ("   ".into(), Paint::Plain),
        (curve(&c.samples, b.now, width.saturating_sub(6 + CONTEXT)), Paint::Color(color)),
        (" ".into(), Paint::Plain),
    ];
    bottom.extend(context(c));
    bottom.push((" ".into(), Paint::Plain));
    out.push(painted(&bottom, width));
    out
}

/// The agents a member's session started, as `helpers` says, each line `width` wide: its sign, what it is and the time.
/// A subagent under an hourglass, its type and what it was asked, the time since it started (until it ended): yellow
/// at work, faded with ✓ once done, red with ✗ once failed or stopped. A teammate under a person, its name, the time
/// in its status: yellow at work, faded waiting for a message or done, red once failed; faded under `?` when its
/// status stayed the same too long (see [`bridge::TEAMMATE_UNKNOWN`]). Counted, both kinds on one line, yellow when
/// one of them works.
fn helpers_drawn(c: &Card, helpers: Helpers, width: usize, b: &Board) -> Vec<Vec<(String, Paint)>> {
    let list = &c.helpers;
    if list.is_empty() {
        return Vec::new();
    }
    let at_work = Paint::Color(State::Working.color());
    let failed = Paint::Color(State::Waiting.color());
    match helpers {
        Helpers::Left => Vec::new(),
        Helpers::Counted => {
            let subagents = list.iter().filter(|h| h.teammate.is_none()).count();
            let teammates = list.len() - subagents;
            // Each kind there: its sign, how many, and what they are.
            let kinds: Vec<(char, usize, String)> = [
                (b.glyphs.hourglass(), subagents, t!("sous-agent{}", "subagent{}", plural(subagents))),
                (b.glyphs.person(), teammates, t!("teammate{}", "teammate{}", plural(teammates))),
            ]
            .into_iter()
            .filter(|(_, n, _)| *n > 0)
            .collect();
            // The richest that holds on the line, its last column left blank: `⧗ 12 sous-agents   ♙ 3 teammates`,
            // then `⧗ 12 · ♙ 3`, then the first kind alone, `⧗ 12`, then nothing.
            let words =
                kinds.iter().map(|(sign, n, what)| format!("{sign} {n} {what}")).collect::<Vec<_>>().join("   ");
            let counts: Vec<String> = kinds.iter().map(|(sign, n, _)| format!("{sign} {n}")).collect();
            let text = [words, counts.join(" · "), counts[0].clone()]
                .into_iter()
                .find(|text| text.chars().count() < width)
                .unwrap_or_default();
            let paint = if list.iter().any(|h| h.status == HelperStatus::Running && !h.unknown(b.now)) {
                at_work
            } else if list.iter().any(|h| h.status == HelperStatus::Failed) {
                failed
            } else {
                Paint::Dim
            };
            vec![vec![(text, paint)]]
        }
        Helpers::Listed => list
            .iter()
            .map(|h| {
                let (sign, label, secs) = match &h.teammate {
                    Some(name) => {
                        let sign = if h.unknown(b.now) { '?' } else { b.glyphs.person() };
                        (sign, name.clone(), b.now - h.since)
                    }
                    None => {
                        let sign = match h.status {
                            HelperStatus::Done => '✓',
                            HelperStatus::Failed => '✗',
                            _ => b.glyphs.hourglass(),
                        };
                        let label = match h.description.trim() {
                            "" => h.kind.clone(),
                            text => format!("{} · {text}", h.kind),
                        };
                        let until = if h.status == HelperStatus::Running { b.now } else { h.since };
                        (sign, label, until - h.started)
                    }
                };
                let paint = match h.status {
                    _ if h.unknown(b.now) => Paint::Dim,
                    HelperStatus::Running => at_work,
                    HelperStatus::Failed => failed,
                    HelperStatus::Idle | HelperStatus::Done => Paint::Dim,
                };
                let time = duration(secs.max(0) as u64);
                let label = fit(&label, width.saturating_sub(4 + time.chars().count()));
                let gap = width.saturating_sub(4 + label.chars().count() + time.chars().count());
                let plain = if paint == at_work { Paint::Plain } else { paint };
                vec![
                    (sign.to_string(), paint),
                    (" ".into(), Paint::Plain),
                    (label, plain),
                    (" ".repeat(gap + 1), Paint::Plain),
                    (time, plain),
                    (" ".into(), Paint::Plain),
                ]
            })
            .collect(),
    }
}

/// The plural's `s`, in either language.
fn plural(n: usize) -> &'static str {
    if n > 1 { "s" } else { "" }
}

/// What the member does, over at most `lines` lines of a card `width` wide: the task in progress; at rest, the task
/// done after ✓, the lines after the first under its text; in a state the dashboard does not know, the task as it
/// was. Nothing before the model has said.
fn doing_lines(c: &Card, width: usize, lines: usize) -> Vec<String> {
    let Some(task) = &c.doing else { return Vec::new() };
    let (mark, text) = match &task.done {
        Some(done) if c.state == State::Idle => ("✓ ", done),
        _ if c.state == State::Idle => ("✓ ", &task.now),
        _ => ("", &task.now),
    };
    let indent = mark.chars().count();
    wrap(text, width.saturating_sub(5 + indent), lines)
        .into_iter()
        .enumerate()
        .map(|(i, line)| if i == 0 { format!("{mark}{line}") } else { format!("{}{line}", " ".repeat(indent)) })
        .collect()
}

/// `text` over at most `lines` lines of `width` characters, cut between words, a word longer than a line where it
/// must; an ellipsis ends the last line when the text goes on. One line at least, empty for an empty text.
fn wrap(text: &str, width: usize, lines: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = line.chars().count();
            if used > 0 && used + 1 + word.len() <= width {
                line.push(' ');
                line.extend(&word);
                break;
            }
            if used > 0 {
                out.push(std::mem::take(&mut line));
                continue;
            }
            if word.len() <= width {
                line.extend(&word);
                break;
            }
            // Longer than a line: cut, the rest on the next.
            out.push(word.drain(..width).collect());
        }
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    let lines = lines.max(1);
    if out.len() > lines {
        let rest = out.split_off(lines - 1).join(" ");
        out.push(fit(&rest, width));
    }
    out
}

/// The model, faded, then the effort's sign and level in Claude Code's colors; each padded to its column.
fn model_and_effort(c: &Card, right: &Right, frame: usize) -> Vec<(String, Paint)> {
    let model = c.model.clone().unwrap_or_default();
    let mut pieces = vec![(format!("{model:<w$}", w = right.model), Paint::Dim)];
    if right.effort == 0 {
        return pieces;
    }
    pieces.push((" ".into(), Paint::Plain));
    let mut used = 0;
    if let Some(level) = &c.effort {
        let sign = effort_sign(level);
        let text = if right.effort == 1 { sign.to_string() } else { format!("{sign} {level}") };
        for (i, ch) in text.chars().enumerate() {
            // `max` in a rainbow, turning with the spinners.
            let color = effort_color(level).unwrap_or(RAINBOW[(i + frame / 2) % RAINBOW.len()]);
            pieces.push((ch.to_string(), Paint::Color(color)));
        }
        used = text.chars().count();
    }
    pieces.push((" ".repeat(right.effort.saturating_sub(used)), Paint::Plain));
    pieces
}

/// A context near compaction.
const ORANGE: Color = Color::AnsiValue(208);

/// The context's field, `CONTEXT` wide: the share of the window, orange near compaction, red where Claude Code
/// warns; `⟳` before it for a member at rest.
fn context(c: &Card) -> Vec<(String, Paint)> {
    let Some(percent) = c.context else { return vec![(" ".repeat(CONTEXT), Paint::Plain)] };
    let text = format!("{percent:.0} %");
    let paint = match c.pressure {
        Pressure::Warning => Paint::Bold(Color::Red),
        Pressure::Near => Paint::Color(ORANGE),
        Pressure::Calm if resting(c) => Paint::Dim,
        Pressure::Calm => Paint::Plain,
    };
    let compacts = compaction_cells(c);
    let mut pieces = vec![(" ".repeat(CONTEXT.saturating_sub(compacts.unwrap_or(text.chars().count()))), Paint::Plain)];
    if compacts.is_some() {
        pieces.push(("⟳ ".into(), Paint::Dim));
    }
    pieces.push((text, paint));
    pieces
}

/// The share of time spent working in each of `width` slices of the last half hour, oldest first, as braille
/// bars; blank where nothing was seen yet. The slices are cut on the clock, not counted back from `now`: one past
/// stays as it is, only the current one fills, and the curve moves one slice at a time.
fn curve(samples: &[(i64, bool)], now: i64, width: usize) -> String {
    let (mut seen, mut busy) = (vec![0u32; width], vec![0u32; width]);
    let slice = |at: i64| (at * width as i64).div_euclid(HISTORY);
    for &(at, working) in samples {
        let back = slice(now) - slice(at);
        if !(0..width as i64).contains(&back) {
            continue;
        }
        let i = width - 1 - back as usize;
        seen[i] += 1;
        busy[i] += u32::from(working);
    }
    (0..width)
        .map(|i| match seen[i] {
            0 => ' ',
            n => LEVELS[((busy[i] as f64 / n as f64) * 4.0).round() as usize],
        })
        .collect()
}

/// "42min", "4h27", "6j12h": the time before a window resets, short.
fn remaining(secs: u64) -> String {
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes}min"),
        (0, _) => format!("{hours}h{minutes:02}"),
        _ => t!("{}j{}h", "{}d{}h", days, hours),
    }
}

/// A message one member sent another.
#[derive(Debug, Clone, PartialEq)]
struct Message {
    at: i64,
    from: String,
    to: String,
    text: String,
}

/// The messages a line of `from`'s conversation sends.
fn messages(from: &str, line: &str) -> Vec<Message> {
    if !line.contains(r#""name":"SendMessage""#) {
        return Vec::new();
    }
    let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else { return Vec::new() };
    let at = entry.get("timestamp").and_then(|t| t.as_str()).and_then(iso_epoch).unwrap_or_else(now);
    let Some(blocks) = entry.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .filter(|b| b.get("name").and_then(|n| n.as_str()) == Some("SendMessage"))
        .filter_map(|b| b.get("input"))
        .map(|input| {
            let text = |key: &str| match input.get(key) {
                Some(serde_json::Value::String(s)) => s.trim().to_string(),
                Some(other) if !other.is_null() => other.to_string(),
                _ => String::new(),
            };
            let summary = text("summary");
            let body = if summary.is_empty() { text("message") } else { summary };
            Message {
                at,
                from: from.to_string(),
                to: printable(&text("to")),
                text: printable(body.lines().next().unwrap_or_default()),
            }
        })
        .collect()
}

/// A recipient by name: replies go to `uds:/tmp/cc-socks/<pid>.sock`, the sender's process; a name two sessions share
/// comes with the one meant, `coordinateur [ceeb11]`, which `claude agents --json` does not give: only the name is
/// kept.
fn recipient(to: &str, by_pid: &HashMap<u32, String>) -> String {
    let pid = to.strip_prefix("uds:").and_then(|path| Path::new(path).file_stem()?.to_str()?.parse::<u32>().ok());
    let named = || {
        let reference = to.strip_suffix(']').and_then(|rest| rest.rsplit_once(" ["));
        match reference {
            Some((name, id)) if !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit()) => name.to_string(),
            _ => to.to_string(),
        }
    };
    pid.and_then(|pid| by_pid.get(&pid).cloned()).unwrap_or_else(named)
}

/// What a conversation file gained since the last read, in whole lines.
#[derive(Default)]
struct Tail {
    offset: u64,
    rest: Vec<u8>,
}

impl Tail {
    fn read(&mut self, file: &Path) -> Vec<String> {
        let Ok(mut handle) = fs::File::open(file) else { return Vec::new() };
        let len = handle.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            *self = Tail::default();
        }
        let mut bytes = std::mem::take(&mut self.rest);
        if handle.seek(SeekFrom::Start(self.offset)).is_err() || handle.read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        self.offset = len.max(self.offset);
        let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        self.rest = bytes.split_off(complete);
        String::from_utf8_lossy(&bytes).lines().map(String::from).collect()
    }
}

/// Earlier messages shown when the journal opens.
const BACKLOG: usize = 200;

fn journal(mut s: Snapshot, state: &Path) -> Result<()> {
    let mut out = std::io::stdout();
    queue!(out, cursor::Hide)?;
    let width = terminal::size().map_or(80, |(w, _)| w as usize);
    let hint = t!("{ALT}j taille", "{ALT}j size");
    let title = " Journal ";
    let gap = width.saturating_sub(title.len() + hint.chars().count() + 1).max(2);
    writeln!(out, "{}{}{}\n", title.reverse().bold(), " ".repeat(gap), hint.dim())?;
    let mut tails: HashMap<PathBuf, Tail> = HashMap::new();
    let mut first = true;
    loop {
        reread(&mut s, state);
        let sessions = sessions(&s);
        let mut fresh = Vec::new();
        for (name, session) in &sessions.members {
            let Some(id) = &session.session_id else { continue };
            let file = claude::transcript(&s.dir, s.config_dir.as_deref(), id);
            for line in tails.entry(file.clone()).or_default().read(&file) {
                fresh.extend(messages(name, &line));
            }
        }
        fresh.sort_by_key(|m| m.at);
        if first {
            fresh.drain(..fresh.len().saturating_sub(BACKLOG));
            first = false;
        }
        let width = terminal::size().map_or(80, |(w, _)| w as usize);
        for m in fresh {
            let to = recipient(&m.to, &sessions.by_pid);
            let time = clock(m.at).chars().take(5).collect::<String>();
            writeln!(out, "{}", message_head(&s, &time, &m.from, &to, width))?;
            writeln!(out, "{}{}", "        ╰─ ".dark_grey(), fit(&m.text, width.saturating_sub(11)))?;
        }
        out.flush()?;
        std::thread::sleep(TICK);
    }
}

/// Between a message's sender and its recipient in the journal.
const ARROW: &str = "──▶";

/// A message's first line in the journal, ` 14:32  dev-cli ──▶ coordinateur`, cut to `width` rather than wrapped:
/// tmux gives a click only the line it lands on, and a name split over two would read as another one.
fn message_head(s: &Snapshot, time: &str, from: &str, to: &str, width: usize) -> String {
    let (sender, recipient) = (member_color(s, from), member_color(s, to));
    let pieces = [
        (" ", Paint::Plain),
        (time, Paint::Dim),
        ("  ", Paint::Plain),
        (from, Paint::Bold(sender)),
        (" ", Paint::Plain),
        (ARROW, Paint::Dim),
        (" ", Paint::Plain),
        (to, Paint::Color(recipient)),
    ];
    painted(&pieces, width)
}

/// The name under column `x` on a message's first line in the journal, ` 14:32  dev-cli ──▶ coordinateur`: its
/// sender or its recipient, nothing elsewhere nor on any other line. Columns counted in characters, as the journal
/// writes them. A recipient that reaches the last of the pane's `columns` may go on over the next line: tmux wraps
/// again the lines already written when the pane narrows, `dev-cli` then reads `dev`. It is no one's.
fn journal_name_at(line: &str, x: usize, columns: usize) -> Option<&str> {
    let (time, rest) = line.strip_prefix(' ')?.split_at_checked(5)?;
    let clock = time.bytes().enumerate().all(|(i, b)| if i == 2 { b == b':' } else { b.is_ascii_digit() });
    let (from, to) = rest.strip_prefix("  ").filter(|_| clock)?.split_once(&format!(" {ARROW} "))?;
    let width = |text: &str| text.chars().count();
    let from_at = 1 + 5 + 2;
    let to_at = from_at + width(from) + 1 + width(ARROW) + 1;
    let to = to.trim_end();
    let to = (to_at + width(to) < columns).then_some((to_at, to));
    [Some((from_at, from)), to]
        .into_iter()
        .flatten()
        .find(|(at, name)| (*at..at + width(name)).contains(&x))
        .map(|(_, name)| name)
}

/// The member under a click on a panel, at column `x` and row `y` of its pane from 0, the pane `columns` wide, `line`
/// the clicked line as tmux shows it: on the dashboard, a card or a name in a list, where the dashboard last drew
/// them; in the journal, the sender or the recipient on a message's first line, read from `line` alone (it may be
/// scrolled back).
pub fn member_at(
    s: &Snapshot,
    state: &Path,
    panel: Kind,
    x: usize,
    y: usize,
    columns: usize,
    line: &str,
) -> Option<String> {
    let name = match panel {
        Kind::Dashboard => {
            zones(state).into_iter().find(|zone| zone.kind == ZoneKind::Member && zone.contains(x, y))?.member
        }
        Kind::Journal => journal_name_at(line, x, columns)?.to_string(),
        Kind::Toggle => return None,
    };
    s.members.iter().any(|m| m.name == name).then_some(name)
}

/// The member at rest whose context a click on the dashboard falls on, at column `x` and row `y` of its pane: the
/// click offers to compact it. Asked before `member_at`, whose card holds the same place.
pub fn compaction_at(s: &Snapshot, state: &Path, x: usize, y: usize) -> Option<String> {
    let name = zones(state).into_iter().find(|zone| zone.kind == ZoneKind::Compact && zone.contains(x, y))?.member;
    s.members.iter().any(|m| m.name == name).then_some(name)
}

/// Takes the journal to its next size: full, reduced, hidden, full again.
pub fn toggle(s: &Snapshot) -> Result<tmux::JournalSize> {
    let tmux = Tmux::new(&TmuxSettings { socket: Some(s.socket.clone()), ..Default::default() })?;
    let journal = Pane {
        member: journal_title(s.lang).to_string(),
        role: Some(tmux::JOURNAL),
        argv: s.journal.clone(),
        env: Vec::new(),
    };
    tmux.toggle_journal(&s.session, &s.dir, &journal)
}

/// `text` cut to `width` characters, an ellipsis marking the cut.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    if width > 0 {
        cut.push('…');
    }
    cut
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Seconds since the epoch of an ISO 8601 time: `2026-10-07T10:28:51.123Z`, or with an offset (`+02:00`).
pub fn iso_epoch(text: &str) -> Option<i64> {
    let (date, time) = text.trim().split_once('T')?;
    let mut parts = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    let (time, offset) = match time.find(['Z', '+', '-']) {
        Some(at) if &time[at..] == "Z" => (&time[..at], 0),
        Some(at) => {
            let sign = if time[at..].starts_with('-') { -1 } else { 1 };
            let (h, m) = time[at + 1..].split_once(':')?;
            (&time[..at], sign * (h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60))
        }
        None => (time, 0),
    };
    let mut clock = time.splitn(3, ':');
    let hours: i64 = clock.next()?.parse().ok()?;
    let minutes: i64 = clock.next()?.parse().ok()?;
    let seconds: f64 = clock.next()?.parse().ok()?;
    Some(days_from_civil(year, month, day) * 86_400 + hours * 3600 + minutes * 60 + seconds as i64 - offset)
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `HH:MM:SS` in local time.
// libc marks musl's `time_t` deprecated over its size on 32-bit targets; it is 64 bits on the ones recruit ships.
#[allow(deprecated)]
fn clock(epoch: i64) -> String {
    let time = epoch as libc::time_t;
    // SAFETY: localtime_r only writes the `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times() {
        assert_eq!(iso_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_epoch("2026-10-07T10:28:51.123Z"), Some(1_791_368_931));
        assert_eq!(iso_epoch("2026-10-07T12:28:51+02:00"), Some(1_791_368_931));
        assert_eq!(iso_epoch("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(iso_epoch("hier"), None);
    }

    #[test]
    fn sent_messages() {
        let line = r#"{"type":"assistant","timestamp":"2026-10-07T10:28:51.123Z","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"SendMessage","input":{"to":"dev","summary":"API prête","message":"L'API est prête.\nDétails…"}},{"type":"tool_use","name":"SendMessage","input":{"to":"uds:/tmp/cc-socks/42.sock","message":"Bien reçu\nsuite"}}]}}"#;
        let found = messages("lead", line);
        assert_eq!(
            found[0],
            Message { at: 1_791_368_931, from: "lead".into(), to: "dev".into(), text: "API prête".into() }
        );
        assert_eq!(found[1].text, "Bien reçu");
        let by_pid = HashMap::from([(42, "coordinateur".to_string())]);
        assert_eq!(recipient(&found[1].to, &by_pid), "coordinateur");
        assert_eq!(recipient("dev", &by_pid), "dev");
        // The session meant, among those of one name: the name alone, the member's color and its click.
        assert_eq!(recipient("coordinateur [ceeb11]", &by_pid), "coordinateur");
        assert_eq!(recipient("coordinateur [x]", &by_pid), "coordinateur [x]");
        assert_eq!(recipient("[ceeb11]", &by_pid), "[ceeb11]");
        assert!(messages("lead", r#"{"type":"user","message":"SendMessage"}"#).is_empty());
        // Printed as it is, so no control character: no escape sequence, no clipboard write (OSC 52).
        let trapped = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "SendMessage",
            "input": {"to": "dev\u{1b}[31m", "message": "Fini\u{1b}]52;c;cm0gLXJmIH4=\u{7} \u{9b}2Jvoilà"}}]}});
        let found = messages("lead", &trapped.to_string());
        assert_eq!((found[0].to.as_str(), found[0].text.as_str()), ("dev[31m", "Fini]52;c;cm0gLXJmIH4= 2Jvoilà"));
        // Nor formatting: a text turned right to left, invisible characters.
        let turned = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "SendMessage",
            "input": {"to": "d\u{200b}ev", "message": "\u{202e}fdp.exe\u{2066} prêt"}}]}});
        let found = messages("lead", &turned.to_string());
        assert_eq!((found[0].to.as_str(), found[0].text.as_str()), ("dev", "fdp.exe prêt"));
    }

    #[test]
    fn tails_read_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.jsonl");
        fs::write(&file, "a\nb").unwrap();
        let mut tail = Tail::default();
        assert_eq!(tail.read(&file), ["a"]);
        fs::write(&file, "a\nbc\nd\n").unwrap();
        assert_eq!(tail.read(&file), ["bc", "d"]);
        assert!(tail.read(&file).is_empty());
        fs::write(&file, "x\n").unwrap();
        assert_eq!(tail.read(&file), ["x"]);
    }

    fn sample_card(name: &str, contact: bool, state: State) -> Card {
        Card {
            name: name.into(),
            color: Color::Cyan,
            contact,
            state,
            since: 90,
            context: Some(41.0),
            pressure: Pressure::Calm,
            model: Some("Opus".into()),
            effort: Some("xhigh".into()),
            doing: None,
            samples: (0..600).map(|i| (1000 - i, i % 3 == 0)).collect(),
            helpers: Vec::new(),
        }
    }

    /// What a line shows, escape sequences left out.
    fn visible(line: &str) -> String {
        let mut out = String::new();
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn board(cards: Vec<Card>) -> Board {
        Board {
            now: 1000,
            usage: vec![("5 h".into(), 41.0, Some(4920)), ("7 j".into(), 5.0, Some(560_000))],
            cards,
            absent: vec!["ops".into()],
            ..Default::default()
        }
    }

    /// The team of a capture: thirteen members, the contact and nine agents at rest (two who just finished, some
    /// without a task), two at work, one with a task over two lines.
    fn capture() -> Vec<Card> {
        let task = |now: &str, done: &str| Some(Task { now: now.into(), done: Some(done.into()) });
        let at = |name: &str, state, since, doing| Card { since, doing, ..sample_card(name, false, state) };
        let mut cards = vec![
            Card {
                doing: task("Je répartis les tâches", "Tâches réparties"),
                ..sample_card("coordinateur", true, State::Idle)
            },
            at("opérateur", State::Idle, 4000, None),
            at("ui-ux", State::Idle, 3000, task("Je relis les écrans", "Écrans relus")),
            at("designer", State::Idle, 20, task("Je livre la maquette", "Maquette des cartes livrée")),
            at(
                "gpu-engine",
                State::Working,
                120,
                task("Je profile le rendu des tuiles et je corrige la fuite du cache de textures", "Fuite corrigée"),
            ),
            at("performance", State::Idle, 2500, None),
            at("dev-back", State::Working, 300, task("Je migre l'API des cartes vers le nouveau schéma", "API migrée")),
            at("sécurité", State::Idle, 1800, task("J'audite les dépendances", "Dépendances auditées")),
            at("devops", State::Idle, 900, None),
            at("traitement-cartes", State::Idle, 1200, task("Je découpe les cartes", "Cartes découpées en tuiles")),
            at("planificateur", State::Idle, 5000, None),
            at("reviewer", State::Idle, 45, task("Je relis la PR 42", "Relecture de la PR 42 terminée")),
        ];
        cards.insert(1, at("opérateur-ui-ux", State::Idle, 3500, None));
        cards
    }

    /// The lines of a dashboard: no more than the pane holds, no card with an empty line, every zone over what it names.
    fn checked(b: &Board, width: usize, height: usize) -> Vec<String> {
        let drawn = render(b, width, height);
        zones_match(&drawn);
        let lines: Vec<String> = drawn.lines.iter().map(|l| visible(l)).collect();
        assert!(lines.len() == height - 1 && lines.iter().all(|l| l.chars().count() <= width), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.trim_end().ends_with('┃')), "{lines:#?}");
        lines
    }

    /// Where a member's card starts, if it has one.
    fn title(lines: &[String], name: &str) -> Option<usize> {
        lines.iter().position(|l| l.contains(&format!(" {name} ")) && l.contains('╻'))
    }

    /// What a member's card says it does, line by line, its capsule first.
    fn task(lines: &[String], name: &str) -> Vec<String> {
        let top = title(lines, name).unwrap();
        let col = lines[top].find(&format!(" {name} ")).unwrap();
        let col = lines[top][..col].chars().count() - 3;
        let column = |l: &String| l.chars().skip(col).collect::<String>();
        let middles = lines[top + 1..].iter().map(column).take_while(|l| l.starts_with('┃'));
        // The card on the right of a pair ends at its own width.
        middles.map(|l| l.split(" ┃ ").next().unwrap_or_default().trim_end().to_string()).collect()
    }

    /// The agents at rest only named, as they show.
    fn named(lines: &[String]) -> Vec<String> {
        let rest = t!("au repos", "idle");
        let from = lines.iter().position(|l| l.starts_with(&rest)).unwrap_or(lines.len());
        let list = lines[from..].iter().take_while(|l| !l.is_empty()).map(|l| l.trim_start_matches(&rest));
        let list = list.map(|l| l.split(" … ").next().unwrap_or_default().to_string());
        list.flat_map(|l| l.split(" · ").map(|n| n.trim().to_string()).collect::<Vec<_>>()).collect()
    }

    #[test]
    fn capture_at_several_heights() {
        let at = |height: usize| checked(&board(capture()), 60, height);
        let gpu = ["┃   Je profile le rendu des tuiles et je corrige la fuite", "┃   du cache de textures"];
        // Those who just finished: across the pane, what they did whole.
        let finished = |lines: &[String]| {
            assert_eq!(task(lines, "designer"), ["┃   ✓ Maquette des cartes livrée"], "{lines:#?}");
            assert_eq!(task(lines, "reviewer"), ["┃   ✓ Relecture de la PR 42 terminée"], "{lines:#?}");
        };

        // The contact, the two at work across the pane, their task over two lines, uncut; then those at rest, the
        // latest first; the three oldest only named.
        let lines = at(44);
        let order = ["coordinateur", "gpu-engine", "dev-back", "designer", "reviewer", "devops", "sécurité", "ui-ux"];
        let tops: Vec<usize> = order.iter().map(|name| title(&lines, name).unwrap()).collect();
        assert!(tops.windows(2).all(|w| w[0] <= w[1]) && tops[0] == 2, "{lines:#?}");
        assert_eq!(task(&lines, "gpu-engine"), gpu);
        finished(&lines);
        // Beside one another as they come, the same height or not.
        assert_eq!(title(&lines, "performance"), title(&lines, "ui-ux"));
        assert_eq!(task(&lines, "ui-ux"), ["┃   ✓ Écrans relus"]);
        assert_eq!(named(&lines), ["opérateur-ui-ux", "opérateur", "planificateur"]);

        // Less room: fewer cards at rest, those who just finished keep theirs whole.
        for height in [38, 34, 30] {
            let lines = at(height);
            assert_eq!(task(&lines, "gpu-engine"), gpu);
            finished(&lines);
        }
        // The next ones side by side, what they did cut rather than left out.
        let lines = at(34);
        assert_eq!(title(&lines, "devops"), title(&lines, "traitement-cartes"));
        assert_eq!(task(&lines, "traitement-cartes"), ["┃   ✓ Cartes découpées en tu…"]);
        assert_eq!(named(&lines).len(), 6);
        // Named on one line, the others counted, rather than a card less.
        let lines = at(30);
        // As many names as the line holds, in either language: the others make eight.
        let shown = named(&lines);
        assert_eq!(shown[..2], ["devops", "traitement-cartes"]);
        let line = format!("{:<10}{} {}", t!("au repos", "idle"), shown.join(" · "), others(8 - shown.len()));
        assert!(lines.contains(&line), "{lines:#?}");

        // Those who just finished side by side, without what they did, before the ones at work give anything up.
        let lines = at(24);
        assert_eq!(task(&lines, "gpu-engine"), gpu);
        let pair = &lines[title(&lines, "designer").unwrap()];
        assert!(pair.contains(" reviewer ") && task(&lines, "designer").is_empty(), "{lines:#?}");

        // Then the ones at work side by side, their task on one line; the contact still across the pane.
        let lines = at(18);
        let pair = &lines[title(&lines, "gpu-engine").unwrap()];
        assert!(pair.contains(" dev-back ") && pair.matches('╻').count() == 2, "{lines:#?}");
        assert_eq!(task(&lines, "coordinateur"), ["┃   ✓ Tâches réparties"]);

        // At last, the contact alone, what it did whole, and how many more.
        let lines = at(12);
        assert_eq!(task(&lines, "coordinateur"), ["┃   ✓ Tâches réparties"]);
        assert!(lines.contains(&t!("… et {} de plus", "… and {} more", 12)), "{lines:#?}");
        assert!(named(&lines).is_empty());
    }

    #[test]
    fn a_contact_across_rather_than_two_cut() {
        // Two contacts, 6 lines for them: the one at work across the pane, its task on one line; the other without.
        let cards = vec![
            Card {
                doing: Some(Task {
                    now: "Je relis la maquette des cartes et je corrige les colonnes de droite".into(),
                    done: None,
                }),
                ..sample_card("coordinateur", true, State::Working)
            },
            Card {
                doing: Some(Task { now: String::new(), done: Some("Plan relu".into()) }),
                ..sample_card("architecte", true, State::Idle)
            },
        ];
        let lines = checked(&Board { absent: Vec::new(), ..board(cards) }, 60, 11);
        assert!(lines.iter().all(|l| l.matches('╻').count() <= 1), "{lines:#?}");
        assert_eq!(task(&lines, "coordinateur"), ["┃   Je relis la maquette des cartes et je corrige les colo…"]);
        assert!(task(&lines, "architecte").is_empty());
        assert_eq!(lines[2..8].iter().filter(|l| l.is_empty()).count(), 1, "{lines:#?}");
    }

    #[test]
    fn too_low_a_pane() {
        for height in 0..=7 {
            let lines = render(&board(capture()), 60, height).lines;
            assert!(lines.len() <= height.saturating_sub(1), "{height}: {lines:#?}");
            assert!(!lines.iter().any(|l| visible(l).starts_with('…')), "{height}: {lines:#?}");
        }
        // The header, and the usage at the bottom.
        let lines: Vec<String> = render(&board(capture()), 60, 5).lines.iter().map(|l| visible(l)).collect();
        assert!(
            lines[0].contains(&format!("{}j", crate::tmux::ALT))
                && lines[3].contains("41 % ↻ 1h22")
                && lines.len() == 4,
            "{lines:#?}"
        );
        assert!(render(&board(capture()), 60, 5).zones.is_empty());
        // Room for how many more only.
        let lines: Vec<String> =
            render(&Board { absent: Vec::new(), ..board(capture()) }, 60, 6).lines.iter().map(|l| visible(l)).collect();
        assert_eq!(lines[2], t!("… et {} de plus", "… and {} more", 13));
    }

    #[test]
    fn names_on_one_line() {
        let list: Vec<String> =
            ["devops", "traitement-cartes", "sécurité", "performance", "ui-ux"].map(String::from).into();
        let drawn = names("au repos", &list, 50, Color::DarkGrey, 1);
        let lines: Vec<String> = drawn.lines.iter().map(|l| visible(l)).collect();
        assert_eq!(lines, [format!("au repos  devops · traitement-cartes {}", others(3))]);
        // The names shown are zones, the others are not.
        zones_match(&drawn);
        assert_eq!(drawn.zones.iter().map(|z| z.member.as_str()).collect::<Vec<_>>(), ["devops", "traitement-cartes"]);
        // One left over; none that holds.
        let lines = |width| {
            names("au repos", &list[..2], width, Color::DarkGrey, 1)
                .lines
                .iter()
                .map(|l| visible(l))
                .collect::<Vec<_>>()
        };
        assert_eq!(lines(30), [format!("au repos  devops {}", others(1))]);
        assert_eq!(lines(20), [format!("au repos  {}", others(2))]);
        // As many lines as it takes.
        assert_eq!(names("au repos", &list, 50, Color::DarkGrey, usize::MAX).len(), 2);
    }

    #[test]
    fn same_layout_from_one_second_to_the_next() {
        let later = |mut b: Board| {
            b.now += 1;
            b.cards.iter_mut().for_each(|c| c.since += 1);
            b
        };
        for height in [60, 44, 36, 30, 24, 18, 12] {
            let now = render(&board(capture()), 60, height);
            assert_eq!(now.zones, render(&later(board(capture())), 60, height).zones, "{height}");
        }
        // Two at rest since the same second: as they come, the latest first (see `Memory::board`).
        let cards = vec![
            Card { since: 30, ..sample_card("b", false, State::Idle) },
            Card { since: 30, ..sample_card("a", false, State::Idle) },
            Card { since: 10, ..sample_card("c", false, State::Idle) },
            sample_card("w", false, State::Working),
            sample_card("v", false, State::Waiting),
            sample_card("lead", true, State::Idle),
        ];
        let names: Vec<&str> = ordered(&cards).iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["lead", "v", "w", "c", "b", "a"]);
    }

    #[test]
    fn small_team_full_width() {
        let b = board(vec![
            sample_card("coordinateur", true, State::Working),
            sample_card("lanceur", false, State::Idle),
            sample_card("interface", false, State::Waiting),
        ]);
        let lines: Vec<String> = render(&b, 70, 23).lines.iter().map(|l| visible(l)).collect();
        assert_eq!(lines.len(), 22);
        assert!(lines.iter().all(|l| l.chars().count() <= 70), "{lines:#?}");
        let tops: Vec<&String> = lines.iter().filter(|l| l.starts_with('╻')).collect();
        assert_eq!(tops.len(), 3);
        assert!(tops.iter().all(|l| l.chars().count() == 70));
        // The cards right under the header, on two lines, an empty one between two: the contact, the agent waiting
        // for the user, the one at rest; the usage at the bottom.
        assert!(lines[2].starts_with("╻ ⠋ coordinateur ") && lines[2].ends_with(" Opus █ xhigh  1m "));
        assert!(lines[3].starts_with("╹   ") && lines[3].ends_with("   41 % "));
        assert!(lines[4].is_empty() && lines[5].starts_with("╻ ⚑ interface "));
        assert!(lines[7].is_empty() && lines[8].starts_with("╻ ◷ lanceur "));
        // At rest, the context offers to compact.
        assert!(lines[9].ends_with(" ⟳ 41 % "));
        assert!(lines[21].contains("41 % ↻ 1h22") && lines[20].is_empty());
        assert!(lines.iter().any(|l| l.contains("ops")));
    }

    #[test]
    fn what_they_do_on_each_card() {
        let doing = |name: &str, state, now: &str, done: Option<&str>| Card {
            doing: Some(Task { now: now.into(), done: done.map(String::from) }),
            ..sample_card(name, false, state)
        };
        let cards = vec![
            doing("coordinateur", State::Working, "Je relis la maquette des cartes", Some("Maquette relue")),
            doing("dev", State::Idle, "Je publie la version 0.6.2", Some("Version 0.6.2 publiée")),
            sample_card("ops", false, State::Waiting),
        ];
        let lines: Vec<String> = render(&board(cards.clone()), 50, 23).lines.iter().map(|l| visible(l)).collect();
        // The one waiting for the user first; nothing said yet, no line for it.
        assert!(lines[2].starts_with("╻ ⚑ ops ") && lines[3].starts_with("╹ "));
        assert!(lines[5].starts_with("╻ ⠋ coordinateur "));
        assert_eq!(lines[6].trim_end(), "┃   Je relis la maquette des cartes");
        assert!(lines[7].starts_with("╹   ") && lines[8].is_empty());
        // At rest, the last task, done.
        assert_eq!(lines[10].trim_end(), "┃   ✓ Version 0.6.2 publiée");
        assert!(!lines.iter().any(|l| l.trim_end() == "┃"), "{lines:#?}");
        // In a state the dashboard does not know: the task as it was, faded, nothing done.
        let unknown = doing("ops", State::Other, "Je publie la version 0.6.2", Some("Version 0.6.2 publiée"));
        let right = Right { model: 4, effort: 7, time: 2 };
        let line = card(&unknown, 50, 1, Helpers::Left, &right, &board(Vec::new()))[1].clone();
        assert_eq!(visible(&line).trim_end(), "┃   Je publie la version 0.6.2");
        assert!(line.contains(&"Je publie la version 0.6.2".dim().to_string()));
        // The model gave no form for it done: as it was.
        let undone = doing("review", State::Idle, "Je relis le clic", None);
        let right = Right { model: 4, effort: 7, time: 2 };
        assert_eq!(
            visible(&card(&undone, 50, 1, Helpers::Left, &right, &board(Vec::new()))[1]).trim_end(),
            "┃   ✓ Je relis le clic"
        );
        // Short of room, the agent at rest goes first, into the list of names; the one at work keeps its task.
        let lines: Vec<String> = render(&board(cards), 50, 15).lines.iter().map(|l| visible(l)).collect();
        assert!(lines.iter().any(|l| l.trim_end() == "┃   Je relis la maquette des cartes"), "{lines:#?}");
        assert_eq!(lines.iter().filter(|l| l.starts_with('╻')).count(), 2);
        assert!(lines.iter().any(|l| l.starts_with(&t!("au repos", "idle")) && l.ends_with(" dev")), "{lines:#?}");
    }

    #[test]
    fn wrapped_between_words() {
        let text = "Je publie la version 0.6.2 de recruit";
        assert_eq!(wrap(text, 20, 2), ["Je publie la version", "0.6.2 de recruit"]);
        assert_eq!(wrap(text, 40, 2), [text]);
        // On one line, cut.
        assert_eq!(wrap(text, 20, 1), ["Je publie la versio…"]);
        // Past two lines, the second ends with an ellipsis.
        assert_eq!(wrap("un deux trois quatre cinq six sept huit", 10, 2), ["un deux", "trois qua…"]);
        // A word longer than a line, cut where it must.
        assert_eq!(wrap("abcdefghij", 6, 2), ["abcdef", "ghij"]);
        assert_eq!(wrap("abcdefghijklmnop qr", 6, 2), ["abcdef", "ghijk…"]);
        assert_eq!(wrap("ok", 6, 2), ["ok"]);
        assert_eq!(wrap("", 6, 2), [""]);
        // Accents at the edge of a line: counted as characters, not bytes.
        assert_eq!(wrap("Équipe réglée près", 7, 2), ["Équipe", "réglée…"]);
        assert_eq!(wrap("é é é é", 3, 2), ["é é", "é é"]);
    }

    /// Two members: one whose task goes over two lines in one column, one at rest whose task holds in one.
    fn long_and_short() -> Vec<Card> {
        let task = |now: &str, done: Option<&str>| Some(Task { now: now.into(), done: done.map(String::from) });
        vec![
            Card {
                doing: task("Je relis la maquette des cartes et je corrige les colonnes de droite", None),
                ..sample_card("coordinateur", true, State::Working)
            },
            Card {
                doing: task("Je publie la version 0.6.2", Some("Version 0.6.2 publiée")),
                ..sample_card("dev", false, State::Idle)
            },
        ]
    }

    /// The contact at work over two lines, with what its session started: two subagents, one at work and one done,
    /// and a teammate waiting for a message; the agent at rest with its task done.
    fn with_helpers() -> Vec<Card> {
        let helper = |id: &str, kind: &str, description: &str, teammate: Option<&str>, status, started, since| Helper {
            id: id.into(),
            kind: kind.into(),
            description: description.into(),
            teammate: teammate.map(String::from),
            status,
            started,
            since,
        };
        let mut cards = long_and_short();
        cards[0].helpers = vec![
            helper("a", "Explore", "Lire le tableau de bord", None, HelperStatus::Running, 988, 988),
            helper("b", "Plan", "Découper la tâche", None, HelperStatus::Done, 900, 996),
            helper("m", "reviewer", "", Some("critic"), HelperStatus::Idle, 800, 940),
        ];
        cards
    }

    #[test]
    fn helpers_under_the_card() {
        let lines: Vec<String> = render(&board(with_helpers()), 70, 30).lines.iter().map(|l| visible(l)).collect();
        let top = title(&lines, "coordinateur").unwrap();
        let card: Vec<&str> = lines[top..top + 7].iter().map(|l| l.trim_end()).collect();
        // Under what it does, one line each, the time on the right as on the title: since the start, until the end;
        // in its status.
        assert!(card[1].starts_with("┃   Je relis") && card[2].starts_with("┃   droite"), "{lines:#?}");
        let helper = |line: &str, left: &str, time: &str| line.starts_with(left) && line.ends_with(time);
        assert!(helper(card[3], "┃   ⧗ Explore · Lire le tableau de bord ", " 12s"), "{lines:#?}");
        assert!(helper(card[4], "┃   ✓ Plan · Découper la tâche ", " 1m"), "{lines:#?}");
        assert!(helper(card[5], "┃   ♙ critic ", " 1m"), "{lines:#?}");
        assert_eq!(card[3].chars().count(), card[0].chars().count(), "{lines:#?}");
        assert!(card[6].starts_with('╹'), "{lines:#?}");
        // The card's zone holds them.
        let drawn = render(&board(with_helpers()), 70, 30);
        let zone = drawn.zones.iter().find(|z| z.member == "coordinateur" && z.kind == ZoneKind::Member).unwrap();
        assert_eq!((zone.row, zone.rows), (top, 7));
    }

    #[test]
    fn helpers_counted_then_left_before_what_they_do() {
        let at = |height: usize| -> Vec<String> {
            render(&board(with_helpers()), 70, height).lines.iter().map(|l| visible(l)).collect()
        };
        let middles = |lines: &[String]| {
            let top = title(lines, "coordinateur").unwrap();
            lines[top + 1..]
                .iter()
                .take_while(|l| l.starts_with('┃'))
                .map(|l| l.trim_end().to_string())
                .collect::<Vec<_>>()
        };
        let counted = format!(
            "┃   ⧗ {}   ♙ {}",
            t!("{} sous-agent{}", "{} subagent{}", 2, "s"),
            t!("{} teammate{}", "{} teammate{}", 1, "")
        );
        // As the pane gets lower: one line each, then counted, then left out; what it does on two lines all along.
        let mut seen = Vec::new();
        for height in (10..=30).rev() {
            let lines = at(height);
            let Some(middles) = title(&lines, "coordinateur").map(|_| middles(&lines)) else { break };
            let helpers = middles.len().saturating_sub(2);
            if helpers > 0 || seen.last() != Some(&0) {
                assert!(middles[0].starts_with("┃   Je relis") && middles[1].starts_with("┃   droite"), "{lines:#?}");
            }
            if helpers == 1 {
                assert_eq!(middles[2], counted, "{lines:#?}");
            }
            if seen.last() != Some(&helpers) {
                seen.push(helpers);
            }
        }
        assert_eq!(seen[..3], [3, 1, 0], "{seen:?}");
    }

    fn helper(id: &str, teammate: Option<&str>, status: HelperStatus) -> Helper {
        Helper {
            id: id.into(),
            kind: "Explore".into(),
            description: format!("Lire {id}"),
            teammate: teammate.map(String::from),
            status,
            started: 990,
            since: 990,
        }
    }

    /// Review's probe: the contact at work with six subagents, an agent at work on a task over two lines, one at rest.
    fn probe() -> Vec<Card> {
        let task = |now: &str| Some(Task { now: now.into(), done: None });
        let mut contact =
            Card { doing: task("Je répartis le travail"), ..sample_card("coordinateur", true, State::Working) };
        contact.helpers = (0..6).map(|i| helper(&format!("f{i}"), None, HelperStatus::Running)).collect();
        vec![
            contact,
            Card {
                doing: task("Je corrige la route POST /api/links pour rendre le slug déjà donné à une URL connue"),
                ..sample_card("backend", false, State::Working)
            },
            Card {
                doing: Some(Task { now: "x".into(), done: Some("Version publiée".into()) }),
                ..sample_card("dev", false, State::Idle)
            },
        ]
    }

    #[test]
    fn helpers_never_take_what_the_next_ones_do() {
        let doing = |lines: &[String], name: &str| -> Option<usize> {
            let top = title(lines, name)?;
            Some(lines[top + 1..].iter().take_while(|l| l.starts_with('┃')).filter(|l| !l.starts_with("┃   ⧗")).count())
        };
        let mut before: Option<(usize, Vec<Option<usize>>)> = None;
        for height in 6..=40 {
            let lines: Vec<String> = render(&board(probe()), 60, height).lines.iter().map(|l| visible(l)).collect();
            let now: Vec<Option<usize>> = ["coordinateur", "backend", "dev"].map(|n| doing(&lines, n)).into();
            // Never less of a task on a taller pane that shows the same cards (one more card, on a pane too low for all
            // of them, comes in its barest form).
            let same = |then: &[Option<usize>]| then.iter().zip(&now).all(|(a, b)| a.is_some() == b.is_some());
            if let Some((was, then)) = before.as_ref().filter(|(_, then)| same(then)) {
                for (i, (then, now)) in then.iter().zip(&now).enumerate() {
                    if let (Some(then), Some(now)) = (then, now) {
                        assert!(now >= then, "card {i}, {was} → {height} lines: {then} → {now}\n{lines:#?}");
                    }
                }
            }
            // The contact's subagents only once the agent at work, if it has a card, has its task whole. A pane too
            // low for all the cards in their barest form shows the first ones only, before any subagent.
            if lines.iter().any(|l| l.starts_with("┃   ⧗")) && now[1].is_some() {
                assert_eq!(now[1], Some(2), "{height}: {lines:#?}");
                assert!(!lines.iter().any(|l| l.starts_with('┃') && l.trim_end().ends_with('…')), "{lines:#?}");
            }
            before = Some((height, now));
        }
        // Tall enough, all of them.
        let lines: Vec<String> = render(&board(probe()), 60, 40).lines.iter().map(|l| visible(l)).collect();
        assert_eq!(lines.iter().filter(|l| l.starts_with("┃   ⧗")).count(), 6, "{lines:#?}");
    }

    #[test]
    fn helper_line_as_wide_as_its_card() {
        let mut cards = probe();
        cards[0].helpers = vec![Helper {
            description: "Lire tout le code du serveur et des tests pour trouver où les liens sont comptés".into(),
            ..helper("long", None, HelperStatus::Running)
        }];
        let lines: Vec<String> = render(&board(cards), 60, 40).lines.iter().map(|l| visible(l)).collect();
        let top = title(&lines, "coordinateur").unwrap();
        let line = lines.iter().find(|l| l.starts_with("┃   ⧗")).unwrap();
        // Cut before the time, which stays whole; as wide as the title.
        assert!(line.trim_end().ends_with("… 10s"), "{line:?}");
        assert_eq!(line.chars().count(), lines[top].chars().count(), "{lines:#?}");
    }

    #[test]
    fn helpers_counted_as_the_line_holds() {
        let mut card = sample_card("coordinateur", true, State::Working);
        card.helpers = (0..12).map(|i| helper(&format!("f{i}"), None, HelperStatus::Running)).collect();
        card.helpers.extend((0..3).map(|i| helper(&format!("m{i}"), Some("critic"), HelperStatus::Idle)));
        let b = board(Vec::new());
        let text = |card: &Card, width| -> String {
            helpers_drawn(card, Helpers::Counted, width, &b).concat().into_iter().map(|(text, _)| text).collect()
        };
        let words = format!("⧗ 12 {}   ♙ 3 {}", t!("sous-agents", "subagents"), t!("teammates", "teammates"));
        assert_eq!(text(&card, 60), words);
        assert_eq!(text(&card, 20), "⧗ 12 · ♙ 3");
        assert_eq!(text(&card, 8), "⧗ 12");
        assert_eq!(text(&card, 4), "");
        // Its color: yellow when one works, else red when one failed, else faded.
        let paint = |card: &Card| helpers_drawn(card, Helpers::Counted, 60, &b)[0][0].1;
        assert_eq!(paint(&card), Paint::Color(State::Working.color()));
        let mut failed = card.clone();
        failed.helpers = vec![helper("a", None, HelperStatus::Failed), helper("b", None, HelperStatus::Done)];
        assert_eq!(paint(&failed), Paint::Color(State::Waiting.color()));
        failed.helpers.remove(0);
        assert_eq!(paint(&failed), Paint::Dim);
    }

    #[test]
    fn helpers_only_while_the_mod_reports() {
        let report = |at| Report {
            at,
            helpers: vec![
                helper("a", None, HelperStatus::Running),
                Helper { since: 900, ..helper("b", None, HelperStatus::Done) },
            ],
            ..Default::default()
        };
        // Done long ago: no longer shown.
        assert_eq!(helpers_shown(&report(995), 1000).iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(helpers_shown(&report(1000 - FRESH_REPORT), 1000).len(), 1);
        // A report older than that: the member stopped, they went with it.
        assert!(helpers_shown(&report(1000 - FRESH_REPORT - 1), 1000).is_empty());
    }

    #[test]
    fn teammate_gone_quiet_shows_unknown() {
        let mut cards = with_helpers();
        cards[0].helpers.clear();
        cards[0].helpers.push(Helper {
            id: "m".into(),
            kind: "reviewer".into(),
            description: String::new(),
            teammate: Some("critic".into()),
            status: HelperStatus::Running,
            started: 0,
            since: 1000 - bridge::TEAMMATE_UNKNOWN,
        });
        let lines: Vec<String> = render(&board(cards), 70, 30).lines.iter().map(|l| visible(l)).collect();
        assert!(lines.iter().any(|l| l.starts_with("┃   ? critic")), "{lines:#?}");
    }

    #[test]
    fn capsule_over_four_lines() {
        let right = Right { model: 4, effort: 7, time: 2 };
        let lines: Vec<String> = card(&long_and_short()[0], 50, 2, Helpers::Left, &right, &board(Vec::new()))
            .iter()
            .map(|l| visible(l))
            .collect();
        let bars: Vec<char> = lines.iter().map(|l| l.chars().next().unwrap()).collect();
        assert_eq!(bars, ['╻', '┃', '┃', '╹']);
        assert_eq!(lines[1].trim_end(), "┃   Je relis la maquette des cartes et je corrige");
        assert_eq!(lines[2].trim_end(), "┃   les colonnes de droite");
        // At rest, the lines after the first under the text, past its ✓.
        let done = Card {
            doing: Some(Task {
                now: String::new(), done: Some("Version 0.6.2 publiée et poussée dans le tap".into())
            }),
            ..sample_card("dev", false, State::Idle)
        };
        let lines: Vec<String> =
            card(&done, 40, 2, Helpers::Left, &right, &board(Vec::new())).iter().map(|l| visible(l)).collect();
        assert_eq!(lines[1].trim_end(), "┃   ✓ Version 0.6.2 publiée et poussée");
        assert_eq!(lines[2].trim_end(), "┃     dans le tap");
    }

    #[test]
    fn forms_given_up_in_turn() {
        // The contact at work over two lines, the agent at rest with its task done: 15 lines leave them 8.
        let at = |height: usize| -> Vec<String> {
            render(&board(long_and_short()), 70, height).lines.iter().map(|l| visible(l)).collect()
        };
        let middles = |lines: &[String], name: &str| {
            let top = lines.iter().position(|l| l.contains(&format!(" {name} "))).unwrap();
            lines[top + 1..].iter().take_while(|l| l.starts_with('┃')).count()
        };
        let named =
            |lines: &[String]| lines.iter().any(|l| l.starts_with(&t!("au repos", "idle")) && l.contains("dev"));
        let cut = |lines: &[String]| lines.iter().any(|l| l.starts_with('┃') && l.trim_end().ends_with('…'));
        let lines = at(15);
        assert!(middles(&lines, "coordinateur") == 2 && middles(&lines, "dev") == 1 && !cut(&lines), "{lines:#?}");
        // The agent at rest first: without its task, then only named.
        let lines = at(14);
        assert!(middles(&lines, "coordinateur") == 2 && middles(&lines, "dev") == 0, "{lines:#?}");
        let lines = at(13);
        assert!(middles(&lines, "coordinateur") == 2 && named(&lines) && !cut(&lines), "{lines:#?}");
        // Then the contact: its task on one line, cut, then without it.
        let lines = at(12);
        assert!(middles(&lines, "coordinateur") == 1 && named(&lines) && cut(&lines), "{lines:#?}");
        let lines = at(11);
        assert!(middles(&lines, "coordinateur") == 0 && named(&lines), "{lines:#?}");
        // At last, rather than no card, no names: only how many more.
        let lines = at(10);
        assert!(middles(&lines, "coordinateur") == 0 && !named(&lines), "{lines:#?}");
        assert!(lines.iter().any(|l| l == &t!("… et {} de plus", "… and {} more", 1)), "{lines:#?}");
    }

    #[test]
    fn zones_over_cards_of_different_heights() {
        // One column: four lines, an empty one, three.
        let drawn = render(&board(long_and_short()), 70, 15);
        zones_match(&drawn);
        let rows = |drawn: &Drawn| -> Vec<(String, ZoneKind, usize, usize, usize)> {
            drawn.zones.iter().map(|z| (z.member.clone(), z.kind, z.row, z.col, z.rows)).collect()
        };
        assert_eq!(
            rows(&drawn)[..3],
            [
                ("coordinateur".into(), ZoneKind::Member, 2, 0, 4),
                ("dev".into(), ZoneKind::Member, 7, 0, 3),
                ("dev".into(), ZoneKind::Compact, 9, 63, 1),
            ]
        );
        // Side by side, two at work: each its own height, the shorter one's curve on its own last line.
        let pair = || {
            let task = Some(Task { now: "Je relis le clic".into(), done: None });
            vec![
                Card { doing: task, ..sample_card("relecteur", false, State::Working) },
                sample_card("dev", false, State::Working),
            ]
        };
        let drawn = render(&board(pair()), 70, 10);
        zones_match(&drawn);
        assert_eq!(
            rows(&drawn)[..2],
            [("relecteur".into(), ZoneKind::Member, 2, 0, 3), ("dev".into(), ZoneKind::Member, 2, 35, 2)]
        );
        // The shorter one on the left: room kept under it, the right one's capsule in its column on all its lines.
        let mut cards = pair();
        cards.reverse();
        let drawn = render(&board(cards), 70, 10);
        zones_match(&drawn);
        assert_eq!(rows(&drawn)[0], ("dev".into(), ZoneKind::Member, 2, 0, 2));
        assert_eq!(rows(&drawn)[1], ("relecteur".into(), ZoneKind::Member, 2, 35, 3));
        let capsule: Vec<Option<char>> = drawn.lines[2..5].iter().map(|l| visible(l).chars().nth(35)).collect();
        assert_eq!(capsule, [Some('╻'), Some('┃'), Some('╹')]);
        assert_eq!(visible(&drawn.lines[4]).chars().position(|c| c == '╹'), Some(35));
    }

    #[test]
    fn the_right_of_the_titles_in_columns() {
        let cards = vec![
            sample_card("coordinateur", true, State::Working),
            Card {
                model: Some("Sonnet".into()),
                effort: Some("low".into()),
                since: 4000,
                ..sample_card("dev", false, State::Idle)
            },
            Card { model: None, effort: None, ..sample_card("ops", false, State::Waiting) },
        ];
        let titles = |width: usize| -> Vec<String> {
            let lines = render(&board(cards.clone()), width, 23).lines;
            lines.iter().map(|l| visible(l)).filter(|l| l.starts_with('╻')).collect()
        };
        let ends = |titles: Vec<String>, ends: [&str; 3]| {
            assert!(
                titles
                    .iter()
                    .zip(ends)
                    .all(|(t, end)| t.ends_with(end) && t.chars().count() == titles[0].chars().count()),
                "{titles:#?}"
            );
        };
        // The richest form they all hold, each column as wide as its widest, the time on the right: coordinateur
        // holds the model and the effort's level from 38 columns on, its sign from 32, the model alone from 30.
        ends(titles(38), [" Opus   █ xhigh    1m ", &format!(" {}  1m ", " ".repeat(14)), " Sonnet ▂ low    1h06 "]);
        ends(titles(37), [" Opus   █    1m ", &format!(" {}  1m ", " ".repeat(8)), " Sonnet ▂  1h06 "]);
        ends(titles(31), [" Opus      1m ", "   1m ", " Sonnet  1h06 "]);
        let bare = titles(29);
        assert!(bare.iter().all(|t| !t.contains("Opus") && !t.contains("Sonnet") && t.chars().count() == 29));
        assert!(bare[2].ends_with(" 1h06 "));
    }

    #[test]
    fn model_faded_effort_in_its_colors() {
        let right = Right { model: 6, effort: 7, time: 2 };
        let title = card(&sample_card("dev", false, State::Working), 50, 0, Helpers::Left, &right, &board(Vec::new()))
            [0]
        .clone();
        let dim = "\x1b[2m";
        assert!(title.contains(&format!("{dim}Opus")));
        let violet = "█".with(Color::AnsiValue(141)).to_string();
        assert!(!title.contains(&format!("{dim}█")) && title.contains(&violet));
    }

    #[test]
    fn context_near_compaction() {
        assert_eq!(Pressure::of(Some(150_000), None), Pressure::Calm);
        assert_eq!(Pressure::of(None, Some(167_000)), Pressure::Calm);
        assert_eq!(Pressure::of(Some(133_000), Some(167_000)), Pressure::Calm);
        assert_eq!(Pressure::of(Some(133_600), Some(167_000)), Pressure::Near);
        assert_eq!(Pressure::of(Some(147_000), Some(167_000)), Pressure::Warning);
        assert_eq!(Pressure::of(Some(773_600), Some(967_000)), Pressure::Near);
        let near = Card { pressure: Pressure::Near, ..sample_card("dev", false, State::Idle) };
        let field: String = context(&near).iter().map(|(text, paint)| paint.apply(text)).collect();
        assert!(field.contains(&"41 %".with(ORANGE).to_string()));
        assert_eq!(visible(&field), " ⟳ 41 %");
        // At work, no compacting.
        assert_eq!(compaction_cells(&sample_card("dev", false, State::Working)), None);
    }

    #[test]
    fn big_team_two_columns_then_names() {
        let mut cards =
            vec![sample_card("coordinateur", true, State::Idle), sample_card("archi", true, State::Working)];
        cards.extend((0..4).map(|i| sample_card(&format!("w{i}"), false, State::Working)));
        cards.extend((0..9).map(|i| sample_card(&format!("repos{i}"), false, State::Idle)));
        let lines: Vec<String> = render(&board(cards.clone()), 70, 23).lines.iter().map(|l| visible(l)).collect();
        assert!(lines.iter().all(|l| l.chars().count() <= 70), "{lines:#?}");
        // Nothing to say of what they do: two side by side rather than a name less. A contact at rest keeps its card,
        // the latest agents at rest too; the others are only named.
        assert!(lines[2].contains("╻ ◷ coordinateur ") && lines[2].contains("╻ ⠋ archi "), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("╻ ◷ repos0 ") && l.contains("╻ ◷ repos1 ")));
        assert_eq!(named(&lines), ["repos4", "repos5", "repos6", "repos7", "repos8"]);

        // Twice the height: every one in cards, two side by side.
        let tall: Vec<String> = render(&board(cards.clone()), 70, 47).lines.iter().map(|l| visible(l)).collect();
        assert!(tall.iter().any(|l| l.contains("╻ ◷ repos0 ")));
        // Too narrow for two side by side.
        let narrow: Vec<String> = render(&board(cards), 50, 47).lines.iter().map(|l| visible(l)).collect();
        assert!(narrow.iter().all(|l| l.matches('╻').count() <= 1 && l.chars().count() <= 50));
    }

    /// Each zone over what it names: a whole card, in its capsule, with the member's name on top; the name itself; or
    /// the context to compact.
    fn zones_match(drawn: &Drawn) {
        for zone in &drawn.zones {
            let line: Vec<char> = visible(&drawn.lines[zone.row]).chars().collect();
            let part: String = line[zone.col..zone.col + zone.cols].iter().collect();
            if zone.kind == ZoneKind::Compact {
                assert_eq!((zone.rows, part.chars().next()), (1, Some('⟳')), "{zone:?}");
                assert!(part.ends_with(" %"), "{zone:?}");
            } else if zone.rows > 1 {
                assert!(part.starts_with('╻') && part.contains(&zone.member), "{zone:?}");
                let bottom = visible(&drawn.lines[zone.row + zone.rows - 1]);
                assert!(bottom.chars().nth(zone.col) == Some('╹'), "{zone:?}");
            } else {
                // A name too wide for the line is cut; its zone still names the member.
                let shown = part
                    .strip_suffix('…')
                    .filter(|cut| zone.member.starts_with(cut))
                    .map_or(part.as_str(), |_| &zone.member);
                assert_eq!((zone.rows, shown), (1, zone.member.as_str()), "{zone:?}");
            }
        }
    }

    #[test]
    fn zones_over_cards_and_names() {
        let b = board(vec![
            sample_card("coordinateur", true, State::Working),
            sample_card("lanceur", false, State::Idle),
            sample_card("interface", false, State::Waiting),
        ]);
        let drawn = render(&b, 70, 23);
        zones_match(&drawn);
        let at =
            |member: &str, kind| drawn.zones.iter().find(|z| z.member == member && z.kind == kind).cloned().unwrap();
        let zone =
            |member: &str, kind, row, col, rows, cols| Zone { member: member.into(), kind, row, col, rows, cols };
        assert_eq!(at("coordinateur", ZoneKind::Member), zone("coordinateur", ZoneKind::Member, 2, 0, 2, 70));
        assert_eq!(at("interface", ZoneKind::Member), zone("interface", ZoneKind::Member, 5, 0, 2, 70));
        // The context of a member at rest, to compact it.
        assert_eq!(at("lanceur", ZoneKind::Compact), zone("lanceur", ZoneKind::Compact, 9, 63, 1, 6));
        assert_eq!(drawn.zones.iter().filter(|z| z.kind == ZoneKind::Compact).count(), 1);
        // An absent member is named in a list, after its label.
        assert_eq!(at("ops", ZoneKind::Member), zone("ops", ZoneKind::Member, 11, 10, 1, 3));

        // Two side by side, the oldest agents at rest named.
        let mut cards =
            vec![sample_card("coordinateur", true, State::Idle), sample_card("archi", true, State::Working)];
        cards.extend((0..4).map(|i| sample_card(&format!("w{i}"), false, State::Working)));
        cards.extend((0..9).map(|i| sample_card(&format!("repos{i}"), false, State::Idle)));
        let drawn = render(&board(cards), 70, 23);
        zones_match(&drawn);
        let archi = drawn.zones.iter().find(|z| z.member == "archi").unwrap();
        assert_eq!((archi.row, archi.col, archi.cols), (2, 35, 35));
        let w1 = drawn.zones.iter().find(|z| z.member == "w1").unwrap();
        assert_eq!((w1.row, w1.col, w1.cols), (5, 35, 35));
        let named =
            drawn.zones.iter().filter(|z| z.member.starts_with("repos") && z.rows == 1 && z.kind == ZoneKind::Member);
        let named: Vec<(&str, usize)> = named.map(|z| (z.member.as_str(), z.col)).collect();
        assert_eq!(named, [("repos4", 10), ("repos5", 19), ("repos6", 28), ("repos7", 37), ("repos8", 46)]);
    }

    #[test]
    fn zones_follow_what_moves_them() {
        let cards = || vec![sample_card("coordinateur", true, State::Working), sample_card("dev", false, State::Idle)];
        let rows = |b: &Board| render(b, 70, 23).zones.iter().map(|z| z.row).collect::<Vec<_>>();
        // An error under the header: everything one line lower.
        let failing = Board { error: Some("claude agents: boom".into()), ..board(cards()) };
        zones_match(&render(&failing, 70, 23));
        assert_eq!(rows(&failing), rows(&board(cards())).iter().map(|r| r + 1).collect::<Vec<_>>());

        // Two side by side on an odd width: the right one a column wider.
        let mut many: Vec<Card> = (0..6).map(|i| sample_card(&format!("w{i}"), false, State::Working)).collect();
        many.extend((0..4).map(|i| sample_card(&format!("v{i}"), false, State::Waiting)));
        let drawn = render(&board(many), 71, 23);
        zones_match(&drawn);
        assert!(drawn.lines.iter().all(|l| visible(l).chars().count() <= 71));
        let w1 = drawn.zones.iter().find(|z| z.member == "w1").unwrap();
        assert_eq!((w1.col, w1.cols), (35, 36));

        // A name wider than its line: cut, its zone over what shows.
        let list = vec!["court".to_string(), "un-nom-bien-trop-long".to_string()];
        let drawn = names("au repos", &list, 20, Color::DarkGrey, usize::MAX);
        let lines: Vec<String> = drawn.lines.iter().map(|l| visible(l)).collect();
        assert_eq!(lines, ["au repos  court", "          un-nom-bi…"]);
        zones_match(&drawn);
        let cut = Zone { member: list[1].clone(), kind: ZoneKind::Member, row: 1, col: 10, rows: 1, cols: 10 };
        assert_eq!(drawn.zones[1], cut);
    }

    #[test]
    fn zone_bounds() {
        let zone = Zone { member: "dev".into(), kind: ZoneKind::Member, row: 2, col: 35, rows: 3, cols: 35 };
        assert!(zone.contains(35, 2) && zone.contains(69, 4));
        // Beside the card, above and under it.
        assert!(!zone.contains(34, 2) && !zone.contains(70, 2) && !zone.contains(35, 1) && !zone.contains(35, 5));
    }

    fn snapshot(names: &[&str]) -> Snapshot {
        let member = |name: &&str| crate::state::MemberInfo {
            name: name.to_string(),
            role: String::new(),
            contact: false,
            quiet: false,
            argv: Vec::new(),
            ..Default::default()
        };
        Snapshot {
            team: "web".into(),
            session: "web".into(),
            socket: "rtest".into(),
            dir: "/tmp/web".into(),
            claude: "claude".into(),
            config_dir: None,
            lang: Lang::Fr,
            members: names.iter().map(member).collect(),
            dashboard: Vec::new(),
            journal: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn at_rest_the_latest_first_by_the_moment() {
        let s = snapshot(&["a", "b", "c"]);
        let state = tempfile::tempdir().unwrap();
        let look = |busy: &[&str]| Sessions {
            members: ["a", "b", "c"]
                .map(|name| {
                    let status = if busy.contains(&name) { "busy" } else { "idle" };
                    let session = Running {
                        name: Some(name.into()),
                        cwd: Some("/tmp/web".into()),
                        status: Some(status.into()),
                        session_id: None,
                        pid: None,
                    };
                    (name.to_string(), session)
                })
                .into(),
            ..Default::default()
        };
        let mut memory = Memory::default();
        let mut order = |busy: &[&str]| -> Vec<String> {
            let board = memory.board(&s, state.path(), &look(busy));
            ordered(&board.cards).iter().map(|c| c.name.clone()).collect()
        };
        // Seen at rest in one look: the file's order.
        assert_eq!(order(&[]), ["a", "b", "c"]);
        // At rest again, the latest; in the same second as the others, all the same.
        assert_eq!(order(&["b"]), ["b", "a", "c"]);
        assert_eq!(order(&[]), ["b", "a", "c"]);
        // Two at rest again in one look: the file's order between them.
        assert_eq!(order(&["c", "a"]), ["a", "c", "b"]);
        assert_eq!(order(&[]), ["a", "c", "b"]);
    }

    #[test]
    fn renamed_on_its_conversation_keeps_its_curve() {
        let state = tempfile::tempdir().unwrap();
        let look = |name: &str, session: &str| Sessions {
            members: [(
                name.to_string(),
                Running {
                    name: Some(name.into()),
                    cwd: Some("/tmp/web".into()),
                    status: Some("busy".into()),
                    session_id: Some(session.into()),
                    pid: None,
                },
            )]
            .into(),
            ..Default::default()
        };
        let mut memory = Memory::default();
        let samples = |board: &Board| board.cards.first().map(|c| c.samples.len());
        memory.board(&snapshot(&["dev"]), state.path(), &look("dev", "s1"));
        assert_eq!(samples(&memory.board(&snapshot(&["dev"]), state.path(), &look("dev", "s1"))), Some(2));
        // Renamed: its Claude starts again, a few looks without it.
        let renamed = snapshot(&["dev-rust"]);
        assert!(memory.board(&renamed, state.path(), &Sessions::default()).cards.is_empty());
        assert_eq!(samples(&memory.board(&renamed, state.path(), &look("dev-rust", "s1"))), Some(3));
        assert!(!memory.activity.contains_key("dev"));
        // On a new conversation: a new curve.
        let other = snapshot(&["qa"]);
        assert_eq!(samples(&memory.board(&other, state.path(), &look("qa", "s2"))), Some(1));
    }

    #[test]
    fn states_for_the_menu() {
        let state = tempfile::tempdir().unwrap();
        let running = |name: &str, status: &str, session: &str| {
            let session = Running {
                name: Some(name.into()),
                cwd: Some("/tmp/web".into()),
                status: Some(status.into()),
                session_id: Some(session.into()),
                pid: None,
            };
            (name.to_string(), session)
        };
        let look = |members: &[(&str, &str, &str)]| Sessions {
            members: members.iter().map(|(n, st, id)| running(n, st, id)).collect(),
            ..Default::default()
        };
        let mut memory = Memory::default();
        let s = snapshot(&["dev", "qa", "ops"]);
        let board = memory.board(&s, state.path(), &look(&[("dev", "busy", "s1"), ("qa", "waiting", "s2")]));
        // One moment for every look below: the same moment gives the same since.
        let (now, seen) = (SystemTime::now(), Instant::now());
        let first = memory.states(&board, now, seen);
        let at = now.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(first.at, at);
        assert_eq!(first.members["dev"].state, State::Working);
        assert!((at - 1..=at).contains(&first.members["dev"].since));
        assert_eq!(first.members["qa"].state, State::Waiting);
        assert_eq!(first.absent, ["ops"]);
        // A look later, the same moment.
        let board = memory.board(&s, state.path(), &look(&[("dev", "busy", "s1"), ("qa", "waiting", "s2")]));
        assert_eq!(memory.states(&board, now, seen).members, first.members);
        // Renamed on its conversation, it keeps its moment; a new state, a new one.
        let renamed = snapshot(&["dev-rust", "qa"]);
        let board = memory.board(&renamed, state.path(), &look(&[("dev-rust", "busy", "s1"), ("qa", "idle", "s2")]));
        let states = memory.states(&board, now, seen);
        assert_eq!(states.members["dev-rust"], first.members["dev"]);
        assert!(!states.members.contains_key("dev"));
        assert_eq!(states.members["qa"].state, State::Idle);
        assert!(states.members["qa"].since >= at - 1);

        // Written whole, read back as it was.
        assert_eq!(read_states(state.path()), None);
        save_states(state.path(), &states).unwrap();
        assert_eq!(read_states(state.path()), Some(states));
        let text = fs::read_to_string(state.path().join(STATES)).unwrap();
        assert!(text.contains(r#""qa":{"state":"idle","#), "{text}");
        // A state of a later recruit, as one it does not know.
        fs::write(state.path().join(STATES), r#"{"at":1,"members":{"a":{"state":"asleep","since":0}},"absent":[]}"#)
            .unwrap();
        assert_eq!(read_states(state.path()).unwrap().members["a"].state, State::Other);
    }

    #[test]
    fn sessions_of_the_team_and_elsewhere() {
        let s = snapshot(&["coordinateur", "dev"]);
        let running: Vec<Running> = serde_json::from_value(serde_json::json!([
            {"name": "dev", "cwd": "/tmp/autre", "pid": 4},
            {"name": "coordinateur", "cwd": "/tmp/web", "status": "busy", "pid": 1},
            {"name": "coordinateur", "cwd": "/home/user/code/shop", "pid": 2},
            {"name": "dev", "cwd": "/tmp/autre", "pid": 5},
            {"name": "dev", "pid": 6},
            {"name": "inconnu", "cwd": "/tmp/ailleurs", "pid": 7},
            {"name": "dev", "cwd": "/tmp/pi\u{1b}[31mège", "pid": 8},
            {"name": "dev", "cwd": "/tmp/\u{202e}gpj.exe\u{200b}", "pid": 9},
        ]))
        .unwrap();
        let found = sorted(&s, running);
        assert_eq!(found.members.keys().collect::<Vec<_>>(), ["coordinateur"]);
        assert_eq!(found.by_pid.get(&7).map(String::as_str), Some("inconnu"));
        // In the file's order, each once; the folder printed without its control or formatting characters.
        let elsewhere: Vec<(&str, Option<&str>)> =
            found.elsewhere.iter().map(|(n, f)| (n.as_str(), f.as_deref())).collect();
        assert_eq!(
            elsewhere,
            [
                ("coordinateur", Some("shop")),
                ("dev", None),
                ("dev", Some("autre")),
                ("dev", Some("gpj.exe")),
                ("dev", Some("pi[31mège"))
            ]
        );
    }

    #[test]
    fn names_in_use_elsewhere_over_the_usage() {
        let cards = || {
            vec![
                sample_card("coordinateur", true, State::Working),
                sample_card("lanceur", false, State::Idle),
                sample_card("interface", false, State::Waiting),
            ]
        };
        let b = Board { elsewhere: vec![("coordinateur".into(), Some("shop".into()))], ..board(cards()) };
        let lines =
            |height: usize| -> Vec<String> { render(&b, 70, height).lines.iter().map(|l| visible(l)).collect() };
        let line = t!(" nom en double ailleurs : {}", " name in use elsewhere: {}", "coordinateur (shop)");
        // Right over the usage, dimmed.
        let tall = lines(23);
        assert_eq!(tall[20], line);
        assert!(tall[21].contains("41 % ↻ 1h22") && tall[19].is_empty());
        assert!(render(&b, 70, 23).lines[20].contains(&line.clone().dim().to_string()));
        // The cards first: no room left, no line.
        assert_eq!(lines(16)[13], line);
        assert!(!lines(15).contains(&line), "{:#?}", lines(15));
        assert_eq!(render(&b, 70, 15).zones, render(&board(cards()), 70, 15).zones);
        // Several, on one line at most.
        let b = Board {
            elsewhere: vec![("coordinateur".into(), Some("shop".into())), ("dev".into(), None)],
            ..board(cards())
        };
        let shown = render(&b, 30, 23)
            .lines
            .iter()
            .map(|l| visible(l))
            .find(|l| l.contains("ailleurs") || l.contains("elsewhere"));
        assert_eq!(shown.map(|l| (l.chars().count(), l.ends_with('…'))), Some((30, true)));
    }

    #[test]
    fn click_on_the_dashboard() {
        let dir = tempfile::tempdir().unwrap();
        let s = snapshot(&["coordinateur", "dev"]);
        let at = |x, y| member_at(&s, dir.path(), Kind::Dashboard, x, y, 70, "");
        // Nothing drawn yet.
        assert_eq!(at(0, 2), None);
        let zone =
            |member: &str, kind, row, col, rows, cols| Zone { member: member.into(), kind, row, col, rows, cols };
        let zones = vec![
            zone("coordinateur", ZoneKind::Member, 2, 0, 3, 35),
            zone("dev", ZoneKind::Member, 2, 35, 3, 35),
            zone("dev", ZoneKind::Compact, 4, 62, 1, 6),
            zone("parti", ZoneKind::Member, 6, 10, 1, 5),
        ];
        save_zones(dir.path(), &zones).unwrap();
        assert_eq!(super::zones(dir.path()), zones);
        assert_eq!(at(0, 2).as_deref(), Some("coordinateur"));
        assert_eq!(at(40, 4).as_deref(), Some("dev"));
        // The header, past the cards, and a name that is no member.
        assert_eq!(at(3, 0), None);
        assert_eq!(at(3, 5), None);
        assert_eq!(at(12, 6), None);
        assert_eq!(member_at(&s, dir.path(), Kind::Toggle, 0, 2, 70, ""), None);
        // On the context of a member at rest: compacting it, not going to it; elsewhere on its card, no compacting.
        assert_eq!(compaction_at(&s, dir.path(), 63, 4).as_deref(), Some("dev"));
        assert_eq!(at(63, 4).as_deref(), Some("dev"));
        assert_eq!(compaction_at(&s, dir.path(), 40, 4), None);
        // Only the file is left, no partial one.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn click_on_the_journal() {
        let s = snapshot(&["coordinateur", "dev", "dev-cli", "rédac"]);
        let within =
            |line: &str, x, columns| member_at(&s, Path::new("/nonexistent"), Kind::Journal, x, 0, columns, line);
        let at = |line: &str, x| within(line, x, 80);
        let line = format!(" 14:32  dev-cli {ARROW} dev");
        let names: Vec<Option<String>> = (0..24).map(|x| at(&line, x)).collect();
        let expected = |x: usize| match x {
            8..15 => Some("dev-cli".to_string()),
            20..23 => Some("dev".to_string()),
            _ => None,
        };
        assert_eq!(names, (0..24).map(expected).collect::<Vec<_>>());
        // A name in another: the whole one under the click.
        let line = format!(" 14:32  dev {ARROW} dev-cli   ");
        assert_eq!(at(&line, 10).as_deref(), Some("dev"));
        assert_eq!(at(&line, 16).as_deref(), Some("dev-cli"));
        assert_eq!(at(&line, 23), None);
        // Columns, not bytes: `é` takes two bytes and one column.
        let line = format!(" 09:05  rédac {ARROW} coordinateur");
        assert_eq!(at(&line, 12).as_deref(), Some("rédac"));
        assert_eq!(at(&line, 13), None);
        assert_eq!(at(&line, 18).as_deref(), Some("coordinateur"));
        // A recipient that is no member: a process nobody was found for.
        assert_eq!(at(&format!(" 09:05  dev {ARROW} uds:/tmp/cc-socks/42.sock"), 16), None);
        // Cut to a narrow journal rather than wrapped: the cut name is no one's.
        let head = visible(&message_head(&s, "14:32", "coordinateur", "dev-cli", 28));
        assert_eq!(head, format!(" 14:32  coordinateur {ARROW} de…"));
        assert_eq!(at(&head, 8).as_deref(), Some("coordinateur"));
        assert!((20..28).all(|x| at(&head, x).is_none()));
        let wide = visible(&message_head(&s, "14:32", "coordinateur", "dev-cli", 40));
        assert_eq!(at(&wide, 25).as_deref(), Some("dev-cli"));
        // Written 40 wide, then wrapped again by tmux in a pane narrowed to 28: `dev` up to the last column, `-cli` on
        // the next line. The recipient may go on: no one; the sender still answers.
        let wrapped = format!(" 14:32  coordinateur {ARROW} dev");
        assert_eq!(wrapped.chars().count(), 28);
        assert!((25..28).all(|x| within(&wrapped, x, 28).is_none()));
        assert_eq!(within(&wrapped, 8, 28).as_deref(), Some("coordinateur"));
        // A column left after it: the whole name.
        assert_eq!(within(&wrapped, 25, 29).as_deref(), Some("dev"));
        assert_eq!(within(&wrapped, 27, 29).as_deref(), Some("dev"));
        // Not a message's first line: its text, the title, or no time in front.
        assert!((0..40).all(|x| at("        ╰─ dev-cli asks dev ──▶ coordinateur", x).is_none()));
        assert!((0..30).all(|x| at(&format!(" Journal          {ALT}j masque"), x).is_none()));
        assert!((0..20).all(|x| at(&format!(" ab:cd  dev {ARROW} dev-cli"), x).is_none()));
    }

    #[test]
    fn curves() {
        let all: Vec<(i64, bool)> = (0..HISTORY).map(|i| (HISTORY - i, true)).collect();
        assert_eq!(curve(&all, HISTORY, 10), "⣿".repeat(10));
        // The last minute, over the end of a slice and the start of the next one.
        let recent: Vec<(i64, bool)> = (0..60).map(|i| (HISTORY - i, false)).collect();
        assert_eq!(curve(&recent, HISTORY, 10), format!("{}⣀⣀", " ".repeat(8)));

        // Slices of three minutes, working one, resting the next, since `base`.
        let base = 200 * 180;
        let samples =
            |until: i64| -> Vec<(i64, bool)> { (base..=until).map(|t| (t, (t - base) / 180 % 2 == 0)).collect() };
        let now = base + 1800;
        assert_eq!(curve(&samples(now), now, 10), "⣀⣿⣀⣿⣀⣿⣀⣿⣀⣿");
        // Half a slice later, the past ones have not moved.
        assert_eq!(curve(&samples(now + 90), now + 90, 10), "⣀⣿⣀⣿⣀⣿⣀⣿⣀⣿");
        // A new slice: the curve moves by one.
        assert_eq!(curve(&samples(now + 90), now + 180, 10), "⣿⣀⣿⣀⣿⣀⣿⣀⣿ ");
    }

    #[test]
    fn usage_from_the_freshest_window() {
        let report = |at: i64, five_hour: f64, resets: Option<&str>| Report {
            rate_limits: vec![bridge::RateLimit {
                kind: "five_hour".into(),
                percent_used: five_hour,
                resets_at: resets.map(String::from),
            }],
            at,
            ..Default::default()
        };
        let now = iso_epoch("2026-10-07T12:00:00Z").unwrap();
        let five_hour = |reports: &[Report]| windows(reports, now)[0].clone();
        // A member at rest reports just now what it heard long ago; the other, earlier, a newer use.
        let stale = report(now, 3.0, Some("2026-10-07T14:00:00.000Z"));
        let fresh = report(now - 30, 4.0, Some("2026-10-07T14:00:00Z"));
        assert_eq!(five_hour(&[stale.clone(), fresh.clone()]), ("5 h".into(), 4.0, Some(7200)));
        assert_eq!(five_hour(&[fresh.clone(), stale.clone()]).1, 4.0);
        // A new window resets later: its use starts again from little, whatever the old one reached.
        let full = report(now, 97.0, Some("2026-10-07T15:00:00Z"));
        let new_window = report(now - 60, 1.0, Some("2026-10-07T20:00:00Z"));
        assert_eq!(five_hour(&[full.clone(), new_window]).1, 1.0);
        // Without a reset time, only when nothing else tells one.
        let unknown = report(now, 50.0, None);
        assert_eq!(five_hour(&[unknown.clone(), fresh.clone()]).1, 4.0);
        assert_eq!(five_hour(std::slice::from_ref(&unknown)), ("5 h".into(), 50.0, None));
        // The window is over and nobody has heard of the next one: nothing used yet, no reset time.
        let over = iso_epoch("2026-10-07T15:00:00Z").unwrap();
        assert_eq!(windows(&[full, stale], over)[0], ("5 h".into(), 0.0, None));
        assert!(windows(&[], now).is_empty());
    }

    #[test]
    fn names_keep_their_column() {
        let b = board(Vec::new());
        let column = |state| {
            let right = Right { model: 4, effort: 7, time: 2 };
            let top = visible(&card(&sample_card("archi", false, state), 40, 0, Helpers::Left, &right, &b)[0]);
            assert_eq!(top.chars().count(), 40);
            top.split("archi").next().unwrap().chars().count()
        };
        for state in [State::Working, State::Idle, State::Waiting, State::Other] {
            assert_eq!(column(state), 4);
        }
    }

    #[test]
    fn header_counts_then_less_when_narrow() {
        let mut cards = vec![
            sample_card("a", true, State::Working),
            sample_card("b", false, State::Working),
            sample_card("c", false, State::Waiting),
        ];
        cards.extend((0..3).map(|i| sample_card(&format!("r{i}"), false, State::Idle)));
        let mut b = Board { frame: 2, ..board(cards) };
        let keys = keys();
        let pad = |width: usize, used: usize| " ".repeat(width - used - keys.chars().count());
        let counts = "⠹ 2  ⚑ 1  ◷ 3";
        let at = |b: &Board, width: usize| visible(&header(b, width));
        // Everything takes a space, the time (8), the counts (2 + 13), two spaces and the keys, whose length depends
        // on `ALT`.
        let all = 26 + keys.chars().count();
        assert_eq!(at(&b, all + 10), format!(" {}  {counts}{}{keys}", clock(1000), pad(all + 10, 24)));
        assert_eq!(at(&b, all), format!(" {}  {counts}  {keys}", clock(1000)));
        // The time goes first, then the keys; the counts stay.
        assert_eq!(at(&b, all - 1), format!(" {counts}{}{keys}", pad(all - 1, 14)));
        assert_eq!(at(&b, all - 10), format!(" {counts}  {keys}"));
        assert_eq!(at(&b, all - 11), format!(" {counts}"));
        assert_eq!(at(&b, 14), format!(" {counts}"));
        // Narrower still, cut rather than wrapped.
        assert_eq!(at(&b, 10), " ⠹ 2  ⚑ 1…");
        b.glyphs = Glyphs::Nerd;
        assert_eq!(at(&b, 14), " \u{F06A9} 2  \u{F009E} 1  \u{F0150} 3");
        // No one running: the time alone, then the keys.
        let empty = board(Vec::new());
        assert_eq!(at(&empty, 11 + keys.chars().count()), format!(" {}  {keys}", clock(1000)));
    }

    #[test]
    fn usage_on_one_line() {
        let usage = board(Vec::new()).usage;
        let days = t!("6j11h", "6d11h");
        let (full, empty) = (|n| "▰".repeat(n), |n| "▱".repeat(n));
        let wide = visible(&usage_line(&usage, 70));
        assert_eq!(wide, format!(" 5 h {}{} 41 % ↻ 1h22   7 j ▰{} 5 % ↻ {days}", full(7), empty(10), empty(16)));
        assert_eq!(wide.chars().count(), 70);
        // Too narrow: the reset times go first, then the bars.
        assert_eq!(
            visible(&usage_line(&usage, 40)),
            format!(" 5 h {}{} 41 %   7 j {} 5 %", full(4), empty(5), empty(9))
        );
        assert_eq!(visible(&usage_line(&usage, 25)), " 5 h 41 %   7 j 5 %");
        // Never longer than 20 cells.
        assert_eq!(visible(&usage_line(&usage, 200)).matches(['▰', '▱']).count(), 40);
        assert_eq!(remaining(42 * 60), "42min");
        assert_eq!(remaining(3900), "1h05");
        assert_eq!(remaining(3 * 86_400 + 4 * 3600 + 59), t!("3j4h", "3d4h"));
    }

    #[test]
    fn short_texts() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 4), "abc");
    }
}
