//! Material and shader context of the standard sorted BSP draw. The words
//! written here are shared with object and special geometry handlers.
//! In particular, an implementation transition consumes the old restore
//! range before replacing the pass pointer. The two shader vector banks,
//! pixel program and texture slots survive draws and are only partly written.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use guest_layout::entry::{Cx, EntryError, GuestCall};
use engine::render_loop::opaque_material::ProjectedDistance;
use native_render::world_ranges::SortedPart;
use guest_layout::memory::{GuestMemory, MemFault};
use engine::render_loop::states;
use guest_layout::tags::TAG_ENTRIES_GLOBAL;
use native_render::skip::{RENDER_RECORDS, RENDER_RECORD_SIZE};

use super::{opaque_material_animation as animation, opaque_material_pixel as pixel,
    opaque_material_registers as registers, opaque_material_states as dynamic,
    opaque_material_textures as stage_textures};

const SHADER_CONTEXT: u32 = 0x0048_5b48;
const FIRST_DRAW: u32 = 0x0046_70bc;
const LIGHTING_INPUT: u32 = 0x0048_5a58;
const SHADER_VALUES: u32 = 0x0048_5a64;
const STAGE_STATES: u32 = 0x0040_4dd0;
const TEXTURES: u32 = 0x0051_f3d8;
const DEFAULT_SLOT_FRAME: u32 = 0x0045_dbd4;
const SET_TEXTURE: u32 = 0x003f_8320;
const SETTER_TABLE: u32 = 0x0040_38d8;

#[derive(Clone, Copy, Debug)]
struct MaterialCall {
    part: u32,
    layer: u32,
    fade: u8,
    record: u16,
    shader: u32,
    lod: u32,
    pass: u32,
    implementation: u32,
}

#[derive(Clone, Copy)]
struct Selection {
    shader: u32,
    instance: u32,
    template: u32,
    lod: u32,
    pass: u32,
    bindings: u32,
    definition: u32,
    implementation: u32,
    animation: Option<animation::Leaf>,
}

type Fault<T> = Result<T, MemFault>;
type Applied = Result<(), EntryError>;

impl MaterialCall {
    fn select(self, m: &dyn GuestMemory, table: u32) -> Fault<Selection> {
        let tag = |datum: u32| m.u32(table.wrapping_add((datum & 0xffff) * 16 + 8));
        let shader = tag(self.shader)?;
        let instance = m.u32(shader.wrapping_add(0x24))?;
        let template = m.u32(tag(m.u32(instance)?)?.wrapping_add(0x5c))?;
        let lod = m.u32(template.wrapping_add(4))?.wrapping_add(self.lod * 10);
        let layer_index = u32::from(m.u16(lod)? & 511) + self.layer;
        let layer = m.u32(template.wrapping_add(12))?.wrapping_add(layer_index * 2);
        let pass_index = u32::from(m.u16(layer)? & 511) + self.pass;
        let pass = m.u32(template.wrapping_add(20))?.wrapping_add(pass_index * 10);
        let row = u32::from(m.u16(pass.wrapping_add(8))? & 511) + self.implementation;
        let bindings = m.u32(template.wrapping_add(28))?.wrapping_add(row * 6);
        let definition = m.u32(tag(m.u32(pass.wrapping_add(4))?)?.wrapping_add(0x20))?;
        let implementation = m.u32(definition.wrapping_add(4))?.wrapping_add(self.implementation * 0x132);
        let animation = animation::prepare(m, instance, self.lod, self.layer, self.pass, self.implementation)?;
        Ok(Selection { shader, instance, template, lod, pass, bindings, definition, implementation, animation })
    }
}

fn or_word(m: &mut dyn GuestMemory, at: u32, bits: u32) -> Fault<()> {
    let old = m.u32(at)?;
    m.set_u32(at, old | bits)
}

