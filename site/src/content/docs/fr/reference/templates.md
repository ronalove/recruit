---
title: Équipes intégrées
description: Les types de projet et les tailles que connaît recruit, les membres de chacun, et les rôles intégrés.
sidebar:
  order: 3
---

recruit connaît 8 types de projet, chacun en 3 tailles. Choisis-en un dans la création guidée, ou avec `recruit new <équipe> --template <type> --size <taille>`. `recruit templates` affiche cette liste.

Chaque membre arrive avec son rôle et ses instructions, et l'équipe avec les règles communes de recruit. Le coordinateur est l'interlocuteur ; les autres membres sont des agents de travail. Les noms et les textes sont écrits dans la langue de l'interface au moment de la création : en français ci-dessous, en anglais avec `--lang en` (`coordinator`, `backend`…).

## Tailles

| Taille | Membres |
|---|---|
| `small` | 3 : l'essentiel |
| `medium` | 5 : quelques spécialistes en plus |
| `large` | 8 : planification, revue et spécialistes |

## Types de projet

| Type | | Petite | Moyenne | Grande |
|---|---|---|---|---|
| `personal` | Projet personnel : un outil ou un site perso, peu de formalités, l'essentiel du code et de l'interface | coordinateur, développeur, designer | coordinateur, développeur, designer, reviewer, testeur | coordinateur, planificateur, reviewer, rédacteur, dev-back, dev-front, designer, testeur |
| `web` | Application web : serveur et interface web, avec design et revue | coordinateur, dev-back, dev-front | coordinateur, reviewer, dev-back, dev-front, designer | coordinateur, planificateur, reviewer, sécurité, devops, dev-back, dev-front, designer |
| `mobile` | Application mobile, avec son serveur si elle en a un | coordinateur, dev-mobile, designer | coordinateur, reviewer, dev-mobile, dev-back, designer | coordinateur, planificateur, reviewer, devops, dev-mobile, dev-back, designer, testeur |
| `api` | API / service : un service sans interface, centré sur la fiabilité et les tests | coordinateur, dev-back, testeur | coordinateur, reviewer, devops, dev-back, testeur | coordinateur, planificateur, reviewer, sécurité, devops, rédacteur, dev-back, testeur |
| `library` | Bibliothèque : un paquet réutilisable, API publique, tests et documentation | coordinateur, développeur, testeur | coordinateur, reviewer, rédacteur, développeur, testeur | coordinateur, planificateur, reviewer, rédacteur, devops, développeur, testeur, performance |
| `data` | Données : pipelines, analyses, modèles et traitements de données | coordinateur, data, développeur | coordinateur, reviewer, data, développeur, testeur | coordinateur, planificateur, reviewer, devops, rédacteur, data, développeur, testeur |
| `game` | Jeu : mécaniques, rendu et game design | coordinateur, gameplay, designer | coordinateur, gameplay, graphismes, designer, testeur | coordinateur, planificateur, reviewer, gameplay, graphismes, designer, testeur, performance |
| `infra` | Infrastructure : déploiement, serveurs, configuration et sécurité | coordinateur, devops, sécurité | coordinateur, reviewer, rédacteur, devops, sécurité | coordinateur, planificateur, reviewer, sécurité, rédacteur, devops, performance, testeur |

## Rôles intégrés

Les rôles où puisent les modèles. Le menu de l'équipe les propose pour un nouvel agent, et Claude part d'eux quand il compose une équipe.

| Nom | Nom en anglais | Rôle |
|---|---|---|
| coordinateur | coordinator | Point d'entrée de l'utilisateur : répartit le travail, suit l'avancement, arbitre entre les zones |
| planificateur | planner | Tient le plan : découpage en tâches, priorités, dépendances et liste des tâches pour plus tard |
| reviewer | reviewer | Relit les changements : justesse, lisibilité, cohérence avec le code existant et les conventions |
| sécurité | security | Sécurité : secrets, authentification, droits, entrées non fiables, dépendances, configuration exposée |
| devops | devops | Build, CI, déploiement, environnements et outillage du dépôt |
| rédacteur | writer | Documentation : README, guides, référence d'API, exemples et notes de version |
| développeur | developer | Développement : écrit et fait évoluer le code du projet, hors des zones confiées à d'autres membres |
| dev-back | backend | Côté serveur : logique métier, API, accès aux données, tâches de fond et intégrations externes |
| dev-front | frontend | Interface web : pages, composants, état côté client, appels à l'API, accessibilité |
| dev-mobile | mobile | Application mobile : écrans, navigation, état local, échanges avec le serveur, intégration système |
| gameplay | gameplay | Gameplay : mécaniques, règles, contrôles, états du jeu, IA et physique |
| graphismes | graphics | Rendu et visuels : affichage, shaders, caméra, animations, effets et ressources graphiques |
| data | data | Données : ingestion, transformations, schémas, analyses, modèles et leur évaluation |
| designer | designer | Design et expérience : parcours, interface, cohérence visuelle, textes affichés et ressenti |
| testeur | tester | Tests : stratégie, intégration et bout en bout, reproduction des bugs, non-régression |
| performance | performance | Performance : mesures, profilage, optimisation des points chauds, suivi des régressions |

## Règles communes

Chaque équipe intégrée reçoit ces règles, comme `instructions` :

- Le coordinateur est le point d'entrée de l'utilisateur : il découpe le travail, le répartit et tranche les questions entre zones.
- Si l'utilisateur te parle directement, fais ce qu'il demande et tiens le coordinateur informé.
- Ne modifie que ta zone. Ce qui touche la zone d'un autre membre passe par le coordinateur, ou demande l'accord explicite de ce membre.
- Messages courts et précis : nomme les fichiers, les commits ou les commandes en cause, et dis ce que tu attends en retour.
- Personne ne commite ni ne pousse sans la demande de l'utilisateur.
- Le dépôt git est partagé avec des membres qui travaillent en ce moment même : aucune commande destructrice (git reset --hard, git checkout sur des fichiers, git stash, git clean, suppressions en masse) qui effacerait le travail d'un autre. Dans le doute, demande au coordinateur.
- Vous travaillez tous en même temps dans le même dossier : préviens le coordinateur avant toute action qui modifie l'état partagé (installation ou mise à jour de dépendances, migrations, régénération de fichiers, serveur sur un port fixe).
- Les choix produit reviennent à l'utilisateur, par l'intermédiaire du coordinateur ; les choix techniques dans ta zone te sont délégués : fais-les et explique-les.
- Le CLAUDE.md du projet s'applique à tous ; lis-le avant de commencer.
- Dis honnêtement ce qui n'est pas vérifié : ce que tu n'as ni lancé, ni testé, ni relu est signalé comme tel.
- Quand tu rends un travail, dis au coordinateur ce qui a été fait, les fichiers touchés, les vérifications lancées avec leur résultat, et ce qui reste ouvert.
- Les tâches pour plus tard vont au planificateur s'il y en a un dans l'équipe, sinon au coordinateur.

Elles sont un point de départ : une fois l'équipe créée, modifie-les dans son fichier (`recruit edit`). Le rôle et les instructions de chaque membre se changent aussi dans le [menu de l'équipe](/recruit/fr/guides/menu/).
