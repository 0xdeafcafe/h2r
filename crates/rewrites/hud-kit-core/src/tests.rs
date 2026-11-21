//! The core without a map: the record bytes at the draw's offsets, the
//! superbounce HUD compiled against templates resolved from an invented
//! `nhdt` through a [`MapView`], and the cropped meter's ramp.

use map::{FourCC, TagId};

use crate::huds::superbounce::{self, CHARGE, CHARGE_COLUMNS, CHARGE_DOWN};
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
        }),
    }
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
                superbounce::ARM_FULL,
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

/// The superbounce HUD resolved and compiled: the pill one count step past
/// the frag pill, its end 20 on, the meter 21 under it, the strip's last
/// 27 columns with their ramp; bitmaps first, then the icon; the four
/// channels.
#[test]
fn the_superbounce_hud_compiles_against_an_invented_row() {
    let hud = superbounce::hud();
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
            (superbounce::PILL, WidgetKind::Bitmap),
            (superbounce::PILL_END, WidgetKind::Bitmap),
            (CHARGE, WidgetKind::Bitmap),
            (superbounce::ARM, WidgetKind::Bitmap),
            (superbounce::PILL_ICON, WidgetKind::Text),
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
    assert_eq!(off(superbounce::PILL), (106, 0));
    assert_eq!(off(superbounce::PILL_END), (106 + PILL_BODY, 0));
    assert_eq!(off(CHARGE), (106, CHARGE_DOWN));
    // The arming bar: the scoreboard's bomb_defusal, uncropped, centred
    // where retail draws it fullscreen.
    assert_eq!(off(superbounce::ARM), superbounce::ARM_FULL);
    let arm = c.widget(superbounce::ARM).unwrap();
    assert_eq!(u32_at(&arm.bytes, bf::BITMAP + 4), BOMB);
    assert_eq!(u16_at(&arm.bytes, bf::ANCHOR), Anchor::Centre as u16);
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
    let pill = c.widget(superbounce::PILL).unwrap();
    assert_eq!((pill.ramp, pill.input(0.4)), (None, 0.4));
    assert_eq!(pill.crop, Some(row.crop(PillPart::Body)));
    let icon = &c.widget(superbounce::PILL_ICON).unwrap().bytes;
    assert_eq!(
        (
            u16_at(icon, tf::OFFSETS[0]) as i16,
            u16_at(icon, tf::OFFSETS[0] + 2) as i16
        ),
        (118, -1)
    );
    assert_eq!(u32_at(icon, tf::STRING), 7, "the frag count's string");
    // The blocks the runtime appends: the records end to end.
    assert_eq!(c.block(WidgetKind::Bitmap).len(), 3 * BITMAP_SIZE);
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
    let hud = superbounce::hud();
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

#[test]
fn the_modes_are_the_widgets_three() {
    assert_eq!(Mode::ALL.map(Mode::index), [0, 1, 2]);
    assert_eq!(Font::Hud.engine(), 6);
    assert_eq!(Font::Lcd.engine(), 8);
    assert_eq!(records::ROOT_BITMAPS, 0x08);
    assert_eq!(records::ROOT_TEXTS, 0x10);
}

/// A notification is one line of at most 63 UTF-16 units: the game's
/// insert copies 63.
#[test]
fn a_notification_is_one_line_of_at_most_63_units() {
    assert_eq!(message_fault(&"a".repeat(MESSAGE_UNITS)), None);
    assert!(message_fault(&"a".repeat(MESSAGE_UNITS + 1)).is_some());
    // A private-use glyph is one unit.
    let medal = format!("\u{e2a0}{}", "a".repeat(MESSAGE_UNITS - 1));
    assert_eq!(message_fault(&medal), None);
    assert!(message_fault("two\nlines").is_some());
}
