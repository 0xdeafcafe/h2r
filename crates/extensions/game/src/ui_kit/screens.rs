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
pub const NAMES: [&str; 25] = [
    "campaigns",
    "skulls",
    "skulls.h1",
    "skulls.mp",
    "h1-options",
    "variant-more-options",
    "coop",
    "coop.friends",
    "coop.join",
    "signin-code",
    "account",
    "kit.example",
    "kit.choices",
    "kit.dialog",
    "kit.dialog.large",
    "clankers",
    crate::clanker_card::SCREEN,
    "mods",
    "mods.pack",
    "mods.detail",
    crate::theater_ui::FILMS,
    crate::theater_ui::DETAIL,
    crate::theater_ui::OVERLAY,
    crate::theater_ui::PAUSE_MENU,
    "kit.picker",
    "kit.picker.game",
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
        "mods" | "mods.pack" | "mods.detail" => {
            use crate::mods_screen::spec::{Sample, preview::Preview};
            Some(Box::new(Preview::new(Sample::named(name)?)))
        }
        crate::theater_ui::FILMS | crate::theater_ui::DETAIL | crate::theater_ui::OVERLAY
        | crate::theater_ui::PAUSE_MENU => {
            crate::theater_ui::preview::live(name)
        }
        _ => Some(Box::new(Driven::new(list_page(name)?))),
    }
}

/// The kit's choice rows as a dialog (docs/game/ui-builder.md "The Dialog
/// kind"): on the game's own settings dialog (the lobby's Quick Options,
/// `quick_options`), its description in Quick Options' help line, as the
/// Clankers and Halo 1 Options pages are, or on the large dialog, the one
/// the game maps have too (a pause menu's screen).
pub fn dialog_example(
    store: Arc<settings::Store>,
    base: crate::ui_kit::dialog::DialogBase,
) -> crate::ui_kit::screen::KitScreen {
    use crate::ui_kit::choice::example::{COUNT, LEVEL};
    use crate::ui_kit::dialog::DialogBase;
    let (feature, name) = match base {
        DialogBase::QuickOptions => ("kit-dialog", "kit.dialog"),
        DialogBase::Large | DialogBase::ValuePicker => ("kit-dialog-large", "kit.dialog.large"),
    };
    crate::ui_kit::screen::KitScreen::new(
        feature,
        name,
        crate::ui_kit::ids::family(name),
        "Choices",
    )
    .dialog(base, crate::ui_kit::view::Opener::MAIN_MENU)
    .row(COUNT.row(Arc::clone(&store)))
    .row(LEVEL.row(store))
    .prompts(&[
        Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
        Prompt::new(&[Glyph::B], "Back"),
    ])
}

/// The value picker's sample: the kit's Count (nine values, so the list
/// scrolls past its six), 4 held, over the lobby's Game Setup as the
/// Clankers page's pickers are.
pub fn picker_sample() -> crate::ui_kit::picker::Pick {
    use crate::ui_kit::choice::example::{COUNT, store};
    let store = store();
    let values = COUNT.choices().into_iter().map(|(_, n)| n).collect();
    let current = COUNT.index(&store);
    crate::ui_kit::picker::Pick::new(
        COUNT.label,
        values,
        current,
        COUNT.description,
        Arc::new(move |i| COUNT.set(&store, i)),
    )
    .over(crate::ui_kit::view::Opener::GAME_SETUP_CLANKERS)
}

/// The kit's choice rows on a page laid out as Game Options is (the
/// level select's list, the description pane, the legend in the footer):
/// what a page such as a bots page's Count and Difficulty looks like.
pub fn choices_example(store: Arc<settings::Store>) -> crate::ui_kit::screen::KitScreen {
    use crate::ui_kit::choice::example::{COUNT, LEVEL};
    crate::ui_kit::screen::KitScreen::new(
        "kit-choices",
        "kit.choices",
        crate::ui_kit::ids::family("kit.choices"),
        "Choices",
    )
    .layout(skulls_menu::LIST_LAYOUT, ValueStyle::Inline)
    .description(skulls_menu::DESCRIPTION_TEXT)
    .row(COUNT.row(Arc::clone(&store)))
    .row(LEVEL.row(store))
    .prompts(&[
        Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
        Prompt::new(&[Glyph::B], "Back"),
    ])
}

