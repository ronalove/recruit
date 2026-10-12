// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

//! Whether a newer recruit is published: GitHub's latest release of ronalove/recruit, asked by the server at most once
//! a day, in a thread of its own, and kept in `~/.cache/recruit/update.json` for every team. The menu reads that file
//! only: no network on the drawing path. Updating is left to the package manager (decision of 2026-10-12).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Where the latest release is asked.
const RELEASE_URL: &str = "https://api.github.com/repos/ronalove/recruit/releases/latest";

/// The age of a check from which the next one is due.
const MAX_AGE: i64 = 24 * 3600;

/// The wait after a failed check, for every server: with several teams and a failure that lasts (GitHub's rate limit),
/// each would otherwise try every hour.
const RETRY_AFTER: i64 = 6 * 3600;

/// How often the server's thread looks at that age again.
const WAKE: Duration = Duration::from_secs(3600);

/// A non-empty value turns the check off: no thread, no network.
const OPT_OUT: &str = "RECRUIT_NO_UPDATE_CHECK";

/// What `update.json` holds. The freshness is read in `checked` and `tried`, never in the file's time (CLAUDE.md,
/// « Pièges »). A file with only `tried` is the trace of a first check that failed.
#[derive(Default, Serialize, Deserialize)]
struct Cache {
    /// Seconds since the epoch, when GitHub answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checked: Option<i64>,
    /// `X.Y.Z`, without the `v` of the tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    latest: Option<String>,
    /// Seconds since the epoch, when the last check failed; a success leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tried: Option<i64>,
}

/// The cache, shared by every team of the user.
fn cache_file() -> PathBuf {
    crate::sys::cache_dir().join("update.json")
}

/// `X.Y.Z` as numbers; anything else (a fourth part, a suffix, a sign, an empty part) is not a version.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let mut next = || {
        let part = parts.next()?;
        part.bytes().all(|byte| byte.is_ascii_digit()).then(|| part.parse().ok()).flatten()
    };
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// The cache of `file`, if it can be read and the version it names, if any, is one.
fn read(file: &Path) -> Option<Cache> {
    let cache: Cache = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    if let Some(latest) = &cache.latest {
        parse(latest)?;
    }
    Some(cache)
}

/// The version of `cache` if it is newer than `running`.
fn newer_in(cache: Option<Cache>, running: &str) -> Option<String> {
    let latest = cache?.latest?;
    (parse(&latest)? > parse(running)?).then_some(latest)
}

/// The version published after `running`, if the last check found one. Reads the cache only, never the network;
/// `None` without a check, or when it cannot be read. The opt-out is not looked at here: it stops the check, and a
/// cache that someone wrote still tells what it tells.
pub fn newer(running: &str) -> Option<String> {
    newer_in(read(&cache_file()), running)
}

/// Whether a check is due at `now`: none was made, or it is a day old or more; and not within `RETRY_AFTER` of a
/// failed one. A time in the future (a clock that went back) counts for nothing, or it would hold the next check off
/// for good.
fn due(cache: Option<&Cache>, now: i64) -> bool {
    let Some(cache) = cache else { return true };
    let fresh = cache.checked.is_some_and(|checked| checked <= now && now - checked < MAX_AGE);
    let failed = cache.tried.is_some_and(|tried| tried <= now && now - tried < RETRY_AFTER);
    !fresh && !failed
}

/// Whether `value`, the opt-out's, turns the check off.
fn disabled_by(value: Option<OsString>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// The version in GitHub's answer for a release: its `tag_name` without the `v`, if it is `X.Y.Z`.
fn parse_release(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    parse(version)?;
    Some(version.to_string())
}

/// Asks GitHub: `curl`, with nothing on the terminal and no more than ten seconds. The reason, when it fails. `-q`
/// first, the only place it counts: the user's `.curlrc` could hold an `-i`, an `-o` or a `-D -` that spoils the JSON
/// (the proxies of the environment are still followed).
fn fetch() -> Result<String, String> {
    let output = Command::new("curl")
        .args(["-q", "-fsSL", "--max-time", "10", "--proto", "=https", "--proto-redir", "=https"])
        .args(["-H", "Accept: application/vnd.github+json", RELEASE_URL])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("curl: {error}"))?;
    if !output.status.success() {
        return Err(format!("curl: {}", output.status));
    }
    parse_release(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| "no X.Y.Z in the release".to_string())
}

/// Writes `cache` to `file`, atomically, its folder made.
fn store(file: &Path, cache: &Cache) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    let text = serde_json::to_string(cache).map_err(|error| error.to_string())?;
    crate::sys::write_atomic(file, &text).map_err(|error| format!("{error:#}"))
}

