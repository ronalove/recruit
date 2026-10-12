# Inventaire de l'architecte

Part : `backend.rs`, `launch.rs`, `live.rs`, `app.rs`, `main.rs`, `Cargo.toml`.

## Supprimer

- `launch.rs:559-612` `mod legacy` (tmux de la 1.x : `running`, `stop`, `stop_for_launch`) ; `launch.rs:49-66` (avertissement `[tmux]`, arrêt de l'équipe sous tmux avant le lancement). Décision de l'utilisateur. Le commentaire de `check_duplicates` (« from recruit 1's tmux ») suit.
- `app.rs:263-266` (`legacy` dans `list`), `app.rs:283` (champ `"tmux"` de `recruit list --json`), `app.rs:299` (« en cours sous recruit 1 (tmux) »), `app.rs:366-373` (erreur de `target`), `app.rs:405-437` `stop_legacy` et son appel dans `stop`.
- `config.rs:58,91-105,278,398` `TmuxSettings`, `Found.tmux` : part de reviewer (config), à faire dans le même lot pour que l'arbre compile.
- `backend.rs:126-156` le trait `Backend` : une seule mise en œuvre (`Native`). Ses méthodes deviennent celles de `Native` (part de dev-serveur pour `native.rs`), `Box<dyn Backend>` et `&dyn Backend` deviennent `Native` et `&Native` (`launch.rs`, `live.rs`, `app.rs`, `board.rs`, `look.rs`, `menu.rs`, `mux/mod.rs`). `for_session`, `of` et `running` restent comme constructeurs (`Native::for_session`, `Native::running`).
- `backend.rs:111-114` `session_name` : un nom d'équipe valide n'a ni `.` ni `:` (`config::validate_team_name`, vérifié à chaque lecture depuis le 2026-10-08) ; la règle venait des sessions tmux de la 1.x. La session d'une équipe est son nom (8 appels). Risque nul pour un nom valide ; un nom tapé à la main dans `recruit stop a.b` ne trouvera plus `a_b`, qui ne peut pas exister.
- `live.rs:54-62` `origin()` et `launch.rs:385-386` : « lancée par une version plus ancienne de recruit » ; à vérifier avec dev-serveur (`state.rs`) : si toute équipe en cours a été lancée par une 2.x, `Snapshot.origin` est toujours là et ces chemins tombent.

## Simplifier

- **Le panneau d'un membre se construit à trois endroits** : `launch.rs:197-218` (lancement), `launch.rs:400-414` (`repair`), `live.rs:260-269` (`member_pane`). Les trois font `/bin/sh -c <member_script>` + `member_env`. Un seul `launch::member_pane(snapshot ou équipe, exe, state, name, resume) -> Pane`.
- **Les panneaux du tableau de bord et du journal, à trois endroits aussi** : `launch.rs:221-234`, `launch.rs:430-436`, `live.rs:290-302`. Un seul `launch::panel_pane(kind, exe, lang, state) -> Pane`.
- **Les sessions déjà ouvertes dans le dossier** : `launch::open_here` (`launch.rs:519`) et `live::Running::check_open` (`live.rs:164`) filtrent pareil (même dossier, nom dans une liste). Un seul `claude::open_in(running, dir, names)`.
- **Le préambule des modifications à chaud** : `apply`, `restart`, `restart_all`, `dismiss` (`live.rs:70-150`) font tous `guard`, verrou, relecture de `team.json`, puis `config()`. Un `fn begin(&mut self) -> Result<(Lock, Found)>`.
- **`launch()` fait 250 lignes** (`launch.rs:38-287`). Découpage : `running_team` (l'équipe tourne déjà : rattacher, réparer ou arrêter), `claude_for_launch` (exécutable, profil, doublons, approbation du dossier), `plan` (onglets et panneaux, commandes affichées, argv), puis l'écriture de `team.json` et le lancement. Aucun changement de comportement.
- **La session d'un essai** : `format!("{}{}", session_name(name), DRY_RUN)` à trois endroits (`app.rs:262,328,359`). Un `trial_session(name)`.
- **Le nom de l'équipe visée par `attach`/`stop`** : `app.rs:352-357` et `app.rs:408-413` (part avec `stop_legacy`).
- **En travers** : `std::env::current_exe()…to_string_lossy()` 6 fois (`launch.rs` ×2, `live.rs`, `mux/native.rs` ×2, `mux/pty.rs`) ; l'heure en secondes depuis l'époque 11 fois (`board.rs` ×5, `launch.rs` ×2, `mux/server.rs` ×2, `bridge.rs`, `member.rs`), avec déjà deux `now()` (`board.rs:2912` en `i64`, `mux/server.rs:798` en `u64`). Une fonction chacune, dans un module commun (à placer avec l'arborescence, lot B).

## Découper

- `launch()` (voir plus haut).
- `backend.rs` sans le trait : il ne garde que les types d'un lancement (`Plan`, `TabPlan`, `Side`, `Pane`, `PaneState`, `JournalSize`, `RunningTeam`, `MenuOpened`) et `crashed`/`recovered`. Nom proposé : `team/plan.rs`.

## Renommer

- `backend.rs:1-4` (doc du module : « which the Backend trait drives ») ; `mux/mod.rs:3-4` (« to replace tmux », `RECRUIT_BACKEND=native until step 4 ») : part de dev-terminal.
- `Pane.member` « the member's name, or the panel's title » : un `title` serait plus juste ; à voir avec dev-serveur (le protocole porte peut-être ce nom).

## Déplacer

- Arborescence cible de `refacto.md`, lot B.

## Dépendances (`Cargo.toml`)

Toutes servent : `memchr` (claude.rs, sniff.rs), `memmap2` (claude.rs), `shlex` (launch.rs), `inquire` (ui.rs), `indexmap`, `sys-locale`, `toml_edit`, `libc`. `alacritty_terminal` reste en dépendance de test (28 usages dans `tests/`). Rien à retirer.

## Hors refacto

- Rien de vu.

## Estimation

- Lot A, ma part : `legacy`, `stop_legacy`, le trait `Backend`, `session_name` : ~1 h, dépend de reviewer (`TmuxSettings`) et de dev-serveur (`native.rs`) dans le même lot.
- Lot C, ma part : `member_pane`, `panel_pane`, `begin`, découpage de `launch()` : ~1 h 30.
