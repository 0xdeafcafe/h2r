# The debug menu

Status: Built. Unit tested: the model, navigation, gestures, input
swallowing, layout, drawing on a headless GPU, the cheats over a fake game.
Not seen in a running game; "Not verified" lists what only a run can show.
Code: `crates/apps/cli/src/play_cmd.rs` (the overlay is created in `h2 play` and
`h2 run`), `crates/extensions/agent/src/hostpads.rs`.
Switch: `H2_DEBUG_MENU` (on), listed in [switches.md](switches.md).

The retail game has no debug menu, so this one is ours. It is an overlay the
host draws over the game image, like the perf watermark, in the same 8x8 font.
It reads and changes the same registries `h2 agent` does. Every row is an
agent answer, and every change is an agent call. A change made in the menu
shows in `h2 agent`, and a change made by `h2 agent` shows in the menu.

It is on in `h2 play` and `h2 run` windows unless `H2_DEBUG_MENU=0`.

## Controls

| | DualSense / DualShock 4 | Any pad | Keyboard |
|---|---|---|---|
| Open | touchpad click (or touchpad click + Options) | hold Back (View) + D-pad down for 0.5 s | `` ` `` (backquote) |
| Close | two-finger tap at the top, or touchpad click + Options | Circle / B at the top | `` ` ``, Escape at the top |
| Move the selection | swipe up / down on the touchpad | D-pad up / down (hold repeats), wraps at the ends | arrows up / down, wraps |
| Change a value | swipe left / right | D-pad left / right (hold repeats) | arrows left / right |
| Page up / down | | LB / RB | PageUp / PageDown |
| First / last row | | | Home / End |
| Select or enter | touchpad click | Cross / A | Enter, Space |
| Back | two-finger tap | Circle / B | Escape, Backspace |
| Star or unstar the row | | Triangle / Y | F |
| Search every section | | | type, or `/` first |

Any pad can open and use the menu. The open chord is Back and D-pad down:
none of the layouts in [controller-layouts.md](../input/controller-layouts.md)
gives either an action in the table there, and nobody holds both for half
a second by accident. The game keeps the pair until the menu opens. After
that it is hidden from the game until both are let go, and the D-pad down
that was part of the chord does not move the cursor.

- **The click alone.** The game has no use for the touchpad click, so a
  click on a closed menu opens it. That press neither selects a row nor,
  with Options added, closes the menu again; the next click selects.
- **The chord.** A pad with a touchpad holds Options back for 80 ms
  (`debug_menu::gate::CHORD`) in case the click follows. If the click doesn't
  come, the game gets Options then. A press shorter than that still reaches
  the game, one sync late. Pads without a touchpad pass Options straight
  through. The Back + D-pad down hold (`debug_menu::gate::HOLD`, 0.5 s)
  works on every pad.
- **Hold-repeat.** A held D-pad direction moves once at the press, then
  repeats every 80 ms after 350 ms (`REPEAT_DELAY`, `REPEAT_EVERY` in
  `gate.rs`). The keyboard repeats at the OS's key-repeat rate.
- **Wrap and paging.** Up from the top goes to the last row, down from the
  bottom to the first. A page is 14 rows (`debug_menu::model::PAGE`) and
  stops at the ends.
- **Swipes.** A swipe is 0.4 of the touchpad's half-width (`SWIPE` in
  `input::touchpad`). Each 0.4 travelled is one step, so a long swipe moves
  several rows. A second finger is one back, and the rest of that touch does
  nothing.
- **Swallowing.** While the menu is open, the game sees every pad as neutral:
  no buttons, sticks centred, triggers released. Opening the menu releases
  whatever was held, so a player doesn't keep running or firing. After it
  closes, anything still held stays hidden until it is let go. The Circle
  that closed the menu never melees, and the Options from the chord never
  pauses the game.
