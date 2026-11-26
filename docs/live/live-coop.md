# Campaign co-op over Live (design; phase 0 slice, the phase 2 probe and Co-op over Live built)

Status: Partly built. The base (phase 0), the probe of the retail path and Co-op over Live run on tests and in the game; join in progress needs its first game run.

Retail Halo 2 plays campaign co-op only in split screen on one console. This page lays out the two ways to play it over Live and the retail machinery they rest on. The owner's choices are in [section 6](#6-decisions), the phase 0 crate in [section 8](#8-the-coop-crate), the probe of the retail path in [section 9](#9-probe-phase-2) and Co-op over Live itself in [section 10](#10-co-op-over-live-built). More than two players are in [section 13](#13-more-than-two-players), join in progress in [section 14](#14-join-in-progress) and latency and prediction in [section 15](#15-latency-and-prediction).

In short:

- **The retail game carries a networked lockstep path built for campaign.** It has a synchronous client and server mode, four synchronous messages, a client catch-up loop and an out-of-sync latch. Two checks stop a campaign session (more than one machine, and never joinable), and no lobby screen drives it.
- **Lockstep below the game** runs two-player split-screen co-op on both machines, offline as far as the guest knows. The hosts exchange only pad input and each shows its own player's view. It rests on the runtime's machine-level determinism and needs no retail network code.
- **Recommendation:** build the shared base (two-pad tape, simulation hash, pad-exchange transport), probe the retail path with two instances, and finish whichever holds.
- **It builds on the settings and hook layer**: co-op options are `Def`s, the settings peers must share are those with `View` or `Sim` scope, the lobby is `ui` screens, and hooks chain through `Dispatch::override_handler` (section 5).

Evidence labels, as in [theater.md](../game/theater.md): **[local]** read from this repository or from a static reading of the owner's retail XBE; **[community]** public knowledge; **[guess]** inference, to check before building on it.

## 1. What the retail engine has

Detail: see the [detail note](notes/2026-11-26-live-coop-detail.md), "1. What the retail engine has".

### 1.1 The simulation modes

The game mode is the byte at game options `+4` (`G+0x0c` in [live-gamestate.md](live-gamestate.md)): none, local, sync-client, sync-server, dist-client, dist-server (0 to 5). The simulation world's mode is the dword at `+8` of the simulation globals (`0x004CF77C`). The jump table at `0x00068da8` (world builder `0x00068c00`) maps game 1 to 5 to world 1, 3, 2, 5, 4.

The game-start routine `0x0006ec80` reads the session protocol (`S+0x4DAC`: 0 local, 1 synchronous, 2 distributed) and writes game mode 1 for protocol 0, `2 + is_host` for 1 and `4 + is_host` for 2. `0x00064060` picks the protocol: a session with no variant gets synchronous, and `0x00161c90` maps a variant's engine to distributed except for the two deprecated engines. `variants::record::protocol` answers the map natively ([game-types-without-carriers.md](../game/game-types-without-carriers.md#no-retail-engine-object)). Multiplayer is distributed; the synchronous protocol is the campaign one.

### 1.2 Campaign through a session

When the session difficulty is not `0xFFFF`, `0x0006ec80` writes scenario type 1, the difficulty at `+0x12a` and "more than one player" at `+0x12c`, and skips the variant, as the menu's own campaign start does ([level-boot.md](../runtime/level-boot.md)). `0x0019a76d` sets the difficulty on a session and clears the variant; `0x0019a728` does the reverse.

### 1.3 The synchronous machinery

Four messages are registered by `0x000b2b30`: synchronous-update (max `0x4048`), -actions (`0x180`), -join (4) and -gamestate (8 to `0xFFFF`). `0x4048` is the per-tick record `game_tick` (`0x00137fe0`) builds, so every tick already runs from it.

| step | address | what |
|---|---|---|
| build | `0x000682e0`, `0x00069260` | stamps update number, tick and shared seed; servers compare and latch a mismatch at globals `+0x2c` |
| before | `0x000683a0` | applies per-player entries and events |
| after | `0x000684e0` | world mode 2 calls `0x00069880`; clients advance the update number |
| client catch-up | `0x000698e0` | checks the update is next, then loops `game_tick` while `0x00069300` reports ticks pending |
| ending | `0x000680f0`, `0x00068750` | the latch ends the simulation (flag `0x004CF772`) |

The latch is set at `0x00068342` (mismatch) and `0x0006992b` (out of order). The reason tables at `0x0046715c` and `0x00467268` have no code reference.

### 1.4 What blocks it

| block | where |
|---|---|
| a campaign start is refused with more than one peer or two players | `0x0006ee9b`, `0x0006eea5` |
| a campaign session is never joinable | `0x00063e00` returns false at `0x00063e7a` |
| no lobby flow | the campaign menus build a local session [guess] |

### 1.5 What is unknown

Whether the path ever ran a mission; whether campaign simulation is deterministic across machines with different local players; cutscenes, checkpoints, reverts and level transitions on a client; what a client does while waiting; and the join transfer, at least 4 MiB before compression ([checkpoints.md](../runtime/checkpoints.md)).

## 2. Lockstep below the game

### 2.1 The idea

Both peers boot the same image and run retail two-player split-screen co-op with the guest offline. Pad port 0 is the host player and port 1 the guest player on both machines. The hosts agree per frame on every pad read ([high-framerate.md](../game/high-framerate.md) section 1), each presents only its own view, and a hash exchanged every tick proves the machines still match. Built blocks: the logical clock, `xapi::tap` (`InputTap`, `Recorder`, `Replayer`), `crates/extensions/lockstep` (per-tick RAM hash, `compare`, `lockstep::synthetic`), four input ports ([input.md](../input/input.md)), Live sessions and mailboxes ([live-api.md](live-api.md)). Not yet shown: the two-run control on the retail game and split screen in this runtime ([high-resolution.md](../graphics/high-resolution.md)).

### 2.2 Input exchange and latency

Each host samples its pad on wall time, assigns it to frame now + *D* and sends it. The guest starts frame *f* only when both ports' packets for *f* are in hand, so a stall changes nothing in the run. Packets repeat the last several frames, so one lost datagram costs nothing. *D* defaults to 2 frames (67 ms, holds to about 100 ms round trip) and changes only at an agreed frame. Rollback needs machine snapshots and is not in the plan ([theater.md](../game/theater.md) phase 7). Delay table: see the [detail note](notes/2026-11-26-live-coop-detail.md), "2.2 Input exchange and latency".

### 2.3 Start-state agreement

The handshake compares and refuses on any difference in: build (image stamp, [BOOT.md](../runtime/BOOT.md)), features (the `exact` fingerprint of [feature-engine.md](../engine/feature-engine.md) "Determinism"), disc (title id, XBE hash, map hashes), epoch (`Config::epoch_100ns`, chosen by the host), user data and cache partitions ([cache-slots.md](../runtime/cache-slots.md)). The host's profile and saves are packed and the guest boots from them in a **session data directory**. `View` and `Sim` settings are the host's and locked ("locked: co-op"); `Host` settings stay per machine. The guest also keeps its own progress at session end; copying a save under the guest's profile needs the save format read first.

### 2.4 Join and start flow

The host creates a Live session with a co-op flag ([live-api.md](live-api.md) "Sessions") and invites a friend through the mailbox; the shell handles the accepted invite. The hosts connect over direct UDP ([live-matchmaking.md](live-matchmaking.md) 3.5 and 3.6), with the Live server's WebSocket as relay. After the handshake (2.3) both boot from the title screen, then each reports the hash every tick. The guest never learns it is online.

