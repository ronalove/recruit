---
title: Interlocuteurs et agents de travail
description: À qui tu parles, qui travaille pour eux, et comment recruit répartit les membres en onglets.
sidebar:
  order: 2
---

Tu ne pilotes pas chaque agent toi-même. Tu parles à un ou deux membres, tes interlocuteurs. Ils confient le travail aux autres, et te disent où ça en est. Les autres, les agents de travail, s'occupent chacun de leur zone. Tu peux quand même regarder n'importe lequel, et intervenir quand tu veux.

En coulisse, chaque membre est une session Claude Code avec son nom et son rôle, et écrit à ses coéquipiers par `SendMessage`.

## Interlocuteurs

Tes **interlocuteurs** sont les membres à qui tu parles : le coordinateur, par exemple. Ils sont dans le premier onglet. Une équipe en a un ou deux :

```toml
[teams.web.members.coordinateur]
role = "Point d'entrée de l'utilisateur : répartit le travail, suit l'avancement"
contact = true
```

Si aucun membre n'a `contact = true`, le premier membre est l'interlocuteur. Le premier interlocuteur est l'**interlocuteur principal** : lui seul garde tout le bas de Claude Code, ta ligne d'état comprise (voir [Le bas de Claude Code](/recruit/fr/guides/claude-code/#le-bas-de-claude-code)).

## Agents de travail

Les autres membres sont des **agents de travail**. Ils reçoivent leur travail des interlocuteurs et leur rendent compte, sans t'attendre : leur prompt leur dit de poser leurs questions aux interlocuteurs quand ils sont bloqués, plutôt que d'attendre dans leur terminal.

Chaque agent garde un panneau visible. Regarde n'importe lequel, et écris dans son panneau quand tu dois intervenir : l'agent fait ce que tu demandes, puis tient les interlocuteurs informés.

## Onglets

Avec la disposition par défaut, `layout = "auto"` :

- **Le premier onglet**, « Interlocuteurs », réunit tes interlocuteurs, l'un au-dessus de l'autre, et à leur droite le [tableau de bord au-dessus du journal](/recruit/fr/guides/dashboard/). Avec `dashboard = false`, les interlocuteurs sont côte à côte.
- **Les agents de travail** suivent, regroupés selon leur `tab`, ou dans « Agents » s'ils n'en ont pas.
- **Un groupe plus grand qu'un onglet** (`columns × rows`, 3 × 2 = 6 par défaut) est réparti à parts égales : « Agents (1) », « Agents (2) »… Par exemple, 8 agents donnent 4 + 4, et 7 donnent 4 + 3. Un groupe `tab` trop grand est réparti de la même façon : « Code (1) », « Code (2) »…
- **Chaque onglet est une grille** : le moins de lignes possible, puis le moins de colonnes que ces lignes demandent. Avec 3 colonnes : 2 panneaux côte à côte, 4 en 2 × 2, 5 en 3 + 2, 6 en 3 × 2.

Dans une équipe en anglais, le premier onglet s'appelle « Contacts ». Le `tab` d'un membre peut prendre n'importe quel titre sauf ceux de recruit, dans les deux langues : « Interlocuteurs », « Contacts », « Agents (1) », « Agents (2) »… « Agents » seul est le groupe par défaut.

![Un onglet d'agents de travail en grille 2 × 2, dont deux qui attendent une permission](../../../../assets/screenshots/agents.png)

```toml
[teams.web]
columns = 3            # 3 colonnes au plus…
rows = 2               # …et 2 lignes dans un onglet

[teams.web.members.dev-back]
role = "Serveur et API"
tab = "Code"           # partage l'onglet « Code » avec les autres agents qui l'ont

[teams.web.members.dev-front]
role = "Interface web"
tab = "Code"
```

La grille se règle aussi depuis le [menu de l'équipe](/recruit/fr/guides/menu/#les-actions-déquipe), « Grille » (`g`) : 4 × 2, 3 × 2, 2 × 2, 3 × 1 ou 2 × 1. Une grille plus petite donne plus de place à chaque membre, sur plus d'onglets. Un terminal n'a qu'une taille de police pour toute sa fenêtre : un panneau ne peut pas avoir sa propre police plus petite, et c'est la grille et le focus, plus bas, qui font de la place.

### Focus

Quand un onglet compte plus de membres que tu ne peux en suivre, `layout = "focus"` en montre un en grand et seulement les plus occupés à côté :

- **À gauche**, le membre que tu as choisi, sur toute la hauteur de l'onglet.
- **À droite**, les mieux placés des autres, l'un au-dessus de l'autre : ceux qui t'attendent, ceux dont le tour a raté, ceux au travail, puis ceux au repos depuis le moins longtemps. Trois, ou deux dans une fenêtre de moins de 120 × 36.
- **En dessous**, les autres en cartes, comme au tableau de bord, dans un cadre titré « autres » qui les compte par état. Une carte montre l'état du membre, son modèle et son effort, ce qu'il fait, et la courbe de sa dernière demi-heure.

Les places changent lentement, pour que rien ne saute sous tes yeux. Un membre qui se met à t'attendre, ou dont le tour rate, passe à droite tout de suite, mais jamais à la place d'un autre qui attend ou a raté son tour. Un membre qui se met au travail y passe au bout de 30 secondes, à la place du moins bien placé. Sinon, un panneau garde sa place une minute au moins. Rien ne bouge tant que le pointeur est à droite ou sur les cartes, pendant que tu tapes, ni dans la seconde qui suit un clic.

| Pour | Fais |
|---|---|
| amener un membre à gauche | clique sur sa carte, double-clique sur le bord du haut de son panneau à droite, ou `Alt+n` pour le suivant de l'onglet |
| taper dans un panneau de droite | clique dedans : il reçoit les touches, et reste où il est |
| revenir à la grille, ou essayer le focus | `Alt+l`, ou le bouton « grille (⌥l) » ou « focus (⌥l) » de la barre : pour l'onglet courant seulement, jusqu'à ce que tu te détaches |

Le premier onglet, avec le tableau de bord et le journal, garde sa grille, tout comme un onglet dans une fenêtre de moins de 80 colonnes ou de 14 lignes. « Disposition » (`l`) dans le menu de l'équipe passe toute l'équipe de la grille au focus, ou l'inverse, et l'écrit dans le fichier.

### Un onglet par membre

`layout = "tabs"` donne à chaque membre son propre onglet, à son nom, interlocuteurs en tête. Le tableau de bord et le journal vont dans le premier onglet.

## Changer en cours de route

Le [menu de l'équipe](/recruit/fr/guides/menu/) fait d'un membre un interlocuteur ou un agent de travail, le change d'onglet, et règle la grille et la disposition. Les panneaux se déplacent sans que leur Claude s'arrête. Quand le seul interlocuteur part ou devient agent de travail, le menu demande qui le remplace.

## Passer d'un onglet à l'autre

`Alt+1`…`Alt+9` mènent à un onglet, `Alt+Maj+←` / `Alt+Maj+→` au précédent ou au suivant, `Alt+n` au membre suivant de l'onglet (un membre montré sur tout l'onglet le reste : le suivant prend sa place), et un clic sur la carte d'un membre dans le tableau de bord mène à son panneau. Quand un membre que tu ne vois pas t'attend, une notification te le dit, et `Alt+g` t'y mène. Voir [L'écran de l'équipe](/recruit/fr/guides/screen/).
