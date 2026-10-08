---
title: Configuration
description: Every key of a team's file, with its type, its default and its effect.
sidebar:
  order: 2
---

A team's file is TOML. The same format serves everywhere:

| File | Holds |
|---|---|
| `.recruit/settings.toml` | the project's teams, committed |
| `.recruit/settings.local.toml` | your personal settings for the project, merged over `settings.toml` key by key, not committed |
| `~/.config/recruit/<team>.toml` | a global team (in `$XDG_CONFIG_HOME/recruit/` when that variable is set) |

See [Teams](/recruit/guides/teams/) for how they combine.

## A full example

```toml title=".recruit/settings.toml"
default = "web"                   # team launched by a bare `recruit` when the file holds several

[claude]
command = "claude"                # Claude Code executable
config_dir = "~/.claude-work"     # Claude Code profile; better in settings.local.toml

[tmux]                            # read when recruit's tmux server starts
socket = "recruit"
mouse = true
user_config = true                # also read ~/.tmux.conf
options = ["set -g status-position top"]

[teams.web]
description = "Web application"
lang = "en"
instructions = """
- Rules shared by every member.
"""
permission_mode = "auto"
model = "opus"
effort = "high"
args = []
layout = "auto"
columns = 3
rows = 2
dashboard = true

[teams.web.members.coordinator]
role = "The user's entry point: hands out work, tracks progress"
contact = true
instructions = """
- What this member does, its area, how it hands work back.
"""

[teams.web.members.backend]
role = "Server and API"
tab = "Code"
model = "sonnet"
```

## Top level

| Key | Type | Default | Effect |
|---|---|---|---|
| `default` | string | none | The team a bare `recruit` launches when the project's file holds several. Without it, recruit asks which. Read in the project's files. |

## `[claude]`

Applies to every team of the file.

| Key | Type | Default | Effect |
|---|---|---|---|
| `command` | string | `"claude"` | The Claude Code executable, a name looked up in the `PATH` or a path. |
| `config_dir` | string | the environment's | The Claude Code profile the team uses: its configuration folder, given to the members as `CLAUDE_CONFIG_DIR`. An absolute path, or one that starts with `~/`. The folder must exist. See [Claude Code profile](/recruit/guides/claude-code/#claude-code-profile). |

## `[tmux]`

Read when recruit's tmux server starts, that is when the first team is launched in it, except `socket`, which picks the server at each launch. To apply a change, run `tmux -L recruit kill-server`, which stops every team in that server.

| Key | Type | Default | Effect |
|---|---|---|---|
| `socket` | string | `"recruit"` | The name of recruit's tmux server: `tmux -L <socket>`. |
| `mouse` | boolean | `true` | Mouse support: click to focus, drag borders, scroll. |
| `user_config` | boolean | `true` | Also read your own tmux configuration, `~/.tmux.conf` and `~/.config/tmux/tmux.conf`. |
| `options` | list of strings | `[]` | tmux commands applied last, after recruit's settings and yours. |

## Teams

`[teams.<team>]`: one table per team. The team's name is the key: letters (accents included) and digits, `-` and `_`, 40 characters at most, not starting with `-`, and not one of recruit's commands. See [Names](/recruit/guides/teams/#names).

| Key | Type | Default | Effect |
|---|---|---|---|
| `description` | string | none | One line about the team, for you. |
| `lang` | `"fr"` or `"en"` | the interface's language | The team's language: the prompt recruit writes around the instructions, the names of its tabs, its panels and its commands (`/equipe` or `/team`). |
| `instructions` | string | none | Rules shared by every member, added to each one's prompt. |
| `permission_mode` | string | Claude Code's | Claude Code permission mode of every member: `default`, `acceptEdits`, `auto`, `plan`, `bypassPermissions`… |
| `model` | string | Claude Code's | Claude model of every member: an alias (`opus`, `sonnet`, `haiku`, `fable`) or a full model name. |
| `effort` | string | Claude Code's | Effort of every member: `low`, `medium`, `high`, `xhigh` or `max`. |
| `args` | list of strings | `[]` | Extra arguments given to `claude` for every member. |
| `layout` | `"auto"` or `"tabs"` | `"auto"` | `auto`: contacts in a first tab, then the working agents in tabs of `columns × rows` at most. `tabs`: one tab per member. See [Tabs](/recruit/guides/contacts-and-agents/#tabs). |
| `columns` | integer | `3` | Most columns in a tab. |
| `rows` | integer | `2` | Most rows in a tab. |
| `dashboard` | boolean | `true` | The [dashboard and the journal](/recruit/guides/dashboard/), on the right of the contacts. With `false`, the contacts sit side by side. |

## Members

`[teams.<team>.members.<member>]`: one table per member, in the order of the tabs. The member's name is the key, and the address its teammates write to: letters and digits, `-`, `_` and `.`, no spaces, 40 characters at most, not starting with `-` or `.`. Two members of a team cannot differ only by case.

| Key | Type | Default | Effect |
|---|---|---|---|
| `role` | string | required | One line: what the member is responsible for. Its teammates see it. |
| `contact` | boolean | `false` | One of your contacts, in the first tab; two at most. When no member has it, the first member is the contact. See [Contacts](/recruit/guides/contacts-and-agents/). |
| `instructions` | string | none | What this member does, its area, how it hands work back. Added to its prompt. |
| `tab` | string | `"Agents"` | For a working agent: the tab it shares with the agents that have the same one. Cannot be one of recruit's tab names ("Contacts", "Interlocuteurs", "Agents (1)"…). |
| `permission_mode` | string | the team's | Overrides the team's for this member. |
| `model` | string | the team's | Overrides the team's for this member. |
| `effort` | string | the team's | Overrides the team's for this member. |
| `args` | list of strings | `[]` | Extra arguments for this member, after the team's. |

## How values combine

- For `permission_mode`, `model` and `effort`: the member's value, else the team's, else Claude Code's own default. The [team's menu](/recruit/guides/menu/) shows where each value comes from.
- Between `settings.local.toml` and `settings.toml`: tables merge key by key, and any other value of `settings.local.toml`, a list included, replaces the one in `settings.toml`.

## What recruit refuses

recruit checks the files each time it reads them (to launch a team, list, attach, stop or edit one, open the menu…), and stops with the file's name and the reason when:

- a key is unknown, or a value has the wrong type;
- a team's or a member's name breaks the [rules for names](/recruit/guides/teams/#names), or two members of a team differ only by case;
- a member has no role;
- a team has more than two contacts;
- `config_dir` is a relative path;
- a member's `tab` takes the name of one of recruit's tabs.

At launch, it also refuses a team without members, and a `config_dir` whose folder does not exist.

recruit writes into these files too, from `recruit new` and the [team's menu](/recruit/guides/menu/#where-changes-are-written): it keeps their comments.
