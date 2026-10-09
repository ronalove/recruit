// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The lists of choices (`ui::select`, `ui::multi_select`), drawn by recruit itself: inquire shows the terminal's
//! cursor after the question at each frame, and nothing in it hides that. Here the cursor stays hidden while the list
//! is on screen. What the list holds and what each key does ([`List`]) is apart from the terminal ([`run`]).

use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Stderr, Write};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Color, Print, PrintStyledContent, Stylize};
use crossterm::{cursor, queue, terminal};
use unicode_width::UnicodeWidthStr;

use super::Cancelled;
use crate::canvas::PanicGuard;
use crate::t;

/// Choices on screen at once, at most.
const PAGE: usize = 12;

/// Rows narrower than this are not worth wrapping to.
const NARROWEST: usize = 10;

/// Choices worth keeping the list below the cursor for; with fewer, it takes the whole screen.
const BELOW: usize = 3;

/// What a piece of a row looks like (inquire's colors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Paint {
    Plain,
    /// The question's sign, a ticked box.
    Mark,
    /// The highlighted choice, the answer, the help.
    Chosen,
    Error,
    Canceled,
}

impl Paint {
    fn color(self) -> Option<Color> {
        match self {
            Paint::Plain => None,
            Paint::Mark => Some(Color::Green),
            Paint::Chosen => Some(Color::Cyan),
            Paint::Error => Some(Color::Red),
            Paint::Canceled => Some(Color::DarkRed),
        }
    }
}

/// One row on screen, in pieces.
type Row = Vec<(String, Paint)>;

/// A key, as the list understands it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    /// Every choice the filter keeps, in a list with boxes.
    Right,
    /// No choice, in a list with boxes.
    Left,
    /// Typed: into the filter, or, for a space in a list with boxes, a tick.
    Char(char),
    Erase,
    EraseWord,
    EraseAll,
    Enter,
    /// Esc or Ctrl-C.
    Cancel,
}

/// What a key leaves the list at.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Stay,
    Done,
    Cancel,
}

fn key_of(event: KeyEvent) -> Option<Key> {
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
    let alt = event.modifiers.contains(KeyModifiers::ALT);
    Some(match event.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Char('p') if ctrl => Key::Up,
        KeyCode::Char('n') if ctrl => Key::Down,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Right => Key::Right,
        KeyCode::Left => Key::Left,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Cancel,
        KeyCode::Char('c') if ctrl => Key::Cancel,
        KeyCode::Char('w') if ctrl => Key::EraseWord,
        KeyCode::Char('u') if ctrl => Key::EraseAll,
        // ^H: what some terminals send for Backspace.
        KeyCode::Char('h') if ctrl => Key::Erase,
        KeyCode::Backspace if alt => Key::EraseWord,
        KeyCode::Backspace => Key::Erase,
        KeyCode::Char(c) if !ctrl && !alt => Key::Char(c),
        _ => return None,
    })
}

/// The choices, the filter typed so far, the highlighted choice and, in a list with boxes, the ticked ones.
pub(super) struct List {
    labels: Vec<String>,
    /// The labels as the filter reads them.
    folded: Vec<Vec<char>>,
    filter: String,
    /// The choices the filter keeps (indexes in `labels`), best first.
    shown: Vec<usize>,
    /// The highlighted choice, in `shown`.
    at: usize,
    /// The ticked choices, in a list with boxes.
    ticked: Option<BTreeSet<usize>>,
    max: Option<usize>,
    error: Option<String>,
    /// Choices on screen at once, from the last frame.
    page: usize,
}

impl List {
    pub(super) fn single(labels: Vec<String>) -> Self {
        Self::new(labels, None, None)
    }

    /// `checked`: indexes of the choices ticked at first; `max`: how many can be ticked at most.
    pub(super) fn multi(labels: Vec<String>, checked: &[usize], max: Option<usize>) -> Self {
        let ticked = checked.iter().copied().filter(|&i| i < labels.len()).collect();
        Self::new(labels, Some(ticked), max)
    }

    fn new(labels: Vec<String>, ticked: Option<BTreeSet<usize>>, max: Option<usize>) -> Self {
        let folded = labels.iter().map(|l| fold(l)).collect();
        let shown = (0..labels.len()).collect();
        List { labels, folded, filter: String::new(), shown, at: 0, ticked, max, error: None, page: PAGE }
    }

    fn press(&mut self, key: Key) -> Step {
        let page = self.page;
        match key {
            Key::Up => self.at = self.at.checked_sub(1).unwrap_or(self.shown.len().saturating_sub(1)),
            Key::Down => self.at = if self.at + 1 >= self.shown.len() { 0 } else { self.at + 1 },
            Key::PageUp => self.at = self.at.saturating_sub(page),
            Key::PageDown => self.at = (self.at + page).min(self.shown.len().saturating_sub(1)),
            Key::Home => self.at = 0,
            Key::End => self.at = self.shown.len().saturating_sub(1),
            Key::Char(' ') if self.ticked.is_some() => {
                if let (Some(ticked), Some(&i)) = (self.ticked.as_mut(), self.shown.get(self.at))
                    && !ticked.remove(&i)
                {
                    ticked.insert(i);
                }
                self.error = None;
            }
            Key::Right | Key::Left => {
                if let Some(ticked) = self.ticked.as_mut() {
                    ticked.clear();
                    if key == Key::Right {
                        ticked.extend(&self.shown);
                    }
                    self.error = None;
                }
            }
            Key::Char(c) if !c.is_control() => self.refilter(|f| f.push(c)),
            Key::Char(_) => {}
            Key::Erase => self.refilter(|f| {
                f.pop();
            }),
            Key::EraseWord => self.refilter(|f| {
                let kept = f.trim_end().rfind(' ').map_or(0, |at| at + 1);
                f.truncate(kept);
            }),
            Key::EraseAll => self.refilter(String::clear),
            Key::Enter => return self.submit(),
            Key::Cancel => return Step::Cancel,
        }
        Step::Stay
    }

