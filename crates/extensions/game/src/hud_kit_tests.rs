//! The HUD kit's runtime on `FlatMemory`: the appended widget records'
//! byte layout, a second install leaving one copy, the draw hooks on a
//! kit widget and passing a retail one through, the crop, and the
//! training, prompt, navpoint and notification channels.

use engine::memory::FlatMemory;
use engine::tags::{
    TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, entry_layout, group_layout, index_layout,
};
use hud_kit_core::model::{Anchor, Art, Element, Font, Justify, Place};

use super::*;

const BASE: u32 = 0x0040_0000;
const ROOT: u32 = 0x0053_0000;
const RETAIL_BITMAPS: u32 = 0x0053_1000;
const RETAIL_TEXTS: u32 = 0x0053_2000;
const REGION: (u32, u32) = (0x0056_0000, REGION_LEN);
const POINTS: (u32, u32) = (0x0057_0000, POINTS_LEN);
const SP: u32 = 0x0054_0000;
const GLOBALS: u32 = 0x0054_1000;

fn memory() -> FlatMemory {
    let mut m = FlatMemory::new(BASE, 0x0018_0000);
    // Two retail bitmap widgets and one text widget, each byte its index.
    m.set_u32(ROOT + records::ROOT_BITMAPS, 2).unwrap();
    m.set_u32(ROOT + records::ROOT_BITMAPS + 4, RETAIL_BITMAPS)
        .unwrap();
    m.set_u32(ROOT + records::ROOT_TEXTS, 1).unwrap();
    m.set_u32(ROOT + records::ROOT_TEXTS + 4, RETAIL_TEXTS)
        .unwrap();
    m.fill(RETAIL_BITMAPS, records::BITMAP_SIZE as u32, 0xb0)
        .unwrap();
    m.fill(
        RETAIL_BITMAPS + records::BITMAP_SIZE as u32,
        records::BITMAP_SIZE as u32,
        0xb1,
    )
    .unwrap();
    m.fill(RETAIL_TEXTS, records::TEXT_SIZE as u32, 0x70)
        .unwrap();
    // One local player: fullscreen.
    m.set_u32(points::PLAYER_GLOBALS.base, GLOBALS).unwrap();
    m.set_u16(GLOBALS + 8, 1).unwrap();
    m
}

/// A HUD of a cropped meter in every mode and a text fullscreen only.
fn hud() -> (Hud, Compiled) {
    let hud = Hud {
        name: "t",
        elements: vec![
            Element::meter(
                "m",
                Art::StripMeter,
                Place::new(Anchor::TopLeft, (0, 0)).split((0, 0), (0, 0)),
            ),
            Element::text(
                "x",
                Font::Lcd,
                Justify::Start,
                "99",
                Place::new(Anchor::TopLeft, (0, 30)),
            ),
        ],
    };
    let mut bitmap = vec![0x4b; records::BITMAP_SIZE];
    bitmap[records::bitmap::SEQUENCES..records::bitmap::SEQUENCES + 3].copy_from_slice(&[0, 0, 0]);
    let mut text = vec![0x54; records::TEXT_SIZE];
    text[records::text::FONTS..records::text::FONTS + 3].copy_from_slice(&[
        1,
        records::NO_FONT as u8,
        records::NO_FONT as u8,
    ]);
    let compiled = Compiled {
        hud: "t",
        widgets: vec![
            records::Widget {
                id: "m",
                kind: WidgetKind::Bitmap,
                bytes: bitmap,
                crop: Some([0.5, 1.0, 0.0, 1.0]),
                ramp: Some([0.25, 0.75]),
            },
            records::Widget {
                id: "x",
                kind: WidgetKind::Text,
                bytes: text,
                crop: None,
                ramp: None,
            },
        ],
        channels: Vec::new(),
    };
    (hud, compiled)
}

fn install(m: &mut FlatMemory, hud: &Hud, c: &Compiled) -> Table {
    let joins = [Join {
        root: ROOT,
        huds: vec![(0, hud, c)],
    }];
    install_records(m, REGION, &joins).unwrap()
}

