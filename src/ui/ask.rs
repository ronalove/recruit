// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The prompts, in the interface language: lists of choices drawn by recruit ([`super::list`]), text and yes or no by
//! inquire.

use std::fmt;
use std::io::IsTerminal;

use anyhow::Result;
use inquire::validator::Validation;
use inquire::{Confirm, InquireError, Text};

use super::list::{self, List};
use crate::t;

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
        .with_default_value_formatter(&|default| if default { t!("O/n", "Y/n") } else { t!("o/N", "y/N") })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_keeps_the_order_of_the_choices() {
        let choices = vec![Choice::new('a', "A"), Choice::new('b', "B"), Choice::new('c', "C")];
        assert_eq!(take(choices, &[2, 0, 7]), ['a', 'c'], "in their order, and nothing out of range");
    }
}
