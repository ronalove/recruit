#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (C) 2026 Ronan Lamour
# Screenshots for the documentation site and the README: a demo team, with real Claude sessions, in recruit's own
# multiplexer, filmed by vhs.
#
#   scripts/screenshots.sh                 everything, in English then in French
#   scripts/screenshots.sh --lang en       one language (en, fr)
#   scripts/screenshots.sh --only team     only the team's screenshots; `cli`, the tapes of scripts/demo/tapes (no
#                                          team); `anim`, the animation
#   scripts/screenshots.sh --keep          leave the demo team running at the end, to look at it (`recruit attach demo`)
#   RECRUIT_BIN=target/day5/recruit scripts/screenshots.sh   a recruit already built, in place of a `cargo build`
#   SHOTS_OUT=… SHOTS_MEDIA=… scripts/screenshots.sh         other folders than the site's, to try without replacing it
#
# Writes into site/src/assets/screenshots/: team.png, dashboard.png, journal.png, menu.png, agents.png, those of
# notice.png, zoom.png, scripts/demo/tapes/*.tape and demo.gif (for the README); into site/public/media/: demo.mp4 and demo.webm. With a -fr
# suffix in French. The screenshots are rendered at twice the size (Retina): show them at half.
#
# The demo (scripts/demo/project, its team in .recruit/settings.toml, the tasks in scripts/demo/tasks.<lang>.tsv) runs
# in a copy under /tmp/recruit-shots, with XDG_CONFIG_HOME and XDG_CACHE_HOME of its own, never the user's teams. Its
# members use the user's Claude Code profile, on light models: a few minutes of sonnet and haiku per language. The profile's settings.json is checked to be the same at the end, and the
# demo's conversations are removed from it. An image that shows the account's plan or a home folder is not kept.
#
# Needs vhs (brew install vhs), ffmpeg, jq, python3 and claude. What it asks of the multiplexer is in
# scripts/shots-backend.sh.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
DEMO=$ROOT/scripts/demo
OUT=${SHOTS_OUT:-$ROOT/site/src/assets/screenshots}
MEDIA=${SHOTS_MEDIA:-$ROOT/site/public/media}

# Size of the terminal (columns × rows) for the team's window and the command line tapes.
TEAM_SIZE=180x48
CLI_SIZE=100x30
# Rendering: twice the size of a 14 px font, for Retina screens (the pixel sizes in `tape` depend on it).
FONT_SIZE=28
# The animation: about this long, in seconds, the start (the request typed) at its own pace and the rest sped up; the
# video this wide, the GIF that wide, in pixels.
ANIM_LENGTH=24
VIDEO_WIDTH=1600
GIF_WIDTH=1200

die() {
  echo "screenshots: $*" >&2
  exit 1
}

langs=(en fr)
only=all
keep=false
while (($#)); do
  case $1 in
    --lang) langs=("$2"); shift ;;
    --only) only=$2; shift ;;
    --keep) keep=true ;;
    -h | --help) sed -n '4,26s/^# \{0,1\}//p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done
for lang in "${langs[@]}"; do [[ $lang == en || $lang == fr ]] || die "unknown language: $lang"; done
[[ $only =~ ^(all|team|cli|anim)$ ]] || die "--only takes team, cli or anim"

for tool in vhs ffmpeg claude jq python3; do command -v "$tool" >/dev/null || die "$tool not found"; done

profile=${CLAUDE_CONFIG_DIR:-$HOME/.claude}
settings_sum() { shasum "$profile/settings.json" 2>/dev/null || echo none; }
before=$(settings_sum)

# The recruit to film: the one given (`RECRUIT_BIN`, a copy kept apart, say), or a fresh debug build.
if [[ -n ${RECRUIT_BIN:-} ]]; then
  RECRUIT_BIN=$(cd "$(dirname "$RECRUIT_BIN")" && pwd)/$(basename "$RECRUIT_BIN")
  [[ -x $RECRUIT_BIN ]] || die "RECRUIT_BIN is not an executable: $RECRUIT_BIN"
