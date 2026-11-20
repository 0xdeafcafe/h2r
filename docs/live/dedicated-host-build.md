# Dedicated hosts: the build plan and its contracts

This is the shared contract for the parallel dedicated-host build
(2026-11-19). Twenty lanes write code at once, each on its own branch
off `dh-base`. Lanes **write code only**. They don't run cargo, bun,
tsc, tests, builds, Docker or the game. The coordinator merges every
branch onto one integration branch and runs one combined pass.

Read [dedicated-host.md](dedicated-host.md) first for the why. This file
fixes the seams so lanes don't block on each other. If a lane needs a
seam this file doesn't define, it defines the smallest one it can inside
the files it owns and says so in its report. It doesn't edit another
lane's files.

## What we're building

A Live match can run on a **dedicated host**: a light, headless game
process with no local player. It's started **ad hoc** when matchmaking
forms a lobby and stopped when the match ends. It's never always on.
Cost scales with play.

The same host profile runs **sim matches**: a sim match is a light host
whose seats are bots.

Every piece runs in both homes ([live-two-homes]):

- **Local:** the Live server spawns `h2 host run` as a child process on this machine.
- **Cloudflare:** the Worker starts a Linux container through a Durable Object.
- **Fly Machines** is the second cloud provider behind the same trait. We pick between Fly and Cloudflare after real route timings.

The game data never goes in git or a public image. A host gets the
owner's game data from the owner's private bucket at start and checks
its hash.

```
 matchmaker forms a cohort ──► HostProvider.start(lease) ──► host process
   (SBMM, ping-aware)              local | cf | fly            h2 host run
          │                                                         │
          │◄──── HostReady{endpoint} (host registers itself) ◄──────┤
          ▼                                                         │
 clients get Queue::Join{session_id} + endpoint ──► join over UDP or tunnel
                                                                    │
 match ends / last client leaves / idle 120 s ──► HostEnded ──► process exits
```

## Slim host

The slim profile keeps the guest functions observed in the light host
as native code and uses the boot interpreter for omitted owners. It
preserves the full clang plan's entry and resume routes, including HLE
overrides and returns into an interpreted caller. A coverage miss logs
`fallback: slim host h2fn_<owner> at <selector>, hits=N` on the first hit
and on powers of two. Shutdown logs the exact counts per owner and
selector. `h2 agent fallbacks` reports the interpreter totals and the
number of coverage misses while the process is up.

### Record the coverage

Build a full clang module locally from the same XBE and checkout used for
the candidate. Set `H2_DISC` and `H2_DATA` to the owner's local paths and
choose separate directories for `H2_FULL_IMAGE` and `H2_SLIM_IMAGE`.

```sh
env -u H2_HOST_MODULE H2_OPT=2 h2 build "$H2_DISC" --tier clang --out "$H2_FULL_IMAGE"
```

Install the owned map packs and each deployed mod map, and prepare their
bot navigation files. The existing [light host corpus](../runtime/sim-examples/light-host-corpus.toml)
runs the 23 stock and map-pack multiplayer maps, all seven game types
and three seeds. Add deployed mod maps and targeted matches for
objective, weapon, equipment, vehicle and player join/leave paths. Run
against the original map caches to keep training and map pruning
independent.

```sh
env -u H2_HOST_MODULE -u H2_SERVER_MAP_DIR H2_OPT=2 \
  h2 sim batch docs/runtime/sim-examples/light-host-corpus.toml \
  --disc "$H2_DISC" --out "$H2_FULL_IMAGE" --tier clang \
  --data "$H2_DATA" --parallel 4 --pace unpaced
```

Record real light host matches as well, since the sim boots straight
into the game. Give each process a separate `H2_HOST_AUDIT` output path.
Use `h2 agent hostaudit` against the host's socket for a live read and
retain the final report after a clean stop. Include lobby creation,
peer join, match start, postgame and lobby close. The existing
[host audit agent method](../runtime/agent-api.md) supplies this data.