/// The root points at copies: the retail records byte for byte first, the
/// kit's after them, each block's count the sum; the header keeps what
/// the root held.
#[test]
fn the_kits_records_follow_the_retail_ones() {
    let mut m = memory();
    let (hud, c) = hud();
    let t = install(&mut m, &hud, &c);
    let bitmaps = m.u32(ROOT + records::ROOT_BITMAPS + 4).unwrap();
    let texts = m.u32(ROOT + records::ROOT_TEXTS + 4).unwrap();
    assert_eq!(bitmaps, REGION.0 + HEADER_LEN);
    assert_eq!(m.u32(ROOT + records::ROOT_BITMAPS).unwrap(), 3);
    assert_eq!(m.u32(ROOT + records::ROOT_TEXTS).unwrap(), 2);
    let b = records::BITMAP_SIZE as u32;
    let read = |m: &FlatMemory, at: u32, len: usize| {
        let mut v = vec![0; len];
        m.read(at, &mut v).unwrap();
        v
    };
    assert_eq!(read(&m, bitmaps, b as usize), vec![0xb0; b as usize]);
    assert_eq!(read(&m, bitmaps + b, b as usize), vec![0xb1; b as usize]);
    assert_eq!(read(&m, bitmaps + 2 * b, b as usize), c.widgets[0].bytes);
    assert_eq!(texts, bitmaps + 3 * b, "the text block after the bitmaps");
    let tl = records::TEXT_SIZE;
    assert_eq!(read(&m, texts, tl), vec![0x70; tl]);
    assert_eq!(read(&m, texts + tl as u32, tl), c.widgets[1].bytes);
    // The draw table: each kit widget at its address, with its modes and
    // fonts.
    assert_eq!(t.widgets.len(), 2);
    let meter = t.at(bitmaps + 2 * b).unwrap();
    assert_eq!((meter.id, meter.modes), ("m", [true; 3]));
    let text = t.at(texts + tl as u32).unwrap();
    assert_eq!((text.id, text.modes), ("x", [true, false, false]));
    assert_eq!(text.font, [8, -1, -1]);
    assert!(t.at(bitmaps).is_none(), "a retail widget is not the kit's");
    // The header.
    assert_eq!(m.u32(REGION.0).unwrap(), HEADER_MAGIC);
    assert_eq!(m.u32(REGION.0 + 4).unwrap(), 1);
    assert_eq!(m.u32(REGION.0 + 8).unwrap(), ROOT);
    assert_eq!(m.u32(REGION.0 + 12).unwrap(), 2);
    assert_eq!(m.u32(REGION.0 + 16).unwrap(), RETAIL_BITMAPS);
}

/// A second install puts the retail fields back first: one copy, the
/// same counts; `restore` alone gives the retail root back.
#[test]
fn a_second_install_leaves_one_copy() {
    let mut m = memory();
    let (hud, c) = hud();
    let first = install(&mut m, &hud, &c);
    let second = install(&mut m, &hud, &c);
    assert_eq!(first, second);
    assert_eq!(m.u32(ROOT + records::ROOT_BITMAPS).unwrap(), 3);
    assert_eq!(m.u32(ROOT + records::ROOT_TEXTS).unwrap(), 2);
    assert_eq!(restore(&mut m, REGION).unwrap(), 2);
    assert_eq!(m.u32(ROOT + records::ROOT_BITMAPS).unwrap(), 2);
    assert_eq!(
        m.u32(ROOT + records::ROOT_BITMAPS + 4).unwrap(),
        RETAIL_BITMAPS
    );
    assert_eq!(m.u32(ROOT + records::ROOT_TEXTS + 4).unwrap(), RETAIL_TEXTS);
    // A root the game reloaded is not touched.
    let mut m = memory();
    install(&mut m, &hud, &c);
    m.set_u32(ROOT + records::ROOT_BITMAPS + 4, RETAIL_BITMAPS)
        .unwrap();
    m.set_u32(ROOT + records::ROOT_BITMAPS, 2).unwrap();
    assert_eq!(restore(&mut m, REGION).unwrap(), 1, "only the texts' field");
}

