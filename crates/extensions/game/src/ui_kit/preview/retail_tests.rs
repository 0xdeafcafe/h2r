//! The retail screens drawn from the disc in their sample states. Skipped
//! without a disc (`H2_DISC`, else `disc/halo2.iso`); with
//! `H2_UI_PREVIEW_DIR` set the pictures are written there.

use super::retail_sample::{self, TOKENS};
use super::{Disc, Retail, RetailPlaces};
use crate::ui_kit::view::{Kind, View};

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

fn write(c: &super::Canvas, file: &str) {
    if let Some(dir) = std::env::var_os("H2_UI_PREVIEW_DIR").map(std::path::PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        c.write_png(&dir.join(file)).unwrap();
        eprintln!("drew {}", dir.join(file).display());
    }
}
