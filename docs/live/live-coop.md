# Campaign co-op over Live (design; phase 0 slice, the phase 2 probe and Co-op over Live built)

Retail Halo 2 plays campaign co-op only in split screen on one console.
This page answers "can we add co-op over our Live service?" and lays out
the options. The owner has since decided
([section 6](#6-decisions-made-2026-11-16)), and the first slice of phase 0
is built on tests ([section 8](#8-what-is-built-phase-0-first-slice)), as is
the probe of the retail path ([section 9](#9-probe-phase-2)) and, on the
assumption that the probe's path works, Co-op over Live itself
([section 10](#10-co-op-over-live-built)).
The owner has Co-op over Live working in the game (2026-11-17).
More than two players (up to 16, behind `coop.max_players`) is built on
tests and traced in [section 13](#13-more-than-two-players).

In short:

- **The retail game carries a networked lockstep path, and it was built
  for campaign.** The simulation layer has a synchronous client and a
  synchronous server mode, four "synchronous" network messages, a client
  catch-up loop that calls the tick, and an out-of-sync latch. A network
  session picks the synchronous protocol by itself when it has no
  multiplayer variant, and its game-start routine builds *campaign*
  options (difficulty, a co-op flag) from session parameters. Two checks
  stop it: a campaign session will not start with more than one machine
  in it, and a campaign session reports itself as not joinable. There is
  no lobby screen for it. Whether the path works once those are lifted is
  unknown; it never shipped.
- **We can also do lockstep below the game.** Both machines run the same
  image in ordinary two-player split-screen co-op, completely offline as
  far as the guest knows. The hosts exchange only pad input, each frame,
  and each shows only its own player's half of the screen. This rests on
  the runtime's machine-level determinism rather than on the engine's. It
  needs no dormant retail code, but it has presentation compromises
  (split-screen view and HUD, shared audio and pause) and it puts a hard
  requirement on everything that reaches the guest from the host,
  including GPU readbacks.
- **Recommendation:** build the shared base first (a two-pad tape, a
  simulation hash, a pad-exchange transport, all provable by tests), then
  spend one or two weeks probing the retail path with two instances. If
  two peers hold a campaign level in sync, finish that path: it gives each
  player a normal full-screen game. If not, ship the host-level lockstep.
  The probe is cheap and the base is needed either way.
- **It builds on the settings and hook layer** of the `settings-slice`
  branch: the co-op options are `Def`s, the settings peers must share
  are the ones whose scope is `View` or `Sim`, the lobby is `ui` screens,
  and the hooks chain through `Dispatch::override_handler`. Section 5
  lists what that layer still lacks for this.

Evidence labels, as in [theater.md](../game/theater.md):

- **[local]**: read from this repository's docs or code, or from a static
  reading of the owner's retail XBE with `h2 disasm` and a string and
  cross-reference scan. Static only.
- **[community]**: public knowledge. Where I could not re-check a source
  for this page it says "from memory".
- **[guess]**: my inference. Check it before building on it.

## 1. What the retail engine has

### 1.1 The simulation modes

[local] The simulation globals pointer is at `0x004CF77C` (set by the
initialization callback `0x00067e30`); the mode is the dword at `+8`. The
name table at `0x00468c64` gives the values in order: none, local,
sync-client, sync-server, dist-client, dist-server (0 to 5). Game options
carry the same value as a byte at `+4` (`G+0x0c` in
[live-gamestate.md](live-gamestate.md)); the options check `0x00138180`
accepts 0 to 5. The new-map callback `0x00067ee0` reads it and builds the
simulation world (`0x00068c00`).

[local] The session's game-start routine `0x0006ec80` computes it. With
`S` the session:

| input | where | meaning |
|---|---|---|
| protocol | `S+0x4DAC` | 0 local, 1 synchronous, 2 distributed; anything else refuses the start |
| is host | `S+0x72D8 == S+0x40` | host peer index against the local one |
| campaign difficulty | `S+0x5DD0` (word) | `0xFFFF` means "not campaign" |
| peer count | `S+0x54` | [guess] the word before the 16-entry peer array at `S+0x58` (stride `0x10C`) |
| player count | `S+0x1118` | directly after that array; compared with the session's player limit at `0x00063e4f` |

The mode written to options `+4` is 1 for protocol 0, `2 + is_host` for
protocol 1 and `4 + is_host` for protocol 2. So a synchronous session
makes the host a sync-server and everyone else a sync-client.

[local] The protocol is chosen in `0x00064060`, which the lobby's "set
variant" call `0x0019a728` reaches: a session of type 0 (local) gets
protocol 0; a session with **no variant gets protocol 1, synchronous**;
otherwise `0x00161c90` maps the variant's game engine to a protocol. That
map gives distributed for every engine that shipped and synchronous for
the two deprecated ones (Race, Headhunter) and for anything out of range.
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
its first map"). The lobby side exists too: `0x0019a76d` sets the
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
- **After, `0x000684e0`.** Mode 2 (sync-client) calls `0x00069880`, mode 4
  (dist-client) calls two other routines, and the servers only advance
  the update number.
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
port 1 is the guest player, on both machines. The two hosts agree, frame
by frame, on what every pad read returns, so both machines run the same
instructions on the same schedule ([high-framerate.md](../game/high-framerate.md)
section 1, "In our runtime"). Each host presents only its own player's
view. A hash exchanged every tick proves the two machines are still the
same.

What exists [local]: the logical clock and cooperative scheduler;
`xapi::tap` (an `InputTap` on every `XInputGetState`, with `Recorder` and
`Replayer` keyed by read index); `crates/lockstep` (a hash of in-use guest
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

## 3. Other options

- **Stream player two's view.** The host runs split screen alone; the
  friend receives video of their half and sends pad input. No determinism
  needed and it works with any build on the far side, even a browser.
  Against: an encoder and a low-latency transport we do not have, 60 to
  120 ms of latency for the friend only, upstream bandwidth, compressed
  half-resolution video, and the host renders everything. It is a remote
  desktop. Tools that do this already exist, so there is little reason to
  build it into the project. Worth remembering as the fallback that
  cannot desync.
- **Play co-op as a multiplayer game**: load a campaign scenario with the
  distributed protocol. [guess] Campaign AI, scripts and cutscenes have
  no replication in the distributed model, which is why the campaign
  protocol is synchronous. Not viable.
- **Host-authoritative state copy**: the host sends the changed parts of
  the 4 MiB game-state block every tick and the guest only renders.
  [guess] Tens of KB per tick even after XOR and compression, and the
  guest's own input still arrives a round trip late. Lockstep is strictly
  better while determinism holds.

## 4. Recommendation and plan

### Which path

| | retail synchronous path (section 1) | lockstep below the game (section 2) |
|---|---|---|
| each player's screen | full screen, normal HUD and audio | half of a split screen, composed by us |
| cost per machine | one view | two views |
| depends on | dormant retail code working, and the engine being deterministic across different local players | the *machine* being deterministic across two computers, GPU answers included |
| if it breaks | a retail bug in code nobody finished; we can see it (per-pool hashes) but may have to fix guest logic | a boundary of ours that leaks; ours to fix |
| menus and lobby | needs a lobby flow built on the session calls | free: the retail split-screen flow |
| Live features in game | the guest is online | the guest is offline |
| join in progress | maybe, through the game-state message | restart from checkpoint |
| known to work | no | no, but every part is ours |

Section 2 is the one we can promise. Section 1 is the better result if it
works, and finding out is cheap because our tools make a desync visible:
two hashes of the game-state block, per named allocation, name the pool
that parted. So: build the base, probe section 1, then commit.

### Phases

| # | what | needs the owner's game runs | effort |
|---|---|---|---|
| 0 | **Base, tests only.** A two-port schedule tap (`InputTap` answering from agreed per-frame packets, deterministic packet numbers); a pad-exchange protocol with input delay, redundancy and stall; a simulated link (latency, jitter, loss, reorder) on a fake clock; a simulation hash over the game-state block beside the whole-RAM hash; on the settings layer (section 5): the co-op `Def`s, a store fingerprint by scope, releasing forced values, the tick observer chaining instead of refusing | no | 1.5 to 2 weeks |
| 1 | **Controls on the real game.** Split-screen co-op boots and plays; a two-pad tape replays to an identical trace twice in two processes, then on a second machine; list every chunk that differs and close the boundaries of 2.9 (visibility policy, readbacks, storage, audio pacing) | yes: record one co-op session, run the replays, send the traces | 1 to 2 weeks, mostly waiting on what the traces show |
| 2 | **Probe the retail path.** Two instances over system link in a pregame lobby; a Rust entry for the session start (`0x0006ec80` is the options builder [feature-engine.md](../engine/feature-engine.md) already wants as a port) without the peer-count refusal; set the difficulty and a campaign map from the host through `0x0019a76d`; read the mode on both peers; per-pool simulation hashes on both; see how far a mission gets | yes: two instances, a few short sessions | 1 to 2 weeks, time-boxed |
| 3a | **If the probe holds: finish the retail path.** A campaign lobby (invite, difficulty, level) on the session calls; the joinable predicate; fixes for whatever desyncs; disconnect handling | yes | 4 to 8 weeks, wide error bars |
| 3b | **Otherwise: lockstep below the game.** Session data directory and start-state pack; handshake; UDP transport and server relay; stall and resume in the runner; desync report; the letterboxed own-view presenter | yes: two machines | 3 to 4 weeks |
| 4b | Anamorphic view and HUD placement; direct level boot with two players; carry-on-alone; copy progress to the guest | yes | 2 to 4 weeks |

Phases 0 and 1 serve Theater too: its phase 1 needs the same tape,
manifest and hash ([theater.md](../game/theater.md)).

### The first slice (phase 0), provable without the game

All on `lockstep::synthetic::Toy`, extended to read two ports:

1. Two toy guests in one process, fed the same two-port tape, produce
   identical traces; a one-bit change in one port's packet on one peer
   diverges at exactly that tick and `compare` names it.
2. The exchange over a simulated link: with latency under the delay
   budget neither guest stalls; above it they stall and still produce the
   trace of a zero-latency run; with loss and reordering the trace is the
   same again.
3. A read can never be answered from a local device: the tap fails the
   test if a frame's packet is missing rather than falling back.
4. Packet numbers come from the agreed stream: two peers with different
   local packet counters give the guest the same bytes.
5. A hash mismatch stops both sides within one exchange and reports the
   tick.
6. The delay can change at an agreed frame without a divergence.

Crates: `xapi` (the tap), `lockstep` (toy, hashes), a new small crate for
the protocol and the simulated link. No guest addresses, no game data.

### What needs the game, and only the owner can run

- That split-screen co-op runs at all here, and at what frame rate.
- The two-run and two-machine controls (phase 1).
- Everything in phase 2: it is an experiment on retail behaviour.
- Feel: whether 67 ms of input delay is acceptable, and how the
  half-screen view looks.

## 5. How it plugs into the settings and hook layer

This is written against the `settings-slice` branch (head `0b5c9bc`, not
on master when this page was written): `crates/settings` (`Def`, `Store`,
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
  equal**: the `exact` fingerprint. Of today's five, `graphics.fov` is
  `View` and must match; `graphics.fps`, `graphics.render_scale`,
  `pad_speaker.enabled` and `pad.mute_button` are `Host` and stay per
  machine. There is no `Sim` setting yet. Future ones the design names
  (button layouts, Forge and game variants) would be.
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

## 6. Decisions (made 2026-11-16)

The owner's answers to the questions this page ended with:

1. **Order.** Probe the retail synchronous path first, after phase 0.
   Lockstep below the game is the fallback.
2. **Relay.** Yes: the Live server may relay co-op traffic (pad input and
   tick hashes, or the retail path's packets) when a direct connection
   fails. Built: section 8.
3. **Saves.** The guest keeps the checkpoint and campaign progress too,
   not the host only (2.3).

Defaults taken for the rest, until someone says otherwise:

4. **Two players.** Both paths stop at two.
5. **Input delay** of 2 frames by default, per machine.
6. **Same-architecture peers first.** Cross-architecture play waits for
   traces that prove it.
7. **Telemetry off in co-op** until the tick observer chains (5.3).
8. **View compromises** for the fallback path stay as proposed in 2.5:
   letterboxed strip first, anamorphic later; shared audio and pause.

Still open: whether `settings-slice` lands before the co-op options are
built. Section 5 assumes it does; the slice in section 8 does not depend
on it.

## 7. Research points

- What `S+0x54`, `S+0x40` and `S+0x50` are, exactly (peer count, local
  peer, and a second host comparison at `0x0005b91a`).
- The code that turns the latch at simulation globals `+0x2c` into the
  "out-of-sync" reason, and what a client shows.
- The sync-client's `0x00069880` and `0x000860b0`: where the client's own
  actions are sent, and how many updates the server buffers.
- The five event kinds in the update record, and the `+0x403C` flag.
- Whether the campaign UI's session really is the same object a system
  link lobby uses.
- Readers of the split-screen viewport table and the field of view per
  view, for the anamorphic view.
- Which GPU readbacks happen during a campaign level, and which of them
  the CPU reads.
- The campaign save and profile formats, for copying progress to the
  guest's own profile (2.3).

## 8. What is built (phase 0, first slice)

All on tests; nothing here was run in the game. It is the part of phase 0
that does not need the settings layer.

**`crates/coop`** (no guest, no game data):

- `wire`: the one datagram. It carries the sender's acknowledgements and
  repeats every pad frame and tick hash the other side has not
  acknowledged (up to 32 of each, 859 bytes at most), so delivery is in
  order whatever the link drops, and nothing is retransmitted on a timer.
  A datagram of another session, another version or another shape is
  counted and dropped.
- `link`: `Link` is an unreliable datagram path. `UdpLink` is the direct
  one, on the XNet base port + 4 (`live_hle::NetworkConfig::coop_endpoint`;
  base+0, +1, +5, +6 are the title's channels and +2 is QoS, so a host
  that opens base..base+6 for System Link already opens it). `Fallback`
  sends on the direct path always and on the relay too while nothing has
  been heard on direct for a timeout. `sim_pair` is a simulated link on a
  fake clock: latency, jitter, loss, reordering, a switch to take it down,
  repeatable by seed.
- `relay`: `RelayLink`, the relay path as a `Link`, on a thread of its own.
- `schedule`: the two-port pad schedule of 2.2. A machine's sample taken
  when it starts frame *f* is frame *f + delay* of its port; the first
  `delay` frames are neutral; the delay is per machine and can change
  during a game without notice to the other side. Packet numbers are
  derived from the agreed stream.
- `peer`: one machine's end. `begin_frame` starts a frame only when both
  ports' pads are in hand (`Hold::Waiting` otherwise); `report_hash` takes
  a tick's hash, and the first tick whose hashes differ latches
  `Hold::Desync` on both machines.
- `tap`: `ScheduleTap`, an `xapi::tap::InputTap`. It overwrites the live
  packet for ports 0 and 1, answers "not connected" for anything else,
  and counts a read that came with no frame started.

**`crates/lockstep`**: `SimRegion` and `hash_sim`, the simulation hash:
one word over the game-state block (`SimRegion::HALO2`: `0x3fe000` bytes
behind the base word at `0x004e6080`, from the static notes, unchecked in
a running game) instead of all of RAM.

**Live server**: `GET /v1/sessions/{id}/relay` ([live-api.md](live-api.md),
"Co-op relay"): a WebSocket for the two members of an open session
advertised with `flags: "coop"`, authenticated with a `sessions.relay`
token, 1200 bytes a message, rate-limited per socket
(`Policy::relay_rate`). It forwards bytes and never reads or stores them.

**What the tests prove** (`cargo test -p coop -p lockstep -p live-server`):

| claim | test |
|---|---|
| both machines' guests read the identical per-frame pads under latency, jitter, loss and reordering, equal to a zero-latency run | `coop/tests/peers.rs`: `loss_and_reordering_change_nothing_the_guest_reads` (8 seeds), `over_the_budget_frames_wait_and_the_stream_does_not_change` |
| inside the delay budget no frame waits | `inside_the_delay_budget_no_frame_waits` |
| a dead link stalls both machines within their delay's worth of frames, and they resume with the same stream | `a_dead_link_stalls_both_and_they_resume` |
| the relay carries the game while direct is dead, direct takes over when it works, and the relay then goes quiet | `the_relay_carries_the_game_while_direct_is_dead`; with the real server and real sockets, `live-server/tests/relay.rs`: `two_coop_peers_play_through_the_relay_when_direct_is_dead` |
| a differing tick hash stops both machines at that tick | `a_differing_hash_stops_both_machines_at_that_tick` |
| per-machine delays, and a change of delay during a game | `each_machine_has_its_own_delay_and_may_change_it` |
| real UDP on loopback | `two_peers_over_real_udp_on_loopback` |
| two toy guests (`lockstep::synthetic`) reading through the tap over a bad link give identical whole-RAM traces, and a word changed on one machine stops both at that tick | `coop/tests/toy.rs` |
| a read never falls back to the local device | `coop::tap` unit test |
| only the session's two members get into the relay; size and rate caps close the socket | `live-server/tests/relay.rs` |

**Left for the rest of phase 0 and after**

- Wiring into `boot` and `app`: nothing calls `Peer` yet. The runner
  needs the hold at the scheduler point after a present, the tap needs a
  handle-to-port map from `xapi` (the tap is given a handle only), and
  the tick observer has to feed `report_hash`.
- The tap covers `XInputGetState` only. Device arrival and capabilities
  still come from the local backend (2.9).
- The handshake of 2.3 (build, fingerprint, disc, epoch, start-state
  pack) and the session data directory. The datagram's session number is
  the only check today.
- Round-trip measurement and choosing the delay from it; the "waiting
  for <gamertag>" notice; carry-on-alone (2.7).
- The relay client speaks `ws://` and `wss://` through tungstenite's own
  connector. `live-telemetry` builds its TLS connector by hand for the
  ingest socket; the two should share one. Token refresh on a long
  session is on the server side only: the client does not send a fresh
  token yet, so a relay socket lasts one token lifetime.
- UDP hole punching. Direct needs a reachable port, as System Link does
  ([live-matchmaking.md](live-matchmaking.md) 3.6); otherwise the relay.
- The retail path's own packets through the relay (section 1): the relay
  forwards any bytes, but the XNet HLE does not use it.
- A `coop` session shows in `GET /v1/sessions` like any other. The HLE's
  custom-game search and the web session list should leave it out or
  label it.
- The settings-layer gaps of 5.4, when `settings-slice` lands: the co-op
  `Def`s (`coop.input_delay` and the rest), a store fingerprint by scope
  for the handshake, releasing forced values, the scope filter on
  in-guest rows, `Action` rows for the lobby, and the tick observer
  chaining through `Dispatch::override_handler` instead of refusing.

## 9. Probe (phase 2)

Built on tests; **no game was built or run for it**. The probe tries the
retail synchronous path of section 1 with two instances over System Link
and writes enough on each machine to say how far it got and where the two
parted. It is off unless `H2_COOP_PROBE=1`. Code:
`crates/boot/src/coop_probe.rs`, `crates/lockstep/src/pools.rs`,
`h2 coop-probe diff` (`crates/cli/src/coop_probe_cmd.rs`).

### 9.1 What it changes

All through the feature plan (feature `coop-probe`, points in
`engine::points`), nothing patched by hand:

| retail block | what the probe does | why this way |
|---|---|---|
| a campaign start with more than one peer is refused (`0x6eea5`) | a `before` on the session start `0x6ec80` writes 1 into the peer count `S+0x54` when the session is campaign, has exactly two peers and at most two players; a `before` on `OPTIONS_NEW` `0x138110` puts the real count back; an `after` on `0x6ec80` puts it back if the start refused for another reason first | `0x6ec80` reads `S+0x54` once (`0x6ee7e`) and makes no call between its entry and `OPTIONS_NEW`, the first call after the checks. Everything after that, the game start `0x18e8b0` and the simulation world it builds, sees the real count (`0x6e798` builds a peer mask from the same field, so it is the peer count). Porting all of `0x6ec80` (a `0x115c`-byte frame, the options block, five callees) would be a copy of retail logic with room for mistakes; the window trick changes one word for the length of the checks |
| more than two players (`0x6ee9b`) | left alone | two machines with one player each pass it; the probe stops at two anyway |
| the host's game-start parameters set a campaign session's peer limit `S+0x4990` to 1 (`0x6e720`, state 5) | an `after` sets it back to 2 | [guess] the guest is already in by then, but the limit checks (`0x5af70`, `0x65ab0`) compare the peer count with it, and two peers over a limit of one may be dropped |
| no lobby for a networked campaign | on the host, once a second machine is in the System Link lobby, a `before` on `game_tick` calls the campaign menu's own setters: `0x19a6f2` (campaign 1, the level's map id in ESI) then `0x19a76d` (difficulty in AX), and re-enters the tick | these are what the campaign menu calls (`0x199af6`, `0x199b00`); `0x19a76d` also clears the variant, which selects the synchronous protocol, and both replicate to the guest as session parameters. Up to three attempts, 60 ticks apart |
| a campaign session is never joinable (`0x63e7a`) | a `before` on `0x63e00` answers as retail would without that clause while fewer than two players are in | the predicate's only caller feeds Live presence (`0x19104a`, bit `0x10`); [guess] System Link joins are gated by the peer and player limits instead. It is cheap, and a Live co-op lobby will need it |

The level comes from `H2_COOP_PROBE_MAP` (a map file stem, default
`01a_tutorial`) and `H2_COOP_PROBE_DIFFICULTY` (`easy`, `normal`,
`heroic`, `legendary`; default `normal`), resolved against the disc at
boot like `--level`. Only the host uses them; setting the same values on
both machines is harmless. If an address the probe needs already has an
override outside the plan, it prints `coop-probe: refused: ...` and
stays off; the game boots as usual.

### 9.2 What it writes

Every line on stderr starts `coop-probe:`.

| line | meaning |
|---|---|
| `on; the host sets 01a_tutorial normal (campaign 1, map N)` | the probe is on and the level resolved. `... will not set the lobby's level` means it did not (a bad name, or not a campaign map); that machine can still join and log |
| `refused: SESSION_GAME_START (0x0006ec80) has ...` | another override holds an address; the probe is off |
| `session 0x...: state S, peers P (max M), players N (max K), host\|guest (...), protocol X (synchronous), difficulty D, campaign C map M, ...` | the squad session, printed whenever any of it changes. A campaign lobby shows `protocol 1 (synchronous)` and a difficulty |
| `host: lobby setup: waiting for a second machine (1 of 2)` | the host waits for the guest to join |
| `host: setting the lobby to ... attempt 1 of 3`, `host: LOBBY_SET_MAP returned 1`, `host: LOBBY_SET_DIFFICULTY returned 1` | the setters ran. `returned 0 (refused)` names the one that refused |
| `host: the session holds ... press Start in the lobby` | the session now reads campaign with the right map and difficulty |
| `joinable: true (retail false; ...)` | the presence predicate's answer changed |
| `session start: session ...`, then `campaign with 2 peers: showing the start 1 peer until OPTIONS_NEW`, `peer count back to 2 before OPTIONS_NEW`, `session start returned 1 (the game starts)` | the session start on that machine, the bypass and its undo. `returned 0 (refused)` means it refused for some other reason (the line before it shows the session) |
| `game limits: campaign peer limit 1 -> 2` | the host's game-start parameters were corrected |
| `game start: options mode 1, simulation mode sync-server (3), scenarios\solo\...` | a map activated: options mode 1 is campaign; the simulation mode is the dword at simulation globals `+8` (section 1.1). The host should say `sync-server (3)` and the guest `sync-client (2)`. `local (1)` means the start did not take the synchronous path |
| `simulation mode ... at tick T` | the mode changed |
| `hash log <data>/coop-probe/<time>-<pid>-<role>.hashes` | the per-tick hash file for this game; `hashing the game-state block at 0x...: N pools` follows on the first tick |
| `OUT OF SYNC: the retail latch (simulation globals +0x2c) is set at tick T, update U` | **the retail desync latch fired** (update number, tick or random seed mismatch, or an update out of order; section 1.3). The most important line |
| `the simulation's ended latch (0x4cf772) is set at tick T` | the simulation ended itself (on a server, after the latch) |
| `stall: N ms without a tick before tick T (last T', update U)` | more than half a second of wall time between two ticks while a game runs: a peer waiting for updates (or a load) |
| `checkpoint save`, `checkpoint revert`, `bsp switch`, `cinematic start`, `cinematic stop` `at tick T` | those events, also noted in the hash file |
| `game end at tick T`, `wrote N ticks to ...` | the game ended and the hash file is closed |

The hash file starts `# h2 coop-probe hashes v1`, then `#` lines with the
role, modes, scenario and session, then one line per tick and pool:
`<tick> <pool> <16 hex digits>`. The tick is the game's own tick count
(`game_time +8`), so both machines number ticks alike. Pool `*` is the
whole game-state block (the `0x3fe000` bytes behind `0x4e6080`, what
checkpoints save). The other pools cut the block at every datum array
header (`'d@t@'`), named by the array, with `(head)` before the first; a
name seen twice gets `#2`. Events are `# tick T ...` lines. It is flushed
every 300 ticks and at every event, so a crash loses little. About 100
bytes per pool per tick: expect a few MB a minute.

`h2 coop-probe diff <host.hashes> <guest.hashes>` prints the ticks both
cover, the first tick where they differ with the pools that differ there
and any event noted at that tick, and how many ticks each pool differs on.
It exits 0 when every common tick agrees.

### 9.3 Running it

You need an `h2` built from this branch and one disc image. System Link
in `h2 play` runs on the Live HLE, which is on only with `--live`,
`--live-server` or `--live-local`, so both instances talk to one Live server (it carries
no game traffic; the instances find each other by System Link broadcast).

**Two instances on one Mac.** Two windows, two data directories, two XNet
base ports at least seven apart, and the Mac's LAN address, not
`127.0.0.1` (Halo treats `127.0.0.1` as an in-process loopback;
[live.md](live.md), "Client HLE: sockets and System Link").

```sh
# Terminal 1: a Live server for both (SQLite, no database server).
h2 live serve --listen 127.0.0.1:8074

# Your LAN address, used below.
IP=$(ipconfig getifaddr en0)

# Terminal 2: the host.
H2_COOP_PROBE=1 H2_COOP_PROBE_MAP=01a_tutorial H2_COOP_PROBE_DIFFICULTY=normal \
H2_XNET_ADDR=$IP:31000 H2_XNET_LOOPBACK_PEERS=$IP:31010 \
h2 play disc/halo2.iso --data ~/h2-coop/host --live-server http://127.0.0.1:8074 \
  --gamertag Alpha 2> ~/h2-coop/host.log

# Terminal 3, once the host's window is up (the first start may build;
# the second must not build at the same time, and the CLI's account file
# is shared): the guest.
H2_COOP_PROBE=1 \
H2_XNET_ADDR=$IP:31010 H2_XNET_LOOPBACK_PEERS=$IP:31000 \
h2 play disc/halo2.iso --data ~/h2-coop/guest --live-server http://127.0.0.1:8074 \
  --gamertag Bravo --no-audio 2> ~/h2-coop/guest.log
```

`2>` keeps stderr in a file; use `2> >(tee ~/h2-coop/host.log)` to watch
it too. `--no-audio` on one of them keeps the sound from doubling.
`--gamertag` makes the account if it is missing.

**Two Macs.** The same, without `H2_XNET_LOOPBACK_PEERS` (System Link
broadcasts on the LAN), with each Mac's own LAN address and the same base
port on both: `H2_XNET_ADDR=<this Mac's IP>:3074`. Both point
`--live-server` at the one server over HTTPS (a client sends tokens over
plain HTTP only to loopback), e.g. `h2 live serve --tunnel` on one Mac
([live-hosting.md](live-hosting.md)). Both Macs
need UDP 3074 to 3080 open to each other (the macOS firewall asks the
first time). Use the same `h2` build on both, and start with two Macs of
the same architecture (section 6, item 6).

**In the menus.**

1. Both: past the title, pick or make a profile.
2. Host: **System Link**, then create a game. Leave the game type and map
   as they are; the probe replaces them. The host's stderr says
   `host: lobby setup: waiting for a second machine (1 of 2)`.
3. Guest: **System Link**; the host's game should be in the list. Join it.
4. Host's stderr: `host: setting the lobby to ...`, two `returned 1`
   lines, then `host: the session holds ... press Start in the lobby`. The
   lobby may now show something odd for the map or game type: there is no
   retail lobby screen for a campaign.
5. Host: press Start (the usual start of a System Link game).
6. Both: `session start ... returned 1`, then `game start: options mode
   1, simulation mode sync-server (3)` on the host and `sync-client (2)`
   on the guest. Play for a few minutes: move both players, fight, reach a
   checkpoint, let a cutscene run. Quit from the pause menu when it is
   clearly broken or after ten minutes or so.

Do it once on `01a_tutorial` (small, loads fast; its co-op script may end
early) and once on `03a_oldmombasa` (a cutscene at the start, then
combat).

**What to send back.** From each machine: the stderr file
(`host.log`, `guest.log`) and the newest `.hashes` file in
`<data>/coop-probe/` (`~/h2-coop/host/coop-probe/`,
`~/h2-coop/guest/coop-probe/`), plus the output of

```sh
h2 coop-probe diff ~/h2-coop/host/coop-probe/<newest>.hashes \
                   ~/h2-coop/guest/coop-probe/<newest>.hashes
```

and a sentence on what you saw (did the guest load the level, did both
players move, did it freeze, crash or drop back to the menu, and when).
`grep coop-probe: host.log` is the short version of each log.

### 9.4 What might go wrong, and how you will see it

- **The lobby setters refuse or never take.** `returned 0 (refused)`, or
  three attempts and no `the session holds` line. In a pregame lobby the
  map setter sends its change as a message instead of writing it
  (`0x5b790`, states 3 and 4), so the session may update a few frames
  later or not at all. The session lines show what the host's session
  holds.
- **The lobby setup never starts.** No `host: lobby setup` line at all on
  the host: the menu does not tick while the lobby is up, so the setup has
  no place to run. Report it; the next step is a hook on a lobby
  function instead of the tick.
- **The guest starts multiplayer.** The guest's `session start` line shows
  difficulty `none`: the parameters did not reach it before the start.
- **The start is refused anyway.** `session start returned 0 (refused)`
  with the bypass lines before it: another of `0x6ec80`'s checks failed
  (a missing map path, a bad protocol); the session line just above says
  which field looks wrong.
- **The guest never loads, or both freeze after loading.** `stall` lines
  that go on: a sync-client waits for updates the server never sends, or
  the server for the client's actions. Unread retail code (section 7).
- **`OUT OF SYNC`.** The retail latch: the two simulations disagree on the
  update number, the tick or the random seed. The diff names the first
  tick and pools that differ; with no `OUT OF SYNC` line the hashes are
  the only witness.
- **A crash** in either instance while starting the game or on the first
  ticks: dormant retail code meeting a case it never met. The last
  `coop-probe:` lines say what had happened.
- **Cost.** Hashing reads 4 MiB per tick on the game thread, a millisecond
  or two on Apple silicon, so 30 ticks a second costs a few percent. Only
  while a game runs, and only with the probe on.
- Nothing is written to the disc image or to a profile by the probe. The
  hash files grow by a few MB a minute; delete `<data>/coop-probe/` when
  done.

### 9.5 What is tested

`cargo test -p boot --lib coop_probe`, `-p lockstep --lib pools`,
`-p cli coop_probe`:

- the variables: off unless `H2_COOP_PROBE` is on, defaults for the map and
  difficulty, bad values refused;
- only a campaign level with ids is used;
- the peer bypass is taken only for a live campaign session with exactly
  two peers and at most two players;
- the joinable answer differs from retail only in the campaign clause and
  only below two players;
- the host setup waits for a second machine, runs once due, stops when the
  session holds the level, and never on a guest; its two calls are the
  menu's;
- the peer limit is put back only for a campaign;
- the session and simulation are read through the life cycle's pointers,
  and mode, latch and ended changes are reported once;
- the plan: nothing when off; every hook placed when on; the tick setup
  only with a level; it shares the tick with the lockstep observer;
  `H2_FEATURES=-coop-probe` turns it off; an address overridden outside
  the plan refuses the probe and leaves the plan installable;
- the pools cut at every header and cover the block; a changed byte
  changes only its pool; tick lines parse back as a log; the diff names
  the first tick and pool on invented logs, and the command exits by its
  verdict.

Not tested, because only the game can: whether the lobby ticks, whether
the setters take in a System Link lobby, whether the bypassed start runs,
and everything after it.

## 10. Co-op over Live (built)

Built on tests, on the owner's decision to assume the probe's approach
(the retail synchronous campaign path, two consoles, one player each)
works and to test it all at the end. **No game was built or run for it.**
If the retail path breaks, only the start part (10.3) is redone; the
screens, the session plumbing, the text and the web stay.

### 10.1 What a player sees

**Host.** Xbox Live menu: Quickmatch, OptiMatch, Custom Game, Custom Game
Browser, **Co-op Campaign**, Content Download. The list still has five item
widgets, so with six rows it scrolls: Content Download comes into view
below the fifth row. Co-op Campaign has its own pane (the Custom Game
picture, our help text). It opens the co-op screen:

| row | value | press |
|---|---|---|
| Level | the level's name as the level select shows it (read from the disc's `mainmenu.map` at boot), else its map stem | the next level |
| Difficulty | Easy, Normal, Heroic, Legendary | the next one |
| Invite a Friend | | opens the friends screen |
| Status | `Invite a friend`, `Waiting for <gamertag>`, `<gamertag> is in; ready`, `... the lobby holds the level`, or what blocks Start | logs the status |
| Ping | the round trip to the guest, `42 ms`, or `-` with no guest (10.6) | logs it |
| Start | Ready / Not ready | when ready: the campaign menu's setters put the level and difficulty on the session (as the probe's tick setup does) and the screen closes, back to the lobby; otherwise logs why not |

Start is enabled when the squad session is in a lobby or game state with
exactly two consoles and at most two players, and the level has campaign
and map ids. The friends screen lists the host's friends (online first, at
most 20) with Online, Playing, their presence activity or Invited; a press
on an online friend sends a Live game invitation to the host's advertised
lobby. B on any of these screens closes it as on every screen of ours.
Another Live menu row ends hosting: the lobby becomes a custom game again
and the hooks disarm.

**Guest.** A console that joined a host's co-op session (an accepted
invite, joining a friend from the friends list, or the custom games
browser, which lists co-op sessions too) is a co-op guest. A watcher asks
the HLE once a second, while the squad session has a second console and
this one is not its host, which session it joined; when the server says it
is a `coop` session, the console arms (10.3), takes the host's skulls for
the campaign start, and opens the join screen at the next tick
(`H2_COOP_JOIN_SCREEN=0` leaves that to the Co-op Campaign row, which opens
the join screen for a guest): Host (`<gamertag>, 42 ms` once a ping came
back), Level, Difficulty, Skulls (`N from the host`, ours counted),
Handshake (`OK: the lobby holds the host's campaign`, or `Waiting
for the host's campaign`, `The lobby's difficulty differs`, `N consoles in
the lobby`, `No lobby`) and Ready (closes the screen; the guest waits for
the host's start in the lobby).

**Text.** The retail lobby line saying co-op needs two players on one
console (string id `0x0e0001ec`,
`coop_takes_two` in `mainmenu.map`'s strings) is the only text of that kind
on this path: the code names that id once, in the lobby's status
line (`0x24ffc9`, at `0x2500ab`), for start state 7 in co-op mode
(`0x199994`). Start Game (`0x251050`) acts only on state 9 and shows no
dialog otherwise, so the status line is where a refused start explains
itself. On the Live co-op path it reads ours (`coop_live::line_for`):

| When | Line |
|---|---|
| Co-op Campaign chosen, no session read yet | `Co-op over Xbox Live: 2 players, one per console.` / `Press Y in the lobby to invite a friend.` (`up to N players` above 2) |
| the host alone in its session | `Invite a friend first: co-op over Xbox Live` / `needs at least 2 consoles. Press Y for Friends.` |
| more players than consoles | `Co-op over Xbox Live: one player per console.` / `Sign out the extra players to start.` |
| more consoles than the limit | `Co-op over Xbox Live takes up to N consoles;` / `M are in this lobby.` |
| a guest (joined, or in a host's networked lobby before it knows) | `Co-op over Xbox Live: one player per console.` / `Waiting for the host to start the game.` |

It is ours from the moment the Co-op Campaign row is chosen (the role is
the host's before the row's original runs), and, with Live on, on any
console in a networked squad session (class not 0): a friend who accepted
an invitation sits in the host's co-op lobby before the host's
advertisement says co-op, and read the retail line until then. The tick
reads the session, picks the line and writes it, in that order.

Split screen keeps the game's own text: no session, or a local one (class
0), and not hosting or joined. So does a system link lobby with Live off.
Every text of the co-op screens is ours: guest memory slots of 128 bytes under ids `0x20ffd000` up, rewritten when a row
is drawn and every tick. Not `0x20ffe000`, the first choice: the community
variants answer index `0xffe0xx` whatever the top byte, so the co-op screen
showed their names (a test now checks every id against every private range).

**Skulls.** The host's own campaign selection (collected and on, for the
signed-in profiles: the Skulls page's rule, so the host has them unlocked)
is advertised on the co-op session in place of the multiplayer set, and the
host's campaign start applies it as for any campaign. A co-op guest's
campaign start applies exactly the host's skulls instead of its own
profiles': the engine's into the array (`skulls::coop_start`), and our own
(from 15, `coop_live::ours_of`) to `skulls_custom`, which acts on them as on
the host. The host advertises both (`coop_live::host_campaign_skulls`).

### 10.2 Session plumbing

- **Advertising.** Opening the co-op screen calls `Hle::set_coop(campaign,
  skulls)`. The lobby the game advertises (and any it already advertised,
  through a signed `SessionUpdated`) becomes `flags: "coop"` with a
  `campaign` (`live_proto::session::Campaign`: `level` stem, `name`,
  `difficulty` 0 to 3) and the host's campaign skulls. A public lobby is
  made friends-only (`coop_visibility`); invite-only stays invite-only.
  Leaving sends `flags: "custom"`, which drops the campaign.
- **Presence.** The HLE sends `activity: "Co-op: <level>"` with every
  presence change while armed (`live_proto::valid_activity`, at most 64
  characters); the server keeps and serves it ([live-api.md](live-api.md)).
- **Invites.** `Hle::invite_to_lobby` sends the same social `Send` the
  title's `XOnlineGameInviteSend` does, for this console's advertised
  session. The guest accepts with the retail invite flow and joins through
  the existing XNet/session path.
- **The guest's read.** `Hle::joined` (what `host_skulls` now uses too)
  returns the joined session as the server last described it, asking again.
- **Server and web.** `GET /v1/sessions?flags=coop`; finished co-op games
  at `GET /v1/coop/games` (a closed `coop` session that a guest joined: the
  host and everyone who joined, the campaign, start and end, the session's
  visibility), shown on the web's Games page (Campaign) and a player's
  Co-op tab. A friends-only game shows only to who played in it and the
  host's friends, an invite-only one only to who played in it, admins see
  all (live-api.md, "Co-op games"); the same holds for the game's telemetry
  in the games list and player pages, for the log (`/v1/events`,
  `/v1/stream`: a hidden game's and session's events go only to who may
  see them) and for the stats summary (non-admins get the public one). A
  running game follows its session's visibility when the host changes it,
  and keeps the last one once it ends. The web reads all of these as the
  signed-in account (a `read` token, and `?auth=1` on the stream), and
  reads again at once on a sign-in or sign-out. Presence follows the
  same rule: a reader who may not see the host's or guest's co-op game
  sees them "In a game", with no game id and no "Co-op: <level>". Co-op sessions show as "Co-op
  campaign" with level, difficulty, players and the host's ping in Find a game;
  friends' activity shows in the friends lists.

- **Ping** (10.6): the host's `ping_ms` on the session (`Update { ping_ms }`,
  0 clears it; left out until measured), shown on the web's co-op rows.

### 10.3 The hooks (the probe as a feature)

The probe's hooks of 9.1 are now also the feature `coop-live`, on the plan
whenever Live is on (`H2_FEATURES=-coop-live` takes them out), armed only
for a co-op session (`coop_probe::Arm`): the host arms with its level, a
guest with none. Disarmed, every hook lets the game run as retail and
nothing is logged or hashed. Armed, they are exactly the probe's: the
peer-count window on the session start `0x6ec80` / `OPTIONS_NEW`
`0x138110`, the peer limit back to 2 after `0x6e720`, the joinable
predicate `0x63e00`, the campaign setters `0x19a6f2` and `0x19a76d` on the
host's lobby once a second console is in (three attempts, reset when the
host changes level or difficulty), and all of 9.2's logging, with the hash
files in `<data>/coop-probe/`. `H2_COOP_PROBE=1` still forces the probe on
for the whole run (System Link), and then the co-op screens only advertise
and invite.

### 10.4 Your test (two instances, or two Macs)

Same build on both; nothing here needs a build of ours beyond your usual
`h2`. One Live server for both. Make the two accounts friends.

```sh
# Terminal 1: a server for both (loopback: clients send tokens over
# plain HTTP only to loopback).
h2 live serve
IP=$(ipconfig getifaddr en0)      # the XNet address below, not the server's
h2 live account create Alpha
h2 live account create Bravo
h2 live friend add Bravo --as Alpha
h2 live friend accept Alpha --as Bravo

# Terminal 2: the host.
H2_XNET_ADDR=$IP:31000 H2_XNET_LOOPBACK_PEERS=$IP:31010 \
h2 play disc/halo2.iso --data ~/h2-coop/host --live-server http://127.0.0.1:8074 \
  --gamertag Alpha 2> >(tee ~/h2-coop/host.log)

# Terminal 3, once the host is up: the guest.
H2_XNET_ADDR=$IP:31010 H2_XNET_LOOPBACK_PEERS=$IP:31000 \
h2 play disc/halo2.iso --data ~/h2-coop/guest --live-server http://127.0.0.1:8074 \
  --gamertag Bravo --no-audio 2> >(tee ~/h2-coop/guest.log)
```

On two Macs: drop `H2_XNET_LOOPBACK_PEERS`, use each Mac's own address with
port 3074 (`H2_XNET_ADDR=<this Mac's IP>:3074`), and open UDP 3074 to 3080.
The server must be reachable over HTTPS, since a client will not send a
token over plain HTTP to a LAN address: `h2 live serve --tunnel`
(or `h2 play --live-tunnel` on the host), and `--live-server
https://<that URL>` on both ([live-hosting.md](live-hosting.md)).
`h2 play --live` (the default service) works the same way when both
accounts are on it and friends there.

Checklist (stderr lines in brackets):

1. Both: sign in to Live. stderr: `coop-live: N campaign levels (M named)`
   (N about 15 on the retail disc; 0 means the disc read failed and the
   Level row says None).
2. Host: Xbox Live menu. Six rows, Co-op Campaign above Content Download,
   which scrolls into view. Moving onto Co-op Campaign shows the Custom Game
   picture and our help line. [`custom games: xbox live menu tag has 6
   panes and room for 5 rows`]
3. Host: Co-op Campaign. The screen shows Level, Difficulty, Invite a
   Friend, Status, Ping (`-` for now), Start. [`coop-live: hosting a co-op
   campaign`]. Step the
   level and difficulty [`coop-live: level ...`, `coop-live: difficulty
   ...`]. On the web, Find a game shows a Co-op campaign session once the
   game advertises its lobby; the friends list shows Alpha's activity
   `Co-op: <level>`.
4. Host: Invite a Friend, press Bravo [`coop-live: invited Bravo to session
   <id>`]. `could not invite ... is the lobby advertised yet?` means the
   game had not advertised a lobby at that moment: note where you were.
5. Guest: accept the invitation (retail invite flow). The join screen opens
   [`coop-live: guest: joined Alpha's co-op session ...`, `coop-live:
   guest: opening the join screen`]. Check its rows. Press Ready.
6. Both: within a few seconds of the guest joining, the host's Ping row
   and the guest's Host row show the round trip [`coop-live: ping N ms`
   on both; `coop-live: ping: no answer` means the guest's pings to the
   host's port + 2 got nothing back: a firewall on UDP base+2, or the
   session's endpoint is not the host's reachable address]. The web's co-op
   session row shows the same number.
   Host: Status says `Bravo is in; ready`, Start says Ready. Press Start
   [`coop-live: host: LOBBY_SET_MAP returned 1`, `... LOBBY_SET_DIFFICULTY
   returned 1`, `coop-live: host: start <level> on <difficulty>; back to the
   lobby`]. `returned 0 (refused)` is the first thing to send back: the
   setters' own host check (`0x63f00`, session class at `+0x4980`) may
   refuse a Live (not System Link) session.
7. The lobby line about co-op on one console reads ours on both.
8. Host: start the game from the lobby. From here it is section 9's
   probe: `coop-probe: session start ...`, `campaign with 2 peers: showing
   the start 1 peer`, `session start returned 1`, then `game start: options
   mode 1, simulation mode sync-server (3)` on the host and `sync-client
   (2)` on the guest, and the hash files. 9.4 says what each failure looks
   like. Play a few minutes: both move, a fight, a checkpoint, a cutscene.
9. Skulls: with a skull on in the host's profile, the guest's stderr says
   `skulls: co-op campaign start with the host's skulls: ...`; with one of
   ours on (Low Gravity, say), also `skulls: ours on this campaign: ...`,
   and the guest feels it.
10. Afterwards the web's Games page (Campaign) lists the co-op game with
    both players once the host's lobby closes.

What to send back: `host.log` and `guest.log` (or `grep -E
'coop-(live|probe)|skulls:|custom games' *.log`), the newest `.hashes` from
each `<data>/coop-probe/`, `h2 coop-probe diff <host>.hashes
<guest>.hashes`, and a sentence on what you saw and where it stopped.

### 10.5 What is tested

Since the first cut: the ping (`qos.rs` unit tests, the server test
below pinging the host's responder and seeing `ping_ms` arrive,
`coop_live` tests of both sides), our skulls carried, and every family
of our string ids checked against every other (`boot/src/string_ids.rs`).
`cargo test -p boot --lib coop_live` and the custom games tests: the
level list (start order, Halo 2's campaign only, the level select's
names, from synthetic globals; `#[ignore]` against `$H2_DISC`), the text
slots, the host flow (arming, advertising level, difficulty and skulls,
stepping, leaving), Start's checks and the status lines, the friends screen
and invites, the guest's arming, join screen and handshake lines, a custom
session or the host itself never being a guest, the lobby line only in a
co-op session, the guest's skulls at the campaign start, the six-row Live
menu and its sixth pane on a synthetic tag, and the hooks on the plan, off
until armed. `cargo test -p live-hle --test server
a_coop_lobby_advertises_its_campaign_and_the_invited_guest_reads_it`
against the embedded server: a friends-only `coop` advertisement with the
campaign and the campaign skulls, a stranger's browser not listing it, the
friends list, the invitation reaching the guest, the guest's join reading
the campaign, presence activity, and leaving back to `custom`. Server and
web: `crates/live-server/tests/coop_sessions.rs`, `web/test/coop.test.ts`.

Not tested, because only the game can: whether our screens open over the
Live lobby and look right, whether the join screen opens cleanly from the
tick, whether the setters take on a Live session (10.4, step 6), and
everything from the session start on (section 9).

### 10.6 Ping

**Mechanism.** The HLE's QoS side channel: every responder (the host's
XNet base port + 2, already there for the title's QoS probes) also answers
a co-op ping, `H2PNG001`, nonce, last round trip (u16 ms), with
`H2PNR001` and the nonce, for any sender. The guest's pinger thread pings
the host every 2 s (1 s timeout) at the session's endpoint as the server
saw it; each ping carries the previous round trip, so the host learns the
guest's number from the next ping, and both show the same one. The host
keeps its responder up while co-op is armed (`Hle::guest_ping`), takes the
slowest report of the last 6 s, and advertises it as `ping_ms` when it
moved by at least 5 ms and a tenth (`live_hle::ping_moved`).

Why this one: it measures the path the game's own traffic takes (the same
host and port family, peer to peer), with no new port and no server in the
middle. A server round trip would measure each console's way to the
server, not to each other; the phase-0 transport (`coop::link`) is not
running in the retail-synchronous path co-op over Live uses. The friends
screen shows no ping: friends not in the lobby have no endpoint to ping.


## 11. The retail lobby (co-op over Live, as the game's own co-op)

Section 10's list screens were ours. The Live menu's Co-op Campaign row now
opens the game's own pregame lobby in co-op mode instead: its own level and
difficulty screens, its player slots and gamertags, its ready state and its
Start Game. Live adds two things only: the second player can be a Live
friend (the lobby's Y opens the friends list and invites), and the guest
lands in the same lobby, showing the host's level and difficulty. The list
screens stay behind `H2_COOP_LIST_UI=1`.

### 11.1 Static trace

- Live menu row chosen: `0x230f92`. Its Custom Game branch (row 2, at
  `0x231001`) calls `0x199e2e(1)`, then `0x59890(2,0,0)` to create the Live
  squad session, and on success `0x199a03(2)`.
- `0x199a03(mode)` (`LOBBY_MODE_SET`, stdcall, `ret 4`) sets the lobby
  mode. Mode 0 (co-op) calls `0x63fb0`, then `0x199a92`, the campaign setup
  that calls `LOBBY_SET_MAP` and `LOBBY_SET_DIFFICULTY`. Mode 2 (custom
  game) calls `0x19a864`.
- The game's own way into co-op from a lobby is the squad settings dialog
  (row chosen `0x2bb801`, jump table `0x2bb884`). "Switch To: Co-Op Game"
  (`0x2bba01`) is gated on byte `[0x54D5A0]` (error dialog `0x32`). After
  the gate it calls `0x199a57`, `0x199a03(0)`, `0x121100` and `0x148d42`,
  clears `[0x54E7CC]`, calls `0x19a76d` and closes the dialog. Error `0x32`
  is also raised from Live menu code (`0x230888`), so the byte is not a
  plain "networked" flag. Its meaning is not traced; the row does not go
  through it.
- Start state `0x19a2ce` (`LOBBY_START_STATE`, stdcall, `ret 4`): 9 is
  ready, 7 means the co-op/teams check failed. At `0x19a3ed`..`0x19a41c`
  co-op wants exactly two players, both on this console (`0x19ab77`). The
  tail (`0x19a4c5`) gives 10 while the countdown runs (`0x64a60` >= 0,
  `LOBBY_COUNTDOWN`), otherwise 9 when `0x592f0` (`LOBBY_ALL_READY`) is
  true, else 8.
- The status line (`0x24ffc9`) shows `coop_takes_two` (`0x0e0001ec`) for
  state 7 in co-op mode (`0x199994`), at `0x2500ab`.
- `pregame_lobby` (screen 14): pane 0 is multiplayer, pane 1 is co-op
  (`level_format` `0x0c00181a`, `difficulty_format` `0x1100181b`, the level
  description `0x1d00181c`). The player table has 16 cells on
  `player_skin_default`. The help text says to press Y for the friends
  list to invite. The level and difficulty screens are `load_level_lobby`
  and `difficulty_lobby`.

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
  with one player each (section 12). The
  session start that follows is the probe's synchronous path (section 9).
- The pinger thread's tick (`Lobby::sync_retail`): the host advertises
  the level and difficulty the lobby holds, so the guest's join reads
  them.

Ping: the lobby has no natural spot for it (the `active_protocol` block,
`0x0f001818`, is the nearest, and it says what the session is, not how
fast). Ping stays on the web.

### 11.3 What needs a game run, and the lines to look for

1. Pick Co-op Campaign on the Live menu. Expect
   `coop-live: Live menu row 4 opens the custom game lobby (row 2) in co-op mode`,
   `coop-live: hosting a co-op campaign in the game's own lobby` and
   `coop-live: the Co-op Campaign row's lobby: mode 2 (custom game) set as 0 (co-op campaign)`.
   The lobby should show pane 1 (level, difficulty, description).
2. Pick a level and difficulty in the game's own screens. Expect
   `coop-live: the lobby holds <level> on <difficulty>; advertised`.
3. Press Y, invite a friend; the friend accepts and lands in the same
   lobby, showing the host's level and difficulty and both gamertags.
4. Start Game. Expect
   `coop-live: lobby start state 7 (co-op wants one console) answered 9 for 2 consoles` (the number of consoles in the lobby),
   then the probe's session start lines (section 9.2).

Also to check: whether the Live menu's cursor comes back on Custom Game
(the press record was rewritten), and whether `[0x54D5A0]` matters on
this path.

### 11.4 Previews

`the_retail_co_op_lobby_on_live_drawn_from_the_disc` (in
`crates/boot/src/coop_live/tests.rs`) draws the lobby as host and as
guest, plus the level and difficulty screens, with
`ui_kit::preview::Disc::render_retail`. It needs `H2_DISC` and writes to
`H2_UI_PREVIEW_DIR`; never into git. The texts the game fills at run time
(gamertags, status, level) are stand-ins, and the level screen's list is
not drawn.

## 12. One command on one Mac: `h2 coop test`

Section 10.4 takes three terminals and a list of variables. `h2 coop test`
(`crates/cli/src/coop_test_cmd.rs`) does all of it and reads the logs back:

```sh
h2 coop test --disc disc/halo2.iso --auto
```

It:

1. Starts a Live server in its own process on `127.0.0.1:8075`
   (`--server-port`), with the file store under `--root` (by default
   `~/Library/Application Support/h2recomp/coop-test`), so accounts,
   friends and profiles last between runs. The web app is served there
   too when it is built.
2. Makes an account per game: Alpha hosts, Bravo joins (`--players N`, up
   to 16, adds Charlie, Delta and on through the NATO alphabet to Papa;
   past 2 the host is started with `H2_COOP_MAX_PLAYERS=N`, §13). Each game's key goes in its own Live
   directory (`<root>/<gamertag>/live`, given to the game as
   `H2_LIVE_DIR`), never in your own `accounts.json`. Every guest is made
   the host's friend. A second run reuses them; `--fresh` forgets the
   server and the accounts (not the game data).
3. Opens the host's window with `h2 play <disc> --data <root>/alpha/data
   --live-server http://127.0.0.1:8075 --gamertag Alpha`, and once it says
   `live: playing as Alpha` (after any build into `--out`) the guests'
   windows, the same way with their own data. `--dry-run` prints these
   lines, variables included, and starts nothing.
4. Once the host is at its main menu (it asks the host's agent socket:
   `session.wait {"phase":"menu","map_ticks":60}`, up to 10 minutes),
   prints one line saying what to press next, for example:
   `coop test: next, on Bravo's window: Xbox Live (to sign in). Then on
   Alpha's window: Xbox Live > Co-op Campaign > pick a level and
   difficulty; Bravo get the invitation and join by themselves; then Start
   Game on Alpha.` If the socket can't be asked, the same line comes with
   "once at the main menu".
5. Writes every line of each game to `<root>/logs/<time>/<gamertag>.log`
   and its co-op lines (`coop-live`, `coop-probe`, `coop-auto`, `skulls`,
   `session`, `game start: options`, `live: playing as`, `custom games`,
   `agent: listening`, panics) to `coop.log` there, as `[+SECONDS]
   GAMERTAG | line`, also shown on the terminal as they come. Boot-time
   dumps that only name a co-op hook are left out: the `feature plan:`
   lines, each hook's first `feature: ... active`, `health:` lines and the
   features' `hooks installed via feature plan` summaries.
6. When the last window closes (Ctrl-C closes them; twice exits at once),
   prints the checklist and the log's path.

### 12.1 What `--auto` does

The host's Live code invites the guests to its advertised lobby as soon as
it has one and they are online, and each guest's Live code accepts the
host's invitation as soon as it arrives, through the same requests as the
friends list's Invite and Accept (`crates/live-hle/src/auto.rs`; switched
on by `H2_COOP_AUTO_INVITE=<gamertags>` on the host and
`H2_COOP_AUTO_ACCEPT=<host>` on a guest). Once per lobby and per
invitation. They print `coop-auto: invited Bravo to session <id>` and
`coop-auto: accepting Alpha's invitation to session <id>`.

Menus are still yours: there is no input over the agent API yet (see
12.5). With `--auto`: on every window, Xbox Live (that signs the game in
to Live; until then the host sees nobody online and a guest can't take an
invitation); on the host, Co-op Campaign, a level and difficulty, then
Start Game. Without `--auto`, invite from the lobby's Y (friends list) and
accept on the guest, as on a console.

Not established without a game run: that the title reads an invitation
accepted this way (`XOnlineFriendsGetAcceptedGameInvite`, which the
retail flow reads after its own Accept) and joins at that moment, rather
than only when its friends list is open. The test
`coop_auto_invites_the_named_friend_and_the_guest_accepts_by_itself`
(`crates/live-hle/tests/server.rs`) shows the accepted record is there,
naming the host's lobby, without a press.

### 12.2 Two games on one Mac

Checked in the code, not by a run:

| Shared thing | How the games keep apart |
|---|---|
| XNet ports | Base 31000 and 31010 (`--port`, 10 up per game) on `--ip` (this Mac's LAN address, else loopback). Each game binds base to base+6 (title channels +0, +1, +5, +6; QoS and the co-op ping +2; the co-op channel +4); `NetworkConfig::validate` wants bases 7 apart on one IP. `H2_XNET_LOOPBACK_PEERS` names the others for discovery. |
| Data | `--data <root>/<gamertag>/data` each: profiles, settings, saves. |
| Live account | `H2_LIVE_DIR` each: `accounts.json` and `play-accounts.json` (the one account handed to the game). Without it two games on one Mac write the same `play-accounts.json`, and the one that reads last may sign in as the other. |
| The game image | One `--out` (default `$TMPDIR/h2boot`), read by both. Only the host may build it: the guests open after the host's `live: playing as`, which `h2 play` prints after its build. Set `H2_OPT` as you do for `h2 play`, or the image rebuilds. |
| Agent socket | `H2_AGENT` each: `~/Library/Caches/h2recomp/coop-<gamertag>.sock`. A live socket at a path is an error, so one each is required. `h2 agent --socket <path> status` (or `watch phase map`) reads each game. |
| Pipeline cache | `~/Library/Caches/h2recomp`'s pipeline store is made for several processes (`d3d::pipeline_store`). |
| Input | Each window takes the keyboard while focused. Pads go to the host (`--pad guest`: to the first guest); the other games get `--no-gamepads`. |
| Sound | The guests run `--no-audio`. |
| Map pack, Halo 1 | Not installed into the games' data (`--no-mappack --no-halo1`) unless `--extras`. |

Each window's title names its game and role (`Halo 2 — Alpha (host)`,
`Halo 2 — Bravo (guest)`; `h2 play --title`), and the windows sit side by
side across the main screen, the host leftmost: `h2 play --tile I/N`
sizes a window to column I of N (the largest 4:3 that fits under the menu
bar) and places it there (`app::tile`).

### 12.3 The checklist

`h2 coop verdict <coop.log> [--auto]` prints it again
(`crates/cli/src/coop_verdict.rs`). Each step is seen when every game it
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

A step not seen comes with the likely reason in plain words, most often a
press nobody made: for step 3, "Alpha never opened the Xbox Live menu" (no
`custom games:` line from the Live menu), "opened the Xbox Live menu but
never picked Co-op Campaign" (rows drawn, no co-op row chosen), "picked
another of its rows", or that the co-op hooks are off; for steps 4 and 5,
"picked Co-op Campaign but the lobby did not open in co-op mode"; for step
7, which guest never opened Xbox Live (so the host had nobody online to
invite); for step 9, accept by hand, or (with `--auto`) accepted but not
joined; for step 11, nobody pressed Start Game, or the lobby's start check
never asked about co-op. For steps 3 to 6, when the host walked none of
its path but a guest did (picked a lobby mode, a campaign's difficulty,
one of our Live rows), it says "you drove Charlie's window (it started a
campaign or lobby itself); drive Alpha (host), the leftmost window".

It ends with the first step not seen ("stopped at step 9: ...", with its
reason) and lines
that look like trouble (`ping: no answer`, `returned 0 (refused)`,
`could not invite`, `could not accept`, `did not open`, panics). What to
send back: `coop.log`, and the two whole logs if it stopped early.

### 12.4 What is tested

Without the game (`cargo test -p cli --bin h2 coop`, `cargo test -p
live-hle`): the arguments; each game's ports, data, Live directory, socket,
pads and `--auto` variables; the shell lines; accounts and friendships made
against an embedded server, reused on a second run, and a lost key saying
`--fresh`; two stand-in games (shell scripts) through the log merger with
the host's sign-in starting the next; the checklist from canned logs (a
whole run, a run that stopped, a run without `--auto`); the reason given
for each step a missing press explains (3: no Live menu, no co-op row,
another row, hooks off; 5; 7: a guest not in Xbox Live; 8; 9 with and
without `--auto`; 11; 1); the filter over the host's log of the
2026-11-17 three-game run, cut to its main menu
(`crates/cli/src/coop_verdict/host-at-menu.log`, our own lines, paths
shortened): only the co-op lines kept, no feature plan; the next-steps
line; the wait for the main menu against a stand-in agent socket;
`H2_LIVE_DIR`; and the automatic invite and acceptance between two HLEs
and a server.

### 12.5 Driving the host's menus (not built)

`--auto` stops at the menus. What exists to press through them:

- `--input-timeline FILE` (`input::backend::scripted::Timeline`,
  docs/input/input.md): pad frames at absolute guest times, played *instead
  of* live input, from launch. It knows nothing of the screen, so a press
  lands where the menu happens to be: the intro, the profile screen, the
  Live sign-in and the session made by Co-op Campaign take different
  times on every run (the host's first start here built for 20 minutes
  before its window even opened). The window's keyboard and pads are off
  while it plays.
- `H2_BOOT_INPUT`: presses counted in scheduler slices, for headless
  runs; the same blindness, and slices are not even time.
- The agent API (docs/runtime/agent-api.md) designs `input.pad` (timed
  packets through the normal input path) and the `ui` domain (which screen
  is open, which row has focus) and events like `screen`, but phase 1 has
  neither: only `session.*`, `world.*`, `profile.counters` and
  `features.list`. `session.wait` can wait for `phase: menu`, not for
  "the Xbox Live menu with Co-op Campaign focused".

What it would take, in order: (1) `input.pad` on the agent socket, a queue
the xapi input glue drains ahead of live input (the boot already merges a
keyboard pad for `H2_BOOT_INPUT`, so the path exists); (2) enough of `ui`
to know the screen and focused row: the open screen's tag id (the Live
menu is screen id `custom_games::SCREEN_ID`) and the list's cursor, read
on the tick like the other answers; (3) a short script in the harness:
wait for the main menu, press down to Xbox Live and A, wait for the Live
menu, move to the Co-op Campaign row and A, wait for `coop-live: hosting`,
then the level and difficulty screens, then Start once every guest is in.
Each step waits on what the screen says, not on time.

Risk: (1) is small and testable without the game; (2) needs the screen
stack's layout, which is partly traced (`layout_glue`, the screen loaders)
and would need a game run to confirm; (3) depends on row order, which our
own Live menu rows change (Co-op Campaign moved the rows down), and on
the profile screen, which a fresh data dir shows and a reused one may not.
A press in the wrong place is harmless in the menus but confusing in a
report. Until (1) and (2) exist, the line in step 4 above is the guide.

## 13. More than two players

Co-op over Live (sections 10 and 11) takes two consoles. This section is
what holds a campaign at two, what is lifted, and what is left. Built on
tests only; **no game was built or run for it**. Evidence as at the top:
**[local]** read in `h2 disasm`, counted on the disc, or in this repo;
**[guess]** inferred.

### 13.1 Where the game assumes two

The session and lobby:

| what | where | bound | evidence |
|---|---|---|---|
| a new session's limits | `0x59d0d`, `0x59d13` (and `0x6102e`): peers and players 16 | 16 | [local] |
| join check | `0x5af70` (called at `0x6304a`): peers + 1 within `S+0x4990`, players + n within `S+0x4994` | the limits | [local] |
| Live presence "joinable" | `0x63e00`: players under `S+0x4994` (`0x63e4f`), never for a campaign (`0x63e7a`) | campaign: none | [local] |
| max-players setter | `0x5c490`: writes only in states 5 to 8, so a lobby keeps 16 | | [local] |
| the host's game-start limits | `0x6e720`: a campaign gets player limit 2 (`0x6e8aa`) and peer limit 1 (`0x6e8eb`) | 2 | [local] |
| session start | `0x6ec80`: player count read once at `0x6ee69`; more than two refuses (`0x6ee9b`), more than one peer refuses (`0x6eea5`); the options' co-op byte is `count > 1` (`0x6ef61`); 16 session players copied into 16 options players (`0x6ee12`, `0x6efc8`) | 2 | [local] |
| lobby start check | `0x19a2ce`: the co-op check `0x19a3ed`..`0x19a41c` loops 16 slots, present (`0x19a951`) and local (`0x19ab77`), and wants exactly 2, both local | 2 on one console | [local] |
| lobby text | `coop_takes_two` (`0x0e0001ec`) for start state 7 in co-op mode (`0x2500ab`) | | [local] |
| lobby player table | 16 cells (11.1) | 16 | [local] |

The game itself:

| what | where | bound | evidence |
|---|---|---|---|
| players table | `PLAYERS` (`0x4e8c24`): 16 datums of `0x21c`; `players_globals` (`0x4e8c20`) holds 4 *local* player handles at `+0xc` (loops `0x12b8c0`, `0x138b70`) | 16 players, 4 local | [local] |
| per-tick player update | `0x14d3a0` (from `game_tick`, `0x13801f`): every player; a first spawn pending (flags `+2` bit 3) goes to the spawn each tick until it succeeds | 16 | [local] |
| spawn | `0x14f2e0`: unit type, location, unit; clears bit 3 (`0x14f6c1`); AL 0 when no location was free | | [local] |
| location pick | `0x14ef00`: best of `scnr+0x100` (`0x34` each) that passes the collision test `0x23bb70`, so a spot a biped stands on is skipped; `0xffff` when all are; no wrap | | [local] |
| starting locations | 00a: 1; every other campaign level: 2 | **2** | [local], counted on the disc |
| a third player at the start | waits, retried each tick, until P1 or P2 steps off a spot; nothing crashes | | [local] behaviour, [guess] timing |
| character | `0x14bc80`, `0x14f040`: every campaign player gets the representation of the first typed starting location (`matg+0x138`), so players 3+ are the same Chief or Arbiter as P1 | none | [local] |
| starting profile | `0x1529a0`: the 16 options players | 16 | [local] |
| co-op respawn | gates `0x138820` (co-op byte) and `0x1388a0` (not Iron, not Legendary); `0x14cdd0` walks every player: blocked while a delay (3), a game flag (2), the lost timer (8), `0xcc170` (4) or `0x1ca670` (5) holds; otherwise the anchor is the last player in table order whose unit is not busy, and every dead player is spawned then placed next to it with `0x14e970` (`0x14cfeb`, `0x14cffd`) | 16 | [local] |
| respawn "reason 6" | written in the anchor loop for a busy player (`0x14cf9a`), but the loop goes on and the respawn runs if *any* player can anchor; it only says why when none can. One busy player does not block the others | 16 | [local] |
| dead flags and game over | `0x152340`: `players_globals+4` all dead, `+5` some dead, over all 16; `0x138e40`: co-op loses when all are dead, solo, Iron and Legendary when any is | 16 | [local] |
| checkpoint safe | `game_safe_to_save` (`0x2a9bb0`) is `0x226190`: a run of checks, among them `0xcc170` (combat, as respawn reason 4), `0x14df40` and `0x14deb0` (each walks every player; `0x14deb0` refuses when any unit has object flag `+0x10a` bit 3), `players_globals+5` (some player dead) and the lost timer | every player | [local] |
| BSP switch volumes | `0x14d8b0` (from `0x138078`): every player's unit is tested against `scnr+0x130`; any player can switch | 16 | [local] |
| hs `players` | `0x2a0d10` calls `0x29f480`: every player *with a unit*, in table order | 16 | [local] |
| `volume_teleport_players_not_inside` | `0x2a0df0` calls `0x29f5b0`: every player outside the volume goes to the flag (`0x29ffb0`) | 16 | [local] |
| `object_teleport` | `0x2a24e0` calls `0x29ffb0` for its one object | 1 | [local] |
| the scripts' players | `player0` and `player1` in all 15 levels, `player2`/`player3` in none; `players` in 15, `player_count` in 13, `game_is_cooperative` in 11. Cutscene teleports (`object_teleport (player0) ...`, flags like `bridge_player0/1`, `boss_player0/1` in 04b, 05a, 05b, 08b) move two players | **2** | [local] tokens; [guess] that `playerN` is `(list_get (players) N)`, so with player 1 dead the script's `player1` is player 2 |
| `cinematic_stash_players` | 01b only | | [local] tokens |
| synchronous update | actions: 16 entries of `0x5c` (`0x138db0`; `0x14d6c3`); per-player entries 16 of `0x7c` at `+0x610` (1.3) | 16 | [local] |
| bandwidth | 16 × `0x5c` × 30 ticks ≈ 44 KB/s from the host to *each* client, ~0.66 MB/s up for 15 clients, if the encoder `0xb2710` sends entries raw; ~5.5 KB/s at 2 | | [guess] |
| difficulty, HUD by player count | none seen; only the co-op byte and difficulty are read on these paths | | [local], absence only |
| AI targeting | unread | | |

So the engine runs 16 players already. What holds at two is the data
(two starting locations) and the scripts (`player0`, `player1`).

### 13.2 What is built

**Stage 1a** (`coop.max_players`, `H2_COOP_MAX_PLAYERS`, 2 to 16, default
2, the host's value counts; `coop_probe`, `coop_live`):

- the session start's window shows one peer and, for more than two
  players, two players until `OPTIONS_NEW`, then puts both back (the
  co-op byte `count > 1` comes out the same);
- after `0x6e720`, a campaign's peer and player limits go to N;
- the joinable answer, the host setup and the lobby's co-op check take 2
  to N consoles with one player each; the blocker and the lobby line say
  "up to N";
- a guest takes the limit as 16: the host's lobby already held the start.

**Stage 1b** (feature `coop-players`, `crates/boot/src/coop_players.rs`,
on the co-op arm, so off outside a co-op session; `H2_FEATURES=-coop-players`):

- *Spawn.* A first spawn of a player of index 2 or more runs the spawn,
  and when its location pick found every spot taken, the pick answers
  spot `index % count`; the spawn is then followed by `0x14e970` next to
  player 0's unit (player 1's when player 0 is in a vehicle), all inside
  the one spawn call, as the co-op respawn does after its own spawn.
- *Stragglers.* A teleport (`0x29ffb0`) of the scripts' `player0` or
  `player1` (the first two players with a unit), a BSP switch or a
  cinematic's end marks the squad; at the next player update, outside a
  cinematic, every player of index 2 or more on foot and more than 8
  world units from the anchor is placed next to it with `0x14e970`.
- The hooks never read a per-console setting, only the game: they act on
  players of index 2 or more, who exist only when the host let a third
  console in. Every console runs them alike, which lockstep needs.

New points: `PLAYER_UPDATE` `0x14d3a0`, `PLAYER_SPAWN` `0x14f2e0`,
`PLAYER_SPAWN_LOCATION` `0x14ef00`, `PLAYER_PLACE_NEAR` `0x14e970`,
`SCRIPT_TELEPORT` `0x29ffb0`; global `SCENARIO` `0x4e0350`.

Tests (`cargo test -p boot --lib coop`): the fallback spot for players
3+ only; the tables read (players, objects, campaign, start count); the
anchor (player 0 on foot, else player 1); who straggles (index 2+, on
foot, past the leash; not player 1, not a driver, not the dead); the
scripts' `player0`/`player1` as the first two with units; the spawn
entry (original with the fallback armed, then the placement at player
0's position, nothing when the spawn failed); the gathering entry (the
stragglers placed, then the update); nothing moves with two players; the
feature on the plan with the co-op hooks.

### 13.3 What needs a game run

Three consoles (or instances), `H2_COOP_MAX_PLAYERS=3` on the host,
section 10.4's setup with a third account. Expect, besides 10.4's lines:

- host: `coop-live: co-op takes up to 3 players (coop.max_players)`;
  `coop-live: lobby start state 7 (co-op wants one console) answered 9 for 3 consoles`;
- every console: `coop-probe: session start: campaign with 3 peers: showing the start 1 peer until OPTIONS_NEW`,
  `coop-probe: session start: 3 players: showing the start 2 until OPTIONS_NEW`,
  `... peer count back to 3 before OPTIONS_NEW`, `... player count back to 3 before OPTIONS_NEW`,
  `session start returned 1 (the game starts)`;
- host: `coop-probe: game limits: campaign peer limit 1 -> 3`, `... campaign player limit 2 -> 3`;
- at the start of the level, every console:
  `coop-players: player 2: every starting location is taken; spawning on 0 and moving it`,
  `coop-players: player 2 spawned; placing it next to the anchor`,
  `coop-players: player 2 placed next to the anchor` (`REFUSED` means
  `0x14e970` found no room: it stays on the spot);
- after a cutscene or a script teleport, when the third player was far:
  `coop-players: 1 player(s) of index 2 or more left behind; bringing them over`.
- with the third player dead, or far away, at a checkpoint:
  `coop-players: checkpoint checks leave out player(s) 2 (dead, or past 8 units from the anchor)`,
  and the checkpoint is taken (`coop-probe: checkpoint save at tick T`
  on every console, at the same tick).

Then: does the third player appear next to player 0, does a dead third
player respawn (retail's co-op respawn, no line of ours), do all three
stay in sync (`OUT OF SYNC`, `h2 coop-probe diff` on three hash files,
pairwise), and do 03a's and 05a's opening cutscenes leave anyone behind.
Players 3+ are the same character as player 1.

### 13.4 Staged plan

1. **2 to 4 (now).** Stage 1a and 1b. Run 13.3 on 01a, 03a, 05a.
   The harness's `h2 coop test --players` (section 12) can allow up to `coop.max_players` once this lands.
2. **4, done properly.** Script handling for players 3+: for each
   level, the cutscene teleports of `player0`/`player1` and the
   `bridge_`/`boss_` flags (04b, 05a, 05b, 08b) get a third and fourth
   flag generated beside the first two (or the hook teleports players 3+
   to the same flag through `0x14e970`, which searches around it). The
   checkpoint rule (13.5, built). Which of the two characters each player is
   (a per-player representation, `player+0x88` read by `0x14f040`).
3. **8 to 16.** Measure the synchronous update's encoded size at 16
   (the encoder `0xb2710`); the host's upstream decides the cap. Pace:
   the slowest console gates every tick. AI targeting with many players
   (unread). Respawn anchors and leash in big open levels.

### 13.5 Risks and open items

- **Checkpoints with more players (built, option (a)).** `0x226190`
  (`game_safe_to_save`, point `SAVE_SAFE`) refuses a save while any
  player is dead (`players_globals+5`) or any unit is unsafe (`0x14df40`,
  `0x14deb0`, which walk the players' in-use bits). `coop-players` now
  counts players 0 and 1 always, and a player of index 2 or more only
  while alive and within the leash (8 units) of the anchor. For the
  length of the check, the players that do not count are taken out of
  the in-use bits (the datum iterator `0x16bc00` reads them too) and the
  dead flag is recomputed over the rest (only ever lowered); both go
  back when it returns. An unsafe player 3 near the anchor still blocks;
  a player 3+ in a vehicle, or with no anchor on foot, counts as retail.
  With two players nothing is left out: retail. Stderr, once per change
  of who is left out:
  `coop-players: checkpoint checks leave out player(s) 2 (dead, or past 8 units from the anchor)`.
  Tests: `coop_players::tests` (two players unchanged; a dead player 3 no
  longer blocks; a dead player 1 still does; a far player 3 does not
  block, a near or driving one counts; the bits and flag are put back).
- **The scripts know two players.** The leash covers being left behind;
  it does not put players 3+ into cutscene positions, and a script that
  checks `(volume_test_objects ... (players))` counts every player, so a
  straggler inside a volume can trigger it (as retail with two).
- **Two start spots.** Players 3+ start inside player 0's spot for no
  tick at all (placed within the spawn call), but if `0x14e970` refuses
  they stay overlapped until they move.
- **Respawn anchor** is the last player in table order who is not busy,
  so a respawn can land next to player 15 rather than player 0. Retail
  behaviour, now with more choices.
- **Bandwidth at 16** (13.1) and the lockstep pace.
- **Unread:** AI targeting with many players, the engine's handling of
  stragglers outside a new BSP, cinematic player handling beyond the
  stash, the update encoder.
