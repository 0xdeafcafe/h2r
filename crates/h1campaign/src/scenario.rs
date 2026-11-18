//! The converted level's scenario: the base's `scnr`, relocated as a block
//! tree (`tags::schema::Relocator::to_built_emptied`) with the fields below
//! emptied first ([`replaced_fields`]), so the base scenario's old bytes
//! are free for built tags; only the arrays other tags keep inside them
//! stay, as unowned metadata. When the base scenario does not relocate,
//! the root is rebuilt over the old bytes instead (`0x3e0` bytes, its
//! untouched fields keeping their raw addresses, the old bytes kept). The
//! fields below are replaced.
//!
//! | Field | Stride | Written |
//! |---|---|---|
//! | `+0x08` skies | 8 | Halo 1's skies, converted (`h1sky`) |
//! | `+0x48` object names | `0x24` | Halo 1's names; each placement index mapped to the converted placement, or -1 |
//! | `+0x50`..`+0x330` object placements and palettes | per type (`tags::object::OBJECT_TYPES`) | emptied; weapons, equipment, vehicles, scenery and bipeds from Halo 1's |
//! | `+0xf8` player starting profiles | `0x44` | Halo 1's, weapons the converted tags ([`crate::profiles`]) |
//! | `+0x100` player starting locations | `0x34` | Halo 1's, each on the base's first element |
//! | `+0x108` trigger volumes | `0x44` | the base's, parked out of the level (`h1convert::convert::park_trigger_volumes`), then Halo 1's |
//! | `+0x130` BSP switch trigger volumes | `0xe` | Halo 1's (`h1::BSP_SWITCHES`), on its volumes and BSPs |
//! | AI fields (`h1ai::convert::ScenarioBlocks`) | | Halo 1's AI, or empty |
//! | script fields (`h1scripts::compile::field`) | | Halo 1's scripts, or empty |
//! | `+0x210` BSP descriptors | `0x44` | one per Halo 1 BSP, in Halo 1's order: its region (`FixupKind::Bsp`), `sbsp` and `ltmp` |
//! | `+0x250`, `+0x258` background sounds, sound environments | `0x64`, `0x48` | Halo 1's (`h1convert::ambience`), the palettes the clusters name |
//! | `+0x290` cluster data | `0x34` | one per BSP: per cluster its background sound and sound environment (as the structure's cluster names them), its centroid; the BSP's checksum |
//! | `+0x388` per-BSP points | `0x10` | one per BSP, no points |

use std::collections::HashMap;
use std::io::{Read, Seek};

use h1map::H1Map;
use map::build::{Block, BspField, FixupKind, TagPayload, TagSet};
use map::{FourCC, Map, TagId};

use crate::{Options, Report, Result, invalid};

/// `scnr` root size (`tags/src/schema/layouts.txt`).
pub const ROOT: usize = 0x3e0;
/// Skies: 8-byte `sky ` references.
pub const SKIES: u32 = 0x08;
/// Object names (`scenario_edit::layout::OBJECT_NAMES`).
pub const OBJECT_NAMES: u32 = 0x48;
pub const OBJECT_NAME: usize = 0x24;
/// Player starting locations.
pub const STARTING_LOCATIONS: u32 = 0x100;
pub const STARTING_LOCATION: usize = 0x34;
/// Trigger volumes (`h1convert::convert::TRIGGER_VOLUMES`).
pub const TRIGGER_VOLUMES: u32 = 0x108;
pub const TRIGGER_VOLUME: usize = 0x44;
/// BSP descriptors.
pub const BSPS: u32 = 0x210;
pub const BSP: usize = 0x44;
/// BSP switch trigger volumes: `i16` trigger volume, source BSP,
/// destination BSP, then four shorts retail mostly leaves -1
/// (`h2_bsp_switch_volumes`).
pub const BSP_SWITCHES: u32 = 0x130;
pub const BSP_SWITCH: usize = 0xe;
/// Cluster data, one element per BSP (every retail map has one per BSP):
/// an `sbsp` reference left null, per-cluster blocks of background sound
/// (`+0x08`), sound environment (`+0x10`), centroid (`+0x1c`, a point)
/// and two more indices (`+0x24`, `+0x2c`), each `0xffff` for none, and
/// the BSP's checksum at `+0x18` (`sbsp+0x08`).
pub const CLUSTER_DATA: u32 = 0x290;
pub const CLUSTER_DATUM: usize = 0x34;
/// Background sound and sound environment palettes
/// (`h1convert::ambience`).
pub const BACKGROUND_SOUNDS: u32 = h1convert::ambience::BACKGROUND_SOUNDS as u32;
pub const SOUND_ENVIRONMENTS: u32 = h1convert::ambience::SOUND_ENVIRONMENTS as u32;
/// One element per BSP too: a null `sbsp` reference and a block of
/// points (`0xc`), often empty.
pub const BSP_POINTS: u32 = 0x388;
pub const BSP_POINT: usize = 0x10;

/// One converted BSP, as the scenario refers to it.
#[derive(Debug, Clone)]
pub struct BspSlot {
    /// Index into `TagSet::bsps`.
    pub section: usize,
    pub sbsp: TagId,
    pub ltmp: TagId,
    /// `sbsp+0x08`.
    pub checksum: u32,
    /// One per cluster.
    pub centroids: Vec<[f32; 3]>,
    /// Per cluster: background sound and sound environment (-1 none).
    pub sounds: Vec<[i16; 2]>,
}

/// The `+0x290` element of `b`.
fn cluster_datum(b: &BspSlot) -> Block {
    let n = b.centroids.len();
    let sound = |which: usize| {
        let mut c = Block::zeroed(4 * n);
        for k in 0..n {
            let v = b.sounds.get(k).map_or(-1, |s| s[which]);
            c.put_u16(4 * k, v as u16);
        }
        c
    };
    let none = || {
        let mut c = Block::zeroed(4 * n);
        for k in 0..n {
            c.put_u16(4 * k, 0xffff);
        }
        c
    };
    let mut e = Block::zeroed(CLUSTER_DATUM);
    e.put_u32(0, u32::from_be_bytes(*b"sbsp"))
        .put_u32(4, u32::MAX);
    let mut centroids = Block::zeroed(0xc * n);
    for (k, p) in b.centroids.iter().enumerate() {
        for (j, v) in p.iter().enumerate() {
            centroids.put_f32(0xc * k + 4 * j, *v);
        }
    }
    e.block(0x08, n as u32, sound(0))
        .block(0x10, n as u32, sound(1))
        .put_u32(0x18, b.checksum)
        .block(0x1c, n as u32, centroids)
        .block(0x24, n as u32, none())
        .block(0x2c, n as u32, none());
    e
}

/// Squads the engine holds: the AI's squad array at load (`0x00200974`,
/// `mov ebx,14Fh`, `0x98` per datum) takes one datum per scenario squad
/// (`+0x160`, `0x74` each; filled by `0x00200b30`), and a squad past it
/// faults at `0x00200c44`. **E**.
pub const MAX_SQUADS: usize = 0x14f;
/// Squad groups the engine holds: the array at `0x002009ad` (`mov
/// ebx,64h`, `0x38` per datum), one datum per `+0x158` element. **E**.
pub const MAX_SQUAD_GROUPS: usize = 0x64;

/// Script tables: string data, scripts, globals, references, nodes.
pub const SCRIPT_FIELDS: [u32; 5] = [0x1b0, 0x1b8, 0x1c0, 0x1c8, 0x238];
/// Palette element (`scenario_edit::layout`): a tag reference first.
pub const PALETTE: usize = 0x28;

/// Halo 1 scenario blocks read here (offset of the 12-byte block field,
/// element size). **S** (the editing kits' scenario layout), checked on
/// a30 by `a30_converts_onto_its_base` (V): every placement's palette
/// index is in range and the strides tile each block.
pub mod h1 {
    /// Object names: 32-byte name, `i16` object type, `i16` placement.
    pub const OBJECT_NAMES: (usize, usize) = (0x204, 0x24);
    /// Placement flags (`u16` at `+4` of every placement kind): bit 0, the
    /// object is not placed when the level loads, only when a script
    /// creates it. **V** (`h1_placement_flags`): set on a10's 67 cutscene
    /// bipeds (the x10 crew in space, Keyes, the hangar marines) and on
    /// none of its 41 fighting or set-dressing ones.
    pub const NOT_AUTOMATIC: u16 = 1;
    pub const BIPEDS: (usize, usize) = (0x228, 0x78);
    pub const BIPED_PALETTE: (usize, usize) = (0x234, 0x30);
    pub const VEHICLES: (usize, usize) = (0x240, 0x78);
    pub const VEHICLE_PALETTE: (usize, usize) = (0x24c, 0x30);
    pub const EQUIPMENT: (usize, usize) = (0x258, 0x28);
    pub const EQUIPMENT_PALETTE: (usize, usize) = (0x264, 0x30);
    pub const WEAPONS: (usize, usize) = (0x270, 0x5c);
    pub const WEAPON_PALETTE: (usize, usize) = (0x27c, 0x30);
    pub const SCENERY: (usize, usize) = (0x210, 0x48);
    pub const SCENERY_PALETTE: (usize, usize) = (0x21c, 0x30);
    /// Devices. **V** (`every_device_placement_lands`): on all ten levels
    /// each block's elements tile it with these strides, every palette
    /// index is in range, and every power and position group is -1 or a
    /// device group.
    pub const MACHINES: (usize, usize) = (0x294, 0x40);
    pub const MACHINE_PALETTE: (usize, usize) = (0x2a0, 0x30);
    pub const CONTROLS: (usize, usize) = (0x2ac, 0x40);
    pub const CONTROL_PALETTE: (usize, usize) = (0x2b8, 0x30);
    pub const LIGHT_FIXTURES: (usize, usize) = (0x2c4, 0x58);
    pub const LIGHT_FIXTURE_PALETTE: (usize, usize) = (0x2d0, 0x30);
    /// A device placement after the object fields: `i16` power group and
    /// position group (indices into the device groups, -1 none), `u32`
    /// device flags, then for machines and controls their own `u32` flags.
    /// Halo 2 has the same fields in the same order ([`H2_DEVICE_FIELDS`]).
    pub const DEVICE_POWER_GROUP: usize = 0x28;
    pub const DEVICE_POSITION_GROUP: usize = 0x2a;
    pub const DEVICE_FLAGS: usize = 0x2c;
    pub const DEVICE_OWN_FLAGS: usize = 0x30;
    /// BSP switch trigger volumes: `i16` trigger volume, source BSP,
    /// destination BSP, one more. **S** (`h1_bsp_switch_volumes`): on all
    /// ten levels the only root block whose elements read so, between
    /// BSPs adjacent in the level.
    pub const BSP_SWITCHES: (usize, usize) = (0x39c, 8);
}

