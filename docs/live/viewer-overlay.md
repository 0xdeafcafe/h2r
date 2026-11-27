# The 3D view's opening camera, overlay and reveal

Status: Built. The opening camera, loading overlay, name-tag layout and progressive reveal are part of the 3D viewer.
Code: `web/src/viewer/Viewer3D.tsx` (the 3D view), `web/src/viewer/scene.ts` (`ViewerScene`), `web/src/viewer/camera.ts`, `web/src/viewer/GameViewer.tsx`. Tests: `web/test/`.
Switch: none. Address parameters `player`, `dmg` and `fx`, and the viewer's `localStorage` keys, are listed under [Interfaces](#interfaces).

How the game page's 3D view opens: where its camera starts, what is on
screen while the map's geometry loads, how the geometry appears, how the
controls and name tags sit over it. How damage reads, and the combat
effects (shields, explosions, impacts), share the look and are specified
in [viewer-effects.md](viewer-effects.md). The one rule: the level is never
shown as a small object in an empty sky.

The first frame the viewer can see
is framed on a player or on the space the match was played in.

Everything here is drawn with CSS, inline SVG and the viewer's own
shaders. It evokes Halo 2's 2004 menus and loading screens (dark navy
plates, pale blue keylines, cut corners, tracked capitals, a segmented
bar) and copies no Bungie or Microsoft art, logo, font or texture.

## How it works

### Opening camera

`startCamera` (`web/src/viewer/start.ts`, pure) picks the opening shot
once, when the first frame with players arrives or 1500 ms after the map
is adopted, whichever is first. It returns `{ kind: "player", index }` or
`{ kind: "bounds" }`. The first rule that matches wins:

| # | Rule | Source |
|---|---|---|
| 0 | An explicit camera: `?cam=`, a fresh killcam request (`cameraFresh`), or a `director` | as `Viewer3D` takes them; `startCamera` is not asked |
| 1 | The player the address names: `?player=<gamertag or xuid>` | `GameViewer`'s search params |
| 2 | The signed-in player, when they are in the match | `identity.me()?.xuid` equal to a `game.players[i].xuid` |
| 3 | The page's focus player | `GameViewer`'s `focus?: () => number \| null` prop |
| 4 | The bounds of the playable space | `playableBounds` |

- `?player=` is matched, trimmed and without regard to case, against each
  player's `xuid`, `name` and `game_name` (`GamePlayerState`,
  `web/src/state/reducer.ts`); the lowest index wins. A value that matches
  nobody falls through to rule 2.
- Player pages link to a game with their player in the address:
  `gameHref(id, { player })` in `web/src/routes.ts`, used by the game
  links on the service record, game history and campaign record
  (`web/src/stats/Player.tsx` and `web/src/stats/Side.tsx`).
- An embedding page passes `focus` (the bot under training in
  `web/src/admin/ClankerTraining.tsx`). The game page passes none.
- A player rule needs that player in the frame it is applied to. Either
  life state qualifies. If the player is absent when the geometry opens,
  the remaining rules use that frame.
- A player result opens in `follow` on that index. A bounds result opens
  in `orbit`.

**Playable bounds.** The level's whole box is the wrong frame: a map's
structure carries cliffs, towers and far ice that nobody stands on, and a
sphere fitted to it leaves the playable part small. `playableBounds(level,
spots, seen)` (`start.ts`) builds the frame box from where play happens:

1. Points: every scenario placement of kind `netgame`, `weap`, `eqip`,
   `vehi` or `ctrl` (`ViewerScene.spots()`), every teleporter and lift
   (`MapFeatures`), and the four corners of the recording's `seen` bounds
   (`MapFrame.seen`) at the points' lowest z.
2. Fewer than six points and no `seen`: the level box is the frame box.
3. Otherwise the points' box, with z from `min - 1` to `max + 3` (head
   room), x and y padded by `max(4, 12% of the longer side)` game units,
   then intersected with the level box. A side under 6 units is widened to
   6 about its middle.
4. If the result covers more than 85% of the level box's floor area, the
   level box is used: the tighter frame gains nothing.

A level in zones (`LevelInfo.zoneCount > 1`) keeps the zone in play's box
(`ZoneFollower`); `playableBounds` is for a level with one playable zone.
With no geometry the frame box is the fitted grid's.

The scene keeps two boxes. `bounds` (the level box) still sizes the sun,
shadows, fog, ground, cut range and camera limits (`setBounds`).
`frameBox` is what `frameCamera`, **Reset** and the top-down camera fit.

**The fit.** `fitBox(box, dir, fovDeg, aspect, margin)` (`camera.ts`)
replaces the sphere fit in `cameraFit`. With `d` the unit vector from the
box's centre to the camera, `r` and `u` the camera's right and up, and `c`
each of the eight corners relative to the centre, the distance is

```
D = max over corners of  c·d + |c·r| / (tan(h) · (1 - margin))
                         c·d + |c·u| / (tan(v) · (1 - margin))
```

where `v` is half the vertical field of view and `h = atan(tan(v) ·
aspect)`. `margin` is 0.10. `D` is clamped to `controls.minDistance × 4`
and `controls.maxDistance`. The top-down distance uses the same function
with `d` straight up.

