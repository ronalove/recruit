// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The dashboard: who works, who rests, who waits, its looks each second, and the board it draws.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::style::Color;
use crossterm::{cursor, queue, terminal};

use crate::claude::bridge::{Helper, Report};
use crate::claude::{self};
use crate::mux::Mux;
use crate::sys::now;
use crate::team::state::Snapshot;
use crate::ui::canvas::{Canvas, Features, Painter, columns, fit};
use crate::ui::look::{self, FRAME, Glyphs, State, frame};
use crate::{claude::bridge, t};

use super::agents::*;
use super::arrange::*;
use super::clock::*;
use super::keyboard::*;
use super::paint::*;
use super::sessions::*;
use super::states::*;
use super::zones::*;
use super::*;

/// How often the dashboard asks `claude agents` while each member's pane tells its state: for the sessions (the
/// journal's transcripts, the session a member resumes), the names in use elsewhere and the processes.
const AGENTS_EVERY: Duration = Duration::from_secs(5);

pub(super) fn dashboard(first: &Snapshot, state: &Path) -> Result<()> {
    let mut out = std::io::stdout();
    queue!(out, cursor::Hide)?;
    let redraw = Arc::new(AtomicBool::new(false));
    let _quiet = quiet_input(Arc::clone(&redraw));
    watch_resizes();
    // Whether a member's pane does not tell its state: `claude agents` is then asked each second.
    let eager = &AtomicBool::new(true);
    std::thread::scope(|scope| {
        // `claude agents --json` takes a tenth of a second or more, and far more on a loaded machine: asked on the
        // side, it stalls neither the spinners nor the looks of each second.
        let (found, agents) = mpsc::channel();
        scope.spawn(move || ask_agents(first, state, found, eager));
        // Each second, the states as the members' panes tell them (one question to the server), with what `claude
        // agents` last said: the states file is written each second, however long `claude agents` takes.
        let (sender, news) = mpsc::channel();
        scope.spawn(move || look_at_panes(first, state, agents, sender, eager));
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
                    let seen = memory.states(&board, SystemTime::now(), Instant::now());
                    let _ = states.send(sessions.written(seen));
                }
            }
            for card in &mut board.cards {
                if let Some((_, at)) = memory.since.get(&card.data.name) {
                    card.data.since = at.elapsed().as_secs();
                }
            }
            board.now = now();
            board.frame = frame(start.elapsed());
            if redraw.swap(false, Ordering::Relaxed) | RESIZED.swap(false, Ordering::Relaxed) {
                screen.forget();
            }
            screen.draw(&mut out, &board)?;
            if saved.as_ref() != Some(&screen.zones) {
                // Not written, it is only the clicks that miss; tried again when the layout changes.
                let _ = save_zones(state, &screen.zones);
                saved = Some(screen.zones.clone());
            }
            // The spinners turn while someone works; otherwise only the clock moves, redrawn on the second.
            let working = board.cards.iter().any(|c| c.data.state == State::Working);
            fresh = match news.recv_timeout(if working { FRAME } else { next_second() }) {
                Ok(look) => Some(look),
                Err(RecvTimeoutError::Timeout) => None,
                // Only a panic stops the looking, and the scope passes it on.
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            };
        }
    })
}

/// Asks `claude agents` for the sessions again and again, and sends what it says; each second while a member's pane
/// does not tell its state (`eager`), else every `AGENTS_EVERY`. Ends when nothing listens any more.
fn ask_agents(first: &Snapshot, state: &Path, found: Sender<Sessions>, eager: &AtomicBool) {
    let mut s = first.clone();
    loop {
        reread(&mut s, state);
        let asked = Instant::now();
        if found.send(sessions(&s, Some(state))).is_err() {
            break;
        }
        // Again in a while; each second while a member's pane does not tell its state.
        while asked.elapsed() < AGENTS_EVERY && !eager.load(Ordering::Relaxed) {
            std::thread::sleep(TICK);
        }
        if eager.load(Ordering::Relaxed) {
            std::thread::sleep(TICK);
        }
    }
}

