// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The menu's state, apart from the terminal and the team's files: the team as the menu shows it, where the user is
//! on the screen, and what each key or click does. What must touch the team, the terminal or another program comes
//! back as an [`Effect`], for `menu.rs` to carry out.

use std::collections::HashMap;

use crossterm::style::Color;

use crate::config::{self, Edit, Field, Layered, MAX_CONTACTS, Member, MemberLayers, Origin, Source, TeamLayers};
use crate::look::{self, State};
use crate::{bridge, i18n, t};

pub(crate) const MODELS: [(&str, &str); 4] =
    [("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku"), ("fable", "Fable")];
pub(crate) const MODES: [&str; 5] = ["default", "acceptEdits", "auto", "plan", "bypassPermissions"];

/// A member as the menu shows it.
#[derive(Clone, Debug)]
pub(crate) struct Person {
    pub name: String,
    pub contact: bool,
    /// Its own color, as on the dashboard.
    pub color: Color,
    /// Still running, but no longer in the team's files (taken out by hand).
    pub gone: bool,
    /// The model's family and the effort its requests go with now.
    pub model: Option<String>,
    pub effort: Option<String>,
    pub role: String,
    /// Its settings as the team's files give them; None for a member gone from them, or files that do not read.
    pub layers: Option<MemberLayers>,
    /// The tab it is in now: « Agents (2) ».
    pub tab_now: Option<String>,
    /// Its requests can carry another model or effort (the mod is loaded): else any change starts it again.
    pub live: bool,
}

/// The team as the menu shows it.
#[derive(Clone, Debug)]
pub(crate) struct Team {
    pub name: String,
    pub origin: Origin,
    pub people: Vec<Person>,
    /// The team as its files give it: who is in it, who the contacts are.
    pub config: config::Team,
    pub dashboard: bool,
    /// The team's other settings, with where each comes from; None while its files do not read.
    pub layers: Option<TeamLayers>,
    /// A client to detach: the one the menu was opened from.
    pub detach: bool,
    /// The file a new member goes into, as the user knows it.
    pub file: String,
    /// Why the team's files do not read: the members as launched, nothing to change but the team as a whole.
    pub unreadable: Option<String>,
}

impl Team {
    /// The tabs the members name, in their order.
    pub(crate) fn tabs(&self) -> Vec<String> {
        let mut tabs: Vec<String> = Vec::new();
        for tab in self.config.members.values().filter_map(|m| m.tab.as_ref()) {
            if !tabs.contains(tab) {
                tabs.push(tab.clone());
            }
        }
        tabs
    }

    fn person(&self, name: &str) -> Option<&Person> {
        self.people.iter().find(|p| p.name == name)
    }

    /// The people's places, the contacts first.
    pub(crate) fn order(&self) -> Vec<usize> {
        let contacts = (0..self.people.len()).filter(|&i| self.people[i].contact);
        contacts.chain((0..self.people.len()).filter(|&i| !self.people[i].contact)).collect()
    }
}

/// An entry of the list on the left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Member(String),
    New,
}

/// Where the arrows act.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pane {
    List,
    Sheet,
}

/// A row of the sheet on the right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fid {
    /// A member whose turn failed: « continue » sent to it, its prompt left as it is; nothing sent if the prompt holds
    /// a draft (mock-up C; architect, 2026-10-10).
    Resume,
    Model,
    Effort,
    Mode,
    Contact,
    Tab,
    Name,
    Role,
    Instructions,
    Restart,
    Remove,
    /// A member gone from the files: its pane closed.
    Close,
    /// The new agent: how it is made.
    How,
    Builtin,
    Request,
    Compose,
    NewName,
    NewRole,
    NewInstructions,
    Add,
}

impl Fid {
    /// Whether ←→ go through its values.
    pub(crate) fn cycles(self) -> bool {
        matches!(self, Fid::Model | Fid::Effort | Fid::Mode | Fid::Contact | Fid::Tab | Fid::How | Fid::Builtin)
    }
}

/// A member's setting that ←→ go through, read from its layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Model,
    Effort,
    Mode,
    Tab,
}

impl Setting {
    fn of(fid: Fid) -> Option<Setting> {
        match fid {
            Fid::Model => Some(Setting::Model),
            Fid::Effort => Some(Setting::Effort),
            Fid::Mode => Some(Setting::Mode),
            Fid::Tab => Some(Setting::Tab),
            _ => None,
        }
    }

    fn layered(self, layers: &MemberLayers) -> &Layered<Option<String>> {
        match self {
            Setting::Model => &layers.model,
            Setting::Effort => &layers.effort,
            Setting::Mode => &layers.permission_mode,
            Setting::Tab => &layers.tab,
        }
    }

    /// The values offered.
    fn values(self, team: &Team) -> Vec<String> {
        match self {
            Setting::Model => MODELS.iter().map(|(id, _)| id.to_string()).collect(),
            Setting::Effort => bridge::EFFORTS.iter().map(|e| e.to_string()).collect(),
            Setting::Mode => MODES.iter().map(|m| m.to_string()).collect(),
            Setting::Tab => team.tabs(),
        }
    }

    /// A value, as the user knows it: `opus` → `Opus`.
    fn label(self, value: &str) -> String {
        match self {
            Setting::Model => model_label(value),
            // Its sign and its name, as the user reads an effort everywhere (the files keep `xhigh`).
            Setting::Effort if bridge::EFFORTS.contains(&value) => look::effort_label(value),
            _ => value.to_string(),
        }
    }

    /// What the member has when nothing gives the setting: Claude Code's default, or recruit's tab.
    fn nobody(self, p: &Person) -> String {
        match self {
            Setting::Tab => p.tab_now.clone().unwrap_or_else(|| "Agents".into()),
            _ => t!("défaut de Claude Code", "Claude Code's default"),
        }
    }

    /// The choice back to the default when nothing under the member gives the setting.
    fn back(self) -> String {
        match self {
            Setting::Tab => t!("défaut : onglets « Agents »", "default: \"Agents\" tabs"),
            _ => t!("défaut de Claude Code", "Claude Code's default"),
        }
    }

    /// The choice that opens a value typed in place.
    fn other(self) -> Option<String> {
        match self {
            Setting::Model => Some(t!("Autre…", "Other…")),
            Setting::Tab => Some(t!("Nouvel onglet…", "New tab…")),
            _ => None,
        }
    }

    fn detail(self, value: &str) -> Option<String> {
        (self == Setting::Mode).then(|| mode_detail(value))
    }

    fn field(self, value: Option<String>) -> Field {
        match self {
            Setting::Model => Field::Model(value),
            Setting::Effort => Field::Effort(value),
            Setting::Mode => Field::PermissionMode(value),
            Setting::Tab => Field::Tab(value),
        }
    }

    /// What a change says once made, `restarted` when it started the member again.
    fn changed(self, name: &str, restarted: bool) -> String {
        let what = match self {
            Setting::Model => t!("Modèle de « {} » changé", "Model of \"{}\" changed", name),
            Setting::Effort => t!("Effort de « {} » changé", "Effort of \"{}\" changed", name),
            Setting::Mode => t!("Mode de permission de « {} » changé", "Permission mode of \"{}\" changed", name),
            Setting::Tab => return t!("« {} » a changé d'onglet.", "\"{}\" moved to another tab.", name),
        };
        let when = if restarted {
            t!(" : relancé sur sa conversation.", ": restarted on its conversation.")
        } else if self == Setting::Mode {
            ".".into()
        } else {
            t!(" : effet dès sa prochaine requête.", ": in effect from its next request.")
        };
        format!("{what}{when}")
    }
}

/// How a new agent is made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum How {
    #[default]
    Builtin,
    Composed,
    Typed,
}

/// A value to pick.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Choice {
    /// None: back to the default.
    pub value: Option<String>,
    pub label: String,
    /// More about it, beside it in a list.
    pub detail: Option<String>,
    /// Another one, typed.
    pub other: bool,
    /// Taking it starts the member again (known only in the quick list).
    pub restarts: bool,
    /// The effort level its label shows (sign and name), for its colors.
    pub level: Option<String>,
}

impl Choice {
    fn new(value: Option<&str>, label: impl Into<String>) -> Self {
        Choice {
            value: value.map(String::from),
            label: label.into(),
            detail: None,
            other: false,
            restarts: false,
            level: None,
        }
    }
}

/// A list of values open under a row.
#[derive(Clone, Debug)]
pub(crate) struct Dropdown {
    /// The row it opens under; None for a team setting, which opens above its button.
    pub field: Option<Fid>,
    pub items: Vec<Choice>,
    pub at: usize,
    /// The one in place now.
    pub current: Option<usize>,
    pub purpose: Purpose,
}

impl Dropdown {
    fn empty() -> Self {
        Dropdown { field: None, items: Vec::new(), at: 0, current: None, purpose: Purpose::Value }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Purpose {
    Value,
    /// The grid of the agents' tabs; the values are `columns x rows`.
    Grid,
    /// The share for the automatic compaction; the values are a percentage, or `off`.
    Compaction,
    /// Who becomes a contact in place of the only one, which leaves or becomes an agent.
    Replacement {
        leaving: String,
        removed: bool,
    },
}

/// A text typed in place, over the row it is for (the model's or the tab's: another one than those offered).
#[derive(Clone, Debug)]
pub(crate) struct Typing {
    pub field: Fid,
    pub text: String,
    /// In characters.
    pub cursor: usize,
    pub error: Option<String>,
}

/// A question asked before doing something that cannot be taken back, or interrupts someone.
#[derive(Clone, Debug)]
pub(crate) struct Confirm {
    pub title: String,
    pub lines: Vec<String>,
    /// Those it interrupts, said: those at work, those waiting for the user, those unknown; one sentence for each.
    pub busy: Vec<(State, String)>,
    pub yes: String,
    /// Red rather than cyan: it removes or stops.
    pub danger: bool,
    pub then: Vec<Effect>,
    /// The keys that answer « yes » and « cancel » (the quick list's restart question).
    pub letters: Option<(char, char)>,
    /// On « yes » (0) or « cancel » (1): « cancel » at first, as the old menu's « no ».
    pub at: usize,
}

impl Confirm {
    fn new(title: String, lines: Vec<String>, yes: String, then: Vec<Effect>) -> Self {
        Confirm { title, lines, busy: Vec::new(), yes, danger: false, then, letters: None, at: 1 }
    }

    fn busy(self, busy: Vec<(State, String)>) -> Self {
        Confirm { busy, ..self }
    }

    fn danger(self, danger: bool) -> Self {
        Confirm { danger, ..self }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Overlay {
    None,
    List(Dropdown),
    Typing(Typing),
    Confirm(Confirm),
}

/// What the last change did, or why it failed, or what the user is asked: at the bottom of the screen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Said {
    Done(String),
    Failed(String),
    Asked(String),
}

/// What a change says once made.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Done {
    Text(String),
    /// The contacts as they are once it is made.
    Contacts,
}

/// What a text in the editor is for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Edited {
    Instructions(String),
    Draft,
}

/// What the menu must carry out.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    Apply {
        edits: Vec<Edit>,
        said: Done,
        select: Option<String>,
    },
    Restart {
        member: String,
        fresh: bool,
        said: String,
    },
    RestartAll,
    Dismiss(String),
    Editor(Edited),
    /// Claude composes a new agent: request `id`, its result to come back with it.
    Compose {
        id: u64,
        request: String,
    },
    CancelCompose(u64),
    Detach,
    Stop,
    Quit,
    /// A member whose turn failed takes up its work: its mod sends « continue », unless its prompt holds a draft.
    Resume(String),
}

/// Where « Reprendre » stands for a member: asked, then what its mod did.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Resuming {
    Asked,
    Sent,
    /// Not sent: its prompt holds a draft.
    Draft,
    Failed(String),
}

/// What the sheet asks of the running team, without changing it.
pub(crate) trait Env {
    /// The members these edits would start again (`Running::restarts`).
    fn restarts(&self, edits: &[Edit]) -> Vec<String>;
    /// Whether `member` has exchanged anything on its conversation: renamed, it resumes it.
    fn has_conversation(&self, member: &str) -> bool;
}

/// A key, as the menu understands it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    EraseWord,
    EraseAll,
    Char(char),
    Paste(String),
}

/// The team's own actions, at the bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    New,
    Dashboard,
    /// A tab of members as a grid of panes, or as « focus ».
    Layout,
    /// The most panes a tab of working agents shows.
    Grid,
    /// The share of its window from which a member at rest is compacted.
    AutoCompact,
    RestartAll,
    Detach,
    Stop,
}

impl Action {
    pub(crate) const ALL: [Action; 8] = [
        Action::New,
        Action::Dashboard,
        Action::Layout,
        Action::Grid,
        Action::AutoCompact,
        Action::RestartAll,
        Action::Detach,
        Action::Stop,
    ];

    /// Whether it sets something of the team (rather than doing something): they go on their own row.
    pub(crate) fn is_setting(self) -> bool {
        matches!(self, Action::Dashboard | Action::Layout | Action::Grid | Action::AutoCompact)
    }

    /// Its key, the same in both languages.
    pub(crate) fn key(self) -> char {
        match self {
            Action::New => 'n',
            Action::Dashboard => 't',
            Action::Layout => 'l',
            Action::Grid => 'g',
            Action::AutoCompact => 'c',
            Action::RestartAll => 'R',
            Action::Detach => 'd',
            Action::Stop => 'q',
        }
    }
}

/// Where a click lands, as the drawing tells.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Target {
    Entry(Entry),
    /// A row of the sheet, by its place among those that take the focus.
    Row(usize),
    /// The ‹ (false) or › (true) of a row.
    Step(usize, bool),
    Action(Action),
    Item(usize),
    Yes,
    No,
    /// On its conversation (false) or a new one (true).
    Restart(bool),
}

/// The new agent, as its form stands.
#[derive(Clone, Debug, Default)]
pub(crate) struct Draft {
    pub how: How,
    /// The built-in role chosen, in `roles`.
    pub role_at: usize,
    pub name: String,
    /// Typed by the user: no longer follows the role chosen.
    pub named: bool,
    pub role: String,
    pub instructions: Option<String>,
    pub request: String,
    /// The request Claude composes now.
    pub composing: Option<u64>,
    pub composed: bool,
    /// The member the built-in role or Claude gives, but its name, role and instructions.
    pub base: Member,
}

/// The menu as a list only (`recruit _menu … --field f --list`): the values of one field of one member, under its
/// pane's header. It ends with the first answer; a refused write keeps it open.
#[derive(Clone, Debug)]
pub(crate) struct Quick {
    pub field: Fid,
    /// The list as opened, kept under a question or a text typed in place.
    pub list: Dropdown,
    /// Whether the values that start the member again are marked: asked of the files after the first image, never
    /// before it.
    pub marked: bool,
}

pub(crate) struct Sheet {
    pub quick: Option<Quick>,
    pub team: Team,
    /// Each member's state and since when (seconds since the epoch), as last seen.
    pub states: HashMap<String, (State, i64)>,
    /// Whether `states` says how things are: a member missing from it then runs no session. Unknown, any member may
    /// be at work.
    pub states_known: bool,
    /// The text of the error each member whose turn failed stopped on, whole, as its pane gave it.
    pub errors: HashMap<String, String>,
    /// « Reprendre » asked for a member, and how it went.
    pub resuming: Option<(String, Resuming)>,
    pub entry: Entry,
    pub pane: Pane,
    /// The focused row, among those that take the focus.
    pub row: usize,
    /// A value prepared with ←→, applied with ⏎: the row and the value's place among its choices.
    pub pending: Option<(Fid, usize)>,
    pub overlay: Overlay,
    pub said: Option<Said>,
    pub draft: Draft,
    /// The last request given to Claude to compose.
    composes: u64,
    /// For the restart row: on its conversation (false) or a new one (true).
    pub fresh: bool,
    /// The built-in roles a new agent can take, once its form opened.
    pub roles: Option<Vec<(String, Member)>>,
}

