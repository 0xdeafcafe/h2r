# Performance tooling

Three tools for finding out where time goes, in two crates:

- `crates/timeline`: dependency-free timeline events, written as Chrome
  trace-event JSON. The runtime, kernel and d3d call it directly.
- `crates/perf`: the sampling profiler, `perf::Session` (the profiler plus
  a timeline capture around any run), and the benchmark harness. Behind
  `h2 profile` and `h2 bench`.

The commands are behind the CLI's `perf` cargo feature, off by default so
ordinary `h2` builds skip the profiler's dependencies:
`cargo run --release -p cli --features perf -- bench`. Without it,
`h2 profile` and `h2 bench` say how to rebuild. The runtime, kernel and
d3d always depend on `timeline`, which has no dependencies of its own.

Measure release builds. Debug builds
of the runtime are 10-100x slower and spend their time in different places.
Generated code is compiled with emit's own flags (`H2_OPT`, default `-O1`)
whatever the Cargo profile.

## Sampling profiler

```
h2 profile hot-loop/memory --seconds 5 --out $TMPDIR/prof
H2_PROFILE=$TMPDIR/prof <any binary that opens perf::Session::from_env()>
```

### How it samples

A sampler thread wakes at the chosen rate (default 1000 Hz). For each target
thread it suspends the thread with Mach `thread_suspend`, reads its registers
with `thread_get_state`, walks the frame-pointer chain, and resumes it with
`thread_resume`. `thread_info` records whether the thread was running or
blocked, so the profile is wall-clock with an on-CPU flag per sample
(`--cpu` keeps only on-CPU samples).

This was chosen over the alternatives for reliability:

- A `SIGPROF` handler runs on the target thread, which may be inside
  generated code holding the guest memory lock, or inside malloc. Everything
  the handler does would have to be async-signal-safe. `thread_get_state`
  needs no cooperation from the target.
- `sample(1)` and `xctrace` run as a separate process and need the
  generated dylib's symbols to be mapped back afterwards. In process, the
  dylib is still loaded and its line tables are at hand.
- Frame pointers are reliable here. The Apple arm64 and x86_64 ABIs keep
  them, Rust's Apple targets keep them, and emit's clang flags do not omit
  them. The walk only reads inside the thread's stack between its SP and
  base, requires each record to be aligned and to move toward the base, and
  strips pointer-authentication bits.

While a target is suspended the sampler only copies registers and stack
words into a fixed buffer: no allocation and no locks, because the target
might hold the allocator's or the runtime's lock. On arm64 the link register
is kept too. When the leaf has not pushed a frame yet, LR is its caller,
which the chain would otherwise skip; it is added only when it names a
function that is not already on the stack.

The sampler is macOS-only (`perf::sample::SUPPORTED`). On other hosts
`Sampler::start` returns `Unsupported`; the timeline and the benchmarks
still work.

Cost: each sample holds the target for the time `thread_suspend` takes to
stop it plus the walk. The mean is printed as "mean pause": about 50 us in
a debug build, which is 5% of the thread's time at 1000 Hz. Lower `--hz`
for long runs.

### Mapping generated code back to the guest

Every generated handler is the C function `h2fn_<entry>` (`emit::c::symbol`),
so a PC whose symbol has that name belongs to the guest function at
`<entry>`. The guest instruction comes from the line tables kept by emit's
default `H2_DEBUG=lines`. The PC gives a C source line, and the nearest
`/* 0x........ */` marker at or above that line in the same shard is the
instruction (emit's `c.rs` writes one before every instruction's block). So
no sidecar map is needed. With `H2_DEBUG=none`, or once the shard sources
or objects are deleted, samples still resolve to the guest function but not
to the instruction.

The `h2gen.h` helpers are `static inline` and get folded into the handler,
so time in them counts as guest code. Calls that leave the handler count
against whatever they call (`h2abi_v1_read` is runtime, a kernel export is
kernel, and so on).

The search for a marker is clamped to the PC's own function. Markers
belong to the `h2fn_<entry>(` definition above them, so a line in a
handler's prologue or selector switch, which comes before its first
marker, has no instruction. It does not inherit the last instruction of
the previous function in the shard.

