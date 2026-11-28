//! Read-only views of the guest's dynamic-light and scene-layer state.
//!
//! Phase 0 of the E3 lighting design (`docs/graphics/e3-lighting.md`, "Phase 0
//! findings"): what a host-side observer needs to read the live lights and
//! to tell the guest's draws apart by layer. Nothing here writes guest
//! memory, and nothing is wired into the runtime yet.
//!
//! - [`light`]: the engine's light datum array (`0x110`-byte records) and
//!   the `ligh` definition flag bits the render path tests.
//! - [`layer`]: the scene layer numbering shared by shader templates and the
//!   engine's layer loop, and the per-layer descriptor the loop reads.
//! - [`stencil`]: the engine's seven-mode stencil helper.
//!
//! Every offset is recovered from engine code; the doc gives the address of
//! each read and says which names are ours. Tests use invented records.

use crate::datum::DatumArray;
use crate::handle::DatumHandle;
use crate::memory::{GuestMemory, MemFault};

fn f32_at(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}
fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}
fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(b[off..off + 2].try_into().unwrap())
}
fn vec3_at(b: &[u8], off: usize) -> [f32; 3] {
    [f32_at(b, off), f32_at(b, off + 4), f32_at(b, off + 8)]
}

pub mod light_volume;

/// The light gather's typed model and its leaf ports
/// (docs/graphics/native-light-gather.md).
pub mod light_gather;

/// Per-caster inputs to the object-shadow projection.
pub mod object_shadow;

/// The object lighting cache's records (`0x0003dc90`, `0x0003ddd0`).
pub mod object_lighting;

/// The engine's dynamic lights.
pub mod light {
    use super::*;

    /// Guest global holding the light datum array's header address, stored
    /// by the array's initializer (`0x000bfffb`).
    pub const ARRAY_POINTER: u32 = 0x004e_030c;
    /// Element size (`0x000bffc4`, and `imul esi,110h` in every reader).
    pub const RECORD_SIZE: usize = 0x110;
    /// Capacity passed to the array initializer (`0x000bffc9`).
    pub const MAXIMUM_COUNT: i32 = 350;

    /// Record flag bits (`+0x02`, u16). Names are ours.
    pub mod flags {
        /// Set by both constructors.
        pub const LIVE: u16 = 0x0001;
        /// Set by the placement constructor (`0x000c05cd`), not by the effect
        /// part constructor.
        pub const PLACED: u16 = 0x0004;
        /// The definition has a positive duration (`ligh +0xb0`); the update
        /// fades the light over it (`0x000c0e53`).
        pub const TIMED: u16 = 0x0010;
        /// Copied from bit 2 of a placement byte (`0x000c05e6`); unread here.
        pub const PLACEMENT_BIT: u16 = 0x0020;
    }

    /// Field offsets inside a light record.
    pub mod layout {
        /// u16 datum salt.
        pub const SALT: usize = 0x00;
        /// u16, [`super::flags`].
        pub const FLAGS: usize = 0x02;
        /// u32 `ligh` tag datum.
        pub const DEFINITION: usize = 0x04;
        /// u32 game tick at creation.
        pub const CREATED_TICK: usize = 0x14;
        /// f32[3] bounding-sphere centre and f32 radius.
        pub const BOUNDS_CENTRE: usize = 0x18;
        pub const BOUNDS_RADIUS: usize = 0x24;
        /// u32 object whose functions drive the light, or none.
        pub const DRIVER_OBJECT: usize = 0x4c;
        /// u32 object the light is attached to, or none; u16 marker index.
        pub const PARENT_OBJECT: usize = 0x58;
        pub const PARENT_MARKER: usize = 0x5c;
        /// f32[3] each: frame relative to the parent marker.
        pub const LOCAL_POSITION: usize = 0x60;
        pub const LOCAL_FORWARD: usize = 0x6c;
        pub const LOCAL_UP: usize = 0x78;
        /// f32[3] each: world frame, refreshed per frame for attached lights.
        pub const POSITION: usize = 0x84;
        pub const TARGET: usize = 0x90;
        pub const FORWARD: usize = 0xac;
        pub const UP: usize = 0xb8;
        /// f32 scale passed to the constructor, and the current intensity.
        pub const BASE_SCALE: usize = 0xcc;
        pub const INTENSITY: usize = 0xd0;
        /// f32[3] each: the two colours the update clamps to `[0, 1]`.
        pub const COLOUR_A: usize = 0xd4;
        pub const COLOUR_B: usize = 0xe0;
        /// f32 shadow-related scalar: shadows need it above zero.
        pub const SHADOW_SCALAR: usize = 0xf8;
        /// f32 maximum weight copied to the visible-light row at 0x00031a17.
        pub const VISIBLE_WEIGHT: usize = 0x100;
    }

