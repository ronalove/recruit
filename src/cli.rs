// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Command line. Help texts are written in English here and translated in `localize` for French.

use std::ffi::OsString;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::i18n::{self, Lang};
use crate::t;
use crate::{board, bridge, launch};

/// Launch a team of Claude Code agents in tmux, or create one.
#[derive(Parser, Debug)]
#[command(name = "recruit", version)]
pub struct Cli {
    /// Team to launch: the local one if both exist, else the global one; created if it does not exist
    pub team: Option<String>,

    #[command(flatten)]
    pub launch: LaunchArgs,

    /// Interface language (default: from the locale)
    #[arg(long, global = true, value_enum)]
    pub lang: Option<Lang>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args, Debug, Default, Clone, PartialEq, Eq)]
pub struct LaunchArgs {
    /// Each member resumes the last conversation that carries its name
    #[arg(short, long)]
    pub resume: bool,
    /// Open the layout only: each pane shows its member, Claude is not started
    #[arg(long)]
    pub dry_run: bool,
    /// Launch without attaching to the team
    #[arg(short, long)]
    pub detach: bool,
    /// Stop the team first if it is already running
    #[arg(long)]
    pub restart: bool,
    /// Print the tabs and the commands, launch nothing
    #[arg(long)]
    pub print: bool,
}

