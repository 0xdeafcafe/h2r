#!/usr/bin/env bash
# Captures six seconds of HUD-less gameplay per level from your own disc.
# h2 serve renders off-screen into scratch, and the agent API waits for
# gameplay, hides the HUD and queues a timed controller move. No window opens.
#
#   scripts/readme-video.sh --out-image <dir> [--disc <iso>]
#       [--only <name>[,<name>]] [--dest <dir>] [--settle <seconds>] [--scale <2-6>]
#
# The image must be built with the same H2_OPT (default 2). H2_RENDER_SCALE defaults to 2 (1280x960).
# Requires h2 (or $H2), jq and a GPU. Pass --dest to keep the frame folders
# somewhere else, then pass that directory to scripts/readme-art.sh --frames.
# Commit only the final documentation media under docs/images/** (Git LFS).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
names=(oldmombasa deltaapproach ascension)
levels=(03a_oldmombasa 05a_deltaapproach ascension)
clip_frames=180

usage() {
  cat <<'EOT'
Usage: scripts/readme-video.sh --out-image <dir> [--disc <iso>] [--only <names>]
                               [--dest <dir>] [--settle <seconds>] [--scale <2-6>]

  --out-image <dir>  prebuilt H2_OPT=2 game image, required
  --disc <iso>       your disc image (default: $H2_DISC, else disc/halo2.iso)
  --only <names>     comma-separated names (oldmombasa, deltaapproach, ascension)
  --dest <dir>       where to save frame folders (default: docs/images/readme/frames)
  --settle <s>       gameplay seconds before the move (default: 20)
  --scale <2-6>      capture render scale (default: $H2_RENDER_SCALE or 2)
EOT
}

fail() { echo "$*" >&2; exit 1; }

image=""
disc="${H2_DISC:-$root/disc/halo2.iso}"
only=""
dest="$root/docs/images/readme/frames"
settle=20
scale="${H2_RENDER_SCALE:-2}"
while (($# > 0)); do
  case "$1" in
    --out-image) image="${2:?--out-image needs a directory}"; shift 2 ;;
    --disc) disc="${2:?--disc needs an ISO path}"; shift 2 ;;
    --only) only="${2:?--only needs a list of levels}"; shift 2 ;;
    --dest) dest="${2:?--dest needs a directory}"; shift 2 ;;
    --settle) settle="${2:?--settle needs seconds}"; shift 2 ;;
    --scale) scale="${2:?--scale needs a render scale}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

[[ -n "$image" && -d "$image" ]] || fail "pass the built image with --out-image <dir>"
[[ -f "$disc" ]] || fail "no disc image at $disc (set H2_DISC or pass --disc)"
[[ "$scale" =~ ^[2-6]$ ]] || fail "--scale must be an integer from 2 to 6"
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

mkdir -p "$dest"
# Disk-backed PNGs are large. Keep the boot and pre-roll files in scratch.
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
  (cd "$scratch" && "$h2" agent --socket "$1" --timeout 610 "${@:2}")
}

capture() {
  local name="$1" level="$2" gfx="$scratch/$1" socket="$1.sock"
  local i ready map tick start_tick params queued lead present first end following frame
  serve_log="$scratch/$name.log"
  mkdir "$gfx"
  echo "$name: starting $level at ${scale}x"
  (
    cd "$scratch"
    exec env H2_OPT="${H2_OPT:-2}" H2_LEVEL="$level" H2_RENDER_SCALE="$scale" H2_FPS=30 \
      H2_BOOT_GFX="$gfx" H2_BOOT_GFX_EVERY=1 H2_BOOT_GFX_LIGHTING_AB=0 \
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

  ready="$(agent "$socket" wait '{"phase":"gameplay","map_ticks":30}' 600000)"
  map="$(jq -er '.at.map' <<< "$ready")"
  agent "$socket" wait "$(jq -cn --arg map "$map" --argjson ticks "$settle_ticks" \
    '{phase:"gameplay", $map, $ticks}')" 600000 >/dev/null
  agent "$socket" cheat.set '{"name":"hide_hud","value":true}' >/dev/null
  tick="$(agent "$socket" status | jq -er '.at.tick')"
  start_tick=$((tick + 30))

  # One stick sample per game tick. The cosine ramps occupy the first and
  # last second, with a steady pan and walk in between.
  params="$(jq -cn --argjson start "$start_tick" --argjson count "$clip_frames" '
    {at_tick:$start, steps:[range($count) as $i |
      ([$i, $count - 1 - $i, 30] | min) as $ramp |
      ((1 - (($ramp / 30 * 3.141592653589793) | cos)) / 2) as $ease |
      {left:[0, $ease * 0.34], right:[$ease * 0.32, 0], ticks:1}]}')"
  queued="$(agent "$socket" input.pad "$params")"
  jq -e --argjson count "$clip_frames" '.open and .queued == $count' <<< "$queued" >/dev/null \
    || fail "$name: player one's pad is not open or the move was not queued"

  # Start after the present reported at the scheduled tick. Wait for one
  # extra file so the final PNG is closed even when rendering trails the game.
  lead="$(agent "$socket" wait "$(jq -cn --arg map "$map" --argjson tick "$start_tick" \
    '{phase:"gameplay", $map, $tick}')" 600000)"
  [[ "$(jq -er '.at.tick' <<< "$lead")" == "$start_tick" ]] \
    || fail "$name: missed the first tick of the camera move"
  present="$(jq -er '.at.present' <<< "$lead")"
  first=$((present + 1))
  end=$((first + clip_frames))
  agent "$socket" wait "$(jq -cn --arg map "$map" --argjson present "$end" \
    '{phase:"gameplay", $map, $present}')" 600000 >/dev/null
  printf -v following '%s/frame-%05d.png' "$gfx" "$end"
  for ((i = 0; i < 120; i++)); do
    [[ -s "$following" ]] && break
    kill -0 "$serve_pid" 2>/dev/null || fail "$name: serve exited during capture"
    sleep 1
  done
  [[ -s "$following" ]] || fail "$name: timed out waiting for $following"
  stop_serve

  clip_dir="$dest/$name"
  staging="$(mktemp -d "$dest/.$name.XXXXXX")"
  mkdir "$staging/clip"
  for ((i = first; i < end; i++)); do
    printf -v frame 'frame-%05d.png' "$i"
    [[ -s "$gfx/$frame" ]] || fail "$name: missing $frame"
    mv "$gfx/$frame" "$staging/clip/"
  done
  # Replace the clip only once all of its frames are ready to publish.
  if [[ -e "$clip_dir" ]]; then mv "$clip_dir" "$staging/previous"; fi
  mv "$staging/clip" "$clip_dir"
  rm -rf "$staging"
  staging=""
  rm -rf "$gfx" "$scratch/data-$name"
  echo "wrote $dest/$name ($clip_frames frames at 30 fps)"
}

for ((i = 0; i < ${#names[@]}; i++)); do
  if [[ -z "$only" || ",$only," == *",${names[i]},"* ]]; then
    capture "${names[i]}" "${levels[i]}"
  fi
done
