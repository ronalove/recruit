// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! What the menu does with the sheet's [`Effect`]s: the writes to the running team, the editor, Claude composing a new
//! agent and the mod answering « Reprendre », on the side, and the processes stopped when the menu closes.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::Result;

use super::sheet::{self, Done, Effect, Said, Sheet};
use super::{Live, MOVES_OFF, Menu, News, edit_text, editor, load};
use crate::config::Edit;
use crate::create::templates::Templates;
use crate::{claude, claude::bridge, create::compose, t};

/// Past this, the menu stops waiting for what a member's mod did with « Reprendre »: `bridge::resumption` always
/// answers, on its own lapse; this is only a bound, should it not.
const RESUME_BOUND: Duration = Duration::from_secs(30);

impl Menu<'_> {
    /// Carries out effects in turn; true when the menu is done.
    pub(super) fn carry_all(&mut self, sheet: &mut Sheet, effects: Vec<Effect>) -> Result<bool> {
        for effect in effects {
            if self.carry(sheet, effect)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Carries out an effect; true when the menu is done.
    fn carry(&mut self, sheet: &mut Sheet, effect: Effect) -> Result<bool> {
        let failed = |error: anyhow::Error| Said::Failed(format!("{error:#}"));
        if sheet.quick.is_some()
            && let Effect::Apply { edits, .. } = &effect
        {
            // The quick list ends with the write; refused, it stays open and says why on its last line.
            match self.running.apply(edits) {
                Ok(()) => return Ok(true),
                Err(error) => {
                    sheet.said = Some(failed(error));
                    sheet.reopen_list(&Live { running: self.running });
                    return Ok(false);
                }
            }
        }
        match effect {
            Effect::Apply { edits, said, select } => {
                let added = edits.iter().any(|e| matches!(e, Edit::Add { .. }));
                match self.running.apply(&edits) {
                    Ok(()) => {
                        self.reload(sheet, select);
                        if added {
                            sheet.added();
                            sheet.pane = sheet::Pane::List;
                        }
                        sheet.said = Some(Said::Done(match said {
                            Done::Text(text) => text,
                            Done::Contacts => {
                                let made: Vec<&str> = sheet.team.config.contacts();
                                t!("Interlocuteurs : {}.", "Contacts: {}.", sheet::quoted(&made))
                            }
                        }));
                    }
                    Err(error) => sheet.said = Some(failed(error)),
                }
            }
            Effect::Restart { member, fresh, said } => {
                sheet.said = Some(self.running.restart(&member, fresh).map_or_else(failed, |()| Said::Done(said)));
                self.reload(sheet, None);
            }
            Effect::RestartAll => {
                let done = t!("L'équipe a été réinitialisée.", "The team was reset.");
                sheet.said = Some(self.running.restart_all().map_or_else(failed, |()| Said::Done(done)));
                self.reload(sheet, None);
            }
            Effect::Dismiss(name) => {
                let done = t!("Panneau de « {} » fermé.", "Pane of \"{}\" closed.", name);
                sheet.said = Some(self.running.dismiss(&name).map_or_else(failed, |()| Said::Done(done)));
                self.reload(sheet, None);
            }
            Effect::Editor(edited) => {
                let text = sheet.to_edit(&edited);
                let state = self.running.state.clone();
                let result = self.session.lend(|| edit_text(&state, &text, &editor()))?;
                self.painter.forget();
                let effects = sheet.edited(edited, result.map_err(|error| format!("{error:#}")));
                return self.carry_all(sheet, effects);
            }
            Effect::Compose { id, request } => self.compose(sheet, id, request),
            Effect::CancelCompose(id) => {
                if let Some(pid) = self.composers.cancel(id) {
                    stop(pid);
                }
            }
            Effect::Detach => {
                let Some(client) = self.client else { return Ok(false) };
                match self.running.detach(client) {
                    Ok(()) => return Ok(true),
                    Err(error) => sheet.said = Some(failed(error)),
                }
            }
            Effect::Stop => match self.running.stop() {
                Ok(()) => return Ok(true),
                Err(error) => sheet.said = Some(failed(error)),
            },
            Effect::Quit => return Ok(true),
            Effect::Resume(member) => self.resume(member),
        }
        Ok(false)
    }

    /// Asks `member`'s mod to send « continue », then waits on the side for what it did (its tick comes every 2 s).
    fn resume(&mut self, member: String) {
        let state = self.running.state.clone();
        let send = self.news.0.clone();
        std::thread::spawn(move || {
            let asked = crate::sys::now();
            let how = match bridge::request_resume(&state, &member) {
                Err(error) => sheet::Resuming::Failed(format!("{error:#}")),
                Ok(()) => {
                    let until = Instant::now() + RESUME_BOUND;
                    loop {
                        if let Some(done) = bridge::resumption(&state, &member).filter(|done| done.at >= asked) {
                            break match done {
                                bridge::Resumption { error: Some(error), .. } => sheet::Resuming::Failed(error),
                                bridge::Resumption { sent: true, .. } => sheet::Resuming::Sent,
                                bridge::Resumption { draft: true, .. } => sheet::Resuming::Draft,
                                _ => sheet::Resuming::Failed(t!(
                                    "sa session n'a pas dit pourquoi",
                                    "its session did not say why"
                                )),
                            };
                        }
                        if Instant::now() >= until {
                            break sheet::Resuming::Failed(t!(
                                "pas de réponse de sa session",
                                "no answer from its session"
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
            };
            let _ = send.send(News::Resumed(member, how));
        });
    }

    /// The team read again after a change, the same member chosen (or `select`).
    fn reload(&mut self, sheet: &mut Sheet, select: Option<String>) {
        sheet.reload(load(self.running, self.client.is_some()), select);
    }

    /// Claude composes the new agent of request `id` on the side; its process noted, to stop it.
    fn compose(&mut self, sheet: &Sheet, id: u64, request: String) {
        self.composers.asked(id);
        let s = self.running.snapshot.clone();
        let team = sheet.team.config.clone();
        let send = self.news.0.clone();
        std::thread::spawn(move || {
            let templates = Templates::for_lang(s.lang);
            let started = send.clone();
            let mut tell = |pid: u32| {
                let _ = started.send(News::Started(id, pid));
            };
            let mut claude = claude::command(&s.claude, s.config_dir.as_deref());
            claude.current_dir(&s.dir);
            let result = compose::compose_member(claude, &s.team, &team, &request, &templates, &mut tell)
                .map_err(|error| format!("{error:#}"));
            let _ = send.send(News::Composed(id, result));
        });
    }

    /// What Claude composing tells: its process, stopped at once if its request was given up; what it composed, for
    /// the sheet (none when the menu closes).
    pub(super) fn heard(&mut self, news: News, sheet: Option<&mut Sheet>) {
        match news {
            News::Started(id, pid) => {
                if let Some(pid) = self.composers.started(id, pid) {
                    stop(pid);
                }
            }
            News::Composed(id, result) => {
                self.composers.done(id);
                if let Some(sheet) = sheet {
                    sheet.composed(id, result);
                }
            }
            News::Resumed(member, how) => {
                if let Some(sheet) = sheet {
                    sheet.resumed(&member, how);
                }
            }
        }
    }

    /// The menu closes: every Claude still composing is stopped, those about to start too (waited for a little).
    pub(super) fn close(&mut self) {
        if self.told.is_some() {
            let _ = self.session.write(MOVES_OFF.as_bytes());
        }
        let until = Instant::now() + Duration::from_millis(500);
        loop {
            while let Ok(news) = self.news.1.try_recv() {
                self.heard(news, None);
            }
            if !self.composers.starting() || Instant::now() >= until {
                break;
            }
            if let Ok(news) = self.news.1.recv_timeout(Duration::from_millis(50)) {
                self.heard(news, None);
            }
        }
        for pid in self.composers.all() {
            stop(pid);
        }
    }
}

/// Stops the process Claude composes in.
fn stop(pid: u32) {
    // SAFETY: a signal to a process this menu started.
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// Claude's processes composing new agents, by request: asked, started, given up. A request given up before its
/// process is known has it stopped as soon as it is.
#[derive(Default)]
pub(super) struct Composers {
    asked: HashSet<u64>,
    running: HashMap<u64, u32>,
    given_up: HashSet<u64>,
}

impl Composers {
    fn asked(&mut self, id: u64) {
        self.asked.insert(id);
    }

    /// Request `id`'s process started: the process to stop at once when the request was given up.
    fn started(&mut self, id: u64, pid: u32) -> Option<u32> {
        self.asked.remove(&id);
        if self.given_up.remove(&id) {
            return Some(pid);
        }
        self.running.insert(id, pid);
        None
    }

    /// Request `id` done, composed or failed.
    fn done(&mut self, id: u64) {
        self.asked.remove(&id);
        self.running.remove(&id);
        self.given_up.remove(&id);
    }

    /// Request `id` given up: its process to stop; not known yet, it is stopped as it starts.
    fn cancel(&mut self, id: u64) -> Option<u32> {
        if let Some(pid) = self.running.remove(&id) {
            return Some(pid);
        }
        if self.asked.contains(&id) {
            self.given_up.insert(id);
        }
        None
    }

    /// Whether a request asked has no process known yet.
    fn starting(&self) -> bool {
        !self.asked.is_empty()
    }

    /// Every process still composing, all given up.
    fn all(&mut self) -> Vec<u32> {
        self.asked.clear();
        self.given_up.clear();
        self.running.drain().map(|(_, pid)| pid).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_given_up_before_it_starts_is_stopped_as_it_starts() {
        let mut composers = Composers::default();
        composers.asked(1);
        assert_eq!(composers.cancel(1), None, "no process known yet");
        assert_eq!(composers.started(1, 4242), Some(4242), "stopped as soon as it is known");
        assert!(composers.all().is_empty(), "and not kept");
        // Known before it is given up: stopped at once.
        composers.asked(2);
        assert_eq!(composers.started(2, 7), None);
        assert_eq!(composers.cancel(2), Some(7));
        // Done: nothing left to stop.
        composers.asked(3);
        composers.started(3, 8);
        composers.done(3);
        assert_eq!(composers.cancel(3), None);
        assert!(!composers.starting());
        // The menu closing: those running stopped, those asked waited for.
        composers.asked(4);
        assert!(composers.starting());
        assert_eq!(composers.started(4, 9), None);
        assert_eq!(composers.all(), [9]);
    }
}
