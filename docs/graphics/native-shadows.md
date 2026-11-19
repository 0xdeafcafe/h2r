# Native shadows

`impl.render.native_shadows` defaults off. On a single 1× retail GPU backend it can replace sorted BSP and model draws in layers 10, 11 and 14. It keeps the game's gather, template selection, material setup, push-buffer decoding and vertex resolution. Each replacement submits independent GPU packets. There is no measured performance or pixel-parity result for this lane yet.

## Caster enumeration and projection

`0x00044383` selects list 2 for layer 10 (`0x004c1a78`). The descriptor's OR word at +8 supplies scene bit 10. Layers 11 and 14 draw list 1 (`0x004c1a60`). Each list has an independent active interval [first, count). Its layer mask selects candidates. A build can reject an item, the game sorts the surviving entries, and the entry loop drops an entry whose section is unavailable. With `H2_NATIVE_SHADOWS_COMPARE=1`, the log records the list suffix, item index, full shader datum, variant, packed part argument, BSP owner or model datum/record, sorted implementation and actual GPU batches.

The geometry chooser `0x00030cd0` excludes render-record bit 10. With a matching template, BSP uses implementation 2. Models use 0 for the small colour mask or 1 for depth. Alpha kill in a material can punch holes in either silhouette. A query-counting draw and unknown build/draw pairs stay on D3D.

`0x00048ee0` produces 36 words at `0x004c9798`, uploaded to hardware vertex constants 40 through 48. Layers 10 and 11 admit a range only when all 36 floating-point words match bit for bit with the resolved draw's constants. The compare log includes the matrix block, target format, pushed clip bounds, clamp, viewport, scissor pieces, polygon offset enable, bias and slope for each admitted sorted item. The range recorder uses the same vertex program, constants, winding, streams, depth/stencil state and scissors as the resolved draw.

- **Depth map.** Target 9 pairs the colour alias with a D24S8 level, cleared to depth 1 and stencil 0. `0x000495e0` enables solid polygon offset. Its constant offset comes from the float at `0x0045e46c`, its slope from `0x0045dc0c`. Pass bit 4 selects the depth clip control. `0x000496a0` restores the state. A shader writing W depth already applies the bias and disables the extra pipeline bias. The sampled result encodes D24 in RGB, most significant byte first, and stencil in alpha.
- **Object mask.** Pass bit 4 set with bit 5 clear selects target 10, with white RGB and zero alpha as its clear. It is a colour texture. The receiver reads the filtered mask after the game finishes the intermediate passes.

## Application and layer 14

Layer 11 retains `0x000496f0`'s `GREATER` shadow comparison (stored depth greater than projected reference), per-stage filtering/addressing, receiver shader, inherited stencil test and per-channel writes. The layer end restores the shadow comparison. A 03a concrete material illustrates why these states travel with the draw.

| Implementation | Sampled input | Effect on a passing fragment |
| --- | --- | --- |
| 0, 1, 6 | One or three projected samples of engine texture 13 and the gel at 10 | Write alpha only, blending off, equal depth, depth writes off |
| 2 | Gel | Same alpha-only write |
| 3 | Depth map at 13 | Multiply destination RGB by source RGB, alpha test GREATER, equal depth, preserve destination alpha |
| 4 | Filtered colour mask at 8 | Multiply destination RGB by source RGB, equal depth, preserve destination alpha |

The colour passes use source factor destination colour, destination factor zero, and ADD. The material chooses the shaders and combiner constants. The engine selects the receiver implementation from its flags, definition bit 5, material word at +0x3e and record bit 10. The original per-draw stencil reference, read/write masks and operations reach the same depth/stencil attachment.

**Layer 14 can draw.** List bit 14 requests target 1 and the shared depth surface. The caller resets stencil before drawing. The mask tested by `0x00044550` includes bit 14. `0x00015d70` chooses implementation 0 for a present layer-14 template, and the geometry builder passes it through. The lane records these ranges for shadowed lights at the verified target; other implementations, targets and callbacks keep the original draw. The light graph must preserve this pass before 12/13 whenever the list requests it.

## Object chain

`0x000335e0` checks two predicates, enables the object-shadow mode, sets up target 9 at 256×256, and gathers up to 32 entries via `0x00033400`. Each entry is 20 bytes. Before accepting `0x00032350`, the observer verifies the gather caller, both mode bytes, the call's return address, the ESI cursor and EDI countdown, and both arguments against words +8 and +12 in that entry. `chain_match` reports this check in compare mode.

