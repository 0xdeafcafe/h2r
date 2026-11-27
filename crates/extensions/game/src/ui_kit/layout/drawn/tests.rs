use super::*;
use crate::ui_kit::layout::{Rules, check};
use crate::ui_kit::preview::{Canvas, Image, SyntheticSource};
use crate::ui_kit::view::{Backing, Justify, ListAt, Metrics, Synthetic};

fn text(id: &str, rect: [i16; 4], s: &str) -> Widget {
    Widget {
        id: id.into(),
        rect,
        layer: Layer::Content,
        alpha: 255,
        kind: Kind::Text {
            text: s.into(),
            font: 1,
            justify: Justify::Left,
            colour: [255, 255, 255],
        },
    }
}

fn art(id: &str, rect: [i16; 4], layer: Layer, art: Art) -> Widget {
    let [top, left, bottom, right] = rect;
    Widget {
        id: id.into(),
        rect,
        layer,
        alpha: 255,
        kind: Kind::Art {
            art,
            size: [(right - left) as u16, (top - bottom) as u16],
        },
    }
}

fn screen_art(index: u16) -> Art {
    Art::Screen {
        screen_id: 14,
        pane: 0,
        index,
        image: 0,
    }
}

fn bar() -> Art {
    Art::Skin {
        skin: 0,
        index: 0,
        image: 0,
    }
}

fn view(widgets: Vec<Widget>) -> View {
    View {
        screen: "t".into(),
        state: "s".into(),
        backing: Backing::None,
        underlay: None,
        widgets,
        focus: None,
        list: None,
    }
}

fn of_kind<'a>(issues: &'a [Issue], kind: &str) -> Vec<&'a Issue> {
    issues.iter().filter(|i| i.kind() == kind).collect()
}

/// A lobby screen with a mode list on the status panel: the lobby's `lobby_display` (`panel.1`), the mode list's five rows on
/// the `default` skin from `[-262, 114]`, and the game's lines right of
/// them at `[135, -40, 61, 105]`. The rects never meet; the panel holds
/// both.
fn lobby_f(with_game: bool, list_top: i16) -> View {
    let mut w = vec![art("panel.1", [149, -283, 21, 229], Layer::Panel, screen_art(1))];
    for (i, mode) in ["Multiplayer", "Custom Games", "Forge", "Firefight", "Co-op"].iter().enumerate() {
        let oy = list_top - 22 * i as i16;
        w.push(art(&format!("row.{i}.art.0"), [oy + 27, -252, oy + 6, -52], Layer::Under, bar()));
        w.push(text(&format!("row.{i}.label"), [oy + 28, -242, oy + 8, -52], mode));
    }
    if with_game {
        w.push(text("game", [135, -40, 61, 105], "Forge\nLockout"));
    }
    view(w)
}

#[test]
fn a_row_list_in_the_status_panel_beside_its_text_is_found() {
    let r = check(&lobby_f(true, 114), &SyntheticSource, &Rules::default());
    // The old checks see nothing: the rects don't meet.
    assert!(of_kind(&r.issues, "on_other_art").is_empty(), "{:?}", r.issues);
    assert!(of_kind(&r.issues, "text_overlap").is_empty(), "{:?}", r.issues);
    assert_eq!(
        of_kind(&r.issues, "panel_shared"),
        [&Issue::PanelShared {
            panel: "panel.1".into(),
            rows: "row.0.art.0".into(),
            with: vec!["game".into()],
        }]
    );
    // The panel with the list alone, or the list out of the panel, is fine.
    let alone = check(&lobby_f(false, 114), &SyntheticSource, &Rules::default());
    assert!(of_kind(&alone.issues, "panel_shared").is_empty(), "{:?}", alone.issues);
    let below = check(&lobby_f(true, -10), &SyntheticSource, &Rules::default());
    assert!(of_kind(&below.issues, "panel_shared").is_empty(), "{:?}", below.issues);
    // A modal's panel holds its rows and its body by design.
    let modal = Rules {
        panel: Some(crate::ui_kit::layout::SAFE),
        ..Rules::default()
    };
    let r = check(&lobby_f(true, 114), &SyntheticSource, &modal);
    assert!(of_kind(&r.issues, "panel_shared").is_empty());
}

/// A value picker over a lobby's map picture: its panel, and with `text`
/// its title at `[0, -100, -20, -60]`, which lies over the picture.
fn picker_over_lobby(screen: &str, with_text: bool) -> View {
    let under = View {
        backing: Backing::GameShell,
        ..view(vec![
            art(
                "panel.2",
                [32, -152, -175, 68],
                Layer::Panel,
                Art::Level {
                    stem: "lockout".into(),
                },
            ),
            text("game", [135, -258, 61, 45], "Forge"),
            text("row.0.label", [0, -200, -20, -100], "Row"),
        ])
    };
    View {
        screen: screen.into(),
        underlay: Some(Box::new(under)),
        ..view({
            let mut w = vec![art("panel.0", [104, -268, -152, -12], Layer::Panel, screen_art(2))];
            if with_text {
                w.push(text("title", [0, -100, -20, -60], "Pick"));
            }
            w
        })
    }
}

