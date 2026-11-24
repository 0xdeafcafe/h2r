//! The mask caster in `0x00032350`. Its volume, bounds, frusta, records,
//! filter and layer draws use the original callees and their return sites.
//! The posed volume and bounds live in the caster's guest frame because
//! the callees take their addresses. Caller-visible writes follow each
//! callee, including the early return when the bounds miss the view.

use super::super::{LIGHT_VIS, MAIN_VIS, Refusal, Result, frame, guest, passes, visible};
use crate::entry::{Cx, EntryError, GuestCall, Regs, Ret};
use crate::light_constants::{Error, Fpu, Sse, X87};
use crate::light_passes::global::{PASS_FLAGS, PASSES};
use crate::memory::{GuestMemory, MemFault};

/// Locals and the four saved registers below the entry ESP.
pub const FRAME: u32 = 0xb50;
/// Return from the reset, cluster, placement and object record wrapper.
pub const RECORDS_RETURN: u32 = 0x0003_288e;
/// The return site of the no-argument object filter wrapper.
pub const FILTER_RETURN: u32 = 0x0003_2966;

const ORIGIN: u32 = 0x0047_fd18;
const CENTER: u32 = 0x0047_fd24;
const FORWARD: u32 = 0x0047_fd40;
const UP: u32 = 0x0047_fd4c;
const WEIGHT: u32 = 0x0047_fd8c;
const DEFINITION: u32 = 0x0047_fda0;
const HORIZONTAL: u32 = DEFINITION + 0x20;
const NEAR: u32 = DEFINITION + 0x2c;
const FAR: u32 = DEFINITION + 0x30;
const PRIMARY_AXIS: u32 = 0x0046_87ac;
const OTHER_AXIS: u32 = 0x0046_87b4;
const AXIS_CUTOFF: u32 = 0x0045_dc84;
const RADIUS_FLOOR: u32 = 0x0045_df24;
const EXTENT_SCALE: u32 = 0x0045_dc0c;
const HALF: u32 = 0x0045_dbbc;
const UNIT: u32 = 0x0045_dbc0;
const ZERO: u32 = 0x0045_dbd8;
const MIN_WEIGHT: u32 = 0x0045_df88;
const SHADOWS_ENABLED: u32 = 0x004b_a020;
const MASK_MODE: u32 = 0x0050_9413;
const CREATURE_MODE: u32 = MASK_MODE + 1;
const LIGHT_ACTIVE: u32 = 0x0048_58bc;
const LIGHT_WEIGHT: u32 = 0x0048_5a30;
const CURRENT_DEFINITION: u32 = 0x004c_5008;
const RECEIVERS: u32 = 0x004c_1a60;
const CASTER_MASK: u32 = 0x004c_1a80;
const LAYER: u32 = 0x0046_7130;
const TARGET_STATE: u32 = 0x0048_5898;
const CURRENT_TARGET: u32 = 0x0048_58b4;
const VIEW_TARGET: u32 = 0x0048_58b8;

// Immediates of the passes word and the two pushed constants words in
// the mask arm. Read them from the guest image, including in tests.
const MASK_FLAGS: u32 = 0x0003_28cc;
const CONSTANTS_WORD: u32 = 0x0003_28dd;
const CONSTANTS_WEIGHT: u32 = 0x0003_28d6;

#[derive(Clone, Debug)]
pub struct Plan {
    entry_esp: u32,
    fpu: Fpu,
    object: u32,
    headers: u32,
    body: u32,
}

/// Read the sample for a root without asking the render cache to allocate
/// (`0x0003dc90`, `0x0003ddd0`). The original owns a missing cache entry.
fn sample(m: &dyn GuestMemory, body: u32, object: u32) -> Result<u32> {
    if m.u8(body + 7)? & 1 != 0 {
        return guest::at(guest::ptr(m, 0x004d_e2f4)?, 0x1c);
    }
    let id = m.u32(body + 0xcc)?;
    if id == guest::NONE {
        return Err(Refusal::Residency);
    }
    let pool = guest::ptr(m, 0x0050_9434)?;
    let data = guest::ptr(m, guest::at(pool, 0x44)?)?;
    let entry = guest::at(data, (id & 0xffff) << 8)?;
    if m.u32(guest::at(entry, 4)?)? != object || m.u8(guest::at(entry, 3)?)? == 0 {
        return Err(Refusal::Residency);
    }
    guest::at(entry, 0x54)
}

