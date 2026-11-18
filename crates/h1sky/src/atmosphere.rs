//! Halo 2's per-cluster atmosphere: the scenario block at `+0x340`
//! (`0xf4` bytes an element), the home of Halo 1's indoor fog.
//!
//! **E** `0x0012efb0`: a structure cluster whose `+0x6f` is not `0xff`
//! takes its fog from element `[+0x6f]` of this block (count `scnr+0x340`,
//! elements `+0x344`) in place of its sky's (`0x00130f60`, `0x00131030`).
//! The element's first group feeds the accumulators the sky's `fog_a`
//! element 0 feeds (`0x0012f234`..`0x0012f2c1` against
//! `0x0013153a`..`0x001315fc`), its second those of `fog_b`
//! (`0x0012f2cd`..`0x0012f36b` against `0x00131611`..`0x001316ba`):
//! colour, then density, start and opaque distance. `+0x10` is the
//! distance over which the element fades in by how far the cluster is from
//! the view, clamped to 10 (`0x45DC00`), an engine default when 0 or less
//! (`0x0012f1c2`..`0x0012f1e1`). `+0x88`..`+0xd4` (patchy fog, `fpch` at
//! `+0xd0`, gated by `+0xa4` above 0) and `+0xe0`..`+0xec` (gated by
//! `+0xe0` above 0) take the place of the sky's `+0x60`..`+0x74`.
//!
//! **V** (every Halo 2 map on the disc): 10 maps fill the block, 34
//! elements, 1 to 12 a map; on 9 of them clusters' `+0x6f` index it, every
//! index inside the block (06a uses 9 of its 12, 03a its second), and no
//! map without the block sets one. 03a's first element is a brown fog,
//! density 0.6, 5 to 200 units, `+0x10` 5; 08a's first fills both groups.
//! `+0x10` is 0 on 21 of the 34, otherwise 1 to 30.
//!
//! **E** Halo 1 `0x000581a0`: a cluster naming no sky (`-1`) takes the fog
//! B of the scenario's first sky; a cluster naming one takes that sky's
//! fog A. So the indoor fog is one per level: sky 0's fog B.

use crate::h1::Fog;

/// Scenario root field: the atmosphere block (count, then elements).
pub const SCENARIO_FIELD: u32 = 0x340;
/// Bytes per element.
pub const LEN: usize = 0xf4;

/// Element field offsets.
pub mod layout {
    /// String id: the element's name.
    pub const NAME: usize = 0x00;
    /// The first fog's colour, three floats (the sky's `fog_a` slot).
    pub const FOG_COLOR: usize = 0x04;
    /// The distance the element fades in over (0: the engine's default).
    pub const FADE_DISTANCE: usize = 0x10;
    /// The first fog's density, 0 to 1.
    pub const FOG_DENSITY: usize = 0x18;
    /// The first fog's start distance.
    pub const FOG_START: usize = 0x1c;
    /// The first fog's opaque distance.
    pub const FOG_OPAQUE: usize = 0x20;
    /// Patchy fog reference (`fpch`), 8 bytes: group, datum.
    pub const PATCHY_FOG: usize = 0xd0;
}

/// An element carrying `fog` as its first fog, nothing else: no second
/// fog, no patchy fog, a fade distance of 0 (the engine's default, as on
/// 21 of the 34 retail elements). The name is the caller's (a string id).
pub fn element(fog: &Fog) -> Vec<u8> {
    let mut b = vec![0u8; LEN];
    let mut put = |at: usize, v: f32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    for (k, c) in fog.color.iter().enumerate() {
        put(layout::FOG_COLOR + 4 * k, *c);
    }
    put(layout::FOG_DENSITY, fog.max_density);
    put(layout::FOG_START, fog.start_distance);
    put(layout::FOG_OPAQUE, fog.opaque_distance);
    // A null `fpch` reference, as retail writes one: the group, datum -1.
    b[layout::PATCHY_FOG..layout::PATCHY_FOG + 4]
        .copy_from_slice(&u32::from_be_bytes(*b"fpch").to_le_bytes());
    b[layout::PATCHY_FOG + 4..layout::PATCHY_FOG + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    b
}

/// The first fog of an element, read back; `None` for a short element.
pub fn fog(e: &[u8]) -> Option<Fog> {
    let f = |at: usize| e.get(at..at + 4).map(|w| f32::from_le_bytes(w.try_into().expect("4")));
    Some(Fog {
        color: [
            f(layout::FOG_COLOR)?,
            f(layout::FOG_COLOR + 4)?,
            f(layout::FOG_COLOR + 8)?,
        ],
        max_density: f(layout::FOG_DENSITY)?,
        start_distance: f(layout::FOG_START)?,
        opaque_distance: f(layout::FOG_OPAQUE)?,
    })
}