else
  (cd "$ROOT" && cargo build --quiet)
  RECRUIT_BIN=$ROOT/target/debug/recruit
fi
mkdir -p "$OUT" "$MEDIA"
# Always the same folder: Claude Code asks once whether to trust it, and the screenshots show its name.
tmp=/tmp/recruit-shots
[[ ! -e $tmp || -e $tmp/.recruit-shots ]] || die "$tmp exists and is not this script's"
running=$(cat "$tmp/.recruit-shots" 2>/dev/null || true)
[[ -n $running ]] && kill -0 "$running" 2>/dev/null && die "already running (pid $running)"
rm -rf "$tmp"
mkdir -p "$tmp"
echo $$ >"$tmp/.recruit-shots"
# Where the films write: an image goes to its place in the site once it is known to show nothing private, so that a
# try that fails never takes away the image that was there.
STAGE=$tmp/stage
mkdir -p "$STAGE"
export XDG_CONFIG_HOME=$tmp/config XDG_CACHE_HOME=$tmp/cache
# `recruit` on the PATH is the one filmed: a link to it, under the name it is run by.
mkdir -p "$tmp/bin"
ln -sf "$RECRUIT_BIN" "$tmp/bin/recruit"
export PATH=$tmp/bin:$PATH
OWN_RECRUIT=$tmp/bin/recruit
# Short: the server's socket lives there (a path of 100 characters at most). RECRUIT_BACKEND: a recruit that still
# has tmux needs it; one that has not ignores it.
export RECRUIT_BACKEND=native RECRUIT_TMPDIR=$tmp/run
mkdir -p "$RECRUIT_TMPDIR"
# What is asked of the multiplexer.
# shellcheck source=scripts/shots-backend.sh
source "$ROOT/scripts/shots-backend.sh"
# The dashboard leaves out the usage of the account, the user's and not the demo's.
export RECRUIT_NO_USAGE=1
started=$(date +%s)

