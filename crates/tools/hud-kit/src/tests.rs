//! The kit without a disc: compiled widgets read back through the HUD
//! editor's model, one layout per HUD, the template lookup on the HUD
//! editor's invented map, the synthetic sheet, each check on a HUD built
//! to break it, and the JSON's rectangles against the compiled bytes.

use hud_edit::Mode;
use hud_edit::model::{Value as Field, WidgetKind};
use hud_edit::place::{self, Frame};
use hud_edit::render;
use scenario_edit::MapSource;

use crate::check::{self, Check};
use crate::compile::AsHudEdit;
use crate::views::pill_block::{self, PillBlock};
use crate::views::superbounce::{self, Superbounce};
use crate::views::superbounce_segments as segs;
use crate::model::*;
use crate::preview;
use crate::templates;
use crate::{Source, compile, hudg, synthetic};

fn synthetic() -> Source {
    Source::synthetic().expect("the synthetic map")
}

fn bl(x: i16, y: i16) -> Place {
    Place::new(Anchor::BottomLeft, (x, y))
}

/// A compiled widget reads back through `hud_edit::model::Widget` with
/// the anchor, offsets, registration, font and inputs asked for, its
/// art and text template, and zero show words (only the runtime's gate
/// draws it).
#[test]
fn a_compiled_widget_reads_back_as_asked() {
    let mut src = synthetic();
    let hud = Hud {
        name: "t",
        elements: vec![
            Element::meter(
                "m",
                Art::StripMeter,
                bl(-1, -124)
                    .registered([0.25, 0.75])
                    .split((3, -60), (2, -30)),
            ),
            Element::text(
                "t",
                Font::Lcd,
                Justify::End,
                "99.9",
                Place::new(Anchor::TopRight, (-10, 20)),
            ),
            Element::channel("c", Kind::Prompt { longest: "x" }),
        ],
    };
    let t = src.templates(&hud);
    assert_eq!(t.missing(&hud), Vec::<String>::new());
    let c = compile(&hud, &t).unwrap();
    assert_eq!(c.channels, ["c"]);
    let back = c.as_hud_edit(0x1000);
    assert_eq!(back.widgets.len(), 2);
    let m = &back.widgets[0];
    assert_eq!(
        (m.key.kind, m.name.as_str(), m.address),
        (WidgetKind::Bitmap, "m", 0x1000)
    );
    assert_eq!(m.bytes.len(), hud_edit::layout::BITMAP_SIZE);
    assert_eq!(m.anchor(), Anchor::BottomLeft as u16);
    assert_eq!(m.offset(Mode::Full), (-1, -124));
    assert_eq!(m.offset(Mode::Half), (3, -60));
    assert_eq!(m.offset(Mode::Quarter), (2, -30));
    for mode in Mode::ALL {
        assert_eq!(m.registration(mode), [0.25, 0.75]);
        assert_eq!(m.sequence(mode), templates::STRIP_SEQUENCE);
    }
    assert_eq!(m.inputs(), [compile::KIT_INPUT; 4]);
    assert_eq!(
        m.state(),
        &[0; 20],
        "no show word: the retail gate never draws it"
    );
    assert_eq!(m.special(), 0);
    let strip = t.art(&Art::StripMeter).unwrap();
    assert_eq!(m.bitmap(), Some(strip.bitmap));
    assert_eq!(m.shader(), Some(strip.shader.1));
    assert_eq!(m.get("in0"), Some(Field::Int(0)));
    let w = &back.widgets[1];
    assert_eq!(
        (w.key.kind, w.address),
        (
            WidgetKind::Text,
            0x1000 + hud_edit::layout::BITMAP_SIZE as u32
        )
    );
    assert_eq!(w.bytes.len(), hud_edit::layout::TEXT_SIZE);
    assert_eq!(w.anchor(), Anchor::TopRight as u16);
    assert_eq!(w.offset(Mode::Full), (-10, 20));
    assert_eq!(w.justification(), Justify::End as u16);
    assert_eq!(w.font(Mode::Full), Font::Lcd.byte());
    assert_eq!(
        w.font(Mode::Half),
        compile::NO_FONT,
        "hidden in split screen: no font"
    );
    assert_eq!(w.font(Mode::Quarter), compile::NO_FONT);
    assert_eq!(hud_edit::layout::text_font(w.font(Mode::Full)), Some(8));
    let text = t.text.as_ref().unwrap();
    assert_eq!(w.shader(), Some(text.shader.1));
    assert_eq!(w.string(), text.string);
    assert_eq!(w.inputs(), [0; 4]);
    assert_eq!(w.flags(), 0, "not a number, not the player's name");
}

/// One layout per HUD: a state names every element once and nothing
/// else, and a HUD names each element once.
#[test]
fn one_layout_per_hud() {
    let hud = superbounce::hud();
    let mut v = Superbounce.states().remove(1).1;
    assert!(v.layout_problems(&hud).is_empty());
    v.0.retain(|(id, _)| *id != superbounce::NAVPOINT);
    v.0.push(("sb.nope", Value::shown()));
    v.0.push((superbounce::ARM, Value::shown()));
    let p = v.layout_problems(&hud);
    assert!(p.contains(&"sb.navpoint has no value".to_owned()), "{p:?}");
    assert!(p.contains(&"sb.arm has 2 values".to_owned()), "{p:?}");
    assert!(
        p.contains(&"sb.nope is not an element of superbounce".to_owned()),
        "{p:?}"
    );
    let mut twice = hud.clone();
    twice.elements.push(twice.elements[0].clone());
    assert_eq!(twice.problems(), ["sb.arm is named twice"]);
    let mut src = synthetic();
    let t = src.templates(&twice);
    assert!(compile(&twice, &t).is_err());
}

/// The templates on the HUD editor's invented map: the player HUD its
/// biped names, a retail widget's art by name, the frag count's text
/// shader and string; and what is not there is named, not replaced.
#[test]
fn templates_resolve_on_the_hud_editors_fixture_map() {
    let (bytes, player, _, bitmap) = hud_edit::fixture::build();
    let source = MapSource {
        bytes: Some(bytes.into()),
        ..MapSource::file("hudtest.map")
    };
    let mut map = source.open().unwrap();
    assert_eq!(templates::player_huds(&mut map).unwrap(), [player]);
    let meter = Art::Widget {
        hud: HudRef::Player,
        name: "meter",
    };
    let hud = Hud {
        name: "t",
        elements: vec![
            Element::icon("i", meter.clone(), bl(0, -100)),
            Element::text("t", Font::Hud, Justify::Start, "x", bl(0, -120)),
            Element::meter("s", Art::StripMeter, bl(0, -140)),
            Element::pointer("p", Art::WaypointArrow, bl(0, -160)),
        ],
    };
    let hudg = hudg::read(&mut map).ok();
    assert!(hudg.is_none(), "the fixture has no hudg");
    let mut art = render::Art::new(assets::ResourceFiles::new());
    let t = templates::resolve(&mut map, &mut art, player, hudg.as_ref(), &hud);
    let a = t.art(&meter).unwrap();
    assert_eq!(a.bitmap, bitmap);
    assert_eq!(a.sequences, [1, 1, 1]);
    let name = |id| map.tag(id).unwrap().name.clone().unwrap();
    assert_eq!(name(a.shader.1), "invented\\hud\\shaders\\ammo_meter");
    let text = t.text.as_ref().unwrap();
    assert_eq!(name(text.shader.1), "invented\\hud\\shaders\\text_flash");
    assert_eq!(
        hud_edit::model::string(&map, text.string).as_deref(),
        Some("hud_input1")
    );
    let missing = t.missing(&hud);
    assert_eq!(missing.len(), 2, "{missing:?}");
    assert!(
        missing[0].starts_with("StripMeter") && missing[0].contains("disintegrator_meter"),
        "{missing:?}"
    );
    assert!(
        missing[1].starts_with("WaypointArrow") && missing[1].contains("no hudg"),
        "{missing:?}"
    );
    let e = compile(&hud, &t).unwrap_err().to_string();
    assert!(e.starts_with("s: "), "{e}");
    // A widget name the HUD does not have.
    let nope = Art::Widget {
        hud: HudRef::Player,
        name: "nope",
    };
    let h2 = Hud {
        name: "t",
        elements: vec![Element::icon("i", nope.clone(), bl(0, -100))],
    };
    let t = templates::resolve(&mut map, &mut art, player, None, &h2);
    assert!(t.art(&nope).unwrap_err().contains("no bitmap widget nope"));
}

