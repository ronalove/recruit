// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The image: when a frame is due, what each header shows, and the screen composed from the panes and the layers.

use std::sync::atomic::Ordering;
use std::sync::{MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::clients::{Client, Out};
use super::focus_layout::CARDS;
use super::lifecycle::{log, now};
use super::pane::{Pane, Role};
use super::status::{look_state, pane_state};
use super::{FRAME, Hover, Server};
use crate::mux::cards::{self, Panel};
use crate::mux::chrome;
use crate::mux::engine::Engine;
use crate::mux::layout;
use crate::mux::proto::ServerMsg;
use crate::mux::screen::{self, Scene, View};
use crate::ui::Rect;
use crate::ui::canvas::Canvas;
use crate::ui::look::{self, Glyphs};
use crate::{board, t};

/// The signs the client's terminal can show.
pub(super) fn glyphs(client: &Client) -> Glyphs {
    Glyphs::of([(client.term.as_str(), client.caps.name.as_deref().unwrap_or_default())])
}

/// The members' panes as wide as each other take the same form of header, the poorest that one of them needs (but one
/// with nothing left at the right: `chrome::common_form`), for their columns to line up: a
/// layout of each header on its own, as `chrome::frame` makes it for its drawing.
pub(super) fn common_forms(views: &mut [View<'_>]) {
    let forms: Vec<Option<(usize, usize)>> = views
        .iter()
        .map(|view| view.header.zoom.map(|_| (view.area.width, chrome::form(&view.header, view.area.width))))
        .collect();
    for (view, form) in views.iter_mut().zip(&forms) {
        let Some((width, _)) = form else { continue };
        view.header.form =
            chrome::common_form(forms.iter().flatten().filter(|(w, _)| w == width).map(|(_, form)| *form));
    }
}

/// How long until a time shown as `secs` (12s, 22m, 1h05) reads otherwise: the next second under a minute, else the
/// next minute; `nanos`: how far into its second now is.
pub(super) fn tick(secs: u64, nanos: u32) -> Duration {
    let into = Duration::from_nanos(u64::from(nanos));
    let whole = if secs < 60 { 1 } else { 60 - secs % 60 };
    Duration::from_secs(whole).saturating_sub(into).max(Duration::from_millis(1))
}

/// A member's own color, by its place in the team's order, as `board` gives it; one the order does not know yet
/// (a pane opened by `_ctl`) comes after the others.
pub(super) fn member_color(order: &[String], member: &str) -> crossterm::style::Color {
    look::member_color(order.iter().position(|name| name == member).unwrap_or(order.len()))
}

/// The most urgent of states: waiting, else failed, else working, else at rest.
pub(super) fn urgent(states: impl Iterator<Item = look::State>) -> look::State {
    let states: Vec<look::State> = states.collect();
    [look::State::Waiting, look::State::Error, look::State::Working, look::State::Idle]
        .into_iter()
        .find(|state| states.contains(state))
        .unwrap_or(look::State::Other)
}

/// What happened to a pane, for its header.
fn note(pane: &Pane) -> Option<String> {
    let mut notes = Vec::new();
    if let Some(why) = &pane.failure {
        notes.push(t!("moteur en échec : {}", "engine failed: {}", why));
    }
    if pane.exited {
        notes.push(t!("terminé", "exited"));
    }
    (!notes.is_empty()).then(|| notes.join(" · "))
}

impl Server {
    /// The next frame, if one is due and the client can take it.
    pub(super) fn frame(&mut self) {
        if !self.dirty || !(self.pacing.echo || self.pacing.last_frame.is_none_or(|at| at.elapsed() >= FRAME)) {
            return;
        }
        for pane in &self.panes {
            pane.told.store(false, Ordering::Release);
        }
        self.take_relays();
        let Some(client) = self.client.as_ref() else {
            self.dirty = false;
            self.pacing.echo = false;
            return;
        };
        if client.busy {
            if let Some(client) = self.client.as_mut() {
                client.late = true;
            }
            self.dirty = false;
            return;
        }
        let started = Instant::now();
        self.check_selection();
        if !self.compose() {
            return;
        }
        // Every move of the mouse, always: the headers' parts, the tabs and the buttons light up under the pointer, a
        // choice's selection follows it, the focused pane may ask. Only a change of what is lit draws again.
        let Some(client) = self.client.as_mut() else { return };
        let relays = client.relays.take();
        self.pacing.written += relays.len() as u64;
        if !relays.is_empty() {
            client.out.push(Out::Output(relays));
        }
        if !client.motion {
            client.motion = true;
            client.out.message(&ServerMsg::Motion(true));
        }
        let bytes = client.painter.frame(self.screen.borrow().canvas());
        self.dirty = false;
        // Nothing changed on screen, the cursor included (the pointer lit what was lit already): no frame, no round
        // trip with the client, and the next one is not held back.
        if bytes.is_empty() {
            self.pacing.echo = false;
            return;
        }
        self.pacing.frames += 1;
        self.pacing.written += bytes.len() as u64;
        client.out.push(Out::Output(bytes));
        client.busy = true;
        self.pacing.last_frame = Some(started);
        self.pacing.echo = false;
    }

    /// A pane's header: a member's state, since when, its model, effort and context (`looks`), its zoom, and the
    /// part under the pointer; a panel's title (the dashboard, the journal, the menu). `epoch`: now, in seconds.
    pub(super) fn header<'a>(
        &'a self,
        pane: &'a Pane,
        note: Option<&'a str>,
        zoomed: Option<&str>,
        epoch: u64,
    ) -> chrome::Header<'a> {
        let hint = match pane.role {
            Role::Journal => Some(chrome::Hint::JournalSize),
            Role::Dashboard => Some(self.counts()),
            _ => None,
        };
        let base = chrome::Header {
            // The menu names itself on its first line: its frame has no title.
            name: if pane.role == Role::Menu { "" } else { &pane.member },
            state: pane_state(pane, &self.looks),
            note,
            hint,
            ..chrome::Header::default()
        };
        if !pane.is_member() {
            return base;
        }
        let looks = self.looks.get(&pane.member);
        let context = looks.and_then(|looks| looks.context);
        // Its program says its state: at rest by it. Else (none said, or `Clear`) as the dashboard last saw it.
        let told = look_state(pane.state) != look::State::Other;
        // At rest, or its turn failed: a click on its context compacts it.
        let compactable = if told {
            matches!(base.state, look::State::Idle | look::State::Error) && context.is_some()
        } else {
            looks.is_some_and(|looks| looks.compactable)
        };
        // At rest by its program, a command still running by `claude agents` (`"status": "shell"`, which OSC 7501 does
        // not tell): the command's sign, and since when it runs, as its card. At work, or
        // waiting, its program wins.
        let shell = looks.is_some_and(|looks| looks.shell)
            && !matches!(base.state, look::State::Working | look::State::Waiting | look::State::Error);
        // Why its turn failed, short: as its program said it, else unknown (as the dashboard saw it).
        let error = (base.state == look::State::Error)
            .then(|| pane.error.as_ref().map_or(self.notices.unknown_error.as_str(), |error| error.label.as_str()));
        let seen = || looks?.since.and_then(|since| u64::try_from(since).ok());
        let own = pane.since.filter(|_| told);
        let since = if shell { seen().or(own) } else { own.or_else(seen) };
        let hover = match &self.pointer.hover {
            Some(Hover::Part(id, part)) if *id == pane.id => Some(*part),
            _ => None,
        };
        chrome::Header {
            // A member's color, as on the dashboard; none for the dashboard, the journal, the menu.
            color: Some(member_color(&self.order, &pane.member)),
            model: looks.and_then(|looks| looks.model.as_deref()),
            effort: looks.and_then(|looks| looks.effort.as_deref()),
            context,
            pressure: looks.map(|looks| looks.pressure).unwrap_or_default(),
            compactable,
            shell,
            error,
            since: since.map(|since| epoch.saturating_sub(since)),
            zoom: Some(if zoomed == Some(pane.id.as_str()) { chrome::Zoom::Out } else { chrome::Zoom::In }),
            hover,
            // The text a list is open under stays lit.
            open: self.list.as_ref().filter(|list| list.pane == pane.id).map(|list| list.part),
            ..base
        }
    }

    /// How many members work, wait for the user, failed their turn and rest, for the dashboard's border, as their
    /// programs say it; for one whose program says nothing (no OSC 7501, or `Clear`), as the
    /// dashboard last saw it (`looks`).
    pub(super) fn counts(&self) -> chrome::Hint {
        let (mut working, mut waiting, mut error, mut idle) = (0, 0, 0, 0);
        for pane in self.panes.iter().filter(|pane| pane.is_member()) {
            match pane_state(pane, &self.looks) {
                look::State::Working => working += 1,
                look::State::Waiting => waiting += 1,
                look::State::Error => error += 1,
                look::State::Idle => idle += 1,
                look::State::Other => {}
            }
        }
        chrome::Hint::Counts { working, waiting, error, idle }
    }

    /// The screen as it is, in `self.screen`: the shown tab's panes in their frames, the bar. False while an engine
    /// is broken.
    pub(super) fn compose(&mut self) -> bool {
        let (width, height) = (self.size.0 as usize, self.size.1 as usize);
        let secs = now();
        self.tick_cards(secs);
        let held = self.gate.hold();
        let mut shown: Vec<&Pane> = self.shown_ids().into_iter().filter_map(|id| self.pane(&id)).collect();
        // The menu last: drawn over the others.
        shown.extend(self.menu.as_ref().and_then(|(menu, _)| self.pane(menu)));
        let engines: Vec<_> =
            shown.iter().map(|pane| pane.engine.lock().unwrap_or_else(PoisonError::into_inner)).collect();
        if shown.iter().any(|pane| pane.broken.is()) {
            return false;
        }
        let notes: Vec<Option<String>> = shown.iter().map(|pane| note(pane)).collect();
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let mut views = self.pane_views(&shown, &engines, &notes, epoch.as_secs());
        common_forms(&mut views);
        // The cards' frame, under the menu if it is open (the menu is the last view).
        let glyphs = self.client.as_ref().map_or(Glyphs::Unicode, glyphs);
        let card_clock = board::Clock { now: secs as i64, frame: look::frame(self.pacing.born.elapsed()), glyphs };
        let panel = Panel { cards: &self.cards.data, wide: width >= layout::FOCUS_WIDE, clock: card_clock };
        let title = cards::title();
        if let Some(area) = self.cards_rect().filter(|_| !self.cards.data.is_empty()) {
            let at = views.len() - usize::from(self.menu.is_some());
            let header =
                chrome::Header { name: &title, hint: Some(cards::counts(&self.cards.data)), ..Default::default() };
            let frame = View { id: CARDS, engine: &panel, area, header, focused: false, selection: None, bare: false };
            views.insert(at, frame);
        }
        let tabs = self.tab_bar();
        // The spinner turns while a member at work shows, in a header or a tab.
        let working = views.iter().map(|view| view.header.state).chain(tabs.iter().map(|tab| tab.state));
        let working = working.into_iter().any(|state| state == look::State::Working)
            || views.iter().any(|view| view.id == CARDS)
                && self.cards.data.iter().any(|card| card.state == look::State::Working);
        let bar = Rect { x: 0, y: height.saturating_sub(1), width, height: usize::from(height > 0) };
        let look = chrome::Look { glyphs, frame: look::frame(self.pacing.born.elapsed()) };
        let team = self.team.as_str();
        let screen = &self.screen;
        // Over the team, the rest dimmed: a choice, else the menu (the last view).
        // A list under a header's text dims nothing: the team stays as it is around it.
        let over = match (&self.choice, &self.menu) {
            (Some(open), _) => screen::Over::Choice(&open.choice),
            (None, Some(_)) if self.list.is_none() => screen::Over::LastView,
            _ => screen::Over::Nothing,
        };
        let hover = match &self.pointer.hover {
            Some(Hover::Bar(target)) => Some(*target),
            _ => None,
        };
        let notice = self.notices.notice.as_ref().map(|(notice, _)| notice);
        let composed = crate::mux::caught(|| {
            let scene = Scene { team, tabs: &tabs, bar, hover, look, over, notice };
            screen.borrow_mut().compose(width, height, &views, &scene)
        });
        let covered = self.choice.is_some() || self.menu.is_some();
        let clock = if covered { None } else { self.next_tick(&views, epoch.subsec_nanos()) };
        let ids: Vec<String> = views.iter().map(|view| view.id.to_string()).collect();
        // The zones, or whether an engine is to blame.
        let zones = composed.map_err(|why| {
            let found = blame_engines(&shown, &views, (width, height));
            log!("screen: {why}");
            found
        });
        drop(views);
        drop(engines);
        drop(shown);
        drop(held);
        let zones = match zones {
            Ok(zones) => zones,
            Err(found) => {
                // No engine to blame: the screen would fail again at each frame. The next change asks again.
                if !found {
                    self.dirty = false;
                }
                return false;
            }
        };
        self.zones = zones.bar;
        self.parts =
            zones.parts.into_iter().filter_map(|(at, i, part)| Some((at, ids.get(i)?.clone(), part))).collect();
        self.notices.notice_zone = zones.notice;
        self.pacing.clock = clock;
        if let Some(open) = self.choice.as_mut() {
            open.zones = zones.options;
        }
        // Under a layer (the menu, a choice), the spinner shows dimmed: not worth a frame each `look::FRAME`.
        self.pacing.animate = (working && !covered).then(|| Instant::now() + look::FRAME);
        true
    }

    /// The cards' times in their states tick: `secs` is now, in seconds since the epoch.
    fn tick_cards(&mut self, secs: u64) {
        for (card, began) in self.cards.data.iter_mut().zip(&self.cards.epoch) {
            if let Some(began) = began {
                card.since = secs.saturating_sub(*began);
            }
        }
    }

    /// The panes `shown`, locked in `engines`, as the screen shows them; `epoch` is now, in seconds.
    fn pane_views<'a>(
        &'a self,
        shown: &[&'a Pane],
        engines: &'a [MutexGuard<'_, Box<dyn Engine>>],
        notes: &'a [Option<String>],
        epoch: u64,
    ) -> Vec<View<'a>> {
        let zoomed = self.tabs.get(self.active).and_then(|tab| tab.zoomed.as_deref());
        shown
            .iter()
            .zip(engines)
            .zip(notes)
            .map(|((pane, engine), note)| View {
                id: &pane.id,
                engine: engine.as_ref(),
                area: pane.area,
                header: self.header(pane, note.as_deref(), zoomed, epoch),
                focused: self.focus.as_deref() == Some(pane.id.as_str()),
                selection: self
                    .pointer
                    .selection
                    .as_ref()
                    .filter(|(selection, _)| selection.pane == pane.id)
                    .and_then(|(selection, _)| selection.range()),
                // The menu's list draws its own frame.
                bare: self.list.is_some() && self.menu.as_ref().is_some_and(|(menu, _)| *menu == pane.id),
            })
            .collect()
    }

    /// The tabs of the bar, each with its most urgent state.
    fn tab_bar(&self) -> Vec<chrome::Tab<'_>> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let states = tab.panes.iter().filter_map(|id| self.pane(id)).map(|pane| pane_state(pane, &self.looks));
                let zoomed = tab.zoomed.as_deref().and_then(|id| self.pane(id)).map(|pane| pane.member.as_str());
                chrome::Tab {
                    title: &tab.title,
                    active: i == self.active,
                    state: urgent(states),
                    zoomed,
                    switch: if i == self.active { self.switch(i) } else { None },
                }
            })
            .collect()
    }

    /// The next image when the first of the times shown changes (12s, 13s; 22m, 23m): under a minute, only those a
    /// header has room for (`shows_since` lays the header out: asked of these only); beyond, a wake-up a minute costs
    /// nothing worth asking. The cards' times change every second, the spinner aside. `nanos`: how far into its second
    /// now is.
    fn next_tick(&self, views: &[View<'_>], nanos: u32) -> Option<Instant> {
        let shows = |view: &View<'_>| {
            view.header.since.filter(|secs| *secs >= 60 || chrome::shows_since(&view.header, view.area.width))
        };
        let ticks = views.iter().filter_map(shows).map(|secs| tick(secs, nanos));
        let cards = self.cards.data.iter().filter(|_| views.iter().any(|view| view.id == CARDS));
        let ticks = ticks.chain(cards.map(|card| tick(card.since, nanos)));
        ticks.min().map(|after| Instant::now() + after)
    }
}

