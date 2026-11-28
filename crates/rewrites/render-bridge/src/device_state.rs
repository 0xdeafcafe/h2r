//! The guest's own copy of the device state its D3D setters keep
//! (docs/graphics/d3d-entries.md, "Render-state and texture-state
//! setters").
//!
//! Each `SetRenderState_*` and `SetTextureState_*` entry pushes NV2A
//! methods and stores its value in `D3D__RenderState` (`0x407498`, one
//! dword per guest slot) or `D3D__TextureState` (`0x404dd0`, 32 dwords per
//! stage). Game code reads those arrays back (inlined `GetRenderState`), so
//! the answer here is the store alone: the slot gets the value, as the
//! retail body leaves it, and nothing is pushed (no push buffer exists; the
//! native renderer takes its state from the scene).
//!
//! Not written: D3D's own commit bookkeeping (the dirty word `0x407490`,
//! the texgen byte at `0x475cbd + stage`, the W-buffer viewport refresh of
//! `ZEnable`, the render-target rebind of the two multisample modes). Only
//! the deferred commit `0x400fa0` and the push fragments read them, and
//! those are gaps (`crate::guest_entries`).

use runtime::{CpuAbi, Guest, GuestFault};

/// `D3D__RenderState`: guest slot `n` at `RENDER_STATE + 4n`.
pub const RENDER_STATE: u32 = 0x0040_7498;
/// `D3D__TextureState`: stage `s`, state `t` at `TEXTURE_STATE + 0x80 s + 4 t`.
pub const TEXTURE_STATE: u32 = 0x0040_4dd0;
/// Bytes per stage in [`TEXTURE_STATE`].
pub const STAGE_STRIDE: u32 = 0x80;

/// `D3DTSS_TEXCOORDINDEX`, `D3DTSS_BORDERCOLOR`, `D3DTSS_COLORKEYCOLOR`:
/// the slots the three register-argument setters store
/// (`0x3f5d32`, `0x3f5edb`, `0x3f5f27`).
const TSS_TEXCOORDINDEX: u32 = 0x1c;
const TSS_BORDERCOLOR: u32 = 0x1d;
const TSS_COLORKEYCOLOR: u32 = 0x1e;

/// `ZBias`'s derived slots (`0x3f59b0`): `ZSLOPESCALE`, `ZOFFSET` and the
/// point, wireframe and solid offset enables.
const ZSLOPESCALE: u32 = 77;
const ZOFFSET: u32 = 78;
const OFFSET_ENABLES: [u32; 3] = [79, 80, 81];
/// `ZBias`'s own slot.
const ZBIAS: u32 = 149;
/// The two floats `ZBias` reads from the image: 2^32 (added to a negative
/// `fild`, so the bias converts as unsigned) and the slope scale factor.
const ZBIAS_UNSIGNED: u32 = 0x0045_dbb8;
const ZBIAS_SLOPE: u32 = 0x0045_dc20;

/// `stdcall(Value); ret 4` setters: entry and the guest slot its body
/// stores the raw value in (the `mov [0x4076..], reg` of each body).
pub const RENDER_STATE_SETTERS: &[(u32, u32)] = &[
    (0x003f_52f0, 151), // EdgeAntiAlias
    (0x003f_5330, 156), // ShadowFunc
    (0x003f_5370, 138), // FogColor
    (0x003f_53d0, 147), // CullMode
    (0x003f_5450, 146), // FrontFace
    (0x003f_54a0, 142), // NormalizeNormals
    (0x003f_54f0, 143), // ZEnable
    (0x003f_5590, 144), // StencilEnable
    (0x003f_5640, 145), // StencilFail
    (0x003f_56c0, 148), // TextureFactor
    (0x003f_5730, 157), // LineWidth
    (0x003f_57a0, 159), // Dxt1NoiseEnable
    (0x003f_5830, 160), // YuvEnable
    (0x003f_5870, 161), // OcclusionCullEnable
    (0x003f_58e0, 162), // StencilCullEnable
    (0x003f_5950, 163), // RopZCmpAlwaysRead
    (0x003f_5970, 164), // RopZRead
    (0x003f_5990, 165), // DoNotCullUncompressed
    (0x003f_5a60, 154), // MultiSampleMode
    (0x003f_5a90, 155), // MultiSampleRenderTargetMode
    (0x003f_5ac0, 152), // MultiSampleAntiAlias
    (0x003f_5b00, 153), // MultiSampleMask
    (0x003f_5b20, 158), // SampleAlpha
    (0x003f_5b40, 150), // LogicOp
    (0x003f_5ba0, 139), // FillMode
    (0x003f_5bf0, 140), // BackFillMode
    (0x003f_5c10, 141), // TwoSidedLighting
    (0x003f_5c70, 137), // VertexBlend
    (0x003f_5cc0, 136), // PSTextureModes
];

