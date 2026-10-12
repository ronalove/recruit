// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The server's clients: the socket's side (accepting, the connection of each client, its writer) and the client that
//! has the team.

use std::collections::VecDeque;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use super::lifecycle::{log, now};
use super::{BYE_TIME, Msg, Server};
use crate::mux::Caps;
use crate::mux::engine::Relay;
use crate::mux::proto::{self, Bye, ClientMsg, Hello, Kind, Refusal, Reply, Request, ServerMsg, Welcome};
use crate::mux::socket;
use crate::t;
use crate::ui::canvas::Painter;
use crate::ui::look;

/// How long a connection may take to say what it wants.
const HELLO_TIME: Duration = Duration::from_secs(5);

/// Notifications kept for a client that does not read; the oldest go first.
pub(super) const NOTIFICATIONS: usize = 64;

/// A client's connection, once it said what it is.
pub(super) struct Attaching {
    pub(super) id: String,
    pub(super) stream: UnixStream,
    pub(super) out: Arc<Outbox>,
    pub(super) caps: Caps,
    pub(super) size: (u16, u16),
    pub(super) term: String,
    pub(super) env: Vec<(String, String)>,
}

/// What goes to a client, for its writer.
pub(super) enum Out {
    Json(Vec<u8>),
    Output(Vec<u8>),
    /// The last: the connection is shut once what comes before is written.
    Close,
}

#[derive(Default)]
pub(super) struct Outbox {
    pub(super) queue: Mutex<VecDeque<Out>>,
    pub(super) ready: Condvar,
    /// Set by the writer when it has left: all written, or the connection lost.
    pub(super) closed: Mutex<bool>,
    pub(super) left: Condvar,
}

impl Outbox {
    /// Waits, `time` at most, for the writer to leave. Whether it did.
    pub(super) fn wait_closed(&self, time: Duration) -> bool {
        let closed = self.closed.lock().unwrap_or_else(PoisonError::into_inner);
        let (closed, _) =
            self.left.wait_timeout_while(closed, time, |closed| !*closed).unwrap_or_else(PoisonError::into_inner);
        *closed
    }

    fn set_closed(&self) {
        *self.closed.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.left.notify_all();
    }

    pub(super) fn push(&self, out: Out) {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).push_back(out);
        self.ready.notify_one();
    }

    pub(super) fn message(&self, message: &ServerMsg) {
        if let Ok(json) = serde_json::to_vec(message) {
            self.push(Out::Json(json));
        }
    }
}

/// A client attached.
pub(super) struct Client {
    pub(super) id: String,
    pub(super) stream: UnixStream,
    pub(super) out: Arc<Outbox>,
    pub(super) caps: Caps,
    pub(super) term: String,
    pub(super) size: (u16, u16),
    pub(super) since: u64,
    pub(super) painter: Painter,
    /// Its writer has a frame to write.
    pub(super) busy: bool,
    /// A frame came due while it was busy: the next one goes when it is ready.
    pub(super) late: bool,
    /// The client was told it gets every move of the mouse (1003).
    pub(super) motion: bool,
    pub(super) relays: Relays,
}

/// What the programs asked to hand on to the real terminal, until the client's next frame: bounded by nature.
#[derive(Default)]
pub(super) struct Relays {
    pub(super) clipboard: Option<Relay>,
    pub(super) title: Option<Relay>,
    pub(super) progress: Option<Relay>,
    pub(super) bell: bool,
    pub(super) notifications: VecDeque<Relay>,
}

impl Relays {
    pub(super) fn add(&mut self, relay: Relay) {
        match relay {
            Relay::Clipboard(_) => self.clipboard = Some(relay),
            Relay::Title(_) => self.title = Some(relay),
            Relay::Progress(_) => self.progress = Some(relay),
            Relay::Bell => self.bell = true,
            Relay::Notify { .. } => {
                if self.notifications.len() == NOTIFICATIONS {
                    self.notifications.pop_front();
                }
                self.notifications.push_back(relay);
            }
            Relay::Status(_) | Relay::Resize { .. } => {}
        }
    }

    pub(super) fn take(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let relays = [self.clipboard.take(), self.title.take(), self.progress.take()];
        for relay in relays.into_iter().flatten().chain(self.notifications.drain(..)) {
            bytes.extend(relay.encode().unwrap_or_default());
        }
        if std::mem::take(&mut self.bell) {
            bytes.extend(Relay::Bell.encode().unwrap_or_default());
        }
        bytes
    }
}