cleanup() {
  if ! $keep; then
    stop_all
    rm -rf "$tmp"
    # The conversations of the demo's sessions, in the profile: those of the folders under $tmp only (Claude Code
    # names a folder's after its real path, each character but letters and digits a dash).
    local key
    key=$(cd /tmp && pwd -P | sed 's/[^A-Za-z0-9]/-/g')-recruit-shots-
    rm -rf "$profile/projects/$key"*
  fi
  [[ $(settings_sum) == "$before" ]] || echo "screenshots: WARNING: $profile/settings.json changed" >&2
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# A tape for vhs: the shared settings, the size, then the steps (stdin), and a second at the end: vhs could leave
# before writing a screenshot that ends a tape. `{{out}}` (the staging folder) and `{{sfx}}` are replaced.
tape() { # <file> <cols>x<rows> <sfx>
  local cols=${2%x*} rows=${2#*x}
  {
    echo 'Set Shell "bash"'
    echo "Set FontSize $FONT_SIZE"
    # The pixel size that gives exactly <cols> × <rows> at 28 px, measured (vhs 0.12): cells of 17.144 × 33.49 pixels,
    # and a margin (good up to 187 columns); even, as a video wants it.
    echo "Set Width $(((cols * 17 + 2 * PADDING + 28) / 2 * 2))"
    echo "Set Height $((((rows * 336 + 9) / 10 + 2 * PADDING + BAR + 3) / 2 * 2))"
    echo "Set Padding $PADDING"
    echo 'Set WindowBar Colorful'
    echo "Set WindowBarSize $BAR"
    echo 'Set BorderRadius 16'
    echo 'Set TypingSpeed 0'
    echo 'Set Theme "Builtin Dark"'
    sed -e "s#{{out}}#$STAGE#g" -e "s#{{sfx}}#$3#g"
    echo 'Sleep 1s'
  } >"$1"
}
PADDING=24
BAR=56

# Films a tape from its steps (stdin), in the demo's project.
# With `FILM_WATCH=1` (a team runs), the screen the client shows is read while the film is taken, and what is taken is
# never kept if the account's plan, a tip about it or a home folder showed at any moment of it: the image itself is
# checked, not the screen before or after. Returns 3 then.
film() { # <name> <size> <sfx>
  tape "$tmp/$1.tape" "$2" "$3"
  local seen="$tmp/private-seen" over="$tmp/film-over" watcher="" rc=0
  rm -f "$seen" "$over"
  if [[ ${FILM_WATCH:-} == 1 ]]; then
    (
      while [[ ! -e $over ]]; do
        ctl capture 2>/dev/null | grep -qE "$PRIVATE" && touch "$seen"
        sleep 0.3
      done
    ) &
    watcher=$!
  fi
  (cd "$project" && vhs "$tmp/$1.tape" >/dev/null 2>&1) || rc=$?
  if [[ -n $watcher ]]; then
    touch "$over"
    wait "$watcher" 2>/dev/null || true
  fi
  [[ ! -e $seen ]] || return 3
  return $rc
}

# Cuts a pane out of a screenshot of its whole tab, with its title row above it, and frames it with the padding of the
# terminal. The cells are 17.144 × 33.49 pixels at 28 px, measured on a screenshot (vhs 0.12); a few pixels less at the
# bottom keep the next row out.
crop() { # <screenshot> <out> <pane> <title>
  local x y w h
  read -r x y w h < <(pane_rect "$3" "$4" |
    awk -v p="$PADDING" -v b="$BAR" '{ printf "%d %d %d %d\n", p + $1 * 17.144, p + b + ($2 - 1) * 33.49, $3 * 17.144, ($4 + 1) * 33.49 - 4 }')
  ffmpeg -loglevel error -y -i "$1" \
    -vf "crop=$w:$h:$x:$y,pad=iw+$((2 * PADDING)):ih+$((2 * PADDING)):$PADDING:$PADDING:black" "$2"
}

# The member states the dashboard last saw: `working 2`, `idle 1`, …
count() { jq -r --arg s "$1" '[.members[] | select(.state == $s)] | length' "$states" 2>/dev/null || echo 0; }

# Waits until a condition (a shell command) holds, at most <seconds>.
wait_for() { # <seconds> <what> <command…>
  local deadline=$(($(date +%s) + $1)) what=$2
  shift 2
  until "$@"; do
    (($(date +%s) < deadline)) || { echo "screenshots: no $what after a while, going on" >&2; return 1; }
    sleep 1
  done
}

# Claude Code asks whether to trust a folder it has never been approved in, "No, exit" first: yes.
trust() {
  local pane
  for pane in $(member_panes); do
    if pane_text "$pane" | grep -q "trust this folder"; then
      pane_key "$pane" Down
      pane_key "$pane" Enter
    fi
  done
}

# Whether the tab on screen shows what a public image must not: the account's plan (in Claude Code's welcome, on top
# of a conversation that has not said much yet), its usage (Claude Code's warning near a limit), a tip that names a
# paid plan (guest passes), or a home folder.
PRIVATE="Claude (Max|Pro|Team|Enterprise|API)|% of your [a-z0-9 -]*limit|guest passes|/passes|/Users/|/home/|$HOME"
private() { ctl capture | grep -qE "$PRIVATE"; }

# Films a screenshot of the tab on screen once nothing private shows and each member's header and card say the same
# state (the card comes from `claude agents`, a second or two late), before and after: four tries, 15 s apart; an image
# that still shows something is removed.
film_clean() { # <name> <size> <sfx> <image>  (steps on stdin)
  local steps try
  steps=$(cat)
  for try in 1 2 3 4; do
    # A try again: what was at work has probably finished, so it is given work again (`REFRESH`, a command).
    [[ $try -eq 1 || -z ${REFRESH:-} ]] || $REFRESH
    if ! private && wait_for 30 "the headers and the cards to agree" states_agree; then
      local vector
      vector=$(states_vector)
      # What the client starts on, set again for each try: a client that left takes the next one back to the tab it had.
      # shellcheck disable=SC2086
      [[ -z ${VIEW:-} ]] || view $VIEW
      local rc=0
      film "$1" "$2" "$3" <<<"$steps" || rc=$?
      [[ $rc -eq 0 || $rc -eq 3 ]] || die "vhs failed on $1"
      # Nothing private showed while the film was taken, and nothing changed state.
      if [[ $rc -eq 0 ]] && ! private && states_agree && [[ $(states_vector) == "$vector" ]]; then
        mv -f "$STAGE/$(basename "$4")" "$4"
        return 0
      fi
    fi
    echo "screenshots: $1 would show the account's plan, a home folder or states that differ, again ($try)" >&2
    [[ -n ${REFRESH:-} ]] || sleep 15
  done
  rm -f "$STAGE/$(basename "$4")"
  echo "screenshots: WARNING: no new $4: it showed the account's plan, a home folder or states that differ" >&2
  return 1
}

# The team launched in the demo's project, its members up and its folder trusted. Sets `session`, `state`, `states`.
launch() { # <lang>
  local team=demo members
  [[ $1 == fr ]] && team=demo-fr
  session=$team
  (cd "$project" && RECRUIT_LANG=$1 recruit --detach "$team")
  state=$(state_of_team)
  states=$state/states.json
  members=$(member_panes | wc -l | tr -d ' ')
  up() {
    trust
    [[ $(jq '[.members[] | select(.state == "idle")] | length' "$states" 2>/dev/null) == "$members" ]]
  }
  wait_for 120 "members up" up || true
  sleep 3
  # What the user's hooks may have left in the project at startup (an index's ignore files, say): committed, so the
  # members do not talk about it.
  if [[ -n $(git -C "$project" status --porcelain) ]]; then
    git -C "$project" add -A
    git -C "$project" -c user.name=demo -c user.email=demo@example.com commit -qm "Local tooling"
  fi
}

# Types a message in a member's pane, as the user would.
send() { # <member> <message>
  local pane
  pane=$(pane_of "$1")
  [[ -n $pane ]] || die "no member $1 in the demo team"
  pane_type "$pane" "$2"
}

# What the next film of the team starts on: the first tab or the second, and then one of `menu` (the /recruit menu opened
# over it), `zoom` (the focused member zoomed) or a member's name (the focus on it, in its tab). A client keeps the tab
# it was on, so the keys go to it once vhs's terminal is attached, in the background (about two minutes to wait for it
# at most).
view() { # <first|second> [menu|zoom|<member>]
  (
    local waited=0
    until has_client; do
      sleep 0.5
      ((++waited < 240)) || exit 0
    done
    sleep 1
    if [[ $1 == first ]]; then select_first; else select_second; fi
    sleep 0.5
    case ${2:-} in
      "") ;;
      menu) open_menu ;;
      zoom) ctl key alt+z ;;
      *) focus_member "$2" || true ;;
    esac
    true
  ) &
}

