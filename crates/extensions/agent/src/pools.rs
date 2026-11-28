//! `world.pools` (docs/runtime/agent-api.md, "world.pools"): how many
//! actors, decals and particles the game has live, read from the headers of
//! their data arrays. Cheap enough to poll every present, which the
//! watermark's `full` level does.

use guest_layout::{GuestMemory, datum::{SIGNATURE, layout}};
use serde_json::{Value, json};

use crate::{Access, Agent};

/// Where the game keeps the decal pool's header pointer.
const DECAL_POOL: u32 = 0x004e_a950;
/// Where the game keeps the particle pool's header pointer.
const PARTICLE_POOL: u32 = 0x0051_ec84;

/// The live count of the data array whose header pointer is at `global`:
/// `None` while there is no array, its signature is wrong, or the count is
/// not within its capacity.
fn live(m: &dyn GuestMemory, global: u32) -> Option<i32> {
    let header = m.u32(global).ok()?;
    if header == 0 || m.u32(header.checked_add(layout::SIGNATURE)?).ok()? != SIGNATURE {
        return None;
    }
    let live = m.i32(header.checked_add(layout::ACTIVE_COUNT)?).ok()?;
    let capacity = m.i32(header.checked_add(layout::MAXIMUM_COUNT)?).ok()?;
    (0..=capacity).contains(&live).then_some(live)
}

/// `{actors, decals, particles}`; a pool that cannot be read is `null`.
fn pools(m: &dyn GuestMemory) -> Value {
    json!({
        "actors": crate::ai::actor_count(m).ok(),
        "decals": live(m, DECAL_POOL),
        "particles": live(m, PARTICLE_POOL),
    })
}

/// Registers `world.pools`.
pub fn install(agent: &Agent) {
    agent.add_game_method("world.pools", Access::Read, |_, ctx| {
        let mut counts = ctx.with_guest(|m| Ok(pools(m)))?;
        counts["at"] = json!(ctx.at);
        Ok(counts)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    fn array(m: &mut FakeGuest, global: u32, header: u32, live: u32, capacity: u32) {
        m.put_u32(global, header)
            .put_u32(header + layout::SIGNATURE, SIGNATURE)
            .put_u32(header + layout::ACTIVE_COUNT, live)
            .put_u32(header + layout::MAXIMUM_COUNT, capacity);
    }

    #[test]
    fn a_pool_reads_its_live_count_only_when_valid() {
        let mut m = FakeGuest::new();
        assert_eq!(live(&m, DECAL_POOL), None);
        array(&mut m, DECAL_POOL, 0x10000, 4, 10);
        assert_eq!(live(&m, DECAL_POOL), Some(4));
        m.put_u32(0x10000 + layout::ACTIVE_COUNT, 11);
        assert_eq!(live(&m, DECAL_POOL), None);
        m.put_u32(0x10000 + layout::ACTIVE_COUNT, 3)
            .put_u32(0x10000 + layout::SIGNATURE, 0);
        assert_eq!(live(&m, DECAL_POOL), None);
    }

    #[test]
    fn the_answer_names_each_pool_and_nulls_the_unreadable() {
        let mut m = FakeGuest::new();
        array(&mut m, PARTICLE_POOL, 0x20000, 130, 1024);
        let v = pools(&m);
        assert_eq!(v["particles"], 130);
        assert!(v["decals"].is_null());
    }
}
