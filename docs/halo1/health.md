# Health: Halo 1's model on Halo 2's engine

Status: **Built.** The hooks and the switch are confirmed in a game run by
the owner. The health meter is part of Halo 2's HUD (below, "The health
meter in the HUD"); its placement is not confirmed in a game run. The
game start picks the model from the loaded map's name
(`health::model_for_map`): converted Halo 1 multiplayer maps (`h1_<map>`)
in the Halo 1 sandbox and Halo 1's campaign levels get Halo 1's; the
debug override `H2_HEALTH_MODEL` beats it ([multiplayer.md](multiplayer.md),
"Halo 1 sandbox"). The converted player shows Halo 1's own HUD by default
([hud.md](hud.md)); the HUD with the health strip below is the Halo 2
alternative the player's HUD choice selects.

Halo 1 has a shield that recharges over a health bar that doesn't, and
health packs that refill it. Halo 2 has a shield over hidden body health
that regenerates. For Halo 1 content we keep Halo 1's model, the one
agreed exception to "Halo 2's engine wins" ([campaign.md](campaign.md)
decision 2). The engine still uses Halo 2's damage, shield and physics code.

Labels: **confirmed** means read from the retail XBE's instructions with
`h2 disasm` and asserted by `crates/apps/boot/tests/health_xbe.rs` (ignored;
needs `H2_XBE`); **likely** means strong evidence but not every path read;
**community** means public knowledge.

## What the game does

### Body regeneration (confirmed)

| Address | What |
|---|---|
| `0x0bc470` | `object_update`: for each object, `0xd5de0`, then its children |
| `0x0d5de0` | the object's damage update (`OBJECT_DAMAGE_UPDATE`): stdcall `(object datum)`, `ret 4`, AL whether anything changed. Its only direct caller is `0xbc527`. Once per object per tick |
| `0x0d5b60` | the object's damage info: definition `+0x38` (its `hlmt`) → the `hlmt`'s damage info block (`+0x60` count, `+0x64` pointer), element 0 |
| `0x0d63de`..`0x0d64b5` | **the regeneration.** Target = damage info `+0x38` (likely the body "recharge fraction"), rate = damage info `+0xd4` (likely the runtime body recharge velocity); for an AI unit both come from its character's vitality block instead (`0x1e4a10`, `+0x14` and `+0x68`). When target > health and rate > 0: if the body stun timer (`+0x106`, `i16`) is 0, `health += rate * dt` (`[0x510c54]+4`), times a team scale for units (`0x1e9720`), clamped to the target; "body recharging" (`+0x10b` bit 0) set, cleared at the target. Else the timer counts down |
| stores to `+0xec` in `0xd5de0` | only `0xd647b` (the add) and `0xd6485` (the clamp): the update never raises health any other way |

Other writers of `+0xec` (all outside the update): `0xd5c90` (reset
vitality from the definition), `0xd6790` (body restore, below), `0xd8056`,
`0xda51d`, `0xdc168` (kills, set to 0), `0xd8cb0` (body damage),
`0xdb71f` (a floor, `0xeaaad`), and HaloScript and checkpoint code.

### Health packs (confirmed)

Halo 2 already has them; it just never ships one.

| Address | What |
|---|---|
| `0x151ea0` | powerup pickup: stdcall `(player datum, equipment object datum)`, `ret 8`. Reads the equipment's `eqip` powerup type (`+0x12c`, `i16`) and powerup time (`+0x130`, `f32` seconds; times ticks per second, **zero or less and nothing happens**). Type 2 overshield (`0xd6af0`), 3 active camo, **5 health**, 1 and others timed (`0x152000`) |
| `0x151f6a` | type 5: `0xd6790` on the player's unit; if it says yes, `0x14ec00` (likely the pickup flash), then the equipment is deleted (`0xb8540`) |
| `0x0d6790` | body restore (`BODY_RESTORE`): EAX the object datum, AL 1 when done. Refuses (AL 0) when the body is depleted (`+0x10a` bit 2) or already at `[0x45dbc0]` (1.0); else health = 1.0. Its only caller is `0x151f73` |
| `0x15102f`, `0x14fbc5` | the pickup's callers: touching an equipment object whose type is not 0 and not 6 (grenades), and the pickup action |

