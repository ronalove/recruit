# Refactorisation (après la 2.2.0)

Tenu par l'architecte. Ouverte le 2026-10-11 par l'utilisateur, qui a donné son accord pour les trois étapes à la suite.

## But

Un code plus petit et plus lisible, sans changer ce que fait recruit : retirer ce que tmux et la 1.x laissent derrière eux, ranger `src/` par domaine, découper les fichiers trop gros, supprimer les doublons et les détours devenus inutiles.

## Règles

- **Aucun changement de comportement.** Pas de fonctionnalité au passage, pas de texte vu par l'utilisateur modifié, sauf ce qui disparaît avec la 1.x. Un bug trouvé est noté (section « Hors refacto » de l'inventaire), pas corrigé dans un lot de refacto.
- **Pas de coût sur le chemin du dessin.** Le banc du testeur mesure avant (étape 1) et après (étape 3) ; un écart se justifie ou s'annule.
- **Déplacer avec `git mv`**, pour garder l'historique. Pas de changement de contenu dans le même geste qu'un déplacement, sauf les chemins `use`.
- La visibilité (`pub`, `pub(crate)`) ne se retouche que dans le code qu'on déplace ou découpe.
- Chaque lot : `cargo fmt` (sur ses fichiers seulement), `cargo clippy --all-targets -- -D warnings`, `RECRUIT_LANG=en cargo test` et `RECRUIT_LANG=fr cargo test`, relu par reviewer avant d'être accepté.

## Décision de l'utilisateur (2026-10-11)