Check `audit-missing.json` and the equipment gap report before exporting.
`audit-coverage` verifies that inputs share a full clang image and registry,
have completed ticks and have no reported missing runs, hooks or unmapped
selectors. It writes two eight-digit guest addresses per line, the owner
and the entry or continuation that ran. It sorts and deduplicates the
pairs. Keep this small list with the matching XBE revision and corpus
description. The audit JSON and all disc-derived build artifacts stay
private.

```sh
h2 sim audit-coverage "$H2_HOST_COVERAGE" "$H2_SIM_AUDIT" "$H2_LOBBY_AUDIT"
```

### Build and stage

Build a local candidate for sim or LAN checks. A host build always keeps
the complete body and validated selectors for each observed owner.

```sh
H2_OPT=2 h2 build "$H2_DISC" --tier clang --profile host \
  --coverage "$H2_HOST_COVERAGE" --out "$H2_SLIM_IMAGE"
```

For Linux, use the same options with `--arch x86_64-linux`, or stage it
through the script. Set `H2_LINUX_SYSROOT` to the bookworm amd64 sysroot
and `H2_BIN` to a Mac `h2` from this checkout if it is already installed.

```sh
H2_OPT=2 deploy/host/build.sh "$H2_DISC" --coverage "$H2_HOST_COVERAGE"
```

The Linux Rust binary uses the `host` Cargo feature without defaults.
It leaves out the Cranelift compiler and module build commands, native
render glue, headless draw recording and captures, local keyboard
scripts and button icon substitution, and DirectSound's PCM mixing,
resampling and output threads. The movie skip hook, guest sound clock
and completion events, descriptor-only D3D, XAPI and bot pads, and lobby
UI support remain in the host. The stage strips both ELF files, and the
scratch image copies the ELF loader and runtime libraries, certificate
bundle and small entrypoint shell from bookworm.

[Build and package the image](../../deploy/host/README.md). Pass the
matching `H2_OPT` and other codegen settings to both the local run and the
container. `h2 serve --system-link --host-light` and `h2 sim run` accept
the staged slim module by its stamp with no separate coverage file.
Use a distinct output directory for the full module.

### Check size, memory and coverage

- Let `N` be the full Linux build's planned owner count and `K` the
  number of unique owners in the validated list. The build logs both.
  It compiles `K` native owners and omits `N - K`, a count reduction of
  `100 × (N - K) / N` percent. The macOS 18,647-function baseline only
  applies if the matched build has the same owner count.
- For a size estimate, let `F` be the measured bytes of all native
  bodies in a matched full build, and `O` the slim module's other
  bytes. Estimated slim bytes are `O + F × K / N` if retained owners
  are average-sized. With per-owner sizes, use `O + sum(bytes of retained
  owners)` for the estimate. Shard reuse, code layout and linker padding
  affect the final result. Use `wc -c` on both staged ELFs and
  `llvm-size -A` on the modules to measure disk and native text.
- The scratch root filesystem is the sum of the two stripped ELFs,
  loader and libraries, certificates, shell and small configuration
  files. A cold registry pull transfers the compressed rootfs layer size
  plus the manifest and config blobs. Read the layer descriptor sizes
  from the Linux/amd64 image manifest. Compare like-for-like stored or
  compressed bytes with the previous image.
- Use the same map, seed, options and match tick window on the full
  light and slim modules. Audit the full build first, then run both
  with auditing off. For a second pass of the corpus set `audit = false`
  in a copy of its batch TOML and give it a distinct results directory.
  Compare lobby and match completion, lockstep hashes, ms/tick, and the
  `profile.fallbacks` counters. Exercise a legal lobby path that missed
  the recording and check it continues after logging a coverage miss.
- On Linux, capture `docker stats --no-stream` when each lease is in the
  lobby and at the same steady portion of each match. On the Linux
  Docker host, find the game child's PID with `docker top` and compare
  its `/proc/<pid>/smaps_rollup` RSS and PSS. Sum `Rss` and `Pss` from
  the executable `libh2game.so` mappings in that child's `smaps` to
  isolate native text. On macOS, compare `vmmap -summary <pid>` for
  the sim child at the same points and use detailed `vmmap <pid>` to
  isolate the game's resident `__TEXT` from the rest of the process. The host's interpreter analyses and caches guest code as
  needed, so disk bytes and resident bytes should be reported separately.

