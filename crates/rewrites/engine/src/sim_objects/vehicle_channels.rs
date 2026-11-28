//! The vehicle pose's channels (docs/engine/object-update.md, "Slot pose
//! callees"): `f0760 (object, callback, frame)`, which the vehicle pose
//! slot `f0f50` calls with the per-channel callback `f06e0`; `ret c`.
//!
//! **`f0760`.** Reads the vehicle's body, definition (frame `+14`), model
//! and animation graph definition (`ebx`, frame `+28`), and the body's
//! root velocity (`ba1d0` into frame `+18`). With no graph or no
//! animation block (body `+12a`) it returns. Otherwise it initialises the
//! channel record at frame `+4c` (`1c62f0`); when the animation at
//! `[block+68]` differs from body `+400` and its `+74` and `+78` are set,
//! it stores the animation at `+400`, finds six named channels
//! (`1cb9d0`) into `+404`..`+418` and three indexed ones (`1cc1a0`) into
//! `+41c`..`+424`. Each of the six named channels whose word `+2` is not
//! none binds (`1c6470`) and, when it binds, calls `callback(object,
//! graph, &record, weight, 0, 1.0, frame)` (`ret 1c`) with its weight:
//! body `+35c`; the velocity's projection on the body's basis over the
//! definition's `+1f4`, scaled into 0..1; the speed `+354` or the forward
//! speed over `+1f4` or `+1f8`, clamped to 0..1; and `+360`, `+364` over
//! `+20c`. Then it walks the graph's channel list (`1dceb0`), binding and
//! calling back each one with a weight from body `+398` (a byte over 255,
//! or 1.0 for `ff`), and the three entries at body `+288 + 194` with
//! their weight at `+288`, scaled into 0..1.
//!
//! **`f06e0`.** The callback: with the record's word `+6` not none it
//! resolves its animation (`1daea0`, an animation manager entry); the
//! animation's byte `+12` picks the sample `1c7100` (six arguments,
//! `ret 18`) or, for `ff`, `1c7200` (`ebx` the record, four arguments,
//! `ret 10`).
//!
//! Every SSE instruction runs through `sse`, so MXCSR, the `xmm`
//! registers and the flags are retail's at every call.

use super::alu::{cmp8, cmp16, dec32, inc32, shl};
use super::sse::{self, Ss};
use super::{Cx, Next, Result, unowned};
use crate::datum::layout::DATA;
use crate::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::{CpuAbi, Guest};

const NONE: u32 = u32::MAX;
/// binary32 1.0, the weight scale every callback call pushes.
const ONE: u32 = 0x3f80_0000;
/// Image constants the weights read: `1/255`, the half and 1.0.
const K_45DBB4: u32 = 0x0045_dbb4;
const K_45DBBC: u32 = 0x0045_dbbc;
const K_45DBC0: u32 = 0x0045_dbc0;

/// `(record: ecx)`; `ret`.
const CHANNEL_INIT: u32 = 0x001c_62f0;
/// `(graph: eax, out: ebx, id: edi, kind: edx, kind: stack) -> eax`; `ret 4`.
const CHANNEL_FIND: u32 = 0x001c_b9d0;
/// `(graph: eax, index: edx, out: esi) -> eax`; `ret`.
const CHANNEL_INDEXED: u32 = 0x001c_c1a0;
/// `(record: eax, flags: edx, animation, -1, -1, -1, -1) -> al`; `ret 14`.
const CHANNEL_BIND: u32 = 0x001c_6470;
/// `(iterator: eax, graph: edx) -> al`; `ret`.
const GRAPH_NEXT: u32 = 0x001d_ceb0;
/// The callback's samples and lookup.
const ANIMATION_GET: u32 = 0x001d_aea0;
const SAMPLE: u32 = 0x001c_7100;
const SAMPLE_OVERLAY: u32 = 0x001c_7200;

const AFTER_VELOCITY: u32 = 0xf07d4;
const AFTER_INIT: u32 = 0xf080d;
/// After each named channel's find.
const AFTER_FIND: [u32; 6] = [0xf0856, 0xf087b, 0xf08a0, 0xf08c5, 0xf08e7, 0xf090c];
const AFTER_INDEXED: u32 = 0xf092f;
const AFTER_FIRST: u32 = 0xf0dde;
const AFTER_LIST_BIND: u32 = 0xf0e09;
const AFTER_LIST_CALLBACK: u32 = 0xf0e60;
const AFTER_LIST_NEXT: u32 = 0xf0e7e;
const AFTER_TAIL_BIND: u32 = 0xf0f06;
const AFTER_TAIL_CALLBACK: u32 = 0xf0f2c;

/// Internal labels.
const NAMED: u32 = 0xf094e;
const LIST: u32 = 0xf0d30;
const LIST_ENTRY: u32 = 0xf0de6;
const TAIL: u32 = 0xf0e86;
const TAIL_ENTRY: u32 = 0xf0ea0;
const EXIT: u32 = 0xf0f3e;

/// One named channel: its label, the word that says it is set, its
/// record, the bind's return site, the callback's return site (where an
/// unbound channel joins it) and the next label.
struct Named {
    label: u32,
    word: u32,
    record: u32,
    bound: u32,
    done: u32,
    next: u32,
}

static CHANNELS: [Named; 6] = [
    Named { label: NAMED, word: 0x406, record: 0x404, bound: 0xf0974, done: 0xf099c, next: 0xf099f },
    Named { label: 0xf099f, word: 0x40a, record: 0x408, bound: 0xf09cd, done: 0xf0a8a, next: 0xf0a8d },
    Named { label: 0xf0a8d, word: 0x40e, record: 0x40c, bound: 0xf0abb, done: 0xf0b56, next: 0xf0b59 },
    Named { label: 0xf0b59, word: 0x412, record: 0x410, bound: 0xf0b87, done: 0xf0c43, next: 0xf0c46 },
    Named { label: 0xf0c46, word: 0x416, record: 0x414, bound: 0xf0c6c, done: 0xf0cb8, next: 0xf0cbb },
    Named { label: 0xf0cbb, word: 0x41a, record: 0x418, bound: 0xf0ce1, done: 0xf0d2d, next: LIST },
];

