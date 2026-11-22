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
    let mut m = FlatMemory::new(BASE, 0x0020_0000);
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

/// A widget whose element gives way to word D bits is hidden while the
/// game's word D (the gate's third stack argument) has one, and shown at
/// its value otherwise; a widget without such bits ignores the word.
#[test]
fn the_gate_gives_way_to_word_d() {
    let mut m = memory();
    let (mut hud, c) = hud();
    hud.elements[0] = hud.elements[0].clone().hidden_on_d(0x400);
    let t = install(&mut m, &hud, &c);
    let meter = t.widgets[0].address;
    assert_eq!(t.widgets[0].hide_d, 0x400);
    assert_eq!(t.widgets[1].hide_d, 0);
    m.set_u32(SP, GATE_RETURNS[1]).unwrap();
    let on = shown_values(1.0);
    assert_eq!(gate(&m, SP, meter + 8, &t, &on).unwrap(), Some(true));
    m.set_u32(SP + 12, 0x0401).unwrap();
    assert_eq!(gate(&m, SP, meter + 8, &t, &on).unwrap(), Some(false));
    m.set_u32(SP + 12, 0x0001).unwrap();
    assert_eq!(gate(&m, SP, meter + 8, &t, &on).unwrap(), Some(true));
    // The high half of the argument is not word D.
    m.set_u32(SP + 12, 0x0400_0000).unwrap();
    assert_eq!(gate(&m, SP, meter + 8, &t, &on).unwrap(), Some(true));
    m.set_u32(SP + 12, 0x0400).unwrap();
    let text = t.widgets[1].address;
    assert_eq!(gate(&m, SP, text + 8, &t, &on).unwrap(), Some(true));
}

/// Publishing stamps a moving value that changed, from the fill it was
/// showing; the same value again keeps its stamp; the draw hooks read it
/// on its curve at the time since.
#[test]
fn a_moving_value_is_stamped_on_change_and_drawn_on_its_curve() {
    use hud_kit_core::motion::{EASE_MS, ending};
    use std::time::Duration;
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let eased = |fill| Values(vec![("m", Value::fill(fill).moving(Motion::Ease))]);
    let (v1, s1) = stamp_values(&Values::default(), &[], eased(1.0 / 3.0), at(0), true);
    assert_eq!(s1, vec![("m", at(0))]);
    assert_eq!(v1.get("m").unwrap().from, 0.0);
    // The same value a tick later: the stamp holds.
    let (v2, s2) = stamp_values(&v1, &s1, eased(1.0 / 3.0), at(33), true);
    assert_eq!((v2.clone(), s2.clone()), (v1.clone(), s1.clone()));
    // Drawn half way through the ease: half way to 1/3.
    let half = animated(v2.get("m").unwrap().clone(), Some(at(0)), at(100), true);
    assert!((half.fill - 1.0 / 6.0).abs() < 1e-4, "{}", half.fill);
    // On to 2/3 at the end of the ease: from 1/3, stamped then.
    let (v3, s3) = stamp_values(&v2, &s2, eased(2.0 / 3.0), at(EASE_MS as u64), true);
    assert_eq!(s3, vec![("m", at(EASE_MS as u64))]);
    assert!((v3.get("m").unwrap().from - 1.0 / 3.0).abs() < 1e-6);
    // The ending: hidden in its blink, scaled up in its swell.
    let ending_v = Values(vec![("m", Value::fill(1.0).moving(Motion::Ending))]);
    let (v4, s4) = stamp_values(&v3, &s3, ending_v, at(1000), true);
    let e = v4.get("m").unwrap().clone();
    let blink = ending::BLINK_START as u64 + 1000 + 10;
    assert!(!animated(e.clone(), Some(s4[0].1), at(blink), true).shown);
    assert!(animated(e.clone(), Some(s4[0].1), at(1000 + 500), true).shown);
    assert!(
        !animated(e, Some(s4[0].1), at(1000 + 500), false).shown,
        "the plain ending without the scale"
    );
    // A value with no motion carries no stamp and is drawn as given.
    let still = Values(vec![("m", Value::fill(0.4))]);
    let (v5, s5) = stamp_values(&v4, &s4, still, at(2000), true);
    assert!(s5.is_empty());
    assert_eq!(
        animated(v5.get("m").unwrap().clone(), None, at(9000), true).fill,
        0.4
    );
}

