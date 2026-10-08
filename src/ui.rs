// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Terminal prompts, in the interface language: lists of choices drawn by recruit ([`list`]), text and yes or no by
//! inquire.

mod list;

use std::fmt;
use std::io::IsTerminal;

use anyhow::Result;
use inquire::validator::Validation;
use inquire::{Confirm, InquireError, Text};
use unicode_width::UnicodeWidthStr;

use crate::config::Team;
use crate::t;
use list::List;

/// The user left a prompt with Esc or Ctrl-C.
#[derive(Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

fn wrap(error: InquireError) -> anyhow::Error {
    match error {
        InquireError::OperationCanceled | InquireError::OperationInterrupted => Cancelled.into(),
        other => other.into(),
    }
}

/// Prompts need a terminal on both ends.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// A value shown with a label of its own.
pub struct Choice<T> {
    pub value: T,
    pub label: String,
}

impl<T> Choice<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Choice { value, label: label.into() }
    }
}

pub fn select<T>(message: &str, choices: Vec<Choice<T>>) -> Result<T> {
    let help = t!("↑↓ pour choisir, Entrée pour valider", "↑↓ to move, Enter to select");
    let chosen = list::run(List::single(labels(&choices)), message, &help)?;
    Ok(take(choices, &chosen).into_iter().next().expect("one choice"))
}

/// `checked`: indexes of the choices ticked at first.
/// `max`: how many can be ticked at most.
pub fn multi_select<T>(
    message: &str,
    choices: Vec<Choice<T>>,
    checked: &[usize],
    max: Option<usize>,
) -> Result<Vec<T>> {
    let help = t!(
        "↑↓ pour se déplacer, Espace pour cocher, Entrée pour valider",
        "↑↓ to move, Space to toggle, Enter to confirm"
    );
    let chosen = list::run(List::multi(labels(&choices), checked, max), message, &help)?;
    Ok(take(choices, &chosen))
}

fn labels<T>(choices: &[Choice<T>]) -> Vec<String> {
    choices.iter().map(|c| c.label.clone()).collect()
}

/// The values of the choices at `chosen`, in their order.
fn take<T>(choices: Vec<Choice<T>>, chosen: &[usize]) -> Vec<T> {
    choices.into_iter().enumerate().filter(|(i, _)| chosen.contains(i)).map(|(_, c)| c.value).collect()
}

pub fn confirm(message: &str, default: bool) -> Result<bool> {
    let error = t!("Réponds par o (oui) ou n (non)", "Answer y (yes) or n (no)");
    Confirm::new(message)
        .with_default(default)
        .with_parser(&|answer| match answer.trim().to_lowercase().as_str() {
            "o" | "oui" | "y" | "yes" => Ok(true),
            "n" | "non" | "no" => Ok(false),
            _ => Err(()),
        })
        .with_default_value_formatter(&|default| match (crate::i18n::lang(), default) {
            (crate::i18n::Lang::Fr, true) => "O/n".into(),
            (crate::i18n::Lang::Fr, false) => "o/N".into(),
            (crate::i18n::Lang::En, true) => "Y/n".into(),
            (crate::i18n::Lang::En, false) => "y/N".into(),
        })
        .with_formatter(&|answer| if answer { t!("oui", "yes") } else { t!("non", "no") })
        .with_error_message(&error)
        .prompt()
        .map_err(wrap)
}

/// Free text. `check` returns an error message for an answer it refuses.
pub fn text(
    message: &str,
    default: Option<&str>,
    check: impl Fn(&str) -> Result<(), String> + Clone + 'static,
) -> Result<String> {
    let mut prompt = Text::new(message).with_validator(move |answer: &str| {
        Ok(match check(answer.trim()) {
            Ok(()) => Validation::Valid,
            Err(message) => Validation::Invalid(message.into()),
        })
    });
    if let Some(default) = default {
        prompt = prompt.with_default(default);
    }
    Ok(prompt.prompt().map_err(wrap)?.trim().to_string())
}

pub fn required(answer: &str) -> Result<(), String> {
    if answer.is_empty() { Err(t!("Une réponse est attendue.", "An answer is required.")) } else { Ok(()) }
}

pub fn optional(_: &str) -> Result<(), String> {
    Ok(())
}

/// The members, one per line: name, role, and whether the user talks to them. A role too long for the terminal goes
/// on under itself, broken between words.
pub fn print_team(team: &Team) {
    for line in team_lines(team, terminal_width()) {
        println!("{line}");
    }
}

