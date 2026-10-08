// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The menu of a running team (`recruit _menu <state> [--client <name>] [--nerd]`), in a tmux popup opened by
//! `/recruit`, ⌥r or the bar's button: the members on the left as on the dashboard, the chosen one's sheet on the
//! right, the team's actions at the bottom (direction A of the mockups). Every change is written to the team's files
//! and applied at once to the running team.
//!
//! What it is quick to show comes first: nothing slow before the first frame (no `claude`, no `tmux`), the members'
//! states from what the dashboard last saw, then kept up to date on the side. Each frame sends only the cells that
//! changed (`canvas`). Team files that do not read still open it: the members as launched, to detach or stop.

mod draw;
mod sheet;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::terminal;

use crate::board;
use crate::canvas::{Painter, Session};
use crate::config::{self, Edit, Member, Origin};
use crate::live::Running;
use crate::look::{self, Glyphs, State, family};
use crate::state::Snapshot;
use crate::templates::Templates;
use crate::{bridge, claude, generate, layout, t};
use draw::{Drawn, Look};
use sheet::{Done, Effect, Env, Key, Person, Said, Sheet, Team};

pub fn run(state: &Path, client: Option<&str>, nerd: bool) -> Result<()> {
    // For `/recruit`, which cannot tell otherwise whether tmux opened the window.
    mark_opened(state);
    let mut running = Running::open(state)?;
    let mut sheet = Sheet::new(load(&running, client.is_some()));
    // What the dashboard saw, if it still looks: a small file, read before the first frame.
    if let Some(states) = fresh_states(state, board::now()) {
        sheet.seen(Some(states));
    }
    let (send_states, states) = mpsc::channel();
    let (watched, snapshot) = (state.to_path_buf(), running.snapshot.clone());
    std::thread::spawn(move || watch(watched, snapshot, send_states));
    let mut menu = Menu {
        running: &mut running,
        client,
        session: Session::open()?,
        painter: Painter::default(),
        states,
        news: mpsc::channel(),
        composers: Composers::default(),
        start: Instant::now(),
        glyphs: if nerd { Glyphs::Nerd } else { Glyphs::Unicode },
    };
    let result = menu.run(&mut sheet);
    menu.close();
    result
}

/// Written in the team's folder each time the menu starts, with a token of its own.
const OPENED: &str = "menu.opened";

/// A token no other start of the menu writes: its process, its clock, and a count for two starts in one process
/// within the clock's resolution. Compared as it is, never as a time: on Linux, a file's modification time comes
/// from a coarser clock than `SystemTime::now()`, a file written after a moment may look older than it.
fn mark_opened(state: &Path) {
    static STARTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let count = STARTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _ = config::write_atomic(&state.join(OPENED), &format!("{} {nanos} {count}", std::process::id()));
}

/// The token of the menu's last start, read before asking for a new one: `opened_since` waits for another.
pub fn last_opened(state: &Path) -> Option<String> {
    fs::read_to_string(state.join(OPENED)).ok()
}

/// Whether the menu started in the team's folder since `last_opened` gave `before`.
pub fn opened_since(state: &Path, before: Option<&str>) -> bool {
    last_opened(state).is_some_and(|now| Some(now.as_str()) != before)
}

/// Further than this from now, `states.json` is not looked after: the dashboard is off or stopped, or the clock moved.
const STALE: i64 = 3;

/// How often the menu asks `claude agents --json` itself, the dashboard off.
const ASK_EVERY: Duration = Duration::from_secs(2);

/// Each member's state and since when.
type States = HashMap<String, (State, i64)>;

/// What the dashboard last saw, if it saw it lately.
fn fresh_states(state: &Path, now: i64) -> Option<States> {
    let states = board::read_states(state)?;
    ((now - states.at).abs() <= STALE)
        .then(|| states.members.into_iter().map(|(name, m)| (name, (m.state, m.since))).collect())
}

