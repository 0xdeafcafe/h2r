# Halo 1's HUD on converted Halo 1 maps

Status (2026-11-18): the unit HUD is built and is the converted player's
default; the switch to Halo 2's HUD is built in the boot but the
converter does not yet record the pairs, so Halo 1's HUD always shows.
Weapon HUDs, grenades and the HUD globals are not converted yet. Nothing
here has been seen running. Labels as in [health.md](health.md): **E**
engine code (`h2 disasm`), **V** checked on the owner's discs, **D** a
choice made here.

## What converts

`h1objects::h1hud` turns Halo 1's unit HUD (`unhi`) into a Halo 2 `nhdt`
written from scratch: the root, every widget and every byte of each is
set there; no retail `nhdt` is copied. The converted player biped (the
cyborg, `health_hud::is_player_biped`) names it in every HUD interface
(`unit +0x1a0`), as `<unhi name>_h1_hud`.

| Halo 1 element (`unhi`) | Widget |
|---|---|
| shield panel background `+0x8c` | static, Halo 1's anchor |
| health panel background `+0x17c` | static (null on the cyborg: skipped) |
| shield panel meter `+0xf4` | meter on input 16 (shield) |
| health panel meter `+0x1e4` | meter on input 17 (body) |
| motion sensor background `+0x26c` | static, bottom left |
| motion sensor foreground `+0x2d4` | static, bottom left |

Halo 1 element layout (V: the cyborg's, every element's bitmap reference
resolving at `+0x24`): offset (two `i16`) `+0`, scale (two `f32`) `+4`,
bitmap `+0x24`, colour `+0x34`; a static element's sequence `+0x54`, a
meter's `+0x46`. `unhi +0` is the anchor (`u16`, 1 on the cyborg, whose
shield and health are top right: D, Halo 1's enumeration top left, top
right, bottom left, bottom right, centre). The motion sensor has no anchor
field and is drawn bottom left (D).

Why it draws as Halo 1's:

