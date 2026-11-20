# The web app's game art and 3D viewer

Status: Built. The optional game art the app loads from a server, and the game page's 2D map and three.js 3D view.
Code: `web/src/state/gameart.ts`, `web/src/lib/gameart.ts`, `web/src/map/`, `web/src/viewer/`. Tests: `web/test/`.
Switch: `H2_MOCK_GAME_ART`, `H2_MOCK_MODELS`, `H2_MOCK_INDOOR` and related mock knobs, listed in [web-app.md](web-app.md#knobs) (web-only, not in [switches.md](../runtime/switches.md)).

How the web app ([web-app.md](web-app.md)) uses the pictures, map geometry and models that a server's owner extracts from their own disc, and how the game page's viewer draws a game from them. None of it is shipped: without it the app uses its own art pack.

### Game art (optional, never shipped)

A server whose owner ran
`h2 live assets extract` on their own disc serves that art at
`/art/game/manifest.json` (docs/live/live-api.md "Optional game art"), by default
only to a browser on the same machine. The app asks once at startup and on a
server change (`src/state/gameart.ts`); a 404 or anything unreadable means
"use the art pack", and a miss is remembered for a minute. With a manifest,
any picture whose key it has replaces the pack's (`src/lib/gameart.ts`):

- **Emblems** load from `/art/emblem/*.png` as soon as the image is
  rendered, without waiting for the manifest or a browser canvas. Both
  servers compose the two mask layers using the extraction's palette.
  A recorded player's layers and colours take precedence, and seed-derived
  choices fill in any missing fields. The art pack symbol appears if the
  server cannot supply the image. With `H2_MOCK_GAME_ART=1`, the mock
  composes its synthetic masks too. To use a real extraction with a mock
  event feed, point `H2_GAME_ART_URL` at the Live server serving that
  extraction.
- **Stats banners** (`banner/home`, `banner/strip`): collages of in-game
  views of the owner's maps, drawn by `bun run banner` (`banner/`: the shot
  list, layout and grade in `plan.ts` and `page.ts`; docs/live/live-art.md
  "Stats banners"). `Strip` shows them; without, the pack's header art.
- **Ranks**: our experience ladder is spread over the game's levels
  (`levelForExp`, then `rank/<1..50>`).
- **Map previews** (`map/<map>/preview`) replace the top-down thumbnail in
  game cards, Recent Games, history rows and preview, and the game page's
  header; a live game keeps its blips, drawn over the picture.
- **Game-type icons** go by the words of the variant name (`gametypeKey`:
  slayer, ctf, assault, oddball, king of the hill, juggernaut, territories).
- **Medals** (`medal/<alias>`): the derived medals that mean the same as
  one of the game's use its picture (`medalKey`: twofer, hat-trick,
  full-house, encore are the multi-kills; hot-streak, wildfire, untouchable
  the sprees). Opening Act, Last Word and Payback have no counterpart and
  keep the pack's.
- **Weapons** (`weapon/<alias>`, by the words of the reported name,
  `weaponKey`) are single masks, used as a CSS mask so they take the page's
  tint like the SVG silhouettes, which every other weapon keeps.

The mock's stand-in (`mock/gameart.ts`) uses the pack's rank badges and
medals and draws its own emblem masks, weapon masks, game-type glyphs and
map postcards in the real encoding, so nothing from the game is needed to
run or test this path. To look at a real server's art over the mock's
world, point only that route at it: `H2_GAME_ART_URL=http://127.0.0.1:8074`.

### Map geometry and the 3D view (optional, never shipped)

