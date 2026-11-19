use super::*;
use crate::ui_kit::page::{ListEffect, Page, named, run};
use Input::*;

fn map(id: u32, name: &str, sort: i32, path: &str) -> Map {
    Map {
        id,
        name: name.into(),
        sort,
        path: path.into(),
        replaces: None,
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
    assert_eq!(s.len(), 3 + 4 + n);
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
    assert!(!prepare(Catalogue::default(), Vec::new()).active());
    let retail = Catalogue::new([map(
        0x0a,
        "Lockout",
        10,
        "scenarios\\multi\\lockout\\lockout",
    )]);
    assert!(!prepare(retail, Vec::new()).active());
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
    let mut r = feature::Registry::new(feature::Switches::defaults());
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
    r.add(&MapSelect {
        sel: Arc::new(Selection::new(sample())),
        flow: Arc::default(),
        slots: Arc::default(),
    })
    .unwrap();
    r.add(&crate::h1_options_row::SetupRow {
        data_root: std::env::temp_dir(),
    })
    .unwrap();
    let text = match r.build(&feature::Base) {
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
        "0x00149f49 SCREEN_REQUEST: observe_before [map-select:change map arms the categories] -> before [campaigns:the campaign picker first] -> original",
        "0x0019c6a0 MP_LEVEL_ARRAY: before [map-select:a Select Map of ours lists ours] -> original",
        "0x0019c190 MP_LEVEL_REFRESH: before [map-select:the mods' names and pictures onto their maps] -> original",
        "0x002afd70 MP_LEVEL_NAME: before [map-select:category and package names] -> original",
        "0x002c9ac4 MAP_LIST_ROW_TEXT: observe_before [map-select:note the list drawing] -> original",
        "0x002c9b17 MAP_LIST_ROW_CHOSEN: before [map-select:categories open their maps] -> original",
        "0x002bb384 MAP_SCREEN_FREE: before [map-select:the categories close after a map] -> original",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
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
fn a_written_list_reads_back_as_the_game_reads_the_retail_one() {
    let mut g = engine::testing::FakeGuest::new();
    g.map(0x5000_0000, 0x1000);
    let items = [(0x0a, 0), (0x14, 1), (0x2_0001, 0)];
    write_list(&mut g, 0x5000_0000, 0x5000_0800, &items).unwrap();
    assert_eq!(read_flags(&g, 0x5000_0000), items);
    let a = engine::datum::DatumArray::at(0x5000_0000);
    let h = a.header(&g).unwrap();
    assert_eq!((h.element_size, h.high_water, h.active_count), (8, 3, 3));
    // Written again with fewer: only those.
    write_list(&mut g, 0x5000_0000, 0x5000_0800, &items[..1]).unwrap();
    assert_eq!(read_flags(&g, 0x5000_0000), &items[..1]);
}

#[test]
fn rows_show_the_category_or_the_replacing_package_and_nothing_else() {
    let cat = Catalogue::with_mods(
        [
            map(0x0a, "Lockout", 10, "scenarios\\multi\\lockout\\lockout"),
            map(
                0x3e8,
                "Containment",
                130,
                "t:\\$C\\4D53006400000101\\triplicate",
            ),
            map(
                0x46,
                "Containment",
                70,
                "scenarios\\multi\\containment\\containment",
            ),
        ],
        &[("containment".into(), "Containment (Rockets)".into())],
    );
    let first = |c| cat.maps(c)[0].id;
    assert_eq!(
        row_label(&cat, Mode::Categories, first(Category::Mods)),
        Some((2, "Mods".into()))
    );
    assert_eq!(
        row_label(&cat, Mode::Categories, 0x0a),
        Some((0, "Halo 2".into()))
    );
    // A map's own name is the slot's: no label, unless a package replaces it.
    assert_eq!(row_label(&cat, Mode::Maps(Category::Halo2), 0x0a), None);
    let (n, text) = row_label(&cat, Mode::Maps(Category::Mods), 0x46).unwrap();
    assert_eq!(text, "Containment (Rockets)");
    assert!(n >= 4);
    assert_eq!(utf16z("Mods"), b"M\0o\0d\0s\0\0\0");
    assert_eq!(utf16z(&"x".repeat(40)).len(), 64, "31 units and the NUL");
}

#[test]
fn a_disc_map_a_package_replaces_is_listed_under_mods_by_the_package_name() {
    let cat = Catalogue::with_mods(
        [
            map(0x0a, "Lockout", 10, "scenarios\\multi\\lockout\\lockout"),
            map(
                0x46,
                "Containment",
                70,
                "scenarios\\multi\\Containment\\Containment",
            ),
            // A Map Pack map of the same file name is not the disc's.
            map(
                0x3e8,
                "Containment",
                130,
                "t:\\$C\\4D53006400000101\\containment",
            ),
        ],
        &[("containment".into(), "Rocket Containment".into())],
    );
    assert_eq!(
        cat.categories(),
        [Category::Halo2, Category::MapPack, Category::Mods]
    );
    assert_eq!(
        cat.maps(Category::Halo2).len(),
        1,
        "the disc's slot serves the package"
    );
    let m = &cat.maps(Category::Mods)[0];
    assert_eq!((m.id, m.name.as_str()), (0x46, "Rocket Containment"));
    assert_eq!(m.replaces.as_deref(), Some("Containment"));
    assert_eq!(cat.maps(Category::MapPack)[0].name, "Containment");
}

/// A mod on a Map Pack base is hosted on that map's slot, listed under
/// Mods by the package's name: the slot loads the installed map, which
/// the vfs serves patched.
#[test]
fn a_map_pack_map_a_package_replaces_is_listed_under_mods_by_the_package_name() {
    let cat = Catalogue::with_mods(
        [
            map(0x0a, "Lockout", 10, "scenarios\\multi\\lockout\\lockout"),
            map(0x3e8, "Turf", 130, "t:\\$C\\4D53006400000101\\turf"),
            map(0x3f2, "Relic", 140, "t:\\$C\\4D53006400000102\\dune"),
        ],
        &[("turf".into(), "Ownage Turf".into())],
    );
    assert_eq!(
        cat.categories(),
        [Category::Halo2, Category::MapPack, Category::Mods]
    );
    let m = &cat.maps(Category::Mods)[0];
    assert_eq!((m.id, m.name.as_str()), (0x3e8, "Ownage Turf"));
    assert_eq!(m.replaces.as_deref(), Some("Turf"));
    assert_eq!(
        m.path, "t:\\$C\\4D53006400000101\\turf",
        "the slot loads the installed map"
    );
    let pack: Vec<&str> = cat
        .maps(Category::MapPack)
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(pack, ["Relic"]);
    assert_eq!(
        row_label(&cat, Mode::Maps(Category::Mods), 0x3e8).map(|l| l.1),
        Some("Ownage Turf".into())
    );
}

#[test]
fn the_flow_lists_categories_then_the_picked_maps_and_closes_after_a_map() {
    let f = Flow::default();
    // A Select Map opened another way: retail.
    assert_eq!(f.building(0x100), None);
    f.change_map();
    assert_eq!(f.building(0x1614), Some(Mode::Categories));
    assert_eq!(f.building(0x9999), None, "taken once");
    assert_eq!(f.mode_of(0x1614), Some(Mode::Categories));
    f.pick(Category::Halo1);
    assert_eq!(f.building(0x2614), Some(Mode::Maps(Category::Halo1)));
    // B on the maps: freed with nothing to close; the categories stay ours.
    assert_eq!(f.freed(0x2000), None);
    assert_eq!(f.mode_of(0x2614), None);
    assert_eq!(f.mode_of(0x1614), Some(Mode::Categories));
    // Another category, then a map: the categories close once it's freed.
    f.pick(Category::Halo2);
    assert_eq!(f.building(0x3614), Some(Mode::Maps(Category::Halo2)));
    assert_eq!(f.picked(0x3614), Some(0x1000), "the categories' screen");
    assert_eq!(f.freed(0x3000), Some(0x1000));
    assert_eq!(f.freed(0x3000), None, "once");
    assert_eq!(f.freed(0x1000), None, "the categories: nothing after");
    assert_eq!(f.mode_of(0x1614), None);
}

#[test]
fn a_map_pressed_with_no_categories_under_it_closes_nothing() {
    let f = Flow::default();
    f.pick(Category::Halo1);
    assert_eq!(f.building(0x2614), Some(Mode::Maps(Category::Halo1)));
    assert_eq!(f.picked(0x2614), None);
    assert_eq!(f.freed(0x2000), None);
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
    f.change_map();
    assert!(f.snapshot_live().unwrap().contains("to be built"));
    assert_eq!(f.building(0x1614), Some(Mode::Categories));
    assert!(f.snapshot_live().unwrap().contains("1 Select Map"));
    f.pick(Category::Halo2);
    assert_eq!(f.building(0x2614), Some(Mode::Maps(Category::Halo2)));
    assert_eq!(f.picked(0x2614), Some(0x1000));
    assert!(f.snapshot_live().is_some());
    assert_eq!(f.freed(0x2000), Some(0x1000));
    assert!(f.snapshot_live().unwrap().contains("1 Select Map"));
    assert_eq!(f.freed(0x1000), None);
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
    // At boot nothing is installed: the retail list, the disc unread.
    let (cat, fills) = sources.read().unwrap();
    let sel = Selection::with_fills(cat, fills);
    assert!(!sel.active());
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 0);
    let g0 = sel.generation();

    // A download installs a package for Lockout: read again, the mod is
    // under Mods by its own name, and its slot will take the name.
    crate::mod_slots::tests::replacing_package(
        &root,
        "live-00aa",
        "lockout",
        "Lockout Snow",
        false,
    );
    refresh(&sel, &sources, || {});
    assert!(sel.generation() > g0);
    assert!(sel.active());
    let cat = sel.catalogue();
    assert_eq!(cat.categories(), [Category::Halo2, Category::Mods]);
    let m = &cat.maps(Category::Mods)[0];
    assert_eq!((m.id, m.name.as_str()), (0x0a, "Lockout Snow"));
    assert_eq!(m.replaces.as_deref(), Some("Lockout"));
    assert_eq!(sel.fills()[0].names[0], "Lockout Snow");
    assert!(!sel.take_content_changed(), "no content package came");
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);

    // A new map (a content package) installed by the refresh: the game's
    // content scan is asked for; the disc's slots aren't read again.
    let content_root = sources.content_root();
    refresh(&sel, &sources, || {
        let d = content_root.join("4D53006480020001");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("patch.lvl"), b"not a record").unwrap();
    });
    assert!(sel.take_content_changed());
    assert!(!sel.take_content_changed(), "taken once");
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    std::fs::remove_dir_all(&root).ok();
}
