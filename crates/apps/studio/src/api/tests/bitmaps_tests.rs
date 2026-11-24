//! The `bitmap.*` methods and the Textures tool's actions over the
//! invented textured map (docs/editor/modeling.md, slice 5): its floor
//! bitmap is 4 by 4 pixels of one `A8R8G8B8` colour, (200, 60, 20, 255).

use super::*;
use crate::app::Doc;
use crate::app::textures::{BitmapAction, PaintTool};

/// The invented textured map open, for the bitmap methods.
fn bitmaps(t: &Temp) -> (Api, App) {
    let mut app = app(t);
    let mut api = Api::new();
    let map = t.path("textured.map");
    std::fs::write(&map, fixture::fixture_with_textured_bsp()).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    (api, app)
}

fn pixels(api: &mut Api, app: &mut App, rect: Value) -> String {
    let r = call(
        api,
        app,
        m::BITMAP_PIXELS,
        json!({"bitmap": "floor", "rect": rect}),
    );
    r["rgba"].as_str().unwrap().to_owned()
}

#[test]
fn bitmaps_list_read_and_name_their_images() {
    let t = Temp::new("bitmaps-read");
    let (mut api, mut app) = bitmaps(&t);
    let list = call(&mut api, &mut app, m::BITMAP_LIST, json!({}));
    let mut names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "levels\\test\\bitmaps\\floor",
            "levels\\test\\bitmaps\\floor_lightmap"
        ]
    );
    let some = call(
        &mut api,
        &mut app,
        m::BITMAP_LIST,
        json!({"name": "lightmap"}),
    );
    assert_eq!(some.as_array().unwrap().len(), 1);
    // The end of the path names one bitmap; a part naming two is refused.
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor"}),
    );
    let img = &read["images"][0];
    assert_eq!(
        (img["width"].clone(), img["height"].clone()),
        (json!(4), json!(4))
    );
    assert_eq!(img["format"], "Argb8888");
    assert_eq!(img["layers"], json!(["0:0:0"]));
    assert_eq!(img["write"], "in_place");
    assert_eq!(img["reshape"], Value::Null);
    assert_eq!(img["locked"], Value::Null);
    for params in [
        json!({"bitmap": "test"}),
        json!({"bitmap": "nothing"}),
        json!({"bitmap": "floor", "image": 5}),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, m::BITMAP_READ, params.clone())),
            code::INVALID_PARAMS,
            "{params}"
        );
    }
    // The list now says which is open.
    let list = call(
        &mut api,
        &mut app,
        m::BITMAP_LIST,
        json!({"name": "bitmaps\\floor"}),
    );
    assert!(list.as_array().unwrap().iter().any(|b| b["open"] == true));
}

#[test]
fn bitmap_pixels_follow_paint_fill_adjust_and_undo() {
    let t = Temp::new("bitmaps-paint");
    let (mut api, mut app) = bitmaps(&t);
    // The decoded base map: one colour, B, G, R, A in the file.
    assert_eq!(
        pixels(&mut api, &mut app, json!({"x": 0, "y": 0, "w": 2, "h": 1})),
        "c83c14ffc83c14ff"
    );
    // One hard red dab on the first pixel only.
    let p = call(
        &mut api,
        &mut app,
        m::BITMAP_PAINT,
        json!({"bitmap": "floor", "points": [[0.5, 0.5]], "color": [255, 0, 0, 255], "size": 1, "hardness": 1}),
    );
    assert_eq!(p["layer"], "0:0:0");
    assert_eq!(p["tiles"], 1);
    assert_eq!(p["edited"], true);
    assert_eq!(p["dirty"], json!({"x": 0, "y": 0, "w": 2, "h": 2}));
    assert_eq!(
        pixels(&mut api, &mut app, json!([0, 0, 2, 1])),
        "ff0000ffc83c14ff"
    );
    // The document is dirty, listed, and diffed by tile.
    let docs = call(&mut api, &mut app, m::DOC_LIST, Value::Null);
    let d = docs
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["doc"] == "bitmaps")
        .unwrap();
    assert_eq!(
        (d["dirty"].clone(), d["can_undo"].clone()),
        (json!(true), json!(true))
    );
    let diff = call(&mut api, &mut app, m::DOC_DIFF, json!({"doc": "bitmaps"}));
    assert_eq!(
        diff["edited"][0]["layers"][0],
        json!({"layer": "0:0:0", "tiles": 1})
    );
    // A fill of a rectangle, then an invert of one channel of it.
    call(
        &mut api,
        &mut app,
        m::BITMAP_FILL,
        json!({"bitmap": "floor", "rect": {"x": 2, "y": 2, "w": 2, "h": 2}, "color": "#336699"}),
    );
    let corner = json!({"x": 2, "y": 2, "w": 1, "h": 1});
    assert_eq!(pixels(&mut api, &mut app, corner.clone()), "336699ff");
    call(
        &mut api,
        &mut app,
        m::BITMAP_ADJUST,
        json!({"bitmap": "floor", "invert": true, "channels": "r", "rect": [2, 2, 2, 2]}),
    );
    assert_eq!(pixels(&mut api, &mut app, corner.clone()), "cc6699ff");
    // Undo takes them back one at a time, then the dab; redo brings it.
    for _ in 0..2 {
        let u = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
        assert_eq!(u["done"], true);
    }
    assert_eq!(pixels(&mut api, &mut app, corner), "c83c14ff");
    assert_eq!(pixels(&mut api, &mut app, json!([0, 0, 1, 1])), "ff0000ff");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
    assert_eq!(pixels(&mut api, &mut app, json!([0, 0, 1, 1])), "c83c14ff");
    let docs = call(&mut api, &mut app, m::DOC_LIST, Value::Null);
    let d = docs
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["doc"] == "bitmaps")
        .unwrap();
    assert_eq!(
        (d["dirty"].clone(), d["can_redo"].clone()),
        (json!(false), json!(true))
    );
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "bitmaps"}));
    assert_eq!(pixels(&mut api, &mut app, json!([0, 0, 1, 1])), "ff0000ff");
}

