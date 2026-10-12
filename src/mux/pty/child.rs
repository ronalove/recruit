// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The child's side of starting a program: everything `exec` needs, made before `fork` (the child must not allocate),
//! and what the child runs between `fork` and `exec`, which is async-signal-safe calls only.

use std::ffi::{CString, OsStr, OsString, c_char};
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::Spawn;
use crate::t;

/// How many descriptors this process may have: its descriptors are below. Asked before `fork`.
fn table_size() -> libc::c_int {
    // macOS: the soft limit, within the kernel's own per-process maximum (an unlimited soft limit included).
    #[cfg(target_vendor = "apple")]
    // SAFETY: a plain query.
    let size = unsafe { libc::getdtablesize() };
    #[cfg(not(target_vendor = "apple"))]
    let size = {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: a plain query into a local.
        let soft = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
            limit.rlim_cur
        } else {
            libc::RLIM_INFINITY
        };
        // Unlimited, or more than Linux allows by default (`fs.nr_open`): that default.
        soft.min(1 << 20) as libc::c_int
    };
    size.max(3)
}

/// Everything `exec` needs, made before `fork`: the child must not allocate.
pub(super) struct Exec {
    path: CString,
    _argv: Vec<CString>,
    argv: Vec<*const c_char>,
    _envp: Vec<CString>,
    envp: Vec<*const c_char>,
    cwd: Option<CString>,
    /// Written to the terminal, before the error number, if the program cannot be started after all.
    failed: Vec<u8>,
    /// The size of the descriptor table: where the child stops closing them one by one ([`Exec::close_the_rest`]).
    table: libc::c_int,
}