- **Values.** A toggle flips with left, right or select. A number steps by
  a step scaled to its range (the range over 64, rounded to 1, 2, 5 or 10
  times a power of ten, and never finer than the row's own step) unless the
  row sets one, clamped to its range. Select on a number with a default puts
  it back. A choice cycles through its options. An action (reload, revert)
  runs on select. Greyed rows say why in the footer, and refuse to change.
- **Confirm.** A destructive row asks first: the first select shows
  "press again to <label>", and a second within 2 s makes the call.
- **Results.** A change's result is one line under the menu for 3 s: the
  row's own text for its result (`Row::on_ok`), else "<label>: done" for an
  action and "<label>: <value>" for an edit. Errors show the same way, in
  red. The overlay also draws the line for those 3 s after the menu closes.
- **Footer.** The last line teaches the controls of the device last used
  (keyboard, a touchpad pad's touchpad, or a pad's buttons). The line above
  it is the call the selected row makes, as the agent API would send it, so
  the menu shows the matching `h2 agent` call.
- **Header.** `Menu::set_header` pins a section above every page. Its rows
  are drawn as `label value` pairs on up to two lines, listed every second
  while the menu is open.
- **Favourites and Recent.** Star the selected row with Triangle / Y or F.
  The first page lists starred rows under Favourites and the last five
  actions or edits under Recent, above the sections. A row is keyed by its
  section's id and its label. They live in memory, for the run.
- **Search.** With the menu open, a letter, digit or `/ - .` starts a
  search over every section's rows; the list becomes the matching rows,
  labelled "Section > row". `f` stars a row and Space selects until the
  search has started; `/` starts it without a letter, so `f` can be the
  first one. Escape clears the search, and only then goes back. Backspace
  erases a letter, and goes back with none. Modifier and function keys are
  not the menu's.

## Keyboard

The keys are the Keyboard column of the table above, mapped by
`debug_menu::Key` (`crates/extensions/debug-menu/src/lib.rs`, from
`keymap::menu_key` in `crates/apps/app/src/keymap.rs`). While the menu is
open the arrows, PageUp/PageDown, Home/End, Enter, Space, Escape and
Backspace act on it, and letters, digits and `/ - .` go to its search. Modifier
keys and F1 to F12 are not the menu's and pass as before; every other key is
swallowed.

## Sections

The first page is organised by what you are doing, not by agent namespace:
Player, World, Game, Time, View, Perf, Settings, Dev. Each is a
`debug_menu::Section`: the agent calls it makes, and a function from their
answers to rows (`crates/extensions/debug-menu/src/sources/`, one file a
section). A page reached from another (a weapon picker, the Video settings)
is a sub-page: its id has a `/` (`player/give`), a row of its parent opens
it, and `sources::is_page` tells the two apart. The menu re-asks every
second while a page is open, and right after any change.

Rows read as a person would say them: a human label with its unit
(`Range (world units)`, `Frame time p99 (ms)`), never a JSON key or a hex
id. A tag is named by the last part of its path (`Battle Rifle`). A row
that can't act is greyed with the reason, and a row that changes the game
for good (load, restart, quit, revert all) asks twice. What a change did
shows as a one-line result.

A pinned line over every page (`sources::header`, from `session.status`,
`sim.view` and `sim.session`) says where you are:
`coagulation  tick 5412  30 fps  pos 36.7 -80.8 1.7  offline`.

Writes to the simulation (give, spawn, teleport, load, restart) are host
writes: `sim.session` refuses them in a co-op session and on a multiplayer
client. Those rows are greyed with its reason, which the session gives in
its own words.

| Section | Calls | Rows |
|---|---|---|
| Player | `cheat.list`, `cheat.set`, `sim.session`, `session.status` | God mode, Infinite ammo, Bottomless clip, Fly, Noclip, Give weapon >, Teleport >, Hide HUD. At the main menu every cheat says no game is running instead of looking accepted. |
| Player > Give weapon | `edit.tags {group: "weap"}`, `sim.view`, `sim.give` | the map's weapons by name; select one to put it in the local player's hands (it replaces what they hold). The result reads `gave <name>`. |
| Player > Teleport | `sim.view`, `sim.state.set` | each other player by name; select one to move beside them, facing the way they do. |
| World | `sim.session` | Spawn vehicle >, Spawn object > |
| World > Spawn vehicle, Spawn object > (Weapons, Equipment, Scenery, Crates, Machines, Creatures) | `edit.tags {group}`, `sim.view`, `sim.spawn` | the map's tags of that kind by name; select one to make it 5 world units (vehicles) or 3 (objects) in front of the local player, half a unit up, facing the way the player does. |
| Game | `state.list`, `sim.session`, `session.status`, `state.save`, `state.load`, `map.reload`, `game.menu` | Quick save (`state.save {name: "quick", replace: true}`), Quick load (confirm; greyed until a `quick` snapshot exists), Snapshots >, Restart map (confirm), Test maps >, Quit to main menu (confirm), Game code >. A save or load that finds the machine busy fails with the agent's BUSY message, which names what is live. |
| Game > Snapshots | `state.list`, `state.load` | each snapshot of this build, with its map, tick and size; select one twice to load it |
| Game > Test maps | `maps.test.list`, `maps.test.load` | each test map; a ready one loads (confirm), one being built or that failed is greyed with why |
| Game > Game code | `game.code`, `game.code.restart` | Running (full speed, or the slower start on the fast tier), Full-speed build (building with minutes in and about how many left, ready, failed or not needed), the build log's last step and path, the module and its stamp, and Restart at full speed (confirm), greyed with why until the full-speed build is ready |
| Time | `cheat.list`, `cheat.set` | Game speed, over the speeds the cheat lists. Pause and frame step join it when the agent has a method for them. |
| View | `debug.views`, `debug.view` | the world debug views below, each with its own switch; Range (world units, steps of 8, default 48); Trace lifetime (ticks); Render passes >; Graphics settings > (the Video page of Settings) |
| View > Render passes | `render.toggle`, `render.param` | the renderer's switches and parameters (a parameter steps by a tenth of its default; the all-overrides switch is under Dev) |
| Perf | `perf.overlay`, `profile.counters`, `profile.capture` | Overlay (F3), FPS, frame time p50 and p99 (ms), slowest frame, the five biggest phases by name (frame pacing left out), and Capture 10 s (`profile.capture {ms: 10000}`, which says how many frames it saw and the p99) |
| Settings | `settings.list {}` | every setting, grouped by prefix into Video, Controls, Co-op, Skulls, Theater, Superbounce, Halo 1, Game modes and Other (a key no group owns), each a sub-page `settings/<group>` that keeps its own keys. A setting that applies after a restart is greyed with "takes effect on restart"; one locked for this run by a flag or env var says what locked it. |
| Dev | `edit.changes` | Shader overrides >, Tag edits >, Features >, Render stats > |
| Dev > Shader overrides | `render.overrides`, `render.reload`, `render.toggle {name: "overrides"}` | the all-overrides switch, Reload, the directory, each override by file name and its state; a rejected one is greyed with its error |
| Dev > Tag edits | `edit.changes`, `edit.revert` | each live tag edit (tag, field, old -> new); select one to revert it, or Revert all (confirm) |
| Dev > Features | `features.list` | each feature, on or off with the reason it is off (read-only). The rows come from the answer's structured `features` array when it has one, else from the `feature <id>: on` and `feature <id>: off (<reason>)` lines of the plan text (`Plan::text`, `crates/hooking/hooks/src/plan.rs`); the plan's `warning:` lines show as greyed rows. |
| Dev > Render stats | `render.stats` | every counter as a read-only row named by its path |

The Player and Time pages are the cheats below; the same `cheat.list`
answer also gives the cheat line in the watermark.

### Cheat line

While the menu is closed, a small line at the bottom left of the game image
lists the cheats that are on and a game speed other than 1x
(`cheats: God mode, Fly, 0.5x`). It shows with the perf overlay off, so a
forgotten god mode is never invisible. `app::Watermark::set_cheats` takes
the list; `debug_menu::sources::active_cheats` makes it from a `cheat.list`
answer (`crates/apps/app/src/watermark.rs`, `shell.rs`).

### Debug views

All switches start off. Each enabled view contributes up to 2,048 lines to a
genuine image, shared across its split-screen views. Range is 1–512 world
units (default 48). Capture measures distance from local listeners, falling
back to player unit positions when available. Without either reference it
takes a bounded sample. The overlay projects those lines with each view's
camera. Hidden edges draw at full strength.

| View | Geometry and source |
|---|---|
| Geometry edges (blue) | Triangle edges from drawn sections of the active structure BSP, including placed geometry instances at their world transforms. |
| Trigger volumes (green) | Oriented boxes from scenario trigger origins, forward/up axes and full extents. |
| Kill / death volumes (red) | The subset of trigger boxes referenced by the scenario's kill-trigger list. |
| Invisible barriers (pink) | BSP collision edges bordering a surface with the invisible flag, including instance collision. |
| Collision geometry (orange) | Edges of the active BSP's collision surfaces and its placed instance collision surfaces. |
| Hitboxes (violet) | Selected object collision permutations transformed by their current node matrices, plus supported live rigid-body shapes. The physics outlines cover boxes, spheres, pills, convex hulls and the active biped capsule. |
| Traces (yellow) | The start and requested endpoint of each completed world collision query, with a three-axis cross at the engine's returned hit. Misses draw a dashed line. |

The host caches static geometry by map and BSP. Each static layer stores up
to 131,072 lines. Object-attached trigger volumes need the live marker's
world transform and are omitted. Hitboxes skip unrecognized physics shapes
and collision nodes without a matching pose.

The trace history holds the newest 512 queries from the owning simulation
executor. Trace colors fade over 1–300 guest ticks (default 30). The traces
row gives a reason if the query entry is unavailable or lockstep, film
recording or film playback is active. Repeated images retain their sampled
lines. See [Drawing world-space lines over the frame](agent-api.md#drawing-world-space-lines-over-the-frame).

## Cheats

The retail game has no usable cheat commands, so these are our own
(`crates/extensions/game/src/cheats.rs`). They are applied after each game tick to the
local players' units, with the helpers our custom skulls use:

| Cheat | What it does |
|---|---|
| `god` (god mode) | sets the body's cannot-take-damage flag; switching off clears it only on the units we set it on |
| `infinite_ammo` | each magazine's reserve to its maximum, an energy weapon's charge to full, both grenade counts topped; reloads still happen |
| `bottomless_clip` | each magazine's loaded rounds to its maximum, so no reloads (plasma heat still builds) |
| `game_speed` | 0.1x, 0.25x, 0.5x, 1x, 1.5x, 2x or 4x game time, set the way the game's own set-speed helper sets it; a new map puts it back to 1 |
| `fly` | flight through the game's own physics (walls still stop you); controls below |
| `noclip` | the same flight through walls; the body takes no damage while it is on |
| `hide_hud` | the HUD hidden through the game's own HUD switch (the flag a script's HUD builtin sets); the game's last value goes back when off |
| `ai_off` | the actors stand still: the AI globals' on byte (`[0x4f55d0]+0`, which `ai_update` checks and the script's `ai` switch writes) held at 0; the game's last value goes back when off |

