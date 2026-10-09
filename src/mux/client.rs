// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The thin client (specs/multiplexeur-serveur.md §5): the real terminal set up (`input::Terminal`), its events sent
//! to the team's server, what the server sends written back. All the logic is the server's.
//!
//! Threads: this one reads the socket and writes on the terminal; the terminal's reader sends the events; one waits
//! for signals: SIGTERM detaches, SIGTSTP gives the terminal back and stops, SIGCONT sets it up again. The terminal is
//! given back as it was when the client leaves, panic included.
//!
//! Owner: dev-serveur.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::engine::MouseTracking;
use super::input::{Event, Terminal};
use super::proto::{self, Bye, ClientMsg, Frame, Hello, Kind, Refusal, Reply, Request, ServerMsg, Welcome};
use super::socket::{self, Found};
use crate::t;

/// The size taken when the terminal does not say its own.
const DEFAULT_SIZE: (u16, u16) = (80, 24);

/// Reaches the server of the team whose state is in `state`, and checks it speaks our protocol. `Welcome` comes
/// whatever the protocols: `recruit list` reads it.
pub(crate) fn hello(state: &Path, kind: Kind) -> Result<Option<(UnixStream, Welcome)>> {
    let Found::Running(mut stream) = socket::connect(state)? else { return Ok(None) };
    // A server stopped (`kill -STOP`) or stuck must not hold `recruit list`.
    stream.set_read_timeout(Some(WELCOME)).context("socket")?;
    proto::send(&mut stream, &Hello::new(kind)).context("socket")?;
    let welcome: Welcome = proto::recv(&mut stream)
        .map_err(|error| match error.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => anyhow::anyhow!(t!(
                "le serveur de l'équipe ne répond pas ({})",
                "the team's server does not answer ({})",
                state.display()
            )),
            _ => anyhow::Error::new(error).context("socket"),
        })?
        .context(t!("le serveur de l'équipe a fermé la connexion", "the team's server closed the connection"))?;
    // A server of another protocol refuses an attach and closes at once: on a socket closed by the other side, macOS
    // refuses socket options (EINVAL). Its welcome says why, which the caller tells: the error here would hide it.
    let _ = stream.set_read_timeout(None);
    Ok(Some((stream, welcome)))
}

/// How long the server has to welcome a connection.
const WELCOME: Duration = Duration::from_secs(2);

/// How long a command waits for its answer, unless it says otherwise.
pub(crate) const ANSWER: Duration = Duration::from_secs(5);

/// Sends `request` to the server of the team whose state is in `state`, and reads its answer within `wait`. `None`
/// when no server runs. A server of another protocol takes only `Stop`.
pub(crate) fn request(state: &Path, request: &Request, wait: Duration) -> Result<Option<Reply>> {
    let Some((mut stream, welcome)) = hello(state, Kind::Command)? else { return Ok(None) };
    if welcome.proto != proto::PROTO && *request != Request::Stop {
        return Err(other_protocol(&welcome));
    }
    // The protocols compared: a failure here is the socket's own, and without the delay a stuck server would hold
    // the answer's read for good.
    stream.set_read_timeout(Some(wait)).context("socket")?;
    proto::send(&mut stream, request).context("socket")?;
    let reply = proto::recv::<Reply>(&mut stream).map_err(|error| match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => anyhow::anyhow!(t!(
            "le serveur de l'équipe ne répond pas ({})",
            "the team's server does not answer ({})",
            state.display()
        )),
        _ => anyhow::Error::new(error).context("socket"),
    })?;
    Ok(Some(
        reply.context(t!("le serveur de l'équipe a fermé la connexion", "the team's server closed the connection"))?,
    ))
}

