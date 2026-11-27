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

**Tokens** are declared on `.vo` in `web/src/viewer/overlay.css` (`--vo-veil` `#050d18`, `--vo-plate`, `--vo-line`, `--vo-ink` `#dfeeff`, `--vo-dim`, `--vo-accent` `#9fd8ff`, `--vo-on` `#17668a`, `--vo-warn` `#ffc857`, `--vo-bad` `#f26122`).
**Type and plates.** Labels and values use `var(--font-tech)` (Michroma, in `web/src/styles/theme.css`), titles and body `var(--font-ui)`. A panel is two nested boxes with cut top-left and bottom-right corners (`.vo-plate`, `clip-path`). The veil (`.vo-veil`) is `--vo-veil` with the map's `MapThumb` picture blurred over it, or a CSS floor grid when there is none.

**Loading** (`boot`, `loading`, `decoding`). A centred plate, up to 320px wide, over the veil.

- Eyebrow `MAP UPLINK`; the title is `GameViewer`'s `title`. The bar has 24 segments. When the download size is unknown a chase runs along it; during decoding the 23rd segment pulses.
- Status text, left: `LOADING VIEWER` (`boot`), `LOADING MAP GEOMETRY` (`loading`), `DECODING GEOMETRY` (`decoding`). Right: the percent, the size read, or nothing.
- The bar is `role="progressbar"`; the status text is `aria-live="polite"` and changes only with the state.
**Ready** (`reveal`, `ready`). The veil fades `opacity 1 → 0` in 420 ms
`cubic-bezier(0.4, 0, 0.2, 1)` and is then `display: none`. The rail
enters 300 ms after the reveal starts (`translateY(-8px) → 0` with
opacity, 260 ms, the plate's curve); the status strip and the key hints
500 ms after.


- The **rail** (`.vo-rail`) holds the buttons in regular 11px UI type; they are at least 36px by 32px under `max-width: 640px` or `pointer: coarse`.
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
- **Becomes a key hint:** `F` fly, `V` eyes, `C` cut, `X` see-through, `R` reset, `T` top-down, `M` menu, `H` hide the overlay. `R`, `T`, `M` and `H` are added to `Viewer3D`'s `onKey`; none is a replay key (`replayKey`, `web/src/viewer/Transport.tsx`). The hint row (`.vo-keys`) is hidden under `pointer: coarse` and `max-width: 640px`, and fades four seconds after the last key or pointer move.
- **Status strip** (`.vo-status`, bottom left): the map's name, then one of `SURFACES <done>/<total>` while pictures stream, `FLYCAM <speed>`, `CUTSCENE` or nothing. Sentence notices (the stale-models note, the flycam's help) are a second line.
- **Cut slider:** while the cut is on, a range input sits at the bottom centre.
- `H` adds `.vo--bare`: the rail, strip and hints are hidden and name tags stay.

**View menu** (`.vo-menu`). A plate 240px wide, right-aligned under
**View**, `max-height: calc(100% - 48px)`, scrolling inside.


- Rows are 24px (32px on touch). A two-value row toggles; a row with more values, and **Level layers**, opens a picker dialog in the same plate (a `‹ SEE-THROUGH` title row, one row per value, the current one marked). Escape or the title row goes back.
- Roles: `role="menu"`, rows `menuitemcheckbox` or `menuitemradio`. Up and Down move, Enter and Space act, Escape closes and returns focus to **View**.

**No geometry** (`none`). The grid floor is drawn, framed by the opening camera rules, with a notice plate ("NO 3D GEOMETRY FOR THIS MAP", **2D map**) above the status strip for 6 s.

**Error** (`error`). The same plate with keyline `--vo-bad` and `role="alert"`, over the grid floor, with **Retry** (calls `loadMap` again) and **2D map**.

**Unsupported** (`unsupported`). The veil stays; the plate reads `3D VIEW UNAVAILABLE`, then the sentence `Viewer3D` sets in `failed`, then **2D map**. No rail. Under `prefers-reduced-motion: reduce` all overlay transitions and animations are off and the veil cross-fades in 200 ms.

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

**Shader.** `patchReveal(vertex, fragment)` runs in `ViewerScene.cutaway` after `patchSeeThrough` and reuses its `vSeeWorld` varying. Two uniform objects (one for the level, one for machines and objects) hold the origin and radius (`uReveal`, `w < 0` is off), the band width, the grid cell (the scene's marker scale) and the grid lines' strength. Outside the radius only grid lines are drawn (the rest is discarded, so the level is a see-through lattice); inside it the surface is shaded as usual; the band between glows. Every map material carries `|reveal` in its program key from the start, so finishing the reveal sets `uReveal.w = -1` and compiles nothing.

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

**Cost.** During the reveal each level fragment does one distance, one `smoothstep` and three `fwidth`, with no early depth rejection (the see-through's `discard` is already in the same shaders). The grid is computed in the shader, so no wireframe index buffer is built. Under reduced motion there is no lattice and no sweep: `uReveal.w` stays `-1`, and the veil cross-fades over the finished scene in 200 ms.

### Damage and combat effects

A hit reads on the victim's tag as bars and chips, at their feet as a
direction tick, and at their chest as a kill confirm. The **Damage** row
controls this display in 3D and on the tactical map. The [damage and
combat effects page](viewer-effects.md) specifies how the viewer draws
damage and describes the design for the shared combat-effects pool.
