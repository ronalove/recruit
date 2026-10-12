// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What every part of recruit needs from the system: the user's folders, files written in one go, the time, recruit's
//! own executable, and a stable hash.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::t;

pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => PathBuf::from(path),
    }
}

/// `~/x` rather than `/Users/me/x`, for messages.
pub fn tilde(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// Generated files: the members' prompts, the mod, the running teams' state.
pub fn cache_dir() -> PathBuf {
    match std::env::var_os("XDG_CACHE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("recruit"),
        _ => home().join(".cache").join("recruit"),
    }
}

/// Writes a file in one go: a reader sees it as it was or as it is, never half written. A file that was there keeps
/// its permissions; a link stays a link, its target written (`write_temporary`).
pub fn write_atomic(file: &Path, text: &str) -> Result<()> {
    let (temporary, target) = write_temporary(file, text)?;
    fs::rename(&temporary, &target).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        anyhow::Error::new(error).context(t!("écriture de {}", "writing {}", file.display()))
    })
}

/// Writes `text` in a temporary file for a rename over `file`: beside the file it names, a link followed (the
/// dotfiles of a global team are often links), with that file's permissions if it exists. Returns the temporary file
/// and the one to rename it over.
pub fn write_temporary(file: &Path, text: &str) -> Result<(PathBuf, PathBuf)> {
    let target = fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = fs::write(&temporary, text).and_then(|()| match fs::metadata(&target) {
        Ok(meta) => fs::set_permissions(&temporary, meta.permissions()),
        Err(_) => Ok(()),
    });
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| t!("écriture de {}", "writing {}", file.display()));
    }
    Ok((temporary, target))
}

/// Seconds since the epoch.
pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The path of recruit's own executable, for the command lines it starts again (panels, members, its server).
pub fn exe() -> Result<PathBuf> {
    std::env::current_exe().context("recruit")
}

/// FNV-1a, 64 bits, of `parts` one after the other: the same names for the same text from one version of recruit to
/// the next (prompt files, sockets), without a dependency.
pub fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in parts.iter().flat_map(|part| part.iter()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_as_published() {
        // The FNV-1a test values: these names must never change.
        assert_eq!(fnv64(&[b""]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64(&[b"a"]), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv64(&[b"foobar"]), 0x8594_4171_f739_67e8);
        assert_eq!(fnv64(&[b"foo", b"bar"]), fnv64(&[b"foobar"]));
    }

    #[test]
    fn home_written_short() {
        assert_eq!(expand_home("~/x"), home().join("x"));
        assert_eq!(expand_home("~"), home());
        assert_eq!(expand_home("/x"), PathBuf::from("/x"));
        assert_eq!(tilde(&home().join("x")), "~/x");
        assert_eq!(tilde(Path::new("/x")), "/x");
    }
}
