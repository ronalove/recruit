// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Where a team's server is, and how to reach it (specs/multiplexeur-serveur.md §2).
//!
//! The authority is the lock in the team's state folder, `server.lock`, held by the server all its life, never by
//! anyone else for more than a look. Beside it, `server.json` says where its socket is; written by the server once
//! it holds the lock, and removed when it stops. The socket lives in `${RECRUIT_TMPDIR:-/tmp}/recruit-<uid>/`,
//! short enough for `sun_path`, the same from a graphical terminal and over SSH; its name binds it to the state
//! folder, so that a test's server never takes the socket of the user's team.
//!
//! Owner: dev-serveur.

use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::write_atomic;
use crate::t;

/// The longest socket path: `sun_path` holds 104 bytes on macOS, its final NUL included (108 on Linux).
pub(crate) const SUN_MAX: usize = 103;

/// The least a socket's name may have once the folder is counted: `-`, the hash, `.sock` and a little of the
/// session's name.
const NAME_MIN: usize = 24;

/// The kernel buffers of a server's and a client's socket: macOS gives a Unix socket 8 KiB, which would cut a full
/// frame into tens of writes.
pub(crate) const BUFFER: usize = 256 << 10;

const LOCK: &str = "server.lock";
const INFO: &str = "server.json";
/// What `server.json` becomes when its server died without stopping: the launch that follows knows it crashed
/// (`launch.rs`), whatever looked at the team in between.
const CRASHED: &str = "crashed";

/// How long a server that holds its lock without `server.json` yet is waited for: it is starting.
const STARTING: Duration = Duration::from_secs(2);

/// What `server.json` holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Info {
    pub pid: u32,
    pub socket: PathBuf,
    pub proto: u32,
    pub version: String,
    /// When it started, in seconds since the epoch.
    pub started: u64,
}

pub(crate) fn info_file(state: &Path) -> PathBuf {
    state.join(INFO)
}

pub(crate) fn crashed_file(state: &Path) -> PathBuf {
    state.join(CRASHED)
}

/// What a server that died without stopping left, cleaned: its socket removed (by the rule of `remove_stale`), its
/// `server.json` renamed `crashed`, over an older one. To be called with the lock held, the server surely gone.
pub(crate) fn mark_crashed(state: &Path, info: &Info) {
    let _ = remove_stale(&info.socket);
    let _ = fs::rename(info_file(state), crashed_file(state));
}

/// What a server stopped on purpose left (killed after it stopped answering), cleaned without a crash's mark. With
/// the lock held.
pub(crate) fn clean_stopped(state: &Path, info: &Info) {
    let _ = remove_stale(&info.socket);
    let _ = fs::remove_file(info_file(state));
}

pub(crate) fn read_info(state: &Path) -> Option<Info> {
    serde_json::from_slice(&fs::read(info_file(state)).ok()?).ok()
}

pub(crate) fn write_info(state: &Path, info: &Info) -> Result<()> {
    write_atomic(&info_file(state), &serde_json::to_string_pretty(info)?)
}

/// The folder of the sockets, made if missing: `${RECRUIT_TMPDIR:-/tmp}/recruit-<uid>`, ours and closed to the
/// others. One prepared by someone else is refused, as tmux does.
pub(crate) fn dir() -> Result<PathBuf> {
    let base = match std::env::var_os("RECRUIT_TMPDIR") {
        Some(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
        Some(dir) if !dir.is_empty() => {
            eprintln!(
                "{}",
                t!(
                    "recruit : RECRUIT_TMPDIR n'est pas un chemin absolu ({}), /tmp à la place.",
                    "recruit: RECRUIT_TMPDIR is not an absolute path ({}), /tmp instead.",
                    Path::new(&dir).display()
                )
            );
            PathBuf::from("/tmp")
        }
        _ => PathBuf::from("/tmp"),
    };
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let dir = base.join(format!("recruit-{uid}"));
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).with_context(|| t!("création de {}", "creating {}", dir.display())),
    }
    check_dir(&dir, uid)?;
    Ok(dir)
}

/// The sockets' folder must be a folder, not a link, owned by `uid`, without any right for the others.
fn check_dir(dir: &Path, uid: u32) -> Result<()> {
    let meta = fs::symlink_metadata(dir).with_context(|| dir.display().to_string())?;
    let why = if !meta.file_type().is_dir() {
        t!("ce n'est pas un dossier", "it is not a directory")
    } else if meta.uid() != uid {
        t!("il appartient à un autre utilisateur", "it belongs to another user")
    } else if meta.mode() & 0o077 != 0 {
        t!("d'autres que toi y ont accès (chmod 700)", "others than you can reach it (chmod 700)")
    } else {
        return Ok(());
    };
    bail!(t!("dossier des sockets refusé, {} : {}", "sockets' directory refused, {}: {}", dir.display(), why))
}

