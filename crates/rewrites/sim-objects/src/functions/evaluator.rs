//! The object-function evaluator over Rust-owned data, phase 3 of the
//! simulation rewrite (docs/engine/sim-rewrite.md, "Objects and units
//! update (phase 3)"): `bab40` OBJECT_FUNCTION_VALUE, its export
//! evaluator `ba8c0`, the import dispatcher `108d90`, the base level's
//! import handler `bcc20`, and `13bb90`'s output range over the curve
//! kernel [`super::curve`]. The register-level segment bodies in
//! `engine::sim_objects::functions` are the oracle (docs/engine/object-update.md,
//! "Slice 3 bodies"); this is the same arithmetic as plain Rust calls.
//!
//! **Rust-owned data.** A [`Library`] is built once per map: the image
//! constants ([`Constants`]), the shape and wave tables ([`Tables`]), and
//! each definition's exports with their function blobs ([`Export`]),
//! decoded on first use and kept until the map changes. A definition whose
//! export count or table address differs from the decoded one is decoded
//! again. Per call, only object state is read from guest memory: the
//! body, its flags, parent and type, the fields the base imports name,
//! game time and the world globals.
//!
//! **What it answers.** A call is answered only when every step on its
//! path is Rust: the built-in names, the base level's own values, every
//! export, the scale and gate functions, the parent's value. Anything else
//! is declined, named ([`Decline`]), and the guest answers: a scripted
//! override (body `+c0` bit 9), an import a type level or a base callout
//! answers, curve kinds 9 and 10, an operand off the exact path, a read
//! that faults, a recursion deeper than [`MAX_DEPTH`].
//!
//! **What an answer is.** The value `bab40` stores through its out
//! pointer, its `al`, the byte its `ecx` carries in the top lane (the
//! frame's result byte, as retail pops it), and the low lane of `xmm0`
//! where retail leaves the value there.

use super::curve::{self, Image};
use sim_core::det::{self, Fp};
use std::collections::HashMap;
use std::sync::Arc;

/// Guest memory the evaluator reads: `None` for a read that faults.
pub trait Memory {
    fn u32(&self, at: u32) -> Option<u32>;
    fn u16(&self, at: u32) -> Option<u16>;
    fn u8(&self, at: u32) -> Option<u8>;
}

/// The null datum handle.
pub const NONE: u32 = u32::MAX;

/// Nested `bab40` calls (scale functions, parents) past this are declined.
pub const MAX_DEPTH: u32 = 16;

/// More exports than this on one definition: declined as a fault.
pub const MAX_EXPORTS: u32 = 4096;

/// Shape (transition) and wave (periodic) sub-types, 1024 bytes each.
pub const SHAPES: usize = 8;
pub const WAVES: usize = 12;
pub const TABLE: usize = 1024;

/// Why a call is the guest's to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// MXCSR or the x87 not in the pinned state, the host not either, or
    /// an operand off the exact path.
    Float,
    /// Body `+c0` bit 9: the scripted override `10aac0`.
    Override,
    /// An import a type level's handler or a base callout answers.
    Import,
    /// Curve kinds 9 and 10, or a shape or wave sub-type with no table.
    Curve,
    /// A guest read that faulted, or a definition that would not decode.
    Fault,
    /// Nested calls past [`MAX_DEPTH`].
    Depth,
}

impl Decline {
    pub const ALL: [Decline; 6] = [
        Decline::Float,
        Decline::Override,
        Decline::Import,
        Decline::Curve,
        Decline::Fault,
        Decline::Depth,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Decline::Float => "float",
            Decline::Override => "override",
            Decline::Import => "import",
            Decline::Curve => "curve",
            Decline::Fault => "fault",
            Decline::Depth => "depth",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

type R<T> = Result<T, Decline>;

/// Guest addresses the evaluator reads per call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Globals {
    /// The objects pool pointer (its data at `+44`, `c` bytes a header,
    /// the body at `+8`).
    pub objects: u32,
    /// The tag entries pointer (`10` bytes an entry, the definition at `+8`).
    pub tags: u32,
    /// The type table: a descriptor pointer per object type.
    pub types: u32,
    /// The world globals pointer (`+70..7c` the world imports).
    pub world: u32,
    /// The game globals pointer (`+8` the mode).
    pub game: u32,
    /// The game time pointer (`+4` seconds per tick, `+8` the tick).
    pub game_time: u32,
    /// `bcc20`, as a level's slot `+4c` holds it.
    pub base_import: u32,
}

/// A handle hash: `(handle * mul + add) >> 16`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hash {
    pub mul: u32,
    pub add: u32,
}

