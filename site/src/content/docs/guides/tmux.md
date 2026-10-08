---
title: In tmux
description: recruit's tmux server, its keys, Option on macOS, and how to detach and come back.
sidebar:
  order: 5
---

Your team keeps working when you leave. Close the terminal, come back tomorrow, from another machine if you like: everyone is where you left them. This page gives the few keys you need to move around, leave and come back.

Under the hood, a team runs in a tmux session named after it, in recruit's own tmux server, `tmux -L recruit`. Your other tmux sessions are not touched.

## What recruit sets up

recruit's tmux server is set up for Claude Code: Shift+Enter inserts a new line, notifications reach your terminal, the mouse works, colors and the clipboard too, and each pane shows the name of its member on its top border. Tabs are numbered from 1. On the right of the status line: recruit's version and two buttons, "menu" and "quit".

Your own tmux configuration is read too, `~/.tmux.conf` then `~/.config/tmux/tmux.conf`, when they exist. It comes after recruit's basic settings (tab numbers, status line, `Alt` keys), so it can change them. What Claude Code needs (mouse, colors, clipboard, keys) is set after it, so that it holds, and `[tmux] options` come last. `[tmux]` in the team's file changes this, and the server's name: see [Configuration](/recruit/reference/configuration/#tmux).

## Keys

| Key | What it does |
|---|---|
| `Alt+1`…`Alt+9` | go to a tab |
| `Alt+Shift+←` / `Alt+Shift+→` | previous / next tab |
| `Ctrl-b n` / `Ctrl-b p` | next / previous tab, without Alt |
| `Ctrl-b z` | zoom on a pane, and back |
| `Ctrl-b d` | detach: the team keeps running |
| `Alt+j` | journal full, reduced, then hidden (or `/team`, `/equipe` in a French team, in a member's prompt) |
| `Alt+r`, or the "menu" button | the [team's menu](/recruit/guides/menu/) (or `/recruit` in a member's prompt) |
| `Alt+q`, or the "quit" button | a small menu: Detach (`d`, the team keeps running), Quit (`q`, stops the team), Cancel (`c`) |

A click on a tab or a pane also works, and so does a click on a member's card on the dashboard. `Ctrl-b` is tmux's prefix, unless your configuration changes it.

## Option on macOS

On macOS, Alt is the Option key: recruit writes it ⌥ in the dashboard, the journal and the status line (⌥j, ⌥q, ⌥r).

The terminal has to send Option as Alt. Otherwise Option+letter types a special or accented character, depending on the keyboard layout, and tmux sees no shortcut.

| Terminal | Setting |
|---|---|
| Ghostty | `macos-option-as-alt = left` (or `right`, or `true`) in its configuration. Ghostty only does it by default for U.S. layouts. |
| Terminal.app | Profiles › Keyboard › Use Option as Meta Key |
| iTerm2 | Profiles › Keys, left Option key set to "Esc+" |

## Detach and come back

Detach with `Alt+q` or `Ctrl-b d`. The team keeps working: closing the terminal, even with `Cmd+q`, closes the window, not the team. To come back, from any terminal, even over SSH:

```sh
recruit            # in the project: joins the running team
recruit attach     # the project's team, or the only one running
recruit attach web # a given team
recruit list       # the teams, and which ones are running
```

Running `recruit` again on a running team also starts again the members that no longer run, and reopens a closed dashboard. When panes were closed, it offers to build the team again, each member resuming its conversation.

## Stop

`Alt+q` then "Quit" (`q`), "Quit" in the team's menu, or from a shell:

```sh
recruit stop       # asks first
recruit stop -y    # does not ask
```

Every member's Claude session is closed. `recruit --resume` later picks up each member's last conversation.

## tmux settings

recruit reads `[tmux]` when its tmux server starts, that is with the first team launched. To apply a change, stop the server, which stops every team running in it:

```sh
tmux -L recruit kill-server
```
