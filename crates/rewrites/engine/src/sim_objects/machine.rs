//! The machine update `1152e0` (descriptor `mach`, slot `+40`) and the
//! machine's slot `+48` query `115a40` (docs/engine/object-update.md,
//! "Slice 11: small types"). Read at their addresses with `h2 disasm`.
//!
//! **`1152e0 (object: stack 0) -> al`, `ret 4`.** A segment body. Frame:
//! `sub esp, c8`, then `push ebx, ebp, esi, edi`; with `esp` after those
//! pushes (the same at every site, since every callee pops its own
//! arguments): the object at `[esp+dc]`, "has an animation driver" byte at
//! `[esp+12]`, the result byte at `[esp+13]`, a scratch dword at
//! `[esp+14]` (the power rate, then the result index, then the word
//! cursor), the definition at `[esp+18]`, the result count (then a float
//! spill) at `[esp+1c]`, the current result's body (then the `fistp` out)
//! at `[esp+20]`, a one at `[esp+24]`, the marker `b8d30` fills at
//! `[esp+28]`, the basis `b9fc0` fills at `[esp+64]`, the centre copy at
//! `[esp+88]`, and the `10`-entry result buffer of the spatial visit at
//! `[esp+98]`. Across calls: `ebp` the body, `ebx` the header offset
//! (`index * c`) until `1154d2` replaces it with the body, `edi` the
//! animation driver (body + body `+12a`) and later the result index,
//! `esi` the definition (reloaded from `[esp+18]` after the calls that take
//! it in `esi`).
//!
//! What it does, in retail order:
//!
//! 1. Body `+c0` bit 12 (the component should follow the machine's power):
//!    when `|body +134|` is at least a small threshold, or bit 10 is set,
//!    and the object has a physics component, `1d24a0(component,
//!    max(+134, 0 or 1))`; the result byte becomes 1.
//! 2. With an animation (`+12a`, `+18c`, `+192` all set) the driver is
//!    `body + (i16) body[+12a]` and `[esp+12]` is 1.
//! 3. Definition `+11c` = 2 (a cycling machine): optionally `1c6620`, then
//!    the x87 rate `(1 - power) * def[+d0] + def[+c8] * power`, spilled to
//!    `[esp+14]`; body `+144` := 0. When the rate is above the threshold:
//!    body `+140` += rate × the tick length, wrapped at 1 (with `1c6920`
//!    and `1c6620` on the wrap), `b58c0(body +d4, 400)` when `+d4` is set,
//!    `107520(object)`, `1c6920` again, and the device group of `+13c`
//!    (`[4e0328]`, `+4`) gets `+140`.
//! 4. Definition `+11c` = 0 (a door): every fourth tick (staggered by the
//!    handle) outside network mode 4, a spatial visit `bb050(1, 3,
//!    location, centre, radius, buffer, 10)` around the machine; for each
//!    blocking result (`b8d30` marker, else `b9fc0` basis, and a dot
//!    product against zero) `[esp+12]` is set, and then `1071e0(group,
//!    1.0)` and body `+1d0` := −3. Then the bit words from `+1da`/`+1dc`:
//!    each entry's bit is set or cleared by the power state, and its word
//!    gets `clamp(position) × scale` through `fistp`; a closed door's
//!    timer `+1d0` counts up and at the definition's `+120` calls
//!    `1071e0(group, 0)`.
//! 5. Body `+12c` bit 2: `bd020(object)`, the bit cleared, result 1. If
//!    that bit or a POST ancestor (header bit 2 up the `+14` chain) is
//!    set: `115b10(eax = object, 0)` and `bba20(object)`, returning 1;
//!    otherwise `115c70(eax = object)`, returning `[esp+13]`.
//!
//! x87: three sequences, kept on `cpu.x87` with `x87::abi`
//! (`fld/fabs/fld/fcomip/fstp st0`, the rate above, and `fld/fistp`).
//!
//! **`115a40 (object: stack 0, bits: stack 1, out: stack 2) -> al`, `ret
//! 0c`.** A leaf. For each entry `i` of the machine's range (`+1da` start,
//! `+1dc` count) it looks up a record (`[[[4e0348]+220]+c]` word `i`, a
//! `24`-byte record from `[[4e0348]+60]`) and tests the two bit indices at
//! its `+0` and `+2` in `bits`; the first hit stores 1 to `[out]`. It
//! returns 1 unless the count is not positive (then 0). `push ecx` makes a
//! local: the range end is stored over it, so `ecx` returns as the end on
//! every path but the empty one; the record base is stored over the
//! object argument.

use super::{Cx, Next, Result, sse, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push};
use runtime::{CpuAbi, Guest};
use sse::Ss::{Add, Mul, Sub};
use x87::abi::{self as fp, arith_op, src_kind};

const NONE: u32 = u32::MAX;

/// Binary32 constants the machine update reads: one (L), the threshold
/// below which a power or rate counts as zero (L), zero (C, as in
/// `sim_native::ray`), and the scale of the words it writes (L).
const ONE: u32 = 0x0045_dbc0;
const EPSILON: u32 = 0x0045_dbdc;
const ZERO: u32 = 0x0045_dbd8;
const WORD_SCALE: u32 = 0x0045_dca4;

/// The device group datum array (`c`-byte elements; the machine's group
/// handle at body `+13c`, its value at element `+4`). L for the name.
const GROUPS: u32 = 0x004e_0328;
/// The structure whose `+220` holds the entry word table (`+c`) and `+60`
/// the `24`-byte records `115a40` reads. L for the name.
const INDEX_OWNER: u32 = 0x004e_0348;
/// The word array the door writes per entry (base `+28`).
const DOOR_WORDS: u32 = 0x004e_d288;
/// The bit set the door sets or clears per entry.
const DOOR_BITS: u32 = 0x004f_93a4;

