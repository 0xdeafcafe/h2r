# hot-tags

Live edits of the running game's tags in guest memory, by field path from the tag definitions, with a journal to list and revert them. The agent API's `tags` methods and the debug menu use it.

## Where it sits

`extensions`. Uses `tags`, `map`, `runtime` and `engine`. Used by `agent`, `boot` and `game`.

## Layout

- `src/index.rs`: where the loaded map's tags sit in guest memory.
- `src/path.rs`, `src/names.rs`: field paths and names.
- `src/value.rs`: typed values.
- `src/lib.rs`: `Editor` and its journal.

## Tests

`cargo nextest run -p hot-tags`.

## Docs

- [hot-iteration.md](../../../docs/graphics/hot-iteration.md)
