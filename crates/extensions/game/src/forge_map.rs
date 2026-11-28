//! The local Objects tool opens the active cache slot on a worker. Only
//! typed palette references and the base identity enter the Forge document.
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use anyhow::{Context, ensure};
use bots::los::Walls;
use guest_layout::GuestMemory;
use forge::{BlockKind, PaletteItem, Session, Teleporter, tool::Extent};
use forge_doc::{BuilderVersions, ForgeDoc, Limits, MapRef, Sha256, TagKey, WorldBounds};
use guest_layout::{
    game::{GameGlobals, MODE_MULTIPLAYER, cache_checksum},
    global,
};
use map::{CacheType, FourCC, Map, TagId, scenario_layout};
use tags::{collision::Bsp, geometry::TagView, structure::StructureRender};

use crate::{layout_glue::ReadGm, map_slots};

/// Replay is a host property and is checked separately by the installer.
/// The active game session (including the matchmaking target) must be offline.
pub(crate) fn admitted(m: &dyn GuestMemory) -> bool {
    use bots::lobby;
    let offline = || -> Option<bool> {
        if m.u8(lobby::NETWORKING).ok()? == 0 {
            return Some(true);
        }
        let cycle = m.u32(lobby::SESSION_TYPE).ok()?;
        // Matchmaking uses the target session through its game and postgame
        // phases. A missing target during that transition still refuses Forge.
        if cycle > 5 {
            return Some(false);
        }
        let address = m.u32(lobby::game_session_at(cycle)).ok()?;
        Some(
            address == 0
                || m.u32(address.checked_add(lobby::STATE)?).ok()? == 0
                || m.u32(address.checked_add(crate::variant_ext::SESSION_NETWORK_CLASS)?)
                    .ok()?
                    == 0,
        )
    };
    GameGlobals::current(m).ok().flatten().is_some_and(|g| {
        g.map_active(m) == Ok(1)
            && g.game_ended(m) == Ok(0)
            && g.options().mode(m) == Ok(MODE_MULTIPLAYER)
    }) && crate::coop_probe::read_sim(&ReadGm(m)).is_some_and(|sim| {
        use agent_proto::sim::SessionKind;
        let kind = SessionKind::from_sim_mode(sim.mode);
        (kind == SessionKind::Local && offline() == Some(true))
            || (matches!(kind, SessionKind::Local | SessionKind::MpHost)
                && crate::lobby_start::mode() == crate::lobby_start::Mode::Forge
                && crate::lobby_mode::remote_members(m) == Some(0))
    })
}

/// A synchronous network game (co-op host or guest) on a multiplayer map,
/// in which a shared Forge session may run
/// (docs/game/forge-design.md, F5).
pub(crate) fn admitted_shared(m: &dyn GuestMemory) -> bool {
    GameGlobals::current(m).ok().flatten().is_some_and(|g| {
        g.map_active(m) == Ok(1)
            && g.game_ended(m) == Ok(0)
            && g.options().mode(m) == Ok(MODE_MULTIPLAYER)
    }) && crate::coop_probe::read_sim(&ReadGm(m)).is_some_and(|sim| {
        matches!(
            agent_proto::sim::SessionKind::from_sim_mode(sim.mode),
            agent_proto::sim::SessionKind::CoopHost | agent_proto::sim::SessionKind::CoopGuest
        )
    })
}

/// Where Forge may run: an offline local game, or a synchronous network
/// game whose link has a Forge channel waiting
/// ([`crate::forge_glue::net::pending`], re-offered each tick).
pub(crate) fn admitted_session(m: &dyn GuestMemory) -> bool {
    admitted(m) || (admitted_shared(m) && crate::forge_glue::net::pending())
}

#[derive(Clone, Debug)]
pub(crate) struct Loaded {
    pub scenario: u32,
    pub name: String,
    pub checksum: u32,
    path: String,
    base_name: String,
    author: String,
    free_slots: usize,
}

