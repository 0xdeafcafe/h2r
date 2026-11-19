//! Halo 2's scenario AI blocks and `char` tags, decoded from a v8 cache
//! file's tag data: squad groups, squads and their starting locations,
//! zones with firing positions and areas, orders, point sets (in the
//! scenario's scripting data) and the character palette.
//!
//! Offsets and strides are in [`layout`]; evidence per field is in
//! `docs/halo1/ai.md`. A block field is 8 bytes on Xbox: count, then
//! the address of element 0. A tag reference is 8 bytes: group, datum.
//! These are also the types the converter writes, so a conversion reads
//! back into exactly what it was built from.

use map::TagId;
use tags::geometry::TagView;

use crate::Result;
use crate::bytes::{f32_at, fixed_str, i16_at, u16_at, u32_at, vec3};

/// Field offsets and element sizes, `(offset, stride)`.
pub mod layout {
    /// Scenario: squad groups.
    pub const SQUAD_GROUPS: (usize, usize) = (0x158, 0x24);
    /// Scenario: squads.
    pub const SQUADS: (usize, usize) = (0x160, 0x74);
    /// Scenario: zones.
    pub const ZONES: (usize, usize) = (0x168, 0x38);
    /// Scenario: character palette (a `char` reference each).
    pub const CHARACTER_PALETTE: (usize, usize) = (0x178, 0x8);
    /// Scenario: AI conversations (counted only).
    pub const CONVERSATIONS: usize = 0x1a0;
    /// Scenario: scripting data (one element; its point sets).
    pub const SCRIPTING_DATA: (usize, usize) = (0x1d8, 0x80);
    /// Scenario: orders.
    pub const ORDERS: (usize, usize) = (0x240, 0x7c);
    /// The scenario struct is at least this long (the fields above fit).
    pub const SCENARIO_MIN: usize = 0x248;

    /// Squad: starting locations.
    pub const SQUAD_STARTING_LOCATIONS: (usize, usize) = (0x48, 0x64);
    /// Zone: firing positions.
    pub const ZONE_FIRING_POSITIONS: (usize, usize) = (0x28, 0x20);
    /// Zone: areas.
    pub const ZONE_AREAS: (usize, usize) = (0x30, 0x88);
    /// Order: primary area set.
    pub const ORDER_PRIMARY_AREAS: (usize, usize) = (0x54, 0x8);
    /// Order: secondary area set.
    pub const ORDER_SECONDARY_AREAS: (usize, usize) = (0x5c, 0x8);
    /// Order: order endings (counted only).
    pub const ORDER_ENDINGS: (usize, usize) = (0x74, 0x2c);
    /// Scripting data: point sets.
    pub const POINT_SETS: (usize, usize) = (0x00, 0x30);
    /// Point set: points.
    pub const POINTS: (usize, usize) = (0x20, 0x3c);

    /// `char`: the root struct's size.
    pub const CHARACTER_SIZE: usize = 0xec;
    /// `char`: general properties.
    pub const CHARACTER_GENERAL: (usize, usize) = (0x34, 0xc);
}

/// A squad group (`0x24` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct SquadGroup {
    /// Name (32 characters).
    pub name: String,
    /// Parent group, `-1` none (`i16 +0x20`).
    pub parent: i16,
    /// Initial order, `-1` none (`i16 +0x22`).
    pub initial_order: i16,
}

/// A squad's starting location (`0x64` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct StartingLocation {
    /// Name, a string ID (`+0x00`).
    pub name: u32,
    /// Position (`+0x04`).
    pub position: [f32; 3],
    /// Reference frame, `-1` none (`i16 +0x10`).
    pub reference_frame: i16,
    /// Facing: yaw, pitch in radians (`+0x14`).
    pub facing: [f32; 2],
    /// Flags (`u32 +0x1c`).
    pub flags: u32,
    /// Character palette index, `-1` the squad's (`i16 +0x20`).
    pub character: i16,
    /// Weapon palette index, `-1` the squad's (`i16 +0x22`).
    pub initial_weapon: i16,
    /// Secondary weapon palette index (`i16 +0x24`).
    pub initial_secondary_weapon: i16,
    /// Vehicle palette index, `-1` none (`i16 +0x28`).
    pub vehicle: i16,
    /// Seat type (`u16 +0x2a`).
    pub seat_type: u16,
    /// Grenade type (`u16 +0x2c`).
    pub grenade_type: u16,
    /// Swarm count (`i16 +0x2e`).
    pub swarm_count: i16,
    /// The command script to start on placement, name and compiled HS index.
    pub placement_script: String,
    pub placement_script_index: i16,
    /// Emitter vehicle, `-1` none (`i16 +0x3c`).
    pub emitter_vehicle: i16,
}