/// The high word of the unique IDs given to converted placements
/// (`+0x28`: retail values are a per-level high word and a running low
/// word, V on the Halo 2 disc's campaign weapons).
pub const UNIQUE_ID_HIGH: u32 = 0x4831;

/// One BSP descriptor of the base: its `sbsp` and `ltmp`.
#[derive(Debug, Clone, Copy)]
pub struct Descriptor {
    pub sbsp: TagId,
    pub ltmp: TagId,
}

/// The base's scenario, as read.
/// A relocated base scenario: its tree, and the old byte ranges other
/// tags' arrays still use (address, end).
type Relocated = (Block, Vec<(u32, u32)>);

pub struct BaseScenario {
    pub id: TagId,
    pub address: u32,
    pub bytes: Vec<u8>,
    pub descriptors: Vec<Descriptor>,
    /// The base scenario as a relocatable tree (`tags::schema`), when it
    /// is one and no other tag points into it. The converted scenario is
    /// built from it, so the fields replaced here leave the map and the
    /// base scenario's old bytes are free for built tags. `None` keeps the
    /// old bytes as unowned metadata instead.
    pub tree: Option<Block>,
    /// Why the base scenario did not relocate, when it did not.
    pub refusal: Option<String>,
    /// With a tree: the old bytes other tags' arrays still use, as
    /// (address, end). They stay as unowned metadata.
    pub keep: Vec<(u32, u32)>,
    /// The base's sky shaders converted sky models' materials copy, and
    /// the one a material falls back to
    /// (`h1convert::sky_shaders::sky_templates`).
    pub sky_templates: h1convert::sky_shaders::SkyTemplates,
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|w| u32::from_le_bytes(w.try_into().expect("4")))
        .ok_or_else(|| invalid(format!("scenario field {at:#x} outside the tag")))
}

impl BaseScenario {
    pub fn read<R: Read + Seek>(base: &mut Map<R>) -> Result<Self> {
        let t = base
            .scenario()
            .ok_or_else(|| invalid("the base has no scenario"))?
            .clone();
        let address = t
            .location
            .ok_or_else(|| invalid("the base scenario has no data"))?
            .address;
        let bytes = base.read_tag_bytes(t.id)?;
        let mut s = Self {
            id: t.id,
            address,
            bytes,
            descriptors: Vec::new(),
            tree: None,
            refusal: None,
            keep: Vec::new(),
            sky_templates: h1convert::sky_shaders::sky_templates(base),
        };
        match s.relocate(base)? {
            Ok((t, keep)) => {
                s.tree = Some(t);
                s.keep = keep;
            }
            Err(why) => s.refusal = Some(why),
        }
        s.descriptors = s
            .elements(BSPS, BSP)?
            .iter()
            .map(|e| Descriptor {
                sbsp: TagId(u32::from_le_bytes(e[0x14..0x18].try_into().expect("4"))),
                ltmp: TagId(u32::from_le_bytes(e[0x1c..0x20].try_into().expect("4"))),
            })
            .collect();
        Ok(s)
    }

    /// The base scenario as a relocatable tree, or why not: `tags::schema`
    /// refuses it, or another tag points into it. The fields `write`
    /// replaces are emptied first, so neither their old contents (the BSP
    /// descriptors' region words point into BSP space, which the walk
    /// refuses) nor their bytes come along.
    fn relocate<R: Read + Seek>(
        &self,
        base: &mut Map<R>,
    ) -> Result<std::result::Result<Relocated, String>> {
        let mut r = tags::schema::Relocator::new(tags::schema::Schema::builtin(), base)
            .map_err(|e| invalid(format!("base scenario: {e}")))?;
        // Other tags' arrays inside the old scenario bytes stay put.
        let mut keep: Vec<(u32, u32)> = r
            .pointed_into(self.id)
            .into_iter()
            .map(|(_, a, n)| (a, a + n))
            .collect();
        keep.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for (lo, hi) in keep {
            match merged.last_mut() {
                Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        for at in replaced_fields() {
            r.memory_mut().patch(self.address + at, &[0; 8]);
        }
        Ok(r.to_built_emptied(self.id)
            .map(|b| (b, merged))
            .map_err(|d| {
                format!(
                    "{:?} at field path {:x?} +{:#x} ({:#x})",
                    d.refusal, d.path, d.offset, d.value
                )
            }))
    }

    /// The elements of the root's block field `at`.
    pub fn elements(&self, at: u32, stride: usize) -> Result<Vec<&[u8]>> {
        let count = u32_at(&self.bytes, at as usize)? as usize;
        if count == 0 {
            return Ok(Vec::new());
        }
        let first = u32_at(&self.bytes, at as usize + 4)?.wrapping_sub(self.address) as usize;
        let end = first
            .checked_add(count * stride)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| invalid(format!("scenario block {at:#x} outside the tag")))?;
        Ok(self.bytes[first..end].chunks(stride).collect())
    }
}

/// What the scenario layers wrote.
#[derive(Debug, Default, Clone)]
pub struct ScenarioReport {
    /// BSP descriptors written (Halo 1's BSP count).
    pub bsps: usize,
    /// Halo 1's BSP switch trigger volumes written.
    pub bsp_switches: usize,
    /// Halo 1 palette entries placed as a retail object a donor brought:
    /// (Halo 1 tag name, retail datum).
    pub stand_ins: Vec<(String, TagId)>,
    /// Skies converted (Halo 1 tag names).
    pub skies: Vec<String>,
    /// Player starting locations written.
    pub starts: usize,
    /// Per placed kind: (kind, placed, Halo 1 placements, palette entries
    /// converted, palette entries).
    pub placements: Vec<(&'static str, usize, usize, usize, usize)>,
    /// Object tags added by `h1objects` (models, collisions, animations,
    /// projectiles included).
    pub object_tags: usize,
    /// Sounds and looping sounds the objects' effects and attachments play
    /// (`h1objects::convert::Tree::install_audio`), installed with the
    /// gestalt, and their sample bytes.
    pub object_sounds: usize,
    pub object_loops: usize,
    pub object_sound_bytes: u64,
    /// Of those, the sample bytes of the sounds only the rebuilt bipeds'
    /// dialogue plays (within the size ladder's dialogue budget).
    pub dialogue_bytes: u64,
    /// The player biped, converted and set as the campaign players' unit.
    pub player_biped: Option<String>,
    /// HUD pairs `boot::hud` switches between (owner, Halo 1 HUD, Halo 2
    /// HUD; docs/game/hud.md), as this set's datums.
    pub hud_swaps: Vec<[TagId; 3]>,
    /// Where the player's Halo 1 HUD centres the motion sensor's blips
    /// (`h1objects::h1hud::sensor_centre`), for `boot::hud_swap`.
    pub hud_sensor: Option<[i16; 2]>,
    /// `None` when the scenario was rebuilt from the base's relocated tree
    /// (the base scenario's old bytes freed); else why it was written over
    /// them.
    pub base_tree: Option<String>,
    /// Bytes of the old scenario kept for other tags' arrays.
    pub base_kept: u32,
    /// The scenario's largest root fields, as (offset, bytes of the
    /// field's tree).
    pub fields: Vec<(u32, u64)>,
    /// Object names written.
    pub names: usize,
    /// Base trigger volumes parked.
    pub parked_volumes: usize,
    /// Simulation table entries (fresh levels).
    pub simulation: usize,
    /// Halo 1 player starting profiles written.
    pub profiles: usize,
    /// Globals grenade types written from Halo 1's
    /// (`h1convert::globals::set_grenades`).
    pub grenades: usize,
    /// AI: squads, zones, characters (and substitutions made).
    pub ai: Option<AiReport>,
    /// Scripts: scripts, globals, nodes, hooks resolved and left.
    pub scripts: Option<ScriptReport>,
}

#[derive(Debug, Default, Clone)]
pub struct AiReport {
    pub squad_groups: usize,
    pub squads: usize,
    pub zones: usize,
    pub orders: usize,
    pub characters: Vec<(String, String)>,
    pub notes: usize,
}

#[derive(Debug, Default, Clone)]
pub struct ScriptReport {
    pub scripts: usize,
    pub globals: usize,
    pub nodes: usize,
    pub hooks: usize,
    /// Hooks left `none`, by Halo 2 type name.
    pub unresolved: Vec<(String, usize)>,
    /// Hooks the level data, seat table and tag-name lookups resolve (the
    /// rest of `hooks` minus `unresolved` came from the other targets).
    pub resolved_by_level: usize,
    /// A few unresolved tokens per type.
    pub samples: Vec<(String, String)>,
    /// Sounds the scripts name, converted (`h1sounds`), and those left out
    /// (over the cap, or the gestalt's 16-bit indices).
    pub sounds: usize,
    pub sounds_dropped: usize,
    /// Looping sounds (music and ambience) the scripts name, converted with
    /// `h1sounds::looping`, and those left out (not in the level, failed,
    /// or no room: `Lean::Music` leaves them all out, a map over the engine's
    /// file size limit fewer of them), and the sample bytes they added.
    pub looping: usize,
    pub looping_dropped: Vec<String>,
    pub looping_bytes: u64,
    pub missing_calls: usize,
    /// Calls stubbed because a cutscene flag, camera point, title or device
    /// group they name does not resolve (`h1scripts::compile::INDEXED`).
    pub unindexed: Vec<String>,
    pub references: usize,
    pub references_resolved: usize,
    /// Squad groups the startup script places (Halo 1's initially created
    /// encounters).
    pub initial: Vec<String>,
    /// Animation graphs the cutscenes name (crate::cutscenes).
    pub graphs: crate::cutscenes::Graphs,
    /// Recorded animations written (Halo 2 `scnr +0x110`), and their event
    /// streams' bytes.
    pub recordings: usize,
    pub recording_bytes: usize,
    /// The streams' rewrite for Halo 2's player.
    pub recordings_rewritten: crate::recordings::Report,
}

impl std::fmt::Display for ScenarioReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "  BSPs: {}, BSP switch volumes: {}",
            self.bsps, self.bsp_switches
        )?;
        writeln!(f, "  skies: {:?}", self.skies)?;
        for (n, _) in &self.stand_ins {
            writeln!(f, "  placed as the retail object a donor brought: {n}")?;
        }
        writeln!(f, "  player starts: {}", self.starts)?;
        for (k, placed, of, pal, pals) in &self.placements {
            writeln!(
                f,
                "  {k}: {placed} of {of} placed, {pal} of {pals} palette entries converted"
            )?;
        }
        writeln!(
            f,
            "  object tags added: {}, player biped: {:?}, names: {}, base trigger volumes parked: {}",
            self.object_tags, self.player_biped, self.names, self.parked_volumes
        )?;
        writeln!(
            f,
            "  object sounds: {}, looping sounds: {} ({:#x} sample bytes, {:#x} of dialogue)",
            self.object_sounds, self.object_loops, self.object_sound_bytes, self.dialogue_bytes
        )?;
        writeln!(
            f,
            "  scenario: {}",
            match &self.base_tree {
                None => format!(
                    "rebuilt from the base's relocated tree ({:#x} old bytes kept for other tags)",
                    self.base_kept
                ),
                Some(why) => format!("over the base's old bytes ({why})"),
            }
        )?;
        let fields: Vec<String> = self
            .fields
            .iter()
            .map(|(at, n)| format!("{at:#x} {n:#x}"))
            .collect();
        writeln!(f, "  scenario's largest fields: {}", fields.join(", "))?;
        if let Some(a) = &self.ai {
            writeln!(
                f,
                "  ai: {} squad groups, {} squads, {} zones, {} orders, {} notes",
                a.squad_groups, a.squads, a.zones, a.orders, a.notes
            )?;
            for (h1, h2) in &a.characters {
                writeln!(f, "    character {h1} -> {h2}")?;
            }
        }
        if let Some(s) = &self.scripts {
            writeln!(
                f,
                "  scripts: {} scripts, {} globals, {} nodes, {} hooks ({} left none: {:?}; {} by level data, seats and tag names), {} missing calls stubbed, {} of {} references resolved",
                s.scripts,
                s.globals,
                s.nodes,
                s.hooks,
                s.unresolved.iter().map(|u| u.1).sum::<usize>(),
                s.unresolved,
                s.resolved_by_level,
                s.missing_calls,
                s.references_resolved,
                s.references
            )?;
            writeln!(f, "    unresolved samples: {:?}", s.samples)?;
            if !s.unindexed.is_empty() {
                writeln!(
                    f,
                    "    {} calls stubbed for an unresolved flag, camera point, title or device group: {:?}",
                    s.unindexed.len(),
                    s.unindexed
                )?;
            }
            writeln!(
                f,
                "    sounds: {} converted, {} left out; looping sounds: {} converted, {} left out {:?}",
                s.sounds,
                s.sounds_dropped,
                s.looping,
                s.looping_dropped.len(),
                s.looping_dropped
            )?;
            writeln!(
                f,
                "    placed at startup ({INITIAL_SCRIPT}): {:?}",
                s.initial
            )?;
            for r in &s.recordings_rewritten.stilled {
                writeln!(f, "    recording held still: {r}")?;
            }
        }
        Ok(())
    }
}

