---
title: Installation
description: Install recruit with Homebrew, from a release archive or from source, and check what it needs.
sidebar:
  order: 1
---

recruit is a single binary. It needs [Claude Code](https://claude.com/claude-code) and tmux 3.5 or newer.

## Homebrew

On macOS and Linux:

```sh
brew install ronalove/tap/recruit
```

Homebrew downloads a prebuilt binary, for macOS (Apple Silicon, Intel) or Linux (x86_64, arm64, static), and installs tmux along with it.

To update:

```sh
brew update && brew upgrade recruit
```

## Release archives

Each [release](https://github.com/ronalove/recruit/releases) holds the same binaries as archives, one per platform, with a `SHA256SUMS` file:

| Archive | Platform |
|---|---|
| `recruit-<version>-aarch64-apple-darwin.tar.gz` | macOS, Apple Silicon |
| `recruit-<version>-x86_64-apple-darwin.tar.gz` | macOS, Intel |
| `recruit-<version>-x86_64-unknown-linux-musl.tar.gz` | Linux x86_64, static |
| `recruit-<version>-aarch64-unknown-linux-musl.tar.gz` | Linux arm64, static |

Extract `recruit` and put it somewhere on your `PATH`. Install tmux yourself in that case.

## From source

With Rust 1.88 or newer:

```sh
cargo install --git https://github.com/ronalove/recruit
```

## What recruit needs

- **Claude Code**, installed and logged in: recruit runs `claude` from your `PATH`, or the command given by [`[claude] command`](/recruit/reference/configuration/). With Claude Code 2.1.287 or newer, each member also loads [recruit's mod](/recruit/guides/claude-code/), which adds the model, the context and the usage to the dashboard, and the `/recruit` command. Older versions work without these.
- **tmux 3.5 or newer.** recruit checks it at launch and says so when it is older.
- **On macOS**, a terminal that sends Option as Alt, for recruit's shortcuts: see [Option on macOS](/recruit/guides/tmux/#option-on-macos).
- **No special font.** In Ghostty, kitty and WezTerm, which carry the Nerd Font symbols themselves, the dashboard and the menu draw their icons with them; elsewhere, with plain Unicode signs.

Check the installation:

```sh
recruit --version
```

Next: [your first team in two minutes](/recruit/getting-started/quick-start/).
