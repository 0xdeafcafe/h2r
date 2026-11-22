//! Three concepts of the whole pregame lobby with its mode control, each
//! a specimen page (`design.lobby.d`, `.e`, `.f`; `h2 ui preview
//! design.lobby.d --state all`), built from the kit's components over the
//! game shell: the header in the header layout's place, the lobby's two
//! backings and its picture, its buttons, the party, the footer legend.
//!
//! **What is the lobby tag's** (`pregame_lobby`, screen 14, pane 0, as
//! `crate::mods_screen::page_layout` holds it to the disc): `name_field`
//! under the buttons and `lobby_display` under the status, at their own
//! points; the picture's point and size; the status text's rectangle; the
//! first button's rectangle; the player table's rows, 22 apart; the
//! player count's rectangle; the texts' font and colours; the buttons'
//! words and the count's (`start_game`, `game_setup`, `player_count2`).
//! Rectangles the tag runs past the title-safe area (the first button's
//! left, -274; the table's and the count's right, 276 and 280) are held
//! to its edge.
//!
//! **What is not read from the tag**, and so is placed here: the second
//! button's rectangle (beside the first, on `name_field`), how a button
//! with the focus differs from one without (white against the header's
//! blue), and the player rows' cell art (`player_skin_default`), which
//! the pages leave out.
//!
//! - **D, a mode button and a picker** ([`d`]): the header names the
//!   lobby's mode (FORGE LOBBY); a third button, MODE: FORGE, sits beside
//!   Start Game and Game Setup. A on it opens the kit's value picker on
//!   the modes; the bumpers step the mode from anywhere in the lobby.
//! - **E, the header and position marks** ([`e`]): the header is the mode;
//!   beside it on the band a mark for each mode, the current one lit,
//!   with the bumpers at the marks' ends. The buttons are the lobby's
//!   two.
//! - **F, a list of modes** ([`f`]): the modes are a list on the lobby's
//!   panel, on the `default` list skin the main menu's rows are on; the
//!   bumpers move its lit row, and with the list focused so does the
//!   stick.
//!
//! Every text takes its font, colour and justification from a component
//! of `assets/ui/design.yaml`.

use super::designs::Lobby;
use super::{Avail, Dir, Tab, glyph_on};
use crate::list_class::{ListLayout, PictureSpec};
use crate::mods_screen::page_layout as retail;
use crate::ui_kit::components::{Screen, legend, row};
use crate::ui_kit::input::{Glyph, Prompt};
use crate::ui_kit::spec::{PaneText, ScreenSpec, ValueStyle};
use crate::ui_kit::view::{Art, Justify, Opener, PaneArt};

/// The three pages, as `ui_kit::screens::NAMES` lists them.
pub const PAGES: [&str; 3] = ["design.lobby.d", "design.lobby.e", "design.lobby.f"];

/// The title-safe area's left and right edges (`ui_kit::layout::SAFE`).
const SAFE_LEFT: i16 = -272;
const SAFE_RIGHT: i16 = 272;

/// The lobby's first button (`[173 -274 153 -118]` in the tag), its left
/// at the title-safe edge.
pub const START: [i16; 4] = [173, SAFE_LEFT, 153, -118];
/// The second button, beside the first on `name_field` (338 wide from
/// -279): placed here, not read from the tag.
pub const SETUP: [i16; 4] = [173, -112, 153, 44];
/// The lobby's status text (`retail::BODY`'s rectangle before the mods
/// page shortened it).
pub const STATUS: [i16; 4] = [135, -258, 61, 45];
/// The player count (`[199 85 179 280]` in the tag), its right at the
/// title-safe edge.
pub const COUNT: [i16; 4] = [199, 85, 179, SAFE_RIGHT];
/// The lobby's picture at its own point (`unknown_map`, 220 by 207).
pub const PICTURE_AT: [i16; 2] = [-152, 32];
/// The player table's left (`[177 118 157 276]` is its first row).
pub const PLAYERS_LEFT: i16 = 118;

/// Start Game's and Game Setup's words (`start_game`, `game_setup`).
pub const START_TEXT: &str = "START GAME";
pub const SETUP_TEXT: &str = "GAME SETUP";

/// What the lobby's buttons and its mode control have the focus on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Start,
    Setup,
    /// D's mode button.
    Mode,
    /// F's list of modes.
    List,
}

