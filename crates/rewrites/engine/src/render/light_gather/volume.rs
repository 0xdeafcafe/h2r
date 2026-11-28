//! The posed light volume: 31 words a caller keeps on its stack.
//! `0x000c19f0` zeroes them and copies the shape from the definition or
//! its placement (`0x000c1930`, `0x000c1980`); `0x000c1a80` poses it with
//! the record's frame and scale. `0x000c17f0` gates both on the light's
//! options and answers whether the volume has extent.
//!
//! Every word is written (the zeroing is `rep stosd`), so [`Posed`] is
//! the whole record.

use super::guest::{NONE, TAGS, at, lights, ptr, tag};
use super::{Refusal, Result, Writes};
use guest_layout::memory::GuestMemory;
use crate::render::light::{LightRecord, RECORD_SIZE};
use crate::scene_list_math::cone_slopes;

/// The volume's bytes.
pub const BYTES: usize = 0x7c;
pub const WORDS: usize = BYTES / 4;

const ACTIVE_SCENARIO: u32 = 0x004e_0350;
const SCENARIO_LIGHT: u32 = 8;
const INSTANCE_SCALE: u32 = 0xc4;
const RECT_SLOPES: u32 = 0x0046_8774;
const ONE: u32 = 0x0045_dbc0;
const HALF: u32 = 0x0045_dbbc;
const EPSILON: u32 = 0x0045_dbdc;
const ZERO: u32 = 0x0045_dbd8;
const LIM_SLOPE: u32 = 0x0045_e0b0;
const LIGHT_OPTIONS: u32 = 0x0051_07e8;
/// The multiplayer engine globals (`points::GAME_ENGINE`); `+0xc14` the
/// engine index, `+0` a flags byte.
const GAME_ENGINE: u32 = 0x004e_9ae8;
/// The engine table (`points::ENGINE_TABLE`): one engine object per index.
const ENGINE_TABLE: u32 = 0x0055_e4d0;

/// The 31 words of a posed volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Posed(pub [u32; WORDS]);

impl Posed {
    fn float(&mut self, offset: usize, value: f32) {
        self.0[offset / 4] = value.to_bits();
    }

    fn vec3(&mut self, offset: usize, value: [f32; 3]) {
        self.0[offset / 4..][..3].copy_from_slice(&value.map(f32::to_bits));
    }

    pub fn word(&self, offset: usize) -> u32 {
        self.0[offset / 4]
    }

    pub fn f32(&self, offset: usize) -> f32 {
        f32::from_bits(self.word(offset))
    }

    /// The shape code at `+0` (0 a sphere, 1 to 3 projected).
    pub fn kind(&self) -> u32 {
        self.0[0]
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.0.iter().flat_map(|w| w.to_le_bytes()).collect()
    }
}

