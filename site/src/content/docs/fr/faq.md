---
title: FAQ et dépannage
description: Bon à savoir avant de commencer, et que faire quand quelque chose ne marche pas.
---

## Bon à savoir

### Fermer le terminal n'arrête pas l'équipe

Une équipe tourne dans le serveur tmux propre à recruit : fermer la fenêtre, même avec `Cmd+q`, laisse les agents travailler. `recruit`, ou `recruit attach`, les retrouve là où tu les as laissés, depuis n'importe quel terminal, même en SSH. Pour arrêter une équipe : `Alt+q` puis « Quitter » (`q`), ou `recruit stop`.

### Approbation du dossier

Claude Code demande à chaque nouvelle session s'il peut faire confiance à un dossier qu'il n'a jamais approuvé, et sa réponse par défaut le ferme. recruit te prévient avant de lancer. Pour approuver le dossier une fois pour toute l'équipe, lance d'abord `claude` dans ce dossier et accepte.

### Un seul endroit à la fois

Une équipe ne tourne qu'à un endroit à la fois : ses membres sont joints par leur nom, et deux copies recevraient les messages l'une de l'autre. Lancer une équipe qui tourne déjà dans un autre dossier est refusé ; depuis le même dossier, recruit la rejoint.

### Mêmes noms ailleurs

Une autre équipe, ou une session à toi, peut porter le nom d'un membre. recruit n'arrête rien et ne renomme personne : le prompt de chaque membre donne la session tmux de son équipe, et quand `ListAgents` montre plusieurs sessions sous un même nom, le membre écrit à celle dont la ligne indique cette session, à son adresse « nom [ref] ». Le tableau de bord nomme ces sessions, s'il a la place.

Des sessions au nom d'un membre déjà ouvertes dans le dossier de l'équipe sont refusées au lancement : on ne pourrait pas les distinguer.

### Prompt à la reprise