/// Element bytes of a Halo 1 block, or nothing when the field is empty or
/// outside the tag.
fn h1_block(h1: &H1Map, (at, stride): (usize, usize)) -> Result<Vec<&[u8]>> {
    let scnr = h1.tag_data(h1.scenario())?;
    Ok(h1.tag_space().block_at(scnr, at, stride)?)
}

fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

fn f32s<const N: usize>(b: &[u8], at: usize) -> [f32; N] {
    std::array::from_fn(|i| {
        f32::from_le_bytes(b[at + 4 * i..at + 4 * i + 4].try_into().expect("4"))
    })
}

fn name32(b: &[u8]) -> String {
    let end = b[..32].iter().position(|&c| c == 0).unwrap_or(32);
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Concatenates fixed-size elements into one block child.
fn array(elements: Vec<Block>, stride: usize) -> Block {
    let mut out = Block::zeroed(elements.len() * stride);
    for (i, e) in elements.into_iter().enumerate() {
        out.bytes[i * stride..(i + 1) * stride].copy_from_slice(&e.bytes);
        for mut f in e.fixups {
            f.offset += (i * stride) as u32;
            out.fixups.push(f);
        }
    }
    out
}

/// Replaces the block field `at` of `root` with `elements`.
fn put_block(root: &mut Block, at: u32, stride: usize, elements: Vec<Block>) {
    root.fixups.retain(|f| f.offset != at);
    root.bytes[at as usize..at as usize + 8].fill(0);
    if !elements.is_empty() {
        let n = elements.len() as u32;
        root.block(at, n, array(elements, stride));
    }
}

/// Halo 2's device placement fields after the object fields (`+0x34`):
/// the same as Halo 1's at `+0x28` ([`h1::DEVICE_POWER_GROUP`]), 12 bytes
/// on. **V**: every machine placement on 01b, 03a, 05a and 08a and every
/// control placement on 01b holds -1 or an index below the device group
/// count at `+0x34` and `+0x36`, and device flags of 0 to 3 at `+0x38`.
pub const H2_DEVICE_FIELDS: usize = 0x34;

/// A placed kind: Halo 1 placements and palette and object type, Halo 2
/// object type, and whether it is a device.
struct Kind {
    label: &'static str,
    h1: (usize, usize),
    h1_palette: (usize, usize),
    h1_type: i16,
    h2_type: u16,
    device: Device,
}

#[derive(Clone, Copy, PartialEq)]
enum Device {
    No,
    /// Machines and controls: groups, device flags and their own flags.
    WithFlags,
    /// Light fixtures (placed as machines): groups and device flags.
    Fixture,
}

const KINDS: [Kind; 8] = [
    Kind {
        label: "weapons",
        h1: h1::WEAPONS,
        h1_palette: h1::WEAPON_PALETTE,
        h1_type: 2,
        h2_type: 2,
        device: Device::No,
    },
    Kind {
        label: "equipment",
        h1: h1::EQUIPMENT,
        h1_palette: h1::EQUIPMENT_PALETTE,
        h1_type: 3,
        h2_type: 3,
        device: Device::No,
    },
    Kind {
        label: "vehicles",
        h1: h1::VEHICLES,
        h1_palette: h1::VEHICLE_PALETTE,
        h1_type: 1,
        h2_type: 1,
        device: Device::No,
    },
    Kind {
        label: "scenery",
        h1: h1::SCENERY,
        h1_palette: h1::SCENERY_PALETTE,
        h1_type: 6,
        h2_type: 6,
        device: Device::No,
    },
    Kind {
        label: "bipeds",
        h1: h1::BIPEDS,
        h1_palette: h1::BIPED_PALETTE,
        h1_type: 0,
        h2_type: 0,
        device: Device::No,
    },
    Kind {
        label: "machines",
        h1: h1::MACHINES,
        h1_palette: h1::MACHINE_PALETTE,
        h1_type: 7,
        h2_type: 7,
        device: Device::WithFlags,
    },
    Kind {
        label: "controls",
        h1: h1::CONTROLS,
        h1_palette: h1::CONTROL_PALETTE,
        h1_type: 8,
        h2_type: 8,
        device: Device::WithFlags,
    },
    // Halo 2 has no lifi tags: fixtures join the machines (after them, in
    // both the palette and the placements).
    Kind {
        label: "light fixtures",
        h1: h1::LIGHT_FIXTURES,
        h1_palette: h1::LIGHT_FIXTURE_PALETTE,
        h1_type: 9,
        h2_type: 7,
        device: Device::Fixture,
    },
];

/// The Halo 1 player biped the objects layer converts.
pub const PLAYER_BIPED: &str = "characters\\cyborg\\cyborg";

/// The scenario root fields [`write`] replaces (or empties).
pub fn replaced_fields() -> Vec<u32> {
    use h1ai::h2::layout as ai;
    let mut v = vec![
        SKIES,
        OBJECT_NAMES,
        STARTING_LOCATIONS,
        crate::profiles::H2.0,
        TRIGGER_VOLUMES,
        BSP_SWITCHES,
        BSPS,
        CLUSTER_DATA,
        BSP_POINTS,
        BACKGROUND_SOUNDS,
        SOUND_ENVIRONMENTS,
    ];
    for t in tags::object::OBJECT_TYPES.iter() {
        if let Some((at, pal, _)) = t.placement {
            v.extend([u32::from(at), u32::from(pal)]);
        }
    }
    v.extend(SCRIPT_FIELDS);
    v.extend(h1scripts::level::H2_FIELDS);
    for (at, _) in [
        ai::SQUAD_GROUPS,
        ai::SQUADS,
        ai::ZONES,
        ai::CHARACTER_PALETTE,
        ai::SCRIPTING_DATA,
        ai::ORDERS,
    ] {
        v.push(at as u32);
    }
    v
}

/// Null tag references a fresh scenario root holds, as retail writes
/// them: the group, then datum -1. **V** on 05a, 03a and Coagulation.
pub const FRESH_NULL_REFS: [(u32, [u8; 4]); 4] = [
    (0x000, *b"sbsp"),
    (0x1f8, *b"unic"),
    (0x208, *b"hmt "),
    (0x320, *b"sfx+"),
];

/// The scenario root a fresh level starts from: every field empty, the
/// null references of [`FRESH_NULL_REFS`], and every other root tag
/// reference null. What the base's root held and a fresh level does not
/// take (docs/game/campaigns.md, "Fresh levels"): its light volumes and
/// their palette (`+0xe8`, `+0xf0`), decals and their palette (`+0x138`, `+0x140`), AI styles (`+0x150`;
/// every converted order has style -1), mission scenes (`+0x170`), its
/// chapter title, subtitles and objectives references (`+0x200`, `+0x358`,
/// `+0x3b8`), resources (`+0x218`), kill triggers (`+0x230`, indices of
/// its own trigger volumes), AI triggers (`+0x248`), background sound,
/// sound environment and weather palettes (`+0x250`..`+0x260`; the
/// cluster data names none), object salts (`+0x298`, zero on Coagulation),
/// spawn data (`+0x318`, all zero on 05a), fog and flock blocks (`+0x348`,
/// `+0x350`), decorator blocks (`+0x378`, `+0x380`), level data
/// (`+0x398`) and mission dialogue (`+0x3b0`). The simulation table
/// (`+0x3d8`) is written once the map's tags are known
/// ([`write_simulation_table`]).
pub fn fresh_root() -> Block {
    let mut root = Block::zeroed(ROOT);
    let schema = tags::schema::Schema::builtin();
    for (&at, f) in &schema.groups[&FourCC(*b"scnr")].fields {
        if matches!(f, tags::schema::Field::TagRef) {
            root.put_u32(at as usize, u32::MAX)
                .put_u32(at as usize + 4, u32::MAX);
        }
    }
    for (at, g) in FRESH_NULL_REFS {
        root.put_u32(at as usize, u32::from_be_bytes(g))
            .put_u32(at as usize + 4, u32::MAX);
    }
    root
}

/// Writes the scenario layers into `set` (built from the base), leaving
/// out what `lean` says ([`crate::Lean`]; object textures are left out in
/// `lib.rs`). Unless `lean` leaves out the music, `music` caps the looping
/// sounds' sample bytes (the map file's own bound) and `dialogue` the
/// rebuilt bipeds' dialogue sounds'.
/// `bsps[k]` is Halo 1 BSP `k`, converted.
#[allow(clippy::too_many_arguments)]
pub fn write(
    h1: &H1Map,
    set: &mut TagSet,
    base: &BaseScenario,
    bsps: &[BspSlot],
    sounds: &h1convert::ambience::Installed,
    objects: Option<&h1objects::base::Base>,
    opts: &Options,
    lean: crate::Lean,
    [music, dialogue]: [u64; 2],
    report: &mut Report,
) -> Result<()> {
    // A fresh level's scenario is written from nothing ([`fresh_root`]);
    // a pinned one is the base's with the replaced fields emptied.
    let fresh = !opts.pinned;
    let mut root = match &base.tree {
        _ if fresh => fresh_root(),
        Some(t) => t.clone(),
        None => Block::new(base.bytes[..ROOT].to_vec()),
    };
    let rep = &mut report.scenario;

    // BSP descriptors: exactly Halo 1's BSPs, in order, so its BSP
    // indices (`switch_bsp`, zones, placements) need no map. A BSP past
    // the base's descriptors takes the base's first as its template.
    let templates = base.elements(BSPS, BSP)?;
    let first = templates
        .first()
        .ok_or_else(|| invalid("the base has no BSP descriptor"))?;
    let mut desc = Vec::new();
    for (k, slot) in bsps.iter().enumerate() {
        let mut b = if fresh {
            Block::zeroed(BSP)
        } else {
            Block::new(templates.get(k).unwrap_or(first).to_vec())
        };
        for (at, field) in [
            (0, BspField::FileOffset),
            (4, BspField::Size),
            (8, BspField::Address),
        ] {
            b.fixup(
                at,
                FixupKind::Bsp {
                    section: slot.section,
                    field,
                },
            );
        }
        b.tag_ref(0x10, slot.sbsp).tag_ref(0x18, slot.ltmp);
        desc.push(b);
    }
    rep.bsps = bsps.len();
    put_block(
        &mut root,
        CLUSTER_DATA,
        CLUSTER_DATUM,
        bsps.iter().map(cluster_datum).collect(),
    );
    let points = bsps
        .iter()
        .map(|_| {
            let mut e = Block::zeroed(BSP_POINT);
            e.put_u32(0, u32::from_be_bytes(*b"sbsp"))
                .put_u32(4, u32::MAX);
            e
        })
        .collect();
    put_block(&mut root, BSP_POINTS, BSP_POINT, points);
    for (at, stride, entries, entry) in [
        (
            BACKGROUND_SOUNDS,
            h1convert::ambience::BACKGROUND_SOUND,
            &sounds.backgrounds,
            h1convert::ambience::background_entry as fn(&str, TagId) -> Vec<u8>,
        ),
        (
            SOUND_ENVIRONMENTS,
            h1convert::ambience::SOUND_ENVIRONMENT,
            &sounds.environments,
            h1convert::ambience::environment_entry,
        ),
    ] {
        let elements = entries
            .iter()
            .map(|(n, id)| {
                let mut e = Block::new(entry(n, *id));
                e.tag_ref(0x20, *id);
                e
            })
            .collect();
        put_block(&mut root, at, stride, elements);
    }
    put_block(&mut root, BSPS, BSP, desc);

    // Skies.
    let mp = h1.multiplayer()?;
    let mut skies = Vec::new();
    for r in &mp.skies {
        let tag = h1.tag(r.id)?;
        // Each material names a copy of a base sky shader drawing Halo 1's
        // bitmaps (h1convert::sky_shaders).
        let id = h1convert::sky::Sky::from_map(h1, tag)
            .and_then(|sky| sky.install(set, &base.sky_templates))
            .map_err(|e| invalid(format!("sky: {e}")))?;
        let mut e = Block::zeroed(8);
        e.tag_ref(0, id);
        skies.push(e);
        rep.skies.push(tag.name.clone());
    }
    if !skies.is_empty() {
        put_block(&mut root, SKIES, 8, skies);
    }

    // Player starting locations, on the base's first element.
    let template = base
        .elements(STARTING_LOCATIONS, STARTING_LOCATION)?
        .first()
        .filter(|_| !fresh)
        .map(|e| e.to_vec())
        .unwrap_or_else(|| vec![0; STARTING_LOCATION]);
    let starts: Vec<Block> = mp
        .starting_locations
        .iter()
        .map(|s| {
            let mut b = Block::new(template.clone());
            for (k, v) in s.position.iter().enumerate() {
                b.put_f32(4 * k, *v);
            }
            b.put_f32(0xc, s.facing);
            b
        })
        .collect();
    if starts.is_empty() {
        return Err(invalid("the Halo 1 level has no player starting location"));
    }
    rep.starts = starts.len();
    put_block(&mut root, STARTING_LOCATIONS, STARTING_LOCATION, starts);

    // Every base placement goes; names are Halo 1's.
    for t in tags::object::OBJECT_TYPES.iter() {
        if let Some((at, _, _)) = t.placement {
            put_block(&mut root, u32::from(at), 0, Vec::new());
        }
    }
    let placed = match objects {
        Some(b) => {
            // Device groups are written with the scripts' level data;
            // without them a placement's group index would dangle.
            let groups = opts.scripts.is_some() && !lean.leaves_out(crate::Lean::LevelData);
            place_objects(
                h1,
                set,
                b,
                &mut root,
                rep,
                &mut report.dropped,
                groups,
                opts.devices,
                opts.rebuild_bipeds,
                dialogue,
            )?
        }
        None => HashMap::new(),
    };
    let h1_names = h1_block(h1, h1::OBJECT_NAMES)?;
    let names: Vec<Block> = h1_names
        .iter()
        .map(|e| {
            let mut b = Block::zeroed(OBJECT_NAME);
            b.bytes[..32].copy_from_slice(&e[..32]);
            b.bytes[31] = 0;
            let ty = i16_at(e, 0x20);
            let at = i16_at(e, 0x22);
            // A light fixture's name names the machine it became.
            let (ty, to) = placed.get(&(ty, at)).copied().unwrap_or((ty, -1));
            b.put_u16(0x20, ty as u16);
            b.put_u16(0x22, to as u16);
            b
        })
        .collect();
    rep.names = names.len();
    let names_placed: Vec<bool> = names.iter().map(|b| i16_at(&b.bytes, 0x22) >= 0).collect();
    put_block(&mut root, OBJECT_NAMES, OBJECT_NAME, names);

    // The base's trigger volumes are its own level's: parked, and free of
    // object names (which are now Halo 1's).
    let mut volumes: Vec<Block> = base
        .elements(TRIGGER_VOLUMES, TRIGGER_VOLUME)?
        .iter()
        .filter(|_| !fresh)
        .map(|e| {
            let mut b = Block::new(e.to_vec());
            for (k, v) in h1convert::convert::PARKED.iter().enumerate() {
                b.put_f32(0x24 + 4 * k, *v);
            }
            b.put_u16(4, 0xffff);
            b
        })
        .collect();
    rep.parked_volumes = volumes.len();
    let parked = volumes.len();
    let h1_volumes = crate::scenario::trigger_volumes(h1)?;
    for v in &h1_volumes {
        let mut b = Block::zeroed(TRIGGER_VOLUME);
        b.string_id(0, &v.name);
        b.put_u16(4, 0xffff);
        b.put_u32(8, 0);
        for (k, x) in v
            .forward
            .iter()
            .chain(&v.up)
            .chain(&v.position)
            .chain(&v.extents)
            .enumerate()
        {
            b.put_f32(0xc + 4 * k, *x);
        }
        b.put_u16(0x40, 0xffff);
        volumes.push(b);
    }
    put_block(&mut root, TRIGGER_VOLUMES, TRIGGER_VOLUME, volumes);

    // BSP switch trigger volumes: Halo 1's, on its volumes (after the
    // parked ones) and BSPs (same indices); the base's go.
    let switches: Vec<Block> = h1_block(h1, h1::BSP_SWITCHES)?
        .iter()
        .map(|e| {
            let mut b = Block::zeroed(BSP_SWITCH);
            b.put_u16(0, (i16_at(e, 0) as i32 + parked as i32) as u16);
            b.put_u16(2, i16_at(e, 2) as u16);
            b.put_u16(4, i16_at(e, 4) as u16);
            for at in [6, 8, 0xa, 0xc] {
                b.put_u16(at, 0xffff);
            }
            b
        })
        .collect();
    rep.bsp_switches = switches.len();
    put_block(&mut root, BSP_SWITCHES, BSP_SWITCH, switches);

    // AI.
    let mut ai_src = h1ai::h1::H1Ai::read(h1).map_err(|e| invalid(format!("ai: {e}")))?;
    // h1ai keeps squads with no starting locations (empty) so scripts can
    // name them; only those the scripts name are worth their room.
    let named = h1scripts::Tables::read(h1)
        .map(|t| t.atom_tokens(h1scripts::catalog::types::AI))
        .unwrap_or_default()
        .into_iter()
        .map(|n| n.to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    // They go first when the level would pass the engine's squad cap
    // ([`MAX_SQUADS`]; a50 and b40 would): the scripts then name the
    // squad's encounter, its squad group ([`hook_value`]).
    let empty_named = |e: &h1ai::h1::Encounter, q: &h1ai::h1::Squad| {
        q.starting_locations.is_empty()
            && named.contains(&format!("{}/{}", e.name, q.name).to_ascii_lowercase())
    };
    let placed = ai_src
        .encounters
        .iter()
        .flat_map(|e| &e.squads)
        .filter(|q| !q.starting_locations.is_empty())
        .count();
    let empty = ai_src
        .encounters
        .iter()
        .flat_map(|e| e.squads.iter().map(move |q| (e, q)))
        .filter(|(e, q)| empty_named(e, q))
        .count();
    let keep_empty = !lean.leaves_out(crate::Lean::EmptySquads) && placed + empty <= MAX_SQUADS;
    if !keep_empty && empty > 0 {
        report.dropped.push(format!(
            "ai: {empty} empty squads the scripts name left out ({placed} placed; cap {MAX_SQUADS}); the scripts name their squad groups"
        ));
    }
    for e in &mut ai_src.encounters {
        let encounter = e.name.clone();
        e.squads.retain(|q| {
            !q.starting_locations.is_empty()
                || (keep_empty
                    && named.contains(&format!("{encounter}/{}", q.name).to_ascii_lowercase()))
        });
    }
    let plan = h1ai::convert::convert(&ai_src, &Default::default());
    if plan.ai.squads.len() > MAX_SQUADS || plan.ai.squad_groups.len() > MAX_SQUAD_GROUPS {
        return Err(invalid(format!(
            "ai: {} squads and {} squad groups, the engine holds {MAX_SQUADS} and {MAX_SQUAD_GROUPS}",
            plan.ai.squads.len(),
            plan.ai.squad_groups.len()
        )));
    }
    let mut chars = Vec::new();
    let resolve = |p: &str| -> Option<TagId> {
        let find = |p: &str| {
            set.slots.iter().find_map(|s| {
                let t = s.tag.as_ref()?;
                (t.group == FourCC(*b"char") && s.name.as_deref() == Some(p)).then_some(t.id)
            })
        };
        find(p).or_else(|| {
            // Stand-ins for characters the base lacks: the marine for
            // marine kinds (a30's crewmen), else the folder's first.
            let folder = p.rsplit_once('\\')?.0;
            if p.contains("\\marine\\") {
                find("objects\\characters\\marine\\ai\\marine")
            } else {
                set.slots.iter().find_map(|s| {
                    let t = s.tag.as_ref()?;
                    (t.group == FourCC(*b"char") && s.name.as_deref()?.starts_with(folder))
                        .then_some(t.id)
                })
            }
        })
    };
    for c in &plan.characters {
        let got = resolve(c.h2_character);
        let name = got
            .and_then(|id| set.slots[id.index()].name.clone())
            .unwrap_or_else(|| "none".into());
        chars.push((c.h1_variant.clone(), name));
    }
    let blocks = plan
        .blocks(|p| resolve(p).or(Some(TagId::NULL)))
        .map_err(|e| invalid(format!("ai: {e}")))?;
    if opts.ai {
        blocks
            .apply(&mut root)
            .map_err(|e| invalid(format!("ai: {e}")))?;
        rep.ai = Some(AiReport {
            squad_groups: plan.ai.squad_groups.len(),
            squads: plan.ai.squads.len(),
            zones: plan.ai.zones.len(),
            orders: plan.ai.orders.len(),
            characters: chars,
            notes: plan.notes.len(),
        });
    } else {
        for (at, _, _) in blocks.fields {
            put_block(&mut root, at, 0, Vec::new());
        }
    }

    // Scripts.
    match opts.scripts {
        Some(s) => {
            let names: Vec<String> = h1_names.iter().map(|e| name32(e)).collect();
            let volume_names: Vec<&str> = h1_volumes.iter().map(|v| v.name.as_str()).collect();
            let (squads, groups): (Vec<String>, Vec<String>) = if opts.ai {
                (
                    plan.ai.squads.iter().map(|q| q.name.clone()).collect(),
                    plan.ai
                        .squad_groups
                        .iter()
                        .map(|g| g.name.clone())
                        .collect(),
                )
            } else {
                Default::default()
            };
            // Encounters Halo 1 creates at load (encounter flag bit 0, "not
            // initially created", clear): their squad groups, placed by a
            // startup script.
            let initial: Vec<String> = if opts.ai {
                ai_src
                    .encounters
                    .iter()
                    .zip(&plan.ai.squad_groups)
                    .filter(|(e, _)| e.flags & 1 == 0)
                    .map(|(_, g)| g.name.clone())
                    .collect()
            } else {
                Vec::new()
            };
            rep.scripts = Some(install_scripts(
                h1,
                s,
                set,
                &mut root,
                &Targets {
                    names: &names,
                    placed: &names_placed,
                    volumes: &volume_names,
                    parked,
                    squads: &squads,
                    groups: &groups,
                },
                &initial,
                opts.sounds,
                !lean.leaves_out(crate::Lean::LevelData),
                !lean.leaves_out(crate::Lean::SeatTable),
                if lean.leaves_out(crate::Lean::Music) {
                    0
                } else {
                    music
                },
                (!lean.leaves_out(crate::Lean::CutsceneData) && opts.cutscene_data)
                    .then_some(opts.cutscene_graphs),
                crate::cutscenes::policy(opts, &h1.header().name.to_ascii_lowercase()),
            )?);
        }
        None => {
            for at in SCRIPT_FIELDS {
                put_block(&mut root, at, 0, Vec::new());
            }
        }
    }

    // Without a tree, the old bytes stay where they were, so the untouched
    // fields' children (still raw addresses in the root) stay valid.
    // With one, only the ranges other tags' arrays use stay.
    let pinned = set
        .pinned
        .as_mut()
        .ok_or_else(|| invalid("the set is not from a retail map"))?;
    // A fresh level keeps them all in the intermediate pinned map, so the
    // base tags whose arrays live there unpin from it whole (zero bytes
    // included); none of it reaches the fresh map.
    if base.tree.is_none() || fresh {
        pinned.unowned.push((base.address, base.bytes.clone()));
    } else {
        for &(lo, hi) in &base.keep {
            let (a, b) = ((lo - base.address) as usize, (hi - base.address) as usize);
            pinned
                .unowned
                .push((lo, base.bytes[a..b.min(base.bytes.len())].to_vec()));
        }
        rep.base_kept = base.keep.iter().map(|(lo, hi)| hi - lo).sum();
    }
    rep.base_tree = base.refusal.clone();
    let mut fields: Vec<(u32, u64)> = root
        .fixups
        .iter()
        .filter_map(|f| match &f.kind {
            FixupKind::Block { child, .. } | FixupKind::Data(child) => {
                Some((f.offset, child.tree_len()))
            }
            _ => None,
        })
        .collect();
    fields.sort_by_key(|f| std::cmp::Reverse(f.1));
    fields.truncate(12);
    rep.fields = fields;
    set.tag_mut(base.id)
        .ok_or_else(|| invalid("no scenario"))?
        .payload = TagPayload::Built(root);
    Ok(())
}

/// A palette entry: a tree tag `h1objects` converted, or a retail tag
/// already in the set.
#[derive(Clone, Copy)]
enum Pal {
    Converted(TagId),
    Retail(TagId),
}

/// The retail object in `set` whose `h1objects` template row matches the
/// Halo 1 object `tag` (a template the base lacks but a donor brought).
fn retail_stand_in(set: &TagSet, tag: &h1map::Tag) -> Option<TagId> {
    let group = tag.group.0.to_be_bytes();
    h1objects::convert::template_candidates(&group, &tag.name)
        .into_iter()
        .find_map(|n| set.find(FourCC(group), n))
}

/// Converts the palettes' objects, installs them, and writes the
/// placements. Returns, per (Halo 1 object type, Halo 1 placement), the
/// Halo 2 object type and placement index (a light fixture becomes a
/// machine). `device_groups`: the level's device groups are written, so
/// device placements may index them. `devices`: which devices are
/// placed ([`crate::Devices`]).
#[allow(clippy::too_many_arguments)]
fn place_objects(
    h1: &H1Map,
    set: &mut TagSet,
    base: &h1objects::base::Base,
    root: &mut Block,
    rep: &mut ScenarioReport,
    dropped: &mut Vec<String>,
    device_groups: bool,
    devices: crate::Devices,
    rebuild_bipeds: bool,
    dialogue: u64,
) -> Result<HashMap<(i16, i16), (i16, i16)>> {
    let mut conv = h1objects::convert::Converter::new(h1, base);
    conv.options.rebuild_bipeds = rebuild_bipeds;
    conv.options.dialogue_budget = dialogue;
    let mut palettes = Vec::new();
    for k in &KINDS {
        let mut pal = Vec::new();
        let left_out = match k.device {
            Device::No => false,
            Device::Fixture => devices != crate::Devices::All,
            Device::WithFlags => devices == crate::Devices::None,
        };
        if left_out {
            let n = h1_block(h1, k.h1)?.len();
            if n > 0 {
                dropped.push(format!("{}: {n} left out to fit ({devices:?})", k.label));
            }
            palettes.push(pal);
            continue;
        }
        for e in h1_block(h1, k.h1_palette)? {
            let r = h1map::TagRef::read(e, 0).ok_or_else(|| invalid("palette reference"))?;
            let got = if r.id.is_none() {
                None
            } else {
                let tag = h1.tag(r.id)?;
                match conv.object(tag) {
                    Ok(id) => Some(Pal::Converted(id)),
                    // A template a donor brought (`crate::donor`): placed
                    // as the retail object itself, since `h1objects`
                    // builds only on the base's own templates.
                    Err(e) => match matches!(e, h1objects::Error::NoTemplate(_))
                        .then(|| retail_stand_in(set, tag))
                        .flatten()
                    {
                        Some(id) => {
                            rep.stand_ins.push((tag.name.clone(), id));
                            Some(Pal::Retail(id))
                        }
                        None => {
                            dropped.push(format!("{} palette {}: {e}", k.label, tag.name));
                            None
                        }
                    },
                }
            };
            pal.push(got);
        }
        palettes.push(pal);
    }
    // Starting profiles' weapons, through the same converter.
    let profiles = crate::profiles::read(h1)?;
    let mut profile_weapons = HashMap::new();
    for w in profiles
        .iter()
        .flat_map(|p| p.weapons.iter().filter_map(|w| w.0))
    {
        let tag = h1.tag(w)?;
        let got = match conv.object(tag) {
            Ok(id) => Some(Pal::Converted(id)),
            Err(h1objects::Error::NoTemplate(_)) => retail_stand_in(set, tag).map(Pal::Retail),
            Err(e) => {
                dropped.push(format!("starting profile weapon {}: {e}", tag.name));
                None
            }
        };
        if let Some(p) = got {
            profile_weapons.insert(w, p);
        }
    }
    let mut player = None;
    rep.player_biped = match h1.find(h1map::Group::new(b"bipd"), PLAYER_BIPED) {
        Some(t) => match conv.object(t) {
            Ok(id) => {
                player = Some(id);
                Some(t.name.clone())
            }
            Err(e) => {
                dropped.push(format!("player biped {}: {e}", t.name));
                None
            }
        },
        None => None,
    };
    // Halo 1's grenade types (the globals), through the same converter.
    let mut grenades = Vec::new();
    for g in h1convert::globals::h1_grenades(h1).map_err(|e| invalid(format!("grenades: {e}")))? {
        let mut one = |id: Option<h1map::TagId>, what: &str| -> Option<TagId> {
            let tag = h1.tag(id?).ok()?;
            match conv.object(tag) {
                Ok(id) => Some(id),
                Err(e) => {
                    dropped.push(format!("grenade {what} {}: {e}", tag.name));
                    None
                }
            }
        };
        grenades.push(h1convert::globals::Grenade {
            maximum: g.maximum,
            equipment: one(g.equipment, "equipment"),
            projectile: one(g.projectile, "projectile"),
        });
    }
    rep.dialogue_bytes = conv.dialogue_bytes;
    let mut tree = std::mem::take(&mut conv.tree);
    rep.object_tags = tree.tags.len();
    let hud_swaps = std::mem::take(&mut tree.hud_swaps);
    // The sounds the objects' effects and attachments play, with the
    // gestalt, before the tags that name them.
    rep.object_sounds = tree.sounds.len();
    rep.object_loops = tree.loops.len();
    let raw_before: u64 = set.raw.iter().map(|r| r.bytes.len() as u64).sum();
    tree.install_audio(set, true);
    rep.object_sound_bytes = set.raw.iter().map(|r| r.bytes.len() as u64).sum::<u64>() - raw_before;
    dropped.extend(tree.audio_notes.drain(..).map(|n| format!("object sounds: {n}")));
    rep.hud_sensor = tree.hud_sensor;
    let ids = tree
        .install(set)
        .map_err(|e| invalid(format!("objects: {e}")))?;
    let real = |t: TagId| ids[(t.0 & 0xffff) as usize];
    // A pair's Halo 2 HUD may be a base-map tag (a weapon's counterpart's).
    let resolve = |t: TagId| h1objects::convert::local_index(t).map_or(t, |k| ids[k]);
    rep.hud_swaps = hud_swaps
        .iter()
        .map(|s| [resolve(s.owner), resolve(s.halo1), resolve(s.halo2)])
        .collect();
    let palette_id = |p: &Pal| match *p {
        Pal::Converted(t) => real(t),
        Pal::Retail(t) => t,
    };
    // The campaign players (globals representation 0 and 1) are the Halo 1
    // cyborg; Halo 1 health follows from the map name at boot.
    if let Some(p) = player {
        h1convert::globals::set_player_unit(set, real(p), h1convert::globals::Players::Campaign)
            .map_err(|e| invalid(format!("player unit: {e}")))?;
    }

    let grenades: Vec<_> = grenades
        .iter()
        .map(|g| h1convert::globals::Grenade {
            maximum: g.maximum,
            equipment: g.equipment.map(real),
            projectile: g.projectile.map(real),
        })
        .collect();
    rep.grenades = h1convert::globals::set_grenades(set, &grenades)
        .map_err(|e| invalid(format!("grenades: {e}")))?;

    let profile_blocks =
        crate::profiles::elements(&profiles, |w| profile_weapons.get(&w).map(palette_id));
    rep.profiles = profile_blocks.len();
    let (at, stride) = crate::profiles::H2;
    put_block(root, at, stride, profile_blocks);

    let mut placed = HashMap::new();
    let mut unique = 0u32;
    let groups = if device_groups {
        h1_block(h1, h1scripts::level::h1::DEVICE_GROUPS)?.len()
    } else {
        0
    };
    // Per Halo 2 type: palette entries, placements, and the kinds in them.
    let mut blocks: Vec<(u16, Vec<Block>, Vec<Block>)> = Vec::new();
    for (k, pal) in KINDS.iter().zip(&palettes) {
        let slot = match blocks.iter().position(|b| b.0 == k.h2_type) {
            Some(i) => i,
            None => {
                blocks.push((k.h2_type, Vec::new(), Vec::new()));
                blocks.len() - 1
            }
        };
        let (_, entries, out) = &mut blocks[slot];
        let first = entries.len();
        let ty = tags::object::OBJECT_TYPES[usize::from(k.h2_type)];
        let (_, _, stride) = ty.placement.expect("placeable");
        entries.extend(pal.iter().map(|p| {
            let mut b = Block::zeroed(PALETTE);
            b.tag_ref(0, p.as_ref().map_or(TagId::NULL, palette_id));
            b
        }));
        let converted = pal.iter().filter(|p| p.is_some()).count();
        let src = h1_block(h1, k.h1)?;
        let before = out.len();
        for (i, e) in src.iter().enumerate() {
            let p = i16_at(e, 0);
            if p < 0 || pal.get(p as usize).copied().flatten().is_none() {
                continue;
            }
            let mut b = Block::zeroed(usize::from(stride));
            b.put_u16(0, (first + p as usize) as u16);
            b.put_u16(2, i16_at(e, 2) as u16);
            // Halo 1's "not placed automatically" (bit 0 of the u16 at +4)
            // is Halo 2's "not at start" (bit 0 of the u32 at +4): objects
            // only the scripts create, the cutscene casts among them.
            b.put_u32(4, u32::from(i16_at(e, 4) as u16 & h1::NOT_AUTOMATIC));
            for (j, v) in f32s::<6>(e, 8).iter().enumerate() {
                b.put_f32(8 + 4 * j, *v);
            }
            b.put_u32(0x28, UNIQUE_ID_HIGH << 16 | unique);
            unique += 1;
            b.put_u16(0x2c, 0xffff);
            b.bytes[0x2e] = k.h2_type as u8;
            b.bytes[0x2f] = 1;
            b.put_u16(0x32, 0xffff);
            if k.device != Device::No {
                write_device(&mut b, e, k.device, groups);
            }
            placed.insert((k.h1_type, i as i16), (k.h2_type as i16, out.len() as i16));
            out.push(b);
        }
        let n = out.len() - before;
        if n < src.len() {
            dropped.push(format!(
                "{}: {} of {} placements (their palette entry did not convert)",
                k.label,
                src.len() - n,
                src.len()
            ));
        }
        rep.placements
            .push((k.label, n, src.len(), converted, pal.len()));
    }
    for (h2_type, entries, out) in blocks {
        let ty = tags::object::OBJECT_TYPES[usize::from(h2_type)];
        let (at, pal_at, stride) = ty.placement.expect("placeable");
        put_block(root, u32::from(pal_at), PALETTE, entries);
        put_block(root, u32::from(at), usize::from(stride), out);
    }
    Ok(placed)
}

/// The device fields of a placement ([`H2_DEVICE_FIELDS`]) from Halo 1's
/// element `e` ([`h1::DEVICE_POWER_GROUP`]). A group index past the
/// level's `groups` device groups is none (-1); none is on the disc.
fn write_device(b: &mut Block, e: &[u8], device: Device, groups: usize) {
    let group = |at| {
        let g = i16_at(e, at);
        if g >= 0 && (g as usize) < groups {
            g
        } else {
            -1
        }
    };
    b.put_u16(H2_DEVICE_FIELDS, group(h1::DEVICE_POWER_GROUP) as u16);
    b.put_u16(
        H2_DEVICE_FIELDS + 2,
        group(h1::DEVICE_POSITION_GROUP) as u16,
    );
    b.put_u32(
        H2_DEVICE_FIELDS + 4,
        u32::from_le_bytes(
            e[h1::DEVICE_FLAGS..h1::DEVICE_FLAGS + 4]
                .try_into()
                .unwrap(),
        ),
    );
    if device == Device::WithFlags {
        b.put_u32(
            H2_DEVICE_FIELDS + 8,
            u32::from_le_bytes(
                e[h1::DEVICE_OWN_FLAGS..h1::DEVICE_OWN_FLAGS + 4]
                    .try_into()
                    .unwrap(),
            ),
        );
    }
}

/// A Halo 1 trigger volume.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerVolume {
    pub name: String,
    pub forward: [f32; 3],
    pub up: [f32; 3],
    pub position: [f32; 3],
    pub extents: [f32; 3],
}

/// Halo 1 trigger volumes ([`h1_volumes::FIELD`]).
pub fn trigger_volumes(h1: &H1Map) -> Result<Vec<TriggerVolume>> {
    Ok(h1_block(h1, h1_volumes::FIELD)?
        .iter()
        .map(|e| TriggerVolume {
            name: name32(&e[4..]),
            forward: f32s(e, 0x30),
            up: f32s(e, 0x3c),
            position: f32s(e, 0x48),
            extents: f32s(e, 0x54),
        })
        .collect())
}

/// Where Halo 1 keeps trigger volumes.
pub mod h1_volumes {
    /// Field offset and element size: `scnr+0x360`, `0x60` each, the
    /// name at `+4`. **V** on a30: the only `0x60`-stride block of
    /// printable names that the scripts' trigger volume tokens name;
    /// the frame (forward and up unit vectors, position, extents from
    /// `+0x30`) is checked by `h1_trigger_volumes_read`.
    pub const FIELD: (usize, usize) = (0x360, 0x60);
}

/// What script hooks resolve against.
/// The startup script that places the encounters Halo 1 creates at load.
pub const INITIAL_SCRIPT: &str = "h1_initial_encounters";

pub struct Targets<'a> {
    /// Object names, in the scenario's order.
    pub names: &'a [String],
    /// Whether each of `names` points at a converted placement (a name
    /// whose palette entry did not convert points at -1).
    pub placed: &'a [bool],
    /// Halo 1 trigger volume names, written after `parked` base volumes.
    pub volumes: &'a [&'a str],
    pub parked: usize,
    /// The converted AI's squad and squad group names (empty when the AI
    /// isn't written).
    pub squads: &'a [String],
    pub groups: &'a [String],
}

