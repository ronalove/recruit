// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the sheet shows: the rows and values of the chosen member's fields, their origins, the line under the sheet.

use std::collections::HashMap;

use crate::config::{self, Layered};
use crate::ui::look::State;
use crate::{claude::bridge, t};

use super::text::*;
use super::*;

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
            version: crate::mux::proto::VERSION.to_string(),
            update: None,
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
    /// A member whose turn failed opens on « Reprendre », unless a field is asked for.
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

    pub(super) fn name(&self) -> String {
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
                // Its turn failed: taking up its work first.
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
    pub(super) fn setting(&self, fid: Fid) -> Option<(Setting, &Layered<Option<String>>)> {
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
            Some(value) => t!(
                "défaut : {} ({})",
                "default: {} ({})",
                setting.label(value),
                source_name(*source, &self.team.origin)
            ),
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
    pub(super) fn applied(&self, setting: Setting, p: &Person) -> Option<String> {
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
            return Some(source_name(layered.source, &self.team.origin));
        }
        let layers = self.person()?.layers.as_ref()?;
        let source = match fid {
            Fid::Contact => layers.contact.source,
            Fid::Name | Fid::Role => layers.role.source,
            Fid::Instructions if layers.instructions.value.is_some() => layers.instructions.source,
            _ => return None,
        };
        Some(source_name(source, &self.team.origin))
    }

    /// The line under the sheet: what the focused row's default gives, and when a change takes effect.
    pub(crate) fn hint(&self) -> Option<String> {
        if let Overlay::Typing(typing) = &self.overlay {
            return Some(self.typing_hint(typing.field));
        }
        let fid = self.field()?;
        self.setting_hint(fid).or_else(|| self.field_hint(fid))
    }

    /// What a text typed in place for `field` takes.
    fn typing_hint(&self, field: Fid) -> String {
        match field {
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
        }
    }

    /// The line under a setting (model, effort, mode, tab): what its default gives and when a change takes effect; none
    /// for the other rows.
    fn setting_hint(&self, fid: Fid) -> Option<String> {
        let pending = self.pending.is_some_and(|(f, _)| f == fid);
        let p = self.person();
        let restarts = t!("il sera relancé sur sa conversation", "it restarts on its conversation");
        let (setting, layered) = self.setting(fid)?;
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
        Some(format!("{default} · {when}"))
    }

    /// The line under any other row of the sheet.
    fn field_hint(&self, fid: Fid) -> Option<String> {
        let p = self.person();
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
    pub(super) fn interrupted(&self, members: &[String]) -> Vec<(State, String)> {
        let of = |state: State| -> Vec<String> {
            members.iter().filter(|m| self.state(m).is_some_and(|(s, _)| s == state)).cloned().collect()
        };
        let unknown = if self.states_known { Vec::new() } else { members.to_vec() };
        [(State::Working, of(State::Working)), (State::Waiting, of(State::Waiting)), (State::Other, unknown)]
            .into_iter()
            .filter(|(_, names)| !names.is_empty())
            .map(|(state, names)| (state, interruption(state, names.len() == 1, &quoted(&names))))
            .collect()
    }

    // -- Keys and clicks ----------------------------------------------------------------------------------------
}
