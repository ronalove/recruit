# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (C) 2026 Ronan Lamour
# shellcheck shell=bash disable=SC2154
# What scripts/screenshots.sh asks of the multiplexer under the demo team: the same questions for tmux (`-L rtest-shots`)
# and for recruit's own (`recruit _ctl <state>`). Sourced, never run. Reads `backend` (tmux or native), `SOCKET`,
# `session` (the team's name), `state` (its folder) and `OWN_RECRUIT` (the recruit to run), set by the script.

# Everything a tmux command takes.
t() { tmux -L "$SOCKET" "$@"; }

# `recruit _ctl` on the demo's team.
ctl() { "${OWN_RECRUIT:-recruit}" _ctl "$state" "$@"; }

# Writes a text the way `_ctl send` reads it: a backslash is `\\`, the rest as it is.
ctl_text() { printf '%s' "$1" | sed 's/\\/\\\\/g'; }

# A pane by its member's name, or a panel's role (dashboard, journal): its id.
pane_of() {
  if [[ $backend == native ]]; then
    ctl panes --json | jq -r --arg m "$1" '.[] | select(.role == $m or (.role == "" and .member == $m)) | .id' | head -1
  else
    t list-panes -s -t "=$session" -F '#{pane_id} #{?@recruit_role,#{@recruit_role},#{@recruit_member}}' |
      awk -v m="$1" '$2 == m { print $1 }'
  fi
}

# The members' panes (not the panels'), one id a line.
member_panes() {
  if [[ $backend == native ]]; then
    ctl panes --json | jq -r '.[] | select(.role == "") | .id'
  else
    t list-panes -s -t "=$session" -F '#{pane_id} #{@recruit_role}' | awk 'NF == 1 { print $1 }'
  fi
}

# Every pane's id: members' and panels'.
all_panes() {
  if [[ $backend == native ]]; then
    ctl panes --json | jq -r '.[].id'
  else
    t list-panes -s -t "=$session" -F '#{pane_id}'
  fi
}

# A pane's text.
pane_text() { # <pane>
  if [[ $backend == native ]]; then ctl capture --pane "$1"; else t capture-pane -p -t "$1"; fi
}

# Keys typed into a pane: `Down` or `Enter`.
pane_key() { # <pane> <Down|Enter>
  if [[ $backend == native ]]; then
    case $2 in
      Down) ctl send --pane "$1" '\x1b[B' ;;
      Enter) ctl send --pane "$1" '\r' ;;
      *) return 1 ;;
    esac
  else
    t send-keys -t "$1" "$2"
  fi
}

# A text typed into a pane, as the user would: the text, then Enter half a second later (in one burst, Claude Code takes
# it for a paste and does not send it).
pane_type() { # <pane> <text>
  if [[ $backend == native ]]; then
    ctl send --pane "$1" "$(ctl_text "$2")"
    sleep 0.5
    ctl send --pane "$1" '\r'
  else
    t send-keys -t "$1" -l "$2"
    sleep 0.5
    t send-keys -t "$1" Enter
  fi
}

# The panes whose text the screenshot may show (tmux: the current window; native: every pane, the tab the client is on
# not being known from here: stricter).
shown_panes() {
  if [[ $backend == native ]]; then
    ctl panes --json | jq -r '.[].id'
  else
    t list-panes -t "=$session:" -F '#{pane_id}'
  fi
}

# Where the demo's state is, once it runs.
state_of_team() {
  if [[ $backend == native ]]; then
    echo "$XDG_CACHE_HOME/recruit/teams/$session"
  else
    t show-options -v -t "=$session:" @recruit_state
  fi
}

# Whether a client is attached.
has_client() {
  if [[ $backend == native ]]; then
    [[ $(ctl clients --json 2>/dev/null | jq '.clients | length' 2>/dev/null || echo 0) -gt 0 ]]
  else
    [[ -n $(t list-clients -t "=$session" -F '#{client_name}' | head -1) ]]
  fi
}

# The first client's name (tmux), for what is sent to it.
client_name() { t list-clients -t "=$session" -F '#{client_name}' | head -1; }

