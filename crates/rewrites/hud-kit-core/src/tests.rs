//! The core without a map: the record bytes at the draw's offsets, the
//! superbounce HUD's arming bar and the kit's pill block
//! (`views::pill_block`) compiled against templates resolved from an
//! invented `nhdt` through a [`MapView`], and the cropped meter's ramp.

use map::{FourCC, TagId};

use crate::views::pill_block::{self, CHARGE, CHARGE_COLUMNS, CHARGE_DOWN};
use crate::views::superbounce;
use crate::model::*;
use crate::records::{
    self, BITMAP_SIZE, KIT_INPUT, NO_FONT, TEXT_SIZE, WidgetKind, bitmap as bf, text as tf,
};
use crate::templates::{
    self, ArrowRef, ArtTemplate, FRAG_SELECTED, GrenadeRow, MapView, PLASMA_SELECTED, RawHud,
    RawWidget, Templates, TextTemplate,
};
use crate::{Mode, compile};

const SHAD: FourCC = FourCC(*b"shad");

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn art(bitmap: u32, crop: Option<[f32; 4]>) -> ArtTemplate {
    ArtTemplate {
        bitmap: TagId(bitmap),
        shader: (SHAD, TagId(0xe000_0002)),
        sequences: [3, 4, 5],
        crop,
        ramp: None,
    }
}

fn templates(art_list: Vec<(Art, Result<ArtTemplate, String>)>) -> Templates {
    Templates {
        nhdt: TagId(0xe000_0001),
        text: Ok(TextTemplate {
            shader: (SHAD, TagId(0xe000_0003)),
            string: 0x0400_0123,
        }),
        art: art_list,
        row: Ok(GrenadeRow {
            pill_anchor: 1,
            pill: [(106, 0), (80, 1), (60, 2)],
            count_anchor: 1,
            count: [(118, -1), (90, -2), (70, -3)],
            panel: 103.0,
            frag: [53.0, 50.0],
            step: [(53, 0), (40, 0), (30, 0)],
        }),
    }
}

/// A place from its own anchor fullscreen and from the grenade row in
/// split screen (`Place::split_origin`): the fullscreen offset as given,
/// the split ones added to the row's point in each mode, in the row's
/// anchor; with another anchor it is an error, since a widget has one.
#[test]
fn a_place_can_be_from_the_grenade_row_in_split_screen_alone() {
    let t = templates(Vec::new());
    let at = Place::new(Anchor::TopLeft, (90, 340))
        .split((20, 0), (20, 0))
        .split_from(Origin::NextGrenadePill);
    assert_eq!(
        records::placed(&at, &t).unwrap(),
        (1, [(90, 340), (80 + 20, 1), (60 + 20, 2)])
    );
    let count = Place::new(Anchor::TopLeft, (102, 339))
        .split((0, 0), (0, 0))
        .split_from(Origin::NextGrenadeCount);
    assert_eq!(
        records::placed(&count, &t).unwrap(),
        (1, [(102, 339), (90, -2), (70, -3)])
    );
    // The row in every mode, as before.
    let row = Place::in_grenade_row(Origin::NextGrenadePill);
    assert_eq!(
        records::placed(&row, &t).unwrap(),
        (1, [(106, 0), (80, 1), (60, 2)])
    );
    // Its own anchor in every mode: no row needed.
    let own = Place::new(Anchor::BottomLeft, (1, 2)).split((3, 4), (5, 6));
    assert_eq!(records::placed(&own, &t).unwrap(), (2, [(1, 2), (3, 4), (5, 6)]));
    let wrong = Place::new(Anchor::BottomLeft, (90, -68))
        .split((0, 0), (0, 0))
        .split_from(Origin::NextGrenadePill);
    let e = records::placed(&wrong, &t).unwrap_err();
    assert!(e.to_string().contains("one anchor"), "{e}");
}

/// A bitmap record: the anchor and an offset per mode, both tag
/// references (group then datum), the sequences and registrations, and
/// nothing for the retail gate or inputs to act on.
#[test]
fn a_bitmap_record_has_its_fields_at_the_draws_offsets() {
    let hud = Hud {
        name: "t",
        elements: vec![Element::meter(
            "m",
            Art::StripMeter,
            Place::new(Anchor::BottomLeft, (-1, -124))
                .registered([0.25, 0.75])
                .split((3, -60), (2, -30)),
        )],
    };
    let t = templates(vec![(Art::StripMeter, Ok(art(0xe000_0010, None)))]);
    let c = compile(&hud, &t).unwrap();
    let w = &c.widgets[0];
    let b = &w.bytes;
    assert_eq!((w.kind, b.len()), (WidgetKind::Bitmap, BITMAP_SIZE));
    assert_eq!(u16_at(b, bf::ANCHOR), Anchor::BottomLeft as u16);
    let offsets: Vec<_> = bf::OFFSETS
        .iter()
        .map(|o| (u16_at(b, *o) as i16, u16_at(b, o + 2) as i16))
        .collect();
    assert_eq!(offsets, [(-1, -124), (3, -60), (2, -30)]);
    assert_eq!(u32_at(b, bf::BITMAP), templates::BITM.as_u32());
    assert_eq!(u32_at(b, bf::BITMAP + 4), 0xe000_0010);
    assert_eq!(u32_at(b, bf::SHADER), SHAD.as_u32());
    assert_eq!(u32_at(b, bf::SHADER + 4), 0xe000_0002);
    assert_eq!(&b[bf::SEQUENCES..bf::SEQUENCES + 3], &[3, 4, 5]);
    for r in bf::REGISTRATIONS {
        assert_eq!([f32_at(b, r), f32_at(b, r + 4)], [0.25, 0.75]);
    }
    assert_eq!(&b[bf::INPUTS..bf::INPUTS + 4], &[KIT_INPUT; 4]);
    assert_eq!(&b[bf::STATE..bf::STATE_END], &[0; 20], "no show word");
    assert_eq!(u16_at(b, bf::SPECIAL), 0, "not the shield meter");
    assert_eq!(u32_at(b, bf::EFFECTS), 0, "no effects");
    assert_eq!(u32_at(b, bf::NAME), 0);
    assert_eq!(w.crop, None);
}