/// A text of no row in component `component`'s font, colour and
/// justification (`assets/ui/design.yaml`).
fn text(id: &str, component: &str, rect: [i16; 4], s: &str) -> PaneText {
    let d = crate::ui_kit::data::design();
    let (font, colour) = d.text_style(component).unwrap_or_else(|e| panic!("{e}"));
    let right = d
        .component(component)
        .unwrap_or_else(|e| panic!("{e}"))
        .justify()
        == Justify::Right;
    PaneText {
        id: id.into(),
        rect,
        font,
        colour,
        right,
        text: s.into(),
    }
}

/// A lobby button: white with the focus, the header's blue without.
fn button(id: &str, rect: [i16; 4], words: &str, focused: bool) -> PaneText {
    let component = if focused { "lobby.button" } else { "lobby.button.off" };
    text(id, component, rect, words)
}

/// A lobby bitmap as the kit's art.
fn lobby_art(p: PictureSpec, size: [u16; 2]) -> PaneArt {
    PaneArt {
        art: Art::Screen {
            screen_id: p.screen_id as i16,
            pane: p.pane as u16,
            index: p.bitmap as u16,
            image: p.image,
        },
        at: p.at,
        size,
        depth: 0,
    }
}

/// The map whose picture a specimen's lobby shows: a multiplayer map, and
/// the campaign's first level for CO-OP (stems of the UI level catalogue,
/// `Art::Level`).
pub const fn picture(tab: Tab) -> &'static str {
    match tab {
        Tab::CoOp => "01b_spacestation",
        _ => "lockout",
    }
}

/// What a specimen's lobby says on its panel, in the status text's
/// place: the game and where. CO-OP's lines are the co-op pane's
/// (`level_format`, `difficulty_format`), MULTIPLAYER's the matchmaking
/// pane's (`optimatch`, `in_hopper`).
pub const fn game(tab: Tab) -> &'static str {
    match tab {
        Tab::Multiplayer => "Matchmaking\nIn Rumble Pit",
        Tab::CustomGames => "Slayer\nLockout",
        Tab::Forge => "Forge\nLockout",
        Tab::Firefight => "Firefight\nLockout",
        Tab::CoOp => "Cairo Station\non Heroic",
    }
}

/// A mode's line in the picker's help text.
pub const fn about(tab: Tab) -> &'static str {
    match tab {
        Tab::Multiplayer => "Play a matchmaking playlist on Xbox Live.",
        Tab::CustomGames => "Play your own game types on any map.",
        Tab::Forge => "Edit a map with your party.",
        Tab::Firefight => "Hold out together against waves of Covenant.",
        Tab::CoOp => "Play the campaign together.",
    }
}

/// How many are in a specimen's party: two in CO-OP, as retail's co-op
/// lobby takes, else four.
pub const fn players(tab: Tab) -> usize {
    match tab {
        Tab::CoOp => 2,
        _ => 4,
    }
}

/// Player `k`'s name in a specimen.
fn player(k: usize) -> String {
    format!("Player {}", k + 1)
}

/// Player row `k`'s rectangle: the lobby table's, its right at the
/// title-safe edge.
pub const fn player_rect(k: usize) -> [i16; 4] {
    let top = retail::TABLE_TOP - retail::ROW_STEP * k as i16;
    [top, PLAYERS_LEFT, top - retail::ROW_HEIGHT, SAFE_RIGHT]
}

/// The bumpers' prompt in the footer: White and Black step the mode.
fn mode_prompt() -> Prompt {
    Prompt::new(&[Glyph::White, Glyph::Black], "Mode")
}

/// The lobby every concept shares, on list place `list`: the title, the
/// two backings and the picture, the game's lines at `status`, the count
/// and the party. A leader gets `buttons` (the one `focus` names white); a
/// member the party leader's line in their place, as `party_leader` reads.
fn lobby(
    name: &str,
    title: &str,
    list: ListLayout,
    l: &Lobby,
    status: [i16; 4],
    buttons: &[(&str, [i16; 4], String, Focus)],
    focus: Focus,
) -> Screen {
    let n = players(l.selected);
    let mut s = Screen::moved(name, title, list, ValueStyle::Inline);
    if l.leader {
        for (id, rect, words, is) in buttons {
            s = s.text(button(id, *rect, words, *is == focus));
        }
    } else {
        let line = format!("{} is the party leader", player(0));
        s = s.text(text(
            "leader",
            "text.label",
            [START[0], START[1], START[2], SETUP[3]],
            &line,
        ));
    }
    s = s
        .text(text("game", "text.label", status, game(l.selected)))
        .text(text(
            "count",
            "tab.count",
            COUNT,
            &format!("{n} player(s) in party"),
        ));
    for k in 0..n {
        s = s.text(text(
            &format!("player.{k}"),
            "text.emphasis",
            player_rect(k),
            &player(k),
        ));
    }
    s
}

