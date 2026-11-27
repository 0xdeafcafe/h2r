# Halo 1's HUD on converted Halo 1 maps

**Built.** Converted Halo 1 maps default to their source unit, grenade and
weapon interfaces. The HUD pair recorded by the converter also supports the
player's Halo 2 HUD choice. A read-only disc census found 1,497 authored
weapon draw entries in 360 interfaces across ten campaign and thirteen
multiplayer maps. The placement test also walks inherited entries for each
owning interface. Not run in the game.

Labels as in [health.md](health.md): **E**
engine code (`h2 disasm`), **V** checked on the owner's discs, **D** a
choice made here.

**Built.** Campaign scripts control help, objectives, help flashes and
the countdown. They can also hide or blink the health, shield and
motion-sensor HUD components and hide the crosshair. The addresses and
disc tests are in [text.md](text.md).

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
    right anchor and y on a bottom one (`0x000e07a0`). The independent
    scale factors apply to the sprite basis (`0x000e15f4`);
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

Moved to [Halo 1 HUD: weapon HUDs](hud-weapons.md#weapon-huds).

### Zoom level

Moved to [hud-weapons.md](hud-weapons.md#zoom-level).

## Grenades

Halo 1 draws the selected grenade type's `grhi` (E `0x000db3c0`..`0x000db5c1`:
the type `[unit +0x2cc]`, its `grhi` from the globals' grenades block
`matg +0x128`, `0x44` each, reference `+0x14`). `h1hud::grenade_huds`
reads each type's, `h1hud::grenade_elements` its background `+0x24` and
total background `+0x8c` (the element layout of the unit HUD), and
`Converter::unit_hud` appends them to the unit's `nhdt` per type, with
the state flags of Halo 2's own grenade background for that type
(`h1hud::GRENADE_STATES`; V: `ui\hud\masterchief` widgets 0 and 1, word 0
bit `0x04` frag and `0x08` plasma, the same bits as its `frag_count` and
`plasma_count` text widgets), so only the selected type's shows. Halo 1's
type order (frag, plasma) is Halo 2's, as the converted globals keep it.

### The count

Halo 1's count is the number element `grhi +0xf4`, drawn when byte
`+0x138` is set (E `0x000db514`) by the number draw `0x000e1a60`: offset
and scale as any element's, digits `+0x44`, flags `+0x45` (bit `0x04`
one cell more), fraction digits `+0x46`, each digit `hud#` `+0x11` wide
(9 on `ui\hud\counter`, the digits tag the globals' first interface
element names at `+0xb0`). It draws right to left from a start the anchor
moves: on a left anchor by `(cells - 2) * width`, on a right anchor not
at all, centred by half of `(cells - 1) * width` (E the switch
`0x000e1bd1`). On the disc every `grhi` count is one digit, top left, at
(107, 3), so it ends at x 107.

Halo 2 draws its counts as text widgets. The layout, from the retail
`ui\hud\masterchief` `frag_count` and `plasma_count` and their draw
(E `0x22aa16`, the string `0x22a871`, the value `0x22ace7`, the caller
`0x13a63d`), `0x54` bytes in the `nhdt`'s `+0x10` block:

| offset | field |
|---|---|
| `+0x00` | name (string ID) |
| `+0x04` | four inputs (`0x13ad48`); their values are `hud_input1`..`hud_input4` |
| `+0x08`..`+0x1b` | state flags, as a bitmap widget's (`0x13ac87`) |
| `+0x1c` | anchor (`u16`) |
| `+0x1e` | flags (`u16`): 1 draws the number `+0x28` names; 2 pads it to two digits, 4 to three; 8 draws the player's name; otherwise `+0x28` is a text |
| `+0x20` | shader (tag reference; no shader, no draw) |
| `+0x28` | string ID: `hud_input1` (retail) is the first input's value |
| `+0x2c` | justification (`u16`): 0 starts at the point, 1 centred, 2 ends at it |
| `+0x40`, `+0x44`, `+0x48` | offset (two `i16`) for fullscreen, half and quarter screen |

