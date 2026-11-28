# Agent API: animation, collision, traces, checks, test maps and the tag store

Status: Built. Part of the [agent API](agent-api.md), which holds the wire, the method index and the rules.

## `anim`: the animation graph walk

The animation graph's node walk `1dd1c0` is answered by the Rust body
([skeleton-walk.md](../engine/skeleton-walk.md#the-graph-walk-1dd1c0)).
The game method reads process counters at a tick boundary. Code:
`crates/extensions/agent/src/anim.rs`, wire types in
`crates/extensions/agent-proto/src/anim.rs`.

| Method | Access | Params and result |
|---|---|---|
| `anim.graph_walk` | read | No parameters. Returns `addr` (`001dd1c0`) and `body`. |
| `anim.manager` | read | No parameters. Returns `entries`, keyed by the eight-digit guest address of each observed manager or graph function. |

`body` counts calls the Rust body answered without the guest (`native`, `nodes`
posed); a declined plan panics in the hook with its reason. Sample before and after a scenario and
subtract to isolate that run.

```sh
h2 agent anim.graph_walk
```

`anim.manager` counts segments starting at a function entry or resuming
after a call. For each observed function, `matched` and `differed` count
completed comparisons. `interrupted` counts predictions for which the guest exited
before a comparison boundary. `answered` counts segments the Rust body
answered without the guest. The first difference for each function is
retained in `first`, with the starting selector (`pc`), differing field
(`what`), and both values (`plan` and `guest`). An entry with `first` set
to `null` has no recorded difference. Compare counts before and after
the scenario to establish coverage. See [animation-manager.md](../engine/animation-manager.md).

```sh
h2 agent anim.manager
```

## `sim.arms`: the native-sim gate read

`sim.arms` reads every native-sim shadow and authority arm in one call:
the instrument that decides whether a sim phase may be on by default
([sim-rewrite.md](../engine/sim-rewrite.md#3-determinism-and-exactness)).
It is a game method answered at a tick boundary from process counters, so
it needs no guest memory. Code: `crates/extensions/agent/src/sim_arms.rs`,
wire types in `crates/extensions/agent-proto/src/sim_arms.rs`.

| Method | Access | Params and result |
|---|---|---|
| `sim.arms` | read | No parameters. Returns `subsystems`: `physics`, `objects`, `anim`, `collision`, `seats`, `effects`, `sound`, `effects-parts`, in that order. |

Each subsystem has `name`, `switch` (`impl.sim.native.<name>`, or `""`
for `objects`, `anim`, `effects` and `effects-parts`, which have none),
`on` (the switch in this process; always true with no switch), `arms` and `clean`: `checked > 0 && mismatched
== 0`, summed over the arms. Each arm has `name`, `checked`, `matched`,
`mismatched`, `answered` (calls Rust answered in the guest's place) and
`declined` (calls left to the guest unchecked, summed over reasons). The
sources are `sim_physics::parity::status`, `sim_objects::functions::parity::status`
(`objects.curve`), `sim_objects::functions::arm::status` (`objects.function`,
whose `declined` adds its `dropped` audits), `sim_objects::native_walk::stats`
(`objects.walk.<entry>`, one arm per walk-table body: `answered` its
segments, `declined` its refused calls, nothing `checked`),
the graph walk's `stats`, `sim_anim::manager::arm::stats`,
`sim_collision::WORLD.stats`, the shadow registry's `sim.seats` and
`sim.effects` groups, `sim_seats::aim::arm::status` (`seats.aim`, the
aim-assist cone's kernel arm) and `sim_seats::respawn::status`
(`seats.respawn`), whose unported paths panic, `sim_seats::leaf::status`
(`seats.leaf.<name>`, one arm per leaf callee, whose `declined` adds its
unwatched audits), and `sim_sound::voice_arm::stats` (`sound.voices`). A source without a counter reports 0 for that
field: physics and collision have no `checked` (it is `matched` plus
`mismatched`) and no `answered`; `anim.manager`'s `differed` is
`mismatched`; the `anim` arms have no `declined` (a decline panics);
shadow ports have no `answered` and their
`skipped` is `declined`; `sound.voices` counts ticks for `checked`
(sampled ticks with voices), `matched` and `mismatched`, and voices for
`answered`. `effects-parts` is the effects phase in Rust, always on
with no switch (its `switch` is empty): its
`effects.damage`, `effects.object`, `effects.sound`,
`effects.character`, `effects.coln`, `effects.decal` and `effects.light`
arms are `sim_effects_parts::status`'s part arms, and `effects.parts` counts
the parts returned from Rust as `answered` and the parts the guest's arm
ran unchecked as `declined`, then `effects.body.<entry>`, one arm per
per-effect body (`effect_update::arm::stats`, whose `declined` adds its
interrupted guest segments)
([effects-parts.md](../engine/effects-parts.md)). A subsystem with its
switch off reads zeros, so `clean` is false.

```sh
h2 agent sim.arms
```

Run the scenario, read `sim.arms`, and flip a phase on only when its
subsystem is `clean` with `checked` large enough for the scenario.

## `collision`: the native collision world

The collision switch counts admitted native answers, declined queries,
floating-point environments and comparisons with lifted returns. The
game exposes these counters at a tick boundary. See the
[entry addresses and admission rules](../engine/collision-queries.md).

| Method | Access | Params and result |
|---|---|---|
| `collision.stats` | read | No parameters. Returns `switch`, `entries`, `decodes`, `decode_failures`, `flushes`, `bsps`, `mopps`, `bytes`, `array_bytes`, `budget_bytes`, `ring_fallbacks`, `cluster_budget_reads`, `precision` (aggregate single, reserved, double and extended x87 calls at C and D), `vector` (C’s result parity), `context` (D’s outer walk parity), `instance`, `flood`, `visit`, `query` (E, G, H and J’s return comparisons), and `ray` (I’s walk and callbacks). |
| `collision.trace` | read | `{start, delta, flags}` where `start` and `delta` are three finite binary32 world coordinates and `flags` is an unsigned 32-bit whole-query mask. |
| `collision.sources` | read | No parameters. Checks the source fingerprints of each resident native BSP and MOPP against guest RAM. |
| `collision.sources.clear` | claim | No parameters. Returns the same check and drops all resident entries from the collision world at that tick boundary. |

`collision.trace` evaluates entry J at a tick boundary on the active
structure. The segment ends at `start + delta`. Both ignore handles are
empty. The reply gives the tick stamp, numeric `hit_type` (zero for a
miss), `fraction`, world `position`, optional `material` and `plane`, and
the first and last `locations` as `{leaf, cluster, bsp}`. The arrays
`cluster_marks`, `instance_marks` and `object_marks` give the slots J
would mark in traversal order. The query leaves guest memory untouched.
A path outside the native kernel returns `UNAVAILABLE` with the bail
name in `error.data.reason`.

```sh
h2 agent collision.trace '{"start":[0,0,1],"delta":[0,0,-2],"flags":1}'
```

Each row in `entries` gives the `name` and eight-digit guest `addr`, the
count of `native` answers, and `bails` by reason. Two frequency maps count
the control words at entry, including calls that fall back at the first
environment check. Keys are lowercase hexadecimal strings with leading
zeros: eight digits for `mxcsr` and four for `x87_control`. Each value is
a call count. Each map counts its own register independently. `cluster_flood` includes recursive entries in
its environment census. The `mopp_ray` and `query` rows count the
environment while their comparison records report the lifted outcomes.
An image without an observed entry leaves its row empty. The `object_sphere` row counts native answers at the
whole query's object-reference pretest. `array_bytes` counts the decoded
BSP allocations, the packed cluster table, the portal graph and the decoded MOPP trees (`mopps`
of them; `decodes` and `decode_failures` include MOPP programs, a failure
being one left to the guest). `bytes` adds the reserved space for BSP owners and address-index
capacity. `budget_bytes` limits that total to 8 MiB per loaded map.
`ring_fallbacks` counts transitions to scalar ring reads, and
`cluster_budget_reads` counts lookups that need the live cluster word
because the packed table would exceed the budget. Sample before and after
a scenario and subtract the counters to isolate that run.

`vector` counts entry C’s `matched` and `mismatched` returns and
`unverified` queries by bail reason. `first_mismatch` identifies AL or the
first differing byte offset in the result record, with expected and guest values.
`context` uses the same counters for entry D. It compares AL, R and the
40-byte context, counting each outer walk once across its recursive calls.

`vector` and `context` also carry the crossing-exit gate
([collision-queries.md, "Crossing exits"](../engine/collision-queries.md#crossing-exits)):

| Field | Meaning |
|---|---|
| `exits_matched` | matches that also covered every register and frame byte of a crossing exit |
| `answered` | crossing exits answered natively once the gate opened |
| `sampled` | crossing calls left to the guest for a comparison while answering |
| `fell_back` | a difference was seen; the lifted body answers from then on |

`instance`, `flood`, `visit` and `query` use the same comparison counters.
An admitted native exit counts under `entries[].native`. A comparison
counts only after the guest returns. A declined prediction is
`unverified`. For J the comparison covers AL, flags, the result including
padding, and the active mark tables and generations.

`ray` counts the MOPP ray queries the native walk followed while the
guest answered them: `matched` (every interpreter call, and every key
where the instanced collection shows it, equal), `mismatched` with
`first_mismatch` describing the first since start (the observation index
and both sides), `unverified` by bail reason (the walk could not follow
the query exactly, or its environment is one it does not model), and
`fraction_moved` (no key was visible and a terminal lowered the fraction).
Read `matched`, `mismatched` and `unverified` together to distinguish
a covered query from one the walker could not follow.

```sh
h2 agent collision.stats
```

The bail audit (under 0.1% of calls bailing) reads `bails` against
`native`. For `mopp_box`, `grow` counts answers that did not fit the
key array's room. A reason that appears on one architecture only is a determinism
bug.

### Checking a cold collision world

At a tick boundary, `collision.sources` returns `sources` sorted by
`kind` and guest `address`, with `cached` and `live` 64-bit hexadecimal
fingerprints. A BSP fingerprint covers the live header, all eight blocks,
the projection table and the zero used by the matcher. A MOPP fingerprint
covers origin, scale, program length and bytecode. A failed live read
returns `live: null` and the `bail` reason. The reply counts `mismatched`
and `unreadable` separately. An empty list means no BSP or MOPP is
resident. A debugger memory trap makes the method unavailable.

For the cache-cold parity arm, `collision.sources.clear` gives the
pre-clear readings and drops the BSPs, MOPPs, leaf table and portal graph.
The next queries repopulate the world from current guest sources. The
read-only form leaves the warm world's entries in place.

Start each arm from the same match state and input with
`impl.sim.native.collision` and `impl.sim.arena-hash` enabled.
Use one claimed connection per arm. This example captures 1,800 cold ticks.
Replace `collision.sources.clear` with `collision.sources` for the warm
run and save its JSON lines separately.

```sh
{
  echo claim
  echo pause
  echo collision.sources.clear
  for tick in $(seq 1 1800); do
    echo 'step 1'
    echo collision.sources.clear
  done
  echo sim.hashes
  echo release
} | h2 agent -
```

Compare tick stamps and the sampled gameplay and arena rows from
`sim.hashes`. At each tick, match the cold sources to warm sources of the
same kind and address, and compare fingerprints. A warm world may retain
entries from earlier ticks. Inspect unreadable sources, missing counterparts
and cached/live differences before treating a run as clean.

## Tick traces and the flight recorder

Polling `world.object` from outside (`h2 agent -`) drops ticks when the
game or the client is busy. `trace.*` and `flight.*` take the rows inside
the game, in the tick observer, before queued requests are served, so each
tick gives exactly one row. Code: `crates/extensions/agent/src/trace.rs` (tests in
`trace/tests.rs`), wire types and `show` in `crates/extensions/agent-proto/src/trace.rs`,
CLI in `crates/apps/cli/src/agent_flight.rs`. File readback lives in
`crates/extensions/agent/src/outputs.rs`.

The game puts `traces/`, `flights/` and `inputs/` under the
[agent output root](switches.md#tools), which defaults to its cache
directory. For a [lab job](lab.md#runs), that root is `runs/<id>` and
`h2 lab fetch` includes the files.

```sh
h2 agent trace start player.0 physics,pos          # rows to traces/trace-<ms>.jsonl
h2 agent trace stop                                # rows, bytes, first/last tick
h2 agent flight start player.0 physics,pos,velocity 20
h2 agent flight dump crouch-bug.jsonl              # flights/crouch-bug.jsonl
h2 agent trace.list                                # saved trace and flight file names
h2 agent flight show crouch-bug.jsonl --remote --field physics.crouch
h2 agent flight show ~/Library/Caches/h2recomp/flights/crouch-bug.jsonl \
    --from 4100 --to 4160 --field physics.crouch --field physics.airborne --field input
```

- **Rows.** One JSON line per tick: `{tick, ticks, map, objects: {id:
  Object}, gone, error}`. `tick` is the stamp's game tick, `ticks` the
  agent's count (never resets, one more each row). `objects` holds each
  asked-for id in the world, trimmed to `fields` (`world.query`'s names;
  `id` and `kind` always kept; all when absent). Ids not in the world that
  tick are in `gone`; a failed world read gives `error` and still a row.
  The world is read once a tick when either recorder runs, and not at all
  otherwise. A paused game writes no rows; each `session.step` tick does.
- **`trace.start {objects, fields?, path?}`.** 1 to 64 ids. Streams rows to
  a bare file name in `traces/` (default `trace-<ms>.jsonl`).
  An existing file is replaced. The writer flushes every 30 rows, at
  `trace.stop` and at exit. Memory use stays constant with file length.
  `trace.stop` and `trace.status` give `rows`, `bytes`, `first_tick`,
  `last_tick`, `path` and `error` (a write failure ends the trace there).
  One trace at a time; the last one's counts stay for `trace.status`.
- **`flight.start {objects, fields?, seconds?}`.** Keeps the last `seconds`
  (default 30, 1 to 600) x 30 rows in memory, at most 64 MiB of JSON (the
  oldest go first). Each row also has `input`: every `XInputGetState`
  result the game got since the last tick, taken at the agent's input tap
  after agent steps merge in (`{port, handle, status, packet, buttons,
  left, right, raw}`, `raw` the 18 pad bytes in hex), at most 64 a tick
  (`input_dropped` counts the rest, as during a load). Without XAPI input
  there is no tap and rows have no `input`. `flight.dump {path?}` writes
  the ring to a bare file name in `flights/` (default `flight-<ms>.jsonl`)
  and keeps recording. `flight.stop` drops the ring. `flight.status` gives
  `capacity`, `rows`, `bytes`, `ticks` seen and the ring's tick range.
- **`trace.list`** returns `{traces, flights}`, with sorted file names
  from each directory.
- **`trace.read {path, kind?, from_tick?, to_tick?, offset?, limit?}`**
  returns `{rows, next}`. Pass a bare file name. Set `kind` to `trace` or
  `flight` to select a directory, or search `traces/` first and
  `flights/` second by leaving it out. The tick bounds are inclusive.
  Each row is a complete JSON line without its newline. The limit is in
  rows (default 64, maximum 256), and file scanning is bounded per call.
  Start at offset 0, then pass the returned byte cursor as `offset` until
  `next` is `null`. Follow the cursor even after a short or empty page.
  Read a finished trace or flight dump to keep the file stable across pages.
- **Scrub.** `h2 agent flight show <file>` opens a local file when it exists.
  A missing file or `--remote` reads through the game socket with
  `trace.read`. Use `--kind trace|flight` to choose a socket directory.
  The command prints whole rows or selected fields such as
  `--field physics.crouch` and controller reads with `--field input`.
  Use `--from` and `--to` to select game ticks. To re-simulate a stretch,
  start from a [snapshot](agent-api-input.md#state-machine-snapshots).
- **Access.** These methods require no claim and are answered between
  slices as well as at tick boundaries.

## Shadow checks

Design: [shadow-exec.md](../engine/shadow-exec.md). Host methods
(`agent::shadow`, registered by boot's agent glue) over
`feature::shadow`'s registry. A native sim port is checked against the
lifted original, each on its own copy of guest memory from the state the
game reached, then runs on the live Guest as usual. Only installed ports
are checked, and `shadow.enable` is the only switch for the checks.

Select ports by port label (`sim.port.unit_pose`), object point name
(`OBJECT_FUNCTION_VALUE`), group (`sim.native`, `sim.physics`,
`sim.native.objects`) or `all`. The object group samples individual entries
and resumes, including those reached inside a fused walk.

| Method | Access | What |
|---|---|---|
| `shadow.enable` | act | `{ports, every?, step_limit?}`: checks on for those ports, their counts and mismatches reset. `every` checks one call in that many (default 1); `step_limit` caps the original's instructions per check (default 5,000,000). The first call builds the lifted original from the image's cached analysis (a few seconds on the game thread). → `{enabled, not_installed, every, step_limit}`. Unknown names: invalid params; no image in the process: unavailable |
| `shadow.disable` | act | `{ports?}`: checks off (no ports: all). → `{disabled}`. Counts and mismatches stay |
| `shadow.status` | read | `{on, ready, every, step_limit, calls, checked, matched, mismatched, skipped, ports, mismatches, mismatches_dropped}`. Each port: `name`, `group`, `point`, `function`, `installed`, `on`, `calls`, `checked`, `matched`, `mismatched`, `skipped`, `skip_reasons`. Each mismatch (at most 64 kept): `port`, `group`, `point`, `function`, `pc` (entry or resume), `tick`, `call` (which call since `enable`), `kind` (`transfer`, `registers`, `memory`, `returned`, `nondeterministic`), `summary`, `transfer` (`port`, `original`), `registers` (`name`, `port`, `original`), `memory` (runs of bytes: `address` folded physical, `virtual_addresses`, `before`, `port`, `original`), `memory_bytes`, `steps`, `entry` (every register at entry, hex) |

```sh
h2 serve ...
h2 agent shadow.enable '{"ports":["sim.port.unit_pose"]}'
h2 agent session.wait '{"until":{"ticks":300}}'
h2 agent shadow.status
```

`scripts/shadow-gate.sh` is the scripted disc gate over these methods: a
headless sim match with the ports installed, checks on for N ticks, and a
failure on any mismatch ([shadow-exec.md](../engine/shadow-exec.md#the-disc-gate)).

## Scripted scenarios with a verdict

```sh
cat > grenade.txt <<'X'
wait {"phase":"gameplay","map_ticks":30}
expect /at/phase == "gameplay"
claim
press rt 100
capture 90
expect /over_budget <= 2
expect /causes missing
release
X
h2 agent run grenade.txt          # one JSON verdict; exit 1 unless it passed
```

`h2 agent run <script|->` (`crates/apps/cli/src/agent_run.rs`) is the `h2
agent -` script on one connection, plus `expect <pointer> <op> [JSON]` on
the last answer (a JSON pointer; ops `==`, `!=`, `<`, `<=`, `>`, `>=` on
numbers, `contains` for a substring, an array element or an object key,
`exists`, `missing`, and `near {"to", "within"}`) and `keep` (put the last
answer in the verdict). The expectations are `agent_proto::expect`, which
`sim.expect` judges in the game too. The verdict is `{"pass", "failed",
"ms", "calls": [{line, call, ms, ok, error}], "expects": [{line, expect,
want, pass, got}], "kept", "stopped"}`. A failed expect is recorded and
the script goes on; a failed call stops it. Answers are not printed
except kept ones, so a long scenario costs a few lines of context.

## Test maps

Purpose-built test maps from recipes: a spawn grid, a weapon range, the
superbounce patch and more, built from the user's own disc while the game
runs and queued as the next game. Design, recipe format and the built-in
rigs: [test-maps.md](../game/test-maps.md). Host methods registered by
`crates/extensions/game/src/test_maps.rs` (the `test-maps` cargo feature of
`boot`, which supplies the recipes in `crates/apps/boot/src/test_maps_recipes.rs`); wire types `agent_proto::test_maps`.

| Method | Access | Params and result |
|---|---|---|
| `maps.load` | act | `{map_id}` → `{name, map {id, path}, check, queued, expect}`. Queues a local Slayer game on the installed multiplayer package with this full hash. The VFS verifies the list slot's package identity. When the id is not found, the offers are installed again and the VFS's map packages reloaded before the id is refused, so a package written while the game runs is found ([the tag workspace](../editor/tag-workspace.md)). Wait for gameplay on the basename of the returned path before driving the camera. |
| `maps.test.list` | read | `{name?}` → `{recipes, maps}`: the built-in recipes (`name`, `title`, `base`, `fidelity`, `purpose`, `note`) and each test map (`name`, `base`, `state` `building`/`ready`/`failed`, `dir`, `error`, `map_id`, `listed {id, path}` once it has a multiplayer slot, `built`: the build report with `anchor`, `placed`, `counts`, `notes`, `expect`) |
| `maps.test.create` | act | `{recipe: <built-in name> \| <recipe object>, base?, replace?}` → `{name, state: "building", dir}` at once; the build runs on its own thread, then the offers are installed again. `base` builds on another disc map (`<recipe>-<base>`) |
| `maps.test.load` | act | `{name}` → after the next tick, `{name, map {id, path}, check, queued, expect}`: a local Slayer game on the map is queued through the `agent-calls` feature (`Call::QueueGame`, the level boot's steps). Wait for it with `session.wait` or `h2 agent maps load` |
| `maps.test.remove` | act | `{name}` → `{name, removed}` |

`map.reload {}` queues the active campaign or multiplayer map again and
returns `{queued, map, activation}` with the map name, the activation
reserved for this request and the executor's tick stamp. It uses the
[object edit session rules and hooks](agent-api-sim.md#object-edits). Film and input
recording or playback refuse both forms of reload.
`Sim::reload_map` in `crates/extensions/agent/src/sim/edits.rs` copies the active options block into
`Call::QueueGame`'s scratch. This preserves the scenario path, players,
variant and difficulty. `OPTIONS_QUEUE` takes the scratch address in EAX
and copies the block to the pending transition. See
[level-boot.md](level-boot.md#addresses) for the active flag, block and
entry. The answer confirms queueing. Pass its `activation` and `map` to
`session.wait` together with `phase: "gameplay"` and `map_ticks: 1`.
For a reply naming activation 4 on `lockout`, the wait is
`{"until":{"activation":4,"map":"lockout","phase":"gameplay","map_ticks":1}}`.
Use the returned value even when the file checksum stays the same.
A checkpoint revert cannot publish a queued reload's activation.

`map.reload {"file":"/absolute/path/<map>.map"}` loads a play copy for the
active DVD map. Both the filename and the map name in its Xbox cache header
must match the active name, ignoring ASCII case. A worker prepares a
private generation and checks its cache type, slot capacity, tag index
and retail payload checksum. A disconnect or timeout releases the preparation
and the worker checks for cancellation between chunks. At the tick boundary
the agent rechecks the session and active source, validates the pending
options storage, reserves the DVD file and retires the old cache keys
before calling the guest's options queue.
The VFS publishes the copy when the guest accepts the transition. If the
guest call is interrupted, the host checks the pending options before the
loader uses them. Matching options publish the copy. Otherwise the host
restores the retired keys. The result adds the requested `file` and the
header's numeric `checksum`.
After the activation wait, check that `live.status.map.checksum` or
`edit.map.checksum` matches the returned checksum. Both reload forms accept `confirm_peers`
on a System Link host. This total includes the host. Each peer needs its
own matching file. A writable game with the agent enabled can accept a play
copy even if it started without an override directory.

Forge's **Rebuild map** and the reload after **Build AI ground** run the
same reload from the game thread (`agent::sim::MapReloader`, which calls
`Sim::start_reload` with the checks `map.reload {file}` makes), so the
rules and refusals above apply to them as well
([forge-design.md](../game/forge-design.md), "Rebuild map").

A light host, a server-map mount, a scenario loaded from a content path,
or a host without the editor map mount returns `REFUSED` with `data.why`.
A film named with `--film`, film playback and external Live sign-in also
exclude file binding. A session's
[default recording](../game/film-format.md#recording-every-game) allows
it: the first authorized editor write ends the film of the game in play at the tick before it and keeps it to there; films of games already ended stay. After a file binding succeeds, restart
the game before signing in to external Live. Machine snapshots and
snapshot-based rewind are unavailable while a play copy is bound.
See [Map overrides](storage.md#map-overrides) and [Changed maps](cache-slots.md#changed-maps).

```sh
h2 agent maps                          # recipes and test maps
h2 agent maps create superbounce       # waits for the build
h2 agent maps load superbounce         # waits until the new map is in gameplay
h2 agent sim expect '/player/0/pos near {"to":[-10.5,3.9,-3.15],"within":1.5}'
```

`h2 agent maps` (`crates/apps/cli/src/agent_maps.rs`) reads a recipe file
(`.toml` or `.json`) on the client and sends it as the recipe object; its
verbs work in `h2 agent -` and `h2 agent run` scripts.

### Test maps on disk

`h2 testmap <source.map> <script.json | -> --out <new.map> [--force]`
(`crates/apps/h2-tools/src/testmap_cmd.rs`) is a host-only tool for a repro that
needs one thing in an empty room. It runs a JSON edit script
(`scenario_edit::script`, `crates/tools/scenario-edit/src/script.rs`) over the
source's placement blocks through the editor's own `Session`, so every
edit is checked as the editor checks it, and saves a new map (the source
is never written). It prints what each operation did and the entries left
per block. Operations: `clear` (blocks, or `"all"`; a block the editor
will not empty is reported and left), `keep` (these entries of a block,
renumbered from 0), `add` (a copy of `like`, or a zeroed entry with the
palette's first item, then `set`), `set` (fields by the editor's names:
`position` `[x, y, z]`, `rotation`, `facing`, `team`, palette `type`, ...),
`remove`. `allow_growth` lets a block grow past its source size, as the
editor's switch. Blocks: `scenery`, `vehicles`, `equipment`, `weapons`,
`machines`, `sound_scenery`, `crates`, `starting_locations`,
`netgame_flags`, `netgame_equipment`, `starting_equipment`. AI (squads and
their spawn points) and tag references other than through a palette index
are not covered. The user or coordinator loads the result (`H2_MAP_DIR`);
agents do not.

## Content packages

The game's content packages: ours baked into `h2` from `content/<id>/`,
then the user's under `<data>/packages/<id>/`, ordered and switched by
`<data>/packages/set.toml` and the game's features, and what each map the
game read got from them ([studio-redesign.md](../editor/studio-redesign.md),
"How the game consumes packages"). Registered by
`crates/apps/boot/src/packages.rs`; the set and the rows are
`workspace::GameSet`, the maps `workspace::PackageRewrite`'s record.

| Method | Access | Params and result |
|---|---|---|
| `packages.list` | read | `{}` → `{packages, problems, content_hash, local_only, maps}` |

- `packages`: every package read, those on in load order, then those off
  by id. Each row has `id`, `name`, `package_id` (the `PackageId`),
  `origin` (`baked` or `local`), `dir` for a local one, `on`, `why` when
  off (switched off in `set.toml`, a feature that is off, a `requires`
  that is missing or off, a set that cannot be ordered), `always_on`
  (`core` and `ui`), `signed` (baked packages are; a local one is not and
  loads in local games only), `features`, `items` (a count by kind) and
  `scopes` (`sim`, `client`, `live`, `per-feature`).
- `problems`: the lines the boot printed: a package that did not read, a
  local package with a baked one's id, a `set.toml` that is not used (it
  does not parse, or it switches `core` or `ui` off), a cycle.
- `content_hash`: the SHA-256 of the content set (the `PackageId`s of the
  packages on with a `sim` item), or `null` when the set did not order.
- `local_only`: why a Live or System Link session would be refused with
  this set (an unsigned package with a `sim` item, named), or `null`.
- `maps`: by map name, each map the rewrite was asked for (or, with
  `store-maps`, each level opened from the store): `scope` (its
  header's class), `packages` applied in order, `held` (packages whose
  tags the map has already, left out), `refused` when packages are off
  for this map (each `package` and `why`: a `from`, `was` or `replaces`
  that does not match what the map holds, a clash with an earlier
  package, or a `requires` that is off), `key` (the cache key; with
  `store-maps`, the level's `LevelId`, empty for a level with nothing
  over it), `hits` and `builds`, `added` and `changed` tag counts, and
  `error` when the map was served without its packages.

```sh
h2 agent packages.list
```

## The tag store

The tag store at `<data>/tags` and the maps it can give back byte for
byte from their skeletons and extent tables
([asset-store.md](../engine/asset-store.md)). With the `store-maps`
feature on, the game reads the disc's maps from it and never from the
disc. Registered by `crates/apps/boot/src/store_status.rs`; the maps are
`vfs::store_map::status`, read from the extent tables.

| Method | Access | Params and result |
|---|---|---|
| `store.status` | read | `{}` → `{root, exists, served, maps, stale, broken, images}` |

- `root`: the store's directory; `exists`: whether it is a tag
  repository.
- `served`: whether this boot serves the disc's maps from it
  (`store-maps`).
- `maps`: each map it can serve: `map`, `length`, `sha256` (the file it
  was extracted from), `skeleton_bytes` (the skeleton file, compressed),
  `extents` and `extent_bytes`.
- `stale`: maps `extract.toml` records with no skeleton or extent table
  (`h2 tags extract --update` takes them again).
- `broken`: maps whose extent table does not read, with `why`.
- `images`: the level images under `<data>/store/images`
  (`workspace::level`): `level_id`, `length` and `sha256` of the map file
  each gives, `disk_bytes` (the image and its table), `extents` and
  `extent_bytes`.

```sh
h2 agent store.status
```
