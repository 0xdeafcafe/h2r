//! The colour forms on invented bytes: decode, encode, quantisation,
//! hex, `tag.set` values, and the recent and saved colours.
use serde_json::json;
use tags::registry::FieldView;

use super::*;

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

#[test]
fn each_form_decodes_and_encodes_its_bytes() {
    let cases: [(FieldView, Vec<u8>, [f32; 4]); 5] = [
        (
            FieldView::Rgb,
            floats(&[0.25, 0.5, 1.0]),
            [0.25, 0.5, 1.0, 1.0],
        ),
        (
            FieldView::Argb,
            floats(&[0.5, 1.0, 0.0, 0.25]),
            [1.0, 0.0, 0.25, 0.5],
        ),
        (
            FieldView::Argb8,
            0x80ff_0000u32.to_le_bytes().to_vec(),
            [1.0, 0.0, 0.0, 128.0 / 255.0],
        ),
        (
            FieldView::Rgb8,
            0x7f00_ff00u32.to_le_bytes().to_vec(),
            [0.0, 1.0, 0.0, 1.0],
        ),
        (
            FieldView::Xrgb8,
            0x0100_00ffu32.to_le_bytes().to_vec(),
            [0.0, 0.0, 1.0, 1.0],
        ),
    ];
    for (form, bytes, rgba) in cases {
        let c = decode(form, &bytes);
        assert_eq!(c.rgba, rgba, "{form:?}");
        assert_eq!(c.intensity, None);
        assert_eq!(encode(form, c, &bytes), bytes, "{form:?} round trip");
        assert_eq!(width(form), bytes.len());
    }
}

#[test]
fn a_packed_form_without_alpha_keeps_its_top_byte() {
    let old = 0x7f10_2030u32.to_le_bytes();
    let white = Colour::new([1.0, 1.0, 1.0, 0.0], None);
    for form in [FieldView::Rgb8, FieldView::Xrgb8] {
        assert_eq!(encode(form, white, &old), 0x7fff_ffffu32.to_le_bytes());
    }
    assert_eq!(
        encode(FieldView::Argb8, white, &old),
        0x00ff_ffffu32.to_le_bytes()
    );
}

#[test]
fn an_hdr_colour_splits_into_colour_and_intensity() {
    let bytes = floats(&[2.5, 1.25, 0.0]);
    let c = decode(FieldView::HdrRgb, &bytes);
    assert_eq!(c.rgba, [1.0, 0.5, 0.0, 1.0]);
    assert_eq!(c.intensity, Some(2.5));
    assert_eq!(encode(FieldView::HdrRgb, c, &bytes), bytes);
    // At or below 1 the intensity is 1.
    let dim = decode(FieldView::HdrRgb, &floats(&[0.5, 0.25, 1.0]));
    assert_eq!(dim.intensity, Some(1.0));
    assert_eq!(dim.rgba, [0.5, 0.25, 1.0, 1.0]);
    // The hex is the colour at intensity 1.
    assert_eq!(hex(c), "#ffff8000");
}

#[test]
fn a_byte_form_holds_the_quantised_value() {
    let half = Colour::new([0.5, 0.5, 0.5, 0.5], None);
    let q = 128.0 / 255.0;
    assert_eq!(held(FieldView::Argb8, half).rgba, [q, q, q, q]);
    assert_eq!(held(FieldView::Rgb8, half).rgba, [q, q, q, 1.0]);
    assert_eq!(held(FieldView::Rgb, half).rgba, [0.5, 0.5, 0.5, 1.0]);
    assert_eq!(held(FieldView::Argb, half).rgba, [0.5; 4]);
    assert_eq!(held(FieldView::HdrRgb, half).intensity, Some(1.0));
    assert_eq!(
        held(FieldView::Rgb, Colour::new([0.5; 4], Some(2.0))).intensity,
        None
    );
}

#[test]
fn hex_reads_and_writes_eight_and_six_digits() {
    assert_eq!(parse_hex("#80ff0000"), Some([1.0, 0.0, 0.0, 128.0 / 255.0]));
    assert_eq!(parse_hex("00ff00"), Some([0.0, 1.0, 0.0, 1.0]));
    assert_eq!(parse_hex(" #FF0000FF "), Some([0.0, 0.0, 1.0, 1.0]));
    for bad in [
        "",
        "#",
        "#12345",
        "#1234567",
        "#123456789",
        "red",
        "#ggffff",
        "#+1ffff",
    ] {
        assert_eq!(parse_hex(bad), None, "{bad:?}");
    }
    let c = Colour::new([1.0, 0.25, 0.0, 0.5], None);
    assert_eq!(hex(c), "#80ff4000");
    assert_eq!(
        parse_hex(&hex(c)).map(|x| hex(Colour::new(x, None))),
        Some(hex(c))
    );
}