/// Admit a visible root of the mask kind with resident lighting. Planning
/// only reads. Eligibility, extents and the bounds test are real calls
/// during the body and their answers are consumed at their guest sites.
pub fn plan(m: &dyn GuestMemory, entry_esp: u32, fpu: Fpu) -> Result<Plan> {
    visible::fp_supported(fpu)?;
    let s = entry_esp.checked_sub(FRAME + 0x20).ok_or(Refusal::Layout)?;
    m.bytes(s, (FRAME + 0x20) as usize)?;
    let object = m.u32(guest::at(entry_esp, 4)?)?;
    let fade = m.f32(guest::at(entry_esp, 8)?)?;
    let pool = guest::ptr(m, guest::OBJECTS)?;
    let headers = guest::ptr(m, guest::at(pool, 0x44)?)?;
    let header = guest::at(headers, (object & 0xffff) * 12)?;
    let body = guest::ptr(m, guest::at(header, 8)?)?;
    guest::at(body, 0xd0)?;
    if m.u8(SHADOWS_ENABLED)? == 0
        || m.u32(body + 0x14)? != guest::NONE
        || m.u8(body + 0xaa)? == 12
        || m.u8(body + 6)? & 1 != 0
        || m.u8(header + 2)? & 0x10 != 0
        || m.u8(body + 4)? & 1 != 0
    {
        return Err(Refusal::Unsupported);
    }
    let lighting = sample(m, body, object)?;
    let strength = m.f32(guest::at(lighting, 0x1c)?)?;
    m.vec3(guest::at(lighting, 0xc)?)?;
    if !(strength > m.f32(ZERO)? && fade > 0.0) {
        return Err(Refusal::Unsupported);
    }
    m.bytes(body + 0x2c, 0x14)?;
    for slot in [PRIMARY_AXIS, OTHER_AXIS] {
        m.bytes(guest::ptr(m, slot)?, 12)?;
    }
    m.bytes(ORIGIN, (FAR + 4 - ORIGIN) as usize)?;
    for at in [
        AXIS_CUTOFF,
        RADIUS_FLOOR,
        EXTENT_SCALE,
        HALF,
        UNIT,
        MIN_WEIGHT,
        MASK_FLAGS,
        CONSTANTS_WORD,
        CONSTANTS_WEIGHT,
        LIGHT_WEIGHT,
        CURRENT_DEFINITION,
        RECEIVERS + 8,
        CASTER_MASK,
        VIEW_TARGET,
        CURRENT_TARGET,
        TARGET_STATE,
        PASS_FLAGS,
    ] {
        m.u32(at)?;
    }
    m.bytes(LAYER, 12)?;
    m.u8(LIGHT_ACTIVE)?;
    m.u8(PASSES)?;
    m.bytes(MASK_MODE, 2)?;
    let clusters = guest::ptr(m, LIGHT_VIS + 0xc)?;
    m.u16(guest::at(clusters, 4)?)?;
    Ok(Plan {
        entry_esp,
        fpu,
        object,
        headers,
        body,
    })
}

fn fp_error(e: Error) -> EntryError {
    match e {
        Error::Memory(fault) => EntryError::Fault(fault),
        other => EntryError::IsolatedCall(format!("object shadow float operation: {other:?}")),
    }
}

fn words(m: &dyn GuestMemory, at: u32) -> Result<[u32; 3], MemFault> {
    Ok([m.u32(at)?, m.u32(at + 4)?, m.u32(at + 8)?])
}

