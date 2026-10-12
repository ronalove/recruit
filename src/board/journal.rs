// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The journal: the messages the members send each other, and their compactions, written as they come.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use crossterm::style::Stylize;
use crossterm::{cursor, queue, terminal};

use crate::claude::{self};
use crate::sys::now;
use crate::team::state::Snapshot;
use crate::ui::canvas::{columns, fit};
use crate::ui::look::{self};
use crate::{claude::bridge, t};

use super::clock::*;
use super::keyboard::*;
use super::paint::*;
use super::sessions::*;
use super::states::*;
use super::*;

/// What the journal writes, by the time it happened.
enum Entry {
    Message(Message),
    Compacted(Compacted),
}

impl Entry {
    pub(super) fn at(&self) -> i64 {
        match self {
            Entry::Message(m) => m.at,
            Entry::Compacted(c) => c.at,
        }
    }
}

/// A message one member sent another.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Message {
    pub(super) at: i64,
    pub(super) from: String,
    pub(super) to: String,
    pub(super) text: String,
}

/// The messages a line of `from`'s conversation sends.
pub(super) fn messages(from: &str, line: &str) -> Vec<Message> {
    if !line.contains(r#""name":"SendMessage""#) {
        return Vec::new();
    }
    let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else { return Vec::new() };
    let at = entry.get("timestamp").and_then(|t| t.as_str()).and_then(iso_epoch).unwrap_or_else(now);
    let Some(blocks) = entry.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .filter(|b| b.get("name").and_then(|n| n.as_str()) == Some("SendMessage"))
        .filter_map(|b| b.get("input"))
        .map(|input| {
            let text = |key: &str| match input.get(key) {
                Some(serde_json::Value::String(s)) => s.trim().to_string(),
                Some(other) if !other.is_null() => other.to_string(),
                _ => String::new(),
            };
            let summary = text("summary");
            let body = if summary.is_empty() { text("message") } else { summary };
            Message {
                at,
                from: from.to_string(),
                to: printable(&text("to")),
                text: printable(body.lines().next().unwrap_or_default()),
            }
        })
        .collect()
}

/// A recipient by name: replies go to `uds:/tmp/cc-socks/<pid>.sock`, the sender's process; a name two sessions share
/// comes with the one meant, `coordinateur [ceeb11]`, which `claude agents --json` does not give: only the name is
/// kept.
pub(super) fn recipient(to: &str, by_pid: &HashMap<u32, String>) -> String {
    let pid = to.strip_prefix("uds:").and_then(|path| Path::new(path).file_stem()?.to_str()?.parse::<u32>().ok());
    let named = || {
        let reference = to.strip_suffix(']').and_then(|rest| rest.rsplit_once(" ["));
        match reference {
            Some((name, id)) if !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit()) => name.to_string(),
            _ => to.to_string(),
        }
    };
    pid.and_then(|pid| by_pid.get(&pid).cloned()).unwrap_or_else(named)
}

/// What a conversation file gained since the last read, in whole lines.
#[derive(Default)]
pub(super) struct Tail {
    pub(super) offset: u64,
    pub(super) rest: Vec<u8>,
}

impl Tail {
    pub(super) fn read(&mut self, file: &Path) -> Vec<String> {
        let Ok(mut handle) = fs::File::open(file) else { return Vec::new() };
        let len = handle.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            *self = Tail::default();
        }
        let mut bytes = std::mem::take(&mut self.rest);
        if handle.seek(SeekFrom::Start(self.offset)).is_err() || handle.read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        self.offset = len.max(self.offset);
        let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        self.rest = bytes.split_off(complete);
        String::from_utf8_lossy(&bytes).lines().map(String::from).collect()
    }
}

/// Earlier messages shown when the journal opens.
const BACKLOG: usize = 200;

/// How long the journal waits for the context a compaction left (`Compaction::after`) before it writes the line
/// without it: the mod tells within a minute.
const COMPACTION_GRACE: i64 = 65;