/// A text record: its font per mode (none where it is hidden), the
/// template's shader and string, the justification.
#[test]
fn a_text_record_has_its_fields_at_the_draws_offsets() {
    let hud = Hud {
        name: "t",
        elements: vec![
            Element::text(
                "t",
                Font::Lcd,
                Justify::End,
                "99",
                Place::new(Anchor::TopRight, (-10, 20)),
            ),
            Element::channel("c", Kind::Prompt { longest: "x" }),
        ],
    };
    let t = templates(Vec::new());
    let c = compile(&hud, &t).unwrap();
    assert_eq!(c.channels, ["c"]);
    let w = &c.widgets[0];
    let b = &w.bytes;
    assert_eq!((w.kind, b.len()), (WidgetKind::Text, TEXT_SIZE));
    assert_eq!(u16_at(b, tf::ANCHOR), Anchor::TopRight as u16);
    assert_eq!(
        (
            u16_at(b, tf::OFFSETS[0]) as i16,
            u16_at(b, tf::OFFSETS[0] + 2) as i16
        ),
        (-10, 20)
    );
    // A hidden mode keeps the fullscreen offset; its font is none.
    assert_eq!(u16_at(b, tf::OFFSETS[1]) as i16, -10);
    assert_eq!(
        [
            b[tf::FONTS] as i8,
            b[tf::FONTS + 1] as i8,
            b[tf::FONTS + 2] as i8
        ],
        [Font::Lcd.byte(), NO_FONT, NO_FONT]
    );
    assert_eq!(u32_at(b, tf::SHADER + 4), 0xe000_0003);
    assert_eq!(u32_at(b, tf::STRING), 0x0400_0123);
    assert_eq!(u16_at(b, tf::JUSTIFICATION), Justify::End as u16);
    assert_eq!(u16_at(b, tf::FLAGS), 0, "not a number");
    assert_eq!(&b[tf::STATE..tf::STATE_END], &[0; 20]);
}

/// An invented player `nhdt` with retail's grenade row: one 103-wide
/// panel drawn by both pill widgets at (0, 0), the counts at (65, -1) and
/// (12, -1); the strip, 92 wide, its blue rising from column 13; and the
/// scoreboard `nhdt` with Assault's arming bar, `bomb_defusal` (input
/// `0x43`, shown on word D `0x400`, 140 by 13), the arming bar's art.
struct Invented;

const PANEL: u32 = 0xe000_0020;
const STRIP: u32 = 0xe000_0021;
const SHADER: u32 = 0xe000_0022;
const SCOREBOARD: u32 = 0xe000_0023;
const BOMB: u32 = 0xe000_0024;

fn widget(
    kind: WidgetKind,
    name: &str,
    word: u16,
    input: u8,
    offset: (i16, i16),
    bitmap: u32,
) -> RawWidget {
    let mut b = vec![0u8; kind.size()];
    b[bf::INPUTS] = input;
    b[bf::STATE..bf::STATE + 2].copy_from_slice(&word.to_le_bytes());
    b[bf::ANCHOR..bf::ANCHOR + 2].copy_from_slice(&1u16.to_le_bytes());
    let offsets = match kind {
        WidgetKind::Bitmap => bf::OFFSETS,
        WidgetKind::Text => tf::OFFSETS,
    };
    for o in offsets {
        b[o..o + 2].copy_from_slice(&(offset.0 as u16).to_le_bytes());
        b[o + 2..o + 4].copy_from_slice(&(offset.1 as u16).to_le_bytes());
    }
    match kind {
        WidgetKind::Bitmap => {
            b[bf::BITMAP + 4..bf::BITMAP + 8].copy_from_slice(&bitmap.to_le_bytes());
            b[bf::SHADER + 4..bf::SHADER + 8].copy_from_slice(&SHADER.to_le_bytes());
        }
        WidgetKind::Text => {
            b[tf::SHADER + 4..tf::SHADER + 8].copy_from_slice(&SHADER.to_le_bytes());
            b[tf::STRING..tf::STRING + 4].copy_from_slice(&7u32.to_le_bytes());
        }
    }
    RawWidget {
        kind,
        name: name.into(),
        bytes: b,
    }
}

