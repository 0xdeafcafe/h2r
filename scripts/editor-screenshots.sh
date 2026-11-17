#!/usr/bin/env bash
# Renders the editor guide's screenshots (docs/editor/editor-guide.md) with
# h2-editor's headless --screenshot mode against your own disc image, and
# writes them to docs/images/editor/. No window opens and the game does not
# run.
#
#   scripts/editor-screenshots.sh                  use disc/halo2.iso
#   scripts/editor-screenshots.sh --disc <iso>     use another image
#   scripts/editor-screenshots.sh --only <name>    render one shot
#   scripts/editor-screenshots.sh --out <dir>      write somewhere else
#
# Every shot renders at 1600x1000 points at scale 2 (3200x2000 pixels).
# Whole-window shots and crops of the 3D view become JPEGs (macOS `sips`,
# quality 82, at most 1600 wide); crops of panels, menus and tooltips stay
# PNG at full resolution so their text reads. The steps (--do), crops
# (--crop) and numbered callouts (--callout) are h2-editor's own flags;
# crates/studio/src/script.rs describes the targets. Run a shot by hand
# with H2_EDITOR_PROBE=1 to list the windows and widgets it can aim at.
# Variants and packages are saved under a scratch data root, never your own.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

usage() {
  cat <<'EOF'
Usage: scripts/editor-screenshots.sh [--disc <iso>] [--only <name>] [--out <dir>]

  --disc <iso>    the disc image (default: $H2_DISC, else disc/halo2.iso)
  --only <name>   render only this shot (e.g. scenario-weapon); several,
                  comma-separated, share one scratch data root
                  (halo1-converted,halo1-overlay)
  --out <dir>     where to write (default: docs/images/editor)
  -h, --help      show this help
EOF
}

disc="${H2_DISC:-$root/disc/halo2.iso}"
only=""
out="$root/docs/images/editor"
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
    --out)
      out="$2"
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
disc="$(cd "$(dirname "$disc")" && pwd)/$(basename "$disc")"
if ! command -v sips >/dev/null; then
  echo "needs macOS sips to scale and compress the shots" >&2
  exit 1
fi

mkdir -p "$out"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/editor-screenshots.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

cargo build --release -p studio
bin="${CARGO_TARGET_DIR:-$root/target}/release/h2-editor"

# Run from the scratch folder with the discs linked into it, so the paths
# the shots show are short (disc/halo2.iso, data/maps).
mkdir -p "$scratch/disc"
ln -s "$disc" "$scratch/disc/halo2.iso"
render() {
  local png
  png="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
  (cd "$scratch" && "$bin" --disc disc/halo2.iso --data data --size 1600x1000 --scale 2 \
    --screenshot "$png" "${@:2}" >/dev/null)
}

wanted() {
  [[ -z "$only" || ",$only," == *",$1,"* ]]
}

# shot <name> <h2-editor flags...>: the whole window, or a crop of the 3D
# view, as a JPEG at most 1600 wide.
shot() {
  local name="$1"
  shift
  wanted "$name" || return 0
  local png="$scratch/$name.png"
  render "$png" "$@"
  sips -s format jpeg -s formatOptions 82 -Z 1600 "$png" --out "$out/$name.jpg" >/dev/null
  echo "wrote $out/$name.jpg"
}

# detail <name> <h2-editor flags with --crop...>: a crop of panels, menus or
# tooltips, as a PNG at full resolution.
detail() {
  local name="$1"
  shift
  wanted "$name" || return 0
  render "$out/$name.png" "$@"
  echo "wrote $out/$name.png"
}

# The layout's regions, numbered.
regions=(
  --callout 'win:##MainMenuBar=1' --callout 'win:###tools=2' --callout 'win:###outliner=3'
  --callout 'win:###viewport=4' --callout 'win:###inspector=5' --callout 'win:###log=6'
  --callout 'win:##status-bar=7'
)
# Ascension with a weapon spawn selected and in view.
asc=(--pick ascension --select netgameequipment:0 --focus --camera 40,35,25)
# A menu and its title.
menu() { echo "--crop" "$1" "--crop" "win:##Menu"; }

# A package of Ascension in the scratch data root, for the shots that list
# or open packages.
render "$scratch/setup.png" --pick ascension --dialog package --do "click Save package"

## Starting and the layout

# The first run: no disc found.
shot first-run --no-disc --data empty
# A disc open: its maps listed, none picked yet.
detail maps-list --crop 'win:###tools'
# The window: a multiplayer map framed, nothing selected.
shot window --pick ascension
# The window's regions, numbered.
shot layout-regions "${asc[@]}" "${regions[@]}"
# Dragging the inspector's tab onto the outliner: the docking targets.
shot layout-drag "${asc[@]}" \
  --do 'move win:###inspector@60,10; down; move win:###outliner; wait 8'
# Dropped there, it is a tab beside the outliner.
shot layout-docked "${asc[@]}" \
  --do 'move win:###inspector@60,10; down; move win:###outliner; wait 8; up'
