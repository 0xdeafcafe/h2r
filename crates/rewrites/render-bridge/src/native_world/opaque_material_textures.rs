//! Stage-slot copies, lightmap selection and bitmap residency from the
//! standard geometry material binder (0x18a90 and 0x1b0a0). Resolving a
//! stage changes the desired resource. The emitter compares those headers
//! with the currently bound ones and issues the effective D3D texture binds.

use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::render_loop::{opaque_material::bitmap_frame, opaque_texture, world_lightmap, world_walk};
use engine::screen_grid::constant::{BOUND_HEIGHT, BOUND_WIDTH};
use guest_layout::tags::TAG_ENTRIES_GLOBAL;
use engine::screen_effect::{TARGET_OVERRIDE, TARGET_STRIDE, TARGETS};
use native_render::object_nodes::TEXACCUM_HEADER;
#[cfg(test)]
use native_render::object_nodes::TEXACCUM_TARGET;

const CTX: u32 = engine::render_loop::layer::SETUP_CONTEXT;
const SLOTS: u32 = CTX + 0x320;
const STAGES: u32 = CTX + 0x1320;
const DIRTY: u32 = CTX + 0x1420;
const DRAW_LOD: u32 = CTX + 0x2c;
const RASTERIZER: u32 = 0x0048_5a80;
const DESIRED_TEXTURE: u32 = 0x0051_f3d8;
const BITMAP_EPOCH: u32 = 0x004e_6488;
const FIXED_LOD: u32 = 0x004b_a019;
const RESET_MIP: u32 = 0x0045_dbd4;
const SMALL_FRAME: u32 = engine::function_eval::constant::SMALL;
const ENGINE_TEXTURE_SOURCE: u32 = 0x0001_9530;
const TARGET_BINDER: u32 = 0x0001_4f60;
const BITMAP_RESOURCE: u32 = 0x0001_2310;
const GUEST_RESOLVE: u32 = 0x0001_b0a0;
const STAGE_BYTES: u32 = 64;
const IMAGE_BYTES: u32 = 0x74;

type Applied = Result<(), EntryError>;

fn fp(error: engine::light_constants::Error) -> EntryError {
    match error {
        engine::light_constants::Error::Memory(fault) => fault.into(),
        other => EntryError::IsolatedCall(format!("world texture arithmetic: {other:?}")),
    }
}

fn tag(m: &dyn GuestMemory, datum: u32) -> Result<u32, MemFault> {
    m.u32(m.u32(TAG_ENTRIES_GLOBAL)?.wrapping_add((datum & 0xffff) * 16 + 8))
}

fn fallback_bitmap(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    m.u32(m.u32(RASTERIZER)?.wrapping_add(0x64))
}

fn image(m: &dyn GuestMemory, bitmap: u32, frame: i32) -> Result<u32, MemFault> {
    Ok(m.u32(bitmap.wrapping_add(0x48))?.wrapping_add((frame as u32).wrapping_mul(IMAGE_BYTES)))
}

fn dimensions(m: &dyn GuestMemory, image: u32) -> Result<(i32, i32), MemFault> {
    Ok((i32::from(m.u16(image.wrapping_add(4))? as i16),
        i32::from(m.u16(image.wrapping_add(6))? as i16)))
}

fn stage_dimensions(m: &mut dyn GuestMemory, stage: u32, width: i32, height: i32) -> Result<(), MemFault> {
    m.set_u32(BOUND_WIDTH + stage * 4, width as u32)?;
    m.set_u32(BOUND_HEIGHT + stage * 4, height as u32)
}

