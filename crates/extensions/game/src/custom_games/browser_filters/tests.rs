use super::*;
use crate::list_class::Rows;
use crate::ui::Chosen;
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{Focus, ListEffect, Page};
use crate::ui_kit::picker;
use crate::ui_kit::spec::check_fallback;

fn filter_page(browser: &Arc<Browser>, samples: Vec<String>) -> Arc<KitScreen> {
    Arc::new(page(Arc::clone(browser), samples))
}

#[test]
fn every_filter_row_opens_its_picker_and_applies_the_selected_value() {
    let preferences = Arc::new(Browser::new(BrowserFilters::default(), None, None));
    let screen = filter_page(&preferences, vec!["Team Slayer".into(), "One Flag".into()]);
    let page = ListPage::new(screen.class(), described(&screen));
    let picker = Arc::new(picker::Picker::new(false));
    let picker_page = ListPage::new(picker.class(), described(&picker));

    let expected = [
        ("TYPE", "Campaign co-op"),
        ("CLANKERS", "With Clankers"),
        ("GAME TYPE", "Slayer"),
        ("VARIANT", "One Flag"),
        ("MAP CATEGORY", "Map Pack"),
        ("PLAYER SPACES", "Has space"),
        ("CAMPAIGN", "Halo: CE"),
        ("DIFFICULTY", "Heroic"),
    ];
    for (index, (title, value)) in expected.into_iter().enumerate() {
        let mut focus = Focus(index);
        assert_eq!(
            page.spec(&focus).rows[index].plain_value().as_deref(),
            Some("Any")
        );
        assert_eq!(page.step(&mut focus, Input::Right), ListEffect::None);
        assert_eq!(
            page.step(&mut focus, Input::A),
            ListEffect::Open(picker::NAME)
        );
        let offer = picker::current().unwrap();
        assert_eq!(offer.title, title);
        // The picker says the row's help, from the doc.
        let help = page.spec(&focus).rows[index].description.clone();
        assert_eq!(Some(offer.description.clone()), help);
        assert_eq!(offer.current, 0);
        assert_eq!(offer.values[0], "Any");
        let chosen = Focus(offer.values.iter().position(|v| v == value).unwrap());
        let mut pick_focus = chosen;
        assert_eq!(picker_page.spec(&pick_focus).rows[chosen.0].label, value);
        assert_eq!(
            picker_page.step(&mut pick_focus, Input::B),
            ListEffect::Close
        );
        assert_eq!(
            page.spec(&focus).rows[index].plain_value().as_deref(),
            Some("Any")
        );
        assert_eq!(
            page.step(&mut focus, Input::A),
            ListEffect::Open(picker::NAME)
        );
        assert_eq!(
            picker_page.step(&mut pick_focus, Input::A),
            ListEffect::Action
        );
        assert_eq!(
            page.spec(&focus).rows[index].plain_value().as_deref(),
            Some(value)
        );
        assert_eq!(
            page.step(&mut focus, Input::A),
            ListEffect::Open(picker::NAME)
        );
        assert_eq!(picker::current().unwrap().current, chosen.0);
        assert!(check_fallback(&picker_page.spec(&pick_focus)).is_empty());
    }
    assert_eq!(
        preferences.filters(),
        BrowserFilters {
            kind: Some(BrowserKind::Coop),
            clankers: Some(true),
            game_type: Some(GameType::Slayer.engine()),
            variant: Some("One Flag".into()),
            map_category: Some(MapCategory::MapPack),
            has_space: true,
            campaign: Some(CampaignTitle::HaloCe),
            difficulty: Some(2),
        }
    );
    let mut focus = Focus::default();
    assert_eq!(page.step(&mut focus, Input::X), ListEffect::Hook);
    assert_eq!(preferences.filters(), BrowserFilters::default());
    assert!(check_fallback(&page.spec(&focus)).is_empty());
}

