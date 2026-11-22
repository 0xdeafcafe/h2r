//! The brush, the eraser, fill and the adjustments, on pixels.

use bitmap_edit::{
    Adjust, Brush, Canvas, Channels, Rect, Stroke, apply_adjust, blend, fill_rect, flood, paint,
    stamp, to_linear, to_srgb,
};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

fn hard(size: f32, color: [u8; 4]) -> Brush {
    Brush {
        size,
        hardness: 1.0,
        opacity: 1.0,
        color,
        ..Brush::default()
    }
}

#[test]
fn srgb_round_trips_every_byte() {
    for v in 0..=255u8 {
        assert_eq!(to_srgb(to_linear(v)), v);
    }
}

#[test]
fn a_hard_stamp_is_a_disc() {
    let mut c = Canvas::new(16, 16, BLACK).unwrap();
    let dirty = stamp(&mut c, &hard(8.0, RED), 8.0, 8.0);
    // The centre is the colour, a corner of the box is untouched, and the
    // disc is symmetric.
    assert_eq!(c.pixel(8, 8), RED);
    assert_eq!(c.pixel(7, 7), RED);
    assert_eq!(c.pixel(0, 0), BLACK);
    assert_eq!(c.pixel(5, 8), c.pixel(10, 8));
    assert_eq!(c.pixel(8, 5), c.pixel(8, 10));
    assert_eq!(c.pixel(8, 5), RED);
    assert_eq!(c.pixel(8, 2), BLACK);
    assert!(dirty.encloses(Rect::new(4, 4, 8, 8)));
    assert!(Rect::new(0, 0, 16, 16).encloses(dirty));
}

#[test]
fn a_soft_stamp_falls_off_from_the_centre() {
    let mut c = Canvas::new(32, 32, BLACK).unwrap();
    let b = Brush {
        size: 24.0,
        hardness: 0.0,
        opacity: 1.0,
        color: WHITE,
        ..Brush::default()
    };
    stamp(&mut c, &b, 16.0, 16.0);
    let at = |x| c.pixel(x, 16)[0];
    assert!(at(16) > at(20), "{} {}", at(16), at(20));
    assert!(at(20) > at(24), "{} {}", at(20), at(24));
    assert!(at(24) > at(27) || at(27) == 0);
    assert_eq!(c.pixel(0, 0), BLACK);
}

#[test]
fn opacity_blends_in_linear_light() {
    // Half white over black is 0.5 in linear light, 188 encoded, not 128.
    let out = blend(BLACK, WHITE, 0.5, false, Channels::ALL);
    assert!((i32::from(out[0]) - 188).abs() <= 1, "{out:?}");
    assert_eq!(out[3], 255);
    // Opaque paint is exact.
    assert_eq!(blend(BLACK, RED, 1.0, false, Channels::ALL), RED);
    // Nothing for no coverage.
    assert_eq!(blend(BLACK, RED, 0.0, false, Channels::ALL), BLACK);
}

#[test]
fn painting_over_transparent_takes_the_colour_whole() {
    let out = blend([0, 0, 0, 0], [10, 200, 30, 255], 0.5, false, Channels::ALL);
    assert_eq!(&out[..3], &[10, 200, 30]);
    assert_eq!(out[3], 128);
}

#[test]
fn the_eraser_takes_alpha_and_keeps_colour() {
    let mut c = Canvas::new(8, 8, RED).unwrap();
    let eraser = Brush {
        erase: true,
        ..hard(4.0, WHITE)
    };
    stamp(&mut c, &eraser, 4.0, 4.0);
    assert_eq!(c.pixel(4, 4), [255, 0, 0, 0]);
    assert_eq!(c.pixel(0, 0), RED);
}

#[test]
fn channels_paint_apart() {
    let mut c = Canvas::new(8, 8, [10, 20, 30, 255]).unwrap();
    // Alpha alone: the colour's alpha value is the value painted.
    let b = Brush {
        channels: Channels::parse("a").unwrap(),
        color: [0, 0, 0, 64],
        ..hard(8.0, WHITE)
    };
    stamp(&mut c, &b, 4.0, 4.0);
    assert_eq!(c.pixel(4, 4), [10, 20, 30, 64]);
    // Red alone leaves green, blue and alpha.
    let b = Brush {
        channels: Channels::parse("r").unwrap(),
        color: [200, 0, 0, 255],
        ..hard(8.0, WHITE)
    };
    stamp(&mut c, &b, 4.0, 4.0);
    assert_eq!(c.pixel(4, 4), [200, 20, 30, 64]);
    assert_eq!(Channels::parse("rgba"), Some(Channels::ALL));
    assert_eq!(Channels::parse("x"), None);
    assert_eq!(Channels::parse(""), None);
}

#[test]
fn a_stroke_stamps_at_the_spacing() {
    let mut c = Canvas::new(64, 8, BLACK).unwrap();
    let b = Brush {
        spacing: 0.5,
        ..hard(4.0, WHITE)
    };
    // A line of 40 pixels with stamps every 2, along pixel centres: a
    // continuous bar. (Stamps one diameter apart only touch, so the
    // pixels between them are not fully covered.)
    let dirty = paint(&mut c, &b, &[[4.5, 4.5], [44.5, 4.5]]);
    for x in 3..=45 {
        assert_eq!(c.pixel(x, 4)[0], 255, "x = {x}");
    }
    assert_eq!(c.pixel(60, 4), BLACK);
    assert!(dirty.encloses(Rect::new(3, 2, 42, 4)));
}