fn put(m: &mut dyn GuestMemory, at: u32, v: [u32; 3]) -> Result<(), MemFault> {
    for (i, word) in v.into_iter().enumerate() {
        m.set_u32(at + 4 * i as u32, word)?;
    }
    Ok(())
}

// The absolute value here is a comparison and a subtraction, including
// its signed-zero and unordered cases (0x000324c1).
fn magnitude(sse: &mut Sse, v: u32) -> Result<u32, Error> {
    if sse.below(v, 0)? {
        sse.sub(0, v)
    } else {
        Ok(v)
    }
}

fn cross(sse: &mut Sse, a: [u32; 3], b: [u32; 3]) -> Result<[u32; 3], Error> {
    let mut c = [0; 3];
    for (i, j, k) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
        let x = sse.mul(a[j], b[k])?;
        let y = sse.mul(a[k], b[j])?;
        c[i] = sse.sub(x, y)?;
    }
    Ok(c)
}

// 0x0003256b: compare the cutoff to |z*rz + x*rx + y*ry| in
// the caller's x87 precision without a store to float in the middle.
fn primary_axis(fpu: Fpu, a: [u32; 3], b: [u32; 3], cutoff: u32) -> Result<bool, Error> {
    let mut x = X87::new(fpu.x87_control);
    for i in [2, 0, 1] {
        x.fld(a[i])?;
        x.fmul_m(b[i])?;
        if i != 2 {
            x.arith_reg(x87::abi::arith_op::ADD, 1, 0, true)?;
        }
    }
    x.unary(x87::abi::unary_op::ABS)?;
    x.fld(cutoff)?;
    Ok(x.fcomip(1)? & 0x41 == 0)
}

fn projection_axis(m: &mut dyn GuestMemory, s: u32, fpu: Fpu) -> Result<(), Error> {
    let mut sse = Sse { mxcsr: fpu.mxcsr };
    let forward = words(m, FORWARD)?;
    let x = magnitude(&mut sse, forward[0])?;
    let y = magnitude(&mut sse, forward[1])?;
    let lateral = if sse.above(x, y)? { x } else { y };
    let scale = m.u32(EXTENT_SCALE)?;
    let radius = m.u32(s + 0x20)?;
    let scaled = sse.mul(radius, scale)?;
    let end = sse.add(scaled, lateral)?;
    m.set_u32(NEAR, sse.sub(end, radius)?)?;
    let far = sse.add(end, radius)?;
    m.set_u32(FAR, sse.sub(far, radius)?)?;
    let horizontal = m.u32(s + 0x24)?;
    m.set_u32(HORIZONTAL, sse.mul(horizontal, scale)?)?;
    let reference = words(m, m.u32(PRIMARY_AXIS)?)?;
    let reference = if primary_axis(fpu, forward, reference, m.u32(AXIS_CUTOFF)?)? {
        reference
    } else {
        words(m, m.u32(OTHER_AXIS)?)?
    };
    put(m, UP, cross(&mut sse, forward, reference)?)?;
    Ok(())
}

/// The transform and half extents given to the bounds test (ESI = S+0x2c)
/// and subsequently the visibility gather (centre = S+0x54).
fn posed_bounds(m: &mut dyn GuestMemory, s: u32, fpu: Fpu) -> Result<(), Error> {
    let mut sse = Sse { mxcsr: fpu.mxcsr };
    let forward = words(m, FORWARD)?;
    let up = words(m, UP)?;
    let half = m.u32(HALF)?;
    let depth = sse.mul(m.u32(FAR)?, half)?;
    for offset in [0x64, 0x68] {
        let extent = sse.mul(m.u32(s + offset + 0x60)?, half)?;
        m.set_u32(s + offset, extent)?;
    }
    put(m, s + 0x3c, cross(&mut sse, up, forward)?)?;
    m.set_u32(s + 0x60, depth)?;
    put(m, s + 0x30, forward)?;
    put(m, s + 0x48, up)?;
    let depth = sse.sub(0, depth)?;
    let origin = words(m, ORIGIN)?;
    let center = words(m, CENTER)?;
    for i in 0..3 {
        let offset = sse.mul(forward[i], depth)?;
        let end = sse.add(offset, center[i])?;
        let delta = sse.sub(origin[i], end)?;
        let value = if i == 1 {
            sse.add(center[i], delta)?
        } else {
            sse.add(delta, center[i])?
        };
        m.set_u32(s + 0x54 + i as u32 * 4, value)?;
    }
    let unit = m.u32(UNIT)?;
    m.set_u32(s + 0x2c, unit)?;
    Ok(())
}

