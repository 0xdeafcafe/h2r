//! What a mod changes, read off the patched map against its base, for the
//! in-game Mods page's checklist (docs/game/maps.md §5, "A mod's page";
//! `live_proto::maps::FEATURES`). `h2 live maps import` runs it on the two
//! maps it already holds and writes the result into the package's
//! `mod.toml` (`features`), which `publish` sends.
//!
//! Each map is read into a [`Census`]: every tag's size, metadata address
//! and bytes (hashed), each bitmap's own pixel data (its images' local LOD
//! 0, hashed), what the scenario places (by tag name and position) and
//! its scripts (hashed). [`features`] compares two censuses:
//!
//! - a tag the mod has and the base has not counts for its group's
//!   feature; so does a tag of the same name whose size differs, or whose
//!   bytes differ at the same address (a tag that moved and kept its size
//!   is not compared: its pointers moved with it, so a byte compare would
//!   call every tag of a grown map changed);
//! - a bitmap whose pixels differ is a texture, wherever it moved;
//! - a placement added, removed or moved counts for its kind: vehicles,
//!   weapons, or objects (scenery, equipment, controls, light fixtures,
//!   sound scenery, crates); the netgame equipment's items by their group;
//! - scripts that differ are scripted events.
//!
//! Groups: `vehi` vehicles, `weap` weapons, scenery and the other placed
//! kinds objects, `bitm` textures, `shad` and `stem` shaders, `sbsp`
//! geometry, `char` AI. The scenario tag itself is read only through its
//! placements and scripts. AI squads are not read: a mod that only moves
//! squads says `ai` in its `mod.toml`.
//!
//! The comparison is pure over two censuses and tested with invented ones.
//!
//! The census also keeps what [`crate::live_mod_contents`] reports from:
//! every tag of every group (the checklist reads only its groups), and the
//! walked fields of the groups that report names (references, numbers,
//! block counts), the scenario's among them.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek};

use anyhow::{Context, Result};
use content::Sha256;
use map::{FourCC, Map, TagId};
use tags::geometry::TagView;

use crate::live_geometry::Placement;
use crate::live_scenario::{BLOCKS, NETGAME_EQUIPMENT, NETGAME_EQUIPMENT_STRIDE, NetgameEntry};

/// One tag as compared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagPrint {
    pub size: u32,
    /// Its metadata address: bytes are compared only at the same one.
    pub address: u32,
    pub hash: Sha256,
}

/// What [`features`] compares of one map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Census {
    /// Every tag with a payload, by group (`vehi`) and name.
    pub tags: BTreeMap<(String, String), TagPrint>,
    /// Each bitmap's local pixel data, hashed, by name.
    pub pixels: BTreeMap<String, Sha256>,
    /// What the scenario places, by feature: each placement's tag name and
    /// position (tenths of a world unit), sorted.
    pub placed: BTreeMap<&'static str, Vec<(String, [i32; 3])>>,
    /// The scenario's scripts, hashed; None when there are none to read.
    pub scripts: Option<Sha256>,
    /// The walked tags' fields by group and name
    /// ([`crate::live_mod_contents::walk_tag`]).
    pub fields: BTreeMap<(String, String), crate::live_mod_contents::Fields>,
    /// The scenario's references and block counts (kept apart: a mod's
    /// scenario may be named otherwise than its base's).
    pub scenario: crate::live_mod_contents::Fields,
}

/// The feature a tag group counts for.
pub fn feature_of_group(group: &str) -> Option<&'static str> {
    Some(match group {
        "vehi" | "vehc" => "vehicles",
        "weap" | "itmc" => "weapons",
        "scen" | "eqip" | "ctrl" | "lifi" | "ssce" | "bloc" | "mach" => "objects",
        "bitm" => "textures",
        "shad" | "stem" => "shaders",
        "sbsp" => "geometry",
        "char" => "ai",
        _ => return None,
    })
}