#[test]
fn a_drag_in_the_canvas_is_one_undo_step() {
    let t = Temp::new("bitmaps-drag");
    let (mut api, mut app) = bitmaps(&t);
    let tag = app.resolve_bitmap("floor").unwrap();
    let layer = bitmap_edit::LayerId::new(0, 0, 0);
    app.textures.fg = [0, 255, 0, 255];
    app.textures.brush.size = 1.0;
    app.textures.brush.hardness = 1.0;
    app.dispatch(Action::Bitmap(BitmapAction::StrokeBegin { tag, layer }));
    app.dispatch(Action::Bitmap(BitmapAction::StrokeTo { x: 0.5, y: 3.5 }));
    // Mid-stroke the pixels are there, and there is nothing to undo yet.
    assert_eq!(pixels(&mut api, &mut app, json!([0, 3, 1, 1])), "00ff00ff");
    assert!(!app.textures.can_undo());
    app.dispatch(Action::Bitmap(BitmapAction::StrokeTo { x: 3.5, y: 3.5 }));
    app.dispatch(Action::Bitmap(BitmapAction::StrokeEnd));
    assert_eq!(
        pixels(&mut api, &mut app, json!([0, 3, 4, 1])),
        "00ff00ff".repeat(4)
    );
    assert!(app.textures.can_undo());
    assert!(app.tool_dirty(crate::ToolKind::Textures));
    app.dispatch(Action::UndoIn(crate::ToolKind::Textures));
    assert_eq!(
        pixels(&mut api, &mut app, json!([0, 3, 4, 1])),
        "c83c14ff".repeat(4)
    );
    assert!(!app.tool_dirty(crate::ToolKind::Textures));
}

/// A bitmap's tab is its document (docs/editor/studio-flow.md, "Shared
/// patterns"): the Textures window's Undo and Redo, and Cmd+Z in the tool,
/// take its own steps, the newest first, and the map's placements are
/// another document's stack.
#[test]
fn a_bitmaps_tab_undoes_its_own_steps_and_the_map_keeps_its_own() {
    let t = Temp::new("bitmaps-tab");
    let (mut api, mut app) = bitmaps(&t);
    app.dispatch(Action::OpenAsset {
        asset: r"tag:bitm:levels\test\bitmaps\floor".into(),
        map: None,
    });
    let Some(Doc::Tag(floor)) = app.front_doc() else {
        panic!("the bitmap has no tab: {:?}", app.tabs());
    };
    let tab = Doc::Tag(floor);
    assert_eq!(app.tool, crate::ToolKind::Textures);
    assert_eq!(app.textures.selected, Some(floor));
    assert_eq!(app.doc_can(tab), (false, false));

    let layer = bitmap_edit::LayerId::new(0, 0, 0);
    app.textures.fg = [0, 255, 0, 255];
    app.textures.brush.size = 1.0;
    app.textures.brush.hardness = 1.0;
    // One dab at the left of the last row, then a stroke across it.
    for x in [0.5, 3.5] {
        app.dispatch(Action::Bitmap(BitmapAction::StrokeBegin {
            tag: floor,
            layer,
        }));
        app.dispatch(Action::Bitmap(BitmapAction::StrokeTo { x, y: 3.5 }));
        app.dispatch(Action::Bitmap(BitmapAction::StrokeEnd));
    }
    let row = json!([0, 3, 4, 1]);
    let green = "00ff00ff".repeat(4);
    assert_eq!(pixels(&mut api, &mut app, row.clone()), green);
    assert_eq!(app.doc_can(tab), (true, false));
    assert_eq!((app.can_undo(), app.can_redo()), (true, false));
    // The map's document takes a step after the bitmap's.
    app.dispatch(Action::MovePlacement {
        block: BlockKind::NetgameFlags,
        index: 0,
        position: [1.0, 2.0, 3.0],
        ypr: None,
    });

    // The window's Undo takes the bitmap's newest step, and leaves the map's.
    app.dispatch(Action::UndoDoc(tab));
    assert_eq!(pixels(&mut api, &mut app, json!([3, 3, 1, 1])), "c83c14ff");
    assert_eq!(pixels(&mut api, &mut app, json!([0, 3, 1, 1])), "00ff00ff");
    assert!(app.session.can_undo(), "the map's step stays");
    assert_eq!(app.doc_can(tab), (true, true));
    // Cmd+Z in the tool takes the same stack.
    app.dispatch(Action::Undo);
    assert_eq!(pixels(&mut api, &mut app, row), "c83c14ff".repeat(4));
    assert_eq!(app.doc_can(tab), (false, true));
    app.dispatch(Action::RedoDoc(tab));
    assert_eq!(pixels(&mut api, &mut app, json!([0, 3, 1, 1])), "00ff00ff");
    assert_eq!(app.doc_can(tab), (true, true));
    // The map's document undoes on its own; the bitmap keeps its redo.
    app.dispatch(Action::UndoDoc(Doc::Map));
    assert!(!app.session.can_undo() && app.session.can_redo());
    assert_eq!(app.doc_can(tab), (true, true));
}

#[test]
fn bitmap_import_resamples_and_export_writes_a_png() {
    let t = Temp::new("bitmaps-png");
    let (mut api, mut app) = bitmaps(&t);
    // A 2 by 2 PNG fills the 4 by 4 layer.
    let png = bitmap_edit::encode_png(2, 2, &[10, 20, 30, 255].repeat(4)).unwrap();
    let input = t.path("in.png");
    std::fs::write(&input, png).unwrap();
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_IMPORT,
        json!({"bitmap": "floor", "path": input.display().to_string()}),
    );
    assert_eq!(r["dirty"], json!({"x": 0, "y": 0, "w": 4, "h": 4}));
    assert_eq!(
        pixels(&mut api, &mut app, json!([0, 0, 4, 4])),
        "0a141eff".repeat(16)
    );
    // Into a selection only.
    call(
        &mut api,
        &mut app,
        m::BITMAP_IMPORT,
        json!({"bitmap": "floor", "path": input.display().to_string(), "rect": [1, 1, 2, 2]}),
    );
    // Export writes the layer; mip 1 halves it.
    let out = t.path("out/floor.png");
    let x = call(
        &mut api,
        &mut app,
        m::BITMAP_EXPORT,
        json!({"bitmap": "floor", "out": out.display().to_string()}),
    );
    assert_eq!(
        (x["width"].clone(), x["height"].clone()),
        (json!(4), json!(4))
    );
    let (w, h, rgba) = bitmap_edit::decode_png(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!((w, h), (4, 4));
    assert_eq!(&rgba[..4], &[10, 20, 30, 255]);
    let half = t.path("out/floor_1.png");
    let x = call(
        &mut api,
        &mut app,
        m::BITMAP_EXPORT,
        json!({"bitmap": "floor", "out": half.display().to_string(), "mip": 1}),
    );
    assert_eq!(
        (x["width"].clone(), x["height"].clone()),
        (json!(2), json!(2))
    );
    assert_eq!(
        bitmap_edit::decode_png(&std::fs::read(&half).unwrap())
            .unwrap()
            .0,
        2
    );
    // Not a PNG, not there, not a rectangle in the layer.
    std::fs::write(t.path("bad.png"), b"nope").unwrap();
    for (path, rect) in [
        ("bad.png", Value::Null),
        ("missing.png", Value::Null),
        ("in.png", json!([3, 3, 4, 4])),
    ] {
        let e = api
            .call(
                &mut app,
                m::BITMAP_IMPORT,
                json!({"bitmap": "floor", "path": t.path(path).display().to_string(), "rect": rect}),
            )
            .unwrap_err();
        assert!(
            e.code == code::INVALID_PARAMS || e.code == code::FAILED,
            "{path}: {e}"
        );
    }
}

