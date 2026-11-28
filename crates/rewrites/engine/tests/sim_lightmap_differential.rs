//! Contract D for the lightmap sample (docs/engine/sim-render-split.md,
//! "6. Render scratch and stamps"): `0x000d1e10` lifted and run on the IR
//! interpreter against `engine::sim_lightmap::sample::run`, over the same
//! random worlds, with every block the sample asks for resident.
//!
//! Each case builds a world from a seed: a structure hit (a cluster, or
//! an instance lit by the lightmap or by its vertices) or an object hit;
//! lightmap pages P8 or DXT, base maps of every format; sections, pages
//! and base maps resident or not. Both sides read the same bytes. The
//! render-owned reads are answered the way a resident cache answers
//! them:
//!
//! | Guest | Harness (an exit, answered and returned from) | Rust side |
//! |---|---|---|
//! | `0x0012de70` (ECX the block info) | AL 1 for a resident block, else 0; the section sits at `[info+0x28]` | [`Source::section`] from the same map |
//! | `0x000d15e0` (EAX the image) | the image itself (a non-null texture) when resident, else 0 | [`Source::pixels`] `Some(pixels)` |
//! | `0x000d1680` | 0: not in the loaded set, so the lock path | |
//! | `0x000d1630`, `0x0001cfb0`, `0x00012360` | nothing (cache stamps and requests) | |
//! | `0x0001d2f0(texture, record)` | the lock's record over the same pixels: zeroed, `'mtib'`, the image's size, depth 1, its format, flags 1 / 2 (DXT) / 4 (P8), one level, `+0x54` the pixels and `+0x34` from the guest's own `0x00136490`; AL 0 for a linear image or V8U8/G8B8 | |
//! | `[0x005093e8]` | 0 (no PRT entries) | [`Source::prt_entries`] 0 |
//! | The palette table `0x00484dd4`, `[0x005093ac]` | filled as `0x00015b70` fills it from the world's tags | rebuilt from the same tags |
//!
//! Addresses with the top bit set read the low copy on both sides (the
//! palette reads `0x00015ceb` make), as the Xbox's contiguous mirror does.
//!
//! The comparison is EAX and every byte of RAM outside the stack: the
//! guest's memory after the call must equal the world with the Rust
//! side's writes applied, word for word. Each path kind must be seen.
//!
//! Not covered: `0x000d1680` non-null after `0x000d15e0` (the guest then
//! reads the image's own `+0x54`, which no tag image sets), the PRT
//! entries path (`0x000241c0`), formats 17 and 19..21 through the lock
//! (the port stops on them; the direct path covers their decode).
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it sim_lightmap_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::{BTreeMap, HashMap};

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::sim_lightmap::sample::{self, Sampled, Source};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory, MemFault};
use runtime::GuestFault;
use runtime::guest::{Access, FaultReason};

const RAM: usize = 64 << 20;
const STACK_TOP: u32 = 0x03ff_0000;
/// The stack the call and its stubs use; not compared.
const STACK_LO: u32 = STACK_TOP - 0x2_0000;
const STEP_LIMIT: u64 = 200_000;

const SAMPLE: u32 = 0x000d_1e10;
/// Where the call returns to, and where a nested call does.
const RETURN: u32 = 0x0bad_0000;
const NESTED: u32 = 0x0bad_0010;

// The seams.
const RESIDENT: u32 = 0x0012_de70;
const TEXTURE: u32 = 0x000d_15e0;
const LOADED: u32 = 0x000d_1680;
const TOUCH: u32 = 0x000d_1630;
const REQUEST: u32 = 0x0001_cfb0;
const RELEASE: u32 = 0x0001_2360;
const LOCK: u32 = 0x0001_d2f0;
/// `0x00136490(EAX record)`: the record's bytes, run on the guest for the
/// lock's `+0x34`.
const IMAGE_SIZE: u32 = 0x0013_6490;
const EXITS: [u32; 8] = [RETURN, RESIDENT, TEXTURE, LOADED, TOUCH, REQUEST, RELEASE, LOCK];

// Globals the sample reads.
const TAGS: u32 = 0x004e_3b44;
const LIGHTMAP: u32 = 0x004e_0344;
const BSP: u32 = 0x004e_0348;
const SCENARIO: u32 = 0x004e_0350;
const BSP_INDEX: u32 = 0x0046_86c4;
const ASK_AGAIN: u32 = 0x004e_6494;
const PRT_COUNT: u32 = 0x0050_93e8;
const PALETTE_COUNT: u32 = 0x0050_93ac;
/// The palette table's first entry (`+4` of each 12-byte entry is read).
const PALETTE_TABLE: u32 = 0x0048_4dd0;
const DXT_OUTSIDE: u32 = 0x0055_e72c;
const DEFAULTS: [u32; 3] = [0x0046_87b0, 0x0046_8734, 0x0046_872c];