/// The function pointer `1c6920` is passed.
const CALLBACK: u32 = 0x000b_f600;
/// The marker name `b8d30` looks up.
const MARKER: u32 = 0x0700_0686;

const COMPONENT_UPDATE: u32 = 0x001d_24a0;
const DRIVER_RESET: u32 = 0x001c_6620;
const DRIVER_SET: u32 = 0x001c_6920;
const ATTACHED_EFFECT: u32 = 0x000b_58c0;
const DEVICE_NOTIFY: u32 = 0x0010_7520;
const MARKER_GET: u32 = 0x000b_8d30;
const OBJECT_BASIS: u32 = 0x000b_9fc0;
const GROUP_SET: u32 = 0x0010_71e0;
const OBJECT_RESET: u32 = 0x000b_d020;
const MACHINE_AWAKE: u32 = 0x0011_5b10;
const MACHINE_ASLEEP: u32 = 0x0011_5c70;

/// The retail return addresses of `1152e0`'s calls, in address order.
pub(super) const MACHINE_SITES: [u32; 16] = [
    0x1153b1, 0x115416, 0x1154bc, 0x1154c3, 0x1154f5, 0x115502, 0x11552a, 0x115604,
    0x1156ff, 0x115736, 0x1157cb, 0x11598a, 0x1159bb, 0x115a0f, 0x115a18, 0x115a2e,
];