#[test]
fn the_current_variant_remains_selectable_and_matches_by_its_full_name() {
    let name = "custom ".repeat(8) + "variant";
    let filters = BrowserFilters {
        variant: Some(name.clone()),
        ..BrowserFilters::default()
    };
    let preferences = Arc::new(Browser::new(filters, None, None));
    let pick = pick(&preferences, &["Fiesta".into()], FilterRow::Variant);
    assert_eq!(pick.values.len(), 3);
    assert!(pick.values[pick.current].ends_with("..."));
    (pick.set)(pick.current);
    assert_eq!(
        preferences.filters().variant.as_deref(),
        Some(name.as_str())
    );
    (pick.set)(0);
    assert_eq!(preferences.filters().variant, None);
}

#[test]
fn the_variant_picker_opens_even_when_only_any_is_available() {
    let preferences = Arc::new(Browser::new(BrowserFilters::default(), None, None));
    let screen = filter_page(&preferences, Vec::new());
    let row = FilterRow::ALL
        .iter()
        .position(|row| matches!(row, FilterRow::Variant))
        .unwrap();
    assert_eq!(screen.press(row as i16), Chosen::Open(picker::NAME));
    assert_eq!(picker::current().unwrap().values, ["Any"]);
}

#[test]
fn selecting_from_an_open_picker_preserves_other_changed_filters() {
    let preferences = Arc::new(Browser::new(
        BrowserFilters {
            variant: Some("Fiesta".into()),
            ..BrowserFilters::default()
        },
        None,
        None,
    ));
    let pick = pick(&preferences, &[], FilterRow::Clankers);
    preferences.clear();
    (pick.set)(2);
    assert_eq!(
        preferences.filters(),
        BrowserFilters {
            clankers: Some(false),
            ..BrowserFilters::default()
        }
    );
}

#[test]
fn the_saved_preferences_round_trip_and_clear_as_one_setting() {
    let dir = std::env::temp_dir().join(format!("h2-browser-filters-{}", std::process::id()));
    let file = dir.join(FILE);
    let preferences = Browser::new(BrowserFilters::default(), Some(file.clone()), None);
    assert_eq!(
        load(&dir.join("absent.toml")).unwrap(),
        BrowserFilters::default()
    );
    preferences.update(|filters| {
        filters.kind = Some(BrowserKind::Multiplayer);
        filters.clankers = Some(false);
        filters.variant = Some("One Flag".into());
        filters.map_category = Some(MapCategory::MapPack);
        filters.has_space = true;
    });
    assert_eq!(load(&file).unwrap(), preferences.filters());
    preferences.clear();
    assert_eq!(load(&file).unwrap(), BrowserFilters::default());
    fs::remove_dir_all(dir).unwrap();
}

/// The page is the shared settings dialog: a row per filter in the
/// order of [`FilterRow::ALL`], each bound by name, the header the same
/// in every state, and X CLEAR FILTERS in the legend.
#[test]
fn the_page_is_the_shared_dialog_with_a_clear_button() {
    use crate::ui_kit::dialog::icon;
    use crate::ui_kit::input::{Glyph, Prompt};
    use crate::ui_kit::spec::Describe;
    let doc = crate::ui_kit::data::require(SCREEN).unwrap();
    let binds: Vec<&str> = doc.rows.values().filter_map(|r| r.bind.as_deref()).collect();
    let rows: Vec<&str> = FilterRow::ALL.iter().map(|r| r.bind()).collect();
    assert_eq!(binds, rows);
    let browser = Arc::new(Browser::new(BrowserFilters::default(), None, None));
    let screen = filter_page(&browser, Vec::new());
    assert!(screen.slots() <= TEXTS.len);
    let s = screen.spec();
    assert_eq!(Rows::picture_image(&*screen), Some(icon::SQUARES));
    let texts: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["Custom games list", "Show only the games that match these filters."]);
    assert_eq!(
        s.prompts,
        [
            Prompt::new(&[Glyph::A], "SELECT"),
            Prompt::new(&[Glyph::B], "BACK"),
            Prompt::new(&[Glyph::X], "CLEAR FILTERS"),
        ],
        "no CHANGE: no row steps"
    );
    browser.update(|f| f.has_space = true);
    assert_eq!(screen.press_extra(Input::X).as_deref(), Some("browser filters: cleared"));
    assert_eq!(browser.filters(), BrowserFilters::default());
}