Avec [le mod de recruit](/recruit/fr/guides/claude-code/#le-mod-de-recruit) (Claude Code 2.1.287 ou plus récent), un membre repris apprend son rôle, ses instructions et ses coéquipiers actuels avec son prochain message, s'ils ont changé. Sans le mod, une conversation reprise garde le prompt de son début : après avoir changé les rôles, relance des sessions neuves plutôt que `--resume`.

### Langue

Français ou anglais. L'interface suit `--lang`, puis `RECRUIT_LANG`, `LC_ALL` et `LC_MESSAGES`, puis la langue du système. Sous macOS, les terminaux fixent souvent `LANG=en_US.UTF-8` d'eux-mêmes ; la langue du système y passe donc avant `LANG`.

Une équipe a sa propre langue, `lang` dans son fichier, fixée à sa création : celle de ses prompts, de ses onglets et de ses commandes.

### Ce que coûte une équipe

Chaque membre est une session Claude Code sur ton compte, et l'utilise comme n'importe quelle autre. En plus, avec le mod, Haiku résume chaque nouvelle demande en un court appel pour le tableau de bord, et compacter une conversation depuis le tableau de bord coûte ce que coûte la compaction de Claude Code. La ligne du bas du tableau de bord montre ton usage sur 5 heures et sur 7 jours.

### Tes réglages de Claude Code

recruit n'écrit rien dans les fichiers de réglages de Claude Code. Les membres autres que l'interlocuteur principal n'ont pas ta ligne d'état pour leur session seulement, par `--settings`, et le menu de l'équipe écrit ses changements dans les fichiers de l'équipe.

## Dépannage

### « tmux est introuvable » ou « … est trop ancien »

recruit demande tmux 3.5 ou plus récent. Installe-le ou mets-le à jour : `brew install tmux` sous macOS, le paquet de ta distribution ailleurs (certaines livrent encore une version plus ancienne : Homebrew sous Linux en a une récente). `tmux -V` affiche la version.

### Chaque membre demande « Do you trust this folder? », ou se ferme aussitôt

Claude Code n'a pas encore approuvé le dossier, et Entrée seule répond « No, exit ». Réponds « Yes, I trust this folder » dans chaque panneau, ou arrête l'équipe, lance `claude` une fois dans le dossier pour l'approuver, et relance :

```sh
recruit stop -y
claude            # accepte, puis /exit
recruit
```

### Un membre s'est arrêté et n'est pas relancé

Quand le Claude d'un membre s'arrête deux fois de suite juste après son démarrage, son panneau cesse d'essayer et le dit. Regarde l'erreur dans le panneau, corrige la cause (un argument faux dans `args`, un profil non connecté…), puis :

```sh
recruit <équipe> --restart --resume
```

### Sous macOS, `Alt+j` ou `Alt+r` tape un caractère

Le terminal envoie Option comme un caractère, pas comme Alt. Règle-le pour qu'il envoie Alt : voir [Option sous macOS](/recruit/fr/guides/tmux/#option-sous-macos). En attendant, les boutons « menu » et « quitter » à droite de la barre tmux, et `/recruit` et `/equipe` dans l'invite d'un membre, font la même chose.

### Le tableau de bord n'affiche ni modèle, ni contexte, ni usage, et `/recruit` est inconnu

Tout cela vient du mod de recruit, qui demande Claude Code 2.1.287 ou plus récent. Mets Claude Code à jour, puis relance l'équipe. Là où une organisation n'autorise pas les mods installés par ses utilisateurs, l'équipe marche sans.

### « le profil Claude … n'existe pas »

`[claude] config_dir` nomme un dossier qui n'existe pas encore. Crée le profil en lançant une fois Claude Code avec, et connecte-toi :

```sh
CLAUDE_CONFIG_DIR=~/.claude-perso claude
```

### « l'équipe … tourne déjà dans … »

L'équipe tourne dans un autre dossier, et une équipe ne tourne qu'à un endroit à la fois. Rejoins-la avec `recruit attach <équipe>`, ou arrête-la avec `recruit stop <équipe>`.

### « déjà ouvertes dans ce dossier : … »

Des sessions Claude Code au nom de membres sont ouvertes dans le dossier de l'équipe, en dehors de l'équipe. Ferme-les d'abord : deux sessions porteraient le même nom.

### « configuration invalide », « n'a pas de rôle », « interlocuteurs (contact = true), 2 au plus »

Le fichier de l'équipe ne se lit pas. Le message nomme le fichier et la raison : une clé inconnue ou un mauvais type, un membre sans `role`, plus de deux interlocuteurs. `recruit edit` ouvre quand même le fichier, et dit à la fermeture de l'éditeur s'il reste une erreur. Voir [Ce que recruit refuse](/recruit/fr/reference/configuration/#ce-que-recruit-refuse).

### « ne peut pas porter ce nom »

Le nom d'une équipe ou d'un membre, écrit à la main dans le fichier de l'équipe, ne suit pas les [règles des noms](/recruit/fr/guides/teams/#noms) de recruit. Le message nomme le fichier, le nom et la raison :

```
recruit: /home/moi/mon-app/.recruit/settings.toml : le membre « -dev » de l'équipe « web » ne peut pas porter ce nom : lettres, chiffres, -, _ et . seulement, sans espace, sans - ni . au début, 40 caractères au plus. Renomme-le dans le fichier.
```

Deux membres dont les noms ne diffèrent que par la casse donnent « ne diffère de … que par la casse ». `recruit edit` (ou `recruit edit <équipe>`, `recruit edit --local`) ouvre quand même le fichier : renomme-y l'équipe ou le membre. Tant qu'il n'est pas corrigé, les autres commandes de recruit qui lisent le fichier s'arrêtent dessus ; le fichier d'une équipe globale les arrête toutes, puisque chacune lit toutes les équipes globales. Une équipe qui tourne déjà continue de travailler : son menu s'ouvre en lecture seule, et `Alt+q` permet toujours de se détacher ou de la quitter.

### « plusieurs équipes dans ce projet »

Sans terminal pour demander, recruit ne peut pas choisir. Nomme l'équipe (`recruit <équipe>`), ou fixe `default` en tête de `.recruit/settings.toml`.

### Un changement dans `[tmux]` n'a pas d'effet

recruit lit `[tmux]` au démarrage de son serveur tmux. Arrête le serveur, ce qui arrête toutes les équipes qui y tournent, puis relance :

```sh
tmux -L recruit kill-server
```

### Le menu ne s'ouvre pas

Le menu demande une fenêtre de 50 colonnes et 14 lignes au moins : agrandis le terminal. Avec le mod, `/recruit` dans l'invite d'un membre l'ouvre aussi.