/// The campaign picker with Halo 1 installed beside the test campaign,
/// Chief signed in, a50 finished on Heroic, the Halo 1 options at their
/// defaults: an in-memory store and progress, never a file.
pub fn campaign_picker_sample() -> crate::campaign_menu::PickerRows {
    use content::campaign_list::{self as cl, CampaignManifest, FIXTURE, HALO1};
    let ms = [
        CampaignManifest::parse(HALO1).unwrap(),
        CampaignManifest::parse(FIXTURE).unwrap(),
    ];
    let state = crate::campaigns::State {
        built: cl::build(&ms, &[1], &[0x65]).unwrap(),
        selection: Default::default(),
        progress: crate::campaigns::Progress::memory(),
        cards: Default::default(),
    };
    state.progress.finished("Chief", "halo1", 2, 0x1003);
    state.selection.set_seen(crate::campaigns::Seen {
        profiles: vec!["Chief".into()],
        pictures: Default::default(),
    });
    let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
    crate::campaign_menu::PickerRows::new(Arc::new(state), Some(store))
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
            let class = crate::h1_options_row::class(Arc::clone(&s));
            list(ListPage::new(class, move || s.spec()))
        }
        "variant-more-options" => {
            let p = Arc::new(crate::variant_page::Page::new());
            let e = variants::ext::Ext {
                gravity: 50,
                ..Default::default()
            };
            p.read(e.encode());
            let class = crate::variant_page::class(
                Arc::clone(&p),
                skulls_menu::DESCRIPTION_TEXT,
                skulls_menu::LIST_LAYOUT,
            );
            list(ListPage::new(class, move || p.spec()))
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
        "campaigns" => {
            let rows = Arc::new(campaign_picker_sample());
            list(ListPage::new(
                crate::campaign_menu::picker_class(Arc::clone(&rows)),
                described(&rows),
            ))
        }
        "kit.example" => {
            let s = Arc::new(crate::ui_kit::screen::example().0);
            list(ListPage::new(s.class(), described(&s)))
        }
        "kit.choices" => {
            let s = Arc::new(choices_example(crate::ui_kit::choice::example::store()));
            list(ListPage::new(s.class(), described(&s)))
        }
        "kit.dialog" | "kit.dialog.large" => {
            let base = if name == "kit.dialog" {
                crate::ui_kit::dialog::DialogBase::QuickOptions
            } else {
                crate::ui_kit::dialog::DialogBase::Large
            };
            let s = Arc::new(dialog_example(
                crate::ui_kit::choice::example::store(),
                base,
            ));
            list(ListPage::new(s.class(), described(&s)))
        }
        crate::clanker_card::SCREEN => list(crate::clanker_card::page_sample()),
        "clankers" => {
            let store = Arc::new(settings::Store::defaults(&crate::clankers::DEFS));
            let s = Arc::new(crate::clankers::page(store));
            list(ListPage::new(s.class(), described(&s)))
        }
        "mods" | "mods.pack" | "mods.detail" => {
            use crate::mods_screen::spec::{Sample, sample_repo};
            let open = Sample::named(name)?;
            list(open.page(sample_repo(open)))
        }
        "kit.picker" | "kit.picker.game" => {
            crate::ui_kit::picker::offer(picker_sample());
            let p = Arc::new(crate::ui_kit::picker::Picker::new(
                name == crate::ui_kit::picker::GAME_NAME,
            ));
            list(ListPage::new(p.class(), described(&p)))
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

    /// Our settings dialogs: the pages that are Quick Options inside.
    const QUICK_PAGES: [&str; 2] = ["clankers", "h1-options"];

    /// Screen `name` drawn from `from` has every widget the kit's own
    /// Quick Options sample (`kit.dialog`) has where that sample has it:
    /// the panel, the title, the rows' bars, labels and values, the help
    /// line and the legend, each the same art or font and justification.
    fn assert_quick_options_inside(from: &crate::ui_kit::preview::From<'_>, name: &str) {
        use crate::ui_kit::view::Kind;
        let (quick, _) = from.draw(&sample("kit.dialog").unwrap(), "q").unwrap();
        let (v, _) = from.draw(&sample(name).unwrap(), "q").unwrap();
        let mut compared = 0;
        for w in &quick.widgets {
            let same = w.id.starts_with("panel.")
                || ["title", "description", "legend"].contains(&w.id.as_str())
                || ["row.0.", "row.1."].iter().any(|p| w.id.starts_with(p));
            if !same {
                continue;
            }
            let ours = v
                .widget(&w.id)
                .unwrap_or_else(|| panic!("{name}: no {}", w.id));
            assert_eq!(ours.rect, w.rect, "{name}: {}", w.id);
            assert_eq!(ours.layer, w.layer, "{name}: {}", w.id);
            match (&ours.kind, &w.kind) {
                (Kind::Art { art: a, .. }, Kind::Art { art: b, .. }) => {
                    assert_eq!(a, b, "{name}: {}", w.id)
                }
                (
                    Kind::Text {
                        font: a,
                        justify: ja,
                        ..
                    },
                    Kind::Text {
                        font: b,
                        justify: jb,
                        ..
                    },
                ) => assert_eq!((a, ja), (b, jb), "{name}: {}", w.id),
                _ => panic!("{name}: {} is another kind", w.id),
            }
            compared += 1;
        }
        for id in [
            "panel.0",
            "title",
            "description",
            "legend",
            "row.0.label",
            "row.0.value",
        ] {
            assert!(quick.widget(id).is_some(), "kit.dialog has {id}");
        }
        assert!(compared >= 8, "{name}: {compared} widgets compared");
        assert_eq!(v.list, quick.list, "{name}");
    }

    /// The Clankers and Halo 1 Options pages are Quick Options inside, not
    /// just its frame: a dialog on `quick_options`, its list (skin,
    /// anchor, a row per setting), each value alone in the value column,
    /// Quick Options' header (the focused row's one line in the help line,
    /// its icon in the game type picture), a capitalised title and the
    /// legend in its footer; drawn, laid out as the kit's Quick Options
    /// sample is ([`assert_quick_options_inside`]), each row's icon where
    /// Quick Options has its picture.
    #[test]
    fn our_settings_dialogs_are_quick_options_inside() {
        use crate::ui_kit::dialog::{
            DialogBase, DialogGeometry, QUICK_HELP, QUICK_HELP_TEXT, QUICK_OPTIONS_SCREEN_ID,
            QUICK_PICTURE, QUICK_PICTURE_DEPTH, QUICK_PICTURE_SIZE,
        };
        let g = DialogGeometry::fallback_for(DialogBase::QuickOptions);
        for name in QUICK_PAGES {
            let s = sample(name).unwrap();
            let d = s
                .dialog
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: a dialog"));
            assert_eq!(d.base, DialogBase::QuickOptions, "{name}");
            assert_eq!(d.rows, s.rows, "{name}");
            assert_eq!(s.style, ValueStyle::Setting, "{name}: the value alone");
            assert_eq!(
                s.list,
                DialogGeometry::list_place(g.list, s.rows.len()),
                "{name}: Quick Options' own list"
            );
            assert!(
                s.rows.len() <= g.list_items,
                "{name}: fits without scrolling"
            );
            assert_eq!(
                s.description,
                Some(QUICK_HELP_TEXT.into()),
                "{name}: the help line"
            );
            assert!(
                s.footer && s.footer_text().is_some(),
                "{name}: a footer legend"
            );
            assert_eq!(s.title, s.title.to_uppercase(), "{name}: as QUICK OPTIONS");
            for r in &s.rows {
                let (label, value) = r.cells(s.style);
                assert_eq!(label, r.label, "{name}");
                let v = value.unwrap_or_else(|| panic!("{name}: {} has a value", r.label));
                assert!(!v.contains('<') && !v.contains('>'), "{name}: {v:?}");
                let help = r.description.as_deref().unwrap_or("");
                assert!(!help.is_empty() && !help.contains('\n'), "{name}: {help:?}");
                assert!(
                    help.encode_utf16().count() <= 52,
                    "{name}: {help:?}, one line"
                );
            }
            // Quick Options' header: the focused row's line in the help
            // line, one line, and its icon in Quick Options' game type
            // picture; nothing of the page's own in their place.
            assert_eq!(s.help, None, "{name}: the rows' lines");
            assert!(s.art.is_empty(), "{name}: the icon is the focused row's");
            let room = i32::from(QUICK_HELP.right) - i32::from(QUICK_HELP.left);
            for (row, r) in s.rows.iter().enumerate() {
                let focused = ScreenSpec {
                    focus: row,
                    ..s.clone()
                };
                let help = r.description.as_deref().unwrap_or("");
                assert!(
                    crate::ui_kit::spec::fallback::width(help) <= room,
                    "{name}: {help:?} is more than the help line's one line"
                );
                assert_eq!(focused.description_text().as_deref(), Some(help), "{name}");
                let art = focused.shown_art();
                assert_eq!(art.len(), 1, "{name}: {}'s icon", r.label);
                let icon = &art[0];
                assert!(
                    matches!(
                        icon.art,
                        crate::ui_kit::view::Art::Screen { screen_id, pane: 0, index: 1, image }
                            if screen_id == QUICK_OPTIONS_SCREEN_ID
                                && (0..crate::ui_kit::dialog::icon::COUNT as i16).contains(&image)
                    ),
                    "{name}: {:?} is not an icon in Quick Options' picture",
                    icon.art
                );
                assert_eq!(
                    (icon.at, icon.size, icon.depth),
                    (QUICK_PICTURE.at, QUICK_PICTURE_SIZE, QUICK_PICTURE_DEPTH),
                    "{name}"
                );
            }
            assert_quick_options_inside(&crate::ui_kit::preview::From::Synthetic, name);
            let (v, _) = crate::ui_kit::preview::From::Synthetic
                .draw(&s, "q")
                .unwrap();
            let w = v
                .widget("art.0")
                .unwrap_or_else(|| panic!("{name}: no icon drawn"));
            assert_eq!(
                w.rect,
                [
                    QUICK_PICTURE.at[1],
                    QUICK_PICTURE.at[0],
                    QUICK_PICTURE.at[1] - QUICK_PICTURE_SIZE[1] as i16,
                    QUICK_PICTURE.at[0] + QUICK_PICTURE_SIZE[0] as i16,
                ],
                "{name}: the icon where Quick Options has its picture"
            );
        }
    }

    /// A mod's page is the pregame lobby's layout, drawn from `from`: its
    /// one row (the action) where the lobby's buttons are, the lobby's
    /// panels under it, the body and status on them, the map's picture
    /// under them, and the table column's facts (white names left, pale
    /// values right, 22 apart) and checklist; nothing but the action is a
    /// row, so nothing else takes the focus.
    fn assert_lobby_inside(from: &crate::ui_kit::preview::From<'_>) {
        use crate::mods_screen::page_layout as l;
        use crate::ui_kit::view::{Art, Justify, Kind, Layer};
        let s = sample("mods.detail").unwrap();
        assert_eq!(s.rows.len(), 1, "one row: the action");
        assert_eq!(s.rows[0].press, Press::Act);
        assert!(!s.scrolls);
        assert_eq!(s.texts.len(), l::TEXTS);
        let (v, _) = from.draw(&s, "lobby").unwrap();
        assert_eq!(v.focus.as_deref(), Some("row.0"));
        assert!(v.widget("row.1.label").is_none());
        // The action's bar on the lobby buttons' band (153 to 174).
        let bar = v.widget("row.0.art.0").unwrap().rect;
        assert!(bar[0] <= 174 && bar[2] >= 153, "{bar:?}");
        // The lobby's backings, at the lobby's own places, as panels.
        for (k, spec) in l::ART.iter().enumerate() {
            let w = v.widget(&format!("panel.{k}")).unwrap();
            assert_eq!(w.layer, Layer::Panel);
            assert_eq!([w.rect[1], w.rect[0]], spec.at);
            assert!(matches!(
                w.kind,
                Kind::Art {
                    art: Art::Screen {
                        screen_id: 14,
                        pane: 0,
                        ..
                    },
                    ..
                }
            ));
        }
        // The picture: the lobby's picture widget, nothing over it.
        let pic = v.widget("art.0").unwrap();
        assert_eq!([pic.rect[1], pic.rect[0]], l::PICTURE.at);
        let text = |id: &str| {
            let w = v.widget(id).unwrap_or_else(|| panic!("no {id}"));
            match &w.kind {
                Kind::Text { font, justify, .. } => (w.rect, *font, *justify),
                _ => panic!("{id} is not a text"),
            }
        };
        assert_eq!(text("page.body").0, l::BODY.rect);
        assert_eq!(text("description").0, l::STATUS.rect);
        for k in 0..l::FACT_ROWS {
            let (r, font, j) = text(&format!("page.fact.{k}.label"));
            let [top, left, bottom, right] = l::row_rect(k);
            assert_eq!(
                (r, font, j),
                ([top, left, bottom, l::VALUE_LEFT - 5], 1, Justify::Left),
                "fact {k}"
            );
            let (r, _, j) = text(&format!("page.fact.{k}.value"));
            assert_eq!(
                (r, j),
                ([top, l::VALUE_LEFT, bottom, right], Justify::Right),
                "fact {k}"
            );
        }
        assert_eq!(text("page.check.heading").0[1], text("page.check.0.0").0[1]);
        assert!(l::BODY.rect[2] > l::STATUS.rect[0]);
        for w in v.widgets.iter().filter(|w| w.id.starts_with("page.")) {
            let r = w.rect;
            assert!(
                r[1] >= l::TABLE_LEFT
                    || w.id == "page.body"
                    || (w.id.starts_with("page.check.") && r[1] >= l::CHECK_LEFT),
                "{}: in the table column",
                w.id
            );
            assert!(
                r[2] > pic.rect[0] || r[3] < pic.rect[1] || r[1] > pic.rect[3],
                "{}: off the picture",
                w.id
            );
        }
    }

    #[test]
    fn a_mods_page_is_the_lobby_layout_with_one_action() {
        assert_lobby_inside(&crate::ui_kit::preview::From::Synthetic);
        // The list keeps the level select's layout, and every row is a map
        // (or a pack) its press opens.
        let list = sample("mods").unwrap();
        assert_eq!(list.list, ListPlace::from(Some(skulls_menu::LIST_LAYOUT)));
        let opens: Vec<&Press> = list.rows.iter().map(|r| &r.press).collect();
        let (detail, pack) = (
            Press::Open(crate::mods_screen::DETAIL.into()),
            Press::Open(crate::mods_screen::PACK.into()),
        );
        assert_eq!(opens, [&detail, &detail, &pack, &detail]);
    }

    #[test]
    fn a_mods_pack_is_the_lists_layout_download_all_then_its_maps() {
        let s = sample("mods.pack").unwrap();
        assert_eq!(s.list, ListPlace::from(Some(skulls_menu::LIST_LAYOUT)));
        assert_eq!(s.title, "Sample Pack");
        let labels: Vec<&str> = s.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels[0], "Download All (3)");
        assert_eq!(labels.len(), 4);
        assert_eq!(s.rows[0].press, Press::Act);
        assert!(
            s.rows[1..]
                .iter()
                .all(|r| r.press == Press::Open(crate::mods_screen::DETAIL.into()))
        );
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

    /// Findings left on purpose, each with its reason.
    ///
    /// - The dialog samples' unfocused rows (white at 40%) against what
    ///   is under them: the large dialog's panel is see-through, retail's
    ///   way, and the sample opens it over the main menu, whose logo is
    ///   under its rows (its real users open it over a pause menu); the
    ///   Quick Options dialogs (the sample, Clankers, Halo 1 Options) pass
    ///   on the disc's art, but the synthetic source has no `fs_dialog` to
    ///   cover their opener's rows.
    fn known(screen: &str, issue: &crate::ui_kit::layout::Issue) -> bool {
        use crate::ui_kit::layout::Issue;
        matches!(
            (screen, issue),
            ("kit.dialog" | "kit.dialog.large" | "clankers" | "h1-options" | "kit.picker" | "kit.picker.game", Issue::Contrast { id, .. })
                if id.starts_with("row.")
        )
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
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
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
        for base in [
            crate::ui_kit::dialog::DialogBase::Large,
            crate::ui_kit::dialog::DialogBase::QuickOptions,
            crate::ui_kit::dialog::DialogBase::ValuePicker,
        ] {
            assert_eq!(
                crate::ui_kit::dialog::DialogGeometry::fallback_for(base),
                disc.dialog_geometry_for(base).unwrap(),
                "DialogGeometry::fallback_for({base:?}) differs from the disc"
            );
        }
        // The value picker's description block and list are
        // `low_level_settings`' own.
        {
            use crate::ui_kit::dialog::{
                PICKER_ANIMATION, PICKER_HELP_TEXT, PICKER_LIST, PICKER_LIST_ROWS,
                VALUE_PICKER_SCREEN_ID,
            };
            let picker = disc
                .screen(VALUE_PICKER_SCREEN_ID)
                .expect("low_level_settings");
            assert_eq!(picker.header_layout(), 2, "the large dialog's header");
            let pane = &picker.panes[0];
            assert_eq!(pane.animation.0, PICKER_ANIMATION);
            let help = &pane.text_blocks[0];
            let r = help.rect;
            assert_eq!([r.top, r.left, r.bottom, r.right], PICKER_HELP_TEXT.rect);
            assert_eq!(help.font, PICKER_HELP_TEXT.font);
            assert_eq!(help.text_color, PICKER_HELP_TEXT.colour);
            assert_eq!(help.animation.0, PICKER_HELP_TEXT.animation);
            let list = &pane.lists[0];
            assert_eq!(list.skin, PICKER_LIST.skin);
            assert_eq!([list.anchor.x, list.anchor.y], PICKER_LIST.anchor);
            assert_eq!(list.visible_items as usize, PICKER_LIST_ROWS);
        }
        // Quick Options' help line and list, which the Clankers and Halo 1
        // Options pages use, are its tag's own, and those pages are laid out
        // as Quick Options on the disc's geometry too.
        {
            use crate::ui_kit::dialog::{
                QUICK_ANIMATION, QUICK_HELP_TEXT, QUICK_LIST, QUICK_LIST_ROWS,
                QUICK_OPTIONS_SCREEN_ID,
            };
            let quick = disc.screen(QUICK_OPTIONS_SCREEN_ID).expect("quick_options");
            let pane = &quick.panes[0];
            assert_eq!(pane.animation.0, QUICK_ANIMATION);
            let help = &pane.text_blocks[0];
            let r = help.rect;
            assert_eq!([r.top, r.left, r.bottom, r.right], QUICK_HELP_TEXT.rect);
            assert_eq!(help.font, QUICK_HELP_TEXT.font);
            assert_eq!(help.text_color, QUICK_HELP_TEXT.colour);
            assert_eq!(help.animation.0, QUICK_HELP_TEXT.animation);
            assert_eq!(
                help.flags & 3,
                1,
                "left-justified, as TextBlock::bytes writes"
            );
            let list = &pane.lists[0];
            assert_eq!(list.skin, QUICK_LIST.skin);
            assert_eq!([list.anchor.x, list.anchor.y], QUICK_LIST.anchor);
            assert_eq!(list.visible_items as usize, QUICK_LIST_ROWS);
            // Its game type picture, where our pages show their icons:
            // the block, and `mp_games2`'s icons, 40 x 40 each.
            {
                use crate::ui_kit::dialog::{
                    QUICK_PICTURE, QUICK_PICTURE_DEPTH, QUICK_PICTURE_SIZE, icon,
                };
                assert_eq!(QUICK_PICTURE.screen_id, QUICK_OPTIONS_SCREEN_ID as u32);
                assert_eq!(QUICK_PICTURE.pane, 0);
                let pic = &pane.bitmaps[QUICK_PICTURE.bitmap as usize];
                assert_eq!([pic.top_left.x, pic.top_left.y], QUICK_PICTURE.at);
                assert_eq!(pic.animation.0, QUICK_PICTURE.animation);
                assert_eq!(pic.image, QUICK_PICTURE.image);
                assert_eq!(pic.render_depth_bias, QUICK_PICTURE_DEPTH);
                assert_eq!(
                    crate::ui_kit::dialog::DialogBase::QuickOptions.content_bitmaps(),
                    [QUICK_PICTURE.bitmap as u16],
                    "the base's own picture is left out; ours goes there"
                );
                for i in 0..icon::COUNT {
                    let img = disc
                        .image(pic.bitmap, i as i16)
                        .unwrap_or_else(|| panic!("mp_games2 has no image {i}"));
                    assert_eq!(img.height as u16, QUICK_PICTURE_SIZE[1], "image {i}");
                    assert!(
                        (img.width as u16).abs_diff(QUICK_PICTURE_SIZE[0]) <= 1,
                        "image {i}: {} wide",
                        img.width
                    );
                }
                assert!(disc.image(pic.bitmap, icon::COUNT as i16).is_none());
                // Our pages' rows' lines fit the help line in its font.
                let room = i32::from(QUICK_HELP_TEXT.rect[3]) - i32::from(QUICK_HELP_TEXT.rect[1]);
                for name in QUICK_PAGES {
                    for r in sample(name).unwrap().rows {
                        let help = r.description.unwrap_or_default();
                        let w = disc.fonts.width(QUICK_HELP_TEXT.font, &help);
                        assert!(w <= room, "{name}: {help:?}: {w} of {room}");
                    }
                }
            }
            for name in QUICK_PAGES {
                assert_quick_options_inside(&crate::ui_kit::preview::From::Disc(&disc), name);
            }
        }
        // A mod's page: the pregame lobby's own blocks (pane 0), and the
        // page laid out as the lobby on the disc's geometry too.
        {
            use crate::mods_screen::page_layout as l;
            let lobby = disc
                .screen(l::LOBBY_SCREEN_ID as i16)
                .expect("pregame_lobby");
            let pane = &lobby.panes[0];
            let body = &pane.text_blocks[0];
            let r = body.rect;
            assert_eq!([r.top, r.left, r.right], [135, -258, 45]);
            assert_eq!(l::BODY.rect[..2], [r.top, r.left]);
            // The lobby's status text is conduit-13 (font 4); the page sets
            // its body in conduit-12 (font 1) on purpose, so the body and its hint fit.
            assert_eq!((body.font, l::BODY.font), (4, 1));
            assert_eq!(
                (body.text_color, body.animation.0),
                (l::BODY.colour, l::BODY.animation)
            );
            assert_eq!(body.render_depth_bias, l::TEXT_BIAS);
            // The table column's first row: a value (right) and a name.
            let value = &pane.text_blocks[11];
            let r = value.rect;
            assert_eq!(
                [r.top, r.left, r.bottom],
                [l::row_rect(0)[0], 118, l::row_rect(0)[2]]
            );
            assert_eq!(value.flags & 3, 2, "right-justified");
            assert_eq!(value.text_color, l::PALE_BLUE);
            let next = &pane.text_blocks[12];
            assert_eq!(value.rect.top - next.rect.top, l::ROW_STEP);
            for (spec, size) in l::ART.iter().zip(l::ART_SIZES) {
                let b = &pane.bitmaps[spec.bitmap as usize];
                assert_eq!([b.top_left.x, b.top_left.y], spec.at);
                assert_eq!(b.animation.0, spec.animation);
                assert_eq!(b.image, spec.image);
                let _ = size;
            }
            let pic = &pane.bitmaps[l::PICTURE.bitmap as usize];
            assert_eq!(pic.animation.0, l::PICTURE.animation);
            assert_eq!(pic.image, l::PICTURE.image);
            assert_lobby_inside(&crate::ui_kit::preview::From::Disc(&disc));
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
                for n in &r.notes {
                    eprintln!("note: {name}/{state}: {n}");
                }
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
        // The sheet first, so a finding can be looked at.
        if let Some(dir) = std::env::var_os("H2_UI_SHEET_DIR") {
            let from = crate::ui_kit::preview::From::Disc(&disc);
            let path =
                crate::ui_kit::preview::write_sheet(&from, std::path::Path::new(&dir)).unwrap();
            eprintln!("wrote {}", path.display());
        }
        assert!(issues.is_empty(), "{} layout findings", issues.len());
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
        for r in &report {
            eprintln!("finding: {r}");
        }
        assert!(report.is_empty(), "{} findings", report.len());
    }
}