/// Each second, the sessions `claude agents` last said (`agents`) with what the members' panes tell now, and the
/// signs the attached clients' terminals carry, sent to the drawing; `eager` says whether a pane tells nothing.
fn look_at_panes(
    first: &Snapshot,
    state: &Path,
    agents: Receiver<Sessions>,
    sender: Sender<(Snapshot, (Sessions, Glyphs))>,
    eager: &AtomicBool,
) {
    // The clients asked at each look: one can attach again from another terminal.
    let mux = Mux::at(state);
    let glyphs = || {
        let clients = mux.clients();
        Glyphs::of(clients.iter().map(|c| (c.term.as_str(), c.name.as_deref().unwrap_or_default())))
    };
    let mut s = first.clone();
    // The first look waits a moment for `claude agents`, no more: the panes' states are enough to draw.
    let mut last = match agents.recv_timeout(TICK) {
        Ok(found) => found,
        Err(RecvTimeoutError::Timeout) => Sessions { kept: kept(state), ..Default::default() },
        Err(RecvTimeoutError::Disconnected) => return,
    };
    loop {
        reread(&mut s, state);
        while let Ok(found) = agents.try_recv() {
            last = found;
        }
        let panes = mux.panes().ok();
        let (told, failed) = (told(panes.as_deref()), failed(panes.as_deref()));
        let mut sessions = Sessions { told, failed, ..last.clone() };
        eager.store(s.members.iter().any(|m| !sessions.told.contains_key(&m.name)), Ordering::Relaxed);
        with_a_command(&mut sessions, |pid| claude::session_status(s.config_dir.as_deref(), pid));
        if sender.send((s.clone(), (sessions, glyphs()))).is_err() {
            break;
        }
        std::thread::sleep(TICK);
    }
}

/// What the dashboard remembers from one look to the next: since when each member is in its state, and what it did.
#[derive(Default)]
pub(super) struct Memory {
    pub(super) since: HashMap<String, (State, Instant)>,
    pub(super) activity: HashMap<String, Activity>,
    /// The name each session last showed under, and when: a member renamed on its conversation keeps its time and
    /// its curve, across the seconds it takes to start again.
    pub(super) sessions: HashMap<String, (String, Instant)>,
}

/// Older than this, in seconds, a member's report says nothing of the agents it started: its mod ticks every 2 s.
pub(super) const FRESH_REPORT: i64 = 10;

