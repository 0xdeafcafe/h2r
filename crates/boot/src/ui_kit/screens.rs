//! Our existing screens, described for the kit: the skulls pages, the
//! co-op screens, the sign-in prompt and the account menu
//! ([`Describe`]), each with a sample state for the checks and previews
//! ([`sample`], `h2 ui preview <screen>`).
//!
//! The descriptions read the screens' own rows (their string ids resolved
//! through the tables that serve them), so a check here is a check of
//! what the game is handed.

use std::sync::Arc;

use crate::coop_live::{self, FriendRows, HostRows, JoinRows, Lobby};
use crate::list_class::Rows;
use crate::skulls::{self, SKULLS, State, TOTAL};
use crate::skulls_menu::{self, Mode, SkullsScreen};
use crate::ui_kit::input::{Glyph, Input, Prompt};
use crate::ui_kit::page::{AccountPage, Driven, ListPage, Live, described};
use crate::ui_kit::spec::{Describe, ListPlace, Press, RowSpec, ScreenSpec, ValueStyle};

/// The campaign page's legend: left/right change, A selects (Start Level
/// goes on), B backs out.
pub fn skulls_prompts() -> Vec<Prompt> {
    vec![
        Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
        Prompt::new(&[Glyph::A], "Select"),
        Prompt::new(&[Glyph::B], "Back"),
    ]
}

/// The skulls pages' own art: the level select's bracket for the list
/// ([`skulls_menu::FRAME`]) and the skull picture ([`skulls_menu::PICTURE`],
/// drawn untinted here).
pub fn skulls_art() -> Vec<crate::ui_kit::view::PaneArt> {
    use crate::list_class::PictureSpec;
    use crate::ui_kit::view::{Art, PaneArt};
    let art = |p: PictureSpec, size: [u16; 2]| PaneArt {
        art: Art::Screen {
            screen_id: p.screen_id as i16,
            pane: p.pane as u16,
            index: p.bitmap as u16,
            image: p.image,
        },
        at: p.at,
        size,
        depth: 0,
    };
    vec![
        art(skulls_menu::FRAME, skulls_menu::FRAME_SIZE),
        art(skulls_menu::PICTURE, [256, 256]),
    ]
}

/// A string the skulls pages serve.
fn skull_text(id: u32) -> Option<String> {
    skulls_menu::strings()
        .into_iter()
        .find(|(i, _)| *i == id)
        .map(|(_, t)| t.to_string())
}

impl Describe for SkullsScreen {
    fn spec(&self) -> ScreenSpec {
        let title = skull_text(self.title()).unwrap_or_else(|| "?".into());
        let campaign = self.mode() == Mode::Campaign;
        let mut s = ScreenSpec::new(
            if campaign {
                crate::skulls_screen::SCREEN
            } else {
                "skulls.mp"
            },
            &title,
        );
        s.ids = vec![self.title()];
        s.list = ListPlace::from(Some(skulls_menu::LIST_LAYOUT));
        s.style = ValueStyle::Inline;
        s.description = Some(skulls_menu::DESCRIPTION_TEXT.into());
        s.scrolls = true;
        s.art = skulls_art();
        if campaign {
            s.prompts = skulls_prompts();
            s.footer = true;
            s.ids.push(skulls_menu::FOOTER_ID);
            s.inputs = vec![
                Input::Up,
                Input::Down,
                Input::Left,
                Input::Right,
                Input::A,
                Input::B,
            ];
            let ids = |row: i16| -> Vec<u32> {
                let mut ids: Vec<u32> = self.texts(row).into_iter().map(|(_, id)| id).collect();
                ids.extend(Rows::description(self, row));
                ids
            };
            s.rows.push(
                RowSpec::action(skulls_menu::START)
                    .press(Press::Act)
                    .describe(skulls_menu::START_DESCRIPTION)
                    .ids(&ids(0)),
            );
            for (n, o) in self.options().iter().enumerate() {
                let row = 1 + n as i16;
                let halo1 = crate::h1_options::is_halo1(&self.store(), o);
                s.rows.push(
                    RowSpec::value(o.label, &["Halo 1", "Halo 2"], usize::from(!halo1))
                        .press(Press::Step)
                        .describe(&crate::h1_options::description(o))
                        .ids(&ids(row)),
                );
            }
            let mut order: Vec<(i16, usize)> = (0..TOTAL)
                .filter_map(|i| Some((self.row_of_skull(i)?, i)))
                .collect();
            order.sort();
            for (row, i) in order {
                let label = SKULLS[i].label;
                let r = match self.state(i) {
                    State::On => RowSpec::value(label, &["On", "Off"], 0),
                    State::Off => RowSpec::value(label, &["On", "Off"], 1),
                    State::Locked => RowSpec::info(label, skulls_menu::LOCKED),
                };
                s.rows.push(
                    r.press(Press::Step)
                        .describe(&skulls_menu::description(i))
                        .ids(&ids(row)),
                );
            }
        } else {
            s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
            for row in 0..self.count() as i16 {
                let ids: Vec<u32> = self
                    .texts(row)
                    .into_iter()
                    .map(|(_, id)| id)
                    .chain(Rows::description(self, row))
                    .collect();
                let label = ids
                    .first()
                    .and_then(|id| skull_text(*id))
                    .unwrap_or_else(|| "?".into());
                s.rows.push(
                    RowSpec::action(&label)
                        .press(Press::Step)
                        .describe(&skulls_menu::mp_description(row as usize))
                        .ids(&ids),
                );
            }
        }
        s
    }
}

