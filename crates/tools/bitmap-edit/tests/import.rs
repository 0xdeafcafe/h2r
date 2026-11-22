//! Import: PNG in and out, and resampling when the sizes differ.

use bitmap_edit::{
    Bitmap, BitmapOp, Canvas, Error, LayerId, Rect, decode_png, encode_png, resample, to_srgb,
};

const L0: LayerId = LayerId::new(0, 0, 0);

fn gradient(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&[(x * 255 / (w - 1).max(1)) as u8, (y * 255 / (h - 1).max(1)) as u8, 7, 255]);
        }
    }
    v
}

#[test]
fn same_size_is_the_same_bytes() {
    let src = gradient(5, 3);
    assert_eq!(resample(&src, 5, 3, 5, 3).unwrap(), src);
}

#[test]
fn down_by_two_is_a_box_average_in_linear_light() {
    // Black and white texels average to 0.5 in linear light: 188.
    let src = [0, 0, 0, 255, 255, 255, 255, 255];
    let out = resample(&src, 2, 1, 1, 1).unwrap();
    assert_eq!(out.len(), 4);
    assert_eq!(out[0], to_srgb(0.5));
    assert_eq!(out[0], out[1]);
    assert_eq!(out[3], 255);
    // A flat image stays flat.
    let flat = [90u8, 120, 30, 200].repeat(16);
    let out = resample(&flat, 4, 4, 2, 2).unwrap();
    for px in out.chunks_exact(4) {
        assert_eq!(px, &[90, 120, 30, 200]);
    }
}

#[test]
fn down_by_a_fraction_weights_the_overlap() {
    // Three texels into two: the middle one is shared.
    let src = [0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255];
    let out = resample(&src, 3, 1, 2, 1).unwrap();
    assert_eq!(out[0], out[4], "the two halves are mirror images");
    assert!(out[0] > 0 && out[0] < 255);
}

#[test]
fn up_is_bilinear_and_monotone() {
    let src = [0, 0, 0, 255, 255, 255, 255, 255];
    let out = resample(&src, 2, 1, 8, 1).unwrap();
    let r: Vec<u8> = out.chunks_exact(4).map(|p| p[0]).collect();
    assert_eq!(r.len(), 8);
    assert_eq!(r[0], 0);
    assert_eq!(r[7], 255);
    assert!(r.windows(2).all(|w| w[0] <= w[1]), "{r:?}");
    // One texel up is that colour everywhere.
    let one = resample(&[10, 20, 30, 255], 1, 1, 3, 3).unwrap();
    for px in one.chunks_exact(4) {
        assert_eq!(px, &[10, 20, 30, 255]);
    }
}

#[test]
fn transparent_texels_do_not_bleed_colour() {
    // Opaque red beside a transparent texel whose colour is blue: the
    // average is red at half alpha.
    let src = [255, 0, 0, 255, 0, 0, 255, 0];
    let out = resample(&src, 2, 1, 1, 1).unwrap();
    assert_eq!(&out[..3], &[255, 0, 0]);
    assert_eq!(out[3], 128);
}

#[test]
fn resample_checks_its_input() {
    assert!(matches!(resample(&[0; 8], 2, 1, 0, 1), Err(Error::Size(_))));
    assert!(matches!(resample(&[0; 7], 2, 1, 1, 1), Err(Error::Rect(_))));
}

#[test]
fn png_round_trips() {
    let src = gradient(7, 5);
    let png = encode_png(7, 5, &src).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let (w, h, back) = decode_png(&png).unwrap();
    assert_eq!((w, h), (7, 5));
    assert_eq!(back, src);
}

#[test]
fn png_refuses_what_it_cannot_hold() {
    assert!(encode_png(0, 4, &[]).is_err());
    assert!(encode_png(2, 2, &[0; 15]).is_err());
    assert!(matches!(decode_png(b"not a png"), Err(Error::Png(_))));
}

