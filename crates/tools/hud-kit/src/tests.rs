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
use crate::huds::superbounce::{self, Superbounce};
use crate::model::*;
use crate::preview;
use crate::templates::{self, Templates};
use crate::{Source, compile, hudg};

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
    v.0.retain(|(id, _)| *id != superbounce::DROP);
    v.0.push(("sb.nope", Value::shown()));
    v.0.push((superbounce::LAUNCH, Value::shown()));
    let p = v.layout_problems(&hud);
    assert!(p.contains(&"sb.drop has no value".to_owned()), "{p:?}");
    assert!(p.contains(&"sb.launch has 2 values".to_owned()), "{p:?}");
    assert!(
        p.contains(&"sb.nope is not an element of superbounce".to_owned()),
        "{p:?}"
    );
    let mut twice = hud.clone();
    twice.elements.push(twice.elements[0].clone());
    assert_eq!(twice.problems(), ["sb.status is named twice"]);
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
    let t = Templates::resolve(&mut map, player, hudg.as_ref(), &hud);
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
    let t = Templates::resolve(&mut map, player, None, &h2);
    assert!(t.art(&nope).unwrap_err().contains("no bitmap widget nope"));
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
    assert_eq!(r.shots.len(), 9 * 3);
    let (sheet, tiles) = preview::sheet(&r.shots, 3);
    assert_eq!((sheet.width, sheet.height), (3 * 320, 9 * 240));
    assert_eq!(tiles["tiles"].as_array().unwrap().len(), 27);
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
    assert_eq!(
        drawn,
        [
            "sb.status",
            "sb.pointer",
            "sb.distance",
            "sb.distance.unit",
            "sb.drop.label",
            "sb.drop",
            "sb.drop.unit",
            "sb.launch.meter",
            "sb.launch"
        ]
    );
    let at = |id| s.element(id).unwrap().rect.unwrap().pixels();
    // The table's rectangles (docs/game/hud-kit.md, "The block"), in the
    // stand-in font: 7 pixels a character, 12 high.
    assert_eq!(at("sb.status"), [47, 272, 47 + 7 * 10, 284]);
    assert_eq!(at("sb.pointer"), [47, 296, 63, 312]);
    assert_eq!(at("sb.distance"), [82, 296, 110, 308]);
    assert_eq!(at("sb.launch.meter"), [47, 320, 139, 328]);
    assert_eq!(at("sb.launch"), [145, 316, 173, 328]);
    // The retail tracker and shield meter are in the picture.
    assert!(
        s.retail
            .iter()
            .any(|p| p.drawn && p.rect.unwrap().pixels() == [47, 360, 130, 443])
    );
    // In split screen the block and the tutorial are hidden, and say why.
    for (state, id) in [
        ("not-primed", superbounce::STATUS),
        ("tutorial-1", superbounce::TUTORIAL),
    ] {
        let half = r
            .shots
            .iter()
            .find(|s| s.state == state && s.mode == Mode::Half)
            .unwrap();
        let d = half.element(id).unwrap();
        assert!(
            !d.drawn && d.hidden.as_deref() == Some("no half screen offset"),
            "{id}"
        );
    }
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
    assert_eq!(back["elements"][0]["id"], "sb.status");
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
    assert!(compared > 40, "{compared}");
}

/// A `Dim` text is drawn fainter than a `Normal` one; the preview's kit
/// text colour is the widget stand-in.
#[test]
fn styles_change_the_text_colour() {
    let mut src = synthetic();
    let r = check::hud(&mut src, &Superbounce, &[Mode::Full]).unwrap();
    let px = |state: &str| {
        let s = r.shots.iter().find(|s| s.state == state).unwrap();
        let rect = s.element(superbounce::STATUS).unwrap().rect.unwrap();
        let (x, y) = (rect.x0 as u32 + 1, rect.y0 as u32 + 4);
        let i = ((y * s.canvas.width + x) * 4) as usize;
        s.canvas.rgba[i..i + 3].to_vec()
    };
    let normal = px("primed-airborne");
    assert_eq!(normal, render::TEXT_COLOUR[..3]);
    assert_ne!(px("not-primed"), normal, "dim");
    assert_eq!(
        px("primed"),
        src.colours.flash(Pulse::Objective)[..3],
        "the objective flash at its start"
    );
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
    // prompt: three lines in the training box.
    fires(
        case(
            vec![Element::channel("t", Kind::Training { longest: "one" })],
            &[("t", Value::text("one\ntwo\nthree"))],
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
    // glyphs: a font without the X button's glyph.
    let mut r = check::Report::default();
    let no_x = |_: i16, u: u16| Some(u != 0xe102);
    check::glyphs(&mut r, &superbounce::hud(), &Superbounce.states(), &no_x);
    assert!(!r.findings.is_empty());
    assert!(
        r.findings
            .iter()
            .all(|f| f.check == Check::Glyphs && f.elements == ["sb.tutorial"])
    );
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