/// What the journal writes in its stream besides messages: a member's compaction, made or failed on its own.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Compacted {
    pub(super) at: i64,
    pub(super) member: String,
    /// Made by the server, not by hand.
    pub(super) auto: bool,
    /// Its context before and after, in percent of its window.
    pub(super) before: Option<u8>,
    pub(super) after: Option<u8>,
    /// An automatic one that failed: why, as the member's mod said it.
    pub(super) failed: Option<String>,
}

/// The compactions of `member`'s report not written yet: its last one made, once the context after is known (or the
/// wait is over), and an automatic one that failed. `printed` keeps what was written, by member, request and kind.
pub(super) fn compactions(state: &Path, member: &str, now: i64, printed: &mut HashSet<String>) -> Vec<Compacted> {
    let Some(report) = bridge::report(state, member) else { return Vec::new() };
    let mut found = Vec::new();
    if let Some(c) = report.compacted.as_ref().filter(|c| c.done) {
        let key = format!("{member}/{}/{}", c.id, c.at);
        if (c.after.is_some() || now - c.at > COMPACTION_GRACE) && printed.insert(key) {
            found.push(Compacted {
                at: c.at,
                member: member.to_string(),
                auto: c.auto,
                before: c.percent,
                after: c.after,
                failed: None,
            });
        }
    }
    // Refused because an agent was at work: nothing, it will come back. Refused for any other reason, an automatic
    // one: a line, once.
    if let Some(c) = report.compaction.as_ref().filter(|c| c.auto && !c.done && c.skip.is_none()) {
        let key = format!("{member}/{}/{}/failed", c.id, c.at);
        if let Some(error) = c.error.as_ref().filter(|_| printed.insert(key)) {
            found.push(Compacted {
                at: c.at,
                member: member.to_string(),
                auto: true,
                before: c.percent,
                after: None,
                failed: Some(error.clone()),
            });
        }
    }
    found
}

/// `text` in `room` characters, cut between words with « … » when it does not hold; None when not even one whole word
/// holds (half of one would say nothing).
pub(super) fn fit_words(text: &str, room: usize) -> Option<String> {
    if columns(text) <= room {
        return Some(text.to_string());
    }
    let mut kept = String::new();
    for word in text.split_whitespace() {
        let wanted = columns(&kept) + usize::from(!kept.is_empty()) + columns(word);
        if wanted + 1 > room {
            break;
        }
        if !kept.is_empty() {
            kept.push(' ');
        }
        kept.push_str(word);
    }
    (!kept.is_empty()).then(|| format!("{kept}…"))
}

/// How a compaction event lays out in a pane `width` wide: its head (` 14:32  ⟳ dev-rendu`) and its `words`, whole ones
/// (an atom such as `· 74 → 9 %` never breaks), over one line or two: the words that hold after the head, then the
/// rest under it, from the name's column. None when the rest does not hold on the second line either.
fn lay_out(head: usize, indent: usize, words: &[String], width: usize) -> Option<(usize, Vec<String>)> {
    let mut used = head;
    let mut first = 0;
    for word in words {
        let wanted = used + 1 + columns(word);
        if wanted > width {
            break;
        }
        used = wanted;
        first += 1;
    }
    let rest: Vec<String> = words[first..].to_vec();
    let second = rest.join(" ");
    (indent + columns(&second) <= width).then_some((first, rest))
}

