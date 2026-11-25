//! Undo, the shared tiles under it, the dirty log and ops as values.

use bitmap_edit::{
    Adjust, Bitmap, BitmapOp, Brush, Canvas, Channel, Channels, Error, LayerId, Rect, TILE, Turn,
};

const BLACK: [u8; 4] = [0, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const L0: LayerId = LayerId::new(0, 0, 0);

/// A 3 by 3 tiles bitmap, black.
fn big() -> Bitmap {
    let mut b = Bitmap::new();
    let side = TILE * 2 + 44;
    b.add_layer(L0, Canvas::new(side, side, BLACK).unwrap(), None);
    b
}

/// A bitmap whose one layer's pixel at (`x`, `y`) is `[x, y, 7, 255]`.
fn numbered(side: u32) -> Bitmap {
    let mut c = Canvas::new(side, side, BLACK).unwrap();
    for y in 0..side {
        for x in 0..side {
            c.set_pixel(x, y, [x as u8, y as u8, 7, 255]);
        }
    }
    let mut b = Bitmap::new();
    b.add_layer(L0, c, None);
    b
}

/// The pixel at (`x`, `y`) of the bitmap's one layer.
fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
    b.layer(L0).unwrap().canvas.pixel(x, y)
}

fn dab(at: [f32; 2]) -> BitmapOp {
    BitmapOp::Paint {
        layer: L0,
        points: vec![at],
        brush: Brush {
            size: 8.0,
            hardness: 1.0,
            color: WHITE,
            ..Brush::default()
        },
    }
}

#[test]
fn a_snapshot_shares_every_tile_a_stroke_did_not_touch() {
    let mut b = big();
    let opened = b.layer(L0).unwrap().canvas.clone();
    // 9 tiles; a stroke in the middle of the first one.
    let applied = b.apply(&dab([20.0, 20.0])).unwrap();
    assert_eq!(applied.tiles, vec![(0, 0)]);
    let now = &b.layer(L0).unwrap().canvas;
    assert_eq!(now.shared_tile_count(&opened), 8);
    assert!(!now.shares_tile(&opened, 0, 0));
    assert!(now.shares_tile(&opened, 1, 1));
    assert!(now.shares_tile(&opened, 2, 2));
    assert_eq!(now.changed_tiles(&opened), vec![(0, 0)]);
    // The source kept the original bytes.
    assert_eq!(opened.pixel(20, 20), BLACK);
    assert_eq!(now.pixel(20, 20), WHITE);
}

#[test]
fn an_edge_tile_is_smaller() {
    let b = big();
    let c = &b.layer(L0).unwrap().canvas;
    assert_eq!(c.tile_cols(), 3);
    assert_eq!(c.tile_rect(2, 2), Rect::new(512, 512, 44, 44));
    assert_eq!(c.tile_rect(0, 0), Rect::new(0, 0, 256, 256));
}

#[test]
fn a_stroke_across_a_tile_edge_touches_both() {
    let mut b = big();
    let applied = b.apply(&dab([256.0, 100.0])).unwrap();
    assert_eq!(applied.tiles, vec![(0, 0), (1, 0)]);
}

#[test]
fn undo_restores_the_snapshot_and_redo_the_edit() {
    let mut b = big();
    b.apply(&dab([20.0, 20.0])).unwrap();
    b.apply(&dab([300.0, 20.0])).unwrap();
    assert!(b.is_edited());
    assert_eq!(b.undo_len(), 2);
    let u = b.undo().unwrap();
    assert_eq!(u.tiles, vec![(1, 0)]);
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(300, 20), BLACK);
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(20, 20), WHITE);
    b.undo().unwrap();
    // Back to the opened tiles: nothing is edited, by pointer.
    assert!(!b.is_edited());
    assert!(b.undo().is_none());
    let r = b.redo().unwrap();
    assert_eq!(r.tiles, vec![(0, 0)]);
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(20, 20), WHITE);
    b.redo().unwrap();
    assert!(b.redo().is_none());
    assert!(b.is_edited());
}

#[test]
fn a_new_edit_forgets_the_redo_history() {
    let mut b = big();
    b.apply(&dab([20.0, 20.0])).unwrap();
    b.undo().unwrap();
    assert!(b.can_redo());
    b.apply(&dab([40.0, 40.0])).unwrap();
    assert!(!b.can_redo());
}

