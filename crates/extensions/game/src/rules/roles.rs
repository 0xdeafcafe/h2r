//! Script-owned roles at the native player trait query (vtable slot
//! `+0x8c`). Every engine gets the same answers for a role holder. A
//! nonholder uses the native variant and held-object traits. On retail
//! Juggernaut, the engine also maintains its announcement and team mask.
use std::sync::Arc;

use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::{GuestMemory, MemFault, points};

use super::feature::Shared;
use super::runtime::Runtime;
use crate::actor_net::roles::State;
use crate::firefight::guest as ff;

/// Game-engine index of the retail role setter.
pub const JUGGERNAUT: i32 = 7;
/// In the Juggernaut globals: the role mask, a bit per player index (`u16`).
pub const ROLE_MASK: u32 = 0;

fn fault(e: impl Into<EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

/// The Juggernaut role mask, or none before the Juggernaut globals exist.
pub fn role_mask(m: &dyn GuestMemory) -> Result<Option<u16>, MemFault> {
    let g = m.u32(points::JUGGERNAUT_GLOBALS.base)?;
    if g == 0 {
        return Ok(None);
    }
    Ok(Some(m.u16(g + ROLE_MASK)?))
}

/// What a `role_traits` call does, read before it is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Call the role setter with this salted player datum.
    Set(u32),
    /// No native role change is needed.
    Already,
}

/// Reads what `role_traits(slot, on)` does, or why it cannot.
pub fn plan(m: &dyn GuestMemory, slot: u8, on: bool) -> anyhow::Result<Plan> {
    let read = |e: MemFault| anyhow::anyhow!("guest memory at {:#x} faulted", e.addr);
    if super::guest::engine_index(m).map_err(read)? != Some(JUGGERNAUT) {
        return Ok(Plan::Already);
    }
    let mask = role_mask(m)
        .map_err(read)?
        .ok_or_else(|| anyhow::anyhow!("role_traits: the Juggernaut globals are not set up"))?;
    let datum = ff::player_datum(m, slot)
        .map_err(read)?
        .ok_or_else(|| anyhow::anyhow!("role_traits: no player in slot {slot}"))?;
    // The mask has one bit per player slot.
    let has = mask & (1 << slot) != 0;
    Ok(if has == on { Plan::Already } else { Plan::Set(datum) })
}

/// Gives (`on`) or takes the player's role through the engine's setter.
pub async fn set(cx: &Cx, slot: u8, on: bool) -> anyhow::Result<()> {
    match cx.read(|m| plan(m, slot, on))? {
        Plan::Already => Ok(()),
        Plan::Set(datum) => {
            cx.call(
                GuestCall::new(points::JUGGERNAUT_ROLE_SET.base)
                    .eax(datum)
                    .ecx(u32::from(on)),
            )
            .await
            .map_err(fault)?;
            Ok(())
        }
    }
}

/// The role holders and the exact three trait answers the host replicates.
pub fn network_state(m: &dyn GuestMemory, runtime: &Runtime) -> Option<State> {
    let r = runtime.running().filter(|r| r.authority == Some(true))?;
    let traits = if r.record_engine == JUGGERNAUT {
        // The active record includes the player's edited variant settings.
        let g = m.u32(points::GAME_GLOBALS.base).ok().filter(|&g| g != 0)?;
        let at = g + super::guest::VARIANT + variants::record::TYPE_AREA.start as u32;
        let flags = m.u8(at).ok()?;
        let camo = flags & (1 << 2) != 0;
        let damage = flags & (1 << 4) != 0;
        let resistance = flags & (1 << 6) != 0;
        u8::from(camo) | (u8::from(damage) << 1) | (u8::from(resistance) << 2)
    } else {
        u8::from(r.vm.role_trait(1)?) | (u8::from(r.vm.role_trait(2)?) << 1)
            | (u8::from(r.vm.role_trait(3)?) << 2)
    };
    Some(State { prefix: r.record_prefix, mask: r.vm.ext().role_mask, traits })
}

