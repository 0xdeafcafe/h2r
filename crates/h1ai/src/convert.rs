//! Halo 1 AI data to Halo 2's, and the result as `map::build` blocks.
//!
//! | Halo 1 | Halo 2 |
//! |---|---|
//! | encounter | squad group, and a zone of the same name |
//! | encounter's firing positions, by group letter | the zone's firing positions, one area per letter used |
//! | squad | squad (in the encounter's group and zone), with an order over the areas of its attacking groups; squads with no starting locations are kept empty (no locations, no actors) so scripts can name them |
//! | squad starting location | squad starting location |
//! | actor variant (`actv` → `actr` type) | a retail `char`, nearest by species and rank ([`character_for`]) |
//! | command list points | a point set of the same name |
//! | command list | an order stub of the same name, plus its atoms as a [`CommandScript`] for the scripts lane |
//!
//! [`convert`] makes the plan as [`h2`] types, so building it and reading it
//! back through [`h2::ScenarioAi::read`] must give the same values.
//! [`Plan::blocks`] writes it; [`ScenarioBlocks::apply`] puts it in a
//! scenario's root block.

use map::TagId;
use map::build::Block;

use crate::h1::{self, ActorType, H1Ai};
use crate::h2::{self, AreaRef, ScenarioAi, layout as l};
use crate::{Error, Result};

/// Values the Halo 1 data has no counterpart for. The defaults come from
/// the retail Halo 2 campaign census (the `h2_retail_census` disc test
/// prints it): the most common firing position flags, zone flags and area
/// type, and order style `-1` (none; 175 retail orders), which needs no
/// style palette entry.
#[derive(Debug, Clone)]
pub struct Options {
    /// Flags of each firing position.
    pub firing_position_flags: u16,
    /// Flags of each zone.
    pub zone_flags: u32,
    /// Area type of each area in an order's area set.
    pub area_kind: u16,
    /// Style palette index of each order.
    pub order_style: i16,
    /// Halo 1 platoons the scripts name (`encounter/platoon`, lower case),
    /// each converted to a squad group of that name under the encounter's
    /// group, holding exactly the platoon's squads. Halo 2's `ai` builtins
    /// walk a group's children (`ai_place`, `ai_actors`, `ai_strength`, ...
    /// on retail's own parent groups, 05a's `all_enemies` among them), so a
    /// platoon argument covers every squad it did in Halo 1 while the
    /// encounter's group still covers them all. Empty: no platoon groups
    /// (the squad's group is its encounter's, as before).
    pub platoon_groups: std::collections::BTreeSet<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            firing_position_flags: 0x42,
            zone_flags: 1,
            area_kind: 0,
            order_style: -1,
            platoon_groups: std::collections::BTreeSet::new(),
        }
    }
}

/// Halo 2 grenade types (`u16`): none, human fragmentation, covenant plasma.
pub mod grenade {
    /// No grenades.
    pub const NONE: u16 = 0;
    /// Fragmentation.
    pub const HUMAN: u16 = 1;
    /// Plasma.
    pub const COVENANT: u16 = 2;
}

/// A retail Halo 2 character chosen for a Halo 1 actor variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterChoice {
    /// The Halo 1 actor variant (`actv`) path.
    pub h1_variant: String,
    /// The Halo 1 actor type.
    pub h1_type: ActorType,
    /// The retail Halo 2 `char` path.
    pub h2_character: &'static str,
    /// Grenades the squad carries.
    pub grenade: u16,
}