/// The co-op screens' inputs: the list's, and the action on A, X, Start.
fn coop_inputs() -> Vec<Input> {
    vec![
        Input::Up,
        Input::Down,
        Input::A,
        Input::B,
        Input::X,
        Input::Start,
    ]
}

fn coop_spec(
    name: &str,
    title: String,
    rows: impl Iterator<Item = (String, String, Press)>,
) -> ScreenSpec {
    let mut s = ScreenSpec::new(name, &title);
    s.inputs = coop_inputs();
    s.prompts = coop_live::prompts(name);
    s.footer = true;
    for (label, value, press) in rows {
        let r = if value.is_empty() && press != Press::Nothing {
            RowSpec::action(&label)
        } else {
            RowSpec::info(&label, &value)
        };
        s.rows.push(r.press(press));
    }
    s
}

fn coop_title(rows: &dyn Rows, lobby: &Lobby) -> String {
    let id = rows.title();
    lobby
        .texts
        .get(id - coop_live::TEXT_ID_BASE)
        .unwrap_or_else(|| "?".into())
}

impl Describe for HostRows {
    fn spec(&self) -> ScreenSpec {
        use coop_live::{ROW_INVITE, ROW_START};
        let rows = (0..self.count() as i16).filter_map(|r| {
            let (l, v) = self.row_texts(r)?;
            let press = match r {
                ROW_INVITE => Press::Open(coop_live::FRIENDS.into()),
                ROW_START => Press::Act,
                _ => Press::Step,
            };
            Some((l, v, press))
        });
        let mut s = coop_spec(coop_live::SCREEN, coop_title(self, &self.0), rows);
        // Six rows in five shown: Start scrolls into view (seen in game,
        // 2026-11-16), and is meant to.
        s.scrolls = true;
        s
    }
}

impl Describe for FriendRows {
    fn spec(&self) -> ScreenSpec {
        let rows = (0..self.count() as i16).filter_map(|r| {
            let (l, v) = self.row_texts(r)?;
            Some((l, v, Press::Step))
        });
        let mut s = coop_spec(coop_live::FRIENDS, coop_title(self, &self.0), rows);
        s.scrolls = true;
        s
    }
}

impl Describe for JoinRows {
    fn spec(&self) -> ScreenSpec {
        let rows = (0..self.count() as i16).filter_map(|r| {
            let (l, v) = self.row_texts(r)?;
            let press = match r {
                coop_live::JOIN_READY => Press::Act,
                4 => Press::Step,
                _ => Press::Nothing,
            };
            Some((l, v, press))
        });
        let mut s = coop_spec(coop_live::JOIN, coop_title(self, &self.0), rows);
        // Ready, the sixth row, scrolls into view as the host's Start does.
        s.scrolls = true;
        s
    }
}

