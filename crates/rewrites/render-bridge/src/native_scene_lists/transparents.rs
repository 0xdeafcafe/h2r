use scene_extract::layer_sort::sort;
pub(crate) mod bsp;
mod display;
pub(super) use display::install as install_display;

use super::*;

const LIST_END: u32 = 0x004b_6280;
const LIST_FIRST: u32 = 0x004b_6284;
const LIST_ENTRIES: u32 = 0x004b_6288;
const LIST_ORDER: u32 = 0x004b_628c;
const CAMERA: u32 = 0x004b_9da0;
const STRIDE: u32 = 0x70;
const CAPACITY: u32 = 0x400;

const LIST_OVERFLOW: u32 = 0x004b_6291;
const ENTRY_CAMERA: u32 = 0x0048_5618;
const ENTRY_FORWARD: u32 = 0x0048_5624;
const POLYGON_HANDLER: u32 = 0x0003_5b00;

struct Entry {
    address: u32,
    count: u32,
    bytes: [u8; STRIDE as usize],
}

impl Entry {
    fn read(m: &dyn GuestMemory) -> Result<Option<Self>> {
        let count = m.u32(LIST_END)?;
        if count > CAPACITY {
            return Err(Refusal::Layout);
        }
        if count == CAPACITY {
            return Ok(None);
        }
        Self::at(m, ptr(m, LIST_ENTRIES)?, count).map(Some)
    }

    fn at(m: &dyn GuestMemory, entries: u32, count: u32) -> Result<Self> {
        let address = at(entries, count * STRIDE)?;
        let mut bytes = [0; STRIDE as usize];
        m.read(address, &mut bytes)?;
        bytes[0xc] = 0;
        Ok(Self {
            address,
            count,
            bytes,
        })
    }

