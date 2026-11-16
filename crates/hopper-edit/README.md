# hopper-edit

The playlist (`multiplayer_hopper.dat`) editor's logic with no UI, shared
by `h2-editor`'s egui hopper tool and `h2-studio`'s playlist tool:

- `model`: the working copy of the 16 playlists, edits, validation (the
  Live server's rules) and writing back byte-exact.
- `catalog`: the multiplayer maps a record can name, read from the user's
  disc.
- `choices`: the variant picker's built-ins, saves and `.h2variant` files.
- `presets`: historical Xbox Live lineups, resolved against the disc.
- `session`: `HopperSession`, the open file with undo, the map list and
  the picker's sources, reading and writing both forms of the file.
- `fixture`: invented test data (no game data).

See docs/editor/editor.md ("The hopper tool") and docs/live/live-admin.md.
