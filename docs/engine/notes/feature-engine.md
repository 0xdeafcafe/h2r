# Feature engine and engine contracts: evidence

Evidence for [feature-engine.md](../feature-engine.md), [README.md](../README.md)
and [hs_vm.md](../hs_vm.md). Nothing here states what the code does today;
the design pages do.

## 2026-11-16: differential harness over every leaf, the game path and the views

Commit `ae5e6a190` (harness at `crates/rewrites/engine/tests/differential.rs`); image
pinned by SHA-256 (the hash is in `crates/rewrites/engine/tests/common`). Debug build.

Method: every call runs from one 64 MB guest state three ways: the lifted
original on the IR interpreter, the adapter directly, and
`engine::runtime_glue::invoke` on a `runtime::Guest` holding the same
memory (the body `engine-leaves` installs). For `datum_try_and_get`,
`datum_get_absolute` and `tag_get_checked` the harness also asks the
typed view (`Datums::get`, `Datums::get_absolute`, `Tags::get`) in the
original's memory.

Result, seeds 1, 7, 99, 2026 and 31337 at scale 1, all 22 adapters
exercised:

| Count | Value |
|---|---|
| original instructions interpreted | 7.66 M |
| calls | 93,304 |
| disagreements, adapter against original | 0 |
| disagreements, game path against adapter | 0 |
| view answers | 9,798 (`datum_try_and_get` 2,700, `datum_get_absolute` 1,337, `tag_get_checked` 5,761) |
| view disagreements | 0 |
| wall time | 817 s |

The locale leaves saw 4,500 finds and 4,502 exists (one a fault), the tag
leaves 11,762 calls and the datum leaves 72,540.

Reproduce: the command in the header of `crates/rewrites/engine/tests/differential.rs`
(feature `differential`, the owner's XBE, `--nocapture` prints the table).

## 2026-11-15: first differential run and injected bugs

Commit `5ce2974ef`. Seeds 1, 7, 99, 2026 and 31337 at scale 1: 7.66 M
original instructions, 93,291 calls, 5 matching fault outcomes, zero
disagreements across the 22 adapters. After the argument-order fixes and
the unmapped-argument fixtures, the same seeds: 93,304 calls, 17 matching
fault outcomes, zero disagreements.

Injected bugs the harness caught:

- `fill` as an all-or-nothing write fails the first fault fixture.
- Reading all of `data_initialize`'s arguments up front, reading the
  contiguous variant's name before its allocator, reading the locale count
  before the loaded test, or storing `data_last_datum`'s slot only at the
  end each fail an unmapped-argument or mid-walk fixture (9 disagreements
  together).
- A salt wrap, grandparent acceptance and a locale bound were each caught
  by the seeded runs.

## 2026-11-15: HaloScript VM against the disc's 30 maps

Commit `f5f7390a5`. Test: `crates/formats/hs/tests/vm_disc.rs` (`#[ignore]`;
`H2_DISC` names the owner's disc image).

Each of 30 maps runs under three policies (3,000 ticks of initialize and
update; every static and stub script on a transient thread; every dormant
script woken, then 3,000 more ticks): 0 faults, 90 runs, 1.4 s in release.

| Kind | Reachable | Called | Reachable, not called |
|---|---|---|---|
| Native | 26 | 26 | 0 |
| Discarded stub | 10 | 10 | 0 |
| Engine-facing | 354 | 292 | 62 |

The 62 not called sit behind conditions the mock never satisfies. Casts
performed include short to boolean (15), real to short (17,356) and object
name to object (67,568 plus the typed variants). Scripts started at least
once: 13,524 of 15,891 (script, run) pairs, 85%.