impl From<LaunchArgs> for launch::Options {
    fn from(a: LaunchArgs) -> Self {
        launch::Options { resume: a.resume, dry_run: a.dry_run, detach: a.detach, restart: a.restart, print: a.print }
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a team: interactive without options, scripted with them
    New(NewArgs),
    /// List local and global teams, and those running
    List {
        /// JSON output
        #[arg(long)]
        json: bool,
    },
    /// Join a running team
    Attach {
        /// Team name (default: this project's team, or the only one running)
        team: Option<String>,
    },
    /// Stop a running team: its Claude sessions are closed
    Stop {
        /// Team name (default: this project's team, or the only one running)
        team: Option<String>,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Open a team's file in $EDITOR
    Edit {
        /// Team name (default: this project's settings)
        team: Option<String>,
        /// Edit the personal file .recruit/settings.local.toml instead
        #[arg(long)]
        local: bool,
    },
    /// List built-in project types and sizes
    Templates,
    /// Internal: a running team's panels, launched by recruit in its tmux panes
    #[command(name = "_panel", hide = true)]
    Panel {
        kind: board::Kind,
        /// The team's folder, under ~/.cache/recruit/teams/
        state: std::path::PathBuf,
    },
    /// Internal: a click on a panel, bound by recruit in its tmux server; goes to the member clicked
    #[command(name = "_click", hide = true)]
    Click {
        /// The team's folder, under ~/.cache/recruit/teams/
        state: std::path::PathBuf,
        /// The panel's tmux pane (`%12`)
        pane: String,
    },
    /// Internal: what runs in a member's pane, its Claude started again when it stops on its own
    #[command(name = "_member", hide = true)]
    Member {
        /// The team's folder, under ~/.cache/recruit/teams/
        state: std::path::PathBuf,
        member: String,
        /// Start on the member's last conversation
        #[arg(long)]
        resume: bool,
    },
    /// Internal: a running team's menu, in a tmux popup opened by /recruit, Alt+r or the status line's button
    #[command(name = "_menu", hide = true)]
    Menu {
        /// The team's folder, under ~/.cache/recruit/teams/
        state: std::path::PathBuf,
        /// The tmux client the menu shows on, to detach it
        #[arg(long)]
        client: Option<String>,
        /// Open the menu in a popup on --client, and return at once
        #[arg(long, requires = "client")]
        popup: bool,
        /// The client's terminal carries the Nerd Font symbols
        #[arg(long)]
        nerd: bool,
    },
    /// Internal: changes a running team as its menu does, without the menu: for scripts and tests
    #[command(name = "_edit", hide = true)]
    EditRunning {
        /// The team's folder, under ~/.cache/recruit/teams/
        state: std::path::PathBuf,
        /// Changes, as a JSON list: [{"set": {"member": "dev", "field": {"model": "opus"}}}, {"rename": {"from": "a",
        /// "to": "b"}}, {"add": {"name": "x", "member": {"role": "…"}}}, {"remove": "x"}, {"dashboard": false}]
        edits: Option<String>,
        /// Then start this member again, on its conversation
        #[arg(long, value_name = "MEMBER")]
        restart: Option<String>,
        /// With --restart: on a new conversation
        #[arg(long, requires = "restart")]
        fresh: bool,
        /// Then start every member again on a new conversation
        #[arg(long, conflicts_with = "restart")]
        restart_all: bool,
        /// Close the pane of a running member that the team's files no longer have
        #[arg(long, value_name = "MEMBER")]
        dismiss: Option<String>,
    },
    /// Internal: a team's server, recruit's own multiplexer; detached, returns once it is ready
    #[command(name = "_server", hide = true)]
    Server {
        /// The team's folder, under ~/.cache/recruit/teams/
        state: std::path::PathBuf,
    },
    /// Internal: a team's server observed and driven without a terminal, for tests
    #[command(name = "_ctl", hide = true)]
    Ctl {
        /// The team's folder, under ~/.cache/recruit/teams/
        state: std::path::PathBuf,
        #[command(subcommand)]
        action: crate::mux::ctl::Action,
    },
    /// Internal: the mod recruit puts in each member calls it, JSON in and out
    #[command(name = "_mod", hide = true)]
    Mod {
        event: bridge::Event,
        /// The team's folder, under ~/.cache/recruit/teams/
        state: std::path::PathBuf,
        member: String,
        /// The slash command run, for `command`
        command: Option<String>,
    },
}

#[derive(Args, Debug, Default)]
pub struct NewArgs {
    /// Team name
    pub name: Option<String>,
    /// Save in ~/.config/recruit/<name>.toml instead of the project's .recruit/settings.toml
    #[arg(short, long)]
    pub global: bool,
    /// Built-in project type (see `recruit templates`)
    #[arg(short, long)]
    pub template: Option<String>,
    /// Team size for --template or --describe: small, medium, large
    #[arg(short, long)]
    pub size: Option<String>,
    /// Let Claude read the project and compose the team from this description
    #[arg(long, conflicts_with = "template")]
    pub describe: Option<String>,
    /// Add a member, as "name:role" (repeatable)
    #[arg(short, long = "member", value_name = "NAME:ROLE")]
    pub members: Vec<String>,
    /// Mark a member as one of your contacts, the members you talk to (repeatable; default: the template's, or
    /// the first member)
    #[arg(short, long = "contact", value_name = "NAME")]
    pub contacts: Vec<String>,
    /// Claude Code permission mode for every member (acceptEdits, auto, bypassPermissions…)
    #[arg(long)]
    pub permission_mode: Option<String>,
    /// Claude model for every member (opus, sonnet…)
    #[arg(long)]
    pub model: Option<String>,
    /// Replace a team with the same name
    #[arg(short, long)]
    pub force: bool,
    /// Launch the team once created
    #[arg(short, long)]
    pub launch: bool,
}

/// `--lang` is needed before parsing, to write the help in the right language.
pub fn lang_from_args(args: &[OsString]) -> Option<Lang> {
    let mut iter = args.iter().filter_map(|a| a.to_str());
    while let Some(arg) = iter.next() {
        let value = match arg.strip_prefix("--lang") {
            Some("") => iter.next(),
            Some(rest) => rest.strip_prefix('='),
            None => continue,
        };
        return match value {
            Some("fr") => Some(Lang::Fr),
            Some("en") => Some(Lang::En),
            _ => None,
        };
    }
    None
}

pub fn parse(args: Vec<OsString>) -> Cli {
    let matches = localize(Cli::command()).get_matches_from(args);
    Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit())
}

fn localize(command: clap::Command) -> clap::Command {
    if i18n::lang() != Lang::Fr {
        return command;
    }
    in_french(command)
}

fn in_french(command: clap::Command) -> clap::Command {
    // What clap adds on its own (--help, --version, the help command) only exists once the command is built.
    let mut command = french(command);
    command.build();
    french_builtins(command)
}

fn french(command: clap::Command) -> clap::Command {
    let launch_help = |c: clap::Command| {
        c.mut_arg("resume", |a| a.help("Chaque membre reprend la dernière conversation qui porte son nom"))
            .mut_arg("dry_run", |a| {
                a.help("Ouvrir la disposition seule : chaque panneau affiche son membre, sans lancer Claude")
            })
            .mut_arg("detach", |a| a.help("Lancer sans s'attacher à l'équipe"))
            .mut_arg("restart", |a| a.help("Arrêter d'abord l'équipe si elle tourne déjà"))
            .mut_arg("print", |a| a.help("Afficher les onglets et les commandes, sans rien lancer"))
    };
    let team_help = t!(
        "Nom de l'équipe (par défaut : celle du projet, ou la seule qui tourne)",
        "Team name (default: this project's team, or the only one running)"
    );
    launch_help(command)
        .about("Lance une équipe d'agents Claude Code dans tmux, ou en crée une.")
        .mut_arg("team", |a| {
            a.value_name("ÉQUIPE").help("Équipe à lancer : la locale si les deux existent, sinon la globale ; créée si elle n'existe pas")
        })
        .mut_arg("lang", |a| a.help("Langue de l'interface (par défaut : selon la locale)"))
        .mut_subcommand("new", |c| {
            c.about("Créer une équipe : interactive sans option, scriptée avec des options")
                .mut_arg("name", |a| a.help("Nom de l'équipe").value_name("ÉQUIPE"))
                .mut_arg("global", |a| {
                    a.help("Enregistrer dans ~/.config/recruit/<nom>.toml plutôt que dans .recruit/settings.toml du projet")
                })
                .mut_arg("template", |a| a.help("Type de projet intégré (voir `recruit templates`)"))
                .mut_arg("size", |a| a.help("Taille de l'équipe pour --template ou --describe : small, medium, large"))
                .mut_arg("describe", |a| a.help("Claude lit le projet et compose l'équipe d'après cette description"))
                .mut_arg("members", |a| {
                    a.help("Ajouter un membre, sous la forme « nom:rôle » (répétable)").value_name("NOM:RÔLE")
                })
                .mut_arg("contacts", |a| {
                    a.help("Désigner un interlocuteur, un membre à qui tu parles (répétable ; par défaut : ceux du modèle, ou le premier membre)")
                        .value_name("NOM")
                })
                .mut_arg("permission_mode", |a| {
                    a.help("Mode de permission de Claude Code pour tous les membres (acceptEdits, auto, bypassPermissions…)")
                })
                .mut_arg("model", |a| a.help("Modèle de Claude pour tous les membres (opus, sonnet…)"))
                .mut_arg("force", |a| a.help("Remplacer une équipe du même nom"))
                .mut_arg("launch", |a| a.help("Lancer l'équipe une fois créée"))
        })
        .mut_subcommand("list", |c| {
            c.about("Lister les équipes locales et globales, et celles qui tournent").mut_arg("json", |a| a.help("Sortie JSON"))
        })
        .mut_subcommand("attach", |c| c.about("Rejoindre une équipe qui tourne").mut_arg("team", |a| a.help(team_help.clone()).value_name("ÉQUIPE")))
        .mut_subcommand("stop", |c| {
            c.about("Arrêter une équipe : ses sessions Claude sont fermées")
                .mut_arg("team", |a| a.help(team_help.clone()).value_name("ÉQUIPE"))
                .mut_arg("yes", |a| a.help("Ne pas demander de confirmation"))
        })
        .mut_subcommand("edit", |c| {
            c.about("Ouvrir le fichier d'une équipe dans $EDITOR")
                .mut_arg("team", |a| a.help("Nom de l'équipe (par défaut : les réglages du projet)").value_name("ÉQUIPE"))
                .mut_arg("local", |a| a.help("Ouvrir plutôt le fichier personnel .recruit/settings.local.toml"))
        })
        .mut_subcommand("templates", |c| c.about("Lister les types de projet et les tailles intégrés"))
        .mut_subcommand("_edit", |c| {
            c.about("Interne : modifie une équipe lancée comme son menu, sans le menu : pour les scripts et les tests")
                .mut_arg("state", |a| a.help("Le dossier de l'équipe, sous ~/.cache/recruit/teams/"))
                .mut_arg("edits", |a| a.help("Les modifications, en liste JSON (voir l'aide en anglais)"))
                .mut_arg("restart", |a| a.help("Puis relancer ce membre, sur sa conversation").value_name("MEMBRE"))
                .mut_arg("fresh", |a| a.help("Avec --restart : sur une nouvelle conversation"))
                .mut_arg("restart_all", |a| a.help("Puis relancer tous les membres sur une nouvelle conversation"))
                .mut_arg("dismiss", |a| {
                    a.help("Fermer le panneau d'un membre lancé que les fichiers de l'équipe n'ont plus").value_name("MEMBRE")
                })
        })
        .mut_subcommand("_menu", |c| {
            c.about(format!(
                "Interne : le menu d'une équipe lancée, dans une fenêtre tmux ouverte par /recruit, {}r ou le bouton de la barre",
                crate::tmux::ALT
            ))
                .mut_arg("state", |a| a.help("Le dossier de l'équipe, sous ~/.cache/recruit/teams/"))
                .mut_arg("client", |a| a.help("Le client tmux où s'affiche le menu, pour le détacher"))
                .mut_arg("popup", |a| a.help("Ouvrir le menu dans une fenêtre sur --client, et rendre la main aussitôt"))
                .mut_arg("nerd", |a| a.help("Le terminal du client embarque les symboles Nerd Font"))
        })
}

/// What clap writes itself, in French: the help and version flags, the help command, the possible values and the
/// headings. On a built command, in each of its subcommands.
fn french_builtins(command: clap::Command) -> clap::Command {
    let template = help_template(&command);
    let command = match command.get_name() {
        "help" => command.about("Afficher cette aide, ou celle des commandes données"),
        _ => command,
    };
    command
        .help_template(template)
        .subcommand_value_name("COMMANDE")
        .mut_args(|arg| match arg.get_id().as_str() {
            "help" if arg.get_long_help().is_some() => {
                arg.help("Afficher l'aide (le détail avec --help)").long_help("Afficher l'aide (le résumé avec -h)")
            }
            "help" => arg.help("Afficher l'aide"),
            "version" => arg.help("Afficher la version"),
            _ => possible_values(arg),
        })
        .mut_subcommands(french_builtins)
}

/// clap's help with its headings in French, a space before their colon: it writes "Usage:" itself.
fn help_template(command: &clap::Command) -> String {
    let styles = command.get_styles();
    let (usage, header) = (styles.get_usage(), styles.get_header());
    let section = |title: &str, tag: &str| format!("{header}{title} :{header:#}\n{{{tag}}}");
    let mut sections = Vec::new();
    if command.get_subcommands().any(|c| !c.is_hide_set()) {
        sections.push(section("Commandes", "subcommands"));
    }
    if command.get_positionals().any(|a| !a.is_hide_set()) {
        sections.push(section("Arguments", "positionals"));
    }
    if command.get_arguments().any(|a| !a.is_positional() && !a.is_hide_set()) {
        sections.push(section("Options", "options"));
    }
    let mut template = format!("{{before-help}}{{about-with-newline}}\n{usage}Utilisation :{usage:#} {{usage}}");
    if !sections.is_empty() {
        template.push_str("\n\n");
        template.push_str(&sections.join("\n\n"));
    }
    template + "{after-help}"
}

/// "[possible values: fr, en]", which clap writes after the help, in French.
fn possible_values(arg: clap::Arg) -> clap::Arg {
    let values: Vec<String> =
        arg.get_possible_values().iter().filter(|v| !v.is_hide_set()).map(|v| v.get_name().to_string()).collect();
    if values.is_empty() || arg.is_hide_possible_values_set() {
        return arg;
    }
    let help = arg.get_help().map(ToString::to_string).unwrap_or_default();
    arg.hide_possible_values(true).help(format!("{help} [valeurs possibles : {}]", values.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn lang_flag_before_parsing() {
        assert_eq!(lang_from_args(&args(&["recruit", "--lang", "fr"])), Some(Lang::Fr));
        assert_eq!(lang_from_args(&args(&["recruit", "list", "--lang=en"])), Some(Lang::En));
        assert_eq!(lang_from_args(&args(&["recruit", "x"])), None);
    }

    #[test]
    fn command_line_is_consistent() {
        Cli::command().debug_assert();
        localize(Cli::command()).debug_assert();
        let cli = Cli::try_parse_from(["recruit", "perso", "--resume"]).unwrap();
        assert_eq!(cli.team.as_deref(), Some("perso"));
        assert!(cli.launch.resume);
        let cli =
            Cli::try_parse_from(["recruit", "new", "x", "-t", "web", "-m", "a:b", "-m", "c:d", "-c", "c"]).unwrap();
        let Some(Command::New(new)) = cli.command else { panic!() };
        assert_eq!(new.members, ["a:b", "c:d"]);
        assert_eq!(new.contacts, ["c"]);
        let cli = Cli::try_parse_from(["recruit", "--lang", "fr", "_menu", "/s", "--client", "/dev/ttys004"]).unwrap();
        let Some(Command::Menu { state, client, popup, nerd }) = cli.command else { panic!() };
        assert_eq!((state.to_str(), client.as_deref(), popup, nerd), (Some("/s"), Some("/dev/ttys004"), false, false));
        assert!(Cli::try_parse_from(["recruit", "_menu", "/s", "--popup"]).is_err());
    }

    #[test]
    fn french_help_leaves_no_english() {
        in_french(Cli::command()).debug_assert();
        let mut command = in_french(Cli::command());
        let mut help = |path: &[&str]| {
            let mut cmd = &mut command;
            for name in path {
                cmd = cmd.find_subcommand_mut(name).unwrap();
            }
            cmd.render_help().to_string()
        };
        let root = help(&[]);
        assert!(root.contains("Utilisation : recruit [OPTIONS] [ÉQUIPE] [COMMANDE]\n\nCommandes :\n"), "{root}");
        assert!(root.contains("\n\nArguments :\n") && root.contains("\n\nOptions :\n"), "{root}");
        assert!(root.contains("Afficher l'aide") && root.contains("Afficher la version"), "{root}");
        assert!(root.contains("help       Afficher cette aide"), "{root}");
        assert!(root.contains("[valeurs possibles : fr, en]"), "{root}");
        for page in [root, help(&["new"]), help(&["list"]), help(&["help"]), help(&["_panel"])] {
            for english in ["Usage", "Commands", "Options:", "Print", "possible values", "[COMMAND]"] {
                assert!(!page.contains(english), "{english}: {page}");
            }
        }
        let list = help(&["list"]);
        assert!(!list.contains("Commandes") && !list.contains("Arguments"), "only the sections it has: {list}");
    }
}
