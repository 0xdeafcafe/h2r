//! The ocean's draw-state helpers in Rust over the native targets' host
//! (docs/graphics/retire-d3d-display.md, "The water end"): each is the
//! guest helper's body for the arguments the ocean calls it with, its
//! words written as the guest writes them and its D3D entries made through
//! [`Host`]. `Err` names what a helper could not make.
//!
//! | Guest | Rust |
//! |---|---|
//! | `0x0001cbb0(eax 1, edi vertex type, 1)`: `0x0001cb20`, `0x0001c590` (`0x0001c2e0`), `0x0001cb70` | [`vertex_type`] |
//! | `0x000151c0`: `c[0x32..0x35]` from `0x004c1b38` | [`grid_constants`] |
//! | `0x0001cda0(ecx stream)`: `0x0001c620` on `0x0051f0f0` | [`stream`] |
//! | `0x0001bd50(eax 0x004c1a90)` | [`state_block`] |
//! | `0x0001cdd0(eax 0, 0)`: the identity position decompression | [`position_decompression`] |
//! | `0x00012d50(eax 0, bl 0, 0)`: the viewport push and the projection | [`viewport`] |
//! | `0x0003bf00(eax definition)`: `0x0003c650`, `0x0003d000` | [`structure`] ([`engine::render_loop::structure_parameters`]) |
//! | `0x0001cf50`: the stage commit | [`commit_stages`] |

use engine::memory::GuestMemory;
use engine::render_loop::structure_parameters as structure_upload;

use super::{Cause, TAGS};
use crate::native_targets::{Host, STAGE_COMMITTED, STAGE_REQUESTED};

/// The vertex stream record `0x0001c590`, `0x0001c620` and `0x0001c710`
/// keep.
pub const STREAMS: u32 = 0x0051_f0f0;
/// The built-in vertex types (stride 20: the program handle at `+0`).
pub const VERTEX_BUILTIN: u32 = 0x0046_70ec;
/// `0x000151c0`'s upload: 12 words at `0x004c1b38` to `c[0x32]`, and the
/// word it clears.
pub const GRID_REGISTER: u32 = 0x32;
pub const GRID_CONSTANTS: u32 = 0x004c_1b38;
pub const GRID_DWORDS: u32 = 0xc;
pub const GRID_CLEARED: u32 = 0x0050_93d8;
/// `0x0001bd50`'s cached block and the five words it ors in.
pub const STATE_CACHED: u32 = 0x0048_5a58;
pub const STATE_DIRTY: [(u32, u32, u32); 5] = [
    (0x0048_5b78, 0x0048_508c, 0x0048_507c),
    (0x0048_5b7c, 0x0048_5090, 0x0048_5080),
    (0x0048_5b80, 0x0048_50bc, 0x0048_50ac),
    (0x0048_5b84, 0x0048_50c0, 0x0048_50b0),
    (0x0048_5b88, 0x0048_5068, 0x0048_5060),
];
/// `0x0001cdd0`'s flag (an upload pending) and register.
pub const DECOMPRESSION_PENDING: u32 = 0x0048_5af1;
pub const DECOMPRESSION_REGISTER: u32 = 0xaa;
/// The device pointer, the push buffer's threshold and the device's
/// viewport words `0x00012d50` writes (`+0x1c10`, `+0x1c14`, `+0x1c18`).
pub const DEVICE: u32 = 0x0040_7488;
pub const DEVICE_Z_SCALE: u32 = 0x1c10;
pub const DEVICE_Z_OFFSET: u32 = 0x1c14;
pub const DEVICE_VIEWPORT_FLAGS: u32 = 0x1c18;
/// `0x00012d50`'s inputs: the depth scale and offset words, the camera
/// (`0x0048568c` when EAX is 0), its divisor pair and the cached byte
/// that gates the projection's upload.
pub const ONE: u32 = 0x0045_dbc0;
pub const Z_SCALE: u32 = 0x0048_5adc;
pub const Z_OFFSET: u32 = 0x0048_5ae0;
pub const CAMERA: u32 = 0x0048_568c;
pub const PROJECTION_NUMERATOR: u32 = 0x0045_dd40;
pub const PROJECTION_DIVISOR: u32 = 0x0048_565c;
pub const PROJECTION_CACHED: u32 = 0x0055_e6bc;
/// `0x0003c650`'s bump states per stage, in its order: `BUMPENVMAT00`,
/// `01`, `11`, `10` (`StructureUpload::bump_env`'s order).
pub const BUMP_STATES: [u32; 4] = [0x16, 0x17, 0x19, 0x18];
/// `0x0003d000`'s registers in its order, by index into
/// `StructureUpload::constants` (`c[21..25]`), the fog colour after the
/// third.
pub const STRUCTURE_ORDER: [usize; 5] = [0, 2, 3, 1, 4];
/// The game's x87 control word and MXCSR (docs/lifter/FLOAT.md).
pub const FCW: u16 = 0x027f;
pub const MXCSR: u32 = 0x1f80;