    /// One light record, decoded. Fields we have not interpreted stay in
    /// [`raw`](Self::raw).
    #[derive(Clone, Debug, PartialEq)]
    pub struct LightRecord {
        pub salt: u16,
        pub flags: u16,
        pub definition: DatumHandle,
        pub created_tick: u32,
        pub bounds_centre: [f32; 3],
        pub bounds_radius: f32,
        pub driver_object: DatumHandle,
        pub parent_object: DatumHandle,
        /// `None` when the stored marker word is `0xffff`.
        pub parent_marker: Option<u16>,
        pub local_position: [f32; 3],
        pub local_forward: [f32; 3],
        pub local_up: [f32; 3],
        pub position: [f32; 3],
        pub target: [f32; 3],
        pub forward: [f32; 3],
        pub up: [f32; 3],
        pub base_scale: f32,
        pub intensity: f32,
        pub colour_a: [f32; 3],
        pub colour_b: [f32; 3],
        pub shadow_scalar: f32,
        pub visible_weight: f32,
        pub raw: Box<[u8; RECORD_SIZE]>,
    }

    impl LightRecord {
        /// Decodes a record. Any bit pattern decodes; whether it is sane is
        /// [`check`](Self::check)'s business.
        pub fn parse(b: &[u8; RECORD_SIZE]) -> Self {
            use layout::*;
            let marker = u16_at(b, PARENT_MARKER);
            LightRecord {
                salt: u16_at(b, SALT),
                flags: u16_at(b, FLAGS),
                definition: DatumHandle(u32_at(b, DEFINITION)),
                created_tick: u32_at(b, CREATED_TICK),
                bounds_centre: vec3_at(b, BOUNDS_CENTRE),
                bounds_radius: f32_at(b, BOUNDS_RADIUS),
                driver_object: DatumHandle(u32_at(b, DRIVER_OBJECT)),
                parent_object: DatumHandle(u32_at(b, PARENT_OBJECT)),
                parent_marker: (marker != 0xffff).then_some(marker),
                local_position: vec3_at(b, LOCAL_POSITION),
                local_forward: vec3_at(b, LOCAL_FORWARD),
                local_up: vec3_at(b, LOCAL_UP),
                position: vec3_at(b, POSITION),
                target: vec3_at(b, TARGET),
                forward: vec3_at(b, FORWARD),
                up: vec3_at(b, UP),
                base_scale: f32_at(b, BASE_SCALE),
                intensity: f32_at(b, INTENSITY),
                colour_a: vec3_at(b, COLOUR_A),
                colour_b: vec3_at(b, COLOUR_B),
                shadow_scalar: f32_at(b, SHADOW_SCALAR),
                visible_weight: f32_at(b, VISIBLE_WEIGHT),
                raw: Box::new(*b),
            }
        }

        /// Whether the record is attached to an object.
        pub fn is_attached(&self) -> bool {
            !self.parent_object.is_none()
        }

