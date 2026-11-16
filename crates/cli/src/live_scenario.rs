//! The objects a scenario places, for `map.glb`'s `scenario` node: scenery,
//! crates, vehicles, weapons, equipment, controls, light fixtures and sound
//! scenery from their placement blocks, and the multiplayer weapon and
//! vehicle spawns from the netgame equipment block (docs/live-art.md,
//! "Scenario objects"). Machines have their own node (`machines`). Each
//! object's render model is read once per level and shared by its
//! placements.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, anyhow};
use assets::ResourceFiles;
use map::{FourCC, TagId};
use tags::geometry::TagView;

use crate::live_geometry::{MachineModel, MapReader, NothingDrawn, Placement, object_model};
use crate::live_looks::Looks;

/// A placement block of the scenario: the object type table's columns
/// (docs/objects.md): the kind (tag group) it places, its offset in `scnr`
/// and its record stride. The palette follows 8 bytes on, 0x28-byte
/// entries, tag reference first. Machines (`mach`) are not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: &'static str,
    pub offset: usize,
    pub stride: usize,
}

pub const BLOCKS: [Block; 8] = [
    Block {
        kind: "scen",
        offset: 0x50,
        stride: 0x5c,
    },
    Block {
        kind: "vehi",
        offset: 0x70,
        stride: 0x54,
    },
    Block {
        kind: "eqip",
        offset: 0x80,
        stride: 0x38,
    },
    Block {
        kind: "weap",
        offset: 0x90,
        stride: 0x54,
    },
    Block {
        kind: "ctrl",
        offset: 0xb8,
        stride: 0x44,
    },
    Block {
        kind: "lifi",
        offset: 0xc8,
        stride: 0x54,
    },
    Block {
        kind: "ssce",
        offset: 0xd8,
        stride: 0x50,
    },
    Block {
        kind: "bloc",
        offset: 0x328,
        stride: 0x4c,
    },
];
const PALETTE_STRIDE: usize = 0x28;

/// The netgame equipment block (`scnr+0x120`, 0x90-byte entries;
/// docs/forge.md): what multiplayer spawns and respawns at run time.
/// Position at `+0x40`, yaw, pitch and roll at `+0x4c`, the item at
/// `+0x58` (group, then tag ID at `+0x5c`): a weapon, vehicle or other
/// object, or an item or vehicle collection the game rolls.
pub const NETGAME_EQUIPMENT: usize = 0x120;
pub const NETGAME_EQUIPMENT_STRIDE: usize = 0x90;
/// The kind written for netgame equipment spawns.
pub const NETGAME: &str = "netgame";

/// A placement record's flags (u32 at `+4`); bit 0 set: the object is not
/// created when the level starts, only when a script asks for it. The name
/// is the public scenario layout's (C, as `crates/scenario-edit` has it);
/// on the disc (V) it marks the cinematics' props (capital ships, a ring
/// world, drop pods stacked out of sight under a level), the weapons a
/// cutscene hands out and vehicles brought in later.
pub const PLACEMENT_FLAGS: usize = 0x4;
pub const NOT_AT_START: u32 = 1;

/// Whether a placement record's object is there when the level starts.
pub fn placed_at_start(record: &[u8]) -> bool {
    record
        .get(PLACEMENT_FLAGS..PLACEMENT_FLAGS + 4)
        .is_some_and(|b| u32::from_le_bytes(b.try_into().unwrap()) & NOT_AT_START == 0)
}

/// Where a placement record keeps the BSP it belongs to (i16; -1 none).
pub const ORIGIN_BSP: usize = 0x2c;

const ITMC: FourCC = FourCC(*b"itmc");
const VEHC: FourCC = FourCC(*b"vehc");

/// One placed object: what kind it is (its block's group, or
/// [`NETGAME`]), its index in that block, which of the level's object
/// models it shows, where, and, for netgame equipment, the group of the
/// object drawn there.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedObject {
    pub kind: &'static str,
    pub index: usize,
    pub model: usize,
    pub at: Placement,
    /// The scenario BSP the record says it belongs to, when in range.
    pub bsp: Option<usize>,
    /// Netgame equipment only: the drawn object's group (`weap`, `vehi`, ...).
    pub item: Option<String>,
}

impl PlacedObject {
    /// The node's name in `map.glb`: `<kind>_<index>`.
    pub fn name(&self) -> String {
        format!("{}_{}", self.kind, self.index)
    }