/// Accepts connections on `listener`, each served by a thread of its own.
pub(super) fn accept(listener: UnixListener, tx: Sender<Msg>, welcome: Arc<Mutex<Welcome>>) -> io::Result<()> {
    static CLIENTS: AtomicU64 = AtomicU64::new(0);
    std::thread::Builder::new().name("accept".into()).spawn(move || {
        // Under macOS, `accept` then `FD_CLOEXEC` is not atomic: a connection accepted while a pane forks may leak
        // into it until it execs. Rare, and the program gets a socket it does not know of.
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(error) => {
                    // A lasting one (EMFILE, ENFILE) would spin: said, then a pause.
                    log!("accept: {error}");
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            if !socket::same_user(&stream) {
                log!("connection refused: uid {:?}", socket::peer_uid(&stream).ok());
                continue;
            }
            let (tx, welcome) = (tx.clone(), Arc::clone(&welcome));
            let spawned = std::thread::Builder::new().name("connection".into()).spawn(move || {
                if let Err(error) = connection(stream, &tx, &welcome, &CLIENTS) {
                    log!("connection: {error}");
                }
            });
            if let Err(error) = spawned {
                log!("connection: {error}");
            }
        }
    })?;
    Ok(())
}

/// A connection, from its `Hello` to its end.
pub(super) fn connection(
    mut stream: UnixStream,
    tx: &Sender<Msg>,
    welcome: &Mutex<Welcome>,
    clients: &AtomicU64,
) -> io::Result<()> {
    stream.set_read_timeout(Some(HELLO_TIME))?;
    let Some(hello) = proto::recv::<Hello>(&mut stream)? else { return Ok(()) };
    let answer = welcome.lock().unwrap_or_else(PoisonError::into_inner).clone();
    proto::send(&mut stream, &answer)?;
    let ours = hello.proto == proto::PROTO;
    match hello.kind {
        Kind::Command => {
            let request = match proto::read_frame(&mut stream)? {
                Some(proto::Frame::Json(json)) => serde_json::from_slice::<Request>(&json),
                _ => return Ok(()),
            };
            // Another protocol: only `Stop`, frozen, is taken.
            let reply = match request {
                Ok(request) if ours || request == Request::Stop => return command(stream, tx, request),
                Err(error) if ours => Reply::Err(format!("request: {error}")),
                _ => Reply::Refused(Refusal::Proto),
            };
            proto::send(&mut stream, &reply)
        }
        Kind::Attach if !ours => proto::send(&mut stream, &ServerMsg::Refused(Refusal::Proto)),
        Kind::Attach => {
            let Some(ClientMsg::Attach { caps, cols, rows, term, env }) = proto::recv::<ClientMsg>(&mut stream)? else {
                return Ok(());
            };
            stream.set_read_timeout(None)?;
            socket::enlarge_send_buffer(&stream);
            let id = format!("c{}", clients.fetch_add(1, Ordering::Relaxed) + 1);
            let out = Arc::new(Outbox::default());
            let writer = stream.try_clone()?;
            let (writer_out, writer_tx, writer_id) = (Arc::clone(&out), tx.clone(), id.clone());
            std::thread::Builder::new()
                .name(format!("write-{id}"))
                .spawn(move || write_client(writer, &writer_out, &writer_tx, &writer_id))?;
            let attaching =
                Attaching { id: id.clone(), stream: stream.try_clone()?, out, caps, size: (cols, rows), term, env };
            if tx.send(Msg::Attach(Box::new(attaching))).is_err() {
                return Ok(());
            }
            // What the client says, until it goes.
            loop {
                match proto::recv::<ClientMsg>(&mut stream) {
                    Ok(Some(message)) => {
                        if tx.send(Msg::Client(id.clone(), message)).is_err() {
                            return Ok(());
                        }
                    }
                    Ok(None) | Err(_) => {
                        let _ = tx.send(Msg::Gone(id));
                        return Ok(());
                    }
                }
            }
        }
        Kind::Other => proto::send(&mut stream, &Reply::Refused(Refusal::Other)),
    }
}

/// A command handed to the loop; its answer written, then said written.
pub(super) fn command(mut stream: UnixStream, tx: &Sender<Msg>, request: Request) -> io::Result<()> {
    let (reply_tx, reply) = mpsc::channel();
    let (written_tx, written) = mpsc::channel();
    if tx.send(Msg::Command(request, reply_tx, written)).is_err() {
        return proto::send(&mut stream, &Reply::Err(t!("le serveur s'arrête", "the server is stopping")));
    }
    let reply = reply.recv().unwrap_or_else(|_| Reply::Err(t!("le serveur s'arrête", "the server is stopping")));
    let result = proto::send(&mut stream, &reply);
    let _ = written_tx.send(());
    result
}

/// A client's writer: what the loop gives it, in order; it says when it has written all.
fn write_client(stream: UnixStream, out: &Outbox, tx: &Sender<Msg>, id: &str) {
    write_all(stream, out, tx, id);
    out.set_closed();
}