#[test]
fn a_drag_is_one_undo_step_with_many_dirty_rectangles() {
    let mut b = big();
    b.begin_stroke(
        L0,
        Brush {
            size: 6.0,
            hardness: 1.0,
            color: WHITE,
            ..Brush::default()
        },
    )
    .unwrap();
    let r1 = b.stroke_to(10.0, 10.0).unwrap();
    let r2 = b.stroke_to(60.0, 10.0).unwrap();
    assert!(!r1.is_empty() && !r2.is_empty());
    // Pixels are visible mid-stroke, and the dirty log has them.
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(30, 10), WHITE);
    let d = b.take_dirty();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].layer, L0);
    assert!(d[0].rect.encloses(r1) && d[0].rect.encloses(r2));
    assert_eq!(b.undo_len(), 0);
    let applied = b.end_stroke().unwrap();
    assert!(applied.dirty.encloses(r1.union(r2)));
    assert_eq!(b.undo_len(), 1);
    b.undo().unwrap();
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(30, 10), BLACK);
    assert!(!b.is_edited());
}

#[test]
fn a_cancelled_stroke_leaves_nothing() {
    let mut b = big();
    b.begin_stroke(L0, Brush::default()).unwrap();
    b.stroke_to(50.0, 50.0).unwrap();
    assert!(b.stroke_active());
    b.cancel_stroke();
    assert!(!b.stroke_active());
    assert_eq!(b.undo_len(), 0);
    assert!(!b.is_edited());
    // Edits are refused during a stroke.
    b.begin_stroke(L0, Brush::default()).unwrap();
    assert_eq!(b.apply(&dab([1.0, 1.0])), Err(Error::Busy));
    assert_eq!(b.undo(), None);
}

#[test]
fn a_stroke_that_missed_the_canvas_is_no_step() {
    let mut b = big();
    let a = b.apply(&dab([-500.0, -500.0])).unwrap();
    assert!(a.dirty.is_empty());
    assert_eq!(b.undo_len(), 0);
    assert!(b.take_dirty().is_empty());
}

#[test]
fn dirty_rectangles_merge_per_layer_until_taken() {
    let mut b = big();
    b.apply(&dab([20.0, 20.0])).unwrap();
    b.apply(&dab([100.0, 60.0])).unwrap();
    let d = b.take_dirty();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].rect, Rect::new(15, 15, 90, 50));
    assert!(b.take_dirty().is_empty());
    // An undo is dirty too, over the tiles it swapped.
    b.undo().unwrap();
    let d = b.take_dirty();
    assert_eq!(d[0].rect, Rect::new(0, 0, TILE, TILE));
}

#[test]
fn fill_and_adjust_are_ops_with_one_step_each() {
    let mut b = big();
    let fill = BitmapOp::Fill {
        layer: L0,
        at: None,
        rect: Some(Rect::new(10, 10, 20, 20)),
        color: [200, 0, 0, 255],
        tolerance: 0.0,
        channels: Channels::ALL,
    };
    let a = b.apply(&fill).unwrap();
    assert_eq!(a.dirty, Rect::new(10, 10, 20, 20));
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(15, 15), [200, 0, 0, 255]);
    let invert = BitmapOp::Adjust {
        layer: L0,
        adjust: Adjust::Invert,
        rect: Some(Rect::new(0, 0, 16, 16)),
        channels: Channels::parse("rgb").unwrap(),
    };
    b.apply(&invert).unwrap();
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(15, 15), [55, 255, 255, 255]);
    assert_eq!(b.undo_len(), 2);
    b.undo().unwrap();
    assert_eq!(b.layer(L0).unwrap().canvas.pixel(15, 15), [200, 0, 0, 255]);
}

