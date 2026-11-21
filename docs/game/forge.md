# Forge for Halo 2 multiplayer maps

Status: Reference. Nothing of Forge is built.
Code: none. The blocks below are read and written by `crates/tools/scenario-edit` ([scenario-editor.md](../editor/scenario-editor.md)).
Switch: none.

What Halo 3's Forge is, and what retail Halo 2 has that a Forge sits on:
the scenario placement blocks, how multiplayer maps place weapons, the
netgame equipment spawner and object creation. The design of Forge
itself (the document, the edit log, sharing, the UI, the slices) is in
[forge-design.md](forge-design.md) and [forge-features.md](forge-features.md).

No game data is in this page. Addresses are guest virtual addresses of the
retail XBE (the build in [haloscript.md](../formats/haloscript.md)), offsets are
`scnr` offsets, and counts are measurements over the owner's disc. No tag
names, strings or tables are reproduced.

## Evidence labels

- **E**: read from the XBE with `h2 disasm` (this page or a linked doc).
- **V**: measured over every scenario on the local disc with a scratch
  reader in `$TMPDIR` (deleted afterwards), using the `map` crate.
- **C**: community or public source, cited, summarised in our words.
- **S**: speculation or a design choice; check it before building on it.

## 1. What Forge is, and what Halo 2 has

### 1.1 Halo 3 Forge (C)

- A **map variant** is a base map plus a list of placed objects. The
  geometry and lighting never change; players add, move and remove objects
  from a per-map palette ([Halopedia: Forge][hp-forge]).
- **Monitor mode**: a player toggles between their Spartan and a flying
  editor body, picks objects up, rotates and drops them, and spawns new ones
  from the palette ([Halopedia: Forge][hp-forge]).
- **Budget**: each palette item has a cost and a per-item limit, and the
  map has a total budget; there is also a separate overall object-count cap
  that every object counts against equally ([Halopedia: Sandbox][hp-sandbox],
  [GameFAQs thread][gf-limit]; the latter is anecdotal).
- **Per-object properties** in the Halo 3 map-variant file include placed
  at start, respawn time, team, spare clips (or teleporter channel / spawn
  order), a game-type mask and symmetry flags. Positions are quantised
  against the map's bounds, so precision is limited by the format
  ([HaloMapStudio v1.8.0 notes][hms]).
- **Sharing**: variants were saved to the console and shared through the
  File Share on Bungie.net (C, common knowledge; slot counts not checked).
- **Network**: Forge runs as a hosted game: one console is the host and the
  others replicate from it (C, [Aldridge, GDC 2011][gdc-reach], which is
  about Reach; that Halo 3's Forge used the same hosted model is S).

Halo 2 Anniversary's Forge in MCC is a different engine (the Halo 4-based
multiplayer of Halo 2 Anniversary) and is only useful as UI inspiration.

### 1.2 What retail Halo 2 has

**No Forge, no map variants.** Halo 2 has game variants only: a `0x130`
byte settings record at `G+0x13c` (E, [live-gamestate.md](../live/live-gamestate.md)
§3) with built-ins from a `goof` tag and saved copies through XAPI
save games (E, [community-gametypes.md](community-gametypes.md)). A game
variant chooses rules, and through settings such as "weapons on map",
which weapon sets appear. It never says *where* anything is.

Neither Halo 2 Vista community project we checked has a Forge. Project
Cartographer adds multiplayer over its own network and modes such as Gun
Game ([cartographer][pc]); H2Codez extends the PC editing kit (Sapien,
Guerilla, Tool) and gives it a static playtest view, not an in-game
editor ([H2Codez][h2codez]) (C). The kit's Sapien is the real Halo 2
placement editor: everything it writes ends up in the scenario blocks below.

**The scenario placement blocks.** The object type table at `0x00468630`
gives each object type's placement block, palette block and placement
stride in `scnr` (E, [objects.md](../formats/objects.md)). The multiplayer blocks
after them were located for this page:

