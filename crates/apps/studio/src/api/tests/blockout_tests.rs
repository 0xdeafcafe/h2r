//! `blockout.describe` and `blockout.render` (no map, no disc): the labels
//! BO2's layout gives, the sheet's items, the refusals, and the pictures
//! when there is a GPU.

use std::collections::BTreeSet;

use content_kit::blockout::mesh::{self, View};
use dev_texture::label;
use forge_shapes::{PieceDef, Shape};

use super::*;
use crate::api::blockout::sheet_items;

fn block(size: [u16; 3]) -> Value {
    json!({"shape": "block", "size": size})
}

/// `describe` gives each plate the rows `label::layout` gives its face,
/// and the counts the kit's mesh has.
#[test]
fn blockout_describe_gives_the_labels_the_layout_gives() {
    let t = Temp::new("blockout-describe");
    let mut app = app(&t);
    let mut api = Api::new();
    for (shape, size, label_name) in [
        (Shape::Block, [256, 256, 256], "BLOCK"),
        (Shape::Wall, [256, 16, 128], "WALL"),
        (Shape::FloorPlate, [512, 512, 16], "FLOOR PLATE"),
    ] {
        let v = call(
            &mut api,
            &mut app,
            m::BLOCKOUT_DESCRIBE,
            json!({"shape": shape.name(), "size": size}),
        );
        let def = PieceDef::dev(shape, size, 0);
        assert_eq!(v["name"], def.name());
        assert_eq!(v["tag_path"], def.tag_path());
        assert_eq!(v["family"], "structural");
        assert_eq!(v["label"], label_name);
        let solid = def.generate().unwrap();
        let built = mesh::build(&def, &solid, View::Dev);
        assert_eq!(v["vertices"], built.vertices());
        assert_eq!(v["triangles"], built.triangles());
        assert_eq!(v["faces"], solid.shell.len());
        assert_eq!(v["hulls"], solid.hulls.len());
        let plates = v["plates"].as_array().unwrap();
        assert_eq!(plates.len(), built.plates.len());
        assert!(!plates.is_empty(), "{shape}: no plate");
        assert_eq!(v["unplated"], solid.shell.len() - plates.len());
        for (j, fp) in plates.iter().zip(&built.plates) {
            let (w, h) = (j["width"].as_u64().unwrap() as u32, j["height"].as_u64().unwrap() as u32);
            let plate = label::layout(label_name, w, h).expect("a plate the kit cut");
            assert_eq!(j["h"], plate.h);
            assert_eq!(j["text"], json!(plate.text()));
            let rows = label::rows(label_name, w, h);
            assert_eq!(j["metres"], json!([rows[1], rows[2]]));
            assert_eq!(fp.plate, plate);
        }
    }
    // A 256 cube's faces all say its size, in units and in metres.
    let cube = call(&mut api, &mut app, m::BLOCKOUT_DESCRIBE, block([256, 256, 256]));
    assert_eq!(cube["plates"].as_array().unwrap().len(), 6);
    assert_eq!(cube["plates"][0]["text"], json!(["BLOCK", "256 \u{d7} 256 U", "7.80 \u{d7} 7.80 M"]));
    // A face too small for a plate has none: a 32 cube has no plate.
    let small = call(&mut api, &mut app, m::BLOCKOUT_DESCRIBE, block([32, 32, 32]));
    assert!(small["plates"].as_array().unwrap().is_empty());
    assert_eq!(small["unplated"], 6);
    // A rock has no plate and says it is natural.
    let rock = call(
        &mut api,
        &mut app,
        m::BLOCKOUT_DESCRIBE,
        json!({"shape": "boulder", "size": [192, 192, 128], "param": 2}),
    );
    assert_eq!(rock["family"], "natural");
    assert!(rock["plates"].as_array().unwrap().is_empty());
}

/// A definition is also given by name, with or without its digest.
#[test]
fn blockout_describe_takes_a_canonical_name() {
    let t = Temp::new("blockout-describe-name");
    let mut app = app(&t);
    let mut api = Api::new();
    let def = PieceDef::dev(Shape::Stairs, [128, 64, 64], 4);
    let by_name = call(&mut api, &mut app, m::BLOCKOUT_DESCRIBE, json!({"name": def.name()}));
    let short = call(&mut api, &mut app, m::BLOCKOUT_DESCRIBE, json!({"name": "stairs_128x64x64_4"}));
    let by_shape = call(
        &mut api,
        &mut app,
        m::BLOCKOUT_DESCRIBE,
        json!({"shape": "stairs", "size": [128, 64, 64], "param": 4}),
    );
    assert_eq!(by_name, by_shape);
    assert_eq!(short, by_shape);
    assert_eq!(by_shape["name"], def.name());
    assert_eq!(by_shape["hulls"], 4);
}

