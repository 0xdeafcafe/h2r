# Halo 2 retail Xbox engine map

A map of the retail engine by subsystem: where each part starts, what calls
it and what it reads, with guest addresses and a confidence label on each
claim. It is the first page to read before touching a guest function. It
follows [ARCHITECTURE.md](../ARCHITECTURE.md), [LIFTER.md](../lifter/LIFTER.md)
and [map-format.md](../formats/map-format.md). It is static analysis, **not an
observed frame trace**. Names are assigned descriptions, not recovered Bungie
symbols. No executable bytes, disassembly listings, assets or string tables
are included.

## Identity, evidence, and limitations

- XBE SHA-256: `03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d`.
  Title ID `4D530064`, retail certificate version 3, entry `0x002d0aee`.
  The linked XDK library records report build 5849, including `D3D8LTCG`.
- `h2 analyze`: 18,735 discovered functions across executable sections;
  `.text` has 16,072 starts, 3,453,203 decoded code bytes out of 3,570,764 bytes.
  Its 99.90% *known* coverage includes data and padding. This is not proof of
  function boundaries or complete dynamic reachability.
- The existing Ghidra 12.1.4 project/export has 13,654 functions. It misses
  several table-referenced initialization functions and truncates some switch
  bodies. For example, Ghidra shows only the dispatch prefix of `0x0018eaa0`;
  direct disassembly reveals its cache-load branch at `0x0018eade`.
- `h2 symbols --list --no-xcheck`: 315 matches at 311 distinct addresses.
  The signature database is pinned to
  [`20eced544726f5558c5a408458f38a086cc4e543`][xbsym]. A match is supporting
  evidence, not a recovered prototype. Shared/weak matches are not used to
  establish engine identities here.

**Confirmed** means the described operation is directly supported by inspected
instructions/data or a distinctive SDK match plus its call context. **Likely**
means strong structural/call-graph evidence but incomplete semantics.
**Guess** means a search lead or an estimated ownership boundary. Confidence
applies to the narrow description in each row, not every implication of its
assigned name. All addresses are guest virtual addresses, never host pointers.

Evidence uses direct calls, global accesses, selected string xrefs, FourCC
constants, and table entries. Ghidra's inferred C
signatures are particularly unreliable with LTCG: EAX/EBX/EDI often carry
arguments omitted from the displayed parameter list.

## Startup and the main loop

```text
0x002d0aee  mainCRTStartup
  -> 0x002d21c6 CreateThread -> kernel PsCreateSystemThreadEx
     -> 0x002d212e thread system routine
        -> 0x002d0a7a application-thread startup
           -> 0x002d1b4e XapiInitProcess / CRT startup helpers
           -> 0x00012190 game_main
              -> 0x00012420 boot display create/swap/release
              -> 0x0012b400 physical arena setup
              -> 0x00012000 platform/game bootstrap
              -> 0x0012b690 main_loop_run
                 -> 0x00137c20 initialize subsystem table
                 -> repeat 0x0012b450 main_loop_iteration
```

| Address / assigned name | Confidence | Evidence and consequence |
|---|---|---|
| `0x002d0aee` `mainCRTStartup` | Confirmed | XAPI signature; computes TLS sizing and starts the application thread. It does not directly call game main. |
| `0x002d0a7a` `application_thread_start` | Confirmed | Start address passed by entry; calls process/CRT helpers and calls `0x00012190` at `0x002d0ad6`. Ghidra has no function at this entry; inspect with `h2 disasm`. |
| `0x00012190` `game_main` | Confirmed | Calls the boot-display path, arena setup, bootstrap, and loop runner in that order; shutdown follows if the runner returns. |
| `0x00012420` `boot_display_once` | Confirmed | Calls `0x00012490`, then matched D3D Swap, then releases the temporary device. Thus the first Swap is not evidence of a menu frame. |
| `0x0012b400` `physical_arena_initialize` | Confirmed | Calls XPhysicalAlloc for `0x30e4000` bytes with preferred address `0x61000`; establishes opposing arena cursors. Preserve physical/virtual alias semantics. |
| `0x00012000` `platform_bootstrap` | Confirmed | Allocates a small arena, configures FP, then calls async I/O init, preferences, fonts, cache services, animation cache, game-state arena, rasterizer, input, and sound setup. Semantic names of some children remain likely. |
| `0x00120a90` `async_io_initialize` | Likely | Initializes a request pool, mutexes, worker state; creates a thread at `0x00120c30`. Cache reads and preferences wait on completion flags. |
| `0x00137c20` `engine_systems_initialize` | Confirmed | Allocates/clears game globals, stores the pointer at `0x004e6948`, then calls 68 initialization slots at `0x00440dd8 + n*0x24`. |
| `0x0012b690` `main_loop_run` | Confirmed | Initializes timing and systems, repeatedly calls `0x0012b450` until exit byte `0x00547f2b` changes, then walks disposal callbacks backwards. |
| `0x0012b450` `main_loop_iteration` | Confirmed | Reached from that back edge. |

The ordered bootstrap children in `0x00012000` are useful stopping points:
`0x00120a90` async I/O, `0x00121350` preferences, `0x00121fc0` fonts,
`0x002136c0` cache services, `0x00123180` animation cache,
`0x00123b30` game-state arena, `0x00012560` rasterizer,
`0x001248b0` input, `0x001253d0` sound. Fonts are identified by file-path
xrefs at `0x00453390`; the animation-cache diagnostic is at `0x00453454`.

The lifecycle table has 36-byte records, with the first initialization pointer
at `0x00440dd8`; do not treat every word as a function. Inspected entries include
time `0x00146550`, players `0x0014b4b0`, objects `0x000b67c0`, rasterizer
`0x00012560`, script nodes `0x0020a700`, AI `0x001c7790`, and physics
`0x001c25a0`. Map/BSP lifecycle columns are separate from initialization and
per-tick updates. Shared no-op callbacks are common. The table is not a list
of 68 independent rewrite modules.

### Frame phases, in observed call order

This is the normal branch of `0x0012b450`; gates also select loading/inactive
paths. Labels are semantic identifications, not claims of a strict single-thread
pipeline. Async I/O and SDK workers can run concurrently.

| Phase | Anchors | Confidence / evidence |
|---|---|---|
| Pending map/state changes | `0x0018f1c0`, `0x0018e810` | Confirmed: requests mainmenu when needed, processes pending transitions; also handles save/revert/reset flags before ordinary frame work. |
| Input | `0x00124950` → `0x00124c00` → `0x00409b6c`; then `0x00217c40` | Confirmed: XInputGetState match, four controller handles, dead-zone/remapping arithmetic. |
| Cache/background progress and UI time | `0x002141f0`, `0x00163820`, `0x0012b0e0`, `0x001479c3` | Likely phase labels: cache state and UI dispatch. `0x0012b0e0` produces frame timing; do not trust its missing decompiler return type. |
| Network pre-update | `0x0008df50` | Likely receive/pump phase: calls link-status refresh `0x0008d7e0`, transport/session helpers; matched XNetGetEthernetLinkStatus below it. |
| Simulation timing | `0x00146660` → `0x001870a0` → `0x00137f70` | Confirmed fixed-step budgeting and tick loop; likely player-control preparation at `0x001870a0`. |
| Simulation tick(s) | `0x00137fe0` | Confirmed: invokes players before/after, AI, scripting, object phases and Havok; increments game-time tick count. Can execute zero or multiple times per frame. |
| Network post-update | `0x0008dfc0` | Likely send/session flush phase; called after ticks, and also on the inactive branch. Exact receive/send ownership below these two pumps needs packet traces. |
| Frame-time updates / camera | `0x00137ed0`, `0x001554b0`, `0x0016f280` | Confirmed variable-delta work and camera callback dispatch; likely observer smoothing at the last address. |
| Controller feedback | `0x00222a70` | Likely: walks local players/controller state, accumulates timers, updates feedback values. |
| Render / present | `0x00223240` → world or fallback render → `0x00013f10` | Confirmed: normal renderer calls Swap through the presentation wrapper. Swap is gated while resource-cache countdowns are active. |
| Sound service | `0x00125b40` | Confirmed sound-related frame service after rendering: voice/listener/cache helpers, sound state and DirectSound-dependent children. |

Inside a simulated tick, inspected call order includes players-before
`0x0014d3a0`, AI `0x001c83c0`, script scheduler `0x00209310`, object pre-physics
`0x000b6f10`, Havok `0x001c3d30`, object post-physics `0x000b7060`, object
finalization `0x000b7150`, then players-after `0x0014d8b0`. Other game systems
interleave these calls.

