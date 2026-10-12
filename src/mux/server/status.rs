// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the programs of the panes say of their state (OSC 7501) and the dashboard's looks, and what the server reads of
//! them.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::lifecycle::{log, now};
use super::pane::{Pane, TurnError};
use super::{Msg, Server};
use crate::mux::engine::{self, Relay, State, Surface};
use crate::team::state::Snapshot;
use crate::ui::canvas::is_direction_mark;
use crate::ui::look;
use crate::{board, claude::bridge};

/// How often the members' looks are read for their headers.
pub(super) const LOOKS: Duration = Duration::from_secs(2);

/// Every [`LOOKS`], the members' looks for their headers (`board::looks`: a few small files), to the loop when they
/// changed: never on the way to a frame, and no message while nothing changes. The team's automatic compaction goes
/// with them (`team.json`, which a change from the menu rewrites).
pub(super) fn watch_looks(state: PathBuf, tx: Sender<Msg>, watched: Arc<Mutex<Vec<String>>>) -> io::Result<()> {
    std::thread::Builder::new().name("looks".into()).spawn(move || {
        let mut last = BTreeMap::new();
        let mut compact = None;
        let mut outcomes = BTreeMap::new();
        loop {
            // A panic in its reading would end the server (the hook): caught, and tried again later.
            match crate::mux::caught(|| (board::looks(&state), Snapshot::read(&state).ok().map(|s| s.auto_compact))) {
                Ok((looks, share)) => {
                    if share.is_some() && share != compact {
                        compact = share;
                        if tx.send(Msg::AutoCompact(share.flatten())).is_err() {
                            return;
                        }
                    }
                    if looks != last {
                        if tx.send(Msg::Looks(looks.clone())).is_err() {
                            return;
                        }
                        last = looks;
                    }
                    // How the compactions asked went: their reports, for the members asked only.
                    let asked = watched.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    let said: BTreeMap<_, _> = asked
                        .into_iter()
                        .filter_map(|member| Some((member.clone(), bridge::report(&state, &member)?.compaction?)))
                        .collect();
                    if said != outcomes {
                        if tx.send(Msg::Outcomes(said.clone())).is_err() {
                            return;
                        }
                        outcomes = said;
                    }
                }
                Err(why) => log!("looks: {why}"),
            }
            std::thread::sleep(LOOKS);
        }
    })?;
    Ok(())
}

/// A pane's state, as the chrome draws it.
pub(super) fn look_state(state: Option<State>) -> look::State {
    match state {
        Some(State::Working) => look::State::Working,
        Some(State::Blocked) => look::State::Waiting,
        Some(State::Idle | State::Done) => look::State::Idle,
        Some(State::Error) => look::State::Error,
        Some(State::Clear) | None => look::State::Other,
    }
}

/// What a pane's program saying its state (`status`) comes to.
enum Told {
    /// A `working` while the member compacts on its own: not a turn, the member stays at rest; kept in `Pane::held`.
    Held,
    Noted {
        /// Another state while it compacted on its own: the end of the compaction (or of the turn it was in).
        compaction_over: bool,
        /// The member went waiting or failed out of sight: its notice.
        call: Option<look::State>,
    },
}

/// A pane's program said its state: noted in the pane. `compacting`: its member compacts on its own; `in_view`: the
/// pane is in the client's view.
fn on_status(
    pane: &mut Pane,
    status: &engine::Status,
    compacting: bool,
    looks: &BTreeMap<String, board::MemberLook>,
    in_view: bool,
) -> Told {
    let mut compaction_over = false;
    if status.id.is_none() && compacting {
        if status.state == State::Working {
            pane.held = Some((status.clone(), now()));
            return Told::Held;
        }
        compaction_over = true;
    }
    if status.id.is_none() {
        pane.held = None;
    }
    let state = look_state(Some(status.state));
    // So already, by its program or by the dashboard: no second notice.
    let call = (calls(state) && pane_state(pane, looks) != state && pane.is_member() && !in_view).then_some(state);
    note_status(pane, status);
    Told::Noted { compaction_over, call }
}

/// What its program says of its state, noted in a pane: since when (from its first state, then at each change), and
/// why its turn failed, kept until another state.
pub(super) fn note_status(pane: &mut Pane, status: &engine::Status) {
    if pane.since.is_none() || look_state(pane.state) != look_state(Some(status.state)) {
        pane.since = Some(now());
    }
    pane.state = Some(status.state);
    pane.error = (status.state == State::Error).then(|| TurnError {
        message: status.message.clone(),
        label: one_line(&look::error_label(status.message.as_deref())),
    });
}

/// A text from a pane's program on one line, for a header or a notice: its line breaks and tabs as spaces.
pub(super) fn one_line(text: &str) -> String {
    text.chars().map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c }).collect()
}

