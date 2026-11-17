//! `ligh` (light): a 0xe4-byte root with four tag references and three
//! blocks of function references.
//!
//! Evidence (**V**, all 1,590 distinct disc records): root size 0xe4 (the
//! next-lowest block pointer is always root + 0xe4 where blocks are inline);
//! the blocks at `+0xc4`, `+0xcc`, `+0xd4` have stride 8, each element a data
//! reference whose bytes begin with a function header whose size matches its
//! type byte; references resolve to `bitm`, `lens`, `MGS2`, `shad`.
//!
//! The shape, colour and intensity scalars are named from three sources,
//! recorded per field below and in `docs/formats/effects.md` ("ligh"): the
//! disc's records (**V**), the engine's light update (**E**, docs/graphics/
//! e3-lighting.md "The light record"), and paired study tags from the Digsite
//! E3 tag set, whose fields sit in the same order (**D**; read outside the
//! repository, never copied). Everything else stays in [`Light::raw`].
use super::EffectTag;
use super::mem::{Ctx, Rec, TagRef};
use crate::Result;
use crate::wire::TagBlock;
use map::FourCC;

pub const LIGHT_SIZE: usize = 0xe4;

/// `ligh +0` flag bits named here. The engine's uses of these and of the
/// other bits are in `engine::render::light::definition_flags`.
pub mod flags {
    /// Bit 3: the light casts no shadow. **E** (`0x00031e02` skips the shadow
    /// passes); **D** (a no-shadow study tag differs from its pair in this
    /// bit alone); the MCC tools call it "no stencil shadow".
    pub const NO_SHADOW: u32 = 1 << 3;
}

/// The light's shape, `ligh +4` (u16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    /// 0: a point light with a radius ([`Light::radius`]). 1,349 of the
    /// disc's 1,596 records; their cone angle is the kit's 1-degree default.
    Sphere,
    /// 1: a parallel projection, the directional case (name **S**; two disc
    /// records, one tag, with a cone angle and range like a frustum's).
    Orthogonal,
    /// 2: a projected cone ([`Light::field_of_view`], range
    /// [`Light::cutoff_distance`]). 245 disc records; 8 also carry a radius.
    Frustum,
    /// 3: a pyramid projection (name **S**; not on the disc).
    Pyramid,
    /// Any other value, kept.
    Other(u16),
}

impl LightKind {
    pub fn from_raw(v: u16) -> Self {
        match v {
            0 => Self::Sphere,
            1 => Self::Orthogonal,
            2 => Self::Frustum,
            3 => Self::Pyramid,
            v => Self::Other(v),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sphere => "sphere",
            Self::Orthogonal => "orthogonal",
            Self::Frustum => "frustum",
            Self::Pyramid => "pyramid",
            Self::Other(_) => "other",
        }
    }
}

/// An RGB colour of three f32.
pub type Rgb = [f32; 3];

