---
title: Dans tmux
description: Le serveur tmux de recruit, ses raccourcis, Option sous macOS, et comment se détacher et revenir.
sidebar:
  order: 5
---

Ton équipe continue de travailler quand tu pars. Ferme le terminal, reviens demain, depuis une autre machine si tu veux : chacun est là où tu l'as laissé. Cette page donne les quelques touches pour circuler, partir et revenir.

Sous le capot, une équipe tourne dans une session tmux à son nom, dans un serveur tmux propre à recruit, `tmux -L recruit`. Tes autres sessions tmux n'y sont pour rien.

## Ce que règle recruit

Le serveur tmux de recruit est réglé pour Claude Code : Shift+Entrée va à la ligne, les notifications remontent au terminal, la souris marche, les couleurs et le presse-papiers aussi, et chaque panneau affiche le nom de son membre sur sa bordure du haut. Les onglets sont numérotés à partir de 1. À droite de la barre tmux : la version de recruit et deux boutons, « menu » et « quitter ».

Ta propre configuration tmux est lue aussi, `~/.tmux.conf` puis `~/.config/tmux/tmux.conf`, s'ils existent. Elle vient après les réglages de base de recruit (numéros d'onglet, barre, raccourcis `Alt`), qu'elle peut donc changer. Ce dont Claude Code a besoin (souris, couleurs, presse-papiers, touches) est réglé après, pour tenir, et `[tmux] options` passe en tout dernier. `[tmux]` dans le fichier de l'équipe change cela, et le nom du serveur : voir [Configuration](/recruit/fr/reference/configuration/#tmux).

## Raccourcis

| Touche | Effet |
|---|---|
| `Alt+1`…`Alt+9` | aller à un onglet |
| `Alt+Maj+←` / `Alt+Maj+→` | onglet précédent / suivant |
| `Ctrl-b n` / `Ctrl-b p` | onglet suivant / précédent, sans Alt |
| `Ctrl-b z` | agrandir un panneau, puis revenir |
| `Ctrl-b d` | se détacher : l'équipe continue de tourner |
| `Alt+j` | journal complet, réduit, puis masqué (ou `/equipe`, `/team` dans une équipe en anglais, dans l'invite d'un membre) |
| `Alt+r`, ou le bouton « menu » | le [menu de l'équipe](/recruit/fr/guides/menu/) (ou `/recruit` dans l'invite d'un membre) |
| `Alt+q`, ou le bouton « quitter » | se détacher (l'équipe continue) ou arrêter l'équipe, en une fois |

Un clic sur un onglet ou un panneau marche aussi, tout comme un clic sur la carte d'un membre dans le tableau de bord. `Ctrl-b` est le préfixe de tmux, sauf si ta configuration le change.

## Option sous macOS

Sous macOS, Alt est la touche Option : recruit l'écrit d'ailleurs ⌥ dans le tableau de bord, le journal et la barre tmux (⌥j, ⌥q, ⌥r).

Le terminal doit envoyer Option comme Alt. Sinon, Option+lettre tape un caractère spécial ou accentué, selon la disposition du clavier, et tmux ne voit aucun raccourci.

| Terminal | Réglage |
|---|---|
| Ghostty | `macos-option-as-alt = left` (ou `right`, ou `true`) dans sa configuration. Ghostty ne le fait d'office que pour les dispositions américaines. |
| Terminal.app | Profils › Clavier › Utiliser « Option » comme touche Meta |
| iTerm2 | Profiles › Keys, Option gauche en « Esc+ » |

## Se détacher et revenir

Détache-toi avec `Alt+q` ou `Ctrl-b d`. L'équipe continue de travailler : fermer le terminal, même avec `Cmd+q`, ferme la fenêtre, pas l'équipe. Pour revenir, depuis n'importe quel terminal, même en SSH :

```sh
recruit            # dans le projet : rejoint l'équipe qui tourne
recruit attach     # l'équipe du projet, ou la seule qui tourne
recruit attach web # une équipe donnée
recruit list       # les équipes, et celles qui tournent
```

Relancer `recruit` sur une équipe qui tourne remet aussi en route les membres arrêtés et rouvre un tableau de bord fermé. Si des panneaux ont été fermés, il propose de reconstruire l'équipe, chaque membre reprenant sa conversation.

## Arrêter

`Alt+q` puis « Arrêter l'équipe », ou depuis un shell :

```sh
recruit stop       # demande d'abord
recruit stop -y    # ne demande pas
```

La session Claude de chaque membre est fermée. `recruit --resume` reprendra plus tard la dernière conversation de chaque membre.

## Réglages tmux

recruit lit `[tmux]` au démarrage de son serveur tmux, c'est-à-dire avec la première équipe lancée. Pour appliquer un changement, arrête le serveur, ce qui arrête toutes les équipes qui y tournent :

```sh
tmux -L recruit kill-server
```
