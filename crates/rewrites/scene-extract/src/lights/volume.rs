//! Pose the light volume for the read-only constants ports. Shape input
//! comes from the definition or placement (0x000c19f0), and the light
//! record supplies its frame (0x000c1a80). The light pass builds the
//! same scratch record on its stack for the lights the guest draws.

use engine::memory::{GuestMemory, MemFault};
use engine::render::light::LightRecord;
use engine::scene_list_math::cone_slopes;
use frame_scene::LightVolume;

use super::tag;

const ACTIVE_SCENARIO: u32 = 0x004e_0350;
const SCENARIO_LIGHT: u32 = 8;
const INSTANCE_SCALE: u32 = 0xc4;
const RECT_SLOPES: u32 = 0x0046_8774;
const ONE: u32 = 0x0045_dbc0;
const HALF: u32 = 0x0045_dbbc;
const EPSILON: u32 = 0x0045_dbdc;
const WORDS: usize = 31;

/// Address used exclusively inside the constants reader.
pub(super) const ADDRESS: u32 = 0xffff_ff00;

pub(super) struct Volume([u32; WORDS]);

impl Volume {
    fn float(&mut self, offset: usize, value: f32) {
        self.0[offset / 4] = value.to_bits();
    }

    fn vec3(&mut self, offset: usize, value: [f32; 3]) {
        self.0[offset / 4..][..3].copy_from_slice(&value.map(f32::to_bits));
    }

    pub(super) fn scene(&self) -> LightVolume {
        let read = |offset: usize| f32::from_bits(self.0[offset / 4]);
        let vec3 = |offset| [read(offset), read(offset + 4), read(offset + 8)];
        let origin = vec3(0x2c);
        let kind = self.0[0] as u16;
        if kind == 0 {
            LightVolume::Sphere { origin, radius: read(0x44) }
        } else {
            LightVolume::Projected {
                kind, origin, axis: vec3(0x3c),
                near: read(0x60), far: read(0x64), far_extent: [read(0x68), read(0x6c)],
            }
        }
    }

    pub(super) fn at<'a>(&'a self, m: &'a dyn GuestMemory) -> impl GuestMemory + 'a {
        VolumeMemory { words: &self.0, guest: m }
    }
}

struct VolumeMemory<'a> {
    words: &'a [u32; WORDS],
    guest: &'a dyn GuestMemory,
}

impl GuestMemory for VolumeMemory<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        if let Some(offset) = addr.checked_sub(ADDRESS) {
            let offset = offset as usize;
            if offset + buf.len() <= WORDS * 4 {
                for (i, byte) in buf.iter_mut().enumerate() {
                    let n = offset + i;
                    *byte = self.words[n / 4].to_le_bytes()[n % 4];
                }
                return Ok(());
            }
        }
        self.guest.read(addr, buf)
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault { addr, len: data.len() as u32, write: true })
    }
}

fn placement(m: &dyn GuestMemory, record: u32) -> Option<Option<u32>> {
    let index = m.u32(record.checked_add(SCENARIO_LIGHT)?).ok()?;
    if index == u32::MAX {
        return Some(None);
    }
    let scenario = m.u32(ACTIVE_SCENARIO).ok()?;
    if scenario == 0 || index >= m.u32(scenario.checked_add(0xe8)?).ok()? {
        return None;
    }
    let rows = m.u32(scenario.checked_add(0xec)?).ok()?;
    let row = rows.checked_add(index.checked_mul(0x6c)?)?;
    Some((m.u8(row.checked_add(0x3e)?).ok()? & 1 != 0).then_some(row))
}