fn word(mem: &dyn GuestMemory, at: u32) -> Result<u32, Cause> {
    mem.u32(at).map_err(|_| "unread")
}

fn byte(mem: &dyn GuestMemory, at: u32) -> Result<u8, Cause> {
    mem.u8(at).map_err(|_| "unread")
}

fn set(mem: &mut dyn GuestMemory, at: u32, v: u32) -> Result<(), Cause> {
    mem.set_u32(at, v).map_err(|_| "unwritable")
}

fn set_byte(mem: &mut dyn GuestMemory, at: u32, v: u8) -> Result<(), Cause> {
    mem.write(at, &[v]).map_err(|_| "unwritable")
}

fn stage(mem: &mut dyn GuestMemory, at: u32, words: &[u32]) -> Result<(), Cause> {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    mem.write(at, &bytes).map_err(|_| "unwritable")
}

fn made(ok: bool) -> Result<(), Cause> {
    if ok { Ok(()) } else { Err("device") }
}

/// Tag `index`'s data (`index & 0xffff`).
fn tag_data(mem: &dyn GuestMemory, index: u32) -> Result<u32, Cause> {
    let table = word(mem, TAGS)?;
    word(mem, table.wrapping_add((index & 0xffff) << 4).wrapping_add(8))
}

/// `0x0001cb20`: the vertex kind `select` picks (its jump table at
/// `0x0001cb60`), `extra` the caller's EAX.
pub fn kind(mem: &dyn GuestMemory, select: u32, tag: u32, extra: u32) -> Result<u32, Cause> {
    match select {
        0 => Ok(0),
        1 => Ok(1),
        2 => Ok(2),
        3 => {
            let count = word(mem, tag_data(mem, tag)?.wrapping_add(4))?;
            let k = extra.wrapping_add(2);
            Ok(if k < count { k } else { 0 })
        }
        _ => Err("vertex_kind"),
    }
}

/// `0x0001c2e0`: the vertex program handle of `tag`'s kind `kind` (a
/// built-in one for tag -1).
pub fn vertex_handle(mem: &dyn GuestMemory, kind: u32, tag: u32) -> Result<u32, Cause> {
    if tag == u32::MAX {
        return word(mem, VERTEX_BUILTIN.wrapping_add(kind.wrapping_mul(20)));
    }
    let kinds = word(mem, tag_data(mem, tag)?.wrapping_add(8))?;
    word(mem, kinds.wrapping_add(kind.wrapping_mul(0x1c)).wrapping_add(0x10))
}