The main branch sets pass flags `0x14`, renders the layer-10 silhouette, calls `0x000222a0` for the two filter passes through targets 10 → 11 → 10, and applies layer 11 with the filtered target 10. The native freeze is ordered at the start of 11. A native caster can therefore share its map with D3D casters and with the guest filter. The original handles the early and alternate branches, projection setup, tail at `0x0004a780`, and the outer loop's restoration of both mode bytes. Each object and each additional-caster suffix at `0x00030ed0` starts a new generation.

## Interface for the light graph

`native_render::shadow_gpu::Producer` accepts the ordered caster batches on their encoder and returns a generation-owned `SampledMap`. In compare mode the private producer and original see identical resolved packets. The GPU differential compares all four channels, including stencil for D24.

In production the guest targets hold the result of all admitted and fallback casters. Boot publishes a handle at the matching layer-11 boundary through `boot::native_shadows_glue::observe_receivers(Some(observer))`. The observer receives a `ReceiverTerm` with the present, view, caster generation, site, identity (light and definition datums or both object arguments), projection, map kind and guest texture header. **Store the handle in the ordered graph node during the callback.** The guest memory guard is held. Calling back into the runtime there would reacquire it.

Resolve `term.sampled()` in render order, after the caster/filter freeze, on the same device. `None` keeps that light's original chain. For `MapKind::Depth`, `SampledMap::shadow_stage(linear, address)` supplies the D24 comparison input for the existing NV2A pixel translator. Use every stage occurrence requested by the receiver and keep its gel, colour mask, stencil and blend state. For `ColourMask`, bind the frozen colour view at the receiver's engine-8 stage. Retaining the handle keeps one generation alive across target reuse. An unshadowed light starts with no handle. Layers 12/13 and decisions about fusing an application belong to the lights lane.

Range admission requires the original sorted entry, matching shader/part, proven target, pipeline, and every texture and scissor range. A missing freeze, missing resource, changed owner or projection, pending guest callback or incomplete external recording leaves the draw with D3D. Scaled/E3 display copies and interpolated replay currently use the original renderer. The single-copy renderer can run headless or hosted. The mixed GPU fixture runs a shadowed receiver, an unshadowed D3D draw, and a receiver refused for a missing map on the same target. It compares all pixels with three original D3D draws. The compare diagnostic can observe the packets of physical backends that expose the shadow attachments; a missing marker or replay with zero packets provides no coverage.

The shared sorted point is `LIGHT_GEOMETRY_DRAW` at `0x0004d0b0`. Its seven stack arguments and 28-byte return are declared in `engine::points`. When integrating the lights lane, retain the typed declaration for its mutating hook. Give shadow work the ordered handoff from `2a61f863` or a compatible successor.

## Validation

`H2_NATIVE_SHADOWS_COMPARE=1` leaves the original submissions active, prints the exact caster/receiver evidence above and compares private layer-10 maps. `H2_NATIVE_SHADOWS_PROBE=1` counts replacement ranges recorded per site, layer, identity, generation and view. The handoff's lights-stage suppressed counter counts committed replacements, which includes other light work when that lane is active.

The agent method `render.shadows {}` exposes the bounded comparison history, including differently sized physical maps. A comparison reader can check nonzero packet counts, extent and differing texels per site. On a coordinator-owned comparison serve, use the existing agent connection:

```sh
h2 agent render.shadows '{}'
h2 agent frame draws
h2 agent render
```

These return the readback history, a guest-frame capture and the render counters respectively. The lane used static disassembly, decoded material state and synthetic fixtures. All cargo tests, live API checks, frame comparisons and timing runs belong to the coordinator under the lane rules.

From the integrated checkout, with the user's extracted XBE:

```sh
cargo test -p native-render --lib shadows::tests
cargo test -p native-render --test shadow_gpu -- --nocapture
cargo test -p boot --lib native_shadows -- --nocapture
cargo test -p shaders --test shadow
cargo test -p d3d --test external_pass --test depth_textures --test render_scale
cargo test -p feature --test native_shadows --test plan
H2_XBE="$PWD/disc/default.xbe" cargo test -p engine --features differential --test layer_differential --test shadow_constants_differential -- --nocapture
```

