# firefight-kit

The offline stage-one Firefight map kit: it builds a Firefight package from the owner's multiplayer cache. Only the authored character gets a new datum; scenario and BSP roots keep their addresses and point at new block trees in resident tag memory.

## Where it sits

`tools` domain. Depends on `bots`, `content`, `h1campaign`, `h1convert`, `hs`, `map`, `pathfind`, `scenario-edit` and `tags`.

## Layout

- `src/lib.rs`: package assembly.
- `src/ai.rs`, `src/ai/`: the AI data the package carries.
- `src/nav.rs`, `src/nav/`: pathfinding data, drop zones and base zones for the package.
- `src/bases.rs`, `src/bases/`: the map's two bases (CTF flag stands, team starts or start clusters) and the `--home` option.
- `src/transplant.rs`, `src/transplant/`: stage two, classes copied from the owner's campaign maps, their cost and the cut order.
- `src/retail.rs`: the retail class table (each class's `char` tag name and the campaign maps that carry it).
- `Fit` in `src/lib.rs`: the steps a map short of tag headroom takes (merged sectors, then fewer drops).
- `src/testkit.rs`: `court`, the invented map the tests build on, and `court_kit`, its kit map; `src/nav/fixture.rs`: its collision.
- `src/tests.rs`: tests on invented maps.

## Features

- `testkit`: `firefight_kit::testkit` for other crates' tests (`host-data`'s kit bundle, `cli`'s server-map strip).

## Tests

`cargo nextest run -p firefight-kit`.

## Docs

- [game/firefight.md](../../../docs/game/firefight.md)