#[test]
fn a_region_too_small_is_refused_with_the_root_retail() {
    let mut m = memory();
    let (hud, c) = hud();
    let joins = [Join {
        root: ROOT,
        huds: vec![(0, &hud, &c)],
    }];
    let e = install_records(&mut m, (REGION.0, HEADER_LEN + 8), &joins).unwrap_err();
    assert!(matches!(e, InstallError::Full { .. }), "{e:?}");
    assert_eq!(m.u32(ROOT + records::ROOT_BITMAPS).unwrap(), 2);
}

fn shown_values(fill: f32) -> impl Fn(usize, &str) -> Option<Value> {
    move |_, id| {
        Some(match id {
            "m" => Value::fill(fill),
            _ => Value::text("42"),
        })
    }
}

/// The gate shows a kit widget at its value, in its modes, in a style
/// the runtime draws; a retail widget or another caller is not answered.
#[test]
fn the_gate_answers_kit_widgets_only() {
    let mut m = memory();
    let (hud, c) = hud();
    let t = install(&mut m, &hud, &c);
    let meter = t.widgets[0].address;
    let text = t.widgets[1].address;
    m.set_u32(SP, GATE_RETURNS[0]).unwrap();
    let on = shown_values(1.0);
    assert_eq!(gate(&m, SP, meter + 8, &t, &on).unwrap(), Some(true));
    let hidden = |_: usize, _: &str| Some(Value::hidden());
    assert_eq!(gate(&m, SP, meter + 8, &t, &hidden).unwrap(), Some(false));
    let inactive = |_: usize, _: &str| Some(Value::shown().styled(Style::Inactive));
    assert_eq!(gate(&m, SP, meter + 8, &t, &inactive).unwrap(), Some(false));
    assert_eq!(gate(&m, SP, RETAIL_BITMAPS + 8, &t, &on).unwrap(), None);
    // Split screen: the text has no half offset.
    m.set_u16(GLOBALS + 8, 2).unwrap();
    assert_eq!(gate(&m, SP, text + 8, &t, &on).unwrap(), Some(false));
    assert_eq!(gate(&m, SP, meter + 8, &t, &on).unwrap(), Some(true));
    m.set_u32(SP, 0x1234).unwrap();
    assert_eq!(gate(&m, SP, meter + 8, &t, &on).unwrap(), None);
}

/// `hud_input1` is the fill through the meter's ramp, the rest 0.
#[test]
fn the_input_is_the_fill_through_the_ramp() {
    let mut m = memory();
    let (hud, c) = hud();
    let t = install(&mut m, &hud, &c);
    let out = SP + 0x100;
    m.set_u32(SP, INPUT_RETURNS[0]).unwrap();
    m.set_u32(SP + 8, t.widgets[0].address + 4).unwrap();
    m.set_u32(SP + 12, out).unwrap();
    m.fill(out, 16, 0xff).unwrap();
    assert!(inputs(&mut m, SP, &t, &shown_values(0.5)).unwrap());
    assert_eq!(m.f32(out).unwrap(), 0.5);
    for k in 1..4 {
        assert_eq!(m.u32(out + 4 * k).unwrap(), 0);
    }
    assert!(inputs(&mut m, SP, &t, &shown_values(1.0)).unwrap());
    assert_eq!(m.f32(out).unwrap(), 0.75);
    // A retail widget passes through.
    m.set_u32(SP + 8, RETAIL_BITMAPS + 4).unwrap();
    assert!(!inputs(&mut m, SP, &t, &shown_values(1.0)).unwrap());
}

