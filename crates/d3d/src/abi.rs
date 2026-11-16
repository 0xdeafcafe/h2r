//! TODO-register: LTCG calling-convention adapters.
//!
//! Halo's D3D was link-time code generated, so most entry points take
//! arguments in whatever registers the optimiser picked. The conventions
//! below were recovered per function from the XBE (call-site setup plus
//! register liveness at entry, see `docs/graphics/d3d.md`). Each adapter reads its
//! arguments from [`Regs`] and the guest stack, calls the [`Device`] method,
//! and returns the `EAX` (and `EDX` where relevant) result plus how many
//! stack bytes the callee pops.
//!
//! Nothing here is verified at run time against the original code yet: that
//! is what "TODO-register" marks. When the runtime's override path lands,
//! each entry should get a differential test (original function under the
//! interpreter vs. adapter) over the recorded call sites.
//!
//! Registration with the runtime is [`crate::register`] (in
//! [`crate::runtime_glue`]), built on [`handler`] and compiled against the
//! real `crates/runtime`; `tests/registration.rs` runs it through
//! `runtime::Runner`.

use crate::backend::Backend;
use crate::device::Device;
use crate::events::D3dError;
use crate::guest::GuestMemory;

/// Guest registers at entry (`esp` points at the return address).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    /// EAX.
    pub eax: u32,
    /// ECX.
    pub ecx: u32,
    /// EDX.
    pub edx: u32,
    /// EBX.
    pub ebx: u32,
    /// ESI.
    pub esi: u32,
    /// EDI.
    pub edi: u32,
    /// ESP.
    pub esp: u32,
}

/// What the adapter wants the runtime to do on return. Registers not named
/// are left as they were at entry (stricter than the originals, which
/// clobber caller-saved registers; LTCG callers sometimes rely on a register
/// surviving, e.g. `EDX` across `KickOff`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// New EAX.
    pub eax: Option<u32>,
    /// New EDX.
    pub edx: Option<u32>,
    /// Bytes of stack arguments the callee pops (`ret n`).
    pub cleanup: u32,
}

impl Outcome {
    fn pop(cleanup: u32) -> Self {
        Self {
            cleanup,
            ..Self::default()
        }
    }
    fn eax(eax: u32, cleanup: u32) -> Self {
        Self {
            eax: Some(eax),
            cleanup,
            ..Self::default()
        }
    }
}

/// Adapter signature.
pub type Adapter<B> = fn(&mut Device<B>, &mut dyn GuestMemory, &Regs) -> Result<Outcome, D3dError>;

/// One overridable entry point.
pub struct Entry<B: Backend> {
    /// Guest address of the function.
    pub addr: u32,
    /// XDK name (or the name assigned in `docs/graphics/d3d.md` for unmatched ones).
    pub name: &'static str,
    /// Recovered convention, human-readable.
    pub convention: &'static str,
    /// The adapter.
    pub call: Adapter<B>,
}

/// Stack argument `i` (0 = first, just above the return address).
fn arg(mem: &dyn GuestMemory, r: &Regs, i: u32) -> Result<u32, D3dError> {
    let addr = r.esp as u64 + 4 + 4 * i as u64;
    let addr = u32::try_from(addr).map_err(|_| D3dError::Invalid {
        entry: "stack argument",
        detail: format!("ESP {:#010x} + {} overflows", r.esp, 4 + 4 * i),
    })?;
    Ok(mem.read_u32(addr)?)
}

/// The first `N` stack arguments.
fn args<const N: usize>(mem: &dyn GuestMemory, r: &Regs) -> Result<[u32; N], D3dError> {
    let mut a = [0; N];
    for (i, v) in a.iter_mut().enumerate() {
        *v = arg(mem, r, i as u32)?;
    }
    Ok(a)
}

macro_rules! render_state_setters {
    ($($addr:literal $name:literal $slot:literal;)*) => {
        /// `SetRenderState_*` setters (stdcall `Value`, `ret 4`) and the guest
        /// render-state slot each stores (from the stores in its body).
        pub const RENDER_STATE_SETTERS: &[(u32, &str, u32)] = &[$(($addr, $name, $slot)),*];
    };
}

