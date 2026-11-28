//! A generation for the bytes the BSP decoder reads. One physical page
//! subscription covers all cached BSPs and the projection constants. It
//! observes writes through any guest mapping of those bytes.
use super::{Read, layout};
use runtime::{Guest, WatchId};
use sim_collision::{
    Bail, Bsp,
    bsp::{HEADER_BYTES, block},
};
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

const OWNER: &str = "collision-bsp-sources";
// Bound the auxiliary address index even for a guest presenting fresh
// header addresses on every call. Resetting the generation only costs a decode.
const MAX_WATCHED_BSPS: usize = 1024;

struct Subscription {
    epoch: (u64, u64),
    id: WatchId,
    bsps: HashSet<u32>,
}

#[derive(Default)]
struct Sources {
    active: Option<Subscription>,
    generation: u64,
}

impl Sources {
    fn revision(&mut self, g: &mut Guest, addr: u32) -> Result<(u64, u64), Bail> {
        if g.has_trap() {
            // The lifted body attributes trapped reads to the guest instruction.
            return Err(Bail::Fault);
        }
        let epoch = g.write_watch_epoch();
        let reset = self.active.as_ref().is_none_or(|s| {
            s.epoch != epoch
                || g.has_dirty(s.id)
                || (s.bsps.len() >= MAX_WATCHED_BSPS && !s.bsps.contains(&addr))
        });
        if reset {
            // Watch IDs belong to this Guest. A previous Guest may still
            // exist; switching back retires its old subscription by owner.
            for (id, owner) in g.write_watch_owners() {
                if owner == OWNER {
                    g.unsubscribe_writes(id);
                }
            }
            self.generation += 1;
            self.active = Some(Subscription {
                epoch,
                id: g.subscribe_writes(OWNER),
                bsps: HashSet::new(),
            });
        }
        let s = self.active.as_mut().expect("source subscription installed");
        if !s.bsps.contains(&addr) {
            // A failed validation never enters the index, so an edited
            // header can recover even when no decode was admitted.
            let header = Bsp::read_header(&Read(&*g, (0, 0)), addr)?;
            Bsp::essential_bytes(&header)?;
            let blocks = block::STRIDE
                .into_iter()
                .enumerate()
                .map(|(i, stride)| (header[2 * i + 1], header[2 * i] * stride));
            let fixed = [
                (addr, HEADER_BYTES),
                (layout::ZERO, 4),
                (layout::PROJECTION, 36),
            ];
            for (start, len) in fixed.into_iter().chain(blocks) {
                if len != 0 {
                    // watch_virtual_read validates the whole non-wrapping
                    // virtual read and subscribes each backing physical span.
                    g.watch_virtual_read(s.id, start, len as usize)
                        .map_err(|_| Bail::Fault)?;
                }
            }
            s.bsps.insert(addr);
        }
        Ok((epoch.0, self.generation))
    }
}

const MOPP_OWNER: &str = "collision-mopp-sources";
const MAX_WATCHED_MOPPS: usize = 1024;

struct MoppSubscription {
    epoch: (u64, u64),
    id: WatchId,
    // Largest watched span at this address and the generation its size was checked in.
    codes: HashMap<u32, (u32, u64)>,
}

/// A generation for the bytes under the MOPP programs the box query
/// names. A program's reference count (`+0x24`) shares its page and moves
/// whenever the engine wraps the code in a shape, so a write here does not
/// drop the decoded trees: it moves the generation, and the world compares
/// each held program's bytes with the guest's again before using it.
#[derive(Default)]
struct MoppSources {
    active: Option<MoppSubscription>,
    generation: u64,
}

