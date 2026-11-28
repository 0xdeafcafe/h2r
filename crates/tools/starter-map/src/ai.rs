//! A campaign map's AI, written into its scenario before the build: one
//! character in the palette, one zone of firing positions (one area, all
//! in cluster 0, no pathfinding sector), one order whose primary area is
//! that area, and one squad of the character in the zone under the order,
//! with one starting location.
//!
//! The layouts are the ones `h1campaign::ai::h2` reads and writes for
//! converted levels (`scnr +0x160` squads, `+0x168` zones, `+0x178` the
//! character palette, `+0x240` orders), the values the ones its converter
//! gives a level with no Halo 1 setting of its own (`ai::convert::Options`'
//! defaults: firing position flags `0x42`, zone flags 1, area type 0,
//! order style -1). The structure the floor writes has no pathfinding, so
//! the squad stands where it is placed and fights from there: it can be
//! placed, and moved once a nav mesh is built for the map.

use std::f32::consts::FRAC_PI_2;

use map::FourCC;
use content_kit::stage::map::set_block;
use tags::repo::{Node, TagKey, Tree, Value};

use crate::template::Ai;
use crate::{Error, Result};

/// `scnr +0x160`: squads, `0x74` each; a squad's starting locations at
/// `+0x48`, `0x64` each.
pub const SQUADS: (u32, u32) = (0x160, 0x74);
pub const SQUAD_STARTS: (u32, u32) = (0x48, 0x64);
/// `scnr +0x168`: zones, `0x38` each; a zone's firing positions at
/// `+0x28` (`0x20` each) and areas at `+0x30` (`0x88` each).
pub const ZONES: (u32, u32) = (0x168, 0x38);
pub const FIRING_POSITIONS: (u32, u32) = (0x28, 0x20);
pub const AREAS: (u32, u32) = (0x30, 0x88);
/// `scnr +0x178`: the character palette, a `char` reference each.
pub const CHARACTERS: (u32, u32) = (0x178, 0x8);
/// `scnr +0x240`: orders, `0x7c` each; an order's primary areas at
/// `+0x54`, `8` each.
pub const ORDERS: (u32, u32) = (0x240, 0x7c);
pub const PRIMARY_AREAS: (u32, u32) = (0x54, 0x8);

pub const CHAR: FourCC = FourCC(*b"char");

/// What [`write`] wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Written {
    pub character: String,
    pub squads: usize,
    pub actors: i16,
    pub zones: usize,
    pub firing_positions: usize,
    pub orders: usize,
}

fn node(size: u32, count: u32) -> Node {
    Node {
        bytes: vec![0; (size * count) as usize],
        count,
        size,
        align: 4,
        fields: Vec::new(),
    }
}

