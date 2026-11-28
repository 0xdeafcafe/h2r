# Residual render callers

Evidence for [retire-d3d-remainders.md](../retire-d3d-remainders.md), "The residual callers".

## 2026-11-28 - the callers left in a 16-bot game, read by caller

Measured by lab run 0016 (`render.residual`, the coordinator's
`lab0016-residual.json`; its commit is not recorded in the result), armed
for 120 frames (presents 7175 to 7294) in a 16-bot game on `cyclotron`,
without `frame-scene.census` (so no `producers` rows) and without
`render-timing`. The window's totals: 16,153 resolved draws, 13,728
claimed, 2,174 residual, 269,570 D3D entries. The five callers below hold
2,099 of the residual draws. The identifications are `h2 disasm` reads at
`ede64cd1f`:

```sh
h2 disasm disc/default.xbe 0x4daa0 120   # the emitter, its whole-part draw at 0x4dadb
h2 disasm disc/default.xbe 0x4ebd0 90    # the decal decorators' draw
h2 disasm disc/default.xbe 0x4c690 230   # the sky model renderer
h2 disasm disc/default.xbe 0x25d50 30    # then 0x26520 and 0x34420
h2 disasm disc/default.xbe 0x224f90 20   # the callback 0x224fd0 handed to 0x175610
```

| `callers` row | Residual of draws | What it is | Why its draws reach D3D |
|---|---|---|---|
| `0x4dad0`, site `0x4dae0` | 1,434 of 1,434 `DrawIndexedVertices` | the whole-part indexed draw of the geometry emitter `0x0004daa0`, which every layer's geometry handler and the sorted layer-15 handler call; `0x0004dad0` is where the transparent stage's hook splits the function | each stage's refused items, every layer together. The window has no `producers`, so it does not say which layer draw holds them |
| `0x4ebd0`, sites `0x4ed25`..`0x4edc8` | 325 of 325 `DrawVertices` (with 650 `ZBias`, 325 `c[53]`) | the draw of decorator types 1 and 2 (decals, built by `0x0004dfa0`) in the transparent layers: `c[53]` the detail level's fade, the vertex stream of the geometry record's block, the shader begin `0x0001bbf0`, `ZBias` 1, `DrawVertices(QUADLIST` or `TRIANGLELIST`, placement `+0xc`, `+0xe)`, `ZBias` 0 | no native stage takes decal decorators ([native-decorators.md](../native-decorators.md) types only type 0) |
| `0x4c690`, sites `0x4c6bf`..`0x4c9e8` | 200 of 480 strips | the sky model renderer, its strip at `0x0004c976` (layer 15) | strips `IndexedPass::for_sky` refused. The window does not name the cause. An earlier `render.scene` on `05b_deltatowers` (2026-11-27, the coordinator's `geo/scene.json`) shows `stencil` as the sky's one refusal, 3,285 of 13,480 strips. Every strip a pass draws on the guest also keeps the cut's arm from becoming ready |
| `0x25d50`, site `0x25db3` | 120 inline draws, one a frame | the planar fog's screen quad: `0x00026520` (view modes 2 and 3) sets the blend, depth and combiner state (`0x00025e50`, the other modes' setup, not read), then `0x00034420` pushes one four-vertex fan whose corners `0x00026710` fills | no native stage takes it. Its fan is written inside `0x00025d50` and filed at that function's next entry |
| `0x225475` (in `0x00224fd0`), sites `0x2256a8`, `0x2256c7` | 20 of 20 `DrawIndexedVertices` | the callback `0x00224fd0` that `0x00224fc1` hands to `0x00175610`: per item, `SET_VERTEX_DATA4F` rows pushed by hand (methods `0x1ac0`, `0x1ad0`, ... from `0x0022557c`), `D3DDevice_SetVertexData4f` and `DrawIndexedVertices(TRIANGLELIST)` of a model part. **S**: a particle drawn as a model (the caller asks `0x00173ba0`, which fills particle inputs, `prt3.txt`) | no native stage takes these draws |
