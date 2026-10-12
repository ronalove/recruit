// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn text_in_columns() {
    let mut canvas = Canvas::new(10, 2);
    assert_eq!(canvas.put(1, 0, "日本x", Style::PLAIN), 6);
    assert_eq!(canvas.row(0), " 日本x");
    // Cut before a wide character that would not fit.
    assert_eq!(canvas.put(8, 1, "a日", Style::PLAIN), 9);
    assert_eq!(canvas.row(1), "        a");
    // Written over half a wide character: the other half goes blank.
    canvas.put(2, 0, "z", Style::PLAIN);
    assert_eq!(canvas.row(0), "  z本x");
    let mut canvas = Canvas::new(10, 1);
    canvas.put(0, 0, "日", Style::PLAIN);
    canvas.put(0, 0, "a", Style::PLAIN);
    assert_eq!(canvas.row(0), "a");
    // A wide character over another, or over one shifted by a column.
    let mut canvas = Canvas::new(10, 1);
    canvas.put(0, 0, "日本", Style::PLAIN);
    canvas.put(0, 0, "語", Style::PLAIN);
    assert_eq!(canvas.row(0), "語本");
    canvas.put(1, 0, "語", Style::PLAIN);
    assert_eq!(canvas.row(0), " 語");
    // An accent goes with the letter before it; a joiner or a variation selector is left out, and an accent
    // with nothing before it too.
    let mut canvas = Canvas::new(10, 1);
    assert_eq!(canvas.put(0, 0, "\u{301}e\u{301}\u{200d}x\u{fe0f}", Style::PLAIN), 2);
    assert_eq!(canvas.row(0), "e\u{301}x");
    assert_eq!(columns("e\u{301}\u{200d}x\u{fe0f}"), 2);
    // Format characters left out as well, direction controls among them.
    let mut canvas = Canvas::new(10, 1);
    canvas.put(0, 0, "a\u{61c}\u{fff9}\u{1d173}\u{13430}\u{202e}\u{2066}b", Style::PLAIN);
    assert_eq!(canvas.row(0), "ab");
    assert!(canvas.clusters.is_empty());
    assert_eq!(columns("a\u{fff9}\u{ad}b"), 2, "measured as drawn");
    // A pile of accents: the cell full, nothing more kept.
    let mut canvas = Canvas::new(2, 1);
    canvas.put(0, 0, &format!("e{}", "\u{301}".repeat(500)), Style::PLAIN);
    assert_eq!(canvas.row(0), format!("e{}", "\u{301}".repeat(31)));
    assert!(canvas.clusters.len() < 2 * CLUSTER * CLUSTER, "{}", canvas.clusters.len());
}

#[test]
fn a_canvas_kept_from_frame_to_frame() {
    let mut canvas = Canvas::new(8, 2);
    // A fill whose edges cut wide characters: their other halves go blank.
    canvas.put(0, 0, "日本語x", Style::PLAIN);
    canvas.fill(1, 0, 4, 1, Style::PLAIN);
    assert_eq!(canvas.row(0), "      x");
    canvas.put(0, 1, "abcdefgh", Style::PLAIN);
    canvas.fill(2, 1, 3, 1, Style::PLAIN);
    assert_eq!(canvas.row(1), "ab   fgh");
    // Graphemes written again and again: their store keeps the ones shown only, once compacted.
    let family = "\u{1f468}\u{200d}\u{1f469}";
    for _ in 0..100 {
        canvas.put_cell(0, 0, family, 2, Style::PLAIN);
    }
    let stored = canvas.clusters.len();
    canvas.compact(stored);
    assert_eq!(canvas.clusters.len(), stored, "within its room: left alone");
    canvas.compact(0);
    assert_eq!(canvas.clusters, family);
    assert!(canvas.row(0).starts_with(family));
    // Links ever shown: past LINKS, only those still shown stay, renumbered.
    let mut canvas = Canvas::new(2, 1);
    for i in 0..=LINKS {
        let link = canvas.link(&format!("https://e.example/{i}"));
        canvas.put(0, 0, "a", Style { link: Some(link), ..Style::PLAIN });
    }
    let first = canvas.link("https://e.example/0");
    canvas.put(1, 0, "b", Style { link: Some(first), ..Style::PLAIN });
    canvas.compact(usize::MAX);
    assert_eq!(canvas.links.len(), 2);
    assert_eq!(canvas.link_at(0, 0), Some(format!("https://e.example/{LINKS}").as_str()));
    assert_eq!(canvas.link_at(1, 0), Some("https://e.example/0"));
    // Blank again, at another size.
    canvas.reset(4, 1);
    assert_eq!((canvas.width(), canvas.height(), canvas.row(0)), (4, 1, String::new()));
    assert!(canvas.clusters.is_empty());
}

