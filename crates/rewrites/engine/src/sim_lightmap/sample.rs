//! The lightmap sample `0x000d1e10` (stdcall, `ret 0x10`) and every
//! function under it, as plain Rust over a [`View`]
//! (docs/engine/sim-render-split.md, "6. Render scratch and stamps", the
//! effect tint). Its one caller here is `0x000d2a50` (`0x000d2b6d`), which
//! passes the collision result of the ray `0x0014af40`.
//!
//! Arguments, as pushed: the collision result, the 0x14-byte output
//! (`+0` a vector, `+0xc` and `+0x10` two packed colours), the texture
//! cache flags (bit 1 asks the cache to load: only cache requests read
//! it), and a byte that asks for the defaults on an unlit mesh.
//!
//! | Path | Guest | Here |
//! |---|---|---|
//! | Structure (`[hit+8]` is -1) | `0x000d1e35`..`0x000d21a6` | [`Run::structure`] |
//! | Object | `0x000d21a9`..`0x000d2a28` | [`Run::object`] |
//! | The block and its section | `0x000d1d70` | [`geometry::block`] |
//! | Lightmap UV, texcoord, normal, vertex colour | `0x000d2bf0`, `0x000d2dc0`, `0x000d31c0`, `0x000d33a0` | [`geometry`] |
//! | Instance vertex colour | `0x000d35a0`, `0x000d3630` | [`geometry::instance_stream`], [`geometry::instance_colour`] |
//! | Lightmap page, base map | `0x000d2f90`, `0x000d30d0` | [`texture::lightmap`], [`texture::sample`] |
//! | Texel read and decode | `0x001362b0`, `0x00136140`, `0x000358d0`, `0x00135ca0`, DXT `0x00223ed0`, `0x00224040`, `0x00224080` | [`texture`] |
//! | Colour pack and unpack | `0x00131ed0`, `0x00131e50`, `0x00131bb0` | [`pack3`], [`unpack`] |
//!
//! Render-owned reads, and what answers them here:
//!
//! | Guest | Here |
//! |---|---|
//! | `0x0012de70` (flags 3) and the runtime section `[info+0x28]` (`0x000d1d70`, `0x000d35a0`), and `[0x004e6494]` in `0x0016e290`, which asks again with flags 0 | [`Source::section`] |
//! | The texture cache's gets (`0x000d15e0`, `0x000d1680`, `0x000d1630`, `0x0001cfb0`, `0x00012360`, through `0x0012ccf0`/`0x0012ce00`, with their stamps of the image's `+0x50`/`+0x70` and the cache's slot) and the lock `0x0001d2f0` | [`Source::pixels`]: `Some` is a lock that succeeds, `None` a texture not resident ([`texture::resolve`]) |
//! | The PRT table `0x004b89b0`, `[0x005093e8]` entries (`0x00024490`) | [`Source::prt_entries`]: with none the lookup takes its not-found path; with any the port stops ([`PRT`]) |
//! | The palette table `0x00484dd4`, `[0x005093ac]` entries, that `0x00015b70` builds at a BSP load | rebuilt from the same tag reads ([`texture::palette_colour`]) |
//!
//! The x87 parts run on the `x87` crate under [`CONTROL`], the game's
//! control word (53-bit precision, round to nearest, all masked); the SSE
//! scalar parts are `f32` operations in the guest's order, which equal
//! `mulss`/`addss` under MXCSR `0x1f80` for every non-NaN operand (a NaN's
//! payload can differ between hosts).
//!
//! Stores into the call tree's own stack are not output; [`Sampled::writes`]
//! has every other store, in order: the output, the instance stream's
//! offset (`0x000d3605`, into the section) and the DXT read's
//! out-of-image flag (`0x001361de`, a byte, given as its word).
use super::view::View;
use crate::colour_pass::{fistp, pack_colour};
use crate::light_constants::{Error, X87};
use crate::sim_native::Memory;
use runtime::GuestFault;

mod geometry;
mod texture;
#[cfg(test)]
mod tests;

/// The game's x87 control word: every exception masked, 53-bit precision,
/// round to nearest.
pub const CONTROL: u16 = 0x027f;

