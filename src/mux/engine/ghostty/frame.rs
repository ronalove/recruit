// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the libghostty-vt engine draws: the viewport read into a frame, row by row, only the rows Ghostty says changed,
//! and the frame painted into a canvas. A pane at rest costs no reading.

use anyhow::Result;
use crossterm::style::Color as Paint;
use libghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RenderState, RowIteration, RowIterator, Snapshot};
use libghostty_vt::screen::{Cell, CellContentTag, CellWide};
use libghostty_vt::style::{Palette, RgbColor, StyleColor, Underline};
use libghostty_vt::terminal::{Point, PointCoordinate};

use super::Ghostty;
use crate::ui::Rect;
use crate::ui::canvas::{self, Canvas, Cursor, Style};

/// The 16 system colors, as crossterm names them.
const NAMED: [Paint; 16] = [
    Paint::Black,
    Paint::DarkRed,
    Paint::DarkGreen,
    Paint::DarkYellow,
    Paint::DarkBlue,
    Paint::DarkMagenta,
    Paint::DarkCyan,
    Paint::Grey,
    Paint::DarkGrey,
    Paint::Red,
    Paint::Green,
    Paint::Yellow,
    Paint::Blue,
    Paint::Magenta,
    Paint::Cyan,
    Paint::White,
];

/// The terminal as last drawn, and what draws it.
pub(super) struct View {
    state: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    pub(super) frame: Frame,
    /// A row read again goes here, its buffers reused, then is compared with the one it replaces.
    spare: Row,
}

impl View {
    pub(super) fn new() -> Result<View> {
        Ok(View {
            state: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            frame: Frame { generation: super::super::fresh_generation(), ..Frame::default() },
            spare: Row::default(),
        })
    }
}

/// A frame of the viewport, row by row: a row is read again from Ghostty only when it says the row changed, and
/// nothing at all when nothing did (a pane at rest costs no reading). Shown again as is while a synchronized update
/// holds it.
#[derive(Default)]
pub(super) struct Frame {
    pub(super) rows: Vec<Row>,
    pub(super) cursor: Option<Cursor>,
    /// The default colors the rows were read with (`Colors::fg`, `Colors::bg`): Ghostty marks nothing dirty when the
    /// program changes them, the palette only.
    defaults: (Option<Paint>, Option<Paint>),
    /// `Surface::generation`: a fresh one each time a row read again differs from what it was (Ghostty marks the
    /// cursor's rows dirty when it only moves).
    pub(super) generation: u64,
}

/// A row of the frame.
#[derive(Default, PartialEq)]
pub(super) struct Row {
    /// The cells' graphemes one after the other.
    text: String,
    cells: Vec<Drawn>,
    pub(super) links: Vec<String>,
}

#[derive(PartialEq)]
struct Drawn {
    x: u16,
    /// The grapheme, in `Row::text`.
    start: u32,
    end: u32,
    width: u8,
    /// Its link, in `Row::links`, given to the canvas when drawn.
    link: Option<u32>,
    style: Style,
}

/// The colors a frame is drawn with: the palette, and the default colors if the program changed them.
pub(super) struct Colors {
    palette: Palette,
    standard: Palette,
    fg: Option<Paint>,
    bg: Option<Paint>,
}

/// What reading the rows of a frame shares: the colors, the style of a cell with none of its own, and the buffers
/// reused from one row to the next.
struct Reading<'a> {
    colors: &'a Colors,
    /// A cell without styling of its own takes the default colors, the program's if it changed them.
    plain: Style,
    /// A link's URI read into this buffer; the row's last link is reused while the next cells carry the same (the C
    /// API gives no link id to compare instead).
    uri: Vec<u8>,
    /// Ghostty writes a cell's grapheme from the start of the buffer: one for the cell, then copied.
    grapheme: String,
}

impl Ghostty {
    pub(super) fn colors(&self) -> Colors {
        let (fg, bg) = self.defaults();
        Colors {
            palette: self.term.color_palette().unwrap_or(Palette([RgbColor::default(); 256])),
            standard: self.term.default_color_palette().unwrap_or(Palette([RgbColor::default(); 256])),
            fg,
            bg,
        }
    }

