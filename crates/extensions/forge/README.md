# forge

The local in-game Forge session: the monitor camera, an Objects tool, a Links tool for teleporters and machines, and the saved Forge files. The caller supplies the loaded map's palette, bounds and teleporters and invokes each edit at a game boundary after checking session access.

## Where it sits

`extensions` domain. Depends on `content`, `forge-doc` and `settings`. It has no guest access; `crates/extensions/game` is the host.

## Layout

- `src/session.rs`: the session state and its edits.
- `src/tool.rs`: the Objects tool.
- `src/camera.rs`: the monitor camera.
- `src/links.rs`: the Links tool: the map's teleporters and the session's machines.
- `src/files.rs`: the saved Forge files.
- `src/scene.rs`: the placement blocks in scenario order, appended on Add and swap-removed on Remove.
- `src/look.rs`: how placed objects look.

## Tests

`cargo nextest run -p forge`.

## Docs

- [game/forge-design.md](../../../docs/game/forge-design.md)
