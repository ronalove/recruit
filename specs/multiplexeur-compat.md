# Grille de compatibilité du multiplexeur natif

Tenue par le testeur. Une ligne par point du [§6 de la spec](multiplexeur.md#6-compatibilité-avec-claude-code), découpé en vérifications qu'on peut rejouer. Rejouée à chaque étape (§8) et à chaque mise à jour de Claude Code ; chaque passage ajoute une campagne en bas, sans effacer les précédentes.

Résultats : **ok** · **ko** (avec la note) · **cont.** (marche par un contournement, décrit dans la note) · **—** pas encore passé · **n/a** sans objet pour ce terminal.

## Mise en place commune

Chaque passage se fait sur une copie du binaire, avec des dossiers à part, jamais les équipes ni le serveur tmux de l'utilisateur :

```sh
T=$(mktemp -d)                      # tout le passage vit ici
R=$(mktemp -d /tmp/rt.XXXX)         # RECRUIT_TMPDIR court : le socket tient en 103 octets
cp target/debug/recruit "$T/recruit"
export XDG_CONFIG_HOME="$T/config" XDG_CACHE_HOME="$T/cache" RECRUIT_TMPDIR="$R"
mkdir -p "$T/proj/.recruit" && cd "$T/proj"   # une équipe de test (settings.toml), git init
shasum ~/.claude/settings.json > "$T/settings.before"
RECRUIT_BACKEND=native "$T/recruit" --detach   # l'équipe en natif ; puis recruit attach, list, stop
# … vérifications : recruit _ctl <état> capture|send|key|clients|stats, un client sur le terminal …
"$T/recruit" stop <équipe>
shasum -c "$T/settings.before"      # S1 : doit dire OK
rm -rf -- "$R" "$T"
```

- **Deux verrous**, qu'un `cargo test -- --ignored` (sur tout le dépôt, ou par erreur) n'ouvre pas : `KEYS_CONSENT=$(date +%F)` pour les frappes réelles (`tests/mux_keys.rs`), `CLAUDE_SESSIONS=$(date +%F)` pour les vraies sessions Claude (`tests/mux_claude.rs` et la ligne « 2 Claude at rest » du banc). La date du jour, posée seulement après le go de l'utilisateur pour ce jour-là, et le test nommé sur la ligne de commande (`real_keys`, `claude_modes`…) : sans les deux, le test dit pourquoi et ne fait rien, même avec la variable encore exportée. Ajoutés après le passage accidentel du 2026-10-09.
- **Dans un tmux** : `tmux -L rtest-testeur new -s compat`, puis la même chose dans le panneau. Jamais `-L recruit`.
- **Par SSH** : le même binaire sur la machine distante (ou `ssh` vers cette machine), lancé dans la session SSH.
- **Référence** : chaque point se compare au même `claude` lancé directement dans le terminal, sans multiplexeur. Ce qu'on ne voit pas en direct non plus n'est pas un ko du multiplexeur ; la note le dit.
- **Programmes factices** : `tests/` (banc d'intégration) écrit les séquences connues ; les lignes marquées *banc* se vérifient sans terminal, une fois pour toutes, et aussi à la main dans chaque terminal. Un serveur de test se lance par `recruit --lang fr _server <état>` (la langue donnée : sous macOS, la détecter démarre des fils avant le fork), ses panneaux par `_ctl <état> spawn <membre> -- <commande>`, un client par `_ctl <état> attach` ; `tests/common/server.rs` le fait.
- **Tests actifs** (depuis la fin de l'étape 1, 2026-10-09 ; le prototype `_mux` est retiré) : `tests/mux.rs` (3 : relais vers le terminal, avec et sans XTVERSION ; aucune image partielle) et `tests/mux_server.rs` (9 : spawn, send, capture, stop et leurs codes ; socket privé et court ; client parti puis revenu ; deuxième attach ; client tué ; serveur tué ; redimensionnement ; touches avec et sans kitty), et depuis la campagne 2 `tests/mux_screen.rs` (4, 1,4 s : un panneau tel qu'écrit ; couleurs sans 24 bits ; historique dans l'ordre ; focus et clics entre deux panneaux, avec le factice `script`), tournent dans `cargo test`, donc dans la CI, sous macOS et Linux. Environ 10 s de plus. Vérifiés 10 fois de suite en parallèle sous macOS (FR et EN) et sous Linux (Alpine 3.22.6, aarch64-musl, `--init`). Restent ignorés : le banc (`tests/mux_bench.rs`, plusieurs minutes, en release), les vraies sessions (`tests/mux_claude.rs`, verrou), les frappes réelles (`tests/mux_keys.rs`, verrou, macOS), et `ssh_attach`, `attach_existing` (à la main).
- **Touches de chaque terminal** : ce qu'un terminal envoie pour chaque touche est relevé une fois (programme de relevé en mode brut, protocole kitty demandé comme le fait crossterm) et rangé dans les données du banc. Le banc rejoue ces octets dans le client : l'encodage vers le panneau se teste alors sans le terminal.
- Effets connus des vraies sessions sur le profil de l'utilisateur : la confiance accordée au dossier de test s'écrit dans `~/.claude.json` (une entrée sous `projects`, pour `$TMPDIR/recruit-testeur-claude` et les dossiers de la campagne 0), et chaque session laisse sa transcription dans `~/.claude/projects/<dossier>/`. `~/.claude/settings.json`, lui, ne doit jamais changer (S1).
- Les programmes lancés par les tests (factices, multiplexeur, Claude, terminaux) n'héritent d'aucune marque de terminal, de multiplexeur ni de session Claude (`SCRUB` de `tests/common/pty.rs`, la même liste que `src/mux/server/pane.rs`) : les tests tournent dans le panneau du testeur, donc dans le tmux de l'utilisateur et dans une session Claude.
- Les vraies sessions Claude restent courtes, avec des demandes simples (« écris trois paragraphes sur les chats », « lance `seq 1 2000` »).

## Points et vérifications

### R : rendu

| ID | Vérification | Reproduire |
|---|---|---|
| R1 | Mode par défaut : accueil, saisie, réponse longue en flux, sans cellule fantôme ni ligne cassée. | Demander une réponse de 60 lignes ; comparer à la référence. |
| R2 | Plein écran (`/tui fullscreen`) : écran alterné, retour à l'écran normal intact en sortant. | `/tui fullscreen`, une réponse, puis retour (`/tui` par défaut) ; l'historique d'avant est là. |
| R3 | Régions de défilement (DECSTBM) : une réponse longue défile en plein écran sans déborder sur la saisie. | Plein écran, réponse de 200 lignes. *Banc* : factice qui pose une région et défile. |
| R4 | Mode par défaut : les lignes qui sortent du haut vont dans l'historique du panneau, dans l'ordre. | `! seq 1 2000`, puis molette jusqu'en haut : 1 à 2000 sans trou. |
| R5 | Mise à jour synchronisée : DECRQM `?2026` répond « reconnu » ; aucune image à moitié. | *Banc* : factice qui écrit BSU, un écran rempli en 10 morceaux espacés, ESU ; le client de test ne voit jamais d'état intermédiaire. |
| R6 | Délai de garde du mode 2026 : un BSU sans ESU n'arrête pas le dessin plus que le délai prévu. | *Banc* : BSU seul, puis texte ; l'image arrive après le délai (mesuré). |
| R7 | Requêtes pendant un BSU (vte 0.15 : DA1, CPR, etc. répondus à l'ESU ou après 150 ms) : Claude Code n'en souffre pas (démarrage, redimensionnement, saisie). | Temps de lancement jusqu'à l'invite, comparé à la référence (10 lancements, médiane) ; `strace`/journal du moteur : requêtes reçues pendant un BSU et délai de réponse. *Banc* : BSU, DA1, mesure du délai de réponse. |
| R8 | Redimensionnement : agrandir, rétrécir, plein écran et défaut ; pas de cellule fantôme, Claude redessine, `TIOCSWINSZ` reçu. | Changer la taille du terminal 10 fois pendant une réponse en flux. |
| R9 | Pas de scintillement : indicateur animé et flux, observés 30 s. | Observation, et *banc* : chaque image envoyée au client est entourée de BSU/ESU (compté). |
| R10 | Curseur : celui du panneau actif (position, forme DECSCUSR, visibilité), caché ailleurs. | Changer de panneau ; saisie au milieu d'une ligne ; plein écran. |

### K : clavier

| ID | Vérification | Reproduire |
|---|---|---|
| K1 | Shift+Entrée va à la ligne sans envoyer. | Taper `a`, Shift+Entrée, `b` : deux lignes dans la saisie. |
| K2 | Alt/Option : Alt+b et Alt+f déplacent d'un mot ; Alt+Entrée. | Dans la saisie. (Option comme Alt est un réglage du terminal, prérequis.) |
| K3 | Ctrl+B arrive à Claude Code (tâche en arrière-plan) : pas de préfixe, décision de l'utilisateur du 2026-10-09. | `! sleep 30`, puis Ctrl+B : la commande passe en arrière-plan. *Frappes* : le panneau reçoit `0x02`. |
| K4 | Ctrl+R : recherche dans l'historique des invites. | Ctrl+R après deux invites. |
| K5 | Ctrl+O : transcription détaillée. | Après une réponse avec outil. |
| K6 | Ctrl+C : interrompt ; deux fois sur une saisie vide, quitte. | Pendant une réponse, puis à vide. |
| K7 | Ctrl+D : quitte sur une saisie vide. | — |
| K8 | Échap : interrompt sans délai perceptible ; double Échap (revenir en arrière). | Pendant une réponse ; délai mesuré au banc (latence, voir le banc). |
| K9 | Tab : complétion. | `@` puis début d'un nom de fichier, Tab. |
| K10 | Shift+Tab : change le mode de permission. | La ligne du mode change. |
| K11 | Flèches : historique des invites, choix dans les listes, en mode normal et application (DECCKM). | Haut après une invite ; `/model` puis flèches. |
| K12 | Protocole clavier kitty : Claude Code le demande (`CSI > flags u`), le panneau reçoit les touches encodées selon ses drapeaux, il est retiré à la sortie. | Journal des modes du moteur ; *banc* : factice qui pousse et retire des drapeaux et relit les touches. |
| K13 | Lettres accentuées et caractères du clavier de l'utilisateur (QWERTY French 2.0, Cmd droit en Option droite) : é, è, à, ç, œ, «, ». | Taper `é è à ç œ « »` dans la saisie. |
| K14 | Les raccourcis du multiplexeur ne prennent rien à Claude Code (liste des raccourcis pris, comparée à ceux de Claude Code). | Chaque raccourci du multiplexeur dans un panneau où Claude attend une saisie. |
| K15 | Raccourcis du multiplexeur (⌥1…9, ⌥⇧←/→, ⌥r, ⌥j, ⌥q, et ceux proposés : ⌥o, ⌥z, ⌥g) contre la table de Claude Code (`p1` du binaire, `~/.claude/keybindings.json`) et les liaisons par défaut de zsh, bash, fish. | Table relevée dans le binaire de chaque version ; chaque touche envoyée en octets bruts (`ESC` + lettre, `CSI 1;4 D/C`) à une saisie remplie, écran relu ; `bindkey -e` (zsh -f), `bind -p` (bash --norc). |

### C : collage

| ID | Vérification | Reproduire |
|---|---|---|
| C1 | Collage encadré (mode 2004) quand l'application le demande, brut sinon. | Coller trois lignes : Claude affiche « [Pasted text …] » comme en référence. *Banc* : 2004 posé et retiré. |
| C2 | Grand collage (plus de 100 Ko) : arrive entier, sans blocage ni perte. | `pbcopy < fichier de 150 Ko`, coller ; *banc* : octets reçus = octets envoyés. |
| C3 | Retours à la ligne gardés (CR, LF, CRLF), tabulations gardées. | Coller un texte avec les trois fins de ligne. |
| C4 | Image du presse-papiers (Ctrl+V) : Claude la lit lui-même sur la machine ; rien à relayer, mais pas de séquence perdue. | Copier une capture d'écran, Ctrl+V. Par SSH : comme en référence. |

### M : souris

| ID | Vérification | Reproduire |
|---|---|---|
| M1 | Clics : relayés dans les modes demandés (1000, 1002, 1003, 1006), coordonnées dans le panneau. | Plein écran : clic dans la liste ; *banc* : clics à des positions connues dans un panneau décalé. |
| M2 | Molette : à l'application si elle capture la souris, sinon défilement de l'historique. | Plein écran (capturée) puis mode par défaut (historique). |
| M3 | Sélection en plein écran (celle de Claude Code) et sélection du multiplexeur ailleurs. | Glisser sur un texte ; vérifier le presse-papiers. |
| M4 | Liens cliquables (voir L1). | — |
| M5 | Un clic dans un panneau inactif lui donne le focus sans être perdu ni envoyé deux fois. | Clic dans l'autre panneau. |

### L : liens

| ID | Vérification | Reproduire |
|---|---|---|
| L1 | OSC 8 relayés au vrai terminal (Cmd+clic ou clic selon le terminal). | Demander à Claude un lien vers un fichier ; *banc* : OSC 8 en entrée, OSC 8 identique en sortie. |
| L2 | Pas de double ouverture (le multiplexeur n'ouvre rien lui-même quand le terminal le fait). | Cmd+clic : un seul onglet ouvert. |
| L3 | Repli : terminal sans OSC 8, le texte reste, le lien est retiré proprement. | Terminal.app. |

### P : presse-papiers

| ID | Vérification | Reproduire |
|---|---|---|
| P1 | La copie de Claude Code (OSC 52) arrive dans le presse-papiers du système. | `/copy` après une réponse, puis `pbpaste`. |
| P2 | La copie d'une sélection du multiplexeur part en OSC 52. | Sélection à la souris, puis `pbpaste`. |
| P3 | Par SSH et dans un tmux : OSC 52 arrive au terminal local. | Mêmes gestes. |

### N : notifications

| ID | Vérification | Reproduire |
|---|---|---|
| N1 | BEL, OSC 9, OSC 777, OSC 99 relayés au vrai terminal. | Claude qui attend une permission (notification) ; *banc* : chaque séquence en entrée, la même en sortie. |
| N2 | OSC 9;4 (progression) relayé. | *Banc* ; Claude Code pendant un tour, si le terminal l'affiche. |
| N3 | Titre (OSC 0, 2) : relayé ou gardé selon la spec. | Titre que pose Claude Code. |
| N4 | Visibles dans l'interface (badge sur le membre et l'onglet). | Étape 1 et suivantes. |

### F : focus

| ID | Vérification | Reproduire |
|---|---|---|
| F1 | Changement de panneau : `CSI O` au panneau quitté, `CSI I` au panneau rejoint, seulement s'ils ont demandé 1004. | *Banc* ; Claude Code (atténue son curseur hors focus). |
| F2 | Le terminal perd puis reprend le focus : relayé au panneau actif. | Passer à une autre application et revenir. |

### Co : couleurs

| ID | Vérification | Reproduire |
|---|---|---|
| Co1 | 24 bits : le dégradé et les couleurs de Claude Code comme en référence. | Accueil de Claude ; *banc* : SGR 38;2 et 48;2 en entrée, mêmes couleurs en sortie. |
| Co2 | Thème clair ou sombre détecté comme dans le vrai terminal (OSC 11). | Thème `auto` de Claude ; terminal en clair puis en sombre. |
| Co3 | OSC 10 et 11 répondus avec les couleurs du vrai terminal (demandées une fois par le client). | *Banc* : requête du factice, réponse égale à celle du terminal. |
| Co4 | Repli en 256 couleurs quand le terminal n'a pas le 24 bits. | `COLORTERM` absent, ou terminal en 256. |

### U : Unicode

| ID | Vérification | Reproduire |
|---|---|---|
| U1 | CJK (deux colonnes) aligné. | Demander « 漢字かなカナ 한국어 » dans une liste à colonnes. |
| U2 | Emoji simples. | « 🙂 🚀 ✅ ». |
| U3 | Séquences ZWJ, drapeaux et teintes de peau, comme le vrai terminal (une ou plusieurs cases selon lui, mode 2027). | « 👩‍💻 👨‍👩‍👧 🏳️‍🌈 🇫🇷 👍🏽 » ; *frappes* : la sonde du factice `keylog` écrit 👍🏽 et demande la position du curseur, en direct (réponse du terminal) et via le mux (réponse du moteur) : les colonnes doivent être égales. |
| U4 | Accents combinants (e + U+0301) dans une case. | Texte décomposé (NFD). |
| U5 | Indicateur animé de Claude Code : aligné, sans reste. | Pendant un tour. |
| U6 | Mode 2027 (graphèmes) : demandé et répondu selon le vrai terminal. | DECRQM `?2027` dans le panneau ; *banc*. |
| U7 | Dessin de boîtes et blocs (cadres de Claude Code, barres). | Accueil, `/model`. |

### Pf : performance

Chiffres du banc (voir « Banc de performance » ci-dessous), reportés à chaque campagne.

| ID | Vérification | Reproduire |
|---|---|---|
| Pf1 | CPU au repos (2 Claude au repos, et 10 factices au repos) : rien, aucun réveil. | Banc. |
| Pf2 | CPU en flux : 1 et 10 panneaux qui écrivent en continu, fluides. | Banc. |
| Pf3 | Latence ajoutée à la frappe. | Banc. |
| Pf4 | Mémoire par panneau avec 100 000 lignes d'historique (y compris les 2 Mio de mémoire virtuelle que chaque `Processor` de vte réserve). | Banc. |

### T : terminal annoncé

| ID | Vérification | Reproduire |
|---|---|---|
| T1 | `TERM` donné aux panneaux (xterm-256color, xterm-ghostty…) : ce que Claude Code en fait (couleurs, protocole clavier). | `--term`, une valeur à la fois. |
| T2 | `TERM_PROGRAM` et `TERM_PROGRAM_VERSION` : quatre variantes jouées à chaque campagne, aucun ; `ghostty` sans version ; `ghostty` 1.2.3 ; `tmux`. Effets attendus d'après le code de 2.1.295, à confirmer : une réponse à `CSI ? u` active le protocole kitty quel que soit le nom ; XTVERSION fait demander DECRQM 2026 ; notifications ghostty → OSC 777, iTerm.app → OSC 9, kitty → OSC 99, Apple_Terminal → BEL, sinon rien ; progression 9;4 avec ghostty ≥ 1.2.0 seulement ; liens OSC 8 pour une liste de noms. Effet sur K1, K12, N1, N2, L1. | `--term-program`, `--term-program-version`, une variante à la fois ; journal de ce que Claude écrit (modes, OSC). |
| T3 | Réponses à DA1, DA2, XTVERSION, DECRQM : ce que Claude Code lit. | Journal du moteur. |

### Md : mod et barres (à partir de l'étape 1, membres lancés par recruit)

| ID | Vérification | Reproduire |
|---|---|---|
| Md1 | Le mod se charge. | `/recruit` répond. |
| Md2 | `PromptHint` et `SessionMode` masqués, la ligne QUIET reste. | Comparer à la même équipe sous tmux. |
| Md3 | `/recruit` et `/equipe` marchent. | — |

### Tm, S, D : teammates, réglages, doublons

| ID | Vérification | Reproduire |
|---|---|---|
| Tm1 | Sans `TMUX`, les teammates tournent dans le processus du membre ; le tableau de bord les montre. | Demander à un membre une équipe d'agents de deux teammates. |
| S1 | `<profil>/settings.json` inchangé après chaque session réelle. | `shasum -c` (mise en place). |
| S2 | Rien de l'environnement de la session qui lance le multiplexeur ne passe aux panneaux (`TMUX`, `TERM_PROGRAM`, `CLAUDECODE`, `CLAUDE_CODE_*` propres à une session : sinon « Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker »). | Lancer depuis un shell de Claude Code ; `env` dans un panneau. |
| D1 | Ligne ListAgents d'une session hors tmux, et formule de la ref (voir plus bas). | Deux `claude -n x` hors tmux ; ListAgents ; ref recalculée. |

### Sv : serveur et client (à partir de l'étape 1)

Équipe de test lancée par `RECRUIT_BACKEND=native` sur une copie du binaire, `XDG_*` et `RECRUIT_TMPDIR` temporaires, dans un dossier de test ; jamais une équipe de l'utilisateur. Les membres de test sont de vrais Claude, nommés, avec des demandes simples.

| ID | Vérification | Reproduire |
|---|---|---|
| Sv1 | Détacher (Alt+q, puis Détacher) puis `recruit attach` : même écran (onglets, panneaux, historique), les membres ont continué pendant l'absence, le terminal est rendu tel qu'il était au détachement. | Un membre en train de répondre ; détacher ; 30 s ; revenir. Comparer `_ctl capture` avant et après. |
| Sv2 | Terminal fermé (SIGHUP du client) puis revenir : le serveur et les membres continuent. | Fermer la fenêtre du terminal ; `recruit attach` depuis une autre. |
| Sv3 | Deuxième `attach` qui reprend la main (décision de l'utilisateur, étape 1) : le premier client est détaché avec un message clair, son terminal rendu propre ; le second voit l'écran à SA taille ; les membres ne voient qu'un redimensionnement. | Deux terminaux de tailles différentes ; `recruit attach` dans le second ; lire le premier. |
| Sv4 | Plantage du serveur (`kill -9`) : les membres s'arrêtent (pas de processus orphelin) ; `recruit` relance l'équipe, chaque membre reprend sa conversation (`sessions/<membre>`), le socket mort est nettoyé. | Pendant qu'un membre travaille ; `kill -9` du serveur ; `pgrep` des membres ; relancer ; vérifier la reprise. |
| Sv5 | Plantage du client (`kill -9`) : le serveur ne bouge pas ; le terminal reste en mode brut (à noter : `reset` nécessaire ?), `recruit attach` répare. | `kill -9` du client ; état du terminal ; revenir. |
| Sv6 | Client et serveur de versions différentes : refus avec un message clair, l'équipe reste joignable par l'ancien binaire ou s'arrête proprement. | Serveur lancé par un binaire, `attach` par un autre (version de protocole changée). |
| Sv7 | Socket : `${RECRUIT_TMPDIR:-/tmp}/recruit-<uid>/` en 0700, chemin sous les 104 octets de `sun_path`, pair d'un autre uid refusé. | `stat` du dossier ; chemin le plus long des tests ; connexion par un autre utilisateur si possible. |
| Sv8 | `recruit list`, `recruit stop`, `_mod`, `_edit` et `live.rs` passent par le socket : mêmes résultats que sous tmux. | Les commandes, équipe native et équipe tmux côte à côte. |
| Sv9 | Par SSH : `ssh macbook`, copie du binaire dans un dossier temporaire là-bas, équipe de test lancée dans la session SSH ; clavier, couleurs, OSC 52 jusqu'au terminal local ; SSH coupé (`kill` du ssh local) : l'équipe survit, `recruit attach` après reconnexion. SSH local inactif sur la machine de test. | Depuis Ghostty : `ssh macbook`, puis les mêmes gestes que Sv1, Sv2, P3. |
| Sv10 | Le serveur ne fait rien au repos : 0 octet vers le client, CPU nul, au plus un réveil par seconde ; et sans client attaché. | `cpu_rest_and_flood` par le socket ; puis détaché. |

### Journée de travail (critère de sortie de l'étape 1)

Une vraie équipe (à choisir avec l'architecte, par exemple `mux` elle-même ou une équipe de test sur un vrai projet) tourne une journée en natif.

**Ce qui est relevé, sans intervention :** toutes les 5 minutes, par un relevé automatique (test `#[ignore]` ou script du banc, à partir de `_ctl … clients` et des mesures par processus) : empreinte et CPU du serveur, nombre de clients, pour chaque panneau le membre, son état, son historique en lignes ; dans un fichier CSV daté, hors du dépôt.

**Ce qui est noté par l'utilisateur ou les membres, au fil de l'eau :** un événement par ligne, dans le journal de la journée (même dossier) : heure, ce qui s'est passé, gravité (bloquant, gênant, cosmétique), terminal, ce qui était en cours, et si possible comment le reproduire. À noter en priorité :

- saisie perdue, doublée ou envoyée au mauvais panneau ;
- écran faux : cellule fantôme, ligne décalée, curseur au mauvais endroit, scintillement, image figée ;
- membre arrêté, relancé ou repris sans raison ; conversation perdue ;
- lenteur perçue (frappe, défilement, changement d'onglet) ;
- détacher, revenir, SSH : tout ce qui ne revient pas comme avant ;
- presse-papiers, liens, notifications qui ne passent pas.

**En fin de journée :** le testeur dépouille le CSV (croissance de la mémoire par heure, CPU moyen et maximal, plantages) et le journal ; chaque événement devient une ligne de la grille (ok, ko, contournement) ou un bug confié par l'architecte. Sortie si aucun événement bloquant et aucune perte (saisie, conversation, historique).

## Campagnes

### Campagne 0 : Claude Code 2.1.295, hors multiplexeur (2026-10-09)

Faits établis avant le prototype, sur macOS 27.0.1, Apple M4.

**D1, doublons hors tmux (question 1 du §10).** Trois `claude -n dupe-q1` dans des PTY à eux (`TMUX`, `TMUX_PANE`, `TERM_PROGRAM` et les variables de session de Claude Code retirés), dossiers temporaires.

- Ligne ListAgents hors tmux : `dupe-q1 [f85787]  ·  interactive  ·  idle  ·  started 20s ago`. Sous tmux, la même ligne a en plus `·  tmux mux:@15.%41`. Ni dossier, ni terminal.
- Le champ `tmux` vient de `<profil>/sessions/<pid>.json`, rempli au démarrage seulement si `TMUX` et `TMUX_PANE` sont posés, par `tmux display-message -p -t $TMUX_PANE '#{session_name}:#{window_id}.#{pane_id}'`. `claude agents --json` ne le donne jamais (champs : pid, cwd, kind, startedAt, sessionId, name, status, waitingFor).
- La ref : les 6 premiers caractères hexadécimaux de `sha256("session:" + messagingSocketPath)`, `messagingSocketPath` étant dans `<profil>/sessions/<pid>.json` (`/tmp/cc-socks/<pid>.sock`). Sous le drapeau `tengu_session_stable_address` (absent aujourd'hui) : `sha256("session:sid:" + sessionId)` ; la résolution accepte alors les deux. Vérifié sur trois sessions.
- Un nom nu en double est refusé (« 2 agents are named 'dupe-q1'. Re-send with the ref of the one you mean »). `nom [ref]`, la ref étant calculée sans aucun listing, arrive à la bonne session.
- Revérifier à chaque version : la formule est interne à Claude Code.

**S2.** Une session lancée depuis l'environnement d'une session Claude Code hérite de `CLAUDE_CODE_CHILD_SESSION` et n'enregistre pas sa transcription. Les panneaux doivent en être protégés. Sous tmux (recruit actuel), une équipe lancée depuis un shell de Claude reçoit toutes ces variables quand le serveur tmux est démarré par ce lancement, mais la transcription reste écrite : avec `TMUX` posé, Claude Code lance `tmux show-environment -g`, et si la marque est dans l'environnement global du serveur, il la tient pour héritée de tmux (« ambiante ») et garde la transcription. Hors tmux, ce test répond « absent » : le nettoyage de l'environnement des panneaux est indispensable. Un serveur tmux déjà démarré ailleurs, dans un environnement propre, ne passe aucune de ces variables (ni `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` de l'utilisateur).

**K15, raccourcis (Claude Code 2.1.295, zsh 5.9, bash 5.3).** Table par défaut de Claude Code, liaisons avec Alt : `meta+o` mode rapide (Chat), `meta+p` choix du modèle, `meta+t` réflexion, `meta+w` mot-clé de workflow, `meta+up`/`meta+down` (liste des fichiers d'un diff, groupes d'agents). Pas de `~/.claude/keybindings.json` sur la machine de test. Envoyées à une saisie remplie : ⌥z, ⌥g, ⌥r, ⌥j, ⌥q, ⌥1, ⌥9 n'ont aucun effet visible et n'insèrent rien ; ⌥⇧← déplace le curseur d'un mot (comme ⌥←). Shell (mode emacs) : zsh lie ⌥1…9 (digit-argument), ⌥g (get-line), ⌥q (push-line), ⌥z (execute-last-named-cmd) ; bash lie ⌥1…9, ⌥g (glob-complete-word), ⌥r (revert-line) ; ni l'un ni l'autre ne lie ⌥o, ⌥j, ⌥⇧←/→. fish n'est pas installé : non vérifié. Conflit : **⌥o** (mode rapide de Claude Code). ⌥z et ⌥g ne prennent qu'au shell resté après Claude ; ⌥1…9, ⌥r, ⌥q le font déjà sous tmux.

**S1.** `~/.claude/settings.json` inchangé.

### Résultats par campagne

Frappes réelles, campagne 1 (2026-10-09, 16 h 05) : Terminal.app, kitty et WezTerm joués tels qu'installés, direct puis via le prototype `_mux`, factice sans modes, touches dangereuses comprises ; iTerm2 pas joué (`open -a` n'y lance pas le script) ; Ghostty pas joué (activation refusée par macOS 14, voir plus haut).

Terminaux de la machine de test (2026-10-09) : Ghostty 1.3.1, Terminal.app 2.15, iTerm2 3.7.4, kitty 0.49.2, WezTerm 20240203-110809. Les trois derniers sont installés pour ces tests, sans réglage. Pas de SSH à l'étape 0 (pas de serveur) : la colonne vient à l'étape 1. Versions notées à chaque campagne.

Réglages qui comptent, notés sans être changés chez l'utilisateur : Option comme Alt (Ghostty `macos-option-as-alt`, iTerm2 profil > Keys > « Left Option key: Esc+ », Terminal.app « Utiliser Option comme touche Méta », kitty `macos_option_as_alt`, WezTerm `send_composed_key_when_left_alt_is_pressed`). Les frappes sont jouées telles qu'installées et, quand le terminal accepte un réglage en ligne de commande, avec Option comme Alt.

Frappes réelles : `tests/mux_keys.rs` les envoie par System Events dans chaque terminal, directement puis à travers le multiplexeur, et compare les octets reçus par un factice. Personne ne doit taper pendant un passage. Garde-fous depuis l'incident du 2026-10-09 (des touches parties dans un zsh d'une fenêtre restaurée) : pas de fenêtres restaurées, fenêtre clé vérifiée par son titre dans le même script que la frappe, touche témoin, touches dangereuses seulement avec `KEYS_DANGEROUS=1`, terminal fermé par son pid vérifié. Limite de macOS 14 et suivants (activation « coopérative ») : une seconde instance du terminal où travaille l'utilisateur (Ghostty) ne vient jamais au premier plan, et le passage s'arrête sans rien taper ; décision de l'utilisateur (2026-10-09) : pas de clic de sa part, les frappes réelles passent par les terminaux qu'il n'a pas ouverts (Terminal.app d'abord), Ghostty se voit à l'usage.

| ID | Test, sans fenêtre | Ghostty | iTerm2 | Terminal.app | kitty | WezTerm | dans tmux | par SSH | Note |
|---|---|---|---|---|---|---|---|---|---|
| R1 | ok | — | — | — | — | — | — | — | campagne 1 : démarrage, écran propre, 7 variantes ; campagne 2, natif : 7 variantes, écran propre |
| R2 | ok | — | — | — | — | — | — | — | campagne 1 : démarrage en plein écran (`CLAUDE_CODE_NO_FLICKER=1`), écran propre ; campagne 2, natif : plein écran (1049, souris 1003 SGR) |
| R3 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::a_pane_as_written` : région 7 à 9 posée, cinq lignes défilées dedans, rien au-dessus ni au-dessous ne bouge |
| R4 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::history_in_order` : 2000 lignes, `capture --history` rend 1 à 2000 dans l'ordre, sans trou ; la molette dans un vrai terminal : journée |
| R5 | ok | — | — | — | — | — | — | — | campagne 1 : Claude demande DECRQM 2026 et pose des BSU dans le panneau (réponse XTVERSION du moteur) ; campagne 1, *banc* `no_partial_frame` : 23 images synchronisées, 0 partielle |
| R6 | ok | — | — | — | — | — | — | — | unitaire `engine::ghostty::synchronized_updates_hold_the_frame` : un BSU sans ESU est montré à son échéance, pas avant |
| R7 | cont. | — | — | — | — | — | — | — | unitaire (même test) : pendant un BSU, les requêtes sont répondues tout de suite, pas après 150 ms comme avec vte ; le temps de lancement de Claude jusqu'à l'invite, comparé à la référence : à mesurer (vraies sessions) |
| R8 | ok | — | — | — | — | — | — | — | campagne 1 : 200×50 → 150×40 → 200×50, écran propre ; campagne 2, natif : 200×50 → 150×40 → 200×50 propre ; panneau rétréci pendant le démarrage de Claude : propre (sans `script`, qui ne transmet pas la taille) |
| R9 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::a_pane_as_written` (terminal qui se dit Ghostty) : aucun texte envoyé hors d'une mise à jour synchronisée, aucune laissée ouverte ; `mux::no_partial_frame` : aucune image partielle ; l'observation à l'œil : journée |
| R10 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen` : position et forme (DECSCUSR 4, soulignement fixe) du panneau ; avec deux panneaux, le curseur suit le panneau actif (clic, puis ⌥n) |
| K1 | ok | — | — | ok | ok | ok | — | — | campagne 1 : `CSI 13;2u` (kitty poussé) ; campagne 1, frappes : Terminal.app `ESC CR` des deux côtés ; kitty et WezTerm envoient `CR` en direct, le mux traduit en `LF` (va à la ligne dans Claude Code, §5.4) ; campagne 2, natif : par le vrai client, avec et sans kitty |
| K2 | — | — | — | cont. | cont. | ok | — | — | campagne 1, Terminal.app tel qu'installé : ⌥b donne « ₿ », ⌥f « ẟ », en direct comme via le mux (Option n'est pas Méta par défaut ; réglage du terminal) ; campagne 1, frappes : Terminal.app et kitty tels qu'installés envoient le caractère de la disposition (⌥b « ₿ ») ; WezTerm envoie `ESC b` ; identique via le mux |
| K3 | — | — | — | ok | ok | ok | — | — | campagne 1, frappes : `^B` identique (sans préfixe) |
| K4 | — | — | — | ok | ok | ok | — | — | campagne 1, Terminal.app : `^R` identique en direct et via le mux |
| K5 | — | — | — | ok | ok | ok | — | — | campagne 1, frappes : `^O` identique |
| K6 | ok | — | — | ok | ok | ok | — | — | campagne 1 : vide la saisie ; campagne 1, frappes : `^C` identique |
| K7 | — | — | — | ok | ok | ok | — | — | campagne 1, frappes : `^D` identique |
| K8 | — | — | — | ok | ok | ok | — | — | campagne 1, frappes : `ESC` identique (délai : voir le banc) |
| K9 | — | — | — | ok | ok | ok | — | — | campagne 1, Terminal.app : `^I` identique |
| K10 | ok | — | — | ok | ok | ok | — | — | campagne 1 : le mode change, puis revient ; campagne 1, frappes : `ESC[Z` identique |
| K11 | — | — | — | ok | ok | ok | — | — | campagne 1, Terminal.app : `ESC[A`…`ESC[D` identiques |
| K12 | ok | — | — | — | — | — | — | — | campagne 1 : drapeaux 5 poussés par Claude, kitty poussé au terminal extérieur |
| K13 | ok | — | — | ok | ok | cont. | — | — | campagne 1 : « é à ç « » » ; campagne 1, Terminal.app : é è à ç identiques (« » non tapables par System Events sur cette disposition) ; campagne 1, frappes : é è à ç identiques ; WezTerm (Option = Alt) envoie `ESC w` pour é sur la disposition de l'utilisateur |
| K14 | ok | — | — | — | — | — | — | — | unitaires `mux::keys::claude_code_keeps_its_keys` et `the_table` ; dans un vrai terminal, avec Claude en attente : journée |
| K15 | ok | ko | | | — | — | — ; campagne 1 : ⌥1 et ⌥⇧→ passés par le prototype `_mux`, qui ne les connaît pas : à rejouer par le vrai client ; étape 1, `client_keys_reach_pane` (avec et sans kitty) : ⌥1 et ⌥⇧→ pris par le serveur, rien au panneau |
| C1 | ok | — | — | — | — | — | — | — | unitaire `input::encode::pastes` : encadré si 2004, brut sinon, rien dans le texte ne ferme le cadre, 200 Ko ; le « [Pasted text …] » de Claude : journée |
| C2 | — | — | — | ok | ok | ok | — | — | campagne 1, frappes : 150 000 octets entiers via le mux ; kitty en direct demande « Allow paste? » (pas de collage encadré), pas via le mux |
| C3 | — | — | — | ok | ok | ok | — | — | campagne 1, frappes : CR et tabulation gardés |
| C4 | — | — | — | — | — | — | — | — | attend l'usage réel : image collée par Ctrl+V, lue par Claude lui-même ; et par SSH |
| M1 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::focus_and_clicks` : clic dans le panneau de droite (1000, SGR), reçu à ses propres coordonnées (`CSI < 0;10;4 M` puis `m`) ; unitaire `encode::mouse_reports` pour 1000, 1002, 1003 et X10 |
| M2 | ok | — | — | — | — | — | — | — | étape 1, `claude_mouse_native` : molette à Claude en plein écran, à l'historique en mode classique ; campagne 2, natif : rejoué, ok |
| M3 | ok | — | — | — | — | — | — | — | étape 1, `claude_mouse_native` : Maj+glisser sur « FINI-TEST » en plein écran ; campagne 2, natif : rejoué, « FINI-TEST » copié |
| M4 | — | — | — | — | — | — | — | — | attend l'usage réel (voir L1, L2) |
| M5 | ok | — | — | — | — | — | — | — | campagne 1 : Alt+→ change le panneau (clavier) ; campagne 2, `mux_screen::focus_and_clicks` : le clic donne le focus au panneau (cadre épais) et lui parvient une fois, ni perdu ni doublé |
| L1 | ok | — | — | — | — | — | — | — | campagne 1, *banc* : OSC 8 relayé quand le terminal dit son nom (XTVERSION), retiré sinon, le texte gardé |
| L2 | — | — | — | — | — | — | — | — | attend l'usage réel : Cmd+clic dans un vrai terminal, un seul onglet ouvert |
| L3 | ok | — | — | — | — | — | — | — | campagne 1, *banc* : terminal sans XTVERSION, lien retiré, texte gardé |
| P1 | ok | — | — | — | — | — | — | — | campagne 1, *banc* : OSC 52 relayé tel quel |
| P2 | ok | — | — | — | — | — | — | — | étape 1, `claude_mouse_native` : la sélection copiée arrive en OSC 52, décodée « FINI-TEST » |
| P3 | — | n/a | n/a | n/a | n/a | n/a | — | ok | étape 1, `ssh_attach` : l'OSC 52 d'un panneau sur macbook arrive au terminal local |
| N1 | cont. | — | — | — | — | — | — | — | campagne 1, *banc* : OSC 9, 777 et 99 relayés tous en OSC 9 (« titre: corps »), BEL relayé |
| N2 | ok | — | — | — | — | — | — | — | campagne 1, *banc* : OSC 9;4 relayé |
| N3 | ok | — | — | — | — | — | — | — | campagne 1, *banc* : titre OSC 2 relayé (panneau actif) |
| N4 | cont. | — | — | — | — | — | — | — | unitaires `chrome::an_alert_before_the_rest`, `server::the_most_urgent_state_shows` ; le badge vu sur le membre et l'onglet : journée |
| F1 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::focus_and_clicks` : `CSI O` au panneau quitté qui a demandé 1004, rien à celui qui ne l'a pas demandé, `CSI I` au retour par ⌥n |
| F2 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::focus_and_clicks` : le client demande 1004 à son terminal ; `CSI O` puis `CSI I` du terminal relayés au panneau actif ; passer à une autre application : journée |
| Co1 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::a_pane_as_written` : SGR 38;2 et 48;2 rendus aux mêmes couleurs ; le dégradé de l'accueil de Claude : journée |
| Co2 | — | — | — | — | — | — | — | — | mécanisme couvert en unitaire (`ghostty::colors_are_the_real_terminals` : 996n répond 997;1 ou 2 selon le fond) ; le thème `auto` de Claude, terminal en clair puis en sombre : usage réel |
| Co3 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::a_pane_as_written` : OSC 10 et 11 du panneau répondus avec les couleurs du terminal de test (c0c1c2 et 1e1e1e, distinctes des défauts du moteur) |
| Co4 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::colors_without_24_bits` : `COLORTERM` vide, terminal inconnu : rouge 24 bits envoyé en couleur 196, aucun 38;2 |
| U1 | ok | — | — | — | — | — | — | — | campagne 1 : « 漢字 » saisi et affiché |
| U2 | ok | — | — | — | — | — | — | — | campagne 1 : « 🙂 » saisi |
| U3 | — | ko | — | ok | ok | ok | — | — | campagne 1, sonde du factice `keylog` (👍🏽 puis CPR) : Ghostty 1.3.1 répond colonne 3 (2 cases, mode 2027 actif par défaut), le moteur du mux colonne 5 (4 cases) : la ligne se décale ; dev-saisie coupe 2027 à l'ouverture ; campagne 1, Terminal.app : 👍🏽 colonne 5 en direct (4 cases, pas de 2027), colonne 3 via le mux (le moteur en compte 2) : la ligne se décale ; campagne 1 : kitty et WezTerm colonne 3, comme le moteur ; Terminal.app (sans 2027) colonne 5 en direct, mais le rendu lui envoie 👍 sans 🏽 : aligné par conception (à vérifier au banc) |
| U4 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::a_pane_as_written` : « e » + U+0301 dans une seule case, la suivante à sa place |
| U5 | — | — | — | — | — | — | — | — | attend l'usage réel : l'indicateur animé pendant un vrai tour |
| U6 | ok | — | — | — | — | — | — | — | unitaires `input::grapheme_clusters_turned_off_only_where_on`, `ghostty::queries_are_answered` ; campagne 2, `mux_screen` : DECRQM `?2027` du panneau répondu (`2027;1`) |
| U7 | ok | — | — | — | — | — | — | — | campagne 2, `mux_screen::a_pane_as_written` : « ┌─┐│└┘█▀▄░ » rendu à l'identique ; les cadres de Claude (`/model`) : journée |
| Pf1 | ok | — | — | — | — | — | — | — | campagne 2, serveur natif + client : 0,00 %, 0 réveil, 0 octet, avec 2 vrais Claude, 10 factices ou 9 historiques pleins (voir « Banc ») |
| Pf2 | ok | — | — | — | — | — | — | — | campagne 2 : 10 panneaux en flux à 58,7 images/s (seuil 50) ; au débit de Claude, 4,3 % pour un panneau contre 1,4 % pour tmux |
| Pf3 | ok | — | — | — | — | — | — | — | campagne 2 : ajouté +0,42 / +1,20 ms (p50 / p99) au repos, p99 +2,84 ms avec 9 panneaux en flux |
| Pf4 | ok | — | — | — | — | — | — | — | campagne 2 (80 / 120 / 200 colonnes) : lignes synthétiques 4,7 / 6,5 / 7,2 Mio par panneau ; rejeu d'une vraie sortie de Claude 9,0 / 17,1 / 16,6 Mio, contre 97 / 143 / 172 pour tmux |
| T1 | cont. | — | — | — | — | — | — | — | campagne 2, `mux_screen::a_pane_as_written` : le panneau natif reçoit `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=recruit` ; ce que Claude fait de chaque `TERM` : `claude_modes` (vraies sessions) |
| T2 | ok | — | — | — | — | — | — | — | campagne 1 : voir « Terminal annoncé » plus bas ; campagne 2, natif : les 5 variantes par `spawn --env`, mêmes modes demandés |
| T3 | ok | — | — | — | — | — | — | — | unitaire `ghostty::queries_are_answered` (DA1, CPR, DECRQM, `CSI ? u`, XTVERSION, OSC 7501, `CSI 18 t`) ; campagne 2, `mux_screen` : DA2 répondu `CSI > 1;0;0 c` ; ce que Claude en lit : `claude_modes` |
| Md1 | ok | — | — | — | — | — | — | — | étape 1 ; campagne 2 : le mod se charge, `/recruit` ouvre le menu natif |
| Md2 | ok | — | — | — | — | — | — | — | étape 1, équipe native réelle : l'aide masquée chez les agents, gardée chez l'interlocuteur principal avec la ligne d'état de l'utilisateur |
| Md3 | ok | — | — | — | — | — | — | — | étape 1 ; campagne 2 : `/recruit` ouvre le calque ; `/equipe` bascule le journal (voulu) ; menu déjà ouvert : le message parle encore de tmux (mineur) |
| Tm1 | ok | — | — | — | — | — | — | — | campagne 2 : deux teammates lancés par un membre, dans son processus (sans `TMUX`), montrés au tableau de bord |
| S1 | ok | — | — | — | — | — | — | — | campagne 1 ; campagne 2 : inchangé à chaque passage |
| S2 | ok | — | — | — | — | — | — | — | étape 1, équipe native réelle : membres sans `CLAUDECODE` ni `TMUX`, `TERM_PROGRAM=recruit` |
| D1 | — | ok | | | | | | | campagne 0, hors multiplexeur |
| Sv1 | ok | — | — | — | — | — | — | — | étape 1, `client_gone_then_back` et équipe native réelle : le client parti, le panneau continue ; revenu, l'écran suit |
| Sv2 | ok | — | — | — | — | — | — | — | étape 1, `client_gone_then_back` : client tué par SIGHUP (terminal fermé), serveur intact |
| Sv3 | ok | — | — | — | — | — | — | — | étape 1, `second_attach_takes_over` : le premier client sort, terminal rendu propre, message « Détaché : l'équipe … a été ouverte dans un autre terminal. » ; le second à sa taille |
| Sv4 | ok | — | — | — | — | — | — | — | étape 1, `server_killed` : serveur tué par SIGKILL, aucun programme orphelin (hors l'annexe MCP `ha_mcp`, qui survit aussi sous tmux), `_ctl` code 2 ; reprise des conversations pas encore vérifiée ; campagne 2 : kill -9 du serveur, aucun orphelin (sauf l'annexe MCP `ha_mcp`, comme sous tmux) ; `recruit --detach` reprend les conversations en le disant, même après `recruit list` ; second plantage dans les 10 min : refus sans terminal, question (O/n) depuis un terminal, `--resume` reprend ; serveur figé, `stop --yes` (SIGKILL après 10 s) puis relance : à neuf, sans parler de plantage |
| Sv5 | ok | — | — | — | — | — | — | — | étape 1, `client_killed` |
| Sv6 | ok | — | — | — | — | — | — | — | étape 1 ; campagne 2 (copie à `PROTO=2`) : `_ctl` refuse avec un message clair, `list` voit l'équipe, `stop` passe ; `_ctl attach` échouait sur « socket: Invalid argument (os error 22) » ; rejoué à 19 h 55 après le correctif de `client.rs`, dans les deux sens (serveur 1 et client 2, serveur 2 et client 1) : `attach` et `panes` donnent « l'équipe … tourne avec recruit X, dont le protocole diffère… recruit stop …, puis … --resume », code 1, rien laissé ; les deux versions s'affichent pareilles (1.1.2) dans ce montage, un protocole changé sans changer de version |
| Sv7 | ok | — | — | — | — | — | — | — | étape 1, `socket_is_private_and_short` : dossier en 0700, chemin sous 104 octets ; un `RECRUIT_TMPDIR` long est refusé avec un message |
| Sv8 | cont. | — | — | — | — | — | — | — | étape 1 : `list`, `stop`, `_edit` (ajout, retrait, renommage avec relance, déplacement d'onglet sans relance) vérifiés sur une équipe native réelle ; `_mod` et le menu pas encore ; campagne 2 : `list`, `stop`, `_edit` (ajout, retrait, renommage, déplacement, modèle à chaud) ok ; `_mod` ok (modèle Sonnet servi au tour suivant, note d'équipe) ; ⌥q ouvre le menu principal au lieu de Détacher/Quitter/Annuler, voulu à l'étape 1 (le calque Détacher, Quitter, Annuler arrive à l'étape 2) |
| Sv9 | — | — | — | — | — | — | — | ok | étape 1, `ssh_attach` vers macbook (macOS 26.7, arm64), équipe `--dry-run` lancée par SSH avec `--detach` : survit à la fin de la session, `attach` par `ssh -tt` (écran complet), SSH coupé net, le serveur reste sans client, second attach ; `stop` sans reste ; `settings.json` distant inchangé |
| Sv10 | cont. | — | — | — | — | — | — | — | étape 1 ; campagne 2, équipe réelle : sans client 0 % ; avec un client 0,3 % sur 30 s et ~2 petites images/s, dues à l'horloge du tableau de bord et à la ligne d'état de l'utilisateur (voulues) |

### Campagne 1 : prototype de l'étape 0, Claude Code 2.1.295 (2026-10-09)

Colonne « Test, sans fenêtre » : `tests/mux_claude.rs`, le terminal de test (`tests/common/pty.rs`, un `Term` d'alacritty qui répond aux requêtes et a le protocole kitty), 200×50, `recruit _mux --panes 2`, deux vrais Claude Code nommés (`-n`), prototype de 13 h. Les terminaux réels viennent avec `tests/mux_keys.rs`.

**Terminal annoncé (T1, T2).** Variantes jouées : recruit (référence, `TERM_PROGRAM=recruit` et la version de recruit), aucun, ghostty, ghostty 1.2.3, tmux avec `TERM=tmux-256color`. Les 11 contrôles passent dans toutes. Ce que Claude demande dans le panneau (relevé par `script -F`) ne dépend pas du nom : kitty poussé avec les drapeaux 5, XTVERSION puis DECRQM 2026 demandés, BSU posés, 1049 seulement en plein écran. Seul, dans un terminal qui ne répond pas à XTVERSION, Claude ne pose de BSU qu'avec `TERM_PROGRAM=ghostty` : dans le multiplexeur, la synchro vient de la réponse XTVERSION du moteur. Claude ne demande jamais OSC 11 au démarrage (2031 seulement).

**Pièges des sessions de test.**
- Le rendu de Claude se choisit par session sans `/tui` (qui l'enregistre pour les sessions suivantes) : `CLAUDE_CODE_NO_FLICKER=1` pour le plein écran, `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1` pour le mode classique. Sur la machine de test, `~/.claude/settings.json` de l'utilisateur a `"tui": "fullscreen"`.
- Un `claude` sans `-n` s'ouvre sur la vue des agents (« describe a task for a new session ») : les sessions de test sont nommées, comme les membres de recruit.

### Campagne 2 : serveur natif et vrai client, Claude Code 2.1.295 (2026-10-09, soir)

Fin de l'étape 1, avant la journée de travail. Prototype retiré. Vraies sessions autorisées par l'utilisateur (`CLAUDE_SESSIONS`), pas de frappes réelles. Équipes de test en Haiku dans des dossiers temporaires, copie du binaire (copiée sous un autre nom puis renommée), `RECRUIT_TMPDIR` court ; `~/.claude/settings.json` inchangé à chaque passage.

- **Sv4** : voir le tableau (reprise après plantage, `recover` de l'architecte).
- **Sv6** : un binaire construit à part avec `PROTO=2` contre un serveur à `PROTO=1` ; `attach` ne donnait pas le message prévu (EINVAL sur le socket fermé), corrigé dans `client.rs` par dev-serveur, rejoué ok à 19 h 55 dans les deux sens.
- **Sv8, Md1, Md3** : menu natif par ⌥r et `/recruit`, en calque ; ⌥q ouvre le menu principal, voulu à l'étape 1 (le calque Détacher, Quitter, Annuler vient à l'étape 2) ; modèle changé à chaud et servi au tour suivant (`claude-sonnet-5-5` dans la transcription), note d'équipe reçue au message suivant.
- **Tm1** : teammates dans le processus du membre, au tableau de bord.
- **Sv10** : rien du multiplexeur au repos ; seules l'horloge du tableau de bord et la ligne d'état de l'utilisateur changent.
- **Vraies sessions par le vrai client** : `claude_through_mux` 77 sur 77, `claude_mouse_native` 3 sur 3, `claude_modes` inchangé.
- **Piège du banc** : `script` (macOS) ne transmet pas les changements de taille à son PTY : un Claude lancé sous `script` dans un panneau qui rétrécit dessine à sa largeur de départ (lettres semées au bord, cellules fantômes). Les tests ne passent plus par `script` par défaut.
- **Effet de bord** : la mémoire automatique de Claude a gardé des mots de test dans la mémoire du projet de test (`~/.claude/projects/<dossier de test>/memory/`).

- **Frappes réelles, iTerm2** (go de l'utilisateur, 19 h 05) : aucune touche jouée, le factice n'a pas démarré. Cause : au premier lancement, iTerm2 3.7.4 ouvre la question de Sparkle « Check for updates automatically? » et n'ouvre aucune fenêtre tant qu'on n'y répond pas. Corrigé dans `tests/mux_keys.rs` (`-SUEnableAutomaticChecks NO` au lancement, pour ce lancement seulement), avec les `iTermServer` qui survivaient à iTerm2 désormais arrêtés eux aussi. iTerm2 avait recopié le profil de test dans `New Bookmarks` : retiré par `defaults`, profil Default identique. Passage à rejouer au go suivant.
- **Lignes vides de la grille** (20 h 40) : `tests/mux_screen.rs`, 4 tests actifs (1,4 s), 10 sur 10 sous macOS et sous Alpine ; R3, R4, R9, R10, M1, M5, F1, F2, Co1, Co3, Co4, U4, U6, U7, T3 ok, T1 en partie ; R6, K14, C1 couverts par les tests unitaires ; C4, M4, L2, Co2, U5 attendent l'usage réel. Défaut du terminal de test trouvé en passant (pas du multiplexeur) : il répondait à OSC 11 la couleur du texte (l'indice d'alacritty pour OSC 11 est 257, pas 11) ; corrigé dans `tests/common/pty.rs`. Avant, un client natif dans ce terminal croyait à un fond clair (d8d8d8) : les passages précédents avec un panneau qui demande le thème (`CSI ? 996 n`) l'ont vu « clair ».
- **ECH du Painter** (dev-rendu ; captures du 2026-10-09 à 23 h 45, go de l'utilisateur, `mux_keys::window_shots`, sans frappe) : le motif `target/day/shots/ech-pattern.txt` (blancs de 10 cases et plus sur le fond par défaut effacés par `CSI n X`, blancs sur fond coloré écrits) est juste dans Ghostty 1.3.1, kitty 0.49.2, WezTerm 20240203, Terminal.app 2.15 et iTerm2 3.7.4 : six trous au fond normal de 10 à 30 cases, `|` jaune juste après chacun, 7e rangée au trou gris, aucun X resté. Captures dans `target/day/shots/`. Rangement : aucun processus de test, iTerm2 rendu (profil par défaut, DynamicProfiles vide, copie retirée de `New Bookmarks`). Aussi vérifié sans fenêtre par dev-rendu (alacritty, libghostty-vt, tmux).

Performance : voir « Campagne 2 » sous « Banc de performance ».

## Parité (§7.1 de la spec) : vérifications de l'étape 2

À cocher en natif (`RECRUIT_BACKEND=native`), en français et en anglais, sous macOS et, pour ce qui n'a pas besoin d'une fenêtre, sous Linux (Alpine). Chaque ligne dit sa preuve :
- **T** : un test sans fenêtre, dans `cargo test`, existant ou à écrire. Les tests d'équipe (`tests/mux_team.rs`, à écrire) lancent une vraie équipe native par `recruit` avec un faux `claude` dans le `PATH` (factice des tests : `--version`, `agents --json`, une invite, une sortie sur commande, `-r <session>` noté), `XDG_*` et `RECRUIT_TMPDIR` temporaires, un client sur le terminal de test.
- **C** : une capture de `scripts/screenshots.sh` adapté au natif, comparée à celle de tmux.
- **M** : un essai à la main, sur une équipe de test (faux `claude`, ou vrais Claude courts), noté avec sa date.

| # | Vérification | Preuve | Fait |
|---|---|---|---|
| D1 | Premier onglet « Interlocuteurs », puis un onglet par `tab`, sinon « Agents » ; groupes de 6 au plus répartis à parts égales (« Agents (1) », « Agents (2) »), même pour une petite équipe | T : tests de `layout.rs` (le plan) ; `mux_team` : équipes de 3, 7 et 13 agents, les onglets et leurs membres (`_ctl panes --json`) égaux au plan, la barre lue à l'écran ; C : la barre | ok (2026-10-09, `mux_team::tabs_as_planned` et `small_team_without_dashboard`, macOS et Alpine ; capture à faire) |
| D2 | `layout = "tabs"`, `columns`, `rows` suivis | T : `mux_team`, un fichier par réglage, onglets et grilles comparés au plan (positions des cadres à l'écran) | |
| D3 | Tableau de bord à droite des interlocuteurs, journal dessous ; deux interlocuteurs au plus, empilés à gauche | T : `mux_team`, positions des cadres « Tableau de bord » et « Journal » par rapport aux interlocuteurs ; C : capture de l'équipe | |
| D4 | ⌥j : complet, réduit (trois derniers messages), masqué, complet ; réduit au lancement | T : `mux_team`, ⌥j envoyé par le client, hauteur du journal (`_ctl panes`) à chaque pas ; C | |
| D5 | `dashboard = false` : ni tableau de bord ni journal, interlocuteurs côte à côte | T : `mux_team` | ok (`mux_team::small_team_without_dashboard`) |
| K1 | Clic sur une carte du tableau de bord : on va au panneau du membre (son onglet, son cadre épais) | T : `mux_team`, zone de la carte lue dans `zones.json`, clic SGR du client, focus vérifié ; M : la journée (événement de 21 h 18) | |
| K2 | Clic sur l'expéditeur ou le destinataire d'un message du journal : même chose | M : journée (il faut de vrais messages) ; T si le faux `claude` écrit une transcription | |
| K3 | Clic sur le contexte d'un membre au repos : confirmation en calque (compacter, annuler ; ouverte sur annuler, le reste atténué) ; annuler ne fait rien, compacter le demande au membre | T : `mux_team`, le calque à l'écran, Échap et « annuler » sans effet ; M : compaction réelle d'un vrai Claude | |
| K4 | Le bouton « menu » et le bouton « quitter » de la barre répondent au clic | T : `mux_team`, clic sur chaque bouton, calque ouvert | |
| R1 | ⌥r et le bouton ouvrent le menu en calque, le reste atténué | T : `mux_team` (le menu à l'écran : liste des membres) ; C : le menu | |
| R2 | `/recruit` ouvre le même calque ; menu déjà ouvert : message propre au natif (`AlreadyOpen`) ; sans client : message propre au natif | M : vrai Claude (comme Md3) ; T : `recruit _mod command` appelé directement si la forme JSON le permet | |
| Q1 | ⌥q et le bouton « quitter » : Détacher (d), Quitter (q), Annuler (a) ; en anglais Detach (d), Quit (q), Cancel (c) ; ouvert sur Annuler ; ←→, ⏎, Échap et clic sur chaque option | T : `mux_team` dans les deux langues : `d` fait sortir ce client et le serveur reste, `q` arrête l'équipe sans reste (autocontrôle), `a`, `c` et Échap ferment ; ⏎ à l'ouverture = Annuler ; C : le calque | |
| A1 | ⌥1…⌥9 changent d'onglet, ⌥⇧←/→ au précédent et au suivant ; l'onglet courant en vidéo inversée | T : unitaires `keys::the_table` ; `mux_team`, touches envoyées par le client (kitty et sans), onglet vu dans la barre | |
| A2 | ⌥ affiché sous macOS, Alt+ ailleurs (barre, aide) | T : `mux_team` sous macOS et sous Alpine, texte de la barre ; C | |
| S1 | Un membre arrêté (`/exit`, plantage) est relancé après 2 s sur sa session | T : `mux_team`, le faux `claude` sort de lui-même ; relancé après 2 s au moins, avec `-r <session relevée>` | ok (`mux_team::a_stopped_member_comes_back`, ignoré dans cargo test : 5 s ; 10/10 macOS, ok Alpine) |
| S2 | Ctrl-C : un shell à la place ; pas de relance après deux départs ratés, ni à l'arrêt de l'équipe | T : `mux_team`, faux `claude` tué par SIGINT puis `SHELL` factice qui s'annonce ; deux sorties ratées de suite, plus de relance ; `_ctl stop`, aucune relance | ok (`ctrl_c_leaves_a_shell`, `a_stopped_member_comes_back`), macOS et Alpine ; sous busybox ou dash, Ctrl-C dans la pause fermait le panneau : corrigé (`trap : INT` dans `launch::member_script`, architecte), rejoué 10/10 sous Alpine |
| S3 | Revenir sur une équipe qui tourne relance les membres arrêtés, rouvre le tableau de bord, propose de reconstruire si des panneaux manquent | T : `mux_team`, panneau du tableau de bord fermé (`_ctl kill`) et un membre arrêté, puis `recruit <équipe>` dans le terminal de test : tableau de bord revenu, membre relancé ; panneau manquant : la question posée | |
| H1 | Modifications du menu à chaud (`live.rs`) : ajout, retrait, renommage (relance sur sa conversation), déplacement entre onglets sans arrêter le membre | T : `mux_team` avec `recruit _edit` : pid du membre déplacé inchangé, onglets à jour, renommé relancé avec `-r` ; M : par le menu lui-même (Sv8, campagne 2) | ok (`mux_team::hot_edits` : déplacé sans relance, ajouté, retiré, renommé repris avec `-r`) |
| L1 | Fichiers d'équipe illisibles : menu en lecture seule (membres tels que lancés, détacher, arrêter) | T : `mux_team`, `settings.toml` cassé après le lancement, ⌥r : le menu le dit et ne propose que détacher et arrêter ; C | |
| L2 | `--dry-run` en natif : le plan, des panneaux sans Claude, sans shell de connexion à la fin | T : `mux_team` (aucun processus `claude` lancé, texte des panneaux lu) | |
| L3 | `recruit list`, `attach`, `stop` en natif | T : `mux_team` (déjà vu à la main, Sv8) ; `stop` sans reste | `list` et `stop` ok (`mux_team`, chaque test s'arrête par `recruit stop` sans reste) ; `attach` à venir avec les tests du client |
| C1 | `scripts/screenshots.sh` produit les mêmes captures en natif (mêmes fichiers, même cadrage ; sans forfait, `/Users/` ni `$HOME`) | C : les deux jeux côte à côte ; M : relecture des captures | |

Ordre prévu : `mux_team` et son faux `claude` dès maintenant (D, S, H, L), puis Q, K et R quand le choix et les clics seront là (dev-serveur, dev-interface, dev-rendu), enfin C1.

## Banc de performance

`tests/mux_bench.rs`, en release, un test à la fois, chaque mesure dans un processus à part (le binaire de test relancé) :

```sh
CARGO_TARGET_DIR=<à soi> CARGO_PROFILE_RELEASE_LTO=off cargo test --release --test mux_bench -- --ignored --nocapture --test-threads=1
```

Chaque mesure est comparée à tmux (`-L rtest-testeur`, `-f /dev/null`, `history-limit 100000`) dans la même séance. La charge de la machine est notée : seuls les rapports au tmux de la même séance se comparent d'une campagne à l'autre.

Seuils (architecte, 2026-10-09) : au repos, 0 octet vers le terminal, CPU indiscernable de 0, au plus 1 réveil par seconde ; latence ajoutée (mux moins direct) p50 ≤ 2 ms et p99 ≤ 16 ms au repos, p99 ≤ 33 ms avec 9 panneaux en flux ; 10 panneaux en flux à 50 images/s au moins ; 0 image partielle ; mémoire comparée à tmux, sans seuil.

### Mémoire par panneau, 100 000 lignes (`memory_per_pane`)

Campagne 0, 2026-10-09, macOS 27.0.1, Apple M4, 24 Gio, charge ~15. Le moteur seul (`Term` d'alacritty_terminal 0.26 et `Processor` de vte 0.15, sans le prototype), 50 rangées. Coût par panneau : la pente entre 1 et 10 panneaux. Empreinte physique (`phys_footprint`, ce que montre le Moniteur d'activité, mémoire compressée comprise), en Mio.

| Colonnes | alacritty, lignes typiques | tmux 3.8, typiques | alacritty, lignes pleines | tmux 3.8, pleines |
|---|---|---|---|---|
| 80 | 200 | 47 | 200 | 73 |
| 120 | 318 | 66 | 318 | 109 |
| 200 | 507 | 97 | 507 | 182 |

Rejeu d'une vraie sortie de Claude (`BENCH_REPLAY`, fichier de dev-terminal fait des transcriptions du projet, 6,18 Mo, gardé en local : contenu réel de l'utilisateur), joué deux fois pour remplir l'historique. libghostty-vt mesuré avec le binaire `ghostty-eval` de dev-terminal (même méthode : 50 rangées, tranches de 64 Kio, pente de 1 à 10 panneaux), recontrôlé par le testeur :

| Colonnes | alacritty 0.26 | tmux 3.8 | libghostty-vt | libghostty-vt, historique compressé |
|---|---|---|---|---|
| 80 | 200 | 35 | 67 | 10 |
| 120 | 318 | 42 | 99 | 13 |
| 200 | 507 | 66 | 165 | 14 |

Compression complète de 100 000 lignes : 123 à 278 ms chez le testeur (machine chargée), 44 à 51 ms chez dev-terminal.

Lignes typiques : longueurs réparties de 0 à la largeur, quelques mots en couleur ; pleines : toute la largeur. alacritty garde chaque rangée entière (24 octets par case) : son coût ne dépend que de la largeur. Résident bien plus bas (28 à 102 Mio par panneau) : macOS compresse les cases vides. Mémoire virtuelle : +5 à 6 Mio par panneau, dont les 2 Mio réservés par le `Processor` de vte.

### Mémoire à travers le multiplexeur (`memory_through_mux`)

Depuis l'étape 1 : le serveur natif d'un panneau rempli de 100 000 lignes, moins le même avec un panneau vide, un client attaché. À l'étape 0 : le panneau de `recruit _mux --panes 1` (prototype, retiré depuis), `BENCH_ENGINE` choisissant le moteur. Campagne 1, moteur alacritty : 80 colonnes 200,5 Mio, 120 colonnes 318,0 Mio, 200 colonnes 506,9 Mio par panneau, soit le coût du moteur seul (le reste du prototype pèse 3,5 à 4,3 Mio). Avec libghostty-vt : voir « Étape 1 » et « Campagne 2 » plus bas.

### Latence ajoutée à la frappe (`latency_added`)

Campagne 1, 2026-10-09 vers 13 h 40, prototype de 13 h 14 (latence sous flux corrigée par dev-rendu), release sans LTO, charge 4 à 10. Terminal de test 200×50 ; 500 frappes par ligne, espacées de 20 à 50 ms (graine fixe) ; écho dans le panneau actif. « Ajouté » : moins la référence directe de la même passe. tmux 3.8 avec `escape-time 10`, sans ligne d'état.

| Montage | Touche (terminal) | p50 | p99 | max | Ajouté p50 | Ajouté p99 | Seuil |
|---|---|---|---|---|---|---|---|
| mux, 1 panneau | x (kitty) | 0,37 | 1,90 | 7,3 | +0,25 | +1,64 | ok (2 / 16) |
| mux, 10 panneaux, 9 en flux | x (kitty) | 1,58 | 4,60 | 32,1 | +1,47 | +4,33 | ok (33) |
| mux, 10 panneaux, 9 en images de 120 Ko | x (kitty) | 0,65 | 4,92 | 8,7 | +0,53 | +4,65 | ok |
| tmux, 1 panneau | x (kitty) | 0,21 | 0,44 | 3,3 | +0,09 | +0,17 | |
| tmux, 10 panneaux, 9 en flux | x (kitty) | 7,87 | 14,17 | 18,5 | +7,76 | +13,91 | |
| tmux, 10 panneaux, 9 en images de 120 Ko | x (kitty) | 0,42 | 11,01 | 29,7 | +0,30 | +10,75 | |
| mux, 1 panneau | Échap (sans kitty) | 11,36 | 11,98 | 30,8 | +11,22 | +11,38 | attente voulue |
| mux, 10 panneaux, 9 en flux | Échap (sans kitty) | 12,32 | 15,86 | 44,5 | +12,18 | +15,26 | |
| mux, 1 panneau | Échap (kitty, `CSI 27u`) | 0,37 | 1,16 | 2,7 | +0,24 | +0,72 | ok |
| mux, 10 panneaux, 9 en flux | Échap (kitty) | 1,47 | 4,01 | 18,2 | +1,35 | +3,56 | ok |

(ms.) Premier passage, avant le correctif de dev-rendu : p99 ajouté +51 ms avec 9 panneaux en flux. tmux retient Échap 500 ms dans ce montage malgré `escape-time 10` (lu par le serveur, vérifié) : non expliqué, à revoir avec les frappes réelles.

### CPU (`cpu_rest_and_flood`)

Même passe. Processus mesuré : le mux, ou le serveur tmux. « Écrit » : ce que les factices ont réellement écrit (comptés par eux), donc ce que le multiplexeur a consommé.

| Montage | mux : CPU | tmux : CPU | mux : images/s | tmux : images/s | mux : écrit, CPU par Mio | tmux : écrit, CPU par Mio |
|---|---|---|---|---|---|---|
| 2 vrais Claude au repos, 60 s | 0,00 %, 0 réveil/s, 48 octets | 0,00 %, 0 réveil/s, 280 octets | 0 | 0 | | |
| 10 factices au repos, 60 s | 0,00 %, 0 réveil/s, 0 octet | 0,00 %, 0, 0 | 0 | 0 | | |
| 1 panneau à 20 Ko/s | 2,1 % | 1,05 % | 54 | 48 | 0,02 Mio/s | 0,02 Mio/s |
| 9 panneaux à 20 Ko/s | 3,8 % | 3,7 % | 60 | 354 | 0,18 Mio/s | 0,16 Mio/s |
| 1 panneau au débit max | 73 % | 96 % | 54 | 1018 | 64 Mio/s, 0,011 s | 24 Mio/s, 0,040 s |
| 9 panneaux au débit max | 377 % | 97 % | 54 | 386 | 149 Mio/s, 0,025 s | 26 Mio/s, 0,038 s |

Seuils : repos (0 octet, CPU nul, ≤ 1 réveil/s) ok ; 10 panneaux en flux à 50 images/s au moins : ok (54). Le mux lit 5,7 fois plus vite que tmux au débit max, pour 1,5 fois moins de CPU par Mio.

### libghostty-vt : compression de l'historique pendant un flux

Binaire `ghostty-eval flood-compress` de dev-terminal (hors de l'arbre), 120 colonnes, 50 rangées, 100 000 lignes du rejeu compressées d'abord, puis 30 s de flux du rejeu en tranches de 16 Kio.

| Débit | Compression | CPU pendant le flux | Écriture p99 (max) | Mémoire en fin de flux | Ensuite, au repos |
|---|---|---|---|---|---|
| 20 Kio/s | au repos | 0,0 % | 172 µs (183) | 30 Mio | 85 étapes, 4,7 ms de CPU, 21 Mio |
| 20 Kio/s | après chaque écriture | 0,0 % | 440 µs (598) | 28 Mio | 3,4 ms |
| 256 Kio/s | au repos | 0,2 % | 260 µs (353) | 119 Mio | 283 étapes, 52 ms, 23 Mio |
| 256 Kio/s | après chaque écriture | 0,4 % | 612 µs (1 037) | 31 Mio | 2,5 ms |
| 2 Mio/s | au repos | 1,2 % | 216 µs (2 948) | 119 Mio | 46 ms, 22 Mio |
| 2 Mio/s | après chaque écriture | 3,6 % | 1 031 µs (6 318) | 33 Mio | 2,9 ms |

Les maxima à 2 Mio/s sont peut-être gonflés : le test de frappes tournait en même temps.

### Étape 1 : libghostty-vt, le seul moteur (2026-10-09, après 16 h)

Prototype `_mux` sur libghostty-vt, release sans LTO, machine calme (charge 2 à 4 après un redémarrage). Mêmes montages que la campagne 1.

**Mémoire par panneau, 100 000 lignes** (`memory_through_mux`, empreinte, à travers le prototype) : lignes synthétiques 4,8 / 6,3 / 22,4 Mio (80, 120, 200 colonnes ; très compressibles) ; rejeu d'une vraie sortie de Claude, une fois la compression faite (`MEM_SETTLE=10`, et pareil à 30) : **10,4 / 12,7 / 14,5 Mio**, contre 35 / 42 / 66 pour tmux et 200 / 318 / 507 pour alacritty. Deux secondes après la dernière écriture, la compression n'a pas fini : 37 / 71 / 139 Mio. Mémoire virtuelle réservée : 78 à 193 Mio par panneau, pas résidente.

**CPU** (`cpu_rest_and_flood`) :

| Montage | mux : CPU | tmux : CPU | mux : images/s | mux : écrit, CPU par Mio | tmux : écrit, CPU par Mio |
|---|---|---|---|---|---|
| 2 vrais Claude au repos, 60 s | 0,00 %, 0 réveil/s, 0 octet | 0,00 %, 0, 280 octets | 0 | | |
| 10 factices au repos, 60 s | 0,00 %, 0, 0 | 0,00 %, 0, 0 | 0 | | |
| 9 historiques pleins (100 000 lignes chacun) au repos, 60 s après 30 s | **0,00 %, 0 réveil/s, 0 octet** | 0,00 %, 0, 0 | 0 | | |
| 1 panneau à 20 Ko/s | 5,1 % | 1,5 % | 45 | | |
| 9 panneaux à 20 Ko/s | 7,1 % | 5,5 % | 58 | | |
| 1 panneau au débit max | 89 % | 100 % | 52 | 74 Mio/s, 0,012 s | 26 Mio/s, 0,038 s |
| 9 panneaux au débit max | 471 % | 100 % | 52 | 143 Mio/s, 0,033 s | 28 Mio/s, 0,035 s |

Rien ne se réveille au repos une fois la compression finie. Au débit de Claude, le CPU monte par rapport à alacritty (2,1 et 3,8 %), surtout par image (environ 1,1 ms de CPU par image contre 0,4 ms). Au débit max, le CPU par Mio reste au niveau de tmux pour 5 fois son débit.

**Latence** (`latency_added`, ajouté = mux moins direct, ms) :

| Montage | x : p50 | x : p99 | x : max | Échap sans kitty : p50 | Échap avec kitty : p50 / p99 |
|---|---|---|---|---|---|
| mux, 1 panneau | +0,86 | +2,21 | 2,9 | +12,8 (attente voulue) | +0,61 / +2,12 |
| mux, 9 en flux | +1,54 | +3,09 | 4,0 | +13,4 | +1,37 / +2,96 |
| mux, 9 en images de 120 Ko | +0,75 | +1,86 | 3,0 | +12,7 | +0,70 / +3,02 |
| tmux, 9 en flux | +7,35 | +10,10 | 11,4 | (500 ms, non expliqué) | |

Tous les seuils sont tenus (p50 ≤ 2 ms et p99 ≤ 16 ms au repos ; p99 ≤ 33 ms sous flux ; 50 images/s ; rien au repos).

### Campagne 2 : serveur natif et vrai client (2026-10-09, 18 h 50 à 19 h 45)

Serveur `recruit _server` et client `_ctl attach` de l'arbre de 18 h 50, libghostty-vt, release sans LTO (`CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`), charge 2,7 à 4,9. Processus mesurés : le serveur **et** le client, additionnés (le prototype était un seul processus). tmux 3.8 : son serveur, comme avant. Mêmes montages que la campagne 1, terminal de test 200×50.

```sh
CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo test --release --test mux_bench -- --ignored --nocapture --test-threads=1 memory_through_mux
CLAUDE_SESSIONS=$(date +%F) … cpu_rest_and_flood        # la ligne des 2 vrais Claude, après le go du jour
… latency_added
```

**Mémoire par panneau, 100 000 lignes** (`memory_through_mux`, empreinte du serveur, pente vide → plein, `MEM_SETTLE=10`), lignes synthétiques :

| Colonnes | Natif (étape 1) | Prototype libghostty-vt (étape 1) | Prototype alacritty (campagne 1) | tmux 3.8, lignes typiques |
|---|---|---|---|---|
| 80 | 4,7 | 4,8 | 200,5 | 47 |
| 120 | 6,5 | 6,3 | 318,0 | 66 |
| 200 | 7,2 | 22,4 | 506,9 | 97 |

(Mio.) Serveur à vide avec un client : 6,0 à 7,1 Mio. Virtuel réservé : 78 / 124 / 193 Mio par panneau, pas résident. Rejeu d'une vraie sortie de Claude (20 h 20) : nouveau fichier de dev-terminal, `target/bench/claude-replay.bin` (28,3 Mo, sha256 `0fe1000f3d966c00…`), une session de Claude Code 2.1.295 en mode ligne à ligne (`tui: default`) enregistrée dans un panneau natif de 120×40, mise 140 fois bout à bout sans ses sondes ; rien du compte. Joué deux fois : la limite de 100 000 lignes est atteinte (51 916 lignes par passage à 120 colonnes, mesuré par dev-terminal).

| Colonnes | Natif, `MEM_SETTLE=10` | Natif, `MEM_SETTLE=30` | tmux 3.8, même rejeu | alacritty 0.26, même rejeu |
|---|---|---|---|---|
| 80 | 9,0 | 9,0 | 97,5 | 200,5 |
| 120 | 17,0 | 17,1 | 143,1 | 318,3 |
| 200 | 16,5 | 16,6 | 172,2 | 506,8 |

(Mio par panneau ; tmux et alacritty par `memory_per_pane`, pente de 1 à 10 panneaux.) Stable de 10 à 30 s : la compression est finie. Le natif prend 8 à 10 fois moins que tmux. Pas comparable au rejeu de la campagne 1 (autre fichier, 6,18 Mo, fait de transcriptions) : au prototype, celui-ci donnait 10,4 / 12,7 / 14,5 Mio contre 35 / 42 / 66 pour tmux.

```sh
BENCH_REPLAY=$PWD/target/bench/claude-replay.bin cargo test --release --test mux_bench -- --ignored --nocapture --test-threads=1 memory_through_mux memory_per_pane
```

**CPU** (`cpu_rest_and_flood`, serveur + client) :

| Montage | Natif : CPU, réveils/s, octets | Prototype : CPU | Alacritty (c. 1) : CPU | tmux : CPU, réveils/s, octets | Natif : images/s | tmux : images/s | Natif : écrit, CPU par Mio | tmux : écrit, CPU par Mio |
|---|---|---|---|---|---|---|---|---|
| 2 vrais Claude au repos, 60 s | **0,00 %, 0, 0** | 0,00 %, 0, 0 | 0,00 %, 0, 48 | 0,00 %, 0, 281 | 0 | 0 | | |
| 10 factices au repos, 60 s | **0,00 %, 0, 0** | 0,00 % | 0,00 % | 0,00 %, 0, 0 | 0 | 0 | | |
| 9 historiques pleins au repos, 60 s après 30 s | **0,00 %, 0, 0** | 0,00 % | | 0,00 %, 0, 0 | 0 | 0 | | |
| 1 panneau à 20 Ko/s | 4,3 %, 1,4 | 5,1 % | 2,1 % | 1,36 %, 51,8 | 51,9 | 51,8 | 0,02 Mio/s | 0,02 Mio/s |
| 9 panneaux à 20 Ko/s | 8,3 %, 55,8 | 7,1 % | 3,8 % | 5,09 %, 62,7 | 61,2 | 338,7 | 0,18 Mio/s | 0,18 Mio/s |
| 1 panneau au débit max | 82 % | 89 % | 73 % | 99,6 % | 58,9 | 926,6 | 86,3 Mio/s, 0,010 s | 26,8 Mio/s, 0,037 s |
| 9 panneaux au débit max | 455 % | 471 % | 377 % | 99,7 % | 58,7 | 399,2 | 146,5 Mio/s, 0,031 s | 28,2 Mio/s, 0,035 s |

Seuils : repos (0 octet, CPU nul, ≤ 1 réveil/s) **ok**, y compris 9 historiques pleins ; 10 panneaux en flux à 50 images/s au moins : **ok** (58,7). Au débit de Claude, le natif reste plus cher que tmux (4,3 % contre 1,4 % pour un panneau ; environ 0,8 ms de CPU par image envoyée contre 0,26 ms), au niveau du prototype : le coût par image vient du moteur et de la composition, pas du découpage serveur et client. Au débit max, il lit 3,2 fois plus vite que tmux (1 panneau) et 5,2 fois (9 panneaux), pour un CPU par Mio de 0,010 à 0,031 s contre 0,035 à 0,037.

**Latence** (`latency_added`, 500 frappes par ligne, ajouté = moins le direct de la même passe, ms) :

| Montage | Natif : x p50 / p99 / max | Prototype : x p50 / p99 | tmux : x p50 / p99 / max | Natif : Échap sans kitty p50 | Natif : Échap kitty p50 / p99 | tmux : Échap |
|---|---|---|---|---|---|---|
| 1 panneau | +0,42 / +1,20 / 4,1 | +0,86 / +2,21 | +0,10 / +0,31 / 8,5 | +12,4 (attente voulue) | +0,47 / +1,30 | +501 / +502 |
| 10 panneaux, 9 en flux | +1,39 / +2,84 / 16,1 | +1,54 / +3,09 | +7,00 / +9,76 / 11,4 | +13,3 | +1,55 / +2,56 | +507 / +510 |
| 10 panneaux, 9 en images de 120 Ko | +0,39 / +1,12 / 1,9 | +0,75 / +1,86 | +0,00 / +0,86 / 1,5 | +12,3 | +0,40 / +1,27 | +501 / +502 |

Seuils : p50 ≤ 2 ms et p99 ≤ 16 ms au repos **ok** (+0,42 / +1,20) ; p99 ≤ 33 ms avec 9 panneaux en flux **ok** (+2,84) ; 0 frappe perdue. Le natif fait mieux que le prototype sur toutes les lignes, et 5 fois mieux que tmux sous flux. Échap sans protocole kitty : l'attente voulue d'environ 12 ms (une séquence qui commence par Échap) ; tmux la retient toujours 500 ms dans ce montage, malgré `escape-time 10` (non expliqué, voir campagne 1). Première passe de latence interrompue à 19 h 05 pour le passage des frappes iTerm2 ; ces chiffres sont ceux de la relance de 19 h 12.