/// A compaction event in the journal, on two lines at most, cut between words like a message:
/// ` 14:32  ⟳ dev-rendu compacté auto · 74 → 9 %`, dimmed but for the name, or, short of room,
/// ` 14:32  ⟳ dev-rendu compacté` then `          auto · 74 → 9 %`; an automatic one that failed,
/// ` 14:32  ✗ dev-rendu compaction auto ratée · why`, in orange, as `compaction` then `auto ratée · API…`. Words go
/// only past two lines: the figures first, then « auto », never in the middle of one.
pub(super) fn compaction_lines(s: &Snapshot, time: &str, c: &Compacted, width: usize) -> Vec<String> {
    let piece = |text: &str, paint: Paint| (text.to_string(), paint);
    let indent = 1 + columns(time) + 2 + 2;
    let orange = Paint::Color(look::palette().orange);
    let (sign, sign_paint, name_paint, word_paint) = match &c.failed {
        Some(_) => ("✗ ", orange, Paint::Bold(look::palette().orange), orange),
        None => ("⟳ ", Paint::Dim, Paint::Bold(member_color(s, &c.member)), Paint::Dim),
    };
    let head_pieces = vec![
        piece(" ", Paint::Plain),
        piece(time, Paint::Dim),
        piece("  ", Paint::Plain),
        piece(sign, sign_paint),
        piece(&c.member, name_paint),
    ];
    let head = indent + columns(&c.member);
    let candidates = compaction_candidates(c, head, indent, width);
    for candidate in &candidates {
        if let Some((first, rest)) = lay_out(head, indent, candidate, width) {
            let text = |words: &[String]| words.iter().map(|w| format!(" {w}")).collect::<String>();
            let mut first_line = head_pieces.clone();
            first_line.push(piece(&text(&candidate[..first]), word_paint));
            let mut lines = vec![painted(&first_line, width).printed()];
            if !rest.is_empty() {
                let second = vec![piece(&" ".repeat(indent), Paint::Plain), piece(&rest.join(" "), word_paint)];
                lines.push(painted(&second, width).printed());
            }
            return lines;
        }
    }
    // Not even the head holds: the name, cut.
    vec![painted(&head_pieces, width).printed()]
}

/// What a compaction event says, word by word, the richest form first, down to nothing: each form tried in turn until
/// one lays out in `width` columns after a head `head` wide (`indent` for the second line).
fn compaction_candidates(c: &Compacted, head: usize, indent: usize, width: usize) -> Vec<Vec<String>> {
    // What it says, word by word, the richest first.
    let figures = match (c.before, c.after) {
        (Some(before), Some(after)) => Some(t!("· {} → {} %", "· {} → {}%", before, after)),
        (Some(before), None) => Some(t!("· {} %", "· {}%", before)),
        _ => None,
    };
    let words = |text: &str| -> Vec<String> { text.split_whitespace().map(String::from).collect() };
    match &c.failed {
        None => {
            let whole = if c.auto { t!("compacté auto", "auto-compacted") } else { t!("compacté", "compacted") };
            let mut all = words(&whole);
            let mut sets = Vec::new();
            if let Some(figures) = figures {
                // The figures stay together: « · 74 → 9 % » is one word to the layout.
                all.push(figures);
                sets.push(all.clone());
                all.pop();
            }
            sets.extend([all, words(&t!("compacté", "compacted")), Vec::new()]);
            sets
        }
        Some(error) => {
            let label = words(&t!("compaction auto ratée", "auto-compaction failed"));
            let short = words(&t!("compaction ratée", "compaction failed"));
            let why = error.lines().next().unwrap_or_default();
            // The reason takes what is left of the last line, cut between words; none when too little remains.
            let with_why = |label: &Vec<String>| {
                let (first, rest) = lay_out(head, indent, label, width)?;
                let last = if rest.is_empty() {
                    head + label[..first].iter().map(|w| 1 + columns(w)).sum::<usize>()
                } else {
                    indent + columns(&rest.join(" "))
                };
                let room = width.saturating_sub(last + 3);
                // No whole word of it fits: no reason, rather than half of one.
                let why = fit_words(why, room).filter(|_| room >= 6 && !why.is_empty())?;
                let mut all = label.clone();
                all.push(format!("· {why}"));
                Some(all)
            };
            [with_why(&label), Some(label.clone()), Some(short), Some(Vec::new())].into_iter().flatten().collect()
        }
    }
}

/// What the journal wrote, kept to write again when the pane changes width: its lines are cut to the width they were
/// written at, and a pane that was narrower for a moment (before the multiplexer sized it) would keep them short.
#[derive(Debug, Clone)]
pub(super) enum Written {
    Message { time: String, from: String, to: String, text: String },
    Compacted { time: String, compaction: Compacted },
}