    /// A paste goes into the filter, without its line breaks.
    fn paste(&mut self, text: &str) {
        self.refilter(|f| f.extend(text.chars().filter(|c| !c.is_control())));
    }

    fn submit(&mut self) -> Step {
        match (&self.ticked, self.max) {
            (Some(ticked), Some(max)) if ticked.len() > max => {
                self.error = Some(t!("{} au plus", "{} at most", max));
                Step::Stay
            }
            (Some(_), _) => Step::Done,
            // Nothing to pick when the filter keeps no choice.
            (None, _) if self.shown.is_empty() => Step::Stay,
            (None, _) => Step::Done,
        }
    }

    fn refilter(&mut self, change: impl FnOnce(&mut String)) {
        change(&mut self.filter);
        let filter = fold(&self.filter);
        let mut scored: Vec<(usize, i64)> =
            self.folded.iter().enumerate().filter_map(|(i, l)| score(l, &filter).map(|s| (i, s))).collect();
        scored.sort_by_key(|&(_, s)| Reverse(s));
        let shown: Vec<usize> = scored.into_iter().map(|(i, _)| i).collect();
        if shown != self.shown {
            self.shown = shown;
            self.at = 0;
        }
    }

    /// The indexes of what was chosen, in the order of the choices.
    fn chosen(&self) -> Vec<usize> {
        match &self.ticked {
            Some(ticked) => ticked.iter().copied().collect(),
            None => self.shown.get(self.at).copied().into_iter().collect(),
        }
    }

    /// The labels chosen, as the answer reads.
    fn answer(&self) -> String {
        self.chosen().iter().map(|&i| self.labels[i].as_str()).collect::<Vec<_>>().join(", ")
    }

    /// The list on a terminal `cols` wide: in the `below` rows under the cursor when a few choices fit there, else in
    /// its `lines` rows, what is above going up.
    fn frame(&mut self, message: &str, help: &str, cols: usize, below: usize, lines: usize) -> Vec<Row> {
        let (rows, roomy) = self.fit(message, help, cols, below);
        if roomy || below >= lines {
            return rows;
        }
        self.fit(message, help, cols, lines).0
    }

    /// The list in `lines` rows at most: what goes wrong, the question and the filter, as many choices as fit, the
    /// help. Short of room, the help goes first, then the question's first rows, and what goes wrong last (without
    /// it, Enter would be refused for no reason seen). And whether a few choices fit with everything else.
    fn fit(&mut self, message: &str, help: &str, cols: usize, lines: usize) -> (Vec<Row>, bool) {
        let width = width(cols);
        let error = match &self.error {
            Some(error) => wrap(&[("#", Paint::Error), (" ", Paint::Plain), (error, Paint::Error)], width),
            None => Vec::new(),
        };
        let mut pieces = vec![("?", Paint::Mark), (" ", Paint::Plain), (message, Paint::Plain)];
        if !self.filter.is_empty() {
            pieces.extend([(" ", Paint::Plain), (self.filter.as_str(), Paint::Plain)]);
        }
        let mut question = wrap(&pieces, width);
        let mut help = wrap(&[("[", Paint::Chosen), (help, Paint::Chosen), ("]", Paint::Chosen)], width);
        let mut choices = self.choices(width, lines.saturating_sub(error.len() + question.len() + help.len()));
        let mut roomy = self.page >= BELOW.min(self.shown.len());
        if error.len() + question.len() + choices.len() + help.len() > lines {
            help.clear();
            choices = self.choices(width, lines.saturating_sub(error.len() + question.len()));
            roomy = false;
        }
        let over = (error.len() + question.len() + choices.len()).saturating_sub(lines);
        if over > 0 {
            question.drain(..over.min(question.len()));
            roomy = false;
        }
        let mut rows = [error, question, choices, help].concat();
        rows.drain(..rows.len().saturating_sub(lines));
        (rows, roomy)
    }

    /// As many choices as their rows leave room for in `room` rows, one at least: the page is what fits.
    fn choices(&mut self, width: usize, room: usize) -> Vec<Row> {
        self.page = PAGE;
        loop {
            let (start, end) = self.window();
            let rows: Vec<Row> = (start..end).flat_map(|at| self.choice(at, start, end, width)).collect();
            if rows.len() <= room || self.page == 1 {
                return rows;
            }
            self.page -= 1;
        }
    }

    /// The choices on screen, in `shown`: the highlighted one kept in the middle when the list scrolls.
    fn window(&self) -> (usize, usize) {
        let (count, page, at) = (self.shown.len(), self.page, self.at);
        if count <= page || at < page / 2 {
            (0, page.min(count))
        } else if count - at - 1 < page / 2 {
            (count - page, count)
        } else {
            (at - page / 2, at - page / 2 + page)
        }
    }