# The tasks of scripts/demo/tasks.<lang>.tsv: `member<TAB>message`, one a line; `#` starts a comment.
tasks() { grep -v '^#' "$DEMO/tasks.$1.tsv" | grep .; }

# The members of one state among the working agents (not the contacts), one a line.
agents_in() { # <state>
  jq -r --arg s "$1" --slurpfile team "$state/team.json" '
    ($team[0].members | map(select(.contact) | .name)) as $contacts
    | .members | to_entries[] | select(.key | IN($contacts[]) | not) | select(.value.state == $s) | .key' \
    "$states" 2>/dev/null
}

# What the dashboard last saw of every member, in one line: it changes when any of them changes state.
states_vector() { jq -c '[.members | to_entries[] | [.key, .value.state]]' "$states" 2>/dev/null; }

# The state the dashboard last saw of a member.
state_of() { jq -r --arg m "$1" '.members[$m].state // empty' "$states" 2>/dev/null; }

# Gives work to what is at rest among `members` (and the lead), so that it is at work when the next film is taken: a
# long enough reading of the project, with nothing to ask permission for.
stir() { # <lang> <member…>
  local lang=$1 member task
  shift
  task="Read every file of src/, test/ and public/ one at a time, each with a command of its own, then read them all a second time in reverse order, then say in one line what each does. Do not hurry."
  [[ $lang == fr ]] && task="Lis chaque fichier de src/, test/ et public/ l'un après l'autre, chacun par sa propre commande, puis relis-les tous une seconde fois en sens inverse, puis dis en une ligne ce que chacun fait. Ne te presse pas."
  for member in "$@"; do
    [[ $(state_of "$member") == idle ]] && send "$member" "$task"
  done
  return 0
}