### 2.5 Showing one player's view

Both guests render both split views; only presentation differs, and it is `Host` class, after the device ([high-resolution.md](../graphics/high-resolution.md)). The presenter tags each draw by viewport (`0x00014bc0` calls `SetViewport`): top, bottom or full screen. Own view is scaled to the window with full-screen draws over it. Aspect goes in three steps: letterboxed strip, an anamorphic `View`-class change using the field-of-view hook at `0x00187130` ([settings-menu.md](../game/settings-menu.md)), then a full-resolution re-render. The other view can be skipped on the view backend only. Rumble goes to the local pad.

### 2.6 Pause, menus, checkpoints, reverts

All of these are guest behaviour driven by pad input, so they stay in step. Either Start pauses the shared game. Checkpoints are written by the same guest code at the same tick, so storage completion must be on the logical clock (2.9). Host overlays send neutral input while open.

### 2.7 Disconnect and rejoin

A short loss stalls and shows "waiting for <gamertag>". After a timeout the remaining player carries on alone (the missing pad reads neutral from an agreed frame) or saves and quits. Rejoin restarts both from the last checkpoint. A desync stops both, writes both traces and names the tick and MiB (`lockstep::compare`).

### 2.8 Audio

Both guests mix retail split-screen audio and each machine plays it. Output must use logical pacing ([dsound.md](../audio/dsound.md) "Pacing"). Voice chat is out of scope.

### 2.9 Determinism risks

The recorder boundaries ([feature-engine.md](../engine/feature-engine.md) "Recording and replay", [theater.md](../game/theater.md) "What the recorder captures") are covered by the offline guest, the handshake and locked settings. The extra risks, because the peers are different computers:

| source | what to do |
|---|---|
| GPU readbacks | measure which occur in campaign; exclude pixel memory the CPU never branches on; make one peer authoritative for what it reads ([high-resolution.md](../graphics/high-resolution.md)) |
| pixel counts (`GetVisibilityTestResult`, `0x00020bb0`) | deterministic wait policy, count from the host peer ([render-thread.md](../graphics/render-thread.md)) |
| frame overruns | caught by the whole-RAM hash and clock |
| storage timing | completion on logical time ([storage.md](../runtime/storage.md)) |
| movies, audio | guest-side timing only ([bink-hle.md](../audio/bink-hle.md)) |
| host architecture | floats are software-exact ([FLOAT.md](../lifter/FLOAT.md), [simd.md](../lifter/simd.md)); start same-architecture |
| our overrides | audit `View` and `Sim` overrides that read host state |
| telemetry and the tick hook | one chain per point; telemetry off in co-op |

The whole-RAM hash is the strict test: any difference is a bug.

## 3. Other options

Streaming player two's view needs an encoder and transport we lack and is the fallback that cannot desync. Campaign as a distributed multiplayer game is not viable, since campaign AI and scripts have no replication there. A host-authoritative state copy costs tens of KB per tick; lockstep is better while determinism holds.

## 4. Recommendation and plan

### Which path

Section 2 is the one we can promise; section 1 gives each player a full screen and a normal HUD if it works, and per-pool hashes of the game-state block name the pool that parted. So: build the base, probe section 1, then commit. The comparison table is in the [detail note](notes/2026-11-26-live-coop-detail.md), "Which path".

### Phases

| # | what |
|---|---|
| 0 | base, tests only: two-port schedule tap, pad-exchange protocol, simulated link, simulation hash, settings-layer pieces |
| 1 | controls on the real game: split-screen co-op replays to an identical trace twice, then on a second machine |
| 2 | probe the retail path: two instances, a Rust entry for `0x0006ec80` without the peer-count refusal, difficulty through `0x0019a76d` |
| 3a | if the probe holds: finish the retail path (lobby, joinable predicate, desync fixes) |
| 3b | otherwise lockstep below the game: session data directory, handshake, transport, stall, letterboxed presenter |
| 4b | anamorphic view, direct two-player level boot, carry-on-alone, copy progress to the guest |

Phases 0 and 1 serve Theater too ([theater.md](../game/theater.md)). Effort estimates: see the [detail note](notes/2026-11-26-live-coop-detail.md), "Phases".

### The first slice (phase 0), provable without the game

Six tests on `lockstep::synthetic::Toy` extended to two ports: identical traces from the same tape and a one-bit change diverging at that tick; stalls under a simulated link that match a zero-latency trace; a missing packet fails rather than reading a local device; packet numbers from the agreed stream; a hash mismatch stopping both sides in one exchange; a delay change at an agreed frame. Crates: `xapi`, `lockstep` and a new small protocol crate.

### What needs the game, and only the owner can run

Split-screen co-op at all and its frame rate; the two-run and two-machine controls (phase 1); the phase 2 experiment; and feel (67 ms delay, half-screen view).

## 5. How it plugs into the settings and hook layer

Written against the `settings-slice` branch: `crates/formats/settings` (`Def`, `Store`, `Setting<T>`), `boot::ui`, `boot::setting_defs` and `Dispatch::override_handler` in `runtime/src/dispatch.rs`, with the chaining pattern of `level_boot::install_activate`. Co-op is the first feature built on it.

### 5.1 Settings, and which must match

Co-op options are `Def`s in `boot::setting_defs`:

| key | scope | apply |
|---|---|---|
| `coop.input_delay` (1 to 8, plus `auto`) | `Host` | `Live` |
| `coop.transport` (auto, direct, relay) | `Host` | next session |
| `coop.view` (mine, split) | `Host` | `Live` |
| `coop.aspect` (strip, anamorphic) | `View` | `NextMap` |
| `coop.difficulty` | `Sim` | next session |
| `coop.tick_hash` (ram, simulation) | `View` | next session |

Lockstep below the game (3b) needs every `View` and `Sim` value equal (the `exact` fingerprint); the retail path (3a) needs only `Sim` (the `sim` fingerprint). The guest peer calls `Store::force(key, host_value, "co-op session")` for each key in the handshake's list; a forced value is locked and the file is not written.

### 5.2 Screens and rows

The co-op screens are `ui::Screen`s reached by a row added with `ui::extend` (`Row::open("coop")`, "Co-op over Live"): `coop` (difficulty, level, delay, aspect, invite, status, start), `coop.friends` and `coop.join`. Start boots the lockstep machine (2.4) or makes the session calls of 1.2. Layout sketch: see the [detail note](notes/2026-11-26-live-coop-detail.md), "5.2 Screens and rows".

### 5.3 Hooks

| hook | where |
|---|---|
| pad boundary | `InputTap` on `xapi` (`set_input_tap`), wired by `boot::lockstep_glue`; the schedule tap chains to the recorder |
| tick observer | `lockstep::install_tick_observer` on `game_tick` `0x00137fe0`; chains like `install_activate` |
| start request | `level_boot` hook on `REQUEST_MAINMENU`; `Request` gains two players and the co-op flag (4b) |
| split-view field of view | the `fov:camera` override on `PLAYER_FOV` `0x00187130` reads `coop.aspect` |
| own-view presenter | a view stage after the device, not a dispatch hook |
| session start, joinable (retail path) | overrides on `0x0006ec80` and `0x00063e00` via `register_override` |
| UI rows | list hooks in `layout_glue` through `ui::Model` |

### 5.4 Gaps in the layer that co-op would need filled

