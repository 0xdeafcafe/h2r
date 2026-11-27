# Skulls: the model picture and gold and silver

Status: part of [skulls.md](skulls.md); see there for status, labels and the index. This page: the model picture and gold and silver skulls.

### The model picture

With the native renderer on, both Skulls screens draw the focused skull
as a small 3D picture in the picture box, as Halo 3's skull picker does,
from data on the user's own disc (`boot::skulls_model`, feature
`skulls-model`, on by default).

- **The model** is the retail campaign skull weapon,
  `objects\weapons\multiplayer\ball\head_sp` (`head` on High Charity),
  the tag our hidden skulls already create (`skulls_custom::SKULL_TAGS`,
  "Hidden collectables"). It is not in `mainmenu.map`, so the first ask
  starts a background load that opens the campaign maps in
  `skulls_model::gpu::MAPS` (Cairo Station first) and takes the first that
  has the tag as a `weap`. Its render model is read as the native objects
  read one (`native_render::model_load::load_object_model`: the `hlmt`'s
  default variant, highest detail level), with every opaque material's
  base map. Each part draws with its own material's map; a part whose
  material has none draws in the bone colour alone. Nothing is copied into the repository. The log says which map and
  tag it used, or why none loaded; until then, or after a failure, the
  page keeps the game's picture. To check the tag on a disc:
  `h2 tags 01b_spacestation | grep ball` lists it under `weap`.
