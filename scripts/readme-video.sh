#!/usr/bin/env bash
# Captures six seconds of HUD-less gameplay per level from your own disc.
# h2 serve renders off-screen into scratch, and the agent API waits for
# gameplay, hides the HUD and queues a timed controller move. No window opens.
#
#   scripts/readme-video.sh --out-image <dir> [--disc <iso>]
#       [--only <name>[,<name>]] [--dest <dir>] [--settle <seconds>]
#       [--fps 30|60|120] [--size WxH] [--scale 2-6]
#
# The image must be built with the same H2_OPT (default 2).
# Requires h2 (or $H2), jq and a GPU. Pass --dest to keep the frame folders
# somewhere else, then pass that directory to scripts/readme-art.sh --frames.
# Commit only the final documentation media under docs/images/** (Git LFS).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
names=(oldmombasa deltaapproach ascension)
levels=(03a_oldmombasa 05a_deltaapproach ascension)
clip_ticks=180

usage() {
  cat <<'EOT'
Usage: scripts/readme-video.sh --out-image <dir> [--disc <iso>] [--only <names>]
                               [--dest <dir>] [--settle <seconds>]
                               [--fps 30|60|120] [--size WxH] [--scale 2-6]

  --out-image <dir>  game image built with the same H2_OPT (default: 2), required
  --disc <iso>       your disc image (default: $H2_DISC, else disc/halo2.iso)
  --only <names>     comma-separated names (oldmombasa, deltaapproach, ascension)
  --dest <dir>       where to save frame folders (default: docs/images/readme/frames)
  --settle <s>       gameplay seconds before the move (default: 20)
  --fps 30|60|120    display samples per second (default: 30)
  --size WxH        exact crop and output size, e.g. 3840x2160
  --scale 2-6       render scale (default: $H2_RENDER_SCALE or enough for --size, at least 2)
EOT
}

fail() { echo "$*" >&2; exit 1; }

image=""
disc="${H2_DISC:-$root/disc/halo2.iso}"
only=""
dest="$root/docs/images/readme/frames"
settle=20
scale="${H2_RENDER_SCALE:-}"
fps=30
size=""
while (($# > 0)); do
  case "$1" in
    --out-image) image="${2:?--out-image needs a directory}"; shift 2 ;;
    --disc) disc="${2:?--disc needs an ISO path}"; shift 2 ;;
    --only) only="${2:?--only needs a list of levels}"; shift 2 ;;
    --dest) dest="${2:?--dest needs a directory}"; shift 2 ;;
    --settle) settle="${2:?--settle needs seconds}"; shift 2 ;;
    --scale) scale="${2:?--scale needs a render scale}"; shift 2 ;;
    --fps) fps="${2:?--fps needs a frame rate}"; shift 2 ;;
    --size) size="${2:?--size needs WxH}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

[[ -n "$image" && -d "$image" ]] || fail "pass the built image with --out-image <dir>"
[[ -f "$disc" ]] || fail "no disc image at $disc (set H2_DISC or pass --disc)"
case "$fps" in 30 | 60 | 120) ;; *) fail "--fps must be 30, 60 or 120" ;; esac
required_scale=2
if [[ -n "$size" ]]; then
  [[ "$size" =~ ^([1-9][0-9]{0,3})x([1-9][0-9]{0,3})$ ]] || fail "--size must be WxH"
  width="${BASH_REMATCH[1]}" height="${BASH_REMATCH[2]}"
  while ((width > 640 * required_scale || height > 480 * required_scale)); do
    ((required_scale += 1))
  done
  ((required_scale <= 6)) || fail "--size must fit the 3840x2880 view at scale 6"
fi
scale="${scale:-$required_scale}"
[[ "$scale" =~ ^[2-6]$ ]] || fail "--scale must be an integer from 2 to 6"
((scale >= required_scale)) || fail "--size needs a render scale of at least $required_scale"
[[ "$settle" =~ ^[0-9]+([.][0-9]+)?$ ]] || fail "--settle must be a nonnegative number of seconds"
if [[ -n "$only" ]]; then
  IFS=, read -r -a requested <<< "$only"
  for name in "${requested[@]}"; do
    [[ " ${names[*]} " == *" $name "* && -n "$name" ]] || fail "unknown level: $name"
  done