Symbolisation runs in process. `dladdr` finds each PC's image, asking the
dynamic loader at every lookup, so a dylib loaded after an earlier
profile or lookup is seen. addr2line then reads that image's symbol table
and DWARF line tables, including macOS debug maps that point at the
objects; this is cached per image path and load address. It must run
**before the generated dylib is unloaded**: finish or drop the `Session`
first.

### Attribution

Each sample is charged to the innermost frame that has a category. Frames
in libc, the allocator or std are skipped, so a `memmove` inside a memory
read counts as runtime.

| category | frames |
|---|---|
| `guest` | `h2fn_*` (generated code) |
| `runtime` | `runtime::*`, `h2abi_*`, `h2shim_*`, `h2x87*`, `x87::*`, `simd::*`, the module bridge (`h2bridge*`, `h2boot_*`) |
| `kernel` | `kernel::*`: HLE exports and the scheduler (`Machine::run`) |
| `d3d` | `d3d::*`: D3D HLE and push-buffer execution |
| `gfx` | `gfx::*`, `wgpu*`, `naga::*`, Metal (`MTL`, `AGX`) |
| `fallback` | `*fallback::*` (the boot driver's interpreter fallback), `ir::interp*` |
| `hle` | `xapi::*`, `dsound::*`, `bink_hle::*`, `audio::*`, `wma::*`, `movies::*` |
| `other` | nothing recognised |

### Outputs

Written into `--out` (default `$TMPDIR/h2profile`) or `H2_PROFILE`:

- `profile.folded`: folded stacks, root first (`thread;frame;...;leaf N`).
  Guest frames are `guest 0x<entry>` with an `@0x<insn>` leaf when the line
  tables allow (`--no-insns` to drop it). Samples taken while the thread was
  blocked end in `[off-cpu]`. `--by-category` puts the category at the
  root. Start-up frames (dyld, `main`, `lang_start`) are trimmed unless you
  pass `--full-stacks`. Open the file with `inferno-flamegraph`,
  `flamegraph.pl`, or <https://speedscope.app>.
- `profile.txt`: the top-N tables. They cover samples by category; self
  time (by charged frame); guest functions counting the services they
  called; guest functions counting only their own code; and the hottest
  guest instructions.
- `profile.json`: the same tables as JSON (`perf::profile::Summary`).

### From code

```rust
// Keep `_perf` declared after the loaded module, so it drops (and
// resolves) first.
let _perf = perf::Session::from_env();
```

`Session::from_env` reads the following:

| variable | meaning |
|---|---|
| `H2_PROFILE=<dir>` | sample the calling thread, write the profile there |
| `H2_PROFILE_HZ` | rate (default 1000) |
| `H2_PROFILE_TOP` | rows per table (default 25) |
| `H2_PROFILE_CPU=1` | on-CPU samples only |
| `H2_TIMELINE=<file.json>` | record a timeline and write it there |
| `H2_TIMELINE_CATS` | categories, e.g. `all`, `default`, `frame,d3d,gfx`, `all,-hle` |

A session starts its sampler before its timeline. If the sampler cannot
start (on a host other than macOS, with no target threads, or when the
thread cannot spawn), `start` fails and leaves nothing recording.
`Session::start_with(config, targets)` samples other threads too. Make each
`perf::sample::ThreadTarget::current(name)` on the thread itself.

## Timeline

```
h2 profile d3d/pushbuf-frame --timeline $TMPDIR/trace.json --cats all
H2_TIMELINE=$TMPDIR/trace.json <binary with a Session>
```

Open the JSON in <https://ui.perfetto.dev> or `chrome://tracing`. Guest
threads get their own rows (process "guest threads"); host-side events go
on the row of the host thread that recorded them (process "host").

| category | event | where |
|---|---|---|
| `frame` | `frame` instant, numbered | `d3d::Device::swap` |
| `guest` | `slice`: a guest thread on the CPU | `kernel::Machine::run` |
| `kernel` | `wait objects`, `wait critical section`, `sleep` spans on the guest thread's row, from block to next schedule | `kernel/src/sync.rs`, `threads.rs` |
| `kernel` | `idle: clock to deadline` instant | `Machine::run` |
| `interrupt` | `DPC` span (arg `routine`) on the DPC row | `Machine::run_dpc` |
| `interrupt` | async span named after the `GuestInterrupt` (vblank, ...) from delivery to return | `runtime/src/interrupt.rs` |
| `d3d` | `Swap`, `KickOff`, `pushbuf drain` (arg `bytes`) | `d3d/src/device.rs` |
| `gfx` | `queue submit`, `present` | `d3d/src/gfx_backend.rs` |
| `hle` | `hle call` (arg `pc`) around every Rust override | `runtime/src/handler.rs` |

`hle` is off by default because it fires on every kernel, XAPI and D3D call.

Cost when off: every hook is one relaxed atomic load and a branch. Nothing
allocates, locks or reads the clock unless a category is recording. A
`Span` is one pointer, null when inert, and its drop is an inlined null
check; the recording path is out of line.

Each `start` and `stop` begins a new capture generation. An event keeps
the generation that was current when its hook fired, and it is discarded
unless that generation is still recording when the event is pushed. So a
span (or wait) opened in one capture and closed after `stop`, or after the
next `start`, never lands in a trace.
`timeline::frame()` always bumps a frame counter (`timeline::frames()`),
one relaxed `fetch_add` per Swap, so fps is available without recording.
When on, an event is a clock read and a push under one mutex. The buffer
holds four million events by default; any beyond that are counted as
dropped and reported.

Adding an event:

```rust
let _t = timeline::span(timeline::Cat::D3d, timeline::Track::Host, "SetTexture");
timeline::instant_arg(timeline::Cat::Kernel, timeline::Track::Guest(tid), "signal", "handle", h as u64);
```

Names are `&'static str`; put variable data in the one numeric argument.

## Benchmarks

```
h2 bench --out $TMPDIR/h2bench --label before
# ...change something...
h2 bench --out $TMPDIR/h2bench --label after
h2 bench compare $TMPDIR/h2bench          # the two newest runs
h2 bench compare a.json b.json --threshold 3
h2 bench --only hot-loop --samples 20
h2 bench list
```

| scenario | unit | what it runs |
|---|---|---|
| `hot-loop/alu` | ns/trip | register arithmetic loop in generated code (locals and the back-edge preemption poll) |
| `hot-loop/memory` | ns/trip | inc/load/add-to-memory loop through `h2abi_v1_read`/`write` |
| `hot-loop/call` | ns/trip | call/return per trip: two runner transfers and two dispatches |
| `d3d/pushbuf-frame` | us/frame | `Device::kick_off` draining 1000 inline QUADLIST draws (26 words each) through push-buffer decode, immediate-mode assembly and a recording backend |
| `boot/menu-hold` | fps | boot to the main menu, then hold it for `--frames` frames (wall clock); needs the boot hook below |

The hot loops are synthetic x86: assembled with iced-x86, lifted, emitted
and compiled with clang into `--work` (default `$TMPDIR/h2perf-work`,
reusing emit's object cache), then run by the real runtime. There are no
retail bytes. The push-buffer frame uses synthetic guest memory laid out
like crates/d3d's tests.

Each scenario gets `--warmup` untimed runs (default 2), then `--samples`
timed ones (default 10). A scenario that cannot run (no clang, no boot
driver, no disc) or fails is recorded as skipped, with the reason.
`--quick` shrinks every workload; the tests use it.

A run is written to `<out>/<unix seconds>-<label>.json` and never
overwrites an existing file:

```json
{ "schema": 1, "label": "after", "git": "a41dae4c0ffe", "git_dirty": false,
  "host": { "os": "macos", "arch": "aarch64", "cpus": 12 },
  "started_unix": 1794721821, "samples": 10, "warmup": 2,
  "results": [ { "scenario": "hot-loop/call", "description": "...",
    "unit": "ns/trip", "better": "lower", "samples": [ ... ],
    "stats": { "median": 0, "mean": 0, "min": 0, "max": 0, "stddev": 0, "mad": 0 },
    "extra": { "trips": 200000 }, "skipped": null } ] }
```

`compare` matches scenarios by name and compares medians. A change counts
when it is bigger than both `--threshold` (default 5%) and twice the larger
relative MAD of the two sides, so a noisy scenario needs a bigger move.
Results are `REGRESSED`, `improved`, `same`, or `-` when one side is
skipped or missing. The exit status is 1 if anything regressed, so a script
can gate on it. `--json` prints the rows as JSON.

Adding a scenario: implement `perf::bench::Scenario` (`prepare` untimed,
`measure` one sample) and add it to `perf::scenarios`.

## Boot hook

The boot driver (crates/boot, on the `boot` branch) is not a dependency of
`perf`. Two small changes on the boot side wire it in.

1. **Profiling `h2 run`.** In `boot::run`, after `load_module` and before
   the slice loop, add `let _perf = perf::Session::from_env();`. Declare it
   after `module` so it drops first: Rust drops locals in reverse order,
   and resolution needs the dylib loaded. Then
   `H2_PROFILE=dir H2_TIMELINE=trace.json h2 run disc.iso` profiles a boot.
   The fallback interpreter is already recognised (`boot::fallback::*` is
   the `fallback` category). The kernel, runtime and d3d events come from
   the hooks above. Swap counts frames.
2. **`boot/menu-hold`.** Split `boot::run` into setup and stepping, and
   implement `perf::boot::BootDriver` on the result:
   - `boot_to_menu()`: run until the main menu is up. For example, step
     until `timeline::frames()` passes a menu frame count, or until the
     scripted input (`xapi_glue::InputScript`) says the menu is reached.
   - `step()`: one iteration of today's slice loop: the input script step,
     `m.run(1)`, `xapi::service_input`. Return `false` when the outcome is
     not `SliceLimit`.

   Then register it once at startup, before `perf::cli::bench` or
   `perf::cli::profile` runs. Use the CLI's `main` behind the boot
   feature, or put it in the boot crate's own init:

   ```rust
   perf::boot::set_boot_driver(|args| Ok(Box::new(boot::BenchDriver::new(&args.disc, &args.work)?)));
   ```

   `perf::boot::hold` counts frames with `timeline::frames()` and reports
   frames per wall-clock second.

## Which lane uses what

- **perf** (runtime and emit fast paths): `h2 bench --only hot-loop`
  before and after each change, then `h2 bench compare`. Use
  `h2 profile hot-loop/memory` or `hot-loop/call` to see which accessor,
  lock or dispatch step dominates, and `--by-category` to watch the
  runtime-to-guest ratio. Once the boot hook lands, `H2_PROFILE` on a boot
  gives the same tables for the real game, and the guest-instruction table
  shows which loops are worth a fast path.
- **gfx-perf** (wgpu submission): `--timeline` with
  `--cats frame,d3d,gfx` shows `queue submit` and `present` spans against
  frames and `Swap`. The profile's `gfx` category, and the `d3d` versus
  `gfx` split, separate CPU-side translation from submission.
  `d3d/pushbuf-frame` benchmarks the CPU side without a GPU.
- **codegen-opt** (IR optimisation passes): `hot-loop/alu` and
  `hot-loop/memory` are mostly generated code, so a pass that helps should
  move them. Compare runs built with and without the pass (the scenarios
  rebuild through emit, so the pass applies). The "guest instructions"
  table and the `@0x...` folded leaves show which instructions the time
  goes to.
- **build-perf** (build times): `perf` brings in `addr2line` (with its loader:
  `gimli`, `object`, `memmap2`, `typed-arena`), `rustc-demangle`,
  `serde_json` and `iced-x86` (code_asm), but only into `cli` builds with
  `--features perf`. `timeline` has no dependencies. The hot-loop scenarios use emit's object
  cache under `--work`. Nothing here measures build time yet; a
  `build/emit-shard` scenario would fit the same harness if wanted.
