# describe.tape in French: the same steps, a description in French.
# size: 120x26
# lang: fr

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
Type "Raccourcisseur de liens sur Bun. Ensuite : expiration, clics, tests et doc."
Sleep 500ms
Enter
Wait+Screen@240s /Elle te convient/
Sleep 1s
Screenshot "{{out}}/describe{{sfx}}.png"
