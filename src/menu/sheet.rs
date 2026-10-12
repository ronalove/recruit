// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The menu's state, apart from the terminal and the team's files: the team as the menu shows it, where the user is
//! on the screen, and what each key or click does. What must touch the team, the terminal or another program comes
//! back as an [`Effect`], for `menu.rs` to carry out.

use std::collections::HashMap;

use crossterm::style::Color;

use crate::config::{self, Edit, Field, Layered, Member, MemberLayers, Origin, TeamLayers};
use crate::ui::look::{self, State};
use crate::{claude::bridge, t};

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
    /// A member whose turn failed: « continue » sent to it, its prompt left as it is; nothing sent if the prompt
    /// holds a draft.
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
            Setting::Model => bridge::MODELS.iter().map(|m| m.alias.to_string()).collect(),
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
    /// The version of recruit the team's server runs, at the top of the menu.
    pub version: String,
    /// A published version newer than that one, if the last check found one.
    pub update: Option<String>,
}

#[cfg(test)]
pub(crate) mod fixture;
mod keys;
mod lists;
mod members;
mod mouse;
mod team;
#[cfg(test)]
mod tests;
mod text;
mod typing;
mod view;

use text::mode_detail;
pub(crate) use text::{model_label, quoted};