/// The features `modded` has over `base`, in `FEATURES` order.
pub fn features(base: &Census, modded: &Census) -> Vec<String> {
    let mut found: Vec<&'static str> = Vec::new();
    for ((group, name), t) in &modded.tags {
        let Some(f) = feature_of_group(group) else {
            continue;
        };
        let changed = match base.tags.get(&(group.clone(), name.clone())) {
            None => true,
            Some(b) => b.size != t.size || (b.address == t.address && b.hash != t.hash),
        };
        if changed {
            found.push(f);
        }
    }
    if modded
        .pixels
        .iter()
        .any(|(name, h)| base.pixels.get(name).is_some_and(|b| b != h))
    {
        found.push("textures");
    }
    for (f, placed) in &modded.placed {
        if base.placed.get(f) != Some(placed) {
            found.push(*f);
        }
    }
    for (f, placed) in &base.placed {
        if !placed.is_empty() && !modded.placed.contains_key(f) {
            found.push(*f);
        }
    }
    if modded.scripts != base.scripts {
        found.push("scripts");
    }
    live_proto::maps::in_order(&found, &live_proto::maps::FEATURES)
}

/// The features of `modded` over `base`, both whole maps.
pub fn derive(base: &[u8], modded: &[u8]) -> Result<Vec<String>> {
    let b = census(&mut Map::from_reader(Cursor::new(base)).context("reading the base map")?)?;
    let m = census(&mut Map::from_reader(Cursor::new(modded)).context("reading the mod")?)?;
    Ok(features(&b, &m))
}

fn group_name(g: FourCC) -> String {
    String::from_utf8_lossy(&g.0).trim().to_string()
}

/// A bitmap tag's images block (count, address) and an image's stride.
const BITMAP_IMAGES: usize = 0x44;
const BITMAP_ROOT: usize = 0x4c;
const IMAGE_STRIDE: usize = 0x74;
/// An image's LOD 0 raw pointer and size (docs: `tags/src/defs/bitm.txt`).
const IMAGE_LOD0: usize = 0x1c;
const IMAGE_LOD0_SIZE: usize = 0x34;
/// The most pixel bytes one image is read for.
const MAX_LOD: u32 = 32 << 20;

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// One map's [`Census`].
pub fn census<R: Read + Seek>(map: &mut Map<R>) -> Result<Census> {
    let mut c = Census::default();
    let tags: Vec<_> = map.tags().to_vec();
    let scenario = map.scenario().map(|t| t.id);
    let names: crate::live_mod_contents::Names = tags
        .iter()
        .filter_map(|t| Some((t.id.0, (group_name(t.group), t.name.clone()?))))
        .collect();
    for t in &tags {
        let (Some(loc), Some(name)) = (t.location, t.name.clone()) else {
            continue;
        };
        let group = group_name(t.group);
        if Some(t.id) == scenario {
            if let Ok(bytes) = map.read_tag_bytes(t.id)
                && let Some(f) =
                    crate::live_mod_contents::walk_tag(t.group, "scnr", &bytes, loc.address, &names)
            {
                c.scenario = f;
            }
            continue;
        }
        let Ok(bytes) = map.read_tag_bytes(t.id) else {
            continue;
        };
        if let Some(f) =
            crate::live_mod_contents::walk_tag(t.group, &group, &bytes, loc.address, &names)
        {
            c.fields.insert((group.clone(), name.clone()), f);
        }
        if group == "bitm"
            && let Some(h) = pixels(map, &bytes, loc.address)
        {
            c.pixels.insert(name.clone(), h);
        }
        c.tags.insert(
            (group, name),
            TagPrint {
                size: loc.size,
                address: loc.address,
                hash: Sha256::of(&bytes),
            },
        );
    }
    c.placed = placements(map).unwrap_or_default();
    c.scripts = hs::Scenario::read(map)
        .ok()
        .map(|s| Sha256::of(format!("{s:?}").as_bytes()));
    Ok(c)
}

