# Performance: not adopted

Experiments that were tried and lost, so the same one is not built twice.
Each row: what it was, the result, and where the code went. The design
they were tried against is [../guest-memory.md](../guest-memory.md).

| Experiment | Result | Removed at |
|---|---|---|
| 4 KiB guest translation (`H2_FASTMEM=2` and a write-bit variant: a table mapping 4 KiB virtual to 4 KiB physical pages, with a held lease) | Covered 99.999% of menu accesses against 61% for the 16 KiB eligibility test, but used 22.4% more process CPU per frame than the inline path (medians of two 80-present menu runs each) and 15.3% more clang time; the write-bit variant did not differ | branch `fastmem-4k` at `894f2a3`, over master `c00124d`; never on master |
| f64 native x87 transcendentals (FSIN, FCOS, FSINCOS, FYL2X, F2XM1 through `libm`; switch `H2_X87_FAST_TRANSCENDENTALS`) | Tails of up to 2^41 units of the 64-bit result's last place for |x| < 10^6: not exact. Sweep in [guest-memory.md](guest-memory.md), "Handler entry and exit" | `2658859b9`, `361a99744` |
| A non-inlined leaf per access width for inline loads and stores | 1.5x faster than the helper path against 2.6x for the inline macro, with smaller code; dropped. [guest-memory.md](guest-memory.md), "Inline guest loads and stores" | not merged |
| A fixed-width switch in the per-access `h2_fast_copy` | No speedup established; not retained. [guest-memory.md](guest-memory.md), "Guest CPU: SSE, REP copies and D3D entry" | not merged |
| Write watch by Mach exception and page protection (`crates/recompiler/fastmem-proto`) | Prototype only; nothing in the runtime uses it. [fastmem-proto.md](fastmem-proto.md) | deleted after `0b3bbc32d` (architecture review, item 9) |
| Gate taken at handler entry (`H2_ENTRY_GATE`) | `cold_accesses_by_cause`: entry 3 to 0, lent-entry 39 to 0, after-dcall 39 to 0. `entry_access_benchmark`: runner entry 81.7 to 72.3 ns (11%), direct call 34.9 to 32.9 ns; with the inline charge added it was 2-4 ns slower than the entry gate alone. Never profiled in the game. [notes/guest-memory.md](guest-memory.md), 2026-11-18 | removed 2026-11-22 (flat RAM replaces the gate, architecture review) |
| Dirty bit tested at the store site (`H2_DIRTY_INLINE`) | `dirty_store_benchmark`, two stores to marked pages with a watch active: 4.37 to 3.59 ns; no change without a watch (3.21 vs 3.22). Never profiled in the game | removed 2026-11-22 |
| Gate and access count lent across direct calls (`H2_DIRECT_GATE`, `H2_DIRECT_COUNT`, with the `H2Direct.gate`/`acc` fields) | Proven equivalent to the helper path by `held_gate_matches_the_helper_path_at_every_stop_and_fault`; never timed | removed 2026-11-22; `H2Direct` is 48 + 16 x 64 + 8 bytes |
| Per-hold RAM page cache (`H2_RAM_PAGE_CACHE`) | Built and tested in `fast_copy.c`; never measured in a full image | removed 2026-11-22 |
| Checked x87 TOP specialisation (`H2_X87_TOP`, `H2X87_AT` with a constant slot and an out-of-line dynamic fallback) | Native x87 results were identical to the dynamic path (`native_x87.rs`); never measured in a full image | removed 2026-11-22; `H2X87_AT` names the site's TOP and calls the dynamic helper |
| Per-call charge path (`H2_VIEW_CHARGE=0`) | The inline charge saved about 2 ns a runner entry and nothing on a direct call; the default since feddd0402. Never compared in the game | removed 2026-11-22; a counting module with a view always spends in the view |
| `-march=x86-64-v3` (and `znver4`) for the Linux module through `H2_CPU` / `[h2] cpu` | Host 0.8-2.0% faster in a headless level; on the Steam Machine within noise at the main menu and in 03a. [steammachine-perf.md](steammachine-perf.md) | not merged |
| Fat LTO for `h2` (`-c build_mode.lto=fat`) | rustc was killed for memory after 172 s on a 64 GB host with 9 GB free; thin LTO is the default instead. [steammachine-perf.md](steammachine-perf.md) | kept as an option, not the default |
| `MADV_HUGEPAGE` on guest RAM (Linux) | Not built: SteamOS runs THP `always` and 58 of 64 MiB of guest RAM is already on huge pages; dTLB walks are about 0.2% of the game thread. [steammachine-perf.md](steammachine-perf.md) | never built |