/// The world's own memory.
const WORLD: u32 = 0x0100_0000;
const WORLD_END: u32 = 0x0200_0000;
const TABLE: u32 = WORLD;
const NONE: u32 = u32::MAX;
/// Vertices a section has, triangles, meshes and materials.
const VERTS: u32 = 8;
const TRIS: u32 = 4;
const MATERIALS: u32 = 3;
const P8: u16 = 0x12;
/// What `0x0001d4b0` maps a swizzled texture's format back to itself for.
const LOCKABLE: [u16; 13] = [0, 1, 2, 3, 6, 8, 9, 10, 11, 14, 15, 16, P8];
const PAGE_FORMATS: [u16; 6] = [P8, P8, P8, 14, 15, 16];
const BASE_FORMATS: [u16; 13] = [14, 15, 16, 11, 10, 6, 8, 9, 0, 1, 2, 3, P8];
/// The direct path decodes these too.
const DIRECT_ONLY: [u16; 6] = [17, 19, 20, 21, 22, 23];
const SIDES: [u16; 9] = [0, 1, 2, 3, 4, 5, 8, 12, 16];
const PIXEL_BYTES: u32 = 0x2000;

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn u32(&mut self) -> u32 {
        self.next() as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.u32() % n
    }
    fn one_in(&mut self, n: u32) -> bool {
        self.below(n) == 0
    }
    fn pick<T: Copy>(&mut self, of: &[T]) -> T {
        of[self.below(of.len() as u32) as usize]
    }
    /// A finite float's bits, magnitude 2^-15..2^12, or 0: no NaN, whose
    /// payload can differ between the interpreter and the host.
    fn finite(&mut self) -> u32 {
        if self.one_in(16) {
            return 0;
        }
        (self.u32() & 0x807f_ffff) | ((0x70 + self.below(0x1b)) << 23)
    }
    /// [0, 1).
    fn unit(&mut self) -> f32 {
        (self.u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// The address a guest access lands on: the top bit's mirror folded down.
fn fold(addr: u32) -> u32 {
    if addr >= 0x8000_0000 && ((addr & 0x7fff_ffff) as usize) < RAM {
        addr & 0x7fff_ffff
    } else {
        addr
    }
}

/// The interpreter's memory: flat RAM with the mirror.
struct Ram(FlatMemory);

impl interp::Memory for Ram {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        interp::Memory::read(&mut self.0, fold(addr), buf)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        interp::Memory::write(&mut self.0, fold(addr), data)
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn put16(mem: &mut FlatMemory, a: u32, v: u16) {
    mem.bytes[a as usize..a as usize + 2].copy_from_slice(&v.to_le_bytes());
}

fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

fn half(mem: &FlatMemory, a: u32) -> u16 {
    u16::from_le_bytes(mem.bytes[a as usize..a as usize + 2].try_into().unwrap())
}

/// One world: its memory before the call, and the cache's answers.
struct Case {
    mem: FlatMemory,
    /// Resident block infos and their sections (`[info+0x28]`).
    sections: HashMap<u32, u32>,
    /// Resident images and their level-0 bytes.
    pixels: HashMap<u32, u32>,
    next: u32,
    datums: u32,
    hit: u32,
    /// The path kinds the world sets up.
    kinds: Vec<&'static str>,
}

/// A base map's residency, as a path kind.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Res {
    Locked,
    Direct,
    Absent,
}

impl Case {
    fn new(image: &FlatMemory) -> Self {
        Case {
            mem: image.clone(),
            sections: HashMap::new(),
            pixels: HashMap::new(),
            // The tag table first.
            next: WORLD + 0x1000,
            datums: 1,
            hit: 0,
            kinds: Vec::new(),
        }
    }
    fn alloc(&mut self, n: u32) -> u32 {
        let a = self.next;
        self.next = (a + n + 0xf) & !0xf;
        assert!(self.next < WORLD_END, "the world outgrew its space");
        a
    }
    fn w(&mut self, a: u32, v: u32) {
        put(&mut self.mem, a, v);
    }
    fn h(&mut self, a: u32, v: u16) {
        put16(&mut self.mem, a, v);
    }
    fn b(&mut self, a: u32, v: u8) {
        self.mem.bytes[a as usize] = v;
    }
    fn f(&mut self, a: u32, v: f32) {
        self.w(a, v.to_bits());
    }
    fn fill(&mut self, r: &mut Rng, a: u32, n: u32, floats: bool) {
        for i in (0..n).step_by(4) {
            let v = if floats { r.finite() } else { r.u32() };
            self.w(a + i, v);
        }
    }
    /// A new tag at `addr`; its datum (junk in the salt).
    fn tag(&mut self, r: &mut Rng, addr: u32) -> u32 {
        let index = self.datums;
        self.datums += 1;
        assert!(index < 0x100);
        self.w(TABLE + index * 16 + 8, addr);
        (r.u32() & 0x7fff_0000) | index
    }
    /// A block info whose section is `section`, resident or not.
    fn block(&mut self, info: u32, section: u32, resident: bool) {
        self.w(info + 0x28, section);
        if resident {
            self.sections.insert(info, section);
        }
    }

    /// A section with its meshes, triangles and stream records; the
    /// meshes' types and material indices.
    fn section(&mut self, r: &mut Rng) -> (u32, [(u16, u16); 2]) {
        let sec = self.alloc(0x40);
        let meshes = self.alloc(0x48 * 2);
        self.w(sec + 4, meshes);
        let mut kinds = [(0, 0); 2];
        for (i, k) in kinds.iter_mut().enumerate() {
            let ty = if r.one_in(8) { r.pick(&[4, 5]) } else { r.pick(&[0, 1, 2, 3]) };
            let mat = r.below(MATERIALS) as u16;
            let m = meshes + 0x48 * i as u32;
            self.h(m, ty);
            self.h(m + 4, mat);
            *k = (ty, mat);
        }
        let tris = self.alloc(6 * TRIS);
        self.w(sec + 0x24, tris);
        for i in 0..3 * TRIS {
            let c = r.below(VERTS) as u16;
            self.h(tris + 2 * i, c);
        }
        // 0..7 records; eight are there to read either way (record 1 is
        // read with any count).
        let count = r.below(8);
        self.w(sec + 0x38, count);
        let recs = self.alloc(0x20 * 8);
        self.w(sec + 0x3c, recs);
        for k in 0..8 {
            let rec = recs + 0x20 * k;
            let ty = match k {
                1 => r.pick(&[0x18, 0x18, 0x19]),
                3..=7 => r.pick(&[0x1e, 0x1f, 0x30, 0x2e, 0x2f, 0x30, 0x00, 0x31]),
                _ => r.u32() as u8,
            };
            self.b(rec, ty);
            let data = self.alloc(VERTS * 16 + 0x40);
            self.fill(r, data, VERTS * 16 + 0x40, true);
            self.w(rec + 0xc, data);
            self.w(rec + 8, if k == 1 { 4 * r.below(8) } else { r.u32() });
        }
        (sec, kinds)
    }

    /// An image record (0x74 bytes) at `img` of a format from `formats`
    /// (or, direct, the direct-only ones), resident as `res`.
    fn image(&mut self, r: &mut Rng, img: u32, formats: &[u16], res: Res) {
        let format = if res == Res::Direct && r.one_in(4) { r.pick(&DIRECT_ONLY) } else { r.pick(formats) };
        let dxt = (14..=16).contains(&format);
        let (w, h) = (r.pick(&SIDES), r.pick(&SIDES));
        self.h(img + 4, w);
        self.h(img + 6, h);
        self.h(img + 0xc, format);
        let pixels = self.alloc(PIXEL_BYTES);
        self.fill(r, pixels, PIXEL_BYTES, matches!(format, 19 | 20));
        let mut flags = (r.u32() as u16) & 0xffe1;
        match res {
            Res::Locked => {
                // A linear image fails the lock.
                if r.one_in(10) {
                    flags |= 0x10;
                }
                self.pixels.insert(img, pixels);
                let own = if r.one_in(2) { pixels } else { 0 };
                self.w(img + 0x54, own);
                self.w(img + 0x34, r.u32());
            }
            Res::Direct => {
                if dxt {
                    flags |= 2;
                }
                if r.one_in(2) {
                    flags |= 8;
                }
                if format == P8 {
                    flags |= 4;
                }
                self.w(img + 0x54, pixels);
                // A short bound sends the DXT read outside it.
                let size = if r.one_in(2) { PIXEL_BYTES } else { r.below(0x30) };
                self.w(img + 0x34, size);
            }
            Res::Absent => {
                self.w(img + 0x54, 0);
                self.w(img + 0x34, r.u32());
            }
        }
        self.h(img + 0xe, flags);
    }

    fn residency(r: &mut Rng) -> Res {
        match r.below(4) {
            0 | 1 => Res::Locked,
            2 => Res::Direct,
            _ => Res::Absent,
        }
    }

    /// A bitmap tag of `n` images; its datum and their residencies.
    fn bitmap(&mut self, r: &mut Rng, n: u32, formats: &[u16]) -> (u32, Vec<Res>) {
        let tag = self.alloc(0x50);
        let images = self.alloc(0x74 * n.max(1));
        self.w(tag + 0x44, n);
        self.w(tag + 0x48, images);
        let mut res = Vec::new();
        for i in 0..n {
            let rs = Self::residency(r);
            self.image(r, images + 0x74 * i, formats, rs);
            res.push(rs);
        }
        (self.tag(r, tag), res)
    }

    /// `MATERIALS` materials (0x20 bytes each) and each one's base map kind.
    fn materials(&mut self, r: &mut Rng) -> (u32, Vec<&'static str>) {
        let mats = self.alloc(0x20 * MATERIALS);
        let mut kinds = Vec::new();
        for i in 0..MATERIALS {
            let m = mats + 0x20 * i;
            if r.one_in(6) {
                self.w(m + 0xc, NONE);
                kinds.push("base: none");
                continue;
            }
            let shader = self.alloc(0x30);
            let template = self.alloc(0x70);
            self.w(shader + 0x24, template);
            let index = self.alloc(4);
            let none = r.one_in(8);
            self.h(index, if none { 0xffff } else { 0 });
            self.w(template + 0x60, index);
            let maps = self.alloc(12);
            self.w(template + 8, maps);
            let kind = if none {
                self.w(maps, r.u32());
                "base: none"
            } else if r.one_in(8) {
                self.w(maps, NONE);
                "base: none"
            } else {
                let n = if r.one_in(8) { 0 } else { 1 + r.below(2) };
                let (datum, res) = self.bitmap(r, n, &BASE_FORMATS);
                self.w(maps, datum);
                match res.first() {
                    None => "base: none",
                    Some(Res::Locked) => "base: locked",
                    Some(Res::Direct) => "base: direct",
                    Some(Res::Absent) => "base: absent",
                }
            };
            let datum = self.tag(r, shader);
            self.w(m + 0xc, datum);
            kinds.push(kind);
        }
        (mats, kinds)
    }

    /// The default vectors, the output, the flags the tree reads.
    fn common(&mut self, r: &mut Rng) {
        put(&mut self.mem, TAGS, TABLE);
        for p in DEFAULTS {
            let v = self.alloc(0x10);
            for i in 0..3 {
                let c = if r.one_in(4) { r.finite() } else { (r.unit() * 1.5 - 0.25).to_bits() };
                self.w(v + 4 * i, c);
            }
            self.w(p, v);
        }
        let flag = r.below(2) as u8;
        self.b(DXT_OUTSIDE, flag);
        self.w(ASK_AGAIN, r.below(2));
        self.w(PRT_COUNT, 0);
    }

    /// The lightmap tag, its groups' palettes, the scenario that names it
    /// and the palette table `0x00015b70` builds from them.
    fn lightmap(&mut self, r: &mut Rng) -> (u32, u32) {
        let lm = self.alloc(0x90);
        let groups = self.alloc(0x68 * 2);
        self.w(lm + 0x80, if r.one_in(10) { 0 } else { 1 + r.below(2) });
        self.w(lm + 0x84, groups);
        for g in 0..2 {
            let group = groups + 0x68 * g;
            let n = r.below(4);
            let pals = self.alloc(0x400 * n.max(1));
            self.fill(r, pals, 0x400 * n.max(1), false);
            self.w(group + 8, n);
            let top = if r.one_in(2) { 0x8000_0000 } else { 0 };
            self.w(group + 0xc, pals | top);
        }
        self.w(LIGHTMAP, lm);
        let datum = self.tag(r, lm);
        // The scenario's BSP entries: the current one names the lightmap
        // (or none).
        let scenario = self.alloc(0x220);
        let bsps = self.alloc(0x44 * 2);
        self.w(scenario + 0x214, bsps);
        self.w(SCENARIO, scenario);
        let index = r.below(2);
        put16(&mut self.mem, BSP_INDEX, index as u16);
        let named = if r.one_in(8) { NONE } else { datum };
        self.w(bsps + 0x44 * index + 0x1c, named);
        self.w(bsps + 0x44 * (1 - index) + 0x1c, r.u32());
        // 0x00015b70.
        let mut n = 0;
        if named != NONE {
            for g in 0..word(&self.mem, lm + 0x80) {
                let group = groups + 0x68 * g;
                for j in 0..word(&self.mem, group + 8) {
                    if n < 32 {
                        let e = PALETTE_TABLE + 12 * n;
                        let p = word(&self.mem, group + 0xc).wrapping_add(0x400 * j) & 0x7fff_ffff;
                        self.w(e, 0x0003_0001);
                        self.w(e + 4, p);
                        self.w(e + 8, 0);
                        n += 1;
                    }
                }
            }
        }
        self.w(PALETTE_COUNT, n);
        (lm, groups)
    }

    /// A render info entry: the page and the palette.
    fn render_info(&mut self, r: &mut Rng, e: u32, pages: u32) {
        let page = match r.below(10) {
            0 => 0xffff,
            1 => pages as u16,
            _ => r.below(pages.max(1)) as u16,
        };
        let palette = match r.below(10) {
            0 => 0xff,
            1 => 40,
            _ => r.below(7) as u8,
        };
        self.h(e, page);
        self.b(e + 2, palette);
        self.b(e + 3, r.u32() as u8);
    }

    fn hit_point(&mut self, r: &mut Rng, hit: u32) {
        self.w(hit + 0x20, r.below(2));
        self.w(hit + 0x28, r.below(TRIS));
        let (u, v) = if r.one_in(8) { (r.unit() * 1.2, r.unit() * 1.2) } else { (r.unit() * 0.5, r.unit() * 0.5) };
        self.f(hit + 0x30, u);
        self.f(hit + 0x34, v);
    }

    /// A structure hit; the kinds of its path.
    fn structure(&mut self, r: &mut Rng, hit: u32) {
        let (_, groups) = self.lightmap(r);
        let group = groups;
        let pages = 1 + r.below(3);
        if r.one_in(10) {
            self.w(group + 0x1c, NONE);
        } else {
            let (datum, _) = self.bitmap(r, pages, &PAGE_FORMATS);
            self.w(group + 0x1c, datum);
        }
        let clusters_info = self.alloc(16);
        let instances_info = self.alloc(16);
        self.w(group + 0x2c, clusters_info);
        self.w(group + 0x4c, instances_info);
        for i in 0..4 {
            self.render_info(r, clusters_info + 4 * i, pages);
            self.render_info(r, instances_info + 4 * i, pages);
        }
        let bsp = self.alloc(0x150);
        self.w(BSP, bsp);
        let clusters = self.alloc(0xb0 * 4);
        let defs = self.alloc(0xc8 * 2);
        let instances = self.alloc(0x58 * 4);
        let (mats, bases) = self.materials(r);
        self.w(bsp + 0xa0, clusters);
        self.w(bsp + 0xa8, mats);
        self.w(bsp + 0x13c, defs);
        self.w(bsp + 0x144, instances);
        let (sec, meshes) = self.section(r);
        let resident = !r.one_in(8);
        self.w(hit, r.below(4));
        self.w(hit + 8, NONE);
        self.hit_point(r, hit);
        let mut by_vertex = false;
        if r.one_in(2) {
            let cluster = word(&self.mem, hit);
            self.w(hit + 4, NONE);
            self.block(clusters + 0xb0 * cluster + 0x28, sec, resident);
            self.kinds.push("cluster");
        } else {
            let i = r.below(4);
            let def = r.below(2);
            self.w(hit + 4, i);
            let inst = instances + 0x58 * i;
            self.h(inst + 0x34, def as u16);
            by_vertex = r.one_in(3);
            self.h(inst + 0x56, by_vertex as u16);
            self.block(defs + 0xc8 * def + 0x28, sec, resident);
            self.vertex_buffers(r, group);
            self.kinds.push("instance");
        }
        let (ty, mat) = meshes[word(&self.mem, hit + 0x20) as usize];
        if !resident {
            self.kinds.push("block not resident");
        } else if matches!(ty, 4 | 5) {
            self.kinds.push("unlit mesh");
        } else {
            if by_vertex {
                self.kinds.push("vertex-lit instance");
            }
            self.kinds.push(bases[mat as usize]);
        }
    }

    /// The group's instance streams and vertex buffers (`0x000d35a0`).
    fn vertex_buffers(&mut self, r: &mut Rng, group: u32) {
        let streams = self.alloc(12 * 4);
        let buffers = self.alloc(0x38 * 2);
        self.w(group + 0x54, streams);
        self.w(group + 0x44, buffers);
        for i in 0..4 {
            let e = streams + 12 * i;
            self.h(e + 2, r.below(2) as u16);
            let first = self.alloc(4);
            self.h(first, r.below(16) as u16);
            self.w(e + 8, first);
        }
        for b in 0..2 {
            let buffer = buffers + 0x38 * b;
            let flags = (r.u32() as u8) | if r.one_in(4) { 0 } else { 2 };
            let flags = if r.one_in(5) { flags & !2 } else { flags };
            self.b(buffer, flags);
            let sec = self.alloc(0x10);
            let recs = self.alloc(0x40);
            self.w(sec + 4, recs);
            let rec = recs + 0x20;
            let ty = r.pick(&[0x2e, 0x2e, 0x2f]);
            self.b(rec, ty);
            self.w(rec + 4, r.u32());
            self.w(rec + 8, r.u32());
            let data = self.alloc(0x80);
            self.fill(r, data, 0x80, false);
            self.w(rec + 0xc, data);
            let resident = !r.one_in(6);
            self.block(buffer + 0xc, sec, resident);
        }
    }

    /// An object hit.
    fn object(&mut self, r: &mut Rng, hit: u32) {
        // The lightmap is still there for the palette table's sake.
        self.lightmap(r);
        let model = self.alloc(0x80);
        let entries = self.alloc(0x5c * 2);
        let (shaders, bases) = self.materials(r);
        self.w(model + 0x28, entries);
        self.w(model + 0x64, shaders);
        let prt = if r.one_in(3) { 1 + r.below(3) } else { 0 };
        self.w(model + 0x74, prt);
        self.w(model + 0x78, r.u32());
        let datum = self.tag(r, model);
        self.w(hit + 4, r.u32());
        self.w(hit + 8, r.below(0x400));
        self.w(hit + 0xc, datum);
        self.hit_point(r, hit);
        self.kinds.push("object");
        if r.one_in(10) {
            self.w(hit + 0x14, NONE);
            self.kinds.push("object: no part");
            return;
        }
        let part = r.below(2);
        self.w(hit + 0x14, part);
        let (sec, meshes) = self.section(r);
        let entry = entries + 0x5c * part;
        self.w(entry + 0x34, sec);
        // The store's block info for the section, or none (the tag's
        // pointer either way).
        if r.one_in(2) {
            self.sections.insert(entry + 0x38, sec);
        }
        if prt > 0 {
            self.kinds.push("object: PRT count");
        }
        let (_, mat) = meshes[word(&self.mem, hit + 0x20) as usize];
        self.kinds.push(bases[mat as usize]);
    }

    /// The world for one seed and case; its arguments.
    fn build(&mut self, r: &mut Rng) -> [u32; 4] {
        self.common(r);
        let hit = self.alloc(0x40);
        self.hit = hit;
        let out = self.alloc(0x20);
        self.fill(r, out, 0x20, false);
        if r.one_in(3) {
            self.object(r, hit);
        } else {
            self.structure(r, hit);
        }
        let defaults = (r.u32() & !0xff) | if r.one_in(2) { 0 } else { 1 + r.below(0xff) };
        [hit, out, r.below(4), defaults]
    }
}

/// The Rust side's reads: the same world and the same answers.
struct Fixture<'a>(&'a Case);

impl Source for Fixture<'_> {
    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), GuestFault> {
        let a = fold(addr) as usize;
        match self.0.mem.bytes.get(a..a + dst.len()) {
            Some(src) => {
                dst.copy_from_slice(src);
                Ok(())
            }
            None => Err(GuestFault {
                addr,
                len: dst.len(),
                access: Access::Read,
                reason: FaultReason::Unmapped,
            }),
        }
    }
    fn section(&self, info: u32) -> Result<Option<u32>, GuestFault> {
        Ok(self.0.sections.get(&info).copied())
    }
    fn pixels(&self, image: u32) -> Result<Option<u32>, GuestFault> {
        Ok(self.0.pixels.get(&image).copied())
    }
    fn prt_entries(&self) -> u32 {
        0
    }
}

struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

impl Lifted<'_> {
    fn lift(&mut self, entry: u32) {
        if let Some(f) = self.lifter.lift(entry) {
            for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                self.cache.entry(insn.pc).or_insert(insn);
            }
        }
    }

    /// Steps from `cpu.eip` until one of `exits`; returns the exit.
    fn run(&mut self, cpu: &mut Cpu, mem: &mut Ram, exits: &[u32]) -> Result<u32, String> {
        let mut pc = cpu.eip;
        let mut steps = 0;
        while !exits.contains(&pc) {
            steps += 1;
            if steps > STEP_LIMIT {
                return Err("step limit".into());
            }
            if !self.cache.contains_key(&pc) {
                self.lift(pc);
            }
            let insn = self.cache.get(&pc).ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(cpu, mem, insn)
                .map_err(|e| format!("{pc:#x}: {e:?}"))?
                .target();
        }
        Ok(pc)
    }

    /// `entry(EAX)` on the guest from a stub, below its stack; EAX out.
    fn call(&mut self, cpu: &Cpu, mem: &mut Ram, entry: u32, eax: u32) -> Result<u32, String> {
        let mut c = cpu.clone();
        c.gpr[4] = cpu.gpr[4] - 0x400;
        put(&mut mem.0, c.gpr[4], NESTED);
        c.gpr[0] = eax;
        c.eip = entry;
        self.run(&mut c, mem, &[NESTED])?;
        Ok(c.gpr[0])
    }

    /// The sample on the guest with the seams answered; EAX.
    fn sample(&mut self, case: &Case, mem: &mut Ram, args: [u32; 4], r: &mut Rng) -> Result<u32, String> {
        let esp = STACK_TOP - 0x100;
        put(&mut mem.0, esp, RETURN);
        for (i, a) in args.into_iter().enumerate() {
            put(&mut mem.0, esp + 4 + 4 * i as u32, a);
        }
        let mut c = Cpu {
            gpr: [r.u32(), r.u32(), r.u32(), r.u32(), esp, r.u32(), r.u32(), r.u32()],
            fs_base: RAM as u32,
            eip: SAMPLE,
            ..Cpu::default()
        };
        c.mxcsr = 0x1f80;
        c.x87.control = 0x027f;
        loop {
            let exit = self.run(&mut c, mem, &EXITS)?;
            let ret = |c: &mut Cpu, mem: &Ram, args: u32| {
                c.eip = word(&mem.0, c.gpr[4]);
                c.gpr[4] += 4 + args;
            };
            let al = |c: &mut Cpu, v: bool| c.gpr[0] = (c.gpr[0] & !0xff) | v as u32;
            match exit {
                RETURN => {
                    if c.gpr[4] != esp + 0x14 {
                        return Err(format!("returned with ESP {:#x}, not {:#x}", c.gpr[4], esp + 0x14));
                    }
                    return Ok(c.gpr[0]);
                }
                RESIDENT => {
                    let resident = case.sections.contains_key(&c.gpr[1]);
                    al(&mut c, resident);
                    ret(&mut c, &*mem, 0);
                }
                TEXTURE => {
                    if !case.pixels.contains_key(&c.gpr[0]) {
                        c.gpr[0] = 0;
                    }
                    ret(&mut c, &*mem, 0);
                }
                LOADED => {
                    c.gpr[0] = 0;
                    ret(&mut c, &*mem, 0);
                }
                TOUCH | REQUEST | RELEASE => ret(&mut c, &*mem, 0),
                LOCK => {
                    let ok = self.lock(case, &c, mem)?;
                    al(&mut c, ok);
                    ret(&mut c, &*mem, 8);
                }
                _ => unreachable!(),
            }
        }
    }

    /// `0x0001d2f0(texture, record)` for a resident cache: the texture is
    /// the image `0x000d15e0` answered with.
    fn lock(&mut self, case: &Case, c: &Cpu, mem: &mut Ram) -> Result<bool, String> {
        let esp = c.gpr[4];
        let (image, rec) = (word(&mem.0, esp + 4), word(&mem.0, esp + 8));
        let pixels = *case.pixels.get(&image).ok_or_else(|| format!("lock of {image:#x}, not resident"))?;
        let m = &case.mem;
        let (w, h) = (half(m, image + 4), half(m, image + 6));
        let (format, flags) = (half(m, image + 0xc), half(m, image + 0xe));
        if flags & 0x10 != 0 || !LOCKABLE.contains(&format) {
            return Ok(false);
        }
        let mem0 = &mut mem.0;
        mem0.bytes[rec as usize..rec as usize + 0x74].fill(0);
        put(mem0, rec, 0x6269_746d);
        put16(mem0, rec + 4, w);
        put16(mem0, rec + 6, h);
        mem0.bytes[rec as usize + 8] = 1;
        put16(mem0, rec + 0xc, format);
        let mut f = 1;
        if (14..=16).contains(&format) {
            f |= 2;
        }
        if format == P8 {
            f |= 4;
        }
        put16(mem0, rec + 0xe, f);
        put16(mem0, rec + 0x14, 1);
        put(mem0, rec + 0x54, pixels);
        let size = self.call(c, mem, IMAGE_SIZE, rec)?;
        put(&mut mem.0, rec + 0x34, size);
        Ok(true)
    }
}