impl Hash {
    pub fn of(self, handle: u32) -> u32 {
        handle.wrapping_mul(self.mul).wrapping_add(self.add) >> 16
    }
}

/// The string IDs `bcc20` dispatches on, read from its `cmp` and `sub`
/// immediates. `*_delta` are the `sub` chain's steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BaseNames {
    pub random: u32,
    pub bcad0: u32,
    pub facing: u32,
    pub one: u32,
    pub zero: u32,
    pub flag: u32,
    pub variant: u32,
    pub bcbb0: u32,
    pub bca70: u32,
    pub body_ec: u32,
    pub one_b: u32,
    pub bcb10_delta: u32,
    pub component: u32,
    pub body_f0: u32,
    pub f0_less_one: u32,
    pub c18e0: u32,
    pub shield_test: u32,
    pub one_c: u32,
    pub body_f8: u32,
    pub body_f4: u32,
    pub component_lo: u32,
    pub component_hi: u32,
    pub world_78: u32,
    pub component_b: u32,
    pub world_70_delta: u32,
    pub world_7c: u32,
    pub component_lo_b: u32,
    pub component_hi_b: u32,
}

/// The image words the evaluator uses, read once per map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constants {
    pub globals: Globals,
    /// 1.0 and 0.0 (f32 bits).
    pub one: u32,
    pub zero: u32,
    /// `bab40`'s built-in names.
    pub name_one: u32,
    pub name_zero: u32,
    /// `ba8c0`'s per-object time offset: the hash and its scale (f32).
    pub time_hash: Hash,
    pub time_scale: u32,
    /// `bcc20`'s per-object random value: the hash and its scale (f32).
    pub random_hash: Hash,
    pub random_scale: u32,
    pub base: BaseNames,
    /// The curve kernel's constants. Its table addresses are replaced by
    /// the Rust-owned [`Tables`].
    pub curve: Image,
}

/// The shape and wave tables, copied out of the image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tables {
    pub shapes: Box<[u8]>,
    pub waves: Box<[u8]>,
}

impl Tables {
    /// Copy [`SHAPES`] and [`WAVES`] tables from `shapes` and `waves`.
    pub fn read(m: &impl Memory, shapes: u32, waves: u32) -> Option<Tables> {
        let copy = |base: u32, n: usize| -> Option<Box<[u8]>> {
            (0..(n * TABLE) as u32)
                .map(|i| m.u8(base.wrapping_add(i)))
                .collect()
        };
        Some(Tables {
            shapes: copy(shapes, SHAPES)?,
            waves: copy(waves, WAVES)?,
        })
    }
}

/// A tag function: `13bb90`'s operand. The blob is copied as far as the
/// curve reads it; `None` for an empty reference (`13bb90` answers 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub blob: Option<Box<[u8]>>,
}

/// The bytes the curve kernel reads of a blob of `kind`: the header, the
/// output range at `+4`, and the kind's params from `+14`.
pub fn blob_len(kind: u8, blended: bool) -> u32 {
    let two = |one: u32, both: u32| if blended { both } else { one };
    match kind {
        2 => 0x14 + two(0x08, 0x10),
        3 => 0x14 + two(0x10, 0x20),
        4 => 0x14 + two(0x18, 0x30),
        5 => 0x14 + two(0x50, 0xa0),
        7 => two(0x44, 0x74),
        _ => 0x0c,
    }
}

