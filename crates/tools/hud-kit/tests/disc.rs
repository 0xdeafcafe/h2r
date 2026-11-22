//! The kit on the owner's disc (`H2_DISC`, else `disc/halo2.iso`): the
//! templates on every map (the preview's and the runtime's the same), the
//! retail rectangles where the HUD editor places the retail widgets,
//! every registered HUD passing every check with nothing skipped, and the
//! glyphs in fonts 5, 6 and 8 (the superbounce medals as the font rewrite
//! adds them).

use std::path::PathBuf;

use hud_edit::Mode;
use hud_edit::model::WidgetKind;
use hud_edit::place::{self, Frame};
use hud_kit::check;
use hud_kit::{Source, huds, rects, templates};
use hud_kit_core::map_file;
use scenario_edit::MapSource;

fn iso() -> PathBuf {
    std::env::var_os("H2_DISC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
        })
}

/// The map the sheet is checked on (`hud_edit`'s disc test's).
const MAP: &str = "lockout";

/// Every map with a player HUD resolves every registered HUD's
/// templates, and the runtime's reader
/// (`hud_kit_core::map_file::MapFile`) resolves the same ones. The shared
/// caches and the main menu have no player HUD and are skipped; a HUD
/// that is multiplayer only (`Describe::multiplayer_only`) is not
/// resolved on a campaign map (its player HUD has no grenade row), and
/// each such skip is counted with its reason.
#[test]
#[ignore = "needs the owner's disc image (H2_DISC)"]
fn templates_resolve_on_every_map() {
    let iso = iso();
    let maps = MapSource::disc_maps(&iso).unwrap();
    let (mut checked, mut multiplayer) = (0, 0);
    let mut skipped: Vec<String> = Vec::new();
    for (name, source) in &maps {
        if ["shared", "single_player_shared", "mainmenu"]
            .iter()
            .any(|s| name.eq_ignore_ascii_case(s))
        {
            continue;
        }
        let mut src = Source::disc(&iso, name).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let mp = src.multiplayer();
        for d in huds::all() {
            let hud = d.hud();
            if let (false, Some(why)) = (mp, d.multiplayer_only()) {
                skipped.push(format!("{name}: {}: {why}", hud.name));
                continue;
            }
            let t = src.templates(&hud);
            assert_eq!(t.missing(&hud), Vec::<String>::new(), "{name}");
            // The runtime's reader, on the same map file.
            let mut map = source.open().unwrap();
            let mut files = scenario_edit::scene::resource_files(source);
            let player = hud_kit_core::map_file::player_huds(&mut map);
            assert_eq!(player.first(), Some(&src.player), "{name}: the player nhdt");
            let mut view = hud_kit_core::map_file::MapFile::new(&mut map, &mut files);
            let rt = hud_kit_core::templates::resolve(&mut view, src.player, &hud);
            assert_eq!(rt.row, t.row, "{name}: the grenade row");
            assert_eq!(rt.text, t.text, "{name}: the text template");
            // The arrows are the one deliberate difference: the runtime
            // leaves them to the game's navpoint list
            // (`map_file::NO_ARROWS`), the preview draws them.
            let (rt_arrows, rt_art): (Vec<_>, Vec<_>) =
                rt.art.iter().partition(|(a, _)| map_file::unresolved(a));
            let t_art: Vec<_> = t
                .art
                .iter()
                .filter(|(a, _)| !map_file::unresolved(a))
                .collect();
            assert_eq!(rt_art, t_art, "{name}: the art templates");
            for (a, r) in rt_arrows {
                assert_eq!(r, &Err(map_file::NO_ARROWS.to_owned()), "{name}: {a:?}");
            }
        }
        checked += 1;
        multiplayer += usize::from(mp);
    }
    assert!(checked > 10, "{checked} maps");
    assert!(multiplayer > 10, "{multiplayer} multiplayer maps");
    eprintln!("skipped, by reason:\n{}", skipped.join("\n"));
}