Use an integrated `h2`, a matching optimized image at `H2_SHADOW_IMAGE` and a GPU adapter with readable D24S8. The GPU fixtures skip without the corresponding adapter/capability. Run each pair from matching initial data and input. The commands use a fresh data directory per run.

```sh
shadow_run() {
    local name="$1" level="$2" end="$3" scale="$4" lighting="$5"
    local shadows="$6" lights="$7" compare="$8" every="$9"
    local out="$RUSH_KEEP/$name" data shots=8 probe=1
    if [ "$every" = never ]; then
        shots=0
        probe=0
    fi
    data=$(mktemp -d "$TMPDIR/shadows-data.XXXXXX") || return
    mkdir -p "$out" || return
    H2_OPT=2 H2_LEVEL="$level" H2_RENDER_SCALE="$scale" H2_LIGHTING="$lighting" \
    H2_BOOT_GFX="$out" H2_BOOT_GFX_EVERY="$every" H2_BOOT_GFX_SHADOWS="$shots" \
    H2_FRAME_STATS=1 H2_FRAME_STATS_FROM=750 H2_FRAME_STATS_CSV="$out/frames.csv" \
    H2_NATIVE_SHADOWS_COMPARE="$compare" H2_NATIVE_SHADOWS_PROBE="$probe" \
    H2_BOOT_STOP_AT_PRESENT="$end" \
    H2_FEATURES="impl.render.native_world=on,impl.render.native_lights=$lights,impl.render.native_shadows=$shadows,impl.render.interpolate=off,impl.render.reproject=off" \
    "$HOME/.cargo/bin/h2" run "$PWD/disc/halo2.iso" --out "${H2_SHADOW_IMAGE:?set to the matching image directory}" \
        --data "$data" --no-movies --no-mappack --no-halo1 >"$out/run.log" 2>&1
}
shadow_run 03a-original 03a_oldmombasa 4087 1 retail off off 0 250
shadow_run 03a-compare 03a_oldmombasa 4087 1 retail on off 1 250
shadow_run 03a-shadows 03a_oldmombasa 4087 1 retail on off 0 250
shadow_run 03a-lights 03a_oldmombasa 4087 1 retail off on 0 250
shadow_run 03a-mixed 03a_oldmombasa 4087 1 retail on on 0 250
shadow_run 00a-original 00a_introduction 4520 2 e3 off on 0 250
shadow_run 00a-fallback 00a_introduction 4520 2 e3 on on 0 250
shadow_run 03a-time-original 03a_oldmombasa 4087 1 retail off off 0 never
shadow_run 03a-time-shadows 03a_oldmombasa 4087 1 retail on off 0 never
```

Compare RGBA on identically numbered, visible frames, including 01000, 02000, 03000 and 04000. Check same-frame shadowed/unshadowed lights and the object filter/application separately. Compare both reference and displayed images for the E3 pair. Require zero differing texels on nonempty source comparisons, correct guest targets, and nonzero actual replacement counts for each path claimed. If these cinematics do not exercise a site, that site's runtime validation is still open. Use the same input replay reaching the site for both sides. Measure timings in separate runs with the screenshot interval `never`, `H2_BOOT_GFX_SHADOWS=0`, compare and probe disabled. Guest gather/decoding and synchronous per-range preparation remain in these timings.

## Shared integration files

| Files | Shadow-lane change |
| --- | --- |
| `crates/boot/src/lib.rs`, `registration.rs`, `native_world_glue.rs`, `d3d_glue.rs` | Register the feature, install its device host and configure the appropriate physical GPU copies. |
| `crates/d3d/src/gfx_backend.rs` | Optional observers for resolved caster batches and submitted frames. The handoff lane supplies the replacement contract. |
| `crates/engine/src/points.rs` | Shared typed sorted-geometry dispatch. |
| `crates/feature/src/lib.rs`, `crates/native-render/src/lib.rs` | Export the new modules. |
| `crates/boot/Cargo.toml`, `Cargo.lock` | Test-only shader fixture dependency. |
| `crates/agent-proto/src/lib.rs`, `crates/boot/src/agent_glue.rs`, `docs/runtime/agent-api.md` | Register and document the read-only comparison method. |
| `docs/graphics/layer-draw.md` | Correct the list of layers the dispatcher draws. |