    /// The choice at `at` in `shown`, its label broken between words under itself when it is too long.
    fn choice(&self, at: usize, start: usize, end: usize, width: usize) -> Vec<Row> {
        let i = self.shown[at];
        let here = at == self.at;
        let sign = if here {
            (">", Paint::Chosen)
        } else if at == start && start > 0 {
            ("^", Paint::Plain)
        } else if at + 1 == end && end < self.shown.len() {
            ("v", Paint::Plain)
        } else {
            (" ", Paint::Plain)
        };
        let mut row: Row = vec![(sign.0.into(), sign.1), (" ".into(), Paint::Plain)];
        if let Some(ticked) = &self.ticked {
            let tick = if ticked.contains(&i) { ("[x]", Paint::Mark) } else { ("[ ]", Paint::Plain) };
            row.push((tick.0.into(), if here { Paint::Chosen } else { tick.1 }));
            row.push((" ".into(), Paint::Plain));
        }
        let used = columns(&row);
        let paint = if here { Paint::Chosen } else { Paint::Plain };
        let mut label = wrap(&[(&self.labels[i], paint)], width.saturating_sub(used)).into_iter();
        row.extend(label.next().unwrap_or_default());
        let mut rows = vec![row];
        rows.extend(label.map(|more| [vec![(" ".repeat(used), Paint::Plain)], more].concat()));
        rows
    }
}

/// The question as it stays on screen once left, after `sign`: its answer, or that it was canceled.
fn settled(sign: &str, message: &str, answer: &str, paint: Paint, cols: usize) -> Vec<Row> {
    let pieces =
        [(sign, Paint::Mark), (" ", Paint::Plain), (message, Paint::Plain), (" ", Paint::Plain), (answer, paint)];
    wrap(&pieces, width(cols))
}

/// The widest rows on a terminal `cols` wide: a column to spare, the cursor never left past the last one.
fn width(cols: usize) -> usize {
    cols.saturating_sub(1).max(NARROWEST)
}

/// Columns a row takes on screen: two for a wide character (日, 🚀), none for an accent on its own. Measured on the
/// whole text, as `wrap` measures it: an emoji sequence (❤️, 👩‍💻) is not the sum of its characters.
fn columns(row: &Row) -> usize {
    row.iter().map(|(text, _)| text.as_str()).collect::<String>().width()
}

/// Lowercase, without accents: « equipe » finds « Équipe », « oeuvre » finds « Œuvre ».
fn fold(text: &str) -> Vec<char> {
    let mut folded = Vec::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            'œ' => folded.extend(['o', 'e']),
            'æ' => folded.extend(['a', 'e']),
            'à' | 'â' | 'ä' | 'á' | 'ã' => folded.push('a'),
            'é' | 'è' | 'ê' | 'ë' => folded.push('e'),
            'î' | 'ï' | 'í' | 'ì' => folded.push('i'),
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => folded.push('o'),
            'ù' | 'û' | 'ü' | 'ú' => folded.push('u'),
            'ç' => folded.push('c'),
            'ÿ' => folded.push('y'),
            'ñ' => folded.push('n'),
            c => folded.push(c),
        }
    }
    folded
}

/// How well a label matches the filter, both folded; None when it does not. Best: the filter whole at the start of a
/// word, then the filter whole, the earlier the better; then its letters in order, the closer together the better.
fn score(label: &[char], filter: &[char]) -> Option<i64> {
    if filter.is_empty() {
        return Some(0);
    }
    let whole = (0..label.len().saturating_sub(filter.len() - 1))
        .filter(|&at| label[at..].starts_with(filter))
        .map(|at| {
            let word = at == 0 || !label[at - 1].is_alphanumeric();
            let base = if word { 3_000_000 } else { 2_000_000 };
            base - at as i64
        })
        .max();
    if whole.is_some() {
        return whole;
    }
    let mut next = 0;
    let mut first = None;
    for (at, c) in label.iter().enumerate() {
        if *c == filter[next] {
            let first = *first.get_or_insert(at);
            next += 1;
            if next == filter.len() {
                return Some(1_000_000 - (at + 1 - first - filter.len()) as i64);
            }
        }
    }
    None
}

/// The pieces on rows of `width` columns at most, broken between words (a word too long is broken where it must).
/// The spaces stay as they are (a label may line up columns with them, a filter may end with one), except where a row
/// breaks.
fn wrap(pieces: &[(&str, Paint)], width: usize) -> Vec<Row> {
    let chars: Vec<(char, Paint)> = pieces.iter().flat_map(|(t, p)| t.chars().map(move |c| (c, *p))).collect();
    // Measured as text, like `columns`.
    let wide = |chars: &[(char, Paint)]| chars.iter().map(|&(c, _)| c).collect::<String>().width();
    let mut rows: Vec<Vec<(char, Paint)>> = vec![Vec::new()];
    let mut rest = chars.as_slice();
    while !rest.is_empty() {
        let gap = rest.iter().take_while(|(c, _)| *c == ' ').count();
        let len = rest[gap..].iter().take_while(|(c, _)| *c != ' ').count();
        let (spaces, word) = (&rest[..gap], &rest[gap..gap + len]);
        rest = &rest[gap + len..];
        let row = rows.last_mut().expect("a row");
        if wide(&[row.as_slice(), spaces, word].concat()) <= width {
            row.extend(spaces);
        } else if !row.is_empty() && !word.is_empty() {
            rows.push(Vec::new());
        }
        for &c in word {
            let row = rows.last_mut().expect("a row");
            row.push(c);
            if row.len() > 1 && wide(row) > width {
                row.pop();
                rows.push(vec![c]);
            }
        }
    }
    rows.into_iter()
        .map(|row| {
            let mut pieces: Row = Vec::new();
            for (c, paint) in row {
                match pieces.last_mut() {
                    Some((text, p)) if *p == paint => text.push(c),
                    _ => pieces.push((c.to_string(), paint)),
                }
            }
            pieces
        })
        .collect()
}

