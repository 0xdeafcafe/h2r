//! H1 0x8ba10 and H2 0xba160 both resolve an attached object's local
//! origin through the parent node. Parent node matrices are in world space.
use guest_layout::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::tags::GroupTag;
use guest_layout::view::{Tags, objects};

pub(super) fn position(
    mem: &dyn GuestMemory,
    target: objects::Body,
) -> Result<Option<[f32; 3]>, MemFault> {
    let local = target.position(mem)?;
    let Some(parent) = target.parent(mem)? else {
        return Ok(Some(local));
    };
    let Some((_, parent)) = objects::get(mem, parent)? else {
        return Ok(None);
    };
    let node = mem.u8(target.addr + 0x18)? as i8;
    node_point(mem, parent.addr, i16::from(node), local)
}

// H1 0x3e480 measures from the unit's sight point, even when a vehicle's
// third-person camera is behind the player. H2 0xcafc0 identifies the same
// biped heights, seat marker and body-marker paths in the native records.
pub(super) fn eye(mem: &dyn GuestMemory, unit: DatumHandle) -> Result<Option<[f32; 3]>, MemFault> {
    let Some((kind, body)) = objects::get(mem, unit)? else {
        return Ok(None);
    };
    if let Some(parent) = body.parent(mem)? {
        let Some((kind, parent)) = objects::get(mem, parent)? else {
            return Ok(None);
        };
        let seat = mem.u16(body.addr + 0x1fc)? as i16;
        if !matches!(kind, objects::BIPED | objects::VEHICLE) || seat < 0 {
            return parent.position(mem).map(Some);
        }
        let Some(def) = tag(mem, parent.addr, *b"unit")? else {
            return Ok(None);
        };
        let Some(name) = seat_marker(mem, def, seat, 0x60)? else {
            return Ok(None);
        };
        if kind == objects::VEHICLE && name == 0 {
            return parent.position(mem).map(Some);
        }
        return marker_position(mem, parent, def, name);
    }
    let Some(def) = tag(mem, body.addr, *b"unit")? else {
        return Ok(None);
    };
    if kind == objects::BIPED && body.damage_flags(mem)? & 4 == 0 {
        return biped_eye(mem, body, def).map(Some);
    }
    let rider = DatumHandle(mem.u32(body.addr + 0x24c)?);
    let marker = if let Some((_, rider)) = objects::get(mem, rider)? {
        let seat = mem.u16(rider.addr + 0x1fc)? as i16;
        if seat >= 0 {
            let Some(name) = seat_marker(mem, def, seat, 8)? else {
                return Ok(None);
            };
            name
        } else {
            HEAD
        }
    } else {
        HEAD
    };
    marker_position(mem, body, def, marker)
}

// H1 0x3e4ea..0x3e50e. The converter places the two source heights
// at native bipd +0x218 and +0x21c.
fn biped_eye(mem: &dyn GuestMemory, body: objects::Body, def: u32) -> Result<[f32; 3], MemFault> {
    let mut from = body.position(mem)?;
    let crouch = mem.f32(body.addr + 0x2c4)?;
    from[2] += (1.0 - crouch) * mem.f32(def + 0x218)? + crouch * mem.f32(def + 0x21c)?;
    Ok(from)
}

const HEAD: u32 = 0x0400_0095; // H2 0xcb378

fn tag(mem: &dyn GuestMemory, datum: u32, group: [u8; 4]) -> Result<Option<u32>, MemFault> {
    Tags.get(
        mem,
        DatumHandle(mem.u32(datum)?),
        GroupTag::from_chars(group),
    )
}

fn seat_marker(
    mem: &dyn GuestMemory,
    def: u32,
    seat: i16,
    field: u32,
) -> Result<Option<u32>, MemFault> {
    if u32::from(seat as u16) >= mem.u32(def + 0x1c8)? {
        return Ok(None);
    }
    let at = mem.u32(def + 0x1cc)? + seat as u32 * 0xb0;
    Ok(Some(mem.u32(at + field)?))
}

