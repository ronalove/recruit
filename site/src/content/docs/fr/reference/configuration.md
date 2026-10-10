---
title: Configuration
description: Chaque clé du fichier d'une équipe, avec son type, sa valeur par défaut et son effet.
sidebar:
  order: 2
---

Le fichier d'une équipe est en TOML. Le même format sert partout :

| Fichier | Contient |
|---|---|
| `.recruit/settings.toml` | les équipes du projet, commitées |
| `.recruit/settings.local.toml` | tes réglages personnels pour le projet, fusionnés clé par clé avec `settings.toml`, non commités |
| `~/.config/recruit/<équipe>.toml` | une équipe globale (dans `$XDG_CONFIG_HOME/recruit/` si cette variable est définie) |

Voir [Équipes](/recruit/fr/guides/teams/) pour la façon dont ils se combinent.

## Un exemple complet

```toml title=".recruit/settings.toml"
default = "web"                   # équipe lancée par `recruit` seul quand le fichier en contient plusieurs

[claude]
command = "claude"                # exécutable de Claude Code
config_dir = "~/.claude-perso"    # profil Claude Code ; mieux dans settings.local.toml

[teams.web]
description = "Application web"
lang = "fr"
instructions = """
- Règles communes à tous les membres.
"""
permission_mode = "auto"
model = "opus"
effort = "high"
args = []
layout = "auto"
columns = 3
rows = 2
dashboard = true

[teams.web.members.coordinateur]
role = "Point d'entrée de l'utilisateur : répartit le travail, suit l'avancement"
contact = true
instructions = """
- Ce que fait ce membre, sa zone, comment il rend son travail.
"""

[teams.web.members.dev-back]
role = "Serveur et API"
tab = "Code"
model = "sonnet"
```

## Premier niveau

| Clé | Type | Défaut | Effet |
|---|---|---|---|
| `default` | texte | aucun | L'équipe que lance `recruit` seul quand le fichier du projet en contient plusieurs. Sans elle, recruit demande laquelle. Lue dans les fichiers du projet. |

## `[claude]`

Vaut pour toutes les équipes du fichier.