Retail: inputs 20 (frag count, HUD state `+0x38`) and 21 (plasma, `+0x3a`),
flags 1, `hud_input1`, `ui\hud\shaders\text_flash`, centred, top left at
(65, -1) and (12, -1); the `_hidden` pair draws through a dimmed variant of that shader.

The converter emits one text widget per type (`h1hud::grenade_count`):
input `20 + type`, that type's grenade states, flags 1, `hud_input1`,
`text_flash`, the grhi's anchor, font 0, justified to end (2) where
Halo 1's last digit ends (`h1hud::number_point`). The text is Halo 2's
font, not Halo 1's digit bitmaps. Not carried: the low-count flash
(`+0x148`) and the messaging part (`+0x14c`).

The text's font and where its digits fall:

- `+0x30` (`+0x31`, `+0x32` for half and quarter screen) is a font byte:
  0 is the engine's font 6, 1 its font 8 (E `0x22a85b`, called from the
  string fetch at `0x22a9b1`). Font 6 is `conduit-13` (V: line 6 of
  `maps\fonts\font_table.txt`); retail `frag_count` has 0.
- The draw (E `0x22aa16`) puts the text's rectangle top at the widget's
  point and the measured height under it (`0x22ac83`); a glyph's bitmap
  top is the baseline less its y offset, the ascent on every glyph
  (docs/formats/fonts.md), so it is the rectangle's top. `conduit-13`'s
  digits have ink on rows 5 to 16 of their 21 (V; `h1hud::TEXT_DIGIT_INK`).
- Halo 1's digit sprites (`hud_counter_numbers`) are 12 rows, drawn from
  the number's point down on a top anchor.

At Halo 1's y (3) the text's top would put the digits' ink on rows
8..20 under the indicator's top, which ends at row 17 (V: the `grhi`
background's texels, x 82..119, y 1..17), sticking out of the bottom of
the indicator. The point's y is therefore set so the ink is centred on Halo 1's digit sprite: y -2, ink rows 3..15,
Halo 1's sprite rows 3..15 (`hud_layout_disc`
`grenade_count_digits_sit_inside_the_indicator`).

## Scale and anchors

Halo 1 multiplies the draw basis by the two authored floats at `+4` and
`+8` (E `0x000e15f4`, `0x000e15ff`). Placement comes separately from
`0x000e07a0`: the offset is never scaled, so a scaled sprite keeps its
anchor corner and grows away from it.

Every element (unit, grenade and weapon bitmaps) is built the same way.
`h1hud::widget_for` and `h1hud::weapon_widget` put Halo 1's factors in
`Widget::scale`, and `h1hud::nhdt` writes a scaled widget's effects element
(`Widget::effects`): scale flag 1 and independent constant functions in the
native X and Y slots (`+0x10`, `+0x24`), which Halo 2 applies to each basis
vector about the widget's registration point (E `0x22be12`..`0x22beaa`).
The registration is the anchor's corner (`h1hud::registration`), so the
element keeps Halo 1's corner. An element with scale 1 has no effects
element. In split screen Halo 1 first halves the crosshair basis unless
overlay `+0x0c` bit 1 is set (`0xdae8f`); the boot applies that factor
before the native effect.

`h1hud::check_scale` refuses a factor that is not finite or not above 0
(none on the disc; a named error, not a guess). The motion sensor's
placement scale does not move its centre and is checked the same way; the
sweep it would size is not converted. A number's scale is the number
draw's, not the element's (`h1hud::number_scale`: a factor not above 0 is
1.0). The weapon numbers pass 0. The grenade count's argument is not read
from the XBE, so a grenade count whose result is not 1.0 is a named
conversion error.

### At any aspect

