// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The sessions of Claude Code and the files it keeps for them in its profile: which runs, what it says of itself, its
//! conversations.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{Profile, Running};

/// The contents of Claude's process `pid`'s own file in the profile (`sessions/<pid>.json`), when it reads and is the
/// process's own: a pid seen a few seconds ago may be another process's by now.
fn session_of(config_dir: Option<&Path>, pid: u32) -> Option<serde_json::Value> {
    let file = Profile::of(config_dir).folder().join("sessions").join(format!("{pid}.json"));
    let session: serde_json::Value = serde_json::from_str(&fs::read_to_string(file).ok()?).ok()?;
    (session["pid"].as_u64() == Some(u64::from(pid))).then_some(session)
}

/// What a session's own file in the profile says of it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionFile {
    /// `busy`, `idle`, `shell` while a command it started runs past its turn…
    pub status: String,
    /// Since when, in seconds since the epoch, when it says it in a way read here.
    pub since: Option<i64>,
}

/// A session's own file in the profile (`sessions/<pid>.json`); None when missing, another process's (a pid seen a
/// few seconds ago, gone since) or without a status. Its time (`statusUpdatedAt`, in milliseconds, whole or not) may
/// be missing: the status stands on its own.
pub fn session_status(config_dir: Option<&Path>, pid: u32) -> Option<SessionFile> {
    let session = session_of(config_dir, pid)?;
    let since =
        session["statusUpdatedAt"].as_f64().filter(|ms| ms.is_finite() && *ms > 0.0).map(|ms| (ms / 1000.0) as i64);
    Some(SessionFile { status: session["status"].as_str()?.to_string(), since })
}

/// The session Claude's process `pid` runs, from its own file in the profile (`sessions/<pid>.json`, `sessionId`; after
/// a `/clear`, the new one): what `claude agents` says, for a file read. None when the file is missing, unreadable,
/// another process's, or says no session: an undocumented format, so its reader falls back on something else.
pub fn session_id(config_dir: Option<&Path>, pid: u32) -> Option<String> {
    let session = session_of(config_dir, pid)?;
    session["sessionId"].as_str().filter(|id| !id.is_empty()).map(str::to_string)
}

/// The exact address ListAgents gives a session (`name [ref]`), from its own file in the profile: the first six hex
/// digits of SHA-256 over `session:` and its messaging socket's path (Claude Code 2.1.294). None when the file is
/// missing or says no socket: then no address, and nothing else changes.
pub fn session_ref(config_dir: Option<&Path>, pid: u32) -> Option<String> {
    let session = session_of(config_dir, pid)?;
    let socket = session["messagingSocketPath"].as_str().filter(|s| !s.is_empty())?;
    Some(ref_of(socket))
}

pub(super) fn ref_of(socket: &str) -> String {
    sha256(format!("session:{socket}").as_bytes())[..3].iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 (FIPS 180-4), for the few bytes of a session's address: no crate for that.
pub(super) fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (state, add) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *state = state.wrapping_add(add);
        }
    }
    let mut out = [0u8; 32];
    for (bytes, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *bytes = word.to_be_bytes();
    }
    out
}

/// The exact addresses of the team's `members` whose name more than one open session carries (another team, a
/// session opened by hand): member → ref. Each member's own session is the one `noted` for it (`recruit _member`
/// notes its session id), else the only one under its name in the team's folder `dir`. A member without a session
/// found, or without a ref (`reference`, from its session's file), is left out.
pub fn addresses(
    dir: &Path,
    members: &[&str],
    running: &[Running],
    noted: impl Fn(&str) -> Option<String>,
    reference: impl Fn(u32) -> Option<String>,
) -> std::collections::BTreeMap<String, String> {
    let carried = |name: &str| running.iter().filter(|r| r.name.as_deref() == Some(name)).count();
    members
        .iter()
        .filter(|name| carried(name) > 1)
        .filter_map(|&name| {
            let id = noted(name).filter(|id| !id.is_empty());
            let by_id = id.and_then(|id| running.iter().find(|r| r.session_id.as_deref() == Some(id.as_str())));
            let mut here = running.iter().filter(|r| {
                r.name.as_deref() == Some(name) && r.cwd.as_deref().is_some_and(|cwd| Path::new(cwd) == dir)
            });
            let only_here = match (here.next(), here.next()) {
                (Some(one), None) => Some(one),
                _ => None,
            };
            let session = by_id.or(only_here)?;
            Some((name.to_string(), reference(session.pid?)?))
        })
        .collect()
}

/// The folder name Claude Code gives a project: every character that is not an ASCII letter or digit becomes a
/// dash, one per UTF-16 unit as in JavaScript.
pub fn project_key(dir: &Path) -> String {
    let mut key = String::new();
    for c in dir.to_string_lossy().chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c);
        } else {
            key.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    key
}

