# Sim mode: state hashes and bot navigation cost

Status: Built. Post-tick hashes, finding the first differing state, bot navigation performance and the host audit corpus. Part of [Sim mode](sim.md).

### Post-tick state hashes

Each new sim report records `state_hashes_v1` in guest tick order. A row
has the guest `tick` and a 16-digit hexadecimal `gameplay` digest. The
digest covers the shared engine RNG, BSP, cinematic and stats flags, team
scores, and each player slot's datum, score line, pose, aim, velocity,
vitality, vehicle, seat, inventory and animation channel. The encoding
uses fixed-width little-endian words, option presence tags, and the float
bits from telemetry. Player names and wall time have no bearing on it.
Repeat observations replace the same tick. Reloading a snapshot restarts
the list with the tally. A failed snapshot leaves a gap. Older reports
have no series.

For a stricter comparison, `H2_FEATURES=impl.sim.arena-hash` adds an
`arena` field to each row using the existing lockstep hash of the
`0x3fe000`-byte checkpoint arena. It follows the pointer on every tick
and reuses a read buffer. An unreadable arena leaves the field absent
and adds a report note. Compare the tick sequence, check every expected
digest is present, and find the first differing tick. The default digest
can detect player or RNG divergence before the first kill, while the
arena check also covers hidden state inside that allocation.
`H2_LOCKSTEP` provides the wider whole-RAM check. Time matches with arena
verification off so copying and hashing the arena does not enter the budget.

### Finding the first differing state

`h2 sim diff <a.json> <b.json>` says which state differs first, not only
that a digest does. Each report also carries `state_components_v1`: the
same ticks as `state_hashes_v1`, broken into named components with their
values as text. The first row lists every component; each later row lists
only those that changed (a component that goes away reads `-`). It is on in
every `h2 sim run` unless the match file sets `components = false` or
`detail = "training"`, and is
bounded at 250,000 recorded values; a longer match keeps its first ticks and
sets `truncated_after`. The series is built by `sim_bench::diff::Recorder`:
component names are interned once, the arena's block digests stay `u64` until
the report is written, and a value is stored only when it changed. A timing arm that leaves it out still has
`state_hashes_v1`, and `h2 sim diff` against it compares those digests
and says that one report has no component breakdown.

Components: `time.tick`, `rng.seed`, `world.bsp`, `world.flags`,
`team.NN.score`, and for each player slot `player.NN.{id, stats, pos,
velocity, unit_velocity, facing, aim, health, weapon, vehicle, anim,
motion}` (or `player.NN.object` when the unit is gone). They come from the
same telemetry snapshot as the gameplay digest, so they cost one pass of
string formatting per tick and no extra guest reads. Floats print so they
read back exactly.

The object table and every other hidden allocation are covered by the
checkpoint arena, hashed in fixed 64 KiB blocks as `arena.NN` (hex digest).
That needs the 4 MiB read and hash each tick, so it stays behind
`H2_FEATURES=impl.sim.arena-hash` (the same switch as the aggregate `arena`
digest, which it replaces with a single pass). `arena_base` in the series is
the guest address of block 0.

