//! The Multiplayer Map Pack in the Maps list (docs/game/map-pack.md): the
//! maps `h2 import-mappack` (or `h2 play`'s automatic install) put under
//! the data root, one content package a map, at
//! `<data>/disk/Partition1/TDATA/4D530064/$c/<offer ID>/` (`T:\$c\<offer
//! ID>\` to the game). The list is read the way the game's Select Map
//! reads it (`boot::map_select`): each package's `patch.lvl` record gives
//! the name, description, picture and sort order, the offer ID says it is
//! the Map Pack's, and the group keeps the game's order.
//!
//! A pack map opens as a new map would ([`Session::open_derived`]): its
//! file is read whole, and the shared caches (`shared.map`,
//! `single_player_shared.map`, `mainmenu.map`) come from the Halo 2 disc,
//! as the game reads them: the pack ships no shared map of its own. The
//! installed files are only read; saving writes a package under
//! `<data>/maps` or a `.map` elsewhere, never into `$c`.
//!
//! [`Session::open_derived`]: scenario_edit::Session::open_derived
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
    /// The map file, as installed.
    pub path: PathBuf,
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
            path,
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
    /// The installed Map Pack maps, in the game's order.
    pub fn map_pack(&self) -> &[PackMap] {
        &self.pack
    }
    /// The Map Pack map open in the scenario tool, by [`PackMap::map`].
    pub fn opened_pack_map(&self) -> Option<&str> {
        self.opened_pack.as_deref()
    }
    /// The data root the studio reads and writes.
    pub fn data_root(&self) -> PathBuf {
        self.data_root
            .clone()
            .unwrap_or_else(content::store::default_data_root)
    }

    /// Re-read the installed Map Pack.
    pub(super) fn refresh_map_pack(&mut self) {
        let (pack, notes) = list(&self.data_root());
        self.pack = pack;
        for n in notes {
            if !self.pack_notes.contains(&n) {
                self.error(n.clone());
                self.pack_notes.push(n);
            }
        }
    }
}