render_state_setters! {
    0x3f52f0 "D3DDevice_SetRenderState_EdgeAntiAlias" 151;
    0x3f5330 "D3DDevice_SetRenderState_ShadowFunc" 156;
    0x3f5370 "D3DDevice_SetRenderState_FogColor" 138;
    0x3f53d0 "D3DDevice_SetRenderState_CullMode" 147;
    0x3f5450 "D3DDevice_SetRenderState_FrontFace" 146;
    0x3f54a0 "D3DDevice_SetRenderState_NormalizeNormals" 142;
    0x3f54f0 "D3DDevice_SetRenderState_ZEnable" 143;
    0x3f5590 "D3DDevice_SetRenderState_StencilEnable" 144;
    0x3f5640 "D3DDevice_SetRenderState_StencilFail" 145;
    0x3f56c0 "D3DDevice_SetRenderState_TextureFactor" 148;
    0x3f5730 "D3DDevice_SetRenderState_LineWidth" 157;
    0x3f57a0 "D3DDevice_SetRenderState_Dxt1NoiseEnable" 159;
    0x3f5830 "D3DDevice_SetRenderState_YuvEnable" 160;
    0x3f5870 "D3DDevice_SetRenderState_OcclusionCullEnable" 161;
    0x3f58e0 "D3DDevice_SetRenderState_StencilCullEnable" 162;
    0x3f5950 "D3DDevice_SetRenderState_RopZCmpAlwaysRead" 163;
    0x3f5970 "D3DDevice_SetRenderState_RopZRead" 164;
    0x3f5990 "D3DDevice_SetRenderState_DoNotCullUncompressed" 165;
    0x3f59b0 "D3DDevice_SetRenderState_ZBias" 149;
    0x3f5a60 "D3DDevice_SetRenderState_MultiSampleMode" 154;
    0x3f5a90 "D3DDevice_SetRenderState_MultiSampleRenderTargetMode" 155;
    0x3f5ac0 "D3DDevice_SetRenderState_MultiSampleAntiAlias" 152;
    0x3f5b00 "D3DDevice_SetRenderState_MultiSampleMask" 153;
    0x3f5b20 "D3DDevice_SetRenderState_SampleAlpha" 158;
    0x3f5b40 "D3DDevice_SetRenderState_LogicOp" 150;
    0x3f5ba0 "D3DDevice_SetRenderState_FillMode" 139;
    0x3f5bf0 "D3DDevice_SetRenderState_BackFillMode" 140;
    0x3f5c10 "D3DDevice_SetRenderState_TwoSidedLighting" 141;
    0x3f5c70 "D3DDevice_SetRenderState_VertexBlend" 137;
    0x3f5cc0 "D3DDevice_SetRenderState_PSTextureModes" 136;
}

/// Entry points reached from game code that are not implemented: calling
/// one stops with `D3dError::Unimplemented`.
pub const UNIMPLEMENTED: &[(u32, &str, &str)] =
    &[(0x3f77c0, "D3DDevice_Reset", "EBX = pPresentationParameters")];

/// D3D-internal functions that execute port I/O: `in al, dx` / `out dx, al`
/// on the GPIO port `0x80C0` (the field/encoder bit) at `0x3fe131`,
/// `0x3feb3c` and `0x3fee0d`. No game code calls them. Their only ways in
/// are the original CreateDevice (`0x3fbc10`), the teardown and Reset paths
/// (`0x3fc530`, `0x3f77c0`), and the GPU interrupt service routine and DPC
/// that `0x3fe005` connects, all of which the overrides replace. They are
/// overridden anyway, so generated code never executes the port
/// instructions, even through a stray call.
///
/// * `0x3fe005`: the miniport hardware bring-up (from `0x3fbc8f` under
///   `0x3fbc10`): connects the interrupt (`KeInitializeInterrupt` with the
///   ISR `0x3feac0`, DPC `0x3febe0`), maps the NV2A and writes GPIO.
///   Stops: the device is created by [`crate::Device::create_device`].
/// * `0x3feac0`: the GPU interrupt service routine, stdcall `(Interrupt,
///   ServiceContext)`, `ret 8`, `AL` = handled. Implemented: the device
///   raises no interrupts, so it is never this device's interrupt (`AL` =
///   FALSE, the original's return when nothing is pending, `0x3febd0`).
/// * `0x3fed90`: the vertical-blank service (`EAX` = miniport), from the
///   DPC and the flip paths: reads the field bit, flips, calls the vblank
///   callback. Stops: vblanks come from [`crate::Device::vblank`] and
///   [`crate::vblank`].
pub const PORT_IO: &[(u32, &str, &str)] = &[
    (
        0x3fe005,
        "D3D miniport hardware init (0x3fe005)",
        "EAX = miniport; returns EAX = success",
    ),
    (
        0x3feac0,
        "D3D GPU interrupt service routine (0x3feac0)",
        "stdcall(Interrupt, ServiceContext); returns AL = handled; ret 8",
    ),
    (
        0x3fed90,
        "D3D miniport vertical-blank service (0x3fed90)",
        "EAX = miniport; returns EAX",
    ),
];

/// Overrides that always stop: [`UNIMPLEMENTED`] plus the port-I/O
/// functions in [`PORT_IO`] that have no meaning without the original
/// device.
pub fn stops(addr: u32) -> bool {
    UNIMPLEMENTED.iter().any(|u| u.0 == addr) || matches!(addr, 0x3fe005 | 0x3fed90)
}

