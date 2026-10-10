---
title: Commandes
description: Chaque commande et chaque option de recruit.
sidebar:
  order: 1
---

Chaque commande prend `--lang fr` ou `--lang en`, pour la langue de ses messages (voir [Langue](/recruit/fr/faq/#langue)), et `-h`, `--help`.

## `recruit`

```
recruit [OPTIONS] [ÉQUIPE]
```

Lance une équipe, ou en crée une.

| Tu tapes | recruit… |
|---|---|
| `recruit` dans un projet qui a une équipe | la lance : la seule, ou celle que nomme `default` ; sinon, il demande laquelle |
| `recruit` ailleurs, avec des équipes globales | les propose, et n'en lance jamais une sans demander ; il peut aussi créer une nouvelle équipe |
| `recruit` sans aucune équipe | en crée une en mode interactif |
| `recruit <équipe>` | lance cette équipe : la locale si une équipe locale et une globale portent ce nom, sinon la globale |
| `recruit <équipe>`, inconnue | la crée en mode interactif, sous ce nom |

Si l'équipe tourne déjà, `recruit` la rejoint. Il remet aussi en route les membres arrêtés, rouvre un tableau de bord fermé et, si des panneaux ont été fermés, propose de reconstruire l'équipe.

| Option | Effet |
|---|---|
| `-r`, `--resume` | Chaque membre reprend la dernière conversation qui porte son nom dans ce dossier. Ignoré si l'équipe tourne. |
| `--dry-run` | Ouvre la disposition seule, en essai à part (`<équipe>-dry-run`) : chaque panneau affiche son membre, son rôle et la commande qu'il lancerait, sans lancer Claude. `recruit stop <équipe>` ferme l'essai. |
| `-d`, `--detach` | Lance sans s'attacher à l'équipe. |
| `--restart` | Arrête d'abord l'équipe si elle tourne. |
| `--print` | Affiche les onglets, la ligne de commande de chaque membre et son fichier de prompt, sans rien lancer. |
| `--lang <LANG>` | Langue de l'interface : `fr` ou `en`. |
| `-V`, `--version` | Affiche la version de recruit. |

Un nom d'équipe ou une option de lancement ne va pas avec une sous-commande : `recruit web --resume`, pas `recruit attach web --resume`.

## `recruit new`

```
recruit new [OPTIONS] [ÉQUIPE]
```

Crée une équipe. Sans `--template`, `--describe` ni `--member`, il pose ses questions, le nom déjà rempli s'il est donné (voir [Composer une équipe](/recruit/fr/guides/teams/#composer-une-équipe)). Avec l'une d'elles, il ne demande rien et il lui faut le nom.

| Option | Effet |
|---|---|
| `-g`, `--global` | Enregistre l'équipe dans `~/.config/recruit/<nom>.toml` plutôt que dans `.recruit/settings.toml` du projet. |
| `-t`, `--template <TEMPLATE>` | Une [équipe intégrée](/recruit/fr/reference/templates/) : `personal`, `web`, `mobile`, `api`, `library`, `data`, `game` ou `infra`. |
| `-s`, `--size <SIZE>` | Taille de l'équipe pour `--template` ou `--describe` : `small` (3 membres), `medium` (5) ou `large` (8). `small` par défaut avec `--template` ; Claude décide avec `--describe`. |
| `--describe <DESCRIPTION>` | Claude lit le projet et compose l'équipe d'après cette description. |
| `-m`, `--member <NOM:RÔLE>` | Ajoute un membre, sous la forme `"nom:rôle"`. Répétable. Avec `--template` ou `--describe`, ajoute un membre à l'équipe, ou donne ce rôle à l'un de ses membres. |
| `-c`, `--contact <NOM>` | Fait d'un membre l'un de tes interlocuteurs. Répétable, deux au plus. Par défaut : ceux du modèle, ceux que Claude a désignés avec `--describe`, ou le premier membre. |
| `--permission-mode <MODE>` | Mode de permission de Claude Code pour tous les membres : `acceptEdits`, `auto`, `bypassPermissions`… |
| `--model <MODÈLE>` | Modèle de Claude pour tous les membres : `opus`, `sonnet`… |
| `-f`, `--force` | Remplace une équipe du même nom. |
| `-l`, `--launch` | Lance l'équipe une fois créée. |

```sh
recruit new web --template web --size medium
recruit new web --describe "Boutique en Next.js avec une API en Go" --size small
recruit new web -m "pilote:Coordonne" -m "dev:Écrit le code" -c pilote
recruit new outils --global --template personal --launch
```

Une équipe créée avec `--member` seul reçoit les règles communes de recruit comme instructions, dans la langue de l'interface.

## `recruit list`

```
recruit list [--json]
```

Liste les équipes du projet et tes équipes globales, chacune avec son nombre de membres et son état : arrêtée, en cours, en cours et attachée, essai en cours (`--dry-run`), ou en cours sous recruit 1 (tmux), pour une équipe que recruit 1 a laissée tourner (voir [la FAQ](/recruit/fr/faq/#une-équipe-tourne-encore-sous-recruit-1)). Un `*` marque l'équipe `default` du projet. Les équipes en cours qui ne sont à aucune d'elles viennent en dernier.

```
Équipes de ce projet (~/code/mon-app/.recruit)
  * web    3 membres  en cours, attachée
    api    3 membres  arrêtée
Équipes globales (~/.config/recruit)
    outils    2 membres  arrêtée
```

`--json` affiche un objet par équipe : `name`, `scope` (`local` ou `global`), `file`, `members`, `running`, `attached`, `dir`, le dossier où elle tourne, et `tmux`, vrai pour une équipe qui tourne encore sous recruit 1.

## `recruit attach`

```
recruit attach [ÉQUIPE]
```

Rejoint une équipe qui tourne : celle qui est nommée, sinon l'équipe du projet, sinon la seule qui tourne. Si plusieurs tournent et qu'aucune de ces règles n'en désigne une, recruit demande laquelle ; sans terminal, il les liste et s'arrête. Un essai (`--dry-run`) est trouvé aussi, quand l'équipe elle-même ne tourne pas.

## `recruit stop`

```
recruit stop [-y] [ÉQUIPE]
```

Arrête une équipe qui tourne, choisie comme pour `attach` : la session Claude de chaque membre est fermée. `recruit <équipe> --resume` reprendra les conversations plus tard.

| Option | Effet |
|---|---|
| `-y`, `--yes` | Ne demande pas de confirmation. |

## `recruit edit`

```
recruit edit [--local] [ÉQUIPE]
```

Ouvre le fichier d'une équipe dans ton éditeur : `$VISUAL`, sinon `$EDITOR`, sinon `vi`. Sans nom, le `.recruit/settings.toml` du projet ; avec un nom, le fichier du projet si l'équipe est locale, sinon le fichier global qui la contient, sinon `~/.config/recruit/<équipe>.toml`. Il ouvre le fichier même quand les fichiers de l'équipe ne se lisent pas, pour les corriger, et dit à la fermeture de l'éditeur s'il reste une erreur.

| Option | Effet |
|---|---|
| `--local` | Ouvre ton fichier personnel, `.recruit/settings.local.toml`, créé au besoin. |

## `recruit templates`

```
recruit templates
```

Liste les types de projet et les tailles intégrés, et les membres de chacun : voir [Équipes intégrées](/recruit/fr/reference/templates/).

## Commandes internes

Les commandes qui commencent par `_` (`_member`, `_panel`, `_mod`, `_menu`…) sont ce que recruit lance dans les panneaux, le mod et le menu. Elles n'apparaissent pas dans l'aide, et ne sont pas faites pour être tapées.
