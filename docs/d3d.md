# D3D: the guest-facing Direct3D 8 replacement

`crates/d3d` replaces the Direct3D 8 that Halo 2 links statically (XDK 5849,
`D3D8LTCG`, the `D3D` section) at the API level, on top of `crates/gfx`. It
does not emulate the NV2A. This page covers what the game reaches, which
guest structures matter, what game code does *without* calling D3D, the
device model, and how to wire it into the runtime.

Static analysis only, from `h2 analyze` and `h2 symbols --list --no-xcheck`
over the retail XBE (identity in [engine-map.md](engine-map.md)). Matches
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
- Of those 91, **70** are implemented (adapter plus device method), **18**
  stop with `D3dError::Unimplemented` (which carries the entry name and
  its convention), and **3** are pure helpers left to the original code
  (`d3d::abi::NATIVE`).
- `d3d::register` installs **89** overrides in a `runtime::Dispatch`
  (see "Integration"): **71** implemented and **18** stopping. The extra
  implemented one is `SetRenderState_PSTextureModes` (`0x3f5cc0`). It is
  matched by signature and has an adapter, but no code outside the `D3D`
  section calls it (game code writes the `PSTEXTUREMODES` slot and device
  `+0x790` itself), so it is not part of the 91-function census.
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
| `0x3f6190` | `D3D viewport push fragment` | no | EAX = put, ESI = device; returns EAX | 22 / 22 | stop |
| `0x3f6390` | `D3D push fragment (0x3f6390)` | no | EDX = put, [esp+4] = device; returns EDX; ret 4 | 18 / 18 | stop |
| `0x3f6840` | `D3D_UpdateProjectionViewportTransform` | yes | no args | 18 / 18 | stop |
| `0x3f6c60` | `D3DDevice_LoadVertexShaderProgram` | yes | stdcall(pFunction, Address); ret 8 | 1 / 1 | yes |
| `0x3f6cf0` | `D3DDevice_SelectVertexShaderDirect` | yes | EAX = Address, ECX = pVAF; ret | 1 / 1 | yes |
| `0x3f6e10` | `D3DDevice_SetShaderConstantMode` | yes | EAX = mode; ret | 1 / 1 | yes (mode 0 stops) |
| `0x3f6f40` | `D3D transform/viewport push (0x3f6f40)` | no | ESI = device | 18 / 18 | stop |
| `0x3f71d0` | `D3DDevice_SetVertexShaderConstant1` | yes | ECX = Register + 96, EDX = pConstantData; ret | 12 / 5 | yes |
| `0x3f7230` | `D3DDevice_SetVertexShaderConstant1Fast` | yes | ECX = Register + 96, EDX = pConstantData; ret | 6 / 6 | yes |
| `0x3f7280` | `D3DDevice_SetVertexShaderConstant4` | yes | ECX = Register + 96, EDX = pConstantData; ret | 2 / 2 | yes |
| `0x3f7330` | `D3DDevice_SetVertexShaderConstantNotInlineFast` | yes | ECX = Register + 96, EDX = pConstantData, [esp+4] = DWORD count (pushed in batches of 16, may end mid-register); ret 4 | 30 / 24 | yes |
| `0x3f7410` | `D3DDevice_SetVertexShaderConstantNotInline` | yes | register (unrecovered) | 1 / 1 | stop |
| `0x3f7440` | `D3DDevice_SetVertexShaderInput` | yes | stdcall(Handle, StreamCount, pStreamInputs); ret 0xC | 2 / 1 | yes |
| `0x3f77c0` | `D3DDevice_Reset` | yes | EBX = pPresentationParameters | 1 / 1 | stop |
| `0x3f7870` | `D3DDevice_SetRenderTarget` | yes | stdcall(pRenderTarget, pNewZStencil); ret 8 | 1 / 1 | yes |
| `0x3f7b30` | `D3DDevice_SetRenderTarget variant (0x3f7b30)` | no | 1 stack arg; ret 4 | 1 / 1 | stop |
| `0x3f8100` | `D3DDevice_SetViewport` | yes | stdcall(pViewport); ret 4 | 3 / 2 | yes |
| `0x3f8320` | `D3DDevice_SetTexture` | yes | EAX = Stage, [esp+4] = pTexture; ret 4 | 2 / 2 | yes |
| `0x3f84d0` | `D3DDevice_SetPalette` | yes | EAX = Stage, [esp+4] = pPalette; ret 4 | 7 / 4 | yes |
| `0x3f85d0` | `D3DDevice_EndVisibilityTest` | yes | register (unrecovered) | 1 / 1 | stop |
| `0x3f8630` | `D3DDevice_GetVisibilityTestResult (assumed)` | no | EAX = index; ret 4 | 2 / 2 | stop |
| `0x3f8760` | `D3DDevice_SetScissors` | yes | stdcall(Count, Exclusive, pRects); ret 0xC | 3 / 3 | yes |
| `0x3f8ad0` | `D3DDevice_PersistDisplay` | yes | no args | 1 / 1 | stop |
| `0x3f8c70` | `D3D display/flicker filter (0x3f8c70)` | no | ESI | 1 / 1 | stop |
| `0x3f8cc0` | `D3D gamma/display ramp (0x3f8cc0)` | no | stdcall 1 arg, copies 32 dwords to device+0x197C; ret 4 | 9 / 9 | stop |
| `0x3f9d00` | `D3DDevice_Swap` | yes | EAX = Flags (0 = COPY \| FINISH); returns EAX = swap count; ret | 2 / 2 | yes |
| `0x3f9df0` | `D3DDevice_SetBackBufferScale` | yes | stdcall(x, y); ret 8 | 2 / 1 | yes |
| `0x3f9fc0` | `D3DDevice_Clear` | yes | stdcall(Count, pRects, Flags, Color, Z, Stencil); ret 0x18 | 7 / 7 | yes |
| `0x3fa3d0` | `D3DDevice_SetPixelShader` | yes | EAX = Handle; ret | 29 / 25 | yes |
| `0x3fa680` | `D3D_SetFence` | yes | stdcall(Type); returns EAX = fence; ret 4 | 9 / 9 | yes |
| `0x3fa970` | `D3D_BlockOnTime` | yes | 2 stack args; ret 8 (no-op: the host GPU is synchronous) | 1 / 1 | yes |
| `0x3faae0` | `D3D_MakeRequestedSpace` | yes | [esp+4] = MinimumSpace, EAX = RequestedSpace; returns EAX = put; ret 4 | 199 / 53 | yes |
| `0x3fac30` | `CDevice_KickOff` | yes | no args; EDX live across the call; ret | 17 / 16 | yes |
| `0x3fad50` | `D3D_BlockOnResource` | yes | EAX = pResource; ret (no-op: the host GPU is synchronous) | 10 / 8 | yes |
| `0x3fae10` | `D3D inline UP draw (0x3fae10)` | no | unrecovered | 1 / 1 | stop |
| `0x3faed0` | `D3DDevice_DrawVertices` | yes | EAX = VertexCount, ECX = StartVertex, [esp+4] = PrimitiveType; ret 4 | 3 / 3 | yes |
| `0x3fafa0` | `D3DDevice_DrawIndexedVertices` | yes | stdcall(PrimitiveType, VertexCount, pIndexData); ret 0xC | 8 / 5 | yes |
| `0x3fb280` | `D3DDevice_SetVertexData2f` | yes | stdcall(Register, a, b); ret 0xC | 39 / 6 | yes |
| `0x3fb2d0` | `D3DDevice_SetVertexData4f` | yes | EDI = Register (-1 = position), stack (a, b, c, d); ret 0x10 | 44 / 10 | yes |
| `0x3fb330` | `D3DDevice_SetVertexData2s` | yes | stdcall(Register, a, b); ret 0xC | 19 / 4 | yes |
| `0x3fb380` | `D3DDevice_SetVertexData4ub` | yes | stdcall(Register, a, b, c, d); ret 0x14 | 1 / 1 | yes |
| `0x3fb3e0` | `D3DDevice_SetVertexDataColor` | yes | stdcall(Register, D3DCOLOR); ret 8 | 10 / 5 | yes |
| `0x3fb440` | `D3DDevice_Begin` | yes | stdcall(PrimitiveType); ret 4 | 11 / 9 | yes |
| `0x3fb480` | `D3DDevice_End` | yes | no args; ret | 21 / 17 | yes |
| `0x3fb520` | `D3D_DestroyResource` | yes | register (unrecovered) | 1 / 1 | stop |
| `0x3fb600` | `D3DResource_AddRef` | yes | stdcall(this); returns EAX = new count; ret 4 | 6 / 3 | yes |
| `0x3fb640` | `D3DResource_Release` | yes | stdcall(this); returns EAX = new count; ret 4 | 14 / 7 | yes |
| `0x3fb710` | `D3D surface helper (0x3fb710)` | no | ECX | 7 / 5 | stop |
| `0x3fb7a0` | `D3D surface-object allocator (0x3fb7a0)` | no | EAX = data, EDI = parent, 2 stack args; ret 8 | 2 / 1 | native |
| `0x3fc530` | `D3D device teardown (0x3fc530)` | no | EAX = device; ret | 2 / 2 | yes |
| `0x3fdb20` | `Get2DSurfaceDesc` | yes | register (unrecovered) | 4 / 4 | stop |
| `0x3fdbc0` | `D3D surface level layout (0x3fdbc0)` | no | stdcall 8 args; ret 0x20 | 12 / 9 | native |
| `0x3fde00` | `D3D texture header setup (0x3fde00)` | no | stdcall 8 args; ret 0x20 | 3 / 3 | native |
| `0x3ff5c6` | `D3D miniport (0x3ff5c6)` | no | unrecovered | 1 / 1 | stop |
| `0x400fa0` | `D3D commit deferred state (0x400fa0)` | no | no args; ret | 23 / 19 | yes |
| `0x401030` | `CDevice_SetStateVB` | yes | stdcall(device, base); ret 8 | 1 / 1 | stop |

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
- **`SetPixelShader`**, as compiled, stores the handle at device `+0x784`,
  `PSDEF[8] | PSDEF[9]` at `+0x788`, `PSDEF[59] & 0x100` at `+0x78C` and
  `PSDEF[54]` (PSTextureModes) at `+0x790`. Unless device flag `0x10` is
  set (`BehaviorFlags & 0x10`; Halo passes `0x40`, so it is clear), it then
  copies the first 57 PSDEF dwords into render-state slots 0–56 and
  `PSDEF[54]` into slot 136. The device does the same.
