# Halo 1 HUD: weapon HUDs

Weapon HUD conversion, split from [hud.md](hud.md).

| Section |
|---|
| [Weapon HUDs](#weapon-huds) |

Part of [hud.md](hud.md).

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
nothing in Halo 1 and becomes an `nhdt` with no widget. A failed element stops conversion with the map and source interface in the error.

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

The from-scratch map build reads every tag through the inferred schema,
which would type the thresholds word (`00 0b 01 00` at the widget's
`+0x18`) as a raw-resource pointer and write a copy's offset there; the
schema leaves the word out (`tags::schema::infer::ENGINE_DATA`,
docs/formats/tag-schema.md step 9), so the bytes reach the map as
written. A live gate test that writes the thresholds into memory with
`edit.set` after the build cannot see a wrong word in the built map. Run:
with the thresholds written back that way, "Reload" hides at 60 loaded
and shows at 5 loaded with 180 in reserve, and "No ammo" shows alone at
nothing loaded and nothing in reserve; every other weapon's HUD has its
panel, count, icon and meter where Halo 1 puts them
([notes/hud.md](notes/hud.md), "The reload warning's thresholds").

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
sprite placement. Their draw is not checked in a game run.

The converter keeps the zoom levels of Halo 1's pistol and sniper
rifle. The assault rifle and other unscoped weapons have
zero levels and suppress Halo 2's binocular fallback.
See [Weapon zoom](objects-build-weapons.md#weapon-zoom).

### Scope mask

The sniper rifle's scope lines and zoom labels come from the converted
crosshairs. Its surrounding mask is a separate screen effect. The
sniper rifle's `wphi +0xac` block references the full-screen and
split-screen scope mask bitmaps at
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
and the two colour strengths as the call's values; the native post
plans the blur from that radius
([native-post.md](../graphics/native-post.md#the-halo-1-scope)). The local flashlight button toggles
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
which would draw opaque. `h1hud::static_alpha` gives such a
weapon static empty show words. Unit and grenade statics with that colour are omitted. A colour with alpha 0 but some RGB adds the
sprite's colour (a glow in Halo 1); Halo 2's `widget_gradient_flash`
blends by alpha only, so it is drawn at full alpha. Any other colour's
alpha goes into the sprite as for the unit HUD ("Colours" below): weapon
statics get the same `<bitmap>_alpha_<aa>` copies (their panels'
`0xdc`).