impl MoppSources {
    fn revision(&mut self, g: &mut Guest, code: u32) -> Result<u64, Bail> {
        if g.has_trap() {
            return Err(Bail::Fault);
        }
        let epoch = g.write_watch_epoch();
        let reset = self.active.as_ref().is_none_or(|s| {
            s.epoch != epoch
                || (s.codes.len() >= MAX_WATCHED_MOPPS && !s.codes.contains_key(&code))
        });
        if reset {
            for (id, owner) in g.write_watch_owners() {
                if owner == MOPP_OWNER {
                    g.unsubscribe_writes(id);
                }
            }
            self.generation += 1;
            self.active = Some(MoppSubscription {
                epoch,
                id: g.subscribe_writes(MOPP_OWNER),
                codes: HashMap::new(),
            });
        }
        let id = self.active.as_ref().expect("mopp subscription installed").id;
        if g.has_dirty(id) {
            g.take_dirty(id).map_err(|_| Bail::Fault)?;
            self.generation += 1;
        }
        let s = self.active.as_mut().expect("mopp subscription installed");
        let previous = s.codes.get(&code).copied();
        if previous.is_none_or(|(_, generation)| generation != self.generation) {
            let header = sim_collision::mopp::Header::read(&Read(&*g, (0, 0)), code)?;
            let watched = previous.map_or(0, |(size, _)| size);
            if header.size > watched {
                g.watch_virtual_read(s.id, code, header.size as usize)
                    .map_err(|_| Bail::Fault)?;
            }
            s.codes.insert(code, (watched.max(header.size), self.generation));
        }
        Ok(self.generation)
    }
}

/// The version entry F passes to `CollisionWorld::mopp`.
pub(super) fn mopp_revision(g: &mut Guest, code: u32) -> Result<u64, Bail> {
    static SOURCES: LazyLock<Mutex<MoppSources>> = LazyLock::new(Mutex::default);
    SOURCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .revision(g, code)
}

