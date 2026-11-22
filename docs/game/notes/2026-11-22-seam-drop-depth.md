# Seam drops with a carried velocity: what makes the Ascension seam bounce

Evidence for [superbounce.md](../superbounce.md) and [sim.md](../../runtime/sim.md), "Seam drops". Nothing here states what the code does; the design pages do.

## 2026-11-22: the bounce is set by the depth of the last free tick

Measured at `278598625` on the coordinator's Mac (load ~150; the drops are deterministic, so the load doesn't change them). Built with `H2_OPT=2`, fast tier, image `/Volumes/Builds/h2recomp/image`.

Every drop targets the ghost run's landing on Ascension, `point = [-10.457, 3.976, -3.20]`, with drift `[0, 1]` and the prime held. `carry` writes a fixed horizontal velocity through the fall. Positions are the bounding-sphere centre (0x30).

```sh
H2_OPT=2 h2 sim drops e1.toml --out /Volumes/Builds/h2recomp/image --tier fast --parallel 16
H2_OPT=2 h2 sim drops e2.toml --out /Volumes/Builds/h2recomp/image --tier fast --parallel 16
```

- **E1:** heights 3.45, 3.65 and 3.82, across −0.04, 0 and +0.04, carry (−0.12, 0.2/0.38/0.6/1.0), (0, 0.38) and (0, 0). That is 54 drops, and 9 bounced, all from height 3.45.
- **E2:** heights 3.36 to 3.64 in steps of 0.04, across 0 and +0.04, carry (−0.12, 0.1/0.2/0.3/0.45/0.6/0.8). That is 96 drops, and 38 bounced.

### Findings

- **Speed is not the cause.** Every touchdown came in at 5.24 to 5.39 u/s down, bounced or not.
- **The drift has no window of its own.** Bounces came at drifts from 0.1 to 0.8. In E1, the plumb drops at 3.65 (carry [0, 0]) failed for the same reason as the rest, explained below.
- **What decides it is the sphere centre's height on the last airborne row**, the row before the contact tick. The body bounces when that row ends low enough that the next tick goes deep into the seam:

  | last-row y | bounced at z | failed at z |
  |---|---|---|
  | 3.93 to 3.95 | −2.808 to −2.875 | −2.749 to −2.795 |
  | 3.97 to 3.99 | −2.768 to −2.875 | −2.738 to −2.774 |

  So the cut is near z −2.80 at y 3.94 and near −2.77 at y 3.98, roughly `z − 0.75·y < −5.755`. That is the slope of a line against the seam, and the boundary is about ±0.003 wide.
- **The landing window is y 3.93 to 3.99.** At y 3.91 to 3.92, no drop bounced at any depth.
- **About half the tick phases bounce.** A fall moves about 0.18 per tick at this speed, and the bounce band is the lowest ~0.08 to 0.1 of it. A perfectly placed landing therefore bounces or not depending on where the fall's tick phase puts the last free row. The phase is fixed once the body is in the air, so it can be predicted before touchdown.

Results: `$RUSH_KEEP/sb-drops/results/e1` and `results/e2`, under the coordinator's session keep.