/// The agents a member's session started that its card shows at `now`: while its mod reports (a member stopped took
/// them along), those still shown.
pub(super) fn helpers_shown(report: &Report, now: i64) -> Vec<Helper> {
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
    pub(super) fn states(&self, board: &Board, now: SystemTime, seen: Instant) -> States {
        let at = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let members = board.cards.iter().filter_map(|card| {
            let (state, since) = self.since.get(&card.data.name)?;
            let since = (at - seen.saturating_duration_since(*since).as_millis() as i64).div_euclid(1000);
            Some((card.data.name.clone(), MemberState { state: *state, since, shell: card.data.shell }))
        });
        States {
            at: at.div_euclid(1000),
            members: members.collect(),
            absent: board.absent.clone(),
            refs: BTreeMap::new(),
            sessions: BTreeMap::new(),
            pids: BTreeMap::new(),
            errors: board.cards.iter().filter_map(|card| Some((card.data.name.clone(), card.error.clone()?))).collect(),
            cards: board
                .cards
                .iter()
                .map(|card| {
                    (
                        card.data.name.clone(),
                        CardLook { doing: card.data.doing.clone(), activity: card.data.activity.clone() },
                    )
                })
                .collect(),
        }
    }

    /// The board for the sessions just found.
    pub(super) fn board(&mut self, s: &Snapshot, state: &Path, sessions: &Sessions) -> Board {
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
        // Since when a member's pane says its state, counted back from this moment.
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        for member in members {
            let session = sessions.members.get(&member.name);
            let told = sessions.told.get(&member.name);
            if session.is_none() && told.is_none() {
                board.absent.push(member.name.clone());
                continue;
            }
            if let Some(id) = session.and_then(|session| session.session_id.as_ref()) {
                self.follow_renamed(id, &member.name, seen);
            }
            // As its pane tells it, else as `claude agents` last said.
            let st = match told {
                Some((st, _)) => *st,
                None => State::of(session.and_then(|s| s.status.as_deref()).unwrap_or_default()),
            };
            // At rest with a command running: since its file says, a panel opened again included.
            let shell_since = sessions.shell.get(&member.name).copied().flatten().filter(|_| st == State::Idle);
            let told_since = told.and_then(|(_, since)| *since);
            let (since, activity) = self.note(&member.name, st, (told_since, shell_since), (now, at), seen);
            let report = bridge::report(state, &member.name).unwrap_or_default();
            let (model, effort) = look::shown(bridge::model_and_effort(state, member));
            let context = report.context.as_ref();
            let helpers = helpers_shown(&report, now);
            let error = sessions.failed.get(&member.name).filter(|_| st == State::Error);
            let data = CardData {
                name: member.name.clone(),
                color: member_color(s, &member.name),
                state: st,
                since,
                shell: sessions.shell.contains_key(&member.name),
                context: context.and_then(|c| c.percent),
                pressure: report.pressure(),
                model,
                effort,
                doing: bridge::doing(state, &member.name),
                activity,
                failed: (st == State::Error).then(|| look::error_label(error.map(String::as_str))),
                compacted: report.compacted.as_ref().filter(|c| c.done).map(|c| c.at),
            };
            board.cards.push(Card { data, contact: member.contact, helpers, error: error.cloned() });
        }
        self.forget_the_gone(s);
        // The working agents at rest, the latest first, by the very moment: two in the same second keep their order
        // from one second to the next, and so does the dashboard (see `ordered`).
        board.cards.sort_by_key(|c| {
            (!c.contact && resting(&c.data)).then(|| std::cmp::Reverse(self.since.get(&c.data.name).map(|(_, at)| *at)))
        });
        board
    }

    /// `name` carries on the session `id`, which was shown under another name before: its time and its curve follow it.
    fn follow_renamed(&mut self, id: &str, name: &str, seen: Instant) {
        if let Some((old, _)) = self.sessions.get(id).filter(|(old, _)| old != name) {
            let old = old.clone();
            if let Some(since) = self.since.remove(&old) {
                self.since.insert(name.to_string(), since);
            }
            if let Some(activity) = self.activity.remove(&old) {
                self.activity.insert(name.to_string(), activity);
            }
        }
        self.sessions.insert(id.to_string(), (name.to_string(), seen));
    }

    /// Notes that `name` is in `state` at this look: the seconds since it is (counted again when it changed; from what
    /// its pane says, else from its command's file, `since` as `(told, shell)`), and its last half hour.
    /// `moment` is the look's time in seconds and the clock in milliseconds.
    fn note(
        &mut self,
        name: &str,
        state: State,
        since: (Option<u64>, Option<i64>),
        moment: (i64, u64),
        seen: Instant,
    ) -> (u64, Activity) {
        let (now, at) = moment;
        let entry = self.since.entry(name.to_string()).or_insert((state, seen));
        if entry.0 != state {
            *entry = (state, seen);
        }
        if let Some(since) = since.0 {
            let back = Duration::from_millis(at.saturating_sub(since.saturating_mul(1000)));
            entry.1 = seen.checked_sub(back).unwrap_or(seen);
        }
        if let Some(since) = since.1 {
            let back = Duration::from_secs(now.saturating_sub(since).max(0) as u64);
            entry.1 = seen.checked_sub(back).unwrap_or(seen);
        }
        let elapsed = entry.1.elapsed().as_secs();
        let activity = self.activity.entry(name.to_string()).or_default();
        activity.note(now, state == State::Working);
        (elapsed, activity.clone())
    }

    /// Those no longer in the team, forgotten; but for a while those whose session may come back under another name.
    fn forget_the_gone(&mut self, s: &Snapshot) {
        self.sessions.retain(|_, (_, at)| at.elapsed() < KEEP);
        let kept = |name: &String| {
            s.members.iter().any(|m| &m.name == name) || self.sessions.values().any(|(last, _)| last == name)
        };
        let gone: Vec<String> = self.since.keys().chain(self.activity.keys()).filter(|n| !kept(n)).cloned().collect();
        for name in gone {
            self.since.remove(&name);
            self.activity.remove(&name);
        }
    }
}