impl Loaded {
    pub(crate) fn read(m: &dyn GuestMemory, replaying: bool) -> Result<Self, String> {
        if replaying {
            return Err("Forge is unavailable during Theater playback.".into());
        }
        if !admitted_session(m) {
            return Err("Forge requires an active, offline local multiplayer game.".into());
        }
        let slot = map_slots::current_source(&ReadGm(m))
            .ok_or("The active map has no readable cache slot.")?;
        let path = map_slots::recorded_file(&slot).ok_or("The active map source is unavailable.")?;
        let name = crate::health::loaded_map_name(m)
            .map_err(|_| "The loaded cache name is unavailable.")?;
        let base_name = slot
            .rsplit('\\')
            .next()
            .and_then(|file| file.get(..file.len().checked_sub(4)?))
            .ok_or("The active cache has no map filename.")?
            .to_ascii_lowercase();
        let pool = agent::forge::object_pool(m).map_err(|error| error.message)?;
        Ok(Self {
            scenario: global::SCENARIO
                .pointer(m)
                .ok()
                .flatten()
                .ok_or("The loaded scenario is unavailable.")?,
            checksum: cache_checksum(m).map_err(|_| "The loaded cache checksum is unavailable.")?,
            name,
            path,
            base_name,
            author: crate::skulls::profile_or_default(&ReadGm(m), 0),
            free_slots: (pool.limit - pool.used) as usize,
        })
    }

    pub(crate) fn matches(&self, m: &dyn GuestMemory) -> bool {
        admitted_session(m)
            && global::SCENARIO.pointer(m) == Ok(Some(self.scenario))
            && cache_checksum(m) == Ok(self.checksum)
    }
}

pub(crate) struct Opened {
    pub session: Session,
    /// Salted definition tag datums in exactly the session's palette order.
    pub tags: Vec<u32>,
    /// Each palette item's bounding sphere, in the same order.
    pub extents: Vec<Extent>,
    /// The scenario palette entries left out of the Objects palette, each
    /// as `path: reason`.
    pub refused: Vec<String>,
    /// Structure collision indexed by the game's active BSP.
    pub walls: Vec<Walls>,
    /// Every named tag of the map by its salted datum, which
    /// [`rows::read`] resolves the loaded scenario's references through.
    pub names: BTreeMap<u32, TagKey>,
    /// The Forge bodies the map's kit carries, by unit tag datum, and the
    /// reasons its Forge record gives for the rest ([`kit_bodies`]).
    pub bodies: forge::body::Kit,
    /// The paths of the map's item collections (`itmc`) that hold a weapon:
    /// the spawners that show Spare clips ([`armed_collections`]).
    pub armed: std::collections::BTreeSet<String>,
}

/// Read the selected slot's logical bytes, including package layers and
/// storage padding. Hash and decode the snapshot outside the foreground VFS lock.
pub(crate) fn open(vfs: &Arc<Mutex<vfs::Vfs>>, loaded: &Loaded) -> Result<Opened, String> {
    open_checked(vfs, loaded).map_err(|e| e.to_string())
}

fn open_checked(vfs: &Arc<Mutex<vfs::Vfs>>, loaded: &Loaded) -> anyhow::Result<Opened> {
    // A materialised editing kit retains the original document's base.
    // Other installed maps are bases in their own right, including two
    // packages with the same disc basename and different logical bytes.
    let package = {
        let mut mounted = vfs.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(mounted.map_ranges(&loaded.path)?.is_none(), "Forge needs the complete map.");
        mounted.map_id(&loaded.path)
    };
    let base = if package.is_some() && package == crate::map_variant::kit_start() {
        Some(crate::map_variant::installed(&loaded.base_name)
            .context("The installed map the kit was built on is unknown.")?)
    } else {
        None
    };
    let mut file = crate::bots_nav_auto::VfsFile::open(Arc::clone(vfs), &loaded.path)?;
    let len = file.seek(SeekFrom::End(0))?;
    ensure!(len <= map::writer::cache_size_limit(CacheType::Multiplayer)?,
        "The map exceeds the multiplayer cache size limit.");
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![0; usize::try_from(len)?];
    file.read_exact(&mut bytes)?;
    decode(&bytes, loaded, base)
}