#[test]
fn bitmap_export_is_refused_in_a_git_checkout() {
    let t = Temp::new("bitmaps-git");
    let (mut api, mut app) = bitmaps(&t);
    let repo = t.path("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let out = repo.join("floor.png");
    let e = api
        .call(
            &mut app,
            m::BITMAP_EXPORT,
            json!({"bitmap": "floor", "out": out.display().to_string()}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(!out.exists());
    // The window's own export action refuses the same.
    let tag = app.resolve_bitmap("floor").unwrap();
    let r = app.perform(
        Action::Bitmap(BitmapAction::Export {
            tag,
            layer: bitmap_edit::LayerId::new(0, 0, 0),
            mip: 0,
            path: out.clone(),
        }),
        false,
    );
    let r = app.perform(
        Action::Bitmap(BitmapAction::Export {
            tag,
            layer: bitmap_edit::LayerId::new(0, 0, 0),
            mip: 0,
            path: out.clone(),
        }),
        false,
    );
    assert!(r.is_err());
    assert!(!out.exists());
}

/// One row of an image of the floor bitmap, as `bitmap.pixels` gives it.
fn row(api: &mut Api, app: &mut App, image: usize, y: usize) -> String {
    let r = call(
        api,
        app,
        m::BITMAP_PIXELS,
        json!({"bitmap": "floor", "image": image, "rect": [0, y, 4, 1]}),
    );
    r["rgba"].as_str().unwrap().to_owned()
}

#[test]
fn bitmap_paint_draws_a_line_and_clones() {
    let t = Temp::new("bitmaps-clone");
    let (mut api, mut app) = bitmaps(&t);
    let (floor, red) = ("c83c14ff", "ff0000ff");
    // Two points are a line from one to the other: a 1 pixel brush along
    // the second row, and no other pixel.
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_PAINT,
        json!({"bitmap": "floor", "points": [[0.5, 1.5], [3.5, 1.5]], "color": [255, 0, 0, 255], "size": 1, "hardness": 1}),
    );
    assert_eq!(r["dirty"], json!({"x": 0, "y": 0, "w": 4, "h": 3}));
    for y in 0..4 {
        let want = if y == 1 { red } else { floor };
        assert_eq!(row(&mut api, &mut app, 0, y), want.repeat(4), "row {y}");
    }
    // A clone stamp takes no colour: each stamp is the pixel at the
    // offset, here the row below it.
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_PAINT,
        json!({"bitmap": "floor", "source": [0.5, 1.5], "points": [[0.5, 0.5], [3.5, 0.5]], "size": 1, "hardness": 1}),
    );
    assert_eq!(r["dirty"], json!({"x": 0, "y": 0, "w": 4, "h": 2}));
    assert_eq!(row(&mut api, &mut app, 0, 0), red.repeat(4));
    // From another image: a new, transparent image takes a copy of the row.
    call(
        &mut api,
        &mut app,
        m::BITMAP_IMAGE_ADD,
        json!({"bitmap": "floor"}),
    );
    call(
        &mut api,
        &mut app,
        m::BITMAP_PAINT,
        json!({"bitmap": "floor", "image": 1, "from": {"image": 0}, "source": [0.5, 1.5], "points": [[0.5, 1.5], [3.5, 1.5]], "size": 1, "hardness": 1}),
    );
    assert_eq!(row(&mut api, &mut app, 1, 1), red.repeat(4));
    assert_eq!(row(&mut api, &mut app, 1, 0), "00000000".repeat(4));
    // A stroke that changes no pixel is no step.
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_PAINT,
        json!({"bitmap": "floor", "source": [0.5, 1.5], "points": [[0.5, 1.5]], "size": 2, "hardness": 1}),
    );
    assert_eq!(r["dirty"], Value::Null);
    // Undo takes the three back, the last first.
    for (image, y, was) in [(1, 1, "00000000"), (0, 0, floor), (0, 1, floor)] {
        call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
        assert_eq!(row(&mut api, &mut app, image, y), was.repeat(4));
    }
    // A colour unless there is a source, never both erase and a source,
    // and a source and a layer that are there.
    for params in [
        json!({"bitmap": "floor", "points": [[1, 1]], "size": 2}),
        json!({"bitmap": "floor", "points": [[1, 1]], "size": 2, "source": [0, 0], "erase": true}),
        json!({"bitmap": "floor", "points": [[1, 1]], "size": 2, "source": 3}),
        json!({"bitmap": "floor", "points": [[1, 1]], "size": 2, "source": [0, 0], "from": 4}),
        json!({"bitmap": "floor", "points": [[1, 1]], "size": 2, "source": [0, 0], "from": {"image": 9}}),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, m::BITMAP_PAINT, params.clone())),
            code::INVALID_PARAMS,
            "{params}"
        );
    }
}

/// The window's Shift+click line and clone drag are the strokes
/// `bitmap.paint` makes, each one undo step.
#[test]
fn the_windows_line_and_clone_drag_are_the_api_strokes() {
    let t = Temp::new("bitmaps-window-tools");
    let (mut api, mut app) = bitmaps(&t);
    let tag = app.resolve_bitmap("floor").unwrap();
    let layer = bitmap_edit::LayerId::new(0, 0, 0);
    app.textures.tool = PaintTool::Brush;
    app.textures.fg = [255, 0, 0, 255];
    app.textures.brush.size = 1.0;
    app.textures.brush.hardness = 1.0;
    app.dispatch(Action::Bitmap(BitmapAction::Line {
        tag,
        layer,
        from: [0.5, 1.5],
        to: [3.5, 1.5],
    }));
    assert_eq!(row(&mut api, &mut app, 0, 1), "ff0000ff".repeat(4));
    // The clone tool has no source yet: its line is refused.
    app.textures.tool = PaintTool::Clone;
    let e = app.perform(
        Action::Bitmap(BitmapAction::Line {
            tag,
            layer,
            from: [0.5, 0.5],
            to: [3.5, 0.5],
        }),
        false,
    );
    assert!(e.is_err());
    assert_eq!(row(&mut api, &mut app, 0, 0), "c83c14ff".repeat(4));
    // A source at pixel (0, 1) and a drag along the row above: each stamp
    // copies the pixel below it.
    app.textures.clone_source = Some((layer, [0.5, 1.5]));
    app.dispatch(Action::Bitmap(BitmapAction::CloneBegin {
        tag,
        layer,
        from: None,
        at: [0.5, 1.5],
    }));
    app.dispatch(Action::Bitmap(BitmapAction::StrokeTo { x: 0.5, y: 0.5 }));
    app.dispatch(Action::Bitmap(BitmapAction::StrokeTo { x: 3.5, y: 0.5 }));
    app.dispatch(Action::Bitmap(BitmapAction::StrokeEnd));
    assert_eq!(row(&mut api, &mut app, 0, 0), "ff0000ff".repeat(4));
    // With the clone tool the line copies too: the first row onto the last.
    app.dispatch(Action::Bitmap(BitmapAction::Line {
        tag,
        layer,
        from: [0.5, 3.5],
        to: [3.5, 3.5],
    }));
    assert_eq!(row(&mut api, &mut app, 0, 3), "ff0000ff".repeat(4));
    // One undo step each, the last first.
    for (y, was) in [(3, "c83c14ff"), (0, "c83c14ff"), (1, "c83c14ff")] {
        app.dispatch(Action::UndoIn(crate::ToolKind::Textures));
        assert_eq!(row(&mut api, &mut app, 0, y), was.repeat(4));
    }
    assert!(!app.tool_dirty(crate::ToolKind::Textures));
    // The source stays while its layer is shown, and is forgotten once
    // another image is.
    app.textures.clone_source = Some((layer, [0.5, 1.5]));
    app.dispatch(Action::Bitmap(BitmapAction::Select { tag, image: 0 }));
    assert_eq!(app.textures.clone_source, Some((layer, [0.5, 1.5])));
    call(
        &mut api,
        &mut app,
        m::BITMAP_IMAGE_ADD,
        json!({"bitmap": "floor"}),
    );
    app.dispatch(Action::Bitmap(BitmapAction::Select { tag, image: 1 }));
    assert_eq!(app.textures.clone_source, None);
}