/// The light's scenario placement when it overrides the shape:
/// `Some(None)` for none, `None` for one the port cannot read.
fn placement(m: &dyn GuestMemory, record: u32) -> Option<Option<u32>> {
    let index = m.u32(record.checked_add(SCENARIO_LIGHT)?).ok()?;
    if index == NONE {
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

/// `0x000c19f0` and `0x000c1a80` for the light record at `address`:
/// `definition` its `ligh` data, `definition_kind` the shape code at
/// `+4`, `gel` the gel bitmap datum at `+0x7c`. `None` for a shape code
/// past 3, a placement or a gel it cannot read, or a projected volume
/// whose extent is not finite.
pub fn pose(
    m: &dyn GuestMemory,
    address: u32,
    record: &LightRecord,
    definition: u32,
    definition_kind: u16,
    gel: u32,
) -> Option<Posed> {
    let row = placement(m, address)?;
    let kind = match row {
        Some(row) => m.u16(row.checked_add(0x3c)?).ok()?,
        None => definition_kind,
    };
    if kind > 3 {
        return None;
    }

    let mut volume = Posed([0; WORDS]);
    volume.0[0] = u32::from(kind);
    // 0x000c1d7c: the byte at +0x18 marks the volume posed.
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
                // 0x000c19ad: ucomiss against the zero word; unordered divides.
                (near, if end == m.f32(ZERO).ok()? { near } else { (end / d(0x18)?) * near })
            }
            None => (d(0x18)?, d(0x1c)?),
        };
        volume.float(4, near);
        volume.float(8, far);
        let (a, b) = (near * scale, far * scale);
        volume.float(0x3c, a);
        volume.float(0x40, b);
        // 0x000c1aad: comiss, jbe keeps the second (and a NaN).
        volume.float(0x44, if a > b { a } else { b });
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
    let aspect = if gel == NONE {
        aspect
    } else {
        let tags = m.u32(TAGS).ok()?;
        let bitmap = tag(m, tags, gel).ok()?;
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
    let (axis, depth) = if depth.abs() < epsilon || depth <= 0.0 { (record.forward, m.f32(ONE).ok()?) } else { (axis, depth) };
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
        // 0x000c1d40: xorps then subss, so the factor is 0 - apex.
        let back = 0.0 - apex;
        volume.vec3(0x2c, std::array::from_fn(|i| axis[i] * back + record.position[i]));
    }
    Some(volume)
}

/// What `0x000c17f0` does: the volume it writes at its EAX, when its
/// gates pass, and its AL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeCall {
    pub posed: Option<Posed>,
    /// Whether the volume has extent (the caller in `0x00031c80` ignores it).
    pub al: bool,
    pub writes: Writes,
}

/// `0x000c17f0`: EAX `out`, EDX the light datum, stack 0 `bsp_check` (its
/// low byte: test the game engine's light flag). The volume is posed when
/// the light options allow lights, a running game engine's flag (`+0`
/// bit 1) does not hide it,
/// and the record's flag bit 1 is set.
pub fn light_volume(m: &dyn GuestMemory, out: u32, datum: u32, bsp_check: u32) -> Result<VolumeCall> {
    let light = lights(m)?.slot(m, datum)?;
    let tags = ptr(m, TAGS)?;
    let definition = tag(m, tags, m.u32(at(light, 4)?)?)?;
    let flags = m.u32(definition)?;
    let mut allowed = true;
    if flags & 0x100 == 0 && bsp_check as u8 != 0 {
        let engine = m.u32(GAME_ENGINE)?;
        let index = m.u32(at(engine, 0xc14)?)?;
        if index >= 32 {
            return Err(Refusal::Layout);
        }
        if m.u32(at(ENGINE_TABLE, index * 4)?)? != 0 {
            allowed = m.u8(engine)? & 2 == 0;
        }
    }
    let options = m.u32(LIGHT_OPTIONS)?;
    if m.u8(at(options, 8)?)? == 0 || !allowed || m.u8(at(light, 2)?)? & 2 == 0 {
        return Ok(VolumeCall { posed: None, al: false, writes: Writes::new() });
    }
    let mut raw = [0u8; RECORD_SIZE];
    m.read(light, &mut raw)?;
    let record = LightRecord::parse(&raw);
    let kind = m.u16(at(definition, 4)?)?;
    let gel = m.u32(at(definition, 0x7c)?)?;
    let posed = pose(m, light, &record, definition, kind, gel).ok_or(Refusal::Unsupported)?;
    let epsilon = m.f32(EPSILON)?;
    let al = if posed.kind() as u16 == 0 {
        posed.f32(0x44) > epsilon
    } else {
        let [w, h] = [posed.f32(0x68), posed.f32(0x6c)];
        posed.f32(0x64) > epsilon && w * w + h * h > m.f32(LIM_SLOPE)?
    };
    let mut writes = Writes::new();
    writes.array(out, posed.bytes());
    Ok(VolumeCall { posed: Some(posed), al, writes })
}