/// `base` is the hash of the installed map a kit package was built on;
/// the original map is its own base.
fn decode(bytes: &[u8], loaded: &Loaded, base: Option<Sha256>) -> anyhow::Result<Opened> {
    let mut map = Map::from_reader(Cursor::new(bytes))?;
    let header = map.header();
    ensure!(
        header.cache_type == CacheType::Multiplayer,
        "The loaded cache is not a multiplayer map."
    );
    ensure!(
        header.name.eq_ignore_ascii_case(&loaded.name) && header.checksum == loaded.checksum,
        "The active cache and its source file differ. Reload the map."
    );
    map.validate_checksum()?;
    let scnr = map.scenario().context("The map has no scenario.")?;
    let address = scnr
        .location
        .context("The scenario has no payload.")?
        .address;
    let root = map.read_tag_bytes(scnr.id)?;
    let view = TagView::new(&root, address);
    let mut base_counts = BTreeMap::new();
    let mut rows = BTreeMap::new();
    let mut refused = Vec::new();
    for def in scenario_layout::BLOCKS {
        let count = u32::from_le_bytes(view.at(def.offset as usize, 4)?.try_into()?);
        base_counts.insert(def.kind, count);
        let Some(palette) = &def.palette else {
            continue;
        };
        for entry in view.block(&root, palette.offset as usize, palette.stride as usize)? {
            let group = FourCC::from_u32(u32::from_le_bytes(entry[..4].try_into()?));
            let id = TagId(u32::from_le_bytes(entry[4..8].try_into()?));
            if id == TagId::NULL || !palette.groups.contains(&group.0) {
                continue;
            }
            // A palette row naming a tag the map does not hold, or one of
            // another group, is the map's own loose end: left out by name.
            let Ok(tag) = map.tag(id) else {
                refused.push(format!("tag {:#010x}: the map has no such tag", id.0));
                continue;
            };
            let Some(path) = tag.name.clone().filter(|p| !p.is_empty()) else {
                continue;
            };
            if tag.group != group {
                refused.push(format!("{path}: it is not a {group} tag"));
                continue;
            }
            if rows.contains_key(&(def.kind, group, path.clone())) {
                continue;
            }
            let extent = map
                .read_tag_bytes(id)
                .map_err(|e| format!("its definition does not read ({e})"))
                .and_then(|bytes| placeable(def.kind, group, &bytes))
                .and_then(|(extent, model)| match map.tag(model) {
                    Ok(m) if m.group == FourCC(*b"hlmt") => Ok(extent),
                    _ => Err("its model is not in the map, so it would be invisible".into()),
                });
            match extent {
                Ok(extent) => {
                    rows.insert((def.kind, group, path), (id.0, extent));
                }
                Err(reason) => refused.push(format!("{path}: {reason}")),
            }
        }
    }
    for line in &refused {
        eprintln!("forge: the Objects palette leaves out {line}");
    }
    let mut palette = Vec::with_capacity(rows.len());
    let mut tags = Vec::with_capacity(rows.len());
    let mut extents = Vec::with_capacity(rows.len());
    for ((block, group, path), (datum, extent)) in rows {
        palette.push(PaletteItem {
            block,
            tag: TagKey {
                group: group.to_string(),
                path,
            },
        });
        tags.push(datum);
        extents.push(extent);
    }
    let teleporters = teleporters(&view, &root)?;
    let room = kit_room(&mut map, &view)?;
    let names = map
        .tags()
        .iter()
        .filter_map(|tag| {
            let path = tag.name.clone().filter(|p| !p.is_empty())?;
            Some((
                tag.id.0,
                TagKey {
                    group: tag.group.to_string(),
                    path,
                },
            ))
        })
        .collect();
    let bodies = kit_bodies(&names, bytes);
    let (bounds, walls) = geometry(&mut map, &view)?;
    let armed = armed_collections(&mut map);
    let doc = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: loaded.base_name.clone(),
            sha256: base.unwrap_or_else(|| Sha256::of(bytes)),
        },
        title: loaded.name.clone(),
        author: loaded.author.clone(),
        description: String::new(),
        ops: vec![],
        recipes: vec![],
        sources: vec![],
        imports: vec![],
        expect: None,
        waves: None,
    };
    let limits = Limits {
        world: Some(bounds),
        builder: Some(doc.builder),
        base_counts,
        ..Default::default()
    };
    let session = Session::new(doc, limits, palette, loaded.free_slots)
        .and_then(|s| s.with_teleporters(teleporters))
        .map_err(anyhow::Error::msg)?;
    // A map with spare rows past its blocks' counts has the kit.
    let session = if room.values().any(|&n| n > 0) {
        session.with_kit(room).map_err(anyhow::Error::msg)?
    } else {
        session
    };
    Ok(Opened {
        session,
        tags,
        extents,
        refused,
        walls,
        names,
        bodies,
        armed,
    })
}

/// The Forge bodies of a map (docs/game/forge-design-slices.md, "The
/// monitor body"): each body whose unit tag (`forge_doc::bodies::SPECS`)
/// is among `names`, by datum, and the reasons the map's Forge record
/// gives for those its kit build left out. A map with no record gives no
/// reasons: its bodies are refused as a map not built with the kit.
pub(crate) fn kit_bodies(names: &BTreeMap<u32, TagKey>, bytes: &[u8]) -> forge::body::Kit {
    use forge_doc::bodies::Body;
    let carried = Body::ALL
        .into_iter()
        .filter_map(|body| {
            let unit = body.spec().unit;
            let group = String::from_utf8_lossy(&unit.group).into_owned();
            names
                .iter()
                .find(|(_, key)| key.group == group && key.path.eq_ignore_ascii_case(unit.path))
                .map(|(&datum, _)| (body, datum))
        })
        .collect();
    let refused = forge_doc::record::ForgeRecord::read_tail(&mut Cursor::new(bytes))
        .ok()
        .flatten()
        .map(|r| r.bodies_refused)
        .unwrap_or_default();
    forge::body::Kit { carried, refused }
}

