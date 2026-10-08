// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A team composed by Claude: `claude -p` reads the repository (read-only tools) and answers with JSON.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::{MAX_CONTACTS, Member, Team, validate_member_name};
use crate::i18n::Lang;
use crate::t;
use crate::templates::Templates;

const SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "description": { "type": "string" },
    "rules": { "type": "string" },
    "members": {
      "type": "array",
      "minItems": 1,
      "maxItems": 12,
      "items": {
        "type": "object",
        "properties": {
          "name": { "type": "string" },
          "role": { "type": "string" },
          "instructions": { "type": "string" },
          "contact": { "type": "boolean" }
        },
        "required": ["name", "role", "instructions"]
      }
    }
  },
  "required": ["description", "members"]
}"#;

#[derive(Deserialize)]
struct Output {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
    structured_output: Option<Composed>,
}

#[derive(Deserialize)]
struct Composed {
    description: String,
    #[serde(default)]
    rules: Option<String>,
    members: Vec<ComposedMember>,
}

#[derive(Deserialize)]
struct ComposedMember {
    name: String,
    role: String,
    instructions: String,
    #[serde(default)]
    contact: bool,
}

/// The model and effort that compose a team, whatever the user's defaults: reading a repository and sharing out
/// roles needs neither Opus nor a long reflection, and a strong default made it take minutes.
const MODEL: &str = "sonnet";
const EFFORT: &str = "medium";

