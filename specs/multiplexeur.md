# Spec : multiplexeur natif (recruit sans tmux)

Rédigée le 2026-10-09, à partir de l'analyse du même jour. Équipe chargée du travail : `mux`, dans `.recruit/settings.toml` (`recruit mux`).

Ce document fixe le but, les contraintes, les étapes et leurs critères de sortie. Les choix techniques internes reviennent à l'équipe, et l'architecte tranche. Les choix produit, marqués **[utilisateur]**, passent par l'architecte vers l'utilisateur, avec une recommandation ; personne ne les tranche seul. L'architecte tient à jour le [journal des décisions](#11-journal-des-décisions) en bas de ce fichier.

Le CLAUDE.md du projet s'applique : ses « Décisions » restent valides, sauf ce que cette spec dit explicitement de remplacer, et ses « Pièges » restent vrais tant que tmux est là.

## 1. Pourquoi

tmux nous a permis d'aller vite, mais il borne l'interface :

- **Pas de calques.** Le menu est une `display-popup` : pas de mise à jour synchronisée, refusée sans message si elle dépasse le client, commande non développée et lancée par `default-shell`. Les confirmations sont des `display-menu` : invisibles à `capture-pane`, `-M -O` obligatoire, un seul à la fois, refusé sans rien dire sinon.
- **Clics détournés.** Un clic dans un panneau passe par `#{mouse_line}` copié dans une option du panneau, relue par `recruit _click` ; la barre n'offre que des zones (`range`).
- **Raccourcis globaux.** Liés pour tout le serveur, relus par `#{@recruit_state}`, à relier à chaque lancement.
- **Chrome pauvre.** Le tableau de bord et le journal sont des panneaux voisins : impossible de dessiner sur les autres panneaux, et le titre d'un panneau se limite à `pane-border-format`.
- **Conflit de touches.** Le préfixe `Ctrl-b` de tmux prend Ctrl+B à Claude Code (tâches en arrière-plan).
- **Distribution.** tmux 3.5 au minimum : absent d'Ubuntu 24.04 (3.4) et de Debian 12 (3.3a).
- **Coût de maintenance.** Environ la moitié des « Pièges » du CLAUDE.md viennent de tmux.

## 2. But

Un multiplexeur écrit pour recruit, dans le binaire `recruit`, qui remplace tmux. Il :

- fait tourner chaque membre dans un terminal émulé (un PTY et un moteur VT) ;
- dessine lui-même tout l'écran : panneaux, onglets, en-têtes, tableau de bord, journal, menus en calques ;
- garde l'équipe en vie quand on ferme le terminal : serveur et client, détacher, revenir, même par SSH ;
- ne fait que ce dont recruit a besoin.

Critère technique, repris de la décision du 2026-10-08 sur le menu : **la performance**. Premier écran sans attendre, redessin par différence, aucune entrée-sortie lente sur le chemin du dessin, rien au repos.

## 3. Hors périmètre