/// `0x0001cbb0(eax extra, edi tag, select)`: the kind, then `0x0001c590`'s
/// program select on [`STREAMS`]. `0x0001cb70`'s mask is its return
/// value only, which the ocean drops.
pub fn vertex_type(mem: &mut dyn GuestMemory, host: &dyn Host, tag: u32, select: u32, extra: u32) -> Result<(), Cause> {
    let kind = kind(&*mem, select, tag, extra)?;
    let handle = vertex_handle(&*mem, kind, tag)?;
    let s = STREAMS;
    set(mem, s + 0x80, 0)?;
    if word(mem, s + 4)? == tag && word(mem, s + 8)? == kind {
        return Ok(());
    }
    for (at, v) in [(s, 0), (s + 4, tag), (s + 8, kind), (s + 0xc, 0)] {
        set(mem, at, v)?;
    }
    made(host.vertex_program(mem, handle, 0))?;
    set(mem, s + 0x84, 0)?;
    set_byte(mem, s + 0x88, 1)?;
    stage(mem, s + 0xcc, &[0; 16])?;
    set_byte(mem, s + 0x20c, 1)?;
    stage(mem, s + 0x210, &[0; 48])?;
    set_byte(mem, s + 0x2d4, 1)?;
    set(mem, s + 0x2d0, 0)
}

/// `0x000151c0`: the grid's twelve words to `c[0x32]`, then
/// [`GRID_CLEARED`] zeroed.
pub fn grid_constants(mem: &mut dyn GuestMemory, host: &dyn Host) -> Result<(), Cause> {
    made(host.constants(mem, GRID_REGISTER, GRID_CONSTANTS, GRID_DWORDS))?;
    set(mem, GRID_CLEARED, 0)
}

/// `0x0001cda0(ecx at)`: the stream at `at` (its format byte `+0` as an
/// entry of `0x0043f408`, stride `0x15`; its index `+1`, buffer `+8` and
/// offset `+0x10`) recorded in [`STREAMS`]'s next slot by `0x0001c620`.
pub fn stream(mem: &mut dyn GuestMemory, at: u32) -> Result<(), Cause> {
    let format = u32::from(byte(mem, at)?) * 0x15 + 0x0043_f408;
    let (index, buffer, offset) = (u32::from(byte(mem, at + 1)?), word(mem, at + 8)?, word(mem, at + 0x10)?);
    let s = STREAMS;
    let n = word(mem, s + 0x2d0)?;
    let slot = s.wrapping_add(n.wrapping_mul(12));
    if word(mem, slot + 0x214)? != index || word(mem, slot + 0x210)? != offset || word(mem, slot + 0x218)? != buffer {
        set(mem, slot + 0x218, buffer)?;
        set(mem, slot + 0x214, index)?;
        set(mem, slot + 0x210, offset)?;
        set_byte(mem, s + 0x2d4, 1)?;
    }
    let old = word(mem, s.wrapping_add(n.wrapping_mul(4)) + 0xcc)?;
    set(mem, s.wrapping_add(n.wrapping_mul(4)) + 0x8c, format)?;
    if old != format {
        set_byte(mem, s + 0x20c, 1)?;
    }
    set(mem, s + 0x2d0, n.wrapping_add(1))
}

/// `0x0001bd50(eax block)`: nothing when `block` is the cached non-null
/// one; else cached, and each dirty word or-ed with its two masks.
pub fn state_block(mem: &mut dyn GuestMemory, block: u32) -> Result<(), Cause> {
    if word(mem, STATE_CACHED)? == block && block != 0 {
        return Ok(());
    }
    set(mem, STATE_CACHED, block)?;
    for (at, a, b) in STATE_DIRTY {
        let v = word(mem, at)? | word(mem, a)? | word(mem, b)?;
        set(mem, at, v)?;
    }
    Ok(())
}

/// `0x0001cdd0(eax 0, 0)`: with no compression the identity block (scale
/// all one, offset zero, the texture coordinate's `(1, 1, 0, 0)`)
/// uploaded to `c[0xaa]` when [`DECOMPRESSION_PENDING`] is set, which it
/// then clears. A compressed vertex type is `position_compression`.
pub fn position_decompression(mem: &mut dyn GuestMemory, host: &dyn Host, scratch: u32, bounds: u32, flags: u8) -> Result<(), Cause> {
    if flags & 3 != 0 && bounds != 0 {
        return Err("position_compression");
    }
    if byte(mem, DECOMPRESSION_PENDING)? == 0 {
        return Ok(());
    }
    let one = word(mem, ONE)?;
    stage(mem, scratch, &[one, one, one, one, 0, 0, 0, 0, one, one, 0, 0])?;
    made(host.constants(mem, DECOMPRESSION_REGISTER, scratch, 0xc))?;
    set_byte(mem, DECOMPRESSION_PENDING, 0)
}