A cheat switched on at the main menu is kept and takes hold when a
campaign or multiplayer game starts; until then `cheat.list` gives it a
`pending` reason.

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

They act only in a campaign or multiplayer game, not on the main menu,
where the menu says so on each row. When every cheat is refused, the menu
shows "Cheats are off" with the
reason and greys every row. A cheat refused on its own greys only its row,
with its own reason. `cheat.set` answers `UNAVAILABLE` with the reason. If
the session turns into one that refuses a cheat while it is on (someone
else signs in to Live, a co-op session starts), it is switched off and
undone.

`h2 agent cheat.set '{"name":"noclip","value":true}'` does the same as the
menu.

Not built:

- infinite plasma heat: the heat fields are not researched;
- AI ignores the player, active camouflage, one-hit kills, teleport to the
  crosshair, a free camera: each needs more research, or calls into the
  game (ray casts) that these cheats do not make yet;
- kill all enemies: the only delete, `world.object.delete`, is an editor
  edit that frees the object without telling the AI that owns it, so it is
  not offered for an actor;
- low gravity: our Low Gravity skull already owns the gravity constant, and
  two writers would fight over it.

## Adding an item

Rows are not hand-coded. They come from a registry through an agent method,
so an item belongs to one of these:

1. **An entry in an existing registry.** A `hot_render::Hub` toggle or
   parameter (`hub.declare_toggle` / `declare_param`) shows up under View >
   Render passes, a `settings::Def` under Settings (a prefix no group owns
   lands in Other), a cheat under Player or Time, a feature under Dev >
   Features: all with no menu code.