fn write_all(mut stream: UnixStream, out: &Outbox, tx: &Sender<Msg>, id: &str) {
    loop {
        let batch: Vec<Out> = {
            let mut queue = out.queue.lock().unwrap_or_else(PoisonError::into_inner);
            while queue.is_empty() {
                queue = out.ready.wait(queue).unwrap_or_else(PoisonError::into_inner);
            }
            queue.drain(..).collect()
        };
        for item in batch {
            let written = match item {
                Out::Json(json) => proto::send_json(&mut stream, &json),
                Out::Output(bytes) => proto::send_output(&mut stream, &bytes),
                Out::Close => {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return;
                }
            };
            if written.is_err() {
                // The reader sees the end too, and says the client gone.
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
        }
        if tx.send(Msg::Ready(id.to_string())).is_err() {
            return;
        }
    }
}

impl Server {
    /// A new client: it takes the team over from the one that had it.
    pub(super) fn attach(&mut self, attaching: Attaching) {
        let Attaching { id, stream, out, caps, size, term, env } = attaching;
        if self.client.is_some() {
            let _ = self.drop_client(Some(Bye::Replaced));
        }
        log!("client {id} attached: {term}, {}x{}", size.0, size.1);
        self.env = proto::UPDATE_ENV
            .iter()
            .map(|name| (name.to_string(), env.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone())))
            .collect();
        // Its background tells the palette (dark when it said none), and the programs that ask it the theme (`CSI ? 996
        // n`, mode 2031): told the one they do not know yet.
        let light = caps.bg.is_some_and(look::light_background);
        for pane in &self.panes {
            let mut replies = Vec::new();
            pane.with(|engine| {
                engine.set_colors(caps.fg, caps.bg);
                engine.color_scheme(light, &mut replies);
            });
            if !replies.is_empty() {
                pane.send(replies);
            }
        }
        look::set_light(light);
        self.theme(light);
        out.message(&ServerMsg::Attached { client: id.clone() });
        let painter = Painter::new(caps.output);
        self.client = Some(Client {
            id,
            stream,
            out,
            caps,
            term,
            size,
            since: now(),
            painter,
            busy: false,
            late: false,
            motion: false,
            relays: Relays::default(),
        });
        self.set_attached();
        self.resize(size);
    }

    /// The client's theme is `light` (or dark). Changed since the client before: the whole image is drawn again, the
    /// cells kept by the screen were painted with the other palette. True when it changed.
    pub(super) fn theme(&mut self, light: bool) -> bool {
        if light == self.light {
            return false;
        }
        self.light = light;
        self.screen.borrow_mut().redraw();
        self.dirty = true;
        true
    }

    /// Lets the client go: told why, when it is still there to hear it.
    /// Returns its outbox, to wait for its writer to leave.
    pub(super) fn drop_client(&mut self, bye: Option<Bye>) -> Option<Arc<Outbox>> {
        // The menu, and a choice, were opened for this client: they would act for one that is gone.
        if let Some((menu, _)) = self.menu.clone() {
            self.close_pane(&menu);
        }
        self.choice = None;
        // Its pointer with it, and what it chose with ⌥l: the tabs go back to what the team says.
        self.pointer.hover = None;
        self.pointer.at = None;
        if self.tabs.iter().any(|tab| tab.mode.is_some()) {
            for tab in &mut self.tabs {
                tab.mode = None;
            }
            self.layout();
        }
        let client = self.client.take()?;
        self.set_attached();
        let Client { out, stream, .. } = client;
        match bye {
            Some(bye) => {
                out.message(&ServerMsg::Bye(bye));
                out.push(Out::Close);
                // Shut if it does not read its goodbye in time: its writer may be stuck in a write (a dead SSH).
                let watched = Arc::clone(&out);
                let spawned = std::thread::Builder::new().name("bye".into()).spawn(move || {
                    if !watched.wait_closed(BYE_TIME) {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                });
                if let Err(error) = spawned {
                    log!("bye: {error}");
                }
            }
            // Gone: its writer may be stuck in a write; the shut wakes it.
            None => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                out.push(Out::Close);
            }
        }
        Some(out)
    }

    fn set_attached(&self) {
        let mut welcome = self.welcome.lock().unwrap_or_else(PoisonError::into_inner);
        welcome.attached = self.client.is_some();
    }

    pub(super) fn client_message(&mut self, message: ClientMsg) {
        match message {
            ClientMsg::Attach { .. } => {}
            ClientMsg::Input(event) => self.input(event),
            ClientMsg::Redraw => {
                if let Some(client) = self.client.as_mut() {
                    client.painter.forget();
                }
                self.dirty = true;
            }
            ClientMsg::Detach => {
                let _ = self.drop_client(Some(Bye::Detached));
            }
        }
    }
}