    /// The node's `extras`: kind, index and tag stem, then the placement in
    /// game coordinates as machines have it (`pos`, unit `forward` and
    /// `up`, `scale`), and `bsp` and `item` when known.
    pub fn extras(&self, stem: &str) -> serde_json::Value {
        let [forward, _, up] = self.at.axes();
        let mut e = serde_json::json!({
            "kind": self.kind,
            "index": self.index,
            "tag": stem,
            "pos": self.at.position,
            "forward": forward,
            "up": up,
            "scale": self.at.scale(),
        });
        if let Some(b) = self.bsp {
            e["bsp"] = serde_json::json!(b);
        }
        if let Some(i) = &self.item {
            e["item"] = serde_json::json!(i);
        }
        e
    }
}

/// A netgame equipment entry: where it spawns and what it names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NetgameEntry {
    pub at: Placement,
    pub group: FourCC,
    pub item: TagId,
}

impl NetgameEntry {
    pub fn parse(b: &[u8]) -> Option<NetgameEntry> {
        let f = |o: usize| Some(f32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?));
        let u = |o: usize| Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?));
        Some(NetgameEntry {
            at: Placement {
                palette: -1,
                position: [f(0x40)?, f(0x44)?, f(0x48)?],
                rotation: [f(0x4c)?, f(0x50)?, f(0x54)?],
                scale: 1.0,
            },
            group: FourCC(u(0x58)?.to_le_bytes()),
            item: TagId(u(0x5c)?),
        })
    }
}

/// The BSP index at [`ORIGIN_BSP`], when it is one of the scenario's.
pub fn origin_bsp(record: &[u8], bsps: usize) -> Option<usize> {
    let b = i16::from_le_bytes(record.get(ORIGIN_BSP..ORIGIN_BSP + 2)?.try_into().ok()?);
    usize::try_from(b).ok().filter(|&b| b < bsps)
}

/// The entry a collection is drawn as: the heaviest (the first of equals).
pub fn heaviest(entries: &[(f32, TagId)]) -> Option<TagId> {
    let mut best: Option<(f32, TagId)> = None;
    for &(w, id) in entries {
        if id == TagId::NULL {
            continue;
        }
        let w = if w.is_finite() { w } else { 0.0 };
        if best.is_none_or(|(b, _)| w > b) {
            best = Some((w, id));
        }
    }
    best.map(|(_, id)| id)
}

/// The object a netgame item names: itself, or a collection's heaviest
/// entry (collections of collections followed a few levels).
fn resolve_item(ctx: &tags::object::Context, mut id: TagId) -> Result<TagId> {
    for _ in 0..4 {
        let tag = ctx.decode(id)?;
        if tag.group != ITMC && tag.group != VEHC {
            return Ok(id);
        }
        let entries: Vec<(f32, TagId)> = tag
            .layer("collection")
            .map(|c| {
                c.elements("entries")
                    .iter()
                    .map(|e| {
                        let w = match e.get("weight") {
                            Some(tags::object::Value::F32(w)) => *w,
                            _ => 0.0,
                        };
                        (w, e.tag_ref("object").map_or(TagId::NULL, |r| r.id))
                    })
                    .collect()
            })
            .unwrap_or_default();
        id = heaviest(&entries).context("empty collection")?;
    }
    Err(anyhow!("collections nest too deep"))
}

