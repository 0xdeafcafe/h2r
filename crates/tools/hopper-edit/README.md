# hopper-edit

The playlist (`multiplayer_hopper.dat`) editor's logic with no UI, shared
by `h2-editor`'s playlist tool (`crates/apps/studio`):

- `model`: the working copy of the 16 playlists, edits, validation (the
  Live server's rules) and writing back byte-exact.
- `catalog`: the multiplayer maps a record can name, read from the user's
  disc.
- `choices`: the variant picker's matchmaking variants, the built-in
  Firefight wave sets (`choices::firefight_record`, Crossfire with team
  play), saves and `.h2variant` files. A hopper plays scripted variants
  and built-in Firefight sets only (`model::needs_script`), so the disc's
  built-ins are listed but never usable.
- `presets`: historical Xbox Live lineups, resolved against the disc.
- `session`: `HopperSession`, the open file with undo, the map list and
  the picker's sources, reading and writing both forms of the file.
- `fixture`: invented test data (no game data).

See [docs/editor/editor.md](../../../docs/editor/editor.md) ("Playlists") and
[docs/live/live-admin.md](../../../docs/live/live-admin.md).