/// Keeps the members' states up to date for the menu, on the side: what the dashboard writes, read every half second,
/// or else `claude agents --json` every two seconds, with since when as this menu has seen them. None when they
/// cannot be seen: not shown as they were.
fn watch(state: PathBuf, mut snapshot: Snapshot, send: Sender<Option<States>>) {
    let mut sent: Option<Option<States>> = None;
    let mut seen: States = HashMap::new();
    let mut asked: Option<Instant> = None;
    loop {
        let now = board::now();
        let states = match fresh_states(&state, now) {
            Some(states) => Some(Some(states)),
            None if asked.is_none_or(|at| at.elapsed() >= ASK_EVERY) => {
                asked = Some(Instant::now());
                if let Ok(again) = Snapshot::read(&state) {
                    snapshot = again;
                }
                let running = claude::running(&snapshot.claude, snapshot.config_dir.as_deref()).ok();
                Some(running.map(|running| {
                    let mut states = HashMap::new();
                    for member in &snapshot.members {
                        let session = running.iter().find(|r| {
                            r.name.as_deref() == Some(member.name.as_str())
                                && r.cwd.as_deref().map(Path::new) == Some(snapshot.dir.as_path())
                        });
                        let Some(session) = session else { continue };
                        let st = State::of(session.status.as_deref().unwrap_or_default());
                        let since = match seen.get(&member.name) {
                            Some((before, since)) if *before == st => *since,
                            _ => now,
                        };
                        states.insert(member.name.clone(), (st, since));
                    }
                    seen = states.clone();
                    states
                }))
            }
            None => None,
        };
        if let Some(states) = states
            && sent.as_ref() != Some(&states)
        {
            if send.send(states.clone()).is_err() {
                return;
            }
            sent = Some(states);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The team as the menu shows it: the members as launched, their settings as the team's files give them; when the
/// files do not read, the members as launched only, and why.
fn load(running: &Running, detach: bool) -> Team {
    let s = &running.snapshot;
    let live = s.plugin_dir.is_some();
    let person = |i: usize, m: &crate::state::MemberInfo| {
        let (model, effort) = bridge::model_and_effort(&running.state, m);
        Person {
            name: m.name.clone(),
            contact: m.contact,
            color: look::member_color(i),
            gone: false,
            model: model.map(|m| family(&m)),
            effort: effort.filter(|e| bridge::EFFORTS.contains(&e.as_str())),
            role: m.role.clone(),
            layers: None,
            tab_now: None,
            live,
        }
    };
    let found = match running.config() {
        Ok(found) => found,
        Err(error) => {
            let members = s
                .members
                .iter()
                .map(|m| (m.name.clone(), Member { role: m.role.clone(), contact: m.contact, ..Default::default() }));
            return Team {
                name: s.team.clone(),
                origin: s.origin.clone().unwrap_or(Origin::Local { root: s.dir.clone() }),
                people: s.members.iter().enumerate().map(|(i, m)| person(i, m)).collect(),
                config: config::Team { members: members.collect(), ..Default::default() },
                dashboard: !s.dashboard.is_empty(),
                detach,
                file: String::new(),
                unreadable: Some(format!("{error:#}")),
            };
        }
    };
    let origin = found.origin();
    let tabs = layout::tabs(&found.team);
    let contacts = found.team.contacts();
    let people = s
        .members
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let gone = !found.team.members.contains_key(&m.name);
            let layers = if gone { None } else { config::member_layers(&origin, &found.name, &m.name).ok() };
            Person {
                contact: if gone { m.contact } else { contacts.contains(&m.name.as_str()) },
                gone,
                role: layers.as_ref().map_or_else(|| m.role.clone(), |l| l.role.value.clone()),
                layers,
                tab_now: tabs.iter().find(|t| t.members.contains(&m.name)).map(|t| t.title.clone()),
                ..person(i, m)
            }
        })
        .collect();
    let dashboard =
        config::dashboard_layers(&origin, &found.name).map_or(!s.dashboard.is_empty(), |layers| layers.value);
    let file = match &found.root {
        Some(root) => {
            found.file.strip_prefix(root).map_or_else(|_| config::tilde(&found.file), |p| p.display().to_string())
        }
        None => config::tilde(&found.file),
    };
    Team { name: found.name.clone(), origin, people, config: found.team, dashboard, detach, file, unreadable: None }
}

/// What the sheet asks of the running team.
struct Live<'a> {
    running: &'a Running,
}

