# symbols

Identifies statically linked Xbox SDK library functions in an XBE by OOVPA signature. The signature data is ODbL-licensed and is never vendored: it is read at run time from a checkout, so only the matcher lives here.

## Where it sits

`recompiler` domain. Depends on `xbe`. Used by `cli`.

## Layout

- `src/parse.rs`: reading the signature database.
- `src/scan.rs`: scanning code sections per linked library and build.
- `src/solve.rs`: resolving symbols against revisions up to the build.

## Tests

`cargo nextest run -p symbols`. Tests that match real signatures need the checkout from `scripts/setup-xbsymboldb.sh` (`H2_XBSYMDB` overrides the default path) and the retail executable.

## Docs

- [lifter/xdk-libraries.md](../../../docs/lifter/xdk-libraries.md)
