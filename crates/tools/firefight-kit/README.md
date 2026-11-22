# firefight-kit

The offline stage-one Firefight map kit: it builds a Firefight package from the owner's multiplayer cache. Only the authored character gets a new datum; scenario and BSP roots keep their addresses and point at new block trees in resident tag memory.

## Where it sits

`tools` domain. Depends on `bots`, `content`, `h1campaign`, `h1convert`, `hs`, `map`, `pathfind`, `scenario-edit` and `tags`.

## Layout

- `src/lib.rs`: package assembly.
- `src/ai.rs`, `src/ai/`: the AI data the package carries.
- `src/nav.rs`, `src/nav/`: pathfinding data for the package.
- `src/tests.rs`: tests on invented maps.

## Tests

`cargo nextest run -p firefight-kit`.

## Docs

- [game/firefight.md](../../../docs/game/firefight.md)
