// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Interface language: French or English, from `--lang`, then the locale environment, then the system locale.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Fr,
    #[default]
    En,
}

impl Lang {
    /// As `--lang` takes it.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Fr => "fr",
            Lang::En => "en",
        }
    }
}

static LANG: OnceLock<Lang> = OnceLock::new();

/// Sets the interface language once, before anything is printed.
pub fn init(forced: Option<Lang>) {
    let _ = LANG.set(forced.unwrap_or_else(detect));
}

pub fn lang() -> Lang {
    *LANG.get_or_init(detect)
}

fn detect() -> Lang {
    let from_env = |var: &str| std::env::var(var).ok().and_then(|v| from_locale(&v));
    let system = || sys_locale::get_locale().and_then(|l| from_locale(&l));
    let explicit = ["RECRUIT_LANG", "LC_ALL", "LC_MESSAGES"].into_iter().find_map(from_env);
    // On macOS, terminals set LANG themselves (Ghostty puts en_US.UTF-8 when it is missing), whatever the
    // user's language: the system's preferred language says more.
    let guessed = if cfg!(target_os = "macos") {
        system().or_else(|| from_env("LANG"))
    } else {
        from_env("LANG").or_else(system)
    };
    explicit.or(guessed).unwrap_or(Lang::En)
}

/// "fr_FR.UTF-8", "fr-CA", "fr" → French; any other real locale → English; empty, "C" and "POSIX" say nothing.
fn from_locale(value: &str) -> Option<Lang> {
    let value = value.trim();
    if value.is_empty() || value == "C" || value == "POSIX" || value.starts_with("C.") {
        return None;
    }
    Some(if value.to_ascii_lowercase().starts_with("fr") { Lang::Fr } else { Lang::En })
}

/// "1 membre", "3 membres" / "1 member", "3 members".
pub fn count(n: usize, fr: &str, en: &str) -> String {
    let word = match lang() {
        Lang::Fr => fr,
        Lang::En => en,
    };
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

/// Picks the French or English text for the current language: `t!("Bonjour {name}", "Hello {name}")`.
#[macro_export]
macro_rules! t {
    ($fr:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Fr => format!($fr $(, $arg)*),
            $crate::i18n::Lang::En => format!($en $(, $arg)*),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales() {
        assert_eq!(from_locale("fr_FR.UTF-8"), Some(Lang::Fr));
        assert_eq!(from_locale("fr-CA"), Some(Lang::Fr));
        assert_eq!(from_locale("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(from_locale("de_DE"), Some(Lang::En));
        assert_eq!(from_locale("C"), None);
        assert_eq!(from_locale("C.UTF-8"), None);
        assert_eq!(from_locale(""), None);
    }
}