# View > Reset layout puts it back.
shot layout-reset "${asc[@]}" \
  --do 'move win:###inspector@60,10; down; move win:###outliner; wait 8; up; click View; hover Reset layout'
# The menus.
detail menu-file "${asc[@]}" --do 'click File; hover Save as package' $(menu File)
detail menu-edit "${asc[@]}" --nudge 0,0,0.5 --do 'click Edit; hover Allow growing full blocks' \
  $(menu Edit) --crop 'win:##Tooltip'
detail menu-view "${asc[@]}" --do 'click View; hover Rotate gizmo' $(menu View)

## Scenario

# The outliner: a category open, the filter, the context menu.
detail outliner-open --pick ascension --do 'click Netgame equipment' --crop 'win:###outliner'
detail outliner-filter --pick ascension \
  --do 'click Netgame equipment; click ##outliner-filter; type sniper' --crop 'win:###outliner'
detail outliner-menu --pick ascension \
  --do 'click Netgame equipment; rclick Netgame equipment@90,32' \
  --crop 'win:###outliner' --crop 'win:##Popup' --crop-height 420
detail outliner-add --pick ascension --do 'rclick Netgame equipment' \
  --crop 'win:###outliner' --crop 'win:##Popup' --crop-height 330
# Delete with nothing selected.
detail delete-refused --pick ascension --do 'hover Delete' \
  --crop 'win:###outliner' --crop 'win:##Tooltip' --crop-height 150
# The inspector for each kind of placement.
detail inspector-equipment "${asc[@]}" --crop 'win:###inspector' --crop-height 360
detail inspector-spawn --pick ascension --select startinglocations:0 --crop 'win:###inspector' --crop-height 330
detail inspector-flag --pick ascension --select netgameflags:0 --crop 'win:###inspector' --crop-height 300
detail inspector-vehicle --pick 03a_oldmombasa --select vehicles:0 --crop 'win:###inspector' --crop-height 330
detail inspector-scenery --pick ascension --select scenery:0 --crop 'win:###inspector' --crop-height 330
# A field's offset and evidence on hover.
detail inspector-field-help "${asc[@]}" --do 'hover ##NetgameEquipment-respawn time@-90,12' \
  --crop 'win:###inspector' --crop 'win:##Tooltip' --crop-height 300
# The gizmo, moving and rotating.
near=(--pick ascension --select netgameequipment:0 --focus --camera 40,30,10)
shot gizmo-move "${near[@]}" --crop 'win:###viewport'
shot gizmo-rotate "${near[@]}" --do 'click Rotate (E)' --crop 'win:###viewport'
# The same weapon spawn after a move of two units (gizmo-move is before).
shot move-after "${near[@]}" --nudge 2,0,0 --crop 'win:###viewport'
# Undo in the Edit menu, then the log after it.
detail undo-menu "${asc[@]}" --nudge 2,0,0 --do 'click Edit; hover Undo' $(menu Edit)
detail undo-log "${asc[@]}" --nudge 2,0,0 --do 'click Edit; click Undo' \
  --crop 'win:###log' --crop-height 100
# Save as package, and Open package once one is saved.
detail package-form --pick ascension --dialog package --crop 'win:###save-package'
detail package-open --pick ascension --do 'click File; click Open package' --crop 'win:###open-package'
# Unsaved changes, asked on quit.
shot unsaved-prompt "${asc[@]}" --dialog unsaved
# A weapon spawn selected, with the move gizmo.
shot scenario-weapon --pick coagulation --select netgameequipment:3 --focus --camera 40,30,22
# More maps.
shot scenario-lockout --pick lockout --select netgameequipment:0 --focus --camera 40,35,25
shot scenario-zanzibar --pick zanzibar --select vehicles:0 --focus --camera 220,25,30
shot scenario-midship --pick midship --select netgameflags:0 --focus --camera 30,30,30
shot scenario-headlong --pick headlong --select vehicles:0 --focus --camera 140,30,30
shot scenario-beavercreek --pick beavercreek --select netgameequipment:0 --focus --camera 60,35,28

## Packages and Halo 1

