# Dedicated host: data, routing, tunnel and providers

## Game data bundle

Build the bundle from your own Halo 2 disc image or an extracted DVD
directory. Keep the archive, manifest and extracted cache private. Keep
the bundle out of Git and container images.

```sh
h2 host bundle --disc "$H2_DISC" --out "$bundle_dir"
```

| Disc files | Why the host needs them |
|---|---|
| `default.xbe` | Boot reads the original executable and uses its full hash to identify the prebuilt module. |
| `image/libh2game.so`, `image/libh2game.so.stamp` | With `--image DIR`: the Linux game module the host runs and its build stamp, compiled from the disc. The container image does not carry it. |
| `maps/mainmenu.map` | The light host still boots the retail menu and loads the multiplayer lobby. |
| `maps/shared.map`, `maps/single_player_shared.map` | The map loader can resolve references into these shared caches. |
| Multiplayer `maps/*.map` | The host loads these maps for matches. The bundle selects them by their cache headers. |
| `maps/fonts/` | The retail menu reads the font tables and packed glyphs from the DVD. |
| `bots/<map>.<bsp>.bots`, `bots/<map>.<bsp>.vis` | Each multiplayer map's bot nav mesh and visibility grid, built from the disc. A sim child builds none, and a light host would otherwise build each map's when it is first played. |
| `firefight/firefight-<map>/`, `firefight/kits.json` | Each multiplayer map's Firefight kit package (its `manifest.toml` and patch) and the index naming the kit version and each map's package ID. A Firefight match plays its map's kit. |

The light host skips Bink movies. Campaign levels and dashboard media are
outside the multiplayer boot path. See [boot](../../crates/apps/boot/src/lib.rs)
and [disc fonts](../formats/fonts.md).

The bot data comes from `<data>/bots`, the directory `h2 bots nav` and the
game write. The command first builds the maps whose files are missing or
stale there, as `h2 bots nav all` does, restoring completed builds from
the shared nav cache. It then checks every map's files against the
bundled map's header and refuses a map without a current mesh and grid.
`--data DIR` names the data root (default: the one `h2 play` uses).
`--no-bots` leaves the bot data out.
[Bundle nav data](../../crates/live/host-data/src/bundle/nav.rs) · [Bot data files](../game/bots.md)

