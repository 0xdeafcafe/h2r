# Aim assist strength

Status: built and tested on synthetic tags. Confirmed in the game by the owner (2026-11-17).

The ask: aiming with the iOS on-screen touch controls is much harder than
with a stick, so the game's aim assist should be able to help more. The
**Aim Assist** setting (`controls.aim_assist`, a row on our Video screen,
`H2_AIM_ASSIST`) picks how strong it is:

| Value | Name in the file and variable | What it does |
|---|---|---|
| Auto (default) | `auto` | Strong while the touch controls show (`input::touch::controls_shown()`), else Default; in multiplayer, for the touch player only |
| Default | `default` (alias `retail`) | the tags' own values |
| Strong | `strong` | angles x1.5, ranges x1.25, `slowdown` and `follow` x1.25 |
| Very Strong | `very_strong` (alias `very-strong`) | angles x2, ranges x1.5, `slowdown` and `follow` x1.5 |

Code: `boot::aim_assist` (the hook and the memory walk),
`boot::setting_defs::AIM_ASSIST` (the setting). Without a settings store
(headless `h2 run`) the feature is off and everything is retail.

Labels: **engine** means read from the retail XBE's instructions with
`h2 disasm` over the Ghidra function list. The field names are ours, chosen
for what the engine does with each value.

## What Halo 2 reads

Halo 2 has two parts to its aim assist: a per-weapon cone, and global
"sticky crosshair" strengths.

### The weapon (`weap` tag)

| Offset | Field | Unit | What the engine does with it |
|---|---|---|---|
| `+0x208` | `cone_angle` | radians | Divided by the zoom magnification (`0x001a5137`); the cone a target must be inside for the shot to bend towards it |
| `+0x20c` | `cone_range` | world units | Multiplied by the zoom magnification (`0x001a5153`); how far the cone reaches |
| `+0x210` | `pull_angle` | radians | Read at `0x001a5164`, zoom applied; the wider angle in which the crosshair is slowed and dragged (`slowdown` and `follow` below) |
| `+0x214` | `pull_range` | world units | Read at `0x001a5175`, zoom applied; how far the pull reaches |
| `+0x218` | `bend_angle` | radians | Read at `0x001a5186`, which keeps max(this, `cone_angle`) as the bend limit; not changed by us |

Engine: `0x001a50a0` builds the aim-assist parameters for a held weapon. It
finds the weapon's definition through the tag entry table (`[0x4e3b44]`,
`+8` of the 16-byte entry), then at `0x001a5137` to `0x001a5181` reads the
four fields in that order, divides both angles by the zoom magnification
and multiplies both ranges by it, and at `0x001a5186` takes the larger of
`bend_angle` and `cone_angle`. Weapon tag flags bit 5 (`+0x12c`) is tested
just before (what it gates is unverified). So a wider `cone_angle` also lets
a shot bend further, as in retail when a tag's `cone_angle` is above its
`bend_angle`.

The offsets and names are pinned in `tags::object::layouts::WEAP` (`tags`
test `layout_pins`).

The melee fields are left alone: a longer lunge is a bigger change to play
than easier aiming.

### The globals (`matg` tag)

| Offset | Field | What it does |
|---|---|---|
| `+0xf0` | player-control block (count, address), elements of `0x80` | |
| element `+0` | `slowdown` | How much the crosshair slows over a target |
| element `+4` | `follow` | How much the crosshair follows a moving target |
| element `+8` | `unknown_8` | Read by the target search; not changed |