- Azimuth: from the south-east, as `cameraFit`'s `dir` has it, so the sun
  stays behind the camera's left shoulder.
- Pitch: `clamp(35 + (0.25 - height / max(width, depth)) × 40, 26, 44)`
  degrees above the horizon. A flat, wide map is seen from higher (44°
  at most); a tall one from lower, so its storeys separate.
- Field of view: the camera's 42°, unchanged.

**The ease.** The opening move is one tween, started on the frame the
reveal starts (below), so the camera never moves over an empty view.

| Opening | From | To | Duration |
|---|---|---|---|
| bounds | the fit, at `1.35 × D`, azimuth 14° clockwise of it, pitch 4° higher | the fit | 1400 ms |
| player | the fit of the frame box | `follow` on the player, at `followDistance` | 1800 ms |

Position and target are interpolated with `easeOutExpo`: `t === 1 ? 1 :
1 - 2^(-10 t)` (the CSS `cubic-bezier(0.16, 1, 0.3, 1)`). `tween` gains an
`ease` field; other camera moves keep their 650 ms. Any input on the
controls ends the tween where it is (`controls` `"start"`). With reduced
motion the camera is placed at the end pose at once.

### The overlay

Moved to [Viewer overlay: the overlay, name tags and reveal](viewer-overlay-layers.md#the-overlay).

### Name tags

Moved to [Viewer overlay: the overlay, name tags and reveal](viewer-overlay-layers.md#name-tags).

### Progressive reveal

Moved to [Viewer overlay: the overlay, name tags and reveal](viewer-overlay-layers.md#progressive-reveal).

### Damage and combat effects

Moved to [Viewer overlay: the overlay, name tags and reveal](viewer-overlay-layers.md#damage-and-combat-effects).

## Interfaces

- **Address:** `?player=<gamertag|xuid>` (opening camera), `?dmg=off|bars|numbers`,
  and the planned `?fx=off|low|high` ([viewer-effects.md](viewer-effects.md)). `cam`,
  `cut`, `see`, `ao`, `view`, `map` and `t` are unchanged.
- **Storage:** `h2.viewer.damage` (`map/vitals.ts`) and
  `h2.viewer.textures` (`textures3d.ts`). The combat-effects design uses
  `h2.viewer.fx`.
- **`GameViewer` props:** `focus?: () => number | null`. It passes
  `title`, `backdrop` and the resolved start to `Viewer3D`.
- **`ViewerScene`:** `loadMap(url, onProgress?): Promise<"ok" |
  "failed">`, `setCurtain(on)`, `setStart(start)`, and `onReveal?:
  (state: "started" | "done") => void`. `SceneOptions.reveal?: boolean`.
- **Keys** on the 3D view's root: `r` reset, `t` top-down, `m` the View
  menu, `h` hide the overlay, beside `f`, `c`, `[`, `]`, `x`, `v`.
- **Marks** for `web/bench/viewer.bench.ts`: `h2-viewer:revealed`, beside
  `h2-viewer:map-loaded`, `h2-viewer:first-map-frame` and
  `h2-viewer:textured`.

## Rules and limits

- No art, logo, font or texture from the game or its publisher is used.
  The map picture behind the loading plate is the server owner's own
  extraction, when their server has one, as everywhere else in the app.
- The overlay never covers the transport: it is inside `.v3`, above the
  controls row.
- An explicit camera in the address always beats the opening rules.
- The opening rules run once per mount. A live game's `seen` bounds grow,
  and the camera is not refitted to them.
- The reveal and the opening tween never block input: the rail takes
  clicks as soon as it is in, and a drag ends the tween.
- `playableBounds` depends on what the map's glb places. A level with
  fewer than six item placements and no recording frames is framed on its
  whole box.

## Evidence

- **Test** `web/test/viewer-start.test.ts` covers opening selection, playable bounds and the corner fit.
- **Code** `web/src/viewer/scene.ts` `loadMap`: the box leaves out the
  backdrop, cutscene zones and placed objects (`aside`), and nothing else
  of the structure.
- **Test** `web/test/viewer-overlay.test.ts` covers load states, progress ARIA and conditional menu rows.
- **Code** `web/src/viewer/cutaway.ts` `SEE_FRAGMENT_BODY`: the map
  materials already `discard` and already carry `vSeeWorld`.
- **Test** `web/test/reveal.test.ts` covers the sweep timeline and shared shader uniforms.
- **S** The art server sends a `Content-Length` for `map.glb`, so the
  bar is determinate. The chase covers the case where it does not.
- **Design** Every size, colour, curve and duration on this page.

## Not verified

- That the playable box frames each retail multiplayer map well, Lockout
  first: a look at the running viewer per map.
- The reveal's frame time on the largest campaign level, and that no
  shader recompiles when it ends (`bun run bench:viewer`, the
  `h2-viewer:revealed` mark).
- How the sweep's shadow fade reads on the viewer's supported GPUs.
- That the rail controls and tags read on a phone in full screen.

## Related

- [web-viewer.md](web-viewer.md): the viewer.
- [viewer-effects.md](viewer-effects.md): damage and combat effects.
- [web-app.md](web-app.md): the app, its pages and the mock server.
- [live-art.md](live-art.md): the map glb, its placements and features.