# The team at work: launched, given its tasks, and filmed once its members are working, idle and waiting.
shoot_team() { # <lang> <sfx>
  local lang=$1 sfx=$2
  launch "$lang"
  FILM_WATCH=1
  local member message
  while IFS=$'\t' read -r member message; do send "$member" "$message"; done < <(tasks "$lang")

  # The lead at work (the focus is on it, where one types), an agent waiting in another tab (the ⚑ in the bar), once the
  # first messages went round; on a try again, someone at work.
  local given try=1 taken=false lead
  lead=$(tasks "$lang" | head -1 | cut -f1)
  given=$(date +%s)
  # The scene: the lead and an agent at work, another agent waiting, and no state changed for four seconds, so that the
  # header and the card of each say the same when the film is taken.
  local last_vec="" last_change=0
  scene() {
    (($(date +%s) - given >= 12)) || return 1
    local vec
    vec=$(states_vector)
    if [[ $vec != "$last_vec" ]]; then
      last_vec=$vec
      last_change=$(date +%s)
      return 1
    fi
    (($(date +%s) - last_change >= 4)) || return 1
    [[ $(state_of "$lead") == working && -n $(agents_in waiting) && -n $(agents_in working) ]]
  }
  # The work the tasks gave comes to an end: what rests is given some more, until the lead and an agent are at work.
  local stirred=0
  scene_or_stir() {
    scene && return 0
    # Not again before the last work has shown in the states.
    if (($(date +%s) - stirred >= 25)); then
      stir "$lang" "$lead" $(agents_in idle | head -2)
      stirred=$(date +%s)
    fi
    return 1
  }
  # Before each of the next films: the lead and two agents at work again (up to a minute).
  at_work_again() {
    stirred=0
    given=0
    wait_for 90 "the lead and an agent at work" scene_or_stir || true
  }
  local attach dash_title=Dashboard
  attach=$(attach_command)
  [[ $lang == fr ]] && dash_title="Tableau de bord"

  # The first tab: the contacts, the dashboard and the reduced journal. Filmed again, three times at most, when the
  # scene changed meanwhile; when it does not come back, the image taken before stays.
  VIEW="first"
  REFRESH=at_work_again
  for try in 1 2 3; do
    if ! wait_for 240 "the scene (try $try)" scene_or_stir && $taken; then break; fi
    film_clean team "$TEAM_SIZE" "$sfx" "$OUT/team$sfx.png" <<EOF || break
Hide
Type "$attach"
Enter
Sleep 2s
Show
Sleep 500ms
Screenshot "{{out}}/team{{sfx}}.png"
EOF
    taken=true
    scene && break
    echo "screenshots: the scene changed while filming, again ($try)" >&2
  done
  # The dashboard's close-up, out of the same image: its whole frame.
  $taken && crop "$OUT/team$sfx.png" "$OUT/dashboard$sfx.png" "$(pane_of dashboard)" "$dash_title"

  # The /recruit menu over the team, on the sheet of the member who has the focus (the lead). Opened as Alt+r does, once
  # vhs's terminal is attached.
  at_work_again
  VIEW="first menu"
  film_clean menu "$TEAM_SIZE" "$sfx" "$OUT/menu$sfx.png" <<EOF || true
Hide
Type "$attach"
Enter
Sleep 5s
Show
Sleep 500ms
Screenshot "{{out}}/menu{{sfx}}.png"
Escape
EOF
  wait

  # A tab of agents, in a grid: the focus on one at work, so that its thick frame and the red one of the agent that
  # waits show together.
  if has_second_tab; then
    at_work_again
    local at_work
    at_work=$(agents_in working | head -1)
    VIEW="second $at_work"
    film_clean agents "$TEAM_SIZE" "$sfx" "$OUT/agents$sfx.png" <<EOF || true
Hide
Type "$attach"
Enter
Sleep 6s
Show
Sleep 500ms
Screenshot "{{out}}/agents{{sfx}}.png"
EOF
    wait
  fi

  # The journal in full (reduced at launch: hidden, then full), cut out of the first tab.
  recruit _panel toggle "$state" >/dev/null
  recruit _panel toggle "$state" >/dev/null
  VIEW="first"
  film_clean full "$TEAM_SIZE" "$sfx" "$tmp/full.png" <<EOF &&
Hide
Type "$attach"
Enter
Sleep 3s
Show
Sleep 500ms
Screenshot "{{out}}/full.png"
EOF
    crop "$tmp/full.png" "$OUT/journal$sfx.png" "$(pane_of journal)" Journal
  wait

  VIEW=""
  REFRESH=""

  # The notice: an agent out of view (the first tab is on screen) is asked for something that needs a permission; once
  # it waits, the notice shows for six seconds at the top right, and the badge stays on its tab.
  local idle task
  has_idle_agent() { [[ -n $(agents_in idle) ]]; }
  wait_for 150 "an agent at rest" has_idle_agent || true
  idle=$(agents_in idle | head -1)
  task=$(tasks "$lang" | awk -F '\t' '/outdated/ { print $2; exit }')
  [[ -n $task ]] || task='Run `bun outdated` and tell me what it says.' # shellcheck disable=SC2016
  if [[ -n $idle ]]; then
    view first
    (
      until has_client; do sleep 0.5; done
      sleep 4
      send "$idle" "$task"
    ) &
    local rc=0
    film notice "$TEAM_SIZE" "$sfx" <<EOF || rc=$?
Hide
Type "$attach"
Enter
Sleep 2s
Show
Wait+Screen@120s /(attend ta réponse|needs your answer)/
Sleep 300ms
Screenshot "{{out}}/notice{{sfx}}.png"
EOF
    if [[ $rc -eq 0 && -e $STAGE/notice$sfx.png ]]; then
      mv -f "$STAGE/notice$sfx.png" "$OUT/notice$sfx.png"
    else
      rm -f "$STAGE/notice$sfx.png"
      echo "screenshots: no new notice$sfx.png (film status $rc: 3 is a private text on screen)" >&2
    fi
    wait
  else
    echo "screenshots: no agent at rest for the notice, none taken" >&2
  fi

  # A member zoomed, full screen: « ⤢ name » in the bar.
  at_work_again
  view first zoom
  rc=0
  film zoom "$TEAM_SIZE" "$sfx" <<EOF || rc=$?
Hide
Type "$attach"
Enter
Sleep 4s
Show
Sleep 500ms
Screenshot "{{out}}/zoom{{sfx}}.png"
EOF
  if [[ $rc -eq 0 && -e $STAGE/zoom$sfx.png ]]; then
    mv -f "$STAGE/zoom$sfx.png" "$OUT/zoom$sfx.png"
  else
    rm -f "$STAGE/zoom$sfx.png"
    echo "screenshots: no new zoom$sfx.png (film status $rc: 3 is a private text on screen)" >&2
  fi
  wait

  FILM_WATCH=0
  $keep || stop_team
}

