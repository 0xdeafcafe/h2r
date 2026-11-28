#!/usr/bin/env bash
# Renders the README's gameplay screenshots (docs/images/readme/) from your
# own disc image. For each level it starts a headless `h2 serve` that writes
# frames off-screen (H2_BOOT_GFX), waits for gameplay through the agent API,
# lets the level settle, keeps one frame and converts it to a JPEG (macOS
# `sips`). No window opens. It never builds: pass the image you built.
#
#   scripts/readme-screenshots.sh --out-image <image dir> [--disc <iso>]
#       [--only <name>[,<name>]] [--dest <dir>] [--settle <seconds>]
#
# Shots (name = file name without .jpg; level = H2_LEVEL):
#   oldmombasa-4x      03a_oldmombasa
#   deltaapproach-4x   05a_deltaapproach
#   ascension-4x       ascension
#   clankers-4x        ascension, a Clanker plays the local seat
#   firefight-4x       the Firefight probe map on Coagulation, Elites vs a Clanker
#   forge-4x           the Forge monitor over Ascension
#   siege-4x           the Siege script on Zanzibar, a Clanker attacking
#   superbounce-4x     the Superbounce script on Ascension, a bounce under way
# The last five are staged by scripts/readme-scenes.sh (its header says how
# each starts); they need jq and `h2 rules build`, and keep the HUD on where
# it is the point (forge, siege, superbounce).
#
# Every shot renders at H2_RENDER_SCALE=4, with H2_OPT=2 (it must match the
# image's build, or the image recompiles), and passes H2_FLAT_RAM through if
# you set it. Use `h2` from $H2 or $PATH (cargo install --force from master).
# The frames come from a scratch folder that is deleted on exit. The JPEGs
# supply fallback art to scripts/readme-art.sh when a level has no clip
# from scripts/readme-video.sh. Commit documentation shots only under
# docs/images/** (Git LFS), as described in CONTRIBUTING.md.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

usage() {
  cat <<'EOT'
Usage: scripts/readme-screenshots.sh --out-image <dir> [--disc <iso>] [--only <names>]
                                     [--dest <dir>] [--settle <seconds>]

  --out-image <dir>  the image you built (what `h2 serve --out` takes); required
  --disc <iso>       the disc image (default: $H2_DISC, else disc/halo2.iso)
  --only <names>     comma-separated shots (default: all)
  --dest <dir>       where to write (default: docs/images/readme)
  --settle <s>       seconds of gameplay before the frame is taken (default 20)
  -h, --help         show this help
EOT
}

disc="${H2_DISC:-$root/disc/halo2.iso}"
image=""
only=""
dest="$root/docs/images/readme"
settle=20
while (($# > 0)); do
  case "$1" in
    --out-image) image="$2"; shift 2 ;;
    --disc) disc="$2"; shift 2 ;;
    --only) only="$2"; shift 2 ;;
    --dest) dest="$2"; shift 2 ;;
    --settle) settle="$2"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

if [[ -z "$image" || ! -d "$image" ]]; then
  echo "pass the built image with --out-image <dir>" >&2
  exit 1
fi
if [[ ! -f "$disc" ]]; then
  echo "no disc image at $disc (set H2_DISC or pass --disc)" >&2
  exit 1
fi
disc="$(cd "$(dirname "$disc")" && pwd)/$(basename "$disc")"
image="$(cd "$image" && pwd)"
command -v sips >/dev/null || { echo "needs macOS sips to convert the frames" >&2; exit 1; }
h2="${H2:-h2}"
command -v "$h2" >/dev/null || { echo "no h2 on PATH (set H2 or cargo install)" >&2; exit 1; }

mkdir -p "$dest"
# Short names: the agent socket path must fit in SUN_LEN (104 bytes).
scratch="$(mktemp -d "${TMPDIR:-/tmp}/rs.XXXXXX")"
serve_pid=""
cleanup() {
  [[ -n "$serve_pid" ]] && kill "$serve_pid" 2>/dev/null || true
  rm -rf "$scratch"
}
trap cleanup EXIT

. "$root/scripts/readme-scenes.sh"
command -v jq >/dev/null || { echo "needs jq (brew install jq)" >&2; exit 1; }

# The scene helpers' view of the agent API.
agent() { "$h2" agent --socket "$1" "${@:2}"; }
agent_wait() { "$h2" agent --socket "$1" wait "$2" 600000; }

wanted() {
  [[ -z "$only" || ",$only," == *",$1,"* ]]
}

# shot <name> <level> [prep act hud]
shot() {
  local name="$1" level="$2" prep="${3:-}" act="${4:-}" hud="${5:-0}" nav=1
  # A scene that builds a map (the Firefight probe) keeps it out of your data.
  local data_args=()
  [[ -z "$prep" ]] || data_args=(--data "$scratch/data-$name")
  wanted "$name" || return 0
  local gfx="$scratch/$name" sock="$scratch/$name.sock"
  mkdir -p "$gfx"
  echo "$name: starting $level at 4x"
  env H2_OPT=2 H2_LEVEL="$level" H2_RENDER_SCALE=4 \
    H2_BOTS_NAV_AUTO="$nav" \
    H2_BOOT_GFX="$gfx" H2_BOOT_GFX_EVERY=30 ${H2_FLAT_RAM:+H2_FLAT_RAM="$H2_FLAT_RAM"} \
    "$h2" serve "$disc" --out "$image" --socket "$sock" ${data_args[@]+"${data_args[@]}"} >"$scratch/$name.log" 2>&1 &
  serve_pid=$!
  # The socket appears once the serve is listening.
  local i
  for ((i = 0; i < 600; i++)); do
    [[ -S "$sock" ]] && break
    kill -0 "$serve_pid" 2>/dev/null || { tail -20 "$scratch/$name.log" >&2; echo "$name: serve exited" >&2; exit 1; }
    sleep 1
  done
  "$h2" agent wait '{"phase":"gameplay","map_ticks":30}' 600000 --socket "$sock" >/dev/null
  if [[ -n "$prep" ]]; then "prep_$prep" "$sock"; fi
  sleep "$settle"
  if [[ -n "$prep$act" && "$hud" != 1 ]]; then agent "$sock" cheat.set '{"name":"hide_hud","value":true}' >/dev/null; fi
  # What the scene stages starts here; a few seconds later the frame shows it.
  if [[ -n "$act" ]]; then "act_$act" "$sock"; sleep 4; fi
  # The newest frame may still be written; take the one before it.
  local frame
  frame="$(ls "$gfx"/frame-*.png | sort | tail -2 | head -1)"
  sips -s format jpeg -s formatOptions 85 "$frame" --out "$dest/$name.jpg" >/dev/null
  echo "wrote $dest/$name.jpg (from $(basename "$frame"))"
  kill "$serve_pid" 2>/dev/null || true
  wait "$serve_pid" 2>/dev/null || true
  serve_pid=""
}

shot oldmombasa-4x 03a_oldmombasa
shot deltaapproach-4x 05a_deltaapproach
shot ascension-4x ascension
for ((n = 0; n < ${#scene_names[@]}; n++)); do
  shot "${scene_names[n]}-4x" "${scene_levels[n]}" "${scene_preps[n]}" "${scene_acts[n]}" "${scene_huds[n]}"
done
