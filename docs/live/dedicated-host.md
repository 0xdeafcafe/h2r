# Running Halo 2 as a dedicated host

**Partially feasible.** A Mac can run the native title without a window. The question is whether Halo 2 will keep that machine in charge when every player is remote. Its simulation chooses the host by machine identity, but its normal lobby starts through a local player. This branch adds `h2 serve --system-link` so the coordinator can test the first real client join. The join and zero-local-player lifecycle still need a game run.

Live would choose and reserve the machine. The native game would run the match. With that arrangement every player has a network path to the authority, including the player who used to host. Fairness improves by removing that player's zero-network-delay advantage. Latency improves for a given player only if the new route and server perform better.

## What the host has to own

| Retail behavior | Consequence for a dedicated process |
|---|---|
| With distributed protocol 2, the game compares the local peer index with the host peer index at start. The host gets game mode 5, simulation **world** mode 4. A client gets game 4, world 5. The host sends each connected remote machine a view of the running world. | Keep the local **peer** and its channels. The authority selector tests peer identity. It does not require the host to own a biped. |
| The party leader controls the lobby. The profile/players screen and Start Game act through a local controller. Each frame the session reconciles signed-in local users into session players. Each peer supplies map readiness and, for matchmaking, hopper status. The start waits for remote machine views. | A local profile normally becomes a player with a spawn, unit, team and score. Despawning it once or leaving its controller idle still leaves its membership. A machine-only owner needs a way to supply properties, accept a ready roster and start without the controller. |
| The session stores **16 peer records and 16 player slots**. A server consumes a peer even with an empty player list. | The existing layout accommodates at most **15 remote consoles**. Sixteen humans require at least two sharing one remote console and a variant allowing 16. An idle host player leaves at most 15 human slots. |

The host indices, local-user reconciliation, player configurations, countdown and remote-view waits are traced in the repo. A permanent nonspawning spectator role has no demonstrated retail path. A spectator workaround would need tests for respawn, team balance, objectives, score, results and capacity. The theater design proposes viewing a stream outside the session. [Phantom session implementation](../../crates/bots/src/phantom/session.rs) · [Local players and remote peers](../game/bots.md) · [Theater spectator options](../game/theater.md)

**Headless today.** The fast sim boots straight into a *local* map with one to four bot controllers. Ordinary `h2 serve` runs the full title without a window, including the guest render front end. `--system-link` enables XNet for the retail lobby. The light host reuses the sim’s world-render skip while running that same lobby. [Sim setup](../../crates/boot/src/sim_glue.rs) · [Direct path and render costs](../runtime/sim.md) · [Headless CLI](../../crates/cli/src/boot_cmd.rs)

**What Clankermatch proves.** Live supplies a hopper and bot roster. An empty search induces the *player console* to form the target session. The party and target can differ. The host can add a phantom as a remote machine and player, borrowing a local configuration and injecting its controls. Its channel is absent, so its special start handling bypasses waits that real clients must satisfy. Zero local **bots** still leaves a human profile. Runs that hit a controller-unplugged prompt demonstrate why a changed slot count cannot establish a working launch. [Clankermatch implementation](../../crates/boot/src/clankermatch.rs) · [Hosting and observed start failures](../game/bots.md)

## What changes for the player