/// The stopgap character table: the nearest retail Halo 2 `char` for a
/// Halo 1 actor type, refined by words in the variant's path ("major",
/// "stealth", "zealot", "specops", "human", "johnson", "sergeant"). Every
/// path here is on the Halo 2 disc with the same type (checked by
/// `h2_retail_census`). Halo 2 even ships a Halo 1 sentinel
/// (`sentinel_aggressor_halo1`, on 04a and 04b).
pub fn character_for(kind: ActorType, variant: &str) -> (&'static str, u16) {
    let v = variant.to_ascii_lowercase();
    let has = |w: &str| v.contains(w);
    let path = match kind.0 {
        0 if has("stealth") && has("major") => {
            "objects\\characters\\elite\\ai\\elite_stealth_major"
        }
        0 if has("stealth") => "objects\\characters\\elite\\ai\\elite_stealth",
        0 if has("zealot") => "objects\\characters\\elite\\ai\\elite_zealot",
        0 if has("specops") => "objects\\characters\\elite\\ai\\elite_specops",
        0 if has("major") => "objects\\characters\\elite\\ai\\elite_major",
        0 => "objects\\characters\\elite\\ai\\elite",
        1 if has("major") => "objects\\characters\\jackal\\ai\\jackal_major",
        1 => "objects\\characters\\jackal\\ai\\jackal",
        2 if has("specops") => "objects\\characters\\grunt\\ai\\grunt_specops",
        2 if has("major") => "objects\\characters\\grunt\\ai\\grunt_major",
        2 => "objects\\characters\\grunt\\ai\\grunt",
        3 => "objects\\characters\\hunter\\ai\\hunter",
        7 | 8 if has("johnson") => "objects\\characters\\marine\\ai\\marine_johnson",
        7 | 8 if has("sergeant") || has("sgt") => "objects\\characters\\marine\\ai\\marine_sgt",
        8 => "objects\\characters\\marine\\ai\\crewman",
        7 => "objects\\characters\\marine\\ai\\marine",
        9 if has("human") => "objects\\characters\\floodcombat_human\\ai\\flood_combat_human",
        9 => "objects\\characters\\floodcombat_elite\\ai\\floodcombat_elite",
        10 => "objects\\characters\\flood_infection\\ai\\flood_infection",
        11 => "objects\\characters\\floodcarrier\\ai\\flood_carrier",
        12 => "objects\\characters\\monitor\\ai\\monitor",
        13 => "objects\\characters\\sentinel_aggressor\\ai\\sentinel_aggressor_halo1",
        15 => "ai\\generic_turret",
        _ => "ai\\generic",
    };
    let grenade = match kind.0 {
        0 | 2 => grenade::COVENANT,
        7 | 8 => grenade::HUMAN,
        _ => grenade::NONE,
    };
    (path, grenade)
}

/// A Halo 1 command list's atoms, kept for the scripts lane to turn into
/// a Halo 2 command script over the point set of the same name.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandScript {
    /// The command list's (and point set's, and order's) name.
    pub name: String,
    /// Atoms: type name ([`h1::atom_name`]), and the points they name.
    pub atoms: Vec<(String, [i16; 2])>,
}

/// A conversion: the Halo 2 AI data, with the character palette as paths
/// still to be resolved against the map being built.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The AI data; [`ScenarioAi::character_palette`] is empty until
    /// [`Plan::blocks`] resolves [`Self::characters`].
    pub ai: ScenarioAi,
    /// Character palette: the choice for each entry.
    pub characters: Vec<CharacterChoice>,
    /// Command lists, for the scripts lane.
    pub command_scripts: Vec<CommandScript>,
    /// What didn't carry over, one line each.
    pub notes: Vec<String>,
}

/// A name in a 32-character field (31 and a NUL).
fn name31(s: &str) -> String {
    s.chars().take(31).collect()
}

fn letter(group: u16) -> String {
    char::from(b'a' + group as u8).to_string()
}

