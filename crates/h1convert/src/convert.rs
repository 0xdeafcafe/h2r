//! A whole conversion: one Halo 1 multiplayer map onto a Halo 2 base map,
//! as the bytes of `h1_<map>.map` and its package manifest
//! (docs/game/h1-multiplayer.md, "Converting a map").
//!
//! [`prepare`] does everything that needs no MOPP code and reports it;
//! [`Prepared::build`] writes the map from it with given MOPP bytes;
//! [`convert`] is both, taking the MOPP code from [`structure_mopp`], and so
//! refuses to make a map until the MOPP writer is merged.

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

/// Size of a collection's (`itmc`, `vehc`) root.
pub const COLLECTION_ROOT: usize = 0xc;
/// Size of a collection entry.
pub const COLLECTION_ENTRY: usize = 0x10;

/// Which game's objects a converted map uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sandbox {
    /// Halo 1's weapons, vehicles and equipment (phase 4; not built).
    Halo1,
    /// Halo 2's nearest equivalents (docs/game/h1-multiplayer.md).
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
    /// Whether triangles were flipped to Halo 2's winding.
    pub flipped: bool,
    /// The base's trigger volumes, moved out of the level when built.
    pub trigger_volumes: usize,
    /// World bounds.
    pub bounds: [f32; 6],
    /// The converted lightmap: Halo 1's lightmap bitmap as a Halo 2 one
    /// (one image per page, colour) and the slot it takes (the base's own
    /// lightmap bitmap's, free once the lightmap is replaced). `None` when
    /// the Halo 1 structure has no lightmap bitmap or the base none to
    /// replace: then every cluster is unlit.
    pub lightmap: Option<Lightmap>,
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
/// [`Sandbox::Halo1`], which isn't built yet.
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

    let plan = placements::plan_map(h1)?;
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
    let sections = structure::pack(&meshes, &material);

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
    let sbsp_at = base
        .tag(sbsp)?
        .location
        .ok_or(Error::Invalid("base sbsp has no data"))?
        .address;
    let templates = Templates::from_base(&base.read_tag_bytes(sbsp)?, sbsp_at)?;
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

    // Slots for new tags: tags only the base's structure named, except
    // the shaders the converted one still names and the base's lightmap
    // bitmap, kept for the converted lightmap.
    let lightmap_slot = slots::lightmap_bitmap(base, ltmp)?;
    let pages = h1bitmaps::lightmap_pages(h1, &src)?;
    let lightmap = match (&pages.tag, lightmap_slot) {
        (Some(tag), Some(slot)) => {
            let (bm, bitmap, _) = h1bitmaps::convert_bitmap(h1, tag)?;
            Some(Lightmap {
                source: tag.name.clone(),
                pages: bm.images.len(),
                bitmap,
                slot,
                replaces: base.tag(slot)?.name.clone().unwrap_or_default(),
            })
        }
        _ => None,
    };
    let mut keep: Vec<TagId> = material_shaders
        .iter()
        .map(|(i, _)| templates.shader(*i))
        .collect();
    keep.push(templates.collision_shader());
    keep.extend(lightmap_slot);
    let mut spare = slots::spare(base, sbsp, ltmp, &keep)?;
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
        flipped,
        trigger_volumes,
        bounds: src.world_bounds(),
        lightmap,
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
    /// The package manifest for `map_bytes`.
    pub fn manifest(&self, map_bytes: &[u8]) -> Manifest {
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
        };
        // Parsed from text so the manifest's own checks run on it.
        let text = format!(
            "[package]\nformat = 1\nname = {display:?}\n\n[base]\nmap = {name:?}\nderived = true\n\n\
             [list]\nbase = {base:?}\n\n[[variant]]\nfile = {file:?}\nsha256 = \"{sha}\"\n\
             origin = {{ game = \"halo1\", map = {map:?}, sandbox = \"{sandbox}\", source_sha256 = \"{src}\" }}\n",
            display = self.display_name,
            base = known(&self.h1_name).map_or("beavercreek", |(_, b)| b),
            file = variant.file,
            sha = variant.sha256,
            map = variant.origin.map,
            sandbox = variant.origin.sandbox,
            src = variant.origin.source_sha256,
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
            let slot = set
                .slots
                .get_mut(c.id.index())
                .ok_or(Error::Invalid("a reused slot is outside the index"))?;
            slot.name = Some(c.name.clone());
            let mut entry = Block::zeroed(COLLECTION_ENTRY);
            entry.put_f32(0, 1.0).tag_ref(4, c.vehicle);
            if let Some(v) = c.variant {
                entry.string_id(0xc, v);
            }
            let mut root = Block::zeroed(COLLECTION_ROOT);
            root.put_u32(8, word8).block(0, 1, entry);
            let tag = set
                .tag_mut(c.id)
                .ok_or(Error::Invalid("a reused slot holds no tag"))?;
            tag.group = FourCC(*b"vehc");
            tag.payload = TagPayload::Built(root);
        }
        Ok(())
    }

    /// Writes the map with `mopp` as the structure's MOPP code block.
    pub fn build<R: Read + Seek>(&self, base: &mut Map<R>, mopp: &[u8]) -> Result<Converted> {
        let mut patched = Cursor::new(Vec::new());
        scenario_edit::save::write_patched(base, &self.scenario, &mut patched)?;
        let mut patched = Map::from_reader(Cursor::new(patched.into_inner()))?;
        let mut set = TagSet::from_map(&mut patched)?;

        self.add_collections(base, &mut set)?;

        let name = map_name(&self.h1_name);
        set.name = name.clone();
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
        let structure = Structure {
            collision: &self.collision,
            collision_materials: self.collision_materials,
            sections: &self.sections,
            materials: &self
                .material_shaders
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            bounds: self.bounds,
            mopp,
            checksum: fnv(&geometry),
            templates: &self.templates,
            lightmap_bitmap,
        };
        let section = structure.write(&mut set, self.sbsp, self.ltmp)?;

        // The scenario stays pinned, so its descriptor is patched with the
        // new region's place: build once to learn it (it doesn't depend on
        // the scenario's bytes), patch, build again.
        let (_, layout) = set.build()?;
        let (file_offset, size) = layout.bsps[section];
        let scenario = set.scenario;
        let tag = set.tag_mut(scenario).ok_or(Error::Invalid("no scenario"))?;
        let map::build::TagPayload::Pinned { address, bytes } = &mut tag.payload else {
            return Err(Error::Invalid("the base scenario should be pinned"));
        };
        park_trigger_volumes(bytes, *address)?;
        let (at, _, _) = bsp_descriptor(bytes, *address)?;
        let off = at.wrapping_sub(*address) as usize;
        bytes[off..off + 4].copy_from_slice(&file_offset.to_le_bytes());
        bytes[off + 4..off + 8].copy_from_slice(&size.to_le_bytes());
        bytes[off + 8..off + 12].copy_from_slice(&layout.bsp_base.to_le_bytes());
        let (bytes, again) = set.build()?;
        if again.bsps[section] != (file_offset, size) {
            return Err(Error::Invalid("the BSP region moved between builds"));
        }
        let manifest = self.manifest(&bytes);
        Ok(Converted {
            name,
            bytes,
            manifest,
        })
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
