// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! An effort's sign, name and color, and a model's family and initial.

use crossterm::style::Color;

use super::palette;
use crate::claude::bridge;
use crate::t;

/// An effort's sign, those Claude Code puts under its prompt: ○ low, ◐ medium, ● high, ◉ extra,
/// ◈ max; the same with or without Nerd Font. Of ambiguous width: counted for one column, as Claude Code does.
pub(crate) fn effort_sign(level: &str) -> char {
    match level {
        "low" => '○',
        "medium" => '◐',
        "high" => '●',
        "xhigh" => '◉',
        "max" => '◈',
        // A level of a later Claude Code, or a slip in a file: neutral.
        _ => '?',
    }
}

/// An effort's name as the user reads it, Claude Code's own (it calls `xhigh` « Extra »); the files keep `xhigh`. A
/// value Claude Code does not know is shown as it is.
pub(crate) fn effort_name(level: &str) -> String {
    match level {
        "low" => t!("faible", "low"),
        "medium" => t!("moyen", "medium"),
        "high" => t!("élevé", "high"),
        "xhigh" => t!("extra", "extra"),
        "max" => "max".to_string(),
        other => other.to_string(),
    }
}

/// An effort as a label: its sign, then its name (`◉ extra`).
pub(crate) fn effort_label(level: &str) -> String {
    format!("{} {}", effort_sign(level), effort_name(level))
}

/// The columns of the widest effort name, in the current language.
pub(crate) fn effort_name_width() -> usize {
    bridge::EFFORTS.iter().map(|l| effort_name(l).chars().count()).max().unwrap_or(0)
}

/// An effort's color, as in Claude Code; none for `max`, in a rainbow; the terminal's grey for a level not known.
pub(crate) fn effort_color(level: &str) -> Option<Color> {
    match level {
        "low" => Some(palette().low),
        "medium" => Some(palette().medium),
        "high" | "xhigh" => Some(palette().high),
        "max" => None,
        _ => Some(Color::DarkGrey),
    }
}

/// An effort's characters with their colors: its sign, and with `word` a blank then its name; those of a level in the
/// level's own color; for `max` the sign in the rainbow's first color and its name letter by letter, all turned by
/// `turn` as the spinners turn. The blank has no color.
pub(crate) fn effort_cells(level: &str, word: bool, turn: usize) -> Vec<(char, Option<Color>)> {
    let rainbow = &palette().rainbow;
    let color = |i: usize| Some(effort_color(level).unwrap_or(rainbow[(i + turn) % rainbow.len()]));
    let mut cells = vec![(effort_sign(level), color(0))];
    if word {
        cells.push((' ', None));
        cells.extend(effort_name(level).chars().enumerate().map(|(i, c)| (c, color(i + 1))));
    }
    cells
}

/// A model's initial, for where its name does not fit: the family's first letter in capitals (`O`, `S`, `H`, `F`; an
/// unknown family's first letter too), `?` when the model is not known yet.
pub(crate) fn model_initial(model: Option<&str>) -> char {
    model.and_then(|m| m.chars().next()).map_or('?', |c| c.to_uppercase().next().unwrap_or(c))
}

/// The model and the effort of a member as the panels show them: the model's family (`Opus`), the effort only when it is
/// one Claude Code knows.
pub(crate) fn shown((model, effort): (Option<String>, Option<String>)) -> (Option<String>, Option<String>) {
    (model.map(|m| family(&m)), effort.filter(|e| bridge::EFFORTS.contains(&e.as_str())))
}