/// The string hook writes the value's text in UTF-16 with its zero, the
/// element's engine font, and the inputs to the material's.
#[test]
fn the_string_is_the_values_text_in_its_font() {
    let mut m = memory();
    let (hud, c) = hud();
    let t = install(&mut m, &hud, &c);
    let (values, out, font) = (SP + 0x100, SP + 0x200, SP + 0x300);
    m.set_u32(SP, STRING_RETURN).unwrap();
    m.set_u32(SP + 8, values).unwrap();
    m.set_u32(SP + 12, out).unwrap();
    m.set_u32(SP + 16, font).unwrap();
    m.set_f32(values, 3.0).unwrap();
    let text = t.widgets[1].address;
    assert!(string(&mut m, SP, text, &t, &shown_values(0.0)).unwrap());
    let mut got = [0; 6];
    m.read(out, &mut got).unwrap();
    assert_eq!(got, [b'4', 0, b'2', 0, 0, 0]);
    assert_eq!(m.u32(font).unwrap(), 8);
    assert_eq!(m.f32(MATERIAL_INPUTS).unwrap(), 3.0);
    assert!(!string(&mut m, SP, t.widgets[0].address, &t, &shown_values(0.0)).unwrap());
    assert_eq!(utf16("abc", 2), vec![b'a', 0, 0, 0], "cut with its zero");
}