#[test]
fn a_stroke_continues_its_spacing_across_calls() {
    // The same points in one stroke or fed one at a time give the same
    // pixels: the carry is in `Stroke`, not in the caller.
    let b = Brush {
        spacing: 0.5,
        ..hard(6.0, WHITE)
    };
    let pts = [[3.0, 3.0], [20.0, 9.0], [41.5, 4.25], [60.0, 12.0]];
    let mut a = Canvas::new(64, 16, BLACK).unwrap();
    paint(&mut a, &b, &pts);
    let mut c = Canvas::new(64, 16, BLACK).unwrap();
    let mut s = Stroke::new();
    for p in pts {
        s.to(&mut c, &b, p[0], p[1]);
    }
    assert_eq!(a.to_rgba(), c.to_rgba());
}

#[test]
fn a_stamp_off_the_edge_is_clipped() {
    let mut c = Canvas::new(8, 8, BLACK).unwrap();
    let dirty = stamp(&mut c, &hard(10.0, WHITE), -2.0, -2.0);
    assert!(Rect::new(0, 0, 8, 8).encloses(dirty));
    assert_eq!(c.pixel(0, 0), WHITE);
    assert!(stamp(&mut c, &hard(4.0, WHITE), 100.0, 100.0).is_empty());
}

#[test]
fn a_stamp_across_tiles_is_whole() {
    // The brush straddles the 256 pixel tile boundaries.
    let mut c = Canvas::new(300, 300, BLACK).unwrap();
    stamp(&mut c, &hard(20.0, WHITE), 256.0, 256.0);
    for (x, y) in [(247, 256), (264, 256), (256, 247), (256, 264), (255, 255), (256, 256)] {
        assert_eq!(c.pixel(x, y), WHITE, "{x},{y}");
    }
}

#[test]
fn fill_rect_sets_the_rectangle_and_clips() {
    let mut c = Canvas::new(10, 10, BLACK).unwrap();
    let r = fill_rect(&mut c, Rect::new(8, 8, 10, 10), RED, Channels::ALL);
    assert_eq!(r, Rect::new(8, 8, 2, 2));
    assert_eq!(c.pixel(9, 9), RED);
    assert_eq!(c.pixel(7, 9), BLACK);
    assert!(fill_rect(&mut c, Rect::new(20, 20, 3, 3), RED, Channels::ALL).is_empty());
}

#[test]
fn fill_rect_on_one_channel_keeps_the_rest() {
    let mut c = Canvas::new(4, 4, [1, 2, 3, 4]).unwrap();
    let all = c.bounds();
    fill_rect(&mut c, all, [9, 9, 9, 200], Channels::parse("a").unwrap());
    assert_eq!(c.pixel(2, 2), [1, 2, 3, 200]);
}

/// A 6x6 canvas split by a vertical wall of red at x = 3.
fn walled() -> Canvas {
    let mut c = Canvas::new(6, 6, BLACK).unwrap();
    for y in 0..6 {
        c.set_pixel(3, y, RED);
    }
    c
}

#[test]
fn flood_stops_at_a_wall() {
    let mut c = walled();
    let r = flood(&mut c, 0, 0, WHITE, 0.0, None, Channels::ALL).unwrap();
    assert_eq!(r, Rect::new(0, 0, 3, 6));
    assert_eq!(c.pixel(2, 5), WHITE);
    assert_eq!(c.pixel(3, 0), RED);
    assert_eq!(c.pixel(4, 0), BLACK);
}

#[test]
fn flood_tolerance_crosses_a_near_colour() {
    let mut c = Canvas::new(6, 1, [100, 100, 100, 255]).unwrap();
    c.set_pixel(3, 0, [110, 100, 100, 255]);
    c.set_pixel(4, 0, [200, 100, 100, 255]);
    let mut tight = c.clone();
    flood(&mut tight, 0, 0, WHITE, 0.0, None, Channels::ALL).unwrap();
    assert_eq!(tight.pixel(2, 0), WHITE);
    assert_eq!(tight.pixel(3, 0), [110, 100, 100, 255]);
    // 10 of 255 is about 0.04.
    flood(&mut c, 0, 0, WHITE, 0.05, None, Channels::ALL).unwrap();
    assert_eq!(c.pixel(3, 0), WHITE);
    assert_eq!(c.pixel(4, 0), [200, 100, 100, 255]);
}

#[test]
fn flood_is_limited_to_a_selection() {
    let mut c = Canvas::new(8, 8, BLACK).unwrap();
    let r = flood(&mut c, 2, 2, WHITE, 0.0, Some(Rect::new(1, 1, 3, 3)), Channels::ALL).unwrap();
    assert_eq!(r, Rect::new(1, 1, 3, 3));
    assert_eq!(c.pixel(0, 0), BLACK);
    assert_eq!(c.pixel(4, 4), BLACK);
    assert!(flood(&mut c, 6, 6, WHITE, 0.0, Some(Rect::new(1, 1, 3, 3)), Channels::ALL).is_err());
}

