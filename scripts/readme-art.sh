#!/usr/bin/env bash
# Builds the README's header banner and montage from the gameplay shots in
# docs/images/readme/*.jpg (rendered by scripts/readme-screenshots.sh).
#
#   scripts/readme-art.sh [--src <dir>] [--dest <dir>] [--header <n>]
#
# header.webp   a 1600x500 banner: the first <n> shots (default 3, by file
#               name) side by side, each crossfading into the next over a
#               soft gradient rather than a hard edge.
# montage.webp  an animated WebP, 960x720, that holds each shot and
#               crossfades to the next, looping back to the first. Every
#               .jpg in the folder is in it, so new shots join on a rerun.
#               Each shot adds about 300 KB; keep it under about 3 MB.
#
# Needs ffmpeg, cwebp and img2webp (Homebrew: ffmpeg, webp). Reads only the
# JPEGs; writes the two WebPs. Both live under docs/images/** in Git LFS:
# see CONTRIBUTING.md.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/docs/images/readme"
dest=""
header_n=3

usage() {
  sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
}

while (($# > 0)); do
  case "$1" in
    --src) src="$2"; shift 2 ;;
    --dest) dest="$2"; shift 2 ;;
    --header) header_n="$2"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
dest="${dest:-$src}"

for tool in ffmpeg cwebp img2webp; do
  command -v "$tool" >/dev/null || { echo "needs $tool (brew install ffmpeg webp)" >&2; exit 1; }
done

shots=()
while IFS= read -r f; do shots+=("$f"); done < <(find "$src" -maxdepth 1 -name '*.jpg' | LC_ALL=C sort)
n=${#shots[@]}
if ((n == 0)); then
  echo "no .jpg shots in $src (render them with scripts/readme-screenshots.sh)" >&2
  exit 1
fi
echo "$n shots: $(for s in "${shots[@]}"; do basename "$s" .jpg; done | paste -sd ' ' -)"

scratch="$(mktemp -d "${TMPDIR:-/tmp}/readme-art.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$dest"
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }

# --- Header ----------------------------------------------------------------
W=1600 H=500 FADE=400
hn=$((header_n < n ? header_n : n))
# Panels overlap by FADE pixels; panel i starts at x_i and is P wide.
P=$(((W + (hn - 1) * FADE + hn - 1) / hn))
inputs=() filter=""
for ((i = 0; i < hn; i++)); do
  inputs+=(-i "${shots[i]}")
  # Cut the panel from the band between the HUD's top row (above 17% of the
  # height) and the motion tracker (below 72%), centred, then scale it.
  chain="[$i:v]crop=w='ih*0.55*$P/$H':h='ih*0.55':x='(iw-ow)/2':y='ih*0.17'"
  chain+=",scale=$P:$H:flags=lanczos,format=rgba"
  if ((i > 0)); then
    # Smoothstep alpha over the left FADE pixels: the soft seam.
    chain+=",geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='255*if(lt(X,$FADE),(X/$FADE)*(X/$FADE)*(3-2*X/$FADE),1)'"
  fi
  filter+="$chain[p$i];"
done
filter+="color=c=black:s=${W}x${H},format=rgba[c0];"
for ((i = 0; i < hn; i++)); do
  x=0
  ((hn > 1)) && x=$((i * (W - P) / (hn - 1)))
  filter+="[c$i][p$i]overlay=x=$x:y=0[c$((i + 1))];"
done
filter+="[c$hn]format=rgb24"
ff "${inputs[@]}" -filter_complex "$filter" -frames:v 1 "$scratch/header.png"
cwebp -quiet -q 82 -m 6 "$scratch/header.png" -o "$dest/header.webp"

# --- Montage ---------------------------------------------------------------
MW=960 MH=720 HOLD_MS=2600 STEP_MS=70 STEPS=12 Q=80
for ((i = 0; i < n; i++)); do
  ff -i "${shots[i]}" -vf "scale=$MW:$MH:flags=lanczos" "$scratch/s$i.png"
done
args=(-loop 0 -lossy -q "$Q" -m 6)
for ((i = 0; i < n; i++)); do
  next=$(((i + 1) % n))
  args+=(-d "$HOLD_MS" "$scratch/s$i.png")
  ((n > 1)) || break
  for ((k = 1; k < STEPS; k++)); do
    f="$scratch/f${i}_$k.png"
    ff -i "$scratch/s$i.png" -i "$scratch/s$next.png" \
      -filter_complex "[1:v][0:v]blend=all_expr='A*$k/$STEPS+B*(1-$k/$STEPS)'" -frames:v 1 "$f"
    args+=(-d "$STEP_MS" "$f")
  done
done
img2webp "${args[@]}" -o "$dest/montage.webp" >/dev/null 2>&1

for out in header montage; do
  printf '%s.webp  %s bytes\n' "$out" "$(wc -c <"$dest/$out.webp" | tr -d ' ')"
done
