// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What each key does on the sheet: moving, changing a value in place, opening a list or an editor.

use crate::config::{Edit, Member};
use crate::{i18n, t};

use super::*;

impl Sheet {
    pub(crate) fn key(&mut self, key: Key, env: &dyn Env) -> Vec<Effect> {
        let effects = self.key_in(key, env);
        self.settled(effects)
    }

    /// In the quick list, the first answer ends it: a list, a question or a text given up, with nothing to carry out.
    pub(super) fn settled(&mut self, effects: Vec<Effect>) -> Vec<Effect> {
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
    pub(super) fn opened(&mut self) {
        if self.entry != Entry::New || self.roles.is_some() {
            return;
        }
        let taken = |name: &str| self.team.config.members.keys().any(|n| n.to_lowercase() == name.to_lowercase());
        let roles: Vec<(String, Member)> = crate::create::templates::Templates::for_lang(i18n::lang())
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
    pub(super) fn step(&mut self, fid: Fid, forward: bool, env: &dyn Env) -> Vec<Effect> {
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
    pub(super) fn edits(&self, setting: Setting, choice: &Choice) -> Vec<Edit> {
        vec![Edit::Set { member: self.name(), field: setting.field(choice.value.clone()) }]
    }

    /// ⏎ on a row.
    pub(super) fn enter(&mut self, fid: Fid, env: &dyn Env) -> Vec<Effect> {
        if let Some((field, at)) = self.pending.take()
            && field == fid
        {
            let choice = self.choices(fid, false)[at].clone();
            return self.apply_choice(fid, &choice, env);
        }
        match fid {
            Fid::Resume => return self.resume(),
            Fid::Model | Fid::Effort | Fid::Mode | Fid::Tab | Fid::Contact | Fid::Builtin => self.open_values(fid),
            Fid::How => {}
            Fid::Name => self.type_in(Fid::Name, &self.name()),
            Fid::Role => {
                let role = self.person().and_then(|p| p.layers.as_ref()).map(|l| l.role.value.trim().to_string());
                self.type_in(Fid::Role, &role.unwrap_or_default())
            }
            Fid::Instructions => return vec![Effect::Editor(Edited::Instructions(self.name()))],
            Fid::Restart => return self.restart(),
            Fid::Remove => return self.remove(),
            Fid::Close => self.ask_to_close(),
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

    /// « Reprendre »: asks the member's mod to send « continue », once, until it has answered.
    fn resume(&mut self) -> Vec<Effect> {
        let name = self.name();
        // Once asked, not again while its mod has not answered.
        if self.resuming.as_ref().is_some_and(|(asked, how)| *asked == name && *how == Resuming::Asked) {
            return Vec::new();
        }
        self.resuming = Some((name.clone(), Resuming::Asked));
        vec![Effect::Resume(name)]
    }

    /// Opens the list of the values of `fid`, on the current one.
    fn open_values(&mut self, fid: Fid) {
        let items = self.choices(fid, true);
        if items.is_empty() {
            return;
        }
        let current = self.current(fid);
        let at = current.unwrap_or(0);
        self.overlay = Overlay::List(Dropdown { field: Some(fid), items, at, current, purpose: Purpose::Value });
    }

    /// Asks to close the pane of a member the files no longer have.
    fn ask_to_close(&mut self) {
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

    /// Claude asked to compose the draft's request, as a new request.
    pub(super) fn compose(&mut self) -> Vec<Effect> {
        self.composes += 1;
        self.draft.composing = Some(self.composes);
        vec![Effect::Compose { id: self.composes, request: self.draft.request.trim().to_string() }]
    }

    /// A value picked, in a list or prepared: written, once confirmed when it interrupts someone.
    pub(super) fn apply_choice(&mut self, fid: Fid, choice: &Choice, env: &dyn Env) -> Vec<Effect> {
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
}
