//! A whole conversion: one Halo 1 multiplayer map onto a Halo 2 base map,
//! as the bytes of `h1_<map>.map` and its package manifest
//! (docs/halo1/multiplayer.md, "Converting a map").
//!
//! [`prepare`] does everything that needs no MOPP code and reports it;
//! [`Prepared::build`] writes the map from it with given MOPP bytes;
//! [`convert`] is both, taking the MOPP code from [`structure_mopp`].

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek};

use content::{Manifest, Origin, Sha256, Variant};
use h1map::H1Map;
use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use scenario_edit::Scenario;

use crate::apply::{Applied, apply};
use crate::structure::{Mesh, Section, Structure, Templates};
use crate::{Error, Result, collision, placements, shaders, slots, structure};

pub mod halo1;

/// At most this many vehicle collections are built (one per vehicle role
/// the base lacks a collection for).
const MAX_NEW_COLLECTIONS: usize = 16;

/// Size of a collection's (`itmc`, `vehc`) root.
pub const COLLECTION_ROOT: usize = 0xc;
/// Size of a collection entry.
pub const COLLECTION_ENTRY: usize = 0x10;

/// Which game's objects a converted map uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sandbox {
    /// Halo 1's weapons, vehicles and equipment (phase 4; not built).
    Halo1,
    /// Halo 2's nearest equivalents (docs/halo1/multiplayer.md).
    Halo2,
}

impl Sandbox {
    /// The manifest's name for it: `h1` or `h2`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Halo1 => "h1",
            Self::Halo2 => "h2",
        }
    }
}

/// The structure's Havok MOPP code (`sbsp+0x1e0`), from
/// `mopp::build_structure`: the block's bytes, one byte per element. It
/// keys structure surfaces by a 16-bit index, so a structure with more
/// surfaces than that is refused rather than written with surfaces
/// physics can't hit.
pub fn structure_mopp(bsp: &tags::collision::Bsp) -> Result<Vec<u8>> {
    if bsp.surfaces.len() > 0x1_0000 {
        return Err(Error::TooManySurfaces(bsp.surfaces.len()));
    }
    Ok(mopp::build_structure(bsp))
}

/// Halo 1's multiplayer maps: internal name, display name, and the Halo 2
/// map used as the base when none is given. The base gives the sky, the
/// item and vehicle collections and the globals. Maps with vehicles take
/// Coagulation, which has most of Halo 2's vehicle collections (warthog,
/// ghost, banshee; a missing one is built, `apply::NewCollection`).
/// Waterworks would suit the snowy ones, but Sidewinder's placements grow
/// its metadata past the engine's tag memory. The rest keep Beaver Creek's
/// sky.
pub const MAPS: [(&str, &str, &str); 19] = [
    ("beavercreek", "Battle Creek", "beavercreek"),
    ("bloodgulch", "Blood Gulch", "coagulation"),
    ("boardingaction", "Boarding Action", "beavercreek"),
    ("carousel", "Derelict", "beavercreek"),
    ("chillout", "Chill Out", "beavercreek"),
    ("damnation", "Damnation", "beavercreek"),
    ("dangercanyon", "Danger Canyon", "coagulation"),
    ("deathisland", "Death Island", "coagulation"),
    ("gephyrophobia", "Gephyrophobia", "coagulation"),
    ("hangemhigh", "Hang 'Em High", "beavercreek"),
    ("icefields", "Ice Fields", "coagulation"),
    ("infinity", "Infinity", "coagulation"),
    ("longest", "Longest", "beavercreek"),
    ("prisoner", "Prisoner", "beavercreek"),
    ("putput", "Chiron TL-34", "beavercreek"),
    ("ratrace", "Rat Race", "beavercreek"),
    ("sidewinder", "Sidewinder", "coagulation"),
    ("timberland", "Timberland", "coagulation"),
    ("wizard", "Wizard", "beavercreek"),
];

/// The display name and default base of a Halo 1 map, if it is one of
/// [`MAPS`].
pub fn known(map: &str) -> Option<(&'static str, &'static str)> {
    MAPS.iter()
        .find(|(n, _, _)| n.eq_ignore_ascii_case(map))
        .map(|&(_, d, b)| (d, b))
}

/// The converted map's name: `h1_<map>`.
pub fn map_name(h1: &str) -> String {
    format!("h1_{}", h1.to_ascii_lowercase())
}

/// The converted map's scenario path.
pub fn scenario_path(h1: &str) -> String {
    let n = map_name(h1);
    format!("scenarios\\multi\\{n}\\{n}")
}

/// A variant's map file in its package: `<sandbox>/h1_<map>.map`.
pub fn variant_file(h1: &str, sandbox: Sandbox) -> String {
    format!("{}/{}.map", sandbox.name(), map_name(h1))
}

