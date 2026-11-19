# The debug menu

Status: built and unit tested (the model, navigation, gestures, input
swallowing, layout, drawing on a headless GPU, the cheats over a fake game).
It has not yet been seen in the game; "To check in the game" lists what to
look at.

The retail game has no debug menu, so this one is ours. It is an overlay the
host draws over the game image, like the perf watermark, in the same 8x8 font.
It reads and changes the same registries `h2 agent` does. Every row is an
agent answer, and every change is an agent call. A change made in the menu
shows in `h2 agent`, and a change made by `h2 agent` shows in the menu.

It is on in `h2 play` and `h2 run` windows unless `H2_DEBUG_MENU=0`.

## Controls

| | DualSense / DualShock 4 | Any pad | Keyboard |
|---|---|---|---|
| Open | touchpad click (or touchpad click + Options) | | `` ` `` (backquote) |
| Close | two-finger tap at the top, or touchpad click + Options | Circle / B at the top | `` ` ``, Escape at the top |
| Move the selection | swipe up / down on the touchpad | D-pad up / down | arrows up / down |
| Change a value | swipe left / right | D-pad left / right | arrows left / right |
| Select or enter | touchpad click | Cross / A | Enter, Space |
| Back | two-finger tap | Circle / B | Escape, Backspace |

The D-pad and face-button fallback means a pad without a touchpad can still
use the menu once it is open, but only a DualSense, a DualShock 4 or the
keyboard can open it.

- **The click alone.** The game has no use for the touchpad click, so a
  click on a closed menu opens it. That press neither selects a row nor,
  with Options added, closes the menu again; the next click selects.

- **The chord.** A pad with a touchpad holds Options back for 80 ms
  (`debug_menu::gate::CHORD`) in case the click follows. If the click doesn't
  come, the game gets Options then. A press shorter than that still reaches
  the game, one sync late. Pads without a touchpad pass Options straight
  through.
- **Swipes.** A swipe is 0.4 of the touchpad's half-width (`SWIPE` in
  `input::touchpad`). Each 0.4 travelled is one step, so a long swipe moves
  several rows. A second finger is one back, and the rest of that touch does
  nothing.
- **Swallowing.** While the menu is open, the game sees every pad as neutral:
  no buttons, sticks centred, triggers released. Opening the menu releases
  whatever was held, so a player doesn't keep running or firing. After it
  closes, anything still held stays hidden until it is let go. The Circle
  that closed the menu never melees, and the Options from the chord never
  pauses the game. While the menu is open, the keyboard keys above go to the
  menu, and every other key is swallowed.
- **Values.** A toggle flips with left, right or select. A number steps by
  its own step, clamped to its range. A choice cycles through its options.
  An action (reload, revert) runs on select. Greyed rows say why in the
  footer, and refuse to change.

## Sections

Each section is a `debug_menu::Section`: the agent calls it makes, and a
function from their answers to rows (`crates/debug-menu/src/sources.rs`).
The menu re-asks every second while a page is open, and right after any
change.

| Section | Calls | Rows |
|---|---|---|
| Render | `render.toggle`, `render.param`, `settings.list {prefix: "graphics."}`, `render.stats` | the renderer's switches and parameters (a parameter steps by a tenth of its default), the graphics settings, the last frame's stats (read-only). A setting that applies after a restart is greyed with that reason, and so is one locked for this run by a flag or env var. |
| Shader overrides | `render.overrides`, `render.reload` | the all-overrides switch, reload, the overrides directory, each override and its state; a rejected override is greyed with its error |
| Tag edits | `edit.changes`, `edit.revert` | each live tag edit (group, tag, field, old -> new). Select one to revert it, or revert all. |
| Cheats | `cheat.list`, `cheat.set` | god mode, infinite ammo, bottomless clip, game speed, fly, noclip, hide HUD; a cheat refused on its own is greyed with its reason |
| Perf | `perf.overlay`, `profile.counters` | the perf overlay switch (the same one F3 flips), fps and frame-time percentiles, per-phase ms and counts |
| Features | `features.list` | every feature with its state (read-only) |

## Cheats

The retail game has no usable cheat commands, so these are our own
(`crates/boot/src/cheats.rs`). They are applied after each game tick to the
local players' units, with the helpers our custom skulls use:

| Cheat | What it does |
|---|---|
| `god` (god mode) | sets the body's cannot-take-damage flag; switching off clears it only on the units we set it on |
| `infinite_ammo` | each magazine's reserve to its maximum, an energy weapon's charge to full, both grenade counts topped; reloads still happen |
| `bottomless_clip` | each magazine's loaded rounds to its maximum, so no reloads (plasma heat still builds) |
| `game_speed` | 0.25x, 0.5x, 1x, 1.5x or 2x game time, set the way the game's own set-speed helper sets it; a new map puts it back to 1 |
| `fly` | flight through the game's own physics (walls still stop you); controls below |
| `noclip` | the same flight through walls; the body takes no damage while it is on |
| `hide_hud` | the HUD hidden through the game's own HUD switch (the flag a script's HUD builtin sets); the game's last value goes back when off |

### Fly and noclip

Both read the player's own controls from the unit after the tick: the
control flags (crouch, jump), the move stick (throttle) and the aiming
vector. With either on:

- **Move stick**: fly where you look. Forward follows the view, up and down
  included, so look up and push forward to climb. Sideways stays level.
- **Jump**: climb. **Crouch**: sink.
- **Speed boost**: hold the stick all the way for half a second and the
  speed ramps from 5 to 20 world units a second over the next second. Let
  go and it drops back.
- **Fly only, take off**: on the ground, a crouch press takes off (a lift
  of 0.1 and a climb). No jump is needed.
- **Fly only, land**: sink onto a floor with crouch held. Crouch still held
  after landing does not take off again; a fresh press does. So with fly
  on, crouching on the ground always takes off: switch fly off to
  crouch-walk.
- **Fly off in the air**: you drop, shielded from damage for three seconds.

Noclip flies from the moment it is on, with no take-off. It holds the
body's position itself and writes it over the physics' each tick, so
nothing stops it. Turning it off hands the body back to the physics where
it is. For three seconds after, the body is shielded and watched. If it
does not move for half a second while airborne or while you push the stick
(stuck in a wall), or falls 20 units, it is put back on the last floor the
physics stood it on (where noclip began, if none since). With fly on too,
turning noclip off leaves you flying, with walls again.

Fly sets the body's velocity, the way our Acrophobia skull does. Noclip
writes the position. Both act only on a biped on foot: in a vehicle, the
vehicle moves you. The research points are in the module doc.

### Who may cheat

- **Off Xbox Live** (offline, split screen, System Link): anyone.
- **On Live** (a local player signed in to our Live service): only the
  gamertags on `cheats::policy::LIVE_ALLOWED` (today `afr`, matched
  case-insensitively and trimmed). Every signed-in Live player on this
  machine must be on it. Otherwise every cheat is refused with "cheats are
  off on Xbox Live". The gamertags come from the Live client's signed-in
  xuids, named from this machine's accounts file; a xuid it can't name
  refuses.
- **Whoever you are**, where a cheat would break the run:

| Session | Refused | Allowed |
|---|---|---|
| lockstep run (`H2_LOCKSTEP`, input record or replay) | every cheat | none |
| synchronous co-op (simulation world modes 2, 3) | all but `hide_hud`: only this machine's simulation would change, and the players would desync | `hide_hud` |
| a networked game's guest (world mode 5) | all but `hide_hud`: the host's simulation decides | `hide_hud` |
| a networked game's host (world mode 4) | `game_speed`: the guests' clocks would not follow | god, ammo, clip, fly and noclip on the host's own players; `hide_hud` |

They act only in a campaign or multiplayer game, not on the main menu.
When every cheat is refused, the menu shows "cheats are off" with the
reason and greys every row. A cheat refused on its own greys only its row,
with its own reason. `cheat.set` answers `UNAVAILABLE` with the reason. If
the session turns into one that refuses a cheat while it is on (someone
else signs in to Live, a co-op session starts), it is switched off and
undone.

`h2 agent cheat.set '{"name":"noclip","value":true}'` does the same as the
menu.

Not built:

- infinite plasma heat: the heat fields are not researched;
- AI ignores the player, active camouflage, one-hit kills, kill all
  enemies, give weapons, spawn vehicles, teleport to the crosshair, a free
  camera: each needs more research, or calls into the game (object
  creation, ray casts) that these cheats do not make yet;
- low gravity: our Low Gravity skull already owns the gravity constant, and
  two writers would fight over it.

## Adding an item

Rows are not hand-coded. They come from a registry through an agent method,
so an item belongs to one of these:

1. **An entry in an existing registry.** A `hot_render::Hub` toggle or
   parameter (`hub.declare_toggle` / `declare_param`), a `settings::Def`
   under `graphics.`, a feature: it shows up in its section with no menu code.
