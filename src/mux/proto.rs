// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What a team's server and the processes that reach it say to each other (specs/multiplexeur-serveur.md §3).
//!
//! Frames: a length on 4 bytes (big-endian, counting the type byte and the payload), a type byte, the payload. Type
//! 0 is a JSON message, both ways; type 1 is bytes for the real terminal (a frame of the screen, a relay), from the
//! server to a client.
//!
//! The first exchange is frozen for good, so that any recruit can list and stop any team: [`Hello`], [`Welcome`],
//! [`Refusal`], [`Request::Stop`] and [`Reply`]. Their forms only ever grow: no `deny_unknown_fields`, new fields
//! with `#[serde(default)]`, a catch-all variant where a newer recruit may send one we do not know. The rest of the
//! protocol may change with [`PROTO`]; a test compares its JSON with a reference, so that a change cannot go
//! unnoticed.
//!
//! Owner: dev-serveur.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::Caps;
use super::input::Event;

/// The protocol's number, raised at each change that an older client or server cannot follow. Client and server
/// refuse each other on it, not on recruit's version: a team launched before an update stays reachable.
pub(crate) const PROTO: u32 = 1;

/// recruit's version, for the messages.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A frame announced longer than this closes the connection: a length gone wrong is never allocated.
pub(crate) const MAX_FRAME: usize = 64 << 20;

/// The payload is read by pieces of this size, in a buffer that grows with what comes.
const CHUNK: usize = 64 << 10;

const JSON: u8 = 0;
const OUTPUT: u8 = 1;

/// A frame read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// A JSON message, still to be read as the type the conversation expects.
    Json(Vec<u8>),
    /// Bytes for the real terminal.
    Output(Vec<u8>),
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Writes one frame in one write: a message or a frame of the screen is never split by another thread's.
fn write_frame(out: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let length = payload.len() + 1;
    if length > MAX_FRAME {
        return Err(invalid(format!("frame of {length} bytes")));
    }
    let mut frame = Vec::with_capacity(4 + length);
    frame.extend_from_slice(&(length as u32).to_be_bytes());
    frame.push(kind);
    frame.extend_from_slice(payload);
    out.write_all(&frame)?;
    out.flush()
}

/// Writes `message` as a JSON frame.
pub(crate) fn send<T: Serialize>(out: &mut impl Write, message: &T) -> io::Result<()> {
    let json = serde_json::to_vec(message).map_err(io::Error::other)?;
    write_frame(out, JSON, &json)
}

/// Writes a message already in JSON.
pub(crate) fn send_json(out: &mut impl Write, json: &[u8]) -> io::Result<()> {
    write_frame(out, JSON, json)
}

/// Writes bytes for the real terminal.
pub(crate) fn send_output(out: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    write_frame(out, OUTPUT, bytes)
}

/// Reads the next frame; `None` when the other side closed between two frames.
pub(crate) fn read_frame(input: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < head.len() {
        match input.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let length = u32::from_be_bytes(head) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(invalid(format!("frame of {length} bytes")));
    }
    let mut kind = [0u8; 1];
    input.read_exact(&mut kind)?;
    // By pieces: a length is only believed as far as the bytes that come.
    let rest = length - 1;
    let mut payload = Vec::with_capacity(rest.min(CHUNK));
    let read = input.by_ref().take(rest as u64).read_to_end(&mut payload)?;
    if read < rest {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    match kind[0] {
        JSON => Ok(Some(Frame::Json(payload))),
        OUTPUT => Ok(Some(Frame::Output(payload))),
        other => Err(invalid(format!("frame of type {other}"))),
    }
}

/// Reads the next frame as a JSON message of type `T`; `None` when the other side closed.
pub(crate) fn recv<T: DeserializeOwned>(input: &mut impl Read) -> io::Result<Option<T>> {
    match read_frame(input)? {
        None => Ok(None),
        Some(Frame::Json(json)) => serde_json::from_slice(&json).map(Some).map_err(|e| invalid(e.to_string())),
        Some(Frame::Output(_)) => Err(invalid("bytes where a message was expected")),
    }
}

// The frozen part. Its JSON never changes, it only grows (module comment).

/// The first message of whoever connects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Hello {
    pub proto: u32,
    pub version: String,
    pub kind: Kind,
}

impl Hello {
    pub(crate) fn new(kind: Kind) -> Hello {
        Hello { proto: PROTO, version: VERSION.to_string(), kind }
    }
}

