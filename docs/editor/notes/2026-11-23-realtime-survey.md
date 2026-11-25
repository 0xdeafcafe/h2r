# Realtime edits for every tag group: survey and sizing

Evidence for [realtime-edits.md](../realtime-edits.md) ("Realtime
coverage" and slices R8 to R17). Nothing here states what the code does
today; the design page does.

## 2026-11-23: every tag group, read from the code

Commit `db7234eb1` (master). Method: static reading only. Nothing was
built, tested or run. The code read is cited per claim below. The group
list is every `group` line of `crates/formats/tags/src/schema/layouts.txt`
(79 groups, every group on the retail disc), plus the abstract parents
`obje`, `unit`, `item`, `devi` and `lifi` (no `lifi` tag is on the disc,
`crates/formats/tags/src/object/mod.rs:294`).

**Reproduce.**

```sh
rg '^group' crates/formats/tags/src/schema/layouts.txt | awk '{print $2}'
rg -c 'applies=' crates/formats/tags/src/defs/*.txt        # only shad.txt: 3
rg -n 'ShaderLibrary::read' crates/extensions/game/src     # 9 native loaders, all from the file
rg -n 'native_edits' crates/extensions/game/src -l         # hot_glue, world and objects glue only
rg -n 'world_rev \+= 1' crates/tools/scenario-edit/src/session.rs
```

Sizes: **S** is one hook or one consumer, days; **M** is a new refresh
kind or call across a few modules, about a week; **L** needs engine
reading (`h2 disasm`) or a new subsystem, weeks; **XL** has unknown
engine contracts across several subsystems.

### What frames the table

1. **There is no embedded game view.** Studio's Play starts `h2 play` as
   a child process in its own window with `H2_AGENT` set
   (`crates/apps/studio/src/play/launch.rs:1-14`, `play.rs:644-740`) and
   pairs with it as with any other game (`play.rs:714-728`). Embedding is
   design only (docs/editor/editor.md, "Embedded play (not built)"). So
   there are two update targets, not three: Studio's own viewport, and a
   game over the agent socket, whether Studio launched it or the user
   attached it. They differ only in the starting mode (a launched game
   starts in Edit, `play.rs:730-737`) and in the reload route (only
   Studio's own game can be relaunched, `playtest.rs:244-246`).
2. **Native copies never read tag metadata through the overlay.**
   `native_map::open_resolved` reads the `Map` from the plain file
   (`crates/extensions/game/src/native_map.rs:181-190`) and puts the raw
   overlay only on the `ResourceFiles` (`:191-199`). Every native loader
   reads shaders through `tags::material::ShaderLibrary::read(map)`
   (world `native_world_glue.rs:1300`, materials
   `native_world_materials.rs:89`, objects `native_objects_glue.rs:1084`,
   first person `native_first_person_load.rs:228`, fog
   `native_fog_glue.rs:92`, water `native_water_glue.rs:85`, decorators
   `native_decorators_glue.rs:131`, lights `native_lights_glue.rs:1019`,
   post `native_post_load.rs:105`). A tag-field write therefore reaches
   the D3D path (write barrier) and the lifted code, and no native copy.
   With `impl.render.native` on by default, a shader colour edit, which
   hot-iteration.md lists as "next frame", does not show on any natively
   drawn world or object part. Instance constants are baked into uniform
   buffers at build (`crates/rewrites/native-render/src/program.rs:1124-1131`);
   those buffers carry `COPY_DST`, so a rewrite is cheap once triggered.
3. **The native refresh carries raw edits only.** `hot_tags::Refresh` has
   `Section`, `Model` and `Image` (`crates/extensions/hot-tags/src/raw.rs:59-67`);
   a field write in `edit.batch` queues nothing. Only the world and
   objects glue drain the queue (`rg native_edits`); first person, lights,
   decorators, fog, water, layers and post do not.
4. **Studio's own viewport follows geometry previews and placements,
   not tag fields.** It rebuilds on `world_rev` (bumped only when a map
   opens or closes, `crates/tools/scenario-edit/src/session.rs:434,532,547`),
   `modeling_rev` (`GpuWorld::set_section_vertices`,
   `crates/apps/studio/src/viewport.rs:244-255`), the native models, the
   markers and the live stream. Nothing is keyed on the tag documents'
   revision, and painted pixels have no viewport hook.
5. **Studio's effect chips are nearly empty data.** 3 of about 4,850
   definition field lines carry `applies=`, all in `shad.txt`
   (docs/editor/tag-editing.md, finding 4). Every other field reads
   `not_known` and is sent at once (`crates/apps/studio/src/play/tag_sync.rs:164-210`).
   `sbsp`, `ltmp`, `coll`, `phmo` and every BSP tag are `reload` by rule
   (`tag_sync.rs:169-170`).
6. **Studio never sends `moves`.** `TagDoc::block_op` adds, duplicates
   and removes elements in the document (`crates/apps/studio/src/app/tag_doc.rs:699`,
   growth past capacity at `:780`), and `tag_sync::diff` turns every
   layout change into a `reload` row (`tag_sync.rs:297-324`). The game's
   `moves` primitive is built (R4) and unused by the editor.
7. **Host copies outside the renderer**, each read for staleness:
   - `sim_anim::STORE` (`crates/rewrites/engine/src/sim_anim.rs:50`):
     decoded `jmad` clips keyed by the data-sizes address and checked by
     shape (`crates/rewrites/sim-anim/src/store.rs:1-9`), dropped only on
     an epoch move. No hot edit moves the epoch (its only movers are
     `crates/rewrites/sim-bridge/src/sim_native.rs:158-172,209-232`), so an
     edit of keyframe bytes at the same sizes is not heard.
   - `sim_collision`'s BSP world: write-watched, starts a new generation
     on a write to its blocks (`crates/rewrites/engine/src/sim_collision/sources.rs`);
     the one host cache that follows guest writes by itself.
   - `sim_objects::DefTable` (`crates/rewrites/sim-objects/src/defs.rs:1-10`):
     built from guest memory, epoch-scoped, no production reader yet.
   - The HUD kit copies each player `nhdt`'s widget blocks into kit memory
     at activation and repoints the root (`crates/extensions/game/src/hud_kit.rs:797`);
     edits to the original blocks no longer draw until the next activation.
   - Aim assist writes scaled `weap` and `matg` aim fields at each
     activation and remembers what it wrote
     (`crates/extensions/game/src/aim_assist.rs:186`); a hot edit to those
     fields is overwritten at the next activation when the setting is not
     Default.
   - Bot data (nav mesh, `los::Walls`, visibility, spots) is built from
     the disc file's BSP and keyed by a hash of the map header
     (`crates/live/bots/src/cache.rs:1-15`); collision edits are never
     seen.
   - The controller-speaker index fingerprints `snd!` permutations from the
     file (`crates/extensions/game/src/pad_speaker.rs`); it only routes a
     voice and is harmless when stale.
   - DirectSound buffers re-read guest sample memory at `Unlock` and `Play`
     (`crates/platform/dsound/src/state.rs:1529,1596`); no tag copy.

### Shared mechanisms, sized once

Each group row below names one or more of these. The size is the shared
path; per-group glue is sized in the row.

| Id | Mechanism | Today | What realtime needs | Size |
|---|---|---|---|---|
| **F** | Field read in place by lifted code: per frame, per use, or at creation (hot-iteration.md, "What shows when") | `edit.batch` writes it; shows when the engine next reads it | nothing for the game; `applies=` evidence per field so Studio says when (finding 5) | built; data M per group family |
| **D3D** | Device copies keyed on guest addresses: `CachedTexture`, `VertexCache`, NV2A programs keyed by content (`crates/platform/d3d/src/device.rs:649,5130`, `vertex_cache.rs`, `nv2a.rs:207`) | the write barrier re-reads dirty pages (`crates/extensions/game/src/d3d_glue.rs:2174`, `d3d/src/dirty.rs:111`); a new program compiles on first use | nothing | built |
| **NP** | Native-from-packet stages: decals, particles and weather, transparents, HUD and UI, gels, residual sky (`crates/rewrites/render-bridge/src/native_decals.rs`, `native-render/src/particle_gpu.rs`, `native_hud_glue.rs`), rebuilt from the resolved D3D draw | see what the guest pushes next frame; caches keyed by content or dropped per map generation | nothing | built |
| **N-geo** | Native geometry: `GpuWorld`, `LitObjects` | raw edits refresh (R2); tag fields not | first person, light models and decorator geometry drain `native_edits` too; an `sbsp` field write (instance transforms, cluster records) queues `reload_bsp` | S per consumer, M together |
| **N-img** | Native textures from `StageDecoder` | `Image` refresh rewrites world and object programs only (`crates/rewrites/native-render/src/gpu.rs:708-716`, `object_program_gpu.rs:852`) | the same rewrite for lightmap pages (`LightmapKey`), fog, water and layer `StageImages`, decorator looks, the lights' `TEXTURES` (`native_lights_glue.rs:729`), first person; `EffectsSlot` keyed by generation, not map name (`native_post_glue.rs:1171-1178`) | M |
| **N-mat** | Native materials: `ProgramSet`, `opaque_materials::Library`, fog, water and layer sets, light passes, decorator looks, `LitMaterial` (`shad +0x44/+0x48`, `native_objects_glue.rs:1177-1194`), first person, screen effects; all from the file | stale until a map or BSP reload | (a) the metadata overlay, so loaders read edited bytes (below); (b) a `Shader { tag }` refresh: a constants-only change rewrites the uniform buffers; a template, pass, bitmap-slot or vertex-shader change rebuilds that material's programs; fallback `reload_bsp` plus a model reload | L (M overlay + M refresh + S per consumer, 8 consumers) |
| **MO** | Metadata overlay: every applied tag write is also put into `vfs::RawOverlay` at the metadata's file offset (ordinary metadata, or the BSP region's own translation, map-format.md "BSP and lightmap metadata"), and `native_map::open_resolved` reads the `Map` through it | absent | the overlay entry per write and revert in `hot_glue`; the `Map` reader overlaid in live opens; offsets from the cache header's magic per region | M |
| **TW** | Tag-write notice: after a batch applies, `(group, tag, range)` to host subscribers | absent; raw only (`hot_glue.rs:90`) | one notice per changed tag; subscribers: `native_edits` (N-mat, N-geo), `sim_anim::STORE` flush, HUD kit (re-copy the widget blocks), aim assist (adopt the edited value as retail), bot data (mark stale) | S bus, S per subscriber |
| **RAW** | Raw blocks the guest pages in: sections and pixels built (R1); sound permutations (`ugh!`), animation resources and `PRTM` model data are not owners | overlay plus resident write, sections evictable | a raw owner per kind and its eviction from the guest cache: the sound cache (`sound_cache_initialize` `0x2185d0`, docs/audio/playstation-audio.md) and the animation resource are not read | L per new owner kind |
| **ARENA** | Tag blocks that grow: `moves` (R4) | built in the game; unused by Studio | `TagDoc::block_op` becomes a `moves` entry plus the count write; a removal is a count write (and a swap where the engine walks by count) | M |
| **RESPAWN** | Values copied into an object at creation (`obje` family, `hlmt`, `phmo`, `coll` per permutation) | `world.object.respawn` per placement; held, attached and engine-made objects refused (docs/runtime/agent-api.md, "Object edits") | respawn by definition: every object of a definition, placed ones linked, the rest (projectiles, held weapons, vehicles in use) left to their next creation with a reason | M; units held by players L |
| **COLL** | Collision BSPs, MOPP, Havok shapes, pathfinding | R7 design | R7, plus bot data rebuilt from the edited BSP | L (R7) + M (bots) |
| **ONCE** | Read once at map load or game activation (`matg`, `mulg`, scenario globals, netgame flags per game) | next map or next game | per-field evidence; an agent "restart game" (round restart) for next-game fields; reload for map-load fields | S each once known; the evidence is M |
| **UIW** | UI widgets built from `wgit` when a screen opens (docs/formats/ui.md, the runtime widget copies) | next open | "reopen the current screen" action, or accept next open | S to M |
| **INACT** | Tags of a BSP that is not active (`sbsp`, `ltmp`): loaded from the slot file at a switch (map-format.md, "BSP and lightmap metadata") | Studio refuses (not in `loaded`) | the metadata overlay over that region's file offsets: the switch reads the edited bytes | S after MO |
| **NEW** | A tag the map does not hold | reload | tag index slot, datum and salt, metadata in the arena, string IDs, every reference repointed, the native `Map`'s index | XL |

### Every group

"Today" is what a field edit in a running game does, judged from code.
Mechanism ids from the table above.

| Group | Kind | Today | Mechanism | Realtime needs | Size | Risks |
|---|---|---|---|---|---|---|
| `shad` shader | material | D3D draws: next frame (constants and bitmap datums read per draw, E `0x0001717b`, `0x00017039`, `0x00016f60`); native world, objects, fog, water, layers, lights, decorators, first person: never until reload | F, D3D, N-mat, MO, TW | `Shader` refresh over MO | L (shared N-mat) | pipeline compile hitch on a template change; D3D and native must agree on both copies of the frame |
| `stem` shader template | material | as `shad`: D3D programs keyed by content recompile on first use; native stale | N-mat | `Shader` refresh of every shader using the template | S after N-mat | one template feeds many shaders: a burst of pipeline builds |
| `spas` shader pass | material | as `stem` | N-mat | as `stem` | S after N-mat | as `stem` |
| `vrtx` vertex shader | material | D3D: new microcode is a new program key; native stale | N-mat | as `stem` | S after N-mat | microcode the translator refuses falls back to D3D for that part |
| `bitm` bitmap | texture | pixels same size and format: live on D3D and native world and objects (R1, R2); not on lightmaps, fog, water, layers, decorators, lights, first person; header, size, format, new images: reload | RAW, D3D, N-img | N-img completeness; a resize needs the guest texture cache's handle table read | M; resize L | the texture cache's allocation for a changed record is not known (modeling.md, "Not verified") |
| `colo` color table | data | F; no host reader found | F | `applies` evidence | S | none |
| `mode` render model | geometry | sections live (R1, R2, R5); first-person copy and light models stale; new regions, permutations, sections: reload | RAW, N-geo, ARENA | first person and light models drain `Model`; new sections need append plus `moves` plus a model reload | S; new parts L | the guest's object render caches hold section counts |
| `sbsp` structure BSP | geometry, collision | sections live; tag fields (cluster records, instance transforms, materials) reach the guest and D3D, not the native world; collision R7; Studio: all `reload` (`tag_sync.rs:169`) | RAW, N-geo, COLL, ARENA, INACT | `reload_bsp` on any `sbsp` field write (TW); Studio classifies fields instead of refusing the group | S (native) + R7 | a BSP's addresses are its region's: writes only while it is active |
| `ltmp` lightmap | lighting | metadata: reload; pages are `bitm` pixels: D3D live, native skips `LightmapKey` | N-img, INACT | lightmap branch of the `Image` refresh; a rebake writes pages through `raw` (realtime-edits.md, "What is live and what is not") | S (pages) | P8 palette quantisation in the editor |
| `DECR` decorator set | geometry | shaders from file; geometry copied from guest memory once per load (`native_decorators_glue.rs:334,360`) | N-mat, N-geo | drain `native_edits` and TW | S | none known |
| `PRTM` particle model | geometry, effect | geometry is raw, not an owner; fields F | RAW, F | a raw owner kind for its geometry | L | layout of its resource not read |
| `hlmt` model | object | read at creation (variants, region and permutation choice, links to `mode`, `coll`, `phmo`, `jmad`) | RESPAWN | respawn by definition | M (shared) | swapping a linked tag under live objects: objects keep node counts |
| `coll` collision model | collision | `reload` in Studio; R7 design: bullets next tick, bodies on respawn | COLL, RESPAWN | R7 | L (R7) | Havok shapes point into the tag; old extents must outlive bodies |
| `phmo` physics model | physics | primitive sizes in place, on respawn (realtime-edits.md, "Collision") | COLL, RESPAWN | respawn by definition | M | live rigid bodies keep the shape they were built with |
| `phys` physics | physics | F at creation (not read further) | RESPAWN | evidence, respawn | M (shared) | as `phmo` |
| `pphy` point physics | physics | F per particle or contrail tick (effects.md) | F | evidence | S | sim-affecting: films, determinism |
| `bsdt` breakable surface | collision, effect | F at break; tied to BSP surfaces | F, COLL | evidence | S | surface indices: R7's `rebuild` |
| `obje` (abstract) | object | creation-time fields copied into the object | RESPAWN | respawn by definition | M (shared) | object datums are network state |
| `unit` (abstract), `bipd` biped | object | creation, and per-tick reads of speeds and seats (F); players are not respawned by placement | RESPAWN, F | respawn by definition; player bipeds respawn at their next death | L | a player's biped is the seat's state: health, weapons, input |
| `vehi` vehicle | object | creation; physics per tick (F) | RESPAWN, F, COLL | respawn by definition, refusing occupied ones | M | occupants; Havok bodies |
| `item` (abstract), `weap` weapon | object | `next_use` values (rate of fire, damage refs) per shot; creation values at spawn; aim fields overwritten by aim assist at activation | F, RESPAWN, TW | evidence; aim assist adopts edits; respawn skips held weapons | M | held weapons are refused (`world.object.respawn`) |
| `eqip` equipment | object | as `weap` | F, RESPAWN | as `weap` | S after shared | none beyond shared |
| `garb` garbage | object | creation (engine-made) | RESPAWN | next creation; nothing to respawn | S | none |
| `proj` projectile | object | creation values on the next shot; per-tick drag and speeds F | F | evidence only; projectiles are short-lived | S | sim-affecting |
| `scen` scenery | object | creation | RESPAWN | respawn by definition (placed scenery is linked, R3) | S after shared | none |
| `devi` (abstract), `mach` machine, `ctrl` control | object | creation; per-tick machine timing F (live-mirror.md, "next use" row) | RESPAWN, F | respawn by definition; device groups not live | M | device-group readers not disassembled (scenario-editor.md section 7) |
| `lifi` light fixture | object | not on the disc; placement not covered by `placement::BLOCKS` | RESPAWN | with bipeds in R3's create-from-record research | L | none on retail maps |
| `bloc` crate | object | creation | RESPAWN | respawn by definition | S after shared | none |
| `crea` creature | object | creation; AI per tick | RESPAWN, F | respawn by definition | M | actor state |
| `ssce` sound scenery | object, sound | no live object method; Studio keeps it behind reload (`crates/apps/studio/src/play/objects.rs:716-728`) | RESPAWN | add `sound_scenery` to `placement::BLOCKS` | M | sound looping state |
| `effe` effect | effect | F: events and parts read when the effect spawns; particle properties per frame (docs/formats/effects.md, "Simulating and drawing a particle") | F, NP | evidence | S | effect parts that deal damage are sim state |
| `prt3` particle | effect | F per particle per frame (effects.md root field table); native particles draw from the packet | F, NP | evidence | S | none for render fields |
| `pmov` particle physics | effect | F per particle tick | F | evidence | S | particle RNG is the shared sim RNG (`249170`, sim-rewrite.md section 4) |
| `cont` contrail | effect | F; drawn from the packet | F, NP | evidence | S | none |
| `deca` decal | effect | F at placement of a decal; drawn from the packet | F, NP | evidence | S | none |
| `jpt!` damage effect | effect | `next_use` per hit | F | evidence | S | sim-affecting |
| `foot` material effects | effect | F per contact | F | evidence | S | none |
| `ligh` light | lighting | F per light per frame on D3D; native light passes from the file | F, N-mat | `Shader` refresh path covers its material; dynamic-light parameters are read per frame from the light datum | S after N-mat | none |
| `MGS2` light volume | effect | F; drawn from the packet | F, NP | evidence | S | none |
| `lens` lens flare | effect | F; no native copy found | F, D3D | evidence | S | none |
| `tdtl` liquid | effect | F; drawn from the packet | F, NP | evidence | S | none |
| `ant!` antenna | effect | F per frame | F | evidence | S | none |
| `clwd` cloth | effect | F at creation of the cloth state, then per tick | F, RESPAWN | evidence | S | none |
| `egor` screen effect | post | D3D: F; native screen effects load from file keyed by map name, not drawn natively yet (`native_post_load.rs:8-10`) | F, N-mat | key `EffectsSlot` by generation before it draws | S | none until native screen effects draw |
| `fog ` planar fog | lighting | fog words pushed into render state per view: live (`native_fog_glue.rs:15-20`); fog shader from file | F, N-mat | `Shader` refresh | S after N-mat | none |
| `fpch` patchy fog | lighting | F; D3D | F | evidence | S | none |
| `sky ` sky | lighting | sky draws on the D3D residual path (`native_residual_glue.rs`): F live; its model a `mode` | F, D3D | evidence | S | none |
| `weat` weather system | effect | F; particles drawn from the packet | F, NP | evidence | S | none |
| `gldf` lighting globals | lighting | read at map load or per frame: not established | ONCE | evidence | M (research) | none |
| `snd!` sound | sound | fields read when a sound plays (`next_use`); sample data raw in the guest sound cache, not an owner | F, RAW | evidence; a sound raw owner and sound-cache eviction for new samples | S fields; L samples | a playing voice keeps its buffer; WMA streams decode as they play |
| `ugh!` sound gestalt | sound | one per map; read at play through the gestalt; pad-speaker index from the file | F, RAW | as `snd!`; growing the gestalt is ARENA plus raw append | L | the gestalt's blocks are shared by every sound |
| `lsnd` looping sound | sound | F at each loop start | F | evidence | S | none |
| `snmx` sound mix | sound | read at use or at load: not established | F, ONCE | evidence | S | none |
| `sncl` sound classes | sound | as `snmx` | F, ONCE | evidence | S | none |
| `spk!` dialogue constants | sound | as `snmx` | F, ONCE | evidence | S | none |
| `sfx+` effect collection | sound | as `snmx` | F | evidence | S | none |
| `<fx>` effect template | sound | as `snmx` | F | evidence | S | none |
| `snde` sound environment | sound | F when the listener's cluster environment changes | F | evidence | S | none |
| `jmad` animation graph | animation | graph fields F; keyframe bytes cached by `sim_anim::STORE` until an epoch move; animation resources raw | F, TW, RAW | TW flushes the store for that tag; resource edits need a raw owner | S store; L resources | a pose change mid-blend; sim-affecting |
| `trak` camera track | camera | F when a track plays | F | evidence | S | none |
| `char` character | AI | read through `tag_get_checked` when an actor decides (`crates/extensions/game/src/h1_ai_control.rs`); actor creation copies are not read | F, RESPAWN | evidence; respawn squads | M | actor state, campaign only |
| `styl` AI style | AI | as `char` | F | evidence | S | none |
| `adlg` AI dialogue globals | AI | read at use: not established | F | evidence | S | none |
| `mdlg` mission dialogue | AI | read when a line plays: not established | F | evidence | S | none |
| `udlg` dialogue | AI | as `mdlg` | F | evidence | S | none |
| `nhdt` HUD definition | HUD | root and widget datums read per frame (docs/game/hud-kit.md); widget blocks copied by the HUD kit at activation | F, TW | HUD kit re-copies the edited blocks on TW, or edits target the copy | S | the kit's repoint is a guest pointer into kit memory |
| `hudg` HUD globals | HUD | F per frame for most draws: not established | F | evidence | S | none |
| `hmt ` HUD message text | HUD, text | strings read when a message shows; a new string grows the block | F, ARENA | evidence; `moves` for growth | S | string IDs must exist in the map's table |
| `wgit` screen widget | UI | built into a runtime widget when the screen opens (docs/formats/ui.md) | UIW | reopen action | S | an open screen holds pointers into the tag |
| `skin` list skin | UI | as `wgit` | UIW | as `wgit` | S | as `wgit` |
| `wgtz` UI globals | UI | through `matg+0x110` at use (docs/engine/engine-map.md) | F, ONCE | evidence | S | none |
| `wigl` UI shared globals | UI | as `wgtz` | F, ONCE | evidence | S | none |
| `unic` unicode string list | text | strings read when shown; growth and new string IDs: reload | F, ARENA, NEW | `moves` for longer strings; a new string ID is NEW | S; new IDs XL | the string ID table is not grown anywhere |
| `sily` text value pairs | UI | read when the settings list builds | UIW | as `wgit` | S | none |
| `goof` variant settings UI | UI | read when the screen builds | UIW | as `wgit` | S | none |
| `matg` globals | globals | read through `[0x4e034c]` at use for many fields (docs/runtime/level-boot.md); aim assist overwrites its fields at activation; some copied once (not listed anywhere) | F, ONCE, TW | per-field evidence; aim assist on TW | M (research) | sim-affecting: player physics, damage, grenades |
| `mulg` multiplayer globals | globals | read live by host code (`pad_lightbar.rs:58`, `bots/arms.rs:288`); engine reads: not established | F, ONCE | evidence; round restart for game-start fields | S | game rules are host state in networked games |
| `itmc` item collection | globals | read when the spawner picks (netgame equipment's per-tick spawner, scenario-editor.md section 6) | F | evidence | S | none |
| `vehc` vehicle collection | globals | as `itmc` | F | evidence | S | none |
| `scnr` scenario | placements, scripts, triggers, squads | placements in 8 blocks live (R3, R6); field edits `next_spawn`; spawn points next spawn; netgame flags next game; trigger volumes, squads, device groups, sound scenery, names, scripts: reload | F, ARENA, RESPAWN, ONCE | create-from-record for the refused shapes; the trigger and squad readers read | L (shared with `lifi`, bipeds); squads XL | placement indices are network state; checkpoints restore old indices |

### Edits that change sizes

- **Tag blocks** grow through `moves` into the 4080 KiB edit arena
  (`hot_tags::arena::WINDOW`), bounded by free physical memory with a
  2 MiB floor; `physical_free` on a loaded map is not measured. A removal
  is a count write, and is safe only where the engine walks the block by
  its count; a block another block indexes (palettes, squads, the
  material list a section names by index) needs the indices rewritten in
  the same batch. Studio never sends `moves` today (finding 6). The send
  side is M; knowing which blocks are indexed from elsewhere is the
  `scenario-editor.md` section 6 table for `scnr` and is not written for
  the other groups.
- **Raw resources** grow through `edit.raw.append` and an eviction, for
  sections only. A bitmap's new size or format and a sound's new sample
  data change what the guest's texture and sound caches allocated; both
  caches' handle tables are not read. L each.
- **Data fields** (`Kind::Data`, strings, function data) that grow are a
  `moves` of the data block; Studio refuses a data resize
  (`tag_doc.rs:926`).

### Edits to tags not loaded yet

- A tag of the map is always in the tag index; its metadata is resident
  for the whole map (`crates/extensions/hot-tags/src/index.rs`), so "not
  loaded" means one of three things:
  - **A BSP that is not active** (`sbsp`, `ltmp`): loaded from the slot
    file at a switch. The metadata overlay over the region's file offsets
    makes the switch read the edit. S after MO.
  - **Raw data not paged in**: the overlay alone is enough (R1).
  - **A tag of another map, or of the shared maps' metadata**: no live
    path; it is the next load of that map.

### Whole new tags

No live path, and none is close. A new tag needs a free index slot and a
salted datum (`TAG_ENTRIES` `0x004e3b44`), metadata in guest memory (the
arena), new string IDs in a table nothing grows, references from existing
tags repointed in the same boundary, the native `Map`'s index to learn it,
and for an object definition every engine cache keyed by definition
index. Its readers are spread over the engine. XL; reload stays the path.

### The views

| View | How an edit reaches it | What is missing |
|---|---|---|
| Studio's own viewport | directly from the documents: `modeling_rev` for geometry, markers for placements, `world_rev` only on open | tag fields (materials), painted pixels, models: no revision drives a rebuild. The viewport uses the same `native-render` types (`GpuWorld`, `ProgramSet`, `LitObjects`), so R10 and R11's refresh functions serve it fed from the `TagDoc` instead of the overlay; M |
| The game Studio launches | the agent socket: `edit.batch`, `world.placement.edit`, `world.object.*`, `rules.reload` (`crates/apps/studio/src/play/edits.rs:450-585`) | whatever the game side lacks, per row above |
| An attached game | the same link and methods; starts in View | the same |
| An embedded picture (not built) | design: frames over the socket (docs/editor/editor.md, "Embedded play (not built)") | `render.picture`; L |

### Risks across the board

- **Determinism.** Every write lands before `game_tick`
  (realtime-edits.md, "Rules and limits"); render-only groups (materials,
  textures, geometry, HUD, UI) never change the sim. Sim-affecting groups
  (`weap`, `proj`, `jpt!`, `matg`, `phmo`, `coll`, `jmad`, `pmov`, `char`)
  make the session's default film unreplayable; the first write already
  discards it. Lockstep hashes all guest RAM every tick
  (`crates/extensions/lockstep/src/lib.rs:1-17`), which is why co-op is
  refused.
- **Networked games.** A host's tag pokes stay on the host. Clients keep
  their own tags: sim-affecting edits make the host and its clients
  disagree (their prediction, their HUD, their effects), so a host with
  clients should be limited to render-only groups, not only confirmed by
  peer count.
- **Guest pointers into tag memory.** Havok shapes into `coll` and the
  MOPP; the HUD kit's repointed root; an open UI screen's runtime widget;
  the guest geometry, texture and sound caches' handles. A `moves` of a
  block one of these holds by address leaves the holder on the old copy
  until it is rebuilt.
- **Tag memory growth.** The arena's window is 4080 KiB, physical memory
  is the real bound, and nothing frees an extent before the map ends but
  a revert. Long sessions of block growth on a large map can reach a
  refusal; `edit.arena` reports it.
- **Snapshots.** An overlay entry or an arena extent refuses snapshots,
  rewind and film keyframes until the map changes; the metadata overlay
  (MO) adds overlay entries for plain field writes, so every field edit
  would refuse snapshots unless MO's entries are excluded from that rule
  (they mirror guest bytes a snapshot already holds).

### Recommended order

By what is edited most and what is missing most:

1. **Shaders** (R8, R9, R10). The most edited kind, and the native
   default hides every shader edit today. TW and MO are small and shared
   by what follows.
2. **Bitmaps and models, completed** (R11). The paths exist; the gaps are
   consumers that do not drain the queue.
3. **Studio's own viewport** (R15). Reuses R10 and R11 against the
   documents.
4. **Object definitions** (R12, R13): respawn by definition, and
   `applies=` evidence for `weap`, `proj`, `jpt!`, `vehi`, `bipd`, `effe`,
   `prt3`, `nhdt`, `matg`.
5. **Block growth from Studio** (R14): every group's add and remove.
6. **HUD and UI** (TW subscribers, the reopen action).
7. **Collision** (R7, then bots' data).
8. **Sounds** (R16) and **inactive BSPs** (R17).
9. **Lighting**: lightmap pages ride R11; a rebake is pages sent through
   `raw` once a relighter produces them.

### The three hardest

1. **Whole new tags** (XL): the index, datums, string IDs and every cache
   keyed by tag index, across the engine and the native `Map`.
2. **Collision that changes indices** (R7 `rebuild`): Havok's shapes and
   cached MOPP keys, each object's location, pathfinding's surface
   references and the bots' offline data all hold indices or addresses;
   only a BSP switch refreshes some of them.
3. **Raw resources that change size** (bitmaps resized or reformatted,
   new sound samples, animation resources): the guest's texture and sound
   caches allocated for the old record and their handle tables are not
   read, so eviction and re-paging is engine research before it is code.