In a full report with arena verification, DirectSound adds `sound.voices`
to the component series. Each line names the logical output frame since
match start, the event, and its guest handles or packet fields. A quiet tick
has an empty value. An A/B run compares the ordered lines using the same
`h2 sim diff` command. A different start, pitch, completion or callback
grid point shows up even when the aggregate gameplay and arena hashes
agree. `h2 sim diff a.json b.json --sound` requires consecutive full-detail
samples, readable voice records and arena digests in both runs. It exits
silently on complete agreement and returns an error for missing coverage
or the first difference. See [DirectSound](../audio/dsound.md#notification-and-callback-contracts).

`detail = "training"` (the match key, or the batch key, which overrides the
match file's) is for runs that only need to know whether two plays agree. It
records no components (`components = true` with it is refused), keeps one
`state_hashes_v1` row a game second (`TRAINING_STRIDE`, 30 ticks, and the
first tick), and adds `state_digest_v1`: `{ stride, ticks, digest }`, a
digest rolled over every tick's gameplay (and arena) digests in order, a
tick seen twice counting once. `h2 sim diff` compares two training reports at
the ticks both sampled, then their rolling digests. If a sampled row differs
it names that tick and the last tick the reports agreed at: the state parted
somewhere in between. If every sampled row agrees but the rolling digests do
not, it says the state parted between sampled ticks. Both say to rerun with
`detail = "full"` to name the component. A training report against a full one
compares the ticks they share and says the rolling digests were not
compared; two training reports of different lengths say so too.

`diff` replays both series and prints the first tick where any component
differs, then those components sorted by name with both values; for an arena
block it adds the block index and guest address range. A tick that only one
run observed shows as component `tick`. If either report lacks the series
(an older report), it falls back to the first tick whose aggregate
`gameplay` or `arena` digest differs, and says so. Reports with neither are
reported as not comparable. A series cut short at its size bound with no
difference so far reports that later ticks are unverified.

The silent audio packet clock and the sim's reduced telemetry reads can
be toggled separately with `-impl.audio.packet-clock` and
`-impl.sim.metrics-projection` in `H2_FEATURES`. The post-bot-change
Waterworks all-local run measured 14.12 ms/tick, with 70.76% of main-thread
samples in guest code. The caller table, skip boundaries and A/B procedure
are in the [simulation budget](../perf/sim-budget.md).

### Bot navigation performance

On 2026-11-20, a five-minute all-local Lockout match took about 10 ms/tick.
Across 12 retail maps the baseline was 10.4–19.5 ms/tick, including
Zanzibar at 19.5, Colossus at 17.9, Headlong at 17.4 and Foundation at 17.1.
A 15-second macOS sample of unpaced, light-profile Waterworks 2v2 (two
local bots and two phantoms) put 3,241 of 5,201 game-thread samples in
bot glue and 2,917 in ground-vehicle pathfinding. Wall checks accounted
for 1,866 top-of-stack samples across threads. The hash-table growth
under those checks came from the temporary set of triangle IDs per ray.

The first optimisation pass added corridor, unreachable-result and portal
clearance reuse. It also removed the temporary triangle set from wall
rays, reused A* search records and let the sim tally take ownership of
each tick sample. The all-local profile in the [simulation budget](../perf/sim-budget.md)
is a separate measurement. Guest code accounted for 70.76% of its main
thread, so a host-side bot change alone cannot halve that workload.

#### Per-tick work

| Work | Before scheduling | Current frequency |
|---|---|---|
| Bot pads | Phantom action gather samples once per fresh simulation tick with a bound unit, then builds and publishes the retail action. | Same cadence for pads, kill events and life transitions. |
| World perception | Per pad sample, read up to 16 players, arms, scoreboard and objective state. A living bot scanned up to 2,048 object slots for weapons, supplies and grenades, then scanned the table again for vehicles and players again for seat occupants. | Per fresh sample, reuse the ground scan's vehicle headers and read occupants when vehicles exist. Perception, vehicle seat markers, recent cues and the delayed observation queue reuse buffers. |
| Player LOS | Per candidate in the notice cone (or near the bot), cast an eye-to-chest wall ray. The delayed mind sample repeated the opponent rays. | Cast against the observer and targets' positions in each fresh sample. Target selection and aim use that sample's visibility and positions. The mask is shared within the sample. The existing human-input timing still applies. |
| Other visibility and wall probes | Ray-test in-cone pickups, supply sites, objective items and non-owned vehicles and eligible events. Foot steering can probe for clearance, headroom, cover and footing. Hearing, grenade arcs and danger checks add conditional probes. | Same sample or action cadence. Aim-point, projected lead and immediate movement clearance use current geometry for the sample. |
| Radio and event cues | Each sample advances the radio at most once per guest tick, filters allies' reports, and gathers recent kills and shots. Fresh thinking ticks update supply and combat memory before issuing the pad. | Same cadence with reused inbox and cue buffers. |
| Target selection and aim | Each fresh mind tick ranks the visible enemies and evaluates tactics, objective and equipment. Some support and engagement checks cast additional rays. Aiming, lead, trigger and pad update while tracking every tick. On-foot combat can cast up to two additional rays for the preferred aim point and lead. | Same cadence and scoring, using the filtered sample. Aim and input timing continue on each mind tick, including ticks without a plan. |
| Foot routes and roaming | A missing route, changed carrier threat or destination movement over one unit could trigger a replan each tick. A plan can try up to three A* searches (safety, blacklist and unrestricted). Picking a roam goal can test reachability of multiple candidates. Ball shelter already caches for 60 ticks or until the threat moves 1.5 units. | Pick a needed roam goal immediately. Plan immediately when a foot route is missing or the destination has moved over one unit. Refresh a usable route on the bot's `foot_plan_ticks` phase. Steering stays tick-rate and committed traversal keeps control. Boarding uses foot routes until entry. |
| Ground vehicle route | For a driver, missing routes and destination movement over three units could query the route every tick. A fresh corridor can check up to three vertical rays per uncached portal. | Active ground drivers with a destination plan immediately when the route is missing, blocked or over three units from the new destination. Refresh a usable route on the bot's `vehicle_plan_ticks` phase. Chassis portals are indexed by nav edge. Corridor and route point storage is reused. |
| Ground vehicle clearance | Each eligible driving tick checked the span to the first point, up to three lookahead points and advancement past nearby waypoints. Each span checks ground and up to three wall rays at each 0.45-unit sample across three lanes. Splatter and reversing can check additional spans. | Full route lookahead on the `vehicle_lookahead_ticks` phase and when a new or reached carrot needs checking. Between full checks, validate the immediate span every driving tick before steering toward the last carrot. That span covers up to 1.8 units or 150 ms of travel, whichever is greater. Splatter and reversing retain live clearance. |
| Fighter steering | Checked floor twice per pilot tick and ran a flip clearance probe even without a newly eligible hit. | Reuse the floor result. Probe flip clearance on an eligible hit. Forward clearance and steering stay tick-rate. |
| Map and session glue | The host seats phantom peers and refreshes session state. Nav decoding or generation is tied to map loads and missing meshes, with the auto-nav worker handling background work. | Same hook cadence. Each brain only copies the map name on a change. |

Refreshes of usable routes are phase-aligned to the in-game player
slot. A lost target or new damage can bring a driver's refresh forward.
On foot, a change in a carrier's threat can bring a route refresh forward.
Urgent refreshes split across even and odd slots, and each planner can
refresh at most once per tick. A route needed for immediate movement
bypasses the slot phase. Timing follows the simulation tick and slot
order, with the same random seeds as the rest of the brain.

The per-sample visibility mask, array scans, perception buffers, driver
portal records and path workspace avoid rebuilding per-tick temporary
collections in the common path. Initial buffer growth, new objectives
and some conditional tactical or route outputs can still allocate.

#### Bot thinking across cores

The brains can think off the game thread ([bots.md §5.5](../game/bots.md)):
`H2_BOT_THREADS=auto` batches every phantom's think across workers, and
`H2_BOT_PIPELINE=1` also overlaps the thinks with the guest's tick. Sims
think serially unless told otherwise: on the quiet laptop four phantoms
think 9% slower batched than serial in one Lockout match, with equal
hashes ([notes](../perf/notes/sim-10x.md)). Per variable, an `H2_BOT_THREADS` or
`H2_BOT_PIPELINE` already in the environment wins, then the match file's
`bot_threads` and `bot_pipeline`, then that default
(`crates/apps/cli/src/sim_cmd.rs`, `think_env`). The run's first log line
names the values it ended with. To measure, run the same match file (Waterworks and
Lockout, phantoms, seed 7) serial, batched and pipelined. Compare
`state_hashes_v1`: batched must match serial at every tick, and pipelined
must match itself across thread counts. Then compare ms/tick and the
game-thread share of bot glue.

#### Matched performance and combat comparison

Use `H2_BEFORE` and `H2_AFTER` for the absolute paths to the two
release `h2` binaries. Both must support phantom peers. Set
`H2_DISC`, `H2_IMAGE` and `H2_DATA` to
absolute paths to the same disc, compatible clang image built at
`H2_OPT=2`, and data directory with both nav meshes. From the repo root,
create two fixtures with four legendary phantoms, identical teams,
seed 7 and a 9,000-tick limit.

```sh
bench_dir="$RUSH_KEEP/bot-speed-phantom"
mkdir -p "$bench_dir"
awk '/^map = / {$0="map = \"waterworks\""}
     /^time_limit_s = / {$0="time_limit_s = 0\nscore_limit = 0\nseed = 7"}
     {print}' \
  docs/runtime/sim-examples/midship-seats.toml > "$bench_dir/waterworks.toml"
sed 's/^map = "waterworks"/map = "lockout"/' \
  "$bench_dir/waterworks.toml" > "$bench_dir/lockout.toml"
```

Run both fixtures on both binaries with the same unpaced light profile
and write separate reports.

```sh
for arm in before after; do
  case "$arm" in
    before) sim_h2="$H2_BEFORE" ;;
    after) sim_h2="$H2_AFTER" ;;
  esac
  for map in waterworks lockout; do
    env -u H2_PROFILE -u H2_TIMELINE -u H2_LOCKSTEP -u H2_BOOT_GFX \
      -u H2_HOST_MODULE -u H2_SERVER_MAP_DIR H2_OPT=2 \
      H2_FEATURES=host-light,-impl.sim.arena-hash \
      "$sim_h2" sim run "$bench_dir/$map.toml" --disc "$H2_DISC" --out "$H2_IMAGE" \
      --data "$H2_DATA" --tier clang --pace unpaced --report "$bench_dir/$map-$arm.json" \
      || break 2
  done
done
```

Verify a loaded mesh, four bound phantom seats, 9,000
ticks and a `tick_limit` exit. Compare `wall.ms_per_tick` and per-seat
kills, shots, hits and accuracy. Vehicle ticks, stuck ticks and replans
help explain a change in kill or accuracy results.

```sh
jq '{map: .match.map, seed: .match.seed, ticks, end: .result.end, nav,
     ms_per_tick: .wall.ms_per_tick,
     players: [.players[] | {seat, name, kills, deaths, shots, hits,
                             accuracy, vehicle_ticks, stuck_ticks, repaths}]}' \
  "$bench_dir"/waterworks-before.json "$bench_dir"/waterworks-after.json \
  "$bench_dir"/lockout-before.json "$bench_dir"/lockout-after.json
```

Repeat each arm with the same inputs and compare `state_hashes_v1` to
test same-build determinism before attributing a difference to the
change. Each run uses fresh processes. Match trajectories can differ
between arms because decisions that require a fresh route can move to a
different tick. Compare Waterworks driving and combat across paired
seeds if a single pair shows a quality change.

## Host audit corpus

Build the full clang image with the usual optimisation setting. Prepare
bot navigation data and install the owned Map Packs in `H2_DATA`. Set
`H2_DISC`, `H2_IMAGE` and `H2_DATA` to absolute paths. Run the matrix
with the original caches so it can report all observed guest reads.

```sh
env -u H2_HOST_MODULE -u H2_SERVER_MAP_DIR H2_OPT=2 h2 sim batch docs/runtime/sim-examples/light-host-corpus.toml --disc "$H2_DISC" --out "$H2_IMAGE" --tier clang --data "$H2_DATA" --parallel 4 --pace unpaced
```

The matrix plans 483 matches across 23 Xbox multiplayer maps, seven game
types, and three seeds with four legendary distributed bots. Missing
assets, unsupported Map Packs and incomplete matches are reported in
`docs/runtime/sim-examples/results/light-host-corpus/audit-missing.json`.
Add deployed mod maps and targeted matches for equipment the bots miss.
The report records the weapon fired, the weapon/vehicle/equipment tag
inventory, player vehicle occupancy, and observed powerup pickup. A
vehicle seat observation does not establish that a bot drove the vehicle
or used its weapon. A map and game-type matrix alone cannot cover every
weapon, vehicle, powerup, objective or lobby transition.

Find the per-run `audit.json` under `runs/<id>/` and the merged
`audit.json` beside `audit-missing.json`. The batch also writes
`audit-equipment-gaps.json` with indexed datums lacking an observed use. Reports from the same image,
profile and registry union their guest owners, entry/resume selectors,
tag lookup requests and byte ranges. Combine additional matches with
`h2 sim audit-merge OUTPUT INPUT [INPUT ...]`. Tag spans are relative to each datum and
are traced at 64-byte line granularity after load or BSP fixation.
The watch observes checked guest reads, including observer reads, and
can miss early loading reads, raw physical access and out-of-line data.
Unidentified tag epochs and missing hooks are recorded.

CPU totals are inclusive across the object, physics, AI, effects,
particles, sound, camera, first-person, HUD/UI and network functions
with mapped hooks. Bot and telemetry times come from their Rust entry
points. The telemetry worker has a separate bucket for consumed samples.
Divide each total by report ticks for an inclusive per-tick
attribution. Nested buckets overlap, and totals include lobby and load
work where the hook runs. Auditing bypasses generated native direct
calls to count visible entries. Second executors forked from the main
runtime (`runtime::executor`), such as the phantom bots' retail input
builder, share its coverage and bypass direct calls the same way, so
the guest code they run is counted with the main runtime's. Inlined
blocks with no selector cannot be counted separately. Use an unaudited
batch for throughput.

For a deployable module, export the owner/selector pairs with
`h2 sim audit-coverage OUTPUT AUDIT [AUDIT ...]` and use
`h2 build --profile host --coverage OUTPUT`. Omitted owners use the
counted interpreter fallback. See [Slim host](../live/dedicated-host-build.md#slim-host)
for recording and comparison.

The experimental `H2_HOST_MODULE` build option accepts a complete
`audit.json` from the matching full clang image. Use a separate output
directory and keep that same report path set when launching it.

```sh
H2_OPT=2 H2_HOST_MODULE="$H2_AUDIT_REPORT" h2 build "$H2_DISC" --out "$H2_SMALL_IMAGE" --tier clang
```

Cold bodies keep their registry and selectors and stop the guest with a
logged address on first entry. Re-run that case with the full light
image, add it to the corpus and rebuild. An audit with missing runs or
hooks cannot build the small module. The full image remains the default.
Compare a candidate against the full light image with the existing
`H2_LOCKSTEP`/`h2 lockstep` check before relying on any prune. Same-profile
repeatability needs to pass first. Cross-profile equality is still open
because the render reads and advances shared state and the hash includes
guest buffers. The light tick keeps guest cosmetic updates and all tag
headers and blocks while those dependencies are being traced.