impl MapView for Invented {
    fn nhdt(&mut self, id: TagId) -> Result<RawHud, String> {
        if id.0 == SCOREBOARD {
            let mut bar = widget(
                WidgetKind::Bitmap,
                superbounce::ARM_WIDGET,
                0,
                0x43,
                superbounce::RETAIL_ARM_FULL,
                BOMB,
            );
            // Shown on word D 0x400, not word A.
            bar.bytes[bf::STATE..bf::STATE + 2].copy_from_slice(&[0, 0]);
            bar.bytes[bf::STATE + 6..bf::STATE + 8].copy_from_slice(&0x400u16.to_le_bytes());
            return Ok(RawHud {
                name: superbounce::ARM_HUD.into(),
                widgets: vec![bar],
            });
        }
        Ok(RawHud {
            name: "invented".into(),
            widgets: vec![
                widget(WidgetKind::Bitmap, "frag", FRAG_SELECTED, 0, (0, 0), PANEL),
                widget(
                    WidgetKind::Bitmap,
                    "plasma",
                    PLASMA_SELECTED,
                    0,
                    (0, 0),
                    PANEL,
                ),
                widget(
                    WidgetKind::Text,
                    "frag_count",
                    FRAG_SELECTED,
                    20,
                    (65, -1),
                    0,
                ),
                widget(
                    WidgetKind::Text,
                    "plasma_count",
                    PLASMA_SELECTED,
                    21,
                    (12, -1),
                    0,
                ),
            ],
        })
    }
    fn sprite_size(&mut self, id: TagId, _: i8) -> Result<[f32; 2], String> {
        match id.0 {
            PANEL => Ok([103.0, 19.0]),
            STRIP => Ok([92.0, 8.0]),
            BOMB => Ok([140.0, 13.0]),
            _ => Err(format!("{id} is not a bitmap")),
        }
    }
    fn sprite_blues(&mut self, id: TagId, _: i8) -> Result<Vec<f32>, String> {
        if id.0 != STRIP {
            return Err("not the strip".into());
        }
        Ok((0..92u32)
            .map(|x| x.saturating_sub(13) as f32 / 78.0)
            .collect())
    }
    fn named(&mut self, name: &str, _: Option<FourCC>) -> Result<TagId, String> {
        match name {
            templates::STRIP_BITMAP => Ok(TagId(STRIP)),
            templates::STRIP_SHADER => Ok(TagId(SHADER)),
            superbounce::ARM_HUD => Ok(TagId(SCOREBOARD)),
            _ => Err(format!("no tag {name} in the map")),
        }
    }
    fn group_of(&mut self, _: TagId) -> Result<FourCC, String> {
        Ok(SHAD)
    }
    fn arrows(&mut self) -> [Result<ArrowRef, String>; 2] {
        let a = Ok(ArrowRef {
            image: TagId(PANEL),
            material: TagId(SHADER),
            sequence: 0,
        });
        [a.clone(), a]
    }
}

/// The pill block's elements with the superbounce HUD's: the pill, its
/// end, the strip, the arming bar, the icon, then the channels.
fn block_and_bar() -> Hud {
    let mut block = pill_block::hud().elements;
    let mut sb = superbounce::hud().elements;
    let icon = block.pop().expect("the icon");
    block.push(sb.remove(0));
    block.push(icon);
    block.extend(sb);
    Hud {
        name: "block-and-bar",
        elements: block,
    }
}

/// The pill block and the superbounce HUD resolved and compiled: the
/// pill one count step past the frag pill, its end 20 on, the meter 21
/// under it, the strip's last 27 columns with their ramp; bitmaps first,
/// then the icon; the five channels.
#[test]
fn the_pill_block_and_the_bar_compile_against_an_invented_row() {
    let hud = block_and_bar();
    assert_eq!(hud.elements[3].id, superbounce::ARM);
    let t = templates::resolve(&mut Invented, TagId(0xe000_0001), &hud);
    assert_eq!(t.missing(&hud), Vec::<String>::new());
    let row = t.row.as_ref().unwrap();
    assert_eq!((row.panel, row.frag), (103.0, [53.0, 50.0]));
    assert_eq!((row.pill[0], row.count[0]), ((106, 0), (118, -1)));
    let c = compile(&hud, &t).unwrap();
    let ids: Vec<_> = c.widgets.iter().map(|w| (w.id, w.kind)).collect();
    assert_eq!(
        ids,
        [
            (pill_block::PILL, WidgetKind::Bitmap),
            (pill_block::PILL_END, WidgetKind::Bitmap),
            (CHARGE, WidgetKind::Bitmap),
            (superbounce::ARM, WidgetKind::Bitmap),
            (pill_block::PILL_ICON, WidgetKind::Text),
        ]
    );
    assert_eq!(
        c.channels,
        [
            superbounce::PROMPT,
            superbounce::MESSAGE,
            superbounce::MEDAL,
            superbounce::NAVPOINT,
            superbounce::TUTORIAL
        ]
    );
    let off = |id: &str| {
        let b = &c.widget(id).unwrap().bytes;
        (
            u16_at(b, bf::OFFSETS[0]) as i16,
            u16_at(b, bf::OFFSETS[0] + 2) as i16,
        )
    };
    // Fullscreen, one block from the top-left anchor (the grenade row's,
    // which split screen places them from): the pill, its end against
    // it, the strip under it, the arming bar over it.
    let (px, py) = pill_block::PILL_FULL;
    assert_eq!(off(pill_block::PILL), (px, py));
    assert_eq!(off(pill_block::PILL_END), (px + PILL_BODY, py));
    assert_eq!(off(CHARGE), (px, py + CHARGE_DOWN));
    for id in [pill_block::PILL, pill_block::PILL_END, CHARGE, superbounce::ARM] {
        let b = &c.widget(id).unwrap().bytes;
        assert_eq!(u16_at(b, bf::ANCHOR), Anchor::TopLeft as u16, "{id}");
    }
    // In half and quarter screen the pill is the grenade row's next.
    let pill = &c.widget(pill_block::PILL).unwrap().bytes;
    for m in 1..3 {
        assert_eq!(
            (
                u16_at(pill, bf::OFFSETS[m]) as i16,
                u16_at(pill, bf::OFFSETS[m] + 2) as i16
            ),
            row.pill[m]
        );
    }
    // The arming bar: the scoreboard's bomb_defusal, uncropped.
    assert_eq!(off(superbounce::ARM), superbounce::ARM_FULL);
    let arm = c.widget(superbounce::ARM).unwrap();
    assert_eq!(u32_at(&arm.bytes, bf::BITMAP + 4), BOMB);
    assert_eq!((arm.crop, arm.ramp, arm.input(0.5)), (None, None, 0.5));
    let charge = c.widget(CHARGE).unwrap();
    assert_eq!(u32_at(&charge.bytes, bf::BITMAP + 4), STRIP);
    assert_eq!(charge.crop, Some([65.0 / 92.0, 1.0, 0.0, 1.0]));
    assert_eq!(usize::from(CHARGE_COLUMNS), 27);
    // Columns 65 to 91: blue (65 - 13) / 78 to (91 - 13) / 78, and one
    // column past the last, so a full fill lights the last column too.
    let [b0, b1] = charge.ramp.unwrap();
    assert!((b0 - 52.0 / 78.0).abs() < 1e-6, "{b0}");
    assert!((b1 - 79.0 / 78.0).abs() < 1e-5, "{b1}");
    assert_eq!(charge.input(0.0), b0);
    assert!((charge.input(1.0) - b1).abs() < 1e-6);
    assert!((charge.input(0.5) - (b0 + b1) / 2.0).abs() < 1e-6);
    assert_eq!(charge.input(f32::NAN), b0);
    // The pill and its icon take no ramp: their input is the fill.
    let pill = c.widget(pill_block::PILL).unwrap();
    assert_eq!((pill.ramp, pill.input(0.4)), (None, 0.4));
    assert_eq!(pill.crop, Some(row.crop(PillPart::Body)));
    let icon = &c.widget(pill_block::PILL_ICON).unwrap().bytes;
    assert_eq!(
        (
            u16_at(icon, tf::OFFSETS[0]) as i16,
            u16_at(icon, tf::OFFSETS[0] + 2) as i16
        ),
        // Fullscreen, the icon is beside the tracker with the pill.
        (
            pill_block::PILL_FULL.0 + pill_block::ICON_AT.0,
            pill_block::PILL_FULL.1 + pill_block::ICON_AT.1
        )
    );
    assert_eq!(u32_at(icon, tf::STRING), 7, "the frag count's string");
    // The blocks the runtime appends: the records end to end.
    assert_eq!(c.block(WidgetKind::Bitmap).len(), 4 * BITMAP_SIZE);
    assert_eq!(c.block(WidgetKind::Text), icon.clone());
    // The widgets alone need no arrow; the preview's navpoint does.
    let mut no_arrows = t.clone();
    no_arrows.art.retain(|(a, _)| *a != Art::NavpointArrow);
    assert!(no_arrows.missing_widgets(&hud).is_empty());
    assert_eq!(no_arrows.missing(&hud).len(), 1);
}