/// One round: if a check is due, `ask` for the version and write it down at `now`. `Ok(true)` when it was written.
/// A failure keeps the `checked` and the `latest` that were there and notes `tried`, for every server: the next
/// round, here or in another team's server, waits `RETRY_AFTER`.
fn round(file: &Path, now: i64, ask: impl FnOnce() -> Result<String, String>) -> Result<bool, String> {
    if !due(read(file).as_ref(), now) {
        return Ok(false);
    }
    match ask() {
        Ok(latest) => {
            store(file, &Cache { checked: Some(now), latest: Some(latest), tried: None })?;
            Ok(true)
        }
        Err(reason) => {
            // The file as it is now, not as it was before `ask`: another server may have written its answer meanwhile.
            // Nowhere to note it, no more to say than the reason of the failure itself.
            let _ = store(file, &Cache { tried: Some(now), ..read(file).unwrap_or_default() });
            Err(reason)
        }
    }
}

/// Asks GitHub for the latest release, in a thread of its own, if the last check is a day old or more (or there is
/// none); never blocks, never says anything when it fails. Nothing at all under `RECRUIT_NO_UPDATE_CHECK`.
///
/// The thread looks again every hour, reading the cache each time: a check made, or a failure noted, by another
/// team's server counts. A failure puts one line in `server.log` (stderr, which the server has redirected there),
/// then none until a check succeeds.
pub fn check_in_background() {
    if disabled_by(std::env::var_os(OPT_OUT)) {
        return;
    }
    let started = std::thread::Builder::new().name("update".into()).spawn(|| {
        let file = cache_file();
        let mut told = false;
        loop {
            match round(&file, crate::sys::now(), fetch) {
                Ok(true) => told = false,
                Ok(false) => {}
                Err(reason) if !told => {
                    told = true;
                    eprintln!("{} recruit[{}]: update check failed: {reason}", crate::sys::now(), std::process::id());
                }
                Err(_) => {}
            }
            std::thread::sleep(WAKE);
        }
    });
    // No thread, no check: nothing to say to anyone.
    drop(started);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(checked: i64, latest: &str) -> Cache {
        Cache { checked: Some(checked), latest: Some(latest.to_string()), tried: None }
    }

    fn failed(cache: Cache, tried: i64) -> Cache {
        Cache { tried: Some(tried), ..cache }
    }

    #[test]
    fn versions_are_numbers_not_text() {
        assert_eq!(parse("2.3.1"), Some((2, 3, 1)));
        assert!(parse("2.10.0") > parse("2.9.9"));
        assert!(parse("10.0.0") > parse("9.9.9"));
        for bad in ["", "2.3", "2.3.1.4", "2.3.x", "2.3.1-rc1", "v2.3.1", "2..1", "+2.3.1", "-1.0.0", "2.3.1 "] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        assert_eq!(parse("99999999999999999999.0.0"), None);
    }

    #[test]
    fn a_newer_version_only() {
        assert_eq!(newer_in(Some(cache(0, "2.4.0")), "2.3.1").as_deref(), Some("2.4.0"));
        assert_eq!(newer_in(Some(cache(0, "2.10.0")), "2.9.0").as_deref(), Some("2.10.0"));
        assert_eq!(newer_in(Some(cache(0, "2.3.1")), "2.3.1"), None);
        assert_eq!(newer_in(Some(cache(0, "2.3.0")), "2.3.1"), None);
        assert_eq!(newer_in(None, "2.3.1"), None);
        // A version of ours that is not X.Y.Z (a build from a branch) gets no arrow rather than a wrong one.
        assert_eq!(newer_in(Some(cache(0, "2.4.0")), "2.3.1-dev"), None);
    }

    #[test]
    fn the_cache_is_read_from_its_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("update.json");
        assert!(read(&file).is_none());
        std::fs::write(&file, r#"{ "checked": 1700000000, "latest": "2.4.0" }"#).unwrap();
        let cache = read(&file).unwrap();
        assert_eq!((cache.checked, cache.latest.as_deref(), cache.tried), (Some(1_700_000_000), Some("2.4.0"), None));
        // The trace of a first check that failed has no version.
        std::fs::write(&file, r#"{ "tried": 1700000000 }"#).unwrap();
        let cache = read(&file).unwrap();
        assert_eq!((cache.checked, cache.latest, cache.tried), (None, None, Some(1_700_000_000)));
        for bad in [
            "",
            "{",
            "null",
            "7",
            r#"{"checked": "x", "latest": "2.4.0"}"#,
            r#"{"checked": 1, "latest": "next"}"#,
            r#"{"checked": 1, "latest": "2.4.0", "tried": "x"}"#,
        ] {
            std::fs::write(&file, bad).unwrap();
            assert!(read(&file).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_check_is_due_after_a_day() {
        let now = 1_700_000_000;
        assert!(due(None, now));
        assert!(!due(Some(&cache(now, "2.4.0")), now));
        assert!(!due(Some(&cache(now - MAX_AGE + 1, "2.4.0")), now));
        assert!(due(Some(&cache(now - MAX_AGE, "2.4.0")), now));
        assert!(due(Some(&cache(now - 3 * MAX_AGE, "2.4.0")), now));
        // A time in the future: the clock went back, ask again.
        assert!(due(Some(&cache(now + 1, "2.4.0")), now));
    }

    #[test]
    fn a_failed_check_is_not_tried_again_for_hours() {
        let now = 1_700_000_000;
        let old = now - 2 * MAX_AGE;
        // A day-old answer and a failure just now: wait, for every server.
        assert!(!due(Some(&failed(cache(old, "2.4.0"), now)), now));
        assert!(!due(Some(&failed(cache(old, "2.4.0"), now - RETRY_AFTER + 1)), now));
        assert!(due(Some(&failed(cache(old, "2.4.0"), now - RETRY_AFTER)), now));
        // The same with no answer at all.
        let first = Cache { tried: Some(now), ..Cache::default() };
        assert!(!due(Some(&first), now));
        assert!(due(Some(&Cache { tried: Some(now - RETRY_AFTER), ..Cache::default() }), now));
        // A fresh answer needs no check, whatever failed before; a failure in the future counts for nothing.
        assert!(!due(Some(&failed(cache(now, "2.4.0"), now - 10 * RETRY_AFTER)), now));
        assert!(due(Some(&failed(cache(old, "2.4.0"), now + 1)), now));
    }

    #[test]
    fn the_opt_out_wants_a_value() {
        assert!(!disabled_by(None));
        assert!(!disabled_by(Some(OsString::new())));
        assert!(disabled_by(Some("1".into())));
        assert!(disabled_by(Some("0".into())));
    }

    #[test]
    fn githubs_release_is_read() {
        let body = |tag: &str| format!(r#"{{ "url": "x", "tag_name": "{tag}", "draft": false, "assets": [] }}"#);
        assert_eq!(parse_release(&body("v2.4.0")).as_deref(), Some("2.4.0"));
        assert_eq!(parse_release(&body("2.4.0")).as_deref(), Some("2.4.0"));
        assert_eq!(parse_release(&body("v2.4")), None);
        assert_eq!(parse_release(&body("v2.4.0-rc1")), None);
        assert_eq!(parse_release(&body("latest")), None);
        assert_eq!(parse_release(r#"{ "tag_name": 240 }"#), None);
        assert_eq!(parse_release(r#"{ "message": "Not Found" }"#), None);
        assert_eq!(parse_release("<html>rate limited</html>"), None);
        assert_eq!(parse_release(""), None);
    }

    #[test]
    fn a_round_writes_only_what_it_was_told() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cache").join("update.json");
        let now = 1_700_000_000;

        // A first failure notes only that it was tried, which `newer` does not take for a version.
        assert_eq!(round(&file, now, || Err("offline".into())), Err("offline".into()));
        let cache = read(&file).unwrap();
        assert_eq!((cache.checked, cache.latest.clone(), cache.tried), (None, None, Some(now)));
        assert_eq!(newer_in(Some(cache), "2.3.1"), None);

        // Within the wait nobody is asked, here or in another server.
        assert_eq!(round(&file, now + RETRY_AFTER - 1, || panic!("asked")), Ok(false));

        // After it, a success writes the version at that time, which `newer` then reads, and forgets the failure.
        let then = now + RETRY_AFTER;
        assert_eq!(round(&file, then, || Ok("2.4.0".into())), Ok(true));
        let cache = read(&file).unwrap();
        assert_eq!((cache.checked, cache.latest.clone(), cache.tried), (Some(then), Some("2.4.0".into()), None));
        assert_eq!(newer_in(Some(cache), "2.3.1").as_deref(), Some("2.4.0"));

        // Fresh: nobody is asked.
        assert_eq!(round(&file, then + 3600, || panic!("asked")), Ok(false));

        // A day later a failure keeps the old answer and its date, and notes the try.
        let later = then + MAX_AGE;
        assert_eq!(round(&file, later, || Err("offline".into())), Err("offline".into()));
        let cache = read(&file).unwrap();
        assert_eq!((cache.checked, cache.latest.clone(), cache.tried), (Some(then), Some("2.4.0".into()), Some(later)));
        assert_eq!(newer_in(Some(cache), "2.3.1").as_deref(), Some("2.4.0"));

        // Not before six hours (the `RETRY_AFTER` the failure waits for), then the next success clears it.
        assert_eq!(round(&file, later + 3600, || panic!("asked")), Ok(false));
        assert_eq!(round(&file, later + RETRY_AFTER, || Ok("2.5.0".into())), Ok(true));
        let cache = read(&file).unwrap();
        assert_eq!((cache.latest.as_deref(), cache.tried), (Some("2.5.0"), None));

        // Another server's answer, written while this one's curl was running, survives this one's failure.
        let other = later + RETRY_AFTER + MAX_AGE;
        let theirs = Cache { checked: Some(other - 5), latest: Some("2.6.0".into()), tried: None };
        let result = round(&file, other, || {
            store(&file, &theirs).unwrap();
            Err("offline".into())
        });
        assert_eq!(result, Err("offline".into()));
        let cache = read(&file).unwrap();
        assert_eq!(
            (cache.checked, cache.latest.as_deref(), cache.tried),
            (Some(other - 5), Some("2.6.0"), Some(other))
        );
    }
}
