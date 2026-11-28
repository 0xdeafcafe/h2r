use super::*;
use crate::ui_kit::page::{ListEffect, Page, named, run};
use Input::*;

fn map(id: u32, name: &str, sort: i32, path: &str) -> Map {
    Map {
        id,
        name: name.into(),
        sort,
        path: path.into(),
    }
}

/// The offer directory of a new map with list id `id`.
fn new_map_path(id: u32, map: &str) -> String {
    format!(
        "t:\\$C\\{}\\{map}",
        vfs::content_dir_name(vfs::offer_id(
            crate::new_maps::HALO2_TITLE,
            content::map_list::offer_content(id)
        ))
    )
}

#[test]
fn a_map_s_category_is_where_the_game_loads_it_from() {
    let c = category_of;
    assert_eq!(c("scenarios\\multi\\midship\\midship"), Category::Halo2);
    assert_eq!(c(""), Category::Halo2);
    assert_eq!(c("t:\\$C\\4D53006400000101\\triplicate"), Category::MapPack);
    assert_eq!(c("t:\\$c\\4d530064000003ff\\warlock"), Category::MapPack);
    assert_eq!(c(&new_map_path(0x2_0000, "h1_bloodgulch")), Category::Halo1);
    assert_eq!(c(&new_map_path(0x2_0000, "H1_Bloodgulch")), Category::Halo1);
    assert_eq!(c(&new_map_path(0x2_0000, "coldsnap")), Category::Mods);
    // h1_ outside a new map's offer: not one of ours, so a mod.
    assert_eq!(c("t:\\$C\\4D53006400000101\\h1_x"), Category::MapPack);
    assert_eq!(c("t:\\$C\\4D53006400000004\\h1_x"), Category::Mods);
    // Any other content package (the disc's own offers 1 to 3, a short
    // directory name): a mod.
    assert_eq!(c("t:\\$C\\4D53006400000002\\something"), Category::Mods);
    assert_eq!(c("t:\\$C\\junk\\something"), Category::Mods);
}

#[test]
fn categories_in_order_empty_ones_hidden_and_maps_sorted() {
    let cat = Catalogue::new([
        map(
            0x14,
            "Ascension",
            20,
            "scenarios\\multi\\ascension\\ascension",
        ),
        map(0x0a, "Lockout", 10, "scenarios\\multi\\lockout\\lockout"),
        map(
            0x2_0002,
            "battle creek",
            1000,
            &new_map_path(0x2_0002, "h1_beavercreek"),
        ),
        map(
            0x2_0001,
            "Blood Gulch",
            1000,
            &new_map_path(0x2_0001, "h1_bloodgulch"),
        ),
    ]);
    assert_eq!(cat.categories(), [Category::Halo2, Category::Halo1]);
    let names = |c| {
        cat.maps(c)
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>()
    };
    // Halo 2 keeps the game's order (sort order), Halo 1 by name, any case.
    assert_eq!(names(Category::Halo2), ["Lockout", "Ascension"]);
    assert_eq!(names(Category::Halo1), ["battle creek", "Blood Gulch"]);
    assert!(cat.maps(Category::MapPack).is_empty());
    assert!(cat.maps(Category::Mods).is_empty());
}

#[test]
fn the_map_pack_keeps_its_sort_and_mods_go_by_name_then_id() {
    let mp = |id, name: &str, sort| {
        map(
            id,
            name,
            sort,
            &format!("t:\\$C\\4d530064{:08x}\\m", 0x100 + id - 0x3e7),
        )
    };
    let cat = Catalogue::new([
        mp(0x3f2, "Warlock", 140),
        mp(0x3e8, "Containment", 130),
        map(0x2_0003, "Arena", 1000, &new_map_path(0x2_0003, "arena2")),
        map(0x2_0001, "Arena", 1000, &new_map_path(0x2_0001, "arena")),
        map(
            0x2_0002,
            "Coldsnap",
            1000,
            &new_map_path(0x2_0002, "coldsnap"),
        ),
    ]);
    let ids = |c| cat.maps(c).iter().map(|m| m.id).collect::<Vec<_>>();
    assert_eq!(ids(Category::MapPack), [0x3e8, 0x3f2]);
    assert_eq!(ids(Category::Mods), [0x2_0001, 0x2_0003, 0x2_0002]);
}

#[test]
fn a_package_naming_a_slot_s_id_takes_the_slot() {
    // `0x19be00` refills the slot with the record's id: one map, the
    // package's.
    let cat = Catalogue::new([
        map(0x0a, "Lockout", 10, "scenarios\\multi\\lockout\\lockout"),
        map(
            0x0a,
            "Lockout (pack)",
            10,
            "t:\\$C\\4D53006400000101\\lockout",
        ),
        map(u32::MAX, "free", 0, ""),
    ]);
    assert_eq!(cat.categories(), [Category::MapPack]);
    assert_eq!(cat.maps(Category::MapPack)[0].name, "Lockout (pack)");
}