# The animation from its recording: the start as it was (the request typed, `typed` characters, and sent), the rest
# sped up to fit about ANIM_LENGTH seconds, the last image held, the end word cut off (its last 2.5 s). Into
# <video>.mp4 (H.264) and <video>.webm (VP9), VIDEO_WIDTH wide, and a GIF GIF_WIDTH wide.
montage() { # <recording> <typed> <video> <gif>
  local length start end fast
  length=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$1")
  read -r start end fast < <(awk -v l="$length" -v n="$2" -v a="$ANIM_LENGTH" 'BEGIN {
    s = 1.5 + n * 0.03 + 0.7 + 2; e = l - 2.5; r = (e - s) / (a - s - 2); if (r < 1) r = 1
    printf "%.2f %.2f %.3f\n", s, e, r }')
  local cut="[0:v]trim=0:$start,setpts=PTS-STARTPTS[a];[0:v]trim=$start:$end,setpts=(PTS-STARTPTS)/$fast[b];"
  cut+="[a][b]concat=n=2:v=1,tpad=stop_mode=clone:stop_duration=2"
  ffmpeg -loglevel error -y -i "$1" -filter_complex "$cut,scale=$VIDEO_WIDTH:-2:flags=lanczos,fps=25" \
    -c:v libx264 -preset slow -crf 26 -pix_fmt yuv420p -movflags +faststart -an "$3.mp4"
  ffmpeg -loglevel error -y -i "$3.mp4" -c:v libvpx-vp9 -b:v 0 -crf 40 -row-mt 1 -an "$3.webm"
  ffmpeg -loglevel error -y -i "$3.mp4" -vf "fps=10,scale=$GIF_WIDTH:-1:flags=lanczos,split[a][b];\
[a]palettegen=max_colors=64:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" "$4"
}

