// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The system prompt each member receives: who they are, their role, their teammates, the shared rules.

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::{Team, cache_dir, write_atomic};
use crate::i18n::{self, Lang};
use crate::t;

/// The prompts of a team's session.
pub fn folder(session: &str) -> PathBuf {
    cache_dir().join("prompts").join(session)
}

/// Writes a member's prompt in its team's folder, in a file named after the member and the text: a file never gets
/// another text, the one a conversation started with stays as that conversation got it. Returns the file.
pub fn write(session: &str, member: &str, text: &str) -> Result<PathBuf> {
    write_in(&folder(session), member, text)
}

fn write_in(dir: &Path, member: &str, text: &str) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| t!("création de {}", "creating {}", dir.display()))?;
    let stem: String = member
        .chars()
        .enumerate()
        .map(|(i, c)| if c.is_alphanumeric() || c == '-' || c == '_' || (c == '.' && i > 0) { c } else { '_' })
        .collect();
    let file = dir.join(format!("{stem}-{:016x}.md", fnv(text)));
    if fs::read_to_string(&file).ok().as_deref() != Some(text) {
        write_atomic(&file, text)?;
    }
    Ok(file)
}

/// FNV-1a, 64 bits: the same name for the same text from one version of recruit to the next.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// `session`: the team's tmux session, which ListAgents shows on each of its members' lines.
pub fn build(team_name: &str, session: &str, team: &Team, member_name: &str) -> String {
    let lang = team.lang.unwrap_or_else(i18n::lang);
    let member = &team.members[member_name];
    let description = team.description.as_deref().map(str::trim).filter(|d| !d.is_empty());
    let mut out = String::new();

    let _ = match (lang, description) {
        (Lang::Fr, Some(d)) => write!(out, "Tu es « {member_name} », membre de l'équipe « {team_name} » ({d})."),
        (Lang::Fr, None) => write!(out, "Tu es « {member_name} », membre de l'équipe « {team_name} »."),
        (Lang::En, Some(d)) => write!(out, "You are \"{member_name}\", a member of the \"{team_name}\" team ({d})."),
        (Lang::En, None) => write!(out, "You are \"{member_name}\", a member of the \"{team_name}\" team."),
    };
    out.push(' ');
    out.push_str(match lang {
        Lang::Fr => {
            "Tes coéquipiers sont d'autres sessions Claude Code ouvertes en même temps que toi dans le même dossier ; \
             écris-leur avec l'outil SendMessage en utilisant leur nom comme adresse (ListAgents les liste)."
        }
        Lang::En => {
            "Your teammates are other Claude Code sessions running at the same time as you, in the same directory; \
             write to them with the SendMessage tool, using their name as the address (ListAgents lists them)."
        }
    });
    // Another team may run members with the same names: its tmux session tells them apart. A reply needs nothing:
    // SendMessage answers the exact address the message came from.
    if team.members.len() > 1 {
        out.push(' ');
        let _ = match lang {
            Lang::Fr => write!(
                out,
                "Ton équipe tourne dans la session tmux « {session} » : si ListAgents montre plusieurs sessions sous un \
                 même nom (une autre équipe peut avoir les mêmes), écris à celle dont la ligne indique \
                 « tmux {session}:… », à l'adresse « nom [ref] » de cette ligne."
            ),
            Lang::En => write!(
                out,
                "Your team runs in the tmux session \"{session}\": if ListAgents shows several sessions under one name \
                 (another team may use the same names), write to the one whose line shows \"tmux {session}:…\", at \
                 that line's \"name [ref]\" address."
            ),
        };
    }

    // Contacts talk with the user; working agents get their work from the contacts, and must not wait for an
    // answer in a terminal nobody watches all the time.
    let contacts = team.contacts();
    out.push_str("\n\n");
    if contacts.contains(&member_name) {
        out.push_str(match lang {
            Lang::Fr => {
                "Tu es l'un des interlocuteurs de l'utilisateur : il te parle directement dans ton terminal. Les autres \
                 membres sont des agents de travail : ils reçoivent leur travail des interlocuteurs et leur rendent compte."
            }
            Lang::En => {
                "You are one of the user's contacts: the user talks to you directly in your terminal. The other members \
                 are working agents: they get their work from the contacts and report back to them."
            }
        });
    } else {
        let names = list(lang, &contacts);
        let _ = match lang {
            Lang::Fr => write!(
                out,
                "Tu es un agent de travail : l'utilisateur ne te parle pas d'habitude. Ton travail te vient des \
                 interlocuteurs de l'équipe ({names}), et c'est à eux que tu rends compte. Si tu as une question ou si tu \
                 es bloqué, pose-la-leur avec SendMessage au lieu d'attendre une réponse dans ton terminal : \
                 l'utilisateur ne le regarde pas en permanence. Il peut parfois intervenir dans ton terminal : fais alors \
                 ce qu'il demande, puis préviens les interlocuteurs."
            ),
            Lang::En => write!(
                out,
                "You are a working agent: the user does not usually talk to you. Your work comes from the team's contacts \
                 ({names}), and you report back to them. When you have a question or are stuck, ask them with \
                 SendMessage instead of waiting for an answer in your terminal: the user does not watch it all the \
                 time. The user may sometimes step in in your terminal: then do what they ask, and let the contacts know."
            ),
        };
    }

    out.push_str(match lang {
        Lang::Fr => "\n\n## Ton rôle\n\n",
        Lang::En => "\n\n## Your role\n\n",
    });
    out.push_str(member.role.trim());
    if let Some(instructions) = member.instructions.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        out.push_str("\n\n");
        out.push_str(instructions);
    }

    out.push_str(match lang {
        Lang::Fr => "\n\n## L'équipe\n",
        Lang::En => "\n\n## The team\n",
    });
    for (name, m) in &team.members {
        let role = m.role.trim().lines().next().unwrap_or_default();
        let mut marks = Vec::new();
        if contacts.contains(&name.as_str()) {
            marks.push(if lang == Lang::Fr { "interlocuteur" } else { "contact" });
        }
        if name == member_name {
            marks.push(if lang == Lang::Fr { "toi" } else { "you" });
        }
        let marks = if marks.is_empty() { String::new() } else { format!(" ({})", marks.join(", ")) };
        let _ = match lang {
            Lang::Fr => write!(out, "\n- « {name} »{marks} : {role}"),
            Lang::En => write!(out, "\n- \"{name}\"{marks}: {role}"),
        };
    }

    if let Some(rules) = team.instructions.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        out.push_str(match lang {
            Lang::Fr => "\n\n## Règles communes\n\n",
            Lang::En => "\n\n## Shared rules\n\n",
        });
        out.push_str(rules);
    }
    out.push('\n');
    out
}