fn put_u32(b: &mut [u8], at: u32, v: u32) {
    b[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_u16(b: &mut [u8], at: u32, v: u16) {
    b[at as usize..at as usize + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_i16(b: &mut [u8], at: u32, v: i16) {
    b[at as usize..at as usize + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_f32s(b: &mut [u8], at: u32, v: &[f32]) {
    for (i, x) in v.iter().enumerate() {
        let o = at as usize + 4 * i;
        b[o..o + 4].copy_from_slice(&x.to_le_bytes());
    }
}

/// A 32-byte inline name: up to 31 bytes and a zero.
fn put_name(b: &mut [u8], at: u32, name: &str) {
    let n = name.len().min(31);
    b[at as usize..at as usize + n].copy_from_slice(&name.as_bytes()[..n]);
}

/// Writes the AI (module docs) into `tree`, the scenario, whose squads,
/// zones, character palette and orders the caller has emptied.
pub fn write(tree: &mut Tree, ai: &Ai) -> Result<Written> {
    if tree.nodes.is_empty() || (tree.nodes[0].bytes.len() as u32) < ORDERS.0 + 8 {
        return Err(Error::Base(format!(
            "the scenario's root is too short for the AI blocks (+{:#x})",
            ORDERS.0
        )));
    }
    for (at, what) in [
        (SQUADS.0, "squads"),
        (ZONES.0, "zones"),
        (CHARACTERS.0, "the character palette"),
        (ORDERS.0, "orders"),
    ] {
        let at = at as usize;
        let count = u32::from_le_bytes(tree.nodes[0].bytes[at..at + 4].try_into().expect("4"));
        if count != 0 {
            return Err(Error::Template(format!(
                "campaign: {what} (+{at:#x}) are not in the clear list; the base's {count} would stay"
            )));
        }
    }

    // The character palette: one `char`.
    let mut palette = node(CHARACTERS.1, 1);
    put_u32(&mut palette.bytes, 0, CHAR.as_u32());
    palette
        .fields
        .push((4, Value::Ref(TagKey::new(CHAR, ai.character.clone()))));
    set_block(tree, 0, CHARACTERS.0, palette);

    // The zone: its firing positions, all in one area.
    let fps = &ai.zone.firing_positions;
    let n = fps.len();
    let mut zone = node(ZONES.1, 1);
    put_name(&mut zone.bytes, 0, &ai.zone.name);
    put_u32(&mut zone.bytes, 0x20, 1);
    put_i16(&mut zone.bytes, 0x24, 0);
    let zone_id = set_block(tree, 0, ZONES.0, zone);
    let mut positions = node(FIRING_POSITIONS.1, n as u32);
    for (i, p) in fps.iter().enumerate() {
        let b = &mut positions.bytes[i * FIRING_POSITIONS.1 as usize..];
        put_f32s(b, 0, p);
        put_i16(b, 0x0c, -1);
        put_u16(b, 0x0e, 0x42);
        put_i16(b, 0x10, 0);
        put_i16(b, 0x12, 0);
        put_u32(b, 0x14, u32::MAX);
        put_f32s(b, 0x18, &[0.0, FRAC_PI_2]);
    }
    set_block(tree, zone_id, FIRING_POSITIONS.0, positions);
    let k = n as f32;
    let mean: [f32; 3] = std::array::from_fn(|a| fps.iter().map(|p| p[a]).sum::<f32>() / k);
    let var = fps
        .iter()
        .map(|p| (0..3).map(|a| (p[a] - mean[a]).powi(2)).sum::<f32>())
        .sum::<f32>()
        / k;
    let mut area = node(AREAS.1, 1);
    put_name(&mut area.bytes, 0, "a");
    put_f32s(&mut area.bytes, 0x24, &mean);
    put_i16(&mut area.bytes, 0x30, -1);
    put_f32s(&mut area.bytes, 0x34, &[var.sqrt()]);
    put_i16(&mut area.bytes, 0x38, 0);
    put_i16(&mut area.bytes, 0x3a, n as i16);
    // Cluster 0: the floor's one cluster.
    put_u32(&mut area.bytes, 0x3c, 1);
    set_block(tree, zone_id, AREAS.0, area);

    // The order: the zone's area as its primary area.
    let mut order = node(ORDERS.1, 1);
    put_name(&mut order.bytes, 0, &ai.zone.name);
    put_i16(&mut order.bytes, 0x20, -1);
    put_i16(&mut order.bytes, 0x4c, -1);
    put_i16(&mut order.bytes, 0x4e, -1);
    let order_id = set_block(tree, 0, ORDERS.0, order);
    let mut primary = node(PRIMARY_AREAS.1, 1);
    put_u16(&mut primary.bytes, 0, 0);
    put_i16(&mut primary.bytes, 4, 0);
    put_i16(&mut primary.bytes, 6, 0);
    set_block(tree, order_id, PRIMARY_AREAS.0, primary);

    // The squad, in the zone under the order, and its starting location.
    let s = &ai.squad;
    let mut squad = node(SQUADS.1, 1);
    put_name(&mut squad.bytes, 0, &s.name);
    put_u16(&mut squad.bytes, 0x24, ai.team);
    put_i16(&mut squad.bytes, 0x26, -1);
    put_i16(&mut squad.bytes, 0x2c, s.count);
    put_i16(&mut squad.bytes, 0x2e, s.count);
    put_i16(&mut squad.bytes, 0x34, -1);
    put_i16(&mut squad.bytes, 0x36, 0);
    put_i16(&mut squad.bytes, 0x38, 0);
    put_i16(&mut squad.bytes, 0x3c, -1);
    put_i16(&mut squad.bytes, 0x3e, -1);
    put_i16(&mut squad.bytes, 0x42, 0);
    put_i16(&mut squad.bytes, 0x70, -1);
    let squad_id = set_block(tree, 0, SQUADS.0, squad);
    let mut start = node(SQUAD_STARTS.1, 1);
    put_f32s(&mut start.bytes, 0x04, &s.at);
    put_i16(&mut start.bytes, 0x10, -1);
    put_f32s(&mut start.bytes, 0x14, &[s.facing.to_radians(), 0.0]);
    for at in [0x20, 0x22, 0x24, 0x28, 0x3c, 0x60] {
        put_i16(&mut start.bytes, at, -1);
    }
    set_block(tree, squad_id, SQUAD_STARTS.0, start);

    Ok(Written {
        character: ai.character.clone(),
        squads: 1,
        actors: s.count,
        zones: 1,
        firing_positions: n,
        orders: 1,
    })
}