/// A resident image takes the fast arm of 0x12310. The streaming arm can
/// allocate, change the heap or replace a resource, so call it in order.
async fn bitmap(cx: &Cx, mut datum: u32, frame: i32, stage: u32, lod: u32) -> Applied {
    let mut index = i32::from(frame as i16);
    let selected = cx.read(|m| -> Result<Option<u32>, MemFault> {
        if datum == u32::MAX {
            datum = fallback_bitmap(m)?;
            if datum == u32::MAX { return Ok(None); }
            index = 0;
        }
        let mut definition = tag(m, datum)?;
        if index < 0 || index >= m.i32(definition.wrapping_add(0x44))? {
            definition = tag(m, fallback_bitmap(m)?)?;
            index = 0;
        }
        if definition == 0 || m.i32(definition.wrapping_add(0x44))? <= 0 { return Ok(None); }
        let image = image(m, definition, index)?;
        if image == 0 { return Ok(None); }
        Ok(Some(image))
    })?;
    if let Some(image) = selected { bitmap_image(cx, image, stage, lod).await?; }
    Ok(())
}

async fn bitmap_image(cx: &Cx, image: u32, stage: u32, lod: u32) -> Applied {
    let resident = cx.read(|m| -> Result<_, MemFault> {
        let header = m.u32(image.wrapping_add(0x50))?;
        // The non-streamed branch of the residency check (0x12cd2c)
        // returns the image's header irrespective of its epoch.
        Ok(if header != 0 && (m.i32(image.wrapping_add(0x70))? > m.i32(BITMAP_EPOCH)?
            || m.u8(image.wrapping_add(0x0f))? & 2 == 0) { header } else { 0 })
    })?;
    let header = if resident == 0 {
        cx.call(GuestCall::new(BITMAP_RESOURCE).eax(image).xmm2(u128::from(lod))).await?.eax
    } else { resident };
    cx.write(|m| {
        m.set_u32(DESIRED_TEXTURE + stage * 4, header)?;
        let (width, height) = dimensions(m, image)?;
        stage_dimensions(m, stage, width, height)
    })?;
    Ok(())
}

/// A missing lightmap selects the rasterizer fallback datum without setting
/// the stage dirty or overwriting its old page, size or LOD fields.
fn lightmap(m: &mut dyn GuestMemory, at: u32, fcw: u16, mxcsr: u32) -> Result<bool, EntryError> {
    let active = m.u32(world_walk::LIGHTMAP)?;
    let selection = (|| -> Result<_, MemFault> {
        if active == 0 || m.i32(active + 0x80)? <= 0 { return Ok(None); }
        let bsp = m.u32(super::ACTIVE_BSP)?;
        if bsp == 0 { return Ok(None); }
        let group = m.u32(active + 0x84)?;
        let datum = m.u32(group + 0x1c)?;
        if datum == u32::MAX || m.u32(group + 4)? != m.u32(bsp + 8)? { return Ok(None); }
        let page = m.i32(world_lightmap::PAGE)?;
        Ok((page != -1).then_some((datum, page)))
    })()?;
    let Some((datum, page)) = selection else {
        let fallback = fallback_bitmap(m)?;
        m.set_u32(at, fallback)?;
        return Ok(false);
    };
    let frame = bitmap_frame(page, mxcsr).map_err(fp)?;
    if m.u32(at)? != datum || !opaque_texture::equal(m.u32(at + 0x18)?, frame, mxcsr).map_err(fp)? {
        let selected = image(m, tag(m, datum)?, page)?;
        let reset = m.u32(RESET_MIP)?;
        m.set_u32(at + 0x10, reset)?;
        m.set_u32(at, datum)?;
        m.set_u32(at + 0x18, frame)?;
        let (w, h) = dimensions(m, selected)?;
        let log = opaque_texture::lightmap_lod(m, w.max(h) as i16, fcw).map_err(fp)?;
        m.set_u32(at + 0x14, log)?;
    }
    Ok(true)
}

