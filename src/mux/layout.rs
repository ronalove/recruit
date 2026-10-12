// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! How a tab is cut into a grid of panes, with the dashboard and the journal on the side, or into the places of the
//! « focus » layout (`focus`): rectangles of the screen, from the size of the tab and its panes.

use crate::ui::Rect;

/// The grid of a tab of `count` panes with at most `max_columns` columns: as few rows as possible, then as few
/// columns as these rows need (4 panes make 2 × 2, 5 make 3 + 2). Returns the number of panes in each column;
/// pane `i` goes in column `i % columns`, reading order.
fn column_heights(count: usize, max_columns: usize) -> Vec<usize> {
    let rows = count.div_ceil(max_columns.max(1)).max(1);
    let cols = count.div_ceil(rows).max(1);
    let mut heights = vec![0; cols];
    for i in 0..count {
        heights[i % cols] += 1;
    }
    heights
}

/// The panels' column of the first tab, by pane ids: the dashboard, and the journal under it with its height when
/// it is reduced (half the column otherwise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidePanes {
    pub(crate) dashboard: Option<String>,
    pub(crate) journal: Option<(String, Option<usize>)>,
}

/// Width of the panels' column, as a share of the window.
const SIDE_PERCENT: usize = 35;

/// Where each pane of a tab goes in the multiplexer, frame included: a grid of at most `columns` columns, the frames
/// side by side with no line between them, in a `width` × `height` area from its
/// top left corner. Returns the panes (ids, as given) with their rectangles.
pub(crate) fn rects(
    width: usize,
    height: usize,
    panes: &[String],
    columns: usize,
    side: Option<&SidePanes>,
) -> Vec<(String, Rect)> {
    let mut out = Vec::new();
    tree(width, height, panes, columns, side).place(Rect { x: 0, y: 0, width, height }, &mut out);
    out
}

fn tree(width: usize, height: usize, panes: &[String], columns: usize, side: Option<&SidePanes>) -> Node {
    let side_panes: Vec<(String, Option<usize>)> = side
        .map(|s| {
            s.dashboard
                .iter()
                .map(|d| (d.clone(), None))
                .chain(s.journal.iter().map(|(j, r)| (j.clone(), *r)))
                .collect()
        })
        .unwrap_or_default();
    let columns = if side_panes.is_empty() { columns } else { 1 };
    let heights = column_heights(panes.len(), columns);
    let cols = heights.len();
    let grid: Vec<Vec<String>> =
        (0..cols).map(|c| (0..heights[c]).map(|r| panes[r * cols + c].clone()).collect()).collect();
    let grid_node = |w: usize, h: usize| -> Node {
        let column = |members: &[String]| -> Node {
            let sizes = shares(h, members.len());
            Node::split(false, members.iter().zip(sizes).map(|(m, s)| (s, Node::Pane(m.clone()))).collect())
        };
        let sizes = shares(w, cols);
        Node::split(true, grid.iter().zip(sizes).map(|(members, s)| (s, column(members))).collect())
    };
    if side_panes.is_empty() {
        return grid_node(width, height);
    }
    let side_width = width * SIDE_PERCENT / 100;
    let left = width.saturating_sub(side_width);
    let column = match side_panes.as_slice() {
        [(dashboard, _), (journal, rows)] => {
            let journal_rows = rows.unwrap_or(height / 2).min(height.saturating_sub(2));
            let dashboard_rows = height.saturating_sub(journal_rows);
            Node::split(
                false,
                vec![(dashboard_rows, Node::Pane(dashboard.clone())), (journal_rows, Node::Pane(journal.clone()))],
            )
        }
        [(only, _)] => Node::Pane(only.clone()),
        _ => unreachable!("one or two panels"),
    };
    Node::split(true, vec![(left, grid_node(left, height)), (side_width, column)])
}

/// Narrower than this, the cards of a tab in « focus » go two by two, in their short form.
pub(crate) const FOCUS_WIDE: usize = 120;
/// Shorter than this, a tab in « focus » shows two panes on the right instead of three.
const FOCUS_TALL: usize = 36;
/// Fewest columns of a tab in « focus »; narrower, it keeps its grid.
const FOCUS_MIN_WIDTH: usize = 80;
/// Fewest rows of a tab in « focus » (two panes of three rows and the cards' frame); shorter, it keeps its grid.
const FOCUS_MIN_HEIGHT: usize = 14;
/// The fewest rows of a pane on the right.
const FOCUS_PANE_ROWS: usize = 3;

