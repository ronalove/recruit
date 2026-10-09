// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! `recruit _ctl <state> …` (hidden): a team's server observed and driven without a terminal, for the test bench,
//! the captures and the step-by-step checks (specs/multiplexeur-serveur.md §3.5). What `tmux capture-pane`,
//! `send-keys` and `run-shell` did.
//!
//! Exit codes: 0 done; 2 the team is not running; 3 no such pane; 1 anything else, its message on stderr. `--json`
//! output is never translated.
//!
//! Owner: dev-serveur.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::client;
use super::input::{Event, Key, KeyCode, Mods};
use super::proto::{self, Captured, ClientInfo, Kind, PaneInfo, Reply, ReplyError, Request, Stats};
use super::socket;
use crate::t;

#[derive(clap::Subcommand, Debug, Clone)]
pub(crate) enum Action {
    /// The server's `server.json`: pid, socket, protocol, version
    Where,
    /// Opens a pane running a command, for tests (refused if the member has one)
    Spawn {
        member: String,
        /// The tab it goes in, made if missing (default: the current one)
        #[arg(long)]
        tab: Option<String>,
        /// Where it runs (default: here)
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// KEY=VALUE set in its environment
        #[arg(long = "env", value_parser = key_value)]
        env: Vec<(String, String)>,
        /// The command and its arguments, after `--`
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// A pane's text (its member or its id), or the whole screen as composed; a line per row
    Capture {
        #[arg(long)]
        pane: Option<String>,
        /// Also the last N lines of the pane's history, above its screen
        #[arg(long, default_value_t = 0)]
        history: usize,
        /// With the cursor and the pane's modes
        #[arg(long)]
        json: bool,
    },
    /// For each pane, its member, pid, size, history and bytes read; the frames and bytes sent by the server
    Stats {
        #[arg(long)]
        json: bool,
    },
    /// Bytes written as they are to a pane's program: \e, \r, \n, \t, \\, \0 and \xNN understood
    Send {
        #[arg(long)]
        pane: String,
        bytes: String,
    },
    /// A key by its name (shift+enter, alt+b, escape, ctrl+c, f5, a…), encoded for the pane as its modes ask;
    /// without --pane, routed as the client's keys are, the server's shortcuts first
    Key {
        #[arg(long)]
        pane: Option<String>,
        key: String,
    },
    /// The panes, tab by tab
    Panes {
        #[arg(long)]
        json: bool,
    },
    /// The client attached, and the server's pid
    Clients {
        #[arg(long)]
        json: bool,
    },
    /// Closes a pane, its program stopped
    Kill {
        #[arg(long)]
        pane: String,
    },
    /// Stops the team: returns once its panes have ended
    Stop,
    /// The real client, on this terminal
    Attach,
}

fn key_value(text: &str) -> Result<(String, String), String> {
    text.split_once('=')
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .ok_or_else(|| format!("KEY=VALUE expected: {text}"))
}

/// Why `_ctl` failed, for its exit code.
enum Failure {
    NotRunning,
    NoPane(String),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Failure::Other(error)
    }
}

/// Runs `action` on the team whose state is in `state`; exits with its code on failure.
pub(crate) fn run(state: &Path, action: Action) -> Result<()> {
    let state = state.canonicalize().unwrap_or_else(|_| state.to_path_buf());
    match act(&state, action) {
        Ok(()) => Ok(()),
        Err(Failure::NotRunning) => {
            eprintln!("recruit: {}", t!("l'équipe ne tourne pas", "the team is not running"));
            std::process::exit(2)
        }
        Err(Failure::NoPane(pane)) => {
            eprintln!("recruit: {}", t!("panneau inconnu : {}", "unknown pane: {}", pane));
            std::process::exit(3)
        }
        Err(Failure::Other(error)) => Err(error),
    }
}

