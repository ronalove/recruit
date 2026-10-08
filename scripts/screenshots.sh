#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (C) 2026 Ronan Lamour
# Screenshots for the documentation site and the README: a demo team, with real Claude sessions, in a test tmux server,
# filmed by vhs.
#
#   scripts/screenshots.sh                 everything, in English then in French
#   scripts/screenshots.sh --lang en       one language (en, fr)
#   scripts/screenshots.sh --only team     only the team's screenshots; `cli`, the tapes of scripts/demo/tapes (no
#                                          team); `anim`, the animation
#   scripts/screenshots.sh --keep          leave the demo team running at the end, to look at it (tmux -L rtest-shots)
#
# Writes into site/src/assets/screenshots/: team.png, dashboard.png, journal.png, menu.png, agents.png, those of
# scripts/demo/tapes/*.tape and demo.gif (for the README); into site/public/media/: demo.mp4 and demo.webm. With a -fr
# suffix in French. The screenshots are rendered at twice the size (Retina): show them at half.
#
# The demo (scripts/demo/project, its team in .recruit/settings.toml, the tasks in scripts/demo/tasks.<lang>.tsv) runs
# in a copy under /tmp/recruit-shots, with XDG_CONFIG_HOME and XDG_CACHE_HOME of its own, in the tmux server named by
# its team file (rtest-shots), never recruit's. Its members use the user's Claude Code profile, on light models: a few
# minutes of sonnet and haiku per language. The profile's settings.json is checked to be the same at the end, and the
# demo's conversations are removed from it. An image that shows the account's plan or a home folder is not kept.
#
# Needs vhs (brew install vhs), ffmpeg, jq, tmux and claude.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
DEMO=$ROOT/scripts/demo
OUT=$ROOT/site/src/assets/screenshots
MEDIA=$ROOT/site/public/media
SOCKET=rtest-shots

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
    -h | --help) sed -n '4,23s/^# \{0,1\}//p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done
for lang in "${langs[@]}"; do [[ $lang == en || $lang == fr ]] || die "unknown language: $lang"; done
[[ $only =~ ^(all|team|cli|anim)$ ]] || die "--only takes team, cli or anim"

for tool in vhs ffmpeg tmux claude jq; do command -v "$tool" >/dev/null || die "$tool not found"; done
grep -q "^socket = \"$SOCKET\"" "$DEMO/project/.recruit/settings.toml" || die "the demo team must run in tmux -L $SOCKET"

profile=${CLAUDE_CONFIG_DIR:-$HOME/.claude}
settings_sum() { shasum "$profile/settings.json" 2>/dev/null || echo none; }
before=$(settings_sum)

(cd "$ROOT" && cargo build --quiet)
mkdir -p "$OUT" "$MEDIA"
# Always the same folder: Claude Code asks once whether to trust it, and the screenshots show its name.
tmp=/tmp/recruit-shots
[[ ! -e $tmp || -e $tmp/.recruit-shots ]] || die "$tmp exists and is not this script's"
running=$(cat "$tmp/.recruit-shots" 2>/dev/null || true)
[[ -n $running ]] && kill -0 "$running" 2>/dev/null && die "already running (pid $running)"
rm -rf "$tmp"
mkdir -p "$tmp"
echo $$ >"$tmp/.recruit-shots"
export XDG_CONFIG_HOME=$tmp/config XDG_CACHE_HOME=$tmp/cache
export PATH=$ROOT/target/debug:$PATH
# The dashboard leaves out the usage of the account, the user's and not the demo's.
export RECRUIT_NO_USAGE=1
started=$(date +%s)

cleanup() {
  if ! $keep; then
    tmux -L "$SOCKET" kill-server 2>/dev/null || true
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

t() { tmux -L "$SOCKET" "$@"; }

# A tape for vhs: the shared settings, the size, then the steps (stdin), and a second at the end: vhs could leave
# before writing a screenshot that ends a tape. `{{out}}` and `{{sfx}}` are replaced.
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
    sed -e "s#{{out}}#$OUT#g" -e "s#{{sfx}}#$3#g"
    echo 'Sleep 1s'
  } >"$1"
}
PADDING=24
BAR=56

