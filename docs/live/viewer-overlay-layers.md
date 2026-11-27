# Viewer overlay: the overlay, name tags and reveal

### The overlay

`Overlay` (`web/src/viewer/Overlay.tsx`, no three.js) is a sibling of
`.v3-host` inside `.v3`, and is also the `Suspense` fallback in
`GameViewer`, so the same plate shows while the three.js chunk is fetched.
`overlayState` (`web/src/viewer/overlayState.ts`, pure) maps the load's facts to
one state:

| State | When | Veil | Controls |
|---|---|---|---|
| `boot` | the 3D chunk is being fetched, or the scene is not made yet | opaque | hidden |
| `loading` | `loadMap` is fetching the glb | opaque | hidden |
| `decoding` | every byte has arrived; the glb is being parsed | opaque | hidden |
| `reveal` | the level has been drawn once and the sweep is running | fading | sliding in |
| `ready` | the sweep has finished | gone | shown |
| `none` | the server has no geometry for this map (`glb()` is null once the art manifest has answered, or after 2500 ms) | gone | shown |
| `error` | there is a glb address and `loadMap` failed | gone | shown |
| `unsupported` | no WebGL, or the scene threw on creation | opaque | hidden |

`loadMap(url, onProgress)` resolves `"ok"` or `"failed"`, so `error` and
`none` are different states (a failed load and a map without geometry both
draw the grid floor, but only one of them offers **Retry**). While the
veil is opaque the scene does not draw (`ViewerScene.setCurtain(true)`
skips `draw`), so nothing is rendered behind it.

**What progress is measurable.**

| Stage | Measure | Bar |
|---|---|---|
| the 3D chunk | none | chase (indeterminate) |
| glb bytes | `ProgressEvent.loaded / total` from `loader.loadAsync(url, onProgress)`, when `lengthComputable` | segments 1 to 22 |
| glb bytes, no length | `loaded` only | chase, with the size read so far (`2.4 MB`) |
| parse and decode | none (one step) | 23 lit, the 23rd pulsing |
| first frame with the level | `h2-viewer:first-map-frame` | 24 lit, then the reveal |
| surface pictures | `streamTextures` calls back per picture: `done / total` | not on the bar: the status strip, after the reveal starts |
| sky, player models | `LevelLook.skyReady`, `ModelLibrary` | not shown |

`loadProgress` (`overlay.ts`) turns these into `{ lit: 0..24, text,
percent | null }`. `lit` never goes down within one load. The plate is
hidden for its first 150 ms (`animation-delay`), so a cached map never
flashes it.

**Tokens.** Declared on `.vo` in `web/src/viewer/overlay.css`.

| Token | Value | Use |
|---|---|---|
| `--vo-veil` | `#050d18` | the veil; the scene's background at the start of the reveal |
| `--vo-plate` | `rgba(6, 18, 36, .92)` | panel fill |
| `--vo-line` | `rgba(111, 182, 232, .55)` | keylines |
| `--vo-ink` | `#dfeeff` | text |
| `--vo-dim` | `#8fb0d0` | secondary text, load segments |
| `--vo-accent` | `#9fd8ff` | values, the highlight bar |
| `--vo-on` | `#17668a`, border `#9ad1ee` | a pressed control (as `.gv-buttons button.on`) |
| `--vo-warn` | `#ffc857` | notices |
| `--vo-bad` | `#f26122` | errors |

**Type.** Labels and values are `var(--font-tech)` (Michroma, already in
`web/src/styles/theme.css`), 9px/12px, uppercase, `letter-spacing: .14em`
by default. Titles are `var(--font-ui)` bold 20px/24px,
uppercase, `letter-spacing: .02em`. Body copy is `var(--font-ui)`
11px/15px.

Numbers use `font-variant-numeric: tabular-nums`.

**Plates.** A panel is two nested boxes with cut top-left and
bottom-right corners:

```css
.vo-plate { padding: 1px; background: var(--vo-line);
  clip-path: polygon(8px 0, 100% 0, 100% calc(100% - 8px), calc(100% - 8px) 100%, 0 100%, 0 8px); }
.vo-plate > div { background: var(--vo-plate);
  clip-path: polygon(7.5px 0, 100% 0, 100% calc(100% - 7.5px), calc(100% - 7.5px) 100%, 0 100%, 0 7.5px); }
```

