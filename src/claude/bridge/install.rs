// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The mod's files, written where Claude Code loads them from.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::sys::cache_dir;
use crate::t;

pub(super) const FILES: [(&str, &str); 3] = [
    (".claude-plugin/plugin.json", include_str!("../../../mod/.claude-plugin/plugin.json")),
    ("hooks/hooks.json", include_str!("../../../mod/hooks/hooks.json")),
    ("hooks/register.ts", include_str!("../../../mod/hooks/register.ts")),
];

/// Writes the mod under `~/.cache/recruit/mod/<version>/`, for `claude --plugin-dir`. One folder per version of
/// recruit: a team still running keeps the one it was launched with.
pub fn install() -> Result<PathBuf> {
    let dir = cache_dir().join("mod").join(env!("CARGO_PKG_VERSION"));
    for (name, text) in FILES {
        let file = dir.join(name);
        if fs::read_to_string(&file).ok().as_deref() != Some(text) {
            fs::create_dir_all(file.parent().expect("a file in a folder"))?;
            fs::write(&file, text).with_context(|| t!("écriture de {}", "writing {}", file.display()))?;
        }
    }
    Ok(dir)
}