#[test]
fn bitmap_channel_copy_and_turn_pixels() {
    let t = Temp::new("bitmaps-turn");
    let (mut api, mut app) = bitmaps(&t);
    let (floor, red) = ("c83c14ff", "ff0000ff");
    // A red second row, then turned: the row goes to the third.
    call(
        &mut api,
        &mut app,
        m::BITMAP_PAINT,
        json!({"bitmap": "floor", "points": [[0.5, 1.5], [3.5, 1.5]], "color": [255, 0, 0, 255], "size": 1, "hardness": 1}),
    );
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_TURN,
        json!({"bitmap": "floor", "by": "flip_vertical"}),
    );
    assert_eq!(r["dirty"], json!({"x": 0, "y": 0, "w": 4, "h": 4}));
    assert_eq!(r["tiles"], 1);
    for y in 0..4 {
        let want = if y == 2 { red } else { floor };
        assert_eq!(row(&mut api, &mut app, 0, y), want.repeat(4), "row {y}");
    }
    // A half turn puts it back; a quarter turn clockwise takes the row to
    // the third column.
    call(
        &mut api,
        &mut app,
        m::BITMAP_TURN,
        json!({"bitmap": "floor", "by": "half"}),
    );
    assert_eq!(row(&mut api, &mut app, 0, 1), red.repeat(4));
    call(
        &mut api,
        &mut app,
        m::BITMAP_TURN,
        json!({"bitmap": "floor", "by": "quarter"}),
    );
    let column = format!("{floor}{floor}{red}{floor}");
    for y in 0..4 {
        assert_eq!(row(&mut api, &mut app, 0, y), column, "row {y}");
    }
    // In a rectangle only: the top two rows flip, so the column moves one
    // left there and stays in the rows below.
    call(
        &mut api,
        &mut app,
        m::BITMAP_TURN,
        json!({"bitmap": "floor", "by": "flip_horizontal", "rect": [0, 0, 4, 2]}),
    );
    let flipped = format!("{floor}{red}{floor}{floor}");
    for (y, want) in [(0, &flipped), (1, &flipped), (2, &column), (3, &column)] {
        assert_eq!(row(&mut api, &mut app, 0, y), *want, "row {y}");
    }
    // Green into alpha and blue: the floor keeps its red and green.
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_CHANNEL_COPY,
        json!({"bitmap": "floor", "from": "g", "to": "ab"}),
    );
    assert_eq!(r["dirty"], json!({"x": 0, "y": 0, "w": 4, "h": 4}));
    assert_eq!(
        row(&mut api, &mut app, 0, 0),
        "c83c3c3cff000000c83c3c3cc83c3c3c"
    );
    // The luma into red, of one pixel.
    call(
        &mut api,
        &mut app,
        m::BITMAP_CHANNEL_COPY,
        json!({"bitmap": "floor", "from": "luma", "to": "r", "rect": [0, 0, 1, 1]}),
    );
    assert_eq!(
        row(&mut api, &mut app, 0, 0),
        "663c3c3cff000000c83c3c3cc83c3c3c"
    );
    // A name that is not a turn or a channel, a quarter turn of a
    // rectangle that is not square, a rectangle off the layer and no
    // channel to write are refused, each naming the parameter.
    for (method, params) in [
        (m::BITMAP_TURN, json!({"bitmap": "floor", "by": "flip"})),
        (m::BITMAP_TURN, json!({"bitmap": "floor"})),
        (
            m::BITMAP_TURN,
            json!({"bitmap": "floor", "by": "quarter", "rect": [0, 0, 4, 2]}),
        ),
        (
            m::BITMAP_TURN,
            json!({"bitmap": "floor", "by": "half", "rect": [2, 2, 4, 4]}),
        ),
        (
            m::BITMAP_CHANNEL_COPY,
            json!({"bitmap": "floor", "from": "y", "to": "a"}),
        ),
        (
            m::BITMAP_CHANNEL_COPY,
            json!({"bitmap": "floor", "from": "a"}),
        ),
        (
            m::BITMAP_CHANNEL_COPY,
            json!({"bitmap": "floor", "from": "a", "to": "q"}),
        ),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, method, params.clone())),
            code::INVALID_PARAMS,
            "{method} {params}"
        );
    }
    // Seven edits, undone the last first, leave the floor as it was read.
    for _ in 0..7 {
        assert_eq!(
            call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}))["done"],
            true
        );
    }
    for y in 0..4 {
        assert_eq!(row(&mut api, &mut app, 0, y), floor.repeat(4), "row {y}");
    }
    let docs = call(&mut api, &mut app, m::DOC_LIST, Value::Null);
    let d = docs
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["doc"] == "bitmaps")
        .unwrap();
    assert_eq!(
        (d["dirty"].clone(), d["can_undo"].clone()),
        (json!(false), json!(false))
    );
}

