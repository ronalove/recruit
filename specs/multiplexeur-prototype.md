# Prototype du multiplexeur natif : compte rendu de l'étape 0

Rédigé par l'architecte le 2026-10-09, à partir des rendus de l'équipe `mux`, relu par reviewer. Spec : [multiplexeur.md](multiplexeur.md) (§8, étape 0) ; grille détaillée : [multiplexeur-compat.md](multiplexeur-compat.md) ; serveur et protocole sur le papier : [multiplexeur-serveur.md](multiplexeur-serveur.md) ; suivi : [multiplexeur-suivi.md](multiplexeur-suivi.md).

**État du document : complet, sauf les frappes réelles dans les cinq vrais terminaux** (§3), suspendues après un premier passage raté (§8) et en attente de garde-fous plus forts.

## 1. En bref

- Le prototype tourne : `recruit _mux` met côte à côte des programmes, chacun dans un terminal émulé par recruit, dessinés par `canvas.rs`, clavier et souris routés, sans tmux ni serveur. Deux vrais Claude Code y passent 77 contrôles sur 77 : 11 vérifications dans 7 montages (5 terminaux annoncés, dont 2 aussi en plein écran), dans un terminal de test sans fenêtre.
- La mémoire décide du moteur : avec 100 000 lignes d'historique à 120 colonnes, alacritty_terminal prend 318 Mio par panneau, libghostty-vt 99 Mio (13 Mio compressé), tmux 42 Mio.
- Les largeurs Unicode aussi : au niveau du moteur, libghostty-vt donne exactement celles de Claude Code sur 9 cas d'emoji ; alacritty se trompe sur 5, dont ❤️ (et donc ⚠️, de la même famille, fréquent chez Claude), et la ligne se décale.
- Performance : tous les seuils tenus (repos, débit de Claude, latence sous flux).
- Recommandation : continuer, avec libghostty-vt comme moteur cible, à condition d'accepter Zig 0.16.0 dans la chaîne de construction (§5).

## 2. Ce qui a été construit

| Fichier | Rôle | Qui |
|---|---|---|
| `src/mux/mod.rs` | `Rect`, `Caps`, `caught` (une panique de moteur rattrapée) | architecte |
| `src/mux/pty.rs` | PTY : `posix_openpt` en `O_NOCTTY \| O_CLOEXEC`, tout préparé avant `fork`, signaux remis à zéro dans l'enfant, un seul fil écrivain non bloquant, récolte par `waitid`, arrêt du groupe (SIGHUP puis SIGKILL après 3 s) | dev-terminal |
| `src/mux/engine.rs`, `engine/alacritty.rs`, `engine/sniff.rs` | trait `Engine`, adaptateur alacritty_terminal, renifleur des OSC qu'alacritty ignore (XTVERSION, 996, 9, 777, 99, 9;4, 7501) | dev-terminal |
| `src/mux/input.rs`, `input/parse.rs`, `input/encode.rs` | lecture maison des octets du terminal (sonde des capacités, réponses tardives jetées), encodeurs clavier (xterm, kitty), souris, collage, focus | dev-saisie |
| `src/canvas.rs` (étendu), `src/mux/screen.rs` | graphèmes entiers, italique, barré, liens OSC 8, curseur, BSU/ESU, repli 256 couleurs ; composition des panneaux | dev-rendu |
| `src/mux/prototype.rs` | la boucle de l'étape 0 : un fil lecteur par PTY, le fil principal route et dessine, rien au repos | architecte, dev-rendu (latence) |
| `tests/common`, `tests/mux_bench.rs`, `tests/mux_claude.rs`, `tests/mux_keys.rs` | banc sans terminal (terminal de test, factices), mesures, grille avec un vrai Claude Code, frappes réelles | testeur |

Relu par reviewer : les interfaces, pty.rs et l'adaptateur alacritty, canvas.rs et screen.rs, input.rs, prototype.rs (protections et correctif de latence), le document serveur. Pas relus : `tests/` (banc, grille, frappes), l'OSC 7501 (renifleur et moteur), les derniers correctifs venus après ses relectures (vidage borné, écrivain non bloquant, VS16 et ZWJ dans le canvas, collage borné, `Event::Closed`, mode 2027), et l'évaluation de libghostty-vt (hors de l'arbre). Les mesures ne sont pas refaites par lui.