/// 0x1bd50. A null lighting input dirties its masks even when it was
/// already null. The next object draw consumes those same dirty bits.
fn lighting(m: &mut dyn GuestMemory, next: u32) -> Fault<()> {
    if m.u32(LIGHTING_INPUT)? == next && next != 0 { return Ok(()); }
    m.set_u32(LIGHTING_INPUT, next)?;
    for (n, (a, b)) in [(0x0048_508c, 0x0048_507c), (0x0048_5090, 0x0048_5080),
        (0x0048_50bc, 0x0048_50ac), (0x0048_50c0, 0x0048_50b0),
        (0x0048_5068, 0x0048_5060)].into_iter().enumerate() {
        let bits = m.u32(a)? | m.u32(b)?;
        or_word(m, SHADER_CONTEXT + 0x30 + n as u32 * 4, bits)?;
    }
    Ok(())
}

/// A section's material recipe. Only immutable tag addresses, the selected
/// property spans, and the projected distance go through preflight. The
/// previous shader, active implementation and dirty flags are read during
/// handoff after the preceding draw, including when the preceding draw is
/// replacement geometry.
pub(super) struct Prepared {
    call: MaterialCall,
    selection: Selection,
    distance: [u32; 2],
    spans: [bool; 5],
}

impl Prepared {
    pub(super) fn shader_address(&self) -> u32 { self.selection.shader }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
struct PassKey {
    shader: u32,
    program: u32,
    layer: u32,
    lod: u8,
    pass: u8,
    implementation: u8,
}

/// Held only while planning a consecutive sorted span, before any guest
/// writes. Entries in a later span (including after an interleaved guest
/// shader or a live tag edit) are resolved from that span's memory.
#[derive(Default)]
pub(super) struct SectionMaterials {
    passes: HashMap<PassKey, (Selection, [bool; 5])>,
    distances: HashMap<ProjectedDistance, [u32; 2]>,
}

impl SectionMaterials {
    pub(super) fn prepare(&mut self, m: &dyn GuestMemory, part: &SortedPart,
        fcw: u16, mxcsr: u32) -> Result<Prepared, &'static str> {
        let call = MaterialCall {
            part: part.part_addr, layer: part.layer, fade: part.projected_size, record: part.record,
            shader: part.shader, lod: u32::from(part.lod), pass: u32::from(part.pass),
            implementation: u32::from(part.implementation),
        };
        let key = PassKey { shader: part.shader, program: part.program, layer: part.layer,
            lod: part.lod, pass: part.pass, implementation: part.implementation };
        let (selection, spans) = match self.passes.entry(key) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let table = m.u32(TAG_ENTRIES_GLOBAL).map_err(|_| "unreadable shader tag table")?;
                let tag = table.checked_add((call.shader & 0xffff) * 16).ok_or("invalid shader tag")?;
                if call.shader as u16 > i16::MAX as u16 || m.u32(tag).ok() != Some(u32::from_be_bytes(*b"shad")) {
                    return Err("material requires another tag accessor");
                }
                let selection = call.select(m, table).map_err(|_| "unreadable material pass")?;
                if selection.implementation != part.program { return Err("sorted implementation changed"); }
                engine::render_loop::world_registers::validate(m, selection.definition, part.program)?;
                let mut spans = [false; 5];
                for (span, offset) in spans.iter_mut().zip([0, 0x124, 0x122, 0x11e, 0x120]) {
                    *span = m.u16(part.program.checked_add(offset).ok_or("invalid implementation")?)
                        .map_err(|_| "unreadable material binding range")? >> 9 != 0;
                }
                *entry.insert((selection, spans))
            }
        };
        let operands = ProjectedDistance::read(m, call.part, call.fade, fcw, mxcsr)
            .map_err(|_| "unreadable projected distance")?;
        let distance = match self.distances.entry(operands) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => *entry.insert(operands.words()
                .map_err(|_| "unrepresentable projected distance")?),
        };
        Ok(Prepared { call, selection, distance, spans })
    }
}