## Cache files, tags, strings, and allocation

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x0018f1c0` `request_mainmenu` | Confirmed | Uses the mainmenu scenario-path literal at `0x004533e4`, builds game options, queues them through `0x0018e790`. |
| `0x0018e810` / `0x0018e8b0` `process_transition` / `start_game` | Confirmed | Pending-options flags lead to teardown, cache acquisition `0x0018eaa0`, then game activation `0x0018f000`. |
| `0x0018eaa0` `load_requested_scenario_cache` | Confirmed | Switch branch calls `0x001228f0` at `0x0018eae0`, then resource preparation and `0x0018ec60`. This branch is missing from Ghidra's abbreviated decompilation. |
| `0x00214db0` `cache_path_build` | Confirmed | Xrefs `0x0045693c`/`0x0045694c`: map-file path formats, disc path and alternate storage prefixes. |
| `0x002136c0` `cache_services_initialize` | Likely | Initializes cache records, language/build invalidation and backing-file setup. `0x00214a60` selects an opened cache record. |
| `0x00122870` `cache_header_validate` | Confirmed | Tests `head`, `foot`, version 8, file-size/type/name constraints. Build compatibility is additionally checked through `0x00012130`. |
| `0x001228f0` `cache_tags_load` | Confirmed | Copies a `0x800` header, allocates tag space, reads index/metadata, checks `tags`, publishes index and entry pointers, resolves scenario/globals roots. |
| `0x00122d60` `cache_read_wait` | Confirmed | Submits via `0x00213760`, yields with SwitchToThread until completion, checks completed byte count. A fake successful file read without completion cannot satisfy it. |
| `0x00122c10` `tag_get_checked` | Confirmed | Validates slot bounds and full salted datum; accepts group/parent/grandparent using `0x00122d00`; returns metadata pointer or zero. ID is in ECX and expected group is in EBX on this body. |
| `0x00122d00` `tag_group_find` | Confirmed | Searches the index's 12-byte group records using `0x0013ddd0` and comparator `0x00122cf0`. |
| `0x00122c70` `tag_iterator_next` | Confirmed | Walks 16-byte tag entries, skips absent group/datum slots, optionally filters through the same ancestry records. |
| `0x0011c1b0` `scenario_switch_structure_bsp` | Confirmed | Reads scenario BSP block at `+0x210/+0x214`, stride `0x44`; invokes disposal/init callbacks and loads selected BSP/lightmap. |
| `0x00122b40` `cache_bsp_load_and_fixup` | Confirmed | Checks `sbsp`; replaces BSP/lightmap tag-entry metadata pointers from region header. Critical because ordinary index entries need not contain resident BSP pointers. |
| `0x0012c0d0`, `0x0012d8b0`, `0x002185d0` `texture/geometry/sound_cache_initialize` | Confirmed | Allocation-name xrefs at `0x004534d8`, `0x00453514`, `0x004570b0`; common cache constructor `0x0013d170`. Cache-content decoding remains outside this survey. |

Important globals, confirmed by loader stores and subsequent reads:
`0x00547700` is the **inline header buffer**, `0x00547f00` holds the tag-index
pointer, `0x004e3b44` the tag-entry pointer, `0x004e0350` the scenario pointer,
`0x004e034c` the globals pointer, `0x004e0348` the active BSP pointer, and
`0x004e0344` the lightmap pointer. The header at `0x00547700` is copied
inline, not fetched through another pointer.

The root identities `scnr`/`matg` follow their index roles and layouts, rather
than literal FourCC comparisons in every caller. Explicit local constants
include `sbsp` at `0x00122b70`, `ltmp` at `0x0012def0`, `bitm` at
`0x0012c654`, `obje` at `0x000a5bdb`, and `bipd`/`coll`/`phmo` in resource
preparation `0x0011c840`. Constants alone identify an expected tag class, not a
whole function. Read FourCCs as numeric little-endian storage, preserving
punctuation in `snd!` and `ugh!`. [Format rules](../formats/map-format.md)

**Rewrite constraint:** the checked accessor is only one path. The common
release path inlines `(datum & 0xffff) * 0x10`, loads `0x004e3b44`, then takes
entry `+8`. This survey found 995 distinct Ghidra function owners directly
referencing that global. A Rust override must keep the published guest index
and guest metadata addresses valid until those consumers are replaced. Do not
silently replace engine behavior with the stricter host parser: the checked
accessor here sign-extends the low 16-bit slot, whereas the fast inline path
masks it. Capture the actual contract for each entry.

### String IDs and localized menu text

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x001a01e0` `locale_tables_load` | Confirmed | Allocates an eight-byte-per-entry index plus string blob and reads both through `0x00122d60`; marks locale resident. Called during `0x0018ec60`. |
| `0x001a0180` `localized_tag_string_get` | Confirmed | Resolves a tag datum, selects language, reads the tag's range at `+0x10 + language*4`, calls `0x001a0350`. This matches `unic` language ranges. |
| `0x001a02d0` `locale_string_id_find` | Confirmed | Scans a bounded locale-index range, compares the **full** string-ID key, returns blob base plus matched offset; miss returns an empty-string address. |
| `0x001a0350` `localized_string_copy` | Confirmed | Requires loaded locale state, invokes lookup, conversion helper `0x00140440`, and bounded wide-string copy. |

A StringID's high byte encodes length and low 24 bits its identifier
(unverified here; see [map-format.md](../formats/map-format.md) for the
on-disc checks). The local **localized** resolver compares the packed
value; do not strip its high byte before that comparison. The general
StringID-to-debug/name-string resolver for the map header's string table was
**not identified**. It is distinct from resolving menu text through `unic` and
`matg`. Implement host-side indexed string names using the map reader, while
preserving this runtime localization path.

The runtime locale record: local code uses `matg + 0x188 + language*0x1c`,
with two resident pointers followed by count/size/file offsets, so the count
is at `matg + 0x190`.

### Arenas, pools, and datums

| Address / assigned name | Confidence | Evidence / rewrite boundary |
|---|---|---|
| `0x00123b30` `game_state_arena_initialize` | Confirmed | Gets backing from `0x00214f10`, clears `0x3fe000` bytes, sets header/global allocations using base `0x004e6080` and cursor `0x004e6084`. |
| `0x00123dd0` `game_state_pool_new` | Confirmed | Bump-allocates a pool plus header from that arena and initializes it with `0x0018e250`. |
| `0x0018e1f0` / `0x0018e2b0` `memory_pool_new` / `memory_pool_allocate` | Confirmed | Allocator-backed pool construction; aligned block allocation, owner pointer/backlinks and free-byte accounting. Shared by objects/UI. |
| `0x0016b570` / `0x0016b5f0` / `0x0016b650` `data_new` / `data_initialize_contiguous` / `data_initialize` | Confirmed | Allocation and construction of named arrays with capacity, element stride, salt and occupancy bitmap. `0x0011cc20` is another arena-backed constructor. |
| `0x0016b840` `datum_new` | Confirmed | Finds free bitmap slot, clears element, assigns/increments salt, returns `(salt << 16) | slot`; failure is all ones. |
| `0x0016ba40` `datum_delete` | Confirmed | Clears occupancy bit and salt, updates free-slot/high-water/live-count fields; used by object rollback and script-thread retirement. |
| `0x0016bc00` `data_next_index` | Confirmed | Searches occupancy bitmap up to array high-water mark. |
| `0x0013d170` `lruv_cache_new` | Likely | Shared named animation/texture/geometry/sound-cache constructor over a data array. |

These are good small Rust candidates once guest allocation identity and failure
behavior are reproduced. Array data and occupancy pointers are at `+0x44` and
`+0x48`; the many inlined consumers make immediate conversion to Rust-owned
collections unsafe for compatibility. Maintain generation/salt behavior and
allocation order before changing ownership.