The runtime places a HUD draw in the 640 x 480 frame at the uniform scale
`min(w / 640, h / 480)` and pins each corner group to its own window corner
(docs/graphics/high-resolution.md, "HUD and UI"). So at 4:3 and at 21:9 the
Halo 1 anchors land on the window's corners (`h1hud::hud_corner`: 0 top
left, 1 top right, 2 bottom left, 3 bottom right, 4 the centre), the offset
stays Halo 1's HUD pixels, and a sprite's size, scale included, is a fixed
fraction of the window's height: `size * scale / 480`.
`h1hud::halo1_screen_rect` (Halo 1's rules) and `h1hud::halo2_screen_rect`
(the converted widget) model both and the tests compare them.

## Colours: alpha only; the motion sensor

Halo 1 tints every element with its colour (ARGB `+0x34`; overlays
`+0x24`), alpha included (E `0x000e1830`). On the disc the colours are
few: `0x2896ff` and `0xaad5ff` blues at alphas `0xc8`..`0xff`, and the
motion sensor's background `0x28061428` (alpha `0x28`: a faint dark disc)
and foreground `0xa06986a0`. The widgets draw through
`widget_gradient_flash`, whose colours come from its own constants (its
`colors` block and six animated pixel-constant bindings), not from the
widget; the texture's alpha is kept (`a = t0.a * c0.a`).

So the converter carries the alpha: a static unit, grenade or weapon
element whose colour's alpha is under 255 draws a copy of its bitmap with every
texel's alpha scaled by it (`h1hud::scale_alpha`,
`Converter::hud_bitmap`, `<bitmap>_alpha_<aa>`). Formats with an alpha
byte (A8, A8Y8, A8R8G8B8) are scaled in place. The rest (block-compressed,
16-bit, no alpha), among them the motion sensor's
(`hud_weapon_backgrounds`, sequence 3), are decoded with Halo 2's own
decoder (`h1bitmaps::decode_image`) and the copy written as linear
A8R8G8B8, top level only, rows at Halo 2's 64-byte linear pitch
(`h1hud::alpha_scaled_argb`). Retail Halo 2 HUD bitmaps are stored the
same way (E: `grenade_bkd`'s images, linear A8R8G8B8). Before this the
sensor background would draw opaque in the shader's light blue; with
the copy it is a faint disc at `0x28/255`.
Halo 1's colour RGB needs a `shad` per colour whose
constant reaches the pixel shader's last-stage `c0`; which `colors` entry
or animation binding feeds it is not established
(`docs/formats/shader-instances.md`: E `0x0001717b`).

## The motion sensor's centre

Halo 2's `nhdt` does not place the sensor's blips: the retail
`ui\hud\masterchief` sensor is only bitmap widgets 5 and 7
(`motion_sensor`, anchor 2, offset (-1, -84), no special type). The game
draws the blips (`0x1a2823`) around a point of its own (E
`0x00139614`..`0x00139685`): anchor 2 (bottom left) through `0x1396c7`,
plus an entry of the table at `0x47ff40` (`.data`) for the screen mode,
two `i16` each: (41, -42) fullscreen, (31, -31) half and quarter screen
(V: the disc's XBE), the centre of the 83 x 83 background.

Halo 1 draws its sweep and blips (`0x000da1e0`) around the point its
`unhi` motion sensor placement `+0x35c` gives (an offset and scale, placed
by `0x000e07a0` from the sensor's stand-in anchor, bottom left;
E `0x000dd658`..`0x000dd691`). `h1hud::sensor_centre` turns it into the
table's terms (y up from the bottom); the converter records it beside the
HUD pairs, `hud_sensor = [x, y]` in a package's `[[variant]]`, `sensor`
in a campaign level's `.hud.toml`.

`boot::hud_swap` writes it into all three entries of the table at each
game activation while Halo 1's HUD is shown (the Halo 1 widgets use one
offset in every screen mode), and at every other activation, any map,
puts back what the table held before its first write (`hud_swap::Sensor`).

## Layout: where each element lands (`hud_layout_disc`)

Both games lay a sprite out the same way, so every converted widget
stands where Halo 1 puts its element. Halo 1 (E, its XBE): `0x000e07a0`
places the point, the offset negated across on a right anchor and down
on a bottom one; `0x000e0330` lays the sprite `[0, w]` or `[-w, 0]` across
and `[0, h]` or `[-h, 0]` down by the anchor, its size the sprite's
bounds times its image's size (`h1hud::halo1_rect`). Halo 2 (E
`0x22aff1`): the widget's registration (fractions of the sprite) lands on
the anchor point plus the offset, the sprite's size the same product
(`h1hud::halo2_rect`). Both draw in a 640 x 480 HUD frame (the recomp
scales it as a whole, docs/graphics/high-resolution.md); the corners
differ (Halo 2's are the safe area's), which moves a whole corner group
together and nothing within it. The disc test checks the two rectangles
equal for every element of every unit HUD on the disc, both grenade
HUDs and every converted static and meter of Blood Gulch's weapons.

