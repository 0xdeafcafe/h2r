//! Structure visibility lists shared by the camera and the dynamic lights.
//! Render-record publication follows this list in the original caller.

use super::objects::{
    ACTIVE_BSP, Clip, ClusterMasks, LIGHT_VIS, List, MAIN_VIS, Row, Sphere, VIEW_KIND, VIS_FLAGS,
    cluster_uses_sky, plane_distance, wholly_inside,
};
use super::*;

const CLUSTER_LIMIT: u32 = 512;
const PLACEMENT_LIMIT: u32 = 0x800;
const PLACEMENT_EPOCH: u32 = 0x004e_7c1c;
const PLACEMENT_STAMPS: u32 = 0x004e_7c20;
const HALF: u32 = 0x0045_dbbc;

pub(super) fn gather(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    let vis = stack(m, cpu, 4);
    let stage = if vis == Ok(LIGHT_VIS) {
        SceneList::LightGeometryGather
    } else {
        SceneList::ViewGeometry
    };
    (stage, vis.and_then(|vis| build(m, cpu, vis)))
}

struct Plane {
    normal: [f32; 3],
    offset: f32,
}

impl Plane {
    fn flags(&self, centre: [f32; 3], radius: f64) -> u16 {
        let d = plane_distance(self.normal, self.offset, centre);
        if radius < f64::from(d.abs()) {
            (u16::from(d < 0.0) << 15) | (u16::from(d > 0.0) << 14)
        } else {
            0xc000
        }
    }
}