impl Sheet {
    pub(crate) fn new(team: Team) -> Self {
        let entry = team.order().first().map_or(Entry::New, |&i| Entry::Member(team.people[i].name.clone()));
        let said = team.unreadable.as_ref().map(|error| Said::Failed(unreadable(error)));
        Sheet {
            quick: None,
            team,
            states: HashMap::new(),
            states_known: false,
            errors: HashMap::new(),
            resuming: None,
            entry,
            pane: Pane::List,
            row: 0,
            pending: None,
            overlay: Overlay::None,
            said,
            draft: Draft::default(),
            composes: 0,
            fresh: false,
            roles: None,
        }
    }

    /// The team read again after a change: the same member chosen, or the first one when it left.
    pub(crate) fn reload(&mut self, team: Team, select: Option<String>) {
        self.team = team;
        if let Some(name) = select {
            self.entry = Entry::Member(name);
        }
        let missing = match &self.entry {
            Entry::Member(name) => self.team.person(name).is_none(),
            Entry::New => self.team.unreadable.is_some(),
        };
        if missing {
            self.entry =
                self.team.order().first().map_or(Entry::New, |&i| Entry::Member(self.team.people[i].name.clone()));
            self.pane = Pane::List;
        }
        if let Some(error) = &self.team.unreadable {
            self.said = Some(Said::Failed(unreadable(error)));
        }
        self.pending = None;
        self.clamp();
    }

    /// Opens on `member`'s sheet, its `field` focused when it has one; the first member as before when it is not in
    /// the team (`recruit _menu --member m --field f`: ⌥r on a member's pane, a click on a header's name, model or
    /// effort in recruit's own multiplexer).
    /// A member whose turn failed opens on « Reprendre », unless a field is asked for (mock-up C).
    pub(crate) fn open_on(&mut self, member: &str, field: Option<Fid>) {
        let Some(name) = self.team.person(member).map(|p| p.name.clone()) else { return };
        self.entry = Entry::Member(name);
        self.pane = Pane::List;
        self.row = 0;
        if let Some(row) =
            field.or(Some(Fid::Resume)).and_then(|field| self.focusable().iter().position(|f| *f == field))
        {
            self.pane = Pane::Sheet;
            self.row = row;
        }
        self.clamp();
    }

    /// The quick list: `member`'s `field` (its model or its effort), its values open. A member or a field that is
    /// not there leaves it closed, which ends it.
    pub(crate) fn open_list(&mut self, member: &str, field: Fid, env: &dyn Env) {
        if !matches!(field, Fid::Model | Fid::Effort) {
            return;
        }
        self.open_on(member, Some(field));
        if self.field() == Some(field) {
            self.quick = Some(Quick { field, list: Dropdown::empty(), marked: false });
            self.reopen_list(env);
        }
    }

    /// The quick list open again, on the member's values as they are now: after a write refused.
    pub(crate) fn reopen_list(&mut self, env: &dyn Env) {
        let Some((field, chosen)) = self.quick.as_ref().map(|q| (q.field, q.list.at)) else { return };
        self.overlay = Overlay::None;
        self.pending = None;
        self.enter(field, env);
        let Overlay::List(mut list) = std::mem::replace(&mut self.overlay, Overlay::None) else { return };
        // The cursor stays on the value chosen, not back on the one in place.
        list.at = chosen.min(list.items.len().saturating_sub(1));
        if let Some(quick) = self.quick.as_mut() {
            quick.list = list.clone();
            quick.marked = false;
        }
        self.overlay = Overlay::List(list);
    }

    /// Whether the quick list waits for its ↻ marks.
    pub(crate) fn unmarked(&self) -> bool {
        self.quick.as_ref().is_some_and(|q| !q.marked)
    }

    /// The values that start the member again, marked (↻): read from the files, so after the first image.
    pub(crate) fn mark_restarts(&mut self, env: &dyn Env) {
        let Some(field) = self.quick.as_ref().map(|q| q.field) else { return };
        let name = self.name();
        let Some((setting, _)) = self.setting(field) else { return };
        let marks = |list: &Dropdown| -> Vec<bool> {
            list.items
                .iter()
                .map(|item| !item.other && env.restarts(&self.edits(setting, item)).contains(&name))
                .collect()
        };
        let marks = match &self.quick {
            Some(quick) => marks(&quick.list),
            None => return,
        };
        let apply = |list: &mut Dropdown| {
            for (item, mark) in list.items.iter_mut().zip(&marks) {
                item.restarts = *mark;
            }
        };
        if let Some(quick) = self.quick.as_mut() {
            apply(&mut quick.list);
            quick.marked = true;
        }
        if let Overlay::List(list) = &mut self.overlay {
            apply(list);
        }
    }

    /// The errors the members whose turn failed stopped on, as last seen.
    pub(crate) fn failed(&mut self, errors: HashMap<String, String>) {
        self.errors = errors;
    }

    /// What `member`'s mod did with « Reprendre ».
    pub(crate) fn resumed(&mut self, member: &str, how: Resuming) {
        if self.resuming.as_ref().is_some_and(|(name, _)| name == member) {
            self.resuming = Some((member.to_string(), how));
        }
    }

    /// Whether `name`'s turn failed, as last seen.
    pub(crate) fn in_error(&self, name: &str) -> bool {
        self.state(name).is_some_and(|(state, _)| state == State::Error)
    }

    /// The states as last seen: None when they cannot be seen (`claude agents` fails, nothing written lately).
    pub(crate) fn seen(&mut self, states: Option<HashMap<String, (State, i64)>>) {
        self.states_known = states.is_some();
        self.states = states.unwrap_or_default();
    }

    /// The person chosen, if a member is.
    pub(crate) fn person(&self) -> Option<&Person> {
        match &self.entry {
            Entry::Member(name) => self.team.person(name),
            Entry::New => None,
        }
    }

    fn name(&self) -> String {
        self.person().map(|p| p.name.clone()).unwrap_or_default()
    }

    /// The list's entries: the contacts, then the agents, then the new agent (not while the files do not read).
    pub(crate) fn entries(&self) -> Vec<Entry> {
        let mut entries: Vec<Entry> =
            self.team.order().into_iter().map(|i| Entry::Member(self.team.people[i].name.clone())).collect();
        if self.team.unreadable.is_none() {
            entries.push(Entry::New);
        }
        entries
    }

    /// The team's actions that can be taken now: the team as a whole only while its files do not read.
    pub(crate) fn actions(&self) -> Vec<Action> {
        Action::ALL
            .into_iter()
            .filter(|a| *a != Action::Detach || self.team.detach)
            .filter(|a| self.team.unreadable.is_none() || matches!(a, Action::Detach | Action::Stop))
            .filter(|a| self.team.layers.is_some() || !matches!(a, Action::Layout | Action::Grid | Action::AutoCompact))
            // One tab per member: no grid, no focus to set.
            .filter(|a| {
                !matches!(a, Action::Grid | Action::Layout)
                    || self.team.layers.as_ref().is_none_or(|l| l.layout.value != config::Layout::Tabs)
            })
            .collect()
    }

    /// The rows of the sheet on the right; None between groups.
    pub(crate) fn rows(&self) -> Vec<Option<Fid>> {
        if self.team.unreadable.is_some() {
            return Vec::new();
        }
        match self.person() {
            Some(p) if p.gone => vec![Some(Fid::Close)],
            Some(p) => {
                let mut rows = Vec::new();
                // Its turn failed: taking up its work first (mock-up C).
                if self.in_error(&p.name) && p.layers.is_some() {
                    rows.extend([Some(Fid::Resume), None]);
                }
                rows.extend([Some(Fid::Model), Some(Fid::Effort), Some(Fid::Mode), None, Some(Fid::Contact)]);
                // A contact is in the first tab, whatever its own.
                if !p.contact {
                    rows.push(Some(Fid::Tab));
                }
                rows.extend([None, Some(Fid::Name), Some(Fid::Role), Some(Fid::Instructions), None]);
                rows.extend([Some(Fid::Restart), Some(Fid::Remove)]);
                rows
            }
            None => {
                let mut rows = vec![Some(Fid::How), None];
                match self.draft.how {
                    How::Builtin => rows.extend([Some(Fid::Builtin), Some(Fid::NewName)]),
                    How::Composed => {
                        rows.extend([Some(Fid::Request), Some(Fid::Compose)]);
                        if self.draft.composed {
                            rows.extend([None, Some(Fid::NewName), Some(Fid::NewRole), Some(Fid::NewInstructions)]);
                        }
                    }
                    How::Typed => rows.extend([Some(Fid::NewName), Some(Fid::NewRole), Some(Fid::NewInstructions)]),
                }
                rows.extend([None, Some(Fid::Add)]);
                rows
            }
        }
    }

    /// The rows that take the focus.
    pub(crate) fn focusable(&self) -> Vec<Fid> {
        self.rows().into_iter().flatten().collect()
    }

    /// The focused row, when the sheet has the focus.
    pub(crate) fn field(&self) -> Option<Fid> {
        (self.pane == Pane::Sheet).then(|| self.focusable().get(self.row).copied()).flatten()
    }

    fn clamp(&mut self) {
        let count = self.focusable().len();
        if self.row >= count {
            self.row = count.saturating_sub(1);
        }
    }

    // -- What each row shows ------------------------------------------------------------------------------------

    /// The setting `fid` of the chosen member, with its layers.
    fn setting(&self, fid: Fid) -> Option<(Setting, &Layered<Option<String>>)> {
        let setting = Setting::of(fid)?;
        Some((setting, setting.layered(self.person()?.layers.as_ref()?)))
    }

    /// The values a row goes through, `all` with the one that opens another, typed, for a list.
    pub(crate) fn choices(&self, fid: Fid, all: bool) -> Vec<Choice> {
        match fid {
            Fid::How => vec![
                Choice::new(Some("builtin"), t!("Un rôle intégré", "A built-in role")),
                Choice::new(Some("composed"), t!("Claude le compose", "Claude composes it")),
                Choice::new(Some("typed"), t!("Je le saisis", "I type it")),
            ],
            Fid::Builtin => self
                .roles
                .iter()
                .flatten()
                .map(|(name, member)| Choice {
                    detail: Some(first_line(&member.role, 80)),
                    ..Choice::new(Some(name), name.clone())
                })
                .collect(),
            Fid::Contact if self.person().is_some() => {
                vec![Choice::new(Some("yes"), t!("oui", "yes")), Choice::new(Some("no"), t!("non", "no"))]
            }
            _ => {
                let Some((setting, layered)) = self.setting(fid) else { return Vec::new() };
                // The default's value is the default: listed apart, it would be written as nothing.
                let fallback = layered.fallback.as_ref().map(|(value, _)| value.as_deref());
                let is_default = |value: &str| fallback == Some(Some(value));
                let mut list = Vec::new();
                if let Some(text) = self.default_text(setting, layered) {
                    // An effort's default says its level, in its colors: the layers' or what Claude Code applies.
                    let level = (setting == Setting::Effort)
                        .then(|| match fallback {
                            Some(Some(value)) => Some(value.to_string()),
                            _ => self.person().and_then(|p| p.effort.clone()),
                        })
                        .flatten()
                        .filter(|level| bridge::EFFORTS.contains(&level.as_str()));
                    list.push(Choice { level, ..Choice::new(None, text) });
                }
                let values = setting.values(&self.team);
                let current = layered.value.clone().filter(|v| !values.contains(v));
                for value in values.iter().chain(current.iter()).filter(|v| !is_default(v)) {
                    list.push(Choice {
                        detail: setting.detail(value),
                        level: (setting == Setting::Effort && bridge::EFFORTS.contains(&value.as_str()))
                            .then(|| value.clone()),
                        ..Choice::new(Some(value), setting.label(value))
                    });
                }
                if all && let Some(other) = setting.other() {
                    list.push(Choice { other: true, ..Choice::new(None, other) });
                }
                list
            }
        }
    }

    /// The choice that gives a setting back to its default: what the member then has, and where it comes from.
    fn default_text(&self, setting: Setting, layered: &Layered<Option<String>>) -> Option<String> {
        let (value, source) = layered.fallback.as_ref()?;
        Some(match value {
            Some(value) => t!("défaut : {} ({})", "default: {} ({})", setting.label(value), self.source_name(*source)),
            // No layer gives one: what Claude Code applies, as the member's mod reported it, when the member runs on it.
            None => match (setting, layered.value.is_none(), self.person()) {
                (Setting::Model, true, Some(Person { model: Some(model), .. })) => {
                    t!("défaut : {} (Claude Code)", "default: {} (Claude Code)", model)
                }
                (Setting::Effort, true, Some(Person { effort: Some(effort), .. })) => {
                    t!("défaut : {} (Claude Code)", "default: {} (Claude Code)", setting.label(effort))
                }
                _ => setting.back(),
            },
        })
    }

    /// The place of the value in place now among a row's choices.
    pub(crate) fn current(&self, fid: Fid) -> Option<usize> {
        let choices = self.choices(fid, false);
        let wanted: Option<String> = match fid {
            Fid::How => Some(
                match self.draft.how {
                    How::Builtin => "builtin",
                    How::Composed => "composed",
                    How::Typed => "typed",
                }
                .into(),
            ),
            Fid::Builtin => return (!choices.is_empty()).then_some(self.draft.role_at.min(choices.len() - 1)),
            Fid::Contact => Some(if self.person()?.contact { "yes" } else { "no" }.into()),
            _ => {
                let (_, layered) = self.setting(fid)?;
                // The value the layers under the member give: the default.
                if layered.fallback.as_ref().is_some_and(|(value, _)| *value == layered.value) {
                    return choices.iter().position(|c| c.value.is_none());
                }
                layered.value.clone()
            }
        };
        choices.iter().position(|c| c.value == wanted && !c.other)
    }

    /// What a row shows as its value: the one prepared, if any.
    pub(crate) fn value(&self, fid: Fid) -> String {
        if let Some((field, at)) = self.pending
            && field == fid
            && let Some(choice) = self.choices(fid, false).get(at)
        {
            return choice.label.clone();
        }
        let Some(p) = self.person() else {
            return match fid {
                Fid::How | Fid::Builtin => self
                    .current(fid)
                    .and_then(|i| self.choices(fid, false).get(i).map(|c| c.label.clone()))
                    .unwrap_or_default(),
                Fid::Request => self.draft.request.clone(),
                Fid::NewName => self.draft.name.clone(),
                Fid::NewRole => first_line(&self.draft.role, 200),
                Fid::NewInstructions => instructions_count(self.draft.instructions.as_deref()),
                _ => String::new(),
            };
        };
        if let Some((setting, layered)) = self.setting(fid) {
            // No layer gives one: what Claude Code applies, when it said (as the default's line of the list does).
            return layered.value.as_deref().map_or_else(
                || self.applied(setting, p).map_or_else(|| setting.nobody(p), |value| setting.label(&value)),
                |v| setting.label(v),
            );
        }
        if fid == Fid::Resume {
            return match self.resuming.as_ref().filter(|(name, _)| *name == p.name).map(|(_, how)| how) {
                None => t!("lui envoyer « continue »", "send it “continue”"),
                Some(Resuming::Asked) => t!("envoi de « continue »…", "sending “continue”…"),
                Some(Resuming::Sent) => t!("« continue » envoyé", "“continue” sent"),
                Some(Resuming::Draft) => {
                    t!("pas envoyé : sa saisie contient du texte", "not sent: its prompt holds a draft")
                }
                Some(Resuming::Failed(error)) => t!("pas envoyé : {}", "not sent: {}", error),
            };
        }
        let Some(layers) = &p.layers else { return String::new() };
        match fid {
            Fid::Contact if p.contact => t!("oui", "yes"),
            Fid::Contact => t!("non", "no"),
            Fid::Name => p.name.clone(),
            Fid::Role => first_line(&layers.role.value, 200),
            Fid::Instructions => instructions_count(layers.instructions.value.as_deref()),
            _ => String::new(),
        }
    }

    /// What Claude Code applies to a member for `setting` when no layer gives it, as its mod reported it: its model's
    /// family, an effort's level; None when it did not say yet or for another setting.
    fn applied(&self, setting: Setting, p: &Person) -> Option<String> {
        match setting {
            Setting::Model => p.model.clone(),
            Setting::Effort => p.effort.clone().filter(|e| bridge::EFFORTS.contains(&e.as_str())),
            _ => None,
        }
    }