/// What is missing is named, and the compiler refuses it.
#[test]
fn a_missing_template_is_named_and_refused() {
    let hud = block_and_bar();
    let mut t = templates::resolve(&mut Invented, TagId(0xe000_0001), &hud);
    t.row = Err("no row".into());
    let m = t.missing_widgets(&hud);
    assert!(m.iter().any(|l| l == "the grenade row: no row"), "{m:?}");
    let e = compile(&hud, &t).unwrap_err().to_string();
    assert!(e.contains("the grenade row: no row"), "{e}");
}

#[test]
fn the_ramp_and_the_end_crop_refuse_what_cannot_fill() {
    assert_eq!(templates::ramp(&[0.5, 0.75, 1.0]).unwrap(), [0.5, 1.25]);
    assert!(templates::ramp(&[0.5]).is_err());
    assert!(templates::ramp(&[0.5, 0.5]).is_err());
    assert!(templates::ramp(&[]).is_err());
    assert_eq!(
        templates::end_crop(92.0, 27).unwrap(),
        [65.0 / 92.0, 1.0, 0.0, 1.0]
    );
    assert!(templates::end_crop(20.0, 27).is_err());
    assert!(templates::end_crop(92.0, 0).is_err());
}

/// A gauge is a text widget in the HUD font, its point its top-left
/// corner, hidden in the modes it has no place in; the string hook's text
/// for it is the one character of its picture for the fill.
#[test]
fn a_gauge_is_a_text_widget_whose_character_is_its_fill() {
    let glyphs = Glyphs {
        first: 0xe2c0,
        steps: 6,
        size: (6, 16),
    };
    let hud = Hud {
        name: "t",
        elements: vec![
            Element::gauge("g", glyphs, Place::new(Anchor::BottomLeft, (-8, -60))).outside(8),
            Element::text("x", Font::Lcd, Justify::End, "88", Place::new(Anchor::TopLeft, (1, 2))),
        ],
    };
    assert!(hud.has_text() && hud.arts().is_empty() && !hud.uses_grenade_row());
    assert_eq!(hud.elements[0].kind.name(), "gauge");
    assert!(!hud.elements[0].kind.is_channel());
    assert_eq!(hud.elements[0].kind.longest(), None);
    let t = templates(Vec::new());
    assert_eq!(t.missing(&hud), Vec::<String>::new());
    let c = compile(&hud, &t).unwrap();
    let kinds: Vec<_> = c.widgets.iter().map(|w| (w.id, w.kind)).collect();
    assert_eq!(kinds, [("g", WidgetKind::Text), ("x", WidgetKind::Text)]);
    let g = c.widget("g").unwrap();
    assert_eq!(g.bytes.len(), TEXT_SIZE);
    assert_eq!((g.glyphs, g.crop, g.ramp), (Some(glyphs), None, None));
    assert_eq!(u16_at(&g.bytes, tf::ANCHOR), Anchor::BottomLeft as u16);
    assert_eq!(u16_at(&g.bytes, tf::JUSTIFICATION), Justify::Start as u16);
    assert_eq!(
        (
            u16_at(&g.bytes, tf::OFFSETS[0]) as i16,
            u16_at(&g.bytes, tf::OFFSETS[0] + 2) as i16
        ),
        (-8, -60)
    );
    assert_eq!(
        g.bytes[tf::FONTS..tf::FONTS + 3],
        [GAUGE_FONT.byte() as u8, NO_FONT as u8, NO_FONT as u8]
    );
    assert_eq!(u32_at(&g.bytes, tf::SHADER + 4), 0xe000_0003);
    assert_eq!(u32_at(&g.bytes, tf::STRING), 0x0400_0123);
    // The text: the picture for the fill, whatever the value's text.
    let at = |fill: f32| g.text(&Value { text: "no".into(), ..Value::fill(fill) });
    assert_eq!(at(0.0), "\u{e2c0}");
    assert_eq!(at(0.5), "\u{e2c3}");
    assert_eq!(at(1.0), "\u{e2c6}");
    assert_eq!(at(0.08), "\u{e2c0}");
    assert_eq!(at(0.09), "\u{e2c1}");
    assert_eq!(at(7.0), "\u{e2c6}");
    assert_eq!(at(f32::NAN), "\u{e2c0}");
    assert_eq!(glyphs.units().collect::<Vec<_>>(), (0xe2c0..=0xe2c6).collect::<Vec<u16>>());
    // A text's is its value's.
    let x = c.widget("x").unwrap();
    assert_eq!((x.glyphs, x.text(&Value::text("17"))), (None, "17".to_owned()));
    // A text template missing is the gauge's to name too.
    let mut none = t.clone();
    none.text = Err("no count".into());
    let only = Hud {
        name: "t",
        elements: vec![hud.elements[0].clone()],
    };
    assert_eq!(none.missing_widgets(&only), ["the text template: no count"]);
    assert!(compile(&only, &none).unwrap_err().to_string().contains("g: no count"));
}

