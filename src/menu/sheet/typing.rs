// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A text typed in place (a name, a role, a model), checked as it is typed.

use crate::config::{self, Edit, Field};
use crate::{claude::bridge, t};

use super::*;

impl Sheet {
    /// Opens a text typed in place over `initial`, on row `field`; what is wrong with it said at once, but for an
    /// empty one, said once typed or refused.
    pub(super) fn type_in(&mut self, field: Fid, initial: &str) {
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

    pub(super) fn typing_key(&mut self, mut typing: Typing, key: Key, env: &dyn Env) -> Vec<Effect> {
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
    pub(super) fn typed(&mut self, typing: Typing, env: &dyn Env) -> Vec<Effect> {
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
                let aliases: Vec<&str> = bridge::MODELS.iter().map(|model| model.alias).collect();
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
}
