#!/usr/bin/env bash
# Runs the workspace hygiene checks: rustfmt, clippy (warnings are errors) and
# the test suite. Stops at the first failing step. Ignored tests (those that
# need the disc in disc/) are not run; see CONTRIBUTING.md.
#
#   scripts/check.sh                 run every check on the whole workspace
#   scripts/check.sh --fix           apply cargo fmt and clippy fixes first
#   scripts/check.sh --crate <name>  limit the checks to one package
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

usage() {
  cat <<'EOF'
Usage: scripts/check.sh [--fix] [--crate <name>]

  --fix            run `cargo fmt` and `cargo clippy --fix` before checking
  --crate <name>   limit fmt, clippy and test to one package
  -h, --help       show this help
EOF
}

fix=0
crate=""
while (($# > 0)); do
  case "$1" in
    --fix)
      fix=1
      shift
      ;;
    --crate)
      if (($# < 2)) || [[ -z "$2" ]]; then
        echo "check.sh: --crate needs a package name" >&2
        exit 2
      fi
      crate="$2"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "check.sh: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -n "$crate" ]]; then
  fmt_scope=(-p "$crate")
  cargo_scope=(-p "$crate")
  scope_label="package $crate"
else
  fmt_scope=(--all)
  cargo_scope=(--workspace)
  scope_label="workspace"
fi

# step <title> <command...>: prints the step, runs it, and exits on failure.
step() {
  local title="$1"
  shift
  printf '\n==> %s\n    $ %s\n' "$title" "$*" >&2
  if ! "$@"; then
    printf '\ncheck.sh: FAILED at "%s" (%s)\n' "$title" "$scope_label" >&2
    printf '    Fix the output above and re-run scripts/check.sh.\n' >&2
    exit 1
  fi
}

if ((fix)); then
  step "apply rustfmt fixes" cargo fmt "${fmt_scope[@]}"
  step "apply clippy fixes" cargo clippy "${cargo_scope[@]}" --all-targets --fix --allow-dirty
fi

step "rustfmt check" cargo fmt "${fmt_scope[@]}" --check
step "clippy (warnings are errors)" cargo clippy "${cargo_scope[@]}" --all-targets -- -D warnings
step "tests" cargo test "${cargo_scope[@]}"

printf '\ncheck.sh: all checks passed (%s)\n' "$scope_label" >&2