impl Function {
    /// The function referenced at `reference` (count, then address).
    pub fn read(m: &impl Memory, reference: u32) -> Option<Function> {
        let count = m.u32(reference)?;
        let data = m.u32(reference.wrapping_add(4))?;
        if data == 0 || count as i32 <= 0 {
            return Some(Function { blob: None });
        }
        let len = blob_len(m.u8(data)?, m.u8(data.wrapping_add(1))? & 1 != 0);
        let blob = (0..len)
            .map(|i| m.u8(data.wrapping_add(i)))
            .collect::<Option<Box<[u8]>>>()?;
        Some(Function { blob: Some(blob) })
    }
}

/// One export of a definition (`ba8c0`'s record, `20` bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    /// `+0`: bit 0 inverts the input, bit 1 is a range (its input's
    /// activity carries through), bit 2 forces active, bit 3 offsets the
    /// time input by a hash of the handle.
    pub flags: u32,
    /// `+4` the input import, `+8` the name, `+c` the gate import.
    pub input: u32,
    pub name: u32,
    pub gate: u32,
    /// `+10` (f32 bits): active only above it, when positive.
    pub threshold: u32,
    /// `+1c`: a function value of the same object the result is scaled by.
    pub scale: u32,
    /// `+14`, and whether its first byte is 3 (a periodic curve, driven by
    /// game time). `None` when the blob would not read: evaluating this
    /// export declines.
    pub function: Option<(Function, bool)>,
}

impl Export {
    fn read(m: &impl Memory, record: u32) -> Option<Export> {
        let w = |off: u32| m.u32(record.wrapping_add(off));
        Some(Export {
            flags: w(0)?,
            input: w(4)?,
            name: w(8)?,
            gate: w(0xc)?,
            threshold: w(0x10)?,
            scale: w(0x1c)?,
            function: Export::function(m, record),
        })
    }

    /// The function at `+14` and whether its blob's first byte is 3.
    fn function(m: &impl Memory, record: u32) -> Option<(Function, bool)> {
        let time = m.u8(m.u32(record.wrapping_add(0x18))?)? == 3;
        Some((Function::read(m, record.wrapping_add(0x14))?, time))
    }
}

/// A decoded definition, with the count and table it was decoded from.
#[derive(Clone, Debug)]
struct Decoded {
    count: u32,
    table: u32,
    exports: Option<Arc<[Export]>>,
}

/// One call's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    /// Stored through the out pointer (f32 bits).
    pub value: u32,
    pub al: u8,
    /// The frame's result byte: the top byte of the `ecx` retail pops.
    pub active: u8,
    /// `xmm0`'s low lane at the return; `None` where retail leaves it as
    /// it was at the entry (the zero name, a null handle).
    pub xmm0: Option<u32>,
}

/// The caller's float environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Env {
    pub mxcsr: u32,
    pub x87_control: u16,
    /// The x87 state an `fild`/`fstp` sequence needs (`det::x87_env` with
    /// one free register), checked by the adapter.
    pub x87_ready: bool,
}

/// The per-map data and the evaluator over it.
pub struct Library {
    constants: Constants,
    tables: Tables,
    definitions: HashMap<u32, Decoded>,
}

/// The synthetic addresses the curve kernel reads the Rust-owned blob and
/// tables at.
const LOCAL_REF: u32 = 0x1000_0000;
const LOCAL_BLOB: u32 = 0x2000_0000;
const LOCAL_SHAPES: u32 = 0x3000_0000;
const LOCAL_WAVES: u32 = 0x4000_0000;
/// The reference at [`LOCAL_REF`]: count 1, then [`LOCAL_BLOB`].
const LOCAL_REFERENCE: [u8; 8] = {
    let b = LOCAL_BLOB.to_le_bytes();
    [1, 0, 0, 0, b[0], b[1], b[2], b[3]]
};

impl Library {
    pub fn new(constants: Constants, tables: Tables) -> Library {
        Library {
            constants,
            tables,
            definitions: HashMap::new(),
        }
    }

