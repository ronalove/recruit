// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The choices over the team, in a layer at the center: the quit choice and the confirmations.

use crossterm::style::Color;

use crate::t;
use crate::ui::Rect;
use crate::ui::canvas::{Canvas, Style, columns, fit};

/// A choice over the team, in a layer at the center, the rest dimmed: the quit choice of ⌥q
/// and the bar's « quitter », a confirmation. Drawn here, placed and dimmed by the screen, kept, answered and acted
/// on by the server.
///
/// - The screen dims the rest before it calls [`choice`], which draws its box only.
/// - An option's key matches whatever its case, with no modifier but Shift (« Q » under Caps Lock picks « q »); Esc
///   picks the cancel, the last option. ⏎ picks the selected one; ←→ and ↑↓ move it. The server answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Choice {
    pub title: String,
    /// A line under the title, dimmed: what the choice costs.
    pub note: Option<String>,
    /// Each option with the key that picks it, both shown; in their order on screen.
    pub options: Vec<(char, String)>,
    /// The option ⏎ picks, highlighted: ←→ (or ↑↓) move it. A confirmation opens on its cancel.
    pub selected: usize,
}

/// The key of an option in the language of the run: `fr` in French, `en` in English.
fn key(fr: char, en: char) -> char {
    match crate::i18n::lang() {
        crate::i18n::Lang::Fr => fr,
        crate::i18n::Lang::En => en,
    }
}

/// The key that cancels a choice: « a » for Annuler, « c » for Cancel.
fn cancel_key() -> char {
    key('a', 'c')
}

/// ⌥q and the bar's « quitter »: Détacher (d), Quitter (q), Annuler (a); Detach (d), Quit (q), Cancel (c) in English,
/// with nothing more in the labels, under « recruit ». Opens on the cancel. Its options' order is the server's: detach,
/// quit, cancel.
pub(crate) fn quit_choice() -> Choice {
    Choice {
        title: "recruit".into(),
        note: None,
        options: vec![
            ('d', t!("Détacher", "Detach")),
            ('q', t!("Quitter", "Quit")),
            (cancel_key(), t!("Annuler", "Cancel")),
        ],
        selected: 2,
    }
}

/// The confirmation of a click on a member's context on the dashboard: « Compacter <membre> ? », what it does with the
/// member's context as the dashboard shows it (`percent`, left out unknown), then Compacter (c) and Annuler (a);
/// Compact (o) and Cancel (c) in English. Opens on the cancel. Its options' order is the server's: compact, cancel.
pub(crate) fn compact_choice(member: &str, percent: Option<u8>) -> Choice {
    let note = match percent {
        Some(percent) => t!(
            "Résume sa conversation pour libérer du contexte ({} %).",
            "Summarizes its conversation to free context ({}%).",
            percent
        ),
        None => t!("Résume sa conversation pour libérer du contexte.", "Summarizes its conversation to free context."),
    };
    let compact = key('c', 'o');
    Choice {
        title: t!("Compacter {} ?", "Compact {}?", member),
        note: Some(note),
        options: vec![(compact, t!("Compacter", "Compact")), (cancel_key(), t!("Annuler", "Cancel"))],
        selected: 1,
    }
}

/// Columns inside a choice's frame: the title in the border, the note, each option with its key at the right.
pub(super) fn choice_inner(choice: &Choice) -> usize {
    let title = columns(&choice.title) + 5;
    let note = choice.note.as_deref().map_or(0, |note| columns(note) + 2);
    let options = choice.options.iter().map(|(_, label)| columns(label) + 5).max().unwrap_or(0);
    title.max(note).max(options)
}

/// Rows a choice takes: its frame, its note, its options, and the rule before the last one (the cancel).
fn choice_rows(choice: &Choice) -> usize {
    2 + usize::from(choice.note.is_some()) + choice.options.len() + usize::from(choice.options.len() > 1)
}

/// Where `choice` goes on a screen whose team shows in `screen` (the bar left out): its size from its text, centered;
/// no larger than the screen.
pub(crate) fn choice_area(screen: Rect, choice: &Choice) -> Rect {
    let width = (choice_inner(choice) + 2).min(screen.width);
    let height = choice_rows(choice).min(screen.height);
    Rect { x: screen.x + (screen.width - width) / 2, y: screen.y + (screen.height - height) / 2, width, height }
}

/// Draws `choice` in `area` (from [`choice_area`]), over what the screen drew and dimmed: a grey frame as the
/// panels', its title in the top border, the note dimmed, one option a row with its key at the right in cyan, a rule
/// before the last one (the cancel), the selected one in reverse video. Returns where each option can be clicked, by
/// its index.
pub(crate) fn choice(canvas: &mut Canvas, area: Rect, choice: &Choice) -> Vec<(Rect, usize)> {
    let Rect { x, y, width, height } = area;
    if width < 4 || height < 3 {
        return Vec::new();
    }
    canvas.frame(x, y, width, height, Style::fg(Color::DarkGrey));
    let inner = width - 2;
    let right = x + width - 1;
    let title = fit(&choice.title, inner.saturating_sub(4));
    canvas.put_in(x + 2, y, right, &format!(" {title} "), Style::PLAIN.bold());
    let (shown, note, rule) = fitted(choice, height - 2);
    let mut row = y + 1;
    if let Some(note) = choice.note.as_deref().filter(|_| note) {
        canvas.put_in(x + 2, row, right - 1, &fit(note, inner.saturating_sub(2)), Style::PLAIN.dim());
        row += 1;
    }
    let mut zones = Vec::new();
    let last = choice.options.len().saturating_sub(1);
    for i in shown {
        let (key, label) = &choice.options[i];
        if i == last && rule {
            canvas.put(x, row, "├", Style::fg(Color::DarkGrey));
            canvas.hline(x + 1, row, inner, '─', Style::fg(Color::DarkGrey));
            canvas.put(right, row, "┤", Style::fg(Color::DarkGrey));
            row += 1;
        }
        let on = i == choice.selected;
        let base = if on { Style::PLAIN.reverse() } else { Style::PLAIN };
        canvas.fill(x + 1, row, inner, 1, base);
        let key_at = right.saturating_sub(2);
        let key_style = if on { base.bold() } else { Style::fg(Color::DarkCyan).bold() };
        canvas.put_in(key_at, row, right, &key.to_string(), key_style);
        let label = fit(label, key_at.saturating_sub(x + 3));
        canvas.put_in(x + 2, row, key_at, &label, if on { base.bold() } else { base });
        zones.push((Rect { x: x + 1, y: row, width: inner, height: 1 }, i));
        row += 1;
    }
    zones
}

/// What of `choice` holds in `rows` rows inside its frame: the options shown, by index in their order, whether the
/// note shows and whether the rule does. Short of rows, the note goes first, then the rule, then the options but the
/// cancel (the last) and the selected one, which always stay while a row is left; the others then show in their
/// order as the rows allow.
pub(super) fn fitted(choice: &Choice, rows: usize) -> (Vec<usize>, bool, bool) {
    let count = choice.options.len();
    let rule = count > 1;
    let note = choice.note.is_some() && rows > count + usize::from(rule);
    let rule = rule && rows > count;
    if rows >= count {
        return ((0..count).collect(), note, rule);
    }
    let last = count.saturating_sub(1);
    let mut kept: Vec<usize> = [last, choice.selected].into_iter().filter(|&i| i < count).collect();
    kept.dedup();
    kept.truncate(rows);
    for i in 0..count {
        if kept.len() >= rows {
            break;
        }
        if !kept.contains(&i) {
            kept.push(i);
        }
    }
    kept.sort_unstable();
    (kept, false, false)
}
