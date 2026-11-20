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
//! | `+0x340` atmosphere | `0xf4` | Halo 1's indoor fog, the first sky's fog B, when it has one ([`indoor_fog`]; `h1sky::atmosphere`); the skyless clusters name it |
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

mod ai_weapons;

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

/// Squad records reserved in the native campaign game-state allocation.
pub const MAX_SQUADS: usize = engine::campaign_squad_pool::LIMIT;
/// Squad groups the engine holds: the array at `0x002009ad` (`mov
/// ebx,64h`, `0x38` per datum), one datum per `+0x158` element. **E**.
pub const MAX_SQUAD_GROUPS: usize = 0x64;

/// Script tables: string data, scripts, globals, references, nodes.
pub const SCRIPT_FIELDS: [u32; 5] = [0x1b0, 0x1b8, 0x1c0, 0x1c8, 0x238];
// Every node the compiler may write has a bit in the runtime's syntax
// bitmap (engine::hs_syntax).
const _: () = assert!(h1scripts::compile::MAX_NODES as u32 <= engine::hs_syntax::CAPACITY);
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
    /// Halo 1 objects the Halo 2-assets variant placed retail Halo 2 tags
    /// for (`crate::h2assets`): (Halo 1 tag name, retail datum).
    pub swapped: Vec<(String, TagId)>,
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
    /// dialogue plays.
    pub dialogue_bytes: u64,
    /// The player biped, converted and set as the campaign players' unit.
    pub player_biped: Option<String>,
    /// HUD pairs `boot::hud` switches between (owner, Halo 1 HUD, Halo 2
    /// HUD; docs/halo1/hud.md), as this set's datums.
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
    /// by the caller's explicit count limit.
    pub sounds: usize,
    pub sounds_dropped: usize,
    pub missing_calls: usize,
    /// Missing builtin calls and external globals, with their authored definition.
    pub missing_builtins: Vec<h1scripts::compile::Unresolved>,
    pub missing_globals: Vec<h1scripts::compile::Unresolved>,
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
        if !self.swapped.is_empty() {
            let names: Vec<&str> = self.swapped.iter().map(|s| s.0.as_str()).collect();
            writeln!(f, "  Halo 2 assets placed for: {}", names.join(", "))?;
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
            for (label, uses) in [("builtin", &s.missing_builtins), ("global", &s.missing_globals)] {
                let sites: std::collections::BTreeSet<_> = uses.iter()
                    .map(|u| (u.definition.as_str(), u.name.as_str()))
                    .collect();
                for (script, name) in sites {
                    writeln!(f, "    {script}: missing {label} {name}")?;
                }
            }
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
                "    script sounds: {} converted, {} left out",
                s.sounds, s.sounds_dropped
            )?;
            writeln!(
                f,
                "    placed at startup ({INITIAL_SCRIPT}): {:?}",
                s.initial
            )?;
            writeln!(
                f,
                "    recordings: {}, {} timed mode changes, {} weapon controls",
                s.recordings, s.recordings_rewritten.animation_events, s.recordings_rewritten.weapon_flags
            )?;
        }
        Ok(())
    }
}

