# runtime-test-probe

Native C fixtures and their declarations for `runtime`'s integration tests. Production builds of `runtime` do not depend on it, and it is not published.

## Where it sits

`runtime` domain, under `crates/runtime/runtime`. Depends on `runtime`; `build.rs` compiles the C fixtures with `cc` against `../include/h2abi.h`.

## Layout

- `build.rs`: compiles the C fixtures.
- `src/lib.rs`: the `extern "C"` declarations the tests call.

## Tests

It has none of its own; `crates/runtime/runtime/tests` use it.
