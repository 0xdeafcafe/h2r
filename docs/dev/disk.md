# Disk cleanup

A machine that runs many worktrees and builds of this repository fills its
disk fast: a debug target is 10 GB or more, and finished worktrees and old
targets stay behind. `h2 disk clean` removes those leftovers and nothing
else.

```sh
h2 disk clean                 # dry run: what would go, and why the rest stays
h2 disk clean --yes           # remove it
```

## What goes

- **Worktrees.** A linked worktree of this repository goes when all of these
  are true:
  - it is on a branch that is merged into `--base` (default `master`);
  - it has no changes and no untracked files;
  - it is not locked;
  - nothing has committed, checked out or staged in it for a day;
  - no process has a file open in it, or names it in its arguments or
    environment.

  The branch goes with it (`git branch -d`). Detached worktrees stay: they
  belong to scripts that remove their own.
- **Idle targets.** A Cargo target goes whole when no build has touched it
  for two days and nothing uses it. Under 20 GB free (`--low-below`), half
  an hour untouched is enough: busy machines build into every target daily,
  and whoever comes back to a removed one rebuilds it. Targets are found in each remaining
  worktree's `target/` and, three levels deep, under every `--scan DIR`. A
  folder counts as a target only if Cargo's `CACHEDIR.TAG` and
  `.rustc_info.json` are both in it.
- **Stale artifacts.** In every other target that no build is using,
  `deps/` artifacts and `incremental/` sessions not written for two days go.
  Cargo rebuilds anything it later finds missing.

Run folders, data roots and every other folder are never touched.
"In use" is checked again right before each removal. Sizes count only files
with no other hard link, so artifacts that a caching wrapper hard-links
from its store (kache) free nothing and count as nothing.

## Options

| Option | Meaning |
| --- | --- |
| `--repo DIR` | a checkout of the repository (default: the current directory) |
| `--base BRANCH` | worktrees must be merged into it |
| `--scan DIR` | also look for targets here (repeatable) |
| `--if-below GB` | do nothing while more than GB are free |
| `--low-below GB` | under GB free, targets idle for half an hour go too (default 20) |
| `--kache PATH` | after cleaning, empty this kache store if still under `--empty-kache-below` GB free (default 20) |

## Every hour on macOS

A LaunchAgent runs the clean every 15 minutes, and it does nothing while
50 GB or more are free. Put this in `~/Library/LaunchAgents/red.forbes.h2-disk-clean.plist`,
replacing the paths in capitals:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>red.forbes.h2-disk-clean</string>
  <key>ProgramArguments</key>
  <array>
    <string>HOME/.cargo/bin/h2</string>
    <string>disk</string><string>clean</string><string>--yes</string>
    <string>--if-below</string><string>50</string>
    <string>--repo</string><string>REPO</string>
    <string>--scan</string><string>SCRATCH</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>
  <key>StartInterval</key><integer>900</integer>
  <key>RunAtLoad</key><true/>
  <key>Nice</key><integer>15</integer>
  <key>LowPriorityIO</key><true/>
  <key>StandardOutPath</key><string>HOME/Library/Logs/h2-disk-clean.log</string>
  <key>StandardErrorPath</key><string>HOME/Library/Logs/h2-disk-clean.log</string>
</dict>
</plist>
```

```sh
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/red.forbes.h2-disk-clean.plist
launchctl kickstart gui/$(id -u)/red.forbes.h2-disk-clean   # run it now
```

`h2` doesn't update itself: after merging a change to the clean, reinstall
it (`cargo install --path crates/cli --force`).

## Keeping targets small

- Leave a configured caching rustc wrapper on (docs/lifter/BUILD-CACHE.md).
  Clearing `RUSTC_WRAPPER` for a command makes its target a full private
  copy.
- Debug builds keep line tables only (`[profile.dev]` in `Cargo.toml`). For
  variables in a debugger, build with `CARGO_PROFILE_DEV_DEBUG=2`.
