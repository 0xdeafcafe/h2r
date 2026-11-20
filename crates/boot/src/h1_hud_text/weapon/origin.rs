//! H1 0x8ba10 and H2 0xba160 both resolve an attached object's local
//! origin through the parent node. Parent node matrices are in world space.
use engine::memory::{GuestMemory, MemFault};
use engine::view::objects;

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
    node_point(mem, parent.addr, node, local)
}

fn node_point(
    mem: &dyn GuestMemory,
    parent: u32,
    node: i8,
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
    use engine::memory::FlatMemory;

    #[test]
    fn detached_target_uses_its_own_origin() {
        let mut mem = FlatMemory::new(0x1000, 0x100);
        mem.set_u32(0x1014, u32::MAX).unwrap();
        for (i, value) in [2.0, 3.0, 4.0].into_iter().enumerate() {
            mem.set_f32(0x1064 + i as u32 * 4, value).unwrap();
        }
        assert_eq!(position(&mem, objects::Body::at(0x1000)).unwrap(), Some([2.0, 3.0, 4.0]));
    }

    #[test]
    fn attached_target_uses_selected_world_node_and_rejects_missing_nodes() {
        let mut mem = FlatMemory::new(0x1000, 0x300);
        mem.set_u16(0x1114, 2 * 0x34).unwrap();
        mem.set_u16(0x1116, 0x200).unwrap();
        let words = [2.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 30.0];
        for (i, value) in words.into_iter().enumerate() {
            mem.set_f32(0x1234 + i as u32 * 4, value).unwrap();
        }
        let local = [2.0, 3.0, 4.0];
        assert_eq!(node_point(&mem, 0x1000, 1, local).unwrap(), Some([4.0, 24.0, 38.0]));
        assert_eq!(node_point(&mem, 0x1000, -1, local).unwrap(), None);
        assert_eq!(node_point(&mem, 0x1000, 2, local).unwrap(), None);
    }
}
