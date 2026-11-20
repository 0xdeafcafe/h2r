//! Particle emission without taking ownership of particle simulation.
//!
//! Research points: `0x0023ed30` emits one quad after the caller evaluated
//! the properties; `0x0023ecd0` selects its attribute registers. The helper
//! writes two vectors, size/rotation, colour, quad and two UV windows, then
//! one inline quad. `0x00174420` remains guest-owned, including its writes.
//! No particle datum is read or changed here. See native-particles.md.

use crate::camera::GuestRead;

pub const EMIT_QUAD: u32 = 0x0023_ed30;
pub const RENDER_UPDATE: u32 = 0x0017_4420;
pub const EMIT_ENABLED: u32 = 0x0047_0a38;
pub const ATTRIBUTE_REGISTERS: u32 = 0x0047_0a3c;
/// Eight arguments popped by the emitter; EBX carries a ninth pointer.
pub const STACK_BYTES: u16 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Unreadable,
    AddressOverflow,
    Disabled,
    AttributeRegister,
    Declaration,
    Geometry,
    Identity,
}

/// Inputs copied at the emission boundary. Missing registers retain the
/// issue-time device defaults. Words preserve NaN payloads and signed zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emission {
    pub attributes: [Option<[u32; 4]>; 16],
}

fn words<const N: usize>(mem: &dyn GuestRead, at: u32) -> Result<[u32; N], Refused> {
    let mut out = [0; N];
    for (i, word) in out.iter_mut().enumerate() {
        *word = mem
            .read_u32(
                at.checked_add(4 * i as u32)
                    .ok_or(Refused::AddressOverflow)?,
            )
            .ok_or(Refused::Unreadable)?;
    }
    Ok(out)
}

/// Decode the helper's entry ABI. Read-only; partial reads never admit a
/// draw. Dynamic register assignments are validated, not hard-coded from
/// the executable's initializer.
pub fn read_emission(mem: &dyn GuestRead, esp: u32, ebx: u32) -> Result<Emission, Refused> {
    if mem.read_u32(EMIT_ENABLED).ok_or(Refused::Unreadable)? & 0xff == 0 {
        return Err(Refused::Disabled);
    }
    let args = words::<8>(mem, esp.checked_add(4).ok_or(Refused::AddressOverflow)?)?;
    let registers = words::<8>(mem, ATTRIBUTE_REGISTERS)?;
    let mut used = 1u16; // v0 is the corner stream, never a constant attribute.
    for &r in &registers {
        if r >= 16 || used & (1 << r) != 0 {
            return Err(Refused::AttributeRegister);
        }
        used |= 1 << r;
    }
    let one = 1.0f32.to_bits();
    let vector = |at| words::<3>(mem, at).map(|v| [v[0], v[1], v[2], one]);
    let mut attributes = [None; 16];
    // Preserve the instruction order (register table slots are semantic,
    // not the order of the writes). DATA4UB uses little-endian RGBA.
    for (slot, value) in [
        (0, vector(args[1])?),
        (1, [args[5], 0, 0, one]),
        (3, vector(args[0])?),
        (4, [args[2], args[3], 0, one]),
        (
            7,
            args[4]
                .to_le_bytes()
                .map(|v| (f32::from(v) / 255.0).to_bits()),
        ),
        (2, words::<4>(mem, args[6])?),
        (5, words::<4>(mem, ebx)?),
        (6, words::<4>(mem, args[7])?),
    ] {
        attributes[registers[slot] as usize] = Some(value);
    }
    Ok(Emission { attributes })
}

/// The only proved inline declaration for this first slice. The adapter
/// must check the *live* declaration: v0 SHORT4, 8 bytes, no other streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Declaration {
    CornerShort4,
    Unsupported,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub registers: [[f32; 4]; 16],
}

impl Emission {
    /// Expand the emitted SHORT4 corners exactly; w is zero, not one.
    /// Other registers keep their issue-time values, overridden by the
    /// helper's writes. D3D performs the same exact i16-to-f32 conversion.
    pub fn vertices(
        &self,
        declaration: Declaration,
        defaults: [[f32; 4]; 16],
    ) -> Result<[Vertex; 4], Refused> {
        if declaration != Declaration::CornerShort4 {
            return Err(Refused::Declaration);
        }
        let mut registers = defaults;
        for (r, value) in registers.iter_mut().zip(&self.attributes) {
            if let Some(value) = value {
                *r = value.map(f32::from_bits);
            }
        }
        Ok([[0., 0.], [1., 0.], [1., 1.], [0., 1.]].map(|[x, y]| {
            let mut registers = registers;
            registers[0] = [x, y, 0., 0.];
            Vertex { registers }
        }))
    }
}