        /// The invariants an observer relies on: a live record with a
        /// definition, finite frame and colours, colours in `[0, 1]` (the
        /// update clamps them) and `target == position + forward` (both
        /// constructors and the per-frame refresh write it that way). A
        /// record that fails is not used; the frame falls back to retail.
        pub fn check(&self) -> Result<(), LightError> {
            if self.salt == 0 || self.flags & flags::LIVE == 0 {
                return Err(LightError::NotLive);
            }
            if self.definition.is_none() {
                return Err(LightError::NoDefinition);
            }
            let vectors = [
                ("position", self.position),
                ("target", self.target),
                ("forward", self.forward),
                ("up", self.up),
                ("bounds centre", self.bounds_centre),
            ];
            for (name, v) in vectors {
                if v.iter().any(|c| !c.is_finite()) {
                    return Err(LightError::NonFinite(name));
                }
            }
            for (name, v) in [("colour a", self.colour_a), ("colour b", self.colour_b)] {
                if v.iter().any(|c| !(0.0..=1.0).contains(c)) {
                    return Err(LightError::ColourRange(name));
                }
            }
            for (name, v) in [
                ("intensity", self.intensity),
                ("bounds radius", self.bounds_radius),
            ] {
                if !v.is_finite() {
                    return Err(LightError::NonFinite(name));
                }
            }
            let sum: [f32; 3] = std::array::from_fn(|i| self.position[i] + self.forward[i]);
            if sum != self.target {
                return Err(LightError::TargetMismatch);
            }
            Ok(())
        }
    }

    /// Why a record was not usable.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum LightError {
        NotLive,
        NoDefinition,
        NonFinite(&'static str),
        ColourRange(&'static str),
        TargetMismatch,
        /// The array header does not describe a light array.
        BadArray {
            element_size: i32,
            maximum_count: i32,
        },
    }

    /// Reads the light a handle names, through the datum array whose header
    /// address is stored at [`ARRAY_POINTER`]. `Ok(None)` for a stale or
    /// absent handle. The header must have the light array's element size
    /// and capacity, so a wrong pointer is reported rather than misread.
    pub fn read(
        m: &dyn GuestMemory,
        handle: DatumHandle,
    ) -> Result<Option<Result<LightRecord, LightError>>, MemFault> {
        let array = DatumArray::at(m.u32(ARRAY_POINTER)?);
        let header = array.header(m)?;
        if header.element_size != RECORD_SIZE as i32 || header.maximum_count != MAXIMUM_COUNT {
            return Ok(Some(Err(LightError::BadArray {
                element_size: header.element_size,
                maximum_count: header.maximum_count,
            })));
        }
        let Some(addr) = array.try_get(m, handle)? else {
            return Ok(None);
        };
        let mut b = [0u8; RECORD_SIZE];
        m.read(addr, &mut b)?;
        Ok(Some(Ok(LightRecord::parse(&b))))
    }

    /// `ligh` definition flag bits (`ligh +0`, u32) that the light render
    /// path tests. Names are ours and describe the engine's use only.
    pub mod definition_flags {
        /// Set: the light never casts a shadow (`0x00031e02`). Clear, the
        /// shadow passes run when shadows are enabled globally and the
        /// record's [`SHADOW_SCALAR`](super::layout::SHADOW_SCALAR) is above
        /// zero and the posed volume has a nonzero shape code.
        pub const NO_SHADOW: u32 = 1 << 3;
        /// Tested with bit 18 before the frame is refreshed from the parent
        /// marker (`0x00031d3a`), and forces the shadow-apply layer in the
        /// per-light passes (`0x000312d4`).
        pub const BIT_5: u32 = 1 << 5;
        /// Set: the parent object's fade value does not dim the light
        /// (`0x000c0fe7`).
        pub const IGNORE_PARENT_FADE: u32 = 1 << 7;
        /// Set with bit 5: the frame is not refreshed from the marker.
        pub const BIT_18: u32 = 1 << 18;
        /// Preserves a same-owner light and exempts it from fade-out below
        /// the visibility budget (`0x0004adf0` and `0x0004ac30`).
        pub const BIT_20: u32 = 1 << 20;
        /// Adds a per-light pass (`0x00031ed6` -> `0x00031c20`) and widens
        /// the object gather (`0x00031e59`).
        pub const BIT_22: u32 = 1 << 22;

        /// Whether a light with these definition flags may cast a shadow.
        pub fn may_cast_shadow(flags: u32) -> bool {
            flags & NO_SHADOW == 0
        }
    }
}

/// The scene layer numbering.
pub mod layer {
    use super::*;

    /// Guest global holding the layer being drawn, set by the layer begin
    /// (`0x000443f4`) and cleared by the layer end (`0x0004464e`).
    pub const CURRENT_LAYER: u32 = 0x0046_7130;
    /// Guest address of the per-layer descriptor table, [`DESCRIPTOR_SIZE`]
    /// bytes per layer, indexed by layer number (`0x00044379`).
    pub const DESCRIPTOR_TABLE: u32 = 0x0043_fa1c;
    pub const DESCRIPTOR_SIZE: usize = 0x24;

