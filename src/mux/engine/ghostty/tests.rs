// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Tests of the libghostty-vt engine: what it answers, relays, draws and counts.

use crossterm::style::Color as Paint;

use super::*;
use crate::ui::canvas;

fn engine() -> Ghostty {
    Ghostty::new(20, 5, 100).unwrap()
}

/// What the engine answers to `bytes`.
fn answer(engine: &mut Ghostty, bytes: &[u8]) -> String {
    let mut replies = Vec::new();
    engine.feed(bytes, &mut replies);
    String::from_utf8(replies).unwrap()
}

fn rows(engine: &Ghostty) -> Vec<String> {
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    (0..5).map(|y| canvas.row(y)).collect()
}

fn relays(engine: &mut Ghostty) -> Vec<Relay> {
    let mut relays = Vec::new();
    engine.relays(&mut relays);
    relays
}

#[test]
fn queries_are_answered() {
    let mut engine = engine();
    assert_eq!(answer(&mut engine, b"\x1b[c"), "\x1b[?62;22c");
    assert_eq!(answer(&mut engine, b"\x1b[5n"), "\x1b[0n");
    assert_eq!(answer(&mut engine, b"\x1b[3;7H\x1b[6n"), "\x1b[3;7R");
    // Synchronized updates known, graphemes measured whole (set by the engine).
    assert_eq!(answer(&mut engine, b"\x1b[?2026$p"), "\x1b[?2026;2$y");
    assert_eq!(answer(&mut engine, b"\x1b[?2027$p"), "\x1b[?2027;1$y");
    // The kitty keyboard protocol: none, then pushed, then popped.
    assert_eq!(answer(&mut engine, b"\x1b[?u"), "\x1b[?0u");
    assert_eq!(answer(&mut engine, b"\x1b[>1u\x1b[?u"), "\x1b[?1u");
    assert_eq!(engine.modes().kitty, 1);
    answer(&mut engine, b"\x1b[<u");
    assert_eq!(engine.modes().kitty, 0);
    // Claude Code's probe, together: XTVERSION, kitty, program status, then DA1 last, which ends it.
    let version = format!("\x1bP>|recruit {}\x1b\\", env!("CARGO_PKG_VERSION"));
    assert_eq!(
        answer(&mut engine, b"\x1b[>0q\x1b[?u\x1b]7501;?\x1b\\\x1b[c"),
        format!("{version}\x1b[?0u\x1b]7501;?\x1b\\\x1b[?62;22c")
    );
    assert_eq!(answer(&mut engine, b"\x1b[18t"), "\x1b[8;5;20t");
}

#[test]
fn colors_are_the_real_terminals() {
    let mut engine = engine();
    assert!(answer(&mut engine, b"\x1b]11;?\x1b\\").contains("11;rgb:0000/0000/0000"));
    assert_eq!(answer(&mut engine, b"\x1b[?996n"), "\x1b[?997;1n");
    engine.set_colors(Some((0x10, 0x20, 0x30)), Some((0xfa, 0xfb, 0xfc)));
    assert!(answer(&mut engine, b"\x1b]10;?\x1b\\").contains("10;rgb:1010/2020/3030"));
    assert!(answer(&mut engine, b"\x1b]11;?\x1b\\").contains("11;rgb:fafa/fbfb/fcfc"));
    assert_eq!(answer(&mut engine, b"\x1b[?996n"), "\x1b[?997;2n");
    // What the program sets wins, and is drawn.
    answer(&mut engine, b"\x1b]11;rgb:01/02/03\x1b\\ ");
    assert!(answer(&mut engine, b"\x1b]11;?\x1b\\").contains("11;rgb:0101/0202/0303"));
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    assert_eq!(canvas.style(0, 0).bg, Some(Paint::Rgb { r: 1, g: 2, b: 3 }));
}

const DARK_REPORT: &str = "\x1b[?997;1n";
const LIGHT_REPORT: &str = "\x1b[?997;2n";

/// What a program that asks for the theme (`CSI ? 996 n`) is told.
fn theme(engine: &mut Ghostty) -> &'static str {
    match answer(engine, b"\x1b[?996n").as_str() {
        DARK_REPORT => "dark",
        LIGHT_REPORT => "light",
        other => panic!("not a color scheme report: {other:?}"),
    }
}

/// The theme asked for is that of the client attached now: a second client, with another background, is not
/// answered with the first one's.
#[test]
fn the_theme_follows_the_client_attached_now() {
    let mut engine = engine();
    let (dark, light, grey) = (Some((0x1c, 0x1e, 0x22)), Some((0xf6, 0xf6, 0xef)), Some((0x80, 0x80, 0x80)));
    assert_eq!(theme(&mut engine), "dark", "no client colors yet");
    engine.set_colors(None, light);
    assert_eq!(theme(&mut engine), "light");
    engine.set_colors(None, dark);
    assert_eq!(theme(&mut engine), "dark");
    engine.set_colors(None, light);
    assert_eq!(theme(&mut engine), "light");
    // A client that does not tell its background: dark, whoever came before.
    engine.set_colors(None, None);
    assert_eq!(theme(&mut engine), "dark");
    // The interface's own measure (look::light_background), so that a program and the interface agree: a mid
    // grey is dark, not light.
    engine.set_colors(None, grey);
    assert_eq!(theme(&mut engine), "dark");
    // What the program sets for itself (OSC 11) is not the terminal's theme.
    engine.set_colors(None, light);
    answer(&mut engine, b"\x1b]11;rgb:00/00/00\x1b\\");
    assert_eq!(theme(&mut engine), "light");
}

