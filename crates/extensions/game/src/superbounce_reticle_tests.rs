use std::time::Duration;

use super::*;

fn at(base: Instant, ms: u64) -> Instant {
    base + Duration::from_millis(ms)
}

fn points(s: &Shape) -> &[[f32; 2]] {
    match s {
        Shape::Path { points, .. } => points,
        _ => panic!("the reticle is lines"),
    }
}

#[test]
fn it_shows_only_while_locked() {
    let t0 = Instant::now();
    // Not locked, never was: nothing, and nothing to draw.
    assert_eq!(Fade::next(None, false, t0), None);
    // Locked: shown, and fully faded in once the fade is done.
    let f = Fade::next(None, true, t0).expect("locked shows");
    assert!(f.level(at(t0, 200)) > 0.999);
    assert!(!shapes(RETICLE_CENTRE, [1.0, 1.0, 0.0, 0.0], f.level(at(t0, 200))).is_empty());
    // Lock lost: fades out, then is gone.
    let g = Fade::next(Some(f), false, at(t0, 300)).expect("fading out");
    assert!(g.level(at(t0, 300)) > 0.999);
    assert!(g.level(at(t0, 500)) < 0.001);
    assert_eq!(Fade::next(Some(g), false, at(t0, 500)), None);
    assert!(shapes(RETICLE_CENTRE, [1.0, 1.0, 0.0, 0.0], 0.0).is_empty());
}

#[test]
fn it_fades_by_the_wall_clock() {
    let t0 = Instant::now();
    let f = Fade::next(None, true, t0).unwrap();
    assert_eq!(f.level(t0), 0.0);
    // Halfway through the fade is half (smoothstep), however many
    // presents there were between.
    let half = f.level(at(t0, (FADE * 500.0) as u64));
    assert!((half - 0.5).abs() < 0.01, "{half}");
    assert!(f.level(at(t0, 30)) < f.level(at(t0, 60)));
    // The same `now` gives the same level.
    assert_eq!(f.level(at(t0, 40)), f.level(at(t0, 40)));
    // Never past the ends.
    assert_eq!(f.level(at(t0, 5000)), 1.0);
}

#[test]
fn a_turn_around_starts_from_the_shown_level() {
    let t0 = Instant::now();
    let f = Fade::next(None, true, t0).unwrap();
    let mid = f.level(at(t0, 50));
    let g = Fade::next(Some(f), false, at(t0, 50)).unwrap();
    assert!((g.level(at(t0, 50)) - mid).abs() < 1e-6);
    // Staying locked keeps the same fade.
    assert_eq!(Fade::next(Some(f), true, at(t0, 70)), Some(f));
}

#[test]
fn it_is_red_a_ring_and_four_ticks_at_the_reticle() {
    let s = shapes(RETICLE_CENTRE, [1.0, 1.0, 0.0, 0.0], 1.0);
    assert_eq!(s.len(), 5);
    for shape in &s {
        let Shape::Path { color, alpha, .. } = shape else {
            panic!("lines only")
        };
        assert_eq!(*color, RED);
        assert!((alpha - ALPHA).abs() < 1e-6);
    }
    // The ring is about the reticle's centre (rects::RETICLE: 288..352,
    // 208..272).
    let ring = points(&s[0]);
    let n = ring.len() as f32;
    let cx = ring.iter().map(|p| p[0]).sum::<f32>() / n;
    let cy = ring.iter().map(|p| p[1]).sum::<f32>() / n;
    assert!((cx - 320.0).abs() < 1e-3 && (cy - 240.0).abs() < 1e-3);
    // The mark stays inside the reserved reticle rectangle.
    for shape in &s {
        for p in points(shape) {
            assert!((288.0..=352.0).contains(&p[0]) && (208.0..=272.0).contains(&p[1]));
        }
    }
}

#[test]
fn the_fit_moves_and_scales_it_about_the_centre() {
    // Pinned 16:9: art squeezed to 0.75 wide about the view's centre.
    let sx = 0.75;
    let fit = [sx, 1.0, 320.0 * (1.0 - sx), 0.0];
    let s = shapes([320.0, 240.0], fit, 1.0);
    let ring = points(&s[0]);
    let n = ring.len() as f32;
    let cx = ring.iter().map(|p| p[0]).sum::<f32>() / n;
    assert!((cx - 320.0).abs() < 1e-3);
    let width = ring.iter().map(|p| p[0]).fold(f32::MIN, f32::max)
        - ring.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
    assert!((width - 2.0 * RING_RADIUS * sx).abs() < 0.05, "{width}");
    // A split screen's first view is centred on its own rect.
    let upper = shapes([320.0, 120.0], [1.0, 1.0, 0.0, 0.0], 1.0);
    let ring = points(&upper[0]);
    let cy = ring.iter().map(|p| p[1]).sum::<f32>() / ring.len() as f32;
    assert!((cy - 120.0).abs() < 1e-3);
}

#[test]
fn nothing_when_not_in_superbounce_mode() {
    // The source asks for the mark only with the mode on: `wanted` false
    // from a hidden start draws nothing at all.
    let t0 = Instant::now();
    assert_eq!(Fade::next(None, false, t0), None);
    assert_eq!(Fade::next(None, false, at(t0, 1000)), None);
}