/// No sorting or deduplication: each primitive retains its position in
/// the guest's sorted stream, even if another primitive is identical.
pub const QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// Device/map/view/call and target-copy identity, assigned by the adapter.
/// A reference and a scaled copy are distinct commitments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawIdentity {
    pub device: u64,
    pub generation: u64,
    pub view: u64,
    pub call: u64,
    pub copy: u32,
}

/// Bitwise compare: float equality hides signed zero and cannot pair NaNs.
/// This is deliberately independent of shader/material matching.
pub fn differing_registers(expected: &[Vertex], actual: &[Vertex]) -> Result<u16, Refused> {
    if expected.len() != actual.len() {
        return Err(Refused::Geometry);
    }
    let mut mask = 0;
    for (e, a) in expected.iter().zip(actual) {
        for r in 0..16 {
            if e.registers[r].map(f32::to_bits) != a.registers[r].map(f32::to_bits) {
                mask |= 1 << r;
            }
        }
    }
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    struct Mem(HashMap<u32, u32>);
    impl GuestRead for Mem {
        fn read_u32(&self, a: u32) -> Option<u32> {
            self.0.get(&a).copied()
        }
    }
    fn memory() -> Mem {
        let mut m = Mem(HashMap::new());
        m.0.insert(EMIT_ENABLED, 1);
        for i in 0..8 {
            m.0.insert(ATTRIBUTE_REGISTERS + i * 4, i + 1);
        }
        let args = [
            0x200,
            0x220,
            2.0f32.to_bits(),
            (-0.0f32).to_bits(),
            0x8040_20ff,
            0.25f32.to_bits(),
            0x240,
            0x260,
        ];
        for (i, a) in args.into_iter().enumerate() {
            m.0.insert(0x104 + i as u32 * 4, a);
        }
        for base in [0x200, 0x220, 0x240, 0x260, 0x280] {
            for i in 0..4 {
                m.0.insert(base + i * 4, (base as f32 + i as f32).to_bits());
            }
        }
        m
    }
    #[test]
    fn emission_matches_attribute_write_semantics_and_keeps_defaults() {
        let m = memory();
        let before = m.0.clone();
        let e = read_emission(&m, 0x100, 0x280).unwrap();
        let mut defaults = [[42.; 4]; 16];
        defaults[12][0] = f32::from_bits(0x7fc0_0042);
        let v = e.vertices(Declaration::CornerShort4, defaults).unwrap();
        assert_eq!(v[0].registers[1], [544., 545., 546., 1.]);
        assert_eq!(v[0].registers[2], [0.25, 0., 0., 1.]);
        assert_eq!(v[0].registers[4], [512., 513., 514., 1.]);
        assert_eq!(
            v[0].registers[5].map(f32::to_bits),
            [2.0f32.to_bits(), 0x8000_0000, 0, 1.0f32.to_bits()]
        );
        assert_eq!(v[0].registers[8], [1., 32. / 255., 64. / 255., 128. / 255.]);
        assert_eq!(v[2].registers[0], [1., 1., 0., 0.]);
        assert_eq!(v[0].registers[12][0].to_bits(), 0x7fc0_0042);
        assert_eq!(m.0, before, "never writes the particle or device state");
    }
    #[test]
    fn missing_words_aliases_and_wrong_declarations_refuse() {
        let mut m = memory();
        m.0.remove(&0x28c);
        assert_eq!(read_emission(&m, 0x100, 0x280), Err(Refused::Unreadable));
        let mut m = memory();
        m.0.insert(ATTRIBUTE_REGISTERS, 2);
        assert_eq!(
            read_emission(&m, 0x100, 0x280),
            Err(Refused::AttributeRegister)
        );
        let m = memory();
        assert_eq!(
            read_emission(&m, u32::MAX, 0),
            Err(Refused::AddressOverflow)
        );
        let e = read_emission(&m, 0x100, 0x280).unwrap();
        assert_eq!(
            e.vertices(Declaration::Unsupported, [[0.; 4]; 16]),
            Err(Refused::Declaration)
        );
    }
    #[test]
    fn compare_is_bitwise_and_never_pairs_different_geometry() {
        let e = read_emission(&memory(), 0x100, 0x280).unwrap();
        let v = e
            .vertices(Declaration::CornerShort4, [[0.; 4]; 16])
            .unwrap();
        let mut other = v;
        other[2].registers[9][1] = -0.0;
        assert_eq!(differing_registers(&v, &other), Ok(1 << 9));
        assert_eq!(differing_registers(&v, &other[..3]), Err(Refused::Geometry));
        assert_eq!(QUAD_INDICES, [0, 1, 2, 0, 2, 3]);
    }
}