fn build(m: &dyn GuestMemory, cpu: &CpuAbi, vis: u32) -> Result<Output> {
    fp_supported(cpu)?;
    if cpu.x87.control != 0x027f {
        return Err(Refusal::Unsupported);
    }
    let kind = m.u32(at(vis, VIEW_KIND)?)?;
    if !matches!((vis, kind), (MAIN_VIS, 0) | (LIGHT_VIS, 1 | 2)) {
        return Err(Refusal::Unsupported);
    }
    let view_flags = m.u32(at(vis, VIS_FLAGS)?)?;
    let mut out = Output::default();
    if view_flags & 0x20 != 0 {
        return Ok(out);
    }
    let clusters = ptr(m, vis)?;
    let count = m.u16(at(clusters, 0xa6c)?)?;
    if count == 0 {
        return Ok(out);
    }
    if u32::from(count) > CLUSTER_LIMIT {
        return Err(Refusal::Layout);
    }
    let bsp = ptr(m, ACTIVE_BSP)?;
    let sections = ptr(m, at(bsp, 0xa0)?)?;
    let section_count = m.u32(at(bsp, 0x9c)?)?;
    let cluster_list = List::read(m, ptr(m, at(vis, 0xc)?)?)?;
    let placement_list = List::read(m, ptr(m, at(vis, 0x18)?)?)?;
    if cluster_list.count != 0 || placement_list.count != 0 {
        return Err(Refusal::Unsupported);
    }
    let prior = m.u32(at(vis, 0x2ad0)?)?;
    let prior_handles = |column| -> Result<Option<Vec<u32>>> {
        if prior == 0 || view_flags & 1 == 0 {
            return Ok(None);
        }
        let prior = List::read(m, ptr(m, at(prior, column)?)?)?;
        let handles = if prior.count == 0 {
            Vec::new()
        } else {
            m.bytes(prior.column(m, 0xc)?, prior.count * 4)?
                .chunks_exact(4)
                .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                .collect()
        };
        Ok(Some(handles))
    };
    let prior_clusters = prior_handles(0xc)?;
    let prior_placements = prior_handles(0x18)?;
    let plane = if view_flags & 0x200 != 0 {
        Some(Plane {
            normal: m.vec3(at(vis, 0x2a88)?)?,
            offset: m.f32(at(vis, 0x2a94)?)?,
        })
    } else {
        None
    };
    let clip = Clip::read(m, vis)?;
    let epoch = m.u32(PLACEMENT_EPOCH)?;
    let mut cluster_rows = Vec::new();
    let mut placement_rows: Vec<Row> = Vec::new();
    let mut current = HashMap::<u32, usize>::new();
    let mut masks = ClusterMasks::read(m, vis)?;
    let mut membership = [0u32; 16];
    let mut ordinals = [None; CLUSTER_LIMIT as usize];
    for (i, word) in membership.iter_mut().enumerate() {
        *word = m.u32(at(vis, 0x21c + i as u32 * 4)?)?;
    }

    for ordinal in 0..u32::from(count) {
        let cluster = u32::from(m.u16(at(clusters, 0xa6e + ordinal * 0x1a)?)?);
        if cluster >= CLUSTER_LIMIT || cluster >= section_count {
            return Err(Refusal::Layout);
        }
        membership[cluster as usize / 32] |= 1 << (cluster % 32);
        ordinals[cluster as usize] = Some(ordinal as u8);
        let section = at(sections, cluster * 0xb0)?;
        let sky = cluster_uses_sky(m, cluster)?;
        let sky_flag = u16::from(sky) << 10;
        if m.u16(at(section, 0x24)?)? != 0 {
            if prior_clusters
                .as_ref()
                .is_some_and(|prior| !prior.contains(&cluster))
            {
                // The jump bypasses placements too, but keeps the cluster
                // presence bit and ordinal written above.
                continue;
            }
            if cluster_list.has_room(cluster_rows.len()) {
                let mut row = Row::plain(cluster);
                let sides = match &plane {
                    Some(plane) => {
                        let (centre, radius) = cluster_sphere(m, section)?;
                        plane.flags(centre, radius)
                    }
                    None => 0xc000,
                };
                row.flags = sides | sky_flag | 0x3000;
                cluster_rows.push(row);
            }
        }
        let placements = m.i32(at(section, 0x98)?)?;
        if placements <= 0 {
            continue;
        }
        if placements as u32 > PLACEMENT_LIMIT {
            return Err(Refusal::Layout);
        }
        let ids = ptr(m, at(section, 0x9c)?)?;
        let placement_defs = ptr(m, at(bsp, 0x13c)?)?;
        let placement_instances = ptr(m, at(bsp, 0x144)?)?;
        let def_count = m.u32(at(bsp, 0x138)?)?;
        let instance_count = m.u32(at(bsp, 0x140)?)?;
        for i in 0..placements as u32 {
            let id = u32::from(m.u16(at(ids, i * 2)?)?);
            if id >= instance_count || id >= PLACEMENT_LIMIT {
                return Err(Refusal::Layout);
            }
            let instance = at(placement_instances, id * 0x58)?;
            let index = u32::from(m.u16(at(instance, 0x34)?)?);
            if index >= def_count || index >= 0x8000 {
                return Err(Refusal::Layout);
            }
            let definition = at(placement_defs, index * 0xc8)?;
            let stamp = at(PLACEMENT_STAMPS, id * 4)?;
            if let Some(&i) = current.get(&id) {
                let row = &mut placement_rows[i];
                if row.flags & 0x800 == 0 {
                    masks.include(m, row.mask, ordinal)?;
                }
                row.flags |= sky_flag;
                continue;
            }
            if m.u32(stamp)? == epoch {
                return Err(Refusal::Layout);
            }
            if m.u16(at(definition, 0x24)?)? == 0 {
                continue;
            }

            let sphere = Sphere::read(m, at(instance, 0x3c)?)?;
            let hit = clip.admits(m, clusters, ordinal, sphere, false)?;
            if hit == 0 && view_flags & 0x80 == 0 {
                continue;
            }
            let inside = wholly_inside(m, clip, kind == 0, sphere, hit)?;
            let shares_prior = hit == 0
                || prior_placements
                    .as_ref()
                    .is_none_or(|prior| prior.contains(&id));
            let sides = plane.as_ref().map_or(0xc000, |plane| {
                plane.flags(sphere.centre, f64::from(sphere.radius))
            });
            let mut row = Row::plain(id);
            row.flags = sides
                | sky_flag
                | (u16::from(inside) << 11)
                | if shares_prior { 0x3000 } else { 0 };
            if !inside {
                row.mask = masks.allocate(m, ordinal)?;
            }
            if placement_list.has_room(placement_rows.len()) {
                current.insert(id, placement_rows.len());
                placement_rows.push(row);
                out.word(stamp, epoch);
            }
        }
    }

    for (cluster, ordinal) in ordinals.into_iter().enumerate() {
        if let Some(ordinal) = ordinal {
            out.byte(at(vis, 0x1c + cluster as u32)?, ordinal);
        }
    }
    out.array(
        at(vis, 0x21c)?,
        membership.into_iter().flat_map(u32::to_le_bytes).collect(),
    );
    cluster_list.publish(m, &mut out, &cluster_rows)?;
    placement_list.publish(m, &mut out, &placement_rows)?;
    masks.publish(&mut out)?;
    Ok(out)
}

fn cluster_sphere(m: &dyn GuestMemory, section: u32) -> Result<([f32; 3], f64)> {
    let half = m.f32(HALF)?;
    let mut centre = [0.0; 3];
    let mut extent = [0.0; 3];
    for i in 0..3 {
        let lower = m.f32(at(section, 0x54 + i as u32 * 8)?)?;
        let upper = m.f32(at(section, 0x58 + i as u32 * 8)?)?;
        centre[i] = (upper + lower) * half;
        extent[i] = f64::from(upper - centre[i]);
    }
    let [x, y, z] = extent;
    Ok((centre, ((z * z + y * y) + x * x).sqrt()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_geometry_uses_clip_side_flags_and_preserves_intersections() {
        let plane = Plane {
            normal: [1.0, 0.0, 0.0],
            offset: 0.0,
        };
        assert_eq!(plane.flags([2.0, 0.0, 0.0], 1.0), 0x4000);
        assert_eq!(plane.flags([-2.0, 0.0, 0.0], 1.0), 0x8000);
        assert_eq!(plane.flags([1.0, 0.0, 0.0], 1.0), 0xc000);
    }
}
