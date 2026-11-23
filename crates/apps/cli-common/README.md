# cli-common

What the `h2` and `h2-tools` binaries share: the default disc (`$H2_DISC`, else the Halo 2 `.iso` in the checkout's `disc/`), opening a disc image, its XBE and its maps, and `safe_out`, the no-clobber output directory every exporter writes through ([cli-tools.md](../../../docs/runtime/cli-tools.md)).

## Where it sits

`apps`. It depends on `map` and `xiso`; `cli` and `h2-tools` depend on it.

## Tests

`cargo xtask test -p cli-common`. Synthetic files in a temp dir only.
