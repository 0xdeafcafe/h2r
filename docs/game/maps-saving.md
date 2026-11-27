# Maps: saving packages from the editor

Saving packages from the editor, and what is not verified. Part of [maps.md](maps.md).

## 8. Saving packages from the editor

(`content::patch::write`, `content::store`,
`scenario_edit::package`, the scenario panel; [scenario-editor.md](../editor/scenario-editor.md) §5).

**Patch writer.** `content::patch::diff(base, edited, description)` makes
one layer:

- same length: **PPF3**, which the resolver overlays without copying.
  Records are the differing runs, runs closer than a record header (9
  bytes) merged, split at 255 bytes; no block check, no undo data (the
  resolver checks the base's SHA-256 instead), no FILE_ID.DIZ. A body
  whose last six bytes would read as a FILE_ID.DIZ tail gets one more
  1-byte record, so it never does.
- any other length (a grown block): **BPS**: SourceRead where bytes match
  in place, SourceCopy that continues the last copy or finds a 64-byte
  source block through a hash index, TargetCopy the same way over the new
  bytes already written (data a mod added twice, padding), TargetRead for
  the rest. For Night Ops (a 95.7 MiB map over 47.6 MiB Containment)
  about 7.8 MiB of the new bytes repeat earlier new bytes, so TargetCopy
  takes the 68.2 MiB delta to about 60 MiB; 03a_oldmombasa holds 65.7 MiB
  of its blocks, but a package's base is the map it replaces, so that
  cannot be its source.

Both reproduce the edited bytes exactly, **header checksum included**. The
editor's map writer leaves the checksum right, so the resolver's repair at
+0x2d0 writes the same four bytes again, and the patch works in any other
PPF or BPS tool without a "resign" step. Tests round-trip random edits
(near the end, at +0x2d0, growth, insertion, truncation) through the
crate's own readers and through the resolver.

**Writing a package.** `content::store::write_package` assembles a new
package in a hidden sibling directory and renames it into place; replacing
the package being edited writes its new files, then the manifest by
rename (the commit point), then removes files only the old manifest named.
The editor can write `active.toml` choices, which the game ignores. `store::default_data_root()` is the runtime's default data root,
`~/Library/Application Support/h2recomp/data`, the one `h2 play` and `h2 run` use without
`--data` (`vfs::Config::for_title`'s `~/Library/Application
Support/h2recomp` is not used by any run).

**The editor's package**:

```text
<data>/maps/<dir>/
  manifest.toml        [package] name, author, description, image
                       [base] map = the disc map's name, sha256 = its SHA-256
                       [[patch]] <map>-<first 8 hex of its sha256>.ppf|.bps
                       [result] geometry (above)
  <map>-<hash>.ppf     base -> current, always one layer
  preview.png|.jpg     optional, PNG/JPEG <= 512 KB, checked before writing
```

Opening a package resolves it with the content resolver over the disc's
map (`Library::resolve_package`, `Resolved::read_all`) into memory; that
becomes the editor's unedited state, with the disc's map kept as the base.
Saving it again replaces its one layer with a new base-to-current patch.

## Not verified

Only a game run shows these (Live on, signed in, a server with at least
two published maps, one on a disc map the player has and one that must
fail, such as a base whose hash differs from the disc's):

- The Xbox Live menu shows six rows with no scroll bar, the last reading
  "Mod Downloader (N new)" with its help text, and the console logs
  `mods: N on the server, M new`.
- A on Mods opens the screen, newest first, a star on each new map; B
  returns to the Live menu, whose row now reads "Mod Downloader", and
  `<data>/live-mods.toml` holds the newest position.
- The Mods list sits in the level select's bracket (up to 13 rows before
  it scrolls) with the focused map's picture at the box's size and four
  summary lines that change with focus; the footer reads Details,
  Download, Back with button glyphs.
- X downloads a map not here: the row reads `<name> ↓ N%`, then
  `<name> ✓`, and `<data>/maps/live-<16 hex>/` exists. Without a
  restart, Change Map lists the mod under Mods with its picture and
  description, and Game Setup and the lobby show its name and package
  image. The mod loads.
- A map whose base the player lacks reads "Needs <Map> from your disc"
  and writes nothing under `<data>/maps`.
- Signed out or with the server stopped, Mods shows Try Again and a hint
  ("Sign in to Live first", "Can't reach the server").
- A mod's page matches the pregame lobby's layout: one action row, the
  description on the panel, the picture under it, the facts table at the
  right with white names and pale blue values, then the checklist in two
  columns. Nothing is clipped, no text is missing (a panel widget not made
  logs `no panel text N`), and the texts slide with the page.
- Removal: the first press reads `Remove? Press again`, the second logs
  `mods: <name> (<id>) removed`, the folder is gone and Change Map no
  longer lists the mod.
- A pack of two or more maps is one row with its name and a star or "N
  maps"; A opens it with Download All (N) first; Download All logs
  `mods: <pack>: downloading N maps`, the row shows the percentage, then
  All Installed, and the pack's row on Mods gets a tick.
- A new map from `<data>/maps/` (a derived package with `[list] base`)
  appears in a local custom game's map choice last, with its name and
  preview (the package image, else the base's picture); it loads from
  `T:\$c\<offer ID>\<name>.map` and reaches spawn; deleting the package
  removes it from the list.
- The Mods screens' own states (long names, partial installation, failure,
  signed-out access, removal) and the B behaviour in
  [map-select.md](map-select.md) under "Game verification".

## Related

[map-select.md](map-select.md), [map-pack.md](map-pack.md),
[live-api.md](../live/live-api.md), [live-custom-games.md](../live/live-custom-games.md),
[scenario-editor.md](../editor/scenario-editor.md),
[test-maps.md](test-maps.md) (packages built from recipes for the agent API).

[formats]: https://connortumbleson.com/2020/04/12/halo-2-patch-format-types/