/// Halo 2 teams by Halo 1 name (the first six agree, S).
const TEAMS: [&str; 6] = [
    "default", "player", "human", "covenant", "flood", "sentinel",
];
/// Halo 1 difficulty names in Halo 2's order.
const DIFFICULTIES: [[&str; 2]; 4] = [
    ["easy", "easy"],
    ["normal", "normal"],
    ["hard", "heroic"],
    ["impossible", "legendary"],
];

/// The data word a hook gets, by the encodings retail Halo 2 scripts use
/// (V, every atom of these types in 05a and 03a, printed by the survey
/// test): object and unit names, `short`, team and difficulty values are
/// `0xffff0000 | index`; trigger volumes are the bare index; an AI squad is
/// its index and a squad group `0x40000000 | index`. Names match without
/// case, as HaloScript's do.
pub fn hook_value(h: &h1scripts::compile::Hook, t: &Targets) -> Option<u32> {
    let short = |i: usize| Some(0xffff_0000 | (i as u32 & 0xffff));
    if h.token == "none" {
        return Some(u32::MAX);
    }
    match h.type_name.as_str() {
        "object_name" | "unit_name" | "vehicle_name" | "weapon_name" | "device_name"
        | "scenery_name" | "object" | "unit" | "vehicle" => short(
            t.names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(&h.token))?,
        ),
        "trigger_volume" => t
            .volumes
            .iter()
            .position(|n| n.eq_ignore_ascii_case(&h.token))
            .map(|i| (t.parked + i) as u32),
        // Adapter defaults read `<builtin>:<value>`.
        "short" => short(h.token.rsplit(':').next()?.trim().parse::<i16>().ok()? as u16 as usize),
        "team" => short(
            TEAMS
                .iter()
                .position(|n| n.eq_ignore_ascii_case(&h.token))?,
        ),
        "game_difficulty" => short(
            DIFFICULTIES
                .iter()
                .position(|n| n.iter().any(|n| n.eq_ignore_ascii_case(&h.token)))?,
        ),
        "ai" => {
            let token: String = h.token.chars().take(31).collect();
            if let Some(i) = t.squads.iter().position(|q| q.eq_ignore_ascii_case(&token)) {
                return Some(i as u32);
            }
            let group = |g: &str| {
                t.groups
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(g))
                    .map(|i| 0x4000_0000 | i as u32)
            };
            // A squad left out (empty, over the squad cap): the first
            // squad of its encounter, else the encounter's squad group.
            // Halo 2's `ai_migrate` ignores a destination with either top
            // bit set (`0x002a54cc`, `test ecx,0C0000000h`) and
            // `ai_attach_units` reads the low 16 bits as a squad index
            // (`0x0027305e`), so a squad group there would do nothing or
            // name another squad; a sibling keeps the actors in their
            // encounter's zone, as Halo 1's migration did.
            group(&token).or_else(|| {
                let (encounter, _) = h.token.split_once('/')?;
                let prefix = format!("{encounter}/").to_ascii_lowercase();
                t.squads
                    .iter()
                    .position(|q| q.to_ascii_lowercase().starts_with(&prefix))
                    .map(|i| i as u32)
                    .or_else(|| group(&encounter.chars().take(31).collect::<String>()))
            })
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn install_scripts(
    h1: &H1Map,
    s: &crate::Scripting,
    set: &mut TagSet,
    root: &mut Block,
    targets: &Targets,
    initial: &[String],
    max_sounds: usize,
    level_data: bool,
    seat_table: bool,
    music: u64,
    cutscene_data: Option<bool>,
    cutscenes: h1scripts::compile::Cutscenes,
) -> Result<ScriptReport> {
    use h1scripts::compile::{Missing, compile_with};
    let t = h1scripts::Tables::read(h1).map_err(|e| invalid(format!("scripts: {e}")))?;
    let refs: Vec<String> = t
        .references
        .iter()
        .map(|r| h1.tag(r.id).map(|t| t.name.clone()).unwrap_or_default())
        .collect();
    // Level data the scripts name (cutscene flags, camera points, titles,
    // device groups), in h1scripts. Without it (`Lean::LevelData`), or for a name
    // it lacks, a call naming one is stubbed: the engine indexes these
    // blocks unchecked (`h1scripts::compile::INDEXED`).
    let level = if level_data {
        h1scripts::level::Level::read(h1).map_err(|e| invalid(format!("scripts: {e}")))?
    } else {
        Default::default()
    };
    let resolves = |ty: &str, token: &str| {
        // An object name handed to a builtin that does not check for none
        // (`h1scripts::compile::NONE_UNSAFE`) must name a placement.
        if h1scripts::compile::OBJECT_NAME_TYPES.contains(&ty) {
            return targets
                .names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(token))
                .is_some_and(|i| targets.placed.get(i).copied().unwrap_or(false));
        }
        level
            .resolve(&h1scripts::compile::Hook {
                slot: 0,
                h2_type: 0,
                type_name: ty.to_owned(),
                h1_type: String::new(),
                token: token.to_owned(),
            })
            .is_some()
    };
    let mut c = compile_with(
        &t,
        &s.catalog,
        &s.table,
        &s.target,
        Missing::Stub,
        cutscenes,
        &resolves,
        &refs,
    )
    .map_err(|e| invalid(format!("scripts: {e}")))?;
    h1scripts::compile::add_startup_placements(&mut c, &s.target, INITIAL_SCRIPT, initial)
        .map_err(|e| invalid(format!("scripts: {e}")))?;
    let (sounds, sounds_dropped) = install_named_sounds(h1, set, &c, max_sounds)?;
    // The looping sounds (`lsnd`: music and ambience) the scripts start,
    // found by the tag-name lookup below.
    let (looping, looping_dropped, looping_bytes) = install_looping_sounds(h1, set, &c, music);
    // Animation graphs the cutscenes name, and their animation names
    // (crate::cutscenes).
    // Recorded animations (Halo 2 `scnr +0x110`), in Halo 1's order. Both
    // are left out at `Lean::CutsceneData`.
    // `cutscene_data` is `None` without either, `Some(false)` without the
    // graphs.
    let graphs = match cutscene_data {
        Some(true) => crate::cutscenes::install_graphs(h1, set, &c)?,
        _ => Default::default(),
    };
    let mut recordings = match cutscene_data {
        Some(_) => crate::cutscenes::read_recordings(h1)?,
        None => Vec::new(),
    };
    // Halo 1's event streams in Halo 2's encoding (crate::recordings).
    let rewritten = crate::recordings::for_halo2(&mut recordings);
    put_block(
        root,
        crate::cutscenes::H2_RECORDINGS,
        crate::cutscenes::H2_RECORDING_LEN,
        crate::cutscenes::recordings_block(&recordings),
    );
    let mut interned: HashMap<String, u32> = HashMap::new();
    for h in &c.hooks {
        if h.type_name == "string_id" && !interned.contains_key(&h.token) {
            let text = match h.h1_type == h1scripts::compile::ANIMATION_NAME {
                true => crate::cutscenes::animation_name(&graphs, h),
                false => h.token.clone(),
            };
            interned.insert(h.token.clone(), set.intern(&text)?);
        }
    }
    // The seat-mapping table, in h1scripts.
    let seats = if seat_table {
        h1scripts::seats::Table::build(
            c.hooks
                .iter()
                .filter(|h| h.type_name == "unit_seat_mapping")
                .map(|h| h.token.as_str()),
            &h1scripts::seats::units(set),
        )
    } else {
        Default::default()
    };
    for (at, stride, elements) in level.blocks() {
        put_block(root, at, stride, elements);
    }
    let (at, stride) = h1scripts::seats::MAPPINGS;
    put_block(root, at, stride, seats.elements());
    let tags_by_name: HashMap<(FourCC, String), TagId> = set
        .slots
        .iter()
        .filter_map(|s| {
            let t = s.tag.as_ref()?;
            Some(((t.group, s.name.as_ref()?.to_ascii_lowercase()), t.id))
        })
        .collect();
    let mut resolved_by_level = 0;
    let left = c.resolve_hooks(|h| {
        match h.type_name.as_str() {
            "string_id" => interned.get(&h.token).copied(),
            "sound" => sounds.get(&h.token).map(|id| id.0),
            _ if hook_value(h, targets).is_some() => hook_value(h, targets),
            "unit_seat_mapping" => seats.values.get(&h.token).copied(),
            "cutscene_recording" => crate::cutscenes::recording_index(&recordings, &h.token),
            ty => level.resolve(h).or_else(|| {
                let g = h1scripts::level::tag_group(ty)?;
                tags_by_name
                    .get(&(g, h.token.to_ascii_lowercase()))
                    .map(|id| id.0)
            }),
        }
        .inspect(|_| {
            resolved_by_level += usize::from(
                !matches!(h.type_name.as_str(), "string_id" | "sound")
                    && hook_value(h, targets).is_none(),
            );
        })
    });
    let mut unresolved: Vec<(String, usize)> = Vec::new();
    let mut samples: Vec<(String, String)> = Vec::new();
    for h in &left {
        match unresolved.iter_mut().find(|u| u.0 == h.type_name) {
            Some(u) => u.1 += 1,
            None => unresolved.push((h.type_name.clone(), 1)),
        }
        if samples.iter().filter(|s| s.0 == h.type_name).count() < 3
            && !samples.iter().any(|s| s.1 == h.token)
        {
            samples.push((h.type_name.clone(), h.token.clone()));
        }
    }
    let by_name: HashMap<String, (FourCC, TagId)> = set
        .slots
        .iter()
        .filter_map(|s| {
            let t = s.tag.as_ref()?;
            Some((s.name.clone()?, (t.group, t.id)))
        })
        .collect();
    let mut resolved = 0;
    for at in SCRIPT_FIELDS {
        put_block(root, at, 0, Vec::new());
    }
    c.install(root, |_, name| {
        let r = by_name.get(name).copied();
        resolved += usize::from(r.is_some());
        r
    })
    .map_err(|e| invalid(format!("scripts: {e}")))?;
    Ok(ScriptReport {
        scripts: c.scripts.len(),
        globals: c.globals.len(),
        nodes: c.nodes.len(),
        hooks: c.hooks.len(),
        unresolved,
        resolved_by_level,
        samples,
        sounds: sounds.len(),
        sounds_dropped,
        looping: looping.len(),
        looping_dropped,
        looping_bytes,
        missing_calls: c.missing.len(),
        unindexed: c
            .unindexed
            .iter()
            .map(|u| format!("{}: {}", u.definition, u.name))
            .collect(),
        references: c.references.len(),
        references_resolved: resolved,
        initial: initial.to_vec(),
        graphs,
        recordings: recordings.len(),
        recording_bytes: recordings.iter().map(|r| r.stream.len()).sum(),
        recordings_rewritten: rewritten,
    })
}