#[test]
fn a_kept_canvas_painted_by_its_changes_only() {
    // Random writes on a canvas kept from frame to frame: a painter that compares the changed columns only sends
    // what one comparing everything sends, frames skipped included.
    use super::rng::Rng;
    // Run longer on other seeds while working on the painter: 20 000 rounds on six seeds passed.
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    let words = ["abc", "日本", "x", "  ", "──", "é\u{301}", "a日b"];
    let colors = [None, Some(Color::Cyan), Some(Color::AnsiValue(141)), Some(Color::Rgb { r: 215, g: 119, b: 87 })];
    let family = "\u{1f468}\u{200d}\u{1f469}";
    for _ in 0..500 {
        let (mut w, mut h) = (1 + r.n(30), 1 + r.n(6));
        let mut canvas = Canvas::new(w, h);
        let features =
            Features { sync: r.n(2) == 1, truecolor: r.n(2) == 1, links: r.n(4) != 0, graphemes: r.n(2) == 1 };
        let (mut by_changes, mut whole) = (Painter::new(features), Painter::new(features));
        let mut ops: Vec<String> = Vec::new();
        for _ in 0..14 {
            canvas.advance();
            ops.push("--".into());
            for _ in 0..r.n(6) {
                let (x, y) = (r.n(w), r.n(h));
                let style = Style { fg: colors[r.n(4)], bg: colors[r.n(4)], bold: r.n(2) == 1, ..Style::PLAIN };
                let op = r.n(11);
                ops.push(format!("{op} at {x},{y}"));
                match op {
                    0 => canvas.fill(x, y, r.n(w) + 1, r.n(h) + 1, style),
                    1 => canvas.tint(x, y, r.n(w) + 1, Color::AnsiValue(236)),
                    2 => canvas.invert(x, y, r.n(w) + 1),
                    3 => canvas.fade_outside(x, y, r.n(w), r.n(h), Style::fg(Color::AnsiValue(239))),
                    4 => canvas.put_cell(x, y, family, 2, style),
                    5 => {
                        let link = canvas.link(["https://a.example", "https://b.example"][r.n(2)]);
                        canvas.put(x, y, words[r.n(words.len())], Style { link: Some(link), ..style });
                    }
                    6 => canvas.set_cursor(Some(Cursor { x, y, shape: CursorShape::Bar, blink: false })),
                    7 => canvas.compact(r.n(2) * 64),
                    // Links past LINKS, each to its own URI: the next compaction renumbers them.
                    8 => {
                        for i in 0..LINKS + 1 + r.n(32) {
                            let link = canvas.link(&format!("https://e.example/{}", r.n(1000) * 1000 + i));
                            let (x, y) = (r.n(w), r.n(h));
                            canvas.put(x, y, "l", Style { link: Some(link), ..style });
                        }
                        canvas.compact(usize::MAX);
                    }
                    // Blank again, at the same size or another.
                    9 => {
                        if r.n(2) == 0 {
                            (w, h) = (1 + r.n(30), 1 + r.n(6));
                        }
                        canvas.reset(w, h);
                    }
                    _ => {
                        canvas.put(x, y, words[r.n(words.len())], style);
                    }
                }
            }
            // Some frames are not sent (a client busy): the next one carries their changes.
            if r.n(4) == 0 {
                continue;
            }
            let mut copy = canvas.clone();
            copy.lineage = lineage();
            let (a, b) = (by_changes.frame(&canvas), whole.frame(&copy));
            assert_eq!(text(&a), text(&b), "{w}×{h} {ops:?}");
        }
    }
}