Vérifications : fmt, clippy sans avertissement (macOS ; clippy aussi pour Linux musl), 311 tests unitaires en français et en anglais. Les 13 tests d'intégration (`tests/mux*.rs`) sont en `#[ignore]` et se lancent à la main. Les tests de l'existant passent ; aucune équipe tmux n'a été relancée pour le vérifier. Rien n'est commité.

## 3. Compatibilité avec Claude Code (§6 de la spec)

Claude Code 2.1.295, à travers `_mux`, terminal de test sans fenêtre (campagne 1 de la grille). Les cinq vrais terminaux (Ghostty, iTerm2, Terminal.app, kitty, WezTerm) : à venir, par les frappes réelles.

| Point | Résultat | Note |
|---|---|---|
| Rendu | ok en partie | R1, R2, R5, R8 : défaut et plein écran, redimensionnement 200×50 → 150×40 → 200×50 sans reste, mises à jour synchronisées demandées et tenues. À venir : R3 (régions de défilement), R4 (historique en mode normal), R6 et R7 (BSU sans fin, requêtes pendant un BSU), R9 (scintillement), R10 (curseur) |
| Clavier | ok en partie (sans fenêtre) | Shift+Entrée, Ctrl+C, Shift+Tab, accents ; kitty poussé par Claude (drapeaux 5) et suivi. À venir : Ctrl+B, Ctrl+R, Ctrl+O, Échap, Tab, flèches. Terminal.app n'a ni kitty ni modifyOtherKeys : Shift+Entrée y envoie le message, comme sans multiplexeur |
| Collage | à venir | encadrement et nettoyage testés au banc (tests unitaires) ; grand collage par les frappes réelles |
| Souris | à venir | encodeurs testés au banc (tests unitaires) |
| Liens | ok au banc (L1, L3) | OSC 8 relayé, retiré proprement sans support ; Claude n'en écrit qu'aux terminaux de sa liste, et pas à `recruit` (§6) |
| Presse-papiers | ok au banc (P1) | OSC 52 relayé ; pas encore vu dans un vrai presse-papiers |
| Notifications | contournement (N1, banc) | relayées en OSC 9. Claude n'en envoie, d'après la lecture de son code (à confirmer), qu'à Ghostty, kitty et iTerm2, et BEL à Terminal.app ; on les fera nous-mêmes à partir de l'OSC 7501 (§7) |
| Focus | à venir | |
| Couleurs | à venir | 24 bits vus à l'écran ; OSC 10 et 11 répondus par le moteur (tests unitaires). Claude ne demande jamais OSC 11 au démarrage : la détection du thème clair ou sombre (Co2) n'est pas vérifiée |
| Unicode | ok en partie | U1, U2 (CJK, emoji simples) ; U3 à U7 à venir. Les 9 cas d'emoji du §5 sont mesurés au niveau du moteur, pas dans le prototype ni dans un vrai terminal. Une case de 2 colonnes (👍🏽, famille, ❤️) part telle quelle au vrai terminal : un terminal qui la mesure autrement (Terminal.app) peut déborder sur la case voisine, non vérifié |
| Performance | ok | §4 : tous les seuils tenus |
| Terminal annoncé | ok | §6 |
| Mod et barres | étape 1 | |
| Teammates | étape 1 | |
| Réglages | ok | `~/.claude/settings.json` inchangé après chaque session |

## 4. Mesures

### Mémoire par panneau, 100 000 lignes, sortie réelle de Claude (Mio)

| Colonnes | alacritty 0.26 | tmux 3.8 | libghostty-vt | libghostty-vt compressé |
|---|---|---|---|---|
| 80 | 200 | 35 | 67 | 10 |
| 120 | 318 | 42 | 99 | 13 |
| 200 | 507 | 66 | 165 | 14 |

