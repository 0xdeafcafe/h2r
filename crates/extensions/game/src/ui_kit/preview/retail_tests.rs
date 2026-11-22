//! The retail screens drawn from the disc in their sample states. Skipped
//! without a disc (`H2_DISC`, else `disc/halo2.iso`); with
//! `H2_UI_PREVIEW_DIR` set the pictures are written there.

use super::retail_sample::{self, TOKENS};
use super::{Canvas, Disc, Retail, RetailBlocks, RetailEdits, RetailPlaces, Source};
use crate::ui_kit::view::{Backing, Kind, View};

const LOBBY: &str = r"ui\screens\game_shell\pregame_lobby\pregame_lobby";
const SETUP: &str = r"ui\screens\game_shell\pregame_lobby\squad_settings_dialog";

fn disc() -> Option<Disc> {
    let iso = std::env::var_os("H2_DISC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
        });
    if !iso.exists() {
        eprintln!("no disc at {}; skipped", iso.display());
        return None;
    }
    Some(Disc::open(&iso).unwrap())
}

fn no_text(_: u32) -> Option<String> {
    None
}

fn pick(tag: &str, pane: usize) -> Retail<'_> {
    Retail {
        tag,
        pane,
        text: &no_text,
        cells: &[],
        footer: None,
        rows: None,
        sample: None,
    }
}

/// The widget texts of `v` by id.
fn texts(v: &View) -> Vec<(&str, &str)> {
    v.widgets
        .iter()
        .filter_map(|w| match &w.kind {
            Kind::Text { text, .. } => Some((w.id.as_str(), text.as_str())),
            _ => None,
        })
        .collect()
}

/// No retail screen shows a substitution unit, with a sample or without:
/// the sample replaced it, or the block is hidden.
#[test]
fn no_retail_screen_shows_a_substitution_unit() {
    let Some(d) = disc() else { return };
    let (mut seen, mut blank) = (0, 0);
    for tag in d.retail_screens() {
        // A screen with no pane has nothing to draw.
        let Ok(v) = d.retail_view(&pick(&tag, 0), &RetailPlaces::new()) else {
            continue;
        };
        seen += 1;
        for (id, text) in texts(&v) {
            assert!(
                !text.chars().any(|c| TOKENS.contains(&c)),
                "{tag} {id} shows a unit the game replaces: {text:?}"
            );
            blank += usize::from(text.is_empty());
        }
    }
    assert!(seen > 20, "{seen} retail screens drawn");
    assert!(blank > 0, "some block somewhere is the game's to fill");
}

/// The lobby's sample gives its panes a state: the leader's line hidden
/// under the buttons, the count and the status filled in, and each pane
/// its own blocks alone.
#[test]
fn the_lobby_is_drawn_one_pane_in_its_sample_state() {
    let Some(d) = disc() else { return };
    let sample = retail_sample::for_screen(LOBBY).unwrap().expect("the lobby has a sample");
    let views: Vec<View> = (0..3)
        .map(|p| d.retail_view(&pick(LOBBY, p), &RetailPlaces::new()).unwrap())
        .collect();
    assert!(d.retail_view(&pick(LOBBY, 3), &RetailPlaces::new()).is_err(), "three panes");
    let custom = texts(&views[0]);
    let of = |id: &str| custom.iter().find(|t| t.0 == id).map(|t| t.1);
    // text.43 is the leader's line, text.44 the count, text.0 the status
    // and text.4 the first quick option (the `h2 ui` dump's order).
    assert_eq!(of("text.43"), Some(""), "the leader has the buttons instead");
    assert!(of("text.44").unwrap().starts_with(&sample.tokens[&'\u{e425}']));
    assert!(!of("text.0").unwrap().is_empty(), "the status line reads something");
    assert_eq!(of("text.4"), Some("Score to Win: 50"));
    assert!(!of("button.0").unwrap().is_empty());
    // The other panes have their own blocks: the co-op pane has a level
    // where the custom game has its quick options, and no pane has
    // another's widgets.
    for (p, v) in views.iter().enumerate() {
        assert_eq!(v.state, format!("pane-{p}"));
        let mut ids: Vec<&str> = v.widgets.iter().map(|w| w.id.as_str()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "pane {p}: every widget once");
    }
    let coop = texts(&views[1]);
    assert!(coop.iter().any(|t| t.1 == sample.tokens[&'\u{e426}']), "the level's name");
    assert!(!custom.iter().any(|t| t.1 == sample.tokens[&'\u{e426}']));
    // The pictures differ pane to pane.
    let shots: Vec<_> = (0..3).map(|p| d.render_retail(&pick(LOBBY, p)).unwrap()).collect();
    assert!(shots[0].rgba != shots[1].rgba && shots[1].rgba != shots[2].rgba);
    write(&shots[0], "retail-lobby-pane-0.png");
    write(&shots[1], "retail-lobby-pane-1.png");
    write(&shots[2], "retail-lobby-pane-2.png");
}