#[test]
fn bitmap_edits_are_kept_until_saved_and_data_formats_are_read_only() {
    let t = Temp::new("bitmaps-readonly");
    let (mut api, mut app) = bitmaps(&t);
    call(
        &mut api,
        &mut app,
        m::BITMAP_FILL,
        json!({"bitmap": "floor", "rect": [0, 0, 1, 1], "color": [1, 2, 3]}),
    );
    // Closing would drop them: refused unless discarded.
    assert_eq!(
        code_of(api.call(&mut app, m::DOC_CLOSE, json!({"doc": "bitmaps"}))),
        code::UNSAVED
    );
    call(
        &mut api,
        &mut app,
        m::DOC_CLOSE,
        json!({"doc": "bitmaps", "discard": true}),
    );
    assert_eq!(pixels(&mut api, &mut app, json!([0, 0, 1, 1])), "c83c14ff");
    // The lightmap page is a P8 image: shown, not edited.
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor_lightmap"}),
    );
    assert_eq!(read["images"][0]["write"], "none");
    assert!(read["images"][0]["locked"].as_str().unwrap().contains("P8"));
    let e = api
        .call(
            &mut app,
            m::BITMAP_FILL,
            json!({"bitmap": "floor_lightmap", "rect": [0, 0, 1, 1], "color": [1, 2, 3]}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    // Bad params are named.
    for params in [
        json!({"bitmap": "floor", "points": [[1, 1]], "size": 0, "color": [1, 2, 3]}),
        json!({"bitmap": "floor", "points": [], "size": 2, "color": [1, 2, 3]}),
        json!({"bitmap": "floor", "points": [[1, 1]], "size": 2}),
        json!({"bitmap": "floor", "points": [[1, 1]], "size": 2, "color": "red"}),
        json!({"bitmap": "floor", "points": [[1, 1]], "size": 2, "color": [1, 2, 3], "channels": "q"}),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, m::BITMAP_PAINT, params.clone())),
            code::INVALID_PARAMS,
            "{params}"
        );
    }
    let adjust = json!({"bitmap": "floor", "invert": true, "levels": {}});
    assert_eq!(
        code_of(api.call(&mut app, m::BITMAP_ADJUST, adjust)),
        code::INVALID_PARAMS
    );
    let big = json!({"bitmap": "floor", "rect": [0, 0, 1000, 1000]});
    assert_eq!(
        code_of(api.call(&mut app, m::BITMAP_PIXELS, big)),
        code::REFUSED
    );
}

/// The bytes of the map file that differ, as `(start, end)` runs, leaving
/// out the header checksum.
fn changed(a: &[u8], b: &[u8]) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if x != y && !(0x2d0..0x2d4).contains(&i) {
            match runs.last_mut() {
                Some(r) if r.1 == i => r.1 = i + 1,
                _ => runs.push((i, i + 1)),
            }
        }
    }
    runs
}

#[test]
fn a_painted_corner_saves_as_its_texels_only_and_the_tool_is_clean_after() {
    let t = Temp::new("bitmaps-write");
    let (mut api, mut app) = bitmaps(&t);
    let src = std::fs::read(t.path("textured.map")).unwrap();
    call(
        &mut api,
        &mut app,
        m::BITMAP_FILL,
        json!({"bitmap": "floor", "rect": [0, 0, 1, 1], "color": [1, 2, 3]}),
    );
    // The bytes the encoder will write, before anything is: texel (0, 0) of
    // the 4x4 swizzled ARGB8888 image, bytes B, G, R, A.
    let spans = call(
        &mut api,
        &mut app,
        m::BITMAP_SPANS,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(spans["write"], "in_place");
    assert_eq!(spans["bytes"], 4);
    assert_eq!(
        spans["spans"],
        json!([{"lod": 0, "offset": 0, "len": 4, "bytes": "030201ff"}])
    );
    let docs = call(&mut api, &mut app, m::DOC_LIST, json!({}));
    let d = docs
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["doc"] == "bitmaps")
        .unwrap();
    assert_eq!(d["dirty"], true);
    let out = t.path("painted.map");
    let saved = call(
        &mut api,
        &mut app,
        m::DOC_SAVE_AS,
        json!({"doc": "bitmaps", "path": out.display().to_string()}),
    );
    assert_eq!(saved["saved"], true);
    let bytes = std::fs::read(&out).unwrap();
    let at = fixture::TEX_BASE_FILE;
    assert_eq!(bytes.len(), src.len());
    // (the alpha byte was 255 already)
    assert_eq!(changed(&src, &bytes), [(at, at + 3)]);
    assert_eq!(&bytes[at..at + 4], &[3, 2, 1, 255]);
    // Saved: nothing left unsaved, and a second edit is unsaved again.
    let docs = call(&mut api, &mut app, m::DOC_LIST, json!({}));
    let d = docs
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["doc"] == "bitmaps")
        .unwrap();
    assert_eq!(d["dirty"], false);
    call(
        &mut api,
        &mut app,
        m::BITMAP_FILL,
        json!({"bitmap": "floor", "rect": [3, 3, 1, 1], "color": [9, 9, 9]}),
    );
    let docs = call(&mut api, &mut app, m::DOC_LIST, json!({}));
    let d = docs
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["doc"] == "bitmaps")
        .unwrap();
    assert_eq!(d["dirty"], true);
    // Both edits are in the next file: the changes are recomputed from the
    // source each time.
    let again = t.path("painted2.map");
    call(
        &mut api,
        &mut app,
        m::DOC_SAVE_AS,
        json!({"doc": "bitmaps", "path": again.display().to_string()}),
    );
    let bytes = std::fs::read(&again).unwrap();
    // (3, 3) is the last texel of a 4x4 Morton block: index 15.
    assert_eq!(changed(&src, &bytes), [(at, at + 3), (at + 60, at + 63)]);
    // Undoing everything saves the source again.
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
    let clean = t.path("clean.map");
    call(
        &mut api,
        &mut app,
        m::DOC_SAVE_AS,
        json!({"doc": "bitmaps", "path": clean.display().to_string()}),
    );
    assert_eq!(std::fs::read(&clean).unwrap(), src);
}

#[test]
fn auto_format_keeps_what_holds_the_picture_and_leaves_dxt_for_odd_sizes() {
    let t = Temp::new("bitmaps-auto-format");
    let (mut api, mut app) = bitmaps(&t);
    // The floor is opaque A8R8G8B8: that holds it, so auto keeps it.
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_RESHAPE,
        json!({"bitmap": "floor", "width": 8, "height": 8, "format": "auto"}),
    );
    assert_eq!(r["reshape"]["format"], "Argb8888");
    assert_eq!(r["reshape"]["width"], 8);
    // Asked to be DXT1 first, an odd size cannot stay DXT1: auto takes the
    // opaque linear format.
    call(
        &mut api,
        &mut app,
        m::BITMAP_RESHAPE,
        json!({"bitmap": "floor", "format": "dxt1"}),
    );
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_RESHAPE,
        json!({"bitmap": "floor", "width": 7, "height": 7, "format": "AUTO"}),
    );
    assert_eq!(r["reshape"]["format"], "Xrgb8888");
    assert_eq!(r["reshape"]["width"], 7);
    // The same through the resize method: the format asked for now,
    // X8R8G8B8, holds an opaque picture at a power of two size too.
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_IMAGE_RESIZE,
        json!({"bitmap": "floor", "size": [8, 8], "format": "auto"}),
    );
    assert_eq!(r["reshape"]["format"], "Xrgb8888");
}

