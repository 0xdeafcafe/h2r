# D3D: the guest-facing Direct3D 8 replacement

`crates/d3d` replaces the Direct3D 8 that Halo 2 links statically (XDK 5849,
`D3D8LTCG`, the `D3D` section) at the API level, on top of `crates/gfx`. It
does not emulate the NV2A. This page covers what the game reaches, which
guest structures matter, what game code does *without* calling D3D, the
device model, and how to wire it into the runtime.

Static analysis only, from `h2 analyze` and `h2 symbols --list --no-xcheck`
over the retail XBE (identity in [engine-map.md](../engine/engine-map.md)). Matches
from XbSymbolDatabase identify a function; Cxbx's LTCG patch list was used
for cross-reference only. Conventions are read off call-site setup plus
register use at entry. No executable bytes or disassembly listings are
included here.

## Summary

- The `D3D` section holds **237** functions. Signatures match **79** of them
  (84 matches; one address carries four shared `SetRenderState_*`
  signatures).
- **91** D3D functions are reached from outside the section. **26** of those
  have no signature match and are named below from evidence: which state
  slot they store, what they push, and who calls them.
- Of those 91, **83** are implemented (adapter plus device method), **1**
  stops with `D3dError::Unimplemented` (which carries the entry name and
  its convention), and **7** are left to the original code
  (`d3d::abi::NATIVE`): four pure helpers, `D3D_DestroyResource` (run by
  the runtime glue after the device's part, see "Resource destruction")
  and two push-buffer writers.
- `d3d::register` installs **88** overrides in a `runtime::Dispatch`
  (see "Integration"): **85** implemented and **3** stopping. Beyond the
  census:
  - `SetRenderState_PSTextureModes` (`0x3f5cc0`, implemented). It is
    matched by signature and has an adapter, but no code outside the
    `D3D` section calls it (game code writes the `PSTEXTUREMODES` slot and
    device `+0x790` itself).
  - The three D3D-internal functions that execute GPIO port I/O
    (`d3d::abi::PORT_IO`, see "Port I/O guards"): one implemented, two
    stopping. No game code calls them.
- Game code also does a lot of D3D's work inline, which no override can
  intercept:
  - it stores into the render-state and texture-state arrays (386 and 432
    `.text` accesses);
  - it writes NV2A methods into the push buffer (199 `MakeRequestedSpace`
    calls from 53 functions; 36 inline `BEGIN_END` stores in 22 functions);
  - it fills in device fields.

  The device is designed around that (see the device model below).

## Inventory: entries reached from game code

"Calls / caller fns" counts direct `call`/`jmp` instructions from outside
the `D3D` section and the distinct functions they sit in. "Matched" says
whether a signature identified the function. Unmatched ones are named by
what they do; names with an address in them are descriptive only. Status is
"yes" (implemented), "stop" (fails with `Unimplemented`) or "native" (not
overridden; the original runs).

| address | entry | matched | convention (as compiled) | calls / caller fns | status |
|---|---|---|---|---|---|
| `0x3f5240` | `Direct3D_CreateDevice` | yes | EAX = BehaviorFlags, ECX = ppDevice, [esp+4] = pPresentationParameters; ret 4 | 2 / 2 | yes |
| `0x3f52f0` | `D3DDevice_SetRenderState_EdgeAntiAlias` | yes | stdcall(Value); ret 4; guest slot 151 | 9 / 4 | yes |
| `0x3f5330` | `D3DDevice_SetRenderState_ShadowFunc` | yes | stdcall(Value); ret 4; guest slot 156 | 11 / 6 | yes |
| `0x3f5370` | `D3DDevice_SetRenderState_FogColor` | yes | stdcall(Value); ret 4; guest slot 138 | 11 / 6 | yes |
| `0x3f53d0` | `D3DDevice_SetRenderState_CullMode` | yes | stdcall(Value); ret 4; guest slot 147 | 58 / 48 | yes |
| `0x3f5450` | `D3DDevice_SetRenderState_FrontFace` | yes | stdcall(Value); ret 4; guest slot 146 | 10 / 5 | yes |
| `0x3f54a0` | `D3DDevice_SetRenderState_NormalizeNormals` | yes | stdcall(Value); ret 4; guest slot 142 | 9 / 4 | yes |
| `0x3f54f0` | `D3DDevice_SetRenderState_ZEnable` | yes | stdcall(Value); ret 4; guest slot 143 | 30 / 24 | yes |
| `0x3f5590` | `D3DDevice_SetRenderState_StencilEnable` | no | stdcall(Value); ret 4; guest slot 144 | 40 / 30 | yes |
| `0x3f5640` | `D3DDevice_SetRenderState_StencilFail` | no | stdcall(Value); ret 4; guest slot 145 | 15 / 6 | yes |
| `0x3f56c0` | `D3DDevice_SetRenderState_TextureFactor` | yes | stdcall(Value); ret 4; guest slot 148 | 9 / 4 | yes |
| `0x3f5730` | `D3DDevice_SetRenderState_LineWidth` | yes | stdcall(Value); ret 4; guest slot 157 | 12 / 7 | yes |
| `0x3f57a0` | `D3DDevice_SetRenderState_Dxt1NoiseEnable` | yes | stdcall(Value); ret 4; guest slot 159 | 10 / 5 | yes |
| `0x3f5830` | `D3DDevice_SetRenderState_YuvEnable` | no | stdcall(Value); ret 4; guest slot 160 | 9 / 4 | yes |
| `0x3f5870` | `D3DDevice_SetRenderState_OcclusionCullEnable` | no | stdcall(Value); ret 4; guest slot 161 | 9 / 4 | yes |
| `0x3f58e0` | `D3DDevice_SetRenderState_StencilCullEnable` | no | stdcall(Value); ret 4; guest slot 162 | 9 / 4 | yes |
| `0x3f5950` | `D3DDevice_SetRenderState_RopZCmpAlwaysRead` | no | stdcall(Value); ret 4; guest slot 163 | 9 / 4 | yes |
| `0x3f5970` | `D3DDevice_SetRenderState_RopZRead` | no | stdcall(Value); ret 4; guest slot 164 | 9 / 4 | yes |
| `0x3f5990` | `D3DDevice_SetRenderState_DoNotCullUncompressed` | no | stdcall(Value); ret 4; guest slot 165 | 5 / 4 | yes |
| `0x3f59b0` | `D3DDevice_SetRenderState_ZBias` | yes | stdcall(Value); ret 4; guest slot 149, derives 77–81 | 59 / 44 | yes |
| `0x3f5a60` | `D3DDevice_SetRenderState_MultiSampleMode` | yes | stdcall(Value); ret 4; guest slot 154 | 11 / 6 | yes |
| `0x3f5a90` | `D3DDevice_SetRenderState_MultiSampleRenderTargetMode` | yes | stdcall(Value); ret 4; guest slot 155 | 9 / 4 | yes |
| `0x3f5ac0` | `D3DDevice_SetRenderState_MultiSampleAntiAlias` | yes | stdcall(Value); ret 4; guest slot 152 | 9 / 4 | yes |
| `0x3f5b00` | `D3DDevice_SetRenderState_MultiSampleMask` | no | stdcall(Value); ret 4; guest slot 153 | 9 / 4 | yes |
| `0x3f5b20` | `D3DDevice_SetRenderState_SampleAlpha` | no | stdcall(Value); ret 4; guest slot 158 | 18 / 11 | yes |
| `0x3f5b40` | `D3DDevice_SetRenderState_LogicOp` | yes | stdcall(Value); ret 4; guest slot 150 | 10 / 5 | yes |
| `0x3f5ba0` | `D3DDevice_SetRenderState_FillMode` | no | stdcall(Value); ret 4; guest slot 139 | 13 / 6 | yes |
| `0x3f5bf0` | `D3DDevice_SetRenderState_BackFillMode` | yes | stdcall(Value); ret 4; guest slot 140 | 7 / 3 | yes |
| `0x3f5c10` | `D3DDevice_SetRenderState_TwoSidedLighting` | yes | stdcall(Value); ret 4; guest slot 141 | 11 / 5 | yes |
| `0x3f5c70` | `D3DDevice_SetRenderState_VertexBlend` | yes | stdcall(Value); ret 4; guest slot 137 | 9 / 4 | yes |
| `0x3f5cf0` | `D3DDevice_SetRenderState_Simple` | no | ECX = push header (`1 << 18 \| method`), EDX = value; ret | 291 / 62 | yes |
| `0x3f5d20` | `D3DDevice_SetTextureState_TexCoordIndex` | no | ESI = stage, [esp+4] = value; ret 4 | 11 / 4 | yes |
| `0x3f5e30` | `D3DDevice_SetTextureState_BumpEnv` | yes | EAX = stage, stack (Type, Value); ret 8 | 15 / 4 | yes |
| `0x3f5ea0` | `D3DDevice_SetTextureState_BorderColor` | yes | EAX = stage, EBX = value; ret | 10 / 5 | yes |
| `0x3f5ef0` | `D3DDevice_SetTextureState_ColorKeyColor` | yes | EAX = stage, EBX = value; ret | 8 / 4 | yes |
| `0x3f6190` | `D3D viewport push fragment` | no | EAX = put, ESI = device; returns EAX = new put | 22 / 22 | yes |
| `0x3f6390` | `D3D push fragment (0x3f6390)` | no | EDX = put, [esp+4] = device; returns EAX = put + 8; ret 4 | 18 / 18 | yes |
| `0x3f6840` | `D3D_UpdateProjectionViewportTransform` | yes | no args | 18 / 18 | yes |
| `0x3f6c60` | `D3DDevice_LoadVertexShaderProgram` | yes | stdcall(pFunction, Address); ret 8 | 1 / 1 | yes |
| `0x3f6cf0` | `D3DDevice_SelectVertexShaderDirect` | yes | EAX = Address, ECX = pVAF; ret | 1 / 1 | yes |
| `0x3f6e10` | `D3DDevice_SetShaderConstantMode` | yes | EAX = mode; ret | 1 / 1 | yes (mode 0 stops) |
| `0x3f6f40` | `D3D transform/viewport push (0x3f6f40)` | no | ESI = device; returns EAX = new put when it pushes, else the shader object `[ESI+0x794]` | 18 / 18 | yes |
| `0x3f71d0` | `D3DDevice_SetVertexShaderConstant1` | yes | ECX = Register + 96, EDX = pConstantData; ret | 12 / 5 | yes |
| `0x3f7230` | `D3DDevice_SetVertexShaderConstant1Fast` | yes | ECX = Register + 96, EDX = pConstantData; ret | 6 / 6 | yes |
| `0x3f7280` | `D3DDevice_SetVertexShaderConstant4` | yes | ECX = Register + 96, EDX = pConstantData; ret | 2 / 2 | yes |
| `0x3f7330` | `D3DDevice_SetVertexShaderConstantNotInlineFast` | yes | ECX = Register + 96, EDX = pConstantData, [esp+4] = DWORD count (pushed in batches of 16, may end mid-register); ret 4 | 30 / 24 | yes |
| `0x3f7410` | `D3DDevice_SetVertexShaderConstantNotInline` | yes | EBX = Register + 96, EDX = pConstantData, EAX = DWORD count; ret | 1 / 1 | yes |
| `0x3f7440` | `D3DDevice_SetVertexShaderInput` | yes | stdcall(Handle, StreamCount, pStreamInputs); ret 0xC | 2 / 1 | yes |
| `0x3f77c0` | `D3DDevice_Reset` | yes | EBX = pPresentationParameters | 1 / 1 | stop |
| `0x3f7870` | `D3DDevice_SetRenderTarget` | yes | stdcall(pRenderTarget, pNewZStencil); ret 8 | 1 / 1 | yes |
| `0x3f7b30` | `D3DDevice_SetRenderTarget (same size, 0x3f7b30)` | no | [esp+4] = target, EBX = depth/stencil; ret 4 | 1 / 1 | yes |
| `0x3f8100` | `D3DDevice_SetViewport` | yes | stdcall(pViewport); ret 4 | 3 / 2 | yes |
| `0x3f8320` | `D3DDevice_SetTexture` | yes | EAX = Stage, [esp+4] = pTexture; ret 4 | 2 / 2 | yes |
| `0x3f84d0` | `D3DDevice_SetPalette` | yes | EAX = Stage, [esp+4] = pPalette; ret 4 | 7 / 4 | yes |
| `0x3f85d0` | `D3DDevice_EndVisibilityTest` | yes | EAX = index; returns EAX = 0; ret | 1 / 1 | yes |
| `0x3f8630` | `D3DDevice_GetVisibilityTestResult` | no | EAX = index, EDX = ULONGLONG* timestamp (optional), [esp+4] = UINT* count; returns EAX = 0 or `D3DERR_TESTINCOMPLETE`; ret 4 | 2 / 2 | yes (incomplete only within two `Swap`s; see "Visibility tests") |
| `0x3f8760` | `D3DDevice_SetScissors` | yes | stdcall(Count, Exclusive, pRects); ret 0xC | 3 / 3 | yes |
| `0x3f8ad0` | `D3DDevice_PersistDisplay` | yes | no args; returns EAX = HRESULT | 1 / 1 | yes |
| `0x3f8c70` | `D3D display/flicker filter (0x3f8c70)` | no | ESI = filter | 1 / 1 | yes |
| `0x3f8cc0` | `D3DDevice_SetStipple` | no | stdcall(pPattern), copies 32 dwords to device+0x197C, pushes method 0x1480; ret 4 | 9 / 9 | yes |
| `0x3f9d00` | `D3DDevice_Swap` | yes | EAX = Flags (0 = COPY \| FINISH); returns EAX = swap count; ret | 2 / 2 | yes |
| `0x3f9df0` | `D3DDevice_SetBackBufferScale` | yes | stdcall(x, y); ret 8 | 2 / 1 | yes |
| `0x3f9fc0` | `D3DDevice_Clear` | yes | stdcall(Count, pRects, Flags, Color, Z, Stencil); ret 0x18 | 7 / 7 | yes |
| `0x3fa3d0` | `D3DDevice_SetPixelShader` | yes | EAX = Handle; ret | 29 / 25 | yes |
| `0x3fa680` | `D3D_SetFence` | yes | stdcall(Type); returns EAX = fence; ret 4 | 9 / 9 | yes |
| `0x3fa970` | `D3D_BlockOnTime` | yes | 2 stack args; ret 8 (no-op: the host GPU is synchronous) | 1 / 1 | yes |
| `0x3faae0` | `D3D_MakeRequestedSpace` | yes | [esp+4] = MinimumSpace, EAX = RequestedSpace; returns EAX = put; ret 4 | 199 / 53 | yes |
| `0x3fac30` | `CDevice_KickOff` | yes | no args; EDX live across the call; ret | 17 / 16 | yes |
| `0x3fad50` | `D3D_BlockOnResource` | yes | EAX = pResource; ret (no-op: the host GPU is synchronous) | 10 / 8 | yes (also after a teardown, for what it destroyed) |
| `0x3fae10` | `D3D BeginPush (0x3fae10)` | no | ESI = dword count; returns EAX = put; ret | 1 / 1 | native |
| `0x3faed0` | `D3DDevice_DrawVertices` | yes | EAX = VertexCount, ECX = StartVertex, [esp+4] = PrimitiveType; ret 4 | 3 / 3 | yes |
| `0x3fafa0` | `D3DDevice_DrawIndexedVertices` | yes | stdcall(PrimitiveType, VertexCount, pIndexData); ret 0xC | 8 / 5 | yes |
| `0x3fb280` | `D3DDevice_SetVertexData2f` | yes | stdcall(Register, a, b); ret 0xC | 39 / 6 | yes |
| `0x3fb2d0` | `D3DDevice_SetVertexData4f` | yes | EDI = Register (-1 = position), stack (a, b, c, d); ret 0x10 | 44 / 10 | yes |
| `0x3fb330` | `D3DDevice_SetVertexData2s` | yes | stdcall(Register, a, b); ret 0xC | 19 / 4 | yes |
| `0x3fb380` | `D3DDevice_SetVertexData4ub` | yes | stdcall(Register, a, b, c, d); ret 0x14 | 1 / 1 | yes |
| `0x3fb3e0` | `D3DDevice_SetVertexDataColor` | yes | stdcall(Register, D3DCOLOR); ret 8 | 10 / 5 | yes |
| `0x3fb440` | `D3DDevice_Begin` | yes | stdcall(PrimitiveType); ret 4 | 11 / 9 | yes |
| `0x3fb480` | `D3DDevice_End` | yes | no args; ret | 21 / 17 | yes |
| `0x3fb520` | `D3D_DestroyResource` | yes | EDI = resource; ret | 1 / 1 | native (after the device's part) |
| `0x3fb600` | `D3DResource_AddRef` | yes | stdcall(this); returns EAX = new count; ret 4 | 6 / 3 | yes |
| `0x3fb640` | `D3DResource_Release` | yes | stdcall(this); returns EAX = new count; ret 4 | 14 / 7 | yes |
| `0x3fb710` | `D3DResource_IsBusy (0x3fb710)` | no | ECX = resource; returns EAX = busy (0/1) | 7 / 5 | yes |
| `0x3fb7a0` | `D3D surface-object allocator (0x3fb7a0)` | no | EAX = data, EDI = parent, 2 stack args; ret 8 | 2 / 1 | native |
| `0x3fc530` | `D3D device teardown (0x3fc530)` | no | EAX = device; ret | 2 / 2 | yes |
| `0x3fdb20` | `Get2DSurfaceDesc` | yes | EDI = container, EBX = level, ESI = out desc | 4 / 4 | native |
| `0x3fdbc0` | `D3D surface level layout (0x3fdbc0)` | no | stdcall 8 args; ret 0x20 | 12 / 9 | native |
| `0x3fde00` | `D3D texture header setup (0x3fde00)` | no | stdcall 8 args; ret 0x20 | 3 / 3 | native |
| `0x3ff5c6` | `D3D miniport gamma ramp (0x3ff5c6)` | no | EAX = miniport (device + 0x1C28), EDX = D3DGAMMARAMP (768 bytes); ret | 1 / 1 | yes (applied on present) |
| `0x400fa0` | `D3D commit deferred state (0x400fa0)` | no | no args; ret | 23 / 19 | yes |
| `0x401030` | `CDevice_SetStateVB` | yes | stdcall(device, base); ret 8 | 1 / 1 | native |

Notes on the table:

- **Render-state setters.** The guest slot listed is the
  `D3D__RenderState` index that the function body stores.
  - In 5849 the guest index equals the canonical (Cxbx `X_D3DRS_*`) index
    below 154, and is one lower from 154 up. The setters' stores confirm
    this.
  - The unmatched setters were named from their slot.
  - `ZBias` also derives the polygon-offset slots, as compiled:
    `ZSLOPESCALE` (77) = `-bias / 4`, `ZOFFSET` (78) = `-bias` (the bias
    converted as an unsigned integer), the point/wireframe/solid offset
    enables (79–81) = `bias != 0`, and `ZBIAS` (149) = `bias`. The device
    stores all six; translation reads 77, 78 and 81.
- **`0x3f5ac0`** matches four `SetRenderState_*` signatures. It stores slot
  152, so it is `MultiSampleAntiAlias`.
- **`SetRenderState_Simple`** (`0x3f5cf0`) is the most-called entry: 291
  calls in 62 functions. It writes `ECX` (a push header, `1 << 18 |
  method`) and `EDX` into the push buffer and nothing else
  (`0x3f5d05`/`0x3f5d08`). Game code stores the array slot either before or
  after the call: 191 sites store first, 285 store after (for example
  `0x1268b` then `0x1269a`, `0x1eb21` then `0x1eb2d`). The header is
  D3D's `D3DSIMPLERENDERSTATEENCODE[slot]`, a 92-entry table at
  `0x0043E6F0` (slots 0–56 combiner registers, 57–83 simple states; the
  inlined generic setter at `0x1828e` indexes it). The table is not copied
  into the crate: `Direct3D_CreateDevice` reads it from the loaded image
  (`GuestLayout::simple_encode`) into `d3d::pushbuf::SimpleEncode`, and
  stops with a named error if it is unreadable or malformed (an entry
  that is not a one-word header for a 4-aligned method below `0x2000`,
  `0x030C` in the table, or every entry the same). Unit tests use a small
  hand-built table; the ignored `tests/xbe_tables.rs`
  (`H2_XBE=/abs/path/default.xbe cargo test -p d3d --test xbe_tables --
  --ignored`) checks the real one against what the crate assumes.
- **Not reached from game code, but implemented on the device** for the API
  surface: `SetVertexShader`, `SetStreamSource`, `DrawVerticesUP`,
  and `SetTransform`. `SetIndices` is not implemented and is not needed.
  - Halo binds vertices only through `SetVertexShaderInput`. Its
    vertex-format choke point (`0x0001c710`) builds a `D3DSTREAM_INPUT`
    array and uses D3D's static shader object at `0x00408518`.
  - Indices are a pointer argument to `DrawIndexedVertices`.
- **Vertex-shader constants.** `Constant1` and `Constant4` also update the
  guest shadow at `0x00407738`; `Constant1Fast` and `NotInlineFast` only
  push. `NotInlineFast` takes a DWORD count, not a register count.
  `NotInline` (`0x3f7410`, one caller at `0x40d01`: 20 dwords for register
  81) copies the dwords into the shadow at `0x407738 + 16 * EBX` unless
  device flag `0x10` is set (`0x3f7410..0x3f742a`), then tail-calls
  `NotInlineFast`.
- **`SetPixelShader`**, as compiled, stores the handle at device `+0x784`,
  `PSDEF[8] | PSDEF[9]` at `+0x788`, `PSDEF[59] & 0x100` at `+0x78C` and
  `PSDEF[54]` (PSTextureModes) at `+0x790`. Unless device flag `0x10` is
  set (`BehaviorFlags & 0x10`; Halo passes `0x40`, so it is clear), it then
  copies the first 57 PSDEF dwords into render-state slots 0–56 and
  `PSDEF[54]` into slot 136. The device does the same.
- **`MakeRequestedSpace`** takes the request in `EAX` and half the kick-off
  size on the stack. It returns the new put in `EAX`.
- **Fragments** (`src/device/fragments.rs`). Halo's inlined `ZEnable`
  (18 sites such as `0x1eb94`, reached at boot through `ZEnable(2)` at
  `0x1267a`) calls `0x3f6840`, `0x3f6f40`, `0x3f6390` and `0x3f6190`
  directly when the old value was `D3DZB_USEW`. Each one reproduces the
  original's reads, device-field writes and push-buffer words. The device
  also runs them where D3D does: CreateDevice, SetViewport,
  SetRenderTarget, the ZEnable setter and a shader-kind change in
  SetVertexShader (`0x3f6d50`). See "Fragments and NV2A registers" below.
- **Display entries** (`src/device/display.rs`). `0x3f8c70` and
  PersistDisplay change the guest globals the way the original does,
  without the AV-encoder kernel calls (there is no TV encoder).
  PersistDisplay copies the image last presented into a surface of its own
  (`Backend::copy_surface`). The copy outlives teardown. On a copy-swap
  device (see "Swap" under Draws) the image is in back0 after a swap, and
  the title keeps drawing into it: the device copies back0 to the front
  before the first write into it (as the hardware's copy would have), so
  a later PersistDisplay still gets the image shown, and a frame nothing
  draws over costs no copy.
- **`SetStipple`** (`0x3f8cc0`). Method `0x1480` is
  `NV097_SET_STIPPLE_PATTERN`, 32 rows of a 32x32 polygon-stipple mask,
  not a gamma ramp. All nine of Halo's calls (`0x2dd86`, `0x2df56`,
  `0x3531c`, `0x445f8`, ...) pass the result of `0x1c290`: one of 17
  dither patterns from the table at `0x487288`, chosen from an opacity
  (screen-door fades). Stippling is
  switched on with `D3DRS_STIPPLEENABLE` (`SET_STIPPLE_CONTROL`, `0x147C`).
  The entry copies the pattern to device `+0x197C` (unless flag `0x10`)
  and pushes it; the drain records it in `HwState::stipple_pattern`
  (`Device::stipple_pattern`, also in every `ResolvedDraw::hw`). For filled
  polygon primitives the translated fragment shader masks pixels against
  that pattern while `STIPPLEENABLE` is set. The row and bit ordering match
  the [asymmetric Xbox capture](https://github.com/abaire/nxdk_pgraph_tests_golden_results/blob/55958b25a77bd011b39bf16241a5b8ea0481e568/results/Stipple_tests/Asymmetric_On.png)
  and its [test source](https://github.com/abaire/nxdk_pgraph_tests/blob/2cfe6073d59ab2a69f98cf4c6ca003c1a02c8c6d/src/tests/stipple_tests.cpp)
  at 640x480: row `31 - (y mod 32)`, bit `(x mod 32) XOR 7`. Vertical
  anchoring at heights not divisible by 32 still needs a hardware capture.
  Wireframe and point fill modes do not use polygon stipple. The visibility
  count pass does not yet apply this discard.
- **`D3DResource_IsBusy`** (`0x3fb710`, called from `0x1db90`/`0x1de50`
  among others while render resources are set up). See "GPU model:
  fences and the busy check".
- **Native helpers** (`d3d::abi::NATIVE`). `0x3fdbc0`, `0x3fde00` and
  `0x3fb7a0` compute surface/texture headers from their arguments and the
  read-only format table at `0x00403A50` (plus `XMemAlloc` and the
  overridden `D3DResource_AddRef` for `0x3fb7a0`). `Get2DSurfaceDesc`
  (`0x3fdb20`) also reads device `+0x1A14` (the back buffer) and `+0x196C`
  (the multisample type, which CreateDevice writes). Their callees touch
  no device global, NV2A register or push buffer, so the original code
  runs.
- **Native push-buffer writers.** BeginPush (`0x3fae10`) and
  `CDevice_SetStateVB` (`0x401030`) also run as compiled. They read only
  guest state the device keeps current (the dirty flags, `D3D__Streams`,
  the shader object at device `+0x794`), call only overridden entries (the
  commit `0x400fa0` and `MakeRequestedSpace` `0x3faae0`), and push only
  methods the drain decodes:
  - SetStateVB clears dirty bits `0x50`, commits when other dirty bits are
    set, then, with dirty bit `0x10`, pushes all 16
    `SET_VERTEX_DATA_ARRAY_FORMAT` words (`0x4010b2`: type and size from
    the shader's attribute format, stride from the stream), and one
    `SET_VERTEX_DATA_ARRAY_OFFSET` per enabled attribute (`0x401134`).
  - BeginPush (`ESI` = dword count) calls `SetStateVB(device, 0)`, makes
    room when `put + 4 * count + 4` passes the threshold plus `0x200`, and
    returns `EAX` = put. Its one caller (`0x4b24b` in `0x4b220`) then
    pushes `BEGIN_END`, a non-incrementing `INLINE_ARRAY` (`0x1818`) that
    a callback fills with vertices, and `BEGIN_END` 0.
  - The overridden commit leaves dirty bits `0xC0000070` set
    (`device::COMMIT_KEEPS`), as the retail commit does (`0x401013`), and
    clears the rest. SetStateVB consumes `0x10` and `0x40` itself, so a
    commit run first (from `Begin`, a setter or SetStateVB's own call)
    must not take them away, or the vertex formats are never pushed.
- **Same-size SetRenderTarget** (`0x3f7b30`, `[esp+4]` = target, `EBX` =
  depth/stencil, `ret 4`; one caller). Halo uses it for a target the size
  and format of the current one. It swaps the two surface pointers and
  pushes their offsets and the depth/stencil test enables
  (`0x3f7bd6..0x3f7c6a`), and clears device flag `0x100` when the target
  is the back buffer (`0x3f7c6d`). Unlike `0x3f7870` it leaves the target
  size fields, the depth scale and the viewport alone. The device does the
  same target switch as `0x3f7870` (eager render-to-texture write-back included)
  without the viewport reset, and does not check the size, as the original
  does not.
- **Internal references on targets.** Both SetRenderTarget paths take an
  internal reference on the new target and depth/stencil
  (`0x3f7b38..0x3f7b60`) and release the old ones through `0x3fb5a0`:
  - acquire adds `0x80000` to the surface, and to its parent texture the
    first time the surface is bound (the surface had no internal
    reference);
  - release, when the surface holds the last internal reference and has a
    parent, takes `0x80000` off the parent (destroying it when no
    reference of any kind remains, `& 0x78FFFF == 0`), then destroys the
    surface when its refcount is 0 and otherwise takes `0x80000` off it.
  - The device's own back-buffer and depth headers are skipped. A destroy
    goes through `Device::destroy_resource` and then the original
    `D3D_DestroyResource` (see "Resource destruction"). As retail does,
    the parent's decremented word is stored before the parent is
    destroyed.
  - The whole switch is staged and checked before anything is written. A
    switch that stops leaves every count, `Lock` and the binding as they
    were, so it can be retried. Destroys are staged with it and happen
    only when the switch commits.
  - Teardown releases the bound target and then depth/stencil the same way
    (`0x3fc720`, called at `0x3fc5cb`) before the texture, palette and
    stream releases, and clears both device fields. The releases are one
    staged set too: a teardown that stops changes no reference.

## Guest structures

| global / structure | address / size | who touches it |
|---|---|---|
| `D3D__pDevice` | `0x00407488` | D3D; written by CreateDevice |
| device object (`CDevice`) | `0x00404FE0`, `0x928` dwords | D3D and game code (fields below) |
| game's own device pointer | `0x005093B0` | game (CreateDevice's `ppDevice`) |
| `D3D__RenderState` | `0x00407498`, 166 dwords | D3D setters and game code directly |
| `D3D__TextureState` | `0x00404DD0`, `[4][32]` dwords | D3D setters and game code directly |
| `D3D__DirtyFlags` | `0x00407490` | D3D and game code (167 `.text` refs, 50 fns) |
| `D3D__Streams` | `0x00408458`, 16 x 12 bytes `{stride, offset, pVB}` | D3D only (game never writes it) |
| VS constant shadow | `0x00407738`, 192 x 16 bytes | constant setters |
| push-buffer size / kick-off size | `0x0040864C` (default `0x80000`) / `0x00408648` (`0x8000`) | read by inline writers |
| static VS object | `0x00408518` (`+0x14`: 16 x `{stream, offset, format, _}`) | `SelectVertexShaderDirect`, game choke point |
| game PSDEF buffer | `0x00484F68`, 60 dwords | game builds a `D3DPIXELSHADERDEF` here (indexed stores, e.g. in `0x00052040`) |

Device fields with evidence (offsets into the device object):

| offset | meaning | evidence |
|---|---|---|
| `+0x000` / `+0x004` | push-buffer put / threshold | inline writers in about 50 functions |
| `+0x008` | flags: `0x10` = `BehaviorFlags & 0x10`; `0x800` set by Begin and draws | CreateDevice, game tests |
| `+0x01C` | base vertex for `DrawIndexedVertices` (`SetStateVB`) | DrawIndexedVertices |
| `+0x02C` | GPU time, the last-use tag stored in resource `Lock` words | fences |
| `+0x784` | current pixel shader handle (checked by `BumpEnv`) | SetPixelShader, BumpEnv |
| `+0x788` / `+0x78C` | final-combiner-in-use / `PSDEF[59] & 0x100` | SetPixelShader |
| `+0x790` | PSTextureModes mirror | game code |
| `+0x794` | current vertex shader object | VS setters |
| `+0x924..+0x92C` | embedded pixel-shader object `{RefCount, Flags, pPSDef}` | 23 game functions fill it in and pass `device+0x924` to SetPixelShader |
| `+0xB04` / `+0xB08` | stream-input count / `D3DSTREAM_INPUT[]` | SetVertexShaderInput |
| `+0xF88` | textures[4] | SetTexture |
| `+0x93C` / `+0x940` / `+0x944` | W near, W far, 1/W far (from the projection) | SetTransform `0x3f802c..0x3f80eb` |
| `+0x948` | depth scale for the depth format (`D24S8` 16777215, `F24S8` 1e30, `D16` 65535, `F16` 511.9375) | `0x3fd970` |
| `+0x954` / `+0x958` | render-target size | SetRenderTarget `0x3f6741` |
| `+0x95C` / `+0x960` | super-sample scale | CreateDevice |
| `+0x96C` / `+0x970` | back-buffer scale | CreateDevice, SetBackBufferScale |
| `+0x980` | composite: projection x viewport | `0x3f6840` |
| `+0xC60` / `+0xCA0` | transforms (10 matrices) / projection | CreateDevice, SetTransform |
| `+0xEF8` / `+0xEFC` | screen offsets (0.53125) | CreateDevice |
| `+0x196C` | multisample type (0 stored as `0x11`) | CreateDevice; read by Get2DSurfaceDesc |
| `+0x197C` | 32-dword stipple pattern copied by `SetStipple` (`0x3f8cc0`) | `0x3f8cc0` |
| `+0x1C08` / `+0x1C10` / `+0x1C18` | depth-clip shadow (fixed function / program) and game-owned flags | `0x3f6190` |
| `+0x1DB8` | vertical-blank callback | game stores it at `0x12b093` |
| `+0x1A04` / `+0x1A08` | render target / depth surface | SetRenderTarget, game reads |
| `+0x1A10` | frame buffers (back buffers + 1, `0x3fc990`) | CreateDevice; Swap's rotation |
| `+0x1A14` / `+0x1A18` (/ `+0x1A1C`) | back buffer / display surface (/ second back buffer) headers; Swap rotates their `Data` | CreateDevice, Swap, viewport code |
| `+0x2478` | swap count, also returned by Swap | game reads |

Resource headers follow the documented XDK layout:

- `D3DResource` is `{Common, Data, Lock}`.
  - Common holds the type in bits 16–18 (VB 0, IB 1, push buffer 2,
    palette 3, texture 4, surface 5, fixup 6) and the refcount in bits
    0–15.
  - The internal refcount is in `0x00780000`; each bind adds `0x80000`.
  - `D3DCREATED` is `0x01000000`.
- `Data` is a physical address. The CPU view is `0x80000000 | pa`.
- `D3DPixelContainer` adds `Format` and `Size`. `D3DSurface` adds `Parent`.
- A palette's size comes from the top two bits of Common.

Presentation parameters at the two CreateDevice sites (boot `0x00012526`,
game `0x000129cd`):

- 640x480 back buffer, format 6 (`A8R8G8B8`);
- auto depth `D24S8` (`0x2A`);
- `PresentationInterval` `0x80000000`;
- `BehaviorFlags` `0x40`.

## Inline sites (game code doing D3D's work)

LTCG inlined D3D's fast paths into the game, so a large share of D3D
behaviour never passes through an entry point.

### State arrays

386 `.text` accesses to `D3D__RenderState` and 432 to `D3D__TextureState`.

- **Render state written directly** (writes / functions):

  | state | writes | functions |
  |---|---|---|
  | ALPHABLENDENABLE | 44 | 37 |
  | ALPHATESTENABLE | 39 | 36 |
  | COLORWRITEENABLE | 38 | 36 |
  | SRCBLEND | 27 | 24 |
  | DESTBLEND | 24 | 21 |
  | ZFUNC | 19 | 19 |
  | BLENDOP | 19 | 17 |
  | ZENABLE | 18, plus 18 reads | 18 |
  | ZWRITEENABLE | 17 | 16 |
  | DEPTHCLIPCONTROL | 9, plus 18 reads | 26 |
  | PSTEXTUREMODES | 8 | 4 |
  | BLENDCOLOR | 7 | 7 |
  | STIPPLEENABLE | 7 | 5 |
  | SOLIDOFFSETENABLE | 5 | 5 |

  Also written: stencil, alpha func/ref, polygon offset, fog, lighting,
  specular, point-sprite and point-scale states, and the final-combiner
  constants.
- **Indexed render-state writes** (`[reg*4 + base]`): 18 writes in 5
  functions. `0x00017b60` is a game-side render-state dispatcher.
- **Texture-stage state written directly** per stage, each in about 24–27
  functions: ADDRESSU/V/W, MAG/MIN/MIPFILTER, MIPMAPLODBIAS, MAXMIPLEVEL,
  MAXANISOTROPY, COLORSIGN, ALPHAKILL. Stage-indexed variants appear in 2–5
  functions.
- **`D3D__DirtyFlags`:** 167 `.text` references in 50 functions. The game
  sets the bits for the deferred commit.

### Push buffer

Each inline site follows the same pattern:

1. Compare put (`device+0`) with the threshold (`device+4`).
2. If put is past the threshold, call `MakeRequestedSpace`.
3. Store NV2A method headers and data words.
4. Store the advanced put.

`MakeRequestedSpace` is called 199 times from 53 functions. Methods stored
inline, with site/function counts:

| method | NV2A meaning | sites / fns | handled |
|---|---|---|---|
| `0x17FC` | `BEGIN_END` | 36 / 22 | yes |
| `0x1A00 + 16r` | `VERTEX_DATA4F` (register r) | 53 across `0x1A00`–`0x1AF0` | yes |
| `0x1880 + 8r` | `VERTEX_DATA2F` | 35 / 3–4 | yes |
| `0x1900` / `0x1940 + 4r` | `VERTEX_DATA2S` / `VERTEX_DATA4UB` | about 17 | yes |
| `0x1518` | `VERTEX4F` (position) | 8 / 3 | yes |
| `0x030C`, `0x1D78` | state mirrors (also stored in the arrays) | 18 / 18 each | payload applied |
| `0x0100`, `0x0110` | NO_OPERATION (with a software method, see below), WAIT_FOR_IDLE | 1 / 1 | yes |
| `0x1D8C` | `SET_ZSTENCIL_CLEAR_VALUE`/`SET_COLOR_CLEAR_VALUE`, two words: an inlined `InsertCallback` at `0x0002460e` (function `0x24970`, context), then NO_OPERATION 7 | 1 / 1 | yes (push callback) |
| `0x1480` | `SET_STIPPLE_PATTERN`, 32 words (`0x0001e4ae`, in `0x0001e370`; what `SetStipple` pushes) | 1 / 1 | yes (register state) |
| `0x1710` | `BREAK_VERTEX_BUFFER_CACHE`, value 0 (`0x00043a1a`) | 1 / 1 | yes (no-op: vertices are read at every draw) |
| `0x17C8` | `CLEAR_REPORT_VALUE`/`SET_ZPASS_PIXEL_COUNT_ENABLE` (an inlined `BeginVisibilityTest`, `0x00020cec`) | 1 / 1 | yes (restarts the backend's sample count; type 1 only; see "Visibility tests") |
| `0x181C` | `SET_EYE_VECTOR`, 3 floats (no literal push header in the XBE; no site known) | 0 / 0 | yes (register state: the eye of `DOT_RFLCT_SPEC_CONST`, `HwState::eye_vector`) |

An earlier version of this table also listed `0x0A20` and a 1800-word
`0x1A20`, both at `0x00052b89`. Those are not push-buffer headers: they are
indexed stores of combiner input/output encodings into the game's PSDEF at
`0x00484F68` (fields at `+0x00`, `+0x68`, `+0x88`, `+0xB4`) ahead of
`SetPixelShader`, and never enter the push buffer. They are not a stop
scenario.

`D3DDevice_Begin` also calls the commit helper `0x00400fa0` before inline
immediate-mode draws. So do 18 other game functions.

### Device fields

- Put and threshold are accessed in about 50 functions.
- `+0x924..+0x92C` (the embedded pixel-shader object) is accessed in 23.
- The game builds a 60-dword PSDEF at `0x00484F68` and calls
  `SetPixelShader` with `EAX = device + 0x924`.

## Device model

### Ownership

- **Guest-owned, read at use:**
  - the state arrays, read at every draw (see "State ordering" below);
  - resource headers and their contents;
  - the inline push-buffer words;
  - PSDEFs and vertex attribute formats.

  Setters only store into the arrays, so direct game writes and API calls
  look the same.
- **Device-owned (host):**
  - surfaces and textures on the backend;
  - the swap ring;
  - API bindings (textures, palettes, streams, shaders, targets);
  - vertex program memory (136 slots) and constants;
  - the viewport, mirrored at device `+0xEE0..+0xEF4` as SetViewport
    stores it (Halo saves and restores those six words at `0x14ac0`).

  Where D3D mirrors these into guest-visible fields that game code reads,
  the device writes the same fields: `D3D__pDevice`, the render-target and
  depth fields, `textures[]`, the constant shadow, the swap count, the
  device reference count (`+0x938`), the viewport words, and the internal
  refcounts in resource headers.

### Push buffer

The device does not intercept inline sites one by one. It gives them a real
guest push buffer (`DeviceConfig::push_buffer`).

- Every entry point first decodes the words written since the last entry,
  so inline draws run in program order relative to API calls.
- `MakeRequestedSpace` executes the words and rewinds put.
- Only the method subset in the table above, plus the register methods
  D3D's own fragments push (see "Fragments and NV2A registers"), is
  understood. Anything else is a `PushError::UnknownMethod` stop. Headers with reserved bits (bit 31,
  bits 17–16), control words (jump, call, return), other subchannels, a put
  that is not on a word boundary, and commands running past put are
  rejected too.
- A command is consumed only once it has fully executed. Any failure while
  draining **latches** the device: the failing command stays unconsumed and
  every later entry returns `D3dError::Faulted` (naming the original error
  and address), so a retry can never silently skip it.
- [graphics.md](graphics.md) says push-buffer paths are out of scope and
  should be intercepted at the region level. That is the long-term plan.
  This decoder is the narrow bridge until it exists: 22 functions emit
  inline Begin/End, and none of them needs to be lifted for it to work.

### State ordering

A value game code stores into `D3D__RenderState` reaches the GPU only when
something pushes it: a setter, `SetRenderState_Simple`, the deferred-state
commit (`0x00400fa0`, also run by `Begin`), or a draw. Words already queued
in the push buffer must not see later stores. The device keeps a *pushed*
snapshot of both arrays:

- setters (including `SetPixelShader`'s PSDEF copy and the ZBias derived
  slots) store into the array and the snapshot;
- the commit, `Begin`, and API draws refresh the snapshot from the arrays
  (D3D commits all pending state there);
- `SetRenderState_Simple` sets the one slot its `ECX` header encodes to
  its `EDX` value. The registers are what counts: game code stores the
  array before the call at some sites and after it at others;
- an inline state method in the push buffer sets its slot to the command's
  own payload when the drain reaches it, so several queued changes apply in
  order however late the drain is. The method-to-slot map is
  the image's encode table inverted, plus `0x030C` (depth-test enable),
  which Halo's inlined ZEnable pushes with the new `D3DRS_ZENABLE` (slot
  143) value at 19 sites (`0x1eb66`, ...). A method several slots share
  is not invertible. The only one is `0x1D90`, the placeholder of the
  unused slots; it is `SET_COLOR_CLEAR_VALUE`, so it lands in the register
  state as on the hardware and changes no render state. Any other
  unknown method is rejected;
- queued inline draws are drained against the snapshot.

### Bounds

Every guest-controlled size and address goes through checked arithmetic,
and allocations are bounded before anything is read:

- one read is at most 128 MiB (`guest::MAX_READ`); ranges that wrap the
  address space fail;
- a draw uses at most 2^20 vertices, and one stream or index list at most
  64 MiB. Immediate-mode primitives share the vertex cap at the one place
  every vertex is emitted (`Immediate::set`), so inline words split over
  any number of drains and `MakeRequestedSpace` rewinds cannot exceed it;
- `Clear` takes at most 4096 rectangles;
- base vertex plus index, stream offset plus start times stride, constant
  register plus count, and stack-argument addresses are all checked.

### Draws

- Every draw resolves to a `ResolvedDraw` that holds no guest pointers:
  - the gfx vertex layout;
  - stream bytes read from guest memory; for indexed draws, the vertices
    the indices reach plus the indices rebased onto them (`indices`);
  - translated render and stage state, with untranslatable values reported
    as `Event::BadState`;
  - the raw state words (`raw`) and the stage state `TextureStageState`
    does not model (`extras`: alpha kill, colour sign, bump environment,
    colour key);
  - `ShaderInputs`;
  - the viewport;
  - the render target's size and formats (`target`), and each bound
    texture's header, palette, format, kind, size and levels
    (`texture_info`).
- `GfxBackend` renders pre-transformed fixed-function draws its built-in
  shader reproduces exactly through that shader, and everything else
  through a `ShaderProvider` (`Nv2aShaders` over `crates/shaders` by
  default). A draw the provider cannot translate is skipped with an
  `Event::DrawSkipped` that gives the reason.
  - Each draw is scissored to `clip`, or, when the scissors leave several
    pieces (`clip_rects`), recorded once per piece in the same pass with
    that piece as its scissor. The pieces are disjoint, so blending touches
    every pixel once. The wgpu viewport stays the whole
    target, because positions are already window coordinates
    (pre-transformed vertices; NV2A programs apply the viewport
    themselves), so a `gfx::DrawViewport` would transform them twice.
  - Indices go through `gfx::Draw::indices`.
  - Draws are batched. Consecutive draws to the same colour and depth
    surfaces, and a whole-target clear before them, are recorded into one
    pending pass (`gfx::Renderer::record`), with their vertices, indices
    and uniforms packed into upload rings, and submitted together. The
    pending pass is encoded as one render pass (no Metal render encoder
    per draw, no attachment store and load between draws). A draw after
    one that writes only some colour channels starts a render pass of its
    own, unless it writes the same ones and cannot `discard`
    (`PreparedShader::discards`: alpha test, alpha kill, a clip-plane
    stage, the polygon stipple). That is the fast rule, the default
    (`GfxBackend::set_merge_discards`): much cheaper in a level, where a
    few edge pixels can differ. The exact rule
    (`H2_GFX_MERGE_PASSES=exact`) also gives every discarding draw a
    render pass of its own; with it the main menu's frames match one
    render pass per draw byte for byte.
    `GfxBackend::set_merge_passes(false)` gives one render pass per draw,
    the exact reference (docs/graphics/graphics.md, "Merged passes"). `GfxBackend::set_merge_check(Some(frame))` renders
    every pending pass of that frame both ways and prints what differs.
    Any other surface operation
    (another target, a partial clear, a read, write, copy, snapshot or
    destruction of a surface, `Swap`) submits the pending pass first. The
    target switch comes before the provider stages anything for the new
    draw, because the submit lets the rings reuse their space. A pending
    pass that will encode about 1024 passes (render passes, plus a count
    pass and a sum per visibility-tested draw) is submitted before the
    next draw too, on the same target: Metal loses the device at about
    2048 render passes in one command buffer, which one pass per draw
    reaches at about 2048 draws to one target (merged passes reach it only
    with that many draws that start their own pass).
    `GfxBackend::flush` submits by hand, and `GfxBackend::read_color`
    flushes before reading. Code that reads `GfxBackend::store` directly
    must flush first. `GfxBackend::batch_stats` counts draws, pending
    passes, render passes and submits. Clearing and reporting the pixel
    counter submit the pending pass first; a report's read waits for its
    own submission only.
  - `Renderer::set_pretransformed_z_max` is renderer-wide, so it is set
    before every built-in draw from that draw's depth format.
- Each draw also carries:
  - `clip`: the pixel rectangle it may touch. With no scissors that is the
    viewport intersected with the target; with scissors it is the scissor
    area on the target, and the viewport does not clip (below). SetViewport
    clips to the target and resets the scissors (exclusive mode
    included), as the original does; a null viewport keeps the current
    one.
  - `clip_rects`: the scissor area's disjoint pieces when it is not one
    rectangle (`clip` is then their bounding box), from
    `d3d::device::scissor_region`. As compiled, `SetScissors`
    (`0x3f8760`) clips to the viewport with no rectangles (`0x3f87a7`),
    makes one inclusive rectangle the surface clip (`0x3f886c`), and
    otherwise programs the NV2A window clip (`0x3f8928`: clip type =
    `Exclusive`, rectangle `i` in `SET_WINDOW_CLIP_HORIZONTAL/VERTICAL(i)`
    with inclusive maxima, surface clip = the whole target). The device
    follows those three branches: the viewport, the one rectangle, or the
    target cut by the window rectangles. The viewport is not a clip in the
    last two, so window rectangles outside it still draw. Inclusive keeps
    pixels inside any rectangle, exclusive keeps pixels outside all of
    them; an exclusive rectangle over the whole target draws nothing. Counted in
    `Stats::scissor_split_draws`. `Clear` ignores the scissors, as before
    (not checked against hardware).
  - `hw`: the NV2A register state (`d3d::hw::HwState`) as last pushed.
- State that cannot be expressed **stops** with `D3dError::Unimplemented`
  rather than rendering wrongly:
  - a back-buffer scale other than 1 x 1;
  - a backend that returns `DrawOutcome::Unsupported`.
- **Clears** (`Device::clear`, `src/device/raster.rs`). `Clear` intersects
  every rectangle with the viewport, or uses the viewport when Count is 0
  (`0x3fa1a0..0x3fa247`), clips to the target and skips empty
  intersections.
  - The whole target with all colour channels (or no colour) goes to
    `Backend::clear`.
  - Anything else goes to `Backend::clear_rects` with a `RectClear`: the
    rectangles, the colour plus a `ChannelMask` (`D3DCLEAR` `0x10` red,
    `0x20` green, `0x40` blue, `0x80` alpha), depth and stencil.
  - Halo's wrapper `0x14b60` turns `EAX` into flags as
    `(eax & 1 ? 0xF0 : 0) | (eax & 0x1E) << 3 | (eax >> 5) & 3`. The
    alpha-only clears are `0x3c515` (`EAX = 0x10` -> `0x80`, colour
    `0x00FFFFFF`, Z 0) and `0x13c3f` (`Clear(0, 0, 0x80, 0, 1.0, 0)`).
  - `GfxBackend::clear_rects` hands them to `gfx::Renderer::clear_rects`
    as a `MaskedClear` plus one `ScissorRect` per rectangle.
- **Render-to-texture** (`src/device/raster.rs`). A target or texture
  surface whose `Parent` is a texture gets its own host surface (an
  `RttTarget`). The `0x14850` 64x64 targets are such levels, and `0x14bc0`
  selects them.
  - The host surface is made with `Backend::create_texture_target`
    (`SurfaceStore::create_texture_target` on `GfxBackend`). It is filled
    from the level's guest texels through `Backend::write_surface`, which
    takes a `TextureDesc` carrying the guest format, size and pitch;
    `GfxBackend` calls `SurfaceStore::write_color`. A backend that cannot
    fill it returns `Ok(false)`, and the surface starts as created.
  - Bound again, a cached level is refilled when its guest texels changed,
    judged apart from the header identity and by the upload policy, as
    for textures: `Always` every time, `Tracked` after a reported write,
    `Verify` when the hash differs (with `Event::MissedInvalidation` if no
    write was reported). A level drawn into and not yet written back is
    kept as it is (lazily, the guest's changes are merged in, below).
  - A draw or clear into it marks it dirty.
  - Binding a level first writes back any other dirty level whose memory
    overlaps it (two surface headers over one level), so it starts from
    their pixels.
  - Lazy render-to-texture is the default (`Device::set_rtt_direct`) on
    a backend that samples surfaces (`Backend::can_sample_surfaces`;
    `GfxBackend` does, `RecordingBackend` only with `sample_surfaces`).
    A draw that samples a texture over a level reads the level's host
    surface in place (`Backend::surface_texture`, `GfxBackend` binds the
    surface's own view), with no write-back and no upload
    (`Stats::rtt_direct_samples`), when:
    - the texture is 2D, has one level and is `A8R8G8B8` or
      `LIN_A8R8G8B8` (texels that read back unchanged, so the surface
      holds what an upload of the written-back level would);
    - a level has the same `Data`, format, size and pitch (the one drawn
      into, if several do; any other level drawn into over part of the
      same memory is written back first);
    - the stage has no `COLORSIGN` (signed channels are stored biased);
    - the level is not the target being drawn (that reads what the level
      held before the draw, through guest memory, as before).
    Otherwise the texture goes through the write-back and the texture
    cache as before. Before the surface is used, it is refreshed from
    guest memory like a bound target.
  - A dirty level is written back to guest memory through
    `Backend::read_surface` and noted in the write tracker. Lazily that is
    when the guest is about to read it:
    - `BlockOnResource` or the busy check `0x3fb710` on the level, its
      texture, or a resource whose memory overlaps it (a texture or
      surface by its whole layout, another resource by its first byte,
      reading only its 12-byte resource header);
    - `BlockOnTime` (after a fence wait anything drawn before may be
      read) and push-buffer callbacks (Halo's `0x24970` reads texels);
    - a sample that cannot read the surface in place, of a texture whose
      memory overlaps the level (an offset alias included);
    - `PersistDisplay`, a destroy of the level or its texture, a header
      rewritten with another identity, and teardown, which then destroys
      the host surfaces.
    Not when the target changes, at `Swap` or at `KickOff`. Each write-back
    is counted by what asked for it (`Stats::writeback_causes`), and
    `H2_D3D_WRITEBACK_TRACE=1` prints each one with the level, format and
    size, for finding which waits a scene's read-backs come from. Eagerly
    (`set_rtt_direct(false)`, or a backend that cannot sample surfaces)
    also at every target change, `Swap` and `KickOff`, and every busy
    check or wait writes back all dirty levels.
  - Lazily, the guest may store into a level whose pixels are only on the
    host. Each level keeps its bytes from when surface and memory last
    matched; when it stops being the target, the guest bytes as they are
    then become the match (eagerly the write-back there would overwrite
    any store made while it was the target). When the level is next
    bound, sampled or written back, the guest's stores since win over the
    drawn pixels, the merged level is written back, and the surface is
    filled with it. A store reported exactly (`Device::note_write`) owns
    every byte it covers, whatever its value. Exact overrides are recorded
    only while the level is neither the active colour target nor the
    active depth/stencil target. A failed write to guest memory keeps
    those overrides for the retry. Page reports
    (`Device::note_written_pages`) and changes the upload policy finds
    own only the bytes that differ from the match: a store of the value
    memory already held is not seen there and takes the drawn pixel, and
    a page report that changed nothing (the device's own write-back
    reported back) costs no write-back. Telling those apart needs exact
    store ranges or a trap on the first store to a page, which the boot's
    page-granular barrier does not give; a title that waits on the level
    before its CPU writes (as it must on the Xbox) is not affected.
  - A compressed format stops. Eagerly, a header rewritten while its
    level holds unwritten pixels stops too.
  - **Depth levels.** A level bound as `pNewZStencil` (shadow maps; the
    first campaign level's first frame binds one at about present 4210)
    gets a host depth/stencil surface (`Backend::create_surface(..,
    true)`) of the level's format and size: `D24S8` or `D16`, swizzled
    or linear. It is filled from the level's texels and written back
    exactly like a colour level, through `Backend::write_surface` and
    `Backend::read_depth` (guest words `z << 8 | stencil` for `D24S8`,
    `z` for `D16`, in the level's layout; `GfxBackend` uses
    `SurfaceStore::write_depth` and `read_depth_as`). It is marked dirty
    only by draws the backend drew that can write depth or stencil
    (`ZENABLE` with `ZWRITEENABLE`; or `STENCILENABLE` on a format with
    stencil, a non-zero `STENCILWRITEMASK` and some operation other than
    `KEEP`) and by clears of depth or stencil; a skipped draw dirties
    nothing. Eagerly it is written back whenever either the colour or the
    depth/stencil binding changes. A backend without `read_depth` stops
    at the write-back.
  - **Lazy depth levels** (the default with lazy render-to-texture on a
    backend that samples depth surfaces, `Backend::can_sample_depth_surfaces`:
    `GfxBackend` with `DEPTH32FLOAT_STENCIL8`; `Device::set_lazy_depth`,
    `H2_D3D_LAZY_DEPTH=0` turns it off). A depth level is then written
    back where a colour level is (a wait or busy check on it, `BlockOnTime`,
    a callback, a sample that cannot use the surface, destroy, teardown),
    not at the binding change. When it stops being the depth target the
    guest bytes become the match, as for a colour level, so guest stores
    after that win at the write-back. A draw that samples a dirty `D24S8`
    level through a whole single-level 2D texture of the same format, size
    and layout (no `COLORSIGN`) gets `Backend::depth_level_texture`: a
    texture made on the GPU from the surface that holds exactly what an
    upload of the written-back level holds (`gfx::depth_bits`: the
    read-back's `round(clamp(d, 0, 1) * 16777215)` with halves away from
    zero, as `depth_channels` lays the bits out). It is made once per time
    the level is drawn into (`Stats::depth_level_textures`). Anything else
    (`D16`, a mismatched header, a level with guest stores since) goes
    through the write-back and the texture cache as before. `gfx`'s
    `depth_bits_*` tests compare the two paths over every 24-bit depth
    and a sweep of `f32` depths with the values next to each rounding tie;
    `gpu_lazy_depth_levels_sample_and_write_back_as_the_eager_ones` checks
    pixels and guest bytes at the wait against the eager device.
    Float depth levels (`F24S8`, `F16`) stop: the host surface does not
    hold their bits. A level bound as colour after depth (or the other
    way), or whose format does not match how it is bound, stops too.
  - **Depth textures.** The parent texture is then sampled from guest
    memory like any texture (written back first). A depth-format texture
    decodes to its raw bits (`gfx::convert::depth_channels`: depth
    bytes high to low in red, green, blue and the stencil in alpha;
    `D16` in red and green). On the NV2A a depth-format texture is a
    shadow map: xemu (`psh_append_shadowmap`) compares the stored depth
    with the texture coordinate under `SHADOWCTL`'s function, which D3D
    sets from `D3DRS_SHADOWFUNC` (guest slot 156, `0x3f5330`;
    `state::slot::SHADOWFUNC`; the function pushes `Value - 0x200` to
    `NV097_SET_SHADOW_DEPTH_FUNC`, `0x1E6C`), and returns 1 or 0 in every
    channel (PROJECT2D compares with 0, PROJECT3D with `r/q` clamped to the
    format's depth range). A colour-format header over the same memory is
    raw bits, not a comparison. `Nv2aShaders` passes each such stage to the
    translator as `PixelOptions::shadow` (`nv2a::shadow_stages`): the
    format, the `D3DCMP_*` function, 2x2 percentage-closer filtering when
    the stage's min or mag filter is linear, and the u/v addressing (see
    docs/graphics/shaders.md, "Shadow maps"). A depth texture sampled by a mode
    other than PROJECT2D/PROJECT3D, or a `SHADOWFUNC` that is not a
    `D3DCMP_*` value, skips the draw with `Event::DrawSkipped` naming the
    stage, format and value; the texture is uploaded all the same.
  - In the boot (boot plus this device, `maps\00a_introduction.map`
    through the scripted click-through) the first level no longer stops
    at about present 4210: it ran to 8164 presents with no stop, drawing
    into 512x512 and 128x128 targets as well as the back buffer. Its one
    shadow map is a `LIN_D24S8` texture (`0x004b50b4`) sampled on stage 0
    with `D3DRS_SHADOWFUNC` `0x204` (`GREATER`); those draws (136,667 over the
    run) were the only skips left. They are now translated as
    comparisons (`stored > reference`), but the boot hasn't been rerun
    with that yet.
- **Swap** (`Device::swap`, `0x3f9d00`). CreateDevice sets device flag
  `0x4000` (`device::COPY_SWAP_FLAG`) for a multisampled device (type bits
  `0x3000`) or `D3DSWAPEFFECT_COPY` (`0x3fc9a2..0x3fc9c0`), and stores the
  frame-buffer count, back buffers + 1, at `+0x1A10`. That flag, not the
  swap flags' `COPY` bit, decides what Swap does:
  - **Flip** (flag clear; Halo's device: no multisampling, swap effect 1
    or 2 from `0x128c0`). With `FINISH` (4) set, as in `Swap(0)`
    (`COPY | FINISH`, Halo's one per-frame call at `0x140fc`), `0x3f9d75`
    sets a fence, counts and goes to `0x3f9a70`, which rotates `Data` and
    `Lock` through the headers from `+0x1A14`: the display surface
    (`+0x1A18`) gets the back buffer's, the back buffer the last
    header's; the CRTC then scans out the display surface's `Data`. The
    headers stay put; the buffers they name trade places. The device does
    the same: its host ring rotates, and the guest headers' words rotate
    as `0x3f9a70` rotates them. Without `FINISH` the original returns the
    swap count and does nothing else (`0x3f9d7a`); so does the device,
    presenting nothing.
  - **Copy** (flag set). The steps are separate, as in `0x3f98f0`:
    - `COPY` (1) or `BYPASSCOPY` (2) set a fence and count. With three
      frame buffers the display and third headers then exchange `Data`
      and `Lock` (`0x3f991c..0x3f9944`; the host front and second back
      buffer trade places, after any copy still owed from back0 is made).
      With `COPY`, back0 is copied into the display buffer (deferred to
      the first write into back0, see "Display entries").
    - With fewer than three frame buffers scanout moves to the display
      buffer at once (`0x3f998c..0x3f99d4`); with three only on `FINISH`
      (`0x3f9981`, `0x3f9a16..0x3f9a57`), which moves it even with
      neither copy bit set, and does not count. So `Swap(1)` then
      `Swap(4)` shows the copied frame at the second call.
  - **`FINISH`** on either kind ends with `0x3f9cc0`: the display
    header's `Lock` (its parent's, for a level) takes the GPU time
    (`+0x2C`), and a new fence is stored at `+0x1974[swap count & 1]`
    (`layout::device_field::SWAP_FENCES`). The order matches; the numbers
    do not, since `SetFence` here advances the time by 1 and returns the
    new value where retail (`0x3fa69e..0x3fa73b`) advances it by 2 and
    returns the old one (see "GPU model").
  - A device re-created after teardown starts with no lost-image state
    (`shown_lost`, set when a backend cannot make the deferred copy).
- **Frame buffers as textures** (`src/device/raster.rs`). The frame
  buffers and the depth buffer exist only on the host. Their headers
  carry `Data` values above any Xbox RAM that name them. Each frame
  buffer keeps one `Data` for its life, whichever header names it:
  `d3d::device::BACK_BUFFER_DATA` (`0x0C000000`, the back buffer at
  creation), `DISPLAY_BUFFER_DATA` (`0x0E000000`, the display surface at
  creation), `THIRD_BUFFER_DATA` (`0x0F000000`, two back buffers only);
  the depth buffer has `DEPTH_BUFFER_DATA` (`0x0D000000`). The back and
  display headers are distinct (the display header and a third live at
  `device::FRAME_HEADERS_OFFSET` in the scratch range).
  - Halo builds such textures at `0x14600`, called at boot (`0x127c1`, just
    before `0x14850`) and again from `0x12b9e` and `0x137f2e`: `LIN_A8R8G8B8`
    640x480 headers (`Common` `0x40001`, format `0x11229`, size
    `0x271DF27F`) over the back-buffer (`+0x1A14`) and display-surface
    (`+0x1A18`) `Data` as they are then, kept at `0x509354`/`0x509358`. It
    also builds two colour textures over the depth surface's `Data`
    (`0x509368`, `LIN_A8R8G8B8`; `0x50936C`, format `0x3F`
    `LIN_A8B8G8R8`) and a colour surface over it (`0x509370`, a copy of
    the back-buffer header with the depth `Data`, selected as a target by
    `0x14bc0`). Its global texture switch (`0x14f80`, table `0x15114`,
    results in `0x51F3D8`) binds them: `0x509354[frame & 1]` (current) at
    `0x14fe9`, `0x509354[(frame - 1) & 1]` (previous) at `0x14fc1` and
    `0x150a7` (frame counter `0x485AA0`), `0x509368` at `0x15020` and
    `0x50936C` at `0x15032`. Because every swap flips, the two textures
    name the buffer being drawn and the buffer last presented in turn,
    and the frame parity picks between them. The intro reaches the
    crossfade evaluator `0x002ab270`, which reads the previous parity.
  - A texture whose `Data` is a frame buffer's, `LIN_A8R8G8B8` or
    `LIN_X8R8G8B8`, 2D, the buffer's size, is sampled from a copy of that
    buffer made for the draw (`Backend::snapshot_surface`; `GfxBackend`
    uses `gfx::SurfaceStore::snapshot`). Over the back buffer it reads the
    image being drawn, also while it is the target: the draw reads what
    was there before it (same-frame feedback). Over the display surface
    it reads the image last presented, which nothing draws into until the
    next flip makes it the back buffer. On a copy-swap device whose shown
    image still waits in back0, that is back0; if the copy to the front
    was lost (`copy_surface` unsupported), the draw is skipped. All are
    counted in `Stats::frame_snapshots`, the display-surface ones also in
    `Stats::previous_frame_snapshots`. An `X8` texture reads alpha as
    one: the copy's alpha is cleared to one with an alpha-only masked
    clear, as uploads of `X8` texels force it to 255.
  - A new device (CreateDevice after teardown) starts with new, cleared
    frame buffers and the headers back at their first `Data`: nothing
    presented by the old device can be read as the previous frame. Halo
    rebuilds its textures from the new headers (`0x12b9e`).
  - A texture inside the depth buffer's `FRAME_DATA_SPAN` reads its
    depth/stencil bits as the colour format it names, the way d3d-pass's
    back-buffer copy reads colour (`Device::depth_texture` in
    `src/device/raster.rs`). The depth buffer is read back as the guest
    words it would hold (`Backend::read_depth`: linear, pitch `width *
    bpp`, `D24S8` as `z << 8 | stencil`), and the bytes from the texture's
    offset on are decoded as the texture, exactly as if guest memory held
    them. So `0x509368` (`LIN_A8R8G8B8`) samples the depth's top byte as
    alpha, its middle and low bytes as red and green and the stencil as
    blue, and `0x50936C` (`LIN_A8B8G8R8`) the same with red and blue
    swapped. The main menu's depth-based fog/distortion pass (about 400
    draws a frame, `HILO` dot mappings) reads it this way; the `HILO`
    modes themselves are the shader translator's.
    - The readback is cached by a depth generation, bumped by every drawn
      draw that can write depth or stencil into the device's depth buffer
      (the same test as for depth levels: read-only stencil tests and
      skipped draws do not count) and by every depth or stencil clear of
      it; each header and `COLORSIGN` mask is decoded once per readback.
      Colour aliases use the same biased signed-channel storage as ordinary
      textures, so signed and unsigned bindings of one header can coexist.
      All of its host textures are freed when the header is destroyed. A run
      of draws that test depth without writing it costs one readback. Like
      the back-buffer copy, a draw sees the buffer as it was before the draw.
      Counted in `Stats::depth_reads` and
      `Stats::depth_alias_uploads`.
    - The colour surface over the same memory (`0x509370`, rasterizer
      target 3: `0x14bc0` -> `0x14c21`; as a texture, the texture switch ->
      `0x15020`, it is `0x509368`) gets a host colour surface of its own
      (`raster::DepthColor`, noted in `surface_for`). The memory holds
      whichever was drawn into last: a drawn draw or clear into that
      surface makes it newer (and bumps the depth generation), one that
      writes the device's depth buffer's depth or stencil makes the depth
      newer again. While the colour surface is newer, a texture over the
      depth buffer reads its pixels instead of the depth bits
      (`Device::depth_color_texture`): the whole surface as a linear
      32-bit texture of its size with no `COLORSIGN` (`0x509368`) is a copy
      (`snapshot_surface`, counted in `Stats::frame_snapshots`), any other
      view (`0x50936C`, an offset, a sign) decodes the surface read back as
      guest bytes (`read_surface`), once per generation. The level loading
      screen needs it: every frame `0x296c0` copies target 1 into target 3
      and `0x29370` box-filters `0x509368` back into target 1 with
      `DESTBLEND = CONSTANTCOLOR`, a trail. Reading the depth bits there
      (stencil zero as blue, depth bytes as red and green) fed a constant
      yellow into the trail. Not modelled: target 3 does not start from the
      depth bits when it is bound after depth was drawn (a draw that blends
      with or only partly covers it sees its own older pixels), and depth
      tests after target 3 was drawn into use the depth as it was. Tests:
      `textures_over_the_depth_buffer_read_the_colour_surface_drawn_into_last`
      and the GPU and render-thread ones after it in
      `tests/depth_textures.rs`.
    - A texture that runs past the end of the buffer, or whose header
      does not decode, skips with a reason. So does any texture over it
      when the backend has no `read_depth` (`GfxBackend` needs
      `DEPTH32FLOAT_STENCIL8`; Metal has it), and a multisampled depth
      buffer.
  - Any other texture inside a frame buffer's `FRAME_DATA_SPAN` (another
    format or size, or an offset into it) skips the draw with
    `Event::DrawSkipped` saying why, when the draw reads that stage's
    image: a pixel shader's `PSTEXTUREMODES` gives the stage a mode that
    fetches a texel (`TexMode::texture_dim`; not `CLIPPLANE`, `PASSTHRU`,
    `BRDF`, `DOT_ZW` or `DOTPRODUCT`), or, fixed function, the stage comes
    before the first `COLOROP = DISABLE`. On a stage the draw does not
    read, a texture over any buffer is left unbound (nothing is copied or
    read back) and the draw goes ahead. A backend without
    `snapshot_surface` skips too.
  - Guest memory at those addresses is never read or written by the
    device. A CPU read of the frame buffer through `Data` faults instead
    of returning stale bytes.

### Textures: lazy upload, dirty tracking

A texture is uploaded at the first draw that samples it, not when it is
bound.

- The cache is keyed by texture header plus, for P8, palette header: one
  header bound with two palettes is two host textures.
- An entry is valid while all of these are unchanged:
  - the identity words (`Data`, `Format`, `Size`);
  - the palette's header, `Common` word (which carries the size bits),
    data address and length;
  - the `WriteTracker` generations of the texel range and the palette
    range.
- `UploadPolicy::Verify` hashes texels and palette entries together.

### Vertex cache

Most of a level's vertices (BSP, object models) never change, but every
draw used to copy its whole vertex window out of guest memory, and the
render thread then staged the same bytes in its upload ring. The vertex
cache (`crates/d3d/src/vertex_cache.rs`) keeps them instead, under the
same write-tracker contract as textures.

- **Key.** A vertex window is keyed by guest address and length (the
  stream's `Data` translated, plus offset and first vertex), so two index
  windows over one buffer are two entries. An index list for
  `DrawIndexedVertices` is keyed by its address and length, and only when
  it lies in the view the tracker reports (`AddressMap::to_virtual` of
  its own `to_data`: the contiguous `0x80000000 | pa` view); a list
  elsewhere is read every time. `DrawVerticesUP` and immediate-mode
  vertices are never cached.
- **Validity.** An entry remembers the tracker generation when it read
  the bytes, and is clean while no page it covers has a newer one. The
  tracker is page-granular: a four-byte store re-reads every window on
  that page.
- **Hit.** The draw carries the snapshot (`StreamData::Cached`, or
  `IndexData::Cached`: the smallest and largest index and the list
  rebased by the smallest, so a clean list is not read, scanned or
  rebuilt). No copy on the guest's thread. Each snapshot has an id that
  is never reused; `GfxBackend` passes it to `gfx::Renderer`
  (`Draw::stream_ids`, `Draw::index_id`), which keeps a GPU buffer per id
  (`gfx::persistent`) and binds it again instead of staging the bytes.
- **Admission.** A window is copied the first time it is seen (a *ghost*
  entry remembers it) and snapshotted at its next use if still clean. A
  window found written twice counts as dynamic (particles, decals,
  streamed or skinned vertices) and is snapshotted again only after two
  clean frames, so a buffer the guest rewrites every frame costs a lookup
  and the old copy, not an allocation and a free.
- **Bounds.** Both caches drop least recently used entries past a budget:
  `H2_VERTEX_CACHE_MB` (default 128) of vertex snapshots, a quarter of it
  for index lists, 65536 entries each; the renderer holds at most the
  same budget of GPU buffers.
- **Policies.** `UploadPolicy::Verify` reads the guest bytes on every hit
  and compares them with the snapshot; a difference raises
  `Event::MissedInvalidation` (header 0 for an index list), drops the
  snapshot and draws the bytes just read. `UploadPolicy::Always` bypasses
  both caches. `H2_VERTEX_CACHE=0` (or `Device::set_vertex_cache(None)`)
  turns them off: every draw copies, as before.
- **Counters.** `Stats::vertex_bytes_copied`, `vertex_bytes_reused`,
  `vertex_cache_hits`, `vertex_cache_fills`, `index_cache_hits`; frame
  stats get `vtx_copied` and `vtx_reused` (bytes per frame).
  `gfx_backend::GfxBackend::persistent_vertex_stats` counts GPU buffer
  hits and uploads.
- **Evidence.** `tests/vertex_cache.rs` runs the same frames with the
  cache on and off (static, a page rewritten, four bytes rewritten, an
  index list rewritten, a buffer freed and its memory reused, a buffer
  rewritten every frame) and requires identical draws through the
  recording backend and the render thread, and identical pixels on the
  GPU. `cargo bench -p d3d --bench streams` (1000 draws a frame, about
  45 MiB of vertex windows): 3.05 ms/frame with the cache off, 1.33 with
  it on (render thread, guest side: 4.17 and 1.84); 0 bytes copied a
  frame instead of 46,165 KiB, and no allocation of 4 KiB or more instead
  of 0.75 a draw. With one of four vertex buffer sets rewritten every
  frame (`H2_BENCH_DYNAMIC=1`): 3.32 and 1.81 ms, 11,231 KiB copied a
  frame, no snapshot taken after warm-up.

What it relies on is rule 1 below: a guest store into vertex or index
memory the barrier does not report leaves the cache drawing old
vertices. Verify finds such a store.

### Invalidation contract

The runtime has to keep these rules:

1. It reports every guest store the device did not make, by guest
   virtual address: exactly through `Device::note_write(addr, len)`, or
   as whole 4 KiB pages through `Device::note_written_pages(addr, len)`
   (the boot's barrier, which also reports the device's own stores back).
   This covers lifted code, the interpreter, and kernel or DMA-like copies.
   Only the exact form tells a lazily held render-to-texture level which
   bytes the guest stored (see "Render-to-texture").
2. A write must be reported before the next D3D entry point runs.
3. The runtime folds aliases first: physical and contiguous views map to
   the `0x80000000 | pa` view the device uses.
4. Only pages that back a cached resource record a generation. The tracker
   exposes a watched-page bitmap (`WriteTracker::watched_bitmap`) so a
   write barrier can filter cheaply.

Fallbacks:

- `UploadPolicy::Verify` hashes on every use and reports
  `Event::MissedInvalidation` when the barrier missed a write.
- `UploadPolicy::Always` re-uploads every time.

### Errors and events

- `D3dError` means the call cannot complete, and the runtime should stop:
  `Unimplemented`, `Invalid`, `Push`, `Faulted`, `NoDevice`, `Guest` and
  `Backend`. A backend reports state it cannot render as
  `DrawOutcome::Unsupported`, which the device turns into `Unimplemented`.
- A frame-wait stop (see "Vertical blank") uses the same hook, with the
  engine address `0x12b2e0`.
- `Event` means something is worth knowing but execution continues:
  `DrawSkipped`, `BadState` and `MissedInvalidation`.
- Nothing is skipped silently.

## Integration

`d3d::register(&mut Dispatch, SharedDevice<B>)` (in `runtime_glue`)
installs every override against the real `crates/runtime`:

```rust
let cfg = d3d::DeviceConfig {
    upload: d3d::UploadPolicy::Always, // until step 3 exists
    ..d3d::DeviceConfig::halo2(push_buffer, scratch)
};
d3d::runtime_glue::map_device_memory(&mut guest, &cfg)?;
let device = d3d::runtime_glue::shared(cfg, d3d::GfxBackend::new(ctx));
let report = d3d::register(&mut dispatch, device.clone());
let runtime = runtime::Runtime::new(guest, dispatch);
```

1. **Memory.** `push_buffer` (more than 32 KiB; 256 KiB is comfortable) and
   `scratch` (256 bytes, for the back-buffer and depth headers) are
   host-chosen guest ranges nothing else uses; `map_device_memory` maps
   them read/write. Resource `Data` fields are read at `0x80000000 | pa`
   (`AddressMap::XboxContiguous`); use `AddressMap::Identity` or `Offset`
   if the runtime maps contiguous memory elsewhere.
   Set `nv2a_window: Some(base)` to give game code the NV2A register
   window at device `+0x934` (see "NV2A register window");
   `map_device_memory` then also maps its two pages, `base + 0x3000` and
   `base + 0x400000`. Without it `+0x934` stays zero and the game's
   post-`KickOff` register reads fault.
2. **Overrides.** Each handler copies `CpuAbi` into `abi::Regs`, runs the
   adapter on the locked guest memory, writes `EAX`/`EDX` back if set and
   returns with `Guest::ret(cpu, cleanup)`. Other registers are left as
   they were. Lock order is guest memory, then device.
   - A guest memory fault inside D3D is returned as the runtime's own
     `GuestFault` (the runner records `Event::MemoryFault`).
   - Any other `D3dError` is recorded on the device (`Device::last_stop`,
     a `HleStop` with the entry and the message) and returns
     `TransferAbi::stop(id)`. The default hook emits the runtime's
     `Event::Unsupported { pc: entry, mnemonic: entry name, detail: the
     HleStop text, backtrace: ctx.backtrace(entry) }`, so
     `ExecCtx::report` shows it like any other stop. `register_with` takes
     a different hook.
   - Before the adapter runs, the handler drains the push buffer up to
     the next push-buffer callback and, if there is one, calls it and
     enters the entry again (see "Push-buffer callbacks").
   - After an adapter succeeds, the handler calls the original
     `D3D_DestroyResource` for each resource the entry destroyed
     (`runtime_glue::destroy_then_return`, see "Resource destruction"),
     then delivers pending vblanks, and only then returns to the caller.
   - `abi::NATIVE` addresses get no override.
   `tests/registration.rs` runs Halo's boot device sequence through
   `runtime::Runner` with these overrides.
3. **Write barrier.** `UploadPolicy::Always` (or `Verify`, to find the
   misses) stays the bring-up default; `DeviceConfig::halo2` alone gives
   `Tracked`, so set `upload` as above. `Tracked` is only correct once the
   runtime keeps the invalidation contract above:
   - Forward every guest store D3D did not make to
     `Device::note_write(addr, len)` (exact) or
     `Device::note_written_pages(addr, len)` (whole pages). That covers stores from lifted code,
     the interpreter, kernel calls (`NtReadFile`, `memcpy`-like HLE
     helpers) and DMA-like copies.
   - Fold aliases to the `0x80000000 | pa` view first.
   - Report before the next D3D override runs.

   A cheap form is a check against `WriteTracker::watched_bitmap` in the
   store path, deferring the `note_write` calls to the next override
   entry. The device's own stores (render-to-texture write-back included)
   are already noted.
4. **Vertical blank.** See "Vertical blank" below: on the logical clock,
   `kernel.add_periodic(..)` with `d3d::vblank::clocked_vblanks` and
   `d3d::register_frame_wait(.., FrameWait::HALO2_CLOCKED, ..)`; without a
   kernel, `Device::set_vblanks_per_swap(n)` and/or the inline frame wait.
5. **Present.** `GfxBackend::on_present` receives the surface on every
   `Swap`, after the pending pass has been submitted.
   With `d3d::ThreadedBackend` (opt-in) every backend call, the hook
   included, runs on a render thread; see [render-thread.md](render-thread.md). Blit it with
   `gfx::surface::present_surface`.
6. **Shaders.** `GfxBackend::new` installs `Nv2aShaders`; replace it with
   `GfxBackend::with_shaders`, or drop it with `without_shaders`.

## Fragments and NV2A registers

`src/device/fragments.rs` reproduces D3D's viewport and transform
fragments. Each doc comment there cites the addresses it was checked
against with `h2 disasm`. Intermediates are `f64` and stores round to
`f32` where the x87 code stores; the `0x3fd080` matrix product is `f32` in
the SSE accumulation order.

- `0x3f6190`: the viewport push.
  - For a program or pass-through shader (`[vs+4] & 0x12`), it pushes
    `VIEWPORT_OFFSET` and `VIEWPORT_SCALE` from `0x3f6060` (skipped with
    device flag `0x200`), then `CLIP_MIN/MAX` from `+0x1C10`.
  - For fixed function, it pushes `VIEWPORT_OFFSET = [x + ox, y + oy, 0,
    0]`, then the clip from `+0x1C08`.
  - The clip shadow is refreshed unless the game owns it (`+0x1C18` bits
    0/1). W-buffered (`ZENABLE` = 2), it is `[944 * 93C * 948, 948]`;
    otherwise `[0, 948]` for programs and `[minZ * 948, maxZ * 948]` for
    fixed function.
- `0x3f6390`: `SET_CONTROL0`. The base is `0x100001`; `0x10000000` with
  YUV, `0x10000` when w-buffered, `0x1000` for a float depth format.
- `0x3f6840`: the viewport matrix (scaled by `944 * 948` when
  w-buffered), multiplied by the projection, into `+0x980`; dirty bit
  `0x200`.
- `0x3f6f40`: for a pass-through shader, constants 0–1 (`[ssx, ssy, zs,
  w]` and `[ox, oy, 0, 0]`) and the pass-through program, chunked 32 words
  per header (`0x3f6b70`). The program is read from the image: `0x403E40`
  (48 words, no fog table), `0x403D90` (44, affine projection) or
  `0x403CD0` (48). Space is reserved as the original does.
- CreateDevice sets up what they read (`0x3fc421..0x3fc462`, `0x3fba30`):
  - identity transforms and the W parameters;
  - the default FVF object `0x408340` at `+0x794`;
  - the static programmable object `0x408518`, cleared, with flags `0x10`;
  - the multisample type and scales, depth scale and target size;
  - the 0.53125 offsets;
  - then the composite, the pass-through program and the viewport push.
- SetRenderTarget resets the viewport to the whole target
  (`SetViewport({0, 0, 0x7FFFFFFF, 0x7FFFFFFF, 0, 1})` from `0x403B88`),
  updates the target fields and pushes the viewport.

The drain applies these register methods to `d3d::hw::HwState`
(`Device::hw`, and `ResolvedDraw::hw`):

- `0x0290` CONTROL0;
- `0x0394`/`0x0398` CLIP_MIN/MAX;
- `0x0A20` VIEWPORT_OFFSET and `0x0AF0` VIEWPORT_SCALE, also mirrored into
  constant registers 59 and 58, where vertex programs read them (the xemu
  model);
- `0x0B00..` the transform program and `0x0B80..` transform constant
  words, through the hardware cursors (`0x1E9C`/`0x1EA4`);
- `0x1E94`/`0x1E98` execution mode and context write enable;
- `0x1EA0` program start;
- `0x147C`/`0x1480` stipple control and pattern;
- `0x1710` vertex-cache break (no effect);
- `0x1720..0x175C` `SET_VERTEX_DATA_ARRAY_OFFSET` and `0x1760..0x179C`
  `SET_VERTEX_DATA_ARRAY_FORMAT`, per attribute register
  (`HwState::vertex_offset`/`vertex_format`). Offsets are recorded only:
  nothing draws from GPU-side arrays, so `DRAW_ARRAYS` and the
  `ARRAY_ELEMENT` methods still stop as unknown;
- `0x1D8C`/`0x1D90` the clear values, which are also the operands of
  software methods.

`NO_OPERATION` (`0x0100`) with a non-zero parameter is a software method:
the GPU interrupts and D3D's handler (`0x3ff240`) dispatches on the low
five bits through the table at `0x3ff454`. The drain handles:

- 6 and 7, a callback (`0x3ff3c5`: `function(context)`, cdecl, with the
  two clear values as operands). See "Push-buffer callbacks".
- 11, which does nothing in the original either.

The others (flip bookkeeping, events, register writes) only come from
D3D's own code, which the overrides replace; they stop with a named
error.

## Vertical blank

Halo stores a callback at device `+0x1DB8` (`0x12b093`, the inlined
`SetVerticalBlankCallback`). On the Xbox the GPU interrupt calls it once
per vblank as `cdecl callback(D3DVBLANKDATA *)`, where `D3DVBLANKDATA` is
`{VBlank, Swap, Flags}` (`Flags` 1 = `D3DVBLANK_SWAPDONE`).

- Halo's callback `0x12b2a0` calls `0x14280`. That function increments
  the 64-bit counter at `0x485AB0`, and when `Swap` changed (compared with
  `[0x55E6B8]`) it records the counter and the swap at
  `0x485AB8`/`0x55E6B8`.
- The frame wait `0x12b2e0` spins at `0x12b330..0x12b342` until the
  counter reaches `[0x4E6398] - 1`. Nothing but the callback advances it,
  so without an interrupt the spin never ends.

`Device::vblank` counts a vblank and builds the record. Frame time is
built from the count: `0x12b0e0` divides the vblanks since the last frame
by the refresh word at `0x485AC0` (60 when unset), and that becomes the
dt the tick budget `0x146660` and so physics and script sleeps get. So
vblanks have to follow the kernel's logical clock, or the game's time
and the kernel's (KeTickCount, timers) drift apart.

**On the logical clock** (the faithful delivery). `d3d::vblank::clocked_vblanks(device)`
is a handler for a kernel periodic source (`kernel::periodic`, see
crates/kernel/README.md "Periodic host interrupts and pacing"):

```rust,ignore
kernel.add_periodic(kernel::periodic::Source {
    name: "vblank",
    rate: kernel::periodic::Rate::HZ_60,
    max_catch_up: 4,
    handler: Box::new(d3d::vblank::clocked_vblanks(device.clone())),
})?;
d3d::register_frame_wait(&mut dispatch, device.clone(), FrameWait::HALO2_CLOCKED, hook);
```

- At each 60 Hz deadline the kernel runs it the way the GPU interrupt's
  DPC would: between thread slices, on the kernel DPC stack at
  DISPATCH_LEVEL. Periods owed after a clock jump come four a drain
  (`max_catch_up`), each with its own callback, so Halo's counter, which
  only counts calls, keeps up with the clock. It counts the vblank (plus
  any periods dropped from a debt over ten seconds,
  `Device::skip_vblanks`), writes
  `{VBlank, Swap, Flags}` into the kernel's scratch area on that stack and
  returns the cdecl call `callback(&record)`. No device or no callback:
  counted, nothing called.
- A guest spinning on the counter (the original `0x12b330` loop, with no
  override) sees it advance once its own charged work has carried the
  clock to the deadline: exact, but a period's worth of work (16.7 million
  charged accesses per vblank).
- `FrameWait::HALO2_CLOCKED` (`Delivery::Clock`) makes the override wait
  instead: until the counter reaches `[0x4E6398] - 1` it calls the
  kernel's `NtYieldExecution` (through its import token), so other threads
  run, or with nothing else runnable the clock jumps straight to the next
  vblank. Exit effects are the original's (below). It stops with a named
  error when no callback is registered, when the target is more than
  `max_vblanks` ahead, or when no vblank comes (no source installed):
  after `vblank::MAX_IDLE_YIELDS` (100000) yields in a row with the
  logical clock standing still, or after `vblank::MAX_VBLANK_GAP` (2 s) of
  logical time. The watchdog reads the clock through
  `Runtime::guest_time`, so other threads working their way to the next
  deadline (a tick a round, say) never trip it.
- `set_vblanks_per_swap` stays 0 with a clocked source, or vblanks would
  be counted twice.
- Halo registers no swap callback: in device `+0x1DA0..+0x1E00` game code
  touches only `+0x1DB8`. A swap reaches the game through the next
  record: `Swap` is the swap count and `Flags` has `D3DVBLANK_SWAPDONE`,
  which `0x14280` records.

Without a kernel clock, `src/vblank.rs` calls the guest at a safe point
that is always an override's return, never mid-instruction. No logical
time passes for these vblanks:

- **After `Swap`** (installed by `d3d::register`). `Swap` counts
  `Device::set_vblanks_per_swap(n)` vblanks (default 0, which leaves boot
  unchanged). When the `Swap` override returns, the glue calls the
  callback once for each:
  - the record goes below the caller's `ESP` and its address is pushed as
    the cdecl argument;
  - the callback is entered through `ExecCtx::call_guest`;
  - after the last one, every register is restored to its value at
    `Swap`'s return (`EAX` = the swap count) and execution returns to
    `Swap`'s caller.

  With `n` at least the frame interval Halo waits for, the spin falls
  through at once.
- **In the frame wait** (opt-in: `d3d::register_frame_wait(&mut dispatch,
  device, FrameWait::HALO2, hook)`, `Delivery::Inline`). An override of `0x12b2e0` delivers
  callbacks until the counter reaches the target, optionally paced
  (`FrameWait::pace`, for example `FrameWait::NTSC_VBLANK`). It then has
  the original's exit effects:
  - `[0x4E63A8] = counter at entry + 1`;
  - `[0x4E63B0] = n = counter + 1`;
  - `EAX = 0` if `n - target < 0`, else `min(n - target, 0x7FFF)`;
  - `ECX` = the high word of `n - target`; `EDX` = the counter's low
    word; `EBX`, `ESI`, `EDI` and `EBP` preserved;
  - byte `[0x4E63BC] = (word [0x4E63B8] > 0 && AX == 0)`.

  It stops with a named error instead of spinning forever when no
  callback is registered, when a callback returns without advancing the
  counter, or after `max_vblanks` (600) in one wait.

Tests: `tests/vblank.rs` (through `runtime::Runner`, with Rust stand-ins
for the callback and, for the clocked wait, for NtYieldExecution);
`tests/vblank_clock.rs` (the clocked source and frame wait under
`kernel::Machine`, the clock ending exactly on the fourth vblank); the
kernel's `tests/preempt.rs` (generated code spinning on a vblank counter
and on a swap) and `tests/pacing.rs`.

The conventions in `abi.rs` are marked TODO-register. Each one should get a
differential check, running the original under the interpreter against the
adapter, at its recorded call sites before it is trusted.

## GPU model: fences and the busy check

The device executes the push buffer synchronously: every entry drains
what was queued before it does anything else. So the GPU is idle whenever
guest code can look at it, and its time words say so.

- **Words.** Device `+0x2C` is the GPU time, the stamp D3D puts in a
  resource's `Lock` word (`+8`) when the GPU last uses it. `[+0x30]` points
  at the word the GPU writes each completed fence time to (on the Xbox a
  semaphore release, method `0x1D70`, pushed by `D3D_SetFence`).
  `D3D_BlockOnTime` (`0x3fa977`) and the busy check (`0x3fb770`) read it.
- **Here.** CreateDevice points `+0x30` at a word in the scratch range
  (`GPU_COMPLETED_OFFSET`). `SetFence` drains the queue, bumps the fence
  and writes it to both words: everything before the fence has executed
  by the time it is returned. `Device::gpu_completed` is that time.
  Retail `D3D_SetFence` (`0x3fa69e..0x3fa73b`) instead advances `+0x2C`
  by 2 and returns the old time, so fence and `Lock` values here are not
  retail's numbers; only their order is. Nothing compares them across
  that difference: every fence has completed when it is handed out.
  Releasing a binding stamps `Lock` with the current fence.
- **The busy check** (`0x3fb710`, `D3DResource_IsBusy` inlined; `ECX` =
  resource, `EAX` = 0 or 1) runs as compiled, after draining the queue
  and writing render-to-texture targets back (lazily only those of the
  resource):
  1. Lock bits: busy while the resource holds an internal (binding)
     reference, `Common & 0x780000`. A surface (`Common & 0x70000 ==
     0x50000`) with a parent texture (`+0x14`) is busy if either holds
     one, and the parent is tested from then on.
  2. Fence: with `Lock` non-zero, busy while `GPU_TIME - Lock < GPU_TIME
     - completed` (unsigned), i.e. the last use is newer than the last
     completed fence.
  3. Otherwise not busy, and the tested resource's `Lock` is cleared
     (`0x3fb78d`).

  Against this model the fence test only reports busy for a `Lock` newer
  than every fence the device has issued, which the device never stamps.
  Halo's own inline wait (`0x1d434`) passes the GPU time to
  `BlockOnTime`, which returns at once for the same reason.

## NV2A register window

The original CreateDevice stores the NV2A register base (`0xFD000000`) at
device `+0x934` (`0x3fc31d`). Game code reads it back and reads GPU
registers through it, at 17 sites of one shape: `mov edx, [D3D__pDevice]`,
`call 0x3fac30` (KickOff, which preserves `EDX`), `mov eax, [edx+0x934]`,
then `mov ecx, [eax+0x3240]`, `cmp ecx, [eax+0x3244]` and, when they are
equal, a read of `[eax+0x400700]`. The sites sit in wait loops (a timer
compare, KickOff, the reads, a call, a jump back); the values only decide
whether the PGRAPH read happens.

| site | |
|---|---|
| game | `0x6a932`, `0x80eca`, `0x8638c`, `0x86448`, `0x8c892`, `0x12429d`, `0x12d0cf`, `0x12d4ee`, `0x12e0d6`, `0x12e15b`, `0x146f19`, `0x1561f6`, `0x18f31a`, `0x1941f2`, `0x194362`, `0x199646`, `0x1997d2` |
| D3D | `0x3fa8b5` (MakeRequestedSpace helper), `0x3fa904` (BlockOnTime helper), `0x3fac6a` (KickOff), `0x3fad10` (a swap callback CreateDevice installs), `0x3fc3f2` (CreateDevice) |

The D3D sites also read `0x400B10`, and run only under overrides. No site
writes the window. The miniport (`0x3fe005` and the interrupt paths)
reaches the NV2A through its own base, never through `+0x934`; those
functions are overridden (see "Port I/O guards").

**Registers** (`d3d::regs::reg`):

| offset | register | value |
|---|---|---|
| `0x3240` | `NV_PFIFO_CACHE1_DMA_PUT` | the drained push-buffer position, `& 0x0FFFFFFF` (D3D's encoding, `0x3fad07`) |
| `0x3244` | `NV_PFIFO_CACHE1_DMA_GET` | the same: the FIFO has fetched everything |
| `0x400700` | `NV_PGRAPH_STATUS` | 0: idle |
| `0x400B10` | the PGRAPH register D3D's fence word lands in | `(at << 5) \| (fence & 0x1F) << 2 \| ([device+0x40] & 3)` |

`D3D_SetFence` (`0x3fa6a1`) pushes that fence word with method `0x0310`
on subchannel 5, `at` being the push-buffer address of the fence.
`BlockOnTime` waits until bits 6:2 match the completion word
(`0x3fa910`), and `MakeRequestedSpace` reads bits 31:7 back as the address
the GPU has fetched to (`0x3fa8bb`). Here `fence` is the device's last
fence (the completion word) and `at` the drained position when it was
issued, so both checks see the GPU caught up. No vblank state is read
through `+0x934`.

**Backing.** The runtime keeps the real aperture free of RAM and has no
MMIO handlers (an access is an unsupported-device fault, by design; see
`docs/lifter/LIFTER.md`). Adding a device hook to `Guest`'s access path would
widen that contract for four read-only words, so the window is a
documented substitute instead, with no runtime change:

- The host picks the base (`DeviceConfig::nv2a_window`), page-aligned with
  room for the 16 MB aperture. Only the PFIFO and PGRAPH pages are mapped,
  as ordinary RAM, so any other register page faults, naming its address.
- `Device::sync_registers` writes the words at CreateDevice, KickOff
  (every game site reads right after one), SetFence and Swap. Between
  those they hold the last values, which still describe an idle GPU.
- At each of those points the device compares both pages with what it
  last wrote. A guest store is reported as `Event::RegisterWrite { offset,
  value }` and undone.
- With a runtime MMIO hook the same `RegisterValues` could answer reads at
  `0xFD000000` directly; nothing else would change.

**Tests** (`tests/round6.rs`):

- Synthetic: the field and the words after CreateDevice, KickOff and
  SetFence; logged and undone stores; release and re-create; a misaligned
  base; the window as mapped runtime memory read through `runtime::Runner`.
- Ignored, needing `$H2_XBE`:
  - `every_0x934_site_reads_only_modelled_registers` decodes the image
    with iced-x86 over the `analyze` function map. It finds every
    `[reg+0x934]` load whose base came from `D3D__pDevice` (and every one
    in the D3D section), and follows the loaded register to its
    accesses. It checks the 17 game sites and their three reads, that the
    D3D sites run only under overrides, and that every game-code access
    at a modelled offset belongs to a site.
  - `game_sites_find_an_idle_gpu_through_lifted_code` loads the image
    into the runtime with the overrides and a window and runs
    CreateDevice through the runner. It then interprets each game site's
    lifted IR from its `D3D__pDevice` load, with the KickOff call going to
    the override through the runner. Every register read must execute
    (including the PGRAPH one) without a fault.

## Push-buffer callbacks

Halo inlines one `D3DDevice_InsertCallback` (`0x2460e`, in `0x245c0`,
reached from view rendering through `0x13870`): `SET_ZSTENCIL_CLEAR_VALUE`
= `0x24970`, `SET_COLOR_CLEAR_VALUE` = a context, `WAIT_FOR_IDLE`, then
`NO_OPERATION` 7. On the Xbox the GPU interrupts when it reaches the
method and D3D calls `0x24970(context)` (cdecl). The callback reads a
render target back, so it must run after the work queued before it and
before the work queued after it.

- `Device::drain_until_callback` drains up to and including the first
  callback method and returns it as a `PushCallback` (`function`,
  `context`, `kind` 6 or 7, `at`).
- The runtime glue calls it before every entry. With a callback pending it
  calls the guest function the way D3D's handler does (the argument just
  below the entry's `ESP`, direction flag clear, through
  `ExecCtx::call_guest`), then restores every register and jumps to the
  entry again, which drains on (more callbacks are delivered the same way)
  and runs (`runtime_glue::deliver_push_callback`).
- Draining outside the glue (`sync_push_buffer`, for example a test
  calling the device directly) cannot call the guest: a callback there is
  a named error that latches the device.
- Render-to-texture targets drawn into are written back to guest memory
  before the callback runs: Halo's `0x24970` reads a target's texels
  (through `0x3fdbc0` and `0x24a70`). If the write-back fails, the entry
  stops with that error, unlatched like any other write-back failure; the
  callback is kept, and the next drain retries the write-back and delivers
  it before anything queued after it.
- D3D is not callable from a callback (on the Xbox they run at interrupt
  time). While one is being delivered (`Device::begin_push_callback` ..
  `end_push_callback`), any D3D entry stops with a named error and drains
  nothing, so the work queued after the callback, and the next callback,
  wait until it returns. Halo's `0x24970` only calls the native helper
  `0x3fdbc0`.
- Abandoning a callback is not supported. If one never returns to the
  glue (an unwind past it, a thread that ends inside it), the mark stays
  set and every later D3D call stops with the same error.

Tests: `tests/round5.rs` (`through_the_runtime`, with a Rust stand-in for
`0x24970`).

## Port I/O guards

Three D3D-internal functions execute port I/O on the GPIO port `0x80C0`
(the encoder's field bit): `out` at `0x3fe131`, `in` at `0x3feb3c` and
`0x3fee0d`. No game code calls them. They are reached only from the
original CreateDevice (`0x3fbc10 -> 0x3fe005`), the teardown and Reset
paths (`0x3fc530`, `0x3f77c0` through `0x3fe4cb`, `0x3fe67f`, `0x3fe86a`),
and the GPU interrupt service routine and DPC that `0x3fe005` connects.
All of those are overridden, but the three are overridden too
(`d3d::abi::PORT_IO`) so generated code can never execute the port
instructions:

| address | function | override |
|---|---|---|
| `0x3fe005` | miniport hardware init (`EAX` = miniport): connects the interrupt (`0x3fe0a4`, ISR `0x3feac0`, DPC `0x3febe0`), maps the NV2A, writes GPIO | stops |
| `0x3feac0` | GPU interrupt service routine, stdcall `(Interrupt, ServiceContext)`, `ret 8`, `AL` = handled | `AL` = FALSE (the device raises no interrupts; the original's answer when nothing is pending, `0x3febd0`) |
| `0x3fed90` | vertical-blank service (`EAX` = miniport): field bit, flip, vblank callback; from the DPC and the flip paths | stops (vblanks come from `Device::vblank`, see "Vertical blank") |

These three instructions (`in` x2, `out` x1) are the only ones `h2 lift
--stats` still reports as unlowered (18,685 of 18,688 functions fully
lowered; the other three are these functions). That is deliberate: the
lifter has no port-I/O model, and a guessed open-bus read would hide a stray
call behind a plausible value, where the unsupported stop names it.

The ignored test in `tests/round5.rs` decodes the retail image
(`$H2_XBE`) to check that each port instruction lies in the function its
guard overrides, after `mov dx, 80C0h`.

## Boot path

The original CreateDevice reaches the NV2A through `0x3fbc10 -> 0x3fe005
-> 0x3fe165` (the MMIO read at `0x3fe16b`). With the overrides installed,
Halo's boot reaches D3D in this order (static call graph from the XBE
entry; `main` is `0x12190`):

1. `0x12420`: CreateDevice (`0x12490`, 640x480, BehaviorFlags `0x40`),
   `Swap(0)`, then the inlined Release: device `+0x938` is 1, so the
   teardown `0x3fc530` (one internal reference off every bound texture,
   palette and stream buffer, `D3D__Streams` zeroed, every host surface
   destroyed), after which the game zeroes the device and
   `D3D__pDevice`.
2. `0x12560` (from `0x12000`): CreateDevice again through `0x128c0`
   (Reset `0x3f77c0` only if the game's device pointer is still set, which
   it is not after step 1), SetPalette x4, SetShaderConstantMode(`0x11`),
   ZEnable, a run of Simple states, ZBias, SetTextureState_TexCoordIndex x4, then
   `0x14850`, which builds two 64x64 offscreen targets with the native
   helpers `0x3fdbc0` and `0x3fb7a0`.
3. Nothing else under `0x12000` reaches a stop.

From the main loop (`0x12b690`) on, the first stops a frame can reach are
listed under "Stops on the way to the menu" below.

### Stops on the way to the menu

This list is from static analysis; the boot (docs/runtime/BOOT.md) now runs past
all of it to the menu and into the first level. Conditional stops that normal
view rendering (`0x223240 -> 0x2232a0 -> 0x2c8a0 -> 0x132d0`) can reach:

Of the 18 entries that stopped before, 1 remains (`abi::UNIMPLEMENTED`).
The fragments, the display entries, `SetStipple` (`0x3f8cc0`), the busy
check (`0x3fb710`), `SetVertexShaderConstantNotInline` (`0x3f7410`) and
Get2DSurfaceDesc are done (see the inventory), and so are the cases
below:

- **Viewport.** `0x14bc0` selects a render target by index and calls
  SetViewport with its extents. A smaller viewport (split screen,
  letterboxing, the offscreen targets) now gives each draw and each
  Count=0 clear a clip rectangle, which `GfxBackend` applies as a
  scissor. Partial clears render through `clear_rects`.
- **Offscreen targets.** The `0x14850` targets are texture levels
  (`Parent` set by `0x3fb7a0`). They become render-to-texture targets
  sampled in place and written back lazily. `0x14bc0`'s Get2DSurfaceDesc (`0x3fdb20`) runs
  natively.
- **Channels.** The wrapper `0x14b60` and the alpha-only clears
  (`0x13c3f`, when `0x4BA04C > 1`; `0x3c515`, `EAX = 0x10`, in `0x3c3c0`
  from `0x2bcd0`) go through `clear_rects` with an alpha-only mask.
  Callers that pass a register (`0x132d0`, `0x2bcd0`, `0x3fdf0`,
  `0x41cc0`, `0x44370`) are covered the same way.
- **Frame wait.** `0x12b2e0` spins forever without vblanks (see "Vertical
  blank"; with a clocked source it ends at the next vblank).
- **Inline methods.** The view-rendering paths push the stipple pattern
  (`0x1e4ae`), an `InsertCallback` (`0x2460e`, see "Push-buffer
  callbacks") and the vertex-cache break (`0x43a1a`); all three drain now.
  The inline `BeginVisibilityTest` (`0x17C8`, `0x20cec`) drains too (see
  "Visibility tests").

- **Inline arrays.** `0x4b220`'s draw through BeginPush and SetStateVB
  (both native) drains: `INLINE_ARRAY` (`0x1818`) words between
  `BEGIN_END` words collect in the immediate-mode state
  (`ImmediateDraw::inline`, at most `pushbuf::MAX_INLINE_WORDS` per
  primitive, never mixed with vertex-data writes), and the `End` draws
  them against the pushed state. The vertex is unpacked from the
  `SET_VERTEX_DATA_ARRAY_FORMAT` registers as the NV2A does it (the xemu
  model): enabled registers in register order, each aligned to its
  component size and packed with no gaps; the format's stride field is not
  used. A trailing partial vertex is dropped (64 bytes of 20-byte
  vertices draw 3), as xemu's `pgraph_gl_bind_inline_array` counts whole
  vertices. A format with no vertex type, or one with every register
  disabled, stops. After the draw the last vertex's values stay in the
  attribute registers (xemu `bind_vertex_attributes` does the same), so a
  later draw that leaves a register disabled reads them as its constant.
  Programs get the registers as attributes; an FVF shader draws fixed
  function, pre-transformed for `XYZRHW`. Counted in
  `Stats::inline_array_draws`.
- **Same-size targets.** `0x3f7b30` is implemented (see the inventory
  notes). Tests for both, the commit's kept bits and the internal
  references: `tests/round6.rs`.

- **Visibility tests** (`0x3f85d0`, `0x3f8630`, the lens-flare pass
  `0x20bb0`): implemented with counting queries (see "Visibility tests").
- **Resource destruction** (`0x3fb520`): the device does its part and the
  original runs after the entry (see "Resource destruction").
- **Gamma ramp** (`0x3ff5c6`, from the brightness update at `0x12d08`):
  implemented (see "Gamma ramp").

The one that still stops:

- `0x3f77c0` Reset. It is reached only if the game's device pointer is
  still set at `0x128c0`; it is not, after boot step 1.

It is not on the static boot path above. The port-I/O guards that stop
(see "Port I/O guards") are not reached from game code.

## Visibility tests

Halo's one user is the lens-flare/sun visibility pass (`0x20bb0`). Per
query it pushes `CLEAR_REPORT_VALUE` 1 and `SET_ZPASS_PIXEL_COUNT_ENABLE`
1 inline (`0x20cec`), draws through a callback that returns the pixel
count it expects, and calls `EndVisibilityTest` (`0x20d0f`). The index
rotates through three sets (`frame % 3`; object `i` of set `s` is
`s + 3i`), and a bit per slot (`0x5233F0`, set at `0x20d51`) records that
a test was ended there. `GetVisibilityTestResult` has two callers:

- **The reader** (`0x207b0`, call at `0x20840`) reads set
  `(frame + 1) % 3`, the tests ended two frames before, divides the count
  by the expected one, clamps to `[0, 1]` (`0x20861`) and smooths the
  object's visibility byte towards it. On `D3DERR_TESTINCOMPLETE` (any
  non-zero `EAX`, `0x20847`) it skips the object, which keeps last
  frame's value.
- **The writer** (`0x20bb0`, call at `0x20cbb`), before reusing a slot
  that had a test (set `frame % 3`, ended three frames before), asks for
  its result and discards the count. On `D3DERR_TESTINCOMPLETE`
  (`0x20cc4`) it skips the object for the frame: no new test, the old
  one stays.

Neither retries: both jump forward to the next object (`0x2096d`,
`0x20d5e`), so an incomplete answer costs one frame of staleness for one
flare, never a spin. A constant answer would be wrong somewhere, so the
counts are real. The ignored `visibility_conventions_in_the_image` test
checks these branches.

- **Retail.** `EndVisibilityTest` stamps report slot `index & 0xFF` of page
  `index >> 8` (64 pages of 256 16-byte slots at device `+0x7D4`) with the
  GPU time and pushes `SET_ZPASS_PIXEL_COUNT_ENABLE` 0 and `GET_REPORT`.
  `GetVisibilityTestResult` returns `D3DERR_TESTINCOMPLETE` (`0x88760828`)
  while the slot's status word is `0xFFFFFFFF`, writing nothing, after a
  `D3D_SetFence(0)` when the GPU time (`+0x2C`) has not moved since the
  end (`0x3f8659..0x3f8660`); otherwise it copies out the count and
  timestamp. The pages are D3D-private: only `0x3f8570`,
  `0x3f8630` and the teardown (`0x3fc69d`) touch `+0x7D4`. The device
  keeps the results itself, by index, and allocates no guest pages.
- **Counting.** While `SET_ZPASS_PIXEL_COUNT_ENABLE` is on
  (`HwState::zpass_count_enable`), every draw is a counted draw
  (`gfx::Draw::count_samples`). `CLEAR_REPORT_VALUE` type 1 calls
  `Backend::clear_pixel_count`, and `EndVisibilityTest` drains, takes a
  ticket from `Backend::report_pixel_count` and turns counting off. Any
  other report type stops. A backend that cannot count stops at the end.
- **Polled results (the default).** `GetVisibilityTestResult` asks the
  backend without waiting (`Backend::try_pixel_count`). A count that is
  back is answered and kept, so asking again answers the same. One that
  is not, for a test ended at most `VISIBILITY_PENDING_SWAPS` (2) `Swap`s
  before, answers `D3DERR_TESTINCOMPLETE` with the original's fence kick
  (`Stats::visibility_incomplete` counts them). An older test waits for
  its count (`Backend::read_pixel_count`), so a test is never left
  incomplete past the point where the render thread's two-frame limit
  guarantees it has been reached. In Halo's terms: the reader (two
  frames) may see incomplete and keep last frame's value, the writer
  (three frames) never does. Which calls answer incomplete depends on
  host timing; the hardware's answers depend on the GPU's the same way.
  Before this, the device waited for every count, and with the render
  thread that meant waiting for everything queued: 8.3% of the game
  thread in Old Mombasa (profile at `183781e`).
- **Waited results** (`H2_D3D_VISIBILITY_POLL=0`,
  `Device::set_visibility_poll(false)`): every call waits for the count
  and `D3DERR_TESTINCOMPLETE` is never returned, so the answers are the
  same whatever the host's timing. That is what the hardware answers once
  caught up.
- **The render thread** (`ThreadedBackend`) lands counts on the guest's
  side (`Waits::landed_counts`): after each batch and each present it
  asks the inner backend for the reports in order, up to the first not
  back (`GfxBackend` maps a report the first time it is asked, and at
  each `Swap`). Its `try_pixel_count` answers from those; a miss
  (`Waits::pending_counts`) answers at once and hands the pending batch
  over with a kick, so the count lands at that batch's end. Its
  `read_pixel_count` answers a landed count at once and otherwise waits
  for everything queued before it, as before.
- An index never ended, or at or above `0x4000` (`VISIBILITY_INDICES`), is
  an invalid call. Ending an index again replaces its result. Teardown
  forgets every test. Counted in `Stats::visibility_tests`.
- **The count in gfx** (`gfx::occlusion`). wgpu occlusion queries are
  boolean on Metal, so they cannot give a count. Instead each counted draw
  is drawn a second time first, into an `R16Float` target with additive
  blending and a fragment shader that writes 1, against a scratch copy of
  the depth/stencil attachment with the draw's full depth/stencil state,
  writes included. Overlapping triangles of one draw are then rejected
  in the count as in the real draw, and the real attachment is written
  once. The depth texture needs `COPY_SRC` (`SurfaceStore` depth surfaces
  have it; `DrawError::CountDepthNotCopyable` otherwise). Replacing a
  registered shader drops the cached count pipelines along with the
  others. A compute pass then adds up the texels
  inside the draw's scissor into a `u32` (`PixelCounter`). With
  multisampling it counts samples, as the NV2A does. W-buffered draws
  (all of Halo's 3D, `ZEnable(2)`) store w from the fragment stage, so a
  W-buffered program's count pass runs the program's own count entry
  (`shaders::psh::COUNT_DEPTH_ENTRY`, `ShaderProgram::count_entry`),
  which writes the same depth; until it did, the count tested the
  rasterized z against stored w and every flare in front of geometry
  counted 0 (see "Lens flares" below). Limit: the count
  pass does not run the draw's own fragment program, so alpha test and
  `discard` do not reject samples there. The flare's occlusion draws do
  not depend on them.
- Tests: `tests/visibility_async.rs` (Halo's reader and writer against a
  synchronous backend, a caught-up, a lagging and a held render thread,
  the incomplete path through the ABI; `cargo bench -p d3d --bench
  visibility_wait` prints the guest's time in the call with a lagging
  render thread, waiting and polling), `tests/round7.rs` (device and
  runner) and `gfx`'s
  `pixel_counter_*` tests (within-draw depth and stencil at 1x and 4x, a
  replaced shader, a depth that cannot be copied). The ignored `visibility_conventions`
  checks the call sites and conventions in the image.

### Lens flares

The visibility tests feed lens flares, and through them some scripted
lights. Static trace, all addresses guest VAs:

- A `lens` attachment on an object (definition `+0x94`, classes `lens`,
  `MGS2`, `tdtl`) is not instantiated by the attachment constructor; the
  object render walk `0x41c80 → 0x41c20 → 0x41980` submits it every frame
  at its marker through `0x2dba0`. Lights (`ligh +0x94`, `0xc3340`), the
  sky's sun (`0x3eec0`) and effect parts (`0x179d23`) submit theirs the
  same way.
- `0x2dba0` registers the flare in the visibility array at `0x51f40c`
  (32-byte entries, `0x209b0`). The test pass `0x2dd30` (called from
  `0x2c2e4`) sets `ZEnable(2)`, colour writes off, `LESSEQUAL` without
  depth writes, and runs `0x20bb0` with the callback `0x2e540`: a quad of
  half-size `[2, 128]` pixels around the projected flare, pushed inline
  (`BEGIN_END` quads, four `VERTEX_DATA4F` on register 0) through a game
  vertex program, its expected count the quad's area.
- The reader `0x20770` divides the landed count by that area, clamps it to
  `[0, 1]`, scales it to a byte (x256, at most 255) and smooths it into
  the entry's visibility byte. The draw pass `0x2df00` (from `0x13b7e`,
  callback `0x2e9e0`) draws the flares; that it scales them by the byte is
  inference (the callback is not traced in detail).

So a count of 0 is an invisible flare. Under the W-buffer the quad's
rasterized z is about 1 (or 0.5 with depth clamp), above every stored w
except the cleared far plane, so before the count entry only flares seen
against the sky showed. The Armory's look test lights the top and bottom
lights by creating `looker_light_top_red` / `looker_light_bottom_red`
(`test_mindread_up`/`_down` in `01a_tutorial`'s scripts, as the sergeant
asks): each is a model-less `mach` (tag `ss_look_device\looker_light_red`)
whose one attachment is that `lens`.
In front of the ring the flare counted 0 and the light never lit. The
script's look gates do not depend on it: they pass on
`objects_can_see_object` or `player0_looking_up`/`_down`, so the test
could be passed in the dark. Test: `shaders_render.rs`
`w_buffered_visibility_test_counts_against_the_stored_w`.

## Resource destruction

`D3D_DestroyResource` (`0x3fb520`, `EDI` = resource) frees a resource's
guest memory through the game's allocators (`0x2d0a60`, `0x122c0`). Its
D3D callees are the overridden `BlockOnResource` and `0x3fb4d0`, an unlink
from the list at `0x408644` that nothing else names, so the list is always
empty. `Release` (`0x3fb640`) and the internal release `0x3fb5a0` (target
switches, unbinds and teardown) call it. The other two direct calls, game
code at `0x13d24` (in `0x13cd0`) and teardown at `0x3fc611`, release device
`+0x7A0`, which is only ever stored as 0, so they never run.

- The device implements `Release`, `AddRef`, the internal release and the
  teardown as compiled. `Release` of a level releases its parent first,
  and destroys at the last reference when no internal reference remains.
  `AddRef` of a level with refcount 0 adds a reference to its parent.
  `Release` of a refcount-0 resource is an invalid call.
- A destroy (`Device::destroy_resource`) writes back render-to-texture
  pixels, drops the host surfaces of the resource and its levels, purges
  it from the texture cache and queues the header (`Stats::destroys`).
- After the entry returns, the runtime glue (`runtime_glue`) takes the
  queue (`Device::take_pending_destroys`) and calls the original
  `0x3fb520` for each header through `ExecCtx::call_guest`, with `EDI` =
  header. It then restores the entry's registers and delivers any pending
  vblanks. The guest memory is freed by the game's own code, so the
  allocator state stays the game's.
- The original waits on each resource it frees first (`BlockOnResource`,
  `0x3fb537..0x3fb539`, skipped for a surface the title made). After a
  teardown those calls arrive with no device. Teardown drained the push
  buffer and wrote every target back before it queued the resources, so
  `Device::block_on_resource` answers each resource the teardown destroyed
  once with no device; any other wait with no device stops with
  `NoDevice`. CreateDevice forgets the list.
- Tests: `tests/round7.rs` (a runner with a stand-in at `0x3fb520` that
  waits through `BlockOnResource` as the original does) and
  `round6.rs`/`recheck_fixes.rs` (teardown and unbind destroys). The
  ignored `destroy_resource_conventions` checks that `EDI` is the
  resource, the callees, that only `0x3fb4d0` names `0x408644`, and that
  `+0x7A0` is only stored as 0.

## Gamma ramp

`0x3ff5c6` loads a `D3DGAMMARAMP` (768 bytes: 256 red, 256 green, 256
blue) into the miniport. `EAX` = the miniport (device `+0x1C28`) and
`EDX` = the ramp. Halo calls it once, from the brightness update
(`0x12d08`, an inlined `SetGammaRamp`). The device checks `EAX`, copies the
ramp (`Device::gamma_ramp`). Each `Swap` passes the current ramp to the
frame handoff, which applies it per RGB channel when copying the presented
surface. The unmodified surface remains available for guest reads. The
ignored `gamma_ramp_conventions` checks the convention.

## gfx changes this wants

Done since the last revision: depth/stencil surfaces filled from guest
texels (`SurfaceStore::write_depth`) and read back in any `D16`/`D24S8`
layout (`SurfaceStore::read_depth_as`), depth formats decoded as their raw
bits, `SurfaceStore::snapshot` (a sampleable
copy of a colour surface, for the back buffer read as a texture),
`Draw: Clone` (one draw repeated per scissor piece), per-draw scissor and viewport
(`gfx::Draw::scissor`/`viewport`), rectangle and channel-masked clears
(`Renderer::clear_rects`), render-to-texture surfaces
(`SurfaceStore::create_texture_target`), the surface upload
`SurfaceStore::write_color(ctx, id, &TextureDesc, bytes)`, alpha test in
the built-in shader, indexed draws (`gfx::Draw::indices`), and sample
counts for visibility tests (`gfx::occlusion`, `Draw::count_samples`,
`Renderer::render_counted`). Still wanted:

- **Counting through the draw's own fragment program.** The count pass
  writes 1 for every sample that passes depth/stencil, so alpha test and
  `discard` are not applied to the count.

- **Sampling more render targets directly.** Only whole single-level
  `A8R8G8B8`/`LIN_A8R8G8B8` textures read a level in place. Other formats
  (`X8R8G8B8` reads alpha as one; 16-bit formats quantise on the way
  back), mipmapped textures with a level drawn into, and `COLORSIGN`
  stages still go through guest memory; each needs a view or a shader
  path that reads the surface as the upload would.
- **No guest read trap.** Lazy write-back trusts the guest to wait on a
  level (`BlockOnResource`, the busy check, a fence, a callback) before
  its CPU reads the texels, as the Xbox GPU's own asynchronous writes
  require. A read that skips every wait sees the level as of its last
  write-back. A store that skips every wait is merged by page reports'
  byte differences only (above); exact store ranges from the runtime, or a
  trap on the first store to a watched page, would make it exact.
- **Depth as a texture on the GPU.** `D24S8` depth levels are now
  converted on the GPU (`gfx::depth_bits`, above), and so are 32-bit
  colour textures over the depth buffer ("Depth aliases on the GPU",
  below). `D16` levels, and aliases in other formats or over a `D16`
  depth buffer, still go through a CPU round trip: a readback, then a
  decode and an upload. The shadow-map comparison itself is in
  `crates/shaders` (shaders.md, "Shadow maps").
- **Overlapping levels on the GPU.** Two level headers over one
  allocation (Halo's render-target pool in a level: two 160x120 and a
  320x240 `LIN_A8R8G8B8` level over the 512x512 `LIN_D24S8` shadow map)
  used to move pixels through guest memory: binding or sampling one wrote
  every other dirty level over its bytes back first
  (`WritebackCause::Overlap`, a read-back each, about 3 a frame in the
  user's 327 s level run), then refilled it from memory. The guest's CPU
  never reads those bytes in between. Now the backend keeps an *alias
  mirror* (`Backend::mirror_*`; `GfxBackend` a storage buffer,
  `gfx::guest_words`) standing for the pool's memory, and the device
  (`src/device/mirror.rs`) moves the overlapping dirty level into it
  (`mirror_store`: its guest texels, pitch padding as zeros, exactly what
  the write-back would store) instead of writing it back, and fills the
  bound level from it (`mirror_load`: what an upload of those bytes would
  fill). No read-back. The device tracks, per byte, whether the mirror
  holds it newer than guest memory, and memory's byte when the mirror took
  it:
  - a guest store into such a byte wins, as for lazy levels: an exact
    `note_write` range whatever its value, or a byte that differs from the
    one taken (a page report). A store into part of a word flushes that
    range first, so the mirror's newer bytes stay whole words;
  - a draw into a level makes the level newer over its whole range (it
    was filled from the mirror first), so the mirror gives those bytes up;
  - where dirty levels are written back lazily (a wait or busy check on an
    overlapping resource, `BlockOnTime`, push-buffer callbacks, a sample
    through guest memory, a destroy, `PersistDisplay`, teardown), the
    newer bytes over the same span are read back (`mirror_read`) and
    stored into guest memory, guest stores excepted;
  - a `D24S8` depth level is filled from the mirror on the GPU as well
    (`GuestWords::load_depth`: depth `z / 16777215` drawn as `frag_depth`
    with the CPU division's rounding, computed without a division;
    stencil packed by a compute pass and copied into the stencil aspect).
    Until 2026-11-16 it flushed the overlap and was filled from memory:
    in The Heretic that was a read-back, a 1 MiB guest copy and an upload
    every time the shadow map was bound, with the game thread waiting on
    the render thread (about 22% of it in a headless profile of e9be08f).
    `H2_D3D_MIRROR_DEPTH=0` (`Device::set_alias_mirror_depth`) goes back
    to that; a level the mirror cannot fill on the GPU still does.
  Formats other than the 32-bit word formats and `D24S8`, unaligned
  levels, a bound level that is itself dirty, overlapping dirty levels and
  guest stores waiting to be merged into the moved level take the
  write-back as before. `Stats::mirror_stores`, `mirror_loads` and
  `mirror_flushes` count it. `H2_D3D_ALIAS_MIRROR=0`
  (`Device::set_alias_mirror`) turns it off.
  `crates/d3d/tests/alias_mirror.rs` runs a five-level pool (two pitches,
  a partial overlap, a swizzled alias, `X8` with padding, a depth level)
  through draws, in-place and through-memory samples, guest stores before
  and after the mirror took the bytes (whole words and part of one), a
  `BlockOnResource` and a final flush, with tracked and verified uploads,
  on the device's thread and through the render thread, and compares the
  back buffer and guest memory with the write-back path byte for byte.
  `a_depth_level_fills_from_the_mirror_as_from_memory` does the same for
  a shadow-map pool (a depth level under three colour levels, a
  depth-tested pass that reads what the colour levels left, a guest store
  between frames) with the depth level filled through memory and from the
  mirror, and checks that the second reads nothing back;
  `crates/gfx/tests/guest_words.rs` checks `load_depth` against
  `write_depth` bit for bit. The per-byte state is bytes, scanned eight at
  a time, and guest memory is compared with what the mirror took only on
  pages written since. `bench_shadow_map_binding` (same file, ignored)
  binds a 512x512 shadow map under the Heretic's 320x240 and 160x120
  levels: through the render thread, 4.9 ms per binding with the depth
  level through memory and the old per-byte scans, 0.37 ms now. A
  sampled level is now picked by matching format, size and layout first,
  then by being drawn into, then by header (it was the map's order: two
  headers over the same bytes could pick the wrong one and go through
  guest memory).
- **Depth aliases on the GPU.** A texture over the depth buffer's memory
  (`0x509368` `LIN_A8R8G8B8`, `0x50936C` `LIN_A8B8G8R8`: the menu's fog
  pass, one a frame) was a `read_depth` of the whole buffer, a CPU decode
  and an upload: the guest (or, with the render thread, the render
  thread) waited for the whole queue once a frame. `GfxBackend` now
  answers `Backend::defers_depth_aliases` and makes the texture in a
  compute pass (`gfx::guest_words::GuestWords::depth_alias`): each texel's
  byte address (offset, then the texture's pitch or swizzle) picks a word
  of the `LIN_D24S8` buffer, the word is computed from the depth and
  stencil aspects with `encode_depth`'s rounding, and its bytes become the
  texel's channels as `decode_texture` orders them for the format, with
  `COLORSIGN` channels biased. Covered: `A8R8G8B8`, `X8R8G8B8`,
  `A8B8G8R8`, `B8G8R8A8`, `R8G8B8A8`, linear (pitch a multiple of 4) or
  swizzled, at a 4-byte-aligned offset, over a single-sample
  `Depth32FloatStencil8` buffer. Anything else is read back once per
  depth generation and decoded on the CPU inside the backend, as the
  device did. `crates/gfx/tests/guest_words.rs` checks the pass against
  `read_depth_as` plus `decode_texture` byte for byte, and
  `crates/d3d/tests/gpu_depth_alias.rs` draws seven alias layouts with
  three sign masks over three depth contents with the pass on and off,
  on the device's thread and through the render thread, and compares
  the back buffers byte for byte. `H2_D3D_GPU_DEPTH_ALIAS=0`
  (`GfxBackend::set_gpu_depth_aliases`) goes back to the device's
  read-back.
- **Public `HostPixelOrder` from `gfx::surface`.** It is only reachable as
  `gfx::encode::HostPixelOrder`.