fn visibility_radius(m: &dyn GuestMemory, s: u32, control: u16) -> Result<u32, Error> {
    let mut x = X87::new(control);
    for offset in [0x68, 0x64, 0x60] {
        let v = m.u32(s + offset)?;
        x.fld(v)?;
        x.fmul_m(v)?;
        if offset != 0x68 {
            x.arith_reg(x87::abi::arith_op::ADD, 1, 0, true)?;
        }
    }
    x.unary(x87::abi::unary_op::SQRT)?;
    x.fstp_m32()
}

impl Plan {
    async fn call(&self, cx: &Cx, site: u32, request: GuestCall) -> Result<Regs, EntryError> {
        passes::named(cx, self.entry_esp, FRAME, site, request).await
    }
}

fn low(eax: u32, byte: u8) -> u32 {
    (eax & !0xff) | u32::from(byte)
}

fn missed(cx: &Cx, s: u32, eax: u32) -> Result<Ret, EntryError> {
    let byte = cx.read(|m| m.u8(s + 0x13))?;
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u8(MASK_MODE, 1)?;
        m.set_u8(CREATURE_MODE, 0)
    })?;
    Ok(Ret::new(low(eax, byte), 8))
}

/// Run the admitted caster, returning EAX and popping both stack words.
/// Record building goes through 0x0002cbc0, and item building through
/// the installed light-kind body at SCENE_ITEMS. Each call keeps the
/// return site the layer, filter and gather observers see.
pub async fn run(cx: &Cx, p: Plan) -> Result<Ret, EntryError> {
    let s = p.entry_esp - FRAME;
    let object = p.object;
    let body = p.body;
    let mut sse = Sse { mxcsr: p.fpu.mxcsr };
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u32(s + 0xc, cx.entry().ebx)?;
        m.set_u32(s + 8, cx.entry().ebp)?;
        m.set_u8(s + 0x13, 0)?;
        m.set_u32(s + 0x14, 0)
    })?;
    // The first call precedes the pushes of ESI and EDI.
    let req = GuestCall::new(0x0003_d7f0)
        .eax(s + 0x1b)
        .ecx(s + 0x14)
        .edx(p.headers)
        .ebx(body)
        .xmm0(0)
        .args([object, s + 0x14, s + 0x1b]);
    let eligible = passes::named(cx, p.entry_esp, FRAME - 8, 0x0003_2394, req).await?;
    cx.write(|m| -> Result<(), Error> {
        let fade = sse.mul(m.u32(s + 0x14)?, m.u32(p.entry_esp + 8)?)?;
        Ok(m.set_u32(s + 0x14, fade)?)
    })
    .map_err(fp_error)?;
    if eligible.eax as u8 == 0 || cx.read(|m| m.u32(body + 0x14))? != guest::NONE {
        return Ok(Ret::new(low(eligible.eax, cx.read(|m| m.u8(s + 0x13))?), 8));
    }
    let enabled = cx.read(|m| m.u8(SHADOWS_ENABLED))?;
    if enabled == 0 {
        return Ok(Ret::new(low(eligible.eax, cx.read(|m| m.u8(s + 0x13))?), 8));
    }
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u32(s + 4, cx.entry().esi)?;
        m.set_u32(s, cx.entry().edi)
    })?;
    let fade = cx.read(|m| m.u32(s + 0x14))?;
    let req = GuestCall::new(0x0003_d7c0)
        .eax(low(eligible.eax, enabled))
        .esi(object)
        .edi(s + 0x1c)
        .ebx(body)
        .xmm0(fade.into());
    let visible = p.call(cx, 0x0003_23d5, req).await?;
    if visible.eax as u8 == 0 {
        return Ok(Ret::new(low(visible.eax, cx.read(|m| m.u8(s + 0x13))?), 8));
    }
    // Eligibility sets the creature output exactly for object type 12.
    // The plan leaves that type to the original before its first write.
    let radius = cx
        .write(|m| -> Result<u32, Error> {
            let body_radius = m.u32(body + 0x3c)?;
            let floor = m.u32(RADIUS_FLOOR)?;
            let r = if sse.above(body_radius, floor)? {
                body_radius
            } else {
                floor
            };
            m.set_u32(s + 0x20, r)?;
            m.set_u32(s + 0x24, r)?;
            Ok(r)
        })
        .map_err(fp_error)?;
    let req = GuestCall::new(0x0003_dc90)
        .args([object])
        .eax(low(visible.eax, 0))
        .ebx(body)
        .esi(object)
        .edi(s + 0x1c)
        .xmm0(radius.into());
    let lighting = p.call(cx, 0x0003_2410, req).await?;
    let not_vehicle = cx.read(|m| m.u8(body + 0xaa))? != 1;
    cx.write(|m| m.set_u8(s + 0x28, not_vehicle as u8))?;
    if lighting.eax == 0 {
        return missed(cx, s, lighting.eax);
    }
    let strong = cx
        .write(|m| -> Result<bool, Error> {
            let strength = m.u32(lighting.eax.wrapping_add(0x1c))?;
            if !sse.above(strength, m.u32(ZERO)?)? {
                return Ok(false);
            }
            let weight = sse.mul(strength, m.u32(s + 0x14)?)?;
            m.set_u32(s + 0x14, weight)?;
            sse.above(weight, m.u32(MIN_WEIGHT)?)
        })
        .map_err(fp_error)?;
    if !strong {
        return missed(cx, s, lighting.eax);
    }
    let direction = lighting.eax.wrapping_add(0xc);
    let center = body + 0x30;
    let req = GuestCall::new(0x0003_1f50)
        .eax(s + 0x20)
        .ecx(s + 0x24)
        .edx(low(lighting.edx, not_vehicle as u8))
        .esi(direction)
        .edi(center)
        .ebx(body)
        .args([object, center, direction, s + 0x24, s + 0x20]);
    p.call(cx, 0x0003_2469, req).await?;
    let (position, forward) = cx.write(|m| -> Result<_, MemFault> {
        let position = words(m, center)?;
        put(m, CENTER, position)?;
        put(m, ORIGIN, position)?;
        let v = words(m, direction)?;
        put(m, FORWARD, v)?;
        Ok((position, v))
    })?;
    let pose_call = |target| {
        GuestCall::new(target)
            .ebx(body)
            .esi(direction)
            .edi(position[2])
    };
    p.call(
        cx,
        0x0003_24b6,
        pose_call(0x0003_0bf0)
            .eax(FORWARD)
            .ecx(forward[2])
            .edx(forward[0]),
    )
    .await?;
    cx.write(|m| projection_axis(m, s, p.fpu))
        .map_err(fp_error)?;
    p.call(cx, 0x0003_2645, pose_call(0x0003_0bf0).eax(UP))
        .await?;
    p.call(
        cx,
        0x0003_2653,
        pose_call(0x000c_1930).eax(DEFINITION).edx(s + 0x6c),
    )
    .await?;
    p.call(
        cx,
        0x0003_2666,
        pose_call(0x000c_1a80)
            .ecx(ORIGIN)
            .edx(s + 0x6c)
            .args([DEFINITION]),
    )
    .await?;
    cx.write(|m| posed_bounds(m, s, p.fpu)).map_err(fp_error)?;
    let up = cx.read(|m| words(m, UP))?;
    let req = GuestCall::new(0x0003_20b0)
        .eax(up[1])
        .ecx(up[2])
        .edx(up[0])
        .ebx(body)
        .edi(position[2])
        .esi(s + 0x2c);
    let bounds = p.call(cx, 0x0003_27ee, req).await?;
    if bounds.eax as u8 == 0 {
        return missed(cx, s, bounds.eax);
    }

    let volume = s + 0x6c;
    let frusta = s + 0xe8;
    let from_bounds = |target| {
        GuestCall::new(target)
            .ebx(body)
            .edi(position[2])
            .esi(s + 0x2c)
    };
    let weight = cx.write(|m| -> Result<u32, MemFault> {
        let w = m.u32(s + 0x14)?;
        m.set_u8(LIGHT_ACTIVE, 0)?;
        m.set_u32(WEIGHT, w)?;
        Ok(w)
    })?;
    let req = from_bounds(frame::call::LIGHT_FRUSTA)
        .edx(s + 0x1c)
        .eax(frusta)
        .ecx(volume)
        .xmm0(weight.into())
        .args([ORIGIN, volume, frusta, s + 0x1c]);
    p.call(cx, 0x0003_2827, req).await?;
    let (count, cluster, radius) = cx
        .read(|m| -> Result<_, Error> {
            Ok((
                m.u16(s + 0x1c)? as i16 as i32 as u32,
                m.u16(body + 0x2c)? as i16 as i32 as u32,
                visibility_radius(m, s, p.fpu.x87_control)?,
            ))
        })
        .map_err(fp_error)?;
    let req = from_bounds(frame::call::SCENE_VISIBILITY)
        .eax(2)
        .ecx(count)
        .edx(frusta & !0xff)
        .args([0, frusta, s + 0x54, radius, MAIN_VIS, 0, cluster]);
    p.call(cx, 0x0003_2875, req).await?;
    let gather = |target| {
        GuestCall::new(target)
            .ebx(body)
            .edi(LIGHT_VIS)
            .esi(s + 0x2c)
    };
    p.call(cx, 0x0003_2884, gather(0x0016_5a20).args([object, 0, 0]))
        .await?;
    // Only the low byte of the records argument was set from the object type.
    let record_mode = cx.read(|m| m.u32(s + 0x28))?;
    p.call(
        cx,
        RECORDS_RETURN,
        gather(0x0002_cbc0).eax(record_mode).args([record_mode]),
    )
    .await?;
    let parts = p
        .call(cx, 0x0003_2893, gather(frame::call::PART_VISIBILITY))
        .await?;
    let clusters = cx.read(|m| m.u32(LIGHT_VIS + 0xc))?;
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u8(PASSES, 1)?;
        m.set_u32(PASS_FLAGS, 0)
    })?;
    let eax = if cx.read(|m| m.u16(clusters.wrapping_add(4)))? as i16 > 0 {
        cx.write(|m| -> Result<(), MemFault> {
            let flags = m.u32(MASK_FLAGS)?;
            m.set_u32(CURRENT_DEFINITION, DEFINITION)?;
            m.set_u32(PASS_FLAGS, flags)
        })?;
        p.call(
            cx,
            0x0003_28d5,
            gather(0x0003_0ca0).eax(low(parts.eax, 0)).ecx(5),
        )
        .await?;
        let (word, weight) = cx.read(|m| -> Result<_, MemFault> {
            Ok((m.u32(CONSTANTS_WORD)?, m.u32(CONSTANTS_WEIGHT)?))
        })?;
        let req = gather(passes::call::CONSTANTS)
            .ecx(volume)
            .eax(ORIGIN)
            .edx(guest::NONE)
            .args([DEFINITION, word, 1, weight]);
        p.call(cx, 0x0003_28f7, req).await?;
        let shadow = |target| {
            GuestCall::new(target)
                .ebx(ORIGIN)
                .edi(LIGHT_VIS)
                .esi(s + 0x2c)
        };
        p.call(
            cx,
            0x0003_290a,
            shadow(passes::call::SHADOW_SETUP)
                .ecx(volume)
                .args([DEFINITION]),
        )
        .await?;
        let weight = cx.read(|m| m.u32(s + 0x14))?;
        cx.write(|m| m.set_u32(LIGHT_WEIGHT, weight))?;
        p.call(
            cx,
            0x0003_291f,
            shadow(passes::call::ITEMS).eax(0).xmm0(weight.into()),
        )
        .await?;
        layers(cx, &p, s).await?
    } else {
        parts.eax
    };
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u8(PASSES, 0)?;
        m.set_u32(PASS_FLAGS, 0)?;
        m.set_u8(MASK_MODE, 1)?;
        m.set_u8(CREATURE_MODE, 0)
    })?;
    Ok(Ret::new(low(eax, 1), 8))
}