/// The 64-byte copy precedes the optional update to the *source* slot.
/// A second stage sourcing the same slot sees the refreshed dimensions.
fn copy_slot(m: &mut dyn GuestMemory, slot: i8, at: u32, resize: bool, mxcsr: u32) -> Applied {
    let src = SLOTS.wrapping_add_signed(i32::from(slot) * STAGE_BYTES as i32);
    let mut fields = [0; STAGE_BYTES as usize];
    m.read(src, &mut fields)?;
    m.write(at, &fields)?;
    if resize {
        let datum = m.u32(src)?;
        if datum != u32::MAX {
            let def = tag(m, datum)?;
            let count = bitmap_frame(m.i32(def + 0x44)?, mxcsr).map_err(fp)?;
            let frame = m.u32(src + 0x18)?;
            if opaque_texture::above(count, frame, mxcsr).map_err(fp)? {
                let i = opaque_texture::frame_floor(frame, mxcsr).map_err(fp)?;
                let img = image(m, def, i)?;
                if m.u8(img.wrapping_add(0x0e))? & 0x10 != 0 {
                    let (w, h) = dimensions(m, img)?;
                    m.set_u32(src + 8, w as u32)?;
                    m.set_u32(src + 12, h as u32)?;
                }
            }
        }
    }
    Ok(())
}

fn target_entry(id: u32) -> u32 {
    TARGETS.wrapping_add(id.wrapping_mul(TARGET_STRIDE))
}

fn target_live(m: &dyn GuestMemory, id: u32) -> Result<bool, MemFault> {
    let at = target_entry(id);
    Ok(m.u32(at.wrapping_add(0x88))? != 0 && m.u8(at.wrapping_add(0x95))? == 0)
}

/// 0x14f60 reads a target-kind selector for its texture lookup. A surface
/// and its sample header can refer to different slots in the target table.
fn target_header(m: &dyn GuestMemory, id: u32) -> Result<Option<u32>, MemFault> {
    if id == 1 { return Ok(Some(if target_live(m, 1)? { TEXACCUM_HEADER } else { 0 })); }
    const TARGET_KIND: u32 = 0x0001_5154;
    const TARGET_HANDLERS: u32 = 0x0001_5114;
    let kind = u32::from(m.u8(TARGET_KIND.wrapping_add(id))?);
    let arm = m.u32(TARGET_HANDLERS + kind * 4)?;
    // The MRT aliases, rotating framebuffer slots, and rasterizer globals
    // below are the cases of the live binder, located by their entry PCs.
    let target = match arm {
        0x0001_5044 => 1,
        0x0001_505c => 0x12,
        0x0001_5074 => 0x13,
        0x0001_50ba => 0x0d,
        0x0001_50d2 => 0x14,
        0x0001_50fc => id,
        0x0001_4fa7 | 0x0001_508c | 0x0001_4fd4 => {
            const FRAME: u32 = 0x0048_5aa0;
            const HEADERS: u32 = 0x0050_9354;
            let frame = u64::from(m.u32(FRAME)?) | (u64::from(m.u32(FRAME + 4)?) << 32);
            let delta = i64::from(arm != 0x0001_4fd4);
            let index = (frame as i64).wrapping_sub(delta) % 2;
            return Ok(Some(m.u32(HEADERS.wrapping_add_signed(index as i32 * 4))?));
        }
        _ => {
            let ptr = match arm {
                0x0001_4ffc => 0x0050_9374,
                0x0001_500e => 0x0050_937c,
                0x0001_5020 => 0x0050_9368,
                0x0001_5032 => 0x0050_936c,
                0x0001_50ea => 0x0050_938c,
                _ => return Ok(None),
            };
            return Ok(Some(m.u32(ptr)?));
        }
    };
    Ok(Some(if target_live(m, target)? { target_entry(target) + 4 } else { 0 }))
}

async fn target(cx: &Cx, id: u32, stage: u32, at: u32, copy_size: bool) -> Applied {
    let header = cx.read(|m| target_header(m, id))?;
    let (w, h) = cx.read(|m| engine::screen_effect::target_size(m, id, &[])).map_err(fp)?;
    if let Some(header) = header {
        cx.write(|m| {
            stage_dimensions(m, stage, w, h)?;
            m.set_u32(DESIRED_TEXTURE + stage * 4, header)
        })?;
    } else {
        cx.call(GuestCall::new(TARGET_BINDER).ebx(stage).edi(id)).await?;
    }
    if copy_size {
        cx.write(|m| {
            m.set_u32(at + 8, w as u32)?;
            m.set_u32(at + 12, h as u32)
        })?;
    }
    Ok(())
}