#[test]
fn a_paste_converts_form() {
    let now = Colour::new([0.0, 0.0, 0.0, 1.0], Some(3.0));
    // Alpha is dropped by a form without it.
    assert_eq!(
        paste(FieldView::Rgb, "#00ffffff", now).unwrap().rgba,
        [1.0; 4]
    );
    // An hdr field keeps its intensity.
    assert_eq!(
        paste(FieldView::HdrRgb, "#ffffffff", now)
            .unwrap()
            .intensity,
        Some(3.0)
    );
    assert!(paste(FieldView::Argb8, "nope", now).is_none());
}

#[test]
fn set_values_convert_or_are_refused_by_name() {
    let old = 0x7f10_2030u32.to_le_bytes();
    assert_eq!(
        from_json(FieldView::Rgb8, &json!("#00ffffff"), &old).unwrap(),
        Some(0x7fff_ffffu32.to_le_bytes().to_vec())
    );
    assert_eq!(
        from_json(
            FieldView::Argb8,
            &json!({"rgba": [1.0, 0.0, 0.0, 0.0]}),
            &old
        )
        .unwrap(),
        Some(0x00ff_0000u32.to_le_bytes().to_vec())
    );
    // Numbers are not a colour value: the plain way takes them.
    assert_eq!(from_json(FieldView::Rgb8, &json!(7), &old).unwrap(), None);
    assert_eq!(
        from_json(FieldView::Rgb, &json!([0.1, 0.2, 0.3]), &floats(&[0.0; 3])).unwrap(),
        None
    );
    let hdr = floats(&[2.0, 2.0, 2.0]);
    assert_eq!(
        from_json(
            FieldView::HdrRgb,
            &json!({"rgba": [1.0, 0.5, 0.0, 1.0], "intensity": 4.0}),
            &hdr
        )
        .unwrap(),
        Some(floats(&[4.0, 2.0, 0.0]))
    );
    // Without an intensity, an hdr colour keeps its own.
    assert_eq!(
        from_json(
            FieldView::HdrRgb,
            &json!({"rgba": [0.5, 0.5, 0.5, 1.0]}),
            &hdr
        )
        .unwrap(),
        Some(floats(&[1.0, 1.0, 1.0]))
    );
    for (form, v, why) in [
        (FieldView::Rgb8, json!("#abc"), "not a hex colour"),
        (
            FieldView::Rgb8,
            json!({"rgba": [2.0, 0.0, 0.0, 1.0]}),
            "outside 0 to 1",
        ),
        (
            FieldView::Rgb8,
            json!({"rgba": [1.0, 0.0, 0.0]}),
            "four numbers",
        ),
        (
            FieldView::Rgb8,
            json!({"rgba": [1.0, 0.0, 0.0, 1.0], "intensity": 2.0}),
            "only an hdr_rgb",
        ),
        (
            FieldView::HdrRgb,
            json!({"rgba": [1.0, 0.0, 0.0, 1.0], "intensity": "x"}),
            "intensity",
        ),
    ] {
        let old: &[u8] = if form == FieldView::HdrRgb {
            &hdr
        } else {
            &old
        };
        let e = from_json(form, &v, old).expect_err(&v.to_string());
        assert!(e.contains(why), "{v}: {e}");
    }
}

#[test]
fn recent_colours_are_newest_first_once_and_capped() {
    let mut recent = Vec::new();
    for i in 0..RECENT + 3 {
        remember(&mut recent, &format!("#ff0000{i:02x}"));
    }
    assert_eq!(recent.len(), RECENT);
    assert_eq!(recent[0], format!("#ff0000{:02x}", RECENT + 2));
    remember(&mut recent, "#ff000005");
    assert_eq!(recent[0], "#ff000005");
    assert_eq!(recent.iter().filter(|h| *h == "#ff000005").count(), 1);
}

#[test]
fn saved_colours_keep_their_order_once_and_capped() {
    let mut saved = Vec::new();
    save(&mut saved, "#ff000001");
    save(&mut saved, "#ff000002");
    save(&mut saved, "#ff000001");
    assert_eq!(saved, ["#ff000001", "#ff000002"]);
    for i in 0..SAVED {
        save(&mut saved, &format!("#ff0001{i:02x}"));
    }
    assert_eq!(saved.len(), SAVED);
    assert_eq!(saved[0], "#ff000100", "the oldest go first");
}

#[test]
fn recent_and_saved_colours_are_kept_with_the_layout() {
    let settings = Settings {
        recent_colours: vec!["#ff102030".into(), "#80ffffff".into()],
        saved_colours: vec!["#ff000000".into()],
        colour_clip: "#ff123456".into(),
        ..Settings::default()
    };
    let back = Settings::from_ini(&settings.ini());
    assert_eq!(back.recent_colours, settings.recent_colours);
    assert_eq!(back.saved_colours, settings.saved_colours);
    assert_eq!(back.colour_clip, "", "the clip is not saved");
    // A line that is not a colour is skipped.
    let bad = Settings::from_ini("[Studio][Tags]\nRecentColour=purple\nSavedColour=#ff00\n");
    assert!(bad.recent_colours.is_empty() && bad.saved_colours.is_empty());
}