/// The lobby's backings and its picture: drawn first, under everything.
fn panel(tab: Tab) -> Vec<PaneArt> {
    vec![
        lobby_art(retail::NAME_FIELD, retail::NAME_FIELD_SIZE),
        lobby_art(retail::LOBBY_DISPLAY, retail::LOBBY_DISPLAY_SIZE),
        PaneArt {
            art: Art::Level {
                stem: picture(tab).into(),
            },
            at: PICTURE_AT,
            size: retail::PICTURE_SIZE,
            depth: 0,
        },
    ]
}

/// The legend: A and B for a leader, B alone for a member (retail's
/// `button_key_squad_leader_teams_disabled`, `button_key_peon_teams_disabled`),
/// and `more` after them.
fn prompts(l: &Lobby, more: Option<Prompt>) -> Vec<Prompt> {
    let mut out = if l.leader {
        vec![legend::select(), legend::back()]
    } else {
        vec![legend::back()]
    };
    out.extend(more.filter(|_| l.leader));
    out
}

fn finish(s: Screen, l: &Lobby, more: Option<Prompt>) -> ScreenSpec {
    let mut spec = s.legend(prompts(l, more)).build();
    spec.panel = panel(l.selected);
    spec
}

// ---- D: a mode button and a picker ----

/// Concept D: the mode is a third lobby button, and A on it opens a
/// picker.
pub mod d {
    use super::*;

    /// The three buttons across `name_field`: Start Game and Game Setup
    /// in narrower cells than the lobby's two, the mode button after them.
    pub const START: [i16; 4] = [173, SAFE_LEFT, 153, -188];
    pub const SETUP: [i16; 4] = [173, -182, 153, -98];
    pub const MODE: [i16; 4] = [173, -92, 153, 56];

    /// The header: the mode's name, then LOBBY.
    pub fn title(tab: Tab) -> String {
        format!("{} LOBBY", tab.label())
    }

    /// The mode button's words.
    pub fn mode_text(tab: Tab) -> String {
        format!("MODE: {}", tab.label())
    }

    /// The picker's title.
    pub const PICKER_TITLE: &str = "MODE";

    /// The lobby with `focus` on one of its three buttons.
    pub fn spec(name: &str, l: &Lobby, focus: Focus) -> ScreenSpec {
        let buttons = [
            ("button.start", START, START_TEXT.to_string(), Focus::Start),
            ("button.setup", SETUP, SETUP_TEXT.to_string(), Focus::Setup),
            ("button.mode", MODE, mode_text(l.selected), Focus::Mode),
        ];
        let s = lobby(name, &title(l.selected), retail::LIST, l, STATUS, &buttons, focus);
        finish(s, l, Some(mode_prompt()))
    }

    /// The mode picker open over the lobby, on `on`: the kit's value
    /// picker, a row for each mode present. Its help line is the focused
    /// mode's ([`about`]), or why it can't run in this lobby
    /// (`Reason::text`).
    pub fn picker(name: &str, l: &Lobby, on: Tab) -> ScreenSpec {
        let modes = l.row();
        let labels: Vec<&str> = modes.iter().map(|(t, _)| t.label()).collect();
        let at = modes.iter().position(|(t, _)| *t == on).unwrap_or(0);
        let under = spec(name, l, Focus::Mode);
        let mut s = Screen::picker(name, PICKER_TITLE, &labels, at, "", Opener::MAIN_MENU)
            .over(under)
            .build();
        for (r, (tab, avail)) in s.rows.iter_mut().zip(&modes) {
            r.description = Some(help(*tab, *avail).into());
        }
        if let Some(dialog) = &mut s.dialog {
            dialog.rows.clone_from(&s.rows);
        }
        s
    }