## Game state, objects, players, AI, and HaloScript

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x001df3e0` skull stock / `0x004f55dc` skull array | Confirmed | `ice_cream_flavor_stock` (`0x2ac230`) passes the index in ESI; sets byte `n` of the 15-byte array once. The array is never cleared. See [skulls.md](../game/skulls.md). |
| `0x00146550` / `0x001465b0` `game_time_initialize` / `game_time_new_map` | Confirmed | First allocates `0x24` bytes and stores pointer at `0x00510c54`; second copies tick rate from game options and computes its reciprocal. Both need direct disassembly in this Ghidra project. |
| `0x00146660` `game_time_budget_ticks` | Confirmed | Pause/state/speed checks, fractional accumulator, bounded whole-tick count and elapsed simulation time. |
| `0x00137f70` / `0x00137fe0` `game_update_ticks` / `game_tick` | Confirmed | Loop over budgeted ticks; latter updates subsystems and increments `game_time +8`. Time step is `+4`, tick rate is `+2`, speed is `+0xc`. |
| `0x000b67c0` `objects_initialize` | Confirmed | Allocates 2,048 twelve-byte object headers and a one-MiB object pool; name references at `0x0045322c`/`0x00453224`; stores `0x004e0300` and `0x004de2ec`. |
| `0x000b7b40` `object_new` | Confirmed | Resolves definition tag, chooses type descriptor at `0x00468630`, allocates datum/body, initializes transform/attachments/vitality/physics, invokes type initialization, rolls back on failure. Returns salted object ID. |
| `0x000bc280` `object_header_and_body_allocate` | Confirmed | Calls datum allocator and memory-pool allocator, stores body pointer at object-header `+8`, zeros body; rollback deletes header. |
| `0x000badc0` `object_get_with_type_mask` | Likely | Used by player-from-unit and camera/object helpers with allowed-type masks. Verify failure/salt checks before overriding. |
| `0x00108a90` `object_type_new_dispatch` | Confirmed | Called from object_new after common allocation; walks the descriptor ancestry list at `+0x84` and invokes callback `+0x2c`. |
| `0x00108c60` / `0x00108cd0` `object_type_update` / `object_type_post_physics` | Confirmed | Walk the same descriptor ancestry, invoking `+0x40` / `+0x44`; reached through `0x000bc470` / `0x000bc820`. |
| `0x000c4410` / `0x000c6b30` `unit_new` / `unit_update` | Confirmed callback roles | The `unit` descriptor at `0x004679b0` supplies these at `+0x2c` / `+0x40`; biped ancestry includes it. Full behaviors are not audited. |
| `0x000dc540` / `0x000dd360` / `0x000dd990` `biped_new` / `biped_update` / `biped_post_physics` | Confirmed callback roles | The `bipd` descriptor at `0x00467a78` supplies `+0x2c` / `+0x40` / `+0x44`; biped_new calls `0x000de4c0`. |
| `0x0014b4b0` `players_initialize` | Confirmed | Allocates 16 entries of stride `0x21c`; players-name xref `0x00453984`, published pointer `0x004e8c24`. |
| `0x0014de70` / `0x0014de90` `local_player_get_player_index` / `player_index_from_unit_index` | Confirmed | Local-player mapping and unit-to-player access. The latter reads unit `+0x13c` here. |
| `0x001c7790` / `0x001c79e0` `ai_initialize` / `ai_initialize_for_new_map` | Likely | Lifecycle-table slots. |
| `0x001c83c0` `ai_update` | Confirmed | Called per tick, gates on AI globals `0x004f55d0`, invokes actor/squad/behavior work. Actor array at `0x004f55f0` has extensive local consumers. |
| `0x00257d00` `ai_command_scripts_initialize` | Confirmed | Allocates command-script and joint-command-script arrays; diagnostic-name xrefs `0x0045a690`/`0x0045a678`. |
| `0x0020a700` / `0x00208ff0` `hs_initialize` / `hs_runtime_initialize` | Confirmed | Script-node array has stride `0x14`; runtime creates named thread/global arrays and publishes pointers. |
| `0x00209310` `hs_update` | Confirmed | Walks script-thread datums, compares wake tick with `game_time +8`, executes ready threads via `0x00209850`. |
| `0x00209850` `hs_thread_execute` | Confirmed | Uses scenario scripts, node array, thread stack frames; dispatches builtins through table `0x004744e0` or expression handling `0x00209bf0`. |

The game-time structure is `0x24` bytes; the local tick rate is read from
options, and this survey does not hard-code
30 Hz as a universally valid runtime contract.

Object headers and variable-size object bodies are separate. The local common
body initializes type at `+0xaa`; type descriptors fan out to biped, vehicle,
weapon and other behaviors. Type slot zero at `0x00468630` points to the biped
descriptor. Its ancestry list is object (`0x004678e8`, `obje`), unit
(`0x004679b0`, `unit`), biped (`0x00467a78`, `bipd`), then null. The FourCC
at descriptor `+4`, short diagnostic name, and actual dispatcher offsets
establish the callback roles above. Their bodies are discoverable with
`h2 disasm` even where Ghidra has no function. The unit's player datum is
read at `+0x13c` by the Xbox helper.

HaloScript globals are `0x004f9394` nodes, `0x004f9384` threads,
`0x004f9380` globals. The scheduler uses
`0x418`-byte thread records and script nodes with `0x14` stride. Runtime execution
is not a console text parser.

**Rewrite order:** datums first, common object identity next, type-specific
behavior later. Keep the scheduler and reached script opcodes working for menu
camera/UI behavior. A blank AI state is not proof that all AI lifecycle hooks
can be removed; retain generated initialization until a menu trace proves which
work is unnecessary.

## Rendering, camera, and UI

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x00012560` `rasterizer_initialize` | Confirmed | Creates normal device through `0x000128c0`, sets render states/palettes, creates resources and geometry/texture caches. |
| `0x00012490` / `0x000128c0` `boot_device_create` / `game_device_create` | Confirmed | Two direct calls into SDK creation, at `0x00012526` and `0x000129cd` respectively. |
| `0x003f5240` `Direct3D_CreateDevice` | Confirmed | Unique D3D8LTCG match and device construction/body inspection. Custom register ABI; not desktop D3D8's COM signature. |
| `0x00223240` `render_frame_and_present` | Confirmed | Chooses `0x002234e0` when game/BSP state is ready, otherwise `0x002232a0`; then presents when resource-cache counters allow. |
| `0x002234e0` `render_game_views` | Confirmed | Builds per-player/cinematic viewport and camera records; invokes `0x0002b5d0`. |
| `0x0002b5d0` `render_views` | Confirmed | Begin-frame setup, loops view records, selects game-view `0x0002b790` or alternate view, ends drawing. |
| `0x0002b790` / `0x0002ba10` `render_game_view` / `render_view_scene` | Likely | Camera/player preparation, render-state setup, scene passes, then UI composition `0x00147af1`. |
| `0x00013680` / `0x0002bcd0` `select_scene_passes` / `render_scene_passes` | Likely | Repeated scene-pass dispatch with render-target/state changes; exact pass taxonomy is not recovered. |
| `0x00041040` `submit_structure_or_model_geometry` | Likely | Branches between active BSP cluster/instance records and render-model tag geometry, then shader/material and vertex/index submission callbacks. Strong choke point for separating asset interpretation from D3D. |
| `0x0003d270` `object_render_cache_initialize` | Confirmed | Cached-object-render-state diagnostic xref at `0x004508c0`, data-array initialization. |
| `0x001554b0` `director_update` | Confirmed | Dispatches camera callbacks per local player; explicitly recognizes scripted and first-person callback addresses. |
| `0x0016c840` / `0x0023c110` `scripted_camera_update` / `first_person_camera_update` | Confirmed | Local director references. Following/dead cameras at `0x0023cbb0`/`0x0023de50` are unverified (not inspected). |
| `0x00187130` `player_control_get_field_of_view` | Confirmed | Observer default `[0x54e854]`, else the unit definition's camera FOV (`+0xcc`) divided by the zoom magnification (`0xc8880`). Only caller `0x23c202`; hooked for the FOV setting ([settings-menu.md](../game/settings-menu.md) "Field of view"). |
| `0x0023bd70` | Likely | Following (vehicle) camera: sets its FOV from `[0x54e854]`. |
| `0x00171d90` | Likely | Observer camera result: vertical FOV `+0x4c` from horizontal `+0x38` and aspect, scale `+0x50`. |
| `0x0002f800` | Likely | Render camera from observer camera (FOV `+0x4c` to `+0x28`, split-screen factors). |
| `0x0016f280` `observer_update` | Likely | Called after director with frame delta; observer/camera state feeds view records. |
| `0x00147af1` `ui_render_layers` | Confirmed | Draws several UI object layers via virtual callbacks, screen overlays/fades; called by both world-view and fallback rendering. |
| `0x001a474c` `ui_memory_initialize` | Confirmed | UI-pool diagnostic at `0x00454a94`; configures shared memory-pool allocator. `0x001a480d` creates UI data arrays. |
| `0x00013f10` `rasterizer_present` | Confirmed | Optional readback path, backbuffer scaling, SDK Swap at callsite `0x000140fc`, TV settings, frame counter. |
| `0x003f9d00` `D3DDevice_Swap` | Confirmed | D3D8LTCG signature plus callers. This is the present boundary found here; no separately established desktop-style `Present` entry. |

Structure rendering is distributed: `0x00041040` consumes active BSP fields
including cluster/instance metadata, while `0x0003c3c0` handles another
structure-specific block. Object rendering shares material/geometry submission;
there is no confirmed single `render_objects` wrapper yet. Follow object-render
cache pointer `0x00509434` and scene submissions instead of calling every
low-address renderer routine a BSP renderer. `mode`, `bitm`, shader families,
`sbsp` and `ltmp` are relevant asset classes; their role does not prove a literal
FourCC test exists in each path.

For the menu, `wgtz`/`wigl`/
`wgit`, `unic`, bitmaps, fonts and scenario background assets are the relevant
public tag families, but this survey has not assigned all their constructors.
The font initialization at `0x00121fc0`, localized resolver above, and per-layer
virtual calls are concrete entry points. Profile-list constructor `0x00249f0d`
has a diagnostic xref at `0x00459efc`; it is not the entire main-menu constructor.

**The D3D boundary is more than CreateDevice and Swap.** State setters,
resource methods and GPU push-buffer work are linked/inlined into engine code.
The D3D section
has 237 analyzer starts and 57,105 decoded code bytes; the section's virtual
size is 78,948 bytes, including data/tails. [LTCG and boundary design](../lifter/LIFTER.md)

## Sound, physics, networking, and persistence

| Subsystem / anchors | Confidence | Evidence |
|---|---|---|
| Sound initialization `0x001253d0` → `0x0021e4b0` → `0x0037d797` | Confirmed | Named source/cache/controller arrays; DirectSoundCreate match, capabilities, distance/rolloff/doppler settings, buffer creation/play. |
| Sound frame service `0x00125b40`; looping `0x0021a0c0`; effects `0x0021d490` | Confirmed | Allocation names, shared sound globals, cache/listener helpers. `snd!` expected-group constant at `0x00188c03` is in an object-related sound path. |
| Havok world setup `0x001c2d40` → `0x003101c0` | Confirmed | Publishes world pointer `0x0051e9a4`; constructor has Havok version-mismatch diagnostic and hkWorld source-file xref `0x00413978`. Presence of Havok is established; exact product/version is not inferred from the numeric compatibility token. |
| Havok BSP integration `0x001c2910` | Confirmed | Diagnostic at `0x004558f0` describes building BSP physics; reads active BSP and creates collision representation. |
| Physics step `0x001c3d30` → `0x00311100` | Confirmed wrapper; likely inner step identity | Called between object phases; reads tick delta and world pointer, manages x87/MXCSR, invokes Havok and collision callbacks. |
| Collision query `0x0016a8e0` | Likely sphere/world contact query | Geometry/planes, radius and contact adjustment math, used by effects; exact API name and argument ABI are unresolved. Collision includes Halo geometry queries as well as Havok. |
| Transport `0x000b4d50`, `0x000b4fa0`, `0x000b5000`, `0x000b5060`, `0x000b5110` | Guess: create/read/write/read-from/write-to | SDK/XNet-linked neighborhood; roles unverified, full socket ABI and error paths not audited. |
| Network pumps `0x0008df50` / `0x0008dfc0` | Likely semantic phases | Link polling via `0x0008d7e0` reaches matched XNetGetEthernetLinkStatus `0x0036c47d`; state/session operations surround ticks. Do not remove local simulation merely because Xbox Live is offline. |
| Simulation lifecycle `0x00067e30`, tick envelope `0x000683a0` / `0x000684e0` | Likely | Lifecycle slot plus calls enclosing game_tick. Multiplayer/session and local simulation boundaries still need a runtime trace. |
| Preferences `0x00121350` | Confirmed | File-path xref `0x0045333c`, async read, XCalculateSignature calls, version/size checks and defaults on invalid/missing data. |
| Save backing `0x00214f80`; snapshot/export `0x00124490` | Confirmed backing-file roles; likely snapshot semantics | Save-file path xref `0x00456968`; binary-output path references `0x004534a0`/`0x004534b0`. These identify I/O roles, not a complete save-game contract. |
| Profile/config cache `0x0007fa40` / `0x0007fb50`; profile type naming `0x00216b60` | Likely cache read/write; confirmed type naming | Shared configuration-cache pathname at `0x00450c68`; profile/variant naming branches. UI and persistence are separate parts of the profile flow. |

