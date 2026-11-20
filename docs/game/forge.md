# Forge for Halo 2 multiplayer maps

Status: Design. Nothing is built: no code, no hooks, no file format. This page says what Halo 3's Forge is, what retail Halo 2 already has
that a Forge could sit on, and a phased plan with a small first slice.

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
respawned by the game engine at run time. That is the natural hook for a
first Forge.

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
direct `object_new` call (or a scenario block patch) is.

## 2. The variant model

### 2.1 What a map variant stores (S)

```
map variant
  format version
  base map: cache file name + the scenario's tag datum + a SHA-256 of the
            scenario tag's bytes on the user's disc (never the bytes)
  title + author + description (player text, ours)
  edits, in order:
    equipment  set | add | remove   netgame equipment entry by index
    spawn      set | add | remove   starting location by index
    flag       set | add | remove   netgame flag by index
    scenery    add | remove         object by tag name + transform
  budget used (computed, checked again on load)
```

An object is named by its **tag path and group** (resolved through the
map's tag index on load), plus position, orientation and the few
per-entry fields the engine reads (respawn timing, team, type byte).
Positions are stored as full floats: unlike Halo 3 there is no reason to
quantise.

The file is canonical JSON (RFC 8785, which `live-proto::jcs` already
implements), so its SHA-256 is its identity. It contains player-authored
text and transforms only; tag paths are references to the user's own
disc, the same way [community-gametypes.md](community-gametypes.md) names
settings without copying tables.

### 2.2 Applying it

Two ways:

| | A. Patch scenario blocks at map load | B. Create objects at run time |
|---|---|---|
| How | after the cache loads, rewrite `scnr` blocks in guest memory; the engine spawns as if the map shipped that way | call `object_new` from a hook once the game starts |
| Respawn, game variant rules, objectives | free: the engine's own code (`0x0015ae70` etc.) | ours to rebuild |
| Network | every peer loads the same patched scenario (S: if peers run these paths) | host-only creation needs the engine's replication to carry it (unknown) |
| Needs | the block layouts, and guest space for blocks that grow | object placement ABI, a safe call point in the tick |
| Fits | weapons, vehicles, spawns, flags, scenery | live editing (the editor, §3) |

**Recommendation (S):** A for playing a variant, B only inside the editor.
A game on a variant is then a retail game on a retail-looking map.

When to patch: after `load_requested_scenario_cache` (`0x0018eaa0`) has
published the scenario pointer at `0x004e0350` and before the new-map
initialisation that first reads `scnr+0x120`/`+0x118`/`+0x100`. Wrapping
`0x0018eaa0` through a dispatch override is the obvious point (S: confirm
that nothing in it reads those blocks before it returns). The tag cache
is reloaded with each map, so a patch never outlives its game.

Growing a block: an edit that only moves, retargets or removes entries
fits in place (removal is a swap-remove and a smaller count). Adding
entries needs a new block somewhere in guest memory, then a rewritten
`count, address` descriptor. Where that space can come from (slack after
the tag data, or a range the runtime reserves) is the first open question
(S). Checkpoints and saved state are not a concern in multiplayer, but
check that the tag region is not in any snapshot.

### 2.3 Validation (S)

On load, before anything touches guest memory:

1. The base map and scenario hash match the user's disc, or the variant
   is refused with a message.
2. Every tag path resolves, with a group the block accepts (an equipment
   entry takes an object or an `itmc`/`vehc`; a scenery entry a `scen` or
   `bloc` already in the cache).
3. Floats are finite; positions lie inside the structure BSP's world
   bounds (`sbsp+0x34`, [structure-bsp.md](../formats/structure-bsp.md)).
4. Counts stay within budget: our own per-map limits, well under the
   2,048-object pool and inside each block's engine limit (unknown; to
   measure).
5. Every edit is applied to a copy first; if any fails, nothing is
   written.

### 2.4 Determinism and networked play

All peers must load identical variants. Halo 2 is widely described as
using a hosted model with an authoritative host, unlike Halo 1's
lockstep System Link (S: commonly repeated, but not checked against a
primary source here; [GDC 2011][gdc-reach] covers Reach's hosted model,
not Halo 2's). Which objects clients
create themselves from the scenario and which the host replicates is
**not known for this build** (S). Both cases need the same rule: a
mismatch is refused in the lobby, not discovered mid-game.

- The host's variant SHA-256 goes into the lobby. For our Live service it
  is a session attribute or a field in our own session record
  ([live-matchmaking.md](../live/live-matchmaking.md) §3). For System Link, the HLE
  socket layer owns both ends and can carry it beside discovery (S: a small
  out-of-band message; the game's own QoS listen data is game-owned and
  should not be borrowed).
- A joiner without the variant fetches it (Live) or is refused (System
  Link without Live). A joiner with a different disc hash is refused.
- The runtime's planned determinism harness (per-tick guest-RAM hash,
  [high-framerate.md](high-framerate.md) step 1) is how we prove two
  peers loaded the same thing.

## 3. The editor

### 3.1 Monitor mode (S)

Two candidates:

- **A flying camera owned by the host.** The editor pauses the
  simulation, takes over the camera (the runtime already pauses on Cmd+P
  with logical time), and lets the user fly freely. No guest biped exists.
  Simple and safe, but the world does not run while editing, so physics
  settling is not visible.
