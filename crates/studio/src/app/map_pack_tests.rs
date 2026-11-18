//! The Map Pack group on a synthetic install (invented maps and records,
//! no game data): the listing, opening through the scenario session,
//! saving as a file and as a package, the installed files left alone, and
//! the map pack disc's maps named from the install. The last test reads
//! the owner's installed Map Pack (ignored).
use std::path::{Path, PathBuf};

use content::map_list::{Entry, PREVIEW_LEN};
use scenario_edit::{BlockKind, Command, Value, fixture};

use super::map_pack::{self, HINT, content_root};
use super::shared_refs::Homes;
use super::{Action, Answer, App, ToolKind};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2-editor-mp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    fn data(&self) -> PathBuf {
        self.0.join("data")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A Map Pack package as `h2 import-mappack` lays it out: `<map>.map`
/// (named `file`) and a version 2 `patch.lvl` naming `map`, in
/// `$c/4d530064<offer>`. The picture is one colour, `bgr`.
fn install(
    data: &Path,
    offer: u32,
    map: &str,
    file: &str,
    name: &str,
    sort: i32,
    id: u32,
    bgr: [u8; 3],
    bytes: &[u8],
) -> PathBuf {
    let dir = content_root(data).join(format!("4d530064{offer:08x}"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut preview = Vec::with_capacity(PREVIEW_LEN);
    while preview.len() < PREVIEW_LEN {
        preview.extend_from_slice(&[bgr[0], bgr[1], bgr[2], 0xbf]);
    }
    let e = Entry {
        scenario: format!("scenarios\\multi\\{map}\\{map}"),
        map_id: id,
        names: std::array::from_fn(|_| name.to_owned()),
        descriptions: std::array::from_fn(|_| format!("{name}, invented for a test.")),
        sort,
        flags: 0,
        teams: [8; 16],
        preview,
    };
    std::fs::write(dir.join("patch.lvl"), e.to_v2().unwrap()).unwrap();
    std::fs::write(dir.join("ContentMeta.xbx"), b"not read by the editor").unwrap();
    let path = dir.join(file);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Three pack maps (directory order is not the game's), a new map's
/// package and a directory without a record beside them.
fn three(t: &Temp) -> Vec<PathBuf> {
    let map = fixture::fixture();
    let data = t.data();
    let a = install(&data, 0x0101, "ridge", "ridge.map", "Ridge", 130, 1001, [0, 0, 0xff], &map);
    let b = install(&data, 0x0102, "basin", "Basin.MAP", "Basin", 110, 1002, [0, 0xff, 0], &map);
    let c = install(&data, 0x0301, "spire", "spire.map", "Spire", 120, 1003, [0xff, 0, 0], &map);
    // A new map from a package (its offer's top bit set): Mods, not here.
    install(&data, 0x8001_0000, "mine", "mine.map", "Mine", 1000, 0x1_0000, [9, 9, 9], &map);
    std::fs::create_dir_all(content_root(&data).join("4d53006400000201")).unwrap();
    vec![a, b, c]
}

fn app_with(t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.data());
    app
}

fn move_flag(app: &mut App, x: f32) {
    app.session
        .apply(Command::set(
            BlockKind::NetgameFlags,
            0,
            "position",
            Value::Vec3([x, 3.0, 3.0]),
        ))
        .unwrap();
}

fn flag_x(app: &App) -> f32 {
    app.session
        .scenario()
        .unwrap()
        .get(BlockKind::NetgameFlags, 0, "position")
        .unwrap()
        .as_vec3()
        .unwrap()[0]
}

/// The byte offsets where `a` and `b` differ (same length).
fn diff(a: &[u8], b: &[u8]) -> Vec<usize> {
    assert_eq!(a.len(), b.len(), "the length changed");
    (0..a.len()).filter(|&i| a[i] != b[i]).collect()
}

/// The header checksum the writer recomputes (`+0x2d0`).
const CHECKSUM: std::ops::Range<usize> = 0x2d0..0x2d4;

#[test]
fn the_map_pack_lists_installed_maps_as_the_games_select_map_does() {
    let t = Temp::new("list");
    let paths = three(&t);
    let (maps, notes) = map_pack::list(&t.data());
    assert!(notes.is_empty(), "{notes:?}");
    // The game's order: by the records' sort order.
    let names: Vec<_> = maps.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Basin", "Spire", "Ridge"]);
    let keys: Vec<_> = maps.iter().map(|m| m.map.as_str()).collect();
    assert_eq!(keys, ["basin", "spire", "ridge"]);
    assert_eq!(maps[0].offer, "4d53006400000102");
    assert_eq!(maps[0].id, 1002);
    assert_eq!(maps[0].sort, 110);
    assert_eq!(maps[0].description, "Basin, invented for a test.");
    // Any case on the host, as FATX.
    assert_eq!(maps[0].path.as_ref(), Some(&paths[1]));
    // The picture is the record's, turned from B, G, R, A to RGBA.
    let pic = maps[0].picture.as_ref().unwrap();
    assert_eq!((pic.width, pic.height), (220, 207));
    assert_eq!(&pic.rgba[..4], &[0, 0xff, 0, 0xbf]);
    assert_eq!(&maps[2].picture.as_ref().unwrap().rgba[..4], &[0xff, 0, 0, 0xbf]);

    // The app lists the same once the data root is set.
    let app = app_with(&t);
    assert_eq!(app.map_pack(), maps.as_slice());
}

#[test]
fn a_record_whose_map_is_missing_is_left_out_and_said_once() {
    let t = Temp::new("missing");
    let p = three(&t);
    std::fs::remove_file(&p[2]).unwrap();
    let mut app = app_with(&t);
    let keys: Vec<_> = app.map_pack().iter().map(|m| m.map.clone()).collect();
    assert_eq!(keys, ["basin", "ridge"]);
    let said = |app: &App| {
        app.log()
            .iter()
            .filter(|l| format!("{:?}", l.status).contains("names spire.map"))
            .count()
    };
    assert_eq!(said(&app), 1);
    app.refresh_packages();
    assert_eq!(said(&app), 1, "not again on every refresh");
}

#[test]
fn nothing_installed_is_an_empty_group_with_the_hint() {
    let t = Temp::new("empty");
    let app = app_with(&t);
    assert!(app.map_pack().is_empty());
    assert!(HINT.contains("mappack.iso") && HINT.contains("h2 import-mappack"));
    assert!(!HINT.contains('\n'), "one line");
}

#[test]
fn a_pack_map_opens_in_the_scenario_tool_and_the_tags_tool() {
    let t = Temp::new("open");
    three(&t);
    let mut app = app_with(&t);
    app.dispatch(Action::OpenMapPack("spire".into()));
    assert_eq!(app.loaded_map(), Some("spire"));
    assert_eq!(app.opened_pack_map(), Some("spire"));
    assert_eq!(app.opened_package(), None);
    assert!(app.session.is_open());
    assert!(!app.session.is_derived(), "the pack map is its own base");
    assert_eq!(app.tool, ToolKind::Scenario);
    // No Halo 2 source open: no shared caches, and the log says so.
    assert_eq!(app.session.source().unwrap().shared, None);
    assert!(app.log().iter().any(|l| l.status.text().contains("needs the Halo 2 disc")));
    // The offer directory names it too.
    app.dispatch(Action::OpenMapPack("4d53006400000101".into()));
    assert_eq!(app.opened_pack_map(), Some("ridge"));
    // Every tool that reads the open map reads it.
    app.dispatch(Action::SetTool(ToolKind::Tags));
    assert!(app.tags.is_open(), "{:?}", app.tags.error);
    assert!(!app.tags.tags().is_empty());
    assert!(app.title().starts_with("ridge"));
    // An unknown name says what there is.
    app.dispatch(Action::OpenMapPack("nowhere".into()));
    let last = format!("{:?}", app.status().unwrap());
    assert!(last.contains("basin, spire, ridge"), "{last}");
    assert_eq!(app.opened_pack_map(), Some("ridge"));
}

#[test]
fn unsaved_edits_hold_opening_another_pack_map() {
    let t = Temp::new("guard");
    three(&t);
    let mut app = app_with(&t);
    app.dispatch(Action::OpenMapPack("ridge".into()));
    move_flag(&mut app, 4.0);
    // Re-picking the open map keeps the edits.
    app.dispatch(Action::OpenMapPack("RIDGE".into()));
    assert!(app.pending().is_none());
    assert!((flag_x(&app) - 4.0).abs() < 1e-6);
    app.dispatch(Action::OpenMapPack("basin".into()));
    assert_eq!(app.pending_tool(), Some(ToolKind::Scenario));
    app.resolve(Answer::Cancel);
    assert_eq!(app.opened_pack_map(), Some("ridge"));
    app.dispatch(Action::OpenMapPack("basin".into()));
    app.resolve(Answer::Discard);
    assert_eq!(app.opened_pack_map(), Some("basin"));
    assert!(!app.is_dirty());
}

#[test]
fn save_as_round_trips_byte_exact_but_for_the_edit_and_never_touches_the_pack() {
    let t = Temp::new("saveas");
    let paths = three(&t);
    let original = std::fs::read(&paths[0]).unwrap();
    let mut app = app_with(&t);
    app.dispatch(Action::OpenMapPack("ridge".into()));

    // Unedited: the same bytes.
    let same = t.0.join("same.map");
    app.dispatch(Action::SaveMapAs(same.clone()));
    assert_eq!(std::fs::read(&same).unwrap(), original);

    // One field: its 4 bytes (and the checksum) change, nothing else.
    move_flag(&mut app, 7.0);
    let out = t.0.join("ridge.map");
    app.dispatch(Action::SaveMapAs(out.clone()));
    assert!(!app.is_dirty());
    let saved = std::fs::read(&out).unwrap();
    let changed: Vec<usize> = diff(&original, &saved)
        .into_iter()
        .filter(|i| !CHECKSUM.contains(i))
        .collect();
    assert!(!changed.is_empty() && changed.len() <= 4, "{changed:?}");
    assert!(changed.last().unwrap() - changed[0] < 4, "one float: {changed:?}");
    // The same edit on the same map opened as a loose file writes the
    // same bytes: the pack path adds nothing of its own.
    let mut loose = App::new();
    loose.set_data_root(t.0.join("other"));
    let copy = t.0.join("loose").join("ridge.map");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::write(&copy, &original).unwrap();
    loose.dispatch(Action::OpenSource(copy));
    move_flag(&mut loose, 7.0);
    let out2 = t.0.join("loose.map");
    loose.dispatch(Action::SaveMapAs(out2.clone()));
    assert_eq!(std::fs::read(&out2).unwrap(), saved);

    // The installed files: refused, unchanged.
    move_flag(&mut app, 8.0);
    app.dispatch(Action::SaveMapAs(paths[0].clone()));
    assert!(format!("{:?}", app.status().unwrap()).contains("installed content"));
    let beside = paths[0].with_file_name("ridge-copy.map");
    app.dispatch(Action::SaveMapAs(beside.clone()));
    assert!(!beside.exists(), "nothing new written into $c");
    assert!(app.is_dirty());
    assert_eq!(std::fs::read(&paths[0]).unwrap(), original);
}

#[test]
fn a_pack_map_saves_as_a_package_over_it_that_reopens_with_the_edit() {
    let t = Temp::new("package");
    let paths = three(&t);
    let original = std::fs::read(&paths[1]).unwrap();
    let mut app = app_with(&t);
    app.dispatch(Action::OpenMapPack("basin".into()));
    move_flag(&mut app, 9.0);
    let form = app.session.form_defaults();
    assert_eq!(form.dir, "basin-edit");
    app.dispatch(Action::SavePackage(form));
    assert!(!app.is_dirty(), "{:?}", app.status());
    assert_eq!(app.opened_package(), Some("basin-edit"));
    assert_eq!(app.opened_pack_map(), None);
    // A package over the pack map: its base is the installed file, by
    // hash, and it holds only the edit. The pack's file stays as it was.
    let pkgs = scenario_edit::package::packages_for(&app.session.maps_dir(), "basin");
    let p = pkgs.iter().find(|p| p.dir_name() == "basin-edit").unwrap();
    assert!(!p.manifest.base.derived);
    assert_eq!(p.manifest.base.sha256, Some(content::Sha256::of(&original)));
    assert_eq!(p.manifest.patches.len(), 1);
    assert_eq!(std::fs::read(&paths[1]).unwrap(), original);
    // The resolved package is the pack map with only the edit changed.
    let entry = app
        .packages()
        .iter()
        .find(|e| e.key() == "basin-edit")
        .unwrap()
        .clone();
    let resolved =
        editor_core::packages::read_map(&app.session.maps_dir(), &entry, Some(&original)).unwrap();
    let changed: Vec<usize> = diff(&original, &resolved)
        .into_iter()
        .filter(|i| !CHECKSUM.contains(i))
        .collect();
    assert!(!changed.is_empty() && changed.len() <= 4, "{changed:?}");
    // The pack map from the list is unedited; the package has the edit.
    app.dispatch(Action::OpenMapPack("basin".into()));
    assert!((flag_x(&app) - 9.0).abs() > 1e-6);
    app.dispatch(Action::OpenPackageEntry("basin-edit".into()));
    assert_eq!(app.opened_package(), Some("basin-edit"));
    assert_eq!(app.loaded_map(), Some("basin"));
    assert!((flag_x(&app) - 9.0).abs() < 1e-6);
    assert_eq!(app.pack_map_named("basin").unwrap().name, "Basin");
}

/// The map pack disc's tree (`/demos/dlc/<package>/`, as an extracted
/// disc) holding `maps`, each the fixture map.
fn pack_disc(t: &Temp, maps: &[&str]) -> PathBuf {
    let root = t.0.join("pack-disc");
    let dlc = root.join("demos").join("dlc").join("00000001");
    std::fs::create_dir_all(&dlc).unwrap();
    std::fs::write(dlc.join("ContentMeta.xbx"), b"meta").unwrap();
    for m in maps {
        std::fs::write(dlc.join(format!("{m}.map")), fixture::fixture()).unwrap();
    }
    root
}

#[test]
fn the_map_pack_discs_maps_take_the_installed_records_names_and_order() {
    let t = Temp::new("disc");
    three(&t);
    // "lone" is on the disc but not installed: listed by its file name,
    // after the named ones.
    let disc = pack_disc(&t, &["ridge", "lone", "basin"]);
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(disc.clone()));
    assert_eq!(app.mappack().unwrap().path(), disc.as_path());
    let rows: Vec<_> = app
        .map_pack()
        .iter()
        .map(|m| (m.name.as_str(), m.path.is_some(), m.picture.is_some()))
        .collect();
    assert_eq!(
        rows,
        [("Basin", false, true), ("Ridge", false, true), ("lone", false, false)]
    );
    // It opens from the disc, not the install.
    app.dispatch(Action::OpenMapPack("ridge".into()));
    assert_eq!(app.opened_pack_map(), Some("ridge"));
    let src = app.session.source().unwrap();
    assert!(src.path.starts_with(&disc), "{}", src.path.display());
    app.dispatch(Action::OpenMapPack("lone".into()));
    assert_eq!(app.opened_pack_map(), Some("lone"));
}

#[test]
fn the_shared_map_note_names_where_raw_data_lives() {
    let all_here = Homes {
        local: 3,
        ..Homes::default()
    };
    assert_eq!(all_here.note(), None);
    assert_eq!(Homes::default().note(), None);
    let away = Homes {
        shared: 2,
        single_player_shared: 1,
        ..Homes::default()
    };
    assert_eq!(
        away.note().unwrap(),
        "raw data lives in a shared map: shared.map (2), single_player_shared.map (1) \
         (read from the disc's copy)"
    );
    let split = Homes {
        local: 4,
        shared: 1,
        ..Homes::default()
    };
    assert_eq!(
        split.note().unwrap(),
        "raw data partly in a shared map: shared.map (1), and 4 in this map"
    );
}

/// The owner's installed Map Pack (`h2 import-mappack`, or `h2 play` with
/// `mappack.iso` beside the disc), read only: every map listed with a
/// name and picture in the game's order, two opened with the disc's
/// shared maps (no geometry left unloaded), tags in `shared.map` found,
/// and a save that differs from the installed file only in the edit.
/// `H2_MAPPACK_DATA` names the data root (default `$TMPDIR/h2boot-data`);
/// the disc is `$H2_DISC`, else `disc/halo2.iso`.
#[test]
#[ignore = "needs the installed Map Pack (H2_MAPPACK_DATA) and disc/halo2.iso (or $H2_DISC)"]
fn installed_map_pack_lists_opens_and_saves() {
    let data = std::env::var_os("H2_MAPPACK_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(content::store::default_data_root);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let disc =
        std::env::var_os("H2_DISC").map_or_else(|| root.join("disc/halo2.iso"), PathBuf::from);
    let mut app = App::new();
    let scratch = Temp::new("real");
    app.set_data_root(data.clone());
    app.dispatch(Action::OpenSource(disc));
    let maps = app.map_pack().to_vec();
    eprintln!(
        "map pack: {}",
        maps.iter()
            .map(|m| format!("{} ({}, sort {})", m.name, m.map, m.sort))
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert_eq!(maps.len(), 9, "the nine maps");
    let mut ids: Vec<u32> = maps.iter().map(|m| m.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 9);
    assert!(maps.windows(2).all(|w| (w[0].sort, w[0].id) <= (w[1].sort, w[1].id)));
    for m in &maps {
        assert!(!m.name.is_empty());
        let p = m.picture.as_ref().unwrap();
        assert_eq!(p.rgba.len(), 220 * 207 * 4);
    }
    for m in [&maps[0], maps.last().unwrap()] {
        // Listed from the install (no map pack disc is open here).
        let path = m.path.clone().expect("an installed map");
        let before = std::fs::metadata(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        app.dispatch(Action::OpenMapPack(m.map.clone()));
        assert_eq!(app.opened_pack_map(), Some(m.map.as_str()), "{:?}", app.status());
        let (tris, failed) = app.session.world();
        eprintln!("{}: {}", m.map, app.session.status());
        assert!(!tris.is_empty());
        assert_eq!(failed, 0, "every section loads with the disc's shared maps");
        app.dispatch(Action::SetTool(ToolKind::Tags));
        assert!(app.tags.is_open(), "{:?}", app.tags.error);
        let ids: Vec<_> = app.tags.tags().iter().map(|t| t.id).collect();
        let shared = ids
            .iter()
            .filter(|id| app.tag_shared_note(**id).is_some())
            .count();
        eprintln!("{}: {} tags, {shared} with raw data in a shared map", m.map, ids.len());
        assert!(shared > 0, "a pack map leans on shared.map");
        app.dispatch(Action::SetTool(ToolKind::Scenario));
        let x = flag_x(&app);
        move_flag(&mut app, x + 1.0);
        let out = scratch.0.join(format!("{}.map", m.map));
        app.dispatch(Action::SaveMapAs(out.clone()));
        let saved = std::fs::read(&out).unwrap();
        let changed: Vec<usize> = diff(&original, &saved)
            .into_iter()
            .filter(|i| !CHECKSUM.contains(i))
            .collect();
        assert!(!changed.is_empty() && changed.len() <= 4, "{changed:?}");
        std::fs::remove_file(&out).unwrap();
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        app.dispatch(Action::OpenMapPack(maps[1].map.clone()));
        app.resolve(Answer::Discard);
    }
}
