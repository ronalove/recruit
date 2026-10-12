// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What changing the members does: contacts, restarts, removing, adding, the editor's text, the team's actions.

use crate::config::{self, Edit, Field, MAX_CONTACTS, Member};
use crate::{i18n, t};

use super::text::*;
use super::*;

impl Sheet {
    /// `edits` to apply, asked first when they start again a member who may be interrupted.
    pub(super) fn confirmed(
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
        let why = restart_reason(fid);
        let title = match restarts {
            [one] => t!("Relancer « {} » ?", "Restart \"{}\"?", one),
            _ => t!("Relancer {} ?", "Restart {}?", quoted(restarts)),
        };
        let again = t!("{} repartira sur sa conversation.", "{} starts again on its conversation.", quoted(restarts));
        let mut confirm = Confirm::new(title, vec![why, again], t!("Relancer maintenant", "Restart now"), vec![effect]);
        if self.quick.is_some() {
            // In the quick list: one note, short buttons, each with its key.
            let note = quick_note(busy.first().map(|(state, _)| *state));
            confirm.lines = vec![note];
            busy = Vec::new();
            confirm.yes = t!("Relancer", "Restart");
            confirm.letters = Some(('r', t!("a", "c").chars().next().unwrap_or('a')));
        }
        self.overlay = Overlay::Confirm(confirm.busy(busy));
        Vec::new()
    }

    pub(super) fn contact(&mut self, on: bool, env: &dyn Env) -> Vec<Effect> {
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
                    i18n::joined(i18n::lang(), &contacts, &t!("ou", "or"))
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

    pub(super) fn restart(&mut self) -> Vec<Effect> {
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

    pub(super) fn remove(&mut self) -> Vec<Effect> {
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

    pub(super) fn confirm_remove(&mut self, name: String, replacement: Option<String>) {
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

    pub(super) fn add(&mut self) -> Vec<Effect> {
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

    pub(super) fn action(&mut self, action: Action) -> Vec<Effect> {
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
}