#[cfg(test)]
pub(super) fn prepare(m: &dyn GuestMemory, part: &SortedPart, fcw: u16, mxcsr: u32)
    -> Result<Prepared, &'static str> {
    SectionMaterials::default().prepare(m, part, fcw, mxcsr)
}

async fn invoke(cx: &Cx, call: GuestCall) -> Applied {
    cx.call(call).await?;
    Ok(())
}

async fn simple(cx: &Cx, header: u32, value: u32, shadow: u32) -> Applied {
    invoke(cx, GuestCall::new(states::SIMPLE).ecx(header).edx(value)).await?;
    cx.write(|m| m.set_u32(shadow, value))?;
    Ok(())
}

/// Fixed pass and restore states, including the setter-table case at
/// state 0x88. The per-draw lists have a different branch for that state.
async fn fixed_states(cx: &Cx, definition: u32, range: u32) -> Applied {
    let span = cx.read(|m| m.u16(range))?;
    if span >> 9 == 0 { return Ok(()); }
    let mut at = cx.read(|m| m.u32(definition + 0x14))? + u32::from(span & 511) * 5;
    for _ in 0..span >> 9 {
        let (state, value) = cx.read(|m| -> Fault<_> { Ok((u32::from(m.u8(at)?), m.u32(at + 1)?)) })?;
        if state < 0x5c {
            let header = cx.read(|m| m.u32(states::HEADERS + state * 4))?;
            simple(cx, header, value, states::SHADOW + state * 4).await?;
        } else if state < 0x88 {
            cx.write(|m| {
                let mask = m.u32(states::MASKS + state * 4)?;
                or_word(m, states::DIRTY, mask)?;
                m.set_u32(states::SHADOW + state * 4, value)
            })?;
        } else {
            let setter = cx.read(|m| m.u32(SETTER_TABLE + state * 4))?;
            invoke(cx, GuestCall::new(setter).args([value])).await?;
        }
        at += 5;
    }
    Ok(())
}

pub(super) async fn clear(cx: &Cx) -> Applied {
    let ctx = SHADER_CONTEXT;
    let old = cx.read(|m| m.u32(ctx + 0x24))?;
    if old != 0 {
        let definition = cx.read(|m| m.u32(ctx + 0x20))?;
        fixed_states(cx, definition, old + 0x11c).await?;
    }
    cx.write(|m| -> Fault<_> {
        m.write(ctx + 0x0c, &[0; 28])?;
        m.set_u32(ctx, u32::MAX)?;
        m.set_u32(ctx + 0x40, u32::MAX)?;
        let empty_frame = m.u32(DEFAULT_SLOT_FRAME)?;
        for stage in 0..4 {
            let slot = ctx + 0x1320 + stage * 64;
            m.set_u32(slot, u32::MAX)?;
            m.set_u32(slot + 16, empty_frame)?;
        }
        Ok(())
    })?;
    Ok(())
}

async fn reset(cx: &Cx) -> Applied {
    clear(cx).await?;
    // D3D calls in first shader begin. A simple setter also updates its
    // distinct shadow; the other entry points own their shadows.
    invoke(cx, GuestCall::new(0x003f_5b40).args([0])).await?;
    invoke(cx, GuestCall::new(0x003f_54f0).args([2])).await?;
    simple(cx, 0x40354, 0x203, 0x0040_757c).await?;
    invoke(cx, GuestCall::new(0x003f_59b0).args([0])).await?;
    simple(cx, 0x40300, 0, 0x0040_7588).await?;
    invoke(cx, GuestCall::new(0x003f_5b20).args([0])).await?;
    simple(cx, 0x40340, 0, 0x0040_758c).await?;
    invoke(cx, GuestCall::new(0x003f_53d0).args([0x900])).await?;
    invoke(cx, GuestCall::new(0x003f_5450).args([0x900])).await?;
    cx.write(|m| -> Fault<_> {
        for stage in 0..4 {
            m.set_u32(STAGE_STATES + stage * 0x80 + 0x28, 0)?;
            m.set_u32(TEXTURES + stage * 4, 0)?;
        }
        or_word(m, states::DIRTY, 15)?;
        m.set_u8(FIRST_DRAW, 0)
    })?;
    Ok(())
}