/// A light tag.
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    /// +0, u32 flag bits (disc: 0xa, 0x80a, 0x900a, 0x100a, 0x40008e, ...);
    /// see [`flags`].
    pub flags: u32,
    /// +4, u16: the shape.
    pub kind: LightKind,
    /// +0x18, f32 world units: a sphere light's radius (zero on 239 of the
    /// 247 cones, and on 125 spheres).
    pub radius: f32,
    /// +0x1c, f32: a second radius, equal to [`Self::radius`] on most
    /// sphere lights (the specular radius, **S**).
    pub specular_radius: f32,
    /// +0x20, f32: a cone's width at its apex.
    pub near_width: f32,
    /// +0x24, f32: a cone's height over its width (1 on most lights).
    pub height_stretch: f32,
    /// +0x28, f32 radians: a cone's full opening angle. The editing kit's
    /// tag files hold degrees; the cache holds radians (**V**: 1 degree on
    /// every sphere, the kit's default; 1 to 90 degrees on cones).
    pub field_of_view: f32,
    /// +0x2c, f32 world units: where a cone starts to fall off.
    pub falloff_distance: f32,
    /// +0x30, f32 world units: where a cone's light ends (its range).
    pub cutoff_distance: f32,
    /// +0x34, u32: how the colours below are interpolated by intensity
    /// (**E**: the "mode" the light update reads).
    pub interpolation: u32,
    /// +0x40 / +0x4c: the specular colour at the lower and upper intensity
    /// bound (**E**: interpolated into the light record's second colour).
    pub specular_colour: [Rgb; 2],
    /// +0x58 / +0x64: the diffuse colour at the lower and upper intensity
    /// bound (**E**: interpolated into the light record's first colour).
    pub diffuse_colour: [Rgb; 2],
    /// +0x70 / +0x74: the brightness (intensity) bounds.
    pub brightness: [f32; 2],
    /// +0xb0, f32 seconds: how long a timed light lasts; zero is untimed
    /// (**E**: the light record's timed flag is set when this is above zero).
    pub duration: f32,
    /// +0xb6, u16: the fade curve over [`Self::duration`] (**E**).
    pub duration_falloff: u16,
    /// +0x78, `bitm` (projected/gel texture, **S**).
    pub bitmap: TagRef,
    /// +0x90, `lens` (a lens-flare tag).
    pub lens_flare: TagRef,
    /// +0x9c, `MGS2` (referenced by the engine's part-creation switch at
    /// `0x0017a8db` as a creatable part type; its name is not established).
    pub mgs2: TagRef,
    /// +0xdc, `shad`.
    pub shader: TagRef,
    /// Function data of the block at +0xc4 (one `Vec` per element).
    pub functions_c4: Vec<Vec<u8>>,
    /// Function data of the block at +0xcc.
    pub functions_cc: Vec<Vec<u8>>,
    /// Function data of the block at +0xd4.
    pub functions_d4: Vec<Vec<u8>>,
    /// The whole root record.
    pub raw: Vec<u8>,
}

impl Light {
    /// Whether the definition forbids shadows ([`flags::NO_SHADOW`]).
    pub fn no_shadow(&self) -> bool {
        self.flags & flags::NO_SHADOW != 0
    }

    /// The colour at full intensity: the diffuse upper bound.
    pub fn colour(&self) -> Rgb {
        self.diffuse_colour[1]
    }

    /// How far the light reaches: a sphere's radius, otherwise the cone's
    /// cutoff distance.
    pub fn range(&self) -> f32 {
        match self.kind {
            LightKind::Sphere => self.radius,
            _ => self.cutoff_distance,
        }
    }
}

/// Read a block whose elements are 8-byte function data references.
/// Shared by `ligh`, `lens`.
pub fn function_refs(ctx: &mut Ctx<'_>, block: TagBlock) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for e in ctx.block(block, 8)? {
        let r = Rec::exact(&e, 8)?;
        out.push(ctx.data(r.data(0))?);
    }
    Ok(out)
}

