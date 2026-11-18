//! The Map Pack group on a synthetic install (invented maps and records,
//! no game data): the listing, opening through the scenario session,
//! saving as a file and as a package, and the installed files left alone.
//! The last test reads the owner's installed Map Pack (ignored).
use std::path::{Path, PathBuf};

use content::map_list::{Entry, PREVIEW_LEN};
use scenario_edit::fixture;

use super::map_pack::{self, HINT, content_root};
use super::App;

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
    assert_eq!(maps[0].path, paths[1]);
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
