# Retiring the guest D3D path

Status: Design. Slice R0 (`render.residual` and the three frame counters, [agent-api.md](../runtime/agent-api.md#renderresidual-residual-d3d-work)) is built. Of R1, the structure's static items of layers 1, 5, 7, 8 and 20, and the objects' items of the scene's layers 1, 5, 7, 8 and 20 (layer 5's with the model's PRT stream) and the first person's layers 1, 7, 8 and 20, draw from the scene's plan by default, each item only where skipping its guest draw leaves the device's render states as drawing it would, a pass whose restore list writes back what it sets included (framescene.md slice 4, "The stage"). Layer 3's object parts draw every topology and subpart choice their records make, a strip part's chosen subparts included (framescene.md slice 3, "Bit 9"), and its decorators draw from the scene only where skipping them leaves the device's render states as drawing them would, a pass whose restore list writes back what it sets included (native-decorators.md, "The glue"). Whole groups of layers 10, 11 and 14 also draw from the scene with no geometry handler, and the object filter draws natively (native-shadow-layers.md). The rest of R1 is not built. Of R5, layer 2 and layer 17 draw their structure and object items from the scene's plan as layers 1, 5, 7, 8 and 20 do (layer 2 at the main, alternate and first-person calls). The sky stage draws supported strips with the view's node palette and the material inputs at issue. The parts of R5 earlier stages draw (fog, water, layers 18, 19 and 21, the flares' colour fans, the post) are in "By caller"; no other slice is built. The stages it builds on are in [native-renderer.md](native-renderer.md) and [framescene.md](framescene.md).
Code: `crates/platform/d3d/src` (what is deleted), `crates/rewrites/scene-extract/src` and `crates/rewrites/native-render/src` (what replaces it), `crates/rewrites/render-bridge/src` (the frame step and the census). Tests: named per slice below.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md). It is removed with the crate. No slice adds a switch.

The recompiled game runs its own render front-end every frame: it makes
D3D8 calls and writes render-state and push-buffer words, `d3d` resolves
them into draws, offers each draw to a native stage, and draws what no
stage claims. At the end of this plan the front-end does not run. A Rust
frame step reads the game's world, object, effect, HUD and UI data into a
`FrameScene`, the native renderer draws it, the guest makes no D3D draw
call, and `crates/platform/d3d` is deleted. The one rule: a slice removes
D3D work only for content whose residual count is zero, and anything not
yet native stays on D3D under a name.

[framescene.md](framescene.md) is the design this page continues. It
ends with the renderer owning the frame while the UI, the HUD and the
long tail stay *carried* (guest draws that `d3d` resolves and the native
renderer plays), and with `d3d`'s entries, state model and push-buffer
decoder kept for them. This page removes the carried grade and then the
crate. Where framescene.md already specifies a step, this page names its
slice and does not restate it.

Evidence labels as in [docs-style.md](../meta/docs-style.md): **Code**,
**Test**, **Run**, **S**, **Design**, and **Estimate** for a number
nobody has measured. Addresses are research points. No game data.

## Inventory: what the native stages do not claim

There are three kinds of D3D use, and a stage that "draws natively" today
removes only part of the first.

1. **Draws.** A resolved draw reaches the backend unless an admission or
   a native producer committed a replacement (`d3d::device::backend_draws`
   counts the ones that do).
2. **Draw setup the guest still runs for a claimed draw.** The stages on
   the resolved-draw path replace the packet only: the guest's shader
   setup, state writes and stencil tail still run and still go through
   `d3d` ([native-renderer.md](native-renderer.md), "Admission"). This is
   most of the front-end's CPU time, and no draw count shows it.
3. **Everything that is not a draw.** Clears, target binds, viewport and
   scissor fragments, visibility tests, texture and palette binds,
   resource reference counting and busy checks, fences, `KickOff`, the
   gamma ramp, vblank and `Swap`.

### External pass refusals

A device refusal includes the producer's Rust pass type, its issue boundary,
and the native stage for a typed plan. The first refusal for each producer,
boundary, stage and reason is logged. Device counters retain every refusal.
A failed reservation leaves the original guest draws available to its caller.
A refusal from a pass with no corresponding guest draw can instead omit
that operation.

`d3d::SceneSink` declines untyped passes and unsealed reservations inside a
captured scene slot, except the commits of native producers it carries
(water, flares and UI; [framescene.md](framescene.md)). The display graph needs retained inputs to record the
operation on its own attachments. A one-shot pass carries no replay command.
The sink names this case with the producer, so it can be distinguished from a
binding failure or a backend recording failure. Outside captured slots these
passes still use their normal backend path.

### By caller

"Claimed" is what a native stage takes in the default configuration.
"Left on D3D" is what the stage pages and `native-renderer.md`,
"Residual render callers", say stays. All of it is **Code** (read from the
stage pages and the hooks they cite) unless marked. No run has counted
these per caller with every stage on: the column "Count" says what exists.

| Caller (guest function) | Draw kind | Claimed | Left on D3D, and why | Count |
|---|---|---|---|---|
| Layer draw `0x00044550`, layer 3, from `0x0002bed0` | indexed lists and strips, world and objects | world and object parts including passes with all colour writes masked and opaque One/Zero/Add blends, object templates with inherited blend and depth-write fields or instance-driven states, model decorators | parts whose material pass does not translate; objects the walk returns (`walk_unsupported`: a bit-11 part whose auxiliary state toggle does not read, a bit-6 part whose visible subparts do not read or number more than 64); decorators with an engine-driven state from a per-draw source or a lightmap stage whose globals bitmap is not uniform, whose skip would leave a render state other than the device holds (one the pass sets and its restore list does not write back, or one the restore list writes another value to: `scene_decorator_draws_state`), or whose layer-3 view's stage palettes, stencil or inherited cull do not read or hold; any view where one copy refuses. Setup (kind 2) for the resolved-draw remainder | 8.26 ms, 1,882 D3D entries a frame on the D3D path [a]; about 14 objects a frame returned [b] |
| Layer draw, layers 1, 5, 7, 8, 20 | indexed | the structure's static items and the objects' items of layers 1, 5, 7, 8 and 20, layer 5's with the PRT stream the draw adds (the first person's 1, 7, 8 and 20) from the scene's plan (`frame-scene.layers`), under the stencil the geometry draw's prologue sets (`layer_pass::item_stencil`) | layer 5's object items whose PRT stream does not read or supplies incompatible registers or components, items whose pass leaves a render state other than the device holds (one it sets and its restore list does not write back, or one the restore list writes another value to: `layer_pass::skip_holds`), object items extraction names a cause for or whose passes do not plan, animated and per-draw items, every item after the first the plan cannot take, and every item with `frame-scene.layers=off` (kind 2 setup for all of it) | layer 1: 0.32 ms, 56 entries [a]; others under 0.3 ms |
| Layer 2 (`0x0002c05b`; the alternate view's `0x0002bda3`; the first person's `0x00040284`) and the sky `0x0003ede0` (strip at `0x0004c976`) | indexed | layer 2's structure and object items at the main, alternate and first-person calls from the scene's plan (`frame-scene.layers`, `layer_pass::OVERLAY_LAYER`), under the device's stencil; the sky strip with per-view node rows, material bindings and the mode-2 stencil state; the remaining standard layer-2 geometry as packets | what layers 1, 5, 7, 8 and 20 refuse; other layer-2 handlers, unknown callers; unresolved sky geometry, transforms, materials or stencil state; guest sky selection and setup | 0.29 ms, 120 entries [a] |
| Object shadows `0x000335e0`, `0x00032350` (both partial, as are their caster and receiver layers 10 and 11) | indexed into shadow targets, then receivers | object lists, histories and admitted mask caster bodies ([native-light-gather.md](native-light-gather.md), L7), whole groups of layers 10, 11 and 14 from the scene with no geometry handler, the filter's native quads; sorted BSP and model draws of layers 10, 11 and 14, at 1x on one backend | scaled and E3 display copies, interpolated replay, a missing freeze or resource, a pending callback, an animated gel, guest creature shadows, and the mask chain's target and device setup | 1.24 ms, 182 entries [a] |
| Dynamic lights `0x00031c80`, per light `0x00031170` | indexed, layers 10 to 14; gel quads into target 13 | `dynamic_lights` and `light_passes` are partial. The Rust gather supplies ordered lights. Eligible diffuse and specular groups draw together over structure and models. | A refused gather runs the guest loop. Shadow and opacity targets, shader gels, incomplete groups and unavailable device copies keep their layer handlers | 1.88 ms without the per-light passes [a]; per-light time not broken out |
| First person `0x0003fdf0(0)`, `(1)` and its lights `0x000404b0` | indexed | record-model uploads from mixed lists and compiled parts of partially supported models; the sole in-layer item, full parts and sorted subranges of layers 1, 2, 3, 7, 8, 20; the object items of layers 1, 2, 7, 8 and 20 from the scene's plan (`frame-scene.layers`); its lights' layers 12 and 13 whose groups the scene holds whole, from the scene ([native-lights.md](native-lights.md)) | mixed-list sorting, individual parts without a compiled pass; the lights' list rebuilds, constants and layers 11 and 14, a group the scene does not hold whole, a light with a shader gel | 0.84 ms, 404 entries; lights 0.39 ms, 173 entries [a] |
| Fog (layer 16), water (layer 6) | indexed, full-screen | planar fog, passes with all colour writes masked, the water edge blend including rigid placements and rasterizer bitmap stages 14 and 15, eligible indexed ranges in mixed lists, and the indexed composite in `0x0003c3c0` | mixed-list sorting and state setup, refused ranges and composite inputs, the conditional alpha clear and resource updates | under 0.3 ms [a] |
| Camouflage (layers 18, 19, sorted handler `0x00041cc0`), hologram (21, `0x000423c0`) | indexed with a framebuffer snapshot | layers 18, 19 and 21's object items from the scene's plan (`frame-scene.layers`), with the shader the record names over the part's and a copy of the bound colour for the view stages ([native-water-fog.md](native-water-fog.md)) | layers 18, 19 and 21's items extraction names a cause for (the stipple fade among them), whose shader's passes do not compile or plan, and every item after the first refused; the handlers' replays of the scene's layers | not measured |
| Sorted transparents `0x0001e370` (also `0x00040870`), layer 15 | indexed; per-handler immediate | Rust sort and native indexed draws for the standard layer-15 handler, including individual subranges and resolved object draws without a lighting owner | handler setup and restore, nonstandard emitters, ranges that fail state or attachment validation | 2.58 ms, 1,010 entries [a] |
| Particles `0x0023ed5c`, effect quads and contrails `0x0004b252`, `0x0004b28c` | inline quads and strips | the draw packet | iteration, property evaluation, ribbon construction, the per-view update `0x00174420`; unknown builders | not broken out |
| Decals `0x00044040` | array quad lists | ordered packets using each copy's constants and scissors, with live depth, offset and texture border state | guest allocation, retirement, traversal and state setup. Packets can fall back when their resources or shader cannot be recorded. | resolved draws per present interval and guest pool occupancy |
| Lens flares: writer `0x00020bb0`, reader `0x00020770` | a counted draw between an inline begin and `EndVisibilityTest` | the pixel counter, on the GPU | the counted draw, the report slots, `GetVisibilityTestResult` | none |
| Post: `0x00013870`, colour pass `0x000214f0`, screen effect `0x0002d140` | strips into side targets | glow, screen strips, blur and convolutions, as one transaction per call, including glow quads with live blend and channel masks, mixed A8/X8 framebuffer reads, and a final strip or convolution into a side target | a call with an unsupported input or a missing live target; a reference-only intermediate under scaling | 0.76 ms, 164 entries [a] |
| `0x0002dd30`, `0x0002df00` (the flares' tests and draws) | a counted visibility fan, reflection billboards and a filtered composite | six colour fan sites ([retail trace](notes/2026-11-24-lens-flares.md)) | visibility test and feedback, guest queue and target changes, refused colour fans | committed fan ranges per copy in `render.scene` |
| Layer 17 `0x0002c30b` (S: bloom) | indexed | its structure and object items from the scene's plan (`frame-scene.layers`) | what layers 1, 5, 7, 8 and 20 refuse | none |
| HUD `0x001917eb` (tail `0x0013954b`), shader quad `0x0022a664` | immediate quads through a shader | tag-shader and supported generic widget quads through the overlay, motion sensor compositing fans | the shader draw `0x0001bbf0` per quad; uncovered widget helpers (`guest_builder_draw`); the built-in diffuse shader | none: the HUD is outside the timed scene-pass sites |
| UI `0x00147af1` (widget dispatch `0x00235445`), screen quads `0x00035b90`/`0x000363a0`/`0x00036560` | one inline quad per glyph or bitmap | supported inline bitmap and glyph quads through the overlay, model-widget material groups | layout, shaping, state and target changes; a wrap or pending fence; model-widget groups that refuse | none for the UI alone; the menu frame is 755 to 1,704 draws [c], most of them its 3D view |
| Fade `0x00040890` | full-screen overlay | all six fade modes through the native overlay | guest state setup and draws refused by the overlay | not measured |
| Menu rectangles and bars (`0x0004b252` from `0x00052ce7`), outlines `0x0001ed98` | immediate fans, a line strip | the geometry | state setup, the line state | none |
| Untraced `D3DDevice_Begin` callers `0x00029b00`, `0x0003bf20` | immediate | none | not read (**S**: what they draw) | none |
| Loading `0x000289e0`, spinner `0x0002a1f4`, bitmaps `0x0002ae60` | immediate fans | the overlay draws supported background, sweep and spinner fans with their bound textures. Progress bars and fades use their respective overlay producers | refused fans, animation clears, binds and presents | none |
| Movies: tick `0x00156040`, blit `0x001568d0`, quad `0x00156960` | a texture lock and upload, one quad | none | all of it ([bink-hle.md](../audio/bink-hle.md)) | none |
| Alternate view `0x0002c790` (`0x0002c863`, `0x0002c884`, `0x0002c889`), fallback view `0x002232a0` | whatever its scene and UI draw | alternate scene layers 1, 5 and 2 from the layer plan, layer 3 from the world and sorted object stages, first-person layers at their own calls | view setup and finish, refused items | none |
| Target selection `0x0001cd30` (five base sites `0x0002bde2` to `0x0002bfe3`), clears `0x00014b60` | binds, clears | attachment loads and clipped rectangle plans up to eight rectangles, device binds and title-surface reference transactions | longer rectangle lists, final resource destruction, the viewport reset | none |
| Frame end `0x00013b90`, `0x00013cd0`; present `0x00013f10` | a target copy, composite, push words, an optional surface lock, `Swap` | supported copy and composite fans | refused fans, setup, readback and presentation | none |
| Outside the render call: boot, the resource caches, the sim | `CreateDevice`, `AddRef`, `Release`, `IsBusy`, `BlockOnResource`, fences | none | all of it; about 0.1 call a frame from the sim [d] | 0.1 a frame [d] |

The residual census marks `screen_effects`, `colour_pass`, `screen_effect` and `fade` partial. Each guest producer still runs. Entries inside a colour pass or an individual screen effect count toward that child, so the outer `screen_effects` row excludes those entries. The display gamma ramp belongs to the frame tail.

[a] [notes/scene-pass-timing.md](notes/scene-pass-timing.md), dated
2026-11-17, Old Mombasa, headless, commit not recorded, on the D3D path
before any native stage. "D3D entries" are calls of D3D entry points, not
draws. The whole `render` phase was 24.9 ms with the observers on; the ten
listed sites sum to 17.5 ms, and the rest (the per-light passes, the HUD
and UI, the frame end) has no row.
[b] [notes/native-ab.md](notes/native-ab.md), dated 2026-11-20.
[c] [menu-budget.md](../perf/notes/menu-budget.md), commit `064a955`.
[d] [notes/pipelined-render.md](notes/pipelined-render.md), dated
2026-11-18 (`sim_d3d_hits`).

### What the guest does without calling D3D

The static census of the retail image in [d3d.md](d3d.md) bounds kind 2
and kind 3 (**E**, static analysis; it predates every native stage and
does not change):

- 91 D3D functions are reached from game code. The draw entries are
  `DrawVertices` (3 call sites), `DrawIndexedVertices` (8), and
  `Begin`/`End` (11 and 21) with the `SetVertexData*` setters (113).
- 36 inline `BEGIN_END` stores in 22 functions draw without any entry.
  `d3d` sees those words only when its next entry drains the push buffer.
- 386 and 432 direct accesses to the render-state and texture-state
  arrays, 167 references to the dirty flags, and 199 calls of
  `MakeRequestedSpace` from 53 functions.

### Missing data

- No per-caller count of residual draws, D3D entries or state and push
  words exists with the native stages on. Every count above is from the
  D3D path or is a per-stage refusal counter.
- No timing of the front-end by caller exists with the native stages on.
  The last A/B gives only the whole game thread: 34.7 ms against 23.9 ms
  in one late window ([notes/native-ab.md](notes/native-ab.md)).
- The HUD, the UI, the fade, the frame end, movies and loading have never
  been timed or counted by caller on either path.
- Multiplayer maps have no residual data at all. Every census run is Old
  Mombasa or the menu.
- `render.scene`'s entry census (`frame-scene.census`) counts
  `d3d_draws` under the layer draws, the light passes, the object shadow
  and the screen effects, and the scene sink (`frame-scene.sink`) counts
  ops by slot. Both exist; neither has a recorded result in `notes/`.

## Measurement plan

Numbers come from the quiet laptop through `h2 lab`
([lab.md](../runtime/lab.md)), on a window with the GPU renderer, at
render scale 4. Four runs: the main menu, `ascension`, `lockout` and
`03a_oldmombasa`. A multiplayer map started alone has no fight in it; use
`--replay NAME` when a recording of that map exists, and say in the note
which was used.

### What the agent API gives today

Counts and time need separate runs: the census options add observers that
move guest calls off the direct path, so a run that counts does not time.

```sh
B=master
start() {  # name, features, then the level flags
  name=$1; features=$2; shift 2
  h2 lab run --host laptop --name "$name" --branch "$B" --serve "$@" \
    --build-env H2_OPT=2 --env H2_RENDER_SCALE=4 --env "H2_FEATURES=$features"
}
ready() {  # id, map
  if [ "$2" = mainmenu ]; then
    h2 lab agent --host laptop "$1" wait '{"map":"mainmenu","map_ticks":900}' 300000 --timeout 400
  else
    h2 lab agent --host laptop "$1" wait '{"phase":"gameplay","map_ticks":900}' 600000 --timeout 700
  fi
}

# Counts. One run per map; ID is the job id `start` prints.
start "Residual count: lockout" frame-scene.census,frame-scene.sink --level lockout
ready $ID lockout
h2 lab agent --host laptop $ID render.scene > $R/lockout-scene-0.json
h2 lab agent --host laptop $ID capture 900 --timeout 200 > $R/lockout-capture.json
h2 lab agent --host laptop $ID render.scene > $R/lockout-scene-1.json
h2 lab agent --host laptop $ID frame draws > $R/lockout-frame.json
h2 lab stop --host laptop $ID

# Time. A second run of the same map.
start "Residual time: lockout" render-timing --level lockout
ready $ID lockout
h2 lab agent --host laptop $ID capture 900 --timeout 200 > $R/lockout-time.json
h2 lab stop --host laptop $ID
h2 lab fetch --host laptop $ID --to $R/lockout-time    # the log has the site lines
```

The other three are the same with `--level ascension`, `--level
03a_oldmombasa --difficulty heroic`, and no `--level` (the menu; `ready
$ID mainmenu`). **S**: that a `--serve` run with no `--level` stops at the
main menu; if it does not, start any level and send `game.menu` first, as
lab.md shows.

Read from them:

- `capture.json`: `counters.draws` (draws the backend drew, a frame),
  `passes`, and each `native_<stage>_{examined,suppressed,refused_*}`;
  `native_ui_guest_builder_draw`; the phases `cpu`, `d3d`, `decode`,
  `encode`.
- The two `render.scene` replies, subtracted and divided by the capture's
  `frames`: `entries.by_entry.*.d3d_draws` per watched entry, and
  `counters.scene_carried_{draws,ops}`. `carried.slots` of the last frame
  gives draws, binds, clears, copies, uploads and samples for each of
  `opaque`, `after_opaque`, `after_lights`, `transparents`, `after_post`,
  `overlay` and `outside`.
- `frame.json`: one frame's draws with their D3D entry and pass, to name
  what the slot counts are made of.
- `time.json`: the `render` and `d3d` phases. The run log's
  `render-timing:` lines give ms, guest and D3D, and D3D entries for each
  call site of `render_scene_passes`.

What this cannot give: a caller for anything in the `overlay` and
`outside` slots (HUD, fade, UI, text, movies, loading, the frame end),
or under an entry the census does not watch (the sky, layer 2's other
handlers, object shadows as a whole, transparents by handler, the first
person, the flares, `0x0002dd30`, `0x0002df00`, layer 17); any count of
entries or state and push words by caller; and any time outside
`render_scene_passes`. The site lines are also not in the API: they are
only in the log.

### New method: `render.residual`

The census every slice here gates on, and the count the deletion is gated
on, so it is an agent method and not a one-off
([AGENTS.md](../../AGENTS.md), "Use the agent API"). It reports, by
caller, what the guest front-end still asks `d3d` to do.

| | |
|---|---|
| Access | read |
| Params | `{arm?: {frames}, top?}`. `frames` 1 to 3600. `top` 1 to 500, default 50 |
| Result | `{state, window?, totals?, producers?, callers?, unattributed?}` |

- **Arming.** `{"arm": {"frames": 600}}` starts a window at the next
  present and ends it after that many presents. `{}` reads `state`
  (`idle`, `armed`, `done`) and the last finished window. A new arming
  replaces one in progress. `h2 agent residual 600` arms, waits and
  prints per-frame numbers.
- **`window`.** `{frames, game_frames, presents: [first, last], ticks:
  [first, last], map, timing}`. `timing` is true when the run has
  `render-timing`.
- **`totals`** over the window: `draws` (resolved), `claimed` (a native
  replacement committed), `residual` (reached the backend), `entries`
  (D3D entry calls), `push_words` (decoded), `state_words` (render-state
  and texture-state words the deferred commit applied from the guest's
  arrays), `clears`, `binds`, `copies`, `uploads`, `locks`, `queries`,
  `presents`, and `d3d_ms`.
- **`producers`**, at most `top`, by `residual` then `entries`:
  `{producer, status, slot, calls, draws, claimed, residual, entries,
  push_words, state_words, clears, binds, native_rect_clears, clear_refused_rect_count,
  native_guest_binds, guest_bind_refused_destroy, copies, uploads, d3d_ms,
  guest_ms}`. `status` is `native`, `d3d` or `unknown` from the registry
  below. Sky, water teardown, clear, target selection and screen effects
  are `partial`, as are layer draws where a native stage draws some items
  from the scene and skips their guest draws. `guest_ms` is the producer's
  exclusive guest time and is `null` without `render-timing`. A stage's refusals are not filed by producer;
  the capture's `native_<stage>_refused_*` counters give them by stage.
  The totals and producer rows count committed native rectangle plans
  (`native_rect_clears`) and lists above the eight-rectangle bound
  (`clear_refused_rect_count`). Title binds that complete without a
  destruction increment `native_guest_binds`; final releases increment
  `guest_bind_refused_destroy`. The helper rows are `scene_clear` and
  `target_select`.
- **`callers`**, at most `top`: `{function, sites, entries, draws,
  inline_draws, residual, d3d_ms}`. `function` is the analysed guest
  function that holds the return address of the entry call, named as
  `profile.fallbacks` names a function. `entries` maps an entry's name to
  its count.
- **`unattributed`**: `{draws, entries}` with no producer open.

**Attribution.** A D3D entry has a caller: the adapter in
`crates/platform/d3d/src/runtime_glue.rs` reads the return address at
`[esp]` (it already does while a post call is being swallowed) and the
census files the call, its time in the `d3d` phase and any draw the entry
itself resolves under that address. An inline draw has no entry. It is
charged to the producer that was open when its words were written, which
is exact because the device decodes the push buffer before a producer
boundary moves, as it does before the sink's slot moves
(`render_bridge::carried_slots::Drain`).

**Producers** are a fixed registry in
`crates/rewrites/render-bridge/src/residual_census.rs` (`REGISTRY`), one
row per caller in the inventory table that has a named point, each with
its entry point, its name, its status and its slot, and the layer draw by
site and layer as `render_bridge::entry_census` names it. Work goes to the
innermost open producer, so the rows sum to `totals`. Callers with no
point yet (movies, the flares, the untraced `Begin`
callers, the first person's own entry) fall to the producer around them
or to `unknown`, and `callers` names their functions. `outside_render` is
every entry call made with no render frame open (boot, the caches, the
sim). A draw or entry under no registered producer is `unknown`, and
`unknown` is never folded into a neighbour.

**Cost and switch.** `totals` and `callers` are always available: unarmed,
the adapter does one relaxed load per entry. `producers` needs the
registry's observers, which are installed with the existing
`frame-scene.census` option; without it `producers` is absent and
`unattributed` holds the inline draws. No new option. Three frame
counters are added for the gates and are always on: `d3d_entries`,
`d3d_residual_draws` and `d3d_push_words`.

**Code.** Types in `crates/extensions/agent-proto/src/render.rs`, logic in
`crates/extensions/agent/src/render/residual.rs`, the registry and window
in `render_bridge::residual_census`, the adapter's hook in
`crates/platform/d3d/src/residual.rs` and `runtime_glue.rs`, glue in
`crates/extensions/game/src/scene_glue.rs` and `d3d_glue.rs`, the CLI
short form (`h2 agent residual N`), and a section in
[agent-api.md](../runtime/agent-api.md#renderresidual-residual-d3d-work).

**Tests** (synthetic, the device's draw rig): entry calls from two return
addresses land in two caller rows; an inline draw written under one
producer and drained under the next is charged to the first; a claimed
draw counts once in `claimed` and not in `residual`; `totals.residual`
equals the growth of `d3d::device::backend_draws`; an entry outside every
producer is `unknown`.

With it, each of the four runs is one run and three calls:

```sh
start "Residual: lockout" frame-scene.census,render-timing --level lockout
ready $ID lockout
h2 lab agent --host laptop $ID render.residual '{"arm":{"frames":900}}'
h2 lab agent --host laptop $ID capture 900 --timeout 200 > $R/lockout-capture.json
h2 lab agent --host laptop $ID render.residual '{"top":500}' > $R/lockout-residual.json
```

`guest_ms` from a run with observers ranks producers; it is not a frame
time. Frame time for a slice's payoff is the A/B of framescene.md
section 12, without either option. Results go in a new
`notes/retire-d3d.md`, dated, with the commit.

## How it works

### The frame

`render_frame_and_present` `0x00223240` is the frame step, and it is
replaced whole (**Design**). The replacement runs on the sim thread after
the frame's ticks:

1. **Writes the tick reads.** It makes the writes of framescene.md
   section 5.2 that the guest front-end made, in the guest's order: the
   render frame counter, the game-time bracket, the present counter and
   frame timing, the two random seeds, the particle systems' view stamps,
   the animation cache's use stamps, and the residency requests.
2. **Extraction.** `scene-extract` reads guest memory into a
   `FrameScene`. It is the only render code that reads the guest, and it
   reads only what the sim owns.
3. **Seal and publish.** The render thread draws the scene. It never
   reads guest memory. The window composites the newest finished frame.

Every fact the renderer needs is in one of three places:

| Source | Examples | How it is read |
|---|---|---|
| Tags | world geometry, models, materials, lights, effects, HUD and UI definitions, fonts, bitmaps | from the map file at load, with no guest read (`tags`, `map`; the material path already works this way) |
| Sim state in the arena | object, effect, particle, light and player datums; the scripted camera; the UI's screens and widgets; the HUD's unit and weapon state | `scene-extract`, each frame, on the sim thread |
| Products of the guest front-end | view records, visibility lists, render records, the render-state cache, device vertex constants, the device's fog words, the glyph cache texture | not read. Each is computed in Rust from the two rows above |

The third row is the work. A typed stage that still reads a front-end
product (`program::ViewInputs` built from the device's vertex constants,
the fog stage's device words, the post stage's strip rows, the UI's
completed corners) cannot have its guest producer removed until that
input is computed in Rust. The count of such readers is in
[notes/editor-guest-reads.md](notes/editor-guest-reads.md): of 113
classified readers, 73 read run state and 30 start from it.

### What replaces each subsystem

| Subsystem | Guest code that stops running | Replacement | State today |
|---|---|---|---|
| Views and cameras | `0x002234e0`, `0x00223080` | `scene-extract` builds the view records from the observer cameras and writes them, since other code reads them | the main view's viewport layout, render camera, projection terms, start cluster and mode built beside the guest's and compared field by field; the visibility takes the Rust camera when equal ([native-visibility.md](native-visibility.md#the-view-records)). Cinematic and override cameras remain guest-owned. The comparison writes nothing |
| Visibility | `0x00040e90`, the portal traversal, the light frustum setup `0x000c1d90` | a Rust traversal over the BSP's clusters and portals; the Rust scene lists consume it | lists built; every game view's traversal (portal flood, sphere mode, the visible set and its run-time rows) and object set, tested against the guest's window frusta, built beside the guest's, compared as sets, and carried by the view, the default and the guest's lists the fallback a decline takes; the sphere and frustum light views compared ([native-visibility.md](native-visibility.md)); verified portal, sphere and PVS calls publish from Rust; the guest owns source-window overlap records |
| World | the layer draws of 1, 3, 5, 7, 8, 20 over structure | `native_render::world` and `layer_pass` from tags, whole-call (framescene.md slice 4) | built for layer 3; for 1, 5, 7, 8 and 20 the structure's static items, and objects' items of 1, 5, 7, 8 and 20 |
| Objects | `OBJECT_RENDER_SETUP` `0x00133790`, the layer-3 handlers, the first person | the Rust object setup and typed instances; a part the walk cannot read is a named drop, not a guest draw | built, with a remainder |
| Decorators | builds `0x0004de20`, draw `0x0004e260` | `native_render::decorators` ([native-decorators.md](native-decorators.md)) | built, not verified |
| Sky, layer 2 | `0x0003ede0`, the layer-2 handlers | a sky stage from the scenario's sky and its model; layer 2 from the scene's plan | layer 2 from the plan at the main, alternate and first-person calls; per-view sky model strips with resolved materials and stencil, with packet fallback |
| Lights and shadows | `0x00031c80`, `0x00031170`, `0x000335e0`, `0x00032350`, `0x000404b0`, gels | typed light groups and shadow layers ([native-lights.md](native-lights.md), [native-shadow-layers.md](native-shadow-layers.md)), the entries as Rust bodies ([native-light-gather.md](native-light-gather.md)); the renderer's own selection in E3 mode ([e3-lighting.md](e3-lighting.md)) | partly built |
| Effects and particles | the producers, their property evaluation, ribbon construction, `0x00174420`, the sorted dispatch `0x0001e370` | a Rust effect pass that walks the effect and particle datums, makes the same draws from the effects seed, writes the same stamps and emits instanced quads and ribbons | packet only |
| Decals | the D3D backend draw when admission succeeds | a native quad packet from the callback's resolved array draw ([native-decals-research.md](native-decals-research.md)) | packet only. The guest walks and maintains the decal pool. |
| Fog, water, camouflage, hologram | layers 16, 6, 18, 19, 21 | typed late passes sampling the scene's depth and colour on the GPU | fog, water, 18, 19 and 21 partly built |
| Lens flares | `0x00020bb0`, `0x00020770`, the visibility entries | a native query pass; the flare's own fade state is advanced in Rust from its result | Native counted and colour fans, fade state guest-owned |
| Post | `0x00013870`, `0x000214f0`, `0x0002d140`, `0x0002dd30`, `0x0002df00`, layer 17 | typed post from the scenario's screen effects and the camera's parameters | glow and screen effects built |
| HUD | `0x001917eb`, `0x0013954b`, the widgets `0x0022b397` to `0x0022bca4` | a Rust HUD that walks the HUD definitions against the unit's and weapon's state and emits a draw list. It is the same HUD kit the added HUD pieces use | the live shader quad and glyph callbacks emit overlay items with the evaluated widget shader and layer inputs |
| Menus and UI | `0x00147af1`, the widget sort `0x002353a5`, each widget's draw | a Rust walk of the guest's screens and widgets (the UI's logic, input and state stay the guest's) that emits a draw list: bitmaps, text runs, model widgets, the backdrop's blur chain | the walk emits the overlay's items (`scene_extract::ui::draw_list`, from a `UiFrame`); at each sorted list the guest's widgets are read and runs of exact ones drawn from the overlay with their draws skipped ([native-post-hud.md](native-post-hud.md#the-menus-on-the-overlay)). Containers and plain bitmap widgets read exactly; a shader bitmap's quad is drawn from the overlay at the shader quad with its template's combiner; the blur under a dialog is the overlay's `Blur` item, drawn in place of its quads ([native-post-hud.md](native-post-hud.md#the-blur-under-a-dialog)); supported text, lists and buttons draw from the sorted claim. Active model scenes use the model material hooks |
| Text | glyph decode `0x0001f5c0`, the glyph cache, one quad per glyph | Rust shaping from the font data ([fonts.md](../formats/fonts.md)) into the renderer's own atlas, one draw per run | not built |
| Movies | `0x00156040`'s blit and quad, `BinkCopyToBuffer` | the decoder's host frame goes to the renderer as a scene item; no guest texture is locked | not built |
| Loading, fade, bars | `0x000289e0`, `0x00040890`, `0x00052ce7` | draw-list items from the same state words the guest reads | each draw taken at its producer onto the overlay with the guest's combiner, blend and blend constant at the draw: all six fade modes, the bars with the combiner they inherit, the loading screen's bitmaps and spinner ([native-post-hud.md](native-post-hud.md#the-fade-the-bars-and-the-loading-screen)); `scene_extract::ui`'s typed `UiFrame` draws are not filled from them |
| Final display | `0x00013b90`, `0x00013cd0`, `0x00013f10`, `Swap` | the publish. Vblank, the logical frame wait and the gamma ramp move out of `d3d` into the frame step; the screenshot readback becomes a `RenderFeedback` message | not built |

The HUD, the UI, text, movies, loading and the fade share one typed
stage, `frame_scene::Overlay` (`FrameScene::overlay`): an ordered list of
items, each with a material key and a clip rectangle, drawn by
`native_render::ui_gpu::OverlayGpu`. The stage has `Quad`, `TextRun`,
`Fill`, `Blur` and `ModelView` items (`frame_scene::overlay`);
`MovieFrame` joins it with R8. Positions
are the guest target's pixels, so a producer writes what the guest's
screen transform would have made, and the renderer scales them and the
clips to its own target. A text run is one font's glyphs, each a glyph
index and a rectangle laid out by `font::layout`; the renderer's glyph
atlas (`native_render::overlay::GlyphAtlas`) supplies their coordinates.

`native_render::overlay::OverlayPlan` turns the list into one vertex and
index list and its batches: consecutive items with the same material
and clip draw as one, and nothing is reordered. Its materials are named
Rust programs for the fixed combiners the screen-quad path builds
(`Combiner`: flat, texture, texture alpha for glyphs, texture luma), and
translated tag shaders for the rest (`Program::Shader`). A blend is the
guest's D3D factors and equation by value, read from the running image
by the producer, never committed. An item the renderer cannot draw (a tag
shader without a supported combiner; a clip off the target; a glyph the
atlas lacks; a texture that is not resident or a blend that is not a D3D
blend) is dropped and counted: the overlay has no guest draw behind it.
`render.stats` reports the counts as `native_ui_overlay_*`
([agent-api.md](../runtime/agent-api.md#renderstats)).

A `Blur` item blurs what the items before it drew, inside its clip: the
menus' backdrop under a dialog. It carries the guest's steps by value
(each step's spread, the tap's offset over it, and the constant of the
combiner that blends the taps, `pick`), and the plan turns the spreads
into offsets that are fractions of the target, so a blur looks the same
at any scale. The plan ends the batch before a blur
(`native_render::overlay::PlanBlur`); `OverlayGpu` ends its pass there,
copies the target, runs each step as four bilinear taps blended in pairs
by the guest's combiner between two scratch targets, and writes the
result back through the texture combiner into every channel inside the
clip. Over a target it cannot copy or sample, every blur is dropped and
counted (`refused_blur`). In the game the blur under a dialog is drawn
this way in place of the Kawase blur's quads
([native-post-hud.md](native-post-hud.md#the-blur-under-a-dialog)). A `ModelView` item is a
model widget's camera and objects with its viewport as the clip. No
stage draws one yet: the plan drops it and counts it
(`refused_model_view`).

The HUD editor and HUD kit preview use `hud_edit::overlay::draw_list`.
In the game, each bitmap or glyph is captured at its draw callback with
the actual corners, inputs, bound image and evaluated shader constants.
The guest still selects the visible widgets and places each shield
layer and each glyph. The overlay executes supported widget combiners
at their place in the frame.
See [the HUD on the overlay](native-post-hud.md#the-hud-on-the-overlay).
The renderer's glyph atlas is `native_render::overlay::GlyphPages` (one
page per font, filled by the host from the fonts as the game serves them:
`game::hud_overlay::fill_glyphs`, each glyph decoded once and uploaded
once by `ui_gpu::OverlayResidency`).

The UI's producer is `scene_extract::ui::draw_list`. Its input, a
`UiFrame`, is the guest's UI draws in call order: the fade, the solid
rectangles of `0x00052ce7`, the loading draw's bitmaps and the screens
of `0x00147af1`, each screen with its widgets' headers (type, depth,
bias, rectangle, animated offset, alpha) and what each draws (a bitmap,
a shader bitmap, text, a model view, or nothing for a container). Each
screen's widgets are drawn in the order of the guest's comparator
`0x002356d8` as the C runtime's `qsort` applies it (`sort_widgets`: the
deeper first, ties within 0.001 by bias, smaller first, then by type,
larger first; not stable), and a dialog's screen first draws its backdrop
as a fill and a blur. Text is laid out by the caller's fonts (`UiText`),
clipped to the widget's rectangle. A shader bitmap names its shader, so
it is refused until tag shaders translate.

The game reads the guest's widgets on the sim thread at each sorted list
(`render_bridge::native_hud::ui_overlay`), in the guest's own sorted
order: each drawn widget is read (`scene_extract::ui::read_widget`) and
listed, consecutive exact ones are drawn by one overlay pass placed at
the first one's draw, their draws are skipped, and the list's skipped
draws are checked against its claim ([native-post-hud.md](native-post-hud.md#the-menus-on-the-overlay)).
`read_widget` reads containers, and `ui_overlay::bitmaps` a bitmap
widget's quad. The class readers also claim supported text, lists and
buttons. Meters, scrolling bitmaps and unsupported widget states keep
their guest builders. Eligible screen quads from those builders reach
the overlay at submission. Sorted-list refusals are counted by reason
(`native_ui_overlay_menu_refused_*`). The fade, bars and loading screen
are not read into a `UiFrame` yet; their quads are the producers of
[native-post-hud.md](native-post-hud.md#native-ui).

### What stays the same, and the proof

| What | Bar | Proof while both paths exist |
|---|---|---|
| The sim | identical: the gameplay digest of every tick, and the arena hash under `impl.sim.arena-hash` | `sim.hashes` on two launches, `impl.render.native=off` against the default, row by row (framescene.md 9.3). Every slice, before anything else |
| Films and saves | a film recorded before a slice replays to the same hashes after it | replay of a recorded film with `sim.hashes` |
| Pixels | framescene.md 9.2's bar: no channel off by more than 8, at most 0.01% of pixels off by more than 2 | `render.parity` in the same process (the guest front-end drawn into the D3D targets, the same tick drawn natively into private targets), and two launches compared with `h2 agent frame compare` on the gate corpus |
| Text | identical at 1x: same glyph, same texel, same pen position | `render.parity` on menu and HUD frames with `max_channel == 0` over the text's rectangles |
| Draw order of blended content | identical order for transparents, the overlay and the UI's sorted widgets | a test per stage that the typed list's order equals the guest list's, on synthetic lists and on dumped scenes |
| In-between frames | at `alpha = 1` identical to the tick's frame | framescene.md 9.2 |

`render.parity` needs the guest front-end to be runnable on demand after
its entry is replaced. It is, because a replacement makes a superset of
the entry's tick-visible writes and the hash gate has shown that. When
the draw path is deleted the reference is the tag `d3d-reference`
(framescene.md section 11), and pixel tests compare against captures made
from it on the same film. Those captures are game images and stay in
local scratch.

## Slices

Sizes are lanes of one Opus session: S under a day, M one to three, L
more, to be split. Payoff is the ceiling from the D3D-path census [a]:
what the caller cost before any stage, so what is left of it is at most
that. Slice R0 replaces every payoff figure with a measured one, and the
order after R0 follows the measured table, not this one. "ms" is guest
front-end time on the game thread a frame on Old Mombasa; "entries" are
D3D entry calls.

| Slice | What | Size | Needs | Payoff (**Estimate**, ceiling) | Proof |
|---|---|---|---|---|---|
| R0 | `render.residual`, the three counters, the baseline on the four maps | S | nothing | none; it orders the rest | the method's tests; `totals.residual` equals `draws` in the same capture |
| R1 | The opaque block whole: layers 1, 3, 5, 7, 8, 20, objects, decorators, first person. framescene.md slices 3 and 4 | L | R0; zero `walk_unsupported` and `layer_remainder` | 9.4 ms, 2,340 entries | 8.2's four rules; `producers` rows for these layers at zero |
| R2 | Transparents and effects: the sorted dispatch, particles, contrails, decals, effect quads, in Rust | L | R0; the effects seed and `0x00174420` stamps reproduced | 2.6 ms, 1,010 entries | hash gate; order test; parity on explosion frames |
| R3 | Lights and shadows whole: the gather, per-light passes, object shadows, first-person lights, gels | L | R1 | 3.5 ms plus the per-light passes, 355 entries | hash gate; parity on lit interiors at 1x and 4x |
| R4 | Visibility and view records in Rust | M | R1 | 0.9 ms, no draws | cluster and object sets equal the guest's on dumped scenes; hash gate |
| R5 | The rest of a game view: sky and layer 2, fog and water remainders, layers 17, 18, 19, 21, `0x0002dd30`, `0x0002df00`, the post remainder, flares, clears and binds | M | R1; the unread callers read | 1.1 ms, 284 entries | parity per effect; `scene_readback` zero |
| R6 | `Overlay` stage and the HUD | M | R0 | not measured | text and HUD parity at 1x exact; HUD frames on every weapon and vehicle |
| R7 | Menus, text, loading, fade, bars, model widgets, the backdrop | L | R6 | not measured; the menu's draws less its 3D view | menu, settings, pregame lobby and loading captures; UI order test |
| R8 | Movies, frames outside a game view, the final display. `0x00223240` is replaced | M | R1 to R7 | the rest of `render` and all of `d3d`, `decode` | from boot to gameplay and back to the menu, `d3d_entries` grows only under `outside_render` |
| R9 | Zero gate: the census corpus with `render.residual`; `impl.render.native=off` stops being built by default | S | R8 | none | the gate in "Rules and limits" |
| R10 | Delete `d3d`'s draw path and the backends that replay it | M | R9, a soak the owner sets | build time and about 30,000 lines | the tree builds; the gate corpus against `d3d-reference` |
| R11 | Delete the crate | M | R10; the decision on non-draw entries below | the last guest D3D call | no `d3d` in `cargo metadata`; boot to menu on the laptop |

R1, R2 and R6 do not depend on each other and can run as three lanes
after R0. framescene.md slice 8 (60 and 120 fps from two scenes) is
independent of all of them once its slice 6 holds.

**R10 deletes**: `pushbuf.rs`, `state.rs`, `nv2a.rs`, `nv2a_draw.rs`,
`nv2a_hot.rs`, `shader.rs`, `vertex_cache.rs`, `dirty.rs`, `regs.rs`,
`glyph.rs`, `pipeline_store.rs`, `device/` except what R11 lists,
`backend.rs`, `gfx_backend.rs` and `gfx_backend/`, `scaled.rs` and
`scaled/`, `threaded.rs` and `threaded/`, `scene_sink.rs` and
`scene_sink/` (with `retarget.rs`, moved to the native side first),
`scene_stage.rs`, `clip_space/`; `DrawAdmission` and every `render_bridge::native_*` hook;
`frame_types::carried` (the `frame-types` crate's carried part); in `gfx`, what only a resolved D3D draw uses
(`state.rs`, `translate.rs`, `xdk.rs`, the D3D pipeline key); the
`render-timing` and `render-census` features and `render.parity`.
`formats/shaders` stays: the native stages translate tag shaders with it.

**R11 deletes** `abi.rs`, `runtime_glue.rs`, `resource.rs`, `layout.rs`,
`guest.rs`, `hw.rs`, `vblank.rs`, `events.rs`, the crate, its dependents'
edges (`boot`, `game`, `cli`, `h2host`, `uwp-host`, `tools/perf`),
`impl.render.native`, and [d3d.md](d3d.md)'s device model. The entry
inventory in d3d.md is a fact about the retail image and stays as a
reference page.

## Rules and limits

- **No quiet fallback.** While both paths exist, content that is not
  native is drawn by D3D and appears as a `producers` row with status
  `d3d` and its counts. Nothing is drawn by a second native
  approximation, and nothing is skipped.
- **One registry.** A producer's status changes from `d3d` to `native`
  in the slice that replaces it, in the same commit as its test. A layer
  draw some of whose items a stage draws from the scene, skipping their
  guest draws, is `partial` (`residual_census::PARTIAL_LAYERS`) until
  the slice leaves it nothing.
  `the_residual_registry_names_every_producer` (new, in `render-bridge`)
  fails when a hook draws for a producer the registry does not list.
- **A replaced producer cannot fall back.** After its slice, content the
  stage cannot draw is a named drop: `scene_dropped_<stage>`, one log
  line per cause, a row in `render.scene` (framescene.md 8.2). A drop is
  a bug with a name, and the gate corpus requires zero.
- **Unknown is a failure.** A slice's gate requires `unknown` and
  `unattributed` at zero on the gate corpus.
- **The zero gate (R9).** Over the census corpus of framescene.md 9.1
  (every campaign level and every multiplayer map, 3,000 presents each
  from boot) plus the menu, a movie, a loading screen, split screen and a
  film playback: `d3d_residual_draws` zero, `d3d_push_words` zero,
  `d3d_entries` zero inside the render frame, every `scene_dropped_*`
  zero, `scene_readback` zero, and hashes equal to `native=off`. R10 does
  not start on a waiver.
- **Not for the removed path:** no stage is patched to claim more on the
  resolved-draw path unless that work survives into its typed form. The
  per-draw admissions are deleted in R10.
- **Refused.** Reading a guest front-end product from the render thread;
  defining a new random stream to avoid reproducing the seed draws (that
  breaks films and co-op, and is the owner's call); committing any table,
  shader word or constant read from the image.

## Risks

- **Tick-visible writes under the front-end.** The shared seed, the
  effects seed and the particle stamps decide how much guest code R1 and
  R2 can drop. If they cannot be reproduced in order, the guest functions
  that make them keep running and those slices save less.
- **Code nobody has read.** The remaining untraced `Begin` callers and `0x0002c370`'s seven calls. R5's size is a guess until they are read.
- **The UI is large.** The widget system is the guest's and stays. R7
  ports only its drawing, but every widget kind and every screen has to
  be covered before `0x00147af1` can go.
- **Memory.** While both paths exist, textures and geometry are resident
  twice: once through the guest's caches and `d3d`, once from tags.
- **Other content.** Converted Halo 1 maps, community map packs and the
  UI-kit menus draw through the same producers and must be in the census
  corpus before R9.
- **Readers of the frame.** Film thumbnails, screenshots and Theater read
  the presented frame; each needs its `RenderFeedback` answer before R8.
- **Hosts.** `h2host`, `uwp-host` and sim mode install `d3d` over a
  recording backend to answer the game's non-render calls. R11 changes
  what they link.

## Not verified

- Every count in the inventory with the native stages on.
- Whether `impl.render.native` on and off give equal hash series today.
- Whether any guest code outside the front-end reads a front-end product
  (a render record, a visibility bit) between frames.
- What the four untraced `Begin` callers and layers 17 to 21 draw.
- Whether the guest's resource caches make any D3D call that changes what
  the sim sees, beyond the residency rows of framescene.md 5.2.
- Whether text shaped in Rust lands on the guest's texels at every font
  and scale.

## Related

- [framescene.md](framescene.md): the scene type, the frame step, the gates and the slices this page continues.
- [native-renderer.md](native-renderer.md): the stages, admission and the residual callers.
- [d3d.md](d3d.md): the crate being retired and the retail entry inventory.
- [render-loop.md](render-loop.md) and [native-post-hud.md](native-post-hud.md): the front-end's call tree, the HUD and the UI.
- [sim-rewrite.md](../engine/sim-rewrite.md): who owns state; the effects phase R2 shares.
- [e3-lighting.md](e3-lighting.md): the lighting that draws from the same scene.
- [native-visibility.md](native-visibility.md): R4's view records, traversals, frusta and object sets, and their differentials.
- [notes/scene-pass-timing.md](notes/scene-pass-timing.md), [notes/native-ab.md](notes/native-ab.md): the measurements cited above.
- [agent-api.md](../runtime/agent-api.md), [lab.md](../runtime/lab.md): the methods and the laptop runs.
