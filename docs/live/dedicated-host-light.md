# Dedicated host: the light host

## Light host

Run `h2 serve --system-link --host-light` with the existing prebuilt image.
Clanker Training uses the same profile, adds bot seats, and defaults to
unpaced. An operator still uses the retail lobby to create and start a
dedicated game.

The
machine-only lobby and remote join lifecycle above still need a game run.

| Area | What the profile does and why |
|---|---|
| World, HUD, effects | `sim-frame` skips every steady-state world call to `0x00223240`. This drops the world, HUD, and effect draw paths reached there. It calls `0x0012b2e0` for the frame clock, advances counters at `0x00485aa0` and `0x004ba034`, and clears `0x004e6098`. Guest effect updates stay because the tick reads the counters. The software D3D shim keeps display timing and device ABI, with no GPU, display presentation, or render thread. [Skip](../../crates/rewrites/sim-bridge/src/sim_frame.rs) · [Read-back census](../runtime/sim.md#2-what-can-be-skipped-and-what-cant) |
| UI and movies | Bink takes the existing skip return, including intro, attract and credits. The lobby and loading frames keep the guest menu path and cache housekeeping. The menu layer at `0x00147af1` dispatches virtual callbacks, so a blanket guest-side skip needs a write audit. Animation has a tick-side request at `0x001236f0` and a render-side load at `0x001237e0`. Test animation-driven hit and melee behaviour in a real match. [Movie mode](../../crates/extensions/game/src/movies_glue.rs) · [Animation dependency](../graphics/notes/pipelined-render-census.md#the-new-dependency-sites) |
| Audio | The guest polls buffer status and position and waits for stream events. `StreamGetStatus` at `0x0037f591` has a pause bit read at `0x002ae4c4`. The same logical 256-frame grid advances cursors and delivers status, events and callbacks with no sample mixing, output queue, sink or output thread. `SetBufferData` and PCM/ADPCM stream packets validate the guest ranges without reading their sample bytes in DirectSound. Machine snapshots are unavailable with this audio profile because their audio section contains samples. [DirectSound state](../../crates/platform/dsound/src/state.rs) · [Output clock](../../crates/platform/dsound/src/output.rs) |
| Map I/O | The world skip avoids render-initiated texture streaming in steady state. The D3D shim takes the descriptor-only texture path before reading or decoding texels, including lobby and loading frames. ISO reads and immutable DVD-directory `.map` reads can use read-only file-backed mappings. Eligible immutable ranges can also back guest pages directly. Writes to those pages are private to the match. Sim reports count mapped and copied read bytes. Keep the source files intact for the lifetime of each process. The retail copy to the writable cache, loading-screen bitmap reads, and guest sound-cache and WMA reads still run. Shared cache calls serve several resource types, and the audio shim only owns the payload after the guest submits it. [DVD reads](../../crates/runtime/vfs/src/disc.rs) · [Directory reads](../../crates/runtime/vfs/src/dir_disc.rs) · [Cache copy](../../crates/extensions/game/src/map_copy.rs) · [Codec](../../crates/platform/dsound/src/xwma.rs) |
| Memory | The guest has 64 MiB of RAM per process. The OS can share clean native code pages when processes open the same dylib or DLL. The loader registers pointers and selectors, and dispatch tables, guest RAM, cache metadata and network state are private. Use separate `--data` directories and the same immutable image. The light profile omits kernel-call trace wrappers and frame-time history, including during a long lobby wait. [Guest RAM](../../crates/runtime/runtime/src/guest.rs) · [Module loading and boot](../../crates/apps/boot/src/lib.rs) |
| Native simulation | Light hosts can opt into the three native animation, collision and object implementations together. Individual subsystem overrides take precedence. [Controls](../runtime/switches.md#waiting-for-a-measurement) |
| Clock and network | The kernel sleeps when logical time leads wall time. It wakes for vblank, packet-related guest work and audio completion deadlines as needed on the way to the next 30 Hz gameplay tick. The optional pack pacer groups intermediate deadlines on a slot-specific 30 Hz grid and wakes for socket traffic. XNet performs one receive per guest poll and uses an OS wait for timed `select`. [Pacer](../../crates/runtime/kernel/src/pacer.rs) · [XNet sockets](../../crates/live/live-hle/src/network.rs) |
| Bots and telemetry | `--host-bots` enables explicit phantom adds. The configured telemetry feature follows the regular boot and encoder path, including packed v8 where the build and ingest negotiate it. Accountless System Link can select a local capture with `H2_TELEMETRY_FILE`. [Feature registration](../../crates/apps/boot/src/registration.rs) · [Telemetry configuration](../../crates/live/live-telemetry/src/config.rs) |
| Completed match archive | `h2 host run --archive dual\|film` or `H2_HOST_ARCHIVE`. For provisioned leases, set `LIVE_HOST_ARCHIVE` on native Live or the Worker. A pack passes the archive mode to each lease and keeps each match’s film in its own directory. The default `dual` sends live telemetry and uploads the sealed film. In `film` mode the host uploads one film at match end, and Live derives the stats and web replay from that upload. [Cutover](live-api-theater-stats.md#switching-a-host-to-film-ingestion) |

For a quiet, low-population game, use **12 ms of game-thread CPU per tick**
as the first sizing target (about **0.36 of one core** at 30 Hz) and
**1 GiB RSS per process** as the provisional memory allowance. These are
planning figures from the two-bot sim and the earlier process allowance.
Replication, map loads and the map itself can move either number. RSS counts
shared pages in each process.

Measure the fleet’s incremental RAM separately
before sizing by game count.

Measured 2026-11-20, one waterworks 2v2 sim process (macOS `vmmap`,
`/usr/bin/time -l`): peak RSS about **981 MiB**, of which **791 MiB** is
the resident pages of the whole mmapped disc image (the game copies
`shared` (183 MiB), `single_player_shared` (429 MiB) and the level map
from `D:` at load); footprint (dirty) **143 MiB** - Malloc Large 103 MiB
(guest RAM is 64 MiB of that), Malloc Small 35 MiB - plus shared
`__TEXT` 414 MiB. On a container host the resident image pages are
charged per container. Three changes cut that, all behaviour-identical:

- Bulk reads through the disc mapping drop their pages again after the
  copy out (`msync(MS_KILLPAGES)` on macOS, `madvise(MADV_DONTNEED)` +
  `posix_fadvise(DONTNEED)` on Linux): the data lands in the title's
  cache copy or guest RAM, so the image's pages stop accumulating.
- The nav builder on a light host scans only the booted map at boot
  (was: every map on the disc, reading each one's BSPs); later maps
  build on demand as they load.
- The descriptor-only D3D path creates no vertex or index caches (was:
  128 + 32 MiB of LRU budget).

Light hosts build interpreter analysis on demand. Slim hosts keep one lifted
cold owner. A bot visibility grid uses its sorted cell list when a dense
index would exceed 256 KiB. Linux glibc lease children default to two
allocator arenas and a 128 KiB trim threshold.

**Sim stores** ([Sim stores](../runtime/sim.md#61-sim-stores-the-map-without-the-map))
take it further: with `H2_SIM_STORE` naming a directory of built
stores, the host serves each `maps\<map>.map` from the store - only the
bytes a recording saw the guest read - and never streams the retail
map. Two hosts on different maps still share `shared.map`'s store pages
and the dylib's code; two on the same map can also share guest RAM
itself, each over a sparse copy-on-write base image
(`state.load` with `"cow": true`). Measurement plan for the fleet:
`wall.peak_rss_mib` and `wall.footprint_mib` in each run's report;
`vmmap`/`smaps` for shared-versus-private per process; the reports'
`wall.guest_written_pages` over `wall.guest_pages` for how much of the
shared base a match keeps.

### Server maps

`h2 server-map strip ORIGINAL.map OUTPUT.map` writes a sparse copy. Use
`h2 server-map verify ORIGINAL.map OUTPUT.map` to compare SHA-256 over
every retained byte, including all tag headers, tag blocks, BSP metadata,
strings, and the cache header. The verifier checks the original cache
checksum and bounds too.

Set `H2_SERVER_MAP_DIR` to the directory of
verified copies. Name disc and official Map Pack copies by the original
filename and packaged mod copies by the original map ID followed by
`.map`. If a copy is missing or its header or length differs, the light
host uses the original.

Keep matched files immutable while mounted.

The stripper clears known local bitmap LOD and low-detail payloads and
classifiable PCM and Xbox ADPCM sample spans. It keeps WMA, IMA, unknown
or externally referenced ranges, and other raw resources. Tag metadata,
BSPs and the index stay whole, so a Firefight kit map keeps its squads,
zones, orders, sectors and moved group table.

The guest can
read codec bytes before DirectSound sees a packet (WMA decoder
`0x0038a0c5`–`0x0038a1cc`). Render also feeds the animation cache used
from the tick. Keep the original cache alongside its verified override.

The result has the same logical length, with allocated size reduced by
the page-aligned holes. The command prints both sizes. Container savings
depend on the captured files and on the layer format preserving holes or
compressing zeros.

Use the provisional RSS allowance above until measured.
[Stripper](../../crates/apps/cli/src/server_map.rs) · [Mount](../../crates/runtime/vfs/src/map_override.rs)

### Measure before pruning

Set `H2_HOST_AUDIT` to a report path and query `h2 agent hostaudit` before
terminating a live host. The [sim corpus](../runtime/sim.md#host-audit-corpus)
collects the same guest function coverage, tag reads, and subsystem CPU
totals. Keep normal light-host traffic and fleet sizing runs unaudited.

Candidate removals cover effects and lights, first-person animation and
camera shake, sound objects, cloth and antenna updates, and HUD and menu
state. Require a read/write trace and tick-hash parity for each switch.
Effect dispatch at `0x0017a8a0` can apply damage or spawn objects, and
particles share RNG and counters.

Audio status, positions and completion
events are gameplay-visible. Keep those paths and the rest of the guest
tick until their cosmetic work can be isolated. A missing tag read or
entry in one corpus is evidence to investigate.

Map sub-tag pruning stays
off until indirect reads and uncovered equipment are accounted for. The
optional clang host module faults with a guest address on an uncovered
entry. Use the full light image for live games.

### Starting one by hand

`h2 live hosts start` is the admin's way to run a custom dedicated host
outside matchmaking. It signs an `admin.write` request - with the server
key in `--data` (the operator, the default) or an admin account's key with
`--as GAMERTAG` - and `POST`s `/v1/admin/hosts`
([API](live-api.md#dedicated-hosts)):

```sh
h2 live hosts start --map lockout --variant 'Slayer' --max-players 8 \
  [--bots N] [--class small|large] [--region lhr] [--transport udp|tunnel] [--no-launch] \
  [--live URL] [--data DIR | --as GAMERTAG]
```

Without `--no-launch` the server reserves the lease (budget,
`LIVE_MAX_HOSTS` and cost rows all apply, exactly as for a match) and its
configured provider starts the container; the command prints the lease
JSON. `--bots N` (default 0, at most `--max-players` minus one, so one
seat stays for a person) puts N Clanker names in the lease's `bots`; the
host adds them to its lobby as phantoms ([`h2 host run`](#h2-host-run)).
With `--no-launch` no provider is started: the server records the
lease as `requested` and answers its host token, and the command prints
shell lines a manual container run can `eval` - the only place the token
is ever shown:

```sh
eval "$(h2 live hosts start --map lockout --no-launch --live http://127.0.0.1:8074)"
```

sets `H2_LEASE`, `H2_HOST_LEASE_JSON`, `H2_TOKEN` and `H2_LIVE` (the JSON
single-quoted so `eval` cannot split on it). Export the data-bundle values
the same way (`H2_DATA_URL`, `H2_DATA_SHA` from `h2 host bundle`), then run
the image as [deploy/host/README.md](../../deploy/host/README.md) shows:

```sh
docker run --rm --name h2host-smoke --platform linux/amd64 \
  -p 31000-31006:31000-31006/udp \
  -e H2_LEASE -e H2_HOST_LEASE_JSON -e H2_LIVE -e H2_TOKEN \
  -e H2_DATA_URL -e H2_DATA_SHA \
  -e H2_REGION -e H2_OPT -e H2_BASE_PORT=31000 -e H2_PUBLIC_ADDR \
  h2host:local
```

`h2 live hosts list` prints `GET /v1/admin/hosts` (running and recent
hosts, each lease's state, end reason, boot time and cost, and the month's
spend against the budget) with an `admin.read` token.
`h2 live hosts stop LEASE` stops one host with reason `stopped`. Both take
the same `--live`, `--data` and `--as` flags as `start`
([CLI](../../crates/apps/cli/src/live_hosts.rs)).

The manual container owns the same lifecycle as a launched one: it
registers its endpoint with the token (moving the lease straight from
`requested` to `joinable` - no provider boot event exists to wait for),
reports playing and ended, and a lease whose container never registers
times out on the same boot deadline as any `requested` lease. Pre-pull the
image: the boot budget is 60 seconds from `hosts start`.

### `h2 host run`

Start one host for an existing Live lease. The provider supplies the full
`HostLease` JSON object in `H2_HOST_LEASE_JSON`, along with the lease ID,
its bearer token and the signed URL and SHA-256 of the owner's private
game-data bundle. Every required flag can also come from the environment.

| Flag | Environment | Value |
|---|---|---|
| `--lease` | `H2_LEASE` | Live host lease ID |
| `--live` | `H2_LIVE` | Live HTTPS base URL or loopback HTTP URL |
| `--token` | `H2_TOKEN` | Lease-scoped host bearer |
| `--data-url` | `H2_DATA_URL` | Signed bundle URL |
| `--data-sha` | `H2_DATA_SHA` | Bundle SHA-256 |
| `--region` | `H2_REGION` | Optional provider region label |
| `--base-port` | `H2_BASE_PORT` | XNet base port, default 31000 |
| `--tunnel-listen` | `H2_TUNNEL_LISTEN` | Internal tunnel listener, default `127.0.0.1:31100` |
| `--out` | `H2_IMAGE` | Prebuilt game module directory, default the verified bundle's `image/` |
| `--data` | `H2_DATA` | Writable game data directory, default private per-run scratch space |
|  | `H2_AGENT` | Optional TCP endpoint for the child's agent, as `tcp://HOST:PORT` |

On Unix, the supervisor checks that the JSON lease ID matches `--lease`
or `H2_LEASE` before provisioning. It provisions the verified bundle under
`H2_HOST_CACHE` (default `host-data` in the user cache) and starts a child
`h2 serve --system-link --host-light`. The bundle's disc directory must
contain `default.xbe` and the maps.

The child runs the bundle's module,
`<cache>/<sha256>/image`, unless `--out` or `H2_IMAGE` names another
directory. Before either child starts, the supervisor checks the module's
stamp against the XBE and this `h2`'s build settings, including
`H2_OPT`, and stops with a named error if the bundle carries no module or
the stamp does not match.
[The game module](dedicated-host-build.md#the-game-module) Before it starts either child, the
supervisor copies the bundle's `bots/` files into the child's data
directory (`<data>/bots`), where the game loads a map's nav mesh and grid
and the nav worker finds them current. A sim lease whose map has no nav
mesh in the bundle fails before its child starts.
[`install_bot_data`](../../crates/live/host-data/src/lib.rs)

The child gets `H2_HOST_LOBBY=1`, its own private Unix agent socket,
and the provider's `H2_HOST_TOKEN`, `H2_HOST_LIVE_URL` and `H2_HOST_LEASE_JSON`.
The supervisor opens the lease's map and variant with `host.lobby.open`
and `live: true`. Open returns the session ID acknowledged by Live.

The
supervisor registers that ID and the endpoint once `host.lobby.state` shows a lobby. It requests
start when the lobby is ready and has at least the reserved player count,
or one player for an open lobby. Lease roster authorisation belongs to Live.

A non-sim lease with `bots` starts the child with `--host-bots`. Once the
host is registered, the supervisor adds each leased bot through the
child's `sim.player.add` as a brain-driven phantom named as the lease
names it, with the host-made xuid `live_proto::bots::local_xuid(n)` for
seat n from 1. A refused add is a named error in the host log; the other
bots and the lobby carry on.

Phantoms are not players to the supervisor:
it counts only lobby players whose name lacks the bot mark (U+2699), so
phantoms alone never start a match, keep a lobby from going idle, or
appear in the reported player counts.
[`add_phantoms`](../../crates/apps/cli/src/host_cmd/supervisor.rs)

When the supervisor receives a TCP `H2_AGENT` address, the child also
binds that address to the same agent. Both listeners share the same
commands, state and controller claim. The child reports a bind failure
as a startup error.

Cloudflare sets `tcp://127.0.0.1:14720` for
container-local diagnostics. A pack gives each slot its own agent port
from 14720 and tunnel port from 31100. The Local provider drops any inherited `H2_AGENT` from Live so
its leases use their own Unix sockets.

For UDP, the child uses the default route's IPv4 or the IP from
`H2_XNET_ADDR`, with the configured base port. Live receives that address
unless `H2_PUBLIC_ADDR` specifies a public IPv4 or IPv4:port. Keep base
through base+6 available and assign each concurrent host its own range.

For tunnel leases, Live receives the WebSocket URL derived from
`--live` at `/v1/hosts/{lease}/tunnel`, and the child listens at
`--tunnel-listen`.

The supervisor samples lobby state once a second. It reports `playing`
on the first playing sample and reports `ended` on postgame
(`MatchOver`), last departure (`Empty`), an unvisited lobby's timeout
(`Idle`), or the lease's hard duration limit (`MaxDuration`). These
ends close the lobby and return 0.

A child exit before a terminal lobby
state reports `Crashed` and returns 1. The child has up to 60 seconds
after spawn to reach the lobby. The provider also enforces its boot
deadline from the original lease request.

The local provider sets `H2_HOST_PARENT_PIPE=1` and holds the write end of
the supervisor's stdin pipe for the lease. When the provider exits, EOF
requests the same shutdown as SIGTERM, so the supervisor closes the lobby
and reaps its game child. Cloud providers leave this variable unset.

Bot-only leases with a `sim-<run_id>` assignment use the existing
unpaced simulator in a dedicated child process. The supervisor registers
the lease and starts the child with the verified disc and the same
checked module. The child fetches its match from `GET /v1/hosts/{lease}/sim` and uses
the lease bearer to upload the recording and report through Live's
private sim routes.

The supervisor observes gameplay through `match.status`.
A successful child exit reports `MatchOver` to the host API. A failed child
exit reports `Crashed`.

### Compare against ordinary headless serve

Use the same prebuilt image and map, matching warm cache state, roster and
telemetry configuration for both trials. Stop both processes between trials
and start the same client again.

Run the control with `start_trial control`
and the light trial with `start_trial light --host-light`.

```sh
start_trial() {
  label=$1
  shift
  H2_BOOT_REALTIME=1 H2_XNET_ADDR="${lan}:31000" H2_XNET_LOOPBACK_PEERS="${lan}:31010"     "$H2_BIN" serve "$H2_DISC" --system-link "$@" --out "$trial/host-image" --tier clang     --data "$trial/host" --no-halo1 --no-mappack --socket tcp://127.0.0.1:14720     >"$trial/$label.log" 2>&1 &
  host_pid=$!
}
host_agent() { "$H2_BIN" agent --socket tcp://127.0.0.1:14720 --timeout 100 "$@"; }
```

After joining and starting the same map in each trial, collect five
non-overlapping 30-tick windows after the first 300 ticks.

```sh
for ticks in 300 360 420 480 540; do
  host_agent wait "{\"phase\":\"gameplay\",\"map_ticks\":$ticks}" 90000 >/dev/null &&
    host_agent hostmetrics || break
done | tee "$trial/$label-metrics.txt"
```

Compare `game_cpu_ms_per_tick`, `process_cpu_ms_per_tick`, `cpu_percent`
and `rss_bytes`. The game agent reports the gameplay tick p99 over 30 seconds
and counts intervals over 36 ms. On Linux it also reports proportional,
private dirty and shared clean bytes.

The pack reports group memory for each slot. Check `wall_ms_per_tick` against 33.3 ms. The match
check is remote movement, hits, death and respawn, results, and a second
round on the same host.

The render consumes shared RNG at
`0x001032a0`, so the rendered control and the light host can diverge in
lockstep. The hash also includes guest texture and sample buffers. Compare
repeat runs from a clean seed and use the same hash to gate each candidate
prune.

Cross-profile parity is still open. See the [sim checks](../runtime/sim.md#8-determinism).
