# cloud-saves

Cloud saves on the Live service: merge an account's saved items into the local save storage at sign-in, and push changes as the game writes them.

## Where it sits

`live`. Uses `live-proto`, `live-server`, `vfs` and `settings`. Used by `cli` and `h2host`.

## Layout

- `src/local.rs`: what this machine has (the save containers and per-profile settings) as items.
- `src/merge.rs`: the pure three-way merge and its conflict rules.
- `src/state.rs`: the last agreed state and the upload queue.
- `src/remote.rs`: the server's storage endpoints.
- `src/watch.rs`: VFS change reports to item names.
- `src/engine.rs`, `src/session.rs`: sync at sign-in, then pushes while the game runs.

## Tests

`cargo nextest run -p cloud-saves`. `tests/http.rs` runs against the real server embedded on loopback with an in-memory log; every byte is invented.

## Docs

- [live-cloud-saves.md](../../../docs/live/live-cloud-saves.md)