/// How many panes the right column of a tab in « focus » shows in a `width` × `height` area: three, two under
/// 120 × 36; None under 80 columns, where the tab keeps its grid (and under 14 rows, where the right column has no
/// room for two panes and the cards).
pub(crate) fn focus_room(width: usize, height: usize) -> Option<usize> {
    if width < FOCUS_MIN_WIDTH || height < FOCUS_MIN_HEIGHT {
        None
    } else if width < FOCUS_WIDE || height < FOCUS_TALL {
        Some(2)
    } else {
        Some(3)
    }
}

/// The rows the frame of `cards` cards takes in a `width` columns area: a card is 4 rows, with its line of text and
/// the gap under it; two by two, in the short form, 3 rows a row of cards; the frame's two borders.
pub(crate) fn focus_cards_rows(width: usize, cards: usize) -> usize {
    match cards {
        0 => 0,
        n if width < FOCUS_WIDE => 1 + 3 * n.div_ceil(2),
        n => 1 + 4 * n,
    }
}

/// Where the panes of a tab in « focus » go, frames included, in a `width` × `height` area from its top left corner:
/// `left` on the left, as the contacts beside the panels; `right` stacked on the right, top to bottom; under them, when
/// there are `cards` cards of the members left out, the area of their frame ([`focus_cards_rows`] rows, fewer when
/// the panes need the room), returned apart. The widths follow `SIDE_PERCENT`. With nothing on the right, the cards'
/// frame takes the whole column; with no cards either, the left pane takes the whole width.
pub(crate) fn focus_rects(
    width: usize,
    height: usize,
    left: &str,
    right: &[String],
    cards: usize,
) -> (Vec<(String, Rect)>, Option<Rect>) {
    let side = if right.is_empty() && cards == 0 { 0 } else { width * SIDE_PERCENT / 100 };
    let main = width - side;
    let mut out = vec![(left.to_string(), Rect { x: 0, y: 0, width: main, height })];
    if side == 0 {
        return (out, None);
    }
    let reserved = FOCUS_PANE_ROWS * right.len();
    let frame =
        if right.is_empty() { height } else { focus_cards_rows(width, cards).min(height.saturating_sub(reserved)) };
    let frame = if cards > 0 && frame >= 3 { frame } else { 0 };
    let panes = height - frame;
    if !right.is_empty() {
        let mut y = 0;
        for (id, rows) in right.iter().zip(shares(panes, right.len())) {
            out.push((id.clone(), Rect { x: main, y, width: side, height: rows }));
            y += rows;
        }
    }
    let cards = (frame > 0).then_some(Rect { x: main, y: panes, width: side, height: frame });
    (out, cards)
}

/// `count` cells sharing `total` columns or rows, evenly, the first ones rounded down.
fn shares(total: usize, count: usize) -> Vec<usize> {
    let count = count.max(1);
    (0..count).map(|i| (i + 1) * total / count - i * total / count).collect()
}

/// A cell of a tab's layout.
enum Node {
    Pane(String),
    /// Side by side when `across`, stacked otherwise; each child with its width or height.
    Split {
        across: bool,
        children: Vec<(usize, Node)>,
    },
}

impl Node {
    fn split(across: bool, mut children: Vec<(usize, Node)>) -> Node {
        if children.len() == 1 { children.remove(0).1 } else { Node::Split { across, children } }
    }