/// A bitmap's images' local LOD 0 bytes, hashed in order; None when it
/// has none in this map.
fn pixels<R: Read + Seek>(map: &mut Map<R>, bytes: &[u8], address: u32) -> Option<Sha256> {
    let view = TagView::new(bytes, address);
    let root = view.at(0, BITMAP_ROOT).ok()?;
    let mut all = Vec::new();
    for image in view.block(root, BITMAP_IMAGES, IMAGE_STRIDE).ok()? {
        let (Some(raw), Some(size)) = (u32_at(image, IMAGE_LOD0), u32_at(image, IMAGE_LOD0_SIZE))
        else {
            continue;
        };
        let Some(p) = map::RawPointer::decode(raw) else {
            continue;
        };
        if p.file != map::ResourceFile::Local || size == 0 || size > MAX_LOD {
            continue;
        }
        if let Ok(px) = map.read_file_range(u64::from(p.offset), u64::from(size)) {
            all.extend_from_slice(Sha256::of(&px).0.as_slice());
        }
    }
    (!all.is_empty()).then(|| Sha256::of(&all))
}

/// Tenths of a unit: a placement nudged less than that has not moved.
fn spot(p: [f32; 3]) -> [i32; 3] {
    p.map(|v| (v * 10.0).round() as i32)
}

