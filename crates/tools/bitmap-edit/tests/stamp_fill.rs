//! The brush, the eraser, fill and the adjustments, on pixels; the clone
//! stamp, channel copy and the turns.

use bitmap_edit::{
    Adjust, Bitmap, BitmapOp, Brush, Canvas, Channel, Channels, Error, LayerId, Rect, Stroke, Turn,
    apply_adjust, blend, copy_channel, fill_rect, flood, paint, stamp, to_linear, to_srgb, turn,
};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const L0: LayerId = LayerId::new(0, 0, 0);

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

/// A canvas whose pixel at (`x`, `y`) is `[x, y, 7, 255]`.
fn numbered(w: u32, h: u32) -> Canvas {
    let mut c = Canvas::new(w, h, BLACK).unwrap();
    for y in 0..h {
        for x in 0..w {
            c.set_pixel(x, y, [x as u8, y as u8, 7, 255]);
        }
    }
    c
}

/// A canvas whose pixel at (`x`, `y`) is `[x + 10 * y, 0, 0, 255]`.
fn grid(w: u32, h: u32) -> Canvas {
    let mut c = Canvas::new(w, h, BLACK).unwrap();
    for y in 0..h {
        for x in 0..w {
            c.set_pixel(x, y, [(x + 10 * y) as u8, 0, 0, 255]);
        }
    }
    c
}

/// A bitmap of the one layer `L0`, holding `canvas`.
fn one(canvas: Canvas) -> Bitmap {
    let mut b = Bitmap::new();
    b.add_layer(L0, canvas, None);
    b
}

/// A one point clone stroke of a hard brush of `size`, from `source` to
/// `at`, on the layer it paints.
fn clone_dab(source: [f32; 2], at: [f32; 2], size: f32) -> BitmapOp {
    BitmapOp::Clone {
        layer: L0,
        from: None,
        source,
        points: vec![at],
        brush: hard(size, WHITE),
    }
}

#[test]
fn two_points_are_a_line_from_one_to_the_other() {
    let mut b = one(Canvas::new(8, 4, BLACK).unwrap());
    let line = |to: f32| BitmapOp::Paint {
        layer: L0,
        points: vec![[0.5, 1.5], [to, 1.5]],
        brush: hard(1.0, RED),
    };
    // A 1 pixel brush stamps every pixel of the row from one point to
    // the other; the next pixel is not touched.
    b.apply(&line(5.5)).unwrap();
    let c = &b.layer(L0).unwrap().canvas;
    for x in 0..6 {
        assert_eq!(c.pixel(x, 1), RED, "x = {x}");
    }
    assert_eq!(c.pixel(6, 1), BLACK);
    assert_eq!(c.pixel(3, 3), BLACK);
    // A point off the pixel centres is reached too: the last stamp is on
    // it, where stamps every spacing from the first point would stop
    // short.
    b.undo().unwrap();
    let a = b.apply(&line(5.2)).unwrap();
    let c = &b.layer(L0).unwrap().canvas;
    assert!(c.pixel(5, 1)[0] > 200, "{:?}", c.pixel(5, 1));
    assert_eq!(c.pixel(6, 1), BLACK);
    assert!(a.dirty.encloses(Rect::new(0, 1, 7, 1)));
    // A line far off the canvas is no step, however long.
    b.undo().unwrap();
    let far = BitmapOp::Paint {
        layer: L0,
        points: vec![[-1.0e20, -50.0], [1.0e20, -50.0]],
        brush: hard(1.0, RED),
    };
    let a = b.apply(&far).unwrap();
    assert!(a.dirty.is_empty());
    assert_eq!(b.undo_len(), 0);
}

#[test]
fn a_clone_stamp_copies_the_pixels_at_the_offset() {
    let mut b = one(numbered(8, 4));
    // The source is pixel (1, 1), the stamp pixel (5, 1): four to the right.
    let a = b.apply(&clone_dab([1.5, 1.5], [5.5, 1.5], 4.0)).unwrap();
    let c = &b.layer(L0).unwrap().canvas;
    for y in 0..3 {
        for x in 4..7 {
            assert_eq!(c.pixel(x, y), [x as u8 - 4, y as u8, 7, 255], "{x},{y}");
        }
    }
    // The rim pixel is the brush's anti-aliased edge: half the source
    // over what was there, in linear light.
    assert_eq!(c.pixel(7, 1), [5, 1, 7, 255]);
    // Past the source's edge (no pixel at x = -1) and outside the stamp,
    // the pixels are as they were.
    assert_eq!(c.pixel(3, 1), [3, 1, 7, 255]);
    assert_eq!(c.pixel(7, 3), [7, 3, 7, 255]);
    assert_eq!(a.dirty, Rect::new(2, 0, 6, 4));
    assert_eq!(a.tiles, vec![(0, 0)]);
    assert!(b.is_edited());
}