fn at(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

fn sx16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

/// `[esp + off]`.
fn local(c: &CpuAbi, off: u32) -> u32 {
    c.esp.wrapping_add(off)
}

/// The guest's x87 state as the `x87` crate's.
fn fpu(c: &CpuAbi) -> fp::X87Abi {
    fp::X87Abi {
        st: c.x87.st,
        control: c.x87.control,
        status: c.x87.status,
        tag: c.x87.tag,
        ..Default::default()
    }
}

/// Back into the guest's x87 state, with the last instruction at `pc` and
/// the last memory operand at `data` (the convention of
/// `sim_native::ray::finish_fpu`).
fn fpu_done(c: &mut CpuAbi, g: &Guest, x: &fp::X87Abi, pc: u32, data: u32) -> Result<()> {
    c.x87.st = x.st;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    c.x87.instruction_pointer = pc;
    c.x87.opcode = g.u16(pc)?.swap_bytes() & 0x7ff;
    c.x87.data_pointer = data;
    c.x87.instruction_selector = 8;
    c.x87.data_selector = 16;
    Ok(())
}

/// `fcomip`: ZF, PF and CF from the compare, OF, SF and AF cleared.
fn fcomi_flags(c: &mut CpuAbi, f: u32) {
    c.eflags = (c.eflags & !ARITH) | (f & 0x45);
    c.eflags_defined |= ARITH;
}

/// `lahf; test ah, 44h`: true when `jp` is taken (the compare was unordered
/// or not equal).
fn lahf_test_44(c: &mut CpuAbi) -> bool {
    sse::lahf(c);
    let r = (c.eax >> 8) & 0x44;
    logic(c, r, 0x80);
    sse::parity(c)
}

/// `1152e0 (object: stack 0) -> al`; `ret 4`.
pub(super) fn machine_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::MACHINE_UPDATE.base;
    if pc != entry && !MACHINE_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut next = pc;
    loop {
        next = match next {
            _ if next == entry => {
                c.esp = c.esp.wrapping_sub(0xc8);
                c.eax = g.u32(local(c, 0xcc))? & 0xffff;
                push(c, g, c.ebx)?;
                c.ebx = c.eax * 3;
                c.eax = g.u32(points::OBJECTS.base)?;
                c.ecx = g.u32(at(c.eax, DATA))?;
                c.eax = g.u32(points::TAG_ENTRIES.base)?;
                push(c, g, c.ebp)?;
                c.ebx <<= 2;
                c.ebp = g.u32(at(at(c.ecx, c.ebx), 8))?;
                c.edx = (g.u32(c.ebp)? & 0xffff) << 4;
                push(c, g, c.esi)?;
                c.esi = g.u32(at(at(c.edx, c.eax), 8))?;
                c.eax = u32::from(g.u16(at(c.ebp, 0xc0))?);
                push(c, g, c.edi)?;
                c.edi = 0;
                g.set_u32(local(c, 0x18), c.esi)?;
                g.set_u8(local(c, 0x12), 0)?;
                g.set_u8(local(c, 0x13), 0)?;
                c.ecx = c.eax >> 12;
                if c.ecx & 1 == 0 { 0x1153b6 } else { 0x115340 }
            }
            0x115340 => {
                // fld [ebp+134]; fabs; fld [EPSILON]; fcomip st, st(1); fstp st(0).
                let mut x = fpu(c);
                fp::ld_f32(&mut x, g.u32(at(c.ebp, 0x134))?);
                fp::unary(&mut x, fp::unary_op::ABS);
                fp::ld_f32(&mut x, g.u32(EPSILON)?);
                let f = fp::cmp(&mut x, 0, src_kind::ST, 1, 1, true);
                fp::st_st(&mut x, 0, true);
                fpu_done(c, g, &x, 0x115350, EPSILON)?;
                fcomi_flags(c, f);
                if sse::below_or_equal(c) {
                    0x11535e
                } else {
                    c.edx = c.eax >> 10;
                    if c.edx & 1 == 0 { 0x1153b6 } else { 0x11535e }
                }
            }
            0x11535e => {
                c.ecx = g.u32(at(c.ebp, 0xb4))?;
                if c.ecx == NONE {
                    0x1153b6
                } else {
                    c.edx = g.u32(points::PHYSICS_COMPONENTS.base)?;
                    sse::load(c, 0, g.u32(ONE)?);
                    c.ecx = ((c.ecx & 0xffff) * 5) << 5;
                    c.ecx = at(c.ecx, g.u32(at(c.edx, DATA))?);
                    c.eax >>= 10;
                    if c.eax & 1 == 0 {
                        sse::zero(c, 0);
                    }
                    sse::load(c, 1, g.u32(at(c.ebp, 0x134))?);
                    sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
                    if !sse::below_or_equal(c) {
                        sse::movaps(c, 0, 1);
                    }
                    g.set_u32(local(c, 0x14), sse::lane0(c, 0))?;
                    c.eax = g.u32(local(c, 0x14))?;
                    push(c, g, c.eax)?;
                    push(c, g, c.ecx)?;
                    return Ok(Next::Call { target: COMPONENT_UPDATE, site: 0x1153b1 });
                }
            }
            0x1153b1 => {
                g.set_u8(local(c, 0x13), 1)?;
                0x1153b6
            }
            0x1153b6 => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.ecx = g.u32(at(at(c.edx, c.ebx), 8))?;
                c.eax = NONE;
                if g.u16(at(c.ecx, 0x12a))? != 0xffff
                    && g.u32(at(c.ebp, 0x18c))? != NONE
                    && g.u16(at(c.ebp, 0x192))? != 0xffff
                {
                    c.edi = sx16(g.u16(at(c.ebp, 0x12a))?).wrapping_add(c.ebp);
                    g.set_u8(local(c, 0x12), 1)?;
                }
                // `1153ee`.
                if g.u16(at(c.esi, 0x11c))? != 2 {
                    0x115566
                } else {
                    let al = g.u8(local(c, 0x12))?;
                    c.eax = byte(c.eax, al);
                    if al == 0 {
                        0x11541a
                    } else {
                        logic(c, al.into(), 0x80);
                        c.edx = g.u32(at(c.ebp, 0x140))?;
                        push(c, g, c.edx)?;
                        c.esi = at(c.ebp, 0x18c);
                        return Ok(Next::Call { target: DRIVER_RESET, site: 0x115416 });
                    }
                }
            }
            0x115416 => {
                c.esi = g.u32(local(c, 0x18))?;
                0x11541a
            }
            0x11541a => {
                // The rate, in x87 extended precision, then `|rate| > EPSILON`.
                let mut x = fpu(c);
                fp::ld_f32(&mut x, g.u32(ONE)?);
                sse::zero(c, 3);
                fp::arith(&mut x, arith_op::SUB, 0, src_kind::F32, g.u32(at(c.ebp, 0x134))?.into());
                g.set_u32(at(c.ebp, 0x144), sse::lane0(c, 3))?;
                fp::arith(&mut x, arith_op::MUL, 0, src_kind::F32, g.u32(at(c.esi, 0xd0))?.into());
                fp::ld_f32(&mut x, g.u32(at(c.esi, 0xc8))?);
                fp::arith(&mut x, arith_op::MUL, 0, src_kind::F32, g.u32(at(c.ebp, 0x134))?.into());
                fp::arith(&mut x, arith_op::ADD | fp::ARITH_POP, 1, src_kind::ST, 0);
                let (_, s) = fp::st_f32(&x, 0);
                g.set_u32(local(c, 0x14), s.value)?;
                fp::store_commit(&mut x, s.status, false);
                fp::unary(&mut x, fp::unary_op::ABS);
                fp::ld_f32(&mut x, g.u32(EPSILON)?);
                fp::xch(&mut x, 1);
                let f = fp::cmp(&mut x, 0, src_kind::ST, 1, 1, true);
                fp::st_st(&mut x, 0, true);
                fpu_done(c, g, &x, 0x115455, EPSILON)?;
                fcomi_flags(c, f);
                if sse::below_or_equal(c) { 0x115569 } else { 0x11545d }
            }
            0x11545d => {
                c.eax = g.u32(points::GAME_TIME.base)?;
                sse::load(c, 0, g.u32(at(c.eax, 4))?);
                sse::ss(c, Mul, 0, g.u32(local(c, 0x14))?);
                sse::ss(c, Add, 0, g.u32(at(c.ebp, 0x140))?);
                sse::load(c, 2, g.u32(ONE)?);
                sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2));
                g.set_u32(at(c.ebp, 0x140), sse::lane0(c, 0))?;
                if sse::below(c) {
                    0x1154d2
                } else {
                    let al = g.u8(local(c, 0x12))?;
                    c.eax = byte(c.eax, al);
                    logic(c, al.into(), 0x80);
                    sse::ss_reg(c, Sub, 0, 2);
                    g.set_u32(at(c.ebp, 0x140), sse::lane0(c, 0))?;
                    if al == 0 {
                        0x1154d2
                    } else {
                        c.ecx = g.u32(local(c, 0xdc))?;
                        push(c, g, c.ecx)?;
                        push(c, g, CALLBACK)?;
                        push(c, g, c.edi)?;
                        c.esi = at(c.ebp, 0x18c);
                        push(c, g, 0x3f80_0000)?;
                        return Ok(Next::Call { target: DRIVER_SET, site: 0x1154bc });
                    }
                }
            }
            0x1154bc => {
                push(c, g, 0)?;
                return Ok(Next::Call { target: DRIVER_RESET, site: 0x1154c3 });
            }
            0x1154c3 => {
                sse::zero(c, 3);
                sse::load(c, 2, g.u32(ONE)?);
                c.esi = g.u32(local(c, 0x18))?;
                0x1154d2
            }
            0x1154d2 => {
                c.edx = g.u32(points::OBJECTS.base)?;
                c.eax = g.u32(at(c.edx, DATA))?;
                c.ebx = g.u32(at(at(c.eax, c.ebx), 8))?;
                c.eax = g.u32(at(c.ebx, 0xd4))?;
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    0x1154f5
                } else {
                    push(c, g, 0x400)?;
                    push(c, g, c.eax)?;
                    return Ok(Next::Call { target: ATTACHED_EFFECT, site: 0x1154f5 });
                }
            }
            0x1154f5 => {
                c.ecx = g.u32(local(c, 0xdc))?;
                push(c, g, c.ecx)?;
                return Ok(Next::Call { target: DEVICE_NOTIFY, site: 0x115502 });
            }
            0x115502 => {
                let al = g.u8(local(c, 0x12))?;
                c.eax = byte(c.eax, al);
                logic(c, al.into(), 0x80);
                if al == 0 {
                    0x115539
                } else {
                    c.edx = g.u32(local(c, 0xdc))?;
                    c.eax = g.u32(at(c.ebp, 0x140))?;
                    push(c, g, c.edx)?;
                    push(c, g, CALLBACK)?;
                    push(c, g, c.edi)?;
                    push(c, g, c.eax)?;
                    c.esi = at(c.ebp, 0x18c);
                    return Ok(Next::Call { target: DRIVER_SET, site: 0x11552a });
                }
            }
            0x11552a => {
                sse::zero(c, 3);
                sse::load(c, 2, g.u32(ONE)?);
                c.esi = g.u32(local(c, 0x18))?;
                0x115539
            }
            0x115539 => {
                c.eax = g.u32(at(c.ebp, 0x13c))?;
                if c.eax != NONE {
                    c.edx = g.u32(GROUPS)?;
                    c.eax &= 0xffff;
                    c.ecx = c.eax * 3;
                    c.eax = g.u32(at(c.edx, DATA))?;
                    c.edx = g.u32(at(c.ebp, 0x140))?;
                    g.set_u32(at(at(c.eax, c.ecx * 4), 4), c.edx)?;
                }
                g.set_u8(local(c, 0x13), 1)?;
                0x115571
            }
            0x115566 => {
                sse::zero(c, 3);
                0x115569
            }
            0x115569 => {
                sse::load(c, 2, g.u32(ONE)?);
                0x115571
            }
            0x115571 => {
                if g.u8(at(c.ebp, 0x1cc))? & 1 != 0 {
                    0x1157e4
                } else if g.u16(at(c.esi, 0x11c))? != 0 {
                    0x11599b
                } else {
                    c.eax = g.u32(points::GAME_TIME.base)?;
                    c.ecx = g.u32(at(c.eax, 8))?;
                    c.ecx = at(c.ecx, g.u32(local(c, 0xdc))?);
                    g.set_u8(local(c, 0x13), 1)?;
                    if c.ecx & 3 != 0 {
                        0x1157e4
                    } else {
                        c.edx = g.u32(points::GAME_GLOBALS.base)?;
                        if g.u8(at(c.edx, 0xc))? == 4 { 0x1157e4 } else { 0x1155b9 }
                    }
                }
            }
            0x1155b9 => {
                sse::load(c, 0, g.u32(EPSILON)?);
                let radius = g.u32(at(c.esi, 0x118))?;
                sse::comiss(c, sse::lane0(c, 0), radius);
                g.set_u8(local(c, 0x12), 0)?;
                if sse::below_or_equal(c) {
                    sse::load(c, 0, g.u32(at(c.esi, 0x118))?);
                } else {
                    sse::load(c, 0, g.u32(at(c.ebp, 0x3c))?);
                }
                push(c, g, 0x10)?;
                g.set_u32(local(c, 0x18), sse::lane0(c, 0))?;
                c.edx = g.u32(local(c, 0x18))?;
                c.ecx = local(c, 0x9c);
                push(c, g, c.ecx)?;
                push(c, g, c.edx)?;
                c.eax = at(c.ebp, 0x30);
                push(c, g, c.eax)?;
                c.eax = at(c.ebp, 0x28);
                push(c, g, c.eax)?;
                push(c, g, 3)?;
                push(c, g, 1)?;
                return Ok(Next::Call { target: points::OBJECT_SPATIAL_VISIT.base, site: 0x115604 });
            }
            0x115604 => {
                c.eax = sx16(c.eax as u16);
                c.edi = 0;
                logic(c, c.eax, 1 << 31);
                g.set_u32(local(c, 0x1c), c.eax)?;
                g.set_u32(local(c, 0x14), c.edi)?;
                if (c.eax as i32) <= 0 { 0x1157d5 } else { 0x115620 }
            }
            0x115620 => {
                c.eax = g.u32(at(local(c, 0x98), c.edi.wrapping_mul(4)))?;
                c.edx = g.u32(points::OBJECTS.base)?;
                c.eax &= 0xffff;
                c.ecx = c.eax * 3;
                c.eax = g.u32(at(c.edx, DATA))?;
                c.esi = g.u32(at(at(c.eax, c.ecx * 4), 8))?;
                c.edx = g.u32(points::TAG_ENTRIES.base)?;
                c.eax = at(c.eax, c.ecx * 4);
                c.ecx = (g.u32(c.esi)? & 0xffff) << 4;
                c.edx = g.u32(at(at(c.ecx, c.edx), 8))?;
                c.ecx = 1;
                g.set_u32(local(c, 0x24), c.ecx)?;
                c.ecx = byte(c.ecx, g.u8(at(c.eax, 3))?);
                c.eax = g.u32(local(c, 0x24))?;
                c.eax = c.eax.wrapping_shl(c.ecx & 31);
                g.set_u32(local(c, 0x20), c.esi)?;
                c.ebx = byte(c.ebx, 1);
                if c.eax & 2 == 0 {
                    0x115681
                } else {
                    c.eax = g.u32(at(c.edx, 0x1ec))? >> 20;
                    if c.eax & 1 != 0 {
                        0x115681
                    } else {
                        c.ebx = byte(c.ebx, 0);
                        0x115693
                    }
                }
            }
            0x115681 => {
                let cl = g.u8(at(c.esi, 0x10a))? >> 2;
                c.ecx = byte(c.ecx, cl);
                if cl & 1 != 0 { 0x11579c } else { 0x115693 }
            }
            0x115693 => {
                c.edx = g.u32(at(c.edx, 0xbc))? >> 14;
                if c.edx & 1 != 0 || c.ebx as u8 == 0 {
                    0x11579c
                } else {
                    c.eax = g.u32(at(c.ebp, 0x1cc))?;
                    if c.eax & 4 != 0 {
                        0x1156cc
                    } else if c.eax & 8 == 0 || g.u32(at(c.esi, 0x13c))? == NONE {
                        0x115797
                    } else {
                        0x1156cc
                    }
                }
            }
            0x1156cc => {
                sse::load(c, 0, g.u32(at(c.ebp, 0x140))?);
                sse::ucomiss(c, sse::lane0(c, 0), g.u32(ZERO)?);
                if lahf_test_44(c) {
                    0x115797
                } else {
                    c.eax = g.u32(local(c, 0xdc))?;
                    push(c, g, 1)?;
                    push(c, g, MARKER)?;
                    push(c, g, c.eax)?;
                    c.ecx = byte(c.ecx, 0);
                    logic(c, 0, 0x80);
                    c.ebx = local(c, 0x34);
                    return Ok(Next::Call { target: MARKER_GET, site: 0x1156ff });
                }
            }
            0x1156ff => {
                let ax = c.eax as u16;
                logic(c, ax.into(), 0x8000);
                if (ax as i16) > 0 {
                    0x11573e
                } else {
                    c.eax = at(c.ebp, 0x30);
                    c.ecx = g.u32(c.eax)?;
                    c.edx = g.u32(at(c.eax, 4))?;
                    c.eax = g.u32(at(c.eax, 8))?;
                    g.set_u32(local(c, 0x90), c.eax)?;
                    c.eax = g.u32(local(c, 0xdc))?;
                    c.edi = 0;
                    logic(c, 0, 1 << 31);
                    c.esi = local(c, 0x64);
                    g.set_u32(local(c, 0x88), c.ecx)?;
                    g.set_u32(local(c, 0x8c), c.edx)?;
                    return Ok(Next::Call { target: OBJECT_BASIS, site: 0x115736 });
                }
            }
            0x115736 => {
                c.edi = g.u32(local(c, 0x14))?;
                c.esi = g.u32(local(c, 0x20))?;
                0x11573e
            }
            0x11573e => {
                // The result's centre relative to the machine, along the
                // marker's (or the basis') forward.
                sse::load(c, 0, g.u32(at(c.esi, 0x30))?);
                sse::ss(c, Sub, 0, g.u32(local(c, 0x88))?);
                sse::load(c, 3, g.u32(local(c, 0x64))?);
                sse::load(c, 2, g.u32(at(c.esi, 0x38))?);
                sse::ss(c, Sub, 2, g.u32(local(c, 0x90))?);
                sse::load(c, 1, g.u32(at(c.esi, 0x34))?);
                sse::ss(c, Sub, 1, g.u32(local(c, 0x8c))?);
                sse::ss_reg(c, Mul, 3, 0);
                sse::load(c, 0, g.u32(local(c, 0x6c))?);
                sse::ss_reg(c, Mul, 0, 2);
                sse::ss_reg(c, Add, 3, 0);
                sse::load(c, 0, g.u32(local(c, 0x68))?);
                sse::ss_reg(c, Mul, 0, 1);
                sse::ss_reg(c, Add, 3, 0);
                sse::comiss(c, sse::lane0(c, 3), g.u32(ZERO)?);
                if !sse::below_or_equal(c) { 0x11579c } else { 0x115797 }
            }
            0x115797 => {
                g.set_u8(local(c, 0x12), 1)?;
                0x11579c
            }
            0x11579c => {
                c.eax = g.u32(local(c, 0x1c))?;
                c.edi = c.edi.wrapping_add(1);
                g.set_u32(local(c, 0x14), c.edi)?;
                if (c.edi as i32) < (c.eax as i32) { 0x115620 } else { 0x1157ad }
            }
            0x1157ad => {
                let al = g.u8(local(c, 0x12))?;
                c.eax = byte(c.eax, al);
                if al == 0 {
                    0x1157d5
                } else {
                    c.eax = g.u32(at(c.ebp, 0x13c))?;
                    arithmetic(c, c.eax, NONE, true);
                    if c.eax == NONE {
                        0x1157cb
                    } else {
                        push(c, g, 0x3f80_0000)?;
                        push(c, g, c.eax)?;
                        return Ok(Next::Call { target: GROUP_SET, site: 0x1157cb });
                    }
                }
            }
            0x1157cb => {
                g.set_u32(at(c.ebp, 0x1d0), 0xffff_fffd)?;
                0x1157d5
            }
            0x1157d5 => {
                sse::zero(c, 3);
                sse::load(c, 2, g.u32(ONE)?);
                c.esi = g.u32(local(c, 0x18))?;
                0x1157e4
            }
            0x1157e4 => {
                if g.u16(at(c.esi, 0x11c))? != 0 {
                    0x11599b
                } else {
                    c.eax = g.u32(at(c.ebp, 0x13c))?;
                    c.ebx = byte(c.ebx, 0);
                    if c.eax == NONE {
                        0x115841
                    } else {
                        c.ecx = g.u32(at(c.ebp, 0x12c))?;
                        if (c.ecx >> 8) & 1 == 0 {
                            0x11582a
                        } else {
                            c.edx = g.u32(GROUPS)?;
                            c.eax &= 0xffff;
                            c.ecx = c.eax * 3;
                            c.eax = g.u32(at(c.edx, DATA))?;
                            sse::load(c, 0, g.u32(at(at(c.eax, c.ecx * 4), 4))?);
                            sse::ucomiss(c, sse::lane0(c, 0), sse::lane0(c, 3));
                            if lahf_test_44(c) { 0x11583f } else { 0x11582a }
                        }
                    }
                }
            }
            0x11582a => {
                sse::load(c, 0, g.u32(at(c.ebp, 0x140))?);
                sse::ucomiss(c, sse::lane0(c, 0), sse::lane0(c, 3));
                if lahf_test_44(c) {
                    0x11583f
                } else {
                    c.ebx = byte(c.ebx, 0);
                    0x115841
                }
            }
            0x11583f => {
                c.ebx = byte(c.ebx, 1);
                0x115841
            }
            0x115841 => {
                c.edx = sx16(g.u16(at(c.ebp, 0x1da))?);
                c.ecx = sx16(g.u16(at(c.ebp, 0x1dc))?).wrapping_add(c.edx);
                if (c.edx as i32) >= (c.ecx as i32) {
                    0x11592d
                } else {
                    c.eax = g.u32(DOOR_WORDS)?;
                    c.edi = g.u32(DOOR_BITS)?;
                    sse::load(c, 4, g.u32(WORD_SCALE)?);
                    c.ecx = at(at(c.eax, c.edx.wrapping_mul(2)), 0x28);
                    g.set_u32(local(c, 0x14), c.ecx)?;
                    0x115880
                }
            }
            0x115880 => {
                c.ecx = g.u32(INDEX_OWNER)?;
                c.ecx = g.u32(at(c.ecx, 0x220))?;
                c.ecx = g.u32(at(c.ecx, 0xc))?;
                sse::load(c, 1, g.u32(at(c.esi, 0x124))?);
                sse::load(c, 0, g.u32(at(c.esi, 0x128))?);
                c.eax = c.edx & 0xffff;
                c.ecx = sx16(g.u16(at(c.ecx, c.eax * 2))?);
                c.eax = c.ecx;
                c.ecx &= 0x1f;
                c.eax = ((c.eax as i32) >> 5) as u32;
                c.esi = 1;
                sse::ss_reg(c, Sub, 0, 1);
                sse::ss(c, Mul, 0, g.u32(at(c.ebp, 0x140))?);
                c.esi <<= c.ecx;
                let set = c.ebx as u8 != 0;
                c.ecx = g.u32(at(c.edi, c.eax.wrapping_mul(4)))?;
                c.eax = at(c.edi, c.eax.wrapping_mul(4));
                sse::ss_reg(c, Add, 0, 1);
                if set {
                    c.ecx |= c.esi;
                } else {
                    c.esi = !c.esi;
                    c.ecx &= c.esi;
                }
                sse::comiss(c, sse::lane0(c, 3), sse::lane0(c, 0));
                g.set_u32(c.eax, c.ecx)?;
                if !sse::below_or_equal(c) {
                    sse::movaps(c, 0, 3);
                } else {
                    sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2));
                    if !sse::below_or_equal(c) {
                        sse::movaps(c, 0, 2);
                    }
                }
                sse::ss_reg(c, Mul, 0, 4);
                g.set_u32(local(c, 0x1c), sse::lane0(c, 0))?;
                // fld [esp+1c]; fistp [esp+20].
                let mut x = fpu(c);
                fp::ld_f32(&mut x, g.u32(local(c, 0x1c))?);
                let (_, s) = fp::st_i32(&x, 0);
                let out = local(c, 0x20);
                g.set_u32(out, s.value as u32)?;
                fp::store_commit(&mut x, s.status, true);
                fpu_done(c, g, &x, 0x1158f9, out)?;
                c.eax = g.u32(local(c, 0x14))?;
                c.ecx = (c.ecx & !0xffff) | u32::from(g.u16(local(c, 0x20))?);
                c.esi = g.u32(local(c, 0x18))?;
                g.set_u16(c.eax, c.ecx as u16)?;
                c.ecx = sx16(g.u16(at(c.ebp, 0x1da))?);
                c.eax = at(c.eax, 2);
                g.set_u32(local(c, 0x14), c.eax)?;
                c.eax = sx16(g.u16(at(c.ebp, 0x1dc))?);
                c.edx = c.edx.wrapping_add(1);
                c.eax = c.eax.wrapping_add(c.ecx);
                if (c.edx as i32) < (c.eax as i32) { 0x115880 } else { 0x11592d }
            }
            0x11592d => {
                sse::load(c, 0, g.u32(at(c.ebp, 0x140))?);
                sse::ucomiss(c, sse::lane0(c, 0), sse::lane0(c, 2));
                if lahf_test_44(c) || g.u8(at(c.ebp, 0x1cc))? & 2 != 0 {
                    0x115991
                } else {
                    c.edx = g.u32(points::GAME_GLOBALS.base)?;
                    if g.u8(at(c.edx, 0xc))? == 4 {
                        0x115991
                    } else {
                        c.eax = g.u32(at(c.ebp, 0x1d0))?.wrapping_add(1);
                        g.set_u32(at(c.ebp, 0x1d0), c.eax)?;
                        sse::cvtsi2ss(c, 0, c.eax);
                        c.eax = g.u32(points::GAME_TIME.base)?;
                        sse::ss(c, Mul, 0, g.u32(at(c.eax, 4))?);
                        sse::comiss(c, sse::lane0(c, 0), g.u32(at(c.esi, 0x120))?);
                        if sse::below_or_equal(c) {
                            0x11598a
                        } else {
                            c.eax = g.u32(at(c.ebp, 0x13c))?;
                            arithmetic(c, c.eax, NONE, true);
                            if c.eax == NONE {
                                0x11598a
                            } else {
                                push(c, g, 0)?;
                                push(c, g, c.eax)?;
                                return Ok(Next::Call { target: GROUP_SET, site: 0x11598a });
                            }
                        }
                    }
                }
            }
            0x11598a => {
                g.set_u8(local(c, 0x13), 1)?;
                0x11599b
            }
            0x115991 => {
                g.set_u32(at(c.ebp, 0x1d0), 0)?;
                0x11599b
            }
            0x11599b => {
                c.ebx = g.u32(at(c.ebp, 0x12c))? >> 2;
                c.esi = g.u32(local(c, 0xdc))?;
                let bl = c.ebx as u8 & 1;
                c.ebx = byte(c.ebx, bl);
                logic(c, bl.into(), 0x80);
                if bl == 0 {
                    0x1159c2
                } else {
                    push(c, g, c.esi)?;
                    g.set_u8(local(c, 0x17), 1)?;
                    return Ok(Next::Call { target: OBJECT_RESET, site: 0x1159bb });
                }
            }
            0x1159bb => {
                let v = g.u32(at(c.ebp, 0x12c))? & !4;
                g.set_u32(at(c.ebp, 0x12c), v)?;
                logic(c, v, 1 << 31);
                0x1159c2
            }
            0x1159c2 => {
                let bl = c.ebx as u8;
                logic(c, bl.into(), 0x80);
                c.ebp = g.u32(at(c.ebp, 0x14))?;
                if bl != 0 { 0x115a06 } else { 0x1159d0 }
            }
            0x1159d0 => {
                // Up the parent chain while no ancestor is POST.
                if c.ebp == NONE {
                    let bl = c.ebx as u8;
                    logic(c, bl.into(), 0x80);
                    if bl == 0 { 0x115a27 } else { 0x115a06 }
                } else {
                    c.edx = g.u32(points::OBJECTS.base)?;
                    c.eax = g.u32(at(c.edx, DATA))?;
                    c.ebp &= 0xffff;
                    c.ecx = c.ebp * 3;
                    let dl = g.u8(at(at(c.eax, c.ecx * 4), 2))?;
                    c.edx = byte(c.edx, dl);
                    c.eax = at(c.eax, c.ecx * 4);
                    c.ecx = g.u32(at(c.eax, 8))?;
                    if dl & 4 != 0 {
                        c.ebx = byte(c.ebx, 1);
                    }
                    let bl = c.ebx as u8;
                    logic(c, bl.into(), 0x80);
                    c.ebp = g.u32(at(c.ecx, 0x14))?;
                    if bl == 0 { 0x1159d0 } else { 0x115a06 }
                }
            }
            0x115a06 => {
                push(c, g, 0)?;
                c.eax = c.esi;
                return Ok(Next::Call { target: MACHINE_AWAKE, site: 0x115a0f });
            }
            0x115a0f => {
                c.eax = c.esi;
                c.ebx = byte(c.ebx, 1);
                return Ok(Next::Call { target: points::OBJECT_WAKE_FOR_POST.base, site: 0x115a18 });
            }
            0x115a18 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.eax = byte(c.eax, c.ebx as u8);
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0xc8, false);
                return Ok(Next::Ret { pop: 4 });
            }
            0x115a27 => {
                c.eax = c.esi;
                return Ok(Next::Call { target: MACHINE_ASLEEP, site: 0x115a2e });
            }
            0x115a2e => {
                c.eax = byte(c.eax, g.u8(local(c, 0x13))?);
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0xc8, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(unowned(next)),
        };
    }
}