/// Converts a Halo 1 scenario's AI data.
pub fn convert(src: &H1Ai, opts: &Options) -> Plan {
    let mut notes = Vec::new();
    let mut ai = ScenarioAi::default();

    // Character palette: one entry per distinct retail character, in the
    // order the actor palette first needs it.
    let mut characters: Vec<CharacterChoice> = Vec::new();
    let palette: Vec<Option<usize>> = (0..src.actor_palette.len())
        .map(|i| {
            let v = src.actor_palette[i].as_ref()?;
            let kind = src.actor_of(i).map(|a| a.kind).unwrap_or(ActorType(14));
            let (path, grenade) = character_for(kind, &v.name);
            if let Some(k) = characters.iter().position(|c| c.h2_character == path) {
                return Some(k);
            }
            characters.push(CharacterChoice {
                h1_variant: v.name.clone(),
                h1_type: kind,
                h2_character: path,
                grenade,
            });
            Some(characters.len() - 1)
        })
        .collect();
    let char_of = |actor: i16| -> Option<usize> {
        usize::try_from(actor)
            .ok()
            .and_then(|a| palette.get(a).copied().flatten())
    };

    for (ei, e) in src.encounters.iter().enumerate() {
        let zone_index = ei as i16;
        // Areas: one per firing position group letter, in letter order.
        let mut letters: Vec<u16> = e.firing_positions.iter().map(|f| f.group).collect();
        letters.sort_unstable();
        letters.dedup();
        let mut fps: Vec<(usize, &h1::FiringPosition)> = e
            .firing_positions
            .iter()
            .map(|f| (letters.binary_search(&f.group).unwrap(), f))
            .collect();
        fps.sort_by_key(|(a, _)| *a);
        let firing_positions: Vec<h2::FiringPosition> = fps
            .iter()
            .map(|(a, f)| h2::FiringPosition {
                position: f.position,
                reference_frame: -1,
                flags: opts.firing_position_flags,
                area: *a as i16,
                cluster: -1,
                surface: f.surface,
                normal: [0.0, std::f32::consts::FRAC_PI_2],
            })
            .collect();
        let areas = letters
            .iter()
            .enumerate()
            .map(|(a, &g)| {
                let first = fps.iter().position(|(x, _)| *x == a).unwrap();
                let pts: Vec<[f32; 3]> = fps
                    .iter()
                    .filter(|(x, _)| *x == a)
                    .map(|(_, f)| f.position)
                    .collect();
                let n = pts.len() as f32;
                let mean: [f32; 3] =
                    std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / n);
                let var = pts
                    .iter()
                    .map(|p| (0..3).map(|k| (p[k] - mean[k]).powi(2)).sum::<f32>())
                    .sum::<f32>()
                    / n;
                h2::Area {
                    name: letter(g),
                    flags: 0,
                    mean,
                    reference_frame: -1,
                    deviation: var.sqrt(),
                    first: first as i16,
                    count: pts.len() as i16,
                    manual_reference_frame: 0,
                }
            })
            .collect();
        if e.firing_positions.is_empty() {
            notes.push(format!(
                "encounter {}: no firing positions, zone has no areas",
                e.name
            ));
        }
        ai.zones.push(h2::Zone {
            name: name31(&e.name),
            flags: opts.zone_flags,
            manual_bsp: e.manual_bsp.max(0),
            firing_positions,
            areas,
        });
        ai.squad_groups.push(h2::SquadGroup {
            name: name31(&e.name),
            parent: -1,
            initial_order: -1,
        });
        let encounter_group = ai.squad_groups.len() as i16 - 1;
        // Platoons the scripts name become child groups of the encounter's,
        // holding exactly their squads: a platoon argument then covers every
        // squad it did in Halo 1 (see `Options::platoon_groups`).
        let mut platoon_of: Vec<Option<i16>> = vec![None; e.platoons.len()];
        for (pi, p) in e.platoons.iter().enumerate() {
            let token = format!("{}/{}", e.name, p.name).to_ascii_lowercase();
            if !opts.platoon_groups.contains(&token) {
                continue;
            }
            ai.squad_groups.push(h2::SquadGroup {
                name: name31(&format!("{}/{}", e.name, p.name)),
                parent: encounter_group,
                initial_order: -1,
            });
            platoon_of[pi] = Some(ai.squad_groups.len() as i16 - 1);
        }

        for s in &e.squads {
            let name = name31(&format!("{}/{}", e.name, s.name));
            // A Halo 1 squad with nowhere to start never spawns, but
            // scripts still name it (migration targets, counts). It is kept
            // with no starting locations and no actors, as retail Halo 2
            // keeps squads that only vehicles, migrations and command
            // scripts fill (06b: 28 of 170; `squad_flag_bits` survey).
            let unplaced = s.starting_locations.is_empty();
            if unplaced {
                notes.push(format!(
                    "{name}: no starting locations, kept empty ({} normal, {} legendary)",
                    s.normal_count, s.insane_count
                ));
            }
            let character = char_of(s.actor);
            if character.is_none() {
                notes.push(format!("{name}: actor {} has no character", s.actor));
            }
            // The order: the areas of the attacking groups, or every area.
            let mut set: Vec<AreaRef> = letters
                .iter()
                .enumerate()
                .filter(|&(_, &g)| s.attacking() & (1 << g) != 0)
                .map(|(a, _)| AreaRef {
                    kind: opts.area_kind,
                    zone: zone_index,
                    area: a as i16,
                })
                .collect();
            if set.is_empty() {
                set = (0..letters.len())
                    .map(|a| AreaRef {
                        kind: opts.area_kind,
                        zone: zone_index,
                        area: a as i16,
                    })
                    .collect();
            }
            // An empty squad has no actors to order: no order (room).
            let order = if unplaced {
                -1
            } else {
                ai.orders.push(h2::Order {
                    name: name.clone(),
                    style: opts.order_style,
                    flags: 0,
                    force_combat_status: 0,
                    entry_script: String::new(),
                    entry_script_index: -1,
                    follow_squad: -1,
                    follow_radius: 0.0,
                    primary_areas: set,
                    secondary_areas: Vec::new(),
                    endings: 0,
                });
                ai.orders.len() as i16 - 1
            };
            let grenade = character.map(|c| characters[c].grenade).unwrap_or(0);
            let starting_locations = s
                .starting_locations
                .iter()
                .map(|l| h2::StartingLocation {
                    name: 0,
                    position: l.position,
                    reference_frame: -1,
                    facing: [l.facing, 0.0],
                    flags: 0,
                    character: if l.actor == -1 {
                        -1
                    } else {
                        char_of(l.actor).map(|c| c as i16).unwrap_or(-1)
                    },
                    initial_weapon: -1,
                    initial_secondary_weapon: -1,
                    vehicle: -1,
                    seat_type: 0,
                    grenade_type: 0,
                    swarm_count: 0,
                    emitter_vehicle: -1,
                })
                .collect::<Vec<_>>();
            ai.squads.push(h2::Squad {
                name,
                flags: 0,
                team: e.team,
                parent: usize::try_from(s.platoon)
                    .ok()
                    .and_then(|i| platoon_of.get(i).copied().flatten())
                    .unwrap_or(encounter_group),
                delay: s.delay,
                normal_count: if unplaced { 0 } else { s.normal_count },
                insane_count: if unplaced { 0 } else { s.insane_count },
                major_upgrade: s.major_upgrade,
                vehicle: -1,
                character: character.map(|c| c as i16).unwrap_or(-1),
                initial_zone: zone_index,
                initial_weapon: -1,
                initial_secondary_weapon: -1,
                grenade_type: grenade,
                initial_order: order,
                starting_locations,
            });
        }
    }

    let mut command_scripts = Vec::new();
    for c in &src.command_lists {
        ai.point_sets.push(h2::PointSet {
            name: name31(&c.name),
            points: c
                .points
                .iter()
                .enumerate()
                .map(|(i, p)| h2::Point {
                    name: format!("p{i}"),
                    position: p.position,
                    reference_frame: -1,
                    surface: p.surface,
                    facing: [0.0, 0.0],
                })
                .collect(),
            bsp: c.manual_bsp.max(0),
            manual_reference_frame: -1,
            flags: 0,
        });
        ai.orders.push(h2::Order {
            name: name31(&c.name),
            style: opts.order_style,
            flags: 0,
            force_combat_status: 0,
            entry_script: String::new(),
            entry_script_index: -1,
            follow_squad: -1,
            follow_radius: 0.0,
            primary_areas: Vec::new(),
            secondary_areas: Vec::new(),
            endings: 0,
        });
        command_scripts.push(CommandScript {
            name: c.name.clone(),
            atoms: c
                .atoms
                .iter()
                .map(|a| (h1::atom_name(a.kind), a.points))
                .collect(),
        });
    }
    if !src.conversations.is_empty() {
        notes.push(format!(
            "{} conversations not converted",
            src.conversations.len()
        ));
    }
    Plan {
        ai,
        characters,
        command_scripts,
        notes,
    }
}

