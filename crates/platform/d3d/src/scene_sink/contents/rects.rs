//! The rectangles still missing from one target aspect. Work in guest pixel
//! coordinates, which both the 1x and scaled backends use for scissors.

use crate::backend::PixelRect;

#[derive(Clone)]
pub(super) struct Holes(Vec<PixelRect>);

impl Holes {
    pub fn whole(width: u32, height: u32) -> Self {
        Self(vec![PixelRect {
            x: 0,
            y: 0,
            width,
            height,
        }])
    }

    /// Subtract what the clear actually wrote. Impose a fragmentation bound
    /// on diagnostic work; dropping this proof leaves the aspect unknown.
    pub fn cover(&mut self, rect: PixelRect) -> Result<bool, ()> {
        let mut remaining = Vec::new();
        for hole in self.0.drain(..) {
            let right = hole.x + hole.width;
            let bottom = hole.y + hole.height;
            let left = rect.x.max(hole.x);
            let top = rect.y.max(hole.y);
            let x1 = rect.x.saturating_add(rect.width).min(right);
            let y1 = rect.y.saturating_add(rect.height).min(bottom);
            if left >= x1 || top >= y1 {
                remaining.push(hole);
            } else {
                let mut keep = |x, y, r, b| {
                    if x < r && y < b {
                        remaining.push(PixelRect {
                            x,
                            y,
                            width: r - x,
                            height: b - y,
                        });
                    }
                };
                keep(hole.x, hole.y, right, top);
                keep(hole.x, y1, right, bottom);
                keep(hole.x, top, left, y1);
                keep(x1, top, right, y1);
            }
            if remaining.len() > 64 {
                return Err(());
            }
        }
        self.0 = remaining;
        Ok(self.0.is_empty())
    }
}