#[test]
fn a_clone_drag_is_one_undo_step() {
    let mut b = numbered(64);
    b.begin_clone(
        L0,
        Brush {
            size: 2.0,
            hardness: 1.0,
            ..Brush::default()
        },
        None,
        [4.5, 8.5],
    )
    .unwrap();
    // Down and right of the source by (4, 8).
    let r1 = b.stroke_to(8.5, 16.5).unwrap();
    let r2 = b.stroke_to(20.5, 16.5).unwrap();
    assert!(!r1.is_empty() && !r2.is_empty());
    assert_eq!(px(&b, 8, 16), [4, 8, 7, 255]);
    assert_eq!(px(&b, 12, 16), [8, 8, 7, 255]);
    assert_eq!(px(&b, 20, 16), [16, 8, 7, 255]);
    // The row read is as it was.
    assert_eq!(px(&b, 8, 8), [8, 8, 7, 255]);
    assert_eq!(px(&b, 16, 8), [16, 8, 7, 255]);
    assert!(b.stroke_active());
    assert_eq!(b.undo_len(), 0);
    let applied = b.end_stroke().unwrap();
    assert!(applied.dirty.encloses(r1.union(r2)));
    assert_eq!(b.undo_len(), 1);
    b.undo().unwrap();
    assert_eq!(px(&b, 12, 16), [12, 16, 7, 255]);
    assert!(!b.is_edited());
}

#[test]
fn an_edit_that_changed_no_pixel_is_no_step() {
    // White over white, as a drag.
    let mut b = Bitmap::new();
    b.add_layer(L0, Canvas::new(16, 16, WHITE).unwrap(), None);
    b.begin_stroke(L0, Brush::default()).unwrap();
    b.stroke_to(8.0, 8.0).unwrap();
    let a = b.end_stroke().unwrap();
    assert!(a.dirty.is_empty());
    assert_eq!(b.undo_len(), 0);
    assert!(b.undo().is_none());
    assert!(!b.is_edited());
    // A uniform layer turned, or its red copied into red.
    let _ = b.take_dirty();
    let quarter = BitmapOp::Turn {
        layer: L0,
        turn: Turn::Quarter,
        rect: None,
    };
    let a = b.apply(&quarter).unwrap();
    assert!(a.dirty.is_empty());
    let same = BitmapOp::CopyChannel {
        layer: L0,
        from: Channel::R,
        to: Channels::parse("r").unwrap(),
        rect: None,
    };
    let a = b.apply(&same).unwrap();
    assert!(a.dirty.is_empty());
    assert_eq!(b.undo_len(), 0);
    assert!(!b.is_edited());
    assert!(b.take_dirty().is_empty());
    // A stroke that does change a pixel is a step.
    b.begin_stroke(
        L0,
        Brush {
            color: BLACK,
            ..Brush::default()
        },
    )
    .unwrap();
    b.stroke_to(8.0, 8.0).unwrap();
    let a = b.end_stroke().unwrap();
    assert!(!a.dirty.is_empty());
    assert_eq!(b.undo_len(), 1);
    assert!(b.is_edited());
}

#[test]
fn channel_copy_and_turn_are_ops_with_one_step_each() {
    let mut b = numbered(4);
    let copy = BitmapOp::CopyChannel {
        layer: L0,
        from: Channel::R,
        to: Channels::parse("g").unwrap(),
        rect: None,
    };
    let a = b.apply(&copy).unwrap();
    assert_eq!(a.dirty, Rect::new(0, 0, 4, 4));
    assert_eq!(px(&b, 3, 2), [3, 3, 7, 255]);
    let flip = BitmapOp::Turn {
        layer: L0,
        turn: Turn::FlipHorizontal,
        rect: None,
    };
    let a = b.apply(&flip).unwrap();
    assert_eq!(a.tiles, vec![(0, 0)]);
    assert_eq!(px(&b, 0, 2), [3, 3, 7, 255]);
    assert_eq!(px(&b, 3, 2), [0, 0, 7, 255]);
    assert_eq!(b.undo_len(), 2);
    b.undo().unwrap();
    assert_eq!(px(&b, 0, 2), [0, 0, 7, 255]);
    // A rectangle outside the layer, a quarter turn of one that is not
    // square, or no channel to write: refused, and nothing changes.
    let outside = BitmapOp::Turn {
        layer: L0,
        turn: Turn::Half,
        rect: Some(Rect::new(2, 2, 4, 4)),
    };
    assert!(matches!(b.apply(&outside), Err(Error::Rect(_))));
    let row = BitmapOp::Turn {
        layer: L0,
        turn: Turn::Quarter,
        rect: Some(Rect::new(0, 0, 4, 1)),
    };
    assert!(matches!(b.apply(&row), Err(Error::Rect(_))));
    let none = BitmapOp::CopyChannel {
        layer: L0,
        from: Channel::A,
        to: Channels {
            r: false,
            g: false,
            b: false,
            a: false,
        },
        rect: None,
    };
    assert!(matches!(b.apply(&none), Err(Error::Params(_))));
    assert_eq!(b.undo_len(), 1);
    // Still as the copy left it.
    assert_eq!(px(&b, 3, 2), [3, 3, 7, 255]);
}

