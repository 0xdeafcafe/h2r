//! The ordinary sorted BSP handler (0x4d0b0). A draw is submitted between
//! the same state transitions as the indexed emitter. The outgoing cache
//! and device are live before control returns to the sorted list.

use engine::entry::{Cx, EntryError, GuestCall};
use engine::memory::{GuestMemory, MemFault};
use native_render::object_nodes::{BLOCK_MATRICES, BLOCK_PARTS, MATRIX_STRIDE, NODE_BLOCKS};
use native_render::world_ranges::{Emitter, IndexedRange, Refusal, SortedPart};

use super::{OpaqueDraw, OpaqueSubmitError, opaque_material};

const CURRENT_SHADER: u32 = 0x0048_5b50;
const TEMPLATE_LAYER: u32 = 0x0003_5510;
const BIND_TARGET: u32 = 0x0001_4bc0;
const COPY_TARGET: u32 = 0x0003_52e0;
const FIRST_SHADER_DRAW: u32 = 0x0046_70bc;
const BSP_TARGET_READY: u32 = 0x0050_9430;
const NODE_CACHE: u32 = 0x0050_93d8;
const NODE_BLOCK_BOUND: u32 = 0x0048_5af0;
const COMPRESSED: u32 = 0x0048_5af1;
const FLOAT_ONE: u32 = 0x0045_dbc0;
const SET_CONSTANT_DWORDS: u32 = 0x003f_7330;
const BITMAP_TICK: u32 = 0x004e_6488;
const BITMAP_LOOKUP: u32 = 0x0012_ccf0;
const REQUEST_BITMAP: u32 = 0x0012_ce00;
const SET_STIPPLE: u32 = 0x003f_8cc0;
const SET_DEPTH_BIAS: u32 = 0x0001_2d50;
const PREPARE_BSP_PARAMETERS: u32 = 0x0003_c650;
const UPLOAD_BSP_PARAMETERS: u32 = 0x0003_d000;
const ENGINE_RENDER_STATES: u32 = 0x004b_81fc;
const STENCIL_MODE: u32 = 0x0046_7010;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Emission {
    Native(usize),
    Indexed(&'static str, usize),
    NoDraw,
}

fn tag(mem: &dyn GuestMemory, datum: u32) -> Result<u32, MemFault> {
    let table = mem.u32(engine::tags::TAG_ENTRIES_GLOBAL)?;
    mem.u32(table.wrapping_add(16 * (datum & 0xffff) + 8))
}

/// Pass transient constant words in the call's own argument area, after
/// its count. EDX points above the return token and the actual argument.
/// The entry driver restores the entire stack after the call. No pointer
/// into Rust memory or a shared scratch buffer crosses the boundary.
async fn rows(cx: &Cx, register: u32, data: [u32; 12]) -> Result<(), EntryError> {
    let address = cx.entry().esp.wrapping_sub(48);
    cx.call(GuestCall::new(SET_CONSTANT_DWORDS).ecx(register).edx(address)
        .args(std::iter::once(12).chain(data))).await?;
    Ok(())
}

async fn world_constants(cx: &Cx, record: u32) -> Result<(), EntryError> {
    let (matrix, handle) = cx.read(|m| {
        let matrix = m.u32(record + 0x1c)?;
        let handle = if matrix == 0 { m.u32(record + 8)? } else { u32::MAX };
        Ok::<_, MemFault>((matrix, handle))
    })?;
    if matrix != 0 {
        // A placement stores the scale followed by four xyz basis/position
        // vectors. The program receives the transpose, ignoring the scale.
        let data = cx.read(|m| {
            let mut data = [0; 12];
            for (n, v) in data.iter_mut().enumerate() {
                *v = m.u32(matrix.wrapping_add(4 + 4 * (n / 4) as u32 + 12 * (n % 4) as u32))?;
            }
            Ok::<_, MemFault>(data)
        })?;
        rows(cx, 50, data).await?;
        cx.write(|m| m.set_u32(NODE_CACHE, 0))?;
    } else if handle != u32::MAX {
        // The model datum of a BSP record is -1. Its section has no
        // model-node override, including for a packed type-1 record.
        let (address, count) = cx.read(|m| {
            let block = if handle as i32 >= 0 { NODE_BLOCKS + (handle & 0x0fff_ffff) } else { 0 };
            let index = m.u32(record + 12)? & 15;
            let span = block + BLOCK_PARTS + index * 4;
            let first = i32::from(m.u16(span)? as i16);
            let address = block.wrapping_add(BLOCK_MATRICES).wrapping_add_signed(first * MATRIX_STRIDE as i32);
            Ok::<_, MemFault>((address, 12 * u32::from(m.u16(span + 2)?)))
        })?;
        cx.call(GuestCall::new(SET_CONSTANT_DWORDS).ecx(50).edx(address).args([count])).await?;
        cx.write(|m| m.set_u8(NODE_BLOCK_BOUND, 1))?;
    }
    if cx.read(|m| m.u8(COMPRESSED))? != 0 {
        // 0x4d7c0 always passes (null, flags=0) for a structure BSP.
        let one = cx.read(|m| m.u32(FLOAT_ONE))?;
        rows(cx, 170, [one, one, one, one, 0, 0, 0, 0, one, one, 0, 0]).await?;
        cx.write(|m| m.set_u8(COMPRESSED, 0))?;
    }
    Ok(())
}

async fn restore_target(cx: &Cx, target: u32) -> Result<(), EntryError> {
    cx.call(GuestCall::new(BIND_TARGET).eax(1).args([target, 0])).await?;
    Ok(())
}

async fn clear_shader(cx: &Cx) -> Result<(), EntryError> {
    cx.write(|m| m.set_u8(FIRST_SHADER_DRAW, 1))?;
    opaque_material::clear(cx).await
}

/// The two target-specific passes of the ordinary material prologue
/// (0x1c0d0). The second target's shader is read after the first pass.
async fn auxiliary_targets(cx: &Cx, material: u32, layer: u32) -> Result<(), EntryError> {
    let template = cx.read(|m| {
        let datum = m.u32(material + 4)?;
        if datum == u32::MAX { return Ok(None); }
        let template = tag(m, datum)?;
        let active = m.u32(template + 0x44)? != u32::MAX || m.u32(template + 0x50)? != u32::MAX;
        Ok::<_, MemFault>(active.then_some(template))
    })?;
    let Some(template) = template else { return Ok(()) };
    let mut changed = false;
    for (offset, target_slot) in [(0x44, 13), (0x50, 17)] {
        let pass = cx.read(|m| {
            let shader = m.u32(template + offset)?;
            if shader == u32::MAX { return Ok(None); }
            Ok::<_, MemFault>(Some((shader, i32::from(m.u16(template + offset + 4)? as i16) as u32)))
        })?;
        if let Some((shader, template_layer)) = pass {
            let selected = cx.call(GuestCall::new(TEMPLATE_LAYER).eax(template_layer)).await?;
            if selected.eax == layer {
                cx.call(GuestCall::new(engine::points::TEMPLATE_PASS.base)
                    .args([shader, layer, u32::MAX, 0, target_slot, 1])).await?;
                changed = true;
            }
        }
    }
    if changed {
        let target = cx.read(|m| {
            let target = m.u32(native_render::shadows::CURRENT_TARGET)?;
            if target != u32::MAX { Ok(target) }
            else { m.u16(engine::screen_effect::TARGET_OVERRIDE).map(u32::from) }
        })?;
        restore_target(cx, target).await?;
        clear_shader(cx).await?;
    }
    Ok(())
}

async fn prepare_bsp_target(cx: &Cx) -> Result<(), EntryError> {
    if cx.read(|m| m.u8(BSP_TARGET_READY))? != 0 { return Ok(()); }
    clear_shader(cx).await?;
    if cx.read(|m| m.i32(m.u32(super::ACTIVE_BSP)? + 0x214))? > 0 {
        let restore = cx.read(|m| m.u32(engine::screen_effect::TARGET_OVERRIDE))?;
        cx.call(GuestCall::new(COPY_TARGET).args([22, 1])).await?;
        restore_target(cx, restore).await?;
    }
    cx.write(|m| m.set_u8(BSP_TARGET_READY, 1))?;
    clear_shader(cx).await
}

/// Material bit 0 requests residency for each bitmap data entry. The
/// streaming helpers update ages and can schedule I/O even when no slot
/// binding changes. Re-read counts and pointers after a service call.
async fn warm_bitmaps(cx: &Cx, material: u32, program: u32) -> Result<(), EntryError> {
    let instance = cx.read(|m| m.u32(material + 0x24))?;
    let mut index = 0;
    while index < cx.read(|m| m.i32(instance + 4))? {
        let datum = cx.read(|m| m.u32(m.u32(instance + 8)?.wrapping_add(index as u32 * 12)))?;
        if datum != u32::MAX {
            let bitmap = cx.read(|m| tag(m, datum))?;
            let mut entry = 0;
            while entry < cx.read(|m| m.i32(bitmap + 0x44))? {
                let pending = cx.read(|m| {
                    let data = m.u32(bitmap + 0x48)?.wrapping_add((entry as u32).wrapping_mul(0x74));
                    if data == 0 || (m.i32(data + 0x70)? > m.i32(BITMAP_TICK)? && m.u32(data + 0x50)? != 0) {
                        return Ok(None);
                    }
                    Ok::<_, MemFault>(Some(data))
                })?;
                if let Some(data) = pending {
                    let ready = cx.call(GuestCall::new(BITMAP_LOOKUP).edx(14).esi(data).xmm1(0)).await?;
                    if ready.eax == 0 {
                        cx.call(GuestCall::new(REQUEST_BITMAP).args([data, 14]).xmm1(0)).await?;
                    }
                }
                entry += 1;
            }
        }
        index += 1;
    }
    if cx.read(|m| m.u32(program + 0x118))? & (1 << 20) != 0 {
        prepare_bsp_target(cx).await?;
    }
    Ok(())
}

/// 0x15370, the three modes used around a BSP draw. Update the cached
/// engine state before each setter. The XDK setter maintains D3D's own
/// shadows and pushes. Both can be observed during a resource callback.
async fn stencil(cx: &Cx, mode: u16) -> Result<(), EntryError> {
    const ENABLE: u32 = 0x90;
    const FAIL: u32 = 0x91;
    const DEPTH_FAIL: u32 = 0x44;
    const PASS: u32 = 0x45;
    const FUNC: u32 = 0x46;
    const REFERENCE: u32 = 0x47;
    const MASK: u32 = 0x48;
    const WRITE_MASK: u32 = 0x49;
    const KEEP: u32 = 0x1e00;
    const REPLACE: u32 = 0x1e01;
    const ALWAYS: u32 = 0x207;
    let states = [(ENABLE, u32::from(mode != 0)), (FAIL, KEEP), (DEPTH_FAIL, KEEP),
        (PASS, REPLACE), (FUNC, ALWAYS), (REFERENCE, if mode == 2 { 255 } else { 0 }),
        (MASK, 255), (WRITE_MASK, 255)];
    for &(slot, value) in &states[..if mode == 0 { 1 } else { states.len() }] {
        cx.write(|m| m.set_u32(ENGINE_RENDER_STATES + slot * 4, value))?;
        super::opaque_material_states::render_state(cx, slot, value).await?;
    }
    cx.write(|m| m.set_u16(STENCIL_MODE, mode))?;
    Ok(())
}

async fn record_states(cx: &Cx, p: &SortedPart, enter: bool, shader_flags: u16, fcw: u16,
    mxcsr: u32) -> Result<(), EntryError> {
    use engine::render_loop::opaque_material::{CACHED_FADE, stipple};
    let (byte, active) = cx.read(|m| {
        let byte = m.u8(p.record_addr + 0x1a)?;
        let flags = m.u32(p.record_addr)?;
        let active = byte != 0xff || flags & (1 << 11) != 0 ||
            (matches!(p.layer, 1 | 3) && (flags & (1 << 14) != 0 ||
                m.u32(native_render::skip::STENCIL_OFF_FLAGS)? & 0x00ff_ff00 != 0)) ||
            (enter && shader_flags & 1 != 0 && m.i32(m.u32(super::ACTIVE_BSP)? + 0x214)? > 0);
        Ok::<_, MemFault>((byte, active))
    })?;
    if !active { return Ok(()); }
    if byte != 0xff {
        let pattern = cx.read(|m| stipple(m, byte, enter, fcw, mxcsr)).map_err(|e| match e {
            engine::light_constants::Error::Memory(e) => EntryError::Fault(e),
            e => EntryError::IsolatedCall(format!("world stipple arithmetic: {e:?}")),
        })?;
        if let Some((fade, address)) = pattern {
            cx.call(GuestCall::new(SET_STIPPLE).args([address])).await?;
            cx.write(|m| m.set_u32(CACHED_FADE, fade))?;
        }
    }
    if cx.read(|m| m.u32(p.record_addr))? & (1 << 11) != 0 {
        cx.call(GuestCall::new(SET_DEPTH_BIAS).eax(0).ebx(u32::from(enter)).args([0])).await?;
    }
    if matches!(p.layer, 1 | 3) {
        let forced_off = cx.read(|m| m.u32(native_render::skip::STENCIL_OFF_FLAGS))? & 0x00ff_ff00 != 0;
        if forced_off || cx.read(|m| m.u32(p.record_addr))? & (1 << 14) != 0 {
            stencil(cx, if enter { if forced_off { 0 } else { 2 } } else { 1 }).await?;
        }
    }
    if enter && shader_flags & 1 != 0 {
        let parameters = cx.read(|m| {
            let bsp = m.u32(super::ACTIVE_BSP)?;
            if m.i32(bsp + 0x214)? > 0 { m.u32(bsp + 0x218).map(Some) } else { Ok(None) }
        })?;
        if let Some(address) = parameters {
            cx.call(GuestCall::new(PREPARE_BSP_PARAMETERS).esi(address)).await?;
            cx.call(GuestCall::new(UPLOAD_BSP_PARAMETERS).esi(address)).await?;
        }
    }
    Ok(())
}

fn emitter_fault(refusal: Refusal) -> EntryError {
    EntryError::IsolatedCall(format!("unreadable BSP indexed emitter state: {refusal:?}"))
}

/// The emitter evaluates its first address/count before flushing textures.
/// Each indexed ABI call can run a fence or release callback that rewrites
/// later subparts. Resume from the saved loop state after each such call.
async fn emit(cx: &Cx, part: &SortedPart, draw: Box<dyn OpaqueDraw>) -> Result<Emission, EntryError> {
    let (mut cursor, first) = cx.write(|m| {
        let mem = super::Mem(m);
        let mut cursor = Emitter::start(&mem, part)?;
        let first = cursor.next(&mem)?;
        Ok::<_, Refusal>((cursor, first))
    }).map_err(emitter_fault)?;
    let Some(mut range) = first else { return Ok(Emission::NoDraw) };

    opaque_material::textures(cx).await?;
    // Hold the same guest-memory borrow for validation and GPU submission.
    // BeforeDraw guarantees no packet was sent and leaves the cursor at the
    // already-frozen first range. A device error may follow a submitted prefix.
    let reason = match cx.write(|m| {
        if !cursor.matches_prepared(&super::Mem(m), range, part).unwrap_or(false) {
            return Ok(Err(OpaqueSubmitError::BeforeDraw("world index ranges changed during setup")));
        }
        if part.indices.len() > 1 && !opaque_material::textures_settled(m)? {
            // A release changed an earlier slot. The next range must flush
            // that slot between its address lookup and its draw.
            return Ok(Err(OpaqueSubmitError::BeforeDraw("world texture callback changed an earlier stage")));
        }
        Ok::<_, MemFault>(draw.submit(m, part))
    })? {
        Ok(()) => return Ok(Emission::Native(part.indices.len())),
        Err(OpaqueSubmitError::Device(error)) => return Err(EntryError::IsolatedCall(error)),
        Err(OpaqueSubmitError::BeforeDraw(reason)) => reason,
    };
    let primitive = if cursor.strip { 6 } else { 5 };
    let mut issued = 0;
    loop {
        let IndexedRange { address, count } = range;
        cx.call(GuestCall::new(native_render::world_layers::DRAW_INDEXED_VERTICES)
            .args([primitive, count, address])).await?;
        issued += 1;
        let next = cx.write(|m| cursor.next(&super::Mem(m))).map_err(emitter_fault)?;
        let Some(next) = next else { break };
        range = next;
        opaque_material::textures(cx).await?;
    }
    Ok(Emission::Indexed(reason, issued))
}

pub(super) async fn handoff(cx: &Cx, part: &SortedPart, draw: Box<dyn OpaqueDraw>,
    fcw: u16, mxcsr: u32, prepared: &opaque_material::Prepared) -> Result<Emission, EntryError> {
    let material = prepared.shader_address();
    let flags = cx.write(|m| {
        m.set_u32(CURRENT_SHADER, part.shader)?;
        m.u16(material + 0x16)
    })?;
    auxiliary_targets(cx, material, part.layer).await?;
    world_constants(cx, part.record_addr).await?;
    if flags & 1 != 0 {
        // The lightmap path has no writes for these shaders.
        warm_bitmaps(cx, material, part.program).await?;
    }
    let streams = draw.bind_streams(cx).await?;
    if !streams { return Ok(Emission::NoDraw); }
    opaque_material::handoff(cx, prepared, fcw, mxcsr).await?;
    record_states(cx, part, true, flags, fcw, mxcsr).await?;
    let outcome = emit(cx, part, draw).await?;
    record_states(cx, part, false, flags, fcw, mxcsr).await?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::entry::{EntryFuture, Regs, Ret, testing};
    use engine::testing::FakeGuest;

    #[test]
    fn stale_ranges_retain_the_first_address_and_observe_every_indexed_callback() {
        use native_render::world::Owner;
        const RECORD: u32 = 0x10000;
        const GEOM: u32 = 0x11000;
        const PART: u32 = 0x12000;
        const SUBS: u32 = 0x13000;
        const MASK: u32 = 0x14000;
        const DESIRED: u32 = 0x0051_f3d8;
        const SET_TEXTURE: u32 = 0x003f_8320;
        const INDEXED: u32 = native_render::world_layers::DRAW_INDEXED_VERTICES;

        struct Stale;
        impl OpaqueDraw for Stale {
            fn bind_streams<'a>(&'a self, _: &'a Cx) -> super::super::OpaqueStreams<'a> {
                unreachable!("already at emitter")
            }

            fn submit(self: Box<Self>, _: &mut dyn GuestMemory, _: &SortedPart)
                -> Result<(), OpaqueSubmitError> {
                panic!("the texture callback changed the prepared ranges")
            }
        }

        let mut m = FakeGuest::new();
        m.map(RECORD, 0x5000);
        m.map(DESIRED - 16, 32);
        m.put_u32(RECORD, (1 << 6) | (1 << 9)).put_u32(RECORD + 0x14, MASK);
        m.put_u32(GEOM + 8, 3).put_u32(GEOM + 12, SUBS).put_u32(GEOM + 0x24, 0x15000);
        m.put_u16(PART + 12, 3).put_u32(MASK, 3).put_u32(DESIRED, 18);
        m.put_u32(SUBS, 3 << 16).put_u32(SUBS + 8, (3 << 16) | 3);
        let f = |cx: Cx| -> EntryFuture { Box::pin(async move {
            let part = SortedPart { owner: Owner::Cluster(0), part: 0, record: 0, record_addr: RECORD,
                geometry: GEOM, part_addr: PART, strip: true, index_buffer: 0x15000, indices: vec![0..3, 3..6],
                sorted_slot: 0, section: 0, shader: 0, layer_argument: 0, lod: 0, variant: 0,
                projected_size: 0, layer: 3, pass: 0, implementation: 0, program: 0, classification: 0 };
            let outcome = emit(&cx, &part, Box::new(Stale)).await?;
            assert_eq!(outcome, Emission::Indexed("world index ranges changed during setup", 3));
            Ok(Ret::new(0, 28))
        }) };
        let mut draws = 0;
        let out = testing::run(&f, m, Regs::default(), |call, m| {
            match call.target {
                SET_TEXTURE => {
                    // This happens after the emitter froze the first address.
                    m.set_u32(GEOM + 0x24, 0x16000).unwrap();
                    m.set_u32(SUBS + 8, (6 << 16) | 9).unwrap();
                }
                INDEXED => {
                    draws += 1;
                    match draws {
                        1 => {
                            assert_eq!(call.args, [6, 3, 0x15000]);
                            m.set_u32(MASK, 7).unwrap();
                            m.set_u32(SUBS + 8, (6 << 16) | 18).unwrap();
                        }
                        2 => {
                            assert_eq!(call.args, [6, 6, 0x16000 + 36]);
                            m.set_u32(GEOM + 0x24, 0x17000).unwrap();
                            m.set_u32(SUBS + 16, (3 << 16) | 26).unwrap();
                        }
                        3 => assert_eq!(call.args, [6, 3, 0x17000 + 52]),
                        _ => unreachable!(),
                    }
                }
                _ => panic!("unexpected callback {call:?}"),
            }
            Regs::default()
        });
        assert_eq!(out.result, Ok(Ret::new(0, 28)));
        assert_eq!(out.calls.len(), 4);
    }

    #[test]
    fn placement_transposes_all_matrix_words_and_resets_compression_once() {
        let (record, matrix, stack) = (0x10000, 0x11000, 0x15000);
        let mut m = FakeGuest::new();
        m.put_u32(record + 0x1c, matrix).put_u32(NODE_CACHE, 123)
            .put_u8(COMPRESSED, 1).put_u32(FLOAT_ONE, 1f32.to_bits());
        let input = [0x7fa00005, 11, 12, 20, 21, 22, 30, 31, 32, 40, 41, 0x80000000];
        for (i, &bits) in input.iter().enumerate() { m.put_u32(matrix + 4 + i as u32 * 4, bits); }
        let serial_rows = [input[0], 20, 30, 40, 11, 21, 31, 41, 12, 22, 32, input[11]];
        let f = move |cx: Cx| -> EntryFuture { Box::pin(async move {
            world_constants(&cx, record).await?;
            cx.write(|m| { m.set_u32(record + 0x1c, 0)?; m.set_u32(record + 8, u32::MAX) })?;
            world_constants(&cx, record).await?;
            Ok(Ret::new(0, 28))
        }) };
        let out = testing::run(&f, m, Regs { esp: stack, ..Default::default() }, |_, _| Regs::default());
        assert_eq!(out.result, Ok(Ret::new(0, 28)));
        assert_eq!(out.calls.len(), 2);
        let matrix_call = &out.calls[0];
        assert_eq!((matrix_call.target, matrix_call.ecx, matrix_call.edx), (SET_CONSTANT_DWORDS, Some(50), Some(stack - 48)));
        assert_eq!(matrix_call.args[0], 12);
        assert_eq!(matrix_call.args[1..], serial_rows);
        let identity = 1f32.to_bits();
        assert_eq!(out.calls[1].ecx, Some(170));
        assert_eq!(out.calls[1].args, [12, identity, identity, identity, identity,
            0, 0, 0, 0, identity, identity, 0, 0]);
        assert_eq!(out.guest.u32(NODE_CACHE), Ok(0));
        assert_eq!(out.guest.u8(COMPRESSED), Ok(0));
    }
}
