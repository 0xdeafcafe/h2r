//! The guest's Direct3D entries with no device behind them
//! (docs/graphics/d3d-entries.md).
//!
//! Every entry the retail D3D library exposes to game code (88 of them) is
//! overridden, and [`answer`] says how each one answers:
//!
//! - **Resources and fences**, reached outside the frame by the resource
//!   caches: `Direct3D_CreateDevice`, `D3DResource_AddRef` and `Release`,
//!   `D3DResource_IsBusy`, `D3D_BlockOnResource`, `D3D_SetFence`,
//!   `D3D_BlockOnTime`.
//! - **State setters**: the `SetRenderState_*` and `SetTextureState_*`
//!   entries store their value in the guest's own state arrays, which game
//!   code reads back, and push nothing ([`crate::device_state`]).
//! - **The frame's end**: `D3DDevice_Swap` is the frame step's seal
//!   ([`crate::frame_wait::swapped`]); `CDevice_KickOff` has no push buffer
//!   to hand over; the device teardown has nothing to free (this
//!   `CreateDevice` allocates nothing, and `SetTexture`/`SetPalette` never
//!   bind); `PersistDisplay` has no display to keep.
//! - **Gaps**: every other entry panics with its name and caller. They
//!   draw, or write the push buffer, or build GPU state a draw reads; their
//!   callers' draws belong to native stages, so a call that still reaches
//!   one is a gap to replace, found from the crash
//!   (docs/graphics/notes/2026-11-28-d3d-live-callers.md lists the callers
//!   still reachable).

use std::sync::atomic::{AtomicU32, Ordering};

use runtime::{CpuAbi, Dispatch, ExecCtx, GuestFault, TransferAbi};

use crate::device_state;

/// The guest's own device object (`D3D__Device`), whose address the game
/// keeps at [`P_DEVICE`].
pub const DEVICE: u32 = 0x0040_4fe0;
/// `D3D__pDevice`.
pub const P_DEVICE: u32 = 0x0040_7488;

