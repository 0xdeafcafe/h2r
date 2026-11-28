//! World-space vehicle seat markers, resolved against the live node pose.

use super::{R, Vec3};
use crate::vehicle::Entry;
use guest_layout::handle::DatumHandle;
use guest_layout::memory::GuestMemory;
use guest_layout::tags::GroupTag;
use guest_layout::view::{Tags, objects};

/// Entry marker (0xc6fb0) and admission limits (0xc9040). See
/// h1objects/tests/vehicle_ground_disc.rs.
pub(super) fn read_into(
    m: &dyn GuestMemory,
    mode: Option<u32>,
    seat: u32,
    body: objects::Body,
    world: [Vec3; 3],
    out: &mut Vec<Entry>,
) -> R<()> {
    out.clear();
    let name = m.u32(seat + 0xc)?;
    let radius = m.f32(seat + 0x98)?;
    let facing = m.f32(seat + 0x9c)?;
    let look = m.f32(seat + 0xa0)?;
    let max_distance = m.f32(seat + 0xa4)?;
    if ![radius, facing, look, max_distance]
        .into_iter()
        .all(|n| n.is_finite() && n > 0.0)
    {
        return Ok(());
    }
    let make = |(at, forward)| Entry {
        at,
        forward,
        radius,
        facing,
        look,
        max_distance,
    };
    if name == 0 {
        out.push(make((world[0], world[1])));
        return Ok(());
    }
    let Some(mode) = mode else {
        return Ok(());
    };
    if let Some((count, base)) = markers(m, mode, name)? {
        for j in 0..count {
            let marker = base + j * 0x24;
            if (count == 1 || m.u16(marker)? == 0xffff)
                && let Some(pose) = world_marker(m, body, marker)?
            {
                out.push(make(pose));
            }
        }
    }
    Ok(())
}

pub(super) fn reference(m: &dyn GuestMemory, at: u32, group: [u8; 4]) -> R<Option<u32>> {
    let Some(id) = DatumHandle::from_raw(m.u32(at)?) else {
        return Ok(None);
    };
    let Some(tag) = Tags.entry(m, id)? else {
        return Ok(None);
    };
    if tag.group(m)? != GroupTag::from_chars(group) {
        return Ok(None);
    }
    Ok(Some(tag.address(m)?).filter(|&a| a != 0))
}

pub(super) fn model(m: &dyn GuestMemory, vehi: u32) -> R<Option<u32>> {
    // obje.model and hlmt.render_model, datum half of each reference.
    let Some(hlmt) = reference(m, vehi + 0x38, *b"hlmt")? else {
        return Ok(None);
    };
    reference(m, hlmt + 4, *b"mode")
}

/// mode.txt: the compression block at +0x14 has one element with
/// position bounds. Measure the side and the full vertical span.
pub(super) fn hull(m: &dyn GuestMemory, mode: u32) -> R<Option<[f32; 2]>> {
    if m.i32(mode + 0x14)? != 1 {
        return Ok(None);
    }
    let at = m.u32(mode + 0x18)?;
    if at == 0 {
        return Ok(None);
    }
    let width = m.f32(at + 8)?.abs().max(m.f32(at + 0xc)?.abs());
    let height = m.f32(at + 0x14)? - m.f32(at + 0x10)?;
    Ok(((0.15..=7.0).contains(&width) && (0.2..=8.0).contains(&height)).then_some([width, height]))
}