/// The Halo 1 map's multiplayer placements, counted as the source holds
/// them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceCounts {
    /// Starting locations.
    pub spawns: usize,
    /// Netgame flags.
    pub flags: usize,
    /// Netgame equipment (item spawns).
    pub items: usize,
    /// Vehicle placements.
    pub vehicles: usize,
}

/// What [`prepare`] found and will write.
pub struct Prepared {
    /// The Halo 1 map's name.
    pub h1_name: String,
    /// The display name for the map list.
    pub display_name: String,
    /// Which sandbox.
    pub sandbox: Sandbox,
    /// SHA-256 of the Halo 1 map file.
    pub source_sha256: Sha256,
    /// The placements written into the base's scenario.
    pub applied: Applied,
    /// Placements with no Halo 2 equivalent.
    pub dropped: Vec<String>,
    /// What the Halo 1 map has, to compare with what was placed.
    pub source: SourceCounts,
    /// The collision BSP, converted.
    pub collision: tags::collision::Bsp,
    /// Collision materials written.
    pub collision_materials: usize,
    /// Render sections, one cluster each.
    pub sections: Vec<Section>,
    /// Halo 1 shader name of each structure material.
    pub materials: Vec<String>,
    /// For each structure material, the base shader it draws with
    /// ([`crate::shaders::pick`]): its index among the base's materials
    /// and its tag name.
    pub material_shaders: Vec<(usize, String)>,
    /// Halo 1's own textures on copies of those shaders
    /// ([`crate::textures`]), added when the map is built.
    pub textures: crate::textures::Retexture,
    /// The level's own skies ([`crate::sky`]), in place of the base's.
    pub skies: crate::sky::Skies,
    /// The level's own background sounds and sound environments
    /// ([`crate::ambience`]), in place of the base's.
    pub ambience: crate::ambience::Ambience,
    /// Per cluster, the background sound and sound environment it plays
    /// (indices into the scenario's palettes as written, -1 none), and
    /// per collision leaf its cluster ([`crate::clusters::sound_zones`]).
    pub sound_clusters: Vec<[i16; 2]>,
    pub sound_leaves: Vec<u16>,
    /// Halo 1's portals and each cluster's list of them
    /// ([`crate::clusters::portals`]).
    pub visibility: crate::clusters::Visibility,
    /// Whether triangles were flipped to Halo 2's winding.
    pub flipped: bool,
    /// The base's trigger volumes, moved out of the level when built.
    pub trigger_volumes: usize,
    /// World bounds.
    pub bounds: [f32; 6],
    /// The converted lightmap: one page per lit cluster, cut from Halo 1's
    /// lightmap bitmap ([`crate::clusters::lightmap_bitmap`]) and the slot it takes (the base's own
    /// lightmap bitmap's, free once the lightmap is replaced). `None` when
    /// the Halo 1 structure has no lightmap bitmap or the base none to
    /// replace: then every cluster is unlit.
    pub lightmap: Option<Lightmap>,
    /// The Halo 1 sandbox's converted objects, added first when the map
    /// is built ([`halo1::prepare_halo1`]); `None` for the Halo 2 sandbox.
    pub objects: Option<halo1::ObjectTags>,
    /// The index slot the first of them takes.
    objects_first: usize,
    /// Halo 1's scenery ([`halo1::plan_scenery`]), written into the
    /// scenario by the fresh build; `None` places none.
    pub scenery: Option<halo1::Scenery>,
    /// The Halo 1 sandbox's player biped (one of the added objects), made
    /// the multiplayer player unit when the map is built
    /// ([`crate::globals::set_player_unit`]); `None` keeps the base's.
    pub player_biped: Option<TagId>,
    /// The Halo 1 sandbox's first-person arms (one of the added tags),
    /// made the multiplayer players' arms when the map is built
    /// ([`crate::globals::set_player_hands`]); `None` keeps the base's.
    pub player_hands: Option<TagId>,
    /// The Halo 1 sandbox's grenade types (their equipment and projectile
    /// among the added objects), written over the globals' when the map
    /// is built ([`crate::globals::set_grenades`]); empty keeps the base's.
    pub grenades: Vec<crate::globals::Grenade<TagId>>,
    /// The Halo 1 sandbox's weapon list entries (entry, a Halo 1 weapon
    /// among the added objects), written over the base `mulg`'s when the
    /// map is built ([`crate::globals::set_weapon_list`]); empty keeps
    /// the base's.
    pub weapon_list: Vec<(usize, TagId)>,
    /// The Halo 1 sandbox's vehicle list entries (entry, a Halo 1 vehicle
    /// among the added objects), written over the base `mulg`'s when the
    /// map is built ([`crate::globals::set_universal_list`]); empty keeps
    /// the base's.
    pub vehicle_list: Vec<(usize, TagId)>,
    /// The base's scenario with the placements applied.
    scenario: Scenario,
    /// The base's structure BSP and lightmap slots, reused.
    sbsp: TagId,
    ltmp: TagId,
    templates: Templates,
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prepared")
            .field("h1_name", &self.h1_name)
            .field("sandbox", &self.sandbox)
            .field("sections", &self.sections.len())
            .field("materials", &self.materials.len())
            .finish_non_exhaustive()
    }
}