fn act(state: &Path, action: Action) -> Result<(), Failure> {
    match action {
        Action::Where => {
            let Some(info) = socket::read_info(state) else { return Err(Failure::NotRunning) };
            // Read, then checked: a `server.json` left by a dead server is not where a server is.
            if client::hello(state, Kind::Command)?.is_none() {
                return Err(Failure::NotRunning);
            }
            println!("{}", serde_json::to_string(&info).context("json")?);
        }
        Action::Spawn { member, tab, cwd, env, command } => {
            let cwd = match cwd {
                Some(cwd) => cwd,
                None => std::env::current_dir().context("current directory")?,
            };
            let cwd = cwd.canonicalize().with_context(|| cwd.display().to_string())?;
            let request = Request::Spawn { member, argv: command, env, cwd: cwd.to_string_lossy().into_owned(), tab };
            let id: String = value(request_to(state, request)?)?;
            println!("{id}");
        }
        Action::Capture { pane, history, json } => {
            if let Some(pane) = &pane {
                known(state, pane)?;
            }
            let captured: Captured = value(request_to(state, Request::Capture { pane, styles: false, history })?)?;
            if json {
                println!("{}", serde_json::to_string(&captured).context("json")?);
            } else {
                for line in captured.history.iter().chain(&captured.lines) {
                    println!("{line}");
                }
            }
        }
        Action::Send { pane, bytes } => {
            known(state, &pane)?;
            let bytes = unescape(&bytes)?;
            value::<()>(request_to(state, Request::Send { pane, bytes })?)?;
        }
        Action::Key { pane, key } => {
            if let Some(pane) = &pane {
                known(state, pane)?;
            }
            let event = Event::Key(parse_key(&key)?);
            value::<()>(request_to(state, Request::Key { pane, event })?)?;
        }
        Action::Stats { json } => {
            let stats: Stats = value(request_to(state, Request::Stats)?)?;
            if json {
                println!("{}", serde_json::to_string(&stats).context("json")?);
            } else {
                println!("pid {}\tframes {}\twritten {}", stats.pid, stats.frames, stats.written);
                for pane in &stats.panes {
                    println!(
                        "{}\t{}\t{}\t{}x{}\thistory {}\tread {}",
                        pane.id, pane.member, pane.pid, pane.cols, pane.rows, pane.history, pane.read
                    );
                }
            }
        }
        Action::Panes { json } => {
            let panes: Vec<PaneInfo> = value(request_to(state, Request::Panes)?)?;
            if json {
                println!("{}", serde_json::to_string(&panes).context("json")?);
            } else {
                for pane in &panes {
                    println!("{}\t{}\t{}\t{}x{}\t{}", pane.id, pane.tab, pane.member, pane.cols, pane.rows, pane.pid);
                }
            }
        }
        Action::Clients { json } => {
            let Some((mut stream, welcome)) = client::hello(state, Kind::Command)? else {
                return Err(Failure::NotRunning);
            };
            if welcome.proto != proto::PROTO {
                return Err(client::other_protocol(&welcome).into());
            }
            proto::send(&mut stream, &Request::Clients).context("socket")?;
            let reply: Reply = proto::recv(&mut stream).context("socket")?.context("socket")?;
            let clients: Vec<ClientInfo> = value(reply)?;
            if json {
                let out = serde_json::json!({ "pid": welcome.pid, "clients": clients });
                println!("{out}");
            } else {
                for client in &clients {
                    let name = client.name.as_deref().unwrap_or("");
                    println!("{}\t{}x{}\t{}\t{name}", client.id, client.cols, client.rows, client.term);
                }
            }
        }
        Action::Kill { pane } => {
            known(state, &pane)?;
            value::<()>(request_to(state, Request::KillPane { id: pane })?)?;
        }
        Action::Stop => {
            value::<()>(request_to(state, Request::Stop)?)?;
        }
        Action::Attach => client::attach(state)?,
    }
    Ok(())
}

/// Sends `request` to the team's server, and reads its answer.
fn request_to(state: &Path, request: Request) -> Result<Reply, Failure> {
    let wait = if request == Request::Stop { STOP_WAIT } else { client::ANSWER };
    client::request(state, &request, wait)?.ok_or(Failure::NotRunning)
}

/// How long `stop` waits: the panes have a few seconds to end.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// An answer's value, or what went wrong.
fn value<T: serde::de::DeserializeOwned>(reply: Reply) -> Result<T, Failure> {
    reply.value().map_err(|error| {
        Failure::Other(match error {
            ReplyError::Failed(message) => anyhow::anyhow!(message),
            ReplyError::Refused(refusal) => anyhow::anyhow!("refused: {refusal:?}"),
            ReplyError::Invalid(why) => anyhow::anyhow!("answer: {why}"),
        })
    })
}