/// The ids `f0760` finds its six named channels by, and the frame slot
/// (from `esp` after the push) each find writes.
const FINDS: [(u32, u32); 6] = [
    (0x0800_0078, 0xa0),
    (0x0400_0079, 0xa4),
    (0x0800_007a, 0xa8),
    (0x0800_007b, 0x9c),
    (0x0c00_007d, 0x28),
    (0x1100_05c9, 0xac),
];
const FIND_KIND: u32 = 0x0700_0101;

/// Every return site of `f0760`.
pub(super) const SITES: [u32; 27] = [
    AFTER_VELOCITY,
    AFTER_INIT,
    AFTER_FIND[0],
    AFTER_FIND[1],
    AFTER_FIND[2],
    AFTER_FIND[3],
    AFTER_FIND[4],
    AFTER_FIND[5],
    AFTER_INDEXED,
    0xf0974,
    0xf099c,
    0xf09cd,
    0xf0a8a,
    0xf0abb,
    0xf0b56,
    0xf0b87,
    0xf0c43,
    0xf0c6c,
    0xf0cb8,
    0xf0ce1,
    0xf0d2d,
    AFTER_FIRST,
    AFTER_LIST_BIND,
    AFTER_LIST_CALLBACK,
    AFTER_LIST_NEXT,
    AFTER_TAIL_BIND,
    AFTER_TAIL_CALLBACK,
];

/// After the callback's animation lookup and its two samples.
pub(super) const CALLBACK_SITES: [u32; 3] = [0xf070e, 0xf0738, 0xf074d];

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

fn ld(g: &Guest, base: u32, off: u32) -> Result<u32> {
    g.u32(at(base, off))
}

fn st(g: &mut Guest, base: u32, off: u32, v: u32) -> Result<()> {
    g.set_u32(at(base, off), v)
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

/// `and r, ffff; shl r, 4; mov r', [r+tags+8]`: a tag's definition, the
/// flags those of the shift.
fn definition(c: &mut CpuAbi, g: &Guest, tag: u32, tags: u32) -> Result<u32> {
    let index = tag & 0xffff;
    logic(c, index, 1 << 31);
    let offset = shl(c, index, 4, 32);
    ld(g, at(offset, tags), 8)
}

/// `test al, al`: whether `al` is set.
fn al(c: &mut CpuAbi) -> bool {
    let v = c.eax & 0xff;
    logic(c, v, 0x80);
    v != 0
}

/// `or edx, -1`.
fn edx_none(c: &mut CpuAbi) {
    c.edx = NONE;
    logic(c, NONE, 1 << 31);
}

/// `xor edx, edx`.
fn edx_zero(c: &mut CpuAbi) {
    c.edx = 0;
    logic(c, 0, 1 << 31);
}

/// `push -1` four times (or `push edx`, none), `push animation`, then
/// `xor edx, edx; lea eax, [esp+off]; call 1c6470`.
fn bind(c: &mut CpuAbi, g: &mut Guest, animation: u32, record: u32, site: u32) -> Result<Next> {
    for _ in 0..4 {
        push(c, g, NONE)?;
    }
    push(c, g, animation)?;
    edx_zero(c);
    c.eax = at(c.esp, record);
    call(CHANNEL_BIND, site)
}

/// `push <record>; push ebx; push edi; call [esp+d0]`: the callback, the
/// record already in a register, the frame, 1.0, 0 and the weight pushed.
fn callback(c: &mut CpuAbi, g: &mut Guest, record: u32, site: u32) -> Result<Next> {
    push(c, g, record)?;
    push(c, g, c.ebx)?;
    push(c, g, c.edi)?;
    let target = ld(g, c.esp, 0xd0)?;
    call(target, site)
}

/// `push frame; push 1.0; push 0`: the three arguments every callback
/// call pushes first.
fn callback_head(c: &mut CpuAbi, g: &mut Guest, frame: u32) -> Result<()> {
    push(c, g, frame)?;
    push(c, g, ONE)?;
    push(c, g, 0)
}

/// `movss xmm, [m]`.
fn load(c: &mut CpuAbi, g: &Guest, r: usize, addr: u32) -> Result<()> {
    sse::load(c, r, g.u32(addr)?);
    Ok(())
}

/// `<op>ss xmm, [m]`.
fn ss(c: &mut CpuAbi, g: &Guest, op: Ss, r: usize, addr: u32) -> Result<()> {
    let b = g.u32(addr)?;
    sse::ss(c, op, r, b);
    Ok(())
}

/// `comiss a, b`, then whether `jbe` is taken.
fn comiss_bel(c: &mut CpuAbi, a: usize, b: usize) -> bool {
    let (x, y) = (sse::lane0(c, a), sse::lane0(c, b));
    sse::comiss(c, x, y);
    sse::below_or_equal(c)
}

/// `f0aed..f0b34` (and `f0bda..f0c21`): after `comiss xmm0, xmm4` with
/// `xmm1` the half, `xmm2` and `xmm3` the definition's `+1f8` and `+1f4`:
/// `xmm0` over `xmm3` (positive) or `xmm2`, then `x * h + h` clamped to
/// 0..1 into frame `+10`; a divisor not above 0 stores the half.
fn ratio(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let from = 'clamp: {
        let divisor = if sse::below_or_equal(c) { 2 } else { 3 };
        if comiss_bel(c, divisor, 4) {
            break 'clamp 1;
        }
        sse::ss_reg(c, Ss::Div, 0, divisor);
        sse::ss_reg(c, Ss::Mul, 0, 1);
        sse::ss_reg(c, Ss::Add, 0, 1);
        sse::movaps(c, 1, 0);
        if !comiss_bel(c, 4, 1) {
            break 'clamp 4;
        }
        load(c, g, 0, K_45DBC0)?;
        if !comiss_bel(c, 1, 0) {
            break 'clamp 0;
        }
        1
    };
    let v = sse::lane0(c, from);
    st(g, c.esp, 0x10, v)
}

