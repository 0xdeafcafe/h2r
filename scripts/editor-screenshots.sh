#!/usr/bin/env bash
# Renders the editor guide's screenshots (docs/editor/editor-guide.md) with
# h2-editor's headless --screenshot mode against your own disc image, and
# writes them to docs/images/editor/ as JPEGs 1600 pixels wide. No window
# opens and the game does not run.
#
#   scripts/editor-screenshots.sh                  use disc/halo2.iso
#   scripts/editor-screenshots.sh --disc <iso>     use another image
#   scripts/editor-screenshots.sh --only <name>    render one shot
#
# Each shot renders at 1600x1000 points at scale 2 (3200x2000 pixels), then
# macOS `sips` scales it to 1600 wide at JPEG quality 82. Variants and
# packages are saved under a scratch data root, never your own.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

usage() {
  cat <<'EOF'
Usage: scripts/editor-screenshots.sh [--disc <iso>] [--only <name>]

  --disc <iso>    the disc image (default: $H2_DISC, else disc/halo2.iso)
  --only <name>   render only this shot (e.g. scenario-weapon)
  -h, --help      show this help
EOF
}

disc="${H2_DISC:-$root/disc/halo2.iso}"
only=""
while (($# > 0)); do
  case "$1" in
    --disc)
      disc="$2"
      shift 2
      ;;
    --only)
      only="$2"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! -f "$disc" ]]; then
  echo "no disc image at $disc (set H2_DISC or pass --disc)" >&2
  exit 1
fi
if ! command -v sips >/dev/null; then
  echo "needs macOS sips to scale and compress the shots" >&2
  exit 1
fi

out="$root/docs/images/editor"
mkdir -p "$out"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/editor-screenshots.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

cargo build --release -p studio
bin="${CARGO_TARGET_DIR:-$root/target}/release/h2-editor"

# shot <name> <h2-editor flags...>
shot() {
  local name="$1"
  shift
  if [[ -n "$only" && "$only" != "$name" ]]; then
    return
  fi
  local png="$scratch/$name.png"
  "$bin" --disc "$disc" --data "$scratch/data" --size 1600x1000 --scale 2 \
    --screenshot "$png" "$@" >/dev/null
  sips -s format jpeg -s formatOptions 82 -Z 1600 "$png" --out "$out/$name.jpg" >/dev/null
  echo "wrote docs/images/editor/$name.jpg"
}

# The window: a multiplayer map framed, nothing selected.
shot window --pick ascension
# A weapon spawn selected, with the move gizmo.
shot scenario-weapon --pick coagulation --select netgameequipment:3 --focus --camera 40,30,22
# Another map: Lockout's battle rifle spawn among its towers.
shot scenario-lockout --pick lockout --select netgameequipment:0 --focus --camera 40,35,25
# Save as package.
shot scenario-package --pick ascension --dialog package
# Unsaved changes, asked on quit.
shot unsaved-prompt --pick ascension --select netgameequipment:0 --focus --camera 40,35,25 --dialog unsaved
# A built-in game variant, read-only.
shot variants-builtin --tool variants --variant 3
# An editable copy of it.
shot variants-edit --tool variants --variant 3 --copy-variant "Swords Night"
# The playlists: the server's default lineup.
shot hopper-playlist --hopper default --playlist 2
# A historical preset resolved against the disc.
shot hopper-preset --hopper default --playlist 1 --preset 0
# A Halo 1 map, read-only, when a Halo 1 disc is at hand ($H1_ISO, else
# disc/halo1.iso next to the Halo 2 one). Skipped without one.
h1="${H1_ISO:-$(dirname "$disc")/halo1.iso}"
if [[ -f "$h1" ]]; then
  shot halo1-map --halo1 "$h1" --halo1-map bloodgulch --select startinglocations:0 --focus --camera 40,35,40
fi