/// A crop narrows the sprite's bounds; the sprite hook reads the bitmap's
/// sequence and gives its image and size with the narrowed bounds, a
/// sequence without sprites included (the charge meter drew all 92
/// columns of its strip while such sequences were left to the game).
#[test]
fn the_crop_narrows_the_sprite() {
    assert_eq!(
        narrow([0.0, 0.8, 0.5, 1.0], [0.5, 1.0, 0.0, 0.5]),
        [0.4, 0.8, 0.5, 0.75]
    );
    let mut m = memory();
    let (hud, mut c) = hud();
    // The meter names bitmap BITMAP_DATUM.
    const BITMAP_DATUM: u32 = 0xe1f1_0001;
    let at = records::bitmap::BITMAP;
    c.widgets[0].bytes[at..at + 4].copy_from_slice(&BITM.to_le_bytes());
    c.widgets[0].bytes[at + 4..at + 8].copy_from_slice(&BITMAP_DATUM.to_le_bytes());
    let t = install(&mut m, &hud, &c);
    // A tag table of one bitmap.
    let (index, groups, entries, bitmap) = (0x0055_0000, 0x0055_0100, 0x0055_1000, 0x0055_2000);
    m.set_u32(TAG_INDEX_GLOBAL, index).unwrap();
    m.set_u32(index + index_layout::GROUPS, groups).unwrap();
    m.set_u32(index + index_layout::GROUP_COUNT, 1).unwrap();
    m.set_u32(index + index_layout::TAG_COUNT, 2).unwrap();
    m.set_u32(groups + group_layout::GROUP, BITM).unwrap();
    m.set_u32(groups + group_layout::PARENT, u32::MAX).unwrap();
    m.set_u32(groups + group_layout::GRANDPARENT, u32::MAX)
        .unwrap();
    m.set_u32(TAG_ENTRIES_GLOBAL, entries).unwrap();
    let e = entries + (BITMAP_DATUM & 0xffff) * entry_layout::STRIDE;
    m.set_u32(e + entry_layout::GROUP, BITM).unwrap();
    m.set_u32(e + entry_layout::DATUM, BITMAP_DATUM).unwrap();
    m.set_u32(e + entry_layout::ADDRESS, bitmap).unwrap();
    // One sequence of one sprite of image 1 (128 by 8), bounds [0, 0.72,
    // 0, 1].
    let (seqs, sprites, images) = (bitmap + 0x100, bitmap + 0x200, bitmap + 0x300);
    m.set_u32(bitmap + 0x3c, 1).unwrap();
    m.set_u32(bitmap + 0x40, seqs).unwrap();
    m.set_u32(bitmap + 0x44, 2).unwrap();
    m.set_u32(bitmap + 0x48, images).unwrap();
    m.set_u32(seqs + 0x34, 1).unwrap();
    m.set_u32(seqs + 0x38, sprites).unwrap();
    m.set_u16(sprites, 1).unwrap();
    for (i, v) in [0.0f32, 0.72, 0.0, 1.0].iter().enumerate() {
        m.set_f32(sprites + 8 + 4 * i as u32, *v).unwrap();
    }
    m.set_u16(images + 0x74 + 4, 128).unwrap();
    m.set_u16(images + 0x74 + 6, 8).unwrap();
    let (bounds, image, width, height) = (SP + 0x100, SP + 0x200, SP + 0x204, SP + 0x208);
    m.set_u32(SP, SPRITE_RETURN).unwrap();
    m.set_u32(SP + 8, image).unwrap();
    m.set_u32(SP + 12, width).unwrap();
    m.set_u32(SP + 16, height).unwrap();
    let meter = t.widgets[0].address;
    assert!(crop_sprite(&mut m, SP, meter, bounds, &t).unwrap());
    let b: Vec<f32> = (0..4).map(|i| m.f32(bounds + 4 * i).unwrap()).collect();
    assert_eq!(b, [0.36, 0.72, 0.0, 1.0]);
    assert_eq!(
        [
            m.u32(image).unwrap(),
            m.u32(width).unwrap(),
            m.u32(height).unwrap()
        ],
        [1, 128, 8]
    );
    // A sequence with no sprites draws its first image whole (the disc's
    // battery strip, 92 by 8): the crop narrows the whole image, so the
    // meter is the strip's last half, not all 92 columns.
    m.set_u32(seqs + 0x34, 0).unwrap();
    m.set_u16(seqs + 0x20, 0).unwrap();
    m.set_u16(images + 4, 92).unwrap();
    m.set_u16(images + 6, 8).unwrap();
    assert!(crop_sprite(&mut m, SP, meter, bounds, &t).unwrap());
    let b: Vec<f32> = (0..4).map(|i| m.f32(bounds + 4 * i).unwrap()).collect();
    assert_eq!(b, [0.5, 1.0, 0.0, 1.0]);
    assert_eq!(
        [
            m.u32(image).unwrap(),
            m.u32(width).unwrap(),
            m.u32(height).unwrap()
        ],
        [0, 92, 8]
    );
    // Its first image is its own `first_image`.
    m.set_u16(seqs + 0x20, 1).unwrap();
    assert!(crop_sprite(&mut m, SP, meter, bounds, &t).unwrap());
    assert_eq!(m.u32(image).unwrap(), 1);
    assert_eq!(
        sprite_pick(&m, bitmap, 0).unwrap(),
        Some((1, [0.0, 1.0, 0.0, 1.0]))
    );
    // No such sequence: image 0 whole, as the game draws it.
    assert_eq!(
        sprite_pick(&m, bitmap, 3).unwrap(),
        Some((0, [0.0, 1.0, 0.0, 1.0]))
    );
    assert_eq!(
        sprite_pick(&m, bitmap, -1).unwrap(),
        Some((0, [0.0, 1.0, 0.0, 1.0]))
    );
    // An image the bitmap does not have is left to the game.
    m.set_u16(seqs + 0x20, 2).unwrap();
    assert_eq!(sprite_pick(&m, bitmap, 0).unwrap(), None);
    assert!(!crop_sprite(&mut m, SP, meter, bounds, &t).unwrap());
    m.set_u16(seqs + 0x20, 0).unwrap();
    // An uncropped widget, or another caller, is left to the game.
    assert!(!crop_sprite(&mut m, SP, t.widgets[1].address, bounds, &t).unwrap());
    m.set_u32(SP, 0x1234).unwrap();
    assert!(!crop_sprite(&mut m, SP, meter, bounds, &t).unwrap());
}

