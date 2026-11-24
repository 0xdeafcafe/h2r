//! The colour picker and `tag.colours` over the scenario crate's
//! invented cache: its weapon tag given one invented colour field of
//! every form, a three-float field whose name says colour, and a list of
//! packed colours (no game data).
use std::path::PathBuf;

use map::TagId;
use scenario_edit::fixture::{self, put, putf};
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::registry::{FieldView, Registry};

use crate::api::Api;
use crate::app::{Action, tag_api, tag_tests};
use crate::fields::colour::{self, Colour};

/// The weapon's root layout: a colour of each form, a name-matched
/// colour, a list of packed colours and a speed.
pub(crate) fn registry() -> &'static Registry {
    static R: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let schema = tags::schema::Schema::parse(
            "group scnr 0x10 align 4\ngroup weap 0x4c align 4\n  0x40 block 0x4 align 4\n",
        )
        .unwrap();
        let mut d = tags::defs::Defs::default();
        d.parse_into(
            "test",
            "\
group weap 0x4c
  0x0 f32[3] tint view=rgb
  0xc f32[4] glow view=argb
  0x1c u32 paint view=argb8
  0x20 u32 trim view=rgb8
  0x24 u32 band view=xrgb8
  0x28 f32[3] light view=hdr_rgb
  0x34 f32[3] edge_colour
  0x40 block shades 0x4 align 4
    0x0 u32 shade view=rgb8
  0x48 f32 speed
",
        )
        .unwrap();
        Registry::build(&schema, &d)
    })
}

/// The weapon's metadata address and its place in the file.
const WEAP: u32 = fixture::META + 0x1100;
const WEAP_FILE: usize = fixture::META_FILE + 0x1100;
/// The shades list's two elements.
const SHADES: u32 = fixture::META + 0x1180;
const SHADES_FILE: usize = fixture::META_FILE + 0x1180;

/// The fixture with the weapon given its invented colours.
pub(crate) fn colour_map() -> Vec<u8> {
    let mut b = fixture::fixture_at_end();
    let tags = fixture::INDEX + 32 + 4 * 12;
    put(&mut b, tags + 2 * 16 + 12, 0x4c);
    let w = WEAP_FILE;
    for (i, x) in [0.25f32, 0.5, 1.0].into_iter().enumerate() {
        putf(&mut b, w + 4 * i, x);
    }
    for (i, x) in [0.5f32, 1.0, 0.0, 0.25].into_iter().enumerate() {
        putf(&mut b, w + 0xc + 4 * i, x);
    }
    put(&mut b, w + 0x1c, 0x80ff_4020);
    put(&mut b, w + 0x20, 0x7f10_2030);
    put(&mut b, w + 0x24, 0x00a0_b0c0);
    for (i, x) in [2.5f32, 1.25, 0.0].into_iter().enumerate() {
        putf(&mut b, w + 0x28 + 4 * i, x);
    }
    for (i, x) in [0.1f32, 0.2, 0.3].into_iter().enumerate() {
        putf(&mut b, w + 0x34 + 4 * i, x);
    }
    put(&mut b, w + 0x40, 2);
    put(&mut b, w + 0x44, SHADES);
    put(&mut b, SHADES_FILE, 0x0011_2233);
    put(&mut b, SHADES_FILE + 4, 0x0044_5566);
    putf(&mut b, w + 0x48, 2.5);
    b
}

fn weap_id() -> TagId {
    fixture::id(2)
}

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-colours-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(t: &Temp) -> (crate::App, Api) {
    let (app, ..) = tag_tests::app_with_registry(&t.0, colour_map(), registry());
    (app, Api::new())
}

fn call(api: &mut Api, app: &mut crate::App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn code_of(r: crate::api::Reply) -> i64 {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e.code,
    }
}

/// The weapon's bytes at `offset`.
fn bytes(app: &crate::App, offset: u32, n: u32) -> Vec<u8> {
    app.tags.read(weap_id(), WEAP + offset, n).unwrap()
}