- **The orientation.** The picture shows the skull upright and
  face-on, like a portrait: the dome at the top, the teeth at the bottom,
  both eye sockets and the nasal opening towards the camera. The model is
  authored well off every axis (it is a carried ball), so before framing
  the picture turns it (`ShowcaseMesh::oriented`,
  `showcase::Orientation`): `skulls_model::gpu::MODEL_FORWARD` (+y) to
  the camera and `MODEL_UP` (-z) up, then `MODEL_ROLL_DEGREES` (8.6)
  about the view axis, `MODEL_PITCH_DEGREES` (41.2, positive tips the
  face up) and `MODEL_YAW_DEGREES` (37.0, positive turns it to the
  picture's right). The angles come from the geometry. The skull's
  mirror plane, the plane a reflection in which lands the surface back on
  itself, gives its side-to-side axis, which is the picture's. The turn
  within that plane puts the teeth (the model's second part) at the
  bottom front and looks at the face from a little above the level of the
  teeth's row, so the whole forehead is in view for the glyph. The
  framing, sway and glyph placement all work on the turned model.
- **The look** (`native_render::showcase`, `showcase.wgsl`): the opaque
  parts at rest, in their own target with a transparent background,
  framed so the sphere round the model's farthest vertex fills 92% of the
  height (`Placement::fitted`). A warm key from
  above left, a cool fill from the right, a soft bounce from below, a
  little specular and a cool rim. Lighting is two-sided: a surface seen
  from behind its normal, such as the inside of the open cranium, is lit
  as its front. The base map's brightness is multiplied
  by the bone colour, gold (`GOLD_BONE`) or silver (`SILVER_BONE`) by the
  skull's tier. The model sways 20 degrees either way over nine seconds
  (`showcase::sway`), from the moment the page first shows it; a focus
  change keeps the sway going.
- **The glyph** is cut into the forehead: a decal projected along the
  model's forward axis (+x) onto the front of the upper head (69% up the
  bounds, 17% of the smaller of width and height in radius, only where
  the surface faces forward). The shader reads the glyph as a signed
  distance field and cuts a groove where the distance is negative: the
  edge is anti-aliased over one pixel of the picture, the wall slopes
  down over 0.05 glyph units and leans the surface normal towards the
  stroke's middle, so the wall under a stroke catches the key light and
  the wall over it falls into shadow, and the floor is darkened to 28%
  with no specular.
- **Locked**: a skull the profile has not collected is a silhouette. The
  model's shape is filled near black with a faint rim and no texture, and
  no glyph is drawn, so nothing about it is given away; the description
  stays its collection hint.
- **Where it goes**: the picture box (`PICTURE_BOX`, `[177 39 -30 259]`)
  in the UI's 640 by 480 units, the UI's height filling the target's and
  centred across (`skulls_model::guest_rect`), scaled into the target's
  texels. The pass runs for each copy of the menu's target, and the
  window's copy is the window's own pixels (`H2_RESOLUTION=native`), so
  the picture is drawn at the size it is seen: the box moved onto whole
  texels and the picture a whole number of times its size
  (`showcase::Fit`), twice each way while its longer side stays within
  1280 texels (averaged down two by two in the composite), otherwise
  texel for texel. Its target has four samples a pixel, resolved before
  the composite; the base maps are sampled with their mip chains,
  trilinear and anisotropic. The log says the picture's size once for
  each window size. It is multiplied by the picture widget's alpha
  (`+0x64`), so it fades with the page. The widget's slide offset is not
  read, so the model fades in place.
- **How it joins the page**: at each row text the list class notes the
  screen's picture widget, its block and the focused row's look
  (`Rows::model_picture`, `skulls_model::note`); the screen's destructor
  forgets it. The native UI walk asks its host at each sorted widget
  whether it draws the widget itself (`native_hud::Host::replace_widget`,
  [native-post-hud.md](../graphics/native-post-hud.md)). For the noted
  widget with the same block and a loaded model, the host queues the
  picture's pass at that point in the walk (`Device::external_pass_now`)
  and the game's bitmap draw is skipped. Any other widget, or no model,
  draws as the game does.

**Glyphs.** Our own line art, one per skull, a simple sign for the
effect; none copies a Halo game's skull symbols. They are vector data in
`boot::skull_glyphs` (lines, arcs and dots in a box from -1 to 1, a stroke
0.14 wide), sampled at run time into a 256 by 256 signed distance field
(`skull_glyphs::distance_field`). The field bounds only how fine a
stroke's shape can be; the groove's edge is resolved at the picture's
resolution.

| n | Skull | Glyph |
|---|---|---|
| 0 | Envy | a ring whose right half has faded to dots |
| 1 | Grunt Birthday Party | eight rays round a small ring |
| 2 | Assassins | a diamond with its corners missing, a point inside |
| 3 | Thunderstorm | two rank chevrons under a point |
| 4 | Famine | a ring filled only in its lower part |
| 5 | I Would Have Been Your Daddy | a speech ring with three dots and a tail |
| 6 | Blind | an eye struck through |
| 7 | Ghost | a plumb line hanging still |
| 8 | Black Eye | a shield's arc over a cross |
| 9 | Catch | a dashed throw and where it lands |
| 10 | Sputnik | a body, an orbit and its satellite |
| 11 | Iron | a loop that comes round to its start |
| 12 | Mythic | three nested rings |
| 13 | Angry | three chevrons pressing forward |
| 14 | Bandana | two rings joined |
| 15 | Acrophobia | an arrow rising on two wings |
| 16 | Low Gravity | a ball floating over the ground |
| 17 | Fall Damage | an arrow striking the ground and cracking it |
| 18 | Unlimited Ammo | three rounds standing on a line |
| 19 | Unlimited Grenades | a banded sphere with its pin |
| 20 | Invincible | a shield with a spine |

Tests (synthetic, no disc data): every skull has a glyph, no two alike,
each inside its box, and the distance field is negative inside a stroke
and measures out from it (`skull_glyphs::tests`); a locked look has no
glyph and the focus picks each skull's own (`skulls_model::tests`,
`skulls_menu::tests::the_model_picture_swaps_its_glyph_with_the_focus`);
the placement, the fitted framing, the picture's fit to its rectangle,
the sway and the uniform (`native_render::showcase::tests`); the half
floats the field is uploaded as and the base maps' levels
(`native_render::showcase_gpu::tests`); and on a headless GPU, an
invented sphere: the glyph darkens the forehead and no pixel outside it,
its edge in a 1024 pixel picture is two pixels wide from a 32 texel
field, another glyph marks it differently, the outline is anti-aliased
and premultiplied, a silhouette is dark with no glyph, the tint colours
the bone, each part takes its own material's base map (white, so the
tint alone, without one), a surface seen from behind its normal is lit
as its front, and the composite stays in its rectangle
(`crates/rewrites/native-render/tests/showcase_gpu.rs`). The 24
orientations are proper rotations that put forward on +x and up on +z,
and the fine-tune angles survive a round trip
(`native_render::showcase::tests`).