#[test]
fn a_reshaped_image_is_appended_and_reads_back_through_the_decoder() {
    let t = Temp::new("bitmaps-reshape");
    let mut app = app(&t);
    let mut api = Api::new();
    let map = t.path("textured.map");
    // The variant whose metadata ends the file, which takes new raw blocks.
    std::fs::write(&map, fixture::fixture_with_textured_bsp_last()).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    call(
        &mut api,
        &mut app,
        m::BITMAP_RESHAPE,
        json!({"bitmap": "floor", "width": 8, "height": 8, "format": "DXT1"}),
    );
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(read["images"][0]["reshape"]["width"], 8);
    // Refused now: a cube-style or data format is not written.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::BITMAP_RESHAPE,
            json!({"bitmap": "floor", "format": "p8"})
        )),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::BITMAP_RESHAPE,
            json!({"bitmap": "floor", "width": 7, "format": "dxt1"})
        )),
        code::INVALID_PARAMS
    );
    let out = t.path("reshaped.map");
    call(
        &mut api,
        &mut app,
        m::DOC_SAVE_AS,
        json!({"doc": "bitmaps", "path": out.display().to_string()}),
    );
    let bytes = std::fs::read(&out).unwrap();
    let mut m = ::map::Map::from_reader(std::io::Cursor::new(bytes.clone())).unwrap();
    let tag = ::tags::DecodedTag::read(&mut m, fixture::id(7)).unwrap();
    let ::tags::TagData::Bitmap(b) = tag.data else {
        panic!("a bitmap")
    };
    let img = &b.images[0];
    assert_eq!(
        (img.dimensions[0], img.dimensions[1], img.format),
        (8, 8, ::tags::PixelFormat::Dxt1)
    );
    let mut files = ::assets::ResourceFiles::new();
    files.set_bytes(::map::ResourceFile::Local, std::sync::Arc::from(bytes));
    let d = ::assets::decode_image(img, &mut files).unwrap();
    assert!(d.size_matches());
    // 8x8 down to one block; the base is the floor's one colour within DXT1's
    // 5:6:5 error.
    assert_eq!((d.levels[0].width, d.levels[0].height), (8, 8));
    for p in d.levels[0].rgba.chunks(4) {
        assert!(
            p[0].abs_diff(200) <= 8
                && p[1].abs_diff(60) <= 4
                && p[2].abs_diff(20) <= 8
                && p[3] == 255,
            "{p:?}"
        );
    }
}

/// The invented textured map whose metadata ends the file (it takes new
/// raw blocks and appended metadata), open, and its bytes.
fn bitmaps_last(t: &Temp) -> (Api, App, Vec<u8>) {
    let mut app = app(t);
    let mut api = Api::new();
    let map = t.path("textured.map");
    let src = fixture::fixture_with_textured_bsp_last();
    std::fs::write(&map, &src).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    (api, app, src)
}

/// Saves the bitmaps document as `name` and reads it back.
fn save_as(api: &mut Api, app: &mut App, t: &Temp, name: &str) -> Vec<u8> {
    let out = t.path(name);
    let saved = call(
        api,
        app,
        m::DOC_SAVE_AS,
        json!({"doc": "bitmaps", "path": out.display().to_string()}),
    );
    assert_eq!(saved["saved"], true);
    std::fs::read(&out).unwrap()
}

/// The floor bitmap of a map's bytes, decoded, and its images' mip 0
/// pixels as RGBA.
fn floor_of(bytes: &[u8]) -> (::tags::Bitmap, Vec<Vec<u8>>) {
    let mut m = ::map::Map::from_reader(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let tag = ::tags::DecodedTag::read(&mut m, fixture::id(7)).unwrap();
    let ::tags::TagData::Bitmap(b) = tag.data else {
        panic!("a bitmap")
    };
    let mut files = ::assets::ResourceFiles::new();
    files.set_bytes(
        ::map::ResourceFile::Local,
        std::sync::Arc::from(bytes.to_vec()),
    );
    let pixels = b
        .images
        .iter()
        .map(|img| {
            let d = ::assets::decode_image(img, &mut files).unwrap();
            let l = d.levels.iter().find(|l| l.level == 0).unwrap();
            l.rgba[..(l.width * l.height * 4) as usize].to_vec()
        })
        .collect();
    (*b, pixels)
}

/// Every tag of `src` but the floor bitmap has the same bytes and index
/// entry in `out`.
fn other_tags_kept(src: &[u8], out: &[u8]) {
    let mut a = ::map::Map::from_reader(std::io::Cursor::new(src.to_vec())).unwrap();
    let mut b = ::map::Map::from_reader(std::io::Cursor::new(out.to_vec())).unwrap();
    b.validate_checksum().unwrap();
    let ids: Vec<_> = a
        .tags()
        .iter()
        .filter(|t| t.location.is_some() && t.id != fixture::id(7))
        .map(|t| t.id)
        .collect();
    assert!(!ids.is_empty());
    for id in ids {
        assert_eq!(
            a.read_tag_bytes(id).unwrap(),
            b.read_tag_bytes(id).unwrap(),
            "tag {:#010x}",
            id.0
        );
        let (x, y) = (a.tag(id).unwrap(), b.tag(id).unwrap());
        assert_eq!(
            (x.index_address, x.index_size),
            (y.index_address, y.index_size)
        );
    }
}

#[test]
fn an_added_image_saves_by_moving_the_tag_and_keeps_everything_else() {
    let t = Temp::new("bitmaps-add");
    let (mut api, mut app, src) = bitmaps_last(&t);
    let (before, before_px) = floor_of(&src);
    assert_eq!(before.images.len(), 1);
    // An 8 by 8 image after the floor's one, shaped like it.
    let add = call(
        &mut api,
        &mut app,
        m::BITMAP_IMAGE_ADD,
        json!({"bitmap": "floor", "size": [8, 8]}),
    );
    assert_eq!(
        (
            add["image"].clone(),
            add["width"].clone(),
            add["images"].clone()
        ),
        (json!(1), json!(8), json!(2))
    );
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(read["grown"], true);
    assert_eq!(read["movable"], Value::Null);
    assert_eq!(read["images"][1]["write"], "new");
    assert_eq!(read["images"][1]["added_from"], 0);
    assert_eq!(read["images"][1]["layers"], json!(["1:0:0"]));
    // Transparent until painted; a fill makes it one colour.
    let corner = json!({"bitmap": "floor", "image": 1, "rect": [0, 0, 1, 1]});
    assert_eq!(
        call(&mut api, &mut app, m::BITMAP_PIXELS, corner)["rgba"],
        "00000000"
    );
    call(
        &mut api,
        &mut app,
        m::BITMAP_FILL,
        json!({"bitmap": "floor", "image": 1, "rect": [0, 0, 8, 8], "color": [10, 200, 30, 255]}),
    );
    let out = save_as(&mut api, &mut app, &t, "added.map");
    // Read back: two images; the first keeps its pixels, the second is
    // the fill; every other tag is byte for byte the source's.
    let (after, px) = floor_of(&out);
    assert_eq!(after.images.len(), 2);
    assert_eq!(px[0], before_px[0]);
    assert_eq!(
        (after.images[1].dimensions[0], after.images[1].dimensions[1]),
        (8, 8)
    );
    assert_eq!(after.images[1].format, before.images[0].format);
    assert_eq!(px[1], [10, 200, 30, 255].repeat(64));
    assert!(after.images[1].low_detail.is_none());
    other_tags_kept(&src, &out);
    // The tag moved past the source's metadata.
    let m = ::map::Map::from_reader(std::io::Cursor::new(out.clone())).unwrap();
    let old = ::map::Map::from_reader(std::io::Cursor::new(src.clone())).unwrap();
    let was = old.tag(fixture::id(7)).unwrap();
    let floor = m.tag(fixture::id(7)).unwrap();
    let end = old.metadata_space().unwrap();
    assert!(floor.index_address >= end.virtual_base + end.size);
    assert!(floor.index_size > was.index_size);
    // Undo the fill and the image: the save is the source again.
    for _ in 0..2 {
        let u = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
        assert_eq!(u["done"], true);
    }
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(read["images"].as_array().unwrap().len(), 1);
    assert_eq!(read["grown"], false);
    assert_eq!(save_as(&mut api, &mut app, &t, "clean.map"), src);
    // Redo brings the image back with its fill.
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "bitmaps"}));
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "bitmaps"}));
    let last = json!({"bitmap": "floor", "image": 1, "rect": [7, 7, 1, 1]});
    assert_eq!(
        call(&mut api, &mut app, m::BITMAP_PIXELS, last)["rgba"],
        "0ac81eff"
    );
    // A copy of an image the bitmap does not have, or an empty size, is
    // refused now.
    for params in [
        json!({"bitmap": "floor", "from": 5}),
        json!({"bitmap": "floor", "size": [0, 8]}),
    ] {
        assert!(
            api.call(&mut app, m::BITMAP_IMAGE_ADD, params.clone())
                .is_err(),
            "{params}"
        );
    }
}

