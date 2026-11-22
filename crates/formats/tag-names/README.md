# tag-names

A developer comparison of retail plugin layouts (the Xbox reference, its fallback and other retail games as hints) against the checked-in tag definitions, so field names can be merged against ours. It is not used by the game or the editor at run time.

## Where it sits

`formats` domain. Depends on `tags` and `quick-xml`.

## Layout

- `src/assembly.rs`, `src/assembly/`: reads the plugin XML.
- `src/merge.rs`, `src/merge/`: `Source`, `Plugin` and the merge against our definitions.

## Tests

`cargo nextest run -p tag-names`.

## Docs

- [editor/notes/tag-field-names.md](../../../docs/editor/notes/tag-field-names.md)
