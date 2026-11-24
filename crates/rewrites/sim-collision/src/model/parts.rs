//! Select the object's active collision permutations and walk their BSPs.
use super::transform::Matrix;
use crate::{Bail, GuestRead, Hit, Outcome, Segment, bsp::NONE};
use sim_core::det::{self, Fp};
use std::cmp::Ordering;

const UNIT: u32 = 0x0045_dbc0;
const INITIAL_FRACTION: u32 = 0x0044_aed4;

pub(super) struct Model {
    pub(super) hlmt: u32,
    pub(super) coll: u32,
    pub(super) states: u32,
    pub(super) matrices: u32,
}

pub struct PartRay {
    pub bsp: u32,
    pub query: Segment,
    pub t1: u32,
}

pub(super) struct PartHit {
    pub key: u32,
    pub bsp: u32,
    pub matrix: Matrix,
    pub hit: Hit,
}

impl Model {
    /// 1efbd0 builds this list each time because damage and permutations
    /// can change between successive rays against the same object.
    fn permutations(&self, g: &impl GuestRead) -> Result<[i8; 16], Bail> {
        let mut selected = [-1i8; 16];
        if self.states == 0 {
            return Ok([0; 16]);
        }
        if self.hlmt == 0 {
            let mut raw = [0; 16];
            g.read(self.states, &mut raw)?;
            return Ok(raw.map(|v| v as i8));
        }
        let n = (g.u32(self.hlmt.wrapping_add(0x70))? as i32).clamp(0, 16);
        if n == 0 {
            return Ok(selected);
        }
        let regions = g.u32(self.hlmt.wrapping_add(0x74))?;
        for i in 0..n as u32 {
            let region = regions.wrapping_add(i * 16);
            let slot = g.u8(region.wrapping_add(4))? as i8;
            let state = g.u8(self.states.wrapping_add(i))? as i8;
            if slot == -1 || state < 0 || i32::from(state) >= g.u32(region.wrapping_add(8))? as i32
            {
                continue;
            }
            let perms = g.u32(region.wrapping_add(12))?;
            let perm = g.u8(perms.wrapping_add(state as u32 * 8 + 5))? as i8;
            if perm != -1 {
                *selected.get_mut(slot as usize).ok_or(Bail::Data)? = perm;
            }
        }
        Ok(selected)
    }

    /// 1eff40 resets the result fraction once, then passes each part the
    /// previous part's best fraction. A hit overwrites the part identifier.
    pub(super) fn ray<G: GuestRead>(
        &self,
        g: &mut G,
        start: [u32; 3],
        delta: [u32; 3],
        flags: u32,
        visit: &mut impl FnMut(&mut G, PartRay) -> Result<Outcome, Bail>,
    ) -> Result<Option<PartHit>, Bail> {
        let mut fraction = g.u32(INITIAL_FRACTION)?;
        let selected = self.permutations(g)?;
        let count = g.u32(self.coll.wrapping_add(0x1c))? as i32;
        if count > selected.len() as i32 {
            return Err(Bail::Data);
        }
        let mut winner = None;
        for (r, &p) in selected.iter().take(count.max(0) as usize).enumerate() {
            if p < 0 {
                continue;
            }
            let regions = g.u32(self.coll.wrapping_add(0x20))?;
            let region = regions.wrapping_add(12 * r as u32);
            if i32::from(p) >= g.u32(region.wrapping_add(4))? as i32 {
                continue;
            }
            let perms = g.u32(region.wrapping_add(8))?;
            let perm = perms.wrapping_add(20 * p as u32);
            let count = g.u32(perm.wrapping_add(4))? as i32;
            if count > 256 {
                return Err(Bail::Data);
            }
            if count <= 0 {
                continue;
            }
            let parts = g.u32(perm.wrapping_add(8))?;
            let one = g.u32(UNIT)?;
            for i in 0..count as u32 {
                let part = parts.wrapping_add(i * 0x44);
                let node = g.u8(part)?;
                let matrix = Matrix::read(g, self.matrices.wrapping_add(0x34 * u32::from(node)))?;
                let mut fp = Fp::new();
                let inv = matrix.inverse(&mut fp, one);
                let local_start = inv.local(&mut fp, start, one, true);
                let local_delta = inv.local(&mut fp, delta, one, false);
                fp.check(fraction);
                if !fp.exact() {
                    return Err(Bail::Float);
                }
                if det::compare(fraction, 0) == Ordering::Less {
                    fraction = 0;
                }
                let t1 = if det::compare(fraction, one) == Ordering::Greater {
                    one
                } else {
                    fraction
                };
                let outcome = visit(
                    g,
                    PartRay {
                        bsp: part.wrapping_add(4),
                        query: Segment {
                            flags,
                            breakable_count: 0,
                            mask: 0,
                            start: local_start,
                            delta: local_delta,
                            fraction,
                            listed: 0,
                            prev_leaf: NONE,
                            prev_kind: 0,
                            plane: NONE,
                        },
                        t1,
                    },
                )?;
                if let Some(hit) = outcome.hit {
                    fraction = hit.fraction;
                    let key = u32::from_le_bytes([node, i as u8, p as u8, r as u8]);
                    winner = Some(PartHit {
                        key,
                        bsp: part.wrapping_add(4),
                        matrix,
                        hit,
                    });
                }
            }
        }
        Ok(winner)
    }
}