impl Env for Live<'_> {
    fn restarts(&self, edits: &[Edit]) -> Vec<String> {
        self.running.restarts(edits)
    }

    /// As `member.rs` decides: the session noted for it, and its conversation file, not empty.
    fn has_conversation(&self, member: &str) -> bool {
        let s = &self.running.snapshot;
        let noted = fs::read_to_string(self.running.state.join("sessions").join(member)).unwrap_or_default();
        !noted.trim().is_empty()
            && claude::transcript(&s.dir, s.config_dir.as_deref(), noted.trim()).metadata().is_ok_and(|m| m.len() > 0)
    }
}

/// What Claude composing a new agent tells: its process, then what it composed, for request `id`.
enum News {
    Started(u64, u32),
    Composed(u64, Result<(String, Member), String>),
}

struct Menu<'a> {
    running: &'a mut Running,
    /// The tmux client the menu was opened from: the one that detaches.
    client: Option<&'a str>,
    session: Session,
    painter: Painter,
    states: Receiver<Option<States>>,
    news: (Sender<News>, Receiver<News>),
    composers: Composers,
    start: Instant,
    glyphs: Glyphs,
}

impl Menu<'_> {
    fn run(&mut self, sheet: &mut Sheet) -> Result<()> {
        loop {
            let drawn = self.draw(sheet)?;
            let working = sheet.states.values().any(|(s, _)| *s == State::Working) || sheet.draft.composing.is_some();
            // The spinners turn while someone works; otherwise only the times move, on the second.
            let wait = if working { look::FRAME } else { Duration::from_millis(1000 - millis() % 1000) };
            if event::poll(wait)? {
                // All that is there, then one frame.
                loop {
                    let effects = self.event(sheet, &drawn, event::read()?);
                    if self.carry_all(sheet, effects)? {
                        return Ok(());
                    }
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
            while let Ok(states) = self.states.try_recv() {
                sheet.seen(states);
            }
            while let Ok(news) = self.news.1.try_recv() {
                self.heard(news, Some(sheet));
            }
        }
    }

    fn draw(&mut self, sheet: &Sheet) -> Result<Drawn> {
        let (width, height) = terminal::size().map_or((80, 24), |(w, h)| (w as usize, h as usize));
        let look = Look {
            glyphs: self.glyphs,
            frame: look::frame(self.start.elapsed()),
            now: board::now(),
            option: cfg!(target_os = "macos"),
        };
        let drawn = draw::draw(sheet, &look, width, height);
        let bytes = self.painter.frame(drawn.canvas.clone());
        self.session.write(&bytes)?;
        Ok(drawn)
    }

    fn event(&mut self, sheet: &mut Sheet, drawn: &Drawn, event: Event) -> Vec<Effect> {
        let env = Live { running: self.running };
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key_of(key) {
                Some(key) => sheet.key(key, &env),
                None => Vec::new(),
            },
            Event::Paste(text) => sheet.key(Key::Paste(text), &env),
            Event::Mouse(mouse) => {
                let (x, y) = (mouse.column as usize, mouse.row as usize);
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => match drawn.hit(x, y) {
                        Some(target) => sheet.click(target, &env),
                        None => Vec::new(),
                    },
                    MouseEventKind::ScrollDown => sheet.wheel(x < drawn.split, true, &env),
                    MouseEventKind::ScrollUp => sheet.wheel(x < drawn.split, false, &env),
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    /// Carries out effects in turn; true when the menu is done.
    fn carry_all(&mut self, sheet: &mut Sheet, effects: Vec<Effect>) -> Result<bool> {
        for effect in effects {
            if self.carry(sheet, effect)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Carries out an effect; true when the menu is done.
    fn carry(&mut self, sheet: &mut Sheet, effect: Effect) -> Result<bool> {
        let failed = |error: anyhow::Error| Said::Failed(format!("{error:#}"));
        match effect {
            Effect::Apply { edits, said, select } => {
                let added = edits.iter().any(|e| matches!(e, Edit::Add { .. }));
                match self.running.apply(&edits) {
                    Ok(()) => {
                        self.reload(sheet, select);
                        if added {
                            sheet.added();
                            sheet.pane = sheet::Pane::List;
                        }
                        sheet.said = Some(Said::Done(match said {
                            Done::Text(text) => text,
                            Done::Contacts => {
                                let made: Vec<&str> = sheet.team.config.contacts();
                                t!("Interlocuteurs : {}.", "Contacts: {}.", sheet::quoted(&made))
                            }
                        }));
                    }
                    Err(error) => sheet.said = Some(failed(error)),
                }
            }
            Effect::Restart { member, fresh, said } => {
                sheet.said = Some(self.running.restart(&member, fresh).map_or_else(failed, |()| Said::Done(said)));
                self.reload(sheet, None);
            }
            Effect::RestartAll => {
                let done = t!("Toute l'équipe a été relancée à neuf.", "The whole team was restarted afresh.");
                sheet.said = Some(self.running.restart_all().map_or_else(failed, |()| Said::Done(done)));
                self.reload(sheet, None);
            }
            Effect::Dismiss(name) => {
                let done = t!("Panneau de « {} » fermé.", "Pane of \"{}\" closed.", name);
                sheet.said = Some(self.running.dismiss(&name).map_or_else(failed, |()| Said::Done(done)));
                self.reload(sheet, None);
            }
            Effect::Editor(edited) => {
                let text = sheet.to_edit(&edited);
                let state = self.running.state.clone();
                let result = self.session.lend(|| edit_text(&state, &text, &editor()))?;
                self.painter.forget();
                let effects = sheet.edited(edited, result.map_err(|error| format!("{error:#}")));
                return self.carry_all(sheet, effects);
            }
            Effect::Compose { id, request } => self.compose(sheet, id, request),
            Effect::CancelCompose(id) => {
                if let Some(pid) = self.composers.cancel(id) {
                    stop(pid);
                }
            }
            Effect::Detach => {
                let Some(client) = self.client else { return Ok(false) };
                match self.running.detach(client) {
                    Ok(()) => return Ok(true),
                    Err(error) => sheet.said = Some(failed(error)),
                }
            }
            Effect::Stop => match self.running.stop() {
                Ok(()) => return Ok(true),
                Err(error) => sheet.said = Some(failed(error)),
            },
            Effect::Quit => return Ok(true),
        }
        Ok(false)
    }

    /// The team read again after a change, the same member chosen (or `select`).
    fn reload(&mut self, sheet: &mut Sheet, select: Option<String>) {
        sheet.reload(load(self.running, self.client.is_some()), select);
    }

    /// Claude composes the new agent of request `id` on the side; its process noted, to stop it.
    fn compose(&mut self, sheet: &Sheet, id: u64, request: String) {
        self.composers.asked(id);
        let s = self.running.snapshot.clone();
        let team = sheet.team.config.clone();
        let send = self.news.0.clone();
        std::thread::spawn(move || {
            let templates = Templates::for_lang(s.lang);
            let started = send.clone();
            let mut tell = |pid: u32| {
                let _ = started.send(News::Started(id, pid));
            };
            let mut claude = claude::command(&s.claude, s.config_dir.as_deref());
            claude.current_dir(&s.dir);
            let result = generate::compose_member(claude, &s.team, &team, &request, &templates, &mut tell)
                .map_err(|error| format!("{error:#}"));
            let _ = send.send(News::Composed(id, result));
        });
    }

    /// What Claude composing tells: its process, stopped at once if its request was given up; what it composed, for
    /// the sheet (none when the menu closes).
    fn heard(&mut self, news: News, sheet: Option<&mut Sheet>) {
        match news {
            News::Started(id, pid) => {
                if let Some(pid) = self.composers.started(id, pid) {
                    stop(pid);
                }
            }
            News::Composed(id, result) => {
                self.composers.done(id);
                if let Some(sheet) = sheet {
                    sheet.composed(id, result);
                }
            }
        }
    }

    /// The menu closes: every Claude still composing is stopped, those about to start too (waited for a little).
    fn close(&mut self) {
        let until = Instant::now() + Duration::from_millis(500);
        loop {
            while let Ok(news) = self.news.1.try_recv() {
                self.heard(news, None);
            }
            if !self.composers.starting() || Instant::now() >= until {
                break;
            }
            if let Ok(news) = self.news.1.recv_timeout(Duration::from_millis(50)) {
                self.heard(news, None);
            }
        }
        for pid in self.composers.all() {
            stop(pid);
        }
    }
}

/// Stops the process Claude composes in.
fn stop(pid: u32) {
    // SAFETY: a signal to a process this menu started.
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// Claude's processes composing new agents, by request: asked, started, given up. A request given up before its
/// process is known has it stopped as soon as it is.
#[derive(Default)]
struct Composers {
    asked: HashSet<u64>,
    running: HashMap<u64, u32>,
    given_up: HashSet<u64>,
}

impl Composers {
    fn asked(&mut self, id: u64) {
        self.asked.insert(id);
    }

    /// Request `id`'s process started: the process to stop at once when the request was given up.
    fn started(&mut self, id: u64, pid: u32) -> Option<u32> {
        self.asked.remove(&id);
        if self.given_up.remove(&id) {
            return Some(pid);
        }
        self.running.insert(id, pid);
        None
    }

    /// Request `id` done, composed or failed.
    fn done(&mut self, id: u64) {
        self.asked.remove(&id);
        self.running.remove(&id);
        self.given_up.remove(&id);
    }

    /// Request `id` given up: its process to stop; not known yet, it is stopped as it starts.
    fn cancel(&mut self, id: u64) -> Option<u32> {
        if let Some(pid) = self.running.remove(&id) {
            return Some(pid);
        }
        if self.asked.contains(&id) {
            self.given_up.insert(id);
        }
        None
    }

    /// Whether a request asked has no process known yet.
    fn starting(&self) -> bool {
        !self.asked.is_empty()
    }

    /// Every process still composing, all given up.
    fn all(&mut self) -> Vec<u32> {
        self.asked.clear();
        self.given_up.clear();
        self.running.drain().map(|(_, pid)| pid).collect()
    }
}

/// Milliseconds since the epoch.
fn millis() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn key_of(event: KeyEvent) -> Option<Key> {
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
    let alt = event.modifiers.contains(KeyModifiers::ALT);
    Some(match event.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::Char('c') if ctrl => Key::Esc,
        KeyCode::Char('w') if ctrl => Key::EraseWord,
        KeyCode::Char('u') if ctrl => Key::EraseAll,
        // ^H: what some terminals send for Backspace.
        KeyCode::Char('h') if ctrl => Key::Backspace,
        KeyCode::Backspace if alt => Key::EraseWord,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Char(c) if !ctrl && !alt => Key::Char(c),
        _ => return None,
    })
}

/// The user's editor: `$VISUAL`, `$EDITOR`, else vi.
fn editor() -> String {
    ["VISUAL", "EDITOR"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|e| !e.trim().is_empty()))
        .unwrap_or_else(|| "vi".into())
}

/// `text` in `editor`, as it is once the editor closes; an error when the editor fails. Through a file in the team's
/// folder, removed afterwards.
fn edit_text(state: &Path, text: &str, editor: &str) -> Result<String> {
    let file = state.join(format!("menu-{}.md", std::process::id()));
    fs::write(&file, text)?;
    // The editor may come with its arguments (`code -w`).
    let status = Command::new("/bin/sh").arg("-c").arg(format!("{editor} \"$1\"")).arg("sh").arg(&file).status();
    let edited = fs::read_to_string(&file);
    let _ = fs::remove_file(&file);
    match status {
        Ok(status) if status.success() => Ok(edited?),
        Ok(status) => anyhow::bail!(t!("« {} » a échoué ({})", "\"{}\" failed ({})", editor, status)),
        Err(error) => anyhow::bail!(t!("« {} » ne se lance pas : {}", "\"{}\" does not start: {}", editor, error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_opened_since_asked() {
        let dir = tempfile::tempdir().unwrap();
        let before = last_opened(dir.path());
        assert!(!opened_since(dir.path(), before.as_deref()), "never opened");
        mark_opened(dir.path());
        assert!(opened_since(dir.path(), before.as_deref()));
        let before = last_opened(dir.path());
        assert!(!opened_since(dir.path(), before.as_deref()), "nothing new since");
        // However soon after, whatever the clocks: the file's time is not looked at.
        mark_opened(dir.path());
        assert!(opened_since(dir.path(), before.as_deref()));
    }

    #[test]
    fn states_from_the_dashboard_while_fresh() {
        let dir = tempfile::tempdir().unwrap();
        assert!(fresh_states(dir.path(), 100).is_none(), "no file");
        fs::write(
            dir.path().join(board::STATES),
            r#"{"at": 100, "members": {"dev": {"state": "working", "since": 90}}, "absent": []}"#,
        )
        .unwrap();
        assert_eq!(fresh_states(dir.path(), 102).unwrap()["dev"], (State::Working, 90));
        assert!(fresh_states(dir.path(), 104).is_none(), "older than 3 s");
        assert!(fresh_states(dir.path(), 97).is_some(), "a little ahead: the clocks of two processes");
        assert!(fresh_states(dir.path(), 96).is_none(), "further ahead than 3 s");
    }

    #[test]
    fn keys() {
        let key = |code, modifiers| key_of(KeyEvent::new(code, modifiers));
        assert_eq!(key(KeyCode::Char('c'), KeyModifiers::CONTROL), Some(Key::Esc));
        assert_eq!(key(KeyCode::Char('h'), KeyModifiers::CONTROL), Some(Key::Backspace));
        assert_eq!(key(KeyCode::Backspace, KeyModifiers::ALT), Some(Key::EraseWord));
        assert_eq!(key(KeyCode::Char('R'), KeyModifiers::SHIFT), Some(Key::Char('R')));
        assert_eq!(key(KeyCode::Char('x'), KeyModifiers::ALT), None);
    }

    #[test]
    fn claude_given_up_before_it_starts_is_stopped_as_it_starts() {
        let mut composers = Composers::default();
        composers.asked(1);
        assert_eq!(composers.cancel(1), None, "no process known yet");
        assert_eq!(composers.started(1, 4242), Some(4242), "stopped as soon as it is known");
        assert!(composers.all().is_empty(), "and not kept");
        // Known before it is given up: stopped at once.
        composers.asked(2);
        assert_eq!(composers.started(2, 7), None);
        assert_eq!(composers.cancel(2), Some(7));
        // Done: nothing left to stop.
        composers.asked(3);
        composers.started(3, 8);
        composers.done(3);
        assert_eq!(composers.cancel(3), None);
        assert!(!composers.starting());
        // The menu closing: those running stopped, those asked waited for.
        composers.asked(4);
        assert!(composers.starting());
        assert_eq!(composers.started(4, 9), None);
        assert_eq!(composers.all(), [9]);
    }

    #[test]
    fn editing() {
        let dir = tempfile::tempdir().unwrap();
        let error = edit_text(dir.path(), "x", "false").unwrap_err().to_string();
        assert!(error.contains("false"), "{error}");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "its file removed");
        assert_eq!(edit_text(dir.path(), "x", "true").unwrap(), "x");
        let error = edit_text(dir.path(), "x", "/nonexistent/editor").unwrap_err().to_string();
        assert!(error.contains("nonexistent"), "{error}");
    }
}