    pub fn constants(&self) -> &Constants {
        &self.constants
    }

    /// Definitions decoded so far.
    pub fn decoded(&self) -> usize {
        self.definitions.len()
    }

    /// `bab40(handle, name)`.
    pub fn value(&mut self, m: &impl Memory, env: Env, handle: u32, name: u32) -> R<Answer> {
        if !det::fast_env(env.mxcsr) || env.x87_control & 0xc00 != 0 || !det::host_fast_env() {
            return Err(Decline::Float);
        }
        let mut e = Eval {
            lib: self,
            m,
            env,
            fp: Fp::new(),
            depth: 0,
        };
        let answer = e.value(handle, name)?;
        if !e.fp.exact() {
            return Err(Decline::Float);
        }
        Ok(answer)
    }

    /// The exports of the definition at `def`, decoded once.
    fn exports(&mut self, m: &impl Memory, def: u32) -> R<Arc<[Export]>> {
        let count = m.u32(def.wrapping_add(0x64)).ok_or(Decline::Fault)?;
        let table = m.u32(def.wrapping_add(0x68)).ok_or(Decline::Fault)?;
        if let Some(d) = self.definitions.get(&def) {
            if d.count == count && d.table == table {
                return d.exports.clone().ok_or(Decline::Fault);
            }
        }
        let exports = decode(m, count, table);
        self.definitions.insert(
            def,
            Decoded {
                count,
                table,
                exports: exports.clone(),
            },
        );
        exports.ok_or(Decline::Fault)
    }
}

fn decode(m: &impl Memory, count: u32, table: u32) -> Option<Arc<[Export]>> {
    if count as i32 <= 0 {
        return Some(Arc::from(Vec::new()));
    }
    if count > MAX_EXPORTS {
        return None;
    }
    (0..count)
        .map(|i| Export::read(m, table.wrapping_add(i * 0x20)))
        .collect::<Option<Vec<_>>>()
        .map(Arc::from)
}

/// What `bcc20` does with a name, as its signed compare tree decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Missing,
    /// A callout answers it (`bca70`, `bcbb0`, `bcad0`, `bcb10`, `bcb60`,
    /// `c18e0`, the facing angle, the component names).
    Callout,
    Random,
    One,
    Zero,
    /// Body `+10a` bit 2 clear: 1.
    Flag,
    Variant,
    /// A body field at this offset, clamped.
    Field(u32),
    F0LessOne,
    Shield,
    /// A world global at this offset, clamped.
    World(u32),
}

/// `bcc47`: the dispatch, compare for compare.
pub fn classify(name: u32, k: &BaseNames) -> Base {
    let n = name as i32;
    let key = |v: u32| v as i32;
    if n == key(k.random) {
        return Base::Random;
    }
    if n > key(k.random) {
        return classify_high(name, k);
    }
    if n == key(k.bcad0) {
        return Base::Callout;
    }
    if n > key(k.bcad0) {
        return classify_mid(name, k);
    }
    if n == key(k.facing) {
        return Base::Callout;
    }
    if n > key(k.facing) {
        if n == key(k.variant) {
            return Base::Variant;
        }
        if n == key(k.bcbb0) || n == key(k.bca70) {
            return Base::Callout;
        }
        return Base::Missing;
    }
    if n == key(k.one) {
        return Base::One;
    }
    if n == key(k.zero) {
        return Base::Zero;
    }
    if n == key(k.flag) {
        Base::Flag
    } else {
        Base::Missing
    }
}

/// `bcdb2`.
fn classify_mid(name: u32, k: &BaseNames) -> Base {
    let n = name as i32;
    if n == k.body_ec as i32 {
        return Base::Field(0xec);
    }
    if n > k.body_ec as i32 {
        if n == k.component as i32 {
            return Base::Callout;
        }
        return if n == k.body_f0 as i32 {
            Base::Field(0xf0)
        } else {
            Base::Missing
        };
    }
    let e = name.wrapping_sub(k.one_b);
    if e == 0 {
        return Base::One;
    }
    let e = e.wrapping_sub(k.bcb10_delta);
    if e == 0 || e.wrapping_sub(1) == 0 {
        Base::Callout
    } else {
        Base::Missing
    }
}