#[test]
fn a_theme_change_is_told_to_a_program_that_asked() {
    let mut engine = engine();
    let told = |engine: &mut Ghostty, light| {
        let mut replies = Vec::new();
        engine.color_scheme(light, &mut replies);
        String::from_utf8(replies).unwrap()
    };
    // Mode 2031 off: nothing is said, but the theme asked for later is the new one.
    assert_eq!(told(&mut engine, true), "");
    assert_eq!(theme(&mut engine), "light");
    // On: a change is told, in the form of the answer to the query; the same theme again is not.
    assert_eq!(answer(&mut engine, b"\x1b[?2031h"), "");
    assert_eq!(told(&mut engine, false), DARK_REPORT);
    assert_eq!(told(&mut engine, false), "");
    assert_eq!(told(&mut engine, true), LIGHT_REPORT);
    assert_eq!(told(&mut engine, true), "");
    assert_eq!(theme(&mut engine), "light");
    // Off again: nothing is said, and the program keeps what it was last told (light); on again, the next call
    // tells it where the theme stands.
    answer(&mut engine, b"\x1b[?2031l");
    assert_eq!(told(&mut engine, false), "");
    answer(&mut engine, b"\x1b[?2031h");
    assert_eq!(told(&mut engine, false), DARK_REPORT);
    // Its own query counts as being told: it learns the theme that changed while the mode was off.
    answer(&mut engine, b"\x1b[?2031l");
    assert_eq!(told(&mut engine, true), "");
    assert_eq!(theme(&mut engine), "light");
    answer(&mut engine, b"\x1b[?2031h");
    assert_eq!(told(&mut engine, true), "");
    // A reset turns the mode off.
    assert_eq!(told(&mut engine, false), DARK_REPORT);
    answer(&mut engine, b"\x1bc");
    assert_eq!(told(&mut engine, true), "");
}

/// The theme a program was told is not told to the next one, in the same pane: a reset (RIS), or `recruit _member`
/// saying its Claude stopped, makes the pane start from nothing.
#[test]
fn a_new_program_is_told_the_theme_its_predecessor_knew() {
    let told = |engine: &mut Ghostty, light| {
        let mut replies = Vec::new();
        engine.color_scheme(light, &mut replies);
        String::from_utf8(replies).unwrap()
    };
    let ends: [(&str, &[u8]); 2] = [("reset", b"\x1bc"), ("recruit's clear", b"\x1b]7501;state=clear:app=recruit\x07")];
    for (name, end) in ends {
        let mut engine = engine();
        // A Claude that asks for statuses and for the theme, and is told.
        answer(&mut engine, b"\x1b]7501;?\x1b\\\x1b[?2031h");
        assert_eq!(told(&mut engine, true), LIGHT_REPORT, "{name}");
        assert_eq!(told(&mut engine, true), "", "{name}");
        answer(&mut engine, end);
        // Its successor turns the mode on without asking: it is told, once, at that point.
        assert_eq!(answer(&mut engine, b"\x1b[?2031h"), LIGHT_REPORT, "{name}");
        assert_eq!(told(&mut engine, true), "", "{name}");
    }
    // A status that is not recruit's clear leaves it as it was.
    let mut engine = engine();
    answer(&mut engine, b"\x1b]7501;?\x1b\\\x1b[?2031h");
    assert_eq!(told(&mut engine, true), LIGHT_REPORT);
    answer(&mut engine, b"\x1b]7501;state=clear\x07\x1b]7501;state=done:app=claude-code\x07");
    assert_eq!(told(&mut engine, true), "");
}

/// A Claude that was killed leaves mode 2031 on: once recruit says it stopped, the shell that takes the pane over
/// is sent nothing when a client attaches, whatever its theme.
#[test]
fn a_shell_after_a_killed_claude_is_told_nothing() {
    let (dark, light) = (Some((0x1c, 0x1e, 0x22)), Some((0xf6, 0xf6, 0xef)));
    let attach = |engine: &mut Ghostty, bg| {
        let mut replies = Vec::new();
        engine.set_colors(None, bg);
        engine.color_scheme(bg.is_some_and(light_background), &mut replies);
        String::from_utf8(replies).unwrap()
    };
    let mut engine = engine();
    engine.set_colors(None, light);
    assert_eq!(answer(&mut engine, b"\x1b]7501;?\x1b\\\x1b[?2031h"), format!("\x1b]7501;?\x1b\\{LIGHT_REPORT}"));
    assert_eq!(answer(&mut engine, b"\x1b[?2031$p"), "\x1b[?2031;1$y");
    // It dies without turning the mode off; recruit says so; the shell starts.
    answer(&mut engine, b"\x1b]7501;state=clear:app=recruit\x07$ ");
    assert_eq!(answer(&mut engine, b"\x1b[?2031$p"), "\x1b[?2031;2$y");
    // A client with another theme attaches, then one with the same theme again, then the first one's.
    assert_eq!(attach(&mut engine, dark), "");
    assert_eq!(attach(&mut engine, dark), "");
    assert_eq!(attach(&mut engine, light), "");
    // The next Claude turns it on itself, and is told.
    assert_eq!(answer(&mut engine, b"\x1b[?2031h"), LIGHT_REPORT);
    assert_eq!(attach(&mut engine, dark), DARK_REPORT);
}