/// The Guest guard excludes a write between arming/checking these pages
/// and gathering or committing the call. The token travels with the
/// reader, so switching Guests on another executor cannot relabel it.
pub(super) fn revision(g: &mut Guest, addr: u32) -> Result<(u64, u64), Bail> {
    static SOURCES: LazyLock<Mutex<Sources>> = LazyLock::new(Mutex::default);
    SOURCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .revision(g, addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_collision::{location, point_to_leaf};
    use runtime::{CpuAbi, guest::Permissions};
    use sim_collision::{
        CollisionWorld, Entry,
        bsp::NONE,
        synth::{Builder, SOLID, leaf},
    };
    use sim_core::Epoch;

    #[test]
    fn growing_a_mopp_watches_its_new_tail_even_after_another_query() {
        use sim_collision::mopp::{HEADER_BYTES, SIZE};
        let code = 0x10000;
        let other = code + 0x2000;
        let alias = 0x20000;
        let mut g = Guest::new();
        g.map(code, 0x3000, 0, Permissions::RW).unwrap();
        g.map(alias, 0x1000, 0x1000, Permissions::RW).unwrap();
        for at in [code, other] {
            g.set_u32(at + SIZE, HEADER_BYTES + 1).unwrap();
        }
        let mut sources = MoppSources::default();
        let first = sources.revision(&mut g, code).unwrap();
        sources.revision(&mut g, other).unwrap();
        g.set_u32(code + SIZE, 0x1010).unwrap();
        // This lookup consumes the dirty notification for the changed header.
        let grown = sources.revision(&mut g, other).unwrap();
        assert_ne!(first, grown);
        assert_eq!(grown, sources.revision(&mut g, code).unwrap());
        g.set_u8(alias + 0x0f, 1).unwrap();
        assert_ne!(grown, sources.revision(&mut g, code).unwrap());
    }

    #[test]
    fn decode_constants_are_watched_and_incomplete_subscriptions_retry() {
        const BSP: u32 = 0x10_000;
        let mut g = Guest::new();
        g.map(BSP, HEADER_BYTES, 0, Permissions::RW).unwrap();
        g.map(layout::PROJECTION, 36, 0x1000, Permissions::RW)
            .unwrap();
        g.map(layout::ZERO, 4, 0x2000, Permissions::RW).unwrap();
        let mut sources = Sources::default();
        let first = sources.revision(&mut g, BSP).unwrap();
        assert_eq!(first, sources.revision(&mut g, BSP).unwrap());
        g.set_u32(layout::ZERO, (-0.0f32).to_bits()).unwrap();
        let second = sources.revision(&mut g, BSP).unwrap();
        assert_ne!(first, second);
        g.set_u16(layout::PROJECTION, 2).unwrap();
        let third = sources.revision(&mut g, BSP).unwrap();
        assert_ne!(second, third);

        // A temporarily unreadable node block must be retried even though
        // neither the virtual layout nor the header address changes.
        g.set_u32(BSP, 1).unwrap();
        g.set_u32(BSP + 4, 0x70_000).unwrap();
        assert_eq!(sources.revision(&mut g, BSP), Err(Bail::Fault));
        g.set_u32(BSP, 0).unwrap();
        assert_ne!(sources.revision(&mut g, BSP).unwrap(), third);
    }

    #[test]
    fn a_warm_point_walk_observes_aliased_edits_and_relocated_planes() {
        const BSP: u32 = 0x10_000;
        const ALIAS: u32 = 0x20_000;
        const QUERY: u32 = 0x30_000;
        const STACK: u32 = QUERY + 0x1800;
        const OTHER_PLANE: u32 = QUERY + 0x2200;
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let mut g = Guest::new();
        g.map(BSP, 0x1000, 0, Permissions::RW).unwrap();
        g.map(ALIAS, 0x1000, 0, Permissions::RW).unwrap();
        g.map(QUERY, 0x3000, 0x1000, Permissions::RW).unwrap();
        g.map(layout::PROJECTION, 36, 0x4000, Permissions::RW)
            .unwrap();
        g.map(layout::ZERO, 4, 0x5000, Permissions::RW).unwrap();
        g.map(layout::BSP_INDEX, 2, 0x6000, Permissions::RW)
            .unwrap();
        g.map(layout::STRUCTURE_BSP, 4, 0x7000, Permissions::RW)
            .unwrap();
        let mut b = Builder::default();
        b.planes.push([1.0, 0.0, 0.0, 0.0]);
        b.nodes.push((0, leaf(0), SOLID));
        b.leaves.push((0, 0, 0));
        b.write(BSP, |at, bytes| g.write_bytes(at, bytes, 1).unwrap());
        g.set_u32(QUERY, 1.0f32.to_bits()).unwrap();
        let cpu = CpuAbi {
            ecx: BSP,
            edx: QUERY,
            esp: STACK,
            esi: QUERY + 32,
            mxcsr: 0x1fa0,
            ..CpuAbi::default()
        };
        let point = |g: &mut Guest| {
            let mut c = cpu;
            assert!(point_to_leaf(&mut c, g, &world).unwrap().is_some());
            c.eax
        };
        assert_eq!(point(&mut g), NONE);
        let plane_field = BSP + (8 * block::PLANES + 4) as u32;
        let plane = g.u32(plane_field).unwrap();
        g.set_u32(ALIAS + (plane - BSP) + 12, 2.0f32.to_bits())
            .unwrap();
        assert_eq!(point(&mut g), 0);
        // The descriptor moves to a different physical page. Future writes
        // there must be watched as well as the edit that moved the pointer.
        g.set_u32(OTHER_PLANE, 1.0f32.to_bits()).unwrap();
        g.set_u32(plane_field, OTHER_PLANE).unwrap();
        assert_eq!(point(&mut g), NONE);
        g.set_u32(OTHER_PLANE + 12, 3.0f32.to_bits()).unwrap();
        assert_eq!(point(&mut g), 0);
        g.set_u32(OTHER_PLANE + 12, 0).unwrap();

        // B's active solid path elides one call to A. The inactive and
        // declined paths must leave A's histogram alone.
        g.set_u16(layout::BSP_INDEX, 3).unwrap();
        g.set_u32(layout::STRUCTURE_BSP, BSP).unwrap();
        let mut active = cpu;
        assert!(location(&mut active, &mut g, &world).unwrap().is_some());
        // Rounding down: an environment the walk does not run.
        let mut declined = CpuAbi {
            mxcsr: 0x3f80,
            ..cpu
        };
        assert!(location(&mut declined, &mut g, &world).unwrap().is_none());
        g.set_u16(layout::BSP_INDEX, u16::MAX).unwrap();
        let mut inactive = cpu;
        assert!(location(&mut inactive, &mut g, &world).unwrap().is_some());
        let stats = world.stats();
        let entry = &stats.entries[Entry::PointToLeaf as usize];
        assert_eq!(entry.mxcsr[&cpu.mxcsr], 1);
        assert_eq!(entry.x87_control[&cpu.x87.control], 1);
        assert_eq!(
            g.write_watch_owners()
                .iter()
                .filter(|(_, owner)| *owner == OWNER)
                .count(),
            1
        );
    }
}