The linked middleware sizes are measurable even when engine ownership is not:
DSOUND has 476 analyzer starts / 47,892 decoded bytes; WMADEC 144 / 45,803;
XONLINE 913 / 182,012; XNET 381 / 77,836. These are discovery/code-byte counts,
not API counts. WMADEC contains substantial undecoded content. Havok is mixed
into `.text`, not a convenient separately named section. Bink has its own
sections; a movie-only display must not be mistaken for the menu-map renderer.

## Size, coupling, and rewrite priority

**Size estimates below are survey windows, not recovered module boundaries.**
`functions` counts analyzer starts in the stated half-open address windows;
`bytes` is their summed address span, including alignment, embedded tables and
possibly neighboring systems. They exclude out-of-line code elsewhere. Window
ownership is **guess**; counts/spans are measured. This avoids pretending a
partial symbol map establishes complete subsystem totals. Startup/frame is
scattered; its row measures selected known bodies instead.

The order in which subsystems are rewritten is in
[sim-rewrite.md](sim-rewrite.md) for the simulation; the feature pages of
each area say the rest.

| Subsystem | Survey size: functions / bytes | Window(s), hex | Coupling |
|---|---:|---|---|
| Startup/frame orchestration | ≥10 / 1,679 body bytes | Explicit anchors below | Very high; lifecycle callbacks, threads, all services |
| Cache/tag core | 46 / 8,048 | `122870–122f90`, `2136c0–214f10` | Very high fan-out; relatively small loader core |
| Localized strings | 6 / 560 | `1a0180–1a03b0` | Moderate; tags, locale blobs, conversion/fonts |
| Datum/pool primitives | 30 / 3,120 | `16b570–16bc90`, `18e1f0–18e700` | Very high; inlined layout users |
| Game time core | 9 / 1,088 | `146550–146990` | High; simulation, scripts, camera, audio |
| Objects/units/weapons | 1,194 / 415,328 | `0b67c0–11be20` | Very high; physics, tags, network, animation |
| Player core | 98 / 33,728 | `14b4b0–153870` | High; input, objects, simulation |
| AI actor neighborhood | 426 / 112,176 | `1a6f70–1c25a0` | Very high; objects, world queries, scripts |
| HaloScript core | 65 / 12,784 | `207cf0–20aee0` | High; builtin dispatch reaches most engine systems |
| Rasterizer/renderer core | 641 / 265,120 | `012560–053100` | Very high; resource caches, materials, GPU state |
| Camera neighborhoods | 76 / 20,016 | `1552e0–155f70`, `16c840–1709e0` | High; script/player/observer/view state |
| Sound core | 164 / 41,984 | `2185d0–2229d0` | High; cache, objects, workers, SDK |
| Physics integration | 97 / 20,976 | `1c25a0–1c7790` | Very high; excludes Havok library and other collision code |
| UI neighborhood | 582 / 57,616 | `22e000–23c110` | High; vtables, strings, input, profiles, renderer |
| Network core | 2,032 / 400,976 | `053420–0b5270` | Very high; session and simulation cross gameplay |
| Save/profile core | 93 / 11,328 | `214f80–217bc0` | Moderate/high; VFS, signatures, UI, async work |

The 10 startup/body anchors are `2d0aee`, `12190`, `12000`, `12420`, `12b400`,
`137c20`, `12b690`, `12b450`, `137f70`, `137fe0`. Their sizes are the existing
Ghidra body's address counts; they are a lower bound, not a startup-module total.
Other rows deliberately use a different, explicitly labeled window metric.
Do not sum this table into a total engine size.

Coupling also has direct evidence: unique Ghidra function owners referencing
object-array pointer `0x004e0300`: **1,387**; tag entries `0x004e3b44`: **995**;
game time `0x00510c54`: **486**; actor array `0x004f55f0`: **502**; active BSP
`0x004e0348`: **184**. These are consumers, not subsystem-owned function counts,
and omit indirect aliases and Ghidra's undiscovered functions. They show how much outside code reads each structure directly.

## Top 20 functions to understand first

Ordered for the first menu frame. These are investigation priorities; the
smallest safe rewrite is not always the earliest function executed.

| # | Address / assigned name | Confidence | Why it comes first |
|---:|---|---|---|
| 1 | `0x002d0aee` `mainCRTStartup` | Confirmed | Establish TLS/thread handoff and the real entry contract. |
| 2 | `0x00012190` `game_main` | Confirmed | Separates boot display, memory/bootstrap, and persistent loop. |
| 3 | `0x00012000` `platform_bootstrap` | Confirmed | Exposes the earliest required file, font, renderer, input and audio services. |
| 4 | `0x00137c20` `engine_systems_initialize` | Confirmed | Recover the lifecycle registration model before bypassing any systems. |
| 5 | `0x0012b450` `main_loop_iteration` | Confirmed | Coordinates transitions, ticks, network, camera, render and sound. |
| 6 | `0x0018eaa0` `load_requested_scenario_cache` | Confirmed | Connects game options to cache loading; inspect the switch's full body. |
| 7 | `0x001228f0` `cache_tags_load` | Confirmed | Publishes the structures almost every engine system reads. |
| 8 | `0x00122c10` `tag_get_checked` | Confirmed | Small salt/group-aware Rust target and reference for inline accesses. |
| 9 | `0x0011c1b0` `scenario_switch_structure_bsp` | Confirmed | Owns BSP/lightmap residency and lifecycle transitions. |
| 10 | `0x0016b840` `datum_new` | Confirmed | Small shared foundation for objects, scripts and UI lists. |
| 11 | `0x001a02d0` `locale_string_id_find` | Confirmed | Small deterministic path from packed StringID to localized bytes. |
| 12 | `0x000128c0` `game_device_create` | Confirmed | Recover Halo's actual device parameters and output handle. |
| 13 | `0x003f5240` `Direct3D_CreateDevice` | Confirmed | First normal-renderer SDK replacement and custom ABI boundary. |
| 14 | `0x00223240` `render_frame_and_present` | Confirmed | Distinguishes active world rendering, fallback rendering, and skipped swaps. |
| 15 | `0x0002b5d0` `render_views` | Confirmed | Connects camera/view records to rendering. |
| 16 | `0x00041040` `submit_structure_or_model_geometry` | Likely | Promising native Rust geometry/material boundary after D3D works. |
| 17 | `0x00147af1` `ui_render_layers` | Confirmed | Menu composition and virtual widget draw calls. |
| 18 | `0x00013f10` `rasterizer_present` | Confirmed | Engine-level end-of-frame observables and readback. |
| 19 | `0x003f9d00` `D3DDevice_Swap` | Confirmed | Actual display boundary, needed by boot and normal renderer. |
| 20 | `0x00209310` `hs_update` | Confirmed | Follow menu script scheduling and identify the reached builtin subset. |

Next: `object_new` at `0x000b7b40`, tick budgeting `0x00146660`, async reader
`0x00213760`, and sound initializer `0x0021e4b0`. They are central but larger or
more stateful than the first leaf overrides.

## Reproduce and extend

These commands are read-only. The CLI's explicit disc argument, when used,
comes **before** disassembly address/count, despite its abbreviated usage text.

```sh
target/release/h2 info disc/default.xbe
target/release/h2 analyze disc/default.xbe
target/release/h2 disasm disc/default.xbe 0x2d0a7a 35
target/release/h2 disasm disc/default.xbe 0x18eaa0 60
scripts/ghidra decompile 0x12b450
scripts/ghidra x-ref to 0x4e3b44
scripts/ghidra x-ref to 0x3f9d00
target/release/h2 symbols disc/default.xbe --list --no-xcheck
```

The survey joined Ghidra instruction references to function owners and inspected
selected decompilations, then used `h2 disasm` for absent entries/switch arms.
Raw pointer scans were only candidate generators; relevant candidates were
checked against instructions. Size-window counts use analyzer starts, while
coupling counts use unique Ghidra owners with direct references to the named
global. Neither uses transitive call-graph reachability as subsystem ownership.

## Not verified

- the unit/biped callback behaviors;
- a general StringID-to-name resolver, if one exists;
- the names of individual render passes and UI constructors;
- every register/stack ABI;
- menu scripts, asynchronous resource completion and offline simulation conditions;
- actual menu-frame reachability.

No confidence label here substitutes for those runtime checks.

## Pinned public references

