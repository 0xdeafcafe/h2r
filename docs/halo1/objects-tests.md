# Halo 1 objects: tests

The tests of the object conversion, split from [objects.md](objects.md).

| Section |
|---|
| [Tests](#tests) |

Part of [objects.md](objects.md).

## Tests

```sh
cargo xtask test -p h1objects                   # synthetic: templates, coll, tree install
H1_ISO=<halo1 iso> H2_DISC=disc/halo2.iso cargo xtask test --disc -p h1objects
```

`crates/tools/h1objects/tests/collision_disc.rs` (`H2_MAPS=` limits the Halo 2 maps,
`H1_LEVELS=` the Halo 1 levels, default a10 and b40):

- `retail_static_scenery_carries_bsp_physics`: the slot rule, each
  slot's code keying `0..surfaces` and its box holding the BSP, on every
  object of every Halo 2 map; prints the `hlmt` region bytes seen.
- `campaign_collision_converts_and_answers_as_halo1`: every `coll` on the
  level converted with BSP physics, built, read back and validated; on
  every BSP, 64 rays and 64 points through its box (a quarter wider) give
  the same answer from the read-back Halo 2 tree as from Halo 1's, and
  every surface within a centimetre of a ray's entry point is a key the
  slot's code emits there. A crate's level walk through its middle stops.
- `level_objects_select_collision_with_bsp_physics` (`H1_LEVEL=`,
  default a10): the level's objects through the converter on 05a: slots
  on scenery `coll`s, region 0 of every `hlmt` with a `coll` selects a
  collision region; prints the bytes the slots add.

`bloodgulch_sandbox_converts_and_reads_back` converts the twelve tags of
the proof set (52 Halo 2 tags with their models, collisions, animations,
projectiles and `hlmt`s), installs them over donor slots in Coagulation,
builds the map, and reads it back: every object decodes through
`tags::object::Context::resolve_object` with its `hlmt` -> `mode` (all),
`coll` (all with a Halo 1 collision model), `jmad` (cyborg, warthog,
ghost) and `phmo` (cyborg, warthog, ghost); every converted `coll` passes
`tags::collision::Collision::validate`; and every reference word pair in
the new tags (136) names a tag of that group in the built map.

`crates/tools/h1objects/tests/particles_disc.rs`: `every_pctl_part_becomes_particle_systems`:
every `pctl` effect part (201 over the conversions) becomes one system
per lasting particle state (402), on that state's `prt3`, at the part's
location, the type's count over the event, the state's lifespan and size.
`every_effect_particle_becomes_a_particle_system`:
every Halo 1 effect particle whose `part` converts (42,266 over the
campaign and multiplayer conversions) is a particle system of its event
on the converted `prt3`, at its location, its rate times 30 times the
event's duration its count, its speed, cone and size Halo 1's.
`every_particle_converts`: every Halo 1
`part` of the campaign and multiplayer maps (2,301 conversions) reads
back and writes the same, names its blend function's stem, its bitmap as
parameter and compiled bitmap, Halo 1's sequences and converted effects.
`retail_particle_shaders_compile_from_their_stems`: the compiled shader
rule on 533 retail particles.

`every_contrail_converts`: every Halo 1
`cont` of the campaign and multiplayer maps (191) converts, reads back
through the definition with Halo 1's spans at Halo 2's offsets and its
bitmap converted; every rebuilt projectile and vehicle whose Halo 1
attachments name a contrail (236 attachments) names the converted one.
