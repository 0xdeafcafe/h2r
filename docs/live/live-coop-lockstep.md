# Live co-op: the retail engine, lockstep and settings

Lockstep below the game; 5. How it plugs into the settings and hook layer.

## 1. What the retail engine has

### 1.1 The simulation modes

[local] There are two mode numberings, and they differ. The **game
mode** is the byte in game options at `+4` (`G+0x0c` in
[live-gamestate.md](live-gamestate.md)); the options check `0x00138180`
accepts 0 to 5, and the name table at `0x00468c64` names it: none, local,
sync-client, sync-server, dist-client, dist-server (0 to 5). The
**simulation world's mode** is the dword at `+8` of the simulation globals
(pointer at `0x004CF77C`, set by the initialisation callback
`0x00067e30`). The new-map callback `0x00067ee0` reads the game mode and
builds the simulation world (`0x00068c00`), which maps it through the jump
table at `0x00068da8`: game 1, 2, 3, 4, 5 become world 1, 3, 2, 5, 4. So in
the world's numbering 2 is the synchronous server, 3 the synchronous
client, 4 the distributed server and 5 the distributed client (the name
table does not index it).

Only 0 and 1 mean the same in both. Everything
below says "world mode" or "game mode" where it matters.

[local] The session's game-start routine `0x0006ec80` computes it. With
`S` the session:

| input | where | meaning |
|---|---|---|
| protocol | `S+0x4DAC` | 0 local, 1 synchronous, 2 distributed; anything else refuses the start |
| is host | `S+0x72D8 == S+0x40` | host peer index against the local one |
| campaign difficulty | `S+0x5DD0` (word) | `0xFFFF` means "not campaign" |
| peer count | `S+0x54` | [guess] the word before the 16-entry peer array at `S+0x58` (stride `0x10C`) |
| player count | `S+0x1118` | directly after that array; compared with the session's player limit at `0x00063e4f` |

The game mode written to options `+4` is 1 for protocol 0, `2 + is_host`
for protocol 1 and `4 + is_host` for protocol 2. So a synchronous session
makes the host a sync-server (game mode 3, world mode 2) and everyone else a
sync-client (game mode 2, world mode 3).