/// Blocks for the scenario's AI fields: `(field offset, count, child)`.
#[derive(Debug, Clone)]
pub struct ScenarioBlocks {
    /// One entry per scenario field written.
    pub fields: Vec<(u32, u32, Block)>,
}

impl ScenarioBlocks {
    /// Puts each block on `scnr`, the scenario's root block, replacing what
    /// those fields held.
    pub fn apply(self, scnr: &mut Block) -> Result<()> {
        for (at, count, child) in self.fields {
            if scnr.bytes.len() < at as usize + 8 {
                return Err(Error::Invalid(format!(
                    "scenario root has no field {at:#x}"
                )));
            }
            scnr.fixups.retain(|f| f.offset != at);
            scnr.bytes[at as usize..at as usize + 8].fill(0);
            if count > 0 {
                scnr.block(at, count, child);
            }
        }
        Ok(())
    }
}

fn put_name(b: &mut Block, at: usize, s: &str) {
    let n = s.len().min(31);
    b.bytes[at..at + n].copy_from_slice(&s.as_bytes()[..n]);
}

fn put_i16(b: &mut Block, at: usize, v: i16) {
    b.put_u16(at, v as u16);
}

fn put_f32s(b: &mut Block, at: usize, v: &[f32]) {
    for (i, x) in v.iter().enumerate() {
        b.put_f32(at + 4 * i, *x);
    }
}