/// The tag instance table: `[TAGS] + (datum & 0xffff) * 16 + 8` is a tag's
/// address.
pub const TAGS: u32 = 0x004e_3b44;
/// The active lightmap tag (`ltmp`).
pub const LIGHTMAP: u32 = 0x004e_0344;
/// The active structure BSP tag.
pub const BSP: u32 = 0x004e_0348;
/// Pointers to the three default vectors the body copies first
/// (`0x000d1f51`..`0x000d1fa0`): the normal `(0, 0, 1)`, the base colour
/// and the light colour.
pub const DEFAULT_NORMAL: u32 = 0x0046_87b0;
pub const DEFAULT_ALBEDO: u32 = 0x0046_8734;
pub const DEFAULT_LIGHT: u32 = 0x0046_872c;
/// Where the port stops when the PRT table has entries: `0x00024490`'s
/// hit leads to `0x000241c0`, not ported.
pub const PRT: u32 = 0x0002_4490;

const NONE: u32 = u32::MAX;
const ONE: u32 = 0x3f80_0000;
const HALF: u32 = 0x3f00_0000;
/// `[0x0045dbcc]`, the default vector's z on an unlit mesh.
const MINUS_ONE: u32 = 0xbf80_0000;
/// `[0x0045dc38]`.
const SCALE_255: u32 = 0x437f_0000;
/// `[0x0045dbb4]`, 1/255.
const INV_255: u32 = 0x3b80_8081;
/// The unlit mesh's two colours (`0x000d1e86`).
const UNLIT: u32 = 0xff40_4040;
/// The object path's base colour when its map cannot be read (`0x000d2a32`).
const NO_BASE: u32 = 0xffc0_c0d0;

/// EAX and every store outside the call tree's stack, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sampled {
    pub eax: u32,
    pub writes: Vec<(u32, [u8; 4])>,
}

/// What the sample reads. [`View`] is the one the simulation uses; the
/// tests stand a flat fixture in for it.
pub trait Source {
    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), GuestFault>;
    fn section(&self, info: u32) -> Result<Option<u32>, GuestFault>;
    fn pixels(&self, image: u32) -> Result<Option<u32>, GuestFault>;
    fn prt_entries(&self) -> u32;
}

impl<M: Memory> Source for View<'_, M> {
    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), GuestFault> {
        self.read_bytes(addr, dst, 1)
    }
    fn section(&self, info: u32) -> Result<Option<u32>, GuestFault> {
        View::section(self, info)
    }
    fn pixels(&self, image: u32) -> Result<Option<u32>, GuestFault> {
        View::pixels(self, image)
    }
    fn prt_entries(&self) -> u32 {
        View::prt_entries(self)
    }
}

/// `0x000d1e10(args)`.
pub fn sample(v: &View<'_, impl Memory>, args: [u32; 4]) -> Result<Sampled, GuestFault> {
    run(v, args)
}

/// `0x000d1e10(args)` over any [`Source`] (the differential test's
/// fixture stands in for a [`View`]).
pub fn run(src: &impl Source, args: [u32; 4]) -> Result<Sampled, GuestFault> {
    let mut r = Run { src, writes: Vec::new() };
    let eax = r.body(args)?;
    Ok(Sampled { eax, writes: r.writes })
}