- XbSymbolDatabase commit `20eced544726f5558c5a408458f38a086cc4e543`:
  [signature source][xbsym]. Signatures and their database are not reproduced
  here; the reported SDK identities were checked at the indicated call sites.

[xbsym]: https://github.com/Cxbx-Reloaded/XbSymbolDatabase/tree/20eced544726f5558c5a408458f38a086cc4e543

## Collision and physics model layouts

Static analysis of the same XBE (Ghidra decompilation plus `h2 disasm`), done to
establish the `coll` and `phmo` layouts in [collision.md](../formats/collision.md). Addresses
are guest virtual addresses; "bsp pointer" below means the pointer to element
`+4` of a 0x44-byte collision BSP element, because that is what the engine
passes around. Names are assigned descriptions. No executable bytes are quoted.

| Address / assigned name | Confidence | Evidence and consequence |
|---|---|---|
| `0x0011c840` tag-group post-process dispatch | Confirmed | Compares the group FourCC (`coll` at `0x0011c891`, `phmo` at `0x0011c8d2`, also `bipd`, `vehi`, `crea`) and calls the per-group routine. `coll` -> `0x002122b0`; `phmo` -> `0x001ea9c0` with argument 0. |
| `0x002122b0` `collision_model_postprocess` | Confirmed | Reads root `+0x1c` (region count) / `+0x20` (address), region stride `0xc` (`+4` permutation count, `+8` address), permutation stride `0x14`. Loops `permutation+0xc` times pairing the array at `permutation+8` (stride `0x44`, passed as element `+4`) with the array at `permutation+0x10` (stride `0x70`). Fixes root = 0x34 bytes, and that the BSP element is 0x44 with its blocks starting at `+4`. |
| `0x00246c60` collision BSP Havok slot init | Likely | Called per BSP/slot pair; tests the surface count at bsp-pointer `+0x28`, then writes vtable pointers into the 0x70-byte slot. Content of the slot is not decoded. |
| `0x001de6d0` BSP vector test | Confirmed | Node = 8 bytes: plane `i16` at `+0`, child at `*(i32*)(node+1+3*side) >> 8` (signed 24 bit, bit 23 leaf, all ones none), side 1 when `n.p - d >= 0`. Plane = 16 bytes `(nx,ny,nz,d)` at bsp-pointer `+0xc`; nodes `+4`; leaves `+0x14` (4 bytes, byte 0 bit 0 tested); surfaces `+0x2c` (8 bytes: plane `i16`, byte `+4` flags, byte `+5`, `i16` `+6`). |
| `0x0014a280` BSP point descent | Confirmed | Same node walk; returns `leaf & 0x7fffff` or `0xffffffff`. |
| `0x001ddea0` BSP volume query | Confirmed | Leaf: byte `+1` reference count, `u16` `+2` first reference; references at bsp-pointer `+0x1c` (4 bytes: `i16` plane with bit 15 flip, `i16` 2D node/surface); plane index masked `& 0x7fff`. |
| `0x001de220` 2D BSP walk | Confirmed | 2D node = 16 bytes at bsp-pointer `+0x24`: floats `+0,+4,+8` with test `a*x + b*y - d`, children `i16` at `+0xc` (taken when `dist <= radius`) and `+0xe`; bit 15 set means surface index in the low 15 bits. |
| `0x001de2c0` BSP surface walk | Confirmed | Surface `+2` first edge; edges at bsp-pointer `+0x34`, 12 bytes: `u16` `+0` start vertex, `+2` end vertex, `+4` forward, `+6` reverse, `+8` left surface, `+10` right surface. Rule: if `right_surface == surface` use end vertex and reverse edge, else start vertex and forward edge. Vertices at `+0x3c`, 16 bytes, position floats `+0,+4,+8`. Surface byte `+4` bit 3 and byte `+5` index a breakable-surface mask. |
| `0x001dea00` reference matcher | Confirmed | Walks a leaf's reference run comparing `ref.plane & 0x7fff` to a plane, using bit 15 as flip; selects the surface from `ref.node`. |
| `0x001ea9c0` `physics_model_postprocess` | Confirmed | Block table and strides of `phmo` (count at root offset, address at +4): `+0x38` 0x90 (rigid bodies), `+0x48` 0x80 (spheres), `+0x50` 0xb0, `+0x58` 0x50 (pills), `+0x60` 0x90 (boxes), `+0x68` 0x60, `+0x70` 0x100 (polyhedra), `+0x78` 0x30 (four vectors), `+0x80` 0x10 (planes), `+0x90` 0x38 (lists), `+0x98` 8 (list children), `+0xa0` 0x14 (mopps), `+0xa8` mopp code bytes, `+0x108` 0x20. Writes vtable pointers at element `+0x20` (list/mopp: `+0`) and `0x80` reference-count words at `+0x26`. Polyhedra: capacity at `+0x58` above 3 selects the external four-vector block (cursor advanced by `+0x54`), else inline at `+0x60`; `+0xf4` points at planes taken in element order, advanced by `+0xf8`. Lists: capacity at `+0x14` above 4 selects the external child block (cursor advanced by `+0x10`), else inline at `+0x18`. Mopps: code cursor advanced by `(u32 at code+0x20 + 15) & ~15`. Root `+0x110` is the first child. |
| `0x001eaf30` shape reference resolver | Confirmed | Switch on shape type with the element stride and the offset of the Havok object inside it: 0 sphere (0x80, +0x30), 1 pill (0x50, +0x20), 2 box (0x90, +0x40), 3 triangle (0x60, +0x20), 4 polyhedron (0x100, +0x20), 5 (0xb0, +0x20), 6 (0x20), 0xe list (0x38, +0), 0xf mopp (0x14, +0). |
| `0x001ef070` rigid-body transform fetch | Confirmed | Rigid body = `index * 0x90 + [phmo+0x3c]`; `i16` at `+0` is the node (-1 = none, otherwise a 0x34-byte node matrix), the pointer at `+0x38` is the resolved shape (reference count at its `+6`). |
| Havok class names, `.rdata` `0x0040de98`-`0x00410178` | Confirmed | `hkSphereShape` `0x40e000`, `hkCapsuleShape` `0x40dfd8`, `hkBoxShape` `0x40ef2c`, `hkTriangleShape` `0x40ed64`, `hkConvexVerticesShape` `0x40edc8` (member `planeEquations` `0x40edac`), `hkListShape` `0x40ee40`, `hkMoppBvTreeShape` `0x40ee8c`, `hkMultiSphereShape` `0x40ecfc`, `hkTransformShape` `0x40e054`, `hkRigidBody` `0x40fde4` with `mass` `0x40fc60`, `centerOfMass` `0x40fc68`, `inertiaTensor` `0x40fc78`. These are class names only; no Havok SDK layout was consulted. |

## UI tag walkers

Appended by the UI tag survey. Found with an `iced-x86` sweep of analyzer
function starts and checked against the disc; field layouts are in
[ui.md](../formats/ui.md). No UI group FourCC appears as an immediate in the XBE: these
functions reach tags through the inline `(datum & 0xffff) * 16` index on
`[0x004e3b44]+8`. Labels are descriptive, not recovered symbols.

| Address | Role | Confidence |
|---|---|---|
| `0x001482e8` | `wgtz` for the current scenario: `matg +0x110` interface element, `+0x84`/`+0x8c`/`+0x94` by `scnr +0x10` type (main menu / 0 / 1) | Confirmed |
| `0x00148350` | `wigl` from `wgtz +0` | Confirmed |
| `0x00236a7f` | `wgtz +0x10` variant-settings tag | Likely |
| `0x00148098` | screen lookup by ID: walks `wgtz +8` (stride 8), compares `wgit +4`; wrappers `0x001480ed`, `0x001480ff` | Confirmed |
| `0x0013e615` | `matg` interface tag reference by index | Likely |
| `0x0014837a` | list skin by index (`wigl +0x138`) | Confirmed |
| `0x0022e27b` | widget base constructor (type in `eax`) | Confirmed |
| `0x001a47fd` | UI widget allocation | Likely |
| `0x0022eb18` | attach widget to parent | Likely |
| `0x0022f5ca` | screen widget constructor (type 0, vtable `0x00458840`) | Confirmed |
| `0x0022f871` | screen widget to `wgit` (screen ID at widget `+0x70`) | Confirmed |
| `0x0022f899` | current pane (`+0x5f0` index, stride `0x4c`) | Confirmed |
| `0x0022f8df` | screen init: flags 0x20 and 2, shape group | Confirmed |
| `0x0022facd` | shape-group widgets (`wigl +0x128`, stride `0x18`) | Confirmed |
| `0x0022fc08` | pane builder: bitmaps, shader bitmaps, tables, text, model scenes | Confirmed |
| `0x0022f6ca` | pane runtime state and buttons | Likely |
| `0x0022fda6` | screen header text (`wigl +0x160` fonts, `+0x168` colour, `+0x178` layouts) | Confirmed |
| `0x0022ff53` | secondary header rectangle | Likely |
| `0x00230134` | widget string ID through the screen's `unic` (calls `0x001a0180`) | Confirmed |
| `0x00230195`, `0x00253c3a` | screen string lists (`wgit +0x30`) | Confirmed |
| `0x0022f28c`, `0x0022f672` | preloaded bitmaps (`wgit +0x38`) | Likely |
| `0x0022fa30`, `0x0022fa5b`, `0x0022fa7d`, `0x0022f9bf`, `0x002baf38` | allocate bitmap (`0x90`), model scene (`0x74`), table (`0x78`), text (`0xf8`/`0x2b8`), shader bitmap (`0x88`) widgets | Confirmed |
| `0x0024bb00` | list widget (type 1, vtable `0x0045a030`); definition `0x0024bbf3`, layout `0x0024bc12` | Confirmed |
| `0x0024bd3b` | skin per-item animation (stride `0x10`) | Likely |
| `0x00253c8b` | list item / button (type 3, vtable `0x0045a628`); `0x00253cc8`, `0x00253cff`, `0x00253ea8`, `0x00254173` read the `0x3c` button | Confirmed |
| `0x00253746`, `0x00253765` | text widget (type 6, vtable `0x0045a5e0`) and init | Confirmed |
| `0x002b01eb` | bitmap widget (type 8, vtable `0x0045ad60`); update `0x002b0328`, draw `0x002b0565`/`0x002b07e2`, meter test `0x002b0a68`, image `0x002b0b19` | Confirmed |
| `0x002b0b5e`, `0x002b0baa` | model scene widget (type 7, vtable `0x0045ada8`) and draw | Confirmed |
| `0x002b1111`, `0x002b11c9` | shader bitmap widget (type 9, vtable `0x0045adf0`) and draw | Confirmed |
| `0x002afd97`, `0x002bac82` | table widget (type 10, vtable `0x0045ad18`); cell layout `0x002afdbc`, skin widgets `0x002bacbc`, bounding box `0x002bafa4` | Confirmed |
| `0x002b41bc` | widget type 5 constructor, role not traced | Tentative |
| `0x0022e65e` | transition track selection (`wigl +0x120`, `0x2c` collections) | Confirmed |
| `0x0022e957` | apply a transition recursively to children | Likely |
| `0x0022e89c` | copy animation state into widget `+0x34` | Likely |
| `0x0022e3cd` | keyframe interpolation (`0x14` keyframes, alpha and offset) | Confirmed |
| `0x002353a5`, `0x002356d8` | draw-list collection and comparator (depth, bias, type) | Confirmed |
| `0x00236299` | UI sound by index (`wigl +0x90 + i*8`) | Confirmed |
| `0x00236235`, `0x0023620d`, `0x0022d456`, `0x00149ead`, `0x0023661f`, `0x00235b46` | read `wigl +0x110`, `+0x118`, `+0x148`, `+0x158`, `+0x88`, `+0x1b8` | Likely |

