---
title: Built-in teams
description: The project types and sizes recruit knows, the members of each, and the built-in roles.
sidebar:
  order: 3
---

recruit knows 8 project types, each in 3 sizes. Pick one in the guided creation, or with `recruit new <team> --template <type> --size <size>`. `recruit templates` prints this list.

Each member comes with its role and its instructions, and the team with recruit's shared rules. The coordinator is the contact; the other members are working agents. The names and the texts are written in the interface's language when the team is created: English below, French with `--lang fr` (`coordinateur`, `dev-back`…).

## Sizes

| Size | Members |
|---|---|
| `small` | 3: the essentials |
| `medium` | 5: a few specialists on top |
| `large` | 8: planning, review and specialists |

## Project types

| Type | | Small | Medium | Large |
|---|---|---|---|---|
| `personal` | Personal project: a personal tool or site, little ceremony, just the code and the interface | coordinator, developer, designer | coordinator, developer, designer, reviewer, tester | coordinator, planner, reviewer, writer, backend, frontend, designer, tester |
| `web` | Web app: server and web interface, with design and review | coordinator, backend, frontend | coordinator, reviewer, backend, frontend, designer | coordinator, planner, reviewer, security, devops, backend, frontend, designer |
| `mobile` | Mobile app, plus its server if it has one | coordinator, mobile, designer | coordinator, reviewer, mobile, backend, designer | coordinator, planner, reviewer, devops, mobile, backend, designer, tester |
| `api` | API / service: a headless service, focused on reliability and tests | coordinator, backend, tester | coordinator, reviewer, devops, backend, tester | coordinator, planner, reviewer, security, devops, writer, backend, tester |
| `library` | Library: a reusable package, public API, tests and documentation | coordinator, developer, tester | coordinator, reviewer, writer, developer, tester | coordinator, planner, reviewer, writer, devops, developer, tester, performance |
| `data` | Data: pipelines, analyses, models and data processing | coordinator, data, developer | coordinator, reviewer, data, developer, tester | coordinator, planner, reviewer, devops, writer, data, developer, tester |
| `game` | Game: mechanics, rendering and game design | coordinator, gameplay, designer | coordinator, gameplay, graphics, designer, tester | coordinator, planner, reviewer, gameplay, graphics, designer, tester, performance |
| `infra` | Infrastructure: deployment, servers, configuration and security | coordinator, devops, security | coordinator, reviewer, writer, devops, security | coordinator, planner, reviewer, security, writer, devops, performance, tester |

## Built-in roles

The roles the templates draw from. The team's menu offers them for a new agent, and Claude starts from them when it composes a team.

| Name | Name in French | Role |
|---|---|---|
| coordinator | coordinateur | The user's entry point: hands out work, tracks progress, settles questions between areas |
| planner | planificateur | Keeps the plan: task breakdown, priorities, dependencies and the backlog of later work |
| reviewer | reviewer | Reviews changes for correctness, readability and consistency with existing code and conventions |
| security | sécurité | Security: secrets, authentication, permissions, untrusted input, dependencies, exposed config |
| devops | devops | Build, CI, deployment, environments and repository tooling |
| writer | rédacteur | Documentation: README, guides, API reference, examples and release notes |
| developer | développeur | Development: writes and evolves the project's code, outside areas owned by other members |
| backend | dev-back | Server side: business logic, API, data access, background jobs and external integrations |
| frontend | dev-front | Web interface: pages, components, client-side state, API calls, accessibility |
| mobile | dev-mobile | Mobile app: screens, navigation, local state, server communication, platform integration |
| gameplay | gameplay | Gameplay: mechanics, rules, controls, game state, AI and physics |
| graphics | graphismes | Rendering and visuals: display, shaders, camera, animation, effects and graphic assets |
| data | data | Data: ingestion, transformations, schemas, analyses, models and their evaluation |
| designer | designer | Design and experience: user flows, interface, visual consistency, on-screen text and feel |
| tester | testeur | Testing: strategy, integration and end-to-end tests, bug reproduction, regression checks |
| performance | performance | Performance: measurement, profiling, optimizing hot spots and catching regressions |

## Shared rules

Every built-in team gets these rules, as its `instructions`:

- The coordinator is the user's entry point: they break work down, hand it out and settle questions between areas.
- If the user talks to you directly, do what they ask and keep the coordinator in the loop.
- Only change your own area. Anything that reaches into another member's area goes through the coordinator, or needs that member's explicit agreement.
- Keep messages short and specific: name the files, commits or commands involved, and say what you expect back.
- Nobody commits or pushes unless the user asks for it.
- The git repository is shared with members who are working in it right now: never run a destructive command (git reset --hard, git checkout of files, git stash, git clean, bulk deletes) that could wipe out someone else's work. When in doubt, ask the coordinator.
- You all work in the same folder at the same time: warn the coordinator before anything that changes shared state (installing or upgrading dependencies, migrations, regenerating files, starting a server on a fixed port).
- Product decisions belong to the user, through the coordinator; technical decisions in your area are yours to make, so make them and explain them.
- The project's CLAUDE.md applies to everyone; read it before you start.
- Be honest about what you have not verified: anything you did not run, test or read gets flagged as such.
- When you hand work back, tell the coordinator what was done, which files you touched, which checks you ran and their results, and what is still open.
- Tasks for later go to the planner if the team has one, otherwise to the coordinator.

They are a starting point: once the team is created, edit them in its file (`recruit edit`). Each member's role and instructions can also be changed in the [team's menu](/recruit/guides/menu/).
