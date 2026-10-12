// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the panels draw with: lines of painted pieces, and the lines of a drawn panel with its zones.

use crossterm::style::{Color, Stylize};

use crate::ui::canvas::{Canvas, Style, columns, fit};

use super::zones::*;

/// Lines of the dashboard, and where the members show in them, rows counted from the first line.
#[derive(Debug, Default)]
pub(super) struct Drawn {
    pub(super) lines: Vec<Line>,
    pub(super) zones: Vec<Zone>,
}

impl From<Vec<Line>> for Drawn {
    fn from(lines: Vec<Line>) -> Self {
        Drawn { lines, zones: Vec::new() }
    }
}

impl Drawn {
    pub(super) fn len(&self) -> usize {
        self.lines.len()
    }

    /// `below` under these lines.
    pub(super) fn append(&mut self, below: Drawn) {
        let rows = self.lines.len();
        self.zones.extend(below.zones.into_iter().map(|zone| Zone { row: zone.row + rows, ..zone }));
        self.lines.extend(below.lines);
    }

    /// After an empty line, unless there is nothing.
    pub(super) fn spaced(self) -> Drawn {
        if self.lines.is_empty() {
            return self;
        }
        let mut spaced = Drawn::from(vec![Line::default()]);
        spaced.append(self);
        spaced
    }
}

/// How a piece of a line is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Paint {
    Plain,
    Dim,
    Color(Color),
    Bold(Color),
    /// Colored and dimmed.
    Faint(Color),
}

impl Paint {
    /// As a cell of a canvas is drawn.
    pub(super) fn style(self) -> Style {
        let fg = |color: Color| Style { fg: (color != Color::Reset).then_some(color), ..Style::PLAIN };
        match self {
            Paint::Plain => Style::PLAIN,
            Paint::Dim => Style::PLAIN.dim(),
            Paint::Color(color) => fg(color),
            Paint::Bold(color) => fg(color).bold(),
            Paint::Faint(color) => fg(color).dim(),
        }
    }

    /// `text` as a terminal prints it, in this paint.
    pub(super) fn apply(self, text: &str) -> String {
        match self {
            Paint::Plain => text.to_string(),
            Paint::Dim => text.dim().to_string(),
            Paint::Color(color) => text.with(color).to_string(),
            Paint::Bold(color) => text.with(color).bold().to_string(),
            Paint::Faint(color) => text.with(color).dim().to_string(),
        }
    }
}

/// A line of a panel: pieces of text one after the other, each in its paint.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Line(pub(super) Vec<(String, Paint)>);

impl Line {
    pub(super) fn of(text: impl Into<String>, paint: Paint) -> Line {
        let mut line = Line::default();
        line.push(text, paint);
        line
    }

    pub(super) fn push(&mut self, text: impl Into<String>, paint: Paint) {
        let text = text.into();
        if !text.is_empty() {
            self.0.push((text, paint));
        }
    }

    /// `other` after this one.
    pub(super) fn append(&mut self, other: Line) {
        self.0.extend(other.0);
    }

    /// Its text, without its paints.
    pub(super) fn text(&self) -> String {
        self.0.iter().map(|(text, _)| text.as_str()).collect()
    }

    /// On row `y` of `canvas`, from its first column.
    pub(super) fn draw(&self, canvas: &mut Canvas, y: usize) {
        let mut x = 0;
        for (text, paint) in &self.0 {
            x = canvas.put(x, y, text, paint.style());
        }
    }

    /// On row `y` of `canvas`, from column `x`, cut before column `end`; returns the column after it.
    pub(crate) fn draw_at(&self, canvas: &mut Canvas, x: usize, y: usize, end: usize) -> usize {
        let mut x = x;
        for (text, paint) in &self.0 {
            x = canvas.put_in(x, y, end, text, paint.style());
        }
        x
    }

    /// As a terminal prints it: for the journal, which writes its lines as they come.
    pub(super) fn printed(&self) -> String {
        self.0.iter().map(|(text, paint)| paint.apply(text)).collect()
    }
}

/// `pieces` one after the other, each in its paint, cut to `width` as `fit` cuts their text joined: « … » in the paint
/// of the piece it cuts, and blanks after it up to `width` when the cut leaves the line short (blanks taken off before
/// the ellipsis, a wide character that did not fit).
pub(super) fn painted(pieces: &[(impl AsRef<str>, Paint)], width: usize) -> Line {
    let joined: String = pieces.iter().map(|(text, _)| text.as_ref()).collect();
    let mut line = Line::default();
    if columns(&joined) <= width {
        for (text, paint) in pieces {
            line.push(text.as_ref(), *paint);
        }
        return line;
    }
    let fitted = fit(&joined, width);
    // The characters of the text that stay, then the ellipsis in place of the rest.
    let mut kept = fitted.strip_suffix('…').unwrap_or(&fitted).chars().count();
    for (text, paint) in pieces {
        let text = text.as_ref();
        let size = text.chars().count();
        if kept >= size {
            line.push(text, *paint);
            kept -= size;
            continue;
        }
        line.push(text.chars().take(kept).collect::<String>(), *paint);
        if fitted.ends_with('…') {
            line.push("…", *paint);
        }
        break;
    }
    let short = width.saturating_sub(columns(&line.text()));
    line.push(" ".repeat(short), Paint::Plain);
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::canvas::columns;

    /// Pieces cut to a width in columns: wide characters count two, the ellipsis the last one.
    #[test]
    fn pieces_cut_by_columns() {
        let line = painted(&[("日本語", Paint::Plain), ("日本語", Paint::Dim)], 9);
        assert_eq!(line.text(), "日本語日…");
        assert_eq!(columns(&line.text()), 9);
        // The ellipsis in the paint of the piece it cuts.
        assert_eq!(line.0.last(), Some(&("…".to_string(), Paint::Dim)));
        // Whole when it holds.
        assert_eq!(painted(&[("日本", Paint::Plain)], 4).text(), "日本");
        // Cut at the border of two pieces: the ellipsis takes the paint of the one that follows, not of the one before.
        let line = painted(&[("ab", Paint::Plain), (" cdef", Paint::Dim)], 4);
        assert_eq!(line.text(), "ab… ");
        assert_eq!(line.0[1], ("…".to_string(), Paint::Dim));
        // A wide character that does not fit at the edge: blanks fill the width.
        let line = painted(&[("日本語x", Paint::Plain)], 6);
        assert_eq!((line.text(), columns(&line.text())), ("日本… ".to_string(), 6));
    }
}