fn nonempty(cx: &Cx, range: u32) -> Fault<bool> {
    cx.read(|m| m.u16(range).map(|span| span >> 9 != 0))
}

pub(super) fn textures_settled(m: &dyn GuestMemory) -> Fault<bool> {
    let mut headers = [0; 32];
    m.read(TEXTURES - 16, &mut headers)?;
    Ok(headers[..16] == headers[16..])
}

/// Flush desired stage headers using D3D's resource-binding entry. Its
/// releases and retains occur in stage order, even when several slots
/// alias the same resource.
pub(super) async fn textures(cx: &Cx) -> Applied {
    for stage in 0..4 {
        let at = TEXTURES + stage * 4;
        let (want, bound) = cx.read(|m| -> Fault<_> { Ok((m.u32(at)?, m.u32(at - 16)?)) })?;
        if want != bound {
            invoke(cx, GuestCall::new(SET_TEXTURE).eax(stage).args([want])).await?;
            cx.write(|m| {
                let header = m.u32(at)?;
                m.set_u32(at - 16, header)
            })?;
        }
    }
    Ok(())
}

fn math_error(error: engine::light_constants::Error) -> EntryError {
    match error {
        engine::light_constants::Error::Memory(fault) => EntryError::Fault(fault),
        error => EntryError::IsolatedCall(format!("opaque material floating point: {error:?}")),
    }
}

fn instance_textures(m: &mut dyn GuestMemory, s: Selection, mxcsr: u32) -> Applied {
    let span = m.u16(s.bindings)?;
    if span >> 9 == 0 { return Ok(()); }
    let records = m.u32(s.instance + 8)?;
    let mut row = m.u32(s.template + 0x24)? + 4 * u32::from(span & 511);
    let default_frame = m.u32(DEFAULT_SLOT_FRAME)?;
    for _ in 0..span >> 9 {
        let dst = SHADER_CONTEXT + 0x320 + u32::from(m.u8(row)?) * 64;
        let src = records + u32::from(m.u8(row + 3)?) * 12;
        let bitmap = m.u32(src)?;
        let (bitmap, frame, index) = if bitmap == u32::MAX {
            let globals = m.u32(0x0048_5a80)?;
            (m.u32(globals + 0x64)?, 0, 0)
        } else {
            let frame = engine::render_loop::opaque_material::bitmap_frame(m.i32(src + 4)?, mxcsr)
                .map_err(math_error)?;
            (bitmap, frame, m.u32(src + 8)?)
        };
        m.set_u32(dst, bitmap)?;
        m.set_u32(dst + 0x18, frame)?;
        m.set_u32(dst + 0x14, index)?;
        m.set_u32(dst + 0x10, default_frame)?;
        row += 4;
    }
    Ok(())
}

fn instance_vectors(m: &mut dyn GuestMemory, s: Selection) -> Fault<()> {
    let span = m.u16(s.bindings + 4)?;
    if span >> 9 == 0 { return Ok(()); }
    let records = m.u32(s.instance + 0x18)?;
    let mut row = m.u32(s.template + 0x24)? + 4 * u32::from(span & 511);
    for _ in 0..span >> 9 {
        let word = u32::from(m.u16(row)?);
        let bank = word >> 4 & 1;
        let index = word & 15;
        or_word(m, SHADER_CONTEXT + 0x1730 + 4 * bank, 1 << index)?;
        let dst = SHADER_CONTEXT + 0x1530 + 0x100 * bank + 16 * index;
        let src = records + u32::from(m.u8(row + 3)?) * 16;
        if let Some(&mask) = tags::material::VECTOR_MODE_MASKS.get((word as usize >> 5) & 31) {
            for lane in 0..4 {
                if mask & (1 << lane) != 0 {
                    let value = m.u32(src + lane * 4)?;
                    m.set_u32(dst + lane * 4, value)?;
                }
            }
        }
        row += 4;
    }
    Ok(())
}