/// A program that turns mode 2031 on and never asks (`CSI ? 996 n`) is told the theme there and then, without a
/// call to [`Engine::color_scheme`]: a Claude started in a pane with a client attached all along learns it at once.
#[test]
fn a_program_turning_the_mode_on_is_told_where_the_theme_stands() {
    let (dark, light) = (Some((0x1c, 0x1e, 0x22)), Some((0xf6, 0xf6, 0xef)));
    let on = |bg, bytes: &[u8]| {
        let mut engine = engine();
        engine.set_colors(None, bg);
        answer(&mut engine, bytes)
    };
    assert_eq!(on(light, b"\x1b[?2031h"), LIGHT_REPORT);
    assert_eq!(on(dark, b"\x1b[?2031h"), DARK_REPORT);
    // Among other modes; in its place in what the program wrote.
    assert_eq!(on(light, b"\x1b[?1004;2031;2004h"), LIGHT_REPORT);
    assert_eq!(on(light, b"\x1b[5n\x1b[?2031h\x1b[6n"), format!("\x1b[0n{LIGHT_REPORT}\x1b[1;1R"));
    // Not before the client's background is known, and not for another mode, or a mode turned off.
    assert_eq!(on(None, b"\x1b[?2031h"), "");
    assert_eq!(on(light, b"\x1b[?2031l\x1b[?2004h\x1b[?2026h\x1b[?2026l"), "");
    // Cut between two reads: told once.
    let mut engine = Ghostty::new(20, 5, 100).unwrap();
    engine.set_colors(None, light);
    assert_eq!(answer(&mut engine, b"\x1b[?20"), "");
    assert_eq!(answer(&mut engine, b"31h"), LIGHT_REPORT);
    // Once: a program that turns it on again, or asks, hears nothing more of it unasked.
    assert_eq!(answer(&mut engine, b"\x1b[?2031h"), "");
    assert_eq!(answer(&mut engine, b"\x1b[?2031l\x1b[?2031h"), "");
    assert_eq!(answer(&mut engine, b"\x1b[?996n"), LIGHT_REPORT);
    // Having asked first, it is not told again when it turns the mode on.
    let mut engine = Ghostty::new(20, 5, 100).unwrap();
    engine.set_colors(None, dark);
    assert_eq!(answer(&mut engine, b"\x1b[?996n\x1b[?2031h"), DARK_REPORT);
    // The client shows up after the program turned the mode on: told by the attach (`color_scheme`), as before.
    let mut engine = Ghostty::new(20, 5, 100).unwrap();
    assert_eq!(answer(&mut engine, b"\x1b[?2031h"), "");
    let mut replies = Vec::new();
    engine.set_colors(None, light);
    engine.color_scheme(true, &mut replies);
    assert_eq!(replies, LIGHT_REPORT.as_bytes());
}

/// What the program was told is what it is compared with: an attach that moved the colors first
/// ([`Engine::set_colors`]) still tells a change, and one that finds the theme the program knows tells nothing.
#[test]
fn an_attach_tells_a_program_only_what_changed() {
    let mut engine = engine();
    let (dark, light) = (Some((0, 0, 0)), Some((0xff, 0xff, 0xff)));
    let attach = |engine: &mut Ghostty, bg| {
        let mut replies = Vec::new();
        engine.set_colors(None, bg);
        engine.color_scheme(bg.is_some_and(light_background), &mut replies);
        String::from_utf8(replies).unwrap()
    };
    engine.set_colors(None, dark);
    answer(&mut engine, b"\x1b[?2031h");
    assert_eq!(theme(&mut engine), "dark");
    assert_eq!(attach(&mut engine, dark), "", "the same theme");
    assert_eq!(attach(&mut engine, light), LIGHT_REPORT);
    assert_eq!(theme(&mut engine), "light");
    assert_eq!(attach(&mut engine, light), "");
    assert_eq!(attach(&mut engine, dark), DARK_REPORT);
    // Never asked, never told: a program that only turned the mode on is told where it stands, once.
    let mut engine = Ghostty::new(20, 5, 100).unwrap();
    answer(&mut engine, b"\x1b[?2031h");
    assert_eq!(attach(&mut engine, light), LIGHT_REPORT);
    assert_eq!(attach(&mut engine, light), "");
}

#[test]
fn relayed() {
    let mut engine = engine();
    answer(&mut engine, b"\x1b]52;c;aGVsbG8=\x07\x1b]2;Claude\x07\x07\x1b]9;Done\x07\x1b]9;4;1;50\x07");
    assert_eq!(
        relays(&mut engine),
        [
            Relay::Clipboard("hello".into()),
            Relay::Title("Claude".into()),
            Relay::Bell,
            Relay::Notify { title: None, body: "Done".into() },
            Relay::Progress(super::super::Progress::Set(50)),
        ]
    );
    assert!(relays(&mut engine).is_empty(), "given once");
}

