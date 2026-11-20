# app-icon

The app icon for the unbundled Mac windows (`h2 play`, `h2-editor`), built from the art in `art/app-icon`. It is a one-file crate so both apps share one icon, since bare executables have no `.icns` for macOS to show.

## Where it sits

`apps`. No workspace dependencies. Used by `app` and `studio`.

## Layout

- `src/lib.rs`: the embedded PNG (`DOCK_PNG`), `decode`, and `set_dock_icon`, which gives the running app the plated Mac icon.

## Tests

`cargo nextest run -p app-icon`.

## Docs

- [app.md](../../../docs/runtime/app.md)