/// Runs `f` with the image and a lifter over it; `false` without the XBE.
fn with_image(f: impl FnOnce(&FlatMemory, Lifted<'_>)) -> bool {
    let Some(path) = xbe_path() else {
        return false;
    };
    let data = std::fs::read(&path).unwrap();
    assert_eq!(sha256(&data), IMAGE_SHA256, "the retail XBE only");
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();
    let image = lift::harness::load_image(&x).unwrap();
    f(
        &image,
        Lifted {
            lifter: &lifter,
            cache: BTreeMap::new(),
        },
    );
    true
}

fn seeds() -> Vec<u64> {
    std::env::var("H2_DIFF_SEEDS")
        .map(|s| s.split(',').map(|v| v.trim().parse().unwrap()).collect())
        .unwrap_or_else(|_| vec![1, 7, 99])
}

/// The guest's RAM after the call against the world with the Rust
/// side's writes applied, outside the stack; the first word that differs.
fn compare(pre: &FlatMemory, post: &FlatMemory, writes: &[(u32, [u8; 4])]) -> Result<(), String> {
    let mut want: BTreeMap<u32, u8> = BTreeMap::new();
    for (a, bytes) in writes {
        for (i, b) in bytes.iter().enumerate() {
            want.insert(fold(a.wrapping_add(i as u32)), *b);
        }
    }
    let expected = |a: u32| want.get(&a).copied().unwrap_or(pre.bytes[a as usize]);
    let report = |a: u32| {
        let w = a & !3;
        let got = word(post, w);
        let rust = u32::from_le_bytes([0, 1, 2, 3].map(|i| expected(w + i)));
        Err(format!("word {w:#x}: guest {got:#010x}, rust {rust:#010x}"))
    };
    for &a in want.keys() {
        if a as usize >= RAM || (STACK_LO..STACK_TOP).contains(&a) {
            return Err(format!("rust store at {a:#x}, in the stack or outside RAM"));
        }
        if post.bytes[a as usize] != expected(a) {
            return report(a);
        }
    }
    const CHUNK: usize = 0x1000;
    for (k, (p, q)) in pre.bytes.chunks(CHUNK).zip(post.bytes.chunks(CHUNK)).enumerate() {
        if p == q {
            continue;
        }
        for i in 0..p.len() {
            let a = (k * CHUNK + i) as u32;
            if p[i] != q[i] && !(STACK_LO..STACK_TOP).contains(&a) && !want.contains_key(&a) {
                return report(a);
            }
        }
    }
    Ok(())
}

/// The format of a structure hit's lightmap page (one that exists).
fn page_format(case: &Case) -> u16 {
    let m = &case.mem;
    let group = word(m, word(m, LIGHTMAP) + 0x84);
    let hit = case.hit;
    let instance = word(m, hit + 4);
    let info = if instance == NONE {
        word(m, group + 0x2c) + 4 * word(m, hit)
    } else {
        word(m, group + 0x4c) + 4 * instance
    };
    let page = u32::from(half(m, info));
    let bitmap = word(m, TABLE + (word(m, group + 0x1c) & 0xffff) * 16 + 8);
    half(m, word(m, bitmap + 0x48) + 0x74 * page + 0xc)
}

/// The path kinds a case's result shows, beside those its world set up.
fn result_kinds(case: &Case, s: &Sampled) -> Vec<&'static str> {
    let mut k = Vec::new();
    let lit = case.kinds.iter().any(|&x| matches!(x, "cluster" | "instance"))
        && !case
            .kinds
            .iter()
            .any(|&x| matches!(x, "block not resident" | "unlit mesh" | "vertex-lit instance"));
    if lit {
        // 0x000d2f90's EDI.
        k.push(match s.eax {
            0 => "page direct",
            1 => "page none or not resident",
            3 if page_format(case) == P8 => "page locked P8",
            3 => "page locked DXT",
            _ => "page: other result",
        });
    }
    if s.writes.iter().any(|(a, _)| *a == DXT_OUTSIDE) {
        k.push("DXT read outside its image");
    }
    k
}

