# Forge design: features, UI, interfaces and rules

The Forge feature list, its UI, interfaces, rules and risks. Part of [forge-design.md](forge-design.md).

## Features

Size is the code a slice needs with its tests: **S** under 800 lines,
**M** under 2,500, **L** more. "Game runs" counts the things only a run
can settle.

| Feature | Feasibility | Size | Rests on | Game runs |
|---|---|---|---|---|
| Monitor mode, objects, save | high | L | live-mirror M8, the director fly camera ([theater.md](theater.md)) | few |
| Precision: movement and rotation snap, axes, Coordinates, duplicate | high | S | `forge::tool`; tool state only | none |
| Spawns, goals, respawning items and their properties | high for the fields that are decoded; medium for game types and place at start | L | the kit; the netgame equipment and flag blocks ([forge.md](forge.md) 1.2) | kit entries live in a round; the undecoded fields |
| Groups, prefabs, labels, the placed-object browser | high | M | `forge-doc`'s next format | group moves in one tick |
| Placed lights | medium | M | the kit's generators; tag field writes | dynamic light count; that a generated light lights the level |
| Teleporters, buttons, triggers | high for teleporters and triggers; medium for device groups | M | the kit; `ForgeExt` | device groups |
| Projectile emitters | high alone and synchronous; host-replicated by shared schedule in F15 | M | `SimExt`; a trace of projectile creation | replication |
| Invisible barriers | high for new ones; low for retail ones before collision writes | M, then L | `collision-build`; [forge-blockout.md](forge-blockout.md) | collision groups |
| Nav mesh | high for bots; medium for AI sectors | M | `bots::autonav`, `pathfind` | AI on generated sectors in multiplayer |
| Monitor bodies (Forge sessions only) | medium | L | transplant, tag headroom, player control of a flying biped | several |
| Firefight maps: drops, bases, cover, waves, a test in the session | high: the ground, the director and the runtime are Firefight's | L, in eight slices | `firefight-kit`, `firefight::Def`, the host runtime ([forge-firefight.md](forge-firefight.md)) | Firefight's own (firefight.md, "What a run settles") |
| Geometry in game | block-out high ([forge-blockout.md](forge-blockout.md): pieces sized on a grid, the natural set, the dev textures); face push and pull medium (F13); full vertex editing on a pad medium, after it (F16) | L, then L | modeling slices 1 to 4, 7, 8, 11 | a crate's fixed body; every write class |
| Textures in game | medium | M | modeling slices 5, 6 | cache behaviour |
| Character import | medium for a skin on a chassis; low for own animation | L | the `mode` writer, `assets::encode` | every imported kind |
| Sharing on Live | high | M | the content store, `MapGate` | one deploy check |
| Custom models on Live | high | M | the content store, Live Admin | one deploy check |
| Host-replicated mode | medium; mirrors high, puppets medium | L | F3's results, `agent_proto::live` | several |

Each feature's mechanism, options and open engine points are in
[forge-features.md](forge-features.md). The editing rows (precision,
spawns and goals, groups, lights) and what Forge does not take from the
later games are in [forge-editing.md](forge-editing.md).

## The UI

Forge behaves as Halo 3's and Reach's does, with the advanced tools of
Halo Infinite's that fit, and looks like Halo 2. Three pages hold it:

- [forge-editing.md](forge-editing.md): what a monitor can do. The
  controls, the tools, snapping and Coordinates, the object menu's
  groups, properties by kind, groups and prefabs, and the verdict on
  each Infinite feature.
- [forge-firefight-ui.md](forge-firefight-ui.md): how it is drawn. Every
  screen is a kit screen the game's own widgets draw, every HUD piece a
  HUD kit element, and each copies a retail screen or HUD element.
- [notes/forge-prior-art.md](notes/forge-prior-art.md): what the three
  retail Forges do, with sources.

