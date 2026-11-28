# D3D gap callers that still run

Supports [d3d-entries.md](../d3d-entries.md) and
`crates/rewrites/render-bridge/src/guest_entries.rs`.

## 2026-11-28: static sweep at 5c0c88684

**Method.** Static, no run. The call graph is every direct
`H2_*CALL(h2fn_X, …)` in the lifted image's C (`shard_*.c`), each call
charged to the `h2fn_` it sits in. The hooks are read from the source:
every `implement`/`implement_async` point (by name through
`guest-layout/src/points.rs`, or the `points::` names near a point passed
as a variable), every `register_override`, and the cut tables
(`cut.rs`, `native_first_person.rs`). A caller **no longer runs** when it
is implemented, or every one of its direct callers no longer runs.

A caller with no direct caller (a fragment, a resume entry, a function
pointer) counts as running, and so does one behind a conditional `before`
hook. The list therefore over-counts: it is the set a crash can still
come from, not the set that does.

One way it under-counts: an
implementation that calls the original for some calls counts as stopping
the body. The present `0x13f10` (`render_frame::loop_present`) is one: a
kept loop frame runs its body, which reaches `SetBackBufferScale`, the
flicker filter `0x3f8c70` (both gaps) and `Swap` (answered).

**Answered** means every D3D entry the caller reaches now answers:
render-state and texture-state setters store their slot
(`device_state`), `Swap` seals, `KickOff`, the teardown and
`PersistDisplay` answer.

**Reproduce.** The scripts are in the coordinator's keep folder
(`sweep/graph.py` builds `graph.json` from the image's C; `live.py` and
`note.py` read the hooks from a checkout and print these tables):
`python3 note.py <checkout> graph.json rows.json d3d-gap-callers.md out.md`.

**Result.** 194 game-code functions call a D3D entry. 25 no longer run; 169 still run; 55 of those reach only answered entries now, 114 still reach a gap.

### Still reach a gap