/// FNV-1a, 64 bits: enough to tell names apart, without a dependency.
fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in parts.iter().flat_map(|part| part.iter()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// A team's socket in `dir`: `<session>-<h>.sock`, where `h` names its state folder. A name too long for
/// `sun_path` is cut, on a character, before a longer hash of the whole session and the folder.
pub(crate) fn path(dir: &Path, session: &str, state: &Path) -> Result<PathBuf> {
    let state = state.as_os_str().as_bytes();
    let short = format!("{session}-{:08x}.sock", fnv64(&[state]) >> 32);
    let room = SUN_MAX.saturating_sub(dir.as_os_str().len() + 1);
    if short.len() <= room {
        return Ok(dir.join(short));
    }
    if room < NAME_MIN {
        bail!(t!(
            "le dossier des sockets est trop long pour un socket ({}) : donne à RECRUIT_TMPDIR un chemin plus court",
            "the sockets' directory is too long for a socket ({}): give RECRUIT_TMPDIR a shorter path",
            dir.display()
        ));
    }
    let hash = format!("-{:016x}.sock", fnv64(&[session.as_bytes(), b"\0", state]));
    let mut cut = room - hash.len();
    while !session.is_char_boundary(cut) {
        cut -= 1;
    }
    Ok(dir.join(format!("{}{hash}", &session[..cut])))
}

/// The uid of the process at the other end of `stream`.
pub(crate) fn peer_uid(stream: &impl AsRawFd) -> io::Result<u32> {
    let fd = stream.as_raw_fd();
    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
    {
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: getpeereid writes two integers we own.
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
    #[cfg(target_os = "linux")]
    {
        let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: SO_PEERCRED fills a ucred of the size given.
        let done =
            unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len) };
        if done != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
}

/// Whether the process at the other end of `stream` is ours.
pub(crate) fn same_user(stream: &impl AsRawFd) -> bool {
    // SAFETY: getuid cannot fail.
    peer_uid(stream).is_ok_and(|uid| uid == unsafe { libc::getuid() })
}

/// Sets a socket's send (`SO_SNDBUF`) or receive (`SO_RCVBUF`) buffer; the kernel may give less.
pub(crate) fn set_buffer(stream: &impl AsRawFd, option: libc::c_int, size: usize) {
    let size = size as libc::c_int;
    // SAFETY: an integer option of the size given.
    unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            option,
            (&raw const size).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

/// The state folder's lock, if no one holds it: then no server runs for it. Kept open, it is held; it is let go
/// only by closing it, never by `LOCK_UN` (`flock` holds for the open file, wherever a copy of it is).
pub(crate) fn try_lock(state: &Path) -> Result<Option<File>> {
    let file = state.join(LOCK);
    let handle = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&file)
        .with_context(|| t!("ouverture de {}", "opening {}", file.display()))?;
    // SAFETY: flock on a descriptor we own (CLOEXEC, as std opens every file).
    if unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(handle));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        return Ok(None);
    }
    Err(error).with_context(|| file.display().to_string())
}

/// Removes what is left at a socket's path by a server that is gone. A socket a server still answers on is left
/// where it is (`AddrInUse`), as is anything that is not a socket.
pub(crate) fn remove_stale(path: &Path) -> io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !meta.file_type().is_socket() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "not a socket"));
    }
    match UnixStream::connect(path) {
        Ok(_) => Err(io::ErrorKind::AddrInUse.into()),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// What tells a socket's file apart from another made at the same path: its device and inode, and the time of its
/// last change. The inode alone is not enough: Linux gives the one just freed to the next file made, and a server
/// that replaced a dead one's socket often gets its inode back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileId {
    dev: u64,
    ino: u64,
    ctime: (i64, i64),
}

impl FileId {
    fn of(meta: &fs::Metadata) -> FileId {
        FileId { dev: meta.dev(), ino: meta.ino(), ctime: (meta.ctime(), meta.ctime_nsec()) }
    }
}

/// A server's socket, ready: what a dead server left there removed, never one a server answers on. Returns it with
/// what tells the file apart, to remove it at the end only if it is still this one.
pub(crate) fn listen(path: &Path) -> Result<(UnixListener, FileId)> {
    remove_stale(path).map_err(|error| match error.kind() {
        io::ErrorKind::AddrInUse => anyhow::anyhow!(t!(
            "un autre serveur répond déjà sur {}",
            "another server already answers on {}",
            path.display()
        )),
        _ => anyhow::Error::new(error).context(path.display().to_string()),
    })?;
    let listener = UnixListener::bind(path).with_context(|| path.display().to_string())?;
    // The folder, 0700, already keeps the others out between the two. The change of mode comes before the
    // identity is taken: it changes the file's ctime.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| path.display().to_string())?;
    let meta = fs::symlink_metadata(path).with_context(|| path.display().to_string())?;
    Ok((listener, FileId::of(&meta)))
}