/// Each entry: address, name, calling convention.
pub const ENTRIES: &[(u32, &str, &str)] = &[
    (0x003f5240, "Direct3D_CreateDevice", "EAX = BehaviorFlags, ECX = ppDevice, [esp+4] = pPresentationParameters; ret 4"),
    (0x003f52f0, "D3DDevice_SetRenderState_EdgeAntiAlias", "stdcall(Value); ret 4"),
    (0x003f5330, "D3DDevice_SetRenderState_ShadowFunc", "stdcall(Value); ret 4"),
    (0x003f5370, "D3DDevice_SetRenderState_FogColor", "stdcall(Value); ret 4"),
    (0x003f53d0, "D3DDevice_SetRenderState_CullMode", "stdcall(Value); ret 4"),
    (0x003f5450, "D3DDevice_SetRenderState_FrontFace", "stdcall(Value); ret 4"),
    (0x003f54a0, "D3DDevice_SetRenderState_NormalizeNormals", "stdcall(Value); ret 4"),
    (0x003f54f0, "D3DDevice_SetRenderState_ZEnable", "stdcall(Value); ret 4"),
    (0x003f5590, "D3DDevice_SetRenderState_StencilEnable", "stdcall(Value); ret 4"),
    (0x003f5640, "D3DDevice_SetRenderState_StencilFail", "stdcall(Value); ret 4"),
    (0x003f56c0, "D3DDevice_SetRenderState_TextureFactor", "stdcall(Value); ret 4"),
    (0x003f5730, "D3DDevice_SetRenderState_LineWidth", "stdcall(Value); ret 4"),
    (0x003f57a0, "D3DDevice_SetRenderState_Dxt1NoiseEnable", "stdcall(Value); ret 4"),
    (0x003f5830, "D3DDevice_SetRenderState_YuvEnable", "stdcall(Value); ret 4"),
    (0x003f5870, "D3DDevice_SetRenderState_OcclusionCullEnable", "stdcall(Value); ret 4"),
    (0x003f58e0, "D3DDevice_SetRenderState_StencilCullEnable", "stdcall(Value); ret 4"),
    (0x003f5950, "D3DDevice_SetRenderState_RopZCmpAlwaysRead", "stdcall(Value); ret 4"),
    (0x003f5970, "D3DDevice_SetRenderState_RopZRead", "stdcall(Value); ret 4"),
    (0x003f5990, "D3DDevice_SetRenderState_DoNotCullUncompressed", "stdcall(Value); ret 4"),
    (0x003f59b0, "D3DDevice_SetRenderState_ZBias", "stdcall(Value); ret 4"),
    (0x003f5a60, "D3DDevice_SetRenderState_MultiSampleMode", "stdcall(Value); ret 4"),
    (0x003f5a90, "D3DDevice_SetRenderState_MultiSampleRenderTargetMode", "stdcall(Value); ret 4"),
    (0x003f5ac0, "D3DDevice_SetRenderState_MultiSampleAntiAlias", "stdcall(Value); ret 4"),
    (0x003f5b00, "D3DDevice_SetRenderState_MultiSampleMask", "stdcall(Value); ret 4"),
    (0x003f5b20, "D3DDevice_SetRenderState_SampleAlpha", "stdcall(Value); ret 4"),
    (0x003f5b40, "D3DDevice_SetRenderState_LogicOp", "stdcall(Value); ret 4"),
    (0x003f5ba0, "D3DDevice_SetRenderState_FillMode", "stdcall(Value); ret 4"),
    (0x003f5bf0, "D3DDevice_SetRenderState_BackFillMode", "stdcall(Value); ret 4"),
    (0x003f5c10, "D3DDevice_SetRenderState_TwoSidedLighting", "stdcall(Value); ret 4"),
    (0x003f5c70, "D3DDevice_SetRenderState_VertexBlend", "stdcall(Value); ret 4"),
    (0x003f5cc0, "D3DDevice_SetRenderState_PSTextureModes", "stdcall(Value); ret 4"),
    (0x003f5cf0, "D3DDevice_SetRenderState_Simple", "ECX = NV2A method, EDX = value; ret"),
    (0x003f5d20, "D3DDevice_SetTextureState_TexCoordIndex", "ESI = stage, [esp+4] = value; ret 4"),
    (0x003f5e30, "D3DDevice_SetTextureState_BumpEnv", "EAX = stage, stack (Type, Value); ret 8"),
    (0x003f5ea0, "D3DDevice_SetTextureState_BorderColor", "EAX = stage, EBX = value; ret"),
    (0x003f5ef0, "D3DDevice_SetTextureState_ColorKeyColor", "EAX = stage, EBX = value; ret"),
    (0x003f6190, "D3D viewport push fragment", "EAX = put, ESI = device; returns EAX = new put; ret"),
    (0x003f6390, "D3D push fragment (0x3f6390)", "EDX = put, [esp+4] = device; returns EAX = put + 8; ret 4"),
    (0x003f6840, "D3D_UpdateProjectionViewportTransform", "no args; ret"),
    (0x003f6c60, "D3DDevice_LoadVertexShaderProgram", "stdcall(pFunction, Address); ret 8"),
    (0x003f6cf0, "D3DDevice_SelectVertexShaderDirect", "EAX = Address, ECX = pVAF; ret"),
    (0x003f6e10, "D3DDevice_SetShaderConstantMode", "EAX = mode; ret"),
    (0x003f6f40, "D3D transform/viewport push (0x3f6f40)", "ESI = device; returns EAX = new put when it pushes, else the shader object [ESI+0x794]; ret"),
    (0x003f71d0, "D3DDevice_SetVertexShaderConstant1", "ECX = Register + 96, EDX = pConstantData; ret"),
    (0x003f7230, "D3DDevice_SetVertexShaderConstant1Fast", "ECX = Register + 96, EDX = pConstantData; ret"),
    (0x003f7280, "D3DDevice_SetVertexShaderConstant4", "ECX = Register + 96, EDX = pConstantData; ret"),
    (0x003f7330, "D3DDevice_SetVertexShaderConstantNotInlineFast", "ECX = Register + 96, EDX = pConstantData, [esp+4] = DWORD count (pushed in batches of 16); ret 4"),
    (0x003f7410, "D3DDevice_SetVertexShaderConstantNotInline", "EBX = Register + 96, EDX = pConstantData, EAX = DWORD count; shadows at 0x407738 unless device flag 0x10, then NotInlineFast; ret"),
    (0x003f7440, "D3DDevice_SetVertexShaderInput", "stdcall(Handle, StreamCount, pStreamInputs); ret 0xC"),
    (0x003f77c0, "D3DDevice_Reset", "EBX = pPresentationParameters"),
    (0x003f7870, "D3DDevice_SetRenderTarget", "stdcall(pRenderTarget, pNewZStencil); ret 8"),
    (0x003f7b30, "D3DDevice_SetRenderTarget (same size, 0x3f7b30)", "[esp+4] = pRenderTarget, EBX = pNewZStencil; ret 4"),
    (0x003f8100, "D3DDevice_SetViewport", "stdcall(pViewport); ret 4"),
    (0x003f8320, "D3DDevice_SetTexture", "EAX = Stage, [esp+4] = pTexture; ret 4"),
    (0x003f84d0, "D3DDevice_SetPalette", "EAX = Stage, [esp+4] = pPalette; ret 4"),
    (0x003f85d0, "D3DDevice_EndVisibilityTest", "EAX = index; returns EAX = 0 (S_OK); ret"),
    (0x003f8630, "D3DDevice_GetVisibilityTestResult", "EAX = index, EDX = ULONGLONG* timestamp (optional), [esp+4] = UINT* count; returns EAX = 0, or D3DERR_TESTINCOMPLETE writing nothing; ret 4"),
    (0x003f8760, "D3DDevice_SetScissors", "stdcall(Count, Exclusive, pRects); ret 0xC"),
    (0x003f8ad0, "D3DDevice_PersistDisplay", "no args; returns EAX = HRESULT; ret"),
    (0x003f8c70, "D3D display/flicker filter (0x3f8c70)", "ESI = filter; ret"),
    (0x003f8cc0, "D3DDevice_SetStipple", "stdcall(pPattern): 32 dwords to device +0x197C and method 0x1480; ret 4"),
    (0x003f9d00, "D3DDevice_Swap", "EAX = Flags (0 = COPY|FINISH); returns EAX = swap count; ret"),
    (0x003f9df0, "D3DDevice_SetBackBufferScale", "stdcall(x, y); ret 8"),
    (0x003f9fc0, "D3DDevice_Clear", "stdcall(Count, pRects, Flags, Color, Z, Stencil); ret 0x18"),
    (0x003fa3d0, "D3DDevice_SetPixelShader", "EAX = Handle; ret"),
    (0x003fa680, "D3D_SetFence", "stdcall(Type); returns EAX = fence; ret 4"),
    (0x003fa970, "D3D_BlockOnTime", "2 stack args; ret 8 (no-op: the host GPU is synchronous)"),
    (0x003faae0, "D3D_MakeRequestedSpace", "[esp+4] = MinimumSpace, EAX = RequestedSpace; returns EAX = put; ret 4"),
    (0x003fac30, "CDevice_KickOff", "no args; EDX live across the call; ret"),
    (0x003fad50, "D3D_BlockOnResource", "EAX = pResource; ret (no-op: the host GPU is synchronous)"),
    (0x003faed0, "D3DDevice_DrawVertices", "EAX = VertexCount, ECX = StartVertex, [esp+4] = PrimitiveType; ret 4"),
    (0x003fafa0, "D3DDevice_DrawIndexedVertices", "stdcall(PrimitiveType, VertexCount, pIndexData); ret 0xC"),
    (0x003fb280, "D3DDevice_SetVertexData2f", "stdcall(Register, a, b); ret 0xC"),
    (0x003fb2d0, "D3DDevice_SetVertexData4f", "EDI = Register (-1 = position), stack (a, b, c, d); ret 0x10"),
    (0x003fb330, "D3DDevice_SetVertexData2s", "stdcall(Register, a, b); ret 0xC"),
    (0x003fb380, "D3DDevice_SetVertexData4ub", "stdcall(Register, a, b, c, d); ret 0x14"),
    (0x003fb3e0, "D3DDevice_SetVertexDataColor", "stdcall(Register, D3DCOLOR); ret 8"),
    (0x003fb440, "D3DDevice_Begin", "stdcall(PrimitiveType); ret 4"),
    (0x003fb480, "D3DDevice_End", "no args; ret"),
    (0x003fb600, "D3DResource_AddRef", "stdcall(this); returns EAX = new count; ret 4"),
    (0x003fb640, "D3DResource_Release", "stdcall(this); returns EAX = new count; ret 4"),
    (0x003fb710, "D3DResource_IsBusy (0x3fb710)", "ECX = resource; returns EAX = busy (0/1); ret"),
    (0x003fc530, "D3D device teardown (0x3fc530)", "EAX = device; ret"),
    (0x003fe005, "D3D miniport hardware init (0x3fe005)", "EAX = miniport; returns EAX = success"),
    (0x003feac0, "D3D GPU interrupt service routine (0x3feac0)", "stdcall(Interrupt, ServiceContext); returns AL = handled; ret 8"),
    (0x003fed90, "D3D miniport vertical-blank service (0x3fed90)", "EAX = miniport; returns EAX"),
    (0x003ff5c6, "D3D miniport gamma ramp (0x3ff5c6)", "EAX = miniport (device + 0x1C28), EDX = D3DGAMMARAMP (768 bytes); ret"),
    (0x00400fa0, "D3D commit deferred state (0x400fa0)", "no args; ret"),
];