/// `bce54`.
fn classify_high(name: u32, k: &BaseNames) -> Base {
    let n = name as i32;
    if n == k.f0_less_one as i32 {
        return Base::F0LessOne;
    }
    if n > k.f0_less_one as i32 {
        return classify_top(name, k);
    }
    if n == k.c18e0 as i32 {
        return Base::Callout;
    }
    if n > k.c18e0 as i32 {
        if n == k.body_f4 as i32 {
            return Base::Field(0xf4);
        }
        if n <= k.component_lo as i32 {
            return Base::Missing;
        }
        return if n <= k.component_hi as i32 {
            Base::Callout
        } else {
            Base::Missing
        };
    }
    if n == k.shield_test as i32 {
        return Base::Shield;
    }
    if n == k.one_c as i32 {
        return Base::One;
    }
    if n == k.body_f8 as i32 {
        Base::Field(0xf8)
    } else {
        Base::Missing
    }
}

/// `bcf45`.
fn classify_top(name: u32, k: &BaseNames) -> Base {
    let n = name as i32;
    if n == k.world_78 as i32 {
        return Base::World(0x78);
    }
    if n > k.world_78 as i32 {
        if n == k.world_7c as i32 {
            return Base::World(0x7c);
        }
        if n <= k.component_lo_b as i32 || n > k.component_hi_b as i32 {
            return Base::Missing;
        }
        return Base::Callout;
    }
    let e = name.wrapping_sub(k.component_b);
    if e == 0 {
        return Base::Callout;
    }
    let e = e.wrapping_sub(k.world_70_delta);
    if e == 0 {
        return Base::World(0x70);
    }
    if e.wrapping_sub(1) == 0 {
        Base::World(0x74)
    } else {
        Base::Missing
    }
}

/// One evaluation: the library, the memory, and the float record every
/// step adds to.
struct Eval<'a, M: Memory> {
    lib: &'a mut Library,
    m: &'a M,
    env: Env,
    fp: Fp,
    depth: u32,
}

