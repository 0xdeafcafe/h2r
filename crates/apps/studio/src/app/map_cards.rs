//! The Maps list's cards: each map's name as the game's menus show it, its
//! description and its picture, for every group (docs/editor/editor.md,
//! "The Maps list's cards"). Read from the user's own files when a source
//! opens, on a thread of their own so opening the editor never waits on
//! them; nothing of them is in the repository.
//!
//! - **Halo 2** (the open disc or folder): `mainmenu.map`'s level lists
//!   (`tags::ui_levels`, the reader the boot's map select and the Live
//!   catalog share): the multiplayer list the Select Map dialog shows and
//!   the campaign levels the Select Level screen shows, keyed by the map
//!   file stem of each entry's scenario path. Each picture is the bitmap
//!   tag the entry names, decoded from the main menu map (its raw data in
//!   that map or `shared.map`).
//! - **Map Pack**: the installed records ([`super::map_pack`]).
//! - **Halo 1** (an open Halo 1 disc or folder): `ui.map`'s map and level
//!   lists (`h1convert::ui`, as a conversion's card is made).
//! - **Packages** (yours and converted Halo 1 maps): the manifest's
//!   `[package] image` and `description`.
//!
//! A map with none (the shared caches, the main menu itself, or a map the
//! lists do not name) keeps its file name and a line on what it is.
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use editor_core::MapSource;
use editor_core::halo1::H1Source;
use editor_core::map_groups::MapGroup;
use editor_core::packages::PackageEntry;
use map::{Map, ResourceFile};
use tags::ui_levels::{self, CampaignLevel, MpLevel};

use super::App;
use super::map_pack::Picture;

/// What a card is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Campaign,
    Multiplayer,
    /// A map package's own picture and description.
    Package,
}

/// One map's card.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// The game's name for it (English).
    pub name: String,
    pub description: String,
    pub picture: Option<Arc<Picture>>,
    pub kind: Kind,
}

/// A source map as the pickers and agent API list it.
#[derive(Clone, Copy)]
pub struct MapEntry<'a> {
    pub stem: &'a str,
    pub label: &'a str,
    pub group: MapGroup,
    pub card: Option<&'a Card>,
    pub halo1: bool,
}

impl MapEntry<'_> {
    pub fn sort(entries: &mut [Self]) {
        entries.sort_by_cached_key(|entry| (entry.group.sort_key(entry.label), entry.stem));
    }

    /// Take the final section so callers can put their other maps before it.
    pub fn take_shared(entries: &mut Vec<Self>) -> Vec<Self> {
        let first = entries.partition_point(|entry| entry.group != MapGroup::SharedAndMenu);
        entries.split_off(first)
    }
}

fn map_entries<'a>(
    stems: &'a [String],
    groups: &HashMap<String, MapGroup>,
    cards: &'a CardSet,
    halo1: bool,
) -> Vec<MapEntry<'a>> {
    let mut entries: Vec<_> = stems
        .iter()
        .map(|stem| {
            let card = cards.get(stem);
            let group = match groups.get(stem).copied().unwrap_or(MapGroup::Other) {
                MapGroup::Other => MapGroup::classify(
                    stem,
                    match card.map(|card| card.kind) {
                        Some(Kind::Campaign) => MapGroup::Campaign,
                        Some(Kind::Multiplayer) => MapGroup::Multiplayer,
                        _ => MapGroup::Other,
                    },
                ),
                group => group,
            };
            let card = card.filter(|_| group != MapGroup::SharedAndMenu);
            MapEntry {
                stem,
                label: card.map_or(stem.as_str(), |card| card.name.as_str()),
                group,
                card,
                halo1,
            }
        })
        .collect();
    MapEntry::sort(&mut entries);
    entries
}

/// Cards by key (a map file stem, or a package directory), any case.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cards(HashMap<String, Card>);