/// What to say when the server's protocol is not ours.
pub(crate) fn other_protocol(welcome: &Welcome) -> anyhow::Error {
    let team = if welcome.team.is_empty() { &welcome.session } else { &welcome.team };
    anyhow::anyhow!(t!(
        "l'équipe « {0} » tourne avec recruit {1}, dont le protocole diffère de celui de ce recruit ({2}). Pour la reprendre avec celui-ci : recruit stop {0}, puis recruit {0} --resume (chaque membre reprend sa conversation).",
        "team \"{0}\" runs with recruit {1}, whose protocol differs from this recruit's ({2}). To take it over with this one: recruit stop {0}, then recruit {0} --resume (each member resumes its conversation).",
        team,
        welcome.version,
        proto::VERSION
    ))
}

/// Attaches the terminal to the team whose state is in `state`, until the client leaves: detached, replaced by
/// another, or the team stopped.
pub(crate) fn attach(state: &Path) -> Result<()> {
    // From inside one of the team's own panes, the team would show in itself.
    if std::env::var_os("RECRUIT_STATE").is_some_and(|inside| Path::new(&inside) == state) {
        bail!(t!(
            "tu es déjà dans cette équipe : détache-toi d'abord ({}q)",
            "you are already inside this team: detach first ({}q)",
            crate::tmux::ALT
        ));
    }
    let Some((mut stream, welcome)) = hello(state, Kind::Attach)? else {
        bail!(t!("l'équipe ne tourne pas", "the team is not running"));
    };
    if welcome.proto != proto::PROTO {
        return Err(other_protocol(&welcome));
    }
    // The protocols compared, the reset `hello` may have let go is made again: left at its delay, the first silence
    // of a healthy server would read as its end.
    stream.set_read_timeout(None).context("socket")?;
    let team = if welcome.team.is_empty() { welcome.session.clone() } else { welcome.team.clone() };
    // Before any thread of the terminal's: they inherit the mask, and the signals can reach none of them, which would
    // end or stop the client without giving the terminal back.
    let signals = Signals::block();
    let (mut terminal, caps) = Terminal::open().context(t!("terminal", "terminal"))?;
    let (cols, rows) = terminal.size().unwrap_or(DEFAULT_SIZE);
    let env = proto::UPDATE_ENV.iter().filter_map(|name| Some((name.to_string(), std::env::var(name).ok()?))).collect();
    let term = std::env::var("TERM").unwrap_or_default();
    proto::send(&mut stream, &ClientMsg::Attach { caps, cols, rows, term, env }).context("socket")?;

    // One writer on the socket at a time: the terminal's reader, and the signal's thread.
    let writer = Arc::new(Mutex::new(stream.try_clone().context("socket")?));
    let events = Arc::clone(&writer);
    terminal
        .read_events(move |event| {
            let closed = matches!(event, Event::Closed(_));
            let mut stream = events.lock().unwrap_or_else(PoisonError::into_inner);
            if closed {
                // The terminal is gone: the client leaves, the team stays.
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return false;
            }
            proto::send(&mut *stream, &ClientMsg::Input(event)).is_ok()
        })
        .context(t!("terminal", "terminal"))?;
    let terminal = Arc::new(Mutex::new(terminal));
    signals.wait(Arc::clone(&writer), Arc::downgrade(&terminal));

    let end = serve(&mut stream, &terminal);
    give_back(terminal);
    drop(signals);
    match end {
        Ok(End::Bye(Bye::Detached)) => {
            println!(
                "{}",
                t!(
                    "Détaché de l'équipe « {0} ». Pour la rejoindre : recruit attach {0}",
                    "Detached from team \"{0}\". To join it: recruit attach {0}",
                    team
                )
            );
            Ok(())
        }
        Ok(End::Bye(Bye::Replaced)) => {
            println!(
                "{}",
                t!(
                    "Détaché : l'équipe « {} » a été ouverte dans un autre terminal.",
                    "Detached: team \"{}\" was opened in another terminal.",
                    team
                )
            );
            Ok(())
        }
        Ok(End::Bye(Bye::Stopped)) => {
            println!("{}", t!("Équipe « {} » arrêtée.", "Team \"{}\" stopped.", team));
            Ok(())
        }
        Ok(End::Bye(Bye::Ended)) => {
            println!(
                "{}",
                t!(
                    "L'équipe « {} » s'est arrêtée : son dernier panneau s'est fermé.",
                    "Team \"{}\" stopped: its last pane closed.",
                    team
                )
            );
            Ok(())
        }
        Ok(End::Bye(Bye::Error(error))) => bail!(error),
        Ok(End::Bye(Bye::Other)) | Ok(End::Closed) => Ok(()),
        Ok(End::Refused(Refusal::Proto)) => Err(other_protocol(&welcome)),
        Ok(End::Refused(Refusal::Busy)) => bail!(t!(
            "l'équipe « {} » est déjà ouverte dans un autre terminal",
            "team \"{}\" is already open in another terminal",
            team
        )),
        Ok(End::Refused(Refusal::Other)) => bail!(t!(
            "le serveur de l'équipe « {} » a refusé ce client (recruit {} ; ce recruit-ci : {})",
            "the server of team \"{}\" refused this client (recruit {}; this one: {})",
            team,
            welcome.version,
            proto::VERSION
        )),
        Ok(End::Lost) => bail!(t!(
            "le serveur de l'équipe s'est arrêté sans prévenir (voir {})",
            "the team's server stopped without warning (see {})",
            state.join("server.log").display()
        )),
        Err(error) => Err(error),
    }
}

