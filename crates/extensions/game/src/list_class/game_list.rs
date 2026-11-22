//! The game's list widget as a model: item widgets, rows, focus, scroll and
//! arrows, as the retail list code moves them (docs/formats/ui.md "Item
//! widgets, scrolling and spacing", docs/game/ui-builder.md "Item
//! count"). Its tests hold every list of ours to it: what the game is
//! given, it places; the focus never leaves the rows; nothing scrolls
//! when every row fits.

use super::{EMBEDDED_ITEMS, item_widgets};

/// A list as the game holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameList {
    /// Item widgets linked to the handler (`0x24c177` counts them).
    pub items: usize,
    /// Rows the list block lays out (its visible count, `+6`): children
    /// past these are never placed (`0x24bc12`).
    pub laid_out: usize,
    /// Rows in the handler's datum array (its active count, `+0x3c`).
    pub rows: usize,
    /// The list block's flag bit 0 (`0x24bc12` copies it to `+0x7c`): the
    /// focus wraps at either end (`0x24c1c5`).
    pub wraps: bool,
    /// The row the first item widget shows.
    pub first: usize,
    /// The focused item widget.
    pub focus: usize,
}

/// A direction the list handler takes (`0x24c024`: 1 up, 3 down).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
}

impl GameList {
    /// A Controls list as retail links it: all six embedded items whatever
    /// the tag lays out (slot 19, `0x2b198a`), plus `extra` of ours.
    pub fn retail(laid_out: usize, rows: usize, extra: usize, wraps: bool) -> GameList {
        GameList::new(EMBEDDED_ITEMS as usize + extra, laid_out, rows, wraps)
    }

    /// A list of ours: our slot 19 ([`item_widgets`]) and `extra` of ours.
    pub fn ours(laid_out: usize, rows: usize, extra: usize, wraps: bool) -> GameList {
        let laid = i16::try_from(laid_out).unwrap_or(i16::MAX);
        let embedded = item_widgets(laid, extra as u32) as usize;
        GameList::new(embedded + extra, laid_out, rows, wraps)
    }

    fn new(items: usize, laid_out: usize, rows: usize, wraps: bool) -> GameList {
        GameList {
            items,
            laid_out,
            rows,
            wraps,
            first: 0,
            focus: 0,
        }
    }

    /// The row item widget `i` shows (`0x24c0c4` hands rows to the
    /// children in order, none past the last row).
    pub fn row_of(&self, i: usize) -> Option<usize> {
        let r = self.first + i;
        (i < self.items && r < self.rows).then_some(r)
    }

    /// The focused row: what A presses (`0x24c024` reads the focused
    /// child's row for the press record).
    pub fn pressed(&self) -> Option<usize> {
        self.row_of(self.focus)
    }

    /// Whether item widget `i` is placed (one of the first `laid_out`).
    pub fn placed(&self, i: usize) -> bool {
        i < self.laid_out
    }

    /// Rows on item widgets that are never placed: the invisible rows the
    /// focus can still reach.
    pub fn phantoms(&self) -> usize {
        (self.laid_out..self.items)
            .filter(|&i| self.row_of(i).is_some())
            .count()
    }

    /// More rows than item widgets: the list scrolls (`0x24c515`).
    pub fn scrolls(&self) -> bool {
        self.rows > self.items
    }

    /// The arrows the draw shows (`0x24bda2`): up while the first child's
    /// row has one before it (`0x24c63e`), down while the last child's
    /// row has one after it (`0x24c676`).
    pub fn arrows(&self) -> (bool, bool) {
        let up = self.items > 0 && self.first > 0;
        let down = self.items > 0 && self.first + self.items < self.rows;
        (up, down)
    }

