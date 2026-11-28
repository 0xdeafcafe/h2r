//! Particle emission without taking ownership of particle simulation.
//!
//! Research points: `0x0023ed30` emits one quad after the caller evaluated
//! the properties; `0x0023ecd0` selects its attribute registers. The helper
//! writes two vectors, size/rotation, colour, quad and two UV windows, then
//! one inline quad. `0x00174420` is not this module's (render-bridge's effects
//! shadow makes it and keeps its stamps out of the arena).
//! No particle datum is read or changed here. See native-particles.md.

use crate::camera::GuestRead;

pub const EMIT_QUAD: u32 = 0x0023_ed30;
pub const RENDER_UPDATE: u32 = 0x0017_4420;
pub const EMIT_ENABLED: u32 = 0x0047_0a38;
pub const ATTRIBUTE_REGISTERS: u32 = 0x0047_0a3c;
/// Eight arguments popped by the emitter; EBX carries a ninth pointer.
pub const STACK_BYTES: u16 = 32;

/// Why a guest call's inputs cannot be read. The emission itself has no
/// refusals: [`read_emission`] reads every call the guest's emitter would
/// draw. `Disabled` and `Geometry` are the dynamic builders' own
/// ([`crate::effect_geometry::read_call`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Unreadable,
    AddressOverflow,
    Disabled,
    /// An attribute register past the NV2A's sixteen.
    AttributeRegister,
    Geometry,
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
/// draw. Dynamic register assignments are read, not hard-coded from the
/// executable's initializer. `None` is the emitter's gate (`0x00470a38`)
/// off: the guest draws nothing then either.
///
/// Two slots naming one register write it in instruction order, so the
/// later write is the one drawn, as on the NV2A. A slot naming v0 is
/// overwritten by the corner stream ([`Emission::vertices`]).
pub fn read_emission(mem: &dyn GuestRead, esp: u32, ebx: u32) -> Result<Option<Emission>, Refused> {
    if mem.read_u32(EMIT_ENABLED).ok_or(Refused::Unreadable)? & 0xff == 0 {
        return Ok(None);
    }
    let args = words::<8>(mem, esp.checked_add(4).ok_or(Refused::AddressOverflow)?)?;
    let registers = words::<8>(mem, ATTRIBUTE_REGISTERS)?;
    if registers.iter().any(|&r| r >= 16) {
        return Err(Refused::AttributeRegister);
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
    Ok(Some(Emission { attributes }))
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
    /// The adapter checks the live declaration (v0 SHORT4, 8 bytes, one
    /// stream) before it expands.
    pub fn vertices(&self, defaults: [[f32; 4]; 16]) -> [Vertex; 4] {
        let mut registers = defaults;
        for (r, value) in registers.iter_mut().zip(&self.attributes) {
            if let Some(value) = value {
                *r = value.map(f32::from_bits);
            }
        }
        [[0., 0.], [1., 0.], [1., 1.], [0., 1.]].map(|[x, y]| {
            let mut registers = registers;
            registers[0] = [x, y, 0., 0.];
            Vertex { registers }
        })
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
pub fn differing_registers(expected: &[Vertex; 4], actual: &[Vertex; 4]) -> u16 {
    let mut mask = 0;
    for (e, a) in expected.iter().zip(actual) {
        for r in 0..16 {
            if e.registers[r].map(f32::to_bits) != a.registers[r].map(f32::to_bits) {
                mask |= 1 << r;
            }
        }
    }
    mask
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
        let e = read_emission(&m, 0x100, 0x280).unwrap().unwrap();
        let mut defaults = [[42.; 4]; 16];
        defaults[12][0] = f32::from_bits(0x7fc0_0042);
        let v = e.vertices(defaults);
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
    fn missing_words_and_registers_past_sixteen_refuse() {
        let mut m = memory();
        m.0.remove(&0x28c);
        assert_eq!(read_emission(&m, 0x100, 0x280), Err(Refused::Unreadable));
        let mut m = memory();
        m.0.insert(ATTRIBUTE_REGISTERS, 16);
        assert_eq!(read_emission(&m, 0x100, 0x280), Err(Refused::AttributeRegister));
        let m = memory();
        assert_eq!(read_emission(&m, u32::MAX, 0), Err(Refused::AddressOverflow));
    }
    #[test]
    fn the_emitter_gate_off_draws_nothing_rather_than_refusing() {
        let mut m = memory();
        m.0.insert(EMIT_ENABLED, 0x100);
        assert_eq!(read_emission(&m, 0x100, 0x280), Ok(None));
    }
    #[test]
    fn two_slots_on_one_register_draw_the_later_write() {
        // Slot 0 (the vector at args[1]) and slot 1 (args[5]) both name
        // register 2; slot 1 is written after slot 0.
        let mut m = memory();
        m.0.insert(ATTRIBUTE_REGISTERS, 2);
        m.0.insert(ATTRIBUTE_REGISTERS + 4, 2);
        let v = read_emission(&m, 0x100, 0x280).unwrap().unwrap().vertices([[0.; 4]; 16]);
        assert_eq!(v[0].registers[2], [0.25, 0., 0., 1.]);
        // A slot on v0 is overwritten by the corner stream.
        let mut m = memory();
        m.0.insert(ATTRIBUTE_REGISTERS, 0);
        let v = read_emission(&m, 0x100, 0x280).unwrap().unwrap().vertices([[0.; 4]; 16]);
        assert_eq!(v[1].registers[0], [1., 0., 0., 0.]);
    }
    #[test]
    fn compare_is_bitwise() {
        let e = read_emission(&memory(), 0x100, 0x280).unwrap().unwrap();
        let v = e.vertices([[0.; 4]; 16]);
        let mut other = v;
        other[2].registers[9][1] = -0.0;
        assert_eq!(differing_registers(&v, &other), 1 << 9);
        assert_eq!(QUAD_INDICES, [0, 1, 2, 0, 2, 3]);
    }
}