/// A path the port does not take (named by its guest address), as the
/// engine's other ports name one.
pub(crate) fn unported(pc: u32) -> GuestFault {
    GuestFault {
        addr: pc,
        len: 1,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

pub(crate) fn at(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

pub(crate) fn f(bits: u32) -> f32 {
    f32::from_bits(bits)
}

/// `lo` if `lo > v`, else `hi` if `v > hi`, else `v` (`comiss`, `jbe`):
/// every clamp in the tree has this shape.
pub(crate) fn clamp(v: u32, lo: u32, hi: u32) -> u32 {
    if f(lo) > f(v) {
        lo
    } else if f(v) > f(hi) {
        hi
    } else {
        v
    }
}

fn clamp01(v: [u32; 3]) -> [u32; 3] {
    v.map(|c| clamp(c, 0, ONE))
}

/// `((c - a) * v + (b - a) * u) + a`, the barycentric blend every
/// interpolation in the tree makes (its two products are added in
/// either order, which IEEE addition does not see).
pub(crate) fn blend(a: f32, b: f32, c: f32, u: f32, v: f32) -> f32 {
    ((c - a) * v + (b - a) * u) + a
}

fn fpu(pc: u32) -> impl Fn(Error) -> GuestFault {
    move |_| unported(pc)
}

/// `0x00131ed0`: x, y, z each times 255 on the x87, `fistp`'d under
/// [`CONTROL`], packed `x << 16 | y << 8 | z` with each lane masked to a
/// byte and no alpha.
pub(crate) fn pack3(v: [u32; 3]) -> Result<u32, GuestFault> {
    let e = fpu(0x0013_1ed0);
    let mut x = X87::new(CONTROL);
    for lane in v {
        x.fld(lane).map_err(&e)?;
    }
    x.fld(SCALE_255).map_err(&e)?;
    let mul = x87::abi::arith_op::MUL;
    x.arith_reg(mul, 3, 0, false).map_err(&e)?;
    x.arith_reg(mul, 2, 0, false).map_err(&e)?;
    x.arith_reg(mul, 1, 0, true).map_err(&e)?;
    let z = fistp(&mut x).map_err(&e)? & 0xff;
    let y = fistp(&mut x).map_err(&e)? & 0xff;
    let xx = fistp(&mut x).map_err(&e)? & 0xff;
    Ok(z | y << 8 | xx << 16)
}

/// `0x00131e50`: four lanes, alpha first, packed with no masking.
pub(crate) fn pack4(v: [u32; 4]) -> Result<u32, GuestFault> {
    pack_colour(v, SCALE_255, CONTROL).map_err(fpu(0x0013_1e50))
}

/// `0x00131bb0`: bits 16..23, 8..15 and 0..7 of `c`, each `fild`'d, times
/// `[0x0045dbb4]` (1/255) and stored as f32.
pub(crate) fn unpack(c: u32) -> Result<[u32; 3], GuestFault> {
    let e = fpu(0x0013_1bb0);
    let mut out = [0; 3];
    for (lane, shift) in out.iter_mut().zip([16, 8, 0]) {
        let mut x = X87::new(CONTROL);
        x.fild(((c >> shift) & 0xff) as i32).map_err(&e)?;
        x.fmul_m(INV_255).map_err(&e)?;
        *lane = x.fstp_m32().map_err(&e)?;
    }
    Ok(out)
}

/// `fld dword; fistp dword` under [`CONTROL`].
pub(crate) fn round(bits: u32, pc: u32) -> Result<i32, GuestFault> {
    let mut x = X87::new(CONTROL);
    x.fld(bits).map_err(fpu(pc))?;
    Ok(fistp(&mut x).map_err(fpu(pc))? as i32)
}

/// One sample's reads and stores. A read sees the stores made before it.
pub(crate) struct Run<'s, S: Source> {
    pub(crate) src: &'s S,
    pub(crate) writes: Vec<(u32, [u8; 4])>,
}

impl<S: Source> Run<'_, S> {
    fn bytes<const N: usize>(&self, a: u32) -> Result<[u8; N], GuestFault> {
        let mut b = [0; N];
        self.src.read(a, &mut b)?;
        for (w, v) in &self.writes {
            for (i, byte) in v.iter().enumerate() {
                let off = at(*w, i as u32).wrapping_sub(a) as usize;
                if off < N {
                    b[off] = *byte;
                }
            }
        }
        Ok(b)
    }
    pub(crate) fn u8(&self, a: u32) -> Result<u8, GuestFault> {
        Ok(self.bytes::<1>(a)?[0])
    }
    pub(crate) fn u16(&self, a: u32) -> Result<u16, GuestFault> {
        Ok(u16::from_le_bytes(self.bytes(a)?))
    }
    /// `movsx r32, word`.
    pub(crate) fn i16(&self, a: u32) -> Result<i32, GuestFault> {
        Ok(self.u16(a)? as i16 as i32)
    }
    pub(crate) fn u32(&self, a: u32) -> Result<u32, GuestFault> {
        Ok(u32::from_le_bytes(self.bytes(a)?))
    }
    pub(crate) fn f32(&self, a: u32) -> Result<f32, GuestFault> {
        Ok(f(self.u32(a)?))
    }
    fn vec3(&self, a: u32) -> Result<[u32; 3], GuestFault> {
        Ok([self.u32(a)?, self.u32(at(a, 4))?, self.u32(at(a, 8))?])
    }
    pub(crate) fn store(&mut self, a: u32, v: u32) {
        self.writes.push((a, v.to_le_bytes()));
    }
    /// A tag's address from its datum (`[TAGS] + (datum & 0xffff) * 16 + 8`).
    pub(crate) fn tag(&self, datum: u32) -> Result<u32, GuestFault> {
        self.u32(at(self.u32(TAGS)?, ((datum & 0xffff) << 4) + 8))
    }

    /// `flags` reaches only the texture cache's requests (`0x000d2fcd`,
    /// `0x000d30fe`), which the store answers without them.
    fn body(&mut self, [hit, out, _flags, defaults]: [u32; 4]) -> Result<u32, GuestFault> {
        if self.u32(at(hit, 8))? == NONE {
            self.structure(hit, out, defaults as u8 != 0)
        } else {
            self.object(hit, out)
        }
    }

    /// `0x000d1e35`..`0x000d21a6`: a hit on the structure (a cluster, or
    /// an instance of the BSP's geometry).
    fn structure(&mut self, hit: u32, out: u32, defaults: bool) -> Result<u32, GuestFault> {
        // 0x000d1e44: no block, or one not resident: 2, nothing stored.
        let Some(section) = geometry::block(self, hit)? else {
            return Ok(2);
        };
        let mesh = at(self.u32(at(section, 4))?, self.u32(at(hit, 0x20))?.wrapping_mul(0x48));
        // 0x000d1e68: 0x0013d9f0 is true for mesh types 4 and 5.
        if matches!(self.u16(mesh)?, 4 | 5) {
            if defaults {
                self.store(out, 0);
                self.store(at(out, 4), 0);
                self.store(at(out, 0xc), UNLIT);
                self.store(at(out, 0x10), UNLIT);
                self.store(at(out, 8), MINUS_ONE);
            }
            return Ok(0);
        }
        let group = self.u32(at(self.u32(LIGHTMAP)?, 0x84))?;
        let instance = self.u32(at(hit, 4))?;
        let bsp = self.u32(BSP)?;
        // 0x000d1ece..0x000d1f2a: the page and palette from the cluster's
        // or the instance's render info; an instance whose definition has
        // `+0x56` set is lit by its own vertex colours instead.
        let mut lit_by_vertex = false;
        let (mut page, mut palette) = (-1, -1);
        let info = if instance != NONE {
            let instances = self.u32(at(bsp, 0x144))?;
            if self.u16(at(at(instances, instance.wrapping_mul(0x58)), 0x56))? != 0 {
                lit_by_vertex = true;
                None
            } else {
                Some(at(self.u32(at(group, 0x4c))?, instance.wrapping_mul(4)))
            }
        } else {
            Some(at(self.u32(at(group, 0x2c))?, self.u32(hit)?.wrapping_mul(4)))
        };
        if let Some(e) = info {
            palette = self.u8(at(e, 2))? as i8 as i32;
            page = self.i16(e)?;
        }
        // 0x000d1f2f: the mesh's shader's base map.
        let materials = self.u32(at(bsp, 0xa8))?;
        let material = at(materials, (self.i16(at(mesh, 4))? as u32) << 5);
        let base = texture::shader_bitmap(self, material)?;
        let mut normal = self.vec3(self.u32(DEFAULT_NORMAL)?)?;
        let mut albedo = self.vec3(self.u32(DEFAULT_ALBEDO)?)?;
        let mut light = self.vec3(self.u32(DEFAULT_LIGHT)?)?;
        let tri = self.u32(at(hit, 0x28))?;
        let (u, v) = (self.f32(at(hit, 0x30))?, self.f32(at(hit, 0x34))?);
        let result = if !lit_by_vertex {
            // 0x000d1fc1: the lightmap UV stays (0, 0) when the section
            // has no fourth stream.
            let (lu, lv) = geometry::lightmap_uv(self, section, tri, u, v)?.unwrap_or((0.0, 0.0));
            texture::lightmap(self, page, palette, lu, lv, &mut light)?
        } else {
            // 0x000d1ff3.
            let stream = geometry::instance_stream(self, group, instance)?;
            if stream != 0 {
                geometry::instance_colour(self, stream, tri, section, u, v, &mut light)?;
                0
            } else {
                1
            }
        };
        // 0x000d2043, 0x000d2065: the base map at the texcoord, grey when
        // it cannot be read.
        let (s, t) = geometry::texcoord(self, section, tri, u, v)?;
        if !texture::sample(self, base, s, t, &mut albedo)? {
            albedo = [HALF; 3];
        }
        // 0x000d209c.
        if let Some(n) = geometry::normal(self, section, tri, u, v)? {
            normal = n;
        }
        let albedo = clamp01(albedo);
        let light = clamp01(light);
        let a = pack3(albedo)?;
        self.store(at(out, 0xc), a);
        let l = pack3(light)?;
        self.store(at(out, 0x10), l);
        self.store(out, normal[0]);
        self.store(at(out, 4), normal[1]);
        self.store(at(out, 8), normal[2]);
        Ok(result)
    }

    /// `0x000d21a9`..`0x000d2a28`: a hit on an object's model section.
    fn object(&mut self, hit: u32, out: u32) -> Result<u32, GuestFault> {
        let part = self.u32(at(hit, 0x14))?;
        if part == NONE {
            return Ok(2);
        }
        let tag = self.tag(self.u32(at(hit, 0xc))?)?;
        // 0x000d21d3: the guest reads the model section's runtime pointer
        // `+0x34` as the tag holds it, with no residency call: the render
        // made the section resident when it drew the object. Here the
        // store answers for its block info `+0x38` (runtime offset -8, so
        // the same `+0x34`), as for any section; a section with no raw
        // block keeps the tag's pointer.
        let entry = at(self.u32(at(tag, 0x28))?, part.wrapping_mul(0x5c));
        let section = match self.src.section(at(entry, 0x38))? {
            Some(s) => s,
            None => self.u32(at(entry, 0x34))?,
        };
        let mut result = 2;
        if self.u32(at(tag, 0x74))? as i32 > 0 {
            // 0x000d21fd: the PRT lookup; with no entries it finds none.
            if self.src.prt_entries() > 0 {
                return Err(unported(PRT));
            }
            result = 1;
        }
        let tri = self.u32(at(hit, 0x28))?;
        let (u, v) = (self.f32(at(hit, 0x30))?, self.f32(at(hit, 0x34))?);
        // 0x000d2454, 0x000d246e.
        let lit = match geometry::vertex_colour(self, section, tri, u, v)? {
            Some(c) => geometry::normal(self, section, tri, u, v)?.map(|n| (c, n)),
            None => None,
        };
        if let Some((colour, normal)) = lit {
            let c = pack3(clamp01(colour))?;
            self.store(at(out, 0x10), c);
            self.store(out, normal[0]);
            self.store(at(out, 4), normal[1]);
            self.store(at(out, 8), normal[2]);
        } else {
            // 0x000d2909.
            self.store(at(out, 0x10), 0);
            self.store(at(out, 0xc), 0);
            let n = self.vec3(self.u32(DEFAULT_NORMAL)?)?;
            self.store(out, n[0]);
            self.store(at(out, 4), n[1]);
            self.store(at(out, 8), n[2]);
        }
        // 0x000d2947..0x000d297f.
        let (s, t) = geometry::texcoord(self, section, tri, u, v)?;
        let meshes = self.u32(at(section, 4))?;
        let mesh = self.u32(at(hit, 0x20))?;
        let shaders = self.u32(at(tag, 0x64))?;
        let material = at(shaders, (self.i16(at(at(meshes, mesh.wrapping_mul(0x48)), 4))? as u32) << 5);
        let base = texture::shader_bitmap(self, material)?;
        let mut colour = [0; 3];
        if texture::sample(self, base, s, t, &mut colour)? {
            let c = pack3(clamp01(colour))?;
            self.store(at(out, 0xc), c);
        } else {
            self.store(at(out, 0xc), NO_BASE);
        }
        Ok(result)
    }
}
