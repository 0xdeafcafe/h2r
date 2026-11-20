# crates/hooking

How our code meets guest code: the feature registry with its named points and ordered hook chains, and the typed guest layouts that name every structure offset and global address once. A hooking crate may use `formats` and `runtime`; `rewrites`, `live`, `extensions` and `apps` use it. See [crate-structure.md](../../docs/dev/crate-structure.md); the layering is checked by `scripts/crate-graph.py layers --check`.

| Crate | What it is |
|---|---|
| [`guest-layout`](guest-layout/) | Typed guest layouts, the guest-memory boundary and the named points. |
| [`hooks`](hooks/) | The feature registry: named points, hook chains, conflicts at plan time, switches. |