/// The converted lightmap bitmap and where it goes.
#[derive(Debug, Clone)]
pub struct Lightmap {
    /// The Halo 1 bitmap's tag name.
    pub source: String,
    /// Pages (images).
    pub pages: usize,
    /// The converted tag.
    pub bitmap: h1models::Converted,
    /// The base tag whose slot it takes, and that tag's name.
    pub slot: TagId,
    /// The name of the tag in that slot before.
    pub replaces: String,
}

/// A finished conversion.
#[derive(Debug, Clone)]
pub struct Converted {
    /// `h1_<map>`.
    pub name: String,
    /// The map file.
    pub bytes: Vec<u8>,
    /// Its package manifest: a derived map with `[list]` and one
    /// `[[variant]]` (this sandbox's), file [`variant_file`]. Merge it with
    /// the package's other variant to write both.
    pub manifest: Manifest,
}

/// The scenario's first BSP descriptor: the sbsp and ltmp datums, and the
/// element's address.
fn bsp_descriptor(scnr: &[u8], address: u32) -> Result<(u32, TagId, TagId)> {
    let word = |at: usize| -> Result<u32> {
        scnr.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
            .ok_or(Error::Invalid("scenario BSP descriptor outside the tag"))
    };
    if word(0x210)? == 0 {
        return Err(Error::Invalid("the base scenario has no structure BSP"));
    }
    let at = word(0x214)?;
    let off = at.wrapping_sub(address) as usize;
    Ok((at, TagId(word(off + 0x14)?), TagId(word(off + 0x1c)?)))
}

/// Base scenario blocks of things placed for the base's own level that no
/// converted map wants, emptied ([`clear_base_blocks`]): `(offset, what)`.
/// D (Coagulation, every element's references by group): `+0x378` 2
/// `DECR` (decorator sets: the base's grass and rocks, placed by its own
/// clusters); the converted clusters name no decorator. The background
/// sound and sound environment palettes (`+0x250`, `+0x258`) name Halo
/// 1's instead ([`crate::ambience`]). `+0xe8` (0x6c each, D: 21 on
/// Coagulation, 10 on Beaver Creek) places light fixtures: `+0x00` an
/// index into the `ligh` palette at `+0xf0` (0x28 each, the tag at `+0`;
/// 6 and 2 entries), `+0x02` the object name (-1 on all), `+0x08` the
/// position, inside the base's level on both. Neither base's scenario
/// names a `deca` from any block (their 51 decals are effects'), so there
/// are no placed decals to empty.
pub const BASE_BLOCKS: [(usize, &str); 2] = [(0xe8, "light fixtures"), (0x378, "decorators")];

/// Empties each of [`BASE_BLOCKS`] in a scenario payload: count and
/// pointer zero. Returns the elements dropped, per block.
pub fn clear_base_blocks(scnr: &mut [u8]) -> Result<[u32; 2]> {
    let mut dropped = [0; 2];
    for (k, (at, _)) in BASE_BLOCKS.iter().enumerate() {
        let field = scnr
            .get_mut(*at..*at + 8)
            .ok_or(Error::Invalid("scenario root too small"))?;
        dropped[k] = u32::from_le_bytes(field[..4].try_into().expect("4"));
        field.fill(0);
    }
    Ok(dropped)
}

/// Scenario trigger volumes: count at `scnr+0x108`, elements at `+0x10c`,
/// stride `0x44` (E `0x0011c383`..`0x0011c392`). The engine builds a
/// volume's frame from `+0x0c` and `+0x18` (directions) and `+0x24` (its
/// origin) (E `0x0011c40c`..`0x0011c417`).
pub const TRIGGER_VOLUMES: usize = 0x108;
/// Trigger volume element size.
pub const TRIGGER_VOLUME: usize = 0x44;
/// Where [`park_trigger_volumes`] moves each volume's origin: far below any
/// level, so nothing is ever inside it.
pub const PARKED: [f32; 3] = [0.0, 0.0, -1.0e6];