/// « a », « b » et « c » / "a", "b" and "c".
fn list(lang: Lang, names: &[&str]) -> String {
    let quoted: Vec<String> = names
        .iter()
        .map(|n| match lang {
            Lang::Fr => format!("« {n} »"),
            Lang::En => format!("\"{n}\""),
        })
        .collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => {
            format!("{} {} {last}", rest.join(", "), if lang == Lang::Fr { "et" } else { "and" })
        }
        _ => quoted.join(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Member;

    #[test]
    fn french_prompt() {
        let mut team = Team {
            description: Some("projet perso".into()),
            lang: Some(Lang::Fr),
            instructions: Some("- Personne ne commite sans demande.".into()),
            ..Default::default()
        };
        for (name, role) in [("coordinateur", "Répartit le travail"), ("développeur", "Écrit le code")] {
            team.members.insert(name.into(), Member { role: role.into(), ..Default::default() });
        }
        team.members["développeur"].instructions = Some("- Teste avant de rendre.".into());
        let prompt = build("perso", "perso", &team, "développeur");
        assert!(prompt.starts_with("Tu es « développeur », membre de l'équipe « perso » (projet perso)."));
        assert!(prompt.contains("Tu es un agent de travail"));
        assert!(prompt.contains("des interlocuteurs de l'équipe (« coordinateur »)"));
        assert!(prompt.contains("## Ton rôle\n\nÉcrit le code\n\n- Teste avant de rendre."));
        assert!(prompt.contains(
            "- « coordinateur » (interlocuteur) : Répartit le travail\n- « développeur » (toi) : Écrit le code"
        ));
        assert!(prompt.ends_with("## Règles communes\n\n- Personne ne commite sans demande.\n"));

        // With no member marked, the first one is the contact.
        let lead = build("perso", "perso", &team, "coordinateur");
        assert!(lead.contains("Tu es l'un des interlocuteurs de l'utilisateur"));
        assert!(lead.contains("- « coordinateur » (interlocuteur, toi) : Répartit le travail"));
    }

    #[test]
    fn several_contacts_are_named() {
        let mut team = Team { lang: Some(Lang::Fr), ..Default::default() };
        for (name, contact) in [("coordinateur", true), ("dev", false), ("opérateur", true)] {
            team.members.insert(name.into(), Member { role: "r".into(), contact, ..Default::default() });
        }
        assert!(build("t", "t", &team, "dev").contains("(« coordinateur » et « opérateur »)"));
        assert_eq!(list(Lang::En, &["a", "b", "c"]), "\"a\", \"b\" and \"c\"");
    }

    #[test]
    fn english_prompt_without_optional_parts() {
        let mut team = Team { lang: Some(Lang::En), ..Default::default() };
        team.members.insert("solo".into(), Member { role: "Everything".into(), ..Default::default() });
        let prompt = build("t", "t", &team, "solo");
        assert!(prompt.starts_with("You are \"solo\", a member of the \"t\" team. Your teammates"));
        assert!(prompt.contains("You are one of the user's contacts"));
        assert!(prompt.ends_with("- \"solo\" (contact, you): Everything\n"));
    }

    #[test]
    fn prompt_files_never_change() {
        let dir = tempfile::tempdir().unwrap();
        let write = |member: &str, text: &str| write_in(dir.path(), member, text).unwrap();
        let first = write("dev", "Tu es dev.");
        assert_eq!(write("dev", "Tu es dev."), first);
        let second = write("dev", "Tu es dev, renommé.");
        assert_ne!(second, first);
        assert_eq!(fs::read_to_string(&first).unwrap(), "Tu es dev.");
        assert!(folder("web").ends_with("recruit/prompts/web"));
        let odd = write("../x y", "t");
        assert_eq!(odd.parent(), first.parent());
        assert!(odd.file_name().unwrap().to_string_lossy().starts_with("_._x_y-"));
        assert_eq!(fnv(""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn same_names_are_told_apart_by_tmux_session() {
        let mut team = Team::default();
        for name in ["coordinateur", "dev"] {
            team.members.insert(name.into(), Member { role: "r".into(), ..Default::default() });
        }
        team.lang = Some(Lang::Fr);
        for member in ["coordinateur", "dev"] {
            assert!(build("omni.dex", "omni_dex", &team, member).contains(
                "(ListAgents les liste). Ton équipe tourne dans la session tmux « omni_dex » : si ListAgents montre \
                 plusieurs sessions sous un même nom (une autre équipe peut avoir les mêmes), écris à celle dont la \
                 ligne indique « tmux omni_dex:… », à l'adresse « nom [ref] » de cette ligne.\n\n"
            ));
        }
        team.lang = Some(Lang::En);
        for member in ["coordinateur", "dev"] {
            assert!(build("omni.dex", "omni_dex", &team, member).contains(
                "(ListAgents lists them). Your team runs in the tmux session \"omni_dex\": if ListAgents shows several \
                 sessions under one name (another team may use the same names), write to the one whose line shows \
                 \"tmux omni_dex:…\", at that line's \"name [ref]\" address.\n\n"
            ));
        }

        // Alone, no teammate to tell apart.
        team.members.shift_remove("dev");
        assert!(!build("omni.dex", "omni_dex", &team, "coordinateur").contains("tmux"));
    }
}
