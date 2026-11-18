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

| Halo 1 element (`unhi`), in draw order | Widget |
|---|---|
| HUD background `+0x24` | static, Halo 1's anchor (skipped when null) |
| shield panel meter `+0xf4` | meter on input 16 (shield) |
| shield panel background `+0x8c` | static, over the meter |
| health panel meter `+0x1e4` | meter on input 17 (body) |
| health panel background `+0x17c` | static (null on the cyborg: skipped) |
| motion sensor background `+0x26c` | static, bottom left |
| motion sensor foreground `+0x2d4` | static, bottom left |

Halo 1 element layout (V: every element's bitmap reference resolving at
`+0x24`; E: the element draw `0x000e1830` reads the bitmap's datum at
`+0x30`, the sequence at `+0x54`, the colour at `+0x34`, the scale at
`+4`): offset (two `i16`) `+0`, scale (two `f32`) `+4`, bitmap `+0x24`,
colour `+0x34`; a static element's sequence `+0x54`, a meter's `+0x46`.
`unhi +0` is the anchor (`u16`; the enumeration under Placement).

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
- **Placement** is Halo 1's, read from Halo 1's XBE (`disc/halo1.iso`,
  `default.xbe`, with `h2 disasm`; E below means that XBE here):
  - the unit HUD draw calls the element draw `0x000e1830` for each element
    in this order: the HUD background `+0x24` (`0x000dcf8f`), the shield
    meter through the meter draw `0x000e20a0` (`0x000dd2ea`), the shield
    panel `+0x8c` over it (`0x000dd32f`), the health meter (`0x000dd52e`),
    the health panel `+0x17c` (`0x000dd556`), the motion sensor's
    background `+0x26c` and foreground `+0x2d4` (`0x000dd618`,
    `0x000dd63b`). The widgets keep that order. (`0x000db3c0`..`0x000db5c1`,
    with the same element offsets, is the grenade HUD's draw, `grhi`.)
  - the element's sprite: its sequence (`+0x54`), the sequence's first
    sprite and its bounds (`0x000e18a0`..`0x000e18c5`);
  - the anchor is the `unhi`'s `+0` word; for the motion sensor a stand-in
    whose anchor is 2, bottom left (`0x000dd5ab`);
  - the offset is measured inward from the anchor's corner, x negated on a
    right anchor and y on a bottom one (`0x000e07a0`), and scaled by the
    element's scale (`0x000e15f4`);
  - the sprite is laid out from that point away from the corner
    (`0x000e0330`): Halo 1 never reads the sprite's registration point.

  So a widget takes Halo 2's same corner (E `0x1396c7` in Halo 2: 0 top
  right, 1 top left, 2 bottom left, 3 bottom right, 4 centre), Halo 1's
  offset turned to screen directions (`h1hud::to_halo2_offset`), and is
  registered at the sprite's corner on the anchor's side
  (`h1hud::registration`).
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

`boot::hud` is the switch (the player's choice, `halo1.hud` on the Halo 1
options screens: `set_choice`, `style_for_map`, `style`; Halo 1's by
default, on Halo 1 maps only). `boot::hud_swap` (feature `h1-hud`) draws
it:

- Before `GAME_ACTIVATE`, `apply` points every recorded owner's HUD
  references (a `bipd`'s HUD interfaces, a `weap`'s `+0x2b0`) at the HUD
  of `hud::style_for_map(map, hud::halo1_chosen())`, both ways; a target that is not a loaded `nhdt`, or
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

## Weapon HUDs

`h1hud::weapon_entries` reads a weapon's `wphi` and its children (the
child first, as Halo 1 draws it: E `0x000dbded`; V: the assault rifle's
`wphi` → `ui\hud\master rounds` → `ui\hud\master`), and
`Converter::weapon_hud` writes one `nhdt` per `wphi`,
`<wphi name>_h1_weapon_hud`. The rebuilt weapon (`convert_arsenal`) names
it at `weap +0x2b0`; the Halo 2 counterpart's `nhdt` becomes the Halo 2
side of the weapon's HUD pair. `weapon_hud` is the function to call for
any other builder of a `weap`.

Halo 1's `wphi` layout, from its draw (E, Halo 1's XBE, `0x000db5d0`..,
`ebx` the `wphi`): child `+0x00`, anchor `+0x3c`, static elements `+0x60`
(`0xb4`), meters `+0x6c` (`0xb4`), numbers `+0x78` (`0xa0`), crosshairs
`+0x84` (`0x68`, each with overlays `+0x34`, `0x6c`), overlay elements
`+0x90`. An entry's head: the weapon state it reads `+0`, flags `+2` (bit
0 not drawn), screen type `+4`; the element at `+0x24`. Details in
`h1hud::wphi`.