Ten gaps: no store fingerprint (and `Dispatch` cannot list overrides), no `unforce`, row presses are guest input in the lockstep machine (`Model` needs a session filter and the host overlay), no `Action` rows, static `Kind::Choice`, no retail campaign list variant, no text values, chaining by hand, no "next session" apply scope, and no restart-into-session request. Items 1 to 3 belong in phase 0 or 1, 4 to 6 and 10 in phase 3, 8 in the feature-engine registry work. Detail: see the [detail note](notes/2026-11-26-live-coop-detail.md), "5.4 Gaps in the layer that co-op would need filled".

## 6. Decisions

The owner's answers to the questions this page ended with:

1. **Order.** Probe the retail synchronous path first, after phase 0.
   Lockstep below the game is the fallback.
2. **Relay.** The Live server may relay co-op traffic when a direct
   connection fails. Built: section 8.
3. **Saves.** The guest keeps the checkpoint and campaign progress too (2.3).

Defaults taken until someone says otherwise:

4. **Two players.** Both paths stop at two.
5. **Input delay** of 2 frames by default, per machine.
6. **Same-architecture peers first.** Cross-architecture play waits for
   traces that prove it.
7. **Telemetry off in co-op** until the tick observer chains (5.3).
8. **View compromises** for the fallback path stay as proposed in 2.5:
   letterboxed strip first, anamorphic later; shared audio and pause.

Still open: whether `settings-slice` lands before the co-op options are
built. Section 5 assumes it does; the slice in section 8 does not.

## 7. Research points

Open questions about retail code: the session fields `S+0x54`, `S+0x40`,
`S+0x50`; the out-of-sync latch reason (simulation globals `+0x2c`); the
sync-client's `0x00069880` and `0x000860b0`; the update record's event
kinds; whether the campaign session is the system link lobby's object; the
split-screen viewport table; GPU readbacks in a level; the campaign save
formats (2.3). Full list: [detail note](notes/2026-11-26-live-coop-detail.md),
"7. Research points".

## 8. The coop crate

Built on tests only; nothing here ran in the game. It is the part of phase 0
that does not need the settings layer.

| module (`crates/live/coop`) | role |
|---|---|
| `wire` | one datagram (version 3, at most 1159 bytes): acks, repeated unacknowledged pads and tick hashes, the Forge lane and the beat |
| `link` | `Link`, `UdpLink` (XNet base port + 4, `NetworkConfig::coop_endpoint`), `UdpPort`, `Fallback` (direct, plus relay after a silence), `sim_pair` |
| `relay` | `RelayLink`, the relay as a `Link` on its own thread |
| `schedule` | the two-port pad schedule of 2.2; delay is per machine |
| `peer` | `begin_frame` (`Hold::Waiting`), `report_hash` (`Hold::Desync`), the Forge lane ends |
| `tap` | `ScheduleTap`, an `xapi::tap::InputTap` for ports 0 and 1 |

`crates/extensions/lockstep` holds `SimRegion` and `hash_sim` (the
`0x3fe000` bytes behind `0x004e6080`). The Live server's
`GET /v1/sessions/{id}/relay` ([live-api.md](live-api.md), "Co-op relay")
is a WebSocket for the two members of a `coop` or `forge` session; it caps
messages at 1200 bytes, rate-limits each socket and never reads the bytes.
The Forge lane is described in [forge-design.md](../game/forge-design.md),
"F5 in detail".

Tests: `cargo test -p coop -p lockstep -p live-server`; the main files are
`coop/tests/peers.rs`, `coop/tests/toy.rs` and
`live-server/tests/relay.rs`. Test-by-test claims, and what is left for the
rest of phase 0 (runner wiring, handshake, delay choice, hole punching, the
settings-layer gaps of 5.4): [detail note](notes/2026-11-26-live-coop-detail.md),
"8. The coop crate".

## 9. Probe (phase 2)

Built on tests; **no game was built or run for it**. The probe tries the
retail synchronous path of section 1 with two instances over System Link
and writes enough on each machine to say how far it got and where the two
parted. It is off unless `H2_COOP_PROBE=1`. Code:
`crates/extensions/game/src/coop_probe.rs`,
`crates/extensions/lockstep/src/pools.rs`, `h2 coop-probe diff`
(`crates/apps/cli/src/coop_probe_cmd.rs`).

### 9.1 What it changes

All through the feature plan (feature `coop-probe`, points in
`engine::points`); nothing is patched by hand.

| retail block | hook |
|---|---|
| campaign start with more than one peer refused (`0x6eea5`) | `before` on session start `0x6ec80` shows peer count `S+0x54` as 1; `before` on `OPTIONS_NEW` `0x138110` and `after` on `0x6ec80` restore it |
| game-start parameters set peer limit `S+0x4990` to 1 (`0x6e720`) | `after` sets it back to 2 |
| no lobby for a networked campaign | `before` on `game_tick` calls the menu setters `0x19a6f2` (level) and `0x19a76d` (difficulty), up to three tries |
| campaign session never joinable (`0x63e7a`) | `before` on `0x63e00` answers as retail would without that clause below two players |

The host takes its level from `H2_COOP_PROBE_MAP` (default `01a_tutorial`)
and `H2_COOP_PROBE_DIFFICULTY` (default `normal`). An address that already
has an override outside the plan makes the probe print `coop-probe:
refused: ...` and stay off. Rationale for each hook:
[detail note](notes/2026-11-26-live-coop-detail.md), "9.1 What it changes".

### 9.2 What it writes

Every stderr line starts `coop-probe:`: session state, lobby setup, session
start and bypass, game start (`simulation mode sync-server (2)` on the host,
`sync-client (3)` on the guest), stalls, events, and `OUT OF SYNC` when the
retail latch (simulation globals `+0x2c`) fires. Each game also writes
`<data>/coop-probe/<time>-<pid>-<role>.hashes`: one line per tick and pool,
pool `*` being the whole game-state block, flushed every 300 ticks.
`h2 coop-probe diff <host.hashes> <guest.hashes>` prints the first tick and
pools that differ and exits 0 when every common tick agrees. The line
table and file format: [detail note](notes/2026-11-26-live-coop-detail.md),
"9.2 What it writes".

### 9.3 Running it

