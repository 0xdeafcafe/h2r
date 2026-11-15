#!/bin/sh
# Run from any directory. Requires bun and Playwright Chromium.
# Optional first argument overrides the screenshot output directory.
set -eu
DESIGN_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
DESIGN_SHOTS=${1:-/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc/design-shots}
mkdir -p "$DESIGN_SHOTS"
for page in home game players player events; do
  bunx playwright screenshot --browser chromium --viewport-size '1440,1000' --wait-for-timeout 1200 --full-page "file://$DESIGN_DIR/$page.html" "$DESIGN_SHOTS/$page-desktop.png"
  bunx playwright screenshot --browser chromium --viewport-size '390,844' --wait-for-timeout 1200 --full-page "file://$DESIGN_DIR/$page.html" "$DESIGN_SHOTS/$page-phone.png"
done
