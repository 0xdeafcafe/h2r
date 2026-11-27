# Firefight: how the retail multiplayer collision marks sky

Evidence for [firefight.md](../firefight.md), "Drop pods" (probe P2).

## 2026-11-27: no sky material in the collision of five maps

Read offline from the owner's disc with the branch's `sky` code path (kit v-current), at the commit that adds
`sky::sky_materials`. Nothing was run in the game.

**Method.** For each structure BSP, the materials block (`sbsp+0xa4`, 0x20 bytes an element, the shader tag
reference at `+8`, datum at `+0xc`) gives each collision surface material (`Surface::material` indexes it) a shader
name. The collision surfaces were grouped by material with their height range, mean normal z and flags. A throwaway
reader over `tags::collision::Bsp::decode_structure` did it; it is not kept.

**Result.** Coagulation (85 materials), Zanzibar (93), Beaver Creek (42), Lockout (54) and Warlock: no shader of any
material has a path component `sky`. The tall surfaces with no lid are invisible two-sided walls (flags 3, normal z
about 0, up to z 115 on Coagulation, 16 on Beaver Creek): a boundary round the map, not a ceiling. The highest
surface on each map is such a wall. So open sky over a start is a ray to the top of the collision that hits nothing,
and `sky_materials` is empty on these maps; a map that does mark sky by shader name gets the pass-through.

**Not settled.** Whether a roofed start's ray is closed by the right surface on each map is for the coordinator's R6
run (`h2 firefight kit MAP --report` shows `pod starts: N of M open`).
