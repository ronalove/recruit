// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Built-in teams: a project type and a size give a list of roles, written in the interface language.

use anyhow::{Result, bail};
use indexmap::IndexMap;
use serde::Deserialize;

use crate::config::{Member, Team};
use crate::i18n::{self, Lang};
use crate::t;

const CATALOG: &str = include_str!("../templates/catalog.toml");
const FR: &str = include_str!("../templates/fr.toml");
const EN: &str = include_str!("../templates/en.toml");

#[derive(Deserialize)]
struct Catalog {
    sizes: Vec<String>,
    /// The roles the user talks to.
    contacts: Vec<String>,
    /// Type → size → role ids.
    types: IndexMap<String, IndexMap<String, Vec<String>>>,
}

#[derive(Deserialize)]
struct Texts {
    team_instructions: String,
    types: IndexMap<String, Label>,
    sizes: IndexMap<String, Label>,
    roles: IndexMap<String, RoleText>,
}

#[derive(Deserialize, Clone)]
pub struct Label {
    pub label: String,
    pub hint: String,
}

#[derive(Deserialize)]
struct RoleText {
    name: String,
    role: String,
    instructions: String,
}

pub struct Templates {
    lang: Lang,
    catalog: Catalog,
    texts: Texts,
}

impl Templates {
    pub fn load() -> Self {
        Self::for_lang(i18n::lang())
    }

    pub fn for_lang(lang: Lang) -> Self {
        let texts = match lang {
            Lang::Fr => FR,
            Lang::En => EN,
        };
        Templates {
            lang,
            catalog: toml::from_str(CATALOG).expect("templates/catalog.toml"),
            texts: toml::from_str(texts).expect("templates/<lang>.toml"),
        }
    }

    /// Project types, in catalog order: (id, label).
    pub fn types(&self) -> Vec<(&str, &Label)> {
        self.catalog.types.keys().map(|id| (id.as_str(), &self.texts.types[id])).collect()
    }

    pub fn sizes(&self) -> Vec<(&str, &Label)> {
        self.catalog.sizes.iter().map(|id| (id.as_str(), &self.texts.sizes[id])).collect()
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Shared rules every built-in team starts with; also given to teams composed by Claude or by hand.
    pub fn team_instructions(&self) -> &str {
        &self.texts.team_instructions
    }

    /// Every built-in role: (member name, one-line role). Claude names the members it composes after them.
    pub fn roles(&self) -> Vec<(&str, &str)> {
        self.texts.roles.values().map(|r| (r.name.as_str(), r.role.trim())).collect()
    }

    /// Every built-in role as a member, in the catalog's order, each name once: its role and instructions, a contact
    /// when the catalog makes it one. A running team's menu adds them as they are.
    pub fn members(&self) -> Vec<(String, Member)> {
        let mut members: Vec<(String, Member)> = Vec::new();
        for (id, text) in &self.texts.roles {
            if members.iter().any(|(name, _)| *name == text.name) {
                continue;
            }
            let member = Member {
                role: text.role.trim().to_string(),
                contact: self.catalog.contacts.contains(id),
                instructions: Some(text.instructions.trim().to_string()).filter(|i| !i.is_empty()),
                ..Default::default()
            };
            members.push((text.name.clone(), member));
        }
        members
    }

    /// The team of a type and size.
    pub fn build(&self, kind: &str, size: &str) -> Result<Team> {
        let Some(sizes) = self.catalog.types.get(kind) else {
            let known: Vec<&str> = self.catalog.types.keys().map(String::as_str).collect();
            bail!(t!("type inconnu « {} » (connus : {})", "unknown type \"{}\" (known: {})", kind, known.join(", ")));
        };
        let Some(roles) = sizes.get(size) else {
            bail!(t!(
                "taille inconnue « {} » (connues : {})",
                "unknown size \"{}\" (known: {})",
                size,
                self.catalog.sizes.join(", ")
            ));
        };
        let mut team = Team {
            description: Some(format!(
                "{}, {}",
                self.texts.types[kind].label,
                self.texts.sizes[size].label.to_lowercase()
            )),
            lang: Some(self.lang),
            instructions: Some(self.texts.team_instructions.trim().to_string()),
            ..Default::default()
        };
        for id in roles {
            let text = &self.texts.roles[id];
            team.members.insert(
                text.name.clone(),
                Member {
                    role: text.role.trim().to_string(),
                    contact: self.catalog.contacts.contains(id),
                    instructions: Some(text.instructions.trim().to_string()),
                    ..Default::default()
                },
            );
        }
        Ok(team)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;

    #[test]
    fn every_team_builds_in_both_languages() {
        for lang in [Lang::Fr, Lang::En] {
            let templates = Templates::for_lang(lang);
            for (kind, _) in templates.types() {
                for (size, _) in templates.sizes() {
                    let team = templates.build(kind, size).unwrap();
                    let expected = templates.catalog.types[kind][size].len();
                    assert_eq!(team.members.len(), expected, "{lang:?} {kind} {size}: duplicate member names");
                    assert!(team.members.values().all(|m| !m.role.is_empty()));
                    for name in team.members.keys() {
                        crate::config::validate_member_name(name).unwrap();
                    }
                    // Every tab fits the default 3 × 2 grid.
                    assert!(layout::tabs(&team).iter().all(|t| t.members.len() <= 6));
                    assert_eq!(team.contacts().len(), 1, "{lang:?} {kind} {size}: the coordinator is the contact");
                }
            }
        }
    }

    #[test]
    fn built_in_roles_as_members() {
        for lang in [Lang::Fr, Lang::En] {
            let templates = Templates::for_lang(lang);
            let members = templates.members();
            assert_eq!(members.len(), templates.roles().len(), "{lang:?}");
            for (i, (name, member)) in members.iter().enumerate() {
                crate::config::validate_member_name(name).unwrap();
                assert!(members[..i].iter().all(|(other, _)| other != name), "{lang:?}: {name} twice");
                assert!(!member.role.is_empty() && member.instructions.is_some(), "{lang:?}: {name}");
            }
            // As the teams built from them have them.
            let team = templates.build("personal", "small").unwrap();
            for (name, member) in &team.members {
                let built_in = &members.iter().find(|(n, _)| n == name).unwrap().1;
                assert_eq!((&built_in.role, built_in.contact), (&member.role, member.contact), "{lang:?}: {name}");
            }
        }
    }

    #[test]
    fn small_personal_team_matches_the_spec() {
        let team = Templates::for_lang(Lang::Fr).build("personal", "small").unwrap();
        let names: Vec<&str> = team.members.keys().map(String::as_str).collect();
        assert_eq!(names, ["coordinateur", "développeur", "designer"]);
        assert_eq!(team.contacts(), ["coordinateur"]);
        let tabs: Vec<(String, usize)> = layout::tabs(&team).into_iter().map(|t| (t.title, t.members.len())).collect();
        assert_eq!(tabs, [("Interlocuteurs".into(), 1), ("Agents".into(), 2)]);
    }

    #[test]
    fn large_team_splits_its_agents() {
        let team = Templates::for_lang(Lang::En).build("web", "large").unwrap();
        let tabs: Vec<(String, usize)> = layout::tabs(&team).into_iter().map(|t| (t.title, t.members.len())).collect();
        assert_eq!(tabs, [("Contacts".into(), 1), ("Agents (1)".into(), 4), ("Agents (2)".into(), 3)]);
    }
}