/// `115a40 (object: stack 0, bits: stack 1, out: stack 2) -> al`; `ret
/// 0c`. A leaf: the exit registers and flags are retail's on every path.
pub(super) fn machine_slot_48(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    if pc != points::MACHINE_SLOT_48.base {
        return Err(unowned(pc));
    }
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    push(c, g, c.ecx)?;
    c.ecx = g.u32(local(c, 8))?;
    c.edx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(at(c.edx, DATA))?;
    c.ecx = (c.ecx & 0xffff) * 3;
    c.ecx = g.u32(at(at(c.edx, c.ecx * 4), 8))?;
    let dx = g.u16(at(c.ecx, 0x1dc))?;
    c.edx = (c.edx & !0xffff) | u32::from(dx);
    c.eax &= !0xff;
    logic(c, dx.into(), 0x8000);
    if (dx as i16) <= 0 {
        // `115b00`.
        c.ecx = pop(c, g)?;
        return Ok(Next::Ret { pop: 0x0c });
    }
    push(c, g, c.esi)?;
    c.esi = sx16(g.u16(at(c.ecx, 0x1da))?);
    c.eax = sx16(dx);
    c.eax = arithmetic(c, c.eax, c.esi, false);
    arithmetic(c, c.esi, c.eax, true);
    push(c, g, c.edi)?;
    c.edi = g.u32(INDEX_OWNER)?;
    // The range end over the local `push ecx` made.
    g.set_u32(local(c, 8), c.eax)?;
    if (c.esi as i32) >= (c.eax as i32) {
        return slot_48_ret(c, g);
    }
    c.eax = g.u32(at(c.edi, 0x220))?;
    c.ecx = g.u32(at(c.edi, 0x60))?;
    c.edi = g.u32(local(c, 0x14))?;
    push(c, g, c.ebx)?;
    c.ebx = g.u32(at(c.eax, 0xc))?;
    // The record base over the object argument.
    g.set_u32(local(c, 0x14), c.ecx)?;
    push(c, g, c.ebp)?;
    loop {
        c.ecx = g.u32(local(c, 0x18))?;
        c.edx = c.esi & 0xffff;
        c.eax = sx16(g.u16(at(c.ebx, c.edx * 2))?);
        c.eax = c.eax.wrapping_mul(9);
        c.edx = at(c.ecx, c.eax.wrapping_mul(4));
        c.eax = sx16(g.u16(c.edx)?);
        c.ecx = c.eax & 0x1f;
        c.ebp = 1 << c.ecx;
        c.eax = ((c.eax as i32) >> 5) as u32;
        let hit = g.u32(at(c.edi, c.eax.wrapping_mul(4)))? & c.ebp;
        logic(c, hit, 1 << 31);
        if hit != 0 {
            break;
        }
        c.eax = sx16(g.u16(at(c.edx, 2))?);
        c.ecx = c.eax & 0x1f;
        c.edx = 1 << c.ecx;
        c.eax = ((c.eax as i32) >> 5) as u32;
        let hit = g.u32(at(c.edi, c.eax.wrapping_mul(4)))? & c.edx;
        logic(c, hit, 1 << 31);
        if hit != 0 {
            break;
        }
        c.eax = g.u32(local(c, 0x10))?;
        c.esi = c.esi.wrapping_add(1);
        arithmetic(c, c.esi, c.eax, true);
        if (c.esi as i32) >= (c.eax as i32) {
            // `115ae9`.
            c.ebp = pop(c, g)?;
            c.ebx = pop(c, g)?;
            return slot_48_ret(c, g);
        }
    }
    // `115af3`: a hit.
    c.eax = g.u32(local(c, 0x20))?;
    c.ebp = pop(c, g)?;
    g.set_u8(c.eax, 1)?;
    c.ebx = pop(c, g)?;
    slot_48_ret(c, g)
}