/// The prompt fields as `0x0024ca1d` leaves them.
#[test]
fn the_prompt_is_written_as_the_game_writes_its_own() {
    let mut m = memory();
    let huds = 0x0055_8000;
    m.set_u32(USER_HUDS, huds).unwrap();
    m.set_u32(huds + PROMPT_ID, 7).unwrap();
    show_prompt(&mut m, 0, "Let go").unwrap();
    let mut got = [0; 4];
    m.read(huds + PROMPT_TEXT, &mut got).unwrap();
    assert_eq!(got, [b'L', 0, b'e', 0]);
    assert_eq!(m.u32(huds + PROMPT_ID).unwrap(), 0);
    assert_eq!(m.u8(huds + PROMPT_SHOWN).unwrap(), 1);
    assert_eq!(m.u8(huds + 0x44e).unwrap() & 1, 1, "the fade restarts");
}

/// The training id and show byte are set and cleared; the game's own
/// scripted text is not overwritten; the lookup answers only our id.
#[test]
fn the_training_slot_is_ours_only_when_free() {
    let mut m = memory();
    let state = 0x0055_9000;
    m.set_u32(TRAINING, state).unwrap();
    assert!(write_training(&mut m, true).unwrap());
    assert_eq!(m.u32(state + TRAINING_ID).unwrap(), TRAINING_TEXT_ID);
    assert_eq!(m.u8(state + TRAINING_SHOWN).unwrap(), 1);
    assert!(!write_training(&mut m, false).unwrap());
    assert_eq!(m.u8(state + TRAINING_SHOWN).unwrap(), 0);
    assert_eq!(m.u32(state + TRAINING_ID).unwrap(), 0);
    // The game's own.
    m.set_u32(state + TRAINING_ID, 0x1234).unwrap();
    m.set_u8(state + TRAINING_SHOWN, 1).unwrap();
    assert!(!write_training(&mut m, true).unwrap());
    assert_eq!(m.u32(state + TRAINING_ID).unwrap(), 0x1234);
    assert!(!write_training(&mut m, false).unwrap());
    assert_eq!(m.u8(state + TRAINING_SHOWN).unwrap(), 1);
    assert!(owns(TRAINING_TEXT_ID) && !owns(0x1234));
    assert_eq!(text(0x1234), None);
    assert!(text(TRAINING_TEXT_ID).is_some());
}

/// Our point follows the scenario's, the navpoint list gets one slot on
/// it, and it is skipped when the HUD has no target; a second install
/// leaves one copy.
#[test]
fn the_navpoint_is_a_scenario_point_of_ours() {
    let mut m = memory();
    let (scenario, block, list) = (0x0055_a000, 0x0055_b000, 0x0055_c000);
    m.set_u32(SCENARIO, scenario).unwrap();
    m.set_u32(scenario + SCENARIO_POINTS, 2).unwrap();
    m.set_u32(scenario + SCENARIO_POINTS + 4, block).unwrap();
    m.fill(block, 2 * POINT_LEN, 0x5c).unwrap();
    let h = install_point(&mut m, POINTS).unwrap().unwrap();
    assert_eq!(h.index, 2);
    assert_eq!(m.u32(scenario + SCENARIO_POINTS).unwrap(), 3);
    let copy = m.u32(scenario + SCENARIO_POINTS + 4).unwrap();
    assert_eq!(copy, POINTS.0 + POINT_HEADER_LEN);
    assert_eq!(m.u8(copy + 2 * POINT_LEN - 1).unwrap(), 0x5c);
    assert_eq!(m.u8(copy + 2 * POINT_LEN).unwrap(), 0);
    // Again: one copy.
    assert_eq!(install_point(&mut m, POINTS).unwrap().unwrap(), h);
    assert_eq!(m.u32(scenario + SCENARIO_POINTS).unwrap(), 3);
    // The game's list: one navpoint of its own, then the end.
    m.set_u32(NAVPOINTS, list).unwrap();
    m.set_u16(list, 3).unwrap();
    m.set_u16(list + 2, 1).unwrap();
    m.set_u16(list + NAV_SLOT_LEN, u16::MAX).unwrap();
    let slot = write_navpoint(&mut m, POINTS, 0, Some([1.0, 2.0, 3.0])).unwrap();
    assert_eq!(slot, Some(1));
    let s = list + NAV_SLOT_LEN;
    assert_eq!(m.u16(s).unwrap(), 0);
    assert_eq!(m.u16(s + 2).unwrap(), NAV_SCENARIO_POINT);
    assert_eq!(m.u32(s + 8).unwrap(), 2);
    assert_eq!(
        m.u16(s + NAV_SLOT_LEN).unwrap(),
        u16::MAX,
        "the list ends after ours"
    );
    assert_eq!(m.f32(copy + 2 * POINT_LEN + POINT_AT + 8).unwrap(), 3.0);
    assert_eq!(m.u16(list + 2).unwrap(), 1, "the game's slot is kept");
    // The same slot again, then skipped.
    assert_eq!(
        write_navpoint(&mut m, POINTS, 0, Some([0.0; 3])).unwrap(),
        Some(1)
    );
    assert_eq!(write_navpoint(&mut m, POINTS, 0, None).unwrap(), None);
    assert_eq!(m.u16(s + 2).unwrap() & 0xf, NAV_SKIP);
    // And taken again from the skipped slot.
    assert_eq!(
        write_navpoint(&mut m, POINTS, 0, Some([0.0; 3])).unwrap(),
        Some(1)
    );
}