/// An item collection's entries (`itmc+0x00`, a block of `0x10`-byte
/// elements): the weight `+0x00`, the item's group `+0x04` and datum `+0x08`,
/// the variant `+0x0c` (E: `collection_select` `0x19f1a0` steps `0x10`,
/// reads the weight at `+0` and returns `+8`).
const COLLECTION_ENTRIES: usize = 0x0;
const COLLECTION_ENTRY_LEN: usize = 0x10;
const COLLECTION_ENTRY_GROUP: usize = 0x4;

/// Whether the item collection whose payload is `root` at `address` names
/// a weapon (`weap`) among its entries. A mixed collection does.
pub(crate) fn holds_weapon(root: &[u8], address: u32) -> anyhow::Result<bool> {
    let view = TagView::new(root, address);
    Ok(view
        .block(root, COLLECTION_ENTRIES, COLLECTION_ENTRY_LEN)?
        .iter()
        .any(|e| {
            e.get(COLLECTION_ENTRY_GROUP..COLLECTION_ENTRY_GROUP + 4)
                .is_some_and(|g| FourCC::from_u32(u32::from_le_bytes(g.try_into().unwrap())) == FourCC(*b"weap"))
        }))
}

/// The paths of the map's item collections that hold a weapon. A
/// collection that does not read is left out, so its spawners show no Spare
/// clips row.
fn armed_collections<R: Read + Seek>(map: &mut Map<R>) -> std::collections::BTreeSet<String> {
    let collections: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"itmc"))
        .filter_map(|t| Some((t.id, t.name.clone()?, t.location.as_ref()?.address)))
        .collect();
    collections
        .into_iter()
        .filter(|(id, _, address)| {
            map.read_tag_bytes(*id)
                .ok()
                .and_then(|root| holds_weapon(&root, *address).ok())
                .unwrap_or(false)
        })
        .map(|(_, path, _)| path)
        .collect()
}

/// The spare rows the kit's blocks have past their counts: the rows after
/// each count that read in the kit's spare form (`forge::kit::room`).
fn kit_room<R: Read + Seek>(
    map: &mut Map<R>,
    view: &TagView<'_>,
) -> anyhow::Result<BTreeMap<BlockKind, u32>> {
    let mut room = BTreeMap::new();
    for block in forge::kit::BLOCKS {
        let def = scenario_layout::block(block);
        let pair = view.at(def.offset as usize, 8)?;
        let count = u32::from_le_bytes(pair[..4].try_into()?);
        let address = u32::from_le_bytes(pair[4..].try_into()?);
        let spare = forge::kit::room(block, count, |n| {
            let at = address.checked_add(n.checked_mul(def.stride)?)?;
            let offset = map.file_offset(at, def.stride).ok()?;
            map.read_file_range(offset, def.stride.into()).ok()
        });
        room.insert(block, spare);
    }
    Ok(room)
}

/// The object definition header (`obje`, docs/formats/objects.md): its
/// type, bounding sphere and model reference.
const OBJECT_TYPE: usize = 0x0;
const BOUNDING_RADIUS: usize = 0x4;
const BOUNDING_OFFSET: usize = 0x8;
const MODEL_DATUM: usize = 0x38;
/// The bounding radius past which a definition is not taken as an object to
/// carry, in world units.
const MAX_RADIUS: f32 = 1024.0;