#[test]
fn strings_have_ids_in_the_family_one_a_map() {
    let cat = sample();
    let s = cat.strings();
    let n = cat.all().count();
    // Three titles, a name per category (Halo 2, the map pack, Halo 1,
    // mods, Firefight) and one per map.
    assert_eq!(s.len(), 3 + Category::ALL.len() + n);
    assert!(s.iter().all(|(id, _)| IDS.owns(*id)), "{s:?}");
    let mut ids: Vec<u32> = s.iter().map(|(i, _)| *i).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), s.len(), "no id twice");
    // Each map's label id names it.
    for m in cat.all() {
        let id = cat.label_id(m.id).unwrap();
        assert_eq!(s.iter().find(|(i, _)| *i == id).unwrap().1, m.name);
    }
    // The list's slots bound the ids: 7 fixed, 50 maps, in 0x40.
    assert!(7 + MAX_MAPS as u32 <= IDS.len);
}

#[test]
fn slots_and_packages_read_as_the_game_lists_them() {
    let mut slot = vec![0u8; content::map_list::SLOT_LEN];
    slot[..4].copy_from_slice(&0x0au32.to_le_bytes());
    for (i, u) in "Lockout".encode_utf16().enumerate() {
        slot[SLOT_NAME + 2 * i..][..2].copy_from_slice(&u.to_le_bytes());
    }
    slot[SLOT_PATH..][..31].copy_from_slice(b"scenarios\\multi\\lockout\\lockout");
    slot[SLOT_SORT..][..4].copy_from_slice(&10i32.to_le_bytes());
    let m = map_of_slot(&slot).unwrap();
    assert_eq!(
        m,
        map(0x0a, "Lockout", 10, "scenarios\\multi\\lockout\\lockout")
    );
    let mut free = slot.clone();
    free[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(map_of_slot(&free), None);
    assert_eq!(map_of_slot(&slot[..100]), None);

    // A package directory with a record: the path 1.0 gives it.
    let root = std::env::temp_dir().join(format!("h2-map-select-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let offer = vfs::content_dir_name(vfs::offer_id(
        crate::new_maps::HALO2_TITLE,
        content::map_list::offer_content(0x2_0001),
    ));
    let dir = root.join(&offer);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = content::Manifest::from_toml(&format!(
        "[package]\nformat = 1\nname = \"Blood Gulch\"\n\n[base]\nmap = \"h1_bloodgulch\"\nderived = true\n\n[[patch]]\nfile = \"h1_bloodgulch.map\"\nsha256 = \"{}\"\n",
        "00".repeat(32)
    ))
    .unwrap();
    let entry = content::map_list::Entry::for_manifest(
        &manifest,
        0x2_0001,
        None,
        vec![0; content::map_list::PREVIEW_LEN],
    );
    std::fs::write(dir.join("patch.lvl"), entry.to_v2().unwrap()).unwrap();
    // Not a package: no record.
    std::fs::create_dir_all(root.join("4d53006400000002")).unwrap();
    let cat = read(&[slot], &root);
    assert_eq!(cat.categories(), [Category::Halo2, Category::Halo1]);
    let h1 = &cat.maps(Category::Halo1)[0];
    assert_eq!(h1.name, "Blood Gulch");
    assert_eq!(h1.path, format!("t:\\$C\\{offer}\\h1_bloodgulch"));
    std::fs::remove_dir_all(&root).ok();
    // No content directory: the disc's maps alone.
    assert_eq!(read(&[], &root).categories(), Vec::<Category>::new());
}

#[test]
fn one_category_or_none_leaves_the_retail_list() {
    assert!(!Selection::new(Catalogue::default()).active());
    let retail = Catalogue::new([map(
        0x0a,
        "Lockout",
        10,
        "scenarios\\multi\\lockout\\lockout",
    )]);
    assert!(!Selection::new(retail).active());
    assert!(Selection::new(sample()).active());
}

// ---- the screens, pressed ----

fn page() -> StackPage {
    super::page(&Arc::new(Selection::new(sample())))
}

/// One row of a table: from a named state, the presses, their effects, and
/// what shows after.
fn check(cases: &[(&str, &[Input], Vec<ListEffect>, &str)]) {
    for (i, (from, inputs, effects, after)) in cases.iter().enumerate() {
        let p = page();
        let mut s = named(&p, from);
        let got = run(&p, &mut s, inputs.iter().copied());
        assert_eq!(
            &got, effects,
            "case {i}: from {from} on {inputs:?}: effects"
        );
        assert_eq!(
            p.summary(&s),
            *after,
            "case {i}: from {from} on {inputs:?}: shows"
        );
    }
}

#[test]
fn categories_open_their_maps_and_b_comes_back() {
    use ListEffect as E;
    check(&[
        // The categories, every one with maps, in order.
        ("open", &[], vec![], "map-categories | focus 0 Halo 2"),
        (
            "open",
            &[Down, Down, Down, Down],
            vec![E::Focus(1), E::Focus(2), E::Focus(3), E::None],
            "map-categories | focus 3 Halo: CE",
        ),
        // A opens a category's maps, Halo 2's in the game's order.
        (
            "open",
            &[A],
            vec![E::Open(MAPS)],
            "map-list | focus 0 Lockout",
        ),
        (
            "open",
            &[A, Down, Down],
            vec![E::Open(MAPS), E::Focus(1), E::Focus(2)],
            "map-list | focus 2 Zanzibar",
        ),
        // B on the maps comes back to the categories, the focus where it was.
        (
            "open",
            &[Down, A, B],
            vec![E::Focus(1), E::Open(MAPS), E::Close],
            "map-categories | focus 1 Map Pack",
        ),
        // Then another category: its own maps, from the top.
        (
            "maps-mappack",
            &[B, Down, A],
            vec![E::Close, E::Focus(2), E::Open(MAPS)],
            "map-list | focus 0 Arena",
        ),
        // Mods and Halo 1 by name.
        (
            "maps-mods",
            &[Down],
            vec![E::Focus(1)],
            "map-list | focus 1 Coldsnap",
        ),
        (
            "maps-halo1",
            &[Down, Down, Down],
            vec![E::Focus(1), E::Focus(2), E::None],
            "map-list | focus 2 Hang 'Em High",
        ),
        // A on a map is the screen's action (the lobby's map, then back to
        // Game Setup); B on the categories closes them.
        (
            "maps-halo2",
            &[Down, A],
            vec![E::Focus(1), E::Action],
            "map-list | focus 1 Ascension",
        ),
        ("open", &[B], vec![E::Close], "closed"),
        (
            "open",
            &[A, B, B],
            vec![E::Open(MAPS), E::Close, E::Close],
            "closed",
        ),
        // Back is B; X, Y, Start and sideways do nothing.
        (
            "maps-halo2",
            &[Back],
            vec![E::Close],
            "map-categories | focus 0 Halo 2",
        ),
        (
            "maps-halo2",
            &[X, Y, Start, Left, Right],
            vec![E::None; 5],
            "map-list | focus 0 Lockout",
        ),
    ]);
}

#[test]
fn a_map_s_press_says_which_map_and_the_titles_follow_the_category() {
    let sel = Arc::new(Selection::new(sample()));
    let p = super::page(&sel);
    let mut s = p.after(&[Down, Down, Down, A, Down]);
    assert_eq!(p.spec(&s).title, "Halo: CE");
    assert_eq!(p.step(&mut s, A), ListEffect::Action);
    let pressed = sel.take_pressed().unwrap();
    let m = sel.catalogue().maps(Category::Halo1)[1].clone();
    assert_eq!((pressed, m.name.as_str()), (m.id, "Blood Gulch"));
    assert_eq!(sel.take_pressed(), None, "taken once");
    // The rows the game is handed: the category's label as the title, a
    // label id a map, a press that pushes the maps on the same stack.
    let rows = MapRows(Arc::clone(&sel));
    assert_eq!(rows.title(), IDS.id(CATEGORY_LABELS + 3));
    assert_eq!(rows.count(), 3);
    assert_eq!(
        rows.texts(1),
        [(0, sel.catalogue().label_id(m.id).unwrap())]
    );
    assert!(rows.texts(3).is_empty());
    assert_eq!(rows.press(9), Chosen::Nothing);
    let cats = CategoryRows(Arc::clone(&sel));
    assert_eq!(cats.count(), 4);
    assert_eq!(cats.press(1), Chosen::Push(MAPS));
    assert_eq!(sel.category(), Some(Category::MapPack));
    assert_eq!(cats.press(4), Chosen::Nothing);
}

/// Named states show what their presses chose, whichever state was stepped
/// last (the rows' selection is shared).
#[test]
fn named_states_replay_their_presses() {
    let p = page();
    let states = p.states();
    let names: Vec<&str> = states.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "open",
            "maps-halo2",
            "maps-mappack",
            "maps-mods",
            "maps-halo1"
        ]
    );
    let titles: Vec<String> = states.iter().map(|(_, s)| p.spec(s).title).collect();
    assert_eq!(
        titles,
        ["Select Map", "Halo 2", "Map Pack", "Mods", "Halo: CE"]
    );
}

