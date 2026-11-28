# Hot iteration: live tag edits and a hot host renderer

Status: Partly built. Built and tested without the game: WGSL overrides of translated programs with a watched directory, override parameters, toggles, program listing and dumps (`render.*`); field reads, writes, journal and revert of loaded tags (`edit.*`). Not built: live object placement, runtime `impl.render.*` switches, a reloadable renderer dylib ([What is left](#what-is-left)).
Code: `crates/extensions/agent/src/hot.rs`, `crates/extensions/agent-proto/src/hot.rs`. Tests: `crates/extensions/agent/src/hot/tests.rs`.
Switch: none.

Nothing here was established by running the game.

| What you edit | Loop | Mechanism |
|---|---|---|
| how a translated program shades | save a WGSL file, seen next frame | `render.*` overrides, parameters, toggles |
| the values a tag feeds the renderer | `edit.set`, seen next frame | `edit.*` writes guest memory under a journal |

## Why

A rendering change otherwise needs a game image rebuild (host renderer) or a `.map` re-patch and restart (tag), minutes per look. The aim is an edit-to-see loop of about a frame for the two things iterated most: how a translated program shades, and the values tags feed it.

## Choices

**Shader overrides, not a renderer dylib.** The renderer already has the
seam: every draw that is not the built-in shader runs a translated
program registered under a shader id, and re-registering an id drops the
cached pipelines (`gfx::Renderer::register_shader`). An override is a
WGSL file that replaces one program's module. Compared with
reloading host render code as a dylib:

- it is the fastest loop there is: save the file, the next frame draws
  with it (a quarter-second poll, then one pipeline rebuild);
- it needs no stable ABI across wgpu types, statics and thread locals,
  which a Rust dylib reload does not give (reloading code that owns GPU
  objects is the riskiest change on the list);
- it can be checked before it runs. naga parses it, and its interface is
  compared with the program it replaces (`hot_render::iface::check`):
  same entry points, no resource binding the layout lacks, no vertex input
  the buffers lack, no output the targets lack, varyings that link. wgpu
  ends the run on a validation error; a failed check is a rejected
  override and the last good module keeps running.

What overrides can't reach: the native passes written in Rust (scaling,
interpolation, clears). For those the hub has parameters and toggles that
the pass reads each frame, so a knob can be turned without a rebuild. A
change to their code still needs a rebuild.

**Keyed by the pipeline cache's program id.** `d3d::pipeline_store::
program_id` (an FNV hash of the program key: vertex microcode or the
synthesized program, the combiner program with its constants cleared,
the options) already names programs across runs. Override files are
`<id, 16 hex digits>[_note].wgsl`. `render.programs` lists the ids;
`render.dump` writes a program's translated WGSL to start from.

**Parameters by text.** `{{name=default}}` in an override is replaced by
the parameter's value as a float literal before checking. Setting a
parameter recompiles only the overrides that use it. This keeps the
uniform layout the translator's (adding a binding would change the
contract with `gfx::program`) at the cost of a pipeline rebuild per
change, which is a frame.

**Tags edited where the game holds them.** The loaded map's metadata sits
in guest memory at the virtual addresses its pointers use. The tag index
is found from the engine's globals (`TAG_INDEX` `0x00547f00`, the 16-byte
entry table at `TAG_ENTRIES` `0x004e3b44`; docs/engine/tag_get_checked.md).
Fields are named by the editor crates' definitions (`tags::defs`, the
same ones the Sapien-style `.map` patching uses), so a path like
`postprocess[0].colors[1].color` resolves through blocks by their stored
count and address. Groups without a definition take raw fields
(`+0x10:f32[3]`), and raw blocks, `+<offset>/<stride>[<index>]`: element
`index` of the (count, address) pair at that offset, `stride` bytes per
element, checked against the stored count, followed by raw fields only.
`scnr` has no definition, so the editor reaches a placement this way, with
`scenario_edit::layout`'s offsets: `+0x50/0x54[3].+0x8:f32[3]` names a
block at `+0x50` with `0x54`-byte elements (numbers invented here).

**Invalidation by the write barrier.** Tag writes go through
`Guest::write_bytes`, which is the runtime's write barrier: boot's D3D
glue drains it into the device's write tracker before every D3D call
(docs/graphics/d3d.md, "Integration"). So a write to bytes the device
copied (texture or vertex data) re-uploads them at the next draw that
uses them, with no extra invalidation path. Program changes invalidate
by re-registering (pipelines); nothing else on the host caches tag data.

## What shows when

| Edit | Shows | Why |
|---|---|---|
| a shader's postprocess colours (`postprocess[0].colors[n].color`) and vectors (`vectors[n].vector`) | next frame | read at each draw into pixel and vertex constants (E `0x0001717b`, `0x00017039`; docs/formats/shader-instances.md) |
| a shader's bitmap (`postprocess[0].bitmaps[n].bitmap`, a `bitm` datum) | next frame | read at each draw (E `0x00016f60`); the texture is the other bitmap's, already resident or loaded by the game's own cache |
| a shader bitmap slot's image and LOD offset (`postprocess[0].bitmaps[n].image`, `.lod_offset`) | next frame | read at each draw with the datum (E `0x00016f60`, `0x0001b15c`); `shad.txt` marks them `applies=frame` |
| bytes the D3D device made a host copy of | next draw that samples them | the write barrier (above) |
| an override file, a parameter, the `overrides` toggle | next frame | the provider's once-a-frame look at the hub |
| the shader `parameters` block | never by itself | the draw path does not read it (shad.txt): edit the postprocess |
| a bitmap's own header or LOD pointers | when the game reloads it | its texture cache already holds the pixels |
| placements, object definitions | when the object is made again | read once at creation |

The journal is per map: an edit records the scenario datum it was made
under, and a change made on another map is listed as `stale` and never
written back. A revert leaves alone bytes the game changed since.

## Use

`h2 serve` (or any run with `H2_AGENT`) adds the methods and watches
`$H2_RENDER_OVERRIDES`, else `~/Library/Caches/h2recomp/render-overrides`.

```sh
# Programs, and one written out to start from
h2 agent render programs
h2 agent render dump <id>
# -> <override dir>/translated/<id>.wgsl
cp ~/Library/Caches/h2recomp/render-overrides/translated/<id>.wgsl \
   ~/Library/Caches/h2recomp/render-overrides/<id>_test.wgsl
# edit it; it loads on save. Then:
h2 agent render overrides            # applied, or why not
h2 agent render param tint 0.25      # a {{tint=...}} in the file
h2 agent render toggle overrides off # compare with the translation

# Tags
h2 agent edit tags shad metal
h2 agent edit get 0xe1230045 postprocess[0]
h2 agent edit set 0xe1230045 postprocess[0].colors[0].color 0xffff00ff
h2 agent edit changes
h2 agent edit revert
```

Names (`edit get "shaders\...\metal" ...`) need the loaded map's file,
which `h2 serve` finds through the VFS; datums always work. A name
several tags share (a weapon's `weap`, `hlmt`, `mode`, `coll` and `jmad`)
needs its group: `group`, `shad:shaders\...\metal` or
`shaders\...\metal.shad`.

## Methods

Host methods (`agent_proto::hot::method`, `crates/extensions/agent/src/hot.rs`):

| Method | Access | Params → result |
|---|---|---|
| `edit.tags` | read | `{group?, name?, limit?}` → `{scenario, names, total, tags: [{tag, group, name, address, size, defined}]}` |
| `edit.get` | read | `{tag, group?, path?}` → the field (`value`, `type`, `addr`, `meaning`) or the element's `fields` |
| `edit.set` | act | `{tag, group?, path, value}` → the change `{id, old, new, ...}` |
| `edit.changes` | read | → `{changes: [{id, path, old, new, now, live, stale}]}` |
| `edit.revert` | act | `{id?}` → `{reverted, dropped}` |
| `render.overrides` | read | → `{dir, watching, enabled, overrides: [{program, file, params, state, error?}], problems, programs, last_dump}` |
| `render.reload` | act | `{dir?}` → `{loaded, changed, problems, generation}` |
| `render.programs` | read | `{vertex?, limit?}` → `{total, programs: [{program, shader, vertex, textures, discards, warnings, override}]}` |
| `render.dump` | act | `{program?, to?}` → queued; written at the next frame |
| `render.param` | act | `{}` list, `{name}` read, `{name, value}` set, `{name, value: null}` reset |
| `render.toggle` | act | `{}` list, `{name}` read, `{name, on}` set, `{name, on: null}` reset |

Values: numbers or hex strings for integers; arrays for several values
(`f32[3]`); enum names; flag names as an array; datums as `"0x…"` or null;
unknown bytes as hex.

## Where the pieces are

- `crates/platform/hot-render`: the hub (overrides, parameters, toggles, dumps),
  the interface check, the directory watch. No GPU.
- `crates/platform/d3d/src/nv2a_hot.rs`: the provider's side, run from
  `Nv2aShaders::maintain` once a frame.
- `crates/extensions/hot-tags`: the tag index in guest memory, paths, values, the
  editor and journal.
- `crates/extensions/agent/src/hot.rs`, `crates/extensions/agent-proto/src/hot.rs`: the methods.
- `crates/extensions/game/src/hot_glue.rs`: installs them on the agent with the
  runtime's memory and the map file's names.
- `crates/apps/cli/src/agent_hot_cmd.rs`: `h2 agent edit …`, `h2 agent render …`.

## What is left

- **Live placement** (slice 4). A scenario placement is read when its
  object is made, so editing it shows only on respawn. Moving a live
  object means writing the object's own position and letting the engine
  re-cluster it; that needs the object update path read first
  (docs/formats/objects.md), not a blind write.
- **Definitions for lights, fog and sky.** Their groups have no
  definition yet, so they take raw fields only. Names need engine
  evidence, as the other definitions have (docs/formats/tag-definitions.md).
- **Native pass knobs.** The hub has parameters and toggles; no native
  pass reads one yet. The candidates are E3 lighting's shadow factor and
  the reprojection's limits, owned by other work at the moment.
- **`impl.render.*` at run time.** Those switches choose guest-function
  overrides when the feature plan is built; switching one mid-run means
  swapping dispatch entries while the render front-end may be inside
  them. Not attempted.
- **Finding a program on screen.** Today: dump, override with a flat
  colour, look. A `render.highlight {program}` that does that in one call
  would help.
- **A first-frame flash.** A program first seen after its override was
  loaded draws one frame with its translation before the provider's next
  look swaps it.