Engine: the player-control update `0x00185be0` keeps `[[0x4e034c]+0xf4]`
(the block's address) and at `0x0018622b` reads element `+0` and `+4`,
each raised to a per-target value and capped at 1.0 (`0x00186252`). The
target search `0x001a5c00` reads `+8` at `0x001a5c88`.

## What each level changes

Each value is retail times the factor, capped, and never below retail:

| | Angles (`cone_angle`, `pull_angle`) | Ranges (`cone_range`, `pull_range`) | `slowdown`, `follow` |
|---|---|---|---|
| Default | x1 | x1 | x1 |
| Strong | x1.5 | x1.25 | x1.25 |
| Very Strong | x2 | x1.5 | x1.5 |
| Cap | 0.5 rad (about 28.6 degrees) | none | 1.0 (the engine's own cap) |

A value that is already past a cap is kept. A value that is not a finite
positive number (a weapon with no pull has 0) is kept. The factors are
first guesses for touch; they are constants in `Level::factors`.

## When it applies

At every game activation (`GAME_ACTIVATE`, `0x18f000`, a `before` hook; the
scenario's cache is loaded by then), so a change of the setting, or the
touch controls showing or hiding, takes effect at the next map load. The
setting is `Apply::Live` so the hook sees a new value at once; the row
does not say "(restart)".

The walk goes over every tag entry (count at `[[0x547f00]+0x18]`): each
`weap` entry with a definition of at least `0x218` bytes gets its four
fields set, and the `matg` entry its player-control elements (at most 16).
Only guest memory changes; the map file on disk is never written.

**Never compounding.** For each field we keep the retail value and what we
wrote, keyed by (tag datum, address). A field still holding what we wrote
is recomputed from the kept retail value; any other value is a fresh load
(a new map, or a revert that reloaded the cache) and is taken as retail.
Default writes the kept retail values back and forgets them. Tests:
`reapplying_and_switching_never_compound_and_default_restores`,
`a_reloaded_map_is_its_own_retail`.

## Multiplayer

Status: built and tested on synthetic memory. Confirmed in the game by the owner (2026-11-17).

The weapon and globals tags are shared by every player a machine
simulates, so in a multiplayer game (game options mode 2, `[eax]` at
activation) the activation hook keeps them retail whatever the setting
says. The touch player gets stronger aim assist per player instead, from
that player's own machine, and every other player keeps retail values bit
for bit.

### Where each value is used

Engine, read with `h2 disasm` and a scan of `.text` for calls:

| Value | Read by | Side |
|---|---|---|
| `cone_angle`, `cone_range`, `pull_angle`, `pull_range` (`weap +0x208..+0x214`) | `0x1a50a0` builds the held weapons' cone (0x24 bytes at ESI, below). Its two callers: the aim-assist search `0x1a4900` (call at `0x1a49a1`), and the shot aim `0x1a4c00` (call at `0x1a4cbf`) | `0x1a4900`: input. `0x1a4c00`: simulation |
| `slowdown`, `follow` (`matg +0xf0`, element `+0`, `+4`) | the player-control update `0x185be0`, at `0x18622b` | input |
| `unknown_8` (element `+8`) | the target search `0x1a5c00` (`0x1a5c88`), under `0x1a4900` | input; not changed |

- **Input.** `0x1870a0` walks the four local users (`[0x4e8c20]+0xc`), and
  for each bound one calls `0x186ab0` (`0x1870db`), which calls the
  player-control update `0x185be0` (`0x186b16`; stdcall, five stack
  words, the local user first, the player's controller index, player
  `+0x24`, in EDX; `ret 0x14`). That builds the player's action, the
  input handed to the simulation (`[0x4cf77c]`). Inside it, `0x1a4900`
  (local user in EAX, per-user control `[0x4ed284]` stride `0x94`) asks
  `0x1a50a0` for the cone, runs the target search (`0x1a5370`), and writes
  the target and the aim-assist levels into the action (`+0x28`); then
  `slowdown` and `follow` slow and drag the look. None of this runs for a
  remote player: a remote player's action arrives over the network.
- **Simulation.** The barrel fire `0x104150` calls `0x1a4c00` (`0x104886`)
  to aim the shot. With the simulation mode byte (`[[0x4e6948]+0xc]`) 4 or
  5 (distributed client or server) and bit `0x2` of the aim data's `+0x18`,
  it aims straight at the position at `+0xc` with no tag read
  (`0x1a4c4b..0x1a4c95`). Otherwise it calls `0x1a50a0` for the shooter's
  unit and bends the shot within that cone (`0x1a6a80`). Where bit `0x2`
  and the position are written is not traced.

`0x1a50a0`'s result: `+0` `cone_angle` (divided by the zoom
magnification), `+4` `cone_range` (times it), `+8` `pull_angle`, `+0xc`
`pull_range` (the larger of both hands' when dual wielding), `+0x10` the
bend limit (the larger of `bend_angle` and `cone_angle`, zoom applied),
`+0x14` flags, `+0x18` and `+0x1c` the second weapon's `cone_angle` and
`cone_range`, `+0x20` a copy of `cone_range` for some weapon classes
(`0x1a51de`), else 0. Stores at `0x1a514f..0x1a5181`, `0x1a51a5`,
`0x1a5292`, `0x1a52a2`. Stack word: the zoom level; `ret 4`.

### What we change

All of it is input side, around one local player's control update
(`boot::aim_assist::player`):

- `before` `0x185be0` (`PLAYER_CONTROL_UPDATE`): pick the player's level
  (below). For anything but Default, open a window and set the globals'
  `slowdown` and `follow` to retail times the level's factor (capped at 1.0).
- `after` `0x1a50a0` (`AIM_ASSIST_PARAMS`), inside the window only: scale
  the result as the tag path would (angles, capped at 0.5 rad; ranges;
  the bend limit raised to the scaled `cone_angle` when larger). Outside
  a window (the shot aim, any other player) nothing is touched.
- `after` `0x185be0`: put `slowdown` and `follow` back bit for bit (a field
  that no longer holds what we wrote is left alone) and close the window.

No weapon tag is written. The cap is on the angle after zoom, where the
tag path caps the tag's own angle; the two differ only for a zoomed weapon
whose scaled angle would pass 0.5 rad.

**Who gets it.** In a multiplayer game only, the local player whose
controller is port 0 (the touch controls drive port 0, `input::touch`)
while the touch controls show:

| Setting | Touch player | Everyone else |
|---|---|---|
| Auto | Strong | Default |
| Default | Default | Default |
| Strong | Strong | Default |
| Very Strong | Very Strong | Default |

Campaign and co-op keep the tag path above, and the per-player path never
opens outside a multiplayer game, so the two never stack.

### Why peers agree

The boost only changes what the touch player's own machine computes as
that player's input: where the crosshair drags, which target the search
picks, and the aim-assist levels in the action. Every machine then simulates
from the actions it is sent. Everything the simulation reads itself (the
shot aim's call of `0x1a50a0`, and the globals outside the window) is
retail on every machine, so two peers given the same actions compute the
same results. The test `two_peers_simulate_the_same_values` plays a host
with the touch player and a guest with two pads and checks it.

### Not done: the simulation side for remote touch players

When `0x1a4c00` recomputes the cone (not the distributed fast path), the
shot is bent within the retail cone, also for the touch player, on every
machine. Giving the touch player the wider bend there needs every machine
to know which players are on touch:

- each guest would send its touch state to the host (nothing carries
  guest-to-host data today: the multiplayer skulls go host to guests
  only, through `NewSession.skulls` and `Update.skulls`, see
  [skulls.md](skulls.md) "In multiplayer");
- the host would advertise the set per player (XUID or player index) in
  the session data, and each peer map it to a player datum at game start;
- System Link, with no server, would have no path at all.

It is also likely to matter little in Halo 2's distributed multiplayer,
whose fast path aims at the target the shooter's own machine chose
(above), when the shooter's machine sets bit `0x2`. Confirming that
needs the writer of bit `0x2` found, or a look in a system link game.

### Before this

The first version kept multiplayer fully retail: the setting was
`Scope::Sim` and the activation hook used Default in mode 2. That still
holds for the tags.

## Testing

- `cargo test -p boot aim_assist`: the factors and caps
  (`scaling_caps_angles_and_never_weakens`), Default leaving every byte alone,
  Strong and Very Strong over a synthetic tag index with three weapons, a
  biped and a globals tag, no compounding, a reloaded map, odd tables, Auto
  following `input::touch::set_controls_shown`, and multiplayer staying
  retail. The offsets are checked against the `tags` layout
  (`the_offsets_are_the_tag_layouts`).
- `cargo test -p boot aim_assist::player`: the per-player level (touch,
  pad, remote, campaign, hidden controls), the result scaling against the
  tag factors, `slowdown` and `follow` put back bit for bit, a pad player
  and every shot seeing retail bytes, a reload inside the window, odd
  globals, the two paths never stacking, two simulated peers, and the
  two points' signatures.
- `cargo test -p boot setting_defs`: `aim_assist_is_auto_unless_asked`
  (default Auto, the file's names, the variable's names and lock, a press
  stepping Auto, Default, Strong, Very Strong).
- On the retail disc (values printed, not kept):
  `H2_DISC=/path/halo2.iso cargo test -p boot retail_tags_hold_plausible -- --ignored --nocapture`.
- In game: set `H2_AIM_ASSIST=very_strong`, load a campaign level; stderr
  says `aim-assist: Very strong (N weapons, 1 player controls, M fields
  written)`. The crosshair should turn red further from an enemy and drag
  harder over one. Back to `default`, the next load writes retail back.