/// The bytes of a frame of `rows` on a terminal `cols` wide, drawn over the frame before, whose rows took `up` lines
/// above the last.
fn frame(rows: &[Row], cols: usize, up: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    queue!(out, terminal::BeginSynchronizedUpdate, cursor::MoveToColumn(0))?;
    if up > 0 {
        queue!(out, cursor::MoveUp(up.min(u16::MAX as usize) as u16))?;
    }
    // Cleared whole, the rows of the frame before lose the mark tmux leaves on a row it broke in two when the
    // terminal got narrower: only written over, they would be joined again when it gets wider, the frame shorter
    // than counted, and the lines above it erased. The first row is cleared alone, and the rest once it is
    // written: cleared from the top left corner, the whole screen would go into tmux's history (scroll-on-clear),
    // once more at each key.
    queue!(out, terminal::Clear(terminal::ClearType::CurrentLine))?;
    // The first row filling its last line leaves the cursor on its last cell, the wrap pending: ED would erase
    // that cell (xterm, Ghostty and recruit's own multiplexer do). Cleared from the start of the next line instead.
    let full = rows.first().is_some_and(|row| columns(row) > 0 && columns(row).is_multiple_of(cols.max(1)));
    for (n, row) in rows.iter().enumerate() {
        if n > 0 && !(n == 1 && full) {
            queue!(out, Print("\r\n"))?;
        }
        for (text, paint) in row {
            let styled = match paint.color() {
                Some(color) => text.as_str().with(color),
                None => text.as_str().stylize(),
            };
            queue!(out, PrintStyledContent(styled))?;
        }
        if n == 0 && full {
            queue!(out, Print("\r\n"), terminal::Clear(terminal::ClearType::FromCursorDown))?;
            if rows.len() == 1 {
                queue!(out, cursor::MoveUp(1))?;
            }
        } else if n == 0 {
            queue!(out, terminal::Clear(terminal::ClearType::FromCursorDown))?;
        }
    }
    queue!(out, terminal::EndSynchronizedUpdate)?;
    Ok(out)
}

/// How many rows up the first row of a frame is from its last, its rows `drawn` columns wide, on a terminal `cols`
/// wide: one made narrower since may have broken them in several.
fn rows_up(drawn: &[usize], cols: usize) -> usize {
    drawn.iter().map(|&width| width.div_ceil(cols.max(1)).max(1)).sum::<usize>().saturating_sub(1)
}

/// Runs the list until a choice is made: the indexes chosen, or [`Cancelled`].
pub(super) fn run(mut list: List, message: &str, help: &str) -> Result<Vec<usize>> {
    if list.labels.is_empty() {
        bail!("{}", t!("Aucun choix possible.", "Nothing to choose from."));
    }
    let mut screen = Screen::open()?;
    loop {
        let (cols, lines) = size();
        let below = screen.top.map_or(lines, |top| lines.saturating_sub(top));
        screen.draw(&list.frame(message, help, cols, below, lines), cols, lines)?;
        let key = match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => key_of(key),
            Event::Paste(text) => {
                list.paste(&text);
                None
            }
            Event::Resize(..) => {
                screen.resized();
                None
            }
            _ => None,
        };
        let (cols, lines) = size();
        match key.map(|key| list.press(key)) {
            None | Some(Step::Stay) => {}
            Some(Step::Done) => {
                screen.close(&settled(">", message, &list.answer(), Paint::Chosen, cols), cols, lines)?;
                return Ok(list.chosen());
            }
            Some(Step::Cancel) => {
                let canceled = t!("<annulé>", "<canceled>");
                screen.close(&settled("?", message, &canceled, Paint::Canceled, cols), cols, lines)?;
                return Err(Cancelled.into());
            }
        }
    }
}

/// The cursor's row on screen. Asked on stdout, which crossterm writes the question to: redirected, it would wait
/// for an answer 2 s and leave the question in the output.
fn cursor_row() -> Option<usize> {
    io::stdout().is_terminal().then(cursor::position).and_then(Result::ok).map(|(_, row)| row as usize)
}

fn size() -> (usize, usize) {
    terminal::size().map_or((80, 24), |(cols, lines)| (cols as usize, lines as usize))
}

/// The terminal while a list is on it: raw, without its cursor. Dropping it gives the cursor back, and the terminal's
/// modes, however the list ends (an answer, Esc, an error); a panic does it before its message.
struct Screen {
    out: Stderr,
    /// How wide each row of the frame on screen is; the cursor is at the end of the last one.
    drawn: Vec<usize>,
    /// The screen row of the frame's first row, when the terminal tells where its cursor is.
    top: Option<usize>,
    _panic: PanicGuard,
}

impl Screen {
    fn open() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // From here, Drop puts the terminal back.
        let mut screen =
            Screen { out: io::stderr(), drawn: Vec::new(), top: None, _panic: PanicGuard::install(restore) };
        queue!(screen.out, cursor::Hide, event::EnableBracketedPaste)?;
        screen.out.flush()?;
        screen.top = cursor_row();
        Ok(screen)
    }

    /// The terminal changed size, and may have moved the frame: where its first row is now.
    fn resized(&mut self) {
        let (cols, _) = size();
        self.top = cursor_row().map(|row| row.saturating_sub(rows_up(&self.drawn, cols)));
    }

    /// Replaces the frame on screen with `rows`, on a terminal `cols` × `lines`.
    fn draw(&mut self, rows: &[Row], cols: usize, lines: usize) -> io::Result<()> {
        let frame = frame(rows, cols, rows_up(&self.drawn, cols))?;
        self.out.write_all(&frame)?;
        self.drawn = rows.iter().map(columns).collect();
        // Past the bottom of the screen, what was on it went up.
        self.top = self.top.map(|top| top.min(lines.saturating_sub(rows.len())));
        self.out.flush()
    }

    /// Leaves `rows` in place of the frame, and the cursor on the line below.
    fn close(&mut self, rows: &[Row], cols: usize, lines: usize) -> io::Result<()> {
        self.draw(rows, cols, lines)?;
        queue!(self.out, Print("\r\n"))?;
        self.drawn.clear();
        self.out.flush()
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // A panic: its hook did it already.
        if std::thread::panicking() {
            return;
        }
        // Left in the middle (an error): what follows goes below the frame.
        if !self.drawn.is_empty() {
            let _ = queue!(self.out, Print("\r\n"));
        }
        let _ = queue!(self.out, event::DisableBracketedPaste, cursor::Show);
        let _ = self.out.flush();
        let _ = terminal::disable_raw_mode();
    }
}