Grille du testeur : empreinte physique (ce que montre le Moniteur d'activité), pente de 1 à 10 panneaux, rejeu des transcriptions du projet ; libghostty-vt mesuré avec le binaire d'essai de dev-terminal, recontrôlé par le testeur. Dix panneaux à 120 colonnes : 3,1 Gio (alacritty), 0,42 Gio (tmux), 1 Gio ou 0,13 Gio compressé (libghostty-vt). La compression complète prend 44 à 278 ms : à faire hors du dessin, en étapes incrémentales.

Compression de libghostty-vt pendant un flux (grille, 120 colonnes, 30 s) : en compressant après chaque écriture, la mémoire reste sous 33 Mio, pour 0 à 3,6 % de CPU et une écriture p99 de 0,4 à 1 ms (de 20 Kio/s à 2 Mio/s) ; en compressant seulement au repos, le CPU est plus bas (0 à 1,2 %), mais la mémoire monte jusqu'à 119 Mio pendant un long flux, puis redescend à 22 Mio en 5 à 50 ms de travail.

Défilement dans un historique compressé de libghostty-vt (mesuré par dev-terminal, jusqu'à l'image lue, sans l'envoi) : écran vivant 140 à 300 µs ; molette p50 84 à 175 µs ; tout en haut 170 à 290 µs ; page par page dans tout l'historique p50 ~150 µs, p99 ~8 ms, pics de 20 à 32 ms.

### Latence ajoutée à la frappe (mux moins direct)

| Montage | p50 | p99 | seuil p99 | tmux, même passe |
|---|---|---|---|---|
| 1 panneau, avant correction (testeur) | +0,3 ms | +8,5 ms | 16 | +0,08 / +0,18 ms |
| 10 panneaux, 9 en flux, avant correction (testeur) | +10,4 ms | +51,4 ms | 33 | +7,8 / +13,4 ms |
| 10 panneaux, 9 en flux, après correction (passes de dev-rendu, charge 13 à 17) | +0,85 à +0,9 ms | +2,7 à +27,7 ms | 33 | |
| **Passe finale du testeur**, 1 panneau (charge 4 à 10) | +0,25 ms | +1,6 ms | 16 | |
| **Passe finale**, 10 panneaux, 9 en flux | +1,5 ms | +4,3 ms (max 32) | 33 | +7,8 / +13,9 ms |
| **Passe finale**, 9 panneaux en images synchronisées de 120 Ko toutes les 50 ms | | +4,7 ms (max 8,7) | 33 | p99 +10,8 ms (max 29,7) |

La correction (dev-rendu) : les lecteurs nourrissent leur moteur par tranches de 4 Kio et cèdent le tour à la boucle (le verrou n'est pas équitable sous macOS : la boucle attendait jusqu'à 115 ms) ; la réponse du panneau actif est dessinée tout de suite dans les 250 ms qui suivent une frappe ; la cadence part du début de l'image. Pour des images synchronisées jusqu'à 120 Ko, l'analyse d'un coup à l'ESU ne fait pas de queue (vte peut en retenir jusqu'à 2 Mio). Échap : +11,2 ms sans kitty (l'attente voulue de 10 ms, comme `escape-time 10` sous tmux ; +15 ms en p99 sous flux), +0,24 ms avec kitty (+1,35 ms sous flux).

### CPU au repos et en flux

Processus mux, ou serveur tmux seul, passe finale du testeur (charge 4 à 10) :

| Montage | mux CPU | tmux CPU | mux images/s | tmux images/s |
|---|---|---|---|---|
| 2 vrais Claude au repos, 60 s | 0,00 %, 0 réveil/s, 48 octets | 0,00 %, 280 octets | 0 | 0 |
| 10 factices au repos, 60 s | 0,00 %, 0 réveil/s, 0 octet | 0,00 % | 0 | 0 |
| 1 panneau au débit de Claude (20 Ko/s, BSU toutes les 16 ms) | 2,1 % | 1,05 % | 54 | 48 |
| 9 panneaux au débit de Claude | 3,8 % | 3,7 % | 60 | 354 |
| 1 panneau au débit maximal | 73 % | 96 % | 54 | 1018 |
| 9 panneaux au débit maximal | 377 % | 97 % | 54 | 386 |

Au débit maximal, le mux lit bien plus que tmux : 64 Mio/s contre 24 pour un panneau, 149 Mio/s contre 26 pour neuf (tmux plafonne à un cœur). Rapporté aux octets lus, il coûte moins : 0,011 s de CPU par Mio contre 0,040 pour un panneau, 0,025 contre 0,038 pour neuf. Au repos, rien : aucun réveil, aucun octet. Une image coûte ~0,45 ms au fil principal, ~11 Ko envoyés (mesure de dev-rendu).

### Poids et construction (mesures de dev-terminal)

| | alacritty_terminal | libghostty-vt |
|---|---|---|
| Binaire release macOS arm64 | +113 Kio (+3,6 %) | +1,0 Mio |
| Binaire Linux musl x86_64 | +128 Kio | +1,0 Mio |
| Construction release propre | +5 s | +1 à 1,5 min par cible |
| Outils | aucun | Zig 0.16.0 exactement |
| Rust minimal | 1.85 | 1.90 (le projet est en 1.88 : à monter) |
| Licence | Apache-2.0 | Ghostty : MIT ; libghostty-rs : MIT ou Apache-2.0 ; les 4 paquets Zig : à vérifier |

## 5. Moteur VT

| | alacritty_terminal 0.26 | libghostty-vt (libghostty-rs, Ghostty 22d13172) |
|---|---|---|
| Largeurs (9 cas d'emoji, comparés à Claude Code, au niveau du moteur) | 4 justes sur 9 : 👍🏽 sur 4 colonnes, famille ZWJ sur 6, ❤️ ☺️ 1️⃣ sur 1 | 9 sur 9 (mode 2027) |
| Mémoire, 120 colonnes | 318 Mio | 99 Mio, 13 compressé |
| Mises à jour synchronisées | retenues par le parseur ; les requêtes d'un BSU attendent l'ESU | à l'intégrateur de geler l'image ; requêtes répondues tout de suite |
| Encodeurs clavier, souris, collage | non | oui |
| API | Rust pur, mûre | pré-1.0 : déjà cassée de 0.2.2 à master ; dépendance git non publiée ; types `!Send` |
| Construction | rien à installer | Zig 0.16.0 épinglé ; sources de Ghostty en archive (sans réseau, vérifié) ; macOS 27 et musl (x86_64, aarch64) se construisent, binaires Linux non exécutés |
| Bug connu | panique au-delà de 4096 poussées kitty (alacritty#8957, fermé sans correctif) : rattrapée par le prototype | — |

**Recommandation : libghostty-vt.** Il tient les deux exigences qu'alacritty ne tient pas (largeurs, mémoire). Ses coûts :

- **5 à 7 jours** (estimation de dev-terminal) : construction, script de l'archive, CI et `release.sh`, 1 à 1,5 jour ; adaptateur, 3 à 4 jours, y compris le modèle de fils, puisque ses types sont `!Send` alors que le trait est `Engine: Send` : un fil par moteur qui publie un instantané pour le dessin, ou un `unsafe impl Send` justifié, ce qui change le modèle relu (Gate, `guarded`) ; ses encodeurs à la place d'une partie des nôtres, 1 jour, si dev-saisie le juge utile.
- **Zig 0.16.0 pour toute construction** : CI Linux et macOS, `release.sh` (brew `zig@0.16`, à la place du Zig courant de cargo-zigbuild), contributeurs, construction depuis les sources. À trancher : Zig obligatoire, ou alacritty gardé en repli derrière la fonction par défaut, avec deux moteurs à tenir. Ma recommandation : alacritty pendant la transition de l'étape 1, puis Zig obligatoire et alacritty retiré.
- **L'archive des sources de Ghostty** (38 Mo) et ses 4 paquets Zig (6 Mo) : hébergés par nous, par exemple en pièce jointe d'une release GitHub dédiée de ronalove/recruit, ce qui touche `release.sh` et la CI.
- **Une dépendance pré-1.0 à monter avec soin** : le commit de Ghostty et la version de Zig ensemble ; un futur SDK d'Apple peut casser une version de Zig, comme l'ont fait les SDK à partir de Xcode 26.4 pour Zig 0.15 (zig#31658, corrigé en 0.16.0).

**Plan B**, si Zig n'est pas voulu dans la chaîne : alacritty, plus un historique compact à nous (les lignes sorties de l'écran gardées sans leurs cases vides : 8 à 12 Mio pour 100 000 lignes, environ 500 lignes de code, estimation de dev-terminal), en acceptant les largeurs fausses sur les emoji composés.

Le trait `Engine` isole le moteur : l'étape 1 (serveur, client) peut commencer avec alacritty pendant que libghostty-vt est branché.

## 6. Terminal annoncé aux panneaux

`TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=recruit`, `TERM_PROGRAM_VERSION=<version de recruit>`. Le moteur répond à `CSI ? u` (Claude active alors le protocole kitty, donc Shift+Entrée), à XTVERSION (il demande alors le mode 2026) et à `OSC 7501 ; ?` (§7). On ne se fait pas passer pour Ghostty : d'autres programmes des panneaux, shells compris, réagiraient au nom. Vérifié par la grille : les mêmes 11 contrôles passent avec ou sans nom annoncé ; la synchro vient de la réponse XTVERSION, pas du nom.

Liens : Claude n'écrit d'OSC 8 qu'aux terminaux de sa liste, ou avec `FORCE_HYPERLINK=1`. Cette variable n'est pas retenue : elle passerait à tout le panneau, commandes de l'outil Bash comprises, dont la sortie capturée ou redirigée pourrait alors contenir des séquences OSC 8 (non vérifié). Les liens de Claude dans les panneaux restent donc une question ouverte de l'étape 1.

L'environnement des panneaux est nettoyé de `TMUX`, `TMUX_PANE`, des variables propres au vrai terminal et des marques de la session Claude qui aurait lancé recruit (`CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`…) : hérité hors tmux, `CLAUDE_CODE_CHILD_SESSION` coupe l'enregistrement des transcriptions, donc la reprise des membres.

## 7. Découvertes utiles pour la suite

- **OSC 7501, l'état donné par Claude Code lui-même.** Quand le terminal répond à `OSC 7501 ; ?`, Claude écrit son état à chaque changement : `idle`, `working`, `blocked` avec la raison (`kind=permission`, message), `done`, `clear`, sans délai mesurable (vérifié en vrai par dev-terminal). En natif, le tableau de bord interrogera `claude agents` moins souvent (il reste nécessaire pour l'identifiant de session et la commande en cours), et les notifications de la direction « Cadres » partiront au moment exact du passage en attente. Déjà dans le moteur (`Relay::Status`) ; le prototype l'affiche dans le titre de chaque panneau.
- **Doublons de noms hors tmux (question 1).** ListAgents ne montre plus « tmux <session>:… ». La ref d'une session se calcule depuis `<profil>/sessions/<pid>.json` (6 premiers hex de `sha256("session:" + messagingSocketPath)`), et `nom [ref]` arrive à la bonne session sans listing. Formule interne à Claude Code : revérifiée par la grille à chaque version.
- **Réglage de l'utilisateur** : `"tui": "fullscreen"` dans `~/.claude/settings.json` met les membres en plein écran par défaut ; les tests choisissent le rendu par session (`CLAUDE_CODE_NO_FLICKER=1` ou `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1`), jamais par `/tui`.
- **Ghostty a le mode 2027 actif par défaut** (sonde de l'utilisateur) : avec un moteur qui ne regroupe pas les graphèmes, le client le coupe à l'ouverture et le rend à la sortie (fait).
- **Raccourcis** : ⌥o est pris par Claude Code (mode rapide) ; ⌥n le remplace dans les maquettes. ⌥z, ⌥g, ⌥r, ⌥j, ⌥q, ⌥1…9 sont libres chez Claude.

## 8. Risques restants

- libghostty-vt : voir §5.
- L'écriture vers le vrai terminal reste bloquante dans le prototype (pointes de 15 à 53 ms quand il lit lentement) : un fil écrivain avec une seule image en vol à l'étape 1 (multiplexeur-serveur.md §6.2).
- Les cinq vrais terminaux, la souris, le collage, OSC 52, OSC 8 : à venir.
- **Incident du premier passage de frappes réelles.** L'instance de Ghostty lancée pour le test a rouvert les fenêtres enregistrées de l'application (réglage par défaut), et l'une d'elles, avec un zsh neuf, a pris le focus : les frappes y sont allées, au lieu de la fenêtre du test. Le test vérifiait avant chaque touche que l'application au premier plan était cette instance de test (son pid), donc ni l'instance Ghostty de l'utilisateur ni une autre application ne pouvaient recevoir les touches ; la fenêtre clé, elle, n'était pas vérifiée. Trace laissée : deux lignes `;2;13~\` à la fin de `~/.zsh_history` (Shift+Entrée mal lu par zsh, exécuté en erreur de syntaxe), sans horodatage, donc sans preuve formelle de la fenêtre qui les a reçues. Aucune ligne des collages n'a été exécutée ; toutes les sessions Claude sont restées en vie ; le presse-papiers a été rendu. Garde-fous ajoutés avant tout nouveau passage : pas de fenêtres restaurées, fenêtre active vérifiée par son titre avant chaque touche, touche témoin avant chaque touche risquée, touches risquées seulement sur demande explicite, Terminal.app et iTerm2 sautés s'ils sont ouverts.
- Linux : construit et vérifié par clippy pour musl, pas exécuté (la CI le fera).
- SSH et détacher-revenir : étape 1, sur le papier dans `multiplexeur-serveur.md`.

## 9. Décisions demandées à l'utilisateur

1. **Continuer ?** Recommandation : oui.
2. **Moteur (question 6)** : libghostty-vt avec Zig 0.16.0 dans la chaîne (coûts au §5), ou alacritty avec un historique compact (largeurs fausses sur les emoji composés). Recommandation : libghostty-vt, alacritty gardé pendant la transition de l'étape 1 puis retiré.
3. **Doublons de noms (question 1)** : donner à chaque membre les adresses exactes (`nom [ref]`) de ses coéquipiers dans la note d'équipe, calculées depuis les fichiers de session. Inconvénients : la formule est interne à Claude Code, et un drapeau (`tengu_session_stable_address`) la fera changer (la résolution acceptera alors les deux formes) ; la ref change à chaque relance d'un membre (nouveau pid, nouveau socket), donc celles que ses coéquipiers ont en contexte sont périmées jusqu'à la note suivante, et un pid recyclé pourrait même mener une ref périmée vers une autre session ; l'appliquer aussi sous tmux remplacerait la décision du 2026-10-07 (« tmux <session> »). Recommandation : oui, en natif d'abord, sous tmux aussi si l'utilisateur le veut, avec le test de la formule à chaque version de Claude Code.
4. **Deuxième `attach` (question 3)** (détails : multiplexeur-serveur.md §5.4) :
   - A, il reprend la main : simple, fait ce qu'on attend après un SSH mort ; mais un `recruit` lancé par erreur coupe le premier terminal sans prévenir.
   - A', il demande d'abord, et reprend sans demander hors d'un terminal interactif ou avec `--force` : protège du lancement par erreur ; une question de plus.
   - B, refusé : le premier n'est jamais coupé ; mais après un SSH mort non détecté, l'utilisateur est bloqué jusqu'à `--force`.
   - C, écran partagé : comme tmux ; hors périmètre de la spec, le plus petit terminal rétrécit l'autre, deux dessins à tenir justes, coût nettement plus grand.
   Recommandation : A'.
5. **Après un plantage du serveur** : reprendre les conversations des membres sans demander, en le disant, avec un garde-fou contre les plantages en boucle (pas deux reprises automatiques en quelques minutes : la seconde demande). Recommandation : oui.

## 10. Proposition pour l'étape 1

Si l'utilisateur dit de continuer, l'étape 1 (spec §8 : serveur et client, une équipe complète lancée par `RECRUIT_BACKEND=native recruit`, chrome minimal ; sortie : une vraie équipe tient une journée de travail en natif) se découperait ainsi :

| Qui | Travail |
|---|---|
| architecte | trait `Backend` agrandi (primitives, `multiplexeur-serveur.md` §4.3), `Snapshot.backend`, intégration dans `launch.rs`, `app.rs`, `live.rs` ; `layout.rs` qui rende des rectangles ; prototype retiré |
| dev-serveur | `server.rs`, `client.rs`, `proto.rs`, `Native` : processus, socket, protocole, détacher et revenir, `recruit _ctl` pour les tests |
| dev-terminal | selon la décision : libghostty-vt (construction épinglée, adaptateur, compression au repos) ou historique compact pour alacritty ; identifiant stable des panneaux |
| dev-saisie | types d'événements sérialisables, lecture côté client, suivi du mode souris du panneau actif (1003) |
| dev-rendu | fil écrivain avec une seule image en vol, `Painter::frame(&Canvas)`, vue de l'historique, sélection et copie (OSC 52) |
| dev-interface | chrome minimal dans la direction « Cadres » : cadres et en-têtes des panneaux, barre d'onglets en bas ; tableau de bord et journal encore en processus dans un panneau |
| testeur | banc par le socket, frappes réelles avec les nouveaux garde-fous, grille dans les cinq terminaux, SSH, journée de travail |
| reviewer | relecture de chaque rendu, code système en priorité ; les parties non relues de l'étape 0 (§2) |
| designer | rien jusqu'à l'étape 3, sauf une question de dev-interface |

Le moteur est derrière le trait `Engine` : le serveur et le client peuvent avancer avec alacritty pendant que libghostty-vt est branché.
