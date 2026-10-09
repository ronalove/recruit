# Serveur, client et protocole du multiplexeur natif (étape 0, sur le papier)

Rédigé le 2026-10-09 par dev-serveur. Relu par reviewer, puis révisé le même jour (version 2). Référence : [`multiplexeur.md`](multiplexeur.md), §5.1 surtout. Rien n'est codé : ce document fixe ce que `src/mux/server.rs`, `client.rs`, `proto.rs` et l'implémentation `Native` du trait `Backend` feront à l'étape 1, si le prototype la décide.

Les choix techniques de ma zone sont pris et expliqués ici. Ce qui touche une autre zone est marqué **[architecte]** (le trait `Backend`, `Cargo.toml`, `state.rs`, `tmux.rs`, `mod.rs`) ou **[dev-saisie]**, **[dev-rendu]**, **[dev-terminal]**. Ce qui change ce que voit l'utilisateur est marqué **[utilisateur]** et n'est pas tranché. Les décisions déjà prises sont au §8.

Le modèle est celui du prototype (`src/mux/prototype.rs`) : un fil lecteur par PTY qui nourrit le moteur derrière un mutex, un fil principal qui route et dessine, réveillé seulement par un message, une image due ou l'échéance d'une mise à jour synchronisée. Le serveur, c'est cette boucle, avec des clients et des commandes à la place du terminal local.

## 1. Processus et cycle de vie

### 1.1 Lancement

`recruit _server <dossier d'état>` (sous-commande cachée) se comporte comme `tmux new-session -d` : **il rend la main quand le serveur est prêt** (code 0), ou en échec avec le message sur stderr (code 1). Le lanceur (`launch.rs`, par `Native::launch`) l'appelle par `Command::status()` et sait donc tout de suite si le serveur tourne.

Tout ce qui suit se passe avant le premier fil, ce qui rend les `fork` sûrs. `main.rs` n'en lance aucun avant d'aiguiller vers `_server` : un test le vérifie à l'étape 1.

1. **Chemins absolus.** Le dossier d'état passé en argument est rendu absolu (`canonicalize`), puisque le serveur fera `chdir("/")`. Le dossier de travail des membres l'est aussi : `Build` refuse un `Plan.dir` relatif.
2. **Descripteurs hérités.** Tout descripteur 3 et plus hérité du lanceur est fermé. La liste vient de `/dev/fd` (macOS et Linux), lue d'abord puis fermée, en écartant le descripteur du dossier lui-même. Le verrou, le tube et le journal ne sont ouverts qu'après : rien à épargner.
3. **Tube de démarrage**, ses deux bouts en `FD_CLOEXEC` (`pipe` puis `fcntl` : macOS n'a pas `pipe2` ; sans fil, pas de course). Puis `fork()`.
   - **Le parent** ferme le bout d'écriture et lit jusqu'à la fin du tube : `ok`, ou le message d'erreur, qu'il écrit sur stderr. Si le tube se ferme sans rien (le serveur est mort), il écrit les dernières lignes de `server.log`. Il récolte l'intermédiaire (`waitpid`) et sort.
   - **L'intermédiaire** fait `setsid()`, puis `fork()` de nouveau, et sort aussitôt (`_exit`).
   - **Le petit-fils est le serveur.** `setsid` l'a mis hors du groupe du lanceur, donc à l'abri du SIGHUP du terminal et de la fin d'une session SSH. N'étant pas chef de session, il ne peut jamais prendre de terminal de contrôle, même en ouvrant un tty sans `O_NOCTTY`. L'esclave d'un PTY est quand même ouvert avec `O_NOCTTY` **[dev-terminal]**.
4. `chdir("/")` : le serveur ne retient pas le dossier de l'équipe, chaque panneau reçoit le sien. stdin est mis sur `/dev/null`.
5. **Verrou.** `<état>/server.lock` est ouvert en `O_CLOEXEC` (le défaut de std), puis pris par `flock(LOCK_EX | LOCK_NB)`, réessayé pendant 2 s (un serveur qui s'arrête le relâche en sortant). S'il est pris ailleurs, `l'équipe tourne déjà` part sur le tube et le serveur sort. Il est pris **dans le serveur seulement, après les deux `fork`**. Le descripteur ne sort jamais du serveur : `CLOEXEC` le ferme à chaque `exec` des panneaux, et le serveur ne fait plus de `fork` sans `exec`. Le verrou n'est jamais relâché par `LOCK_UN`, seulement par la fermeture, à la sortie, comme `state.rs:109`. `flock` porte sur la description de fichier ouverte : une copie restée dans un autre processus garderait le verrou, et un `LOCK_UN` le relâcherait pour tous.
6. **Ménage.** `server.json` et le socket qu'il nomme, laissés par un serveur mort, sont retirés juste après la prise du verrou (§2.4). Ainsi, quand le verrou est tenu, `server.json` absent veut dire « serveur qui démarre », et présent veut dire « écrit par celui qui tient le verrou ».
7. **Journal.** stdout et stderr passent sur `<état>/server.log`, ouvert en ajout. Au démarrage, s'il dépasse 1 Mo, il est d'abord renommé en `server.log.1`. Cela se fait après le verrou : jamais sous le journal d'un serveur vivant.
8. **Signaux** (§1.5) et crochet de panique (§1.4).
9. Il ouvre le socket (§2), écrit `<état>/server.json` (§2.4), lance le fil d'écoute, écrit `ok` sur le tube, le ferme, et entre dans la boucle.

Le serveur démarre **vide** : le lanceur lui envoie ensuite le plan de l'équipe par une requête `Build` (§3.5) et reçoit sa réponse, comme `Tmux::launch` construit puis arrête l'équipe en cas d'échec (`tmux.rs:741`). Un panneau qui ne démarre pas remonte donc son erreur au terminal qui a lancé l'équipe. La taille de départ est celle du terminal du lanceur, ou 200 × 50 sans terminal (`--detach`, comme `Tmux::build`).

Pourquoi des `fork` plutôt que `Command` avec `setsid` dans `pre_exec` : le lanceur devient ensuite le client, dans le même processus. Un serveur resté son fils deviendrait un zombie à chaque arrêt de l'équipe, sauf à le récolter dans un fil ou à ignorer SIGCHLD, ce qui casserait tous les `Command::output()` du lanceur. Le second `fork` empêche le serveur, chef de session sinon, de prendre un terminal de contrôle.

Le serveur ne construit jamais une commande avec `current_exe()` : sous Linux, après une mise à jour, ce chemin finit par « (deleted) ». Les lignes de commande des membres viennent des requêtes, construites par le demandeur. Ce que le serveur lance lui-même (le panneau flottant du §4.4, les panneaux du tableau de bord) prend le chemin de recruit donné par `Build`, celui que le lanceur met déjà dans `RECRUIT_EXE` (`launch.rs:323`).

### 1.2 Vie du serveur

- Il survit à la perte du client : terminal fermé, SIGHUP du client, SSH coupé, client tué. Il ne fait que retirer le client de sa table (§6.1).
- Sans client, les panneaux gardent leur dernière taille. Rien n'est dessiné (pas de client, pas d'image) ; les moteurs continuent de lire leurs PTY, et leurs relais sont vidés (§6.2).
- Rien au repos : tous les fils sont bloqués dans `read`, `accept`, `recv` ou `sigwait`. La boucle n'a d'échéance que pour une image due ou une mise à jour synchronisée.
- **L'environnement est celui du lanceur, figé au lancement.** Un membre relancé après un retour depuis un autre terminal, ou par SSH, recevrait un `SSH_AUTH_SOCK`, un `DISPLAY`… périmés. Comme `update-environment` de tmux, le client transmet dans `Attach` (§3.4) une courte liste de variables : `SSH_AUTH_SOCK`, `SSH_AGENT_PID`, `SSH_CONNECTION`, `SSH_ASKPASS`, `DISPLAY`, `WAYLAND_DISPLAY`, `XAUTHORITY`, `KRB5CCNAME`, `WINDOWID`. Le serveur les applique aux panneaux qu'il lance ou relance ensuite, jamais à ceux qui tournent. Une variable absente du client est retirée.

### 1.3 Arrêt

Le serveur s'arrête :

- sur `Stop` (Quitter du menu, `recruit stop`, `recruit --restart`, `live.rs`), ou sur SIGTERM ou SIGINT ;
- quand plus aucun panneau de membre ne tourne, comme une session tmux dont tous les panneaux sont fermés (le shell laissé après Claude, quitté à son tour).

Dans l'ordre :

1. Il ne prend plus de requêtes et envoie `Bye { Stopped }` aux clients.
2. Il envoie SIGHUP au groupe de chaque panneau (`Pty::kill`, qui force après un délai ; `member.rs:59` ne relance rien sur un SIGHUP), puis attend leur fin, 3 s au plus.
3. Il retire `server.json`, puis le socket, seulement si c'est encore le sien (même `st_dev` et `st_ino` qu'au `bind`).
4. Il **répond** à `Stop`, et ne sort qu'une fois que le fil de commande dit avoir écrit la réponse (1 s au plus). Celui qui a demandé sait l'équipe arrêtée quand il reprend la main : un `recruit --restart` peut aussitôt relancer.
5. Il sort, ce qui relâche le verrou.

