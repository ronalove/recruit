// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The two palettes recruit picks its colors from, and which one this process draws with.

#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::style::Color;

/// The colors recruit picks itself, for a dark terminal or a light one: all in the 256-color palette. Those the
/// terminal gives (cyan, magenta, blue, red, its greys) stay its own, and follow its theme already.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Palette {
    /// The chosen line of a list, the clicked text of a header.
    pub selected: Color,
    /// The chosen card of the menu.
    pub chosen: Color,
    pub button: Color,
    /// A text edited in place.
    pub input: Color,
    /// What a layer dims around it.
    pub faded: Color,
    /// The members' own colors after the terminal's cyan, magenta and blue (`COLORS`).
    pub members: [Color; 5],
    /// Efforts low, medium, high and xhigh.
    pub low: Color,
    pub medium: Color,
    pub high: Color,
    /// `max`'s rainbow.
    pub rainbow: [Color; 6],
    /// A failed turn, a context near compaction.
    pub orange: Color,
    /// At work: the terminal's yellow on a dark one, too faint on white.
    pub working: Color,
    /// A newer recruit is published.
    pub update: Color,
}

pub(crate) const DARK: Palette = Palette {
    selected: Color::AnsiValue(237),
    chosen: Color::AnsiValue(235),
    button: Color::AnsiValue(236),
    input: Color::AnsiValue(233),
    faded: Color::AnsiValue(239),
    members: [
        Color::AnsiValue(209),
        Color::AnsiValue(147),
        Color::AnsiValue(80),
        Color::AnsiValue(218),
        Color::AnsiValue(111),
    ],
    low: Color::AnsiValue(179),
    medium: Color::AnsiValue(114),
    high: Color::AnsiValue(141),
    rainbow: [
        Color::AnsiValue(203),
        Color::AnsiValue(209),
        Color::AnsiValue(221),
        Color::AnsiValue(114),
        Color::AnsiValue(75),
        Color::AnsiValue(141),
    ],
    orange: Color::AnsiValue(208),
    working: Color::Yellow,
    update: Color::AnsiValue(114),
};

pub(crate) const LIGHT: Palette = Palette {
    selected: Color::AnsiValue(253),
    chosen: Color::AnsiValue(254),
    button: Color::AnsiValue(252),
    input: Color::AnsiValue(255),
    faded: Color::AnsiValue(248),
    members: [
        Color::AnsiValue(167),
        Color::AnsiValue(61),
        Color::AnsiValue(30),
        Color::AnsiValue(162),
        Color::AnsiValue(26),
    ],
    low: Color::AnsiValue(136),
    medium: Color::AnsiValue(28),
    high: Color::AnsiValue(98),
    rainbow: [
        Color::AnsiValue(160),
        Color::AnsiValue(166),
        Color::AnsiValue(136),
        Color::AnsiValue(28),
        Color::AnsiValue(32),
        Color::AnsiValue(98),
    ],
    orange: Color::AnsiValue(166),
    working: Color::AnsiValue(136),
    update: Color::AnsiValue(28),
};

#[cfg(not(test))]
static LIGHT_THEME: AtomicBool = AtomicBool::new(false);

// In the tests, each test thread has its own theme: one that draws in the light palette leaves the others in the dark.
#[cfg(test)]
thread_local! {
    static LIGHT_THEME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// This process's palette: the dark one until told otherwise.
pub(crate) fn palette() -> &'static Palette {
    #[cfg(not(test))]
    let light = LIGHT_THEME.load(Ordering::Relaxed);
    #[cfg(test)]
    let light = LIGHT_THEME.with(std::cell::Cell::get);
    if light { &LIGHT } else { &DARK }
}

/// Whether this process draws with the light palette.
pub(crate) fn is_light() -> bool {
    std::ptr::eq(palette(), &LIGHT)
}

/// Draws with the light palette from now on, or the dark one: the server from its client's background (OSC 11), the
/// menu from `--light`, the panels from the multiplexer's answer to `CSI ? 996 n` and its `CSI ? 997` notices.
pub(crate) fn set_light(light: bool) {
    #[cfg(not(test))]
    LIGHT_THEME.store(light, Ordering::Relaxed);
    #[cfg(test)]
    LIGHT_THEME.with(|theme| theme.set(light));
}

/// Whether a terminal's background (OSC 11) is light: its relative luminance over a half.
pub(crate) fn light_background((r, g, b): (u8, u8, u8)) -> bool {
    let linear = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b) > 0.5
}

/// The `index`-th member's own color, in the order of the team.
pub(crate) fn member_color(index: usize) -> Color {
    // The terminal's own cyan, magenta and blue, then five of ours: no yellow, green or red, which tell the states.
    let own = palette().members;
    let all = [Color::Cyan, Color::Magenta, Color::Blue, own[0], own[1], own[2], own[3], own[4]];
    all[index % all.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_backgrounds() {
        for (bg, light) in [((255, 255, 255), true), ((246, 246, 239), true), ((0, 0, 0), false), ((28, 30, 34), false)]
        {
            assert_eq!(light_background(bg), light, "{bg:?}");
        }
        // The light palette only moves what recruit picks itself.
        assert_eq!((DARK.working, LIGHT.working), (Color::Yellow, Color::AnsiValue(136)));
        assert_ne!(DARK.selected, LIGHT.selected);
    }
}
