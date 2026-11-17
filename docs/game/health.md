# Health: Halo 1's model on Halo 2's engine

Status: the hooks, the window meter and the switch are built and confirmed in
the game by the owner (2026-11-17). Since 2026-11-17 the game start picks
the model from the loaded map's name (`health::model_for_map`): converted
Halo 1 multiplayer maps (`h1_<map>`) in the Halo 1 sandbox and Halo 1's
campaign levels get Halo 1's; the debug override `H2_HEALTH_MODEL` still
beats it ([h1-multiplayer.md](h1-multiplayer.md), "Halo 1 sandbox").
Started 2026-11-16.

Halo 1 has a shield that recharges over a health bar that doesn't, and
health packs that refill it. Halo 2 has a shield over hidden body health
that regenerates. For Halo 1 content we keep Halo 1's model, the one
agreed exception to "Halo 2's engine wins" ([campaigns.md](campaigns.md)
decision 2). Everything else (damage, shields, physics) stays Halo 2's.

Labels: **confirmed** means read from the retail XBE's instructions with
`h2 disasm` and asserted by `crates/boot/tests/health_xbe.rs` (ignored;
needs `H2_XBE`); **likely** means strong evidence but not every path read;
**community** means public knowledge. Nothing here has been seen running.

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
shield meter is (likely) a `nhdt` widget on input 16; which widget in
`masterchief`'s `nhdt` was not read (this repo has no `nhdt` layout yet).

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

The `h1-health` feature (`crates/boot/src/health.rs`), on the feature
plan:

- **No regeneration.** `before` on `OBJECT_DAMAGE_UPDATE` notes a player
  biped's health and its recharging bit (a biped whose unit `+0x13c`
  player datum is set; AI and vehicles are left alone); `after` puts both
  back if the update raised the health. Only the Halo 1 model does this;
  under Halo 2's the hooks note nothing (one atomic load per call).
  Damage in the same update (a fall) still lands. Health from anywhere
  else (packs, scripts, checkpoints, respawns) is untouched, because it is
  not written by this function.
- **Health packs**: no hook needed. An observer on `BODY_RESTORE` logs
  each use.
- **HUD**: a converted Halo 1 map's HUD draws the meter itself (below).
  When the map's HUD has none, the window draws a Halo 1 style meter over
  the game image: eight segments, blue, red at a quarter or less
  (`crates/app/src/health_meter.rs`), right edge and top at 95.5% and
  11.5% of the image by default, under Halo 2's shield meter in the top
  right. `H2_HEALTH_METER_AT=right,top` (fractions) moves it without a
  rebuild. It shows local user 0's health (split screen: user 0 only).
- **Damage**: unchanged.

### The switch (for the campaigns and h1-mp lanes)

`boot::health::set_mode` has no caller outside this module and its tests
(checked in the tree, 2026-11-17). The converter and the map load have to
call it; until they do the model is Halo 2's on every map.

```rust
use boot::health::{self, HealthModel};
// At every game start, from the map's identity:
health::set_mode(if is_halo1_map { HealthModel::Halo1 } else { HealthModel::Halo2 });
// When the converted map's HUD has the body widget (so the window's meter stays off):
health::set_hud_has_meter(true);
```

`set_mode` stays until set again, so call it for every map, not only
Halo 1's. It is read on the next object update; calling it from a
`GAME_ACTIVATE` hook or before `OPTIONS_QUEUE` both work. `mode()` is the
model in force.

### Debug switch: `H2_HEALTH_MODEL=halo1`

**A debug switch, not a setting.** It forces a model on every map,
whatever the map says (`halo2` forces retail's). The boot says
`health: DEBUG OVERRIDE ...` when it is set. Use it to try the model and
the window's meter on a retail level.

### What it logs

- `health: hooks installed via feature plan at 0xd5de0 (regeneration), 0xd6790 (health pack log); model now Halo 2`
- each game start: `health: game start: Halo 1 model (debug override H2_HEALTH_MODEL): no body regeneration; meter drawn by the window`
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

For the HUD, in the converted biped's `nhdt`: a copy of the shield meter's
widget, moved under it, with its input set to **17** (unit body) where the
shield's has 16; then call `set_hud_has_meter(true)`. Open: the `nhdt`
layout (where the widget block and its input bytes are) is not in this
repo yet.

Optional, belt and braces: a converted Halo 1 biped's damage info body
recharge fraction (`hlmt` damage info `+0x38`) of 0 stops regeneration in
data alone. Not enough by itself: with the Halo 2 assets setting the
player is Halo 2's biped.

## Tests

- `cargo test -p boot --lib health` (8): the model switch and override,
  the hold rule, which objects are sampled, write-back, the meter value,
  the log lines, and the hooks on a feature plan around stand-ins for the
  damage update and body restore on the runtime fixture module
  (regeneration held for a player only under Halo 1, damage kept, a
  restore kept, the override).
- `cargo test -p app --lib health_meter` (4): segments, colours,
  placement, the handle.
- `H2_XBE=disc/default.xbe cargo test -p boot --test health_xbe -- --ignored`
  (5): every address above.

## Only a game run can show

- That the hold is invisible: no flicker of the HUD or of the "recharging"
  sound/effect while health is held.
- The window meter's look and place against Halo 2's shield meter.
- A health pack in a real level (none exist in retail maps; needs a
  converted map, or Forge placing an `eqip` of type 5).

Steps for the user:

1. `H2_HEALTH_MODEL=halo1 h2 play --level 01b_spacestation` (any campaign
   level; or start one from the menu).
2. Stderr shows `health: DEBUG OVERRIDE`, then at the level
   `health: game start: Halo 1 model ...`.
3. Top right, under the shield meter: an eight-segment blue meter, full.
4. Take damage until the shield breaks and the meter drops, then hide.
   The shield recharges; the meter (and the body) must **not** come back.
   Stderr says `held a player's body at ...` once.
5. Low health turns the lit segments red. Dying hides the meter until
   respawn (full again).
6. Without the variable the meter never shows and the body regenerates as
   retail.
7. If the meter sits wrong: `H2_HEALTH_METER_AT=0.95,0.13` and so on.

## Open questions

- The `nhdt` layout, and which widget is the shield meter, for the
  converter's health widget.
- Whether Halo 1's AI bodies regenerate in Halo 2 when converted (their
  character vitality block decides; the hold is for players only).
- Network: the hold runs where the simulation runs (host); a client
  predicts with retail's rule and is corrected by the host. Not checked.
- Damage info `+0x38`/`+0xd4` names are likely, from their use only.
