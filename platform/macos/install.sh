#!/usr/bin/env bash
# Package the already-installed desktop CLI and register it with Launch Services.
# The just recipe supplies the absolute path to the user's disc.
set -euo pipefail
disc=${1:?pass the absolute path to your disc}
cli=$(command -v h2) || { echo "Install the CLI with cargo install --locked --path crates/apps/cli." >&2; exit 1; }
[[ $disc = /* && -f "$disc" ]] || { echo "Can't find the disc at $disc." >&2; exit 1; }
root=$(cd "$(dirname "$0")/../.." && pwd -P)
app="$HOME/Applications/Halo 2.app"
if [[ -e "$app" ]]; then
    installed=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist" 2>/dev/null || true)
    [[ "$installed" = red.forbes.h2recomp.mac ]] || { echo "Another app is installed at $app." >&2; exit 1; }
fi

stage=$(mktemp -d "${TMPDIR:?}/h2-mac-app.XXXXXXXX")
trap 'rm -rf "$stage"' EXIT
bundle="$stage/Halo 2.app"
contents="$bundle/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources"
install -m 755 "$cli" "$contents/MacOS/Halo2"
install -m 644 "$root/platform/macos/Info.plist" "$contents/Info.plist"
printf '%s' "$disc" > "$contents/Resources/disc-path"
# The iconset name identifies the 512px representation to iconutil.
mkdir "$stage/AppIcon.iconset"
cp "$root/crates/apps/app-icon/assets/AppIcon-512.png" "$stage/AppIcon.iconset/icon_512x512.png"
iconutil -c icns "$stage/AppIcon.iconset" -o "$contents/Resources/AppIcon.icns"
codesign --force --sign - "$bundle"

mkdir -p "$HOME/Applications"
ditto "$bundle" "$app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app"
echo "$app is registered to open Halo 2 session links."
