# Superbounce: the charge and the crouch spots, evidence

Evidence for [superbounce.md](../superbounce.md), "Charge, crouch spots and landings". Nothing here states what the code does; the design page does.

Newest first.

## 2026-11-21: Zanzibar's window, played

The user played Zanzibar's window bounce on the lab, read 2026-11-21:
lab job 0043, input recording `zanzibar-window` (on the laptop), on the
`sb4-next` build at `70bc527ad`. The coordinator polled
`superbounce.state` and `world.object` about every 12 ticks into a log
(columns: tick, Havok position, Havok velocity, air or ground, primed,
charge phase, `standing_fits`, last landing); the log is in the lab
session's keep directory (`zanz_watch.txt`), not in the repository. To
reproduce, replay the recording on the lab and poll the same two
methods. The polygons are from `h2 superbounce scan zanzibar --near`.

The recipe as the user gave it: in the base facing the beach, the right
inner window furthest back, break the glass, crouch under the broken
glass on the left side of the shard, let go, turn right, press jump,
fall onto a crack.

| Ticks | Where | Phase | What happened |
|---|---|---|---|
| 5338 to 5456 | (5.22, −26.285, 5.11), the sill (poly x 5.20 to 5.40, y −26.10 to −27.10, z 5.10) | `under`, then `held` | crouched under the shard |
| 5470 to 5484 | jumping from the sill | `primed` from about 5484 | the word turned −3 on the jump, still forced crouched |
| 5505 to 5546 | falling past (4.69, −26.52, 3.12) at −4.49 units/s, on the ground at (4.80, −25.89, 2.11) | `primed` | a small pop: z 2.32 with vz −0.02 between, before landing |
| 5818 to 5904 | the sill again | `under`, `held` | the second crouch |
| 5917 to 5971 | falling straight at (4.876, −26.748) from z 4.54 to 0.44, −5.34 units/s | `primed` | through the floor at the crease, then dead (position 0 at 5986) |
| 7565 to 7659 | (8.07, −3.01, 4.51), walked out to (8.10, −2.55) | `under`, `held`, then none | no prime |
| 8016 to 8071 | the same place, walked out to (8.48, −2.83) | `under`, `held`, `primed` at 8071 | primed on the walk out |
| 8555 to 8655 | the same place, walked out to (8.23, −2.75) | `under`, `held`, `primed` at 8655 | primed on the walk out |

- **The window's lip is the broken glass.** No structure or instanced
  geometry polygon makes a forced crouch at the sill; the glass is an
  object, and its broken shard's collision is not known offline.
- **The word turned on the jump at the window**, not on a walk out: the
  phase was `held` until the jump and `primed` on the first reading
  after it.
- **The crack is one concave crease.** The floor below is z 2.10 (poly
  x 1.8 to 6.0, y −22.2 to −26.7) meeting a 17.6° ramp, normal (0,
  0.3033, 0.9529), rising south from y −26.70 to −27.64 (z 2.40). There
  is no second joint within a capsule's diameter, so the slot rule
  rejected it; the scan's nearest bounce spot was its index 8 at (0.0,
  −27.47, 2.36).
- **The first try's pop was small**, about the 5.2 units/s the single
  crease calibration gives. The second try, 5 cm on the ramp side of the
  crease, fell through the floor.
- **The second prime place** at (8.07, −3.01, 4.51) read `near` on the
  way in, so the scan had a crouch spot within 1.5 of it. It primed on
  2 walk outs of 3, within 0.5 of the rest point. The landmark is not
  named. The log is coarse (one reading about every 12 ticks), so the
  tick each word turned is good to about 12.

## What the crouch filter would keep

Read 2026-11-21 with the installed `h2` (built from `origin/master` at
`d2e682614`), offline from the disc, no game run:

```sh
h2 superbounce scan <map> --json                 # the 30 best crouch spots
h2 superbounce scan <map> --near x,y,z,2.0       # the polygons round one
```

A script outside the repository re-tested the capsule (radius 0.175,
standing 0.725, crouched 0.5, hit at 0.005, as `crouch.rs`) on a 2 cm
grid 0.3 round each crouch spot's stand point, on a flat floor at the
stand's height, against the polygons `--near` printed. It is a rough
copy of the scanner, good for counting, not for a threshold.