/// `f0f3e`: `pop edi; pop esi; pop ebp; pop ebx; add esp, 9c; ret c`.
fn exit(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x9c, false);
    Ok(Next::Ret { pop: 0xc })
}

/// `f0760`'s entry to the root velocity call.
fn entry(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esp = arithmetic(c, c.esp, 0x9c, true);
    push(c, g, c.ebx)?;
    push(c, g, c.ebp)?;
    push(c, g, c.esi)?;
    push(c, g, c.edi)?;
    c.edi = ld(g, c.esp, 0xb0)?;
    c.eax = c.edi & 0xffff;
    logic(c, c.eax, 1 << 31);
    c.esi = c.eax.wrapping_mul(3);
    c.eax = g.u32(points::OBJECTS.base)?;
    c.ecx = ld(g, c.eax, DATA)?;
    c.eax = g.u32(points::TAG_ENTRIES.base)?;
    c.esi = shl(c, c.esi, 2, 32);
    c.ebp = ld(g, at(c.ecx, c.esi), 8)?;
    // The definition, its model (`+38`) and the model's graph (`+4`).
    let tag = g.u32(c.ebp)?;
    c.ecx = definition(c, g, tag, c.eax)?;
    st(g, c.esp, 0x14, c.ecx)?;
    let model = ld(g, c.ecx, 0x38)?;
    c.edx = definition(c, g, model, c.eax)?;
    let graph = ld(g, c.edx, 4)?;
    c.ebx = definition(c, g, graph, c.eax)?;
    push(c, g, 0)?;
    c.ecx = at(c.esp, 0x1c);
    c.eax = c.edi;
    st(g, c.esp, 0x2c, c.ebx)?;
    call(points::OBJECT_ROOT_VELOCITY.base, AFTER_VELOCITY)
}

/// `push 7000101; mov edi, id; mov edx, 7000101; lea ebx, [esp+off];
/// mov eax, esi; call 1cb9d0`.
fn find(c: &mut CpuAbi, g: &mut Guest, i: usize) -> Result<Next> {
    let (id, off) = FINDS[i];
    push(c, g, FIND_KIND)?;
    c.edi = id;
    c.edx = FIND_KIND;
    c.ebx = at(c.esp, off);
    c.eax = c.esi;
    call(CHANNEL_FIND, AFTER_FIND[i])
}

/// `f0920`: `mov eax, [esp+10]; mov edx, edi; lea esi, [esp+24]; call
/// 1cc1a0`.
fn indexed(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.eax = ld(g, c.esp, 0x10)?;
    c.edx = c.edi;
    c.esi = at(c.esp, 0x24);
    call(CHANNEL_INDEXED, AFTER_INDEXED)
}

/// The weight of named channel `i` once it bound, then its callback.
fn named_weight(c: &mut CpuAbi, g: &mut Guest, i: usize) -> Result<Next> {
    let done = CHANNELS[i].done;
    match i {
        0 => {
            c.eax = ld(g, c.esp, 0xb8)?;
            c.ecx = ld(g, c.ebp, 0x35c)?;
            callback_head(c, g, c.eax)?;
            push(c, g, c.ecx)?;
        }
        1 => {
            let b = c.ebp;
            load(c, g, 1, at(b, 0x80))?;
            load(c, g, 0, at(b, 0x84))?;
            ss(c, g, Ss::Mul, 0, at(b, 0x74))?;
            ss(c, g, Ss::Mul, 1, at(b, 0x78))?;
            load(c, g, 2, at(b, 0x7c))?;
            ss(c, g, Ss::Mul, 2, at(b, 0x78))?;
            load(c, g, 3, at(b, 0x70))?;
            ss(c, g, Ss::Mul, 3, at(b, 0x80))?;
            c.eax = ld(g, c.esp, 0xb8)?;
            c.ecx = ld(g, c.esp, 0x14)?;
            sse::ss_reg(c, Ss::Sub, 1, 0);
            load(c, g, 0, at(b, 0x70))?;
            ss(c, g, Ss::Mul, 0, at(b, 0x84))?;
            sse::ss_reg(c, Ss::Sub, 0, 2);
            load(c, g, 2, at(b, 0x7c))?;
            ss(c, g, Ss::Mul, 2, at(b, 0x74))?;
            ss(c, g, Ss::Mul, 0, at(c.esp, 0x1c))?;
            sse::ss_reg(c, Ss::Sub, 2, 3);
            load(c, g, 3, at(c.esp, 0x18))?;
            push(c, g, c.eax)?;
            sse::ss_reg(c, Ss::Mul, 3, 1);
            sse::ss_reg(c, Ss::Add, 3, 0);
            load(c, g, 0, at(c.esp, 0x24))?;
            push(c, g, ONE)?;
            push(c, g, 0)?;
            sse::ss_reg(c, Ss::Mul, 0, 2);
            sse::ss_reg(c, Ss::Add, 3, 0);
            ss(c, g, Ss::Div, 3, at(c.ecx, 0x1f4))?;
            ss(c, g, Ss::Add, 3, K_45DBC0)?;
            ss(c, g, Ss::Mul, 3, K_45DBBC)?;
            push(c, g, c.ecx)?;
            let v = sse::lane0(c, 3);
            st(g, c.esp, 0, v)?;
        }
        2 => {
            c.eax = ld(g, c.esp, 0x14)?;
            load(c, g, 0, at(c.ebp, 0x354))?;
            sse::zero(c, 4);
            let (x, y) = (sse::lane0(c, 0), sse::lane0(c, 4));
            sse::comiss(c, x, y);
            load(c, g, 2, at(c.eax, 0x1f8))?;
            load(c, g, 3, at(c.eax, 0x1f4))?;
            load(c, g, 1, K_45DBBC)?;
            ratio(c, g)?;
            c.eax = ld(g, c.esp, 0xb8)?;
            c.ecx = ld(g, c.esp, 0x10)?;
            callback_head(c, g, c.eax)?;
            push(c, g, c.ecx)?;
        }
        3 => {
            let b = c.ebp;
            load(c, g, 0, at(b, 0x78))?;
            ss(c, g, Ss::Mul, 0, at(c.esp, 0x20))?;
            load(c, g, 1, at(b, 0x74))?;
            ss(c, g, Ss::Mul, 1, at(c.esp, 0x1c))?;
            c.eax = ld(g, c.esp, 0x14)?;
            sse::zero(c, 4);
            load(c, g, 2, at(c.eax, 0x1f8))?;
            load(c, g, 3, at(c.eax, 0x1f4))?;
            sse::ss_reg(c, Ss::Add, 0, 1);
            load(c, g, 1, at(c.esp, 0x18))?;
            ss(c, g, Ss::Mul, 1, at(b, 0x70))?;
            sse::ss_reg(c, Ss::Add, 0, 1);
            let (x, y) = (sse::lane0(c, 0), sse::lane0(c, 4));
            sse::comiss(c, x, y);
            load(c, g, 1, K_45DBBC)?;
            ratio(c, g)?;
            c.eax = ld(g, c.esp, 0xb8)?;
            c.ecx = ld(g, c.esp, 0x10)?;
            callback_head(c, g, c.eax)?;
            push(c, g, c.ecx)?;
        }
        _ => {
            // `+360` (or `+364`) over the definition's `+20c` when that
            // is above 0, else 0.
            let numerator = if i == 4 { 0x360 } else { 0x364 };
            c.eax = ld(g, c.esp, 0x14)?;
            load(c, g, 1, at(c.eax, 0x20c))?;
            sse::zero(c, 0);
            if !comiss_bel(c, 1, 0) {
                load(c, g, 0, at(c.ebp, numerator))?;
                sse::ss_reg(c, Ss::Div, 0, 1);
            }
            c.eax = ld(g, c.esp, 0xb8)?;
            callback_head(c, g, c.eax)?;
            let v = sse::lane0(c, 0);
            st(g, c.esp, 0x1c, v)?;
            c.ecx = ld(g, c.esp, 0x1c)?;
            push(c, g, c.ecx)?;
        }
    }
    c.edx = at(c.esp, 0x5c);
    callback(c, g, c.edx, done)
}

