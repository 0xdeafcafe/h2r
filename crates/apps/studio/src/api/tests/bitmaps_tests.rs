//! The `bitmap.*` methods and the Textures tool's actions over the
//! invented textured map (docs/editor/modeling.md, slice 5): its floor
//! bitmap is 4 by 4 pixels of one `A8R8G8B8` colour, (200, 60, 20, 255).

use super::*;
use crate::app::textures::BitmapAction;

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
    assert!(r.is_err());
    assert!(!out.exists());
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
        code::REFUSED
    );
    let out = t.path("reshaped.map");
    call(
        &mut api,
        &mut app,
        m::DOC_SAVE_AS,
        json!({"doc": "bitmaps", "path": out.display().to_string()}),
    );
    let bytes = std::fs::read(&out).unwrap();
    let mut m = map::Map::from_reader(std::io::Cursor::new(bytes.clone())).unwrap();
    let tag = ::tags::DecodedTag::read(&mut m, fixture::id(7)).unwrap();
    let ::tags::TagData::Bitmap(b) = tag.data else {
        panic!("a bitmap")
    };
    let img = &b.images[0];
    assert_eq!(
        (img.dimensions[0], img.dimensions[1], img.format),
        (8, 8, ::tags::PixelFormat::Dxt1)
    );
    let mut files = assets::ResourceFiles::new();
    files.set_bytes(map::ResourceFile::Local, std::sync::Arc::from(bytes));
    let d = assets::decode_image(img, &mut files).unwrap();
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