/// Fails with `NoPane` unless the team has `pane`, by its id or its member's name.
fn known(state: &Path, pane: &str) -> Result<(), Failure> {
    let panes: Vec<PaneInfo> = value(request_to(state, Request::Panes)?)?;
    if panes.iter().any(|p| p.id == pane || p.member == pane) { Ok(()) } else { Err(Failure::NoPane(pane.to_string())) }
}

/// `\e`, `\r`, `\n`, `\t`, `\\`, `\0` and `\xNN` in `text`, as bytes.
fn unescape(text: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('e') => bytes.push(0x1b),
            Some('r') => bytes.push(b'\r'),
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some('0') => bytes.push(0),
            Some('\\') => bytes.push(b'\\'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                let byte = u8::from_str_radix(&hex, 16).ok().filter(|_| hex.len() == 2);
                bytes.push(byte.with_context(|| format!("\\x{hex}: two hex digits expected"))?);
            }
            other => bail!("\\{}: unknown escape", other.map(String::from).unwrap_or_default()),
        }
    }
    Ok(bytes)
}

/// A key by its name: modifiers (`shift`, `alt` or `option`, `ctrl` or `control`, `super` or `cmd`) joined by `+`
/// to a key (`enter`, `tab`, `backspace`, `escape` or `esc`, `up`…, `home`, `end`, `pageup`, `pagedown`, `insert`,
/// `delete`, `f1` to `f35`, `space`, or one character).
pub(crate) fn parse_key(name: &str) -> Result<Key> {
    let parts: Vec<&str> = name.split('+').collect();
    let (code, modifiers) = match parts.split_last() {
        // `ctrl++`: the last part is empty, the key is `+`.
        Some((last, rest)) if last.is_empty() && !rest.is_empty() => ("+", &rest[..rest.len() - 1]),
        Some((last, rest)) => (*last, rest),
        None => bail!("empty key"),
    };
    let mut mods = Mods::NONE;
    for modifier in modifiers {
        mods = mods
            | match modifier.to_lowercase().as_str() {
                "shift" => Mods::SHIFT,
                "alt" | "option" | "opt" | "meta" => Mods::ALT,
                "ctrl" | "control" => Mods::CTRL,
                "super" | "cmd" | "command" => Mods::SUPER,
                other => bail!("unknown modifier: {other}"),
            };
    }
    let lower = code.to_lowercase();
    let code = match lower.as_str() {
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "escape" | "esc" => KeyCode::Esc,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "insert" => KeyCode::Insert,
        "delete" | "del" => KeyCode::Delete,
        "space" => KeyCode::Char(' '),
        f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=35).contains(&n)) => {
            KeyCode::F(f[1..].parse().unwrap_or(1))
        }
        _ => {
            let mut chars = code.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => bail!("unknown key: {code}"),
            }
        }
    };
    Ok(Key::new(code, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_by_their_names() {
        assert_eq!(parse_key("shift+enter").unwrap(), Key::new(KeyCode::Enter, Mods::SHIFT));
        assert_eq!(parse_key("alt+b").unwrap(), Key::new(KeyCode::Char('b'), Mods::ALT));
        assert_eq!(parse_key("Ctrl+C").unwrap(), Key::new(KeyCode::Char('C'), Mods::CTRL));
        assert_eq!(parse_key("escape").unwrap(), Key::new(KeyCode::Esc, Mods::NONE));
        assert_eq!(parse_key("shift+tab").unwrap(), Key::new(KeyCode::Tab, Mods::SHIFT));
        assert_eq!(parse_key("f12").unwrap(), Key::new(KeyCode::F(12), Mods::NONE));
        assert_eq!(parse_key("ctrl++").unwrap(), Key::new(KeyCode::Char('+'), Mods::CTRL));
        assert_eq!(parse_key("é").unwrap(), Key::new(KeyCode::Char('é'), Mods::NONE));
        assert!(parse_key("hyper+a").is_err());
        assert!(parse_key("f36").is_err());
        assert!(parse_key("nope").is_err());
    }

    #[test]
    fn escapes_become_bytes() {
        assert_eq!(unescape(r"a\r\e[A\x1b\\\0é").unwrap(), b"a\r\x1b[A\x1b\\\0\xc3\xa9");
        assert!(unescape(r"\x1").is_err());
        assert!(unescape(r"\q").is_err());
    }
}