    /// The default foreground and background, if the program changed them.
    fn defaults(&self) -> (Option<Paint>, Option<Paint>) {
        let changed = |now: Option<RgbColor>, default: Option<RgbColor>| {
            now.filter(|now| Some(*now) != default).map(|c| Paint::Rgb { r: c.r, g: c.g, b: c.b })
        };
        (
            changed(self.term.fg_color().ok().flatten(), self.term.default_fg_color().ok().flatten()),
            changed(self.term.bg_color().ok().flatten(), self.term.default_bg_color().ok().flatten()),
        )
    }

    /// The viewport as it is now, into `frame`: the cursor every time, the rows Ghostty says changed since the last
    /// time (all of them after a resize, a scroll, a change of colors).
    pub(super) fn capture(&self, view: &mut View) {
        let View { state, rows, cells, frame, spare } = view;
        let Ok(snapshot) = state.update(&self.term) else { return };
        frame.cursor = cursor_of(&snapshot);
        let height = usize::from(snapshot.rows().unwrap_or(0));
        let dirty = snapshot.dirty().unwrap_or(Dirty::Full);
        let defaults = self.defaults();
        let full = dirty == Dirty::Full || frame.rows.len() != height || frame.defaults != defaults;
        if dirty == Dirty::Clean && !full {
            return;
        }
        let mut changed = frame.rows.len() != height;
        frame.rows.resize_with(height, Row::default);
        let colors = self.colors();
        let mut reading = Reading {
            colors: &colors,
            plain: Style { fg: colors.fg, bg: colors.bg, ..Style::PLAIN },
            uri: Vec::new(),
            grapheme: String::new(),
        };
        let Ok(mut row_it) = rows.update(&snapshot) else { return };
        let mut y = 0u16;
        // Every row read: Ghostty may forget what was dirty.
        let mut complete = true;
        while let Some(row) = row_it.next() {
            let Some(out) = frame.rows.get_mut(usize::from(y)) else { break };
            if full || row.dirty().unwrap_or(true) {
                std::mem::swap(out, spare);
                if !self.read_row(row, cells, out, y, &mut reading) {
                    // The row kept as it was, and read again next time.
                    std::mem::swap(out, spare);
                    complete = false;
                    break;
                }
                let _ = row.set_dirty(false);
                changed |= out != spare;
            }
            y += 1;
        }
        if changed {
            frame.generation = super::super::fresh_generation();
        }
        if complete {
            frame.defaults = defaults;
            let _ = snapshot.set_dirty(Dirty::Clean);
        }
    }

    /// Row `y` of the viewport, cell by cell, into `out` (emptied first, its buffers reused). `false` when Ghostty
    /// cannot give its cells.
    fn read_row(
        &self,
        row: &RowIteration<'static, '_>,
        cells: &mut CellIterator<'static>,
        out: &mut Row,
        y: u16,
        reading: &mut Reading<'_>,
    ) -> bool {
        let Reading { colors, plain, uri, grapheme } = reading;
        out.text.clear();
        out.cells.clear();
        out.links.clear();
        // Whether some cell of the row may have a style or a link (never wrong when false): if not, no cell is asked.
        let flags = row.raw_row().ok();
        let styled = flags.is_none_or(|flags| flags.is_styled().unwrap_or(true));
        let linked = flags.is_none_or(|flags| flags.has_hyperlink().unwrap_or(true));
        let Ok(mut cell_it) = cells.update(row) else { return false };
        let mut x = 0u16;
        while let Some(cell) = cell_it.next() {
            let Ok(raw) = cell.raw_cell() else {
                x += 1;
                continue;
            };
            let wide = raw.wide().unwrap_or(CellWide::Narrow);
            if wide == CellWide::SpacerTail {
                x += 1;
                continue;
            }
            // Most cells are blank and plain: neither their style nor their text is asked for.
            let style = if styled && raw.has_styling().unwrap_or(true) { cell.style().ok() } else { None };
            let start = out.text.len() as u32;
            let has_text = raw.has_text().unwrap_or(true);
            let shown = wide != CellWide::SpacerHead && !style.is_some_and(|style| style.invisible) && has_text;
            if shown && cell.graphemes_utf8(grapheme).is_ok() && !grapheme.is_empty() {
                out.text.push_str(grapheme);
            } else {
                out.text.push(' ');
            }
            let link = (linked && raw.has_hyperlink().unwrap_or(false)).then(|| {
                if uri.is_empty() {
                    uri.resize(4096, 0);
                }
                let grid = self.term.grid_ref(Point::Viewport(PointCoordinate { x, y: u32::from(y) })).ok()?;
                let len = grid.hyperlink_uri(uri).ok()?;
                let read = &uri[..len];
                if out.links.last().is_none_or(|last| last.as_bytes() != read) {
                    out.links.push(String::from_utf8_lossy(read).into_owned());
                }
                Some(out.links.len() as u32 - 1)
            });
            out.cells.push(Drawn {
                x,
                start,
                end: out.text.len() as u32,
                width: if wide == CellWide::Wide { 2 } else { 1 },
                link: link.flatten(),
                style: match style {
                    Some(style) => paint_style(&style, colors),
                    None if !has_text => blank_style(raw, colors, *plain),
                    None => *plain,
                },
            });
            x += 1;
        }
        true
    }
}