/// Element bytes of a Halo 1 block, or nothing when the field is empty or
/// outside the tag.
pub(crate) fn h1_block(h1: &H1Map, (at, stride): (usize, usize)) -> Result<Vec<&[u8]>> {
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

/// The scenario's atmosphere block (`h1sky::atmosphere`): one element,
/// Halo 1's indoor fog, or none.
pub const ATMOSPHERE: u32 = h1sky::atmosphere::SCENARIO_FIELD;
/// The name of that element.
pub const INDOOR_ATMOSPHERE: &str = "h1_indoor";

/// Halo 1's indoor fog: the fog B of the scenario's first sky, which Halo
/// 1 draws in every cluster naming no sky (E `0x000581a0`;
/// `h1sky::h1::Sky::indoor_fog`). `None` when the level has no sky or its
/// fog B has no density; a sky that does not read is an error naming it.
pub fn indoor_fog(h1: &H1Map) -> Result<Option<h1sky::h1::Fog>> {
    let Some(first) = h1.multiplayer()?.skies.into_iter().next() else {
        return Ok(None);
    };
    let tag = h1.tag(first.id)?;
    let sky = h1sky::h1::Sky::from_map(h1, tag).map_err(|e| {
        h1convert::Error::no_rebuild(&h1.header().name, &tag.name, format!("indoor fog: {e}"))
    })?;
    Ok(sky.indoor_fog())
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

/// Source object groups an untyped script definition may name.
const OBJECT_GROUPS: [&[u8; 4]; 13] = [
    b"bipd", b"vehi", b"weap", b"eqip", b"garb", b"proj", b"scen", b"mach", b"ctrl", b"ssce",
    b"bloc", b"crea", b"lifi",
];

/// The Halo 1 player biped the objects layer converts.
pub const PLAYER_BIPED: &str = "characters\\cyborg\\cyborg";

/// The scenario root fields [`write`] replaces (or empties).
pub fn replaced_fields() -> Vec<u32> {
    use h1ai::h2::layout as ai;
    let mut v = vec![
        SKIES,
        ATMOSPHERE,
        OBJECT_NAMES,
        STARTING_LOCATIONS,
        crate::profiles::H2.0,
        TRIGGER_VOLUMES,
        crate::cutscenes::H2_RECORDINGS,
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
/// take (docs/halo1/campaign.md, "Fresh levels"): its light volumes and
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
/// `lib.rs`).
/// `bsps[k]` is Halo 1 BSP `k`, converted.
#[allow(clippy::too_many_arguments)]
pub fn write(
    h1: &H1Map,
    set: &mut TagSet,
    base: &BaseScenario,
    bsps: &[BspSlot],
    pathfinding: &[tags::pathfinding::PathfindingData],
    structures: &[crate::structure::Prepared],
    sounds: &h1convert::ambience::Installed,
    objects: Option<&h1objects::base::Base>,
    opts: &Options,
    lean: crate::Lean,
    level_text: &h1text::scenario::LevelText,
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
    level_text.write(&mut root);

    // The loader fills these descriptors from the converted BSP sections.
    let mut desc = Vec::new();
    for slot in bsps {
        let mut b = Block::zeroed(BSP);
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
        let id = h1convert::sky::Sky::from_map(h1, tag)?.install(set, &base.sky_templates)?;
        let mut e = Block::zeroed(8);
        e.tag_ref(0, id);
        skies.push(e);
        rep.skies.push(tag.name.clone());
    }
    put_block(&mut root, SKIES, 8, skies);

    // Halo 1's indoor fog as atmosphere element 0, which the skyless
    // clusters name (`structure::Prepared::atmosphere`).
    let atmosphere = indoor_fog(h1)?
        .map(|f| {
            let mut e = Block::new(h1sky::atmosphere::element(&f));
            e.string_id(h1sky::atmosphere::layout::NAME as u32, INDOOR_ATMOSPHERE);
            e
        })
        .into_iter()
        .collect();
    put_block(&mut root, ATMOSPHERE, h1sky::atmosphere::LEN, atmosphere);

    let starts: Vec<Block> = mp
        .starting_locations
        .iter()
        .map(|s| {
            let mut b = Block::zeroed(STARTING_LOCATION);
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
    let mut aliases = crate::h2assets::Aliases::default();
    let ai_src = h1ai::h1::H1Ai::read(h1).map_err(|e| invalid(format!("ai: {e}")))?;
    let (ai_weapon_tags, ai_weapon_loudness) = if opts.ai && opts.assets == crate::h2assets::Assets::Halo2 {
        ai_weapons::retail_tags(h1, &ai_src, set)?
    } else {
        (Vec::new(), Vec::new())
    };
    let mut weapon_palette = Vec::new();
    let mut kit = crate::roster::Loadout::default();
    let source_tables = (opts.ai || opts.scripts.is_some())
        .then(|| h1scripts::Tables::read(h1))
        .transpose()
        .map_err(|e| invalid(format!("AI script references: {e}")))?;
    let script_tags = source_tables.as_ref().filter(|_| opts.scripts.is_some());
    let placed = match objects {
        Some(b) if opts.objects => {
            // Device groups are written with the scripts' level data;
            // without them a placement's group index would dangle.
            let groups = opts.scripts.is_some() && !lean.leaves_out(crate::Lean::LevelData);
            h1map::phase::time("objects", || {
                place_objects(
                    h1,
                    set,
                    b,
                    &mut root,
                    rep,
                    &mut report.dropped,
                    groups,
                    opts.devices,
                    opts.ai.then_some(&ai_src),
                    script_tags,
                    &mut kit,
                    opts.assets,
                    &mut aliases,
                    &mut weapon_palette,
                )
            })?
        }
        Some(b) if opts.ai => {
            let mut conv = h1objects::convert::Converter::new(h1, b);
            if !fresh {
                weapon_palette = ai_weapons::base_palette(base)?;
            }
            // No placements: the Halo 2-assets variant still arms its
            // rebuilt crew with retail weapons.
            let retail = (opts.assets == crate::h2assets::Assets::Halo2).then_some(&*set);
            let roster =
                crate::roster::prepare(h1, &ai_src, &mut conv, opts.assets, retail, script_tags)?;
            let ids = conv
                .tree
                .install(set)
                .map_err(|e| invalid(format!("AI objects: {e}")))?;
            kit = crate::roster::install(set, &ids, roster)?;
            HashMap::new()
        }
        None => {
            if !fresh && opts.ai {
                // The pinned base keeps its own palette when objects are disabled.
                weapon_palette = ai_weapons::base_palette(base)?;
            }
            HashMap::new()
        }
        _ => HashMap::new(),
    };
    level_text.write_control_prompts(
        set,
        placed
            .iter()
            .filter_map(|(&(kind, from), &(_, to))| (kind == 8).then_some((from, to))),
    );
    let h1_names = h1_block(h1, h1::OBJECT_NAMES)?;
    let names: Vec<Block> = h1_names
        .iter()
        .map(|e| {
            let mut b = Block::zeroed(OBJECT_NAME);
            b.bytes[..32].copy_from_slice(&e[..32]);
            b.bytes[31] = 0;
            let ty = i16_at(e, 0x20);
            let at = i16_at(e, 0x22);
            // A converted prop or device can change its object kind.
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
    let mut retained = ai_src.clone();
    let mut ai_options = h1ai::convert::Options {
        halo2_assets: opts.assets == crate::h2assets::Assets::Halo2,
        ..Default::default()
    };
    let mut requests = Vec::new();
    let mut selection = None;
    let mut generated_starts = std::collections::HashSet::new();
    if let Some(tables) = source_tables.as_ref().filter(|_| opts.ai) {
        if let Some(scripts) = opts.scripts {
            ai_options.first_command_script = Some(u16::try_from(tables.scripts.len()).map_err(|_| invalid("too many scripts"))?);
            requests = h1scripts::aicontrol::requests(tables, &scripts.catalog);
        }
        let subjects = opts.scripts.map(|scripts| {
            h1scripts::aicontrol::retained_subjects(tables, &scripts.catalog)
        });
        generated_starts = crate::squad_roster::prepare(
            &mut retained, tables, &requests, &mut ai_options,
            subjects.as_ref().and_then(|s| s.follow_encounters.as_ref()),
            opts.scripts.is_some(),
        );
        if opts.scripts.is_none() {
            let live = |ai: &h1ai::h1::H1Ai| {
                ai.encounters.iter().flat_map(|e| &e.squads)
                    .filter(|q| !q.starting_locations.is_empty())
                    .count()
            };
            let left = live(&ai_src).saturating_sub(live(&retained));
            if left > 0 {
                report.dropped.push(format!(
                    "ai: {left} squads only the scripts create, left out (converted without scripts)"
                ));
            }
        }
        if let Some(subjects) = subjects {
            let groups = retained.encounters.len() + ai_options.platoon_groups.len();
            let selected = crate::ai_control_roster::prepare(
                &ai_src, &mut retained, &generated_starts, &subjects, &requests, groups,
            )?;
            generated_starts = selected.generated_starts.iter().copied().collect();
            selection = Some((selected, subjects));
        }
    }
    let weapons = ai_weapons::install(&mut root, weapon_palette, &ai_weapon_tags)?;
    let mut plan = h1map::phase::time("ai", || h1ai::convert::convert_for_map(&retained, &ai_options, pathfinding, &weapons));
    let mut ai_control = if let Some((retained, _)) = &selection {
        h1ai::control::prepare_with_indices(
            &ai_src, &mut plan, &ai_options, &requests, retained.squad_indices.clone(),
        )
    } else {
        h1ai::control::prepare(&ai_src, &mut plan, &ai_options, &requests)
    };
    for (weapon, loudness) in &ai_weapon_loudness {
        ai_control
            .weapon_reports
            .entry(weapon.clone())
            .or_insert(*loudness);
    }
    if let Some((retained, subjects)) = &selection {
        h1ai::control::install_teleport_starts(
            &ai_src, &mut ai_control, subjects.teleport_subjects.as_ref(),
        );
        h1ai::control::install_automatic_destinations(
            &ai_src, &mut plan, &ai_options, &mut ai_control,
            &retained.overflow_hosts, subjects.follow_encounters.as_ref(),
        ).map_err(|error| invalid(format!("AI migration orders: {error}")))?;
    }
    if opts.scripts.is_some_and(|scripts| {
        scripts.target.builtins.get("ai_magically_see_players")
            .is_some_and(|builtin| builtin.index == hs::h1_ai_slots::CONTROL)
    }) {
        for &squad in ai_control.combat_timers.keys() {
            plan.ai.squads[usize::from(squad)].delay = 0.0;
        }
    }
    if opts.ai {
        kit.equip(&mut root, &mut plan)?;
        for (i, bsp) in structures.iter().enumerate() {
            let floors = h1ai::floors::Floors::new(&bsp.pathfinding);
            for &index in &generated_starts {
                let squad = &mut plan.ai.squads[index];
                let zone = &plan.ai.zones[squad.initial_zone as usize];
                let encounter = &ai_src.encounters[squad.initial_zone as usize];
                if zone.manual_bsp as usize == i && encounter.flags & 0x20 == 0 {
                    for start in &mut squad.starting_locations {
                        start.position = floors.ground_point(start.position).ok_or_else(|| {
                            invalid(format!("no ground sector for {}", squad.name))
                        })?;
                    }
                }
            }
            for (zone, enc) in plan.ai.zones.iter_mut().zip(&ai_src.encounters) {
                if zone.manual_bsp as usize == i && enc.flags & 0x20 == 0 {
                    floors
                        .bind(zone, |p| {
                            let leaf = h1convert::query::leaf_at(&bsp.collision, p)?;
                            i16::try_from(*bsp.sound_leaves.get(leaf)?).ok()
                        })
                        .map_err(|e| invalid(format!("ai: {e}")))?;
                }
            }
        }
    }
    if opts.ai
        && !engine::campaign_squad_pool::holds(plan.ai.squads.len(), plan.ai.squad_groups.len())
    {
        return Err(invalid(format!(
            "ai: {} squads and {} squad groups, the native campaign squad and group pools cannot hold them",
            plan.ai.squads.len(),
            plan.ai.squad_groups.len()
        )));
    }
    if opts.ai && opts.scripts.is_some()
        && source_tables.as_ref().is_some_and(|t| !t.commands.timing.is_empty())
    {
        let squads = retained.encounters.iter().flat_map(|e| &e.squads);
        for (src, dst) in squads.zip(&mut plan.ai.squads) {
            let locations = src.starting_locations.iter().zip(&mut dst.starting_locations);
            for (i, (source, target)) in locations.enumerate() {
                let variant = if source.actor >= 0 { source.actor } else { src.actor };
                if variant >= 0 {
                    target.name = set.intern(&h1scripts::commands::placement_name(variant, i))?;
                }
            }
        }
    }
    let (characters, chars) = crate::ai_control_characters::resolve(
        h1, set, &plan, &mut ai_control, opts.ai, opts.ai && opts.scripts.is_some(),
    )?;
    let mut characters = characters.into_iter();
    let blocks = plan
        .blocks_with_characters(|_| characters.next())
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
        for (at, count, data) in blocks.fields {
            put_block(&mut root, at, 0, Vec::new());
            if opts.scripts.is_some() && at as usize == h1ai::h2::layout::SCRIPTING_DATA.0 && count > 0 {
                root.block(at, count, data);
            }
        }
    }

    // Recordings also drive gameplay outside cinematics, on every lean step.
    let command_recording = opts.scripts.is_some() && ai_src.command_lists.iter()
        .any(|list| list.atoms.iter().any(|atom| atom.kind == 14));
    let mut recordings = if opts.cutscene_data || command_recording {
        crate::cutscenes::read_recordings(h1)?
    } else {
        Vec::new()
    };
    let rewritten = crate::recordings::for_halo2(&mut recordings)?;
    put_block(
        &mut root,
        crate::cutscenes::H2_RECORDINGS,
        crate::cutscenes::H2_RECORDING_LEN,
        crate::cutscenes::recordings_block(&recordings)?,
    );

    // Scripts.
    match opts.scripts {
        Some(s) => {
            let names: Vec<String> = h1_names.iter().map(|e| name32(e)).collect();
            let volume_names: Vec<&str> = h1_volumes.iter().map(|v| v.name.as_str()).collect();
            let (squads, groups): (&[String], &[String]) = if opts.ai {
                (&plan.squad_names, &plan.group_names)
            } else {
                (&[], &[])
            };
            // Encounters Halo 1 creates at load (encounter flag bit 0, "not
            // initially created", clear): their squad groups, placed by a
            // startup script.
            let initial: Vec<String> = if opts.ai {
                ai_src
                    .encounters
                    .iter()
                    .filter(|e| e.flags & 1 == 0)
                    .map(|e| e.name.clone())
                    .collect()
            } else {
                Vec::new()
            };
            let _scripts = h1map::phase::start("scripts");
            rep.scripts = Some(install_scripts(
                h1,
                source_tables.as_ref().expect("read for the scripts layer"),
                s,
                set,
                &mut root,
                &Targets {
                    names: &names,
                    placed: &names_placed,
                    volumes: &volume_names,
                    parked,
                    squads,
                    groups,
                },
                &initial,
                &ai_control,
                &ai_src.conversations,
                &if opts.ai {
                    h1ai::conversations::SpeakerScopes::from_plan(&plan)
                } else {
                    Default::default()
                },
                opts.sounds,
                !lean.leaves_out(crate::Lean::LevelData),
                !lean.leaves_out(crate::Lean::SeatTable),
                opts.cutscene_data
                    && opts.cutscene_graphs
                    && !lean.leaves_out(crate::Lean::CutsceneData),
                &recordings,
                rewritten,
                crate::cutscenes::policy(opts, &h1.header().name.to_ascii_lowercase()),
                &aliases,
                opts.assets,
                level_text,
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
    ai: Option<&h1ai::h1::H1Ai>,
    scripts: Option<&h1scripts::Tables>,
    kit: &mut crate::roster::Loadout,
    assets: crate::h2assets::Assets,
    aliases: &mut crate::h2assets::Aliases,
    weapon_palette: &mut Vec<TagId>,
) -> Result<HashMap<(i16, i16), (i16, i16)>> {
    let halo2 = assets == crate::h2assets::Assets::Halo2;
    // The Halo 2-assets variant's retail tag for a Halo 1 object
    // (`crate::h2assets`): `Ok(Some)` places it, `Ok(None)` converts
    // Halo 1's own, `Err` leaves it out.
    let swap = |set: &TagSet, tag: &h1map::Tag| -> std::result::Result<Option<TagId>, String> {
        if !halo2 {
            return Ok(None);
        }
        let got = crate::h2assets::stand_in(set, tag)?;
        Ok(got)
    };
    let mut conv = h1objects::convert::Converter::new(h1, base);
    conv.options.unit_hud_draw_colour = true;
    let roster = ai
        .map(|src| {
            crate::roster::prepare(h1, src, &mut conv, assets, halo2.then_some(&*set), scripts)
        })
        .transpose()?
        .unwrap_or_default();
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
                // Weapons, equipment and vehicles swap; placed bipeds are
                // the cutscenes' casts and stay Halo 1's.
                let swappable = matches!(k.h1_type, 1..=3);
                let swapped = match if swappable { swap(&*set, tag) } else { Ok(None) } {
                    Ok(Some(id)) => {
                        aliases.insert(FourCC(tag.group.0.to_be_bytes()), &tag.name, id);
                        rep.swapped.push((tag.name.clone(), id));
                        Some(Some(Pal::Retail(id)))
                    }
                    Ok(None) => None,
                    Err(why) => {
                        return Err(
                            h1convert::Error::no_rebuild(&h1.header().name, &tag.name, why).into(),
                        );
                    }
                };
                if let Some(got) = swapped {
                    pal.push(got);
                    continue;
                }
                Some(Pal::Converted(conv.object(tag).map_err(|e| {
                    h1convert::Error::no_rebuild(&h1.header().name, &tag.name, e)
                })?))
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
        if let Some(id) = swap(set, tag)
            .map_err(|why| h1convert::Error::no_rebuild(&h1.header().name, &tag.name, why))?
        {
            aliases.insert(FourCC(*b"weap"), &tag.name, id);
            profile_weapons.insert(w, Pal::Retail(id));
            continue;
        }
        let id = conv
            .object(tag)
            .map_err(|e| h1convert::Error::no_rebuild(&h1.header().name, &tag.name, e))?;
        profile_weapons.insert(w, Pal::Converted(id));
    }
    // The players' first-person arms: Halo 1's, which Halo 1's
    // first-person graphs name node for node (the engine binds them by
    // name).
    let hands = match h1convert::globals::h1_first_person_hands(h1)
        .map_err(|e| invalid(format!("first-person arms: {e}")))?
    {
        Some(id) => Some(conv.first_person_arms(id).map_err(|e| {
            h1convert::Error::no_rebuild(
                &h1.header().name,
                h1.tag(id).map_or("first-person arms", |t| &t.name),
                e,
            )
        })?),
        None => None,
    };
    let mut player = None;
    let mut retail_player = None;
    let cyborg = h1.find(h1map::Group::new(b"bipd"), PLAYER_BIPED);
    // A swap that leaves the cyborg out is a named error, not Halo 1's.
    let cyborg_swap = match cyborg {
        Some(t) => swap(&*set, t)
            .map_err(|why| h1convert::Error::no_rebuild(&h1.header().name, &t.name, why))?,
        None => None,
    };
    rep.player_biped = match (cyborg, cyborg_swap) {
        (Some(t), Some(id)) => {
            aliases.insert(FourCC(*b"bipd"), &t.name, id);
            retail_player = Some(id);
            set.slots[id.index()].name.clone()
        }
        (Some(t), None) => match conv.object(t) {
            Ok(id) => {
                player = Some(id);
                Some(t.name.clone())
            }
            Err(e) => {
                return Err(h1convert::Error::no_rebuild(&h1.header().name, &t.name, e).into());
            }
        },
        (None, _) => None,
    };
    // Halo 1's grenade types (the globals), through the same converter.
    let mut grenades = Vec::new();
    // With Halo 2's assets the base's grenade types stay.
    let h1_grenades = if halo2 {
        Vec::new()
    } else {
        h1convert::globals::h1_grenades(h1).map_err(|e| invalid(format!("grenades: {e}")))?
    };
    for g in h1_grenades {
        let mut one = |id: Option<h1map::TagId>| -> Result<Option<TagId>> {
            let Some(id) = id else { return Ok(None) };
            let tag = h1.tag(id)?;
            let id = conv
                .object(tag)
                .map_err(|e| h1convert::Error::no_rebuild(&h1.header().name, &tag.name, e))?;
            Ok(Some(id))
        };
        grenades.push(h1convert::globals::Grenade {
            maximum: g.maximum,
            equipment: one(g.equipment)?,
            projectile: one(g.projectile)?,
        });
    }
    if let Some(tables) = scripts {
        for r in &tables.references {
            if r.id.is_none() {
                continue;
            }
            let t = h1.tag(r.id)?;
            let group = FourCC(t.group.0.to_be_bytes());
            if aliases.get(group, &t.name).is_some() {
                continue;
            }
            match &group.0 {
                b"effe" => {
                    conv.effect(t)
                        .map_err(|e| h1convert::Error::no_rebuild(&h1.header().name, &t.name, e))?;
                }
                b"jpt!" => {
                    conv.damage(t.id)
                        .map_err(|e| h1convert::Error::no_rebuild(&h1.header().name, &t.name, e))?;
                }
                b"bipd" | b"vehi" | b"mach" | b"weap" | b"scen" => {
                    if let Some(id) = swap(set, t).map_err(|why| {
                        h1convert::Error::no_rebuild(&h1.header().name, &t.name, why)
                    })? {
                        aliases.insert(group, &t.name, id);
                    } else {
                        conv.object(t).map_err(|e| {
                            h1convert::Error::no_rebuild(&h1.header().name, &t.name, e)
                        })?;
                    }
                }
                _ => {} // Sounds, actor variants and animation graphs have dedicated installers.
            }
        }
    }
    rep.dialogue_bytes = conv.dialogue_bytes;
    let mut tree = std::mem::take(&mut conv.tree);
    if let Some(reason) = &tree.model_shader_error {
        return Err(invalid(reason.clone()));
    }
    rep.object_tags = tree.tags.len();
    let hud_swaps = std::mem::take(&mut tree.hud_swaps);
    // The sounds the objects' effects and attachments play, with the
    // gestalt, before the tags that name them.
    rep.object_sounds = tree.sounds.len();
    rep.object_loops = tree.loops.len();
    let raw_before: u64 = set.raw.iter().map(|r| r.bytes.len() as u64).sum();
    tree.try_install_audio(set).map_err(|e| invalid(format!("object audio: {e}")))?;
    rep.object_sound_bytes = set.raw.iter().map(|r| r.bytes.len() as u64).sum::<u64>() - raw_before;
    dropped.extend(tree.audio_notes.drain(..).map(|n| format!("object sounds: {n}")));
    rep.hud_sensor = tree.hud_sensor;
    let ids = tree
        .install(set)
        .map_err(|e| invalid(format!("objects: {e}")))?;
    *kit = crate::roster::install(set, &ids, roster)?;
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
    if let Some(p) = player.map(real).or(retail_player) {
        h1convert::globals::set_player_unit(set, p, h1convert::globals::Players::Campaign)
            .map_err(|e| invalid(format!("player unit: {e}")))?;
    }
    if let Some(h) = hands {
        h1convert::globals::set_player_hands(set, real(h), h1convert::globals::Players::Campaign)
            .map_err(|e| invalid(format!("first-person arms: {e}")))?;
    }

    let grenades: Vec<_> = grenades
        .iter()
        .map(|g| h1convert::globals::Grenade {
            maximum: g.maximum,
            equipment: g.equipment.map(real),
            projectile: g.projectile.map(real),
        })
        .collect();
    if !halo2 {
        rep.grenades = h1convert::globals::set_grenades(set, &grenades)
            .map_err(|e| invalid(format!("grenades: {e}")))?;
    }

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
    // Keep each source palette's indices and add entries to the output
    // palette of any prop whose native Halo 2 object kind differs.
    let mut blocks: Vec<PlacementBlock> = Vec::new();
    for (k, pal) in KINDS.iter().zip(&palettes) {
        let first = placement_block(&mut blocks, k.h2_type).palette.len();
        let mut targets = Vec::with_capacity(pal.len());
        for (i, p) in pal.iter().enumerate() {
            let id = p.as_ref().map(palette_id);
            let group = id.and_then(|id| set.tag(id)).map(|tag| tag.group.0);
            let ty = placement_type(k.h2_type, group);
            let source = placement_block(&mut blocks, k.h2_type);
            source
                .palette
                .push(palette_entry(if ty == k.h2_type { id } else { None }));
            if ty == k.h2_type {
                targets.push(id.map(|_| (ty, first + i)));
            } else {
                let dest = placement_block(&mut blocks, ty);
                targets.push(Some((ty, dest.palette.len())));
                dest.palette.push(palette_entry(id));
            }
        }
        if k.h2_type == 2 {
            weapon_palette.extend(
                pal.iter()
                    .map(|p| p.as_ref().map_or(TagId::NULL, palette_id)),
            );
        }
        let converted = targets.iter().flatten().count();
        let src = h1_block(h1, k.h1)?;
        let mut n = 0;
        for (i, e) in src.iter().enumerate() {
            let p = i16_at(e, 0);
            let Some((ty, palette)) = targets.get(p as usize).copied().flatten() else {
                continue;
            };
            let def = tags::object::OBJECT_TYPES[usize::from(ty)];
            let (_, _, stride) = def.placement.expect("placeable");
            let mut b = Block::zeroed(usize::from(stride));
            b.put_u16(0, palette as u16);
            b.put_u16(2, i16_at(e, 2) as u16);
            // Halo 1's "not placed automatically" is Halo 2's "not at start".
            b.put_u32(4, u32::from(i16_at(e, 4) as u16 & h1::NOT_AUTOMATIC));
            for (j, v) in f32s::<6>(e, 8).iter().enumerate() {
                b.put_f32(8 + 4 * j, *v);
            }
            b.put_u32(0x28, UNIQUE_ID_HIGH << 16 | unique);
            unique += 1;
            b.put_u16(0x2c, 0xffff);
            b.bytes[0x2e] = ty as u8;
            b.bytes[0x2f] = 1;
            b.put_u16(0x32, 0xffff);
            if k.device != Device::No {
                write_device(&mut b, e, k.device, groups);
            }
            let out = &mut placement_block(&mut blocks, ty).placements;
            placed.insert((k.h1_type, i as i16), (ty as i16, out.len() as i16));
            out.push(b);
            n += 1;
        }
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
    for PlacementBlock {
        object_type,
        palette,
        placements,
    } in blocks
    {
        let ty = tags::object::OBJECT_TYPES[usize::from(object_type)];
        let (at, pal_at, stride) = ty.placement.expect("placeable");
        put_block(root, u32::from(pal_at), PALETTE, palette);
        put_block(root, u32::from(at), usize::from(stride), placements);
    }
    Ok(placed)
}

struct PlacementBlock {
    object_type: u16,
    palette: Vec<Block>,
    placements: Vec<Block>,
}

fn placement_block(blocks: &mut Vec<PlacementBlock>, ty: u16) -> &mut PlacementBlock {
    let slot = match blocks.iter().position(|b| b.object_type == ty) {
        Some(i) => i,
        None => {
            blocks.push(PlacementBlock {
                object_type: ty,
                palette: Vec::new(),
                placements: Vec::new(),
            });
            blocks.len() - 1
        }
    };
    &mut blocks[slot]
}

fn palette_entry(id: Option<TagId>) -> Block {
    let mut entry = Block::zeroed(PALETTE);
    entry.tag_ref(0, id.unwrap_or(TagId::NULL));
    entry
}

fn placement_type(source_type: u16, group: Option<[u8; 4]>) -> u16 {
    // Only the scripted vehicle props can change groups here. Keep
    // devices' existing mapping, which also controls their placement fields.
    if source_type == 1 {
        match group.as_ref() {
            Some(b"bloc") => return 11,
            Some(b"scen") => return 6,
            _ => {}
        }
    }
    source_type
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
            let token = &h.token;
            if let Some(i) = t.squads.iter().position(|q| q.eq_ignore_ascii_case(token)) {
                return Some(i as u32);
            }
            let group = |g: &str| {
                t.groups
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(g))
                    .map(|i| 0x4000_0000 | i as u32)
            };
            group(token)
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn install_scripts(
    h1: &H1Map,
    t: &h1scripts::Tables,
    s: &crate::Scripting,
    set: &mut TagSet,
    root: &mut Block,
    targets: &Targets,
    initial: &[String],
    ai_control: &h1ai::control::Control,
    conversations: &[h1ai::conversations::Conversation],
    speaker_scopes: &h1ai::conversations::SpeakerScopes,
    max_sounds: usize,
    level_data: bool,
    seat_table: bool,
    cutscene_graphs: bool,
    recordings: &[crate::cutscenes::Recording],
    rewritten: crate::recordings::Report,
    cutscenes: h1scripts::compile::Cutscenes,
    aliases: &crate::h2assets::Aliases,
    assets: crate::h2assets::Assets,
    level_text: &h1text::scenario::LevelText,
) -> Result<ScriptReport> {
    use h1scripts::compile::{Missing, compile_scenario};
    let refs: Vec<String> = t
        .references
        .iter()
        .enumerate()
        .map(|(i, r)| {
            h1.tag(r.id).map(|t| t.name.clone()).map_err(|e| {
                h1convert::Error::no_rebuild(&h1.header().name, format!("script reference {i}"), e).into()
            })
        })
        .collect::<Result<_>>()?;
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
    let mut c = compile_scenario(
        t,
        &s.catalog,
        &s.table,
        &s.target,
        Missing::Stub,
        cutscenes,
        &resolves,
        &refs,
        Some(ai_control),
        Some((conversations, speaker_scopes)),
    )
    .map_err(|e| invalid(format!("scripts: {e}")))?;
    h1scripts::compile::add_startup_placements(&mut c, &s.target, INITIAL_SCRIPT, initial)
        .map_err(|e| invalid(format!("scripts: {e}")))?;
    let (threads, _) = engine::hs_pool::capacities(c.globals.len() as u32)
        .ok_or_else(|| invalid("scenario globals exceed HaloScript storage"))?;
    let persistent = c.scripts.iter().filter(|s| u16::from_le_bytes([s[32], s[33]]) <= 2).count();
    if persistent >= threads as usize {
        return Err(invalid(format!("{persistent} persistent scripts leave no free thread in {threads}")));
    }
    if let Some(op) = s.catalog.index_of("cinematic_screen_effect_set_video")
        && t.nodes.iter().any(|n| n.live() && !n.atom() && !n.script_call() && n.opcode() == op)
    {
        crate::cutscenes::install_video(h1, set, &mut c)?;
    }
    let (sounds, sounds_dropped) = install_named_sounds(h1, set, &c, max_sounds)?;
    // Animation graphs the cutscenes name, and their animation names
    // (crate::cutscenes).
    let graphs = if cutscene_graphs {
        crate::cutscenes::install_graphs(h1, set, &c)?
    } else {
        crate::cutscenes::install_graph_hooks(h1, set, c.hooks.iter().filter(|h| {
            h.h1_type == h1scripts::commands::ANIMATION_GRAPH
        }))?
    };
    let mut interned = HashMap::new();
    for h in c.hooks.iter().filter(|h| h.type_name == "string_id") {
        let text = match h.h1_type.as_str() {
            h1scripts::compile::ANIMATION_NAME => Some(crate::cutscenes::animation_name(&graphs, h)),
            h1scripts::compile::SEAT_NAME => Some(h1models::names::mode_name(&h.token)),
            // An unknown pose requests the default mode. Halo 1 performs
            // the same reset when its pose lookup returns -1.
            h1scripts::compile::SEAT_POSE => s
                .catalog
                .has_unit_pose(&h.token)
                .then(|| h1models::names::unit_mode(&h.token)),
            _ => Some(h.token.clone()),
        };
        // The same text can be a marker, a vehicle seat or an animation
        // mode. Resolve by atom slot so these uses keep their own values.
        let id = match text {
            Some(text) => set.intern(&text)?,
            None => 0,
        };
        interned.insert(h.slot, id);
    }
    let profiles = if c.hooks.iter().any(|h| h.type_name == "starting_profile") {
        crate::profiles::read(h1)?
    } else {
        Vec::new()
    };
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
    let blocks = level.blocks(level_text.chapter_text_ids())
        .map_err(|e| invalid(format!("script level data: {e}")))?;
    for (at, stride, elements) in blocks {
        put_block(root, at, stride, elements);
    }
    let (at, stride) = h1scripts::seats::MAPPINGS;
    put_block(root, at, stride, seats.elements());
    let free_characters = crate::actor_control::install(h1, set, aliases, assets, &c.hooks)?;
    let tags_by_name: HashMap<(FourCC, String), TagId> = set
        .slots
        .iter()
        .filter_map(|s| {
            let t = s.tag.as_ref()?;
            let name = s.name.as_ref()?;
            (set.converted(t.group, name) == Some(t.id))
                .then(|| ((t.group, name.to_ascii_lowercase()), t.id))
        })
        .collect();
    let mut resolved_by_level = 0;
    let left = c.resolve_hooks(|h| {
        match h.type_name.as_str() {
            "long" if h.h1_type == "actor_variant" => free_characters.get(&h.token).map(|id| id.0),
            "string_id" => interned.get(&h.slot).copied(),
            "hud_message" => level_text.message_index(&h.token),
            "hud_corner" => h1scripts::hud::corner(&h.token),
            "starting_profile" => crate::profiles::script_index(&profiles, &h.token),
            "sound" => sounds.get(&h.token.to_ascii_lowercase()).map(|id| id.0),
            _ if h.h1_type == "command_voice_tag" => tags_by_name
                .get(&(FourCC(*b"udlg"), h.token.to_ascii_lowercase()))
                .map(|id| id.0),
            _ if hook_value(h, targets).is_some() => hook_value(h, targets),
            "unit_seat_mapping" => seats.values.get(&h.token).copied(),
            "cutscene_recording" => crate::cutscenes::recording_index(recordings, &h.token),
            // An object definition: the converted tag of that name, or the
            // Halo 2 tag placed instead of it (`crate::h2assets`).
            "object_definition" => {
                let name = h.token.to_ascii_lowercase();
                let mut sources = h1.tags().iter().filter(|tag| {
                    tag.name.eq_ignore_ascii_case(&name)
                        && OBJECT_GROUPS.contains(&&tag.group.0.to_be_bytes())
                });
                let source = sources.next()?;
                if sources.next().is_some() {
                    return None;
                }
                let group = FourCC(source.group.0.to_be_bytes());
                let dest = match &group.0 {
                    b"lifi" => FourCC(*b"mach"),
                    b"vehi" => {
                        FourCC(h1objects::scripted_props::replacement_group(&name).unwrap_or(group.0))
                    }
                    _ => group,
                };
                aliases
                    .get(group, &name)
                    .or_else(|| set.converted(dest, &name))
                    .map(|id| id.0)
            }
            ty => level.resolve(h).or_else(|| {
                let g = h1scripts::level::tag_group(ty)?;
                tags_by_name
                    .get(&(g, h.token.to_ascii_lowercase()))
                    .map(|id| id.0)
                    .or_else(|| aliases.get(g, &h.token).map(|id| id.0))
            }),
        }
        .inspect(|_| {
            resolved_by_level += usize::from(
                !matches!(h.type_name.as_str(), "string_id" | "sound")
                    && hook_value(h, targets).is_none(),
            );
        })
    });
    if let Some(h) = left.iter().find(|h| h.h1_type == h1scripts::commands::ANIMATION_GRAPH) {
        return Err(invalid(format!("AI command {} {} could not be installed", h.type_name, h.token)));
    }
    if let Some(hook) = left.iter().find(|h| h.type_name == "hud_message") {
        return Err(invalid(format!("HUD message {:?} is missing", hook.token)));
    }
    if let Some(h) = left.iter().find(|h| {
        !h.token.eq_ignore_ascii_case("none")
            && (h.type_name == "object_definition"
                || h.h1_type == "command_voice_tag"
                || h1scripts::level::tag_group(&h.type_name).is_some())
            && h.h1_type != "command_unit_graph_candidate"
    }) {
        return Err(h1convert::Error::no_rebuild(
            &h1.header().name,
            &h.token,
            format!("script atom {}", h.type_name),
        )
        .into());
    }
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
    let mut resolved = 0;
    let mut missing_character = None;
    let mut missing_weapon = None;
    for at in SCRIPT_FIELDS {
        put_block(root, at, 0, Vec::new());
    }
    c.install(root, |group, name| {
        let source = FourCC(group.0.to_be_bytes());
        // By group and name: a converted weapon, its model and its
        // animation graph share one Halo 1 path, so a name alone picked
        // whichever slot came last (a10's perception rows named a `jmad`
        // for a weapon).
        let dest = match &source.0 {
            b"actv" => FourCC(*b"char"),
            b"lifi" => FourCC(*b"mach"),
            b"vehi" => {
                FourCC(h1objects::scripted_props::replacement_group(name).unwrap_or(source.0))
            }
            _ => reference_group(group),
        };
        let id = if source == FourCC(*b"char") {
            // Compiler-generated references identify a concrete character selected by the AI plan.
            let id = ai_control
                .uncover
                .iter()
                .find(|(_, rule)| rule.character == name)
                .map(|(&id, _)| TagId(id));
            if id.is_none() {
                missing_character = Some(name.to_owned());
            }
            id
        } else {
            let id = set.converted(dest, name)
                .or_else(|| aliases.get(source, name));
            // A weapon row of the perception table: the runtime finds the
            // rows by their place at the end of the references, so a
            // dropped one would shift every row after it.
            if id.is_none()
                && dest == FourCC(*b"weap")
                && ai_control.weapon_reports.contains_key(name)
            {
                missing_weapon = Some(name.to_owned());
            }
            id
        };
        let r = id.and_then(|id| set.tag(id).map(|t| (t.group, id)));
        resolved += usize::from(r.is_some());
        r
    })
    .map_err(|e| invalid(format!("{}: {e}", h1.header().name)))?;
    if let Some(name) = missing_character {
        return Err(h1convert::Error::no_rebuild(
            &h1.header().name,
            &name,
            "script AI character reference",
        )
        .into());
    }
    if let Some(name) = missing_weapon {
        return Err(h1convert::Error::no_rebuild(
            &h1.header().name,
            &name,
            "script AI weapon reference",
        )
        .into());
    }
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
        missing_calls: c.missing.len(),
        missing_builtins: c.missing,
        missing_globals: c.missing_globals,
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

/// The Halo 2 group of the tag converted from a Halo 1 tag of `group`, as
/// a script reference names it: Halo 1's animation graphs, models and
/// shaders are Halo 2's `jmad`, `mode` and `shad`; every other group keeps
/// its four characters.
fn reference_group(group: h1map::Group) -> FourCC {
    let g = group.0.to_be_bytes();
    FourCC(match &g {
        b"antr" => *b"jmad",
        b"mod2" | b"mode" => *b"mode",
        b"shdr" | b"senv" | b"soso" | b"sotr" | b"schi" | b"scex" | b"swat" | b"sgla"
        | b"smet" | b"spla" => *b"shad",
        _ => g,
    })
}

/// Converts the `snd!` tags the scripts name (sound hooks and script
/// references), at most `max` of them, and installs them under their
/// Halo 1 names with the level's other sounds.
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
        if let Some(tag) = h1.find(snd, &n)
            && !names.contains(&tag.name)
        {
            names.push(tag.name.clone());
        }
    }
    let all = names.len();
    let take = all.min(max);
    let mut ids = HashMap::new();
    let mut needed = Vec::new();
    for name in names.into_iter().take(take) {
        if let Some(id) = set.converted(FourCC(*b"snd!"), &name)
        {
            ids.insert(name.to_ascii_lowercase(), id);
        } else {
            needed.push(name);
        }
    }
    let installed = h1sounds::convert::install_source_sounds(h1, set, needed)
        .map_err(|e| invalid(format!("script sounds: {e}")))?;
    ids.extend(
        installed
            .into_iter()
            .map(|(n, id)| (n.to_ascii_lowercase(), id)),
    );
    Ok((ids, all - take))
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