/// The account menu ([`crate::account_screen`]): drawn in the stock
/// dialog today, described as the list it would be on the kit.
pub fn account_spec(a: &crate::account_screen::AccountScreen, now: i64) -> ScreenSpec {
    use crate::account_screen::{MENU, Step};
    let mut s = ScreenSpec::new("account", a.title());
    s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
    s.description = Some(crate::signin_screen::TEXT_BLOCK.into());
    match &a.step {
        Step::Menu { focus } => {
            s.focus = *focus;
            s.rows = MENU.iter().map(|r| RowSpec::action(r)).collect();
            s.prompts = vec![
                Prompt::new(&[Glyph::A], "Select"),
                Prompt::new(&[Glyph::B], "Back"),
            ];
            // In the footer, not the description block: there it sat on
            // rows 1 and 2 (the first layout check's first finding).
            s.footer = true;
        }
        _ => {
            s.rows = vec![RowSpec::action(a.title()).describe(&a.text(now).replace("\r\n", "\n"))];
        }
    }
    s
}

/// The screens `h2 ui preview` knows, by name.
pub const NAMES: [&str; 10] = [
    "skulls",
    "skulls.h1",
    "skulls.mp",
    "h1-options",
    "coop",
    "coop.friends",
    "coop.join",
    "signin-code",
    "account",
    "kit.example",
];

/// Every named state of screen `name` for the preview sheet: a list's
/// first, middle and last row focused; the sign-in prompt's pages
/// ([`crate::ui_kit::page::Page::states`]).
pub fn states(name: &str) -> Option<Vec<(String, ScreenSpec)>> {
    Some(live(name)?.specs())
}

/// A sample of screen `name` in the state it opens in.
pub fn sample(name: &str) -> Option<ScreenSpec> {
    Some(live(name)?.spec())
}

/// Screen `name`'s page, live: the skulls pages for a profile with a few
/// skulls collected and one turned off, the co-op screens with no Live, the
/// sign-in prompt at its code page, the account menu, and the kit's
/// example. List screens step through their real classes
/// ([`ListPage`]): their rows write an in-memory profile store, never a
/// file.
pub fn live(name: &str) -> Option<Box<dyn Live>> {
    match name {
        "signin-code" => Some(Box::new(Driven::new(crate::signin_screen::SigninPage))),
        "account" => Some(Box::new(Driven::new(AccountPage))),
        _ => Some(Box::new(Driven::new(list_page(name)?))),
    }
}