/// D3D-section functions game code calls that are deliberately not
/// overridden, so the original code runs. Three kinds:
///
/// * helpers that only compute from their arguments and read-only D3D
///   tables (plus `XMemAlloc` and the overridden `D3DResource_AddRef`).
///   Their transitive callees were checked for device globals, NV2A
///   registers and push-buffer writes;
/// * `D3D_DestroyResource` (`0x3fb520`), which frees guest memory through
///   the game's allocators and calls only the overridden
///   `BlockOnResource`; the device does its own part of a destroy and the
///   runtime glue calls this for each resource it destroyed
///   ([`crate::Device::destroy_resource`]);
/// * push-buffer writers (`0x3fae10`, `0x401030`) that read only guest
///   state the device keeps current (the dirty flags, `D3D__Streams`, the
///   shader object at device `+0x794`), call only overridden D3D entries
///   (the commit `0x400fa0`, `MakeRequestedSpace` `0x3faae0`), and push only
///   methods the drain decodes, like any inline writer.
///
/// See `docs/graphics/d3d.md`.
pub const NATIVE: &[(u32, &str, &str)] = &[
    (
        0x3fb7a0,
        "D3D surface-object allocator (0x3fb7a0)",
        "EAX = data, EDI = parent, 2 stack args; XMemAlloc(0x18), AddRef(parent); ret 8",
    ),
    (
        0x3fdbc0,
        "D3D surface level layout (0x3fdbc0)",
        "stdcall 8 args; reads the header and the format table 0x403a50; ret 0x20",
    ),
    (
        0x3fdb20,
        "Get2DSurfaceDesc",
        "EDI = pixel container, EBX = level, ESI = D3DSURFACE_DESC out; reads the format table 0x403a50, device +0x1A14 (back buffer Data) and +0x196C (multisample type, written by CreateDevice); callees 0x3fb690, 0x3fd9c0 are pure",
    ),
    (
        0x3fae10,
        "D3D BeginPush (0x3fae10)",
        "ESI = dword count; SetStateVB(device, 0), then MakeRequestedSpace when put + 4 * count + 4 passes threshold + 0x200; returns EAX = put; ret",
    ),
    (
        0x401030,
        "CDevice_SetStateVB",
        "stdcall(device, base); commits through 0x400fa0 when other dirty bits are set, then pushes SET_VERTEX_DATA_ARRAY_FORMAT x16 (dirty bit 0x10) and one SET_VERTEX_DATA_ARRAY_OFFSET per enabled attribute; ret 8",
    ),
    (
        0x3fb520,
        "D3D_DestroyResource",
        "EDI = resource; ret; BlockOnResource (overridden) unless a title surface, unlink from 0x408644 (0x3fb4d0), XMemFree data (0x2d0a60) and header (0x122c0); called by the glue for every resource the device destroys",
    ),
    (
        0x3fde00,
        "D3D texture header setup (0x3fde00)",
        "stdcall 8 args; format table 0x403a50, 0x3fd770; ret 0x20",
    ),
];

fn set_render_state<B: Backend>(
    d: &mut Device<B>,
    m: &mut dyn GuestMemory,
    r: &Regs,
    addr: u32,
) -> Result<Outcome, D3dError> {
    let (_, _, slot) = RENDER_STATE_SETTERS
        .iter()
        .find(|e| e.0 == addr)
        .expect("registered setter");
    let v = arg(m, r, 0)?;
    d.set_render_state(m, *slot, v)?;
    Ok(Outcome::pop(4))
}

macro_rules! setter_adapters {
    ($($addr:literal => $fn:ident),* $(,)?) => {
        $(fn $fn<B: Backend>(d: &mut Device<B>, m: &mut dyn GuestMemory, r: &Regs) -> Result<Outcome, D3dError> {
            set_render_state(d, m, r, $addr)
        })*
        fn setter_entries<B: Backend>() -> Vec<Entry<B>> {
            let adapters: &[(u32, Adapter<B>)] = &[$(($addr, $fn::<B>)),*];
            adapters.iter().map(|&(addr, call)| {
                let name = RENDER_STATE_SETTERS.iter().find(|e| e.0 == addr).expect("setter").1;
                Entry { addr, name, convention: "stdcall(Value); ret 4", call }
            }).collect()
        }
    };
}

setter_adapters! {
    0x3f52f0 => rs_3f52f0, 0x3f5330 => rs_3f5330, 0x3f5370 => rs_3f5370, 0x3f53d0 => rs_3f53d0,
    0x3f5450 => rs_3f5450, 0x3f54a0 => rs_3f54a0, 0x3f54f0 => rs_3f54f0, 0x3f5590 => rs_3f5590,
    0x3f5640 => rs_3f5640, 0x3f56c0 => rs_3f56c0, 0x3f5730 => rs_3f5730, 0x3f57a0 => rs_3f57a0,
    0x3f5830 => rs_3f5830, 0x3f5870 => rs_3f5870, 0x3f58e0 => rs_3f58e0, 0x3f5950 => rs_3f5950,
    0x3f5970 => rs_3f5970, 0x3f5990 => rs_3f5990, 0x3f59b0 => rs_3f59b0, 0x3f5a60 => rs_3f5a60,
    0x3f5a90 => rs_3f5a90, 0x3f5ac0 => rs_3f5ac0, 0x3f5b00 => rs_3f5b00, 0x3f5b20 => rs_3f5b20,
    0x3f5b40 => rs_3f5b40, 0x3f5ba0 => rs_3f5ba0, 0x3f5bf0 => rs_3f5bf0, 0x3f5c10 => rs_3f5c10,
    0x3f5c70 => rs_3f5c70, 0x3f5cc0 => rs_3f5cc0,
}

/// Texture-stage slots of the dedicated `SetTextureState_*` setters.
mod tss {
    pub const TEXCOORDINDEX: u32 = 28;
    pub const BORDERCOLOR: u32 = 29;
    pub const COLORKEYCOLOR: u32 = 30;
}