/// Game Setup's rows are the game's code's: the sample's rows are drawn
/// where the tag has only a list.
#[test]
fn a_code_drawn_dialog_draws_its_samples_rows() {
    let Some(d) = disc() else { return };
    let with = d.render_retail(&pick(SETUP, 0)).unwrap();
    write(&with, "retail-game-setup.png");
    // The list's anchor is (-250, 90): its rows run down the dialog's
    // left half, where the tag draws only the dark frame. With the
    // sample's rows the focused row's label is there in white.
    let mut lit = 0;
    for y in 110..250u32 {
        for x in 80..300u32 {
            let i = ((y * with.width + x) * 4) as usize;
            lit += usize::from(with.rgba[i..i + 3].iter().all(|v| *v > 200));
        }
    }
    assert!(lit > 60, "the rows are drawn: {lit} bright pixels");
}

/// A wrapped text keeps to its rectangle in the disc's fonts: the lobby's
/// help text, the longest on the screen.
#[test]
fn a_long_text_is_broken_to_its_rect_in_the_discs_fonts() {
    let Some(d) = disc() else { return };
    let v = d.retail_view(&pick(LOBBY, 0), &RetailPlaces::new()).unwrap();
    let w = v
        .widgets
        .iter()
        .filter_map(|w| match &w.kind {
            Kind::Text { text, font, .. } => Some((w, text, *font)),
            _ => None,
        })
        .max_by_key(|(_, text, _)| text.len())
        .unwrap();
    let r = w.0.rect();
    let lines = d.fonts.wrap(w.2, w.1, r.width());
    assert!(lines.len() > 1, "{:?} is one line in {}", w.1, r.width());
    for l in &lines {
        assert!(l.width < r.width(), "{:?} is {} wide in {}", l.text, l.width, r.width());
    }
    assert!(d.fonts.lines_drawn(w.2, r.height()) >= lines.len(), "all of it shows");
}