/// The terminal as before a list, from a panic's hook: the cursor shown, out of raw mode, and on a line of its own for
/// the message (in raw mode, it would go down in steps).
fn restore() {
    let mut out = io::stderr();
    let _ = queue!(out, Print("\r\n"), event::DisableBracketedPaste, cursor::Show);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("choix {i}")).collect()
    }

    fn press(list: &mut List, keys: &[Key]) -> Step {
        let mut step = Step::Stay;
        for &key in keys {
            step = list.press(key);
        }
        step
    }

    fn typed(list: &mut List, text: &str) {
        for c in text.chars() {
            list.press(Key::Char(c));
        }
    }

    fn text(row: &Row) -> String {
        row.iter().map(|(text, _)| text.as_str()).collect()
    }

    fn texts(rows: &[Row]) -> Vec<String> {
        rows.iter().map(text).collect()
    }

    /// The list on a screen `cols` × `lines`, the cursor at its top, with « aide » for help.
    fn frame(list: &mut List, message: &str, cols: usize, lines: usize) -> Vec<String> {
        texts(&list.frame(message, "aide", cols, lines, lines))
    }

    #[test]
    fn moves_round_and_by_page() {
        let mut list = List::single(labels(30));
        assert_eq!(list.chosen(), [0]);
        list.press(Key::Up);
        assert_eq!(list.chosen(), [29], "up from the first goes round to the last");
        list.press(Key::Down);
        assert_eq!(list.chosen(), [0], "down from the last goes round to the first");
        press(&mut list, &[Key::PageDown, Key::PageDown]);
        assert_eq!(list.chosen(), [24]);
        list.press(Key::PageDown);
        assert_eq!(list.chosen(), [29], "a page down stops at the last");
        list.press(Key::PageUp);
        assert_eq!(list.chosen(), [17]);
        press(&mut list, &[Key::PageUp, Key::PageUp]);
        assert_eq!(list.chosen(), [0], "a page up stops at the first");
        list.press(Key::End);
        assert_eq!(list.chosen(), [29]);
        list.press(Key::Home);
        assert_eq!(list.chosen(), [0]);
        assert_eq!(list.press(Key::Enter), Step::Done);
    }

    #[test]
    fn left_and_right_do_nothing_in_a_single_list() {
        let mut list = List::single(labels(3));
        list.press(Key::Down);
        assert_eq!(press(&mut list, &[Key::Left, Key::Right]), Step::Stay);
        assert_eq!(list.chosen(), [1]);
        assert_eq!(list.filter, "");
    }

    #[test]
    fn scrolls_with_the_highlight_in_the_middle() {
        let mut list = List::single(labels(20));
        let rows = frame(&mut list, "Lequel ?", 80, 40);
        assert_eq!(rows.len(), 1 + PAGE + 1, "the question, a page, the help");
        assert_eq!(rows[0], "? Lequel ?");
        assert_eq!(rows[1], "> choix 0");
        assert_eq!(rows[12], "v choix 11", "more below");
        assert_eq!(rows[13], "[aide]");

        press(&mut list, &[Key::Down; 9]);
        let rows = frame(&mut list, "Lequel ?", 80, 40);
        assert_eq!(rows[1], "^ choix 3", "more above");
        assert_eq!(rows[7], "> choix 9", "in the middle of the page");
        assert_eq!(rows[12], "v choix 14");

        list.press(Key::End);
        let rows = frame(&mut list, "Lequel ?", 80, 40);
        assert_eq!(rows[1], "^ choix 8");
        assert_eq!(rows[12], "> choix 19", "the last page ends with the list");
    }

    #[test]
    fn a_short_screen_shows_fewer_choices() {
        let mut list = List::single(labels(20));
        let rows = frame(&mut list, "Lequel ?", 80, 7);
        assert_eq!(rows.len(), 7, "the frame fits the screen");
        assert_eq!(list.page, 5);
        list.press(Key::PageDown);
        assert_eq!(list.chosen(), [5], "a page is what fits");
        assert_eq!(frame(&mut list, "Lequel ?", 80, 2), ["? Lequel ?", "> choix 5"], "the help goes first");
        assert_eq!(frame(&mut list, "Lequel ?", 80, 1), ["> choix 5"], "then the question");
    }

    #[test]
    fn never_past_the_bottom() {
        let question = "Qui sont tes interlocuteurs (deux au plus) ? Ils seront dans le premier onglet.";
        let help = "↑↓ pour se déplacer, Espace pour cocher, Entrée pour valider";
        let mut list = List::multi(labels(5), &[], None);
        list.press(Key::Down);
        let rows = texts(&list.frame(question, help, 30, 5, 5));
        assert_eq!(rows.len(), 5, "at 30 × 5");
        assert!(!rows.iter().any(|r| r.starts_with('[')), "the help went");
        assert!(rows.contains(&"> [ ] choix 1".to_string()), "the highlighted choice stays: {rows:?}");
    }

    #[test]
    fn stays_below_the_cursor_when_a_few_choices_fit() {
        let mut list = List::single(labels(20));
        let rows = list.frame("Lequel ?", "aide", 80, 8, 40);
        assert_eq!((rows.len(), list.page), (8, 6), "under the menu's title");
        let rows = list.frame("Lequel ?", "aide", 80, 5, 40);
        assert_eq!((rows.len(), list.page), (5, BELOW), "three choices still fit");
        let rows = list.frame("Lequel ?", "aide", 80, 4, 40);
        assert_eq!((rows.len(), list.page), (14, PAGE), "two do not: the whole screen");
        let mut short = List::single(labels(2));
        let rows = short.frame("Lequel ?", "aide", 80, 4, 40);
        assert_eq!(rows.len(), 4, "two choices in all fit");
    }

    #[test]
    fn the_filter_narrows_and_ranks() {
        let names = ["Retirer", "Relancer", "Modèle", "Rôle", "Tableau de bord"];
        let mut list = List::single(names.map(String::from).to_vec());
        list.press(Key::Down);
        typed(&mut list, "re");
        let rows = frame(&mut list, "Quoi ?", 80, 40);
        assert_eq!(rows[0], "? Quoi ? re", "the filter after the question");
        assert_eq!(rows[1..rows.len() - 1], ["> Retirer", "  Relancer", "  Rôle"], "« re » whole first, then r…e");
        assert_eq!(list.chosen(), [0], "back to the first when the choices change");

        list.press(Key::EraseAll);
        typed(&mut list, "role");
        assert_eq!(list.shown, [3], "accents aside");
        list.press(Key::EraseAll);
        typed(&mut list, "bord");
        assert_eq!(list.shown, [4]);
        assert_eq!(list.chosen(), [4], "the choice, not its place in the list");
        list.press(Key::EraseAll);
        typed(&mut list, "tbd");
        assert_eq!(list.shown, [4], "letters in order");
        list.press(Key::EraseAll);
        typed(&mut list, "DE");
        assert_eq!(list.shown, [4, 2], "at the start of a word first, case aside");
        list.press(Key::EraseAll);
        typed(&mut list, "le");
        assert_eq!(list.shown, [3, 4, 2, 1], "the earliest first, letters apart last");
    }

    #[test]
    fn the_highlight_stays_while_the_filter_keeps_the_same_choices() {
        let mut list = List::single(vec!["abc".into(), "abd".into()]);
        list.press(Key::Down);
        typed(&mut list, "a");
        assert_eq!(list.chosen(), [1]);
    }

    #[test]
    fn folding() {
        assert_eq!(fold("Œuvre Ægée"), "oeuvre aegee".chars().collect::<Vec<_>>());
        let mut list = List::single(vec!["Main-d'œuvre".into(), "Équipe".into()]);
        typed(&mut list, "oeuv");
        assert_eq!(list.shown, [0]);
        list.press(Key::EraseAll);
        typed(&mut list, "equipe");
        assert_eq!(list.shown, [1]);
    }

    #[test]
    fn a_filter_longer_than_every_label() {
        let mut list = List::single(vec!["ab".into(), "a".into()]);
        typed(&mut list, "abcdef");
        assert!(list.shown.is_empty());
        assert_eq!(frame(&mut list, "Lequel ?", 80, 40), ["? Lequel ? abcdef", "[aide]"]);
    }

    #[test]
    fn erasing_the_filter() {
        let mut list = List::single(vec!["un deux".into(), "trois".into()]);
        typed(&mut list, "un de");
        assert_eq!(list.shown, [0]);
        list.press(Key::Erase);
        assert_eq!(list.filter, "un d");
        list.press(Key::EraseWord);
        assert_eq!(list.filter, "un ");
        list.press(Key::EraseWord);
        assert_eq!(list.filter, "");
        assert_eq!(list.shown, [0, 1]);
        list.press(Key::Erase);
        assert_eq!(list.filter, "", "nothing to erase");
        typed(&mut list, "rôlé");
        list.press(Key::Erase);
        assert_eq!(list.filter, "rôl", "a whole accented letter");
    }

    #[test]
    fn a_space_at_the_end_of_the_filter_shows() {
        let mut list = List::single(vec!["a b".into(), "ab".into()]);
        typed(&mut list, "a ");
        assert_eq!(frame(&mut list, "Quoi ?", 80, 40)[0], "? Quoi ? a ");
    }

    #[test]
    fn nothing_to_pick_when_the_filter_keeps_nothing() {
        let mut list = List::single(labels(3));
        typed(&mut list, "zzz");
        assert!(list.shown.is_empty());
        assert_eq!(frame(&mut list, "Lequel ?", 80, 40), ["? Lequel ? zzz", "[aide]"]);
        assert_eq!(list.press(Key::Down), Step::Stay);
        assert_eq!(list.press(Key::Enter), Step::Stay);
        assert!(list.chosen().is_empty());
    }

    #[test]
    fn nothing_to_choose_from() {
        let error = run(List::single(Vec::new()), "Lequel ?", "aide").unwrap_err();
        assert_eq!(error.to_string(), t!("Aucun choix possible.", "Nothing to choose from."));
    }

    #[test]
    fn a_paste_goes_into_the_filter() {
        let mut list = List::multi(vec!["a b".into(), "ab".into()], &[], None);
        list.paste("a b\n");
        assert_eq!(list.filter, "a b", "its spaces typed, not ticks, and no line break");
        assert!(list.chosen().is_empty());
        assert_eq!(list.shown, [0]);
    }

    #[test]
    fn a_space_is_typed_in_a_single_list() {
        let mut list = List::single(vec!["a b".into(), "ab".into()]);
        typed(&mut list, "a b");
        assert_eq!(list.filter, "a b");
        assert_eq!(list.shown, [0]);
    }

    #[test]
    fn ticks() {
        let mut list = List::multi(labels(4), &[1, 9], None);
        assert_eq!(list.chosen(), [1], "ticked at first, and nothing out of range");
        let rows = frame(&mut list, "Lesquels ?", 80, 40);
        assert_eq!(rows[1..5], ["> [ ] choix 0", "  [x] choix 1", "  [ ] choix 2", "  [ ] choix 3"]);
        press(&mut list, &[Key::Char(' '), Key::Down, Key::Char(' ')]);
        assert_eq!(list.chosen(), [0]);
        assert_eq!(list.filter, "", "a space ticks, it is not typed");
        list.press(Key::Right);
        assert_eq!(list.chosen(), [0, 1, 2, 3]);
        list.press(Key::Left);
        assert!(list.chosen().is_empty());
        typed(&mut list, "2");
        list.press(Key::Right);
        assert_eq!(list.chosen(), [2], "all: every choice the filter keeps");
        list.press(Key::EraseAll);
        press(&mut list, &[Key::End, Key::Char(' ')]);
        assert_eq!(list.chosen(), [2, 3], "the ticks stay through the filter, in the order of the choices");
        assert_eq!(list.press(Key::Enter), Step::Done);
    }

    #[test]
    fn at_most() {
        let mut list = List::multi(labels(3), &[0], Some(1));
        press(&mut list, &[Key::Down, Key::Char(' ')]);
        assert_eq!(list.press(Key::Enter), Step::Stay, "two ticked, one at most");
        let rows = frame(&mut list, "Lesquels ?", 80, 40);
        assert_eq!(rows[0], format!("# {}", t!("{} au plus", "{} at most", 1)), "above the question");
        assert_eq!(rows[1], "? Lesquels ?");
        list.press(Key::Char(' '));
        assert!(list.error.is_none(), "gone with the next tick");
        assert_eq!(list.press(Key::Enter), Step::Done);
        assert_eq!(list.chosen(), [0]);
    }

    #[test]
    fn cancel() {
        assert_eq!(List::single(labels(2)).press(Key::Cancel), Step::Cancel);
        assert_eq!(List::multi(labels(2), &[], None).press(Key::Cancel), Step::Cancel);
    }

    #[test]
    fn keys() {
        let key = |code, modifiers| key_of(KeyEvent::new(code, modifiers));
        assert_eq!(key(KeyCode::Esc, KeyModifiers::NONE), Some(Key::Cancel));
        assert_eq!(key(KeyCode::Char('c'), KeyModifiers::CONTROL), Some(Key::Cancel));
        assert_eq!(key(KeyCode::Char('p'), KeyModifiers::CONTROL), Some(Key::Up));
        assert_eq!(key(KeyCode::Char('n'), KeyModifiers::CONTROL), Some(Key::Down));
        assert_eq!(key(KeyCode::Char('w'), KeyModifiers::CONTROL), Some(Key::EraseWord));
        assert_eq!(key(KeyCode::Char('h'), KeyModifiers::CONTROL), Some(Key::Erase), "^H, one character");
        assert_eq!(key(KeyCode::Backspace, KeyModifiers::NONE), Some(Key::Erase));
        assert_eq!(key(KeyCode::Backspace, KeyModifiers::ALT), Some(Key::EraseWord));
        assert_eq!(key(KeyCode::Char('u'), KeyModifiers::CONTROL), Some(Key::EraseAll));
        assert_eq!(key(KeyCode::Char('É'), KeyModifiers::SHIFT), Some(Key::Char('É')));
        assert_eq!(key(KeyCode::Char('x'), KeyModifiers::ALT), None);
        assert_eq!(key(KeyCode::Tab, KeyModifiers::NONE), None);
    }

    #[test]
    fn long_rows() {
        let question = "Qui sont tes interlocuteurs (deux au plus) ? Ils seront dans le premier onglet.";
        let mut list = List::multi(vec!["un choix bien trop long pour la largeur".into(), "court".into()], &[], None);
        let rows = frame(&mut list, question, 31, 40);
        assert_eq!(
            rows,
            [
                "? Qui sont tes interlocuteurs",
                "(deux au plus) ? Ils seront",
                "dans le premier onglet.",
                "> [ ] un choix bien trop long",
                "      pour la largeur",
                "  [ ] court",
                "[aide]"
            ],
            "the question broken between words, a choice under itself"
        );
        assert!(rows.iter().all(|r| r.chars().count() < 31), "a column to spare");
    }

    #[test]
    fn long_words() {
        assert_eq!(texts(&wrap(&[("abcdefghij", Paint::Plain)], 4)), ["abcd", "efgh", "ij"]);
        assert_eq!(texts(&wrap(&[("ab abcdefghij", Paint::Plain)], 4)), ["ab", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn wide_characters_take_two_columns() {
        let mut list = List::single(vec!["日本語のラベル".into(), "fusée 🚀".into()]);
        let rows = list.frame("Lequel ?", "aide", 11, 40, 40);
        assert_eq!(texts(&rows), ["? Lequel ?", "> 日本語の", "  ラベル", "  fusée 🚀", "[aide]"]);
        assert!(rows.iter().all(|r| columns(r) <= 10), "within 10 columns");
        assert_eq!(columns(&rows[1]), 10);
        assert_eq!(columns(&rows[3]), 10);
    }

    #[test]
    fn spaces_stay_in_a_row() {
        let mut list = List::single(vec!["alpha    interlocuteur".into(), "beta     Opus · high".into()]);
        let rows = frame(&mut list, "Que  veux-tu ?", 80, 40);
        assert_eq!(rows[..3], ["? Que  veux-tu ?", "> alpha    interlocuteur", "  beta     Opus · high"]);
        let rows = texts(&wrap(&[("un    deux trois", Paint::Plain)], 8));
        assert_eq!(rows, ["un", "deux", "trois"], "no spaces where a row breaks");
        let rows = texts(&wrap(&[("  retrait", Paint::Plain)], 20));
        assert_eq!(rows, ["  retrait"]);
    }

    #[test]
    fn long_choices_take_room_from_the_page() {
        let long = "un choix bien trop long pour une ligne de l'écran";
        let mut list = List::single(vec![long.into(); 20]);
        let rows = frame(&mut list, "Lequel ?", 31, 10);
        assert_eq!(rows.len(), 10, "the frame fits the screen");
        assert_eq!(list.page, 4, "two rows a choice");
        let rows = frame(&mut list, "Lequel ?", 31, 3);
        assert_eq!((rows.len(), list.page), (3, 1), "one choice, without the help");
    }

    #[test]
    fn answered_and_canceled() {
        let list = List::multi(labels(3), &[0, 2], None);
        let rows = settled(">", "Lesquels ?", &list.answer(), Paint::Chosen, 80);
        assert_eq!(texts(&rows), ["> Lesquels ? choix 0, choix 2"]);
        assert_eq!(rows[0].last().unwrap().1, Paint::Chosen);
        let rows = settled("?", "Lesquels ?", "<annulé>", Paint::Canceled, 80);
        assert_eq!(texts(&rows), ["? Lesquels ? <annulé>"]);
        assert_eq!(rows[0].last().unwrap().1, Paint::Canceled);
    }

    #[test]
    fn colors() {
        let mut list = List::multi(labels(2), &[1], None);
        let rows = list.frame("Lesquels ?", "aide", 80, 40, 40);
        assert_eq!(rows[0][0], ("?".into(), Paint::Mark));
        assert_eq!(
            rows[1],
            [
                (">".into(), Paint::Chosen),
                (" ".into(), Paint::Plain),
                ("[ ]".into(), Paint::Chosen),
                (" ".into(), Paint::Plain),
                ("choix 0".into(), Paint::Chosen)
            ]
        );
        assert_eq!(rows[2][2], ("[x]".into(), Paint::Mark));
        assert_eq!(rows[2][4], ("choix 1".into(), Paint::Plain));
        assert!(rows[3].iter().all(|(_, p)| *p == Paint::Chosen), "the help");
    }

    /// A first row as wide as the terminal keeps its last cell: the rest is cleared from the next line, not from the
    /// cursor left on that cell with the wrap pending.
    #[test]
    fn a_full_first_row_keeps_its_last_cell() {
        let row = |text: &str| vec![(text.to_string(), Paint::Plain)];
        let text = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap();
        let full = text(super::frame(&[row("0123456789"), row("next")], 10, 0).unwrap());
        assert!(full.contains("0123456789\r\n\x1b[J"), "{full:?}");
        assert!(full.contains("\x1b[Jnext"), "the second row right after, no blank line: {full:?}");
        let short = text(super::frame(&[row("012345678"), row("next")], 10, 0).unwrap());
        assert!(short.contains("012345678\x1b[J\r\nnext"), "{short:?}");
        // Alone, the cursor goes back up to it.
        let alone = text(super::frame(&[row("0123456789")], 10, 0).unwrap());
        assert!(alone.contains("0123456789\r\n\x1b[J\x1b[1A"), "{alone:?}");
    }

    #[test]
    fn rows_up_after_a_narrower_terminal() {
        assert_eq!(rows_up(&[10, 56, 8], 30), 3, "the row of 56 is on two");
        assert_eq!(rows_up(&[10, 56, 8], 80), 2);
        assert_eq!(rows_up(&[0], 80), 0);
        assert_eq!(rows_up(&[], 80), 0);
    }

    #[test]
    fn what_goes_wrong_goes_last() {
        let mut list = List::multi(labels(3), &[0, 1], Some(1));
        assert_eq!(list.press(Key::Enter), Step::Stay);
        let error = format!("# {}", t!("{} au plus", "{} at most", 1));
        assert_eq!(frame(&mut list, "Lesquels ?", 80, 3), [error.as_str(), "? Lesquels ?", "> [x] choix 0"]);
        assert_eq!(frame(&mut list, "Lesquels ?", 80, 2), [error.as_str(), "> [x] choix 0"], "after the question");
        assert_eq!(frame(&mut list, "Lesquels ?", 80, 1), ["> [x] choix 0"], "the choice stays");
    }

    #[test]
    fn emoji_sequences_measured_as_text() {
        let woman_coding = "👩\u{200d}💻";
        let rows = wrap(&[(&format!("ab {woman_coding}"), Paint::Plain)], 5);
        assert_eq!(texts(&rows), [format!("ab {woman_coding}")], "two columns, not four");
        assert_eq!(columns(&rows[0]), 5);
        let rows = wrap(&[("abc \u{2764}\u{fe0f}", Paint::Plain)], 5);
        assert_eq!(texts(&rows), ["abc", "\u{2764}\u{fe0f}"], "two columns, not one");
        assert!(rows.iter().all(|r| columns(r) <= 5));
    }
}