/// The scenario's objects: models (model space, each read once) and
/// placements. A placement that cannot be drawn goes to `skipped` under
/// `map/<stem>/object_<kind>_<index>`; one whose object has no model is not
/// drawn.
pub fn load_objects(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    stem: &str,
    bsps: usize,
    skipped: &mut BTreeMap<String, String>,
    looks: &mut Looks,
) -> Result<(Vec<MachineModel>, Vec<PlacedObject>)> {
    let scenario = map.scenario().context("no scenario")?.clone();
    let location = scenario.location.context("scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    let root = view.at(0, 0x330 + 8)?;
    let ctx = tags::object::Context::read(map)?;
    let mut loaded: HashMap<TagId, Result<Option<usize>, String>> = HashMap::new();
    let mut models = Vec::new();
    let mut out = Vec::new();
    let mut model_of = |map: &mut MapReader,
                        files: &mut ResourceFiles,
                        id: TagId,
                        models: &mut Vec<MachineModel>|
     -> Result<Option<usize>, String> {
        loaded
            .entry(id)
            .or_insert_with(|| {
                if is_invisible(map.tag(id).ok().and_then(|t| t.name.as_deref())) {
                    return Ok(None);
                }
                match object_model(map, &ctx, files, id, Some(looks)) {
                    Ok(Some(m)) => {
                        models.push(m);
                        Ok(Some(models.len() - 1))
                    }
                    Ok(None) => Ok(None),
                    Err(e) if e.downcast_ref::<NothingDrawn>().is_some() => Ok(None),
                    Err(e) => Err(format!("{e:#}")),
                }
            })
            .clone()
    };
    for block in BLOCKS {
        let palette: Vec<TagId> = view
            .block(root, block.offset + 8, PALETTE_STRIDE)?
            .into_iter()
            .map(|e| TagId(u32::from_le_bytes([e[4], e[5], e[6], e[7]])))
            .collect();
        if palette.is_empty() {
            continue;
        }
        for (i, rec) in view
            .block(root, block.offset, block.stride)?
            .into_iter()
            .enumerate()
        {
            let key = format!("map/{stem}/object_{}_{i}", block.kind);
            let Some(at) = Placement::parse(rec) else {
                skipped.insert(key, "short placement record".into());
                continue;
            };
            let Some(&id) = usize::try_from(at.palette)
                .ok()
                .and_then(|p| palette.get(p))
            else {
                continue;
            };
            if id == TagId::NULL || !placed_at_start(rec) {
                continue;
            }
            if !finite(&at) {
                skipped.insert(key, "placement is not finite".into());
                continue;
            }
            match model_of(map, files, id, &mut models) {
                Ok(None) => {}
                Ok(Some(model)) => out.push(PlacedObject {
                    kind: block.kind,
                    index: i,
                    model,
                    at,
                    bsp: origin_bsp(rec, bsps),
                    item: None,
                }),
                Err(why) => {
                    skipped.insert(key, why);
                }
            }
        }
    }
    for (i, rec) in view
        .block(root, NETGAME_EQUIPMENT, NETGAME_EQUIPMENT_STRIDE)?
        .into_iter()
        .enumerate()
    {
        let key = format!("map/{stem}/object_{NETGAME}_{i}");
        let Some(e) = NetgameEntry::parse(rec) else {
            skipped.insert(key, "short netgame equipment record".into());
            continue;
        };
        if e.item == TagId::NULL {
            continue;
        }
        if !finite(&e.at) {
            skipped.insert(key, "placement is not finite".into());
            continue;
        }
        let object = match resolve_item(&ctx, e.item) {
            Ok(o) => o,
            Err(why) => {
                skipped.insert(key, format!("{why:#}"));
                continue;
            }
        };
        let group = ctx.decode(object).map(|t| t.group);
        match model_of(map, files, object, &mut models) {
            Ok(None) => {}
            Ok(Some(model)) => out.push(PlacedObject {
                kind: NETGAME,
                index: i,
                model,
                at: e.at,
                bsp: None,
                item: group
                    .ok()
                    .map(|g| String::from_utf8_lossy(&g.0).trim().to_string()),
            }),
            Err(why) => {
                skipped.insert(key, why);
            }
        }
    }
    Ok((models, out))
}

/// Whether an object's tag name says it is not seen in the game: a word
/// `invisible` in its last path component (blocking volumes and the like
/// drawn with a shader the game never shows).
pub fn is_invisible(name: Option<&str>) -> bool {
    let Some(name) = name else { return false };
    let own = name.rsplit(['\\', '/']).next().unwrap_or(name);
    own.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|w| w == "invisible")
}