/// The readout beside the superbounce HUD: every check passes for both
/// in every mode but the glyphs (no fonts), no element of one meets one
/// of the other, and the readout draws fullscreen only, bottom left.
#[test]
fn the_readout_passes_beside_the_superbounce_hud() {
    use crate::views::superbounce_readout::{self as readout, Readout};
    let mut src = synthetic();
    let r = check::all(&mut src, &[&Superbounce, &Readout], &Mode::ALL).unwrap();
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
    let shot = |state: &str, mode: Mode| {
        r.shots
            .iter()
            .find(|s| s.hud == readout::NAME && s.state == state && s.mode == mode)
            .unwrap()
    };
    let drawn = |state: &str, mode: Mode| -> Vec<&str> {
        shot(state, mode)
            .elements
            .iter()
            .filter(|d| d.drawn)
            .map(|d| d.id)
            .collect()
    };
    // The charge segments' ring is drawn in every state but `idle`,
    // after the bar.
    let with_ring = |before: &[&'static str], after: &[&'static str]| -> Vec<&'static str> {
        before
            .iter()
            .chain(&segs::IDS)
            .chain(after)
            .copied()
            .collect()
    };
    // The targets row too, in every state but `idle`.
    assert_eq!(
        drawn("falling-off-seam", Mode::Full),
        with_ring(
            &[readout::FALL],
            &[readout::SPEED, readout::STEER, readout::KINDS]
        )
    );
    assert_eq!(
        drawn("miss-too-slow", Mode::Full),
        with_ring(&[], &[readout::INFO, readout::KINDS])
    );
    assert!(drawn("idle", Mode::Full).is_empty());
    for mode in [Mode::Half, Mode::Quarter] {
        assert!(drawn("falling-off-seam", mode).is_empty(), "{mode:?}");
        assert!(drawn("tagged", mode).is_empty(), "{mode:?}");
    }
    // Right of the tracker and the charge segments, under its top, left of
    // the middle third's far edge.
    for id in [readout::FALL, readout::SPEED, readout::STEER] {
        let d = shot("falling-off-seam", Mode::Full).element(id).unwrap();
        let rect = d.rect.unwrap();
        assert!(
            rect.x0 >= 149.0 && rect.y0 >= 376.0 && rect.y1 <= 445.0,
            "{id}: {rect:?}"
        );
    }
    // The targets row over the arming bar (y 355 to 368), clear of it,
    // and right of the Halo 1 health strip and the shield band.
    let kinds = shot("falling-off-seam", Mode::Full)
        .element(readout::KINDS)
        .unwrap()
        .rect
        .unwrap();
    assert!(
        kinds.x0 >= 149.0 && kinds.y0 >= 326.0 && kinds.y1 <= 355.0,
        "{kinds:?}"
    );
}

/// The combined preview (`h2 hud preview superbounce-all`) in every screen
/// mode: no part of the block is drawn without the rest. The readout has
/// no half or quarter place, so the combined HUD has none either, and a
/// split frame draws none of its widgets.
#[test]
fn the_combined_preview_draws_no_part_of_the_block_alone() {
    use crate::views::superbounce_all::All;
    let mut src = synthetic();
    let r = check::all(&mut src, &[&All], &Mode::ALL).unwrap();
    let modes: Vec<_> = r.of(Check::Modes).collect();
    assert!(modes.is_empty(), "{modes:#?}");
    let hud = All.hud();
    for s in r.shots.iter().filter(|s| s.mode != Mode::Full) {
        for d in s.elements.iter().filter(|d| d.drawn) {
            let e = hud.element(d.id).unwrap();
            assert!(e.kind.is_channel(), "{} {:?}: {}", s.state, s.mode, d.id);
        }
    }
    let full = r
        .shots
        .iter()
        .find(|s| s.mode == Mode::Full && s.state == "3-primed-targets")
        .unwrap();
    for id in segs::IDS {
        let d = full.element(id).unwrap();
        assert!(d.drawn && d.fill == Some(1.0), "{id}");
    }
}

/// The combined Forge preview (`h2 hud preview forge-all`) in every screen
/// mode: no part of the block is drawn without the rest, as for
/// `superbounce-all`.
#[test]
fn the_forge_preview_draws_no_part_of_the_block_alone() {
    use crate::views::forge_all::All;
    let mut src = synthetic();
    let r = check::all(&mut src, &[&All], &Mode::ALL).unwrap();
    let modes: Vec<_> = r.of(Check::Modes).collect();
    assert!(modes.is_empty(), "{modes:#?}");
    let hud = All.hud();
    for s in r.shots.iter().filter(|s| s.mode != Mode::Full) {
        for d in s.elements.iter().filter(|d| d.drawn) {
            let e = hud.element(d.id).unwrap();
            assert!(e.kind.is_channel(), "{} {:?}: {}", s.state, s.mode, d.id);
        }
    }
}

/// Each navpoint of a HUD has a sample of its own, so two shown at once
/// (the readout's two targets, the ranked seams after a bounce) draw
/// apart: neither's arrow, marker and number is over the other's.
#[test]
fn a_huds_navpoints_draw_at_samples_of_their_own() {
    use crate::views::superbounce_readout::{self as readout, Readout};
    for (i, a) in preview::NAVPOINT_SAMPLES.iter().enumerate() {
        for b in &preview::NAVPOINT_SAMPLES[i + 1..] {
            assert!((a[0] - b[0]).abs() >= 0.1, "{a:?} {b:?}");
        }
    }
    assert_eq!(preview::navpoint_sample(0), preview::NAVPOINT_SAMPLE);
    assert_eq!(preview::navpoint_sample(9), preview::NAVPOINT_SAMPLES[3]);
    let mut src = synthetic();
    let r = check::all(&mut src, &[&Readout], &[Mode::Full]).unwrap();
    for state in ["targets", "second-bounce"] {
        let shot = r
            .shots
            .iter()
            .find(|s| s.hud == readout::NAME && s.state == state)
            .unwrap();
        let rect = |id: &str| shot.element(id).unwrap().rect.unwrap();
        let (a, b) = (rect(readout::TARGET_1), rect(readout::TARGET_2));
        assert!(
            a.x1 <= b.x0 || b.x1 <= a.x0 || a.y1 <= b.y0 || b.y1 <= a.y0,
            "{state}: {a:?} {b:?}"
        );
    }
}

/// The synthetic sheet draws: the superbounce HUD in every state and
/// mode, every check passing but the glyphs (no fonts), and a picture
/// and its JSON written to a folder.
#[test]
fn the_synthetic_sheet_draws_and_passes() {
    let mut src = synthetic();
    let r = check::all(&mut src, &[&Superbounce], &Mode::ALL).unwrap();
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
    assert_eq!(
        r.skipped.iter().map(|s| s.0).collect::<Vec<_>>(),
        [Check::Glyphs]
    );
    let states = Superbounce.states().len();
    assert_eq!(states, 26);
    assert_eq!(r.shots.len(), states * 3);
    let (sheet, tiles) = preview::sheet(&r.shots, 3);
    assert_eq!((sheet.width, sheet.height), (3 * 320, states as u32 * 240));
    assert_eq!(tiles["tiles"].as_array().unwrap().len(), states * 3);
    let s = r
        .shots
        .iter()
        .find(|s| s.state == "not-primed" && s.mode == Mode::Full)
        .unwrap();
    let drawn: Vec<_> = s
        .elements
        .iter()
        .filter(|d| d.drawn)
        .map(|d| d.id)
        .collect();
    // Not primed, nothing of this HUD but the navpoint: the charge is the
    // readout's segments'.
    assert_eq!(drawn, [superbounce::NAVPOINT]);
    assert_eq!(crate::rects::BOTTOM_LEFT[1], 36 + superbounce::FRAME_HEIGHT);
    // The navpoint at the sample point, right of and above the centre:
    // the arrow's tip on the point, everything else above it.
    let n = s.element(superbounce::NAVPOINT).unwrap();
    let nr = n.rect.unwrap();
    assert!(nr.x0 > 320.0 && nr.y1 < 240.0, "{nr:?}");
    let f = s.frame;
    let sample_y =
        f.anchor_point(4).unwrap()[1] + preview::NAVPOINT_SAMPLE[1] * f32::from(f.bottom - f.top);
    assert_eq!(nr.y1, sample_y.round(), "{nr:?}");
    assert_eq!(n.text.as_deref(), Some("12"));
    // The retail tracker and frag pill are in the picture.
    assert!(
        s.retail
            .iter()
            .any(|p| p.drawn && p.rect.unwrap().pixels() == [47, 360, 130, 443])
    );
    assert!(s.retail.iter().any(|p| p.drawn && p.name == "frag_pill"));
    // In split screen the tutorial is hidden and says why; the navpoint
    // shows.
    let half = |state: &str| {
        r.shots
            .iter()
            .find(|s| s.state == state && s.mode == Mode::Half)
            .unwrap()
    };
    let d = half("tutorial-1").element(superbounce::TUTORIAL).unwrap();
    assert!(!d.drawn && d.hidden.as_deref() == Some("no half screen offset"));
    let h = half("primed-full");
    assert!(h.element(superbounce::NAVPOINT).unwrap().drawn);
    // In every split frame the navpoint's stack stays inside the frame
    // (the sample is moved down, as the game keeps a navpoint in its
    // waypoint area).
    for s in r.shots.iter().filter(|s| s.mode != Mode::Full) {
        let Some(n) = s.element(superbounce::NAVPOINT).filter(|d| d.drawn) else {
            continue;
        };
        let nr = n.rect.unwrap();
        assert!(
            nr.y0 >= f32::from(s.frame.top),
            "{} {:?}: {nr:?}",
            s.state,
            s.mode
        );
    }
    // No strips: nothing but the retail HUD.
    let none = r
        .shots
        .iter()
        .find(|s| s.state == "no-strips" && s.mode == Mode::Full)
        .unwrap();
    assert!(none.elements.iter().all(|d| !d.drawn));
    // The tutorial's box is drawn in the middle.
    let t = r
        .shots
        .iter()
        .find(|s| s.state == "tutorial-3" && s.mode == Mode::Full)
        .unwrap()
        .element(superbounce::TUTORIAL)
        .unwrap();
    let b = t.rect.unwrap();
    assert!(
        t.drawn && (b.x0 + b.x1) / 2.0 == 320.0 && b.y0 < 240.0 && b.y1 > 240.0,
        "{b:?}"
    );
    // Written outside the checkout.
    let dir = std::env::temp_dir().join(format!("hud-kit-{}", std::process::id()));
    let json = preview::shot_json(s, &src, &r.findings);
    let [png, js] = preview::write(&dir, s, &json).unwrap();
    assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
    assert!(png.ends_with("superbounce-not-primed-full.png"));
    let back: serde_json::Value = serde_json::from_slice(&std::fs::read(&js).unwrap()).unwrap();
    assert_eq!(back["elements"][0]["id"], "sb.arm");
    assert_eq!(back["source"], "synthetic");
    let _ = std::fs::remove_dir_all(&dir);
    let inside = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out");
    assert!(
        preview::write(&inside, s, &json)
            .unwrap_err()
            .to_string()
            .contains("Git checkout")
    );
    assert!(!inside.exists());
}