/// Concatenates fixed-size elements into one block-array child.
fn array(elements: Vec<Block>, stride: usize) -> Block {
    let mut out = Block::zeroed(elements.len() * stride);
    for (i, e) in elements.into_iter().enumerate() {
        debug_assert_eq!(e.bytes.len(), stride);
        out.bytes[i * stride..(i + 1) * stride].copy_from_slice(&e.bytes);
        for mut f in e.fixups {
            f.offset += (i * stride) as u32;
            out.fixups.push(f);
        }
    }
    out
}

fn child(b: &mut Block, (at, stride): (usize, usize), elements: Vec<Block>) {
    if !elements.is_empty() {
        let n = elements.len() as u32;
        b.block(at as u32, n, array(elements, stride));
    }
}

/// [`child`] for an array with no fixups that zones often repeat (Halo 1
/// encounters copy firing positions: b40's 20789 hold 8335 distinct). The
/// builder places identical arrays once, keyed by their bytes.
fn shared_child(b: &mut Block, (at, stride): (usize, usize), elements: Vec<Block>) {
    use std::hash::{Hash, Hasher};
    if !elements.is_empty() {
        let n = elements.len() as u32;
        let a = array(elements, stride);
        debug_assert!(a.fixups.is_empty());
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ("h1ai zone array", at, &a.bytes).hash(&mut h);
        let key = h.finish();
        b.block(at as u32, n, a.shared(key));
    }
}