On retire tout ce qui reste de la 1.x : `launch::legacy` (une équipe encore sous le tmux de la 1.x : question, arrêt, relance ; `recruit stop` sur elle ; la mention dans `recruit list`), la section `[tmux]` des fichiers d'équipe et son avertissement, `TmuxSettings`. Une section `[tmux]` restante devient une clé inconnue, refusée comme les autres (`deny_unknown_fields`). L'utilisateur est le seul utilisateur ; aucun de ses fichiers d'équipe n'a de section `[tmux]` (vérifié par l'architecte).

## Étapes

1. **Inventaire** (fait) : `specs/archive/refacto/<nom>.md`, un par membre ; le testeur ajoute le banc de référence.
2. **Lots**, dans cet ordre, chacun relu et vérifié avant le suivant :
   - **A. Suppressions et commentaires**, par zone, en parallèle.
   - **A'. Le trait `Surface`** (dev-terminal), fenêtre à part.
   - **B. Le trait `Backend`, les types en double, les déplacements** : l'architecte avec dev-serveur, personne d'autre ne code.
   - **C. Découpages et doublons**, par zone : C1 les découpages (chacun dans ses fichiers, en deux vagues), C2 les doublons entre zones (dans l'ordre donné).
   - **D. Corrections** des bugs trouvés, chacune avec son test.
3. **Vérification et documentation** : banc comparé, grille rejouée, relecture d'ensemble, CLAUDE.md et documents de `specs/` proposés à l'utilisateur.

## Décisions de la consolidation (architecte, 2026-10-11)

- **Noms.** `mux::native::Native` devient `mux::Mux` (une poignée sur le serveur d'une équipe), dans `mux/handle.rs` (ex-`native.rs`) ; les types d'un lancement (`Plan`, `TabPlan`, `Side`, `Pane`) dans `mux/plan.rs` (ex-`backend.rs`). Les paramètres `session` de ses méthodes disparaissent (la poignée la connaît). Voir « Interface du lot B ».
- **`sys.rs`** à la racine, à côté de `i18n.rs` : `write_atomic`, `write_temporary`, `cache_dir`, `home`, `expand_home`, `tilde`, FNV-1a (un seul calcul, épinglé par un test de valeurs connues : il nomme des fichiers et des sockets qui doivent rester stables), `now`, `current_exe`.
- **`ui::Rect` et `ui::Rgb`** : ils quittent `mux` (dix fichiers s'en servent, dont `team/layout`).
- **Sorties de la 1.x et d'avant la 1.0** : `Snapshot.origin` devient obligatoire (`auto_compact` reste optionnel : une équipe lancée en 2.0 ou 2.1 peut encore tourner) ; `session_name` disparaît (un nom valide n'a ni `.` ni `:`). L'alias `grid` de `layout` **reste** : le menu l'écrit lui-même (`config/edit.rs:517`).
- **Commentaires.** Partent : « Owner: », noms de membres, étapes (« step 2 »), étiquettes de maquette (« F3 », « mock-up C », « direction A »), « architect's decision », dates de décision (le CLAUDE.md garde l'historique), renvois à `multiplexeur-serveur.md` et `multiplexeur-prototype.md` (remplacés par une phrase quand la raison compte). Restent : les renvois à `specs/multiplexeur.md §x` qui expliquent un pourquoi, et les comparaisons à tmux qui expliquent une convention reprise (`update-environment`, `send-keys`, `escape-time`) ; tmux comme terminal du client (détection, `TMUX` retiré de l'environnement, pièges de `ui/list.rs`) reste partout.
- **Pas dans cette passe** (périmètre) : tables communes de `parse`/`encode` (dev-saisie), fusion des `wrap` (le comportement diffère), fusion des formats de `states.json`, `Engine::generation` sans `Option`, `set_colors`/`color_scheme`, rappels de libghostty-vt pour OSC 9/777, renommage de `engine::State`, fusion des boîtes de confirmation. `Terminal::open_with(true)` (mode 2027, emoji entiers) : rien dans la refacto, proposé à l'utilisateur après.
- **Gardés** : le SHA-256 écrit à la main (pas de dépendance), `Info.proto/version/started` (diagnostic), `alacritty_terminal` en dépendance de test (l'oracle indépendant du banc), `Caps.kitty_keyboard`.
- **`WHOLE_EVERY`** (réécriture du tableau de bord toutes les 30 s) part, avec `CLOCK_JUMP`, une fois que le tableau de bord écoute SIGWINCH : le seul cas où un panneau perd son contenu (moteur remplacé après une panique) l'envoie déjà (`Pane::repair`).

## Arborescence cible (arrêtée)

```
src/
  main.rs  cli.rs  app.rs  i18n.rs  sys.rs
  config.rs   config/{edit, check}.rs
  create.rs   create/{compose, templates, new}.rs          assistant, composition par Claude, équipes intégrées, `recruit new` scripté
  team.rs     team/{launch, live, state, layout, prompt, member}.rs
  claude.rs   claude/{session, args}.rs  claude/bridge.rs  claude/bridge/…
  ui.rs       ui/{ask, print, list, terminal, look, canvas}.rs  ui/canvas/{text, painter}.rs  ui/look/…
  board.rs    board/…
  menu.rs     menu/…
  mux.rs      mux/{handle, plan, client, ctl, proto, socket, layout, focus, cards, chrome, screen, select, keys, pty, engine, input, server}.rs et leurs dossiers
```

- `layout.rs` se coupe en deux : la répartition en onglets (`team/layout.rs`) et la géométrie de l'écran (`mux/layout.rs`) ; `reserved_title` va dans `config`.
- `backend.rs` devient `mux/plan.rs` (les types d'un lancement) ; ses types miroirs laissent la place à ceux du protocole (`PaneInfo`, `MenuOpened`, `JournalSize`), `crashed`/`recovered` vont dans `mux/socket.rs`.
- `ui.rs` ne garde que les modules, leurs réexportations (`ui::select`, `ui::confirm`… restent valables) et `Rect`/`Rgb`.

## Lots

Chaque lot : `cargo fmt` sur ses fichiers seulement, clippy sans avertissement, `RECRUIT_LANG=en cargo test` et `RECRUIT_LANG=fr cargo test`, puis rendu à l'architecte et relu par reviewer (le travail de reviewer est relu par l'architecte). Ne touche que les fichiers nommés ; une ligne chez un autre se fait avec son accord, dit dans le rendu.

### A. Suppressions et commentaires (en parallèle)

| Qui | Quoi (renvois à son inventaire) |
|---|---|
| architecte | sortie de la 1.x en un bloc : `launch::legacy`, `stop_legacy`, `list` (texte et `"tmux"` du JSON), `target`, `TmuxSettings`/`Found.tmux` (config), test du `[tmux]` refusé, FAQ, `commands.md`, `configuration.md` (EN et FR, `bun run build`) ; `session_name` ; `Snapshot.origin` obligatoire (S9 : `state.rs`, `live.rs`, `menu.rs`) ; `app.rs:42` (`_panel toggle`) |
| dev-serveur | S1 à S8, S11, M26 ; ses commentaires (« Renommer ») ; pas `state.rs` (architecte) |
| dev-interface | Supprimer 1 (`Kind::Toggle`, avec `cli.rs:90` et son test), Supprimer 5 (SIGWINCH puis retrait de `WHOLE_EVERY` et `CLOCK_JUMP`, avec le test de dev-serveur), Renommer 1, 2, 4, 5 |
| dev-rendu | Supprimer 1 (`gap`), 2 (`compose` des tests), 3 (un seul `Rng` de test) ; Renommer N1 à N5, N7 à N9 |
| dev-saisie | Supprimer 4 (aide française de `_edit` et `_menu`) ; Simplifier 3 ; Renommer 1, 3, 5 ; Supprimer 2 et 3 selon le testeur (`Caps.graphemes`, `RECRUIT_MUX_CAPS`) ; `open_with` reste |
| dev-terminal | S1, S2, S3 (avec `server.rs:4905`), R1 à R5 (`engine/base64.rs`) ; `mux/mod.rs` : commentaires seulement |
| reviewer | `dashboard_layers`, commentaires de `config` (« from 0.1 », doc de `BadName`, doc d'`Edit`) ; relit les autres |
| testeur | `RECRUIT_BACKEND` de `screenshots.sh`, `shots-backend.sh` → `shots-ctl.sh` (et ses appels), commentaire de `tests/common/pty.rs:4`, tests de la 1.x s'il y en a |

### A'. Le trait `Surface` (dev-terminal seul)

Z1 puis S4 : `engine::Surface` (ce que lisent l'écran et la sélection) et `Engine: Surface` ; touche `engine.rs`, `ghostty.rs`, `screen.rs`, `select.rs`, `cards.rs`, `server.rs`, `server/pane.rs`. Les autres ne codent pas pendant ce temps (une heure et demie).

### B. Trait `Backend`, types, déplacements (architecte et dev-serveur seuls)

1. `Backend` → `Mux` : dev-serveur `native.rs`, `proto.rs` (M17, M18, M19, M27 côté serveur), l'architecte les appelants (`launch`, `live`, `app`, `board` ×4 lignes, `look::Glyphs::of`, `menu`, `bridge`) ; `board::toggle` → `bridge`.
2. `sys.rs` et `ui::Rect`/`Rgb` (contenu déplacé, appels mis à jour).
3. `git mv` vers l'arborescence cible, `layout.rs` en deux, `mux/mod.rs` → `mux.rs`, `use` mis à jour (`handle.rs` et `plan.rs` sont déjà en place depuis B.1). Rien d'autre.

### Interface du lot B (architecte, 2026-10-11)

`mux/handle.rs` (ex-`native.rs`), réexporté en `mux::Mux` et `mux::running` :

```rust
pub(crate) struct Mux { state: PathBuf }
pub(crate) struct RunningTeam { pub session: String, pub team: String, pub dir: String, pub attached: bool }
pub(crate) fn running() -> Vec<RunningTeam>;                 // ex backend::running()
impl Mux {
    pub(crate) fn at(state: &Path) -> Mux;                     // le dossier d'état déjà connu (sous-commandes cachées, live, board)
    pub(crate) fn of(session: &str) -> Mux;                    // Mux::at(&state::dir(session)) : app et launch seulement
    pub(crate) fn is_running(&self) -> bool;                   // ex has_session
    pub(crate) fn launch(&self, plan: &Plan) -> Result<()>;
    pub(crate) fn attach(&self) -> Result<()>;
    pub(crate) fn stop(&self) -> Result<()>;
    pub(crate) fn panes(&self) -> Result<Vec<PaneInfo>>;       // ex PaneState ; l'état par look::State::reported(pane.status.as_deref())
    pub(crate) fn open_window(&self, dir: &Path, pane: &Pane) -> Result<String>;
    pub(crate) fn kill_pane(&self, id: &str) -> Result<()>;
    pub(crate) fn set_member(&self, id: &str, member: &str) -> Result<()>;
    pub(crate) fn respawn(&self, id: &str, dir: &Path, pane: &Pane) -> Result<()>;
    pub(crate) fn arrange(&self, tabs: &[Tab], columns: usize, focus: bool) -> Result<()>;
    pub(crate) fn open_panels(&self, dir: &Path, beside: &str, dashboard: &Pane, journal: &Pane) -> Result<()>;
    pub(crate) fn close_panels(&self) -> Result<()>;
    pub(crate) fn restore_dashboard(&self, dir: &Path, dashboard: &Pane) -> Result<()>;
    pub(crate) fn toggle_journal(&self, dir: &Path, journal: &Pane) -> Result<JournalSize>;   // proto::JournalSize
    pub(crate) fn detach(&self, client: &str) -> Result<()>;
    pub(crate) fn open_menu(&self, member: Option<&str>, client: Option<&str>) -> Result<MenuOpened>; // proto::MenuOpened
    pub(crate) fn clients(&self) -> Vec<ClientInfo>;           // ex client_terminals ; Glyphs::of prend des paires
}
```

- `mux/plan.rs` (ex-`backend.rs`, `git mv`) : `Plan` (sans `session` : la poignée la connaît), `TabPlan`, `Side`, `Pane`, `DASHBOARD`, `JOURNAL`, rien d'autre. `look::State::reported(Option<&str>) -> Option<State>` (un nom inconnu : aucun état, comme aujourd'hui) remplace `PaneState.status` ; `PaneState.window` devient `PaneInfo.tab`. `PaneState`, `MenuOpened`, `JournalSize`, `RunningTeam` et le trait en partent.
- `proto::JournalSize` (`full`, `reduced`, `hidden` : même JSON qu'aujourd'hui, `PROTO` inchangé, épinglé dans `the_protocol_is_as_numbered`) ; son texte (`said`) va dans `bridge` avec `board::toggle`.
- `socket::crashed(state)`, `socket::recovered(state)` (ex-`backend::crashed`, `recovered`).
- `look::Glyphs::of(clients: impl IntoIterator<Item = (&str, Option<&str>)>)` (terminal, nom) ; le serveur et le tableau de bord lui passent leurs clients.
- Partage : dev-serveur écrit `handle.rs` à côté de `native.rs` (les deux compilent), `proto::JournalSize` et son test de référence, `socket::crashed`/`recovered`, le côté serveur de `JournalSize` et de `Glyphs::of` ; puis l'architecte passe les appelants (`launch`, `live`, `app`, `board`, `look`, `bridge`, `menu`) et retire `native.rs` et le trait.

### C1. Découpages (chacun dans ses fichiers ; les chemins publics gardés par `pub(crate) use`)

Vague 1 : dev-serveur (D1 `server.rs` en sous-modules, puis D4 et M1 à M10), dev-interface (D5 `look`, D4 `chrome`, D1 `board`), dev-rendu (C1 `canvas`, C2), dev-saisie (`input` en `terminal` et `probe`, Simplifier 1 et 4).
Vague 2, quand la première a rendu : dev-serveur (D2 `bridge`, D3 `claude`, M12 à M16, M20 à M25, M28), dev-interface (D2 `sheet`, Simplifier 1, 2a, 3, 4, 8, 9, 11), dev-terminal (D1, D3, puis D2 mesuré), reviewer (`config/check.rs`, contrôles réunis, `local_files`, `Team::lang()`, `generate` `Output<T>`, `templates::member`, `prompt.rs`), architecte (`member_pane`, `panel_pane`, `begin`, `launch()` découpé, `open_in`, `trial_session`, `create/new.rs`).

### C2. Doublons entre zones (dans cet ordre)

1. `Scene` pour `compose` (dev-rendu, l'appel de `server.rs`).
2. Paramètres inutiles (dev-interface : `quit_choice`, `choice`, `journal_title` et leurs appels).
3. `Report::pressure()` (dev-serveur, puis `board`).
4. Une table des modèles et efforts (dev-serveur dans `claude`, puis `look`, `sheet`, `bridge`) ; une table des modes de permission (dev-interface, `config` et `wizard`).
5. `i18n::joined` et `t!` avec une langue (dev-saisie), puis leurs appels (`prompt`, `generate`, `sheet`, `chrome`, `board`).
6. `scroll_key` → `keys.rs` (dev-saisie) ; `JOURNAL_ROWS`, `COLUMNS` → `mux/layout.rs` ; grille par défaut en constantes de `config`.
7. Facultatif, s'il reste du temps et que le banc le permet : M2 (`Server` en structures par sujet), D3 `menu/draw`, D6 `menu/effects`.

### D. Corrections (chacune avec un test)

`fit` en colonnes partout (`canvas::fit`) ; l'assistant qui accepte « Dev » et « dev » ; `RESERVED` tenu à jour par un test contre les sous-commandes de clap ; une équipe globale qui en masque une autre ; `session_ref` qui ne vérifie pas le pid ; une équipe composée sans membre ; `find_executable` et le droit d'exécution ; `printable` et les caractères de direction ; l'en-tête de la note d'équipe dans la langue de l'équipe ; la doc qui ne cite pas `grid` ; le renifleur qui perd une séquence après un OSC (H1 de dev-terminal). Chacune par le propriétaire de la zone ; un texte vu qui change est dit à l'utilisateur.

## Suivi

- 2026-10-11 : étape 1 ouverte.
- 2026-10-11 : sept inventaires rendus (`specs/archive/refacto/`), consolidés ci-dessus. Le lot A attend que le banc de référence du testeur n'ait plus besoin de l'arbre.
- 2026-10-11 : lot A ouvert. Rendus : architecte (1.x, `session_name`, `Snapshot.origin` ; site vert), dev-rendu, dev-interface (SIGWINCH au tableau de bord, `WHOLE_EVERY` et `CLOCK_JUMP` retirés ; test d'intégration du redessin en cours), dev-serveur (M26 abandonné : les tests lisent le retour de `note_session`), dev-terminal. Tous : clippy propre, tests EN et FR verts. En relecture chez reviewer. Attendus : dev-saisie, testeur, la part de code de reviewer.
- 2026-10-11 : lot A relu par reviewer, reprises faites (dev-serveur, dev-rendu, dev-terminal R2, dev-interface : screenshots.sh et étiquettes). En attente : la part du testeur, puis les vérifications de clôture de reviewer, avec A' (dev-terminal, en cours). Pour l'étape 3 : le CLAUDE.md nomme encore `scripts/shots-backend.sh` (devenu `shots-ctl.sh`).
- 2026-10-11 : **lot A et A' clos**, vérifiés seuls par reviewer à faible charge (4,9 à 8) : rustfmt --check sur les 55 fichiers touchés, clippy, build, tests EN et FR (556 unitaires, mux 8, mux_screen 4, mux_server 9, mux_team 52, 0 échec), site vert. Les échecs de mux_team vus sous une charge de 8 à 11 ne reviennent pas au calme (attentes trop courtes, au lot C). Au lot D : `screenshots_script_asks_of_the_multiplexer` (ignoré) échoue aussi sur HEAD (attente périmée). Lot B ouvert.
- 2026-10-11 : **B1 fait et accepté** (reviewer) : `mux::Mux` (`mux/handle.rs`, dev-serveur), `backend.rs` devenu `mux/plan.rs`, `native.rs` et le trait supprimés, `proto::JournalSize`, `Glyphs::of` par paires, `look::State::reported`. Un texte change dans un cas limite : `/recruit` avec un team.json illisible pendant que le serveur répond ouvre le menu (en lecture seule) au lieu d'une erreur de lecture. Mesures « avant » du tableau de bord rendues (testeur.md) : seul au repos 21 o/s, dont ~17 o/s pour la réécriture des 30 s retirée au lot A.
- 2026-10-11 : **B2 et B3 faits** (architecte) : `sys.rs` (chemins, écriture atomique, `now`, `exe`, `fnv64` épinglé par les valeurs publiées) ; `git mv` vers `create/`, `team/`, `claude/bridge.rs`, `ui/canvas.rs`, `ui/look.rs`, `mux.rs` ; `layout.rs` coupé en `team/layout.rs` (onglets) et `mux/layout.rs` (géométrie), `reserved_title` dans `config` ; `ui::Rect`, `ui::Rgb`. Racine de `src/` : 13 fichiers et 7 dossiers (contre 22 fichiers et 4 dossiers). Vérifié : rustfmt sur le crate, clippy, build, tests EN et FR (558 unitaires, mux 8, mux_screen 4, mux_server 9, mux_team 52, charge 12). Le testeur a fait sa part de C dans `tests/` en avance (helpers communs, `mux_team.rs` en `tests/mux_team/` et 8 modules). En relecture chez reviewer.
- 2026-10-11 : **lot B clos** (reviewer : renommages dans l'index, contenu inchangé hors chemins, FNV identique) ; `sys::exe()` rend un chemin (remarque de reviewer). Part du testeur dans `tests/` acceptée ; `the_dashboard_draws_again_on_sigwinch` instable sous charge (condition « au repos » de départ), à fiabiliser par le testeur. **Lot C ouvert, vague 1** : dev-serveur (`server.rs`), dev-interface (`look`, `chrome`, `board`), dev-rendu (`canvas`, `screen`, `ui`), dev-saisie (`input`).
- 2026-10-12 : vague 1 du lot C : rendus et acceptés dev-saisie (`input` en trois, `KEYBOARD`), dev-rendu (`canvas` en quatre, `ui` en `ask`/`print`, tests de `screen` à part), architecte (`launch()` découpé, `Panes`, `open_in`, `begin`, `create/new.rs`, `Catalog::root`/`claude`) ; test SIGWINCH fiabilisé (testeur). En cours : dev-serveur (`server.rs`), dev-interface (`look`, `chrome`, `board`). C2 commencé : dev-saisie (`t!(in lang; …)`, `i18n::joined`, appelants de prompt, compose, team/layout), en relecture. Restent à basculer sur `t!(in lang; …)` ou `joined` : board.rs:55, chrome/choice.rs, menu/sheet.rs (dev-interface), ui/ask.rs (dev-rendu), ui/look/error.rs (dev-interface), claude/bridge.rs:491 (dev-serveur).
- 2026-10-12 : `server.rs` découpé (dev-serveur) accepté par reviewer : chemin du dessin identique (verrous, ordre des drops), mêmes essais de la souris, même format de `server.log`. Écart voulu et noté : un rôle de panneau inconnu (qu'aucun pair n'envoie) vaut maintenant « membre ». C2 de dev-saisie (`t!(in lang; …)`, `joined`) accepté : prompts identiques au caractère près. En cours : dev-interface (vague 1), dev-serveur (bridge, claude), dev-terminal (D2 après mesure), dev-saisie (`scroll_key`), dev-rendu (Scene, en relecture), architecte (part de reviewer : `config/check.rs`, `Problem`, `local_files`, `COLUMNS`/`ROWS`/`AGENTS`, `Team::language`, `compose::ask`/`answer`/`Output<T>`, `templates::member`).
- 2026-10-12 : acceptés par reviewer : D1, D2, D3 (dev-terminal ; D2 à confirmer au banc de l'étape 3), `scroll_key` (dev-saisie), `Scene` (dev-rendu), vague 2 de dev-serveur (`claude/bridge/` en 11 fichiers, `claude/{session, args}.rs`, M12 à M15, M20, M22, M25 ; écart noté : `_ctl clients` a maintenant un délai de 5 s), part de reviewer codée par l'architecte (`config/check.rs`, `Problem`, `local_files`, constantes de grille, `Team::language`, `compose::ask`/`answer`, `templates::member` ; `contacts()` une seule fois dans prompt.rs). Reste la vague 1 de dev-interface (`look`, `chrome`, `board`), puis la fin du C2.
- 2026-10-12 : vague 1 de dev-interface rendue (`look`, `chrome`, `board` découpés), en relecture ; il découpe `menu/sheet.rs` et fait ses simplifications. dev-serveur : `Report::pressure`, table des modèles (`claude/bridge/models.rs`), puis M2 en cours. **Lot D ouvert** sur les fichiers libres : dev-terminal (H1 renifleur, H3 `printable`), dev-saisie (`RESERVED` tenu par un test de clap), testeur (test des captures à l'attente périmée, fait), architecte (« Dev »/« dev » dans l'assistant, équipe globale masquée, équipe composée vide, `grid` documenté, faits). Textes vus qui changent au lot D (à dire à l'utilisateur) : « Ce nom est déjà pris. » devient « « x » est déjà dans l'équipe » ; deux erreurs nouvelles (équipe globale déjà définie ailleurs, équipe composée sans membre utilisable). Restent au lot D : `fit` en colonnes (dev-interface), `session_ref` et le pid, `find_executable` et le droit d'exécution, en-tête de la note d'équipe dans la langue de l'équipe (dev-serveur, après M2).
- 2026-10-12 : lot D accepté par reviewer pour dev-terminal (H1 renifleur, H3 `printable`), dev-saisie (`RESERVED`), testeur (test des captures), dev-serveur (`session_ref` et le pid, `find_executable` et `X_OK`, en-tête de la note dans la langue de l'équipe : le défaut d'origine n'existait probablement pas, `RECRUIT_LANG` donnant déjà la langue de l'équipe au processus `_mod` ; le correctif le rend explicite). M2 de dev-serveur et `menu/sheet.rs` découpé (dev-interface) rendus. En cours : dev-interface (Simplifier 2a, 3, 4, 8, 9, 10, 11 ; puis `fit` en colonnes au lot D), dev-rendu (`is_direction_mark`), dev-serveur (retrait de `Model::initial`).
- 2026-10-12 : C2 de dev-interface rendu (2a `Card` autour de `CardData`, 3 `Activity::note`, 4, 8, 9, 10, 11) ; deux écarts acceptés par l'architecte sous réserve de reviewer, à dire à l'utilisateur : la courbe compte un trou (veille, regard en retard) comme du repos au lieu d'un blanc, comme les cartes du focus ; deux sessions de même nom dans le même dossier (impossible) : la dernière au lieu de la première. Reste chez dev-interface : Simplifier 5, `fit` en colonnes (lot D), D3 (`menu/draw.rs`). **Étape 3 commencée** : l'utilisateur accepte la grille avec un vrai Claude Code aujourd'hui (`tests/mux_claude.rs` seul, testeur) et des commits locaux. Bilan des lignes (code de production, blancs et commentaires à part) : 24 501 → 23 654 ; 49 → 125 fichiers ; 555 → 571 tests.
- 2026-10-12 : relecture du C2 de dev-interface : l'écart (a) de la courbe était plus large qu'annoncé (la dernière case d'un membre au travail baissait entre deux regards, 3 à 10 % des images) : correctif demandé (`Activity.last`, test `curves` rétabli). (b) accepté (le menu suit le tableau de bord). `fit` en colonnes rendu (lot D) : un texte large est coupé avec « … » au lieu de déborder ; écart aussi en ASCII : plus d'espace avant « … ». Pour l'étape 3 : le CLAUDE.md nomme encore `_panel … toggle`.
- 2026-10-12 : **grille rejouée avec un vrai Claude Code 2.1.296** (testeur, campagne 3 de `multiplexeur-compat.md`) : modes, 77/77 à travers le multiplexeur, souris (test corrigé : il ne passait que par le minutage), rapport du mod, en-tête contre carte, compaction automatique de bout en bout (3 compactions, message pendant une compaction, `/compact` tapé, mot de contrôle retrouvé). `~/.claude/settings.json` identique avant et après. Quatre dossiers `claude-campaign-tmp…` laissés sous `~/.claude/projects`, à retirer par l'utilisateur quand aucun Claude ne tourne.
- 2026-10-12 : correctif de la courbe accepté (reviewer, simulation sur le vrai code : identique à l'ancienne courbe pour un membre toujours au travail ou toujours au repos ; pour un membre qui alterne, une case peut différer d'un niveau à une transition, la seconde sautée étant attribuée : écart voulu). `fit` en colonnes accepté en ASCII ; `wrap` et `painted` à finir pour les caractères larges (dev-interface).
- 2026-10-12 : **lots C et D clos** (reviewer, seul, charge 8 à 10) : rustfmt --check sur 151 fichiers, clippy, build, tests EN et FR (573 unitaires, mux 8, mux_screen 4, mux_server 9, mux_team 52, 0 échec), site vert. Derniers rendus : `menu/draw.rs` découpé, `wrap` et `painted` en colonnes, `Activity.last` dans l'empreinte de `Panel`. Banc au calme lancé (testeur, en alternance avec la référence de HEAD).
- 2026-10-12 : **banc de l'étape 3 rendu** (testeur, alterné avec la copie de HEAD 0c1cc80, charge 2,4 à 7,5, pics à 17,7) : aucun recul au-delà du bruit. Mémoire par panneau 4,9/6,6/7,3 → 5,3/6,5/7,0 Mio ; repos identique (0,00 %, 0,5 réveil/s, 0 octet) ; flux 1,47 → 1,38 % (un panneau à 20 Ko/s), 405 → 391 % (neuf à fond) ; latence identique (p50 0,37 ms, Échap sans kitty 12 ms) ; tableau de bord au repos 54 → 38 à 40 o/s, seul 21 → 12 o/s (la réécriture des 30 s retirée). D2 gardé (`capture` dans le bruit). Détail : `specs/archive/refacto/testeur.md`. Étape 3 close : CLAUDE.md mis à jour, documents de la construction dans `specs/archive/`, publication en 2.3.0 à la demande de l'utilisateur.

## Suite (après la 2.3.0)

Ouverte le 2026-10-12 à 11 h par l'utilisateur (« ok pour la suite, tu peux enchaîner »), sur les propositions du compte rendu. Mêmes règles que les lots, sauf le mode 2027, qui change l'affichage (accepté par l'utilisateur).

| Qui | Quoi | Fichiers |
|---|---|---|
| dev-saisie, avec dev-rendu | **Mode 2027** : le client garde (ou allume) le mode 2027 du vrai terminal qui le connaît (`Terminal::open` comme `open_with(true)`, puis le paramètre retiré s'il ne sert plus), et le peintre écrit les grappes entières quand il est actif (`Glyph::Cluster`, chemin déjà là) ; sinon, en morceaux comme aujourd'hui. Tests de `input` et du peintre. | `mux/input/terminal.rs`, `mux/client.rs` ; `ui/canvas/painter.rs` si besoin (dev-rendu) |
| testeur | Après le 2027 : U3 et U6 de la grille rejoués. Avant : le banc sans la référence tmux (`tests/mux_bench.rs`, `tests/common/`), et `quick_list_asks_and_refuses` rendu robuste (échec sur la CI macOS de 7536dc9 : la liste ne se rouvre pas après Échap). | `tests/` |
| dev-rendu | **Un seul `wrap`** en colonnes dans `ui/canvas/text.rs`, pour `board/card.rs`, `menu/draw/`, `ui/list.rs`, `ui/print.rs` (`break_words`) ; le comportement de chaque appelant gardé (mot trop long coupé ou fini par « … », nombre de lignes). Les lignes des appelants de `board/` et `menu/draw/` avec l'accord de dev-interface. | `ui/canvas/text.rs` et les appelants |
| dev-interface | **D6** : `menu/effects.rs` (`carry`, `resume`, `compose`, `heard`, `close`, `Composers`, `stop`). | `menu.rs`, `menu/effects.rs` |
| dev-serveur | **M23** : une seule fonction pour le profil de Claude (`profile`, `profile_dir` et la forme de `trusted`). | `claude.rs`, `claude/session.rs`, `claude/args.rs` |
| architecte | `config::deserialize` en une boucle sur les équipes. | `config.rs` |
| reviewer | Relit chaque rendu, puis les vérifications de clôture, seul. | |

Pendant le travail, chacun ne lance que les tests de sa zone (`cargo test <filtre>`) ; la suite complète une fois, avant de rendre. Pas de `cargo fmt` sur tout l'arbre.

Journal de la suite :

- 2026-10-12 : rendus et acceptés par reviewer : `deserialize` en une boucle (architecte ; écart accepté : avec deux erreurs dans deux équipes d'un fichier, la première équipe en défaut parle), D6 `menu/effects.rs` (dev-interface, identique ligne à ligne), M23 `claude::Profile` (dev-serveur, chemins identiques), un seul `wrap` dans `ui/canvas/text.rs` (dev-rendu ; 400 000 textes comparés : card, menu, print identiques hors deux cas pathologiques acceptés, list plus juste avec U+FE0E ; un premier état coûtait 3 à 6 fois plus par carte, corrigé : coût de HEAD à 20–30 % près), banc sans tmux et `quick_list_asks_and_refuses` (testeur : le temps de l'en-tête, aligné à droite, décalait le modèle d'une case entre deux clics).
- 2026-10-12 : mode 2027 (dev-saisie) : le client garde 2027 allumé (réponse 1) ou l'allume (2), rien pour 0, 3, 4 ou sans réponse, et le remet tel qu'il était à toute sortie ; le rapport des capacités écrit après la mise en place. Le moteur garde ses grappes après un RIS (`set_default_mode`, dev-terminal) et sur un `?2027l` d'un programme (`Sniffed::GraphemesOff`). Risque connu, faible, non couvert : `CSI ? 2027 r` (XTRESTORE) sans sauvegarde préalable, et une liste de modes de plus de 64 octets.
- 2026-10-12 : `real_probe` (fenêtres seulement, accord de l'utilisateur pour la journée), campagne 4 de `multiplexeur-compat.md` : 👍🏽 aligné à travers le client dans Ghostty, kitty, WezTerm ; Terminal.app et iTerm2 (pas de 2027 utilisable) et kitty (répond 0) reçoivent 👍 sans la teinte, comme avant. Retouches de sûreté : Ctrl-C remet le profil iTerm2, sondes seulement par construction.
- 2026-10-12 : **suite close** (reviewer, seul, charge 4,5 à 7,4) : rustfmt --check sur les 23 fichiers touchés, clippy, build, tests EN et FR (581 unitaires, mux 8, mux_keys 1, mux_screen 4, mux_server 9, mux_team 52, 0 échec).