/// What boot adds when a second campaign and a map list of several
/// categories are both installed: the campaigns' hooks and picker, the map
/// select's hook and both its screens, and the Halo 1 Options row on the
/// same Game Setup. It plans with no refusal: both pickers swap a loader at
/// `0x149f49`, and a plan with no order between them stopped every boot.
#[test]
fn the_boot_s_features_plan_together_with_the_campaigns() {
    use content::campaign_list::{self as cl, CampaignManifest, FIXTURE, HALO1};
    let ms = [
        CampaignManifest::parse(HALO1).unwrap(),
        CampaignManifest::parse(FIXTURE).unwrap(),
    ];
    let campaigns = Arc::new(crate::campaigns::State {
        built: cl::build(&ms, &[1], &[0x65]).unwrap(),
        selection: crate::campaigns::Selection::default(),
        progress: crate::campaigns::Progress::memory(),
        cards: Default::default(),
    });
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    let none: Arc<dyn Fn() -> u32 + Send + Sync> = Arc::new(|| 0);
    r.add(&crate::campaign_menu::Campaigns {
        state: Arc::clone(&campaigns),
        picker: Arc::clone(&none),
    })
    .unwrap();
    r.add(&crate::list_class::ListClass::with_rows(
        "campaigns-screen",
        crate::campaign_menu::SCREEN,
        Arc::new(crate::campaign_menu::PickerRows::new(
            Arc::clone(&campaigns),
            None,
        )),
    ))
    .unwrap();
    let navigation = Arc::new(Navigation::default());
    r.add(&ui_back::RetailBack(Arc::clone(&navigation))).unwrap();
    r.add(&MapSelect {
        sel: Arc::new(Selection::new(sample())),
        navigation,
        slots: Arc::default(),
        live: None,
    })
    .unwrap();
    r.add(&crate::h1_options_row::SetupRow {
        data_root: std::env::temp_dir(),
        page: crate::h1_options_row::test_page(),
    })
    .unwrap();
    let text = match r.build(&hooks::Base) {
        Ok(p) => p.text(),
        Err(c) => panic!(
            "refused: {}",
            c.iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };
    for line in [
        "0x00149f49 SCREEN_REQUEST: before [campaigns:the campaign picker first] -> original",
        "0x0019c6a0 MP_LEVEL_ARRAY: before [map-select:a Select Map of ours lists ours] -> original",
        "0x00137fe0 GAME_TICK: before [map-select:refresh installed map offers, map-select:land a Firefight lobby on its kit map, map-select:pick a map the game type supports] -> original",
        "0x002afd70 MP_LEVEL_NAME: before [map-select:category names] -> original",
        "0x002c9ac4 MAP_LIST_ROW_TEXT: observe_before [map-select:note the list drawing] -> original",
        "0x0011cae0 UI_LANGUAGE: before [map-select:a map's pane lists what it supports] -> original",
        "0x002c9b17 MAP_LIST_ROW_CHOSEN: before [map-select:categories open their maps] -> original",
        "0x002bb384 MAP_SCREEN_FREE: observe_before [map-select:forget the freed map list] -> original",
        "0x002300ea SCREEN_BACK_EVENT: before [map-select:return from maps to their folders",
        "0x00147f6d SCREEN_PUSH: before [ui-back:remember the replaced lobby selector, map-select:restore the opened folder's focus] -> original",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
}

#[test]
fn refreshing_offers_wraps_the_coop_tick_inside_agent_and_skull_calls() {
    use agent::sim::calls;
    use guest_layout::points::GAME_TICK;

    use crate::{coop_probe, skulls_custom};

    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    registry.add(&calls::CallsFeature(calls::shared())).unwrap();
    let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
    let skulls = skulls_custom::Custom::new(store, None, None, false);
    registry
        .add(&skulls_custom::SkullsCustom(Arc::new(skulls)))
        .unwrap();
    registry
        .add(&MapSelect {
            sel: Arc::new(Selection::new(sample())),
            navigation: Arc::default(),
            slots: Arc::default(),
            live: None,
        })
        .unwrap();
    let coop = coop_probe::Probe::armed(coop_probe::Arm::session(), std::path::PathBuf::new());
    registry.add(&coop).unwrap();

    let plan = registry
        .build(&hooks::Base)
        .unwrap_or_else(|errors| panic!("{errors:?}"));
    let before = &plan.chains[&GAME_TICK.base].before;
    assert_eq!(
        before.iter().map(|hook| hook.feature).collect::<Vec<_>>(),
        [
            calls::FEATURE,
            skulls_custom::FEATURE,
            // Select Map's three tick hooks: the offers' refresh, the
            // Firefight lobby's landing on its kit map, and the pick of a
            // map the game type supports.
            FEATURE,
            FEATURE,
            FEATURE,
            coop_probe::LIVE_ID,
        ]
    );
}

// ---- the retail Select Map in two modes ----

#[test]
fn category_mode_lists_each_category_s_first_map_and_maps_mode_its_maps() {
    let cat = sample();
    let locked = |id: u32| u8::from(id == 0x14);
    let first = |c| cat.maps(c)[0].id;
    assert_eq!(
        list_for(&cat, Mode::Categories, &locked),
        [
            (first(Category::Halo2), 0),
            (first(Category::MapPack), 0),
            (first(Category::Mods), 0),
            (first(Category::Halo1), 0),
        ]
    );
    let halo2 = list_for(&cat, Mode::Maps(Category::Halo2), &locked);
    let ids: Vec<u32> = cat.maps(Category::Halo2).iter().map(|m| m.id).collect();
    assert_eq!(halo2.iter().map(|(i, _)| *i).collect::<Vec<_>>(), ids);
    // The retail lock byte goes with its map (Ascension here).
    assert_eq!(halo2[1], (0x14, 1));
}

#[test]
fn rows_the_dialog_will_not_name_are_those_the_game_lacks_or_locks() {
    let cat = sample();
    let items = list_for(&cat, Mode::Maps(Category::MapPack), &|id| {
        u8::from(id == 0x3f2)
    });
    // Containment is in the catalogue, but the game's own scan didn't put
    // it in a slot; Warlock is there, locked.
    let slots = [0x0a, 0x14, 0x3f2, u32::MAX];
    assert_eq!(
        unshown(&cat, &items, &slots),
        [
            ("Containment (0x3e8)".to_string(), "not in the game's list"),
            ("Warlock (0x3f2)".to_string(), "locked"),
        ]
    );
    // Both in slots, neither locked: nothing to say.
    let items = list_for(&cat, Mode::Maps(Category::MapPack), &|_| 0);
    assert!(unshown(&cat, &items, &[0x3e8, 0x3f2]).is_empty());
}

#[test]
fn a_written_list_reads_back_as_the_game_reads_the_retail_one() {
    let mut g = guest_layout::testing::FakeGuest::new();
    g.map(0x5000_0000, 0x1000);
    let items = [(0x0a, 0), (0x14, 1), (0x2_0001, 0)];
    write_list(&mut g, 0x5000_0000, 0x5000_0800, &items).unwrap();
    assert_eq!(read_flags(&g, 0x5000_0000), items);
    let a = guest_layout::datum::DatumArray::at(0x5000_0000);
    let h = a.header(&g).unwrap();
    assert_eq!((h.element_size, h.high_water, h.active_count), (8, 3, 3));
    // Written again with fewer: only those.
    write_list(&mut g, 0x5000_0000, 0x5000_0800, &items[..1]).unwrap();
    assert_eq!(read_flags(&g, 0x5000_0000), &items[..1]);
}

#[test]
fn categories_report_overflow_even_when_no_mod_fits() {
    let catalogue = Catalogue::new([
        map(1, "Lockout", 1, "scenarios\\multi\\lockout\\lockout"),
        map(2, "Ascension", 2, "scenarios\\multi\\ascension\\ascension"),
    ])
    .with_overflow(3);
    assert_eq!(catalogue.categories(), [Category::Halo2, Category::Mods]);
    assert_eq!(
        list_for(&catalogue, Mode::Categories, &|_| 0),
        [(1, 0), (2, 0)]
    );
    assert_eq!(
        row_label(&catalogue, Mode::Categories, 2),
        Some((2, "Mods (3 mods not shown, list full)".into()))
    );
    assert!(list_for(&catalogue, Mode::Maps(Category::Mods), &|_| 0).is_empty());
    assert_eq!(row_label(&catalogue, Mode::Maps(Category::Halo2), 1), None);
}

#[test]
fn a_folder_return_arms_categories_even_after_the_original_is_freed() {
    let f = Flow::default();
    assert_eq!(f.opening(), None);
    f.categories(None);
    let categories = f.opening().unwrap();
    assert_eq!(categories.mode, Mode::Categories);
    assert_eq!(categories.focus, None);
    f.built(0x1614, categories.mode, None, categories.from_setup);

    f.open(Mode::Maps(Category::Halo1));
    let maps = f.opening().unwrap();
    assert_eq!(maps.mode, Mode::Maps(Category::Halo1));
    f.built(0x2614, maps.mode, None, maps.from_setup);
    assert!(!f.maps_from_setup(0x2614));
    f.freed(0x1000);
    assert_eq!(f.mode_of(0x1614), None);
    let Some(Mode::Maps(folder)) = f.mode_of(0x2614) else {
        panic!("map list lost its category");
    };
    f.categories(Some(folder));
    let restored = f.opening().unwrap();
    assert_eq!(restored.mode, Mode::Categories);
    assert_eq!(restored.focus, Some(Category::Halo1));
    assert_eq!(f.opening(), None, "taken once");
}

#[test]
fn folder_focus_uses_the_restored_order_and_live_datum_salt() {
    let mut g = guest_layout::testing::FakeGuest::new();
    let array = 0x5000_0000;
    g.map(array, 0x1000);
    let entries = [(0x0a, 0), (0x14, 0), (0x2_0001, 0)];
    write_list(&mut g, array, array + 0x800, &entries).unwrap();
    let before = focus_handle(&g, array, 0x2_0001).unwrap();
    assert_eq!(before as u16, 2);

    // A category is absent in the rebuilt list. The opened category's ID
    // stays the same and its position changes.
    write_list(&mut g, array, array + 0x800, &entries[1..]).unwrap();
    let data = guest_layout::datum::DatumArray::at(array);
    let entry = data.get_absolute(&g, 1).unwrap().unwrap();
    g.set_u16(entry, 0x9002).unwrap();
    assert_eq!(focus_handle(&g, array, 0x2_0001), Some(0x9002_0001));
    assert_eq!(focus_handle(&g, array, 0x0a), None, "fall back to default focus");
}

#[test]
fn a_maps_short_name_is_its_file_without_the_ending() {
    let m = map(0x0a, "Lockout", 10, "scenarios\\multi\\lockout\\lockout");
    assert_eq!(m.short_name(), "lockout");
    let m = map(0x1001, "Blood Gulch", 1, "maps\\H1_BloodGulch.map");
    assert_eq!(m.short_name(), "h1_bloodgulch");
}

#[test]
fn snapshots_wait_while_a_select_map_of_ours_is_armed_or_up() {
    let f = Flow::default();
    assert_eq!(f.snapshot_live(), None);
    f.categories(None);
    assert!(f.snapshot_live().unwrap().contains("to be built"));
    let open = f.opening().unwrap();
    f.built(0x1614, open.mode, None, open.from_setup);
    assert!(f.snapshot_live().unwrap().contains("1 Select Map"));
    f.freed(0x1000);
    assert_eq!(f.snapshot_live(), None, "every screen of ours freed");
}

// ---- packages installed while the game runs ----

fn disc_slot(id: u32, map: &str, name: &str) -> Vec<u8> {
    let mut s = vec![0u8; content::map_list::SLOT_LEN];
    s[..4].copy_from_slice(&id.to_le_bytes());
    let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    s[SLOT_NAME..SLOT_NAME + units.len()].copy_from_slice(&units);
    let path = format!("scenarios\\multi\\{map}\\{map}");
    s[SLOT_PATH..SLOT_PATH + path.len()].copy_from_slice(path.as_bytes());
    s[SLOT_SORT..SLOT_SORT + 4].copy_from_slice(&(id as i32).to_le_bytes());
    s
}

#[test]
fn a_package_installed_while_the_game_runs_is_listed_without_a_restart() {
    let root = std::env::temp_dir().join(format!("h2-map-select-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let r = Arc::clone(&reads);
    let sources = Sources::new(&root, move || {
        r.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(vec![
            disc_slot(0x0a, "lockout", "Lockout"),
            disc_slot(0x14, "ascension", "Ascension"),
        ])
    });
    // A disc-only install still has a catalogue for Change Map and Forge.
    let sel = Selection::new(sources.read(0).unwrap());
    assert!(!sel.active());
    assert_eq!(sel.catalogue().maps(Category::Halo2).len(), 2);
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    let g0 = sel.generation();

    let content_root = sources.content_root();
    refresh(&sel, &sources, || {
        let d = content_root.join("4D53006480020001");
        std::fs::create_dir_all(&d).unwrap();
        let entry = content::map_list::Entry {
            scenario: "scenarios\\multi\\mod\\mod".into(),
            map_id: 0x2_0001,
            names: std::array::from_fn(|_| "Lockout Snow".into()),
            descriptions: Default::default(),
            sort: 1000,
            flags: 0,
            teams: [4; 16],
            preview: vec![0; content::map_list::PREVIEW_LEN],
        };
        std::fs::write(d.join("patch.lvl"), entry.to_v2().unwrap()).unwrap();
        0
    });
    assert!(sel.generation() > g0);
    let cat = sel.catalogue();
    assert_eq!(cat.categories(), [Category::Halo2, Category::Mods]);
    assert_eq!(cat.maps(Category::Halo2)[0].name, "Lockout");
    let m = &cat.maps(Category::Mods)[0];
    assert_eq!((m.id, m.name.as_str()), (0x2_0001, "Lockout Snow"));
    assert!(sel.take_content_changed());
    assert!(!sel.take_content_changed());
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn pack_picker_uses_exact_installed_slots_for_disc_and_dlc_bases() {
    let mut cat = sample();
    let lockout = content::MapId([1; 32]);
    let warlock = content::MapId([2; 32]);
    let index: PackIndex = serde_json::from_value(serde_json::json!({
        "release-one": {"name":"After Hours", "members": {
            (lockout.to_string()): "lockout", (warlock.to_string()): "warlock"
        }}
    })).unwrap();
    let a = 0x2_0001;
    let b = 0x2_0002;
    cat.package_slots.insert(lockout, a);
    assert!(index.installed(&cat).is_empty(), "an incomplete install cannot become active");
    cat.package_slots.insert(warlock, b);
    cat.packs = index.installed(&cat);
    assert_eq!(list_for(&cat, Mode::Maps(Category::Mods), &|_| 0), [(a, 0)]);
    assert_eq!(row_label(&cat, Mode::Maps(Category::Mods), a).unwrap().1, "After Hours");
    assert_eq!(list_for(&cat, Mode::Pack(a), &|_| 0), [(a, 0), (b, 0)]);
    assert_eq!(row_label(&cat, Mode::Pack(a), b).unwrap().1, "Warlock");
    let members = cat.playable_maps(Category::Mods);
    assert_eq!(members.iter().map(|m| m.id).collect::<Vec<_>>(), [a, b]);
    assert_eq!(members[1].name, "After Hours / Warlock");
    cat.active_pack = Some("release-one".into());
    assert!(cat.playable_maps(Category::Halo2).iter().any(|m| m.id == a && m.name == "Lockout (After Hours)"));
    let disc = list_for(&cat, Mode::Maps(Category::Halo2), &|id| u8::from(id == 0x0a));
    assert!(disc.contains(&(a, 1)), "the pack maps onto the retail row with its lock flag");
    assert!(disc.contains(&(0x14, 0)), "uncovered bases stay on disc");
    assert!(list_for(&cat, Mode::Maps(Category::MapPack), &|_| 0).contains(&(b, 0)));
    assert_eq!(row_label(&cat, Mode::Maps(Category::Halo2), a).unwrap().1, "Lockout (After Hours)");
    let original = cat.original_row().unwrap();
    assert_eq!(row_label(&cat, Mode::Maps(Category::Mods), original).unwrap().1, "Use original maps");
}

/// There is no Firefight category: every lobby's Change Map opens the same
/// categories, the kit packages among the packages.
#[test]
fn the_lobbys_mode_picks_what_change_map_opens() {
    use crate::lobby_start::Mode;
    let kit = map(
        0x2_0003,
        "Firefight \u{2013} Lockout",
        1000,
        &new_map_path(0x2_0003, "lockout"),
    );
    let probe = map(
        0x2_0004,
        "Firefight Probe \u{2013} Lockout",
        1000,
        &new_map_path(0x2_0004, "lockout"),
    );
    assert_eq!(kit.category(), Category::Mods, "a kit package is listed with the packages");
    assert_eq!(probe.category(), Category::Mods);
    let cat = Catalogue::new([
        map(1, "Lockout", 1, r"d:\maps\lockout"),
        kit.clone(),
        probe.clone(),
    ]);
    assert_eq!(cat.categories(), [Category::Halo2, Category::Mods]);
    assert_eq!(Category::ALL.len(), 4, "there is no Firefight category");
    let _ = Mode::Firefight;
    // Every label has its own string id, clear of the footers.
    let ids: Vec<u32> = cat.strings().into_iter().map(|(id, _)| id).collect();
    let unique: std::collections::BTreeSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "every string has its own id");
    assert!(ids.iter().all(|&id| IDS.owns(id)));
}

#[test]
fn the_picker_checks_the_selected_disc_or_package_file() {
    let disc = map(1, "Arena", 0, r"scenarios\multi\arena\arena");
    let offer = map(2, "Custom arena", 0, r"t:\$C\123456789abcdef0\arena.map");
    assert_eq!(disc.slot_path(), crate::map_slots::slot_file(r"d:\maps\arena.map"));
    assert_eq!(offer.slot_path(), crate::map_slots::slot_file(&offer.path));
    assert_ne!(disc.slot_path(), offer.slot_path());
}

#[test]
fn a_firefight_variant_keeps_every_map_in_the_one_list() {
    use crate::community_variants::listed_named;
    use crate::lobby_start::Mode;
    let catalogue = Catalogue::new([
        map(1, "Lockout", 1, r"d:\maps\lockout"),
        map(2, "Firefight – Lockout", 2, &new_map_path(2, "lockout")),
        map(3, "Firefight – Beaver Creek", 3, &new_map_path(3, "beavercreek")),
        map(4, "Community Midship", 4, &new_map_path(4, "midship")),
    ]);
    let holdout = &listed_named("Holdout").unwrap().record;
    let selection = Selection::new(catalogue);
    let open = selection.begin_picker(Mode::Multiplayer, Some(holdout));
    assert_eq!(open.mode, super::Mode::Categories, "the one list of categories, no Firefight list");
    assert_eq!(selection.picker_catalogue().all().count(), 4, "unsupported maps stay visible");
    assert_eq!(selection.picker_catalogue().modes().played, crate::map_modes::Played::Firefight);
    assert_eq!(selection.begin_picker(Mode::Forge, Some(holdout)).mode, super::Mode::Categories);
}

/// Lockout with a Siege variant and an AI ground variant, Midship with
/// Siege's sites, Ascension with neither, for a lobby playing `played`.
fn with_variants(played: crate::map_modes::Played) -> Catalogue {
    use crate::map_modes::{Support, Variant, analyse};
    let cat = Catalogue::new([
        map(1, "Lockout", 1, r"d:\maps\lockout"),
        map(2, "Midship", 2, r"d:\maps\midship"),
        map(3, "Ascension", 3, r"d:\maps\ascension"),
    ]);
    let variant = |file: &str, name: &str, firefight, siege| Variant {
        file: file.into(),
        name: name.into(),
        support: Support { firefight, siege },
    };
    let view = analyse(&cat, played, None, &|m| m.name == "Midship", &|m| {
        if m.name == "Lockout" {
            vec![variant("towers", "Lockout Towers", true, false), variant("keep", "Castle Keep", false, true)]
        } else {
            Vec::new()
        }
    });
    cat.with_modes(view)
}

#[test]
fn a_map_row_reads_its_name_only() {
    use crate::map_modes::Played;
    for played in [Played::Other, Played::Firefight, Played::Siege] {
        let cat = with_variants(played);
        for (id, _) in list_for(&cat, Mode::Maps(Category::Halo2), &|_| 0) {
            assert_eq!(row_label(&cat, Mode::Maps(Category::Halo2), id), None, "the slot's own name, {played:?}");
        }
    }
}

#[test]
fn the_pane_lists_what_the_focused_map_supports() {
    use crate::map_modes::Played;
    let cat = with_variants(Played::Other);
    let maps = Mode::Maps(Category::Halo2);
    assert_eq!(pane_text(&cat, maps, 1, 0).as_deref(), Some("Supports\nFirefight on Lockout Towers\nSiege on Castle Keep"));
    assert_eq!(pane_text(&cat, maps, 2, 0).as_deref(), Some("Supports\nSiege"));
    assert_eq!(pane_text(&cat, maps, 3, 0), None);
    assert_eq!(pane_text(&cat, Mode::Categories, 1, 0), None, "a category row is not a map");
    let siege = with_variants(Played::Siege);
    assert_eq!(pane_text(&siege, maps, 3, 0).as_deref(), Some("No Siege sites or flag stand"));
}

#[test]
fn a_map_with_variants_opens_its_page_and_a_map_without_picks() {
    use crate::map_modes::Played;
    let cat = with_variants(Played::Siege);
    // Siege lists Lockout (through Castle Keep) with Midship, first.
    assert_eq!(list_for(&cat, Mode::Maps(Category::Halo2), &|_| 0), [(1, 0), (2, 0), (3, 0)]);
    assert!(opens_page(&cat, 1));
    assert!(!opens_page(&cat, 2), "no variants: A picks it");
    assert!(!opens_page(&cat, 3), "refused, not opened");
    let page = Mode::Variants { map: 1, from: Category::Halo2 };
    // Every row carries the map's id, so each shows its picture.
    assert_eq!(list_for(&cat, page, &|_| 0), [(1, 0), (1, 0), (1, 0)]);
    let names: Vec<String> = (0..3).map(|row| page_label(&cat, 1, row).unwrap().1).collect();
    assert_eq!(names, ["Lockout", "Castle Keep", "Lockout Towers"]);
    assert_eq!(page_label(&cat, 1, 3), None);
    let slots: Vec<u32> = (0..3).map(|row| page_label(&cat, 1, row).unwrap().0).collect();
    assert!(slots.iter().all(|&n| n >= (Category::ALL.len() + MAX_MAPS) as u32), "past the maps' labels");
    // Each row has its own list; the map itself cannot play Siege.
    assert_eq!(pane_text(&cat, page, 1, 0).as_deref(), Some("No Siege sites or flag stand"));
    assert_eq!(pane_text(&cat, page, 1, 1).as_deref(), Some("Supports\nSiege"));
    assert_eq!(pane_text(&cat, page, 1, 2).as_deref(), Some("No Siege sites or flag stand\nSupports\nFirefight"));
    let rows = page_of(&cat, 1);
    assert_eq!(crate::map_modes::refusal(rows[0].support, Played::Siege), Some(crate::lobby_tabs::Reason::MapNoSiege));
    assert_eq!(crate::map_modes::refusal(rows[1].support, Played::Siege), None);
    assert_eq!(rows[1].file.as_deref(), Some("keep"));
    // A map without variants has no page.
    assert!(list_for(&cat, Mode::Variants { map: 2, from: Category::Halo2 }, &|_| 0).is_empty());
}

/// Change Map opens on the groups, and a group's maps read as any map
/// list's: names only, the pane under the description, and a map's page
/// that goes back to its own group.
#[test]
fn the_picker_opens_on_the_groups_and_each_group_lists_its_maps_the_same_way() {
    use crate::map_modes::{Played, Support, Variant, analyse};
    let selection = Selection::new(sample());
    let open = selection.begin_picker(crate::lobby_start::Mode::Multiplayer, None);
    assert_eq!(open.mode, Mode::Categories, "the groups first, whatever the lobby plays");
    let base = sample();
    let view = analyse(&base, Played::Other, None, &|_| false, &|m| {
        vec![Variant {
            file: "keep".into(),
            name: format!("{} Keep", m.name),
            support: Support { firefight: false, siege: true },
        }]
    });
    let cat = base.with_modes(view);
    let groups: Vec<String> = list_for(&cat, Mode::Categories, &|_| 0)
        .into_iter()
        .map(|(id, _)| row_label(&cat, Mode::Categories, id).expect("a group's row is labelled").1)
        .collect();
    assert_eq!(groups, ["Halo 2", "Map Pack", "Mods", "Halo: CE"]);
    for group in Category::ALL {
        let maps = Mode::Maps(group);
        let rows = list_for(&cat, maps, &|_| 0);
        assert_eq!(rows.len(), cat.maps(group).len(), "{group:?}");
        for (id, _) in rows {
            let name = &cat.all().find(|m| m.id == id).unwrap().name;
            assert_eq!(row_label(&cat, maps, id), None, "{name} reads its slot's own name");
            // The built-in kit maps play Firefight themselves; the variant
            // gives Siege on every map.
            let firefight = if crate::lobby_start::BUILTIN_KIT_MAPS
                .iter()
                .any(|b| b.eq_ignore_ascii_case(name)) { "Firefight\n" } else { Default::default() };
            assert_eq!(
                pane_text(&cat, maps, id, 0),
                Some(format!("Supports\n{firefight}Siege on {name} Keep")),
                "{group:?}"
            );
            assert!(opens_page(&cat, id), "{name}");
            let page = Mode::Variants { map: id, from: group };
            assert_eq!(list_for(&cat, page, &|_| 0), [(id, 0), (id, 0)]);
            assert_eq!(page_label(&cat, id, 1).unwrap().1, format!("{name} Keep"));
        }
        // Back from a map's page is its group's maps, and Back again the
        // groups on that group.
        let f = Flow::default();
        f.maps_on(group, 1);
        assert_eq!(f.opening().unwrap().mode, maps);
        f.categories(Some(group));
        let back = f.opening().unwrap();
        assert_eq!((back.mode, back.focus), (Mode::Categories, Some(group)));
    }
}

#[test]
fn back_from_a_page_reopens_its_maps_on_the_map() {
    let f = Flow::default();
    f.maps_on(Category::Halo2, 7);
    let open = f.opening().unwrap();
    assert_eq!(open.mode, Mode::Maps(Category::Halo2));
    assert_eq!((open.focus, open.focus_map), (None, Some(7)));
}

#[test]
fn the_pane_is_placed_where_the_handler_reads_a_description() {
    // The handler shows `slot + MP_DESC + (language << 8)` in 32 bits.
    let read = |slot: u32, language: u32| slot.wrapping_add(SLOT_DESC).wrapping_add(language << 8);
    for (region, slot) in [(0x0100_0000u32, 0x8012_3464u32), (0x9000_0010, 0x0034_5678), (0x0200_0abc, 0x0200_0000)] {
        let panes = [
            Mode::Categories,
            Mode::Maps(Category::Halo2),
            Mode::Pack(4),
            Mode::Variants { map: 1, from: Category::Mods },
        ];
        let mut placed = Vec::new();
        for mode in panes {
            let at = pane_at(region, mode, slot);
            assert_eq!(read(slot, pane_language(at, slot)), at, "{region:#x} {slot:#x} {mode:?}");
            assert!(at >= region + PANES_AT && at + PANE_UNITS as u32 * 2 <= region + REGION);
            placed.push(at);
        }
        assert!(placed.windows(2).all(|w| w[1] - w[0] >= PANE_UNITS as u32 * 2), "the panes do not overlap");
    }
    assert_eq!(pane_bytes(&"x".repeat(1000)).len(), PANE_UNITS * 2);
    assert_eq!(pane_bytes("ab"), [b'a', 0, b'b', 0, 0, 0]);
}