/// The colours in `c`'s pixels `x0..x1` by `y0..y1`.
fn colours(c: &Canvas, x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> Vec<[u8; 3]> {
    let mut v = Vec::new();
    for py in y {
        for px in x.clone() {
            let i = ((py * c.width + px) * 4) as usize;
            v.push([c.rgba[i], c.rgba[i + 1], c.rgba[i + 2]]);
        }
    }
    v
}

fn distinct(mut v: Vec<[u8; 3]>) -> usize {
    v.sort_unstable();
    v.dedup();
    v.len()
}

/// The title is in the screen's own header layout, and the layout's
/// rectangle holds one line of its font: the lobby's and Game Setup's
/// are different layouts.
#[test]
fn a_title_is_in_its_screens_own_header_layout() {
    let Some(d) = disc() else { return };
    let title = |tag: &str| {
        let v = d.retail_view(&pick(tag, 0), &RetailPlaces::new()).unwrap();
        let w = v.widget("title").unwrap_or_else(|| panic!("{tag} has no title")).clone();
        let Kind::Text { text, font, .. } = &w.kind else {
            panic!("{tag}: the title is not a text");
        };
        let r = w.rect();
        assert_eq!(d.fonts.wrap(*font, text, r.width()).len(), 1, "{tag}: {text:?} in {}", r.width());
        assert!(
            d.fonts.lines_drawn(*font, r.height()) >= 1,
            "{tag}: a rectangle {} high draws no line of font {font}",
            r.height()
        );
        w.rect
    };
    let (lobby, setup) = (title(LOBBY), title(SETUP));
    assert_ne!(lobby, setup, "each on its own layout");
    assert!(setup[0] < lobby[0], "the dialog's title is the lower: {setup:?} {lobby:?}");
}

/// An empty player slot draws nothing in any pane: the co-op and
/// matchmaking panes' slot bars are hidden as the custom game's are
/// parked.
#[test]
fn an_empty_slot_draws_nothing_in_any_pane() {
    let Some(d) = disc() else { return };
    let shots: Vec<_> = (0..3).map(|p| d.render_retail(&pick(LOBBY, p)).unwrap()).collect();
    // The sixth slot's bar: its point is (113, 65), 168 by 19, between
    // the fourth player and the help text.
    let slot = |c: &Canvas| colours(c, 440..590, 178..192);
    assert_eq!(slot(&shots[1]), slot(&shots[0]), "co-op");
    assert_eq!(slot(&shots[2]), slot(&shots[0]), "matchmaking");
}

/// A player's emblem is drawn in the cell's shader bitmap: left of the
/// first player's name, pixels in the sample's emblem colours.
#[test]
fn a_cells_emblem_is_drawn_in_its_colours() {
    let Some(d) = disc() else { return };
    let sample = retail_sample::for_screen(LOBBY).unwrap().expect("the lobby has a sample");
    let e = sample.cells[0].emblem.as_ref().expect("the first player has an emblem");
    let shot = d.render_retail(&pick(LOBBY, 0)).unwrap();
    // The table's anchor is (113, 137) and the shader bitmap's rectangle
    // [6 -20 -26 12], moved up by its height: the emblem's 19 by 19 is
    // at pixels (413, 65).
    let slot = colours(&shot, 413..432, 65..84);
    let n = slot.iter().filter(|p| e.colours.contains(p)).count();
    assert!(n > 30, "{n} pixels in the emblem's colours");
}

/// The lobby's map picture and Game Setup's panel show a map's picture,
/// and Game Setup is drawn over the lobby.
#[test]
fn a_dialog_is_drawn_over_its_opener_with_its_map_picture() {
    let Some(d) = disc() else { return };
    let setup = pick(SETUP, 0);
    let with = d.render_retail(&setup).unwrap();
    let alone = d.render_retail_layer(&setup, &RetailEdits::default(), false).unwrap();
    assert!(with.rgba != alone.rgba, "the lobby shows around the dialog");
    // The panel's point is (27, 68): its picture starts at pixels
    // (347, 172).
    let panel = distinct(colours(&with, 357..547, 182..360));
    assert!(panel > 100, "the panel has a picture: {panel} colours");
    // The tag alone draws nothing there: the panel block's image is 8,
    // past `xbox_live`'s eight images (0 to 7). The game's code picks the
    // picture per focused row; the sample's `level` stands in for it.
    let parts = d.retail_parts(SETUP).unwrap();
    let b = parts.screen.panes[0]
        .bitmaps
        .iter()
        .find(|b| {
            b.bitmap.id.and_then(|id| parts.bitmaps.get(&id)).is_some_and(|t| t.ends_with(r"\xbox_live"))
        })
        .expect("the panel block draws xbox_live");
    assert_eq!(b.image, 8, "the panel block's image");
    assert!(d.image(b.bitmap, 8).is_none(), "xbox_live has no image 8");
}

/// The first of `rows` in which `a` and `b` differ in pixel columns
/// `x`.
fn first_row_differing(
    a: &Canvas,
    b: &Canvas,
    x: std::ops::Range<u32>,
    rows: std::ops::Range<u32>,
) -> Option<u32> {
    rows.into_iter()
        .find(|&y| colours(a, x.clone(), y..y + 1) != colours(b, x.clone(), y..y + 1))
}

/// A button's art is drawn with its offset's y subtracted from the
/// rect's top (`0x253fb0`): the lobby's (-12, -5) puts the frame's top
/// edge above the label, not through it.
#[test]
fn a_buttons_art_is_raised_by_its_offset() {
    let Some(d) = disc() else { return };
    let parts = d.retail_parts(LOBBY).unwrap();
    let pane = &parts.screen.panes[0];
    let blocks = |buttons: Vec<tags::ui::Button>| RetailBlocks {
        header: parts.screen.header.0,
        text_blocks: pane.text_blocks.clone(),
        buttons,
        bitmaps: pane.bitmaps.clone(),
    };
    let r = pick(LOBBY, 0);
    let full = d.render_retail_layer(&r, &RetailEdits::default(), false).unwrap();
    // The buttons' art alone: their labels blanked.
    let art_only = RetailEdits {
        texts: pane.buttons.iter().map(|b| (b.string.0, String::new())).collect(),
        ..RetailEdits::default()
    };
    let art = d.render_retail_layer(&r, &art_only, false).unwrap();
    // Neither: no buttons.
    let none = RetailEdits {
        blocks: Some(blocks(Vec::new())),
        ..RetailEdits::default()
    };
    let bare = d.render_retail_layer(&r, &none, false).unwrap();
    let art_top = first_row_differing(&art, &bare, 60..200, 55..100).expect("the art is drawn");
    let label_top = first_row_differing(&full, &art, 60..200, 55..100).expect("the label is drawn");
    assert!(art_top < label_top, "the art's top row {art_top}, the label's {label_top}");
    write(&full, "retail-lobby-buttons.png");
}

/// The sum of `c`'s colour channels in pixels `x` by `y`.
fn brightness(c: &Canvas, x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> u64 {
    colours(c, x, y)
        .iter()
        .map(|p| p.iter().map(|v| u64::from(*v)).sum::<u64>())
        .sum()
}

/// A dialog of ours over its opener: the opener drawn as a retail screen
/// in its sample state (no substitution unit in its texts), then dimmed
/// by the shared globals' overlay colour.
#[test]
fn a_picker_is_drawn_over_its_opener_dimmed_and_without_units() {
    let Some(d) = disc() else { return };
    let spec = crate::ui_kit::screens::sample(crate::ui_kit::picker::GAME_NAME).unwrap();
    let v = d.view(&spec, "sample").unwrap();
    let Backing::Over(o) = v.backing else {
        panic!("kit.picker.game is not over an opener: {:?}", v.backing);
    };
    let opener = d.opener_view(&o).unwrap();
    for view in [&v, &opener] {
        for (id, text) in texts(view) {
            assert!(
                !text.chars().any(|c| TOKENS.contains(&c)),
                "{} {id} shows a unit: {text:?}",
                view.screen
            );
        }
    }
    // The opener's rows (Game Setup's list, from (-250, 90)) are darker
    // under the dialog than in the opener alone.
    let alone = d.opener_drawn(&o).expect("the opener is drawn");
    let mut under = Canvas::new(640, 480, [0, 0, 0, 255]);
    Source::backing(&d, &mut under, Backing::Over(o));
    let (rows_x, rows_y) = (70..300, 150..250);
    let (lit, dimmed) = (
        brightness(&alone.1, rows_x.clone(), rows_y.clone()),
        brightness(&under, rows_x, rows_y),
    );
    assert!(dimmed < lit, "under the dialog {dimmed}, alone {lit}");
    write(&alone.1, "opener-game-setup-clankers.png");
    write(&d.render(&spec).unwrap(), "kit-picker-game.png");
}

/// The dim layer is the disc's overlay colour: 85% of (0, 20, 44), the
/// synthetic source's constant.
#[test]
fn the_dim_layer_is_the_discs_overlay_colour() {
    let Some(d) = disc() else { return };
    let mut c = Canvas::new(640, 480, [0, 0, 0, 0]);
    d.dim(&mut c);
    assert_eq!(c.rgba[..4], super::SYNTHETIC_DIM);
}

/// Game Setup's Switch To variant draws its seven rows; the plain draw
/// is the plain sample's, and a name the screen has no sample for fails
/// with the names it has.
#[test]
fn a_named_sample_variant_draws_its_own_rows() {
    let Some(d) = disc() else { return };
    let plain = d.render_retail(&pick(SETUP, 0)).unwrap();
    let switch = d
        .render_retail(&Retail {
            sample: Some("switch-to"),
            ..pick(SETUP, 0)
        })
        .unwrap();
    assert!(plain.rgba != switch.rgba, "the variant's rows differ");
    // All seven rows read in the disc's table, ours between Quick
    // Options and Co-Op.
    let parts = d.retail_parts(SETUP).unwrap();
    let (table, name, named) = parts.strings();
    let state = retail_sample::State {
        sample: retail_sample::for_screen_variant(SETUP, "switch-to").unwrap(),
        pane: 0,
        strings: retail_sample::Strings {
            text: &table,
            name: &name,
            named: &named,
        },
    };
    assert!(state.missing().is_empty(), "{:?}", state.missing());
    let (rows, focus) = state.rows(0).expect("the variant gives the list rows");
    assert_eq!(rows.len(), 7, "{rows:?}");
    assert_eq!((rows[3].as_str(), rows[4].as_str()), ("Switch To: Forge", "Switch To: Firefight"));
    assert_eq!(focus, 0);
    write(&switch, "retail-game-setup-switch-to.png");
    let e = d
        .render_retail(&Retail {
            sample: Some("no-such"),
            ..pick(SETUP, 0)
        })
        .unwrap_err()
        .to_string();
    assert!(e.contains("no-such") && e.contains("switch-to"), "{e}");
}

/// Without a disc: a variant is looked up by name, and an unknown one is
/// refused with the screen's names.
#[test]
fn an_unknown_sample_variant_is_refused_with_the_names() {
    let found = super::sample_of(&Retail {
        sample: Some("switch-to"),
        ..pick(SETUP, 0)
    })
    .unwrap()
    .expect("Game Setup's variant");
    assert_eq!(found.variant.as_deref(), Some("switch-to"));
    let plain = super::sample_of(&pick(SETUP, 0)).unwrap().expect("the plain sample");
    assert_eq!(plain.variant, None);
    let e = super::sample_of(&Retail {
        sample: Some("no-such"),
        ..pick(SETUP, 0)
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("switch-to"), "{e}");
}

fn write(c: &Canvas, file: &str) {
    if let Some(dir) = std::env::var_os("H2_UI_PREVIEW_DIR").map(std::path::PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        c.write_png(&dir.join(file)).unwrap();
        eprintln!("drew {}", dir.join(file).display());
    }
}