/// `115afc`: `pop edi; mov al, 1; pop esi; pop ecx; ret 0c`.
fn slot_48_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.eax = byte(c.eax, 1);
    c.esi = pop(c, g)?;
    c.ecx = pop(c, g)?;
    Ok(Next::Ret { pop: 0x0c })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const STACK: u32 = 0x0070_0000;
    const HEADERS: u32 = 0x0060_0000;
    const BODY: u32 = 0x0060_2000;
    const OWNER: u32 = 0x0060_3000;
    const BITS: u32 = 0x0060_7000;
    const OUT: u32 = 0x0060_8000;

    /// A machine with entries 1 and 2: entry 1 names record 0 (bits 5
    /// and 40), entry 2 record 1 (bits 70 and 3).
    fn machine(bit40: bool) -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        g.map_zeroed(0x0040_0000, 0x0040_0000, Permissions::RW).unwrap();
        g.set_u32(points::OBJECTS.base, HEADERS).unwrap();
        g.set_u32(HEADERS + DATA, HEADERS + 0x1000).unwrap();
        g.set_u32(HEADERS + 0x1000 + 2 * 12 + 8, BODY).unwrap();
        g.set_u16(BODY + 0x1da, 1).unwrap();
        g.set_u16(BODY + 0x1dc, 2).unwrap();
        g.set_u32(INDEX_OWNER, OWNER).unwrap();
        g.set_u32(OWNER + 0x220, OWNER + 0x100).unwrap();
        g.set_u32(OWNER + 0x10c, OWNER + 0x200).unwrap();
        g.set_u32(OWNER + 0x60, OWNER + 0x300).unwrap();
        g.set_u16(OWNER + 0x202, 0).unwrap();
        g.set_u16(OWNER + 0x204, 1).unwrap();
        for (record, bits) in [(0u32, [5u16, 40]), (1, [70, 3])] {
            g.set_u16(OWNER + 0x300 + 0x24 * record, bits[0]).unwrap();
            g.set_u16(OWNER + 0x302 + 0x24 * record, bits[1]).unwrap();
        }
        if bit40 {
            g.set_u32(BITS + 4, 1 << 8).unwrap();
        }
        for (i, v) in [0x1234, 0x8000_0002, BITS, OUT].into_iter().enumerate() {
            g.set_u32(STACK + 4 * i as u32, v).unwrap();
        }
        let c = CpuAbi { esp: STACK, eax: 0xaabb_ccdd, ecx: 0x1111, esi: 0x5555, edi: 0x7777, ebx: 0x3333, ebp: 0x4444, ..Default::default() };
        (c, g)
    }

    #[test]
    fn slot_48_without_a_hit_returns_one_and_the_end_in_ecx() {
        let (mut c, mut g) = machine(false);
        let next = machine_slot_48(&mut Cx { cpu: &mut c, g: &mut g }, points::MACHINE_SLOT_48.base).unwrap();
        assert_eq!(next, Next::Ret { pop: 0x0c });
        assert_eq!(c.esp, STACK);
        assert_eq!(c.eax, 1, "the end, then al = 1");
        assert_eq!(c.ecx, 3, "the end stored over the pushed ecx");
        assert_eq!((c.esi, c.edi, c.ebx, c.ebp), (0x5555, 0x7777, 0x3333, 0x4444));
        assert_eq!(g.u8(OUT).unwrap(), 0);
        // The record base was stored over the object argument.
        assert_eq!(g.u32(STACK + 4).unwrap(), OWNER + 0x300);
    }

    #[test]
    fn slot_48_with_a_hit_stores_one_to_out() {
        let (mut c, mut g) = machine(true);
        let next = machine_slot_48(&mut Cx { cpu: &mut c, g: &mut g }, points::MACHINE_SLOT_48.base).unwrap();
        assert_eq!(next, Next::Ret { pop: 0x0c });
        assert_eq!(c.esp, STACK);
        assert_eq!(c.eax, OUT | 1);
        assert_eq!(c.ecx, 3);
        assert_eq!(g.u8(OUT).unwrap(), 1);
        // Bit 40: word 1, bit 8, tested with `edx` = 1 << 8.
        assert_eq!(c.edx, 1 << 8);
    }

    #[test]
    fn slot_48_with_no_entries_returns_zero_and_keeps_ecx() {
        let (mut c, mut g) = machine(false);
        g.set_u16(BODY + 0x1dc, 0).unwrap();
        machine_slot_48(&mut Cx { cpu: &mut c, g: &mut g }, points::MACHINE_SLOT_48.base).unwrap();
        assert_eq!(c.eax, 0xaabb_cc00);
        assert_eq!(c.ecx, 0x1111);
        assert_eq!(c.esp, STACK);
        assert_eq!(c.eflags & 0x40, 0x40, "test dx, dx: zero");
    }
}