Two `h2 play` instances (one Mac with two data directories and XNet base
ports at least seven apart, or two Macs of the same architecture) against
one Live server (`h2 live serve`), each with `H2_COOP_PROBE=1`,
`H2_XNET_ADDR` and, on one Mac, `H2_XNET_LOOPBACK_PEERS`. Use the LAN
address, not `127.0.0.1` ([live.md](live.md), "Client HLE: sockets and
System Link"). Two Macs need UDP 3074 to 3080 and an HTTPS server
([live-hosting.md](live-hosting.md)). Commands, menu steps and what to send
back: [detail note](notes/2026-11-26-live-coop-detail.md), "9.3 Running it".

### 9.4 What might go wrong, and how you will see it

Failures and their signs: the lobby setters refuse or never take, the lobby
setup never starts, the guest starts multiplayer, the start is refused,
`stall` lines that go on, `OUT OF SYNC`, a crash. Hashing costs 4 MiB a tick
on the game thread, only with the probe on. The probe writes nothing to the
disc image or a profile. Details: [detail note](notes/2026-11-26-live-coop-detail.md),
"9.4 What might go wrong, and how you will see it".

### 9.5 What is tested

`cargo test -p boot --lib coop_probe`, `-p lockstep --lib pools`,
`-p cli coop_probe`: the variables, the peer bypass and joinable
conditions, the host setup, the plan (`H2_FEATURES=-coop-probe` turns it
off), the pool cuts and the diff. Only the game can show whether the lobby
ticks, whether the setters take in a System Link lobby and whether the
bypassed start runs.

## 10. Co-op over Live (built)

Built on tests, on the owner's decision to assume the probe's approach (the
retail synchronous campaign path, two consoles, one player each) works and
to test it all at the end. **No game was built or run for it.** If the
retail path breaks, only the start part (10.3) is redone.

### 10.1 What a player sees

The host picks **Co-op Campaign** in the Xbox Live menu
([live-custom-games.md](live-custom-games.md) §5), which opens the co-op
screen: Level, Difficulty, Invite a Friend, Status, Ping, Start. Start is
enabled with exactly two consoles, at most two players and a level with
campaign and map ids. The guest, who joined by invite, friends list or the
custom games browser (**Y** filters: [Browser controls](live-custom-games.md#filter-games)),
is armed when the server reports a `coop` session and gets a join screen
(`H2_COOP_JOIN_SCREEN=0` leaves that to the Co-op Campaign row).

The lobby's status line for co-op (string id `0x0e0001ec`, at `0x2500ab`)
reads ours through `coop_live::line_for` on any console in a networked
squad session; split screen and a Live-off system link lobby keep the
game's text. Co-op text uses guest slots under ids `0x20ffd000` up. The
host advertises its campaign skulls and a guest's campaign start applies
exactly those (`skulls::coop_start`, `coop_live::ours_of`,
`coop_live::host_campaign_skulls`). Rows, status lines and the id-range
reasoning: [detail note](notes/2026-11-26-live-coop-detail.md),
"10.1 What a player sees".

### 10.2 Session plumbing

- **Advertising.** `Hle::set_coop(campaign, skulls)` makes the lobby
  `flags: "coop"` with a `live_proto::session::Campaign`; public becomes
  friends-only (`coop_visibility`); leaving sends `flags: "custom"`.
- **Presence.** `activity: "Co-op: <level>"` while armed
  (`live_proto::valid_activity`).
- **Invites.** `Hle::invite_to_lobby`; the guest accepts with the retail flow.
- **The guest's read.** `Hle::joined` returns the joined session.
- **Server and web.** `GET /v1/sessions?flags=coop`, finished games at
  `GET /v1/coop/games`, with visibility enforced for games, telemetry, events
  and presence ([live-api.md](live-api.md), "Co-op games").
- **Ping** (10.6): the host's `ping_ms` on the session.

### 10.3 The hooks (the probe as a feature)

The hooks of 9.1 are also the feature `coop-live`, on the plan whenever Live
is on (`H2_FEATURES=-coop-live` removes them) and armed only for a co-op
session (`coop_probe::Arm`): the host with its level, a guest with none.
Disarmed, every hook lets retail run and nothing is logged. Armed, they are
the probe's, with the hash files in `<data>/coop-probe/`. `H2_COOP_PROBE=1`
forces the probe on for the whole run.

### 10.4 Your test (two instances, or two Macs)

Same build on both, one Live server, the two accounts friends
(`h2 live account create`, `h2 live friend add`). The host and guest run
`h2 play` as in 9.3 without `H2_COOP_PROBE`. A ten-step checklist with
stderr lines to look for covers the menu, the invite, the ping, Start, the
lobby text, the probe's lines and skulls. The first thing to report is
`LOBBY_SET_MAP returned 0 (refused)`. Checklist and commands:
[detail note](notes/2026-11-26-live-coop-detail.md), "10.4 Your test".

### 10.5 What is tested

`cargo test -p boot --lib coop_live`, the custom games tests,
`cargo test -p live-hle --test server
a_coop_lobby_advertises_its_campaign_and_the_invited_guest_reads_it`,
`crates/live/live-server/tests/coop_sessions.rs` and
`web/test/coop.test.ts`. Not tested: whether the screens open and look right
over the Live lobby, whether the setters take on a Live session, and
everything from the session start on (section 9).

### 10.6 Ping

The HLE's QoS responder on XNet base port + 2 also answers a co-op ping
(`H2PNG001` / `H2PNR001`). The guest pings the host every 2 s; the host
takes the slowest report of the last 6 s and advertises `ping_ms` when it
moves (`live_hle::ping_moved`, `Hle::guest_ping`). It measures the path the
game's own traffic takes, with no new port and no server in the middle.
Wire detail: [detail note](notes/2026-11-26-live-coop-detail.md), "10.6 Ping".

## 11. The retail lobby (co-op over Live, as the game's own co-op)

Section 10's list screens were ours. The Live menu's Co-op Campaign row
opens the game's own pregame lobby in co-op mode instead: its own level and
difficulty screens, its player slots and gamertags, its ready state and its
Start Game. Live adds two things only: the second player can be a Live
friend (the lobby's Y opens the friends list and invites), and the guest
lands in the same lobby, showing the host's level and difficulty. The list
screens stay behind `H2_COOP_LIST_UI=1`.

### 11.1 Static trace

The Live menu's Custom Game row (row 2, `0x231001`) creates the Live squad
session (`0x59890`) and sets the lobby mode (`0x199a03`). The hooks turn that
into co-op. Traces: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "11.1 Static trace".

| Symbol | Address | Role |
|---|---|---|
| Live menu row chosen | `0x230f92` | Custom Game branch calls `0x199e2e(1)`, `0x59890(2,0,0)`, `0x199a03(2)` |
| `LOBBY_MODE_SET` | `0x199a03` | mode 0 is co-op (campaign setup `0x199a92`), mode 2 is custom game |
| `LOBBY_START_STATE` | `0x19a2ce` | 9 ready, 8 waiting, 10 countdown, 7 co-op/teams check failed |
| `LOBBY_COUNTDOWN`, `LOBBY_ALL_READY` | `0x64a60`, `0x592f0` | feed the start state's tail (`0x19a4c5`) |
| co-op check | `0x19a3ed`..`0x19a41c` | exactly two players, both local (`0x19ab77`) |
| status line | `0x24ffc9` | `coop_takes_two` (`0x0e0001ec`) for state 7 |

`pregame_lobby` (screen 14) has pane 0 for multiplayer and pane 1 for co-op
(level, difficulty, description). The level and difficulty screens are
`load_level_lobby` and `difficulty_lobby`.

### 11.2 What the hooks do

- `ROW_CHOSEN` (layout glue): the Co-op Campaign row arms co-op as host
  (`Lobby::open_retail`), asks for co-op mode, rewrites the press record's
  row to 2 and runs the original handler. The game then creates its Live
  squad session and opens its lobby as for Custom Game.
- `LOBBY_MODE_SET`, "co-op mode for the co-op row": within 5 s of that
  press, the next mode 2 becomes mode 0, once. Every other row drops the
  request.
- `LOBBY_START_STATE`, "co-op over 2 to N consoles": while co-op is armed,
  a 7 is answered with the tail's own result (10, 9 or 8) when the
  session is a networked campaign of 2 to `coop.max_players` consoles
  with one player each (section 12). The session start that follows is the
  probe's synchronous path (section 9).
- The pinger thread's tick (`Lobby::sync_retail`): the host advertises
  the level and difficulty the lobby holds, so the guest's join reads
  them.

Ping has no place in the lobby and stays on the web.

### 11.3 What needs a game run, and the lines to look for

Four lines mark the path: the row opening the lobby in co-op mode, hosting,
the mode 2 to 0 change, and `coop-live: the lobby holds <level> on
<difficulty>; advertised`, then the start state answer. The checklist in
12.3 reads the same lines. Steps: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "11.3 What needs a game
run, and the lines to look for".

### 11.4 Previews

`the_retail_co_op_lobby_on_live_drawn_from_the_disc` (in
`crates/extensions/game/src/coop_live/tests.rs`) draws the lobby as host and as
guest, plus the level and difficulty screens, with
`ui_kit::preview::Disc::render_retail`. It needs `H2_DISC` and writes to
`H2_UI_PREVIEW_DIR`; never into git. Run-time texts are stand-ins.

## 12. One command on one Mac: `h2 coop test`

`h2 coop test` (`crates/apps/cli/src/coop_test_cmd.rs`) does what section
10.4 does by hand and reads the logs back:

```sh
h2 coop test --disc disc/halo2.iso --auto
```

It starts a Live server (`127.0.0.1:8075`, `--server-port`; file store under
`--root`), makes an account per game (Alpha hosts, Bravo joins; `--players N`
up to 16 through the NATO alphabet; past 2 the host gets
`H2_COOP_MAX_PLAYERS=N`, section 13), opens each window with `h2 play`
(`--data <root>/<gamertag>/data --live-server ... --gamertag ...`), waits for
the host's main menu (`session.wait {"phase":"menu","map_ticks":60}`) and
prints what to press next. Every line goes to
`<root>/logs/<time>/<gamertag>.log`; the co-op lines go to `coop.log` there
as `[+SECONDS] GAMERTAG | line`. When the last window closes, or a game
stops while others run, it prints the checklist (12.3). `--fresh` forgets the
server and accounts; `--dry-run` prints the commands and starts nothing.
Details: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "12. One command on one
Mac".

### 12.1 What `--auto` does

The host's Live code invites the guests to its advertised lobby as soon as
it has one and they are online, and each guest's Live code accepts the
host's invitation as soon as it arrives, through the same requests as the
friends list's Invite and Accept (`crates/live/live-hle/src/auto.rs`;
`H2_COOP_AUTO_INVITE=<gamertags>` on the host, `H2_COOP_AUTO_ACCEPT=<host>`
on a guest). Once per lobby and per invitation; they print `coop-auto:
invited ...` and `coop-auto: accepting ...`.

The lobby's session is the newest advertisement carrying a co-op campaign,
else the one advertised last (`Sessions::lobby_id`). A re-advertised lobby
gets a new id, and a guest accepts only an invitation newer than its last.
The title reads the accepted invitation through
`XOnlineFriendsGetAcceptedGameInvite` (reader `0x0022f3b5`, a delegate on
the main menu, screen 9), which runs on an event, so `--auto` has the
guests join from the friends list. Session lookup is `0x000900e0`
(FindFromID `0x00090121`); the join is `0x0006f4b0`. For a web
[join link](live-custom-games.md#join-in-halo-2), the host's lobby pinger
in `crates/extensions/game/src/coop_live.rs` publishes the observed player
count and available seats on the current Live advertisement; a new console
needs a free peer and a free slot within `coop.max_players`.

Menus are yours unless `--drive` is set (12.6): Xbox Live on every window,
then on the host Co-op Campaign, a level and difficulty, and Start Game.
Traces (the reader, QoS, clocks, run evidence): see the
[detail note](notes/2026-11-26-live-coop-detail.md), "12.1 What `--auto`
does".

#### "UNABLE TO JOIN ... network problems"

The dialog (`unable_to_join` `0x0e00169d`; `error_join_qos_failure`
`0x160016e2`, `error_join_data_decode_failure` `0x1e0016e3`) comes from the
join's QoS probe: `XNetQosLookup` (`0x0007b558`) to the host's XNADDR with
the session's XNKID. The HLE answers on the host's XNet base + 2
(`crates/live/live-hle/src/qos.rs`, `qos::answer`) for a session the host
listens for (`XNetQosListen`, `0x00073243`); an answer without lobby data
fails the decode. A probe tries three times, 100 ms each. Peers are told
apart by IP and base port ([live.md](live.md), XNetXnAddrToInAddr). The
games' logs and `coop.log` carry one line per step:

| Line | Step |
|---|---|
| `live: session <id> advertised for <ip>:<port>` / `deleted` | the host's lobby on the server |
| `live: xnet: key for session <id> registered` / `unregistered` | XNetRegisterKey on either side |
| `live: qos: listening for session <id> on <ip>:<port+2>: on, N bytes of data` | the host's QosListen, on each change |
| `live: qos: <guest> asked about session <id>, which this console isn't listening for (listening: ...)` | the host, once per unknown session: the usual cause |
| `live: join: find session <id>: <host> hosts it at <ip>:<port> ...` (or `no such session`, `the session is <state>`) | the guest's FindFromID |
| `live: join: qos lookup: <ip>:<port+2> session <id>: contacted, N bytes of data` (or `no answer in 3 tries ...`, `nothing on that port`) | the guest's QoS probe; a host behind the guest's own NAT shows its LAN address first, then the public one if the LAN address did not answer |
| `live: join: <ip>:<port> for session <id>: address made` / `connecting to ...` | XNetXnAddrToInAddr and XNetConnect |

Run evidence and fixes: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "UNABLE TO JOIN ...
network problems".

### 12.2 Two games on one Mac

Checked in the code, not by a run:

| Shared thing | How the games keep apart |
|---|---|
| XNet ports | Base 31000 and 31010 (`--port`, 10 up per game) on `--ip` (this Mac's LAN address, else loopback). Each game binds base to base+6 (title channels +0, +1, +5, +6; QoS and the co-op ping +2; the co-op channel +4); `NetworkConfig::validate` wants bases 7 apart on one IP. `H2_XNET_LOOPBACK_PEERS` names the others for discovery. |
| Data | `--data <root>/<gamertag>/data` each: profiles, settings, saves. |
| Live account | `H2_LIVE_DIR` each: `accounts.json` and `play-accounts.json`. Without it two games write the same `play-accounts.json`. |
| The game image | One `--out` (default `$TMPDIR/h2boot`). Only the host builds it: the guests open after the host's `live: playing as`. Set `H2_OPT` as for `h2 play`, or the image rebuilds. |
| Agent socket | `H2_AGENT` each: `~/Library/Caches/h2recomp/coop-<gamertag>.sock`. `h2 agent --socket <path> status` reads each game. |
| Pipeline cache | The pipeline store is made for several processes (`d3d::pipeline_store`). |
| Input | Each window takes the keyboard while focused. Pads go to the host (`--pad guest`: to the first guest); the others get `--no-gamepads`. |
| Sound | The guests run `--no-audio`. |
| Map pack, Halo 1 | Not installed (`--no-mappack --no-halo1`) unless `--extras`. |

Windows are titled by game and role (`h2 play --title`) and tiled across the
main screen, host leftmost (`h2 play --tile I/N`, `app::tile`).

### 12.3 The checklist

`h2 coop verdict <coop.log> [--auto]` prints it again
(`crates/apps/cli/src/coop_verdict.rs`). Each step is seen when every game it
is for printed its line:

| # | Step | Game | Line |
|---|---|---|---|
| 1 | signed in to Live | each | `live: playing as <tag>` |
| 2 | the disc's campaign levels read | each | `coop-live: N campaign levels (` |
| 3 | Co-op Campaign opens the game's own lobby | host | `coop-live: Live menu row ... in co-op mode` |
| 4 | hosting in that lobby | host | `coop-live: hosting a co-op campaign in the game's own lobby` |
| 5 | the lobby is in co-op mode | host | `... lobby: mode 2 (custom game) set as 0 (co-op campaign)` |
| 6 | level and difficulty advertised | host | `coop-live: the lobby holds ... ; advertised` |
| 7 | the host invited the guest | host | `coop-auto: invited` (only with `--auto`) |
| 8 | the guest accepted | guests | `coop-auto: accepting` (only with `--auto`) |
| 9 | the guest is in the host's session | guests | `coop-live: guest: joined` |
| 10 | ping | each | `coop-live: ping N ms` |
| 11 | the co-op check passed for two consoles | host | `coop-live: lobby start state 7 ... answered` |
| 12 | the session start counted the peers | host | `session start: campaign with` |
| 13 | the session started | host | `session start returned 1` |
| 14 | the host simulates | host | `simulation mode sync-server` |
| 15 | the guest follows | guests | `simulation mode sync-client` |

A step not seen comes with the likely reason, most often a press nobody
made. The report ends with the first step not seen and lines that look like
trouble (`ping: no answer`, `returned 0 (refused)`, `could not invite`,
`could not accept`, `did not open`, panics). Send back `coop.log`, and the
two whole logs if it stopped early. Reasons per step: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "12.3 The checklist".

### 12.4 What is tested

`cargo test -p cli --bin h2 coop` and `cargo test -p live-hle` cover the
arguments, accounts, log merging, the checklist and its reasons, the
auto invite and accept, stopped-game handling and the QoS answers. The
host's log filter uses `crates/apps/cli/src/coop_verdict/host-at-menu.log`.
List: see the [detail note](notes/2026-11-26-live-coop-detail.md), "12.4
What is tested".

### 12.5 Driving the host's menus (the plan; built as 12.6)

`--auto` stops at the menus. `--input-timeline`
(`input::backend::scripted::Timeline`, docs/input/input.md) and
`H2_BOOT_INPUT` press blind. The agent API's `input.pad` and `ui` domain
(docs/runtime/agent-api.md) give timed pads and the open screen; the plan
was a pad queue, a screen reader and a script that waits on what the screen
says. Built as 12.6. Plan and risks: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "12.5 Driving the
host's menus".

### 12.6 `--drive`: nobody presses

`h2 coop test --players 3 --auto --late --drive` runs the whole test
without a press:

1. **Input.** `input.pad` on every game's agent socket (tap at XAPI, merged
   over the live pad). One short step per press: the button for 150 ms and
   at least two pad reads.
2. **Screens.** `H2_UI_WATCH=1` (set by `--drive`) prints `ui: screen 0x09
   (window 5, slot 4)` when the screen taking player one's input changes;
   `ui.screen` answers it (`h2 agent --socket <path> ui`). The Live menu
   line adds the restored row (`0x510a18`); a dialog reads as `dialog`.
   `boot::ui_watch` holds the layout. [local] Read statically (dispatcher
   `0x147dbe`, push `0x147f6d`, channel test `0x1473b6`, delivery
   `0x23515d`). [guess] Not seen in a run. The focused row is not read; the
   driver counts its own moves.
3. **The script** (`crates/apps/cli/src/coop_drive.rs`). One driver per game
   reads its `ui: screen` lines and every game's co-op lines, and asks for
   one press at a time, at least 0.7 s apart, once a screen has held 1.5 s.
   A step has a 15 s timeout and 4 tries, then `coop-drive: <tag>: stuck:
   ...; press on its window`. Each press goes into coop.log as `coop-drive:
   Alpha: press a on screen 0xba row 3 (Co-op Campaign)`.

| Game | Path |
|---|---|
| every game | Main menu, DOWN, A (Xbox Live); the players' screen `0x1e` in mode 3, A opens the Live menu. A wrong mode makes the driver press B and retry |
| host | Live menu (`0xba`): row 3 (Co-op Campaign), A; wait `coop-live: hosting`; wait every `coop-live: guest: joined`; A on Start Game (lobby row 0, `0x250fb3` → `0x251050`) |
| guest | B to the main menu, where the invitation reader runs (12.1); nudge after `coop-auto: accepting`; fall back to the Custom Game Browser (row 7) after 20 s; done at `coop-live: guest: joined` |
| late guest | As a guest, but the browser fallback waits until 120 s after the invitation |

Tests: `cargo test -p cli --bin h2 coop_drive` and `-p boot --lib ui_watch`
(menu model with and without the invitation reader, a late guest, cursor
correction, boot presses, replays of `coop_drive/run-092343.log` and
`coop_drive/run-023244.log`). First-run observations, menu rows and open
questions: see the [detail note](notes/2026-11-26-live-coop-detail.md),
"12.6 `--drive`: nobody presses".

## 13. More than two players

Co-op over Live (sections 10 and 11) takes two consoles. This section is
what holds a campaign at two, what is lifted, and what is left. Built on
tests only; **no game was built or run for it**. Evidence as at the top:
**[local]** read in `h2 disasm`, counted on the disc, or in this repo;
**[guess]** inferred.

### 13.1 Where the game assumes two

The engine runs 16 players already. What holds at two is the session start,
the data and the scripts:

| what | where | bound | evidence |
|---|---|---|---|
| the host's game-start limits | `0x6e720`: campaign player limit 2 (`0x6e8aa`), peer limit 1 (`0x6e8eb`) | 2 | [local] |
| session start | `0x6ec80`: more than two players refuses (`0x6ee9b`), more than one peer refuses (`0x6eea5`) | 2 | [local] |
| lobby start check | `0x19a2ce`: exactly 2, both local | 2 on one console | [local] |
| join check | `0x5af70` | the limits (16) | [local] |
| players table | `PLAYERS` (`0x4e8c24`): 16 datums of `0x21c` | 16, 4 local | [local] |
| starting locations | 00a: 1; every other campaign level: 2 | **2** | [local], counted on the disc |
| character | `0x14bc80`, `0x14f040`: players 3+ are the same Chief or Arbiter as P1 | none | [local] |
| the scripts' players | `player0` and `player1` in all 15 levels, `player2`/`player3` in none | **2** | [local] tokens; [guess] `playerN` is `(list_get (players) N)` |
| bandwidth | 16 × `0x5c` × 30 ticks ≈ 44 KB/s per client from the host if the encoder `0xb2710` sends raw | | [guess] |

The full table (spawn, respawn, game over, checkpoint, switch volumes,
teleports, synchronous update) is in the
[detail note](notes/2026-11-26-live-coop-detail.md), "13.1 Where the game
assumes two".

### 13.2 What the code does for N players

**Stage 1a** (`coop.max_players`, `H2_COOP_MAX_PLAYERS`, 2 to 16, default
2, the host's value counts; `coop_probe`, `coop_live`):

- the session start's window shows one peer and, for more than two
  players, two players until `OPTIONS_NEW`, then puts both back;
- after `0x6e720`, a campaign's peer and player limits go to N;
- the joinable answer, the host setup and the lobby's co-op check take 2
  to N consoles with one player each; the blocker and the lobby line say
  "up to N";
- a guest takes the limit as 16: the host's lobby already held the start.

**Stage 1b** (feature `coop-players`, `crates/extensions/game/src/coop_players.rs`,
on the co-op arm, so off outside a co-op session; `H2_FEATURES=-coop-players`):

- *Spawn.* A first spawn of a player of index 2 or more, when every spot is
  taken, picks spot `index % count` and places the player next to player
  0's unit (player 1's when player 0 is in a vehicle).
- *Stragglers.* A teleport of the scripts' `player0`/`player1`, a BSP switch
  or a cinematic's end marks the squad; at the next player update, players
  of index 2 or more on foot and more than 8 world units from the anchor are
  placed next to it with `0x14e970`.
- The hooks read only the game, never a per-console setting, so every
  console runs them alike, which lockstep needs.

Hook points: `PLAYER_UPDATE` `0x14d3a0`, `PLAYER_SPAWN` `0x14f2e0`,
`PLAYER_SPAWN_LOCATION` `0x14ef00`, `PLAYER_PLACE_NEAR` `0x14e970`,
`SCRIPT_TELEPORT` `0x29ffb0`; global `SCENARIO` `0x4e0350`.

Tests: `cargo test -p boot --lib coop` (fallback spot, table reads, anchor,
stragglers, spawn and gathering entries, nothing moves with two players).
List: see the [detail note](notes/2026-11-26-live-coop-detail.md), "13.2
What the code does for N players".

### 13.3 What needs a game run

Three consoles (or instances), `H2_COOP_MAX_PLAYERS=3` on the host,
section 10.4's setup with a third account. Expect the `coop-live`,
`coop-probe` and `coop-players` lines of 10.4 plus the three-player ones
(lobby start state 9 for 3 consoles, peer and player limits 1 and 2 raised
to 3, player 2 spawned and placed next to the anchor, stragglers brought
over, checkpoint checks leaving out player 2). The exact lines are in the
[detail note](notes/2026-11-26-live-coop-detail.md), "13.3 What needs a
game run".

Then check: the third player appears next to player 0, a dead third player
respawns (retail's co-op respawn), all three stay in sync (`OUT OF SYNC`,
`h2 coop-probe diff` pairwise), and the 03a and 05a opening cutscenes leave
nobody behind. Players 3+ are the same character as player 1.

### 13.4 Staged plan

1. **2 to 4.** Run 13.3 on 01a, 03a, 05a. `h2 coop test --players`
   (section 12) can allow up to `coop.max_players`.
2. **4, done properly.** Generate third and fourth flags for the
   `player0`/`player1` cutscene teleports and the `bridge_`/`boss_` flags
   (04b, 05a, 05b, 08b), or teleport players 3+ through `0x14e970`; the
   checkpoint rule (13.5); a per-player character (`player+0x88`, read by
   `0x14f040`).
3. **8 to 16.** Measure the update's encoded size at 16 (encoder
   `0xb2710`); the host's upstream sets the cap; the slowest console gates
   every tick; AI targeting and respawn anchors in big levels are unread.

### 13.5 Risks and open items

- **Checkpoints with more players (built).** `0x226190` (`game_safe_to_save`,
  point `SAVE_SAFE`) refuses a save while any player is dead or any unit is
  unsafe (`0x14df40`, `0x14deb0`). `coop-players` counts players 0 and 1
  always, and players 2+ only while alive and within 8 units of the anchor;
  the others are taken out of the in-use bits for the check and put back
  after. With two players nothing changes. Tests: `coop_players::tests`.
  Details: see the [detail note](notes/2026-11-26-live-coop-detail.md),
  "13.5 Risks and open items".
- **The scripts know two players.** The leash does not put players 3+ into
  cutscene positions, and `(volume_test_objects ... (players))` counts all.
- **Two start spots.** If `0x14e970` refuses, players 3+ stay overlapped
  with player 0.
- **Respawn anchor** is the last non-busy player in table order.
- **Unread:** AI targeting with many players, stragglers outside a new BSP,
  cinematic player handling, the update encoder.

## 14. Join in progress

A friend can join a running co-op campaign without anyone reverting. The
host holds at a tick boundary, snapshots the state and sends it compressed
to the newcomer, who has loaded the same level, restores it and picks up the
synchronous stream on the next tick. A hold of a second or two is
acceptable. Behind `coop.join_in_progress` (default on,
`H2_COOP_JOIN_IN_PROGRESS`, docs/game/settings-menu.md).

The retail join is wired (14.9) with a `coop-jip:` line per stage. Our own
protocol (14.7) is the fallback.

### 14.1 What the retail engine already has

Retail has a synchronous join that sends the game state, never used in a
shipped mode. Join, state and updates reach a client in one ordered stream
(`0x000865d0`, messages `0x29`, `0x2a`, `0x27`). `0x000862e0` starts a join
and compresses the `0x3fe000` byte state in `0x400` byte pieces; `0x0014c630`
applies a player set change carried in the update, which is how the
newcomer's player enters on the same tick everywhere. Addresses and the
view state machine: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "14.1 What the retail
engine already has".

### 14.2 Two ways to build it

1. **The retail join.** Lift the session's "not joinable while in game",
   and let the server's view machine send the state.
2. **Our own join** (14.7): the host holds, snapshots with the checkpoint
   machinery and sends over the co-op transport (`crates/live/coop`).

Decided (14.9): 1 is wired; 2 is the fallback. Either way the player enters
through the update's player-set change.

### 14.3 What the snapshot covers, and what the newcomer rebuilds

The `0x3fe000` block holds objects, units, players, AI, physics, effects,
script state and game time. The newcomer rebuilds the level and BSP (loaded
before asking), sound, HLE state, network session tables and the local
player mapping. It must be a sync-client at update T with an empty queue
before the restore, so the restore does not undo its session. Table: see
the [detail note](notes/2026-11-26-live-coop-detail.md), "14.3".

### 14.4 The flow

Newcomer sends Request; the host holds at the next tick boundary, snapshots
the end of T (before the player exists), sends Accept and windowed chunks;
the newcomer restores and sends Ready; update T+1 carries the player-set
change. The snapshot predates the player, so a failed join has nothing to
undo. Diagram: see the [detail note](notes/2026-11-26-live-coop-detail.md),
"14.4 The flow".

### 14.5 Transfer format and size

`coop::join` datagrams (magic `h2cj`): 1024 byte chunks, a window of 64,
cumulative acks, an FNV-1a 64 checksum. Compression is the `snapshot`
crate's codec. The newcomer accepts up to twice the raw block
(`MAX_STATE`). Sizing: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "14.5".

### 14.6 Failure paths

The host refuses without holding when the setting is off, it is not in a
level, no slot is free or another join runs. Otherwise every failure
(5 s without an answer, 3 s without progress, a 15 s hold, a checksum or
restore failure) aborts, the host resumes without the player and the
newcomer returns to the lobby. A lost ready is repeated until the first
update. Log lines: `coop-jip: joined in progress at tick T as player N`,
`coop-jip: back to the lobby: <why>`, and on the host
`coop-jip: admitted player N at tick T`. Table: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "14.6".

### 14.7 The join code and its tests

- `crates/live/coop/src/join.rs`: messages, `Host` and `Joiner` state
  machines; pure (time and state bytes are passed in). Tests:
  `coop::join::tests`.
- `coop.join_in_progress` (`boot::setting_defs`), default on, live.
- `h2 coop test --players 3 --auto --late`: the last guest's accept waits
  `H2_COOP_AUTO_ACCEPT_DELAY` (120 s) (`live_hle::auto`); the merged log
  names it `# late` and the checklist adds a "join in progress" part.

The fallback's four steps: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "14.7".

### 14.8 What needs a game run

See 14.9.4: the run and the lines to expect, stage by stage.

### 14.9 The retail join, traced and wired (second pass)

Static reading with `h2 disasm`, no run. The client side is complete, the
server side sends the state, and a pending player set reaches the update
stream by itself.

**14.9.1 The client side is there.** Dispatch `0x00093aa0` routes `0x27` to
`0x00094990`, `0x28` to `0x000949d0`, `0x29` to `0x00094a10`, `0x2a` to
`0x00094a50`; `0x0006aa20` decompresses the state straight into the
game-state block, for exactly `0x3fe000` bytes. This replaces our own
snapshot and restore on the retail path.

**14.9.2 The server side.** `0x0006a860` runs the four callbacks at
`0x0046e320`; `0x000862e0` compresses and queues and calls the callback at
`0x0046e330`. The update's `+0x403C` flag runs the same hooks on every
console.

**14.9.3 Who adds the player.** `0x00068350` copies the pending player set
from the simulation world (`*0x004cf780`) into the update record; the
session's player array sets the pending byte; `0x0014c630` adds the player
on every console on the same update.

**What blocks it today.** The host's join check `0x00062fe0` answers 3
unless session state `S+0x741c` is 5 (the lobby); states 6 to 8 refuse. The
view machine's gates (`*0x004ced48`, `*0x004ced54`, `*0x004ced50`) are set to
8, 3 and 2000 by the network setup.

Walkthroughs and tables: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "14.9".

**14.9.4 What is wired** (`crates/extensions/game/src/coop_jip.rs`, feature
`coop-jip`, on the co-op over Live arm):

- **The lift.** On the host of a co-op campaign with
  `coop.join_in_progress` on and a free slot (`coop.max_players`), a join
  check made while the game runs (state 6 to 8) sees state 5 for the length
  of retail's call, and the real state goes back after it. Off:
  `H2_COOP_JOIN_IN_PROGRESS=0`, or `H2_COOP_JIP_LIFT=0` for this part.
- **A line per stage** (off with `H2_COOP_JIP_LOG=0`), on the points
  `SESSION_JOIN_CHECK`, `SIM_JOIN_SEND_STATE`, `SIM_JOIN_RECEIVE`,
  `SIM_STATE_RECEIVED` and `SIM_UPDATE_APPLY` (all optional).
- `H2_FEATURES=-coop-jip` drops all of it.
- Tests: `coop_jip::tests` (debug).

**The user's run:** `h2 coop test --players 3 --auto --late`. Charlie
accepts 120 s after its invitation, so have the level running by then. Watch
the `coop-jip:` lines on Alpha (lift, send, queue, admit) and Charlie (join
message, state restored, joined at tick T as player 3), then play a minute
for the out-of-sync latch (1.3). If the retail path stops at stages 2 to 4,
the fallback is 14.7's protocol. Stage-by-stage lines: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "14.9.4".

## 15. Latency, and prediction

Co-op over Live runs the retail synchronous path: one console builds every
tick's update from everyone's actions and the others run only the updates
they receive, so a guest's own simulation action takes a round trip.
First-person look has a local camera path (15.4). This section is a static
reading (`h2 disasm`) and our own code; the `coop-live: latency` line (15.3)
puts numbers on it in a run.

### 15.1 Which console does what

The update builder (world mode 2, the sync-server) builds each tick's record
in `0x00069880` and sends it at once with `0x000860b0`. A follower (world
mode 3) sends its actions at once with `0x00085fc0` (message `0x28`) and
simulates updates as they arrive (`0x000698e0`). The update record is
bit-packed (`0x000b2710`).

**Converted Halo 1 cinematic ticks (built).** `game_skip_ticks` requests up
to 15 extra updates, dispatched by the authority in world modes 1 and 2 and
consumed by mode 3 through the catch-up path. Host-level wire version 2
carries the per-frame tick counts (`Peer::begin_cinematic_frame`); the
peer test covers loss, reordering and conflicting guest requests.

Address detail: see the [detail note](notes/2026-11-26-live-coop-detail.md),
"15.1 Which console does what".

### 15.2 The budget of one press

A press waits for the pad read, the wire and two 30 Hz polls on the way to
the host and back, then one display frame. Split screen pays about 35 ms;
the synchronous path adds about 35 ms on a wired LAN (up to 70) plus the
round trip over the internet. None of the added part is ours, and there is
no fixed input delay. The ten-hop table: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "15.2".

### 15.3 What was changed (ours, tested)

- **The latency line** (`boot::coop_latency`, feature `coop-latency`; off
  with `H2_COOP_LATENCY=0`). Once a second, `coop-live: latency guest:` or
  `coop-live: latency host:`, with update rate, input-to-update and wait
  (min/median/max) and traffic. Points: `SIM_ACTIONS_SEND` (`0x85fc0`),
  `SIM_ACTIONS_RECEIVE` (`0x85f50`), `SIM_UPDATE_SEND` (`0x860b0`),
  `SIM_CATCH_UP` (`0x698e0`).
- **Traffic counters** (`live_hle::traffic`): datagrams, bytes and the
  largest datagram; a datagram near 1400 bytes fragments.
- **A kept receive buffer** per thread for `recv`/`recvfrom`; no switch.
- **`H2_XNET_LOWDELAY=1`** marks the UDP sockets IP_TOS `0xb8` (DSCP EF).
  Opt-in, best effort.

### 15.4 Prediction

**Retail's distributed modes** (multiplayer) predict locally and replicate
objects and events from the host; the synchronous path has none. Mode 4
calls `0x00069a00` and `0x00069b50`; the image names a "predicted data"
pool. The correction is not traced.

**The synchronous path has none.** A follower's ticks run only from
received updates.

**Could campaign run distributed?** Not cheaply: AI, scripts, cinematics and
device machines are never replicated, so it needs a new replication layer
and gives up the synchronous path's exactness.

**Prediction on the synchronous path.** Look prediction is in progress.

1. **Guest look prediction (implemented, game measurement pending).**
   `coop.look_prediction` defaults to true; `H2_COOP_LOOK_PREDICTION=0`
   disables it. Only an armed guest (world mode 3) draws predicted look.
   The outgoing `0x28` action (encoder `0xb2790`; `0x1870a0` and `0x186ab0`
   write the angles) is matched against the applied update (`0x683a0`) to
   find which sends the host has confirmed. `boot::coop_look` adds only the
   rotation the drawn camera lacks, through `render_game_view`. Small
   errors settle over two draws; 30 degrees or more snap; death, vehicles,
   cinematics, teleports and zoom changes discard pending look. Tests:
   `cargo test -p boot --lib coop_look`. Matching rules, expiry and limits:
   see the [detail note](notes/2026-11-26-live-coop-detail.md), "15.4".
2. **Full prediction (rollback).** Run ahead on the guest's input and roll
   back to each update. Months, high risk; not before 1 is measured.

### 15.5 What needs a game run

Measure before changing the input schedule: a prediction-off baseline on
the same map, repeated with controlled delay and jitter; compare prediction
on and off. Ping-aware host selection is being added to SBMM:
[Matchmaking](live-matchmaking.md#9-ping-aware-optimatch-assignments).

`h2 coop test --players 2 --auto`. Read the two `coop-live: latency` lines
and the `coop-live: ping` line once a second, then repeat with
`H2_XNET_LOWDELAY=1` on Wi-Fi. For look prediction, record a turn with
`input.record {"name":"coop-look"}`, read `render.coop_look` on both
sockets, repeat with `H2_COOP_LOOK_PREDICTION=0` and compare with
`h2 coop-probe diff` as in §9. The recorded-stick regression is
`cargo test -p boot --lib recorded_stick_turns -- --ignored`, with
`H2_COOP_LOOK_CAPTURE` and `H2_COOP_LOOK_INPUT`. Full checklist: see the
[detail note](notes/2026-11-26-live-coop-detail.md), "15.5".