## Contracts

### 1. Lease (live-proto, owned by lane `dh-lease`)

`crates/live/live-proto/src/hosts.rs`, re-exported as `live_proto::hosts`:

```rust
pub struct HostLease {
    pub id: String,                  // "hl-<base64url 16 bytes>"
    pub assignment_id: String,       // matchmaking Assignment.id, or "sim-<id>" / "custom-<id>"
    pub provider: ProviderKind,      // Local | Cloudflare | Fly
    pub region: Option<String>,      // provider region code, e.g. "lhr", "enam"
    pub class: HostClass,            // Small (1 vCPU, 2 GiB) | Large (2 vCPU, 4 GiB)
    pub map: String,                 // scenario name, e.g. "lockout"
    pub map_id: Option<String>,      // mod package hash, None for retail
    pub variant: Option<String>,     // game variant name
    pub hopper: Option<u32>,
    pub max_players: u8,             // 1..=16
    pub roster: Vec<Xuid>,           // empty for open custom games
    pub bots: Vec<String>,           // bot seat names; non-empty only for sim and bot fill
    pub transport: Transport,        // Udp | Tunnel
    pub idle_timeout_s: u32,         // default 120
    pub max_match_s: u32,            // hard stop, default 1800
    pub created_ms: i64,
}
pub enum ProviderKind { Local, Cloudflare, Fly }
pub enum HostClass { Small, Large }
pub enum Transport { Udp, Tunnel }

pub struct HostEndpoint {
    pub transport: Transport,
    pub udp: Option<String>,         // "203.0.113.4:31000", base port; game uses base..base+6
    pub tunnel_url: Option<String>,  // "wss://live.forbes.red/v1/hosts/<lease>/tunnel"
    pub xnaddr: String,              // hex of the host's XNADDR as the HLE advertises it
    pub session_id: Option<String>,  // Live session the host published
}

pub enum HostState { Requested, Booting, Joinable, Playing, Ended, Failed }
pub enum EndReason { MatchOver, Empty, Idle, MaxDuration, Stopped, BootTimeout, Crashed, Budget }
```

All types derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`
with `#[serde(rename_all = "snake_case")]` on enums. `Assignment` gains:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub host_lease: Option<String>,      // set when a dedicated host serves this assignment
```

### 2. Events (live-proto event types, owned by `dh-lease`)

Server-signed events on stream `hosts`. Payloads are JSON:

| type | data |
|---|---|
| `HostRequested` | `{ lease: HostLease, provider_ref? }`. Cloudflare's room address is known before allocation. |
| `HostBooting` | `{ lease_id, provider_ref }` (container id or machine id or pid) |
| `HostReady` | `{ lease_id, endpoint: HostEndpoint, boot_ms }` |
| `HostPlaying` | `{ lease_id, players }` |
| `HostEnded` | `{ lease_id, reason: EndReason, ran_ms, players_peak }` |
| `HostStopped` | `{ lease_id }`. The provider confirmed shutdown for a lease with a reserved address. |

Cost metering (`dh-cost`) and the admin page (`dh-web`) read only these
events. They don't call providers.

### 3. HostProvider trait (live-server, owned by `dh-provider`)

`crates/live/live-server/src/hosts.rs` (+ `hosts/` submodules):

```rust
#[async_trait(?Send)]   // match the crate's existing async trait style; wasm has no Send
pub trait HostProvider {
    fn kind(&self) -> ProviderKind;
    /// Start a host for `lease`. Returns the provider's own reference.
    async fn start(&self, lease: &HostLease, boot: &HostBoot) -> Result<String, HostError>;
    async fn stop(&self, lease_id: &str, provider_ref: &str) -> Result<(), HostError>;
    async fn alive(&self, provider_ref: &str) -> Result<bool, HostError>;
}

