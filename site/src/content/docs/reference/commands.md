---
title: Commands
description: Every recruit command and option.
sidebar:
  order: 1
---

Every command takes `--lang fr` or `--lang en`, for the language of its messages (see [Language](/recruit/faq/#which-language-does-recruit-speak)), and `-h`, `--help`.

## `recruit`

```
recruit [OPTIONS] [TEAM]
```

Launches a team, or creates one.

| You type | recruit… |
|---|---|
| `recruit` in a project with a team | launches it: the only one, or the one `default` names; otherwise it asks which |
| `recruit` elsewhere, with global teams | offers them, and never launches one without asking; it can also create a new team |
| `recruit` with no team at all | creates one interactively |
| `recruit <team>` | launches that team: the local one when both a local and a global team have that name, otherwise the global one |
| `recruit <team>`, unknown | creates it interactively, with that name |

When the team is already running, `recruit` joins it. It also starts again the members that no longer run, reopens a closed dashboard and, when panes were closed, offers to build the team again.

| Option | Effect |
|---|---|
| `-r`, `--resume` | Each member resumes the last conversation that carries its name in this folder. Ignored when the team is running. |
| `--dry-run` | Opens the layout only, as a separate trial (`<team>-dry-run`): each pane shows its member and role and the command it would run, and Claude is not started. `recruit stop <team>` closes the trial. |
| `-d`, `--detach` | Launches without attaching to the team. |
| `--restart` | Stops the team first if it is running. |
| `--print` | Prints the tabs, each member's command line and its prompt file, and launches nothing. |
| `--lang <LANG>` | Interface language: `fr` or `en`. |
| `-V`, `--version` | Prints recruit's version. |

A team name or a launch option does not go with a subcommand: `recruit web --resume`, not `recruit attach web --resume`.

## `recruit new`

```
recruit new [OPTIONS] [NAME]
```

Creates a team. Without `--template`, `--describe` or `--member`, it asks its questions, the name filled in when given (see [Composing a team](/recruit/guides/teams/#composing-a-team)). With one of them, it asks nothing and needs the name.

| Option | Effect |
|---|---|
| `-g`, `--global` | Saves the team in `~/.config/recruit/<name>.toml` instead of the project's `.recruit/settings.toml`. |
| `-t`, `--template <TEMPLATE>` | A [built-in team](/recruit/reference/templates/): `personal`, `web`, `mobile`, `api`, `library`, `data`, `game` or `infra`. |
| `-s`, `--size <SIZE>` | Team size for `--template` or `--describe`: `small` (3 members), `medium` (5) or `large` (8). `small` by default with `--template`; Claude decides with `--describe`. |
| `--describe <DESCRIPTION>` | Claude reads the project and composes the team from this description. |
| `-m`, `--member <NAME:ROLE>` | Adds a member, as `"name:role"`. Repeatable. With `--template` or `--describe`, adds a member to the team, or gives one of its members this role. |
| `-c`, `--contact <NAME>` | Makes a member one of your contacts. Repeatable, two at most. By default: the template's, the ones Claude marked with `--describe`, or the first member. |
| `--permission-mode <MODE>` | Claude Code permission mode for every member: `acceptEdits`, `auto`, `bypassPermissions`… |
| `--model <MODEL>` | Claude model for every member: `opus`, `sonnet`… |
| `-f`, `--force` | Replaces a team of the same name. |
| `-l`, `--launch` | Launches the team once created. |

```sh
recruit new web --template web --size medium
recruit new web --describe "Shop in Next.js with a Go API" --size small
recruit new web -m "lead:Coordinates" -m "dev:Writes the code" -c lead
recruit new tools --global --template personal --launch
```

A team created with `--member` alone gets recruit's shared rules as its instructions, in the interface's language.

## `recruit list`

```
recruit list [--json]
```

Lists the project's teams and your global teams, each with its number of members and its state: stopped, running, running and attached, trial running (`--dry-run`), or running under recruit 1 (tmux), for a team that recruit 1 left running (see [the FAQ](/recruit/faq/#a-team-still-runs-under-recruit-1)). A `*` marks the project's `default` team. Running teams that belong to none of them come last.

```
Teams of this project (~/code/my-app/.recruit)
  * web    3 members  running, attached
    api    3 members  stopped
Global teams (~/.config/recruit)
    tools    2 members  stopped
```

`--json` prints one object per team: `name`, `scope` (`local` or `global`), `file`, `members`, `running`, `attached`, `dir`, the folder it runs in, and `tmux`, true for a team still running under recruit 1.

## `recruit attach`

```
recruit attach [TEAM]
```

Joins a running team: the one named, else the project's team, else the only one running. When several run and none of these rules picks one, recruit asks which; without a terminal, it lists them and stops. A trial (`--dry-run`) is found too, when the team itself is not running.

## `recruit stop`

```
recruit stop [-y] [TEAM]
```

Stops a running team, chosen as for `attach`: every member's Claude session is closed. `recruit <team> --resume` picks the conversations up again later.

| Option | Effect |
|---|---|
| `-y`, `--yes` | Does not ask for confirmation. |

## `recruit edit`

```
recruit edit [--local] [TEAM]
```

Opens a team's file in your editor: `$VISUAL`, else `$EDITOR`, else `vi`. Without a name, the project's `.recruit/settings.toml`; with one, the project's file if the team is local, else the global file that holds it, else `~/.config/recruit/<team>.toml`. It opens the file even when the team's files do not read, to fix them, and says once the editor closes whether an error remains.

| Option | Effect |
|---|---|
| `--local` | Opens your personal file, `.recruit/settings.local.toml`, created when needed. |

## `recruit templates`

```
recruit templates
```

Lists the built-in project types and sizes, and the members of each: see [Built-in teams](/recruit/reference/templates/).

## Internal commands

Commands that start with `_` (`_member`, `_panel`, `_mod`, `_menu`…) are what recruit runs in the panes, the mod and the menu. They do not show in the help, and are not meant to be typed.