So a full-health player walks over a pack and leaves it; a hurt one is
healed to full and the pack goes, as in Halo 1. The type names follow the
`eqip` powerup enum (none, double speed, overshield, active camo,
full-spectrum vision, health, grenade); only 2, 3, 5 and 6 were read in
code.

### HUD (confirmed)

| Address | What |
|---|---|
| `0x13992a` | per-user HUD state (`0x270` bytes, kept at `0x4e69d0 + user * 0x270`): unit body `+0xec` to `+0x28`, shield `+0xf0` to `+0x2c`, damage flags to `+0x1e`, grenades, weapons |
| `0x13ad48` | a HUD widget's four inputs (one byte each) to floats: **input 16 is the shield (`+0x2c`), input 17 the body (`+0x28`)**; 0 zero, 1 one, 3 `0x1392a9`, 18, 19 others |
| `0x13a720` | the shield's state flags for the HUD (low, empty, ...) |

So Halo 2's HUD can already show the body: a widget on input 17. The
shield meter does not use input 16: it is the widget with special type 5,
whose draw reads the shield itself (below).

### HUD draw and its gates (confirmed)

Asserted by `health_xbe.rs` `every_hud_widget_is_drawn_behind_the_same_gates`.

| Address | What |
|---|---|
| `0x13954b` | the HUD render: calls HUD_SHOWN (`0x13939b`) first and returns at `0x1396ba` when it says no; then fills the HUD state (`0x13992a`) and draws each `nhdt` of the HUD (`0x1394b9`, from `0x1395cd` and `0x1395f5`) |
| `0x13939b` | HUD_SHOWN: a per-user state (`0x155760`: 2 hides, 3 asks `0x155d60`), the HUD-enabled byte `[0x510c4c]+0x1d2`, and in a campaign the Blind skull's byte (`0x4f55e2`). The multiplayer Blind hook (`skulls_mp`) answers 0 here |
| `0x1394b9` | draws the unit's and the weapons' `nhdt`s, each through `0x13a532` |
| `0x13a532` | one `nhdt`: bitmap widgets (`+0x08` count, `+0x0c` address, `0x64` each): the widget's state gate `0x13ac87` on widget `+0x08`, its inputs `0x13ad48` from widget `+0x04`, the draw `0x22aff1`; text widgets (`+0x10`, `0x54`) and effects (`+0x20`, `0x50`) the same way |
| `0x22aff1` | a bitmap widget's draw: copies its four input floats to the HUD shader inputs (`0x4e6980`, `hud_input1`..`4`), places it by screen mode (fullscreen, half, quarter: offsets `+0x34`, `+0x38`, `+0x3c`), then switches on the special type `+0x60`. Type 5, the shield meter, reads the HUD state's shield (`+0x2c`) and draws up to five layers (overshield) with its own colours |
| unit `+0x1a0` | the unit's HUD interfaces: a block of 8-byte `nhdt` references (D: `masterchief` and `masterchief_mp` name `ui\hud\masterchief` twice) |

