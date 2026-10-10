---
title: FAQ and troubleshooting
description: Good to know before you start, and what to do when something does not work.
---

## Good to know

### Does closing the terminal stop the team?

No. A team runs in the background, apart from your terminal: closing the window, even with `Cmd+q`, leaves the agents working. `recruit`, or `recruit attach`, finds them where you left them, from any terminal, even over SSH. To stop a team: `Alt+q` then "Quit" (`q`), or `recruit stop`.

### Why does Claude Code ask whether to trust the folder?

Claude Code asks every new session whether to trust a folder it has never approved, and its default answer exits. recruit warns you before launching. To approve the folder once for the whole team, run `claude` there first and accept.

### Can a team run twice?

No: a team runs in one place at a time. Its members are addressed by name, so two copies would get each other's messages. Launching a team that already runs in another folder is refused; from the same folder, recruit joins it.

### What if another session uses a member's name?

Another team, or a session of yours, may use a member's name. recruit stops nothing and renames no one: each member is given the exact address of its teammates who share a name with another session, "name [ref]", and writes to that one. Without it, the member asks its contact, or you. The dashboard names these sessions, if it has room.

Sessions with a member's name already open in the team's folder are refused at launch: they could not be told apart.

### Does a resumed conversation get the new roles?

With [recruit's mod](/recruit/guides/claude-code/#recruits-mod) (Claude Code 2.1.287 or newer), yes: the member is told its current role, instructions and teammates with its next message, if they changed. Without the mod, a resumed conversation keeps the prompt it started with: after changing roles, launch new sessions rather than `--resume`.

### Which language does recruit speak?

French or English. The interface follows `--lang`, then `RECRUIT_LANG`, `LC_ALL` and `LC_MESSAGES`, then the system language. On macOS, terminals often set `LANG=en_US.UTF-8` on their own, so the system language takes precedence over `LANG` there.

A team has its own language, `lang` in its file, set when it is created: the language of its prompts, its tabs and its commands.

### What does a team cost?

Each member is a Claude Code session on your account, and uses it like any other. On top of that, with the mod, Haiku sums up each new request in one short call for the dashboard, and compacting a conversation from the dashboard costs what Claude Code's own compaction costs. The dashboard's bottom line shows your 5-hour and 7-day usage.

### Does recruit change my Claude Code settings?

No. recruit writes nothing in Claude Code's settings files. The members other than the main contact go without your status line for their session only, through `--settings`, and the team's menu writes its changes into the team's files.

## Troubleshooting

### Every member asks "Do you trust this folder?", or closes at once

Claude Code has not approved the folder yet, and Enter alone answers "No, exit". Answer "Yes, I trust this folder" in each pane, or stop the team, run `claude` once in the folder to approve it, and launch again:

```sh
recruit stop -y
claude            # accept, then /exit
recruit
```

### A member stays at work while it does nothing

A command it started may still run past its turn: a development server, a watcher, a command that hangs. Claude Code counts such a session as busy. With a recent Claude Code, the dashboard tells the two apart and shows the member at rest, under ❯, until the command ends. With an older one, the member stays "at work" until the command ends or you stop it in its pane.

### A member stopped and is not started again

When a member's Claude stops twice in a row just after starting, its pane stops trying and says so. Look at the error in the pane, fix the cause (a wrong argument in `args`, a profile not logged in…), then:

```sh
recruit <team> --restart --resume
```

### On macOS, `Alt+j` or `Alt+r` types a character

The terminal sends Option as a character, not as Alt. Set it to send Alt: see [Option on macOS](/recruit/guides/screen/#option-on-macos). Meanwhile, the "menu" and "quit" buttons at the bottom right, and `/recruit` and `/team` in a member's prompt, do the same.

### The dashboard shows no model, context or usage, and `/recruit` is unknown

These come from recruit's mod, which needs Claude Code 2.1.287 or newer. Update Claude Code, then launch the team again. Where an organization does not allow the mods its users install, the team works without them.

### "Claude profile … does not exist"

`[claude] config_dir` names a folder that does not exist yet. Create the profile by running Claude Code with it once, and log in:

```sh
CLAUDE_CONFIG_DIR=~/.claude-work claude
```

### "team … is already running in …"

The team runs in another folder, and a team runs in one place at a time. Join it with `recruit attach <team>`, or stop it with `recruit stop <team>`.

### "already open in this directory: …"

Claude Code sessions with members' names are open in the team's folder, outside the team. Close them first: two sessions would share a name.

### "invalid configuration", "has no role", "contacts (contact = true), 2 at most"

The team's file does not read. The message names the file and the reason: an unknown key or a wrong type, a member without `role`, more than two contacts. `recruit edit` opens the file even so, and says once the editor closes whether an error remains. See [What recruit refuses](/recruit/reference/configuration/#what-recruit-refuses).

### "cannot have this name"

A team's or a member's name, written by hand in the team's file, breaks recruit's [rules for names](/recruit/guides/teams/#names). The message names the file, the name and the reason:

```
recruit: /home/me/my-app/.recruit/settings.toml: member "-dev" of team "web" cannot have this name: letters, digits, -, _ and . only, no spaces, not starting with - or ., at most 40 characters. Rename it in the file.
```

Two members whose names differ only by case get "differs from … by case only". `recruit edit` (or `recruit edit <team>`, `recruit edit --local`) opens the file even so: rename the team or the member there. Until it is fixed, the other recruit commands that read the file stop on it; a global team's file stops all of them, since each one reads all the global teams. A team already running keeps working: its menu opens read-only, and `Alt+q` still detaches from it or quits it.

### "several teams in this project"

Without a terminal to ask in, recruit cannot choose. Name the team (`recruit <team>`), or set `default` at the top of `.recruit/settings.toml`.

### "The [tmux] section … has no effect any more"

recruit 2 no longer uses tmux: teams draw their own screen. The `[tmux]` section of an older file is read and ignored. Remove it from the team's file, and the warning goes with it.

### A team still runs under recruit 1

recruit 1 ran teams in tmux, and such a team keeps running after an upgrade. `recruit` (or `recruit <team>`) finds it, and offers to stop it and start it again on the new screen, each member resuming its conversation. Without a terminal to ask in, it stops there; then:

```sh
recruit stop <team>          # stops it, tmux or not, after confirmation
recruit <team> --resume
```

### Copying a selection does nothing

The copy goes through your terminal. iTerm2 asks you to allow it once, and Terminal.app cannot take it: see [Select and copy](/recruit/guides/screen/#select-and-copy) for what to do.

### A team stopped on its own

A crash, or the machine restarted. Run `recruit`: it starts the team again, each member resuming its conversation, and says so. What happened is in the team's log, `~/.cache/recruit/teams/<team>/server.log`.

### The menu does not open

The menu needs a window of 50 columns and 14 rows at least: enlarge the terminal. With the mod, `/recruit` in a member's prompt opens it too.
