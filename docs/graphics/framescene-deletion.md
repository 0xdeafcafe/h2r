# FrameScene: order of deletion and open questions

Status: Design. Sections 11 and 13 of [framescene.md](framescene.md): what is deleted in which slice, and what is not yet known.
Code: `crates/platform/d3d/src`, `crates/rewrites/native-render/src`, `crates/extensions/game/src/native_*_glue.rs`, `crates/extensions/boot/src/native_*_glue.rs`.

| When | What goes | About (lines) |
|---|---|---|
| Slice 2 | `native-render`'s `d3d` dependency for world and objects: their modules use `gfx::external`; the crate keeps `d3d` for the other stages | |
| Slice 4 | `scene-extract/src/layout/skip.rs`; the mask and admission halves of `boot/src/native_{world,objects,lights,shadows,first_person,world_layers}_glue.rs` and `feature/src/native_{world,objects,lights,shadows,first_person,world_layers}*`; `d3d/src/device/{opaque_world,opaque_vertices}.rs` | 18,000 |
| Slice 5 | `boot/src/native_{fog,water,post}_glue.rs`, `feature/src/native_{fog,water,post}.rs` | 5,000 |
| Slice 6 | `DrawAdmission`, `ExternalPass`, `ExternalPlan`, reservations and `device/admission_scope.rs`; `gfx_backend/native_runs.rs`; the scene-stage command of slice 2; the remaining `boot/src/native_*_glue.rs` and `feature/src/native_*` except the scene lists | 7,800 |
| Slice 7 | nothing: the old stack is what `native=off` selects | |
| Slice 8 | `d3d/src/interp/` and its three switches | 5,600 |
| Slice 10, step 1 | `impl.render.native` and everything only `=off` reaches: `d3d/src/threaded.rs`, `scaled.rs` and `scaled/` except the view mapping, the `Backend` implementations other than `SceneSink`, `gfx_backend.rs`'s surface, write-back and present code, `device/{mirror,display,snap}.rs`'s render-target half, lazy depth, alias mirrors, the visibility poll, and the "old-behaviour opt-out" variables of render-switches.md that configure them; `render.parity` | 8,000 |
| Slice 10, step 2 | what is left of `engine/src/render_loop` that no typed stage uses | |
| Slice 10, step 3 | the `Backend` trait, reduced to what `SceneSink` implements, then folded into `Device` | |
| Later, not scheduled | `viewer`'s and `scenario-edit`'s own world renderers, onto `native-render` through `frame-scene` | |

Each row is deleted in the slice named, in that slice's last commit, after its gate. Line counts are the size of the files named (**Code**, `wc -l`, before the native-cost integration), so they are a ceiling: loaders, material translation and anything a typed stage still uses move to `native-render` or `scene-extract` and stay.

## What stays of `d3d`

The 88 entries and their ABI, resources and locks, the push-buffer decoder and the state model (they resolve the carried draws), vertex-shader and combiner translation, the vertex cache, vblank and the logical frame wait, fences and `KickOff` answered at once. It stays as long as anything is carried, and the UI can stay carried indefinitely: it is cheap and still drawn once, by the native renderer. [retire-d3d.md](retire-d3d.md) is the plan that types the carried remainder and deletes the crate.

After step 1 there is no D3D reference in the tree. The reference for later render work is the last commit before it, tagged `d3d-reference`; `h2 lab run --name "D3D reference" --rev d3d-reference` builds it on the laptop. Its captures are game images and stay in local scratch, never in the repository.

## Risks and open questions

| Item | State |
|---|---|
| **The shared seed.** Section 5.2's first row decides how much guest code a typed stage can drop | If the draws from the seed under the object draw path cannot be reproduced in order without running most of that path, objects stay "typed for drawing, guest for side effects" and save less guest time than planned. The alternative is a new stream, which breaks existing films and needs every co-op peer on the same build. The owner's call; the spec assumes exact |
| **Is `native=on` hash-identical to `off`?** | Slice 0 answers it |
| **Do two launches stay comparable?** | They did to present 4500 in the all-native push; agent-api.md notes headless runs drifting apart after a few minutes. The gate corpus stays inside 5,000 presents, and `render.parity` needs no second launch |
| **Does a snapshot load across the switch?** | If `state.load` of a snapshot saved with `native=off` works with `on`, parity can start mid-level. Not checked; the spec does not rely on it |
| **Readbacks nobody listed** | Slice 6's census finds them. Film thumbnails and screenshots are owned by the theater session and need a `render.capture`-style answer from the native frame |
| **Slice 6 is large** | The cut, if needed, is by view kind ([framescene-owner.md](framescene-owner.md)) |
| **Carried draws at 120** | A world-space draw whose view block cannot be rebuilt is held at the tick's camera and judders against the blended world. `scene_held_draws` per level says how much slice 9 has to type before 120 looks right |
| **Change 5 (the layout crate) is not built** | `scene-extract` starts with a private `layout` module. If change 5 lands first, slice 1 uses it directly |
| **Display frames cost a whole draw of `B`** | The headroom cap (`frame-scene::pace::Headroom`) holds the rate at 60 when a display frame does not fit; `render.stats`' `display_rate.capped` says so |
| **Sim mode and dedicated hosts** | They skip the frame step with stubs. Running the step without extraction on a host is a follow-up for the session that owns hosts |