/// What the connection is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    /// A client: the screen and the input, until it leaves.
    Attach,
    /// One [`Request`], one [`Reply`].
    Command,
    /// A kind of a newer recruit.
    #[serde(other)]
    Other,
}

/// The server's answer to [`Hello`], always, whatever the client's protocol: what `recruit list` reads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Welcome {
    pub proto: u32,
    pub version: String,
    pub team: String,
    pub session: String,
    /// Where the members work. A string, not a path: serde_json fails on a path that is not UTF-8.
    pub dir: String,
    pub pid: u32,
    /// A client is attached.
    pub attached: bool,
}

/// Why the server does not take what was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Refusal {
    /// The client's protocol is not the server's: only [`Request::Stop`] is taken.
    Proto,
    /// Another client has the team, and this one did not ask to take it over.
    Busy,
    #[serde(other)]
    Other,
}

/// The answer to a [`Request`]. `Ok`, `Err` and `Refused` are frozen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reply {
    /// What the request returns; `null` when nothing.
    Ok(serde_json::Value),
    /// What went wrong, in the team's language.
    Err(String),
    Refused(Refusal),
}

impl Reply {
    /// The value of an `Ok`, read as `T`; an error for the others, with what to tell the user.
    pub(crate) fn value<T: DeserializeOwned>(self) -> Result<T, ReplyError> {
        match self {
            Reply::Ok(value) => serde_json::from_value(value).map_err(|e| ReplyError::Invalid(e.to_string())),
            Reply::Err(message) => Err(ReplyError::Failed(message)),
            Reply::Refused(refusal) => Err(ReplyError::Refused(refusal)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReplyError {
    Failed(String),
    Refused(Refusal),
    Invalid(String),
}

/// What a command connection asks. `Stop` is frozen; the others follow [`PROTO`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Request {
    /// Stops the team; answered once its panes have ended.
    Stop,
    /// The panes, tab by tab: `Vec<PaneInfo>`.
    Panes,
    KillPane {
        id: String,
    },
    SetMember {
        id: String,
        member: String,
    },
    /// The team as `launch` plans it, opened in an empty server; the screen `cols` × `rows` until a client says its
    /// own.
    Build {
        team: String,
        dir: String,
        columns: usize,
        tabs: Vec<TabSpec>,
        cols: u16,
        rows: u16,
        /// The recruit the server runs its own programs with (the menu): the launcher's.
        #[serde(default)]
        exe: String,
        /// The team's language, for them.
        #[serde(default)]
        lang: String,
    },
    /// A pane in a tab of its own, at the end, titled after it: its id.
    OpenWindow {
        pane: PaneSpec,
    },
    /// Pane `id`'s program started again, as `pane` says.
    Respawn {
        id: String,
        pane: PaneSpec,
    },
    /// The members' panes put in these tabs (title, members in reading order), in this order, none stopped; the
    /// panels stay beside the first.
    Arrange {
        tabs: Vec<(String, Vec<String>)>,
        columns: usize,
    },
    /// The dashboard over the reduced journal, beside pane `beside`.
    OpenPanels {
        beside: String,
        dashboard: PaneSpec,
        journal: PaneSpec,
    },
    ClosePanels,
    /// The dashboard opened again: over the journal, else beside the first member.
    RestoreDashboard {
        dashboard: PaneSpec,
    },
    /// The journal to its next size (full, reduced, hidden): its new one, `full`, `reduced` or `hidden`. `journal`
    /// opens it again once hidden.
    ToggleJournal {
        journal: PaneSpec,
    },
    /// Gives the focus to a member's pane, in its tab: `bool`, false when it has none.
    Focus {
        member: String,
    },
    /// Opens the team's menu: on the client that shows `member`, or on `client`. With `field`, on `member`'s sheet,
    /// on that field (written only when there is one: the form of step 2 stays). `MenuOpened`.
    OpenMenu {
        #[serde(default)]
        member: Option<String>,
        #[serde(default)]
        client: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<MenuField>,
    },
    Detach {
        client: String,
    },
    /// The clients attached: `Vec<ClientInfo>`.
    Clients,
    /// For tests and captures: a pane's text, or the whole screen as composed when `pane` is `None`; with its
    /// styles as escape sequences when `styles`. A `String`.
    Capture {
        #[serde(default)]
        pane: Option<String>,
        #[serde(default)]
        styles: bool,
        /// Lines of a pane's history above its screen, at most.
        #[serde(default)]
        history: usize,
    },
    /// For the bench: what each pane read, what the server sent. `Stats`.
    Stats,
    /// For tests: bytes written as they are to a pane's program, as `tmux send-keys` does.
    Send {
        pane: String,
        bytes: Vec<u8>,
    },
    /// For tests: an event encoded for `pane` as its modes ask; without a pane, routed as a client's would be,
    /// shortcuts first.
    Key {
        #[serde(default)]
        pane: Option<String>,
        event: Event,
    },
    /// For tests: a pane running `argv` in `cwd`, in tab `tab` (made if missing; the current one when `None`). Its id.
    Spawn {
        member: String,
        argv: Vec<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
        cwd: String,
        #[serde(default)]
        tab: Option<String>,
    },
}

/// A pane to open: what it shows and runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaneSpec {
    /// The member's name, or the panel's title.
    pub member: String,
    /// A panel's kind (`dashboard`, `journal`); empty for a member.
    #[serde(default)]
    pub role: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Where it runs: absolute.
    pub cwd: String,
}

/// A tab to open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TabSpec {
    pub title: String,
    pub panes: Vec<PaneSpec>,
    /// The dashboard, then the journal, in a column on the right.
    #[serde(default)]
    pub side: Option<(PaneSpec, PaneSpec)>,
}

/// A pane, as [`Request::Panes`] gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaneInfo {
    pub id: String,
    /// The member's name, or the panel's title.
    pub member: String,
    /// A panel's kind; empty for a member.
    pub role: String,
    /// Its tab's title.
    pub tab: String,
    /// Its program's pid.
    pub pid: u32,
    pub cols: u16,
    pub rows: u16,
}