/// A squad (`0x74` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Squad {
    /// Name (32 characters).
    pub name: String,
    /// Flags (`u32 +0x20`).
    pub flags: u32,
    /// Team (`u16 +0x24`).
    pub team: u16,
    /// Squad group, `-1` none (`i16 +0x26`).
    pub parent: i16,
    /// Seconds before the squad spawns (`f32 +0x28`).
    pub delay: f32,
    /// Actors on normal (`i16 +0x2c`).
    pub normal_count: i16,
    /// Actors on legendary (`i16 +0x2e`).
    pub insane_count: i16,
    /// Major upgrade (`u16 +0x30`).
    pub major_upgrade: u16,
    /// Vehicle palette index, `-1` none (`i16 +0x34`).
    pub vehicle: i16,
    /// Character palette index (`i16 +0x36`).
    pub character: i16,
    /// Zone, `-1` none (`i16 +0x38`).
    pub initial_zone: i16,
    /// Weapon palette index, `-1` the character's (`i16 +0x3c`).
    pub initial_weapon: i16,
    /// Secondary weapon palette index (`i16 +0x3e`).
    pub initial_secondary_weapon: i16,
    /// Grenade type (`u16 +0x40`).
    pub grenade_type: u16,
    /// Order, `-1` none (`i16 +0x42`).
    pub initial_order: i16,
    /// Starting locations (`+0x48`).
    pub starting_locations: Vec<StartingLocation>,
}

/// A firing position (`0x20` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct FiringPosition {
    /// Position (`+0x00`).
    pub position: [f32; 3],
    /// Reference frame, `-1` none (`i16 +0x0c`).
    pub reference_frame: i16,
    /// Flags (`u16 +0x0e`).
    pub flags: u16,
    /// Area index in the zone (`i16 +0x10`).
    pub area: i16,
    /// Cluster (`i16 +0x12`).
    pub cluster: i16,
    /// Pathfinding sector (`u32 +0x14`), or all ones for airborne positions.
    pub surface: u32,
    /// Normal: yaw, pitch in radians (`+0x18`).
    pub normal: [f32; 2],
}

/// An area (`0x88` bytes): a named group of a zone's firing positions.
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    /// Name (32 characters).
    pub name: String,
    /// Flags (`u32 +0x20`).
    pub flags: u32,
    /// Mean of its firing positions (`+0x24`).
    pub mean: [f32; 3],
    /// Reference frame of the mean, `-1` none (`i16 +0x30`).
    pub reference_frame: i16,
    /// Standard deviation of its firing positions' distance from the mean
    /// (`f32 +0x34`).
    pub deviation: f32,
    /// First firing position in the zone (`i16 +0x38`).
    pub first: i16,
    /// Firing position count (`i16 +0x3a`).
    pub count: i16,
    /// Manual reference frame (`i16 +0x7c`).
    pub manual_reference_frame: i16,
}

/// A zone (`0x38` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    /// Name (32 characters).
    pub name: String,
    /// Flags (`u32 +0x20`).
    pub flags: u32,
    /// Manual BSP index (`i16 +0x24`).
    pub manual_bsp: i16,
    /// Firing positions, grouped by area.
    pub firing_positions: Vec<FiringPosition>,
    /// Areas.
    pub areas: Vec<Area>,
}

/// One area an order sends a squad to (`8` bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AreaRef {
    /// Area type (`u16 +0x00`).
    pub kind: u16,
    /// Zone (`i16 +0x04`).
    pub zone: i16,
    /// Area in the zone (`i16 +0x06`).
    pub area: i16,
}