#[test]
fn a_requested_size_is_relayed_not_applied() {
    let mut engine = engine();
    answer(&mut engine, b"\x1b[8;12;40t\x1b[18t\x1b[8;0;40t\x1b[8;2000;40tab");
    assert_eq!(relays(&mut engine), [Relay::Resize { rows: 12, cols: 40 }]);
    // Never written back, and the screen keeps its size.
    assert_eq!(answer(&mut engine, b"\x1b[8;12;40t"), "");
    assert_eq!((engine.term.cols().unwrap(), engine.term.rows().unwrap()), (20, 5));
}

#[test]
fn program_statuses_count_once_asked() {
    let mut engine = engine();
    let working = b"\x1b]7501;state=working:app=claude-code\x1b\\";
    // Not asked: a shell replaying old output sets nothing.
    answer(&mut engine, working);
    assert!(relays(&mut engine).is_empty());
    answer(&mut engine, b"\x1b]7501;?\x1b\\");
    answer(&mut engine, working);
    assert!(matches!(&relays(&mut engine)[..], [Relay::Status(s)] if s.state == State::Working));
    // A sub-agent's, paused, then its end (Claude Code 2.1.296): not the program's state, which goes on.
    answer(&mut engine, b"\x1b]7501;state=working:app=claude-code:id=a1b2:title=VGVzdHM=\x1b\\");
    answer(&mut engine, b"\x1b]7501;state=blocked:app=claude-code:id=a1b2:title=VGVzdHM=\x1b\\");
    answer(&mut engine, b"\x1b]7501;state=clear:id=a1b2\x1b\\");
    answer(&mut engine, working);
    assert!(matches!(&relays(&mut engine)[..], [Relay::Status(s)] if s.state == State::Working));
    // Claude Code's own clear (on its way out, or the component that writes its status unmounted): the pane
    // without a state, until the next.
    answer(&mut engine, b"\x1b]7501;state=clear\x1b\\");
    answer(&mut engine, working);
    assert!(matches!(
        &relays(&mut engine)[..],
        [Relay::Status(a), Relay::Status(b)] if a.state == State::Clear && b.state == State::Working
    ));
    // Recruit's, once its Claude stopped (member.rs): then nothing until asked again.
    answer(&mut engine, b"\x1b]7501;state=clear:app=recruit\x07");
    answer(&mut engine, working);
    assert!(matches!(&relays(&mut engine)[..], [Relay::Status(s)] if s.state == State::Clear));
    answer(&mut engine, b"\x1b]7501;?\x1b\\");
    answer(&mut engine, working);
    assert!(matches!(&relays(&mut engine)[..], [Relay::Status(s)] if s.state == State::Working));
    // A reset too.
    answer(&mut engine, b"\x1b]7501;?\x1b\\\x1bc");
    answer(&mut engine, working);
    assert!(relays(&mut engine).is_empty());
}

/// A reset (RIS) that follows the end of a string without `ESC \` still stops the program statuses.
#[test]
fn a_reset_right_after_a_string_stops_program_statuses() {
    let mut engine = engine();
    let working = b"\x1b]7501;state=working:app=claude-code\x1b\\";
    answer(&mut engine, b"\x1b]7501;?\x1b\\");
    answer(&mut engine, working);
    assert!(matches!(&relays(&mut engine)[..], [Relay::Status(s)] if s.state == State::Working));
    answer(&mut engine, b"\x1b]9;done\x1bc");
    relays(&mut engine);
    answer(&mut engine, working);
    assert!(relays(&mut engine).is_empty());
}

#[test]
fn an_update_may_take_its_time() {
    // A slow machine: the update comes through in pieces, well past 150 ms; held all the same, up to a second.
    let mut engine = engine();
    answer(&mut engine, b"\x1b[Hold");
    rows(&engine);
    let start = Instant::now();
    answer(&mut engine, b"\x1b[?2026h\x1b[Hne");
    assert!(!engine.expire(start + Duration::from_millis(500)));
    assert_eq!(rows(&engine)[0], "old");
    answer(&mut engine, b"w\x1b[?2026l");
    assert_eq!(rows(&engine)[0], "new");
    // A program that never ends one: shown after a second.
    answer(&mut engine, b"\x1b[?2026h\x1b[Hstuck");
    assert_eq!(rows(&engine)[0], "new");
    assert!(engine.expire(Instant::now() + SYNC_TIMEOUT));
    assert_eq!(rows(&engine)[0], "stuck");
}

#[test]
fn an_update_that_ends_and_begins_in_one_read() {
    // Output read late: the end of one update and the start of the next come in one read.
    let mut engine = engine();
    answer(&mut engine, b"\x1b[?2026h\x1b[2J\x1b[Hone");
    let first = engine.sync_until.expect("a deadline");
    std::thread::sleep(Duration::from_millis(2));
    answer(&mut engine, b"\x1b[?2026l\x1b[?2026h\x1b[2J\x1b[Htwo");
    // The next update has a deadline of its own, and holds the frame: the first update, whole.
    assert!(engine.sync_until.is_some_and(|next| next > first), "the first update's deadline kept");
    assert_eq!(rows(&engine)[0], "one");
    // Past its deadline, frames go on; then it ends, and the next begins, in one read: held all the same.
    assert!(engine.expire(engine.sync_until.unwrap()));
    assert_eq!(rows(&engine)[0], "two");
    answer(&mut engine, b"\x1b[?2026l\x1b[?2026h\x1b[2J\x1b[Hthr");
    assert!(engine.sync_until.is_some(), "the next update not held");
    assert_eq!(rows(&engine)[0], "two", "half of the next update shown");
    // Its end shows it whole.
    answer(&mut engine, b"ee\x1b[?2026l");
    assert_eq!(rows(&engine)[0], "three");
}