/// Removes a server's socket at its end, if the file there is still the one it made. One whose identity changed
/// since (someone changed its mode) stays: the next server cleans it, as it would a dead one's.
pub(crate) fn remove_own(path: &Path, id: FileId) {
    if fs::symlink_metadata(path).is_ok_and(|meta| FileId::of(&meta) == id) {
        let _ = fs::remove_file(path);
    }
}

/// Where a team's server stands, from its state folder.
pub(crate) enum Found {
    /// Connected to it, and it is ours.
    Running(UnixStream),
    /// No server: whatever a dead one left was cleaned.
    Gone,
}

/// Reaches the server of the team whose state folder is `state` (§2.4). The lock decides: free, no server runs,
/// and what one left is removed (no signal: the pid it noted may be another process's by now); held without
/// `server.json`, the server is starting, and is waited for; held with it, the pid is the holder's, and a socket
/// gone from `/tmp` is asked for again by SIGURG.
pub(crate) fn connect(state: &Path) -> Result<Found> {
    let until = Instant::now() + STARTING;
    let mut nudged = false;
    loop {
        let info = read_info(state);
        let mut missing = false;
        if let Some(info) = &info {
            match UnixStream::connect(&info.socket) {
                Ok(stream) if same_user(&stream) => {
                    set_buffer(&stream, libc::SO_RCVBUF, BUFFER);
                    return Ok(Found::Running(stream));
                }
                Ok(_) => bail!(t!(
                    "le socket {} est tenu par un autre utilisateur",
                    "socket {} is held by another user",
                    info.socket.display()
                )),
                Err(error) => missing = error.kind() == io::ErrorKind::NotFound,
            }
        }
        match try_lock(state)? {
            Some(_lock) => {
                // Held while cleaning: a server starting now waits for it. A `server.json` without a server: it died
                // without stopping, which the next launch must know.
                if let Some(info) = &info {
                    mark_crashed(state, info);
                }
                return Ok(Found::Gone);
            }
            None => {
                if let Some(info) = &info
                    && missing
                    && !nudged
                {
                    // SAFETY: a signal to the process that holds the lock: SIGURG, which does nothing by default.
                    unsafe { libc::kill(info.pid as libc::pid_t, libc::SIGURG) };
                    nudged = true;
                }
                if Instant::now() >= until {
                    let pid = info.map(|i| i.pid.to_string()).unwrap_or_else(|| "?".into());
                    bail!(t!(
                        "le serveur de l'équipe ne répond pas (pid {}, {})",
                        "the team's server does not answer (pid {}, {})",
                        pid,
                        state.display()
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_is_named_after_its_team_and_state() {
        let dir = Path::new("/tmp/recruit-501");
        let a = path(dir, "mux", Path::new("/Users/u/.cache/recruit/teams/mux")).unwrap();
        let b = path(dir, "mux", Path::new("/tmp/test/cache/recruit/teams/mux")).unwrap();
        assert!(a.to_string_lossy().starts_with("/tmp/recruit-501/mux-"), "{a:?}");
        assert!(a.to_string_lossy().ends_with(".sock"));
        assert_ne!(a, b, "two caches, one /tmp: two sockets");
        assert_eq!(a, path(dir, "mux", Path::new("/Users/u/.cache/recruit/teams/mux")).unwrap());
    }

    #[test]
    fn a_long_name_is_cut_on_a_character() {
        let dir = Path::new("/var/folders/8x/abcdefgh12345678ijklmnop9012/T/tmp.AbCdEf/recruit-501");
        let state = Path::new("/s");
        let long = format!("{}-dry-run", "é".repeat(40));
        let cut = path(dir, &long, state).unwrap();
        let text = cut.to_str().unwrap();
        assert!(text.len() <= SUN_MAX, "{} bytes", text.len());
        assert!(text.contains('é') && text.ends_with(".sock"));
        // Two long names that start alike stay apart.
        let other = format!("{}-dry-run", "é".repeat(39) + "a");
        assert_ne!(cut, path(dir, &other, state).unwrap());
        // A folder that leaves no room is refused.
        let deep = PathBuf::from(format!("/{}", "d".repeat(90)));
        assert!(path(&deep, "mux", state).is_err());
    }

    #[test]
    fn the_sockets_folder_must_be_ours_and_closed() {
        let temp = tempfile::tempdir().unwrap();
        // SAFETY: getuid cannot fail.
        let uid = unsafe { libc::getuid() };
        let good = temp.path().join("good");
        fs::DirBuilder::new().mode(0o700).create(&good).unwrap();
        assert!(check_dir(&good, uid).is_ok());
        assert!(check_dir(&good, uid + 1).is_err(), "someone else's");
        let open = temp.path().join("open");
        fs::DirBuilder::new().mode(0o700).create(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check_dir(&open, uid).is_err(), "readable by others");
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert!(check_dir(&link, uid).is_err(), "a link");
    }

    #[test]
    fn a_dead_servers_socket_goes_a_live_ones_stays() {
        let temp = tempfile::tempdir().unwrap();
        let sock = temp.path().join("s.sock");
        let (listener, id) = listen(&sock).unwrap();
        assert!(same_user(&UnixStream::connect(&sock).unwrap()));
        // Someone answers on it: not taken, not removed.
        assert!(listen(&sock).is_err());
        assert_eq!(remove_stale(&sock).unwrap_err().kind(), io::ErrorKind::AddrInUse);
        assert!(sock.exists());
        // The old file kept alive by a second name, so that its inode cannot go to the next one: the identities
        // then always differ, as they must (with the inode freed, Linux often gives it back, and only the ctime
        // would tell them apart, within the clock's tick).
        let kept = temp.path().join("kept.sock");
        fs::hard_link(&sock, &kept).unwrap();
        drop(listener);
        // No one answers any more: removed, and taken again. Not at once in every case: another test that forks
        // (a PTY's program, a pane) gives its child a copy of every descriptor of this process, the listener's
        // among them, until the child's `exec` closes it (CLOEXEC). Meanwhile the socket still takes connections,
        // and is rightly left where it is. A bounded wait: a socket that went on answering would still fail.
        let until = std::time::Instant::now() + Duration::from_secs(2);
        let (_again, other) = loop {
            match listen(&sock) {
                Ok(listening) => break listening,
                Err(error) if std::time::Instant::now() < until => {
                    assert!(error.to_string().contains(&sock.display().to_string()), "{error:#}");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error:#}"),
            }
        };
        assert_ne!(id, other);
        remove_own(&sock, id);
        assert!(sock.exists(), "not the old one's any more");
        remove_own(&sock, other);
        assert!(!sock.exists());
        // Not a socket: left alone.
        fs::write(&sock, "x").unwrap();
        assert!(remove_stale(&sock).is_err());
        assert!(sock.exists());
    }

    #[test]
    fn the_lock_says_who_runs() {
        let state = tempfile::tempdir().unwrap();
        let held = try_lock(state.path()).unwrap().expect("free");
        assert!(try_lock(state.path()).unwrap().is_none(), "held");
        drop(held);
        assert!(try_lock(state.path()).unwrap().is_some(), "free again once closed");
    }

    #[test]
    fn a_stale_info_is_cleaned_and_its_pid_left_alone() {
        let state = tempfile::tempdir().unwrap();
        let sock = state.path().join("gone.sock");
        drop(listen(&sock).unwrap());
        // A pid that is surely alive and not a server: ours. No signal may reach it.
        let info = Info { pid: std::process::id(), socket: sock.clone(), proto: 1, version: "x".into(), started: 0 };
        write_info(state.path(), &info).unwrap();
        // A crash's mark, from an older one, is replaced.
        fs::write(crashed_file(state.path()), "old").unwrap();
        assert!(matches!(connect(state.path()).unwrap(), Found::Gone));
        assert!(read_info(state.path()).is_none());
        assert!(!sock.exists());
        // The crash is marked: `server.json` as it was, for the next launch.
        let marked: Info = serde_json::from_slice(&fs::read(crashed_file(state.path())).unwrap()).unwrap();
        assert_eq!(marked, info);
        // A look after that changes nothing.
        assert!(matches!(connect(state.path()).unwrap(), Found::Gone));
        assert!(crashed_file(state.path()).exists());
    }

    #[test]
    fn a_stop_on_purpose_leaves_no_mark() {
        let state = tempfile::tempdir().unwrap();
        let sock = state.path().join("gone.sock");
        drop(listen(&sock).unwrap());
        let info = Info { pid: 1, socket: sock.clone(), proto: 1, version: "x".into(), started: 0 };
        write_info(state.path(), &info).unwrap();
        clean_stopped(state.path(), &info);
        assert!(read_info(state.path()).is_none());
        assert!(!sock.exists());
        assert!(!crashed_file(state.path()).exists());
    }

    #[test]
    fn a_running_server_is_reached() {
        let state = tempfile::tempdir().unwrap();
        let _lock = try_lock(state.path()).unwrap().unwrap();
        let sock = state.path().join("s.sock");
        let (_listener, _) = listen(&sock).unwrap();
        let info = Info { pid: std::process::id(), socket: sock, proto: 1, version: "x".into(), started: 0 };
        write_info(state.path(), &info).unwrap();
        assert!(matches!(connect(state.path()).unwrap(), Found::Running(_)));
    }
}