`recruit stop` sur un serveur qui ne répond pas en 5 s : il relit `server.json` et vérifie que le verrou est tenu (§2.4), puis envoie SIGTERM au `pid`, et SIGKILL 3 s plus tard. Si le verrou est libre, aucun signal n'est envoyé : le `pid` peut appartenir à un autre processus.

### 1.4 Paniques et plantage

- **Panique dans un lecteur de PTY** (le moteur d'un panneau : alacritty en a déjà eu sur un redimensionnement). C'est la seule rattrapée : `catch_unwind` autour de `feed`, comme le prototype. Le moteur du panneau est remplacé par un neuf, son titre le dit, `server.log` la note, et l'équipe continue (décision de l'architecte).
- **Toute autre panique** (la boucle, un fil de client, d'écoute, de commande, de signaux) : le crochet de panique l'écrit dans `server.log` avec sa trace, puis fait sortir le processus par `process::exit` (code 70). Ce n'est pas `abort`, qui laisserait un rapport de plantage sous macOS et un fichier core sous Linux. En `panic = unwind` (le défaut du projet), une panique de la boucle sur un autre fil que le principal laisserait sinon un serveur vivant, mais sans boucle. Le crochet distingue les lecteurs de PTY par une marque locale au fil, posée autour de `feed`.
- **Ce qui arrive aux membres** quand le serveur sort ou plante : les maîtres des PTY se ferment avec le processus, et le noyau envoie SIGHUP à la session de chaque panneau (`sh`, `recruit _member` et Claude sont tous dans le groupe au premier plan, `sh -c` ne faisant pas de contrôle des tâches). `member.rs` sort sans relancer, et Claude s'arrête. Les conversations restent sur disque et se reprennent (`recruit <équipe> --resume`). Cela suppose que les panneaux aient leurs signaux par défaut (§1.5).
- **Le client** lit la fin du socket sans `Bye` : il rend le terminal et dit « le serveur de l'équipe s'est arrêté sans prévenir (voir `<état>/server.log`) ».
- **Au lancement suivant**, le verrou est libre et `server.json` est là : le lanceur sait que l'arrêt n'a pas été propre. Proposition **[utilisateur]** : reprendre alors les conversations sans demander, en le disant, comme `repair` relance sur leur conversation les membres arrêtés (`launch.rs:336`).

Survivre à la mort du serveur (garder les PTY ailleurs) demanderait un second processus gardien et le passage de descripteurs : tmux ne le fait pas, je ne le propose pas.

### 1.5 Signaux

**Comment un signal atteint la boucle.** Avant tout fil, le serveur bloque SIGTERM, SIGINT, SIGHUP et SIGURG (`pthread_sigmask`). Tous les fils héritent de ce masque. Un fil dédié boucle sur `sigwait`, et passe chaque signal à la boucle par un message :

- SIGTERM, SIGINT : arrêt propre (§1.3) ;
- SIGHUP : noté dans le journal, sans effet (le serveur n'a pas de terminal) ;
- SIGURG : recréer le socket (§2.4). SIGURG est ignoré par défaut : envoyé par erreur à un autre processus, il ne le tue pas, contrairement à SIGUSR1.

Je choisis `sigwait`, sans dépendance, plutôt que `signal-hook`, qui demanderait l'accord de l'architecte pour `Cargo.toml` : quatre signaux et un fil ne justifient pas une caisse. SIGPIPE reste ignoré, comme le fait déjà le runtime de Rust : une écriture vers un client parti rend `EPIPE`.

**Ce que les panneaux en reçoivent.** Les dispositions ignorées et le masque de signaux passent à travers `exec`. Hérités tels quels, SIGHUP bloqué et SIGPIPE ignoré feraient survivre les membres à un plantage du serveur, et forceraient chaque arrêt à passer par SIGKILL. L'enfant d'un PTY, entre `fork` et `exec`, remet donc toutes les dispositions à `SIG_DFL` et vide son masque **[dev-terminal, prévenu par reviewer]**. Un test le vérifie (§7).

## 2. Socket

### 2.1 Dossier

`${RECRUIT_TMPDIR:-/tmp}/recruit-<uid>/`, comme tmux (`TMUX_TMPDIR`). `/tmp` plutôt que `$TMPDIR` : sous macOS, `$TMPDIR` est long (`/var/folders/…/T/`) et peut manquer dans une session SSH ; le même chemin doit servir depuis le terminal graphique et par SSH. `RECRUIT_TMPDIR` doit être absolu, sinon il est ignoré avec un avertissement.

Le dossier est créé par `mkdir(0700)`. S'il existe déjà, `lstat` doit dire : un dossier (pas un lien), à nous (`st_uid == getuid()`), sans aucun droit pour le groupe ni les autres (`mode & 0o077 == 0`). Sinon, refus, avec le chemin et la raison, comme tmux : un autre utilisateur aurait pu le préparer.

### 2.2 Nom par équipe, et la limite de `sun_path`

Le socket se nomme `<session>-<h>.sock` : `session` est celle de `tmux::session_name`, suffixe `-dry-run` compris, et `h` les 8 premiers chiffres hexadécimaux d'un hachage FNV-1a 64 bits du **dossier d'état absolu** (dix lignes, sans dépendance). Le hachage lie le socket au verrou. Deux `XDG_CACHE_HOME` différents avec un même `/tmp` (un serveur de test à côté de l'équipe de l'utilisateur) donnent deux sockets différents, et le serveur de test ne peut pas prendre celui de l'équipe vivante.

`sun_path` fait 104 octets sous macOS, NUL compris (108 sous Linux) : on vise 103 octets partout. Un nom d'équipe fait 40 caractères au plus, mais en lettres Unicode (`config.rs:714`), soit jusqu'à 160 octets, plus `-dry-run`. Si le chemin dépasse, la session est coupée (sur un caractère) avant `-<h>`, et le hachage porte alors sur la session entière et le dossier d'état. Si même le dossier ne laisse pas 24 octets, refus : « le dossier des sockets est trop long (`RECRUIT_TMPDIR`) ».

Personne ne recalcule ce chemin pour joindre le serveur : il est écrit dans `server.json` (§2.4).

**Avant tout `unlink`, on se connecte.** Si un fichier existe déjà au chemin du socket, le serveur s'y connecte d'abord. Si un serveur répond, il refuse de démarrer (« un autre serveur répond sur `<chemin>` ») et ne retire rien. Il ne retire le fichier qu'en cas de `ECONNREFUSED`. La même règle vaut pour le ménage du §1.1, point 6, et pour la recréation du §2.4.

Le socket est créé par `bind`, puis `chmod 0600` (le dossier en 0700 ferme déjà la fenêtre entre les deux). Son `st_dev` et son `st_ino` sont gardés pour l'arrêt (§1.3). Les tampons sont montés à 256 Kio : `SO_SNDBUF` côté serveur, `SO_RCVBUF` côté client. Sous macOS, le tampon par défaut d'un socket Unix (8 Kio) découperait une image pleine en dizaines d'écritures.

### 2.3 Uid du pair

Le serveur vérifie l'uid de chaque connexion avant de lire quoi que ce soit : `getpeereid` sous macOS (dans `libc`), `getsockopt(SO_PEERCRED)` sous Linux (`libc::ucred`, glibc et musl). Un autre uid est fermé sans réponse et noté dans `server.log`. Le client vérifie de même que le serveur est à lui. `UnixStream::peer_cred` de std n'est pas stable : deux petites fonctions sous `cfg`.

### 2.4 Où est le serveur, et le socket orphelin

L'autorité est le **verrou dans le dossier d'état**, pas le socket :

- `<état>/server.lock`, tenu par `flock` toute la vie du serveur (§1.1, point 5). Le dossier d'état, sous `~/.cache`, n'est jamais nettoyé par le système.
- `<état>/server.json`, écrit par `config::write_atomic` : `{ "pid", "socket", "proto", "version", "started" }`. Il est retiré à l'arrêt propre, et retiré s'il est périmé dès la prise du verrou (§1.1, point 6).

Pour joindre l'équipe dont on a le dossier d'état (`_mod`, `_edit`, `live.rs`, le tableau de bord), on lit `server.json` et on se connecte au chemin qu'il donne. En cas d'échec, on regarde le verrou :

- **verrou libre** (`flock(LOCK_NB)` réussit) : le serveur est mort. On retire `server.json`, et le socket selon la règle du §2.2, en tenant le verrou (un serveur qui démarre attend). L'équipe ne tourne pas. **Aucun signal n'est envoyé** : le `pid` noté peut désormais appartenir à un autre processus ;
- **verrou tenu, `server.json` absent** : le serveur démarre. On réessaie pendant 2 s ;
- **verrou tenu, `server.json` présent** : il a été écrit par le serveur qui tient le verrou, donc son `pid` est le bon. Si le socket a disparu (nettoyage de `/tmp` par `systemd-tmpfiles`, ou à la main), on envoie SIGURG au `pid`. Le serveur recrée le socket et lance un nouveau fil d'écoute ; on réessaie une fois (tmux fait de même avec SIGUSR1). L'ancien fil d'écoute reste bloqué dans `accept` sur l'ancien descripteur : sous macOS, `shutdown` ne le réveille pas. On le laisse, ce qui coûte un fil et un descripteur par recréation, qui reste rare.

Il reste une fenêtre de quelques microsecondes entre la prise du verrou et le ménage du §1.1, où `server.json` nomme encore l'ancien `pid`. Un signal n'y part qu'après un échec de connexion et la vérification du verrou ; `recruit stop` attend en plus 5 s et relit `server.json` avant de signaler (§1.3).

`recruit list` parcourt les dossiers d'état (`~/.cache/recruit/teams/*/server.json`) et interroge chaque serveur (`Hello`, §3.3) : quelques dixièmes de milliseconde par équipe. Pendant la transition, il fusionne avec les sessions du serveur tmux.

Deux `recruit` qui lancent la même équipe en même temps : un seul prend le verrou, l'autre échoue au §1.1, point 5, avec « l'équipe vient d'être lancée ailleurs ».

## 3. Protocole

### 3.1 Trames

Chaque trame : une longueur sur 4 octets (gros-boutiste, `u32`), un octet de type, la charge. La longueur compte l'octet de type et la charge, pas elle-même.

| Type | Sens | Charge |
|---|---|---|
| 0 | les deux | un message JSON (`serde_json`) |
| 1 | serveur → client | des octets à écrire tels quels sur le vrai terminal : une image (déjà entourée de BSU/ESU) ou un relais (OSC 52, titre, notification) |

Une trame annoncée au-delà de 64 Mio ferme la connexion. La charge est lue par morceaux de 64 Kio, dans un tampon qui grandit avec ce qui arrive : une longueur annoncée n'est jamais allouée d'avance. Un collage passe dans un message JSON (100 Ko de collage font environ 100 Ko de JSON) ; au-delà de 32 Mio, le client le coupe et le dit **[dev-saisie]**.

### 3.2 Encodage : `serde_json`, sans dépendance nouvelle

- `serde` et `serde_json` sont déjà là : rien à ajouter à `Cargo.toml`, rien de plus à construire en musl.
- Le gros du trafic, les images, ne passe pas par JSON : c'est la trame de type 1, des octets bruts produits par `canvas::Painter`.
- Le reste est petit et rare : une touche fait une centaine d'octets, analysée en une microseconde environ. Même la souris en mode 1003 (une centaine d'événements par seconde) reste sous 10 Ko/s.
- `postcard` ou `bincode` seraient plus compacts, mais ajouteraient une dépendance pour un gain qu'on ne mesurerait pas. La latence ajoutée à la frappe sera mesurée à l'étape 1. Cible : moins de 0,2 ms entre la lecture de la touche par le client et l'écriture dans le PTY.

Les messages sont des `enum` Rust sérialisés par serde, étiquetés à l'extérieur (`{"key": {...}}`). Un test compare la forme JSON d'un jeu de messages fixe à un texte de référence : un changement de forme qui n'a pas monté `PROTO` fait échouer le test.

### 3.3 Version et refus clair

Deux nombres : `PROTO`, un entier monté à chaque changement incompatible du protocole, et la version de recruit (`CARGO_PKG_VERSION`), pour les messages. On compare `PROTO`, pas la version (décision de l'architecte, §8).

**Une partie est figée pour toujours**, pour que n'importe quel recruit puisse toujours lister et arrêter n'importe quelle équipe : le format des trames, `Hello`, `Welcome`, `Refused`, la requête `Stop` et sa réponse.

```rust
// client → serveur, premier message
Hello { proto: u32, version: String, kind: Kind }         // Kind: Attach | Command | Other (#[serde(other)])
// serveur → client, toujours, quel que soit le proto du client
Welcome { proto: u32, version: String, team: String, session: String, dir: String, pid: u32, attached: bool }
// serveur → client, à toute demande qu'il ne prend pas
Refused { reason: Refusal }                               // Refusal: Proto | Busy | Other (#[serde(other)])
// requête de commande acceptée quel que soit le proto, et sa réponse
"stop"  →  Reply::Ok(null) | Reply::Err(String)
```

Déroulement :

1. Le client envoie `Hello`. Le serveur répond **toujours** `Welcome`, qui porte son `proto`. C'est ce que lit `recruit list`, quelle que soit la version de chacun.
2. Le client compare. Avec le même `PROTO`, la suite est normale, même si les versions de recruit diffèrent. Avec un `PROTO` différent, il n'envoie que `Stop` (pour `recruit stop`), ou rien. Dans ce cas, il dit dans sa langue, par `t!` :

   > L'équipe « mux » tourne avec recruit 1.4.0, dont le protocole diffère de celui de ce recruit (1.5.0). Pour la reprendre avec celui-ci : `recruit stop mux`, puis `recruit mux --resume` (chaque membre reprend sa conversation).

3. Le serveur, de son côté, sait que le `proto` du `Hello` diffère du sien : sur cette connexion, il ne prend que `Stop`, et répond `Refused { Proto }` à tout le reste. Un `Kind` qu'il ne connaît pas reçoit `Refused { Other }`.

**Règles serde pour les formes figées** (testées) :

- jamais `deny_unknown_fields` : un recruit plus récent peut ajouter des champs ;
- tout champ ajouté après coup en `#[serde(default)]` ;
- une variante fourre-tout `#[serde(other)]` dans `Kind` et `Refusal` : une valeur inconnue se lit, au lieu de faire échouer le message ;
- pas de `PathBuf` : `Welcome.dir` est une `String` (`to_string_lossy`), puisque `serde_json` échoue sur un chemin qui n'est pas de l'UTF-8.

Pourquoi `PROTO` plutôt que la version : le client est mince (des événements en entrée, des octets en sortie), son contrat changera rarement, et une mise à jour de recruit ne coupe pas l'accès aux équipes lancées. Après `brew upgrade`, `RECRUIT_EXE` des membres pointe sur le nouveau binaire ; avec l'égalité stricte des versions, `/recruit` et `/equipe` d'une équipe lancée avant la mise à jour répondraient par un refus.

### 3.4 Session d'un client (`Kind::Attach`)

Après `Welcome`, le client envoie `Attach`, puis le flux d'événements.

```rust
// client → serveur
Attach { caps: Caps, cols: u16, rows: u16, term: String, lang: Lang, env: Vec<(String, String)>, takeover: bool }
Input(Event)            // touche, souris, collage, focus gagné ou perdu
Resize { cols: u16, rows: u16 }
Redraw                  // image complète demandée (après SIGCONT, ou un terminal effacé)
Detach                  // le client part de lui-même (SIGTERM reçu)

// serveur → client
Attached { client: String }   // l'identifiant de ce client (« c3 »)
Output(bytes)                 // trame de type 1
Bye { reason: Bye }           // Detached | Replaced | Stopped | Ended | Error(String) | Other (#[serde(other)])
```

- `Caps` est celui de `src/mux/mod.rs`, rempli chez le client par `input::Terminal::open`. `Caps` et `canvas::Features` dérivent `Serialize`/`Deserialize` (§8).
- `term` est le `TERM` du client, et `Caps.name` le nom et la version du terminal (XTVERSION). C'est ce qu'il faut au tableau de bord pour les icônes Nerd Font (`client_terminals`, `board.rs:322`). Par SSH, `TERM_PROGRAM` ne passe pas, XTVERSION oui.
- `env` : les variables de la liste du §1.2, telles que le client les a.
- `Event` : en attente du choix de dev-saisie entre les événements de crossterm et des types propres à recruit. L'architecte penche pour les nôtres : avec crossterm, des réponses tardives du terminal sont lues comme des touches, les drapeaux kitty 4 et 16 se perdent, et 0x08 et 0x1C à 0x1F aussi. Avec nos types, ils dérivent `Serialize`/`Deserialize`, et la fonction `serde` de crossterm n'est pas utile ; avec ceux de crossterm, elle l'est (aucune caisse nouvelle). Dans les deux cas, le test de forme du §3.2 attrape un changement.
- `lang` : la langue du client, pour les textes qu'il écrit lui-même (`Bye`, refus). Le serveur garde la langue de l'équipe pour ce qu'il dessine.
- **Identifiant de client** : partout une `String` (« c1 », « c2 »…), attribuée par le serveur et jamais réutilisée pendant sa vie. C'est le même type que le nom de client de tmux, ce que prend déjà `Backend::detach(&str)`.
- Le serveur répond à `Attach` en appliquant `env` (§1.2), en donnant aux moteurs les couleurs du terminal (`Engine::set_colors`), en redimensionnant les panneaux si la taille a changé, puis en envoyant `Attached` et une image complète (un nouveau `Painter` avec les capacités de ce client).

### 3.5 Requêtes de commande (`Kind::Command`)

Une connexion, une requête, une réponse, fermeture. Le coût d'un aller-retour (connexion, `Hello`, requête, réponse) est de l'ordre de 0,1 à 0,3 ms ; `live.rs` en fait une dizaine par changement.

```rust
// client → serveur
Request::Build { plan: Plan, cols: u16, rows: u16 }   // launch.rs : le plan de l'équipe, dont le chemin de recruit
Request::Stop                                         // figé
Request::Panes                                        // → Vec<PaneState>
Request::OpenWindow { pane: Pane }                    // → id du panneau
Request::KillPane { id }
Request::SetMember { id, member }
Request::Respawn { id, pane: Pane }
Request::Arrange { tabs: Vec<layout::Tab>, columns }
Request::OpenPanels { beside, dashboard: Pane, journal: Pane }   // tant que les panneaux sont des processus
Request::ClosePanels
Request::RestoreDashboard { dashboard: Pane }
Request::ToggleJournal                                // → JournalSize
Request::Focus { member }                             // → bool
Request::OpenMenu { member: Option<String>, client: Option<String> }   // → Opened | NoClient
Request::Detach { client: String }
Request::Clients                                      // → Vec<ClientInfo { id, term, name, cols, rows }>
// outils de test et de capture (§4.1)
Request::Capture { pane: Option<String>, styles: bool }   // → texte du panneau, ou de l'écran composé si None
Request::Send { pane: String, bytes: Vec<u8> }            // octets écrits tels quels dans le PTY (send-keys)
Request::Key { event: Event }                             // événement routé comme s'il venait du client (send-keys -K)

// serveur → client
Reply::Ok(serde_json::Value) | Reply::Err(String)
```

- `_mod` (par `bridge.rs`), `_edit` (par `live.rs`), `stop` et `list` passent tous par là ; `list` n'envoie que `Hello`.
- Délais côté demandeur : 5 s par défaut, 30 s pour `Build` (il démarre tous les panneaux), 10 s pour `Stop`.
- Côté serveur, chaque connexion de commande a son fil, court. Il lit la requête avec un délai de lecture de 5 s (un pair qui n'envoie rien ne retient rien), la passe à la boucle par un message avec un canal de réponse, attend la réponse et l'écrit. Seule la boucle touche l'état.
- Les identifiants de panneau sont attribués par le serveur (`p1`, `p2`…) et jamais réutilisés pendant sa vie ; `PaneState.id` reste une `String` pour le trait. `Pane.role`, aujourd'hui un `Option<&'static str>` (`tmux.rs:58`), devient un `enum`. `Plan`, `TabPlan`, `Side`, `Pane`, `PaneState`, `JournalSize` et `layout::Tab` dérivent `Serialize`/`Deserialize` (§8). `Plan.toggle`, `click` et `menu` (`launch.rs:215-218`), propres à tmux, sont ignorés par `Native`.
- **Atomicité** : chaque requête est appliquée en un tour de boucle. Une suite de requêtes peut donc laisser voir un état intermédiaire pendant une image (un panneau ouvert, puis rangé). Si ça se voit, `Request::Batch(Vec<Request>)` les applique en un seul tour, sans image entre elles.
- **Dans le serveur** (étape 2) : le menu devient un calque du serveur, et ses `Effect` appellent `live.rs`, qui fait des entrées-sorties lentes (`config::edit`, `claude agents`). Ces appels tournent sur un fil de travail du serveur, jamais sur la boucle ; `Native` y parle à la boucle par le même message, sans socket (§4.3).
- **Commande cachée `recruit _ctl <état> capture|send|key|clients …`** : les requêtes de test en ligne de commande, pour `scripts/screenshots.sh`, la grille du testeur et la méthode de test du CLAUDE.md. Elle remplace `capture-pane`, `send-keys` et `run-shell` (§4.1). Le client réel reste utilisable sous `script -q /dev/null recruit attach`, comme on fait aujourd'hui avec tmux.

## 4. Ce qui demande quelque chose à `Tmux` aujourd'hui, et ce qui le remplace

### 4.1 Appel par appel

Le choix du moteur (`Tmux::new(&found.tmux)`, `Tmux::running(…)`) devient `backend::of(…)`, qui rend `Tmux` ou `Native` : selon `RECRUIT_BACKEND` au lancement, puis selon l'équipe pour une équipe lancée (§4.2).

| Appel | Où | Remplacé par |
|---|---|---|
| `tmux::session_name` | `launch.rs:39`, `app.rs:263`, `app.rs:324`, `app.rs:354` | inchangé (fonction pure, à déplacer hors de `tmux.rs` au retrait) |
| `Tmux::new(&found.tmux)` | `launch.rs:43` | `backend::of` |
| `has_session` | `launch.rs:46`, `app.rs:355`, `app.rs:358` | `server.json` lu, connexion, `Hello` → `Welcome` (orphelin nettoyé, §2.4) |
| `running()` | `launch.rs:47`, `app.rs:262`, `app.rs:368` | parcours des dossiers d'état, `Hello` à chacun (§2.4) |
| `stop` | `launch.rs:59`, `launch.rs:83`, `app.rs:413`, `live.rs:158` | `Request::Stop` |
| `panes` (dans `repair`) | `launch.rs:340` | `Request::Panes` |
| `respawn` (dans `repair`) | `launch.rs:363` | `Request::Respawn` |
| `restore_dashboard` (dans `repair`) | `launch.rs:386` | `Request::RestoreDashboard` ; à l'étape 2, vue native : sans objet |
| `launch(&plan)` | `launch.rs:243` | `recruit _server <état>` (§1.1), puis `Request::Build` ; `Request::Stop` en cas d'échec |
| `attach` (par `attach_or_hint`) | `launch.rs:80`, `launch.rs:255`, `launch.rs:422`, `app.rs:394` | le client (§5), dans le même processus |
| `Tmux::running(socket)` | `live.rs:38`, `live.rs:563`, `live.rs:575` | `backend::of(&snapshot)` |
| `detach(client)` | `live.rs:153` | `Request::Detach { client }` ; l'identifiant du client vient du calque du menu |
| `panes` | `live.rs:192`, `live.rs:284` | `Request::Panes` |
| `set_member` | `live.rs:208` | `Request::SetMember` (le nom dans l'en-tête natif) |
| `kill_pane` | `live.rs:253` | `Request::KillPane` |
| `open_window` | `live.rs:271` | `Request::OpenWindow` |
| `close_panels` | `live.rs:286`, `live.rs:294` | `Request::ClosePanels` ; à l'étape 2, vues natives retirées du modèle d'écran |
| `open_panels` | `live.rs:295` | `Request::OpenPanels` ; à l'étape 2, vues natives |
| `arrange` | `live.rs:303` | `Request::Arrange` : le modèle d'écran prend les onglets tels quels, sans la danse de `swap-pane`, `join-pane` et `break-pane` (`tmux.rs:450-539`) |
| `respawn` | `live.rs:310` | `Request::Respawn` |
| `client_of` + `popup` (`open_menu`) | `live.rs:561-567` | `Request::OpenMenu { member }` : le serveur choisit le client qui montre ce membre, sinon son onglet, sinon le seul ; `NoClient` sinon |
| `client_look` + `popup_size` + `popup` (`popup`) | `live.rs:573-581` | `Request::OpenMenu { client }`, ou rien : `Alt+r` et le bouton sont traités dans le serveur |
| `running()` + `session_name` (`list`) | `app.rs:257-265` | parcours des dossiers d'état (§2.4) |
| `target` | `app.rs:345-379` | idem, puis `has_session` natif |
| `click` | `app.rs:423` | sans objet : le serveur reçoit le clic et sait sur quel panneau et quelle case il tombe. Il appelle `board::compaction_at` et `board::member_at` sur un fil de travail, puisqu'ils lisent `zones.json` |
| `confirm` | `app.rs:434` | calque de confirmation natif (étape 2) |
| `focus` | `app.rs:437` | interne au serveur ; `Request::Focus` pour les autres processus |
| `client_terminals` | `board.rs:322-323` | `Request::Clients` tant que le tableau de bord est un processus ; à l'étape 2, lu directement |
| `toggle_journal` (par `board::toggle`) | `board.rs:1833-1841`, appelé par `bridge.rs:904` (`/equipe`) | `Request::ToggleJournal` ; `Alt+j` est traité dans le serveur |
| `live::open_menu` | `bridge.rs:910` | `Request::OpenMenu { member }` |
| attente de `menu.opened` | `bridge.rs:917-928` | sans objet en natif : la réponse dit si le calque s'est ouvert. Les textes « Aucun client tmux… » et « tmux n'a pas ouvert le menu… » ont besoin d'une variante native (`bridge.rs` n'est pas à moi) |
| `menu.rs` : `running.detach`, `running.stop` | `menu.rs:392`, `menu.rs:397` | par `live::Running`, donc `Request::Detach` et `Request::Stop` |
| `tmux_layout` (chaîne de disposition de `select-layout`) | `layout.rs:184` | en natif, il faut des rectangles : une fonction sœur qui rend un `Rect` par panneau, à partir des mêmes entrées (`layout.rs` n'est pas à moi : **[architecte, dev-rendu]**) |
| « tmux <session>:… » dans le prompt | `prompt.rs:69-83` | dépend de la question 1 du §10 de la spec (ce que ListAgents montre hors de tmux) |
| `tmux::ALT` (⌥ ou Alt+) | `bridge.rs:914`, `board.rs:30` (utilisé à `board.rs:2166`), `cli.rs:331` | à déplacer hors de `tmux.rs` (dans `look.rs` ou `i18n.rs`, **[architecte]**), comme le prévoit la spec (§5.4) |
| `tmux::DASHBOARD`, `tmux::JOURNAL` | `launch.rs:195`, `launch.rs:201`, `launch.rs:379`, `launch.rs:382`, `live.rs:284`, `live.rs:298`, `live.rs:299`, `app.rs:425`, `app.rs:426`, `board.rs:1837` | les variantes de l'`enum` de `Pane.role` (§3.5) |
| outils de test et de capture : `capture-pane`, `send-keys`, `send-keys -K`, `run-shell -t`, `attach` sous `script` | `scripts/screenshots.sh`, la méthode de test du CLAUDE.md (« Pièges »), la grille du testeur | `Request::Capture`, `Request::Send`, `Request::Key` par `recruit _ctl` (§3.5) ; `script -q /dev/null recruit attach` pour un vrai client |

Ce qui reste propre à tmux et ne passe pas dans le trait : `config`, `build`, les liaisons (`click_binding`, `status_click`, `quit_menu`), `Click`, `confirm`, `popup`, `popup_size`, `client_look`. `check_duplicates` (`launch.rs:429`) dépend, comme le prompt, de la question 1.

### 4.2 L'équipe lancée sait par quoi elle tourne

`Snapshot.socket` (`state.rs:24`) nomme le serveur tmux (`-L`). Accordé par l'architecte, à l'étape 1 : un champ `backend`, `tmux` par défaut pour les équipes déjà lancées. `Native` retrouve son serveur par `server.json`, dans le même dossier d'état, donc `socket` reste vide en natif. `backend::of(&snapshot)` lit ce champ, pas `RECRUIT_BACKEND` : `_mod`, le menu et `_edit` d'une équipe tmux restent sous tmux même si la variable change.

### 4.3 Croissance du trait `Backend`

Décision de l'architecte : les primitives ci-dessous jusqu'au retrait de tmux. Il code le trait à l'étape 1 ; `apply` est à repenser à l'étape 4.

Aujourd'hui (`tmux.rs:153`), le trait couvre `running`, `launch`, `attach` et `stop`. Il grandit des primitives ci-dessus, avec la session en argument comme aujourd'hui :

```rust
pub trait Backend {
    // Équipes
    fn running(&self) -> Result<Vec<RunningTeam>>;
    fn has_session(&self, session: &str) -> bool;
    fn launch(&self, plan: &Plan) -> Result<()>;
    fn attach(&self, session: &str) -> Result<()>;
    fn stop(&self, session: &str) -> Result<()>;
    // Panneaux des membres
    fn panes(&self, session: &str) -> Result<Vec<PaneState>>;
    fn open_window(&self, session: &str, dir: &Path, pane: &Pane) -> Result<String>;
    fn kill_pane(&self, id: &str) -> Result<()>;
    fn set_member(&self, id: &str, member: &str) -> Result<()>;
    fn respawn(&self, id: &str, dir: &Path, pane: &Pane) -> Result<()>;
    fn arrange(&self, session: &str, tabs: &[Tab], columns: usize) -> Result<()>;
    // Tableau de bord et journal, tant qu'ils sont des processus
    fn open_panels(&self, dir: &Path, beside: &str, dashboard: &Pane, journal: &Pane) -> Result<()>;
    fn close_panels(&self, session: &str) -> Result<()>;
    fn restore_dashboard(&self, session: &str, dir: &Path, dashboard: &Pane) -> Result<()>;
    fn toggle_journal(&self, session: &str, dir: &Path, journal: &Pane) -> Result<JournalSize>;
    // Clients
    fn focus(&self, session: &str, member: &str) -> Result<bool>;
    fn detach(&self, client: &str) -> Result<()>;
    /// Remplace `client_of`, `client_look`, `popup_size` et `popup` : chaque moteur trouve le client et ouvre le menu.
    fn open_menu(&self, session: &str, member: Option<&str>, client: Option<&str>) -> Result<MenuOpened>;
    fn client_terminals(&self, session: &str) -> String;
}
```

- `Tmux` l'implémente avec ses fonctions actuelles, presque sans changement (`open_menu` regroupe `live.rs:561-581`).
- `Native` (proposé dans `src/mux/native.rs`, ma zone) tient un lien : le socket du serveur pour les autres processus, ou un `Sender` vers la boucle pour les fils de travail du serveur (étape 2). Chaque méthode est une requête du §3.5.
- Les outils de test (`Capture`, `Send`, `Key`) restent hors du trait : tmux a déjà les siens, et `_ctl` ne sert qu'en natif.

### 4.4 Le menu à l'étape 1

La spec met le menu en calque à l'étape 2. D'ici là, `Alt+r`, le bouton et `/recruit` n'ont pas de `display-popup`. Proposition : un « panneau flottant », un PTY de plus qui lance `recruit _menu <état> --client <id>` et que l'écran dessine au centre, au-dessus du reste. Il se ferme quand son programme sort. Tout est déjà là (PTY, moteur, composition) ; il sert de pont jusqu'au calque natif, puis disparaît. Sinon, `/recruit` répond « pas encore en natif » pendant l'étape 1. L'architecte penche pour le panneau flottant et tranchera à l'ouverture de l'étape 1.

## 5. Détacher et revenir

### 5.1 Le client

En natif, `recruit`, `recruit attach` et `recruit <équipe>` sur une équipe lancée deviennent le client : connexion, `Hello`, `Attach`, puis deux fils et le fil principal :

- le fil des événements du terminal (`input::read_events`, dev-saisie) envoie `Input` et `Resize` ;
- le fil principal lit le socket et écrit les trames `Output` sur le terminal (`Terminal::write`, une écriture par trame) ;
- à `Bye`, ou à la fin du socket, il rend le terminal (`Drop` de `Terminal`), écrit la raison par `t!`, et sort.

Signaux du client :

- SIGHUP (terminal fermé, SSH coupé) : sortie sans rien écrire ;
- SIGTERM : `Detach`, terminal rendu, sortie ;
- SIGTSTP venu de l'extérieur (`kill -TSTP`) : terminal rendu, puis arrêt (SIGSTOP à soi-même) ;
- SIGCONT : mode brut rétabli et `Redraw`.

En mode brut, Ctrl-Z et Ctrl-C sont des touches, envoyées au panneau.

Un `recruit attach` lancé depuis un panneau de la même équipe est refusé : le `RECRUIT_STATE` des membres (`launch.rs:324`) est égal au dossier visé, et l'équipe se montrerait dans elle-même. Depuis une autre équipe ou depuis un tmux, il s'imbrique, comme tmux dans tmux.

### 5.2 Depuis un autre terminal

Rien n'est lié au premier terminal. À chaque `Attach`, le serveur :

- prend les capacités (`Caps`), la taille, le `TERM` et l'environnement (§1.2) du nouveau client ;
- refait son `Painter` ;
- redimensionne les panneaux (`TIOCSWINSZ`, donc SIGWINCH à Claude Code) ;
- envoie une image complète.

Le protocole clavier envoyé aux panneaux dépend de leurs modes, pas du terminal : il ne change pas. Les couleurs OSC 10 et 11 du nouveau terminal sont données aux moteurs ; un Claude Code déjà lancé garde le thème qu'il a choisi à son démarrage.

### 5.3 Par SSH

- Le serveur, détaché par `setsid`, survit à la session SSH qui l'a lancé et à celle qui s'y est attachée.
- Le dossier des sockets est `/tmp/recruit-<uid>` dans les deux cas (§2.1), et `server.json` donne le chemin exact : un `RECRUIT_TMPDIR` absent de la session SSH ne perd pas l'équipe, tant que `XDG_CACHE_HOME` est le même.
- `SSH_AUTH_SOCK` et les autres variables du §1.2 suivent le dernier client, pour les membres relancés ensuite.
- La détection des capacités passe par SSH (requêtes au terminal, DA1 en dernier). Le délai d'attente du client doit couvrir un aller-retour réseau, jusqu'à 1 s **[dev-saisie]**.
- Un lien lent ne prend pas de retard : les images sont sautées, jamais empilées (§6.2).
- Risque, le même qu'avec tmux : sous macOS, une équipe **lancée** depuis SSH hérite du contexte de sécurité de la session SSH, et Claude Code peut ne pas lire son jeton dans le trousseau. Sous Linux, `KillUserProcesses=yes` de systemd-logind tue le serveur à la déconnexion (rare par défaut) ; tmux conseille `systemd-run --user --scope`. À noter dans la documentation, pas à résoudre ici.
- À vérifier à l'étape 1 : `ssh localhost` (Session à distance activée sous macOS) et la CI Linux.

### 5.4 Un deuxième `attach` (spec §10, question 3) **[utilisateur]**

La plomberie (table des clients, un `Painter` et des `Caps` par client, des fils par client) sert les trois options ; seule la règle change. L'architecte porte la question à l'utilisateur.

**A. Il reprend la main.** Le premier client reçoit `Bye { Replaced }` et sort en disant « Détaché : l'équipe a été ouverte dans un autre terminal ».
- Pour : c'est simple (une taille, des capacités, un focus). C'est ce qu'on attend en rentrant chez soi sur une équipe restée ouverte au bureau, ou après un SSH coupé dont le client n'est pas encore mort : sans `ClientAliveInterval`, sshd peut ne s'en rendre compte qu'au bout de plusieurs heures.
- Contre : un `recruit` lancé par erreur dans un second terminal coupe le premier sans prévenir.
- Variante A' : le second demande d'abord « L'équipe est ouverte dans un autre terminal. La reprendre ici ? (O/n) ». Il prend la main sans demander hors d'un terminal interactif, ou avec `--force`.

**B. Il est refusé.** « L'équipe est déjà ouverte dans un autre terminal », et `recruit attach --force` pour la prendre.
- Pour : le premier n'est jamais coupé.
- Contre : après un SSH mort mais pas encore détecté, l'utilisateur est bloqué jusqu'à `--force`, qu'il doit connaître. En pratique, B devient A' avec une étape de plus.

**C. Les deux partagent l'écran.** Chacun voit l'équipe ; la taille est celle du plus petit, ou du dernier actif (comme `window-size latest` de tmux).
- Pour : regarder l'équipe de deux endroits, comme avec tmux.
- Contre : c'est hors du périmètre de la spec (§3). Le plus petit terminal rétrécit l'autre. Des capacités différentes (largeur des graphèmes, Nerd Font, protocole clavier) donnent deux dessins à tenir justes. Il faut décider si le focus et l'onglet sont communs ou propres à chaque client, et vers quel terminal vont OSC 52 et les notifications.

Coût technique : A et A' petits (A' ajoute une question côté client), B petit, C nettement plus grand (dessin et tests par client).

## 6. Fils, sans runtime async

### 6.1 Les fils

| Fil | Nombre | Bloqué dans | Rôle |
|---|---|---|---|
| boucle | 1 | `recv`, avec une échéance seulement si une image est due ou une mise à jour synchronisée en cours | seule à toucher l'état (panneaux, écran, clients) : route les entrées, applique les requêtes, compose et peint |
| lecteur et écrivain de PTY | 2 par panneau (dev-terminal) | `read`, file d'attente | nourrit le moteur, dit « du neuf » au plus une fois en attente (comme le prototype) |
| signaux | 1 | `sigwait` | §1.5 |
| écoute | 1, plus un par recréation du socket (§2.4) | `accept` | vérifie l'uid du pair, puis donne la connexion à un fil de client ou de commande |
| client : lecteur | 1 par client | `read` du socket | `Hello`, `Attach`, puis chaque message vers la boucle |
| client : écrivain | 1 par client | attente d'une image | écrit relais et images sur le socket ; dit à la boucle quand il est libre |
| commande | 1 par requête, court | `read`, puis la réponse de la boucle | §3.5 |
| travail | 1 (étape 2) | file de tâches | ce qui est lent hors de la boucle : `live.rs`, `zones.json`, `claude agents` |

Avec 10 panneaux et un client, cela fait environ 27 fils, presque tous bloqués, sans réveil au repos. Leur pile est surtout de la mémoire virtuelle : les pages ne sont prises qu'à l'usage. Un runtime async n'apporterait rien à cette échelle et ajouterait une dépendance lourde : je n'en propose pas. Si les mesures de l'étape 1 disent le contraire (latence, mémoire), je reviens avec les chiffres.

**Retirer un client** (`Bye` envoyé, client remplacé, erreur d'écriture) : la boucle appelle `shutdown(SHUT_RDWR)` sur son socket. Son écrivain, même bloqué dans `write`, en sort avec une erreur ; son lecteur lit la fin du socket ; chacun se termine, et le dernier ferme le descripteur. Jamais `close` depuis un autre fil : le numéro pourrait être réattribué pendant qu'un `write` l'utilise encore.

### 6.2 Client lent : images sautées, jamais empilées

- La boucle compose l'écran une fois par image due (60/s au plus), dans un `Canvas`, et **garde la dernière image composée**.
- Pour chaque client, une seule image est en vol. Si son écrivain est libre, la boucle peint cette image pour lui (`painter.frame`) et lui passe les octets ; sinon, elle marque seulement ce client « en retard ».
- Quand l'écrivain a fini, il le dit (`Msg::ClientReady`). Si le client est en retard, la boucle peint pour lui **l'écran du moment**, c'est-à-dire la dernière image composée, gardée. Le `Painter` compare avec la dernière image envoyée à ce client : les images intermédiaires sont sautées, et la différence envoyée est juste.
- `Painter::frame` prend aujourd'hui le `Canvas` par valeur : avec une image gardée et plusieurs clients, il faudrait une copie par client et par image. À l'étape 1, il prendrait `&Canvas` **[dev-rendu]**.
- Rien ne s'accumule : au plus une image en vol et un drapeau par client, quelle que soit la lenteur du lien (SSH lent, client suspendu par `kill -STOP`). Un client qui ne lit plus du tout reste attaché, sans coût, et ne retient jamais la boucle ni les autres clients.

**Relais** (OSC 52, titre, notifications, BEL) :

- La boucle vide les relais des moteurs (`Engine::relays`) chaque fois qu'un panneau a du neuf, **qu'il y ait un client ou non**. Les notifications et BEL sont versés dans l'état du serveur (le badge du membre et de son onglet, spec §5.6), avec ou sans client.
- Avec un client, ils vont dans sa file, écrits avant l'image suivante. La file est bornée par nature : on ne garde que **le dernier OSC 52 par sélection** (`c`, `p`, `s`…) et le dernier titre, comme le ferait un terminal qui les recevrait l'un après l'autre. Les notifications sont gardées dans une limite de 64, les plus anciennes tombant ; les BEL de suite n'en font qu'un.
- Sans client, OSC 52 et titre sont jetés : un presse-papiers rempli des heures plus tard, au retour, surprendrait.

## 7. Tests prévus (étape 1)

- **Unitaires** :
  - trames : aller-retour, longueur absurde refusée, charge lue par morceaux ;
  - formes JSON figées (`Hello`, `Welcome`, `Refused`, `Stop` et sa réponse), lues avec des champs et des variantes inconnus ; forme de référence des autres messages ; refus sur un `PROTO` différent, `Stop` accepté ;
  - chemin du socket : nom long, Unicode, coupure sur un caractère, hachage du dossier d'état, refus si le dossier est trop long ;
  - droits du dossier (lien, mauvais mode, mauvais propriétaire), dans un `tempfile` ;
  - socket orphelin : verrou libre, nettoyé ; verrou tenu avec `server.json`, SIGURG ; verrou tenu sans `server.json`, attente.
- **Intégration sans terminal** (spec §9) : un serveur lancé par le test, avec `RECRUIT_TMPDIR` et `XDG_CACHE_HOME` temporaires, des panneaux factices (`/bin/sh -c 'printf …'`), et un client de test qui relit les images dans un moteur et compare l'écran en texte. Cas couverts :
  - détacher et revenir avec une autre taille ;
  - deux `attach`, selon la règle choisie ;
  - un client qui ne lit plus : la boucle continue, les autres clients reçoivent leurs images ;
  - `kill -9` du serveur : le client le dit, les panneaux reçoivent SIGHUP, le lancement suivant le voit ;
  - `recruit stop` d'un serveur de `PROTO` différent ;
  - `Capture`, `Send` et `Key` par `recruit _ctl`.
- **Descripteurs** : un panneau factice qui liste les siens (`ls /dev/fd`) n'a que 0, 1 et 2 (plus celui de `ls` lui-même) : ni le verrou, ni le tube, ni le journal, ni un socket.
- **Signaux des panneaux** : `sh -c 'kill -HUP $$; echo survived'` dans un panneau ne doit rien afficher (SIGHUP n'est ni bloqué ni ignoré).
- **Deux serveurs de même session**, avec des `XDG_CACHE_HOME` différents et un même `RECRUIT_TMPDIR` : deux sockets distincts, et l'arrêt de l'un laisse l'autre joignable.
- **`server.json` périmé qui nomme un `pid` vivant étranger** (un `sleep` lancé par le test), verrou libre : aucun signal ne lui est envoyé (il vit encore après `recruit stop`, `list` et `attach`), et le fichier est retiré.
- **Aucun fil avant `_server`** : la liste des fils du processus (`/proc/self/task` sous Linux, `proc_pidinfo` sous macOS) n'en compte qu'un au moment du `fork`.
- **SIGHUP du terminal** : le client dans `script -q /dev/null`, dont on tue le processus ; le serveur reste, et `recruit attach` le rejoint.
- **Mesures** : aller-retour d'une requête, latence ajoutée à la frappe, mémoire et fils du serveur au repos, octets envoyés par seconde avec 10 panneaux en flux, et le même flux par un lien lent simulé (un client qui lit à débit limité).

## 8. Décisions de l'architecte (2026-10-09)

1. Comparaison par `PROTO` (§3.3) : accordée, inscrite au journal de la spec.
2. Trait `Backend` (§4.3) : les primitives jusqu'au retrait de tmux, `apply` à repenser à l'étape 4 ; l'architecte code le trait à l'étape 1.
3. Champ `backend` de `Snapshot` (§4.2), `tmux` par défaut à la lecture : à l'étape 1.
4. `Serialize`/`Deserialize` sur `Caps`, `canvas::Features`, `Plan` et ce qu'il contient, `PaneState`, `JournalSize` et `layout::Tab` : à l'étape 1 ; `Pane.role` devient un `enum`.
5. Événements (§3.4) : en attente du choix de dev-saisie entre crossterm et nos propres types.
6. Menu à l'étape 1 (§4.4) : penchant pour le panneau flottant, tranché à l'ouverture de l'étape 1.
7. Question 3 (§5.4) et reprise sans demander après un plantage (§1.4) : portées à l'utilisateur au compte rendu de fin d'étape 0.
8. Paniques (§1.4) : seule celle d'un lecteur de PTY est rattrapée (moteur remplacé) ; toute autre fait sortir le serveur après le journal.

Restent ouverts pour d'autres zones, relevés à la relecture :

- **[dev-terminal]** l'enfant d'un PTY remet les signaux à `SIG_DFL` et vide son masque (§1.5) ; l'esclave est ouvert avec `O_NOCTTY` (§1.1).
- **[dev-rendu]** `Painter::frame(&Canvas)` (§6.2) ; avec l'architecte, une fonction sœur de `layout::tmux_layout` qui rend des rectangles (§4.1).
- **[architecte]** déplacer `tmux::ALT` hors de `tmux.rs` (§4.1).