/// What [`Request::Stats`] gives.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stats {
    /// The server's pid.
    pub pid: u32,
    pub panes: Vec<PaneStats>,
    /// Frames sent to clients, and bytes written to them, frames and relays, since the server started.
    pub frames: u64,
    pub written: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaneStats {
    pub id: String,
    pub member: String,
    /// Its program's pid.
    pub pid: u32,
    pub cols: u16,
    pub rows: u16,
    /// Lines kept above its screen.
    pub history: usize,
    /// Bytes its programs wrote since it opened.
    pub read: u64,
}

/// What [`Request::Capture`] gives.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Captured {
    /// One per row, without the blanks at the end.
    pub lines: Vec<String>,
    /// A pane's history above its live screen, oldest first, as many lines as asked at most.
    #[serde(default)]
    pub history: Vec<String>,
    /// Column and row of the cursor, from 0; `None` when hidden.
    pub cursor: Option<(usize, usize)>,
    /// A pane's modes; `None` for the whole screen.
    pub modes: Option<PaneModes>,
}

/// A pane's modes, for the tests.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaneModes {
    pub kitty: u8,
    pub bracketed_paste: bool,
    pub focus: bool,
    /// 0, 1000, 1002 or 1003.
    pub mouse: u16,
    pub mouse_sgr: bool,
    pub alt_screen: bool,
    pub app_cursor: bool,
    /// Lines up into the history the pane shows.
    pub scrolled: usize,
    pub history: usize,
}

/// What a client says, once welcomed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClientMsg {
    /// The first: what the terminal is and can do. The team is taken over from the client that had it, if any.
    Attach {
        caps: Caps,
        cols: u16,
        rows: u16,
        /// Its `TERM`.
        term: String,
        /// The variables of [`UPDATE_ENV`] as the client has them, for the programs started from now on.
        #[serde(default)]
        env: Vec<(String, String)>,
    },
    Input(Event),
    /// A whole frame, rather than what changed: after SIGCONT, or a terminal cleared.
    Redraw,
    /// The client leaves of its own (SIGTERM).
    Detach,
}

/// What the server says to a client, besides the frames (type 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServerMsg {
    /// The client's id, once it has the team.
    Attached {
        client: String,
    },
    /// Whether every move of the mouse is wanted (1003), by the focused pane or the chrome: the client asks its
    /// terminal for them only then. Sent on attaching, then when it changes; presses, drags and the wheel always come.
    Motion(bool),
    /// The last message: why the client leaves.
    Bye(Bye),
    Refused(Refusal),
}

/// Why a client is let go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Bye {
    /// Detached, at its request or the menu's.
    Detached,
    /// Another client took the team over.
    Replaced,
    /// The team was stopped.
    Stopped,
    /// The team's last pane ended.
    Ended,
    Error(String),
    #[serde(other)]
    Other,
}