/// Converts the `snd!` tags the scripts name (sound hooks and script
/// references), at most `max` of them, and installs them under their
/// Halo 1 names with the base's gestalt. When the gestalt's 16-bit indices
/// overflow, half as many are tried, down to none. Returns each installed
/// sound's datum by name, and how many were left out.
/// The Halo 1 `lsnd` tags the scripts name (`looping_sound` hooks), in
/// first-use order.
fn looping_names(h1: &H1Map, c: &h1scripts::compile::Compiled) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for h in c.hooks.iter().filter(|h| h.type_name == "looping_sound") {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&h.token))
            && h1.find(h1sounds::looping::LSND, &h.token).is_some()
        {
            names.push(h.token.clone());
        }
    }
    names
}

/// Converts and installs the looping sounds the scripts name, in first-use
/// order while their samples fit in `budget` bytes (0 installs none).
/// Returns the installed tags, per sound left out why, and the sample bytes
/// added. The ones that fit go in at once or none do (a failure leaves
/// `set` as it was).
fn install_looping_sounds(
    h1: &H1Map,
    set: &mut TagSet,
    c: &h1scripts::compile::Compiled,
    budget: u64,
) -> (Vec<TagId>, Vec<String>, u64) {
    let mut loops = Vec::new();
    let mut dropped = Vec::new();
    let mut counted: Vec<String> = Vec::new();
    let mut bytes = 0u64;
    for n in looping_names(h1, c) {
        if budget == 0 {
            dropped.push(format!("{n}: no room"));
            continue;
        }
        let tag = h1.find(h1sounds::looping::LSND, &n).expect("found above");
        let l = match h1sounds::looping::read_looping(h1, tag) {
            Ok(l) => l,
            Err(e) => {
                dropped.push(format!("{n}: {e}"));
                continue;
            }
        };
        let new: u64 = l
            .sounds
            .iter()
            .filter(|(s, _)| !counted.contains(s))
            .map(|(_, s)| sample_bytes(s))
            .sum();
        if bytes + new > budget {
            dropped.push(format!("{n}: over the map file's size limit"));
            continue;
        }
        bytes += new;
        counted.extend(l.sounds.iter().map(|(s, _)| s.clone()));
        loops.push(l);
    }
    let mut trial = set.clone();
    match h1sounds::looping::install_looping(&mut trial, &loops) {
        Ok(ids) => {
            *set = trial;
            (ids, dropped, bytes)
        }
        Err(e) => {
            dropped.extend(loops.iter().map(|l| format!("{}: {e}", l.name)));
            (Vec::new(), dropped, 0)
        }
    }
}