/// `claude-opus-5-5` → `Opus`, `us.anthropic.claude-sonnet-4-5-…` → `Sonnet`: the model's family, as Claude Code names
/// it, wherever the id puts it (a Bedrock profile, an ARN); else the id's first word.
pub(crate) fn family(model: &str) -> String {
    if let Some(known) = bridge::Model::of_name(model) {
        return known.label.to_string();
    }
    let id = model.to_lowercase();
    let name = id.strip_prefix("claude-").unwrap_or(&id);
    let word = name.split(|c: char| !c.is_alphanumeric()).next().unwrap_or(name);
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The efforts as Claude Code names and signs them: ○ ◐ ● ◉ ◈, extra for xhigh, the files'
    /// values untouched; a model's initial.
    #[test]
    fn efforts_and_models_in_the_users_words() {
        let levels = bridge::EFFORTS;
        assert_eq!(levels.map(effort_sign), ['○', '◐', '●', '◉', '◈']);
        assert_eq!(
            levels.map(effort_name),
            [t!("faible", "low"), t!("moyen", "medium"), t!("élevé", "high"), "extra".into(), "max".into()]
        );
        assert_eq!(effort_label("xhigh"), "◉ extra");
        // A value Claude Code does not know is shown as it is.
        assert_eq!(effort_name("turbo"), "turbo");
        assert_eq!(effort_name_width(), levels.iter().map(|l| effort_name(l).chars().count()).max().unwrap());
        // A level in its own color, sign and name; `max` the sign in the rainbow's first color, its name letter by
        // letter after it, turned by the spinner.
        let cells = effort_cells("high", true, 0);
        assert_eq!(cells.iter().map(|(c, _)| *c).collect::<String>(), effort_label("high"));
        assert!(cells.iter().filter(|(c, _)| *c != ' ').all(|(_, color)| *color == Some(palette().high)));
        assert_eq!(cells[1], (' ', None));
        let max = effort_cells("max", true, 0);
        let rainbow = palette().rainbow;
        assert_eq!([max[0].1, max[2].1, max[3].1], [Some(rainbow[0]), Some(rainbow[1]), Some(rainbow[2])]);
        // A level not known: a neutral « ? » and its name in grey, no rainbow.
        assert_eq!(effort_sign("turbo"), '?');
        assert_eq!(effort_color("turbo"), Some(Color::DarkGrey));
        assert!(
            effort_cells("turbo", true, 3).iter().filter(|(c, _)| *c != ' ').all(|(_, c)| *c == Some(Color::DarkGrey))
        );
        assert_eq!(effort_cells("max", false, 2).len(), 1);
        assert_eq!(effort_cells("max", false, 2)[0].1, Some(rainbow[2]));
        assert_eq!(
            ["Opus", "Sonnet", "Haiku", "Fable", "mistral"].map(|m| model_initial(Some(m))),
            ['O', 'S', 'H', 'F', 'M']
        );
        assert_eq!(model_initial(None), '?');
        assert_eq!(model_initial(Some("")), '?');
    }

    /// What the panels show of a model and an effort: the family, the effort when Claude Code knows it.
    #[test]
    fn what_is_shown_of_a_model_and_an_effort() {
        let pair = |model: &str, effort: &str| (Some(model.to_string()), Some(effort.to_string()));
        assert_eq!(shown(pair("claude-opus-5-5", "xhigh")), (Some("Opus".into()), Some("xhigh".into())));
        assert_eq!(shown(pair("sonnet", "turbo")), (Some("Sonnet".into()), None));
        assert_eq!(shown((None, None)), (None, None));
    }

    #[test]
    fn model_families() {
        assert_eq!(family("claude-opus-5-5"), "Opus");
        assert_eq!(family("claude-haiku-4-5-20251001"), "Haiku");
        assert_eq!(family("claude-sonnet-5-5[1m]"), "Sonnet");
        assert_eq!(family("fable"), "Fable");
        // Wherever the id puts it: a Bedrock profile, an ARN.
        assert_eq!(family("us.anthropic.claude-sonnet-4-5-20250929-v1:0"), "Sonnet");
        assert_eq!(
            family("arn:aws:bedrock:us-east-1::foundation-model/anthropic.claude-opus-4-1-20250805-v1:0"),
            "Opus"
        );
        // No family known: the first word.
        assert_eq!(family("mistral-large-2"), "Mistral");
    }
}
