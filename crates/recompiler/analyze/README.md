# analyze

Disc-supplied x86 function discovery, control-flow graphs and coverage. The
analyzer does not contain executable bytes or extracted game tables.

`analyze(&xbe)` descends direct control flow, tries address-taken and gap
candidates, and seeds callbacks supported by runtime-decoded pointer tables.
The seeds live in `src/pointer_seeds.rs`; switch sizing remains in `lib.rs`.
`analyze_with_pointer_seeds(&xbe, false)` reproduces the pre-seeding census.

For the six campaign systems, run the address-only TSV report:

```sh
H2_XBE=/path/to/default.xbe cargo run -p analyze --release --example dispatch_coverage
```

Append `-- --unseeded` for the baseline. Use release mode for retail analysis.
The report enumerates each root's conservative closure, indirect sites, table
slots and missing entries. Read [dispatch coverage](../../../docs/lifter/dispatch-coverage.md)
for its evidence levels, limitations, reproduction and runtime diagnosis.

```sh
cargo test -p analyze
cargo clippy -p analyze --all-targets -- -D warnings
H2_XBE=/path/to/default.xbe cargo test -p analyze --release --test dispatch_coverage -- --ignored --nocapture
```