    /// A mode's help line in the picker.
    pub fn help(tab: Tab, avail: Avail) -> &'static str {
        match avail.reason() {
            Some(reason) => reason.text(),
            None => about(tab),
        }
    }

    pub fn states(name: &str) -> Vec<(String, ScreenSpec)> {
        let mut out: Vec<(String, ScreenSpec)> = Tab::ALL
            .iter()
            .map(|t| (state_name(*t), spec(name, &Lobby::live(*t), Focus::Start)))
            .collect();
        let forge = Lobby::live(Tab::Forge);
        out.push(("mode-focused".into(), spec(name, &forge, Focus::Mode)));
        out.push(("picker".into(), picker(name, &forge, Tab::Forge)));
        out.push((
            "picker-unavailable".into(),
            picker(name, &split_screen(), Tab::Multiplayer),
        ));
        out
    }
}

// ---- E: the header and position marks ----

/// Concept E: the mode is the header, with a mark for each mode beside
/// it and the bumpers at the marks' ends.
pub mod e {
    use super::*;

    /// A mark's side, and the step from one mark to the next.
    pub const MARK: u16 = 6;
    pub const PITCH: i16 = 12;
    /// The marks' top: the middle of the band's line.
    pub const MARK_TOP: i16 = 193;
    /// The middle of the run of marks.
    pub const MIDDLE: i16 = -2;
    /// The bumpers' cells at the run's ends, inside the band's cut corner.
    pub const PREV: [i16; 4] = [199, -62, 179, -38];
    pub const NEXT: [i16; 4] = [199, 34, 179, 58];
    /// An unavailable mode's mark is drawn this faint.
    pub const OFF_ALPHA: u8 = 110;

    /// Each mode's mark: its top-left, in the row's order, centred on
    /// [`MIDDLE`].
    pub fn marks(tabs: &[Tab]) -> Vec<(Tab, [i16; 2])> {
        let n = tabs.len() as i16;
        let width = PITCH * n - (PITCH - MARK as i16);
        let left = MIDDLE - width / 2;
        tabs.iter()
            .enumerate()
            .map(|(i, t)| (*t, [left + PITCH * i as i16, MARK_TOP]))
            .collect()
    }

    /// A mark's colour: white for the lobby's mode, the lobby's value
    /// blue for another, faint for one that can't run here.
    pub fn mark_rgba(tab: Tab, selected: Tab, avail: Avail) -> [u8; 4] {
        let d = crate::ui_kit::data::design();
        let colour = |name: &str| {
            crate::ui_kit::components::rgb(d.colour(name).unwrap_or_else(|e| panic!("{e}")))
        };
        if tab == selected {
            let [r, g, b] = colour("white");
            [r, g, b, 255]
        } else {
            let [r, g, b] = colour("pale_blue");
            [r, g, b, if avail.is_on() { 255 } else { OFF_ALPHA }]
        }
    }

    pub fn spec(name: &str, l: &Lobby) -> ScreenSpec {
        let buttons = [
            ("button.start", START, START_TEXT.to_string(), Focus::Start),
            ("button.setup", SETUP, SETUP_TEXT.to_string(), Focus::Setup),
        ];
        let modes = l.row();
        let mut s = lobby(
            name,
            l.selected.label(),
            retail::LIST,
            l,
            STATUS,
            &buttons,
            Focus::Start,
        );
        let tabs: Vec<Tab> = modes.iter().map(|(t, _)| *t).collect();
        for ((tab, at), (_, avail)) in marks(&tabs).into_iter().zip(&modes) {
            s = s.art(PaneArt {
                art: Art::Solid {
                    rgba: mark_rgba(tab, l.selected, *avail),
                },
                at,
                size: [MARK, MARK],
                depth: 0,
            });
        }
        if l.leader {
            for (id, rect, dir) in [
                ("bumper.prev", PREV, Dir::Prev),
                ("bumper.next", NEXT, Dir::Next),
            ] {
                let on = glyph_on(&modes, l.selected, dir);
                let component = if on { "lobby.bumper" } else { "lobby.bumper.off" };
                s = s.text(text(id, component, rect, &dir.glyph().char().to_string()));
            }
        }
        finish(s, l, None)
    }

    pub fn states(name: &str) -> Vec<(String, ScreenSpec)> {
        let mut out: Vec<(String, ScreenSpec)> = Tab::ALL
            .iter()
            .map(|t| (state_name(*t), spec(name, &Lobby::live(*t))))
            .collect();
        out.push((
            "member".into(),
            spec(
                name,
                &Lobby {
                    leader: false,
                    ..Lobby::live(Tab::Forge)
                },
            ),
        ));
        out
    }
}