/// What the pane shows, sent by difference cell by cell ([`Painter`]): with the spinners turning ten times a second,
/// a frame sends the few cells that changed, not the lines they are on. Each frame a synchronized update: the
/// multiplexer, which may read a whole one in several goes, shows it only once it is all there.
pub(super) struct Screen {
    /// The frame being drawn, in the same buffers from one to the next.
    pub(super) canvas: Canvas,
    pub(super) painter: Painter,
    pub(super) zones: Vec<Zone>,
}

impl Default for Screen {
    fn default() -> Self {
        Screen {
            canvas: Canvas::default(),
            painter: Painter::new(Features { sync: true, ..Features::default() }),
            zones: Vec::new(),
        }
    }
}

impl Screen {
    /// Forgets what the pane shows: the next frame is sent whole.
    pub(super) fn forget(&mut self) {
        self.painter.forget();
    }

    /// The board over the pane, only what changed.
    pub(super) fn draw(&mut self, out: &mut impl Write, board: &Board) -> Result<()> {
        let changes = self.changes(terminal::size().unwrap_or((80, 24)), board);
        if !changes.is_empty() {
            out.write_all(&changes)?;
            out.flush()?;
        }
        Ok(())
    }

    /// What to write for `board` on a pane of `size`: the cells that changed, in one write; nothing if none did.
    pub(super) fn changes(&mut self, size: (u16, u16), board: &Board) -> Vec<u8> {
        let (width, height) = (size.0 as usize, size.1 as usize);
        let Drawn { lines, zones } = render(board, width, height);
        self.canvas.reset(width, height);
        for (y, line) in lines.iter().enumerate() {
            line.draw(&mut self.canvas, y);
        }
        self.zones = zones;
        self.painter.frame(&self.canvas)
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
pub(super) fn usage(state: &Path, s: &Snapshot, now: i64) -> Vec<(String, f64, Option<u64>)> {
    let reports: Vec<Report> = s.members.iter().filter_map(|m| bridge::report(state, &m.name)).collect();
    windows(&reports, now)
}

/// Each window as the reports tell it: label, percent used, seconds before it resets. A report keeps what its session
/// last heard, old news for a member at rest however recent the report: per window, the one that resets last (a new
/// window resets later than the one before), and of those the highest use (it only goes up in a window). Without a
/// reset time, a report counts only when no other tells one.
pub(super) fn windows(reports: &[Report], now: i64) -> Vec<(String, f64, Option<u64>)> {
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

/// The dashboard's lines for a pane of `width` × `height`: the cards from the top, each in the form the room leaves
/// it (see [`arrange`]), the account's usage at the bottom; no header, its frame says the counts and the bar the
/// keys. An error first, if any.
pub(super) fn render(b: &Board, width: usize, height: usize) -> Drawn {
    let mut top = Vec::new();
    if let Some(error) = &b.error {
        top.extend([Line::of(fit(error, width), Paint::Color(Color::DarkRed)), Line::default()]);
    }
    let mut bottom = Vec::new();
    if !b.usage.is_empty() {
        bottom.extend([Line::default(), usage_line(&b.usage, width)]);
    }
    let absent = names(&t!("absent", "absent"), &b.absent, width, Color::DarkGrey, usize::MAX).spaced();
    let room = height.saturating_sub(1 + top.len() + bottom.len() + absent.len());
    let layout = arrange(&ordered(&b.cards), width, room);

    let mut middle = cards(&layout.rows, width, b);
    if layout.more > 0 {
        middle.lines.push(Line::of(t!("… et {} de plus", "… and {} more", layout.more), Paint::Dim));
    }
    middle.append(resting_names(&layout.named, width, layout.whole));
    // The agents the sessions started, in the room the members leave.
    let left = room.saturating_sub(middle.len());
    middle.append(helper_block(b, width, left));
    middle.append(absent);
    // Sessions open elsewhere under a member's name, over the usage: in the room the cards leave, if any.
    if let Some(line) = elsewhere_line(&b.elsewhere, width) {
        let mut with = if bottom.is_empty() { vec![Line::default()] } else { bottom.clone() };
        with.insert(1, line);
        if 1 + top.len() + middle.len() + with.len() <= height {
            bottom = with;
        }
    }

    let filler = height.saturating_sub(1 + top.len() + middle.len() + bottom.len());
    let mut drawn = Drawn::from(top);
    drawn.append(middle);
    drawn.append(Drawn::from(vec![Line::default(); filler]));
    drawn.append(Drawn::from(bottom));
    // A pane too low for all that: the usage at the bottom only, if it holds.
    let rows = height.saturating_sub(1);
    if drawn.len() > rows {
        let mut low = Vec::new();
        if !b.usage.is_empty() && rows >= 1 {
            low.extend(vec![Line::default(); rows - 1]);
            low.push(usage_line(&b.usage, width));
        }
        drawn = Drawn::from(low);
    }
    drawn
}

/// ` 5 h ▰▰▰▰▱▱▱▱ 41 % ↻ 4h27   7 j ▰▱▱▱▱▱▱▱ 5 % ↻ 6j11h`, the bars as long as the line allows. Too narrow, the reset
/// times go first, then the bars.
pub(super) fn usage_line(usage: &[(String, f64, Option<u64>)], width: usize) -> Line {
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
            .map(|((label, ..), (used, left))| 2 + columns(label) + columns(used) + columns(left))
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
    let mut line = Line::default();
    for (i, ((label, percent, _), (used, left))) in usage.iter().zip(tails).enumerate() {
        line.push(format!("{} {label} ", if i == 0 { "" } else { "  " }), Paint::Plain);
        if cells > 0 {
            let filled = ((percent / 100.0 * cells as f64).round() as usize).min(cells);
            let color = match percent {
                p if *p >= 90.0 => Color::Red,
                p if *p >= 70.0 => look::palette().working,
                _ => Color::Green,
            };
            line.push("▰".repeat(filled), Paint::Color(color));
            line.push("▱".repeat(cells - filled), Paint::Color(Color::DarkGrey));
            line.push(" ", Paint::Plain);
        }
        line.push(used, Paint::Plain);
        line.push(left, Paint::Dim);
    }
    line
}

/// "42min", "4h27", "6j12h": the time before a window resets, short.
pub(super) fn remaining(secs: u64) -> String {
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes}min"),
        (0, _) => format!("{hours}h{minutes:02}"),
        _ => t!("{}j{}h", "{}d{}h", days, hours),
    }
}

#[cfg(test)]
mod tests {