/// `0x00012d50`'s projection of the camera at `camera` into four rows
/// (Code, `0x00012de2..0x00012edc`), in its SSE order.
pub fn projection(mem: &dyn GuestMemory, camera: u32) -> Result<[f32; 16], Cause> {
    let f = |at: u32| mem.f32(at).map_err(|_| "unread");
    let c = |o: u32| f(camera.wrapping_add(o));
    let s = f(PROJECTION_NUMERATOR)? / f(PROJECTION_DIVISOR)?;
    let mut out = [0f32; 16];
    for i in 0..4u32 {
        let p = |o: i32| f(camera.wrapping_add(0x88 + 4 * i).wrapping_add(o as u32));
        let (lo, mid, hi) = (p(-0x10)?, p(0)?, p(0x10)?);
        let row = &mut out[4 * i as usize..4 * i as usize + 4];
        row[0] = (hi * c(0xc)? + c(4)? * lo + mid * c(8)?) * s;
        row[1] = (hi * c(0x18)? + lo * c(0x10)? + mid * c(0x14)?) * s;
        row[2] = (hi * c(0x24)? + c(0x20)? * mid + lo * c(0x1c)?) * s;
        let m3 = (hi * c(0x30)? + c(0x2c)? * mid + lo * c(0x28)?) * s;
        row[3] = p(0x20)? * s + m3;
    }
    Ok(out)
}

/// `0x00012d50(eax 0, bl 0, 0)`: the device's depth scale and offset and
/// its viewport flag 2, the viewport pushed (`0x003f6190`), then the
/// projection of [`CAMERA`] to `c[0..4]` when the cached byte is set, which
/// it then clears.
pub fn viewport(mem: &mut dyn GuestMemory, host: &dyn Host, scratch: u32) -> Result<(), Cause> {
    let device = word(mem, DEVICE)?;
    let scale = mem.f32(ONE).map_err(|_| "unread")? * mem.f32(Z_SCALE).map_err(|_| "unread")?;
    set(mem, device.wrapping_add(DEVICE_Z_SCALE), scale.to_bits())?;
    let offset = word(mem, Z_OFFSET)?;
    set(mem, device.wrapping_add(DEVICE_Z_OFFSET), offset)?;
    let flags = word(mem, device.wrapping_add(DEVICE_VIEWPORT_FLAGS))? | 2;
    set(mem, device.wrapping_add(DEVICE_VIEWPORT_FLAGS), flags)?;
    let put = crate::view_setup::reserve(mem, host, device)?;
    let put = host.push_viewport(mem, put, device).ok_or("device")?;
    set(mem, device, put)?;
    if byte(mem, PROJECTION_CACHED)? == 0 {
        return Ok(());
    }
    let rows = projection(&*mem, CAMERA)?;
    stage(mem, scratch, &rows.map(f32::to_bits))?;
    made(host.registers(mem, 0, scratch, 4))?;
    set_byte(mem, PROJECTION_CACHED, 0)
}

/// `0x0003bf00(eax definition)`: `0x0003c650` and `0x0003d000` for the
/// definition's parameter block ([`structure_upload::upload`]): the
/// thirteen scratch words, stages 1 to 3's bump matrices, then `c[21]`,
/// `c[23]`, `c[24]`, the fog colour, `c[22]` and `c[25]`.
pub fn structure(mem: &mut dyn GuestMemory, host: &dyn Host, scratch: u32, definition: u32) -> Result<(), Cause> {
    let u = structure_upload::upload(&*mem, definition, FCW, MXCSR).map_err(|_| "structure")?;
    stage(mem, structure_upload::SCRATCH, &u.scratch)?;
    for (stage_index, words) in (1u32..).zip(u.bump_env) {
        for (state, value) in BUMP_STATES.into_iter().zip(words) {
            made(host.texture_stage_state(mem, stage_index, state, value))?;
        }
    }
    for (n, &i) in STRUCTURE_ORDER.iter().enumerate() {
        if n == 3 {
            made(host.render_state(mem, crate::view_setup::SLOT_FOG_COLOUR, u.fog_colour))?;
        }
        stage(mem, scratch, &u.constants[i])?;
        made(host.registers(mem, (structure_upload::FIRST_CONSTANT + i) as u32, scratch, 1))?;
    }
    Ok(())
}