/// A converted sound's sample bytes in the map file, each chunk rounded up
/// to the builder's 0x1000 alignment at most.
pub(crate) fn sample_bytes(s: &h1sounds::ConvertedSound) -> u64 {
    s.ranges
        .iter()
        .flat_map(|r| &r.permutations)
        .flat_map(|p| &p.chunks)
        .map(|c| (c.len() as u64).next_multiple_of(0x1000))
        .sum()
}

fn install_named_sounds(
    h1: &H1Map,
    set: &mut TagSet,
    c: &h1scripts::compile::Compiled,
    max: usize,
) -> Result<(HashMap<String, TagId>, usize)> {
    let snd = h1map::Group::new(b"snd!");
    let mut names: Vec<String> = Vec::new();
    let wanted = c
        .hooks
        .iter()
        .filter(|h| h.type_name == "sound")
        .map(|h| h.token.clone())
        .chain(
            c.references
                .iter()
                .filter(|(g, _)| *g == snd)
                .map(|(_, n)| n.clone()),
        );
    for n in wanted {
        if !names.contains(&n) && h1.find(snd, &n).is_some() {
            names.push(n);
        }
    }
    let all = names.len();
    let mut take = all.min(max);
    let mut converted = Vec::new();
    for n in names.iter().take(take) {
        let tag = h1.find(snd, n).expect("found above");
        let s = h1sounds::Sound::read(h1, tag).map_err(|e| invalid(format!("{n}: {e}")))?;
        let cs =
            h1sounds::convert_sound(&s, h1.bytes()).map_err(|e| invalid(format!("{n}: {e}")))?;
        converted.push((n.clone(), cs));
    }
    while take > 0 {
        let mut trial = set.clone();
        match h1sounds::install_sounds(&mut trial, converted[..take].to_vec()) {
            Ok(ids) => {
                *set = trial;
                let out = converted[..take]
                    .iter()
                    .map(|(n, _)| n.clone())
                    .zip(ids)
                    .collect();
                return Ok((out, all - take));
            }
            Err(_) => take /= 2,
        }
    }
    Ok((HashMap::new(), all))
}