# The command that attaches to the team, typed by vhs.
attach_command() {
  if [[ $backend == native ]]; then
    echo "RECRUIT_BACKEND=native RECRUIT_TMPDIR=$RECRUIT_TMPDIR XDG_CONFIG_HOME=$XDG_CONFIG_HOME XDG_CACHE_HOME=$XDG_CACHE_HOME ${OWN_RECRUIT:-recruit} attach $session"
  else
    echo "tmux -L $SOCKET attach -t =$session"
  fi
}

# The first tab, on the client that is attached or will be.
select_first() {
  if [[ $backend == native ]]; then
    # A client starts on its first tab; one already there goes back to it.
    has_client && ctl key alt+1 || true
  else
    t select-window -t "=$session:$(t list-windows -t "=$session" -F '#{window_index}' | head -1)"
  fi
}

# The second tab (the agents'), on the attached client; tmux: the window of the session, which the next client opens on.
select_second() {
  if [[ $backend == native ]]; then
    ctl key alt+2
  else
    t select-window -t "=$session:$(t list-windows -t "=$session" -F '#{window_index}' | sed -n 2p)"
  fi
}

# Whether the team has a second tab.
has_second_tab() {
  if [[ $backend == native ]]; then
    [[ $(ctl panes --json | jq '[.[].tab] | unique | length') -ge 2 ]]
  else
    [[ -n $(t list-windows -t "=$session" -F '#{window_index}' | sed -n 2p) ]]
  fi
}

# Opens the /recruit menu on the client, as Alt+r does.
open_menu() { # <lang>
  if [[ $backend == native ]]; then
    ctl key alt+r
  else
    "${OWN_RECRUIT:-recruit}" --lang "$1" _menu "$state" --client "$(client_name)" --popup >/dev/null 2>&1
  fi
}

# A word shown on the client for a while, for vhs to wait for. tmux: its status line; native: typed into the lead's
# input (the video's last seconds are cut off).
say_on_client() { # <word> <member>
  if [[ $backend == native ]]; then
    ctl send --pane "$2" "$1"
  else
    t display-message -c "$(client_name)" -d 10000 "$1"
  fi
}

# The rectangle of a panel or a member on the screen, in cells (`x y width height`), for cutting it out of a screenshot
# of the whole tab. tmux: from its pane; native: from its frame on the composed screen (a frame is drawn with corners:
# its title in the top border).
pane_rect() { # <pane> <title>
  if [[ $backend == native ]]; then
    ctl capture | python3 -I -c '
import sys
title = sys.argv[1]
rows = sys.stdin.read().split("\n")
TOP, TOPR, BOT = "┏╭", "┓╮", "┗╰"
for y, row in enumerate(rows):
    at = row.find(" " + title + " ")
    if at < 0:
        continue
    left = max((row.rfind(c, 0, at) for c in TOP), default=-1)
    if left < 0:
        continue
    right = min((i for i in (row.find(c, at) for c in TOPR) if i >= 0), default=-1)
    if right < 0:
        continue
    bottom = next((j for j in range(y + 1, len(rows)) if len(rows[j]) > left and rows[j][left] in BOT), len(rows) - 1)
    # As tmux gives a pane: inside its frame, the title row above it.
    print(left, y + 1, right - left + 1, bottom - y)
    break
' "$2"
  else
    t display-message -p -t "$1" '#{pane_left} #{pane_top} #{pane_width} #{pane_height}'
  fi
}

# Stops the demo's team (tmux: its session; native: the team, and its server with it).
stop_team() {
  if [[ $backend == native ]]; then
    "${OWN_RECRUIT:-recruit}" stop "$session" >/dev/null 2>&1 || true
  else
    t kill-session -t "=$session" 2>/dev/null || true
  fi
}

# Stops everything the script started.
stop_all() {
  if [[ $backend == native ]]; then
    local team
    for team in demo demo-fr; do "${OWN_RECRUIT:-recruit}" stop "$team" >/dev/null 2>&1 || true; done
  else
    t kill-server 2>/dev/null || true
  fi
}