# The animation: the request typed to the first contact, the work handed out, the cards at work and the messages in
# the journal, until two working agents are done. Filmed whole, then the wait sped up to fit about ANIM_LENGTH seconds.
shoot_anim() { # <lang> <sfx>
  local lang=$1 sfx=$2
  launch "$lang"
  local lead request
  IFS=$'\t' read -r lead request < <(tasks "$lang" | head -1)

  # Claude Code's welcome, which names the account's plan, out of the contacts' panes: each does a first task before
  # the recording, its own of the tasks file, or reading the project for the first one. (`!` and a command, typed by
  # send-keys, reaches Claude as a message all the same.)
  local contact first_task
  first_task=$([[ $lang == fr ]] && echo "Lis README.md et TODO.md, puis attends ma demande." ||
    echo "Read README.md and TODO.md, then wait for my request.")
  for contact in $(jq -r '.members[] | select(.contact) | .name' "$state/team.json"); do
    if [[ $contact == "$lead" ]]; then
      send "$contact" "$first_task"
    else
      send "$contact" "$(tasks "$lang" | awk -F '\t' -v m="$contact" '$1 == m { print $2 }' | grep . || echo "$first_task")"
    fi
  done
  select_first
  sleep 5
  rested() { (($(count working) == 0)); }
  wait_for 180 "the contacts at rest" rested || true
  sleep 4
  if private; then
    echo "screenshots: WARNING: no animation in $lang: the account's plan still shows" >&2
    $keep || stop_team
    return 0
  fi

  # Done once two working agents went from work to rest, or after 4 minutes: a word in the status line then tells
  # vhs to stop, and it is cut out of the video.
  local done_word="recruit-shots-end-$$"
  (
    deadline=$(($(date +%s) + 240))
    until has_client; do sleep 0.5; done
    # The working agents seen at work, one a line, and those of them at rest now.
    worked=$tmp/worked
    : >"$worked"
    while (($(date +%s) < deadline)); do
      jq -r --slurpfile team "$state/team.json" '
        ($team[0].members | map(select(.contact) | .name)) as $contacts
        | .members | to_entries[] | select(.key | IN($contacts[]) | not) | "\(.value.state) \(.key)"' \
        "$states" 2>/dev/null >"$tmp/now" || true
      sed -n 's/^working //p' "$tmp/now" >>"$worked"
      finished=$(sed -n 's/^idle //p' "$tmp/now" | grep -cxFf <(sort -u "$worked") || true)
      ((finished >= 2)) && break
      sleep 1
    done
    sleep 4
    say_on_client "$done_word" "$lead"
  ) &

  local attach
  attach=$(attach_command)
  view first
  # The request goes to the focused pane: the first contact's, the lead, when a client attaches.
  local rc=0
  FILM_WATCH=1
  film anim "$TEAM_SIZE" "$sfx" <<EOF || rc=$?
Output "$tmp/anim.mp4"
Set Framerate 10
Hide
Type "$attach"
Enter
Sleep 3s
Show
Sleep 1500ms
Type@30ms "$request"
Sleep 700ms
Enter
Wait+Screen@300s /$done_word/
EOF
  FILM_WATCH=0
  wait
  if [[ $rc -ne 0 ]]; then
    [[ $rc -eq 3 ]] || die "vhs failed on the animation"
    echo "screenshots: WARNING: no new animation: the account's plan or a home folder showed while it was filmed" >&2
    $keep || stop_team
    return 0
  fi

  montage "$tmp/anim.mp4" "${#request}" "$MEDIA/demo$sfx" "$OUT/demo$sfx.gif"

  $keep || stop_team
}