impl Cards {
    pub fn get(&self, key: &str) -> Option<&Card> {
        self.0.get(&key.to_ascii_lowercase())
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Adds `card` under `key` unless the key has one already.
    fn add(&mut self, key: &str, card: Card) {
        self.0.entry(key.to_ascii_lowercase()).or_insert(card);
    }
}

/// A picture from RGBA rows, `None` when empty or not `width` x `height`.
pub fn picture(width: u32, height: u32, rgba: Vec<u8>) -> Option<Arc<Picture>> {
    let ok = width > 0 && height > 0 && rgba.len() == (width * height * 4) as usize;
    ok.then(|| {
        Arc::new(Picture {
            width,
            height,
            rgba,
        })
    })
}

/// The cards the main menu's lists give: the campaign levels, then the
/// multiplayer maps (on the disc no stem is in both; the first wins),
/// each keyed by its map file stem. An entry with no stem or no name is
/// left out. `decode` turns a picture's bitmap datum into RGBA; each
/// datum is decoded once.
pub fn cards_from(
    campaign: &[CampaignLevel],
    mp: &[MpLevel],
    decode: &mut dyn FnMut(u32) -> Option<Picture>,
) -> Cards {
    let mut decoded: HashMap<u32, Option<Arc<Picture>>> = HashMap::new();
    let mut picture_of = |id: Option<u32>| {
        let id = id?;
        decoded
            .entry(id)
            .or_insert_with(|| decode(id).and_then(|p| picture(p.width, p.height, p.rgba)))
            .clone()
    };
    let mut out = Cards::default();
    let levels = campaign
        .iter()
        .map(|l| (l.stem(), &l.name, &l.description, l.picture, Kind::Campaign));
    let maps = mp.iter().map(|l| {
        (
            l.stem(),
            &l.name,
            &l.description,
            l.picture,
            Kind::Multiplayer,
        )
    });
    for (stem, name, description, pic, kind) in levels.chain(maps) {
        let Some(stem) = stem else { continue };
        if name.is_empty() || out.get(stem).is_some() {
            continue;
        }
        out.add(
            stem,
            Card {
                name: name.clone(),
                description: description.clone(),
                picture: picture_of(pic),
                kind,
            },
        );
    }
    out
}

/// The Halo 2 source's cards, from its `mainmenu.map`. Blocking: run it
/// off the UI thread. Fails only when neither list can be read.
pub fn read_halo2(source: &MapSource) -> Result<Cards, String> {
    let mut map = source
        .open_map("mainmenu")
        .map_err(|e| format!("mainmenu.map: {e:#}"))?;
    let campaign = ui_levels::read_campaign(&mut map);
    let mp = ui_levels::read_mp(&mut map);
    if let (Err(c), Err(m)) = (&campaign, &mp) {
        return Err(format!(
            "mainmenu.map's level lists: campaign: {c}; multiplayer: {m}"
        ));
    }
    let mut files = resource_files(source);
    Ok(cards_from(
        &campaign.unwrap_or_default(),
        &mp.unwrap_or_default(),
        &mut |id| decode(&mut map, &mut files, id),
    ))
}

/// The main menu's raw-resource files in `source`: itself (as the local
/// map and as the main menu) and `shared.map`, those it has.
fn resource_files(source: &MapSource) -> assets::ResourceFiles {
    let mut files = assets::ResourceFiles::new();
    for (file, name) in [
        (ResourceFile::Local, "mainmenu"),
        (ResourceFile::MainMenu, "mainmenu"),
        (ResourceFile::Shared, "shared"),
    ] {
        let Ok((path, region)) = source.location(name) else {
            continue;
        };
        let (base, length) = match region {
            Some(r) => r,
            None => match std::fs::metadata(&path) {
                Ok(m) => (0, m.len()),
                Err(_) => continue,
            },
        };
        let _ = files.set(file, assets::Region { path, base, length });
    }
    files
}

/// Bitmap tag `id`'s first image, its base level as RGBA: through the
/// bitmap decoder, else (a linear `ARGB8888` image in the map's own file,
/// as the menu's level pictures are) read directly.
fn decode<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: u32,
) -> Option<Picture> {
    decoded(map, files, id).or_else(|| linear(map, id))
}

fn decoded<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: u32,
) -> Option<Picture> {
    let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, map::TagId(id)).ok()?.data else {
        return None;
    };
    let image = b.images.first()?;
    let d = assets::decode_image(image, files).ok()?;
    let l = d.levels.into_iter().find(|l| l.level == 0 && l.face == 0)?;
    let n = (l.width * l.height * 4) as usize;
    Some(Picture {
        width: l.width,
        height: l.height,
        rgba: l.rgba.get(..n)?.to_vec(),
    })
}