/// How many entries the journal keeps to write again.
const KEPT: usize = 2 * BACKLOG;

/// `written` as the journal writes it in a pane `width` wide: a message on two lines, a compaction on one.
pub(super) fn write_entry(out: &mut impl Write, s: &Snapshot, written: &Written, width: usize) -> std::io::Result<()> {
    match written {
        Written::Message { time, from, to, text } => {
            writeln!(out, "{}", message_head(s, time, from, to, width))?;
            writeln!(out, "{}{}", "        ╰─ ".dark_grey(), fit(text, width.saturating_sub(11)))
        }
        Written::Compacted { time, compaction } => {
            for line in compaction_lines(s, time, compaction, width) {
                writeln!(out, "{line}")?;
            }
            Ok(())
        }
    }
}

pub(super) fn journal(mut s: Snapshot, state: &Path) -> Result<()> {
    let mut out = std::io::stdout();
    queue!(out, cursor::Hide)?;
    // What is typed there shows nowhere either (an Enter would leave an empty line between two messages).
    let _quiet = quiet_input(Arc::new(AtomicBool::new(false)));
    let mut tails: HashMap<PathBuf, Tail> = HashMap::new();
    let mut printed: HashSet<String> = HashSet::new();
    let mut history: VecDeque<Written> = VecDeque::new();
    let mut at_width: Option<usize> = None;
    let mut first = true;
    loop {
        reread(&mut s, state);
        // As the dashboard last saw them while it looks: `claude agents` asked once for the team.
        let sessions = fresh_states(state).and_then(Sessions::of).unwrap_or_else(|| sessions(&s, None));
        let mut fresh = Vec::new();
        for (name, session) in &sessions.members {
            let Some(id) = &session.session_id else { continue };
            let file = claude::transcript(&s.dir, s.config_dir.as_deref(), id);
            for line in tails.entry(file.clone()).or_default().read(&file) {
                fresh.extend(messages(name, &line).into_iter().map(Entry::Message));
            }
        }
        // Their compactions, among the messages by the time they were made.
        let now = now();
        for member in &s.members {
            fresh.extend(compactions(state, &member.name, now, &mut printed).into_iter().map(Entry::Compacted));
        }
        fresh.sort_by_key(Entry::at);
        if first {
            fresh.drain(..fresh.len().saturating_sub(BACKLOG));
            first = false;
        }
        let width = terminal::size().map_or(80, |(w, _)| w as usize);
        // The pane changed width: all of what is kept again, at the new one, over a clean screen.
        if at_width.is_some_and(|before| before != width) {
            write!(out, "\x1b[H\x1b[2J\x1b[3J")?;
            for written in &history {
                write_entry(&mut out, &s, written, width)?;
            }
        }
        at_width = Some(width);
        for entry in fresh {
            let time = clock(entry.at()).chars().take(5).collect::<String>();
            let written = match entry {
                Entry::Message(m) => {
                    Written::Message { time, from: m.from, to: recipient(&m.to, &sessions.by_pid), text: m.text }
                }
                Entry::Compacted(compaction) => Written::Compacted { time, compaction },
            };
            write_entry(&mut out, &s, &written, width)?;
            history.push_back(written);
            if history.len() > KEPT {
                history.pop_front();
            }
        }
        out.flush()?;
        std::thread::sleep(TICK);
    }
}

/// Between a message's sender and its recipient in the journal.
pub(super) const ARROW: &str = "──▶";

/// A message's first line in the journal, ` 14:32  dev-cli ──▶ coordinateur`, cut to `width` rather than wrapped:
/// a click reaches only the line it lands on, and a name split over two would read as another one.
pub(super) fn message_head(s: &Snapshot, time: &str, from: &str, to: &str, width: usize) -> String {
    let (sender, recipient) = (member_color(s, from), member_color(s, to));
    let pieces = [
        (" ", Paint::Plain),
        (time, Paint::Dim),
        ("  ", Paint::Plain),
        (from, Paint::Bold(sender)),
        (" ", Paint::Plain),
        (ARROW, Paint::Dim),
        (" ", Paint::Plain),
        (to, Paint::Color(recipient)),
    ];
    painted(&pieces, width).printed()
}