| `scnr` offset | Block | Stride | Evidence |
|---|---|---|---|
| `+0x50` / `+0x58` | scenery / palette | `0x5c` / `0x28` | E (type table), V |
| `+0x70` / `+0x78` | vehicles / palette | `0x54` / `0x28` | E, V |
| `+0x80` / `+0x88` | equipment / palette | `0x38` / `0x28` | E, V |
| `+0x90` / `+0x98` | weapons / palette | `0x54` / `0x28` | E, V |
| `+0xa8` / `+0xb0` | machines / palette | `0x48` / `0x28` | E, V |
| `+0xd8` / `+0xe0` | sound scenery / palette | `0x50` / `0x28` | E, V |
| `+0x100` | player starting locations | at most `0x34` | V; readers in `0x0014bc80`, `0x0014ef00`, `0x00152660`, `0x001529a0`, `0x0023cea0` (E) |
| `+0x118` | netgame flags | at most `0x20` | V; 24 readers of `+0x11c`, clustered at `0x0019cb90` and `0x00241800..0x00243c00` (E) |
| `+0x120` | netgame equipment | `0x90` | E (below), V |
| `+0x128` | starting equipment | at most `0x9c` | V (one entry per MP map); reader `0x0019ef40` (E) |
| `+0x328` / `+0x330` | crates (`bloc`) / palette | `0x4c` | E (type table) |

"At most" strides are the gap to the next block divided by the count (V),
not yet confirmed by an engine loop. The names for `+0x100`, `+0x118` and
`+0x128` are unverified labels; the offsets and readers above are ours.

**How MP maps place weapons (V, E).** On the eleven retail multiplayer
maps, the weapon and vehicle placement blocks are empty, apart from a
handful on two maps. The palettes are mostly empty too. What is populated:
starting locations (48 to 204 per map), netgame flags (62 to 90) and
netgame equipment (21 to 82). So multiplayer weapons and vehicles are not
scenario placements: they are **netgame equipment** entries, spawned and
respawned by the game engine at run time. Forge's equipment edits are edits of these entries.

**The netgame equipment spawner `0x0015aad0` (E).** Called with an
equipment index and a per-entry state array (8 bytes per entry, the object
ID first). It:

1. reads the block address from `[0x004e0350]+0x124` and takes entry
   `index * 0x90`;
2. if the reference at `+0x58/+0x5c` is an `itmc` or `vehc`, rolls the
   collection with `collection_select` (`0x0019f1a0`);
3. switches on the byte at `+0x14`: values 1..7 go to `0x0015a4d0`, 8 and 9
   to `0x0015a5b0`, 0 to `0x0015a640`. These pick the actual definition,
   and are where the game variant's weapon settings come in (S, by their
   shape: read them before relying on it);
4. fills placement data (`0x000b7930`), copies the position from `+0x40`
   (three floats) and converts the orientation at `+0x4c` with `0x0011df60`;
5. deletes the entry's previous object if there is one (`0x0015b1e0`),
   creates the new one with `object_new` (`0x000b7b40`), stores its ID,
   writes the equipment index into object `+0xaf`, and if bit 0 of the
   entry's first byte is set, calls `0x000b9b90` on it.

Its only caller is the per-tick update `0x0015ae70`, which walks
`scnr+0x120` and decides when each entry is due. Helpers `0x0015ace0`,
`0x0015ad30`, `0x0015adb0` and `0x0015ac50` read the same block.

**Object creation (E).** `object_new` (`0x000b7b40`) takes a placement
record filled by `0x000b7930`, returns a salted object ID and rolls back on
failure ([engine-map.md](../engine/engine-map.md)). The object pool is 2,048 headers
and 1 MiB of bodies (`0x000b67c0`). 34 call sites call it directly.

**HaloScript (E).** The VM and builtin catalog are decoded
([haloscript.md](../formats/haloscript.md)), but builtin names are only known when a
scenario calls them, and HaloScript's object creation works on *named*
scenario placements. It is not a good way to place arbitrary objects; a
direct `object_new` call or a scenario block edit is.

## Sources

[hp-forge]: https://www.halopedia.org/Forge
[hp-sandbox]: https://www.halopedia.org/Sandbox
[gf-limit]: https://gamefaqs.gamespot.com/boards/926632-halo-3/49202034
[hms]: https://github.com/Sopitive/HaloMapStudio/releases/tag/v1.8.0
[gdc-reach]: https://www.youtube.com/watch?v=h47zZrqjgLc
[pc]: https://github.com/pnill/cartographer
[h2codez]: https://github.com/Project-Cartographer/H2Codez

- Halopedia, [Forge][hp-forge] and [Sandbox][hp-sandbox].
- [GameFAQs: object count limit][gf-limit] (anecdotal).
- [HaloMapStudio v1.8.0][hms] (Halo 3 map-variant fields; not read for
  layout).
- David Aldridge, [I Shot You First: Networking the Gameplay of Halo:
  Reach][gdc-reach], GDC 2011.
- [Project Cartographer][pc], [H2Codez][h2codez].

Not consulted: leaked builds, map-variant source code from any project,
and community decompilations of the retail game.
