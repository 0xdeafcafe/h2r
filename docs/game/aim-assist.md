# Aim assist strength

Status: built and tested on synthetic tags; not yet seen in game.

The ask: aiming with the iOS on-screen touch controls is much harder than
with a stick, so the game's aim assist should be able to help more. The
**Aim Assist** setting (`controls.aim_assist`, a row on our Video screen,
`H2_AIM_ASSIST`) picks how strong it is:

| Value | Name in the file and variable | What it does |
|---|---|---|
| Auto (default) | `auto` | Strong while the touch controls show (`input::touch::controls_shown()`), else Default |
| Default | `default` (alias `retail`) | the tags' own values |
| Strong | `strong` | angles x1.5, ranges x1.25, friction and adhesion x1.25 |
| Very Strong | `very_strong` (alias `very-strong`) | angles x2, ranges x1.5, friction and adhesion x1.5 |

Code: `boot::aim_assist` (the hook and the memory walk),
`boot::setting_defs::AIM_ASSIST` (the setting). Without a settings store
(headless `h2 run`) the feature is off and everything is retail.

Labels: **engine** means read from the retail XBE's instructions with
`h2 disasm` over the Ghidra function list; **community** means Assembly's
plugin names, cited.

## What Halo 2 reads

Halo 2 has two parts to its aim assist: a per-weapon cone, and global
"sticky crosshair" strengths.

### The weapon (`weap` tag)

| Offset | Field | Unit | What it does |
|---|---|---|---|
| `+0x208` | autoaim angle | radians | Inside this angle of the crosshair a target gets full-strength autoaim: shots bend towards it |
| `+0x20c` | autoaim range | world units | How far autoaim reaches |
| `+0x210` | magnetism angle | radians | Inside this angle the crosshair is slowed and dragged by the target (friction and adhesion below) |
| `+0x214` | magnetism range | world units | How far magnetism reaches |
| `+0x218` | deviation angle | radians | How far a shot may leave the barrel's line; not changed by us |

Engine: `0x001a50a0` builds the aim-assist parameters for a held weapon. It
finds the weapon's definition through the tag entry table (`[0x4e3b44]`,
`+8` of the 16-byte entry), then at `0x001a5137` to `0x001a5181` reads the
four fields in that order, divides both angles by the zoom magnification
and multiplies both ranges by it, and at `0x001a5186` takes the larger of
the deviation angle and the autoaim angle. Weapon tag flags bit 5 (`+0x12c`,
"aim assists only when zoomed") is tested just before. So a wider autoaim
angle also lets a shot deviate further, as in retail when a tag's autoaim
angle is above its deviation angle.

Community: the names and units are Assembly's
`src/Assembly/Plugins/Halo2/weap.xml` ("Weapon Aim Assist" section) at
commit `eecb829971d3b58d7bd8285d9b4dfe86425dda6f`
([XboxChaos/Assembly](https://github.com/XboxChaos/Assembly)). The offsets
in this repo come from the engine reads above; they are pinned in
`tags::object::layouts::WEAP` (`tags` test `layout_pins`).

The melee aim assist (`+0x19c` magnetism angle, `+0x1a0` range and the
lunge throttle after them, per Assembly) is left alone: a longer lunge is a
bigger change to play than easier aiming.

### The globals (`matg` tag)

| Offset | Field | What it does |
|---|---|---|
| `+0xf0` | player-control block (count, address), elements of `0x80` | |
| element `+0` | magnetism friction | How much the crosshair slows over a target |
| element `+4` | magnetism adhesion | How much the crosshair follows a moving target |
| element `+8` | inconsequential target scale | Magnetism on minor targets (Flood infection forms); not changed |

Engine: the player-control update `0x00185be0` keeps `[[0x4e034c]+0xf4]`
(the block's address) and at `0x0018622b` reads element `+0` and `+4`,
each raised to a per-target value and capped at 1.0 (`0x00186252`). The
target search `0x001a5c00` reads `+8` at `0x001a5c88`. Community: names
from Assembly's `Plugins/Halo2/matg.xml` at the same commit ("Player
Control").

## What each level changes

Each value is retail times the factor, capped, and never below retail:

| | Angles (autoaim, magnetism) | Ranges (autoaim, magnetism) | Friction, adhesion |
|---|---|---|---|
| Default | x1 | x1 | x1 |
| Strong | x1.5 | x1.25 | x1.25 |
| Very Strong | x2 | x1.5 | x1.5 |
| Cap | 0.5 rad (about 28.6 degrees) | none | 1.0 (the engine's own cap) |

A value that is already past a cap is kept. A value that is not a finite
positive number (a weapon with no magnetism has 0) is kept. The factors are
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

The weapon and globals tags feed the simulation (where shots go), so the
setting is `Scope::Sim`. In a multiplayer game (game options mode 2,
`[eax]` at activation) the hook uses Default whatever the setting says:
every peer aims with the same values, a touch player is not given an edge
over a controller player, and host and guest never disagree about where a
shot went. Campaign, co-op and anything else follows the setting. Open: a
"touch players get Strong in multiplayer too" option, if wanted, needs the
host to decide and advertise it like the multiplayer skulls
([skulls.md](skulls.md) "In multiplayer").

## Testing

- `cargo test -p boot aim_assist`: the factors and caps
  (`scaling_caps_angles_and_never_weakens`), Default leaving every byte alone,
  Strong and Very Strong over a synthetic tag index with three weapons, a
  biped and a globals tag, no compounding, a reloaded map, odd tables, Auto
  following `input::touch::set_controls_shown`, and multiplayer staying
  retail. The offsets are checked against the `tags` layout
  (`the_offsets_are_the_tag_layouts`).
- `cargo test -p boot setting_defs`: `aim_assist_is_auto_unless_asked`
  (default Auto, the file's names, the variable's names and lock, a press
  stepping Auto, Default, Strong, Very Strong).
- On the retail disc (values printed, not kept):
  `H2_DISC=/path/halo2.iso cargo test -p boot retail_tags_hold_plausible -- --ignored --nocapture`.
- In game: set `H2_AIM_ASSIST=very_strong`, load a campaign level; stderr
  says `aim-assist: Very strong (N weapons, 1 player controls, M fields
  written)`. The crosshair should turn red further from an enemy and drag
  harder over one. Back to `default`, the next load writes retail back.