Constants used by these paths: `0x0045dc70` = 0.001, `0x0045dccc` = 1000.0,
`0x0045dbc0` = 1.0. The UI clock at `0x0054d5b8` counts milliseconds.

## Animation graphs (`jmad`) and animation decompression

Added with the animation decoder (`crates/formats/tags/src/anim.rs`, `anim_sample.rs`;
[animation.md](../formats/animation.md)). Names are assigned descriptions, except codec
and cache names, which are strings the executable itself carries. All addresses
are guest virtual addresses read in Ghidra's decompilation and cross-checked with
`h2 disasm`; every layout claim was then confirmed by decoding every `jmad` tag
and animation on the disc (77,856 animations, zero failures). **Confirmed**
means direct instruction evidence plus the disc check.

### Data path, from tag to bytes

| Address / assigned name | Confidence | Evidence and consequence |
|---|---|---|
| `0x00123180` `animation_cache_initialize` | Confirmed | Creates the "xbox animation" and "xbox predicted animations" data arrays and the LRU "xbox animation cache" (`0x0013d170`). Called from `platform_bootstrap` (`0x00012000`). Animation data is loaded on demand, not with the tag. |
| `0x001237e0` `animation_resource_get` | Confirmed | Takes a resource record (graph `+0xb0`, stride `0x14`), loads it through the cache if needed (`0x001235b0` rounds its size up to `0x200` and reads it, `0x001234a0` evicts), returns the cache base. |
| `0x001236f0` `animation_resource_request` | Confirmed | Queues a prediction/load; records `+0x10` byte `!= 0` gate it (the `flags` byte in the resource record). |
| `0x001dd7c0` `animation_data_pointer` | Confirmed | Animation element = `graph[+0x30] + index * 0x6c`. If element byte `+0x16 & 0x10`: resource record `graph[+0xb0] + element[+0x24] * 0x14`, returned pointer is the cached base plus `element[+0x28]`. Otherwise the pointer is `element[+0x38]` (inline data, never taken on the disc). |
| `0x001ddb40` `animation_view_build` | Confirmed | Fills a 12-byte view: data pointer, `&element[+0x3c]` (the data-sizes struct), node count (`+0x13`), frame-info type (`+0x11`), frame count (`+0x14`). |
| `0x001daea0` / `0x001dafc0` / `0x001dd880` `animation_resolve` | Confirmed | A packed animation reference has the graph in its low 16 bits (-1 = this graph, otherwise a position in the *inheritance list*, graph `+0x4c`/`+0x50`, stride `0x20`, datum at `+4`) and the animation index in its high 16 bits. |
| `0x001dd8f0` `animation_static_pose` | Confirmed | Reads `static_pose` (element `+0x2c`) from the block at graph `+0xb4`/`+0xb8`, stride `0x18`: four i16 (normalised), then translation and scale floats. |
| `0x001dd9d0` / `0x001db170` / `0x001ddb90` `animation_preload` | Likely | Walk the action tables and the int lists of each weapon type (`+0x24`, `+0x2c`) to queue resources. |
| `0x001db170` `animation_action_lookup` | Confirmed | Looks `(mode, class, type)` up through three nested blocks (strides `0x14`, `0x14`, `0x34`; helper `0x001dd560` is a linear search of a block by its leading string ID, stride passed in) and then the action list (stride 8: string ID, graph index i16, animation index i16). Resolved names on the disc confirm the hierarchy (mode, weapon class and weapon type names all resolve to sensible strings). |
| `0x001c7450` `animation_events` | Confirmed | Walks element blocks at `+0x4c` (stride 4: type i16, frame i16; `1 << type` builds the event mask), `+0x54` (stride 8, sound events, index into graph block `+0x14`, stride `0x0c`) and `+0x5c` (stride 4, effect events, index into graph block `+0x1c`, stride `0x0c`). |

### Evaluation and the codec table

| Address / assigned name | Confidence | Evidence and consequence |
|---|---|---|
| `0x002798a0` `animation_evaluate_setup` | Confirmed | Clamps the frame to `frame_count - 1`, sets the current/next frame and the blend fraction (`0x00504464/68/6c`), then selects up to two streams. **Static stream** at blob offset 0 when the animation type (`+0x10`) is 0 and `sizes.static_size` (`+0x42`) is nonzero. **Animated stream** at `static_size` when `sizes.animated_size` (`+0x48`) is nonzero. Flag-array pointers (`0x00504490/94/98`: rotation, translation, scale) are `blob + static_size + animated_size`, plus the signed byte at `+0x3c` for the animated stream; each array is `((nodes + 31) >> 3) & ~3` bytes. The stream's first byte indexes the codec table. |
| `0x00279d80`, `0x00279e40`, `0x0027a060` | Confirmed | Entry wrappers (plain frame, positional blend, interpolated) calling `0x002798a0`. |
| `0x0027a6e0` .. `0x0028c090` | Confirmed | About 60 specialised per-node evaluators (one per codec/blend combination). Each walks the three flag arrays eight nodes at a time, increments the stream-local channel index (`0x005044b4/b8/bc`) for every flagged node, and for nodes in the requested mask calls the codec function. The output record is 32 bytes per node: rotation (4 floats) at `+0`, translation at `+0x10`, scale at `+0x1c`. |
| `0x0047fb14` `animation_codec_table` | Confirmed | Records of `0x28` bytes indexed by the stream's codec byte (record 0 is unused). Record layout: `+0x00` default function `0x0028d170` (returns 0), `+0x04` name pointer, `+0x08`/`+0x0c` two ints, `+0x10`..`+0x24` six function pointers: rotation, translation, scale at an integer frame, then the same three for an interpolated frame. The dispatcher at `0x001c73a8` computes `codec * 5 * 8 + 0x47fb28`. |
| `0x0045a7c8` .. `0x0045a8c4` codec names | Confirmed | `_uncompressed_static_data_codec`, `_uncompressed_animated_data_codec`, `_8byte_quantized_rotation_only_codec`, `byte_keyframe_lightly_quantized`, `word_keyframe_lightly_quantized`, `reverse_byte_keyframe_lightly_quantized`, `reverse_word_keyframe_lightly_quantized`, `_blend_screen_codec`: codecs 1 to 8 in table order. |
| `0x00504460` .. `0x005044c0` evaluation globals | Confirmed | `0x504464` frame, `0x504468` next frame, `0x50446c` blend weight, `0x504480` stream pointer, `0x504484/88/8c` function slots, `0x5044b4/b8/bc` rotation/translation/scale indices, `0x5044c0` output pointer. |

Per-codec functions (rotation / translation / scale at an integer frame; the
interpolated variants are listed after the slash). Stream header: byte 0 codec,
bytes 1 to 3 the number of rotation, translation and scale channels in the
stream, then the offsets listed (all relative to the stream start):