fn checked_image(m: &dyn GuestMemory, datum: u32, index: i32) -> Result<Option<u32>, MemFault> {
    if datum == u32::MAX { return Ok(None); }
    let def = tag(m, datum)?;
    let index = i32::from(index as i16);
    if def == 0 || index < 0 || index >= m.i32(def + 0x44)? { return Ok(None); }
    Ok(Some(image(m, def, index)?).filter(|&p| p != 0))
}

async fn engine_source(cx: &Cx, mut source: u8, stage: u32, at: u32) -> Applied {
    const GEL_DATA: u32 = 0x0048_58c4;
    const GEL_ACTIVE: u32 = 0x0048_58c8;
    const VIEW_BITMAP: u32 = 0x004e_6978;
    const TARGET_SLOTS: u32 = 0x0048_5a18;
    const UNUSED_GEL_WARNING: u32 = 0x0055_e6bd;
    cx.write(|m| { m.set_u32(at + 8, 1)?; m.set_u32(at + 12, 1) })?;
    // The gel path may reselect the dynamic view source (18). The guest
    // restarts its dispatch and reinitializes both dimensions to one.
    if source == 10 || source == 12 {
        let gel = cx.read(|m| -> Result<_, MemFault> {
            if m.u16(GEL_ACTIVE)? == 0 { Ok(0) } else { m.u32(GEL_DATA) }
        })?;
        if gel == 0 && source == 10 {
            cx.write(|m| -> Result<(), MemFault> {
                if m.u8(UNUSED_GEL_WARNING)? == 0 { m.set_u8(UNUSED_GEL_WARNING, 1)?; }
                Ok(())
            })?;
            let img = cx.read(|m| checked_image(m, fallback_bitmap(m)?, 1))?;
            if let Some(img) = img { bitmap_image(cx, img, stage, 0).await?; }
            return Ok(());
        }
        if gel == 0 { source = 9; }
        else if source == 10 && cx.read(|m| m.u32(gel))? & (1 << 19) != 0 { source = 18; }
        else if source == 10 && cx.read(|m| m.u32(gel + 0xe0))? != u32::MAX
            && cx.read(|m| m.u8(0x0048_5a77))? == 0 {
            return target(cx, 13, stage, at, false).await;
        } else {
            // The typed bitmap helper requests image zero only for type 0,
            // and otherwise takes image one of the rasterizer fallback.
            let img = cx.read(|m| -> Result<_, MemFault> {
                if let Some(img) = checked_image(m, m.u32(gel + 0x7c)?, 0)?
                    && m.u16(img + 10)? == 0 { return Ok(Some(img)); }
                checked_image(m, fallback_bitmap(m)?, 1)
            })?;
            if let Some(img) = img { bitmap_image(cx, img, stage, 0).await?; }
            return Ok(());
        }
    }
    match source {
        1 | 9 | 14 | 15 => {
            let offset = match source { 1 => 12, 9 => 4, _ => 20 };
            let datum = cx.read(|m| m.u32(m.u32(RASTERIZER)?.wrapping_add(offset)))?;
            bitmap(cx, datum, i32::from(source == 15), stage, 0).await
        }
        2 | 3 | 5 | 6 | 8 | 13 | 17 | 19..=25 => {
            let mut copy_size = source != 2;
            let id = match source {
                2 | 3 => 1,
                5 => cx.read(|m| m.u32(TARGET_OVERRIDE))?,
                6 => 3,
                8 => 10,
                13 => 9,
                17 => 0x13,
                19 => 0x15,
                20 => 0x16,
                21 => 0x0d,
                22 => 0x11,
                23 => 0x17,
                _ => {
                    let t = cx.read(|m| m.u32(TARGET_SLOTS + 4 * u32::from(source)))?;
                    if matches!(t, 0x19..=0x1b) { t } else { copy_size = false; 1 }
                }
            };
            target(cx, id, stage, at, copy_size).await
        }
        4 => { cx.write(|m| m.set_u32(DESIRED_TEXTURE + 4 * stage, 0))?; Ok(()) }
        16 => {
            let pair = |m: &dyn GuestMemory| -> Result<_, MemFault> { Ok((m.u32(VIEW_BITMAP)?, m.i32(VIEW_BITMAP + 4)?)) };
            let (datum, i) = cx.read(pair)?;
            bitmap(cx, datum, i, stage, 0).await?;
            cx.write(|m| -> Result<(), MemFault> {
                let (datum, i) = pair(m)?;
                if datum == u32::MAX || i < 0 { return Ok(()); }
                let def = tag(m, datum)?;
                if i < m.i32(def + 0x44)? {
                    let (w, h) = dimensions(m, image(m, def, i)?)?;
                    m.set_u32(at + 8, w as u32)?;
                    m.set_u32(at + 12, h as u32)?;
                }
                Ok(())
            })?;
            Ok(())
        }
        18 => {
            if cx.read(|m| m.u8(0x0048_5607))? == 0 {
                for id in [0x14, 0x12] {
                    if cx.read(|m| target_live(m, id))? { return target(cx, id, stage, at, true).await; }
                }
            }
            let datum = cx.read(fallback_bitmap)?;
            bitmap(cx, datum, 0, stage, 0).await
        }
        26 => {
            let (datum, index) = cx.read(|m| -> Result<_, MemFault> {
                let definition = |d: u32| m.u32(tag(m, d)? + 0x24);
                let selector = |p: u32| Ok::<_, MemFault>(i32::from(m.u16(m.u32(p + 0x60)? + 0x10)? as i16));
                let mut imp = definition(m.u32(CTX + 8)?)?;
                let mut i = selector(imp)?;
                if i == -1 {
                    imp = definition(m.u32(m.u32(RASTERIZER)? + 0xd0)?)?;
                    i = selector(imp)?;
                }
                let bitmap = m.u32(imp + 8)?.wrapping_add_signed(i.wrapping_mul(12));
                Ok((m.u32(bitmap)?, i32::from(m.u16(bitmap + 4)? as i16)))
            })?;
            bitmap(cx, datum, index, stage, 0).await
        }
        27 => {
            if cx.read(|m| m.u8(0x0048_5a76))? != 0 { return target(cx, 16, stage, at, true).await; }
            let img = cx.read(|m| checked_image(m, fallback_bitmap(m)?, 0))?;
            if let Some(img) = img { bitmap_image(cx, img, stage, 0).await?; }
            cx.write(|m| { m.set_u32(at + 8, 4)?; m.set_u32(at + 12, 4) })?;
            Ok(())
        }
        _ => {
            cx.call(GuestCall::new(ENGINE_TEXTURE_SOURCE).eax(i32::from(source as i8) as u32)
                .args([CTX, stage, at + 8, at + 12])).await?;
            Ok(())
        }
    }
}

