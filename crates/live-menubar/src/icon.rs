//! The menu bar icon, drawn here: a monochrome template image (black on
//! clear; macOS tints it for light and dark menu bars). A ring, and in it
//! the state: a solid dot when running, a ring with a gap while starting,
//! the bare ring when stopped, a slash through it on error.

use crate::menu::Overall;

/// Pixel size: 18 pt menu bar icons at 2x.
pub const SIZE: u32 = 36;

/// Coverage of one pixel in 0..=1, from 4x4 samples.
fn coverage(x: u32, y: u32, inside: &dyn Fn(f32, f32) -> bool) -> f32 {
    let mut hits = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let px = x as f32 + (sx as f32 + 0.5) / 4.0;
            let py = y as f32 + (sy as f32 + 0.5) / 4.0;
            if inside(px, py) {
                hits += 1;
            }
        }
    }
    hits as f32 / 16.0
}

/// RGBA, `SIZE` x `SIZE`, for `state`.
pub fn rgba(state: Overall) -> Vec<u8> {
    let s = SIZE as f32;
    let c = s / 2.0;
    let outer = s * 0.44;
    let inner = outer - s * 0.1;
    let shape = move |x: f32, y: f32| -> bool {
        let (dx, dy) = (x - c, y - c);
        let r = (dx * dx + dy * dy).sqrt();
        let ring = r <= outer && r >= inner;
        match state {
            Overall::Running => ring || r <= s * 0.18,
            Overall::Stopped => ring,
            // A gap in the top-right quarter.
            Overall::Starting => ring && !(dx > 0.0 && dy < 0.0),
            Overall::Error => {
                // A bar from top-left to bottom-right, across the ring.
                let d = (dx - dy).abs() / std::f32::consts::SQRT_2;
                ring || (d <= s * 0.06 && r <= outer)
            }
        }
    };
    let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let a = (coverage(x, y, &shape) * 255.0).round() as u8;
            out.extend_from_slice(&[0, 0, 0, a]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(px: &[u8]) -> usize {
        px.chunks(4).filter(|p| p[3] > 127).count()
    }

    fn alpha(px: &[u8], x: u32, y: u32) -> u8 {
        px[((y * SIZE + x) * 4 + 3) as usize]
    }

    #[test]
    fn states_draw_differently() {
        let all = [
            Overall::Running,
            Overall::Starting,
            Overall::Stopped,
            Overall::Error,
        ]
        .map(rgba);
        for (i, a) in all.iter().enumerate() {
            assert_eq!(a.len(), (SIZE * SIZE * 4) as usize);
            assert!(
                a.chunks(4).all(|p| p[..3] == [0, 0, 0]),
                "template: black only"
            );
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        let [running, starting, stopped, error] = &all;
        let c = SIZE / 2;
        assert_eq!(alpha(running, c, c), 255, "a dot in the middle");
        assert_eq!(alpha(stopped, c, c), 0, "an empty ring");
        assert_eq!(alpha(error, c, c), 255, "the slash crosses the middle");
        assert!(ink(starting) < ink(stopped), "a gap in the ring");
        assert!(ink(running) > ink(stopped));
        // Clear corners.
        assert_eq!(alpha(running, 0, 0), 0);
        assert_eq!(alpha(error, SIZE - 1, SIZE - 1), 0);
    }
}