impl<M: Memory> Eval<'_, M> {
    fn w(&self, at: u32) -> R<u32> {
        self.m.u32(at).ok_or(Decline::Fault)
    }

    fn h(&self, at: u32) -> R<u16> {
        self.m.u16(at).ok_or(Decline::Fault)
    }

    fn b(&self, at: u32) -> R<u8> {
        self.m.u8(at).ok_or(Decline::Fault)
    }

    fn k(&self) -> &Constants {
        &self.lib.constants
    }

    /// `comiss a, b; ja`, on operands a compare reads without a flag.
    fn above(&mut self, a: u32, b: u32) -> bool {
        self.fp.check_compared(a);
        self.fp.check_compared(b);
        det::compare(a, b) == std::cmp::Ordering::Greater
    }

    /// `v` clamped to `[0, one]`, `0 > v` tested first (NaN declines).
    fn clamp(&mut self, v: u32) -> u32 {
        let one = self.k().one;
        if self.above(0, v) {
            0
        } else if self.above(v, one) {
            one
        } else {
            v
        }
    }

    /// The body of `handle` (no salt check, as retail).
    fn body(&self, handle: u32) -> R<u32> {
        let pool = self.w(self.k().globals.objects)?;
        let data = self.w(pool.wrapping_add(0x44))?;
        self.w(data
            .wrapping_add((handle & 0xffff).wrapping_mul(12))
            .wrapping_add(8))
    }

    fn overridden(&self, body: u32) -> R<bool> {
        Ok((self.h(body.wrapping_add(0xc0))? >> 9) & 1 != 0)
    }

    /// `bab40`.
    fn value(&mut self, handle: u32, name: u32) -> R<Answer> {
        if self.depth >= MAX_DEPTH {
            return Err(Decline::Depth);
        }
        self.depth += 1;
        let answer = self.value_at(handle, name);
        self.depth -= 1;
        answer
    }

    fn value_at(&mut self, handle: u32, name: u32) -> R<Answer> {
        let body = self.body(handle)?;
        if self.overridden(body)? {
            return Err(Decline::Override);
        }
        let k = *self.k();
        if name == 0 || name == k.name_one {
            return Ok(Answer {
                value: k.one,
                al: 1,
                active: 0,
                xmm0: Some(k.one),
            });
        }
        if name == k.name_zero || handle == NONE {
            return Ok(Answer {
                value: 0,
                al: 0,
                active: 0,
                xmm0: None,
            });
        }
        if let Some((value, active)) = self.import(handle, body, name)? {
            return Ok(Answer {
                value,
                al: active,
                active,
                xmm0: Some(value),
            });
        }
        let tag = self.w(body)? & 0xffff;
        let tags = self.w(k.globals.tags)?;
        let def = self.w(tags.wrapping_add(tag << 4).wrapping_add(8))?;
        let exports = self.lib.exports(self.m, def)?;
        let mut found = None;
        for export in exports.iter().filter(|e| e.name == name) {
            let (value, al) = self.export(handle, body, export)?;
            found = Some(Answer {
                value,
                al,
                active: al,
                xmm0: Some(value),
            });
        }
        if let Some(answer) = found {
            return Ok(answer);
        }
        if (self.w(body.wrapping_add(4))? >> 26) & 1 != 0 {
            let parent = self.w(body.wrapping_add(0x14))?;
            if parent != NONE {
                let inner = self.value(parent, name)?;
                return Ok(Answer { active: 0, ..inner });
            }
        }
        Ok(Answer {
            value: 0,
            al: 0,
            active: 0,
            xmm0: Some(0),
        })
    }

    /// `108d90`: each ancestry level's slot `+4c` until one answers.
    /// `Some((value, active))` when one did.
    fn import(&mut self, handle: u32, body: u32, name: u32) -> R<Option<(u32, u8)>> {
        let g = self.k().globals;
        let ty = self.b(body.wrapping_add(0xaa))? as i8 as i32 as u32;
        let desc = self.w(g.types.wrapping_add(ty.wrapping_mul(4)))?;
        let mut level = desc.wrapping_add(0x84);
        loop {
            let entry = self.w(level)?;
            if entry == 0 {
                return Ok(None);
            }
            let handler = self.w(entry.wrapping_add(0x4c))?;
            if handler != 0 {
                if handler != g.base_import {
                    return Err(Decline::Import);
                }
                if let Some(answer) = self.base(handle, body, name)? {
                    return Ok(Some(answer));
                }
            }
            level = level.wrapping_add(4);
        }
    }

    /// `108d90` into a value and an active byte, both zero unanswered.
    fn import_or_zero(&mut self, handle: u32, body: u32, name: u32) -> R<(u32, u8)> {
        Ok(self.import(handle, body, name)?.unwrap_or((0, 0)))
    }

    /// `bcc20`: `Some((value, active))`, or `None` for a name it does not
    /// know.
    fn base(&mut self, handle: u32, body: u32, name: u32) -> R<Option<(u32, u8)>> {
        let k = *self.k();
        let value = match classify(name, &k.base) {
            Base::Missing => return Ok(None),
            Base::Callout => return Err(Decline::Import),
            Base::Random => {
                if !self.env.x87_ready {
                    return Err(Decline::Float);
                }
                // `fild` of a 16-bit hash, `fmul` by an f32, `fstp m32`:
                // the exact product rounded once to binary32 in every
                // precision control (it fits in 40 bits).
                let h = det::from_int(k.random_hash.of(handle) as i32);
                let v = self.fp.mul(h, k.random_scale);
                self.clamp(v)
            }
            Base::One => k.one,
            Base::Zero => 0,
            Base::Flag => {
                if (self.b(body.wrapping_add(0x10a))? >> 2) & 1 == 0 {
                    k.one
                } else {
                    0
                }
            }
            Base::Variant => self.variant(body)?,
            Base::Field(off) => {
                let v = self.w(body.wrapping_add(off))?;
                self.clamp(v)
            }
            Base::F0LessOne => {
                let v = self.w(body.wrapping_add(0xf0))?;
                let v = self.fp.sub(v, k.one);
                self.clamp(v)
            }
            Base::Shield => {
                let game = self.w(k.globals.game)?;
                if self.w(game.wrapping_add(8))? == 2 {
                    let ty = u32::from(self.b(body.wrapping_add(0xaa))?);
                    if 1u32.wrapping_shl(ty) & 3 != 0 && self.w(body.wrapping_add(0x13c))? != NONE {
                        return Err(Decline::Import);
                    }
                }
                // `ucomiss` against zero: ordered and equal is 1.
                let v = self.w(body.wrapping_add(0xf0))?;
                self.fp.check_compared(v);
                if det::compare(v, 0) == std::cmp::Ordering::Equal {
                    k.one
                } else {
                    0
                }
            }
            Base::World(off) => {
                let world = self.w(k.globals.world)?;
                let v = self.w(world.wrapping_add(off))?;
                self.clamp(v)
            }
        };
        let active = u8::from(self.above(value, 0));
        Ok(Some((value, active)))
    }

    /// `bcd42`: `(variant + 1) / count`, the variant byte at body `+b1`
    /// clamped to `0..=count`, the count at the model's `+50`.
    fn variant(&mut self, body: u32) -> R<u32> {
        let g = self.k().globals;
        let tags = self.w(g.tags)?;
        let entry = |tag: u32| (tag & 0xffff) << 4;
        let def = self.w(tags.wrapping_add(entry(self.w(body)?)).wrapping_add(8))?;
        let model = self.w(tags
            .wrapping_add(entry(self.w(def.wrapping_add(0x38))?))
            .wrapping_add(8))?;
        let count = self.w(model.wrapping_add(0x50))? as i32;
        if count <= 0 {
            return Ok(0);
        }
        let v = (i32::from(self.b(body.wrapping_add(0xb1))? as i8) + 1).clamp(0, count);
        let v = self.fp.div(det::from_int(v), det::from_int(count));
        Ok(self.clamp(v))
    }

    /// `ba8c0`: one export, `(value, al)`.
    fn export(&mut self, handle: u32, body: u32, e: &Export) -> R<(u32, u8)> {
        let k = *self.k();
        let (function, time) = e.function.as_ref().ok_or(Decline::Fault)?;
        if self.overridden(body)? {
            return Err(Decline::Override);
        }
        let not_range = e.flags & 2 == 0;
        let (mut v, mut a) = self.import_or_zero(handle, body, e.input)?;
        if a == 0 && !not_range && !*time {
            // A range whose input is inactive: the input as it is.
        } else if *time {
            let t = self.time(handle, e.flags)?;
            let x = self.function(function, t, k.one)?;
            v = self.fp.mul(x, v);
        } else {
            if e.flags & 1 != 0 {
                v = self.fp.sub(k.one, v);
            }
            v = self.function(function, v, k.one)?;
            if not_range {
                a = u8::from(self.above(v, k.zero));
            }
        }
        if e.scale != 0 {
            let scale = self.value(handle, e.scale)?;
            v = self.fp.mul(scale.value, v);
            a = u8::from(a != 0 && scale.al != 0);
        }
        if a != 0 && self.above(e.threshold, k.zero) {
            a = u8::from(self.above(v, e.threshold));
        }
        if e.flags & 4 != 0 {
            a = 1;
        }
        if e.gate != 0 {
            if a == 0 {
                return Ok((0, 0));
            }
            if self.import_or_zero(handle, body, e.gate)?.1 == 0 {
                return Ok((0, 0));
            }
        }
        if a == 0 {
            return Ok((0, 0));
        }
        Ok((self.clamp(v), a))
    }

    /// `ba9cb`: ticks times seconds per tick, plus, with flags bit 3, a
    /// hash of the handle in the x87 (`fild`, `fmul m32`, `fadd m32`,
    /// `fstp m32`).
    fn time(&mut self, handle: u32, flags: u32) -> R<u32> {
        let k = *self.k();
        let clock = self.w(k.globals.game_time)?;
        let ticks = self.w(clock.wrapping_add(8))? as i32;
        let per_tick = self.w(clock.wrapping_add(4))?;
        let t = self.fp.mul(det::from_int(ticks), per_tick);
        if flags & 8 == 0 {
            return Ok(t);
        }
        if !self.env.x87_ready {
            return Err(Decline::Float);
        }
        let h = k.time_hash.of(handle);
        self.fp.check(k.time_scale);
        self.fp.check(t);
        match (self.env.x87_control >> 8) & 3 {
            // Single: each operation rounds to binary32.
            0 => {
                let p = self.fp.mul(det::from_int(h as i32), k.time_scale);
                Ok(self.fp.add(p, t))
            }
            // Double: the product is exact (40 bits), the sum rounds to
            // 53 bits, the store to 24.
            2 => {
                let p = f64::from(h) * f64::from(f32::from_bits(k.time_scale));
                let s = (p + f64::from(f32::from_bits(t))) as f32;
                let bits = s.to_bits();
                self.fp.check(bits);
                Ok(bits)
            }
            // Extended rounds the sum to 64 bits first: not carried.
            _ => Err(Decline::Float),
        }
    }

    /// `13bb90(function, x, r)`: the curve, then its output range unless
    /// the blob's flags take it raw.
    fn function(&mut self, f: &Function, x: u32, r: u32) -> R<u32> {
        let Some(blob) = f.blob.as_deref() else {
            return Ok(0);
        };
        let k = *self.k();
        let image = Image {
            shapes: LOCAL_SHAPES,
            waves: LOCAL_WAVES,
            ..k.curve
        };
        let tables = &self.lib.tables;
        let byte = |at: u32| -> Result<u8, ()> { local(blob, tables, at) };
        let word = |at: u32| -> Result<u32, ()> {
            let mut b = [0u8; 4];
            for (i, v) in b.iter_mut().enumerate() {
                *v = local(blob, tables, at.wrapping_add(i as u32))?;
            }
            Ok(u32::from_le_bytes(b))
        };
        let v = match curve::evaluate(
            &image,
            self.env.mxcsr,
            self.env.x87_control,
            LOCAL_REF,
            x,
            r,
            word,
            byte,
        ) {
            Ok(curve::Answer::Value(v)) => v,
            Ok(curve::Answer::Declined(curve::Decline::Float)) => return Err(Decline::Float),
            Ok(curve::Answer::Declined(curve::Decline::Kind)) | Err(()) => {
                return Err(Decline::Curve);
            }
        };
        if blob[1] & 0xf0 != 0 {
            return Ok(v);
        }
        let v = self.clamp(v);
        let lo = u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]);
        let hi = u32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]);
        let d = self.fp.sub(hi, lo);
        let d = self.fp.mul(d, v);
        Ok(self.fp.add(d, lo))
    }
}

/// A byte of the curve kernel's synthetic address space: the reference
/// (count 1, the blob's address), the blob, the tables.
fn local(blob: &[u8], tables: &Tables, at: u32) -> Result<u8, ()> {
    let region = |base: u32, bytes: &[u8]| -> Option<u8> {
        let i = at.checked_sub(base)? as usize;
        bytes.get(i).copied()
    };
    region(LOCAL_REF, &LOCAL_REFERENCE)
        .or_else(|| region(LOCAL_BLOB, blob))
        .or_else(|| region(LOCAL_SHAPES, &tables.shapes))
        .or_else(|| region(LOCAL_WAVES, &tables.waves))
        .ok_or(())
}

#[cfg(test)]
mod tests;
