// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The terminal while a full screen is on it ([`Session`]), given back as it was however the program ends, a panic
//! included ([`PanicGuard`]).

use std::io::{self, Stdout, Write};
use std::panic::{self, PanicHookInfo};
use std::sync::Arc;

use crossterm::{cursor, event, queue, terminal};

/// Mouse reports: presses, releases, the wheel and drags, in SGR coordinates; not the moves without a button, which
/// would wake the program at each one.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

/// The terminal while a full screen is on it: raw, on its alternate screen, without its cursor, with the mouse and
/// pastes reported. Dropping it gives everything back; a panic does it before its message.
pub(crate) struct Session {
    out: Stdout,
    _panic: PanicGuard,
}

impl Session {
    pub(crate) fn open() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // From here, Drop puts the terminal back.
        let mut session = Session { out: io::stdout(), _panic: PanicGuard::install(leave) };
        session.enter()?;
        Ok(session)
    }

    fn enter(&mut self) -> io::Result<()> {
        queue!(self.out, terminal::EnterAlternateScreen, cursor::Hide, event::EnableBracketedPaste)?;
        self.out.write_all(MOUSE_ON.as_bytes())?;
        self.out.flush()
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.out.write_all(bytes)?;
        self.out.flush()
    }

    /// Hands the terminal to another program (an editor) while `run` runs, then takes it back: the next frame must
    /// be sent whole.
    pub(crate) fn lend<T>(&mut self, run: impl FnOnce() -> T) -> io::Result<T> {
        leave();
        let result = run();
        terminal::enable_raw_mode()?;
        self.enter()?;
        Ok(result)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A panic: its hook did it already.
        if !std::thread::panicking() {
            leave();
        }
    }
}

/// The terminal as before a full screen: its own screen back, the cursor shown, nothing reported, out of raw mode.
fn leave() {
    let mut out = io::stdout();
    let _ = out.write_all(MOUSE_OFF.as_bytes());
    let _ = queue!(out, event::DisableBracketedPaste, cursor::Show, terminal::LeaveAlternateScreen);
    let _ = out.write_all(b"\x1b[0m");
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// While it lives, a panic runs `restore` before the hook in place when it was installed; dropping it puts that hook
/// back.
pub(crate) struct PanicGuard {
    before: Option<Arc<Hook>>,
}

impl PanicGuard {
    pub(crate) fn install(restore: fn()) -> Self {
        let before = Arc::new(panic::take_hook());
        let hook = Arc::clone(&before);
        panic::set_hook(Box::new(move |info| {
            restore();
            hook(info)
        }));
        PanicGuard { before: Some(before) }
    }
}

impl Drop for PanicGuard {
    fn drop(&mut self) {
        // A panicking thread cannot change the hook: this one stays, and puts the terminal back once more.
        if std::thread::panicking() {
            return;
        }
        // Dropping the list's hook leaves the one before alone in its Arc.
        drop(panic::take_hook());
        if let Some(before) = self.before.take().and_then(|before| Arc::try_unwrap(before).ok()) {
            panic::set_hook(before);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    static RESTORED: AtomicBool = AtomicBool::new(false);

    #[test]
    fn a_panic_puts_the_terminal_back_before_its_message() {
        let guard = PanicGuard::install(|| RESTORED.store(true, Ordering::SeqCst));
        let _ = panic::catch_unwind(|| panic!("essai"));
        assert!(RESTORED.load(Ordering::SeqCst));
        drop(guard);
        RESTORED.store(false, Ordering::SeqCst);
        let _ = panic::catch_unwind(|| panic!("essai, la garde relâchée"));
        assert!(!RESTORED.load(Ordering::SeqCst), "the hook from before is back");
    }
}