/// The last fence [`set_fence`] handed out.
static FENCE: AtomicU32 = AtomicU32::new(0);

/// What an entry does: answer with `eax` and pop `cleanup` bytes, or panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    CreateDevice,
    AddRef,
    Release,
    Return { eax: Option<u32>, cleanup: u32 },
    SetFence,
    /// A state setter's store ([`device_state::set`]).
    State(device_state::Setter),
    /// `D3DDevice_Swap` (EAX = flags; returns EAX = the swap count; `ret`).
    Swap,
    Gap,
}

fn answer(addr: u32) -> Answer {
    if let Some(s) = device_state::setter(addr) {
        return Answer::State(s);
    }
    match addr {
        0x003f_5240 => Answer::CreateDevice,
        0x003f_b600 => Answer::AddRef,
        0x003f_b640 => Answer::Release,
        // D3DResource_IsBusy: never busy.
        0x003f_b710 => Answer::Return { eax: Some(0), cleanup: 0 },
        // D3D_BlockOnResource: nothing in flight.
        0x003f_ad50 => Answer::Return { eax: None, cleanup: 0 },
        0x003f_a680 => Answer::SetFence,
        // D3D_BlockOnTime: every fence has passed.
        0x003f_a970 => Answer::Return { eax: None, cleanup: 8 },
        0x003f_9d00 => Answer::Swap,
        // CDevice_KickOff: no push buffer to hand over; EDX untouched.
        0x003f_ac30 => Answer::Return { eax: None, cleanup: 0 },
        // The device teardown (from `boot_display_once` 0x12420 and the
        // shutdown 0x141c0): nothing bound, nothing allocated. The caller
        // zeroes the device object itself.
        0x003f_c530 => Answer::Return { eax: None, cleanup: 0 },
        // D3DDevice_PersistDisplay: S_OK, no display surface to keep.
        0x003f_8ad0 => Answer::Return { eax: Some(0), cleanup: 0 },
        // D3DDevice_SetPalette (from `rasterizer_initialize` 0x12560, the
        // scene reset 0x15b70, the lightmap bind 0x15c90, 0x445d0): the
        // native draws read the game's own palette cache (`0x484dc0`,
        // `game::native_targets_glue::stage_palette_header`), never D3D's.
        0x003f_84d0 => Answer::Return { eax: None, cleanup: 4 },
        // D3DDevice_SetShaderConstantMode (once, from 0x12560): the native
        // renderer has no constant-mode switch to set.
        0x003f_6e10 => Answer::Return { eax: None, cleanup: 0 },
        _ => Answer::Gap,
    }
}