**Entering Forge.** Forge is a mode of the party lobby
([party-lobby.md](party-lobby.md)), beside Custom Games,
in Split Screen, in a hosted System Link game and in an Xbox Live
custom game, which the Xbox Live menu's Forge row opens in it
(forge-firefight-ui.md, "The lobbies"). In Forge Game Setup has
**Change Map...** (the installed multiplayer maps), **Map Variant...** (Original, or a
map variant of the lobby's map, `forge::files::variants::list`) and the game
type, which picks the goals the session edits. Start Game is the
game's; the draft is loaded as the session opens
(`forge_glue::request_draft`), and a variant that fails the store's checks is refused with the
editor's notice. The main menu has no Forge row: its six rows
are the retail five and Theater.

**In a session** there are three layers.

1. *The HUD*: the reticle, one prompt for what A does, the budget as a
   pill with a count, the tool's name, notices, and the motion tracker.
2. *Screens* over the world: the object menu, Properties, Tools, Tool
   settings, Coordinates, the Link question, and the Forge menu on
   Back, with **Save**, **Save as**, **Load**, **Rebuild map** and the
   rest on its tabs.
3. *The scene*: the held object itself, navpoints on it, on other
   monitors and on a link's ends, the axis gizmo, and the volume, nav
   and barrier overlays.

A Forge lobby with the default Slayer variant starts with an unscored
game record. The lobby keeps its selected variant when the player switches
back to Custom Games. Explicit rules scripts remain available for testing
in the editor.

The buttons are Halo 3's: D-pad Up swaps monitor and player, A picks up
and drops, X opens the object menu or the Properties of the thing aimed
at, hold Y deletes, B cancels, RT held turns and pushes a held object,
RB and LB rise and fall, LT thrusts. The whole table, with the keyboard,
is in forge-editing.md, "Controls".

Forge is a mode of the custom game lobby, picked in the lobby's GAME
MODE picker ([party-lobby.md](party-lobby.md);
[forge-firefight-ui.md](forge-firefight-ui.md), "The lobbies").
The screens are the game's (`forge_screens`, slice U4) and the monitor
HUD is the HUD kit's `forge` HUD (`forge_hud`, slices U3 and U6); the
play window draws no Forge page (forge-firefight-ui.md, "Audit").

Rules: every pending Rebuild edit shows the same count in the HUD and
on the Forge menu; a refused proposal shows its reason as a HUD notice
for a few seconds; nothing in a screen needs the keyboard except names
and typed numbers; a choice opens a picker; copy is sentence case in
rows and capitals in titles, as retail.

## Interfaces

**Agent API** (game). New namespace `forge`, in
[agent-api.md](../runtime/agent-api.md) when built. It is added because
every Forge test, lane and later tool drives a session through it.

| Method | Access | Does |
|---|---|---|
| `forge.status` | read | session, role, log head, pending Rebuild count, budget |
| `forge.doc` | read | the document as of the log head |
| `forge.input` | act | one monitor action or one tick of camera input through the local controls |
| `forge.propose {op, was?}` | act | one proposal; the entry or the refusal |
| `forge.log {since?}` | read | entries after a position |
| `forge.rebuild` | act | the host's rebuild; `{map_id, seconds}` |
| `forge.capacity` | read | tag headroom, object pool use, each block's count and limit, kit pools left |
| `forge.nav {generate?}` | act | build state, counts, problems, overlay ([forge-nav.md](forge-nav.md)) |
| `forge.ground`, `forge.ground.build` | read, act | the AI ground's state, markers, problems and receipt; build it ([forge-map-variants.md](forge-map-variants.md), "AI ground") |
| `forge.waves`, `forge.waves.test` | read and act | the wave set; a wave test in the session (design, [forge-firefight.md](forge-firefight.md), "Interfaces") |
| `forge.probe {what}` | act | one of the F3 measurements |

**Editor API.** `mirror.edit` in a Forge session routes the editor's
actions to `forge.propose`. `import.*` (list, convert, check, remove)
for bundles, a provider in the studio's `api`.

**CLI.** `h2 forge build <doc> [--disc <iso>] [--maps <dir>]`
materialises and writes the package. `h2 forge check <doc> [--disc <iso>]`
runs the same checks in memory and prints each entry print it compared
([forge-map-variants.md](forge-map-variants.md), "Naming an entry"). `h2 forge looks [--disc <iso>] [--json]`
lists the models a monitor can look like
([forge-monitor-appearance.md](forge-monitor-appearance.md)).
`h2 import-asset <file> --chassis <key>`
is planned.

**Live.** `ForgePublished` and its routes, in [live-api.md](../live/live-api.md)
when built; `NewSession.map_id` is unchanged.

**Engine entries** Forge needs beyond those the agent API has, each
proven by a differential test before use: the device group setter, the
unit swap for a body, the damage entry for a kill sink. Squads go
through Firefight's entry (`actor_net::squads::SquadEntry`), which
Forge shares and does not add to.

## Rules and limits

- A document holds no bytes from any game and no imported asset. The
  validator has no type that could carry them. An imported model is a
  separate `model-bundle` blob under its own check.
- Recipes name disc tags by key and source hash. A machine whose disc
  lacks a source is refused by name before the map loads.
- An op is validated before any guest write: finite floats, positions
  inside the BSP's world bounds, a tag that exists with a group the
  block accepts, counts inside engine limits and the budget. A proposal
  that fails changes nothing.
- No Forge session on a hopper or ranked session. Outside a Forge
  session the only edits are those a rules script declares and makes or
  grants in a match, through the same validated, host-stamped proposals
  ([rules-engine.md](rules-engine.md#roles-powers-and-capabilities)).
  No edit in a ranked match; no edit in Theater.
- The Forge step has a time budget a tick. Emitters and the graph skip
  work past it and say so.
- Forge files, bundles and materialised packages are user output and
  stay out of Git checkouts.
- A Rebuild is refused while any player holds an object.
- A monitor body exists only in a Forge session. A published map played
  as a match gives every player their own biped; the recipe for a body
  is left out of a map materialised for play.
- No bundle is uploaded from an account without `model_uploads`, in
  either home.

## Risks

| Risk | Effect | Answer |
|---|---|---|
| Tag headroom is small on multiplayer maps | monitor bodies and AI may not fit; one map has none | F3 measures it; the cut order; a body recipe names its cost up front; the AI ground takes the kit's fit steps and cuts classes from the end ([firefight.md](firefight.md), "The kit") |
| Host-made objects may not replicate, by group | more of F15 falls to Forge's mirrors | F3 measures; synchronous is built first and needs none of it |
| The synchronous protocol may not hold a multiplayer map | F5 cannot ship; F15 moves ahead of it. Firefight maps are not held up: their actors are host actors in the distributed model | F3, before F5 is built |
| `SimExt` has no user | no emitters, triggers or logic | the blob is built and tested on synthetic memory (`crates/rewrites/sim-world/src/ext`; sim-rewrite.md, "The `SimExt` blob"); F7 gives `FORGE` its first record and proves it on a running game |
| The modeling crates are design | geometry and paint cannot start | block-out needs only `collision-build`; the rest follows modeling's own slices |
| A crate with a fixed body may not stop a player | block-out pieces are walked through, as scenery is | BO4's first game run places one block; a piece becomes a machine if it fails ([forge-blockout.md](forge-blockout.md), "Not verified") |
| A game that loads a map with added tags is unproven on retail bases | every recipe | map-builder.md, "Not verified"; F10's first run is one transplanted scenery tag |
| Materialise time grows with a document | long holds on Rebuild | cache by op-prefix hash; measure in F2 |
| A shared document is hostile input | crashes, resource exhaustion | the validator's caps; fuzzing in F1; the Forge step's budget |
| Users import assets they have no right to | legal exposure for Live | uploads only from admin-approved accounts, with a statement; reports, the Models queue, takedown by hash, revocation; no extractor ships |
| Mirrors and puppets drift or cost bandwidth | rubber-banding, a low actor cap | corrections only on movement; a per-map cap on mirrored objects and actors, measured in F15 |
| Vertex editing on a pad is slow to use | F16 disappoints | the face tier ships first and covers most shaping; F16 is designed around loops, snapping and a radial menu, and the paired editor stays available |
