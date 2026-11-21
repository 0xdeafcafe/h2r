# Guest reads the editor would serve

Evidence for [polish-plan.md](../../editor/polish-plan.md) (P15, "The game's material passes") and [native-world.md](../native-world.md). Nothing here states what the code does today. Entries are newest first.

## 2026-11-21: which guest reads are tag data and which are run state

Commit: `1157cbe48`. Method: every function taking or calling `scene_extract::GuestRead` in `crates/rewrites/native-render/src` and `crates/rewrites/scene-extract/src` was read (`rg -n GuestRead` over both), and its address arithmetic and comments classified. Tests, `scene-extract/src/testing.rs` and small pass-through helpers (`word`, `half`, `float`, `element` and the like) are not counted. Re-run by repeating the read at a later commit.

Classes: **tag**, an address reached only through tag memory; **run**, the game's run state (object and render datums, render globals, D3D target tables, the stack of a hooked call) or a constant in the executable's data, which the map does not hold either; **mixed**, starts from run state and reads tag data through it; **unsure**, the code does not settle it.

| | native-render | scene-extract | total |
|---|---|---|---|
| tag | 0 | 5 | 5 |
| run | 21 | 52 | 73 |
| mixed | 2 | 28 | 30 |
| unsure | 0 | 5 | 5 |

**The material path reads no guest memory.** `layer_pass::Materials` is answered from a library read from the map file: `tags::material::ShaderLibrary::read(map)` in `native_layers_glue::load` (`crates/extensions/game/src/native_layers_glue.rs`), which resolves each world material and its instance (`shad +0x3c`, `+0x44`, `+0x48`). `layer_pass.rs` and `program.rs` name `GuestRead` only in a test.

**The five tag readers** are `light_groups::tag` and `light_groups::material`, `layout::object_nodes::section_node`, and `layout::decorators::implementation` and `definition`. Each reaches tag memory through one of three pointer words: `0x004e3b44` (the tag instance table), `0x004e0348` (the active BSP) and `0x004e0350` (the scenario). A reader serving tag memory answers those three words as well as the tag space.

**The mixed readers** start from a render record, a sorted dispatch slot or an object datum (run state) and then read the BSP's clusters, placements and definitions (`sbsp +0x9c`, `+0xa0`, `+0x138` to `+0x144`), a `mode`'s sections (`+0x28`), a shader (`+0x16`, `+0x24`) or the implementation word at slot `+0x10` then `+0x100`. They are in `extract.rs`, `light_groups::entry` and `push`, `light_list::sorted_at`, and `layout::{object_walk, world_ranges, object_nodes::node_rows, decorators::read, post::PostFrame::read}`.

**Unsure**, with why:

- The section data pointer at `sbsp` section `+0x50` and `mode` section `+0x34` (`world_ranges::Emitter::range` and `subpart`, and the geometry reads of `light_groups::entry` and `object_walk::read_part_layout`). `light_groups` treats zero there as unreadable, and the decorators check a residency byte before reading geometry, so the geometry cache may fill it at run time.
- `layout::decorators::resident`: a byte at a tag address whose value the cache sets.
- `layout::object_lighting::Defaults::read`: `[0x004686d4]+4` and `[0x00468710]`, pointers whose targets the code does not name.
- `layout::layer_inputs::integer`: `[0x004858c4]` `+0x80`, `+0x8a`, `+0`, named a light definition; it may be light tag data.

**The run readers** cover the camera record (`0x004b9da0`, projection `0x004b9e14`), the render lists (`0x00467130`, `0x00467138`), the render records, the object array (`0x004e0300`) and render state cache, the render targets (`0x004b4b58`, `0x004b4bf0`), fog, glow, the HUD and screen quads, shadows, particles and effect geometry, the layer inputs (render time `0x004858a0`, the material, fade, light and view blocks) and the executable's constant tables (stream formats `0x0043f408`, blend tables, decorator constants).

**`program::ViewInputs` is not read from guest memory.** It is built only by the game's glue (`native_world_glue`, `native_layers_glue`, `native_lights_glue`, `native_decorators_glue`, `native_fog_glue`, `native_water_glue`, `native_first_person_glue`, `native_objects_glue`), from the D3D device: `constants` are the device's vertex constants handed over by `bind_external` (`crates/platform/d3d/src/device.rs`) and widened by `widen_projection_rows`; `guest_size` and `z_scale` come from `Device::external_ready`; `w_clip` from `Device::depth_clip`; `size` from the target copy. No function computes them from a camera.

**`framescene.md` section 4.5** names no editor reader; it says the editor and `viewer` become readers later by building scenes without a guest.
