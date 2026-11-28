# The Pogo Stick's static reads: evidence

Evidence for [pogo-stick.md](../pogo-stick.md) ("The loadout", "The
camera", "The pose", "The kit"). Nothing here states what the code does
today; the design page does.

## 2026-11-23: slice P0, read from the retail image and this repository

Commit `d5ed5c4bf`; retail 1.0 `default.xbe` through `$H2_DISC`. Static
reading only: `h2 disasm` for the image, and this repository's code and
docs for the rest. No game run, no build.

**Reproduce.**

```sh
h2 disasm "$H2_DISC" 0x19eda0 90    # the starting weapon pick
h2 disasm "$H2_DISC" 0x19ef40 140   # the spawn's weapon pass, its callers
h2 disasm "$H2_DISC" 0xcd5c0 40     # "the unit already holds this definition"
h2 disasm "$H2_DISC" 0xcd0c0 120    # unit_add_weapon
```

Labels: **E** read from the image at the address given, **Code** read
from this repository at the path given, **S** supposed.

### The starting weapon pick `0x19eda0` (E)

- No frame; padding before it. Arguments: `[esp+4]` the default tag,
  `[esp+8]` the variant's weapon value (a byte, sign-extended),
  `[esp+0xc]` the other weapon's tag. Every exit is `ret 0xc` with the tag
  datum in `eax`.
- Value 1 gives `-1` (no weapon). Value 2 (random) calls itself and
  returns to `0x19ee18`.
- Nothing in it, or after it in the pass, checks that the tag it gives is
  in the multiplayer globals' weapon list.

### The pass `0x19ef40` (E)

- The primary value is the variant's byte at `+0x212` of the record
  `[0x4e6948]` points at, the secondary at `+0x213`.
- Call order and return addresses:
  - primary random (value 2): the secondary is picked first, with the
    other tag `-1`, returning to `0x19f042`; then the primary, returning
    to `0x19f056`;
  - otherwise: the primary, returning to `0x19f063`; then the secondary
    with the primary's tag as the other, returning to `0x19f07b`.
- So a call returning to `0x19f042` or `0x19f07b` is the secondary's, and
  the routine has no player argument: the pick is the same for every
  spawn of a game.
- After the picks, two slots (`[esp+0x14]` primary, `[esp+0x18]`
  secondary); a `-1` is skipped at `0x19f085`. For each tag:
  - `0xb7930(tag, -1, 0)` with `esi` the placement, then `0xb7b40`
    (object new); a `-1` object is skipped;
  - `0xcd5c0` (`eax` the unit, one argument, the new object's definition)
    gives 1 when one of the unit's four weapon slots already holds that
    definition, and then the new object is deleted (`0xb8540`);
  - otherwise `0xcd0c0(unit, weapon, 1)`: `unit_add_weapon`, mode 1.

### What can refuse the stick (E)

`0xcd0c0` refuses a weapon whose object has bit 7 of `+4` set, or bit 0
of `+0x12c`, or whose class the unit has no `idle` for (`0xcd6a0`, which
passes the weapon definition's class and type to `0x10fcd0`;
[objects.md](../../halo1/objects.md)). The pass ignores its result, so a
refused weapon's object stays in the world (**S**: read from the pass not
testing `al`; a run shows it).

### The weapon class (Code)

Both player unit graphs must hold an `idle` for the stick's class and
type. The kit reads the pair from the owner's map when it builds
(`kit::player_class` and `kit::choose_class`,
`crates/tools/content-kit/src/kit.rs`): the first pair both graphs hold,
preferring the one-handed pistol pair. Nothing about a
weapon with no first-person animations is in the image reads or the docs:
whether readying or swapping to it faults is for a run.

### The camera route (Code)

- `spectator_view::draw_world_view`
  (`crates/extensions/game/src/spectator_view.rs`) hands the world
  renderer a private copy of a view with the player index forced to `-1`.
  Forge (`forge_controls/render.rs`), Theater (`theater/camera.rs`) and
  the agent's spawn view (`agent_spawn_view.rs`) use it.
- In such a view the local player's biped is an ordinary entry of the
  visibility list and is drawn: Forge has to hide its parked biped by
  hand (`forge_controls/render.rs`, the `OBJECT_RENDER_SETUP` hook).
- The HUD is a separate draw (`points::UI_HUD_DRAW`); Theater hides it by
  hand (`theater/camera.rs`), so it is drawn otherwise (**S** until a run).
- The other route, the director's following camera (`0x23cbb0`, the
  director's update at `0x4e8c50`), bisects the unit's camera track by
  look pitch, and the default unit track runs the wrong way
  ([rebuild.md](../../halo1/rebuild.md)); it would need a camera track
  tag of ours and changes what the director reads. Not taken.

### The node rows (Code)

- An object's world node rows are in its body
  (`guest_layout::Object::nodes`,
  `crates/hooking/guest-layout/src/object.rs`): 0x34 bytes a row, scale,
  forward, left, up, position.
- The sim writes them once a tick (`points::OBJECT_NODE_MATRICES`). A
  held weapon's rows are computed there from its parent's node; nothing
  at render time derives them again.
- `points::OBJECT_RENDER_SETUP` copies the body's rows into the render
  palette, for the native objects pass and the D3D path alike
  (`crates/rewrites/render-bridge/src/native_scene_lists/setup/palette.rs`).
  So rows written into the body before `points::RENDER_VIEWS` are the
  rows both paths draw.
- Nothing in the repository edits rows; the readers are
  `h1_hud_text/weapon/origin.rs` and the agent's hitbox view.
- A film hashes guest memory after each tick
  ([film-format.md](../film-format.md)), and Havok, markers and bounds
  read the rows in the sim, so posed rows must be put back before the
  next tick.

### The shader (Code)

No helper clones a shader with a new bitmap. `h1convert::shad::Library`
(`crates/tools/h1convert/src/shad.rs`) writes a `shad` from a template's
definition with bitmaps of ours bound to its roles, and reads its
templates from the map it is given. The kit uses it, so no retail shader
instance is copied. Whether its templates resolve from every multiplayer
map is for the kit's disc tests (`every_multiplayer_map_holds_the_class`
and `every_multiplayer_map_has_the_room`, both `#[ignore]`).

### Tag headroom (Code)

The kit adds 17 tags to a map: five a look (`bitm`, `shad`, `mode`,
`hlmt`, `weap`) and two shared. Bitmap pixels and geometry are raw pages
outside tag memory. The headroom check is
`map::writer::max_metadata_size` against the built layout
(`crates/tools/content-kit/src/inject.rs`, `Costs`);
`every_multiplayer_map_has_the_room` prints it per map.
