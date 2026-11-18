//! The Halo 2 Multiplayer Map Pack disc's layout, read from a directory
//! listing alone (docs/game/map-pack.md section 1): content packages under
//! `/demos/dlc/<8 hex digits>/`, each with a `ContentMeta.xbx` and its
//! `.map` files. Shared by the editor and anything else that needs "is this
//! the map pack, and where are its maps" answered the same way.

use crate::Entry;

/// Where the disc keeps its content packages, one directory per package.
pub const DLC_DIR: &str = "/demos/dlc/";
/// The package metadata file every package carries.
pub const CONTENT_META: &str = "contentmeta.xbx";

/// One map file in a content package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlcMap {
    /// The package directory's name: 8 hex digits, as on the disc.
    pub package: String,
    /// The map's file stem (`backwash`), as the disc spells it.
    pub name: String,
    pub entry: Entry,
}

/// `dir` is a package directory name: 8 hex digits.
pub fn is_package_dir(dir: &str) -> bool {
    dir.len() == 8 && dir.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `(package, file)` of a file directly inside a package directory.
fn in_package(e: &Entry) -> Option<(String, String)> {
    if e.is_dir {
        return None;
    }
    let head = e.path.get(..DLC_DIR.len())?;
    if !head.eq_ignore_ascii_case(DLC_DIR) {
        return None;
    }
    let (package, file) = e.path[DLC_DIR.len()..].split_once('/')?;
    (is_package_dir(package) && !file.contains('/')).then(|| (package.to_owned(), file.to_owned()))
}

/// The maps of every content package in `entries`, in listing order. A
/// package counts only with a `ContentMeta.xbx` (the installer's metadata),
/// so a stray folder of maps is not mistaken for one. Empty when the
/// listing is not the map pack's.
pub fn maps(entries: &[Entry]) -> Vec<DlcMap> {
    let with_meta: Vec<String> = entries
        .iter()
        .filter_map(in_package)
        .filter(|(_, f)| f.eq_ignore_ascii_case(CONTENT_META))
        .map(|(p, _)| p.to_ascii_lowercase())
        .collect();
    entries
        .iter()
        .filter_map(|e| {
            let (package, file) = in_package(e)?;
            let cut = file.len().checked_sub(4).filter(|&c| c > 0)?;
            if !file.get(cut..)?.eq_ignore_ascii_case(".map")
                || !with_meta.contains(&package.to_ascii_lowercase())
            {
                return None;
            }
            Some(DlcMap {
                package,
                name: file[..cut].to_owned(),
                entry: e.clone(),
            })
        })
        .collect()
}

/// Whether `entries` is the map pack's: at least one package with maps.
pub fn is_map_pack(entries: &[Entry]) -> bool {
    !maps(entries).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, sector: u32) -> Entry {
        Entry {
            path: path.to_owned(),
            sector,
            size: 100,
            is_dir: false,
        }
    }

    fn listing() -> Vec<Entry> {
        vec![
            file("/default.xbe", 1),
            file("/demos/dlc/maps.png", 2),
            file("/demos/dlc/00000001/ContentMeta.xbx", 3),
            file("/demos/dlc/00000001/Warlock.map", 4),
            file("/demos/dlc/00000001/patch.lvl", 5),
            file("/demos/dlc/00000002/contentmeta.xbx", 6),
            file("/DEMOS/DLC/00000002/turf.MAP", 7),
            // No metadata: not a package.
            file("/demos/dlc/00000003/stray.map", 8),
            // Not a package directory name.
            file("/demos/dlc/extras/ContentMeta.xbx", 9),
            file("/demos/dlc/extras/x.map", 10),
            file("/maps/lockout.map", 11),
        ]
    }

    #[test]
    fn maps_come_from_packages_with_metadata() {
        let m = maps(&listing());
        let names: Vec<_> = m
            .iter()
            .map(|m| (m.package.as_str(), m.name.as_str()))
            .collect();
        assert_eq!(names, [("00000001", "Warlock"), ("00000002", "turf")]);
        assert_eq!(m[1].entry.sector, 7);
    }

    #[test]
    fn a_retail_disc_is_not_a_map_pack() {
        assert!(is_map_pack(&listing()));
        let disc = vec![file("/default.xbe", 1), file("/maps/lockout.map", 2)];
        assert!(!is_map_pack(&disc));
        assert!(!is_map_pack(&[]));
    }
}