#[test]
fn flood_to_its_own_colour_ends() {
    let mut c = Canvas::new(5, 5, BLACK).unwrap();
    let r = flood(&mut c, 0, 0, BLACK, 0.0, None, Channels::ALL).unwrap();
    assert_eq!(r, Rect::new(0, 0, 5, 5));
}

#[test]
fn flood_rejects_a_bad_tolerance_or_seed() {
    let mut c = Canvas::new(4, 4, BLACK).unwrap();
    assert!(flood(&mut c, 0, 0, RED, 2.0, None, Channels::ALL).is_err());
    assert!(flood(&mut c, 0, 0, RED, f32::NAN, None, Channels::ALL).is_err());
    assert!(flood(&mut c, 9, 0, RED, 0.0, None, Channels::ALL).is_err());
}

#[test]
fn invert_flips_colour_and_leaves_alpha_when_asked() {
    let mut c = Canvas::new(2, 1, [10, 20, 30, 40]).unwrap();
    let rgb = Channels::parse("rgb").unwrap();
    let all = c.bounds();
    apply_adjust(&mut c, &Adjust::Invert, all, rgb).unwrap();
    assert_eq!(c.pixel(1, 0), [245, 235, 225, 40]);
    apply_adjust(&mut c, &Adjust::Invert, Rect::new(0, 0, 1, 1), Channels::ALL).unwrap();
    assert_eq!(c.pixel(0, 0), [10, 20, 30, 215]);
    assert_eq!(c.pixel(1, 0), [245, 235, 225, 40]);
}

#[test]
fn levels_stretch_and_gamma_lifts() {
    let mut c = Canvas::new(3, 1, BLACK).unwrap();
    c.set_pixel(0, 0, [51, 51, 51, 255]);
    c.set_pixel(1, 0, [153, 153, 153, 255]);
    c.set_pixel(2, 0, [128, 128, 128, 255]);
    let levels = Adjust::Levels {
        black: 0.2,
        white: 0.6,
        gamma: 1.0,
    };
    let all = c.bounds();
    apply_adjust(&mut c, &levels, all, Channels::parse("rgb").unwrap()).unwrap();
    assert_eq!(c.pixel(0, 0)[0], 0);
    assert_eq!(c.pixel(1, 0)[0], 255);
    assert_eq!(c.pixel(0, 0)[3], 255);
    let mut g = Canvas::new(1, 1, [64, 64, 64, 255]).unwrap();
    let gamma = Adjust::Levels {
        black: 0.0,
        white: 1.0,
        gamma: 2.0,
    };
    let gall = g.bounds();
    apply_adjust(&mut g, &gamma, gall, Channels::ALL).unwrap();
    assert!(g.pixel(0, 0)[0] > 64);
    let inverted = Adjust::Levels {
        black: 0.7,
        white: 0.3,
        gamma: 1.0,
    };
    assert!(apply_adjust(&mut g, &inverted, gall, Channels::ALL).is_err());
}

#[test]
fn hsv_identity_keeps_pixels_and_a_turn_moves_hue() {
    let mut c = Canvas::new(1, 1, [200, 40, 90, 255]).unwrap();
    let id = Adjust::Hsv {
        hue: 0.0,
        saturation: 1.0,
        value: 1.0,
    };
    let all = c.bounds();
    apply_adjust(&mut c, &id, all, Channels::ALL).unwrap();
    let p = c.pixel(0, 0);
    for (a, b) in p.into_iter().zip([200u8, 40, 90, 255]) {
        assert!((i32::from(a) - i32::from(b)).abs() <= 1, "{p:?}");
    }
    // Red turned 120 degrees is green.
    let mut r = Canvas::new(1, 1, RED).unwrap();
    let turn = Adjust::Hsv {
        hue: 120.0,
        saturation: 1.0,
        value: 1.0,
    };
    let rall = r.bounds();
    apply_adjust(&mut r, &turn, rall, Channels::ALL).unwrap();
    assert_eq!(r.pixel(0, 0), [0, 255, 0, 255]);
    // Zero saturation is grey.
    let mut s = Canvas::new(1, 1, RED).unwrap();
    let grey = Adjust::Hsv {
        hue: 0.0,
        saturation: 0.0,
        value: 1.0,
    };
    let sall = s.bounds();
    apply_adjust(&mut s, &grey, sall, Channels::ALL).unwrap();
    assert_eq!(s.pixel(0, 0), WHITE);
}

#[test]
fn brush_parameters_are_checked() {
    let mut b = Brush::default();
    assert!(b.validate().is_ok());
    b.size = 0.0;
    assert!(b.validate().is_err());
    b = Brush {
        hardness: 2.0,
        ..Brush::default()
    };
    assert!(b.validate().is_err());
    b = Brush {
        opacity: f32::NAN,
        ..Brush::default()
    };
    assert!(b.validate().is_err());
}