#[test]
fn a_resized_image_is_one_undo_step_and_saves_at_its_new_size() {
    let t = Temp::new("bitmaps-resize");
    let (mut api, mut app, src) = bitmaps_last(&t);
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_IMAGE_RESIZE,
        json!({"bitmap": "floor", "size": [8, 8]}),
    );
    assert_eq!(r["reshape"]["width"], 8);
    assert_eq!(r["edited"], true);
    // Undo takes it back, redo brings it.
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(read["images"][0]["reshape"], Value::Null);
    assert_eq!(save_as(&mut api, &mut app, &t, "same.map"), src);
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "bitmaps"}));
    let out = save_as(&mut api, &mut app, &t, "resized.map");
    let (b, px) = floor_of(&out);
    assert_eq!(
        (b.images[0].dimensions[0], b.images[0].dimensions[1]),
        (8, 8)
    );
    // Upsampled from one colour: that colour everywhere.
    assert_eq!(px[0], fixture::TEX_BASE_RGBA.repeat(64));
    // A resize keeps the tag where it is: nothing was added.
    other_tags_kept(&src, &out);
    let m = ::map::Map::from_reader(std::io::Cursor::new(out)).unwrap();
    let old = ::map::Map::from_reader(std::io::Cursor::new(src)).unwrap();
    assert_eq!(
        m.tag(fixture::id(7)).unwrap().index_address,
        old.tag(fixture::id(7)).unwrap().index_address
    );
    assert_eq!(
        code_of(api.call(&mut app, m::BITMAP_IMAGE_RESIZE, json!({"bitmap": "floor"}))),
        code::INVALID_PARAMS
    );
}

#[test]
fn an_added_sequence_names_the_added_image_and_reads_back() {
    let t = Temp::new("bitmaps-sequence");
    let (mut api, mut app, src) = bitmaps_last(&t);
    call(
        &mut api,
        &mut app,
        m::BITMAP_IMAGE_ADD,
        json!({"bitmap": "floor"}),
    );
    let s = call(
        &mut api,
        &mut app,
        m::BITMAP_SEQUENCE_ADD,
        json!({"bitmap": "floor", "first": 1, "label": "new icon"}),
    );
    assert_eq!(s["sequence"], 0);
    assert_eq!(
        s["sequences"][0],
        json!({"sequence": 0, "label": "new icon", "first": 1, "count": 1, "sprites": 0, "added": true})
    );
    // Images past the bitmap's are refused.
    assert!(
        api.call(
            &mut app,
            m::BITMAP_SEQUENCE_ADD,
            json!({"bitmap": "floor", "first": 1, "count": 2})
        )
        .is_err()
    );
    let out = save_as(&mut api, &mut app, &t, "sequence.map");
    let (b, _) = floor_of(&out);
    assert_eq!(b.images.len(), 2);
    assert_eq!(b.sequences.len(), 1);
    let w = &b.sequences[0].wire;
    assert_eq!((w.first_image, w.image_count), (1, 1));
    other_tags_kept(&src, &out);
    // Undoing the sequence leaves the image; undoing that, the source.
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
    let (b, _) = floor_of(&save_as(&mut api, &mut app, &t, "image-only.map"));
    assert_eq!((b.images.len(), b.sequences.len()), (2, 0));
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
    assert_eq!(save_as(&mut api, &mut app, &t, "clean.map"), src);
}