    /// What a layer draws, as far as phase 0 recovered it. The numbers are
    /// the engine's and the shader templates' (`stem` layer index); the
    /// names are ours, from what the layer's passes do. See the doc for the
    /// evidence and confidence of each.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Role {
        /// Depth-writing base colour, drawn first.
        BaseDepth,
        /// Blended detail over the base, no depth write.
        BaseOverlay,
        /// Static lighting: lightmap and per-vertex lighting.
        Lightmap,
        /// Precomputed transfer lighting.
        Transfer,
        /// Water edge blend.
        WaterEdge,
        /// Additive emissive.
        SelfIllumination,
        /// Additive environment map.
        EnvironmentMap,
        /// Depth into a light's shadow buffer.
        ShadowGenerate,
        /// Shadow buffer applied to the scene.
        ShadowApply,
        /// A dynamic light's diffuse, additive.
        LightDiffuse,
        /// A dynamic light's specular, additive.
        LightSpecular,
        /// Blended transparent geometry.
        Transparent,
        /// Planar fog.
        Fog,
        /// Bloom source.
        Bloom,
        /// Active camouflage.
        ActiveCamo,
        /// The second active-camouflage layer.
        ActiveCamoModulate,
        /// Hologram.
        Hologram,
        /// A layer number with no recovered role.
        Unknown(u8),
    }

    impl Role {
        pub fn of(layer: u8) -> Role {
            use Role::*;
            match layer {
                1 => BaseDepth,
                2 => BaseOverlay,
                3 => Lightmap,
                5 => Transfer,
                6 => WaterEdge,
                7 => SelfIllumination,
                8 => EnvironmentMap,
                10 => ShadowGenerate,
                11 => ShadowApply,
                12 => LightDiffuse,
                13 => LightSpecular,
                15 => Transparent,
                16 => Fog,
                17 => Bloom,
                18 => ActiveCamo,
                19 => ActiveCamoModulate,
                21 => Hologram,
                n => Unknown(n),
            }
        }

        /// Draws the E3 view replaces: a dynamic light's own contribution.
        pub fn is_dynamic_light(self) -> bool {
            matches!(
                self,
                Role::ShadowGenerate | Role::ShadowApply | Role::LightDiffuse | Role::LightSpecular
            )
        }

        /// Opaque, depth-establishing or static-lighting draws: the layers
        /// before the dynamic lights, where the view's G-buffer comes from.
        pub fn is_opaque_base(self) -> bool {
            matches!(
                self,
                Role::BaseDepth | Role::BaseOverlay | Role::Lightmap | Role::Transfer
            )
        }
    }

    /// Which render list a layer draws from (descriptor `+0x1c`).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum List {
        /// The view's scene list (guest `0x004c1a48`).
        Scene,
        /// The current light's list (guest `0x004c1a60`).
        Light,
        /// The current light's shadow-caster list.
        ShadowCasters,
        Other(i32),
    }

    /// One layer descriptor. Field names are ours; offsets and uses are the
    /// layer begin's (`0x00044370`).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Descriptor {
        /// Render target id for the layer, `None` (stored -1) to keep the
        /// view's target (`+0x00`).
        pub target: Option<i32>,
        /// Clear flags for the layer's target, 0 for none (`+0x04`), and the
        /// colour, depth and stencil values cleared to (`+0x08`, `+0x0c`,
        /// `+0x10` byte).
        pub clear_flags: u32,
        pub clear_colour: u32,
        pub clear_depth: f32,
        pub clear_stencil: u8,
        /// Unread by the layer begin (`+0x14`).
        pub unknown_14: i32,
        /// Passed to the multisample helper `0x00015680` (`+0x18`, u16).
        pub multisample_mode: u16,
        /// Render list (`+0x1c`).
        pub list: List,
        /// Unread by the layer begin (`+0x20`).
        pub unknown_20: i32,
    }

    impl Descriptor {
        pub fn parse(b: &[u8; DESCRIPTOR_SIZE]) -> Self {
            let i = |o| u32_at(b, o) as i32;
            let target = i(0x00);
            Descriptor {
                target: (target != -1).then_some(target),
                clear_flags: u32_at(b, 0x04),
                clear_colour: u32_at(b, 0x08),
                clear_depth: f32_at(b, 0x0c),
                clear_stencil: b[0x10],
                unknown_14: i(0x14),
                multisample_mode: u16_at(b, 0x18),
                list: match i(0x1c) {
                    0 => List::Scene,
                    1 => List::Light,
                    2 => List::ShadowCasters,
                    n => List::Other(n),
                },
                unknown_20: i(0x20),
            }
        }

        /// Reads layer `layer`'s descriptor from guest memory.
        pub fn read(m: &dyn GuestMemory, layer: u8) -> Result<Self, MemFault> {
            let mut b = [0u8; DESCRIPTOR_SIZE];
            m.read(
                DESCRIPTOR_TABLE + layer as u32 * DESCRIPTOR_SIZE as u32,
                &mut b,
            )?;
            Ok(Self::parse(&b))
        }
    }

    /// Layers whose begin skips the layer when its bit is clear in the
    /// render list's layer mask (`test eax,0BFFDEEh` at `0x00044399`).
    pub const LIST_MASK_GATED: u32 = 0x00bf_fdee;

    /// Whether the layer begin consults the list's layer mask for `layer`.
    pub fn gated_by_list_mask(layer: u8) -> bool {
        layer < 32 && LIST_MASK_GATED & (1 << layer) != 0
    }
}

