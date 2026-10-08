# "Claude builds the team", in the guided creation: Claude reads the project and proposes a team (one real
# `claude -p`, sonnet, about a minute). Filmed on "Does it suit you?", the team shown.
# The description is typed, so this tape is English only; describe-fr.tape is the French one.
# size: 120x26
# lang: en

Hide
Type `mkdir -p "$XDG_CACHE_HOME" && D="$(mktemp -d "$XDG_CACHE_HOME/describe.XXXXXX")"`
Enter
Type `cp -R . "$D/shorty" && cd "$D/shorty" && rm -rf .recruit && clear`
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
# Other: I describe my project, Claude builds the team
Down 8
Sleep 300ms
Enter
Sleep 500ms
# Let Claude decide
Down 3
Sleep 300ms
Enter
Sleep 500ms
Type "A link shortener on Bun. Next up: expiring links, click stats, tests, docs."
Sleep 500ms
Enter
Wait+Screen@240s /Does it suit you/
Sleep 1s
Screenshot "{{out}}/describe{{sfx}}.png"
