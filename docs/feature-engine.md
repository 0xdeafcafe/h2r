# Feature engine (design; Phases 0 to 3 built, Phase 4 in part)

Status: design, except Phase 0, the [first slice](#the-first-slice),
Phase 1, the registry, Phase 2 (the differential harness, the leaves,
`locale` sources, `record!` and the views) and Phase 3 (async entries, the
UI views, our own list class; the Video screen on it), which are built ([below](#what-was-built)). No other
crate, type or macro named here exists yet unless the text says so. This page says how the opt-in enhancements (settings, UI
rows and screens, game variants, Forge, Theater, render scale, E3 lighting,
high frame rates, Live extras) should be built so that a new one is mostly
plain Rust against a Rust subsystem, is tested without the game, and cannot
quietly break determinism or another feature.

The primary model is **subsystems rewritten in Rust at clean boundaries**.
Patching individual guest functions (hooks) is kept as the mechanism
underneath and as the bridge for subsystems that are still recompiled.

No game data is in this page. Guest addresses appear only as named points
that another doc here already records. Rust blocks are sketches of an
interface, not code that compiles today.

## Evidence labels

- **Code**: read from this repository at `2c34d20`. File given where it
  matters.
- **Doc**: stated by another page here; follow the link for its evidence.
- **S**: speculation or a design choice. Check it before building on it.

Nothing on this page was established by running the game.

## Summary

[ARCHITECTURE.md](ARCHITECTURE.md) already says where this project is
going: stage 1 is the mechanical recompile, stage 2 is "an engine in
readable Rust that you can extend", reached one guest function at a time
behind the dispatch table, with the recompiled code as the reference. The
enhancements built so far did not take that path. They wrap retail
functions and tell "our" calls from the game's inside the wrapper. The
Controls, Video, FOV and speaker work shows what that costs
([below](#what-exists-today-and-what-it-costs)).

The proposal:

1. **A feature talks to a Rust subsystem, never to a guest address.** It
   registers a screen with `ui`, a string source with `locale`, a patch
   with the map `loader`, a camera modifier with `camera`, a pass with
   `render`. These are ordinary Rust extension points.
2. **Each subsystem has a backing: `Bridge` or `Native`.** Bridge means the
   subsystem's code is still the recompiled original and the extension
   point is implemented with hooks. Native means the subsystem's boundary
   functions are Rust and the extension point is a direct call. A feature
   does not change when its subsystem goes from one to the other.
3. **Subsystems go native in an order set by leverage, risk and size**
   ([the ranking](#subsystem-map-and-ranking)): UI screens and our
   settings first, then the game-start path, the frame loop, the camera,
   map loading, and the rasterizer front-end. The simulation (physics, AI,
   object update, damage) stays recompiled.
4. **The hook registry is the one place that touches dispatch**: named
   points, ordered chains, conflict detection, per-implementation
   switches. Both bridges and native boundaries are installed through it.

| Adding | Today | Bridge (phases 0-1) | Native subsystem |
|---|---|---|---|
| A setting with a menu row | a `*_setting.rs` (300-450 lines), a `Row` variant, edits in four files | one `Def` | one `Def` |
| A settings screen | flags, a list-name trick, a header hook, branches in four shared closures | `ui::Screen` | `ui::Screen`, with its own list class: no retail function involved |
| Our strings | a private id range and a lookup hook per module | `locale.source(..)` | the Rust lookup consults sources |
| Something after each tick | resolve, shadow page, two overrides, pop and restore the return address | `frame.on_tick(..)` | a Rust call in the Rust tick loop |
| A patched map | (not built) a hook after the cache load | `loader.patch(..)` | the Rust loader applies it before publishing |
| A lighting pass | (not built) tag draws from guest observers, rewrite in the view | `render.stage(..)` | the Rust pass loop runs a Rust pass |

The first slice ([below](#the-first-slice), 3 to 4 days) is our settings
as a schema plus the `ui` subsystem's Rust model, with the Video screen
expressed on it. It lands on tests alone. Its follow-up gives our screens
their own Rust list class, which is the first retail-function-free UI.

## What exists today, and what it costs

All **Code**.

**Stage 2 exists in small.** `crates/engine` has Rust implementations of
22 guest functions (datum arrays, tag lookup, locale lookup) with a
calling-convention adapter each, and a differential test against the
lifted original that found zero disagreements over 7.66 M interpreted
instructions ([engine/README.md](engine/README.md)). `crates/hs` has a
native HaloScript VM. Neither is wired into boot:
`engine::runtime_glue::register_all` has no caller, and the differential
feature does not build since `ir` and `lift` moved (the TODO in
`crates/engine/Cargo.toml`). The SDK libraries (D3D, DirectSound, XAPI,
Bink, Live) are the other precedent: whole subsystems replaced in Rust at
their API boundary, which is why 4K, the render thread and our Live
service were possible at all.

**Hooks.** `runtime::Dispatch` holds one override per guest address
(`crates/runtime/src/dispatch.rs`). `register_override` replaces whatever
was there and returns it; nothing reports a collision unless the caller
checks `override_name` first. A hook that runs code after the original
pops the return address, calls the original through a second override at
a private "shadow" address, and returns from a continuation. That dance is
written out six times (`layout_glue::call_then`, `lockstep`,
`live-telemetry`, `level_boot`, `live_notice`, `keyboard_glue`).

- `0x0018f000` (game activation) is wrapped by Live telemetry
  (`live-telemetry/src/layout.rs`) and by the level boot
  (`boot/src/level_boot.rs`). Telemetry installs first
  (`boot/src/lib.rs:516`), the level boot later (`:602`) with a plain
  `register_override` whose "original" is the generated function. By
  reading, `h2 play --level` with Live telemetry on dropped telemetry's
  match-start observer without a message. **Fixed:** the level boot now
  runs whatever override was there (`Dispatch::override_handler`) where it
  ran the generated original, and logs that it chained; a synthetic runtime
  test covers it. Not run in the game. **Phase 1:** both are listeners on
  the `game` bridge's start, one chain on the point.
- `0x00137fe0` (the game tick) is wrapped by Live telemetry and by the
  lockstep observer. Lockstep refuses when the other is there, so per-tick
  hashing and telemetry cannot be on together. Theater wants a third
  observer on the same function ([theater.md](theater.md)). **Fixed in
  phase 1:** both are listeners on the `frame` bridge's tick.
- `layout_glue::install` registers 19 overrides for three features and
  interleaves them: the Controls row-text closure serves four owners, in
  a fixed order, behind four booleans. Its header says why: "a function
  entry takes one override".
- `engine::abi::OVERRIDES` replaces `0x001a02d0` (locale lookup).
  `layout_glue` overrides the same address to serve its strings. The day
  `register_all` is called, one displaces the other. **Fixed in phase 1:**
  the strings are a `before` on the point and the Rust lookup an
  implementation under it (`impl.engine=on`).

**Telling ours from theirs.** The Video screen is a second Controls
screen. Because it runs the Controls class's code, every wrapper on that
class has to ask "is this list ours?" (the datum array's name), "is our
open call in progress?" (a static flag), "is our header being built?"
(another flag). That is the cost of patching a class instead of having
one.

**Addresses, memory, strings.** The same global is a private constant in
several files (tag instances: three; game globals: two; object headers:
two; tick and game time: two). Guest scratch pages sit at hand-picked
addresses (`0xa0fd0000`, `0xa0ff0000` to `0xa0ff9000`, `0xa2f00000` to
`0xa2f41000`); the list is a comment in `boot/src/lib.rs` that is already
incomplete. Two private string-id ranges with hand-assigned sub-offsets
are served through two lookups by two hooks.

**Settings.** `speaker_setting.rs` (452 lines), `fov_setting.rs` (327)
and `video_setting.rs` (417) each implement TOML load, validate, store
with a temp file and rename, an environment override, and "locked" and
"pending" state, each with a different temp file name beside the same
`settings.toml`. The workspace reads 141 distinct `H2_*` variables, most
straight from `std::env` at the point of use, including in `crates/d3d`.

**Tests.** Two guest-memory traits (`engine::GuestMemory`,
`boot::layout_glue::Gm`); live-telemetry uses `runtime::Guest` directly.
The sparse fake that makes `Gm` logic testable is defined six times. The
logic over `Gm` is well covered; the parts that call guest functions
(`row_texts`, `add_items`, `open_video`: nested `call_guest`
continuations) have no tests.

## The model

### Subsystem, boundary, contract

A **subsystem** is a part of the engine with a narrow way in. Rewriting it
means implementing its **boundary** in Rust: the guest functions through
which the rest of the (still recompiled) game enters it. Everything behind
the boundary stops running.

Two things make a boundary usable, and both come from how this XBE was
built (link-time code generation, Doc: [LIFTER.md](LIFTER.md)):

- **Few entries.** Every caller outside the subsystem must come through a
  function entry we can name. Code the compiler inlined into callers is
  not behind any entry.
- **A contract, not private state.** Other code reads engine structures
  directly: 1,387 functions reference the object array, 995 the tag
  entries, 486 game time (Doc: [engine-map.md](engine-map.md) "Size,
  coupling"). So a subsystem's **contract** is the set of guest structures
  outsiders read. The Rust side must keep those bytes as the original
  would. Everything else it owns as it likes.

```rust
pub struct Subsystem {
    pub id: &'static str,                  // "ui", "game", "camera"
    pub grade: Grade,                      // Port | Native
    pub sim: Sim,                          // Host | View | Sim ("Determinism")
    pub entries: &'static [Entry],         // the boundary
    pub contract: &'static [&'static str], // views other code depends on
    pub doc: &'static str,
}
pub struct Entry {
    pub point: &'static Point,             // named guest function
    pub run: Impl,                         // the Rust body
    pub status: Status,                    // Ported | Stub | Passthrough
}
```

### Two grades

| | **Port** | **Native** |
|---|---|---|
| Promise | the same guest memory and registers as the original, call for call | the contract structures hold what the rest of the game needs; behind them the design is ours |
| For | anything the simulation reads or that decides what the simulation does: datums, tags, game time, game options, the frame and tick loops | presentation and host-facing parts: UI screens, camera modes, render passes, HUD, input mapping, our settings |
| Checked by | differential test against the lifted original, per function | unit tests on the contract, and the simulation hash unchanged with it on |
| Extending it | only at explicit extension points that default to retail | freely |

A subsystem can be a Port first and become Native where we want different
behaviour (the camera: retail modes ported, new modes native).

### State

Three stages, as ARCHITECTURE.md's "typed memory in stages":

1. **Guest layout.** The state stays in guest memory in the original
   layout and Rust works through views. Always the first stage, and the
   only one for contract structures.
2. **Mirrored.** Rust owns the state and writes the contract structures
   back at the boundary.
3. **Rust-owned.** Nothing outside reads it. Only reachable when the
   readers have been rewritten too.

### Calling code that is still recompiled

A Rust subsystem has to call down into guest functions that have not been
rewritten. It cannot do that as a plain Rust call: one host thread drives
every guest thread, and a guest call that blocks or is preempted must
unwind to the scheduler ("blocking HLE calls save an explicit return
transfer, never a host stack", Code: `kernel/src/threads.rs`). Today that
is why hooks are nested `call_guest` closures.

Boundary functions that call guest code are therefore `async`: each guest
call is an await, and the installer parks the future in the continuation
slot `call_guest` already provides.

```rust
/// A Rust implementation of a list class's row text function.
pub async fn row_text(cx: Cx, list: ListHandler, item: Item) -> R<Ret> {
    let row = cx.read(|m| item.row(m))?;
    let (label, value) = cx.host(|h| h.ui.texts(list.id(), row));
    widget::set_item_text(&cx, item, 0, label).await?;   // guest calls
    widget::set_item_text(&cx, item, 1, value).await?;
    Ok(Ret::pop(8))
}
```

- The compiler builds the state machine that `row_texts`/`next_text`
  spell out by hand today.
- `cx.read`/`cx.write` take guest memory for the closure only, so no
  guard is held across an await. The future is `Send + 'static`.
- A test polls the same future with a table of fake callees and asserts
  the calls. The differential harness polls it with the IR interpreter
  running the real callees. This is what makes the currently untested
  code testable.
- Functions that call nothing in the guest stay plain `fn`, as the 22
  engine adapters are.

Theater's note still holds: a parked future, like a parked closure, cannot
be cloned, so whole-machine forks need every thread at a point with none
pending ([theater.md](theater.md) "In-memory machine forks").

### Coexisting with the original

- **A switch per entry and per subsystem.** `impl.ui = rust`,
  `impl.ui.controls_row_text = original`. An entry set to `original` is
  simply not overridden (`Dispatch::set_enabled` exists). ARCHITECTURE.md
  asked for this so a regression bisects to one function.
- **Unit tests** on the fake guest: every entry, always.
- **Differential tests** (Port grade): the original, lifted and run on
  the IR interpreter, against the Rust entry, from the same guest state
  and registers; compare return target, registers outside the clobber
  list, and all of memory. This is `crates/engine/tests/differential.rs`,
  generalised to async entries (guest calls served by the interpreter).
  It needs the owner's XBE, is pinned by hash and skips without it.
- **Call captures** for state too rich to synthesise: at the n-th call of
  a point, dump registers and guest RAM (the mechanism behind
  `H2_BOOT_DUMP_RAW`), then run the differential test from that state.
  Captures hold game memory. They stay in the user's data directory and
  are never committed.
- **Run-level comparison**: the same input replay with the entry on
  `original` and on `rust`, compared with `h2 lockstep`.

One caveat on the last (S, to check against RUNTIME-CONTRACT.md section
4): the logical clock and the time slices are counted in guest memory
accesses. A Rust entry does not make the accesses the generated code made,
so even a perfect Port can shift when a slice ends. Whole-RAM traces may
then differ in time-dependent state while every call agrees. The
per-call differential test is the proof for a Port; run-level comparison
should use the simulation hash Theater defines (the game-state arena only)
once it exists, and `exact` fingerprints include the switch set (see
"Determinism").

### Extension points and backings

A feature sees only this:

```rust
pub trait Feature: Send + Sync + 'static {
    fn manifest(&self) -> Manifest;       // id, title, default, Sim class, doc
    fn build(&self, b: &mut Builder) -> Result<(), BuildError>;
}

b.setting(&DEF) -> Setting<T>             // our settings
b.ui().screen(..), b.ui().extend(..)      // screens and rows
b.locale().string("Video") -> StringId    // text
b.frame().on_tick(..), .on_present(..)    // frame events
b.game().on_start(..), .variants(..)      // game start, variants
b.loader().patch(..), .on_loaded(..)      // map load
b.camera().modifier(..), .mode(..)        // camera
b.render().observer(..), .stage(..)       // rendering
b.tape().stream(..)                       // recording
b.raw().hook(points::P)..                 // escape hatch: a hook
```

Each of `ui`, `locale`, `frame`, `game`, `loader`, `camera`, `render` is
a subsystem facade. Behind it is either a bridge (hooks on retail
functions, the code that exists today, moved) or the native subsystem.
`b.raw()` is for research and for the gaps; a feature that needs it is a
sign of a missing extension point.

## Subsystem map and ranking

Sizes are [engine-map.md](engine-map.md)'s survey windows (analyzer
function starts and their address span): measured counts over guessed
ownership, not module boundaries. "Leverage" is how many of the wanted
features get easier. "Fidelity risk" is the cost of being subtly wrong.

| # | Subsystem | Size (fn / bytes) | Leverage | Fidelity risk | Boundary | Verdict |
|---|---|---|---|---|---|---|
| 0 | Leaves: datums, tag lookup, locale lookup | 30 / 3.1 K, 6 / 0.6 K | medium: views, strings | low (already differential-tested) | clean, done | **turn on**; Port |
| 1 | UI: screen classes | per screen, small; class contract traced for Controls | very high: settings, every new screen, Forge and Theater menus | low: presentation, runs outside the tick | clean per class: loader, constructor, list handler | **first**; Native |
| 2 | Our settings (host side) | none in the guest | very high | none | ours | **first**; Native |
| 3 | Game options, variants, game start | about a dozen functions (not sized) | high: level boot, variants, Forge and map-pack selection, films, Live start | medium: the options block feeds the simulation | clean: the `0x1118`-byte options block is the contract | **second**; Port, then extension points |
| 4 | Frame loop, tick loop, game time | 10 / 1.7 K, 9 / 1.1 K | high: every event becomes a Rust call; Theater transport, high fps | medium: order and gates must match; timing is exact | clean: three orchestrators and the time budget | **third**; Port |
| 5 | Camera, director, observer | 76 / 20 K | high: FOV, Theater cameras, interpolation, widescreen | low to medium: view state, float math | clean: the observer result and view records are the contract | **fourth**; Port retail modes, Native new ones |
| 6 | Map and cache loading, tag publication | 46 / 8 K | high: Forge, content packs, custom maps | medium: must publish identical structures (995 readers) | medium: async I/O and cache slots are entangled | **fifth**; Port the result, Native the loader |
| 7 | Rasterizer front-end above D3D | 641 / 265 K | very high: 4K, E3 lighting, high fps natively | medium visually, low for the simulation if guest-visible reads keep coming from the reference | layered: view loop, pass loop, geometry submission | **sixth, in stages**; Native |
| 8 | HUD | not surveyed | medium: view-resolution HUD, hide for Theater, widescreen | low | unknown: its draw entry is a research point | after 7's pass loop |
| 9 | UI widget system and UI renderer | 582 / 57.6 K (window; screen classes outside it not counted) | medium: new widget kinds, resolution-independent menus | medium: every retail screen must still look right | wide: nine widget classes, draw list, transitions | later; the screen-class contract (1) gets most of the value |
| 10 | Input mapping (pad to actions) | small (not sized) | medium: layouts and rebinding natively, mouse look, action-level recording | medium to high: it is the simulation's input | the per-player action table and control record | optional; Native above the action table, recompiled below |
| 11 | Player profile and preferences | 93 / 11.3 K | medium: profile import/export, Live sync | medium: signed saves, untraced fields, fields read inline everywhere | only the load/save edge is clean | optional; our settings stay host-side |
| 12 | Saves, checkpoints | part of 11; snapshot `0x00124490` | medium: Theater keyframes | high: raw snapshots of game state | the arena table | **view only**: read the arena, do not rewrite revert |
| 13 | HaloScript VM | 65 / 12.8 K; Rust VM exists (`crates/hs`) | medium: scripting for Forge and custom games | high: campaign scripts are simulation | clean: scheduler and evaluators | wire in behind a switch only with simulation-hash evidence |
| 14 | Sound front-end | 164 / 42 K | low (routing and devices are at the DirectSound HLE) | unknown: check that nothing in the simulation reads it | - | stay recompiled |
| 15 | Networking above the SDK | 2,032 / 401 K | low: Live features come from the HLE boundary | very high: replication is simulation | - | stay recompiled |
| 16 | Objects, units, weapons, damage | 1,194 / 415 K | low for these features | very high | - | **stay recompiled** |
| 17 | AI | 426 / 112 K (excludes squads, pathfinding) | low | very high | - | **stay recompiled** |
| 18 | Physics integration, Havok, collision | 97 / 21 K plus the library | low | very high: float behaviour, super bouncing ([high-framerate.md](high-framerate.md) section 4) | - | **stay recompiled** |

Why the simulation stays: films and the lockstep harness depend on it
running the same instructions; its floating-point results are the bar
ARCHITECTURE.md puts last ("physics and AI, last"); it is three quarters
of the engine by size; and none of the wanted features needs it changed.
Forge changes what the simulation is *given* (scenario blocks), not how
it runs. Views over its structures (players, objects) are enough.

The SDK libraries are already Rust and are not in this table.

## The top candidates

### 1. UI: screen classes in Rust

**What is known (Doc: [settings-menu.md](settings-menu.md) "Video
screen", [ui.md](ui.md), [engine-map.md](engine-map.md) "UI tag
walkers").** A screen is opened by calling its *loader* with a request
(`0x00149f1e` builds the request and calls the loader). The loader
allocates the screen (`0x001a47fd`), runs the screen's constructor (base
screen `0x00230451` with a screen id, which finds the `wgit` tag for the
layout), and pushes it on the screen stack (`0x00147f6d`). A list screen
embeds a list handler with its own vtable and constructor; the handler's
row-text and row-chosen functions are where a screen's behaviour lives.
The generic parts (widget tree, layout, draw list, transitions, pad
navigation) are shared code.

**Boundary.** The screen class: loader, constructor, and the list
handler's functions. That is the contract the generic UI code calls, and
it is where the retail game puts per-screen code too.

```rust
pub trait ScreenClass: Send + Sync {
    fn id(&self) -> &'static str;
    fn template(&self) -> Template;             // which retail layout to reuse
    fn title(&self) -> StringId;
    fn rows(&self, cx: &UiCx) -> Vec<Row>;
    fn chosen(&self, cx: &mut UiCx, row: usize) -> Chosen; // Stepped | Open(..) | Close | Nothing
}
```

Most screens never implement it: `ui::Screen` is a declarative
`ScreenClass`.

```rust
ui::Screen::new("video").title("Video").rows_from_settings();
ui::extend(ui::lists::CONTROLS)
    .row(ui::Row::setting("pad_speaker.enabled"))
    .row(ui::Row::open("video").label("Video"));
```

**Three steps, each shippable:**

| Step | Backing | What runs | Retail functions patched for our screens |
|---|---|---|---|
| a. Bridge (first slice) | hooks on the Controls class, as today, but driven by the `ui` model | today's guest behaviour exactly | the same eight as today |
| b. Own list class | our loader at an address the engine owns; the Controls constructor for the layout; then the handler's vtable pointer swapped for our copy whose row functions are Rust | Rust for every row of our screens | **none**: no "is it ours" tests, no open-in-progress flags |
| c. Retail classes ported | the Controls and Button Layout list handlers themselves in Rust (Port), with extension rows as data | Rust for retail rows too | none; the added-row hooks go |

Step b needs two facts read from the XBE before it is built (S): which
vtable slots (or stored pointers) reach row text and row chosen from the
shared list code, and that the header id can be set on the screen after
construction instead of swapped mid-build. Step c needs the retail
handlers' row logic read in full; they are small.

**Coexistence.** Step b touches only screens that did not exist in
retail, so there is nothing to diff against: unit tests on the model, the
async entries against fake callees, and the user's play-test. Step c is
Port grade: differential tests of each handler function from call
captures taken on the Controls screen.

**Unlocks.** A settings screen is a struct. Forge's variant picker, the
Theater film list, a Live server browser and the "Enhancements" root are
list screens, so they need nothing more. Anything that is not a list (a
palette, a transport bar, a slider) goes to the host overlay until
subsystem 9; the same `ui::Screen` declaration can target either
(`Host::Guest` or `Host::Overlay`).

**Limits to check in the game (S).** A screen built from the Controls
layout has six item widgets; more rows need the added-item path, which is
proven on Controls itself but not on a second screen. A screen opened
from one of ours should work by the same call; not tried.

### 2. Our settings

Not a guest subsystem: the retail game has no concept of these options.
`crates/settings` (new, depends on `toml` only) is the whole of it. See
[Settings](#settings). It is ranked with the subsystems because it is the
other half of "adding a setting is one declaration", and it has no
fidelity risk at all.

### 3. Game options, variants and the game-start path

**What is known (Doc: [level-boot.md](level-boot.md),
[community-gametypes.md](community-gametypes.md),
[live-gamestate.md](live-gamestate.md)).** A game starts from a
`0x1118`-byte options block: cleared and defaulted by `0x00138110`,
players added by `0x00138640`, checked by `0x00138180`, queued by
`0x0018e790` for the main loop's pending-transition step. The variant is
a `0x130`-byte record inside it, built from defaults by `0x0019d220` and
from the built-in table by `0x00149f88`. `level_boot.rs` already fills
the block field by field in Rust and calls those functions;
`community_variants.rs` already builds variants symbolically.

**Boundary.** `GameOptions` and `Variant` views are the contract. The
entries are the five functions above plus the main-menu request
(`0x0018f1c0`). All Port grade: the block decides what the simulation
does, so a retail start must produce the same bytes.

```rust
impl Game {
    pub fn options_new(&self, m: &mut dyn GuestMemory, at: u32) -> R<GameOptions>;
    pub fn queue(&self, cx: &Cx, opts: &GameOptions) -> ..;
    pub fn check(&self, m: &dyn GuestMemory, opts: &GameOptions) -> R<Verdict>;
}
b.game().on_start(|g: &GameStart| ..);          // was: after GAME_ACTIVATE
b.game().variants(|list: &mut Variants| ..);    // was: a hook before BUILTINS_LOAD
b.game().request(StartRequest::campaign("..", Difficulty::Heroic));
```

**Coexistence.** Each entry is small and nearly pure (memory in, memory
out), which is the best case for the differential test. `OPTIONS_CHECK`
in Rust gives readable refusals where the original gives a byte.

**Unlocks.** Starting any game from Rust: the level boot, a custom-game
lobby, "play this film", a Live match. Variants become Rust data
validated in Rust, so a content pack can carry them. A Forge variant or a
map-pack map is selected by filling the same block.

### 4. Frame loop, tick loop, game time

**What is known (Doc: [engine-map.md](engine-map.md) "Frame phases").**
`main_loop_iteration` `0x0012b450` calls the phases in a fixed order
behind a few gates; `game_update_ticks` `0x00137f70` loops over the ticks
that `game_time_budget_ticks` `0x00146660` allows; `game_tick`
`0x00137fe0` calls the per-tick phases and increments the tick count.
These are orchestrators: almost no arithmetic outside the time budget.

**Boundary.** Those four functions, Port grade: the same children, in the
same order, under the same gates, each child still recompiled and reached
by an await. Game time's `0x24`-byte record is the contract.

**Coexistence.** Differential tests of the budget function (a fractional
accumulator: bit-exact or it is wrong). For the orchestrators, the call
sequence is the behaviour: the async test records the callee order and
compares it with the lifted original's for the same gates.

**Unlocks.** `Tick`, `Present`, "before input" and "before render" stop
being after-hooks with a shadow entry and a continuation and become Rust
call sites, so telemetry, the lockstep hash and Theater's recorder are
three listeners in a loop. Theater's pause, step and seek and the
high-fps design's "render between ticks" get a place to stand that is not
a wrapper. Until then `frame` is a bridge over the registry's one chain
per point, which already fixes the telemetry and lockstep collisions.

### 5. Camera, director, observer

**What is known (Doc: [engine-map.md](engine-map.md) "Rendering, camera,
and UI", [settings-menu.md](settings-menu.md) "Field of view").**
`director_update` `0x001554b0` dispatches a camera callback per local
player (first person `0x0023c110`, scripted `0x0016c840`, following,
dead); `observer_update` `0x0016f280` smooths; `0x00171d90` produces the
observer result (horizontal and vertical field of view, scale);
`0x0002f800` turns that into the render camera. It runs in the frame-time
phase, after the ticks.

**Boundary.** In: the director's per-player dispatch. Out: the observer
result and render camera records, which the renderer and culling read.

```rust
b.camera().modifier(|cam: &mut FirstPerson, cx| cam.fov = ..);   // FOV
b.camera().mode("theater.free", FreeCamera::new());              // a new mode
b.camera().on_view(|v: &ViewCamera| ..);                         // for interpolation
```

**Coexistence.** Retail modes are Ports checked differentially from call
captures; floats must match in bits or carry a documented tolerance.
New modes are Native and exist only when selected. Bridge backing today:
the after-hook on `0x00187130` that the FOV row uses.

**Unlocks.** FOV without a clamp patch. Theater's flying, follow and
orbit cameras as Rust types. The per-view camera that frame
interpolation and E3 lighting both need, handed over by the code that
computed it instead of rediscovered from shader constants.

**Risk.** Whether any camera path writes game-state memory that the
simulation reads ([theater.md](theater.md) risk 4). The simulation hash
is the test.

### 6. Map loading and tag publication

**What is known (Doc: [engine-map.md](engine-map.md) "Cache files, tags",
[cache-slots.md](cache-slots.md), [map-format.md](map-format.md)).**
`crates/map` reads and writes cache files; `crates/tags` decodes most tag
groups; `load_requested_scenario_cache` `0x0018eaa0` publishes the
scenario pointer at `0x004e0350`.

**Boundary.** In: the cache-load request and its progress polling. Out:
the published tag table, the scenario and globals pointers, and the tag
data itself, byte for byte where nothing patches it.

```rust
b.loader().patch(|map: &mut LoadedMap| -> Result<(), Refusal> ..); // before anything reads it
b.loader().on_loaded(|map: &LoadedMap| ..);
```

**Coexistence.** The published memory after a load is one big value:
compare the tag region the Rust loader produces with the one the original
produced for the same map, from a capture. That is Port grade on the
result while the loader's inside (I/O, cache slots, ordering of reads)
is ours. Bridge backing today: an after-hook on `0x0018eaa0` (forge.md
2.2, marked S there).

**Unlocks.** Forge and content packs apply to bytes we are about to
publish instead of racing the engine's first read. Growing a block is an
allocation in our loader, not a search for slack. Pack hashes are checked
against data we already hold. Custom and map-pack maps become a loader
feature. ARCHITECTURE.md lists this as the first thing "done" means.

### 7. Rasterizer front-end

**What is known (Doc: [engine-map.md](engine-map.md),
[e3-lighting.md](e3-lighting.md) "Phase 0 findings",
[shader-instances.md](shader-instances.md)).**
`render_frame_and_present` `0x00223240`, `render_game_views`
`0x002234e0`, `render_views` `0x0002b5d0`, `render_game_view`
`0x0002b790`, `render_scene_passes` `0x0002bcd0`, the geometry choke
point `0x00041040`, shader draw begin `0x0001bbf0`, the per-light pass
`0x00031170`.

**Stages, top down.** Each is useful alone.

| Stage | Rust owns | Still recompiled | Gives |
|---|---|---|---|
| 7a | the view loop and the pass loop (orchestrators) | every pass body | pass and view identity for every draw, known rather than inferred; camera per view |
| 7b | chosen pass bodies: dynamic lights, shadows | geometry submission, materials | E3 lighting as a Rust pass |
| 7c | targets and post-processing | - | view-resolution effects without the reference/view fill tricks |
| 7d | geometry submission and materials | - | the renderer ARCHITECTURE.md calls "native" |

**Coexistence.** The reference backend keeps answering every read the
guest makes ([high-resolution.md](high-resolution.md)). Draw-stream
comparison is the test: with `RecordingBackend`, the calls the Rust pass
loop produces equal the original's for the same frame capture. Until 7a,
`render` is a bridge: observers on those addresses tag the draws
([Rendering](#rendering)).

**Unlocks.** 4K, lighting and interpolation stop being reconstructions
from a flat draw list.

**Measured first (built).** `render-timing` (`crates/feature/src/
render_timing.rs`, off by default) puts observers on both sides of the
four orchestrators, `render_frame_and_present` and the per-light pass, and
reports the front-end's guest time as the frame stats' `render` phase with
an exclusive split per stage ([frame-budget.md](frame-budget.md), "Render
front-end versus simulation"). It answers how much of the game thread's
guest time 7a-7d can move at all.

**7a status.** `render_views` is ported (`engine::render_loop`, placed by
the `render-loop` feature under `impl.render.views`, off by default) and
agrees with the lifted original call for call; the read of the other
orchestrators and the next steps are in [render-loop.md](render-loop.md).

### The rest, briefly

- **Input mapping (10).** Native above the per-player action table: the
  button layouts become a table our code fills rather than a rewrite
  after the original built it. Player control (aim, look acceleration)
  stays recompiled.
- **Profile (11).** Own the load and save edge only. Our settings never
  go into the signed profile ([settings-menu.md](settings-menu.md)).
- **HaloScript (13).** The VM exists. Wiring it is a Port of simulation
  code: do it only when the simulation hash can prove a campaign level
  runs the same.

## The mechanisms underneath

### Hooks

One registry places every override: native entries, bridges, and raw
hooks.

**Points.** One table of named guest locations in `engine::points`, keyed
by the 1.0 address. It is the anchor list
[title-updates.md](title-updates.md) section 3.3 asks for.

```rust
pub struct Point {
    pub name: &'static str,
    pub base: u32,            // address in the base (1.0) image
    pub sig: Sig,             // where arguments and the result live; the `ret n`
    pub class: Class,         // Required | Optional
    pub hot: bool,            // called thousands of times a frame
    pub doc: &'static str,    // the page that records the evidence
}
points! {
    PLAYER_FOV @ 0x0018_7130: fn(user: eax) -> f32 @ xmm0, ret 0;
    CONTROLS_ROW_TEXT @ 0x002c_8548: fn(list: ecx, item: stack 1) ret 8;
    GAME_TICK @ 0x0013_7fe0: fn(), ret 0;
    LOAD_SCENARIO_CACHE @ 0x0018_eaa0: opaque;   // signature not read
}
globals! { GAME_GLOBALS @ 0x004e_6948: ptr; SCENARIO @ 0x004e_0350: ptr; }
```

**Kinds.**

| Kind | May write guest state | Runs the original | More than one per point |
|---|---|---|---|
| `implement` (a subsystem entry) | everything | never | no |
| `observe_before` / `observe_after` | no (`&dyn GuestMemory`, no CPU) | always | yes |
| `before` | arguments, memory | unless it returns early | yes, ordered |
| `after` | result, memory | already ran | yes, reverse order |
| `.at(site)` (one call site, by return address) | as its kind | as its kind | yes |

Typed access comes from the signature: `c.arg::<u32>(0)`,
`c.ret_f32()`, `c.set_ret_f32(..)`. An `opaque` point takes observers and
after-hooks only.

**One chain per point.** The installer registers one override per point:
`observe_before`, `before` (outermost first), the implementation or the
generated original, `after`, `observe_after`. The shadow entry and
continuation are used only when something runs after the original.
Order: `First`/`Default`/`Last`, then declared `.after_feature(..)`,
then id.

**Conflicts.** `Plan::build` is pure (no runtime, no guest) and refuses,
naming both parties and the point:

- two implementations of one point, or a native subsystem and a bridge
  both claiming it;
- two mutating hooks with no declared order between them;
- a `before`/`after` on a point a native subsystem implements, unless
  that subsystem exported the point as hookable (it should offer an
  extension point instead);
- an order cycle; a typed hook on an `opaque` point;
- duplicate setting keys, environment names, screen ids, content kinds;
- an override already on the point that the plan did not place (today's
  silent displacement becomes an error).

It warns when a hook lands on a `hot` point: an overridden entry leaves
the direct-call fast path (Code: `runtime/src/direct.rs`).

**Switches.** Every feature has `features.<id>`; every subsystem entry
has `impl.<subsystem>[.<entry>]`. Off means not in the plan. A feature
marked `Toggle::Live` can also be switched while running (one atomic
check per link).

**Version key.** `Plan::build` takes `&dyn ImageAddrs`: identity for the
base image, the local address map for a title update. A point resolves
to `At(addr)`, `Skip` or `Refuse`. A native subsystem with a `Required`
entry missing falls back to `original` for the whole subsystem and says
so; a feature whose extension point has no backing is turned off with
the reason. Structure layouts go through `engine::view::Layouts::for_image`.

**Resources.** `b.guest_alloc(len, perms)` hands out guest scratch memory
from one reserved range; `b.locale().string(..)` hands out string ids
from one private range, served by the Rust locale lookup (subsystem 0)
or, until that is on, by one hook on each of the two lookups.

**Visibility.** `h2 features plan` prints one line per point with its
chain and which subsystems are native. A test pins that text for the
default configuration.

### Guest data access

**One memory trait.** `engine::GuestMemory`: object-safe,
`Result`-returning, implemented for the runtime's guest and for
`FlatMemory`. Add `f32`, `bytes`, `cstr`, `utf16`, `Vec3`.
`layout_glue::Gm` becomes an adapter, then goes. Reads from Rust are not
charged to the guest's access count (Doc: [e3-lighting.md](e3-lighting.md)
"Observer rules").

**Views.** An address plus accessors; the layout is constants in one
place with the evidence beside them, as `engine::render::lights` does.

```rust
record! {
    /// forge.md 1.2: scnr+0x120, stride 0x90.
    pub struct NetgameEquipment: 0x90 {
        flags: u8 @ 0x00,
        kind: u8 @ 0x14,
        position: Vec3 @ 0x40,
        orientation: Vec3 @ 0x4c,
        item: TagRef @ 0x58,
    }
}
// generates NetgameEquipment::at(addr), .position(m)?, .set_position(m, v)?,
// offset constants and a FakeGuest builder.
pub struct Block<T: Record> { .. }   // count + address: get, iter, swap_remove
```

| View | Built on | Readers to fold in |
|---|---|---|
| `Tags`, `Tag` | `engine::tags` | `fov_row`, `community_variants` |
| `Datums<T>` | `engine::datum` | live-telemetry |
| `Players`, `Player`, `Objects`, `Object` | `Datums`, [objects.md](objects.md) | live-telemetry, `fov_row` |
| `GameGlobals`, `GameOptions`, `Variant` | [level-boot.md](level-boot.md) | `level_boot`, live-telemetry |
| `Scenario`, `Block<T>` | [forge.md](forge.md) 1.2 | none yet |
| `Widget`, `ListHandler`, `Item`, `ScreenTag` | [ui.md](ui.md) | `layout_glue`, `speaker_row`, `video_screen` |
| `PlayerControl`, `ObserverCamera` | [settings-menu.md](settings-menu.md) | `fov_row` |
| `Lights`, `Layers` | `engine::render` | exists |
| `Arena` (game-state allocations) | [checkpoints.md](checkpoints.md) | Theater's hash and keyframes |

A field marked "likely" is range-checked by its accessor and yields
`None` rather than a wrong value. Views never cache.

**The fake guest.** `engine::testing::FakeGuest`: sparse pages, faults on
unmapped reads, builders generated by `record!`. It replaces the six
private fakes.

```rust
let mut g = FakeGuest::new();
g.tag(7, |t| t.unit_camera_fov(1.2217));
g.player_control(0).unit(g.object_with_tag(7));
let scnr = g.scenario(|s| s.netgame_equipment([
    NetgameEquipment::build().position([1.0, 2.0, 3.0]).kind(2),
]));
```

`FakeCall::at(point)` builds registers and a stack for a sync entry;
`FakeCallees` answers the awaits of an async one and records them.

### Settings

`crates/settings`. `cli`, `app`, `boot`, `d3d` and `gfx` can all depend
on it.

```rust
pub struct Def {
    pub key: &'static str,        // "graphics.fov": table and key in settings.toml
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,               // Bool | Int{valid, steps, unit} | Choice | Float | Path | Text
    pub default: Value,
    pub env: Option<&'static str>,
    pub flag: Option<&'static str>,
    pub apply: Apply,             // Live | NextMap | Restart
    pub scope: Scope,             // Host | View | Sim
    pub tier: Tier,               // User | Advanced | Diagnostic
    pub menu: Option<Menu>,       // screen id and order
}
impl Store {
    pub fn load(defs: &[&Def], file: Option<&Path>, env: &Env, flags: &Flags)
        -> (Store, Vec<Problem>);
    pub fn handle<T: SettingType>(&self, key: &str) -> Setting<T>;
    pub fn set(&self, key: &str, v: Value) -> Result<Outcome, SetError>;
    pub fn step(&self, key: &str) -> Result<Outcome, SetError>;    // a row press
    pub fn state(&self, key: &str) -> State;   // running, saved, source, locked
    pub fn on_change(&self, key: &str, f: impl Fn(&Value) + Send + Sync + 'static);
}
```

Rules, from [settings-menu.md](settings-menu.md) "Where settings persist"
and the three implementations that exist:

- Precedence: flag, environment, file, default. A value from a flag or
  the environment is locked: the row shows it and a press does nothing.
- `steps` separates what the file may hold from what a row steps through
  (FOV: 30 to 150 in the file, 70 to 120 by 5 on the row).
- A bad value is a `Problem` naming the key, reported once; the next
  source is used.
- One writer: validate, write the whole file atomically keeping unknown
  keys, then report. The file format stays as it is.
- `Restart` and `NextMap` settings keep running and saved values apart;
  the row shows "(restart)".
- **A `Setting<T>` read is one atomic load.** Code that holds guest
  memory reads settings; three comments in `layout_glue.rs` warn that a
  setting's lock must never be taken there. Handles make that impossible
  to get wrong. `on_change` runs after the store's lock is released, and
  the `ui` subsystem calls `set` only after dropping guest memory.

| Surface | Shows | How |
|---|---|---|
| In-game menu | `User` tier with a `menu` | `ui::Screen::rows_from_settings` |
| Host overlay | `User` and `Advanced` | a panel in `app` over the same `Store` and `ui::Screen`s |
| CLI | everything | `h2 settings list [--all] / get / set`, `h2 play --set key=value` |
| Environment | every `Def` with `env` | automatic |
| Docs | everything | `h2 settings doc`; a test fails when the committed table is stale |

The 141 `H2_*` variables sort by `tier`: about fifteen user settings,
about fifteen renderer A/B switches, the rest diagnostics and build
inputs (registered so they are listed, never shown in a menu). Crates
below boot stop reading the environment: `d3d` exports its `Def`s and
takes a config built from the `Store`. `impl.*` and `features.*` switches
are settings like any other.

### UI

Covered under [candidate 1](#1-ui-screen-classes-in-rust). The pieces:

```rust
pub enum RowKind {
    Setting(&'static str),              // label + value; press steps it
    Open(&'static str),                 // opens a screen
    Action(Arc<dyn Fn(&mut UiCx) + Send + Sync>),
    Custom(Arc<dyn RowBehaviour>),
}
impl Model {                            // pure; the same under every backing
    pub fn count(&self, list: ListId, retail: u32) -> u32;
    pub fn row(&self, list: ListId, index: i16) -> Option<&Row>;
    pub fn texts(&self, row: &Row, store: &Store) -> [(TextSlot, StringId); 2];
    pub fn press(&self, row: &Row, store: &Store) -> Chosen;
}
```

A `Setting` row needs a string id per value it can show; the model
enumerates them from `Kind` (the FOV row already registers one string per
degree). More than 256 values is refused for the guest host and shown in
the overlay. What stays hand-written: the Button Layout screen's pane
cloning (`extend_panes`) is tag surgery and becomes a `loader` patch on
the UI tags plus a `Custom` row.

### Content packs

Data that changes what the game loads. Never game bytes: a pack names
things on the user's own disc and carries hashes to check it applies.

```toml
[pack]
id = "lockout-snipers"
kind = "map-variant"        # map-variant | game-variants | strings | maps
format = 1

[base]
map = "lockout"
scenario_sha256 = "..."     # of the scenario tag on the user's disc

[[edit]]
block = "netgame_equipment"
op = "set"
index = 3
item = { group = "itmc", path = "..." }   # a reference, resolved on load
position = [1.5, -2.0, 0.25]
```

```rust
pub trait ContentKind: 'static {
    const KIND: &'static str;
    type Pack: DeserializeOwned + Send + Sync;
    type Target<'a>;
    fn validate(pack: &Self::Pack, target: &Self::Target<'_>, m: &dyn GuestMemory)
        -> Result<Applied, Refusal>;      // on a copy; yields the exact byte edits
    fn apply(applied: Applied, m: &mut dyn GuestMemory) -> Result<(), MemFault>;
}
```

- Identity is the SHA-256 of the canonical JSON form (`live-proto::jcs`),
  as [forge.md](forge.md) 2.1 proposes.
- The engine owns discovery (`<data>/packs/`, `--pack`, Live), parsing,
  hashing, the enabled set and the fingerprint. The feature owns
  `validate` and `apply`. The split is forge.md's "apply to a copy first;
  if any edit fails, nothing is written" made structural.
- Packs are data: no hook, no code.
- Hashes of the user's disc data may sit in a pack the user shares; none
  is committed here, and tests use synthetic scenarios and hashes (Doc:
  [title-updates.md](title-updates.md) 3.1).

| Kind | Subsystem | Bridge | Native |
|---|---|---|---|
| `map-variant` | `loader` | patch after `0x0018eaa0` returns; in place only | patch before publication; growth is an allocation |
| `game-variants` | `game` | append before `0x00149f88` reads the table (as `community_variants.rs`) | the Rust variant list |
| `strings` | `locale` | lookup hooks | the Rust lookup's sources |
| `maps` | `loader` | blocked on [map-pack.md](map-pack.md) section 6 | the Rust loader's map list |

### Rendering

**What exists (Code).** `d3d::Backend` is the seam: the device resolves
each guest draw into a `ResolvedDraw`. Wrappers compose by generics:
`DiagBackend`, the sweep diagnostic, `ScaledBackend<R, V>` (reference
plus view), `ThreadedBackend`. `GfxBackend` has one `on_present` closure
and merge-pass setters read from the environment.

**The plug-in point** is the same under both backings:

```rust
/// Side data on every ResolvedDraw. The reference backend ignores it.
pub struct PassTag {
    pub view: Option<ViewId>,
    pub layer: Option<engine::render::layers::Role>,
    pub object: Option<ObjectKey>,
    pub shader: Option<ShaderKey>,
    pub frame_tick: u32,
}
/// Sees the stream. Changes nothing. Works on any backend.
pub trait FrameObserver: Send {
    fn draw(&mut self, d: &ResolvedDraw, tag: &PassTag) {}
    fn present(&mut self, f: &FrameInfo) {}
}
/// Runs in the view only. May rewrite, add or drop view-side work.
pub trait ViewStage: Send {
    fn slot(&self) -> Slot;   // Geometry | Lighting | PostScene | Composite | Pacing
    fn begin_frame(&mut self, f: &mut FrameCtx) {}
    fn draw(&mut self, d: ViewDraw, tag: &PassTag, out: &mut Vec<ViewDraw>) { out.push(d) }
    fn end_scene(&mut self, f: &mut FrameCtx) {}
    fn compose(&mut self, f: &mut FrameCtx) {}
    fn present(&mut self, f: &mut FrameCtx) -> PresentPlan { PresentPlan::Once }
}
```

- **Where tags come from.** Bridge: read-only observers on the render
  entries (shader draw begin, the per-light pass, `render_game_views`)
  set the device's current tag. Native (7a on): the Rust pass loop sets
  it, because it knows which pass it is running.
- **Observers** replace the diagnostic wrappers (draw log, capture, the
  sweep log), removing two layers of backend generics.
- **Stages** are ordered by slot, then declared order; two rewriting
  stages in one slot must declare their order.

| Feature | Bridge | With 7a-7b |
|---|---|---|
| Render scale | view configuration | the same |
| E3 lighting | tags + a `Lighting` stage that replaces lit draws | a Rust lighting pass |
| 60/120 fps | tags + a `Geometry` stage substituting interpolated constants + `Pacing` | the pass loop re-run with interpolated cameras and poses |
| Screenshots, clips | a `FrameObserver` on the view | the same |

**Invariants** ([high-resolution.md](high-resolution.md) "Determinism",
[e3-lighting.md](e3-lighting.md) "Opt-in"): stages run in the view only
and hold no guest handle; the reference backend answers every guest read
and never sees a stage; a failing stage is counted and skipped for the
frame; a `RecordingBackend` test proves the device's call list does not
depend on which stages are registered.

### Recording and replay

Every place something from outside reaches the guest is a named boundary,
and the HLE code there asks a tape for the answer.

```rust
boundaries! { PAD_READ, DEVICE_CHANGES, NET_RECV, NET_SELECT, RANDOM,
              LIVE_ANSWER, EPOCH, SETTING_CHANGE }
pub trait Tape: Send + Sync {
    /// Off: run `real`. Record: run it and log by call index. Replay: return the log.
    fn io(&self, b: Boundary, real: &mut dyn FnMut() -> Vec<u8>) -> Vec<u8>;
    /// A value that must match on replay (a send's hash, a RAM hash).
    fn check(&self, s: Stream, value: u64) -> Result<(), Desync>;
}
```

`xapi::tap` is this for pad reads already; [theater.md](theater.md) "What
the recorder captures" lists the rest. A film or a lockstep trace is a
tape plus a header: image stamp, fingerprint, start-state manifest.

- Features get `frame.on_tick` (hash, keyframe, per-tick actions are
  three listeners), `tape.stream("actions")`, and a view `FrameObserver`.
- Settings of `View` or `Sim` scope and enabled packs are start state:
  they go in the header.
- While a tape records or replays, those settings are locked ("locked:
  recording") unless the feature opted in to `SETTING_CHANGE`, which makes
  the change a tape record applied at the same tick on replay.
- A film opens only on its fingerprint. theater.md says exact films are
  build-bound; this adds feature-bound and implementation-bound.

## Determinism

Anything on the guest thread changes the run a little even when it writes
nothing: an override costs dispatch transfers, a wrapped call leaves a
continuation token where the return address was (Code: the lockstep
crate's header, "compare runs that all have it on, or all off"), and a
Rust entry does not make the charged accesses the original made.

| Class | Meaning | Examples |
|---|---|---|
| `Host` | never on the guest thread, never writes guest memory | render scale, vsync, bindings, the overlay, view stages |
| `View` | on the guest thread or writes guest memory, presentation state only | UI screens, strings, FOV and camera modes, render tags, the tick hash |
| `Sim` | changes what the simulation computes or is given | Forge variants, game variants, the game-start path, button layouts |

```rust
pub struct Fingerprint {
    /// Every View and Sim feature and subsystem (id, version, rust or original),
    /// the hook plan, every View/Sim setting value, every enabled pack hash.
    pub exact: [u8; 32],
    /// Sim features, Sim subsystems' versions, Sim settings and packs only.
    pub sim: [u8; 32],
}
```

- `exact` equal: two runs can be compared tick by tick. Lockstep traces
  and films carry it.
- `sim` equal: two peers play the same game. The lobby carries it
  ([forge.md](forge.md) 2.4).
- A Port-grade `Sim` subsystem keeps `sim` unchanged between `rust` and
  `original` only while its differential evidence stands; its version is
  in `sim` so a peer on a different port is visible.
- Enforced where it can be: an observer gets `&dyn GuestMemory` and no
  CPU; a `Host` feature's builder has no guest-side extension points.
  The rest is the simulation-hash comparison.

Follow-up for the runtime (S): let a Rust entry or observer charge a
fixed, declared access count, so that switching an implementation does
not move slice boundaries.

## Examples

### Field of view

Today: `fov_setting.rs` (327 lines), `fov_row.rs` (370), a `Row::Fov`
arm, a block in `layout_glue::install`, a builder in `play_cmd.rs`, a
field on `boot::Host`.

```rust
pub static FOV: Def = Def {
    key: "graphics.fov",
    label: "Field of View",
    help: "First-person horizontal field of view in degrees. 70 is the game's own.",
    kind: Kind::Int { valid: 30..=150, steps: Steps::range(70, 120, 5), unit: Unit::Degrees },
    default: Value::Int(70),
    env: Some("H2_FOV"),
    flag: None,
    apply: Apply::Live,
    scope: Scope::View,
    tier: Tier::User,
    menu: Some(Menu { screen: "video", order: 10 }),
};

impl Feature for Fov {
    fn manifest(&self) -> Manifest { Manifest::new("fov", "Field of view", Sim::View)
        .legacy_off("H2_NO_FOV_ROW").doc("docs/settings-menu.md") }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let fov: Setting<u32> = b.setting(&FOV);
        b.camera().modifier(move |cam: &mut FirstPerson, _| {
            if let d @ 30..=150 = fov.get() {
                if d != 70 { cam.set_unzoomed_fov((d as f32).to_radians()) }
            }
        });
        Ok(())
    }
}
```

The row, its strings, "set by H2_FOV; row locked", the file write and
`h2 settings set graphics.fov=90` come from the `Def`.

- **Bridge backing** of `camera().modifier`: the after-hook on
  `PLAYER_FOV` that exists now. `FirstPerson::set_unzoomed_fov` is
  `fov_row::apply` (scale by `target / unzoomed`, raise the clamp), with
  `unzoomed_fov` reading through `PlayerControl`, `Objects` and `Tags`
  views instead of five private constants. Its tests carry over.
- **Native backing** (camera subsystem): the Rust first-person update
  asks its modifiers before it clamps. No clamp global to patch.

```rust
#[test]
fn the_camera_keeps_the_zoom_ratio() {
    let mut g = FakeGuest::new();
    g.tag(7, |t| t.unit_camera_fov(70f32.to_radians()));
    g.player_control(0).unit(g.object_with_tag(7));
    let t = feature::testing::build(&[&Fov]).set("graphics.fov", 90);
    let out = t.camera().first_person(&mut g, 0, 35f32.to_radians()); // zoomed 2x
    assert_close(out.fov, 45f32.to_radians());
}
```

### Forge: move, retarget or remove netgame equipment

forge.md's first slice.

```rust
impl ContentKind for MapVariant {
    const KIND: &'static str = "map-variant";
    type Pack = Variant;                      // the model in forge.md 2.1
    type Target<'a> = Scenario;

    fn validate(v: &Variant, scnr: &Scenario, m: &dyn GuestMemory)
        -> Result<Applied, Refusal>
    {
        v.base.check(scnr, m)?;               // map name and scenario hash
        let mut eq = scnr.netgame_equipment(m)?.to_owned(m)?;   // a copy
        for e in &v.edits {
            match e {
                Edit::Set { index, item, position, orientation } => {
                    let tag = Tags::get(m)?.resolve(item)?;      // path -> datum
                    eq.get_mut(*index)?.set(tag, *position, *orientation)?;
                }
                Edit::Remove { index } => eq.swap_remove(*index)?,
                Edit::Add { .. } => return Err(Refusal::NeedsGrowth),
            }
        }
        check_bounds(&eq, scnr, m)?;          // finite, inside the BSP's world bounds
        Ok(eq.diff_against(scnr, m)?)         // the exact byte edits
    }
    fn apply(a: Applied, m: &mut dyn GuestMemory) -> Result<(), MemFault> { a.write(m) }
}

impl Feature for Forge {
    fn manifest(&self) -> Manifest { Manifest::new("forge", "Map variants", Sim::Sim)
        .toggle(Toggle::Restart).doc("docs/forge.md") }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let variants = b.content_kind::<MapVariant>();
        b.loader().patch(move |map| {
            let Some(v) = variants.selected_for(map.name()) else { return Ok(()) };
            let applied = MapVariant::validate(&v, &map.scenario(), map.mem())?;
            MapVariant::apply(applied, map.mem_mut())?;
            Ok(())                            // a refusal shows a notice; the map loads retail
        });
        Ok(())
    }
}
```

No address appears in the feature. With the bridge loader the patch runs
after the cache load returns and `Edit::Add` is refused; with the native
loader it runs before publication and `NeedsGrowth` goes away. The pack
hash reaches the lobby through `Fingerprint::sim` without the feature
doing anything. The test is forge.md's acceptance: a synthetic scenario
in a `FakeGuest`, a variant, the expected block bytes; a variant for
another map's hash is refused and nothing is written.

### Render scale

```rust
pub static RENDER_SCALE: Def = Def {
    key: "graphics.render_scale",
    label: "Resolution",
    help: "Internal resolution as a multiple of 640x480.",
    kind: Kind::Int { valid: 1..=6, steps: Steps::range(1, 4, 1), unit: Unit::Times },
    default: Value::Int(1),
    env: Some("H2_RENDER_SCALE"),
    flag: None,
    apply: Apply::Restart,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu { screen: "video", order: 30 }),
};

impl Feature for RenderScale {
    fn manifest(&self) -> Manifest { Manifest::new("render-scale", "Resolution", Sim::Host) }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let scale: Setting<u32> = b.setting(&RENDER_SCALE);
        b.render().view(move |v: &mut ViewConfig| v.scale = scale.get());
        Ok(())
    }
}
```

That is the whole feature. It is `Restart` because the backend type is
chosen when D3D is installed (`ScaledBackend::new(reference, view,
scale)`); making it `Live` is a change inside `d3d` (rebuild the view's
surfaces at a present), after which the feature changes one word. Being
`Host`, it is in neither fingerprint, which is
[high-resolution.md](high-resolution.md)'s claim stated as data.

## Testing

Agents do not build or run the game; the user plays. Everything here
runs under `cargo test -p <crate>`.

| What | How |
|---|---|
| Settings | `Store` over a temp dir: precedence, bad values, locked, pending, atomic write, unknown keys kept, today's `settings.toml` loads |
| Plan | feature and subsystem sets through `Plan::build`: order, every conflict, a missing point, switches off. Golden text of the default plan |
| Sync entries and hooks | `FakeCall` + `FakeGuest`: arguments in, result and memory out |
| Async entries | poll with `FakeCallees`; assert the guest calls and their arguments |
| Port entries | differential against the lifted original (needs the owner's XBE; skipped without it; captures stay local) |
| Views | `record!` round trip: build with the generated builder, read with the accessors |
| UI | `Model` for each list; every string id resolves; row limits enforced |
| Content | validate and apply on synthetic targets; refusal writes nothing; hash stable under key reordering |
| Render | `RecordingBackend`: same device call list with and without stages; a failing stage falls back |
| Tape | record then replay through fake boundaries; a changed fingerprint refuses the film |
| Installer | a `runtime` test with a synthetic generated module (as `lockstep::synthetic`): two parties on one point run in order; an implementation replaces the original; an async entry's awaits reach the fake guest functions and resume |
| Docs | `h2 settings doc` and `h2 features plan` match the committed tables |

What tests cannot show and the user checks in the game: that a point's
signature is right, that a screen looks right, that a patched map plays.
The installer prints `<name> active` the first time each link runs, as
the hooks do now, so one session shows what was reached.

## Migration

Old `register_override` calls and the plan coexist on different points;
the installer refuses a point that already has an override it did not
place.

1. **Settings** and the `ui` model (the first slice). No hook changes.
2. **One memory trait, one fake, the points table, the registry.** Move
   the tick and activation users (live-telemetry, lockstep, the level
   boot's loaded mark) onto the `frame` and `game` bridges. Fixes the two
   collisions.
3. **Turn on the ported leaves** (datums, tags, locale) behind their
   switches, once the differential feature builds again. Strings move to
   `locale` sources; the two string hooks go.
4. **Async entries; our screens get their own list class** (UI step b).
   The Video screen stops touching the Controls class.
5. **Retail list handlers ported** (UI step c). The speaker and Video
   rows and the layouts become extension rows; `layout_glue.rs` ends as
   the `ui` subsystem plus a `layouts` feature. Do the layouts last: they
   carry the pane cloning and the crash guards of 2026-11-16
   ([controller-layouts.md](controller-layouts.md)).
6. **Game options** native: `level_boot.rs` and `community_variants.rs`
   become clients of `game`.
7. **Frame loop** native: the `frame` bridge's hooks go.
8. **Camera**, **loader**, **render** as their features need them.
9. Single-hook features (`live_notice`, `keyboard_glue`) move to the
   registry whenever convenient; they become `ui` clients after step 5.
10. **HLE addresses into `points`** when the title-update work starts.

## Phases and effort

One developer who knows the runtime. Every number is S. "Days" are
implementation and tests, not the user's play-testing, and not the
reading of the XBE that each native subsystem needs first (noted where it
is more than a few hours).

| Phase | Work | Effort | Pays off with |
|---|---|---|---|
| 0 | **First slice**: `crates/settings`; fov, fps, render scale, speaker on it; `ui` model; Video screen from the schema over the existing hooks; `h2 settings` | 3-4 days | a setting with a row is one `Def`; three files of plumbing go |
| 1 | `FakeGuest`, one memory trait, `points!`; `crates/feature` (`Feature`, `Builder`, `Plan`, chains, conflicts, switches, arena); `frame` and `game` bridges; migration step 2 | 6-8 days | collisions fixed; extension points exist; addresses named once |
| 2 | Differential harness building again and generalised; leaves on; `locale` sources; views for tags, datums, players, objects | 4-6 days | the first Rust code actually running in the game; strings without hooks |
| 3 | Async entries in the installer; `Widget`/`ListHandler` views; our own list class and loader (UI step b). Reading: the list handler's vtable | 5-7 days | new screens with no retail function patched |
| 4 | Retail Controls and Button Layout handlers ported (UI step c), with call captures | 6-9 days | `layout_glue.rs` splits; rows on retail screens are data |
| 5 | `game` native: options, players, check, queue, variant defaults and built-ins (Port); `game-variants` packs | 6-9 days | any game started from Rust; variants as data |
| 6 | `frame` native: main loop iteration, tick loop, tick, time budget (Port). Reading: the gates in `0x0012b450` | 6-10 days | events are Rust calls; Theater transport and high-fps pacing have a home |
| 7 | Host overlay in `app` over `Store` and `ui::Screen`; pad navigation; pause on open | 5-8 days | every setting reachable; non-list UI for Forge and Theater |
| 8 | Content packs: manifest, hashing, discovery, `ContentKind`; `loader` bridge; scenario views | 5-7 days | forge.md phase 1's plumbing |
| 9 | `camera` native: director dispatch, first-person and following cameras (Port), modifiers and modes. Reading: the observer and view records | 8-12 days | FOV native; Theater cameras; per-view camera for interpolation |
| 10 | Tape: boundaries, three modes, header with fingerprint; `xapi::tap` and `lockstep_glue` on it | 5-8 days | theater.md phase 1 becomes a file format and a command |
| 11 | `loader` native: Rust cache load publishing the same structures | 10-15 days | growth for Forge; custom and map-pack maps |
| 12 | `d3d::stage` (tags, observers, view stages); diagnostics moved | 6-10 days | E3 lighting and interpolation plug in on the bridge |
| 13 | `render` 7a: view loop and pass loop in Rust | 10-15 days | pass identity known; tags without observers |
| 14 | `render` 7b onward, HUD, widget system | weeks each, per its own design doc | native lighting, view-resolution HUD and menus |
| 15 | `ImageAddrs` through the plan and views | 1-2 days here | rides on [title-updates.md](title-updates.md) |

Phases 0 to 4 are about five weeks and change how every setting and
screen is added. Phases 5 and 6 are about three more and change how games
start and how per-frame work is attached. After phase 1 the others are
largely independent: 7 needs only 0; 8 and 10 need 1; 9, 11 and 13 need 3
(async entries). Do each just before the first feature that needs it.

### The first slice

**Our settings as a schema, and the `ui` subsystem's Rust model with the
Video screen on it.** Target 3 to 4 days. It is deliberately the bridge
step of candidate 1: all Rust that survives into the native steps, no new
guest behaviour, so unit tests are enough to land it.

1. `crates/settings`: `Def`, `Kind`, `Value`, `Store`, `Setting<T>`,
   `State`, `Outcome`; precedence; one atomic writer; change callbacks.
   Tests over temp dirs. (Day 1.)
2. Four `Def`s: `graphics.fov`, `graphics.fps`, `graphics.render_scale`,
   `pad_speaker.enabled`, with today's file keys, environment variables
   and row steps. A test loads a `settings.toml` written by today's code.
   `FovSetting`, `LaunchSetting` and `SpeakerSetting` shrink to handles
   (the speaker keeps its `on_change` into the audio router).
   `play_cmd.rs` builds one `Store`; `boot::Host` carries it instead of
   four fields. (Day 2.)
3. `boot::ui`: `Screen`, `Row`, `RowKind`, `Model` (count, row, texts,
   press), string enumeration from `Kind`. `video_screen`'s `Row` enum
   and `match` arms go; the Video screen is
   `ui::Screen::new("video").rows_from_settings()` and the Controls
   additions are `ui::extend(CONTROLS)`. The hooks in `layout_glue` are
   not touched: they already ask for a row count, texts and a press, and
   now ask the model. (Days 2 to 3.)
4. `h2 settings list|get|set` and `h2 play --set key=value`. (Day 3.)
5. Proof: one new Video row added as a single `Def` in the same change
   (the controller mute button, `H2_PAD_MUTE`, `Restart`: it is read once
   at launch). (Day 4, with the docs.)

### What was built

All on tests; nothing here was run in the game.

- `crates/settings`: `Def`, `Kind` (`Bool`, `Int { valid, steps, unit }`,
  `Choice`), `Value`, `Store` (`load`, `handle`, `set`, `step`, `state`,
  `force`, `on_change`), `Setting<T>`, `State`, `Outcome`, `Problem`, and
  one writer for `settings.toml`. Two fields beyond the sketch above:
  `aliases` (`H2_FOV=0` means 70) and `parse_env` (`H2_PAD_SPEAKER` may
  name a routes file). `Store::force` locks a value the run decides (the
  controller speaker with audio off).
- `boot::setting_defs`: `graphics.fov`, `graphics.fps`,
  `graphics.render_scale`, `pad_speaker.enabled` and the proof row
  `pad.mute_button`. `fov_setting.rs` and `video_setting.rs` are gone;
  `SpeakerSetting` keeps only its router and HID plumbing; `boot::Host`
  carries one `Store`.
- `boot::ui`: `Screen`, `Row`, `RowKind` (`Setting`, `Open`), `Extension`,
  `Model` (`count`, `row`, `texts`, `press`, string ids interned by text).
  `RowKind::Action`/`Custom`, left/right presses and `Host::Overlay` are not
  built: nothing uses them yet. The model lives in `boot` (it needs the
  speaker row's retail string ids), not in a crate of its own.
- `h2 settings list|get|set`, `h2 play --set key=value`. `h2 settings doc`
  is not built.
- Not done from the design's settings section: the other `H2_*` variables
  are not `Def`s yet, and `d3d` still reads `H2_RENDER_SCALE` itself for
  `h2 run`.

#### Phase 1

All on tests; nothing here was run in the game.

- `engine::points`: `Point` (name, base address, `Sig`, `Class`, `hot`,
  doc page), `Global`, and the `points!` and `globals!` macros. The table
  holds what phase 1 hooks: `GAME_TICK`, `GAME_ACTIVATE`, `GAME_END`,
  `PLAYER_KILLED`, `WEAPON_FIRE` and `DAMAGE_REPORT` (optional), `LOCALE_FIND` (hot), and the
  globals `GAME_GLOBALS` and `GAME_TIME`. The macro grammar is
  `NAME @ 0x...: [fn(arg: loc, ..) -> loc ret n] | [opaque], Hot, Optional; "docs/page.md";`
  with `stack n` as the n-th dword argument. Telemetry's layout, the level
  boot and lockstep read these instead of their own copies. A test fails
  when one address gets two names. The other points (layouts, FOV,
  speaker, level boot's option calls) move as their hooks move.
- One memory trait: `engine::GuestMemory` gains `f32`, `bytes`, `cstr`,
  `utf16`, `vec3`, `set_f32`, and is implemented for `runtime::Guest`
  (feature `runtime-glue`). `engine::testing::FakeGuest`: sparse pages,
  faults on unmapped access, never wraps. Not done: `layout_glue::Gm` is
  still its own trait and the six private fakes are not replaced (the
  `Gm` logic and its fakes move with UI steps b and c); `record!` views
  are phase 2.
- `crates/feature`: `Feature`, `Manifest`, `Registry` (`add`, `build`),
  `Builder` (`frame()`, `game()`, `raw()`, `guest_alloc`), `Plan`
  (`text`, `install`), `Conflict`, `Switches`, `Arena`, `ImageAddrs`
  (`Base`, and `Remap` for synthetic guests).
- Kinds as the table under "Hooks": `observe_before`, `before`,
  `implement`, `after`, `observe_after`. Observers get `Obs` (registers,
  the runtime for memory and clock, typed `arg_u32` from the `Sig`);
  mutating hooks get `Call` (CPU and context). Decision: observers see the
  runtime's `Guest`, not `&dyn GuestMemory`, because telemetry's readers
  take `&runtime::Guest`; they move to views in phase 2.
- Conflicts, refused at `build` and all reported: two implementations;
  two `before` or two `after` of different features in one `Order` class
  with no `after_feature` between them; an `after_feature` cycle; a
  `before` on an opaque point; one address with two point names; two
  features with one id. At `install`, an override the plan did not place
  on any of its addresses refuses the whole plan (`InstallError::Occupied`)
  before anything is mapped or registered. Decisions where the design was
  vague: an `implement` on an opaque point is allowed (the implementation
  owns the whole ABI; the 22 engine adapters are opaque points); a `before`
  that returns early skips the rest of the chain, observers after
  included; `after` hooks run inside out (the outermost last); a missing
  point drops an optional hook with a note and otherwise turns the
  feature off, at build (`ImageAddrs`) and again at install (no generated
  original).
- Chains: one override per address, named `plan:<POINT>[features]`; a
  shadow entry from the arena only when something runs after the body
  (the call-then-continue dance, written once). Each link prints
  `feature: <POINT> <kind> <feature>:<label> active` the first time it
  runs.
- Arena: `0xa0f00000..0xa0f80000`, inside the kernel's HLE reservation
  and above where Bink and DirectSound search. Shadows and `guest_alloc`
  come from it. Telemetry's `0xa0ff5000`, lockstep's `0xa0ff9000` and the
  level boot's `0xa2f40000` shadow pages are gone.
- Switches: `H2_FEATURES` (`id`, `-id`, `id=on|off`, `impl.x=on`), with
  `impl.a.b` falling back to `impl.a`. Not yet settings `Def`s, and no
  `Toggle::Live`.
- Bridges: `frame().on_tick`, `game().on_start` (activation returned AL
  non-zero), `game().on_end`. All are read-only listeners, so any number
  share a point.
- Migration step 2: Live telemetry is a feature (match start, tick, end
  on the bridges; kill and weapon fire as raw observers, the latter
  optional). Lockstep's tick hash is `lockstep::TickObserver`;
  `install_tick_observer` is now a one-feature plan. The level boot's
  loaded mark is `level_boot::LoadedMark` on `game().on_start`; its menu
  replacement stays a plain override (game options are phase 5). The
  button layouts' string hook is `ui-strings`, a `before` on
  `LOCALE_FIND`. `engine-leaves` places the 22 engine adapters as
  implementations behind `impl.engine` (off by default, as nothing
  installed them before); `engine::runtime_glue::register_all` stays,
  with no caller.
- Boot builds one `Registry`, adds the features where it used to install
  them, then builds and installs the plan once, after lockstep and before
  Bink and the trace. A refused plan stops boot with every conflict
  named. Boot logs the plan, one `feature plan:` line per feature and per
  point, then any notes.
- Not built: `h2 features plan` (the text exists and a boot test pins it),
  `Toggle::Live`, `Subsystem`/`Entry` and async entries (phase 3), the
  `.at(site)` call-site hooks, `Refuse` in `ImageAddrs`, and a measurement
  of chain overhead on the hot locale point (the closure does no
  allocation and takes no lock; the plan warns that the point is hot).

#### Phase 2

Tests only, except that the differential harness ran on the owner's XBE
(local, never committed); nothing here was run in the game.

- **The differential harness builds and runs** (`crates/engine/tests/
  differential.rs`, feature `differential`, which now includes
  `runtime-glue`). The `ir`/`lift` breakage the earlier TODO named was
  already gone.
- **Generalised to the game's path.** Every call now runs three ways from
  one state: the original (lifted, on the IR interpreter), the adapter
  directly, and `engine::runtime_glue::invoke` on a `runtime::Guest`
  holding the same 64 MB, which is the body the `engine-leaves` feature
  installs. The game path must equal the adapter exactly (outcome, every
  register, every byte, the runtime fault's address and direction). A
  `game-path` column counts its disagreements. Not covered: the plan's
  chain around the body (covered by the `feature` install tests on the
  fixture module), and async entries (phase 3).
- **Evidence** (debug build, image sha256 pinned): seeds 1, 7, 99, 2026,
  31337 at scale 1; every one of the 22 adapters exercised, zero
  disagreements against the original and zero on the game path: 7.66 M
  original instructions, 93,304 calls (for the locale leaves 4,500 finds
  and 4,502 exists, one a fault; tags 11,762; datums 72,540). The table is
  printed with `--nocapture`.
- **Leaves on, by group.** `impl.engine.<group>[.<name>]`, falling back to
  `impl.engine.<group>` and `impl.engine`. Groups: `locale` (find, exists),
  `tags` (`tag_get_checked`, `tag_group_find`), `datums` (the other 18).
  **`locale` is on by default; `tags` and `datums` stay off.** Why the
  split: the harness supports all 22 equally, but the cost is not the
  same. `LOCALE_FIND` was already overridden by default (our strings), so
  the Rust lookup adds no dispatch there and removes a hop into the
  original. The datum and tag leaves are the hottest functions in the
  game; each override takes its callers off the direct-call path, and
  every Rust access goes through the checked guest instead of the C fast
  path. That has not been measured, and the user plays. Turn them on for a
  play-test with `H2_FEATURES=impl.engine=on` and compare frame times
  (`perf` scopes) before defaulting them. A second caveat for lockstep:
  Rust accesses are not charged to the logical clock, so traces are only
  comparable within one switch set (see "Coexisting").
- **`locale` sources.** `b.locale().source(label, f)`: `f` maps a packed
  id to the guest address of its text. The Rust `locale_string_id_find`
  asks the sources first (`b.locale().serve()`), so with it on our strings
  need no hook at all. When nothing native serves them (switched off, or
  missing), `Registry::build` adds one `before` on `LOCALE_FIND` owned by
  the `locale` bridge. The decision is made after every feature declared.
  The plan text gains one `locale source <feature>:<label> (served by ...)`
  line per source. `locale_string_id_exists` does not consult sources: it
  answers as retail does, so an id of ours "does not exist" there, as
  before.
- **Boot.** `ui-strings` is a source now (one function in
  `layout_glue.rs`). There was one string hook, not two: the speaker and
  Video rows already share it.
- **Points.** `LOCALE_EXISTS` joins the table (opaque).
- **`record!` and the views** (`crates/engine/src/view.rs`). A record is a
  `Copy` handle on a guest address with typed getters and `set_` setters,
  an uppercase offset constant per field, and a builder that writes one on
  a `FakeGuest`. The grammar is `field @ 0xOFF: Type`, not the sketch's
  `Type @ OFF` (macro_rules cannot follow a type with `@`); it uses the
  `paste` crate for the names. Field types: the integers, `f32`, `Vec3`,
  `Option<DatumHandle>` (none is `0xffffffff`). Nothing is read until a
  getter is called, and nothing is copied: every offset is a named
  research point next to the doc it came from.
  - `Datums<T>`: a datum array of records, from a global (`from_global`)
    or an address. `get` is `datum_try_and_get`, `get_absolute` is
    `datum_get_absolute`, both through the ported functions, so a view
    has the game's semantics (salt, high water, the element size from
    the header), not a reader's guess. `live(limit)` lists the slots below
    the high water with a nonzero salt.
  - `Tags`: `entry(handle)`, `get(handle, group)` (which is
    `tag_get_checked`, parents included), `scenario()`.
  - `players::Player` (`PLAYERS`) with `name`, `listed`; `objects::Header`
    and `objects::Body` (`OBJECTS`), `objects::get(handle)` giving the type
    and body or none (no array, stale handle, no body).
  - `view::fake` lays out a datum array of a record on a `FakeGuest` with
    the ported allocator, so tests put records where the game would.
  - Points: `PLAYERS`, `OBJECTS`, `TAG_INDEX` and `TAG_ENTRIES` join the
    globals table; `tags.rs` takes its two from there.
- **Tests:** `crates/engine/tests/views.rs` on `FakeGuest` (round trip,
  players from the global with names and stale handles, no array, objects
  by handle, tags by handle and parent group, out of range and stale).
  **The harness checks the views** on real layouts: for every
  `datum_try_and_get`, `datum_get_absolute` and `tag_get_checked` call it
  also asks the view (`Datums::get`, `Datums::get_absolute`, `Tags::get`)
  in the original's memory, and the view must return the original's
  `eax` (none as 0) or fault with it. Columns `views` and `view-bad`.
  Evidence, with the same five seeds and pinned image as above: 9,798
  view answers (`datum_try_and_get` 2,700, `datum_get_absolute` 1,337,
  `tag_get_checked` 5,761), zero disagreements, and every other count
  unchanged (93,304 calls, 7.66 M original instructions, 817 s debug).
- **Telemetry folded** (`live-telemetry`): the roster (`players`) and
  object lookups (`object_body`) read through `Datums`, `Player` and
  `objects::get`; its layout constants for those records are aliases of
  the view constants, and its three globals come from `engine::points`.
  Two differences, both only on a corrupt array: the high-water check is
  signed as in the game (it was unsigned), and the element size comes
  from the header (it was a constant). The other telemetry readers
  (`fov_row`, community variants) are untouched, to keep boot edits
  small.
- **Not built:** `Scenario`/`Block<T>` tag-block views, the `likely`
  range-checked fields, `GameGlobals`, and the UI views (`Widget`,
  `ListHandler`, which phase 3 needs). `record!` is the base for all of
  them.

#### Phase 3

Tests only; nothing here was run in the game. The first part of it that
the game exercises is the Video screen, so it is the one to play-test.

- **Reading first** ([ui.md](ui.md) "Screen and list classes", static).
  Open question 1 is answered: row text is list vtable slot 20, reached
  through the vtable on every draw (`0x24abe0`); row chosen is not a slot
  but the function pointer in the list's press record (`+0x14`; Controls
  keeps the record at `handler + 0x388`). Both are read at each call, so a
  handler's vtable pointer and its press function can be replaced after
  the constructor ran. Two more facts a class needs: screen slot 18 builds
  the header (a text widget at `screen + 0x80`), so the title is set after
  it rather than swapped during it; and screen slot 26 returns the loader,
  which the screen manager calls (`0x2352e8`, `0x2354d0`), so a screen of
  ours must answer with our loader.
- **Async entries** (`engine::entry`, `feature::entry`). A body is an
  `async` block over a `Cx`: `cx.read`/`cx.write` take guest memory per
  closure, `cx.call(GuestCall)` awaits a guest call and gives back the
  registers. `engine::entry::testing::run` drives one against fake
  callees on a `FakeGuest` and records the calls; the installer drives it
  with `ExecCtx::call_guest`, parking the future in the continuation. The
  stack is put back after every call and the caller's EBX, ESI, EDI, EBP
  and ESP on return, so callee- and caller-cleanup need no care. Two ways
  in: `Builder::entry(label, f)` places a body at a fresh address on the
  feature's executable arena page (16 bytes apart; the plan text has one
  `entry` line each), and `raw().implement_async(point, ..)` makes one a
  chain's body. Tests: the core on `FakeGuest` (order, faults, arguments,
  a stall is an error), and on the fixture runtime an entry that awaits two
  real guest calls and returns as stdcall with the caller's registers.
- **Views** (`engine::view::ui`): `Screen`, `ListHandler`, `Item`,
  `PressRecord`, the slot numbers, and the UI calls as named points
  (`OPEN_SCREEN`, `UI_ALLOC`, `CONTROLS_SCREEN_CTOR`, `SCREEN_PUSH`,
  `SCREEN_SETUP_PANE`, `TEXT_SET`, `ITEM_TEXT`; the Controls vtables as
  globals).
- **Our own list class** (`boot::list_class`, a feature): five entries
  (loader, row text, row chosen, pane set-up, loader slot) and one page
  for its vtable copies, written from the game's own tables at the first
  open. The loader does what the Controls loader does and adopts the
  screen between construct and push: our vtables, our row chosen in the
  press record, the rows trimmed to the model's (the Controls list hook
  may have grown them), the loaded byte. A screen that is not what the
  Controls constructor builds is left as Controls and says so.
- **The Video screen is on it.** The Settings row opens it with the
  class's loader. Gone: the open-in-progress flag, the array-name test,
  the Video branches in the Controls row text and row chosen hooks, the
  header id swap, and the "h2r video settings" string. The Settings row
  itself is still a hook on the retail Settings list (step c territory).
  The Skulls screen keeps the old way; it is the next to move.
- **Limits.** Six rows (the Controls item widgets); more are cut, with a
  line. Opening a screen from one of ours is not wired (an `Open` row
  logs). Nothing has measured the entries' cost; they run only on menu
  draws and presses.

#### Phase 4 (in part)

Tests only; nothing here was run in the game.

- **Rows on retail lists are data on the plan** (`boot::retail_rows`).
  `Extension` answers a retail list's extension rows from the `ui` model
  with `before` hooks at the list's row text and row chosen (a retail row
  goes on to the original): the Settings list (the Video row) and the
  Controls list (the speaker row). The Xbox Live menu's Custom Game
  Browser row stays raw in `layout_glue` (it landed there with its own
  tag extension and item widget). The layouts' value text
  on Controls is a `before`/`after` pair inside the speaker row's hook at
  the same point. A `before` hook answers with an async body through
  `Call::answer_async`.
- **The Skulls screen** is our list class (`boot::skulls_screen`); its row
  on the difficulty list is two plan hooks. Gone from `layout_glue`: the
  Skulls open flag, its array-name test, the header hook (`0x22fda6`) and
  the text hook's title swap, the Skulls branches in the Controls hooks.
  Its rows' array is built at the screen's count: while a class loader
  runs the Controls constructor, the datum array constructor's hook
  (`0x1a480d`, count in EAX) asks `list_class::array_count`. Building a
  second array and swapping it in was the other way; the destructors
  (`0x2b7563` → `0x2376c8` → `0x1474e8` → `0x22e34b`) never free
  `list + 0x70`, so nothing would own a replaced array's lifetime. Past
  six rows the list scrolls, as the old Skulls path did.
- **Open rows** open another screen of ours: classes register their loader
  by screen id (`list_class::loader_of`), and a press on an `Open` row
  calls `0x149f1e(1 << player, loader)` for the pressing player.
- **`layout_glue.rs`** is down from 1,929 to about 1,700 lines; what is
  left is the layouts, the Live menu's rows, the shared list hooks (the datum array constructor,
  the pane, the screen tag, the text set), the smaller Controls screen's
  speaker row, the added item widgets and the camera's field of view (it
  needs EAX from entry after the original, which an `after` hook does not
  see).
- **The Controls row text is ported** (`boot::controls_text`, feature
  `controls-text`, an `implement` at `0x2c8548`). Every string id is read
  at run time from its instruction in the loaded image (the `mov ecx` /
  `mov esi` immediates, the `and`/`add` pair of the on/off rows, row 2's
  jump table), named by instruction address; nothing is copied. Our rows'
  `before` hooks and the layouts' value pair run around it as they did
  around the original. Its tests run it on a synthetic image and capture
  its guest calls (`ITEM_TEXT` twice, then `TEXT_SET` for each text that
  exists). Not yet: captures of the retail function from a play session to
  diff against, and the Button Layout list's handlers.

Why this first:

- It changes no hook and no guest write.
- It removes the most duplicated code per day (about 1,200 lines across
  the three `*_setting.rs` files and their plumbing).
- Every item on [settings-menu.md](settings-menu.md)'s list (vsync, merge
  passes, E3 lighting, mouse sensitivity) then starts as a `Def`, and
  `impl.*` switches for native subsystems have somewhere to live.
- The `ui` model is the part of the UI subsystem that does not change
  between bridge and native, so nothing in the slice is thrown away.

## Risks

- **Boundaries cut through inlined code.** A function we implement may
  have copies the compiler inlined into callers we did not rewrite; those
  keep running the retail logic. Before a subsystem goes native, list the
  readers and writers of its private state (as engine-map.md counts
  owners per global) and treat anything with outside writers as contract.
- **A Port that is almost right.** Differential tests cover the states
  they were given. Keep the per-entry switch forever, capture from real
  sessions, and treat a simulation-hash mismatch as a release blocker for
  `Sim` subsystems.
- **Logical time moves when code moves to Rust** (see "Coexisting").
  Until entries can charge a declared cost, `exact` comparisons are only
  valid within one switch set.
- **Async entries.** A future parked across a guest call holds a
  continuation slot (1,024 per thread, Code: `runtime/src/continuation.rs`)
  and blocks whole-machine forks while pending. Deeply recursive entries
  must stay sync or stay recompiled.
- **Research is the real cost.** Every native subsystem starts with
  reading retail code. The estimates above assume the reading goes as it
  did for the Controls screen; the camera and the pass loop are the least
  read.
- **Abstraction ahead of knowledge.** The `ui` model describes list
  screens because that is what has been read. Keep `RowKind::Custom` and
  `b.raw()` first-class, and do not add a model field without a second
  user.
- **Chain overhead on hot points.** Text set and the string lookups run
  many times a frame. A chain is built as one closure at plan time: no
  per-call allocation, no lock. Measure with the `perf` scopes before and
  after phase 1.
- **More overrides, fewer direct calls.** Every native entry and every
  hooked point leaves the direct-call fast path. Native subsystems reduce
  this over time (Rust calling Rust); bridges on hot functions do not.
- **Wrong signatures.** A typed argument is only as good as the `Sig`.
  One table is easier to review than nineteen closures, but "opaque until
  read" stays the default.
- **Lock order.** Settings callbacks into audio or the window: handles
  are atomics; callbacks run with guest memory dropped. The subsystem
  owns that rule, not the feature.
- **Two UIs for a while.** Guest list screens and the host overlay will
  both exist. The shared `ui::Screen` declaration keeps them from
  diverging in content, not in look.
- **Public-repo rule.** The points table and the views concentrate
  addresses and offsets. They remain named research points with a doc
  reference. No retail table, string or tag byte goes in a `Def`, a pack
  in the repo, a test fixture or a call capture that is committed.

## Open questions

1. ~~Which list-handler vtable slots carry row text and row chosen, and
   can a handler's vtable pointer be swapped after its constructor ran?~~
   Answered (phase 3, [ui.md](ui.md) "Screen and list classes"): slot 20,
   and the press record's function; yes, both are read at every call.
2. Does anything in `0x0018eaa0` read the scenario blocks before it
   returns ([forge.md](forge.md) 2.2)? It decides where the bridge
   loader's patch runs.
3. Where is map unload? Per-map `guest_alloc` is released at the next
   load until there is a point for it.
4. Does any camera path write memory the simulation reads?
5. Where is the HUD's draw entry, and how large is it?
6. How are a Rust entry's memory accesses charged today, and can an entry
   declare a fixed charge?
7. Can the view be rebuilt at a present, so render scale becomes `Live`?
8. Is the tag region inside any checkpoint snapshot
   ([settings-menu.md](settings-menu.md) section A)? It matters for
   campaign content packs.