| Map | Crouch spots | Below the lowest strip by 1.5 or more | Deepest standing overlap in the band, quartiles | Kept by the rule |
|---|---|---|---|---|
| ascension | 21 | 5 | 0.047, 0.115, 0.157 | 12 |
| lockout | 23 | 1 | 0.038, 0.043, 0.152 | 9 |
| midship | 30 | 0 | 0.050, 0.087, 0.113 | 12 |
| zanzibar | 30 | 0 | 0.066, 0.089, 0.100 | 12 |
| beavercreek | 30 | 0 | 0.045, 0.067, 0.094 | 12 |
| coagulation | 30 | 10 | 0.089, 0.122, 0.140 | 0, so the nearest 3 |
| foundation | 17 | 0 | 0, 0.060, 0.070 | 9 |
| colossus | 30 | 0 | 0.024, 0.095, 0.105 | 12 |

"Kept by the rule" is: not below the lowest strip by 1.5 (a stand-in for
the spawn rule, which needs the scenario's starting locations), a strip's
stand within 8, none within 3.0 of a better one, 12 at most. It filters
the scan's output, which is already the best 30 with none within 1.5 of
another, so a map at 30 may have more to choose from than this saw.

- **Overlap depth does not separate spots.** The measured Ascension
  corner's band reaches 0.040 (0.033 by the lip's distance from the
  axis). A cut at 0.025 or 0.03 drops 0 to 2 spots on six maps, 6 on
  foundation and 10 on colossus. Most bands are far deeper than the one
  spot known to prime, so depth is no reason to drop a spot. Foundation's
  zero quartile is the script's flat floor missing bands on slopes.
- **The measured corner ranks 13th of 21** on Ascension by today's score
  (0.75): a cut by score alone would drop the one spot known to work.
- **Five Ascension spots are under the map**, at z −5.4, −12.1, −16.3,
  −21.0 and −21.1; its strips are between −3.2 and 2.3.
- **Coagulation's crouch spots are all more than 8 from its 7 strips.**

## The pair word around a forced crouch, tick by tick

Read 2026-11-21 from the lab playback of [the pair
word](../superbounce.md#the-pair-word-trace-lab-playback) (`havok.agents`
and `world.object` every second tick through the recording, the primed
run; the replies are in the lab session's keep directory, not in the
repository). To reproduce, run that playback and print the word, the
crouch amount and the position each tick.

| Recording tick | Crouch amount | Word |
|---|---|---|
| 6069 to 6073, crouching | 0.33, 0.67, 1.0 | 0 |
| 6089 to 6127, still in the corner | 1.0 | 0, then no pair at 6127 |
| 6129 | 1.0 | 0 (a new pair) |
| 6159, the first tick of standing up | 0.83 | −3 |
| 6161 to 6165 | 0.5, 0.17, 0 | −3 |
| 6441, the first tick of the next crouch | 0.17 | 0 |
| 6451 to 6489, still in the corner | 1.0 | 0, then no pair at 6489 |
| 6503 | 1.0 | 0 (a new pair) |
| 6579, standing up | 0.83 | −3 |
| 6943 to 6981, standing still after the bounce | 0 | −3, then no pair at 6981 |

- **The word does not step.** It reads 0 through the whole crouch and −3
  on the first tick of the stand-up, both times. No reply between showed
  1, 2, −1 or −2. Every second tick was read, so a one-tick value could
  have been missed.
- **A crouch takes it away at once.** The word is back at 0 on the first
  tick the crouch amount leaves 0.
- **A still body loses its pair after 38 to 40 ticks.** Three times the
  pair went 38, 38 and 38 to 40 ticks after the body's speed fell under
  about 0.03 units/s (it was creeping at 0.02 to 0.03 the first two
  times). The speed limit itself is not measured.
- **The stand-up was 6 cm out.** The body stood up at (−10.861, 0.692)
  and (−10.853, 0.680), having rested at (−10.915, 0.725): the word
  became −3 where the crouch let go, at the band's way out, not at its
  deepest point.