/// A setter's store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setter {
    /// `stdcall(Value); ret 4` into one render-state slot.
    RenderState { slot: u32 },
    /// `SetRenderState_ZBias`: its slot and the five it derives.
    ZBias,
    /// `SetRenderState_Simple` (ECX = push header, EDX = value; `ret`):
    /// a push and nothing else. Game code stores the slot itself, before
    /// or after the call.
    Simple,
    /// `SetTextureState_TexCoordIndex`: ESI = stage, `[esp+4]` = value; `ret 4`.
    TexCoordIndex,
    /// `SetTextureState_BumpEnv`: EAX = stage, stack (Type, Value); `ret 8`.
    BumpEnv,
    /// `SetTextureState_BorderColor`: EAX = stage, EBX = value; `ret`.
    BorderColor,
    /// `SetTextureState_ColorKeyColor`: EAX = stage, EBX = value; `ret`.
    ColorKeyColor,
}

/// The setter at `entry`, if it is one.
pub fn setter(entry: u32) -> Option<Setter> {
    if let Some(&(_, slot)) = RENDER_STATE_SETTERS.iter().find(|s| s.0 == entry) {
        return Some(Setter::RenderState { slot });
    }
    Some(match entry {
        0x003f_59b0 => Setter::ZBias,
        0x003f_5cf0 => Setter::Simple,
        0x003f_5d20 => Setter::TexCoordIndex,
        0x003f_5e30 => Setter::BumpEnv,
        0x003f_5ea0 => Setter::BorderColor,
        0x003f_5ef0 => Setter::ColorKeyColor,
        _ => return None,
    })
}

/// The address of render-state slot `slot`.
pub fn render_state(slot: u32) -> u32 {
    RENDER_STATE + 4 * slot
}

/// The address of texture state `state` of `stage`.
pub fn texture_state(stage: u32, state: u32) -> u32 {
    TEXTURE_STATE.wrapping_add(STAGE_STRIDE.wrapping_mul(stage)).wrapping_add(4 * state)
}

/// Stores what `s` stores, with the CPU at the entry; returns the bytes
/// the entry pops.
pub fn set(s: Setter, cpu: &CpuAbi, mem: &mut Guest) -> Result<u32, GuestFault> {
    let arg = |mem: &Guest, n: u32| mem.u32(cpu.esp.wrapping_add(4 + 4 * n));
    match s {
        Setter::RenderState { slot } => {
            let value = arg(mem, 0)?;
            mem.set_u32(render_state(slot), value)?;
            Ok(4)
        }
        Setter::ZBias => {
            let bias = arg(mem, 0)?;
            zbias(bias, mem)?;
            Ok(4)
        }
        Setter::Simple => Ok(0),
        Setter::TexCoordIndex => {
            let value = arg(mem, 0)?;
            mem.set_u32(texture_state(cpu.esi, TSS_TEXCOORDINDEX), value)?;
            Ok(4)
        }
        Setter::BumpEnv => {
            let (kind, value) = (arg(mem, 0)?, arg(mem, 1)?);
            mem.set_u32(texture_state(cpu.eax, kind), value)?;
            Ok(8)
        }
        Setter::BorderColor => {
            mem.set_u32(texture_state(cpu.eax, TSS_BORDERCOLOR), cpu.ebx)?;
            Ok(0)
        }
        Setter::ColorKeyColor => {
            mem.set_u32(texture_state(cpu.eax, TSS_COLORKEYCOLOR), cpu.ebx)?;
            Ok(0)
        }
    }
}

