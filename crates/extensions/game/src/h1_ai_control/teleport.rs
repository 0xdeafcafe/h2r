//! The level's authored starting locations share a reservation pool across
//! teleport calls. Save it in the HaloScript globals of the converted map.

use std::collections::BTreeMap;

use guest_layout::memory::{GuestMemory, MemFault};
use hooks::{Builder, Flow, points};

use super::state;

const GAMEPLAY_RANDOM_SEED: u32 = 0x004e_7408;
const WIDTH: u32 = 2;

type R<T> = Result<T, MemFault>;

#[derive(Clone, Copy)]
pub(super) struct Pool {
    base: u32,
    count: usize,
}

pub(super) fn pools(m: &dyn GuestMemory, base: u32) -> R<BTreeMap<u32, Pool>> {
    let mut pools = BTreeMap::new();
    let Some(at) = state::registry(m, base, 1)? else {
        return Ok(pools);
    };
    let rows = m.u32(at)?;
    if rows > u32::from(hs::h1_ai_slots::timers::SQUADS) {
        return Ok(pools);
    }
    let mut next = base + 1;
    for _ in 0..rows {
        let Some(header) = state::registry(m, next, 1)? else {
            return Ok(BTreeMap::new());
        };
        let row = m.u32(header)?;
        let count = usize::from(row as u16);
        if count == 0 {
            return Ok(BTreeMap::new());
        }
        let size = WIDTH * count.div_ceil(32) as u32;
        let Some(base) = state::registry(m, next + 1, size)? else {
            return Ok(BTreeMap::new());
        };
        pools.insert(row >> 16, Pool { base, count });
        next += 1 + size;
    }
    Ok(pools)
}

pub(super) fn register_spawn_hook(builder: &mut Builder<'_>) {
    builder.raw().before(
        &points::AI_CHOOSE_SQUAD_START,
        "share Halo 1 spawn and teleport start reservations",
        |call| {
            if !super::is_converted_campaign(call) {
                return Ok(Flow::Continue);
            }
            let (Some(squad), Some(available), Some(excluded)) = (
                call.arg_u32("squad").map(|squad| u32::from(squad as u16)),
                call.arg_u32("available"),
                call.arg_u32("excluded"),
            ) else {
                return Ok(Flow::Continue);
            };
            let result = {
                let mut guest = call.ctx.runtime().memory();
                reserve_spawn(&mut *guest, squad, available, excluded)
            };
            match result {
                Ok(Some(index)) => {
                    call.cpu.eax = index;
                    call.ret(8).map(Flow::Return)
                }
                _ => Ok(Flow::Continue),
            }
        },
    );
    builder.raw().before(
        &points::AI_SPAWN_AT_SQUAD_START,
        "reserve an explicitly selected Halo 1 starting location",
        |call| {
            if !super::is_converted_campaign(call) {
                return Ok(Flow::Continue);
            }
            if let Some(squad) = call.arg_u32("squad") {
                let mut guest = call.ctx.runtime().memory();
                let _ = reserve_explicit(
                    &mut *guest,
                    u32::from(squad as u16),
                    usize::from(call.cpu.ecx as u16),
                );
            }
            Ok(Flow::Continue)
        },
    ).after_feature("h1-conversations");
}

fn reserve_explicit(m: &mut dyn GuestMemory, squad: u32, index: usize) -> R<()> {
    if let Some(base) = state::teleport_base(m)?
        && let Some(pool) = pools(m, base)?.get(&squad).copied()
    {
        pool.consume(m, index)?;
    }
    Ok(())
}

fn reserve_spawn(
    m: &mut dyn GuestMemory,
    squad: u32,
    available: u32,
    excluded: u32,
) -> R<Option<u32>> {
    let Some(base) = state::teleport_base(m)? else {
        return Ok(None);
    };
    let Some(pool) = pools(m, base)?.get(&squad).copied() else {
        return Ok(None);
    };
    let Some(record) = super::actors::squad_record(m, squad)? else {
        return Ok(None);
    };
    let count = m.u32(record + 0x48)?;
    // The native selector consumes its temporary per-wave bit too.
    let Some(index) = pool.reserve_excluding(m, count, excluded)? else {
        return Ok(Some(super::NONE));
    };
    let at = available + 4 * (index / 32);
    let mask = m.u32(at)? & !(1 << (index % 32));
    m.set_u32(at, mask)?;
    Ok(Some(index))
}

#[derive(Clone, Copy)]
struct Word {
    preferred: u32,
    available: u32,
}

fn advance_random(seed: u32) -> u32 {
    seed.wrapping_mul(0x19660d).wrapping_add(0x3c6ef35f)
}