/// Screen `name`'s page when it is a list screen: its real class over a
/// sample (see [`live`]).
pub fn list_page(name: &str) -> Option<ListPage> {
    let skull_store = || {
        let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
        for i in [0, 2, 4, 9, 12] {
            skulls::record_pickup(&store, &["Chief".to_string()], i);
        }
        let _ = skulls::toggle(&store, "Chief", SKULLS[4].key);
        store
    };
    let lobby = || {
        let levels = vec![coop_live::Level {
            stem: "03a_oldmombasa".into(),
            campaign: 1,
            map: 3,
            name: Some("Outskirts".into()),
        }];
        Arc::new(Lobby::new(levels, None, None, Box::new(|_| Vec::new())))
    };
    let skulls_page = |s: SkullsScreen| {
        let s = Arc::new(s);
        let class = crate::skulls_screen::class(
            Arc::clone(&s),
            skulls_menu::DESCRIPTION_TEXT,
            skulls_menu::LIST_LAYOUT,
            skulls_menu::PICTURE,
            Arc::new(crate::skulls_screen::PageFlow::default()),
        );
        ListPage::new(class, described(&s))
    };
    let list = Some;
    match name {
        "skulls" => {
            let s = SkullsScreen::new(skull_store());
            s.open("Chief".into());
            list(skulls_page(s))
        }
        "skulls.h1" => {
            let s = SkullsScreen::new(skull_store());
            s.open_with("Chief".into(), crate::h1_options::CAMPAIGN);
            list(skulls_page(s))
        }
        "skulls.mp" => {
            let s = Arc::new(SkullsScreen::multiplayer(skull_store()));
            s.open("Chief".into());
            let class = crate::skulls_mp_row::mp_class(
                Arc::clone(&s),
                skulls_menu::DESCRIPTION_TEXT,
                skulls_menu::LIST_LAYOUT,
                skulls_menu::PICTURE,
            );
            list(ListPage::new(class, described(&s)))
        }
        "h1-options" => {
            let s = Arc::new(crate::h1_options::OptionsScreen {
                store: skull_store(),
            });
            let class = crate::h1_options_row::class(
                Arc::clone(&s),
                skulls_menu::DESCRIPTION_TEXT,
                skulls_menu::LIST_LAYOUT,
            );
            list(ListPage::new(class, move || s.spec()))
        }
        "coop" | "coop.friends" | "coop.join" => {
            let lobby = lobby();
            let [host, friends, join] = coop_live::classes(&lobby);
            match name {
                "coop" => list(ListPage::new(host, described(&Arc::new(HostRows(lobby))))),
                "coop.friends" => list(ListPage::new(
                    friends,
                    described(&Arc::new(FriendRows(lobby))),
                )),
                _ => list(ListPage::new(join, described(&Arc::new(JoinRows(lobby))))),
            }
        }
        "kit.example" => {
            let s = Arc::new(crate::ui_kit::screen::example().0);
            list(ListPage::new(s.class(), described(&s)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::spec::{Finding, check_fallback};

    /// The kit's legend is the skulls page's own, glyph for glyph, and the
    /// description the page serves is the described one.
    #[test]
    fn the_skulls_legend_and_descriptions_are_the_served_ones() {
        assert_eq!(
            crate::ui_kit::input::footer_legend(&skulls_prompts()),
            skulls_menu::PROMPT
        );
        let s = sample("skulls").unwrap();
        for (row, r) in s.rows.iter().enumerate() {
            let focused = ScreenSpec {
                focus: row,
                ..s.clone()
            };
            let served = skull_text(*r.ids.last().unwrap()).unwrap();
            assert_eq!(focused.description_text().unwrap(), served, "row {row}");
        }
    }

    /// Every screen, without the disc: ids in families, presses handled.
    /// Widths are the disc test's (`preview_and_check_against_the_disc`).
    #[test]
    fn every_screen_passes_the_fallback_checks_but_the_known_ones() {
        for name in NAMES {
            let s = sample(name).unwrap();
            let f: Vec<Finding> = check_fallback(&s)
                .into_iter()
                .filter(|f| !matches!(f, Finding::Wide { .. } | Finding::Description { .. }))
                .collect();
            assert_eq!(f, vec![], "{name}: {f:#?}");
        }
    }

    #[test]
    fn screens_describe_themselves_as_json_for_the_ui_stack() {
        let s = sample("coop").unwrap();
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["name"], "coop");
        assert_eq!(j["rows"][2]["label"], "Invite a Friend");
        assert_eq!(j["rows"][2]["press"]["open"], "coop.friends");
        assert_eq!(j["rows"][0]["kind"], "info");
        let k = serde_json::to_value(sample("skulls").unwrap()).unwrap();
        assert_eq!(k["style"], "inline");
        assert_eq!(k["prompts"][0]["glyphs"][0], "DpadLeft");
    }

    /// Findings left on purpose, each with its reason: the sign-in prompt's
    /// help is words in its description, not a legend; the prompt is
    /// rebuilt as a dialog with one (docs/game/ui-builder.md, slice 6).
    fn known(screen: &str, issue: &crate::ui_kit::layout::Issue) -> bool {
        screen == crate::signin_screen::SCREEN && *issue == crate::ui_kit::layout::Issue::NoLegend
    }

    /// Without a disc: every screen in every state drawn from the synthetic
    /// source, a PNG and a JSON each, and the sheet.
    #[test]
    fn every_state_draws_without_a_disc() {
        let dir = std::env::temp_dir().join(format!("h2-ui-sheet-{}", std::process::id()));
        let from = crate::ui_kit::preview::From::Synthetic;
        let sheet = crate::ui_kit::preview::write_sheet(&from, &dir).unwrap();
        assert!(sheet.exists());
        // One picture per screen and state, none overwritten by another
        // screen built from the same spec (skulls and skulls.h1).
        let mut stems = Vec::new();
        for name in NAMES {
            for (state, _) in states(name).unwrap() {
                let stem = format!("{name}-{state}");
                assert!(dir.join(format!("{stem}.png")).exists(), "{stem}");
                stems.push(stem);
            }
        }
        let n = stems.len();
        stems.sort();
        stems.dedup();
        assert_eq!(stems.len(), n, "every state's files are its own");
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("signin-code-code-wrong.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["state"], "code-wrong");
        assert!(
            json["widgets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["id"] == "title")
        );
        // And every state passes the layout checks that need no art.
        let mut issues = Vec::new();
        for name in NAMES {
            let all = states(name).unwrap();
            assert!(!all.is_empty(), "{name}");
            for (state, spec) in all {
                let (v, _) = from.draw(&spec, &state).unwrap();
                let r = crate::ui_kit::layout::check(
                    &v,
                    from.source(),
                    &crate::ui_kit::layout::Rules::for_spec(&spec),
                );
                issues.extend(
                    r.issues
                        .iter()
                        .filter(|i| !known(name, i))
                        .map(|i| format!("{name}/{state}: {i}")),
                );
            }
        }
        assert!(issues.is_empty(), "{issues:#?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// With the disc (`disc/halo2.iso`, or `H2_DISC`): every screen checked
    /// against the real list geometry and fonts, and, with
    /// `H2_UI_PREVIEW_DIR` set, drawn there. Skipped without a disc.
    #[test]
    fn preview_and_check_against_the_disc() {
        let iso = std::env::var_os("H2_DISC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso")
            });
        if !iso.exists() {
            eprintln!("no disc at {}; skipped", iso.display());
            return;
        }
        let disc = crate::ui_kit::preview::Disc::open(&iso).unwrap();
        // The geometry without a disc is the disc's, layout for layout.
        for name in ["coop", "skulls"] {
            let place = sample(name).unwrap().list;
            let d = disc.geometry(place).unwrap();
            let f = crate::ui_kit::view::Geometry::fallback(place);
            assert_eq!(f, d, "{name}: Geometry::fallback differs from the disc");
        }
        // Every state passes the layout checks on the disc's art and fonts,
        // the art ones included.
        let mut issues = Vec::new();
        for name in NAMES {
            for (state, spec) in states(name).unwrap() {
                let v = disc.view(&spec, &state).unwrap();
                let r = crate::ui_kit::layout::check(
                    &v,
                    &disc,
                    &crate::ui_kit::layout::Rules::for_spec(&spec),
                );
                assert!(r.skipped.is_empty(), "{name}/{state}: {:?}", r.skipped);
                issues.extend(
                    r.issues
                        .iter()
                        .filter(|i| !known(name, i))
                        .map(|i| format!("{name}/{state}: {i}")),
                );
            }
        }
        for i in &issues {
            eprintln!("layout: {i}");
        }
        assert!(issues.is_empty(), "{} layout findings", issues.len());
        if let Some(dir) = std::env::var_os("H2_UI_SHEET_DIR") {
            let from = crate::ui_kit::preview::From::Disc(&disc);
            let path =
                crate::ui_kit::preview::write_sheet(&from, std::path::Path::new(&dir)).unwrap();
            eprintln!("wrote {}", path.display());
        }
        let out = std::env::var_os("H2_UI_PREVIEW_DIR").map(std::path::PathBuf::from);
        let mut report = Vec::new();
        for name in NAMES {
            let s = sample(name).unwrap();
            let g = disc.geometry(s.list).unwrap();
            eprintln!(
                "{name}: list at {:?}, {} visible, items {} high, label {:?} font {} ({:?}), value {:?}",
                g.anchor,
                g.visible,
                g.item_height,
                g.label.0,
                g.label.1,
                disc.fonts.file(g.label.1).map(|f| f.0),
                g.value
            );
            for f in disc.check(&s).unwrap() {
                report.push(format!("{name}: {f}"));
            }
            if let Some(dir) = &out {
                std::fs::create_dir_all(dir).unwrap();
                for focus in [0, s.rows.len().saturating_sub(1)] {
                    let f = ScreenSpec { focus, ..s.clone() };
                    let path = dir.join(format!("{name}-focus{focus}.png"));
                    disc.render(&f).unwrap().write_png(&path).unwrap();
                    eprintln!("wrote {}", path.display());
                }
            }
        }
        // The co-op screen as the 2026-11-16 screenshot has it (focus on
        // row 4) beside that screenshot, scaled to 640 by 480, when
        // `H2_UI_COMPARE` names it.
        if let (Some(dir), Some(shot)) = (&out, std::env::var_os("H2_UI_COMPARE")) {
            let shot =
                crate::ui_kit::preview::Canvas::read_png(std::path::Path::new(&shot)).unwrap();
            let s = ScreenSpec {
                focus: 4,
                ..sample("coop").unwrap()
            };
            let path = dir.join("coop-vs-game.png");
            disc.render(&s)
                .unwrap()
                .beside(&shot)
                .write_png(&path)
                .unwrap();
            eprintln!("wrote {}", path.display());
        }
        for r in &report {
            eprintln!("finding: {r}");
        }
        assert!(report.is_empty(), "{} findings", report.len());
    }
}
