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
        map(0x14, "Ascension", 20, "scenarios\\multi\\ascension\\ascension"),
        map(0x0a, "Lockout", 10, "scenarios\\multi\\lockout\\lockout"),
        map(0x2_0002, "battle creek", 1000, &new_map_path(0x2_0002, "h1_beavercreek")),
        map(0x2_0001, "Blood Gulch", 1000, &new_map_path(0x2_0001, "h1_bloodgulch")),
    ]);
    assert_eq!(cat.categories(), [Category::Halo2, Category::Halo1]);
    let names = |c| cat.maps(c).iter().map(|m| m.name.as_str()).collect::<Vec<_>>();
    // Halo 2 keeps the game's order (sort order), Halo 1 by name, any case.
    assert_eq!(names(Category::Halo2), ["Lockout", "Ascension"]);
    assert_eq!(names(Category::Halo1), ["battle creek", "Blood Gulch"]);
    assert!(cat.maps(Category::MapPack).is_empty());
    assert!(cat.maps(Category::Mods).is_empty());
}

#[test]
fn the_map_pack_keeps_its_sort_and_mods_go_by_name_then_id() {
    let mp = |id, name: &str, sort| map(id, name, sort, &format!("t:\\$C\\4d530064{:08x}\\m", 0x100 + id - 0x3e7));
    let cat = Catalogue::new([
        mp(0x3f2, "Warlock", 140),
        mp(0x3e8, "Containment", 130),
        map(0x2_0003, "Arena", 1000, &new_map_path(0x2_0003, "arena2")),
        map(0x2_0001, "Arena", 1000, &new_map_path(0x2_0001, "arena")),
        map(0x2_0002, "Coldsnap", 1000, &new_map_path(0x2_0002, "coldsnap")),
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
        map(0x0a, "Lockout (pack)", 10, "t:\\$C\\4D53006400000101\\lockout"),
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
    let manifest = content::Manifest::from_toml(
        "[package]\nformat = 1\nname = \"Blood Gulch\"\n\n[base]\nmap = \"h1_bloodgulch\"\nderived = true\n",
    )
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
    assert!(prepare(Catalogue::default()).is_none());
    let retail = Catalogue::new([map(0x0a, "Lockout", 10, "scenarios\\multi\\lockout\\lockout")]);
    assert!(prepare(retail).is_none());
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
        assert_eq!(&got, effects, "case {i}: from {from} on {inputs:?}: effects");
        assert_eq!(p.summary(&s), *after, "case {i}: from {from} on {inputs:?}: shows");
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
        ("maps-mods", &[Down], vec![E::Focus(1)], "map-list | focus 1 Coldsnap"),
        (
            "maps-halo1",
            &[Down, Down, Down],
            vec![E::Focus(1), E::Focus(2), E::None],
            "map-list | focus 2 Hang 'Em High",
        ),
        // A on a map is the screen's action (the lobby's map, then back to
        // Game Setup); B on the categories closes them.
        ("maps-halo2", &[Down, A], vec![E::Focus(1), E::Action], "map-list | focus 1 Ascension"),
        ("open", &[B], vec![E::Close], "closed"),
        ("open", &[A, B, B], vec![E::Open(MAPS), E::Close, E::Close], "closed"),
        // Back is B; X, Y, Start and sideways do nothing.
        ("maps-halo2", &[Back], vec![E::Close], "map-categories | focus 0 Halo 2"),
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
    let m = sel.catalogue.maps(Category::Halo1)[1].clone();
    assert_eq!((pressed, m.name.as_str()), (m.id, "Blood Gulch"));
    assert_eq!(sel.take_pressed(), None, "taken once");
    // The rows the game is handed: the category's label as the title, a
    // label id a map, a press that pushes the maps on the same stack.
    let rows = MapRows(Arc::clone(&sel));
    assert_eq!(rows.title(), IDS.id(CATEGORY_LABELS + 3));
    assert_eq!(rows.count(), 3);
    assert_eq!(rows.texts(1), [(0, sel.catalogue.label_id(m.id).unwrap())]);
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
        ["open", "maps-halo2", "maps-mappack", "maps-mods", "maps-halo1"]
    );
    let titles: Vec<String> = states.iter().map(|(_, s)| p.spec(s).title).collect();
    assert_eq!(
        titles,
        ["Select Map", "Halo 2", "Map Pack", "Mods", "Halo: CE"]
    );
}
