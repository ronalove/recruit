// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The team's own settings the menu offers lists for: the grid of the tabs and the share of automatic compaction.

use crate::config::{self, Edit};
use crate::t;

use super::*;

impl Sheet {
    /// The grids offered, the current one among them.
    const GRIDS: [(usize, usize); 5] = [(4, 2), (3, 2), (2, 2), (3, 1), (2, 1)];
    /// The shares offered for the automatic compaction; `off` first.
    const SHARES: [u8; 5] = [50, 60, 70, 80, 90];

    /// Opens the list of grids under the button.
    pub(super) fn open_grid(&mut self) {
        let Some(layers) = &self.team.layers else { return };
        let now = (layers.columns.value, layers.rows.value);
        let default = (
            layers.columns.fallback.as_ref().map_or(3, |(v, _)| *v),
            layers.rows.fallback.as_ref().map_or(2, |(v, _)| *v),
        );
        // The default and the one in place, offered even when they are not among the usual: else there is no way back.
        let mut grids = Self::GRIDS.to_vec();
        for extra in [default, now] {
            if !grids.contains(&extra) {
                grids.insert(0, extra);
            }
        }
        let items: Vec<Choice> = grids
            .iter()
            .map(|&(columns, rows)| {
                let mut choice = Choice::new(Some(&format!("{columns}x{rows}")), format!("{columns} × {rows}"));
                let per_tab = t!("{} par onglet", "{} per tab", columns * rows);
                choice.detail = Some(if (columns, rows) == default {
                    format!("{per_tab} · {}", t!("défaut", "default"))
                } else {
                    per_tab
                });
                choice
            })
            .collect();
        let current = grids.iter().position(|g| *g == now);
        self.overlay =
            Overlay::List(Dropdown { field: None, items, at: current.unwrap_or(0), current, purpose: Purpose::Grid });
    }

    pub(super) fn apply_grid(&mut self, choice: &Choice) -> Vec<Effect> {
        self.said = None;
        let parsed = choice.value.as_deref().and_then(|v| v.split_once('x'));
        let Some((Ok(columns), Ok(rows))) = parsed.map(|(c, r)| (c.parse::<usize>(), r.parse::<usize>())) else {
            return Vec::new();
        };
        vec![Effect::Apply {
            edits: vec![Edit::Grid { columns, rows }],
            said: Done::Text(t!("Grille des onglets : {} × {}.", "Grid of the tabs: {} × {}.", columns, rows)),
            select: None,
        }]
    }

    /// Opens the list of shares under the button.
    pub(super) fn open_compaction(&mut self) {
        let Some(layers) = &self.team.layers else { return };
        let now = layers.auto_compact.value;
        let default = layers.auto_compact.fallback.as_ref().map_or(Some(config::AUTO_COMPACT_AT), |(v, _)| *v);
        let mut shares: Vec<Option<u8>> = std::iter::once(None).chain(Self::SHARES.map(Some)).collect();
        // The default and the one in place, offered even when they are not among the usual: else there is no way back.
        for at in [default, now].into_iter().flatten().filter(|at| !Self::SHARES.contains(at)) {
            if shares.contains(&Some(at)) {
                continue;
            }
            let place = shares.iter().position(|s| s.is_some_and(|s| s > at)).unwrap_or(shares.len());
            shares.insert(place, Some(at));
        }
        let items: Vec<Choice> = shares
            .iter()
            .map(|share| {
                let (value, label) = match share {
                    None => ("off".to_string(), t!("non", "off")),
                    Some(at) => (at.to_string(), t!("{} %", "{}%", at)),
                };
                let mut choice = Choice::new(Some(&value), label);
                if *share == default {
                    choice.detail = Some(t!("défaut", "default"));
                }
                choice
            })
            .collect();
        let current = shares.iter().position(|s| *s == now);
        self.overlay = Overlay::List(Dropdown {
            field: None,
            items,
            at: current.unwrap_or(0),
            current,
            purpose: Purpose::Compaction,
        });
    }

    pub(super) fn apply_compaction(&mut self, choice: &Choice) -> Vec<Effect> {
        self.said = None;
        let share = match choice.value.as_deref() {
            Some("off") => None,
            Some(at) => match at.parse::<u8>() {
                Ok(at) => Some(at),
                Err(_) => return Vec::new(),
            },
            None => return Vec::new(),
        };
        let said = match share {
            None => t!("Compaction automatique : non.", "Automatic compaction: off."),
            Some(at) => {
                t!("Compaction automatique à {} % de la fenêtre.", "Automatic compaction at {}% of the window.", at)
            }
        };
        vec![Effect::Apply { edits: vec![Edit::AutoCompact(share)], said: Done::Text(said), select: None }]
    }
}
