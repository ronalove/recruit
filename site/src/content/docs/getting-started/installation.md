---
title: Installation
description: Install recruit with Homebrew, from a release archive or from source, and check what it needs.
sidebar:
  order: 1
---

recruit is a single binary, with nothing else to install. It needs [Claude Code](https://claude.com/claude-code).

## Homebrew

On macOS and Linux:

```sh
brew install ronalove/tap/recruit
```

Homebrew downloads a prebuilt binary, for macOS (Apple Silicon, Intel) or Linux (x86_64, arm64, static).

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

Extract `recruit` and put it somewhere on your `PATH`.

## From source

With Rust 1.99 or newer, curl and tar. recruit's terminal engine, [libghostty-vt](https://github.com/ghostty-org/ghostty), is built with Zig 0.16: `scripts/ghostty.sh` fetches Zig and Ghostty's sources once, checks them, and builds it.

```sh
git clone https://github.com/ronalove/recruit
cd recruit
scripts/ghostty.sh
cargo install --path .
```

## What recruit needs

- **Claude Code**, installed and logged in: recruit runs `claude` from your `PATH`, or the command given by [`[claude] command`](/recruit/reference/configuration/). With Claude Code 2.1.287 or newer, each member also loads [recruit's mod](/recruit/guides/claude-code/), which adds the model, the context and the usage to the dashboard, and the `/recruit` command. Older versions work without these.
- **On macOS**, a terminal that sends Option as Alt, for recruit's shortcuts: see [Option on macOS](/recruit/guides/screen/#option-on-macos).
- **No special font.** In Ghostty, kitty and WezTerm, which carry the Nerd Font symbols themselves, the dashboard and the menu draw their icons with them; elsewhere, with plain Unicode signs.

Check the installation:

```sh
recruit --version
```

Next: [your first team in two minutes](/recruit/getting-started/quick-start/).