### Shield over health

The cyborg's shield meter sprite (122 x 31) and health meter (88 x 14)
overlap as rectangles in Halo 1 too: the health meter sits in the shield
sprite's lower rows. Halo 1 shows only texels with alpha, and no texel
of the two shows in both (V, every unit HUD on the disc).

Halo 2's legacy meter shader shows more. Its alpha is the texel's alpha
plus, on the filled side, the texel's green (`h1hud::legacy_meter_alpha`;
V: `ammo_meter`'s combiners, stage 0's `c0` fixed at `0x0000ff00`,
stage 3's `c1` at `0xffffffff`, nothing the engine writes). Halo 1's meter
sprites are A8Y8 with their luminance ramp carried under the clear parts
too: the cyborg's shield meter has 1435 clear texels with a luminance,
722 of them on the health meter's visible texels. With both meters
full, 849 texels would show both (297 on `cyborg_mp`): the shield drawn
over the health meter.

The converter gives each meter its own copy, `<bitmap>_meter`, with
every clear texel's colour made black (`h1hud::clear_meter_colour`;
other formats decoded to linear A8R8G8B8 first). A visible texel is
unchanged, and the copy shows exactly Halo 1's texels (checked for the
unit meters of every map and Blood Gulch's weapon meters). Weapon meters
get the same copy.

## The HUD globals (`hudg`): not converted

