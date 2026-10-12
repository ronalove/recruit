// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Where a click on the dashboard or the journal lands: the zones the dashboard last drew, and the member or the
//! compaction a click reaches.

use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::sys::write_atomic;
use crate::team::state::Snapshot;

use super::journal::*;
use super::*;

/// Where a member shows on the dashboard, its card or its name in a list: a click there leads to its pane; or the
/// context of a member at rest, which a click offers to compact. Rows and columns of the pane, from 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Zone {
    pub(super) member: String,
    #[serde(default)]
    pub(super) kind: ZoneKind,
    pub(super) row: usize,
    pub(super) col: usize,
    pub(super) rows: usize,
    pub(super) cols: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ZoneKind {
    #[default]
    Member,
    Compact,
}

impl Zone {
    pub(super) fn contains(&self, x: usize, y: usize) -> bool {
        (self.col..self.col + self.cols).contains(&x) && (self.row..self.row + self.rows).contains(&y)
    }
}

/// The zones as the dashboard last drew them, in the team's folder.
pub const ZONES: &str = "zones.json";

/// Written in one go: a click never reads half a file.
pub(super) fn save_zones(state: &Path, zones: &[Zone]) -> Result<()> {
    write_atomic(&state.join(ZONES), &serde_json::to_string(zones)?)
}

