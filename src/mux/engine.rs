// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The terminal a pane's program writes to: a VT engine behind [`Engine`], so that the rest of the code never sees
//! which one (spec §5.2). The engine is libghostty-vt (`engine/ghostty.rs`, built by scripts/ghostty.sh), with a
//! sniffer in front of it for what it lets pass (`engine/sniff.rs`).
//!
//! Owner: dev-terminal.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::Result;

use super::{Rect, Rgb};
use crate::canvas::{Canvas, Cursor};

mod ghostty;
mod sniff;

/// A new engine, `cols` × `rows`, keeping `history` lines above the screen.
pub(crate) fn new(cols: u16, rows: u16, history: usize) -> Result<Box<dyn Engine>> {
    Ok(Box::new(ghostty::Ghostty::new(cols, rows, history)?))
}

/// A pane's terminal. Fed from the PTY's reader thread and drawn from the screen's: an engine lives behind a mutex,
/// and no call may wait on anything.
pub(crate) trait Engine: Send {
    /// Parses what the program wrote. What the terminal answers (DA1, DA2, DSR and CPR, DECRQM, XTVERSION, kitty
    /// keyboard query, OSC 10 and 11) is appended to `replies`, for the PTY. While the program holds a synchronized
    /// update (mode 2026), what it writes is kept aside, and [`Engine::draw`] goes on showing the screen as before.
    fn feed(&mut self, bytes: &[u8], replies: &mut Vec<u8>);

    /// When the engine wants [`Engine::expire`] called, if it does: the end of a synchronized update that never got
    /// its closing sequence, or spare work to finish once the pane is quiet (history compression). `None` at rest.
    fn deadline(&self) -> Option<Instant>;

    /// Does what was due by `now`: ends a synchronized update past its deadline, as terminals do, or a bounded step
    /// of spare work. Whether the screen changed.
    fn expire(&mut self, now: Instant, replies: &mut Vec<u8>) -> bool;

    /// What the program asked to reach the real terminal, or the interface, since the last call (spec §5.6).
    fn relays(&mut self, out: &mut Vec<Relay>);

    fn resize(&mut self, cols: u16, rows: u16);

    /// The real terminal's colors, for the OSC 10 and 11 queries; unknown, the engine answers its defaults.
    fn set_colors(&mut self, fg: Option<Rgb>, bg: Option<Rgb>);

    /// The modes the program asked for, which decide how its input is encoded (`input.rs`).
    fn modes(&self) -> Modes;

    /// Draws what the pane shows into `area` of `canvas`: the live screen, or the history where [`Engine::scroll`]
    /// left it. Cells go one by one with the width the engine gave them (`Canvas::put_cell`): what the program
    /// measured is what is drawn.
    fn draw(&self, canvas: &mut Canvas, area: Rect);

    /// What [`Engine::draw`] would draw, as a number: it changes when the cells drawn change (the screen, the view
    /// moved into the history, the colors), and stays the same otherwise, whoever drew in between; so that the screen
    /// can keep a pane's cells from one frame to the next (the area, the selection and the frame around them are the
    /// screen's to compare). Not the cursor, which [`Engine::cursor`] gives up to date after this call. Unique to this
    /// engine: no other engine of the process gives the same number ([`fresh_generation`]), a new engine for the same
    /// pane included. Asked of every pane at every frame: nothing is read again when nothing changed. `None` for an
    /// engine that does not tell: drawn every time.
    fn generation(&self) -> Option<u64> {
        None
    }

    /// The cursor, in the pane's cells, as the program left it; `None` when hidden.
    fn cursor(&self) -> Option<Cursor>;

    /// Lines kept above the screen.
    fn history(&self) -> usize;

    /// Moves the view `lines` up into the history (down when negative), within it; `isize::MIN` goes back to the
    /// live screen. While the view is up, what the program writes keeps it on the same lines, as in terminals.
    fn scroll(&mut self, lines: isize);

    /// Lines the view is up into the history; 0 on the live screen.
    fn scrolled(&self) -> usize;

    /// The line on the pane's first row, scroll included, counted from the first line the pane ever had (history
    /// included): a point of a selection (`select.rs`) stays on its text while the program writes. On the alternate
    /// screen, its own lines, from 0.
    fn top(&self) -> u64 {
        0
    }

    /// The oldest line still kept.
    fn oldest(&self) -> u64 {
        0
    }

    /// Line `line`'s cells, one grapheme per column (see [`Line`]), and whether it goes on in the next one (soft
    /// wrap); `None` when it is no longer kept, or not written yet (below the screen's last row).
    fn line(&self, line: u64) -> Option<Line> {
        let _ = line;
        None
    }
}

/// A number for [`Engine::generation`] that no other call gives in this process.
fn fresh_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A line of a pane's text, for a selection: see [`Engine::line`]. Two allocations a line, rather than one a cell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Line {
    /// The cells' graphemes one after the other: a blank for an empty cell, nothing for the second column of a wide
    /// one.
    pub text: String,
    /// Where each column starts in `text`, then where the last one ends: one more than the columns.
    pub starts: Vec<u32>,
    /// It goes on in the next line (soft wrap).
    pub wrapped: bool,
}

