# Halo 1's HUD on converted Halo 1 maps

**Built.** Converted Halo 1 maps default to their source unit, grenade and
weapon interfaces. The HUD pair recorded by the converter also supports the
player's Halo 2 HUD choice. A read-only disc census found 1,497 authored
weapon draw entries in 360 interfaces across ten campaign and thirteen
multiplayer maps. The placement test also walks inherited entries for each
owning interface. Runtime verification of this rebuild is pending.

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

`h1hud::weapon_entries` reads a weapon's `wphi` and its children (the
child first, as Halo 1 draws it: E `0x000dbded`; V: the assault rifle's
`wphi` → `ui\hud\master rounds` → `ui\hud\master`), and
`Converter::weapon_hud` writes one `nhdt` per `wphi`,
`<wphi name>_h1_weapon_hud`. The rebuilt weapon (`convert_arsenal`) names
it at `weap +0x2b0`; the Halo 2 counterpart's `nhdt` becomes the Halo 2
side of the weapon's HUD pair. `weapon_hud` is the function to call for
any other builder of a `weap`. A chain with no element at all (V: the
multiplayer maps' flamethrower, `flame thrower` → `master fuel`) draws
nothing in Halo 1 and becomes an `nhdt` with no widget (since converter
v87; before, it was refused and the flamethrower kept the shotgun's
Halo 2 HUD). A failed element stops conversion with the map and source interface in the error.

### Held and stowed weapons

Halo 2 calls a weapon's `nhdt` for the primary weapon, the off-hand weapon
and the stowed weapon. It looks up those three weapons separately
(E `0x00139ca0`, `0x00139d00`, `0x00139dba`) and sets word C to
`0x01`, `0x02` or `0x04` for their passes (`0x0013a1d8`).
Retail Coagulation and Zanzibar's shotgun and sniper rifle panels hide
on `0x06`. The magnum has a second panel on the opposite corner for the
off hand. All three use a dedicated `backpack` bitmap widget at a different
offset from their held panels. It shows on `0x04`.

The overlap in converted Blood Gulch came from using the shield-meter
state for weapon panels, meters, icons and counts. Word A could show them
on every pass, so a stowed weapon drew its entire panel at the same
top-left coordinates as the held weapon. The older Blood Gulch serve
reported both the assault rifle and sniper reserve counts at the same
top-left offset with `hide_when_c` zero (`h2 agent edit get`).
The widget gate (`0x0013ac87`) accepts a match on **any** show word. A primary show bit
alone would still permit a stowed draw when another show word matched.

Halo 1 draws a single weapon HUD. Its converted bitmap and text widgets
all set the retail primary panel's hide mask `0x06` in word C
(`h1hud::primary_weapon_state`). The panel, reticle, warnings and
zoom widgets draw for the active weapon at Halo 1's positions.
The converted Halo 1 HUD has no separate backpack icon.

### Elements

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
| meter states 0–5 | native legacy meter with the source minimum, multiplier and bias |
| reserve, loaded rounds and battery percentage | number text with authored padding and screen selector |
| counts in magazines | signed whole division by the active weapon's first magazine capacity |
| secondary reserve, secondary loaded rounds and heat | number input from the corresponding magazine or heat byte |
| sniper range and elevation | signed, fixed-precision number texts while the target is fully acquired |
| aim and zoom crosshairs | centred sprite or direct image, gated by source zoom flags |
| warning crosshairs | centred sprite with the owner's thresholds, source flash colours and clock |
| ammunition type icon | source overlay bitmap at its authored anchor |

**Built.** The converter builds both widget blocks from the Halo 2 `nhdt`
definition. A hidden source entry or transparent static keeps its widget
with empty show words. Source screen selectors 1 and 2 select single-player
and split views by local player count. Word C excludes the off hand and the
stowed weapon. The vehicle draw uses pass 4, whose weapon snapshot is at
HUD `+0x50` and whose slot bits are zero (H2 `0x13a069`, `0x13a1d8`).

H1 `0xdbc4e` reads reserve, loaded rounds, heat, remaining battery and the
second magazine's two counts. The meter reads the low byte of the selected
value before applying the multiplier and bias and bounding it by the
authored minimum and 255 (H1 `0xe213a`–`0xe21c7`). Four rounds in the
sniper rifle give 241/255. H2 `0x100603` and `0x10064a` walk the two
magazine records with a 16-byte stride. The draw adapter supplies the
result to the native legacy-meter shader at `0x13ad48`.

### Overlay elements: the ammunition type icon

Halo 1 draws a `wphi`'s overlay elements (`+0x90`, `0x68` each) after its
numbers (E `0x000dc031`..`0x000dc0b1`): each has an entry head as the
statics' (state `+0`, flags `+2`, screen type `+4`), and at `+0x24` a
bitmap (reference) and its overlays (block `+0x34`, `0x88` each), drawn
by `0x000e16b0` at the `wphi`'s anchor. An overlay (`h1hud::wphi::
element_overlay`): offset `+0` and scale `+4` as an element's (it goes
to the element draw `0x000e1590` as an element does, so it is laid out
the same way), colour `+0x24`, frame rate `+0x44` (0: the sequence's
first sprite, `0x000e174c`), sequence `+0x48`, the conditions it shows
on `+0x4a`, flags `+0x4c` (bit 0 flashes, bit 1 not drawn, `0x000e16dd`).

The conditions (`h1hud::overlay_shown`) are a mask tested against one
the weapon draw builds per state (E `0x000db917`..`0x000db9d6` for the
rounds in reserve and loaded): 1 below the state's threshold with no
reload under way, 2 the value is 0, 4 a reload under way, 8 none of
those, and `0x10` on every state. On Blood Gulch (V) the overlay
elements are the shared `ui\hud\master rounds` (two, full screen and
split screen; one overlay each, `hud_ammo_type_icons` sequence 0 at
(53, 4), colour `0xffe6f0ff`, shown always) and `ui\hud\master plasma`
(one, sequence 4 at (51, 2), shown always, flashing). The converter
draws an overlay shown always as a static (`h1hud::weapon_widget`,
`h1_weapon_overlay`): the icon right of the count. Flashing is not
carried (as for statics), nor overlays shown only on a condition
(`Skip::OverlayCondition`) or animated (`Skip::Animated`). Neither kind
is on the disc (V: `no_overlay_on_the_disc_is_shown_on_a_condition`,
every `wphi` of every map: the three overlays above are all its overlay
elements). Should one appear, "below the threshold" and "empty" have
Halo 2 states ("Warnings" below) and "reloading" has none.

### Warnings

Halo 1's warning crosshairs (types 2 and up, `h1hud::crosshair_type`)
are crosshairs on `hud_reticle_warnings`, a sheet of words; each type is
active on its own test of the weapon state (E `0x000da70a`, the table at
`0x000daa18`), against the low thresholds of the weapon's own `wphi`
(`+0x14` rounds in reserve, `+0x16` loaded, `+0x18` heat, `+0x1a`
battery; the test reads the `wphi` it was called with, `0x000da67b`, not
the child the warning sits in). On the disc (V, every `wphi` of every
map):

| Type | Active when (E) | Sprite (V) | Where | Converted |
|---|---|---|---|---|
| 3 | rounds in reserve, loaded at or below `+0x16` (`0x000da7ab`) | "Reload", sequence 1 | `master rounds` | yes |
| 6 | battery below 1, remaining percent at or below `+0x1a` (`0x000da814`) | "Low battery", 0 | plasma pistol, plasma rifle | yes |
| 7 | a reload under way (state `+0x0c`, `0x000da7a1`) | arrows, 9 | Banshee and Scorpion guns | yes |
| 8 | trigger held with nothing loaded or in reserve (`0x000da874`) | "No ammo", 2 | `master rounds` (`empty`: "No weapon", 6) | yes |
| 9 | no grenades, the grenade action idle and the throw pressed (`0x000da889`) | "No grenades", 5 | `master` | yes |
| 10 | nothing in reserve, loaded and at or below `+0x16` (`0x000da8e7`) | "Low ammo", 7 | `master rounds` | yes |
| 18 | battery spent (age 1.0), the trigger held (`0x000da855`) | "Battery depleted", 10 | `master plasma` | yes |

The boot samples the active weapon and the local unit on simulation ticks.
For type 9, H1 checks the idle action byte at unit `+0x23d` (`0xda8bb`).
H2 stores the equivalent byte at action block `+0x0c`. The biped supplies
the signed block displacement at `+0x346` (`0xe775b`, `0xe78d4`, `0xe78e9`).
Each local user has independent clocks for their personal and vehicle
weapons. H1 starts a type's clock at `0xda9d4`. Most warnings draw while
their predicate holds. The empty-trigger and failed-throw branches keep
their clock through the rest of the authored display window after release
(`0xdb052`). The overlay's flash record carries both colours, period,
delay, count, flags and on-time. Retail periods include 3 seconds for
ammunition, 4.05 for low battery and 1 for vehicle arrows. The arrows
also advance through their sequence from the same clock (`0xdb094`).

H2 supplies weapon state in `0x13afa9`. A converted warning stores the
owning interface's four signed thresholds and the original 24-byte
flash record in the inactive rotation-data slot of its bitmap effect.
The native bitmap draw at `0x22aff1` saves its caller at frame `+0x68`.
The tint adapter verifies the return to `0x13a62a` before looking up the
weapon that actually supplied the widget. Switching HUDs returns the
native widgets to the normal input, visibility and material paths.

v83 showed "Reload" on a full magazine (seen 2026-11-19 on the hud
serve: assault rifle, 60 loaded, 180 in reserve). The converter wrote the
thresholds right (`00 0b 01 00` at the widget's `+0x18`), but the
from-scratch map build reads every tag through the inferred schema, which
typed that word as a raw-resource pointer; it copied "the resource" at
file offset `0x10b00` and wrote the copy's offset there (`00 da 22 01` in
the built map: rounds loaded below 218, so always). Since v84 the schema
leaves the word out (`tags::schema::infer::ENGINE_DATA`,
docs/formats/tag-schema.md step 9), so the bytes reach the map as
written. The live proof of the gate above wrote the thresholds into
memory with `edit.set`, after the build, which is why it did not see this.
Live, v83 map (2026-11-19): writing each weapon's thresholds back with
`edit.set` (`bitmap_widgets[3].raw_18`, assault rifle `0x00010b00`)
hid "Reload" at 60 loaded, showed it at 5 loaded with 180 in reserve, and
showed "No ammo" alone at nothing loaded and nothing in reserve. Every
other weapon's HUD, drawn on the held assault rifle by pointing its `hud`
reference at it, had its panel, count, icon and meter where Halo 1 puts
them and no widget over another; only the always-on "Reload" overlapped
the crosshair.

### Zoom level

Halo 1 draws the zoom-level readout (crosshair type 1) as one overlay
whose sprite the zoom level picks. The type's value is the unit's zoom
level plus 2 (E `0x000da775`; the level from `0x000f8c80`, 1 unzoomed),
kept less one (`0x000da9e5`). An overlay with flag bit 2 draws only while
that is above 0 (`0x000dae69`) and takes sprite value less one
(`0x000daf55`..`0x000daf60`): sprite 0 at zoom level 0, sprite 1 at
level 1. V: the sniper rifle's is `ui\hud\bitmaps\sniper\hud_reticles_scope`
sequence 1, two sprites, "2x" then "10x".

A Halo 2 widget draws its sequence's first sprite, and Halo 2's state
words tell the zoom levels apart (word A `0x40` unzoomed, `0x80` level 0,
`0x100` level 1; E `0x0013a0df`..`0x0013a127`, the unit's `+0x3c`). So
the converter writes a widget for each authored level sprite.
Each copy has a single selectable frame, including the first level, so
target acquisition cannot switch the label. Without flag bit 2, frame
zero is the unzoomed label. Bit 6 hides an overlay at both zoom levels.
Bit 1 selects the source image directly (H1 `0xdaeb2`, `0xdb12b`).
The rebuilt bitmap gives each such image a one-image native sequence. The states are
Halo 2's own sniper rifle
"5x" and "10x" (V: `ui\hud\sniper_rifle`): show word A `0x90`, hide
`0x140`; show `0x110`, hide `0xc0`; both shown on word C `0x01` and
hidden on `0x06` (`ZOOM_LEVEL_STATES`). The widgets use Halo 1's
sprite placement. Their draw has yet to be checked in a game run.

Converter version 99 retains the zoom levels of Halo 1's pistol and sniper
rifle. The assault rifle and other unscoped weapons have
zero levels and suppress Halo 2's binocular fallback.
See [Weapon zoom](objects.md#weapon-zoom).

### Scope mask

The sniper rifle's scope lines and zoom labels come from the converted
crosshairs. Its surrounding mask is a separate screen effect. The
sniper rifle's `wphi +0xac` block references the full-screen
`sniper_scope_mask2` and split-screen `sniper_scope_mask_sm` at
element `+0x18` and `+0x28` on Blood Gulch. The pistol and rocket
launcher reference their scope masks through the same block.

Halo 1 looks at the first screen effect in the weapon's own
`wphi +0xac`, before drawing the weapon HUD (E `0x000d6b37` and
`0x000d6e33`). The effect is `0xb8` bytes. Flag 0 at `+0x04` gates
the mask on zoom. The references at `+0x18` and `+0x28` select the
first image of the full- and split-screen bitmap respectively
(E `0x000d6b85`.. `0x000d6bb2`). The pistol and rocket launcher share
the pistol art. The sniper rifle has its own masks (V, Blood Gulch).
The mask's luminance describes the dark surround, transparent opening
and brighter edge (V, both screen modes from the disc).

`h1hud::scope` converts those images into one `bitm` with a sequence per
mode. The rebuilt `nhdt` puts the effect at `+0x20`. Halo 2 gates it
before the weapon widgets (E `0x0013a57d`.. `0x0013a5d8`) and selects
the bitmap sequence and `egor` reference by mode (E `0x0022b8e2`).
The new `egor`, `shad`, `stem` and `spas` use the layer-15
screen-effect draw. The pass samples the HUD's engine texture 16
(E `0x0022bb20`, `0x0022bb9f`, `0x00019817`) and blends black
into the view by the mask's luminance. Its UVs come from the position
normalized inside the current viewport (E `0x0002d5e0`), so each view
gets the whole mask.

### Zoom blur and colour filters (**Built**)

The same Halo 1 effect has a separate zoom gate and two
angle/radius pairs at `+0x40`. The draw interpolates between them
using the rendered camera's FOV, clamps to the endpoints and skips blur
in split screen (E `0x000d6bd2`–`0x000d6c63`). The pistol, rocket
launcher and sniper rifle specify the curve (V, Blood Gulch). The
converted full-screen effect copies the current view to target 1,
uses Halo 2's screen convolution (`0x0002da96` → `0x000211a0`)
with the interpolated spread, and blends that result by mask alpha
before the mask's luminance overlay. It uses Halo 2's four-tap
convolution kernel, with two steps, at half resolution. Halo 1 used
radially spaced taps (E `0x0006bacc`–`0x0006bc36`).

Desaturation at `+0x6c` and tint at `+0x8c` have independent
zoom and night-vision gates and strengths. The colour pass averages
the scene's RGB for desaturation and uses that luminance for the
additive or blended tint. Each filter can limit its coverage using
the scope mask's luminance (E `0x000d6c6e`–`0x000d6e23`,
`0x0006cb9d`–`0x0006cde8`). The sniper uses both filters, with green
additive tint, in full and split screen (V, Blood Gulch and a30).
The four Halo 1 script channels start at 1 (E `0x000805bf`,
`0x0011a8fe`), and the retail maps leave them at that value. The
converted draw uses those initial values for the valid channels.

The `h1-scope-zoom` feature reads the profile only on converted HUD
effect draws (`0x0022bbbb`). It supplies the current view's blur radius
and the two colour strengths. The local flashlight button toggles
night vision on supported scopes. The unit-light update at
`0x000c50a0` advances its fade with simulation time, using Halo 1's
rise and fall rates (`0x00045c6c`, `0x00045cae`). State follows the
local unit and resets when the game activates.

The synthetic checks cover the gates, the FOV curve, the per-view
runtime inputs, the fade and the pixel combiner. The ignored disc
check `scope_masks_use_the_hud_screen_effect_in_each_view` resolves
the rebuilt shader passes and checks the mask images and zoom
profiles against the discs. In-game appearance remains to be verified.

### The sniper rifle's angle ticks

The sniper rifle's own `wphi` has two statics on `weapons\sniper
rifle\bitmaps\angle_ticks_black` at (92, 85) and (445, 85). Halo 1
draws statics whatever the zoom (E `0x000dbe4d`..`0x000dbead`: only the
hidden flag and the screen type are tested; `0x000e1830` has no early
out), but that bitmap (DXT1, 8 x 256) is black with alpha 0 in every
texel (V), so under blend mode 7 it adds nothing: Halo 1 shows nothing
there, and the converted widgets show nothing either (the shader keeps
the texture's alpha). No zoom gate is needed.

### The ammunition and battery counts

Halo 1's weapon numbers (`wphi +0x78`, `0xa0` each; drawn at E
`0x000dbf1e`..`0x000dc0f0` by the number draw `0x000e1a60`, the grenade
count's) are read by `h1hud::weapon_entries` and written by
`h1hud::weapon_number` as Halo 2 number texts, placed as the grenade
count is (`h1hud::number_point`, "The count" below): right-justified to
end where Halo 1's last digit cell does, the digits' ink centred on
Halo 1's digit sprites' rows. The number element: offset `+0x24` (its
anchor the `wphi`'s), colour `+0x48` (the draw's default, element
`+0x24`, `0x000e1c85`), digits `+0x68`, flags `+0x69`, fraction digits
`+0x6a`; entry `+0x78` bit 0 divides the value by the magazine size
(`0x000dbf5f`, `0x000dc0de`: "in magazines").

The values (E `0x000dbc4e`..`0x000dbc99`, from the per-magazine block
`0x000be2f0` fills): state 0 the magazine's `+6`, state 1 its `+8`,
state 3 `(1 - age) * 100`. Halo 2's inputs: `0x33` is `[ecx+8]`, filled
from `0x001008f0`, which reads the magazine's `+0x22a`, the rounds in
reserve (V: retail Halo 2's battle rifle, magnum, shotgun and rocket
launcher counts read it); `0x30` is `[ecx+6]`, the magazine's `+0x22c`,
the rounds loaded; `0x32` is `100 - [ecx+0x10] * 100`, the battery's
percentage. Halo 1's magazine holds reserve then loaded at `+6`, `+8`, as
Halo 2's does at `+0x22a`, `+0x22c` (inferred from the field order).
For a converted reserve count the input adapter reads the active weapon's
own reserve directly, as H1 does at `0xbe3c5`.

On Blood Gulch (V) the full-screen counts are one per weapon: rounds in
reserve, three digits at (29, 3) on the assault rifle, pistol, shotgun,
rocket launcher, sniper rifle and needler; the battery, three digits at
(26, 3), on the plasma rifle and plasma pistol (screen type 0). Rounds
loaded and the reserve in magazines are split-screen only.

Their scale field is 0. The number draw does not read it: its scale is
its eighth argument (E `0x000e1b3d`), 0 from the weapon HUD
(`0x000dc0d6`), and a scale not above 0 is 1.0 (`0x000e1b58`). So they
are not refused as scaled.

**Built.** H2's number formatter at `0x22a8c3` and `0x22a8e5` pads the
two- and three-digit counts, so 60 rounds in a three-cell field reads
`060`. For magazine counts H1 divides by the first magazine's capacity
(`0xdbf5f`, `0xdc0de`). The two sniper values require a valid target
with acquisition strength exactly 1. H1 measures observer-to-target
distance and signed vertical separation, multiplies by 3.048, and sets
both values to an invalid float when acquisition fails
(`0xdbd15`–`0xdbde6`). The converted readouts measure from the local
user’s sight point to the target’s world origin. The sight point follows
standing and crouching heights on foot, or the vehicle origin or active
seat marker while seated (H1 `0x3e480`, H2 `0xcafc0` and `0x1d8f50`).
Attached targets inherit their parent node transform (H1 `0x8ba10`,
H2 `0xba160`). The text uses the source digit and fractional-place counts
and the signed decimal spacing at `hud# +0x14` (6 pixels on all 23 maps).

### Statics of colour 0

Halo 1's element draw (E `0x000e1320`) sets blend mode 7 (draw struct
`+0x88`) and writes the element's colour into each vertex's diffuse
(`+0x0c`). Mode 7 in the tables `0x0007f4e0` indexes (`0x001e9c0c`
source, `0x001e9c30` destination, `0x001e9c54` operation; V: read from
the disc's XBE by `hud_layout_disc`) is ONE, ONE_MINUS_SRC_ALPHA, ADD:
premultiplied, `tex * colour + dst * (1 - tex.a * colour.a)` (the texture
times the diffuse is inferred from the vertex layout; the texture stages
were not read). So an element of colour `0x00000000` adds nothing and
takes nothing away: Halo 1 does not draw it. Each ballistic weapon on
Blood Gulch has one, `hud_ammo_alphas` on the rounds-loaded state (V:
assault rifle, pistol, shotgun, rocket launcher, sniper rifle, needler),
which v66's first cut drew opaque. `h1hud::static_alpha` gives such a
weapon static empty show words. Unit and grenade statics with that colour are omitted. A colour with alpha 0 but some RGB adds the
sprite's colour (a glow in Halo 1); Halo 2's `widget_gradient_flash`
blends by alpha only, so it is drawn at full alpha. Any other colour's
alpha goes into the sprite as for the unit HUD ("Colours" below): weapon
statics now get the same `<bitmap>_alpha_<aa>` copies (their panels'
`0xdc`).

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
(65, -1) and (12, -1); the `_hidden` pair draws through `dim_text_flash`.

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

Up to v65 the text's top was Halo 1's y (3), so the digits' ink was rows
8..20 under the indicator's top, which ends at row 17 (V: the `grhi`
background's texels, x 82..119, y 1..17): the count stuck out of the
bottom of the indicator, the bug seen at v60. Now the point's y is set
so the ink is centred on Halo 1's digit sprite: y -2, ink rows 3..15,
Halo 1's sprite rows 3..15 (`hud_layout_disc`
`grenade_count_digits_sit_inside_the_indicator`).