/// The name under column `x` on a message's first line in the journal, ` 14:32  dev-cli ──▶ coordinateur`: its
/// sender or its recipient, nothing elsewhere nor on any other line. Columns counted as the pane shows
/// them. A recipient that reaches the last of the `pane` columns may go on over the next line: the pane's terminal wraps
/// again the lines already written when it narrows, `dev-cli` then reads `dev`. It is no one's.
pub(super) fn journal_name_at(line: &str, x: usize, pane: usize) -> Option<&str> {
    let (time, rest) = line.strip_prefix(' ')?.split_at_checked(5)?;
    let clock = time.bytes().enumerate().all(|(i, b)| if i == 2 { b == b':' } else { b.is_ascii_digit() });
    let rest = rest.strip_prefix("  ").filter(|_| clock)?;
    let width = |text: &str| columns(text);
    let from_at = 1 + 5 + 2;
    // A compaction: ` 14:32  ⟳ dev-rendu compacté auto · 74 → 9 %`, its name alone; or a failed one, with ✗.
    if let Some(after) = rest.strip_prefix("⟳ ").or_else(|| rest.strip_prefix("✗ ")) {
        let name = after.split(' ').next().filter(|name| !name.is_empty())?;
        let at = from_at + 2;
        return (at..at + width(name)).contains(&x).then_some(name);
    }
    let (from, to) = rest.split_once(&format!(" {ARROW} "))?;
    let to_at = from_at + width(from) + 1 + width(ARROW) + 1;
    let to = to.trim_end();
    let to = (to_at + width(to) < pane).then_some((to_at, to));
    [Some((from_at, from)), to]
        .into_iter()
        .flatten()
        .find(|(at, name)| (*at..at + width(name)).contains(&x))
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::fs;

    use crate::claude::bridge::Report;

    use crate::team::state::Snapshot;

    use crate::ui::look::State;
    use crate::{claude::bridge, t};

    use super::*;

    use crate::board::card::*;

    use crate::board::fixture::*;

    /// A compaction event's lines, one string.
    fn compaction_head(s: &Snapshot, time: &str, c: &Compacted, width: usize) -> String {
        compaction_lines(s, time, c, width).join("\n")
    }

    fn strip_ansi(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b {
                while i < bytes.len() && bytes[i] != b'm' {
                    i += 1;
                }
                i += 1;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        out
    }

    #[test]
    fn sent_messages() {
        let line = r#"{"type":"assistant","timestamp":"2026-10-07T10:28:51.123Z","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"SendMessage","input":{"to":"dev","summary":"API prête","message":"L'API est prête.\nDétails…"}},{"type":"tool_use","name":"SendMessage","input":{"to":"uds:/tmp/cc-socks/42.sock","message":"Bien reçu\nsuite"}}]}}"#;
        let found = messages("lead", line);
        assert_eq!(
            found[0],
            Message { at: 1_791_368_931, from: "lead".into(), to: "dev".into(), text: "API prête".into() }
        );
        assert_eq!(found[1].text, "Bien reçu");
        let by_pid = HashMap::from([(42, "coordinateur".to_string())]);
        assert_eq!(recipient(&found[1].to, &by_pid), "coordinateur");
        assert_eq!(recipient("dev", &by_pid), "dev");
        // The session meant, among those of one name: the name alone, the member's color and its click.
        assert_eq!(recipient("coordinateur [ceeb11]", &by_pid), "coordinateur");
        assert_eq!(recipient("coordinateur [x]", &by_pid), "coordinateur [x]");
        assert_eq!(recipient("[ceeb11]", &by_pid), "[ceeb11]");
        assert!(messages("lead", r#"{"type":"user","message":"SendMessage"}"#).is_empty());
        // Printed as it is, so no control character: no escape sequence, no clipboard write (OSC 52).
        let trapped = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "SendMessage",
            "input": {"to": "dev\u{1b}[31m", "message": "Fini\u{1b}]52;c;cm0gLXJmIH4=\u{7} \u{9b}2Jvoilà"}}]}});
        let found = messages("lead", &trapped.to_string());
        assert_eq!((found[0].to.as_str(), found[0].text.as_str()), ("dev[31m", "Fini]52;c;cm0gLXJmIH4= 2Jvoilà"));
        // Nor formatting: a text turned right to left, invisible characters.
        let turned = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "SendMessage",
            "input": {"to": "d\u{200b}ev", "message": "\u{202e}fdp.exe\u{2066} prêt"}}]}});
        let found = messages("lead", &turned.to_string());
        assert_eq!((found[0].to.as_str(), found[0].text.as_str()), ("dev", "fdp.exe prêt"));
    }

    #[test]
    fn tails_read_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.jsonl");
        fs::write(&file, "a\nb").unwrap();
        let mut tail = Tail::default();
        assert_eq!(tail.read(&file), ["a"]);
        fs::write(&file, "a\nbc\nd\n").unwrap();
        assert_eq!(tail.read(&file), ["bc", "d"]);
        assert!(tail.read(&file).is_empty());
        fs::write(&file, "x\n").unwrap();
        assert_eq!(tail.read(&file), ["x"]);
    }

    /// The journal's lines at the pane's real width, 46 columns beside the dashboard: whole there, short only when the
    /// room is short, and written again at the new width when the pane changes it. The forms, longest first: a success
    /// « compacté auto · 74 → 9 % », « compacté auto », « compacté »; a failure « compaction auto ratée · why »,
    /// « compaction auto ratée », « compaction ratée ».
    #[test]
    fn journal_lines_at_the_width_of_the_pane() {
        let s = snapshot(&["work", "waits"]);
        let plain = |line: String| String::from_utf8_lossy(&strip_ansi(line.as_bytes())).into_owned();
        let done = |name: &str| Compacted {
            at: 1000,
            member: name.into(),
            auto: true,
            before: Some(74),
            after: Some(9),
            failed: None,
        };
        let failed = Compacted { failed: Some("API surchargée, réessaie plus tard".into()), ..done("waits") };
        let auto = t!("compacté auto", "auto-compacted");
        let figures = t!("74 → 9 %", "74 → 9%");
        // 46 columns: all of it, on one line.
        assert_eq!(
            plain(compaction_head(&s, "14:32", &done("work"), 46)),
            format!(" 14:32  ⟳ work {auto} · {figures}")
        );
        let line = plain(compaction_head(&s, "14:32", &failed, 46));
        let label = t!("compaction auto ratée", "auto-compaction failed");
        // The reason takes what the line leaves, when it is enough to say a word of it.
        assert!(line.starts_with(&format!(" 14:32  ✗ waits {label}")) && line.chars().count() <= 46, "{line:?}");
        // A little less: the words that do not hold go on a second line, under the name, nothing taken away.
        let indent = " ".repeat(10);
        let at = |width| plain(compaction_head(&s, "14:32", &done("work"), width));
        assert_eq!(at(33), format!(" 14:32  ⟳ work {auto}\n{indent}· {figures}"));
        // The journal beside the dashboard at 80 × 24: 26 columns, 17 after the time and the sign.
        assert_eq!(
            at(26),
            t!(" 14:32  ⟳ work compacté\n          auto · 74 → 9 %", " 14:32  ⟳ work\n          auto-compacted")
        );
        let fails = |width| plain(compaction_head(&s, "14:32", &failed, width));
        assert_eq!(
            fails(26),
            t!(" 14:32  ✗ waits compaction\n          auto ratée", " 14:32  ✗ waits compaction\n          failed")
        );
        // What was asked is kept as long as two lines hold: a failure never loses its object.
        for width in 26..=60 {
            for text in [fails(width), at(width)] {
                let lines: Vec<&str> = text.lines().collect();
                assert!(lines.len() <= 2 && lines.iter().all(|l| l.chars().count() <= width), "{width}: {text:?}");
            }
            assert!(fails(width).contains(&t!("compaction", "compaction")), "{width}: {}", fails(width));
        }
        // An event of two lines fits the reduced journal as a message does: its three last messages, two lines each.
        assert!(compaction_lines(&s, "14:32", &done("work"), 26).len() <= 2);
        // Written again at the width the pane gets: short at 20, whole at 46.
        let history = [
            Written::Message { time: "14:30".into(), from: "work".into(), to: "waits".into(), text: "salut".into() },
            Written::Compacted { time: "14:32".into(), compaction: done("work") },
        ];
        let at_width = |width: usize| {
            let mut out = Vec::new();
            for written in &history {
                write_entry(&mut out, &s, written, width).unwrap();
            }
            plain(String::from_utf8(out).unwrap())
        };
        assert!(!at_width(20).contains("74"), "{}", at_width(20));
        assert!(at_width(46).contains(&format!("{auto} · {figures}")), "{}", at_width(46));
    }

    /// A compaction leaves its trace: a line in the journal at its time, dimmed but for the
    /// name, with « auto » when the server made it; an automatic one that failed, a line in orange; one put off for
    /// an agent at work, nothing. And a ⟳ in the card's curve, in the slice it fell in, until it leaves the half hour.
    #[test]
    fn a_compaction_leaves_a_trace() {
        let state = tempfile::tempdir().unwrap();
        fs::create_dir_all(state.path().join("members")).unwrap();
        let write = |report: &Report| {
            fs::write(state.path().join("members/dev.json"), serde_json::to_string(report).unwrap()).unwrap();
        };
        let done = |auto, after| bridge::Compaction {
            id: "c1".into(),
            done: true,
            at: 1000,
            auto,
            percent: Some(74),
            after,
            ..Default::default()
        };
        let mut printed = HashSet::new();
        // Made, the context after not known yet: the line waits for it, a minute at most.
        write(&Report { compacted: Some(done(true, None)), ..Default::default() });
        assert!(compactions(state.path(), "dev", 1010, &mut printed).is_empty());
        let found = compactions(state.path(), "dev", 1070, &mut printed);
        assert_eq!((found.len(), found[0].after), (1, None));
        // Once only.
        assert!(compactions(state.path(), "dev", 1080, &mut printed).is_empty());
        // With the context after, at once.
        let mut printed = HashSet::new();
        write(&Report { compacted: Some(done(false, Some(9))), ..Default::default() });
        let found = compactions(state.path(), "dev", 1001, &mut printed);
        assert_eq!((found[0].auto, found[0].before, found[0].after), (false, Some(74), Some(9)));
        // The lines.
        let s = snapshot(&["dev"]);
        let plain = |line: String| String::from_utf8_lossy(&strip_ansi(line.as_bytes())).into_owned();
        let auto = Compacted { auto: true, ..found[0].clone() };
        let (what, auto_what) = (t!("compacté", "compacted"), t!("compacté auto", "auto-compacted"));
        assert_eq!(
            plain(compaction_head(&s, "14:32", &auto, 80)),
            format!(" 14:32  ⟳ dev {auto_what} · {}", t!("74 → 9 %", "74 → 9%"))
        );
        assert_eq!(
            plain(compaction_head(&s, "14:32", &found[0], 80)),
            format!(" 14:32  ⟳ dev {what} · {}", t!("74 → 9 %", "74 → 9%"))
        );
        let half = Compacted { after: None, ..found[0].clone() };
        assert_eq!(
            plain(compaction_head(&s, "14:32", &half, 80)),
            format!(" 14:32  ⟳ dev {what} · {}", t!("74 %", "74%"))
        );
        // Its name is a click on that member, nowhere else.
        let line = plain(compaction_head(&s, "14:32", &found[0], 80));
        assert_eq!(journal_name_at(&line, 11, 80), Some("dev"));
        assert_eq!(journal_name_at(&line, 14, 80), None);
        // A narrow pane cuts between words over two lines, then drops some: the figures first, then « auto ».
        let words =
            ["14:32", "⟳", "dev", what.as_str(), "auto", "auto-compacted", "·", "74", "→", "9", "%", "9%", "74%"];
        for width in 14..60 {
            let text = plain(compaction_head(&s, "14:32", &auto, width));
            let lines: Vec<&str> = text.lines().collect();
            assert!(lines.len() <= 2 && lines.iter().all(|l| l.chars().count() <= width), "{width}: {text:?}");
            assert!(text.split_whitespace().all(|word| words.contains(&word)), "cut in a word at {width}: {text:?}");
        }
        // A failed automatic one: orange, once; put off for an agent at work: nothing.
        let mut printed = HashSet::new();
        let failed = bridge::Compaction {
            id: "c2".into(),
            at: 2000,
            auto: true,
            error: Some("API surchargée".into()),
            ..Default::default()
        };
        write(&Report { compaction: Some(failed.clone()), ..Default::default() });
        let found = compactions(state.path(), "dev", 2001, &mut printed);
        assert_eq!(found[0].failed.as_deref(), Some("API surchargée"));
        assert!(compaction_head(&s, "14:32", &found[0], 80).contains("\x1b[38;5;208m"), "orange");
        // Its reason cut between words, « … » after the last, and dropped before the rest is cut.
        let long = Compacted {
            failed: Some("API surchargée pour le moment, réessaie plus tard".into()),
            ..found[0].clone()
        };
        let line = plain(compaction_head(&s, "14:32", &long, 60));
        assert!(line.chars().count() <= 60 && line.ends_with('…') && !line.ends_with(" …"), "{line:?}");
        let text = plain(compaction_head(&s, "14:32", &long, 30));
        assert!(text.starts_with(" 14:32  ✗ dev") && text.lines().count() <= 2, "{text:?}");
        // A reason none of whose words holds is left out whole, not cut to « ECONNR… ».
        let word = Compacted { failed: Some("ECONNREFUSEDXYZ".into()), ..found[0].clone() };
        for width in 24..60 {
            let text = plain(compaction_head(&s, "14:32", &word, width));
            assert!(!text.contains("ECONN") || text.contains("ECONNREFUSEDXYZ"), "{width}: {text:?}");
        }
        assert!(!plain(compaction_head(&s, "14:32", &word, 40)).contains('…'));
        assert_eq!(fit_words("ECONNREFUSEDXYZ", 8), None);
        assert_eq!(fit_words("ECONNREFUSEDXYZ", 15).as_deref(), Some("ECONNREFUSEDXYZ"));
        assert!(compactions(state.path(), "dev", 2002, &mut printed).is_empty());
        let put_off = bridge::Compaction { skip: Some(bridge::SKIP_HELPERS.into()), error: None, ..failed };
        write(&Report { compaction: Some(put_off), ..Default::default() });
        assert!(compactions(state.path(), "dev", 2003, &mut HashSet::new()).is_empty());
        // The curve's ⟳: 30 minutes over 60 cells, 30 s each; at 1000 s, now 1100 s: the slice 3 cells back.
        assert_eq!(compaction_cell(1000, 1100, 60), Some(56));
        assert_eq!(compaction_cell(1100, 1100, 60), Some(59));
        assert_eq!(compaction_cell(1000, 1000 + HISTORY, 60), None, "left the half hour");
        let mut card = sample_card("dev", false, State::Idle);
        card.data.compacted = Some(1000);
        let lines = crate::board::card::card(&card.data, 44, 0, &Right::of([(&card.data, 44)]), &board(Vec::new()));
        let curve = lines.last().unwrap().text();
        assert_eq!(curve.matches('⟳').count(), 2, "the curve's and the context's: {curve}");
        card.data.compacted = None;
        let lines = crate::board::card::card(&card.data, 44, 0, &Right::of([(&card.data, 44)]), &board(Vec::new()));
        assert_eq!(lines.last().unwrap().text().matches('⟳').count(), 1, "only the context's (at rest)");
    }
}