/// The scenario's placements by feature.
fn placements<R: Read + Seek>(
    map: &mut Map<R>,
) -> Result<BTreeMap<&'static str, Vec<(String, [i32; 3])>>> {
    let scenario = map.scenario().context("no scenario")?.clone();
    let location = scenario.location.context("scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    let root = view.at(0, 0x330 + 8)?;
    let name_of = |map: &Map<R>, id: TagId| {
        map.tag(id)
            .ok()
            .map(|t| t.name.clone().unwrap_or_else(|| format!("{:08x}", id.0)))
    };
    let mut out: BTreeMap<&'static str, Vec<(String, [i32; 3])>> = BTreeMap::new();
    for block in BLOCKS {
        let feature = feature_of_group(block.kind).unwrap_or("objects");
        let palette: Vec<TagId> = view
            .block(root, block.offset + 8, crate::live_scenario::PALETTE_STRIDE)?
            .into_iter()
            .map(|e| TagId(u32::from_le_bytes([e[4], e[5], e[6], e[7]])))
            .collect();
        for rec in view.block(root, block.offset, block.stride)? {
            let Some(at) = Placement::parse(rec) else {
                continue;
            };
            let Some(&id) = usize::try_from(at.palette)
                .ok()
                .and_then(|p| palette.get(p))
            else {
                continue;
            };
            if let Some(name) = name_of(map, id) {
                out.entry(feature)
                    .or_default()
                    .push((name, spot(at.position)));
            }
        }
    }
    for rec in view.block(root, NETGAME_EQUIPMENT, NETGAME_EQUIPMENT_STRIDE)? {
        let Some(e) = NetgameEntry::parse(rec) else {
            continue;
        };
        if e.item == TagId::NULL {
            continue;
        }
        // The item's group as the tag index has it (the record's own copy
        // is stored byte-reversed).
        let feature = map
            .tag(e.item)
            .ok()
            .and_then(|t| feature_of_group(&group_name(t.group)))
            .unwrap_or("objects");
        if let Some(name) = name_of(map, e.item) {
            out.entry(feature)
                .or_default()
                .push((name, spot(e.at.position)));
        }
    }
    for v in out.values_mut() {
        v.sort();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(group: &str, name: &str) -> (String, String) {
        (group.to_string(), name.to_string())
    }

    fn tag(size: u32, address: u32, fill: u8) -> TagPrint {
        TagPrint {
            size,
            address,
            hash: Sha256([fill; 32]),
        }
    }

    /// An invented level: one vehicle, one weapon, one bitmap, scripts.
    fn base() -> Census {
        let mut c = Census::default();
        for (g, n, t) in [
            ("vehi", "objects\\v\\hog", tag(0x100, 0x1000, 1)),
            ("weap", "objects\\w\\gun", tag(0x80, 0x2000, 2)),
            ("bitm", "scenery\\b\\rock", tag(0x40, 0x3000, 3)),
            ("shad", "scenery\\s\\rock", tag(0x40, 0x4000, 4)),
            ("sbsp", "levels\\x\\x", tag(0x9000, 0x5000, 5)),
            ("matg", "globals\\globals", tag(0x10, 0x6000, 6)),
        ] {
            c.tags.insert((g.into(), n.into()), t);
        }
        c.pixels.insert("scenery\\b\\rock".into(), Sha256([7; 32]));
        c.placed
            .insert("vehicles", vec![("objects\\v\\hog".into(), [0, 0, 10])]);
        c.placed
            .insert("weapons", vec![("objects\\w\\gun".into(), [5, 5, 0])]);
        c.scripts = Some(Sha256([8; 32]));
        c
    }

    #[test]
    fn the_same_map_changes_nothing() {
        assert!(features(&base(), &base()).is_empty());
    }

    #[test]
    fn each_kind_of_change_is_its_feature() {
        let b = base();
        let with = |f: &dyn Fn(&mut Census)| {
            let mut m = base();
            f(&mut m);
            features(&b, &m)
        };
        assert_eq!(
            with(&|m| {
                m.placed
                    .get_mut("vehicles")
                    .unwrap()
                    .push(("objects\\v\\hog".into(), [90, 0, 10]))
            }),
            ["vehicles"],
            "a second hog"
        );
        assert_eq!(
            with(&|m| m.placed.get_mut("weapons").unwrap()[0].1 = [6, 5, 0]),
            ["weapons"],
            "a gun moved"
        );
        assert_eq!(
            with(&|m| {
                m.placed
                    .insert("objects", vec![("scenery\\crate".into(), [0, 0, 0])]);
            }),
            ["objects"]
        );
        assert_eq!(
            with(&|m| {
                m.placed.remove("weapons");
            }),
            ["weapons"],
            "every gun taken out"
        );
        assert_eq!(
            with(&|m| {
                m.pixels.insert("scenery\\b\\rock".into(), Sha256([9; 32]));
            }),
            ["textures"],
            "repainted pixels, the tag unchanged"
        );
        assert_eq!(
            with(&|m| {
                m.tags.insert(
                    ("bitm".into(), "scenery\\b\\new".into()),
                    tag(0x40, 0x7000, 1),
                );
            }),
            ["textures"],
            "a new bitmap"
        );
        assert_eq!(
            with(&|m| m
                .tags
                .get_mut(&key("shad", "scenery\\s\\rock"))
                .unwrap()
                .hash = Sha256([0; 32])),
            ["shaders"],
            "a shader's bytes at its own address"
        );
        assert_eq!(
            with(&|m| m.tags.get_mut(&key("sbsp", "levels\\x\\x")).unwrap().size = 0xa000),
            ["geometry"]
        );
        assert_eq!(
            with(&|m| {
                m.tags
                    .insert(("char".into(), "ai\\elite".into()), tag(0x10, 0x8000, 1));
            }),
            ["ai"]
        );
        assert_eq!(with(&|m| m.scripts = Some(Sha256([1; 32]))), ["scripts"]);
        assert!(
            with(&|m| m
                .tags
                .get_mut(&key("matg", "globals\\globals"))
                .unwrap()
                .size = 1)
            .is_empty(),
            "globals count for nothing"
        );
    }

    #[test]
    fn a_tag_that_moved_is_not_compared_by_bytes() {
        // A grown map: every later tag's address moves and its pointers
        // with it, so its bytes differ though nothing in it changed.
        let mut m = base();
        let t = m.tags.get_mut(&key("shad", "scenery\\s\\rock")).unwrap();
        t.address += 0x100;
        t.hash = Sha256([0x42; 32]);
        assert!(features(&base(), &m).is_empty());
    }

    #[test]
    fn many_changes_come_in_the_checklists_order() {
        let mut m = base();
        m.scripts = None;
        m.placed.get_mut("vehicles").unwrap().clear();
        m.pixels.insert("scenery\\b\\rock".into(), Sha256([0; 32]));
        assert_eq!(features(&base(), &m), ["vehicles", "textures", "scripts"]);
    }

    #[test]
    fn groups_map_to_features() {
        assert_eq!(feature_of_group("vehc"), Some("vehicles"));
        assert_eq!(feature_of_group("ssce"), Some("objects"));
        assert_eq!(feature_of_group("stem"), Some("shaders"));
        assert_eq!(feature_of_group("scnr"), None);
        for b in BLOCKS {
            assert!(feature_of_group(b.kind).is_some(), "{}", b.kind);
        }
    }
}