/// Whether a member gone into `state` out of sight gets a notice: waiting for the user, or its turn failed.
pub(super) fn calls(state: look::State) -> bool {
    matches!(state, look::State::Waiting | look::State::Error)
}

/// A pane's state, as its program says it (OSC 7501); for a member whose program says none, or `Clear`, as the
/// dashboard last saw it (`looks`): everywhere a member's state is shown (headers, counts, tabs, ⌥g, notices); never
/// in `Panes`, which the dashboard reads.
pub(super) fn pane_state(pane: &Pane, looks: &BTreeMap<String, board::MemberLook>) -> look::State {
    match look_state(pane.state) {
        look::State::Other if pane.is_member() => {
            looks.get(&pane.member).and_then(|looks| looks.state).unwrap_or(look::State::Other)
        }
        state => state,
    }
}

/// Since when a pane is in its [`pane_state`], in seconds since the epoch: by its program, else by the dashboard.
pub(super) fn pane_since(pane: &Pane, looks: &BTreeMap<String, board::MemberLook>) -> Option<u64> {
    if look_state(pane.state) != look::State::Other || !pane.is_member() {
        return pane.since;
    }
    looks.get(&pane.member)?.since.and_then(|since| u64::try_from(since).ok())
}

/// What a pane's program wrote last, for the log when it fails: its last lines up to the live screen's last row (its
/// `rows` rows),
/// a line wrapped joined to the next, the others by « ⏎ », at most [`LAST_WORDS`] characters.
pub(super) fn last_words(engine: &dyn Surface, rows: u16) -> String {
    const LINES: u64 = 40;
    let end = engine.top() + engine.scrolled() as u64 + u64::from(rows);
    let mut text = String::new();
    for line in end.saturating_sub(LINES).max(engine.oldest())..end {
        let Some(line) = engine.line(line) else { continue };
        if line.wrapped {
            text.push_str(&line.text);
        } else if !line.text.trim().is_empty() {
            text.push_str(line.text.trim_end());
            text.push_str(" ⏎ ");
        }
    }
    // What a program wrote goes to the log: no control (C0, C1) nor direction mark (as `canvas::bidi`), which would
    // act on whoever reads it in a terminal.
    let text: String = text.chars().filter(|&c| !c.is_control() && !is_direction_mark(c)).collect();
    let text = text.trim_end_matches(" ⏎ ").trim();
    let skip = text.chars().count().saturating_sub(LAST_WORDS);
    text.chars().skip(skip).collect()
}

/// The most of a failed pane's last words the log keeps.
const LAST_WORDS: usize = 600;

impl Server {
    /// The relays of every engine taken: kept for the client, or dropped without one (a clipboard filled hours
    /// later would surprise); the states noted, and since when; a member gone waiting out of sight noticed.
    pub(super) fn take_relays(&mut self) {
        let focus = self.focus.clone();
        let shown = self.in_view_ids();
        let mut relays = Vec::new();
        let mut called = Vec::new();
        let mut resized = None;
        // The members compacting on their own, by pane: their `working` is held back.
        let compacting: Vec<String> = self
            .panes
            .iter()
            .filter(|pane| pane.is_member() && self.compacting(&pane.member, pane.since))
            .map(|pane| pane.id.clone())
            .collect();
        let mut ended = Vec::new();
        let list_pane = self.list.as_ref().and(self.menu.as_ref()).map(|(menu, _)| menu.as_str());
        let held = self.gate.hold();
        let looks = &self.looks;
        for pane in &mut self.panes {
            relays.clear();
            let gate_free = pane.with(|engine| engine.relays(&mut relays));
            if gate_free.is_none() {
                continue;
            }
            for relay in relays.drain(..) {
                // The size a list asks for: honoured for the menu's list alone, relayed to no one.
                if let Relay::Resize { rows, cols } = relay {
                    if list_pane == Some(pane.id.as_str()) {
                        resized = Some((rows, cols));
                    }
                    continue;
                }
                if let Relay::Status(status) = &relay {
                    let told = on_status(pane, status, compacting.contains(&pane.id), looks, shown.contains(&pane.id));
                    match told {
                        Told::Held => continue,
                        Told::Noted { compaction_over, call } => {
                            if compaction_over {
                                ended.push(pane.member.clone());
                            }
                            if let Some(state) = call {
                                called.push((pane.id.clone(), state));
                            }
                        }
                    }
                    self.look_again = true;
                    self.cards.stale = true;
                }
                if matches!(relay, Relay::Title(_)) && focus.as_deref() != Some(pane.id.as_str()) {
                    continue;
                }
                if let Some(client) = self.client.as_mut() {
                    client.relays.add(relay);
                }
            }
        }
        drop(held);
        for member in ended {
            if let Some(asked) = self.auto.asked.get_mut(&member) {
                asked.ended = true;
            }
        }
        if let Some((rows, cols)) = resized {
            self.list_resize(rows, cols);
        }
        self.call(called);
    }
}
