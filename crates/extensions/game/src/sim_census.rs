//! The guest census of a sim match (docs/runtime/sim.md §6,
//! `guest_census`; docs/perf/sim-headless-skips.md, lane H0): the
//! presentation-side state a headless match still holds, read once a tick
//! by the sim's tick observer and live by `sim.guest_census`.
//!
//! A few guest reads, no walk: the local-user count, the local players'
//! visibility set and the live count in the header of five data arrays.
//! An array is counted only when its header carries the data-array
//! signature and the name the title gave it, so a wrong pointer reads as
//! `None`, never as a count.

use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use guest_layout::datum::{SIGNATURE, layout};
use serde_json::{Value, json};
use sim_bench::metrics::{CENSUS_ARRAYS, CensusSample, GuestCensus};

/// The agent method answering the census live.
pub const METHOD: &str = agent_proto::method::SIM_GUEST_CENSUS;

/// The local-user count, `i16` at `+8` of the players' globals
/// (`guest_layout::PlayerGlobals::LOCAL_COUNT`); the phantom builder sets
/// it to 1 in its private view only (`bot_glue::retail_input`).
const LOCAL_COUNT: u32 = guest_layout::game::PlayerGlobals::LOCAL_COUNT.offset;

/// The local players' visibility set: `0x40` bytes (512 cluster bits) at
/// game globals `+0x1178`. `0x137d5c` clears its sixteen words; the tick's
/// visibility pass `0x138c5f` builds it with the local flag set, and
/// `0x150820` ORs in only players with a local user (`0x150891`).
const LOCAL_VISIBILITY: u32 = 0x1178;
const VISIBILITY_BYTES: usize = 0x40;

/// The pointer word to each array's header, in [`CENSUS_ARRAYS`] order,
/// with the name its initialiser gives it. Read from the retail XBE with
/// `h2 disasm`:
///
/// - `effect`: `0x175b93` stores the header from `0x16b5f0` (name
///   `0x453b44`, stride `0x190`, 256) to `[0x4ea93c]`;
/// - `particle_system`: `0x173e29`, `[0x510c74]` (name `0x453b24`, `0x54`, 128);
/// - `particles`: `0x173e62`, `[0x51ec84]` (name `0x459e50`, `0x40`, 1024);
/// - `decals`: `0x17d263`, `[0x4ea950]` (name `0x453b68`, `0x40`, 1024);
/// - `lights`: `0xbfffb`, `[0x4e030c]` (name `0x45323c`, `0x110`, 350),
///   the array `engine::sim_objects::lights` walks.
///
/// docs/engine/engine-map.md ("Effects: particles and particle movement",
/// "Effects: damage, contrail, decal, light, lens flare") lists the same.
const ARRAYS: [(u32, &str); CENSUS_ARRAYS.len()] = [
    (0x004e_a93c, "effect"),
    (0x0051_0c74, "particle_system"),
    (0x0051_ec84, "particles"),
    (0x004e_a950, "decals"),
    (0x004e_030c, "lights"),
];

/// This tick's census.
pub fn read(m: &dyn GuestMemory) -> CensusSample {
    let mut arrays = [None; CENSUS_ARRAYS.len()];
    for (out, (word, name)) in arrays.iter_mut().zip(ARRAYS) {
        *out = live_count(m, word, name);
    }
    CensusSample {
        local_users: local_users(m),
        local_visibility_empty: local_visibility_empty(m),
        arrays,
    }
}

fn pointer(m: &dyn GuestMemory, word: u32) -> Option<u32> {
    m.u32(word).ok().filter(|&p| p != 0)
}

fn local_users(m: &dyn GuestMemory) -> Option<u32> {
    let globals = pointer(m, guest_layout::points::PLAYER_GLOBALS.base)?;
    let count = m.u16(globals.wrapping_add(LOCAL_COUNT)).ok()? as i16;
    u32::try_from(count).ok()
}

fn local_visibility_empty(m: &dyn GuestMemory) -> Option<bool> {
    let globals = pointer(m, guest_layout::points::GAME_GLOBALS.base)?;
    let mut set = [0; VISIBILITY_BYTES];
    m.read(globals.wrapping_add(LOCAL_VISIBILITY), &mut set)
        .ok()?;
    Some(set.iter().all(|&b| b == 0))
}

/// The live count of the data array whose header `word` points at, when
/// the header is a data array named `name`.
fn live_count(m: &dyn GuestMemory, word: u32, name: &str) -> Option<u32> {
    let header = pointer(m, word)?;
    if m.u32(header.wrapping_add(layout::SIGNATURE)).ok()? != SIGNATURE {
        return None;
    }
    // The name and its terminator.
    let mut got = [0; 16];
    let want = name.as_bytes();
    let got = &mut got[..=want.len()];
    m.read(header.wrapping_add(layout::NAME), got).ok()?;
    if &got[..want.len()] != want || got[want.len()] != 0 {
        return None;
    }
    let count = m.i32(header.wrapping_add(layout::ACTIVE_COUNT)).ok()?;
    u32::try_from(count).ok()
}

