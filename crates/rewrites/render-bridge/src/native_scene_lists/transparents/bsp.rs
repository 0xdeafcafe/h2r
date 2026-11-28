//! BSP inputs to the geometry queue. The material walk (0x45ce0) uses
//! the authored part centre for clusters and the placement sphere centre
//! for instances. Its BSP sort radius is zero in both cases.

pub(crate) mod parity;

use super::*;

pub(crate) struct Bounds {
    pub position: [f32; 3],
    pub plane: Option<[f32; 4]>,
}

impl Bounds {
    pub(crate) fn read(
        m: &dyn GuestMemory,
        bsp: u32,
        planes: u32,
        owner: native_render::world::Owner,
        part: u8,
        part_address: u32,
    ) -> Option<Self> {
        use native_render::world::Owner;
        let (key, position, transform) = match owner {
            Owner::Cluster(id) => (
                u16::try_from(id).ok()?,
                m.vec3(part_address.checked_add(0x10)?).ok()?,
                None,
            ),
            Owner::Instance(id) => {
                if id >= m.u32(bsp.checked_add(0x140)?).ok()? {
                    return None;
                }
                let at = m
                    .u32(bsp.checked_add(0x144)?)
                    .ok()?
                    .checked_add(id * 0x58)?;
                (
                    m.u16(at.checked_add(0x34)?).ok()?,
                    m.vec3(at.checked_add(0x3c)?).ok()?,
                    Some(at),
                )
            }
        };
        let packed = u32::from(key) | (u32::from(part) * 2 + u32::from(transform.is_some())) << 16;
        let mut plane = lookup(m, planes, packed)?;
        if let (Some(at), Some(local)) = (transform, plane) {
            let mut world = [0.; 4];
            for i in 0..3 {
                let column = at.checked_add(4 + i as u32 * 4)?;
                world[i] = (m.f32(column.checked_add(24)?).ok()? * local[2]
                    + m.f32(column.checked_add(12)?).ok()? * local[1])
                    + m.f32(column).ok()? * local[0];
            }
            let t = m.vec3(at.checked_add(0x28)?).ok()?;
            world[3] = ((local[3] * m.f32(at).ok()? + t[2] * world[2]) + t[1] * world[1])
                + t[0] * world[0];
            plane = Some(world);
        }
        Some(Self { position, plane })
    }
}

// 0x1e280 calls the table's signed-byte hash then compares both packed
// shorts. Read the image's hash weights so no retail table is embedded.
fn lookup(m: &dyn GuestMemory, table: u32, key: u32) -> Option<Option<[f32; 4]>> {
    let count = m.u32(table.checked_add(0x20)?).ok()?;
    if count == 0
        || m.u32(table.checked_add(0x2c)?).ok()? != PLANE_HASH
        || m.u32(table.checked_add(0x30)?).ok()? != PLANE_EQUAL
    {
        return None;
    }
    if count == 1 && m.u32(table.checked_add(0x3c)?).ok()? == 0 {
        return Some(None);
    }
    let [a, b, c, d] = key.to_le_bytes().map(|v| i32::from(v as i8));
    let weight = |addr| m.u8(addr).ok().map(|v| i32::from(v as i8));
    let hash = a
        .wrapping_shl(u32::from(m.u8(PLANE_HASH + 0x24).ok()?))
        .wrapping_add(a)
        .wrapping_add(b * weight(PLANE_HASH + 0x1d)?)
        .wrapping_add(c * weight(PLANE_HASH + 0x11)?)
        .wrapping_add(d * weight(PLANE_HASH + 0x0e)?) as u32;
    let bucket = (hash % count).checked_mul(4)?.checked_add(0x3c)?;
    let mut node = m.u32(table.checked_add(bucket)?).ok()?;
    let mut seen = std::collections::HashSet::new();
    while node != 0 {
        if !seen.insert(node) {
            return None;
        }
        if m.u32(node.checked_add(4)?).ok()? == hash {
            let data = m.u32(node).ok()?;
            if data == 0 {
                return None;
            }
            if m.u32(data).ok()? == key {
                let [x, y, z] = m.vec3(data.checked_add(4)?).ok()?;
                return Some(Some([x, y, z, m.f32(data.checked_add(16)?).ok()?]));
            }
        }
        node = m.u32(node.checked_add(8)?).ok()?;
    }
    Some(None)
}