#[test]
fn a_failed_op_changes_nothing() {
    let mut b = big();
    let outside = BitmapOp::Fill {
        layer: L0,
        at: None,
        rect: Some(Rect::new(10_000, 0, 4, 4)),
        color: WHITE,
        tolerance: 0.0,
        channels: Channels::ALL,
    };
    assert!(matches!(b.apply(&outside), Err(Error::Rect(_))));
    let neither = BitmapOp::Fill {
        layer: L0,
        at: None,
        rect: None,
        color: WHITE,
        tolerance: 0.0,
        channels: Channels::ALL,
    };
    assert!(matches!(b.apply(&neither), Err(Error::Params(_))));
    let empty = BitmapOp::Paint {
        layer: L0,
        points: vec![],
        brush: Brush::default(),
    };
    assert!(matches!(b.apply(&empty), Err(Error::Params(_))));
    let nan = BitmapOp::Paint {
        layer: L0,
        points: vec![[f32::NAN, 0.0]],
        brush: Brush::default(),
    };
    assert!(matches!(b.apply(&nan), Err(Error::Params(_))));
    let missing = BitmapOp::Adjust {
        layer: LayerId::new(9, 0, 0),
        adjust: Adjust::Invert,
        rect: None,
        channels: Channels::ALL,
    };
    assert_eq!(b.apply(&missing), Err(Error::NoLayer(LayerId::new(9, 0, 0))));
    assert_eq!(b.undo_len(), 0);
    assert!(!b.is_edited());
}

#[test]
fn a_locked_layer_refuses_edits_with_its_reason() {
    let mut b = Bitmap::new();
    let id = LayerId::new(1, 0, 0);
    b.add_layer(
        id,
        Canvas::new(8, 8, BLACK).unwrap(),
        Some("its channels are a hypothesis".to_owned()),
    );
    let op = BitmapOp::Fill {
        layer: id,
        at: None,
        rect: Some(Rect::new(0, 0, 2, 2)),
        color: WHITE,
        tolerance: 0.0,
        channels: Channels::ALL,
    };
    match b.apply(&op) {
        Err(Error::ReadOnly(why)) => assert!(why.contains("hypothesis")),
        other => panic!("{other:?}"),
    }
    assert!(b.begin_stroke(id, Brush::default()).is_err());
}

#[test]
fn layers_edit_apart() {
    let mut b = Bitmap::new();
    for face in 0..6 {
        b.add_layer(LayerId::new(0, face, 0), Canvas::new(8, 8, BLACK).unwrap(), None);
    }
    let op = BitmapOp::Fill {
        layer: LayerId::new(0, 3, 0),
        at: None,
        rect: Some(Rect::new(0, 0, 8, 8)),
        color: WHITE,
        tolerance: 0.0,
        channels: Channels::ALL,
    };
    b.apply(&op).unwrap();
    assert_eq!(b.layer(LayerId::new(0, 3, 0)).unwrap().canvas.pixel(4, 4), WHITE);
    assert_eq!(b.layer(LayerId::new(0, 2, 0)).unwrap().canvas.pixel(4, 4), BLACK);
    assert!(b.layer(LayerId::new(0, 3, 0)).unwrap().is_edited());
    assert!(!b.layer(LayerId::new(0, 2, 0)).unwrap().is_edited());
}