- **`MakeRequestedSpace`** takes the request in `EAX` and half the kick-off
  size on the stack. It returns the new put in `EAX`.
- **Stops that are likely to be hit first.** These are called from many game
  functions:
  - `0x3f6190`, `0x3f6390`, `0x3f6f40` and
    `D3D_UpdateProjectionViewportTransform` (18–22 callers each). They
    are viewport/transform push fragments called from render setup.
  - `0x3f8cc0` (9 callers).

  They are next in line after the first frame. See "Boot path" for the
  order the game reaches them in.
- **Native helpers** (`d3d::abi::NATIVE`). `0x3fdbc0`, `0x3fde00` and
  `0x3fb7a0` compute surface/texture headers from their arguments and the
  read-only format table at `0x00403A50` (plus `XMemAlloc` and the
  overridden `D3DResource_AddRef` for `0x3fb7a0`). Their callees touch no
  device global, NV2A register or push buffer, so the original code runs.

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
| `+0x197C` | 32-dword ramp copied by `0x3f8cc0` | `0x3f8cc0` |
| `+0x1A04` / `+0x1A08` | render target / depth surface | SetRenderTarget, game reads |
| `+0x1A14` / `+0x1A18` | back buffer / display surface | CreateDevice, viewport code |
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
| `0x0100`, `0x0110` | NOP, WAIT_FOR_IDLE | 1 / 1 | yes |
| `0x1D8C` | clear values, two words (inline clear) | 1 / 1 | **stops** |
| `0x1480` | a 32-word block (`0x0001e370`; the same data `0x3f8cc0` copies to device `+0x197C`) | 1 / 1 | **stops** |
| `0x1710` | one word, value 0 | 1 / 1 | **stops** |
| `0x17C8` | two words (report / pixel-count methods) | 1 / 1 | **stops** |

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
- Only the method subset in the table above is understood. Anything else is
  a `PushError::UnknownMethod` stop. Headers with reserved bits (bit 31,
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
  (`0x1D90`) is not invertible and is rejected, as is any other method;
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
  - stream bytes read from guest memory, de-indexed on the CPU for indexed
    draws;
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
- `GfxBackend` renders pre-transformed fixed-function draws through gfx's
  built-in shader.
- Programmable draws need a `ShaderProvider`, the interface for
  `crates/shaders` (TODO(shaders)). Its `prepare` receives the whole
  `ResolvedDraw`. Until one is installed they are skipped with an
  `Event::DrawSkipped` that gives the reason.
- State gfx cannot express **stops** with `D3dError::Unimplemented` rather
  than rendering wrongly:
  - a viewport that is not the whole target, or scissors that do not cover
    it (SetViewport clips to the target and resets the scissors, as the
    original; a null viewport keeps the current one);
  - a back-buffer scale other than 1 x 1;
  - alpha test on the built-in shader path (it never discards);
  - partial clears: `Clear` intersects every rectangle with the viewport
    and uses the viewport when Count is 0 (`0x3fa1a0..0x3fa247`), skips
    empty intersections, and stops if what is left is not the whole target;
  - partial-channel clears;
  - render-to-texture (a surface whose `Parent` is a texture).

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

### Invalidation contract

The runtime has to keep these rules:

1. It reports every guest store the device did not make, through
   `Device::note_write(addr, len)`, by guest virtual address in 4 KiB pages.
   This covers lifted code, the interpreter, and kernel or DMA-like copies.
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
   - `abi::NATIVE` addresses get no override.
   `tests/registration.rs` runs Halo's boot device sequence through
   `runtime::Runner` with these overrides.
3. **Write barrier.** Forward every guest store D3D did not make to
   `Device::note_write`. Until that exists, run with `UploadPolicy::Always`
   (or `Verify` to find the misses).
4. **Present.** `GfxBackend::on_present` receives the surface on every
   `Swap`. Blit it with `gfx::surface::present_surface`.
5. **Shaders.** Implement `ShaderProvider` over `shaders::link` once
   `crates/shaders` lands, and install it with `GfxBackend::with_shaders`.

The conventions in `abi.rs` are marked TODO-register. Each one should get a
differential check, running the original under the interpreter against the
adapter, at its recorded call sites before it is trusted.

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

No boot trace exists yet, so this is static. Conditional stops that normal
view rendering (`0x223240 -> 0x2232a0 -> 0x2c8a0 -> 0x132d0`) can reach:

- **Viewport.** `0x14bc0` selects a render target by index and calls
  SetViewport with its extents. For the back buffer at 640x480 that is the
  whole target. Any smaller viewport (split screen, letterboxing, the
  offscreen targets) stops at the next draw, and at any Count=0 clear,
  since every game clear passes Count=0 and so clears the viewport.
- **Offscreen targets.** The targets `0x14850` builds are texture levels
  (`Parent` set by `0x3fb7a0`): SetRenderTarget stops on them
  (render-to-texture). `0x14bc0` also calls Get2DSurfaceDesc (`0x3fdb20`,
  a stop) for targets other than indices 0, 1, 2 and `0x12` that are not
  the current one.
- **Channels.** The clear wrapper `0x14b60` builds the flags from `EAX`:
  bit 0 is all colour, bits 1–4 single channels (`0x10..0x80`), bits 5–6
  depth/stencil. Alpha-only clears: `0x13c3f` (when `0x4BA04C > 1`, that
  is, more than one view) and `0x3c515` (`EAX = 0x10`, in `0x3c3c0`, from
  `0x2bcd0`). Callers passing a register (`0x132d0`, `0x2bcd0`, `0x3fdf0`,
  `0x41cc0`, `0x44370`) depend on the view. The full-screen helper
  `0x14ac0` clears with `0xF0`, which is fine.
- **Fragments.** `0x3f6840`, `0x3f6f40`, `0x3f6190`, `0x3f6390` are reached
  from render setup (`0x35b90`) and also from the inlined ZEnable when the
  old value was `D3DZB_USEW` (`0x1eb94`).

## gfx changes this wants

These were not made here, since gfx is its own lane.

- **Per-draw viewport and scissor** in `gfx::Draw`. The renderer always
  uses the full target, so non-full viewports and scissors stop today.
- **Alpha test in the built-in pre-transformed shader.** Alpha-tested draws
  on that path stop today.
- **Indexed draws.** The device de-indexes on the CPU.
- **Batching several draws into one pass.** The backend opens one pass per
  draw. That is correct but slow.
- **Partial-rectangle and channel-masked clears.** Both stop today.
- **Public `HostPixelOrder` from `gfx::surface`.** It is only reachable as
  `gfx::encode::HostPixelOrder`.