/// An order (`0x7c` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    /// Name (32 characters).
    pub name: String,
    /// Style palette index (`i16 +0x20`).
    pub style: i16,
    /// Flags (`u32 +0x24`).
    pub flags: u32,
    /// Force combat status (`u16 +0x28`).
    pub force_combat_status: u16,
    /// Entry script name (32 characters, `+0x2c`).
    pub entry_script: String,
    /// Static entry script index, or -1 (i16 +0x4c).
    pub entry_script_index: i16,
    /// Squad whose orders this one follows, or -1 (i16 +0x4e).
    pub follow_squad: i16,
    /// Follow radius (`f32 +0x50`).
    pub follow_radius: f32,
    /// Primary area set.
    pub primary_areas: Vec<AreaRef>,
    /// Secondary area set.
    pub secondary_areas: Vec<AreaRef>,
    /// Order endings (counted only; the converter writes none).
    pub endings: usize,
}

/// A scripted point (`0x3c` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    /// Name (32 characters).
    pub name: String,
    /// Position (`+0x20`).
    pub position: [f32; 3],
    /// Reference frame, `-1` none (`i16 +0x2c`).
    pub reference_frame: i16,
    /// Collision surface (`u32 +0x30`).
    pub surface: u32,
    /// Facing: yaw, pitch in radians (`+0x34`).
    pub facing: [f32; 2],
}

/// A point set (`0x30` bytes), what `cs_go_to` and friends name.
#[derive(Debug, Clone, PartialEq)]
pub struct PointSet {
    /// Name (32 characters).
    pub name: String,
    /// Points.
    pub points: Vec<Point>,
    /// BSP index (`i16 +0x28`).
    pub bsp: i16,
    /// Manual reference frame (`i16 +0x2a`).
    pub manual_reference_frame: i16,
    /// Flags (`u32 +0x2c`).
    pub flags: u32,
}

/// A scenario's AI data.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScenarioAi {
    /// Squad groups.
    pub squad_groups: Vec<SquadGroup>,
    /// Squads.
    pub squads: Vec<Squad>,
    /// Zones.
    pub zones: Vec<Zone>,
    /// Character palette: `char` datums.
    pub character_palette: Vec<TagId>,
    /// Orders.
    pub orders: Vec<Order>,
    /// Point sets.
    pub point_sets: Vec<PointSet>,
}

/// A `char` tag's fields the converter uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Character {
    /// Flags (`u32 +0x00`).
    pub flags: u32,
    /// Parent character (`+0x04`); unset fields are inherited from it.
    pub parent: TagId,
    /// Unit (`+0x0c`).
    pub unit: TagId,
    /// Creature (`+0x14`).
    pub creature: TagId,
    /// Style (`+0x1c`).
    pub style: TagId,
    /// Major character (`+0x24`).
    pub major: TagId,
    /// General properties type (`u16 +0x04` of the first element of the
    /// block at `+0x34`), when the tag has one in its own payload; the
    /// numbering is Halo 1's actor type, extended
    /// ([`crate::h1::ActorType`]).
    pub kind: Option<u16>,
}

fn name32(e: &[u8], at: usize) -> String {
    fixed_str(&e[at..at + 0x20])
}

fn area_refs(v: &TagView, e: &[u8], l: (usize, usize)) -> Result<Vec<AreaRef>> {
    Ok(v.block(e, l.0, l.1)?
        .into_iter()
        .map(|a| AreaRef {
            kind: u16_at(a, 0),
            zone: i16_at(a, 4),
            area: i16_at(a, 6),
        })
        .collect())
}