- **Bitmaps are Halo 1's**, converted unchanged (`h1bitmaps`). Halo 1's
  HUD bitmaps are sprite sheets; Halo 2's widget draws a sequence's sprite
  when it has one (E `0x22ae94`..`0x22aeeb`: sprite count `+0x34` above
  zero, then the sprite's image and its bounds `+8..+0x18`).
- **Meters** draw through `ui\hud\shaders\ammo_meter` (template
  `widget_legacy_meter`: fills where the texture's blue is below
  `hud_input1`). Halo 1's meter sprites are A8Y8, a luminance ramp under
  an alpha mask (V: `hud_unit_meters`), so the blue sampled is Halo 1's
  ramp and the fill order is Halo 1's.
- **Inputs**: 16 shield, 17 body (`0x13ad48`). No special type (5 is
  Halo 2's own shield meter code, `0x22aff1`).
- **Placement**: Halo 2's anchors (E `0x1396c7`, the safe area's corner):
  0 top right, 1 top left, 2 bottom left, 3 bottom right, 4 centre. The
  widget's offset is Halo 1's; its registration is the sprite's corner on
  the anchor's side (D). Halo 1's own sprite registration points are not
  used: they do not read as fractions of the sprite (V: -3.9 to 1.0 on the
  cyborg's elements), and Halo 1's offsets on a top-right anchor are
  positive as well as negative, so how Halo 1 places from them is not
  established. **The placement is the least proven part.**
- **Hiding**: every widget has the state flags of Halo 2's shield meter
  (`h1hud::STATE`), so the per-widget gate `0x13ac87` answers as for it,
  and the `nhdt` is drawn behind HUD_SHOWN (`0x13939b`) like any other:
  pause menu, cinematics, Blind and death hide it ([health.md](health.md),
  "Why it hides with the shield").

## The switch: one map, both HUDs

Decision: a runtime swap, no map variants. Evidence: the HUD reads its
`nhdt` datum from the owner's tag every frame, a weapon's from `weap
+0x2b4` (E `0x0013b071`: the tag table, the weapon's definition, the datum
of the reference at `+0x2b0`), a unit's from its HUD interfaces. Both are
loaded tag data, so rewriting the datum before the game starts changes what
is drawn.

`boot::hud` (feature `h1-hud`):

- `set_mode(HudMode::Halo1 | HudMode::Halo2)`, `mode()`: the pre-game
  option calls `set_mode`; Halo 1 is the default. It stays until set
  again.
- Before `GAME_ACTIVATE`, `apply` points every recorded owner's HUD
  references (a `bipd`'s HUD interfaces, a `weap`'s `+0x2b0`) at the HUD
  the mode asks for, both ways; a target that is not a loaded `nhdt`, or
  an owner that is not a loaded `bipd`/`weap`, is left alone.
- The pairs come from the package manifest, `[[variant]] hud = [{ owner,
  halo1, halo2 }]` (`content::HudSwap`), read for Halo 1 sandbox variants,
  and for campaign levels from `<data>/campaigns/<campaign>/<level>.hud.toml`
  (`content::LevelHud`), which `h2 h1 campaign` writes beside each level.

The converter records each pair (`h1objects::convert::Tree::hud_swaps`:
the owner, its Halo 1 HUD, its Halo 2 HUD; a tree tag or a base-map tag).
For converted multiplayer maps `h1convert` keeps every pair's Halo 2 HUD
through the prune (nothing else references it), remaps the three datums
through the from-scratch rebuild by group and name (`h1convert::fresh`,
`Fresh::hud`) and writes them to the variant. For campaign levels
`h1campaign` keeps them through its scratch build or prune the same way
(`h1campaign::remap_hud`) and the CLI writes the level's `.hud.toml`.

## Not converted yet

- **Weapon HUDs** (`wphi`): crosshairs, ammo and battery counters, zoom
  and reticles. Seen on the disc: a weapon's `wphi +0` names a parent
  `wphi` (assault rifle → `ui\hud\master rounds` → `ui\hud\master`)
  whose elements draw too; bitmaps `hud_ammo_meters`, `hud_reticles`,
  `hud_weapon_backgrounds`, `hud_reticle_warnings`, scope masks. Halo 2's
  crosshair is a bitmap widget with anchor 4 and registration 0.5, its
  ammo meter a legacy meter on input 54 (V: `ui\hud\magnum`). The weapons
  lane rebuilds `weap`; the HUD reference is `weap +0x2b0`.
- **Grenades** (`grhi`), **HUD globals** (`hudg`), **numbers** (`hud#`):
  Halo 2 draws counts as text widgets in its HUD font (`ui\hud\masterchief`
  `frag_count`), not from a numbers bitmap.
- **Motion sensor blips**: Halo 2 draws them by its own code; whether they
  follow the converted background's place is for a game run to show.
- Halo 1's element colours and scale are not carried: Halo 2's HUD
  shaders have their own colours.

## Tests

- `cargo test -p h1objects --lib h1hud`: anchors, registration, the
  widget bytes (inputs, shader, state, no special type or effects,
  sequences, offsets, registration), the `nhdt` holding only its widgets.
- `cargo test -p boot --lib hud`: the swap both ways on bipeds and
  weapons, other HUDs left alone, refused targets, the mode.
- `H1_ISO=... cargo test -p h1objects --test hud_disc -- --ignored`: every
  `unhi` on every Halo 1 map reads (root at least `0x56c`), each element a
  sprite or image of a `bitm`.
- `H1_ISO=... H2_DISC=... cargo test -p h1objects --test sandbox_disc -- --ignored`:
  Blood Gulch's player HUD in the built map: every interface names the
  `_h1_hud` `nhdt`, each widget a Halo 1 bitmap through the right shader,
  shield and health meters present.

## What a game run should show

On a converted Halo 1 map (converter version 50 or later) in the Halo 1
sandbox, or a Halo 1 campaign level: Halo 1's shield and health bars top
right, Halo 1's motion sensor frame bottom left, Halo 2's weapon HUD and
grenades as before. The shield bar drains from the right as the shield
falls, the health bar likewise and stays down (Halo 1's health model). The
whole HUD hides in the pause menu, under Blind and in cinematics. Bars
that sit wrong or off screen are expected until the placement (D above)
is confirmed: `h1hud::registration` and the offsets are the place to
change.