async fn sampler_stages(cx: &Cx, s: Selection) -> Applied {
    let span = cx.read(|m| m.u16(s.implementation + 4))?;
    if span >> 9 == 0 { return Ok(()); }
    let first = cx.read(|m| m.u32(s.definition + 0x1c))? + 24 * u32::from(span & 511);
    for stage in 0..u32::from(span >> 9) {
        let at = first + 24 * stage;
        let (border, color_key) = cx.write(|m| -> Fault<_> {
            let row = STAGE_STATES + stage * 0x80;
            let nibbles = m.u32(at)?;
            let values = [
                u32::from(m.u8(at + 4)?), u32::from(m.u8(at + 5)?), u32::from(m.u8(at + 6)?),
                nibbles >> 8 & 15, nibbles >> 4 & 15, nibbles >> 12 & 15, m.u32(at + 20)?,
                nibbles >> 28, nibbles >> 24 & 15, nibbles >> 20 & 15, m.u32(at + 16)?,
                nibbles >> 16 & 15,
            ];
            for (index, value) in values.into_iter().enumerate() {
                m.set_u32(row + index as u32 * 4, value)?;
            }
            or_word(m, states::DIRTY, 1u32.wrapping_shl(stage))?;
            Ok((m.u32(at + 8)?, m.u32(at + 12)?))
        })?;
        // These D3D setters use EBX for the packed value and EAX for the stage.
        invoke(cx, GuestCall::new(0x003f_5ea0).eax(stage).ebx(border)).await?;
        invoke(cx, GuestCall::new(0x003f_5ef0).eax(stage).ebx(color_key)).await?;
    }
    Ok(())
}

async fn upload_constants(cx: &Cx) -> Applied {
    const MAKE_PUSH_SPACE: u32 = 0x003f_aae0;
    const PUSH_BUFFER_SIZE: u32 = 0x0040_8648;
    const SET_VERTEX_CONSTANT: u32 = 0x003f_7230;
    for bank in 0..2 {
        let dirty = SHADER_CONTEXT + 0x1730 + bank * 4;
        let mut bits = cx.read(|m| m.u32(dirty))?;
        while bits != 0 {
            let index = bits.trailing_zeros();
            let row = SHADER_CONTEXT + 0x1530 + bank * 0x100 + index * 16;
            if bank == 0 {
                // The attribute bank goes through the inlined NV2A method
                // writer. Advance the ring pointer so D3D observes it at
                // exactly the next drain, including resource setters.
                let (device, mut put, limit, value) = cx.read(|m| -> Fault<_> {
                    let device = m.u32(states::DEVICE)?;
                    let mut value = [0; 16];
                    m.read(row, &mut value)?;
                    Ok((device, m.u32(device)?, m.u32(device + 4)?, value))
                })?;
                if put >= limit {
                    let size = cx.read(|m| m.u32(PUSH_BUFFER_SIZE))?;
                    put = cx.call(GuestCall::new(MAKE_PUSH_SPACE).eax(size).args([size >> 1])).await?.eax;
                }
                cx.write(|m| -> Fault<_> {
                    m.set_u32(put, 0x0010_1a00 + index * 16)?;
                    m.write(put + 4, &value)?;
                    m.set_u32(device, put + 20)
                })?;
            } else {
                invoke(cx, GuestCall::new(SET_VERTEX_CONSTANT).ecx(18 + index).edx(row)).await?;
            }
            bits &= bits - 1;
            cx.write(|m| {
                let remaining = m.u32(dirty)? & !(1 << index);
                m.set_u32(dirty, remaining)
            })?;
        }
    }
    Ok(())
}