A peer host pays **0 ms of network RTT** to its own authority. For scale, Microsoft Research used **85 ms mean RTT** from Halo 3 console-pair probes. That sample includes candidate peers before final host selection. Saudi Arabia’s Q4 2023 fixed-line measurements to regional VALORANT servers showed **14–18 ms** by ISP, versus **76 ms** nationally to a European World of Warcraft service. These are different games, populations and routes. We have no measured h2recomp distribution or promised saving. Friends on the same LAN can already beat a regional provider. [Microsoft Research, Htrae](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/sigcomm09-265.pdf) · [CST, Game Mode Q4 2023, pp. 13–15](https://www.cst.gov.sa/-/media/cst-website-app/data/media/Reports/Data%20and%20Analysis/EN/Game%20Mode/Game_Mode_202304-en)

At 40 ms symmetric RTT, a shot takes about 20 ms to reach the host and confirmation about another 20 ms to return, plus tick scheduling, replication and presentation. Distributed clients predict local movement and reconcile with the host. The co-op pad schedule belongs to synchronous campaign play. Its input delays do not predict distributed multiplayer hit registration. Retail weapon behavior also varies. A client can report a projectile impact in its first tick. At 30 Hz the Battle Rifle’s 400 world units/s gives about **13.3 units** of first-tick travel. Longer shots involve projectile flight. Bungie described original Halo 2 melee as awarding the first blow observed by the host. A lower-ping remote can still win a close contest. We have no evidence of a universal hitbox rewind or a verified per-weapon validation algorithm in this recomp. [Distributed versus synchronous prediction](live-coop.md#154-prediction) · [Halo developer notes on the original BR](https://support.halowaypoint.com/hc/en-us/articles/23707515298324-Halo-The-Master-Chief-Collection-MCC-Update-February-2024) · [Bungie’s original melee account](https://halo.bungie.org/bwu/index.html?item=163)

An operator-controlled host keeps a player from choosing the authoritative clock, changing the match result on that machine or ending everyone’s game by quitting. Its telemetry can corroborate damage and kills. Input automation, information available to modified clients, packet loss and differing routes remain. A donor controls their machine and can falsify the authoritative record. Start donated servers as unranked. Central hosting also trades the former host’s immediate authority and availability of local play for dependence on capacity, routing and operator uptime. Preserve ordinary custom games and abort an interrupted ranked round without awarding a partial result. [Live trust boundary](live-security.md) · [Gameplay telemetry](live-telemetry.md)

## Where the game could run

**Asset rights decide whether to build a fleet.** The repo expects each user’s own legally obtained disc. The runtime loads a native module derived from its XBE and needs maps and shared disc data. Microsoft’s fan-content rules do not establish permission here to copy those assets into a hosted registry, backups or regional replicas, or operate them concurrently for other people. A private bucket or a disc upload does not settle those questions. Obtain a rights determination covering the actual copies, operators and service model before a managed pilot. Owner-provisioned hardware keeps the disc on its owner’s machine. Separately review public donated hosting. [Project requirements](../../README.md) · [Boot inputs](../runtime/BOOT.md) · [Microsoft Game Content Usage Rules](https://www.xbox.com/en-US/developers/rules)

The native emitter targets arm64 and x86-64 macOS, arm64 iOS/tvOS and x86-64 Windows MSVC. The Windows desktop target has cross-build evidence. A dedicated desktop run still needs qualification. The native game emitter lists no Linux target. A Linux Live service or a Worker build does not produce a Linux game module. Both cloud choices below need an x86-64 Linux ELF module, validated loading/ABI, headless runtime dependencies and an end-to-end join. [Target triples](../../crates/emit/src/compile.rs) · [Windows status](../runtime/windows.md) · [Worker implementation](live-worker.md)

| Host | Path to players and readiness | Illustrative USD per occupied match-hour |
|---|---|---|
| Home Mac or PC | Mac is the first test platform. Expose direct UDP on a reachable address for WAN. Availability depends on power, upload and NAT. | Assuming 30 W incremental load and $0.30/kWh, **$0.009/h** electricity, plus hardware, broadband and idle time. |
| Donated host | Owner installs their own disc data. Admit by build, available slots and measured route. CGNAT needs a relay or a different host. | Same power calculation paid by the donor. Service still pays for coordination, optional relay and support. |
| Fly Machines | Pick a region. UDP needs dedicated IPv4, binding to `fly-global-services` and matching external/internal ports. Give each match a stable endpoint or route all its clients to one machine. | In iad/ewr, performance-1x/2 GB is $33 per 720 h, **$0.046/h**. Performance-2x/4 GB is **$0.092/h**. Add $2/month per dedicated IPv4, storage and NA/EU egress at $0.02/GB. [Pricing](https://docs.fly.io/about/pricing) · [UDP](https://docs.fly.io/networking/udp-and-tcp) |
| Cloudflare Containers | A Worker can allocate a Linux/amd64 Container through a Durable Object. End users cannot send raw UDP directly. An adapter through a Worker and Durable Object adds a routing leg. A WebSocket can also delay later updates behind a lost packet. Measure that path to the actual container region. A Worker isolate cannot execute this native game. | Standard-2, 1 vCPU/6 GiB/12 GB. After plan allowances, 6 × $0.009 memory + 12 × $0.000252 disk + 0.5–1 active CPU × $0.072 = **$0.093–0.129/h**. Add Workers Paid, Durable Objects, transfer and idle allocation as applicable. [Pricing](https://developers.cloudflare.com/containers/platform/pricing/) · [Ingress and lifecycle](https://developers.cloudflare.com/containers/concepts/architecture/) |

Rates checked November 19, 2026. The figures assume one running instance per occupied match and exclude taxes. Divide running compute cost by occupancy to account for a warm pool. For example, 50% occupancy doubles compute per occupied hour. At an *assumed* aggregate game egress of 1 Mbit/s, 0.45 GB leaves per hour, another $0.009 on Fly in NA/EU. Telemetry ingest is a different stream. Fly describes low double-digit seconds to create a VM and typically under a second to start a prepared Machine. Cloudflare reports cold starts often around 1–3 s. Both exclude title boot, disc reads, lobby readiness and start-to-spawn. In the Mac co-op harness with seeded data it took about 19 s from opening the agent socket to the main menu. A joinable distributed lobby and playable spawn come later. Cache a matching module ahead of allocation. Measure cold and warm allocation-to-joinable and join-to-first-playable p50/p95 separately before choosing a warm pool. [Co-op startup observations, §12.6](live-coop.md) · [Fly lifecycle](https://docs.fly.io/machines/overview) · [Container lifecycle](https://developers.cloudflare.com/containers/concepts/architecture/)

**Sizing is provisional.** The two-bot result supplied for this design, 2.8× unpaced at 30 ticks/s, gives 84 ticks/s or **11.9 ms/tick**. If CPU-bound, that uses about 36% of one comparable serial core when paced. If 10–20% of that work scales linearly with player count, sixteen seats cost 11.9 × (1 + 7f) with f = 0.10–0.20, or **20–29 ms/tick** (about **0.61–0.86 of a comparable core**), before network replication. At about 26% scalable work the 33.3 ms budget is already exhausted. Remote humans replace bot thinking with real replication and acknowledgement work. The sim design budgets roughly **1 GB/process** including guest RAM, module and caches. Sixteen-player CPU and peak RSS are unmeasured. Trial one sustained core and 2 GiB, then two cores and 4 GiB. Extra cores can handle other threads. One slow simulation thread can still miss its deadline. [Sim cost model and report](../runtime/sim.md#10-expected-speed-up) · [Report fields](../../crates/sim/src/metrics.rs)

## Light host

Run `h2 serve --system-link --host-light` with the existing prebuilt image.
An operator still uses the retail lobby to create and start the game. The
machine-only lobby and remote join lifecycle above still need a game run.

| Area | What the profile does and why |
|---|---|
| World, HUD, effects | `sim-frame` skips every steady-state world call to `0x00223240`. This drops the world, HUD, and effect draw paths reached there. It calls `0x0012b2e0` for the frame clock, advances counters at `0x00485aa0` and `0x004ba034`, and clears `0x004e6098`. Guest effect updates stay because the tick reads the counters. The software D3D shim keeps display timing and device ABI, with no GPU, display presentation, or render thread. [Skip](../../crates/feature/src/sim_frame.rs) · [Read-back census](../runtime/sim.md#2-what-can-be-skipped-and-what-cant) |
| UI and movies | Bink takes the existing skip return, including intro, attract and credits. The lobby and loading frames keep the guest menu path and cache housekeeping. Animation has a tick-side request at `0x001236f0` and a render-side load at `0x001237e0`. Test animation-driven hit and melee behaviour in a real match. [Movie mode](../../crates/boot/src/movies_glue.rs) · [Animation dependency](../graphics/pipelined-render.md#the-new-dependency-sites) |
| Audio | The guest polls buffer status and position and waits for stream events. `StreamGetStatus` at `0x0037f591` has a pause bit read at `0x002ae4c4`. The same logical 256-frame grid advances cursors and delivers status, events and callbacks with no sample mixing, output queue, sink or output thread. `SetBufferData` and PCM/ADPCM stream packets validate the guest ranges without reading their sample bytes in DirectSound. Machine snapshots are unavailable with this audio profile because their audio section contains samples. [DirectSound state](../../crates/dsound/src/state.rs) · [Output clock](../../crates/dsound/src/output.rs) |
| Map I/O | The world skip avoids render-initiated texture streaming in steady state. ISO reads and immutable DVD-directory `.map` reads can use read-only file-backed mappings. Keep those source files intact for the lifetime of each process. The retail copy to the writable cache, loading-screen bitmap reads, and guest sound-cache and WMA reads still run. Shared cache calls serve several resource types, and the audio shim only owns the payload after the guest submits it. [DVD reads](../../crates/vfs/src/disc.rs) · [Directory reads](../../crates/vfs/src/dir_disc.rs) · [Cache copy](../../crates/boot/src/map_copy.rs) · [Codec](../../crates/dsound/src/xwma.rs) |
| Memory | The guest has 64 MiB of RAM per process. The OS can share clean native code pages when processes open the same dylib or DLL. The loader registers pointers and selectors, and dispatch tables, guest RAM, cache metadata and network state are private. Use separate `--data` directories and the same immutable image. The light profile omits kernel-call trace wrappers and frame-time history, including during a long lobby wait. [Guest RAM](../../crates/runtime/src/guest.rs) · [Module loading and boot](../../crates/boot/src/lib.rs) |
| Clock and network | The kernel sleeps when logical time leads wall time. It wakes for vblank, packet-related guest work and audio completion deadlines as needed on the way to the next 30 Hz gameplay tick. XNet performs one receive per guest poll and uses an OS wait for timed `select`. [Pacer](../../crates/kernel/src/pacer.rs) · [XNet sockets](../../crates/live-hle/src/network.rs) |
| Bots and telemetry | `--host-bots` enables bot support for explicit adds. `H2_BOTS` also opts in and supplies the roster. The configured telemetry feature follows the regular boot and encoder path, including packed v8 where the build and ingest negotiate it. Accountless System Link can select a local capture with `H2_TELEMETRY_FILE`. [Feature registration](../../crates/boot/src/registration.rs) · [Telemetry configuration](../../crates/live-telemetry/src/config.rs) |

For a quiet, low-population game, use **12 ms of game-thread CPU per tick**
as the first sizing target (about **0.36 of one core** at 30 Hz) and
**1 GiB RSS per process** as the provisional memory allowance. These are
planning figures from the two-bot sim and the earlier process allowance.
Replication, map loads and the map itself can move either number. RSS counts
shared pages in each process. Measure the fleet’s incremental RAM separately
before sizing by game count.

### Compare against ordinary headless serve

Use the image, profile and port setup from the [Mac spike](#mac-system-link-spike).
Use the same prebuilt image and map, matching warm cache state, roster and
telemetry configuration for both trials. Stop both processes between trials
and start the same client again. Run the control with `start_trial control`
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
  host_agent wait "{"phase":"gameplay","map_ticks":$ticks}" 90000 >/dev/null &&
    host_agent hostmetrics || break
done | tee "$trial/$label-metrics.txt"
```

Compare `game_cpu_ms_per_tick`, `process_cpu_ms_per_tick`, `cpu_percent`
and `rss_bytes`. Check `wall_ms_per_tick` against 33.3 ms. The match
check is remote movement, hits, death and respawn, results, and a second
round on the same host. The render consumes shared RNG at
`0x001032a0`, so the rendered control and the light host can diverge in
lockstep. For an isolated tick-hash comparison use two runs of the same sim
match with `render_every = 0`, setting `H2_FEATURES=audio.clock-only` on one.
Compare with `h2 lockstep` as in the [sim checks](../runtime/sim.md#first-runs).

## Routing, assignment and arbitration

**Game traffic.** System Link discovers pregame hosts by UDP broadcast. Live discovers through a session ID, XNADDR/key and QoS description. Both reach the game over XNet’s UDP mapping. Keep base through base+6 reachable (game channels +0/+1/+5/+6, QoS +2). On a home router this generally requires port forwarding. The HLE provides no general NAT traversal or transport encryption. Existing public/friends listings use the observed HTTP source address. A successful XNetConnect only records an address mapping. Cloud egress and public UDP ingress can differ. Verify the advertised endpoint externally and allow an authenticated managed ingress separate from the local bind address. The existing WebSocket relay admits an open **co-op** host and one guest for messages up to 1,200 bytes. It cannot carry a retail multiplayer match today. A multiplayer fallback needs authenticated multi-peer channel mapping, datagram boundaries, expiry, limits and real route measurements. [XNet transport](live.md#client-hle-sockets-and-system-link) · [Join and address restrictions](live-custom-games.md) · [Relay](../../crates/live-server/src/relay.rs) · [Worker counterpart](../../crates/live-worker/src/relayroom.rs)

**Proposed assignment.** Current OptiMatch searches compatible sessions and lets a party form one on an empty search. Add a per-match lease on Live, shared by native and Worker backends. For compatible hopper, build, maps, skill and party size, probe each candidate region’s actual UDP endpoint from each client. Choose the lowest worst-player RTT within jitter/loss limits, breaking ties by median. The successful title QoS lookup reports a fixed 5 ms. The separate UDP echo measures elapsed time. Include relay paths in probes. Reserve seats atomically, start the host, wait until discoverable, then return its join information. Prevent parallel peer-host creation while the lease is pending. Expire stale leases, drain hosts and requeue on startup failure. [Current matchmaking](live-matchmaking.md) · [QoS implementation](../../crates/live-hle/src/qos.rs)

**Ranked admission.** Use a service identity to own and sign for the session, separate from human entrants. Bind a human’s authenticated reservation to the joined peer/player. The telemetry path assigns identities to local and rostered bot slots. An ordinary remote player can lack a verified XUID. Today rank processing requires a finished matchmade game from the recorded host, a host-registered arbitration round before start, and eligible registered participants on the host record. Preserve registration and single-use results, exclude the service account from all rank and score calculations and reconcile disconnects against the frozen roster. A signature authenticates the reporter. The host remains trusted for the result. [Ranking predicates](../../crates/live-server/src/ranks.rs) · [Arbitration](../../crates/live-server/src/arbitration.rs) · [Identity projection](../../crates/live-telemetry/src/session.rs)

## Order of experiments

1. Run the Mac control below. Advance only after a real remote client joins and its actions reach the authoritative world.
2. Decouple machine ownership/ready/start from local player reconciliation in an isolated host mode. Exercise join, load, combat, results, a second round and last-client departure. If retail requires an owner profile, identify the first failing transition. A retained spectator needs explicit evidence for every slot and gameplay consequence.
3. Reuse the sim’s gameplay render skip with network pacing. Compare normal and skipped runs on the same maps at 1, 4, 8 and 15 remote machines, including a 16-human split-screen case. Record p50/p95/p99 tick work, missed 33.3 ms deadlines, host CPU, RSS, packets/bytes and both startup intervals. Use agent `hostmetrics`, `capture`, `loads` and host OS resource counters (macOS `/usr/bin/time -l` for whole-process peak RSS and CPU).
4. Once rights and Linux support are established, pilot Fly in one region with unranked games. Its public UDP path tests the title’s existing transport without first building a multiplayer relay. Compare actual routes and hit outcomes against peer hosts. Introduce fleet assignment, then ranked arbitration after remote identity and crash/no-contest checks pass.

## Mac System Link spike

The coordinator can test and build the CLI from this branch with the usual Cargo target directory. Point `H2_BIN` at the resulting release binary, or use the installed copy if it contains this change.

```sh
cargo test --locked -p cli --bin h2 boot_cmd::tests::system_link_enables_accountless_networking_only_on_request -- --exact
cargo build --locked --release -p cli --bin h2
export H2_BIN="${CARGO_TARGET_DIR:-$PWD/target}/release/h2"
```

Use that CLI, the coordinator’s own disc, and a matching prebuilt clang image. Set `H2_DISC` to the ISO, `H2_IMAGE` to its output directory, and `H2_PROFILE_A` and `H2_PROFILE_B` to different game data directories with distinct saved profiles. Choose unused UDP bases and loopback agent ports. Run these blocks in **one macOS shell**. The image check should succeed before either process starts.

```sh
H2_BIN=${H2_BIN:-"$HOME/.cargo/bin/h2"}
export H2_OPT=2 H2_UI_WATCH=1
unset H2_SIM H2_LEVEL H2_BOOT_GFX H2_INPUT_REPLAY H2_BOOT_INPUT H2_BOTS
"$H2_BIN" build "${H2_DISC:?set the disc path}" --out "${H2_IMAGE:?set the image directory}" --tier clang --check
```

```sh
trial=$(mktemp -d "$TMPDIR/microhost.XXXXXX")
cp -cR "${H2_PROFILE_A:?set host data directory}" "$trial/host"
cp -cR "${H2_PROFILE_B:?set client data directory}" "$trial/client"
cp -cR "$H2_IMAGE" "$trial/host-image"
iface=$(route -n get default | awk '/interface:/ {print $2}')
lan=$(ipconfig getifaddr "$iface")
```

Use the Mac’s active LAN IPv4 for `lan` if its default route is a VPN. Retail treats 127.0.0.1 specially for game channels. The opposite bases fan out discovery on one machine. The cloned output isolates the host’s kernel trace. Keep this test short. Ordinary serve retains an unbounded in-memory trace and writes a TSV. Bound tracing before measuring fleet RSS.

```sh
H2_BOOT_REALTIME=1 H2_HOST_PERF=1 H2_XNET_ADDR="${lan:?set LAN IPv4}:31000" \
  H2_XNET_LOOPBACK_PEERS="${lan}:31010" "$H2_BIN" serve "$H2_DISC" --system-link \
  --out "$trial/host-image" --tier clang --data "$trial/host" \
  --no-halo1 --no-mappack --socket tcp://127.0.0.1:14720 >"$trial/host.log" 2>&1 &
host_pid=$!
host_agent() { "$H2_BIN" agent --socket tcp://127.0.0.1:14720 --timeout 100 "$@"; }
```

Once `host_agent methods` responds, start the independent windowed client. Its embedded Live instance enables the XNet HLE. Its first launch may build the web app and extract private disc art. Exclude that one-time setup from game-ready timings. Select **System Link** in the title.

```sh
H2_AGENT=tcp://127.0.0.1:14721 H2_XNET_ADDR="${lan}:31010" \
  H2_XNET_LOOPBACK_PEERS="${lan}:31000" H2_LIVE_DIR="$trial/client-live" \
  H2_LIVE_DB=file H2_CLOUD_SAVES=0 "$H2_BIN" play "$H2_DISC" --out "$H2_IMAGE" \
  --tier clang --data "$trial/client" --no-halo1 --no-mappack --no-movies \
  --live-local --gamertag LanClient >"$trial/client.log" 2>&1 &
client_pid=$!
client_agent() { "$H2_BIN" agent --socket tcp://127.0.0.1:14721 --timeout 100 "$@"; }
```

Use `host_agent ui`, `host_agent press start` and `host_agent press a` as the saved profile requires to reach the main menu. Do the same through `client_agent`. From the main menu’s first row, open the players screen (mode 2) and then Available Games (screen 13) on each.

```sh
host_agent sim lobby mode system_link
host_agent sim lobby press a '{"screen":13}'
client_agent sim lobby mode system_link
client_agent sim lobby press a '{"screen":13}'
```

On an empty host browser choose **Create New Game** (`host_agent press a` when selected). Leave the default Slayer lobby open. In the client browser select the named host with the direction pad and A (`client_agent press down`, then `client_agent press a` if Create is highlighted above it). The UI method reports the screen. Browser row order depends on available games. Check `host_agent sim session` and `client_agent sim session` for two peers before pressing **Start Game** on the host. Two one-peer lobbies mean the client also selected Create.

```sh
host_agent press a
host_agent wait '{"phase":"gameplay","map_ticks":30}' 90000
client_agent wait '{"phase":"gameplay","map_ticks":30}' 90000
host_agent sim session
client_agent sim session
host_agent sim view at player
client_agent sim view at player
client_agent stick left 0 1 1000
host_agent wait '{"ticks":60}' 10000
host_agent sim view at player
```

**Control passes** when the host reports distributed `mp_host` (world 4, host true), the client reports `mp_client` (world 5, host false), both count two peers and two players, and the named client’s live unit moves on the host. Move into open space if blocked. An unattended local player is expected on the host. If discovery fails, verify addresses, opposite UDP bases and host lobby state before investigating game start. [Agent CLI](../../crates/cli/src/agent_cmd.rs) · [Menu steps](../../crates/agent-proto/src/sim/menu.rs) · [World-mode mapping](../../crates/agent-proto/src/sim/mod.rs)

On a disposable repeat, try the ordinary local player’s leave/sign-out action through the host agent UI after the client joins pregame. Check both session roles, peer and player counts immediately afterward. A refusal, a disbanded lobby, or migration to the client identifies a failing retail transition. If the original host survives without a player, try starting the round.

**Zero-local passes** only if that experiment or the host-mode implementation in step 2 meets all of the following checks. Repeat with the service-owned lobby and identical client. Require **two peers, exactly one session player**, and a world player with a live unit and no `local_user` on the host. On the client the same named player must be local. Verify remote input, a death and respawn, round results free of a host entrant, a second successful start and the last client leaving. Add a second remote for opponent damage, scoring and team objectives. The existing API removes only simulated players. It cannot detach the real owner. There is no zero-local flag in this spike. Use the existing agent API for all checks. Add a documented, tested lobby action if recurring playerless creation/start requires it. No running game was inspected for this design. [Player reader](../../crates/agent/src/reader.rs) · [Agent API](../runtime/agent-api.md)

Stop the two processes from this shell when finished.

```sh
kill -TERM "$client_pid" "$host_pid"
wait "$client_pid" "$host_pid"
```
