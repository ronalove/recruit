// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A whole team on the native multiplexer, launched by `recruit` with a fake `claude` (`common::team`): the parity
//! checks of the spec's §7.1 (grid, « Parité ») that need no window. Part of `cargo test`.

#[path = "../common/mod.rs"]
mod common;

mod chrome;
mod compaction;
mod dashboard;
mod failed_turn;
mod launch;
mod menu;
mod quick;
mod screen;

use common::client::{alt, base64, click, fails, find, find_in_header, hover, report, says, thick};
use common::pty::{Profile, TestTerm};
use common::team::{Team, wait};
use std::time::{Duration, Instant};

#[test]
#[ignore = "started by the other tests only"]
fn child() {
    common::child_main();
}

/// `[teams.<name>]` with `contacts` and `agents` (`(name, tab)`), roles made up.
fn tables(name: &str, contacts: &[&str], agents: &[(&str, Option<&str>)], extra: &str) -> String {
    let mut out = format!("[teams.{name}]\ndescription = \"test\"\n{extra}\n");
    for c in contacts {
        out.push_str(&format!("[teams.{name}.members.{c}]\nrole = \"Contact {c}\"\ncontact = true\n"));
    }
    for (a, tab) in agents {
        out.push_str(&format!("[teams.{name}.members.{a}]\nrole = \"Agent {a}\"\n"));
        if let Some(tab) = tab {
            out.push_str(&format!("tab = \"{tab}\"\n"));
        }
    }
    out
}

/// The session `member` runs, once `member.rs` noted it (2 s after its start).
fn noted(team: &Team, member: &str) -> String {
    let file = team.state().join("sessions").join(member);
    assert!(wait(Duration::from_secs(8), || file.exists()), "{member}'s session never noted");
    std::fs::read_to_string(file).expect("session").trim().to_string()
}

/// A client attached to `team`, once it shows its first member.
fn client(team: &Team, member: &str) -> TestTerm {
    let term = team.attach(120, 40, Profile::default());
    assert!(term.wait_for(member, Duration::from_secs(5)), "no client: {:?}", term.screen());
    term
}

/// Whether a pane's header names `name` on `term`: a frame's top border, not the dashboard's card.
fn has_header(term: &TestTerm, name: &str) -> bool {
    term.screen().iter().any(|row| {
        row.contains(&format!(" {name} ")) && row.chars().any(|c| matches!(c, '┏' | '╭')) && row.contains(['━', '─'])
    })
}

/// The members, among `members`, whose header is thick on the screen of `term`: the active pane.
fn active(term: &TestTerm, members: &[&str]) -> Vec<String> {
    let screen = term.screen();
    members
        .iter()
        .filter(|m| screen.iter().any(|row| row.contains(&format!(" {m} ")) && thick(row, m)))
        .map(|m| m.to_string())
        .collect()
}

/// Collects the failed checks of a test, to report them all at once.
#[derive(Default)]
struct Checks(Vec<String>);

impl Checks {
    fn check(&mut self, what: &str, ok: bool, got: impl FnOnce() -> String) {
        if !ok {
            self.0.push(format!("{what}: {}", got()));
        }
    }

    fn done(self) {
        let more = self.0.len().saturating_sub(8);
        assert!(
            self.0.is_empty(),
            "{} failed:\n{}\n({more} more)",
            self.0.len(),
            self.0[..self.0.len() - more].join("\n")
        );
    }
}

fn shown(term: &TestTerm) -> String {
    term.screen().join("\n")
}

/// The text of the menu's pane (the server names it « recruit »), or nothing while there is none.
fn menu_text(team: &Team) -> Option<Vec<String>> {
    let out = team.ctl_output(["capture", "--pane", "recruit"]);
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect())
}

/// Closes the menu from wherever it is: Esc until its pane is gone.
fn close_menu(team: &Team, term: &mut TestTerm) -> bool {
    wait(Duration::from_secs(8), || {
        if menu_text(team).is_none() {
            // And the client's screen without it: a click sent before is lost.
            return wait(Duration::from_secs(3), || !shown(term).contains("Nouvel agent"));
        }
        term.send(b"\x1b");
        std::thread::sleep(Duration::from_millis(300));
        false
    })
}