#[test]
fn the_modes_are_the_widgets_three() {
    assert_eq!(Mode::ALL.map(Mode::index), [0, 1, 2]);
    assert_eq!(Font::Hud.engine(), 6);
    assert_eq!(Font::Lcd.engine(), 8);
    assert_eq!(records::ROOT_BITMAPS, 0x08);
    assert_eq!(records::ROOT_TEXTS, 0x10);
}

/// A notification is one line of at most 62 UTF-16 units: the game's
/// insert hands its copy 63, which keeps 62 and a zero.
#[test]
fn a_notification_is_one_line_of_at_most_62_units() {
    assert_eq!(MESSAGE_UNITS, 62);
    assert_eq!(MESSAGE_UNITS, BANNER_UNITS);
    assert!(message_fault(&"a".repeat(63)).is_some());
    assert_eq!(message_fault(&"a".repeat(MESSAGE_UNITS)), None);
    assert!(message_fault(&"a".repeat(MESSAGE_UNITS + 1)).is_some());
    // A private-use glyph is one unit.
    let medal = format!("\u{e2a0}{}", "a".repeat(MESSAGE_UNITS - 1));
    assert_eq!(message_fault(&medal), None);
    assert!(message_fault("two\nlines").is_some());
}

/// A banner is a text channel of one line of at most 62 UTF-16 units: the
/// game's copy keeps one fewer than the 63 it is given, and a zero.
#[test]
fn a_banner_is_one_line_of_at_most_62_units() {
    assert_eq!(banner_fault(&"A".repeat(BANNER_UNITS)), None);
    assert!(banner_fault(&"A".repeat(BANNER_UNITS + 1)).is_some());
    assert!(banner_fault("WAVE 3\nBOSS").is_some());
    assert!(banner_fault("WAVE 3\r").is_some());
    let k = Kind::Banner {
        longest: "ROUND COMPLETE",
    };
    assert_eq!(k.name(), "banner");
    assert!(k.is_channel() && k.is_lines());
    assert_eq!(k.longest(), Some("ROUND COMPLETE"));
    assert_eq!(k.art(), None);
    let e = Element::channel("b", k);
    assert_eq!(e.at, Place::GAME);
    assert_eq!(BANNER_TICKS, 3 * 30);
}

// ---- K5: team styles and the line shape ----

/// A team style names itself; the line is a shape; the game has eight
/// teams.
#[test]
fn a_team_style_names_its_team() {
    assert_eq!(Style::Team(0).name(), "team 0");
    assert_eq!(Style::Team(7).name(), "team 7");
    assert_ne!(Style::Team(0), Style::Team(1));
    assert!(crate::world::Shape::ALL.contains(&crate::world::Shape::Line));
    assert_eq!(crate::world::TEAMS, 8);
}

/// The Forge readout (`views::forge_readout`, slice H1): its states, the
/// texts a view gives, the widest each row may be and what an overrun
/// drops.
mod forge_readout_tests {
    use crate::views::forge_readout::{self as fr, Readout, View};
    use crate::views::superbounce_readout as sb;
    use crate::model::*;
    use crate::Mode;

    const NAMES: [&str; 14] = [
        "idle",
        "aim-object",
        "aim-spawner",
        "aim-starting-point-red",
        "aim-goal-hill",
        "aim-teleporter",
        "aim-other-game-type",
        "hold-allowed",
        "hold-refused",
        "hold-snapped",
        "refused",
        "budget-low",
        "near-full",
        "zone",
    ];

    fn shown(v: &Values, id: &str) -> Option<String> {
        let x = v.get(id).unwrap();
        x.shown.then(|| x.text.clone())
    }

    fn view(name: &str) -> View {
        fr::views().into_iter().find(|(n, _)| *n == name).unwrap().1
    }

    fn chars(s: &str) -> usize {
        s.chars().count()
    }