pub(super) fn read(
    m: &dyn GuestMemory,
    address: u32,
    record: &LightRecord,
    definition: u32,
    definition_kind: u16,
    gel: u32,
) -> Option<Volume> {
    let row = placement(m, address)?;
    let kind = match row {
        Some(row) => m.u16(row.checked_add(0x3c)?).ok()?,
        None => definition_kind,
    };
    if kind > 3 {
        return None;
    }

    let mut volume = Volume([0; WORDS]);
    volume.0[0] = u32::from(kind);
    // The guest sets byte +0x18 when the posed volume is ready.
    volume.0[0x18 / 4] = 1;
    let [fx, fy, fz] = record.forward;
    let [ux, uy, uz] = record.up;
    volume.vec3(0x1c, [uz * fy - fz * uy, fz * ux - uz * fx, uy * fx - fy * ux]);
    volume.vec3(0x2c, record.position);
    let scale = m.f32(address.checked_add(INSTANCE_SCALE)?).ok()?;
    let d = |off| m.f32(definition.checked_add(off)?).ok();
    if kind == 0 {
        let (near, far) = match row {
            Some(row) => {
                let near = m.f32(row.checked_add(0x68)?).ok()?;
                let end = d(0x1c)?;
                (near, if end == 0.0 { near } else { (end / d(0x18)?) * near })
            }
            None => (d(0x18)?, d(0x1c)?),
        };
        volume.float(4, near);
        volume.float(8, far);
        volume.float(0x3c, near * scale);
        volume.float(0x40, far * scale);
        volume.float(0x44, (near * scale).max(far * scale));
        return Some(volume);
    }

    let source = row.unwrap_or(definition);
    let start = if row.is_some() { 0x58 } else { 0x20 };
    let field = |n: u32| m.f32(source.checked_add(start + 4 * n)?).ok();
    let near_extent = if kind == 3 { 0.0 } else { field(0)? };
    let aspect = field(1)?;
    let angle = if kind == 1 { 0.0 } else { field(2)? };
    let near = field(3)?;
    let far = field(4)?;
    for (i, value) in [near_extent, aspect, angle, near, far].into_iter().enumerate() {
        volume.float(4 + 4 * i, value);
    }
    let aspect = if gel == u32::MAX {
        aspect
    } else {
        let bitmap = tag(m, gel, b"bitm")?;
        let image = m.u32(bitmap.checked_add(0x48)?).ok()?;
        let width = m.u16(image.checked_add(4)?).ok()? as i16;
        let height = m.u16(image.checked_add(6)?).ok()? as i16;
        (f32::from(height) / f32::from(width)) * aspect
    };
    let half = m.f32(HALF).ok()?;
    let [sx, sy] = if kind == 1 {
        let slopes = m.u32(RECT_SLOPES).ok()?;
        [m.f32(slopes).ok()?, m.f32(slopes.checked_add(4)?).ok()?]
    } else {
        let (sx, sy) = cone_slopes(angle, half, aspect)?;
        [sx, sy]
    };
    let epsilon = m.f32(EPSILON).ok()?;
    let axis = std::array::from_fn::<_, 3, _>(|i| record.target[i] - record.position[i]);
    let depth = (fz * axis[2] + fy * axis[1]) + fx * axis[0];
    let (axis, depth) = if depth.abs() < epsilon || depth <= 0.0 {
        (record.forward, m.f32(ONE).ok()?)
    } else {
        (axis, depth)
    };
    let reciprocal = m.f32(ONE).ok()? / depth;
    let axis = axis.map(|v| v * reciprocal);
    let far_depth = far * scale;
    let height = aspect * near_extent;
    let far_extent = [sx * far_depth + near_extent, sy * far_depth + height];
    if ![aspect, near, far_depth, far_extent[0], far_extent[1]].into_iter().all(f32::is_finite) {
        return None;
    }
    volume.vec3(0x3c, axis);
    volume.float(0x48, depth);
    volume.float(0x4c, sx);
    volume.float(0x50, sy);
    volume.float(0x54, aspect);
    volume.float(0x58, near_extent);
    volume.float(0x5c, height);
    volume.float(0x60, near * scale);
    volume.float(0x64, far_depth);
    volume.float(0x68, far_extent[0]);
    volume.float(0x6c, far_extent[1]);
    volume.vec3(0x70, std::array::from_fn(|i| axis[i] * far_depth + record.position[i]));

    if kind == 2 && sx > epsilon {
        let apex = near_extent / sx * half;
        volume.float(0x28, apex);
        volume.vec3(0x2c, std::array::from_fn(|i| axis[i] * -apex + record.position[i]));
    }
    Some(volume)
}