/// Where the quick list (a click on the model or the effort of a header) is on the client's screen: its top left cell
/// and its size, which the menu in list mode announced (`CSI 8 ; rows ; columns t`) and the server gave its pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListBox {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl ListBox {
    fn right(&self) -> usize {
        self.x + self.w - 1
    }

    fn bottom(&self) -> usize {
        self.y + self.h - 1
    }
}

/// The quick list as drawn: found by its first row (« défaut : … »), its frame checked against the size
/// its pane has. None while it is not (all) there.
fn list_box(team: &Team, term: &TestTerm) -> Option<ListBox> {
    let panes = team.panes();
    let menu = panes.iter().find(|p| p["role"] == "menu")?;
    let (w, h) = (menu["cols"].as_u64()? as usize, menu["rows"].as_u64()? as usize);
    let screen = term.screen();
    let (row, line) = screen.iter().enumerate().find(|(_, line)| line.contains("défaut"))?;
    let label = unicode_width::UnicodeWidthStr::width(&line[..line.find("défaut")?]);
    // « │ › défaut »: the frame, a blank, the cursor, a blank.
    let found = ListBox { x: label.checked_sub(4)?, y: row.checked_sub(1)?, w, h };
    let corner = |col: usize, row: usize| term.cell(col, row).0;
    let framed = corner(found.x, found.y) == "╭"
        && corner(found.right(), found.y) == "╮"
        && corner(found.x, found.bottom()) == "╰"
        && corner(found.right(), found.bottom()) == "╯";
    framed.then_some(found)
}

/// Waits for the quick list, 8 s at most.
fn wait_list(team: &Team, term: &TestTerm) -> Option<ListBox> {
    let mut seen = None;
    wait(Duration::from_secs(8), || {
        seen = list_box(team, term);
        seen.is_some()
    });
    seen
}

/// What a cell looks like: its text color, its background and its attributes.
fn look_of(term: &TestTerm, (col, row): (usize, usize)) -> (String, String, String) {
    let cell = term.cell(col, row);
    (cell.1, cell.2, term.flags(col, row))
}

/// The looks of what the quick list must leave as it is: lead's text, the dashboard's title, the bar's first cells.
fn untouched(term: &TestTerm) -> Vec<(String, String, String)> {
    let mut cells = Vec::new();
    for (text, len) in [("FAKE CLAUDE", 11), ("Tableau de bord", 15)] {
        if let Some((col, row)) = (0..term.screen().len()).find_map(|r| {
            let line = term.screen()[r].clone();
            line.find(text).map(|at| (unicode_width::UnicodeWidthStr::width(&line[..at]), r))
        }) {
            cells.extend((0..len).map(|i| look_of(term, (col + i, row))));
        }
    }
    let bar = term.screen().len() - 1;
    cells.extend((0..8).map(|i| look_of(term, (i, bar))));
    cells
}

/// What `claude agents` says of a member: `busy`, `idle`, `waiting`.
fn agent_status(team: &Team, member: &str, status: &str) {
    team.ctl(["send", "--pane", member, &format!("agent {status}\\r")]);
}

/// The sign before `name` on its header row on `term` (the frame's top border, not a card of the dashboard).
fn header_sign(term: &TestTerm, name: &str) -> Option<char> {
    let wanted = format!(" {name} ");
    term.screen().iter().find_map(|row| {
        if !row.chars().any(|c| matches!(c, '┏' | '╭')) || !row.contains(['━', '─']) {
            return None;
        }
        row[..row.find(&wanted)?].chars().last()
    })
}

/// Whether `sign` is an image of the spinner (Braille), the sign of a member at work.
fn is_spinner(sign: Option<char>) -> bool {
    sign.is_some_and(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
}

/// Waits until the header of `name` on `term` shows (or no longer shows) a member at work.
fn header_works(term: &TestTerm, name: &str, works: bool, timeout: Duration) -> bool {
    wait(timeout, || is_spinner(header_sign(term, name)) == works)
}

// A failed turn (« tour raté »): Claude Code 2.1.296 writes `state=error:app=claude-code:msg=<base64>` when a turn ends
// on an API error. The state lasts until the member's next one.

const OVERLOADED: &str =
    "API Error: 529 {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}";

/// The bar (the screen's last row).
fn bar_of(term: &TestTerm) -> String {
    term.screen().last().cloned().unwrap_or_default()
}