/// Whether a palette entry of `block` and `group` is an object the
/// Objects tool can place and show, from its definition's bytes `def`: its
/// bounding sphere and model datum, or why not in plain words.
fn placeable(block: BlockKind, group: FourCC, def: &[u8]) -> Result<(Extent, TagId), String> {
    if block == BlockKind::SoundScenery {
        return Err("sound scenery has no body to place or see".into());
    }
    let u32_at = |at: usize| {
        def.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")))
            .ok_or_else(|| "its definition is too short".to_string())
    };
    let f32_at = |at: usize| u32_at(at).map(f32::from_bits);
    let kind = def
        .get(OBJECT_TYPE..OBJECT_TYPE + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or("its definition is too short")?;
    let expected = tags::object::OBJECT_TYPES
        .iter()
        .find(|t| t.group.as_bytes() == group.0.as_slice())
        .map(|t| t.index);
    if expected != Some(kind) {
        return Err(format!("its definition is not a {group} object"));
    }
    let radius = f32_at(BOUNDING_RADIUS)?;
    let offset = [
        f32_at(BOUNDING_OFFSET)?,
        f32_at(BOUNDING_OFFSET + 4)?,
        f32_at(BOUNDING_OFFSET + 8)?,
    ];
    if !(radius.is_finite() && (0.0..=MAX_RADIUS).contains(&radius))
        || !offset
            .iter()
            .all(|v| v.is_finite() && v.abs() <= MAX_RADIUS)
    {
        return Err("its bounding sphere is not a usable size".into());
    }
    let model = TagId(u32_at(MODEL_DATUM)?);
    if model == TagId::NULL {
        return Err("it has no model, so it would be invisible".into());
    }
    Ok((Extent { radius, offset }, model))
}

/// The scenario's teleporter flags for the Links tool: netgame flags of
/// type 7 (sender) or 8 (receiver) at a finite position, with their
/// identifier as the channel (docs/game/bots.md 3.2).
fn teleporters(view: &TagView<'_>, root: &[u8]) -> anyhow::Result<Vec<Teleporter>> {
    let def = scenario_layout::block(forge::BlockKind::NetgameFlags);
    let at = |name: &str| {
        def.field(name)
            .map(|f| f.offset as usize)
            .with_context(|| format!("netgame flags have no {name}"))
    };
    let (kind, identifier, position) = (at("type")?, at("identifier")?, at("position")?);
    let mut out = Vec::new();
    for (index, flag) in view
        .block(root, def.offset as usize, def.stride as usize)?
        .into_iter()
        .enumerate()
    {
        let short = |o: usize| [flag[o], flag[o + 1]];
        let sender = match u16::from_le_bytes(short(kind)) {
            7 => true,
            8 => false,
            _ => continue,
        };
        let position: [f32; 3] = std::array::from_fn(|k| {
            let o = position + 4 * k;
            f32::from_le_bytes([flag[o], flag[o + 1], flag[o + 2], flag[o + 3]])
        });
        if position.iter().all(|v| v.is_finite()) {
            out.push(Teleporter {
                index: u32::try_from(index)?,
                position,
                sender,
                channel: i16::from_le_bytes(short(identifier)),
            });
        }
    }
    Ok(out)
}

fn geometry<R: Read + Seek>(
    map: &mut Map<R>,
    scenario: &TagView<'_>,
) -> anyhow::Result<(WorldBounds, Vec<Walls>)> {
    let mut bounds = WorldBounds {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    let mut walls = Vec::new();
    for bsp in tags::structure::scenario_bsp_ids(scenario)? {
        let addr = map
            .tag(bsp)?
            .location
            .context("A structure BSP has no payload.")?
            .address;
        let bytes = map.read_tag_bytes(bsp)?;
        let view = TagView::new(&bytes, addr);
        for cluster in StructureRender::decode(bsp, &view)?.clusters {
            for i in 0..3 {
                bounds.min[i] = bounds.min[i].min(cluster.bounds[2 * i]);
                bounds.max[i] = bounds.max[i].max(cluster.bounds[2 * i + 1]);
            }
        }
        let view = TagView::new(&bytes, addr);
        let mut polygons = Vec::new();
        for child in view.block(
            view.bytes(),
            tags::pathfinding::SBSP_COLLISION_FIELD,
            Bsp::STRUCTURE_SIZE,
        )? {
            let offset = child.as_ptr() as usize - bytes.as_ptr() as usize;
            let collision = Bsp::decode_structure(&bytes, addr, offset)?;
            for i in 0..collision.surfaces.len() {
                let ring = collision.surface_ring(i);
                let points: Vec<_> = ring
                    .into_iter()
                    .map(|v| {
                        collision
                            .vertices
                            .get(v)
                            .map(|v| v.point)
                            .context("A collision surface has an invalid vertex.")
                    })
                    .collect::<anyhow::Result<_>>()?;
                if points.len() >= 3 {
                    polygons.push(points);
                }
            }
        }
        walls.push(Walls::from_polygons(polygons.iter().map(Vec::as_slice)));
    }
    ensure!(
        bounds.min.iter().all(|x| x.is_finite()),
        "The map has no BSP world bounds."
    );
    Ok((bounds, walls))
}

/// The loaded scenario's rows, typed fields and prints. The session reads
/// them once when it opens (docs/game/forge-map-variants.md, "Applying an
/// edit in a session"), and the Things list is built from them.
#[allow(dead_code)]
pub(crate) mod rows;

#[cfg(test)]
mod rows_tests;
#[cfg(test)]
mod tests;