[local] The protocol is chosen in `0x00064060`, which the lobby's "set
variant" call `0x0019a728` reaches: a session of type 0 (local) gets
protocol 0; a session with **no variant gets protocol 1, synchronous**;
otherwise `0x00161c90` maps the variant's game engine to a protocol. That
map gives distributed for every engine that shipped and synchronous for
the two deprecated ones (Race, Headhunter) and for anything out of range.
With the `rules` feature, `variants::record::protocol` answers the map
natively: the same table, but Headhunter's index, the script index,
plays distributed
([game-types-without-carriers.md](../game/game-types-without-carriers.md#no-retail-engine-object)).

A new session also starts at protocol 1 (`0x00059d58`). The protocol and
the difficulty are replicated session parameters: setters at `0x0005b8e0`
and `0x0005bc10` (host only), applied from a parameters update near
`0x0005dd0a` and `0x0005ddd0`.

This confirms what [theater.md](../game/theater.md) had from community sources:
multiplayer is distributed, and the synchronous protocol is the campaign
one.

### 1.2 Campaign through a session

[local] `0x0006ec80` has a campaign branch. When the difficulty parameter
is not `0xFFFF` it writes scenario type 1 (campaign) at options `+0`, the
difficulty at `+0x12a`, "more than one player" at `+0x12c` from the
session's player count, and skips the variant. The fields match the menu's
own campaign start ([level-boot.md](../runtime/level-boot.md), "How the game starts
its first map").

The lobby side exists too: `0x0019a76d` sets the
difficulty on the session (through `0x00064100`) and then clears the
variant, which is what selects the synchronous protocol; `0x0019a728`
sets a variant and resets the difficulty to "not campaign". The callers of
`0x0019a76d` are in UI code (`0x00252a1b`, `0x002bba4b`, `0x002c9d74`) and
`0x00199b00`.

[guess] So the retail campaign lobby already drives a network session
object, of the local type, and a networked campaign session was a matter
of using a non-local session type with the same calls.

### 1.3 The synchronous machinery

[local] Message descriptors are registered by the routine at `0x000b2b30`
(a name, size bounds, and encode, decode and compare functions each):

| message | max bytes | functions | reading |
|---|---|---|---|
| synchronous-update | `0x4048` | `0x000b2710`, `0x000b2730`, `0x000b2770` | one tick's update record, whole |
| synchronous-actions | `0x180` | `0x000b2790`, `0x000b2860`, `0x000b2920` | [guess] a client's player actions for the server |
| synchronous-join | 4 | `0x000b2980`, `0x000b29a0` | join handshake |
| synchronous-gamestate | 8 to `0xFFFF` | `0x000b29e0`, `0x000b2a90` | [guess] the game-state block sent in pieces to a joining client |

[local] `0x4048` is also the size of the per-tick record that `game_tick`
(`0x00137fe0`) builds on its stack and passes to its envelope
(`0x000682e0` build, `0x000683a0` before, `0x000684e0` after;
[live-gamestate.md](live-gamestate.md) calls it the simulation state).
So the synchronous update is not a side structure: every tick of every
game, local ones included, already runs from this record. Reading the
envelope:

- **Build, `0x000682e0` and `0x00069260`.** Outside sync-server mode the
  record is stamped with the update number (globals `+0x28`), the tick
  count (`game_time+8`) and the engine's shared random seed
  (`*0x004e7408`, [engine/hs_vm.md](../engine/hs_vm.md)) at `+0x4040` and
  `+0x4044`. In the two server modes those three are compared with the
  local values, and a mismatch sets the byte at globals `+0x2c`.
- **Before, `0x000683a0`.** Applies the record: per-player entries for
  the players in the mask at `+0x5CC` (16 entries of `0x7C` bytes from
  `+0x610`), a list of up to `+0xE38` events of `0xC8` bytes each with
  five kinds (`0x00084f30` to `0x000854c0`), and a flag at `+0x403C`
  that, on a server, calls the game-state table at `0x0046E320` (also
  referenced from the game-state code at `0x00123e23`). [guess] That flag
  is "save the game state on this tick", which would put checkpoints
  inside the synchronous stream.
- **After, `0x000684e0`.** World mode 2 (the sync-server) calls
  `0x00069880`, world mode 4 (the dist-server) calls two other routines, and
  the clients (world 3 and 5) only advance the update number. (This step
  reads the world's mode; it was once written with the game's names, which
  put the roles the other way round.)
- **Client catch-up, `0x000698e0`.** Takes a received update, checks it is
  the next one (else sets `+0x2c`), and then calls `game_tick` in a loop
  while `0x00069300` reports ticks pending. A sync-client is driven by the
  updates it receives.
- **Ending, `0x000680f0` and `0x00068750`.** The per-frame simulation
  update ends the simulation when `+0x2c` is set on a server
  (`0x00068750` latches "ended" at `0x004CF772`).

[local] Two reason tables hold "out-of-sync": the session end reasons at
`0x0046715c` (stopped, reset-failed, fatal-error, lost-connection,
out-of-sync, film-ended) and a per-view reason table at `0x00467268`
(none, disconnected, out-of-sync, failed-to-join, blocking, catchup-fail,
ended, mode-error, player-error, then three replication reasons and
no-longer-authority). Neither table has a code reference, so they are for
debug text. **The producer I can point to is the latch at globals
`+0x2c`**, set at `0x00068342` (update number, tick or seed mismatch) and
`0x0006992b` (update out of order). I did not trace the latch to the code
that stores the reason value. "Blocking" and "catchup-fail" in the view
table fit the synchronous join: a joiner blocks the game while it receives
the game state, then catches up.

### 1.4 What blocks it

| block | where | evidence |
|---|---|---|
| A campaign start is refused when the session has more than one peer (and when it has more than two players) | `0x0006ee9b` (players), `0x0006eea5` (peers), both only when the difficulty is set | [local]; "peer count" for `S+0x54` is the [guess] above |
| A campaign session is never joinable | the predicate `0x00063e00` returns false at `0x00063e7a` when the difficulty is set (reached from `0x0019a015`) | [local] |
| No lobby flow | the campaign menus build a local session; nothing offers a system link or Live campaign lobby | [guess] from the callers above; the UI was not read |
| Player limit of two | same check | [local] |

Everything else needed is present as code: both simulation modes, the
messages, the join and catch-up path, the desync latch, campaign options
from a session.

### 1.5 What is unknown

- **Whether it works.** [guess] The code is there because the engine is
  shared with what became Halo 3's networked co-op, which is described as
  synchronous lockstep [community, from memory]. Halo 2 shipped without
  it, so the path may never have run a full mission, and nobody fixed its
  bugs. The gate at `0x0006eea5` reads as a deliberate switch-off.
- **Whether the campaign simulation is deterministic between two
  machines with different local players.** In split screen both players
  are local on one machine, so retail never had to keep simulation state
  independent of the local camera, sound and rendering. Anything in the
  game-state block that depends on them (object activation by distance to
  a local camera, effects that draw from the shared random seed in render
  code) desyncs two peers. The seed comparison every tick would catch it
  at once. Our floats are bit-exact by construction ([FLOAT.md](../lifter/FLOAT.md)),
  so host hardware is not a source of drift; the game's own logic is.
- **Cutscenes, checkpoints, reverts, respawn rules and level transitions**
  on a client. Unread.
- **What a client does while waiting for an update** (stall the whole
  frame, or keep rendering), and how much input delay the server adds.
  Unread.
- **The join path**: the game-state transfer is at least 4 MiB before
  compression ([checkpoints.md](../runtime/checkpoints.md)).

## 2. Lockstep below the game

### 2.1 The idea

Both peers boot the same image with the same start state and run retail
two-player split-screen co-op. The guest is offline: no Live HLE, no XNet,
cable unplugged, as in a plain `h2 play`. Pad port 0 is the host player,
port 1 is the guest player, on both machines.

The two hosts agree, frame
by frame, on what every pad read returns, so both machines run the same
instructions on the same schedule ([high-framerate.md](../game/high-framerate.md)
section 1, "In our runtime"). Each host presents only its own player's
view. A hash exchanged every tick proves the two machines are still the
same.

What exists [local]: the logical clock and cooperative scheduler;
`xapi::tap` (an `InputTap` on every `XInputGetState`, with `Recorder` and
`Replayer` keyed by read index); `crates/extensions/lockstep` (a hash of in-use guest
RAM per tick, traces, `compare` naming the first differing record and
MiB, and a toy guest in `lockstep::synthetic`); four input ports
([input.md](../input/input.md)); Live sessions and game-invite mailboxes
([live-api.md](live-api.md)); host UDP sockets in `live-hle`.

What does not [local]: the two-run control on the retail game has not
been reported ([high-framerate.md](../game/high-framerate.md), "Next", item 1);
split screen has not been run in this runtime
([high-resolution.md](../graphics/high-resolution.md) lists it as "once it runs");
only pad reads are taped.

### 2.2 Input exchange and latency

The guest polls input once per main-loop iteration, before the ticks, and
a level frame is one tick and 33.3 ms of logical time
([high-framerate.md](../game/high-framerate.md) section 1). Call that a frame.

- **Schedule.** Frame *f* uses one packet per port. Each host samples its
  own pad on wall time and assigns the sample to frame *now + D*, where
  *D* is the input delay in frames, and sends it. A new `InputTap`
  answers every read in frame *f* from the agreed packets for *f*; the
  packet number is derived from the agreed stream (it goes up when the
  pad state changes), never from the local device.
- **Stall.** The guest may not start frame *f* until both ports' packets
  for *f* are in hand. The runner holds at the scheduler point after a
  present. Wall time is outside the guest, so a stall changes nothing in
  the run.
- **Delay.** *D* frames add *D* x 33 ms to both players' input latency.
  The game runs without stalls while one-way latency plus jitter stays
  under *D* x 33 ms.

  | *D* | added latency | holds up to about (round trip) |
  |---|---|---|
  | 1 | 33 ms | 40 ms (LAN, same city) |
  | 2 | 67 ms | 100 ms (default) |
  | 3 | 100 ms | 160 ms |
  | 4 | 133 ms | 230 ms |

  Pick *D* at connect from measured round trip and jitter, and let it
  change only at an agreed frame. [guess] 67 ms on top of the game's own
  latency is noticeable when aiming and fine for co-op with a friend.
- **Loss.** Every packet repeats the last several frames' inputs (a frame
  is 18 bytes of pad state plus a header), so one lost datagram costs
  nothing. About 2 KB/s each way.
- **Speed.** Lockstep runs at the pace of the slower machine. A host that
  cannot hold 30 fps with two views slows both players.
- **Rollback** (predict the remote pad, re-simulate on a miss) would
  remove the delay. It needs a machine snapshot and restore every frame,
  which the runtime does not have ([theater.md](../game/theater.md) phase 7,
  "in-memory machine forks", 4 to 8 weeks). Not in this plan.
- **Cheating** is not a concern: this is for friends.

### 2.3 Start-state agreement

The machines must be identical before the first instruction. The
handshake compares, and refuses on any difference:

| item | source |
|---|---|
| build | the image stamp (generated code, overrides, passes; [BOOT.md](../runtime/BOOT.md)) |
| features | the `exact` fingerprint of [feature-engine.md](../engine/feature-engine.md) "Determinism": every `View` and `Sim` feature and setting, and the hook plan. Not built; section 5 says what the settings store needs for it |
| disc | title id, XBE hash, hashes of the maps the level loads |
| epoch | the NT epoch (`Config::epoch_100ns`), chosen by the host |
| user data | profile, settings and campaign saves the game reads from the emulated disk |
| cache partitions | the map cache state ([cache-slots.md](../runtime/cache-slots.md)); a warm cache and a cold one run different guest code |

For user data, the host packs its own profile and saves (small files, the
user's own data) and the guest's machine boots from that pack in a
**session data directory**, not from its own. Both start with empty cache
partitions. Difficulty and skulls are then simply part of the shared
state: skulls are found and toggled inside the game, and the difficulty
is chosen in the shared menu. `View` and `Sim` settings are the host's
and are locked for the session ("locked: co-op"); `Host` settings
(resolution, vsync, bindings, volume) stay per machine.

In retail split screen only player one's profile gets campaign progress.
Here the guest keeps theirs too (decided, section 6): when a session ends,
or at each checkpoint, the checkpoint and the campaign progress written in
the session data directory are copied into the guest's own data as well as
the host's. Both machines hold the same bytes, so nothing is sent for it.
[guess] The save names the profile that made it; copying it under the
guest's profile needs the save format read first (a research point).

### 2.4 Join and start flow

1. The host picks "Co-op over Live" in our shell (the Live side panel or
   a CLI flag), which creates a Live session with a co-op flag
   ([live-api.md](live-api.md), "Sessions": a new `flags` value) and
   invites a friend through the existing mailbox.
2. The friend accepts. Today an accepted invite becomes a local launch
   record; for a co-op session the shell handles it instead of the guest.
3. The hosts connect: direct UDP to the session endpoint
   ([live-matchmaking.md](live-matchmaking.md) 3.5 and 3.6, same NAT
   limits), with a relay through the Live server's WebSocket as the
   fallback. Input is small enough that relaying it costs the server
   nothing, but it adds a hop of latency.
4. Handshake (2.3), then both boot. The first version shares everything
   from the title screen: both players drive the same menus, player two
   presses Start to join in the campaign lobby as on a console. A later
   version boots straight into the level with two players through the
   level boot ([level-boot.md](../runtime/level-boot.md); it adds one player today).
5. Both report the hash each tick; the game runs.

The guest never learns it is online, so none of the network boundaries
exist in the guest during co-op.

### 2.5 Showing one player's view

Both machines must do the same guest work, so both guests render both
split-screen views. Only presentation differs, and it must be `Host`
class: after the device, holding no guest handle, as the scaled view of
[high-resolution.md](../graphics/high-resolution.md) is.

- **Classify by viewport.** In split screen the game sets a smaller
  viewport per player (`0x00014bc0` calls `SetViewport`;
  [high-resolution.md](../graphics/high-resolution.md), "The game's 640x480
  assumptions"). The presenter tags each draw with the viewport it was
  made under: top view, bottom view, or full screen. Full-screen draws
  are the things both players share: the pause menu, cutscene frames,
  loading screens, letterbox bars.
- **Compose.** Own view scaled to the window, then full-screen draws over
  it. While a frame has no split viewports (menus, cutscenes) it is shown
  as it is.
- **Aspect.** A two-player view is half the height of the screen, so its
  image is twice as wide as the output wants. Three steps, in order of
  cost:
  1. Show the strip letterboxed. Ugly, correct, no guest change. Good
     enough to prove the feature.
  2. An anamorphic view: a `View`-class change, identical on both peers
     and in the fingerprint, that makes the game render each split view
     with the full screen's field of view squeezed into the half-height
     viewport; the presenter stretches it back. The field-of-view hook on
     `0x00187130` ([settings-menu.md](../game/settings-menu.md)) is the handle.
     The HUD is drawn inside the same viewport and would be stretched, so
     HUD draws need their own placement in the presenter. [guess] This is
     the fiddly part.
  3. Re-render the own view at full resolution in the scaled view
     backend with the full-size viewport, so it is sharp rather than
     upscaled. Needs render scale to be landed.
- **GPU cost.** The other player's view can be skipped on the *view*
  backend only. The reference backend still draws it, because the guest
  reads values back from it.
- **Rumble** goes only to the local player's pad. It is an output.

### 2.6 Pause, menus, checkpoints, reverts

All of these are guest behaviour driven by pad input, so they stay in
step by construction:

- **Pause.** Either player's Start pauses the one shared game, as in
  split screen. Both see the same menu.
- **Checkpoints** are written by the same guest code at the same tick on
  both machines, into each session data directory. The files must not
  feed anything back that differs: storage completion has to be on the
  logical clock (see 2.9).
- **Reverts and deaths** follow retail co-op rules, in the guest.
- **Host overlays** (our settings, the Live panel) are host-side. Opening
  one must not stall the other player: local input goes neutral while it
  is open.
- **Cutscenes** are full-screen draws and show as they are. Skipping one
  is a pad input.

### 2.7 Disconnect and rejoin

- **Short loss:** stall, show "waiting for <gamertag>" from the host
  shell, resume. No state is lost.
- **Gone:** after a timeout offer the remaining player two choices:
  carry on alone (the missing pad reads neutral from an agreed frame; the
  game keeps a second, idle player) or save and quit. [guess] Reporting
  the pad as unplugged instead would bring up the game's own "reconnect
  controller" pause, which is worse.
- **Rejoin** to a running machine needs the whole machine state sent
  over (64 MiB of RAM plus HLE state), which does not exist. First
  version: both restart from the last checkpoint, re-running the
  handshake with the host's current saves.
- **Desync:** stop both, write both traces, and say which tick and which
  MiB of RAM differ (`lockstep::compare` does this today). Then the same
  restart from the last checkpoint.

### 2.8 Audio

Both guests mix the same audio, which is retail split-screen audio: one
mix that serves both players. Each machine plays it. It is correct for
the couch and slightly wrong apart, since each player also hears the
other's surroundings. [guess] A per-listener mix would need the sound
code's listener choice to become a host-side filter; not in this plan.

The audio output must use logical pacing ([dsound.md](../audio/dsound.md),
"Pacing"); device pacing is "not for deterministic runs". Voice chat is
out of scope; players use something else.

### 2.9 Determinism risks

The boundaries [feature-engine.md](../engine/feature-engine.md) ("Recording and
replay") and [theater.md](../game/theater.md) ("What the recorder captures")
name, and what co-op does with each:

| boundary | in co-op | risk |
|---|---|---|
| `PAD_READ` | agreed per frame and port (2.2) | none once keyed by frame; the packet number must come from the agreed stream |
| `DEVICE_CHANGES`, capabilities | two virtual pads, always present, same kind on both machines; host hot-plug is hidden from the guest | low; today the tap does not cover these, so they must be made constant |
| `NET_RECV`, `NET_SELECT`, connect status, QoS | the guest is offline | none |
| `RANDOM` (`XNetRandom`, `XNetCreateKey`) | not called offline; if anything is, seed from the handshake | low; check the offline boot path calls none |
| `LIVE_ANSWER` | the guest is not signed in | none; the price is no Live features inside the guest during co-op |
| `EPOCH` | from the handshake | none |
| `SETTING_CHANGE` | `View` and `Sim` settings locked | none |
| start state | 2.3 | medium: the cache partitions and any file the guest reads that the pack misses |
| build | image stamp and fingerprint must match | none, but both players need the same build; cross-version play is impossible by design |

And the ones those lists do not have, which matter more here because the
two machines are different computers:

| source | why it matters | what to do |
|---|---|---|
| **GPU readbacks** | render-to-texture write-backs and depth reads land in guest memory, and [high-resolution.md](../graphics/high-resolution.md) says outright that they are not identical across GPUs or drivers | measure which occur in campaign; exclude pixel memory the CPU never branches on from the hash; for anything the CPU does read, make one peer's answer authoritative and send it like an input |
| **Pixel counts** | `GetVisibilityTestResult` feeds the lens-flare pass (`0x00020bb0`); the count can differ by GPU, and the poll answers "incomplete" depending on *wall time* ([render-thread.md](../graphics/render-thread.md)) | co-op forces a deterministic policy (wait, or a fixed frame delay) and takes the count from the host peer |
| **Frame overruns** | a frame that takes more than its logical 33.3 ms runs two ticks on the next one; identical only if charged work is identical | follows from the rows above; the whole-RAM hash and clock catch it |
| **Storage timing and content** | write-behind and asynchronous completion ([storage.md](../runtime/storage.md)) | completion must be on logical time; test it |
| **Movies** | Bink playback length and skip timing; the optional MP4 conversion differs per machine | movie timing from the guest side only; check [bink-hle.md](../audio/bink-hle.md) uses no wall clock in the guest-visible path |
| **Audio** | `GetCurrentPosition` and `GetStatus` are guest-visible | logical pacing only |
| **Host architecture** | arm64 and x86-64 builds of the same image | floats are software-exact by design ([FLOAT.md](../lifter/FLOAT.md), [simd.md](../lifter/simd.md)); still, start with same-architecture peers and prove cross-architecture with traces |
| **Our own overrides** | a Rust override that reads host state (time, a file, a setting) on the guest thread | the fingerprint covers which are installed, not what they read; audit the `View` and `Sim` ones |
| **Telemetry and the tick hook** | the lockstep observer refuses a tick that telemetry already wraps | one chain per point (feature-engine "Hooks"); until then, telemetry off in co-op |

A whole-RAM hash is the strict test and the right one for this option:
any difference at all is a bug to find, because differences in guest work
move the schedule.

## 5. How it plugs into the settings and hook layer

This is written against the `settings-slice` branch (head `0b5c9bc`, not
on master when this page was written): `crates/formats/settings` (`Def`, `Store`,
`Setting<T>`), `boot::ui` (`Screen`, `Row`, `Extension`, `Model`),
`boot::setting_defs`, `Dispatch::override_handler` in
`runtime/src/dispatch.rs` with the chaining pattern of
`level_boot::install_activate`, and the "What was built" sections of its
[feature-engine.md](../engine/feature-engine.md) and
[settings-menu.md](../game/settings-menu.md). [local: read on that branch.] Co-op
should be the first feature built on it rather than beside it.

### 5.1 Settings, and which must match

Co-op's own options are `Def`s in `boot::setting_defs`, so they get the
file, the environment variable, `h2 play --set`, `h2 settings` and a row
for free:

| key | kind | scope | apply | notes |
|---|---|---|---|---|
| `coop.input_delay` | `Int`, 1 to 8, steps 1 to 4, plus an `auto` alias | `Host` | `Live` | each peer delays only its own samples, so the two need not match; a change takes effect at a frame the peers agree on |
| `coop.transport` | `Choice`: auto, direct, relay | `Host` | next session | |
| `coop.view` | `Choice`: mine, split | `Host` | `Live` | "split" shows the raw frame, for debugging and for a couch |
| `coop.aspect` | `Choice`: strip, anamorphic | **`View`** | `NextMap` | anamorphic changes the guest's field of view per split view (2.5), so both peers must run it |
| `coop.difficulty` | `Choice`: the four difficulties | **`Sim`** | next session | the last one used; the session's value comes from the host |
| `coop.tick_hash` | `Choice`: ram, simulation | **`View`** | next session | the tick observer is on the guest thread (5.3) |

Which existing settings must match between peers depends on the path,
and `Def::scope` already says it:

- **Lockstep below the game (3b) needs every `View` and `Sim` value
  equal**: the `exact` fingerprint. Field of view, HUD position and LOD
  distance carry `View` scope. Resolution is a local presentation choice. Frame rate,
  controller speaker and mute button carry `Host` scope and stay per machine.
- **The retail path (3a) needs only `Sim` values equal**: the `sim`
  fingerprint. Each peer renders its own view, so `graphics.fov` and
  every other `View` setting stays personal.

The mechanism to make them match is built: the guest peer calls
`Store::force(key, host_value, "co-op session")` for each key in the
handshake's list. A forced value is locked, its row shows it and a press
does nothing, and the file is not written. That is exactly "locked:
co-op" from 2.3.

### 5.2 Screens and rows

The co-op screens are `ui::Screen`s, reached by a row we add to a retail
list with `ui::extend`:

```text
(retail campaign list) + Row::open("coop")            "Co-op over Live"
Screen "coop"
  Row::setting("coop.difficulty")
  Row  Level                    (choice from the disc's level list)
  Row::setting("coop.input_delay")
  Row::setting("coop.aspect")
  Row  Invite a friend          (opens "coop.friends")
  Row  Status                   ("Waiting for <gamertag>", ping, checks passed)
  Row  Start
Screen "coop.friends"           one row per online friend; a press invites
Screen "coop.join"              opened by an accepted invite: host, level,
                                difficulty, the handshake's result, Ready
```

These run in the player's normal machine, signed in to Live. For the
host-level path, Start then boots the lockstep machine (2.4). For the
retail path, Start is where the session calls of 1.2 are made.

### 5.3 Hooks

| hook | where it goes | through the layer |
|---|---|---|
| pad boundary | not a guest address: the `InputTap` on `xapi` (`set_input_tap`), wired today by `boot::lockstep_glue` for record and replay | the schedule tap is a third tap; it should chain to the recorder so a co-op session can also be recorded as a film |
| tick observer (hash) | `lockstep::install_tick_observer` on `game_tick` `0x00137fe0` | today it **refuses** a tick that already has an override. It should chain the way `install_activate` does: take `Dispatch::override_handler(at)` as the thing its shadow entry runs, and log that it chained |
| start request | `level_boot`'s hook on `REQUEST_MAINMENU` and its activation mark | extend its `Request` with two players and the co-op flag (phase 4b); the mark already chains |
| split-view field of view | the existing `fov:camera` override on `PLAYER_FOV` `0x00187130` | no second hook: the same handler reads a second `Setting<T>` (`coop.aspect`) beside `graphics.fov`. A handle read is one atomic load, so it is safe with guest memory held |
| own-view presenter, HUD placement | after the device, on host surfaces | not a dispatch hook at all. It is a view stage in the design's "Rendering" section, or a mode of the scaled view backend |
| session start, joinable check (retail path) | `0x0006ec80`, `0x00063e00` | plain overrides registered with `register_override`, chained with `override_handler` if anything else is there |
| UI rows | the list hooks in `layout_glue`, asking `ui::Model` | as built |

### 5.4 Gaps in the layer that co-op would need filled

1. **No fingerprint.** `Def::scope` is recorded and nothing reads it. A
   `Store::fingerprint(scopes)` over (key, value in effect) is small; the
   handshake needs it and the list of keys to force. The `exact` one also
   needs the hook plan, and `Dispatch` cannot list its overrides (only
   `override_name(addr)`): add an iterator of (address, name).
2. **No way to release a forced value.** `Store::force` has no inverse,
   so after a session the guest peer's settings stay locked until the
   next launch. Needs `unforce`, or a store scoped to the session.
3. **Row presses are guest input.** In the lockstep machine both peers
   see every press, so a press on a `Host` setting's row would step it on
   *both* machines, and a row whose value differs between peers would
   put different string ids into guest memory and desync. `Model` needs a
   session filter: inside a lockstep machine it lists only `View` and
   `Sim` rows (all locked) and hides `Host` ones. Per-machine settings
   then need the host overlay (`Host::Overlay`), which is not built.
   Until it is, they are set before the session or from the CLI.
4. **Rows that do something.** Only `RowKind::Setting` and `Open` exist.
   Invite, Start and Ready need `Action`; the status row and the friends
   list need `Custom` or rows built at open time. A press today may only
   go through the store, never touch the guest; the retail path's Start
   has to call guest functions, so `Action` needs the `UiCx` the design
   sketches.
5. **Dynamic choices.** `Kind::Choice` is a static slice. The level row's
   choices come from the disc. Either an `Action` row with its own
   screen, or a choice list supplied at load.
6. **More lists.** `ui::List` has `Controls` and our own screens. The
   entry row needs the retail campaign list (or the main menu) as a new
   variant, with its hook addresses in `layout_glue`.
7. **Text values.** `Kind` has no text, so a relay address or a friend's
   gamertag cannot be a setting. Co-op takes both from the Live session,
   so this is only a note.
8. **Chaining is by hand and by install order.** `override_handler`
   lets a second hook run the first, but nothing declares or checks the
   order, and each chain needs its own shadow address. With telemetry,
   the level-boot mark and the tick hash all on two functions this works;
   the registry and `Plan` of the design are what make it safe at scale.
9. **Apply scopes.** `Apply` has `Live`, `NextMap` and `Restart`; co-op
   wants "next session". `Restart` is close enough to start with.
10. **Restart into a session.** Start has to end the current machine and
    boot another with a start request and a session data directory. The
    shell can restart (`h2 play --follow` does on F5), but there is no
    request to do it from a row.

Items 1 to 3 belong in phase 0 or 1; 4 to 6 and 10 in phase 3; 8 is the
registry work the feature-engine plan already has.