impl Line {
    /// One cell more: its grapheme, " " when empty, "" for the second column of a wide one.
    pub(crate) fn push(&mut self, grapheme: &str) {
        if self.starts.is_empty() {
            self.starts.push(0);
        }
        self.text.push_str(grapheme);
        self.starts.push(self.text.len() as u32);
    }

    /// Its columns.
    pub(crate) fn width(&self) -> usize {
        self.starts.len().saturating_sub(1)
    }

    /// Column `col`'s grapheme; "" past the end.
    pub(crate) fn cell(&self, col: usize) -> &str {
        self.span(col, col + 1)
    }

    /// Columns `from..to` as text, cut at the end.
    pub(crate) fn span(&self, from: usize, to: usize) -> &str {
        let to = to.min(self.width());
        if from >= to {
            return "";
        }
        self.text.get(self.starts[from] as usize..self.starts[to] as usize).unwrap_or("")
    }
}

/// The modes a program sets on its terminal that change what its input looks like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Modes {
    /// DECCKM: cursor keys sent as `ESC O A` rather than `ESC [ A`.
    pub app_cursor: bool,
    /// DECKPAM: the keypad in application mode.
    pub app_keypad: bool,
    /// 2004: pastes framed by `ESC [ 200 ~` and `ESC [ 201 ~`.
    pub bracketed_paste: bool,
    /// 1004: focus reported, `ESC [ I` and `ESC [ O`.
    pub focus: bool,
    /// Which mouse events the program asked for, if any (1000, 1002, 1003).
    pub mouse: Option<MouseTracking>,
    /// 1006: mouse reports in SGR form.
    pub mouse_sgr: bool,
    /// 1007: on the alternate screen, the wheel sent as cursor keys when the mouse is not reported.
    pub alternate_scroll: bool,
    /// 1049, 1047, 47: the alternate screen is on.
    pub alt_screen: bool,
    /// The kitty keyboard protocol's flags in effect (`CSI = | > flags u`), 0 when off.
    pub kitty: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseTracking {
    /// 1000: presses, releases and the wheel.
    Click,
    /// 1002: and moves while a button is down.
    Drag,
    /// 1003: and every move.
    Motion,
}

/// What a program writes for the real terminal, or for the interface, rather than for its screen (spec §5.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Relay {
    /// OSC 52: text for the system clipboard. A program reading the clipboard is never answered.
    Clipboard(String),
    /// OSC 0, 2: the title.
    Title(String),
    /// BEL.
    Bell,
    /// OSC 9, 777 and 99: a desktop notification.
    Notify { title: Option<String>, body: String },
    /// OSC 9;4: progress.
    Progress(Progress),
    /// OSC 7501: what the program says it is doing. Claude Code writes it once its terminal answered
    /// `OSC 7501 ; ?`, which the engine does: idle, working, blocked (and why), done.
    Status(Status),
}

/// A program's status (OSC 7501: `state=…:app=…:id=…:kind=…:progress=…:title=…:msg=…`, title and message in
/// base64).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub state: State,
    /// Why it is blocked: `permission`…
    pub kind: Option<String>,
    /// Percent.
    pub progress: Option<u8>,
    pub title: Option<String>,
    pub message: Option<String>,
    /// Tells apart several statuses of the same program.
    pub id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Ready, nothing asked yet.
    Idle,
    Working,
    /// Waiting on the user: a permission, a question.
    Blocked,
    /// Its turn is over.
    Done,
    /// No status any more (the program is leaving).
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    Clear,
    /// Percent.
    Set(u8),
    Error(Option<u8>),
    Indeterminate,
    Paused(Option<u8>),
}

impl Relay {
    /// The sequence that hands it on to the real terminal, if it goes there.
    /// Text from the program goes without its control characters (C0, DEL, C1): it must not write sequences of
    /// its own to the real terminal. Sequences end with BEL, which every terminal takes.
    pub(crate) fn encode(&self) -> Option<Vec<u8>> {
        let osc = |body: String| Some(format!("\x1b]{body}\x07").into_bytes());
        match self {
            Relay::Clipboard(text) => osc(format!("52;c;{}", sniff::base64_encode(text.as_bytes()))),
            Relay::Title(title) => osc(format!("2;{}", printable(title))),
            Relay::Bell => Some(b"\x07".to_vec()),
            // OSC 9 (iTerm2, Ghostty, kitty, WezTerm), the most widely shown. A text that starts with a number
            // would be read as one of ConEmu's commands.
            Relay::Notify { title, body } => {
                let text = match title {
                    Some(title) if !title.is_empty() => format!("{}: {}", printable(title), printable(body)),
                    _ => printable(body),
                };
                let lead = if text.starts_with(|c: char| c.is_ascii_digit()) { " " } else { "" };
                osc(format!("9;{lead}{text}"))
            }
            Relay::Progress(progress) => osc(match progress {
                Progress::Clear => "9;4;0".to_string(),
                Progress::Set(percent) => format!("9;4;1;{percent}"),
                Progress::Error(Some(percent)) => format!("9;4;2;{percent}"),
                Progress::Error(None) => "9;4;2".to_string(),
                Progress::Indeterminate => "9;4;3".to_string(),
                Progress::Paused(Some(percent)) => format!("9;4;4;{percent}"),
                Progress::Paused(None) => "9;4;4".to_string(),
            }),
            // For the interface only.
            Relay::Status(_) => None,
        }
    }
}

/// `text` without control characters: C0, DEL and C1.
fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}
