#!/bin/sh
# Installs the Rust toolchain rust-toolchain.toml pins and pins the rest of
# the job to it (.github/workflows/ci.yml). Arguments are extra rustup
# components, e.g. `install-rust.sh clippy`.
#
# RUSTUP_TOOLCHAIN makes rustup ignore rust-toolchain.toml, so it never
# installs that file's clippy and rustfmt on top (on a runner that install
# collides with the image's own cargo-fmt). live-deploy.yml does the same.
set -eu
channel=$(awk -F'"' '/^channel/ { print $2 }' rust-toolchain.toml)
[ -n "$channel" ] || { echo "no channel in rust-toolchain.toml" >&2; exit 1; }
components=
for c in "$@"; do components="$components --component $c"; done
# shellcheck disable=SC2086
rustup toolchain install "$channel" --profile minimal $components
echo "RUSTUP_TOOLCHAIN=$channel" >> "${GITHUB_ENV:-/dev/null}"
RUSTUP_TOOLCHAIN=$channel rustc -V
RUSTUP_TOOLCHAIN=$channel cargo -V