#[test]
fn an_image_taken_out_comes_back_with_its_edits() {
    let mut b = Bitmap::new();
    b.add_layer(L0, Canvas::new(8, 8, BLACK).unwrap(), None);
    let added = LayerId::new(1, 0, 0);
    b.add_layer(added, Canvas::new(8, 8, BLACK).unwrap(), None);
    let fill = BitmapOp::Fill {
        layer: added,
        at: None,
        rect: Some(Rect::new(0, 0, 2, 2)),
        color: WHITE,
        tolerance: 0.0,
        channels: Channels::ALL,
    };
    b.apply(&fill).unwrap();
    let rev = b.rev();
    let out = b.remove_image(1);
    assert_eq!(out.len(), 1);
    assert!(b.layer(added).is_none());
    assert!(b.layer(L0).is_some());
    assert_ne!(b.rev(), rev);
    b.restore_layers(out);
    let l = b.layer(added).unwrap();
    assert_eq!(l.canvas.pixel(0, 0), WHITE);
    assert!(l.is_edited());
    // Its undo step still applies to it.
    assert!(b.undo().is_some());
    assert_eq!(b.layer(added).unwrap().canvas.pixel(0, 0), BLACK);
    b.clear_redo();
    assert!(!b.can_redo());
}

#[test]
fn ops_round_trip_through_json() {
    let ops = vec![
        dab([1.5, 2.5]),
        BitmapOp::Fill {
            layer: L0,
            at: Some([3, 4]),
            rect: None,
            color: [1, 2, 3, 4],
            tolerance: 0.25,
            channels: Channels::parse("rg").unwrap(),
        },
        BitmapOp::Adjust {
            layer: L0,
            adjust: Adjust::Levels {
                black: 0.1,
                white: 0.9,
                gamma: 1.5,
            },
            rect: Some(Rect::new(1, 2, 3, 4)),
            channels: Channels::ALL,
        },
        BitmapOp::Clone {
            layer: L0,
            from: Some(LayerId::new(1, 2, 3)),
            source: [1.5, 2.5],
            points: vec![[3.0, 4.0], [5.0, 6.0]],
            brush: Brush::default(),
        },
        BitmapOp::CopyChannel {
            layer: L0,
            from: Channel::Luma,
            to: Channels::parse("a").unwrap(),
            rect: None,
        },
        BitmapOp::Turn {
            layer: L0,
            turn: Turn::ThreeQuarters,
            rect: Some(Rect::new(1, 2, 3, 3)),
        },
    ];
    for op in ops {
        let text = serde_json::to_string(&op).unwrap();
        let back: BitmapOp = serde_json::from_str(&text).unwrap();
        assert_eq!(back, op, "{text}");
    }
    // A clone stroke reads the layer it paints, and the brush's defaults
    // fill what a script leaves out.
    let op: BitmapOp = serde_json::from_str(
        r#"{"op":"clone","layer":{"image":0,"face":0,"slice":0},"source":[1.0,2.0],"points":[[3.0,4.0]],"brush":{"size":4.0,"color":[1,2,3,4]}}"#,
    )
    .unwrap();
    match op {
        BitmapOp::Clone { from, brush, .. } => {
            assert_eq!(from, None);
            assert_eq!(brush.size, 4.0);
            assert_eq!(brush.color, [1, 2, 3, 4]);
            assert_eq!(brush.hardness, Brush::default().hardness);
        }
        other => panic!("{other:?}"),
    }
    let op: BitmapOp = serde_json::from_str(
        r#"{"op":"copy_channel","layer":{"image":0,"face":0,"slice":0},"from":"a","to":"rgb"}"#,
    )
    .unwrap();
    assert_eq!(
        op,
        BitmapOp::CopyChannel {
            layer: L0,
            from: Channel::A,
            to: Channels::parse("rgb").unwrap(),
            rect: None,
        }
    );
    let op: BitmapOp =
        serde_json::from_str(r#"{"op":"turn","layer":{"image":0,"face":0,"slice":0},"turn":"half"}"#)
            .unwrap();
    assert_eq!(
        op,
        BitmapOp::Turn {
            layer: L0,
            turn: Turn::Half,
            rect: None,
        }
    );
    // Defaults fill what a script leaves out.
    let op: BitmapOp = serde_json::from_str(
        r#"{"op":"fill","layer":{"image":0,"face":0,"slice":0},"rect":{"x":0,"y":0,"w":2,"h":2},"color":[9,9,9,255]}"#,
    )
    .unwrap();
    match op {
        BitmapOp::Fill {
            tolerance, channels, ..
        } => {
            assert_eq!(tolerance, 0.0);
            assert_eq!(channels, Channels::ALL);
        }
        other => panic!("{other:?}"),
    }
}
