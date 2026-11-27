# FrameScene measurements: how to run the A/B, the counters and the parity gates

Status: Guide. Section 12 of [framescene.md](framescene.md): the commands that measure a slice. No measured row is recorded here; numbers go in `notes/` with the commit that produced them.
Switch: the `H2_FEATURES` arms below (`impl.render.native`, `frame-scene.sink`, `frame-scene.census`) are listed in [switches.md](../runtime/switches.md).

| Check | Where | Section |
|---|---|---|
| fps, p50, p99 and phases, native against D3D | the laptop through `h2 lab` | A/B on the laptop |
| per-stage counters | `h2 agent` on a serve | Counters on a serve |
| pixels and the sim | two launches from boot | Pixels and the sim |
| 60 and 120 fps | laptop A/B with `H2_FPS` | 60 and 120 |

Performance numbers come from the quiet laptop through `h2 lab`
([lab.md](../runtime/lab.md)). Local runs are for parity and smoke
checks. Run each pair back to back, three times, and report medians.

## A/B on the laptop

Set `B` to the slice branch. The slice 6 owner arm
uses the threaded, single-copy defaults of a fresh GPU run.

```sh
for arm in off on; do
  features="impl.render.native=$arm"
  if [ "$arm" = on ]; then features="$features,frame-scene.sink"; fi
  h2 lab run --name "Native renderer $arm" --host laptop --branch "$B" --level 03a_oldmombasa --difficulty heroic \
    --frames 5000 --stats-from 3000 --build-env H2_OPT=2 --fresh-data \
    --env H2_RENDER_SCALE=4 --env "H2_FEATURES=$features" \
    --wait --to "$RUSH_KEEP/lab/$B-$arm"
done
h2 lab runs --host laptop
```

Repeat with `--env H2_RENDER_SCALE=1`, and with `--replay fight-1` once
the user has recorded a fight (`h2 lab record --level 03a_oldmombasa
--difficulty heroic --name fight-1`). Read `frames.csv`: fps, p50 and
p99 frame time, and the phases `cpu`, `render`, `decode`, `encode`,
`submit`, `gpu-wait`, `render-wait`, `d3d`.

## Counters on a serve

The coordinator's `ab.sh`; `H` is the built
`h2`, `IMG` its image directory, and `F` either
`impl.render.native=on,frame-scene.sink` or `impl.render.native=off`:

```sh
env H2_OPT=2 H2_BOOT_GFX=$TMPDIR/ab-gfx H2_BOOT_GFX_EVERY=never \
    H2_LEVEL=03a_oldmombasa H2_RENDER_SCALE=4 H2_RENDER_THREAD=1 H2_SINGLE_COPY=1 H2_FEATURES="$F" \
    $H serve disc/halo2.iso --no-movies --socket $S --out $IMG &
$H agent wait '{"phase":"gameplay"}' 600000 --socket $S --timeout 620
$H agent capture 600 --socket $S --timeout 300 > capture.json
$H agent render.stats --socket $S > render-stats.json
$H agent profile.counters --socket $S > counters.json
$H agent render.scene --socket $S > scene.json          # from slice 1
```

For the slice 6 owner arm, check `render.scene.counters` after gameplay
starts. The completed window must contain game-view presents, each
reported in `scene_window_owned`, with zero `scene_window_ordinary`.

For the entry census (slice 4) add `frame-scene.census` to
`H2_FEATURES` and read `.entries` of `render.scene`: per entry, its
`d3d_draws` (8.2 rule 1) and its seed steps, animation gets and
particle updates (rule 2), with `unbalanced` zero.

Counters to read per frame:

| From | Counters |
|---|---|
| always | `passes`, `command_buffers`, `draws`, `native_<stage>_{examined,suppressed,refused_*,prepare_ns,encode_ns,worker_round_trips,command_buffers}` |
| slice 1 | `scene_dropped_*`, `scene_unresident`, `scene_readback`, `scene_held_draws` |
| slice 2 | `native_<stage>_{extract_ns,render_prepare_ns,scene_stages,scene_dropped}` |
| slice 3 | `walk_unsupported`, `walk_part_{aux_state,subparts,shader_flag,argument,program,layout}` |
| slice 5 | `render.scene`'s per-view `fog`, `water` and `post`; `scene_{fog,water}_{views,gate_held,drawn,parts}` |
| slice 6 (with `frame-scene.sink` in `H2_FEATURES`) | `render.scene`'s `carried`; `scene_carried_{frames,draws,ops}`, `scene_side_target_frames`, `scene_readback_{surface,depth,mirror,pixel_count}` |

## Pixels and the sim

Two launches from boot, same image:

```sh
for arm in off on; do
  features="impl.render.native=$arm"
  if [ "$arm" = on ]; then features="$features,frame-scene.sink"; fi
  env H2_OPT=2 H2_LEVEL=03a_oldmombasa H2_FEATURES="$features" \
      H2_RENDER_SCALE="${SCALE:?set SCALE to 1 or 4}" H2_RENDER_THREAD=1 H2_SINGLE_COPY=1 \
      H2_BOOT_GFX=$R/$arm H2_BOOT_GFX_EVERY=500 H2_BOOT_STOP_AT_PRESENT=5000 \
      H2_FRAME_STATS=1 H2_FRAME_STATS_FROM=3000 H2_FRAME_STATS_CSV=$R/$arm.csv \
      H2_LOCKSTEP=$R/$arm.lock \
      $H run disc/halo2.iso --out $IMG --data $R/data-$arm --fresh-data --slices 10000000
done
for f in $R/off/frame-*.png; do
  n=$(basename "$f")
  $H agent frame compare "$f" "$R/on/$n" --diff "$R/diff-$n"
done
$H lockstep $R/off.lock $R/on.lock
```

The hash gate, on two serves started as in the counters block (one per
arm), once both are in gameplay:

```sh
$H agent sim.hashes --socket $S_OFF > off-hashes.json
$H agent sim.hashes --socket $S_ON  > on-hashes.json
diff <(jq -c '.rows[]' off-hashes.json) <(jq -c '.rows[]' on-hashes.json) | head
```

## 60 and 120

Slice 8: the laptop A/B with `--env H2_FPS=60` and
`120`, plus on a serve `H2_FPS=120` and `h2 agent render.stats`, which
reports the render thread's time per display frame, frames per scene,
holds and their reasons, and `scene_held_draws`.

## Frame step cost

Slices 4 to 9: add `--env H2_FEATURES=render-timing,impl.render.native=on`
to the laptop run and read the `render` phase and its sites. The target
for the sim thread's frame step on Old Mombasa is under 6 ms after
slice 6 and under 3 ms after slice 9 (**Estimate**; `render` was about 25 ms of the game thread before any native stage).
## Recording a result

Each slice adds a dated row to a notes page: commit, scale, fps and p99 for both arms, the counters above, the hash result.