/// Which engine made the screen fail: each one asked alone what `compose` asks of it, the panes `shown` being the
/// views but the cards'. Whether one was found (it is marked broken).
fn blame_engines(shown: &[&Pane], views: &[View<'_>], size: (usize, usize)) -> bool {
    let mut found = false;
    for (pane, view) in shown.iter().zip(views.iter().filter(|view| view.id != CARDS)) {
        let mut alone = Canvas::new(size.0, size.1);
        let replay = crate::mux::caught(|| {
            view.engine.draw(&mut alone, view.inside());
            (view.engine.cursor(), view.engine.scrolled(), view.engine.history())
        });
        if let Err(why) = replay {
            pane.broken.mark(why);
            found = true;
        }
    }
    found
}

/// When the next frame is due, and what the server sent.
pub(super) struct Pacing {
    pub(super) last_frame: Option<Instant>,
    pub(super) typed: Option<Instant>,
    pub(super) echo: bool,
    /// The earliest deadline of the engines.
    pub(super) deadline: Option<Instant>,
    /// When the spinner of a member at work shown needs its next image; `None` when none is shown.
    pub(super) animate: Option<Instant>,
    /// When a time in a header shown next changes (12s, 13s; 22m, 23m); `None` when none shows.
    pub(super) clock: Option<Instant>,
    /// When the server started: the spinner's clock.
    pub(super) born: Instant,
    /// Frames sent to clients, and bytes written to them, since the server started: `_ctl stats`.
    pub(super) frames: u64,
    pub(super) written: u64,
}

impl Pacing {
    pub(super) fn new() -> Pacing {
        Pacing {
            last_frame: None,
            typed: None,
            echo: false,
            deadline: None,
            animate: None,
            clock: None,
            born: Instant::now(),
            frames: 0,
            written: 0,
        }
    }
}