impl Exec {
    pub(super) fn new(spawn: &Spawn) -> io::Result<Exec> {
        let env = environment(spawn);
        let cwd = (!spawn.cwd.as_os_str().is_empty()).then_some(spawn.cwd.as_path());
        if let Some(cwd) = cwd
            && !cwd.is_dir()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                t!("{} : dossier introuvable", "{}: no such directory", cwd.display()),
            ));
        }
        let path = resolve(&spawn.program, cwd.unwrap_or(Path::new("")), &env)?;
        let argv: Vec<CString> =
            std::iter::once(&spawn.program).chain(&spawn.args).map(|arg| c_string(arg)).collect::<io::Result<_>>()?;
        let envp: Vec<CString> = env
            .iter()
            .map(|(key, value)| {
                let mut pair = key.as_bytes().to_vec();
                pair.push(b'=');
                pair.extend(value.as_bytes());
                c_string(OsStr::from_bytes(&pair))
            })
            .collect::<io::Result<_>>()?;
        let failed = t!(
            "recruit : impossible de lancer {} (erreur ",
            "recruit: cannot start {} (error ",
            spawn.program.to_string_lossy()
        );
        Ok(Exec {
            path: c_string(path.as_os_str())?,
            argv: argv.iter().map(|arg| arg.as_ptr()).chain([std::ptr::null()]).collect(),
            _argv: argv,
            envp: envp.iter().map(|pair| pair.as_ptr()).chain([std::ptr::null()]).collect(),
            _envp: envp,
            cwd: cwd.map(|cwd| c_string(cwd.as_os_str())).transpose()?,
            failed: failed.into_bytes(),
            table: table_size(),
        })
    }

    /// The child's side, between `fork` and `exec`: default signals, the terminal as its standard descriptors (first,
    /// so that a failure is told there) and no other, a session of its own with the terminal as its controlling one,
    /// its directory, then the program. Only async-signal-safe calls.
    ///
    /// SAFETY: to be called in the child, right after `fork`.
    pub(super) unsafe fn run(&self, slave: RawFd) -> ! {
        // SAFETY: async-signal-safe calls on memory made before `fork`.
        unsafe {
            // Ignored signals stay ignored across `exec` (Rust ignores SIGPIPE): the program gets the defaults back.
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            for signal in 1..32 {
                if signal != libc::SIGKILL && signal != libc::SIGSTOP {
                    libc::signal(signal, libc::SIG_DFL);
                }
            }
            // Above the standard descriptors first: `dup2` onto itself would leave it `CLOEXEC`.
            let slave = if slave < 3 { libc::fcntl(slave, libc::F_DUPFD, 3) } else { slave };
            if slave < 0 {
                self.fail();
            }
            for fd in 0..3 {
                if libc::dup2(slave, fd) < 0 {
                    self.fail();
                }
            }
            self.close_the_rest();
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                self.fail();
            }
            if let Some(cwd) = &self.cwd
                && libc::chdir(cwd.as_ptr()) < 0
            {
                self.fail();
            }
            libc::execve(self.path.as_ptr(), self.argv.as_ptr(), self.envp.as_ptr());
            self.fail()
        }
    }

    /// Closes every descriptor above the standard ones, the terminal's copy among them. `CLOEXEC` is not enough: this
    /// process may hold descriptors without it that it did not open, inherited from whatever started it (a CI runner
    /// leaves some), or being made by another thread (Rust makes a pipe in two steps on macOS, which has no `pipe2`).
    /// The program would keep them open, a pipe's write end among them, so that its reader never sees its end. As
    /// tmux does (`closefrom`).
    ///
    /// SAFETY: in the child, between `fork` and `exec`: `close_range` and `close` are async-signal-safe.
    unsafe fn close_the_rest(&self) {
        // SAFETY: plain system calls on descriptors this process owns.
        unsafe {
            // Linux 5.9 and later; otherwise (an older kernel, a sandbox that refuses it), one by one.
            #[cfg(target_os = "linux")]
            if libc::syscall(libc::SYS_close_range, 3 as libc::c_uint, libc::c_uint::MAX, 0 as libc::c_uint) == 0 {
                return;
            }
            for fd in 3..self.table {
                libc::close(fd);
            }
        }
    }

    /// Says why on the terminal, without allocating, and ends the child as a shell would.
    unsafe fn fail(&self) -> ! {
        // SAFETY: async-signal-safe calls, on memory made before `fork` and on the stack.
        unsafe {
            let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            let mut digits = [0u8; 12];
            let mut at = digits.len();
            let mut n = errno.unsigned_abs();
            loop {
                at -= 1;
                digits[at] = b'0' + (n % 10) as u8;
                n /= 10;
                if n == 0 {
                    break;
                }
            }
            libc::write(2, self.failed.as_ptr().cast(), self.failed.len());
            libc::write(2, digits[at..].as_ptr().cast(), digits.len() - at);
            libc::write(2, b")\r\n".as_ptr().cast(), 3);
            libc::_exit(127)
        }
    }
}

fn c_string(text: &OsStr) -> io::Result<CString> {
    CString::new(text.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, t!("octet nul dans {:?}", "NUL byte in {:?}", text)))
}

/// The program's environment: this one's, less `env_remove`, plus `env`.
fn environment(spawn: &Spawn) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> =
        std::env::vars_os().filter(|(key, _)| !spawn.env_remove.contains(key)).collect();
    for (key, value) in &spawn.env {
        env.retain(|(known, _)| known != key);
        env.push((key.clone(), value.clone()));
    }
    env
}

/// Where `program` is: as given if it has a slash (from `cwd` when relative, as `exec` will after `chdir`), else
/// the first executable file of that name in the program's `PATH`.
fn resolve(program: &OsStr, cwd: &Path, env: &[(OsString, OsString)]) -> io::Result<PathBuf> {
    let executable =
        |path: &Path| path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    let not_found =
        || io::Error::new(io::ErrorKind::NotFound, t!("{} : introuvable", "{}: not found", program.to_string_lossy()));
    if program.as_bytes().contains(&b'/') {
        return if executable(&cwd.join(program)) { Ok(program.into()) } else { Err(not_found()) };
    }
    if program.is_empty() {
        return Err(not_found());
    }
    let path = env.iter().find(|(key, _)| key == "PATH").map_or(OsStr::new("/usr/bin:/bin"), |(_, path)| path);
    std::env::split_paths(path)
        .map(|dir| cwd.join(dir).join(program))
        .find(|path| executable(path))
        .ok_or_else(not_found)
}