/// `f0d30..f0dd9`: the list iterator at frame `+6c` and the list
/// channel's record at `+2c`, then the first `1dceb0`.
fn list(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    sse::zero(c, 0);
    c.ecx = g.u32(points::TAG_ENTRIES.base)?;
    c.eax = 0;
    logic(c, 0, 1 << 31);
    let s = c.esp;
    for off in [0x3e, 0x40, 0x42] {
        g.set_u16(at(s, off), 0)?;
    }
    c.eax = ld(g, c.esi, 0x68)? & 0xffff;
    logic(c, c.eax, 1 << 31);
    let zero = sse::lane0(c, 0);
    for off in [0x48, 0x74, 0x78, 0x7c] {
        st(g, s, off, zero)?;
    }
    load(c, g, 0, K_45DBC0)?;
    c.eax = shl(c, c.eax, 4, 32);
    st(g, s, 0x6c, c.edx)?;
    st(g, s, 0x8c, c.edx)?;
    for off in [0x90, 0x92, 0x94, 0x96] {
        g.set_u16(at(s, off), c.edx as u16)?;
    }
    st(g, s, 0x2c, c.edx)?;
    g.set_u16(at(s, 0x30), c.edx as u16)?;
    g.set_u16(at(s, 0x32), c.edx as u16)?;
    g.set_u8(at(s, 0x3c), 0)?;
    g.set_u8(at(s, 0x3d), 0)?;
    st(g, s, 0x44, sse::lane0(c, 0))?;
    st(g, s, 0x34, c.edx)?;
    g.set_u16(at(s, 0x3a), c.edx as u16)?;
    g.set_u8(at(s, 0x38), 0xff)?;
    g.set_u8(at(s, 0x39), 0xff)?;
    c.edx = ld(g, at(c.eax, c.ecx), 8)?;
    c.eax = at(s, 0x6c);
    call(GRAPH_NEXT, AFTER_FIRST)
}

/// `f0e60`: the next list entry (`1dceb0`).
fn list_next(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edx = ld(g, c.esi, 0x68)?;
    c.eax = g.u32(points::TAG_ENTRIES.base)?;
    c.edx = definition(c, g, c.edx, c.eax)?;
    c.eax = at(c.esp, 0x6c);
    call(GRAPH_NEXT, AFTER_LIST_NEXT)
}

/// `f0f2c`: the next tail entry, or the exit.
fn tail_next(c: &mut CpuAbi, g: &mut Guest) -> Result<u32> {
    c.eax = ld(g, c.esp, 0x14)?;
    c.ebp = arithmetic(c, c.ebp, 4, false);
    c.eax = dec32(c, c.eax);
    st(g, c.esp, 0x14, c.eax)?;
    Ok(if c.eax != 0 { TAIL_ENTRY } else { EXIT })
}