| Codec | Functions | Header fields the functions read |
|---|---|---|
| 1 `uncompressed_static_data` | `0x0028c470`, `0x0028c4e0`, `0x0028c510` (same three for interpolation) | rotations i16x4 at `+0x20 + i*8`; `+0x0c` translations (`i*12`); `+0x10` scales (`i*4`). Frame is ignored. |
| 2 `uncompressed_animated_data` | `0x002c4d60`, `0x002c4da0`, `0x002c4de0` (same) | `+0x14`/`+0x18`/`+0x1c` per-channel strides; rotation float4 at `+0x20 + i*stride + frame*16`; translations at `[+0x0c] + i*stride + frame*12`; scales at `[+0x10] + i*stride + frame*4`. Not present on the disc. |
| 3 `8byte_quantized_rotation_only` | `0x0028c960`, `0x0028c750`, `0x002c4de0` / `0x0028c790`, `0x0028c880`, `0x0028c6b0` | rotation i16x4 at `+0x20 + i*[+0x14] + frame*8`; translation by its own function `0x0028c750` (same stride layout as codec 2's, strides at `+0x18`), scale by codec 2's `0x002c4de0`. |
| 4, 6 byte keyframes | `0x0028cdb0`, `0x0028cf40`, `0x0028d090` / `0x0028c9e0`, `0x0028cb70`, `0x0028ccc0` | per-channel word `count = w & 0xfff`, `first = w >> 12`: rotation words inline at `+0x30`, translation words at `[+0x0c]`, scale words at `[+0x10]`; key times (bytes) at `[+0x14]`, `[+0x18]`, `[+0x1c]` plus `first`; values (i16x4, float3, float) at `[+0x20]`, `[+0x24]`, `[+0x28]` plus `first` times the value size. Bisection for the last key time `<=` frame; blend with the next key. Codecs 4 and 6 share all six functions. |
| 5, 7 word keyframes | `0x0028d560`, `0x0028d6f0`, `0x0028d840` / `0x0028d180`, `0x0028d320`, `0x0028d470` | as 4, with u16 key times. Codecs 5 and 7 share all six functions. |
| 8 `blend_screen` | `0x0028c710`, `0x0028c750`, `0x002c4de0` / `0x0028c530`, `0x0028c5d0`, `0x0028c6b0` | rotation float4 at `+0x20 + i*[+0x14] + frame*16` (the code adds `frame + 2` blocks of 16 bytes to the stride base), translation (`0x0028c750`, not codec 2's function) and scale (`0x002c4de0`) as codec 3. |

What the engine does not say: the meaning of `reverse_` (codecs 6 and 7 use
exactly the functions of 4 and 5; on the disc all of their key times ascend like
the non-reverse codecs), and the two ints in each table record. See
[animation.md](../formats/animation.md) for the open items.

## Bink player and movie wrapper

Identified by hand (no signatures cover Bink); details, argument lists and
evidence are in [bink-hle.md](../audio/bink-hle.md). Calling convention is stdcall.

| Entry | Role | Confidence |
|---|---|---|
| `0x003e0450` BinkSetSoundSystem, `0x003e0af0` BinkOpen, `0x003e2330` BinkWait, `0x003e2630` BinkSetMixBins, `0x003e2680` BinkSetMixBinVolumes, `0x003e2820` BinkSetMemory, `0x003e2830` BinkCopyToBuffer, `0x003e2870` BinkDoFrame, `0x003e2e50` BinkNextFrame, `0x003e2ff0` BinkClose | The only Bink functions called from outside the Bink sections | Likely from call shape and argument counts; BinkOpen confirmed by the `BIKi` compare |
| `0x00156090` MovieStart `(path, flags)`, `0x00156040` tick, `0x001568d0` decode and blit, `0x00156960` draw, `0x00155f80` service, `0x00155f60` IsPlaying, `0x001565a0` stop, `0x00156ab0` skip-button poll, `0x001564e0` remaining ms, `0x00155ea0` / `0x00155ed0` init / dispose | The engine's movie wrapper, state block at `0x004e9188` (handle at `0x004e9194`, finished flag `0x004e918c`, flags `0x004e9190`) | Confirmed by reading the code |
| `0x00223aa8` | Builds `d:\bink\%s_%d.bik` (`0x4575d0`) from a name; names `attract` `0x4575e4`, `credits` `0x4575ec`, `intro` `0x4575f4` | Confirmed |
| Callers: `0x0012bf90` intro (flags `0x1c6`), `0x00163890` intro on entry (`0x3c6`), `0x00223a21` attract from idle timer `0x00223976` (`0xe`), `0x002ab8c0` credits (`0x40c`) | End of movie and skip-on-button are both inside the wrapper (service `0x155f80`, input `0x156ab0`) | Confirmed |

## Effects: damage, contrail, decal, light, lens flare

Static analysis only; names are assigned descriptions. Layouts and evidence are
in [effects.md](../formats/effects.md).

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x0017a8a0` part creation switch | Confirmed | Dispatches on the effect part's type class at part `+8`: `lens`, `MGS2`, `deca`, `jpt!`, `ligh`, `obje`, `snd!`, `char` and others. |
| `0x0017a97f` `jpt!` case -> `0x000d6660`, `0x000d6c80` | Likely | Builds an origin/direction pair from the effect instance and applies the damage effect. |
| `0x0017aa5c` `deca` case -> `0x00177d10`, `0x0017e670` | Likely | Resolves part transform then places a decal on the first-hit surface. |
| `0x0017e670` decal placement | Likely | Reads decal `+0x14` as radius; traces six directions (`0x001697c0`) to find surfaces; calls `0x0017ee20`. |
| `0x0017ee20` / `0x0017ef10` decal add | Likely | Walks a chain of decal tags (`next_decal`), reads `+6` (skip if zero), `+0x14`, `+0x18`. |
| `0x0017b7cb` contrail arrays init | Confirmed | Creates "contrail" and "contrail point" data arrays. |
| `0x0017d23c` decal array init | Confirmed | "decals" data array. |
| `0x0017ab2e` `ligh` case, `0x0017a8bd` `lens` case | Confirmed (dispatch only) | Light and lens flare creation. The light case calls `0x000c0350`; the light array, update and render path (`0x00031c80`, `0x00031170`) are in [e3-lighting.md](../graphics/e3-lighting.md), "Phase 0 findings". Lens flare render not traced. |

## Effects: function evaluator

Static analysis only; names are assigned descriptions. Layout, evidence and the
Rust port (`crates/formats/tags/src/effects/function.rs`) are in [effects.md](../formats/effects.md).

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x0013b390` `function_evaluate` | Confirmed | Takes the data reference in EDI and two floats (input, range input); switches on the first data byte through the 11-entry table at `0x0013bb14`; every path ends in a clamp to `[0, 1]` (`0x0013b5f2`). Types 6, 8 and above 10 return 0. Header flag bit 0 selects a second set and a `(1 - r) * a + r * b` blend. |
| `0x0013bec0` `function_point_fetch` | Confirmed | Returns a pointer to point `k` of set `s`: data `+0x14 + (T[type] * s + 2 * k) * 4`, with `T` the byte table at `0x0044178c`. The same table gives every type's per-set size, hence the blob lengths `0x14 + 8 * T`. |
| `0x0017c900` `function_wave` | Confirmed | Periodic wave: sub-type 0 returns 1; else scales by 36.57143 (1024 / 28), indexes a 1024-byte table at `0x00468d48 + sub * 0x400`, lerps neighbours; sub-types 6 and 7 wrap across the seam. |
| `0x0017ca10` `function_shape` | Confirmed | Eased transition: clamps to `[0, 1]`; sub-type 0 is linear; else indexes a 1024-byte table at `0x0046bd48 + sub * 0x400` using round-to-nearest of `x * 1023 - 0.1`. |
| `0x00246cd0` `property_evaluate` | Confirmed | Reads the 16-byte property `{input, range input, modifier mode, modifier input, data reference}`; evaluates the function; if `(flags & 0xf0) == 0` clamps and maps onto the header floats at data `+4/+8`; then adds (mode 1) or multiplies (mode 2) the modifier input. |
| `0x00372d48` / `0x00373b6a` CRT `pow` / `fmod` | Likely | Used with `x`, exponent and with `1.0` for the fractional part. |

## Effects: particles and particle movement

Static analysis only (no boot). Names are assigned descriptions. Details and
the tag layouts they support are in [effects.md](../formats/effects.md).

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x00173de0` `particle_data_initialize` | Confirmed | Allocates four arrays with their string names: `particle_system` stride `0x54` max `0x80` at `[0x510c74]`, `particles` stride `0x40` max `0x400` at `[0x51ec84]`, `particle_emitter` `0x4c`/`0x100` at `[0x51ec88]`, `particle_location` `0x34`/`0x100` at `[0x51ec8c]`. `0x00175b50` does the same for `effect` (`0x190`/`0x100`, `[0x4ea93c]`) and `effect location` (`0x3c`/`0x200`). |
| `0x00246cd0` `property_evaluate` | Confirmed | Reads a 16-byte property: input, range input, output modifier (1 add, 2 multiply), modifier input, data reference; evaluates the function and applies the modifier. Shared by particles, emitters and movement. |
| `0x0013b390` `function_evaluate` | Confirmed | Switch over function type 0..=10 on the first byte of the function data; flags byte bit 0 selects the ranged variant. Full port in `effects/function.rs`. |
| `0x00248df0` `particle_input_value` | Confirmed | Jump table of 17 input selectors (particle age, per-particle randoms, system and effect scalars, camera-relative value). |
| `0x00247140` `particle_system_update` | Likely | Walks a system's particle list, advances age by rate, frees finished particles, integrates position/velocity, and calls the draw submission `0x002ba3a0`. |
| `0x00249170` `particle_initialize_from_emitter` | Likely | Evaluates emitter properties into a new particle: lifespan as `1 / max(value, minimum)`, a scaled value into the particle's `+0x34`, then an emission-shape switch (jump table `0x00249a74`) that picks position/velocity direction using the game's random state at `[0x4e7408]`. |
| `0x00247fd0` `particle_allocate` | Likely | Takes a slot from the particle array, links it into the system list, seeds per-particle random words. |
| `0x00248450`..`0x00248d90` | Guess | Particle/emitter/location array helpers (new, delete, iterate); referenced from `[0x51ec88]`/`[0x51ec8c]`. |
| `0x002b8e10`, `0x002b9060`, `0x002b9b10`, `0x002ba100` | Likely | Per-system renderers selected by a 4-way type switch at `0x002b8da0`. Not read in depth; the prt3 fields they consume were not identified. |

## Effects: effects, events, parts, particle systems

Static analysis only; names are assigned descriptions. Layouts and evidence are
in [effects.md](../formats/effects.md).

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x00173de0` effect-family arrays initialise | Confirmed | Allocates the `particle_system` (stride 0x54, max 0x80, pointer `0x00510c74`), `particles` (0x40, 0x400, `0x0051ec84`), `particle_emitter` (0x4c, 0x100, `0x0051ec88`) and `particle_location` (0x34, 0x100, `0x0051ec8c`) arrays; names from the string pool at `0x00453b24`/`0x00459e50`. |
| `0x00175b50` effect arrays initialise | Confirmed | Allocates `effect` (stride 0x190, max 0x100, pointer `0x004ea93c`) and `effect location` (0x3c, 0x200). |
| `0x0017a380` `effect_event_process` | Likely | Walks one event's parts: event stride 0x38, part count at event `+0x18`, array at `+0x1c`, part stride 0x38; skips by location, create-in vs underwater flag and the disposition switch (jump table `0x0017a888`), then calls the part-creation switch. |
| `0x0017a8a0` part creation switch | Confirmed | Dispatches on the part's class at part `+8` (see the damage/decal/light subsection). |
| `0x00178eb0` `particle_system_new_from_event` | Likely | Reads the event's particle-system array at event `+0x34`, record stride 0x38, resolves the location (`0x0017ae50`), allocates a `particle_system` entry and sets its fields. |
| `0x00173cb0` particle tag class check | Confirmed | Accepts exactly `prt3` and `PRTM` for a particle system's particle reference (compares `0x70727433`, `0x5052544d`). |
| `0x00174180` `particle_system_delete` | Likely | Frees a `particle_system` entry and walks its child chain. |

## Object definitions and models

These addresses support the tag layouts in [objects.md](../formats/objects.md). Every
row was checked with `h2 disasm`, and with Ghidra where it has a function.

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x00468630` `object_type_table` | Confirmed | 13 descriptor pointers, in the order bipd, vehi, weap, eqip, garb, proj, scen, mach, ctrl, lifi, ssce, bloc, crea. `object_new` indexes it with the definition's `+0` word. |
| Descriptor `+8` / `+0xa` / `+0xc` / `+0xe` | Confirmed | `+8` is the runtime body size, which `object_new` passes to `0x000bc280`. The others are the scenario placement-block offset, palette-block offset and placement stride (`0xffff` for garb/proj), with values per type in objects.md. |
| `0x004678e8` / `0x004679b0` / `0x00467c08` / `0x00468248` | Confirmed | Abstract object/unit/item/device descriptors. Body sizes are `0x12c`/`0x348`/`0x16c`/`0x1cc`. |
| `0x000ba540` `object_set_variant` | Confirmed | Uses the requested variant, else definition `+0x30`. Looks it up in the hlmt at `+0x38` via `0x0016d180` and stores the index byte at object `+0xb1`. Called from `object_new`. |
| `0x0016d180` `model_find_variant` | Confirmed | Null hlmt gives -1. A zero name gives 0 if variants exist, else -1. Otherwise it scans hlmt `+0x50/+0x54` with stride `0x38` and compares the full string ID; no match gives -1. |
| `0x0016d1d0` `model_find_region` | Confirmed | Scans hlmt `+0x70/+0x74` with stride `0x10` by full string ID. |
| `0x0016d764` region-permutation selection (inside a larger body) | Likely | At this instruction, reads the variant's signed region-map byte at `+4+region` and the variant regions at `+0x18` (×`0x14`), then calls `0x0016dad0`. |
| `0x000b8c40` `object_attachment_marker` | Confirmed | Bounds-checks an index against definition `+0x94` (`0x000b8c7a`), then loads element `+8` (stride `0x18`) at `0x000b8c8b`. |
| `0x000becd1` attachment instantiation (inside a larger body) | Confirmed | Walks definition `+0x94`, switches on the element's reference class (ligh, lsnd, effe, cont; skips lens/MGS2/tdtl), and creates instances into object storage at `+0x11e`. |
| `0x000bac0c` object function lookup (inside a larger body) | Likely | Steps definition `+0x64/+0x68` by `0x20`, matching element `+8`, then evaluates through `0x000ba8c0`. |
| `0x000bdf90`..`0x000be075` bounding-sphere update | Confirmed | Transforms definition `+8..+0x10` by the root node and scales `+4` by object scale. |
| `0x0019f1a0` `collection_select` | Confirmed | `itmc`/`vehc`: block at tag `+0`, stride `0x10`, weighted by `+0` f32. Returns the `+8` datum and the `+0xc` word (the `vehc` variant name). The class comparisons against `itmc` and `vehc` are at `0x0015ab15`/`0x0015ab1d`; the calls follow at `0x0015ab29`/`0x0015ab34`. |
| `0x0011c840` resource preparation | Likely | Passes bipd `+0x264` and crea `+0xd4` to `0x001c2390`, and vehi `+0x2ac` to `0x00205550`. A `coll` tag goes to `0x002122b0`. |
| `0x000de4c0` `biped_new` helper | Confirmed | Called by `biped_new` `0x000dc540`; tests a bit in the bipd definition flags at `+0x1f0`. |

`object_new` (`0x000b7b40`) also reads several hlmt fields:

- `+0xc` and `+0x14` null checks on the collision and animation datums.
- `+0x60` element 0 `+0xbc`, as a count of 8-byte entries.
- `+0x70` count: 10 bytes reserved per region.
- `+0x78` count: `0x34` bytes reserved per node.

It also reserves 8 bytes per attachment and `0x18` bytes per change-colour
entry, and spawns the effect at definition `+0x50`. Ghidra has no functions at
`0x000c4410`, `0x000dc540`, `0x000eeb80` or `0x000ee910`; use `h2 disasm`.

## Shader instance setup

Static analysis of the draw-time material path, used to derive the `shad`,
`stem` and `spas` layouts in [shader-instances.md](../formats/shader-instances.md).
D3D identities are XDK signature matches (`h2 symbols`).

| Address / assigned name | Confidence | Evidence |
|---|---|---|
| `0x0001bbf0` `shader_draw_begin` | Confirmed | Checks the tag group is `shad` (`0x0001bcc9`), copies root `+0x3e/+0x40/+0x44/+0x48` to `0x485a64..0x485a70`, calls `0x00016b90`. 13 callers, e.g. `0x00016713`, `0x0003c577`. |
| `0x00016610` `shader_draw_layer_passes` | Confirmed | Walks the template's pass list for a layer and draws each implementation that has a vertex shader (`+0x100 != -1`). |
| `0x00016b90` `shader_setup` | Confirmed | `(state 0x485b48, shad, lod, layer, pass, implementation, size)`, `ret 0x1c`. Resolves shad → stem → spas postprocess records and runs the setup steps below. |
| `0x00016f60` / `0x00017000` / `0x00017170` | Confirmed | Instance bitmaps → texture slots; vectors → vertex defaults/constants (copy-mode table `0x00017130`/`0x00017154`); colours → `PSConstant0/1` or the final constants of the copied PSDEF (lane masks `0x467068`). |
| `0x00018900` / `0x000189a0` / `0x00018a90` | Confirmed | Pass render states (simple via method table `0x43e6f0`, deferred mask `0x43e4d0`, complex setters `0x4038d8`); sampler states into `0x404dd0 + 0x80·stage`; texture-stage bindings (engine sources via `0x00019530`, lightmap from `0x4e0344`). |
| `0x0001ba00` `pixel_shader_build_variant` | Confirmed | Builds general/final combiners from spas fragments/variants/stages, selected by engine integer inputs. |
| `0x0001ae70`, `0x0001b2c0` | Confirmed | Computes engine inputs per implementation mask (scalars `0x000336f0` table `0x338d4`, vectors `0x00033980` table `0x33f8c`, integers `0x00034060` table `0x340e0`); writes one into a register with mode/transform. |
| `0x00019be0` / `0x00019ca0` / `0x0001a170` / `0x0001ab50` | Confirmed | Engine-driven vertex constants, pixel constants, render states (table `0x467020`) and sampler states (table `0x46703c`). |
| `0x000174d0`, `0x00017420`, `0x00017550`, `0x000176a0`, `0x00017960`, `0x00017b60`, `0x00018560` | Confirmed | Instance animation leaf lookup and the five animated-target kinds; properties evaluated by `0x0001b230` → `0x0013b390`. |
| `0x00018d70` / `0x00018e80` | Confirmed | Upload vertex defaults as `NV097_SET_VERTEX_DATA4F_M` and vertex constants via `D3DDevice_SetVertexShaderConstant1Fast` (`0x003f7230`) at hardware `c[18+i]`. |
| `0x0001b0a0`, `0x000143c0`, `0x00014390`, `0x00014f60`, `0x0001cf50` | Confirmed | Frame/mip selection and bitmap binding; engine texture binding by source index; pending textures `0x51f3d8[4]` flushed to `D3DDevice_SetTexture` (`0x003f8320`). |
| `0x0001b050` | Confirmed | `D3DDevice_SetPixelShader` (`0x003fa3d0`) with the patched PSDEF copy at state `+0x1424`. |