    fn word(&mut self, offset: usize, value: u32) {
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn payload(&mut self, m: &dyn GuestMemory, offset: usize, addr: u32, len: u32) -> Result<()> {
        if len as usize > self.bytes.len() - offset {
            return Err(Refusal::Layout);
        }
        if len != 0 {
            m.read(addr, &mut self.bytes[offset..offset + len as usize])?;
        }
        Ok(())
    }

    fn sort_key(
        &mut self,
        m: &dyn GuestMemory,
        class: u32,
        position: [f32; 3],
        radius: Option<f32>,
    ) -> Result<()> {
        let depth = entry_depth(m, position, radius)?;
        self.word(0, class);
        self.word(4, depth.to_bits());
        self.word(8, 0);
        for (i, coord) in position.into_iter().enumerate() {
            self.word(0x64 + i * 4, coord.to_bits());
        }
        Ok(())
    }

    fn geometry(&mut self, m: &dyn GuestMemory, position: [f32; 3], radius: f32, flags: u8, shader: u32, argument: u32) -> Result<()> {
        let class = geometry_class(flags);
        self.sort_key(m, class, position, Some(radius))?;
        self.word(0x20, points::TRANSPARENT_GEOMETRY.base);
        self.word(0x24, shader);
        self.word(0x28, argument);
        Ok(())
    }

    fn publish(self, out: &mut Output) {
        out.array(self.address, self.bytes.to_vec());
        out.word(LIST_END, self.count + 1);
    }
}

fn entry_depth(m: &dyn GuestMemory, position: [f32; 3], radius: Option<f32>) -> Result<f32> {
    let camera = m.vec3(ENTRY_CAMERA)?;
    let [x, y, z] = std::array::from_fn(|i| position[i] - camera[i]);
    let [fx, fy, fz] = m.vec3(ENTRY_FORWARD)?;
    let depth = 0.0 - ((z * fz + y * fy) + x * fx);
    let depth = radius.map_or(depth, |radius| depth - radius);
    if !depth.is_finite() {
        return Err(Refusal::Unsupported);
    }
    Ok(depth)
}

pub(crate) fn display_sort_key(m: &dyn GuestMemory, position: [f32; 3], flags: u8) -> Option<[u32; 2]> {
    Some([geometry_class(flags), entry_depth(m, position, Some(0.)).ok()?.to_bits()])
}

fn geometry_class(flags: u8) -> u32 { [3, 1, 2, 0][usize::from(flags & 3)] }

fn full(m: &dyn GuestMemory, eax: u32) -> Result<Output> {
    let mut out = Output {
        eax: Some(eax & !0xff),
        ..Output::default()
    };
    if m.u8(LIST_OVERFLOW)? != 0 {
        out.byte(LIST_OVERFLOW, 0);
    }
    Ok(out)
}

pub(super) fn effect(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    (SceneList::TransparentEffect, effect_entry(m, cpu, false))
}

pub(super) fn polygon(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    (SceneList::TransparentPolygon, effect_entry(m, cpu, true))
}

fn effect_entry(m: &dyn GuestMemory, cpu: &CpuAbi, polygon: bool) -> Result<Output> {
    fp_supported(cpu)?;
    let Some(mut entry) = Entry::read(m)? else {
        return full(m, cpu.eax);
    };
    let handler = if polygon {
        for i in 0..7 {
            entry.word(0x24 + i * 4, stack(m, cpu, 4 + i as u32 * 4)?);
        }
        if (cpu.ecx as i32) > 0 {
            entry.payload(m, 0x40, stack(m, cpu, 0x20)?, cpu.ecx)?;
        }
        POLYGON_HANDLER
    } else {
        if cpu.ecx != 0 {
            entry.payload(m, 0x24, stack(m, cpu, 8)?, cpu.ecx)?;
        }
        stack(m, cpu, 4)?
    };
    let position = m.vec3(cpu.edx)?;
    entry.sort_key(m, 3, position, None)?;
    entry.word(0x20, handler);
    let eax = (at(entry.address, 0x64)? & !0xff) | 1;
    let mut out = Output {
        eax: Some(eax),
        ..Output::default()
    };
    entry.publish(&mut out);
    Ok(out)
}

pub(super) fn geometry(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    if bsp::parity::checking() {
        return (SceneList::TransparentGeometry, Err(Refusal::Unsupported));
    }
    (SceneList::TransparentGeometry, geometry_entry(m, cpu))
}

fn geometry_entry(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    fp_supported(cpu)?;
    let Some(mut entry) = Entry::read(m)? else {
        return full(m, 0);
    };
    let bounds = stack(m, cpu, 0x38)?;
    let position = m.vec3(at(bounds, 4)?)?;
    let radius = m.f32(at(bounds, 0x10)?)?;
    let flags = m.u8(at(cpu.ebx, 0x16)?)?;
    entry.geometry(m, position, radius, flags, cpu.edi, cpu.ebp)?;
    let plane = m.u8(at(bounds, 0x14)?)?;
    entry.bytes[0xc] = plane;
    let (eax, ecx, edx) = if plane == 0 {
        let eax = (position[2].to_bits() & !0xff) | u32::from(plane);
        (eax, bounds, at(entry.address, 0x64)?)
    } else {
        let normal = at(bounds, 0x18)?;
        entry.payload(m, 0x10, normal, 16)?;
        (m.u32(at(normal, 12)?)?, normal, m.u32(at(normal, 8)?)?)
    };
    let mut out = Output {
        eax: Some(eax),
        ecx: Some(ecx),
        edx: Some(edx),
        ..Output::default()
    };
    entry.publish(&mut out);
    Ok(out)
}

struct Key {
    class: i32,
    depth: f32,
    tie: i32,
    plane: Option<([f32; 3], f32)>,
    position: [f32; 3],
}

impl Key {
    fn read(m: &dyn GuestMemory, address: u32) -> Result<Self> {
        let plane = if m.u8(at(address, 0xc)?)? != 0 {
            Some((m.vec3(at(address, 0x10)?)?, m.f32(at(address, 0x1c)?)?))
        } else {
            None
        };
        Ok(Self {
            class: m.i32(address)?,
            depth: m.f32(at(address, 4)?)?,
            tie: m.i32(at(address, 8)?)?,
            plane,
            position: m.vec3(at(address, 0x64)?)?,
        })
    }