pub struct HostBoot {
    pub live_url: String,     // where the host registers, e.g. https://live.forbes.red
    pub token: String,        // host credential (contract 4)
    pub data_url: String,     // signed URL of the owner's game-data bundle (contract 7)
    pub data_sha256: String,
}
```

`hosts.rs` also owns the lifecycle in `Live`:

- `Live::request_host(assignment-ish input) -> HostLease`. It checks the budget guard first (contract 8), then emits `HostRequested` and calls `provider.start`.
- Then it moves through `Booting → Joinable → Playing → Ended`.
- **Boot timeout:** 60 s → `Failed(BootTimeout)`, plus the fallback.
- **Reaper:** on the existing alarm/tick, it stops hosts past `max_match_s` or idle past `idle_timeout_s`.
- **Max concurrent hosts:** `LIVE_MAX_HOSTS`, default 4.

Providers:

| impl | file | owner |
|---|---|---|
| `LocalProvider`: spawns `h2 host run` with its own `--data` dir under the server's data root, or with `LIVE_HOST_PACK_MATCHES` set, `h2 host pack` processes routed by the shared `Pool` | `crates/live/live-server/src/hosts/local.rs` (native only, `cfg(not(target_arch = "wasm32"))`) | `dh-provider` |
| `CloudflareProvider`: a `HostRoom` Durable Object per lease, routed by a pool room to pack rooms that each own a Container running several matches ([Packed hosts](dedicated-host.md#packed-hosts)) | `crates/live/live-worker/src/hostroom.rs` + `wrangler.toml` `[[containers]]` | `dh-cf` |
| `FlyProvider`: Machines API, `auto_destroy`, region from the lease | `crates/live/live-server/src/hosts/fly.rs` (HTTP through the crate's existing fetch abstraction so it builds for both homes) | `dh-fly` |

### 4. Host credential and host-signed telemetry (owned by `dh-identity`)

A host signs in as a **service identity** scoped to one lease. The
token is a server-issued JWT with:

- `sub = "host:<lease_id>"`
- `lxm` in `hosts.register`, `hosts.report`, `sessions.publish`, `telemetry.ingest`
- an expiry of `max_match_s + 300`

`Live::issue_host_token(&HostLease) -> String` lives in
`crates/live/live-server/src/host_identity.rs`. The telemetry the host
uploads carries this identity. The ranks code counts a game for
Ranked only when the assigned host's lease signed it and its players
match the lease roster.

When a lease is active, clients don't upload frames for that game; the
host is the one source. A `frames_from_host: bool` field rides on
`Queue::Join`.

### 5. Host HTTP API (live-server http, owned by `dh-api`)

| route | auth | does |
|---|---|---|
| `POST /v1/hosts/{lease}/register` | host token | body `HostEndpoint`; → `HostReady`; answers the lease |
| `POST /v1/hosts/{lease}/report` | host token | body `{ state: "playing"\|"ended", players, reason? }` → `HostPlaying`/`HostEnded` |
| `GET /v1/hosts/{lease}` | admin or roster member | lease + state + endpoint |
| `GET /v1/admin/hosts` | admin | running + recent hosts with cost (contract 8) |
| `POST /v1/admin/hosts` | admin (`admin.write`) | starts a custom host by hand: `{lease}` when launched, `{lease, token}` with `launch: false` (the only route that returns the token) |
| `POST /v1/admin/hosts/{lease}/stop` | admin | stops it (`Stopped`) |
| `GET /v1/hosts/{lease}/tunnel` | roster member's `sessions.relay` bearer on upgrade (like the co-op relay) | WebSocket datagram tunnel (contract 6) |

The handlers call into `hosts.rs` (`Live::host_register`,
`Live::host_report`, `Live::host_get`, `Live::admin_hosts`,
`Live::admin_stop_host`). `dh-api` writes the routes and request types
against those names. `dh-provider` writes the methods.

### 6. Tunnel wire (live-proto, owned by `dh-tunnel-client`)

`crates/live/live-proto/src/tunnel.rs`. One WebSocket binary message is
one datagram:

```
u8  version = 1
u8  kind     0 = datagram, 1 = hello (client → host, once), 2 = bye
u16 port     game port offset 0..=6 (little-endian)
u32 peer     tunnel peer id assigned by the host in its hello reply; 0 from a client
..  payload  ≤ 1,264 bytes
```

The API:

- `pub fn encode(frame: &Frame) -> Vec<u8>`
- `pub fn decode(bytes: &[u8]) -> Result<Frame, TunnelError>`
- `pub struct Frame { pub kind: Kind, pub port: u16, pub peer: u32, pub payload: Vec<u8> }`

The host gives each tunnel peer a synthetic address `10.77.x.y`. The
game sees it as an ordinary XNet peer.

### 7. Game data bundle (owned by `dh-provision`)

The owner runs `h2 host bundle --disc <iso> --out <dir>`. It writes:

- `host-data.tar.zst`: only what a host needs. That's maps stripped by the light-host stage-2 stripper when present, and full maps otherwise, plus `shared.map`, `single_player_shared.map` and mainmenu.
- `host-data.json`: the sha256 and the file list.

The owner uploads both to a **private** bucket. A host fetches the
bundle at start from `HostBoot.data_url` and checks the hash. It keeps
a per-machine cache keyed by hash. The bundle is never baked into an
image or committed.

### 8. Cost and budget (owned by `dh-cost`)

`crates/live/live-server/src/host_costs.rs` folds the durable `hosts` stream
into per-lease cost estimates. The providers use the following rates.

- Price config is `LIVE_HOST_PRICES` as JSON, with defaults from [dedicated-host.md](dedicated-host.md).
- Cloudflare: USD per vCPU-second, GiB-second of memory, GB-second of
  disk and GB of estimated egress, plus a flat estimate per start.
- Fly: $/s per class.
- Local: zero.

Before a provider start, `Live::request_host` checks accrued spend, the
remaining commitments on open leases and the proposed lease against
`LIVE_HOST_BUDGET_USD`. The Worker defaults to $25 per UTC month and
native Live defaults to unlimited. A refusal lets matchmaking fall back
to a peer host. A Worker-launched container keeps its reservation until
the provider confirms shutdown. The admin hosts inventory exposes
month-to-date spend and the configured cap.

### 9. Host process (owned by `dh-supervisor` and `dh-lobby`)

`h2 host run --lease <id> --live <url> --token <t> --data-url <u> --data-sha <h> [--region r]`
(`crates/apps/cli/src/host_cmd.rs`, `dh-supervisor`):

1. Provision the data (contract 7).
2. Boot the light host: the same boot path as `h2 serve --system-link --host-light`, with no window and no local player.
3. Drive the lobby through the agent methods below.
4. Register the endpoint (contract 5) and report state.
5. Exit on match over, empty, idle or max duration. The exit code is 0 on a clean end.

Playerless lobby agent methods (`crates/apps/boot/src/host_lobby.rs` +
agent-proto entries, `dh-lobby`):

| method | params | result |
|---|---|---|
| `host.lobby.open` | `{ map, variant?, max_players, live: bool }` | `{ session_id? }` |
| `host.lobby.state` | `{}` | `{ phase: "menu"\|"lobby"\|"loading"\|"playing"\|"postgame", players: [..], ready }` |
| `host.lobby.start` | `{}` | `{}` |
| `host.lobby.close` | `{}` | `{}` |

### 10. Linux x86-64 build (owned by `dh-linux-*`)

- The target triple is `x86_64-unknown-linux-gnu`.
- The game module is an ELF shared object loaded the way the macOS dylib is.
- The `h2` binary builds with `--no-default-features --features host` on Linux. It leaves out window, Metal, AVFoundation, input devices and app icons.
- The container image is `deploy/host/Dockerfile` for `linux/amd64`: a scratch rootfs with bookworm libraries, `h2` and the module, and no game data.

## Lanes and who owns which files

Each lane owns the files listed and may add new files under its own
directory. The **Touch** column lists a shared file the lane may edit
in one small, marked place (a `mod` line, a route line, a match arm).

| # | lane | model | owns | touch |
|---|---|---|---|---|
| 1 | `dh-lease` | sol | `live-proto/src/hosts.rs`, host event types, `Assignment.host_lease`, `Queue::Join.frames_from_host` | `live-proto/src/lib.rs`, `matchmaking.rs` |
| 2 | `dh-provider` | astra | `live-server/src/hosts.rs`, `hosts/mod`, `hosts/local.rs`, lifecycle + reaper | `live-server/src/lib.rs`, `app.rs` (one field + alarm call) |
| 3 | `dh-api` | sol | `live-server/src/http/hosts` routes (or `http_hosts.rs`) | `http.rs` router lines |
| 4 | `dh-identity` | astra | `live-server/src/host_identity.rs`, ranks/arbitration host-signed checks | `ranks.rs`, `arbitration.rs`, telemetry identity |
| 5 | `dh-cost` | glm | `live-server/src/host_costs.rs` + tests | — |
| 6 | `dh-cf` | astra | `live-worker/src/hostroom.rs`, `CloudflareProvider`, container binding, tunnel forwarding in the DO | `live-worker/src/entry.rs` routing, `wrangler.toml` |
| 7 | `dh-fly` | glm | `live-server/src/hosts/fly.rs` | — |
| 8 | `dh-matchmaker` | sol | dedicated request at cohort form, region pick from RTT, peer fallback | `live-server/src/matchmaking*.rs` |
| 9 | `dh-tunnel-client` | astra | `live-proto/src/tunnel.rs`, client XNet transport over the tunnel | `live-hle/src/network.rs` (transport select) |
| 10 | `dh-tunnel-host` | sol | host side of the tunnel: accept, peer ids, synthetic addresses, port fan-out | new `live-hle/src/tunnel_host.rs` |
| 11 | `dh-client-join` | sol | game client joins a dedicated host from `Queue::Join` (+ endpoint), UI line | `live-hle` matchmaking client glue |
| 12 | `dh-lobby` | astra | playerless lobby: `boot/src/host_lobby.rs`, agent methods | `boot/src/lib.rs` registration line, `agent-proto` |
| 13 | `dh-supervisor` | sol | `cli/src/host_cmd.rs`, the host run loop | `cli/src/main.rs` subcommand line |
| 14 | `dh-provision` | glm | `h2 host bundle`, fetch + verify + cache (`crates/live/host-data/`) | `cli/src/main.rs` subcommand line |
| 15 | `dh-linux-emit` | astra | `emit` Linux target, ELF module loading | `runtime` loader cfg |
| 16 | `dh-linux-runtime` | astra | Linux `cfg` for runtime, kernel, fastmem, perf: guest RAM reservation, fault handling, threads, timers | those crates' platform modules |
| 17 | `dh-linux-headless` | sol | `host` cargo feature: cli/boot/bink/input/app gated so Linux builds without GUI | `Cargo.toml` features across those crates |
| 18 | `dh-image` | glm | `deploy/host/`: Dockerfile, cross-build script (zig or clang + sysroot), entrypoint | — |
| 19 | `dh-sim` | sol | sim batches on hosts: `h2 sim batch --provider local\|cloud` through `HostProvider`, reports into sim_runs | `sim` batch runner, `live-server/src/sim_runs.rs` |
| 20 | `dh-web` | glm | Uplink admin **Hosts** page, game pages say "Dedicated (region)" | `web/src` route line |

Lane 2 is the lifecycle hub. Lanes 3, 5, 6, 7, 8 and 19 call its names
as written above. If lane 2 has to rename something, the coordinator
fixes the callers at integration.

Light-host stages 2–3 (stripped server maps, measured pruning) continue
in their own lane and plug into contract 7.

## Coordinator's integration pass

1. Merge all 20 branches onto `integ-dh`, resolving touch-point conflicts.
2. Run the full workspace test pass plus wasm, `worker-build`, smoke and web.
3. Run `cargo build --features host` for `x86_64-unknown-linux-gnu` (zig cross), then build the image with `docker buildx --platform linux/amd64`.
4. Locally: `h2 host run` through `LocalProvider` on this Mac, a matchmade 1-v-1 with a bot, join, end, exit, and the cost row.
5. Run the Linux image under Docker (Rosetta) with a local bundle: boot to `Joinable`.
6. Cloudflare: deploy to a preview environment and run one ad-hoc container match. It stops by itself. Check the billing row.

[live-two-homes]: live.md