fi

h2="$(command -v "${H2:-h2}")" || fail "no h2 on PATH (set H2 or install the CLI)"
h2="$(cd "$(dirname "$h2")" && pwd)/$(basename "$h2")"
command -v jq >/dev/null || fail "needs jq (brew install jq)"
disc="$(cd "$(dirname "$disc")" && pwd)/$(basename "$disc")"
image="$(cd "$image" && pwd)"
settle_ticks="$(jq -n --argjson seconds "$settle" '($seconds * 30) | round')"
clip_frames=$((clip_ticks * fps / 30))
features="${H2_FEATURES:-}"
if ((fps > 30)); then
  features="${features:+$features,}impl.render.native=off,impl.render.pipelined_frame=off"
  features+=",impl.render.interpolate=on,impl.render.reproject=off,impl.render.reproject_ahead=off"
fi

mkdir -p "$dest"
# Stage the raw PNGs outside the checkout until the clip is complete.
scratch="$(mktemp -d "${TMPDIR:?set TMPDIR to a scratch directory}/rv.XXXXXX")"
serve_pid=""
serve_log=""
staging=""
clip_dir=""
stop_serve() {
  if [[ -n "$serve_pid" ]]; then
    kill "$serve_pid" 2>/dev/null || true
    wait "$serve_pid" 2>/dev/null || true
    serve_pid=""
  fi
}

