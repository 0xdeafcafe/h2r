# Native light gather and passes

Status: Partly built. Slices L1 and L2 are built: the model's rows and histories and the leaves (the visible-light list, the geometry flags, the extra caster, the light volume) in `engine::render::light_gather`, which the seams of [native-renderer.md](native-renderer.md), "Scene list builders", call; and the frame body of `0x00031c80` (`light_gather::frame`), which `render_bridge::native_lights::frame` installs at `RENDER_DYNAMIC_LIGHTS`. L3 on are design. The other leaf ports it starts from (the light constants, the pass flags, the shadow projection, the gathered groups) are described in [native-lights.md](native-lights.md).
Code: `crates/rewrites/engine/src/render/light_gather.rs` and `light_gather/` (the model and the bodies), `crates/rewrites/render-bridge/src/native_lights/frame.rs` (the install), `crates/rewrites/render-bridge/src/native_scene_lists/lights.rs` (the leaf seams that stay for the fallback). Tests: `crates/rewrites/engine/tests/light_gather_differential.rs`, `light_gather.rs`, `crates/rewrites/render-bridge/tests/native_lights.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md). No slice adds a switch.

Slice R3 of [retire-d3d.md](retire-d3d.md): the guest's light front-end
stops running. Three guest entries draw lights: the dynamic lights
`0x00031c80` with its per-light passes `0x00031170`, the object shadows
`0x000335e0`, and the first-person lights `0x000404b0`. Each becomes a
Rust body that owns the decisions as typed data (a `LightFrame`), makes
the guest's writes while a guest consumer is left, and puts the lights,
their groups and their passes into the `FrameScene`. The one rule: a body
that has made its first guest write finishes the frame; it never hands a
half-run frame back to the guest function.

Evidence labels as in [docs-style.md](../meta/docs-style.md). Addresses
and offsets are research points. No game data.

## How it works

### The three entries and their phases (Design)

Each entry is a `before` hook that plans synchronously and then answers
with a render entry (`hooks::Call::answer_render_async`,
`crates/hooking/hooks/src/entry.rs`), or returns `Flow::Continue` so the
guest function runs whole. A light has two phases:

- **Gather.** Which lights, their weights, the pose, the volume, the
  shadow decision, the light's visibility, its render records and its
  two item lists. It makes no D3D call (**Run**:
  [notes/scene-pass-timing.md](notes/scene-pass-timing.md) counts zero
  D3D entries for the site outside the per-light passes).
- **Passes.** The light's constants, the scissor, the gel, the shadow
  projection, and layers 10, 11, 14, 12 and 13.

A body calls a guest function with `Cx::call` wherever the Rust form
is not built or refuses a case. The slices replace those calls one at a
time. Three groups of guest calls outlive R3 and belong to R4
(retire-d3d.md, "Visibility"): the light frusta `0x000c1d90`, the
traversal inside `0x001320f0`, and the part visibility `0x00044720`.

### The data model (Design)

All of it lives in `engine::render::light_gather`. Floats that the guest
only copies are kept as their bits, so NaNs and stale words survive.

```rust
/// One row of the visible list (0x14 bytes at the guest's buffer). The
/// four bytes are kept as written: the ranking tests each against zero.
pub struct VisibleLight {
    pub local_owner: u8,     // +0
    pub first_person: u8,    // +1: definition bit 5
    pub owner_kind_2: u8,    // +2: the owner's object type byte is 2
    pub keep: u8,            // +3: definition bit 20
    pub light: u32,          // +4: the light datum
    pub root_owner: u32,     // +8
    pub weight: u32,         // +0xc, f32 bits: the record's on the way in, the faded one on the way out
    pub priority: u32,       // +0x10, f32 bits
}

/// One row of a per-view history (0x24 bytes).
pub struct HistoryRow {
    pub visible: VisibleLight, // +0..+0x14
    pub rate: u32,             // +0x14
    pub weight: u32,           // +0x18
    pub seen: u32,             // +0x1c: the render frame it was last a candidate
    pub first: u32,            // +0x20: the render frame it entered
}

/// A view's history: up to 32 rows, the count at `+0x480`. A removal
/// copies the last live row over the removed one, so a slot past the
/// count keeps what was last written there: `slots` is every slot
/// written since the decode, the first `count` live.
pub struct History { pub slots: Vec<HistoryRow>, pub count: usize }

/// What one call of `0x00031c80` decides, in draw order.
pub struct LightFrame {
    pub view: u8,
    pub render_frame: u32,
    pub lights: Vec<FrameLight>,
    pub first_person: FirstPersonLights,
}