## Scale

**Built for weapon widgets.** H1 multiplies the draw basis by the two
authored floats at `+4` and `+8` (`0xe15f4`, `0xe15ff`). Placement
comes separately from `0xe07a0`. The converter writes independent
constant functions into the native X and Y effect slots. H2 applies
them at `0x22be12`–`0x22beaa`. In split screen H1 first halves the
crosshair basis unless overlay `+0x0c` bit 1 is set (`0xdae8f`).
The boot applies that factor before the native effect.

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
sensor background drew opaque in the shader's light blue (seen in a run
on a10 and a30); now it should be a faint disc at `0x28/255`.
Halo 1's colour RGB needs a `shad` per colour whose
constant reaches the pixel shader's last-stage `c0`; which `colors` entry
or animation binding feeds it is not yet established
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

### Shield over health (fixed in v66)

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
full, 849 texels showed both (297 on `cyborg_mp`): the shield drawn over
the health meter, the bug seen at v60.

The converter now gives each meter its own copy, `<bitmap>_meter`, with
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
  scales to the screen, a factor not read yet. Halo 1's range belongs in
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
- `H1_ISO=... H2_DISC=... cargo test -p h1objects --test sandbox_disc -- weapon_hud_ --ignored`
  checks retail held, off-hand and stowed placement and gating on
  Coagulation and Zanzibar. It also checks the weapon zoom fields and
  both widget types' hide flags in the freshly built Blood Gulch map.