pub(super) async fn bind(cx: &Cx, range: u32, fcw: u16, mxcsr: u32) -> Applied {
    let (first, count) = cx.read(|m| -> Result<_, MemFault> {
        let packed = u32::from(m.u16(range)?);
        let entries = m.u32(m.u32(CTX + 0x20)?.wrapping_add(12))?;
        Ok((entries.wrapping_add((packed & 511) * 4), packed >> 9))
    })?;
    for n in 0..count {
        let binding = cx.read(|m| m.u32(first.wrapping_add(n * 4)))?.to_le_bytes();
        let [slot, source, stage, flags] = binding;
        let stage = u32::from(stage);
        let at = STAGES + stage * STAGE_BYTES;
        let dirty = if slot != u8::MAX {
            cx.write(|m| copy_slot(m, slot as i8, at, flags & 2 != 0, mxcsr))?;
            true
        } else if source == 11 {
            cx.write(|m| lightmap(m, at, fcw, mxcsr))?
        } else {
            cx.write(|m| m.set_u32(at, u32::MAX))?;
            if source != 0 { engine_source(cx, source, stage, at).await?; }
            false
        };
        if dirty { cx.write(|m| m.set_u32(DIRTY, m.u32(DIRTY)? | 1u32.wrapping_shl(stage)))?; }
    }
    Ok(())
}

