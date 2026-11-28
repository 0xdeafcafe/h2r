# h2host

The Rust half of the tvOS and iOS apps: a static library that the UIKit hosts call to boot the game into their `CAMetalLayer`. The `uwp-host` Xbox app links it too.

## Where it sits

`apps`. Uses `boot`, `app`, `audio`, `input`, `vfs`, `xiso`, `kernel`, `settings`, `live-hle`, `live-proto` and `cloud-saves`. Used by `uwp-host`.

## Layout

- `src/host.rs`: the shell and the C entry points the host calls (Apple and Windows only: `app::embedded` is built for those) (`h2host_start`, `h2host_redraw`, `h2host_resize`, `h2host_status` and the text-input calls). `src/lib.rs` declares the modules.
- `src/layout.rs`, `src/overlay.rs`, `src/touch.rs`: on-screen controls and touch input.
- `src/text.rs`: the system keyboard bridge.
- `src/maps.rs`: the map picker data.

## Features

- `perf`: the sampling profiler (`boot/perf`); the Xbox app turns it on.

## Tests

`cargo nextest run -p h2host`. No test needs a disc.

## Docs

- [tvos.md](../../../docs/runtime/tvos.md)
- [ios.md](../../../docs/runtime/ios.md)
- [app.md](../../../docs/runtime/app.md)
- [live.md](../../../docs/live/live.md)