async fn publish_pixel_shader(cx: &Cx) -> Applied {
    let ctx = SHADER_CONTEXT;
    if cx.read(|m| m.u8(ctx + 0x1514))? != 0 {
        let entry = cx.write(|m| -> Fault<_> {
            let device = m.u32(states::DEVICE)?;
            m.set_u32(device + 0x924, 1)?;
            m.set_u32(device + 0x928, 0)?;
            m.set_u32(device + 0x92c, ctx + 0x1424)?;
            Ok(device + 0x924)
        })?;
        invoke(cx, GuestCall::new(0x003f_a3d0).eax(entry)).await?;
        cx.write(|m| m.set_u8(ctx + 0x1514, 0))?;
    } else {
        // 0x18ee0 emits individual dirty combiner constants. The final
        // combiner uses its own method and takes bit eight out of each
        // stage mask before either set of stages is flushed.
        let masks = cx.write(|m| -> Fault<_> {
            let masks = [m.u32(ctx + 0x1518)?, m.u32(ctx + 0x151c)?];
            for (i, mask) in masks.into_iter().enumerate() {
                m.set_u32(ctx + 0x1518 + i as u32 * 4, mask & !(1 << 8))?;
            }
            Ok(masks)
        })?;
        for bank in 0..2 {
            let dirty = ctx + 0x1518 + bank * 4;
            let mut bits = masks[bank as usize] & !(1 << 8);
            while bits != 0 {
                let stage = bits.trailing_zeros();
                bits &= bits - 1;
                cx.write(|m| m.set_u32(dirty, bits))?;
                let value = cx.read(|m| m.u32(ctx + 0x144c + bank * 32 + stage * 4))?;
                dynamic::render_state(cx, 10 + bank * 8 + stage, value).await?;
            }
        }
        for (bank, mask) in masks.into_iter().enumerate() {
            if mask & (1 << 8) == 0 { continue; }
            let offset = bank as u32 * 4;
            let value = cx.read(|m| m.u32(ctx + 0x14d0 + offset))?;
            simple(cx, 0x41e20 + offset, value, 0x0040_7544 + offset).await?;
        }
    }
    Ok(())
}