impl EffectTag for Light {
    const GROUP: [u8; 4] = *b"ligh";
    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self> {
        let raw = ctx.mem.read(address, LIGHT_SIZE)?;
        let r = Rec::exact(&raw, LIGHT_SIZE)?;
        let rgb = |o: usize| [r.f32(o), r.f32(o + 4), r.f32(o + 8)];
        Ok(Self {
            flags: r.u32(0),
            kind: LightKind::from_raw(r.u16(4)),
            radius: r.f32(0x18),
            specular_radius: r.f32(0x1c),
            near_width: r.f32(0x20),
            height_stretch: r.f32(0x24),
            field_of_view: r.f32(0x28),
            falloff_distance: r.f32(0x2c),
            cutoff_distance: r.f32(0x30),
            interpolation: r.u32(0x34),
            specular_colour: [rgb(0x40), rgb(0x4c)],
            diffuse_colour: [rgb(0x58), rgb(0x64)],
            brightness: [r.f32(0x70), r.f32(0x74)],
            duration: r.f32(0xb0),
            duration_falloff: r.u16(0xb6),
            bitmap: r.tag_ref(0x78),
            lens_flare: r.tag_ref(0x90),
            mgs2: r.tag_ref(0x9c),
            shader: r.tag_ref(0xdc),
            functions_c4: function_refs(ctx, r.block(0xc4))?,
            functions_cc: function_refs(ctx, r.block(0xcc))?,
            functions_d4: function_refs(ctx, r.block(0xd4))?,
            raw,
        })
    }
    fn references(&self) -> Vec<(String, TagRef)> {
        vec![
            ("bitmap".into(), self.bitmap),
            ("lens_flare".into(), self.lens_flare),
            ("mgs2".into(), self.mgs2),
            ("shader".into(), self.shader),
        ]
    }

    fn functions(&self) -> Vec<(String, &[u8])> {
        let mut v = Vec::new();
        for (name, blk) in [
            ("c4", &self.functions_c4),
            ("cc", &self.functions_cc),
            ("d4", &self.functions_d4),
        ] {
            for (i, f) in blk.iter().enumerate() {
                v.push((format!("functions_{name}[{i}]"), f.as_slice()));
            }
        }
        v
    }
}