The same
extraction can write, per map, a `map.glb`, a `views.json` and pre-drawn
pictures of the level (top-down and four iso corners), named in the art
manifest's `maps` table. When a map has them, the game page's Tactical Map
lays players over the picture using the view's `world_to_pixel` matrix
(`src/map/views.ts`, `src/map/ViewMap.tsx`), with a switcher for the views
and pan and zoom; without them it is the fitted grid as before. The game
page's viewer (`src/viewer/GameViewer.tsx`) sits under the match hero,
with the scoreboard, field report and overview below. The Field Report
keeps visible rows keyed by kill event as the replay advances. A new row
can enter once, with motion disabled when the browser requests reduced
motion. The viewer switches between the tactical map and the 3D view
(`src/viewer/`). The 3D view loads three.js when selected and draws a sun,
soft shadows and ambient occlusion that switches off if the device can't
keep up. It offers orbit, top-down,
follow-a-player and free-fly cameras (F; WASD, Q/E down and up, drag with
either button to look or click to lock the mouse, wheel or trackpad for
speed, Shift to go faster, one finger to look and two to fly on a touch
screen, or a gamepad, `viewer/flycam.ts`). A grid floor appears when the
map has no geometry. The [map uplink](viewer-overlay.md#the-overlay)
shows geometry load progress, offers **Retry** after a failed load and
puts level settings in the 3D view's **View** panel. [Name tags](viewer-overlay.md#name-tags)
give priority to the followed player and reduce crowded labels to pips. The [opening camera](viewer-overlay.md#opening-camera)
follows the player selected by `?player=`, the signed-in player or the
embedding page when present, or frames the playable area. The
**Full screen** button puts the viewer's box full screen with the Fullscreen
API (Escape leaves), or lays it over the window where there is none (an
iPhone; Escape leaves that too); the canvas and the 2D map refit either way
(`viewer/fullscreen.ts`). The viewer's keys (the replay's Space, arrows,
`,` `.` `<` `>` `k`, and the 3D view's) are its own only while it has the
focus or the pointer is over it, so the page keeps them otherwise. The 2D
map, the 3D view and the scrubber share one session (`src/viewer/session.ts`).
Finished games autoplay without audio from the first recorded frame.
**Watch in Theater**, links from the film library, and legacy
`/theater/:id` links open Game Details at `#game-viewer`. The same viewer
offers playback, camera controls, the HUD and clip creation. A shared
`/theater/:id/clip/:clip` link opens the requested range on a loop.
Use `?t=12.5` to start playing 12.5 seconds into a full recording. Playback
controls used in one replay tab also apply to other open tabs on that game.
Live games open at the live edge. Scrub back through up to 450 recent
frames (about 30 seconds at 15 frames per second), or select **Live** to
return to the edge. The history fills as the tab receives frames. When the
match ends, the recording takes over at the current playhead with the same
play or pause state. `?view=3d` opens the 3D view (with `cam`, `cut`, `see`
and `ao` as below) and `?map=iso-ne` picks a view, so both can be linked
to; the old pop-out address, `/games/halo2/games/:id/3d`, goes to the game
page with the 3D view showing. The mock builds
two arenas of its own from its invented floor plans (`mock/arena.ts`:
slabs, ramps, pillars and cone trees, with a small rasteriser for the
pictures); its third map has none, so the fallbacks run too.

### Vehicles and AI

A frame may list `vehicles` (those with someone in
them: kind, position, forward and up) and `actors` (living AI-controlled
bipeds, campaign), both optional. A vehicle is drawn as its glb from the
manifest's `models.vehicles`, turned by its forward and up, or as a box of
about its kind's size; a player's vehicle takes a light wash of their
colour. Whoever is in one is not drawn themselves: the vehicle stands for
them and carries their label (seat nodes are not used yet). A turret
(a vehicle with a `parent`: the Warthog's chaingun, the Scorpion's
cannon) is not drawn on its own: its vehicle's model turns its
`turret_<n>` part and raises its guns to the turret's `aim`, else its
gunner's, eased (`viewer/turrets.ts`, `viewer/turrets3d.ts`), and its
gunner rides the vehicle. Actors are
sampled, eased and drawn through the same path as players (an index made
from their id, negative so it never meets a player's): their species' glb
in its own colours, else the spartan's untinted, else a small capsule, with
a ring in their side's colour (covenant purple, human green, flood tan,
sentinel grey). They have no name label unless "AI names" is on (the button
shows when a game has actors) and no aim ray; their shots draw like
players'. At most 128 are drawn from a frame. On the 2D maps actors are
small squares and vehicles an outline under their riders.

### Seeing into a level

A level's structure is single-sided and faces
into the playable space, so the 3D view culls its back faces whatever the
glb says and it can be looked into from outside; meshes under a `machines`
node are ordinary models and keep the side they came with. Every mesh is
found by walking the glb, so one `structure` node or many `bsp_<n>` ones
are the same to it. Two more ways in (`viewer/cutaway.ts`): **Cut** (key
`c`, `[` and `]` or the slider) hides the level above a height. While following
a player it keeps to just above their head. **See-through** (key `x` to
cycle through off, *in focus* and *everyone*) opens a window around each
target's displayed body. *In focus* uses the followed or highlighted
player, or the nearest players when no one is selected. *Everyone* uses
up to eight targets.

The same shader tests each pixel ray against the player's body volume.
Fragments in front of the body that block the window are discarded.
BSP sections and clusters, instanced scenery, and doors, lifts and
platforms take the same cut, including machines drawn from frames and
scenery drawn from the film. The cut leaves the player's supporting floor
and geometry outside the window visible. Bipeds and vehicles draw whole.
The cut applies to both colour and depth, so the remaining fragments
keep their normal depth ordering. Ambient occlusion uses that same depth,
and shadows stay in place. `?cut=<height>` and `?see=0` set the initial
view from a link. (Culling front faces instead was
tried: Halo's levels are thick-walled and faced inwards, so it shows the
inside of the near walls and ceilings rather than what is beyond them.)

### Players, aim and shots

Frames may say each player's `species`, `aim`
(a unit direction), held `weapon` and `shots` fired since their last frame;
all four are optional and anything off the contract is dropped
(`state/frames.ts`). When the art manifest has a `models` table
(`viewer/models.ts`), the 3D view draws each player as their species' glb
(a spartan for a species it has no model of) with the held weapon's glb
under its `weapon` node, in the roster's colour: the glb's `COLOR_0` gives
each vertex a primary weight (R, the player's colour) and a secondary one
(G, a darker shade), and a model without it is tinted all over. Each glb is
fetched once and cloned per player; team mates share the coloured mesh
(`viewer/players3d.ts`). It is life size up close and grows as the camera
backs away, up to the size capsules are drawn at. Without models (the mock
by default, an older extraction) players stay capsules. Either way a faint
ray from the head shows the aim (level along the yaw when the frame has
none), the label names the weapon, and each frame's shots draw a muzzle
flash and up to three tracers along the aim that fade in 150 ms
(`map/shots.ts`), from the weapon's `muzzle` node or from chest height. The
2D maps draw a short aim line from each dot and a fading line for each
shot. With reduced motion nothing flashes: the aim ray turns bright while a
player is firing instead. Recorded frames are one a second, so a replay
shows a second's shots as one burst.

### Animation, actions and damage (optional)

Frames may also say how each
player and AI biped is moving (`stance`: stand, crouch, airborne, vehicle,
dead; `speed`), what they just did (`actions`: grenade, melee, reload,
jump, switch, pickup; `grenade`: the type), who hit whom (`hits`: attacker
and victim as `p<index>` or an actor or vehicle id, the victim's position,
an amount and a kind), every vehicle in the world with whether anyone is in
it (`occupied`), and which zone of a campaign level is loaded (`bsp`). All
optional; a frame without them draws as before.

- *Clips.* A models table of version 2 has skinned bipeds whose glb carries
  clips named `<weapon_class>:<state>`, and gives each weapon stem its
  class (clips every class shares are `any:<state>`). `viewer/anim.ts`
  (pure) picks the looping state from the stance and speed (idle, walk,
  run, crouch, airborne, die, dead; a sender without speeds has them
  worked out from how far the player moved) and lays one-shots over it
  (fire on a frame's shots, grenade, melee, reload, switch from its
  actions, land or land_hard on coming down, hard after a drop of more
  than 1.2 units below the jump's peak); the clip is the held weapon's
  class's, else the rifle's, the unarmed, or any class that has the
  state, a missing loop falls back to a nearer one and a missing one-shot
  isn't played. The game's jump has no take-off clip: `airborne` is the
  whole leap (tuck, then hang) and plays once from leaving the ground,
  held; dying plays once (`die`) and holds its last frame; a body first
  seen already dead lies in `dead`.
  `BipedAnimator` (`viewer/players3d.ts`) plays them on a three.js mixer
  per biped: loops cross-fade, walk and run are paced to the ground speed,
  and a one-shot outweighs the loop only on the bones it moves. An AI's
  model faces where it aims and its spine takes a third of the aim's turn
  (at most 0.3 rad) and a pitch of at most 0.6 rad: turning a grunt's or
  hunter's whole spine to the aim swung its tank or back plates about. A
  version 1 (static) model glides in its pose as before, a capsule squats
  when crouched, and reduced motion leaves every model in its pose.
- *Bodies* (`viewer/bodies.ts`). A player who dies leaves their model
  where they fell (where the frames put the dead, which is their body),
  playing the death clip and holding it; respawning makes a new model and the body stays
  for a minute, at most 24 of them, all cleared on a seek. AI bodies come
  in the frames as actors with the `dead` stance and lie until the game
  clears them away.
- *Damage* (`map/events.ts`). A hit draws a line from attacker to victim
  that fades in a quarter of a second and a marker that shows damage as
  a percent of the victim's vitality. Sustained fire on a victim adds to
  the same marker. A finishing hit gets a larger red ring that stays
  longer. With reduced motion, the numbers stay put and the line holds.
- *Projectile trails* (`state/projectile-trails.ts`). The 2D and 3D viewers
  use consecutive `Frame.projectiles` positions for each projectile ID. The
  projectile's recorded type sets its colour. Interpolation stays between
  observations for the same projectile and parent, so a trail can follow a
  recorded bounce. A disappearance, cut or seek clears the trail. A
  grenade action still animates the throw. Reduced motion hides the trails.
  Recordings without projectile samples still show the throw animation,
  muzzle flashes and fast shot effects. Host-film objects use their own
  playback path.
- *Vehicles.* All of them are drawn; one nobody is in is darker (3D) or a
  faint grey outline (2D) and has no label. Once the frames carry
  vehicles the level's own copies (`vehi` placements and multiplayer
  vehicle spawns) are hidden, so a vehicle isn't drawn twice
  (`viewer/objects3d.ts`).
- *Zones* (`viewer/zones.ts`). When a level's glb has `bsp_<n>` nodes, the
  3D view shows only the zone in play (`ZoneFollower`): the one the frames
  name when the players are in it (by each zone's bounding box, a unit of
  margin), else the one the followed player, or most players, stand in,
  the zone already shown winning ties where zones overlap. A cutscene's
  zone isn't play. A new zone has to be wanted for 0.8 s before the view
  moves to it (at once on a seek), and the camera, unless following
  someone, moves to frame it. With nothing to go on it shows every zone
  not marked cinematic-only in its node's extras, and never none at all.
  A `backdrop` node (far scenery) is hidden. When the level has several,
  the **View** panel's zone picker offers "Zone in play", "All zones", and each zone by its
  BSP's name (`extras.name`; cutscene zones too when "Cutscene zones" is
  ticked); its **Level layers** picker switches the backdrop, cutscene zones, decals,
  effects, sky and fog. The camera and the lights fit the playable zones
  only (`viewer/level3d.ts`).
- *Mods* (`map/views.ts` `mapFilesFor`, `map/geometry.ts`). A game on a
  map package (`map_id`) draws the mod's own entry of the art when there
  is one (`mod-<id>`, carrying the `map_id`): its level, objects and
  pictures extracted from the patched map. Without one, the base map's,
  unless the package changed the geometry (docs/live/live-art.md, "Mods").
  Effects the level's scripts play (`asset.extras.spawners`) are diamonds
  that burst at each play, shown with the effects (`viewer/spawners.ts`).
- *Layers, sky and fog* (`viewer/layers3d.ts`, `viewer/sky3d.ts`). A zone's
  `decals_<n>` and `effects_<n>` meshes are drawn with their pictures,
  after the level and without depth: decals pulled just in front of their
  wall and multiplied, doubled or alpha-blended as their material's
  `extras.blend` says, glows and light shafts added, unlit and unfogged
  (docs/live/live-art.md, "Layers"). The level's `sky.glb` is drawn around the
  camera before everything else, moving with it but not turning, its
  additive layers left out; behind it is the sky's clear colour, else its
  fog colour. The sky's atmospheric fog is drawn as the game's, not
  three's: clear up to its start past the point in focus, thickening
  linearly to the sky's maximum density at its opaque distance and never
  thicker beyond, so an overview fades the far side instead of hiding it.
  `useGameFog` patches three's fog chunks for that (the cap is one shared
  uniform every material's `onBeforeCompile` gets; a material that sets
  its own must chain the prototype's). A sky without fog but with a
  sun-lit glow keeps the exponential haze in the glow's colour. With no sky the
  view keeps its own gradient and haze. The occlusion pass leaves out
  anything see-through that writes no depth (`viewer/aopass.ts`).

### Machines, teleporters and lifts (optional)

Frames may carry `machines`:
lift platforms, doors and other big scripted movers, each with the name of
the map glb's `machines` child it is (`node`), its place and facing.
`machine_positions` carries the normalized animation position by scenario
placement index, as a byte sent on change. Live and replay state retain
quiet devices and sample between frames at the same render tick as the
players. The 3D view uses the map glb's device clip to pose its moving
parts and blends across the seam for looping devices. Older recordings
and map art without device clips draw with world poses as before.

The 3D view moves a named machine node (`viewer/machines3d.ts`): one with
a transform of its own is put where
the frame says; one whose mesh is already in world space is moved by how
far the machine is from where it was first seen. A machine the glb has no
node for is drawn from `models.objects.<kind>` when the server has that
glb, else as a box. A map glb's `asset.extras.features` lists teleporters
(with `links` between them) and gravity lifts (`map/features.ts`): the 3D
view stands a ring at each teleporter in its role's colour, arcs each link
through the air with an arrowhead where it arrives, and draws each lift as
a translucent column with an arrow along its push
(`viewer/features3d.ts`); the 2D maps draw rings, dashed curves and a disc
with chevrons, reading the extras from the head of the glb (a streamed read
that stops after the JSON, never the meshes). Both have a "Lifts &
teleporters" button when the map has any.

The lift's rendered glow is part of the map art. Ascension and Midship
carry it on their placed objects, Lockout in the level geometry, and
Ivory Tower (`cyclotron`) on its air lifts. The extractor reads their
`plasma_alpha` shader's layer-15 pass and exports two noise maps, the beam
mask, tint constants and the time-driven UV offsets. The viewer streams
those pictures alongside the map textures and combines them in one
unlit additive pass. It tests against the solid scene's depth and writes
no depth.
The emitter's solid parts still use their ordinary materials. The
**Effects** switch controls these glows, and reduced motion holds the
texture at the start of its cycle. **Lifts & teleporters** controls the
guide separately. The plasma pass approximates the game's shading with
its mask, tint and scrolling textures. Attached particles still require
effect playback.

To refresh an existing art directory with the updated CLI, run

```sh
h2 live assets extract --disc disc/halo2.iso --out "/path/to/game-art"
```

Use the same `--mappack` and `--installed` inputs as for your previous export.
If the viewer uses published art, republish the refreshed directory.

## Related

- [web-app.md](web-app.md): the app, its pages and the mock server.
- [live-art.md](live-art.md): extracting the game art.
- [live-api.md](live-api.md): "Optional game art".