pub(crate) const PLANE_HASH: u32 = 0x0001_e8c0;
pub(crate) const PLANE_EQUAL: u32 = 0x0001_e8f0;

#[derive(Default)]
pub(crate) struct SectionQueue {
    start: Option<(u32, u32)>,
    rows: Vec<u8>,
}

impl SectionQueue {
    pub(crate) fn push(
        &mut self,
        m: &dyn GuestMemory,
        bounds: Bounds,
        shader: u32,
        argument: u32,
    ) -> Option<()> {
        let (first, entries) = match self.start {
            Some(start) => start,
            None => {
                let start = (m.u32(LIST_END).ok()?, ptr(m, LIST_ENTRIES).ok()?);
                self.start = Some(start);
                start
            }
        };
        let index = first.checked_add(u32::try_from(self.rows.len() / STRIDE as usize).ok()?)?;
        // Overflow clears a guest diagnostic latch. Let the original walk
        // own the whole section if any append would exceed the list.
        if index >= CAPACITY {
            return None;
        }
        let mut entry = Entry::at(m, entries, index).ok()?;
        let table = ptr(m, TAGS).ok()?;
        let tag = ptr(m, table.checked_add((shader & 0xffff) * 16 + 8)?).ok()?;
        let flags = m.u8(tag.checked_add(0x16)?).ok()?;
        entry
            .geometry(m, bounds.position, 0., flags, shader, argument)
            .ok()?;
        if let Some(plane) = bounds.plane {
            entry.bytes[0xc] = 1;
            for (i, v) in plane.into_iter().enumerate() {
                entry.word(0x10 + i * 4, v.to_bits());
            }
        }
        self.rows.extend_from_slice(&entry.bytes);
        Some(())
    }

    pub(crate) fn pending(&self) -> Option<(u32, &[u8], u32, [u8; 4])> {
        let (first, entries) = self.start?;
        let count = first + (self.rows.len() / STRIDE as usize) as u32;
        Some((
            entries + first * STRIDE,
            &self.rows,
            LIST_END,
            count.to_le_bytes(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use native_render::world::Owner;

    #[test]
    fn placement_uses_the_definition_plane_but_its_own_centre_and_transform() {
        let mut m = FakeGuest::new();
        let (bsp, table, node, data, instances) = (0x7000, 0x8000, 0x9000, 0xa000, 0xb000);
        // Invented weights. Key bytes are [2, 0, 5, 0] for definition 2,
        // part 2 on a placement. Both shorts and the cached hash must match.
        let hash = 2 * 3 + 5 * 7;
        m.put_u8(PLANE_HASH + 0x24, 1)
            .put_u8(PLANE_HASH + 0x1d, 3)
            .put_u8(PLANE_HASH + 0x11, 7)
            .put_u8(PLANE_HASH + 0x0e, 11)
            .put_u32(table + 0x20, 5)
            .put_u32(table + 0x2c, PLANE_HASH)
            .put_u32(table + 0x30, PLANE_EQUAL)
            .put_u32(table + 0x3c + 4 * (hash % 5), node)
            .put_u32(node, data)
            .put_u32(node + 4, hash)
            .put_u32(data, (5 << 16) | 2)
            .put_f32(data + 4, 1.)
            .put_f32(data + 8, 2.)
            .put_f32(data + 12, 3.)
            .put_f32(data + 16, 4.)
            .put_u32(bsp + 0x140, 1)
            .put_u32(bsp + 0x144, instances)
            .put_f32(instances, 2.)
            .put_f32(instances + 8, 1.)
            .put_f32(instances + 0x10, -1.)
            .put_f32(instances + 0x24, 1.)
            .put_f32(instances + 0x28, 3.)
            .put_f32(instances + 0x2c, 4.)
            .put_f32(instances + 0x30, 5.)
            .put_u16(instances + 0x34, 2)
            .put_f32(instances + 0x3c, 7.)
            .put_f32(instances + 0x40, 8.)
            .put_f32(instances + 0x44, 9.);
        let bounds = Bounds::read(&m, bsp, table, Owner::Instance(0), 2, 0).unwrap();
        assert_eq!(bounds.position, [7., 8., 9.]);
        assert_eq!(bounds.plane, Some([-2., 1., 3., 21.]));
        m.put_u32(data, 3).put_u32(node + 8, node);
        assert!(Bounds::read(&m, bsp, table, Owner::Instance(0), 2, 0).is_none());
    }
}