impl ScenarioAi {
    /// Decodes the AI blocks of the scenario struct at the start of `v`.
    pub fn read(v: &TagView) -> Result<Self> {
        let root = v.at(0, layout::SCENARIO_MIN)?;
        let b = |l: (usize, usize)| v.block(root, l.0, l.1);
        let squads = b(layout::SQUADS)?
            .into_iter()
            .map(|e| {
                Ok(Squad {
                    name: name32(e, 0),
                    flags: u32_at(e, 0x20),
                    team: u16_at(e, 0x24),
                    parent: i16_at(e, 0x26),
                    delay: f32_at(e, 0x28),
                    normal_count: i16_at(e, 0x2c),
                    insane_count: i16_at(e, 0x2e),
                    major_upgrade: u16_at(e, 0x30),
                    vehicle: i16_at(e, 0x34),
                    character: i16_at(e, 0x36),
                    initial_zone: i16_at(e, 0x38),
                    initial_weapon: i16_at(e, 0x3c),
                    initial_secondary_weapon: i16_at(e, 0x3e),
                    grenade_type: u16_at(e, 0x40),
                    initial_order: i16_at(e, 0x42),
                    starting_locations: v
                        .block(
                            e,
                            layout::SQUAD_STARTING_LOCATIONS.0,
                            layout::SQUAD_STARTING_LOCATIONS.1,
                        )?
                        .into_iter()
                        .map(|s| StartingLocation {
                            name: u32_at(s, 0),
                            position: vec3(s, 4),
                            reference_frame: i16_at(s, 0x10),
                            facing: [f32_at(s, 0x14), f32_at(s, 0x18)],
                            flags: u32_at(s, 0x1c),
                            character: i16_at(s, 0x20),
                            initial_weapon: i16_at(s, 0x22),
                            initial_secondary_weapon: i16_at(s, 0x24),
                            vehicle: i16_at(s, 0x28),
                            seat_type: u16_at(s, 0x2a),
                            grenade_type: u16_at(s, 0x2c),
                            swarm_count: i16_at(s, 0x2e),
                            emitter_vehicle: i16_at(s, 0x3c),
                            placement_script: name32(s, 0x40),
                            placement_script_index: i16_at(s, 0x60),
                        })
                        .collect(),
                })
            })
            .collect::<Result<_>>()?;
        let zones = b(layout::ZONES)?
            .into_iter()
            .map(|e| {
                Ok(Zone {
                    name: name32(e, 0),
                    flags: u32_at(e, 0x20),
                    manual_bsp: i16_at(e, 0x24),
                    firing_positions: v
                        .block(
                            e,
                            layout::ZONE_FIRING_POSITIONS.0,
                            layout::ZONE_FIRING_POSITIONS.1,
                        )?
                        .into_iter()
                        .map(|f| FiringPosition {
                            position: vec3(f, 0),
                            reference_frame: i16_at(f, 0xc),
                            flags: u16_at(f, 0xe),
                            area: i16_at(f, 0x10),
                            cluster: i16_at(f, 0x12),
                            surface: u32_at(f, 0x14),
                            normal: [f32_at(f, 0x18), f32_at(f, 0x1c)],
                        })
                        .collect(),
                    areas: v
                        .block(e, layout::ZONE_AREAS.0, layout::ZONE_AREAS.1)?
                        .into_iter()
                        .map(|a| Area {
                            name: name32(a, 0),
                            flags: u32_at(a, 0x20),
                            mean: vec3(a, 0x24),
                            reference_frame: i16_at(a, 0x30),
                            deviation: f32_at(a, 0x34),
                            first: i16_at(a, 0x38),
                            count: i16_at(a, 0x3a),
                            manual_reference_frame: i16_at(a, 0x7c),
                        })
                        .collect(),
                })
            })
            .collect::<Result<_>>()?;
        let orders = b(layout::ORDERS)?
            .into_iter()
            .map(|e| {
                Ok(Order {
                    name: name32(e, 0),
                    style: i16_at(e, 0x20),
                    flags: u32_at(e, 0x24),
                    force_combat_status: u16_at(e, 0x28),
                    entry_script: name32(e, 0x2c),
                    entry_script_index: i16_at(e, 0x4c),
                    follow_squad: i16_at(e, 0x4e),
                    follow_radius: f32_at(e, 0x50),
                    primary_areas: area_refs(v, e, layout::ORDER_PRIMARY_AREAS)?,
                    secondary_areas: area_refs(v, e, layout::ORDER_SECONDARY_AREAS)?,
                    endings: u32_at(e, layout::ORDER_ENDINGS.0) as usize,
                })
            })
            .collect::<Result<_>>()?;
        let mut point_sets = Vec::new();
        for sd in b(layout::SCRIPTING_DATA)? {
            for e in v.block(sd, layout::POINT_SETS.0, layout::POINT_SETS.1)? {
                point_sets.push(PointSet {
                    name: name32(e, 0),
                    points: v
                        .block(e, layout::POINTS.0, layout::POINTS.1)?
                        .into_iter()
                        .map(|p| Point {
                            name: name32(p, 0),
                            position: vec3(p, 0x20),
                            reference_frame: i16_at(p, 0x2c),
                            surface: u32_at(p, 0x30),
                            facing: [f32_at(p, 0x34), f32_at(p, 0x38)],
                        })
                        .collect(),
                    bsp: i16_at(e, 0x28),
                    manual_reference_frame: i16_at(e, 0x2a),
                    flags: u32_at(e, 0x2c),
                });
            }
        }
        Ok(Self {
            squad_groups: b(layout::SQUAD_GROUPS)?
                .into_iter()
                .map(|e| SquadGroup {
                    name: name32(e, 0),
                    parent: i16_at(e, 0x20),
                    initial_order: i16_at(e, 0x22),
                })
                .collect(),
            squads,
            zones,
            character_palette: b(layout::CHARACTER_PALETTE)?
                .into_iter()
                .map(|e| TagId(u32_at(e, 4)))
                .collect(),
            orders,
            point_sets,
        })
    }