# Films a tape from its steps (stdin), in the demo's project.
film() { # <name> <size> <sfx>
  tape "$tmp/$1.tape" "$2" "$3"
  (cd "$project" && vhs "$tmp/$1.tape" >/dev/null 2>&1)
}

# Cuts a pane out of a screenshot of its whole tab, with its title row above it, and frames it with the padding of the
# terminal. The cells are 17.144 × 33.49 pixels at 28 px, measured on a screenshot (vhs 0.12); a few pixels less at the
# bottom keep the next row out.
crop() { # <screenshot> <out> <pane>
  local x y w h
  read -r x y w h < <(t display-message -p -t "$3" '#{pane_left} #{pane_top} #{pane_width} #{pane_height}' |
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

# A pane by its member's name, or a panel's role (dashboard, journal).
pane_of() {
  t list-panes -s -t "=$session" -F '#{pane_id} #{?@recruit_role,#{@recruit_role},#{@recruit_member}}' |
    awk -v m="$1" '$2 == m { print $1 }'
}

# The members' panes.
member_panes() { t list-panes -s -t "=$session" -F '#{pane_id} #{@recruit_role}' | awk 'NF == 1 { print $1 }'; }

# Claude Code asks whether to trust a folder it has never been approved in, "No, exit" first: yes.
trust() {
  local pane
  for pane in $(member_panes); do
    if t capture-pane -p -t "$pane" | grep -q "trust this folder"; then
      t send-keys -t "$pane" Down
      t send-keys -t "$pane" Enter
    fi
  done
}

# Whether the tab on screen shows what a public image must not: the account's plan (in Claude Code's welcome, on top
# of a conversation that has not said much yet), its usage (Claude Code's warning near a limit), or a home folder.
PRIVATE="Claude (Max|Pro|Team|Enterprise|API)|% of your [a-z0-9 -]*limit|/Users/|/home/|$HOME"
private() {
  local pane
  for pane in $(t list-panes -t "=$session:" -F '#{pane_id}'); do
    t capture-pane -p -t "$pane" | grep -qE "$PRIVATE" && return 0
  done
  return 1
}

# Films a screenshot of the tab on screen once nothing private shows, before and after: four tries, 15 s apart; an
# image that still shows something is removed.
film_clean() { # <name> <size> <sfx> <image>  (steps on stdin)
  local steps try
  steps=$(cat)
  for try in 1 2 3 4; do
    if ! private; then
      film "$1" "$2" "$3" <<<"$steps" || die "vhs failed on $1"
      private || return 0
    fi
    echo "screenshots: $1 would show the account's plan or a home folder, again in 15 s ($try)" >&2
    sleep 15
  done
  rm -f "$4"
  echo "screenshots: WARNING: no $4: it showed the account's plan or a home folder" >&2
  return 1
}

# The team launched in the demo's project, its members up and its folder trusted. Sets `session`, `state`, `states`.
launch() { # <lang>
  local team=demo members
  [[ $1 == fr ]] && team=demo-fr
  session=$team
  (cd "$project" && RECRUIT_LANG=$1 recruit --detach "$team")
  state=$(t show-options -v -t "=$session:" @recruit_state)
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
  t send-keys -t "$pane" -l "$2"
  sleep 0.5
  t send-keys -t "$pane" Enter
}

# The tasks of scripts/demo/tasks.<lang>.tsv: `member<TAB>message`, one a line; `#` starts a comment.
tasks() { grep -v '^#' "$DEMO/tasks.$1.tsv" | grep .; }

# The team at work: launched, given its tasks, and filmed once its members are working, idle and waiting.
shoot_team() { # <lang> <sfx>
  local lang=$1 sfx=$2
  launch "$lang"
  local member message
  while IFS=$'\t' read -r member message; do send "$member" "$message"; done < <(tasks "$lang")

  # Someone at work and someone waiting, once the first messages went round.
  local given
  given=$(date +%s)
  scene() { (($(count working) >= 1 && $(count waiting) >= 1 && $(date +%s) - given >= 25)); }
  local attach="tmux -L $SOCKET attach -t =$session"
  local first try
  first=$(t list-windows -t "=$session" -F '#{window_index}' | head -1)

  # The first tab: the contacts, the dashboard and the reduced journal. Filmed again, three times at most, when the
  # scene changed meanwhile; not when it never came.
  t select-window -t "=$session:$first"
  for try in 1 2 3; do
    local came=true
    wait_for 240 "working and waiting members at once" scene || came=false
    film_clean team "$TEAM_SIZE" "$sfx" "$OUT/team$sfx.png" <<EOF || break
Hide
Type "$attach"
Enter
Sleep 2s
Show
Sleep 500ms
Screenshot "{{out}}/team{{sfx}}.png"
EOF
    if ! $came || scene; then break; fi
    echo "screenshots: the scene changed while filming, again ($try)" >&2
  done
  # The dashboard's close-up, out of the same image.
  [[ -e $OUT/team$sfx.png ]] && crop "$OUT/team$sfx.png" "$OUT/dashboard$sfx.png" "$(pane_of dashboard)"

  # The /recruit menu, on the sheet of the second member. Opened as Alt+r does, once vhs's terminal is attached: Alt+r
  # typed by vhs reaches tmux as a plain r.
  (
    client=""
    until [[ -n $client ]]; do
      sleep 0.5
      client=$(t list-clients -t "=$session" -F '#{client_name}' | head -1)
    done
    sleep 1
    recruit --lang "$lang" _menu "$state" --client "$client" --popup >/dev/null 2>&1
  ) &
  film_clean menu "$TEAM_SIZE" "$sfx" "$OUT/menu$sfx.png" <<EOF || true
Hide
Type "$attach"
Enter
Sleep 5s
Down
Sleep 1s
Show
Sleep 500ms
Screenshot "{{out}}/menu{{sfx}}.png"
Escape
EOF
  wait

  # A tab of working agents, in a grid.
  local agents
  agents=$(t list-windows -t "=$session" -F '#{window_index}' | sed -n 2p)
  if [[ -n $agents ]]; then
    t select-window -t "=$session:$agents"
    film_clean agents "$TEAM_SIZE" "$sfx" "$OUT/agents$sfx.png" <<EOF || true
Hide
Type "$attach"
Enter
Sleep 4s
Show
Sleep 500ms
Screenshot "{{out}}/agents{{sfx}}.png"
EOF
    t select-window -t "=$session:$first"
  fi

  # The journal in full (reduced at launch: hidden, then full), cut out of the first tab.
  recruit _panel toggle "$state" >/dev/null
  recruit _panel toggle "$state" >/dev/null
  film_clean full "$TEAM_SIZE" "$sfx" "$tmp/full.png" <<EOF &&
Hide
Type "$attach"
Enter
Sleep 3s
Show
Sleep 500ms
Screenshot "$tmp/full.png"
EOF
    crop "$tmp/full.png" "$OUT/journal$sfx.png" "$(pane_of journal)"

  $keep || t kill-session -t "=$session"
}

# The animation: the request typed to the first contact, the work handed out, the cards at work and the messages in
# the journal, until two working agents are done. Filmed whole, then the wait sped up to fit about ANIM_LENGTH seconds.
shoot_anim() { # <lang> <sfx>
  local lang=$1 sfx=$2
  launch "$lang"
  local lead request
  IFS=$'\t' read -r lead request < <(tasks "$lang" | head -1)

  # Claude Code's welcome, which names the account's plan, out of the contacts' panes: the project's files shown
  # there, as the user would before asking (`!` runs a command without Claude).
  local contact file
  for contact in $(jq -r '.members[] | select(.contact) | .name' "$state/team.json"); do
    for file in README.md TODO.md; do
      private || break
      # `!` on its own, as a key: typed with the rest at once, it reaches Claude as a message.
      t send-keys -t "$(pane_of "$contact")" '!'
      sleep 0.5
      send "$contact" "cat $file"
      sleep 2
    done
  done
  if private; then
    echo "screenshots: WARNING: no animation in $lang: the account's plan still shows" >&2
    $keep || t kill-session -t "=$session"
    return 0
  fi

  # Done once two working agents went from work to rest, or after 4 minutes: a word in the status line then tells
  # vhs to stop, and it is cut out of the video.
  local done_word="recruit-shots-end-$$"
  (
    deadline=$(($(date +%s) + 240))
    client=""
    until [[ -n $client ]]; do
      sleep 0.5
      client=$(t list-clients -t "=$session" -F '#{client_name}' | head -1)
    done
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
    t display-message -c "$client" -d 10000 "$done_word"
  ) &

  local attach="tmux -L $SOCKET attach -t =$session"
  local first
  first=$(t list-windows -t "=$session" -F '#{window_index}' | head -1)
  t select-window -t "=$session:$first"
  t select-pane -t "$(pane_of "$lead")"
  film anim "$TEAM_SIZE" "$sfx" <<EOF || die "vhs failed on the animation"
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
  wait

  # The start as it was (the request typed and sent), the rest sped up, the last image held, the end word cut off.
  local length start fast
  length=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$tmp/anim.mp4")
  start=$(awk -v n="${#request}" 'BEGIN { printf "%.2f", 1.5 + n * 0.03 + 0.7 + 2 }')
  fast=$(awk -v l="$length" -v s="$start" -v a="$ANIM_LENGTH" \
    'BEGIN { r = (l - 2.5 - s) / (a - s - 2); printf "%.3f", r < 1 ? 1 : r }')
  local cut="[0:v]trim=0:$start,setpts=PTS-STARTPTS[a];[0:v]trim=$start:$length-2.5,setpts=(PTS-STARTPTS)/$fast[b];"
  cut+="[a][b]concat=n=2:v=1,tpad=stop_mode=clone:stop_duration=2"
  ffmpeg -loglevel error -y -i "$tmp/anim.mp4" -filter_complex "$cut,scale=$VIDEO_WIDTH:-2:flags=lanczos,fps=25" \
    -c:v libx264 -preset slow -crf 26 -pix_fmt yuv420p -movflags +faststart -an "$MEDIA/demo$sfx.mp4"
  ffmpeg -loglevel error -y -i "$MEDIA/demo$sfx.mp4" -c:v libvpx-vp9 -b:v 0 -crf 40 -row-mt 1 -an "$MEDIA/demo$sfx.webm"
  ffmpeg -loglevel error -y -i "$MEDIA/demo$sfx.mp4" -vf "fps=10,scale=$GIF_WIDTH:-1:flags=lanczos,split[a][b];\
[a]palettegen=max_colors=64:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" "$OUT/demo$sfx.gif"

  $keep || t kill-session -t "=$session"
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
  done
}

for lang in "${langs[@]}"; do
  sfx=""
  [[ $lang == fr ]] && sfx=-fr
  project=$tmp/$lang/$(jq -r '.name // "demo"' "$DEMO/project/package.json")
  mkdir -p "$(dirname "$project")"
  cp -R "$DEMO/project" "$project"
  # Project settings, over the user's: the members answer in the demo's language, read none of the user's own
  # instructions (paths and names would show), leave no memory behind, and run the tests without asking. The status
  # line is an empty one.
  language=English
  [[ $lang == fr ]] && language=French
  mkdir -p "$project/.claude"
  jq -n --arg language "$language" --arg profile "$profile" '{
    language: $language,
    claudeMdExcludes: [($profile + "/CLAUDE.md"), ($profile + "/rules/**")],
    autoMemoryEnabled: false,
    autoDreamEnabled: false,
    permissions: {allow: ["Bash(bun test)", "Bash(bun test:*)"]},
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