/// `ZBias`'s stores (`0x3f59b6..0x3f5a4c`): the bias `fild`ed as signed,
/// plus 2^32 when negative, negated; `ZOFFSET` is that as a float,
/// `ZSLOPESCALE` that times the image's slope factor (both rounded once
/// to single, from the x87 value), the three enables `bias != 0`, and
/// `ZBIAS` the raw value.
fn zbias(bias: u32, mem: &mut Guest) -> Result<(), GuestFault> {
    let mut x = bias as i32 as f64;
    if (bias as i32) < 0 {
        x += f32::from_bits(mem.u32(ZBIAS_UNSIGNED)?) as f64;
    }
    let offset = -x;
    let slope = offset * f32::from_bits(mem.u32(ZBIAS_SLOPE)?) as f64;
    mem.set_u32(render_state(ZSLOPESCALE), (slope as f32).to_bits())?;
    mem.set_u32(render_state(ZOFFSET), (offset as f32).to_bits())?;
    for slot in OFFSET_ENABLES {
        mem.set_u32(render_state(slot), (bias != 0) as u32)?;
    }
    mem.set_u32(render_state(ZBIAS), bias)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const STACK: u32 = 0x0001_0000;

    fn guest() -> Guest {
        let mut mem = Guest::new();
        mem.map_zeroed(0x0040_4000, 0x5000, Permissions::RW).unwrap();
        mem.map_zeroed(0x0045_d000, 0x1000, Permissions::RW).unwrap();
        mem.map_zeroed(STACK, 0x1000, Permissions::RW).unwrap();
        mem.set_u32(ZBIAS_UNSIGNED, 4_294_967_296.0f32.to_bits()).unwrap();
        mem.set_u32(ZBIAS_SLOPE, 0.25f32.to_bits()).unwrap();
        mem
    }

    fn cpu_with_args(mem: &mut Guest, args: &[u32]) -> CpuAbi {
        let esp = STACK + 0x800;
        mem.set_u32(esp, 0x0001_2345).unwrap();
        for (i, a) in args.iter().enumerate() {
            mem.set_u32(esp + 4 + 4 * i as u32, *a).unwrap();
        }
        CpuAbi { esp, ..CpuAbi::default() }
    }

    #[test]
    fn the_slots_are_the_bodies_stores() {
        // Each body's own store address (`mov [addr], reg`), read from the
        // disassembly: slot n is at 0x407498 + 4n.
        for (entry, addr) in [
            (0x003f_52f0, 0x0040_76f4),
            (0x003f_5330, 0x0040_7708),
            (0x003f_5370, 0x0040_76c0),
            (0x003f_53d0, 0x0040_76e4),
            (0x003f_54f0, 0x0040_76d4),
            (0x003f_5990, 0x0040_772c),
            (0x003f_5c70, 0x0040_76bc),
            (0x003f_5cc0, 0x0040_76b8),
        ] {
            let Some(Setter::RenderState { slot }) = setter(entry) else { panic!("{entry:#x}") };
            assert_eq!(render_state(slot), addr, "{entry:#x}");
        }
        let mut slots: Vec<u32> = RENDER_STATE_SETTERS.iter().map(|s| s.1).collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), RENDER_STATE_SETTERS.len());
    }

    #[test]
    fn a_render_state_setter_stores_its_value_and_pops_it() {
        let mut mem = guest();
        let cpu = cpu_with_args(&mut mem, &[3]);
        let cull = setter(0x003f_53d0).unwrap();
        assert_eq!(set(cull, &cpu, &mut mem).unwrap(), 4);
        assert_eq!(mem.u32(0x0040_76e4).unwrap(), 3);
    }

    #[test]
    fn zbias_derives_the_offset_slots() {
        let mut mem = guest();
        let cpu = cpu_with_args(&mut mem, &[8]);
        assert_eq!(set(Setter::ZBias, &cpu, &mut mem).unwrap(), 4);
        assert_eq!(f32::from_bits(mem.u32(render_state(ZSLOPESCALE)).unwrap()), -2.0);
        assert_eq!(f32::from_bits(mem.u32(render_state(ZOFFSET)).unwrap()), -8.0);
        for slot in OFFSET_ENABLES {
            assert_eq!(mem.u32(render_state(slot)).unwrap(), 1);
        }
        assert_eq!(mem.u32(render_state(ZBIAS)).unwrap(), 8);

        let cpu = cpu_with_args(&mut mem, &[0]);
        set(Setter::ZBias, &cpu, &mut mem).unwrap();
        assert_eq!(mem.u32(render_state(81)).unwrap(), 0);
        assert_eq!(f32::from_bits(mem.u32(render_state(ZOFFSET)).unwrap()), 0.0);

        // A negative fild converts as unsigned.
        let cpu = cpu_with_args(&mut mem, &[u32::MAX]);
        set(Setter::ZBias, &cpu, &mut mem).unwrap();
        assert_eq!(f32::from_bits(mem.u32(render_state(ZOFFSET)).unwrap()), -(u32::MAX as f32));
    }

    #[test]
    fn simple_stores_nothing() {
        let mut mem = guest();
        let cpu = CpuAbi { ecx: 0x0004_0300, edx: 1, ..cpu_with_args(&mut mem, &[]) };
        assert_eq!(set(Setter::Simple, &cpu, &mut mem).unwrap(), 0);
        let mut bytes = vec![0u8; 0x5000];
        mem.read_bytes(0x0040_4000, &mut bytes, 1).unwrap();
        assert!(bytes.iter().all(|b| *b == 0));
    }

    #[test]
    fn texture_state_setters_store_per_stage() {
        let mut mem = guest();
        let cpu = CpuAbi { esi: 2, ..cpu_with_args(&mut mem, &[0x0001_0001]) };
        assert_eq!(set(Setter::TexCoordIndex, &cpu, &mut mem).unwrap(), 4);
        assert_eq!(mem.u32(0x0040_4e40 + 2 * 0x80).unwrap(), 0x0001_0001);

        // BumpEnv: [edi*4 + 0x404dd0] with edi = stage * 32 + Type.
        let cpu = CpuAbi { eax: 1, ..cpu_with_args(&mut mem, &[22, 0x3f80_0000]) };
        assert_eq!(set(Setter::BumpEnv, &cpu, &mut mem).unwrap(), 8);
        assert_eq!(mem.u32((32 + 22) * 4 + 0x0040_4dd0).unwrap(), 0x3f80_0000);

        let cpu = CpuAbi { eax: 3, ebx: 0xff00_00ff, ..cpu_with_args(&mut mem, &[]) };
        assert_eq!(set(Setter::BorderColor, &cpu, &mut mem).unwrap(), 0);
        assert_eq!(mem.u32(3 * 0x80 + 0x0040_4e44).unwrap(), 0xff00_00ff);
        assert_eq!(set(Setter::ColorKeyColor, &cpu, &mut mem).unwrap(), 0);
        assert_eq!(mem.u32(3 * 0x80 + 0x0040_4e48).unwrap(), 0xff00_00ff);
    }
}
