// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Text measured in columns, as a canvas draws it.

use unicode_width::UnicodeWidthChar;

/// Columns of `text` as a canvas draws it.
pub(crate) fn columns(text: &str) -> usize {
    text.chars().map(|c| width(c).unwrap_or(0)).sum()
}

/// Columns of `c` as a canvas draws it: none for what it leaves out ([`detached`]), some of which unicode-width
/// gives one; `None` for a control character.
pub(super) fn width(c: char) -> Option<usize> {
    if detached(c) { Some(0) } else { c.width() }
}

/// `text` in `room` columns at most, with « … » when it is cut.
pub(crate) fn fit(text: &str, room: usize) -> String {
    if columns(text) <= room {
        return text.to_string();
    }
    let mut cut = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = width(c).unwrap_or(0);
        if used + w + 1 > room {
            break;
        }
        cut.push(c);
        used += w;
    }
    if room > 0 {
        cut.truncate(cut.trim_end().len());
        cut.push('…');
    }
    cut
}

/// The characters of no width that [`Canvas::put`] leaves out rather than add to the character before: joiners and
/// non-joiners, variation selectors, direction marks and controls, invisible operators, format characters and tags.
pub(super) fn detached(c: char) -> bool {
    is_direction_mark(c)
        || matches!(c,
            '\u{ad}'
            | '\u{61c}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2060}'..='\u{2064}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e007f}'
            | '\u{e0100}'..='\u{e01ef}')
}

/// The controls of the writing direction (embeddings, overrides, isolates): a terminal that follows them would reorder
/// what a pane shows.
pub(crate) fn is_direction_mark(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// What [`Canvas::put_cell`] leaves out of a grapheme `width` columns wide: control characters and direction
/// controls; VS16 when it is one column wide, since the terminals that heed it draw the character on two.
pub(super) fn left_out(c: char, width: usize) -> bool {
    c.is_control() || is_direction_mark(c) || (c == '\u{fe0f}' && width < 2)
}