#[test]
fn every_whole_frame_of_a_late_read_shows() {
    // A read that holds several updates, the last one unfinished: the last whole frame shows, not the first one,
    // nor the unfinished one.
    let mut engine = engine();
    rows(&engine);
    answer(&mut engine, b"\x1b[?2026h\x1b[Ha\x1b[?2026l\x1b[?2026h\x1b[Hb\x1b[?2026l\x1b[?2026h\x1b[Hc");
    assert!(engine.sync_until.is_some());
    assert_eq!(rows(&engine)[0], "b");
    // A mode 2026 reset among others ends it too.
    answer(&mut engine, b"\x1b[?1000;2026l");
    assert_eq!(rows(&engine)[0], "c");
    assert!(engine.sync_until.is_none());
}

#[test]
fn synchronized_updates_hold_the_frame() {
    let mut engine = engine();
    answer(&mut engine, b"before");
    assert_eq!(rows(&engine)[0], "before");
    // Held, write after write: the frame stays, and the queries are answered at once.
    assert_eq!(answer(&mut engine, b"\x1b[?2026h\x1b[2J\x1b[Hafter\x1b[c"), "\x1b[?62;22c");
    assert_eq!(answer(&mut engine, b" more"), "");
    assert_eq!(rows(&engine)[0], "before");
    let deadline = engine.deadline().expect("a deadline");
    assert!(deadline <= Instant::now() + SYNC_TIMEOUT);
    // Its end shows it all.
    answer(&mut engine, b"\x1b[?2026l");
    assert_eq!(rows(&engine)[0], "after more");
    // Never ended: shown at the deadline, not before.
    answer(&mut engine, b"\x1b[?2026h\x1b[2J\x1b[Hlate");
    let deadline = engine.sync_until.unwrap();
    assert!(!engine.expire(deadline - Duration::from_millis(1)));
    assert_eq!(rows(&engine)[0], "after more");
    assert!(engine.expire(deadline));
    assert_eq!(rows(&engine)[0], "late");
    // Still in the update past its deadline: frames go on.
    answer(&mut engine, b" too");
    assert_eq!(rows(&engine)[0], "late too");
}

#[test]
fn cells_are_drawn_as_claude_measures_them() {
    let mut engine = engine();
    // Each takes the columns Bun.stringWidth gives it: 👍🏽, ❤️ and 🇫🇷 two, e + accent one.
    answer(&mut engine, "a界b e\u{301} 👍🏽 ❤\u{fe0f}🇫🇷|".as_bytes());
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    assert_eq!(canvas.row(0), "a界b e\u{301} 👍🏽 ❤\u{fe0f}🇫🇷|");
    assert_eq!(engine.cursor().unwrap().x, 15);
    answer(&mut engine, b"\r\n\x1b[1;31mR\x1b[0m\x1b[38;2;1;2;3mT\x1b[0m\x1b[38;5;200mI\x1b[0mx");
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    let at = |x| canvas.style(x, 1);
    assert_eq!((at(0).fg, at(0).bold), (Some(Paint::DarkRed), true));
    assert_eq!(at(1).fg, Some(Paint::Rgb { r: 1, g: 2, b: 3 }));
    assert_eq!(at(2).fg, Some(Paint::AnsiValue(200)));
    assert_eq!(at(3).fg, None, "the real terminal's own color");
    // Cells erased with a background color set: that background, without text or style of their own.
    answer(&mut engine, b"\r\n\x1b[41m\x1b[K\x1b[0m\r\n\x1b[48;2;4;5;6m\x1b[2X\x1b[0m");
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    assert_eq!((canvas.style(0, 2).bg, canvas.style(19, 2).bg), (Some(Paint::DarkRed), Some(Paint::DarkRed)));
    assert_eq!((canvas.style(1, 3).bg, canvas.style(2, 3).bg), (Some(Paint::Rgb { r: 4, g: 5, b: 6 }), None));
    // A wide character cut by the pane's edge is not drawn half.
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 2, height: 1 });
    assert_eq!(canvas.row(0), "a");
}