/// Each refusal names the param it is about, before any GPU is asked.
#[test]
fn blockout_refuses_by_name() {
    let t = Temp::new("blockout-refusals");
    let mut app = app(&t);
    let mut api = Api::new();
    let out = t.path("x.png").display().to_string();
    for (method, params, why) in [
        (m::BLOCKOUT_DESCRIBE, json!({}), "name"),
        (m::BLOCKOUT_DESCRIBE, json!({"shape": "sphere", "size": [64, 64, 64]}), "shape"),
        (m::BLOCKOUT_DESCRIBE, json!({"shape": "block"}), "size"),
        (m::BLOCKOUT_DESCRIBE, json!({"shape": "block", "size": [3, 64, 64]}), "size"),
        (m::BLOCKOUT_DESCRIBE, json!({"shape": "block", "size": [64, 64]}), "size"),
        (m::BLOCKOUT_DESCRIBE, json!({"shape": "stairs", "size": [128, 64, 64], "param": 1}), "param"),
        (m::BLOCKOUT_DESCRIBE, json!({"size": [64, 64, 64]}), "shape"),
        (m::BLOCKOUT_DESCRIBE, json!({"name": "block_64x64x64_0", "size": [64, 64, 64]}), "name"),
        (m::BLOCKOUT_DESCRIBE, json!({"name": "blob_64x64x64_0"}), "name"),
        (m::BLOCKOUT_DESCRIBE, json!({"name": "block_64x64x64_0_000000000000"}), "name"),
        (m::BLOCKOUT_RENDER, json!({"shape": "block", "size": [64, 64, 64], "view": "back", "out": out}), "view"),
        (m::BLOCKOUT_RENDER, json!({"shape": "block", "size": [64, 64, 64], "fit": "shape", "out": out}), "fit"),
        (m::BLOCKOUT_RENDER, json!({"fit": "none", "out": out}), "fit"),
        (m::BLOCKOUT_RENDER, json!({"shape": "block", "size": [64, 64, 64], "pixels": [0, 9], "out": out}), "pixels"),
        (m::BLOCKOUT_RENDER, json!({"shape": "block", "size": [64, 64, 64], "plates": "yes", "out": out}), "plates"),
        // The sheet's 39 cells do not fit 320 by 200.
        (m::BLOCKOUT_RENDER, json!({"pixels": [320, 200], "out": out}), "pixels"),
    ] {
        let e = api.call(&mut app, method, params.clone()).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{method} {params}: {e}");
        assert!(e.message.starts_with(why), "{params}: {}", e.message);
    }
    let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("never-blockout.png");
    assert_eq!(
        code_of(api.call(&mut app, m::BLOCKOUT_RENDER, json!({"shape": "block", "size": [64, 64, 64], "out": inside}))),
        code::REFUSED
    );
    assert!(!inside.exists());
}

/// The sheet holds every shape of every family, each definition valid
/// and named once.
#[test]
fn blockout_sheet_holds_every_shape_in_every_family() {
    let items = sheet_items();
    let shapes: BTreeSet<Shape> = items.iter().map(|i| i.def.shape).collect();
    assert_eq!(shapes, Shape::ALL.into_iter().collect::<BTreeSet<_>>());
    for family in ["structural", "natural"] {
        let of: BTreeSet<Shape> = items.iter().filter(|i| i.family == family).map(|i| i.def.shape).collect();
        let want: BTreeSet<Shape> =
            Shape::ALL.into_iter().filter(|s| (family == "natural") == s.is_natural()).collect();
        assert_eq!(of, want, "{family}");
    }
    // The starter's structural entries are all there, and each natural
    // shape has more than one seed.
    for def in forge_shapes::starter().into_iter().filter(|d| !d.shape.is_natural()) {
        assert!(items.iter().any(|i| i.def == def), "{} is not on the sheet", def.name());
    }
    let names: BTreeSet<String> = items.iter().map(|i| i.def.name()).collect();
    assert_eq!(names.len(), items.len());
    for item in &items {
        item.def.generate().unwrap_or_else(|e| panic!("{}: {e}", item.def.name()));
        if item.def.shape.is_natural() {
            let seeds = items.iter().filter(|i| i.def.shape == item.def.shape).count();
            assert!(seeds > 1, "{}", item.def.name());
        }
    }
    // The structural group comes first, so each family starts its row.
    let first_natural = items.iter().position(|i| i.family == "natural").unwrap();
    assert!(items[first_natural..].iter().all(|i| i.family == "natural"));
}