2. **A new row kind in an existing section.** Change that section's rows
   function in `sources/<section>.rs`, using `Row::toggle`, `int`, `float`,
   `choice`, `action`, `open` or `info`, with a `Setter` naming the agent
   method and parameter that changes it. Mark it `.disabled(reason)` when it
   can't change, `.about(text)` for the footer, `.confirm()` when it can't be
   undone, and `.on_ok(fmt)` for the line that says what it did.
3. **A new section.** Add the host methods to the agent
   (`agent.add_method`, on the game's thread), then a `Section { id, title,
   about, calls, rows }` in a file of `sources/`, and list it in
   `sources::all()`. A page reached from another takes an id with a `/` and
   a `Row::open` on its parent. A section for one mode goes in with
   `Shared::add_section` at run time.

Test new rows in the file's own `tests` module over `sources/fake.rs`, a
fake game that answers each method in the format the host returns (read it
from the method's implementation, not its doc).

## How it is wired

- `crates/platform/input`: the DualSense and DualShock 4 touchpad
  (`HostPadState::touchpad`, read in `game_controller/apple.rs`, never mapped
  to the guest), gestures (`touchpad.rs`), and `InputManager::set_gate`. A
  `PadGate` sees each pad between polling and conversion.
- `crates/extensions/debug-menu`: the model (`model.rs`), sections (`sources/`), layout
  (`layout.rs`), the gate (`gate.rs`) and `Shared`, the handle the window,
  the input layer and the agent connection share.
- `crates/extensions/agent/src/menu.rs`: the menu's in-process agent connection, plus
  `settings.list`/`settings.set` and `perf.overlay`.
- `crates/extensions/game/src/debug_menu_glue.rs`: installs those methods. A game
  without `H2_AGENT` still gets a socketless agent for the menu. Its shader
  overrides are not loaded or watched until asked.
- `crates/apps/app/src/menu_overlay.rs`: the drawing (top left of the game image,
  one step bigger than the watermark, a dark plate, the selected row on a
  bar). `keymap.rs` has the backquote key, and `shell.rs` routes keys and
  redraws.
- `crates/apps/cli/src/play_cmd.rs`: makes the menu, sets the gate, hands it to
  the window and the game.
- With the keyboard on (the default), player one is
  `input::backend::fallback::KeyboardFallback`: the first pad merged with the
  keyboard through the guest's pad. The merge carries the pad's touchpad
  across, so the gate sees its click. It used to drop it, so neither the
  click nor the chord opened the menu from player one's DualShock 4 or
  DualSense (fixed 2026-11-19; test
  `a_touchpad_click_on_player_ones_pad_opens_it_through_the_input_manager`).

## When a pad doesn't open it

Hold Back and D-pad down for half a second; that works on any pad. If it
doesn't, `h2 agent hostpads` (`input.host`,
[agent-api.md](agent-api.md#inputhost-the-host-controllers)) shows each
controller three ways. Run it while the buttons are held:

- `devices`: the controller as GameController reads it. Back must show as
  held. For the touchpad, `profile` must be `dualsense` or `dualshock4`;
  `extended` means the framework gave it a plain profile, so there is no
  touchpad to read, and `touchpad.click` should be `true` while the pad is
  held.
- `pads`: what the gate is handed. Player one's buttons and touchpad should
  match the one under `devices`.
- `gate`: `open`, and per pad `touchpad`, `click`, `chord`, and
  `chord_pending` (Start waiting for the click).

`notes` says in words where the touchpad goes missing.

## Not verified

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
- Give weapon, Teleport and Spawn in a campaign and a multiplayer level: that
  the weapon is held, the vehicle lands on the ground in front of the
  player, and the player stands clear of the one they teleported to.
- Quick save and Quick load in a level with something live that a snapshot
  can't carry: the failure names it.
- The cheat line at the bottom left, with the perf overlay off.
