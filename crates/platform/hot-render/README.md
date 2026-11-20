# hot-render

Hot iteration on the host renderer. A `Hub` is where the agent API's
`render.*` methods and the renderer meet without either knowing the other.
It holds a directory of WGSL overrides named after the translated program
they replace, tunable parameters, and toggles.

## Where it sits

Domain `platform`; it depends on `shaders`. `d3d` reads the hub, and `boot`,
`game` and `agent` set it up and drive it.

## Layout

- `src/iface.rs`: the `Hub` and its interface.
- `src/watch.rs`: the directory watcher (`watch`, `signature`).
- `src/params.rs`: tunable parameters and toggles.

## Tests

`cargo nextest run -p hot-render`.

## Docs

- [docs/graphics/hot-iteration.md](../../../docs/graphics/hot-iteration.md)
- [docs/runtime/agent-api.md](../../../docs/runtime/agent-api.md)