/// The engine's stencil helper, `0x00015370` (mode in BX, stored at
/// `0x00467010`).
pub mod stencil {
    /// `D3DCMP_*`, NV2A numbering.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Compare {
        Equal,
        NotEqual,
        Always,
    }

    /// `D3DSTENCILOP_*` the helper uses.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Op {
        Keep,
        Replace,
    }

    /// The state one mode sets. With `enable` false the helper writes
    /// nothing else, so the other fields are whatever was set before;
    /// [`for_mode`] reports them as `None`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct State {
        pub enable: bool,
        pub fail: Op,
        pub depth_fail: Op,
        pub pass: Op,
        pub func: Compare,
        pub reference: u8,
        pub mask: u8,
        pub write_mask: u8,
    }

    /// The state for a mode (`0x00015370`'s jump table; values are the
    /// D3D constants it pushes). `Some(None)` for mode 0, which only
    /// disables the test; `None` for a mode the helper ignores (above 6).
    ///
    /// Modes 1 and 2 write the reference (0 or 0xff) wherever a pixel is
    /// drawn; 3 to 6 draw only where bit 0 (3, 5), not bit 0 (4) or neither
    /// low bit (6) is set, 5 also setting bit 1. Only 0, 1 and 2 have direct
    /// callers in retail.
    pub fn for_mode(mode: u16) -> Option<Option<State>> {
        use Compare::*;
        use Op::*;
        let s = |pass, func, reference, mask, write_mask| {
            Some(Some(State {
                enable: true,
                fail: Keep,
                depth_fail: Keep,
                pass,
                func,
                reference,
                mask,
                write_mask,
            }))
        };
        match mode {
            0 => Some(None),
            1 => s(Replace, Always, 0x00, 0xff, 0xff),
            2 => s(Replace, Always, 0xff, 0xff, 0xff),
            3 => s(Keep, Equal, 0, 1, 0),
            4 => s(Keep, NotEqual, 0, 1, 0),
            5 => s(Replace, Equal, 2, 1, 2),
            6 => s(Keep, Equal, 0, 3, 0),
            _ => None,
        }
    }

    impl State {
        /// The stencil test as the hardware evaluates it:
        /// `(reference & mask) func (stored & mask)`.
        pub fn passes(&self, stored: u8) -> bool {
            let r = self.reference & self.mask;
            let s = stored & self.mask;
            match self.func {
                Compare::Equal => r == s,
                Compare::NotEqual => r != s,
                Compare::Always => true,
            }
        }

        /// The stored value after a pixel that passes depth and stencil.
        pub fn after_pass(&self, stored: u8) -> u8 {
            match self.pass {
                Op::Keep => stored,
                Op::Replace => (stored & !self.write_mask) | (self.reference & self.write_mask),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::layer::{self, Descriptor, List, Role};
    use super::light::{self, LightError, LightRecord, RECORD_SIZE, definition_flags as df};
    use super::stencil;
    use crate::datum::{DatumArray, layout as dl};
    use crate::handle::DatumHandle;
    use crate::memory::{FlatMemory, GuestMemory};

    fn put_f32s(b: &mut [u8], off: usize, v: &[f32]) {
        for (i, x) in v.iter().enumerate() {
            b[off + 4 * i..off + 4 * i + 4].copy_from_slice(&x.to_le_bytes());
        }
    }

    /// An invented, internally consistent light.
    fn record() -> [u8; RECORD_SIZE] {
        let mut b = [0u8; RECORD_SIZE];
        b[0..2].copy_from_slice(&0x1234u16.to_le_bytes());
        b[2..4].copy_from_slice(&(light::flags::LIVE | light::flags::TIMED).to_le_bytes());
        b[4..8].copy_from_slice(&0xabcd_0007u32.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&900u32.to_le_bytes());
        put_f32s(&mut b, 0x18, &[1.0, 2.0, 3.0, 4.5]);
        b[0x4c..0x50].copy_from_slice(&u32::MAX.to_le_bytes());
        b[0x58..0x5c].copy_from_slice(&u32::MAX.to_le_bytes());
        b[0x5c..0x5e].copy_from_slice(&0xffffu16.to_le_bytes());
        put_f32s(&mut b, 0x84, &[10.0, 20.0, 30.0]);
        put_f32s(&mut b, 0xac, &[0.0, 1.0, 0.0]);
        put_f32s(&mut b, 0x90, &[10.0, 21.0, 30.0]);
        put_f32s(&mut b, 0xb8, &[0.0, 0.0, 1.0]);
        put_f32s(&mut b, 0xcc, &[2.0, 0.5]);
        put_f32s(&mut b, 0xd4, &[1.0, 0.5, 0.25, 0.0, 0.125, 1.0]);
        put_f32s(&mut b, 0xf8, &[0.75]);
        put_f32s(&mut b, 0x100, &[0.625]);
        b
    }

    #[test]
    fn record_fields_come_from_their_offsets() {
        let r = LightRecord::parse(&record());
        assert_eq!(r.salt, 0x1234);
        assert_eq!(r.flags, 0x11);
        assert_eq!(r.definition, DatumHandle(0xabcd_0007));
        assert_eq!(r.created_tick, 900);
        assert_eq!(r.bounds_centre, [1.0, 2.0, 3.0]);
        assert_eq!(r.bounds_radius, 4.5);
        assert!(!r.is_attached());
        assert_eq!(r.parent_marker, None);
        assert_eq!(r.position, [10.0, 20.0, 30.0]);
        assert_eq!(r.forward, [0.0, 1.0, 0.0]);
        assert_eq!(r.target, [10.0, 21.0, 30.0]);
        assert_eq!(r.up, [0.0, 0.0, 1.0]);
        assert_eq!((r.base_scale, r.intensity), (2.0, 0.5));
        assert_eq!(r.colour_a, [1.0, 0.5, 0.25]);
        assert_eq!(r.colour_b, [0.0, 0.125, 1.0]);
        assert_eq!(r.shadow_scalar, 0.75);
        assert_eq!(r.visible_weight, 0.625);
        assert_eq!(r.check(), Ok(()));
    }

    #[test]
    fn attached_record_keeps_marker_and_local_frame() {
        let mut b = record();
        b[0x58..0x5c].copy_from_slice(&0x0042_0003u32.to_le_bytes());
        b[0x5c..0x5e].copy_from_slice(&7u16.to_le_bytes());
        put_f32s(&mut b, 0x60, &[0.5, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        let r = LightRecord::parse(&b);
        assert!(r.is_attached());
        assert_eq!(r.parent_object, DatumHandle(0x0042_0003));
        assert_eq!(r.parent_marker, Some(7));
        assert_eq!(r.local_position, [0.5, 0.0, 0.0]);
        assert_eq!(r.local_forward, [1.0, 0.0, 0.0]);
        assert_eq!(r.local_up, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn check_rejects_what_an_observer_must_not_use() {
        let bad = |f: &dyn Fn(&mut [u8; RECORD_SIZE])| {
            let mut b = record();
            f(&mut b);
            LightRecord::parse(&b).check()
        };
        assert_eq!(bad(&|b| b[0..2].fill(0)), Err(LightError::NotLive));
        assert_eq!(bad(&|b| b[2..4].fill(0)), Err(LightError::NotLive));
        assert_eq!(bad(&|b| b[4..8].fill(0xff)), Err(LightError::NoDefinition));
        assert_eq!(
            bad(&|b| put_f32s(b, 0x88, &[f32::NAN])),
            Err(LightError::NonFinite("position"))
        );
        assert_eq!(
            bad(&|b| put_f32s(b, 0xe0, &[1.5])),
            Err(LightError::ColourRange("colour b"))
        );
        assert_eq!(
            bad(&|b| put_f32s(b, 0x90, &[10.0, 20.0, 30.0])),
            Err(LightError::TargetMismatch)
        );
    }

    const ARRAY: u32 = 0x0010_0000;
    const DATA: u32 = 0x0010_0100;

    fn memory_with(element_size: i32, maximum_count: i32, salt: u16) -> FlatMemory {
        // One range covering the invented array and the pointer global.
        let mut m = FlatMemory::new(0x0010_0000, 0x0040_0000);
        m.set_u32(light::ARRAY_POINTER, ARRAY).unwrap();
        m.set_i32(ARRAY + dl::MAXIMUM_COUNT, maximum_count).unwrap();
        m.set_i32(ARRAY + dl::ELEMENT_SIZE, element_size).unwrap();
        m.set_i32(ARRAY + dl::HIGH_WATER, 3).unwrap();
        m.set_u32(ARRAY + dl::DATA, DATA).unwrap();
        let mut b = record();
        b[0..2].copy_from_slice(&salt.to_le_bytes());
        m.write(DATA + 2 * RECORD_SIZE as u32, &b).unwrap();
        m
    }

    #[test]
    fn read_follows_the_array_pointer_and_checks_the_salt() {
        let m = memory_with(RECORD_SIZE as i32, light::MAXIMUM_COUNT, 0x8001);
        assert_eq!(
            DatumArray::at(ARRAY).element_addr(&m, 2).unwrap(),
            DATA + 0x220
        );
        let got = light::read(&m, DatumHandle::new(0x8001, 2)).unwrap();
        let r = got.unwrap().unwrap();
        assert_eq!(r.salt, 0x8001);
        assert_eq!(r.position, [10.0, 20.0, 30.0]);
        // A stale salt, an index past the high-water mark, and none.
        assert_eq!(light::read(&m, DatumHandle::new(0x8002, 2)).unwrap(), None);
        assert_eq!(light::read(&m, DatumHandle::new(0x8001, 5)).unwrap(), None);
        assert_eq!(light::read(&m, DatumHandle::NONE).unwrap(), None);
    }

    #[test]
    fn read_refuses_an_array_that_is_not_the_light_array() {
        let m = memory_with(0x100, light::MAXIMUM_COUNT, 0x8001);
        assert_eq!(
            light::read(&m, DatumHandle::new(0x8001, 2)).unwrap(),
            Some(Err(LightError::BadArray {
                element_size: 0x100,
                maximum_count: 350
            }))
        );
    }

    #[test]
    fn definition_shadow_bit() {
        assert!(df::may_cast_shadow(0x0000_0002));
        assert!(!df::may_cast_shadow(0x0000_000a));
        assert!(df::may_cast_shadow(df::BIT_22 | df::BIT_5));
    }

    #[test]
    fn descriptor_fields_and_lists() {
        let mut b = [0u8; layer::DESCRIPTOR_SIZE];
        b[0x00..0x04].copy_from_slice(&(-1i32).to_le_bytes());
        b[0x04..0x08].copy_from_slice(&0x30u32.to_le_bytes());
        b[0x08..0x0c].copy_from_slice(&0x00ff_8040u32.to_le_bytes());
        b[0x0c..0x10].copy_from_slice(&0.25f32.to_le_bytes());
        b[0x10] = 9;
        b[0x14..0x18].copy_from_slice(&77i32.to_le_bytes());
        b[0x18..0x1a].copy_from_slice(&3u16.to_le_bytes());
        b[0x1c..0x20].copy_from_slice(&1i32.to_le_bytes());
        b[0x20..0x24].copy_from_slice(&5i32.to_le_bytes());
        let d = Descriptor::parse(&b);
        assert_eq!(d.target, None);
        assert_eq!(
            (
                d.clear_flags,
                d.clear_colour,
                d.clear_depth,
                d.clear_stencil
            ),
            (0x30, 0x00ff_8040, 0.25, 9)
        );
        assert_eq!((d.unknown_14, d.multisample_mode, d.unknown_20), (77, 3, 5));
        assert_eq!(d.list, List::Light);
        b[0x00..0x04].copy_from_slice(&4i32.to_le_bytes());
        b[0x1c..0x20].copy_from_slice(&2i32.to_le_bytes());
        let d = Descriptor::parse(&b);
        assert_eq!((d.target, d.list), (Some(4), List::ShadowCasters));
        b[0x1c..0x20].copy_from_slice(&9i32.to_le_bytes());
        assert_eq!(Descriptor::parse(&b).list, List::Other(9));
    }

    #[test]
    fn descriptor_read_indexes_the_table() {
        let mut m = FlatMemory::new(layer::DESCRIPTOR_TABLE, 0x400);
        let at = layer::DESCRIPTOR_TABLE + 12 * layer::DESCRIPTOR_SIZE as u32;
        m.set_i32(at, 6).unwrap();
        m.set_i32(at + 0x1c, 0).unwrap();
        let d = Descriptor::read(&m, 12).unwrap();
        assert_eq!((d.target, d.list), (Some(6), List::Scene));
    }

    #[test]
    fn roles_partition_the_light_and_base_layers() {
        let lights: Vec<u8> = (0..32)
            .filter(|&n| Role::of(n).is_dynamic_light())
            .collect();
        assert_eq!(lights, [10, 11, 12, 13]);
        let base: Vec<u8> = (0..32).filter(|&n| Role::of(n).is_opaque_base()).collect();
        assert_eq!(base, [1, 2, 3, 5]);
        assert_eq!(Role::of(16), Role::Fog);
        assert_eq!(Role::of(15), Role::Transparent);
        assert_eq!(Role::of(4), Role::Unknown(4));
        assert_eq!(Role::of(200), Role::Unknown(200));
    }

    #[test]
    fn list_mask_gate_bits() {
        assert!(layer::gated_by_list_mask(1));
        assert!(!layer::gated_by_list_mask(0));
        assert!(!layer::gated_by_list_mask(4));
        assert!(!layer::gated_by_list_mask(9));
        assert!(layer::gated_by_list_mask(10));
        assert!(!layer::gated_by_list_mask(22));
        assert!(!layer::gated_by_list_mask(40));
    }

    #[test]
    fn stencil_modes() {
        assert_eq!(stencil::for_mode(0), Some(None));
        assert_eq!(stencil::for_mode(7), None);
        let clear = stencil::for_mode(1).unwrap().unwrap();
        let mark = stencil::for_mode(2).unwrap().unwrap();
        assert!(clear.passes(0xff) && mark.passes(0));
        assert_eq!(clear.after_pass(0xff), 0);
        assert_eq!(mark.after_pass(0x00), 0xff);
        // Mode 3 draws where bit 0 is clear, mode 4 where it is set.
        let (m3, m4) = (
            stencil::for_mode(3).unwrap().unwrap(),
            stencil::for_mode(4).unwrap().unwrap(),
        );
        for s in 0..=255u8 {
            assert_eq!(m3.passes(s), s & 1 == 0);
            assert_eq!(m4.passes(s), s & 1 == 1);
            assert_eq!(m3.after_pass(s), s);
        }
        // Mode 5 sets bit 1 where bit 0 is clear, leaving other bits.
        let m5 = stencil::for_mode(5).unwrap().unwrap();
        assert!(m5.passes(0b100) && !m5.passes(0b101));
        assert_eq!(m5.after_pass(0b100), 0b110);
        // Mode 6 needs both low bits clear.
        let m6 = stencil::for_mode(6).unwrap().unwrap();
        assert!(m6.passes(0b100) && !m6.passes(0b10) && !m6.passes(0b1));
    }
}