#[test]
fn sprites_are_added_edited_removed_undone_and_saved() {
    let t = Temp::new("bitmaps-sprites");
    let (mut api, mut app, src) = bitmaps_last(&t);
    call(
        &mut api,
        &mut app,
        m::BITMAP_SEQUENCE_ADD,
        json!({"bitmap": "floor", "first": 0, "label": "icons"}),
    );
    // A new sprite covers the sequence's first image.
    let a = call(
        &mut api,
        &mut app,
        m::BITMAP_SPRITE_ADD,
        json!({"bitmap": "floor", "sequence": 0, "bounds": [0.0, 0.5, 0.0, 0.5]}),
    );
    assert_eq!(a["sprite"], 0);
    assert_eq!(a["sprites"][0]["image"], 0);
    assert_eq!(a["sprites"][0]["right"], 0.5);
    // The second is a copy of the first, then set.
    let b = call(
        &mut api,
        &mut app,
        m::BITMAP_SPRITE_ADD,
        json!({"bitmap": "floor", "sequence": 0, "registration": [0.25, 0.75]}),
    );
    assert_eq!(b["sprite"], 1);
    assert_eq!(b["sprites"][1]["right"], 0.5);
    assert_eq!(b["sprites"][1]["registration"], json!([0.25, 0.75]));
    let set = call(
        &mut api,
        &mut app,
        m::BITMAP_SPRITE_SET,
        json!({"bitmap": "floor", "sequence": 0, "sprite": 1, "bounds": [0.5, 1.0, 0.5, 1.0]}),
    );
    assert_eq!(set["sprites"][1]["left"], 0.5);
    assert_eq!(set["sprites"][1]["registration"], json!([0.25, 0.75]));
    let list = call(
        &mut api,
        &mut app,
        m::BITMAP_SPRITES,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(list["sequences"][0]["label"], "icons");
    assert_eq!(list["sequences"][0]["sprites"].as_array().unwrap().len(), 2);
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(read["sequences"][0]["sprites"], 2);

    // Refusals change nothing: an inverted pair, an image the bitmap
    // lacks, a sprite that is not there, a label too long.
    for (method, params) in [
        (
            m::BITMAP_SPRITE_SET,
            json!({"sequence": 0, "sprite": 0, "bounds": [0.9, 0.1, 0.0, 1.0]}),
        ),
        (
            m::BITMAP_SPRITE_SET,
            json!({"sequence": 0, "sprite": 0, "image": 5}),
        ),
        (
            m::BITMAP_SPRITE_SET,
            json!({"sequence": 0, "sprite": 7, "image": 0}),
        ),
        (m::BITMAP_SPRITE_REMOVE, json!({"sequence": 0, "sprite": 7})),
        (m::BITMAP_SPRITE_ADD, json!({"sequence": 3})),
        (
            m::BITMAP_SEQUENCE_SET,
            json!({"sequence": 0, "label": "x".repeat(32)}),
        ),
        (m::BITMAP_SEQUENCE_SET, json!({"sequence": 0, "count": 9})),
    ] {
        let mut params = params;
        params["bitmap"] = json!("floor");
        assert!(api.call(&mut app, method, params).is_err(), "{method}");
    }
    let list2 = call(
        &mut api,
        &mut app,
        m::BITMAP_SPRITES,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(list, list2);

    let s = call(
        &mut api,
        &mut app,
        m::BITMAP_SEQUENCE_SET,
        json!({"bitmap": "floor", "sequence": 0, "label": "renamed"}),
    );
    assert_eq!(s["sequences"][0]["label"], "renamed");
    assert_eq!(s["sequences"][0]["sprites"], 2);
    let r = call(
        &mut api,
        &mut app,
        m::BITMAP_SPRITE_REMOVE,
        json!({"bitmap": "floor", "sequence": 0, "sprite": 0}),
    );
    assert_eq!(r["sprites"].as_array().unwrap().len(), 1);
    assert_eq!(r["sprites"][0]["left"], 0.5);

    let out = save_as(&mut api, &mut app, &t, "sprites.map");
    let (bm, _) = floor_of(&out);
    assert_eq!(bm.sequences.len(), 1);
    assert_eq!(bm.sequences[0].sprites.len(), 1);
    other_tags_kept(&src, &out);

    // Undo walks back: the removal, the rename, the set, both adds.
    for want in [2, 2, 2, 1, 0] {
        call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
        let n = call(
            &mut api,
            &mut app,
            m::BITMAP_SPRITES,
            json!({"bitmap": "floor"}),
        )["sequences"][0]["sprites"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(n, want);
    }
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
    assert_eq!(save_as(&mut api, &mut app, &t, "clean.map"), src);
}

#[test]
fn editing_a_stored_sequence_moves_the_tag_and_undo_restores_it() {
    let t = Temp::new("bitmaps-sprites-stored");
    let (mut api, mut app, src) = bitmaps_last(&t);
    // A sequence added and saved is stored in the saved file; open that.
    call(
        &mut api,
        &mut app,
        m::BITMAP_SEQUENCE_ADD,
        json!({"bitmap": "floor", "first": 0, "label": "icons"}),
    );
    call(
        &mut api,
        &mut app,
        m::BITMAP_SPRITE_ADD,
        json!({"bitmap": "floor", "sequence": 0}),
    );
    let first = save_as(&mut api, &mut app, &t, "first.map");
    let mut api = Api::new();
    let mut app = super::app(&t);
    let path = t.path("first.map");
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": path.display().to_string()}),
    );
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(read["grown"], false);
    assert_eq!(read["sequences"][0]["added"], false);
    call(
        &mut api,
        &mut app,
        m::BITMAP_SPRITE_SET,
        json!({"bitmap": "floor", "sequence": 0, "sprite": 0, "registration": [0.5, 0.5]}),
    );
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(read["grown"], true);
    let out = save_as(&mut api, &mut app, &t, "edited.map");
    assert_ne!(out, first);
    assert_eq!(floor_of(&out).0.sequences[0].sprites.len(), 1);
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
    let read = call(
        &mut api,
        &mut app,
        m::BITMAP_READ,
        json!({"bitmap": "floor"}),
    );
    assert_eq!(read["grown"], false);
    assert_eq!(save_as(&mut api, &mut app, &t, "undone.map"), first);
    assert_ne!(first, src);
}

/// The viewport follows the Textures tool (docs/editor/realtime-edits.md,
/// R15): filling the floor bitmap, the base map of the level's one
/// material, changes `view.snapshot` of the flat level to the new colour,
/// and undoing the fill brings the saved colour back. Skipped without a
/// GPU.
#[test]
fn the_snapshot_follows_a_painted_base_map() {
    let t = Temp::new("bitmaps-viewport");
    let (mut api, mut app) = bitmaps(&t);
    call(
        &mut api,
        &mut app,
        m::VIEW_LIGHTING,
        json!({"mode": "flat"}),
    );
    let shot = |api: &mut Api, app: &mut App, name: &str| -> Option<Vec<u8>> {
        let out = t.path(name);
        let params = json!({
            "out": out, "size": [64, 64],
            "camera": {"target": [2, 2, 0], "distance": 6, "yaw": 0, "pitch": 80}
        });
        match api.call(app, m::VIEW_SNAPSHOT, params) {
            Err(e) if e.code == code::NO_GPU => None,
            Ok(_) => Some(png_pixels(&out)),
            Err(e) => panic!("view.snapshot: {e}"),
        }
    };
    let Some(before) = shot(&mut api, &mut app, "before.png") else {
        return;
    };
    call(
        &mut api,
        &mut app,
        m::BITMAP_FILL,
        json!({"bitmap": "floor", "rect": {"x": 0, "y": 0, "w": 4, "h": 4}, "color": "#1428f0"}),
    );
    let blue = shot(&mut api, &mut app, "blue.png").unwrap();
    assert_ne!(before, blue, "the painted floor draws");
    let centre = (32 * 64 + 32) * 4;
    assert!(
        blue[centre + 2] > 150 && blue[centre] < 80,
        "blue floor: {:?}",
        &blue[centre..centre + 4]
    );
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "bitmaps"}));
    let undone = shot(&mut api, &mut app, "undone.png").unwrap();
    assert_eq!(before, undone, "undo restores the saved floor");
}