- `cargo test -p h1objects --lib h1hud` covers all weapon meter sources,
  padding, magazine and target selectors, zoom and screen flags, dormant
  entries, placement and native scale and flash data.
- `cargo test -p boot --lib h1_hud_text::weapon` checks warning predicates,
  clock transitions, input transfer and the draw frame.
- `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1objects --test hud_layout_disc -- --ignored every_retail_weapon_hud_element_is_placed`
  compares the source entries, including child interfaces, with their
  actual rebuilt bitmap and text records on each map's base.

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
  HUD pairs (the biped's and the weapons') in the manifest; the motion
  sensor background's copy, decoded back from the built map, has a mean
  alpha inside its circle within 1 of `0x28/255` of Halo 1's; the
  grenade counts are two number texts on inputs 20 and 21 reading
  `hud_input1` through `text_flash`; the manifest's `hud_sensor` is the
  cyborg `unhi`'s sensor centre. `halo2s_motion_sensor_table` checks the
  XBE's table.
- `cargo test -p boot --lib hud`: also the sensor table both ways, and
  Halo 2's back on a map without a Halo 1 centre.
- `H1_ISO=... H2_DISC=... cargo test -p h1objects --test hud_layout_disc
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
- `H1_ISO=... cargo test -p h1objects --test hud_disc -- --ignored
  weapon_huds_read --nocapture`: prints every `wphi` chain of `$H1_MAPS`
  as read.

## What a game run should show

On a converted Halo 1 map (converter version 66 or later) in the Halo 1
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
draw it; v66's first cut did); the panel itself slightly translucent, as
Halo 1's (alpha `0xdc`). With the shield
full, only the shield bar's own shape is lit: nothing of it over the
health bar under it. The shield bar drains from the right as the shield
falls, the health bar likewise and stays down (Halo 1's health model). The
whole HUD hides in the pause menu, under Blind and in cinematics. Each
element should sit where Halo 1 puts it (Placement above); one that does
not points at a difference in how Halo 2 scales HUD pixels to the screen.