#[test]
fn a_clone_stroke_reads_the_layer_as_it_began() {
    let mut b = one(numbered(12, 1));
    // Four pixels along the row: what the stroke reaches at x = 8 and 9
    // is read from x = 4 and 5, which the stroke itself has painted by
    // then, so a stroke reading its own output would repeat x = 0 and 1.
    let op = BitmapOp::Clone {
        layer: L0,
        from: None,
        source: [0.5, 0.5],
        points: vec![[4.5, 0.5], [9.5, 0.5]],
        brush: hard(2.0, WHITE),
    };
    let a = b.apply(&op).unwrap();
    let c = &b.layer(L0).unwrap().canvas;
    for x in 4..10 {
        assert_eq!(c.pixel(x, 0), [(x - 4) as u8, 0, 7, 255], "x = {x}");
    }
    // Outside the stroke: the pixel left of its first stamp (no source
    // pixel there) and the row past the last stamp's fringe.
    assert_eq!(c.pixel(3, 0), [3, 0, 7, 255]);
    assert_eq!(c.pixel(11, 0), [11, 0, 7, 255]);
    assert_eq!(a.dirty, Rect::new(2, 0, 10, 1));
}

#[test]
fn a_clone_stroke_reads_another_layer_and_leaves_it_alone() {
    let mut b = Bitmap::new();
    b.add_layer(L0, numbered(8, 4), None);
    let other = LayerId::new(1, 0, 0);
    b.add_layer(other, Canvas::new(8, 4, [0; 4]).unwrap(), None);
    // One down and right of the source: the offset is (1, 1).
    let op = BitmapOp::Clone {
        layer: other,
        from: Some(L0),
        source: [2.5, 1.5],
        points: vec![[3.5, 2.5]],
        brush: hard(4.0, WHITE),
    };
    let a = b.apply(&op).unwrap();
    let o = &b.layer(other).unwrap().canvas;
    for y in 1..4 {
        for x in 2..5 {
            assert_eq!(
                o.pixel(x, y),
                [(x - 1) as u8, (y - 1) as u8, 7, 255],
                "{x},{y}"
            );
        }
    }
    assert_eq!(o.pixel(0, 0), [0; 4]);
    assert_eq!(a.layer, other);
    assert!(b.layer(other).unwrap().is_edited());
    assert!(!b.layer(L0).unwrap().is_edited());
    // A layer that is not there is refused, with nothing changed.
    let mut b = one(numbered(4, 4));
    let op = BitmapOp::Clone {
        layer: L0,
        from: Some(LayerId::new(7, 0, 0)),
        source: [0.5, 0.5],
        points: vec![[1.5, 1.5]],
        brush: hard(2.0, WHITE),
    };
    assert_eq!(b.apply(&op), Err(Error::NoLayer(LayerId::new(7, 0, 0))));
    assert!(!b.is_edited());
}

#[test]
fn a_clone_stroke_copies_and_never_erases() {
    let mut b = one(numbered(8, 4));
    let mut op = clone_dab([1.5, 1.5], [5.5, 1.5], 2.0);
    if let BitmapOp::Clone { brush, .. } = &mut op {
        *brush = Brush {
            erase: true,
            ..hard(2.0, WHITE)
        };
    }
    let a = b.apply(&op).unwrap();
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(5, 1), [1, 1, 7, 255]);
    assert!(!a.dirty.is_empty());
    // Copying a layer onto itself where nothing differs is no step.
    let mut b = one(numbered(8, 4));
    let a = b.apply(&clone_dab([5.5, 1.5], [5.5, 1.5], 4.0)).unwrap();
    assert!(a.dirty.is_empty());
    assert_eq!(b.undo_len(), 0);
    assert!(!b.is_edited());
}

#[test]
fn a_clone_stroke_refuses_what_a_stroke_does() {
    let mut b = one(numbered(8, 4));
    let bad = |source: [f32; 2], points: Vec<[f32; 2]>| BitmapOp::Clone {
        layer: L0,
        from: None,
        source,
        points,
        brush: hard(2.0, WHITE),
    };
    assert!(matches!(
        b.apply(&bad([f32::NAN, 0.5], vec![[1.0, 1.0]])),
        Err(Error::Params(_))
    ));
    assert!(matches!(
        b.apply(&bad([0.5, 0.5], vec![])),
        Err(Error::Params(_))
    ));
    assert!(!b.is_edited());
    b.begin_stroke(L0, Brush::default()).unwrap();
    assert_eq!(
        b.apply(&bad([0.5, 0.5], vec![[1.0, 1.0]])),
        Err(Error::Busy)
    );
    assert_eq!(
        b.begin_clone(L0, Brush::default(), None, [0.5, 0.5]),
        Err(Error::Busy)
    );
    b.cancel_stroke();
    let mut locked = Bitmap::new();
    locked.add_layer(
        L0,
        numbered(4, 4),
        Some("its channels are a hypothesis".to_owned()),
    );
    match locked.apply(&bad([0.5, 0.5], vec![[1.0, 1.0]])) {
        Err(Error::ReadOnly(why)) => assert!(why.contains("hypothesis")),
        other => panic!("{other:?}"),
    }
    assert!(
        locked
            .begin_clone(L0, Brush::default(), None, [0.5, 0.5])
            .is_err()
    );
}

