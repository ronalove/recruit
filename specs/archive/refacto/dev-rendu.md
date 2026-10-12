# Inventaire de dev-rendu

Part : `canvas.rs` (1672 lignes, dont 415 de tests), `mux/screen.rs` (1353, dont 993 de tests), `mux/select.rs` (536), `mux/focus.rs` (642), `layout.rs` (524), `ui.rs` (254), `ui/list.rs` (1117). Lu avec `graft skeleton` et `graft callers`/grep ; rien compilé. Je lis aussi `look.rs` pour l'avis d'arborescence, sans rien proposer de plus sur son contenu (c'est à dev-interface).

Constat général : ma zone est saine. Presque rien n'y est mort, parce que tmux et la 1.x n'y avaient laissé que de la géométrie (`gap`) et des commentaires. Le gros du gain est dans les découpages et dans un trait `Engine` trop large pour ce que `Screen` en consomme.

## Chemin du dessin : ce qui est touché

Rien de ce qui suit ne change un octet envoyé ni un algorithme. À vérifier au banc de l'étape 3 quand même :
- D1. Découpage de `canvas.rs` (Canvas, Painter, texte, Session) : déplacement pur, `Painter` en sous-module de `Canvas` pour garder l'accès aux champs privés. Release en `lto = true`, `codegen-units = 1` : un déplacement entre modules ne change pas l'inlining ; seul le build debug (tests, banc de dev) peut différer.
- D2. `Canvas::fill/tint/invert/frame…(x, y, w, h)` prenant un `Rect` (voir « Déplacer », R1) : signature seulement, appels inlinés ; à ne faire que si `Rect` bouge.
- D3. Scinder `Engine` en deux traits (voir « Simplifier », M1) : `View.engine` reste un `&dyn`, même appel virtuel qu'aujourd'hui. Aucun coût nouveau.
- Pas touché, et à ne pas toucher : `Screen::compose` (garde par panneau, `generation`), `Painter::frame`/`cells`, `Canvas::put_cell`/`compact`/`catch_up`.

## Supprimer

1. `layout.rs:127` `tree(.., gap)`, `layout.rs:241` `shares(.., gap)` (branche `gap > 0`, l. 245-255), `layout.rs:274` `Node::place(.., gap)` : `gap` vaut toujours 0 (`rects` l. 120 passe 0, `focus_rects` appelle `shares(.., 0)`). Reste d'un tmux qui mettait une ligne entre deux panneaux. Retirer le paramètre et la branche (~20 lignes) ; `at += size + gap` devient `at += size`. Risque faible (géométrie pure). Tests : `native_rects_tile_the_area`, `focus_rects_tile_the_area`, `grid_shapes`, tests de `server.rs` qui lisent les rectangles.
2. `mux/screen.rs:80-103` `compose` libre (`#[cfg(test)]`, 24 lignes) : c'est `Screen::default().compose(..)` (état neuf : `same = false`, tout est dessiné, par les mêmes `draw` et `finish`). Les tests `same_as_whole*` comparent un `Screen` gardé à une image neuve : le remplacer par un `Screen` neuf garde la même comparaison. Gain : le doublon de l'en-tête, une signature à 9 arguments de moins. Risque faible, tests seulement (`as_whole`, `same_as_whole_with`, `panes_in_their_frames`…).
3. `canvas.rs:1350` et `mux/screen.rs:933` : le même `struct Rng` xorshift écrit deux fois dans des tests (plus un LCG dans `tests/mux_bench.rs:270`). Un seul, dans un module de test partagé (`#[cfg(test)] pub(crate) mod rng` dans `ui/` ou `tests/common` pour le banc). Risque nul, tests seulement.
4. Rien dans `ui/list.rs`, `ui.rs`, `select.rs`, `focus.rs` : tout y sert. Dans `ui/list.rs` les pièges écrits pour tmux (l. 454-458 : la ligne effacée seule, puis jusqu'en bas) **restent vrais** : ils ne tiennent pas à tmux seul. (a) Un terminal qui réarrange les lignes (reflow) à l'élargissement recolle les lignes repliées, `rows_up` (l. 489) et `Clear(CurrentLine)` couvrent tous les terminaux qui le font (Ghostty, iTerm2, kitty, tmux) ; (b) `Clear(FromCursorDown)` depuis le coin haut-gauche pousserait tout l'écran dans l'historique d'un tmux **de l'utilisateur** dans lequel `recruit` tourne (vérifié 3.7c). Donc : le code ne bouge pas, seuls les commentaires se reformulent (voir « Renommer », N2). La liste (`ui/list.rs`) tourne dans le terminal du client avant tout multiplexeur, donc dans le tmux éventuel de l'utilisateur : le piège reste vrai.

## Simplifier

M1. **Scinder `Engine` en deux traits** (`mux/engine.rs:27`, dev-terminal ; `mux/cards.rs:136`, `mux/screen.rs`, `mux/select.rs`). `Screen` et `Selection` ne consomment que `draw`, `generation`, `cursor`, `history`, `scroll`, `scrolled`, `top`, `oldest`, `line`, `modes` ; `feed`, `deadline`, `expire`, `relays`, `resize`, `set_colors`, `color_scheme` sont le côté « terminal alimenté par un PTY ». Aujourd'hui, `Panel` (cartes), `Lines`/`Fill`/`Counted` (tests de `screen.rs`) et `Fake` (tests de `select.rs`) écrivent chacun ~12 méthodes dont 7 ne font rien : 5 implémentations, ~100 lignes de bruit. Avec un trait `Surface` (affichage et histoire) et `Engine: Surface` (le reste), `View.engine` devient `&dyn Surface`, `Panel` et les fakes n'implémentent que `Surface`. Dépend de dev-terminal (le trait), dev-serveur (`server.rs` donne `engine.as_ref()` aux `View`), dev-interface (`cards.rs`). Risque moyen (signature d'un trait qui traverse `server.rs`), mécanique. Tests : tous ceux de `screen`, `select`, `cards`, `server`. À faire dans le lot C après le lot B.
M2. `mux/screen.rs:82,251` `compose` (10 arguments, `#[allow(clippy::too_many_arguments)]`) et `finish` (8) : `team, tabs, bar, hover, look, over, notice` voyagent toujours ensemble (`Bar` en regroupe déjà quatre). Un `Scene<'a>` (ou `Frame`) les porte : un seul argument en plus des `views`, deux `allow` retirés. Touche l'appel de `server.rs:2913` (dev-serveur). Risque faible, tests : `screen`, `server`.
M3. Trois coupes aux mots, quatre avec celle de `ui.rs` : `board.rs:2201` `wrap` (compte les `char`, coupe un mot trop long), `menu/draw.rs:1159` `wrap` (compte les colonnes, ne coupe pas un mot trop long, `fit` après), `ui.rs:186` `break_words` (`UnicodeWidthStr`, une rangée minimum, pas de limite de lignes), `ui/list.rs:406` `wrap` (morceaux colorés). Leurs comportements diffèrent aux marges (mot plus long que la ligne, caractères larges) : les réunir en une `ui::text::wrap(text, width, lines)` (colonnes de `Canvas`) change ce que voit l'utilisateur dans le cas du mot trop long du tableau de bord. Pas de fusion sans choix explicite : proposer à dev-interface de fusionner `board::wrap` et `menu::wrap` en une (même contrat, à trancher), et de laisser `break_words` et `list::wrap` à leur place. Tests : `board`, `menu`.
M4. `canvas.rs:9` et `ui/list.rs` ont chacun leur ouverture de terminal (`Session::open`/`leave` : raw, écran alterné, souris, collage ; `list::Screen::open`/`restore` : raw, curseur caché, collage, stderr, sans écran alterné) et `board.rs:100-200` un troisième (`QuietInput`, `leave` en `libc::write`). Seul `PanicGuard` est commun et déjà partagé. Les trois diffèrent assez (flux, modes, signaux) pour que les fusionner coûte plus que ça ne rend : **ne pas les fusionner**. Seul gain : `Session`, `PanicGuard`, `leave`, `MOUSE_*` sortent de `canvas.rs` vers un `ui/terminal.rs` (voir « Découper »).
M5. `mux/screen.rs:332-342` `overlap` et `within` : deux méthodes de `Rect` (`Rect::overlaps`, `Rect::within`) à côté de `Rect::contains` (`mux/mod.rs:73`, dev-terminal). Mineur, à faire avec R1.
M6. `ui.rs:87-132` `confirm`/`text` par inquire pendant que les listes sont dessinées par `ui/list.rs` : deux mécanismes d'invite. Ne pas y toucher dans ce chantier (changerait ce qu'on voit), mais c'est la seule raison pour laquelle `inquire` reste dans `Cargo.toml` (architecte) : si l'utilisateur veut un jour retirer la dépendance, c'est un travail de fonctionnalité (éditeur de ligne), pas de refacto. Noté pour l'architecte, rien à faire ici.

## Découper

C1. **`canvas.rs` (1672 lignes)** → `ui/canvas.rs` + sous-modules, déplacement pur, un seul `git mv` d'abord, les `mod` ensuite :
   - `ui/canvas.rs` (l. 19-208, 273-700, ~560 lignes) : `Style`, `LinkId`, `Cursor`, `CursorShape`, `Features`, `Cell`, `Glyph`, `Wide`, `Canvas` et ses méthodes (écriture, `put_cell`, `compact`, `catch_up`, `row`…).
   - `ui/canvas/text.rs` (l. 207-270, ~70 lignes) : `columns`, `fit`, `width`, `detached`, `bidi`, `left_out` — la mesure du texte, ce qu'importent `chrome`, `menu`, `board`, `ghostty`.
   - `ui/canvas/painter.rs` (l. 712-1155, ~450 lignes) : `Painter`, `Params`, `color`, `ansi256`, `narrowed`, `GAP`, `ERASE`, `URI`. Sous-module de `canvas` pour voir les champs privés de `Canvas`, `Cell`, `Glyph`.
   - `ui/terminal.rs` (l. 1157-1253, ~100 lignes) : `Session`, `PanicGuard`, `leave`, `MOUSE_ON/OFF` ; ne dépend pas de `Canvas`. `menu.rs`, `board.rs` et `ui/list.rs` l'importent.
   - Tests (l. 1256-1672, 415 lignes) : `ui/canvas/tests.rs` pour le `Canvas`, ceux du `Painter` et du `Session` dans leurs fichiers. Dès la règle des 1500 lignes ; `pane_row`, `text` en helpers du module de test.
   Chemin du dessin : déplacement seulement. Tests : `canvas::tests` entiers, plus `chrome`, `cards`, `screen`, `board`, `menu`, `ghostty` qui lisent `Canvas`.
C2. `mux/screen.rs` (1353) : le code fait 358 lignes, les tests 993 (dont trois faux moteurs, retirés en grande partie par M1). Sous la règle de 1500, mais je propose `mux/screen/tests.rs` : la lecture du module en dépend. Optionnel, au lot C.
C3. `ui.rs` (254) : mélange trois métiers : erreurs et invites (`Cancelled`, `select`, `multi_select`, `confirm`, `text`, `required`, `optional`), affichage de l'équipe (`print_team`, `team_lines`, `print_paragraph`, `break_words`, `terminal_width`), et le sous-module `list`. Si `ui/` devient la boîte à outils d'affichage (voir « Déplacer », R2) : `ui/prompt.rs` (invites, avec `prompt/list.rs` pour la liste) et `ui/print.rs` (`print_team`, `print_paragraph`). `app.rs` et `wizard.rs` écrivent `ui::select`, `ui::confirm` : des `pub use` dans `ui.rs` évitent de toucher 60 appels. Sinon, laisser `ui.rs` tel quel : ce n'est pas gros.
C4. `ui/list.rs` (1117, code 613) : cohérent, pas de découpage utile. Si l'on veut : `fold` et `score` (filtre flou, ~50 lignes) en `list/filter.rs`. Je ne le propose pas.
C5. `layout.rs` (524) : à scinder en deux, voir R3. Pas de découpage plus fin.
C6. `mux/select.rs` (536), `mux/focus.rs` (642) : rien.
C7. Grosses fonctions : `Painter::cells` (l. 829-932, ~100 lignes) et `Canvas::compact` (35) sont longues mais d'un seul tenant, sur le chemin du dessin : pas touchées. `Screen::compose` (68 lignes) : clair, la garde `keep` tient en un bloc ; pas touchée.

## Renommer

N1. `canvas.rs:8` « (tmux drops them in a popup) » : le menu n'est plus dans une popup tmux mais dans un panneau flottant du multiplexeur. Le principe (une image, un `write`) reste ; retirer la parenthèse. Idem `canvas.rs:1133` `ansi256` « (as tmux does) » : dire « (the xterm way) » ou rien.
N2. `ui/list.rs:454-458` : « the mark tmux leaves on a row it broke in two » / « tmux's history (scroll-on-clear) » : reformuler en « a terminal that reflows its lines » (a) et « a multiplexer around recruit, tmux for one (scroll-on-clear) » (b), voir « Supprimer » 4. Le piège de CLAUDE.md (« Les listes de choix… ») se garde, avec « tmux » ramené à « un tmux de l'utilisateur » (reviewer, étape 3).
N3. En-têtes caducs : `mux/screen.rs:3` (« spec §5.3 », « direction B « Cadres » ») et `:7` « Owner: dev-rendu. » ; `mux/select.rs:3` (« spec §5.5 ») et `:12` « Owner: dev-rendu. » ; `mux/screen.rs:17,230` (« direction « Cadres » », « spec §2 »). Les noms de membres et les références à la spec ne servent plus après le chantier : retirer « Owner » et les « spec § ».
N4. Dates de décision et maquettes dans les commentaires : `mux/focus.rs:3` (artifact, « decision of 2026-10-11 »), `:199`, `:340` (« the maquette ») ; `layout.rs:113` (« CLAUDE.md »), `:170`, `:181`, `:194`, `:205` (artifact, « maquette », « decision of »), `:470`. Reformuler en ce que fait le code, sans lien ni date (CLAUDE.md garde l'historique).
N5. `layout.rs:3` : le doc dit « and, for a running team whose members changed, how its panes move to their new tabs » : cela vit dans `live.rs` depuis longtemps. Corriger.
N6. `layout.rs:11-125` : `pub` partout (`Tab`, `columns`, `tabs`, `reserved_title`, `column_heights`, `SidePanes`, `SIDE_PERCENT`) alors que le crate est un binaire ; `column_heights` n'est appelé que dans le fichier, `SIDE_PERCENT` aussi. Passer en `pub(crate)` ou privé dans le code déplacé (règle de visibilité du plan).
N7. `ui/list.rs:540` `struct Screen` : même nom que `mux::screen::Screen`, et fait la même chose que `canvas::Session` en plus petit. `Inline` ou `Prompt` dirait mieux ce que c'est (un cadre redessiné sur place, sans écran alterné). Purement local au fichier.
N8. `canvas.rs:60-66` `Features::default` : « what the menu has always assumed » : seuls les tests s'en servent tels quels (menu et tableau de bord font `sync: true`) ; reformuler « no synchronized updates, no links, 24 bits ».
N9. `mux/focus.rs` : nom exact mais deux notions de « focus » dans le code (le panneau qui a le focus clavier, et la disposition « focus » de la config `Layout::Focus`). Un doc d'ouverture qui dit « the focus layout » lèverait l'ambiguïté ; pas de renommage de fichier.

## Déplacer

R1. `Rect` (`mux/mod.rs:65`) : `layout.rs` (hors `mux`), `chrome`, `screen`, `cards` l'utilisent, et `Canvas` prend toujours `(x, y, width, height)` au lieu d'un `Rect` (`fill`, `tint`, `invert`, `frame`, `hline`…). Je propose `ui::Rect` à côté de `Canvas`, avec `contains`, `overlaps`, `within` (M5), `mux::Rect` en `pub use`. Dépend de dev-terminal (`mux/mod.rs`) et de tous ceux qui l'importent ; à faire dans le lot B de l'architecte (un `use` de plus à chaque endroit). Les signatures `Canvas::fill(Rect, ..)` (D2) seraient du lot C, par module, si l'architecte le veut.
R2. `canvas.rs` et `look.rs` dans `ui/` : oui, pour les deux. `Canvas`/`Painter` ne sont pas propres au hors-multiplexeur (le moteur, `chrome`, `screen`, `cards`, `board` et `menu` en vivent) mais c'est bien la boîte à outils d'affichage, et `mux` en dépendant n'a rien d'étrange : `ui` ne dépend de rien dans `mux`. `look.rs` (palettes, signes, états) est utilisé par `board`, `menu`, `mux::chrome`, `mux::cards` : même place. Alors `ui` = la boîte à outils : `ui/canvas.rs` (+ `canvas/`), `ui/terminal.rs`, `ui/look.rs`, `ui/prompt.rs` + `ui/prompt/list.rs` (ce que `ui.rs` et `ui/list.rs` font aujourd'hui), `ui/print.rs`. La table de l'architecte met les invites à la racine de `ui` et `canvas.rs`/`look.rs` en voisins : même résultat, avec des fichiers plus petits ; c'est mon seul amendement (invites en `ui/prompt`, pas en `ui.rs`).
R3. `layout.rs` mélange deux métiers : (a) la répartition en onglets du fichier d'équipe (`Tab`, `columns`, `per_tab`, `tabs`, `reserved_title`, `own_tab`, `split` : lit `Team`, pas de géométrie) ; (b) la géométrie d'un écran de multiplexeur (`rects`, `tree`, `column_heights`, `SidePanes`, `SIDE_PERCENT`, `focus_room`, `focus_rects`, `focus_cards_rows`, `shares`, `Node`, constantes `FOCUS_*` : lit `Rect`, jamais `Team`). (a) va à `team/layout.rs` (avec `launch`, `live`) ; (b) va à `mux/layout.rs`, à côté de `mux/focus.rs` qui en consomme `focus_room` et `focus_rects`. `config.rs:573` appelle `layout::reserved_title` : une dépendance de `config` vers `team` à l'envers de la table ; la fonction (14 lignes, texte seulement) est mieux dans `config` (reviewer) avec sa règle de nom. Les tests se partagent de même (a : 7 tests ; b : 6). Callers à suivre : `config.rs`, `config/edit.rs`, `menu.rs`, `launch.rs`, `live.rs`, `templates.rs`, `backend.rs`, `mux/native.rs`, `mux/server.rs`, `mux/focus.rs`.
R4. `mux/select.rs:93` `scroll_key` (Shift+PageUp/PageDown/Home/End vers un défilement) n'a rien de la sélection : c'est un raccourci, appelé de `server.rs:1421`. À `mux/keys.rs` (dev-saisie). Le reste de `select.rs` (`point`, `edge`, `Selection`) est bien là.
R5. `mux/focus.rs` et `layout::focus_*` : la logique des places (`Focus`, rangs, gardes) et la géométrie de ces places (`focus_rects`, `focus_room`, `focus_cards_rows`) sont aujourd'hui dans deux fichiers sans lien visible. Avec R3(b), `mux/focus.rs` + `mux/layout.rs` se voisinent. Pas de fusion.

## Hors refacto

H1. `config.rs:229` `#[serde(alias = "grid")]` : à garder, le menu écrit lui-même « grid » (`config/edit.rs:517`) ; seul le commentaire « from 0.1 » est caduc (reviewer, tranché par l'architecte). Mon premier avis (« reste de la 0.x ») était faux.
H2. Les textes d'aide des invites (`ui.rs:57,70` « ↑↓ pour choisir, Entrée pour valider ») vont à l'encontre du principe « pas d'aide dans l'interface » (mémoire du 2026-10-11) si on l'étend aux invites de la ligne de commande. Hors du multiplexeur, probablement voulu ; à confirmer par l'architecte, je ne touche à aucun texte.

## Estimation

Lot A (suppressions, ma zone, ~1/2 journée) : points 1, 2, 3 de « Supprimer » ; commentaires N1-N5, N7-N9 ; visibilité N6 si `layout.rs` bouge avant. Aucune dépendance. Fichiers touchés : `layout.rs`, `mux/screen.rs`, `mux/select.rs`, `mux/focus.rs`, `canvas.rs`, `ui/list.rs` (commentaires seulement). Rien ne casse `server.rs`.

Lot B (déplacements, architecte seul) : `git mv canvas.rs ui/canvas.rs`, `look.rs ui/look.rs`, `ui.rs` et `ui/list.rs` → `ui/prompt.rs` et `ui/prompt/list.rs` ; `layout.rs` en deux (R3) avec les `use` de dix fichiers ; `Rect` (R1) si accepté. À cadrer : R3 et R1 sont les plus larges en `use`.

Lot C (découpages et simplifications, par zone, ~1 journée) :
- C1 (canvas en quatre fichiers + tests) : le plus long, 100 % de mon côté, après B ; ~3 h.
- M1 (trait `Surface`) : dépend de dev-terminal (le trait), dev-serveur (`server.rs`), dev-interface (`cards.rs`) ; ~2 h, en bloc avec eux.
- M2 (`Scene`) : dépend de dev-serveur (un appel) ; ~1 h.
- C2, C3 : optionnels, ~1 h chacun.
- M3 : dépend de dev-interface (choix de comportement).
- R4 : dépend de dev-saisie.

Je ne fais rien de M1, M2, M3, R1, R3, R4 sans l'accord du membre concerné.

## Récapitulatif
- Supprimer : 3 points (+ un 4e « rien à retirer », avec le piège de `list.rs` justifié).
- Simplifier : 6 points (M1 à M6, dont 3 sans action ou à ne pas faire).
- Découper : 7 points (C1 à C7, dont C4, C6, C7 « ne rien faire »).
- Renommer : 9 points.
- Déplacer : 5 points.
- Hors refacto : 2 points.
