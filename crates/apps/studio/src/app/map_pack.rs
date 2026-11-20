//! The Multiplayer Map Pack in the Maps list (docs/game/map-pack.md).
//! Two places hold its maps: the map pack disc (or a folder of it), opened
//! beside the Halo 2 disc (`editor_core::mappack`), and the copy the game
//! installed under the data root (`h2 import-mappack`, or `h2 play`'s
//! automatic install): one content package a map at
//! `<data>/disk/Partition1/TDATA/4D530064/$c/<offer ID>/` (`T:\$c\<offer
//! ID>\` to the game). The group lists the disc's maps when it is open,
//! else the installed ones, and either way names, orders and pictures them
//! as the game's Select Map does (`boot::map_select`): the installed
//! packages' `patch.lvl` records give the name, description, picture and
//! sort order, and the offer ID says a package is the Map Pack's.
//!
//! A pack map opens like a disc map ([`App::select_map`]): its own bytes,
//! with the shared caches (`shared.map`, `single_player_shared.map`,
//! `mainmenu.map`) taken from the Halo 2 disc (`MapSource::shared`), as the
//! game reads them; the pack ships no shared map of its own. Its packages
//! take the pack map as their base. The installed files are only read:
//! "Save map as" refuses any path inside `$c`.
//!
//! [`App::select_map`]: super::App
use std::path::{Path, PathBuf};
use std::sync::Arc;

use boot::map_select::{self, Catalogue, Category};

use super::App;

/// What the Map Pack group says when nothing is installed.
pub const HINT: &str = "Not installed: put mappack.iso next to halo2.iso and start the game \
     once, or run h2 import-mappack <iso> --data <data root>.";

/// Halo 2's title ID: the `TDATA` directory the content lives under.
const HALO2_TITLE: u32 = boot::new_maps::HALO2_TITLE;

/// A record's picture as RGBA, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One installed Map Pack map.
#[derive(Clone, Debug, PartialEq)]
pub struct PackMap {
    /// The map's file name without `.map`, as the record names it
    /// (`warlock`): what the game loads, the key the list opens it by.
    pub map: String,
    /// The content package's directory name, the 16-digit offer ID.
    pub offer: String,
    /// The map file, as installed; `None` for a map read from the open
    /// map pack disc.
    pub path: Option<PathBuf>,
    /// The multiplayer list's id.
    pub id: u32,
    /// The English name and description from the record.
    pub name: String,
    pub description: String,
    /// The list's sort order.
    pub sort: i32,
    pub picture: Option<Arc<Picture>>,
}

/// `T:\$c` on the host for data root `data_root`.
pub fn content_root(data_root: &Path) -> PathBuf {
    vfs::Config::new(data_root, HALO2_TITLE)
        .title_data_dir()
        .join(vfs::CONTENT_DIR)
}

/// The installed Map Pack maps under `data_root`, in the game's order, and
/// a note for each package left out.
pub fn list(data_root: &Path) -> (Vec<PackMap>, Vec<String>) {
    let root = content_root(data_root);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    dirs.sort();
    let found: Vec<(PathBuf, map_select::Map)> = dirs
        .into_iter()
        .filter_map(|d| Some((d.clone(), map_select::map_of_package(&d)?)))
        .filter(|(_, m)| map_select::category_of(&m.path) == Category::MapPack)
        .collect();
    let catalogue = Catalogue::new(found.iter().map(|(_, m)| m.clone()));
    let mut notes = Vec::new();
    let mut out = Vec::new();
    for m in catalogue.maps(Category::MapPack) {
        let Some((dir, _)) = found.iter().find(|(_, f)| f.path == m.path) else {
            continue;
        };
        let offer = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = m.file();
        let Some(path) = map_file(dir, &base) else {
            notes.push(format!(
                "Map Pack {offer}: its record names {base}.map, which is not in the package"
            ));
            continue;
        };
        let record = read_record(dir);
        out.push(PackMap {
            map: base,
            offer,
            path: Some(path),
            id: m.id,
            name: m.name.clone(),
            description: record
                .as_ref()
                .map(|e| e.descriptions[0].clone())
                .unwrap_or_default(),
            sort: m.sort,
            picture: record.map(|e| Arc::new(picture(&e.preview))),
        });
    }
    (out, notes)
}

/// The package's `patch.lvl` record, as `map_of_package` reads it.
fn read_record(dir: &Path) -> Option<content::map_list::Entry> {
    let bytes = std::fs::read(dir.join("patch.lvl")).ok()?;
    content::map_list::Entry::from_v2(bytes.get(..content::patch_lvl::V2_LEN)?).ok()
}

/// `<base>.map` in `dir`, any case (FATX names are case-insensitive).
fn map_file(dir: &Path, base: &str) -> Option<PathBuf> {
    let want = format!("{base}.map");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_file()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&want))
        })
}

/// A record's preview (`A8R8G8B8`: bytes B, G, R, A) as RGBA.
pub fn picture(preview: &[u8]) -> Picture {
    use content::map_list::{PREVIEW_H, PREVIEW_W};
    let rgba = preview
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    Picture {
        width: PREVIEW_W as u32,
        height: PREVIEW_H as u32,
        rgba,
    }
}