h1="${H1_ISO:-$(dirname "$disc")/halo1.iso}"
if [[ -f "$h1" ]]; then
  ln -s "$(cd "$(dirname "$h1")" && pwd)/$(basename "$h1")" "$scratch/disc/halo1.iso"
  h1=disc/halo1.iso
  # The Maps list's Packages and Halo 1 groups.
  detail maps-groups --halo1 "$h1" --size 1600x2400 --do 'move win:###tools; wheel -8' --crop 'win:###tools'
  # A Halo 1 map, read-only.
  shot halo1-map --halo1 "$h1" --halo1-map bloodgulch --select startinglocations:0 --focus --camera 40,35,40
  detail halo1-banner --halo1 "$h1" --halo1-map bloodgulch --crop 'win:###outliner' --crop-height 260
  # Blood Gulch converted to Halo 2 (written into the scratch data root),
  # then the same view with its Halo 1 source over it.
  shot halo1-converted --halo1 "$h1" --halo1-map bloodgulch --convert h2 --overlay off \
    --select startinglocations:0 --focus --camera 40,35,40
  shot halo1-overlay --halo1 "$h1" --package 'h1_bloodgulch#H2' --overlay on \
    --select startinglocations:0 --focus --camera 40,35,40
  detail halo1-compare --halo1 "$h1" --package 'h1_bloodgulch#H2' --select startinglocations:0 \
    --crop 'win:###inspector' --crop-height 120
  # Convert to Halo 2: the controls, a conversion running, and a refusal.
  detail convert-controls --halo1 "$h1" --halo1-map bloodgulch --crop 'win:###inspector' --crop-height 300
  detail convert-running --halo1 "$h1" --halo1-map bloodgulch --do 'click Convert to Halo 2; move 1500,900; sleep 150' \
    --crop 'win:###inspector' --crop-height 300
  detail convert-error --halo1 "$h1" --halo1-map bloodgulch \
    --do 'click Halo 1 (its own; click Convert to Halo 2; move 1500,900; sleep 3000' --crop 'win:###inspector' --crop-height 330
fi

## Game variants

# The list, a built-in, and a copy.
detail variants-list --tool variants --variant 3 --crop 'win:###outliner'
shot variants-builtin --tool variants --variant 3
detail variants-copy --tool variants --variant 3 --crop 'win:###viewport' --crop-height 110
shot variants-edit --tool variants --variant 3 --copy-variant "Swords Night"
# Each settings page on its own.
page() { detail "variants-page-$1" --tool variants --variant "$2" --page "$3" --crop "$3" --crop-height "$4"; }
page match 3 'Match options' 172
page slayer 3 'Slayer options' 110
page player 3 'Player options' 300
page weapons 3 'Weapon options' 270
page vehicles 3 'Vehicle options' 240
page teams 3 'Team options' 260
page oddball 9 'Oddball options' 270
page king 14 'King of the Hill options' 220
page juggernaut 19 'Juggernaut options' 270
page ctf 21 'Capture the Flag options' 255
page assault 24 'Assault options' 290
page territories 28 'Territories options' 170
# A setting's help on hover.
detail variants-help --tool variants --variant 3 --do 'hover ##setting-2@-200,12' \
  --crop 'Match options' --crop '##setting-12' --crop 'win:##Tooltip'
# Two settings changed on a copy: the changes summary.
changed=(--tool variants --variant 3 --copy-variant "Swords Night"
  --do 'click ##setting-2; click win:##Combo@60,80; click ##setting-17; click win:##Combo@60,14')
detail variants-changes "${changed[@]}" --crop 'win:###inspector' --crop-height 330
# Renamed, then saved: it joins the Saved list.
detail variants-rename --tool variants --variant 3 --copy-variant "Swords Night" \
  --do 'click ##variant-name; key end; key backspace*20; type Lockout Swords' --crop 'win:###viewport' --crop-height 110
detail variants-saved "${changed[@]}" --do 'click Save' --crop 'win:###outliner' --crop-height 200
# Export's hover help, and pasting a portable variant.
detail variants-export --tool variants --variant 3 --copy-variant "Swords Night" --do 'hover Export...' \
  --crop 'win:###inspector' --crop 'win:##Tooltip' --crop-height 160
detail variants-import --tool variants --variant 3 --do 'click Paste a portable variant' \
  --crop 'win:###inspector' --crop-height 470

## Playlists

detail hopper-list --tool playlists --hopper default --playlist 2 --crop 'win:###outliner'
shot hopper-playlist --hopper default --playlist 2
detail hopper-entries --hopper default --playlist 2 --crop 'win:###viewport' --crop-height 372
# The map and variant pickers open.
detail hopper-map-picker --hopper default --playlist 2 --do 'click ##map#1' \
  --crop 'win:###viewport' --crop 'win:##Combo' --crop-height 300
detail hopper-variant-picker --hopper default --playlist 2 --do 'click ##variant#0' \
  --crop 'win:###viewport' --crop 'win:##Combo' --crop-height 300
# Edit hands the entry's variant to the variant tool as a copy.
shot hopper-edit --hopper default --playlist 2 --do 'click ###viewport>Edit#0'
# A preset, the raw fields, and a validation error.
shot hopper-preset --hopper default --playlist 1 --preset 0
detail hopper-preset-panel --hopper default --playlist 1 --preset 0 --crop 'win:###inspector' --crop-height 560
detail hopper-raw --hopper default --playlist 2 --do 'click Advanced: raw fields; move win:###inspector; wheel -20' \
  --crop 'win:###inspector'
detail hopper-error --hopper default --playlist 2 --do 'click ##hopper-title; key end; key backspace*40' \
  --crop 'win:###viewport' --crop-height 430
detail hopper-file-menu --tool playlists --hopper default --playlist 2 --do 'click File; hover Save as' $(menu File)