/// `0x0001cf50`: each stage whose requested texture is not its committed
/// one set (`SetTexture`) and committed.
pub fn commit_stages(mem: &mut dyn GuestMemory, host: &dyn Host) -> Result<(), Cause> {
    for stage_index in 0..4 {
        let requested = word(mem, STAGE_REQUESTED + 4 * stage_index)?;
        let at = STAGE_COMMITTED + 4 * stage_index;
        if word(mem, at)? != requested {
            made(host.texture(mem, stage_index, requested))?;
            set(mem, at, requested)?;
        }
    }
    Ok(())
}

/// D3D's static vertex shader object: `0x0001c710` copies the format to
/// its `+0x14` and sets it as the input's handle (`| 1`).
pub const STATIC_SHADER: u32 = 0x0040_8518;
/// The record's words `0x0001c330` and `0x0001c710` read: the slot, its
/// vertex type (`slot * 16 + 4`, kind `+ 8`), the program's address and
/// dirty byte, the pending and last formats, the format, its dirty byte,
/// the inputs, their count and their dirty byte.
pub const RECORD_SLOT: u32 = 0x80;
pub const RECORD_PROGRAM: u32 = 0x84;
pub const RECORD_PROGRAM_DIRTY: u32 = 0x88;
pub const RECORD_FORMATS: u32 = 0x8c;
pub const RECORD_LAST_FORMATS: u32 = 0xcc;
pub const RECORD_VAF: u32 = 0x10c;
pub const RECORD_VAF_DIRTY: u32 = 0x20c;
pub const RECORD_INPUTS: u32 = 0x210;
pub const RECORD_COUNT: u32 = 0x2d0;
pub const RECORD_INPUTS_DIRTY: u32 = 0x2d4;
/// The built-in vertex type's registers by semantic (`0x0001c3c1`).
pub const BUILTIN_REGISTERS: [(usize, i32); 3] = [(0, 0), (3, 3), (14, 9)];
/// Semantics `0x0001c330`'s register table holds (`0x15` words).
pub const SEMANTICS: usize = 0x15;

