// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! The menu over the team, and the list under a header's text.

use std::sync::PoisonError;

use anyhow::{Result, bail};

use super::Server;
use super::frame::glyphs;
use super::lifecycle::log;
use super::pane::Role;
use crate::mux::chrome;
use crate::mux::proto::{MenuField, MenuOpened, PaneSpec};
use crate::t;
use crate::ui::Rect;
use crate::ui::look::Glyphs;

/// The menu in its list mode: the values of a member's model or effort, in a pane
/// floating under the text of the header clicked, nothing dimmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct List {
    /// The member's pane whose header was clicked.
    pub(super) pane: String,
    /// The part of its header: the model or the effort. Kept in inverse video while the list is open.
    pub(super) part: chrome::Part,
    /// Where that text is on the screen.
    pub(super) anchor: Rect,
    /// The list's size, its own frame included (it draws it): estimated at first, then as the menu tells it (`CSI 8 ;
    /// rows ; cols t`).
    pub(super) size: (usize, usize),
}

/// A list's size before the menu tells its own: six values and the frame, columns and rows.
pub(super) const LIST_SIZE: (usize, usize) = (26, 8);

/// Where a list goes on a screen of `screen` columns and rows (the bar left out), under the text at `anchor`, its
/// values aligned on it (they start four columns inside the frame: the frame goes four columns to the left):
/// moved left just enough to end at the screen's edge; above the text when there is no room below; never
/// out of the screen.
pub(super) fn list_area(anchor: Rect, size: (usize, usize), screen: (usize, usize)) -> Rect {
    let width = size.0.min(screen.0).max(1);
    let x = anchor.x.saturating_sub(4).min(screen.0.saturating_sub(width));
    let below = screen.1.saturating_sub(anchor.y + 1);
    let above = anchor.y.min(screen.1);
    let (y, height) = if size.1 <= below {
        (anchor.y + 1, size.1)
    } else if size.1 <= above {
        (anchor.y - size.1, size.1)
    } else if below >= above {
        (anchor.y + 1, below.max(1))
    } else {
        (anchor.y - above, above.max(1))
    };
    let height = height.min(screen.1).max(1);
    Rect { x, y: y.min(screen.1.saturating_sub(height)), width, height }
}

impl Server {
    /// The list under a header's text closed, the focus back where it was.
    pub(super) fn close_list(&mut self) {
        if self.list.is_some()
            && let Some(menu) = self.menu.as_ref().map(|(menu, _)| menu.clone())
        {
            self.close_pane(&menu);
        }
        self.list = None;
        self.dirty = true;
    }

    /// The list asks for `rows` × `cols`, its own frame included (`CSI 8 ; rows ; cols t`): its place changes with it,
    /// within the screen.
    pub(super) fn list_resize(&mut self, rows: u16, cols: u16) {
        let Some(list) = self.list.as_mut() else { return };
        let size = (usize::from(cols), usize::from(rows));
        if list.size != size {
            list.size = size;
            self.layout();
        }
    }

    /// The team's menu (`recruit _menu`) in a pane floating over the team, on the client: `client`, else the one there
    /// is. A menu already open stays: one at a time.
    pub(super) fn open_menu(
        &mut self,
        client: Option<&str>,
        sheet: Option<(&str, Option<MenuField>)>,
    ) -> Result<MenuOpened> {
        self.open_menu_as(client, sheet, None)
    }

    /// [`Server::open_menu`], as a `list` under a header's text if given: `recruit _menu … --list`.
    pub(super) fn open_menu_as(
        &mut self,
        client: Option<&str>,
        sheet: Option<(&str, Option<MenuField>)>,
        list: Option<List>,
    ) -> Result<MenuOpened> {
        let Some(attached) = self.client.as_ref().filter(|c| client.is_none_or(|id| c.id == id)) else {
            return Ok(MenuOpened::NoClient);
        };
        if self.menu.is_some() {
            return Ok(MenuOpened::AlreadyOpen);
        }
        let Some(exe) = self.exe.clone() else {
            bail!(t!("le menu n'est pas disponible pour cette équipe", "the menu is not available for this team"));
        };
        let mut argv = vec![exe, "--lang".into(), self.lang.clone(), "_menu".into()];
        argv.push(self.state.to_string_lossy().into_owned());
        argv.extend(["--client".into(), attached.id.clone()]);
        if glyphs(attached) == Glyphs::Nerd {
            argv.push("--nerd".into());
        }
        if self.light {
            argv.push("--light".into());
        }
        if let Some((member, field)) = sheet {
            argv.extend(["--member".into(), member.to_string()]);
            if let Some(field) = field.and_then(MenuField::arg) {
                argv.extend(["--field".into(), field.into()]);
            }
        }
        if list.is_some() {
            argv.push("--list".into());
        }
        let cwd = self.welcome.lock().unwrap_or_else(PoisonError::into_inner).dir.clone();
        let cwd = if cwd.is_empty() { "/".to_string() } else { cwd };
        let spec = PaneSpec { member: "recruit".into(), role: Role::Menu.name().into(), argv, env: Vec::new(), cwd };
        let id = self.spawn(spec)?;
        let before = self.focus.clone();
        self.menu = Some((id.clone(), before));
        self.list = list;
        self.focus_on(&id);
        Ok(MenuOpened::Opened)
    }

    /// The menu's place: 100 × 28 inside its frame, centered; less in a smaller screen.
    pub(super) fn menu_area(&self) -> Rect {
        if let Some(list) = &self.list {
            return list_area(list.anchor, list.size, self.tab_room());
        }
        let (width, height) = self.tab_room();
        let fit = |wanted: usize, room: usize| wanted.min(room.saturating_sub(2)).max(1).min(room);
        let (w, h) = (fit(102, width), fit(30, height));
        Rect { x: (width - w) / 2, y: (height - h) / 2, width: w, height: h }
    }

    /// The menu, on the client: on the sheet of the active pane's member (⌥r and the bar's button), else on the
    /// menu's first sheet (the dashboard or the journal active).
    pub(super) fn menu_here(&mut self) {
        let member = self.focused_member().map(str::to_string);
        if let Err(error) = self.open_menu(None, member.as_deref().map(|member| (member, None))) {
            log!("menu: {error:#}");
        }
    }
}
