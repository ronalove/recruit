// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A failed turn's reason, short, from the text of the error Claude Code gave.

use crate::t;

/// A failed turn's reason, short, for a header, a card or a notice, from the text of the error Claude Code gave
/// « API surchargée (529) », « connexion perdue », « erreur du serveur (500) », « limite de débit (429) »,
/// « limite d'usage, jusqu'à 15 h » (the hour only when the text gives it); else « erreur d'API », with its status
/// code when the text has one.
pub(crate) fn error_label(message: Option<&str>) -> String {
    let text = message.unwrap_or_default();
    let lower = text.to_lowercase();
    let code = status_code(text);
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&["usage limit", "limit reached", "hit your limit", "limite d'usage"]) {
        return match reset_hour(text) {
            Some(hour) => t!("limite d'usage, jusqu'à {}", "usage limit until {}", hour),
            None => t!("limite d'usage", "usage limit"),
        };
    }
    if code == Some(529) || has(&["overloaded"]) {
        return t!("API surchargée (529)", "API overloaded (529)");
    }
    if code == Some(429) || has(&["rate_limit", "rate limit"]) {
        return t!("limite de débit (429)", "rate limited (429)");
    }
    if let Some(code) = code.filter(|code| (500..600).contains(code)) {
        return t!("erreur du serveur ({})", "server error ({})", code);
    }
    if code.is_none()
        && has(&[
            "connection",
            "econnreset",
            "econnrefused",
            "etimedout",
            "enotfound",
            "socket hang up",
            "fetch failed",
            "network",
            "timed out",
            "timeout",
        ])
    {
        return t!("connexion perdue", "connection lost");
    }
    match code {
        Some(code) => format!("{} ({code})", error_short()),
        None => error_short(),
    }
}

/// A failed turn's reason without the status code at its end: « API surchargée » for « API surchargée (529) ».
pub(crate) fn without_code(label: &str) -> &str {
    match label.strip_suffix(')').and_then(|rest| rest.rsplit_once(" (")) {
        Some((reason, code)) if !code.is_empty() && code.bytes().all(|b| b.is_ascii_digit()) => reason,
        _ => label,
    }
}

/// A failed turn's reason at its shortest: « erreur d'API ».
pub(crate) fn error_short() -> String {
    t!("erreur d'API", "API error")
}

/// The HTTP status in an error's text: « API Error: 529 {…} », or « status 500 ».
fn status_code(text: &str) -> Option<u16> {
    // Positions found in the lowered text cut that text, never the original: lowering changes the length of some
    // characters (« İ »), and only ASCII digits are read after the marker.
    let lower = text.to_lowercase();
    let after = ["api error:", "status code", "status:", "status"]
        .iter()
        .find_map(|marker| lower.find(marker).map(|at| &lower[at + marker.len()..]))?;
    let digits: String = after.trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|code| (400..600).contains(code))
}

/// The hour a usage limit resets at, written for the user: « 15 h », “3 pm”; from « resets 3pm », « resets 3:30pm »,
/// « resets 15:00 », or a time in seconds after a « | » (« usage limit reached|1760000000 »). None when it says none.
fn reset_hour(text: &str) -> Option<String> {
    let (hour, minute) = match text.split_once('|') {
        Some((_, epoch)) => local_time(epoch.trim().parse().ok()?)?,
        None => {
            let lower = text.to_lowercase();
            let rest = &lower[lower.find("reset")?..];
            let rest = rest.split_once(' ')?.1.trim_start();
            let rest = rest.strip_prefix("at ").unwrap_or(rest);
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == ':').collect();
            let (h, m) = digits.split_once(':').unwrap_or((&digits, "0"));
            let (mut hour, minute): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
            let suffix = rest[digits.len()..].trim_start();
            if suffix.starts_with("pm") && hour < 12 {
                hour += 12;
            } else if suffix.starts_with("am") && hour == 12 {
                hour = 0;
            }
            (hour, minute)
        }
    };
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(match crate::i18n::lang() {
        crate::i18n::Lang::Fr if minute == 0 => format!("{hour} h"),
        crate::i18n::Lang::Fr => format!("{hour} h {minute:02}"),
        crate::i18n::Lang::En => {
            let (shown, half) = (if hour % 12 == 0 { 12 } else { hour % 12 }, if hour < 12 { "am" } else { "pm" });
            if minute == 0 { format!("{shown} {half}") } else { format!("{shown}:{minute:02} {half}") }
        }
    })
}

/// The hour and minute of `epoch` (seconds) in local time.
// libc marks musl's `time_t` deprecated over its size on 32-bit targets; it is 64 bits on the ones recruit ships.
#[allow(deprecated)]
fn local_time(epoch: i64) -> Option<(u32, u32)> {
    let time = epoch as libc::time_t;
    // SAFETY: localtime_r only writes the `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return None;
    }
    Some((tm.tm_hour as u32, tm.tm_min as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed turn's reason, from the text Claude Code gives, in the language of the run.
    #[test]
    fn failed_turns_said_short() {
        let overloaded =
            r#"API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(error_label(Some(overloaded)), t!("API surchargée (529)", "API overloaded (529)"));
        assert_eq!(error_label(Some("Overloaded")), t!("API surchargée (529)", "API overloaded (529)"));
        let server =
            r#"API Error: 500 {"type":"error","error":{"type":"api_error","message":"Internal server error"}}"#;
        assert_eq!(error_label(Some(server)), t!("erreur du serveur (500)", "server error (500)"));
        assert_eq!(error_label(Some("API Error: 503 upstream")), t!("erreur du serveur (503)", "server error (503)"));
        assert_eq!(error_label(Some("API Error: Request timed out.")), t!("connexion perdue", "connection lost"));
        assert_eq!(error_label(Some("API Error: Connection error.")), t!("connexion perdue", "connection lost"));
        let rate = r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error"}}"#;
        assert_eq!(error_label(Some(rate)), t!("limite de débit (429)", "rate limited (429)"));
        assert_eq!(
            error_label(Some("You've hit your limit · resets 3pm (Europe/Paris)")),
            t!("limite d'usage, jusqu'à 15 h", "usage limit until 3 pm")
        );
        assert_eq!(
            error_label(Some("5-hour limit reached ∙ resets 3:30am")),
            t!("limite d'usage, jusqu'à 3 h 30", "usage limit until 3:30 am")
        );
        assert_eq!(error_label(Some("Claude usage limit reached")), t!("limite d'usage", "usage limit"));
        // Nothing to read: the shortest.
        assert_eq!(error_label(Some("")), t!("erreur d'API", "API error"));
        assert_eq!(error_label(None), t!("erreur d'API", "API error"));
        assert_eq!(error_label(Some("API Error: 400 bad request")), t!("erreur d'API (400)", "API error (400)"));
        assert_eq!(error_short(), t!("erreur d'API", "API error"));
        // Characters whose lowercase is longer or shorter: no panic, the code still read.
        assert_eq!(error_label(Some("İ status 500")), t!("erreur du serveur (500)", "server error (500)"));
        assert_eq!(error_label(Some("ȺȺ API Error: 529")), t!("API surchargée (529)", "API overloaded (529)"));
        assert_eq!(
            error_label(Some("İİ usage limit · resets 3pm")),
            t!("limite d'usage, jusqu'à 15 h", "usage limit until 3 pm")
        );
        assert_eq!(error_label(Some("ȺȺȺ resets İ limit reached")), t!("limite d'usage", "usage limit"));
    }
}