Halo 2's `hudg` is Halo 1's with Halo 2's additions at the end: from
`+0x3f8` on (strings, the HUD sounds `+0x428`/`+0x430`, more blocks) it
is Halo 2's own; before that every field of Halo 1's is there in Halo
1's order, a 16-byte Halo 1 reference becoming an 8-byte one (so the
offsets drift by 8 per reference). Compared on the discs (V: Blood
Gulch's `ui\hud\default` against ascension's, lockout's, 01a's and
`shared`'s, which are alike), byte for byte:

| Halo 1 | Halo 2 | What | Same? |
|---|---|---|---|
| `+0x00..+0x48` | `+0x00..+0x48` | message placement | same |
| `+0x48`, `+0x58` (fonts) | `+0x48`, `+0x50` (`bitm`, null) | | Halo 2's differ |
| `+0x68..+0x94` | `+0x58..+0x84` | message timing | same but `+0x90` 1.35 / `+0x80` 1.1 |
| `+0x94`, `+0xa4`, `+0xb4`, `+0xc4` | `+0x84`, `+0x8c`, `+0x94`, `+0x9c` | item and icon messages: strings, icons, their list | Campaign event text uses the rebuilt message and argument rows. See [HUD text](text.md). |
| `+0xd0..+0xf0` | `+0xa4..+0xc4` | text colours, flash (E Halo 2 reads `+0xa4`, `+0xa8`, `+0xb6`: `0x0024d1a2`..) | same |
| `+0x100..+0x150` | `+0xcc..+0x11c` | (E Halo 2 reads `+0xe8`, `+0xea`: `0x0024c8ca`) | same |
| `+0x150`, `+0x160` | `+0x118`, `+0x120` | the waypoint arrow: bitmap and arrow list (10 in Halo 1, 15 in Halo 2; E Halo 2 `0x0024dacb`) | Halo 2's own assets |
| `+0x16c..+0x2c0` | `+0x12c..+0x280` | | same |
| `+0x2c0` (`wphi`, the unarmed HUD, E Halo 1 reads `+0x2cc`) | `+0x288` (null, not read) | | |
| `+0x2d0` | `+0x290` | motion sensor range (E Halo 1 `0x000da21a`, Halo 2 `0x001a26e2`) | **20.0 / 8.0** |
| `+0x2d4` | `+0x294` | motion sensor movement threshold (E Halo 1 `0x000d97b5`: speed squared; Halo 2 `0x001a1deb`: speed, and only for objects that are not units) | **0.0002 / 0.03** |
| `+0x2d8..+0x338` | `+0x298..+0x2f8` | sensor scale 32 and the rest | same |
| `+0x338` | `+0x2f8` | damage indicator bitmap | Halo 2's own asset |
| `+0x348..+0x3c8` | `+0x300..+0x380` | damage indicator colours and timing | same |
| `+0x3d8`, `+0x3e0` (`snd!`, E Halo 1 `0x000e294b`) | `+0x388`, `+0x390` (E Halo 2 `0x0012b7bc`) | | same; the sound is null in both |

So Halo 1's colours, multiplayer and damage settings are already Halo
2's numbers, and the one sound both read is null on both discs: there is
nothing of them to carry. What differs is the motion sensor's range and
movement threshold, one message time, and the assets (the waypoint
arrow's bitmap and arrows, the message icons, the damage arrows).

Not converted, for two reasons:

- `hudg` is one tag for the whole game (Halo 2 keeps it at `[0x00510c94]`,
  E `0x00191764`: `matg +0x110` interface element `+0x30`), not a HUD's,
  while the HUD style is the player's choice at run time ("The switch").
  Halo 1's values belong behind the switch, written at game activation as
  the sensor centre is (`boot::hud_swap`), not into the map.
- The range alone is not Halo 1's sensor: Halo 1 draws a blip at
  `distance * scale / range` pixels (`+0x2d8` 32 over `+0x2d0` 20,
  `0x000da209`), Halo 2 at `(distance / range + 1) * 32` in a 64-wide
  grid (`0x001a27d0`) that its draw (`0x001a28ac`..`0x001a2917`) then
  scales to the screen, a factor not read. Halo 1's range belongs in
  Halo 2's `+0x290` only once that factor is known, scaled by it.

- **HUD globals** (`hudg`). See the field comparison above.
- **Motion sensor sweep**: Halo 1 draws a sweep around its sensor
  centre; Halo 2's sensor draws its own blips only. The sweep is not
  carried.
- **Motion sensor range**: the blips keep Halo 2's radius (its 83 x 83
  background); Halo 1's centre is carried, its sensor size is not.

## Tests

- `cargo test -p h1objects --lib` covers primary-only weapon widget
  visibility with the retail widget gate and zoom fields rebuilt from
  synthetic Halo 1 weapon tags.
- `H1_ISO=... H2_DISC=... cargo test -p h1objects --test it sandbox_disc:: -- weapon_hud_ --ignored`
  checks retail held, off-hand and stowed placement and gating on
  Coagulation and Zanzibar. It also checks the weapon zoom fields and
  both widget types' hide flags in the freshly built Blood Gulch map.
- `cargo test -p h1objects --lib h1hud` covers all weapon meter sources,
  padding, magazine and target selectors, zoom and screen flags, dormant
  entries, placement and native scale and flash data. It also checks each
  Halo 1 anchor against its window corner at 4:3 and 21:9
  (`each_halo1_anchor_maps_to_its_screen_corner_at_4_3_and_21_9`), a scaled
  element's size as a fixed fraction of the screen height
  (`a_scaled_element_keeps_its_halo1_size_relative_to_the_screen_height`)
  and the scale effect an `nhdt` carries
  (`a_scaled_element_writes_one_scale_effect_and_an_unscaled_one_none`).
- `cargo test -p boot --lib h1_hud_text::weapon` checks warning predicates,
  clock transitions, input transfer and the draw frame.
- `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1objects --test it hud_layout_disc:: -- --ignored every_retail_weapon_hud_element_is_placed`
  compares the source entries, including child interfaces, with their
  actual rebuilt bitmap and text records on each map's base.

- `cargo test -p boot --lib hud`: the swap both ways on bipeds and
  weapons, other HUDs left alone, refused targets, the campaign tables.
- `H1_ISO=... cargo test -p h1objects --test it hud_disc:: -- --ignored`: every
  `unhi` on every Halo 1 map reads (root at least `0x56c`), each element a
  sprite or image of a `bitm`.
- `H1_ISO=... H2_DISC=... cargo test -p h1objects --test it sandbox_disc:: -- --ignored`:
  Blood Gulch's player HUD in the built map: every interface names the
  `_h1_hud` `nhdt`, each widget a Halo 1 bitmap through the right shader,
  shield and health meters present; every Halo 1 weapon with a `wphi`
  names its `_h1_weapon_hud` (Halo 1 bitmaps, Halo 2 weapon inputs); the
  HUD pairs (the biped's and the weapons') in the manifest; the motion
  sensor background's copy, decoded back from the built map, has a mean
  alpha inside its circle within 1 of `0x28/255` of Halo 1's; the
  grenade counts are two number texts on inputs 20 and 21 reading
  `hud_input1` through `text_flash`; the manifest's `hud_sensor` is the
  cyborg `unhi`'s sensor centre. `halo2s_motion_sensor_table` checks the
  XBE's table.
- `cargo test -p boot --lib hud`: also the sensor table both ways, and
  Halo 2's back on a map without a Halo 1 centre.
- `H1_ISO=... H2_DISC=... cargo test -p h1objects --test it hud_layout_disc::
  -- --ignored`: every element's rectangle as Halo 2 draws the widget
  against Halo 1's; shield and health meters sharing no shown texel (and
  the unconverted bitmaps sharing some on the cyborg's); the grenade
  count's digits, in Halo 2's own font, inside the indicator and centred
  on Halo 1's digits (and outside it at Halo 1's y); each weapon count's
  digits, every digit at every length up to Halo 1's digit count, inside
  its weapon's panel, ending at Halo 1's last cell and centred on Halo
  1's digit rows; Halo 1's blend mode 7 from the XBE and Blood Gulch's
  six dormant colour-0 statics; each weapon inheriting
  `master rounds` or `master plasma` with one ammunition type icon
  (`ammo_type_icons_are_drawn_always`), placed as Halo 1's.
