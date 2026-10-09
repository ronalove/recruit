// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The system prompt each member receives: who they are, their role, their teammates, the shared rules.

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::backend::Kind;
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

/// `session`: the team's tmux session, which ListAgents shows on each of its members' lines under tmux; `backend`:
/// the multiplexer the team runs in, recruit's own showing no such line.
pub fn build(team_name: &str, session: &str, team: &Team, member_name: &str, backend: Kind) -> String {
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
    // Another team may run members with the same names: recruit gives the exact addresses of those teammates in the
    // team's note (`addresses`), and the tmux session tells them apart when that fails. A reply needs nothing:
    // SendMessage answers the exact address the message came from.
    if team.members.len() > 1 {
        let contact = team.contacts().contains(&member_name);
        out.push(' ');
        let who = match (lang, contact) {
            (Lang::Fr, true) => "l'utilisateur",
            (Lang::Fr, false) => "ton interlocuteur",
            (Lang::En, true) => "the user",
            (Lang::En, false) => "your contact",
        };
        let _ = match (lang, backend) {
            (Lang::Fr, Kind::Tmux) => write!(
                out,
                "Ton équipe tourne dans la session tmux « {session} ». Si ListAgents montre plusieurs sessions sous un \
                 même nom (une autre équipe peut avoir les mêmes), ou qu'un envoi par nom est refusé pour cette \
                 raison, écris à l'adresse exacte « nom [ref] » que recruit te donne pour ce coéquipier ; si elle \
                 échoue aussi, ou que recruit n'en donne pas, à celle de la ligne de ListAgents qui indique \
                 « tmux {session}:… » ; à défaut, demande-la à {who}."
            ),
            (Lang::En, Kind::Tmux) => write!(
                out,
                "Your team runs in the tmux session \"{session}\". If ListAgents shows several sessions under one name \
                 (another team may use the same names), or a message by name is refused for that reason, write to \
                 the exact \"name [ref]\" address recruit gives you for that teammate; if it fails too, or recruit \
                 gives none, to the address on the ListAgents line that shows \"tmux {session}:…\"; failing that, \
                 ask {who} for it."
            ),
            (Lang::Fr, Kind::Native) => write!(
                out,
                "Si ListAgents montre plusieurs sessions sous un même nom (une autre équipe peut avoir les mêmes), ou \
                 qu'un envoi par nom est refusé pour cette raison, écris à l'adresse exacte « nom [ref] » que recruit \
                 te donne pour ce coéquipier ; si elle échoue aussi, ou que recruit n'en donne pas, demande-la à {who}."
            ),
            (Lang::En, Kind::Native) => write!(
                out,
                "If ListAgents shows several sessions under one name (another team may use the same names), or a \
                 message by name is refused for that reason, write to the exact \"name [ref]\" address recruit gives \
                 you for that teammate; if it fails too, or recruit gives none, ask {who} for it."
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

/// For the team's note, when other sessions carry the names of some teammates: their exact addresses (`name [ref]`,
/// as ListAgents writes them), in the team's language. None without any.
pub fn addresses(team: &Team, refs: &[(String, String)]) -> Option<String> {
    if refs.is_empty() {
        return None;
    }
    let list: Vec<String> = refs.iter().map(|(name, reference)| format!("{name} [{reference}]")).collect();
    let list = list.join(", ");
    Some(match team.lang.unwrap_or_else(i18n::lang) {
        Lang::Fr => {
            format!("Adresses exactes de tes coéquipiers dont le nom est porté par une autre session : {list}.")
        }
        Lang::En => format!("Exact addresses of your teammates whose name another session also carries: {list}."),
    })
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
        let prompt = build("perso", "perso", &team, "développeur", Kind::Tmux);
        assert!(prompt.starts_with("Tu es « développeur », membre de l'équipe « perso » (projet perso)."));
        assert!(prompt.contains("Tu es un agent de travail"));
        assert!(prompt.contains("des interlocuteurs de l'équipe (« coordinateur »)"));
        assert!(prompt.contains("## Ton rôle\n\nÉcrit le code\n\n- Teste avant de rendre."));
        assert!(prompt.contains(
            "- « coordinateur » (interlocuteur) : Répartit le travail\n- « développeur » (toi) : Écrit le code"
        ));
        assert!(prompt.ends_with("## Règles communes\n\n- Personne ne commite sans demande.\n"));

        // With no member marked, the first one is the contact.
        let lead = build("perso", "perso", &team, "coordinateur", Kind::Tmux);
        assert!(lead.contains("Tu es l'un des interlocuteurs de l'utilisateur"));
        assert!(lead.contains("- « coordinateur » (interlocuteur, toi) : Répartit le travail"));
    }

    #[test]
    fn several_contacts_are_named() {
        let mut team = Team { lang: Some(Lang::Fr), ..Default::default() };
        for (name, contact) in [("coordinateur", true), ("dev", false), ("opérateur", true)] {
            team.members.insert(name.into(), Member { role: "r".into(), contact, ..Default::default() });
        }
        assert!(build("t", "t", &team, "dev", Kind::Tmux).contains("(« coordinateur » et « opérateur »)"));
        assert_eq!(list(Lang::En, &["a", "b", "c"]), "\"a\", \"b\" and \"c\"");
    }

    #[test]
    fn english_prompt_without_optional_parts() {
        let mut team = Team { lang: Some(Lang::En), ..Default::default() };
        team.members.insert("solo".into(), Member { role: "Everything".into(), ..Default::default() });
        let prompt = build("t", "t", &team, "solo", Kind::Tmux);
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
            let who = if member == "coordinateur" { "l'utilisateur" } else { "ton interlocuteur" };
            assert!(build("omni.dex", "omni_dex", &team, member, Kind::Tmux).contains(&format!(
                "(ListAgents les liste). Ton équipe tourne dans la session tmux « omni_dex ». Si ListAgents montre \
                 plusieurs sessions sous un même nom (une autre équipe peut avoir les mêmes), ou qu'un envoi par nom \
                 est refusé pour cette raison, écris à l'adresse exacte « nom [ref] » que recruit te donne pour ce \
                 coéquipier ; si elle échoue aussi, ou que recruit n'en donne pas, à celle de la ligne de ListAgents \
                 qui indique « tmux omni_dex:… » ; à défaut, demande-la à {who}.\n\n"
            )));
        }
        team.lang = Some(Lang::En);
        for member in ["coordinateur", "dev"] {
            let who = if member == "coordinateur" { "the user" } else { "your contact" };
            assert!(build("omni.dex", "omni_dex", &team, member, Kind::Tmux).contains(&format!(
                "(ListAgents lists them). Your team runs in the tmux session \"omni_dex\". If ListAgents shows \
                 several sessions under one name (another team may use the same names), or a message by name is \
                 refused for that reason, write to the exact \"name [ref]\" address recruit gives you for that \
                 teammate; if it fails too, or recruit gives none, to the address on the ListAgents line that shows \
                 \"tmux omni_dex:…\"; failing that, ask {who} for it.\n\n"
            )));
        }

        // In recruit's own multiplexer, no tmux line to fall back on: ask.
        team.lang = Some(Lang::Fr);
        let native = build("omni.dex", "omni_dex", &team, "dev", Kind::Native);
        assert!(native.contains(
            "(ListAgents les liste). Si ListAgents montre plusieurs sessions sous un même nom (une autre équipe peut \
             avoir les mêmes), ou qu'un envoi par nom est refusé pour cette raison, écris à l'adresse exacte \
             « nom [ref] » que recruit te donne pour ce coéquipier ; si elle échoue aussi, ou que recruit n'en donne \
             pas, demande-la à ton interlocuteur.\n\n"
        ));
        assert!(!native.contains("tmux") && !native.contains("omni_dex"), "{native}");
        team.lang = Some(Lang::En);
        let native = build("omni.dex", "omni_dex", &team, "coordinateur", Kind::Native);
        assert!(native.contains("if it fails too, or recruit gives none, ask the user for it.\n\n"), "{native}");
        assert!(!native.contains("tmux"), "{native}");

        // Alone, no teammate to tell apart.
        team.members.shift_remove("dev");
        assert!(!build("omni.dex", "omni_dex", &team, "coordinateur", Kind::Tmux).contains("tmux"));
    }

    #[test]
    fn exact_addresses_only_when_names_are_carried_twice() {
        let mut team = Team { lang: Some(Lang::Fr), ..Default::default() };
        assert_eq!(addresses(&team, &[]), None);
        let refs = [("dev-rust".to_string(), "a1b2c3".to_string()), ("qa".to_string(), "d4e5f6".to_string())];
        assert_eq!(
            addresses(&team, &refs).unwrap(),
            "Adresses exactes de tes coéquipiers dont le nom est porté par une autre session : dev-rust [a1b2c3], \
             qa [d4e5f6]."
        );
        team.lang = Some(Lang::En);
        assert_eq!(
            addresses(&team, &refs[..1]).unwrap(),
            "Exact addresses of your teammates whose name another session also carries: dev-rust [a1b2c3]."
        );
    }
}