    /// The checks the core can make without a disc: the model is sound,
    /// every state values every element once, the states are the
    /// document's, and the world check's rules hold (a pose names a
    /// world mark and has no fault; a world mark a state shows has a
    /// pose and one it hides has none). The rest (fit, glyphs, frame,
    /// own, modes, drawn) are `h2 hud check`'s, on the disc.
    #[test]
    fn every_state_passes_every_check() {
        let h = fr::hud();
        assert!(h.problems().is_empty(), "{:?}", h.problems());
        let states = Readout.states();
        let names: Vec<_> = states.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, NAMES);
        assert!(Readout.multiplayer_only().is_some());
        for (name, v) in &states {
            assert!(v.layout_problems(&h).is_empty(), "{name}: {:?}", v.layout_problems(&h));
            let poses = Readout.world(name);
            for (id, pose) in &poses {
                let e = h.element(id).unwrap_or_else(|| panic!("{name}: a pose for {id}"));
                assert!(e.kind.world().is_some(), "{name}: {id} is not a world mark");
                assert_eq!(pose.fault(), None, "{name} {id}");
            }
            for e in h.elements.iter().filter(|e| e.kind.world().is_some()) {
                let has_pose = poses.iter().any(|(id, _)| *id == e.id);
                assert_eq!(v.get(e.id).unwrap().shown, has_pose, "{name} {}", e.id);
            }
            for e in h.elements.iter().filter(|e| e.kind.world().is_none()) {
                assert_ne!(v.get(e.id).unwrap().style, Style::Go, "{name} {}", e.id);
            }
        }
    }

    /// The rows sit at the superbounce readout's three places, bottom
    /// left, fullscreen only.
    #[test]
    fn the_rows_are_the_superbounce_readouts() {
        let h = fr::hud();
        for (id, y) in [
            (fr::NAME_ROW, sb::SPEED_UP),
            (fr::FACTS, sb::FALL_UP),
            (fr::GUIDE, sb::INFO_UP),
        ] {
            let at = h.element(id).unwrap().at;
            assert_eq!(at.anchor, Anchor::BottomLeft, "{id}");
            assert_eq!((at.half, at.quarter), (None, None), "{id}");
            assert_eq!(at.offset(Mode::Full), Some((sb::LEFT, y)), "{id}");
        }
    }

    /// A table of states and the three texts their views give (`None`
    /// is hidden).
    #[test]
    fn a_view_gives_its_three_texts() {
        let up = '\u{e2b0}';
        let spawner = "Item spawner  Respawn 30 s  Slayer, CTF";
        let held = "X 12.34  Y -5.60  Z 1.20  Yaw 90";
        let goal = "Goal  Hill  Neutral  Order 2";
        let some = |s: &str| Some(s.to_owned());
        type Row = (&'static str, Option<&'static str>, Option<&'static str>, Option<String>);
        let table: [Row; 14] = [
            ("idle", None, None, None),
            ("aim-object", Some("Crate, Metal, Small"), Some("Object  Scenery"), some("From the map, moved")),
            ("aim-spawner", Some("Rocket Launcher"), Some(spawner), some("From the map")),
            (
                "aim-starting-point-red",
                Some("Starting point"),
                Some("Starting point  Red team  Slayer, CTF"),
                some("From the map"),
            ),
            ("aim-goal-hill", Some("Hill 2 corner 3"), Some(goal), some("From the map, changed")),
            ("aim-teleporter", Some("Teleporter"), Some("Teleporter  Sender  Channel 3"), some("Added")),
            ("aim-other-game-type", Some("Rocket Launcher"), Some(spawner), some("Not in Slayer")),
            (
                "hold-allowed",
                Some("Rocket Launcher"),
                Some(held),
                Some(format!("{up} 0.85 m  Snap off  World axes")),
            ),
            ("hold-refused", Some("Rocket Launcher"), Some(held), some("Too close to another spawner")),
            (
                "hold-snapped",
                Some("Rocket Launcher"),
                Some(held),
                Some(format!("{up} 0.50 m  Snap 0.25  World axes")),
            ),
            ("refused", Some("Rocket Launcher"), Some(held), some("Item spawners are full (100).")),
            ("budget-low", Some("Rocket Launcher"), Some(held), some("3 item spawners left")),
            ("near-full", None, None, None),
            ("zone", Some("Hill 2 corner 3"), Some(goal), some("From the map, changed")),
        ];
        assert_eq!(table.iter().map(|r| r.0).collect::<Vec<_>>(), NAMES);
        for (name, n, f, g) in &table {
            let v = fr::values(&view(name));
            assert_eq!(shown(&v, fr::NAME_ROW).as_deref(), *n, "{name} name");
            assert_eq!(shown(&v, fr::FACTS).as_deref(), *f, "{name} facts");
            assert_eq!(shown(&v, fr::GUIDE), *g, "{name} guide");
        }
    }

    /// A height under the ground has the down arrow; the local axes are
    /// named; a budget of one is singular and a budget with nothing held
    /// says nothing.
    #[test]
    fn the_hold_line_and_the_budget_line_have_their_words() {
        let mut v = view("hold-snapped");
        let h = v.hold.as_mut().unwrap();
        h.height = -0.4;
        h.axes = fr::Axes::Local;
        assert_eq!(fr::guide_text(&v).unwrap(), "\u{e2b4} 0.40 m  Snap 0.25  Local axes");
        let mut v = view("budget-low");
        v.budget.as_mut().unwrap().left = 1;
        assert_eq!(fr::guide_text(&v).unwrap(), "1 item spawner left");
        v.hold = None;
        assert_eq!(fr::guide_text(&v).unwrap(), "From the map");
    }

    /// The marks: what each state shows of them.
    #[test]
    fn the_marks_follow_the_thing_and_the_hold() {
        let marks = |name: &str| {
            let v = fr::values(&view(name));
            let on: Vec<&str> = fr::MARKS.iter().copied().filter(|id| v.get(id).unwrap().shown).collect();
            (on, v)
        };
        assert_eq!(marks("idle").0, Vec::<&str>::new());
        assert_eq!(marks("aim-spawner").0, [fr::SEL]);
        assert_eq!(marks("aim-starting-point-red").0, [fr::SEL, fr::RING, fr::FACE]);
        let (on, v) = marks("hold-allowed");
        assert_eq!(on, [fr::SEL, fr::UNDER, fr::FROM]);
        assert_eq!(v.get(fr::SEL).unwrap().style, Style::Go);
        assert_eq!(v.get(fr::FROM).unwrap().style, Style::Dim);
        let (_, v) = marks("hold-refused");
        assert_eq!(v.get(fr::SEL).unwrap().style, Style::Inactive);
        // On the ground there is nothing under it.
        assert!(!marks("refused").0.contains(&fr::UNDER));
        let (_, v) = marks("near-full");
        assert!(fr::NEARS.iter().all(|id| v.get(id).unwrap().shown));
        let (_, v) = marks("idle");
        let near: Vec<_> = fr::NEARS.iter().take(3).map(|id| shown(&v, id).unwrap()).collect();
        assert_eq!(near, ["Spawn", "Flag", "Hill 2"]);
        assert_eq!(v.get(fr::NEARS[2]).unwrap().style, Style::Inactive);
        assert!(!v.get(fr::NEARS[3]).unwrap().shown);
    }

    /// A near mark in the session's game type draws in its team's colour
    /// (`Style::Team`), or the user's HUD colour with no team, and its
    /// label stays short; out of the game type it is `Inactive`,
    /// whatever its team.
    #[test]
    fn a_near_mark_is_in_its_teams_colour() {
        let team = |team: Option<u8>, in_game_type: bool| {
            fr::near_style(&fr::Near {
                label: "Spawn".into(),
                team,
                in_game_type,
            })
        };
        assert_eq!(team(Some(0), true), Style::Team(0));
        assert_eq!(team(Some(7), true), Style::Team(7));
        assert_eq!(team(None, true), Style::Normal);
        assert_eq!(team(Some(0), false), Style::Inactive);
        assert_eq!(team(None, false), Style::Inactive);
        // The label is the label: the team is the colour, not the text.
        let n = fr::Near {
            label: "Flag".into(),
            team: Some(1),
            in_game_type: true,
        };
        assert_eq!(fr::near_text(&n), "Flag");
        let v = fr::values(&view("idle"));
        assert_eq!(v.get(fr::NEARS[0]).unwrap().style, Style::Team(0));
        assert_eq!(v.get(fr::NEARS[1]).unwrap().style, Style::Team(1));
    }

    /// The zone marks are `Shape::Line` elements: an edge between each
    /// pair of corners in order, at most eight, hidden with fewer than
    /// two corners; the `zone` state's closed outline shows all eight.
    #[test]
    fn the_zone_marks_follow_the_corners() {
        let h = fr::hud();
        for id in fr::ZONES {
            assert_eq!(h.element(id).unwrap().kind.world(), Some(crate::world::Shape::Line), "{id}");
        }
        let edges = |corners: &[[f32; 3]]| {
            let mut v = view("zone");
            v.zone = corners.to_vec();
            let values = fr::values(&v);
            let on: Vec<&str> = fr::ZONES.iter().copied().filter(|id| values.get(id).unwrap().shown).collect();
            (fr::zone_edges(&v), on, values)
        };
        let (n, on, _) = edges(&view("zone").zone);
        assert_eq!((n, on.len()), (fr::ZONE, fr::ZONE), "the zone state shows every line");
        let (n, on, _) = edges(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]);
        assert_eq!((n, on), (2, vec![fr::ZONES[0], fr::ZONES[1]]));
        for corners in [&[][..], &[[0.0, 0.0, 0.0]][..]] {
            let (n, on, _) = edges(corners);
            assert_eq!((n, on.len()), (0, 0));
        }
        let ten: Vec<[f32; 3]> = (0..10).map(|i| [i as f32, 0.0, 0.0]).collect();
        let (n, on, _) = edges(&ten);
        assert_eq!((n, on.len()), (fr::ZONE, fr::ZONE), "the pool caps the edges");
    }

    /// A shown zone line has a sample pose: the edge's midpoint, its
    /// direction and half its length, which `world::edges` draws as the
    /// segment between the two corners.
    #[test]
    fn a_zone_lines_pose_is_the_edge() {
        let v = view("zone");
        let poses = fr::sample_poses(&v);
        let z = v.zone;
        for (i, id) in fr::ZONES.iter().enumerate() {
            let (_, pose) = poses.iter().find(|(p, _)| p == id).unwrap_or_else(|| panic!("{id}"));
            let [a, b] = [z[i], z[i + 1]];
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0];
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            assert_eq!(pose.at, mid, "{id}");
            assert_eq!(pose.half, [len / 2.0, 0.0], "{id}");
            let drawn = crate::world::edges(crate::world::Shape::Line, pose);
            let close = |p: [f32; 3], q: [f32; 3]| {
                p.iter().zip(q).all(|(x, y)| (x - y).abs() < 1e-4)
            };
            assert_eq!(drawn.len(), 1, "{id}");
            assert!(close(drawn[0][0], [a[0], a[1], a[2] + crate::world::LIFT]), "{id} from");
            assert!(close(drawn[0][1], [b[0], b[1], b[2] + crate::world::LIFT]), "{id} to");
        }
    }

    /// The rows are at the superbounce readout's places, so the readout
    /// declares it: the two never show together, and the `own` check
    /// does not compare them.
    #[test]
    fn the_readout_declares_the_superbounce_readout() {
        assert!(Readout.never_with().contains(&sb::NAME));
    }

    /// No text is wider than its `longest`, by its characters (the
    /// `fit` check measures the pixels on the disc's font): in every
    /// state, in the widest numbers and in free text over a row.
    #[test]
    fn no_text_is_wider_than_its_longest() {
        let h = fr::hud();
        let check = |what: &str, v: &Values| {
            for e in &h.elements {
                let x = v.get(e.id).unwrap();
                if let (true, Some(l)) = (x.shown, e.kind.longest()) {
                    assert!(chars(&x.text) <= chars(l), "{what} {}: {:?}", e.id, x.text);
                }
            }
        };
        for (name, v) in Readout.states() {
            check(name, &v);
        }
        // The widest numbers, and numbers that are not.
        let mut v = view("hold-snapped");
        let hold = v.hold.as_mut().unwrap();
        hold.position = [-1e9, f32::NAN, 1e9];
        hold.height = f32::INFINITY;
        v.target.as_mut().unwrap().yaw = 1e9;
        let widest = fr::values(&v);
        check("widest numbers", &widest);
        assert_eq!(shown(&widest, fr::FACTS).unwrap(), "X -999.99  Y 0.00  Z 999.99  Yaw 280");
        // Free text over a row's width: words dropped whole.
        let mut v = view("refused");
        v.refusal = Some("This map has no room for another item spawner of any kind at all".into());
        v.target.as_mut().unwrap().name = "A very long name from a tag that goes on and on and on".into();
        v.near = vec![fr::Near {
            label: "Assault Rifle Spawner".into(),
            team: Some(7),
            in_game_type: true,
        }];
        let long = fr::values(&v);
        check("long text", &long);
        assert_eq!(shown(&long, fr::GUIDE).unwrap(), "This map has no room for another item");
        assert_eq!(shown(&long, fr::NAME_ROW).unwrap(), "A very long name from a tag");
        assert_eq!(shown(&long, fr::NEARS[0]).unwrap(), "Assault Rifle");
    }

    /// An overrun drops the last fact whole, last first, and never
    /// cuts one; the kind stays.
    #[test]
    fn an_overrun_drops_the_last_fact_whole() {
        let start = |team: &str, types: &str| {
            let mut v = view("aim-starting-point-red");
            v.target.as_mut().unwrap().thing = fr::Thing::StartingPoint {
                team: team.into(),
                types: types.into(),
            };
            fr::facts_text(&v).unwrap()
        };
        // Fits: all three.
        assert_eq!(start("Red team", "Slayer, CTF"), "Starting point  Red team  Slayer, CTF");
        // The game types overrun: they go, the team stays.
        let types = "Slayer, CTF, Oddball, KotH, Assault, Juggernaut";
        assert_eq!(start("Neutral team", types), "Starting point  Neutral team");
        assert_eq!(start("Red team", types), "Starting point  Red team");
        // The team alone overruns too: the kind stays, nothing is cut.
        let team = "A team whose name is far too long for the row";
        assert_eq!(start(team, types), "Starting point");
        for t in [start("Red team", types), start(team, types)] {
            assert!(!t.contains("..."), "{t}");
            assert!(chars(&t) <= chars(fr::FACTS_LONGEST), "{t}");
        }
        // Held: the widest position and the heading still fit; a fact
        // that did not would go heading first (`fit_facts`).
        let mut v = view("hold-allowed");
        v.hold.as_mut().unwrap().position = [-999.0, -999.0, -999.0];
        assert_eq!(fr::facts_text(&v).unwrap(), "X -999.00  Y -999.00  Z -999.00  Yaw 90");
        assert_eq!(fr::fit_facts(&["X 1".into(), "Y".repeat(50)], fr::FACTS_LONGEST), "X 1");
        // Empty facts are skipped, so a spawner with no game types
        // leaves no gap.
        assert_eq!(start("Red team", ""), "Starting point  Red team");
    }
}