D (read from `shared.map`'s `ui\hud\masterchief`, not kept): 8 bitmap
widgets; the shield meter is widget 3 (special type 5, inputs none,
anchor 2, offset (-1, -98) fullscreen and (-1, -76) split, bitmap
`ui\hud\bitmaps\new_hud\meters\shield_meter`, shader
`ui\hud\shaders\shield_test`, whose fill functions map `hud_input1`/`2`
to 0.2156..0.992, the span of the arc's gradient). Widget 4,
`shield_mask`, flashes on input 22 (shield low). The motion tracker sits
at the same anchor, (-1, -84) fullscreen and (-1, -63) split.

How a widget lands on screen (E, `0x22ae21` and `0x22aff1`): the
sequence byte for the screen mode (`+0x30`, `+0x31`, `+0x32`) picks the
bitmap's sequence, whose first image (or the second, `0x22acb4`) is
drawn whole at its own size in HUD pixels; a bitmap with no sequences
draws image 0. The registration point (`+0x40`, two floats per screen
mode, a fraction of the width and height; 0 for the shield meter and the
motion tracker) lands on the anchor point plus the offset. Flags `+0x1e`
bits 0 and 1 mirror the texture only. At 640 x 480 fullscreen the bottom
left anchor is (48, 444), 7.5% in: the motion tracker's 83 x 83 rim is
at x 47..129, y 360..442, as a frame of a10 shows.

D (shared.map's bitmaps, read, not kept): `shield_meter` is 128 x 32
fullscreen (sequence 0) and 64 x 32 split (sequence 2); what shows of it
is columns 0..82 and rows 0..24, a 10-row band across the top with the
shield emblem at its left, then two wings either side of the tracker.
`disintegrator_meter`, sequence 0, is a 92 x 8 bar, opaque but for a
chamfered bottom-left corner, its blue rising from 0 at column 13 to 1 at
the right edge. Halo 2 draws it as the energy sword's battery meter
(`ui\hud\plasma_sword` widget 3, input 56, shader
`ui\hud\shaders\ammo_meter`).

### Damage bleeds through (confirmed)

`0xd7b80` applies a damage instance: the shield step `0xd9110`, then the
body step `0xd8cb0`, sharing one damage record. The shield step takes the
raw damage away from what is left (`+0x1c`), then, if the hit took the
shield below zero, converts the overflow back through the shield's
material scale and adds it to what is left (`0xd92bd`..`0xd92f7`); the
body step reads `+0x1c` (`0xd9020`). So the part of a hit the shield
could not hold goes to the body **in the same hit**, and once the shield
is down every hit goes to the body. Halo 1 (community) behaves the same
way, so nothing is changed. Differences noted, not changed: Halo 2 takes
no body damage at all while an overshield is charging (`+0x10a` bit 4;
`0xd918f`), and a damage flag in the damage data (`+0x68`, likely "ignores
shields") skips the subtraction so the body takes the full hit.

## What we do

The `h1-health` feature (`crates/extensions/game/src/health.rs`), on the feature
plan:

- **No regeneration.** `before` on `OBJECT_DAMAGE_UPDATE` notes a player
  biped's health and its recharging bit (a biped whose unit `+0x13c`
  player datum is set; AI and vehicles are left alone); `after` puts both
  back if the update raised the health. Only the Halo 1 model does this;
  under Halo 2's the hooks note nothing (one atomic load per call).
  Damage in the same update (a fall) still lands. Health from anywhere
  else (packs, scripts, checkpoints, respawns) is untouched, because it is
  not written by this function.
  The update is a Rust body, and the
  installer guards it so these hooks still run around it
  ([object-update.md](../engine/object-update.md), "Slice 8 body").
- **Health packs**: no hook needed. An observer on `BODY_RESTORE` logs
  each use.
- **HUD**: the converted player's HUD draws the meter (below, "The
  health meter in the HUD"). Nothing on the host draws one.
- **Damage. Built.** Converted weapons and the cyborg carry Halo 1's
  damage and vitality data. The cyborg's collision materials select
  the body or head section, and its armour and shields use each
  damage effect's source multipliers through Halo 2's damage table.
  [Blood Gulch hit registration](objects-build-weapons.md#blood-gulch-hit-registration-built)

### The health meter in the HUD

Built by the converter (`h1objects::health_hud`),
so it is data the game draws, not a host overlay:

- The converted Halo 1 player biped (`is_player_biped`: the Halo 1 biped
  named like `cyborg`, which both the multiplayer Halo 1 sandbox and the
  campaign make the player) gets its own HUD interfaces (`unit +0x1a0`),
  each naming a copy of the template's `nhdt` called
  `<name>_h1_health` (`ui\hud\masterchief_h1_health`).
- That `nhdt` has one more bitmap widget, after the others: made from
  the shield meter's widget (same state flags, anchor and flags) with
  input 0 set to **17** (the body), special type 0 (so the shield code of
  `0x22aff1` does not run), no effects, registered at its top left, and
  drawing Halo 2's own thin meter strip: the energy sword's battery meter,
  bitmap `ui\hud\bitmaps\new_hud\meters\disintegrator_meter` sequence 0
  with shader `ui\hud\shaders\ammo_meter` (template `widget_legacy_meter`:
  fills the bitmap where its blue is below `hud_input1`, so from the left
  as the body goes 0..1; fixed colours, the ammo counters' blue, elite
  colours with `hud_elite`). The pairing is retail's, unchanged.
- It sits on the shield meter's left edge with its bottom `GAP` = 2 HUD
  pixels above the shield meter's top, in every screen mode: offsets
  (-1, -108) fullscreen and (-1, -86) split. At 640 x 480 fullscreen that
  is x 47..138, y 336..343 for the strip and x 47..129, y 346..370 for
  what shows of the shield meter (its whole bitmap is x 47..174,
  y 346..377): no overlap, rows 344 and 345 between them. It covers
  nothing of the shield meter, its emblem or the tracker; it is 9 pixels
  longer than the shield band, to the right.

**Why it hides with the shield.** It is a widget of the same `nhdt` as
the shield meter, so it is drawn by the same `0x13a532` pass, only after
HUD_SHOWN said yes (Blind, the HUD-enabled byte, the per-user state), and
it has the shield meter's state flags, so the per-widget gate `0x13ac87`
answers the same for both. Whatever hides the shield meter (menus, pause,
cinematics, Blind, death) hides it; it scales and moves with split
screen and the safe area as the shield meter does. No hook is involved.

**Where it exists.** Halo 1's model runs on converted Halo 1 maps in the
Halo 1 sandbox and on Halo 1's campaign levels (`model_for_map`), and
those are exactly the maps whose player is the converted cyborg. The
Halo 2 sandbox keeps Halo 2's player and HUD (and Halo 2's model). A
retail map under the debug override `H2_HEALTH_MODEL=halo1` has no meter.

If the base map has no `disintegrator_meter` bitmap or `ammo_meter`
shader, or the template HUD no shield
meter, the conversion goes on without the meter and the biped's report
says `HUD: no health meter: ...`.

### The switch

At every game start the feature sets the model from the loaded map's name
(`model_for_map`); `set_mode` stays until set again. It is read on the
next object update. `mode()` is the model in force.

### Debug switch: `H2_HEALTH_MODEL=halo1`

**A debug switch, not a setting.** It forces a model on every map,
whatever the map says (`halo2` forces retail's). The boot says
`health: DEBUG OVERRIDE ...` when it is set. Use it to try the model on a
retail level (which has no health meter: its HUD is retail's).

### What it logs

- `health: hooks installed via feature plan at 0xd5de0 (regeneration), 0xd6790 (health pack log); model now Halo 2`
- each game start: `health: game start: Halo 1 model (debug override H2_HEALTH_MODEL): no body regeneration; the health meter is in a converted Halo 1 map's HUD`
- the first held regeneration of a game: `health: Halo 1 model: held a player's body at 0.412 (Halo 2 would have regenerated it to 0.430); said once per game`
- each health pack: `health: health pack: object 0x..., a player's body at 0.300, restored to full (Halo 1 model)`
- `H2_FEATURES` with `h1-health` off leaves Halo 2's model everywhere.

## What the converter must write

For a Halo 1 health pack (Halo 1 `eqip` with a health powerup):

- `eqip` `+0x12c` powerup type = **5** (health), `i16`;
- `eqip` `+0x130` powerup time > **0** (any, e.g. 1.0 s: the pickup does
  nothing at zero; the time is not otherwise used for health);
- place it as equipment (scenario equipment or netgame equipment) like
  any other.

For the HUD: built (above, "The health meter in the HUD").

Optional, belt and braces: a converted Halo 1 biped's damage info body
recharge fraction (`hlmt` damage info `+0x38`) of 0 stops regeneration in
data alone. Not enough by itself: with the Halo 2 assets setting the
player is Halo 2's biped.

## Tests

- `cargo test -p boot --lib health`: the model switch and override,
  the hold rule, which objects are sampled, write-back, the log lines,
  and the hooks on a feature plan around stand-ins for the damage update
  and body restore on the runtime fixture module (regeneration held for a
  player only under Halo 1, damage kept, a restore kept, the override).
- `cargo test -p h1objects --lib health_hud` (5, invented bytes and
  `ui\hud\masterchief`'s offsets): the shield meter is the special-type-5
  widget; the health widget reads input 17 (not 16), has no special type
  or effects, sequence 0, top-left registration, and keeps every byte the
  state gate `0x13ac87` reads (`+0x08`..`+0x1b`), the anchor and flags;
  in each screen mode its rectangle overlaps neither the shield meter's
  nor the motion tracker's, starts at the shield's left edge and ends 2
  pixels above it, and at 640 x 480 the three are where the a10 frame
  puts them; the widget block gains exactly it, referencing the strip
  bitmap and shader; only the cyborg is the player biped.
- `H2_XBE=disc/default.xbe cargo test -p boot --test health_xbe -- --ignored`
  (6): every address above, and `every_hud_widget_is_drawn_behind_the_same_gates`
  (HUD_SHOWN before every `nhdt` draw, Blind's byte, one gate, inputs and
  draw per widget, the special type).
- `H1_ISO=... H2_DISC=... cargo test -p h1objects --test it sandbox_disc:: -- --ignored`:
  Blood Gulch in the Halo 1 sandbox: every HUD interface of the player
  names a `*_h1_health` `nhdt` with exactly one body widget, the shield
  meter's gate bytes, the strip bitmap and shader; the strip's image is
  92 x 8 and the shield meter's 128 x 32 in the built map, and placed by
  their offsets they do not overlap, 2 pixels apart, in every screen mode.

## Only a game run can show

- That the hold is invisible: no flicker of the HUD or of the "recharging"
  sound/effect while health is held.
- The HUD meter's look against Halo 2's shield meter (the strip's
  colours, its 9 extra pixels to the right), in fullscreen and split
  screen.
- A health pack in a real level (none exist in retail maps; needs a
  converted map, or Forge placing an `eqip` of type 5).

Steps for the user:

1. Play a converted Halo 1 map: a Halo 1
   campaign level, or a Halo 1 multiplayer map in the Halo 1 sandbox.
2. Stderr shows `health: game start: Halo 1 model ...`.
3. Bottom left, a thin straight strip 2 pixels above the shield bar,
   full, the shield bar and its emblem whole beneath it.
4. Take damage until the shield breaks and the strip shortens from the
   right, then hide. The shield recharges; the strip (and the body) must
   **not** come back. Stderr says `held a player's body at ...` once.
5. The strip hides with the rest of the HUD: pause menu, cinematics,
   Blind, death (and comes back full at respawn).
6. A retail map, or a Halo 1 map in the Halo 2 sandbox, has no strip and
   regenerates as retail.
7. If the strip sits wrong, change `health_hud::GAP` or `offsets` and bump
   the converter version.

## Open questions

- Whether the health strip should turn red when low, as Halo 1's did: the
  meter shader's colours are fixed; a low colour needs a shader whose
  colour follows an input.
- Whether Halo 1's AI bodies regenerate in Halo 2 when converted (their
  character vitality block decides; the hold is for players only).
- Network: the hold runs where the simulation runs (host); a client
  predicts with retail's rule and is corrected by the host. Not checked.
- Damage info `+0x38`/`+0xd4` names are likely, from their use only.