pub(super) async fn handoff(cx: &Cx, prepared: &Prepared, fcw: u16, mxcsr: u32) -> Applied {
    let ctx = SHADER_CONTEXT;
    let Prepared { call, selection: s, distance, spans } = *prepared;
    cx.write(|m| {
        let next = m.u32(RENDER_RECORDS + u32::from(call.record) * RENDER_RECORD_SIZE + 0x10)?;
        lighting(m, next)
    })?;
    if cx.read(|m| m.u8(FIRST_DRAW))? != 0 { reset(cx).await?; }
    let (shader_changed, changed, old_definition, old_implementation) = cx.write(|m| -> Fault<_> {
        let values = [u32::from(m.u16(s.shader + 0x3e)?), u32::from(m.u16(s.shader + 0x40)?),
            m.u32(s.shader + 0x44)?, m.u32(s.shader + 0x48)?];
        for (i, v) in values.into_iter().enumerate() { m.set_u32(SHADER_VALUES + i as u32 * 4, v)?; }
        let old_shader = m.u32(ctx)?;
        let old_definition = m.u32(ctx + 0x20)?;
        let old_implementation = m.u32(ctx + 0x24)?;
        m.set_u32(ctx + 4, old_shader)?;
        m.set_u32(ctx, call.shader)?;
        m.set_u32(ctx + 0x28, distance[0])?;
        m.set_u32(ctx + 0x2c, distance[1])?;
        m.set_u32(ctx + 0x58, 0)?;
        m.set_u8(ctx + 0x1514, 0)?;
        m.set_u32(ctx + 0x1518, 0)?;
        m.set_u32(ctx + 0x151c, 0)?;
        Ok((call.shader != old_shader, old_implementation != s.implementation ||
            old_definition != s.definition, old_definition, old_implementation))
    })?;
    let imp = s.implementation;
    if shader_changed || changed {
        cx.write(|m| -> Applied {
            for (i, address) in [s.instance, s.template, s.lod, s.pass, s.bindings].into_iter().enumerate() {
                m.set_u32(ctx + 0x0c + i as u32 * 4, address)?;
            }
            for (i, at) in [0x0048_506c, 0x0048_5070, 0x0048_509c, 0x0048_50a0, 0x0048_505c]
                .into_iter().enumerate() {
                let bits = m.u32(at)?;
                or_word(m, ctx + 0x30 + i as u32 * 4, bits)?;
            }
            instance_textures(m, s, mxcsr)?;
            instance_vectors(m, s)?;
            Ok(())
        })?;
        if changed {
            if old_implementation != 0 && old_definition != 0 {
                fixed_states(cx, old_definition, old_implementation + 0x11c).await?;
            }
            cx.write(|m| -> Fault<_> {
                m.set_u32(ctx + 0x24, imp)?;
                m.set_u32(ctx + 0x20, s.definition)?;
                for (i, offset) in [0x10c, 0x110, 0x104, 0x108, 0x114].into_iter().enumerate() {
                    let mask = m.u32(imp + offset)?;
                    m.set_u32(ctx + 0x44 + i as u32 * 4, mask)?;
                }
                Ok(())
            })?;
            fixed_states(cx, s.definition, imp + 2).await?;
            sampler_stages(cx, s).await?;
        }
    }
    registers::inputs(cx, ctx, fcw, mxcsr).await?;
    let variant_changed = cx.read(|m| -> Fault<_> { Ok(m.u32(ctx + 0x54)? & m.u32(ctx + 0x58)? != 0) })?;
    if changed || variant_changed {
        cx.write(|m| m.set_u8(ctx + 0x1514, 1))?;
        if cx.read(|m| m.u8(imp + 0xf7))? & 0xfe != 0 {
            cx.write(|m| pixel::build(m, ctx))?;
        } else {
            cx.write(|m| {
                let mut definition = [0; 0xf0];
                m.read(imp + 6, &mut definition)?;
                m.write(ctx + 0x1424, &definition)
            })?;
        }
    }
    // Unchanged shader+implementation keeps the original cached template
    // binding row, even when the selected pass is an alias in another LOD.
    if changed || shader_changed || variant_changed {
        let binding = cx.read(|m| m.u32(ctx + 0x1c))? + 2;
        if nonempty(cx, binding)? { registers::instance_colours(cx, ctx, binding).await?; }
    }
    if let Some(leaf) = s.animation { animation::textures(cx, ctx, leaf, fcw, mxcsr).await?; }
    if spans[0] { stage_textures::bind(cx, imp, fcw, mxcsr).await?; }
    if spans[1] { registers::vertex(cx, ctx, imp + 0x124, mxcsr).await?; }
    if spans[2] { registers::pixel(cx, ctx, imp + 0x122, fcw, mxcsr).await?; }
    if spans[3] { dynamic::render(cx, ctx, imp + 0x11e, fcw).await?; }
    if spans[4] { dynamic::sampler(cx, ctx, imp + 0x120, fcw).await?; }
    if let Some(leaf) = s.animation { animation::constants_states(cx, ctx, leaf, fcw, mxcsr).await?; }
    upload_constants(cx).await?;
    if cx.read(|m| m.u32(ctx + 0x1420))? != 0 { stage_textures::resolve(cx, fcw, mxcsr).await?; }
    textures(cx).await?;
    publish_pixel_shader(cx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::entry::{self, EntryFuture, Regs, Ret};
    use guest_layout::testing::FakeGuest;

    #[test]
    fn layer_reset_applies_outgoing_restore_before_releasing_implementation() {
        let mut m = FakeGuest::new();
        let definition = 0x0020_0000;
        let imp = definition + 0x100;
        let records = definition + 0x300;
        let register = 4;
        let stage0 = SHADER_CONTEXT + 0x1320;
        m.map(definition, 0x400);
        m.map(SHADER_CONTEXT, 0x1800);
        m.map(DEFAULT_SLOT_FRAME, 4);
        m.map(states::HEADERS, 32);
        m.map(states::SHADOW, 32);
        m.put_u32(SHADER_CONTEXT + 0x20, definition);
        m.put_u32(SHADER_CONTEXT + 0x24, imp);
        m.put_u32(definition + 0x14, records);
        m.put_u16(imp + 0x11c, 1 << 9 | 1);
        m.put_u8(records + 5, register as u8);
        m.put_u32(records + 6, 0x6543);
        m.put_u32(states::HEADERS + register * 4, 0x1234);
        m.put_u32(DEFAULT_SLOT_FRAME, 0x123);
        m.put_u32(stage0 + 8, 0x8877);
        m.put_u32(SHADER_CONTEXT + 0x1424, 0x5678);
        let run = entry::testing::run(&|cx| -> EntryFuture {
            Box::pin(async move { clear(&cx).await?; Ok(Ret::new(0, 0)) })
        }, m, Regs::default(), |call, m| {
            assert_eq!((call.target, call.ecx, call.edx), (states::SIMPLE, Some(0x1234), Some(0x6543)));
            assert_eq!(m.u32(SHADER_CONTEXT + 0x24), Ok(imp));
            Regs::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(run.calls.len(), 1);
        let m = run.guest;
        assert_eq!(m.u32(states::SHADOW + register * 4), Ok(0x6543));
        assert_eq!(m.u32(SHADER_CONTEXT + 0x24), Ok(0));
        assert_eq!(m.u32(SHADER_CONTEXT), Ok(u32::MAX));
        assert_eq!(m.u32(stage0), Ok(u32::MAX));
        assert_eq!(m.u32(stage0 + 16), Ok(0x123));
        assert_eq!(m.u32(stage0 + 8), Ok(0x8877));
        assert_eq!(m.u32(SHADER_CONTEXT + 0x1424), Ok(0x5678));
    }

    #[test]
    fn constant_flush_reserves_ring_with_original_byte_counts() {
        const DEVICE: u32 = 0x0030_0000;
        const OLD_PUT: u32 = DEVICE + 0x80;
        const NEW_PUT: u32 = DEVICE + 0x100;
        let mut m = FakeGuest::new();
        m.map(states::DEVICE, 4).put_u32(states::DEVICE, DEVICE);
        m.map(DEVICE, 0x200).put_u32(DEVICE, OLD_PUT).put_u32(DEVICE + 4, OLD_PUT);
        m.map(SHADER_CONTEXT + 0x1530, 0x208);
        m.put_u32(SHADER_CONTEXT + 0x1730, 2);
        for i in 0..4 {
            m.put_u32(SHADER_CONTEXT + 0x1540 + 4*i, 200+i);
        }
        m.map(0x0040_8648, 4).put_u32(0x0040_8648, 512);
        let run = entry::testing::run(&|cx| -> EntryFuture {
            Box::pin(async move { upload_constants(&cx).await?; Ok(Ret::new(0,0)) })
        }, m, Regs::default(), |call, _| {
            assert_eq!((call.target, call.eax), (0x003f_aae0, Some(512)));
            assert_eq!(call.args, [256]);
            Regs { eax: NEW_PUT, ..Regs::default() }
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(run.calls.len(), 1);
        let m = run.guest;
        assert_eq!(m.u32(DEVICE), Ok(NEW_PUT + 20));
        assert_eq!(m.u32(SHADER_CONTEXT + 0x1730), Ok(0));
        for (i, word) in [0x0010_1a10, 200, 201, 202, 203].into_iter().enumerate() {
            assert_eq!(m.u32(NEW_PUT + 4*i as u32), Ok(word));
        }
    }
}
