# forge

The local in-game Forge session: the monitor camera, an Objects tool, a Links tool for teleporters and machines, and the saved Forge files. It also holds the synchronous network mode's protocol: the host, the peers and their stream. The caller supplies the loaded map's palette, bounds, teleporters and rows and invokes each edit at a game boundary after checking session access.

## Where it sits

`extensions` domain. Depends on `content`, `forge-doc`, `map` and `settings`. It has no guest access; `crates/extensions/game` is the host.

## Layout

- `src/session.rs`: the session state and its edits.
- `src/tool.rs`: the Objects tool.
- `src/camera.rs`: the monitor camera.
- `src/links.rs`: the Links tool: the map's teleporters and the session's machines.
- `src/files.rs`: the saved Forge files.
- `src/scene.rs`: the placement blocks in scenario order, appended on Add and swap-removed on Remove.
- `src/things.rs`: the things: the loaded map's own entries and the added ones, the rows the session keeps, and the base calls that bring the world to the document.
- `src/props.rs`: the properties a thing shows by name, and the one entry op each writes.
- `src/look.rs`: how placed objects look.
- `src/net.rs`: the synchronous network protocol's messages, roles, refusals and op ID lanes; `src/net/host.rs` the host that admits proposals, holds and members and writes the stream; `src/net/peer.rs` a machine's end, which orders the stream and hands its game the items due at each tick; `src/net/state.rs` the session after a prefix of the stream. Tests in `src/net/tests.rs`. `src/net/node.rs` runs a machine over a `Lane` (the host with its remotes, or a guest), settles proposals and holds by nonce, checks the beat against the log and reports the session for `forge.status`; `src/net/wire.rs` the framed messages and the 48-byte beat; `src/net/film.rs` the film's Forge records and their replay through a peer. Tests in `src/net/node_tests.rs`. `src/session/shared.rs` is the session in a shared game: edits queue as proposals, items apply with `apply_item`, and `admit` gives the host the game's checks.

## Tests

`cargo nextest run -p forge`.

## Docs

- [game/forge-design.md](../../../docs/game/forge-design.md)
- [game/forge-map-variants.md](../../../docs/game/forge-map-variants.md)