    /// Every index that must be in range, checked; one message per problem.
    /// Also checks each zone's areas tile its firing positions.
    pub fn check_indices(&self) -> Vec<String> {
        let mut bad = Vec::new();
        let opt = |i: i16, n: usize| i == -1 || (i >= 0 && (i as usize) < n);
        let chars = self.character_palette.len();
        for g in &self.squad_groups {
            if !opt(g.parent, self.squad_groups.len()) || !opt(g.initial_order, self.orders.len()) {
                bad.push(format!(
                    "group {}: parent {} order {}",
                    g.name, g.parent, g.initial_order
                ));
            }
        }
        for s in &self.squads {
            if !opt(s.parent, self.squad_groups.len())
                || !opt(s.character, chars)
                || !opt(s.initial_zone, self.zones.len())
                || !opt(s.initial_order, self.orders.len())
            {
                bad.push(format!(
                    "squad {}: group {} char {} zone {} order {}",
                    s.name, s.parent, s.character, s.initial_zone, s.initial_order
                ));
            }
            for l in &s.starting_locations {
                if !opt(l.character, chars) {
                    bad.push(format!("squad {}: start char {}", s.name, l.character));
                }
            }
        }
        for z in &self.zones {
            for f in &z.firing_positions {
                if !opt(f.area, z.areas.len()) {
                    bad.push(format!("zone {}: firing position area {}", z.name, f.area));
                }
            }
            for (i, a) in z.areas.iter().enumerate() {
                let (first, count) = (a.first as usize, a.count as usize);
                // An empty area is `-1+0` on retail maps.
                let tiles = a.count == 0
                    || a.first >= 0
                        && a.count >= 0
                        && first + count <= z.firing_positions.len()
                        && z.firing_positions[first..first + count]
                            .iter()
                            .all(|f| f.area as usize == i);
                if !tiles {
                    bad.push(format!(
                        "zone {} area {}: {}+{}",
                        z.name, a.name, a.first, a.count
                    ));
                }
            }
        }
        for o in &self.orders {
            for r in o.primary_areas.iter().chain(&o.secondary_areas) {
                let ok = r.zone >= 0
                    && (r.zone as usize) < self.zones.len()
                    && opt(r.area, self.zones[r.zone as usize].areas.len());
                if !ok {
                    bad.push(format!("order {}: zone {} area {}", o.name, r.zone, r.area));
                }
            }
        }
        bad
    }
}

impl Character {
    /// Decodes the `char` struct at the start of `v`.
    pub fn read(v: &TagView) -> Result<Self> {
        let root = v.at(0, layout::CHARACTER_SIZE)?;
        // Some retail tags' general properties point outside their own
        // payload (at another tag's block); those read as "not set here".
        let general = v
            .block(
                root,
                layout::CHARACTER_GENERAL.0,
                layout::CHARACTER_GENERAL.1,
            )
            .unwrap_or_default();
        let id = |at: usize| TagId(u32_at(root, at + 4));
        Ok(Self {
            flags: u32_at(root, 0),
            parent: id(0x04),
            unit: id(0x0c),
            creature: id(0x14),
            style: id(0x1c),
            major: id(0x24),
            kind: general.first().map(|g| u16_at(g, 4)),
        })
    }
}