    /// The panes' rectangles in `area`.
    fn place(&self, area: Rect, out: &mut Vec<(String, Rect)>) {
        match self {
            Node::Pane(id) => out.push((id.clone(), area)),
            Node::Split { across, children } => {
                let mut at = if *across { area.x } else { area.y };
                for (size, child) in children {
                    let cell = if *across {
                        Rect { x: at, width: *size, ..area }
                    } else {
                        Rect { y: at, height: *size, ..area }
                    };
                    child.place(cell, out);
                    at += size;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("%{i}")).collect()
    }

    #[test]
    fn native_rects_tile_the_area() {
        // Five members, three columns at most: 3 + 2, no line between the frames.
        let rects = rects(120, 40, &ids(5), 3, None);
        let at = |id: &str| rects.iter().find(|(i, _)| i == id).unwrap().1;
        assert_eq!(at("%0"), Rect { x: 0, y: 0, width: 40, height: 20 });
        assert_eq!(at("%3"), Rect { x: 0, y: 20, width: 40, height: 20 });
        assert_eq!(at("%2"), Rect { x: 80, y: 0, width: 40, height: 40 });
        let area: usize = rects.iter().map(|(_, r)| r.width * r.height).sum();
        assert_eq!(area, 120 * 40, "every cell has its pane, once");
        // Beside the panels: the members in one column, the dashboard over the journal on the right.
        let side = SidePanes { dashboard: Some("%d".into()), journal: Some(("%j".into(), Some(7))) };
        let rects = super::rects(100, 30, &ids(2), 3, Some(&side));
        let at = |id: &str| rects.iter().find(|(i, _)| i == id).unwrap().1;
        assert_eq!(at("%0"), Rect { x: 0, y: 0, width: 65, height: 15 });
        assert_eq!(at("%d"), Rect { x: 65, y: 0, width: 35, height: 23 });
        assert_eq!(at("%j"), Rect { x: 65, y: 23, width: 35, height: 7 });
    }

    #[test]
    fn grid_shapes() {
        assert_eq!(column_heights(1, 3), [1]);
        assert_eq!(column_heights(2, 3), [1, 1]);
        assert_eq!(column_heights(3, 3), [1, 1, 1]);
        assert_eq!(column_heights(4, 3), [2, 2]);
        assert_eq!(column_heights(5, 3), [2, 2, 1]);
        assert_eq!(column_heights(6, 3), [2, 2, 2]);
        assert_eq!(column_heights(7, 3), [3, 2, 2]);
        assert_eq!(column_heights(4, 2), [2, 2]);
    }

    #[test]
    fn the_room_on_the_right_follows_the_window() {
        let table = [
            ((140, 40), Some(3)),
            ((120, 36), Some(3)),
            ((119, 40), Some(2)),
            ((140, 35), Some(2)),
            ((100, 31), Some(2)),
            ((80, 24), Some(2)),
            ((79, 40), None),
            ((140, 13), None),
        ];
        for ((width, height), room) in table {
            assert_eq!(focus_room(width, height), room, "{width} × {height}");
        }
    }

    #[test]
    fn focus_rects_tile_the_area() {
        // 140 × 39, a tall window without its bar: three panes on the right, two cards of 4 rows and the frame.
        let right = ids(3);
        let (rects, cards) = focus_rects(140, 39, "left", &right, 2);
        let at = |id: &str| rects.iter().find(|(i, _)| i == id).unwrap().1;
        assert_eq!(at("left"), Rect { x: 0, y: 0, width: 91, height: 39 });
        let frame = cards.unwrap();
        assert_eq!(frame, Rect { x: 91, y: 30, width: 49, height: 9 });
        // The panes share what the frame leaves, in order, from the top, each as wide as the column.
        let heights: Vec<_> = right.iter().map(|id| at(id)).collect();
        assert_eq!(heights.iter().map(|r| r.height).sum::<usize>(), 30);
        assert_eq!(heights[0].y, 0);
        for pair in heights.windows(2) {
            assert_eq!(pair[1].y, pair[0].y + pair[0].height);
        }
        assert!(heights.iter().all(|r| r.x == 91 && r.width == 49));
        assert_eq!(rects.len(), 4);
    }

    #[test]
    fn the_cards_go_two_by_two_in_a_narrow_window() {
        assert_eq!(focus_cards_rows(140, 2), 9);
        assert_eq!(focus_cards_rows(119, 3), 7);
        assert_eq!(focus_cards_rows(100, 2), 4);
        assert_eq!(focus_cards_rows(140, 0), 0);
        let (_, cards) = focus_rects(100, 30, "l", &ids(2), 3);
        assert_eq!(cards, Some(Rect { x: 65, y: 23, width: 35, height: 7 }));
    }

    #[test]
    fn the_panes_keep_their_rows_before_the_cards() {
        // Six cards would take 25 rows: the frame gives way to two panes of three rows at least.
        let (rects, cards) = focus_rects(140, 20, "l", &ids(2), 6);
        let frame = cards.unwrap();
        assert_eq!(frame.height, 14);
        assert!(rects.iter().skip(1).all(|(_, r)| r.height == 3));
        // No room for even that: no frame.
        let (_, cards) = focus_rects(140, 8, "l", &ids(2), 6);
        assert_eq!(cards, None);
    }

    #[test]
    fn nothing_on_the_right() {
        let (rects, cards) = focus_rects(140, 30, "l", &[], 0);
        assert_eq!(rects, [("l".to_string(), Rect { x: 0, y: 0, width: 140, height: 30 })]);
        assert_eq!(cards, None);
        // Only cards: they have the column.
        let (rects, cards) = focus_rects(140, 30, "l", &[], 2);
        assert_eq!(rects.len(), 1);
        assert_eq!(cards, Some(Rect { x: 91, y: 0, width: 49, height: 30 }));
        // No cards: the panes have the column.
        let (rects, cards) = focus_rects(140, 30, "l", &ids(2), 0);
        assert_eq!(cards, None);
        assert_eq!(rects[1].1.height + rects[2].1.height, 30);
    }
}