// ---- F: a list of modes ----

/// Concept F: the modes are a list on the lobby's panel.
pub mod f {
    use super::*;

    /// The lobby's header, as retail's reads.
    pub const TITLE: &str = "PREGAME LOBBY";

    /// The list: the `default` skin (0), five rows on `lobby_display`,
    /// from under `name_field` to the picture's top. Its left is the mods
    /// page's action's (`retail::LIST`), so the frame cap stays inside the
    /// title-safe area.
    pub const LIST: ListLayout = ListLayout {
        skin: 0,
        anchor: [retail::LIST.anchor[0], 114],
        bitmaps_left_of: i16::MIN,
        visible: 5,
    };
    /// The game's lines, right of the list (the status text's rows).
    pub const STATUS: [i16; 4] = [135, -40, 61, 105];

    /// The lobby with its list on the lobby's mode; `focus` is the list
    /// ([`Focus::List`]) or a button.
    pub fn spec(name: &str, l: &Lobby, focus: Focus) -> ScreenSpec {
        let buttons = [
            ("button.start", START, START_TEXT.to_string(), Focus::Start),
            ("button.setup", SETUP, SETUP_TEXT.to_string(), Focus::Setup),
        ];
        let modes = l.row();
        let mut s = lobby(name, TITLE, LIST, l, STATUS, &buttons, focus);
        for (tab, _) in &modes {
            s = s.row(row::action(tab.label()));
        }
        let mut spec = finish(s, l, Some(mode_prompt()));
        spec.focus = modes
            .iter()
            .position(|(t, _)| *t == l.selected)
            .unwrap_or(0);
        spec
    }

    pub fn states(name: &str) -> Vec<(String, ScreenSpec)> {
        let mut out: Vec<(String, ScreenSpec)> = Tab::ALL
            .iter()
            .map(|t| (state_name(*t), spec(name, &Lobby::live(*t), Focus::List)))
            .collect();
        out.push((
            "buttons".into(),
            spec(name, &Lobby::live(Tab::Forge), Focus::Start),
        ));
        out
    }
}

// ---- the pages ----

/// A mode's state name (`custom-games`).
fn state_name(tab: Tab) -> String {
    tab.name().replace('_', "-")
}

/// A Split Screen lobby on CUSTOM GAMES: MULTIPLAYER can't run in it.
fn split_screen() -> Lobby {
    Lobby {
        place: super::Place::SplitScreen,
        ..Lobby::live(Tab::CustomGames)
    }
}