/// The rendered camera marker of a unit in its current world pose. When a
/// model has no camera marker or head node pose (common on playerless
/// hosts), use the centre of its upper collision cap. The two biped heights
/// and radius at +0x268 are verified in docs/formats/tag-definitions.md.
/// The cap moves with the biped's crouch amount and its parent vehicle.
pub fn unit_eye(m: &dyn GuestMemory, handle: DatumHandle, crouch: f32) -> R<Option<Vec3>> {
    let Some((objects::BIPED, body)) = objects::get(m, handle)? else {
        return Ok(None);
    };
    let Some(id) = body.definition(m)? else {
        return Ok(None);
    };
    let Some(tag) = Tags.entry(m, id)? else {
        return Ok(None);
    };
    let unit = tag.address(m)?;
    if unit == 0 {
        return Ok(None);
    }
    if let Some(at) = camera_marker(m, body, unit).ok().flatten() {
        return Ok(Some(at));
    }
    let standing = m.f32(unit + 0x268)?;
    let kneeling = m.f32(unit + 0x26c)?;
    let radius = m.f32(unit + 0x270)?;
    if ![standing, kneeling, radius, crouch]
        .iter()
        .all(|x| x.is_finite())
        || !(0.0..=1.0).contains(&crouch)
        || !(standing >= kneeling && kneeling > radius && radius > 0.0)
    {
        return Ok(None);
    }
    let [origin, _, up] = super::world_pose(m, body)?;
    let height = standing + crouch * (kneeling - standing) - radius;
    let at = std::array::from_fn(|i| origin[i] + height * up[i]);
    Ok(at.iter().all(|x| x.is_finite()).then_some(at))
}

fn camera_marker(m: &dyn GuestMemory, body: objects::Body, unit: u32) -> R<Option<Vec3>> {
    let name = m.u32(unit + 0xd4)?;
    if name == 0 {
        return Ok(None);
    }
    let Some(mode) = model(m, unit)? else {
        return Ok(None);
    };
    if let Some((count, base)) = markers(m, mode, name)? {
        for j in 0..count {
            let marker = base + j * 0x24;
            if (count == 1 || m.u16(marker)? == 0xffff)
                && let Some((eye, _)) = world_marker(m, body, marker)?
            {
                return Ok(Some(eye));
            }
        }
    }
    Ok(None)
}

fn markers(m: &dyn GuestMemory, mode: u32, name: u32) -> R<Option<(u32, u32)>> {
    // mode.txt: marker groups at +0x58, 0xc each; markers at group+4,
    // 0x24 each. A group can have several entry points (e.g. Scorpion).
    let count = m.i32(mode + 0x58)?.clamp(0, 512) as u32;
    let groups = m.u32(mode + 0x5c)?;
    if groups != 0 {
        for i in 0..count {
            let g = groups + i * 0xc;
            if m.u32(g)? != name {
                continue;
            }
            let count = m.i32(g + 4)?.clamp(0, 64) as u32;
            let base = m.u32(g + 8)?;
            return Ok((base != 0).then_some((count, base)));
        }
    }
    Ok(None)
}

fn world_marker(m: &dyn GuestMemory, body: objects::Body, marker: u32) -> R<Option<(Vec3, Vec3)>> {
    // native-renderer.md: body+0x114/+0x116 is the size/signed offset
    // of the 0x34-byte world matrices. mode.txt describes the node,
    // local translation and quaternion in each render-model marker.
    let node = u32::from(m.u8(marker + 2)?);
    let bytes = u32::from(m.u16(body.addr + 0x114)?);
    let offset = m.u16(body.addr + 0x116)? as i16;
    if offset <= 0 || (node + 1) * 0x34 > bytes {
        return Ok(None);
    }
    let mx = body.addr + u32::from(offset as u16) + node * 0x34;
    let basis = [m.vec3(mx + 4)?, m.vec3(mx + 0x10)?, m.vec3(mx + 0x1c)?];
    let origin = m.vec3(mx + 0x28)?;
    let scale = m.f32(mx)?;
    let local = m.vec3(marker + 4)?;
    let [x, y, z] = m.vec3(marker + 0x10)?;
    let w = m.f32(marker + 0x1c)?;
    let f = [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + w * z),
        2.0 * (x * z - w * y),
    ];
    let turn = |p: Vec3, i| p[0] * basis[0][i] + p[1] * basis[1][i] + p[2] * basis[2][i];
    let at = std::array::from_fn(|i| origin[i] + scale * turn(local, i));
    let forward = std::array::from_fn(|i| turn(f, i));
    Ok((at.into_iter().chain(forward).all(f32::is_finite)
        && forward.into_iter().map(|x| x * x).sum::<f32>() > 0.1)
        .then_some((at, forward)))
}
