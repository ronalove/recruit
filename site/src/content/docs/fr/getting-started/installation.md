---
title: Installation
description: Installer recruit avec Homebrew, depuis une archive de release ou depuis les sources, et ce qu'il lui faut.
sidebar:
  order: 1
---

recruit est un seul binaire, sans rien d'autre à installer. Il a besoin de [Claude Code](https://claude.com/claude-code).

## Homebrew

Sous macOS et Linux :

```sh
brew install ronalove/tap/recruit
```

Homebrew télécharge un binaire précompilé, pour macOS (Apple Silicon, Intel) ou Linux (x86_64, arm64, statique).

Pour mettre à jour :

```sh
brew update && brew upgrade recruit
```

## Archives des releases

Chaque [release](https://github.com/ronalove/recruit/releases) contient les mêmes binaires en archives, une par plateforme, avec un fichier `SHA256SUMS` :

| Archive | Plateforme |
|---|---|
| `recruit-<version>-aarch64-apple-darwin.tar.gz` | macOS, Apple Silicon |
| `recruit-<version>-x86_64-apple-darwin.tar.gz` | macOS, Intel |
| `recruit-<version>-x86_64-unknown-linux-musl.tar.gz` | Linux x86_64, statique |
| `recruit-<version>-aarch64-unknown-linux-musl.tar.gz` | Linux arm64, statique |

Extrais `recruit` et place-le dans un dossier de ton `PATH`.

## Depuis les sources

Avec Rust 1.99 ou plus récent, curl et tar. Le moteur de terminal de recruit, [libghostty-vt](https://github.com/ghostty-org/ghostty), se construit avec Zig 0.16 : `scripts/ghostty.sh` télécharge une fois Zig et les sources de Ghostty, les vérifie, et le construit.

```sh
git clone https://github.com/ronalove/recruit
cd recruit
scripts/ghostty.sh
cargo install --path .
```

## Ce qu'il faut à recruit

- **Claude Code**, installé et connecté : recruit lance `claude` depuis ton `PATH`, ou la commande donnée par [`[claude] command`](/recruit/fr/reference/configuration/). Avec Claude Code 2.1.287 ou plus récent, chaque membre charge aussi [le mod de recruit](/recruit/fr/guides/claude-code/), qui ajoute au tableau de bord le modèle, le contexte et l'usage, et la commande `/recruit`. Les versions plus anciennes marchent sans tout cela.
- **Sous macOS**, un terminal qui envoie Option comme Alt, pour les raccourcis de recruit : voir [Option sous macOS](/recruit/fr/guides/screen/#option-sous-macos).
- **Aucune police particulière.** Dans Ghostty, kitty et WezTerm, qui embarquent les symboles Nerd Font, le tableau de bord et le menu dessinent leurs icônes avec ; ailleurs, avec de simples signes Unicode.

Pour vérifier l'installation :

```sh
recruit --version
```

Ensuite : [ta première équipe en deux minutes](/recruit/fr/getting-started/quick-start/).