/// A sentence or two, broken between words at the terminal's width rather than anywhere by the terminal.
pub fn print_paragraph(text: &str) {
    for line in terminal_width().map_or_else(|| vec![text.to_string()], |cols| break_words(text, cols)) {
        println!("{line}");
    }
}

/// The width of the terminal, when stdout is one.
fn terminal_width() -> Option<usize> {
    std::io::stdout().is_terminal().then(crossterm::terminal::size).and_then(Result::ok).map(|(cols, _)| cols as usize)
}

/// `print_team`'s lines, for a terminal `cols` wide (None: no limit).
fn team_lines(team: &Team, cols: Option<usize>) -> Vec<String> {
    let width = team.members.keys().map(|n| n.width()).max().unwrap_or(0);
    let contacts = team.contacts();
    let mut lines = Vec::new();
    for (i, (name, member)) in team.members.iter().enumerate() {
        let head = format!("  {:>2}. {name}{}  ", i + 1, " ".repeat(width - name.width()));
        let indent = head.width();
        // Below 20 columns for the role, the terminal breaks the lines as it can.
        let room = cols.map_or(usize::MAX, |cols| cols.saturating_sub(indent).max(20));
        let mut rows = break_words(member.role.lines().next().unwrap_or_default(), room);
        if contacts.contains(&name.as_str()) {
            let mark = t!("[interlocuteur]", "[contact]");
            let last = rows.last_mut().expect("a row");
            if last.is_empty() {
                last.push_str(&mark);
            } else if last.width() + 2 + mark.width() <= room {
                last.push_str("  ");
                last.push_str(&mark);
            } else {
                rows.push(mark);
            }
        }
        for (j, row) in rows.into_iter().enumerate() {
            lines.push(if j == 0 { format!("{head}{row}") } else { format!("{}{row}", " ".repeat(indent)) });
        }
    }
    lines
}

/// `text` on rows of `width` columns at most, broken between words; a word longer than a row has one of its own. One
/// row at least.
fn break_words(text: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    for word in text.split_whitespace() {
        let row = rows.last_mut().expect("a row");
        if row.is_empty() {
            row.push_str(word);
        } else if row.width() + 1 + word.width() <= width {
            row.push(' ');
            row.push_str(word);
        } else {
            rows.push(word.to_string());
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Member;

    #[test]
    fn take_keeps_the_order_of_the_choices() {
        let choices = vec![Choice::new('a', "A"), Choice::new('b', "B"), Choice::new('c', "C")];
        assert_eq!(take(choices, &[2, 0, 7]), ['a', 'c'], "in their order, and nothing out of range");
    }

    #[test]
    fn long_roles_go_on_under_themselves() {
        let mut team = Team::default();
        let role = "Breaks the work down and hands it out to the team";
        team.members.insert("lead".into(), Member { role: role.into(), contact: true, ..Default::default() });
        team.members.insert("backend".into(), Member { role: "Server".into(), ..Default::default() });
        let mark = t!("[interlocuteur]", "[contact]");
        let indent = " ".repeat(15);

        // 25 columns for the roles: the mark gets a row of its own.
        let lines = team_lines(&team, Some(40));
        let expected = [
            "   1. lead     Breaks the work down and".to_string(),
            format!("{indent}hands it out to the team"),
            format!("{indent}{mark}"),
            "   2. backend  Server".to_string(),
        ];
        assert_eq!(lines, expected);

        // Wide enough, or no terminal: one line each, as before.
        let one = [format!("   1. lead     {role}  {mark}"), "   2. backend  Server".to_string()];
        assert_eq!(team_lines(&team, Some(120)), one);
        assert_eq!(team_lines(&team, None), one);

        // Too narrow for a column: 20 columns all the same, a long word on a row of its own.
        team.members["backend"].role = "Server-side-everything-and-more".into();
        let lines = team_lines(&team, Some(10));
        assert_eq!(lines[0], "   1. lead     Breaks the work down");
        assert_eq!(lines.last().unwrap(), "   2. backend  Server-side-everything-and-more");
    }

    #[test]
    fn paragraphs_break_between_words() {
        let text = "Commite .recruit/settings.toml ; tes réglages personnels vont dans .recruit/settings.local.toml, ignoré par git.";
        let rows = break_words(text, 100);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.width() <= 100), "{rows:?}");
        assert_eq!(rows.join(" "), text, "no word cut, none lost");
        assert_eq!(break_words(text, 200), [text]);
        assert_eq!(break_words("", 10), [""]);
    }
}
