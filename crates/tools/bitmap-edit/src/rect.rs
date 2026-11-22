//! Pixel rectangles: what an edit touched, what a tool is limited to.

use serde::{Deserialize, Serialize};

/// A rectangle of pixels, top left and size. An empty one (`w` or `h`
/// zero) is "nothing", the identity of [`Rect::union`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Rect { x, y, w, h }
    }

    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// One past the last column.
    pub const fn right(&self) -> u32 {
        self.x.saturating_add(self.w)
    }

    /// One past the last row.
    pub const fn bottom(&self) -> u32 {
        self.y.saturating_add(self.h)
    }

    pub const fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    /// Whether `other` lies wholly inside this rectangle.
    pub const fn encloses(&self, other: Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    /// The smallest rectangle holding both; an empty one adds nothing.
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let r = self.right().max(other.right());
        let b = self.bottom().max(other.bottom());
        Rect::new(x, y, r - x, b - y)
    }

    /// The overlap, or the empty rectangle.
    pub fn intersect(self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        if r <= x || b <= y {
            return Rect::default();
        }
        Rect::new(x, y, r - x, b - y)
    }
}