    use crate::claude::Running;
    use crate::claude::bridge::{Helper, HelperStatus, Report, Task};

    use crate::ui::look::{self, Glyphs, State};
    use crate::{claude::bridge, t};

    use super::*;

    use crate::board::fixture::*;
    use crate::board::journal::*;

    use crate::board::{Board, Card};
    use crate::ui::look::ALT;

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
    fn forgotten_screen_written_whole() {
        let b = board(vec![sample_card("dev", false, State::Idle)]);
        let mut screen = Screen::default();
        let mut out = Vec::new();
        screen.draw(&mut out, &b).unwrap();
        let whole = out.len();
        assert!(whole > 0);
        // The same board: nothing to write.
        out.clear();
        screen.draw(&mut out, &b).unwrap();
        assert!(out.is_empty());
        // Forgotten, as after Ctrl-L or a resize: all of it again.
        screen.forget();
        out.clear();
        screen.draw(&mut out, &b).unwrap();
        assert_eq!(out.len(), whole);
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
            ordered(&board.cards).iter().map(|c| c.data.name.clone()).collect()
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
        // What the curve remembers of a card: its first stretch at work.
        let first = |board: &Board| board.cards.first().and_then(|c| c.data.activity.work.first().copied());
        let before = (now() - 100, now() - 90);
        memory.activity.insert("dev".into(), Activity { seen: before.0, work: vec![before], last: before.1 });
        assert_eq!(first(&memory.board(&snapshot(&["dev"]), state.path(), &look("dev", "s1"))), Some(before));
        // Renamed: its Claude starts again, a few looks without it.
        let renamed = snapshot(&["dev-rust"]);
        assert!(memory.board(&renamed, state.path(), &Sessions::default()).cards.is_empty());
        assert_eq!(first(&memory.board(&renamed, state.path(), &look("dev-rust", "s1"))), Some(before));
        assert!(!memory.activity.contains_key("dev"));
        // On a new conversation: a new curve.
        let other = snapshot(&["qa"]);
        let fresh = first(&memory.board(&other, state.path(), &look("qa", "s2")));
        assert!(fresh.is_some_and(|stretch| stretch != before), "{fresh:?}");
    }