/// Gives the terminal back, here, before anything is written after it: the signals' thread only borrows it while it
/// handles a signal, and is waited for a moment if it does (its drop would otherwise come after the message).
fn give_back(terminal: Arc<Mutex<Terminal>>) {
    let until = std::time::Instant::now() + Duration::from_millis(500);
    let mut terminal = terminal;
    loop {
        match Arc::try_unwrap(terminal) {
            Ok(alone) => return drop(alone),
            Err(shared) if std::time::Instant::now() < until => {
                terminal = shared;
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(shared) => return drop(shared),
        }
    }
}

/// How the client's conversation with the server ended.
enum End {
    Bye(Bye),
    Refused(Refusal),
    /// The client closed it: its terminal is gone.
    Closed,
    /// The server went without a word.
    Lost,
}

/// What the server sends, to the terminal, until it says goodbye.
fn serve(stream: &mut UnixStream, terminal: &Mutex<Terminal>) -> Result<End> {
    loop {
        let frame = match proto::read_frame(stream) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(End::Lost),
            // Shut by the terminal's reader: the terminal is gone.
            Err(error) if error.kind() == io::ErrorKind::NotConnected => return Ok(End::Closed),
            Err(_) => return Ok(End::Lost),
        };
        match frame {
            Frame::Output(bytes) => {
                let mut terminal = terminal.lock().unwrap_or_else(PoisonError::into_inner);
                if terminal.write(&bytes).is_err() {
                    return Ok(End::Closed);
                }
            }
            Frame::Json(json) => match serde_json::from_slice::<ServerMsg>(&json) {
                Ok(ServerMsg::Attached { .. }) => {}
                Ok(ServerMsg::Motion(motion)) => {
                    let mut terminal = terminal.lock().unwrap_or_else(PoisonError::into_inner);
                    let _ = terminal.set_mouse(motion.then_some(MouseTracking::Motion));
                }
                Ok(ServerMsg::Bye(bye)) => return Ok(End::Bye(bye)),
                Ok(ServerMsg::Refused(refusal)) => return Ok(End::Refused(refusal)),
                // A message of a newer server, same protocol: nothing to do with it.
                Err(_) => {}
            },
        }
    }
}

