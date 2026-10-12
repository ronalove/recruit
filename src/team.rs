// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A team at work: its launch ([`launch`]), the changes it follows while it runs ([`live`]), what it leaves for its
//! panels ([`state`]), the prompt of each member ([`prompt`]), and what runs in a member's pane ([`member`]).

pub(crate) mod launch;
pub(crate) mod layout;
pub(crate) mod live;
pub(crate) mod member;
pub(crate) mod prompt;
pub(crate) mod state;