/// A render of a block differs from the picture without it where its
/// plate projects, and from the same block with flat cells. Needs a GPU.
#[test]
fn blockout_render_draws_the_plate_where_it_projects() {
    let t = Temp::new("blockout-render");
    let mut app = app(&t);
    let mut api = Api::new();
    let size = [256, 256, 256];
    let render = |api: &mut Api, app: &mut App, name: &str, plates: bool| {
        api.call(
            app,
            m::BLOCKOUT_RENDER,
            json!({"shape": "block", "size": size, "pixels": [960, 720], "plates": plates, "out": t.path(name)}),
        )
    };
    let with = match render(&mut api, &mut app, "with.png", true) {
        Err(e) if e.code == code::NO_GPU => return,
        Err(e) => panic!("blockout.render: {e}"),
        Ok(v) => v,
    };
    let flat = render(&mut api, &mut app, "flat.png", false).unwrap();
    assert_eq!((with["width"].clone(), with["height"].clone()), (json!(960), json!(720)));
    assert_eq!(with["name"], PieceDef::dev(Shape::Block, size, 0).name());
    assert!(with["covered"].as_u64().unwrap() > 1000, "{with}");
    assert!(with["piece_pixels"].as_u64().unwrap() > 1000, "{with}");
    assert_eq!(with["grid_units"], 64);
    let (a, b) = (png_pixels(&t.path("with.png")), png_pixels(&t.path("flat.png")));
    assert_eq!(a.len(), b.len());
    // The two pictures differ only where a plate the viewer sees
    // projects, and in every such plate.
    let seen: Vec<Vec<i64>> = with["plates"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["visible"] == true)
        .map(|p| p["bounds"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as i64).collect())
        .collect();
    assert!(seen.len() >= 2, "{with}");
    let mut inside = vec![0usize; seen.len()];
    for (i, (p, q)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
        if p == q {
            continue;
        }
        let (x, y) = ((i % 960) as i64, (i / 960) as i64);
        let hit = seen.iter().position(|c| x >= c[0] - 2 && x <= c[2] + 2 && y >= c[1] - 2 && y <= c[3] + 2);
        assert!(hit.is_some(), "pixel ({x}, {y}) differs outside every plate");
        inside[hit.unwrap()] += 1;
    }
    for (c, n) in seen.iter().zip(&inside) {
        assert!(*n > 30, "plate at {c:?} changed only {n} pixels");
    }
    let _ = flat;
}

/// The sheet has a labelled cell for every definition, each drawn.
/// Needs a GPU.
#[test]
fn blockout_render_writes_the_sheet() {
    let t = Temp::new("blockout-sheet");
    let mut app = app(&t);
    let mut api = Api::new();
    let sheet = t.path("sheet.png");
    let v = match api.call(&mut app, m::BLOCKOUT_RENDER, json!({"out": sheet})) {
        Err(e) if e.code == code::NO_GPU => return,
        Err(e) => panic!("blockout.render: {e}"),
        Ok(v) => v,
    };
    assert_eq!((v["width"].clone(), v["height"].clone()), (json!(3200), json!(2000)));
    assert!(sheet.exists());
    let cells = v["cells"].as_array().unwrap();
    let items = sheet_items();
    assert_eq!(cells.len(), items.len());
    for (cell, item) in cells.iter().zip(&items) {
        assert_eq!(cell["name"], item.def.name());
        assert_eq!(cell["family"], item.family);
        assert!(cell["piece_pixels"].as_u64().unwrap() > 20, "{} is not drawn", cell["name"]);
    }
    let shapes: BTreeSet<&str> = cells.iter().map(|c| c["shape"].as_str().unwrap()).collect();
    assert_eq!(shapes.len(), Shape::ALL.len());
    // Each family starts its own row.
    let row_of = |family: &str| -> BTreeSet<u64> {
        cells.iter().filter(|c| c["family"] == family).map(|c| c["cell"][1].as_u64().unwrap()).collect()
    };
    assert!(row_of("structural").is_disjoint(&row_of("natural")));
    // One scale: a bigger piece covers more pixels than a smaller one of
    // its shape.
    let covers = |name: &str| {
        cells.iter().find(|c| c["name"] == PieceDef::dev(Shape::Block, [256, 256, 256], 0).name() && name == "big"
            || c["name"] == PieceDef::dev(Shape::Block, [32, 32, 32], 0).name() && name == "small")
            .unwrap()["piece_pixels"].as_u64().unwrap()
    };
    assert!(covers("big") > 4 * covers("small"));
    // Per-shape fit scales a small block up.
    let by_shape = call(&mut api, &mut app, m::BLOCKOUT_RENDER, json!({"out": t.path("by-shape.png"), "fit": "shape"}));
    let small = |v: &Value| -> u64 {
        v["cells"].as_array().unwrap().iter()
            .find(|c| c["name"] == PieceDef::dev(Shape::Block, [32, 32, 32], 0).name()).unwrap()["piece_pixels"].as_u64().unwrap()
    };
    assert!(small(&by_shape) > small(&v));
}