fn linear<R: Read + Seek>(map: &mut Map<R>, id: u32) -> Option<Picture> {
    let t = tags::rgba_bitmap::Template::read(map, map::TagId(id)).ok()?;
    let (width, height) = t.size();
    let rgba = t.rgba(map).ok()?;
    Some(Picture {
        width: width.into(),
        height: height.into(),
        rgba,
    })
}

/// The open Halo 1 source's cards for `maps`, from its `ui.map`'s map
/// and level lists (`h1convert::ui`). Blocking.
pub fn read_halo1(source: &H1Source, maps: &[String]) -> Result<Cards, String> {
    let ui = source
        .open_map("ui")
        .map_err(|e| format!("Halo 1's ui.map: {e:#}"))?;
    let mut out = Cards::default();
    for m in maps {
        let lower = m.to_ascii_lowercase();
        let card = match h1convert::ui::mp_card(&ui, &lower) {
            Ok(Some(c)) => Some((c, Kind::Multiplayer)),
            _ => h1convert::ui::campaign_card(&ui, &lower)
                .ok()
                .flatten()
                .map(|c| (c, Kind::Campaign)),
        };
        if let Some((c, kind)) = card {
            out.add(
                m,
                Card {
                    name: c.name,
                    description: c.description,
                    picture: picture(c.width, c.height, c.rgba),
                    kind,
                },
            );
        }
    }
    Ok(out)
}

/// A package's picture: its `[package] image`, a PNG or JPEG inside the
/// package directory.
pub fn package_picture(dir: &Path, image: &str) -> Option<Arc<Picture>> {
    // Inside the package only, as the manifest's other files.
    let rel = content::manifest::relative(image).ok()?;
    let bytes = std::fs::read(dir.join(rel)).ok()?;
    let (w, h, rgba) = content::map_list::decode_image(&bytes).ok()?;
    picture(u32::try_from(w).ok()?, u32::try_from(h).ok()?, rgba)
}

/// The packages' pictures under `maps` (`<data>/maps`), keyed by package
/// directory: `(directory, image)` for each package naming an image.
/// Blocking.
pub fn read_packages(maps: &Path, images: &[(String, String)]) -> Cards {
    let mut out = Cards::default();
    for (dir, image) in images {
        if let Some(p) = package_picture(&maps.join(dir), image) {
            out.add(
                dir,
                Card {
                    name: dir.clone(),
                    description: String::new(),
                    picture: Some(p),
                    kind: Kind::Package,
                },
            );
        }
    }
    out
}

/// One group's cards and the reader filling them.
#[derive(Default)]
pub struct CardSet {
    cards: Cards,
    rx: Option<mpsc::Receiver<Result<Cards, String>>>,
}

impl CardSet {
    /// Forget the cards and read new ones with `job` on its own thread.
    /// A reader still running for the old ones is left to finish unheard.
    fn start(&mut self, job: impl FnOnce() -> Result<Cards, String> + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
        self.cards = Cards::default();
        self.rx = Some(rx);
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    /// Takes the reader's result once it is there (waiting for it with
    /// `wait`); the reader's error, if it failed.
    fn poll(&mut self, wait: bool) -> Option<String> {
        let rx = self.rx.as_ref()?;
        let got = if wait {
            rx.recv().ok()
        } else {
            match rx.try_recv() {
                Ok(r) => Some(r),
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => None,
            }
        };
        self.rx = None;
        match got {
            Some(Ok(cards)) => {
                self.cards = cards;
                None
            }
            Some(Err(e)) => Some(e),
            None => Some("the reader stopped".into()),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Card> {
        self.cards.get(key)
    }
    /// Whether the reader is still running.
    pub fn loading(&self) -> bool {
        self.rx.is_some()
    }
    pub fn cards(&self) -> &Cards {
        &self.cards
    }
}

/// What a map with no card is, by its file stem, for the Maps list's
/// tooltip.
pub fn halo2_note(map: &str) -> &'static str {
    match map.to_ascii_lowercase().as_str() {
        "mainmenu" => {
            "The main menu: its screens, and the level lists that name and picture the other maps. Not a level."
        }
        "shared" => {
            "Shared by the multiplayer maps: the tags and art they have in common. Not a level."
        }
        "single_player_shared" => {
            "Shared by the campaign levels: the tags and art they have in common. Not a level."
        }
        _ => "Not in the main menu's level lists, so it has no name or picture of its own.",
    }
}

/// [`halo2_note`] for a Halo 1 map.
pub fn halo1_note(map: &str) -> &'static str {
    match map.to_ascii_lowercase().as_str() {
        "ui" => "Halo 1's menus: the map and level lists' names and pictures. Not a level.",
        _ => "Not in Halo 1's map or level lists, so it has no name or picture of its own.",
    }
}

impl App {
    pub fn halo2_map_entries(&self) -> Vec<MapEntry<'_>> {
        map_entries(&self.maps, &self.h2_map_groups, &self.h2_cards, false)
    }