/// `sim.guest_census`'s answer: this tick's read (`now`, `null` with no
/// guest) and the match's block so far (`match`, `null` before a counted
/// tick). It takes no parameters.
pub fn answer(
    params: &Value,
    now: Option<CensusSample>,
    so_far: Option<GuestCensus>,
) -> Result<Value, RpcError> {
    if !params.is_null() && !params.as_object().is_some_and(|p| p.is_empty()) {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("{METHOD} takes no parameters"),
        ));
    }
    Ok(json!({
        "now": now.map(|s| s.to_json()),
        "match": so_far,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const PLAYER_GLOBALS: u32 = 0x0060_0000;
    const GAME_GLOBALS: u32 = 0x0061_0000;
    const HEADERS: u32 = 0x0062_0000;

    fn header(g: &mut FakeGuest, at: u32, name: &str, live: i32) {
        let mut bytes = [0u8; layout::HEADER_SIZE as usize];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        bytes[layout::SIGNATURE as usize..][..4].copy_from_slice(&SIGNATURE.to_le_bytes());
        bytes[layout::ACTIVE_COUNT as usize..][..4].copy_from_slice(&live.to_le_bytes());
        g.put(at, &bytes);
    }

    /// A guest as a phantom host leaves it: no local user, an empty local
    /// set, and every array set up.
    fn phantom_host() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.put_u32(guest_layout::points::PLAYER_GLOBALS.base, PLAYER_GLOBALS)
            .put_u32(guest_layout::points::GAME_GLOBALS.base, GAME_GLOBALS)
            .map(PLAYER_GLOBALS, 0x1c)
            .map(GAME_GLOBALS, 0x1200);
        for (i, ((word, name), live)) in ARRAYS.iter().zip([3, 2, 40, 1, 12]).enumerate() {
            let at = HEADERS + 0x100 * i as u32;
            g.put_u32(*word, at);
            header(&mut g, at, name, live);
        }
        g
    }

    #[test]
    fn reads_the_users_the_local_set_and_each_arrays_live_count() {
        let mut g = phantom_host();
        assert_eq!(
            read(&g),
            CensusSample {
                local_users: Some(0),
                local_visibility_empty: Some(true),
                arrays: [Some(3), Some(2), Some(40), Some(1), Some(12)],
            }
        );
        // A local user, and one cluster bit in the last word of the set.
        g.put_u16(PLAYER_GLOBALS + LOCAL_COUNT, 1)
            .put_u8(GAME_GLOBALS + LOCAL_VISIBILITY + 0x3f, 0x80);
        let s = read(&g);
        assert_eq!(s.local_users, Some(1));
        assert_eq!(s.local_visibility_empty, Some(false));
        // The byte past the set is not part of it.
        g.put_u8(GAME_GLOBALS + LOCAL_VISIBILITY + 0x3f, 0)
            .put_u8(GAME_GLOBALS + LOCAL_VISIBILITY + 0x40, 0xff);
        assert_eq!(read(&g).local_visibility_empty, Some(true));
    }

    #[test]
    fn an_array_counts_only_with_its_signature_and_its_name() {
        let mut g = phantom_host();
        // `effect location` begins with `effect`; the terminator tells them apart.
        header(&mut g, HEADERS, "effect location", 9);
        // A header without the signature.
        g.put_u32(HEADERS + 0x100 + layout::SIGNATURE, 0);
        // A null pointer: the array is not made yet.
        g.put_u32(ARRAYS[2].0, 0);
        // A pointer to unmapped memory.
        g.put_u32(ARRAYS[3].0, 0x0070_0000);
        // A negative count.
        header(&mut g, HEADERS + 0x400, "lights", -1);
        assert_eq!(read(&g).arrays, [None; 5]);
    }

    #[test]
    fn nothing_is_read_before_the_globals_exist() {
        let mut g = FakeGuest::new();
        g.map(0x004e_0000, 0x4_0000).map(0x0051_0000, 0x1_0000);
        assert_eq!(read(&g), CensusSample::default());
        // Unmapped globals read as nothing too.
        assert_eq!(read(&FakeGuest::new()), CensusSample::default());
    }

    #[test]
    fn the_method_takes_no_parameters_and_answers_now_and_the_match() {
        let now = read(&phantom_host());
        let v = answer(&Value::Null, Some(now), None).unwrap();
        assert_eq!(v["now"]["local_users"], 0);
        assert_eq!(v["now"]["arrays"]["particles"], 40);
        assert!(v["match"].is_null());
        let so_far = GuestCensus {
            ticks: 5,
            ..GuestCensus::default()
        };
        let v = answer(&json!({}), None, Some(so_far)).unwrap();
        assert!(v["now"].is_null());
        assert_eq!(v["match"]["ticks"], 5);
        let e = answer(&json!({"tick": 3}), None, None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }

    #[test]
    fn the_arrays_follow_the_reports_names() {
        let names: Vec<&str> = ARRAYS.iter().map(|(_, n)| *n).collect();
        assert_eq!(names, CENSUS_ARRAYS);
    }
}