cleanup() {
  local status=$?
  stop_serve
  if ((status != 0)) && [[ -f "$serve_log" ]]; then tail -20 "$serve_log" >&2; fi
  if [[ -n "$staging" ]]; then
    if [[ -e "$staging/previous" && ! -e "$clip_dir" ]]; then
      mv "$staging/previous" "$clip_dir" || {
        echo "previous clip saved at $staging/previous" >&2
        staging=""
      }
    fi
    if [[ -n "$staging" ]]; then rm -rf "$staging"; fi
  fi
  rm -rf "$scratch"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

agent() {
  # The Unix socket path must fit in sun_path, even when TMPDIR is long.
  (cd "$scratch" && "$h2" agent --socket "$1" --timeout 1810 "${@:2}")
}

# session.wait answers TIMEOUT after at most 600 s, so a long wait (a level
# intro while the bots build nav meshes) is retried in 600 s slices for up
# to 30 minutes.
agent_wait() {
  local socket="$1" until="$2" slice out
  for ((slice = 0; slice < 3; slice++)); do
    if out="$(agent "$socket" wait "$until" 600000 2>&1)"; then
      printf '%s\n' "$out"
      return 0
    fi
    grep -q 'timed out before the condition held' <<< "$out" || { printf '%s\n' "$out" >&2; return 1; }
  done
  printf '%s\n' "$out" >&2
  return 1
}

capture() {
  local name="$1" level="$2" gfx="$scratch/$1" socket="$1.sock"
  local i ready map tick start_tick capture_tick params queued armed clip_id result recorded
  serve_log="$scratch/$name.log"
  mkdir "$gfx"
  echo "$name: starting $level at ${scale}x, $fps fps${size:+, $size output}"
  (
    cd "$scratch"
    unset H2_BOOT_GFX_SIZE
    if [[ -n "$size" ]]; then export H2_BOOT_GFX_SIZE="$size"; fi
    if ((fps > 30)); then export H2_RENDER_THREAD=1 H2_SINGLE_COPY=true; fi
    exec env H2_OPT="${H2_OPT:-2}" H2_LEVEL="$level" H2_RENDER_SCALE="$scale" H2_FPS="$fps" \
      H2_FEATURES="$features" \
      H2_BOOT_GFX="$gfx" H2_BOOT_GFX_EVERY=never H2_BOOT_GFX_LIGHTING_AB=0 \
      H2_BOOT_GFX_SHADOWS=0 H2_BOOT_GFX_BETWEEN=0 \
      "$h2" serve "$disc" --out "$image" --socket "$socket" \
      --data "$scratch/data-$name" --no-mappack --no-halo1
  ) >"$serve_log" 2>&1 &
  serve_pid=$!

  for ((i = 0; i < 600; i++)); do
    [[ -S "$scratch/$socket" ]] && break
    kill -0 "$serve_pid" 2>/dev/null || fail "$name: serve exited before it opened the agent socket"
    sleep 1
  done
  [[ -S "$scratch/$socket" ]] || fail "$name: timed out waiting for the agent socket"

  ready="$(agent_wait "$socket" '{"phase":"gameplay","map_ticks":30}')"
  map="$(jq -er '.at.map' <<< "$ready")"
  agent_wait "$socket" "$(jq -cn --arg map "$map" --argjson ticks "$settle_ticks" \
    '{phase:"gameplay", $map, $ticks}')" >/dev/null
  agent "$socket" cheat.set '{"name":"hide_hud","value":true}' >/dev/null
  tick="$(agent "$socket" status | jq -er '.at.tick')"
  start_tick=$((tick + 60))
  capture_tick="$start_tick"
  # Interpolation starts at the previous tick's pose.
  if ((fps > 30)); then capture_tick=$((start_tick + 1)); fi
  armed="$(agent "$socket" render.capture_clip "$(jq -cn --argjson tick "$capture_tick" \
    --argjson ticks "$clip_ticks" --argjson fps "$fps" '{$tick, $ticks, $fps}')")"
  clip_id="$(jq -ce '{id}' <<< "$armed")"

  # One stick sample per game tick. The cosine ramps occupy the first and
  # last second, with a steady pan and walk in between.
  params="$(jq -cn --argjson start "$start_tick" --argjson count "$clip_ticks" '
    {at_tick:$start, steps:[range($count) as $i |
      ([$i, $count - 1 - $i, 30] | min) as $ramp |
      ((1 - (($ramp / 30 * 3.141592653589793) | cos)) / 2) as $ease |
      {left:[0, $ease * 0.34], right:[$ease * 0.32, 0], ticks:1}]}')"
  queued="$(agent "$socket" input.pad "$params")"
  jq -e --argjson count "$clip_ticks" '.open and .queued == $count' <<< "$queued" >/dev/null \
    || fail "$name: player one's pad is not open or the move was not queued"

  # The renderer permits two guest frames in flight. Give the last
  # capture and its replays time to drain before querying the PNG writer.
  agent_wait "$socket" "$(jq -cn --arg map "$map" \
    --argjson tick "$((capture_tick + clip_ticks + 3))" '{phase:"gameplay", $map, $tick}')" \
    >/dev/null
  if ! result="$(agent "$socket" render.capture_clip.status "$clip_id")" ||
     ! jq -e --argjson count "$clip_frames" '.written == $count and .frames == $count' \
       <<< "$result" >/dev/null; then
    agent "$socket" render.stats >&2 || true
    fail "$name: expected $clip_frames display frames at $fps fps"
  fi
  recorded="$(jq -er '.path' <<< "$result")"
  stop_serve

  clip_dir="$dest/$name"
  staging="$(mktemp -d "$dest/.$name.XXXXXX")"
  mv "$recorded" "$staging/clip"
  jq -n --argjson fps "$fps" '{$fps}' > "$staging/clip/capture.json"
  # Replace the clip only once all of its frames are ready to publish.
  if [[ -e "$clip_dir" ]]; then mv "$clip_dir" "$staging/previous"; fi
  mv "$staging/clip" "$clip_dir"
  rm -rf "$staging"
  staging=""
  rm -rf "$gfx" "$scratch/data-$name"
  echo "wrote $dest/$name ($clip_frames frames at $fps fps)"
}

for ((i = 0; i < ${#names[@]}; i++)); do
  if [[ -z "$only" || ",$only," == *",${names[i]},"* ]]; then
    capture "${names[i]}" "${levels[i]}"
  fi
done