    /// The effort level the sheet's row shows, for its colors: the one prepared, else the one a layer gives, else the
    /// one Claude Code applies.
    pub(crate) fn effort_level(&self) -> Option<String> {
        let known = |level: &String| bridge::EFFORTS.contains(&level.as_str());
        if let Some((Fid::Effort, at)) = self.pending {
            return self.choices(Fid::Effort, false).get(at).and_then(|c| c.level.clone());
        }
        let p = self.person()?;
        let layered = &p.layers.as_ref()?.effort;
        layered
            .value
            .clone()
            .filter(known)
            .or_else(|| layered.value.is_none().then(|| p.effort.clone()).flatten().filter(known))
    }

    /// Where a row's value comes from, as the user knows the file or the layer.
    pub(crate) fn origin(&self, fid: Fid) -> Option<String> {
        if let Some((setting, layered)) = self.setting(fid) {
            if setting == Setting::Tab && layered.value.is_none() {
                return Some(t!("défaut de recruit", "recruit's default"));
            }
            // What Claude Code applies, known: it is where the value comes from.
            if layered.value.is_none() && self.person().and_then(|p| self.applied(setting, p)).is_some() {
                return Some("Claude Code".to_string());
            }
            return Some(self.source_name(layered.source));
        }
        let layers = self.person()?.layers.as_ref()?;
        let source = match fid {
            Fid::Contact => layers.contact.source,
            Fid::Name | Fid::Role => layers.role.source,
            Fid::Instructions if layers.instructions.value.is_some() => layers.instructions.source,
            _ => return None,
        };
        Some(self.source_name(source))
    }

    pub(crate) fn source_name(&self, source: Source) -> String {
        source_name(source, &self.team.origin)
    }

    /// The line under the sheet: what the focused row's default gives, and when a change takes effect.
    pub(crate) fn hint(&self) -> Option<String> {
        if let Overlay::Typing(typing) = &self.overlay {
            return Some(match typing.field {
                Fid::Name => t!(
                    "renommer relance « {} » sur sa conversation",
                    "renaming restarts \"{}\" on its conversation",
                    self.name()
                ),
                Fid::Model => t!("un alias, ou un identifiant claude-…", "an alias, or a claude-… id"),
                Fid::Tab => t!("le nom du nouvel onglet", "the new tab's name"),
                Fid::Request => t!("ce que doit faire le nouvel agent", "what the new agent should do"),
                Fid::NewName => t!("lettres, chiffres, - et _", "letters, digits, - and _"),
                _ => t!("une ligne, vue par ses coéquipiers", "one line, seen by teammates"),
            });
        }
        let fid = self.field()?;
        let pending = self.pending.is_some_and(|(f, _)| f == fid);
        let p = self.person();
        let restarts = t!("il sera relancé sur sa conversation", "it restarts on its conversation");
        if let Some((setting, layered)) = self.setting(fid) {
            let default = self.default_text(setting, layered).unwrap_or_default();
            let when = match setting {
                _ if pending => {
                    return Some(t!("⏎ applique ; {}", "⏎ applies it; {}", self.pending_effect(fid)));
                }
                Setting::Model | Setting::Effort if p.is_some_and(|p| p.live) => {
                    t!("effet dès sa prochaine requête", "in effect from its next request")
                }
                Setting::Model | Setting::Effort | Setting::Mode => restarts,
                Setting::Tab => t!("⏎ ouvre la liste, avec un nouvel onglet", "⏎ opens the list, with a new tab"),
            };
            return Some(format!("{default} · {when}"));
        }
        Some(match fid {
            Fid::Resume => t!(
                "Lui envoie « continue » ; il reprend où il s'est arrêté. Rien n'est envoyé si sa saisie contient du texte.",
                "Sends it “continue”; it picks up where it stopped. Nothing is sent if its prompt holds a draft."
            ),
            Fid::Contact => {
                let default = p
                    .and_then(|p| p.layers.as_ref())
                    .and_then(|l| l.contact.fallback.as_ref())
                    .map(|(on, _)| if *on { t!("oui", "yes") } else { t!("non", "no") });
                let what = t!(
                    "oui : il passe dans l'onglet « Interlocuteurs » ; deux au plus",
                    "yes: it moves to the \"Contacts\" tab; two at most"
                );
                match default {
                    Some(default) => format!("{} · {what}", t!("défaut : {}", "default: {}", default)),
                    None => what,
                }
            }
            Fid::Name => {
                t!("⏎ pour renommer ; relance sur sa conversation", "⏎ to rename; restarts on its conversation")
            }
            Fid::Role => t!(
                "⏎ pour éditer ici ; passe par une note à son prochain message",
                "⏎ to edit here; goes with a note on its next message"
            ),
            Fid::Instructions => t!(
                "⏎ ouvre l'éditeur ; passe par une note à son prochain message",
                "⏎ opens the editor; goes with a note on its next message"
            ),
            Fid::Restart => t!(
                "sur sa conversation : il reprend où il en était ; à neuf : son contexte est vidé",
                "on its conversation: it goes on where it was; afresh: its context is cleared"
            ),
            Fid::Remove => t!(
                "son panneau se ferme et ses réglages quittent les fichiers de l'équipe",
                "its pane closes and its settings leave the team's files"
            ),
            Fid::Close => t!("sa session s'arrête", "its session stops"),
            Fid::How => t!("←→ change de façon", "←→ changes how"),
            Fid::Builtin => {
                t!("←→ parcourt les rôles ; ⏎ les montre tous", "←→ goes through the roles; ⏎ shows them all")
            }
            Fid::Request => t!("⏎ pour écrire la demande", "⏎ to write the request"),
            Fid::Compose => t!("Claude lit le projet : une minute environ", "Claude reads the project: about a minute"),
            Fid::NewName => t!("⏎ pour le changer", "⏎ to change it"),
            Fid::NewRole => {
                t!("⏎ pour l'écrire ; une ligne, vue par ses coéquipiers", "⏎ to write it; one line, seen by teammates")
            }
            Fid::NewInstructions => t!("⏎ ouvre l'éditeur ; facultatif", "⏎ opens the editor; optional"),
            Fid::Add => t!(
                "il ira dans {}, le fichier qui définit l'équipe",
                "it goes into {}, the file that defines the team",
                self.team.file
            ),
            _ => return None,
        })
    }

    /// What applying the value prepared on `fid` does.
    fn pending_effect(&self, fid: Fid) -> String {
        match fid {
            Fid::Tab => t!("son panneau change de fenêtre", "its pane moves to another window"),
            Fid::Contact => t!("il change d'onglet", "it moves to another tab"),
            _ => t!("il sera relancé sur sa conversation", "it restarts on its conversation"),
        }
    }

    // -- States -------------------------------------------------------------------------------------------------

    pub(crate) fn state(&self, name: &str) -> Option<(State, i64)> {
        self.states.get(name).copied()
    }

    /// Of `members`, those a change would interrupt, in a sentence for each: those at work, whose task stops; those
    /// waiting for the user (a permission, a plan), whose request goes; and, while states cannot be seen, all of
    /// them, who may be at work. Empty when none is.
    fn interrupted(&self, members: &[String]) -> Vec<(State, String)> {
        let of = |state: State| -> Vec<String> {
            members.iter().filter(|m| self.state(m).is_some_and(|(s, _)| s == state)).cloned().collect()
        };
        let unknown = if self.states_known { Vec::new() } else { members.to_vec() };
        let groups: [(State, Vec<String>, Sentence, Sentence); 3] = [
            (
                State::Working,
                of(State::Working),
                |q| {
                    t!(
                        "{} est au travail : sa tâche en cours sera interrompue.",
                        "{} is at work: its current task will be interrupted.",
                        q
                    )
                },
                |q| {
                    t!(
                        "{} sont au travail : leurs tâches en cours seront interrompues.",
                        "{} are at work: their current tasks will be interrupted.",
                        q
                    )
                },
            ),
            (
                State::Waiting,
                of(State::Waiting),
                |q| {
                    t!(
                        "{} attend ta réponse (permission ou plan) : sa demande sera interrompue.",
                        "{} is waiting for your answer (permission or plan): its request will be interrupted.",
                        q
                    )
                },
                |q| {
                    t!(
                        "{} attendent ta réponse (permission ou plan) : leurs demandes seront interrompues.",
                        "{} are waiting for your answer (permission or plan): their requests will be interrupted.",
                        q
                    )
                },
            ),
            (
                State::Other,
                unknown,
                |q| t!("{} : état inconnu, peut-être au travail.", "{}: state unknown, perhaps at work.", q),
                |q| t!("{} : états inconnus, peut-être au travail.", "{}: states unknown, perhaps at work.", q),
            ),
        ];
        groups
            .into_iter()
            .filter(|(_, names, _, _)| !names.is_empty())
            .map(|(state, names, one, many)| {
                let said = if names.len() == 1 { one(&quoted(&names)) } else { many(&quoted(&names)) };
                (state, said)
            })
            .collect()
    }

    // -- Keys and clicks ----------------------------------------------------------------------------------------

    pub(crate) fn key(&mut self, key: Key, env: &dyn Env) -> Vec<Effect> {
        let effects = self.key_in(key, env);
        self.settled(effects)
    }

    /// In the quick list, the first answer ends it: a list, a question or a text given up, with nothing to carry out.
    fn settled(&mut self, effects: Vec<Effect>) -> Vec<Effect> {
        if self.quick.is_some() && effects.is_empty() && matches!(self.overlay, Overlay::None) {
            return vec![Effect::Quit];
        }
        effects
    }

    fn key_in(&mut self, key: Key, env: &dyn Env) -> Vec<Effect> {
        match std::mem::replace(&mut self.overlay, Overlay::None) {
            Overlay::Confirm(confirm) => return self.confirm_key(confirm, key),
            Overlay::List(list) => return self.list_key(list, key, env),
            Overlay::Typing(typing) => return self.typing_key(typing, key, env),
            Overlay::None => {}
        }
        // The team's keys from the list only: in the sheet, a letter typed for a text not yet open does nothing.
        if self.pane == Pane::List
            && let Key::Char(c) = key
            && let Some(action) = self.actions().into_iter().find(|a| a.key() == c)
        {
            return self.action(action);
        }
        match self.pane {
            Pane::List => self.list_pane_key(key),
            Pane::Sheet => self.sheet_key(key, env),
        }
    }

    fn list_pane_key(&mut self, key: Key) -> Vec<Effect> {
        let entries = self.entries();
        if entries.is_empty() {
            return if key == Key::Esc { vec![Effect::Quit] } else { Vec::new() };
        }
        let at = entries.iter().position(|e| *e == self.entry).unwrap_or(0);
        match key {
            Key::Up => self.choose(entries[at.saturating_sub(1)].clone()),
            Key::Down => self.choose(entries[(at + 1).min(entries.len() - 1)].clone()),
            Key::Home | Key::PageUp => self.choose(entries[0].clone()),
            Key::End | Key::PageDown => self.choose(entries[entries.len() - 1].clone()),
            Key::Enter | Key::Right | Key::Tab if !self.focusable().is_empty() => {
                self.pane = Pane::Sheet;
                self.row = 0;
                self.opened();
            }
            Key::Esc => return vec![Effect::Quit],
            _ => {}
        }
        Vec::new()
    }

    /// An entry chosen in the list, again or another: the value prepared is given up.
    pub(crate) fn choose(&mut self, entry: Entry) {
        if entry != self.entry {
            self.entry = entry;
            self.row = 0;
            self.fresh = false;
        }
        self.pending = None;
        self.opened();
    }

    /// The new agent's form opened: the built-in roles at hand, the first one's name.
    fn opened(&mut self) {
        if self.entry != Entry::New || self.roles.is_some() {
            return;
        }
        let taken = |name: &str| self.team.config.members.keys().any(|n| n.to_lowercase() == name.to_lowercase());
        let roles: Vec<(String, Member)> = crate::templates::Templates::for_lang(i18n::lang())
            .members()
            .into_iter()
            .filter(|(name, _)| !taken(name))
            .collect();
        self.roles = Some(roles);
        self.follow_role();
    }

    /// The draft takes the built-in role chosen: its name, unless typed, its role and instructions.
    fn follow_role(&mut self) {
        if self.draft.how != How::Builtin {
            return;
        }
        let Some((name, member)) = self.roles.as_ref().and_then(|r| r.get(self.draft.role_at)).cloned() else { return };
        if !self.draft.named {
            self.draft.name = name;
        }
        self.draft.role = member.role.clone();
        self.draft.instructions = member.instructions.clone();
        self.draft.base = member;
    }

    fn sheet_key(&mut self, key: Key, env: &dyn Env) -> Vec<Effect> {
        let rows = self.focusable();
        let Some(&fid) = rows.get(self.row) else {
            self.pane = Pane::List;
            return Vec::new();
        };
        match key {
            Key::Up | Key::Down | Key::Home | Key::End | Key::PageUp | Key::PageDown => {
                let last = rows.len() - 1;
                self.row = match key {
                    Key::Up => self.row.saturating_sub(1),
                    Key::Down => (self.row + 1).min(last),
                    Key::Home | Key::PageUp => 0,
                    _ => last,
                };
                // Another row: the value prepared is given up.
                self.pending = None;
            }
            Key::Left | Key::Right => return self.step(fid, key == Key::Right, env),
            Key::Enter => return self.enter(fid, env),
            Key::Esc if self.pending.take().is_some() => {}
            Key::Esc if self.draft.composing.is_some() => return self.stop_composing(),
            Key::Esc => self.pane = Pane::List,
            _ => {}
        }
        Vec::new()
    }

    /// The composition under way given up.
    fn stop_composing(&mut self) -> Vec<Effect> {
        self.draft.composing.take().map(Effect::CancelCompose).into_iter().collect()
    }

    /// ←→ on a row: its next value, applied at once only when the change is not seen and needs no restart (a model
    /// or an effort its requests carry); else prepared, for ⏎.
    fn step(&mut self, fid: Fid, forward: bool, env: &dyn Env) -> Vec<Effect> {
        if fid == Fid::Restart {
            self.fresh = forward;
            return Vec::new();
        }
        let choices = if fid.cycles() { self.choices(fid, false) } else { Vec::new() };
        if choices.is_empty() {
            return Vec::new();
        }
        let from = self.pending.filter(|(f, _)| *f == fid).map(|(_, at)| at).or_else(|| self.current(fid)).unwrap_or(0);
        let at = if forward { (from + 1) % choices.len() } else { (from + choices.len() - 1) % choices.len() };
        match fid {
            Fid::How => {
                // Another way: what the one before gave goes, Claude's composing too; the request is kept.
                let effects = self.stop_composing();
                let request = std::mem::take(&mut self.draft.request);
                let role_at = self.draft.role_at;
                self.draft =
                    Draft { how: [How::Builtin, How::Composed, How::Typed][at], request, role_at, ..Draft::default() };
                self.follow_role();
                return effects;
            }
            Fid::Builtin => {
                self.draft.role_at = at;
                self.follow_role();
                return Vec::new();
            }
            _ => {}
        }
        if self.current(fid) == Some(at) {
            self.pending = None;
            return Vec::new();
        }
        if let Some((setting @ (Setting::Model | Setting::Effort), _)) = self.setting(fid) {
            let edits = self.edits(setting, &choices[at]);
            let name = self.name();
            if !env.restarts(&edits).contains(&name) {
                self.pending = None;
                return vec![Effect::Apply { edits, said: Done::Text(setting.changed(&name, false)), select: None }];
            }
        }
        self.pending = Some((fid, at));
        Vec::new()
    }

    /// The edits that give the chosen member `choice` for `setting`.
    fn edits(&self, setting: Setting, choice: &Choice) -> Vec<Edit> {
        vec![Edit::Set { member: self.name(), field: setting.field(choice.value.clone()) }]
    }