/// The signals the client takes, blocked while it has the terminal: a thread waits for them. SIGTERM detaches,
/// rather than leaving the terminal as the client had set it; SIGTSTP (from outside: in raw mode, Ctrl-Z is a key)
/// gives the terminal back, then stops; SIGCONT sets it up again and asks for a whole frame. The mask is given back as
/// it was when dropped.
struct Signals {
    set: libc::sigset_t,
    before: libc::sigset_t,
}

impl Signals {
    fn block() -> Signals {
        // SAFETY: sets filled by sigemptyset and sigaddset, and this thread's mask, kept to be given back.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            let mut before: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signal in [libc::SIGTERM, libc::SIGTSTP, libc::SIGCONT] {
                libc::sigaddset(&mut set, signal);
            }
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut before);
            Signals { set, before }
        }
    }

    fn wait(&self, writer: Arc<Mutex<UnixStream>>, terminal: Weak<Mutex<Terminal>>) {
        let set = self.set;
        let _ = std::thread::Builder::new().name("signals".into()).spawn(move || {
            let send = |message: &ClientMsg| {
                let mut stream = writer.lock().unwrap_or_else(PoisonError::into_inner);
                proto::send(&mut *stream, message).is_ok()
            };
            loop {
                let mut signal = 0;
                // SAFETY: sigwait on signals blocked in every thread of the client.
                if unsafe { libc::sigwait(&set, &mut signal) } != 0 {
                    continue;
                }
                match signal {
                    libc::SIGTERM => {
                        send(&ClientMsg::Detach);
                        return;
                    }
                    libc::SIGTSTP => {
                        let Some(terminal) = terminal.upgrade() else { return };
                        // Held while stopped: no frame is written on the terminal given back.
                        let mut terminal = terminal.lock().unwrap_or_else(PoisonError::into_inner);
                        let _ = terminal.suspend();
                        // SAFETY: stops this process, as SIGTSTP would have; it goes on at SIGCONT.
                        unsafe { libc::kill(libc::getpid(), libc::SIGSTOP) };
                        let _ = terminal.resume();
                        drop(terminal);
                        if !send(&ClientMsg::Redraw) {
                            return;
                        }
                    }
                    // Stopped from outside (SIGSTOP), or the SIGCONT that ended the stop above: set up again if it was
                    // given back (resume does nothing otherwise), the screen drawn anew.
                    _ => {
                        let Some(terminal) = terminal.upgrade() else { return };
                        let _ = terminal.lock().unwrap_or_else(PoisonError::into_inner).resume();
                        if !send(&ClientMsg::Redraw) {
                            return;
                        }
                    }
                }
            }
        });
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        // SAFETY: this thread's mask, as it was.
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &self.before, std::ptr::null_mut()) };
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;
    use crate::mux::socket::{self, Info};

    /// A server of another protocol: it welcomes, refuses, and closes at once, as `server::connection` does.
    fn refusing_server(state: &Path) -> (std::fs::File, std::thread::JoinHandle<()>) {
        let lock = socket::try_lock(state).unwrap().expect("free");
        let path = state.join("s.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let info = Info { pid: std::process::id(), socket: path, proto: 9, version: "9.9.9".into(), started: 0 };
        socket::write_info(state, &info).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _: Option<Hello> = proto::recv(&mut stream).unwrap();
            let welcome = Welcome { proto: 9, version: "9.9.9".into(), team: "mux".into(), ..Welcome::default() };
            proto::send(&mut stream, &welcome).unwrap();
            proto::send(&mut stream, &ServerMsg::Refused(Refusal::Proto)).unwrap();
        });
        (lock, server)
    }

    #[test]
    fn another_protocol_says_why() {
        let state = tempfile::tempdir().unwrap();
        let (_lock, server) = refusing_server(state.path());
        // The terminal is never touched: the protocols are compared first.
        let error = attach(state.path()).unwrap_err().to_string();
        server.join().unwrap();
        assert!(error.contains("9.9.9") && error.contains("mux"), "{error}");
        assert!(!error.contains("os error"), "{error}");
    }
}
