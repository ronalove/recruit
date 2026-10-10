---
title: Avec Claude Code
description: Ce que reçoit chaque membre, le mod de recruit, le bas de Claude Code, les profils, et les membres qui s'arrêtent.
sidebar:
  order: 6
---

Chaque membre est le Claude Code que tu connais déjà : il lit ton code, lance des commandes et modifie des fichiers de la même façon. Ce qu'ajoute recruit, c'est l'équipe autour. Chaque membre connaît son rôle, ses coéquipiers et à qui il rend compte.

Cette page explique ce que reçoit chaque membre, et comment recruit s'accorde avec ta configuration de Claude Code. recruit ne choisit que la façon dont chaque session démarre, et ajoute un petit mod quand Claude Code peut le charger.

## Ce que reçoit chaque membre

Son nom, donné par `claude -n`, et un prompt système ajouté (`--append-system-prompt-file`) qui contient :

- son rôle et ses instructions ;
- s'il est interlocuteur ou agent de travail. Un agent de travail pose ses questions aux interlocuteurs quand il est bloqué, plutôt que d'attendre dans son terminal, et fait ce que tu demandes quand tu interviens ;
- la liste de ses coéquipiers, avec leurs rôles et les interlocuteurs signalés ;
- quand une autre session porte le nom d'un coéquipier, l'adresse exacte où lui écrire, « nom [ref] » ;
- les règles communes de l'équipe.

recruit écrit ce prompt dans la langue de l'équipe (`lang`). Les fichiers de prompt sont dans `~/.cache/recruit/prompts/`. Pour voir la ligne de commande et le fichier de prompt de chaque membre sans rien lancer :

```sh
recruit --print
```

Le modèle, l'effort, le mode de permission et les arguments en plus viennent du fichier de l'équipe : voir [Configuration](/recruit/fr/reference/configuration/#équipes).

## Le mod de recruit

Avec Claude Code 2.1.287 ou plus récent, chaque membre charge aussi un petit mod de recruit, par `claude --plugin-dir`. recruit l'écrit dans `~/.cache/recruit/mod/`. Le mod ne décide rien : il rappelle recruit (`recruit _mod`), qui fait le travail. Grâce à lui :

- le [tableau de bord](/recruit/fr/guides/dashboard/) montre le modèle et l'effort qu'utilise vraiment chaque membre, son contexte, une ligne qui résume ce qu'il fait, et l'usage de ton compte sur 5 heures et sur 7 jours ;
- `/equipe` (`/team` dans une équipe en anglais), tapé dans l'invite d'un membre, fait passer le journal à sa taille suivante, comme `Alt+j`, même quand Claude travaille ;
- `/recruit`, tapé dans l'invite d'un membre, ouvre le [menu de l'équipe](/recruit/fr/guides/menu/), comme `Alt+r` ;
- ce qu'on change dans le menu arrive aux membres sans relance, quand c'est possible : un modèle ou un effort dès la requête suivante du membre, un rôle, des instructions ou des coéquipiers dans une note jointe à son prochain message ;
- chaque membre garde une vue à jour de l'équipe : quand son prompt a changé depuis ce que sa conversation en a reçu en dernier (un membre ajouté, un rôle réécrit), le nouveau lui arrive une fois, avec son prochain message. Après une compaction, il revient s'il diffère de celui du début de la conversation. Un message qui commence par `/` ne le porte jamais, et un message qui l'attendrait plus de 3 secondes part sans lui : le suivant le portera.

Le mod ne change rien à tes réglages de Claude Code. Là où une organisation n'autorise pas les mods installés par ses utilisateurs, l'équipe marche de la même façon, sans tout cela.

## Le bas de Claude Code

Seul l'interlocuteur principal, le premier interlocuteur, le garde en entier. Les autres membres :

- n'ont pas ta ligne d'état : le `statusLine` de tes réglages est retiré pour leur session seulement, par `--settings`, sauf si les `args` de l'équipe ou du membre donnent déjà un `--settings`. En plein écran, Claude Code réserve une ligne à la ligne d'état même quand elle n'affiche rien : ces membres ont donc là une ligne vide ; sans ligne d'état dans tes réglages, rien ne change ;
- avec le mod, n'ont pas non plus la ligne d'aide sous l'invite (`? for shortcuts`, `esc to interrupt`), ni les libellés de mode à sa droite.

La saisie et la ligne du mode de permission restent : Claude Code ne permet pas de les masquer.

## Profil Claude Code

Si tu utilises plusieurs comptes Claude Code, chacun avec son dossier de configuration (`CLAUDE_CONFIG_DIR=~/.claude-perso claude`), `[claude] config_dir` choisit celui de l'équipe :

```toml title=".recruit/settings.local.toml"
[claude]
config_dir = "~/.claude-perso"
```

C'est un chemin personnel : pour une équipe locale, mets-le dans `.recruit/settings.local.toml`, pas dans le fichier commité. Il doit être absolu ou commencer par `~/`.

Les membres démarrent avec `CLAUDE_CONFIG_DIR`, tout comme le shell qui reste dans leur panneau. recruit y cherche aussi les sessions ouvertes, les dossiers approuvés et les conversations que reprend `--resume`. Le dossier doit exister : lance `claude` une fois avec ce profil pour le créer et te connecter.

## Un membre qui s'arrête

Quand le Claude d'un membre s'arrête de lui-même (`/exit`, un plantage), son panneau le relance deux secondes plus tard sur la même conversation : il garde son nom, son rôle et son historique, et prend ses réglages tels que les donnent alors les fichiers de l'équipe.

- `Ctrl-C` pendant ces deux secondes laisse un shell à la place.
- Un membre dont le Claude s'arrête deux fois de suite juste après son démarrage n'est plus relancé : son panneau le dit. Une fois la cause corrigée, `recruit <équipe> --restart --resume` relance l'équipe.
- Un membre retiré de l'équipe pendant ces deux secondes n'est pas relancé : son panneau dit qu'il ne fait plus partie de l'équipe.
- Rien n'est relancé une fois l'équipe arrêtée.

Relancer `recruit` sur une équipe qui tourne remet en route les membres arrêtés et rouvre un tableau de bord fermé. Si des panneaux ont été fermés, recruit propose de reconstruire l'équipe, chaque membre reprenant sa conversation.

## Reprendre les conversations

```sh
recruit --resume
```

Chaque membre reprend la dernière conversation qui porte son nom dans ce dossier. Un membre qui n'en a pas en commence une nouvelle, et recruit le signale. Sur une équipe qui tourne déjà, `--resume` est ignoré : `recruit --restart --resume` la reconstruit sur ses conversations.

Sans le mod, une conversation reprise garde le prompt de son début : après avoir changé les rôles, lance des sessions neuves plutôt que `--resume`. Avec le mod, le membre apprend son rôle et son équipe actuels avec son prochain message, s'ils ont changé.
