//! Dynamic-light candidates for FrameScene (docs/graphics/e3-lighting.md,
//! slice L2). The main view's visibility gather has already culled the
//! light spheres. The guest's ranked and faded draw list is observed
//! separately at the light-pass entries.

pub mod gather;
mod volume;

use guest_layout::datum::DatumArray;
use guest_layout::handle::DatumHandle;
use engine::light_constants as lc;
use engine::light_passes as lp;
use guest_layout::memory::GuestMemory;
use engine::render::light::{self, LightRecord};
use engine::shadow_constants as sc;
use frame_scene::{InstanceKey, Light, LightCandidate, LightConstants, LightProjection, LightVariants};

#[cfg(test)]
const MAIN_LIGHT_LIST: u32 = 0x0054_7f98;
#[cfg(test)]
const HANDLE_COLUMN: u32 = 0xc;
#[cfg(test)]
const COUNT: u32 = 4;
const WEIGHT: u32 = 0x100;

// The view-depth plane and pixel scale used by 0x00031910.
const VIEW_DEPTH: [u32; 4] = [0x004b_9e20, 0x004b_9e2c, 0x004b_9e38, 0x004b_9e44];
const MIN_DEPTH: u32 = 0x0045_dc68;
const PIXEL_SCALE: u32 = 0x004b_9ed0;
const SIZE_FACTOR: u32 = 0x0045_dc0c;

/// The main view's list as the guest left it at `[0x00547f98]`: what
/// [`gather::main_view`] is compared with in the tests.
#[cfg(test)]
fn handles(m: &dyn GuestMemory) -> Option<Vec<u32>> {
    let list = m.u32(MAIN_LIGHT_LIST).ok()?;
    if list == 0 {
        return None;
    }
    let count = usize::from(m.u16(list.checked_add(COUNT)?).ok()?);
    if count > m.u32(list).ok()? as usize || count > light::MAXIMUM_COUNT as usize {
        return None;
    }
    if count == 0 {
        return Some(Vec::new());
    }
    let bytes = m.bytes(m.u32(list.checked_add(HANDLE_COLUMN)?).ok()?, 4 * count).ok()?;
    Some(bytes.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect())
}

/// Reads the pose and both choices of constants for the view's candidates.
/// `gathered` is the view's visible-light list from [`gather::main_view`];
/// the guest's own list is not read. Keep any valid light the guest drew as
/// well, even when the gather list could not be built (`None`). A missing tag or projection leaves the light visible
/// to diagnostics, with its unavailable fields absent.
/// `eligible` applies the existing view's pre-trim tests to the record's
/// address, the definition's address and the definition flags.
pub fn candidates(
    m: &dyn GuestMemory,
    fpu: lc::Fpu,
    gathered: Option<&[u32]>,
    guest: &[u32],
    eligible: impl Fn(u32, u32, u32) -> Option<bool>,
) -> Option<Vec<Light>> {
        let list = gathered.or_else(|| (!guest.is_empty()).then_some(&[][..]))?;
    if list.is_empty() && guest.is_empty() {
        return Some(Vec::new());
    }
    let array = DatumArray::at(m.u32(light::ARRAY_POINTER).ok()?);
    let header = array.header(m).ok()?;
    if header.element_size != light::RECORD_SIZE as i32 || header.maximum_count != light::MAXIMUM_COUNT {
        return None;
    }
    let mut out = Vec::with_capacity(list.len().max(guest.len()));
    let mut seen = [false; light::MAXIMUM_COUNT as usize];
    for &handle in list.iter().chain(guest) {
        let Some(seen) = seen.get_mut(handle as u16 as usize) else { continue };
        if *seen {
            continue;
        }
        let Some(address) = array.try_get(m, DatumHandle(handle)).ok().flatten() else { continue };
        let mut bytes = [0; light::RECORD_SIZE];
        if m.read(address, &mut bytes).is_err() {
            continue;
        }
        let record = LightRecord::parse(&bytes);
        if record.salt == 0 || record.flags & light::flags::LIVE == 0 {
            continue;
        }
        *seen = true;
        let mut light = Light { handle, definition: record.definition.0, ..Default::default() };
        if let Some(definition) = tag(m, record.definition.0, b"ligh") {
            if let Ok(flags) = m.u32(definition) {
                light.definition_flags = flags;
                let view_eligible = eligible(address, definition, flags);
                if view_eligible == Some(false) && !guest.contains(&handle) {
                    continue;
                }
                if record.check().is_ok() {
                    light.candidate = candidate(m, fpu, &light, address, definition, &record, view_eligible);
                }
            }
        }
        out.push(light);
    }
    Some(out)
}