/// The scenario's simulation table (`+0x3d8`, a datum each): the object,
/// projectile and damage tags the game may name over the network. Retail
/// lists only these groups (**V**: 05a's 446 entries, 03a's 454,
/// Coagulation's 350).
pub const SIMULATION_TABLE: u32 = 0x3d8;
pub const SIMULATION_GROUPS: [&[u8; 4]; 13] = [
    b"bipd", b"bloc", b"crea", b"ctrl", b"eqip", b"garb", b"jpt!", b"mach", b"proj", b"scen",
    b"ssce", b"vehi", b"weap",
];

/// Writes the simulation table of a fresh level from the tags `set` holds,
/// in index order. Returns the entries.
pub fn write_simulation_table(set: &mut TagSet) -> usize {
    let list: Vec<Block> = set
        .slots
        .iter()
        .filter_map(|s| s.tag.as_ref())
        .filter(|t| SIMULATION_GROUPS.iter().any(|g| t.group == FourCC(**g)))
        .map(|t| {
            let mut b = Block::zeroed(4);
            b.put_u32(0, t.id.0);
            b
        })
        .collect();
    let n = list.len();
    let id = set.scenario;
    if let Some(TagPayload::Built(root)) = set.tag_mut(id).map(|t| &mut t.payload) {
        put_block(root, SIMULATION_TABLE, 4, list);
    }
    n
}

/// Bounds the clusters' background sounds' samples to `budget` bytes: the
/// palette's entries in order while they fit; the rest keep their palette
/// entry and their tracks (so the clusters' indices and the tag's shape
/// stand) but no sounds: each track's and detail's sound is a null
/// reference. Returns the silenced ones' names.
pub fn bound_ambience(a: &mut h1convert::ambience::Ambience, budget: u64) -> Vec<String> {
    if budget == u64::MAX {
        return Vec::new();
    }
    let mut counted: Vec<String> = Vec::new();
    let mut bytes = 0u64;
    let mut silenced = Vec::new();
    for (entry, l) in &mut a.backgrounds {
        let new: u64 = l
            .sounds
            .iter()
            .filter(|(n, _)| !counted.contains(n))
            .map(|(_, s)| sample_bytes(s))
            .sum();
        if bytes + new <= budget {
            bytes += new;
            counted.extend(l.sounds.iter().map(|(n, _)| n.clone()));
            continue;
        }
        silenced.push(format!("{entry} ({})", l.name));
        l.sounds.clear();
    }
    silenced
}