/// The cursor as the program left it, in the viewport; `None` when hidden.
fn cursor_of(snapshot: &Snapshot<'static, '_>) -> Option<Cursor> {
    if !snapshot.cursor_visible().unwrap_or(false) {
        return None;
    }
    let Ok(Some(at)) = snapshot.cursor_viewport() else { return None };
    let shape = match snapshot.cursor_visual_style() {
        Ok(CursorVisualStyle::Block) => canvas::CursorShape::Block,
        Ok(CursorVisualStyle::Underline) => canvas::CursorShape::Underline,
        Ok(CursorVisualStyle::Bar) => canvas::CursorShape::Bar,
        _ => canvas::CursorShape::Default,
    };
    let x = if at.at_wide_tail { at.x.saturating_sub(1) } else { at.x };
    Some(Cursor { x: usize::from(x), y: usize::from(at.y), shape, blink: snapshot.cursor_blinking().unwrap_or(false) })
}

/// The style of a blank cell that has none of its own: it may still have a background, from an erase with a
/// background color set (`style()` says nothing of it).
fn blank_style(raw: Cell, colors: &Colors, plain: Style) -> Style {
    match raw.content_tag() {
        Ok(CellContentTag::BgColorPalette) => {
            Style { bg: raw.bg_color_palette().ok().map(|index| palette_paint(index.0, colors)), ..plain }
        }
        Ok(CellContentTag::BgColorRgb) => {
            Style { bg: raw.bg_color_rgb().ok().map(|c| Paint::Rgb { r: c.r, g: c.g, b: c.b }), ..plain }
        }
        _ => plain,
    }
}

impl Frame {
    pub(super) fn paint(&self, canvas: &mut Canvas, area: Rect) {
        for (y, row) in self.rows.iter().enumerate().take(area.height) {
            for cell in &row.cells {
                let x = usize::from(cell.x);
                if x >= area.width {
                    break;
                }
                let mut style = cell.style;
                style.link = cell.link.map(|link| canvas.link(&row.links[link as usize]));
                // Half of a wide character cut by the pane's edge: blank.
                let (text, width) = if cell.width == 2 && x + 1 >= area.width {
                    (" ", 1)
                } else {
                    (&row.text[cell.start as usize..cell.end as usize], usize::from(cell.width))
                };
                canvas.put_cell(area.x + x, area.y + y, text, width, style);
            }
        }
    }
}

/// A cell's style for the canvas: its colors as the real terminal names them (`None` for its own default, so that
/// its theme stays), unless the program changed them.
fn paint_style(style: &libghostty_vt::style::Style, colors: &Colors) -> Style {
    let paint = |color: StyleColor, default: Option<Paint>| match color {
        StyleColor::None => default,
        StyleColor::Rgb(c) => Some(Paint::Rgb { r: c.r, g: c.g, b: c.b }),
        StyleColor::Palette(index) => Some(palette_paint(index.0, colors)),
    };
    Style {
        fg: paint(style.fg_color, colors.fg),
        bg: paint(style.bg_color, colors.bg),
        bold: style.bold,
        dim: style.faint,
        reverse: style.inverse,
        underline: style.underline != Underline::None,
        italic: style.italic,
        strike: style.strikethrough,
        link: None,
    }
}

/// A color of the palette for the canvas: by its index, unless the program changed it.
fn palette_paint(index: u8, colors: &Colors) -> Paint {
    let i = usize::from(index);
    let (now, standard) = (colors.palette.0[i], colors.standard.0[i]);
    if now != standard {
        Paint::Rgb { r: now.r, g: now.g, b: now.b }
    } else if i < NAMED.len() {
        NAMED[i]
    } else {
        Paint::AnsiValue(index)
    }
}