#[test]
fn a_pickers_text_over_the_screens_picture_is_found_unless_allowed() {
    // A panel over the picture hides it, as a modal does: no finding.
    let hides = check(&picker_over_lobby("t", false), &SyntheticSource, &Rules::default());
    assert!(of_kind(&hides.issues, "covers_under").is_empty(), "{:?}", hides.issues);
    // The picker's title drawn over the picture is found; the rows and
    // the game's line beneath are the list's and clear of it.
    let r = check(&picker_over_lobby("t", true), &SyntheticSource, &Rules::default());
    let found = of_kind(&r.issues, "covers_under");
    assert!(
        matches!(found[..], [Issue::CoversUnder { under, pixels }] if under == "panel.2" && *pixels > 24),
        "{:?}",
        r.issues
    );
    let allowed = check(
        &picker_over_lobby(crate::theater_ui::PAUSE_MENU, true),
        &SyntheticSource,
        &Rules::default(),
    );
    assert!(of_kind(&allowed.issues, "covers_under").is_empty());
}

/// A source whose screen-14 art draws 60 by 20 whatever its declared
/// size, and whose bars draw as the synthetic boxes: art drawing past its
/// rectangle, as a wide row skin or a frame does.
struct Wide;
impl Metrics for Wide {
    fn width(&self, f: i16, t: &str) -> i32 {
        Synthetic.width(f, t)
    }
    fn line_height(&self, f: i16) -> i32 {
        Synthetic.line_height(f)
    }
}
impl Source for Wide {
    fn art(&self, a: &Art) -> Option<Image> {
        matches!(a, Art::Skin { .. }).then(|| Image {
            width: 60,
            height: 20,
            rgba: [200u8; 4].repeat(60 * 20),
        })
    }
    fn backing(&self, _c: &mut Canvas, _b: Backing) {}
    fn backdrop(&self, _c: &mut Canvas) {}
    #[allow(clippy::too_many_arguments)]
    fn text(&self, c: &mut Canvas, font: i16, t: &str, r: Rect, j: Justify, rgb: [u8; 3], alpha: u8) {
        SyntheticSource.text(c, font, t, r, j, rgb, alpha);
    }
    fn list_of(&self, _: i16, _: u16) -> Option<Option<ListAt>> {
        Some(None)
    }
}

#[test]
fn text_on_art_drawn_past_its_rectangle_is_found() {
    // The bar is declared 20 wide at x -100 and draws 60; the help line
    // starts at -70, clear of the declared rect, on the drawn bar.
    let v = view(vec![
        art("row.0.art.0", [20, -100, 0, -80], Layer::Under, bar()),
        text("description", [18, -70, 2, 0], "Help"),
    ]);
    let r = check(&v, &Wide, &Rules::default());
    assert!(of_kind(&r.issues, "on_other_art").is_empty(), "{:?}", r.issues);
    // Four glyph blocks of 6 by 10.
    assert!(
        r.issues.contains(&Issue::OnDrawn {
            text: "description".into(),
            on: "row.0.art.0".into(),
            pixels: 240,
        }),
        "{:?}",
        r.issues
    );
    assert!(r.issues.contains(&Issue::DrawnOutside {
        id: "row.0.art.0".into(),
        by: 40,
    }));
    // A row's own text on its bar belongs there.
    let own = view(vec![
        art("row.0.art.0", [20, -100, 0, -80], Layer::Under, bar()),
        text("row.0.label", [18, -70, 2, 0], "Help"),
    ]);
    let r = check(&own, &Wide, &Rules::default());
    assert!(of_kind(&r.issues, "on_drawn").is_empty(), "{:?}", r.issues);
}

#[test]
fn a_cards_texts_share_its_panel_with_the_list_row_that_is_its_name() {
    let mut v = lobby_f(false, 114);
    v.widgets.push(text("card.0.stats", [135, -40, 61, 105], "3 rounds"));
    v.widgets.push(text("cards.count", [135, -40, 61, 105], "1 of 3"));
    let r = check(&v, &SyntheticSource, &Rules::default());
    assert!(of_kind(&r.issues, "panel_shared").is_empty(), "{:?}", r.issues);
}

#[test]
fn groups_come_from_ids() {
    assert_eq!(group("row.3.label"), "list");
    assert_eq!(group("cell.2.art"), "list");
    assert_eq!(group("button.start"), "button");
    assert_eq!(group("player.2"), "player");
    assert_eq!(group("game"), "game");
}

#[test]
fn each_shared_panel_allowance_names_a_screen_and_its_precedent() {
    for (screen, why) in SHARED_PANELS {
        assert!(crate::ui_kit::screens::NAMES.contains(screen), "{screen}");
        assert!(why.contains("retail"), "{screen}: {why}");
    }
}

#[test]
fn each_cover_allowance_names_a_screen_and_its_precedent() {
    for (screen, why) in COVERS {
        assert!(crate::ui_kit::screens::NAMES.contains(screen), "{screen}");
        assert!(why.contains("retail"), "{screen}: {why}");
    }
}