#[test]
fn right_aligned_and_cut() {
    let mut canvas = Canvas::new(12, 1);
    assert_eq!(canvas.put_right(12, 0, "équipe", Style::PLAIN), 6);
    assert_eq!(canvas.row(0), "      équipe");
    assert_eq!(fit("settings.local.toml", 10), "settings.…");
    assert_eq!(fit("court", 10), "court");
    assert_eq!(fit("日本語のラベル", 7), "日本語…");
    assert_eq!(columns("日本x"), 5);
}

#[test]
fn boxes_and_fades() {
    let mut canvas = Canvas::new(6, 4);
    canvas.put(0, 0, "abcdef", Style::fg(Color::Cyan));
    canvas.frame(1, 1, 4, 3, Style::fg(Color::DarkGrey));
    assert_eq!((canvas.row(1), canvas.row(2), canvas.row(3)), (" ╭──╮".into(), " │  │".into(), " ╰──╯".into()));
    let faded = Style::fg(Color::AnsiValue(239));
    canvas.fade_outside(1, 1, 4, 3, faded);
    assert_eq!(canvas.style(0, 0), faded);
    assert_eq!(canvas.style(1, 1), Style::fg(Color::DarkGrey));
    canvas.tint(0, 0, 2, Color::AnsiValue(236));
    assert_eq!(canvas.style(1, 0).bg, Some(Color::AnsiValue(236)));
    assert_eq!(canvas.row(0), "abcdef", "tinted, it shows the same");
}