/// Identity of the active script on a guest, including a no-mode port.
/// A guest can consume the host's state without having the script blob.
fn guest_prefix(m: &dyn GuestMemory) -> Option<[u8; 8]> {
    let block = super::guest::block(m).ok()??;
    let script = variants::ext::Ext::in_effect(&block).script;
    match script.mode {
        variants::ext::Mode::Rules => Some(script.prefix),
        variants::ext::Mode::None => {
            let engine = u8::try_from(super::guest::engine_index(m).ok()??).ok()?;
            if !super::runtime::PORTED.contains(&engine) {
                return None;
            }
            let builtin = super::runtime::builtins().iter().find(|b| b.engine == engine)?;
            builtin.id.0[..8].try_into().ok()
        }
        _ => None,
    }
}

pub(super) fn guest_state(m: &dyn GuestMemory, state: Option<State>) -> Option<State> {
    let state = state?;
    (guest_prefix(m)? == state.prefix).then_some(state)
}

/// Trait answers for a current holder or a stale native Juggernaut role.
pub(super) fn answer(m: &dyn GuestMemory, state: State, player: u32, trait_id: u32) -> Option<bool> {
    let slot = (player & 0xffff) as u16;
    if slot >= 16 || !(1..=3).contains(&trait_id) {
        return None;
    }
    let holder = state.mask & (1 << slot) != 0;
    // The retail role setter runs at the tick boundary and is replicated
    // independently. For a former holder still in that mask, answer from
    // the normal variant flags, just as Juggernaut's base query would.
    if !holder && (super::guest::engine_index(m).ok()?? != JUGGERNAUT
        || role_mask(m).ok()?? & (1 << slot) == 0) {
        return None;
    }
    if ff::player_datum(m, slot as u8).ok()?? != player {
        return None;
    }
    if holder {
        return Some(state.traits & (1 << (trait_id - 1)) != 0);
    }
    let g = m.u32(points::GAME_GLOBALS.base).ok().filter(|&g| g != 0)?;
    let flags = m.u32(g + super::guest::VARIANT + super::guest::VARIANT_FLAGS).ok()?;
    let bit = [2, 12, 13][trait_id as usize - 1];
    Some(flags & (1 << bit) != 0)
}

pub fn register(b: &mut hooks::Builder, shared: &Arc<Shared>) {
    // Overriding the base alone misses an objective holder: each engine's
    // own query can return before its fallback to the base.
    for query in [
        &points::ENGINE_TRAIT_QUERY,
        &points::CTF_TRAIT_QUERY,
        &points::ODDBALL_TRAIT_QUERY,
        &points::KING_TRAIT_QUERY,
        &points::JUGGERNAUT_TRAIT_QUERY,
        &points::TERRITORIES_TRAIT_QUERY,
    ] {
        let s = Arc::clone(shared);
        b.raw().before(query, "rules: the role holder's player trait", move |c| {
            let (Some(player), Some(trait_id)) = (c.arg_u32("player"), c.arg_u32("trait")) else {
                return Ok(hooks::Flow::Continue);
            };
            let value = {
                let rt = Arc::clone(c.ctx.runtime());
                let mem = rt.memory();
                let m: &dyn GuestMemory = &*mem;
                let remote = crate::coop_look::view::mode(m)
                    == Some(crate::firefight::runtime::WORLD_DIST_CLIENT);
                let state = if remote {
                    guest_state(m, crate::actor_net::roles::received())
                } else {
                    network_state(m, &s.lock())
                };
                state.and_then(|state| answer(m, state, player, trait_id))
            };
            let Some(value) = value else {
                return Ok(hooks::Flow::Continue);
            };
            c.cpu.eax = (c.cpu.eax & !0xff) | u32::from(value);
            c.ret(8).map(hooks::Flow::Return)
        });
    }
}