| Halo 1 | Widget |
|---|---|
| static element | static, the `wphi`'s anchor, Halo 1's offset turned to screen directions, corner registration |
| meter on rounds loaded (state 1) | legacy meter on input `0x36`, loaded over the magazine (E `0x0013ae7c`; V: Halo 2's magnum) |
| meter on heat (state 2) | input `0x31`, `[ecx+0x14]` (E `0x0013af00`; V: the beam rifle's and sentinel beam's meters) |
| meter on battery (state 3) | input `0x38`, `1 - age` (E `0x0013af94`; V: the energy sword's) |
| aim crosshair overlay (type 0) | centred (E `0x000dad9e`: the crosshairs' header anchor is 4), Halo 2's battle rifle reticle's state flags |
| aim overlay with flag bit 2 (scope lines) | the same, with the scope masks' state flags (`+0x08` bit `0x40`, zoomed only) |

Halo 1's meter fills to `multiplier * value + offset` of 255 (E
`0x000e20a0`); on the disc that is the fraction the Halo 2 input gives (V:
the sniper rifle's 60 per round of 4 plus 1). The states are Halo 1's
(E `0x000db6e7`..): 0 rounds in reserve, 1 rounds loaded, 2 heat, 3
battery, 4 and 5 the second trigger's.

Left out, each named in the weapon's report: split-screen-only entries
(screen type 2), meters on rounds in reserve or the second trigger (no
Halo 2 input for them is established), warning crosshairs (types 3 and
up: reload, overheat, no ammunition, no grenades; V: `hud_reticle_warnings`),
the zoom-level readout (type 1, its sprite chosen by the zoom level),
aim overlays shown only unzoomed (bit 6), numbers and overlay elements.

## Not converted yet

- **Grenades** (`grhi`), **HUD globals** (`hudg`), **numbers** (`hud#`):
  Halo 2 draws counts as text widgets in its HUD font (`ui\hud\masterchief`
  `frag_count`), not from a numbers bitmap.
- **Motion sensor blips**: Halo 2 draws them by its own code; whether they
  follow the converted background's place is for a game run to show.
- Halo 1's element colours and scale are not carried: Halo 2's HUD
  shaders have their own colours.

## Tests

- `cargo test -p h1objects --lib h1hud`: weapon meters on Halo 2's
  inputs, crosshairs centred with their state flags, skipped entries;
  anchors, registration, the widget bytes (inputs, shader, state, no special type or effects,
  sequences, offsets, registration), the `nhdt` holding only its widgets.
- `cargo test -p boot --lib hud`: the swap both ways on bipeds and
  weapons, other HUDs left alone, refused targets, the campaign tables.
- `H1_ISO=... cargo test -p h1objects --test hud_disc -- --ignored`: every
  `unhi` on every Halo 1 map reads (root at least `0x56c`), each element a
  sprite or image of a `bitm`.
- `H1_ISO=... H2_DISC=... cargo test -p h1objects --test sandbox_disc -- --ignored`:
  Blood Gulch's player HUD in the built map: every interface names the
  `_h1_hud` `nhdt`, each widget a Halo 1 bitmap through the right shader,
  shield and health meters present; every Halo 1 weapon with a `wphi`
  names its `_h1_weapon_hud` (Halo 1 bitmaps, Halo 2 weapon inputs); the
  HUD pairs (the biped's and the weapons') in the manifest.
- `H1_ISO=... cargo test -p h1objects --test hud_disc -- --ignored
  weapon_huds_read --nocapture`: prints every `wphi` chain of `$H1_MAPS`
  as read.

## What a game run should show

On a converted Halo 1 map (converter version 53 or later) in the Halo 1
sandbox, or a Halo 1 campaign level: Halo 1's shield and health bars top
right, Halo 1's motion sensor frame bottom left, Halo 1's ammunition
counter top left and its crosshair centred (the sniper's scope lines only
zoomed), Halo 2's grenades as before. The shield bar drains from the right as the shield
falls, the health bar likewise and stays down (Halo 1's health model). The
whole HUD hides in the pause menu, under Blind and in cinematics. Each
element should sit where Halo 1 puts it (Placement above); one that does
not points at a difference in how Halo 2 scales HUD pixels to the screen.