#[test]
fn channel_copy_writes_the_channels_asked() {
    let mut c = Canvas::new(2, 1, [200, 60, 20, 255]).unwrap();
    c.set_pixel(1, 0, [10, 20, 30, 128]);
    let all = c.bounds();
    // The luma (what the encoder writes for L8) into alpha.
    copy_channel(&mut c, Channel::Luma, Channels::parse("a").unwrap(), all).unwrap();
    assert_eq!(c.pixel(0, 0), [200, 60, 20, 98]);
    assert_eq!(c.pixel(1, 0), [10, 20, 30, 18]);
    // Alpha into the colour of one pixel.
    let one_pixel = copy_channel(
        &mut c,
        Channel::A,
        Channels::parse("rgb").unwrap(),
        Rect::new(1, 0, 1, 1),
    )
    .unwrap();
    assert_eq!(one_pixel, Rect::new(1, 0, 1, 1));
    assert_eq!(c.pixel(1, 0), [18, 18, 18, 18]);
    assert_eq!(c.pixel(0, 0), [200, 60, 20, 98]);
    // Red into green and blue.
    copy_channel(&mut c, Channel::R, Channels::parse("gb").unwrap(), all).unwrap();
    assert_eq!(c.pixel(0, 0), [200, 200, 200, 98]);
    assert_eq!(c.pixel(1, 0), [18, 18, 18, 18]);
    // No channel to write is refused.
    let none = Channels {
        r: false,
        g: false,
        b: false,
        a: false,
    };
    assert!(copy_channel(&mut c, Channel::G, none, all).is_err());
    for ch in Channel::ALL {
        assert_eq!(Channel::parse(ch.name()), Some(ch));
        assert_eq!(
            serde_json::to_value(ch).unwrap(),
            serde_json::json!(ch.name())
        );
    }
}

#[test]
fn flip_and_turns_move_the_pixels_of_their_rectangle() {
    // 0 1 2 3 over 10 11 12 13.
    let mut c = grid(4, 2);
    let all = c.bounds();
    turn(&mut c, Turn::FlipHorizontal, all).unwrap();
    assert_eq!(
        (c.pixel(0, 0)[0], c.pixel(3, 0)[0], c.pixel(0, 1)[0]),
        (3, 0, 13)
    );
    turn(&mut c, Turn::FlipVertical, all).unwrap();
    assert_eq!((c.pixel(0, 0)[0], c.pixel(3, 0)[0]), (13, 10));
    // And back.
    turn(&mut c, Turn::FlipVertical, all).unwrap();
    turn(&mut c, Turn::FlipHorizontal, all).unwrap();
    assert_eq!(c.pixel(3, 1)[0], 13);
    turn(&mut c, Turn::Half, all).unwrap();
    assert_eq!((c.pixel(0, 0)[0], c.pixel(3, 1)[0]), (13, 0));
    // A quarter turn needs a square.
    assert!(matches!(
        turn(&mut c, Turn::Quarter, all),
        Err(Error::Rect(_))
    ));
    assert!(matches!(
        turn(&mut c, Turn::ThreeQuarters, all),
        Err(Error::Rect(_))
    ));
    // In a rectangle of a row; the rest stays.
    let mut r = grid(5, 1);
    turn(&mut r, Turn::Half, Rect::new(1, 0, 3, 1)).unwrap();
    let row: Vec<u8> = (0..5).map(|x| r.pixel(x, 0)[0]).collect();
    assert_eq!(row, [0, 3, 2, 1, 4]);
    // 0 1 2 / 10 11 12 / 20 21 22: a quarter turn clockwise takes the
    // top row to the right column.
    let mut q = grid(3, 3);
    let square = q.bounds();
    turn(&mut q, Turn::Quarter, square).unwrap();
    assert_eq!(
        (
            q.pixel(0, 0)[0],
            q.pixel(2, 0)[0],
            q.pixel(0, 2)[0],
            q.pixel(1, 1)[0]
        ),
        (20, 0, 22, 11)
    );
    // A quarter anticlockwise undoes it.
    turn(&mut q, Turn::ThreeQuarters, square).unwrap();
    assert_eq!(q.pixel(2, 0)[0], 2);
    // Two quarters are a half.
    turn(&mut q, Turn::Quarter, square).unwrap();
    turn(&mut q, Turn::Quarter, square).unwrap();
    assert_eq!((q.pixel(0, 0)[0], q.pixel(2, 2)[0]), (22, 0));
    for t in Turn::ALL {
        assert_eq!(Turn::parse(t.name()), Some(t));
        assert_eq!(
            serde_json::to_value(t).unwrap(),
            serde_json::json!(t.name())
        );
    }
}