- **A guest monitor biped.** Spawn a flying, invisible, invulnerable unit
  for the editing player, as Halo 3 does. The multiplayer maps carry no
  monitor biped, and adding a tag is not possible without shipping data,
  so this would mean an existing biped with flags changed at run time.
  Expensive and fragile.

Recommendation: the host camera first.

### 3.2 UI

Follow [settings-menu.md](settings-menu.md) and [ui-screens.md](ui-screens.md):
native game screens built from the game's own list class, with no host
overlay. The palette is built from the map's own tag index, the way the `h2 object` resolver
works: weapons, vehicles, `itmc`/`vehc` collections, scenery and crates
present in the map's cache (and in `shared.map` for multiplayer). Labels
are tag base names, which come from the user's disc at run time.

Placement: ray-cast from the camera against the structure BSP
collision ([collision.md](../formats/collision.md)), drop to the surface, and snap
to the surface normal for spawns and flags. Rotation in 15° steps with a
free mode. The overlay shows a ghost of the object (render model through
the viewer's path) and does not create a guest object until the user
confirms.

Saving writes the variant file (§2.1) under the user's data directory.
Testing is "apply and restart the map": the variant takes the same path
as a real game.

### 3.3 Live editing (later, S)

Creating, moving and deleting real guest objects while the game runs
(option B) needs: a safe point in the tick to call `object_new` and the
delete path, the object's transform layout
([live-gamestate.md](../live/live-gamestate.md) §6), and the netgame equipment
state array, so the engine's respawn does not fight the editor. Multiple
editors over the network, as in Halo 3, would need the editor's
operations replicated by the host: out of scope until the rest works.

## 4. Sharing

- **Format.** The canonical JSON of §2.1. Its SHA-256 is its name. It
  holds tag paths, transforms and player text, nothing from the disc.
- **Live service.** The server already stores blobs by SHA-256 with a
  signed event carrying `{name, sha256, size}` for title files
  ([live.md](../live/live.md) §4.3). A variant share is the same thing with a
  user author: `POST` a variant signed with the user's key, list by map and
  author, fetch by hash. The game's per-user storage facility 5 is not
  needed: the overlay talks to the service directly (S).
- **Offline.** Variants are plain files; copying one between machines is
  enough for System Link.
- **Server-side checks.** Schema, size cap, and the same validation as §2.3
  minus the disc hash (the server has no disc). Player text is moderated
  like gamertags.

## 5. Risks, prerequisites, plan

### Prerequisites

1. A multiplayer custom game running on our runtime, local and System
   Link. [map-pack.md](map-pack.md) §6 lists multiplayer as a separate job;
   this page assumes it.
2. Typed, tested decoding of `scnr+0x100`, `+0x118`, `+0x120`, `+0x128` and
   the scenery/crate blocks in `crates/tags`, with engine-backed strides
   and field meanings (today the header decoder keeps them opaque).

### Risks

- **Network model unknown.** If clients do not run the equipment spawner,
  a host-only patch may still work through replication; if they do, every
  peer must patch. Either way, find out before shipping networked play.
- **Space for growth.** Adding entries needs guest memory the engine will
  not reuse.
- **Engine caps.** Per-block limits in the spawner's state arrays,
  netgame-flag consumers and the object pool are not measured.
- **Game-variant interaction.** The type byte at `+0x14` and the variant's
  weapon settings decide what actually spawns.
- **Title updates.** All addresses here are 1.0; a title-update build needs
  them mapped ([title-updates.md](../runtime/title-updates.md)).
- **Content rules.** Tag paths stay as references to the user's disc;
  nothing derived is committed or served.

### Phases (one developer, S)

| Phase | Work | Effort |
|---|---|---|
| 0 | Prerequisite 2: decode and test the MP scenario blocks; read `0x0015ae70`, `0x0015a4d0`/`0x0015a5b0`/`0x0015a640` | 3-5 days |
| 1 | **First slice**: variant file, validator, and an in-place patch of netgame equipment on one map | 1-1.5 weeks |
| 2 | Spawns and flags, adding entries (guest space), scenery/crates, budgets | 2-3 weeks |
| 3 | Lobby hash for System Link and Live; refuse mismatches | 1-2 weeks |
| 4 | Host overlay editor: camera, palette, ray-cast placement, save | 4-6 weeks |
| 5 | Share via the Live service | 1-2 weeks |
| 6 | Live editing in a running game; multi-editor sync (optional) | 4+ weeks |

Total to a usable single-editor Forge with sharing (phases 0-5): about
10-15 weeks. Phase 6 is open-ended. Multiplayer itself (prerequisite 1)
is not counted.

### The smallest slice worth building

**"Load a variant that moves, retargets or removes netgame equipment
entries on one map."** In place only: no new guest memory, no editor, no
network. Pieces:

1. `crates/tags`: typed netgame equipment entry (position `+0x40`,
   orientation `+0x4c`, reference `+0x58`, type byte `+0x14`, flags `+0`),
   tested on synthetic bytes.
2. A variant crate: the JSON model, canonical hashing, validation against a
   decoded scenario, and `apply(&mut ScenarioBlocks)`, all pure and
   unit-tested with synthetic scenarios.
3. One boot hook after `0x0018eaa0` that applies a variant named by an
   environment variable, writing through `Guest` accessors.

Acceptance is by tests: a synthetic scenario in, the expected block bytes
out, and refusal of a variant for the wrong map hash. Seeing the moved
rocket launcher in a game is for the user to check.

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
