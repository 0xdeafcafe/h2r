//! Select the object's active collision permutations and walk their BSPs.
use super::transform;
use crate::instance::PlacementMatrix;
use crate::{Bail, GuestRead, Hit, Outcome, Segment, bsp::NONE};
use sim_core::det::Fp;
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
    pub matrix: PlacementMatrix,
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
        fp: &mut Fp,
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
                let matrix =
                    transform::matrix(g, self.matrices.wrapping_add(0x34 * u32::from(node)))?;
                let (local_start, local_delta) =
                    transform::local_ray_with(fp, matrix, one, start, delta);
                // Ordered tests: a NaN fraction is neither clamped.
                if fp.comiss(fraction, 0) == Some(Ordering::Less) {
                    fraction = 0;
                }
                let t1 = if fp.comiss(fraction, one) == Some(Ordering::Greater) {
                    one
                } else {
                    fraction
                };
                if !fp.exact() {
                    return Err(Bail::Float);
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Image;

    #[test]
    fn damage_selection_is_live_and_each_part_inherits_the_last_fraction() {
        const BASE: u32 = 0x0044_0000;
        let mut image = Image::new(BASE, 0x20000);
        let model = Model {
            hlmt: BASE + 0x100,
            coll: BASE + 0x200,
            matrices: BASE + 0x300,
            states: BASE + 0x400,
        };
        let regions = BASE + 0x500;
        let damage = BASE + 0x600;
        let coll_regions = BASE + 0x700;
        let permutations = BASE + 0x800;
        let parts = BASE + 0x900;
        for (a, v) in [
            (model.hlmt + 0x70, 1),
            (model.hlmt + 0x74, regions),
            (regions + 8, 2),
            (regions + 12, damage),
            (model.coll + 0x1c, 1),
            (model.coll + 0x20, coll_regions),
            (coll_regions + 4, 1),
            (coll_regions + 8, permutations),
            (permutations + 4, 2),
            (permutations + 8, parts),
        ] {
            image.word(a, v);
        }
        image.put(damage + 5, &[0xff]);
        image.put(damage + 13, &[0]);
        image.put(model.states, &[1]);
        image.float(UNIT, 1.);
        image.float(INITIAL_FRACTION, 2.);
        image.put(parts + 0x44, &[1]);
        for node in 0..2 {
            let matrix = model.matrices + node * 0x34;
            image.float(matrix, 2.);
            for offset in [4, 20, 36] {
                image.float(matrix + offset, 1.);
            }
            image.float(matrix + 40, node as f32);
        }
        let start = [4.0, 1.0, 3.0].map(f32::to_bits);
        let delta = [1.0, 2.0, 3.0].map(f32::to_bits);
        let hit = Hit {
            fraction: 0.25f32.to_bits(),
            plane_addr: BASE + 0x2000,
            leaf: 5,
            surface: 2,
            plane: 1,
            flags: 0,
            breakable: 0,
            material: 11,
        };
        let mut visits = 0;
        let winner = model
            .ray(&mut Fp::new(), &mut image, start, delta, 9, &mut |_, part| {
                assert_eq!(part.bsp, parts + 4 + visits * 0x44);
                assert_eq!(part.query.flags, 9);
                assert_eq!(part.query.delta, [0.5, 1., 1.5].map(f32::to_bits));
                assert_eq!(
                    part.query.start,
                    [(4. - visits as f32) * 0.5, 0.5, 1.5].map(f32::to_bits)
                );
                let first = visits == 0;
                assert_eq!(
                    part.query.fraction,
                    if first { 2.0f32 } else { 0.25 }.to_bits()
                );
                assert_eq!(part.t1, if first { 1.0f32 } else { 0.25 }.to_bits());
                visits += 1;
                Ok(Outcome {
                    hit: first.then_some(hit),
                    ..Outcome::default()
                })
            })
            .unwrap()
            .unwrap();
        assert_eq!(visits, 2);
        assert_eq!((winner.key, winner.hit), (0, hit));
        image.put(model.states, &[0]);
        assert!(
            model
                .ray(&mut Fp::new(), &mut image, start, delta, 9, &mut |_, _| panic!(
                    "inactive permutation"
                ))
                .unwrap()
                .is_none()
        );

        image.put(model.states, &[1]);
        image.put(regions + 4, &[16]);
        assert!(matches!(
            model.ray(&mut Fp::new(), &mut image, start, delta, 9, &mut |_, _| panic!(
                "invalid region"
            )),
            Err(Bail::Data)
        ));
    }
}