    pub fn halo1_map_entries(&self) -> Vec<MapEntry<'_>> {
        map_entries(&self.h1_maps, &self.h1_map_groups, &self.h1_cards, true)
    }

    /// The open Halo 2 source's card for map `name`, once read.
    pub fn halo2_card(&self, name: &str) -> Option<&Card> {
        self.h2_cards.get(name)
    }
    /// Whether the Halo 2 source's cards are still being read.
    pub fn halo2_cards_loading(&self) -> bool {
        self.h2_cards.loading()
    }
    /// The picture of the package in directory `dir` (`<data>/maps/<dir>`).
    pub fn package_picture(&self, dir: &str) -> Option<&Arc<Picture>> {
        self.pkg_cards.get(dir)?.picture.as_ref()
    }

    /// Read the open Halo 2 source's cards, when it has a main menu map.
    pub(super) fn read_halo2_cards(&mut self, path: PathBuf) {
        let has_menu = self
            .source
            .as_ref()
            .is_some_and(|s| s.location("mainmenu").is_ok());
        if !has_menu {
            self.h2_cards.clear();
            return;
        }
        self.h2_cards.start(move || {
            let source = MapSource::open(&path).map_err(|e| format!("{e:#}"))?;
            read_halo2(&source)
        });
    }

    /// Read the open Halo 1 source's cards, when it has `ui.map`.
    pub(super) fn read_halo1_cards(&mut self) {
        let Some(source) = self.h1.clone() else {
            self.h1_cards.clear();
            return;
        };
        if !self.h1_maps.iter().any(|m| m.eq_ignore_ascii_case("ui")) {
            self.h1_cards.clear();
            return;
        }
        let maps = self.h1_maps.clone();
        self.h1_cards.start(move || read_halo1(&source, &maps));
    }

    /// Read the packages' pictures, when any names one.
    pub(super) fn read_package_cards(&mut self) {
        let images: Vec<(String, String)> = self
            .packages
            .iter()
            .filter_map(|e: &PackageEntry| Some((e.dir.clone(), e.image.clone()?)))
            .collect();
        if images.is_empty() {
            self.pkg_cards.clear();
            return;
        }
        let maps = self.session.maps_dir();
        self.pkg_cards
            .start(move || Ok(read_packages(&maps, &images)));
    }

    /// Take the cards readers have finished; log a reader's failure.
    pub fn poll_cards(&mut self) {
        self.take_cards(false);
    }

    /// Wait for every card reader running, then take their cards (for
    /// screenshots and tests).
    pub fn wait_cards(&mut self) {
        self.take_cards(true);
    }

    fn take_cards(&mut self, wait: bool) {
        let mut notes = Vec::new();
        for (what, set) in [
            ("the Halo 2 maps' names and pictures", &mut self.h2_cards),
            ("the Halo 1 maps' names and pictures", &mut self.h1_cards),
            ("the packages' pictures", &mut self.pkg_cards),
        ] {
            if let Some(e) = set.poll(wait) {
                notes.push(format!("{what}: {e}"));
            }
        }
        for n in notes {
            self.info(n);
        }
    }
}