    /// ⏎ on a row.
    fn enter(&mut self, fid: Fid, env: &dyn Env) -> Vec<Effect> {
        if let Some((field, at)) = self.pending.take()
            && field == fid
        {
            let choice = self.choices(fid, false)[at].clone();
            return self.apply_choice(fid, &choice, env);
        }
        match fid {
            Fid::Resume => {
                let name = self.name();
                // Once asked, not again while its mod has not answered.
                if self.resuming.as_ref().is_some_and(|(asked, how)| *asked == name && *how == Resuming::Asked) {
                    return Vec::new();
                }
                self.resuming = Some((name.clone(), Resuming::Asked));
                return vec![Effect::Resume(name)];
            }
            Fid::Model | Fid::Effort | Fid::Mode | Fid::Tab | Fid::Contact | Fid::Builtin => {
                let items = self.choices(fid, true);
                if items.is_empty() {
                    return Vec::new();
                }
                let current = self.current(fid);
                let at = current.unwrap_or(0);
                self.overlay =
                    Overlay::List(Dropdown { field: Some(fid), items, at, current, purpose: Purpose::Value });
            }
            Fid::How => {}
            Fid::Name => self.type_in(Fid::Name, &self.name()),
            Fid::Role => {
                let role = self.person().and_then(|p| p.layers.as_ref()).map(|l| l.role.value.trim().to_string());
                self.type_in(Fid::Role, &role.unwrap_or_default())
            }
            Fid::Instructions => return vec![Effect::Editor(Edited::Instructions(self.name()))],
            Fid::Restart => return self.restart(),
            Fid::Remove => return self.remove(),
            Fid::Close => {
                let name = self.name();
                let confirm = Confirm::new(
                    t!("Fermer le panneau de « {} » ?", "Close the pane of \"{}\"?", name),
                    vec![t!(
                        "« {} » n'est plus dans les fichiers de l'équipe, mais son panneau tourne encore. Sa session s'arrête.",
                        "\"{}\" is no longer in the team's files, but its pane still runs. Its session stops.",
                        name
                    )],
                    t!("Fermer son panneau", "Close its pane"),
                    vec![Effect::Dismiss(name.clone())],
                );
                self.overlay = Overlay::Confirm(confirm.busy(self.interrupted(&[name])).danger(true));
            }
            Fid::Request => self.type_in(Fid::Request, &self.draft.request.clone()),
            Fid::Compose if self.draft.composing.is_some() => {}
            Fid::Compose if self.draft.request.trim().is_empty() => self.type_in(Fid::Request, ""),
            Fid::Compose => return self.compose(),
            Fid::NewName => self.type_in(Fid::NewName, &self.draft.name.clone()),
            Fid::NewRole => self.type_in(Fid::NewRole, &self.draft.role.clone()),
            Fid::NewInstructions => return vec![Effect::Editor(Edited::Draft)],
            Fid::Add => return self.add(),
        }
        Vec::new()
    }

    /// Claude asked to compose the draft's request, as a new request.
    fn compose(&mut self) -> Vec<Effect> {
        self.composes += 1;
        self.draft.composing = Some(self.composes);
        vec![Effect::Compose { id: self.composes, request: self.draft.request.trim().to_string() }]
    }

    /// A value picked, in a list or prepared: written, once confirmed when it interrupts someone.
    fn apply_choice(&mut self, fid: Fid, choice: &Choice, env: &dyn Env) -> Vec<Effect> {
        match fid {
            Fid::Contact => return self.contact(choice.value.as_deref() == Some("yes"), env),
            Fid::Builtin => {
                if let Some(at) = self.choices(fid, false).iter().position(|c| c.value == choice.value) {
                    self.draft.role_at = at;
                    self.follow_role();
                }
                return Vec::new();
            }
            _ => {}
        }
        let Some((setting, _)) = self.setting(fid) else { return Vec::new() };
        if choice.other {
            self.type_in(fid, "");
            return Vec::new();
        }
        let edits = self.edits(setting, choice);
        let restarts = env.restarts(&edits);
        let said = Done::Text(setting.changed(&self.name(), restarts.contains(&self.name())));
        self.confirmed(edits, &restarts, said, None, fid)
    }

    /// `edits` to apply, asked first when they start again a member who may be interrupted.
    fn confirmed(
        &mut self,
        edits: Vec<Edit>,
        restarts: &[String],
        said: Done,
        select: Option<String>,
        fid: Fid,
    ) -> Vec<Effect> {
        let effect = Effect::Apply { edits, said, select };
        let mut busy = self.interrupted(restarts);
        if busy.is_empty() {
            return vec![effect];
        }
        let why = match fid {
            Fid::Mode => {
                t!(
                    "Le mode de permission ne change qu'à la relance :",
                    "The permission mode changes only at a restart:"
                )
            }
            Fid::Name => {
                t!(
                    "Un membre renommé repart sur sa conversation :",
                    "A renamed member starts again on its conversation:"
                )
            }
            Fid::Contact => {
                t!(
                    "Les interlocuteurs changent de barre en repartant :",
                    "The contacts change bars as they start again:"
                )
            }
            _ => t!("Ce changement ne passe pas par ses requêtes :", "This change cannot go through its requests:"),
        };
        let title = match restarts {
            [one] => t!("Relancer « {} » ?", "Restart \"{}\"?", one),
            _ => t!("Relancer {} ?", "Restart {}?", quoted(restarts)),
        };
        let again = t!("{} repartira sur sa conversation.", "{} starts again on its conversation.", quoted(restarts));
        let mut confirm = Confirm::new(title, vec![why, again], t!("Relancer maintenant", "Restart now"), vec![effect]);
        if self.quick.is_some() {
            // In the quick list: one note, short buttons, each with its key.
            let note = match busy.first().map(|(state, _)| *state) {
                Some(State::Waiting) => t!(
                    "Il attend ta réponse : sa demande sera interrompue. Il repartira sur sa conversation.",
                    "It waits for your answer: its request will be interrupted. It starts again on its conversation."
                ),
                Some(State::Working) => t!(
                    "Il est au travail : sa tâche en cours sera interrompue. Il repartira sur sa conversation.",
                    "It is at work: its current task will be interrupted. It starts again on its conversation."
                ),
                _ => t!(
                    "Son état est inconnu : sa tâche en cours peut être interrompue. Il repartira sur sa conversation.",
                    "Its state is unknown: its current task may be interrupted. It starts again on its conversation."
                ),
            };
            confirm.lines = vec![note];
            busy = Vec::new();
            confirm.yes = t!("Relancer", "Restart");
            confirm.letters = Some(('r', t!("a", "c").chars().next().unwrap_or('a')));
        }
        self.overlay = Overlay::Confirm(confirm.busy(busy));
        Vec::new()
    }

    fn contact(&mut self, on: bool, env: &dyn Env) -> Vec<Effect> {
        let name = self.name();
        let contacts: Vec<String> = self.team.config.contacts().into_iter().map(String::from).collect();
        let set = |member: &str, on: bool| Edit::Set { member: member.into(), field: Field::Contact(on) };
        if on == contacts.contains(&name) {
            return Vec::new();
        }
        let edits = if !on {
            if contacts == [name.clone()] {
                if self.team.config.members.len() <= 1 {
                    self.said = Some(Said::Failed(t!(
                        "« {} » est le seul membre : il reste l'interlocuteur.",
                        "\"{}\" is the only member: it stays the contact.",
                        name
                    )));
                    return Vec::new();
                }
                self.replacement(&name, false);
                return Vec::new();
            }
            vec![set(&name, false)]
        } else {
            if contacts.len() >= MAX_CONTACTS {
                self.said = Some(Said::Failed(t!(
                    "Deux interlocuteurs au plus : passe d'abord {} en agent.",
                    "Two contacts at most: make {} a working agent first.",
                    joined(&contacts, &t!("ou", "or"))
                )));
                return Vec::new();
            }
            // With no member marked, the first one is a contact: marked, it stays one.
            let mut edits = Vec::new();
            if !self.team.config.members.values().any(|m| m.contact)
                && let Some(first) = self.team.config.members.keys().next()
            {
                edits.push(set(first, true));
            }
            edits.push(set(&name, true));
            edits
        };
        let restarts = env.restarts(&edits);
        self.confirmed(edits, &restarts, Done::Contacts, None, Fid::Contact)
    }

    /// Asks who becomes a contact in place of `leaving`, the only one.
    fn replacement(&mut self, leaving: &str, removed: bool) {
        let items: Vec<Choice> = self
            .team
            .config
            .members
            .keys()
            .filter(|n| *n != leaving)
            .map(|n| Choice::new(Some(n), n.clone()))
            .collect();
        self.said = Some(Said::Asked(t!(
            "« {} » est le seul interlocuteur : qui le devient à sa place ?",
            "\"{}\" is the only contact: who becomes one in its place?",
            leaving
        )));
        self.overlay = Overlay::List(Dropdown {
            field: Some(if removed { Fid::Remove } else { Fid::Contact }),
            items,
            at: 0,
            current: None,
            purpose: Purpose::Replacement { leaving: leaving.to_string(), removed },
        });
    }

    fn restart(&mut self) -> Vec<Effect> {
        let name = self.name();
        let fresh = self.fresh;
        let said = if fresh {
            t!("« {} » relancé sur une conversation neuve.", "\"{}\" restarted on a new conversation.", name)
        } else {
            t!("« {} » relancé sur sa conversation.", "\"{}\" restarted on its conversation.", name)
        };
        let effect = Effect::Restart { member: name.clone(), fresh, said };
        let busy = self.interrupted(std::slice::from_ref(&name));
        if !fresh && busy.is_empty() {
            return vec![effect];
        }
        let line = if fresh {
            t!(
                "Sa conversation repart de zéro, son contexte vidé.",
                "Its conversation starts over, its context cleared."
            )
        } else {
            t!("« {} » repartira sur sa conversation.", "\"{}\" starts again on its conversation.", name)
        };
        let confirm = Confirm::new(
            t!("Relancer « {} » ?", "Restart \"{}\"?", name),
            vec![line],
            t!("Relancer", "Restart"),
            vec![effect],
        );
        self.overlay = Overlay::Confirm(confirm.busy(busy).danger(fresh));
        Vec::new()
    }

    fn remove(&mut self) -> Vec<Effect> {
        let name = self.name();
        if self.team.config.members.len() <= 1 {
            self.said = Some(Said::Failed(t!(
                "« {} » est le dernier membre : l'équipe ne peut pas rester vide.",
                "\"{}\" is the last member: the team cannot be left empty.",
                name
            )));
            return Vec::new();
        }
        if self.team.config.contacts() == [name.as_str()] {
            self.replacement(&name, true);
            return Vec::new();
        }
        self.confirm_remove(name, None);
        Vec::new()
    }

    fn confirm_remove(&mut self, name: String, replacement: Option<String>) {
        let mut edits = Vec::new();
        if let Some(other) = replacement {
            edits.push(Edit::Set { member: other, field: Field::Contact(true) });
        }
        edits.push(Edit::Remove(name.clone()));
        let effect = Effect::Apply {
            edits,
            said: Done::Text(t!("« {} » a quitté l'équipe.", "\"{}\" left the team.", name)),
            select: None,
        };
        let confirm = Confirm::new(
            t!("Retirer « {} » ?", "Remove \"{}\"?", name),
            vec![t!(
                "« {} » quitte l'équipe : son panneau se ferme et ses réglages quittent les fichiers de l'équipe.",
                "\"{}\" leaves the team: its pane closes and its settings leave the team's files.",
                name
            )],
            t!("Retirer « {} »", "Remove \"{}\"", name),
            vec![effect],
        );
        self.overlay = Overlay::Confirm(confirm.busy(self.interrupted(&[name])).danger(true));
    }

    fn add(&mut self) -> Vec<Effect> {
        if self.draft.how == How::Composed && !self.draft.composed {
            self.said = Some(Said::Failed(t!(
                "Claude ne l'a pas encore composé : ⏎ sur « Composer ».",
                "Claude has not composed it yet: ⏎ on \"Compose\"."
            )));
            return Vec::new();
        }
        let name = self.draft.name.trim().to_string();
        if let Err(error) = config::check_new_name(&self.team.config, &name) {
            self.said = Some(Said::Failed(error));
            return Vec::new();
        }
        if self.draft.role.trim().is_empty() {
            self.said = Some(Said::Failed(t!("Il lui faut un rôle.", "It needs a role.")));
            return Vec::new();
        }
        // A working agent: made a contact from its own sheet, if need be.
        let member = Member {
            role: self.draft.role.trim().to_string(),
            instructions: self.draft.instructions.clone().filter(|i| !i.trim().is_empty()),
            contact: false,
            ..self.draft.base.clone()
        };
        vec![Effect::Apply {
            edits: vec![Edit::Add { name: name.clone(), member }],
            said: Done::Text(t!(
                "« {} » rejoint l'équipe : son panneau s'ouvre.",
                "\"{}\" joins the team: its pane opens.",
                name
            )),
            select: Some(name),
        }]
    }

    /// The new agent added: a blank form for the next one.
    pub(crate) fn added(&mut self) {
        self.draft = Draft::default();
        self.roles = None;
    }

    /// Claude's new agent for request `id`, or why it could not compose one; another request's, ignored.
    pub(crate) fn composed(&mut self, id: u64, result: Result<(String, Member), String>) {
        if self.draft.composing != Some(id) {
            return;
        }
        self.draft.composing = None;
        match result {
            Ok((name, member)) => {
                self.draft.name = name;
                self.draft.role = member.role.clone();
                self.draft.instructions = member.instructions.clone();
                self.draft.base = member;
                self.draft.composed = true;
                if self.entry == Entry::New && self.pane == Pane::Sheet {
                    self.row = self.focusable().iter().position(|f| *f == Fid::Add).unwrap_or(self.row);
                }
            }
            Err(error) => self.said = Some(Said::Failed(error)),
        }
    }

    /// The text the editor starts from, for what it is for.
    pub(crate) fn to_edit(&self, edited: &Edited) -> String {
        match edited {
            Edited::Draft => self.draft.instructions.clone().unwrap_or_default(),
            Edited::Instructions(name) => self
                .team
                .person(name)
                .and_then(|p| p.layers.as_ref())
                .and_then(|l| l.instructions.value.clone())
                .unwrap_or_default(),
        }
    }

    /// The editor gave back `result`: the text, or why it failed.
    pub(crate) fn edited(&mut self, edited: Edited, result: Result<String, String>) -> Vec<Effect> {
        let text = match result {
            Ok(text) => text,
            Err(error) => {
                self.said = Some(Said::Failed(t!("L'éditeur a échoué : {}", "The editor failed: {}", error)));
                return Vec::new();
            }
        };
        let text = Some(text.trim().to_string()).filter(|t| !t.is_empty());
        match edited {
            Edited::Draft => {
                self.draft.instructions = text;
                Vec::new()
            }
            Edited::Instructions(name) => {
                let before = self.to_edit(&Edited::Instructions(name.clone()));
                if text.as_deref() == Some(before.trim()).filter(|b| !b.is_empty()) {
                    self.said = Some(Said::Done(t!(
                        "Instructions inchangées. Si l'éditeur a rendu la main tout de suite, règle-le pour qu'il attende (EDITOR=\"code -w\").",
                        "Instructions unchanged. If the editor returned at once, set it to wait (EDITOR=\"code -w\")."
                    )));
                    return Vec::new();
                }
                vec![Effect::Apply {
                    edits: vec![Edit::Set { member: name.clone(), field: Field::Instructions(text) }],
                    said: Done::Text(t!(
                        "Instructions de « {} » changées : il les lira avec son prochain message.",
                        "Instructions of \"{}\" changed: it reads them with its next message.",
                        name
                    )),
                    select: None,
                }]
            }
        }
    }