**The veil.** `.vo-veil` fills the stage with `--vo-veil`. Over it, when
the art has the map's picture (the one `MapThumb` resolves,
`web/src/stats/Side.tsx`, passed in as `backdrop`), that picture at `object-fit:
cover; opacity: .28; filter: blur(6px) saturate(.7)`. Without one, a CSS
floor grid: `repeating-linear-gradient` lines of `rgba(111, 182, 232,
.16)` every 32px in both directions on a plane with `transform:
perspective(600px) rotateX(62deg) scale(2)`, anchored to the bottom half,
masked by `linear-gradient(transparent, #000 60%)`.

**Loading** (`boot`, `loading`, `decoding`). The plate is centred and up to
320px wide, with 10px by 12px padding and a subdued blue keyline. The
eyebrow and status use 10px/13px UI type with .05em tracking, and the
title uses 15px/19px semibold.

```
+--------------------------------------------------------------------+
| (veil: blurred map picture, or the CSS floor grid)                 |
|                                                                    |
|            ,----------------------------------------.              |
|           /  MAP UPLINK                             |              |
|           |  SLAYER ON LOCKOUT                      |              |
|           |                                         |              |
|           |  //////////////////////////////////     |              |
|           |  [#][#][#][#][#][#][#][#][#][ ][ ][ ]   |              |
|           |  LOADING MAP GEOMETRY              63%  |              |
|           '----------------------------------------/               |
|                                                                    |
+--------------------------------------------------------------------+
```

- Eyebrow `MAP UPLINK` in `--vo-dim`; the title is `GameViewer`'s
  `title`.
- The bar has 24 muted blue segments, 4px high with 2px gaps and a 12°
  skew. Inactive segments sit at .18 opacity. Lit segments reach .78
  opacity in 120 ms.
- When the download size is unknown, the chase runs over 1800 ms with
  75 ms between segments and a .6 opacity peak.
- During decoding the 23rd segment pulses between .42 and .78 opacity
  over 1100 ms.
- A faint scan line with a 12px tail crosses the plate once, over
  3200 ms.
- Status text, left: `LOADING VIEWER` (`boot`), `LOADING MAP GEOMETRY`
  (`loading`), `DECODING GEOMETRY` (`decoding`). Right: the percent, or the
  size read, or nothing.
- The bar is `role="progressbar"` with `aria-valuemin="0"`,
  `aria-valuemax="100"` and `aria-valuenow` (left off when
  indeterminate). The status text is `aria-live="polite"` and changes only
  with the state.
- The plate fades in over 180 ms with `cubic-bezier(0.2, 0.8, 0.2, 1)`.

**Ready** (`reveal`, `ready`). The veil fades `opacity 1 → 0` in 420 ms
`cubic-bezier(0.4, 0, 0.2, 1)` and is then `display: none`. The rail
enters 300 ms after the reveal starts (`translateY(-8px) → 0` with
opacity, 260 ms, the plate's curve); the status strip and the key hints
500 ms after.

```
+--------------------------------------------------------------------+
| [ORBIT|TOP|FLY] [FOLLOW: SPARTAN-117 v] [EYES]      [RESET] [VIEW] |
|                                                                    |
|                 [o Mendoza  BR]                                    |
|                 ======----                                         |
|                        *                  o   o                    |
|          (the level)        [o SPARTAN-117  SMG]                   |
|                             ==========                             |
|                                                                    |
| LOCKOUT · SURFACES 31/48          F FLY  V EYES  C CUT  M MENU     |
+--------------------------------------------------------------------+
```

- The **rail** (`.vo-rail`) has an 8px inset from the top and sides and
  4px gaps. `viewer.css` sets the buttons in regular 11px UI type with
  sentence-case labels, a translucent navy fill and muted blue keylines.
  They are 22px high on desktop and at least 36px by 32px under
  `max-width: 640px` or `pointer: coarse`. An active button has a deeper
  blue fill and a pale blue underline.
- **Stays visible:** the camera tabs **Orbit**, **Top**, **Fly** (one
  segmented group); the **Follow** chip, which shows the followed
  player's name and opens a picker list of the roster (no arrows, no
  "none" row: **Orbit** leaves follow); **Eyes**, shown only while
  following; **Reset**; **View**. With a `director` the tabs, chip and
  **Eyes** are the director's and are not drawn, as in `Viewer3D`.