fn port_io_stop<B: Backend>(
    _: &mut Device<B>,
    _: &mut dyn GuestMemory,
    r: &Regs,
) -> Result<Outcome, D3dError> {
    Err(D3dError::Unimplemented {
        entry: "D3D entry",
        detail: format!(
            "reached with EAX={:#x}: only the original device bring-up, interrupt and teardown paths call it, and the device replaces them (it would run GPIO port I/O on 0x80C0)",
            r.eax
        ),
    })
}

fn unimplemented<B: Backend>(
    _: &mut Device<B>,
    _: &mut dyn GuestMemory,
    r: &Regs,
) -> Result<Outcome, D3dError> {
    // The runtime passes the entry address in EIP-equivalent form; adapters
    // only see registers, so name the entry from the table at registration.
    Err(D3dError::Unimplemented {
        entry: "D3D entry",
        detail: format!(
            "called with EAX={:#x} ECX={:#x} EDX={:#x} ESP={:#x}",
            r.eax, r.ecx, r.edx, r.esp
        ),
    })
}

/// Every entry point with an adapter, implemented or not. Unimplemented ones
/// fail with their name via [`call`].
pub fn entries<B: Backend>() -> Vec<Entry<B>> {
    let mut v: Vec<Entry<B>> = vec![
        Entry {
            addr: 0x3f5240,
            name: "Direct3D_CreateDevice",
            convention: "EAX = BehaviorFlags, ECX = ppDevice, [esp+4] = pPresentationParameters; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                Ok(Outcome::eax(d.create_device(m, r.eax, a[0], r.ecx)?, 4))
            },
        },
        Entry {
            addr: 0x3fc530,
            name: "D3D device teardown (0x3fc530)",
            convention: "EAX = device; ret",
            call: |d, m, r| {
                d.release_device(m, r.eax)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f6e10,
            name: "D3DDevice_SetShaderConstantMode",
            convention: "EAX = mode; ret",
            call: |d, m, r| {
                d.set_shader_constant_mode(m, r.eax)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f5cf0,
            name: "D3DDevice_SetRenderState_Simple",
            convention: "ECX = NV2A method, EDX = value; ret",
            call: |d, m, r| {
                d.set_render_state_simple(m, r.ecx, r.edx)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f5d20,
            name: "D3DDevice_SetTextureState_TexCoordIndex",
            convention: "ESI = stage, [esp+4] = value; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                d.set_texture_stage_state(m, r.esi, tss::TEXCOORDINDEX, a[0])?;
                Ok(Outcome::pop(4))
            },
        },
        Entry {
            addr: 0x3f5e30,
            name: "D3DDevice_SetTextureState_BumpEnv",
            convention: "EAX = stage, stack (Type, Value); ret 8",
            call: |d, m, r| {
                let a = args::<2>(m, r)?;
                d.set_texture_stage_state(m, r.eax, a[0], a[1])?;
                Ok(Outcome::pop(8))
            },
        },
        Entry {
            addr: 0x3f5ea0,
            name: "D3DDevice_SetTextureState_BorderColor",
            convention: "EAX = stage, EBX = value; ret",
            call: |d, m, r| {
                d.set_texture_stage_state(m, r.eax, tss::BORDERCOLOR, r.ebx)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f5ef0,
            name: "D3DDevice_SetTextureState_ColorKeyColor",
            convention: "EAX = stage, EBX = value; ret",
            call: |d, m, r| {
                d.set_texture_stage_state(m, r.eax, tss::COLORKEYCOLOR, r.ebx)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f6c60,
            name: "D3DDevice_LoadVertexShaderProgram",
            convention: "stdcall(pFunction, Address); ret 8",
            call: |d, m, r| {
                let a = args::<2>(m, r)?;
                d.load_vertex_shader_program(m, a[0], a[1])?;
                Ok(Outcome::pop(8))
            },
        },
        Entry {
            addr: 0x3f6cf0,
            name: "D3DDevice_SelectVertexShaderDirect",
            convention: "EAX = Address, ECX = pVAF; ret",
            call: |d, m, r| {
                d.select_vertex_shader_direct(m, r.ecx, r.eax)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f71d0,
            name: "D3DDevice_SetVertexShaderConstant1",
            convention: "ECX = Register + 96, EDX = pConstantData; ret",
            call: |d, m, r| {
                d.set_vertex_shader_constant(m, r.ecx, r.edx, 1, true)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f7230,
            name: "D3DDevice_SetVertexShaderConstant1Fast",
            convention: "ECX = Register + 96, EDX = pConstantData; ret",
            call: |d, m, r| {
                d.set_vertex_shader_constant(m, r.ecx, r.edx, 1, false)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f7280,
            name: "D3DDevice_SetVertexShaderConstant4",
            convention: "ECX = Register + 96, EDX = pConstantData; ret",
            call: |d, m, r| {
                d.set_vertex_shader_constant(m, r.ecx, r.edx, 4, true)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f7330,
            name: "D3DDevice_SetVertexShaderConstantNotInlineFast",
            convention: "ECX = Register + 96, EDX = pConstantData, [esp+4] = DWORD count (pushed in batches of 16); ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                d.set_vertex_shader_constant_dwords(m, r.ecx, r.edx, a[0])?;
                Ok(Outcome::pop(4))
            },
        },
        Entry {
            addr: 0x3f7410,
            name: "D3DDevice_SetVertexShaderConstantNotInline",
            convention: "EBX = Register + 96, EDX = pConstantData, EAX = DWORD count; shadows at 0x407738 unless device flag 0x10, then NotInlineFast; ret",
            call: |d, m, r| {
                d.set_vertex_shader_constant_not_inline(m, r.ebx, r.edx, r.eax)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f7440,
            name: "D3DDevice_SetVertexShaderInput",
            convention: "stdcall(Handle, StreamCount, pStreamInputs); ret 0xC",
            call: |d, m, r| {
                let a = args::<3>(m, r)?;
                d.set_vertex_shader_input(m, a[0], a[1], a[2])?;
                Ok(Outcome::pop(0xC))
            },
        },
        Entry {
            addr: 0x3f7870,
            name: "D3DDevice_SetRenderTarget",
            convention: "stdcall(pRenderTarget, pNewZStencil); ret 8",
            call: |d, m, r| {
                let a = args::<2>(m, r)?;
                d.set_render_target(m, a[0], a[1])?;
                Ok(Outcome::eax(0, 8))
            },
        },
        Entry {
            addr: 0x3f7b30,
            name: "D3DDevice_SetRenderTarget (same size, 0x3f7b30)",
            convention: "[esp+4] = pRenderTarget, EBX = pNewZStencil; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                d.set_render_target_same_size(m, a[0], r.ebx)?;
                Ok(Outcome::pop(4))
            },
        },
        Entry {
            addr: 0x3f8100,
            name: "D3DDevice_SetViewport",
            convention: "stdcall(pViewport); ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                d.set_viewport(m, a[0])?;
                Ok(Outcome::eax(0, 4))
            },
        },
        Entry {
            addr: 0x3f8320,
            name: "D3DDevice_SetTexture",
            convention: "EAX = Stage, [esp+4] = pTexture; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                d.set_texture(m, r.eax, a[0])?;
                Ok(Outcome::pop(4))
            },
        },
        Entry {
            addr: 0x3f84d0,
            name: "D3DDevice_SetPalette",
            convention: "EAX = Stage, [esp+4] = pPalette; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                d.set_palette(m, r.eax, a[0])?;
                Ok(Outcome::pop(4))
            },
        },
        Entry {
            addr: 0x3f85d0,
            name: "D3DDevice_EndVisibilityTest",
            convention: "EAX = index; returns EAX = 0 (S_OK); ret",
            call: |d, m, r| {
                d.end_visibility_test(m, r.eax)?;
                Ok(Outcome::eax(0, 0))
            },
        },
        Entry {
            addr: 0x3f8630,
            name: "D3DDevice_GetVisibilityTestResult",
            convention: "EAX = index, EDX = ULONGLONG* timestamp (optional), [esp+4] = UINT* count; returns EAX = 0, or D3DERR_TESTINCOMPLETE writing nothing; ret 4",
            call: |d, m, r| {
                let out = arg(m, r, 0)?;
                let Some(a) = d.get_visibility_test_result(m, r.eax)? else {
                    return Ok(Outcome::eax(crate::device::D3DERR_TESTINCOMPLETE, 4));
                };
                // 0x3f866f..0x3f8683: the count first, then the timestamp
                // when EDX is non-null.
                m.write_u32(out, a.count)?;
                if r.edx != 0 {
                    m.write_u32(r.edx, a.timestamp as u32)?;
                    m.write_u32(r.edx.wrapping_add(4), (a.timestamp >> 32) as u32)?;
                }
                Ok(Outcome::eax(0, 4))
            },
        },
        Entry {
            addr: 0x3f8760,
            name: "D3DDevice_SetScissors",
            convention: "stdcall(Count, Exclusive, pRects); ret 0xC",
            call: |d, m, r| {
                let a = args::<3>(m, r)?;
                d.set_scissors(m, a[0], a[1], a[2])?;
                Ok(Outcome::eax(0, 0xC))
            },
        },
        Entry {
            addr: 0x3f9d00,
            name: "D3DDevice_Swap",
            convention: "EAX = Flags (0 = COPY|FINISH); returns EAX = swap count; ret",
            call: |d, m, r| Ok(Outcome::eax(d.swap(m, r.eax)?, 0)),
        },
        Entry {
            addr: 0x3f9df0,
            name: "D3DDevice_SetBackBufferScale",
            convention: "stdcall(x, y); ret 8",
            call: |d, m, r| {
                let a = args::<2>(m, r)?;
                d.set_back_buffer_scale(m, f32::from_bits(a[0]), f32::from_bits(a[1]))?;
                Ok(Outcome::pop(8))
            },
        },
        Entry {
            addr: 0x3f9fc0,
            name: "D3DDevice_Clear",
            convention: "stdcall(Count, pRects, Flags, Color, Z, Stencil); ret 0x18",
            call: |d, m, r| {
                let a: Vec<u32> = (0..6).map(|i| arg(m, r, i)).collect::<Result<_, _>>()?;
                d.clear(m, a[0], a[1], a[2], a[3], f32::from_bits(a[4]), a[5])?;
                Ok(Outcome::eax(0, 0x18))
            },
        },
        Entry {
            addr: 0x3fa3d0,
            name: "D3DDevice_SetPixelShader",
            convention: "EAX = Handle; ret",
            call: |d, m, r| {
                d.set_pixel_shader(m, r.eax)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3fa680,
            name: "D3D_SetFence",
            convention: "stdcall(Type); returns EAX = fence; ret 4",
            call: |d, m, _| Ok(Outcome::eax(d.set_fence(m)?, 4)),
        },
        Entry {
            addr: 0x3fa970,
            name: "D3D_BlockOnTime",
            convention: "2 stack args; ret 8 (no-op: the host GPU is synchronous)",
            call: |d, m, _| {
                d.block_on_time(m)?;
                Ok(Outcome::pop(8))
            },
        },
        Entry {
            addr: 0x3faae0,
            name: "D3D_MakeRequestedSpace",
            convention: "[esp+4] = MinimumSpace, EAX = RequestedSpace; returns EAX = put; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                let put = d.make_requested_space(m, a[0], r.eax)?;
                Ok(Outcome::eax(put, 4))
            },
        },
        Entry {
            addr: 0x3fac30,
            name: "CDevice_KickOff",
            convention: "no args; EDX live across the call; ret",
            call: |d, m, _| {
                d.kick_off(m)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3fad50,
            name: "D3D_BlockOnResource",
            convention: "EAX = pResource; ret (no-op: the host GPU is synchronous)",
            call: |d, m, r| {
                d.block_on_resource(m, r.eax)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3faed0,
            name: "D3DDevice_DrawVertices",
            convention: "EAX = VertexCount, ECX = StartVertex, [esp+4] = PrimitiveType; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                d.draw_vertices(m, a[0], r.ecx, r.eax)?;
                Ok(Outcome::pop(4))
            },
        },
        Entry {
            addr: 0x3fafa0,
            name: "D3DDevice_DrawIndexedVertices",
            convention: "stdcall(PrimitiveType, VertexCount, pIndexData); ret 0xC",
            call: |d, m, r| {
                let a = args::<3>(m, r)?;
                d.draw_indexed_vertices(m, a[0], a[1], a[2])?;
                Ok(Outcome::pop(0xC))
            },
        },
        Entry {
            addr: 0x3fb280,
            name: "D3DDevice_SetVertexData2f",
            convention: "stdcall(Register, a, b); ret 0xC",
            call: |d, m, r| {
                let a = args::<3>(m, r)?;
                d.set_vertex_data(
                    m,
                    a[0] as i32,
                    [f32::from_bits(a[1]), f32::from_bits(a[2]), 0.0, 1.0],
                )?;
                Ok(Outcome::pop(0xC))
            },
        },
        Entry {
            addr: 0x3fb2d0,
            name: "D3DDevice_SetVertexData4f",
            convention: "EDI = Register (-1 = position), stack (a, b, c, d); ret 0x10",
            call: |d, m, r| {
                let a = args::<4>(m, r)?;
                d.set_vertex_data(
                    m,
                    r.edi as i32,
                    [
                        f32::from_bits(a[0]),
                        f32::from_bits(a[1]),
                        f32::from_bits(a[2]),
                        f32::from_bits(a[3]),
                    ],
                )?;
                Ok(Outcome::pop(0x10))
            },
        },
        Entry {
            addr: 0x3fb330,
            name: "D3DDevice_SetVertexData2s",
            convention: "stdcall(Register, a, b); ret 0xC",
            call: |d, m, r| {
                let a = args::<3>(m, r)?;
                let s = |v: u32| v as u16 as i16 as f32;
                d.set_vertex_data(m, a[0] as i32, [s(a[1]), s(a[2]), 0.0, 1.0])?;
                Ok(Outcome::pop(0xC))
            },
        },
        Entry {
            addr: 0x3fb380,
            name: "D3DDevice_SetVertexData4ub",
            convention: "stdcall(Register, a, b, c, d); ret 0x14",
            call: |d, m, r| {
                let a = args::<5>(m, r)?;
                let u = |v: u32| (v & 0xFF) as f32 / 255.0;
                let v = [u(a[1]), u(a[2]), u(a[3]), u(a[4])];
                d.set_vertex_data(m, a[0] as i32, v)?;
                Ok(Outcome::pop(0x14))
            },
        },
        Entry {
            addr: 0x3fb3e0,
            name: "D3DDevice_SetVertexDataColor",
            convention: "stdcall(Register, D3DCOLOR); ret 8",
            call: |d, m, r| {
                let a = args::<2>(m, r)?;
                let c = a[1].to_le_bytes(); // B, G, R, A
                let u = |b: u8| b as f32 / 255.0;
                d.set_vertex_data(m, a[0] as i32, [u(c[2]), u(c[1]), u(c[0]), u(c[3])])?;
                Ok(Outcome::pop(8))
            },
        },
        Entry {
            addr: 0x3fb440,
            name: "D3DDevice_Begin",
            convention: "stdcall(PrimitiveType); ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                d.begin(m, a[0])?;
                Ok(Outcome::pop(4))
            },
        },
        Entry {
            addr: 0x3fb480,
            name: "D3DDevice_End",
            convention: "no args; ret",
            call: |d, m, _| {
                d.end(m)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3fb600,
            name: "D3DResource_AddRef",
            convention: "stdcall(this); returns EAX = new count; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                Ok(Outcome::eax(d.resource_add_ref(m, a[0])?, 4))
            },
        },
        Entry {
            addr: 0x3fb640,
            name: "D3DResource_Release",
            convention: "stdcall(this); returns EAX = new count; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                Ok(Outcome::eax(d.resource_release(m, a[0])?, 4))
            },
        },
        Entry {
            addr: 0x3f6190,
            name: "D3D viewport push fragment",
            convention: "EAX = put, ESI = device; returns EAX = new put; ret",
            call: |d, m, r| Ok(Outcome::eax(d.push_viewport(m, r.eax, r.esi)?, 0)),
        },
        Entry {
            addr: 0x3f6390,
            name: "D3D push fragment (0x3f6390)",
            convention: "EDX = put, [esp+4] = device; returns EAX = put + 8; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                Ok(Outcome::eax(d.push_control0(m, r.edx, a[0])?, 4))
            },
        },
        Entry {
            addr: 0x3f6840,
            name: "D3D_UpdateProjectionViewportTransform",
            convention: "no args; ret",
            call: |d, m, _| {
                d.update_projection_viewport(m)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f6f40,
            name: "D3D transform/viewport push (0x3f6f40)",
            convention: "ESI = device; returns EAX = new put when it pushes, else the shader object [ESI+0x794]; ret",
            call: |d, m, r| {
                Ok(match d.load_passthrough(m, r.esi)? {
                    Some(put) => Outcome::eax(put, 0),
                    // 0x3f6f40: `mov eax,[esi+794h]`, then `je 0x3f705c` (ret).
                    None => Outcome::eax(
                        m.read_u32(
                            r.esi
                                .wrapping_add(crate::layout::device_field::VERTEX_SHADER),
                        )?,
                        0,
                    ),
                })
            },
        },
        Entry {
            addr: 0x3f8ad0,
            name: "D3DDevice_PersistDisplay",
            convention: "no args; returns EAX = HRESULT; ret",
            call: |d, m, _| Ok(Outcome::eax(d.persist_display(m)?, 0)),
        },
        Entry {
            addr: 0x3f8c70,
            name: "D3D display/flicker filter (0x3f8c70)",
            convention: "ESI = filter; ret",
            call: |d, m, r| {
                d.set_flicker_filter(m, r.esi)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x3f8cc0,
            name: "D3DDevice_SetStipple",
            convention: "stdcall(pPattern): 32 dwords to device +0x197C and method 0x1480; ret 4",
            call: |d, m, r| {
                let a = args::<1>(m, r)?;
                d.set_stipple(m, a[0])?;
                Ok(Outcome::pop(4))
            },
        },
        Entry {
            addr: 0x3fb710,
            name: "D3DResource_IsBusy (0x3fb710)",
            convention: "ECX = resource; returns EAX = busy (0/1); ret",
            call: |d, m, r| Ok(Outcome::eax(d.resource_busy(m, r.ecx)?, 0)),
        },
        Entry {
            addr: 0x3ff5c6,
            name: "D3D miniport gamma ramp (0x3ff5c6)",
            convention: "EAX = miniport (device + 0x1C28), EDX = D3DGAMMARAMP (768 bytes); ret",
            call: |d, m, r| {
                d.load_gamma_ramp(m, r.eax, r.edx)?;
                Ok(Outcome::pop(0))
            },
        },
        Entry {
            addr: 0x400fa0,
            name: "D3D commit deferred state (0x400fa0)",
            convention: "no args; ret",
            call: |d, m, _| {
                d.commit(m)?;
                Ok(Outcome::pop(0))
            },
        },
    ];
    v.extend(setter_entries::<B>());
    for &(addr, name, convention) in UNIMPLEMENTED {
        v.push(Entry {
            addr,
            name,
            convention,
            call: unimplemented::<B>,
        });
    }
    for &(addr, name, convention) in PORT_IO {
        v.push(Entry {
            addr,
            name,
            convention,
            call: if stops(addr) {
                port_io_stop::<B>
            } else {
                // 0x3feac0: not this device's interrupt (AL = FALSE).
                |_, _, r| Ok(Outcome::eax(r.eax & !0xFF, 8))
            },
        });
    }
    v.sort_by_key(|e| e.addr);
    v
}

impl<B: Backend> Clone for Entry<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: Backend> Copy for Entry<B> {}

/// A shared device, as the runtime's override handlers hold it. `Device<B>`
/// is `Send` whenever the backend is (`GfxBackend` is), so the mutex is
/// `Send + Sync`.
pub type SharedDevice<B> = std::sync::Arc<std::sync::Mutex<Device<B>>>;

/// The adapter for `entry` bound to a shared device: a `Send + Sync +
/// 'static` closure, as `Dispatch::register_override` requires. The runtime
/// wraps it, copying `CpuAbi` into [`Regs`] and back (see the module docs).
/// A poisoned mutex (an earlier panic) is reported as a fault, not a panic.
pub fn handler<B>(
    device: SharedDevice<B>,
    entry: Entry<B>,
) -> impl Fn(&Regs, &mut dyn GuestMemory) -> Result<Outcome, D3dError> + Send + Sync + 'static
where
    B: Backend + Send + 'static,
{
    move |regs, mem| {
        let mut d = device.lock().map_err(|_| {
            D3dError::Faulted("D3D device mutex poisoned by an earlier panic".into())
        })?;
        call(&entry, &mut d, mem, regs)
    }
}

/// Calls `e`, naming the entry in an `Unimplemented` error.
pub fn call<B: Backend>(
    e: &Entry<B>,
    device: &mut Device<B>,
    mem: &mut dyn GuestMemory,
    regs: &Regs,
) -> Result<Outcome, D3dError> {
    match (e.call)(device, mem, regs) {
        Err(D3dError::Unimplemented {
            entry: "D3D entry",
            detail,
        }) => Err(D3dError::Unimplemented {
            entry: e.name,
            detail: format!("not implemented ({}); {detail}", e.convention),
        }),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::RecordingBackend;

    /// D3D-section functions called or jumped to from outside the section.
    const REACHED: &[u32] = &[
        0x3f5240, 0x3f52f0, 0x3f5330, 0x3f5370, 0x3f53d0, 0x3f5450, 0x3f54a0, 0x3f54f0, 0x3f5590,
        0x3f5640, 0x3f56c0, 0x3f5730, 0x3f57a0, 0x3f5830, 0x3f5870, 0x3f58e0, 0x3f5950, 0x3f5970,
        0x3f5990, 0x3f59b0, 0x3f5a60, 0x3f5a90, 0x3f5ac0, 0x3f5b00, 0x3f5b20, 0x3f5b40, 0x3f5ba0,
        0x3f5bf0, 0x3f5c10, 0x3f5c70, 0x3f5cf0, 0x3f5d20, 0x3f5e30, 0x3f5ea0, 0x3f5ef0, 0x3f6190,
        0x3f6390, 0x3f6840, 0x3f6c60, 0x3f6cf0, 0x3f6e10, 0x3f6f40, 0x3f71d0, 0x3f7230, 0x3f7280,
        0x3f7330, 0x3f7410, 0x3f7440, 0x3f77c0, 0x3f7870, 0x3f7b30, 0x3f8100, 0x3f8320, 0x3f84d0,
        0x3f85d0, 0x3f8630, 0x3f8760, 0x3f8ad0, 0x3f8c70, 0x3f8cc0, 0x3f9d00, 0x3f9df0, 0x3f9fc0,
        0x3fa3d0, 0x3fa680, 0x3fa970, 0x3faae0, 0x3fac30, 0x3fad50, 0x3fae10, 0x3faed0, 0x3fafa0,
        0x3fb280, 0x3fb2d0, 0x3fb330, 0x3fb380, 0x3fb3e0, 0x3fb440, 0x3fb480, 0x3fb520, 0x3fb600,
        0x3fb640, 0x3fb710, 0x3fb7a0, 0x3fc530, 0x3fdb20, 0x3fdbc0, 0x3fde00, 0x3ff5c6, 0x400fa0,
        0x401030,
    ];

    #[test]
    fn entry_table_is_unique_and_named() {
        let v = entries::<RecordingBackend>();
        for w in v.windows(2) {
            assert_ne!(w[0].addr, w[1].addr, "{} and {}", w[0].name, w[1].name);
        }
        // Every D3D-section function reached from game code has an entry or
        // is deliberately left native (plus SetRenderState_PSTextureModes,
        // matched but not called, and the port-I/O guards).
        for a in REACHED {
            let entry = v.iter().any(|e| e.addr == *a);
            let native = NATIVE.iter().any(|n| n.0 == *a);
            assert!(entry != native, "{a:#x}: entry {entry}, native {native}");
        }
        for p in PORT_IO {
            assert!(!REACHED.contains(&p.0) && v.iter().any(|e| e.addr == p.0));
        }
        assert_eq!(v.len() + NATIVE.len(), REACHED.len() + 1 + PORT_IO.len());
    }

    #[test]
    fn unimplemented_entries_name_themselves() {
        let v = entries::<RecordingBackend>();
        let e = v.iter().find(|e| e.addr == 0x3f77c0).unwrap();
        let mut d = Device::new(
            crate::device::DeviceConfig::halo2(
                crate::device::GuestRange {
                    addr: 0x0100_0000,
                    len: 0x10_0000,
                },
                crate::device::GuestRange {
                    addr: 0x0200_0000,
                    len: 0x1000,
                },
            ),
            RecordingBackend::new(),
        );
        let mut m = crate::guest::SparseMemory::new();
        let err = call(e, &mut d, &mut m, &Regs::default())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("D3DDevice_Reset") && err.contains("unimplemented"),
            "{err}"
        );
    }
}
