//! J visits each listed cluster once, in BSP order. Generations are read
//! live and increment with wraparound, so a pre-existing mark can skip a
//! cluster, instance or object even on the first visit.
use super::super::instance;
use super::*;
use sim_collision::{
    Fault,
    model::{ObjectQuery, trace_objects_with},
    objects::layout as reference,
    sphere::Sphere,
};
use std::collections::HashSet;

const INSTANCE_GENERATION: u32 = 0x004e_7c1c;
pub(super) const INSTANCE_ACTIVE: u32 = 0x004e_7c18;
pub(super) const INSTANCE_MARKS: u32 = 0x004e_7c20;
const BOUNDARY_MIN_SPEED: u32 = 0x0045_dbdc;

#[derive(Default)]
pub(super) struct Marks {
    pub(super) clusters: Vec<u32>,
    pub(super) instances: Vec<u32>,
    pub(super) objects: Vec<u32>,
    pub(super) cluster_epoch: Option<u32>,
    pub(super) instance_epoch: Option<u32>,
    pub(super) object_epoch: Option<u32>,
}

impl Marks {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn walk(
        &mut self,
        cpu: &CpuAbi,
        env: &mut Env,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
        structure: u32,
        index: i16,
        ray: Ray,
        leaves: &[u32],
        record: &mut Record,
        hit_bsp: &mut Option<u32>,
    ) -> Result<(), Bail> {
        let m = Read(&*g, (0, 0));
        let epoch = m.u32(layout::FLOOD_EPOCH)?.wrapping_add(1);
        self.cluster_epoch = Some(epoch);
        if ray.flags & 4 != 0 {
            self.instance_epoch = Some(m.u32(INSTANCE_GENERATION)?.wrapping_add(1));
        }
        if ray.flags & 8 != 0 {
            self.object_epoch = Some(m.u32(layout::VISIT_GENERATION)?.wrapping_add(1));
        }
        let mut seen_objects = HashSet::new();
        for &leaf in leaves {
            let m = Read(&*g, (0, 0));
            let cluster = location(&m, structure, index, leaf)?.cluster as i32 as u32;
            if cluster >= sim_collision::flood::MAX_CLUSTERS {
                return Err(Bail::Data);
            }
            if self.clusters.contains(&cluster)
                || m.u32(layout::CLUSTER_MARKS + 4 * cluster)? == epoch
            {
                continue;
            }
            self.clusters.push(cluster);
            if ray.flags & 2 != 0 && liquid(env, &m, structure, cluster, ray, record)? {
                *hit_bsp = None;
                if ray.flags & FIRST_HIT != 0 {
                    break;
                }
            }
            if let Some(epoch) = self.instance_epoch {
                let q = instance::Query {
                    cluster,
                    start: ray.start,
                    delta: ray.delta,
                    flags: ray.flags,
                    filter: ray.filter(),
                    limit: record.word(4),
                };
                let e = instance::predict_with_marks(cpu, g, world, q, epoch, &self.instances)?;
                env.absorb(e.mxcsr, e.x87);
                for (addr, _) in e.marks {
                    self.instances
                        .push(addr.checked_sub(INSTANCE_MARKS).ok_or(Bail::Data)? / 4);
                }
                if let Some(hit) = e.hit {
                    hit.writes(|offset, bytes| record.store(offset, bytes));
                    *hit_bsp = None;
                    if ray.flags & FIRST_HIT != 0 {
                        break;
                    }
                }
            }
            if let Some(generation) = self.object_epoch {
                self.objects(
                    env,
                    g,
                    world,
                    cluster,
                    generation,
                    &mut seen_objects,
                    ray,
                    record,
                    hit_bsp,
                )?;
                if record.word(0) != 0 && ray.flags & FIRST_HIT != 0 {
                    break;
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn objects(
        &mut self,
        env: &mut Env,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
        cluster: u32,
        generation: u32,
        seen: &mut HashSet<u32>,
        ray: Ray,
        record: &mut Record,
        hit_bsp: &mut Option<u32>,
    ) -> Result<(), Bail> {
        let m = Read(&*g, (0, 0));
        let heads = m.u32(layout::COLLIDEABLE_HEADS)?;
        let mut datum = m.u32(heads.wrapping_add(4 * cluster))?;
        let types = (ray.flags >> 4) & 0x1fff;
        let excluded = excluded_reference_flags(ray.flags);
        let mut references = HashSet::new();
        while datum != NONE {
            // The list uses indices for addressing regardless of datum salt.
            if !references.insert(datum as u16) {
                return Err(Bail::Data);
            }
            let m = Read(&*g, (0, 0));
            let array = m.u32(layout::COLLIDEABLE_ARRAY)?;
            let data = m.u32(array.wrapping_add(reference::DATA))?;
            let size = m.u32(array.wrapping_add(reference::ELEMENT_SIZE))?;
            let r = data.wrapping_add((datum & 0xffff).wrapping_mul(size));
            datum = m.u32(r.wrapping_add(reference::NEXT))?;
            let object = m.u32(r.wrapping_add(reference::HANDLE))?;
            if object == NONE {
                break;
            }
            let slot = object & 0xffff;
            if slot >= layout::OBJECT_MARK_SLOTS {
                return Err(Bail::Data);
            }
            if !seen.insert(slot) || m.u32(layout::OBJECT_MARKS + 4 * slot)? == generation {
                continue;
            }
            self.objects.push(slot);
            let bounds = r.wrapping_add(reference::BOUNDS);
            if types & 1u32.wrapping_shl(u32::from(m.u8(bounds)?)) == 0
                || m.u16(bounds.wrapping_add(2))? & excluded != 0
            {
                continue;
            }
            let sphere = Sphere {
                center: m.vec3(bounds.wrapping_add(4))?,
                radius: m.u32(bounds.wrapping_add(16))?,
            };
            let offset = sphere.offset_with(&mut env.fp, ray.start)?;
            if !offset.contains_start() && !offset.sweep_with(&mut env.fp, ray.delta)?.hit() {
                continue;
            }
            if ray.ignore.contains(&object) {
                continue;
            }
            let q = ObjectQuery {
                object,
                pretested: true,
                flags: ray.flags,
                bsp_flags: ray.filter(),
                start: ray.start,
                delta: ray.delta,
                ignore: ray.ignore,
                fraction: record.word(4),
            };
            // The parts' walks raise into `walks`; `env.fp` is the trace's.
            let (mxcsr, control) = (env.mxcsr(), env.x87.control());
            let walks = std::cell::Cell::new((0u32, 0u16));
            let hit = trace_objects_with(&mut env.fp, &mut ModelMemory(g), &q, |m, part| {
                let b = bsp(m.0, world, part.bsp)?;
                let out = b.vector_test_under(mxcsr, control, m, &part.query, 0, 0, part.t1)?;
                let (s, x) = walks.get();
                walks.set((s | out.mxcsr.unwrap_or(mxcsr), x | out.x87_flags));
                Ok(out)
            })?;
            let (s, x) = walks.get();
            env.absorb(s, x);
            if let Some(hit) = hit {
                hit.write_result(|offset, bytes| {
                    record.store(offset, bytes);
                    Ok::<(), Bail>(())
                })?;
                *hit_bsp = Some(hit.bsp);
                if ray.flags & FIRST_HIT != 0 {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Final values in live memory, including active bytes cleared on exit.
    pub(super) fn expected(
        &self,
        mut check: impl FnMut(u32, &[u8]) -> Result<(), Bail>,
    ) -> Result<(), Bail> {
        for (epoch, epoch_at, active_at, base, slots) in [
            (
                self.cluster_epoch,
                layout::FLOOD_EPOCH,
                layout::FLOOD_ACTIVE,
                layout::CLUSTER_MARKS,
                &self.clusters,
            ),
            (
                self.instance_epoch,
                INSTANCE_GENERATION,
                INSTANCE_ACTIVE,
                INSTANCE_MARKS,
                &self.instances,
            ),
            (
                self.object_epoch,
                layout::VISIT_GENERATION,
                layout::VISIT_ACTIVE,
                layout::OBJECT_MARKS,
                &self.objects,
            ),
        ] {
            if let Some(epoch) = epoch {
                check(epoch_at, &epoch.to_le_bytes())?;
                check(active_at, &[0])?;
                for &slot in slots {
                    check(
                        base.wrapping_add(slot.wrapping_mul(4)),
                        &epoch.to_le_bytes(),
                    )?;
                }
            }
        }
        Ok(())
    }
}

struct ModelMemory<'a, M>(&'a mut M);
impl<M: CollisionMemory> GuestRead for ModelMemory<'_, M> {
    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), Fault> {
        self.0.read_bytes(addr, dst, 1).map_err(|_| Fault { addr })
    }
}

/// Which reference flags reject this query (the mask returned by 16b500).
fn excluded_reference_flags(flags: u32) -> u16 {
    let mut mask = 1;
    for (src, dst) in [(19, 1), (18, 2), (30, 3), (20, 4), (22, 5), (21, 6)] {
        mask |= ((flags >> src) & 1) << dst;
    }
    mask as u16
}

pub(super) fn dot(fp: &mut Fp, n: [u32; 3], v: [u32; 3]) -> u32 {
    let z = fp.mul(n[2], v[2]);
    let y = fp.mul(n[1], v[1]);
    let zy = fp.add(z, y);
    let x = fp.mul(v[0], n[0]);
    fp.add(zy, x)
}

/// The liquid boundary test in J (`0x169ad1`): SSE for the distances and
/// the time, x87 for the speed (`fabs`, `fcomip`).
fn liquid(
    env: &mut Env,
    g: &impl GuestRead,
    structure: u32,
    cluster: u32,
    ray: Ray,
    record: &mut Record,
) -> Result<bool, Bail> {
    let clusters = g.u32(structure.wrapping_add(0xa0))?;
    let medium = g.u8(clusters.wrapping_add(0xb0 * cluster + 0x70))?;
    if medium == 0xff || medium & 0x80 == 0 {
        return Ok(false);
    }
    let i = u32::from(medium & 0x7f);
    let at = g.u32(structure.wrapping_add(0x68))?.wrapping_add(i * 0x18);
    let material = g.u16(at.wrapping_add(2))?;
    if material == u16::MAX {
        return Ok(false);
    }
    let plane = plane_at(g, at.wrapping_add(4))?;
    let n = [plane[0], plane[1], plane[2]];
    let fp = &mut env.fp;
    let s = dot(fp, n, ray.start);
    let s = fp.sub(s, plane[3]);
    let v = dot(fp, n, ray.delta);
    // `comiss x, 0; jbe` for each: a NaN is not above zero.
    let above = |fp: &mut Fp, x| fp.comiss(x, 0) == Some(Ordering::Greater);
    if above(&mut *fp, s) == above(&mut *fp, v) {
        return Ok(false);
    }
    // `fld v; fabs; fstp m32` then `fld s; fabs; fld m32; fcomip; jbe`.
    let x = &env.x87;
    let speed = x.store32(x.abs(x.load32(v)));
    let gap = x.abs(x.load32(s));
    if x.compare(x.load32(speed), gap, false) != Some(Ordering::Greater) {
        return Ok(false);
    }
    let fp = &mut env.fp;
    let min = g.u32(BOUNDARY_MIN_SPEED)?;
    // `comiss speed, min; jb`: below or unordered rejects.
    if !matches!(fp.comiss(speed, min), Some(Ordering::Greater | Ordering::Equal)) {
        return Ok(false);
    }
    let t = fp.div(s, v);
    let t = fp.sub(0, t);
    // `comiss fraction, t; jbe`: only an ordered earlier time hits.
    if fp.comiss(record.word(4), t) != Some(Ordering::Greater) {
        return Ok(false);
    }
    let flip = fp.comiss(0, s) == Some(Ordering::Greater);
    env.exact()?;
    record.put(0, 2);
    record.put(4, t);
    record.store(0x24, &material.to_le_bytes());
    record.put_plane(env, plane, flip)?;
    record.put(0x38, i);
    record.put(0x3c, NONE);
    record.put(0x40, NONE);
    Ok(true)
}