/// `f0760 (object, callback, frame)`; `ret c`.
pub(super) fn channels(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut pc = pc;
    if pc == points::VEHICLE_POSE_CHANNELS.base {
        return entry(c, g);
    }
    if !SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    loop {
        if let Some(i) = CHANNELS.iter().position(|n| n.label == pc) {
            let n = &CHANNELS[i];
            let word = u32::from(g.u16(at(c.ebp, n.word))?);
            cmp16(c, word, c.edx);
            if word == c.edx & 0xffff {
                pc = n.next;
                continue;
            }
            c.ecx = ld(g, c.esi, 0x68)?;
            arithmetic(c, c.ecx, c.edx, true);
            c.eax = ld(g, c.ebp, n.record)?;
            if c.ecx == c.edx {
                pc = n.next;
                continue;
            }
            return bind(c, g, c.eax, 0x60, n.bound);
        }
        if let Some(i) = CHANNELS.iter().position(|n| n.bound == pc) {
            if !al(c) {
                pc = CHANNELS[i].done;
                continue;
            }
            return named_weight(c, g, i);
        }
        if let Some(i) = CHANNELS.iter().position(|n| n.done == pc) {
            edx_none(c);
            pc = CHANNELS[i].next;
            continue;
        }
        if let Some(i) = AFTER_FIND.iter().position(|&s| s == pc) {
            // `mov r, [eax]; mov [ebp+404+4i], r`.
            let v = g.u32(c.eax)?;
            match i {
                0 | 3 => c.edx = v,
                1 | 4 => c.eax = v,
                _ => c.ecx = v,
            }
            st(g, c.ebp, 0x404 + 4 * i as u32, v)?;
            if i < 5 {
                return find(c, g, i + 1);
            }
            c.edi = 0;
            logic(c, 0, 1 << 31);
            c.ebx = at(c.ebp, 0x41c);
            return indexed(c, g);
        }
        pc = match pc {
            AFTER_VELOCITY => {
                logic(c, c.ebx, 1 << 31);
                if c.ebx == 0 {
                    EXIT
                } else {
                    c.edx = g.u32(points::OBJECTS.base)?;
                    c.eax = ld(g, c.edx, DATA)?;
                    c.ecx = ld(g, at(c.eax, c.esi), 8)?;
                    let block = u32::from(g.u16(at(c.ecx, 0x12a))?);
                    cmp16(c, block, 0xffff);
                    if block == 0xffff {
                        EXIT
                    } else {
                        let off = g.u16(at(c.ebp, 0x12a))? as i16 as i32 as u32;
                        c.esi = arithmetic(c, off, c.ebp, false);
                        c.ecx = at(c.esp, 0x4c);
                        st(g, c.esp, 0x10, c.esi)?;
                        return call(CHANNEL_INIT, AFTER_INIT);
                    }
                }
            }
            AFTER_INIT => {
                c.eax = ld(g, c.esi, 0x68)?;
                c.ecx = ld(g, c.ebp, 0x400)?;
                edx_none(c);
                arithmetic(c, c.eax, c.ecx, true);
                if c.eax == c.ecx {
                    NAMED
                } else {
                    let a = ld(g, c.esi, 0x74)?;
                    arithmetic(c, a, c.edx, true);
                    if a == c.edx {
                        NAMED
                    } else {
                        let b = ld(g, c.esi, 0x78)?;
                        arithmetic(c, b, c.edx, true);
                        if b == c.edx {
                            NAMED
                        } else {
                            st(g, c.ebp, 0x400, c.eax)?;
                            return find(c, g, 0);
                        }
                    }
                }
            }
            AFTER_INDEXED => {
                c.edx = g.u32(c.eax)?;
                g.set_u32(c.ebx, c.edx)?;
                c.edi = inc32(c, c.edi);
                c.ebx = arithmetic(c, c.ebx, 4, false);
                arithmetic(c, c.edi, 3, true);
                if (c.edi as i32) < 3 {
                    return indexed(c, g);
                }
                c.esi = ld(g, c.esp, 0x10)?;
                c.ebx = ld(g, c.esp, 0x28)?;
                c.edi = ld(g, c.esp, 0xb0)?;
                edx_none(c);
                NAMED
            }
            LIST => return list(c, g),
            AFTER_FIRST | AFTER_LIST_NEXT => {
                if al(c) { LIST_ENTRY } else { TAIL }
            }
            LIST_ENTRY => {
                c.ecx = ld(g, c.esi, 0x68)?;
                arithmetic(c, c.ecx, NONE, true);
                if c.ecx == NONE {
                    AFTER_LIST_CALLBACK
                } else {
                    c.edx = ld(g, c.esp, 0x90)?;
                    return bind(c, g, c.edx, 0x40, AFTER_LIST_BIND);
                }
            }
            AFTER_LIST_BIND => {
                if !al(c) {
                    AFTER_LIST_CALLBACK
                } else {
                    c.eax = ld(g, c.esp, 0x8c)?;
                    let weight = g.u8(at(at(c.eax, c.ebp), 0x398))?;
                    c.eax = byte(c.eax, weight);
                    cmp8(c, u32::from(weight), 0xff);
                    if weight == 0xff {
                        load(c, g, 0, K_45DBC0)?;
                    } else {
                        c.ecx = u32::from(weight);
                        sse::cvtsi2ss(c, 0, c.ecx);
                        ss(c, g, Ss::Mul, 0, K_45DBB4)?;
                    }
                    c.edx = ld(g, c.esp, 0xb8)?;
                    callback_head(c, g, c.edx)?;
                    let v = sse::lane0(c, 0);
                    st(g, c.esp, 0x1c, v)?;
                    c.eax = ld(g, c.esp, 0x1c)?;
                    push(c, g, c.eax)?;
                    c.ecx = at(c.esp, 0x3c);
                    return callback(c, g, c.ecx, AFTER_LIST_CALLBACK);
                }
            }
            AFTER_LIST_CALLBACK => return list_next(c, g),
            TAIL => {
                c.ebp = arithmetic(c, c.ebp, 0x288, false);
                st(g, c.esp, 0x14, 3)?;
                TAIL_ENTRY
            }
            TAIL_ENTRY => {
                let word = u32::from(g.u16(at(c.ebp, 0x196))?);
                cmp16(c, word, 0xffff);
                if word == 0xffff {
                    tail_next(c, g)?
                } else {
                    load(c, g, 1, K_45DBBC)?;
                    load(c, g, 0, c.ebp)?;
                    sse::ss_reg(c, Ss::Mul, 0, 1);
                    sse::ss_reg(c, Ss::Add, 0, 1);
                    sse::zero(c, 1);
                    let above = !comiss_bel(c, 1, 0);
                    let v = sse::lane0(c, 0);
                    st(g, c.esp, 0x10, v)?;
                    // `ja`: below 0 stores 0; else above 1.0 stores 1.0.
                    let clamp = above || {
                        load(c, g, 1, K_45DBC0)?;
                        !comiss_bel(c, 0, 1)
                    };
                    if clamp {
                        let v = sse::lane0(c, 1);
                        st(g, c.esp, 0x10, v)?;
                    }
                    c.ecx = ld(g, c.esi, 0x68)?;
                    arithmetic(c, c.ecx, NONE, true);
                    c.eax = ld(g, c.ebp, 0x194)?;
                    if c.ecx == NONE {
                        tail_next(c, g)?
                    } else {
                        return bind(c, g, c.eax, 0x60, AFTER_TAIL_BIND);
                    }
                }
            }
            AFTER_TAIL_BIND => {
                if !al(c) {
                    tail_next(c, g)?
                } else {
                    c.ecx = ld(g, c.esp, 0xb8)?;
                    c.edx = ld(g, c.esp, 0x10)?;
                    callback_head(c, g, c.ecx)?;
                    push(c, g, c.edx)?;
                    c.eax = at(c.esp, 0x5c);
                    return callback(c, g, c.eax, AFTER_TAIL_CALLBACK);
                }
            }
            AFTER_TAIL_CALLBACK => tail_next(c, g)?,
            EXIT => return exit(c, g),
            other => return Err(unowned(other)),
        };
    }
}