/// The JSON's rectangles are where the compiled bytes put the widgets, by
/// the HUD editor's placement rules.
#[test]
fn the_json_rectangles_are_the_compiled_bytes() {
    let mut src = synthetic();
    let hud = superbounce::hud();
    let t = src.templates(&hud);
    let c = compile(&hud, &t).unwrap();
    let back = c.as_hud_edit(preview::KIT_BASE);
    let r = check::hud(&mut src, &Superbounce, &[Mode::Full]).unwrap();
    let frame = Frame::for_mode(Mode::Full);
    let mut compared = 0;
    for s in &r.shots {
        let json = preview::shot_json(s, &src, &r.findings);
        for (i, d) in s.elements.iter().enumerate() {
            let Some(rect) = d.rect.filter(|_| !d.channel) else {
                continue;
            };
            assert_eq!(
                json["elements"][i]["rect"],
                serde_json::json!(rect.pixels()),
                "{} {}",
                s.state,
                d.id
            );
            let w = back.widgets.iter().find(|w| w.name == d.id).unwrap();
            let point = place::widget_point(&frame, w.anchor(), w.offset(Mode::Full)).unwrap();
            match w.key.kind {
                WidgetKind::Text => {
                    let font = hud_edit::layout::text_font(w.font(Mode::Full)).unwrap();
                    let size = render::text_size(None, font, d.text.as_deref().unwrap_or(""));
                    assert_eq!(
                        rect,
                        place::text_rect(point, size, w.justification()),
                        "{}",
                        d.id
                    );
                }
                WidgetKind::Bitmap => {
                    let reg = w.registration(Mode::Full);
                    assert_eq!(rect.x0 + reg[0] * rect.width(), point[0], "{}", d.id);
                    assert_eq!(rect.y0 + reg[1] * rect.height(), point[1], "{}", d.id);
                }
            }
            compared += 1;
        }
    }
    let states = Superbounce.states().len();
    assert_eq!(compared, states, "the arming bar in every state");
}

/// The kit's pill block (`views::pill_block`) on the synthetic map:
/// fullscreen the pill is beside the tracker, cut from the frag pill
/// (the body its first 20 pixels, the end its last 7 against the body,
/// the panel's height) with its icon where a pill's count is; in split
/// screen it is the grenade row's next pill.
#[test]
fn the_pill_block_is_beside_the_tracker_and_in_the_grenade_row() {
    let mut src = synthetic();
    let r = check::hud(&mut src, &PillBlock, &Mode::ALL).unwrap();
    let shot = |state: &str, mode: Mode| {
        r.shots
            .iter()
            .find(|s| s.state == state && s.mode == mode)
            .unwrap()
    };
    let s = shot("not-primed", Mode::Full);
    let drawn: Vec<_> = s
        .elements
        .iter()
        .filter(|d| d.drawn)
        .map(|d| d.id)
        .collect();
    assert_eq!(
        drawn,
        [
            pill_block::PILL,
            pill_block::PILL_END,
            pill_block::PILL_ICON
        ]
    );
    let at = |id| s.element(id).unwrap().rect.unwrap().pixels();
    let left = 48 + i32::from(pill_block::PILL_FULL.0);
    let top = 36 + i32::from(pill_block::PILL_FULL.1);
    let h = i32::from(synthetic::PANEL_SIZE.1);
    let (body, end) = (i32::from(PILL_BODY), i32::from(PILL_END));
    assert_eq!((left, top), (138, 376));
    assert_eq!(at(pill_block::PILL), [left, top, left + body, top + h]);
    assert_eq!(
        at(pill_block::PILL_END),
        [left + body, top, left + body + end, top + h]
    );
    // The icon where a pill's count is in its pill, by the count's own
    // rule (centred: half its width, rounded down, left of the point).
    let icon = s.element(pill_block::PILL_ICON).unwrap().rect.unwrap();
    let point = [
        (left + i32::from(pill_block::ICON_AT.0)) as f32,
        (top + i32::from(pill_block::ICON_AT.1)) as f32,
    ];
    assert_eq!(
        pill_block::ICON_AT,
        (
            synthetic::COUNTS[1].0 - synthetic::PANEL_AT.0,
            synthetic::COUNTS[1].1 - synthetic::PANEL_AT.1
        )
    );
    let size = render::text_size(None, Font::Hud.engine(), pill_block::ICON_TEXT);
    assert_eq!(icon, place::text_rect(point, size, Justify::Centre as u16));
    // In split screen the pill is the grenade row's next: one count step
    // past the frag pill, under the frame's top-left corner.
    let h = shot("primed-full", Mode::Half);
    assert!(h.element(pill_block::PILL).unwrap().drawn);
    let (sx, sy) = (
        synthetic::COUNTS[0].0 - synthetic::COUNTS[1].0,
        synthetic::COUNTS[0].1 - synthetic::COUNTS[1].1,
    );
    let hf = Frame::for_mode(Mode::Half);
    let split = h.element(pill_block::PILL).unwrap().rect.unwrap().pixels();
    assert_eq!(
        [split[0], split[1]],
        [
            i32::from(hf.left + synthetic::PANEL_AT.0 + sx + sx),
            i32::from(hf.top + synthetic::PANEL_AT.1 + sy)
        ]
    );
    assert!(h.element(pill_block::CHARGE).unwrap().drawn);
}

/// The charge segments on the synthetic sheet: nine arcs round the
/// tracker's rim and off it, each the picture the fonts are served with,
/// outside the HUD frame by what they are let and no finding for it; lit
/// as the stage says.
#[test]
fn the_charge_segments_draw_round_the_tracker() {
    use crate::views::superbounce_readout::Readout;
    assert_eq!(
        segs::TRACKER.map(i32::from),
        crate::rects::TRACKER.pixels(),
        "the rim the segments are placed round"
    );
    assert_eq!([segs::CORNER.0, segs::CORNER.1], crate::rects::BOTTOM_LEFT);
    let mut src = synthetic();
    let r = check::hud(&mut src, &Readout, &[Mode::Full]).unwrap();
    let on_segments: Vec<_> = r
        .findings
        .iter()
        .filter(|f| f.elements.iter().any(|e| e.starts_with("sbr.seg.")))
        .collect();
    assert!(on_segments.is_empty(), "{on_segments:#?}");
    let shot = |state: &str| r.shots.iter().find(|s| s.state == state).unwrap();
    let frame = Frame::for_mode(Mode::Full);
    // The tracker's disc and the clear band round it: every pixel within
    // the rim's radius and the gap, less the arcs' soft edge.
    let rim = crate::rects::TRACKER;
    let centre = [(rim.x0 + rim.x1) / 2.0, (rim.y0 + rim.y1) / 2.0];
    let mut disc = render::Cover::new();
    for y in 340..470 {
        for x in 30..150 {
            let (dx, dy) = (x as f32 + 0.5 - centre[0], y as f32 + 0.5 - centre[1]);
            if dx.hypot(dy) <= rim.width() / 2.0 + f32::from(segs::GAP) - 1.0 {
                disc.set(x, y);
            }
        }
    }
    let mut ring = render::Cover::new();
    for (i, id) in segs::IDS.into_iter().enumerate() {
        let ((x, y), (w, h)) = (segs::AT[i], segs::SIZE[i]);
        let d = shot("segments-primed").element(id).unwrap();
        let rect = d.rect.unwrap();
        assert!(d.drawn, "{id}");
        assert_eq!(
            (d.kind, d.font, d.text.as_deref()),
            ("gauge", Some(6), None),
            "{id}"
        );
        assert_eq!(rect.pixels(), [x, y, x + w, y + h].map(i32::from), "{id}");
        assert_eq!(d.reserved.unwrap().pixels(), rect.pixels(), "{id}");
        // The full picture's opaque pixels, none on the tracker or in the
        // gap round it, none of another arc's.
        let cover = d.cover.as_ref().unwrap();
        let full = segs::glyphs(i).picture(1.0).unwrap();
        let inked = full.pixels.iter().filter(|p| **p >> 12 >= 4).count();
        assert!(
            inked > 30 && cover.count() as usize == inked,
            "{id}: {} of {inked}",
            cover.count()
        );
        assert_eq!(cover.overlap(&disc), 0, "{id} is over the tracker");
        assert_eq!(cover.overlap(&ring), 0, "{id} is over another arc");
        ring.add(cover);
        // Empty, the faint track: drawn, and no wider than the lit arc.
        let empty = shot("segments-idle").element(id).unwrap();
        let track = empty.cover.as_ref().unwrap();
        assert!(empty.drawn && empty.fill == Some(0.0), "{id}");
        assert!(track.count() > 15 && track.count() <= cover.count(), "{id}");
        assert_eq!(
            track.overlap(cover),
            track.count(),
            "{id}: the track is under the arc"
        );
    }
    // The left side and the bottom are outside the frame.
    let first = shot("segments-primed")
        .element(segs::IDS[0])
        .unwrap()
        .rect
        .unwrap();
    let foot = shot("segments-primed")
        .element(segs::IDS[4])
        .unwrap()
        .rect
        .unwrap();
    assert!(first.x0 < f32::from(frame.left) && foot.y1 > f32::from(frame.bottom));
    let fills = |state: &str| -> Vec<Option<f32>> {
        segs::IDS
            .iter()
            .map(|id| shot(state).element(id).unwrap().fill)
            .collect()
    };
    assert_eq!(
        fills("segments-under")[..4],
        [Some(1.0), Some(1.0), Some(1.0), Some(0.0)]
    );
    assert_eq!(fills("segments-held")[5..7], [Some(1.0), Some(0.0)]);
    assert_eq!(fills("segments-primed"), [Some(1.0); 9]);
    assert_eq!(fills("segments-lost"), [Some(0.0); 9]);
    assert!(
        segs::IDS
            .iter()
            .all(|id| !shot("idle").element(id).unwrap().drawn)
    );
    // The wipe's edge: three quarters of the charge is 6.75 arcs, the
    // seventh part lit, more than the empty track and less than the arc.
    let count = |state: &str, i: usize| {
        let d = shot(state).element(segs::IDS[i]).unwrap();
        let p = segs::glyphs(i).picture(d.fill.unwrap()).unwrap();
        p.pixels.iter().filter(|p| **p >> 12 > 8).count()
    };
    assert_eq!(fills("segments-draining")[6], Some(0.75));
    assert!(count("segments-draining", 6) > 0);
    assert!(count("segments-draining", 6) < count("segments-primed", 6));
    assert_eq!(count("segments-draining", 7), 0);
}