    // The plane comparator uses an ordered SSE > and adds Z, Y, X in
    // that order. Its two branches need not define a total order.
    fn after(&self, other: &Self, camera: [f32; 3]) -> bool {
        fn side(n: [f32; 3], d: f32, p: [f32; 3]) -> bool {
            d > (n[2] * p[2] + n[1] * p[1]) + n[0] * p[0]
        }
        if self.class != other.class {
            return self.class.wrapping_sub(other.class) > 0;
        }
        if let Some((n, d)) = self.plane {
            return side(n, d, camera) != side(n, d, other.position);
        }
        if let Some((n, d)) = other.plane {
            return side(n, d, camera) == side(n, d, self.position);
        }
        if self.depth > other.depth {
            return true;
        }
        if other.depth > self.depth {
            return false;
        }
        self.tie.wrapping_sub(other.tie) > 0
    }
}

/// `TRANSPARENT_ORDER_BEGIN`: the order Rust makes, unless the arm
/// checks this call against the guest's sort or has fallen back
/// ([`crate::native_transparents::sort`]).
pub(super) fn build(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    use crate::native_transparents::sort::{Decision, Pass, SORT};
    let built = build_order(m, cpu).map(|(output, order)| {
        if order.is_empty() {
            return output;
        }
        match SORT.begin(Pass::at(m, cpu.esp), order) {
            Decision::Cut => output,
            Decision::Guest => Output { guest: true, ..Output::default() },
        }
    });
    (SceneList::Transparents, built)
}

/// `TRANSPARENT_ORDER_READY` after a checked call: the guest's order
/// against the one Rust made.
pub(super) fn install_check(b: &mut Builder) {
    use crate::native_transparents::sort::SORT;
    b.raw()
        .observe_before(&points::TRANSPARENT_ORDER_READY, "compare the transparent sort", |c| {
            if !SORT.pending() {
                return;
            }
            let mem = c.guest();
            SORT.ready(guest_order(&*mem));
        })
        .context_only()
        .optional();
}

/// The order `[0x004b628c]` holds for the live rows.
fn guest_order(m: &dyn GuestMemory) -> Option<Vec<u16>> {
    let end = m.u32(LIST_END).ok()?;
    let first = m.u32(LIST_FIRST).ok()?;
    if first > end || end > CAPACITY {
        return None;
    }
    let order = ptr(m, LIST_ORDER).ok()?;
    (0..end - first).map(|i| m.u16(at(order, 2 * i).ok()?).ok()).collect()
}

/// The order's writes, and the order, for the live rows.
fn build_order(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<(Output, Vec<u16>)> {
    fp_supported(cpu)?;
    let end = m.u32(LIST_END)?;
    let first = m.u32(LIST_FIRST)?;
    if first > end || end > CAPACITY {
        return Err(Refusal::Layout);
    }
    let mut output = Output {
        eax: Some(end),
        ..Output::default()
    };
    if first == end {
        return Ok((output, Vec::new()));
    }
    ptr(m, LIST_ENTRIES)?;
    let order = ptr(m, LIST_ORDER)?;
    let indices = sorted(m, first, end)?;
    output.array(
        order,
        indices.iter().copied().flat_map(u16::to_le_bytes).collect(),
    );
    Ok((output, indices))
}

/// The live rows `first..end` in the order `TRANSPARENT_SORT` with the
/// comparator `0x0001e700` leaves them.
fn sorted(m: &dyn GuestMemory, first: u32, end: u32) -> Result<Vec<u16>> {
    let mut indices: Vec<_> = (first as u16..end as u16).collect();
    if indices.len() > 1 {
        let entries = ptr(m, LIST_ENTRIES)?;
        let keys = (first..end)
            .map(|i| Key::read(m, at(entries, i * STRIDE)?))
            .collect::<Result<Vec<_>>>()?;
        let camera = m.vec3(CAMERA)?;
        sort(&mut indices, &|a, b| {
            keys[a as usize - first as usize].after(&keys[b as usize - first as usize], camera)
        });
    }
    Ok(indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    use guest_layout::testing::FakeGuest;

    #[test]
    fn effect_queue_publishes_callback_payload_and_camera_depth() {
        const ROWS: u32 = 0x10000;
        const STACK: u32 = 0x20000;
        const POSITION: u32 = 0x30000;
        const PAYLOAD: u32 = 0x40000;
        let mut m = FakeGuest::new();
        m.map(ROWS, STRIDE)
            .put_u32(LIST_ENTRIES, ROWS)
            .put_u32(LIST_END, 0)
            .put_f32(ENTRY_CAMERA, 1.0)
            .put_f32(ENTRY_FORWARD, 1.0)
            .put_f32(POSITION, 3.0)
            .put_u32(STACK + 4, 0x55000)
            .put_u32(STACK + 8, PAYLOAD)
            .put(PAYLOAD, &[9, 8, 7]);
        let cpu = CpuAbi {
            esp: STACK,
            ecx: 3,
            edx: POSITION,
            ..CpuAbi::xbox_thread()
        };
        let out = effect_entry(&m, &cpu, false).unwrap();
        for edit in out.writes {
            m.put(edit.addr, edit.bytes.slice());
        }
        assert_eq!(m.u32(LIST_END).unwrap(), 1);
        assert_eq!(m.u32(ROWS).unwrap(), 3);
        assert_eq!(m.f32(ROWS + 4).unwrap(), -2.0);
        assert_eq!(m.u32(ROWS + 0x20).unwrap(), 0x55000);
        assert_eq!(m.bytes(ROWS + 0x24, 3).unwrap(), [9, 8, 7]);
        assert_eq!(m.f32(ROWS + 0x64).unwrap(), 3.0);
    }

    #[test]
    fn active_entries_sort_by_class_depth_and_tie_with_guest_swaps() {
        let mut m = FlatMemory::new(LIST_END, 0x7000);
        let entries = 0x004b_a000;
        let order = 0x004b_b000;
        m.set_u32(LIST_ENTRIES, entries).unwrap();
        m.set_u32(LIST_ORDER, order).unwrap();
        m.set_u32(LIST_FIRST, 2).unwrap();
        m.set_u32(LIST_END, 7).unwrap();
        let keys = [
            (3, 1.0, 5),
            (2, 8.0, 8),
            (3, 0.0, 9),
            (3, 1.0, 5),
            (2, 8.0, 3),
        ];
        for (i, (class, depth, tie)) in keys.into_iter().enumerate() {
            let row = entries + (i as u32 + 2) * STRIDE;
            m.set_i32(row, class).unwrap();
            m.set_f32(row + 4, depth).unwrap();
            m.set_i32(row + 8, tie).unwrap();
        }
        let (output, _) = build_order(&m, &CpuAbi::default()).unwrap();
        assert_eq!(output.writes[0].addr, order);
        assert_eq!(
            output.writes[0].bytes.slice(),
            &[6, 0, 3, 0, 4, 0, 5, 0, 2, 0]
        );
    }

    /// A list blending geometry, particle and contrail entries, with ties
    /// and a plane: the order the cut publishes reads back as the order
    /// `TRANSPARENT_ORDER_READY` compares, every neighbour pair without a
    /// plane is in comparator order, and a check of it does not fall back.
    #[test]
    fn blended_order_equals_the_guest_sort() {
        use crate::native_transparents::sort::{Decision, Pass, Sort};
        let mut m = FlatMemory::new(LIST_END, 0x7000);
        let entries = 0x004b_a000;
        let order = 0x004b_b000;
        m.set_u32(LIST_ENTRIES, entries).unwrap();
        m.set_u32(LIST_ORDER, order).unwrap();
        m.set_u32(LIST_FIRST, 1).unwrap();
        m.set_u32(LIST_END, 9).unwrap();
        // (class, depth, tie, handler, plane)
        let rows = [
            (2, 4.0, 0, points::TRANSPARENT_GEOMETRY.base, false),
            (2, 4.0, 0, POLYGON_HANDLER, false),
            (1, 9.0, 3, 0x0003_5b10, false),
            (2, 1.5, 7, points::TRANSPARENT_GEOMETRY.base, true),
            (0, 2.0, 0, POLYGON_HANDLER, false),
            (2, 4.0, 2, 0x0003_5b10, false),
            (1, 9.0, 1, points::TRANSPARENT_GEOMETRY.base, false),
            (3, 0.5, 0, POLYGON_HANDLER, false),
        ];
        for (i, (class, depth, tie, handler, plane)) in rows.into_iter().enumerate() {
            let row = entries + (i as u32 + 1) * STRIDE;
            m.set_i32(row, class).unwrap();
            m.set_f32(row + 4, depth).unwrap();
            m.set_i32(row + 8, tie).unwrap();
            m.set_u8(row + 0xc, u8::from(plane)).unwrap();
            m.set_f32(row + 0x10, 1.0).unwrap();
            m.set_f32(row + 0x1c, 3.0).unwrap();
            m.set_u32(row + 0x20, handler).unwrap();
            m.set_f32(row + 0x64, i as f32).unwrap();
        }
        let (output, indices) = build_order(&m, &CpuAbi::default()).unwrap();
        for edit in &output.writes {
            m.write(edit.addr, edit.bytes.slice()).unwrap();
        }
        assert_eq!(guest_order(&m).as_deref(), Some(&indices[..]));
        let mut seen = indices.clone();
        seen.sort_unstable();
        assert_eq!(seen, (1..9).collect::<Vec<u16>>());
        let key = |i: u16| Key::read(&m, entries + u32::from(i) * STRIDE).unwrap();
        let camera = m.vec3(CAMERA).unwrap();
        for pair in indices.windows(2) {
            let (a, b) = (key(pair[0]), key(pair[1]));
            if a.plane.is_none() && b.plane.is_none() {
                assert!(!a.after(&b, camera), "{pair:?} out of comparator order");
            }
        }
        let sort = Sort::new();
        assert_eq!(sort.begin(Pass::World, indices.clone()), Decision::Guest);
        sort.ready(guest_order(&m));
        assert!(!sort.fallen_back());
    }

    #[test]
    fn partition_and_short_sorts_preserve_retail_tie_order() {
        let mut indices = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        sort(&mut indices, &|_, _| false);
        assert_eq!(indices, [4, 1, 2, 3, 0, 5, 6, 7, 8]);
    }

    #[test]
    fn oriented_plane_uses_camera_and_the_other_entries_position() {
        let plane = Key {
            class: 0,
            depth: 0.0,
            tie: 0,
            plane: Some(([1.0, 0.0, 0.0], 1.0)),
            position: [0.0; 3],
        };
        let other = Key {
            plane: None,
            position: [2.0, 0.0, 0.0],
            ..plane
        };
        assert!(plane.after(&other, [0.0; 3]));
        assert!(!other.after(&plane, [0.0; 3]));
        assert!(!plane.after(&other, [3.0, 0.0, 0.0]));
        assert!(other.after(&plane, [3.0, 0.0, 0.0]));
    }
}