/// The conversation file of a session.
pub fn transcript(dir: &Path, config_dir: Option<&Path>, session_id: &str) -> PathBuf {
    Profile::of(config_dir).folder().join("projects").join(project_key(dir)).join(format!("{session_id}.jsonl"))
}

/// For each session name, the most recent conversation that carries it in this directory: the last title given
/// in each conversation file, newest file first.
pub fn last_conversations(dir: &Path, config_dir: Option<&Path>) -> Result<HashMap<String, String>> {
    let folder = Profile::of(config_dir).folder().join("projects").join(project_key(dir));
    let Ok(entries) = fs::read_dir(&folder) else { return Ok(HashMap::new()) };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));

    let mut found = HashMap::new();
    for (_, file) in files {
        let Some(title) = last_title(&file) else { continue };
        let Some(id) = file.file_stem().and_then(|s| s.to_str()) else { continue };
        found.entry(title).or_insert_with(|| id.to_string());
    }
    Ok(found)
}

pub(super) fn last_title(file: &Path) -> Option<String> {
    let handle = fs::File::open(file).ok()?;
    if handle.metadata().ok()?.len() == 0 {
        return None;
    }
    // SAFETY: read-only map of a file Claude Code only appends to; a concurrent append does not touch the
    // bytes already mapped.
    let map = unsafe { memmap2::Mmap::map(&handle) }.ok()?;
    let at = memchr::memmem::rfind(&map, br#""type":"custom-title""#)?;
    let start = map[..at].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let end = memchr::memchr(b'\n', &map[at..]).map_or(map.len(), |i| at + i);
    let line: serde_json::Value = serde_json::from_slice(&map[start..end]).ok()?;
    line.get("customTitle")?.as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha256_as_published() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // Two blocks: the length no longer fits after the message.
        assert_eq!(
            hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(hex(&sha256(&[b'a'; 1000])), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }

    /// As ListAgents showed them (Claude Code 2.1.294).
    #[test]
    fn refs_as_list_agents_shows_them() {
        assert_eq!(ref_of("/tmp/cc-socks/72947.sock"), "551272");
        assert_eq!(ref_of("/tmp/cc-socks/72848.sock"), "8dd997");
        assert_eq!(ref_of("/tmp/cc-socks/72860.sock"), "3779a7");
    }

    #[test]
    fn session_ref_from_its_file() {
        let profile = tempfile::tempdir().unwrap();
        let sessions = profile.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("72947.json"), r#"{"pid":72947,"messagingSocketPath":"/tmp/cc-socks/72947.sock"}"#)
            .unwrap();
        assert_eq!(session_ref(Some(profile.path()), 72947).as_deref(), Some("551272"));
        // Missing, empty, or another format: no address.
        assert_eq!(session_ref(Some(profile.path()), 1), None);
        fs::write(sessions.join("2.json"), r#"{"pid":2,"messagingSocketPath":""}"#).unwrap();
        assert_eq!(session_ref(Some(profile.path()), 2), None);
        fs::write(sessions.join("3.json"), r#"{"pid":3,"messaging":{"socket":"/tmp/x.sock"}}"#).unwrap();
        assert_eq!(session_ref(Some(profile.path()), 3), None);
        fs::write(sessions.join("4.json"), "not json").unwrap();
        assert_eq!(session_ref(Some(profile.path()), 4), None);
        // Another process's file (the pid was reused): no address, as for the session and its status.
        fs::write(sessions.join("5.json"), r#"{"pid":6,"messagingSocketPath":"/tmp/cc-socks/6.sock"}"#).unwrap();
        assert_eq!(session_ref(Some(profile.path()), 5), None);
    }

    #[test]
    fn session_id_from_its_file() {
        let profile = tempfile::tempdir().unwrap();
        let sessions = profile.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let write = |pid: u32, json: &str| fs::write(sessions.join(format!("{pid}.json")), json).unwrap();
        write(10, r#"{"pid":10,"sessionId":"s-10","cwd":"/w","name":"dev","status":"idle"}"#);
        assert_eq!(session_id(Some(profile.path()), 10).as_deref(), Some("s-10"));
        // Missing, another process's, without a session, empty, or another format: none.
        assert_eq!(session_id(Some(profile.path()), 1), None);
        write(11, r#"{"pid":12,"sessionId":"s-12"}"#);
        assert_eq!(session_id(Some(profile.path()), 11), None);
        write(13, r#"{"pid":13}"#);
        assert_eq!(session_id(Some(profile.path()), 13), None);
        write(14, r#"{"pid":14,"sessionId":""}"#);
        assert_eq!(session_id(Some(profile.path()), 14), None);
        write(15, r#"{"pid":15,"session":{"id":"s-15"}}"#);
        assert_eq!(session_id(Some(profile.path()), 15), None);
        write(16, "not json");
        assert_eq!(session_id(Some(profile.path()), 16), None);
    }

    fn session(name: &str, cwd: &str, id: &str, pid: u32) -> Running {
        Running {
            name: Some(name.into()),
            cwd: Some(cwd.into()),
            status: None,
            session_id: Some(id.into()),
            pid: Some(pid),
        }
    }

    #[test]
    fn addresses_only_for_names_carried_twice() {
        let running = [
            session("dev", "/team", "s1", 1),
            session("dev", "/other", "s2", 2),
            session("lead", "/team", "s3", 3),
            session("qa", "/team", "s4", 4),
            session("qa", "/team", "s5", 5),
            session("ops", "/team", "s6", 6),
            session("ops", "/other", "s7", 7),
        ];
        let noted = |member: &str| match member {
            "qa" => Some("s5".to_string()),
            "ops" => Some("gone".to_string()),
            _ => None,
        };
        let reference = |pid: u32| (pid != 6).then(|| format!("ref{pid}"));
        let found = addresses(Path::new("/team"), &["dev", "lead", "qa", "ops"], &running, noted, reference);
        // dev: its only session in the team's folder; qa: the one noted, though both are here; lead: alone with its
        // name; ops: found, but its file gives no address.
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            [("dev".into(), "ref1".into()), ("qa".into(), "ref5".into())]
        );
        // Two in the team's folder and none noted: no way to tell, no address.
        let found = addresses(Path::new("/team"), &["qa"], &running, |_| None, |pid| Some(pid.to_string()));
        assert!(found.is_empty());
    }

    #[test]
    fn session_status_from_its_file() {
        let profile = tempfile::tempdir().unwrap();
        let sessions = profile.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("42.json"), r#"{"pid":42,"status":"shell","statusUpdatedAt":1791530838682}"#).unwrap();
        let file = |status: &str, since| Some(SessionFile { status: status.into(), since });
        assert_eq!(session_status(Some(profile.path()), 42), file("shell", Some(1_791_530_838)));
        // Its time written otherwise, or not at all: the status alone.
        fs::write(sessions.join("45.json"), r#"{"pid":45,"status":"shell","statusUpdatedAt":1791530838682.5}"#)
            .unwrap();
        assert_eq!(session_status(Some(profile.path()), 45), file("shell", Some(1_791_530_838)));
        fs::write(sessions.join("46.json"), r#"{"pid":46,"status":"shell","statusUpdatedAt":"soon"}"#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 46), file("shell", None));
        fs::write(sessions.join("47.json"), r#"{"pid":47,"status":"shell"}"#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 47), file("shell", None));
        // Missing, or not as expected: nothing.
        assert_eq!(session_status(Some(profile.path()), 7), None);
        fs::write(sessions.join("43.json"), r#"{"pid":43,"status":3}"#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 43), None);
        fs::write(sessions.join("44.json"), r#"{"pid":44,"status":"idle""#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 44), None);
        // Another process's, or without its pid: nothing.
        fs::write(sessions.join("48.json"), r#"{"pid":49,"status":"shell"}"#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 48), None);
        fs::write(sessions.join("50.json"), r#"{"status":"shell"}"#).unwrap();
        assert_eq!(session_status(Some(profile.path()), 50), None);
    }

    #[test]
    fn project_keys() {
        assert_eq!(project_key(Path::new("/home/user/code/my-project")), "-home-user-code-my-project");
        assert_eq!(project_key(Path::new("/tmp/a.b_c/é")), "-tmp-a-b-c--");
    }

    #[test]
    fn titles_from_conversation_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc.jsonl");
        fs::write(
            &file,
            concat!(
                r#"{"type":"user","message":"x"}"#,
                "\n",
                r#"{"type":"custom-title","customTitle":"ancien"}"#,
                "\n",
                r#"{"type":"custom-title","customTitle":"opérateur"}"#,
                "\n",
                r#"{"type":"assistant"}"#,
                "\n"
            ),
        )
        .unwrap();
        assert_eq!(last_title(&file).as_deref(), Some("opérateur"));
        fs::write(&file, "").unwrap();
        assert_eq!(last_title(&file), None);
    }

    #[test]
    fn transcripts() {
        assert_eq!(
            transcript(Path::new("/tmp/p"), Some(Path::new("/profil")), "id"),
            Path::new("/profil/projects/-tmp-p/id.jsonl")
        );
    }
}