/// The rectangles of `rects` are where the HUD editor places the retail
/// player HUD's tracker and shield meter, and the converter's health
/// strip where `rects::above` puts it. The sheet's retail state words
/// (`Source::retail_words`) let the tracker through the gate.
#[test]
#[ignore = "needs the owner's disc image (H2_DISC)"]
fn the_retail_rectangles_match_the_editors_placement() {
    let mut src = Source::disc(&iso(), MAP).unwrap();
    let name = src.map.tag(src.player).unwrap().name.clone();
    assert_eq!(name.as_deref(), Some(templates::PLAYER_HUD));
    let hud = hud_edit::model::read(&mut src.map, src.player).unwrap();
    // At the sheet's HUD state, so the retail check sees them.
    let st = hud_kit::preview::retail_states(Mode::Full, src.words).remove(0);
    eprintln!("{}", src.words_note);
    let p = hud_edit::render::draw(&mut src.map, &mut src.art, src.fonts.as_ref(), &[&hud], &st);
    let drawn: Vec<_> = p
        .placed
        .iter()
        .filter(|w| w.drawn && w.key.kind == WidgetKind::Bitmap)
        .collect();
    assert!(
        drawn
            .iter()
            .any(|w| w.rect.unwrap().pixels() == rects::TRACKER.pixels()),
        "no drawn widget on the tracker: {:?}",
        drawn
            .iter()
            .map(|w| (&w.name, w.rect.unwrap().pixels()))
            .collect::<Vec<_>>()
    );
    let shield = hud
        .widgets
        .iter()
        .find(|w| {
            w.key.kind == WidgetKind::Bitmap && w.special() == hud_edit::layout::SPECIAL_SHIELD
        })
        .unwrap();
    let r = p.placed.iter().find(|w| w.key == shield.key).unwrap();
    assert!(r.drawn, "the shield meter draws at the sheet's state");
    let r = r.rect.unwrap();
    assert!(r.contains(&rects::SHIELD_BAND), "{r:?}");
    assert_eq!([r.x0, r.y0], [rects::SHIELD_BAND.x0, rects::SHIELD_BAND.y0]);
    let strip = rects::above(rects::SHIELD_BAND, 2, 8);
    let pt = place::widget_point(&Frame::FULLSCREEN, strip.anchor as u16, strip.full).unwrap();
    assert_eq!(
        place::bitmap_rect(pt, [92.0, 8.0], [0.0, 0.0]),
        rects::H1_HEALTH
    );
    // The grenade row: one 103-wide panel at the anchor, the counts 53
    // apart, so the next pill one step past the frag pill (at 53).
    let t = src.templates(&hud_kit::huds::pill_block::hud());
    let row = t.row.as_ref().unwrap();
    assert_eq!((row.panel, row.frag), (103.0, [53.0, 50.0]));
    assert_eq!(row.pill[0], (106, 0));
    assert_eq!(row.count[0], (118, -1));
}

/// Every registered HUD passes every check on the disc, nothing skipped,
/// in every screen mode.
#[test]
#[ignore = "needs the owner's disc image (H2_DISC)"]
fn every_hud_passes_every_check_on_the_disc() {
    let mut src = Source::disc(&iso(), MAP).unwrap();
    assert!(src.fonts.is_some(), "the disc's fonts");
    let all = huds::all();
    let ds: Vec<&dyn hud_kit::Describe> = all.iter().map(|d| d.as_ref()).collect();
    let r = check::all(&mut src, &ds, &Mode::ALL).unwrap();
    let text: Vec<String> = r.findings.iter().map(ToString::to_string).collect();
    assert!(r.findings.is_empty(), "{text:#?}");
    assert!(r.skipped.is_empty(), "{:?}", r.skipped);
    assert!(!r.shots.is_empty());
}

/// The X button's glyph and every character the registered HUDs use are
/// in fonts 6 and 8 (and 5, the split-screen message font).
#[test]
#[ignore = "needs the owner's disc image (H2_DISC)"]
fn the_glyphs_are_in_the_fonts() {
    let src = Source::disc(&iso(), MAP).unwrap();
    let fonts = src.fonts.as_ref().expect("the disc's fonts");
    for f in [5, 6, 8] {
        assert_eq!(fonts.has_glyph(f, 0xe102), Some(true), "X in font {f}");
    }
    // The superbounce medals: new art the font rewrite adds to the two
    // message fonts (`font::medals`), not retail glyphs.
    for f in [5, 6] {
        for (m, c) in hud_kit::huds::superbounce::MEDAL_GLYPHS {
            assert_eq!(
                fonts.has_glyph(f, c as u16),
                Some(true),
                "{m:?} in font {f}"
            );
        }
    }
    let mut r = check::Report::default();
    for d in huds::all() {
        check::glyphs(&mut r, &d.hud(), &d.states(), &|f, u| fonts.has_glyph(f, u));
    }
    let text: Vec<String> = r.findings.iter().map(ToString::to_string).collect();
    assert!(text.is_empty(), "{text:#?}");
}