- **Moves to the View menu:** ambient occlusion, see-through, height cut,
  textures, lifts and teleporters, damage, AI names, and the zone picker.
  **Level layers** contains cutscene zones, backdrop, decals, effects,
  sky, fog, weather, objects and their kinds. `Viewer3D` supplies the rows
  for the loaded level. The [combat-effects design](viewer-effects.md#the-effects)
  defines a separate **Combat effects** row.
- **Becomes a key hint:** `F` fly, `V` eyes, `C` cut, `X` see-through,
  `R` reset, `T` top-down, `M` menu, `H` hide the overlay. `R`, `T`, `M`
  and `H` are added to `Viewer3D`'s `onKey`; none is a replay key
  (`replayKey`, `web/src/viewer/Transport.tsx`). The hint row
  (`.vo-keys`, bottom right) shows four of them, is hidden under
  `pointer: coarse` and `max-width: 640px`, and drops to `opacity: .4`
  four seconds after the last key or pointer move over the viewer.
- **Status strip** (`.vo-status`, bottom left, `bottom: 8px; left: 8px`):
  one line in the label type on `rgba(0, 0, 0, .55)`, padding `2px 7px`.
  The map's name, then one of: `SURFACES <done>/<total>` while pictures
  stream; `FLYCAM <speed>`; `CUTSCENE`; nothing. Notices that are
  sentences (the stale-models note, the flycam's help) are a second line
  in body type, `--vo-warn` for the stale-models note.
- **Cut slider:** while the cut is on, a 140px range input sits at the
  bottom centre, `bottom: 8px`, `accent-color: var(--vo-accent)`.
- `H` adds `.vo--bare`: the rail, strip and hints are hidden and name
  tags stay.

**View menu** (`.vo-menu`). A plate 240px wide, right-aligned under
**View**, `max-height: calc(100% - 48px)`, scrolling inside.

```
                                     ,--------------------------.
                                    /  VIEW                     |
                                    |  Ambient occlusion     ON |
                                    |# See-through      IN FOCUS|
                                    |  Height cut           OFF |
                                    |  Textures              ON |
                                    |  Lifts & teleporters   ON |
                                    |  Damage              BARS |
                                    |  Level layers           > |
                                    '--------------------------/
```

- Rows are 24px (32px on touch): the name in body type on the left, the
  value in the label type and `--vo-accent` on the right.
- The row under the pointer or with focus has `background: rgba(47, 127,
  208, .35)` and a 2px `--vo-accent` bar on its left edge (`#` above).
- A two-value row toggles. A row with more values, and **Level layers**,
  opens a picker dialog in the same plate: a `‹ SEE-THROUGH` title row and one
  row per value, the current one marked. Escape or the title row goes
  back.
- Roles: `role="menu"`, rows `menuitemcheckbox` or `menuitemradio`. Up
  and Down move, Enter and Space act, Escape closes and returns focus to
  **View**. A click outside closes it.
- The menu opens with `opacity 0 → 1` and `translateY(-4px) → 0` in
  160 ms, the plate's curve.

**No geometry** (`none`). The grid floor is drawn, framed by the opening
camera rules. A notice plate sits above the status strip (`bottom: 34px;
left: 8px; max-width: 320px`) for 6 s, then collapses to the strip's
second line.

```
+--------------------------------------------------------------------+
| [ORBIT|TOP|FLY] [FOLLOW v]                          [RESET] [VIEW] |
|                                                                    |
|        .   .   .   .   o   .   .   .   .   .   .                   |
|      .   .   .   o   .   .   .   o   .   .   .   .   (grid floor)  |
|  ,-------------------------------------.                           |
| /  NO 3D GEOMETRY FOR THIS MAP         |                           |
| |  The floor grid is fitted to where   |                           |
| |  players have been.      [ 2D MAP ]  |                           |
| '-------------------------------------/                            |
| LOCKOUT                                                            |
+--------------------------------------------------------------------+
```

**Error** (`error`). The same plate, centred, keyline `--vo-bad`, with
`role="alert"`. The grid floor is drawn behind it so the game is still
watchable. **Retry** calls `loadMap` again with the same address; **2D
map** switches the viewer's mode.

```
+--------------------------------------------------------------------+
| [ORBIT|TOP|FLY] [FOLLOW v]                          [RESET] [VIEW] |
|                                                                    |
|            ,----------------------------------------.              |
|           /  UPLINK LOST                            |              |
|           |  The map's geometry didn't load.        |              |
|           |  The floor grid stands in for it.       |              |
|           |                                         |              |
|           |              [ RETRY ]   [ 2D MAP ]     |              |
|           '----------------------------------------/               |
|                                                                    |
| LOCKOUT                                                            |
+--------------------------------------------------------------------+
```

**Unsupported** (`unsupported`). The veil stays. The centred plate reads
`3D VIEW UNAVAILABLE`, then the sentence `Viewer3D` sets in `failed`, then
**2D map**. No rail.

**Reduced motion.** Under `prefers-reduced-motion: reduce` every `.vo`
transition and animation has `duration: 0.01ms`; the chase, pulse and
scan line are `animation: none` (the bar shows its lit count, or all
segments at `.5` when indeterminate); the veil cross-fades in 200 ms
linear.

### Name tags

A tag is the plate the scene already makes per player (`.v3-label`), with
an inner `.v3-tag` box that takes the declutter's offset (the
`CSS2DRenderer` owns the outer element's transform). `declutter`
(`web/src/viewer/declutter.ts`, pure) runs once per drawn frame, after the
positions are known, over every visible tag. Each plate has a fixed
maximum width in screen pixels. A tooltip shows the full text of a
truncated name or weapon.

```ts
interface TagIn { id: number; x: number; y: number; w: number; h: number; depth: number; rank: number }
type TagLod = "full" | "lifted" | "pip";
function declutter(tags: readonly TagIn[], prev: ReadonlyMap<number, TagState>, now: number): Map<number, TagState>
```

- `x, y` is the tag's bottom centre in stage pixels (the label's world
  point projected with the camera); `w, h` its measured size, cached and
  re-measured when its text changes.
- **Rank** (lower wins): 0 the followed player; 1 the player the
  scoreboard points at, the `?player=` player and the signed-in player; 2
  a player who fired, hit or was hit in the last 2 s; 3 every other
  player; 4 AI. Ties go to the nearer `depth`.
- **Placement**, in rank order: `full` when the tag's rectangle, grown by
  2px, meets no placed rectangle. Else `lifted`: moved up by `h + 3px`
  with a 1px leader line to its anchor, when that position is clear. Else
  `pip`: only the 7px colour dot, 16px hit area, never collision-tested.
- **Hysteresis.** A tag changes level only after the new level has been
  the answer for 250 ms; a `full` tag keeps its place while its overlap
  with a higher-ranked tag is under 4px.
- **Detail by distance.** The weapon name (`small`) shows at rank 0 and
  1, and at other ranks when the camera is within `25 × scale` units.
- The level is written as `data-lod` on the tag; `viewer.css` hides the
  text of a pip and offsets a lifted tag with `transform: translateY(calc(-1 * (100% + 3px)))`
  in 120 ms linear (none with reduced motion).
- At most 64 tags are laid out (2,016 rectangle tests at worst); AI past
  that are pips.

Damage bars and numbers are specified in [viewer-effects.md](viewer-effects.md#damage).

### Progressive reveal

The level arrives as a hologram of grid lines and becomes solid in a
sphere that grows from the opening camera's point of focus. It is a few
lines in the map materials' shaders, beside the see-through
(`web/src/viewer/reveal.ts` holds the timeline and the GLSL, as
`cutaway.ts` holds the see-through's), so it adds no draw call, no pass
and no geometry.

**Order.**

| Step | What | When (ms from the first frame with the level) |
|---|---|---|
| 1 | The veil fades, the level appears as grid lines, and player rings and tags appear | 0 to 300 |
| 2 | The solid sweep crosses the level | 200 to `200 + T` |
| 3 | Machines and placed objects (scenery, crates, weapons, vehicles): the same sweep | 380 to `380 + T` |
| 4 | Sky, fog and sun shadows come up | 25% to 85% of step 2 (sky, fog), 60% to 100% (shadows) |
| 5 | Player models and their stand-ins, vehicles, bodies and projectiles, each as the sweep passes it | within step 2 |
| 6 | Decals, effects layers, lift plasma, teleporter guides, weather | at the end of step 2 |
| 7 | Surface pictures | whenever each arrives (`streamPictures`), before or after |

`T = clamp(1100 + 9 × reach, 1300, 2400)` ms, where `reach` is the
distance in game units from the origin to the farthest corner of the level
box. The radius runs `0 → reach + band` with `1 - (1 - t)^3`. `band =
max(1.5, 0.06 × reach)`. The origin is the followed player's position for
a player opening, else the frame box's centre at its floor.

**Shader.** `patchReveal(vertex, fragment)` runs in `ViewerScene.cutaway`
after `patchSeeThrough` and reuses its `vSeeWorld` varying (adding the
varying itself for a material that does not take the see-through: vehicle
placements). Uniforms, one shared object for the level and one for
machines and objects:

```glsl
uniform vec4 uReveal;      // xyz origin (world), w radius; w < 0.0 is off
uniform float uRevealBand; // width of the edge, world units
uniform float uRevealCell; // grid cell, world units: the scene's marker scale
uniform float uRevealHolo; // 0..1, the grid lines' strength
const vec3 REVEAL_TINT = vec3(0.56, 0.83, 1.0);
```

After `#include <clipping_planes_fragment>` (derivatives are taken before
any branch):

```glsl
vec3 revealQ = vSeeWorld / uRevealCell;
vec3 revealW = max(fwidth(revealQ), vec3(1e-5));
vec3 revealN = normalise(cross(dFdx(vSeeWorld), dFdy(vSeeWorld)));
// A grid axis the surface is perpendicular to draws no lines on it.
vec3 revealG = abs(fract(revealQ - 0.5) - 0.5) / revealW + step(0.9, abs(revealN)) * 1e3;
float revealLine = 1.0 - clamp(min(min(revealG.x, revealG.y), revealG.z) - 0.5, 0.0, 1.0);
float revealK = 1.0;
if (uReveal.w >= 0.0) {
  revealK = 1.0 - smoothstep(uReveal.w - uRevealBand, uReveal.w, distance(vSeeWorld, uReveal.xyz));
  if (revealK <= 0.0 && revealLine * uRevealHolo < 0.5) discard;
}
```

Before `#include <dithering_fragment>`:

```glsl
if (uReveal.w >= 0.0) {
  vec3 revealHolo = REVEAL_TINT * (0.35 + 0.65 * revealLine) * uRevealHolo;
  float revealEdge = revealK * (1.0 - revealK) * 4.0;
  gl_FragColor.rgb = mix(revealHolo, gl_FragColor.rgb, revealK) + REVEAL_TINT * revealEdge * 0.6;
}
```

Outside the radius only the grid lines are drawn (the rest is discarded,
so the level is a see-through lattice); inside it the surface is its
shaded self; the band between glows. The program's cache key gains
`|reveal` on every map material from the start, so finishing the reveal
sets `uReveal.w = -1` and compiles nothing.

**The rest of the scene**, driven by `revealAt(ms, reach)` (`reveal.ts`,
pure), which returns `{ holo, radius, objectsRadius, sky, shadows, done }`:

- `LevelLook.setReveal(k)` (`level3d.ts`): the background colour runs
  from `--vo-veil` to the sky's clear colour; `scene.backgroundIntensity`
  and each sky material's `color` (`SkyDome`, `sky3d.ts`) are scaled by
  `k`; the haze density and the game fog's cap are scaled by `k`; decals,
  effects layers, plasma, weather and the spawner marks are hidden until
  `done`.
- `sun.shadow.intensity = shadows` brings shadows in during the last
  40% of the shell sweep. `CutawayMaterials.attachShadow` applies the
  sweep to the existing shadow pass, which only uses solid geometry
  inside the band.
- The players' rings and tags appear once the camera is framed, and the
  tags fade in (`opacity 0 → 1`, 160 ms linear). Player models and their
  life-size capsule stand-ins, vehicles, bodies and projectiles are
  hidden until their distance from the origin is under `radius - band`.
- The viewer's ambient occlusion reads the scene's rendered depth, so it
  follows the discard (`LevelAoPass`, `aopass.ts`).
- At `done` the scene marks `h2-viewer:revealed`.

**When it runs.** Once per map load. A zone change, a seek and a camera
change do not replay it. `SceneOptions.reveal` is false for the banner's
stills (`opts.bare`, `web/banner`) and under `?perf`
(`web/bench/viewer.bench.ts`): the level is solid in its first frame.

**Cost.** During the reveal: one distance, one `smoothstep` and three
`fwidth` per level fragment, and no early depth rejection on those
materials (the see-through's `discard` is already in the same shaders, so
this is not a new class of cost). After it: one uniform comparison per
fragment. CPU per frame: two uniform objects written and one distance per
player.

Memory: none (true wireframe geometry would add an index buffer
the size of the level's; the grid is computed in the shader instead).

**Reduced motion.** No lattice and no sweep: `uReveal.w` stays `-1`, the
scene is complete in its first frame, and the veil cross-fades over it in
200 ms linear. The camera is placed, not moved.

### Damage and combat effects

A hit reads on the victim's tag as bars and chips, at their feet as a
direction tick, and at their chest as a kill confirm. The **Damage** row
controls this display in 3D and on the tactical map. The [damage and
combat effects page](viewer-effects.md) specifies how the viewer draws
damage and describes the design for the shared combat-effects pool.
