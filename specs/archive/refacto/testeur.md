# Inventaire de testeur

Périmètre : `tests/`, `scripts/` (hors `ghostty.sh`), `.github/workflows/`. Écrit le 2026-10-11, session interrompue par la limite d'usage : l'inventaire est complet, le banc de référence l'est aussi pour 4 mesures sur 4 (la dernière ligne de latence est dans le brut), les mesures du tableau de bord (1)(2)(3) demandées par dev-interface ne sont **pas faites** (voir « Banc »).

Constat général : **aucun test d'intégration ne dépend de l'arborescence de `src/`** (binaire seul, pas de lib ; ils parlent à `recruit` par la ligne de commande, `_ctl`, `_edit`, `_mod`, `_panel`, `_member`, `_server`, et par des fichiers d'état). Seuls 5 commentaires citent `member.rs` ou `src/mux/server/pane.rs`. Aucun test de `tests/` ne touche la 1.x : rien ne part avec elle de ce côté. Les tests qui mentionnent tmux (`mux_team.rs:2519,2716,2724,2726`) vérifient qu'un texte ne parle PAS de tmux : ils restent valides. `mux_claude.rs:88` (`Announced tmux`) est tmux comme terminal extérieur : reste.

## Supprimer
- `scripts/screenshots.sh:98-100` : `RECRUIT_BACKEND=native` (plus lu nulle part dans `src/`, hors la liste SCRUB de `pane.rs:40` à dev-terminal) et son commentaire. Garder `RECRUIT_TMPDIR`. Risque nul ; test : `screenshots_backend_native` (ignoré, bash+jq+python3).
- `tests/mux_team.rs:~1066` (le numéro a glissé de ~26 lignes avec le test de dev-interface) : prélude `backend=native; … SOCKET=x;` : `$backend` et `$SOCKET` ne sont lus par aucun script.
- À trancher (architecte) : `mux_team.rs` `the_dashboard_in_front_of_an_old_server` et `a_team_of_the_2_0_0_passes_to_this_build` (ignorés, `OLD_RECRUIT=<binaire 2.0.0>`, ~70 lignes) : testent le passage depuis la 2.0.0. Mon avis : les garder mais les renommer « version précédente » (ils servent à l'étape 3 avec un binaire 2.2.0 : le protocole ne doit pas bouger) ; sinon les retirer.
- `tests/mux_bench.rs` : la ligne de base tmux (`tmux_memory`, `Setup::Tmux`, `common::tmux*`, `has_tmux`, `TMUX_SOCKET`, ~250 lignes) : **à garder jusqu'à l'étape 3** (elle absorbe la charge de la machine : le « ajouté » se compare au même run), à retirer ensuite si l'utilisateur veut.

## Simplifier
- `child()` copié 8 fois (un par fichier de test, 9 lignes chacun) : un macro `common::child_test!()` (`#[macro_use] mod common;`). Gain ~70 lignes, risque faible.
- Doublons entre `mux_team.rs` et `mux_gallery.rs` (identiques ou presque) : `alt`, `find`, `find_in_header`, `click`, `hover`, `says`, `report` (celle de team garde la clé `compaction` : à prendre), `close_menu`, `base64` (3 copies, dont un décodeur à part dans `mux_claude.rs`), `fails`, `now_secs` (9 `SystemTime::now()…as_secs()`). Aussi `thick` (`mux_screen` / `mux_team`), `shown` (3 formats : `mux_screen` numérote, `mux_server` ajoute la fin brute, `mux_team` joint), `wait` (`mux_screen.rs:53` = `common::team::wait`), `find` de `mux_claude.rs:495` qui rend (ligne, colonne) au lieu de (colonne, ligne) : piège. Cible : `tests/common/client.rs` (helpers d'écran et de souris), méthodes `TestTerm::{click, hover, alt}`. Gain ~150 lignes.
- `std::fs::copy(env!("CARGO_BIN_EXE_recruit"))` à la main (`mux_team.rs:3100,3226`) → `common::recruit_copy`.
- `mux_team.rs` `Team::bash` et le script `shots-backend.sh` : voir « Renommer ».

## Découper
- `tests/mux_team.rs` (3 517 lignes, 59 tests, 8 ignorés) : `tests/mux_team/main.rs` + modules (`#[path = "../common/mod.rs"] mod common;` : sans le `#[path]`, `mod common` cherche `tests/mux_team/common`). Nom de test inchangé (`cargo test --test mux_team`). Découpe : `launch` (tabs, small team, stopped member, ctrl-c, hot edits, dry run, coming back ×2, reprise, passages 2.0.0), `chrome` (quit choice, boutons, alt+g, zoom, header ×2, resize ×3, narrow bar/pane, notice, hover, alt+n, clics du journal et du tableau de bord), `quick` (listes rapides, l. ~1541-2030), `compaction` (auto ×2, held ×2), `focus`, `osc` (état survivant au clear, thème, panneau sans état), `dashboard` (coût : `claude agents` rare, écrit peu, capture), `failed_turn` (l. ~3249 à la fin), `menu` (lecture seule, /recruit, script de captures). 
- `tests/mux_claude.rs` (1 380, tout ignoré, vraies sessions) : même forme possible (`modes`, `mouse`, `header`, `compaction`, `report`), moins utile ; `mux_gallery.rs` (1 057, à la main) et `mux_keys.rs` (1 047) : laisser.
- Le sujet de coordination : dev-interface et d'autres ajoutent des tests à `mux_team.rs` pendant les lots A : la découpe se fait au lot C, en prévenant chacun avant.

## Renommer
- `scripts/shots-backend.sh` → `scripts/shots-ctl.sh` (`git mv`) ; références : `scripts/screenshots.sh:25,103,104`, `tests/mux_team.rs:1062,1070`, `CLAUDE.md:38`, `specs/multiplexeur-compat.md:388`. Le test `screenshots_backend_native` → `screenshots_script_asks_of_the_multiplexer`.
- Commentaires caducs : `tests/common/pty.rs:364` (`tests/gallery.py` → `tests/tools/gallery.py`) ; `pty.rs:4` (« tmux client » ?) ; `tests/common/mod.rs` en-tête et `child_words` (« for tmux » : il sert aussi au banc tmux, ok tant que celui-ci reste) ; `tests/common/server.rs:260-264` (« under tmux too ») ; `mux_claude.rs:97` (« under tmux and natively » → « in the multiplexer ») ; `// Owner: testeur.` ×12 dans `tests/` ; « (step 1) » dans `mux_server.rs:3` et `common/server.rs:3`, « step 0's measures » dans `mux_bench.rs:214,526,681`.
- `specs/multiplexeur-compat.md:18,183,360` : `RECRUIT_BACKEND=native` à retirer ; `:26` « Dans un tmux » reste (colonne terminal) ; les campagnes passées gardent leurs comparaisons à tmux (historique).

## Déplacer
- Rien dans `tests/`. Avis sur l'arborescence cible : sans effet sur les tests ; au lot B, seuls les 5 commentaires ci-dessus changent. Le script `scripts/release.sh:211` lance `cargo test` dans une seule langue (la CI fait les deux) : voir « Hors refacto ».

## Hors refacto
- `recruit list --json` porte encore `"tmux": legacy(f)` (`app.rs:283`) : champ de sortie publique qui disparaît avec la 1.x ; aucun test ne le fixe ; la doc du site (`reference/commands.md`) en parle sans doute.
- Tests vus en échec une fois sous charge (note de l'architecte) : `mux_team.rs` `quick_list_asks_and_refuses`, `unreadable_files_open_a_read_only_menu`, `mux::socket::tests::a_stop_on_purpose_leaves_no_mark` : attentes courtes (5-8 s) quand plusieurs `cargo test` tournent ; à regarder au lot C (`wait` de `common::team`, délais en tête).
- Le banc a tourné avec une charge de 5 à 11 sur 10 cœurs (l'équipe elle-même) : voir les « ajouté » plutôt que les valeurs brutes.
- `release.sh:211` : `cargo test` dans la langue du système seulement.
- `.github/workflows/` : RAS (ni tmux ni backend ; `ci.yml` check + musl, `pages.yml` site). Rien à changer.

## Banc de référence (HEAD 0c1cc80, release sans LTO, copie du binaire dans /tmp/rtbench.Nggu, brut dans `specs/refacto/testeur-banc-brut.txt`)
Reproduire : `CARGO_PROFILE_RELEASE_LTO=off cargo test --release --test mux_bench --no-run`, copier `target/release/recruit` et le binaire de test, `RECRUIT_BIN=<copie> <test> <nom> --exact --ignored --nocapture --test-threads=1` pour `memory_per_pane`, `memory_through_mux`, `cpu_rest_and_flood`, `latency_added`. Charge de la machine pendant les mesures : 5 à 11 (10 cœurs).

- **Mémoire par panneau, 100 000 lignes** (`memory_through_mux`, empreinte, lignes synthétiques) : 80 col. 5,4 Mio ; 120 col. 6,8 Mio ; 200 col. 7,5 Mio ; serveur avec un panneau vide 6,0 / 6,2 / 7,2 Mio. (alacritty et tmux dans le brut.)
- **Repos** (`cpu_rest_and_flood`) : 10 factices au repos 0,00 % CPU, 0,5 réveil/s, 0 octet ; 9 historiques pleins au repos : idem.
- **Flux** : 1 panneau à 20 Ko/s 2,10 % (53,7 images/s) ; 9 panneaux à 20 Ko/s 4,75 % (61,3/s) ; 1 panneau à fond 74 % (67 Mio/s lus) ; 9 à fond 242 % (120 Mio/s), 43,5 images/s.
- **Latence ajoutée** (frappe `x`, terminal kitty, 500 frappes, « ajouté » = écart avec le terminal seul) : 1 panneau p50 +0,21 ms / p99 +1,80 ms ; 10 panneaux dont 9 en flux p50 +1,07 / p99 +4,36 ms (serveur à 402 % CPU) ; 9 en images de 120 Ko p50 +0,24 / p99 +4,85 ms. Échap seul, kitty : p50 +0,23 / p99 +0,51 ms (1 panneau), +1,05 / +3,18 (flux), +0,15 / +0,60 (images). **Échap sans kitty (legacy)** : p50 +12 ms, p99 +111 ms (1 panneau) ; +14 / +842 ms avec 9 panneaux en flux : c'est le délai de lecture d'Échap (`parse.rs`), le même qu'à la campagne précédente, à ne pas faire bouger. tmux retient Échap ~500 ms dans ce montage.
- **Non fait** : la ligne « 2 vrais Claude au repos » (verrou `CLAUDE_SESSIONS`, pas de go) ; les mesures du tableau de bord demandées par dev-interface (octets et CPU du tableau de bord, 9 et 12 membres, 2 min au repos ; focus 12 membres). Le script est écrit (`meas.py` dans le dossier de travail de la session, non commité : une équipe de fakes lancée avec la copie release, `_ctl stats`, `ps`) mais **pas lancé ni validé** ; je le rejoue à la reprise, sur la copie /tmp/rtbench.Nggu (HEAD 0c1cc80, donc un vrai « avant »).

## Estimation
- Lot A (ma part) : ~30 min (`screenshots.sh`, `git mv` + 6 références, 5 commentaires, prélude de test).
- Lot C : ~1 h 30 (helpers communs et `child_test!`, découpe de `mux_team.rs`, ~250 lignes de moins en tout, hors tmux du banc). Dépend de : personne n'ajoute plus de test à `mux_team.rs` pendant la découpe.
- Étape 3 : banc rejoué (~25 min) + grille (campagne courte) : ~1 h.

## Mesures du tableau de bord (HEAD 0c1cc80, copie release /tmp/rtbench.Nggu, 2026-10-11 ~23 h 20)
Équipe de faux claude lancée en détaché, `_ctl stats` (octets lus par le panneau) et `ps` toutes les 5 s ; brut dans `testeur-banc-tableau-brut.txt`, script dans `testeur-meas.py` (`python3 -I testeur-meas.py <membres> <working|idle> <secondes> [grid|focus] [client]`, adapter `B`). Charge de la machine à chaque run : 3,9 à 10,7 (notée dans le brut). **Sans client attaché** le panneau a la taille par défaut du serveur : les octets valent pour cette taille, pas pour 200×50.

| Montage | Tableau de bord : octets/s | plus gros intervalle de 5 s | CPU tableau de bord | CPU journal | CPU serveur |
|---|---|---|---|---|---|
| 9 membres au travail, 60 s | 487 | 2 862 | 0,20 % | 0,03 % | 0,08 % |
| 9 membres au repos, 60 s | 55 | 709 | 0,13 % | 0,03 % | 0,03 % |
| 12 membres au travail, 60 s | 483 | 2 808 | 0,20 % | 0,03 % | 0,08 % |
| 12 membres au repos, 60 s | 55 | 709 | 0,13 % | 0,03 % | 0,05 % |
| 1 membre au repos, 120 s (tableau de bord seul) | 21 | 388 | 0,10 % | 0,02 % | 0,04 % |
| 12 membres au travail, focus, client 200×50 attaché, 60 s | 1 538 (client : 2 510) | 9 237 | 0,31 % | 0,05 % | 0,35 % |

- Redessin complet (`WHOLE_EVERY` 30 s) : visible au repos comme deux intervalles à ~700 B au lieu de ~200 B (à 25 s et 55 s), soit **~500 B tous les 30 s ≈ 17 B/s** à la taille par défaut ; le fond du tableau de bord au repos est ~40 B/s. Coût CPU du redessin : sous le bruit de `ps` (10 ms de granularité). Avec un client à 200×50, 9 237 B dans le pire intervalle (cartes en mouvement, pas seulement le redessin).
- Les lignes 9 et 12 membres sont presque identiques : le panneau par défaut ne montre pas plus de cartes ; pour un chiffre à 12 cartes visibles il faut un client attaché (dernière ligne).
- **Focus, `card_lines` des cartes « autres »** : proxy seulement (dernière ligne) ; pas de mesure du temps de `card_lines` seul.

## D2 : `Ghostty::capture` découpée (draw_costs, avant / après, 2026-10-12)
`cargo test --release draw_costs -- --ignored --nocapture` (sans LTO) sur deux copies de l'arbre (avant D2 : D1+D3 faits ; après D2 : `engine/ghostty/frame.rs`), 21 passes **alternées** (avant, après, après, avant…) pour que la charge pèse pareil sur les deux. Charge machine (moyenne à 1 min, à chaque tour) : 14,0 à 16,8, donc bruit élevé. Médiane [min–max], µs. Script : `dc2.py` du dossier de travail, non commité.

```
load 1-min at each round: min 14.0 max 16.8
a line a frame
  colors          | before median    0 [0-0] p25-75 0-0 | after median    0 [0-0] p25-75 0-0
  capture         | before median  112 [105-480] p25-75 109-118 | after median  116 [110-324] p25-75 112-125
  paint           | before median   52 [48-69] p25-75 50-56 | after median   53 [49-219] p25-75 50-55
  Painter::frame  | before median   44 [41-122] p25-75 41-49 | after median   44 [41-323] p25-75 41-47
one cell a frame
  colors          | before median    0 [0-48] p25-75 0-0 | after median    0 [0-1] p25-75 0-0
  capture         | before median    5 [4-7] p25-75 4-5 | after median    5 [4-19] p25-75 5-5
  paint           | before median   52 [48-213] p25-75 49-53 | after median   51 [48-139] p25-75 49-59
  Painter::frame  | before median   37 [33-135] p25-75 35-39 | after median   37 [33-75] p25-75 34-45
```

Lecture : `capture` d'une ligne par image 112 → 116 µs (+4 %, les écarts interquartiles se touchent : à la limite du bruit) ; tout le reste identique. Le mux_bench complet n'a pas été lancé (consigne de l'architecte : la décision vient du banc de l'étape 3, machine calme).

## Étape 3 : HEAD 0c1cc80 contre l'arbre de la refacto (2026-10-12, 1 h 55 à 4 h 05)
Aucun chiffre ne recule au-delà du bruit. Méthode : release sans LTO comme la référence, copie du binaire (`/tmp/rtbench.sMc2`, arbre de 1 h 53) ; chaque test du banc lancé **en alternance** avec la copie de HEAD (`/tmp/rtbench.Nggu`, même machine, même heure) pour que la charge pèse pareil. Charge de la machine (1 min) : 2,4 à 7,5 en général, pics à 13,6 (2 h 17) et 17,7 (2 h 53) quand l'autre équipe compilait. Un pic a faussé une passe (voir la latence), rejouée à froid. L'orphelin Python (PID 82426, le mien, un cœur plein depuis 23 h 30) a été arrêté avant de commencer.

| Mesure | HEAD 0c1cc80 | Arbre de la refacto | Verdict |
|---|---|---|---|
| Mémoire par panneau, 100 000 lignes, 80 / 120 / 200 col. (`memory_through_mux`) | 4,9 / 6,6 / 7,3 Mio (1re référence : 5,4 / 6,8 / 7,5) | 5,3 / 6,5 / 7,0 Mio | bruit (±0,5) |
| Panneau vide, serveur | 6,0 / 6,2 / 7,3 Mio | 6,0 / 6,3 / 7,3 Mio | identique |
| Mémoire par panneau, moteur seul (`memory_per_pane`) | alacritty et tmux seulement : inchangés | idem | — |
| Repos : 10 factices, 9 historiques pleins, 60 s | 0,00 %, 0,5 réveil/s, 0 octet | 0,00 %, 0,5 réveil/s, 0 octet | identique |
| 1 panneau à 20 Ko/s | 1,47 % (53,1 images/s) | 1,38 % (47,0) | bruit |
| 9 panneaux à 20 Ko/s | 3,39 % (61,3) | 3,36 % (60,2) | identique |
| 1 panneau à fond | 84,4 % (100 Mio/s) | 83,6 % (102 Mio/s) | identique |
| 9 panneaux à fond | 405 % (158 Mio/s) | 391 % (158 Mio/s) | identique (le même binaire a donné 242 % et 74 % ailleurs sous une autre charge) |
| Latence, `x` kitty, 1 panneau, p50 / p99 (absolu, 2 passes) | 0,36–0,38 / 1,01–1,16 ms | 0,36–0,38 / 1,02–1,19 ms | identique |
| Latence, `x`, 10 panneaux dont 9 en flux, p50 / p99 | 1,05–1,16 / 1,8–4,5 ms (14,8 et 26,8 ms sur deux passes chargées) | 1,15–1,20 / 1,8–5,9 ms (58 ms sur la passe chargée) | identique |
| Latence, `x`, 9 en images de 120 Ko, p50 / p99 | 0,29–0,30 / 0,82–1,07 ms | 0,28–0,31 / 0,75–0,94 ms | identique |
| Échap kitty, 1 panneau, p50 / p99 | 0,36–0,37 / 0,9–1,2 ms | 0,36–0,37 / 0,9–1,2 ms | identique |
| Échap **sans kitty** (attente voulue), 1 panneau / 9 en flux / 120 Ko, p50 | 12,4 / 13,0 / 12,3 ms | 12,4 / 13,0 / 12,3 ms (p99 12,9–13,3 / 13,6–13,8 / 12,8–13,1) | identique (tmux : ~500 ms) |
| `draw_costs` « une ligne par image » : capture / paint / Painter::frame | D2 après (charge 14–17) : 116 / 53 / 44 µs | 106 [103–111] / 48 / 40 µs (21 passes, charge 2,4) | pas de recul |
| `draw_costs` « une case par image » : capture / paint / Painter::frame | D2 après : 5 / 51 / 37 µs | 4 / 48 / 32 µs | pas de recul |
| Tableau de bord, 9 membres au travail : octets/s, CPU tableau / serveur | 472 B/s, 0,25–0,27 % / 0,10–0,12 % (alterné) | 458 B/s, 0,25 % / 0,10–0,12 % | octets −3 %, CPU identique |
| 9 ou 12 membres au repos | 54–55 B/s, 0,15–0,17 % | 38–40 B/s, 0,17–0,18 % | octets −30 %, CPU identique |
| 12 membres au travail | 483 B/s (première mesure) | 455 B/s, 0,25 % / 0,10 % | idem |
| Tableau de bord seul, 1 membre au repos, 120 s | 21 B/s, 0,10 % (première mesure) | 12 B/s, 0,12 % | octets en baisse |
| Focus, 12 au travail, client 200×50 | 1 538 B/s (client 2 510), 0,31 % / serveur 0,35 % | 1 473 B/s (client 2 493), 0,35 % / serveur 0,38 % | identique (CPU dans la résolution de `ps`, ±0,05) |

- **Latence : passe fausse, rejouée.** La première passe de la copie (2 h 51 à 3 h 25) a donné, sur « 9 en images de 120 Ko », `x` p95 27 ms, p99 +52 ms, et sur « 9 en flux » p99 +58 ms : elle tombait sur le pic de charge à 17,7 (2 h 53, compilation de l'autre équipe) ; les mêmes lignes rejouées trois fois en alternance avec HEAD, à froid : p99 0,75–0,94 ms (120 Ko) et 1,8–5,9 ms (flux) pour la copie, 0,82–1,07 et 1,8–4,5 ms pour HEAD (une passe de HEAD à 26,8 ms sous charge 7). Les p99 du flux sont dominés par ces pics et ne se comparent qu'en alternance.
- **Tableau de bord, CPU.** La 1re passe de la copie donnait 0,28 % contre 0,20 % de la première référence (+0,08) ; en alternance, à charge égale, 0,25 % contre 0,25–0,27 % : l'écart venait de l'heure de mesure, pas de l'arbre. Les octets baissent : plus de redessin complet visible au repos (plus gros intervalle 203 B contre 709 B).
- **D2 (`Ghostty::capture` découpée) : dans le bruit, sans recul.** Avant / après alternés (21 passes, charge 14–17) : capture 112 → 116 µs, paint 52 → 53, Painter::frame 44 → 44 ; à froid sur l'arbre final : capture 106 µs [103–111], paint 48, Painter::frame 40. Pas de HEAD de référence à froid pour `draw_costs` (le test n'existe pas à HEAD), donc pas de pourcentage à froid ; le `#[inline]` sur `read_row` n'est pas nécessaire.
- **Non fait** : « 2 vrais Claude au repos » (pas de go pour ce banc : la campagne 3 du 2026-10-12 est dans `multiplexeur-compat.md`) ; la baseline tmux du banc reste en place.