#[test]
fn links_and_hidden_text() {
    let mut engine = engine();
    answer(&mut engine, b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\ \x1b[8msecret");
    rows(&engine);
    assert_eq!(engine.view.borrow().frame.rows[0].links, ["https://example.com"], "one link for its four cells");
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    assert_eq!(canvas.link_at(0, 0), Some("https://example.com"));
    assert_eq!(canvas.link_at(5, 0), None);
    assert_eq!(canvas.row(0), "link");
}

#[test]
fn the_cursor_as_the_program_left_it() {
    let mut engine = engine();
    answer(&mut engine, b"\x1b[2;4H");
    rows(&engine);
    let cursor = engine.cursor().unwrap();
    assert_eq!((cursor.x, cursor.y, cursor.shape), (3, 1, canvas::CursorShape::Default));
    answer(&mut engine, b"\x1b[6 q");
    rows(&engine);
    assert_eq!(engine.cursor().unwrap().shape, canvas::CursorShape::Bar);
    answer(&mut engine, b"\x1b[0 q\x1b[?25l");
    rows(&engine);
    assert_eq!(engine.cursor(), None);
    answer(&mut engine, b"\x1b[?25h");
    rows(&engine);
    assert_eq!(engine.cursor().unwrap().shape, canvas::CursorShape::Default);
}

#[test]
fn the_view_stays_on_its_lines() {
    let mut engine = engine();
    for i in 0..10 {
        answer(&mut engine, format!("line {i}\r\n").as_bytes());
    }
    assert_eq!(engine.history(), 6);
    engine.scroll(2);
    assert_eq!(engine.scrolled(), 2);
    assert_eq!(rows(&engine)[0], "line 4");
    // The program writes: the view keeps showing the same lines.
    answer(&mut engine, b"line 10\r\n");
    assert_eq!((engine.scrolled(), rows(&engine)[0].as_str()), (3, "line 4"));
    engine.scroll(100);
    assert_eq!(engine.scrolled(), engine.history());
    engine.scroll(-1);
    assert_eq!(engine.scrolled(), engine.history() - 1);
    engine.scroll(isize::MIN);
    assert_eq!(engine.scrolled(), 0);
    assert_eq!(rows(&engine)[3], "line 10");
}

#[test]
fn lines_keep_their_numbers() {
    let mut engine = Ghostty::new(20, 5, 10).unwrap();
    let text = |line: Option<Line>| line.map(|line| line.text.trim_end().to_string());
    for i in 0..8 {
        answer(&mut engine, format!("line {i}\r\n").as_bytes());
    }
    assert_eq!(engine.oldest(), 0);
    assert_eq!(text(engine.line(0)).as_deref(), Some("line 0"));
    assert_eq!(engine.top(), 4, "lines 0 to 3 above the screen");
    let wide = engine.line(2).unwrap();
    assert_eq!((wide.width(), wide.cell(0), wide.wrapped), (20, "l", false));
    // The program writes on: Ghostty drops the history by whole pages (thousands of lines here, past the limit of
    // 10), and every line keeps its number.
    let mut written = 8;
    while engine.oldest() == 0 {
        answer(&mut engine, format!("line {written}\r\n").as_bytes());
        written += 1;
        assert!(written < 50_000, "the history never turned");
    }
    let oldest = engine.oldest();
    assert_eq!(engine.line(oldest - 1), None);
    assert_eq!(text(engine.line(oldest)), Some(format!("line {oldest}")));
    assert_eq!(text(engine.line(written - 1)), Some(format!("line {}", written - 1)));
    // CSI 3J empties the history: the oldest line jumps forward, never back.
    answer(&mut engine, b"\x1b[3J");
    assert!(engine.oldest() > oldest);
    assert_eq!(text(engine.line(engine.top())), Some(format!("line {}", written - 4)));
    // Not written yet: none.
    assert_eq!(engine.line(engine.top() + 10), None);
    // Soft wraps are said.
    answer(&mut engine, &[b'x'; 25]);
    // 20 on the next-to-last row, which goes on; the last 5 below it.
    let first = engine.top() + 3;
    assert_eq!((engine.line(first).unwrap().text, engine.line(first).unwrap().wrapped), ("x".repeat(20), true));
    assert!(!engine.line(first + 1).unwrap().wrapped);
    // The alternate screen counts its own lines, from 0.
    answer(&mut engine, b"\x1b[?1049h\x1b[Halt");
    assert_eq!((engine.oldest(), engine.top()), (0, 0));
    assert_eq!(text(engine.line(0)).as_deref(), Some("alt"));
}

#[test]
fn modes_for_the_input() {
    let mut engine = engine();
    let start = engine.modes();
    assert_eq!(start, Modes { alternate_scroll: start.alternate_scroll, ..Modes::default() });
    answer(&mut engine, b"\x1b[?1h\x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1006h\x1b[?1049h\x1b[>11u");
    assert_eq!(
        engine.modes(),
        Modes {
            app_cursor: true,
            bracketed_paste: true,
            focus: true,
            mouse: Some(MouseTracking::Drag),
            mouse_sgr: true,
            alternate_scroll: start.alternate_scroll,
            alt_screen: true,
            kitty: 11,
        }
    );
}

#[test]
fn history_compressed_once_quiet_then_nothing() {
    let mut engine = Ghostty::new(80, 24, 100_000).unwrap();
    let line = format!("{}\r\n", "compressible text ".repeat(4));
    let chunk = line.repeat(1000);
    let mut replies = Vec::new();
    for _ in 0..30 {
        engine.feed(chunk.as_bytes(), &mut replies);
    }
    let quiet = engine.deadline().expect("compression to finish");
    assert!(quiet > Instant::now() + QUIET / 2);
    // Nothing before the pane is quiet; then steps until done, then no deadline at all.
    assert!(!engine.expire(quiet - Duration::from_millis(1)));
    assert_eq!(engine.deadline(), Some(quiet));
    let mut now = quiet;
    let mut steps = 0;
    while let Some(at) = engine.deadline() {
        now = now.max(at);
        assert!(!engine.expire(now), "compression does not change the screen");
        steps += 1;
        assert!(steps < 100_000);
    }
    assert!(steps >= 1);
    // The text is still there, read back from compressed pages.
    let oldest = engine.oldest();
    assert!(engine.line(oldest).unwrap().text.starts_with("compressible text"));
}

#[test]
fn resize_keeps_the_screen_and_the_history() {
    let screen = |engine: &Ghostty, (w, h): (u16, u16)| {
        let mut canvas = Canvas::new(usize::from(w), usize::from(h));
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: usize::from(w), height: usize::from(h) });
        (0..usize::from(h)).map(|y| canvas.row(y)).collect::<Vec<_>>()
    };
    // Text at the top: smaller, larger, narrower, the blank rows below go, the text stays where it was (a
    // program that does not redraw on SIGWINCH, the shell after Claude, keeps its screen).
    let mut engine = Ghostty::new(80, 24, 1000).unwrap();
    answer(&mut engine, b"ligne-1\r\nligne-2\r\nligne-3\r\n");
    for size in [(80, 20), (100, 29), (60, 10), (200, 50), (79, 24)] {
        engine.resize(size.0, size.1);
        assert_eq!(screen(&engine, size)[..4], ["ligne-1", "ligne-2", "ligne-3", ""], "{size:?}");
        assert_eq!(engine.history(), 0, "{size:?}");
        rows(&engine);
        assert_eq!(engine.cursor().map(|c| (c.x, c.y)), Some((0, 3)), "{size:?}");
    }
    // A cleared screen written at the top, likewise.
    answer(&mut engine, b"\x1b[2J\x1b[Hen haut");
    engine.resize(70, 12);
    assert_eq!(screen(&engine, (70, 12))[0], "en haut");
    // Smaller than the text: what does not fit goes into the history, nothing is lost.
    let mut engine = Ghostty::new(20, 10, 1000).unwrap();
    for i in 0..8 {
        answer(&mut engine, format!("line {i}\r\n").as_bytes());
    }
    engine.resize(20, 4);
    let shown = screen(&engine, (20, 4));
    assert!(shown.contains(&"line 7".to_string()), "{shown:?}");
    assert!(engine.history() > 0);
    let kept = |engine: &Ghostty| {
        let first = engine.oldest();
        (first..first + 8)
            .filter_map(|line| engine.line(line))
            .map(|line| line.text.trim_end().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(kept(&engine), (0..8).map(|i| format!("line {i}")).collect::<Vec<_>>());
    engine.scroll(100);
    assert_eq!(rows(&engine)[0], "line 0");
    engine.scroll(isize::MIN);
    // Larger again: everything is still there, on the screen or in the history.
    engine.resize(20, 12);
    let all = kept(&engine);
    assert_eq!(all, (0..8).map(|i| format!("line {i}")).collect::<Vec<_>>());
}

#[test]
fn long_graphemes_are_copied_whole() {
    let mut engine = engine();
    // A family of five joined by ZWJ: nine code points in one cell.
    let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦\u{200d}👦";
    answer(&mut engine, format!("{family}|").as_bytes());
    let line = engine.line(engine.top()).unwrap();
    assert_eq!((line.cell(0), line.cell(1), line.cell(2)), (family, "", "|"));
}

#[test]
fn frames_follow_every_change_though_rows_are_read_again_only_when_dirty() {
    let mut engine = engine();
    answer(&mut engine, b"one\r\ntwo\r\n\x1b[31mred\x1b[0m\r\nfour");
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    assert_eq!(rows(&engine)[..4], ["one", "two", "red", "four"]);
    // A change in one row, the rest kept; then nothing new.
    answer(&mut engine, b"\x1b[2;1HTWO");
    assert_eq!(rows(&engine)[..4], ["one", "TWO", "red", "four"]);
    assert_eq!(rows(&engine)[..4], ["one", "TWO", "red", "four"]);
    // The cursor moves alone.
    answer(&mut engine, b"\x1b[5;3H");
    rows(&engine);
    assert_eq!(engine.cursor().map(|c| (c.x, c.y)), Some((2, 4)));
    // Colors changed by the program after a frame: every cell drawn with them shows it.
    answer(&mut engine, b"\x1b]4;1;rgb:00/ff/00\x1b\\");
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    assert_eq!(canvas.style(0, 2).fg, Some(Paint::Rgb { r: 0, g: 0xff, b: 0 }));
    answer(&mut engine, b"\x1b]11;rgb:01/02/03\x1b\\");
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    assert_eq!(canvas.style(10, 0).bg, Some(Paint::Rgb { r: 1, g: 2, b: 3 }));
    // The view moves up into the history: the frame follows.
    for i in 0..10 {
        answer(&mut engine, format!("\r\nmore {i}").as_bytes());
    }
    rows(&engine);
    engine.scroll(3);
    let shown = rows(&engine);
    engine.scroll(isize::MIN);
    assert_ne!(shown, rows(&engine));
    assert_eq!(rows(&engine)[4], "more 9");
}

#[test]
fn generation_changes_with_the_cells_drawn_only() {
    let mut engine = engine();
    let mut seen = engine.generation().unwrap();
    let mut step = |engine: &mut Ghostty, bytes: &[u8]| {
        answer(engine, bytes);
        let now = engine.generation().unwrap();
        let changed = now != seen;
        seen = now;
        changed
    };
    assert!(step(&mut engine, b"hello\r\nworld"));
    assert!(!step(&mut engine, b""), "nothing new");
    assert!(!step(&mut engine, b"\x1b[3;5H"), "the cursor alone");
    assert_eq!(engine.cursor().map(|c| (c.x, c.y)), Some((4, 2)), "given up to date");
    assert!(!step(&mut engine, b"\x1b[1;1Hhello"), "the same text written again");
    // Drawn by someone else in between (a capture): still seen as changed by the screen.
    answer(&mut engine, b"!");
    let mut canvas = Canvas::new(20, 5);
    engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 20, height: 5 });
    assert!(step(&mut engine, b""));
    assert!(step(&mut engine, b"\x1b]11;rgb:01/02/03\x1b\\"), "the background");
    assert!(step(&mut engine, b"\x1b]4;1;rgb:00/ff/00\x1b\\\x1b[31mred\x1b[0m"));
    assert!(step(&mut engine, b"\x1b]4;1;rgb:00/00/ff\x1b\\"), "a color of the palette in use");
    // Held by a synchronized update, then shown.
    assert!(!step(&mut engine, b"\x1b[?2026hsome\r\nmore"));
    assert!(step(&mut engine, b"\x1b[?2026l"));
    // The view moves into the history and back.
    for i in 0..10 {
        answer(&mut engine, format!("\r\nline {i}").as_bytes());
    }
    step(&mut engine, b"");
    engine.scroll(2);
    assert!(step(&mut engine, b""));
    engine.scroll(isize::MIN);
    assert!(step(&mut engine, b""));
    engine.resize(30, 6);
    assert!(step(&mut engine, b""));
    // A new engine for the same pane: never a number the old one gave.
    let again = super::super::new(20, 5, 100).unwrap();
    assert_ne!(again.generation(), Some(seen));
    assert_ne!(again.generation(), self::engine().generation());
}