| Clé | Type | Défaut | Effet |
|---|---|---|---|
| `command` | texte | `"claude"` | L'exécutable de Claude Code, un nom cherché dans le `PATH` ou un chemin. |
| `config_dir` | texte | celui de l'environnement | Le profil Claude Code de l'équipe : son dossier de configuration, donné aux membres comme `CLAUDE_CONFIG_DIR`. Un chemin absolu, ou qui commence par `~/`. Le dossier doit exister. Voir [Profil Claude Code](/recruit/fr/guides/claude-code/#profil-claude-code). |

## `[tmux]`

Ignorée. recruit 1 faisait tourner ses équipes dans tmux, et cette section réglait tmux. Depuis recruit 2, les équipes dessinent leur propre [écran](/recruit/fr/guides/screen/), sans rien à régler : la section est encore lue, pour qu'un fichier plus ancien marche toujours, et le lancement dit qu'elle n'a plus d'effet. Ses clés sont celles de recruit 1 (`socket`, `mouse`, `user_config`, `options`), et une autre est refusée. recruit 2 se sert encore de `socket` pour trouver une équipe que recruit 1 a laissée tourner. Retire la section du fichier pour faire taire l'avertissement.

## Équipes

`[teams.<équipe>]` : une table par équipe. Le nom de l'équipe en est la clé : lettres (accents compris) et chiffres, `-` et `_`, 40 caractères au plus, sans `-` au début, et pas une commande de recruit. Voir [Noms](/recruit/fr/guides/teams/#noms).

| Clé | Type | Défaut | Effet |
|---|---|---|---|
| `description` | texte | aucune | Une ligne sur l'équipe, pour toi. |
| `lang` | `"fr"` ou `"en"` | la langue de l'interface | La langue de l'équipe : celle du prompt que recruit écrit autour des instructions, des noms de ses onglets, de ses panneaux et de ses commandes (`/equipe` ou `/team`). |
| `instructions` | texte | aucune | Règles communes à tous les membres, ajoutées au prompt de chacun. |
| `permission_mode` | texte | celui de Claude Code | Mode de permission de Claude Code de tous les membres : `default`, `acceptEdits`, `auto`, `plan`, `bypassPermissions`… |
| `model` | texte | celui de Claude Code | Modèle de Claude de tous les membres : un alias (`opus`, `sonnet`, `haiku`, `fable`) ou un nom de modèle complet. |
| `effort` | texte | celui de Claude Code | Effort de tous les membres : `low`, `medium`, `high`, `xhigh` ou `max`. |
| `args` | liste de textes | `[]` | Arguments donnés en plus à `claude` pour tous les membres. |
| `layout` | `"auto"` ou `"tabs"` | `"auto"` | `auto` : les interlocuteurs dans un premier onglet, puis les agents de travail dans des onglets de `columns × rows` au plus. `tabs` : un onglet par membre. Voir [Onglets](/recruit/fr/guides/contacts-and-agents/#onglets). |
| `columns` | entier | `3` | Le nombre maximal de colonnes dans un onglet. |
| `rows` | entier | `2` | Le nombre maximal de lignes dans un onglet. |
| `dashboard` | booléen | `true` | Le [tableau de bord et le journal](/recruit/fr/guides/dashboard/), à droite des interlocuteurs. Avec `false`, les interlocuteurs sont côte à côte. |

## Membres

`[teams.<équipe>.members.<membre>]` : une table par membre, dans l'ordre des onglets. Le nom du membre en est la clé, et l'adresse à laquelle lui écrivent ses coéquipiers : lettres et chiffres, `-`, `_` et `.`, sans espace, 40 caractères au plus, sans `-` ni `.` au début. Deux membres d'une équipe ne peuvent pas différer que par la casse.

| Clé | Type | Défaut | Effet |
|---|---|---|---|
| `role` | texte | obligatoire | Une ligne : ce dont le membre est responsable. Ses coéquipiers la voient. |
| `contact` | booléen | `false` | L'un de tes interlocuteurs, dans le premier onglet ; deux au plus. Si aucun membre ne l'a, le premier membre est l'interlocuteur. Voir [Interlocuteurs](/recruit/fr/guides/contacts-and-agents/). |
| `instructions` | texte | aucune | Ce que fait ce membre, sa zone, comment il rend son travail. Ajoutées à son prompt. |
| `tab` | texte | `"Agents"` | Pour un agent de travail : l'onglet qu'il partage avec les agents qui ont le même. Ne peut pas prendre le nom d'un onglet de recruit (« Interlocuteurs », « Contacts », « Agents (1) »…). |
| `permission_mode` | texte | celui de l'équipe | Remplace celui de l'équipe pour ce membre. |
| `model` | texte | celui de l'équipe | Remplace celui de l'équipe pour ce membre. |
| `effort` | texte | celui de l'équipe | Remplace celui de l'équipe pour ce membre. |
| `args` | liste de textes | `[]` | Arguments en plus pour ce membre, après ceux de l'équipe. |

## Comment les valeurs se combinent

- Pour `permission_mode`, `model` et `effort` : la valeur du membre, sinon celle de l'équipe, sinon le défaut de Claude Code. Le [menu de l'équipe](/recruit/fr/guides/menu/) montre d'où vient chaque valeur.
- Entre `settings.local.toml` et `settings.toml` : les tables se fusionnent clé par clé, et toute autre valeur de `settings.local.toml`, une liste comprise, remplace celle de `settings.toml`.

## Ce que recruit refuse

recruit vérifie les fichiers chaque fois qu'il les lit (pour lancer une équipe, en lister, en rejoindre, en arrêter ou en modifier une, ouvrir le menu…), et s'arrête en nommant le fichier et la raison quand :

- une clé est inconnue, ou une valeur n'a pas le bon type ;
- le nom d'une équipe ou d'un membre ne suit pas les [règles des noms](/recruit/fr/guides/teams/#noms), ou deux membres d'une équipe ne diffèrent que par la casse ;
- un membre n'a pas de rôle ;
- une équipe a plus de deux interlocuteurs ;
- `config_dir` est un chemin relatif ;
- le `tab` d'un membre prend le nom d'un onglet de recruit.

Au lancement, il refuse aussi une équipe sans membre, et un `config_dir` dont le dossier n'existe pas.

recruit écrit aussi dans ces fichiers, depuis `recruit new` et le [menu de l'équipe](/recruit/fr/guides/menu/#où-sécrivent-les-changements) : il garde leurs commentaires.