impl App {
    /// The Map Pack group's maps, in the game's order: the open map pack
    /// disc's, else the installed ones.
    pub fn map_pack(&self) -> &[PackMap] {
        &self.pack
    }

    /// Map Pack rows in display-name order.
    pub fn map_pack_entries(&self) -> Vec<&PackMap> {
        use editor_core::map_groups::MapGroup;
        let mut entries: Vec<_> = self.pack.iter().collect();
        entries.sort_by_cached_key(|m| (MapGroup::MapPack.sort_key(&m.name), m.map.as_str()));
        entries
    }
    /// The Map Pack map open in the scenario tool (not a package over it).
    pub fn opened_pack_map(&self) -> Option<&str> {
        self.loaded
            .as_deref()
            .filter(|m| self.opened_package.is_none() && self.is_mappack_map(m))
    }
    /// The data root the studio reads and writes.
    pub fn data_root(&self) -> PathBuf {
        self.data_root
            .clone()
            .unwrap_or_else(content::store::default_data_root)
    }

    /// Re-read the installed Map Pack and rebuild the group: the open map
    /// pack disc's maps when there is one (each named from its installed
    /// record when that is there, else by its file name, after the named
    /// ones), else the installed maps.
    pub(super) fn refresh_map_pack(&mut self) {
        let (installed, notes) = list(&self.data_root());
        for n in notes {
            if !self.pack_notes.contains(&n) {
                self.error(n.clone());
                self.pack_notes.push(n);
            }
        }
        self.pack = match &self.mappack {
            None => installed,
            Some(disc) => {
                let mut maps: Vec<PackMap> = disc
                    .map_names()
                    .into_iter()
                    .map(|name| {
                        match installed.iter().find(|m| m.map.eq_ignore_ascii_case(&name)) {
                            Some(m) => PackMap {
                                map: name,
                                path: None,
                                ..m.clone()
                            },
                            None => PackMap {
                                name: name.clone(),
                                map: name,
                                offer: String::new(),
                                path: None,
                                id: u32::MAX,
                                description: String::new(),
                                sort: i32::MAX,
                                picture: None,
                            },
                        }
                    })
                    .collect();
                maps.sort_by_key(|m| (m.sort, m.id));
                maps
            }
        };
    }

    /// The group's entry for `key`: its map name, or an installed
    /// package's offer directory.
    pub fn pack_entry(&self, key: &str) -> Option<&PackMap> {
        self.pack.iter().find(|m| {
            m.map.eq_ignore_ascii_case(key) || (!m.offer.is_empty() && m.offer.eq_ignore_ascii_case(key))
        })
    }

    /// Where Map Pack map `name`'s bytes are: in the open map pack disc,
    /// else the installed file. `None` when it is neither's.
    pub(super) fn pack_location(
        &self,
        name: &str,
    ) -> Option<anyhow::Result<(PathBuf, Option<(u64, u64)>)>> {
        if let Some(disc) = self.mappack.as_ref().filter(|p| p.has(name)) {
            return Some(disc.location(name));
        }
        self.pack
            .iter()
            .find(|m| m.map.eq_ignore_ascii_case(name))
            .and_then(|m| m.path.clone())
            .map(|p| Ok((p, None)))
    }

    /// Why `path` may not be written by "Save map as": it is inside the
    /// installed content (`T:\$c`), the Map Pack's own files among it.
    pub fn save_refusal(&self, path: &Path) -> Option<String> {
        let canon = |p: &Path| {
            p.canonicalize().ok().or_else(|| {
                let parent = p.parent()?.canonicalize().ok()?;
                Some(parent.join(p.file_name()?))
            })
        };
        let target = canon(path)?;
        let root = canon(&content_root(&self.data_root()));
        let installed = self.pack.iter().filter_map(|m| m.path.as_deref());
        if root.is_some_and(|r| target.starts_with(r))
            || installed.into_iter().any(|p| canon(p).as_deref() == Some(target.as_path()))
        {
            return Some(format!(
                "refusing to write {}: that is the game's installed content (the Map Pack's \
                 files stay as installed). Save as a package, or pick a folder of your own",
                path.display()
            ));
        }
        None
    }

    /// Open a Map Pack map (by [`PackMap::map`] or an installed package's
    /// offer directory) in the scenario tool, and through it every tool
    /// that reads the open map (Tags, the viewport). Re-picking the open
    /// map keeps it and its edits.
    pub(super) fn open_pack_map(&mut self, key: &str) {
        let Some(name) = self.pack_entry(key).map(|m| m.map.clone()) else {
            self.error(format!(
                "no Map Pack map {key}: the Map Pack lists {}",
                if self.pack.is_empty() {
                    "nothing (not installed or opened)".to_owned()
                } else {
                    self.pack.iter().map(|m| m.map.as_str()).collect::<Vec<_>>().join(", ")
                }
            ));
            return;
        };
        if self.opened_pack_map() == Some(name.as_str()) {
            return;
        }
        self.select_map(&name);
    }

    /// The Map Pack map a package's map name is, if any.
    pub fn pack_map_named(&self, map: &str) -> Option<&PackMap> {
        self.pack.iter().find(|m| m.map.eq_ignore_ascii_case(map))
    }
}