    /// One step of the focus (`0x24c1c5`). Down: onto the next child when
    /// it has a row (`0x24ac76`); at a child whose next has none, to the
    /// first when the list wraps, else nowhere; at the last child, the
    /// list scrolls one row while a row follows, else wraps to the top
    /// (`0x24c0c4`) or stops. Up the same way back.
    pub fn step(&mut self, s: Step) {
        if self.items == 0 || self.pressed().is_none() {
            return;
        }
        match s {
            Step::Down => {
                if self.focus + 1 < self.items {
                    if self.row_of(self.focus + 1).is_some() {
                        self.focus += 1;
                    } else if self.wraps {
                        self.first = 0;
                        self.focus = 0;
                    }
                } else if self.first + self.focus + 1 < self.rows {
                    self.first += 1;
                } else if self.wraps {
                    self.first = 0;
                    self.focus = 0;
                }
            }
            Step::Up => {
                if self.focus > 0 {
                    self.focus -= 1;
                } else if self.first > 0 {
                    self.first -= 1;
                } else if self.wraps {
                    let shown = self.rows.min(self.items);
                    self.first = self.rows - shown;
                    self.focus = shown - 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Retail's Controls slot 19 on a list laid out shorter than six: on
    /// the in-game Controller Settings tag (five laid out) a six-row list
    /// had a sixth row on an item widget never placed, which Down reached
    /// and A pressed, and a seventh scrolled in past it.
    #[test]
    fn a_list_laid_out_shorter_than_its_six_items_had_a_phantom_row() {
        let mut l = GameList::retail(5, 7, 0, false);
        assert_eq!(l.phantoms(), 1);
        for _ in 0..5 {
            l.step(Step::Down);
        }
        assert_eq!(l.focus, 5);
        assert!(!l.placed(l.focus), "focus on an item widget never placed");
        assert_eq!(l.pressed(), Some(5), "A presses a row nobody sees");
        l.step(Step::Down);
        assert_eq!((l.first, l.pressed()), (1, Some(6)), "and it scrolls");
    }

    #[test]
    fn our_item_count_is_the_laid_out_count() {
        for laid in 0..=16usize {
            let extra = laid.saturating_sub(EMBEDDED_ITEMS as usize);
            for rows in 0..=20 {
                let l = GameList::ours(laid, rows, extra, false);
                assert_eq!(l.items, laid, "laid out {laid}");
                assert_eq!(l.phantoms(), 0, "laid out {laid}, {rows} rows");
                assert_eq!(l.scrolls(), rows > laid);
            }
        }
        assert_eq!(item_widgets(5, 0), 5);
        assert_eq!(item_widgets(6, 0), 6);
        assert_eq!(item_widgets(13, 7), 6);
        assert_eq!(item_widgets(-1, 0), 0);
    }

    /// Down and Up from every row of lists that fit and lists that scroll:
    /// the focus stays on placed item widgets and real rows, Down on the
    /// last row stops (or wraps to the first when the list wraps), and a
    /// list whose rows all fit never scrolls or shows an arrow.
    #[test]
    fn the_focus_never_leaves_the_rows() {
        for wraps in [false, true] {
            for laid in 1..=8usize {
                for rows in 1..=12usize {
                    let extra = laid.saturating_sub(EMBEDDED_ITEMS as usize);
                    let mut l = GameList::ours(laid, rows, extra, wraps);
                    for s in std::iter::repeat_n(Step::Down, 2 * rows + 2)
                        .chain(std::iter::repeat_n(Step::Up, 2 * rows + 2))
                    {
                        l.step(s);
                        assert!(l.placed(l.focus), "{l:?}");
                        let row = l.pressed().expect("a row");
                        assert!(row < rows, "{l:?}");
                        if rows <= laid {
                            assert_eq!(l.first, 0, "{l:?}");
                            assert_eq!(l.arrows(), (false, false), "{l:?}");
                        }
                    }
                    let mut at_end = GameList::ours(laid, rows, extra, wraps);
                    for _ in 0..rows - 1 {
                        at_end.step(Step::Down);
                    }
                    assert_eq!(at_end.pressed(), Some(rows - 1));
                    at_end.step(Step::Down);
                    let want = if wraps { 0 } else { rows - 1 };
                    assert_eq!(at_end.pressed(), Some(want), "{at_end:?}");
                }
            }
        }
    }
}