fn floats(b: &[u8]) -> Vec<f32> {
    b.chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn word(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// The dialog of the root field `name`, opened as a click on its swatch
/// opens it.
fn dialog(app: &crate::App, name: &str) -> colour::Open {
    let (at, _, f) = tag_api::resolve(&app.tags, weap_id(), name).unwrap();
    colour::Open::new(&app.tags, weap_id(), at + f.offset, &f, name).unwrap()
}

#[test]
fn the_method_is_listed() {
    let t = Temp::new("list");
    let (mut app, mut api) = open(&t);
    let list = call(&mut api, &mut app, m::METHODS, json!({}));
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == m::TAG_COLOURS)
        .expect("tag.colours is listed");
    assert_eq!(row["status"], "ready");
}

/// `tag.colours` gives every colour row, in the panel's order, the list's
/// elements included, with form, source, hex, channels and intensity.
#[test]
fn colours_lists_every_colour_row_with_its_form() {
    let t = Temp::new("lists");
    let (mut app, mut api) = open(&t);
    let v = call(&mut api, &mut app, m::TAG_COLOURS, json!({"tag": "weap"}));
    assert_eq!(v["truncated"], false);
    let rows = v["colours"].as_array().unwrap();
    let got: Vec<(&str, &str, &str, &str)> = rows
        .iter()
        .map(|r| {
            (
                r["path"].as_str().unwrap(),
                r["form"].as_str().unwrap(),
                r["by"].as_str().unwrap(),
                r["hex"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("tint", "rgb", "view", "#ff4080ff"),
            ("glow", "argb", "view", "#80ff0040"),
            ("paint", "argb8", "view", "#80ff4020"),
            ("trim", "rgb8", "view", "#ff102030"),
            ("band", "xrgb8", "view", "#ffa0b0c0"),
            ("light", "hdr_rgb", "view", "#ffff8000"),
            ("edge_colour", "rgb", "name", "#ff1a334d"),
            ("shades[0].shade", "rgb8", "view", "#ff112233"),
            ("shades[1].shade", "rgb8", "view", "#ff445566"),
        ]
    );
    let light = &rows[5];
    assert_eq!(light["intensity"], json!(2.5));
    assert_eq!(light["rgba"], json!([1.0, 0.5, 0.0, 1.0]));
    assert!(rows[0].get("intensity").is_none());
}

/// `tag.fields` rows of colour fields carry `colour`; others do not.
#[test]
fn fields_rows_carry_the_colour() {
    let t = Temp::new("fields");
    let (mut app, mut api) = open(&t);
    let v = call(&mut api, &mut app, m::TAG_FIELDS, json!({"tag": "weap"}));
    let row = |name: &str| {
        v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
            .clone()
    };
    let rgba = json!([1.0f32, 64.0f32 / 255.0, 32.0f32 / 255.0, 128.0f32 / 255.0]);
    assert_eq!(
        row("paint")["colour"],
        json!({"form": "argb8", "by": "view", "hex": "#80ff4020", "rgba": rgba})
    );
    assert_eq!(row("light")["colour"]["intensity"], json!(2.5));
    // Every form reads as a colour.
    for name in [
        "tint",
        "glow",
        "paint",
        "trim",
        "band",
        "light",
        "edge_colour",
    ] {
        assert_eq!(row(name)["type"], "Colour", "{name}");
    }
    assert_eq!(row("speed")["type"], "Decimal number");
    assert_eq!(row("edge_colour")["colour"]["by"], "name");
    assert!(row("speed").get("colour").is_none());
}

/// Through the dialog, on a field of each form: the picked colour is
/// written, and the bytes equal it after quantisation (a packed form
/// without alpha keeping its top byte).
#[test]
fn a_pick_through_the_dialog_writes_each_form() {
    let t = Temp::new("pick");
    let (mut app, _) = open(&t);
    let picked = Colour::new([0.2, 0.4, 0.6, 0.8], None);
    let cases: [(&str, u32, u32); 6] = [
        ("tint", 0x0, 12),
        ("glow", 0xc, 16),
        ("paint", 0x1c, 4),
        ("trim", 0x20, 4),
        ("band", 0x24, 4),
        ("light", 0x28, 12),
    ];
    for (name, offset, n) in cases {
        let mut open = dialog(&app, name);
        let c = if name == "light" {
            Colour::new(picked.rgba, Some(3.0))
        } else {
            picked
        };
        let action = colour::pick(&app.tags, &mut open, c).expect("a change writes");
        app.dispatch(action);
        let mut settings = app.tag_fields.clone();
        app.dispatch(colour::finish(&mut open, &mut settings).expect("a pick seals"));
        let b = bytes(&app, offset, n);
        match name {
            "tint" => assert_eq!(floats(&b), [0.2, 0.4, 0.6]),
            "glow" => assert_eq!(floats(&b), [0.8, 0.2, 0.4, 0.6]),
            "paint" => assert_eq!(word(&b), 0xcc33_6699),
            "trim" => assert_eq!(word(&b), 0x7f33_6699, "the top byte is kept"),
            "band" => assert_eq!(word(&b), 0x0033_6699, "the top byte is kept"),
            "light" => assert_eq!(floats(&b), [0.2f32 * 3.0, 0.4f32 * 3.0, 0.6f32 * 3.0]),
            _ => unreachable!(),
        }
        // The picker shows what the field holds.
        let form = colour::form(&open.field).unwrap();
        assert_eq!(colour::encode(form, open.colour, &b), b, "{name}");
        assert_eq!(
            settings.recent_colours[0],
            colour::hex(open.colour),
            "{name}"
        );
    }
}

/// A byte form's picker never shows a colour the field cannot hold.
#[test]
fn a_byte_form_shows_the_value_after_quantisation() {
    let t = Temp::new("quantise");
    let (app, _) = open(&t);
    let mut open = dialog(&app, "trim");
    colour::pick(
        &app.tags,
        &mut open,
        Colour::new([0.5, 0.5, 0.5, 0.3], None),
    );
    assert_eq!(
        open.colour.rgba,
        [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0]
    );
    let mut open = dialog(&app, "tint");
    colour::pick(
        &app.tags,
        &mut open,
        Colour::new([0.5, 0.5, 0.5, 0.3], None),
    );
    assert_eq!(open.colour.rgba, [0.5, 0.5, 0.5, 1.0], "no alpha on rgb");
}

/// A drag (several picks, then the mouse let go) is one undo step.
#[test]
fn a_drag_is_one_undo_step() {
    let t = Temp::new("drag");
    let (mut app, _) = open(&t);
    let before = bytes(&app, 0x1c, 4);
    let mut open = dialog(&app, "paint");
    for x in [0.1f32, 0.3, 0.8] {
        let action = colour::pick(&app.tags, &mut open, Colour::new([x, x, x, 1.0], None));
        app.dispatch(action.unwrap());
    }
    let mut settings = app.tag_fields.clone();
    app.dispatch(colour::finish(&mut open, &mut settings).unwrap());
    assert!(
        colour::finish(&mut open, &mut settings).is_none(),
        "sealed once"
    );
    assert_eq!(word(&bytes(&app, 0x1c, 4)), 0xffcc_cccc);
    assert_eq!(settings.recent_colours, ["#ffcccccc"]);
    app.dispatch(Action::Undo);
    assert_eq!(bytes(&app, 0x1c, 4), before);
    assert!(!app.tags.can_undo());
}

/// `tag.set` takes a hex string or `{rgba, intensity?}` on a colour
/// field, converting form, and refuses what the form cannot take.
#[test]
fn set_takes_hex_and_channels() {
    let t = Temp::new("set");
    let (mut app, mut api) = open(&t);
    let set = |api: &mut Api, app: &mut crate::App, path: &str, value: Value| {
        api.call(
            app,
            m::TAG_SET,
            json!({"tag": "weap", "path": path, "value": value}),
        )
    };
    set(&mut api, &mut app, "paint", json!("#40102030")).unwrap();
    assert_eq!(word(&bytes(&app, 0x1c, 4)), 0x4010_2030);
    // No alpha in an rgb8 field: the top byte is kept.
    set(&mut api, &mut app, "trim", json!("#00ffffff")).unwrap();
    assert_eq!(word(&bytes(&app, 0x20, 4)), 0x7fff_ffff);
    // Six digits are opaque.
    set(&mut api, &mut app, "glow", json!("#ff0000")).unwrap();
    assert_eq!(floats(&bytes(&app, 0xc, 16)), [1.0, 1.0, 0.0, 0.0]);
    // An hdr colour keeps its intensity under a hex, and takes a new one.
    set(&mut api, &mut app, "light", json!("#ffffffff")).unwrap();
    assert_eq!(floats(&bytes(&app, 0x28, 12)), [2.5, 2.5, 2.5]);
    set(
        &mut api,
        &mut app,
        "light",
        json!({"rgba": [0.5, 0.25, 0.0, 1.0], "intensity": 4.0}),
    )
    .unwrap();
    assert_eq!(floats(&bytes(&app, 0x28, 12)), [2.0, 1.0, 0.0]);
    // Numbers still go the plain way.
    set(&mut api, &mut app, "tint", json!([0.1, 0.2, 0.3])).unwrap();
    assert_eq!(floats(&bytes(&app, 0x0, 12)), [0.1, 0.2, 0.3]);
    set(&mut api, &mut app, "shades[1].shade", json!("#ffabcdef")).unwrap();
    assert_eq!(
        word(&app.tags.read(weap_id(), SHADES + 4, 4).unwrap()),
        0x00ab_cdef
    );
    // Refused by name.
    for (path, value) in [
        ("paint", json!("#12345")),
        ("paint", json!("purple")),
        ("tint", json!({"rgba": [1.5, 0.0, 0.0, 1.0]})),
        ("tint", json!({"rgba": [1.0, 0.0, 0.0]})),
        (
            "tint",
            json!({"rgba": [1.0, 0.0, 0.0, 1.0], "intensity": 2.0}),
        ),
        (
            "light",
            json!({"rgba": [1.0, 0.0, 0.0, 1.0], "intensity": -1.0}),
        ),
    ] {
        assert_eq!(
            code_of(set(&mut api, &mut app, path, value.clone())),
            code::INVALID_PARAMS,
            "{path} {value}"
        );
    }
    let v = call(&mut api, &mut app, m::TAG_COLOURS, json!({"tag": "weap"}));
    assert_eq!(v["colours"][2]["hex"], "#40102030");
}

/// The definitions of this fixture name a field of every colour form.
#[test]
fn the_fixture_has_every_colour_form() {
    let t = Temp::new("forms");
    let (app, _) = open(&t);
    let layout = app.tags.layout(weap_id()).unwrap().clone();
    let mut forms: Vec<FieldView> = layout.fields.iter().filter_map(colour::form).collect();
    forms.sort();
    forms.dedup();
    let mut want: Vec<FieldView> = FieldView::ALL
        .into_iter()
        .filter(|v| v.is_colour())
        .collect();
    want.sort();
    assert_eq!(forms, want);
}
