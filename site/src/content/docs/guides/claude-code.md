---
title: With Claude Code
description: What each member receives, recruit's mod, the bottom of Claude Code, profiles, and members that stop.
sidebar:
  order: 6
---

Every member is the Claude Code you already know: it reads your code, runs commands and edits files the same way. What recruit adds is the team around it. Each member knows its role, its teammates and who it answers to.

This page explains what each member is told, and how recruit fits with your Claude Code setup. recruit only chooses how each session starts, and adds a small mod when Claude Code can load it.

## What each member receives

Its name, given with `claude -n`, and an added system prompt (`--append-system-prompt-file`) that holds:

- its role and its instructions;
- whether it is a contact or a working agent. A working agent asks the contacts when it is stuck rather than waiting in its terminal, and follows you when you step in;
- the list of its teammates, with their roles and the contacts marked;
- when another session carries a teammate's name, the exact address to write to it, "name [ref]";
- the team's shared rules.

recruit writes this prompt in the team's language (`lang`). The prompt files are in `~/.cache/recruit/prompts/`. To see each member's command line and prompt file without launching anything:

```sh
recruit --print
```

The model, the effort, the permission mode and extra arguments come from the team's file: see [Configuration](/recruit/reference/configuration/#teams).

## recruit's mod

With Claude Code 2.1.287 or newer, each member also loads a small mod of recruit's, with `claude --plugin-dir`. recruit writes it in `~/.cache/recruit/mod/`. The mod decides nothing: it calls recruit back (`recruit _mod`), which does the work. With it:

- the [dashboard](/recruit/guides/dashboard/) shows the model and the effort each member actually uses, its context, a line that sums up what it does, and your account's 5-hour and 7-day usage;
- `/team` (`/equipe` in a French team), typed in a member's prompt, takes the journal to its next size, like `Alt+j`, even while Claude works;
- `/recruit`, typed in a member's prompt, opens the [team's menu](/recruit/guides/menu/), like `Alt+r`;
- changes made in the menu reach the members without a restart, when they can: a model or an effort from the member's next request, a role, instructions or teammates in a note joined to its next message;
- each member keeps an up-to-date view of the team: when its prompt changed since what its conversation last received (a member added, a role rewritten), the new one reaches it once, with its next message. After a compaction, it comes again if it differs from the one the conversation started with. A message that starts with `/` never carries it, and one that would wait more than 3 seconds for it leaves without it: the next message carries it.

The mod changes nothing in your Claude Code settings. Where an organization does not allow the mods its users install, the team works the same, without these.

## The bottom of Claude Code

Only the main contact, the first contact, keeps all of it. The other members:

- go without your status line: the `statusLine` of your settings is left out for their session only, with `--settings`, unless the team's or the member's `args` already give `--settings`. In fullscreen, Claude Code keeps a row for the status line even when it prints nothing, so those members show an empty row there; with no status line in your settings, nothing changes;
- with the mod, also go without the hint line under the prompt (`? for shortcuts`, `esc to interrupt`) and the mode labels on its right.

The prompt and the permission mode line stay: Claude Code does not let them be hidden.

## Claude Code profile

When you use several Claude Code accounts, each with its own configuration folder (`CLAUDE_CONFIG_DIR=~/.claude-work claude`), `[claude] config_dir` picks the one the team uses:

```toml title=".recruit/settings.local.toml"
[claude]
config_dir = "~/.claude-work"
```

It is a personal path: for a local team, write it in `.recruit/settings.local.toml`, not in the committed file. It must be absolute or start with `~/`.

The members start with `CLAUDE_CONFIG_DIR` set, and so does the shell left in their pane. recruit also looks there for open sessions, approved folders and the conversations `--resume` picks up. The folder must exist: run `claude` once with this profile to create it and log in.

## A member that stops

When a member's Claude stops on its own (`/exit`, a crash), its pane starts it again two seconds later on the same conversation: it keeps its name, its role and its history, and takes its settings as the team's files give them then.

- `Ctrl-C` in those two seconds leaves a shell instead.
- A member whose Claude stops twice in a row just after starting is not started again: its pane says so. Once the cause is fixed, `recruit <team> --restart --resume` starts the team again.
- A member removed from the team in those two seconds is not started again: its pane says it is no longer in the team.
- Nothing starts again once the team is stopped.

Running `recruit` again on a running team starts again the members that no longer run and reopens a closed dashboard. When panes were closed, it offers to build the team again, each member resuming its conversation.

## Resuming conversations

```sh
recruit --resume
```

Each member resumes the last conversation that carries its name in this folder. A member that has none starts a new one, and recruit says so. On a team already running, `--resume` is ignored: `recruit --restart --resume` builds it again on its conversations.

Without the mod, a resumed conversation keeps the prompt it started with: after changing roles, launch new sessions rather than `--resume`. With the mod, the member is told its current role and team with its next message, if they changed.