/// Where a frame's time goes, with Claude-like lines (colored bullet, bold, a path): `cargo test --release
/// draw_costs -- --ignored --nocapture`.
#[test]
#[ignore = "a measure, not a check"]
fn draw_costs() {
    let (cols, rows) = (120u16, 50u16);
    let area = Rect { x: 0, y: 0, width: usize::from(cols), height: usize::from(rows) };
    let line = |i: usize| {
        format!(
            "\x1b[38;2;215;119;87m⏺\x1b[0m Line {i} of a reply, with \x1b[1mbold\x1b[0m text and a path src/mux/pty.rs:{i}\r\n"
        )
    };
    let mut engine = Ghostty::new(cols, rows, 100_000).unwrap();
    for i in 0..2000 {
        answer(&mut engine, line(i).as_bytes());
    }
    let mut painter = crate::ui::canvas::Painter::new(crate::ui::canvas::Features::default());
    let (mut capture, mut paint, mut painted, mut colors) = (0u128, 0u128, 0u128, 0u128);
    let frames = 500;
    for (i, changing) in (0..frames * 2).map(|i| (i, i < frames)) {
        if changing {
            answer(&mut engine, line(i).as_bytes());
        } else {
            // An indicator that turns, as Claude's does: one cell of one row.
            answer(&mut engine, format!("\x1b7\x1b[10;1H{}\x1b8", ['◐', '◓', '◑', '◒'][i % 4]).as_bytes());
        }
        let started = Instant::now();
        let _ = std::hint::black_box(engine.colors());
        colors += started.elapsed().as_nanos();
        let started = Instant::now();
        engine.capture(&mut engine.view.borrow_mut());
        capture += started.elapsed().as_nanos();
        let mut canvas = Canvas::new(area.width, area.height);
        let started = Instant::now();
        engine.view.borrow().frame.paint(&mut canvas, area);
        paint += started.elapsed().as_nanos();
        let started = Instant::now();
        let _ = std::hint::black_box(painter.frame(&canvas));
        painted += started.elapsed().as_nanos();
        if i == frames - 1 || i == frames * 2 - 1 {
            let n = frames as u128 * 1000;
            println!(
                "{}: colors {} µs, capture {} µs (colors included), paint {} µs, Painter::frame {} µs",
                if changing { "a line a frame (all rows move)" } else { "one cell a frame" },
                colors / n,
                capture / n,
                paint / n,
                painted / n
            );
            (capture, paint, painted, colors) = (0, 0, 0, 0);
        }
    }
}

#[test]
fn resized_down_to_nothing() {
    let mut engine = engine();
    answer(&mut engine, "界界界界界界界界界界界\r\n".repeat(10).as_bytes());
    for (cols, rows) in [(1, 1), (0, 0), (3, 2), (200, 60), (1, 30)] {
        engine.resize(cols, rows);
        answer(&mut engine, "界a\u{301}\r\n".repeat(3).as_bytes());
        let mut canvas = Canvas::new(200, 60);
        engine.draw(&mut canvas, Rect { x: 0, y: 0, width: 200, height: 60 });
        let _ = (engine.top(), engine.line(engine.oldest()));
    }
}
