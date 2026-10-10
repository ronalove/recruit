# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (C) 2026 Ronan Lamour
# shellcheck shell=bash disable=SC2154
# What scripts/screenshots.sh asks of recruit's multiplexer for the demo team: `recruit _ctl <state>`, the way the
# tests drive a team without a terminal. Sourced, never run. Reads `session` (the team's name), `state` (its folder)
# and `OWN_RECRUIT` (the recruit to run), set by the script.

# `recruit _ctl` on the demo's team.
ctl() { "${OWN_RECRUIT:-recruit}" _ctl "$state" "$@"; }

# Writes a text the way `_ctl send` reads it: a backslash is `\\`, the rest as it is.
ctl_text() { printf '%s' "$1" | sed 's/\\/\\\\/g'; }

# A pane by its member's name, or a panel's role (dashboard, journal): its id.
pane_of() {
  ctl panes --json | jq -r --arg m "$1" '.[] | select(.role == $m or (.role == "" and .member == $m)) | .id' | head -1
}

# The members' panes (not the panels'), one id a line.
member_panes() { ctl panes --json | jq -r '.[] | select(.role == "") | .id'; }

# Every pane whose text a screenshot may show: members' and panels'. The tab the client is on is not known from here,
# so all of them: stricter.
shown_panes() { ctl panes --json | jq -r '.[].id'; }

# A pane's text.
pane_text() { ctl capture --pane "$1"; } # <pane>

# Keys typed into a pane: `Down` or `Enter`.
pane_key() { # <pane> <Down|Enter>
  case $2 in
    Down) ctl send --pane "$1" '\x1b[B' ;;
    Enter) ctl send --pane "$1" '\r' ;;
    *) return 1 ;;
  esac
}

# A text typed into a pane, as the user would: the text, then Enter half a second later (in one burst, Claude Code takes
# it for a paste and does not send it).
pane_type() { # <pane> <text>
  ctl send --pane "$1" "$(ctl_text "$2")"
  sleep 0.5
  ctl send --pane "$1" '\r'
}

# Where the demo's state is, once it runs.
state_of_team() { echo "$XDG_CACHE_HOME/recruit/teams/$session"; }

# Whether a client is attached.
has_client() {
  [[ $(ctl clients --json 2>/dev/null | jq '.clients | length' 2>/dev/null || echo 0) -gt 0 ]]
}

# The command that attaches to the team, typed by vhs.
attach_command() {
  echo "RECRUIT_TMPDIR=$RECRUIT_TMPDIR XDG_CONFIG_HOME=$XDG_CONFIG_HOME XDG_CACHE_HOME=$XDG_CACHE_HOME ${OWN_RECRUIT:-recruit} attach $session"
}

# The first tab, on the client that is attached; a client starts on its first tab, and goes back to it.
select_first() { has_client && ctl key alt+1 || true; }

# The second tab (the agents'), on the attached client.
select_second() { ctl key alt+2; }

# Whether the team has a second tab.
has_second_tab() { [[ $(ctl panes --json | jq '[.[].tab] | unique | length') -ge 2 ]]; }

# Opens the /recruit menu on the client, as Alt+r does.
open_menu() { ctl key alt+r; }

# A word shown on the client for a while, for vhs to wait for: typed into a member's input (the video's last seconds
# are cut off).
say_on_client() { ctl send --pane "$2" "$1"; } # <word> <member>

# The rectangle of a panel or a member on the screen, in cells (`x y width height`), for cutting it out of a screenshot
# of the whole tab: read in its frame on the composed screen (a frame is drawn with corners, its title in the top
# border), as a pane's inside with the title row above it.
pane_rect() { # <pane> <title>
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
    print(left, y + 1, right - left + 1, bottom - y)
    break
' "$2"
}

# Stops the demo's team, and its server with it.
stop_team() { "${OWN_RECRUIT:-recruit}" stop "$session" >/dev/null 2>&1 || true; }

# Stops everything the script started.
stop_all() {
  local team
  for team in demo demo-fr; do "${OWN_RECRUIT:-recruit}" stop "$team" >/dev/null 2>&1 || true; done
}

# Moves the focus of the attached client to member `$1`, in its tab (⌥n goes from one member to the next).
focus_member() { # <member>
  local steps
  steps=$(ctl panes --json | jq -r --arg m "$1" '
    (map(select(.member == $m and .role == "")) | .[0].tab) as $tab
    | [.[] | select(.tab == $tab and .role == "") | .member] as $members
    | ($members | index($m))')
  [[ -n $steps && $steps != null ]] || return 1
  while ((steps-- > 0)); do
    ctl key alt+n
    sleep 0.4
  done
}

# Whether the header of each member on screen, and its card on the dashboard, say the same state: at work (a spinner),
# waiting (⚑), at rest (◷, or the sign of a command that runs), none known (◌) set aside. The two come from different
# places, a card up to a second or two after its header: an image must not show them apart. Says which differ.
states_agree() {
  ctl capture | python3 -I -c '
import re, sys
text = sys.stdin.read().split("\n")
def kind(glyph):
    if "⠀" <= glyph <= "⣿":
        return "work"
    return {"⚑": "wait", "◷": "rest", "◌": None}.get(glyph, "rest")
headers, cards = {}, {}
for row in text:
    for glyph, name in re.findall(r"[┏╭][━─] (\S) (\S+) ", row):
        headers[name] = kind(glyph)
    for glyph, name in re.findall(r"╻ (\S) (\S+)", row):
        cards[name] = kind(glyph)
apart = [n for n in headers if n in cards and headers[n] and cards[n] and headers[n] != cards[n]]
if apart:
    print("states apart: " + ", ".join(f"{n} (header {headers[n]}, card {cards[n]})" for n in apart), file=sys.stderr)
    sys.exit(1)
'
}