fn marker_position(
    mem: &dyn GuestMemory,
    body: objects::Body,
    def: u32,
    name: u32,
) -> Result<Option<[f32; 3]>, MemFault> {
    if name != 0
        && name != u32::MAX
        && let Some(model) = tag(mem, def + 0x38, *b"hlmt")?
        && let Some(render) = tag(mem, model + 4, *b"mode")?
    {
        let groups = mem.u32(render + 0x5c)?;
        for i in 0..mem.u32(render + 0x58)? {
            let group = groups + i * 12;
            if mem.u32(group)? != name {
                continue;
            }
            let markers = mem.u32(group + 8)?;
            for i in 0..mem.u32(group + 4)? {
                let marker = markers + i * 0x24;
                // Native marker lookup 0x1d8faf admits any region or the
                // currently active permutation of the marker's region.
                let region = mem.u8(marker)?;
                if region != u8::MAX {
                    let permutations = mem.u16(body.addr + 0x11a)? as i16;
                    if permutations < 0
                        || mem.u8(body.addr + permutations as u32 + u32::from(region))?
                            != mem.u8(marker + 1)?
                    {
                        continue;
                    }
                }
                return node_point(
                    mem,
                    body.addr,
                    i16::from(mem.u8(marker + 2)?),
                    mem.vec3(marker + 4)?,
                );
            }
            break;
        }
    }
    // Both retail marker routines return node zero's transform when the
    // named marker is absent (H1 0x8d6cb, H2 0xb8dfc).
    node_point(mem, body.addr, 0, [0.0; 3])
}

fn node_point(
    mem: &dyn GuestMemory,
    parent: u32,
    node: i16,
    local: [f32; 3],
) -> Result<Option<[f32; 3]>, MemFault> {
    let size = i32::from(mem.u16(parent + 0x114)?);
    let block = mem.u16(parent + 0x116)? as i16;
    let offset = i32::from(node) * 0x34;
    if offset < 0 || block < 0 || offset + 0x34 > size {
        return Ok(None);
    }
    let at = parent + block as u32 + offset as u32;
    let scale = mem.f32(at)?;
    let forward = mem.vec3(at + 4)?;
    let left = mem.vec3(at + 0x10)?;
    let up = mem.vec3(at + 0x1c)?;
    let origin = mem.vec3(at + 0x28)?;
    Ok(Some(std::array::from_fn(|i| {
        origin[i] + scale * (local[0] * forward[i] + local[1] * left[i] + local[2] * up[i])
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn detached_target_uses_its_own_origin() {
        let mut mem = FlatMemory::new(0x1000, 0x100);
        mem.set_u32(0x1014, u32::MAX).unwrap();
        for (i, value) in [2.0, 3.0, 4.0].into_iter().enumerate() {
            mem.set_f32(0x1064 + i as u32 * 4, value).unwrap();
        }
        assert_eq!(
            position(&mem, objects::Body::at(0x1000)).unwrap(),
            Some([2.0, 3.0, 4.0])
        );
    }

    #[test]
    fn sight_point_interpolates_the_converted_biped_heights() {
        let mut mem = FlatMemory::new(0x1000, 0x800);
        let body = objects::Body::at(0x1000);
        let def = 0x1400;
        mem.set_f32(body.addr + 0x6c, 5.0).unwrap();
        mem.set_f32(def + 0x218, 1.0).unwrap();
        mem.set_f32(def + 0x21c, 0.5).unwrap();
        for (crouch, z) in [(0.0, 6.0), (0.5, 5.75), (1.0, 5.5)] {
            mem.set_f32(body.addr + 0x2c4, crouch).unwrap();
            assert_eq!(biped_eye(&mem, body, def).unwrap(), [0.0, 0.0, z]);
        }
    }

    #[test]
    fn empty_marker_uses_root_node_origin() {
        let mut mem = FlatMemory::new(0x1000, 0x300);
        let body = objects::Body::at(0x1000);
        mem.set_u16(0x1114, 0x34).unwrap();
        mem.set_u16(0x1116, 0x200).unwrap();
        mem.set_f32(0x1228, 12.0).unwrap();
        assert_eq!(
            marker_position(&mem, body, 0, 0).unwrap(),
            Some([12.0, 0.0, 0.0])
        );
    }

    #[test]
    fn attached_target_uses_selected_world_node_and_rejects_missing_nodes() {
        let mut mem = FlatMemory::new(0x1000, 0x300);
        mem.set_u16(0x1114, 2 * 0x34).unwrap();
        mem.set_u16(0x1116, 0x200).unwrap();
        let words = [
            2.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 30.0,
        ];
        for (i, value) in words.into_iter().enumerate() {
            mem.set_f32(0x1234 + i as u32 * 4, value).unwrap();
        }
        let local = [2.0, 3.0, 4.0];
        assert_eq!(
            node_point(&mem, 0x1000, 1, local).unwrap(),
            Some([4.0, 24.0, 38.0])
        );
        assert_eq!(node_point(&mem, 0x1000, -1, local).unwrap(), None);
        assert_eq!(node_point(&mem, 0x1000, 2, local).unwrap(), None);
    }
}
