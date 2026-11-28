//! Object lighting cache lookup and the render setup's lighting copies.

use super::staged::Staged;
use super::*;
use engine::light_constants::{Error, Fpu};
use engine::render::object_lighting;

const LIGHTING_CACHE: u32 = 0x0050_9434;
const DEFAULT_LIGHTING: u32 = 0x004d_e2f4;
const ACTIVE_BSP: u32 = 0x004e_0348;

/// The float state the setup runs under: it refuses any other
/// (`setup`'s `fp_supported` and x87 control checks).
const SETUP_FPU: Fpu = Fpu {
    mxcsr: 0x1f80,
    x87_control: 0x027f,
};

fn refused(e: Error) -> Refusal {
    match e {
        Error::Memory(_) => Refusal::Memory,
        _ => Refusal::Unsupported,
    }
}

/// The object's lighting record (`0x0003ddd0`,
/// [`object_lighting::record`]): its own, a new one or the oldest taken
/// over. A full cache with nothing old enough gives none, which the guest
/// body owns.
pub(super) fn cache(m: &mut Staged<'_>, handle: u32) -> Result<u32> {
    let header = ptr(m, LIGHTING_CACHE)?;
    let pool = Pool::read(m, header, 0x100)?;
    let record = object_lighting::record(m, handle, SETUP_FPU).map_err(refused)?;
    if record == NONE {
        return Err(Refusal::Lighting);
    }
    at(pool.data, (record & 0xffff) * pool.stride)
}

pub(super) fn refresh(
    m: &mut Staged<'_>,
    handle: u32,
    body: u32,
    definition: u32,
    entry: u32,
    frame: u32,
    pixels: f32,
) -> Result<()> {
    if m.u8(at(definition, 0x1c)?)? & 1 != 0 {
        object_lighting::colours(m, handle, true, SETUP_FPU).map_err(refused)?;
    }
    let source = if m.u8(at(body, 7)?)? & 1 != 0 {
        at(ptr(m, DEFAULT_LIGHTING)?, 0x1c)?
    } else {
        let bsp_light = m.u16(at(body, 0xd0)?)?;
        if bsp_light != u16::MAX {
            let block = ptr(m, ACTIVE_BSP)?;
            if u32::from(bsp_light) >= m.u32(at(block, 0x22c)?)? {
                return Err(Refusal::Lighting);
            }
            at(ptr(m, at(block, 0x230)?)?, u32::from(bsp_light) * 0x5c + 8)?
        } else {
            let root = lights::root_owner(m, handle)?;
            if root == handle {
                if !keeps_light_sample(m, entry, body, frame, pixels)? {
                    return Err(Refusal::Lighting);
                }
                return Ok(());
            }
            Pool::read(m, ptr(m, OBJECTS)?, 12)?.body(m, root)?;
            let root_entry = cache(m, root)?;
            if m.u32(at(entry, 4)?)? != handle {
                return Err(Refusal::Lighting);
            }
            let sample = m.bytes(at(root_entry, 0x1c)?, 0x14)?;
            m.write(at(entry, 0x30)?, &sample)?;
            m.write(at(entry, 0x1c)?, &sample)?;
            at(root_entry, 0x54)?
        }
    };
    let light = m.bytes(source, 0x54)?;
    m.write(at(entry, 0x54)?, &light)?;
    Ok(())
}

fn keeps_light_sample(
    m: &dyn GuestMemory,
    entry: u32,
    body: u32,
    frame: u32,
    pixels: f32,
) -> Result<bool> {
    let last_sample = m.u32(at(entry, 8)?)?;
    if last_sample == frame {
        return Ok(true);
    }
    if m.u8(at(entry, 2)?)? != 0 {
        return Ok(m.u8(at(entry, 3)?)? != 0 && m.u8(at(body, 0xc1)?)? & 1 != 0);
    }
    let mut period = 60;
    for (threshold, frames) in [
        (0x0045_e17c, 3),
        (0x0045_e314, 6),
        (0x0044_5420, 9),
        (0x0045_dc94, 15),
        (0x0045_dc00, 45),
    ] {
        if pixels > m.f32(threshold)? {
            period = frames;
            break;
        }
    }
    let age = frame.wrapping_sub(last_sample) as i32;
    let age = if age < 0 { 1 } else { age };
    Ok(last_sample != NONE && age <= period)
}
