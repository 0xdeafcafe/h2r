# Damage and combat effects in the game viewer

Status: Design. Nothing on this page is built; [web-viewer.md](web-viewer.md) states the damage numbers, muzzle flashes, tracers, hit lines and film sparks the viewer draws until it is.
Code: `web/src/viewer/effects3d.ts` (`Effects3D`), `web/src/viewer/projectiles.ts`, `web/src/map/events.ts` (`HitQueue`), `web/src/map/render.ts` (`drawHit`), `web/src/api/film-fx.ts`. Tests: `web/test/`.
Switch: none. The address parameters `dmg` and `fx` and the `localStorage` keys `h2.viewer.damage` and `h2.viewer.fx` are listed in [viewer-overlay.md](viewer-overlay.md#interfaces).

How damage reads in the 3D view and on the 2D maps, and what both draw
for the things that happen in a fight: shields, explosions, impacts,
muzzle flashes, melee, teleporters and deaths. Each effect is derived from data a
recording already has wherever that is possible, and the page says which
are not and what the game would have to send. The look evokes Halo 2's
effects with original particles and shaders: no effect texture, sprite
sheet or model from the game is used, and no picture file at all. The one
rule: an effect is drawn only for something the data says happened, at the
place the data puts it.

## How it works

### Damage

The view does not print a hit's `amount`. It shows what the hit did: the
victim's shield and body, on their tag, read from the frames' `body` and
`shield`.

**What `amount` holds.** A hit's `amount` is the damage report's body
amount plus its shield amount ([live-telemetry.md](live-telemetry.md),
"hits"; the report's layout is in [live-gamestate.md](live-gamestate.md),
"Damage-aware alternative"). It is sent in 1/256 steps with no upper
bound (`AMOUNT_SCALE`, `crates/live/live-proto/src/pack.rs`), and
`hitText` (`web/src/map/events.ts`) multiplies it by 100 and prints it as
a percentage of the victim's vitality. The report's two fields are marked
"likely" vitality fractions, not confirmed, and recordings carry values in
the tens (see [Evidence](#evidence)), which no fraction of one body can
be. `amount` is therefore an unscaled weight to the viewer. The right
scale is the share of the victim's vitality actually lost, and the frames
already carry it exactly: `body` (0 to 1) and `shield` (0 to 4, above 1
an overshield), read through the game's own health and shield getters
([live-telemetry.md](live-telemetry.md), "`body` and `shield`").

**Vitals.** `SampledPlayer` (`web/src/state/frames.ts`) gains `body` and
`shield`, held from the frame at or before the sampled tick (not
interpolated: a hit is a step). `VitalsTrack` (`web/src/map/vitals.ts`,
pure, one per view like `EventFeed`) watches them per player and actor and
returns, each frame:

```ts
interface Vitals {
  shield: number; body: number;   // as sampled
  chipFrom: number | null;        // shield + body before the burst, while its chip shows
  lost: number;                   // the burst's loss: percent of a fresh spawn (full shield + full body = 100)
  state: "steady" | "hit" | "broken" | "recharging" | "over";
  from: { x: number; y: number; z: number } | null; // the last attacker's position, from the frame's hits
}
```

- A **burst** is every drop in one victim's shield or body with less than
  600 ms between drops. There is one burst per victim, whoever is
  shooting, so nothing stacks. It ends 600 ms after the last drop.
- The attacker and the hit's kind come from the frame's hit whose `to` is
  the victim, within 3 ticks. A drop with no such hit (a fall, a
  recharge interrupted) has no direction.
- `broken`: the shield reached 0 with the player alive. `recharging`: the
  shield is rising. `over`: the shield is above 1.

**On the tag** (3D), under the name, the tag's width (44px at least):

```
 [o Mendoza  BR]          [o Mendoza  BR]  62       [o Mendoza  BR]
 ==============           =====::::-----            --------------
                          ########:::                ######
   steady                   in a burst                shield broken
```

- Shield bar: 3px, `#8fd4ff` on `rgba(0, 0, 0, .6)`. An overshield adds a
  2px `#9dffb0` bar above it, filled to `min(1, shield - 1)`.
- Body bar: 2px under the shield bar, shown only when `body < 0.995` or
  within 3 s of a body drop. `#ffd36b` above half, `#ff8a3d` to a
  quarter, `#f2452e` under it.
- **Chip:** on a drop the bar steps to its new value at once and the lost
  part stays as a `rgba(223, 243, 255, .8)` chip. The chip holds while
  the burst runs and for 350 ms after, then drains in 250 ms linear. A
  burst's chips join: the chip's far edge is the value before the burst.
- **Flare:** the tag's border is `#dff3ff` for 120 ms on each drop.
- **Break:** the shield bar's track blinks `#f2452e` twice (90 ms on, 90
  off).
- With reduced motion the chip is removed at the burst's end without
  draining, and the break is one 300 ms hold.

**In the world.**

- **Direction tick.** A hit with a known attacker draws a 60° arc on the
  victim's ground ring, on the attacker's side, in `hitColour`'s colour,
  fading over 450 ms. It replaces the hit marker sprite.
- **Line.** The attacker-to-victim line (`HIT_LINE_MS`) is drawn only when
  the attacker or the victim is in focus (followed, or pointed at in the
  scoreboard), and for a kill.
- **Kill confirm.** `HitQueue.markKills` decides it (a victim found dead while their marks are up). A kill
  draws the line in the killer's colour for 400 ms and a ringed cross at
  the victim's chest for 900 ms (`KILL_MARK_MS` becomes 900). In full
  screen, where the page's field report is hidden, the status strip gains
  a line per kill, `KILLER › weapon › VICTIM`, for 3 s, three at most.

**Numbers.** Off by default. With them on, one number per burst sits at
the right of the victim's tag: `Vitals.lost`, rounded, at least 1. Its
unit is percent of a fresh spawn, so a kill from full reads 100 and an
overshield can pass it. It counts up while the burst runs and leaves with
the chip. Nothing floats in the world.

**The switch.** The View menu's **Damage** row: `OFF`, `BARS` (default),
`BARS AND NUMBERS`. It is `?dmg=off|bars|numbers` in the address and
`h2.viewer.damage` in `localStorage` (the address wins), and the 2D map
reads the same value.

**2D maps** (`web/src/map/render.ts`, drawn by `MapCanvas` and
`ViewMap`).

- Each dot has a 270° ring, open at the bottom: the shield as a 1.5px
  `#8fd4ff` arc at the dot's radius plus 3px; the body as a 1px arc
  inside it, only when damaged. The chip is the same arc, paler.
- A hit draws a 6px wedge on the ring towards the attacker for 450 ms.
  The attacker-to-victim line follows the 3D rule (focus and kills).
- A kill keeps the legend's knockout cross.
- With numbers on, the burst's number sits after the dot's label.
- `drawHit`'s floating text is removed. Mini maps draw no damage, as
  `MapCanvas` has it (`props.mini`).
- The tools row (`.s-map-tools`, `TacticalMap.tsx`) gains a **Damage**
  select with the three values.

**Without vitals.** A recording whose frames carry no `body` or `shield`
(a sender before packed version 3) shows no bars and no numbers. Its hits
draw the direction tick, the line under the focus rule, and the kill
confirm. Vehicles have no vitality in the frames: a hit on one draws the
tick only.

`hitText`, `LiveHit.text`, `LiveHit.rise`, the `.v3-hit` label and
`Effects3D`'s hit sprite are removed with their assertions in
`web/test/anim.test.ts`.

### What a recording has

| Source | Fields the effects read | Where |
|---|---|---|
| Frames, per player and AI biped | `pos`, `aim`, `weapon`, `species`, `stance`, `vel`, `actions` (grenade, melee, reload, jump, switch, pickup), `fire` (a shot per tick), `body`, `shield`, `anim` | [live-telemetry.md](live-telemetry.md); `web/src/state/frames.ts` |
| Frames, `hits` | attacker, victim, `kind`, `weapon`, the victim's position | the same; `web/src/map/events.ts` |
| Frames, `projectiles` | every projectile under 100 units a second, by id: kind, `pos`, `vel`, `parent` when stuck | `FrameProjectile`, `web/src/api/types.ts` |
| Frames, `vehicles` | every vehicle: kind, pose, `occupied`, `vel` | `FrameVehicle` |
| Kill events | victim, killer, weapon, kind, position | `Kill`, `web/src/state/reducer.ts` |
| Map glb | teleporters with their links, lifts | `MapFeatures`, `web/src/map/features.ts` |
| Art manifest | each weapon's shot kind and speed | `DiscBallistics`, `web/src/viewer/projectiles.ts` |
| Host film (finished games that have one) | `barrel` (the muzzle and aim of each shot), `impact` and `blast` (a damage report's origin and direction), `terminal` (where a projectile ended, and its velocity) | `crates/live/live-telemetry/src/impacts.rs`, `web/src/api/film-fx.ts`, `FilmEffect` |

Frames reach every viewer: a live game, a recording, a replay tab. The
host film is the exact tier, present for some finished games. Each effect
below names its frames-only derivation first and what the film sharpens.

### The effects

"Derived" means the viewer works it out from the table above and the game
sends nothing new.

| Effect | Data | Derived from | New telemetry |
|---|---|---|---|
| Shield flare | `shield` falls; the hit names the attacker | `VitalsTrack` ([Damage](#damage)) | none |
| Shield break | `shield` reaches 0, alive | `VitalsTrack` | none |
| Shield recharge | `shield` rises | `VitalsTrack` | none |
| Overshield | `shield` above 1 | `VitalsTrack` | none |
| Active camouflage | nothing | not derivable | a `camo` level per unit |
| Grenade, rocket and fuel rod explosions | a projectile id leaves the frames | its last `pos + vel × dt`, stopped at the level | none; the film's `blast` is exact |
| Vehicle explosion | nothing says a vehicle died | not derivable | a `destroyed` event |
| Plasma and needle impacts | a projectile id leaves the frames | the same extrapolation | none; the film's `terminal` is exact |
| Bullet impacts | `fire`, `aim`, the level | the tracer's end, as `projectiles.ts` finds it | none; the film's `impact` is exact |
| Muzzle flash, tracers | `fire`, `aim`, `weapon` | as drawn (`web/src/map/shots.ts`, `projectiles.ts`) | none; the film's `barrel` is exact |
| Melee | the `melee` action; a `melee` hit when it lands | both | none |
| Sword lunge | the `melee` action with a melee-kind weapon and a dash | heuristic (below) | none; `anim` names it when present |
| Teleporter use | a position jump between a teleporter and its link | frames and `MapFeatures` | none |
| Death | `alive` falls; the kill event's kind | both | none (a ragdoll is not drawn) |

**Shields.** One instanced ellipsoid per biped that needs one, 0.5 of its
height wide and 1.05 tall, drawn additive with no depth write. Its
fragment shader is a fresnel rim (`pow(1 - dot(n, v), 2.5)`) times a
hex-cell pattern computed from the fragment's object position, with no
texture. Spartans' shells are warm (`#ffd98a`), elites' cool (`#8fd4ff`),
by the frame's `species`.

- *Flare:* opacity `0.9 → 0` over 280 ms with `1 - (1 - t)^2`, brightest
  on the side facing the attacker (a lobe of `max(0, dot(n, hitDir))^2`).
  A burst re-arms it at most every 90 ms.
- *Break:* the flare at 1.6 times the brightness for 120 ms, then 10
  sparks thrown outwards, living 350 ms. While the shield stays at 0, two
  sparks every 400 ms at the shoulders.
- *Recharge:* the shell at 0.35 opacity, shown below a line that rises
  from the feet to the head as `shield` goes from its low value to full;
  a 150 ms flash at full.
- *Overshield:* the shell held at 0.22 opacity in `#9dffb0`, breathing
  between 0.18 and 0.26 over 1200 ms, for as long as `shield > 1`.
- Without `body` and `shield` in the frames, no shell is drawn.

**Active camouflage.** The frames say nothing of it, and it is not
guessed. With the new field (below), a camouflaged biped's materials go to
0.12 opacity with a rim shimmer (the shell's fresnel, at 0.3, scrolled by
a hash noise), its weapon the same, and its tag stays, dimmed to 0.6 with
`CAMO` after the name: the viewer is a spectator's tool, not a player's
view.

**Explosions.** A projectile whose shot kind is `grenade` or `rocket`
(`Look.kind`) and whose id is in one frame and not the next, with the two
frames at most 0.25 s apart and no seek or `cut` between them, detonated.
The point is its last position plus its velocity times the time to the
later frame, stopped at the first piece of level on that line (the
raycast `projectiles.ts` uses). A stuck one detonates at its parent.
Three layers, sized by kind (a rocket 1.4 times a grenade):

1. *Flash:* one camera-facing quad, a radial falloff in the shader,
   additive, 2.2 units across, 90 ms.
2. *Fireball:* 14 quads (6 at low quality) thrown 0.4 to 1.1 units,
   their alpha eroded by a hash-noise threshold that rises with age. The
   colour ramp is `#fff3c4 → #ff9a2e → #5a1d0a → #2a2a2a` over 550 ms.
3. *Shock ring:* a ground ring from 0 to 3.5 units in 300 ms, `1 - (1 -
   t)^3`, opacity `0.7 → 0`, with 10 spark streaks.

A plasma grenade's ramp starts from the projectile's `Look.colour` instead
of the warm white, and so does the fuel rod's: the tint is the one its
shot is already drawn in. With a host film, a `blast` effect replaces the
derived detonation at the same tick (the derived one within 4 ticks and 2
units of a `blast` is dropped).

**Plasma and needle impacts.** A `plasma` or `needle` projectile that
leaves the frames, by the same extrapolation, draws a splash: a 0.5 unit
disc facing the surface's normal (from the raycast; facing the camera
when there is no surface), additive, in the shot's colour, 180 ms, with 4
sparks. Several needles leaving one parent on the same tick are one burst
instead, in the needle's colour, sized by their count. A bullet's tracer
end gets 2 sparks and a 120 ms puff. With a host film the `terminal` and
`impact` effects give the point and the direction, and the derived one is
dropped as for a blast.

**Muzzle flashes and tracers.** The flash is one quad at the weapon's
`muzzle` node: a three-bladed star in the shader, in the shot's colour,
alive for the shot's tick and two more (about 50 ms). Tracers, bolts,
needles and rockets fly as `projectiles.ts` draws them. No light is added
to the scene for a flash.

**Melee.** The `melee` action draws a swept arc in front of the player
along their aim: a quarter ring 0.9 units out, 160 ms. A `melee` hit on
someone within 3 ticks of it adds a star at the victim's chest and 8
sparks.

**Sword lunge.** When the action's weapon has the shot kind `melee` and
the player moves more than 2 units in the 4 ticks around it, the path
they covered is drawn as a streak in the weapon's colour, fading over 250
ms. When the frame carries `anim`, the animation's state decides it
instead of the distance.

**Teleporter use.** A player whose position jumps between two frames
(further than `SNAP_DISTANCE`, `web/src/state/frames.ts`) from within 1.5
units of a teleporter to within 2.5 units of one its `links` lead to has
used it. The entrance draws a column closing to a line over 300 ms, the
exit a ring opening to 1.2 units with a flash over 350 ms, and the link's
arc (`features3d.ts`) brightens once along its direction. A respawn jumps
too, but not between a teleporter and its link.

**Deaths.** No ragdoll: the frames carry no bones, and sending them would
cost more than everything else in a player's entity put together (see
[Rules and limits](#rules-and-limits)). A death is the body's `die` clip
where the model has one (`viewer/bodies.ts`), a white flash on the model
for 80 ms, the kill confirm ([Damage](#damage))
and, for a kill whose kind is an explosion, the body's marker ring drawn
1.5 times its size for the confirm's life.

### New telemetry

Two additions, both optional on the wire and both dropped by a viewer
that does not know them.

| Addition | What the game does | Game-thread cost | Wire cost |
|---|---|---|---|
| `camo` on a player or AI biped: 0 to 3 (off, fading, mostly, full) | one more read from the unit the tick's object scan already visits | one guest read per unit per frame tick | absent when off; 2 bits and a presence bit when on |
| `destroyed` in a frame's new `fx` list: the vehicle's id and position | a record pushed when the body-depleted transition runs for a vehicle ([live-gamestate.md](live-gamestate.md), "Damage-aware alternative": the transition without player attribution) | a few guest reads per vehicle death, as the damage hook's row in [live-telemetry.md](live-telemetry.md) has it; nothing per tick | a list count per frame (3 bits when empty), about 8 bytes per event |

- Neither scans anything. The `fx` list keeps the newest 32 a frame.
- Both ride the next packed frame version and the v8 live wire's field
  list (`crates/live/live-proto/src/pack.rs`, `web/src/api/pack.ts`), and
  [live-telemetry.md](live-telemetry.md) gains their rows.
- The unit's camouflage field is not a located research point. It is
  found with `h2 disasm` before the field is added; until then no viewer
  code for camouflage is built.
- Exact detonations, impacts and muzzle positions need nothing new: the
  host film records them.

### Drawing them

`FxWatch` (`web/src/viewer/fx.ts`, pure) is to effects what `EventFeed`
is to damage: fed each sampled frame once, it returns the effects that
started (`{ kind, at, dir?, colour, size, seed, born }`), having matched
projectile ids, vitals, actions and teleporters as above. A seek observes
quietly and starts nothing. `FxPool` (`web/src/viewer/fx3d.ts`) draws
them.

- **Three draw calls for everything:** one `InstancedMesh` of quads
  turned to the camera in the vertex shader (flashes, fireballs, sparks,
  splashes, puffs), one of ellipsoids (shields), one of rings and ribbons
  (shock rings, melee arcs, teleporter columns, lunge streaks).
- **No textures.** Every shape is a function in the fragment shader: a
  radial falloff, a hash noise, a star, a hex cell.
- **No CPU per particle per frame.** An instance carries its birth time,
  life, velocity and seed; the vertex shader places it from one `uTime`
  uniform. JavaScript writes an instance only when it is born, into a
  ring buffer.
- **No lights, no depth writes.** Effects test depth against the scene
  and are left out of the ambient occlusion (`userData.noAo`).
- `Effects3D.film` draws the host film's effects through the same pool,
  with the looks above, in place of its spheres.

**Budget.**

| | High | Low |
|---|---|---|
| Live quads | 1024 | 384 |
| Shields | 32 | 16 |
| Rings and ribbons | 64 | 32 |
| Fireball quads per explosion | 14 | 6 |
| Sparks | as given | halved |
| CPU per rendered frame, 16 players in a firefight | 0.5 ms | 0.25 ms |
| GPU at 1080p on an integrated GPU | 1.0 ms | 0.4 ms |

- A pool that is full reuses its oldest instance.
- A quad is never larger than 2.5 units, and one within 3 units of the
  camera is drawn at half its opacity, which bounds the overdraw of an
  explosion in the viewer's face.
- At most three full-brightness flashes start in any second, across the
  whole view: later ones start at 0.4 of their brightness.
- The numbers in the table are targets. The measured ones go in a dated
  note under `docs/live/notes/` from `bun run bench:viewer`, and this
  table then links to it.

**The switch.** The View menu's **Combat effects** row
([viewer-overlay.md](viewer-overlay.md#the-overlay)): `OFF`, `LOW`,
`HIGH`. `HIGH` is the default, `LOW` under `pointer: coarse`. It is
`?fx=` in the address and `h2.viewer.fx` in `localStorage`. It is not the
**Show** menu's **Effects** row, which is the level's own glows and lift
plasma. When frames run over 40 ms for two seconds with the ambient
occlusion already off, the scene drops to `LOW` and tells the menu, as
`onAoDropped` does for the occlusion.

**Reduced motion.** No particle moves, nothing expands and nothing
flashes:

- a shield hit is the tag's bar and border only;
- a break, a recharge and an overshield are a still shell at 0.3 opacity
  for as long as the state lasts;
- an explosion is a still translucent disc of its final size for 400 ms;
- an impact is a still dot for 200 ms;
- a teleport is a still ring at each end for 400 ms;
- melee and the lunge draw nothing more than the model's own pose;
- a death is the kill confirm.

**2D maps.** No particles. `drawFx` (`web/src/map/render.ts`) draws, from
the same `FxWatch` events: a shield flare as a 250 ms pulse of the dot's
shield ring; a break as the ring in `#f2452e` for 300 ms; an overshield
as a second ring; an explosion as a circle outline growing to the blast's
size in map pixels over 300 ms; a teleport as a ring at each end and the
link's dashed curve brightened once; melee as a short arc ahead of the
dot. Impacts, muzzle flashes and the lunge are not drawn in 2D: the shot
lines already there say as much. Mini maps draw none.

## Build order

Damage first, then the derived effects; the two that need the game last. Each is one
branch, with the pure module and its `bun test` file first. Slice numbers
continue [viewer-overlay.md](viewer-overlay.md#build-order)'s.

| # | Slice | Files | Tests that prove it |
|---|---|---|---|
| 5 | Damage | new `map/vitals.ts`; `state/frames.ts` (`body`, `shield`); `map/events.ts`, `map/render.ts`; `viewer/effects3d.ts`, `viewer/scene.ts` (tag bars); `components/MapCanvas.tsx`, `map/ViewMap.tsx`, `viewer/TacticalMap.tsx`; `viewer/viewer.css` | `vitals.test.ts`: drops 500 ms apart are one burst and 700 ms apart two; `lost` is 100 for a kill from full; break, recharge and overshield states; a drop with no hit has no direction; frames without vitals give no bars; the pref reads the address before storage. `frames.test.ts`: `body` and `shield` are held, not interpolated. `anim.test.ts`: the kill mark's life, and no `text` on a live hit |
| 6 | The pool and the switch | new `viewer/fx.ts` (types, the pref), `viewer/fx3d.ts`; `viewer/scene.ts` (update section: one `FxPool`, the auto-drop); `Viewer3D.tsx` (the menu row, `?fx=`) | `fx.test.ts`: the pref reads the address before storage and defaults by pointer; the ring buffer reuses its oldest; the flash limiter passes three a second; a JS mirror of the vertex shader's placement gives `pos + vel × age` and ends at `life` |
| 7 | Shields | `viewer/fx.ts` (shield events from `VitalsTrack`), `viewer/fx3d.ts` (the shell), `viewer/scene.ts` | `fx.test.ts`: a drop gives one flare with the attacker's direction and re-arms no sooner than 90 ms; reaching 0 gives one break; rising gives a recharge that ends at full; above 1 an overshield; frames without vitals give nothing |
| 8 | Muzzle, tracer ends, film effects | `viewer/fx3d.ts`, `viewer/effects3d.ts` (`film` through the pool), `viewer/scene.ts` (shots) | `fx.test.ts`: a `fire` mask gives a flash per set tick; a film `blast`, `impact` and `terminal` each map to their look; `film-poses.test.ts` stays green |
| 9 | Explosions and impacts from projectiles | `viewer/fx.ts` (`FxWatch` projectile matching), `viewer/fx3d.ts` | `fx.test.ts`: a grenade id that leaves detonates at `pos + vel × dt`; a stuck one at its parent; frames 0.3 s apart, a seek and a cut detonate nothing; a plasma bolt is an impact, not an explosion; needles leaving one parent together are one burst; a derived detonation beside a film `blast` is dropped |
| 10 | Melee, lunge, teleporters, deaths | `viewer/fx.ts`, `viewer/fx3d.ts`, `viewer/features3d.ts` (the link's pulse), `viewer/scene.ts` (the death flash) | `fx.test.ts`: a melee action gives an arc and, with a hit in 3 ticks, a star; a 3 unit dash with a melee-kind weapon is a lunge and a 1 unit one is not; a jump from a teleporter to its link is a teleport, to anywhere else is not, and a respawn is not |
| 11 | 2D maps | `map/render.ts` (`drawFx`), `components/MapCanvas.tsx`, `map/ViewMap.tsx` | `fx.test.ts` against a recording canvas context: each 2D effect's calls, none with `fx` off, none on a mini map, the still forms with reduced motion |
| 12 | Reduced motion and low quality, across all of them | `viewer/fx3d.ts`, `viewer/fx.ts` | `fx.test.ts`: every effect kind has a still form with no velocity and no growth; low quality halves the counts in the budget table |
| 13 | Vehicle `destroyed` | `crates/live/live-telemetry/src/hooks.rs` and `event.rs`, `crates/live/live-proto/src/game.rs` and `pack.rs`, `web/src/api/pack.ts` and `types.ts`, `viewer/fx.ts`; [live-telemetry.md](live-telemetry.md) | `live-proto` pack round-trip and frame-size tests with an `fx` list; `pack.test.ts` reads it and an older reader skips it; `fx.test.ts`: a `destroyed` event is an explosion at the vehicle, 1.8 times a grenade's |
| 14 | `camo` | the research point first; then the same files as 13, `viewer/players3d.ts` (the material) | the same pack tests for the field; `fx.test.ts`: a biped with `camo` is drawn at its opacity and its tag reads `CAMO` |

Slice 5 stands alone and ships first. Slices 7 to 11 need slice 6 and
slice 5's `VitalsTrack`. Slices 13 and 14 are the only ones that touch the game.

## Interfaces

- `FxWatch.observe(frame, now, opts): Fx[]` and `FxWatch.clear()`, as
  `EventFeed.observe`.
- `FxPool.spawn(fx)`, `FxPool.update(time, quality, still)`,
  `FxPool.dispose()`.
- `ViewerScene.setFx(quality)` and `onFxDropped?: () => void`.
- The frame's `fx` list and the units' `camo`, once slices 13 and 14 add
  them: [live-telemetry.md](live-telemetry.md) is their home.

## Rules and limits

- A tag's body bar is a viewer's aid: the game's own display shows only
  the shield.
- Nothing is drawn that the data does not say. A projectile that leaves
  the frames more than 0.25 s after it was last seen draws nothing, so a
  recording at one frame a second shows no derived explosions.
- A derived detonation is an estimate: it is where the projectile was
  heading, not where the game detonated it. The film's is exact.
- A grenade that is deleted without detonating (a round ending) would
  draw an explosion. A frame with no players, and a `cut`, start nothing,
  which covers the round's end.
- The lunge is a guess from speed unless the frame has `anim`.
- **Ragdolls are not adopted.** A biped's pose is about twenty node
  rotations; at the frame grid's two 14-bit components a rotation that is
  some 70 bytes per body per frame before deltas, against a player entity
  of a few bytes when little changes, and the game thread would read every
  node of every body each frame tick.
- No picture file is fetched for an effect, and none of the game's effect
  art is reproduced: shapes are the shader functions named above.

## Evidence

- **Code** `web/src/api/types.ts` `FramePlayer.body`, `.shield`; no field
  of `SampledPlayer` carries them.
- **S** A hit's `amount` is not a fraction of one vitality: a Lockout
  recording printed 2222, 3888 and 4721 over three victims, which
  `hitText` makes from amounts of 22 to 47. What would verify the scale:
  on one recording, compare each hit's `amount` with the victim's `body +
  shield` drop across the frames either side of it.
- **Code** `web/src/api/film-fx.ts` `applyFilmEffects`: the film's
  `WeaponFire`, `Impact` and `ProjectileEnded` become `barrel`, `impact`
  or `blast`, and `terminal`.
- **Code** `web/src/viewer/effects3d.ts` `Effects3D.film`: "Deletion
  alone draws no explosion" for film effects, which is why a derived
  detonation is limited to grenade and rocket kinds.
- **Code** `docs/live/live-telemetry.md`, the per-tick cost table: the
  damage hook's "a few guest reads" is the model for the `destroyed`
  hook's cost.
- **S** The body-depleted transition runs once per vehicle death on the
  host. What would verify it: a breakpoint count against vehicles
  destroyed in one game, through the agent API.
- **S** Every figure in the budget table. What would verify them: `bun
  run bench:viewer` with a firefight recording.
- **Design** Every colour, size and duration on this page.

## Not verified

- Whether `amount` is damage in the damage effect's own units, and what
  divides it into a vitality fraction.
- That the shield's `species` tints, the fireball ramp and the plasma
  splash read as the game's at a glance, without its textures.
- The budget on a phone, and that the auto-drop to `LOW` triggers before
  the frame rate is felt.
- Where the unit's active camouflage level lives, and whether it is a
  level or a flag.
- That a teleporter's exit is always within 2.5 units of its link's end
  on every map.
- Whether `anim` names a lunge distinctly.

## Related

- [viewer-overlay.md](viewer-overlay.md): the opening camera, the
  overlay, the reveal and the damage presentation these effects sit in.
- [web-viewer.md](web-viewer.md): the viewer as built.
- [live-telemetry.md](live-telemetry.md): the frames and the hooks.
- [replay-fidelity.md](replay-fidelity.md): recording and playback.