async fn layers(cx: &Cx, p: &Plan, s: u32) -> Result<u32, EntryError> {
    let esi = s + 0x2c;
    let casters = cx.read(|m| m.u32(CASTER_MASK))?;
    let mut ebx = 1;
    let mut edi = LIGHT_VIS;
    if casters & (1 << 10) != 0 {
        let receivers = cx.read(|m| m.u32(RECEIVERS + 8))? >> 11;
        if receivers & 1 != 0 {
            edi = 10;
            let layer = |target| GuestCall::new(target).esi(esi).ebx(1).edi(10);
            let begin = layer(passes::call::LAYER_BEGIN)
                .eax(low(receivers, !(receivers as u8)))
                .edx(low(casters >> 10, !((casters >> 10) as u8)));
            if p.call(cx, 0x0003_2953, begin).await?.eax as u8 != 0 {
                p.call(cx, 0x0003_295c, layer(passes::call::LAYER_DRAW))
                    .await?;
                p.call(cx, 0x0003_2961, layer(passes::call::LAYER_END))
                    .await?;
            }
            p.call(cx, FILTER_RETURN, layer(0x0002_22a0)).await?;
            let receivers = cx.read(|m| m.u32(RECEIVERS + 8))?;
            let draw = (receivers >> 11) as u8 & 1;
            cx.write(|m| m.set_u8(s + 0x13, draw))?;
            if draw != 0 {
                let target = cx.write(|m| -> Result<u32, MemFault> {
                    let target = m.u32(VIEW_TARGET)?;
                    m.set_u32(CURRENT_TARGET, target)?;
                    m.set_u32(LAYER, 11)?;
                    m.set_u32(LAYER + 4, 1)?;
                    m.set_u32(LAYER + 8, RECEIVERS)?;
                    Ok(target)
                })?;
                p.call(
                    cx,
                    0x0003_29a4,
                    layer(passes::call::TARGET)
                        .eax(low(target, 1))
                        .args([target, 0]),
                )
                .await?;
                cx.read(|m| m.u32(TARGET_STATE))?;
                p.call(cx, 0x0003_29b0, layer(passes::call::CLIP_MODE).eax(0))
                    .await?;
                cx.write(|m| m.set_u8(s + 0x13, 1))?;
                p.call(cx, 0x0003_29b9, layer(passes::call::RECEIVER_BLEND))
                    .await?;
            } else {
                cx.write(|m| m.set_u32(LAYER, 0))?;
            }
            ebx = 0;
            let stencil = p
                .call(cx, 0x0003_29cc, layer(passes::call::STENCIL).ebx(0))
                .await?;
            let draw = cx.read(|m| m.u8(s + 0x13))?;
            if draw != 0 {
                p.call(
                    cx,
                    0x0003_29d9,
                    layer(passes::call::LAYER_DRAW)
                        .eax(low(stencil.eax, draw))
                        .ebx(0),
                )
                .await?;
                p.call(cx, 0x0003_29de, layer(passes::call::LAYER_END).ebx(0))
                    .await?;
            }
        }
    }
    let tail = GuestCall::new(0x0004_a780).esi(esi).edi(edi).ebx(ebx);
    Ok(p.call(cx, 0x0003_29e3, tail).await?.eax)
}