/// The effects hook scales a kit bitmap widget's two basis factors by
/// its scale, at the bitmap draw's call only; a retail widget, another
/// call, or a factor that is no positive number is left.
#[test]
fn the_effects_hook_scales_the_basis() {
    let mut m = memory();
    let (hud, c) = hud();
    let t = install(&mut m, &hud, &c);
    let frame = SP + 0x400;
    let set = |m: &mut FlatMemory, widget: u32| {
        m.set_u32(SP, EFFECTS_RETURN).unwrap();
        m.set_u32(frame + FRAME_WIDGET, widget).unwrap();
        m.set_f32(frame + BASIS[0], 1.0).unwrap();
        m.set_f32(frame + BASIS[1], 0.5).unwrap();
    };
    let swell = |_: usize, _: &str| [1.12f32, 1.3];
    let basis = |m: &FlatMemory| {
        [
            m.f32(frame + BASIS[0]).unwrap(),
            m.f32(frame + BASIS[1]).unwrap(),
        ]
    };
    set(&mut m, t.widgets[0].address);
    assert!(scale_basis(&mut m, SP, frame, &t, &swell).unwrap());
    assert_eq!(basis(&m), [1.12, 0.65]);
    // Its own size: seen, nothing written.
    set(&mut m, t.widgets[0].address);
    assert!(scale_basis(&mut m, SP, frame, &t, &|_, _| [1.0, 1.0]).unwrap());
    assert_eq!(basis(&m), [1.0, 0.5]);
    // A retail widget, another call, a text.
    set(&mut m, RETAIL_BITMAPS);
    assert!(!scale_basis(&mut m, SP, frame, &t, &swell).unwrap());
    assert_eq!(basis(&m), [1.0, 0.5]);
    set(&mut m, t.widgets[0].address);
    m.set_u32(SP, SPRITE_RETURN).unwrap();
    assert!(!scale_basis(&mut m, SP, frame, &t, &swell).unwrap());
    set(&mut m, t.widgets[1].address);
    assert!(!scale_basis(&mut m, SP, frame, &t, &swell).unwrap());
    // A factor the frame does not hold as a positive number.
    set(&mut m, t.widgets[0].address);
    m.set_f32(frame + BASIS[1], f32::NAN).unwrap();
    assert!(scale_basis(&mut m, SP, frame, &t, &swell).unwrap());
    assert_eq!(m.f32(frame + BASIS[0]).unwrap(), 1.12);
    assert!(m.f32(frame + BASIS[1]).unwrap().is_nan());
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

/// The slots the game's navpoint draw draws, and their points.
type Drawn = Vec<(u32, Option<[f32; 3]>)>;

/// The game's navpoint draw for `user` (E `0x0024da7d`), with its own
/// constants rather than the kit's: all four slots of `[0x005023f8] +
/// user * 0x30`; a slot whose `+0` or `+8` is -1, or whose kind is 15,
/// is marked kind 15 and passed (`0x0024dc3a`); a kind-0 slot reads its
/// point at `[[0x004e0350] + 0x1e4] + index * 0x38 + 0x24`, with no check
/// of the index (`0x0024db56`..`0x0024db6b`), plus the slot's height. A
/// slot of another kind is an object's (`0x000badc0`), not modelled:
/// `None`. Returns each drawn slot and its point.
fn game_draws(m: &mut FlatMemory, user: u32) -> Result<Drawn, MemFault> {
    let list = m.u32(0x0050_23f8)? + user * 0x30;
    let mut drawn = Vec::new();
    for i in 0..4 {
        let slot = list + i * 12;
        let kind = m.u16(slot + 2)?;
        if m.u16(slot)? == 0xffff || m.u32(slot + 8)? == u32::MAX || kind & 0xf == 0xf {
            m.set_u16(slot + 2, kind | 0xf)?;
            continue;
        }
        if kind & 0xf != 0 {
            drawn.push((i, None));
            continue;
        }
        let block = m.u32(m.u32(0x004e_0350)? + 0x1e4)?;
        let p = (m.u32(slot + 8)?.wrapping_mul(0x38)).wrapping_add(block).wrapping_add(0x24);
        let z = m.f32(p + 8)? + m.f32(slot + 4)?;
        drawn.push((i, Some([m.f32(p)?, m.f32(p + 4)?, z])));
    }
    Ok(drawn)
}

/// Our point follows the scenario's (the tag block at `scnr +0x1e0`, the
/// block next to it untouched), the game's draw reads it through one
/// slot of the navpoint list, and the slot is skipped when the HUD has no
/// target; a second install leaves one copy.
#[test]
fn the_navpoint_is_a_scenario_point_of_ours() {
    let mut m = memory();
    let (scenario, block, list) = (0x0055_a000, 0x0055_b000, 0x0055_c000);
    m.set_u32(SCENARIO, scenario).unwrap();
    m.set_u32(scenario + 0x1e0, 2).unwrap();
    m.set_u32(scenario + 0x1e4, block).unwrap();
    // The next block (scnr +0x1e8, 0x40-byte elements) is not ours.
    m.set_u32(scenario + 0x1e8, 7).unwrap();
    m.set_u32(scenario + 0x1ec, 0x0055_d000).unwrap();
    m.fill(block, 2 * POINT_LEN, 0x5c).unwrap();
    let h = install_point(&mut m, POINTS).unwrap().unwrap();
    assert_eq!(h.index, 2);
    assert_eq!(m.u32(scenario + 0x1e0).unwrap(), 2 + KIT_POINTS);
    let copy = m.u32(scenario + 0x1e4).unwrap();
    assert_eq!(copy, POINTS.0 + POINT_HEADER_LEN);
    assert_eq!(m.u8(copy + 2 * POINT_LEN - 1).unwrap(), 0x5c);
    assert_eq!(m.u8(copy + 2 * POINT_LEN).unwrap(), 0);
    assert_eq!(m.u32(scenario + 0x1e8).unwrap(), 7, "the next block's count");
    assert_eq!(m.u32(scenario + 0x1ec).unwrap(), 0x0055_d000);
    // Again: one copy.
    assert_eq!(install_point(&mut m, POINTS).unwrap().unwrap(), h);
    assert_eq!(m.u32(scenario + 0x1e0).unwrap(), 2 + KIT_POINTS);
    assert_eq!(m.u32(scenario + 0x1e4).unwrap(), copy);
    // The game's list: an empty slot, then an object navpoint of its own,
    // then two more empty slots.
    m.set_u32(NAVPOINTS, list).unwrap();
    for i in 0..4 {
        m.set_u16(list + i * NAV_SLOT_LEN, u16::MAX).unwrap();
    }
    let theirs = list + NAV_SLOT_LEN;
    m.set_u16(theirs, 3).unwrap();
    m.set_u16(theirs + 2, 1).unwrap();
    m.set_u32(theirs + 8, 0x1234_0005).unwrap();
    let slot = write_navpoint(&mut m, POINTS, 0, Some([1.0, 2.0, 3.0])).unwrap();
    assert_eq!(slot, Some(0), "the first slot the draw passes");
    assert_eq!(
        game_draws(&mut m, 0).unwrap(),
        vec![(0, Some([1.0, 2.0, 3.0])), (1, None)],
        "the game draws ours, and its own after it"
    );
    assert_eq!(m.u16(theirs).unwrap(), 3, "the game's slot is kept");
    assert_eq!(m.u16(theirs + 2).unwrap(), 1);
    // The same slot again, then skipped.
    assert_eq!(
        write_navpoint(&mut m, POINTS, 0, Some([4.0, 5.0, 6.0])).unwrap(),
        Some(0)
    );
    assert_eq!(game_draws(&mut m, 0).unwrap()[0], (0, Some([4.0, 5.0, 6.0])));
    assert_eq!(write_navpoint(&mut m, POINTS, 0, None).unwrap(), None);
    assert_eq!(m.u16(list + 2).unwrap() & 0xf, NAV_SKIP);
    assert_eq!(game_draws(&mut m, 0).unwrap(), vec![(1, None)]);
    // And taken again from the skipped slot.
    assert_eq!(
        write_navpoint(&mut m, POINTS, 0, Some([0.0; 3])).unwrap(),
        Some(0)
    );
}

/// The kit has a point for every slot of a user's list: each of our
/// navpoints keeps its own point and slot, takes the first slot the draw
/// passes, leaves the game's alone, and is skipped on its own.
#[test]
fn four_navpoints_each_keep_a_point_and_a_slot() {
    let mut m = memory();
    let (scenario, block, list) = (0x0055_a000, 0x0055_b000, 0x0055_c000);
    m.set_u32(SCENARIO, scenario).unwrap();
    m.set_u32(scenario + 0x1e0, 2).unwrap();
    m.set_u32(scenario + 0x1e4, block).unwrap();
    m.fill(block, 2 * POINT_LEN, 0x5c).unwrap();
    let h = install_point(&mut m, POINTS).unwrap().unwrap();
    assert_eq!(h.index, 2);
    let copy = m.u32(scenario + 0x1e4).unwrap();
    for k in 0..KIT_POINTS * POINT_LEN {
        assert_eq!(m.u8(copy + 2 * POINT_LEN + k).unwrap(), 0, "our points start empty");
    }
    m.set_u32(NAVPOINTS, list).unwrap();
    for i in 0..4 {
        m.set_u16(list + i * NAV_SLOT_LEN, u16::MAX).unwrap();
    }
    // The game has slot 1.
    let theirs = list + NAV_SLOT_LEN;
    m.set_u16(theirs, 3).unwrap();
    m.set_u16(theirs + 2, 1).unwrap();
    m.set_u32(theirs + 8, 0x1234_0005).unwrap();
    let p = |k: u32| [k as f32, 10.0 + k as f32, 20.0 + k as f32];
    assert_eq!(write_navpoint_n(&mut m, POINTS, 0, 0, Some(p(0))).unwrap(), Some(0));
    assert_eq!(write_navpoint_n(&mut m, POINTS, 0, 1, Some(p(1))).unwrap(), Some(2));
    assert_eq!(write_navpoint_n(&mut m, POINTS, 0, 2, Some(p(2))).unwrap(), Some(3));
    // No slot is left for the fourth: it is not drawn, and takes no one's.
    assert_eq!(write_navpoint_n(&mut m, POINTS, 0, 3, Some(p(3))).unwrap(), None);
    assert_eq!(write_navpoint_n(&mut m, POINTS, 0, KIT_POINTS, Some(p(9))).unwrap(), None);
    assert_eq!(
        game_draws(&mut m, 0).unwrap(),
        vec![(0, Some(p(0))), (1, None), (2, Some(p(1))), (3, Some(p(2)))]
    );
    assert_eq!(m.u32(theirs + 8).unwrap(), 0x1234_0005, "the game's slot is kept");
    // Each keeps its slot as it moves; one skipped frees its slot alone.
    assert_eq!(write_navpoint_n(&mut m, POINTS, 0, 1, Some(p(5))).unwrap(), Some(2));
    assert_eq!(write_navpoint_n(&mut m, POINTS, 0, 0, None).unwrap(), None);
    assert_eq!(
        game_draws(&mut m, 0).unwrap(),
        vec![(1, None), (2, Some(p(5))), (3, Some(p(2)))]
    );
    // The fourth takes the slot the first gave up.
    assert_eq!(write_navpoint_n(&mut m, POINTS, 0, 3, Some(p(3))).unwrap(), Some(0));
    assert_eq!(game_draws(&mut m, 0).unwrap()[0], (0, Some(p(3))));
    // The first point is `write_navpoint`'s.
    assert_eq!(write_navpoint(&mut m, POINTS, 0, Some(p(0))).unwrap(), None, "no slot free");
    assert_eq!(write_navpoint_n(&mut m, POINTS, 0, 3, None).unwrap(), None);
    assert_eq!(write_navpoint(&mut m, POINTS, 0, Some(p(0))).unwrap(), Some(0));
}

/// On an empty point block (Ascension's) the game's draw still reads our
/// point, not the block's count as an address (the crash on master
/// 1afa616: a read of 4 bytes at 0x25, pc `0x0024db6b`).
#[test]
fn the_navpoint_on_an_empty_point_block_reads_our_point() {
    let mut m = memory();
    let (scenario, list) = (0x0055_a000, 0x0055_c000);
    m.set_u32(SCENARIO, scenario).unwrap();
    let h = install_point(&mut m, POINTS).unwrap().unwrap();
    assert_eq!(h.index, 0);
    m.set_u32(NAVPOINTS, list).unwrap();
    for i in 0..4 {
        m.set_u16(list + i * NAV_SLOT_LEN, u16::MAX).unwrap();
    }
    assert_eq!(
        write_navpoint(&mut m, POINTS, 0, Some([7.0, 8.0, 9.0])).unwrap(),
        Some(0)
    );
    assert_eq!(game_draws(&mut m, 0).unwrap(), vec![(0, Some([7.0, 8.0, 9.0]))]);
}

/// Once the loaded scenario no longer points at our copy (another map,
/// or the block put back), our slot is skipped and nothing is written
/// through the old scenario.
#[test]
fn the_navpoint_is_skipped_when_our_point_is_gone() {
    let mut m = memory();
    let (scenario, block, list) = (0x0055_a000, 0x0055_b000, 0x0055_c000);
    m.set_u32(SCENARIO, scenario).unwrap();
    m.set_u32(scenario + 0x1e0, 1).unwrap();
    m.set_u32(scenario + 0x1e4, block).unwrap();
    install_point(&mut m, POINTS).unwrap().unwrap();
    m.set_u32(NAVPOINTS, list).unwrap();
    for i in 0..4 {
        m.set_u16(list + i * NAV_SLOT_LEN, u16::MAX).unwrap();
    }
    assert_eq!(write_navpoint(&mut m, POINTS, 0, Some([1.0; 3])).unwrap(), Some(0));
    // Another map's scenario, its block where the old one was.
    let other = 0x0055_e000;
    m.set_u32(SCENARIO, other).unwrap();
    m.set_u32(other + 0x1e0, 1).unwrap();
    m.set_u32(other + 0x1e4, block).unwrap();
    assert_eq!(write_navpoint(&mut m, POINTS, 0, Some([1.0; 3])).unwrap(), None);
    assert!(game_draws(&mut m, 0).unwrap().is_empty(), "our slot is skipped");
    // The old scenario's field is left alone at the next install.
    m.set_u32(scenario + 0x1e0, 0xdead).unwrap();
    let h = install_point(&mut m, POINTS).unwrap().unwrap();
    assert_eq!(m.u32(scenario + 0x1e0).unwrap(), 0xdead);
    assert_eq!((h.scenario, h.index), (other, 1));
}

/// The training text reaches the game with `\r\n` breaks, as the game's
/// strings have them: a lone `\n` drew "lip,let go" in the superbounce
/// tutorial.
#[test]
fn the_training_text_breaks_lines_as_the_game_does() {
    let first = hud_kit_core::huds::superbounce::TUTORIAL_TEXT[0];
    assert_eq!(
        game_line_breaks(first),
        "Crouch, walk in under a lip,\r\nlet go, then walk out."
    );
    assert_eq!(game_line_breaks("a\r\nb\nc"), "a\r\nb\r\nc");
    assert_eq!(game_line_breaks("one line"), "one line");
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
