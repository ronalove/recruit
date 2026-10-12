// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn styles_say_only_what_changes() {
    let mut painter = Painter::default();
    let mut out = Vec::new();
    let canvas = Canvas::new(0, 0);
    painter.style = Some(Style::PLAIN);
    painter.sgr(&mut out, Style::fg(Color::AnsiValue(141)).bold(), &canvas);
    painter.sgr(&mut out, Style::fg(Color::AnsiValue(141)).bold().on(Color::AnsiValue(236)), &canvas);
    painter.sgr(&mut out, Style::fg(Color::Yellow), &canvas);
    painter.sgr(&mut out, Style::fg(Color::Yellow), &canvas);
    assert_eq!(text(&out), "\x1b[1;38;5;141m\x1b[48;5;236m\x1b[0;93m");
    // Italic and struck through: on, then off from scratch.
    let mut out = Vec::new();
    painter.sgr(&mut out, Style { italic: true, strike: true, ..Style::fg(Color::Yellow) }, &canvas);
    painter.sgr(&mut out, Style { strike: true, ..Style::fg(Color::Yellow) }, &canvas);
    assert_eq!(text(&out), "\x1b[3;9m\x1b[0;9;93m");
}

#[test]
fn colors_for_a_terminal_without_24_bits() {
    assert_eq!(ansi256(0, 0, 0), 16);
    assert_eq!(ansi256(0xff, 0xff, 0xff), 231);
    assert_eq!(ansi256(0xaf, 0x87, 0xff), 141);
    assert_eq!(ansi256(0x80, 0x80, 0x80), 244);
    assert_eq!(ansi256(0xd7, 0x77, 0x57), 173);
    let mut painter = Painter::new(Features { truecolor: false, ..Features::default() });
    let mut canvas = Canvas::new(1, 1);
    canvas.put(0, 0, "x", Style::fg(Color::Rgb { r: 0xaf, g: 0x87, b: 0xff }));
    assert!(text(&painter.frame(&canvas)).contains("\x1b[38;5;141mx"));
    let mut painter = Painter::default();
    assert!(text(&painter.frame(&canvas)).contains("\x1b[38;2;175;135;255mx"));
}
