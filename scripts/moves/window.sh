#!/bin/sh
# One freeze window, or its replay on a lane's branch
# (docs/dev/crate-structure.md, "How a window runs").
#
#   scripts/moves/window.sh run <id>      the coordinator, on master
#   scripts/moves/window.sh replay <id>   a lane, on its own branch
#   scripts/moves/window.sh list <id>     the git mv lines, nothing changed
#
# <id> is w1, w2, w3, s5, or the name of a spec in scripts/moves/splits/.
# `run` tags the tip move-<id>-pre, runs the window's script, commits
# exactly its output and tags that move-<id>. `replay` merges
# move-<id>-pre, runs the same script on the lane's tree, commits, and
# records move-<id> as merged with `-s ours`: the lane's tree is already
# the moved one. Neither runs cargo.
set -eu
[ $# -ge 2 ] || { sed -n '2,13p' "$0"; exit 2; }
mode=$1 id=$2
shift 2
here=$(cd "$(dirname "$0")" && pwd)
case $id in
  w1) set -- python3 "$here/w1-domains.py" "$@" ;;
  w2) set -- python3 "$here/w2-hooks.py" "$@" ;;
  w3) set -- python3 "$here/w3-h1-merge.py" "$@" ;;
  s5) set -- python3 "$here/s5-boot-root.py" "$@" ;;
  *)  [ -f "$here/splits/$id.toml" ] || { echo "window: no window '$id'" >&2; exit 2; }
      set -- python3 "$here/split.py" "$id" "$@" ;;
esac
clean() { git diff --quiet && git diff --cached --quiet || { echo "window: commit or drop your changes first" >&2; exit 1; }; }
case $mode in
  list) exec "$@" --list ;;
  run)
    clean
    git tag "move-$id-pre"
    "$@" || { echo "window: the script reported problems. Undo with: git reset --hard move-$id-pre && git tag -d move-$id-pre" >&2; exit 1; }
    git commit -qm "Move $id (scripts/moves)"
    git tag "move-$id"
    echo "window: move-$id is $(git rev-parse --short HEAD). Check it, then merge lanes again."
    ;;
  replay)
    clean
    git merge --no-edit "move-$id-pre"
    "$@" || { echo "window: the script reported problems on this branch; fix them, commit, then: git merge -s ours --no-edit move-$id" >&2; exit 1; }
    git diff --cached --quiet || git commit -qm "Apply move $id"
    git merge -s ours --no-edit "move-$id"
    echo "window: this branch contains move-$id. Next: git merge master"
    ;;
  *) echo "window: run, replay or list" >&2; exit 2 ;;
esac