# The command line tapes of scripts/demo/tapes, filmed in the copy of the demo's project (also $DEMO_PROJECT), with
# RECRUIT_LANG set. A `# size: <cols>x<rows>` line sets their size, a `# lang: <en|fr>` line keeps them to a language.
shoot_cli() { # <lang> <sfx>
  local file size only_lang
  for file in "$DEMO"/tapes/*.tape; do
    [[ -e $file ]] || return 0
    only_lang=$(sed -n 's/^# lang: *//p' "$file" | head -1)
    [[ -z $only_lang || $only_lang == "$1" ]] || continue
    size=$(sed -n 's/^# size: *//p' "$file" | head -1)
    # A tape that fails (one that waits on Claude, say) leaves its screenshot out, not the others.
    RECRUIT_LANG=$1 film "$(basename "$file" .tape)" "${size:-$CLI_SIZE}" "$2" <"$file" ||
      echo "screenshots: vhs failed on $file" >&2
    # What the tape took, from the staging folder to the site.
    find "$STAGE" -maxdepth 1 -name '*.png' -exec mv -f {} "$OUT/" \;
  done
}

for lang in "${langs[@]}"; do
  sfx=""
  [[ $lang == fr ]] && sfx=-fr
  project=$tmp/$lang/$(jq -r '.name // "demo"' "$DEMO/project/package.json")
  mkdir -p "$(dirname "$project")"
  cp -R "$DEMO/project" "$project"
  # Project settings, over the user's: the members answer in the demo's language, read none of the user's own
  # instructions (paths and names would show), leave no memory behind, and read the project and run the tests without
# asking (`bun outdated`, a task's, still asks: someone waits). The status
  # line is an empty one.
  language=English
  [[ $lang == fr ]] && language=French
  mkdir -p "$project/.claude"
  jq -n --arg language "$language" --arg profile "$profile" '{
    language: $language,
    claudeMdExcludes: [($profile + "/CLAUDE.md"), ($profile + "/rules/**")],
    autoMemoryEnabled: false,
    autoDreamEnabled: false,
    permissions: {allow: [
      "Bash(bun test)", "Bash(bun test:*)", "Bash(ls:*)", "Bash(cat:*)", "Bash(head:*)", "Bash(tail:*)",
      "Bash(grep:*)", "Bash(find:*)", "Bash(wc:*)", "Bash(git status:*)", "Bash(git diff:*)", "Bash(git log:*)"
    ]},
    statusLine: {type: "command", command: "true"}
  }' >"$project/.claude/settings.json"
  git -C "$project" init -q
  git -C "$project" add -A
  git -C "$project" -c user.name=demo -c user.email=demo@example.com commit -qm "First version"
  [[ $only == all || $only == team ]] && shoot_team "$lang" "$sfx"
  [[ $only == all || $only == anim ]] && shoot_anim "$lang" "$sfx"
  [[ $only == all || $only == cli ]] && DEMO_PROJECT=$project shoot_cli "$lang" "$sfx"
done

echo "screenshots: done in $((($(date +%s) - started) / 60)) min, in $OUT and $MEDIA"
