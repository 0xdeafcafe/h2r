# sound-edit

The waveform edits of a package sound, UI-free. A source WAV is never
rewritten: its edits are a list in `<name>.sound.yaml` ([`SoundDoc`]),
and [`render`] applies the list to the decoded source, so an edit is undone
by dropping it from the list and reordered by moving it.

- `wave`: [`Wave`], 16-bit interleaved samples; WAV read and write; peaks and levels.
- `edit`: [`Edit`] (`trim`, `fade_in`, `fade_out`, `gain`, `normalise`, `silence`), [`render`], and the list operations.
- `doc`: [`SoundDoc`] (`class`, `edits`, `permutations`) and [`Permutation`] (a WAV file with weight, gain and skip fraction).

The Studio methods over it are `sound.edit`, `sound.render` and
`sound.permutation` (`crates/apps/studio/src/api/sound_edit.rs`); the package
build renders the list before it encodes (`crates/tools/workspace/src/sound.rs`).
