# uwp-host

The Xbox (UWP, Dev Mode) app: a CoreApplication view that runs the game into a composition swap chain, plus the Mac-side packaging helpers (AppxManifest, package identity, the app-container API check). The app code builds on Windows only; the helpers build everywhere.

## Where it sits

`apps`. On Windows it uses `h2host`, `app`, `gfx` and `vfs`; elsewhere it has only the packaging helpers. Nothing depends on it.

## Layout

- `src/main.rs`: the `h2uwp` executable (it says so and exits off Windows).
- `src/shell.rs`: the CoreApplication view.
- `src/manifest.rs`, `src/identity.rs`: AppxManifest and package identity.
- `src/imports.rs`: the app-container API check.
- `src/switches.rs`: switches read on the console.

## Tests

`cargo nextest run -p uwp-host`. `tests/deploy.rs` checks the manifest and package images and needs no disc.

## Docs

- [xbox.md](../../../docs/runtime/xbox.md)
- [windows.md](../../../docs/runtime/windows.md)