    fn action(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::New => {
                self.choose(Entry::New);
                self.pane = Pane::Sheet;
            }
            Action::Dashboard => {
                let on = !self.team.dashboard;
                return vec![Effect::Apply {
                    edits: vec![Edit::Dashboard(on)],
                    said: Done::Text(if on {
                        t!("Tableau de bord et journal ouverts.", "Dashboard and journal opened.")
                    } else {
                        t!("Tableau de bord et journal fermés.", "Dashboard and journal closed.")
                    }),
                    select: None,
                }];
            }
            Action::Layout => {
                let focus = self.team.layers.as_ref().is_some_and(|l| l.layout.value == config::Layout::Focus);
                let (layout, said) = if focus {
                    (config::Layout::Auto, t!("Disposition : grille.", "Layout: grid."))
                } else {
                    (config::Layout::Focus, t!("Disposition : focus.", "Layout: focus."))
                };
                return vec![Effect::Apply { edits: vec![Edit::Layout(layout)], said: Done::Text(said), select: None }];
            }
            Action::Grid => self.open_grid(),
            Action::AutoCompact => self.open_compaction(),
            Action::RestartAll => {
                let everyone: Vec<String> = self.team.people.iter().map(|p| p.name.clone()).collect();
                let confirm = Confirm::new(
                    t!("Réinitialiser l'équipe ?", "Reset the team?"),
                    vec![t!(
                        "Chaque membre repart sur une conversation neuve, son contexte vidé.",
                        "Every member starts over on a new conversation, its context cleared."
                    )],
                    t!("Réinitialiser", "Reset"),
                    vec![Effect::RestartAll],
                );
                self.overlay = Overlay::Confirm(confirm.busy(self.interrupted(&everyone)).danger(true));
            }
            Action::Detach if self.team.detach => return vec![Effect::Detach],
            Action::Detach => {}
            Action::Stop => {
                let confirm = Confirm::new(
                    t!("Quitter l'équipe ?", "Quit the team?"),
                    vec![t!(
                        "Tous les membres s'arrêtent et l'équipe se ferme.",
                        "Every member stops and the team closes."
                    )],
                    t!("Quitter", "Quit"),
                    vec![Effect::Stop],
                );
                self.overlay = Overlay::Confirm(confirm.danger(true));
            }
        }
        Vec::new()
    }

    /// The grids offered, the current one among them.
    const GRIDS: [(usize, usize); 5] = [(4, 2), (3, 2), (2, 2), (3, 1), (2, 1)];
    /// The shares offered for the automatic compaction; `off` first.
    const SHARES: [u8; 5] = [50, 60, 70, 80, 90];

    /// Opens the list of grids under the button.
    fn open_grid(&mut self) {
        let Some(layers) = &self.team.layers else { return };
        let now = (layers.columns.value, layers.rows.value);
        let default = (
            layers.columns.fallback.as_ref().map_or(3, |(v, _)| *v),
            layers.rows.fallback.as_ref().map_or(2, |(v, _)| *v),
        );
        // The default and the one in place, offered even when they are not among the usual: else there is no way back.
        let mut grids = Self::GRIDS.to_vec();
        for extra in [default, now] {
            if !grids.contains(&extra) {
                grids.insert(0, extra);
            }
        }
        let items: Vec<Choice> = grids
            .iter()
            .map(|&(columns, rows)| {
                let mut choice = Choice::new(Some(&format!("{columns}x{rows}")), format!("{columns} × {rows}"));
                let per_tab = t!("{} par onglet", "{} per tab", columns * rows);
                choice.detail = Some(if (columns, rows) == default {
                    format!("{per_tab} · {}", t!("défaut", "default"))
                } else {
                    per_tab
                });
                choice
            })
            .collect();
        let current = grids.iter().position(|g| *g == now);
        self.overlay =
            Overlay::List(Dropdown { field: None, items, at: current.unwrap_or(0), current, purpose: Purpose::Grid });
    }

    fn apply_grid(&mut self, choice: &Choice) -> Vec<Effect> {
        self.said = None;
        let parsed = choice.value.as_deref().and_then(|v| v.split_once('x'));
        let Some((Ok(columns), Ok(rows))) = parsed.map(|(c, r)| (c.parse::<usize>(), r.parse::<usize>())) else {
            return Vec::new();
        };
        vec![Effect::Apply {
            edits: vec![Edit::Grid { columns, rows }],
            said: Done::Text(t!("Grille des onglets : {} × {}.", "Grid of the tabs: {} × {}.", columns, rows)),
            select: None,
        }]
    }

    /// Opens the list of shares under the button.
    fn open_compaction(&mut self) {
        let Some(layers) = &self.team.layers else { return };
        let now = layers.auto_compact.value;
        let default = layers.auto_compact.fallback.as_ref().map_or(Some(config::AUTO_COMPACT_AT), |(v, _)| *v);
        let mut shares: Vec<Option<u8>> = std::iter::once(None).chain(Self::SHARES.map(Some)).collect();
        // The default and the one in place, offered even when they are not among the usual: else there is no way back.
        for at in [default, now].into_iter().flatten().filter(|at| !Self::SHARES.contains(at)) {
            if shares.contains(&Some(at)) {
                continue;
            }
            let place = shares.iter().position(|s| s.is_some_and(|s| s > at)).unwrap_or(shares.len());
            shares.insert(place, Some(at));
        }
        let items: Vec<Choice> = shares
            .iter()
            .map(|share| {
                let (value, label) = match share {
                    None => ("off".to_string(), t!("non", "off")),
                    Some(at) => (at.to_string(), t!("{} %", "{}%", at)),
                };
                let mut choice = Choice::new(Some(&value), label);
                if *share == default {
                    choice.detail = Some(t!("défaut", "default"));
                }
                choice
            })
            .collect();
        let current = shares.iter().position(|s| *s == now);
        self.overlay = Overlay::List(Dropdown {
            field: None,
            items,
            at: current.unwrap_or(0),
            current,
            purpose: Purpose::Compaction,
        });
    }

    fn apply_compaction(&mut self, choice: &Choice) -> Vec<Effect> {
        self.said = None;
        let share = match choice.value.as_deref() {
            Some("off") => None,
            Some(at) => match at.parse::<u8>() {
                Ok(at) => Some(at),
                Err(_) => return Vec::new(),
            },
            None => return Vec::new(),
        };
        let said = match share {
            None => t!("Compaction automatique : non.", "Automatic compaction: off."),
            Some(at) => {
                t!("Compaction automatique à {} % de la fenêtre.", "Automatic compaction at {}% of the window.", at)
            }
        };
        vec![Effect::Apply { edits: vec![Edit::AutoCompact(share)], said: Done::Text(said), select: None }]
    }

    fn confirm_key(&mut self, mut confirm: Confirm, key: Key) -> Vec<Effect> {
        match key {
            Key::Char(c) if confirm.letters.is_some_and(|(yes, _)| yes == c) => return confirm.then,
            Key::Char(c) if confirm.letters.is_some_and(|(_, no)| no == c) => return Vec::new(),
            Key::Left | Key::Right | Key::Tab => confirm.at = 1 - confirm.at,
            Key::Enter if confirm.at == 0 => return confirm.then,
            Key::Enter | Key::Esc => return Vec::new(),
            _ => {}
        }
        self.overlay = Overlay::Confirm(confirm);
        Vec::new()
    }

    fn list_key(&mut self, mut list: Dropdown, key: Key, env: &dyn Env) -> Vec<Effect> {
        let last = list.items.len().saturating_sub(1);
        match key {
            Key::Up => list.at = list.at.saturating_sub(1),
            Key::Down => list.at = (list.at + 1).min(last),
            Key::Home | Key::PageUp => list.at = 0,
            Key::End | Key::PageDown => list.at = last,
            Key::Esc => {
                if !matches!(list.purpose, Purpose::Value) {
                    self.said = None;
                }
                return Vec::new();
            }
            Key::Enter => return self.picked(list, env),
            _ => {}
        }
        self.overlay = Overlay::List(list);
        Vec::new()
    }

    /// The item highlighted in an open list, taken.
    fn picked(&mut self, list: Dropdown, env: &dyn Env) -> Vec<Effect> {
        if let Some(quick) = self.quick.as_mut() {
            quick.list = list.clone();
        }
        let Some(choice) = list.items.get(list.at).cloned() else { return Vec::new() };
        match list.purpose {
            Purpose::Value => list.field.map_or_else(Vec::new, |field| self.apply_choice(field, &choice, env)),
            Purpose::Grid => self.apply_grid(&choice),
            Purpose::Compaction => self.apply_compaction(&choice),
            Purpose::Replacement { leaving, removed } => {
                self.said = None;
                let other = choice.value.unwrap_or_default();
                if removed {
                    self.confirm_remove(leaving, Some(other));
                    return Vec::new();
                }
                let edits = vec![
                    Edit::Set { member: leaving, field: Field::Contact(false) },
                    Edit::Set { member: other, field: Field::Contact(true) },
                ];
                let restarts = env.restarts(&edits);
                self.confirmed(edits, &restarts, Done::Contacts, None, Fid::Contact)
            }
        }
    }

    /// Opens a text typed in place over `initial`, on row `field`; what is wrong with it said at once, but for an
    /// empty one, said once typed or refused.
    fn type_in(&mut self, field: Fid, initial: &str) {
        let mut typing = Typing { field, text: initial.to_string(), cursor: initial.chars().count(), error: None };
        if !initial.trim().is_empty() {
            typing.error = self.check(&typing);
        }
        self.overlay = Overlay::Typing(typing);
    }

    /// What is wrong with a text as typed so far, for the row it is for.
    fn check(&self, typing: &Typing) -> Option<String> {
        let text = typing.text.trim();
        match typing.field {
            Fid::Name => {
                let name = self.name();
                if text == name {
                    return None;
                }
                let mut others = self.team.config.clone();
                others.members.shift_remove(&name);
                config::check_new_name(&others, text).err()
            }
            Fid::NewName => config::check_new_name(&self.team.config, text).err(),
            _ if text.is_empty() => Some(t!("Une réponse est attendue.", "An answer is required.")),
            _ => None,
        }
    }

    fn typing_key(&mut self, mut typing: Typing, key: Key, env: &dyn Env) -> Vec<Effect> {
        let enter = key == Key::Enter;
        let len = typing.text.chars().count();
        let byte = |text: &str, at: usize| text.char_indices().nth(at).map_or(text.len(), |(i, _)| i);
        match key {
            Key::Esc => return Vec::new(),
            Key::Enter => match self.check(&typing) {
                Some(error) => typing.error = Some(error),
                None => return self.typed(typing, env),
            },
            Key::Left => typing.cursor = typing.cursor.saturating_sub(1),
            Key::Right => typing.cursor = (typing.cursor + 1).min(len),
            Key::Home => typing.cursor = 0,
            Key::End => typing.cursor = len,
            Key::Char(c) if !c.is_control() => {
                let at = byte(&typing.text, typing.cursor);
                typing.text.insert(at, c);
                typing.cursor += 1;
            }
            Key::Paste(text) => {
                let clean: String = text.chars().filter(|c| !c.is_control()).collect();
                let at = byte(&typing.text, typing.cursor);
                typing.text.insert_str(at, &clean);
                typing.cursor += clean.chars().count();
            }
            Key::Backspace if typing.cursor > 0 => {
                let at = byte(&typing.text, typing.cursor - 1);
                typing.text.remove(at);
                typing.cursor -= 1;
            }
            Key::Delete if typing.cursor < len => {
                let at = byte(&typing.text, typing.cursor);
                typing.text.remove(at);
            }
            Key::EraseWord => {
                let before: String = typing.text.chars().take(typing.cursor).collect();
                let kept = before.trim_end().rfind(' ').map_or(0, |i| i + 1);
                let after: String = typing.text.chars().skip(typing.cursor).collect();
                typing.cursor = before[..kept].chars().count();
                typing.text = format!("{}{after}", &before[..kept]);
            }
            Key::EraseAll => {
                typing.text.clear();
                typing.cursor = 0;
            }
            _ => {}
        }
        if !enter {
            typing.error = self.check(&typing);
        }
        self.overlay = Overlay::Typing(typing);
        Vec::new()
    }

    /// A text typed and accepted.
    fn typed(&mut self, typing: Typing, env: &dyn Env) -> Vec<Effect> {
        let text = typing.text.trim().to_string();
        let name = self.name();
        match typing.field {
            Fid::Name if text == name => Vec::new(),
            Fid::Name => {
                let edits = vec![Edit::Rename { from: name.clone(), to: text.clone() }];
                let said = if env.has_conversation(&name) {
                    t!(
                        "« {} » s'appelle maintenant « {} » : relancé sur sa conversation.",
                        "\"{}\" is now called \"{}\": restarted on its conversation.",
                        name,
                        text
                    )
                } else {
                    t!(
                        "« {} » s'appelle maintenant « {} » : il n'avait encore rien échangé, relancé sur une conversation neuve.",
                        "\"{}\" is now called \"{}\": it had exchanged nothing yet, restarted on a new conversation.",
                        name,
                        text
                    )
                };
                let restarts = env.restarts(&edits);
                self.confirmed(edits, &restarts, Done::Text(said), Some(text), Fid::Name)
            }
            Fid::Role => {
                let current = self.person().and_then(|p| p.layers.as_ref()).map(|l| l.role.value.trim().to_string());
                if current.as_deref() == Some(text.as_str()) {
                    return Vec::new();
                }
                vec![Effect::Apply {
                    edits: vec![Edit::Set { member: name.clone(), field: Field::Role(text) }],
                    said: Done::Text(t!(
                        "Rôle de « {} » changé : chacun l'apprendra avec son prochain message.",
                        "Role of \"{}\" changed: each member learns it with its next message.",
                        name
                    )),
                    select: None,
                }]
            }
            Fid::Model if bridge::model_id(&text).is_none() => {
                // Claude Code is the judge of the others, at the member's restart.
                let aliases: Vec<&str> = bridge::ALIASES.iter().map(|(alias, _)| *alias).collect();
                let edits = self.edits(Setting::Model, &Choice::new(Some(&text), text.clone()));
                let said = Done::Text(Setting::Model.changed(&name, true));
                let confirm = Confirm::new(
                    t!("Écrire « {} » quand même ?", "Write \"{}\" anyway?", text),
                    vec![t!(
                        "« {} » n'est ni un alias connu ({}) ni un identifiant claude-… : Claude Code pourrait le refuser, et « {} » sera relancé avec.",
                        "\"{}\" is neither a known alias ({}) nor a claude-… id: Claude Code may refuse it, and \"{}\" restarts with it.",
                        text,
                        aliases.join(", "),
                        name
                    )],
                    t!("L'écrire", "Write it"),
                    vec![Effect::Apply { edits, said, select: None }],
                );
                self.overlay = Overlay::Confirm(confirm.busy(self.interrupted(&[name])));
                Vec::new()
            }
            Fid::Model | Fid::Tab => self.apply_choice(typing.field, &Choice::new(Some(&text), text.clone()), env),
            Fid::Request => {
                self.draft.request = text;
                // Opened from « Compose »: composed at once.
                if self.field() == Some(Fid::Compose) { self.compose() } else { Vec::new() }
            }
            Fid::NewName => {
                self.draft.name = text;
                self.draft.named = true;
                Vec::new()
            }
            Fid::NewRole => {
                self.draft.role = text;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// A click on what `target` names.
    pub(crate) fn click(&mut self, target: Target, env: &dyn Env) -> Vec<Effect> {
        let effects = self.click_in(target, env);
        self.settled(effects)
    }

    /// The pointer over `target`: in an open list, its cursor follows; under a question, so does the button.
    pub(crate) fn hover(&mut self, target: Option<Target>) {
        match (&mut self.overlay, target) {
            (Overlay::List(list), Some(Target::Item(at))) if at < list.items.len() => list.at = at,
            (Overlay::Confirm(confirm), Some(Target::Yes)) => confirm.at = 0,
            (Overlay::Confirm(confirm), Some(Target::No)) => confirm.at = 1,
            _ => {}
        }
    }

    fn click_in(&mut self, target: Target, env: &dyn Env) -> Vec<Effect> {
        match (&self.overlay, target) {
            (Overlay::Confirm(_), Target::Yes) => {
                if let Overlay::Confirm(confirm) = std::mem::replace(&mut self.overlay, Overlay::None) {
                    return confirm.then;
                }
                Vec::new()
            }
            (Overlay::Confirm(_), Target::No) => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            // A dialog stays until answered.
            (Overlay::Confirm(_), _) => Vec::new(),
            (Overlay::List(_), Target::Item(at)) => {
                if let Overlay::List(mut list) = std::mem::replace(&mut self.overlay, Overlay::None) {
                    list.at = at.min(list.items.len().saturating_sub(1));
                    return self.picked(list, env);
                }
                Vec::new()
            }
            // A click in the text being typed keeps it.
            (Overlay::Typing(typing), Target::Row(row)) if self.focusable().get(row) == Some(&typing.field) => {
                Vec::new()
            }
            (_, target) => {
                // A click elsewhere closes a list or gives up a text.
                self.overlay = Overlay::None;
                match target {
                    Target::Entry(entry) => {
                        self.choose(entry);
                        self.pane = Pane::List;
                    }
                    Target::Row(row) => {
                        if self.pane == Pane::Sheet
                            && self.row == row
                            && let Some(&fid) = self.focusable().get(row)
                        {
                            return self.enter(fid, env);
                        }
                        self.pending = None;
                        self.pane = Pane::Sheet;
                        self.row = row;
                    }
                    Target::Step(row, forward) => {
                        if self.row != row {
                            self.pending = None;
                        }
                        self.pane = Pane::Sheet;
                        self.row = row;
                        if let Some(&fid) = self.focusable().get(row) {
                            return self.step(fid, forward, env);
                        }
                    }
                    Target::Restart(fresh) => {
                        self.pending = None;
                        self.pane = Pane::Sheet;
                        self.row = self.focusable().iter().position(|f| *f == Fid::Restart).unwrap_or(self.row);
                        self.fresh = fresh;
                        return self.restart();
                    }
                    Target::Action(action) => return self.action(action),
                    Target::Item(_) | Target::Yes | Target::No => {}
                }
                Vec::new()
            }
        }
    }

    /// The wheel over the list (`list`) or the sheet: up or down one; the value prepared is given up.
    pub(crate) fn wheel(&mut self, list: bool, down: bool, env: &dyn Env) -> Vec<Effect> {
        let key = if down { Key::Down } else { Key::Up };
        match &self.overlay {
            Overlay::List(_) => self.key(key, env),
            Overlay::None => {
                self.pending = None;
                self.pane = if list || self.focusable().is_empty() { Pane::List } else { Pane::Sheet };
                self.key(key, env)
            }
            _ => Vec::new(),
        }
    }
}

/// A sentence about some members, quoted.
type Sentence = fn(&str) -> String;

/// What the screen says while the team's files do not read.
fn unreadable(error: &str) -> String {
    t!(
        "Les fichiers de l'équipe ne se lisent pas ({}) : réglages en lecture seule ; détacher et quitter restent possibles.",
        "The team's files do not read ({}): settings read only; detaching and quitting still work.",
        error
    )
}

pub(crate) fn source_name(source: Source, origin: &Origin) -> String {
    match source {
        Source::Personal => config::SETTINGS_LOCAL.to_string(),
        Source::Shared => match origin {
            Origin::Local { .. } => config::SETTINGS.to_string(),
            Origin::Global { file } => file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        },
        Source::Team => t!("équipe", "team"),
        Source::Default => t!("défaut", "default"),
    }
}

/// `opus` → `Opus`; an id as it is.
pub(crate) fn model_label(model: &str) -> String {
    match MODELS.iter().find(|(id, _)| *id == model) {
        Some((_, label)) => label.to_string(),
        None => model.to_string(),
    }
}

fn mode_detail(mode: &str) -> String {
    match mode {
        "default" => t!("demande avant d'agir", "asks before acting"),
        "acceptEdits" => {
            t!("les modifications de fichiers sont acceptées d'office", "file edits are accepted automatically")
        }
        "auto" => {
            t!("Claude Code approuve seul ce qui est sans risque", "Claude Code approves safe actions on its own")
        }
        "plan" => t!("prépare un plan sans rien modifier", "makes a plan, changes nothing"),
        "bypassPermissions" => t!(
            "aucune demande (comme --dangerously-skip-permissions)",
            "never asks (like --dangerously-skip-permissions)"
        ),
        _ => String::new(),
    }
}

pub(crate) fn instructions_count(instructions: Option<&str>) -> String {
    match instructions.map(|i| i.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0) {
        0 => t!("aucune", "none"),
        lines => i18n::count(lines, "ligne", "line"),
    }
}

/// The first line of `text`, cut to `width` characters.
pub(crate) fn first_line(text: &str, width: usize) -> String {
    let line = text.trim().lines().next().unwrap_or_default();
    if line.chars().count() <= width {
        return line.to_string();
    }
    let cut: String = line.chars().take(width.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// « a », « b » `last` « c »: the names quoted, the last two joined by `last` (« et », « ou »).
pub(crate) fn joined(names: &[impl AsRef<str>], last: &str) -> String {
    let quoted: Vec<String> = names.iter().map(|n| t!("« {} »", "\"{}\"", n.as_ref())).collect();
    match quoted.split_last() {
        Some((end, rest)) if !rest.is_empty() => format!("{} {last} {end}", rest.join(", ")),
        _ => quoted.concat(),
    }
}

/// « a », « b » et « c ».
pub(crate) fn quoted(names: &[impl AsRef<str>]) -> String {
    joined(names, &t!("et", "and"))
}

/// A team for the tests, without files: `names` with whether each is a contact.
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;

    pub(crate) fn layered(
        value: Option<&str>,
        source: Source,
        fallback: Option<(Option<&str>, Source)>,
    ) -> Layered<Option<String>> {
        Layered { value: value.map(String::from), source, fallback: fallback.map(|(v, s)| (v.map(String::from), s)) }
    }

    fn unset() -> Layered<Option<String>> {
        layered(None, Source::Default, Some((None, Source::Default)))
    }

    pub(crate) fn layers(name: &str, contact: bool) -> MemberLayers {
        MemberLayers {
            role: Layered { value: format!("Rôle de {name}"), source: Source::Shared, fallback: None },
            instructions: layered(None, Source::Default, None),
            contact: Layered { value: contact, source: Source::Shared, fallback: Some((false, Source::Default)) },
            tab: unset(),
            permission_mode: unset(),
            // Opus from the team.
            model: layered(Some("opus"), Source::Team, Some((Some("opus"), Source::Team))),
            effort: unset(),
        }
    }

    pub(crate) fn team(names: &[(&str, bool)]) -> Team {
        let people = names
            .iter()
            .enumerate()
            .map(|(i, (name, contact))| Person {
                name: name.to_string(),
                contact: *contact,
                color: crate::look::member_color(i),
                gone: false,
                model: Some("Opus".into()),
                effort: Some("high".into()),
                role: format!("Rôle de {name}"),
                layers: Some(layers(name, *contact)),
                tab_now: Some("Agents".into()),
                live: true,
            })
            .collect();
        let members = names
            .iter()
            .map(|(name, contact)| {
                (name.to_string(), Member { role: format!("Rôle de {name}"), contact: *contact, ..Default::default() })
            })
            .collect();
        Team {
            name: "essai".into(),
            origin: Origin::Local { root: "/p".into() },
            people,
            config: config::Team { members, ..Default::default() },
            dashboard: true,
            layers: Some(TeamLayers {
                dashboard: Layered { value: true, source: Source::Default, fallback: None },
                auto_compact: Layered { value: Some(70), source: Source::Default, fallback: None },
                columns: Layered { value: 3, source: Source::Default, fallback: None },
                rows: Layered { value: 2, source: Source::Default, fallback: None },
                layout: Layered { value: config::Layout::Auto, source: Source::Default, fallback: None },
            }),
            detach: true,
            file: ".recruit/settings.toml".into(),
            unreadable: None,
        }
    }

    /// The running team, as the tests want it: `restarts` started again by any edit.
    pub(crate) struct Fake {
        pub restarts: Vec<String>,
        pub conversation: bool,
    }

    impl Fake {
        pub(crate) fn quiet() -> Self {
            Fake { restarts: Vec::new(), conversation: true }
        }

        pub(crate) fn restarting(names: &[&str]) -> Self {
            Fake { restarts: names.iter().map(|n| n.to_string()).collect(), conversation: true }
        }
    }

    impl Env for Fake {
        fn restarts(&self, _: &[Edit]) -> Vec<String> {
            self.restarts.clone()
        }

        fn has_conversation(&self, _: &str) -> bool {
            self.conversation
        }
    }

    /// The sheet of a team whose states are seen: none at work.
    pub(crate) fn seen(team: Team) -> Sheet {
        let mut sheet = Sheet::new(team);
        sheet.states_known = true;
        sheet
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{Fake, layered, seen, team};
    use super::*;

    fn quiet() -> Fake {
        Fake::quiet()
    }

    fn sheet() -> Sheet {
        seen(team(&[("coordinateur", true), ("dev", false), ("ops", false)]))
    }

    fn keys(sheet: &mut Sheet, keys: &[Key], env: &dyn Env) -> Vec<Effect> {
        let mut effects = Vec::new();
        for key in keys {
            effects = sheet.key(key.clone(), env);
        }
        effects
    }

    fn typed(sheet: &mut Sheet, text: &str) {
        for c in text.chars() {
            sheet.key(Key::Char(c), &quiet());
        }
    }

    fn to_row(sheet: &mut Sheet, fid: Fid) {
        sheet.pane = Pane::Sheet;
        sheet.row = sheet.focusable().iter().position(|f| *f == fid).expect("the row");
    }

    fn confirm(sheet: &Sheet) -> &Confirm {
        let Overlay::Confirm(confirm) = &sheet.overlay else { panic!("no dialog: {:?}", sheet.overlay) };
        confirm
    }

    /// `recruit _menu --member m --field f`: the member's sheet, its field focused; an unknown member or field left out.
    #[test]
    fn opened_on_a_member_and_a_field() {
        let mut s = sheet();
        s.open_on("dev", Some(Fid::Effort));
        assert_eq!((s.entry.clone(), s.pane, s.field()), (Entry::Member("dev".into()), Pane::Sheet, Some(Fid::Effort)));
        s.open_on("dev", Some(Fid::Name));
        assert_eq!(s.field(), Some(Fid::Name));
        // No field: the member chosen in the list.
        let mut s = sheet();
        s.open_on("dev", None);
        assert_eq!((s.entry.clone(), s.pane), (Entry::Member("dev".into()), Pane::List));
        // An unknown member: as without.
        let mut s = sheet();
        s.open_on("nobody", Some(Fid::Model));
        assert_eq!((s.entry.clone(), s.pane), (Entry::Member("coordinateur".into()), Pane::List));
    }

    /// A member whose turn failed (mock-up C): « Reprendre » first on its sheet, chosen when the menu opens on it;
    /// ⏎ asks its mod once, then says what it did.
    #[test]
    fn resuming_a_failed_turn() {
        let mut sheet = sheet();
        sheet.states.insert("dev".into(), (State::Error, 100));
        sheet.open_on("dev", None);
        assert_eq!((sheet.pane, sheet.field()), (Pane::Sheet, Some(Fid::Resume)));
        assert_eq!(sheet.value(Fid::Resume), t!("lui envoyer « continue »", "send it “continue”"));
        assert!(sheet.hint().is_some_and(|hint| hint.contains("continue")));
        assert_eq!(sheet.key(Key::Enter, &quiet()), [Effect::Resume("dev".into())]);
        // Asked: not again until its mod answers.
        assert!(sheet.key(Key::Enter, &quiet()).is_empty());
        assert_eq!(sheet.value(Fid::Resume), t!("envoi de « continue »…", "sending “continue”…"));
        sheet.resumed("dev", Resuming::Draft);
        assert_eq!(
            sheet.value(Fid::Resume),
            t!("pas envoyé : sa saisie contient du texte", "not sent: its prompt holds a draft")
        );
        sheet.resumed("ops", Resuming::Sent);
        assert_eq!(sheet.resuming, Some(("dev".into(), Resuming::Draft)));
        assert_eq!(sheet.key(Key::Enter, &quiet()), [Effect::Resume("dev".into())]);
        sheet.resumed("dev", Resuming::Sent);
        assert_eq!(sheet.value(Fid::Resume), t!("« continue » envoyé", "“continue” sent"));
        // A field asked for comes first; another member has no such row.
        sheet.open_on("dev", Some(Fid::Model));
        assert_eq!(sheet.field(), Some(Fid::Model));
        sheet.open_on("ops", None);
        assert_eq!(sheet.pane, Pane::List);
        assert!(!sheet.focusable().contains(&Fid::Resume));
        // Back at work: the row goes.
        sheet.states.insert("dev".into(), (State::Working, 200));
        sheet.choose(Entry::Member("dev".into()));
        assert!(!sheet.focusable().contains(&Fid::Resume));
    }

    #[test]
    fn the_list_then_the_sheet_then_out() {
        let mut s = sheet();
        assert_eq!(s.entry, Entry::Member("coordinateur".into()), "the contacts first");
        assert_eq!(s.entries().last(), Some(&Entry::New));
        keys(&mut s, &[Key::Down], &quiet());
        assert_eq!(s.entry, Entry::Member("dev".into()));
        keys(&mut s, &[Key::Enter], &quiet());
        assert_eq!((s.pane, s.field()), (Pane::Sheet, Some(Fid::Model)));
        assert!(s.focusable().contains(&Fid::Tab), "an agent has its tab");
        keys(&mut s, &[Key::Esc, Key::Up], &quiet());
        assert!(!s.focusable().contains(&Fid::Tab), "a contact is in the first tab");
        assert_eq!(keys(&mut s, &[Key::Esc], &quiet()), [Effect::Quit]);
    }

    #[test]
    fn an_inherited_value_is_the_default() {
        let mut s = sheet();
        to_row(&mut s, Fid::Model);
        let labels: Vec<String> = s.choices(Fid::Model, false).into_iter().map(|c| c.label).collect();
        assert_eq!(labels[1..], ["Sonnet", "Haiku", "Fable"], "Opus is the default's, not listed apart");
        assert_eq!(s.current(Fid::Model), Some(0));
        let effects = keys(&mut s, &[Key::Right], &quiet());
        let [Effect::Apply { edits, said: Done::Text(said), .. }] = effects.as_slice() else { panic!("{effects:?}") };
        assert_eq!(edits, &[Edit::Set { member: "coordinateur".into(), field: Field::Model(Some("sonnet".into())) }]);
        assert!(said.ends_with(&t!(" : effet dès sa prochaine requête.", ": in effect from its next request.")));
        let effects = keys(&mut s, &[Key::Left], &quiet());
        assert!(
            matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Set { member: "coordinateur".into(), field: Field::Model(Some("fable".into())) }]),
            "round to the last"
        );
        // Written by the member as the team's: still the default.
        s.team.people[0].layers.as_mut().unwrap().model =
            layered(Some("opus"), Source::Personal, Some((Some("opus"), Source::Team)));
        assert_eq!(s.current(Fid::Model), Some(0));
        // A mode prepared back to the inherited one: nothing prepared, nothing said.
        let mut s = sheet();
        s.team.people[0].layers.as_mut().unwrap().permission_mode =
            layered(Some("auto"), Source::Team, Some((Some("auto"), Source::Team)));
        to_row(&mut s, Fid::Mode);
        assert!(!s.choices(Fid::Mode, false).iter().any(|c| c.value.as_deref() == Some("auto")));
        keys(&mut s, &[Key::Right, Key::Left], &quiet());
        assert_eq!(s.pending, None);
        assert_eq!(
            Setting::Mode.changed("x", false),
            t!("Mode de permission de « x » changé.", "Permission mode of \"x\" changed.")
        );
    }

    #[test]
    fn a_change_that_restarts_waits_for_enter() {
        let mut s = sheet();
        let env = Fake::restarting(&["coordinateur"]);
        to_row(&mut s, Fid::Effort);
        assert!(keys(&mut s, &[Key::Right], &env).is_empty(), "prepared, not written");
        assert_eq!(s.value(Fid::Effort), look::effort_label("low"));
        keys(&mut s, &[Key::Right], &env);
        assert_eq!(s.pending, Some((Fid::Effort, 2)));
        // Another row: given up.
        keys(&mut s, &[Key::Down, Key::Up], &env);
        assert!(s.pending.is_none());
        keys(&mut s, &[Key::Left], &env);
        assert_eq!(s.value(Fid::Effort), look::effort_label("max"), "round from the default");
        let effects = keys(&mut s, &[Key::Enter], &env);
        let [Effect::Apply { said: Done::Text(said), .. }] = effects.as_slice() else { panic!("{effects:?}") };
        assert!(said.ends_with(&t!(" : relancé sur sa conversation.", ": restarted on its conversation.")));
        // Esc gives up a value prepared, and stays on the sheet.
        to_row(&mut s, Fid::Mode);
        keys(&mut s, &[Key::Right, Key::Esc], &env);
        assert_eq!((s.pending, s.pane), (None, Pane::Sheet));
    }

    #[test]
    fn a_value_prepared_is_given_up() {
        let env = Fake::restarting(&["dev"]);
        let mut s = sheet();
        s.choose(Entry::Member("dev".into()));
        let prepare = |s: &mut Sheet| {
            to_row(s, Fid::Mode);
            keys(s, &[Key::Right], &env);
            assert!(s.pending.is_some());
        };
        prepare(&mut s);
        s.click(Target::Entry(Entry::Member("dev".into())), &env);
        assert!(s.pending.is_none(), "a click on its own card");
        prepare(&mut s);
        s.wheel(true, false, &env);
        assert!(s.pending.is_none(), "the wheel");
        prepare(&mut s);
        s.click(Target::Restart(false), &env);
        assert!(s.pending.is_none(), "a click on « restart »");
    }

    #[test]
    fn a_member_at_work_is_asked_first() {
        let mut s = sheet();
        s.states.insert("coordinateur".into(), (State::Working, 0));
        let env = Fake::restarting(&["coordinateur"]);
        to_row(&mut s, Fid::Mode);
        keys(&mut s, &[Key::Right], &env);
        assert!(keys(&mut s, &[Key::Enter], &env).is_empty());
        assert!(matches!(&confirm(&s).busy[..], [(State::Working, said)] if said.contains("coordinateur")));
        assert_eq!(confirm(&s).at, 1, "« cancel » at first");
        assert!(keys(&mut s, &[Key::Enter], &env).is_empty(), "⏎ cancels");
        assert!(matches!(s.overlay, Overlay::None));
        keys(&mut s, &[Key::Right, Key::Enter], &env);
        let effects = keys(&mut s, &[Key::Left, Key::Enter], &env);
        assert!(matches!(effects.as_slice(), [Effect::Apply { .. }]), "← then ⏎");
    }

    #[test]
    fn one_waiting_for_the_user_is_said_apart() {
        let mut s = sheet();
        s.states.insert("dev".into(), (State::Waiting, 0));
        s.states.insert("ops".into(), (State::Working, 0));
        s.choose(Entry::Member("dev".into()));
        to_row(&mut s, Fid::Restart);
        keys(&mut s, &[Key::Enter], &quiet());
        assert!(matches!(&confirm(&s).busy[..], [(State::Waiting, said)] if said.contains("dev")));
        assert_ne!(confirm(&s).busy[0].1, s.interrupted(&["ops".into()])[0].1, "not the sentence for those at work");
        // Everyone: those at work, then those waiting.
        s.overlay = Overlay::None;
        s.pane = Pane::List;
        keys(&mut s, &[Key::Char('R')], &quiet());
        assert_eq!(
            confirm(&s).busy.iter().map(|(state, _)| *state).collect::<Vec<_>>(),
            [State::Working, State::Waiting]
        );
    }

    #[test]
    fn unknown_states_may_be_at_work() {
        let mut s = sheet();
        s.seen(None);
        s.choose(Entry::Member("dev".into()));
        to_row(&mut s, Fid::Restart);
        assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "asked");
        assert!(matches!(&confirm(&s).busy[..], [(State::Other, _)]));
        // Seen, and no session: nobody to interrupt.
        s.overlay = Overlay::None;
        s.seen(Some(HashMap::new()));
        assert!(matches!(&keys(&mut s, &[Key::Enter], &quiet())[..], [Effect::Restart { .. }]));
    }

    #[test]
    fn dialogs_cancel_at_first() {
        for open in [Key::Char(Action::Stop.key()), Key::Char('R')] {
            let mut s = sheet();
            keys(&mut s, std::slice::from_ref(&open), &quiet());
            assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "⏎ cancels: {open:?}");
            keys(&mut s, std::slice::from_ref(&open), &quiet());
            assert!(keys(&mut s, &[Key::Esc], &quiet()).is_empty());
            keys(&mut s, std::slice::from_ref(&open), &quiet());
            assert_eq!(keys(&mut s, &[Key::Tab, Key::Enter], &quiet()).len(), 1, "Tab to the action");
        }
        let mut s = sheet();
        s.choose(Entry::Member("dev".into()));
        to_row(&mut s, Fid::Remove);
        keys(&mut s, &[Key::Enter], &quiet());
        assert!(confirm(&s).danger);
        assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "Retirer ⏎ ⏎ removes nothing");
    }

    #[test]
    fn the_only_contact_needs_another() {
        let mut s = sheet();
        to_row(&mut s, Fid::Contact);
        keys(&mut s, &[Key::Right], &quiet());
        assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        assert_eq!(list.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["dev", "ops"]);
        let effects = keys(&mut s, &[Key::Down, Key::Enter], &quiet());
        let [Effect::Apply { edits, said: Done::Contacts, .. }] = effects.as_slice() else { panic!("{effects:?}") };
        assert_eq!(
            edits,
            &[
                Edit::Set { member: "coordinateur".into(), field: Field::Contact(false) },
                Edit::Set { member: "ops".into(), field: Field::Contact(true) },
            ]
        );
        // Alone in the team: it stays the contact.
        let mut s = seen(team(&[("seul", true)]));
        to_row(&mut s, Fid::Contact);
        keys(&mut s, &[Key::Right, Key::Enter], &quiet());
        assert!(matches!(s.overlay, Overlay::None), "no empty list");
        assert!(matches!(s.said, Some(Said::Failed(_))));
    }

    #[test]
    fn two_contacts_at_most() {
        let mut s = seen(team(&[("a", true), ("b", true), ("c", false)]));
        s.choose(Entry::Member("c".into()));
        to_row(&mut s, Fid::Contact);
        keys(&mut s, &[Key::Left], &quiet());
        assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty());
        assert!(matches!(&s.said, Some(Said::Failed(said)) if said.contains(&t!("ou", "or"))));
    }

    #[test]
    fn removing() {
        let mut s = seen(team(&[("seul", true)]));
        to_row(&mut s, Fid::Remove);
        keys(&mut s, &[Key::Enter], &quiet());
        assert!(matches!(s.said, Some(Said::Failed(_))), "not the last one");

        let mut s = sheet();
        to_row(&mut s, Fid::Remove);
        keys(&mut s, &[Key::Enter], &quiet());
        let Overlay::List(_) = &s.overlay else { panic!("who takes its place") };
        keys(&mut s, &[Key::Enter], &quiet());
        assert!(confirm(&s).danger);
        let [Effect::Apply { edits, .. }] = confirm(&s).then.as_slice() else { panic!() };
        assert_eq!(
            edits,
            &[Edit::Set { member: "dev".into(), field: Field::Contact(true) }, Edit::Remove("coordinateur".into())]
        );
        assert_eq!(keys(&mut s, &[Key::Left, Key::Enter], &quiet()).len(), 1);
    }

    #[test]
    fn renaming_checks_as_it_is_typed() {
        let mut s = sheet();
        s.choose(Entry::Member("ops".into()));
        to_row(&mut s, Fid::Name);
        keys(&mut s, &[Key::Enter, Key::EraseAll], &quiet());
        typed(&mut s, "Dev");
        let Overlay::Typing(typing) = &s.overlay else { panic!("not typing") };
        assert!(typing.error.is_some(), "differs by case only");
        assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "refused");
        keys(&mut s, &[Key::Backspace, Key::Backspace, Key::Backspace], &quiet());
        typed(&mut s, "release");
        let effects = keys(&mut s, &[Key::Enter], &quiet());
        let [Effect::Apply { edits, select, said: Done::Text(said) }] = effects.as_slice() else {
            panic!("{effects:?}")
        };
        assert_eq!(edits, &[Edit::Rename { from: "ops".into(), to: "release".into() }]);
        assert_eq!(select.as_deref(), Some("release"));
        assert!(said.ends_with(&t!(" : relancé sur sa conversation.", ": restarted on its conversation.")));
        // Nothing exchanged yet: a new conversation.
        let mut s = sheet();
        to_row(&mut s, Fid::Name);
        keys(&mut s, &[Key::Enter, Key::EraseAll], &quiet());
        typed(&mut s, "chef");
        let fresh = Fake { conversation: false, ..Fake::quiet() };
        let effects = keys(&mut s, &[Key::Enter], &fresh);
        assert!(
            matches!(&effects[..], [Effect::Apply { said: Done::Text(said), .. }] if said.contains(&t!("conversation neuve", "new conversation")))
        );
    }

    #[test]
    fn typing_in_place() {
        let mut s = sheet();
        to_row(&mut s, Fid::Role);
        keys(&mut s, &[Key::Enter, Key::Home, Key::Char('«'), Key::End, Key::EraseWord], &quiet());
        let Overlay::Typing(typing) = &s.overlay else { panic!("not typing") };
        assert_eq!(typing.text, "«Rôle de ");
        keys(&mut s, &[Key::Paste("tout\n".into())], &quiet());
        let Overlay::Typing(typing) = &s.overlay else { panic!("not typing") };
        assert_eq!((typing.text.as_str(), typing.cursor), ("«Rôle de tout", 13));
        // A click in it keeps it; elsewhere gives it up.
        let row = s.focusable().iter().position(|f| *f == Fid::Role).unwrap();
        s.click(Target::Row(row), &quiet());
        assert!(matches!(&s.overlay, Overlay::Typing(t) if t.text == "«Rôle de tout"));
        let effects = keys(&mut s, &[Key::Enter], &quiet());
        assert!(
            matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Set { member: "coordinateur".into(), field: Field::Role("«Rôle de tout".into()) }])
        );
        keys(&mut s, &[Key::Enter], &quiet());
        s.click(Target::Row(0), &quiet());
        assert!(matches!(s.overlay, Overlay::None));
    }

    #[test]
    fn the_team_keys_from_the_list_only() {
        let mut s = sheet();
        to_row(&mut s, Fid::Name);
        assert!(keys(&mut s, &[Key::Char('t')], &quiet()).is_empty(), "a letter in the sheet does nothing");
        keys(&mut s, &[Key::Esc], &quiet());
        let effects = keys(&mut s, &[Key::Char('t')], &quiet());
        assert!(matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Dashboard(false)]));
        keys(&mut s, &[Key::Char(Action::Stop.key())], &quiet());
        assert!(confirm(&s).danger, "stopping is asked");
        assert_eq!(keys(&mut s, &[Key::Left, Key::Enter], &quiet()), [Effect::Stop]);
        let mut s = sheet();
        s.team.detach = false;
        assert!(keys(&mut s, &[Key::Char('d')], &quiet()).is_empty(), "no client to detach");
    }

    #[test]
    fn a_new_agent() {
        let mut s = sheet();
        keys(&mut s, &[Key::Char('n')], &quiet());
        assert_eq!((s.entry.clone(), s.pane), (Entry::New, Pane::Sheet));
        let roles = s.roles.clone().unwrap();
        assert!(!roles.is_empty());
        assert_eq!(s.draft.name, roles[0].0, "the first role's name");
        to_row(&mut s, Fid::Builtin);
        keys(&mut s, &[Key::Right], &quiet());
        assert_eq!((s.draft.name.as_str(), s.draft.role.as_str()), (roles[1].0.as_str(), roles[1].1.role.as_str()));
        to_row(&mut s, Fid::Add);
        let effects = keys(&mut s, &[Key::Enter], &quiet());
        let [Effect::Apply { edits, select, .. }] = effects.as_slice() else { panic!("{effects:?}") };
        let [Edit::Add { name, member }] = edits.as_slice() else { panic!() };
        assert_eq!((name.as_str(), member.contact), (roles[1].0.as_str(), false));
        assert_eq!(select.as_deref(), Some(name.as_str()));
        // Typed by hand: nothing left of the built-in role; its own name and role, the role required.
        to_row(&mut s, Fid::How);
        keys(&mut s, &[Key::Right, Key::Right], &quiet());
        assert_eq!(s.draft.how, How::Typed);
        assert_eq!((s.draft.name.as_str(), s.draft.role.as_str(), s.draft.base.clone()), ("", "", Member::default()));
        s.draft.name = "testeur".into();
        to_row(&mut s, Fid::Add);
        assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty());
        assert!(matches!(s.said, Some(Said::Failed(_))), "no role");
        to_row(&mut s, Fid::NewRole);
        keys(&mut s, &[Key::Enter], &quiet());
        let Overlay::Typing(typing) = &s.overlay else { panic!("not typing") };
        assert!(typing.error.is_none(), "an empty text is not wrong before it is typed");
    }

    #[test]
    fn claude_composes_one() {
        let mut s = sheet();
        keys(&mut s, &[Key::Char('n')], &quiet());
        to_row(&mut s, Fid::How);
        keys(&mut s, &[Key::Right], &quiet());
        to_row(&mut s, Fid::Compose);
        keys(&mut s, &[Key::Enter], &quiet());
        assert!(matches!(s.overlay, Overlay::Typing(Typing { field: Fid::Request, .. })), "the request first");
        typed(&mut s, "doc");
        assert_eq!(keys(&mut s, &[Key::Enter], &quiet()), [Effect::Compose { id: 1, request: "doc".into() }]);
        to_row(&mut s, Fid::Add);
        assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "nothing to add before it is composed");
        to_row(&mut s, Fid::Compose);
        assert_eq!(keys(&mut s, &[Key::Esc], &quiet()), [Effect::CancelCompose(1)], "Esc stops it");
        assert_eq!(keys(&mut s, &[Key::Enter], &quiet()), [Effect::Compose { id: 2, request: "doc".into() }]);
        let member = Member { role: "Doc".into(), instructions: Some("- tout".into()), ..Default::default() };
        s.composed(1, Ok(("ancien".into(), member.clone())));
        assert!(!s.draft.composed, "the first request's result, ignored");
        s.composed(2, Ok(("redacteur".into(), member)));
        assert!(s.draft.composed);
        assert_eq!((s.draft.name.as_str(), s.field()), ("redacteur", Some(Fid::Add)));
        to_row(&mut s, Fid::Compose);
        keys(&mut s, &[Key::Enter], &quiet());
        s.composed(3, Err("pas de réseau".into()));
        assert_eq!(s.said, Some(Said::Failed("pas de réseau".into())));
    }

    #[test]
    fn changing_how_stops_claude() {
        let mut s = sheet();
        keys(&mut s, &[Key::Char('n')], &quiet());
        to_row(&mut s, Fid::How);
        keys(&mut s, &[Key::Right], &quiet());
        s.draft.request = "doc".into();
        to_row(&mut s, Fid::Compose);
        keys(&mut s, &[Key::Enter], &quiet());
        to_row(&mut s, Fid::How);
        assert_eq!(keys(&mut s, &[Key::Right], &quiet()), [Effect::CancelCompose(1)]);
        assert_eq!(s.draft.composing, None);
        s.composed(1, Ok(("tard".into(), Member::default())));
        assert!(!s.draft.composed && s.draft.name.is_empty(), "its result comes too late");
    }

    #[test]
    fn lists_of_values() {
        let mut s = sheet();
        to_row(&mut s, Fid::Model);
        keys(&mut s, &[Key::Enter], &quiet());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        assert_eq!(list.current, Some(0));
        assert!(list.items[0].label.contains("Opus"), "the default first, and where it comes from");
        assert!(list.items.last().unwrap().other);
        keys(&mut s, &[Key::End, Key::Enter], &quiet());
        assert!(matches!(s.overlay, Overlay::Typing(Typing { field: Fid::Model, .. })), "another one, typed");
        typed(&mut s, "gpt");
        keys(&mut s, &[Key::Enter], &quiet());
        assert!(matches!(s.overlay, Overlay::Confirm(_)), "an unknown model is asked");
        to_row(&mut s, Fid::Mode);
        s.overlay = Overlay::None;
        keys(&mut s, &[Key::Enter], &quiet());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        assert!(list.items.iter().skip(1).all(|i| i.detail.is_some()), "each mode said");
        keys(&mut s, &[Key::Esc], &quiet());
        assert!(matches!(s.overlay, Overlay::None));
    }

    #[test]
    fn values_and_where_they_come_from() {
        let mut s = sheet();
        let layers = s.team.people[0].layers.as_mut().unwrap();
        layers.model = layered(Some("haiku"), Source::Personal, Some((Some("sonnet"), Source::Shared)));
        layers.instructions = layered(Some("- un\n\n- deux"), Source::Shared, None);
        assert_eq!(s.value(Fid::Model), "Haiku");
        assert_eq!(s.origin(Fid::Model).as_deref(), Some("settings.local.toml"));
        assert_eq!(
            s.choices(Fid::Model, false)[0].label,
            t!("défaut : Sonnet (settings.toml)", "default: Sonnet (settings.toml)")
        );
        assert_eq!(s.current(Fid::Model), s.choices(Fid::Model, false).iter().position(|c| c.label == "Haiku"));
        assert_eq!(s.value(Fid::Instructions), i18n::count(2, "ligne", "line"));
        assert_eq!(s.origin(Fid::Tab).as_deref(), Some(t!("défaut de recruit", "recruit's default").as_str()));
        assert_eq!(s.choices(Fid::Tab, false)[0].label, t!("défaut : onglets « Agents »", "default: \"Agents\" tabs"));
        assert_eq!(model_label("claude-opus-4-1"), "claude-opus-4-1");
        let global = Origin::Global { file: "/c/recruit/web.toml".into() };
        assert_eq!(source_name(Source::Shared, &global), "web.toml");
    }

    /// No layer gives the effort: the sheet shows what Claude Code applies, with « Claude Code » as its origin, as the list's
    /// default line says it; when that is not known, as before (« défaut de Claude Code »). A layer's value keeps its
    /// file.
    #[test]
    fn what_claude_code_applies_is_shown_with_its_origin() {
        let mut s = sheet();
        // The fixture's coordinateur runs on Opus at high, nothing in the files for the effort.
        assert_eq!(s.value(Fid::Effort), look::effort_label("high"));
        assert_eq!(s.origin(Fid::Effort).as_deref(), Some("Claude Code"));
        assert_eq!(s.effort_level().as_deref(), Some("high"));
        // The model a layer gives is a layer's: its file.
        assert_eq!(s.value(Fid::Model), "Opus");
        assert_eq!(s.origin(Fid::Model).as_deref(), Some(t!("équipe", "team").as_str()));
        // No model in the files either: Opus all the same, from Claude Code.
        s.team.people[0].layers.as_mut().unwrap().model = layered(None, Source::Default, Some((None, Source::Default)));
        assert_eq!((s.value(Fid::Model).as_str(), s.origin(Fid::Model).as_deref()), ("Opus", Some("Claude Code")));
        // Not known yet: as before.
        s.team.people[0].effort = None;
        s.team.people[0].model = None;
        assert_eq!(s.value(Fid::Effort), t!("défaut de Claude Code", "Claude Code's default"));
        assert_eq!(s.value(Fid::Model), t!("défaut de Claude Code", "Claude Code's default"));
        assert_eq!(s.origin(Fid::Effort).as_deref(), Some(t!("défaut", "default").as_str()));
        assert_eq!(s.effort_level(), None);
        // A level a layer gives: its own color and its file.
        s.team.people[0].layers.as_mut().unwrap().effort = layered(Some("xhigh"), Source::Shared, None);
        assert_eq!(s.effort_level().as_deref(), Some("xhigh"));
        assert_eq!(s.origin(Fid::Effort).as_deref(), Some(config::SETTINGS));
    }

    #[test]
    fn the_grid_is_chosen_from_a_list() {
        let mut s = Sheet::new(team(&[("dev", false)]));
        keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        let labels: Vec<&str> = list.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["4 × 2", "3 × 2", "2 × 2", "3 × 1", "2 × 1"]);
        assert_eq!(list.current, Some(1), "the grid in place is marked");
        assert_eq!(list.items[0].detail, Some(t!("{} par onglet", "{} per tab", 8)), "how many it holds, beside it");
        keys(&mut s, &[Key::Down], &quiet());
        let effects = s.key(Key::Enter, &quiet());
        assert!(
            matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Grid { columns: 2, rows: 2 }]),
            "{effects:?}"
        );
        assert!(matches!(s.overlay, Overlay::None) && s.said.is_none());
        // Escape gives up, and says nothing.
        keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
        assert!(s.said.is_none());
        assert!(s.key(Key::Esc, &quiet()).is_empty() && s.said.is_none() && matches!(s.overlay, Overlay::None));
    }

    #[test]
    fn a_grid_from_the_files_is_offered_too() {
        let mut s = Sheet::new(team(&[("dev", false)]));
        let layers = s.team.layers.as_mut().unwrap();
        (layers.columns.value, layers.rows.value) = (4, 3);
        keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        assert_eq!((list.items[0].label.as_str(), list.current), ("4 × 3", Some(0)));
        assert_eq!(list.items.len(), 6);
        // The default is told apart, whatever the current one.
        let default = t!("défaut", "default");
        assert_eq!(list.items.iter().filter(|i| i.detail.as_deref().is_some_and(|d| d.contains(&default))).count(), 1);
    }

    #[test]
    fn the_default_is_always_offered() {
        let mut s = Sheet::new(team(&[("dev", false)]));
        let layers = s.team.layers.as_mut().unwrap();
        // The shared file says 4 columns, the local one 2 rows: the default under the local layer is 4 × 2.
        layers.columns.fallback = Some((4, Source::Shared));
        layers.rows.fallback = Some((2, Source::Shared));
        layers.auto_compact.value = Some(85);
        layers.auto_compact.fallback = Some((Some(75), Source::Shared));
        keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        let word = t!("défaut", "default");
        let default = list.items.iter().find(|i| i.detail.as_deref().is_some_and(|d| d.contains(&word)));
        assert_eq!(default.expect("the default is there").label, "4 × 2");
        s.overlay = Overlay::None;
        keys(&mut s, &[Key::Char(Action::AutoCompact.key())], &quiet());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        let default = list.items.iter().find(|i| i.detail.is_some()).expect("the default is there");
        assert_eq!(default.label, t!("{} %", "{}%", 75));
        assert!(list.items.iter().any(|i| i.label == t!("{} %", "{}%", 85)), "and the one in place");
        let places: Vec<u8> = list.items.iter().filter_map(|i| i.value.as_deref()?.parse().ok()).collect();
        assert!(places.is_sorted(), "in order: {places:?}");
    }

    #[test]
    fn the_compaction_share_is_chosen_from_a_list() {
        let mut s = Sheet::new(team(&[("dev", false)]));
        keys(&mut s, &[Key::Char(Action::AutoCompact.key())], &quiet());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        let labels: Vec<String> = list.items.iter().map(|i| i.label.clone()).collect();
        let at = |n: u8| t!("{} %", "{}%", n);
        assert_eq!(labels, [t!("non", "off"), at(50), at(60), at(70), at(80), at(90)]);
        assert_eq!(list.current, Some(3), "70 % in place");
        assert_eq!(list.items[3].detail, Some(t!("défaut", "default")));
        keys(&mut s, &[Key::Down], &quiet());
        let effects = s.key(Key::Enter, &quiet());
        assert!(
            matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::AutoCompact(Some(80))]),
            "{effects:?}"
        );
        // Off is the first: None.
        keys(&mut s, &[Key::Char(Action::AutoCompact.key()), Key::Home], &quiet());
        let effects = s.key(Key::Enter, &quiet());
        assert!(matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::AutoCompact(None)]));
    }

    #[test]
    fn a_share_from_the_files_finds_its_place() {
        let mut s = Sheet::new(team(&[("dev", false)]));
        s.team.layers.as_mut().unwrap().auto_compact.value = Some(75);
        keys(&mut s, &[Key::Char(Action::AutoCompact.key())], &quiet());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        let labels: Vec<&str> = list.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels[4], t!("{} %", "{}%", 75));
        assert_eq!(list.current, Some(4));
        s.team.layers.as_mut().unwrap().auto_compact.value = None;
        s.overlay = Overlay::None;
        keys(&mut s, &[Key::Char(Action::AutoCompact.key())], &quiet());
        let Overlay::List(list) = &s.overlay else { panic!("no list") };
        assert_eq!(list.current, Some(0), "off in place");
    }

    #[test]
    fn no_grid_with_a_tab_per_member() {
        let mut s = Sheet::new(team(&[("dev", false)]));
        assert!(s.actions().contains(&Action::Grid));
        s.team.layers.as_mut().unwrap().layout.value = config::Layout::Tabs;
        assert!(!s.actions().contains(&Action::Grid) && s.actions().contains(&Action::AutoCompact));
        assert!(!s.actions().contains(&Action::Layout), "nor a focus");
        keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
        assert!(matches!(s.overlay, Overlay::None));
    }

    #[test]
    fn the_layout_goes_from_grid_to_focus_and_back() {
        let mut s = Sheet::new(team(&[("dev", false)]));
        let effects = s.key(Key::Char(Action::Layout.key()), &quiet());
        assert!(
            matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Layout(config::Layout::Focus)]),
            "{effects:?}"
        );
        s.team.layers.as_mut().unwrap().layout.value = config::Layout::Focus;
        let effects = s.key(Key::Char(Action::Layout.key()), &quiet());
        assert!(
            matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Layout(config::Layout::Auto)])
        );
    }

    #[test]
    fn the_settings_wait_for_readable_files() {
        let mut s = Sheet::new(team(&[("dev", false)]));
        s.team.unreadable = Some("boom".into());
        s.team.layers = None;
        assert_eq!(s.actions(), [Action::Detach, Action::Stop]);
        s.team.unreadable = None;
        assert!(!s.actions().contains(&Action::Grid), "no layers, no setting to show");
    }

    #[test]
    fn hints() {
        let mut s = sheet();
        s.choose(Entry::Member("dev".into()));
        to_row(&mut s, Fid::Model);
        assert!(s.hint().unwrap().ends_with(&t!("effet dès sa prochaine requête", "in effect from its next request")));
        s.team.people[1].live = false;
        assert!(
            s.hint().unwrap().ends_with(&t!("il sera relancé sur sa conversation", "it restarts on its conversation")),
            "without the mod"
        );
        to_row(&mut s, Fid::Tab);
        assert!(s.hint().unwrap().starts_with(&t!("défaut : onglets", "default: \"Agents\"")), "{:?}", s.hint());
        to_row(&mut s, Fid::Contact);
        assert!(s.hint().unwrap().starts_with(&t!("défaut : non", "default: no")));
    }

    #[test]
    fn clicks() {
        let mut s = sheet();
        s.click(Target::Entry(Entry::Member("ops".into())), &quiet());
        assert_eq!((s.entry.clone(), s.pane), (Entry::Member("ops".into()), Pane::List));
        s.click(Target::Row(1), &quiet());
        assert_eq!(s.field(), Some(Fid::Effort));
        s.click(Target::Row(1), &quiet());
        assert!(matches!(s.overlay, Overlay::List(_)), "a second click opens it");
        let effects = s.click(Target::Item(0), &quiet());
        assert!(effects.len() <= 1);
        let effects = s.click(Target::Step(1, true), &quiet());
        let low = Edit::Set { member: "ops".into(), field: Field::Effort(Some("low".into())) };
        assert!(
            matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[low]),
            "carried by its requests: at once"
        );
        s.click(Target::Action(Action::RestartAll), &quiet());
        assert!(s.click(Target::Row(0), &quiet()).is_empty(), "a dialog stays until answered");
        assert_eq!(s.click(Target::Yes, &quiet()), [Effect::RestartAll]);
        s.click(Target::Restart(true), &quiet());
        assert!(matches!(s.overlay, Overlay::Confirm(_)), "afresh is asked");
        s.click(Target::No, &quiet());
        assert!(matches!(s.overlay, Overlay::None));
    }

    #[test]
    fn a_member_gone_from_the_files() {
        let mut s = sheet();
        s.team.people[2].gone = true;
        s.team.people[2].layers = None;
        s.choose(Entry::Member("ops".into()));
        assert_eq!(s.focusable(), [Fid::Close]);
        to_row(&mut s, Fid::Close);
        keys(&mut s, &[Key::Enter], &quiet());
        assert_eq!(keys(&mut s, &[Key::Left, Key::Enter], &quiet()), [Effect::Dismiss("ops".into())]);
    }

    #[test]
    fn files_that_do_not_read() {
        let mut t = team(&[("coordinateur", true), ("dev", false)]);
        t.unreadable = Some("settings.toml: ligne 3".into());
        for p in &mut t.people {
            p.layers = None;
        }
        let mut s = seen(t);
        assert!(matches!(&s.said, Some(Said::Failed(said)) if said.contains("ligne 3")), "said at once");
        assert!(!s.entries().contains(&Entry::New));
        assert!(s.focusable().is_empty(), "nothing to change");
        assert_eq!(s.actions(), [Action::Detach, Action::Stop]);
        keys(&mut s, &[Key::Enter, Key::Char('t'), Key::Char('n')], &quiet());
        assert_eq!(s.pane, Pane::List);
        assert!(matches!(s.overlay, Overlay::None), "nothing opened");
        keys(&mut s, &[Key::Char(Action::Stop.key())], &quiet());
        assert_eq!(keys(&mut s, &[Key::Left, Key::Enter], &quiet()), [Effect::Stop]);
    }

    #[test]
    fn instructions_edited() {
        let mut s = sheet();
        assert!(s.edited(Edited::Instructions("dev".into()), Err("vi : introuvable".into())).is_empty());
        assert!(matches!(&s.said, Some(Said::Failed(said)) if said.contains("introuvable")));
        assert!(s.edited(Edited::Instructions("dev".into()), Ok("\n".into())).is_empty(), "none before, none after");
        let effects = s.edited(Edited::Instructions("dev".into()), Ok("- tout\n".into()));
        let [Effect::Apply { edits, .. }] = effects.as_slice() else { panic!("{effects:?}") };
        assert_eq!(edits, &[Edit::Set { member: "dev".into(), field: Field::Instructions(Some("- tout".into())) }]);
        s.edited(Edited::Draft, Ok("  - brouillon \n".into()));
        assert_eq!(s.draft.instructions.as_deref(), Some("- brouillon"));
        assert_eq!(s.to_edit(&Edited::Draft), "- brouillon");
    }

    #[test]
    fn reloaded_after_a_change() {
        let mut s = sheet();
        s.choose(Entry::Member("ops".into()));
        s.reload(team(&[("coordinateur", true), ("dev", false)]), None);
        assert_eq!(
            (s.entry.clone(), s.pane),
            (Entry::Member("coordinateur".into()), Pane::List),
            "the first one, ops gone"
        );
        s.reload(team(&[("coordinateur", true), ("release", false)]), Some("release".into()));
        assert_eq!(s.entry, Entry::Member("release".into()));
    }

    #[test]
    fn short_texts() {
        assert_eq!(first_line("  Écrit le code\nen détail", 50), "Écrit le code");
        assert_eq!(first_line("abcdef ghij", 8), "abcdef…");
        if i18n::lang() == i18n::Lang::Fr {
            assert_eq!(quoted(&["a", "b", "c"]), "« a », « b » et « c »");
            assert_eq!(joined(&["a", "b"], "ou"), "« a » ou « b »");
            assert_eq!(instructions_count(None), "aucune");
        }
    }
}
