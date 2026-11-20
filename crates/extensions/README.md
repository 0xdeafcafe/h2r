# crates/extensions

Features we add to the game: the settings and campaign screens, skulls and variants, the debug menu, the agent API, live tag edits, the determinism harness and the Halo 1 hooks. An extension may use `formats`, `runtime`, `platform`, `hooking`, `rewrites` and `live`, and only `apps` uses it. See [crate-structure.md](../../docs/dev/crate-structure.md); the layering is checked by `scripts/crate-graph.py layers --check`.

| Crate | What it is |
|---|---|
| [`agent`](agent/) | The agent API server in the game process. |
| [`agent-proto`](agent-proto/) | The agent API's wire types and client. |
| [`debug-menu`](debug-menu/) | The in-game debug menu over the agent API's registries. |
| [`game`](game/) | Our additions to the game: screens, skulls, variants, campaigns and the glue to every subsystem. |
| [`h1-runtime`](h1-runtime/) | Hooks only converted Halo 1 maps need. |
| [`hot-tags`](hot-tags/) | Live edits of the running game's tags, with a journal. |
| [`lockstep`](lockstep/) | The determinism harness: per-tick RAM hashes and trace comparison. |