/// A monitor's HUD (`Describe::monitor`) sees the grenade row with the
/// retail pills gated off: its next pill and count are the row's first
/// place, one step back from the frag's (the plasma pill's and count's),
/// in every mode; the pills' crop is the frag pill's still.
#[test]
fn a_monitors_grenade_row_is_its_first_place() {
    let hud = block_and_bar();
    let t = templates::resolve(&mut Invented, TagId(0xe000_0001), &hud);
    let row = *t.row.as_ref().unwrap();
    assert_eq!(row.step[0], (53, 0));
    let first = row.first();
    assert_eq!((first.pill[0], first.count[0]), ((0, 0), (12, -1)));
    for i in 0..3 {
        let (s, p, c) = (row.step[i], row.pill[i], row.count[i]);
        assert_eq!(first.pill[i], (p.0 - 2 * s.0, p.1 - 2 * s.1));
        assert_eq!(first.count[i], (c.0 - 2 * s.0, c.1 - 2 * s.1));
    }
    assert_eq!((first.panel, first.frag), (row.panel, row.frag));
    let m = t.for_monitor();
    assert_eq!(*m.row.as_ref().unwrap(), first);
    let at = Place::in_grenade_row(Origin::NextGrenadePill);
    assert_eq!(records::placed(&at, &m).unwrap().1[0], (0, 0));
}

/// The retail bitmap widgets a monitor keeps are named by
/// `MONITOR_KEEPS`, counted among the bitmap widgets alone: a text widget
/// of the same name, or a bitmap of another, is not kept.
#[test]
fn a_monitor_keeps_the_motion_tracker_alone() {
    let hud = RawHud {
        name: "invented".into(),
        widgets: vec![
            widget(WidgetKind::Bitmap, "shield", 0, 0, (0, 0), 1),
            widget(WidgetKind::Text, "motion_sensor", 0, 0, (0, 0), 0),
            widget(WidgetKind::Bitmap, "motion_sensor", 0, 0, (0, 0), 2),
            widget(WidgetKind::Bitmap, "grenade", 0, 0, (0, 0), 3),
            widget(WidgetKind::Bitmap, "motion_sensor", 0, 0, (0, 0), 4),
        ],
    };
    assert_eq!(templates::monitor_keeps(&hud), [1, 3]);
    assert_eq!(MONITOR_KEEPS, ["motion_sensor"]);
}