#[test]
fn only_normal_and_flash_are_drawn() {
    assert!(draws(Style::Normal));
    assert!(draws(Style::Flash(hud_kit_core::model::Pulse::Help)));
    assert!(!draws(Style::Dim));
    assert!(!draws(Style::Inactive));
}

/// A notification is a call to the game's insert with the user in EAX
/// and the text's address its one argument; the text is UTF-16 with its
/// zero, one slot each.
#[test]
fn a_notification_is_the_inserts_call_with_its_text() {
    let mut m = memory();
    let region = (0x0058_0000, NOTES_LEN);
    let notes = [
        Note {
            user: 0,
            text: "Primed".into(),
        },
        Note {
            user: 2,
            text: "\u{e2a1} Double bounce!".into(),
        },
    ];
    let calls = stage_notes(&mut m, region, &notes).unwrap();
    assert_eq!(calls.len(), 2);
    for (i, (c, n)) in calls.iter().zip(&notes).enumerate() {
        let at = region.0 + i as u32 * NOTE_LEN;
        assert_eq!(c.target, NOTIFY);
        assert_eq!(c.target, 0x0024_cbee);
        assert_eq!(c.eax, Some(n.user));
        assert_eq!(c.args, vec![at]);
        let units: Vec<u16> = n.text.encode_utf16().chain([0]).collect();
        for (k, u) in units.iter().enumerate() {
            assert_eq!(m.u16(at + 2 * k as u32).unwrap(), *u, "note {i} unit {k}");
        }
    }
    // The medal's glyph is one unit, at the line's start.
    assert_eq!(m.u16(region.0 + NOTE_LEN).unwrap(), 0xe2a1);
    // A region of one slot sends one; the rest wait.
    let one = stage_notes(&mut m, (region.0, NOTE_LEN), &notes).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(NOTE_LEN, 128);
    assert!(NOTES_PER_TICK >= 16);
}

/// The insert copies 63 units: a longer text, a second line, an empty
/// text and a fifth user are refused, not cut.
#[test]
fn a_notification_the_game_cannot_keep_is_refused() {
    assert_eq!(note_fits(0, &"a".repeat(63)), Ok(()));
    assert!(note_fits(0, &"a".repeat(64)).is_err());
    assert!(note_fits(0, "one\ntwo").is_err());
    assert!(note_fits(0, "").is_err());
    assert!(note_fits(USERS, "Primed").is_err());
    let h = Handle(usize::MAX);
    assert!(!h.notify(0, &"a".repeat(64)));
    assert!(h.notify(3, "Bounced!"));
    let queued = take_notes();
    assert!(queued.contains(&Note {
        user: 3,
        text: "Bounced!".into()
    }));
}