- `H1_ISO=... cargo test -p h1objects --test it hud_disc:: -- --ignored
  weapon_huds_read --nocapture`: prints every `wphi` chain of `$H1_MAPS`
  as read.

## What a game run should show

On a converted Halo 1 map in the Halo 1
sandbox, or a Halo 1 campaign level: Halo 1's shield and health bars top
right, Halo 1's motion sensor frame bottom left, Halo 1's ammunition
counter top left with its count (rounds in reserve; the battery's
percentage on the plasma weapons) in Halo 2's font inside the panel,
right-aligned where Halo 1's last digit is, with Halo 1's leading
zeros, and its crosshair centred (the sniper's scope lines only
zoomed), Halo 1's grenade indicator right of the ammunition counter with
its count in Halo 2's font inside it, not below it. Halo 1's "Reload"
below left of the crosshair once the magazine is at or below the
weapon's threshold (assault rifle 10 rounds) with rounds in reserve, gone
after reloading or with nothing in reserve; "No ammo" when firing empty, "Battery depleted"
when firing a spent plasma weapon, and the source flash on each warning. Zoomed with
the sniper rifle: the view darkened around the scope, "2x" at the first
zoom and "10x" at the second. The pistol and rocket launcher show
their scope mask when zoomed too. Nothing of
`hud_ammo_alphas` under the ammunition panel (colour 0: Halo 1 does not
draw it); the panel itself slightly translucent, as
Halo 1's (alpha `0xdc`). With the shield
full, only the shield bar's own shape is lit: nothing of it over the
health bar under it. The shield bar drains from the right as the shield
falls, the health bar likewise and stays down (Halo 1's health model). The
whole HUD hides in the pause menu, under Blind and in cinematics. Each
element should sit where Halo 1 puts it (Placement above); one that does
not points at a difference in how Halo 2 scales HUD pixels to the screen.