/// Overrides every entry in [`ENTRIES`].
pub fn register(dispatch: &mut Dispatch) {
    for &(addr, name, convention) in ENTRIES {
        let answer = answer(addr);
        let _ = dispatch.register_override(addr, name, move |cpu, ctx, _| {
            call(cpu, ctx, addr, name, convention, answer)
        });
    }
}

fn call(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    addr: u32,
    name: &'static str,
    convention: &'static str,
    answer: Answer,
) -> Result<TransferAbi, GuestFault> {
    let mut mem = ctx.memory();
    let (eax, cleanup) = match answer {
        Answer::CreateDevice => {
            // ECX = ppDevice; S_OK. The device object is the guest's own.
            mem.set_u32(P_DEVICE, DEVICE)?;
            if cpu.ecx != 0 {
                mem.set_u32(cpu.ecx, DEVICE)?;
            }
            (Some(0), 4)
        }
        Answer::AddRef | Answer::Release => {
            // stdcall(this): the count is the low 16 bits of `Common`.
            let this = mem.u32(cpu.esp.wrapping_add(4))?;
            let common = mem.u32(this)?;
            let count = (common & 0xffff) as u16;
            let count = if answer == Answer::AddRef { count.wrapping_add(1) } else { count.saturating_sub(1) };
            mem.set_u32(this, (common & !0xffff) | count as u32)?;
            (Some(count as u32), 4)
        }
        Answer::Return { eax, cleanup } => (eax, cleanup),
        Answer::SetFence => (Some(FENCE.fetch_add(1, Ordering::Relaxed).wrapping_add(1)), 4),
        Answer::State(s) => (None, device_state::set(s, cpu, &mut mem)?),
        Answer::Swap => {
            crate::frame_wait::swapped();
            (Some(crate::frame_wait::swaps()), 0)
        }
        Answer::Gap => {
            let caller = mem.u32(cpu.esp).unwrap_or(0);
            unimplemented!("d3d entry {name} ({addr:#x}, {convention}): caller {caller:#x}");
        }
    };
    if let Some(eax) = eax {
        cpu.eax = eax;
    }
    mem.ret(cpu, cleanup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_is_listed_once() {
        let mut addrs: Vec<u32> = ENTRIES.iter().map(|e| e.0).collect();
        addrs.sort_unstable();
        addrs.dedup();
        assert_eq!(addrs.len(), 88);
    }

    fn named(name: &str) -> Answer {
        let e = ENTRIES.iter().find(|e| e.1 == name).unwrap_or_else(|| panic!("{name}"));
        answer(e.0)
    }

    #[test]
    fn every_setter_is_a_listed_entry() {
        for &(addr, _) in device_state::RENDER_STATE_SETTERS {
            assert!(ENTRIES.iter().any(|e| e.0 == addr), "{addr:#x}");
        }
    }

    #[test]
    fn each_kind_answers_its_entries() {
        let count = |f: fn(&Answer) -> bool| ENTRIES.iter().filter(|e| f(&answer(e.0))).count();
        // Every SetRenderState_* and SetTextureState_* entry stores.
        let setters = ENTRIES
            .iter()
            .filter(|e| e.1.starts_with("D3DDevice_SetRenderState_") || e.1.starts_with("D3DDevice_SetTextureState_"))
            .count();
        assert_eq!(setters, 35);
        assert_eq!(count(|a| matches!(a, Answer::State(_))), setters);
        assert_eq!(named("D3DDevice_SetRenderState_ZBias"), Answer::State(device_state::Setter::ZBias));
        assert_eq!(named("D3DDevice_SetRenderState_Simple"), Answer::State(device_state::Setter::Simple));
        assert_eq!(named("D3DDevice_Swap"), Answer::Swap);
        assert_eq!(named("CDevice_KickOff"), Answer::Return { eax: None, cleanup: 0 });
        assert_eq!(named("D3DDevice_PersistDisplay"), Answer::Return { eax: Some(0), cleanup: 0 });
        assert_eq!(named("D3D device teardown (0x3fc530)"), Answer::Return { eax: None, cleanup: 0 });
    }

    #[test]
    fn the_rest_are_named_gaps() {
        let gaps: Vec<&str> = ENTRIES.iter().filter(|e| answer(e.0) == Answer::Gap).map(|e| e.1).collect();
        assert_eq!(gaps.len(), 88 - 7 - 35 - 4 - 2, "{gaps:?}");
        for name in [
            "D3D_MakeRequestedSpace",
            "D3DDevice_Begin",
            "D3DDevice_End",
            "D3DDevice_Clear",
            "D3DDevice_SetTexture",
            "D3DDevice_SetPixelShader",
            "D3DDevice_DrawIndexedVertices",
        ] {
            assert!(gaps.contains(&name), "{name}");
        }
    }

    #[test]
    fn a_setter_returns_past_its_argument() {
        use runtime::guest::Permissions;
        let mut mem = runtime::Guest::new();
        mem.map_zeroed(0x0040_4000, 0x5000, Permissions::RW).unwrap();
        mem.map_zeroed(0x0001_0000, 0x1000, Permissions::RW).unwrap();
        let esp = 0x0001_0800;
        mem.set_u32(esp, 0x0001_2438).unwrap();
        mem.set_u32(esp + 4, 2).unwrap();
        let mut cpu = CpuAbi { esp, eax: 0x55, ..CpuAbi::default() };
        let Answer::State(s) = named("D3DDevice_SetRenderState_StencilEnable") else { panic!() };
        let cleanup = device_state::set(s, &cpu, &mut mem).unwrap();
        mem.ret(&mut cpu, cleanup).unwrap();
        assert_eq!(cpu.esp, esp + 8);
        assert_eq!(cpu.eax, 0x55);
        assert_eq!(mem.u32(device_state::render_state(144)).unwrap(), 2);
    }
}