#[test]
fn only_what_changed() {
    let mut painter = Painter::default();
    let mut canvas = Canvas::new(20, 3);
    canvas.put(0, 0, "Que veux-tu changer ?", Style::PLAIN);
    let first = text(&painter.frame(&canvas));
    assert!(first.starts_with("\x1b[0m\x1b[2J"), "the first frame whole");
    assert!(painter.frame(&canvas).is_empty(), "nothing changed, nothing sent");

    canvas.put(4, 1, "x", Style::fg(Color::Cyan));
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;5H\x1b[96mx");
    // The same style again: no SGR. A short gap in another style: moved across.
    canvas.put(4, 1, "y", Style::fg(Color::Cyan));
    canvas.put(7, 1, "z", Style::fg(Color::Cyan));
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;5Hy\x1b[2;8Hz");
    // In the same style: written over.
    canvas.put(4, 1, "abcd", Style::fg(Color::Cyan));
    painter.frame(&canvas);
    canvas.put(4, 1, "e", Style::fg(Color::Cyan));
    canvas.put(7, 1, "f", Style::fg(Color::Cyan));
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;5Hebcf");
    // A long gap: moved across.
    canvas.put(0, 2, "a", Style::PLAIN);
    canvas.put(15, 2, "b", Style::PLAIN);
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[3;1H\x1b[39ma\x1b[3;16Hb");
}

/// A frame of one row, `cells` given as the engine would.
fn pane_row(width: usize, cells: &[(&str, usize)]) -> Canvas {
    let mut canvas = Canvas::new(width, 1);
    let mut x = 0;
    for (grapheme, w) in cells {
        canvas.put_cell(x, 0, grapheme, *w, Style::PLAIN);
        x += w;
    }
    canvas
}

#[test]
fn graphemes_go_whole() {
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let canvas = pane_row(8, &[("a", 1), (family, 2), ("e\u{301}", 1), ("\x1bx", 1), ("\u{7}", 1)]);
    assert_eq!(canvas.row(0), format!("a{family}e\u{301}x"), "control characters left out");
    // Placed again after a grapheme of several characters, which the terminal may measure otherwise.
    let mut painter = Painter::new(Features { graphemes: true, ..Features::default() });
    painter.frame(&Canvas::new(8, 1));
    assert_eq!(text(&painter.frame(&canvas)), format!("\x1b[1;1Ha{family}\x1b[1;4He\u{301}\x1b[1;5Hx"));
    // Another canvas, its graphemes stored elsewhere: the same text, nothing to send.
    let mut again = Canvas::new(8, 1);
    again.clusters.push_str("zzzzz");
    again.put(0, 0, "a", Style::PLAIN);
    again.put_cell(1, 0, family, 2, Style::PLAIN);
    again.put(3, 0, "e\u{301}x", Style::PLAIN);
    assert!(painter.frame(&again).is_empty());
    // A terminal that measures character by character: what fits the cells, up to a joiner; the accent stays.
    let thumb = "\u{1f44d}\u{1f3fd}";
    let canvas = pane_row(8, &[(family, 2), (thumb, 2), ("e\u{301}", 1), ("1\u{fe0f}\u{20e3}", 2)]);
    let mut painter = Painter::default();
    painter.frame(&Canvas::new(8, 1));
    assert_eq!(
        text(&painter.frame(&canvas)),
        "\x1b[1;1H\u{1f468}\x1b[1;3H\u{1f44d}\x1b[1;5He\u{301}\x1b[1;6H1\u{fe0f}\u{20e3} "
    );
    // Wide in the last column: the terminal would wrap it, so a blank.
    let mut canvas = Canvas::new(3, 1);
    canvas.put_cell(1, 0, "日", 2, Style::PLAIN);
    canvas.put_cell(2, 0, "本", 2, Style::PLAIN);
    assert_eq!(canvas.row(0), "");
    // VS16 in one column: left out, the terminals that heed it would draw two; in two, kept.
    let mut canvas = Canvas::new(4, 1);
    canvas.put_cell(0, 0, "\u{26a0}\u{fe0f}", 1, Style::PLAIN);
    canvas.put_cell(1, 0, "\u{2764}\u{fe0f}", 2, Style::PLAIN);
    assert_eq!(canvas.row(0), "\u{26a0}\u{2764}\u{fe0f}");
    assert_eq!(canvas.cells[0].glyph, Glyph::Char('\u{26a0}'));
    // A family as an engine that measures character by character gives it, a ZWJ with each of the first two: the
    // ZWJ left out, each one alone.
    let canvas = pane_row(6, &[("\u{1f468}\u{200d}", 2), ("\u{1f469}\u{200d}", 2), ("\u{1f467}", 2)]);
    assert_eq!(canvas.row(0), "\u{1f468}\u{1f469}\u{1f467}");
    assert!(canvas.clusters.is_empty());
    // Direction controls left out: a pane is never reordered.
    let canvas = pane_row(3, &[("a\u{202e}", 1), ("\u{2067}", 1), ("b\u{301}\u{2069}", 1)]);
    assert_eq!(canvas.row(0), "a b\u{301}");
    // A grapheme past its bytes is cut on a character.
    let mut canvas = Canvas::new(2, 1);
    canvas.put_cell(0, 0, &format!("e{}", "\u{301}".repeat(40)), 1, Style::PLAIN);
    assert_eq!(canvas.row(0), format!("e{}", "\u{301}".repeat(31)));
}

#[test]
fn links_by_their_uri() {
    let linked = |uris: &[&str], uri: &str| {
        let mut canvas = Canvas::new(6, 1);
        for other in uris {
            canvas.link(other);
        }
        let link = canvas.link(uri);
        canvas.put(0, 0, "ab", Style { link: Some(link), ..Style::PLAIN });
        canvas.put(2, 0, "c", Style::PLAIN);
        canvas
    };
    let mut painter = Painter::new(Features { links: true, ..Features::default() });
    painter.frame(&Canvas::new(6, 1));
    let first = linked(&[], "https://a.example");
    assert_eq!(first.link_at(1, 0), Some("https://a.example"));
    assert_eq!(text(&painter.frame(&first)), "\x1b[1;1H\x1b]8;;https://a.example\x1b\\ab\x1b]8;;\x1b\\c");
    // Another id for the same URI: the same frame.
    assert!(painter.frame(&linked(&["https://b.example"], "https://a.example")).is_empty());
    // The same id for another URI: sent again, and the link closed at the end.
    assert_eq!(
        text(&painter.frame(&linked(&[], "https://b.example"))),
        "\x1b[1;1H\x1b]8;;https://b.example\x1b\\ab\x1b]8;;\x1b\\"
    );
    // A URI that would end the sequence early goes without its link.
    assert_eq!(text(&painter.frame(&linked(&[], "https://c\x1b\\x"))), "\x1b[1;1Hab");
    // A terminal without links: the text only.
    let mut painter = Painter::default();
    painter.frame(&Canvas::new(6, 1));
    assert_eq!(text(&painter.frame(&linked(&[], "https://a.example"))), "\x1b[1;1Habc");
}

#[test]
fn the_cursor_last() {
    let mut painter = Painter::default();
    let mut canvas = Canvas::new(4, 2);
    // No cursor: it stays hidden, as the screen was set up; nothing said.
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[0m\x1b[2J\x1b[1;1H    \x1b[2;1H    ");
    assert!(painter.frame(&canvas).is_empty());
    let bar = Cursor { x: 2, y: 1, shape: CursorShape::Bar, blink: true };
    canvas.set_cursor(Some(bar));
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;3H\x1b[5 q\x1b[?25h");
    assert!(painter.frame(&canvas).is_empty(), "still there, nothing sent");
    // Cells drawn: the cursor hidden meanwhile, put back after.
    canvas.put(0, 0, "x", Style::PLAIN);
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[?25l\x1b[1;1Hx\x1b[2;3H\x1b[?25h");
    // Moved, steady: only that.
    canvas.set_cursor(Some(Cursor { x: 0, blink: false, ..bar }));
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;1H\x1b[6 q");
    canvas.set_cursor(Some(Cursor { x: 9, ..bar }));
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[?25l", "off the canvas: hidden");
    // The user's own shape: DECSCUSR 0, whatever the blink.
    canvas.set_cursor(Some(Cursor { shape: CursorShape::Default, ..bar }));
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[2;3H\x1b[0 q\x1b[?25h");
}

#[test]
fn synchronized_frames() {
    let mut painter = Painter::new(Features { sync: true, ..Features::default() });
    let mut canvas = Canvas::new(4, 1);
    canvas.set_cursor(Some(Cursor { x: 0, y: 0, shape: CursorShape::Block, blink: false }));
    painter.frame(&canvas);
    assert!(painter.frame(&canvas).is_empty(), "nothing to frame");
    canvas.put(1, 0, "y", Style::PLAIN);
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[?2026h\x1b[1;2Hy\x1b[1;1H\x1b[?2026l");
}

#[test]
fn wide_characters_go_whole() {
    let mut painter = Painter::default();
    let mut canvas = Canvas::new(6, 1);
    canvas.put(0, 0, "ab", Style::PLAIN);
    painter.frame(&canvas);
    canvas.put(0, 0, "日", Style::PLAIN);
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[1;1H日");
    canvas.put(1, 0, "c", Style::PLAIN);
    assert_eq!(text(&painter.frame(&canvas)), "\x1b[1;1H c");
}

#[test]
fn a_new_size_or_a_lent_terminal_sends_it_all() {
    let mut painter = Painter::default();
    let canvas = Canvas::new(4, 1);
    painter.frame(&canvas);
    assert!(text(&painter.frame(&Canvas::new(5, 1))).starts_with("\x1b[0m\x1b[2J"));
    painter.forget();
    assert!(text(&painter.frame(&canvas)).starts_with("\x1b[0m\x1b[2J"));
}

fn rows(text: &str, width: usize, how: Wrap) -> Vec<String> {
    wrap(text, width, how)
}

#[test]
fn text_wrapped_between_words() {
    let split = Wrap::new(Long::Split);
    assert_eq!(rows("un  deux\ttrois", 8, split), ["un deux", "trois"]);
    assert_eq!(rows("", 8, split), [""]);
    assert_eq!(rows("  \n ", 8, split), [""]);
    assert_eq!(rows("日本語 日本語", 6, split), ["日本語", "日本語"]);
    // A word longer than a row: broken where it must, one character at least, however wide.
    assert_eq!(rows("ab abcdefghij", 4, split), ["ab", "abcd", "efgh", "ij"]);
    assert_eq!(rows("日本", 1, split), ["日", "本"]);
    // The last piece takes the next word.
    assert_eq!(rows("abcdef gh", 4, split), ["abcd", "ef", "gh"]);
    assert_eq!(rows("abcdef g", 4, split), ["abcd", "ef g"]);
    // Whole: alone on a row, left as it is; Ellipsis: alone on a row, cut.
    assert_eq!(rows("ab abcdefghij cd", 4, Wrap::new(Long::Whole)), ["ab", "abcdefghij", "cd"]);
    assert_eq!(rows("ab abcdefghij cd", 4, Wrap::new(Long::Ellipsis)), ["ab", "abc…", "cd"]);
    assert_eq!(rows("abcdefghij", 0, Wrap::new(Long::Ellipsis)), [""]);
}

#[test]
fn text_wrapped_in_a_number_of_rows() {
    let text = "Je publie la version 0.6.2 de recruit";
    assert_eq!(rows(text, 20, Wrap::new(Long::Split).lines(2)), ["Je publie la version", "0.6.2 de recruit"]);
    // The last row takes the rest, cut with an ellipsis; one row at least.
    assert_eq!(rows(text, 20, Wrap::new(Long::Split).lines(1)), ["Je publie la versio…"]);
    assert_eq!(rows(text, 20, Wrap::new(Long::Split).lines(0)), ["Je publie la versio…"]);
    assert_eq!(rows("un deux trois quatre cinq", 10, Wrap::new(Long::Ellipsis).lines(2)), ["un deux", "trois qua…"]);
    assert_eq!(rows("abcdefghijklmnop qr", 6, Wrap::new(Long::Split).lines(2)), ["abcdef", "ghijk…"]);
    assert_eq!(rows(text, 8, Wrap::new(Long::Whole).lines(usize::MAX)).len(), 6);
}

#[test]
fn text_wrapped_as_measured() {
    // Character by character, a heart with its selector is one column; whole, it is two.
    let heart = "abc \u{2764}\u{fe0f}";
    assert_eq!(rows(heart, 5, Wrap::new(Long::Whole)), [heart]);
    assert_eq!(rows(heart, 5, Wrap::new(Long::Whole).measured(text_columns)), ["abc", "\u{2764}\u{fe0f}"]);
}

#[test]
fn chars_wrapped_keep_their_spaces() {
    let tie = |text: &str| text.chars().map(|c| (c, c.is_uppercase())).collect::<Vec<_>>();
    let flat = |rows: Vec<Vec<(char, bool)>>| {
        rows.into_iter().map(|r| r.into_iter().map(|(c, _)| c).collect()).collect::<Vec<String>>()
    };
    let split = |text: &str, width| flat(wrap_chars(&tie(text), width, Long::Split, text_columns));
    assert_eq!(split("un    deux trois", 8), ["un", "deux", "trois"]);
    assert_eq!(split("a   b", 8), ["a   b"]);
    assert_eq!(split("  retrait", 20), ["  retrait"]);
    assert_eq!(split("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    assert_eq!(split("", 4), [""]);
    // A word that fits whole stays whole: its prefix « a⌚ » is wider (3) than the word with its text selector (2).
    assert_eq!(split("a\u{231a}\u{fe0e}", 2), ["a\u{231a}\u{fe0e}"]);
    assert_eq!(split("b a\u{231a}\u{fe0e}", 4), ["b a\u{231a}\u{fe0e}"]);
    // Not on the row but alone on a row: a row of its own, whole; broken only if wider than a row.
    assert_eq!(split("b a\u{231a}\u{fe0e}", 2), ["b", "a\u{231a}\u{fe0e}"]);
    assert_eq!(split("  a\u{231a}\u{fe0e}", 2), ["a\u{231a}\u{fe0e}"]);
    assert_eq!(split("b a\u{231a}\u{fe0e}", 1), ["b", "a", "\u{231a}\u{fe0e}"]);
    // What was tied to a character stays with it.
    let rows = wrap_chars(&tie("aB cD"), 3, Long::Split, text_columns);
    assert_eq!(rows[1], [('c', false), ('D', true)]);
}