#[test]
fn the_sample_matches_the_lifted_original() {
    if !with_image(sample_cases) {
        eprintln!("SKIPPED the_sample_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn sample_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(SAMPLE);
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);
            let mut case = Case::new(image);
            let args = case.build(&mut r);
            let want = sample::run(&Fixture(&case), args).unwrap_or_else(|e| panic!("seed {seed} case {n}: the port stopped: {e:?}"));
            let mut mem = Ram(case.mem.clone());
            let eax = lifted
                .sample(&case, &mut mem, args, &mut r)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(eax, want.eax, "seed {seed} case {n}: EAX ({:?})", case.kinds);
            if let Err(e) = compare(&case.mem, &mem.0, &want.writes) {
                panic!("seed {seed} case {n}: {e} ({:?}, rust writes {:x?})", case.kinds, want.writes);
            }
            for k in case.kinds.iter().copied().chain(result_kinds(&case, &want)) {
                *seen.entry(k).or_default() += 1;
            }
        }
    }
    eprintln!("lightmap sample: {seen:#?}");
    for k in [
        "cluster",
        "instance",
        "object",
        "block not resident",
        "unlit mesh",
        "vertex-lit instance",
        "page direct",
        "page none or not resident",
        "page locked P8",
        "page locked DXT",
        "base: locked",
        "base: direct",
        "base: absent",
        "base: none",
        "object: no part",
        "object: PRT count",
        "DXT read outside its image",
    ] {
        assert!(seen.contains_key(k), "{k} never seen: widen the worlds");
    }
}
