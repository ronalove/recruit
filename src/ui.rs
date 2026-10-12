// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What recruit draws and asks on a terminal: the screen drawn cell by cell ([`canvas`]), the looks shared by the
//! dashboard, the menu and the multiplexer ([`look`]), the prompts, in the interface language ([`ask`], with its lists
//! in [`list`]), and what the command line prints ([`print`]).

mod ask;
pub(crate) mod canvas;
mod list;
pub(crate) mod look;
mod print;
pub(crate) mod terminal;

pub(crate) use ask::{Cancelled, Choice, confirm, interactive, multi_select, optional, required, select, text};
pub(crate) use print::{print_paragraph, print_team};

/// A rectangle of the screen, in cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Rect {
    pub(crate) fn contains(&self, col: usize, row: usize) -> bool {
        (self.x..self.x + self.width).contains(&col) && (self.y..self.y + self.height).contains(&row)
    }
}

/// A color as the real terminal reports it (OSC 10, 11).
pub(crate) type Rgb = (u8, u8, u8);