/// A gauge whose pictures no code of ours draws is a finding, and is not
/// drawn.
#[test]
fn a_gauge_with_no_picture_is_a_finding() {
    let none = Glyphs {
        first: u16::from(b'A'),
        steps: 2,
        size: (4, 4),
    };
    let r = findings(&case(
        vec![Element::gauge("g", none, bl(10, -150))],
        &[("g", Value::fill(1.0))],
    ));
    let drawn: Vec<_> = r.of(Check::Drawn).map(|f| f.text.clone()).collect();
    assert!(
        drawn.len() == 1 && drawn[0].contains("no code of ours draws U+0043"),
        "{drawn:?}"
    );
    // One of the ring's, at its own size, is drawn where it is placed.
    let r = findings(&case(
        vec![Element::gauge("g", segs::glyphs(0), bl(10, -150))],
        &[("g", Value::fill(0.5))],
    ));
    assert!(r.of(Check::Drawn).next().is_none(), "{:#?}", r.findings);
    let d = r.shots[0].element("g").unwrap();
    let (w, h) = segs::SIZE[0];
    assert!(d.drawn);
    assert_eq!(
        d.rect.unwrap().pixels(),
        [58, 294, 58 + i32::from(w), 294 + i32::from(h)]
    );
}

/// An element let outside the HUD frame may sit that far out and no
/// further; one that is not let out may not sit outside at all.
#[test]
fn an_element_is_outside_the_frame_only_by_what_it_is_let() {
    let strip =
        |id, x: i16, by: i16| Element::meter(id, Art::StripMeterEnd(9), bl(x, -100)).outside(by);
    let frame = |elements| {
        let r = findings(&case(elements, &[]));
        r.of(Check::Frame)
            .map(|f| f.text.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(frame(vec![strip("a", -14, 14)]), Vec::<String>::new());
    assert_eq!(frame(vec![strip("a", 0, 0)]), Vec::<String>::new());
    let out = frame(vec![strip("a", -15, 14)]);
    assert!(
        out.len() == 1 && out[0].contains("more than its 14 outside"),
        "{out:?}"
    );
    let out = frame(vec![strip("a", -14, 0)]);
    assert!(
        out.len() == 1 && out[0].contains("leaves the HUD frame"),
        "{out:?}"
    );
}

/// The pill block's pill is lit primed (its texels as they are, the icon
/// in the kit text colour), greyed not primed and faded while scanning.
#[test]
fn styles_change_the_pill() {
    let mut src = synthetic();
    let r = check::hud(&mut src, &PillBlock, &[Mode::Full]).unwrap();
    let px = |state: &str, id: &str, dx: f32, dy: f32| {
        let s = r.shots.iter().find(|s| s.state == state).unwrap();
        let rect = s.element(id).unwrap().rect.unwrap();
        let (x, y) = ((rect.x0.round() + dx) as u32, (rect.y0.round() + dy) as u32);
        let i = ((y * s.canvas.width + x) * 4) as usize;
        let p = &s.canvas.rgba[i..i + 3];
        [p[0], p[1], p[2]]
    };
    let sat = |p: [u8; 3]| p.iter().max().unwrap() - p.iter().min().unwrap();
    // The pill's right end, clear of the icon.
    let pill = |state| px(state, pill_block::PILL_END, 2.0, 8.0);
    let (lit, off, dim) = (pill("primed-full"), pill("not-primed"), pill("scanning"));
    assert!(sat(off) * 2 < sat(lit), "greyed: {off:?} against {lit:?}");
    assert_ne!(dim, lit, "faded");
    let icon = |state| px(state, pill_block::PILL_ICON, 1.0, 4.0);
    assert_eq!(icon("primed-full"), render::TEXT_COLOUR[..3]);
    assert_ne!(icon("not-primed"), icon("primed-full"));
    assert_ne!(icon("scanning"), icon("primed-full"));
}

/// A text in a transparent colour (a flash record with no alpha) puts no
/// pixels down, and the drawn check says so; in an opaque one it passes.
#[test]
fn a_text_that_draws_no_pixels_is_a_finding() {
    let c = case(
        vec![Element::text(
            "a",
            Font::Hud,
            Justify::Start,
            "PRIMED -3",
            bl(10, -150),
        )],
        &[(
            "a",
            Value::text("PRIMED -3").styled(Style::Flash(Pulse::Objective)),
        )],
    );
    let mut src = synthetic();
    let at = Pulse::ALL
        .iter()
        .position(|p| *p == Pulse::Objective)
        .unwrap();
    src.colours.flash[at] = [255, 255, 255, 0];
    let r = check::hud(&mut src, &c, &[Mode::Full]).unwrap();
    let drawn: Vec<_> = r.of(Check::Drawn).collect();
    assert_eq!(drawn.len(), 1, "{:#?}", r.findings);
    assert_eq!(drawn[0].elements, ["a"]);
    assert_eq!(drawn[0].state.as_deref(), Some("s"));
    let r = check::hud(&mut synthetic(), &c, &[Mode::Full]).unwrap();
    assert!(r.of(Check::Drawn).next().is_none(), "{:#?}", r.findings);
}

/// A test HUD and its states.
struct Case {
    hud: Hud,
    states: Vec<(&'static str, Values)>,
}

impl Describe for Case {
    fn hud(&self) -> Hud {
        self.hud.clone()
    }
    fn states(&self) -> Vec<(&'static str, Values)> {
        self.states.clone()
    }
}

/// Every element shown, a text with its longest, but for `with`.
fn shown(hud: &Hud, with: &[(&'static str, Value)]) -> Values {
    let mut v = Values::default();
    for e in &hud.elements {
        v.set(e.id, Value::text(e.kind.longest().unwrap_or("")));
    }
    for (id, x) in with {
        v.set(id, x.clone());
    }
    v
}

fn case(elements: Vec<Element>, with: &[(&'static str, Value)]) -> Case {
    let hud = Hud {
        name: "broken",
        elements,
    };
    let states = vec![("s", shown(&hud, with))];
    Case { hud, states }
}

fn findings(c: &Case) -> check::Report {
    check::hud(&mut synthetic(), c, &[Mode::Full]).unwrap()
}

fn fires(c: Case, want: Check) {
    let r = findings(&c);
    assert!(
        r.of(want).next().is_some(),
        "{want:?} did not fire: {:#?}",
        r.findings
    );
}

/// Each check fires on a HUD built to break it.
#[test]
fn each_check_fires_on_a_hud_built_to_break_it() {
    let text = |id, longest, at| Element::text(id, Font::Hud, Justify::Start, longest, at);
    // fit: a state's text wider than its longest.
    fires(
        case(
            vec![text("a", "AB", bl(10, -150))],
            &[("a", Value::text("ABCDEFGH"))],
        ),
        Check::Fit,
    );
    // frame: past the frame's right edge (still on the screen).
    fires(
        case(
            vec![text("a", "AB", Place::new(Anchor::TopRight, (20, 0)))],
            &[],
        ),
        Check::Frame,
    );
    // retail: a strip over the motion tracker.
    fires(
        case(
            vec![Element::meter("m", Art::StripMeter, bl(-1, -60))],
            &[("m", Value::fill(1.0))],
        ),
        Check::Retail,
    );
    // retail: a text on the reticle.
    fires(
        case(
            vec![text("a", "ABCDEF", Place::new(Anchor::Centre, (-10, 0)))],
            &[],
        ),
        Check::Retail,
    );
    // own: two texts in one place.
    fires(
        case(
            vec![text("a", "AB", bl(10, -150)), text("b", "CD", bl(12, -150))],
            &[],
        ),
        Check::Own,
    );
    // one layout: a state without a value for an element.
    let mut c = case(
        vec![text("a", "AB", bl(10, -150)), text("b", "CD", bl(10, -130))],
        &[],
    );
    c.states[0].1.0.retain(|(id, _)| *id != "b");
    fires(c, Check::OneLayout);
    // templates: a retail widget the player's HUD does not have.
    let nope = Art::Widget {
        hud: HudRef::Player,
        name: "nope",
    };
    fires(
        case(vec![Element::icon("i", nope, bl(10, -150))], &[]),
        Check::Templates,
    );
    // prompt: a notification over the game's 62 characters.
    fires(
        case(
            vec![Element::channel("m", Kind::Message { longest: "a" })],
            &[("m", Value::text("a".repeat(64)))],
        ),
        Check::Prompt,
    );
    // prompt: three lines in the training box.
    fires(
        case(
            vec![Element::channel("t", Kind::Training { longest: "one" })],
            &[("t", Value::text("one\ntwo\nthree"))],
        ),
        Check::Prompt,
    );
    // prompt: a banner over the game's 62 characters, and one of two
    // lines.
    fires(
        case(
            vec![Element::channel("b", Kind::Banner { longest: "A" })],
            &[("b", Value::text("A".repeat(63)))],
        ),
        Check::Prompt,
    );
    fires(
        case(
            vec![Element::channel("b", Kind::Banner { longest: "A" })],
            &[("b", Value::text("WAVE 3\nBOSS WAVE"))],
        ),
        Check::Prompt,
    );
    // modes: one element with split-screen offsets, one without.
    fires(
        case(
            vec![
                text("a", "AB", bl(10, -150).split((5, -60), (5, -60))),
                text("b", "CD", bl(10, -130)),
            ],
            &[],
        ),
        Check::Modes,
    );
    // glyphs: a font without the crouch button's glyph, which the
    // prompt line's verbs use.
    let mut r = check::Report::default();
    let no_crouch = |_: i16, u: u16| Some(u != 0xe110);
    check::glyphs(
        &mut r,
        &superbounce::hud(),
        &Superbounce.states(),
        &no_crouch,
    );
    assert!(!r.findings.is_empty());
    assert!(r.findings.iter().all(|f| f.check == Check::Glyphs));
    let mut r = check::Report::default();
    check::glyphs(
        &mut r,
        &superbounce::hud(),
        &Superbounce.states(),
        &|_, _| Some(true),
    );
    assert!(r.findings.is_empty());
    // And each case's HUD, mended, passes (the cases fire for the reason
    // given, not another).
    let r = findings(&case(
        vec![
            text("a", "ABCDEFGH", bl(10, -150)),
            text("b", "CD", bl(10, -130)),
        ],
        &[],
    ));
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
}

/// The grenade row on the synthetic map, a panel of two pills as
/// retail's: the next pill one count step past the frag pill, and its
/// body and end cut from the frag pill's part of the panel; the preview
/// draws those texels.
#[test]
fn the_next_pill_is_cut_from_the_frag_pill() {
    let mut src = synthetic();
    let hud = pill_block::hud();
    let t = src.templates(&hud);
    let row = t.row.as_ref().unwrap();
    let (w, sw) = (
        f32::from(synthetic::PANEL_SIZE.0),
        f32::from(synthetic::PILL_WIDTH),
    );
    assert_eq!(row.panel, w);
    assert_eq!(row.frag, [w - sw, sw]);
    assert_eq!(row.pill, [(106, 0); 3]);
    assert_eq!(row.count, [(118, -1); 3]);
    let body = t.art(&Art::GrenadePill(PillPart::Body)).unwrap();
    let end = t.art(&Art::GrenadePill(PillPart::End)).unwrap();
    assert_eq!(body.crop, Some([53.0 / w, 73.0 / w, 0.0, 1.0]));
    assert_eq!(end.crop, Some([96.0 / w, 1.0, 0.0, 1.0]));
    let c = compile(&hud, &t).unwrap();
    assert_eq!(c.widget(pill_block::PILL_END).unwrap().crop, end.crop);
    assert_eq!(c.widget(pill_block::PILL_ICON).unwrap().crop, None);
    // The frag pill's red rises with its x: the body starts at its x 0,
    // the end at its x 43 and rises to 49.
    let r = check::hud(&mut src, &PillBlock, &[Mode::Full]).unwrap();
    let s = r.shots.iter().find(|s| s.state == "primed-full").unwrap();
    let red = |id, dx: f32| {
        let e = s.element(id).unwrap().rect.unwrap();
        let (x, y) = ((e.x0 + dx) as u32, (e.y0 + 8.0) as u32);
        s.canvas.rgba[((y * s.canvas.width + x) * 4) as usize]
    };
    let (b0, e0, e6) = (
        red(pill_block::PILL, 0.0),
        red(pill_block::PILL_END, 0.0),
        red(pill_block::PILL_END, 6.0),
    );
    assert!(b0 < e0 && e0 < e6, "{b0} {e0} {e6}");
}

/// A crop is the part's span of the frag pill, wherever the frag pill is
/// in the panel.
#[test]
fn a_crop_is_its_part_of_the_frag_pill() {
    let mut row = templates::GrenadeRow {
        pill_anchor: 1,
        pill: [(0, 0); 3],
        count_anchor: 1,
        count: [(0, 0); 3],
        panel: 103.0,
        frag: [53.0, 50.0],
        step: [(53, 0); 3],
    };
    assert_eq!(row.crop(PillPart::End), [96.0 / 103.0, 1.0, 0.0, 1.0]);
    row.frag = [0.0, 50.0];
    assert_eq!(row.crop(PillPart::Body), [0.0, 20.0 / 103.0, 0.0, 1.0]);
    assert_eq!(
        row.crop(PillPart::End),
        [43.0 / 103.0, 50.0 / 103.0, 0.0, 1.0]
    );
}

/// The pill block's strip: the strip's last 27 columns, 2 pixels under the
/// pill's bottom (21 under its top) and in line with its left edge, in
/// every mode; the fill goes through the strip's blue ramp (the runtime's
/// `hud_input1`), so a full meter shows every column and an empty one
/// none; hidden when no charge is building.
#[test]
fn the_charge_meter_is_the_strips_end_under_the_pill() {
    let mut src = synthetic();
    let hud = pill_block::hud();
    let t = src.templates(&hud);
    let c = compile(&hud, &t).unwrap();
    let w = c.widget(pill_block::CHARGE).unwrap();
    let strip = 92.0;
    assert_eq!(w.crop, Some([65.0 / strip, 1.0, 0.0, 1.0]));
    // The synthetic strip's blue is 0 to column 13, then rises 1/78 a
    // column: column 65's is 52/78 (rounded to a byte).
    let [b0, b1] = w.ramp.unwrap();
    assert!((b0 - 52.0 / 78.0).abs() < 0.01, "{b0}");
    assert!(b1 > 1.0, "the last column's blue plus a step: {b1}");
    assert_eq!(w.input(0.0), b0);
    assert_eq!(w.input(1.0), b1);
    let r = check::hud(&mut src, &PillBlock, &Mode::ALL).unwrap();
    for s in &r.shots {
        let pill = s.element(pill_block::PILL).unwrap().rect;
        let charge = s.element(pill_block::CHARGE).unwrap();
        let (Some(p), Some(m)) = (pill, charge.rect) else {
            continue;
        };
        assert_eq!(
            [m.x0, m.y0, m.width(), m.height()],
            [p.x0, p.y0 + f32::from(pill_block::CHARGE_DOWN), 27.0, 8.0],
            "{} {:?}",
            s.state,
            s.mode
        );
    }
    let full = |state: &str| {
        r.shots
            .iter()
            .find(|s| s.state == state && s.mode == Mode::Full)
            .unwrap()
            .element(pill_block::CHARGE)
            .unwrap()
            .clone()
    };
    assert!(!full("not-primed").drawn, "no charge building");
    assert!(full("primed-full").drawn);
    assert_eq!(full("primed-draining").fill, Some(0.25));
    assert_eq!(
        full("primed-still-warning").fill,
        Some(superbounce::LOW_CHARGE)
    );
    // The recipe is the arming bar's: the strip shows primed and lost.
    assert!(!full("near").drawn && !full("held").drawn);
    assert!(full("lost").drawn);
}

/// The arming bar is the scoreboard's `bomb_defusal`, uncropped (its
/// `hud_input1` the fill), at the bottom left over the readout's rows;
/// drawn in the recipe's states at their fills.
#[test]
fn the_arming_bar_is_assaults_bottom_left_with_the_readout() {
    let mut src = synthetic();
    let hud = superbounce::hud();
    let t = src.templates(&hud);
    let c = compile(&hud, &t).unwrap();
    let w = c.widget(superbounce::ARM).unwrap();
    assert_eq!((w.crop, w.ramp), (None, None));
    assert_eq!(w.input(0.5), 0.5);
    let r = check::hud(&mut src, &Superbounce, &[Mode::Full]).unwrap();
    let arm = |state: &str| {
        r.shots
            .iter()
            .find(|s| s.state == state)
            .unwrap()
            .element(superbounce::ARM)
            .unwrap()
            .clone()
    };
    let frame = Frame::for_mode(Mode::Full);
    let corner = frame.anchor_point(1).unwrap();
    let rect = arm("held").rect.unwrap();
    assert_eq!([rect.width(), rect.height()], [140.0, 13.0]);
    // Bottom left with the readout, above the speed row (y 376) and the
    // charge segments' right side.
    let (x, y) = superbounce::ARM_FULL;
    assert_eq!(rect.x0 + rect.width() / 2.0, corner[0] + f32::from(x));
    assert_eq!(rect.y0 + rect.height() / 2.0, corner[1] + f32::from(y));
    assert_eq!(rect.x0, 138.0);
    assert!(
        rect.y1 <= 376.0 && rect.y1 + 16.0 <= f32::from(segs::AT[8].1),
        "{rect:?}"
    );
    assert!(arm("held").drawn && arm("near").drawn && arm("primed-ending").drawn);
    assert_eq!(arm("under").fill, Some(superbounce::FILL_UNDER));
    assert_eq!(arm("held").fill, Some(superbounce::FILL_HELD));
    assert!(!arm("not-primed").drawn && !arm("primed-full").drawn);
}

/// The step verbs, the warning and the new-charge hint go on the prompt
/// line, and the landings and the losses in the message area, in the
/// states that have them.
#[test]
fn the_prompt_and_message_carry_the_steps_and_landings() {
    let states = Superbounce.states();
    let get = |state: &str, id: &str| {
        states
            .iter()
            .find(|(n, _)| *n == state)
            .unwrap()
            .1
            .get(id)
            .cloned()
            .unwrap()
    };
    assert_eq!(
        get("near", superbounce::PROMPT).text,
        superbounce::PROMPT_NEAR
    );
    assert_eq!(
        get("under", superbounce::PROMPT).text,
        superbounce::PROMPT_UNDER
    );
    assert_eq!(
        get("held", superbounce::PROMPT).text,
        superbounce::PROMPT_HELD
    );
    assert!(!get("primed-full", superbounce::PROMPT).shown);
    assert_eq!(
        get("primed-full", superbounce::MESSAGE).text,
        superbounce::MESSAGE_PRIMED
    );
    assert_eq!(
        get("lost", superbounce::MESSAGE).text,
        superbounce::MESSAGE_LOST
    );
    assert_eq!(
        get("primed-still-warning", superbounce::PROMPT).text,
        superbounce::PROMPT_KEEP_MOVING
    );
    for (state, message) in [
        ("lost", superbounce::MESSAGE_LOST),
        ("lost-stood-still", superbounce::MESSAGE_LOST_STOOD_STILL),
        ("lost-crouched", superbounce::MESSAGE_LOST_CROUCHED),
    ] {
        assert_eq!(get(state, superbounce::MESSAGE).text, message);
        assert_eq!(
            get(state, superbounce::PROMPT).text,
            superbounce::PROMPT_CHARGE_AGAIN
        );
    }
    // A bounce's line is its medal, on the medal channel.
    assert!(!get("bounced", superbounce::MESSAGE).shown);
    assert_eq!(
        get("bounced", superbounce::MEDAL).text,
        superbounce::Medal::Superbounce.line()
    );
}

/// A test HUD with world marks: its states and their sample poses.
struct WorldCase {
    hud: Hud,
    states: Vec<(&'static str, Values)>,
    poses: Vec<(&'static str, hud_kit_core::world::Pose)>,
}

impl Describe for WorldCase {
    fn hud(&self) -> Hud {
        self.hud.clone()
    }
    fn states(&self) -> Vec<(&'static str, Values)> {
        self.states.clone()
    }
    fn world(&self, _: &str) -> Vec<(&'static str, hud_kit_core::world::Pose)> {
        self.poses.clone()
    }
}

/// The world check: a mark a state shows needs a sample pose without a
/// fault that draws lines inside the frame; a pose names a world mark;
/// the go green is a world mark's alone. A sound case passes, and its
/// mark claims no rectangle, so nothing overlaps it.
#[test]
fn the_world_check_fires_on_marks_built_to_break_it() {
    use hud_kit_core::world::{Pose, Shape};
    let mark = |id, shape| Element::channel(id, Kind::WorldMark { shape, longest: "" });
    let text = |id| Element::text(id, Font::Hud, Justify::Start, "AB", bl(10, -150));
    let case = |elements: Vec<Element>, values: Vec<(&'static str, Value)>, poses| {
        let hud = Hud {
            name: "world",
            elements,
        };
        WorldCase {
            hud,
            states: vec![("s", Values(values))],
            poses,
        }
    };
    let world = |c: &WorldCase| -> Vec<String> {
        check::hud(&mut synthetic(), c, &[Mode::Full])
            .unwrap()
            .of(Check::World)
            .map(|f| f.text.clone())
            .collect()
    };
    let pose = Pose::at([0.0; 3], [0.3, 0.1]);
    // Sound: a bracket at the scene's origin.
    let sound = case(
        vec![mark("b", Shape::Bracket), text("t")],
        vec![("b", Value::shown()), ("t", Value::text("AB"))],
        vec![("b", pose)],
    );
    let r = check::hud(&mut synthetic(), &sound, &[Mode::Full]).unwrap();
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
    let b = r.shots[0].element("b").unwrap();
    assert!(b.drawn && b.rect.is_none() && b.reserved.is_none());
    let w = b.world.as_ref().unwrap();
    assert_eq!((w.shape, w.edges), (Shape::Bracket, 8));
    // No pose.
    let c = case(
        vec![mark("b", Shape::Bracket)],
        vec![("b", Value::shown())],
        vec![],
    );
    assert_eq!(world(&c), ["b is shown with no sample pose"]);
    // A pose with a fault.
    let c = case(
        vec![mark("b", Shape::Bracket)],
        vec![("b", Value::hidden())],
        vec![(
            "b",
            Pose {
                strength: 2.0,
                ..pose
            },
        )],
    );
    assert_eq!(world(&c), ["b's pose has strength 2 outside 0 to 1"]);
    // A pose with no size draws nothing.
    let c = case(
        vec![mark("b", Shape::Bracket)],
        vec![("b", Value::shown())],
        vec![("b", Pose::at([0.0; 3], [0.0, 0.0]))],
    );
    assert_eq!(world(&c), ["b is shown and its bracket draws no lines"]);
    // A sample far outside the frame.
    let c = case(
        vec![mark("b", Shape::Bracket)],
        vec![("b", Value::shown())],
        vec![("b", Pose::at([9.0, 0.0, 0.0], [0.3, 0.1]))],
    );
    assert!(
        world(&c)[0].starts_with("b's sample leaves the HUD frame"),
        "{:?}",
        world(&c)
    );
    // A pose for a text, and for nothing.
    let c = case(
        vec![text("t")],
        vec![("t", Value::text("AB"))],
        vec![("t", pose), ("nope", pose)],
    );
    assert_eq!(
        world(&c),
        [
            "a pose for t, which is a text",
            "a pose for nope, which is not an element"
        ]
    );
    // The go green on a text.
    let c = case(
        vec![text("t")],
        vec![("t", Value::text("AB").styled(Style::Go))],
        vec![],
    );
    assert_eq!(
        world(&c),
        ["t is a text in the go green, a world mark's alone"]
    );
}

/// The approach guide in the readout's preview: each moment draws the
/// marks it shows where the sample scene puts them, and the checks pass.
/// On the approach the pipper is outside the box with ticks between; in
/// a caught fall it is inside, the box is in the go green and says so.
#[test]
fn the_approach_guide_draws_in_its_moments() {
    use crate::views::superbounce_guide as guide;
    use crate::views::superbounce_readout::{self as readout, Readout};
    let mut src = synthetic();
    let r = check::hud(&mut src, &Readout, &Mode::ALL).unwrap();
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
    let shot = |state: &str, mode: Mode| {
        r.shots
            .iter()
            .find(|s| s.hud == readout::NAME && s.state == state && s.mode == mode)
            .unwrap()
    };
    let bounds = |state: &str, id: &str| {
        let d = shot(state, Mode::Full).element(id).unwrap();
        assert!(d.drawn, "{state} {id}");
        d.world.as_ref().unwrap().bounds.unwrap()
    };
    let drawn = |state: &str, id: &str| shot(state, Mode::Full).element(id).unwrap().drawn;
    // The box is round the scene's origin in every moment.
    let frame = shot("guide-approach", Mode::Full).frame;
    let origin = preview::project(&frame, [0.0; 3]);
    for (state, _, _) in guide::samples() {
        let b = bounds(state, guide::BOX);
        assert!(b.contains_point(origin), "{state}: {b:?} {origin:?}");
        assert!(
            drawn(state, guide::PIPPER) && drawn(state, guide::DRIFT),
            "{state}"
        );
        // Fullscreen only, as the readout is.
        for mode in [Mode::Half, Mode::Quarter] {
            for id in guide::MARKS {
                assert!(
                    !shot(state, mode).element(id).unwrap().drawn,
                    "{state} {id}"
                );
            }
        }
    }
    // The approach: the pipper below the box on the screen (nearer the
    // viewer), ticks between them.
    let (b, p, t) = (
        bounds("guide-approach", guide::BOX),
        bounds("guide-approach", guide::PIPPER),
        bounds("guide-approach", guide::TICKS),
    );
    assert!(p.y0 > b.y1, "{p:?} {b:?}");
    assert!(
        t.y1 <= p.y1 && t.y0 >= b.y0,
        "{t:?} between {b:?} and {p:?}"
    );
    // Caught: no ticks, the pipper inside the box, the call above it.
    for (state, label, style) in [
        ("guide-falling-caught", "Will bounce", Style::Go),
        ("guide-too-shallow", "Too shallow", Style::Normal),
        ("guide-too-high", "Too high", Style::Normal),
        ("guide-no-drift", "No drift", Style::Normal),
    ] {
        assert!(!drawn(state, guide::TICKS), "{state}");
        let d = shot(state, Mode::Full).element(guide::BOX).unwrap();
        assert_eq!(
            (d.text.as_deref(), d.style),
            (Some(label), style),
            "{state}"
        );
        let (b, p) = (bounds(state, guide::BOX), bounds(state, guide::PIPPER));
        assert!(
            b.x0 <= p.x0 && p.x1 <= b.x1 && p.y1 <= b.y1 + 1.0,
            "{state}: {p:?} in {b:?}"
        );
    }
    // The arrow is past the box, up the screen: the drift is away.
    let (b, a) = (
        bounds("guide-lined-up", guide::BOX),
        bounds("guide-lined-up", guide::DRIFT),
    );
    assert!(a.y1 <= b.y0 + 2.0, "{a:?} {b:?}");
    // The box's call is below the box, and never over the drift arrow.
    for state in ["guide-falling-caught", "guide-too-shallow"] {
        let d = shot(state, Mode::Full).element(guide::BOX).unwrap();
        let label = d.world.as_ref().unwrap().label.expect(state);
        let arrow = bounds(state, guide::DRIFT);
        assert!(
            arrow.y1 <= label.y0,
            "{state}: arrow {arrow:?} label {label:?}"
        );
        assert_eq!(
            label.intersect(&arrow),
            None,
            "{state}: {label:?} over {arrow:?}"
        );
    }
    // The go green is drawn green: the box's top-left corner pixel.
    let s = shot("guide-falling-caught", Mode::Full);
    let corner = preview::project(&frame, [-guide::BOX_ALONG, guide::BOX_ACROSS, 0.02]);
    let at = ((corner[1].round() as u32 * s.canvas.width + corner[0].round() as u32) * 4) as usize;
    let px = &s.canvas.rgba[at..at + 3];
    assert!(px[1] > px[0] && px[1] > px[2], "{px:?}");
    // On the ground the takeoff line is drawn below the box on the
    // screen (nearer the viewer), green on it; a fall has none.
    for state in [
        "guide-approach",
        "guide-lined-up",
        "guide-jump-now",
        "guide-walk-to-line",
    ] {
        assert!(drawn(state, guide::TAKEOFF), "{state}");
        let (b, t) = (bounds(state, guide::BOX), bounds(state, guide::TAKEOFF));
        assert!(t.y0 > b.y1, "{state}: {t:?} below {b:?}");
    }
    let d = shot("guide-jump-now", Mode::Full)
        .element(guide::TAKEOFF)
        .unwrap();
    assert_eq!(d.style, Style::Go);
    assert!(!drawn("guide-falling-caught", guide::TAKEOFF));
    // The other states draw no mark.
    for id in guide::MARKS {
        assert!(!drawn("falling-on-seam", id), "{id}");
    }
}

/// The preview's world sample is an oblique view of the floor: +X goes
/// right, depth and height go up the screen, and a split frame draws it
/// smaller.
#[test]
fn the_world_sample_is_an_oblique_view() {
    let full = Frame {
        top: 48,
        left: 48,
        bottom: 444,
        right: 592,
    };
    let o = preview::project(&full, [0.0; 3]);
    assert_eq!(o, [320.0, 246.0 + preview::WORLD_SAMPLE[1] * 396.0]);
    let x = preview::project(&full, [1.0, 0.0, 0.0]);
    assert_eq!([x[0] - o[0], x[1] - o[1]], [preview::WORLD_SCALE, 0.0]);
    let y = preview::project(&full, [0.0, 1.0, 0.0]);
    assert_eq!(y[0], o[0]);
    assert!((o[1] - y[1] - preview::WORLD_SCALE * preview::WORLD_TILT).abs() < 1e-3);
    let z = preview::project(&full, [0.0, 0.0, 1.0]);
    assert!((o[1] - z[1] - preview::WORLD_SCALE).abs() < 1e-3);
    let half = Frame {
        top: 18,
        left: 48,
        bottom: 216,
        right: 592,
    };
    let (ho, hx) = (
        preview::project(&half, [0.0; 3]),
        preview::project(&half, [1.0, 0.0, 0.0]),
    );
    assert!((hx[0] - ho[0] - preview::WORLD_SCALE / 2.0).abs() < 1e-3);
}

// ---- K5: line marks, team colours, HUDs that never show together ----

/// A one-text HUD at a fixed place, named, that may declare HUDs it never
/// shows with.
struct Apart {
    name: &'static str,
    never: &'static [&'static str],
}

impl Describe for Apart {
    fn hud(&self) -> Hud {
        Hud {
            name: self.name,
            elements: vec![Element::text(
                "t",
                Font::Hud,
                Justify::Start,
                "AB",
                bl(10, -150),
            )],
        }
    }
    fn states(&self) -> Vec<(&'static str, Values)> {
        vec![("s", Values(vec![("t", Value::text("AB"))]))]
    }
    fn never_with(&self) -> &'static [&'static str] {
        self.never
    }
}

/// Two HUDs with a text in one place overlap across HUDs: the `own`
/// check says so. Two that declare each other (`Describe::never_with`)
/// are not compared; a HUD that declares another the other does not
/// declare back is not compared with it either, and a third is.
#[test]
fn huds_that_declare_each_other_are_not_compared_by_own() {
    let own = |ds: &[&dyn Describe]| {
        check::all(&mut synthetic(), ds, &[Mode::Full])
            .unwrap()
            .of(Check::Own)
            .count()
    };
    let plain = |name| Apart { name, never: &[] };
    assert_eq!(own(&[&plain("a"), &plain("b")]), 1);
    let a = Apart {
        name: "a",
        never: &["b"],
    };
    let b = Apart {
        name: "b",
        never: &["a"],
    };
    assert_eq!(own(&[&a, &b]), 0, "each declares the other");
    assert_eq!(own(&[&a, &plain("b")]), 0, "one declares the other");
    // Only the pair that declares is let off.
    assert_eq!(own(&[&a, &b, &plain("c")]), 2);
}

/// The world check refuses a team colour on a widget or a channel that is
/// not a world mark, and on a team the game does not have; a line in a
/// team colour passes.
#[test]
fn the_world_check_refuses_team_off_a_world_mark() {
    use hud_kit_core::world::{Pose, Shape, TEAMS};
    let line = |id| {
        Element::channel(
            id,
            Kind::WorldMark {
                shape: Shape::Line,
                longest: "",
            },
        )
    };
    let text = Element::text("t", Font::Hud, Justify::Start, "AB", bl(10, -150));
    let run = |value: Value, element: Element| {
        let id = element.id;
        let c = WorldCase {
            hud: Hud {
                name: "world",
                elements: vec![element],
            },
            states: vec![("s", Values(vec![(id, value)]))],
            poses: if id == "l" {
                vec![("l", Pose::at([0.0; 3], [0.3, 0.0]))]
            } else {
                vec![]
            },
        };
        check::hud(&mut synthetic(), &c, &[Mode::Full])
            .unwrap()
            .of(Check::World)
            .map(|f| f.text.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        run(Value::text("AB").styled(Style::Team(1)), text),
        ["t is a text in team 1's colour, a world mark's alone"]
    );
    assert!(run(Value::shown().styled(Style::Team(TEAMS - 1)), line("l")).is_empty());
    assert_eq!(
        run(Value::shown().styled(Style::Team(TEAMS)), line("l")),
        [format!(
            "l is in team {TEAMS}'s colour; the game has {TEAMS} teams"
        )]
    );
    assert_eq!(Style::Team(3).name(), "team 3");
}

/// The preview draws a line, in a team's stand-in colour: the segment's
/// middle pixel is that colour, and a second team's differs.
#[test]
fn the_preview_draws_a_line_in_a_team_colour() {
    use hud_kit_core::world::{Pose, Shape};
    let shot = |team: u8| {
        let c = WorldCase {
            hud: Hud {
                name: "world",
                elements: vec![Element::channel(
                    "l",
                    Kind::WorldMark {
                        shape: Shape::Line,
                        longest: "",
                    },
                )],
            },
            states: vec![(
                "s",
                Values(vec![("l", Value::shown().styled(Style::Team(team)))]),
            )],
            poses: vec![("l", Pose::at([0.0; 3], [0.4, 0.0]))],
        };
        let r = check::hud(&mut synthetic(), &c, &[Mode::Full]).unwrap();
        assert!(r.findings.is_empty(), "{:#?}", r.findings);
        r.shots.into_iter().next().unwrap()
    };
    let pixel = |s: &preview::Shot| {
        let d = s.element("l").unwrap();
        assert!(d.drawn);
        assert_eq!(d.world.as_ref().unwrap().edges, 1);
        let at = preview::project(&s.frame, [0.0, 0.0, 0.02]);
        let i = ((at[1].round() as u32 * s.canvas.width + at[0].round() as u32) * 4) as usize;
        s.canvas.rgba[i..i + 3].to_vec()
    };
    let (red, blue) = (pixel(&shot(0)), pixel(&shot(1)));
    assert_eq!(red, preview::PREVIEW_TEAMS[0].to_vec());
    assert_eq!(blue, preview::PREVIEW_TEAMS[1].to_vec());
}

/// The banner is drawn where the message routine draws its timed line:
/// one line centred across the frame, its top 40 pixels above the frame's
/// bottom, in every screen mode its place has, in the notifications'
/// colour; a short banner passes the prompt check.
#[test]
fn the_banner_is_drawn_centred_above_the_frames_bottom() {
    let mut banner = Element::channel(
        "b",
        Kind::Banner {
            longest: "ROUND COMPLETE",
        },
    );
    banner.at = Place::GAME.split((0, 0), (0, 0));
    let c = case(vec![banner], &[("b", Value::text("WAVE 3"))]);
    let r = check::hud(&mut synthetic(), &c, &Mode::ALL).unwrap();
    let prompt: Vec<_> = r.of(Check::Prompt).collect();
    assert!(prompt.is_empty(), "{prompt:#?}");
    for mode in Mode::ALL {
        let s = r.shots.iter().find(|s| s.mode == mode).unwrap();
        let d = s.element("b").unwrap();
        assert!(d.channel && d.drawn, "{mode:?}");
        assert_eq!(d.kind, "banner");
        assert_eq!(d.text.as_deref(), Some("WAVE 3"));
        let rect = d.rect.unwrap();
        let f = &s.frame;
        let centre = (f32::from(f.left) + f32::from(f.right)) / 2.0;
        assert!(
            ((rect.x0 + rect.x1) / 2.0 - centre).abs() <= 1.0,
            "{mode:?}: {rect:?}"
        );
        assert_eq!(
            rect.y0,
            f32::from(f.bottom) - preview::BANNER_UP,
            "{mode:?}"
        );
        // The longest is reserved wider than the text drawn.
        let reserved = d.reserved.unwrap();
        assert!(reserved.x1 - reserved.x0 >= rect.x1 - rect.x0, "{mode:?}");
    }
    // Fullscreen only unless its place says otherwise.
    let full = case(
        vec![Element::channel("b", Kind::Banner { longest: "WAVE 3" })],
        &[("b", Value::text("WAVE 3"))],
    );
    let r = check::hud(&mut synthetic(), &full, &[Mode::Half]).unwrap();
    let d = r.shots[0].element("b").unwrap();
    assert!(!d.drawn && d.reserved.is_none());
}

/// The Firefight HUD on the synthetic sheet (`h2 hud preview firefight
/// --state all --mode all --synthetic`): every state and mode passes every
/// check but the glyphs (no fonts); in a fighting state the lines, the
/// meter, the set's numeral, the lit pips and the skulls on draw at full
/// and half size, a quarter screen draws the lives line and the pips
/// alone, the banner (the game's timed line) draws at full and half size,
/// the progress line has words only in a bonus round, and a skull off
/// draws nothing.
#[test]
fn the_firefight_hud_draws_and_passes_on_the_synthetic_sheet() {
    use crate::views::firefight::{self as ff, Firefight};
    let mut src = synthetic();
    let r = check::all(&mut src, &[&Firefight], &Mode::ALL).unwrap();
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
    assert_eq!(r.shots.len(), Firefight.states().len() * 3);
    let shot = |state: &str, mode: Mode| {
        r.shots
            .iter()
            .find(|s| s.state == state && s.mode == mode)
            .unwrap()
    };
    for mode in [Mode::Full, Mode::Half] {
        let s = shot("fighting", mode);
        for id in [
            ff::SCORE, ff::LIVES, ff::ENEMIES, ff::SKULLS[0], ff::SKULLS[5], ff::SET,
            ff::ROUNDS[0], ff::WAVES[0],
        ] {
            assert!(s.element(id).unwrap().drawn, "{mode:?} {id}");
        }
        for id in [ff::BANNER, ff::PROGRESS, ff::SKULLS[1], ff::SKULLS[6]] {
            assert!(!s.element(id).unwrap().drawn, "{mode:?} {id}");
        }
        let b = shot("wave-banner", mode);
        assert!(b.element(ff::BANNER).unwrap().drawn, "{mode:?}");
        assert!(b.element(ff::BANNER).unwrap().channel, "{mode:?}");
        assert!(shot("bonus", mode).element(ff::PROGRESS).unwrap().drawn, "{mode:?}");
        assert_eq!(b.element(ff::BANNER).unwrap().text.as_deref(), Some("WAVE 3"));
        // The skulls sit right to left, one slot a skull.
        let x = |id: &str| s.element(id).unwrap().rect.unwrap().x0;
        assert!(x(ff::SKULLS[5]) < x(ff::SKULLS[0]), "{mode:?}");
        // The block is stacked up from the corner: score, lives, the
        // meter, the pips' row, the skulls; the row reads set, rounds,
        // waves from the left.
        let y = |id: &str| s.element(id).unwrap().rect.unwrap().y0;
        assert!(y(ff::ENEMIES) < y(ff::LIVES) && y(ff::LIVES) < y(ff::SCORE), "{mode:?}");
        assert!(y(ff::WAVES[0]) < y(ff::ENEMIES), "{mode:?}");
        assert!(y(ff::SKULLS[0]) < y(ff::WAVES[0]), "{mode:?}");
        assert!(x(ff::SET) < x(ff::ROUNDS[0]) && x(ff::ROUNDS[0]) < x(ff::WAVES[0]), "{mode:?}");
        assert!(x(ff::ROUNDS[0]) < x(ff::ROUNDS[1]) && x(ff::WAVES[0]) < x(ff::WAVES[1]), "{mode:?}");
    }
    let q = shot("fighting", Mode::Quarter);
    for id in [ff::LIVES, ff::SET, ff::ROUNDS[0], ff::WAVES[0]] {
        assert!(q.element(id).unwrap().drawn, "{id}");
    }
    for id in [ff::SCORE, ff::ENEMIES, ff::SKULLS[0], ff::BANNER] {
        assert!(!q.element(id).unwrap().drawn, "quarter {id}");
    }
    // Between waves the meter is gone and the lines stay.
    let between = shot("between-waves", Mode::Full);
    assert!(!between.element(ff::ENEMIES).unwrap().drawn);
    assert!(between.element(ff::LIVES).unwrap().drawn);
}

/// A gizmo axis is a segment in the axis' colour: the check refuses an
/// axis off a world mark and a fourth axis, the preview draws the
/// segment's middle in the axis' colour, and the three differ.
#[test]
fn the_preview_draws_a_gizmo_segment_in_its_axis_colour() {
    use hud_kit_core::world::{AXES, AXIS_COLOURS, Pose, Shape};
    let seg = |id| {
        Element::channel(
            id,
            Kind::WorldMark {
                shape: Shape::Segment,
                longest: "",
            },
        )
    };
    let run = |element: Element, style: Style, pose: Option<Pose>| {
        let id = element.id;
        let c = WorldCase {
            hud: Hud {
                name: "world",
                elements: vec![element],
            },
            states: vec![("s", Values(vec![(id, Value::shown().styled(style))]))],
            poses: pose.map(|p| vec![(id, p)]).unwrap_or_default(),
        };
        check::hud(&mut synthetic(), &c, &[Mode::Full]).unwrap()
    };
    let pose = Pose::at([-0.2, 0.0, 0.0], [0.0, 0.0]).to([0.2, 0.0, 0.0]);
    let pixel = |axis: u8| {
        let r = run(seg("s"), Style::Axis(axis), Some(pose));
        assert!(r.findings.is_empty(), "{:#?}", r.findings);
        let s = r.shots.into_iter().next().unwrap();
        let d = s.element("s").unwrap();
        assert!(d.drawn);
        assert_eq!(d.world.as_ref().unwrap().edges, 1);
        let at = preview::project(&s.frame, [0.0, 0.0, 0.0]);
        let i = ((at[1].round() as u32 * s.canvas.width + at[0].round() as u32) * 4) as usize;
        s.canvas.rgba[i..i + 3].to_vec()
    };
    let colours: Vec<Vec<u8>> = (0..AXES).map(pixel).collect();
    for (axis, got) in colours.iter().enumerate() {
        let want = AXIS_COLOURS[axis].map(|v| (v * 255.0).round() as u8);
        assert_eq!(got, &want.to_vec(), "axis {axis}");
    }
    let texts = |element, style, pose| -> Vec<String> {
        run(element, style, pose)
            .of(Check::World)
            .map(|f| f.text.clone())
            .collect()
    };
    assert_eq!(
        texts(seg("s"), Style::Axis(AXES), Some(pose)),
        [format!("s is in axis {AXES}'s colour; a gizmo has {AXES} axes")]
    );
    let text = Element::text("t", Font::Hud, Justify::Start, "AB", bl(10, -150));
    assert!(texts(text, Style::Axis(1), None).iter().any(|t| t == "t is a text in axis 1's colour, a world mark's alone"));
    assert_eq!(Style::Axis(2).name(), "axis 2");
}

/// The monitor's HUD (`Describe::monitor`) on the synthetic map: drawn
/// over the motion tracker alone, the retail check meets nothing else,
/// and its budget pill is the grenade row's first place (the panel's
/// left end, where the retail pills would be), not one step past the
/// frag pill.
#[test]
fn the_monitor_hud_is_over_the_tracker_alone_and_its_pill_is_the_rows_first() {
    use crate::views::forge::{BUDGET_PILL, Forge};
    assert!(Forge.monitor() && !PillBlock.monitor());
    let mut src = synthetic();
    let mut retail = hud_edit::model::read(&mut src.map, src.player).unwrap();
    assert!(retail.widgets.iter().any(|w| w.name != "motion_sensor"));
    preview::monitor_retail(&mut retail);
    assert!(!retail.widgets.is_empty());
    assert!(retail.widgets.iter().all(|w| w.name == "motion_sensor"));
    let r = check::hud(&mut src, &Forge, &[Mode::Full]).unwrap();
    assert_eq!(r.of(Check::Retail).count(), 0, "{:?}", r.findings);
    let s = r
        .shots
        .iter()
        .find(|s| s.element(BUDGET_PILL).is_some_and(|d| d.drawn))
        .expect("a state that shows the budget");
    assert!(s.retail.iter().all(|p| p.name == "motion_sensor"));
    let f = Frame::for_mode(Mode::Full);
    let at = s.element(BUDGET_PILL).unwrap().rect.unwrap().pixels();
    assert_eq!(
        [at[0], at[1]],
        [
            i32::from(f.left + synthetic::PANEL_AT.0),
            i32::from(f.top + synthetic::PANEL_AT.1)
        ]
    );
}