fn area_refs(set: &[AreaRef]) -> Vec<Block> {
    set.iter()
        .map(|r| {
            let mut e = Block::zeroed(8);
            e.put_u16(0, r.kind);
            put_i16(&mut e, 4, r.zone);
            put_i16(&mut e, 6, r.area);
            e
        })
        .collect()
}

impl Plan {
    /// The scenario blocks, with each character path resolved to a datum by
    /// `resolve` (the map being built must hold the tag). Fails on a path
    /// that doesn't resolve.
    pub fn blocks(&self, resolve: impl Fn(&str) -> Option<TagId>) -> Result<ScenarioBlocks> {
        let ai = &self.ai;
        let mut fields = Vec::new();
        let mut push = |(at, stride): (usize, usize), elements: Vec<Block>| {
            fields.push((at as u32, elements.len() as u32, array(elements, stride)));
        };

        let chars = self
            .characters
            .iter()
            .map(|c| {
                let id = resolve(c.h2_character)
                    .ok_or_else(|| Error::Invalid(format!("no char {}", c.h2_character)))?;
                let mut e = Block::zeroed(8);
                e.tag_ref(0, id);
                Ok(e)
            })
            .collect::<Result<Vec<_>>>()?;
        push(l::CHARACTER_PALETTE, chars);

        push(
            l::SQUAD_GROUPS,
            ai.squad_groups
                .iter()
                .map(|g| {
                    let mut e = Block::zeroed(l::SQUAD_GROUPS.1);
                    put_name(&mut e, 0, &g.name);
                    put_i16(&mut e, 0x20, g.parent);
                    put_i16(&mut e, 0x22, g.initial_order);
                    e
                })
                .collect(),
        );

        push(
            l::SQUADS,
            ai.squads
                .iter()
                .map(|s| {
                    let mut e = Block::zeroed(l::SQUADS.1);
                    put_name(&mut e, 0, &s.name);
                    e.put_u32(0x20, s.flags);
                    e.put_u16(0x24, s.team);
                    put_i16(&mut e, 0x26, s.parent);
                    e.put_f32(0x28, s.delay);
                    put_i16(&mut e, 0x2c, s.normal_count);
                    put_i16(&mut e, 0x2e, s.insane_count);
                    e.put_u16(0x30, s.major_upgrade);
                    put_i16(&mut e, 0x34, s.vehicle);
                    put_i16(&mut e, 0x36, s.character);
                    put_i16(&mut e, 0x38, s.initial_zone);
                    put_i16(&mut e, 0x3c, s.initial_weapon);
                    put_i16(&mut e, 0x3e, s.initial_secondary_weapon);
                    e.put_u16(0x40, s.grenade_type);
                    put_i16(&mut e, 0x42, s.initial_order);
                    put_i16(&mut e, 0x70, -1);
                    let locs = s
                        .starting_locations
                        .iter()
                        .map(|t| {
                            let mut b = Block::zeroed(l::SQUAD_STARTING_LOCATIONS.1);
                            b.put_u32(0, t.name);
                            put_f32s(&mut b, 4, &t.position);
                            put_i16(&mut b, 0x10, t.reference_frame);
                            put_f32s(&mut b, 0x14, &t.facing);
                            b.put_u32(0x1c, t.flags);
                            put_i16(&mut b, 0x20, t.character);
                            put_i16(&mut b, 0x22, t.initial_weapon);
                            put_i16(&mut b, 0x24, t.initial_secondary_weapon);
                            put_i16(&mut b, 0x28, t.vehicle);
                            b.put_u16(0x2a, t.seat_type);
                            b.put_u16(0x2c, t.grenade_type);
                            put_i16(&mut b, 0x2e, t.swarm_count);
                            put_i16(&mut b, 0x3c, t.emitter_vehicle);
                            put_i16(&mut b, 0x60, -1);
                            b
                        })
                        .collect();
                    child(&mut e, l::SQUAD_STARTING_LOCATIONS, locs);
                    e
                })
                .collect(),
        );

        push(
            l::ZONES,
            ai.zones
                .iter()
                .map(|z| {
                    let mut e = Block::zeroed(l::ZONES.1);
                    put_name(&mut e, 0, &z.name);
                    e.put_u32(0x20, z.flags);
                    put_i16(&mut e, 0x24, z.manual_bsp);
                    let fps = z
                        .firing_positions
                        .iter()
                        .map(|f| {
                            let mut b = Block::zeroed(l::ZONE_FIRING_POSITIONS.1);
                            put_f32s(&mut b, 0, &f.position);
                            put_i16(&mut b, 0xc, f.reference_frame);
                            b.put_u16(0xe, f.flags);
                            put_i16(&mut b, 0x10, f.area);
                            put_i16(&mut b, 0x12, f.cluster);
                            b.put_u32(0x14, f.surface);
                            put_f32s(&mut b, 0x18, &f.normal);
                            b
                        })
                        .collect();
                    shared_child(&mut e, l::ZONE_FIRING_POSITIONS, fps);
                    let areas = z
                        .areas
                        .iter()
                        .map(|a| {
                            let mut b = Block::zeroed(l::ZONE_AREAS.1);
                            put_name(&mut b, 0, &a.name);
                            b.put_u32(0x20, a.flags);
                            put_f32s(&mut b, 0x24, &a.mean);
                            put_i16(&mut b, 0x30, a.reference_frame);
                            b.put_f32(0x34, a.deviation);
                            put_i16(&mut b, 0x38, a.first);
                            put_i16(&mut b, 0x3a, a.count);
                            put_i16(&mut b, 0x7c, a.manual_reference_frame);
                            b
                        })
                        .collect();
                    shared_child(&mut e, l::ZONE_AREAS, areas);
                    e
                })
                .collect(),
        );

        push(
            l::ORDERS,
            ai.orders
                .iter()
                .map(|o| {
                    let mut e = Block::zeroed(l::ORDERS.1);
                    put_name(&mut e, 0, &o.name);
                    put_i16(&mut e, 0x20, o.style);
                    e.put_u32(0x24, o.flags);
                    e.put_u16(0x28, o.force_combat_status);
                    put_name(&mut e, 0x2c, &o.entry_script);
                    put_i16(&mut e, 0x4c, o.entry_script_index);
                    put_i16(&mut e, 0x4e, o.follow_squad);
                    e.put_f32(0x50, o.follow_radius);
                    child(&mut e, l::ORDER_PRIMARY_AREAS, area_refs(&o.primary_areas));
                    child(
                        &mut e,
                        l::ORDER_SECONDARY_AREAS,
                        area_refs(&o.secondary_areas),
                    );
                    e
                })
                .collect(),
        );

        let sets = ai
            .point_sets
            .iter()
            .map(|s| {
                let mut e = Block::zeroed(l::POINT_SETS.1);
                put_name(&mut e, 0, &s.name);
                put_i16(&mut e, 0x28, s.bsp);
                put_i16(&mut e, 0x2a, s.manual_reference_frame);
                e.put_u32(0x2c, s.flags);
                let pts = s
                    .points
                    .iter()
                    .map(|p| {
                        let mut b = Block::zeroed(l::POINTS.1);
                        put_name(&mut b, 0, &p.name);
                        put_f32s(&mut b, 0x20, &p.position);
                        put_i16(&mut b, 0x2c, p.reference_frame);
                        b.put_u32(0x30, p.surface);
                        put_f32s(&mut b, 0x34, &p.facing);
                        b
                    })
                    .collect();
                child(&mut e, l::POINTS, pts);
                e
            })
            .collect();
        let mut sd = Block::zeroed(l::SCRIPTING_DATA.1);
        child(&mut sd, l::POINT_SETS, sets);
        push(l::SCRIPTING_DATA, vec![sd]);

        Ok(ScenarioBlocks { fields })
    }
}