The command builds every multiplayer map's Firefight kit from the disc
with the builder `h2 firefight kit` uses, so the host's package ID is the
one each player gets from their own disc
([The kit](../game/firefight.md#the-kit)). A map the kit refuses, for
lack of headroom or drops, is named and left without one. A kit's patch
is bound to the disc map's hash, so a map with a kit comes from the disc
even when `--stripped-maps` has a copy of it. `--no-firefight` leaves the
kits out; the headless host's own build has no kit builder.

When a lease
names a Firefight match, the host places its map's package in the match's
data root (`<data>/maps/firefight-<map>`) before the game starts. It
refuses the lease when the bundle has no kit for the map, its kits are
another version than the match's, or the lease's `map_id` names another
package.
[Bundle kits](../../crates/live/host-data/src/bundle/kits.rs) · [Placing a kit](../../crates/live/host-data/src/lib.rs)

`--image DIR` adds `libh2game.so` and `libh2game.so.stamp` from `DIR`
(`deploy/host/build.sh --module` stages them in `deploy/host/stage/image`).
A deployed host needs it: without `--out` or `H2_IMAGE`, `h2 host run`
refuses a bundle with no module.
[Bundle module](../../crates/live/host-data/src/bundle/image.rs) · [The game module](dedicated-host-build.md#the-game-module)

To use server copies, make them with `h2 server-map strip ORIGINAL.map
OUTPUT.map`, keeping the original filenames, and add `--stripped-maps DIR`.
A same-named copy must match the original map header and logical length.
Maps without a replacement come from the disc.

Pass `--sim-stores DIR` to package recorded shared resources. Each store must
cover the corresponding cache header and match the disc at every recorded
byte. Playable maps and the menu stay available to native readers before boot.

Both leased child paths mount stores from the verified bundle.
[Recording and packaging](dedicated-host-build.md#package-recorded-shared-resources)

The command writes `host-data.tar.zst` and `host-data.json`. The JSON gives
the archive's `sha256` and the list of payload paths, sizes in bytes and
SHA-256 digests. Keep the two files together.

### Upload to private R2

Create an R2 bucket with public access disabled. Create an R2 API token
with Object Read & Write permission scoped to the bucket, then configure
your AWS CLI profile with its Access Key ID and Secret Access Key. Set
`R2_ACCOUNT_ID`, `R2_BUCKET`, `AWS_PROFILE` and `bundle_dir` before
running these commands with the AWS CLI and `jq`.
[Cloudflare R2 S3 setup](https://developers.cloudflare.com/r2/get-started/s3/)

```sh
R2_ENDPOINT="https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
H2_DATA_SHA=$(jq -er '.sha256' "$bundle_dir/host-data.json")
R2_BUNDLE="s3://$R2_BUCKET/host-data/$H2_DATA_SHA"

aws s3 cp "$bundle_dir/host-data.tar.zst" "$R2_BUNDLE/host-data.tar.zst" \
  --endpoint-url "$R2_ENDPOINT" --region auto
aws s3 cp "$bundle_dir/host-data.json" "$R2_BUNDLE/host-data.json" \
  --endpoint-url "$R2_ENDPOINT" --region auto
```

Create a signed GET URL for the archive and pass it as `HostBoot.data_url`
(`H2_DATA_URL` for a direct host launch). Pass the manifest's archive SHA-256
as `HostBoot.data_sha256` (`H2_DATA_SHA`). Refresh the URL before starting
hosts after it expires and keep it out of logs and shared configuration.
[Cloudflare R2 presigned URLs](https://developers.cloudflare.com/r2/api/s3/presigned-urls/)

```sh
H2_DATA_URL=$(aws s3 presign "$R2_BUNDLE/host-data.tar.zst" \
  --endpoint-url "$R2_ENDPOINT" --region auto --expires-in 3600)
export H2_DATA_URL H2_DATA_SHA
```

At startup the host streams the archive to `<cache>/<sha256>.partial`,
checks the archive digest, verifies the files during extraction and
publishes `<cache>/<sha256>/` atomically. A complete cache entry works
after the signed URL expires. `H2_HOST_CACHE` overrides the cache location.
`H2_HOST_CACHE_MAX_BYTES` sets the cache budget in uncompressed bytes
(default 8 GiB). Provisioning evicts older entries by last use and keeps
the requested entry even if it alone exceeds the budget.

When running
different bundle hashes concurrently, size the cache to hold the live
bundles or give each deployment its own cache.

## Routing, assignment and arbitration

**Game traffic.** System Link discovers pregame hosts by UDP broadcast. Live discovers through a session ID, XNADDR/key and QoS description. Both reach the game over XNet’s UDP mapping. Keep base through base+6 reachable (game channels +0/+1/+5/+6, QoS +2). On a home router this generally requires port forwarding. The HLE provides no general NAT traversal or transport encryption. Existing public/friends listings use the observed HTTP source address. A successful XNetConnect only records an address mapping. Cloud egress and public UDP ingress can differ. Verify the advertised endpoint externally and allow an authenticated managed ingress separate from the local bind address. The existing WebSocket relay admits an open **co-op** host and one guest for messages up to 1,200 bytes. It cannot carry a retail multiplayer match today. A multiplayer fallback needs authenticated multi-peer channel mapping, datagram boundaries, expiry, limits and real route measurements. [XNet transport](live-client-hle.md#client-hle-sockets-and-system-link) · [Join and address restrictions](live-custom-games.md) · [Relay](../../crates/live/live-server/src/relay.rs) · [Worker counterpart](../../crates/live/live-worker/src/relayroom.rs)

**Assignment.** [Ping-aware OptiMatch](live-matchmaking-search.md#9-ping-aware-optimatch-assignments)
can reserve an opted-in solo cohort for a registered dedicated endpoint.
Each client probes a registered endpoint over UDP. For an ad hoc host,
it measures running or configured regional UDP endpoints and can use a
ticket-authenticated WebSocket echo on its Live origin. Cloudflare routes
the echo through a running pack when available or answers in the regional
pool room.

Native Live answers the service echo itself.
A registered endpoint can use its existing account. For an ad hoc host the
matchmaker binds the lease to the assignment before the provider starts the
child.

Live attaches that assignment, including `matchmaking.host_lease`, to
the machine's session. A roster member checks the lease and the registered
session ID before joining.
[QoS implementation](../../crates/live/live-hle/src/qos.rs)

**Ranked admission.** A peer-hosted match uses the session host’s game
record and its latest unused arbitration round registered before the game
started. The existing human registration and bot roster rules apply.

When the published assignment has `host_lease`, Live binds the session to
the lease subject and checks the assignment’s members against the lease
roster. The session has its own ID and retains the matchmaking assignment
ID, leaving that ID available for a peer fallback. Only the lease subject
can write the authoritative game’s frames and event batches.

A client receiving `Queue::Join.frames_from_host` skips
its own frame upload for that game.

A dedicated match counts for Ranked when the lease holder signs a
`GameEnded` with reason `finished` in the assigned ranked hopper. Every
recorded player, including anyone who left, must have an XUID on the
lease roster. Reserved players who never joined receive no result.

A
seat without an XUID or a player outside the reservation makes the whole
result ineligible. The current guest-memory projection gives ordinary remote
humans no XUID. A real game with any such seat therefore remains unranked
until the runtime can identify that player. Gamertags and roster order
cannot establish account identity.

Live spends an eligible assignment once
and ranks the recorded entrants. The service identity has no player slot,
score, or standing.
[Ranking predicates](../../crates/live/live-server/src/ranks.rs) ·
[Arbitration](../../crates/live/live-server/src/arbitration.rs) ·
[Identity projection](../../crates/live/live-telemetry/src/session.rs)



The planned trusted source is the dedicated host. It signs telemetry
with a service identity separate from the players. Live verifies that
the assigned host signed the completed game and ties the record to the
reservation and eligible joined players before it counts for Ranked.

Preserve single-use results, reconcile disconnects against the
reserved roster and keep the service account out of ranks and scores.

## Host tunnel

Local supervisors bind the tunnel to loopback. Cloudflare sets
`H2_TUNNEL_LISTEN=0.0.0.0:31100` inside its private container.
The listener starts on its first initialised XNet call and serves one
lease.

Keep this port on the private network between the game host and
the upstream proxy. Live authenticates and admits the client first. The
host accepts the upstream-authenticated upgrade at its original
`/v1/hosts/{lease}/tunnel` path and logs the optional
`x-h2-tunnel-peer` XUID.

The client sends an empty binary hello with port 0 and peer 0. The host
replies on port 0 with an assigned nonzero peer ID and the four IPv4
octets for `10.77.0.1`. The host reserves `10.77/16` for this lease.

The low 16 bits of a client's synthetic address hold its peer ID,
beginning at `10.77.0.2`. A reconnect gets a fresh ID and address. All
client frames carry peer 0.

Host datagrams and bye replies carry the
recipient's ID. The host sends a WebSocket Ping every five seconds,
answers incoming pings, and drops connections after 15 seconds without
a Pong. Between frames the connection workers sleep until TCP data,
a queued datagram, or a keepalive deadline wakes them. Local QoS input
and shutdown also wake the client worker.

Datagrams go to XNet guest port `1000 + offset` with a source of the
client's synthetic IPv4 and the host's XNet base plus that offset. The
configured XNet IPv4 must be local to the host. The local UDP bridge
lets ordinary socket reads and `select` wake on tunnel traffic.

Sends
to synthetic peers go back over their WebSockets. System Link broadcast
fans out to all active tunnel peers and still goes out over native UDP.
The tunnel answers HLE QoS queries on offset +2 using the same advertised
data as the UDP responder.

A client offers actor channel version 1 with the `x-h2-actor-channel: 1`
upgrade header. The host acknowledges it with one version byte after the
four IPv4 bytes in its binary hello. For a peer without the offer, the
host sends the four-byte hello and ordinary XNet traffic.

Negotiated
peers receive actor packets as kind 3 with port 0 and the recipient's
ID. A client requests a fresh key with kind 4, port 0, peer 0 and an
empty payload. It sends an upstream pick with kind 5, port 0, peer 0
and a payload of its six-byte machine id and one to sixteen message
bytes; the host keeps the machine id of a peer's first pick for that
peer alone, and both relays forward the frame like any other
([Pickers](../game/rules-world-api.md#pickers)). System Link sessions use retail replication for actors.

The actor sender enqueues from the simulation tick and the client
drains its bounded queue at the tick boundary. A full send queue drops
the packet. A full client inbox drops its oldest packet.

Packet loss is
repaired by a later key. Actor packets have a separate 1,000-byte
rolling budget over 30 ticks that excludes the eight-byte tunnel header.
[Actor channel](../game/ai-networking.md#how-it-works)

The host allows 16 concurrent connections, including pending upgrades.
The payload ceiling is 1,264 bytes. Each peer can send and receive up to
240 datagrams per second, with a burst of 64 in each direction.

An
outbound queue holds 64 frames per peer. Rate limits and full queues
drop packets. Bye or WebSocket close makes the address unreachable.

The next receive skips pending packets from that peer and the next XNet
address operation drops any cached mapping.

## Playerless lobby

For the zero-local repeat of the Mac spike, start the host with
`H2_HOST_LOBBY=1` and `serve --system-link --host-light`. Leave the real
client on its usual System Link path. Use the host agent to open a custom
game and read its roster even when nobody is connected.

```sh
host_agent host.lobby.open '{"map":"lockout","variant":"slayer","max_players":2,"live":false}'
host_agent host.lobby.state '{}'
```

After a remote console joins, expect one remote player and two peers
(count peers with `sim session`). Once `ready` is true, use
`host_agent host.lobby.start '{}'`. Check for world mode 4 on the host and
5 on the client.

Verify movement, death and respawn from the host agent,
then the absence of a local player in the result. Close through
`host_agent host.lobby.close '{}'` and repeat the open and join. A second
remote on a separate console tests independent inputs.

Two split-screen
users on one remote test the distinction between a player slot and a peer.

The implementation keeps the local **peer** as host and party leader.
It blocks local profile attachment for this dedicated run. A signed-in
bot-style profile would be reconciled into a real session player by
`0x63d80 → 0x65770`.

Omitting that row from agent output would still
consume a slot and could affect teams, spawns and results. No owner
profile or player record is manufactured.

Selecting Create in the System Link browser calls `0x199df9(0,0)`
and `0x199a03(2)`. A human host's menu then opens the pregame screen.
Each frame the menu reconciler (`0x199b08`) compares its screen with the
network lifecycle.

With zero signed-in users, the pregame branch at
`0x19b284` requests a squad leave. A playerless host skips that UI update
in pregame (session type 1 or 2) while it owns the active squad. Retail
runs the update during play and postgame.

Ownership checks the squad address,
incarnation, network class and local host peer. The title screen's
controller reset also requests a leave during boot. Its call at
`0x1906d3` preserves the same owned squad.

Explicit close uses the
network teardown and releases ownership.

The next profile-specific refusal is the controller countdown at `0x19a78e`. It reads the pressing controller's local-user index,
which is −1 on the playerless host. The machine fallback requests stage
2 through `0x5a220` after the remote roster, map properties, ownership
and retail start check pass.

It calls `0x5a220` as the countdown
finisher does at `0x6e4d7`: the session in ESI, the stage in EDI, no
stack arguments, plain `ret`. The map check is the finisher's: every
peer's status above 2. `host.lobby.state` reports `ready` only then. Set `H2_HOST_LOBBY_MACHINE_START=0` to
observe the original refusal with the same roster.

Remote-view
handshakes still have to finish before the match can run.

| Retail boundary | What the coordinator should confirm |
|---|---|
| Tick `0x137fe0`, profile sign-in `0x18fb34` (callee pops 12 bytes) | Main-menu ticks service RPCs before a profile is chosen. Networking byte `[0x527330]` and lobby byte `[0x4d8ba0]` become nonzero. All four slots from `0x54e8e0` (stride `0xc70`, profile flag `0x10`, local-user index at `+4`) remain signed out through reopen and postgame. |
| System Link Create `0x199df9(0,0)`, Live preparation `0x199e2e(1)` and Create `0x59890(2,0,0)`, custom mode `0x199a03(2)` | Creation returns success in AL. Verify that the post-create squad pointer `S = [0x527364]` identifies the same session through close. Check host state 5 (`S+0x741c`), class 1 or 2 (`+0x18`), peer count (`+0x54`), and that the machine index (`+0x72d8`) equals both host and leader (`+0x40/+0x50`). Confirm discovery and a remote join on each transport. |
| Map setter `0x19a6f2` (ESI = map ID, stack = −1), default variant `0x19d220(out,type)`, variant setter `0x19a728(out)` | Check map (`S+0x4d0c`), distributed protocol 2 (`+0x4dac`), and the full `0x130`-byte rules at `+0x4db0`. Check the limit at variant `+0x78` and session `+0x4994`. Verify that bumping parameter revision `+0x4978` publishes the limit to the client and that retail refuses an extra join. |
| Start check `0x19a2ce(0)`, countdown `0x19a78e` (EAX = port 0, stack = 10, 3), machine request `0x5a220` (ESI = session, EDI = 2) | Verify start state 9 with a remote player and map status 3 or 4 for every peer (`S+0x58+peer*0x10c+0x88`). Check the fallback's calling convention and the stage change at `S+0x4980` against retail's countdown finisher `0x6e410`. Exercise both flag settings and verify remote controls move a unit. |
| Roster and phase | Verify count/mask at `S+0x1118/+0x111c`, records at `S+0x1120+slot*0x13c`, peer at `+0x0c`, and effective UTF-16 name at `+0xa8`. Cross-check lifecycle `[0x527334]` (1/2 lobby, 3 game, 4 postgame), countdown `S+0x49a8`, and game globals `[0x4e6948]` (mode `+8`, active `+0x1120`, ended `+0x1128`). Confirm world `[0x4cf77c]` reaches distributed-server mode 4 at `+8` and running state 5 at `+0x18` before gameplay is reported. |
| Leave sessions `0x593e0()` and return to menu `0x18f1c0(0)` | The target pointer `[0x52736c]` must hold no separate active matchmaking session. Check that remote clients leave, the network squad retires, the host returns to the menu, and the specific Live listing disappears. Repeat open and join on System Link. Use a fresh lease for each Live match. Exercise close during countdown and during play. |

On a leased host, `XOnlineStartup` starts machine logon before the first
retail advertisement. The HLE checks the token against Live discovery and
publishes as `host:<lease_id>`. Live allocates one dedicated session ID for
the lease.

Retail updates and deletion use the host bearer and event batches
signed with the lease key. The machine has zero signed-in users and publishes
no gamertag, friends or presence. Plain `h2 serve --system-link` still runs
with local networking only.

For a real run, compare the ID returned by `host.lobby.open` with the
registered endpoint and the roster member's session detail. Check
`matchmaking.host_lease` against the assigned lease, join from a second
console, and use `host.lobby.close` to verify withdrawal of the same ID.
Only remote players should appear in the roster.

The supervisor uses the agent API for these repeatable operations. The
guest boundaries above come from static traces. The two-console lifecycle
and the first refusal still need a game run. [Agent methods](../runtime/agent-api.md#hostlobby)

## Fly Machines provider

`FlyProvider` ([fly.rs](../../crates/live/live-server/src/hosts/fly.rs)) runs
each lease as one Fly Machine: created when matchmaking forms the lobby,
destroyed with the match. On the native Live server, set
`LIVE_HOST_PROVIDER=fly` to select Fly or leave it unset to use `local`.
The native provider calls the Machines API through its reqwest adapter.

The Worker selects Cloudflare Containers through `HostRoom`. A Worker
deployment that selects Fly would need a platform-fetch implementation of
`FlyHttp`.

Setup, from the environment:

| Variable | Meaning |
|---|---|
| `LIVE_FLY_TOKEN` | Fly API token, sent as the bearer on every call. Required. |
| `LIVE_FLY_APP` | The Fly app the machines live in. Required. |
| `LIVE_FLY_IMAGE` | The host image each machine runs (the `linux/amd64` image from `deploy/host`). Required. |
| `LIVE_FLY_REGION` | Where a lease with no region runs. Default `iad`. |
| `LIVE_FLY_PUBLIC_IP` | The app's dedicated IPv4 advertised to clients. Required for UDP leases. |
| `LIVE_FLY_PORTS` | The UDP range lease windows are picked from, as `30000-44000`. Default `30000-44000`. |
| `LIVE_FLY_API` | The API base, so a test can point the provider at a fake. Default `https://api.fly.io`. |

**UDP needs the app's dedicated IPv4.** Allocate one
(`fly ips allocate-v4`, $2/month) and the machine's host process must
bind `fly-global-services`. The provider sets `H2_XNET_ADDR` to that
name and the window's base port, `H2_BASE_PORT` to the base port, and
`H2_PUBLIC_ADDR` to the app's dedicated IPv4. The supervisor resolves
the bind name before passing a numeric address to the game.

Each machine claims a 7-port window
(`base..base+6`, the game's channels and QoS) as one UDP service per
port with external matching internal, chosen deterministically from the
lease id and aligned inside `LIVE_FLY_PORTS`, so a restarted lease comes
back on the same ports and two concurrent leases either take disjoint
windows or the second create is refused by Fly - an error the lifecycle
falls back from. A Tunnel lease claims no ports: its datagrams arrive
through the Live tunnel instead.

The machine: guest `performance` 1×2048 MiB for a Small lease, 2×4096
for Large; `auto_destroy` so a host that ends by itself (match over,
last client gone) removes its machine; `restart: no` so a crashed host
is not restarted - the lifecycle reads the failure and falls back. The
machine env also carries `H2_LEASE`, `H2_HOST_LEASE_JSON`, `H2_LIVE`,
`H2_TOKEN`, `H2_DATA_URL` and `H2_DATA_SHA`. Its metadata names
the lease for `fly machines status`. `start` returns the machine id as
the provider ref - the host registers itself and answers the lease when
it is up; nothing waits. `stop` deletes the machine with force; a
machine already gone is stopped. `alive` reads the machine state
(booting and running are alive, destroyed or missing is not).

Costs follow the [Fly row above](dedicated-host.md#where-the-game-could-run): the class's
per-second rate while the machine exists, the dedicated IPv4, storage
and egress.
