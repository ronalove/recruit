// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Text measured in columns, as a canvas draws it.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

/// Columns of `text` measured whole, as a terminal draws what it prints: a grapheme counts as one, which `columns`
/// (character by character, as a canvas puts text) does not do.
pub(crate) fn text_columns(text: &str) -> usize {
    text.width()
}

/// What [`wrap`] does with a word longer than a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Long {
    /// Broken where the row ends, the rest on the next rows (at least one character per row, however wide).
    Split,
    /// Alone on a row, which is cut with « … ».
    Ellipsis,
    /// Alone on a row, left as it is: the terminal breaks it.
    Whole,
}

/// How [`wrap`] breaks a text: the word too long for a row, the number of rows, the way a row is measured.
#[derive(Clone, Copy)]
pub(crate) struct Wrap {
    long: Long,
    lines: usize,
    measure: fn(&str) -> usize,
}

impl Wrap {
    /// Rows without limit, measured in canvas columns ([`columns`]).
    pub(crate) fn new(long: Long) -> Wrap {
        Wrap { long, lines: usize::MAX, measure: columns }
    }

    /// At most `lines` rows (one at least): « … » ends the last one when the text goes on.
    pub(crate) fn lines(self, lines: usize) -> Wrap {
        Wrap { lines, ..self }
    }

    /// Rows measured by `measure` instead of in canvas columns (such as [`text_columns`]).
    pub(crate) fn measured(self, measure: fn(&str) -> usize) -> Wrap {
        Wrap { measure, ..self }
    }
}

/// `text` on rows of `width` columns at most, cut between words (any white space between two words becomes one
/// space), as `how` says for a word longer than a row and for the number of rows. One row at least, empty for a text
/// without words.
pub(crate) fn wrap(text: &str, width: usize, how: Wrap) -> Vec<String> {
    let mut chars: Vec<(char, ())> = Vec::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !chars.is_empty() {
            chars.push((' ', ()));
        }
        chars.extend(word.chars().map(|c| (c, ())));
    }
    let mut rows: Vec<String> = wrap_chars(&chars, width, how.long, how.measure)
        .into_iter()
        .map(|row| row.into_iter().map(|(c, _)| c).collect())
        .collect();
    let lines = how.lines.max(1);
    if rows.len() > lines {
        let rest = rows.split_off(lines - 1).join(" ");
        rows.push(fit(&rest, width));
    }
    if how.long == Long::Ellipsis {
        for row in &mut rows {
            *row = fit(row, width);
        }
    }
    rows
}

/// `chars` on rows of `width` columns at most, each character with what the caller tied to it (a color). Cut before
/// a word, `long` saying what a word too long for a row gets. The spaces stay as they are (a label may line up
/// columns with them, a filter may end with one), except where a row breaks. One row at least.
pub(crate) fn wrap_chars<T: Copy>(
    chars: &[(char, T)],
    width: usize,
    long: Long,
    measure: fn(&str) -> usize,
) -> Vec<Vec<(char, T)>> {
    let mut scratch = String::new();
    let mut wide = |parts: &[&[(char, T)]]| {
        scratch.clear();
        scratch.extend(parts.iter().flat_map(|part| part.iter().map(|&(c, _)| c)));
        measure(&scratch)
    };
    let mut rows: Vec<Vec<(char, T)>> = vec![Vec::new()];
    let mut rest = chars;
    while !rest.is_empty() {
        let gap = rest.iter().take_while(|(c, _)| *c == ' ').count();
        let len = rest[gap..].iter().take_while(|(c, _)| *c != ' ').count();
        let (spaces, word) = (&rest[..gap], &rest[gap..gap + len]);
        rest = &rest[gap + len..];
        let row = rows.last_mut().expect("a row");
        let mut fits = wide(&[row.as_slice(), spaces, word]) <= width;
        if fits {
            row.extend(spaces);
        } else {
            if !row.is_empty() && !word.is_empty() {
                rows.push(Vec::new());
            }
            // On a row of its own, with the spaces left behind.
            fits = wide(&[word]) <= width;
        }
        // A word that fits goes in whole, measured once: character by character, a prefix of it could be wider than
        // the word (a wide character before its text-presentation selector).
        if long != Long::Split || fits {
            rows.last_mut().expect("a row").extend(word);
            continue;
        }
        for &c in word {
            let row = rows.last_mut().expect("a row");
            row.push(c);
            if row.len() > 1 && wide(&[row.as_slice()]) > width {
                row.pop();
                rows.push(vec![c]);
            }
        }
    }
    rows
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