/// `f06e0 (object, graph, record, weight, a, scale, frame)`; `ret 1c`.
pub(super) fn channel_callback(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == CALLBACK_SITES[1] || pc == CALLBACK_SITES[2] {
        c.esi = pop(c, g)?;
        c.ebx = pop(c, g)?;
        return Ok(Next::Ret { pop: 0x1c });
    }
    if pc == points::VEHICLE_POSE_CALLBACK.base {
        push(c, g, c.ebx)?;
        push(c, g, c.esi)?;
        c.esi = ld(g, c.esp, 0x14)?;
        c.ebx = NONE;
        logic(c, NONE, 1 << 31);
        c.eax = 0;
        logic(c, 0, 1 << 31);
        let word = u32::from(g.u16(at(c.esi, 6))?);
        cmp16(c, word, c.ebx);
        if word != 0xffff {
            c.eax = g.u32(c.esi)?;
            c.ecx = g.u32(points::TAG_ENTRIES.base)?;
            c.edx = definition(c, g, c.eax, c.ecx)?;
            c.eax = ld(g, c.esi, 4)?;
            push(c, g, c.eax)?;
            return call(ANIMATION_GET, CALLBACK_SITES[0]);
        }
    } else if pc != CALLBACK_SITES[0] {
        return Err(unowned(pc));
    }
    // `f070e`: the animation's byte `+12` picks the sample.
    let kind = g.u8(at(c.eax, 0x12))?;
    c.ecx = byte(c.ecx, kind);
    c.eax = ld(g, c.esp, 0x24)?;
    c.edx = g.u32(c.eax)?;
    cmp8(c, u32::from(kind), c.ebx);
    c.ecx = ld(g, c.eax, 4)?;
    push(c, g, c.ecx)?;
    c.ecx = ld(g, c.esp, 0x24)?;
    push(c, g, c.edx)?;
    if kind as u32 == c.ebx & 0xff {
        c.edx = ld(g, c.esp, 0x20)?;
        push(c, g, c.ecx)?;
        push(c, g, c.edx)?;
        c.edx = ld(g, c.eax, 8)?;
        c.ebx = c.esi;
        return call(SAMPLE_OVERLAY, CALLBACK_SITES[2]);
    }
    c.eax = ld(g, c.eax, 8)?;
    c.edx = ld(g, c.esp, 0x24)?;
    push(c, g, c.eax)?;
    c.eax = ld(g, c.esp, 0x24)?;
    push(c, g, c.ecx)?;
    push(c, g, c.edx)?;
    push(c, g, c.eax)?;
    call(SAMPLE, CALLBACK_SITES[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const STACK: u32 = 0x0070_0000;
    const BODY: u32 = 0x0001_0000;
    const CALLBACK: u32 = 0x0000_c0de;
    const FRAME: u32 = 0x7000;
    const GRAPH: u32 = 0x6200;
    const OBJECT: u32 = 0x8001_0001;
    const ANIMATION: u32 = 0xe003_0003;

    fn f(v: f32) -> u32 {
        v.to_bits()
    }

    fn guest() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0, 0x0080_0000, Permissions::RW).unwrap();
        g.set_u32(K_45DBB4, f(1.0 / 255.0)).unwrap();
        g.set_u32(K_45DBBC, f(0.5)).unwrap();
        g.set_u32(K_45DBC0, f(1.0)).unwrap();
        g
    }

    /// Slot 1's body at [`BODY`], definition tag 0 (`0x6000`) whose model
    /// is tag 1 (`0x6100`) whose graph is tag 2 (`graph`), an animation
    /// block at `+180` whose animation differs from body `+400`.
    fn vehicle(g: &mut Guest, graph: u32) -> CpuAbi {
        let (objects, data, tags) = (0x1000, 0x2000, 0x5000);
        g.set_u32(points::OBJECTS.base, objects).unwrap();
        g.set_u32(objects + DATA, data).unwrap();
        g.set_u32(data + 12 + 8, BODY).unwrap();
        g.set_u32(points::TAG_ENTRIES.base, tags).unwrap();
        for (tag, def) in [(0, 0x6000), (1, 0x6100), (2, graph)] {
            g.set_u32(tags + 16 * tag + 8, def).unwrap();
        }
        g.set_u32(0x6000 + 0x38, 1).unwrap();
        g.set_u32(0x6100 + 4, 2).unwrap();
        g.set_u16(BODY + 0x12a, 0x180).unwrap();
        let block = BODY + 0x180;
        g.set_u32(block + 0x68, ANIMATION).unwrap();
        g.set_u32(block + 0x74, 1).unwrap();
        g.set_u32(block + 0x78, 2).unwrap();
        for (i, v) in [STACK_RET, OBJECT, CALLBACK, FRAME].into_iter().enumerate() {
            g.set_u32(STACK + 4 * i as u32, v).unwrap();
        }
        CpuAbi { esp: STACK, ebx: 0xb0, ebp: 0xb1, esi: 0xb2, edi: 0xb3, mxcsr: 0x1f80, ..Default::default() }
    }

    const STACK_RET: u32 = 0xdead_0000;

    /// One callee as the run made it: its target, `ebx`, `edi` and the
    /// first seven stack words.
    #[derive(Debug)]
    struct Made {
        target: u32,
        ebx: u32,
        edi: u32,
        args: Vec<u32>,
    }

    /// Run from the entry to the return, answering each callee: the
    /// finds and indexed lookups with a pointer to `0x100 + i` and
    /// `0x200 + k`, every bind with 1, the list walk with 0.
    fn run(g: &mut Guest, c: &mut CpuAbi) -> Vec<Made> {
        let mut pc = points::VEHICLE_POSE_CHANNELS.base;
        let mut made = Vec::new();
        let (mut finds, mut indexed) = (0, 0);
        loop {
            match channels(&mut Cx { cpu: &mut *c, g: &mut *g }, pc).unwrap() {
                Next::Call { target, site } => {
                    assert!(SITES.contains(&site), "{site:#x}");
                    let args = (0..7).map(|i| g.u32(c.esp + 4 * i).unwrap()).collect();
                    made.push(Made { target, ebx: c.ebx, edi: c.edi, args });
                    let pop = match target {
                        CHANNEL_FIND => {
                            g.set_u32(0x7100 + 4 * finds, 0x100 + finds).unwrap();
                            c.eax = 0x7100 + 4 * finds;
                            finds += 1;
                            4
                        }
                        CHANNEL_INDEXED => {
                            g.set_u32(0x7200 + 4 * indexed, 0x200 + indexed).unwrap();
                            c.eax = 0x7200 + 4 * indexed;
                            indexed += 1;
                            0
                        }
                        CHANNEL_BIND => {
                            c.eax = 1;
                            0x14
                        }
                        GRAPH_NEXT => {
                            c.eax = 0;
                            0
                        }
                        CALLBACK => 0x1c,
                        t if t == points::OBJECT_ROOT_VELOCITY.base => 4,
                        CHANNEL_INIT => 0,
                        other => panic!("callee {other:#x}"),
                    };
                    c.esp += pop;
                    c.ecx = 0x5555;
                    pc = site;
                }
                Next::Ret { pop } => {
                    assert_eq!(pop, 0xc);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!((c.esp, c.ebx, c.ebp, c.esi, c.edi), (STACK, 0xb0, 0xb1, 0xb2, 0xb3));
        made
    }

    #[test]
    fn with_no_graph_or_no_block_it_returns_after_the_root_velocity() {
        for (graph, block) in [(0, 0x180), (GRAPH, 0xffff)] {
            let mut g = guest();
            let mut c = vehicle(&mut g, graph);
            g.set_u16(BODY + 0x12a, block).unwrap();
            let made = run(&mut g, &mut c);
            let targets: Vec<u32> = made.iter().map(|m| m.target).collect();
            assert_eq!(targets, [points::OBJECT_ROOT_VELOCITY.base]);
            // `ba1d0 (object: eax, linear: ecx, 0)`: the frame's `+18`.
            assert_eq!(made[0].args[0], 0);
            assert_eq!(made[0].edi, OBJECT);
            assert_eq!(g.u32(BODY + 0x400).unwrap(), 0, "nothing stored");
        }
    }

    #[test]
    fn a_changed_animation_finds_the_channels_and_each_live_one_calls_back() {
        let mut g = guest();
        let mut c = vehicle(&mut g, GRAPH);
        // Channel C (`+40e`) none. The tail's records (`+288 + 194`, body
        // `+41c`) are the three indexed lookups' stores (`+404 + 4 * 6`),
        // so the run writes them: `0x200 + k`, each live (high word 0).
        g.set_u16(BODY + 0x40e, 0xffff).unwrap();
        let tail = BODY + 0x288;
        g.set_u32(tail, f(0.5)).unwrap();
        // The weights: `+35c`, and `+360` and `+364` over `+20c`.
        g.set_u32(BODY + 0x35c, f(0.25)).unwrap();
        g.set_u32(BODY + 0x360, f(3.0)).unwrap();
        g.set_u32(BODY + 0x364, f(1.0)).unwrap();
        g.set_u32(0x6000 + 0x20c, f(2.0)).unwrap();
        let made = run(&mut g, &mut c);
        let targets: Vec<u32> = made.iter().map(|m| m.target).collect();
        let mut expected = vec![points::OBJECT_ROOT_VELOCITY.base, CHANNEL_INIT];
        expected.extend([CHANNEL_FIND; 6]);
        expected.extend([CHANNEL_INDEXED; 3]);
        // A, B, D, E and F bind and call back; then the list, then the
        // three tail entries.
        for _ in 0..5 {
            expected.extend([CHANNEL_BIND, CALLBACK]);
        }
        expected.push(GRAPH_NEXT);
        for _ in 0..3 {
            expected.extend([CHANNEL_BIND, CALLBACK]);
        }
        assert_eq!(targets, expected);
        assert_eq!(g.u32(BODY + 0x400).unwrap(), ANIMATION);
        let stored: Vec<u32> = (0..9).map(|i| g.u32(BODY + 0x404 + 4 * i).unwrap()).collect();
        assert_eq!(stored, [0x100, 0x101, 0x102, 0x103, 0x104, 0x105, 0x200, 0x201, 0x202]);
        // The finds' ids, and the binds' animations (the named records).
        let ids: Vec<u32> = made.iter().filter(|m| m.target == CHANNEL_FIND).map(|m| m.args[0]).collect();
        assert_eq!(ids, [FIND_KIND; 6]);
        let binds: Vec<u32> = made.iter().filter(|m| m.target == CHANNEL_BIND).map(|m| m.args[0]).collect();
        assert_eq!(binds, [0x100, 0x101, 0x103, 0x104, 0x105, 0x200, 0x201, 0x202]);
        // Every callback: `(object, graph, &record, weight, 0, 1.0, frame)`.
        let s = STACK - 0xac;
        let calls: Vec<&Made> = made.iter().filter(|m| m.target == CALLBACK).collect();
        for m in &calls {
            assert_eq!((m.edi, m.ebx), (OBJECT, GRAPH));
            assert_eq!([m.args[0], m.args[1], m.args[2]], [OBJECT, GRAPH, s + 0x4c]);
            assert_eq!([m.args[4], m.args[5], m.args[6]], [0, ONE, FRAME]);
        }
        let weights: Vec<u32> = calls.iter().map(|m| m.args[3]).collect();
        assert_eq!(weights[0], f(0.25));
        assert_eq!([weights[3], weights[4]], [f(1.5), f(0.5)]);
        // The tail: 0.5 * 0.5 + 0.5, then 0 * 0.5 + 0.5 twice.
        assert_eq!(&weights[5..], [f(0.75), f(0.5), f(0.5)]);
    }

    #[test]
    fn an_unchanged_animation_skips_the_finds() {
        let mut g = guest();
        let mut c = vehicle(&mut g, GRAPH);
        g.set_u32(BODY + 0x400, ANIMATION).unwrap();
        let made = run(&mut g, &mut c);
        assert!(!made.iter().any(|m| m.target == CHANNEL_FIND || m.target == CHANNEL_INDEXED));
    }

    /// The callback's frame: `(object, graph, record, weight, a, scale,
    /// frame)` at [`STACK`], the record's animation at `0x7400`.
    fn record(g: &mut Guest, live: bool, kind: u8) -> CpuAbi {
        let tags = 0x5000;
        g.set_u32(points::TAG_ENTRIES.base, tags).unwrap();
        g.set_u32(tags + 16 * 3 + 8, 0x6300).unwrap();
        g.set_u32(0x7300, 3).unwrap();
        g.set_u32(0x7304, if live { 0x0001_0055 } else { 0xffff_0055 }).unwrap();
        g.set_u8(0x7412, kind).unwrap();
        for (i, v) in [0xf0, 0xf1, 0xf2].into_iter().enumerate() {
            g.set_u32(0x7500 + 4 * i as u32, v).unwrap();
        }
        let args = [STACK_RET, 0x11, 0x22, 0x7300, 0x33, 0x44, ONE, 0x7500];
        for (i, v) in args.into_iter().enumerate() {
            g.set_u32(STACK + 4 * i as u32, v).unwrap();
        }
        CpuAbi { esp: STACK, ebx: 0xb0, esi: 0xb2, ..Default::default() }
    }

    fn callback_run(g: &mut Guest, c: &mut CpuAbi) -> Vec<(u32, Vec<u32>, [u32; 4])> {
        let mut pc = points::VEHICLE_POSE_CALLBACK.base;
        let mut made = Vec::new();
        loop {
            match channel_callback(&mut Cx { cpu: &mut *c, g: &mut *g }, pc).unwrap() {
                Next::Call { target, site } => {
                    assert!(CALLBACK_SITES.contains(&site));
                    let args = (0..6).map(|i| g.u32(c.esp + 4 * i).unwrap()).collect();
                    made.push((target, args, [c.eax, c.ebx, c.ecx, c.edx]));
                    let pop = match target {
                        ANIMATION_GET => {
                            c.eax = 0x7400;
                            4
                        }
                        SAMPLE => 0x18,
                        SAMPLE_OVERLAY => 0x10,
                        other => panic!("callee {other:#x}"),
                    };
                    c.esp += pop;
                    pc = site;
                }
                Next::Ret { pop } => {
                    assert_eq!(pop, 0x1c);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!((c.esp, c.ebx, c.esi), (STACK, 0xb0, 0xb2));
        made
    }

    #[test]
    fn the_callback_samples_the_record_or_its_overlay() {
        let mut g = guest();
        let mut c = record(&mut g, true, 0);
        let made = callback_run(&mut g, &mut c);
        assert_eq!(made.len(), 2);
        assert_eq!((made[0].0, made[0].1[0], made[0].2[3]), (ANIMATION_GET, 0x0001_0055, 0x6300));
        // `1c7100 (weight, a, scale, frame[2], frame[0], frame[1])`.
        assert_eq!(made[1].0, SAMPLE);
        assert_eq!(made[1].1, [0x33, 0x44, ONE, 0xf2, 0xf0, 0xf1]);
        assert_eq!(made[1].2, [0x33, NONE, ONE, 0x44]);

        let mut g = guest();
        let mut c = record(&mut g, true, 0xff);
        let made = callback_run(&mut g, &mut c);
        // `1c7200 (ebx: record, edx: frame[2]; weight, scale, frame[0], frame[1])`.
        assert_eq!(made[1].0, SAMPLE_OVERLAY);
        assert_eq!(made[1].1[..4], [0x33, ONE, 0xf0, 0xf1]);
        assert_eq!([made[1].2[1], made[1].2[2], made[1].2[3]], [0x7300, ONE, 0xf2]);
    }

    #[test]
    fn a_record_with_no_animation_reads_the_kind_at_0x12() {
        let mut g = guest();
        let mut c = record(&mut g, false, 0);
        g.set_u8(0x12, 0xff).unwrap();
        let made = callback_run(&mut g, &mut c);
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].0, SAMPLE_OVERLAY);
    }
}