2. **A new row kind in an existing section.** Change that section's rows
   function in `sources.rs`, using `Row::toggle`, `int`, `float`, `choice`,
   `action` or `info`, with a `Setter` naming the agent method and parameter
   that changes it. Mark it `.disabled(reason)` when it can't change, and
   `.about(text)` for the footer.
3. **A new section.** Add the host methods to the agent
   (`agent.add_method`, on the game's thread), then a `Section { id, title,
   about, calls, rows }`. Add it to `sources::all()` for every game, or with
   `Shared::add_section` at run time.

The hook for a later Players section (`sim.*`) is
`debug_menu::Shared::add_section`: give it the `sim.*` calls and a rows
function, and it appears on the first page after the built-in sections.

Test new rows in `crates/debug-menu/src/tests.rs`: the fake game there
answers the calls the way the host methods do.

## How it is wired

- `crates/input`: the DualSense and DualShock 4 touchpad
  (`HostPadState::touchpad`, read in `game_controller/apple.rs`, never mapped
  to the guest), gestures (`touchpad.rs`), and `InputManager::set_gate`. A
  `PadGate` sees each pad between polling and conversion.
- `crates/debug-menu`: the model (`model.rs`), sections (`sources.rs`), layout
  (`layout.rs`), the gate (`gate.rs`) and `Shared`, the handle the window,
  the input layer and the agent connection share.
- `crates/agent/src/menu.rs`: the menu's in-process agent connection, plus
  `settings.list`/`settings.set` and `perf.overlay`.
- `crates/boot/src/debug_menu_glue.rs`: installs those methods. A game
  without `H2_AGENT` still gets a socketless agent for the menu. Its shader
  overrides are not loaded or watched until asked.
- `crates/app/src/menu_overlay.rs`: the drawing (top left of the game image,
  one step bigger than the watermark, a dark plate, the selected row on a
  bar). `keymap.rs` has the backquote key, and `shell.rs` routes keys and
  redraws.
- `crates/cli/src/play_cmd.rs`: makes the menu, sets the gate, hands it to
  the window and the game.
- With the keyboard on (the default), player one is
  `input::backend::fallback::KeyboardFallback`: the first pad merged with the
  keyboard through the guest's pad. The merge carries the pad's touchpad
  across, so the gate sees its click. It used to drop it, so neither the
  click nor the chord opened the menu from player one's DualShock 4 or
  DualSense (fixed 2026-11-19; test
  `a_touchpad_click_on_player_ones_pad_opens_it_through_the_input_manager`).

## When a pad doesn't open it

`h2 agent hostpads` (`input.host`, [agent-api.md](agent-api.md#inputhost-the-host-controllers))
shows each controller three ways. Run it while the touchpad is held down:

- `devices`: the controller as GameController reads it. `profile` must be
  `dualsense` or `dualshock4` for it to have a touchpad. `extended` means
  the framework gave it a plain profile, so there is no touchpad to read.
  `touchpad.click` should be `true` while the pad is held.
- `pads`: what the gate is handed. Player one's touchpad should match the
  one under `devices`.
- `gate`: `open`, and per pad `touchpad`, `click`, `chord`, and
  `chord_pending` (Start waiting for the click).

`notes` says in words where the touchpad goes missing.

## To check in the game

- The chord opens and closes the menu on a DualSense and on a DualShock 4,
  and Options alone still pauses the game.
- The DualShock 4's Options is GameController's menu button. If it isn't,
  the chord won't fire.
- Swipe direction: GameController reports +y up. If up swipes move the
  selection down, flip the sign in `input::touchpad`.
- A lifted finger reads (0, 0) and makes no stray swipe.
- Opening the menu while running and firing stops both. Closing it with
  Circle doesn't melee.
- God mode, infinite ammo, bottomless clip and each game speed in a campaign
  level. Game speed during a cutscene, where scripts ramp it, and on the next
  map.
- Fly: a crouch press on the ground takes off (if it does nothing, the
  ground physics drops our velocity); the stick flies along the view, and
  stick left goes left (the throttle is read as forward, left); jump
  climbs, crouch sinks; sinking onto a floor with crouch held lands.
- Noclip: through a wall and out the other side; off in the open falls;
  off inside a wall puts you back on the last floor after half a second;
  no damage meanwhile.
- Hide HUD, and that a cutscene's own HUD switch is kept after it is off.
- The cheats are refused in a co-op session, and on Live for any gamertag
  but afr.
- The perf overlay switch matches F3.
- Shader overrides reload from the default directory.