/// The variables that follow the client, for the programs started from then on (spec §1.2, as tmux's
/// `update-environment`).
pub(crate) const UPDATE_ENV: &[&str] = &[
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "SSH_CONNECTION",
    "SSH_ASKPASS",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "KRB5CCNAME",
];

/// A client attached, as [`Request::Clients`] gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClientInfo {
    /// `c1`, `c2`…: never given twice in a server's life.
    pub id: String,
    /// Its `TERM`.
    pub term: String,
    /// Its terminal's name and version, when it gave them (XTVERSION).
    #[serde(default)]
    pub name: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// Since when it is attached, in seconds since the epoch.
    #[serde(default)]
    pub since: u64,
}

/// The field of a member's sheet the menu opens on ([`Request::OpenMenu`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MenuField {
    Name,
    Model,
    Effort,
    /// One this side does not know: the sheet, on its first field.
    #[serde(other)]
    Other,
}

impl MenuField {
    /// As `recruit _menu --field` takes it; `None` for a field it would not know.
    pub(crate) fn arg(self) -> Option<&'static str> {
        match self {
            MenuField::Name => Some("name"),
            MenuField::Model => Some("model"),
            MenuField::Effort => Some("effort"),
            MenuField::Other => None,
        }
    }
}

/// The answer to [`Request::OpenMenu`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MenuOpened {
    Opened,
    /// No client shows the team.
    NoClient,
    /// It was open already, on that client.
    AlreadyOpen,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json<T: Serialize>(message: &T) -> String {
        serde_json::to_string(message).unwrap()
    }

    fn read<T: DeserializeOwned>(json: &str) -> T {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn frames_go_and_come_back() {
        let mut wire = Vec::new();
        send(&mut wire, &Request::Panes).unwrap();
        send_output(&mut wire, b"\x1b[?2026h").unwrap();
        send_output(&mut wire, b"").unwrap();
        // The length counts the type byte.
        assert_eq!(&wire[..5], &[0, 0, 0, 8, 0]);
        let mut input = wire.as_slice();
        assert_eq!(recv::<Request>(&mut input).unwrap(), Some(Request::Panes));
        assert_eq!(read_frame(&mut input).unwrap(), Some(Frame::Output(b"\x1b[?2026h".to_vec())));
        assert_eq!(read_frame(&mut input).unwrap(), Some(Frame::Output(Vec::new())));
        assert_eq!(read_frame(&mut input).unwrap(), None, "closed between two frames");
    }

    #[test]
    fn bad_frames_are_refused() {
        let kind = |input: &[u8]| read_frame(&mut &input[..]).unwrap_err().kind();
        // An absurd length is refused before anything is allocated.
        assert_eq!(kind(&[0xff, 0xff, 0xff, 0xff, 0]), io::ErrorKind::InvalidData);
        assert_eq!(kind(&[0, 0, 0, 0]), io::ErrorKind::InvalidData);
        assert_eq!(kind(&[0, 0, 0, 2, 7, 0]), io::ErrorKind::InvalidData, "unknown type");
        // Cut in the middle.
        assert_eq!(kind(&[0, 0]), io::ErrorKind::UnexpectedEof);
        assert_eq!(kind(&[0, 0, 0, 9, 1, b'a']), io::ErrorKind::UnexpectedEof);
        // Bytes where a message was expected.
        let mut wire = Vec::new();
        send_output(&mut wire, b"x").unwrap();
        assert!(recv::<Request>(&mut wire.as_slice()).is_err());
    }

    #[test]
    fn a_large_frame_is_read_by_pieces() {
        let big = vec![b'x'; 3 * CHUNK + 17];
        let mut wire = Vec::new();
        send_output(&mut wire, &big).unwrap();
        assert_eq!(read_frame(&mut wire.as_slice()).unwrap(), Some(Frame::Output(big)));
    }

    /// The frozen forms, as any recruit, older or newer, writes and reads them.
    #[test]
    fn the_frozen_part_never_changes() {
        let hello = Hello { proto: 1, version: "1.3.0".into(), kind: Kind::Command };
        assert_eq!(json(&hello), r#"{"proto":1,"version":"1.3.0","kind":"command"}"#);
        assert_eq!(json(&Kind::Attach), r#""attach""#);
        let welcome = Welcome {
            proto: 1,
            version: "1.3.0".into(),
            team: "mux".into(),
            session: "mux".into(),
            dir: "/w".into(),
            pid: 42,
            attached: true,
        };
        assert_eq!(
            json(&welcome),
            r#"{"proto":1,"version":"1.3.0","team":"mux","session":"mux","dir":"/w","pid":42,"attached":true}"#
        );
        assert_eq!(json(&Request::Stop), r#""stop""#);
        assert_eq!(json(&Reply::Ok(serde_json::Value::Null)), r#"{"ok":null}"#);
        assert_eq!(json(&Reply::Err("non".into())), r#"{"err":"non"}"#);
        assert_eq!(json(&Reply::Refused(Refusal::Proto)), r#"{"refused":"proto"}"#);
        assert_eq!(json(&Reply::Refused(Refusal::Busy)), r#"{"refused":"busy"}"#);
    }

    /// What a newer recruit may add is read, not refused.
    #[test]
    fn the_frozen_part_reads_what_comes_later() {
        let hello: Hello = read(r#"{"proto":7,"version":"9.0.0","kind":"watch","colour":"blue"}"#);
        assert_eq!(hello, Hello { proto: 7, version: "9.0.0".into(), kind: Kind::Other });
        let welcome: Welcome = read(
            r#"{"proto":7,"version":"9.0.0","team":"t","session":"t","dir":"/","pid":1,"attached":false,"x":[1]}"#,
        );
        assert_eq!(welcome.proto, 7);
        let reply: Reply = read(r#"{"refused":"quota"}"#);
        assert_eq!(reply, Reply::Refused(Refusal::Other));
    }

    /// The rest of the protocol, as this `PROTO` writes it: a change here means raising it.
    #[test]
    fn the_protocol_is_as_numbered() {
        assert_eq!(PROTO, 1, "a change below raises PROTO, and this test with it");
        let requests = [
            Request::Panes,
            Request::KillPane { id: "p1".into() },
            Request::SetMember { id: "p1".into(), member: "dev".into() },
            Request::ClosePanels,
            Request::Focus { member: "dev".into() },
            Request::OpenMenu { member: Some("dev".into()), client: None, field: None },
            Request::OpenMenu { member: Some("dev".into()), client: None, field: Some(MenuField::Effort) },
            Request::Detach { client: "c1".into() },
            Request::Clients,
            Request::Capture { pane: None, styles: true, history: 0 },
            Request::Send { pane: "p2".into(), bytes: b"a\r".to_vec() },
        ];
        let text: Vec<String> = requests.iter().map(json).collect();
        assert_eq!(
            text,
            [
                r#""panes""#,
                r#"{"kill_pane":{"id":"p1"}}"#,
                r#"{"set_member":{"id":"p1","member":"dev"}}"#,
                r#""close_panels""#,
                r#"{"focus":{"member":"dev"}}"#,
                r#"{"open_menu":{"member":"dev","client":null}}"#,
                r#"{"open_menu":{"member":"dev","client":null,"field":"effort"}}"#,
                r#"{"detach":{"client":"c1"}}"#,
                r#""clients""#,
                r#"{"capture":{"pane":null,"styles":true,"history":0}}"#,
                r#"{"send":{"pane":"p2","bytes":[97,13]}}"#,
            ]
        );
        for request in requests {
            assert_eq!(read::<Request>(&json(&request)), request);
        }
        let client =
            ClientInfo { id: "c1".into(), term: "xterm-ghostty".into(), name: None, cols: 80, rows: 24, since: 0 };
        assert_eq!(json(&client), r#"{"id":"c1","term":"xterm-ghostty","name":null,"cols":80,"rows":24,"since":0}"#);
        assert_eq!(json(&MenuOpened::NoClient), r#""no_client""#);
        // A field a later version adds: the sheet, on its first field.
        assert_eq!(
            read::<Request>(r#"{"open_menu":{"member":"dev","field":"color"}}"#),
            Request::OpenMenu { member: Some("dev".into()), client: None, field: Some(MenuField::Other) }
        );
    }

    #[test]
    fn a_reply_gives_its_value() {
        assert_eq!(Reply::Ok(serde_json::json!(true)).value::<bool>(), Ok(true));
        assert_eq!(Reply::Err("non".into()).value::<bool>(), Err(ReplyError::Failed("non".into())));
        assert_eq!(Reply::Refused(Refusal::Proto).value::<()>(), Err(ReplyError::Refused(Refusal::Proto)));
        assert!(matches!(Reply::Ok(serde_json::json!("x")).value::<bool>(), Err(ReplyError::Invalid(_))));
    }
}