pub struct FrameLight {
    pub row: VisibleLight,
    pub drawn: bool,            // weight above the threshold word
    pub definition: u32,        // the `ligh` datum
    pub definition_flags: u32,
    pub owner: u32,             // record +0x4c when +0x54 is not -1, else -1
    pub root: u32,
    pub repose: Repose,         // None | Asked { answered: bool }
    pub volume: [u32; 31],
    pub slot: Option<u8>,       // its first-person slot
    pub shadow: bool,           // the gather's decision
    pub source_vis: bool,       // restricted to the main view's clusters
    pub cluster: i32,
    pub frusta: u16,
    pub records: Range<u16>,    // its render records
    pub receivers: ItemRange,   // list 1
    pub casters: ItemRange,     // list 2
    pub passes: Option<LightPasses>,
}

pub struct LightPasses {
    pub shadow: bool,           // after the caster list's layer-10 bit
    pub flags: u32,
    pub steps: SmallVec<[frame_scene::LightStep; 6]>,
    pub additional_casters: bool,
}

/// The table at `0x004c6700`: eight slots of `0x408` bytes.
pub struct FirstPersonLights { pub slots: ArrayVec<FirstPersonSlot, 8> }
pub struct FirstPersonSlot {
    pub light: u32,                        // +0x400
    pub records: ArrayVec<[u8; 0x20], 32>, // +0, count i16 at +0x404
}
```

`ObjectShadowFrame` has the same shape for `0x000335e0`: its rows, its
`History`, and per caster the two arguments of `0x00032350`.

**What stays in guest memory.** The two histories are the only state
that crosses frames. They stay at their guest addresses and the model
decodes and encodes them each frame, so the installing feature stays
`SnapshotPolicy::Stateless`, and snapshots, films,
`impl.render.native=off` and `render.parity` see one copy. Every other
guest write below is a mirror for a guest consumer (the fallback path,
the handlers a refused group still runs, `render.parity`) and is deleted
with that consumer in R10.

### The scratch frame (Design, from E)

`0x00031c80` keeps its locals in `0x1508` bytes below its entry `ESP`
and passes pointers into them to its callees (**E** `0x00031c80`,
`0x00031c85`). The body uses the same bytes at the same offsets, so a
guest callee is handed the pointers the guest would hand it and the
differential transcripts are equal. With `E` the entry `ESP` and
`S = E - 0x1518`:

| At | Holds |
|---|---|
| `S+0x10` | the shadow byte; `0x00031660` is handed its address, `0x00031170` its dword, whose upper bytes are those of `S+0xb0`, the visible list's argument pushed there (**E** `0x00031c91`) |
| `S+0x18` | the frusta count `0x000c1d90` writes (i16) |
| `S+0x34` | the posed volume, `0x7c` bytes |
| `S+0xb0` | the visible rows, up to 128 of `0x14` bytes |
| `S+0xab0` | the frusta, up to six of `0x1bc` bytes |

`Cx::call` places a call's frame at the entry `ESP`, which would
overwrite these. `GuestCall::reserve(bytes)` makes the call with `ESP`
lowered by that much first (`crates/hooking/guest-layout/src/entry.rs`,
the driver in `crates/hooking/hooks/src/entry.rs`, and
`entry::testing::run`, which writes the arguments where the driver
pushes them). Every call a light body makes reserves its frame.

The rest of the frame is private: the saved registers (`S..S+0x10`),
the row cursor (`S+0x14`) and `S+0x1c..S+0x34` (`D`, the cluster, the
root, the rows left, the source visibility, the datum). No callee is
handed them and the body keeps them in Rust.

### The dynamic lights, step by step (E `0x00031c80`..`0x00031f4f`)

`L` is the light record, `D` the definition, `V` the light's visibility
struct `0x0054aa68`.

| # | Step | Reads | Writes | Guest function |
|---|---|---|---|---|
| 1 | The list | as `light_gather::visible::list` reads | the view's history and its count; the rows at `S+0xb0` | `0x00031910` (`0x0004a7c0`, `0x0004ac30`): Rust |
| 2 | Per row: drawn | row `+0xc` against the word at `0x0045dc58` (`comiss`, `jbe`: a NaN is not drawn) | none | inline |
| 3 | Record and definition | `[0x004e030c]+0x44`, `L+4`, the tag table `[0x004e3b44]` | none | inline |
| 4 | Owner and root | `L+0x54` (i16), `L+0x4c`, the object headers `[0x004e0300]+0x44` (stride `0xc`), each body `+0x14` | none | inline |
| 5 | Re-pose, when `L+0x54` is not -1 and `D` has bit 5 without bit 18 | the marker name of the owner's attachment `L+0x54`; the first-person state `0x0016640f` reads | `L+0x84`, `L+0xac`, `L+0xb8` when the lookup answers; always `L+0x90..+0x98` = `+0x84` + `+0xac`, and `L+0xb8` made orthogonal to `+0xac` and normalised | `0x000b8c40` (ECX owner, AX the index), `0x0016640f` (ECX owner; marker, `&L+0x84`, `&L+0xac`, `&L+0xb8`; `ret 0x10`), `0x00031b40` (EAX `&L+0xb8`, ECX `&L+0xac`; x87, tail call to `0x00030bf0`) |
| 6 | Volume | as `light_gather::volume::light_volume` reads | `S+0x34` | `0x000c17f0` (EAX `S+0x34`, EDX the datum, one stack word 1, `ret 4`); AL is ignored |
| 7 | First-person slot | `[0x00509440]` | when below 8: `[0x004c6b00 + 0x408·n]` = the datum, `[0x00509440]` = n+1 | inline |
| 8 | Shadow | `D` bit 3 clear, the byte `[0x004ba020]`, `L+0xf8` above the zero word, the volume's kind not 0; the byte `[0x004ba010]` (set: no source visibility) | `S+0x10` | inline |
| 9 | Cluster | `L+0x48` (i16); when -1 and `D` has bits 5 and 18: `[0x004b9ee4]` | none | inline |
| 10 | Frusta | `L+0x84`, the volume | `S+0xab0`, `S+0x18` (six for kind 0, else one) | `0x000c1d90(&L+0x84, S+0x34, S+0xab0, S+0x18)`, `ret 0x10` |
| 11 | Visibility | the frusta, `L+0x18`, `L+0x24`, the source struct | `V` whole; `[0x004c0b80]` = `V`; the epochs `[0x004de2fc]`, `[0x004e0308]`, `[0x004e7c1c]` and their flag bytes; the stamps the two Rust gathers write | `0x001320f0`: EAX 1, ECX the count, DL 0; stack `((D flags >> 12) & 0x400, S+0xab0, &L+0x18, L+0x24, 0x00547f88 or 0, 0, cluster)`; `ret 0x1c`; returns `V` |
| 12 | Geometry flags | as `LIGHT_GEOMETRY_FLAGS` | `V`'s flag columns, `L+0xf8` | `0x00031660`: Rust |
| 13 | Extra caster, `D` bit 22 | as `LIGHT_EXTRA_CASTER` | `V`'s object list | `0x00031c20`: Rust |
| 14 | Records | `V`'s three lists | `[0x004c0b78]` = `[0x004c0b7c]`, then the records `0x004ba138 + 0x20·i`, the bytes at `0x004c0b84 + i` and `0x004c0ed6 + i` | `0x0002cb30`; `0x00133390` (EBX `V`); `0x00133520(V)`; `0x00133720` (EAX `V`, one stack word 1) |
| 15 | Part visibility | the records, `V` | the records' subpart masks | `0x00044720` |
| 16 | Passes | below | below | `0x00031170`: EAX the `ligh` datum, ECX `S+0x34`; stack (datum, `&L+0x84`, the dword at `S+0x10`, the row's weight) |

In `V`'s kind (`[V+0x2a78]` not 0) the record writer `0x0002cbf0`
advances `[0x004c0b78]` only (**E** `0x0002cf64`), so a light's records
are `[0x004c0b7c, 0x004c0b78)` and each light reuses that tail.

### One light's passes (E `0x00031170`..`0x00031518`)

1. **The item lists.** `0x00040e90` with EAX 0. In the light's kind it
   zeroes both lists' intervals and masks (`0x004c1a64`, `0x004c1a68`,
   `0x004c1a6c`; `0x004c1a7c`, `0x004c1a80`, `0x004c1a84`), runs the
   attachment walk `0x00041c80`, and calls `0x00044ac0(kind, -1, 0x1f)`
   (**E** `0x00040ee0`..`0x00040f0d`). `0x00044ac0` walks the light's
   records. A record with flag bit 10 is copied whole into the slot of
   `[0x00509440] - 1` when that slot holds under 32 (**E**
   `0x00044b3a`..`0x00044b66`). Any other record that passes the filters
   gives each visible part to `0x00041040`, which appends its item to
   list 1 when the record has flag bit 1, and to list 2 when it has
   bit 2 (**E** `0x00041319`, `0x00041351`; the chooser `0x00040de0`).
2. **Shadow.** A set shadow byte becomes bit 10 of the caster list's
   mask `[0x004c1a80]` (**E** `0x0003119b`).
3. **Constants.** `0x0004a1e0`
   (`engine::light_constants::light_globals`), which also asks
   `0x001650a0` for the sphere's screen bounds and runs `0x0003a550`:
   the constant upload, the scissor, and the gel shader's draw into
   target 13.
4. **Flags.** `engine::light_passes::pass_flags`: `[0x004ba014]`,
   `[0x004ba018]`, colour B zeroed in place.
5. **Layers**, as `scene_extract::layout::light_passes::steps` gives
   them: with flag bit 2 the shadow projection `0x00048ee0` and layer 10;
   layer 11; the additional casters `0x00030ed0`; layers 14, 12 and 13.
   Each drawn layer is the begin (`0x00044370`, or the inline
   `0x00014bc0`, `0x00015680` pair with the layer words `0x00467130`,
   `0x00467134`, `0x00467138`), the stencil mode `0x00015370`, the draw
   `0x00044550` and the end `0x000445d0`.
6. **Tail.** `[0x004858bc]` and `[0x004b843c]` zeroed, stencil off, the
   scissor cleared, the depth range reset (`0x00015720`), the view's
   target bound (`0x00014bc0`), both flag words zeroed.

### The first-person lights (E `0x000404b0`, `0x000402b0`, `0x0003fd70`)

`0x0003fd70` (from `0x0002bc66` in `render_view_scene`, before the
view's scene call `0x0002b990`) empties the table: every slot's datum -1
and count 0, `[0x00509440]` 0, and the first-person record list
(`0x004c8740`, count `[0x0050943c]`).
`0x000404b0(mode)` (from `0x0002c2b9`, between the two first-person
draws) then, for each slot with records:

1. poses the volume again (`0x000c17f0`);
2. `0x000402b0` (EDX the slot, one stack word the mode): zeroes both
   lists, copies the slot's records to `0x004ba138` from index
   `[0x004c0b7c]`, sets `[0x004c0b78]` past them, and gives each
   record's visible parts to `0x00041040` in the light's kind. The mode
   picks the record filter (0: every record; 2 and others: one bit
   each);
3. sets the constants with shadow 0 and weight one
   (`0x0004a1e0`, EDX the light datum);
4. sets the flags: bit 0 for a kind-0 volume, bit 1 for a colour B
   component above zero, bits 2 to 5 clear;
5. draws layers 11, 14, 12 and, with bits 0 and 1, 13 on list 1, each
   only when the list's mask has its bit; no layer 10 and no additional
   casters;
6. runs the passes' tail.

Two things the port keeps (**S** until the differential shows them):

- After eight lights, step 7 of the gather stops adding slots while
  `0x00044ac0` keeps copying into the slot of `[0x00509440] - 1`, so a
  ninth light's first-person records join the eighth light's slot.
- A slot holds the record only. The replay reads the pixel byte
  `0x004c0b84 + i` and the attachment byte `0x004c0ed6 + i` at the
  record's new index, so it reads what the last gathered light left
  there. `LightFrame` therefore keeps those two byte ranges for the
  tail as the last light left them.

### The object shadows (E `0x000335e0`, `0x00033400`)

`0x000335e0(mode)` runs only with an active lightmap (`0x000226d0`) and
the override's leave (`0x000335c0`). It sets the bytes `0x00485a46` and
`0x00509413`, sizes target 9 (`0x0001ddd0`), gathers rows into the
`0x280` bytes below its entry `ESP` (`0x00033400`, EBX the main view's
object list `[0x00547f9c]`), calls `0x00032350(row+8, row+0xc)` per row
and clears both bytes.

`0x00033400` takes up to 32 objects whose list flags fit the mode and
that `0x0003d7f0` accepts. A row is the candidate row of the lights with
the object in `+4` and `+8`, `0x0003d7f0`'s fade in `+0xc` and the
projected size in `+0x10`. Byte 0 is `0x0003e9c0`'s answer and bytes 1
and 2 are zero. It never writes byte 3 (**E**
`0x00033558`..`0x00033562`), which the ranking reads as the keep byte,
so the port reads that byte from the guest's buffer address. The rows go
through the same merge and ranking as the lights (`0x0004a7c0`,
`0x0004ac30`), on the view's history at `0x005246f8 + 0x484·view`, with
the limit and step the call site passes (**E** `0x0003359b`,
`0x000335ad`).

`0x00032350` is one caster: its own volume (`0x000c1930`,
`0x000c1a80`), frusta, visibility, records (`0x0002cbc0`), part
visibility, lists, constants, projection, layer 10, the filter
`0x000222a0` and layer 11 ([native-shadows.md](native-shadows.md),
"Object chain").

## Interfaces

| Point | Address | The body | Fallback while it exists |
|---|---|---|---|
| `RENDER_DYNAMIC_LIGHTS` | `0x00031c80` | `light_gather::frame::run` | the guest function, with the leaf seams of `native_scene_lists` |
| `RENDER_LIGHT_PASSES` | `0x00031170` | `light_gather::passes::run`, called by the frame body and installed at the point for the fallback loop | the guest function |
| `FIRST_PERSON_LIGHTS` (new) | `0x000404b0` | `light_gather::first_person::run` | the guest function |
| `OBJECT_SHADOW_LIST` | `0x000335e0` | `light_gather::object_shadows::run` | the guest function |
| `OBJECT_SHADOW` | `0x00032350` | `light_gather::object_shadows::caster` | the guest function |

New named points, all `[fn]` with the signatures above:
`FIRST_PERSON_LIGHTS` `0x000404b0`, `FIRST_PERSON_LIGHT_RECORDS`
`0x000402b0`, `FIRST_PERSON_RESET` `0x0003fd70`, `LIGHT_VOLUME`
`0x000c17f0`, `LIGHT_FRUSTA` `0x000c1d90`, `SCENE_VISIBILITY`
`0x001320f0`, `SCENE_PART_VISIBILITY` `0x00044720`, `SCENE_ITEMS`
`0x00040e90`, `RECORD_ITEMS` `0x00044ac0`, `LIGHT_REPOSE_BASIS`
`0x00031b40`, `FIRST_PERSON_MARKER` `0x0016640f`.

The scene takes its lights from the observation at the passes' entry
(`render_bridge::native_lights::begin_light`, `read_light`) until L5:
the frame body calls `0x00031170` as a guest call, so the observation
sees each drawn light as it does under the guest loop. L5 fills
`FrameScene::lights` from `LightFrame` (`guest_selected`,
`guest_weight`, `flags`, `steps`) and removes the observation. The groups
keep their type (`frame_scene::LightGroup`); `GroupSite` gains
`FirstPerson`.

Counters follow the scene lists' scheme
(`timeline::native::SceneList`, `SceneListResult`): stages
`light_frame`, `light_passes`, `first_person_lights`,
`object_shadow_list` and `object_shadow_caster`, each with `_native`,
`_fallback` and a reason. A step a running body gives to the guest
counts as its own stage's `_fallback` and reason: `light_volume` from
the frame body, `light_geometry` and `light_extra_caster` from their
seams, which run inside the guest call. A frame the body makes counts
`light_frame_native` and `visible_lights_native`, and each step it makes
in Rust its stage's `_native`. `render.scene` and `render.stats`
report them ([agent-api.md](../runtime/agent-api.md)). The producers of
`render.residual` are `dynamic_lights`, `light_passes`,
`object_shadows`, `object_shadow` and `first_person`
(`render_bridge::residual_census`).

## Slices

Each slice lands on its own under `impl.render.native`, with the guest
path whole behind `=off`. Sizes as in retire-d3d.md. Every slice's
gate starts with the hash gate: `sim.hashes` equal row by row on two
launches, `impl.render.native=off` against the default
(framescene.md 9.3). "Differential" means the lifted original on the IR
interpreter from seeded states, compared as
`crates/rewrites/engine/tests/sim_objects_differential.rs` compares:
the callout transcript (target, return site, registers, all memory
outside the dead stack) and the final state. Lanes write the tests; the
coordinator runs them.

### L1: the model, and differentials for the built leaves (S)

- **Build.** `engine::render::light_gather::{visible, history}`:
  `VisibleLight`, `HistoryRow`, `History` with `decode`/`encode`, and
  `merge`, `rank_and_fade(limit, step)` and the candidate tests, moved
  from `native_scene_lists/lights.rs` unchanged in behaviour;
  `visible::list` is `0x00031910`. A `Writes` list (address, bytes) is
  the output type; the seams in `native_scene_lists` convert it to their
  `Output`. `geometry::flags` (`0x00031660`) and `extra_caster::list`
  (`0x00031c20`) move beside them as functions of `(memory, typed
  arguments)`. `volume::pose` is the pose of `0x000c19f0` and
  `0x000c1a80` (`scene_extract::lights::volume` calls it), and
  `volume::light_volume` is `0x000c17f0`: its gates, the pose and AL.
- **Guest state.** None new: the seams' reads and writes are unchanged.
- **Tests.** Differential roots `0x00031910` (its tree `0x0004a7c0`,
  `0x0004ac30`, `0x0004b160`, `0x00031590`, `0x0003e9c0` lifted, no
  stub), `0x00031660`, `0x00031c20`, `0x000c17f0`: seeded pools of
  lights, owners and histories, more than 32 and more than 128
  candidates, the frozen override, each definition bit the code tests.
  The existing unit tests move with the code.
- **Live check.** `render.stats`: `native_scene_lists_visible_lights_native`
  and the two others unchanged in rate against master.
- **Refusal.** A seam that refuses leaves its guest function to run.

### L2: the frame body (M)

- **Build.** `GuestCall::reserve`. `light_gather::frame::plan` (pure):
  the float state, the frame's stack, step 1 whole (`visible::list`
  into `S+0xb0`, its `Writes`), and for every drawn row steps 2 to 4
  and the check that step 5 can run (an owner when a re-pose is due),
  with the record fields and globals the steps read. `frame::run(cx,
  plan)` writes the `S+0x10` dword the guest's push leaves, publishes
  step 1, then makes steps 2 to 16 per row in order, reading each row,
  record and global when the guest does, and returns a `LightFrame`
  (per row: drawn, and per drawn light the owner, root, re-pose,
  first-person slot, shadow, source, cluster, frusta count, `V`, and
  how steps 6, 12 and 13 were made). Steps 6, 12 and 13 call the Rust
  functions of L1 and give the step to the guest function when they
  refuse. Steps 7 to 9 are inline. Steps 5, 10, 11, 14, 15 and 16 are
  guest calls (their existing hooks run inside them).
  `render_bridge::native_lights::frame` installs it with `implement` at
  `RENDER_DYNAMIC_LIGHTS` (one implementation; the observers before and
  after keep running), on the render thread (`context_only`), and runs
  the original when the lights stage stands down or the plan refuses.
  The scene's lights stay the passes' observation until L5.
- **Call sites.** A hook that tests its caller's return address sees
  the body's continuation instead. While the body makes a guest call,
  `frame::call_site` names the guest's return site for it (per thread).
  `lod_distance.rs` takes its record-reset caller from it, so a light's
  records keep the world caller `0x00031ef5`
  ([occlusion-culling.md](occlusion-culling.md)).
- **Guest state.** The table above, every row.
- **Tests.** Differential root `0x00031c80`
  (`frame_transcripts_match_the_lifted_original`). Its tree is the root
  and the leaves of L1, lifted; every other callee is a seeded stub that
  writes what the frame reads after it (the re-posed frame, the frusta
  count, a later row's weight). Equal transcripts (target, return site,
  the registers each callee is handed, its stack arguments), equal EAX
  and equal memory outside the dead stack and the private spills, over
  0, 1, 8, 9 and 32 drawn lights, NaN and threshold weights, each
  branch of steps 5, 8 and 9, and frames ranked from candidates. Unit,
  over `entry::testing::run` (`crates/rewrites/engine/tests/light_gather.rs`):
  `a_light_frame_calls_the_gather_in_the_guests_order`,
  `the_ninth_light_takes_no_first_person_slot`,
  `a_refused_plan_writes_nothing`. The reserve:
  `a_reserved_call_is_made_below_the_reserved_bytes` (guest-layout) and
  `a_reserved_guest_call_leaves_the_bodys_frame_alone` (hooks).
- **Live check.** `light_frame_native` per view per frame on a lit
  interior, `light_frame_fallback` zero; `render.scene` lists the same
  lights, weights and shadow flags as master's observation;
  `render.parity` within the bar.
- **Refusal.** `plan` refuses before any write (memory, layout,
  unsupported float state: `fp_supported` and the x87 control word as
  `read_visible` checks) and the guest function runs whole. After
  publication only steps 6, 12 and 13 can refuse, each to its guest
  function.

### L3: the light's records (M)

- **Build.** `engine::render_loop::world_walk::plan` takes the light's
  kind: `vis` may be `0x0054aa68` with `[vis+0x2a78]` not 0, the cursor
  starts at `[0x004c0b78]`, and `MAIN_COUNT` is not advanced. The frame
  body calls it for clusters and placements after doing `0x0002cb30`'s
  light arm itself. `0x00133720` stays a guest call: it is the reorder
  `0x00134240` and a loop over `0x00133790`, both Rust behind their
  hooks, with one repeat of an index whose setup returns AL set in
  kind 1 (**E** `0x00133759`..`0x00133769`).
- **Guest state.** Step 14's row. Reads the camera's depth plane and
  pixel scale (`0x004b9e20`.., `0x004b9ed0`), which are the main view's
  in the light's kind too (**E** `0x00133390`).
- **Tests.** Differential roots `0x00133390` and `0x00133520` under a
  light's `V` (the record writer `0x0002cbf0` lifted). Unit:
  `a_lights_records_start_at_the_main_views_end`,
  `a_lights_walk_leaves_the_main_count`.
- **Live check.** The light's cluster and placement walks native
  outside cold sections in `render.stats`; hash gate; `render.parity`.
- **Refusal.** A walk that refuses (a cold section, a layout it does
  not read) gives that walk to its guest function, whose hooks retry
  row by row as in the main view.

### L4: the item lists and the first-person table (M)

- **Build.** `light_gather::items`: the light's kind of `0x00040e90`.
  It zeroes both lists, runs `0x00041c80` as a guest call, and walks
  the light's records as `0x00044ac0` does: the bit-10 copy into the
  table (`FirstPersonLights::push`, with the slot rule above), and for
  the rest the section append of
  `render_bridge::native_world::section` extended to lists 1 and 2 by
  the record's bits 1 and 2. Object records keep `0x00041040` as a
  guest call per part until their append is ported with the world's.
  The model keeps each light's `receivers` and `casters` ranges and
  both masks.
- **Guest state.** Reads the records, the byte arrays `0x004c0b84` and
  `0x004c0ed6`, the override table `0x004c1228`, the BSP and model
  section tables. Writes the list headers `0x004c1a60` and `0x004c1a78`
  and their items, the table `0x004c6700` with its counts at `+0x404`.
- **Tests.** Differential root `0x00040e90` in the light's kind over
  synthetic sections (`0x00044ac0`, `0x000449e0`, `0x00045ce0`,
  `0x000460d0`, `0x00041040`, `0x00040de0`, `0x00040e30` lifted; the
  shader queries stubbed). Unit:
  `a_receiver_goes_to_list_one_and_a_caster_to_list_two`,
  `a_first_person_record_is_copied_to_the_current_slot`,
  `records_past_the_eighth_light_join_the_last_slot`,
  `a_full_slot_drops_the_record`.
- **Live check.** `scene_light_entry_remainder_*` unchanged; the
  groups `render.scene` dumps are equal to master's on a paused tick.
- **Refusal.** A section the append does not take is appended by the
  guest's per-part calls for that record, in place.

### L5: the passes body (M)

- **Build.** `light_gather::passes::run(cx, light)`: steps 1 to 6 of
  "One light's passes" in Rust order control, with the constants call,
  the projection call and each layer's begin, draw and end still guest
  calls, so the stage's existing whole-group draws apply unchanged.
  `LightPasses` is computed before the first layer from
  `pass_flags` and `light_passes::steps`. The scene's `SceneLight` and
  the candidate for layers 12 and 13 come from it; `begin_light` and
  its hook go.
- **Guest state.** `[0x004ba014]`, `[0x004ba018]`, the colour B words
  `0x00485950`.., the layer words, `[0x004858b4]`.
- **Tests.** Differential root `0x00031170` with its callees stubbed:
  equal transcripts over every flag combination, each mask bit, the
  override byte and the additional casters' condition. The existing
  `engine/tests/light_passes*.rs` stay.
- **Live check.** `light_passes_native` equals the drawn lights;
  `lights_walked_native` and `scene_light_layers_alone` unchanged;
  `render.parity`.
- **Refusal.** None after entry: every step has its guest call.

### L6: the first-person lights (M)

- **Build.** `light_gather::first_person::run`: the six steps above
  from `FirstPersonLights`, the record replay through L4's append, the
  lights and groups into the scene (`GroupSite::FirstPerson`), and the
  layers through the same guest layer calls, so `Glue::alone` draws a
  whole group of layer 12 or 13.
- **Guest state.** The table, `[0x00509440]`, the record tail and both
  byte arrays, both lists, the light globals and flag words.
- **Tests.** Differential roots `0x000402b0` and `0x000404b0` (callees
  stubbed): the three modes, an empty slot, a slot of 32. Unit:
  `a_replayed_record_reads_the_bytes_at_its_new_index`.
- **Live check.** `first_person_lights_native` on a level with a
  flashlight or a muzzle light; the `first_person` producer's entries in
  `render.residual` fall by the lights' share; `render.parity` over the
  weapon's rectangle.
- **Refusal.** Before the first slot: the guest function. After: per
  step, as L4 and L5.

### L7: the object shadows (M)

- **Build.** `light_gather::object_shadows::run`: the gates, the rows
  (byte 3 read from the guest buffer's address), `History` at
  `0x005246f8`, the merge and ranking of L1 with the call site's limit
  and step, then one caster body per row. The caster body drives the
  mask branch of `0x00032350` with L3's and L4's pieces and guest calls
  for the rest; the creature branch (`0x00032ce0`) and the early exits
  are the guest function.
- **Guest state.** `0x00485a46`, `0x00509413`, the history, and per
  caster what the lights' gather and passes write.
- **Tests.** Differential roots `0x00033400` (with `0x0003d7f0` and
  `0x0003e9c0` stubbed) and `0x000335e0` (callees stubbed); a stale
  byte 3 of each value. Unit:
  `an_object_shadow_row_keeps_the_buffers_stale_keep_byte`.
- **Live check.** `object_shadow_list_native`; `shadow_walk_layers`
  unchanged; `render.parity` outdoors.
- **Refusal.** The list refuses before any write. A caster the body
  does not take is one guest call of `0x00032350`.

### L8: the passes typed (L)

- **Build.** A light whose every step's group is whole and whose
  passes the stages hold runs no guest call in its passes: the
  constants from `light_globals` with the screen bounds from a port of
  `0x001650a0`, the scissor as the frame graph's rectangle, the
  projection from `shadow_globals`, the targets' begin, clear and end
  as graph nodes, the gel from the scene
  ([native-gels.md](native-gels.md)), and the additional casters as
  groups of their site. The globals are still written
  (`LightGlobals::write`) for the handlers of other lights. The same
  for a first-person light and an object shadow.
- **Tests.** Differential root `0x001650a0`. A graph test per site
  that a typed light issues the steps' draws in the passes' order with
  no device call. The mixed GPU fixture of native-shadows.md extended
  with a typed light beside a guest one.
- **Live check.** `render.residual`: `light_passes` and `object_shadow`
  at zero entries on the gate corpus; `render.parity` at 1x and 4x on
  lit interiors.
- **Refusal.** While any light can refuse, a refused light runs L5's
  body. The slice ends when the gate corpus has none; from then a light
  the stage cannot draw is a named drop (`scene_dropped_lights`,
  retire-d3d.md, "Rules and limits").

### L9: the gate (S)

The registry rows of the five producers change to `native`. The
resolved-draw light admission, the gathered walk's handler path and the
leaf seams of `native_scene_lists` that only the guest loop reaches are
deleted with their counters. The guest calls a body still makes are
listed in its module doc: the R4 three, the first-person marker lookup
and the attachment walk `0x00041c80`.

## Rules and limits

- **Plan, then write.** Everything that can refuse a whole entry is
  checked before its first guest write.
- **A step's fallback is the guest's own function for that step.** No
  second Rust approximation, and no skipped step.
- **The guest's order.** Writes and guest calls are made in the order
  of the tables above. A light's records and lists are valid only until
  the next light's gather.
- **Float state.** The x87 code (`0x00031b40`, the cluster radius in
  `0x00133390`) is ported through `x87::abi` under the caller's control
  word, and SSE under the caller's MXCSR, as `light_constants` does.
- **No game data.** Thresholds, limits and steps are read from the
  image at their addresses or taken from the call site in the lifted
  code under test. None is written in the repository.
- **Not in R3.** The frusta, the traversal and the part visibility
  (R4); the lighting selection of [e3-lighting.md](e3-lighting.md),
  which keeps reading `scene_extract::lights::candidates`.

## Risks

- **Stale reads.** Three behaviours depend on bytes nobody wrote that
  frame: byte 3 of an object-shadow row, the two byte arrays under a
  replayed first-person record, and the slot a ninth light's records
  join. Each has a test in its slice; if a differential disagrees with
  this page, the differential is right.
- **The re-pose reads first-person state.** `0x0016640f` answers from
  the weapon's first-person pose. A frame step that runs at another
  point than the guest's render would read another pose.
- **Tick-visible writes.** The re-pose writes the light record and
  `0x00031660` writes `L+0xf8`. framescene.md 5.2 classes the light's
  render fields as unread by the tick; the hash gate of L2 is the
  check.
- **Shared record tail.** The lights, the additional casters, the
  object shadows and the first-person replay all write
  `[0x004c0b7c, ..)`. A body that leaves the cursor elsewhere than the
  guest breaks the next consumer.
- **`0x00041c80` in the light's kind.** Its work under a light is not
  read (**S**: nothing, by the kind test). It stays a guest call.
- **Object items.** L4 leaves model parts on `0x00041040`. Their
  append is shared with the opaque block (retire-d3d.md, R1).
- **Two loops.** Until L9 the guest loop and the body both exist, and a
  leaf seam fixed in one place must match in the other. L1 moves the
  leaves to one implementation for that reason.

## Evidence

- **E.** The step tables are the instruction ranges named in their
  headings, read with `h2 disasm`.
- **Code.** The built leaves and the frame: `engine::render::light_gather`
  (`visible::list`, `history::merge`, `history::rank_and_fade`,
  `geometry::flags`, `extra_caster::list`, `volume::light_volume`,
  `frame::plan`, `frame::run`),
  `engine::light_constants`, `engine::light_passes`,
  `engine::shadow_constants`, `engine::render_loop::world_walk`,
  `render_bridge::native_world::section`.
- **S.** That `0x00041040`'s list choice is only the record's bits 1
  and 2 and the kind (L4's differential). That the lighting selection
  in the record writer's BSP arm does not depend on the kind (L3's
  differential).

## Not verified

- Every step table against a running game: the differentials of L1 to
  L7 are the first check.
- Whether any guest code between two frames reads the first-person
  table or a light's records.
- The time the three entries cost with the bodies in place, per slice
  (`h2 lab`, the `dynamic_lights` and `light_passes` rows of
  `render.residual`).
- Pixel parity of a typed first-person light and of a typed object
  shadow.

## Related

[retire-d3d.md](retire-d3d.md) (R3, R4), [native-lights.md](native-lights.md),
[native-shadows.md](native-shadows.md),
[native-shadow-layers.md](native-shadow-layers.md),
[native-gels.md](native-gels.md), [native-renderer.md](native-renderer.md)
("Scene list builders"), [framescene.md](framescene.md) (5.2, slice 3),
[occlusion-culling.md](occlusion-culling.md),
[e3-lighting.md](e3-lighting.md) ("The light render path").