    /// The pane shows the lines as drawn, a line as wide as the pane with its last cell; and while members work, a
    /// frame of the spinners sends their cells only (the title lines whole sent 733 bytes a frame).
    #[test]
    fn a_spinner_frame_sends_its_cells_only() {
        let working = |name: &str| {
            sample_card(name, false, State::Working).tweak(|c| {
                c.data.since = 30;
            })
        };
        let mut b = board(vec![working("dev-a"), working("dev-b"), working("dev-c")]);
        let (width, height) = (60, 23);
        let mut screen = Screen::default();
        let whole = screen.changes((width as u16, height as u16), &b);
        assert!(!whole.is_empty());
        let lines = render(&b, width, height).lines;
        assert!(lines.iter().any(|l| crate::ui::canvas::columns(&l.text()) == width), "{lines:#?}");
        for (y, line) in lines.iter().enumerate() {
            assert_eq!(screen.canvas.row(y).trim_end(), line.text().trim_end());
        }
        // The same board: nothing; the next spinners' image: a few bytes for each.
        assert!(screen.changes((width as u16, height as u16), &b).is_empty());
        b.frame += 1;
        let spun = screen.changes((width as u16, height as u16), &b);
        assert!(!spun.is_empty() && spun.len() <= 3 * 24, "{} bytes: {:?}", spun.len(), String::from_utf8_lossy(&spun));
        // A second of it, the times moving on once: under a kilobyte.
        let mut second = 0;
        for i in 0..10 {
            b.frame += 1;
            if i == 9 {
                b.cards.iter_mut().for_each(|c| c.data.since += 1);
                b.now += 1;
            }
            second += screen.changes((width as u16, height as u16), &b).len();
        }
        eprintln!("a frame: {} bytes, a second: {second} bytes", spun.len());
        assert!(second < 1000, "{second} bytes in a second");
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
        let lines = |height: usize| -> Vec<String> { padded(&b, 70, height).lines.iter().map(Line::text).collect() };
        let line = t!(" nom en double ailleurs : {}", " name in use elsewhere: {}", "coordinateur (shop)");
        // Right over the usage, dimmed.
        let tall = lines(23);
        assert_eq!(tall[20], line);
        assert!(tall[21].contains("41 % ↻ 1h22") && tall[19].is_empty());
        assert_eq!(padded(&b, 70, 23).lines[20], Line::of(line.clone(), Paint::Dim));
        // The cards first: no room left, no line.
        assert_eq!(lines(16)[13], line);
        assert!(!lines(15).contains(&line), "{:#?}", lines(15));
        assert_eq!(padded(&b, 70, 15).zones, padded(&board(cards()), 70, 15).zones);
        // Several, on one line at most.
        let b = Board {
            elsewhere: vec![("coordinateur".into(), Some("shop".into())), ("dev".into(), None)],
            ..board(cards())
        };
        let shown =
            padded(&b, 30, 23).lines.iter().map(Line::text).find(|l| l.contains("ailleurs") || l.contains("elsewhere"));
        assert_eq!(shown.map(|l| (l.chars().count(), l.ends_with('…'))), Some((30, true)));
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
    fn no_size_too_small() {
        // Thirteen cards, the same with long names, twenty-six; with an error, none.
        let long: Vec<Card> = capture()
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                c.tweak(|c| {
                    c.data.name = format!("{}-avec-un-nom-très-long-{i}", c.data.name);
                })
            })
            .collect();
        let many: Vec<Card> = (0..2)
            .flat_map(|k| {
                capture().into_iter().map(move |c| {
                    c.tweak(|c| {
                        c.data.name = format!("{}{k}", c.data.name);
                    })
                })
            })
            .collect();
        let boards = [
            board(capture()),
            Board { glyphs: Glyphs::Nerd, ..board(long) },
            board(many),
            Board { error: Some("claude agents: boom".into()), usage: Vec::new(), ..board(Vec::new()) },
            board(vec![
                sample_card("dev-saisie", false, State::Error).tweak(|c| {
                    c.data.failed = Some(look::error_label(Some("API Error: 529")));
                    c.error = None;
                    c.data.doing = Some(Task { now: "Encodeur des touches kitty".into(), done: None });
                }),
                sample_card("dev-terminal", false, State::Working),
            ]),
        ];
        let s = snapshot(&["coordinateur", "dev-cli"]);
        let failed = std::sync::Mutex::new(Vec::new());
        // Every small size, which carry nearly all the overflows, then a step.
        let widths: Vec<usize> = (0..=40).chain((44..=120).step_by(8)).collect();
        let heights: Vec<usize> = (0..=16).chain((20..=40).step_by(5)).collect();
        std::thread::scope(|scope| {
            for chunk in widths.chunks(widths.len().div_ceil(8)) {
                let (boards, heights, failed, s) = (&boards, &heights, &failed, &s);
                scope.spawn(move || {
                    for &width in chunk {
                        for &height in heights {
                            for (i, b) in boards.iter().enumerate() {
                                let size = (width as u16, height as u16);
                                let drawn = std::panic::catch_unwind(|| {
                                    Screen::default().changes(size, b);
                                    render(b, width, height)
                                });
                                if drawn.is_err() {
                                    failed.lock().unwrap().push(format!("the dashboard {i} at {width} × {height}"));
                                }
                            }
                        }
                        let journal = std::panic::catch_unwind(|| {
                            message_head(s, "14:32", "dev-cli", "coordinateur", width);
                            fit("Le texte d'un message", width.saturating_sub(11))
                        });
                        if journal.is_err() {
                            failed.lock().unwrap().push(format!("the journal at {width}"));
                        }
                    }
                });
            }
        });
        let failed = failed.into_inner().unwrap();
        assert!(failed.is_empty(), "{} failed: {}", failed.len(), failed.join(", "));
    }

    /// No header: its frame says the counts, the bar the keys; the cards from the top.
    #[test]
    fn no_header() {
        let cards = vec![sample_card("a", true, State::Working), sample_card("b", false, State::Idle)];
        let drawn = render(&board(cards), 60, 20);
        assert!(visible(&drawn.lines[0]).contains(" a"), "{:?}", drawn.lines[0]);
        assert!(drawn.lines.iter().all(|line| !visible(line).contains(&format!("{ALT}r menu"))));
        // An error first, then a blank.
        let failing = render(&Board { error: Some("boom".into()), ..board(Vec::new()) }, 60, 20);
        assert_eq!((visible(&failing.lines[0]).as_str(), failing.lines[1].text().as_str()), ("boom", ""));
        // A pane too low: the usage at the bottom only.
        let low = render(&board(vec![sample_card("a", true, State::Working); 6]), 60, 4);
        assert_eq!(low.lines.len(), 3);
        assert!(visible(&low.lines[2]).contains("5 h"), "{:?}", low.lines);
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
}