/// `0x0001c330(record, vaf)`: the attribute format at `vaf` (sixteen
/// registers of 16 bytes, each type `D3DVSDT_NONE` 2 first) from the
/// record's pending formats, each register's stream, offset and type, by
/// the semantics of the record's vertex type; each format taken moves to
/// the last formats and clears the format's dirty byte. With no streams
/// all sixteen pending formats are read.
pub fn declaration(mem: &mut dyn GuestMemory, record: u32, vaf: u32) -> Result<(), Cause> {
    let mut out = [0u8; 0x100];
    for r in out.chunks_exact_mut(16) {
        r[8] = 2;
    }
    let mut registers = [-1i32; SEMANTICS];
    let slot = word(mem, record + RECORD_SLOT)?;
    let at = record.wrapping_add(slot.wrapping_mul(16));
    let tag = word(mem, at.wrapping_add(4))?;
    if tag == u32::MAX {
        for (s, r) in BUILTIN_REGISTERS {
            registers[s] = r;
        }
    } else {
        let kinds = word(mem, tag_data(mem, tag)?.wrapping_add(8))?;
        let kind = kinds.wrapping_add(word(mem, at.wrapping_add(8))?.wrapping_mul(0x1c));
        let (count, list) = (word(mem, kind.wrapping_add(4))?, word(mem, kind.wrapping_add(8))?);
        for i in 0..count {
            let s = usize::from(mem.u16(list.wrapping_add(2 * i)).map_err(|_| "unread")?);
            *registers.get_mut(s).ok_or("vertex_declaration")? = i as i32;
        }
    }
    let n = word(mem, record + RECORD_COUNT)? as i32;
    let streams = if n == 0 { 16 } else { n.max(0) as u32 };
    for stream in 0..streams {
        let pending = record + RECORD_FORMATS + 4 * stream;
        let format = word(mem, pending)?;
        if format == 0 {
            continue;
        }
        let mut offset = 0u32;
        for k in 0..10 {
            let semantic = byte(mem, format.wrapping_add(1 + 2 * k))? as i8;
            let code = byte(mem, format.wrapping_add(2 + 2 * k))? as i8;
            if semantic >= 0 {
                let attribute = native_render::object_streams::Attribute::from_code(code as u8).ok_or("vertex_format")?;
                let register = *registers.get(semantic as usize).ok_or("vertex_declaration")?;
                if register >= 0 {
                    let r = out.get_mut(16 * register as usize..16 * register as usize + 16).ok_or("vertex_declaration")?;
                    r[0..4].copy_from_slice(&stream.to_le_bytes());
                    r[4..8].copy_from_slice(&offset.to_le_bytes());
                    r[8..12].copy_from_slice(&attribute.xbox_type().to_le_bytes());
                    r[12] = 0;
                    r[13] = 0;
                }
                // `movzx`: the size's low byte.
                offset = offset.wrapping_add(u32::from(attribute.size() as u8));
            } else if semantic == -2 {
                offset = offset.wrapping_add(i32::from(code) as u32);
            } else {
                break;
            }
        }
        set(mem, pending + 0x40, format)?;
        set(mem, pending, 0)?;
        set_byte(mem, record + RECORD_VAF_DIRTY, 0)?;
    }
    mem.write(vaf, &out).map_err(|_| "unwritable")
}

/// `0x0001c710(record)`: the stream commit. A dirty format is rebuilt
/// ([`declaration`]); a format or inputs still dirty set the inputs
/// (`SetVertexShaderInput`, the static shader with the format copied in,
/// or none without streams); a dirty program or format selects the
/// program at the record's address with the format
/// (`SelectVertexShaderDirect`). The count is cleared, and the format's
/// dirty byte when either was set.
pub fn stream_commit(mem: &mut dyn GuestMemory, host: &dyn Host, record: u32) -> Result<(), Cause> {
    let vaf = record + RECORD_VAF;
    let mut made_any = false;
    if byte(mem, record + RECORD_VAF_DIRTY)? != 0 {
        declaration(mem, record, vaf)?;
    }
    if byte(mem, record + RECORD_VAF_DIRTY)? != 0 || byte(mem, record + RECORD_INPUTS_DIRTY)? != 0 {
        let n = word(mem, record + RECORD_COUNT)?;
        if n as i32 > 0 {
            let mut format = [0u8; 0x100];
            mem.read(vaf, &mut format).map_err(|_| "unread")?;
            mem.write(STATIC_SHADER + 0x14, &format).map_err(|_| "unwritable")?;
            made(host.vertex_input(mem, STATIC_SHADER | 1, n, record + RECORD_INPUTS))?;
        } else {
            made(host.vertex_input(mem, 0, n, 0))?;
        }
        set_byte(mem, record + RECORD_INPUTS_DIRTY, 0)?;
        made_any = true;
    }
    if byte(mem, record + RECORD_PROGRAM_DIRTY)? != 0 || byte(mem, record + RECORD_VAF_DIRTY)? != 0 {
        let address = word(mem, record + RECORD_PROGRAM)?;
        made(host.select_vertex_program(mem, vaf, address))?;
        set_byte(mem, record + RECORD_PROGRAM_DIRTY, 0)?;
        made_any = true;
    }
    set(mem, record + RECORD_COUNT, 0)?;
    if made_any {
        set_byte(mem, record + RECORD_VAF_DIRTY, 0)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
