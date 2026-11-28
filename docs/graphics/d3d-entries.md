# D3D entries reached from game code

Status: Built. Reference for the D3D functions game code calls, as compiled in the retail image, and how `render_bridge::guest_entries` answers each.
Code: `crates/rewrites/render-bridge/src/guest_entries.rs` (`ENTRIES`, `answer`), `crates/rewrites/render-bridge/src/device_state.rs`. Tests: the modules' own.
Switch: none. Part of [d3d.md](d3d.md).

## Inventory: entries reached from game code

"Calls / caller fns" counts direct `call`/`jmp` instructions from outside
the `D3D` section and the distinct functions they sit in. "Matched" says
whether a signature identified the function. Unmatched ones are named by
what they do; names with an address in them are descriptive only. Status is
"store" (the setter's state slot is written, nothing pushed; see
[Render-state and texture-state setters](#render-state-and-texture-state-setters)),
"seal" (`Swap`: the frame step's seal, `frame_wait::swapped`), "answer"
(answers with no device: resources, fences, `KickOff`, the teardown,
`PersistDisplay`), "gap" (panics with its name and caller; its callers'
draws belong to native stages) or "native" (not overridden; the original
runs).

The callers that still reach a gap are in
[notes/2026-11-28-d3d-live-callers.md](notes/2026-11-28-d3d-live-callers.md).
The notes under the table record what each retail body does.

| address | entry | matched | convention (as compiled) | calls / caller fns | status |
|---|---|---|---|---|---|
| `0x3f5240` | `Direct3D_CreateDevice` | yes | EAX = `BehaviorFlags`, ECX = ppDevice, [esp+4] = pPresentationParameters; ret 4 | 2 / 2 | answer |
| `0x3f52f0` | `D3DDevice_SetRenderState_EdgeAntiAlias` | yes | stdcall(Value); ret 4; guest slot 151 | 9 / 4 | store |
| `0x3f5330` | `D3DDevice_SetRenderState_ShadowFunc` | yes | stdcall(Value); ret 4; guest slot 156 | 11 / 6 | store |
| `0x3f5370` | `D3DDevice_SetRenderState_FogColor` | yes | stdcall(Value); ret 4; guest slot 138 | 11 / 6 | store |
| `0x3f53d0` | `D3DDevice_SetRenderState_CullMode` | yes | stdcall(Value); ret 4; guest slot 147 | 58 / 48 | store |
| `0x3f5450` | `D3DDevice_SetRenderState_FrontFace` | yes | stdcall(Value); ret 4; guest slot 146 | 10 / 5 | store |
| `0x3f54a0` | `D3DDevice_SetRenderState_NormalizeNormals` | yes | stdcall(Value); ret 4; guest slot 142 | 9 / 4 | store |
| `0x3f54f0` | `D3DDevice_SetRenderState_ZEnable` | yes | stdcall(Value); ret 4; guest slot 143 | 30 / 24 | store |
| `0x3f5590` | `D3DDevice_SetRenderState_StencilEnable` | no | stdcall(Value); ret 4; guest slot 144 | 40 / 30 | store |
| `0x3f5640` | `D3DDevice_SetRenderState_StencilFail` | no | stdcall(Value); ret 4; guest slot 145 | 15 / 6 | store |
| `0x3f56c0` | `D3DDevice_SetRenderState_TextureFactor` | yes | stdcall(Value); ret 4; guest slot 148 | 9 / 4 | store |
| `0x3f5730` | `D3DDevice_SetRenderState_LineWidth` | yes | stdcall(Value); ret 4; guest slot 157 | 12 / 7 | store |
| `0x3f57a0` | `D3DDevice_SetRenderState_Dxt1NoiseEnable` | yes | stdcall(Value); ret 4; guest slot 159 | 10 / 5 | store |
| `0x3f5830` | `D3DDevice_SetRenderState_YuvEnable` | no | stdcall(Value); ret 4; guest slot 160 | 9 / 4 | store |
| `0x3f5870` | `D3DDevice_SetRenderState_OcclusionCullEnable` | no | stdcall(Value); ret 4; guest slot 161 | 9 / 4 | store |
| `0x3f58e0` | `D3DDevice_SetRenderState_StencilCullEnable` | no | stdcall(Value); ret 4; guest slot 162 | 9 / 4 | store |
| `0x3f5950` | `D3DDevice_SetRenderState_RopZCmpAlwaysRead` | no | stdcall(Value); ret 4; guest slot 163 | 9 / 4 | store |
| `0x3f5970` | `D3DDevice_SetRenderState_RopZRead` | no | stdcall(Value); ret 4; guest slot 164 | 9 / 4 | store |
| `0x3f5990` | `D3DDevice_SetRenderState_DoNotCullUncompressed` | no | stdcall(Value); ret 4; guest slot 165 | 5 / 4 | store |
| `0x3f59b0` | `D3DDevice_SetRenderState_ZBias` | yes | stdcall(Value); ret 4; guest slot 149, derives 77–81 | 59 / 44 | store |
| `0x3f5a60` | `D3DDevice_SetRenderState_MultiSampleMode` | yes | stdcall(Value); ret 4; guest slot 154 | 11 / 6 | store |
| `0x3f5a90` | `D3DDevice_SetRenderState_MultiSampleRenderTargetMode` | yes | stdcall(Value); ret 4; guest slot 155 | 9 / 4 | store |
| `0x3f5ac0` | `D3DDevice_SetRenderState_MultiSampleAntiAlias` | yes | stdcall(Value); ret 4; guest slot 152 | 9 / 4 | store |
| `0x3f5b00` | `D3DDevice_SetRenderState_MultiSampleMask` | no | stdcall(Value); ret 4; guest slot 153 | 9 / 4 | store |
| `0x3f5b20` | `D3DDevice_SetRenderState_SampleAlpha` | no | stdcall(Value); ret 4; guest slot 158 | 18 / 11 | store |
| `0x3f5b40` | `D3DDevice_SetRenderState_LogicOp` | yes | stdcall(Value); ret 4; guest slot 150 | 10 / 5 | store |
| `0x3f5ba0` | `D3DDevice_SetRenderState_FillMode` | no | stdcall(Value); ret 4; guest slot 139 | 13 / 6 | store |
| `0x3f5bf0` | `D3DDevice_SetRenderState_BackFillMode` | yes | stdcall(Value); ret 4; guest slot 140 | 7 / 3 | store |
| `0x3f5c10` | `D3DDevice_SetRenderState_TwoSidedLighting` | yes | stdcall(Value); ret 4; guest slot 141 | 11 / 5 | store |
| `0x3f5c70` | `D3DDevice_SetRenderState_VertexBlend` | yes | stdcall(Value); ret 4; guest slot 137 | 9 / 4 | store |
| `0x3f5cf0` | `D3DDevice_SetRenderState_Simple` | no | ECX = push header (`1 << 18 \| method`), EDX = value; ret | 291 / 62 | store |
| `0x3f5d20` | `D3DDevice_SetTextureState_TexCoordIndex` | no | ESI = stage, [esp+4] = value; ret 4 | 11 / 4 | store |
| `0x3f5e30` | `D3DDevice_SetTextureState_BumpEnv` | yes | EAX = stage, stack (Type, Value); ret 8 | 15 / 4 | store |
| `0x3f5ea0` | `D3DDevice_SetTextureState_BorderColor` | yes | EAX = stage, EBX = value; ret | 10 / 5 | store |
| `0x3f5ef0` | `D3DDevice_SetTextureState_ColorKeyColor` | yes | EAX = stage, EBX = value; ret | 8 / 4 | store |
| `0x3f6190` | `D3D viewport push fragment` | no | EAX = put, ESI = device; returns EAX = new put | 22 / 22 | gap |
| `0x3f6390` | `D3D push fragment (0x3f6390)` | no | EDX = put, [esp+4] = device; returns EAX = put + 8; ret 4 | 18 / 18 | gap |
| `0x3f6840` | `D3D_UpdateProjectionViewportTransform` | yes | no args | 18 / 18 | gap |
| `0x3f6c60` | `D3DDevice_LoadVertexShaderProgram` | yes | stdcall(pFunction, Address); ret 8 | 1 / 1 | gap |
| `0x3f6cf0` | `D3DDevice_SelectVertexShaderDirect` | yes | EAX = Address, ECX = pVAF; ret | 1 / 1 | gap |
| `0x3f6e10` | `D3DDevice_SetShaderConstantMode` | yes | EAX = mode; ret | 1 / 1 | gap |
| `0x3f6f40` | `D3D transform/viewport push (0x3f6f40)` | no | ESI = device; returns EAX = new put when it pushes, else the shader object `[ESI+0x794]` | 18 / 18 | gap |
| `0x3f71d0` | `D3DDevice_SetVertexShaderConstant1` | yes | ECX = Register + 96, EDX = pConstantData; ret | 12 / 5 | gap |
| `0x3f7230` | `D3DDevice_SetVertexShaderConstant1Fast` | yes | ECX = Register + 96, EDX = pConstantData; ret | 6 / 6 | gap |
| `0x3f7280` | `D3DDevice_SetVertexShaderConstant4` | yes | ECX = Register + 96, EDX = pConstantData; ret | 2 / 2 | gap |
| `0x3f7330` | `D3DDevice_SetVertexShaderConstantNotInlineFast` | yes | ECX = Register + 96, EDX = pConstantData, [esp+4] = DWORD count (pushed in batches of 16, may end mid-register); ret 4 | 30 / 24 | gap |
| `0x3f7410` | `D3DDevice_SetVertexShaderConstantNotInline` | yes | EBX = Register + 96, EDX = pConstantData, EAX = DWORD count; ret | 1 / 1 | gap |
| `0x3f7440` | `D3DDevice_SetVertexShaderInput` | yes | stdcall(Handle, StreamCount, pStreamInputs); ret 0xC | 2 / 1 | gap |
| `0x3f77c0` | `D3DDevice_Reset` | yes | EBX = pPresentationParameters | 1 / 1 | gap |
| `0x3f7870` | `D3DDevice_SetRenderTarget` | yes | stdcall(pRenderTarget, pNewZStencil); ret 8 | 1 / 1 | gap |
| `0x3f7b30` | `D3DDevice_SetRenderTarget (same size, 0x3f7b30)` | no | [esp+4] = target, EBX = depth/stencil; ret 4 | 1 / 1 | gap |
| `0x3f8100` | `D3DDevice_SetViewport` | yes | stdcall(pViewport); ret 4 | 3 / 2 | gap |
| `0x3f8320` | `D3DDevice_SetTexture` | yes | EAX = Stage, [esp+4] = pTexture; ret 4 | 2 / 2 | gap |
| `0x3f84d0` | `D3DDevice_SetPalette` | yes | EAX = Stage, [esp+4] = pPalette; ret 4 | 7 / 4 | gap |
| `0x3f85d0` | `D3DDevice_EndVisibilityTest` | yes | EAX = index; returns EAX = 0; ret | 1 / 1 | gap |
| `0x3f8630` | `D3DDevice_GetVisibilityTestResult` | no | EAX = index, EDX = ULONGLONG* timestamp (optional), [esp+4] = UINT* count; returns EAX = 0 or `D3DERR_TESTINCOMPLETE`; ret 4 | 2 / 2 | gap |
| `0x3f8760` | `D3DDevice_SetScissors` | yes | stdcall(Count, Exclusive, pRects); ret 0xC | 3 / 3 | gap |
| `0x3f8ad0` | `D3DDevice_PersistDisplay` | yes | no args; returns EAX = HRESULT | 1 / 1 | answer |
| `0x3f8c70` | `D3D display/flicker filter (0x3f8c70)` | no | ESI = filter | 1 / 1 | gap |
| `0x3f8cc0` | `D3DDevice_SetStipple` | no | stdcall(pPattern), copies 32 dwords to device+0x197C, pushes method 0x1480; ret 4 | 9 / 9 | gap |
| `0x3f9d00` | `D3DDevice_Swap` | yes | EAX = Flags (0 = COPY \| FINISH); returns EAX = swap count; ret | 2 / 2 | seal |
| `0x3f9df0` | `D3DDevice_SetBackBufferScale` | yes | stdcall(x, y); ret 8 | 2 / 1 | gap |
| `0x3f9fc0` | `D3DDevice_Clear` | yes | stdcall(Count, pRects, Flags, `Color`, Z, Stencil); ret 0x18 | 7 / 7 | gap |
| `0x3fa3d0` | `D3DDevice_SetPixelShader` | yes | EAX = Handle; ret | 29 / 25 | gap |
| `0x3fa680` | `D3D_SetFence` | yes | stdcall(Type); returns EAX = fence; ret 4 | 9 / 9 | answer |
| `0x3fa970` | `D3D_BlockOnTime` | yes | 2 stack args; ret 8 (no-op: the host GPU is synchronous) | 1 / 1 | answer |
| `0x3faae0` | `D3D_MakeRequestedSpace` | yes | [esp+4] = MinimumSpace, EAX = RequestedSpace; returns EAX = put; ret 4 | 199 / 53 | gap |
| `0x3fac30` | `CDevice_KickOff` | yes | no args; EDX live across the call; ret | 17 / 16 | answer |
| `0x3fad50` | `D3D_BlockOnResource` | yes | EAX = pResource; ret (no-op: the host GPU is synchronous) | 10 / 8 | answer |
| `0x3fae10` | `D3D BeginPush (0x3fae10)` | no | ESI = dword count; returns EAX = put; ret | 1 / 1 | native |
| `0x3faed0` | `D3DDevice_DrawVertices` | yes | EAX = VertexCount, ECX = StartVertex, [esp+4] = PrimitiveType; ret 4 | 3 / 3 | gap |
| `0x3fafa0` | `D3DDevice_DrawIndexedVertices` | yes | stdcall(PrimitiveType, VertexCount, pIndexData); ret 0xC | 8 / 5 | gap |
| `0x3fb280` | `D3DDevice_SetVertexData2f` | yes | stdcall(Register, a, b); ret 0xC | 39 / 6 | gap |
| `0x3fb2d0` | `D3DDevice_SetVertexData4f` | yes | EDI = Register (-1 = position), stack (a, b, c, d); ret 0x10 | 44 / 10 | gap |
| `0x3fb330` | `D3DDevice_SetVertexData2s` | yes | stdcall(Register, a, b); ret 0xC | 19 / 4 | gap |
| `0x3fb380` | `D3DDevice_SetVertexData4ub` | yes | stdcall(Register, a, b, c, d); ret 0x14 | 1 / 1 | gap |
| `0x3fb3e0` | `D3DDevice_SetVertexDataColor` | yes | stdcall(Register, D3DCOLOR); ret 8 | 10 / 5 | gap |
| `0x3fb440` | `D3DDevice_Begin` | yes | stdcall(PrimitiveType); ret 4 | 11 / 9 | gap |
| `0x3fb480` | `D3DDevice_End` | yes | no args; ret | 21 / 17 | gap |
| `0x3fb520` | `D3D_DestroyResource` | yes | EDI = resource; ret | 1 / 1 | native (after the device's part) |
| `0x3fb600` | `D3DResource_AddRef` | yes | stdcall(this); returns EAX = new count; ret 4 | 6 / 3 | answer |
| `0x3fb640` | `D3DResource_Release` | yes | stdcall(this); returns EAX = new count; ret 4 | 14 / 7 | answer |
| `0x3fb710` | `D3DResource_IsBusy (0x3fb710)` | no | ECX = resource; returns EAX = busy (0/1) | 7 / 5 | answer |
| `0x3fb7a0` | `D3D surface-object allocator (0x3fb7a0)` | no | EAX = data, EDI = parent, 2 stack args; ret 8 | 2 / 1 | native |
| `0x3fc530` | `D3D device teardown (0x3fc530)` | no | EAX = device; ret | 2 / 2 | answer |
| `0x3fdb20` | `Get2DSurfaceDesc` | yes | EDI = container, EBX = level, ESI = out desc | 4 / 4 | native |
| `0x3fdbc0` | `D3D surface level layout (0x3fdbc0)` | no | stdcall 8 args; ret 0x20 | 12 / 9 | native |
| `0x3fde00` | `D3D texture header setup (0x3fde00)` | no | stdcall 8 args; ret 0x20 | 3 / 3 | native |
| `0x3ff5c6` | `D3D miniport gamma ramp (0x3ff5c6)` | no | EAX = miniport (device + 0x1C28), EDX = D3DGAMMARAMP (768 bytes); ret | 1 / 1 | gap |
| `0x400fa0` | `D3D commit deferred state (0x400fa0)` | no | no args; ret | 23 / 19 | gap |
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
  (`H2_XBE=/abs/path/default.xbe cargo test -p d3d --test it xbe_tables:: --
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
  SetVertexShader (`0x3f6d50`). See [Fragments and NV2A registers](d3d-nv2a.md#fragments-and-nv2a-registers) below.
- **Display entries** (`src/device/display.rs`). `0x3f8c70` and
  PersistDisplay change the guest globals the way the original does,
  without the AV-encoder kernel calls (there is no TV encoder).
  PersistDisplay copies the image last presented into a surface of its own
  (`Backend::copy_surface`). The copy outlives teardown. On a copy-swap
  device (see [Swap](d3d-device.md#swap) under Draws) the image is in back0 after a swap, and
  the title keeps drawing into it: the device copies back0 to the front
  before the first write into it (as the hardware's copy would have), so
  a later PersistDisplay still gets the image shown, and a frame nothing
  draws over costs no copy.
- **`SetStipple`** (`0x3f8cc0`). Method `0x1480` is
  `NV097_SET_STIPPLE_PATTERN`: 32 rows of a 32x32 polygon-stipple mask.
  All nine of Halo's calls (`0x2dd86`, `0x2df56`, `0x3531c`, `0x445f8`, ...)
  pass the result of `0x1c290`: one of 17 dither patterns from the table at `0x487288`, chosen from
  an opacity (screen-door fades). `D3DRS_STIPPLEENABLE`
  (`SET_STIPPLE_CONTROL`, `0x147C`) switches it on.
  - The entry copies the pattern to device `+0x197C` (unless flag `0x10`)
    and pushes it. The drain records it in `HwState::stipple_pattern`
    (`Device::stipple_pattern`, also in every `ResolvedDraw::hw`).
  - For filled polygon primitives the translated fragment shader masks
    pixels against it while `STIPPLEENABLE` is set. Wireframe and point
    fills do not use it. The visibility count pass does not apply it.
  - Row and bit order match the
    [asymmetric Xbox capture](https://github.com/abaire/nxdk_pgraph_tests_golden_results/blob/55958b25a77bd011b39bf16241a5b8ea0481e568/results/Stipple_tests/Asymmetric_On.png)
    and its [test source](https://github.com/abaire/nxdk_pgraph_tests/blob/2cfe6073d59ab2a69f98cf4c6ca003c1a02c8c6d/src/tests/stipple_tests.cpp)
    at 640x480: row `31 - (y mod 32)`, bit `(x mod 32) XOR 7`. The scaled
    view maps fragments back to the same guest grid
    ([high-resolution.md](high-resolution.md#positions)). Vertical
    anchoring at heights not divisible by 32 needs a hardware capture.
  - A native world draw can carry a display-only pattern
    (`Device::set_display_stipple`, `HwState::display_stipple`): the pixel
    uniforms of a target larger than the guest's take it through
    `HwState::stipple_rows`, and the reference copy keeps the guest's
    ([native-world.md](native-world.md)).
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
    `D3D_DestroyResource` (see [Resource destruction](d3d-device.md#resource-destruction)). As retail does,
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

## Render-state and texture-state setters

Each setter stores its value in the guest's own state array and pushes
nothing (`device_state::set`); game code reads the arrays back.

| array | address | layout |
|---|---|---|
| `D3D__RenderState` | `0x407498` | guest slot `n` at `+4n` (the slots in the table below) |
| `D3D__TextureState` | `0x404dd0` | stage `s`, state `t` at `+0x80 s + 4 t` |

- `SetRenderState_*` (`stdcall(Value); ret 4`): the raw value into its slot.
- `ZBias` also writes the five slots it derives (77–81, below), with the
  two floats read from the image (`0x45dbb8` = 2^32, `0x45dc20` = the slope
  factor).
- `Simple` stores nothing: game code stores the slot itself.
- `TexCoordIndex` (ESI = stage), `BorderColor` and `ColorKeyColor` (EAX =
  stage, EBX = value) store states `0x1c`, `0x1d`, `0x1e`; `BumpEnv`
  (EAX = stage, stack Type, Value) stores state Type.
- Not written: D3D's commit bookkeeping (dirty word `0x407490`, the texgen
  byte `0x475cbd + stage`, `ZEnable`'s W-buffer viewport refresh, the
  multisample modes' render-target rebind). Only the deferred commit and
  the push fragments read them, and those are gaps.
