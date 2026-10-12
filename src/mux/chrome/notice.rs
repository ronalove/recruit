// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The notice at the top right when a member out of sight waits or failed its turn.

use crossterm::style::Color;

use super::Look;
use crate::t;
use crate::ui::Rect;
use crate::ui::canvas::{Canvas, Style, columns, fit};
use crate::ui::look::{ALT, Glyphs, State};

/// A member gone waiting out of sight, for six seconds in a layer at the top right, over the rest without dimming it
/// « ⚑ dev-saisie attend ta réponse · Agents (1)   ⌥g y aller ». A click on it goes to the member.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notice {
    pub member: String,
    /// Why it is shown: `Waiting` (⚑, red) or `Error` (✗, orange).
    pub state: State,
    /// The member's color, for its name.
    pub color: Option<Color>,
    /// What it says, from [`notice_text`].
    pub text: String,
    /// The tab the member is in, dimmed after the text.
    pub tab: String,
}

/// What a notice says of `member`.
pub(crate) fn notice_text(member: &str) -> String {
    t!("{} attend ta réponse", "{} needs your answer", member)
}

/// What a notice says of `members` whose turns failed within a few seconds, the last one's reason `label`
/// (`look::error_label`) without its code: « dev-saisie a raté son tour : API surchargée », « 3 membres ont raté leur
/// tour : API surchargée ».
pub(crate) fn error_notice_text(members: &[&str], label: &str) -> String {
    let reason = crate::ui::look::without_code(label);
    match members {
        [member] => t!("{} a raté son tour : {}", "{}'s turn failed: {}", member, reason),
        _ => t!("{} membres ont raté leur tour : {}", "{} members' turns failed: {}", members.len(), reason),
    }
}

/// Columns between a notice and the screen's right edge.
const NOTICE_MARGIN: usize = 2;

/// What a notice says inside its frame, in `room` columns: « ⚑ dev-saisie attend ta réponse · Agents (1)   ⌥g y
/// aller », its name in its color; « ✗ … » in orange for a failed turn. Short of room, ⌥g goes, then the tab, then the
/// text is cut.
fn notice_pieces(notice: &Notice, glyphs: Glyphs, room: usize) -> Vec<(String, Style)> {
    let state = notice_state(notice);
    let sign = format!(" {} ", state.icon(glyphs, 0));
    let mut said = vec![(sign, Style::fg(state.color()).bold())];
    match notice.text.strip_prefix(notice.member.as_str()).filter(|_| !notice.member.is_empty()) {
        Some(rest) => {
            let name = notice.color.map_or(Style::PLAIN, Style::fg).bold();
            said.extend([(notice.member.clone(), name), (rest.to_string(), Style::PLAIN.bold())]);
        }
        None => said.push((notice.text.clone(), Style::PLAIN.bold())),
    }
    let tab = (!notice.tab.is_empty()).then(|| (format!(" · {}", notice.tab), Style::PLAIN.dim()));
    let go = [
        ("   ".to_string(), Style::PLAIN),
        (format!("{ALT}g"), Style::fg(Color::DarkCyan)),
        (format!(" {}", t!("y aller", "go")), Style::PLAIN.dim()),
    ];
    let end = (" ".to_string(), Style::PLAIN);
    let forms: [Vec<(String, Style)>; 3] = [
        said.iter().cloned().chain(tab.clone()).chain(go).chain([end.clone()]).collect(),
        said.iter().cloned().chain(tab).chain([end.clone()]).collect(),
        said.iter().cloned().chain([end.clone()]).collect(),
    ];
    let width = |pieces: &[(String, Style)]| pieces.iter().map(|(text, _)| columns(text)).sum::<usize>();
    if let Some(form) = forms.iter().find(|form| width(form) <= room) {
        return form.clone();
    }
    // Cut: the sign, then what is left of the text, a blank at the end.
    let mut left = room.saturating_sub(1);
    let mut out = Vec::new();
    for (text, style) in &forms[2][..forms[2].len() - 1] {
        if left == 0 {
            break;
        }
        let cut = if columns(text) <= left { text.clone() } else { fit(text, left) };
        left -= columns(&cut);
        out.push((cut, *style));
    }
    out.push(end);
    out
}

/// What a notice is for: a failed turn (orange, ✗), else a member waiting (red, ⚑).
fn notice_state(notice: &Notice) -> State {
    if notice.state == State::Error { State::Error } else { State::Waiting }
}

/// Where `notice` goes on a screen whose team shows in `screen` (the bar left out): three rows at the top right, on
/// the panes' first row, two columns from the edge; no wider than the screen allows, empty on a
/// screen too small for it.
pub(crate) fn notice_area(screen: Rect, notice: &Notice) -> Rect {
    let room = screen.width.saturating_sub(NOTICE_MARGIN + 2);
    if screen.height < 4 || room < 8 {
        return Rect { x: screen.x, y: screen.y, width: 0, height: 0 };
    }
    // Its size does not depend on the glyphs: the sign is one column wide either way.
    let inner: usize = notice_pieces(notice, Glyphs::Unicode, room).iter().map(|(text, _)| columns(text)).sum();
    let width = inner + 2;
    Rect { x: screen.x + screen.width - NOTICE_MARGIN - width, y: screen.y + 1, width, height: 3 }
}

/// Draws `notice` in `area` (from [`notice_area`]), over what the screen drew, which stays as it is around it: a red
/// frame (orange for a failed turn), the text inside. A click anywhere in `area` goes to the member.
pub(crate) fn notice(canvas: &mut Canvas, area: Rect, notice: &Notice, look: Look) {
    let Rect { x, y, width, height } = area;
    if width < 3 || height < 3 {
        return;
    }
    canvas.frame(x, y, width, height, Style::fg(notice_state(notice).color()));
    let end = x + width - 1;
    let mut at = x + 1;
    for (text, style) in notice_pieces(notice, look.glyphs, width - 2) {
        at = canvas.put_in(at, y + 1, end, &text, style);
    }
}