pub(super) fn zones(state: &Path) -> Vec<Zone> {
    fs::read(state.join(ZONES)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// The member under a click on a panel, at column `x` and row `y` of its pane from 0, the pane `columns` wide, `line`
/// the clicked line as the pane shows it: on the dashboard, a card or a name in a list, where the dashboard last drew
/// them; in the journal, the sender or the recipient on a message's first line, read from `line` alone (it may be
/// scrolled back).
pub fn member_at(
    s: &Snapshot,
    state: &Path,
    panel: Kind,
    x: usize,
    y: usize,
    columns: usize,
    line: &str,
) -> Option<String> {
    let name = match panel {
        Kind::Dashboard => {
            zones(state).into_iter().find(|zone| zone.kind == ZoneKind::Member && zone.contains(x, y))?.member
        }
        Kind::Journal => journal_name_at(line, x, columns)?.to_string(),
    };
    s.members.iter().any(|m| m.name == name).then_some(name)
}

/// The member at rest whose context a click on the dashboard falls on, at column `x` and row `y` of its pane: the
/// click offers to compact it. Asked before `member_at`, whose card holds the same place.
pub fn compaction_at(s: &Snapshot, state: &Path, x: usize, y: usize) -> Option<String> {
    let name = zones(state).into_iter().find(|zone| zone.kind == ZoneKind::Compact && zone.contains(x, y))?.member;
    s.members.iter().any(|m| m.name == name).then_some(name)
}

/// What a click on a panel reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clicked {
    /// A member: its pane, in its tab.
    Show(String),
    /// A member at rest, by its context on the dashboard: compacting it is offered, after confirmation.
    Compact(String),
}

/// What a click on a panel reaches, as recruit's own multiplexer (the server) asks: see [`compaction_at`], asked
/// first, then [`member_at`].
pub fn clicked(
    s: &Snapshot,
    state: &Path,
    panel: Kind,
    x: usize,
    y: usize,
    columns: usize,
    line: &str,
) -> Option<Clicked> {
    if panel == Kind::Dashboard
        && let Some(member) = compaction_at(s, state, x, y)
    {
        return Some(Clicked::Compact(member));
    }
    member_at(s, state, panel, x, y, columns, line).map(Clicked::Show)
}

#[cfg(test)]
mod tests {

    use std::fs;

    use std::path::Path;

    use crossterm::style::Color;

    use crate::claude::bridge::Task;

    use crate::ui::look::State;

    use super::*;

    use crate::board::arrange::*;

    use crate::board::fixture::*;

    use crate::board::paint::*;

    use crate::board::{Board, Card, Kind};
    use crate::ui::look::ALT;

    #[test]
    fn zones_over_cards_of_different_heights() {
        // One column: four lines, an empty one, three.
        let drawn = padded(&board(long_and_short()), 70, 15);
        zones_match(&drawn);
        let rows = |drawn: &Drawn| -> Vec<(String, ZoneKind, usize, usize, usize)> {
            drawn.zones.iter().map(|z| (z.member.clone(), z.kind, z.row, z.col, z.rows)).collect()
        };
        assert_eq!(
            rows(&drawn)[..3],
            [
                ("coordinateur".into(), ZoneKind::Member, 2, 0, 4),
                ("dev".into(), ZoneKind::Member, 7, 0, 3),
                ("dev".into(), ZoneKind::Compact, 9, 63, 1),
            ]
        );
        // Side by side, two at work: each its own height, the shorter one's curve on its own last line.
        let pair = || {
            let task = Some(Task { now: "Je relis le clic".into(), done: None });
            vec![
                sample_card("relecteur", false, State::Working).tweak(|c| {
                    c.data.doing = task;
                }),
                sample_card("dev", false, State::Working),
            ]
        };
        let drawn = padded(&board(pair()), 70, 10);
        zones_match(&drawn);
        assert_eq!(
            rows(&drawn)[..2],
            [("relecteur".into(), ZoneKind::Member, 2, 0, 3), ("dev".into(), ZoneKind::Member, 2, 35, 2)]
        );
        // The shorter one on the left: room kept under it, the right one's capsule in its column on all its lines.
        let mut cards = pair();
        cards.reverse();
        let drawn = padded(&board(cards), 70, 10);
        zones_match(&drawn);
        assert_eq!(rows(&drawn)[0], ("dev".into(), ZoneKind::Member, 2, 0, 2));
        assert_eq!(rows(&drawn)[1], ("relecteur".into(), ZoneKind::Member, 2, 35, 3));
        let capsule: Vec<Option<char>> = drawn.lines[2..5].iter().map(|l| visible(l).chars().nth(35)).collect();
        assert_eq!(capsule, [Some('╻'), Some('┃'), Some('╹')]);
        assert_eq!(visible(&drawn.lines[4]).chars().position(|c| c == '╹'), Some(35));
    }

    #[test]
    fn zones_over_cards_and_names() {
        let b = board(vec![
            sample_card("coordinateur", true, State::Working),
            sample_card("lanceur", false, State::Idle),
            sample_card("interface", false, State::Waiting),
        ]);
        let drawn = padded(&b, 70, 23);
        zones_match(&drawn);
        let at =
            |member: &str, kind| drawn.zones.iter().find(|z| z.member == member && z.kind == kind).cloned().unwrap();
        let zone =
            |member: &str, kind, row, col, rows, cols| Zone { member: member.into(), kind, row, col, rows, cols };
        assert_eq!(at("coordinateur", ZoneKind::Member), zone("coordinateur", ZoneKind::Member, 2, 0, 2, 70));
        assert_eq!(at("interface", ZoneKind::Member), zone("interface", ZoneKind::Member, 5, 0, 2, 70));
        // The context of a member at rest, to compact it.
        assert_eq!(at("lanceur", ZoneKind::Compact), zone("lanceur", ZoneKind::Compact, 9, 63, 1, 6));
        assert_eq!(drawn.zones.iter().filter(|z| z.kind == ZoneKind::Compact).count(), 1);
        // An absent member is named in a list, after its label.
        assert_eq!(at("ops", ZoneKind::Member), zone("ops", ZoneKind::Member, 11, 10, 1, 3));

        // Two side by side, the oldest agents at rest named.
        let mut cards =
            vec![sample_card("coordinateur", true, State::Idle), sample_card("archi", true, State::Working)];
        cards.extend((0..4).map(|i| sample_card(&format!("w{i}"), false, State::Working)));
        cards.extend((0..9).map(|i| sample_card(&format!("repos{i}"), false, State::Idle)));
        let drawn = padded(&board(cards), 70, 23);
        zones_match(&drawn);
        let archi = drawn.zones.iter().find(|z| z.member == "archi").unwrap();
        assert_eq!((archi.row, archi.col, archi.cols), (2, 35, 35));
        let w1 = drawn.zones.iter().find(|z| z.member == "w1").unwrap();
        assert_eq!((w1.row, w1.col, w1.cols), (5, 35, 35));
        let named =
            drawn.zones.iter().filter(|z| z.member.starts_with("repos") && z.rows == 1 && z.kind == ZoneKind::Member);
        let named: Vec<(&str, usize)> = named.map(|z| (z.member.as_str(), z.col)).collect();
        assert_eq!(named, [("repos4", 10), ("repos5", 19), ("repos6", 28), ("repos7", 37), ("repos8", 46)]);
    }

    #[test]
    fn zones_follow_what_moves_them() {
        let cards = || vec![sample_card("coordinateur", true, State::Working), sample_card("dev", false, State::Idle)];
        let rows = |b: &Board| padded(b, 70, 23).zones.iter().map(|z| z.row).collect::<Vec<_>>();
        // An error first, then a blank: everything two lines lower.
        let failing = Board { error: Some("claude agents: boom".into()), ..board(cards()) };
        zones_match(&padded(&failing, 70, 23));
        assert_eq!(rows(&failing), rows(&board(cards())).iter().map(|r| r + 2).collect::<Vec<_>>());

        // Two side by side on an odd width: the right one a column wider.
        let mut many: Vec<Card> = (0..6).map(|i| sample_card(&format!("w{i}"), false, State::Working)).collect();
        many.extend((0..4).map(|i| sample_card(&format!("v{i}"), false, State::Waiting)));
        let drawn = padded(&board(many), 71, 23);
        zones_match(&drawn);
        assert!(drawn.lines.iter().all(|l| visible(l).chars().count() <= 71));
        let w1 = drawn.zones.iter().find(|z| z.member == "w1").unwrap();
        assert_eq!((w1.col, w1.cols), (35, 36));

        // A name wider than its line: cut, its zone over what shows.
        let list = vec!["court".to_string(), "un-nom-bien-trop-long".to_string()];
        let drawn = names("au repos", &list, 20, Color::DarkGrey, usize::MAX);
        let lines: Vec<String> = drawn.lines.iter().map(Line::text).collect();
        assert_eq!(lines, ["au repos  court", "          un-nom-bi…"]);
        zones_match(&drawn);
        let cut = Zone { member: list[1].clone(), kind: ZoneKind::Member, row: 1, col: 10, rows: 1, cols: 10 };
        assert_eq!(drawn.zones[1], cut);
    }

    #[test]
    fn zone_bounds() {
        let zone = Zone { member: "dev".into(), kind: ZoneKind::Member, row: 2, col: 35, rows: 3, cols: 35 };
        assert!(zone.contains(35, 2) && zone.contains(69, 4));
        // Beside the card, above and under it.
        assert!(!zone.contains(34, 2) && !zone.contains(70, 2) && !zone.contains(35, 1) && !zone.contains(35, 5));
    }

    #[test]
    fn click_on_the_dashboard() {
        let dir = tempfile::tempdir().unwrap();
        let s = snapshot(&["coordinateur", "dev"]);
        let at = |x, y| member_at(&s, dir.path(), Kind::Dashboard, x, y, 70, "");
        // Nothing drawn yet.
        assert_eq!(at(0, 2), None);
        let zone =
            |member: &str, kind, row, col, rows, cols| Zone { member: member.into(), kind, row, col, rows, cols };
        let zones = vec![
            zone("coordinateur", ZoneKind::Member, 2, 0, 3, 35),
            zone("dev", ZoneKind::Member, 2, 35, 3, 35),
            zone("dev", ZoneKind::Compact, 4, 62, 1, 6),
            zone("parti", ZoneKind::Member, 6, 10, 1, 5),
        ];
        save_zones(dir.path(), &zones).unwrap();
        assert_eq!(crate::board::zones::zones(dir.path()), zones);
        assert_eq!(at(0, 2).as_deref(), Some("coordinateur"));
        assert_eq!(at(40, 4).as_deref(), Some("dev"));
        // The header, past the cards, and a name that is no member.
        assert_eq!(at(3, 0), None);
        assert_eq!(at(3, 5), None);
        assert_eq!(at(12, 6), None);
        // On the context of a member at rest: compacting it, not going to it; elsewhere on its card, no compacting.
        assert_eq!(compaction_at(&s, dir.path(), 63, 4).as_deref(), Some("dev"));
        assert_eq!(at(63, 4).as_deref(), Some("dev"));
        assert_eq!(compaction_at(&s, dir.path(), 40, 4), None);
        // Only the file is left, no partial one.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn click_on_the_journal() {
        let s = snapshot(&["coordinateur", "dev", "dev-cli", "rédac"]);
        let within =
            |line: &str, x, columns| member_at(&s, Path::new("/nonexistent"), Kind::Journal, x, 0, columns, line);
        let at = |line: &str, x| within(line, x, 80);
        let line = format!(" 14:32  dev-cli {ARROW} dev");
        let names: Vec<Option<String>> = (0..24).map(|x| at(&line, x)).collect();
        let expected = |x: usize| match x {
            8..15 => Some("dev-cli".to_string()),
            20..23 => Some("dev".to_string()),
            _ => None,
        };
        assert_eq!(names, (0..24).map(expected).collect::<Vec<_>>());
        // A name in another: the whole one under the click.
        let line = format!(" 14:32  dev {ARROW} dev-cli   ");
        assert_eq!(at(&line, 10).as_deref(), Some("dev"));
        assert_eq!(at(&line, 16).as_deref(), Some("dev-cli"));
        assert_eq!(at(&line, 23), None);
        // Columns, not bytes: `é` takes two bytes and one column.
        let line = format!(" 09:05  rédac {ARROW} coordinateur");
        assert_eq!(at(&line, 12).as_deref(), Some("rédac"));
        assert_eq!(at(&line, 13), None);
        assert_eq!(at(&line, 18).as_deref(), Some("coordinateur"));
        // A recipient that is no member: a process nobody was found for.
        assert_eq!(at(&format!(" 09:05  dev {ARROW} uds:/tmp/cc-socks/42.sock"), 16), None);
        // Cut to a narrow journal rather than wrapped: the cut name is no one's.
        let head = visible(&message_head(&s, "14:32", "coordinateur", "dev-cli", 28));
        assert_eq!(head, format!(" 14:32  coordinateur {ARROW} de…"));
        assert_eq!(at(&head, 8).as_deref(), Some("coordinateur"));
        assert!((20..28).all(|x| at(&head, x).is_none()));
        let wide = visible(&message_head(&s, "14:32", "coordinateur", "dev-cli", 40));
        assert_eq!(at(&wide, 25).as_deref(), Some("dev-cli"));
        // Written 40 wide, then wrapped again by the pane's terminal narrowed to 28: `dev` up to the last column, `-cli` on
        // the next line. The recipient may go on: no one; the sender still answers.
        let wrapped = format!(" 14:32  coordinateur {ARROW} dev");
        assert_eq!(wrapped.chars().count(), 28);
        assert!((25..28).all(|x| within(&wrapped, x, 28).is_none()));
        assert_eq!(within(&wrapped, 8, 28).as_deref(), Some("coordinateur"));
        // A column left after it: the whole name.
        assert_eq!(within(&wrapped, 25, 29).as_deref(), Some("dev"));
        assert_eq!(within(&wrapped, 27, 29).as_deref(), Some("dev"));
        // Not a message's first line: its text, the title, or no time in front.
        assert!((0..40).all(|x| at("        ╰─ dev-cli asks dev ──▶ coordinateur", x).is_none()));
        assert!((0..30).all(|x| at(&format!(" Journal          {ALT}j masque"), x).is_none()));
        assert!((0..20).all(|x| at(&format!(" ab:cd  dev {ARROW} dev-cli"), x).is_none()));
    }
}
