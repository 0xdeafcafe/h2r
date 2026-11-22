# Replay fidelity: the original 1 Hz recording

Evidence for [replay-fidelity.md](../replay-fidelity.md). Nothing here
states what the code does today; the design page does.

## 2026-11-18: measured from the code and four campaign games


From the code, and from four campaign games and the frame API read on
2026-11-18 (`GET /v1/games/{id}/frames`, `/v1/events?stream=game-…`).

### Rate and timing

| stage | rate | clock |
|---|---|---|
| game reads a snapshot | every tick (30 Hz) | engine tick |
| game builds and sends a frame | every even tick (15 Hz) | engine tick, now game tick |
| server fans out live frames | at most 15 Hz (66 ms throttle, wall clock) | throttle on arrival time |
| server **records** a `GameFrame` | the first frame it sends in each new Unix second | wall clock |

So recorded frames are about 1 Hz, but spaced by wall-clock arrival rather
than game time. Measured gaps between recorded frames, in ticks:

| game | frames | gap range | most common | outliers |
|---|---|---|---|---|
| 05a_deltaapproach `05f93f55…` | 48 | 20–34 | 26, 28 | 2 rewinds |
| 03b_newmombasa `953f4ebc…` (first 200) | 199 | 2–82 | 18–22 | 2, 56, 58, 82 |
| 05a_deltaapproach `3f23bbf0…` (first 200) | 199 | 2–92 | 28–32 | 2 rewinds |
| a10 `57b014ca…` | 36 | 18–60 | 30–32 | 1 rewind |

The gap is 30 ticks (one second) only on average. In a slow scene it runs
from 0.6 s to 3 s, and two frames can land 2 ticks apart across a second
boundary. The viewer interpolates linearly between whatever it has.

### Fields

| thing | captured | sent | notes |
|---|---|---|---|
| position | yes | yes | 1/512 unit when packed |
| yaw | yes | yes | 12 bits |
| aim (look, with pitch) | yes | yes | players, actors and vehicles; octahedral 2 × 12 bits |
| stance | crouch amount, physics mode | `stand`/`crouch`/`airborne`/`vehicle`/`dead` | derived per frame |
| speed | position over time | horizontal only | velocity is read (`Motion.velocity`) but not sent |
| animation state | no | no | the viewer guesses from stance, speed and actions |
| weapon held | yes | tag stem | |
| firing | shot count per weapon per tick | **a count per frame**, summed per recorded second | no tick for each shot |
| grenades | count, type | `grenade` action, type | no grenade object, no arc |
| melee, jump, reload, switch, pickup | inferred from state changes | `actions` per frame | to the frame, not the tick |
| vehicle | parent datum | vehicle id | no seat (driver, gunner, passenger) |
| health and shield | no | no | hits carry damage amounts only |
| deaths | `alive`, `PlayerKilled` with tick and place | yes | actors: `dead` stance |
| projectiles | no | no | the viewer makes them up from shots and ballistics |
| damage | each hit | `hits` per frame | no tick for each hit |
| powerups, objectives | no | no | camo, overshield, flags, bombs |
| scenery out of place | yes | `objects`, all of them in every recorded frame | |
| machines (doors, lifts) | yes | moved ones, all of them in every recorded frame | |

### Size

A recorded frame is stored as one packed keyframe (base64url) in a
`GameFrame` event, which the server signs.

| measure | value |
|---|---|
| packed recorded frame | 4–5.5 KB (5.3 KB in `953f4ebc…`) |
| as a log event on `/v1/events` (base64 data + JWS repeating it + envelope) | about 18 KB |
| as JSON on `/v1/games/{id}/frames` | 44–64 KB |
| share of the JSON frame that is `objects` | 53–93% |
| share that is players | about 1% |
| per minute of game, packed | about 320 KB |
| per minute of game, as log events | about 1.1 MB |

Out-of-place scenery is sent again in full every second (300 to 480
entries per frame in these games). That repetition costs most of the
bytes. The players, the part people watch, cost almost nothing.

### Other findings

- **The viewer downloaded every frame twice** (fixed in step 2). `createReplaySession` fetches
  `/frames?format=packed` and the game's whole event stream, which holds
  the same frames as `GameFrame` events at three times the size. For a
  20-minute mission that's about 6 MB plus 22 MB.
- **`/frames` as JSON fails on big campaign games.** For `06d67d77…` and
  `3f23bbf0…` it returned 500 and 502 with the default page of 600; with
  `limit=100` it took 3–4 s for 4–6 MB. The viewer asks for packed frames
  and isn't affected, but the public JSON read is.
- **`to_tick` assumes ticks only go up.** `recorded_frame_data` stops at
  the first frame past `to_tick`. On an old rewinding recording that drops
  later runs. New games keep their ticks in order. Nothing in the web app
  passes `to_tick`.
- **Recording can stop early.** `05f93f55…` ran for six minutes but has 32
  seconds of frames before it ended as `lost`. Something stopped sending
  (a crash or a quit, most likely). Worth a look on its own; it's out of
  scope here.