| caller | gap entries | also answered | hooks at the caller | reached from (first live path) |
|---|---|---|---|---|
| `0x12560` | SetPalette, SetShaderConstantMode | 4 |  | 0x12000 ← 0x12190 |
| `0x128c0` | Reset | 0 |  | 0x12560 ← 0x12000 ← 0x12190 |
| `0x12a50` | D3D miniport gamma ramp (0x3ff5c6) | 1 |  | 0x2c8a0 ← 0x2232a0 ← 0x163890 ← 0x65340 ← 0x63d80 |
| `0x12d50` | D3D viewport push fragment, MakeRequestedSpace, VSC4 | 0 |  | 0x12fa0 ← 0x391f0 ← 0x36dee |
| `0x12fa0` | VSCNotInlineFast | 0 |  | 0x391f0 ← 0x36dee |
| `0x13450` | Clear, D3D viewport push fragment, MakeRequestedSpace | 1 |  | 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x14ac0` | Clear, SetViewport | 0 |  | 0x13450 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x14bc0` | SetViewport | 0 | observe_before `native_lights/walk.rs:588` | 0x13450 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x151e0` | VSC1Fast, VSCNotInlineFast | 0 |  | 0x4c690 ← 0x3ede0 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x152a0` | VSC1Fast, VSCNotInlineFast | 0 |  | 0x4d830 ← 0x4d0b0 |
| `0x15720` | D3D viewport push fragment, MakeRequestedSpace | 0 |  | 0x13630 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x15b70` | SetPalette | 0 |  | 0x226a0 |
| `0x15c90` | SetPalette | 0 |  | 0x4d0b0 |
| `0x15d00` | MakeRequestedSpace | 0 |  | 0x23ed30 ← 0x157020 ← 0x156e10 |
| `0x15ec0` | D3D commit deferred state (0x400fa0), MakeRequestedSpace | 0 | before `native_gels.rs:79`, after `native_gels.rs:95` | 0x16610 ← 0x1c0d0 ← 0x4d0b0 |
| `0x18d70` | MakeRequestedSpace | 0 |  | 0x16b90 ← 0x1bbf0 ← 0x16610 ← 0x1c0d0 ← 0x4d0b0 |
| `0x18e80` | VSC1Fast | 0 |  | 0x16b90 ← 0x1bbf0 ← 0x16610 ← 0x1c0d0 ← 0x4d0b0 |
| `0x1b050` | SetPixelShader | 0 | before `h1_hud_text/tint.rs:155` | 0x16b90 ← 0x1bbf0 ← 0x16610 ← 0x1c0d0 ← 0x4d0b0 |
| `0x1c590` | LoadVertexShaderProgram | 0 |  | 0x16610 ← 0x1c0d0 ← 0x4d0b0 |
| `0x1c710` | SelectVertexShaderDirect, SetVertexShaderInput | 0 |  | 0x16610 ← 0x1c0d0 ← 0x4d0b0 |
| `0x1c8a9` | DrawIndexedVertices | 0 |  |  |
| `0x1ccf0` | SetPixelShader | 0 |  | 0x251b0 |
| `0x1cdd0` | VSCNotInlineFast | 0 |  | 0x4c690 ← 0x3ede0 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x1cf50` | SetTexture | 0 |  | 0x16610 ← 0x1c0d0 ← 0x4d0b0 |
| `0x1cf80` | SetTexture | 0 |  | 0x37a60 ← 0x36dee |
| `0x1e406` | MakeRequestedSpace | 1 |  |  |
| `0x1eb00` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform, VSCNotInlineFast | 3 |  | 0x235d69 ← 0x147af1 ← 0x2c8a0 ← 0x2232a0 ← 0x163890 |
| `0x1ed98` | MakeRequestedSpace | 0 |  |  |
| `0x1ee60` | D3D commit deferred state (0x400fa0), MakeRequestedSpace | 1 |  | 0x1ed70 ← 0x235d69 ← 0x147af1 ← 0x2c8a0 ← 0x2232a0 |
| `0x1ef70` | MakeRequestedSpace | 1 |  | 0x1ee3a |
| `0x20f30` | SetPixelShader | 0 |  | 0x211a0 ← 0x2d565 |
| `0x24040` | VSC1, VSC4, VSCNotInlineFast | 0 |  | 0x4d0b0 |
| `0x245c0` | MakeRequestedSpace | 0 |  | 0x13870 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x246a0` | SetPixelShader | 3 |  |  |
| `0x25ca0` | VSC1 | 0 |  | 0x174f80 ← 0x173c90 |
| `0x26880` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform | 4 | implement? `native_fog_fans.rs:380` |  |
| `0x276d0` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, UpdateProjectionViewportTransform | 4 | implement? `native_fog_fans.rs:380` |  |
| `0x289e0` | Clear | 0 |  | 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x29370` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform | 4 |  |  |
| `0x296c0` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform | 4 |  |  |
| `0x29b00` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, UpdateProjectionViewportTransform | 4 |  | 0x289e0 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x2a1f4` | Begin, End, VD2f, VD4f | 0 |  |  |
| `0x2a2f0` | Begin, D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, End, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform, VD2f, VD4f | 4 |  | 0x289e0 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x2a8d0` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, UpdateProjectionViewportTransform | 4 |  | 0x289e0 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x2ae60` | Begin, End, VD2f, VD4f | 0 |  |  |
| `0x2b000` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform | 4 |  |  |
| `0x2d565` | End, MakeRequestedSpace | 0 |  |  |
| `0x2dd30` | SetPixelShader, SetStipple | 5 |  | 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x2df00` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetStipple, UpdateProjectionViewportTransform | 4 |  | 0x13870 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x2e82b` | D3D commit deferred state (0x400fa0), MakeRequestedSpace | 0 |  |  |
| `0x2e9b6` | End | 0 |  |  |
| `0x2e9e0` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, UpdateProjectionViewportTransform | 1 |  |  |
| `0x34420` | D3D commit deferred state (0x400fa0), MakeRequestedSpace | 0 |  | 0x249f0 ← 0x245c0 ← 0x13870 ← 0x2bcd0 ← 0x2c560 |
| `0x34770` | D3D commit deferred state (0x400fa0), MakeRequestedSpace | 0 |  | 0x34d90 ← 0x21360 ← 0x2d565 |
| `0x352e0` | SetPixelShader, SetStipple | 0 |  | 0x1c0d0 ← 0x4d0b0 |
| `0x35b90` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, UpdateProjectionViewportTransform, VSCNotInlineFast | 3 |  | 0x1f070 ← 0x1fa50 ← 0x1fa30 ← 0x24d0c7 |
| `0x363a0` | D3D commit deferred state (0x400fa0) | 0 |  | 0x1f3a0 ← 0x1f490 |
| `0x363c4` | MakeRequestedSpace | 0 |  |  |
| `0x36580` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform, VSCNotInlineFast | 3 |  | 0x159880 ← 0x13968c |
| `0x36880` | D3D commit deferred state (0x400fa0), MakeRequestedSpace | 0 |  | 0x159880 ← 0x13968c |
| `0x37a60` | Begin, VD2f, VD4f, VSC1 | 5 |  | 0x36dee |
| `0x38336` | End, VD4f | 0 |  |  |
| `0x38349` | SetPixelShader | 2 |  |  |
| `0x391f0` | D3D commit deferred state (0x400fa0), MakeRequestedSpace, VDColor | 3 |  | 0x36dee |
| `0x39780` | SetPixelShader | 0 |  | 0x391f0 ← 0x36dee |
| `0x3a5c0` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, UpdateProjectionViewportTransform | 4 |  | 0x26e50 |
| `0x3bf20` | Begin, End, SetPixelShader, VD4f, VDColor | 6 |  |  |
| `0x3d000` | VSC1 | 1 |  | 0x4d640 ← 0x4d0b0 |
| `0x40d86` | Begin, End, VD2s | 0 |  |  |
| `0x439d0` | MakeRequestedSpace | 0 |  | 0x180d80 ← 0x17ee20 ← 0x17e670 ← 0x23aad0 |
| `0x43a40` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, UpdateProjectionViewportTransform | 4 |  | 0x2c370 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x445d0` | SetPalette, SetStipple | 0 | observe_before `native_lights.rs:1131` | 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x46230` | D3D commit deferred state (0x400fa0), D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform, VD2s, VSCNotInlineFast | 3 |  | 0x4751b |
| `0x4679b` | End | 0 |  |  |
| `0x467c0` | D3D commit deferred state (0x400fa0), D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform, VSCNotInlineFast | 3 |  | 0x4751b |
| `0x46e44` | End | 0 |  |  |
| `0x47306` | End | 0 |  |  |
| `0x4730d` | Begin, VD2s, VD4f | 4 |  |  |
| `0x47501` | End, VD4f | 0 |  |  |
| `0x47732` | Begin, VD2s, VD4f | 0 |  |  |
| `0x47843` | End, VD4f | 0 |  |  |
| `0x47930` | MakeRequestedSpace, SetPixelShader, SetStipple, VSCNotInlineFast | 4 |  | 0x4fe20 ← 0x4f7e0 ← 0x4f010 |
| `0x48010` | MakeRequestedSpace | 0 |  | 0x47930 ← 0x4fe20 ← 0x4f7e0 ← 0x4f010 |
| `0x480a0` | D3D commit deferred state (0x400fa0), MakeRequestedSpace, VDColor | 0 |  | 0x2f247 |
| `0x48497` | End | 0 |  |  |
| `0x484b0` | D3D commit deferred state (0x400fa0), MakeRequestedSpace, VDColor | 0 |  | 0x4fd98 |
| `0x48ae5` | End | 0 |  |  |
| `0x4c690` | DrawIndexedVertices, VSCNotInlineFast | 2 |  | 0x3ede0 ← 0x2bcd0 ← 0x2c560 ← 0x2b0e12 |
| `0x4d640` | SetStipple | 0 |  | 0x4d0b0 |
| `0x4d720` | SetStipple | 0 |  | 0x4d621 |
| `0x4d830` | VSCNotInlineFast | 0 |  | 0x4d0b0 |
| `0x4dad0` | DrawIndexedVertices | 0 |  |  |
| `0x4e260` | DrawIndexedVertices, MakeRequestedSpace, SetStipple, VSC1Fast | 1 |  |  |
| `0x4ede0` | DrawVertices, VSC1Fast | 1 |  |  |
| `0x4f7e0` | VSCNotInlineFast | 1 | before `h1_glow.rs:21` | 0x4f010 |
| `0x4fe20` | SetPixelShader | 1 |  | 0x4f7e0 ← 0x4f010 |
| `0x52040` | D3D push fragment (0x3f6390), D3D transform/viewport push (0x3f6f40), D3D viewport push fragment, MakeRequestedSpace, SetPixelShader, UpdateProjectionViewportTransform, VSCNotInlineFast | 3 |  | 0x13c050 ← 0x147cbc ← 0x2b07e2 ← 0x2b0540 |
| `0x52d40` | SetStipple, VSCNotInlineFast | 5 |  | 0x43720 |
| `0x116b00` | D3D commit deferred state (0x400fa0), MakeRequestedSpace, VD4f | 1 |  |  |
| `0x157020` | VSCNotInlineFast | 1 |  | 0x156e10 |
| `0x174f80` | VSC1 | 0 |  | 0x173c90 |
| `0x19f680` | D3D commit deferred state (0x400fa0), MakeRequestedSpace | 0 |  | 0x244470 |
| `0x224fd0` | DrawIndexedVertices, MakeRequestedSpace, VD4f | 0 |  |  |
| `0x22a7f6` | Begin, End, VD2f, VD4f, VDColor | 0 |  |  |
| `0x23e460` | VSCNotInlineFast | 0 |  | 0x157020 ← 0x156e10 |
| `0x23ea80` | VSCNotInlineFast | 1 |  | 0x2243c0 ← 0x2242b0 |
| `0x254200` | Clear | 3 |  | 0x1a2823 ← 0x13954b |
| `0x254490` | D3D commit deferred state (0x400fa0), End, MakeRequestedSpace | 2 | observe_before `native_hud.rs:641`, before `native_hud.rs:692` | 0x1a2197 ← 0x1a2823 ← 0x13954b |
| `0x2548f0` | D3D commit deferred state (0x400fa0) | 4 | observe_before `native_hud.rs:666` | 0x1a2823 ← 0x13954b |
| `0x254b95` | End, MakeRequestedSpace, VD2f, VD4f | 0 |  |  |
| `0x254e28` | Begin, End, VD2f, VD4f | 1 |  |  |
| `0x255083` | Begin, End, VD2s, VD4f | 0 |  |  |
| `0x4022b8` | D3D miniport vertical-blank service (0x3fed90) | 0 |  | 0x3fe67f ← 0x3f8690 ← 0x3fc930 ← 0x3f77c0 ← 0x128c0 |
| `0x403400` | MakeRequestedSpace | 0 |  | 0x400fa0 ← 0x15ec0 ← 0x16610 ← 0x1c0d0 ← 0x4d0b0 |

### Answered in full now

| caller | entries |
|---|---|
| `0x12420` | D3D device teardown (0x3fc530), Swap |
| `0x141c0` | D3D device teardown (0x3fc530) |
| `0x142f0` | RS_Simple |
| `0x15370` | RS_Simple, RS_StencilEnable, RS_StencilFail |
| `0x15680` | RS_SampleAlpha, RS_Simple |
| `0x16610` | RS_CullMode |
| `0x17b60` | RS_BackFillMode, RS_CullMode, RS_DoNotCullUncompressed, RS_Dxt1NoiseEnable, RS_EdgeAntiAlias, RS_FillMode, RS_FogColor, RS_FrontFace, RS_LineWidth, RS_LogicOp, RS_MultiSampleAntiAlias, RS_MultiSampleMask, RS_MultiSampleMode, RS_MultiSampleRenderTargetMode, RS_NormalizeNormals, RS_OcclusionCullEnable, RS_RopZCmpAlwaysRead, RS_RopZRead, RS_SampleAlpha, RS_ShadowFunc, RS_Simple, RS_StencilCullEnable, RS_StencilEnable, RS_StencilFail, RS_TextureFactor, RS_TwoSidedLighting, RS_VertexBlend, RS_YuvEnable, RS_ZBias, RS_ZEnable |
| `0x18560` | TS_BorderColor, TS_BumpEnv, TS_ColorKeyColor, TS_TexCoordIndex |
| `0x18900` | RS_Simple |
| `0x189a0` | TS_BorderColor, TS_ColorKeyColor |
| `0x18ee0` | RS_CullMode, RS_DoNotCullUncompressed, RS_Dxt1NoiseEnable, RS_EdgeAntiAlias, RS_FillMode, RS_FogColor, RS_FrontFace, RS_LineWidth, RS_LogicOp, RS_MultiSampleAntiAlias, RS_MultiSampleMask, RS_MultiSampleMode, RS_MultiSampleRenderTargetMode, RS_NormalizeNormals, RS_OcclusionCullEnable, RS_RopZCmpAlwaysRead, RS_RopZRead, RS_SampleAlpha, RS_ShadowFunc, RS_Simple, RS_StencilCullEnable, RS_StencilEnable, RS_StencilFail, RS_TextureFactor, RS_TwoSidedLighting, RS_VertexBlend, RS_YuvEnable, RS_ZBias, RS_ZEnable |
| `0x1a170` | RS_BackFillMode, RS_CullMode, RS_DoNotCullUncompressed, RS_Dxt1NoiseEnable, RS_EdgeAntiAlias, RS_FillMode, RS_FogColor, RS_FrontFace, RS_LineWidth, RS_LogicOp, RS_MultiSampleAntiAlias, RS_MultiSampleMask, RS_MultiSampleMode, RS_MultiSampleRenderTargetMode, RS_NormalizeNormals, RS_OcclusionCullEnable, RS_RopZCmpAlwaysRead, RS_RopZRead, RS_SampleAlpha, RS_ShadowFunc, RS_Simple, RS_StencilCullEnable, RS_StencilEnable, RS_StencilFail, RS_TextureFactor, RS_TwoSidedLighting, RS_VertexBlend, RS_YuvEnable, RS_ZBias, RS_ZEnable |
| `0x1ab50` | TS_BorderColor, TS_BumpEnv, TS_ColorKeyColor, TS_TexCoordIndex |
| `0x1bbf0` | RS_CullMode, RS_FrontFace, RS_LogicOp, RS_SampleAlpha, RS_Simple, RS_ZBias, RS_ZEnable |
| `0x1e930` | RS_CullMode, RS_Simple, RS_StencilEnable, RS_ZBias, RS_ZEnable |
| `0x222d0` | RS_BackFillMode, RS_CullMode, RS_DoNotCullUncompressed, RS_Dxt1NoiseEnable, RS_EdgeAntiAlias, RS_FillMode, RS_FogColor, RS_FrontFace, RS_LineWidth, RS_LogicOp, RS_MultiSampleAntiAlias, RS_MultiSampleMask, RS_MultiSampleMode, RS_MultiSampleRenderTargetMode, RS_NormalizeNormals, RS_OcclusionCullEnable, RS_RopZCmpAlwaysRead, RS_RopZRead, RS_SampleAlpha, RS_ShadowFunc, RS_Simple, RS_StencilCullEnable, RS_StencilEnable, RS_StencilFail, RS_TextureFactor, RS_TwoSidedLighting, RS_VertexBlend, RS_YuvEnable, RS_ZBias, RS_ZEnable |
| `0x224f0` | TS_BorderColor, TS_BumpEnv, TS_ColorKeyColor, TS_TexCoordIndex |
| `0x251b0` | RS_CullMode, RS_Simple, RS_StencilEnable |
| `0x25d50` | RS_Simple |
| `0x25dc0` | RS_Simple |
| `0x26e50` | RS_CullMode, RS_Simple, RS_StencilEnable, RS_ZBias, RS_ZEnable |
| `0x29080` | RS_CullMode, RS_Simple, RS_StencilEnable, RS_ZBias, RS_ZEnable |
| `0x2c370` | RS_Simple |
| `0x34a90` | RS_CullMode, RS_Simple, RS_StencilEnable, RS_ZBias, RS_ZEnable |
| `0x34d90` | RS_CullMode, RS_Simple, RS_StencilEnable, RS_ZBias, RS_ZEnable |
| `0x351a0` | RS_CullMode, RS_ZBias |
| `0x35aa0` | RS_Simple |
| `0x396f9` | RS_Simple |
| `0x3c650` | TS_BumpEnv |
| `0x4751b` | RS_CullMode, RS_ZBias |
| `0x495e0` | RS_MultiSampleMode, RS_Simple |
| `0x496a0` | RS_MultiSampleMode, RS_Simple, RS_ZBias |
| `0x496f0` | RS_CullMode, RS_ShadowFunc, RS_Simple |
| `0x49740` | RS_ShadowFunc, RS_Simple |
| `0x4d9c0` | RS_Simple |
| `0x4f010` | RS_SampleAlpha |
| `0x508d0` | RS_SampleAlpha |
| `0x6a8a0` | CDevice_KickOff |
| `0x80e10` | CDevice_KickOff |
| `0x862e0` | CDevice_KickOff |
| `0x8c790` | CDevice_KickOff |
| `0x1241b0` | CDevice_KickOff |
| `0x12d0a0` | CDevice_KickOff |
| `0x12d400` | CDevice_KickOff |
| `0x12e0c0` | CDevice_KickOff |
| `0x12e150` | CDevice_KickOff |
| `0x146e30` | CDevice_KickOff |
| `0x156090` | CDevice_KickOff |
| `0x18f260` | CDevice_KickOff |
| `0x194160` | CDevice_KickOff |
| `0x194260` | CDevice_KickOff |
| `0x1995a0` | CDevice_KickOff |
| `0x199740` | CDevice_KickOff |
| `0x19fc40` | RS_CullMode |
| `0x22376b` | PersistDisplay |

### No longer run

| caller | why |
|---|---|
| `0x132d0` | implemented (view_setup.rs:676) |
| `0x13c20` | implemented (view_setup.rs:677) |
| `0x13f10` | implemented (render_frame.rs:214) |
| `0x14b60` | implemented (native_targets.rs:483) |
| `0x151c0` | all callers dead: 0x3c3c0 |
| `0x1cd30` | implemented (native_targets.rs:482) |
| `0x20770` | implemented (native_flare_query.rs:580) |
| `0x20bb0` | implemented (native_flare_query.rs:579) |
| `0x21a80` | all callers dead: 0x13680 |
| `0x25a10` | all callers dead: 0x132d0 |
| `0x25e50` | implemented (native_fog_quad.rs:228) |
| `0x26520` | implemented (native_fog_quad.rs:227) |
| `0x2d140` | implemented (native_post.rs:403) |
| `0x31170` | implemented (native_lights/passes.rs:26) |
| `0x32ac0` | all callers dead: 0x32350 |
| `0x32ce0` | all callers dead: 0x32350 |
| `0x3a550` | all callers dead: 0x4a1e0 |
| `0x3bd00` | all callers dead: 0x3c3c0 |
| `0x404b0` | implemented (native_lights/first_person.rs:41) |
| `0x40890` | all callers dead: 0x2ba10 |
| `0x44040` | implemented (native_decals.rs:272) |
| `0x46ea0` | implemented (native_flare_draw.rs:229) |
| `0x494b0` | all callers dead: 0x30ed0 |
| `0x4a780` | all callers dead: 0x32350 |
| `0x4ebd0` | implemented (native_decal_decorators.rs:609) |

### Gap entries by live callers

| entry | name | live callers |
|---|---|---|
| `0x3faae0` | D3D_MakeRequestedSpace | 48: 0x12d50 0x13450 0x15720 0x15d00 0x15ec0 0x18d70 0x1e406 0x1eb00 0x1ed98 0x1ee60 0x1ef70 0x245c0 0x26880 0x276d0 0x29370 0x296c0 0x29b00 0x2a2f0 0x2a8d0 0x2b000 0x2d565 0x2df00 0x2e82b 0x2e9e0 0x34420 0x34770 0x35b90 0x363c4 0x36580 0x36880 0x391f0 0x3a5c0 0x439d0 0x43a40 0x46230 0x467c0 0x47930 0x48010 0x480a0 0x484b0 0x4e260 0x52040 0x116b00 0x19f680 0x224fd0 0x254490 0x254b95 0x403400 |
| `0x3f6190` | D3D viewport push fragment | 21: 0x12d50 0x13450 0x15720 0x1eb00 0x26880 0x276d0 0x29370 0x296c0 0x29b00 0x2a2f0 0x2a8d0 0x2b000 0x2df00 0x2e9e0 0x35b90 0x36580 0x3a5c0 0x43a40 0x46230 0x467c0 0x52040 |
| `0x3fa3d0` | D3DDevice_SetPixelShader | 21: 0x1b050 0x1ccf0 0x1eb00 0x20f30 0x246a0 0x26880 0x29370 0x296c0 0x2a2f0 0x2b000 0x2dd30 0x352e0 0x36580 0x38349 0x39780 0x3bf20 0x46230 0x467c0 0x47930 0x4fe20 0x52040 |
| `0x3fb480` | D3DDevice_End | 20: 0x2a1f4 0x2a2f0 0x2ae60 0x2d565 0x2e9b6 0x38336 0x3bf20 0x40d86 0x4679b 0x46e44 0x47306 0x47501 0x47843 0x48497 0x48ae5 0x22a7f6 0x254490 0x254b95 0x254e28 0x255083 |
| `0x3f7330` | D3DDevice_SetVertexShaderConstantNotInlineFast | 19: 0x12fa0 0x151e0 0x152a0 0x1cdd0 0x1eb00 0x24040 0x35b90 0x36580 0x46230 0x467c0 0x47930 0x4c690 0x4d830 0x4f7e0 0x52040 0x52d40 0x157020 0x23e460 0x23ea80 |
| `0x3f6f40` | D3D transform/viewport push (0x3f6f40) | 18: 0x1eb00 0x26880 0x276d0 0x29370 0x296c0 0x29b00 0x2a2f0 0x2a8d0 0x2b000 0x2df00 0x2e9e0 0x35b90 0x36580 0x3a5c0 0x43a40 0x46230 0x467c0 0x52040 |
| `0x3f6840` | D3D_UpdateProjectionViewportTransform | 18: 0x1eb00 0x26880 0x276d0 0x29370 0x296c0 0x29b00 0x2a2f0 0x2a8d0 0x2b000 0x2df00 0x2e9e0 0x35b90 0x36580 0x3a5c0 0x43a40 0x46230 0x467c0 0x52040 |
| `0x3f6390` | D3D push fragment (0x3f6390) | 18: 0x1eb00 0x26880 0x276d0 0x29370 0x296c0 0x29b00 0x2a2f0 0x2a8d0 0x2b000 0x2df00 0x2e9e0 0x35b90 0x36580 0x3a5c0 0x43a40 0x46230 0x467c0 0x52040 |
| `0x400fa0` | D3D commit deferred state (0x400fa0) | 16: 0x15ec0 0x1ee60 0x2e82b 0x34420 0x34770 0x363a0 0x36880 0x391f0 0x46230 0x467c0 0x480a0 0x484b0 0x116b00 0x19f680 0x254490 0x2548f0 |
| `0x3fb2d0` | D3DDevice_SetVertexData4f | 16: 0x2a1f4 0x2a2f0 0x2ae60 0x37a60 0x38336 0x3bf20 0x4730d 0x47501 0x47732 0x47843 0x116b00 0x224fd0 0x22a7f6 0x254b95 0x254e28 0x255083 |
| `0x3fb440` | D3DDevice_Begin | 11: 0x2a1f4 0x2a2f0 0x2ae60 0x37a60 0x3bf20 0x40d86 0x4730d 0x47732 0x22a7f6 0x254e28 0x255083 |
| `0x3f8cc0` | D3DDevice_SetStipple | 9: 0x2dd30 0x2df00 0x352e0 0x445d0 0x47930 0x4d640 0x4d720 0x4e260 0x52d40 |
| `0x3fb280` | D3DDevice_SetVertexData2f | 7: 0x2a1f4 0x2a2f0 0x2ae60 0x37a60 0x22a7f6 0x254b95 0x254e28 |
| `0x3f7230` | D3DDevice_SetVertexShaderConstant1Fast | 5: 0x151e0 0x152a0 0x18e80 0x4e260 0x4ede0 |
| `0x3fb3e0` | D3DDevice_SetVertexDataColor | 5: 0x391f0 0x3bf20 0x480a0 0x484b0 0x22a7f6 |
| `0x3fb330` | D3DDevice_SetVertexData2s | 5: 0x40d86 0x46230 0x4730d 0x47732 0x255083 |
| `0x3fafa0` | D3DDevice_DrawIndexedVertices | 5: 0x1c8a9 0x4c690 0x4dad0 0x4e260 0x224fd0 |
| `0x3f71d0` | D3DDevice_SetVertexShaderConstant1 | 5: 0x24040 0x25ca0 0x37a60 0x3d000 0x174f80 |
| `0x3f9fc0` | D3DDevice_Clear | 4: 0x13450 0x14ac0 0x289e0 0x254200 |
| `0x3f84d0` | D3DDevice_SetPalette | 4: 0x12560 0x15b70 0x15c90 0x445d0 |
| `0x3f8320` | D3DDevice_SetTexture | 2: 0x1cf50 0x1cf80 |
| `0x3f8100` | D3DDevice_SetViewport | 2: 0x14ac0 0x14bc0 |
| `0x3f7280` | D3DDevice_SetVertexShaderConstant4 | 2: 0x12d50 0x24040 |
| `0x3fed90` | D3D miniport vertical-blank service (0x3fed90) | 1: 0x4022b8 |
| `0x3faed0` | D3DDevice_DrawVertices | 1: 0x4ede0 |
| `0x3ff5c6` | D3D miniport gamma ramp (0x3ff5c6) | 1: 0x12a50 |
| `0x3f77c0` | D3DDevice_Reset | 1: 0x128c0 |
| `0x3f7440` | D3DDevice_SetVertexShaderInput | 1: 0x1c710 |
| `0x3f6e10` | D3DDevice_SetShaderConstantMode | 1: 0x12560 |
| `0x3f6cf0` | D3DDevice_SelectVertexShaderDirect | 1: 0x1c710 |
| `0x3f6c60` | D3DDevice_LoadVertexShaderProgram | 1: 0x1c590 |