fn tag(m: &dyn GuestMemory, datum: u32, group: &[u8; 4]) -> Option<u32> {
    if datum == u32::MAX {
        return None;
    }
    let table = m.u32(guest_layout::tags::TAG_ENTRIES_GLOBAL).ok()?;
    let slot = table.checked_add(u32::from(datum as u16) * 16)?;
    if m.u32(slot).ok()? != u32::from_be_bytes(*group) || m.u32(slot.checked_add(4)?).ok()? != datum {
        return None;
    }
    m.u32(slot.checked_add(8)?).ok().filter(|&data| data != 0)
}

fn priority(m: &dyn GuestMemory, ([x, y, z], radius): ([f32; 3], f32)) -> Option<f32> {
    let [a, b, c, d] = VIEW_DEPTH;
    let depth = (((m.f32(c).ok()? * z + m.f32(a).ok()? * x) + m.f32(b).ok()? * y) + m.f32(d).ok()?).abs();
    let depth = depth.max(m.f32(MIN_DEPTH).ok()?);
    let priority = (m.f32(PIXEL_SCALE).ok()? / depth * radius) * m.f32(SIZE_FACTOR).ok()?;
    priority.is_finite().then_some(priority)
}

fn candidate(
    m: &dyn GuestMemory,
    fpu: lc::Fpu,
    light: &Light,
    record_address: u32,
    definition: u32,
    record: &LightRecord,
    view_eligible: Option<bool>,
) -> Option<LightCandidate> {
    let weight = m.f32(record_address.checked_add(WEIGHT)?).ok()?;
    if !weight.is_finite() {
        return None;
    }
    let definition_kind = m.u16(definition.checked_add(4)?).ok()?;
    let gel_bitmap = m.u32(definition.checked_add(lp::DEFINITION_GEL)?).ok()?;
    let gel_shader = m.u32(definition.checked_add(lp::DEFINITION_GEL_SHADER)?).ok()?;
    let frame = record_address.checked_add(light::layout::POSITION as u32)?;
    let volume = volume::read(m, record_address, record, definition, definition_kind, gel_bitmap);
    let constants = volume.as_ref().and_then(|volume| {
        let memory = volume.at(m);
        let call = lc::Call {
            frame, context: volume::ADDRESS, light: light.handle, definition,
            shadow: 0, weight: weight.to_bits(),
        };
        variants(&memory, &call, fpu, light.definition_flags, gel_bitmap, gel_shader)
    });
    let bounds = (record.bounds_centre, record.bounds_radius);
    Some(LightCandidate {
        position: record.position,
        forward: record.forward,
        up: record.up,
        colour_a: record.colour_a,
        colour_b: record.colour_b,
        intensity: record.intensity,
        bounds,
        owner: (!record.parent_object.is_none()).then_some(InstanceKey(record.parent_object.0)),
        definition_kind,
        volume: volume.as_ref().map(volume::Volume::scene),
        view_eligible,
        priority: priority(m, bounds),
        weight,
        gel_bitmap: (gel_bitmap != u32::MAX).then_some(gel_bitmap),
        gel_shader: (gel_shader != u32::MAX).then_some(gel_shader),
        constants,
    })
}

fn variants(
    m: &dyn GuestMemory,
    call: &lc::Call,
    fpu: lc::Fpu,
    definition_flags: u32,
    gel_bitmap: u32,
    gel_shader: u32,
) -> Option<LightVariants> {
    // The shadow argument is only copied to a global by light_globals.
    // The same computed frame feeds the two choices in the pass flags.
    let g = lc::light_globals(m, call, fpu, &mut |_| None).ok()?;
    let unshadowed = lp::pass_flags(m, &g, 0, fpu.mxcsr).ok()?;
    let shadowed = lp::pass_flags(m, &g, 1, fpu.mxcsr).ok()?;
    let definition = call.definition;
    let common = LightConstants {
        frame: g.frame_constants(m).ok()?,
        flags: 0,
        colour_a: g.colour_a_input(),
        colour_b: g.colour_b_input(),
        mode: g.mode,
        definition_flags,
        definition_8a: m.u16(definition.checked_add(0x8a)?).ok()?,
        definition_80: m.u16(definition.checked_add(0x80)?).ok()?,
        falloff_0: g.falloff[0],
        gel: None,
        animated_gel: gel_shader != u32::MAX,
    };
    let make = |flags| LightConstants {
        flags,
        gel: lp::gel(flags, definition_flags, gel_shader, gel_bitmap),
        ..common
    };
    let projection = if shadowed.shadowed() {
        let call = sc::Call { context: call.context, frame: call.frame, definition };
        sc::shadow_globals_for_light(m, &call, fpu, &g, shadowed.flags).ok().map(|shadow| {
            LightProjection {
                rows: core::array::from_fn(|row| core::array::from_fn(|col| shadow.block[4 * row + col])),
                mode: shadow.mode,
                size: shadow.size,
            }
        })
    } else {
        None
    };
    Some(LightVariants { unshadowed: make(unshadowed.flags), shadowed: make(shadowed.flags), projection })
}

#[cfg(test)]
mod tests;
