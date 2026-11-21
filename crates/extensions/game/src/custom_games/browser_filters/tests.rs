use super::*;
use crate::ui_kit::page::{Focus, ListEffect, Page};
use crate::ui_kit::spec::check_fallback;

#[test]
fn every_filter_row_opens_its_picker_and_applies_the_selected_value() {
    let preferences = Arc::new(Browser::new(BrowserFilters::default(), None, None));
    let page = Arc::new(FilterPage::new(
        Arc::clone(&preferences),
        vec!["Team Slayer".into(), "One Flag".into()],
    ))
    .page();
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
    let filter_page = FilterPage::new(Arc::clone(&preferences), vec!["Fiesta".into()]);
    let pick = filter_page.pick(FilterRow::Variant);
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
    let filter_page = FilterPage::new(preferences, Vec::new());
    let row = FilterRow::ALL
        .iter()
        .position(|row| matches!(row, FilterRow::Variant))
        .unwrap();
    assert_eq!(filter_page.press(row as i16), Chosen::Open(picker::NAME));
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
    let filter_page = FilterPage::new(Arc::clone(&preferences), Vec::new());
    let pick = filter_page.pick(FilterRow::Clankers);
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
