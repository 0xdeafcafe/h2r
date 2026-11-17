//! The menu bar icon: a monochrome template image (black on clear; macOS
//! tints it for light and dark menu bars). The app icon's "2" glyph
//! (art/app-icon, through the `app-icon` crate), and on it the state: the
//! bare glyph when running, dimmed when stopped, a ring in the corner while
//! starting, a slash through it on error. Each mark is cut clear of the
//! glyph so it still reads at 18 pt.

use crate::menu::Overall;

/// Pixel size: 18 pt menu bar icons at 2x (the glyph's @2x export).
pub const SIZE: u32 = 36;

/// How much of the glyph a stopped server leaves.
const STOPPED_ALPHA: f32 = 0.35;

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

/// A state's mark and the clear margin cut around it, as (mark, cut).
type Mark = (Box<dyn Fn(f32, f32) -> bool>, Box<dyn Fn(f32, f32) -> bool>);

fn mark(state: Overall) -> Option<Mark> {
    let s = SIZE as f32;
    match state {
        Overall::Running | Overall::Stopped => None,
        // A ring in the bottom-right corner.
        Overall::Starting => {
            let (cx, cy) = (s * 0.8, s * 0.8);
            let r = move |x: f32, y: f32| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            let (outer, inner, cut) = (s * 0.17, s * 0.09, s * 0.24);
            Some((
                Box::new(move |x, y| (inner..=outer).contains(&r(x, y))),
                Box::new(move |x, y| r(x, y) <= cut),
            ))
        }
        // A bar from top-left to bottom-right.
        Overall::Error => {
            let d = move |x: f32, y: f32| (x - y).abs() / std::f32::consts::SQRT_2;
            let (inset, half, cut) = (s * 0.08, s * 0.05, s * 0.11);
            let along = move |x: f32, y: f32| (inset..=s - inset).contains(&((x + y) / 2.0));
            Some((
                Box::new(move |x, y| d(x, y) <= half && along(x, y)),
                Box::new(move |x, y| d(x, y) <= cut && along(x, y)),
            ))
        }
    }
}

/// RGBA, `SIZE` x `SIZE`, for `state`.
pub fn rgba(state: Overall) -> Vec<u8> {
    let glyph = app_icon::status_template();
    assert_eq!(
        (glyph.width, glyph.height),
        (SIZE, SIZE),
        "the status glyph is 18 pt at 2x"
    );
    let dim = if state == Overall::Stopped {
        STOPPED_ALPHA
    } else {
        1.0
    };
    let mark = mark(state);
    let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let g = glyph.pixels[((y * SIZE + x) * 4 + 3) as usize] as f32 / 255.0 * dim;
            let a = match &mark {
                None => g,
                Some((ink, cut)) => (g * (1.0 - coverage(x, y, cut))).max(coverage(x, y, ink)),
            };
            out.extend_from_slice(&[0, 0, 0, (a * 255.0).round() as u8]);
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
        // Running is the glyph as drawn.
        let glyph = app_icon::status_template();
        assert!(
            running
                .chunks(4)
                .zip(glyph.pixels.chunks(4))
                .all(|(a, b)| a[3] == b[3])
        );
        // Stopped: the same shape, fainter, nothing solid left.
        assert_eq!(ink(stopped), 0);
        assert!(stopped.chunks(4).filter(|p| p[3] > 0).count() > 0);
        // Starting: the ring in the corner, hollow in the middle.
        let c = (SIZE as f32 * 0.8) as u32;
        assert_eq!(alpha(starting, c, c), 0, "a hollow ring");
        assert_eq!(alpha(starting, c + 5, c), 255, "the ring itself");
        // Error: the slash crosses the middle, and is cut clear of the glyph.
        let m = SIZE / 2;
        assert_eq!(alpha(error, m, m), 255);
        assert!(ink(error) != ink(running));
        // Clear corners.
        assert_eq!(alpha(running, 0, 0), 0);
        assert_eq!(alpha(error, SIZE - 1, SIZE - 1), 0);
        assert_eq!(alpha(error, 0, 0), 0);
    }
}
