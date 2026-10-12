// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What a launch asks of a team's multiplexer ([`Plan`]), and the panes it opens: [`super::Mux`] turns them into the
//! server's requests.

use std::path::PathBuf;

use crate::i18n::Lang;

/// What a launch asks the multiplexer to open.
#[derive(Debug, Clone)]
pub struct Plan {
    pub team: String,
    pub dir: PathBuf,
    pub columns: usize,
    /// The tabs of members without the panels beside them shown in « focus » (`layout = "focus"`).
    pub focus: bool,
    pub tabs: Vec<TabPlan>,
    /// The team's language, for what the multiplexer writes itself.
    pub lang: Lang,
}

#[derive(Debug, Clone)]
pub struct TabPlan {
    pub title: String,
    pub panes: Vec<Pane>,
    /// The dashboard over the journal, in a column on the right; the members then stack in one column.
    pub side: Option<Side>,
}

#[derive(Debug, Clone)]
pub struct Side {
    pub dashboard: Pane,
    pub journal: Pane,
}

#[derive(Debug, Clone)]
pub struct Pane {
    /// Shown in the pane's frame: the member's name, or the panel's title.
    pub member: String,
    /// A panel's kind (`DASHBOARD`, `JOURNAL`); none for a member.
    pub role: Option<&'static str>,
    /// Run directly, without a shell in between.
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub const DASHBOARD: &str = "dashboard";
pub const JOURNAL: &str = "journal";