/// `size`: "small", "medium", "large", or None to let Claude judge. The team is written in the language of
/// `templates`, whose roles serve as models and whose shared rules every member gets.
pub fn compose(
    claude: &Path,
    config_dir: Option<&Path>,
    dir: &Path,
    description: &str,
    size: Option<&str>,
    templates: &Templates,
) -> Result<Team> {
    let prompt = prompt(description, size, templates);
    let spinner = Spinner::start(t!(
        "Claude lit le projet et compose l'équipe (une minute environ)",
        "Claude is reading the project and composing the team (about a minute)"
    ));
    let output = crate::claude::command(claude, config_dir)
        .current_dir(dir)
        .args(["-p", &prompt, "--output-format", "json", "--json-schema", SCHEMA, "--no-session-persistence"])
        .args(["--tools", "Read,Glob,Grep", "--allowedTools", "Read,Glob,Grep"])
        .args(["--model", MODEL, "--effort", EFFORT])
        .stdin(Stdio::null())
        .output();
    spinner.stop();
    let output = output.context("claude -p")?;
    if !output.status.success() {
        bail!(t!(
            "Claude n'a pas pu composer l'équipe : {}",
            "Claude could not compose the team: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let parsed: Output = serde_json::from_slice(&output.stdout).context("claude -p --output-format json")?;
    let composed = match parsed.structured_output {
        Some(composed) if !parsed.is_error => composed,
        _ => bail!(t!(
            "Claude n'a pas rendu d'équipe : {}",
            "Claude returned no team: {}",
            parsed.result.unwrap_or_default()
        )),
    };
    Ok(into_team(composed, templates.lang(), templates.team_instructions()))
}

fn into_team(composed: Composed, lang: Lang, shared_rules: &str) -> Team {
    let mut instructions = shared_rules.trim().to_string();
    if let Some(rules) = composed.rules.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        instructions.push('\n');
        instructions.push_str(rules);
    }
    let mut team = Team {
        description: Some(composed.description.trim().to_string()),
        lang: Some(lang),
        instructions: Some(instructions),
        ..Default::default()
    };
    for member in composed.members {
        let mut name = member.name.trim().to_lowercase().replace(char::is_whitespace, "-");
        name.retain(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
        let name = name.trim_start_matches(['-', '.']).to_string();
        if validate_member_name(&name).is_err() || team.members.contains_key(&name) {
            continue;
        }
        // Past the limit, the others become working agents.
        let contact = member.contact && team.members.values().filter(|m| m.contact).count() < MAX_CONTACTS;
        team.members.insert(
            name,
            Member {
                role: member.role.trim().to_string(),
                instructions: Some(member.instructions.trim().to_string()).filter(|i| !i.is_empty()),
                contact,
                ..Default::default()
            },
        );
    }
    team
}

fn prompt(description: &str, size: Option<&str>, templates: &Templates) -> String {
    let shared_rules = templates.team_instructions().trim();
    let roles: Vec<String> = templates.roles().iter().map(|(name, role)| format!("- {name} — {role}")).collect();
    let roles = roles.join("\n");
    match templates.lang() {
        Lang::Fr => {
            let size = match size {
                Some("small") => "petite, 3 membres",
                Some("medium") => "moyenne, 5 membres",
                Some("large") => "grande, 8 membres",
                _ => "à toi de juger, entre 2 et 8 membres",
            };
            format!(
                "Tu composes une équipe d'agents Claude Code pour le projet de ce dossier. Chaque membre sera une session Claude Code \
interactive, ouverte en même temps que les autres dans ce même dossier ; ils se parlent par l'outil SendMessage, et l'utilisateur \
parle d'abord au premier membre, le coordinateur.

Description du projet par l'utilisateur :
<description>
{description}
</description>

Taille souhaitée : {size}.

Lis d'abord le dépôt (README, CLAUDE.md, manifestes, arborescence) pour connaître la stack, le découpage du code et la façon dont le \
projet se construit, se teste et se publie.

Compose l'équipe par métier, comme une équipe de développement : chaque membre a un métier et des compétences (un langage ou une \
techno, la revue, les tests, l'ops avec le build, la CI, le packaging et la publication, la sécurité, la documentation…), et non une \
zone du code. Les rôles intégrés de recruit en sont le modèle :
<roles_integres>
{roles}
</roles_integres>
Après le coordinateur, prévois les développeurs dont la stack a besoin, puis les métiers transverses utiles au projet (revue, tests, \
ops, sécurité, documentation), dans la limite de la taille souhaitée.

Rends l'équipe :
- members : le premier s'appelle « coordinateur » ; c'est le point d'entrée de l'utilisateur ; il découpe et répartit le travail, suit \
l'avancement, relit ce qui touche plusieurs zones, demande à l'utilisateur les choix produit, et ne commite que sur sa demande. Les \
autres se partagent le travail sans se chevaucher.
- name : le métier, court, en minuscules, sans espace (tirets permis), unique. Reprends le nom d'un rôle intégré quand il convient ; \
sinon métier-spécialité, comme « dev-rust », « dev-ts » ou « dev-api ». Un métier, pas une partie du code : jamais deux zones collées \
comme « config-cli » ou « tableau-mod ». Un mot français écrit normalement, accents compris (« sécurité », et non « securite »).
- role : une ligne, le métier et ce dont le membre est responsable.
- instructions : 5 à 10 puces concrètes à la deuxième personne (« tu ») : responsabilités, zone avec les vrais chemins du dépôt dont \
le membre a la charge (pour un métier transverse comme la revue, ce qu'il lit et ce qu'il peut modifier), avec qui se coordonner, \
limites, comment rendre un travail (avec les vraies commandes de vérification du projet).
- contact : vrai pour les interlocuteurs de l'utilisateur, ceux à qui il parle (le coordinateur, rarement un second, deux au plus) ; faux pour les \
agents de travail, qui reçoivent leur travail des interlocuteurs et leur rendent compte.
- description : une ligne sur le projet et l'équipe.
- rules : facultatif, des puces de règles propres à ce projet (commandes de vérification, conventions) à ajouter aux règles communes \
ci-dessous, déjà données à tous ; ne les répète pas.

<regles_communes>
{shared_rules}
</regles_communes>

Écris tout en français. Ne modifie aucun fichier."
            )
        }
        Lang::En => {
            let size = match size {
                Some("small") => "small, 3 members",
                Some("medium") => "medium, 5 members",
                Some("large") => "large, 8 members",
                _ => "your call, between 2 and 8 members",
            };
            format!(
                "You are composing a team of Claude Code agents for the project in this directory. Each member will be an interactive \
Claude Code session, running at the same time as the others in this same directory; they talk to each other with the SendMessage \
tool, and the user talks first to the first member, the coordinator.

The user's description of the project:
<description>
{description}
</description>

Desired size: {size}.

First read the repository (README, CLAUDE.md, manifests, file tree) to learn the stack, how the code is split, and how the project \
is built, tested and released.

Compose the team by discipline, like a development team: each member has a discipline and skills (a language or technology, review, \
testing, ops with build, CI, packaging and releases, security, documentation…), not an area of the code. recruit's built-in roles \
are the model:
<built_in_roles>
{roles}
</built_in_roles>
After the coordinator, plan the developers the stack needs, then the cross-cutting disciplines the project calls for (review, \
testing, ops, security, documentation), within the desired size.

Return the team:
- members: the first one is named \"coordinator\"; it is the user's point of contact; it breaks down and assigns the work, tracks \
progress, reviews what spans several areas, asks the user for product decisions, and commits only when the user asks. The others \
share the work without overlapping.
- name: the discipline, short, lowercase, no spaces (dashes allowed), unique. Use a built-in role's name when it fits; otherwise \
discipline-specialty, like \"dev-rust\", \"dev-ts\" or \"dev-api\". A discipline, not a part of the code: never two areas glued \
together like \"config-cli\" or \"dashboard-mod\".
- role: one line, the discipline and what the member is responsible for.
- instructions: 5 to 10 concrete bullet points in the second person (\"you\"): responsibilities, area with the repository's real \
paths the member owns (for a cross-cutting discipline like review, what it reads and what it may change), who to coordinate with, \
limits, how to hand work in (with the project's real verification commands).
- contact: true for the user's contacts, the members the user talks to (the coordinator, rarely a second one, two at most); false for the \
working agents, who get their work from the contacts and report back to them.
- description: one line about the project and the team.
- rules: optional, bullet points of rules specific to this project (verification commands, conventions) to add to the shared rules \
below, which everyone already gets; do not repeat them.

<shared_rules>
{shared_rules}
</shared_rules>

Write everything in English. Do not modify any file."
            )
        }
    }
}

const MEMBER_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "name": { "type": "string" },
    "role": { "type": "string" },
    "instructions": { "type": "string" }
  },
  "required": ["name", "role", "instructions"]
}"#;

#[derive(Deserialize)]
struct MemberOutput {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
    structured_output: Option<ComposedMember>,
}

/// One more member for a running team, as the user asks for it: composed by discipline like a team's members, next
/// to the team as it is (`team_name`, `team`), in the language of `templates`. A working agent: the user makes it a
/// contact if need be. Its name may still be taken: the menu has it checked. `claude` is Claude Code's command, in
/// the team's profile and folder.
pub fn compose_member(
    mut claude: Command,
    team_name: &str,
    team: &Team,
    request: &str,
    templates: &Templates,
    started: &mut dyn FnMut(u32),
) -> Result<(String, Member)> {
    let prompt = member_prompt(team_name, team, request, templates);
    // The menu shows its own sign while Claude composes, and can stop it: the child's process is given out.
    let child = claude
        .args(["-p", &prompt, "--output-format", "json", "--json-schema", MEMBER_SCHEMA, "--no-session-persistence"])
        .args(["--tools", "Read,Glob,Grep", "--allowedTools", "Read,Glob,Grep"])
        .args(["--model", MODEL, "--effort", EFFORT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("claude -p")?;
    started(child.id());
    let output = child.wait_with_output().context("claude -p")?;
    if !output.status.success() {
        bail!(t!(
            "Claude n'a pas pu composer le membre : {}",
            "Claude could not compose the member: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let parsed: MemberOutput = serde_json::from_slice(&output.stdout).context("claude -p --output-format json")?;
    let composed = match parsed.structured_output {
        Some(composed) if !parsed.is_error => composed,
        _ => bail!(t!(
            "Claude n'a pas rendu de membre : {}",
            "Claude returned no member: {}",
            parsed.result.unwrap_or_default()
        )),
    };
    let returned = composed.name.clone();
    into_member(composed, templates.lang()).with_context(|| {
        t!("Claude a rendu un nom inutilisable : « {} »", "Claude returned an unusable name: \"{}\"", returned)
    })
}

/// A composed member, its name cleaned as a team's are; none when nothing usable is left of the name.
fn into_member(composed: ComposedMember, lang: Lang) -> Option<(String, Member)> {
    let alone = Composed { description: String::new(), rules: None, members: vec![composed] };
    let (name, member) = into_team(alone, lang, "").members.into_iter().next()?;
    Some((name, Member { contact: false, ..member }))
}

fn member_prompt(team_name: &str, team: &Team, request: &str, templates: &Templates) -> String {
    let lang = templates.lang();
    let contacts = team.contacts();
    let members: Vec<String> = team
        .members
        .iter()
        .map(|(name, m)| {
            let role = m.role.trim().lines().next().unwrap_or_default();
            let mark = match (contacts.contains(&name.as_str()), lang) {
                (false, _) => "",
                (true, Lang::Fr) => " (interlocuteur)",
                (true, Lang::En) => " (contact)",
            };
            format!("- {name}{mark} — {role}")
        })
        .collect();
    let members = members.join("\n");
    let roles: Vec<String> = templates.roles().iter().map(|(name, role)| format!("- {name} — {role}")).collect();
    let roles = roles.join("\n");
    let description = team.description.as_deref().map(str::trim).filter(|d| !d.is_empty());
    let rules = team.instructions.as_deref().map(str::trim).filter(|r| !r.is_empty());
    match lang {
        Lang::Fr => {
            let description = description.map(|d| format!(" ({d})")).unwrap_or_default();
            let rules = rules.map_or(String::new(), |rules| {
                format!(
                    "\n\nIl recevra les règles communes de l'équipe ; ne les répète pas :\n<regles_communes>\n{rules}\n</regles_communes>"
                )
            });
            format!(
                "Tu ajoutes un membre à une équipe d'agents Claude Code qui travaille déjà sur le projet de ce dossier. Chaque membre est \
une session Claude Code interactive, ouverte en même temps que les autres dans ce même dossier ; ils se parlent par l'outil \
SendMessage, et l'utilisateur parle à leurs interlocuteurs.

L'équipe « {team_name} »{description} :
<equipe>
{members}
</equipe>

Ce que l'utilisateur attend du nouveau membre :
<demande>
{request}
</demande>

Lis d'abord du dépôt ce qu'il faut (README, CLAUDE.md, manifestes, arborescence) pour connaître la stack et les chemins dont le \
nouveau membre aura la charge.

Le nouveau membre a un métier et des compétences (un langage ou une techno, la revue, les tests, l'ops avec le build, la CI, le \
packaging et la publication, la sécurité, la documentation…), et non une zone du code ; il ne chevauche pas les membres actuels. \
Les rôles intégrés de recruit en sont le modèle :
<roles_integres>
{roles}
</roles_integres>

Rends le membre :
- name : le métier, court, en minuscules, sans espace (tirets permis), différent des noms de l'équipe. Reprends le nom d'un rôle \
intégré quand il convient ; sinon métier-spécialité, comme « dev-rust », « dev-ts » ou « dev-api ». Un métier, pas une partie du \
code : jamais deux zones collées comme « config-cli » ou « tableau-mod ». Un mot français écrit normalement, accents compris \
(« sécurité », et non « securite »).
- role : une ligne, le métier et ce dont le membre est responsable.
- instructions : 5 à 10 puces concrètes à la deuxième personne (« tu ») : responsabilités, zone avec les vrais chemins du dépôt \
dont il a la charge (pour un métier transverse comme la revue, ce qu'il lit et ce qu'il peut modifier), avec qui de l'équipe se \
coordonner, limites, comment rendre un travail (avec les vraies commandes de vérification du projet).{rules}

Écris tout en français. Ne modifie aucun fichier."
            )
        }
        Lang::En => {
            let description = description.map(|d| format!(" ({d})")).unwrap_or_default();
            let rules = rules.map_or(String::new(), |rules| {
                format!(
                    "\n\nIt will get the team's shared rules; do not repeat them:\n<shared_rules>\n{rules}\n</shared_rules>"
                )
            });
            format!(
                "You are adding a member to a team of Claude Code agents already working on the project in this directory. Each \
member is an interactive Claude Code session, running at the same time as the others in this same directory; they talk to each \
other with the SendMessage tool, and the user talks to their contacts.

The \"{team_name}\" team{description}:
<team>
{members}
</team>

What the user expects of the new member:
<request>
{request}
</request>

First read what you need of the repository (README, CLAUDE.md, manifests, file tree) to learn the stack and the paths the new \
member will own.

The new member has a discipline and skills (a language or technology, review, testing, ops with build, CI, packaging and \
releases, security, documentation…), not an area of the code; it does not overlap the current members. recruit's built-in roles \
are the model:
<built_in_roles>
{roles}
</built_in_roles>

Return the member:
- name: the discipline, short, lowercase, no spaces (dashes allowed), unlike the team's names. Use a built-in role's name when it \
fits; otherwise discipline-specialty, like \"dev-rust\", \"dev-ts\" or \"dev-api\". A discipline, not a part of the code: never two \
areas glued together like \"config-cli\" or \"dashboard-mod\".
- role: one line, the discipline and what the member is responsible for.
- instructions: 5 to 10 concrete bullet points in the second person (\"you\"): responsibilities, area with the repository's real \
paths it owns (for a cross-cutting discipline like review, what it reads and what it may change), who in the team to coordinate \
with, limits, how to hand work in (with the project's real verification commands).{rules}

Write everything in English. Do not modify any file."
            )
        }
    }
}

/// An elapsed-time line on stderr while waiting, when stderr is a terminal.
struct Spinner {
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    fn start(message: String) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        if !std::io::stderr().is_terminal() {
            eprintln!("{message}…");
            return Spinner { done, thread: None };
        }
        let flag = done.clone();
        let thread = std::thread::spawn(move || {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let start = Instant::now();
            let mut i = 0;
            while !flag.load(Ordering::Relaxed) {
                eprint!("\r{} {message} · {} s ", frames[i % frames.len()], start.elapsed().as_secs());
                let _ = std::io::stderr().flush();
                i += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            eprint!("\r\x1b[2K");
        });
        Spinner { done, thread: Some(thread) }
    }

    fn stop(mut self) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composed_team_is_cleaned() {
        let composed: Composed = serde_json::from_str(
            r#"{"description":"API","rules":"- cargo test","members":[
                {"name":"Coordinateur","role":"Pilote","instructions":"- tout","contact":true},
                {"name":"dev back","role":"API","instructions":"- src/api"},
                {"name":"coordinateur","role":"doublon","instructions":""},
                {"name":"archi","role":"Plans","instructions":"","contact":true},
                {"name":"qa","role":"Tests","instructions":"","contact":true},
                {"name":"Qualité","role":"Qualité","instructions":""},
                {"name":"RÉDACTEUR Technique","role":"Docs","instructions":""}
            ]}"#,
        )
        .unwrap();
        let team = into_team(composed, Lang::Fr, "- commun");
        let names: Vec<&str> = team.members.keys().map(String::as_str).collect();
        assert_eq!(names, ["coordinateur", "dev-back", "archi", "qa", "qualité", "rédacteur-technique"]);
        assert_eq!(team.instructions.as_deref(), Some("- commun\n- cargo test"));
        assert_eq!(team.contacts(), ["coordinateur", "archi"]);
    }

    #[test]
    fn composed_member_is_cleaned() {
        let composed = |name: &str| ComposedMember {
            name: name.into(),
            role: " Tests ".into(),
            instructions: " ".into(),
            contact: true,
        };
        let (name, member) = into_member(composed("Testeur Rust"), Lang::Fr).unwrap();
        assert_eq!(name, "testeur-rust");
        assert_eq!((member.role.as_str(), member.instructions, member.contact), ("Tests", None, false));
        assert_eq!(into_member(composed("Sécurité"), Lang::Fr).unwrap().0, "sécurité");
        assert!(into_member(composed("--"), Lang::Fr).is_none());
    }

    #[test]
    fn member_prompt_tells_the_team_and_its_rules() {
        let mut team = Team {
            description: Some("un CLI".into()),
            instructions: Some("- cargo test".into()),
            ..Default::default()
        };
        for (name, role) in [("coordinateur", "Répartit\nen détail"), ("dev-rust", "Code")] {
            team.members.insert(name.into(), Member { role: role.into(), ..Default::default() });
        }
        let fr = member_prompt("recruit", &team, "quelqu'un pour la doc", &Templates::for_lang(Lang::Fr));
        assert!(fr.contains("L'équipe « recruit » (un CLI) :\n<equipe>\n- coordinateur (interlocuteur) — Répartit\n- dev-rust — Code\n</equipe>"));
        assert!(fr.contains("<demande>\nquelqu'un pour la doc\n</demande>"));
        assert!(fr.contains("« tableau-mod »") && fr.contains("accents compris"));
        assert!(fr.contains("<regles_communes>\n- cargo test\n</regles_communes>"));
        team.instructions = None;
        let en = member_prompt("recruit", &team, "docs", &Templates::for_lang(Lang::En));
        assert!(en.contains("- coordinateur (contact) — Répartit\n") && en.contains("\"dashboard-mod\""));
        assert!(!en.contains("shared_rules") && !en.contains("sécurité"));
    }

    #[test]
    fn prompt_models_members_on_built_in_roles() {
        let cases = [
            (Lang::Fr, ["« dev-rust »", "« dev-ts »", "« dev-api »", "« config-cli »", "« tableau-mod »"], "sécurité"),
            (
                Lang::En,
                ["\"dev-rust\"", "\"dev-ts\"", "\"dev-api\"", "\"config-cli\"", "\"dashboard-mod\""],
                "security",
            ),
        ];
        for (lang, examples, security) in cases {
            let templates = Templates::for_lang(lang);
            let prompt = prompt("Un CLI en Rust", Some("medium"), &templates);
            let roles = templates.roles();
            assert!(roles.len() > 10 && roles.iter().any(|(name, _)| *name == security), "{lang:?}");
            for (name, role) in roles {
                assert!(prompt.contains(&format!("\n- {name} — {role}\n")), "{lang:?}: role {name}");
            }
            for example in examples {
                assert!(prompt.contains(example), "{lang:?}: {example}");
            }
            assert!(prompt.contains(templates.team_instructions().trim()), "{lang:?}: shared rules");
        }
        let fr = prompt("x", None, &Templates::for_lang(Lang::Fr));
        assert!(fr.contains("accents compris (« sécurité », et non « securite »)"));
        assert!(!prompt("x", None, &Templates::for_lang(Lang::En)).contains("sécurité"));
    }
}