fn finite(at: &Placement) -> bool {
    at.position
        .iter()
        .chain(&at.rotation)
        .all(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_the_object_type_tables_minus_machines() {
        // docs/objects.md: placement offset and stride per type.
        let kinds: Vec<&str> = BLOCKS.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            [
                "scen", "vehi", "eqip", "weap", "ctrl", "lifi", "ssce", "bloc"
            ]
        );
        assert!(!kinds.contains(&"mach"));
        let bloc = BLOCKS.iter().find(|b| b.kind == "bloc").unwrap();
        assert_eq!((bloc.offset, bloc.stride), (0x328, 0x4c));
        let weap = BLOCKS.iter().find(|b| b.kind == "weap").unwrap();
        assert_eq!((weap.offset, weap.stride), (0x90, 0x54));
    }

    #[test]
    fn netgame_entries_read_position_rotation_and_item() {
        let mut b = vec![0u8; NETGAME_EQUIPMENT_STRIDE];
        for (k, v) in [1.5f32, -2.0, 3.25, 0.5, 0.1, -0.2].iter().enumerate() {
            b[0x40 + 4 * k..0x44 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
        b[0x58..0x5c].copy_from_slice(b"cmti");
        b[0x5c..0x60].copy_from_slice(&0xe1230004u32.to_le_bytes());
        let e = NetgameEntry::parse(&b).unwrap();
        assert_eq!(e.at.position, [1.5, -2.0, 3.25]);
        assert_eq!(e.at.rotation, [0.5, 0.1, -0.2]);
        assert_eq!(e.at.scale(), 1.0);
        assert_eq!(e.group, FourCC(*b"cmti"));
        assert_eq!(e.item, TagId(0xe1230004));
        assert!(NetgameEntry::parse(&b[..0x5f]).is_none());
    }

    #[test]
    fn collections_are_drawn_as_their_heaviest_entry() {
        let (a, b, c) = (TagId(1), TagId(2), TagId(3));
        assert_eq!(heaviest(&[(1.0, a), (3.0, b), (3.0, c)]), Some(b));
        assert_eq!(heaviest(&[(0.0, a)]), Some(a));
        assert_eq!(heaviest(&[(5.0, TagId::NULL), (f32::NAN, c)]), Some(c));
        assert_eq!(heaviest(&[]), None);
    }

    #[test]
    fn objects_created_by_scripts_are_left_out() {
        let mut rec = vec![0u8; 0x5c];
        assert!(placed_at_start(&rec));
        rec[4..8].copy_from_slice(&0x100u32.to_le_bytes());
        assert!(placed_at_start(&rec), "other flags do not matter");
        rec[4..8].copy_from_slice(&0x101u32.to_le_bytes());
        assert!(!placed_at_start(&rec));
        assert!(!placed_at_start(&rec[..6]), "short record");
    }

    #[test]
    fn invisible_helpers_are_named_so() {
        assert!(is_invisible(Some(
            r"scenarios\objects\special\invisible_box1x2x2"
        )));
        assert!(is_invisible(Some("invisible")));
        assert!(
            !is_invisible(Some(r"scenarios\invisible\crate")),
            "folders do not count"
        );
        assert!(!is_invisible(Some("invisibleish_crate")));
        assert!(!is_invisible(None));
    }

    #[test]
    fn origin_bsp_is_kept_only_when_in_range() {
        let mut rec = vec![0u8; 0x5c];
        rec[ORIGIN_BSP..ORIGIN_BSP + 2].copy_from_slice(&2i16.to_le_bytes());
        assert_eq!(origin_bsp(&rec, 3), Some(2));
        assert_eq!(origin_bsp(&rec, 2), None);
        rec[ORIGIN_BSP..ORIGIN_BSP + 2].copy_from_slice(&(-1i16).to_le_bytes());
        assert_eq!(origin_bsp(&rec, 3), None);
        assert_eq!(origin_bsp(&rec[..0x2d], 3), None);
    }

    #[test]
    fn extras_carry_kind_index_tag_and_the_placement() {
        let o = PlacedObject {
            kind: "weap",
            index: 4,
            model: 0,
            at: Placement {
                palette: 0,
                position: [1.0, 2.0, 3.0],
                rotation: [std::f32::consts::FRAC_PI_2, 0.0, 0.0],
                scale: 0.0,
            },
            bsp: Some(1),
            item: None,
        };
        assert_eq!(o.name(), "weap_4");
        let e = o.extras("battle_rifle");
        assert_eq!(e["kind"], "weap");
        assert_eq!(e["index"], 4);
        assert_eq!(e["tag"], "battle_rifle");
        assert_eq!(e["pos"], serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(e["scale"], 1.0);
        assert_eq!(e["bsp"], 1);
        let f: Vec<f64> = serde_json::from_value(e["forward"].clone()).unwrap();
        assert!(
            f[0].abs() < 1e-6 && (f[1] - 1.0).abs() < 1e-6,
            "yaw 90°: forward is +y"
        );
        let u: Vec<f64> = serde_json::from_value(e["up"].clone()).unwrap();
        assert!((u[2] - 1.0).abs() < 1e-6);
        assert!(e.get("item").is_none());
    }
}