**Preview from a disc**, without the game
(`crates/extensions/game/src/skulls_model/gpu/preview.rs`, ignored by
default): `H2_DISC=<iso> H2_SKULLS_PREVIEW_DIR=<dir> cargo test -p boot
--lib skulls_model::gpu::preview -- --ignored --nocapture` loads the skull
as the game does and writes to that directory only: a locked Iron, a
collected Iron (gold) and a collected Envy (silver), each as the page
shows it in a 640 by 480, a 1280 by 960 and a 3840 by 2160 window, drawn
and composited as the game's pass does (`<state>-480.png`, `-960.png`,
`-2160.png`); the sway's two ends (`sway-left.png`, `sway-right.png`);
the model turned 0, 90, 180 and 270 degrees with Iron's glyph
(`facing-yaw-*.png`); the model as authored under each of the 24
axis-aligned orientations (`orientations.png`); yaw by pitch and roll 30
degrees either side of the orientation in use, in steps of 10, the one
in use in the middle (`orientations-fine.png`, `orientations-roll.png`);
the orientation in use beside the candidates in the preview's
`CANDIDATES` and in `H2_SKULL_CANDIDATES` (`yaw/pitch/roll,...`), each
with Iron's glyph (`candidates.png`); every glyph with its name
(`glyphs.png`); and the model's vertices and triangles as authored
(`mesh.txt`) with its base maps (`base-<material>.png`), for working an
orientation out from the geometry. It measures the turned skull
(`preview::Facing`): the mean distance from each vertex mirrored left to
right to the nearest vertex, and where the teeth sit. The orientation in
use must be upright and face-on by that measure (mirror error under 4%
of the width, the teeth in the middle across, at the front, near the
bottom) or the preview fails; a candidate that is not is left off the
sheet; none of the 24 axis-aligned orientations is. `H2_SKULL_YAW`,
`H2_SKULL_PITCH` and `H2_SKULL_ROLL` (degrees) override the fine-tune
constants for the preview's pictures only; the game reads only the
constants. It prints the map and tag, each
part's material and base map, the bounds as authored and turned, the
orientation in use and each sheet's index-to-label list. The directory
must be outside the repository: the pictures are made from disc data.

## Gold and silver

Halo 2 itself does not split its skulls into kinds: they are fifteen bytes,
all alike, and the game says nothing about them beyond a name
([Halopedia, Skull](https://www.halopedia.org/Skull): the gold/silver split
starts with Halo 3). The split we use is the one the Master Chief
Collection gives Halo 2's skulls, read off its score multipliers
([Halopedia, MCC:Skulls](https://www.halopedia.org/MCC:Skulls)): a skull
that multiplies the campaign score above 1.0 makes the game harder and is
**gold**; one at 1.0 or below is for fun and is **silver**.

| n | Skull | MCC multiplier | Tier |
|---|---|---|---|
| 2 | Assassins | 1.2 | Gold |
| 3 | Thunderstorm | 1.2 | Gold |
| 4 | Famine | 1.1 | Gold |
| 6 | Blind | 1.1 | Gold |
| 8 | Black Eye | 1.1 | Gold |
| 9 | Catch | 1.1 | Gold |
| 11 | Iron | 1.3 | Gold |
| 12 | Mythic | 1.2 | Gold |
| 13 | Angry | 1.1 | Gold |
| 0 | Envy | 0.0 | Silver |
| 1 | Grunt Birthday Party | 1.0 | Silver |
| 5 | IWHBYD | 1.0 | Silver |
| 7 | Ghost | 1.0 | Silver |
| 10 | Sputnik | 1.0 | Silver |
| 14 | Bandana | 0.0 | Silver |

Uncertain, so said here:

- This is MCC's grouping applied back to Halo 2, not anything in the
  Xbox game. A rougher "is it a novelty?" rule disagrees on two: it would
  call Blind silver (it only hides the HUD) and Envy gold. MCC scores
  Blind 1.1 (gold) and Envy 0.0 (silver: active camo helps the player).
  We follow the multipliers.
- The multipliers are from Halopedia's MCC page as read for this work
  (November 2026); MCC patches have changed scoring before.

The tier is `skulls::About::tier` (`SkullTier`), next to the rest of
`ABOUT`. The Skulls page lists gold first, then silver, each in the
engine's order (`skulls::by_tier`; `skulls_menu::row_of` maps an index to
its row), and each description starts "Gold skull:" or "Silver skull:".
Coloured rows are not done: our list class sets each row's text only,
and the rows draw in the skin's colour, so the tier is in words and in
the picture's tint. The web badges
and the games filter group by the same tiers (`web/src/lib/skulls.ts`,
kept in step with `ABOUT` by hand).

`ABOUT` and every index are open-ended: the screen's row count is
`ABOUT.len()`, the wire takes indices up to 63 (`live_proto::skulls::LIMIT`),
and the web shows "Skull n" for an index it has no name for. Fifteen
(`COUNT`) is only the engine's array; a skull of ours past it needs its own
storage, not a byte of `0x4f55dc`.