fn choose(words: &mut [Word], excluded: &[u32], count: usize, seed: u32) -> Option<usize> {
    let use_preferred = words
        .iter()
        .zip(excluded)
        .any(|(word, &exclude)| word.preferred & !exclude != 0);
    if !use_preferred
        && words
            .iter()
            .zip(excluded)
            .all(|(word, &exclude)| word.available & !exclude == 0)
    {
        for (index, word) in words.iter_mut().enumerate() {
            word.available = u32::MAX >> (32 - (count - 32 * index).min(32));
        }
    }
    let mask = |word: &Word, exclude: u32| {
        (if use_preferred {
            word.preferred
        } else {
            word.available
        }) & !exclude
    };
    let options: u32 = words
        .iter()
        .zip(excluded)
        .map(|(word, &exclude)| mask(word, exclude).count_ones())
        .sum();
    let mut rank = ((seed >> 16) * options) >> 16;
    for (index, (word, &exclude)) in words.iter_mut().zip(excluded).enumerate() {
        let mut bits = mask(word, exclude);
        let size = bits.count_ones();
        if rank >= size {
            rank -= size;
            continue;
        }
        for _ in 0..rank {
            bits &= bits - 1;
        }
        let bit = bits.trailing_zeros();
        word.preferred &= !(1 << bit);
        word.available &= !(1 << bit);
        return Some(index * 32 + bit as usize);
    }
    None
}

impl Pool {
    fn consume(self, m: &mut dyn GuestMemory, index: usize) -> R<()> {
        if index >= self.count {
            return Ok(());
        }
        let at = self.base + (index / 32) as u32 * WIDTH * state::STRIDE;
        let keep = !(1 << (index % 32));
        let preferred = m.u32(at)? & keep;
        let available = m.u32(at + state::STRIDE)? & keep;
        m.set_u32(at, preferred)?;
        m.set_u32(at + state::STRIDE, available)
    }

    pub(super) fn reserve(self, m: &mut dyn GuestMemory, native_starts: u32) -> R<Option<u32>> {
        self.reserve_excluding(m, native_starts, 0)
    }

    fn reserve_excluding(
        self,
        m: &mut dyn GuestMemory,
        native_starts: u32,
        exclusions: u32,
    ) -> R<Option<u32>> {
        let seed = m.u32(GAMEPLAY_RANDOM_SEED)?;
        if seed == 0 || self.count as u32 > native_starts {
            return Ok(None);
        }
        let mut words = Vec::with_capacity(self.count.div_ceil(32));
        let mut excluded = Vec::with_capacity(words.capacity());
        for index in 0..self.count.div_ceil(32) {
            let at = self.base + index as u32 * WIDTH * state::STRIDE;
            words.push(Word {
                preferred: m.u32(at)?,
                available: m.u32(at + state::STRIDE)?,
            });
            excluded.push(if exclusions == 0 {
                0
            } else {
                m.u32(exclusions + index as u32 * 4)?
            });
        }
        let random = advance_random(m.u32(seed)?);
        let Some(selected) = choose(&mut words, &excluded, self.count, random) else {
            return Ok(None);
        };
        for (index, word) in words.into_iter().enumerate() {
            let at = self.base + index as u32 * WIDTH * state::STRIDE;
            m.set_u32(at, word.preferred)?;
            m.set_u32(at + state::STRIDE, word.available)?;
        }
        m.set_u32(seed, random)?;
        Ok(Some(selected as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn required_starts_are_consumed_once_and_fallback_recycles_after_exhaustion() {
        let mut words = [Word {
            preferred: 0b1010,
            available: 0b1111,
        }];
        let selected: Vec<_> = (0..5)
            .map(|_| choose(&mut words, &[0], 4, 0).unwrap())
            .collect();
        assert_eq!(selected, [1, 3, 0, 2, 0]);
        assert_eq!(words[0].preferred, 0);
        assert_eq!(words[0].available, 0b1110);
    }

    #[test]
    fn spawn_exclusion_does_not_consume_the_preferred_start_for_teleport() {
        let mut words = [Word {
            preferred: 0b100,
            available: 0b111,
        }];
        assert_eq!(choose(&mut words, &[0b100], 3, 0), Some(0));
        assert_eq!(choose(&mut words, &[0], 3, 0), Some(2));
    }

    #[test]
    fn direct_spawn_reserves_its_start_without_advancing_random_selection() {
        let mut guest = FakeGuest::new();
        let base = 0x0080_0100;
        guest
            .put_u32(base, 0b11)
            .put_u32(base + state::STRIDE, 0b111);
        let pool = Pool { base, count: 3 };
        pool.consume(&mut guest, 0).unwrap();
        pool.consume(&mut guest, 0).unwrap();
        assert_eq!(guest.u32(base), Ok(0b10));
        assert_eq!(guest.u32(base + state::STRIDE), Ok(0b110));
    }

    #[test]
    fn repeated_calls_share_guest_reservations_and_the_gameplay_rng() {
        let mut guest = FakeGuest::new();
        let base = 0x0080_0100;
        let seed = 0x0080_0200;
        guest
            .put_u32(GAMEPLAY_RANDOM_SEED, seed)
            .put_u32(seed, 7)
            .put_u32(base, 4)
            .put_u32(base + state::STRIDE, 7);
        let pool = Pool { base, count: 3 };
        assert_eq!(pool.reserve(&mut guest, 3).unwrap(), Some(2));
        assert_ne!(pool.reserve(&mut guest, 3).unwrap(), Some(2));
        assert_eq!(guest.u32(seed).unwrap(), advance_random(advance_random(7)));
    }
}