/// Page `name`'s named states; `None` for a name that is no concept.
pub fn states(name: &str) -> Option<Vec<(String, ScreenSpec)>> {
    Some(match name {
        "design.lobby.d" => d::states(name),
        "design.lobby.e" => e::states(name),
        "design.lobby.f" => f::states(name),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::components::Kind;
    use crate::ui_kit::layout::{self, Rules};
    use crate::ui_kit::preview::From;
    use crate::ui_kit::view::{self, View, Widget};

    fn drawn(spec: &ScreenSpec, state: &str) -> View {
        From::Synthetic.view(spec, state).unwrap()
    }

    fn white() -> [u8; 3] {
        crate::ui_kit::components::rgb(crate::ui_kit::data::design().colour("white").unwrap())
    }

    fn colour(w: &Widget) -> [u8; 3] {
        match &w.kind {
            view::Kind::Text { colour, .. } => *colour,
            view::Kind::Art { .. } => panic!("{} is art", w.id),
        }
    }

    /// Every state of every concept passes the layout checks that need no
    /// art, as each page of `NAMES` must.
    #[test]
    fn every_state_passes_the_layout_checks() {
        let from = From::Synthetic;
        for name in PAGES {
            let all = states(name).unwrap();
            assert!(!all.is_empty(), "{name}");
            for (state, spec) in all {
                let v = drawn(&spec, &state);
                let r = layout::check(&v, from.source(), &Rules::for_spec(&spec));
                assert!(r.issues.is_empty(), "{name}/{state}: {:?}", r.issues);
            }
        }
        assert!(states("design.lobby.g").is_none());
    }

    /// Each page shows each mode in turn first, then its own states.
    #[test]
    fn each_page_has_every_mode_and_its_own_states() {
        let modes = ["multiplayer", "custom-games", "forge", "firefight", "co-op"];
        for (name, own) in [
            ("design.lobby.d", &["mode-focused", "picker", "picker-unavailable"][..]),
            ("design.lobby.e", &["member"][..]),
            ("design.lobby.f", &["buttons"][..]),
        ] {
            let states = states(name).unwrap();
            let names: Vec<&str> = states.iter().map(|(n, _)| n.as_str()).collect();
            let want: Vec<&str> = modes.iter().chain(own).copied().collect();
            assert_eq!(names, want, "{name}");
        }
    }

    /// The whole lobby is on every page: the header, both backings and
    /// the picture, the buttons, the game's lines, the count, the party
    /// and the legend.
    #[test]
    fn every_lobby_state_is_the_whole_lobby() {
        for name in PAGES {
            for (state, spec) in states(name).unwrap() {
                // A picker's state is checked through the lobby under it.
                let lobby = spec.underlay.as_deref().unwrap_or(&spec);
                assert_eq!(Kind::of(lobby), Kind::List, "{name}/{state}");
                let v = drawn(lobby, &state);
                assert_eq!(v.backing, view::Backing::GameShell, "{name}/{state}");
                let mut ids = vec!["title", "panel.0", "panel.1", "panel.2", "game", "count", "legend"];
                if v.widget("leader").is_none() {
                    ids.extend(["button.start", "button.setup"]);
                }
                for id in ids {
                    assert!(v.widget(id).is_some(), "{name}/{state}: no {id}");
                }
                let lines = v.widget("game").unwrap().text();
                let tab = Tab::ALL.into_iter().find(|t| lines == Some(game(*t))).unwrap();
                for k in 0..players(tab) {
                    let w = v.widget(&format!("player.{k}")).unwrap();
                    assert_eq!(w.rect, player_rect(k), "{name}/{state}");
                }
                assert!(v.widget(&format!("player.{}", players(tab))).is_none());
                // The backings are the lobby tag's own bitmaps at its points.
                let p = v.widget("panel.0").unwrap();
                assert_eq!([p.rect[1], p.rect[0]], retail::NAME_FIELD.at, "{name}/{state}");
                let p = v.widget("panel.1").unwrap();
                assert_eq!([p.rect[1], p.rect[0]], retail::LOBBY_DISPLAY.at, "{name}/{state}");
                assert!(v.widgets.iter().filter(|w| w.id.starts_with("panel.")).all(|w| w.layer == view::Layer::Panel));
            }
        }
    }

    /// One button at most is white, and it is the one with the focus.
    #[test]
    fn the_focused_button_alone_is_white() {
        let lit = |spec: &ScreenSpec| -> Vec<String> {
            drawn(spec, "s")
                .widgets
                .iter()
                .filter(|w| w.id.starts_with("button.") && colour(w) == white())
                .map(|w| w.id.clone())
                .collect()
        };
        let forge = Lobby::live(Tab::Forge);
        assert_eq!(lit(&d::spec("design.lobby.d", &forge, Focus::Start)), ["button.start"]);
        assert_eq!(lit(&d::spec("design.lobby.d", &forge, Focus::Setup)), ["button.setup"]);
        assert_eq!(lit(&d::spec("design.lobby.d", &forge, Focus::Mode)), ["button.mode"]);
        assert_eq!(lit(&e::spec("design.lobby.e", &forge)), ["button.start"]);
        assert_eq!(lit(&f::spec("design.lobby.f", &forge, Focus::Start)), ["button.start"]);
        assert!(lit(&f::spec("design.lobby.f", &forge, Focus::List)).is_empty());
    }

    /// The legends: glyph codes for the bumpers are White then Black
    /// (U+E105, U+E104), and a member has B alone.
    #[test]
    fn the_legends_name_the_bumpers_where_they_step_the_mode() {
        let forge = Lobby::live(Tab::Forge);
        let member = Lobby {
            leader: false,
            ..forge
        };
        let bumpers = "\u{e105}  \u{e104} MODE";
        let legend_of = |spec: &ScreenSpec| spec.footer_text().unwrap();
        for spec in [
            d::spec("design.lobby.d", &forge, Focus::Start),
            f::spec("design.lobby.f", &forge, Focus::List),
        ] {
            let legend = legend_of(&spec);
            assert_eq!(legend, format!("\u{e100} SELECT  \u{e101} BACK  {bumpers}"));
            crate::ui_kit::components::legend::well_formed(&legend, 2, 2).unwrap();
        }
        assert_eq!(
            legend_of(&e::spec("design.lobby.e", &forge)),
            "\u{e100} SELECT  \u{e101} BACK"
        );
        for spec in [
            d::spec("design.lobby.d", &member, Focus::Start),
            e::spec("design.lobby.e", &member),
            f::spec("design.lobby.f", &member, Focus::List),
        ] {
            assert_eq!(legend_of(&spec), "\u{e101} BACK");
            let v = drawn(&spec, "member");
            assert!(v.widget("button.start").is_none() && v.widget("leader").is_some());
            assert!(v.widget("bumper.prev").is_none() && v.widget("bumper.next").is_none());
        }
    }

    #[test]
    fn d_names_the_mode_in_its_header_and_on_its_button() {
        for tab in Tab::ALL {
            let spec = d::spec("design.lobby.d", &Lobby::live(tab), Focus::Start);
            assert_eq!(spec.title, format!("{} LOBBY", tab.label()));
            let v = drawn(&spec, "s");
            let mode = v.widget("button.mode").unwrap();
            assert_eq!(mode.text(), Some(format!("MODE: {}", tab.label()).as_str()));
            assert!(!crate::ui_kit::preview::overflows(mode, &view::Synthetic), "{tab:?}");
            // The three buttons stand side by side on name_field.
            let (start, setup) = (v.widget("button.start").unwrap(), v.widget("button.setup").unwrap());
            assert!(start.rect[3] <= setup.rect[1] && setup.rect[3] <= mode.rect[1]);
            let field_right = retail::NAME_FIELD.at[0] + retail::NAME_FIELD_SIZE[0] as i16;
            assert!(mode.rect[3] <= field_right, "{:?}", mode.rect);
        }
    }

    /// D's picker is the kit's value picker over the lobby: a row for each
    /// mode, the lobby's mode focused, and a mode that can't run saying
    /// why in the help line.
    #[test]
    fn d_opens_the_value_picker_on_the_modes() {
        let forge = Lobby::live(Tab::Forge);
        let p = d::picker("design.lobby.d", &forge, Tab::Forge);
        assert_eq!(Kind::of(&p), Kind::Dialog(crate::ui_kit::dialog::DialogBase::ValuePicker));
        assert_eq!(p.title, d::PICKER_TITLE);
        let labels: Vec<&str> = p.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, Tab::ALL.map(Tab::label));
        assert_eq!(p.focus, 2);
        assert_eq!(p.dialog.as_ref().unwrap().rows, p.rows);
        assert_eq!(p.description_text().as_deref(), Some(about(Tab::Forge)));
        let under = p.underlay.as_deref().unwrap();
        assert_eq!(*under, d::spec("design.lobby.d", &forge, Focus::Mode));
        // Split Screen: Matchmaking can't run, and its row says so.
        let split = split_screen();
        let p = d::picker("design.lobby.d", &split, Tab::Multiplayer);
        assert_eq!(p.focus, 0);
        let reason = super::super::available(Tab::Multiplayer, split.place, split.facts)
            .reason()
            .unwrap();
        assert_eq!(p.description_text().as_deref(), Some(reason.text()));
        for (r, (tab, avail)) in p.rows.iter().zip(split.row()) {
            assert_eq!(r.description.as_deref(), Some(d::help(tab, avail)), "{tab:?}");
            assert!(!d::help(tab, avail).is_empty());
        }
    }

    /// E: one mark a mode in the row's order, the lobby's alone white,
    /// between the bumpers, on the band.
    #[test]
    fn e_marks_each_mode_and_lights_the_lobbys() {
        let design = crate::ui_kit::data::design();
        let w = white();
        for tab in Tab::ALL {
            let l = Lobby::live(tab);
            let spec = e::spec("design.lobby.e", &l);
            assert_eq!(spec.title, tab.label());
            let solid: Vec<(&PaneArt, [u8; 4])> = spec
                .art
                .iter()
                .filter_map(|a| match a.art {
                    Art::Solid { rgba } => Some((a, rgba)),
                    _ => None,
                })
                .collect();
            assert_eq!(solid.len(), Tab::ALL.len());
            let lit: Vec<usize> = solid
                .iter()
                .enumerate()
                .filter(|(_, (_, c))| c[..3] == w)
                .map(|(i, _)| i)
                .collect();
            assert_eq!(lit, [tab as usize], "{tab:?}");
            for pair in solid.windows(2) {
                assert_eq!(pair[1].0.at[0] - pair[0].0.at[0], e::PITCH);
            }
            let (first, last) = (solid[0].0, solid[solid.len() - 1].0);
            assert!(first.at[0] >= e::PREV[3] && last.at[0] + e::MARK as i16 <= e::NEXT[1]);
            assert!(e::NEXT[3] <= super::super::designs::BAND_RIGHT);
            assert!(first.at[1] <= super::super::designs::HEADER_TOP);
            assert!(first.at[1] - e::MARK as i16 >= super::super::designs::HEADER_BOTTOM);
            let v = drawn(&spec, "s");
            assert_eq!(v.widget("bumper.prev").unwrap().text(), Some("\u{e105}"));
            assert_eq!(v.widget("bumper.next").unwrap().text(), Some("\u{e104}"));
            // A bumper with nowhere to go is the value blue, not white.
            let pale = crate::ui_kit::components::rgb(design.colour("pale_blue").unwrap());
            let prev = colour(v.widget("bumper.prev").unwrap());
            assert_eq!(prev, if tab == Tab::Multiplayer { pale } else { w }, "{tab:?}");
        }
        // A mode that can't run here has a faint mark.
        let split = split_screen();
        let spec = e::spec("design.lobby.e", &split);
        let Art::Solid { rgba } = spec.art[0].art else {
            panic!("the first mark");
        };
        assert_eq!(rgba[3], e::OFF_ALPHA);
        // Fewer modes present: the run stays centred.
        let three = e::marks(&[Tab::Multiplayer, Tab::CustomGames, Tab::CoOp]);
        let five = e::marks(&Tab::ALL);
        let middle = |m: &[(Tab, [i16; 2])]| m[0].1[0] + m[m.len() - 1].1[0];
        assert_eq!(middle(&three), middle(&five));
    }

    /// F: the modes are rows of the `default` skin, the lobby's lit, on
    /// the lobby's panel between the buttons and the picture.
    #[test]
    fn f_lists_the_modes_with_the_lobbys_lit() {
        for tab in Tab::ALL {
            let spec = f::spec("design.lobby.f", &Lobby::live(tab), Focus::List);
            assert_eq!(spec.title, f::TITLE);
            let labels: Vec<&str> = spec.rows.iter().map(|r| r.label.as_str()).collect();
            assert_eq!(labels, Tab::ALL.map(Tab::label));
            assert_eq!(spec.focus, tab as usize);
            let v = drawn(&spec, "s");
            assert_eq!(v.focus.as_deref(), Some(format!("row.{}", tab as usize).as_str()));
            assert_eq!(v.list.unwrap().skin, 0);
            let display_bottom = retail::LOBBY_DISPLAY.at[1] - retail::LOBBY_DISPLAY_SIZE[1] as i16;
            for i in 0..Tab::ALL.len() {
                let label = v.widget(&format!("row.{i}.label")).unwrap();
                assert!(label.rect[0] < START[2], "row {i} is under the buttons");
                assert!(label.rect[2] >= display_bottom, "row {i} is on lobby_display");
                // The rows keep clear of the game's lines beside them.
                assert!(label.rect[3] <= f::STATUS[1], "{:?}", label.rect);
            }
        }
        // With the focus on a button, the list still shows the mode.
        let buttons = f::spec("design.lobby.f", &Lobby::live(Tab::Forge), Focus::Start);
        assert_eq!(buttons.focus, Tab::Forge as usize);
    }

    /// Every text of every state is in a palette colour and, off a row,
    /// in the body or title font: what the consistency drive holds each
    /// screen to.
    #[test]
    fn every_text_is_a_design_system_text() {
        let design = crate::ui_kit::data::design();
        let palette: Vec<[u8; 3]> = ["white", "hint_blue", "pale_blue", "item_blue"]
            .iter()
            .map(|n| crate::ui_kit::components::rgb(design.colour(n).unwrap()))
            .collect();
        for name in PAGES {
            for (state, spec) in states(name).unwrap() {
                let lobby = spec.underlay.as_deref().unwrap_or(&spec);
                for t in &lobby.texts {
                    let c = crate::ui_kit::components::rgb(t.colour);
                    assert!(palette.contains(&c), "{name}/{state}: {} is {c:?}", t.id);
                    assert_eq!(t.font, design.font("body").unwrap(), "{name}/{state}: {}", t.id);
                }
            }
        }
    }
}