/// An invalid bitmap count or a non-finite animated frame can run CRT error
/// handlers. Choose the original resolver before publishing any stage in that
/// case. Ordinary resident and streaming bitmaps take the native resolver.
fn crt_error_path(m: &dyn GuestMemory, mut dirty: u32) -> Result<bool, MemFault> {
    let epsilon = m.f32(SMALL_FRAME)?;
    while dirty != 0 {
        let at = STAGES + dirty.trailing_zeros() * STAGE_BYTES;
        dirty &= dirty - 1;
        let datum = m.u32(at)?;
        if datum == u32::MAX { continue; }
        let frame = m.f32(at + 0x18)?;
        if !frame.is_finite() || (frame.abs() >= epsilon && m.i32(tag(m, datum)? + 0x44)? <= 0) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) async fn resolve(cx: &Cx, fcw: u16, mxcsr: u32) -> Applied {
    let mut dirty = cx.read(|m| m.u32(DIRTY))?;
    if dirty == 0 { return Ok(()); }
    if cx.read(|m| crt_error_path(m, dirty))? {
        cx.call(GuestCall::new(GUEST_RESOLVE).args([CTX])).await?;
        return Ok(());
    }
    while dirty != 0 {
        let stage = dirty.trailing_zeros();
        dirty &= dirty - 1;
        let at = STAGES + stage * STAGE_BYTES;
        let request = cx.read(|m| -> Result<_, EntryError> {
            let datum = m.u32(at)?;
            if datum == u32::MAX { return Ok(None); }
            let frame = m.f32(at + 0x18)?;
            let index = if m.f32(SMALL_FRAME)? > frame.abs() { 0 } else {
                let count = m.i32(tag(m, datum)? + 0x44)?;
                opaque_texture::wrapped_image(frame, count, fcw, mxcsr).map_err(fp)?
            };
            let bias = if m.u8(at + 4)? & 1 != 0 || m.u8(FIXED_LOD)? != 0 { None }
                else { Some((m.u32(at + 0x14)?, m.u32(DRAW_LOD)?)) };
            let (lod, threshold) = opaque_texture::resident_lod(m, bias, mxcsr).map_err(fp)?;
            let rebind = opaque_texture::above(m.u32(at + 0x10)?, threshold, mxcsr).map_err(fp)?;
            Ok(rebind.then_some((datum, index, lod, threshold)))
        })?;
        if let Some((datum, index, lod, threshold)) = request {
            bitmap(cx, datum, index, stage, lod).await?;
            cx.write(|m| m.set_u32(at + 0x10, threshold))?;
        }
    }
    // 0x1b0a0 consumes its local mask; the context mask stays available to
    // the following sampler and pixel operations in the material dispatcher.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::entry::{EntryFuture, Regs, Ret, testing::run};
    use guest_layout::testing::FakeGuest;

    const RANGE: u32 = 0x1000;
    const PASS: u32 = 0x1020;
    const BINDINGS: u32 = 0x1100;
    const TAGS: u32 = 0x2000;
    const DEFINITION: u32 = 0x3000;
    const IMAGE: u32 = 0x3100;
    const DATUM: u32 = 0xd000_0006;

    fn memory() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.map(RANGE, 0x3000).map(CTX - 0x100, 0x1900).map(TAG_ENTRIES_GLOBAL, 4)
            .map(DESIRED_TEXTURE, 16).map(BITMAP_EPOCH, 4).map(FIXED_LOD, 1)
            .map(SMALL_FRAME, 4).map(TEXACCUM_TARGET, 0x98);
        m.set_u32(CTX + 0x20, PASS).unwrap();
        m.set_u32(PASS + 12, BINDINGS).unwrap();
        m.set_u32(TAG_ENTRIES_GLOBAL, TAGS).unwrap();
        m.set_u32(TAGS + 6 * 16 + 8, DEFINITION).unwrap();
        m.set_u32(DEFINITION + 0x44, 1).unwrap();
        m.set_u32(DEFINITION + 0x48, IMAGE).unwrap();
        m.set_f32(SMALL_FRAME, 0.001).unwrap();
        m.set_f32(engine::light_constants::constant::TWO, 2.0).unwrap();
        m
    }

    #[test]
    fn duplicate_source_copies_before_size_update_and_only_rebinds_resident_bitmaps() {
        let mut m = memory();
        m.set_u16(RANGE, 2 << 9).unwrap();
        m.write(BINDINGS, &[0, 0, 0, 2, 0, 0, 2, 0]).unwrap();
        m.set_u32(SLOTS, DATUM).unwrap();
        m.set_u8(SLOTS + 4, 1).unwrap();
        m.set_u32(SLOTS + 8, 7).unwrap();
        m.set_u32(SLOTS + 12, 9).unwrap();
        m.set_f32(SLOTS + 0x10, 4.0).unwrap();
        m.set_u16(IMAGE + 4, 20).unwrap();
        m.set_u16(IMAGE + 6, 12).unwrap();
        m.set_u8(IMAGE + 14, 0x10).unwrap();
        m.set_u32(IMAGE + 0x50, 0xf000).unwrap();
        let binding = |cx: Cx| -> EntryFuture { Box::pin(async move {
            bind(&cx, RANGE, 0x27f, 0x1f80).await?;
            resolve(&cx, 0x27f, 0x1f80).await?;
            Ok(Ret::new(0, 0))
        }) };
        let result = run(&binding, m, Regs::default(), |c, _| panic!("unexpected call {c:?}"));
        assert_eq!(result.result, Ok(Ret::new(0, 0)));
        let m = result.guest;
        assert_eq!((m.u32(STAGES + 8), m.u32(STAGES + 12)), (Ok(7), Ok(9)));
        assert_eq!((m.u32(STAGES + 2 * 64 + 8), m.u32(STAGES + 2 * 64 + 12)), (Ok(20), Ok(12)));
        assert_eq!(m.u32(DIRTY), Ok(5));
        for stage in [0, 2] {
            assert_eq!(m.u32(DESIRED_TEXTURE + stage * 4), Ok(0xf000));
            assert_eq!(m.u32(BOUND_WIDTH + stage * 4), Ok(20));
            assert_eq!(m.u32(BOUND_HEIGHT + stage * 4), Ok(12));
            assert_eq!(m.f32(STAGES + stage * 64 + 0x10), Ok(0.0));
        }
    }

    #[test]
    fn texaccum_uses_the_view_header_and_only_source_three_propagates_its_size() {
        let mut m = memory();
        m.set_u16(RANGE, 2 << 9).unwrap();
        m.write(BINDINGS, &[255, 2, 1, 0, 255, 3, 0, 0]).unwrap();
        m.set_u32(TEXACCUM_TARGET + 0x88, 8).unwrap();
        m.set_u32(TEXACCUM_TARGET + 0x80, 100).unwrap();
        m.set_u32(TEXACCUM_TARGET + 0x84, 70).unwrap();
        let binding = |cx: Cx| -> EntryFuture { Box::pin(async move {
            bind(&cx, RANGE, 0x27f, 0x1f80).await?;
            Ok(Ret::new(0, 0))
        }) };
        let r = run(&binding, m, Regs::default(), |_, _| panic!("unexpected guest lookup"));
        assert_eq!(r.result, Ok(Ret::new(0, 0)));
        assert_eq!(r.guest.u32(DIRTY), Ok(0));
        for n in 0..2 {
            assert_eq!(r.guest.u32(DESIRED_TEXTURE + n * 4), Ok(TEXACCUM_HEADER));
            assert_eq!(r.guest.u32(BOUND_WIDTH + n * 4), Ok(100));
            assert_eq!(r.guest.u32(BOUND_HEIGHT + n * 4), Ok(70));
        }
        assert_eq!(r.guest.u32(STAGES + 8), Ok(100));
        assert_eq!(r.guest.u32(STAGES + 12), Ok(70));
        assert_eq!(r.guest.u32(STAGES + 64 + 8), Ok(1));
        assert_eq!(r.guest.u32(STAGES + 64 + 12), Ok(1));
    }
}
