// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! A panel's keyboard kept quiet, its terminal given back as it was, and the color scheme reports and resizes the
//! multiplexer sends it.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::ui::look::{self};
use crate::ui::terminal::PanicGuard;

/// The key that asks for the whole board again, as everywhere in a terminal.
const CTRL_L: u8 = 0x0c;

/// Asks the multiplexer for the colors of the client's terminal, and for a notice each time they change (the color
/// scheme reports, `CSI ? 996 n` and `CSI ? 2031 h`); the answer and the notices are `CSI ? 997 ; 1 n` for a dark
/// background, `CSI ? 997 ; 2 n` for a light one.
const ASK_SCHEME: &str = "\x1b[?996n\x1b[?2031h";
const STOP_SCHEME: &str = "\x1b[?2031l";

/// The color scheme reports in `bytes`, as `true` for a light background; what is left of one cut in two stays in
/// `bytes` for the next read, and everything else is dropped.
pub(super) fn scheme_reports(bytes: &mut Vec<u8>) -> Vec<bool> {
    const HEAD: &[u8] = b"\x1b[?997;";
    let mut found = Vec::new();
    while let Some(at) = bytes.windows(HEAD.len()).position(|w| w == HEAD) {
        let rest = &bytes[at + HEAD.len()..];
        match rest {
            [kind, b'n', ..] => {
                found.push(*kind == b'2');
                bytes.drain(..at + HEAD.len() + 2);
            }
            // Not whole yet, or not one of ours: the next read says.
            [_] | [] => {
                bytes.drain(..at);
                return found;
            }
            _ => {
                bytes.drain(..at + HEAD.len());
            }
        }
    }
    // The tail that may begin a report.
    let keep =
        (1..HEAD.len()).rev().find(|&n| bytes.len() >= n && HEAD.starts_with(&bytes[bytes.len() - n..])).unwrap_or(0);
    bytes.drain(..bytes.len() - keep);
    found
}

/// A panel's keyboard, kept quiet: what is typed in its pane is neither echoed nor gathered into lines, so that
/// nothing moves the screen (an Enter echoed on the last line would scroll every line up one, and the board, written
/// by difference, would stay shifted). What comes is read on the side, and dropped; Ctrl-L asks for the whole board
/// again (`redraw`). Signals stay as they were: Ctrl-C stops the panel. Nothing when the input is no terminal. The
/// terminal is given back as it was once the guard goes, a panic included (a panel run by hand in a terminal).
pub(super) fn quiet_input(redraw: Arc<AtomicBool>) -> Option<QuietInput> {
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut term) } != 0 {
        return None;
    }
    let _ = ORIGINAL.set(term);
    term.c_lflag &= !(libc::ECHO | libc::ICANON);
    term.c_cc[libc::VMIN] = 1;
    term.c_cc[libc::VTIME] = 0;
    if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term) } != 0 {
        return None;
    }
    let guard = QuietInput { _panic: PanicGuard::install(leave) };
    // Stopped by a signal (Ctrl-C, the pane closed): given back too, then stopped as the signal would.
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
    }
    // The client's colors: dark until the answer comes, the first image not waiting for it.
    {
        let mut out = std::io::stdout();
        let _ = out.write_all(ASK_SCHEME.as_bytes());
        let _ = out.flush();
    }
    std::thread::spawn(move || {
        let mut bytes = [0u8; 64];
        let mut pending: Vec<u8> = Vec::new();
        let mut input = std::io::stdin();
        while let Ok(read) = input.read(&mut bytes) {
            if read == 0 {
                break;
            }
            if bytes[..read].contains(&CTRL_L) {
                redraw.store(true, Ordering::Relaxed);
            }
            pending.extend_from_slice(&bytes[..read]);
            for light in scheme_reports(&mut pending) {
                // Only when the colors changed: all the board again, in the others.
                if look::is_light() != light {
                    look::set_light(light);
                    redraw.store(true, Ordering::Relaxed);
                }
            }
        }
    });
    Some(guard)
}

/// The terminal's settings before [`quiet_input`].
static ORIGINAL: OnceLock<libc::termios> = OnceLock::new();

/// The terminal's settings as they were before [`quiet_input`].
fn loud_input() {
    if let Some(term) = ORIGINAL.get() {
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, term) };
    }
}

/// The terminal as it was: its settings given back, the color scheme notices off. For a panic and for a signal, so
/// it only calls what a signal handler may (`tcsetattr`, `write`).
pub(super) fn leave() {
    loud_input();
    unsafe { libc::write(libc::STDOUT_FILENO, STOP_SCHEME.as_ptr().cast(), STOP_SCHEME.len()) };
}

/// A signal that stops a panel: the terminal as it was, then the signal's own way.
extern "C" fn on_signal(signal: libc::c_int) {
    leave();
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// While it lives, the keyboard stays quiet (see [`quiet_input`]).
pub(super) struct QuietInput {
    _panic: PanicGuard,
}

impl Drop for QuietInput {
    fn drop(&mut self) {
        // A panic: its hook did it already.
        if !std::thread::panicking() {
            leave();
        }
    }
}

/// Set by SIGWINCH: the multiplexer gives the pane its size again whenever it may have lost what it showed (it
/// replaced its engine after a panic), the same size included. The whole board is then written again.
pub(super) static RESIZED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_resize(_: libc::c_int) {
    RESIZED.store(true, Ordering::Relaxed);
}

/// Has SIGWINCH note [`RESIZED`]. Restarting, so that the read of the keyboard is not cut short by it.
pub(super) fn watch_resizes() {
    // SAFETY: the handler only stores to an atomic; the `sigaction` is zeroed, then given a handler and its flag.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_resize as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut());
    }
}

#[cfg(test)]
mod tests {

    use std::sync::atomic::Ordering;

    use super::*;

    /// The multiplexer gives a pane its size again, the same one included, when its engine was replaced: SIGWINCH
    /// makes the dashboard write the whole board again.
    #[test]
    fn a_resize_signal_asks_for_the_whole_board() {
        watch_resizes();
        RESIZED.store(false, Ordering::Relaxed);
        // SAFETY: a signal to this very process, whose handler `watch_resizes` just put in place.
        assert_eq!(unsafe { libc::raise(libc::SIGWINCH) }, 0);
        assert!(RESIZED.swap(false, Ordering::Relaxed));
        assert!(!RESIZED.load(Ordering::Relaxed));
    }

    /// The multiplexer's answer to `CSI ? 996 n`, and its notices, however the reads cut them: the others are dropped.
    #[test]
    fn color_scheme_reports_cut_anywhere() {
        let mut bytes = b"abc\x1b[?997;".to_vec();
        assert!(scheme_reports(&mut bytes).is_empty());
        assert_eq!(bytes, b"\x1b[?997;");
        bytes.extend(b"2n typed \x1b[?99");
        assert_eq!(scheme_reports(&mut bytes), [true]);
        assert_eq!(bytes, b"\x1b[?99");
        bytes.extend(b"7;1n\x1b[?997;2n");
        assert_eq!(scheme_reports(&mut bytes), [false, true]);
        assert!(bytes.is_empty());
        // What is not one is never kept: another report, a key.
        bytes.extend(b"\x1b[?996n\x1b[?997;x1 q \x1b[A");
        assert!(scheme_reports(&mut bytes).is_empty());
        assert!(bytes.len() < 3, "{bytes:?}");
    }
}