pub const REFERENCE_CLASSES: [(&str, FourCC); 4] = [
    ("bitmap", FourCC(*b"bitm")),
    ("lens_flare", FourCC(*b"lens")),
    ("mgs2", FourCC(*b"MGS2")),
    ("shader", FourCC(*b"shad")),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::SliceMemory;

    const BASE: u32 = 0x8010_0000;

    fn put(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn root() -> Vec<u8> {
        let mut b = vec![0u8; LIGHT_SIZE];
        put(&mut b, 0x78, u32::from_be_bytes(*b"bitm"));
        put(&mut b, 0x7c, u32::MAX);
        put(&mut b, 0x90, u32::from_be_bytes(*b"lens"));
        put(&mut b, 0x94, 0xe1000001);
        put(&mut b, 0x9c, u32::from_be_bytes(*b"MGS2"));
        put(&mut b, 0xa0, u32::MAX);
        put(&mut b, 0xdc, u32::from_be_bytes(*b"shad"));
        put(&mut b, 0xe0, u32::MAX);
        b
    }

    #[test]
    fn empty_blocks_and_references() {
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&root());
        let l = Light::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert!(l.functions_c4.is_empty() && l.functions_cc.is_empty());
        let refs = l.references();
        assert_eq!(refs.len(), 4);
        assert!(refs[0].1.is_null() && !refs[1].1.is_null());
    }

    #[test]
    fn aliased_function_block_outside_the_tag() {
        // The function block and its bytes live before the tag: dedup.
        let mut mem = SliceMemory::new(BASE);
        let func = mem.push(&[1, 0, 0, 0, 9, 9, 9, 9]);
        let mut elem = Vec::new();
        elem.extend(8u32.to_le_bytes());
        elem.extend(func.to_le_bytes());
        let blk = mem.push(&elem);
        let mut r = root();
        put(&mut r, 0xcc, 1);
        put(&mut r, 0xd0, blk);
        let at = mem.push(&r);
        let l = Light::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(l.functions_cc, vec![vec![1, 0, 0, 0, 9, 9, 9, 9]]);
    }

    #[test]
    fn out_of_bounds_block_and_budget_fail() {
        let mut mem = SliceMemory::new(BASE);
        let mut r = root();
        put(&mut r, 0xc4, 1);
        put(&mut r, 0xc8, 0x9000_0000);
        let at = mem.push(&r);
        assert!(Light::decode_at(&mut Ctx::new(&mut mem), at).is_err());
        let mut r = root();
        put(&mut r, 0xc4, 0x00ff_ffff);
        put(&mut r, 0xc8, BASE);
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&r);
        assert!(Light::decode_at(&mut Ctx::new(&mut mem), at).is_err());
    }

    fn putf(b: &mut [u8], o: usize, v: f32) {
        put(b, o, v.to_bits());
    }

    /// An invented cone light: every named field holds a distinct value.
    fn cone() -> Vec<u8> {
        let mut b = root();
        put(&mut b, 0, flags::NO_SHADOW | 0x2);
        b[4..6].copy_from_slice(&2u16.to_le_bytes());
        for (o, v) in [
            (0x20, 0.5),
            (0x24, 1.25),
            (0x28, 45.0),
            (0x2c, 3.0),
            (0x30, 9.0),
            (0x70, 0.25),
            (0x74, 0.75),
            (0xb0, 1.5),
        ] {
            putf(&mut b, o, v);
        }
        put(&mut b, 0x34, 1);
        for (i, o) in [0x40, 0x4c, 0x58, 0x64].into_iter().enumerate() {
            for k in 0..3 {
                putf(&mut b, o + 4 * k, (i * 3 + k) as f32 / 16.0);
            }
        }
        b[0xb6..0xb8].copy_from_slice(&3u16.to_le_bytes());
        b
    }

    fn decode(b: &[u8]) -> Light {
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(b);
        Light::decode_at(&mut Ctx::new(&mut mem), at).unwrap()
    }

    #[test]
    fn cone_fields_at_their_offsets() {
        let l = decode(&cone());
        assert_eq!(l.kind, LightKind::Frustum);
        assert!(l.no_shadow());
        assert_eq!(l.radius, 0.0);
        assert_eq!(
            [l.near_width, l.height_stretch, l.field_of_view],
            [0.5, 1.25, 45.0]
        );
        assert_eq!([l.falloff_distance, l.cutoff_distance], [3.0, 9.0]);
        assert_eq!(l.range(), 9.0);
        assert_eq!(l.interpolation, 1);
        let c = |i: usize| std::array::from_fn::<f32, 3, _>(|k| (i * 3 + k) as f32 / 16.0);
        assert_eq!(l.specular_colour, [c(0), c(1)]);
        assert_eq!(l.diffuse_colour, [c(2), c(3)]);
        assert_eq!(l.colour(), c(3));
        assert_eq!(l.brightness, [0.25, 0.75]);
        assert_eq!((l.duration, l.duration_falloff), (1.5, 3));
    }

    #[test]
    fn sphere_range_is_its_radius_and_shadow_is_bit_3_alone() {
        let mut b = root();
        putf(&mut b, 0x18, 6.0);
        putf(&mut b, 0x1c, 5.0);
        putf(&mut b, 0x30, 99.0);
        // Every bit but the no-shadow bit: the light may still cast.
        put(&mut b, 0, !flags::NO_SHADOW);
        let l = decode(&b);
        assert_eq!(l.kind, LightKind::Sphere);
        assert_eq!((l.radius, l.specular_radius, l.range()), (6.0, 5.0, 6.0));
        assert!(!l.no_shadow());
    }

    #[test]
    fn every_kind_value_round_trips() {
        for (v, k) in [
            (0, LightKind::Sphere),
            (1, LightKind::Orthogonal),
            (2, LightKind::Frustum),
            (3, LightKind::Pyramid),
            (7, LightKind::Other(7)),
        ] {
            let mut b = root();
            b[4..6].copy_from_slice(&(v as u16).to_le_bytes());
            assert_eq!(decode(&b).kind, k);
            assert_eq!(LightKind::from_raw(v), k);
        }
    }

    #[test]
    fn unknown_flags_preserved() {
        let mut r = root();
        put(&mut r, 0, 0xdead_beef);
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&r);
        let l = Light::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(l.flags, 0xdead_beef);
        assert_eq!(l.raw, r);
    }
}