- La configuration tmux de l'utilisateur, les plugins, un copy-mode vi complet.
- Les découpes libres par l'utilisateur : la disposition reste celle de `layout.rs`, plus un zoom sur un membre.
- Plusieurs clients sur la même équipe en même temps (voir [question 3](#10-questions-ouvertes-utilisateur)).
- Les graphismes (sixel, kitty graphics) et les ligatures.
- Windows.
- La survie au redémarrage de la machine : les membres reprennent déjà leur conversation (`member.rs`).
- Un multiplexeur généraliste : pas de shell libre hors des panneaux des membres (le shell laissé après Claude reste).

## 4. Ce que tmux fait aujourd'hui, et ce qui le reprend

| Fonction | Aujourd'hui | Demain |
|---|---|---|
| Serveur, sessions, équipes lancées | `tmux -L recruit`, `Tmux::running`, `has_session` | un serveur par équipe, joint par socket ; `recruit list` interroge les sockets |
| Onglets et grilles | `Tmux::build`, `arrange` (`split-window`, `select-layout`, `swap-pane`, `join-pane`, `break-pane`) | modèle d'écran natif, nourri par `layout.rs` |
| Lancer, relancer un membre | `open`, `respawn` | PTY qui lance `recruit _member …`, inchangé |
| Tableau de bord, journal | processus `recruit _panel` dans des panneaux tmux | vues natives dans le serveur, avec le code de dessin de `board.rs` |
| Journal `Alt+j` | `toggle_journal`, `resize-pane`, option `@recruit_reduced` | état du serveur |
| Menu (`/recruit`, `Alt+r`, bouton) | `run-shell -b` → `display-popup` → `recruit _menu --popup` | calque natif : `menu/sheet.rs` inchangé, `menu/draw.rs` dans un rectangle |
| Menu quitter `Alt+q`, confirmations | `display-menu` | calques natifs |
| Clics | `click_binding`, `recruit _click`, options `@recruit_click_*`, `zones.json` | routage natif |
| Barre | `status-left`, `status-right`, `range` | barre native |
| Titre des panneaux | `pane-border-format`, `@recruit_member` | en-tête natif |
| Raccourcis | `bind-key -n` | table du serveur |
| Détacher, rejoindre | `detach-client`, `attach-session` | client mince |
| Aller à un membre | `focus`, `client_of`, `switch-client` | requête au serveur |
| Taille et terminal du client | `client_look`, `client_terminals` | envoyés par le client au serveur |
| Historique, défilement, copie | `history-limit 100000`, `set-clipboard on` | moteur VT et vue native |
| Relais vers le vrai terminal | `allow-passthrough`, `extended-keys`, `terminal-features` | relais natif ([§5.6](#56-relais-vers-le-vrai-terminal)) |
| Doublons de noms | ListAgents montre « tmux <session>:… » (`prompt.rs:69`) | **à revoir** ([question 1](#10-questions-ouvertes-utilisateur)) |

Ce qui appelle `Tmux` hors de `tmux.rs` : `launch.rs`, `live.rs`, `app.rs`, `board.rs` (`toggle`, `client_terminals`) et quelques messages de `bridge.rs`. Le trait `Backend` (`tmux.rs:153`) ne couvre aujourd'hui que `running`, `launch`, `attach` et `stop`.

## 5. Architecture cible

### 5.1 Processus

- **Un serveur par équipe** : `recruit _server <dossier d'état>` (sous-commande cachée), détaché (`setsid`, entrées-sorties vers `server.log` dans le dossier d'état). Il possède les PTY, les moteurs VT, l'état de l'écran et les vues natives, et dessine pour le client.
- **Un client mince** : `recruit` ou `recruit attach`, en mode brut. Il envoie les événements (clavier, souris, collage, focus, taille, capacités du terminal) et écrit ce que le serveur lui renvoie. Toute la logique est au serveur, comme pour le mod passerelle.
- Les commandes qui touchent une équipe lancée (`_mod`, `_edit`, `live.rs`, `stop`, `list`) passent par le même socket.
- **Socket** : comme tmux, dans un dossier court, `${RECRUIT_TMPDIR:-/tmp}/recruit-<uid>/` en 0700. Un chemin sous `~/.cache` ou un `XDG_CACHE_HOME` de test dans `/var/folders/…` dépasse vite les 104 octets de `sun_path` sous macOS. Le serveur vérifie l'uid de chaque pair (`getpeereid`). Les tests isolent leurs serveurs par `RECRUIT_TMPDIR` et `XDG_*`.
- **Protocole versionné** : un client et un serveur de versions différentes se refusent avec un message clair (une équipe lancée avant une mise à jour reste joignable par l'ancien binaire, ou s'arrête proprement).
- Pas de runtime async par défaut (fils et canaux). L'architecte peut en décider autrement, mesures à l'appui.
- Le serveur survit à la perte du client (terminal fermé, SIGHUP, SSH coupé). Il s'arrête avec l'équipe (Quitter, `recruit stop`).

### 5.2 Un panneau

- Un PTY (`openpty`/`forkpty` par `libc`, déjà en dépendance, ou un crate à justifier), qui lance `recruit _member …` comme aujourd'hui. `TIOCSWINSZ` à chaque redimensionnement. Arrêt par groupe de processus, pas de zombie, descripteurs en `CLOEXEC`.
- Environnement : celui du `Pane` (`env`), **sans `TMUX` ni `TMUX_PANE`** (lancé depuis un tmux, un membre s'y croirait : teammates en panneaux tmux, ListAgents faux), avec `TERM`, `COLORTERM=truecolor` et `TERM_PROGRAM` choisis au prototype ([§6](#6-compatibilité-avec-claude-code)).
- **Moteur VT derrière un trait** (`Engine`), le reste du code ne voit que lui :
  - `alacritty_terminal` (0.26, avril 2026) d'abord : Rust pur, mûr, c'est le moteur du terminal de Zed ; historique, sélection et recherche fournis ; pas d'encodeur de touches, graphèmes limités.
  - libghostty-vt évalué au prototype : le plus juste, encodeurs de touches, souris, collage et focus fournis, état de rendu ; mais pas d'API stable, liaisons Rust jeunes, et Zig à la compilation (déjà dans `release.sh` par cargo-zigbuild, mais aussi requis en CI et pour qui compile). C'est le choix de herdr, qui a retiré `vt100` en 0.4.0.
  - `vt100` est écarté.
- Le moteur répond aux requêtes de l'application : DA1, DA2, DSR et CPR, DECRQM (dont le mode 2026), XTVERSION, requête du protocole clavier kitty, OSC 10 et 11 (couleurs). Les couleurs du vrai terminal sont demandées une fois par le client, puis transmises.

### 5.3 Écran

- Un modèle d'écran : onglets, rectangles des panneaux (tirés de `layout.rs`), chrome, calques empilés.
- Rendu dans un `Canvas` (`canvas.rs`, étendu : graphèmes entiers au lieu d'écarter les caractères sans largeur, attributs, couleurs 24 bits, liens OSC 8, curseur), envoyé par différence (`Painter`). Chaque image est entourée de BSU/ESU (mode 2026).
- Un panneau en pleine mise à jour synchronisée (mode 2026 demandé par l'application) n'est jamais dessiné à moitié : on attend sa fin, avec un délai maximal comme les terminaux.
- Cadence plafonnée (60 images/s), dessin seulement sur changement, aucun réveil au repos.
- Le curseur affiché est celui du panneau qui a le focus (position, forme DECSCUSR, visibilité), sinon il est caché.
- Les capacités du vrai terminal (couleurs, OSC 8, protocole clavier, largeur des graphèmes et mode 2027) sont détectées par le client et transmises. Repli propre : couleurs 256, liens retirés, etc.
- `menu/draw.rs`, `ui/list.rs` et le tableau de bord se servent déjà de `canvas.rs` : ils doivent continuer de marcher à chaque étape.

### 5.4 Entrées

- Le client lit les événements (crossterm, avec le protocole clavier kitty quand le terminal l'a, rendu tel qu'il était à la sortie et à la panique, comme `canvas::Session`) et les envoie structurés.
- Le serveur traite d'abord ses raccourcis et le chrome (onglets, en-têtes, tableau de bord, calques). Le reste est réencodé pour le panneau qui a le focus, selon les modes que ce panneau a demandés : protocole kitty et ses drapeaux, touches curseur en mode application, collage encadré, souris (1000, 1002, 1003, 1006), focus (1004).
- Souris : coordonnées traduites dans le panneau sous le pointeur ; la molette va à l'application si elle capture la souris, sinon elle fait défiler l'historique.
- Shift+Entrée va à la ligne dans Claude Code, quel que soit le terminal (traduit si besoin).
- Échap passe sans délai perceptible (tmux : `escape-time 10`).
- Sous macOS, Option envoyé comme Alt reste un prérequis du terminal ; les raccourcis s'affichent avec ⌥ (`tmux::ALT`, à déplacer).

### 5.5 Historique, sélection, copie

- Un historique par panneau, au moins 100 000 lignes comme aujourd'hui (mémoire mesurée au prototype).
- Défilement à la molette et au clavier quand l'application ne capture pas la souris (Claude Code en mode normal compte sur l'historique du terminal).
- Sélection à la souris, double clic pour un mot, triple clic pour une ligne, copie par OSC 52 vers le vrai terminal.
- Recherche dans l'historique : plus tard.

### 5.6 Relais vers le vrai terminal

- OSC 52 (presse-papiers), OSC 9, 777 et 99 et BEL (notifications), OSC 9;4 (progression), titre : relayés au vrai terminal.
- Ils deviennent aussi visibles dans l'interface : badge sur le membre et sur son onglet. Ce que tmux ne pouvait pas faire.

### 5.7 Code

- Nouveau dossier `src/mux/` (proposition, l'architecte tranche) : `pty.rs`, `engine.rs`, `input.rs` (lecture et encodage), `screen.rs` (modèle et composition), `chrome.rs`, `server.rs`, `client.rs`, `proto.rs`.
- Réutilisés : `layout.rs`, `canvas.rs`, `look.rs`, le dessin de `board.rs`, `menu/sheet.rs`, `menu/draw.rs`, `member.rs`, `state.rs`, `bridge.rs` et le mod.
- `Backend` grandit pour couvrir ce que `launch.rs`, `live.rs`, `app.rs` et `board.rs` demandent aujourd'hui à `Tmux`. `Native` l'implémente à côté de `Tmux` jusqu'au retrait.
- Pendant la transition, `RECRUIT_BACKEND=native` (variable cachée) choisit le multiplexeur natif ; tmux reste le défaut, et rien de ce qui marche sous tmux ne doit casser.
- Dépendances : licence compatible avec l'AGPL, construction Linux musl statique (cargo-zigbuild) intacte, poids mesuré ; ajoutées avec l'accord de l'architecte.
- Chaque nouveau fichier commence par les lignes SPDX et copyright du projet. Tout texte vu par l'utilisateur passe par `t!`, dans les deux langues. Commentaires en anglais, comme le reste du code.

## 6. Compatibilité avec Claude Code

C'est la vraie difficulté : on devient le terminal de Claude Code. Chaque ligne a son test ([§9](#9-tests)), rejoué à chaque étape et à chaque mise à jour de Claude Code.

| Point | Exigence |
|---|---|
| Rendu | Modes par défaut et plein écran (`/tui fullscreen`) : écran alterné, régions de défilement, mise à jour synchronisée, redimensionnement sans cellules fantômes, pas de scintillement. |
| Clavier | Shift+Entrée, Alt/Option, Ctrl+B (arrière-plan), Ctrl+R, Ctrl+O, Ctrl+C, Ctrl+D, Échap, Tab et Shift+Tab (mode de permission), flèches ; protocole kitty quand Claude Code le demande. |
| Collage | Encadré ; un grand collage (plus de 100 Ko) ; retours à la ligne gardés. |
| Souris | Clics, molette, sélection en plein écran, liens cliquables. |
| Liens | OSC 8 relayés, pas de double ouverture. |
| Presse-papiers | La copie de Claude Code (OSC 52) arrive dans le presse-papiers du système. |
| Notifications | Notifications et progression relayées et visibles. |
| Focus | Événements 1004 quand on change de panneau. |
| Couleurs | 24 bits ; le thème clair ou sombre détecté comme dans le vrai terminal (OSC 11). |
| Unicode | CJK, emoji, séquences ZWJ, accents combinants, indicateur animé de Claude Code : alignés comme dans le vrai terminal. |
| Performance | 10 panneaux qui écrivent en continu restent fluides ; chiffres au compte rendu. |
| Terminal annoncé | Claude Code choisit le protocole clavier sur une liste de noms (`TERM_PROGRAM`), pas par détection ([issue 71700](https://claudeissues.com/issue/71700-bug-kitty-keyboard-protocol-gated-on-terminal-name-allow-list-instead-of-csi-u-c)). Le prototype établit ce qu'on annonce (`TERM`, `TERM_PROGRAM`) et ses effets ; le choix est consigné au journal. |
| Mod et barres | Le mod se charge, `PromptHint` et `SessionMode` restent masqués, la ligne d'état QUIET reste, `/recruit` et `/equipe` marchent. |
| Teammates | Sans `TMUX`, ils tournent dans le processus du membre ; le tableau de bord les montre déjà. |
| Réglages | `<profil>/settings.json` ne change pas (vérifié à chaque session réelle). |

## 7. Expérience visée

### 7.1 Parité, obligatoire avant le retrait de tmux

Toutes les décisions du CLAUDE.md qui touchent l'écran restent vraies en natif :

- onglet « Interlocuteurs », puis onglets d'agents par `tab` ou « Agents », groupes de 6 au plus répartis à parts égales ; `layout = "tabs"`, `columns`, `rows` ;
- tableau de bord à droite des interlocuteurs, journal dessous, `Alt+j` (complet, réduit, masqué ; réduit au lancement), `dashboard = false` ;
- clic sur une carte, un expéditeur ou un destinataire : on va au panneau du membre ; clic sur le contexte d'un membre au repos : compacter après confirmation ;
- `Alt+r`, `/recruit` et le bouton « menu » ouvrent le menu ; `Alt+q` et le bouton « quitter » : Détacher (`d`), Quitter (`q`), Annuler (`a`) (Detach `d`, Quit `q`, Cancel `c` en anglais) ;
- ⌥ sous macOS, `Alt+1`…`Alt+9`, `Alt+Maj+←/→` ;
- relance d'un membre arrêté après 2 s, Ctrl-C pour un shell ; revenir sur une équipe relance les arrêtés, rouvre le tableau de bord, propose de reconstruire ;
- modifications à chaud du menu (`live.rs`) : ajout, retrait, renommage, déplacement entre onglets sans arrêter le membre ;
- menu en lecture seule quand les fichiers sont illisibles ; `--dry-run` ;
- `scripts/screenshots.sh` produit les mêmes captures.

### 7.2 Ce qu'on vise ensuite

À maquetter d'abord ; l'utilisateur choisit sur maquettes, comme pour le tableau de bord et le menu. Pistes :

- **En-tête de panneau** : nom, état (trait et couleur du tableau de bord), modèle, effort, contexte ; un clic ouvre la fiche du membre dans le menu.
- **Calques** : le menu et les confirmations au-dessus de l'équipe, le reste atténué.
- **Notifications brèves** (« dev-rust attend ta réponse ») et badges sur les onglets qui ont un membre en attente.
- **Zoom** d'un membre, plein écran et retour, sans préfixe.
- **Survol** : cartes, boutons et onglets qui réagissent.
- **Plus de préfixe `Ctrl-b`** : raccourcis Alt seulement, Ctrl+B rendu à Claude Code **[utilisateur]**.
- **Barre d'onglets** cliquable, avec l'état de chaque onglet.

Les maquettes sont des artifacts, comme celles citées au CLAUDE.md. La direction choisie y est consignée.

## 8. Étapes et critères de sortie

Chaque étape finit par un compte rendu de l'architecte à l'utilisateur (ce qui est fait, la grille du [§6](#6-compatibilité-avec-claude-code), les mesures, les risques). L'étape suivante ne commence qu'avec son accord.

### Étape 0 : prototype, qui décide de la suite

- Une sous-commande cachée et provisoire : deux panneaux côte à côte, chacun avec un vrai Claude Code (`claude` seul, dans un dossier temporaire), dessinés par le moteur et `canvas.rs`, clavier et souris routés. Sans serveur.
- `alacritty_terminal` et libghostty-vt comparés au moins sur les points durs : graphèmes, encodage du clavier, mise à jour synchronisée, temps de compilation, construction musl.
- En parallèle : les maquettes du [§7.2](#72-ce-quon-vise-ensuite) (designer), et le protocole et le squelette du serveur sur le papier (dev-serveur), sans intégration.
- **Livrable** : `specs/multiplexeur-prototype.md` avec la grille du §6 remplie (ok, ko, contournement), les mesures (CPU au repos et en flux, latence ajoutée à la frappe, mémoire par panneau avec 100 000 lignes d'historique), le moteur et le terminal annoncé recommandés, les risques restants.
- **Sortie** : l'utilisateur dit si on continue, et avec quel moteur.

### Étape 1 : le moteur

- Serveur et client, socket, détacher et revenir (SSH compris), une équipe complète lancée par `RECRUIT_BACKEND=native recruit` : onglets et grilles de `layout.rs`, membres relancés, historique, sélection, copie, relais.
- Chrome minimal : titres de panneaux, barre d'onglets. Le tableau de bord et le journal peuvent encore tourner comme aujourd'hui, en processus dans un panneau.
- **Sortie** : une vraie équipe tient une journée de travail en natif sans perte ; les tests du [§9](#9-tests) sont verts.

### Étape 2 : parité

- Vues natives du tableau de bord et du journal, menu et menu quitter en calques, clics, raccourcis, `live.rs`, `recruit list|attach|stop`, `/recruit` qui ouvre le calque, `--dry-run`, lecture seule, `scripts/screenshots.sh` adapté.
- **Sortie** : la liste du [§7.1](#71-parité-obligatoire-avant-le-retrait-de-tmux) vérifiée point par point, captures à l'appui.

### Étape 3 : nouvelle interface

- Les maquettes choisies par l'utilisateur, implémentées.
- **Sortie** : l'utilisateur valide sur l'application réelle.

### Étape 4 : retrait de tmux

- Le natif par défaut ; `tmux.rs` et le code devenu mort retirés.
- `[tmux]` dans les fichiers d'équipe : sort à décider **[utilisateur]**. Le refus des clés inconnues ferait échouer les fichiers existants ; proposition : accepter et ignorer `[tmux]` avec un avertissement pendant une version.
- Site en anglais et en français (`guides/tmux.md` remplacé, installation sans tmux, configuration, FAQ), README, CLAUDE.md (« Organisation », pièges devenus caducs, nouveaux pièges), notes de version. Numéro de version **[utilisateur]**.
- **Sortie** : l'utilisateur publie.

## 9. Tests

- **Unitaires**, sans terminal ni Claude, comme aujourd'hui : encodage des touches (tables tirées des spécifications kitty et xterm), modes, modèle d'écran et composition (le `Canvas` rendu en texte et comparé), protocole, disposition.
- **Intégration sans terminal** : un serveur dans le test, des programmes factices dans les PTY qui écrivent des séquences connues, un client de test qui reçoit les images et les relit dans un moteur VT ; on compare des écrans en texte. Ils remplacent peu à peu les serveurs tmux de test.
- **Vraies sessions Claude Code** : la grille du §6 à chaque étape, avec `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` et `RECRUIT_TMPDIR` dans des dossiers temporaires ; jamais le serveur `-L recruit`, les équipes ni `~/.config/recruit` de l'utilisateur ; `<profil>/settings.json` inchangé. Sessions courtes, demandes simples.
- **Terminaux** : Ghostty, iTerm2, Terminal.app, kitty, WezTerm ; dans un tmux (imbriqué) ; par SSH ; Linux par la CI.
- **Performance** : un banc reproductible (N panneaux factices en flux et une vraie session), chiffres dans chaque compte rendu.
- La CI (fmt, clippy, tests en français et en anglais, Linux et macOS, binaires musl) reste verte à chaque étape.

## 10. Questions ouvertes [utilisateur]

L'architecte les pose au bon moment, avec une recommandation.

1. **Doublons de noms.** Tranchée le 2026-10-09 (refs dans la note d'équipe, voir le journal). Sans tmux, ListAgents ne montre plus « tmux <session>:… », sur quoi repose la décision du 2026-10-07. À établir au prototype : ce que ListAgents montre d'une session hors tmux. Puis proposer une autre façon de distinguer deux équipes qui ont des membres du même nom.
2. **Préfixe.** Aucun (Alt seulement, Ctrl+B rendu à Claude Code) ou `Ctrl-b` gardé pour les habitués. Tranchée le 2026-10-09 : aucun.
3. **Deuxième `attach`.** Tranchée le 2026-10-09 : il reprend la main. Il reprend la main (le premier client est détaché avec un message), il est refusé, ou les deux clients partagent l'écran.
4. **`[tmux]`** dans les fichiers d'équipe, au retrait.
5. **Version** au retrait (2.0.0 ?).
6. **Moteur VT**, après le prototype. Tranchée le 2026-10-09 : libghostty-vt.
7. **tmux en secours** après la parité. Recommandation : non, ce serait une double maintenance.

## 11. Journal des décisions

| Date | Décision | Par |
|---|---|---|
| 2026-10-09 | Spec écrite ; équipe `mux` créée ; le prototype décide de la suite. | utilisateur |
| 2026-10-09 | Étape 0 ouverte. | utilisateur |
| 2026-10-09 | Code dans `src/mux/` comme proposé au §5.7, plus `engine/` (un adaptateur par moteur, libghostty-vt derrière une fonction cargo pour que l'arbre se construise sans Zig) et `prototype.rs` (la boucle de l'étape 0, retirée à l'étape 1). Interfaces fixées dans le code : trait `Engine`, `Modes`, `Relay` (`engine.rs`) ; `Pty`, `Spawn`, `PtyInput` (`pty.rs`) ; encodeurs et `Terminal` (`input.rs`) ; `compose` et `View` (`screen.rs`) ; `Caps` et `Rect` (`mod.rs`) ; dans `canvas.rs`, `put_cell` (largeur donnée par le moteur), `Style.italic`, `strike`, `link`, `Cursor`, `Features`, `Painter::new`. | architecte |
| 2026-10-09 | Prototype `recruit _mux` (caché, provisoire) : un fil lecteur par PTY qui nourrit le moteur du panneau, derrière un mutex ; le fil principal route les entrées et dessine, réveillé seulement par un message, une image due (60/s au plus) ou l'échéance d'une mise à jour synchronisée. Les événements sont ceux de crossterm à l'étape 0. | architecte |
| 2026-10-09 | `alacritty_terminal` 0.26.0 ajouté (Apache-2.0, compatible AGPL), sans sa fonction `serde`. Poids et construction musl mesurés au prototype. | architecte |
| 2026-10-09 | Après relecture des interfaces : le défilement de l'historique est porté par le moteur (`Engine::scroll`, `scrolled`), pour que la vue reste sur les mêmes lignes quand le programme écrit. Une panique d'un moteur (dans le fil lecteur d'un PTY) remplace ce moteur et le dit dans le titre du panneau, sans arrêter le reste ; au serveur, notée dans `server.log`. Le fil lecteur d'un PTY n'écrit jamais sur le maître : un seul fil écrivain, réponses du moteur en tête de sa file. | architecte |
| 2026-10-09 | Protocole (`specs/multiplexeur-serveur.md`) : client et serveur se refusent sur un numéro de protocole (`PROTO`) différent, pas sur une version de recruit différente (remplace « versions différentes » du §5.1) ; `Hello`, `Welcome`, `Refused` et `Stop` sont figés pour toujours. Le trait `Backend` grandit par primitives jusqu'au retrait de tmux. | architecte |
| 2026-10-09 | Entrées : le client lit lui-même les octets du terminal, avec nos propres types d'événements (`input::Event`, `Key`, `Mouse`), plutôt que crossterm : les réponses tardives du terminal (SSH) sont jetées au lieu d'être tapées, rien ne se perd des touches, et les types se sérialisent pour le protocole. Côté vrai terminal, seul le drapeau kitty 1 est poussé (Échap sans attente, texte composé intact). | dev-saisie, accordé par l'architecte |
| 2026-10-09 | Maquettes du §7.2 (https://claude.ai/artifact/MbjmHDAzeN3mNK9jdqmXQ7) : direction B « Cadres » choisie (cadre à la couleur de l'état, épais sur le panneau actif, en-tête dans le bord du haut, barre et menus en bas, notification en calque en haut à droite). Inscrite au CLAUDE.md. ⌥o, pris par Claude Code (mode rapide), remplacé par ⌥n. | utilisateur |
| 2026-10-09 | Question 2 : pas de préfixe. Raccourcis ⌥ seulement, Ctrl+B rendu à Claude Code. | utilisateur |
| 2026-10-09 | Grille de compatibilité : iTerm2, kitty et WezTerm installés (avec Ghostty et Terminal.app déjà là), pour une application robuste. Les touches sont vérifiées par de vraies frappes envoyées par le testeur (System Events), pour en faire un test d'intégration rejoué avant chaque publication. | utilisateur |
| 2026-10-09 | Bug d'alacritty_terminal 0.26 (plus de 4096 poussées kitty → panique) : déjà signalé en amont (alacritty#8957), fermé sans correctif par le mainteneur. Pas de nouveau signalement ; le prototype rattrape la panique, remplace le moteur, et abandonne un panneau après 3 échecs en 10 s. | architecte |
| 2026-10-09 | Terminal annoncé dans le prototype, sur les essais de dev-terminal avec Claude Code 2.1.295 : TERM=xterm-256color, COLORTERM=truecolor, TERM_PROGRAM=recruit et sa version (FORCE_HYPERLINK retiré après relecture : il passerait aux commandes de l'outil Bash) ; le moteur répond à `CSI ? u` (kitty, donc Shift+Entrée), à XTVERSION (donc 2026) et à `OSC 7501 ; ?` (Claude Code donne alors lui-même son état : idle, working, blocked avec la raison, done). On ne se fait pas passer pour Ghostty ; les notifications viendront de nous, au passage en « blocked ». Provisoire jusqu'au compte rendu de l'étape 0. Nouveau `Relay::Status`. | architecte |
| 2026-10-09 | Étape 0 close (livrable : `specs/multiplexeur-prototype.md`) ; on continue vers l'étape 1. | utilisateur |
| 2026-10-09 | Question 6, moteur : libghostty-vt, historique compressé. alacritty_terminal abandonné : retiré dès que l'adaptateur libghostty-vt tourne, sans période de double moteur. Zig 0.16.0 entre dans la chaîne de construction (CI, `release.sh`, contributeurs). | utilisateur |
| 2026-10-09 | Rust à sa dernière version stable : 1.99.0 installé, `rust-version = "1.99"` dans Cargo.toml (était 1.88 ; libghostty-vt demande 1.90). | utilisateur, fait par l'architecte |
| 2026-10-09 | Question 1, doublons de noms : chaque membre reçoit dans la note d'équipe les adresses exactes (`nom [ref]`) de ses coéquipiers qui ont un homonyme, calculées par recruit depuis `<profil>/sessions/<pid>.json`. Condition de l'utilisateur : une mise à jour de Claude Code ne doit rien casser. D'où : refs seulement quand un nom est en double ; si l'envoi à une ref échoue, le membre se rabat sur ListAgents (consigne dans le prompt) ; la formule revérifiée par la grille à chaque version. | utilisateur, modalités par l'architecte |
| 2026-10-09 | Question 3, deuxième `attach` : A, il reprend la main (le premier client est détaché avec un message). Détacher et revenir est facultatif en V1 si c'est complexe (fonction de tmux, pas de recruit). | utilisateur |
| 2026-10-09 | Après un plantage du serveur : les conversations des membres reprennent sans demander, en le disant ; une seconde reprise automatique dans les minutes qui suivent demande d'abord. | utilisateur |
| 2026-10-09 | `FORCE_HYPERLINK` n'est pas posé dans les panneaux (il passerait aux commandes de l'outil Bash) : les liens de Claude dans les panneaux restent à traiter à l'étape 1. | architecte |
| 2026-10-09 | libghostty-vt : `unsafe impl Send` sur l'enveloppe (aucune variable locale au fil dans le code compilé, audit refait par le script à chaque montée), pas de fil par moteur. Trait `Engine` : `deadline()` remplace `sync_deadline()` (fin de synchro ou compression à finir), `expire()` rend si l'écran a changé. Compression : une étape bornée dans `feed` toutes les ~64 Kio, puis une passe 1 s après la dernière activité, par étapes de 10 ms ; rien au repos. | dev-terminal, accordé par l'architecte |
| 2026-10-09 | Construction de libghostty-vt par remplacement du script de construction de la crate (`links`) : `scripts/ghostty.sh` télécharge et vérifie Zig 0.16.0, l'archive de Ghostty et ses paquets Zig dans `.ghostty/`, construit la bibliothèque par cible et écrit `.cargo/config.toml` (ignoré par git). Cargo ne lance plus Zig ; cargo-zigbuild garde le Zig de brew. Licences : Ghostty MIT, libghostty-rs MIT ou Apache-2.0, paquets Zig zlib, Apache-2.0/BSD-3, MIT. alacritty_terminal reste en dépendance de développement pour le terminal de test. | dev-terminal, accordé par l'architecte |
| 2026-10-09 | OSC 7501 : le moteur ne prend les états qu'après avoir répondu à `OSC 7501 ; ?`, et les oublie sur `state=clear` ou RIS ; `member.rs` écrit `state=clear` quand son Claude s'arrête. | architecte |
| 2026-10-09 | Sélection à la souris : le texte reste surligné après la copie, jusqu'au prochain clic ou à la prochaine touche. `scripts/ghostty.sh` ajouté aux commandes du CLAUDE.md. Passage court de frappes réelles autorisé (Ghostty, sans touches dangereuses). | utilisateur |
| 2026-10-09 | Frappes réelles : macOS 14 et suivants refusent le premier plan à une fenêtre lancée en arrière-plan tant que l'application active ne le cède pas (deux passages sur Ghostty arrêtés par le garde-fou, rien tapé). Pas de clic de l'utilisateur : le test garde au moins Terminal.app (lancé neuf) ; Ghostty sera vérifié plus tard à l'usage réel. | utilisateur |
| 2026-10-09 | ⌥n ne passe que par les panneaux des membres (le tableau de bord et le journal restent accessibles au clic). Tous les tests utiles autorisés, SSH sur `macbook` compris (dossiers temporaires, rien d'autre touché). | utilisateur |
| 2026-10-09 | Trait `Engine` : `top`, `oldest`, `line` (sélection et copie dans l'historique, numérotation absolue des lignes) et `generation()` (un compteur qui ne croît que si ce que montre le panneau change ; `None` : dessiner à chaque fois), pour que la composition garde les panneaux inchangés. Le défilement par régions (DECSTBM, DECSLRM) est reporté, faute de prise en charge partout. | architecte |
| 2026-10-09 | Prototype de l'étape 0 retiré (`recruit _mux`, `prototype.rs`) : les tests passent par le serveur natif ; les tests d'intégration sans fenêtre (`tests/mux.rs`, `tests/mux_server.rs`, 12 tests, environ 10 s) tournent dans `cargo test`, donc dans la CI. Une équipe qui repart sur un autre multiplexeur que son dernier le dit. En natif, `Backend::focus` attend les clics de l'étape 2. | architecte |
| 2026-10-09 | Plantage reconnu à la marque `crashed` (le `server.json` d'un serveur mort, renommé par le ménage au lieu d'être supprimé), verrou libre : un serveur lent ou figé n'est pas planté. Un `recruit stop` qui doit tuer un serveur bloqué est un arrêt voulu, sans marque : l'équipe repart à neuf. La marque part quand l'équipe est relancée ; ni `--restart` ni `--dry-run` ne reprennent. | architecte |
| 2026-10-09 | Passage de frappes réelles dans iTerm2 par le vrai client natif autorisé (profil dynamique temporaire, touches dangereuses comprises). | utilisateur |
| 2026-10-09 | Les dossiers des sessions de test sous `~/.claude/projects/` (transcriptions et mémoire automatique des projets temporaires de l'équipe, 20 dossiers, environ 8 Mio) sont retirés ; celui d'une autre équipe gardé. | utilisateur |
| 2026-10-09 | Journée de travail de l'étape 1 : l'équipe `mux` elle-même passe en natif (copie release dans `target/day/`, relancée par l'utilisateur avec `RECRUIT_BACKEND=native … mux --restart --resume`), et l'étape 2 s'ouvre en même temps pour lui donner du vrai travail ; retour sous tmux par la même commande sans la variable si un événement bloque. Second passage de frappes iTerm2 autorisé. | utilisateur |
| 2026-10-09 | Étape 2 ouverte (utilisateur, avec la journée en natif). Le tableau de bord et le journal restent des programmes dans des panneaux pour la parité ; le serveur lit leurs zones pour les clics (`board::clicked`, commun avec tmux). Le choix Détacher, Quitter, Annuler et les confirmations sont un calque dessiné par le serveur (`chrome::Choice`), le reste atténué. | architecte |
| 2026-10-09 | Confirmation de compaction harmonisée, sous tmux comme en natif : titre « Compacter <membre> ? » (« Compact <member>? »), note « Résume sa conversation pour libérer du contexte (88 %). » (« Summarizes its conversation to free context (88%). ») avec le contexte du membre, options Compacter (c) et Annuler (a) ; Compact (o) et Cancel (c) en anglais : la même touche d'annulation que le choix ⌥q, Échap annule partout. Le choix ⌥q garde le titre « recruit », sans note, comme sous tmux. | utilisateur |
| 2026-10-09 | Passage de captures de fenêtres dans les cinq terminaux (sans frappe, environ 1 min) autorisé, pour vérifier l'effacement par ECH. | utilisateur |
| 2026-10-09 | Panneau actif : cadre épais et gras à la couleur du texte du terminal ; les autres cadres gris, plus de jaune ; seul le rouge reste (en attente, erreur : fin sans le focus, épais avec) ; le travail se lit à l'indicateur animé de l'en-tête. Remplace les couleurs de cadre de la direction « Cadres » (CLAUDE.md mis à jour). | utilisateur |
| 2026-10-10 | Le travail du multiplexeur natif est commité sur la branche `multiplexeur` et poussé sur GitHub, pour faire tourner la CI (lancée à la main, `workflow_dispatch` : la CI ne part d'elle-même que sur `main`). `main` n'est pas touché. | utilisateur |
| 2026-10-10 | Étape 3 ouverte par l'utilisateur pour la nuit : tout le reste de la maquette « Cadres », sans le solliciter (ni question, ni fenêtre, ni frappe) ; l'équipe tourne en natif toute la nuit (test de charge) ; journée de test des étapes 1, 2 et 3 ensemble le lendemain. Les choix de détail suivent la maquette et sont notés pour sa revue. | utilisateur |
| 2026-10-10 | Étape 3 : les survols des cartes du tableau de bord attendent (le tableau de bord est un programme à part) ; ⌥z (zoom) et ⌥g (membre qui attend) tels que montrés sur la maquette ; la notification ne s'affiche que pour un membre hors de vue. | architecte |
| 2026-10-10 | Délai de garde des mises à jour synchronisées (mode 2026) porté à 1 s, comme Ghostty (`sync_reset_ms`) : à 150 ms, une image lente à arriver (machine chargée, petit tampon du PTY sous macOS) s'affichait à moitié. Un programme qui oublie l'ESU fige son panneau une seconde, comme dans Ghostty. | architecte |
| 2026-10-10 | Version 2.0.0 (utilisateur) : tmux retiré dès cette version (tmux.rs et le code devenu mort), le multiplexeur natif seul ; une section `[tmux]` des fichiers d'équipe est acceptée et ignorée, avec un avertissement, pendant une version. Les étapes 1 à 3 sont closes sans la journée de test prévue, à la demande de l'utilisateur. | utilisateur |
| 2026-10-10 | Passage depuis la 1.x : une équipe encore sous le tmux de recruit 1 est proposée à l'arrêt puis relancée ici, chacun sur sa conversation (sans terminal : la marche à suivre) ; `recruit stop` l'arrête ; `recruit list` la montre « en cours sous recruit 1 (tmux) », pas « arrêtée ». Après l'arrêt, la vérification des doublons attend que ses sessions Claude se ferment, comme après `--restart`. | architecte |
| 2026-10-10 | CLAUDE.md mis à jour pour la 2.0.0 (étape 4) : en-tête, organisation, décisions et pièges sans tmux, pièges du multiplexeur ajoutés. Après la relecture et les captures : commit sur `multiplexeur`, push, CI ; fusion dans `main` et push de `main` si la CI est verte ; `scripts/release.sh 2.0.0` lancé par l'utilisateur. | utilisateur |