#[test]
fn a_canvas_reads_back_through_png() {
    let mut c = Canvas::new(300, 20, [1, 2, 3, 255]).unwrap();
    c.set_pixel(299, 19, [9, 8, 7, 6]);
    let png = encode_png(c.width(), c.height(), &c.to_rgba()).unwrap();
    let (w, h, rgba) = decode_png(&png).unwrap();
    let back = Canvas::from_rgba(w, h, &rgba).unwrap();
    assert_eq!(back.pixel(299, 19), [9, 8, 7, 6]);
    assert_eq!(back.pixel(0, 0), [1, 2, 3, 255]);
    assert_eq!(back.to_rgba(), c.to_rgba());
}

fn bitmap(w: u32, h: u32) -> Bitmap {
    let mut b = Bitmap::new();
    b.add_layer(L0, Canvas::new(w, h, [0, 0, 0, 255]).unwrap(), None);
    b
}

#[test]
fn import_of_the_same_size_replaces_the_layer() {
    let mut b = bitmap(8, 4);
    let src = gradient(8, 4);
    let a = b
        .apply(&BitmapOp::Import {
            layer: L0,
            rect: None,
            width: 8,
            height: 4,
            rgba: src.clone(),
        })
        .unwrap();
    assert_eq!(a.dirty, Rect::new(0, 0, 8, 4));
    assert_eq!(b.layer(L0).unwrap().canvas.to_rgba(), src);
    b.undo().unwrap();
    assert!(!b.is_edited());
}

#[test]
fn import_resamples_to_the_layer() {
    let mut b = bitmap(4, 4);
    // A 2 by 2 image of one colour fills the 4 by 4 layer.
    let src = [50u8, 100, 150, 255].repeat(4);
    b.apply(&BitmapOp::Import {
        layer: L0,
        rect: None,
        width: 2,
        height: 2,
        rgba: src,
    })
    .unwrap();
    let c = &b.layer(L0).unwrap().canvas;
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(c.pixel(x, y), [50, 100, 150, 255]);
        }
    }
    // A larger one comes down.
    let big = [200u8, 10, 10, 255].repeat(64 * 64);
    b.apply(&BitmapOp::Import {
        layer: L0,
        rect: None,
        width: 64,
        height: 64,
        rgba: big,
    })
    .unwrap();
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(3, 3), [200, 10, 10, 255]);
}

#[test]
fn import_into_a_selection_keeps_the_rest() {
    let mut b = bitmap(8, 8);
    let src = [255u8, 255, 255, 255].repeat(4);
    let a = b
        .apply(&BitmapOp::Import {
            layer: L0,
            rect: Some(Rect::new(2, 2, 4, 4)),
            width: 2,
            height: 2,
            rgba: src,
        })
        .unwrap();
    assert_eq!(a.dirty, Rect::new(2, 2, 4, 4));
    let c = &b.layer(L0).unwrap().canvas;
    assert_eq!(c.pixel(3, 3), [255, 255, 255, 255]);
    assert_eq!(c.pixel(1, 1), [0, 0, 0, 255]);
    assert_eq!(c.pixel(6, 6), [0, 0, 0, 255]);
}

#[test]
fn import_refuses_a_rectangle_outside_or_a_short_buffer() {
    let mut b = bitmap(8, 8);
    let outside = BitmapOp::Import {
        layer: L0,
        rect: Some(Rect::new(6, 6, 4, 4)),
        width: 1,
        height: 1,
        rgba: vec![0; 4],
    };
    assert!(matches!(b.apply(&outside), Err(Error::Rect(_))));
    let short = BitmapOp::Import {
        layer: L0,
        rect: None,
        width: 4,
        height: 4,
        rgba: vec![0; 12],
    };
    assert!(matches!(b.apply(&short), Err(Error::Rect(_))));
    assert!(!b.is_edited());
    assert_eq!(b.undo_len(), 0);
}
