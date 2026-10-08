# The guided creation, from a bare `recruit` in a project without a team to "Launch the team now?": a web app,
# medium, a second contact, acceptEdits, saved in the project.
# A home of its own, so that the paths show as ~/shorty/… and ~/.config/recruit/…, and no team anywhere. Its real
# path (pwd -P): on macOS /tmp is /private/tmp, and recruit sees the project's real path.
# size: 120x26

Hide
Type `mkdir -p "$XDG_CACHE_HOME" && export HOME="$(cd "$(mktemp -d "$XDG_CACHE_HOME/home.XXXXXX")" && pwd -P)"`
Enter
Type `export XDG_CONFIG_HOME="$HOME/.config" && cp -R . "$HOME/shorty" && cd "$HOME/shorty" && rm -rf .recruit && clear`
Enter
Sleep 500ms
Show

Type "recruit"
Sleep 300ms
Enter
Sleep 1s
# Guided
Enter
Sleep 500ms
# Web app
Down
Sleep 300ms
Enter
Sleep 500ms
# Medium
Down
Sleep 300ms
Enter
Sleep 500ms
# Keep this team
Enter
Sleep 500ms
# The designer as a second contact
Down 4
Sleep 300ms
Space
Sleep 300ms
Enter
Sleep 500ms
# acceptEdits
Down
Sleep 300ms
Enter
Sleep 500ms
# The name the project's folder gives: shorty
Enter
Sleep 500ms
# In this project
Enter
Sleep 1s
Screenshot "{{out}}/wizard{{sfx}}.png"