/// Moves every trigger volume of a scenario payload (loaded at `address`)
/// out of the level. The base's volumes (kill zones, script triggers) are
/// shaped for its own geometry; removing them would break the indices that
/// scripts and the kill list hold, so they stay, unreachable. Returns how
/// many moved; volumes outside `scnr` are an error.
pub fn park_trigger_volumes(scnr: &mut [u8], address: u32) -> Result<usize> {
    let word = |b: &[u8], at: usize| -> Result<u32> {
        b.get(at..at + 4)
            .map(|w| u32::from_le_bytes(w.try_into().expect("4")))
            .ok_or(Error::Invalid("scenario trigger volumes outside the tag"))
    };
    let count = word(scnr, TRIGGER_VOLUMES)? as usize;
    if count == 0 {
        return Ok(0);
    }
    let first = word(scnr, TRIGGER_VOLUMES + 4)?.wrapping_sub(address) as usize;
    let end = first
        .checked_add(count * TRIGGER_VOLUME)
        .filter(|&e| e <= scnr.len())
        .ok_or(Error::Invalid("scenario trigger volumes outside the tag"))?;
    for e in scnr[first..end].as_chunks_mut::<TRIGGER_VOLUME>().0 {
        for (k, v) in PARKED.iter().enumerate() {
            e[0x24 + 4 * k..0x28 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
    }
    Ok(count)
}

/// Everything short of writing: placements onto the base's scenario, the
/// collision BSP, the render geometry and the base's templates. Fails for
/// [`Sandbox::Halo1`], whose objects come from outside: use
/// [`halo1::prepare_halo1`].
pub fn prepare<R: Read + Seek>(
    h1: &H1Map,
    source_sha256: Sha256,
    base: &mut Map<R>,
    sandbox: Sandbox,
) -> Result<Prepared> {
    if sandbox == Sandbox::Halo1 {
        return Err(Error::Halo1Sandbox);
    }
    let h1_name = h1.header().name.clone();
    let display_name = known(&h1_name).map_or_else(|| h1_name.clone(), |(d, _)| d.to_owned());

    let mut plan = placements::plan_map(h1)?;
    // Halo 1's vehicle spots hold the origin on the floor; the base's
    // vehicles are raised by how far each reaches below its origin.
    if let Ok(ctx) = tags::object::Context::read(base) {
        placements::lift_vehicles(&mut plan, |role| placements::retail_vehicle_depth(&ctx, role));
    }
    let mp = h1.multiplayer()?;
    let source = SourceCounts {
        spawns: mp.starting_locations.len(),
        flags: mp.flags.len(),
        items: mp.equipment.len(),
        vehicles: mp.vehicles.len(),
    };
    let tags: Vec<_> = base
        .tags()
        .iter()
        .map(|t| (t.group, t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let mut scenario = Scenario::read(base)?;

    let refs = h1.structure_bsps()?;
    let [bsp] = refs.as_slice() else {
        return Err(Error::Invalid(
            "a multiplayer map should have one structure BSP",
        ));
    };
    let src = h1.structure_bsp(bsp)?;
    let [col] = src
        .collision_bsps()?
        .try_into()
        .map_err(|_| Error::Invalid("a structure BSP should have one collision BSP"))?;
    let collision = collision::convert(&col)?;
    let largest = collision
        .surfaces
        .iter()
        .map(|s| s.material)
        .filter(|&m| m != u16::MAX)
        .max()
        .map_or(0, |m| usize::from(m) + 1);
    let collision_materials = src.collision_materials()?.len().max(largest).max(1);

    let name_of = |r: h1map::TagRef| h1.tag(r.id).map(|t| t.name.clone()).unwrap_or_default();
    let mut meshes: Vec<Mesh> = structure::meshes(&src, name_of)?;
    let flipped = structure::fix_winding(&mut meshes);
    let mut index: BTreeMap<String, u16> = BTreeMap::new();
    let mut materials = Vec::new();
    let material: Vec<u16> = meshes
        .iter()
        .map(|m| {
            *index.entry(m.shader.clone()).or_insert_with(|| {
                materials.push(m.shader.clone());
                (materials.len() - 1) as u16
            })
        })
        .collect();
    let laid = crate::clusters::lay_out(h1, &src, &mut meshes, &material, col.planes.len())?;
    let sections = laid.sections;

    let scnr = base
        .scenario()
        .ok_or(Error::Invalid("the base has no scenario"))?;
    let (scnr_id, scnr_at) = (
        scnr.id,
        scnr.location
            .ok_or(Error::Invalid("base scenario has no data"))?
            .address,
    );
    let scnr_bytes = base.read_tag_bytes(scnr_id)?;
    let (_, sbsp, ltmp) = bsp_descriptor(&scnr_bytes, scnr_at)?;
    let trigger_volumes = park_trigger_volumes(&mut scnr_bytes.clone(), scnr_at)?;
    // Ambient sound: the palettes go into the base's storage, as many as
    // it holds (`ambience::patch_scenario`); each cluster names its zone's.
    let ambience = crate::ambience::read(h1);
    let room = crate::ambience::room(&scnr_bytes)?;
    let written = [
        ambience
            .backgrounds
            .len()
            .min(room[0])
            .min(crate::ambience::MAX_PALETTE),
        ambience
            .environments
            .len()
            .min(room[1])
            .min(crate::ambience::MAX_PALETTE),
    ];
    let (sound_clusters, sound_leaves) =
        crate::clusters::sound_zones(&src, collision.leaves.len(), |z| {
            ambience.index(0, z, written)
        })?;
    let sbsp_at = base
        .tag(sbsp)?
        .location
        .ok_or(Error::Invalid("base sbsp has no data"))?
        .address;
    let mut templates = Templates::from_base(&base.read_tag_bytes(sbsp)?, sbsp_at)?;
    let mut base_shaders = Vec::new();
    for i in 0..templates.material_heads.len() {
        let id = templates.shader(i);
        let name = base
            .tag(id)
            .ok()
            .and_then(|t| t.name.clone())
            .unwrap_or_default();
        let template = scenario_edit::scene::shader_bitmaps(base, id)
            .map(|(t, _)| t)
            .unwrap_or_default();
        templates.part_kinds[i] = crate::structure::base_part_kind(base, id, &template);
        base_shaders.push(shaders::BaseShader { name, template });
    }
    let h1_shaders: Vec<(String, String)> = materials
        .iter()
        .map(|n| {
            let group = h1
                .tags()
                .iter()
                .find(|t| &t.name == n)
                .map(|t| t.group.to_string())
                .unwrap_or_default();
            (n.clone(), group)
        })
        .collect();
    let material_shaders: Vec<(usize, String)> = shaders::pick(&h1_shaders, &base_shaders)
        .into_iter()
        .map(|i| (i, base_shaders[i].name.clone()))
        .collect();
    // Halo 1's own textures on copies of the picked shaders. A map whose
    // shaders can't be read keeps the base's (with a note saying why).
    let picks: Vec<(usize, String)> = material_shaders
        .iter()
        .map(|m| m.0)
        .zip(materials.iter().cloned())
        .collect();
    let textures = crate::textures::plan(h1, base, &templates, &picks).unwrap_or_else(|e| {
        crate::textures::Retexture {
            notes: vec![format!("the base's textures kept: {e}")],
            ..Default::default()
        }
    });

    // The base's lightmap bitmap's slot takes the converted lightmap.
    let lightmap_slot = slots::lightmap_bitmap(base, ltmp)?;
    let lightmap = match (
        crate::clusters::lightmap_bitmap(h1, &src, &laid.pages)?,
        lightmap_slot,
    ) {
        (Some((source, bm, bitmap)), Some(slot)) => Some(Lightmap {
            source,
            pages: bm.images.len(),
            bitmap,
            slot,
            replaces: base.tag(slot)?.name.clone().unwrap_or_default(),
        }),
        _ => None,
    };
    // Built vehicle collections are added after the base's tags when the
    // map is built (`TagSet::add_tag`), so their datums are known now:
    // slot `n` gets salt `0xe174 + n` ([`halo1::added_datum`]).
    let first = base.index_slots() as usize;
    let mut spare: Vec<TagId> = (0..MAX_NEW_COLLECTIONS)
        .map(|k| halo1::added_datum(first + k))
        .collect();
    let applied = apply(&mut scenario, &plan, &tags, &mut spare)?;

    Ok(Prepared {
        h1_name,
        display_name,
        sandbox,
        source_sha256,
        applied,
        dropped: plan.dropped,
        source,
        collision,
        collision_materials,
        sections,
        materials,
        material_shaders,
        textures,
        skies: crate::sky::Skies {
            templates: crate::sky_shaders::sky_templates(base),
            ..crate::sky::read(h1)
        },
        ambience,
        sound_clusters,
        visibility: laid.visibility,
        sound_leaves,
        flipped,
        trigger_volumes,
        bounds: src.world_bounds(),
        lightmap,
        objects: None,
        objects_first: 0,
        scenery: None,
        player_biped: None,
        player_hands: None,
        grenades: Vec::new(),
        weapon_list: Vec::new(),
        vehicle_list: Vec::new(),
        scenario,
        sbsp,
        ltmp,
        templates,
    })
}

/// FNV-1a, for the structure checksum (any value the BSP and lightmap
/// agree on).
fn fnv(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |h, &b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

impl Prepared {
    /// Each structure material's base material and Halo 1 shader name.
    pub fn texture_picks(&self) -> Vec<(usize, String)> {
        self.material_shaders
            .iter()
            .zip(&self.materials)
            .map(|((i, _), n)| (*i, n.clone()))
            .collect()
    }

    /// The package manifest for `map_bytes`.
    pub fn manifest(&self, map_bytes: &[u8]) -> Manifest {
        self.manifest_with(map_bytes, &self.applied.precision, &self.hud_swaps())
    }

    /// The package manifest for the fresh map `f`, with its own datums.
    pub fn manifest_fresh(&self, f: &crate::fresh::Fresh) -> Manifest {
        self.manifest_with(&f.bytes, &f.precision, &f.hud)
    }

    /// The HUD pairs of the Halo 1 objects (owner, Halo 1 HUD, Halo 2
    /// HUD), as the datums the pinned build gives them.
    pub fn hud_swaps(&self) -> Vec<[TagId; 3]> {
        self.objects.as_ref().map_or(Vec::new(), |o| {
            o.hud_swaps
                .iter()
                .map(|s| {
                    s.map(|t| match t {
                        halo1::HudTag::Added(k) => halo1::added_datum(self.objects_first + k),
                        halo1::HudTag::Base(id) => id,
                    })
                })
                .collect()
        })
    }

    /// The package manifest for `map_bytes`, whose precision swaps'
    /// collections are `precision` and HUD pairs `hud` (a fresh map's own
    /// datums, `crate::fresh::Fresh`).
    pub fn manifest_with(
        &self,
        map_bytes: &[u8],
        precision: &[(TagId, TagId)],
        hud: &[[TagId; 3]],
    ) -> Manifest {
        let name = map_name(&self.h1_name);
        let variant = Variant {
            file: variant_file(&self.h1_name, self.sandbox),
            sha256: Sha256::of(map_bytes),
            origin: Origin {
                game: "halo1".into(),
                map: self.h1_name.clone(),
                sandbox: self.sandbox.name().into(),
                source_sha256: self.source_sha256,
            },
            precision: precision
                .iter()
                .map(|&(from, to)| content::Swap {
                    from: from.0,
                    to: to.0,
                })
                .collect(),
            hud: hud
                .iter()
                .map(|&[owner, halo1, halo2]| content::HudSwap {
                    owner: owner.0,
                    halo1: halo1.0,
                    halo2: halo2.0,
                })
                .collect(),
            hud_sensor: self.objects.as_ref().and_then(|o| o.hud_sensor),
        };
        // Parsed from text so the manifest's own checks run on it.
        let text = format!(
            "[package]\nformat = 1\nname = {display:?}\n\n[base]\nmap = {name:?}\nderived = true\n\n\
             [list]\nbase = {base:?}\n\n[[variant]]\nfile = {file:?}\nsha256 = \"{sha}\"\n\
             origin = {{ game = \"halo1\", map = {map:?}, sandbox = \"{sandbox}\", source_sha256 = \"{src}\" }}\n{precision}{hud}{sensor}",
            display = self.display_name,
            base = known(&self.h1_name).map_or("beavercreek", |(_, b)| b),
            file = variant.file,
            sha = variant.sha256,
            map = variant.origin.map,
            sandbox = variant.origin.sandbox,
            src = variant.origin.source_sha256,
            precision = if variant.precision.is_empty() {
                String::new()
            } else {
                let swaps: Vec<String> = variant
                    .precision
                    .iter()
                    .map(|p| format!("{{ from = {}, to = {} }}", p.from, p.to))
                    .collect();
                format!("precision = [{}]\n", swaps.join(", "))
            },
            hud = if variant.hud.is_empty() {
                String::new()
            } else {
                let swaps: Vec<String> = variant
                    .hud
                    .iter()
                    .map(|h| {
                        format!(
                            "{{ owner = {}, halo1 = {}, halo2 = {} }}",
                            h.owner, h.halo1, h.halo2
                        )
                    })
                    .collect();
                format!("hud = [{}]\n", swaps.join(", "))
            },
            sensor = variant
                .hud_sensor
                .map_or(String::new(), |[x, y]| format!("hud_sensor = [{x}, {y}]\n")),
        );
        let m = Manifest::from_toml(&text).expect("a well-formed manifest");
        debug_assert_eq!(m.variants, [variant]);
        m
    }

    /// Adds the vehicle collections the placements name but the base lacks
    /// ([`Applied::collections`]): a `0xc` root whose block at `+0` holds one
    /// `0x10` entry (weight `+0`, vehicle reference `+4`, variant string ID
    /// `+0xc`; docs/formats/objects.md), and the base's own collection
    /// root's `+0x8` word (each retail `vehc` and `itmc` has one). Each
    /// takes the slot and datum of a tag the converted map no longer uses
    /// ([`crate::slots`]), which the scenario already names.
    fn add_collections<R: Read + Seek>(&self, base: &mut Map<R>, set: &mut TagSet) -> Result<()> {
        if self.applied.collections.is_empty() {
            return Ok(());
        }
        let like = base
            .tags()
            .iter()
            .filter(|t| t.group.0 == *b"vehc" || t.group.0 == *b"itmc")
            .min_by_key(|t| t.group.0 != *b"vehc")
            .map(|t| t.id);
        let word8 = match like {
            Some(id) => base
                .read_tag_bytes(id)?
                .get(8..12)
                .map_or(0, |b| u32::from_le_bytes(b.try_into().expect("4"))),
            None => 0,
        };
        set.add_group(FourCC(*b"vehc"), None, None);
        for c in &self.applied.collections {
            let mut entry = Block::zeroed(COLLECTION_ENTRY);
            entry.put_f32(0, 1.0).tag_ref(4, c.vehicle);
            if let Some(v) = c.variant {
                entry.string_id(0xc, v);
            }
            let mut root = Block::zeroed(COLLECTION_ROOT);
            root.put_u32(8, word8).block(0, 1, entry);
            let id = set.add_tag(FourCC(*b"vehc"), &c.name, TagPayload::Built(root))?;
            if id != c.id {
                return Err(Error::Invalid(
                    "a built vehicle collection's datum moved from what the placements name",
                ));
            }
        }
        Ok(())
    }

    /// Writes the map with `mopp` as the structure's MOPP code block.
    pub fn build<R: Read + Seek>(&self, base: &mut Map<R>, mopp: &[u8]) -> Result<Converted> {
        Ok(self.build_pruned(base, mopp)?.0)
    }

    /// [`Self::build`], with what pinned pruning dropped and why each
    /// kept tag stayed ([`crate::fresh::Pruned::why`]).
    pub fn build_pruned<R: Read + Seek>(
        &self,
        base: &mut Map<R>,
        mopp: &[u8],
    ) -> Result<(Converted, crate::fresh::Pruned)> {
        let (_, bytes, _, pruned) = self.assemble(base, mopp)?;
        let manifest = self.manifest(&bytes);
        Ok((
            Converted {
                name: map_name(&self.h1_name),
                bytes,
                manifest,
            },
            pruned,
        ))
    }

    /// The pinned map: the set it was built from (the base decomposed,
    /// with everything converted added, and the base's tags nothing
    /// reaches dropped: [`crate::fresh::prune_pinned`]), its bytes, the
    /// converted structure's BSP section, and what was dropped.
    pub(crate) fn assemble<R: Read + Seek>(
        &self,
        base: &mut Map<R>,
        mopp: &[u8],
    ) -> Result<(TagSet, Vec<u8>, usize, crate::fresh::Pruned)> {
        let (mut set, section, pruned) = self.pinned_set(base, mopp)?;

        // The scenario stays pinned, so its BSP descriptor is patched with
        // the new region's place: build once to learn it (it doesn't
        // depend on the scenario's bytes), patch, build again.
        let (_, layout) = set.build()?;
        let (file_offset, size) = layout.bsps[section];
        let scenario = set.scenario;
        let tag = set.tag_mut(scenario).ok_or(Error::Invalid("no scenario"))?;
        let map::build::TagPayload::Pinned { address, bytes } = &mut tag.payload else {
            return Err(Error::Invalid("the base scenario should be pinned"));
        };
        let (at, _, _) = bsp_descriptor(bytes, *address)?;
        let off = at.wrapping_sub(*address) as usize;
        bytes[off..off + 4].copy_from_slice(&file_offset.to_le_bytes());
        bytes[off + 4..off + 8].copy_from_slice(&size.to_le_bytes());
        bytes[off + 8..off + 12].copy_from_slice(&layout.bsp_base.to_le_bytes());
        let (bytes, again) = set.build()?;
        if again.bsps[section] != (file_offset, size) {
            return Err(Error::Invalid("the BSP region moved between builds"));
        }
        Ok((set, bytes, section, pruned))
    }

    /// The pinned set [`Self::assemble`] builds, before it is built: the
    /// base decomposed with everything converted added and the base's
    /// tags nothing reaches dropped ([`crate::fresh::prune_pinned`]), the
    /// converted structure's BSP section, and what was dropped and why
    /// each kept tag stayed ([`crate::fresh::Pruned::why`]). For seeing
    /// what a map that doesn't fit its tag memory keeps.
    pub fn pinned_set<R: Read + Seek>(
        &self,
        base: &mut Map<R>,
        mopp: &[u8],
    ) -> Result<(TagSet, usize, crate::fresh::Pruned)> {
        let mut patched = Cursor::new(Vec::new());
        scenario_edit::save::write_patched(base, &self.scenario, &mut patched)?;
        let mut patched = Map::from_reader(Cursor::new(patched.into_inner()))?;
        let mut set = TagSet::from_map(&mut patched)?;
        // The vehicle collections the placements build first (their
        // datums were picked from the first added slot), then the
        // converted objects.
        self.add_collections(base, &mut set)?;
        if let Some(objects) = &self.objects {
            halo1::install(objects, self.objects_first, &mut set)?;
        }
        if let Some(biped) = self.player_biped {
            crate::globals::set_player_unit(&mut set, biped, crate::globals::Players::Multiplayer)?;
        }
        if let Some(hands) = self.player_hands {
            crate::globals::set_player_hands(&mut set, hands, crate::globals::Players::Multiplayer)?;
        }
        if !self.grenades.is_empty() {
            crate::globals::set_grenades(&mut set, &self.grenades)?;
        }
        if !self.weapon_list.is_empty() {
            crate::globals::set_weapon_list(&mut set, &self.weapon_list)?;
        }
        if !self.vehicle_list.is_empty() {
            crate::globals::set_universal_list(
                &mut set,
                crate::globals::List::Vehicles,
                &self.vehicle_list,
            )?;
        }

        set.name = map_name(&self.h1_name);
        set.scenario_path = scenario_path(&self.h1_name);
        let slot = (set.scenario.0 & 0xffff) as usize;
        set.slots[slot].name = Some(set.scenario_path.clone());

        let lightmap_bitmap = match &self.lightmap {
            Some(l) => l.bitmap.clone().install_at(
                &mut set,
                l.slot,
                &format!("{}_lightmap", scenario_path(&self.h1_name)),
            )?,
            None => TagId::NULL,
        };

        let mut geometry = Vec::new();
        for s in &self.sections {
            s.vertices.iter().for_each(|v| {
                v.position
                    .iter()
                    .for_each(|f| geometry.extend(f.to_le_bytes()))
            });
        }
        // Halo 1's own textures: new bitmaps and shader copies, and the
        // material heads naming them.
        let (templates, materials) =
            self.textures
                .install(&mut set, &self.templates, &self.texture_picks())?;
        let skies = self.skies.install(&mut set)?;
        let ambience = self.ambience.install(&mut set)?;
        let room = {
            let tag = set.tag(set.scenario).ok_or(Error::Invalid("no scenario"))?;
            let map::build::TagPayload::Pinned { bytes, .. } = &tag.payload else {
                return Err(Error::Invalid("the base scenario should be pinned"));
            };
            crate::ambience::room(bytes)?
        };
        let [backgrounds, environments] = ambience.palettes(room);
        let sound = structure::Sound {
            backgrounds: ambience.backgrounds[..backgrounds.len()].to_vec(),
            environments: ambience.environments[..environments.len()].to_vec(),
            clusters: self.sound_clusters.clone(),
            leaves: self.sound_leaves.clone(),
        };
        let structure = Structure {
            collision: &self.collision,
            collision_materials: self.collision_materials,
            sections: &self.sections,
            materials: &materials,
            bounds: self.bounds,
            mopp,
            checksum: fnv(&geometry),
            templates: &templates,
            lightmap_bitmap,
            sound: &sound,
            visibility: &self.visibility,
            // A multiplayer level keeps the base's scenario, with no
            // atmosphere block to name: every cluster takes its sky's fog.
            atmosphere: &[],
        };
        let section = structure.write(&mut set, self.sbsp, self.ltmp)?;

        // The scenario's own edits (none depends on the layout), before
        // the pruning below, so the base's sky, decorators and ambience
        // they replace are no longer named.
        {
            let scenario = set.scenario;
            let tag = set.tag_mut(scenario).ok_or(Error::Invalid("no scenario"))?;
            let map::build::TagPayload::Pinned { address, bytes } = &mut tag.payload else {
                return Err(Error::Invalid("the base scenario should be pinned"));
            };
            park_trigger_volumes(bytes, *address)?;
            crate::sky::patch_scenario(bytes, *address, &skies)?;
            clear_base_blocks(bytes)?;
            crate::ambience::patch_scenario(bytes, *address, &ambience)?;
        }

        // The base's metadata stays where it is and the conversion's goes
        // after it, so the base's tags nothing reaches any more (the
        // placements the conversion removed, their weapons' effects...)
        // are dropped, their bytes free for the built tags, as the fresh
        // build drops them ([`crate::fresh::prune`]): the same roots, and
        // the precision swaps' collections and each owner's Halo 2 HUD,
        // which no placement names (`boot::precision`, `boot::hud_swap`).
        let mut keep: Vec<TagId> = self.applied.precision.iter().map(|p| p.1).collect();
        keep.extend(self.hud_swaps().iter().map(|h| h[2]));
        let pruned = crate::fresh::prune_pinned(
            &mut set,
            base.index_slots() as usize,
            &crate::fresh::MULTIPLAYER,
            &keep,
        );
        Ok((set, section, pruned))
    }
}

/// Converts `h1` onto `base`. `source_sha256` is the Halo 1 map file's
/// hash, for the manifest's `[origin]`.
pub fn convert<R: Read + Seek>(
    h1: &H1Map,
    source_sha256: Sha256,
    base: &mut Map<R>,
    sandbox: Sandbox,
) -> Result<Converted> {
    let p = prepare(h1, source_sha256, base, sandbox)?;
    let mopp = structure_mopp(&p.collision)?;
    p.build(base, &mopp)
}
