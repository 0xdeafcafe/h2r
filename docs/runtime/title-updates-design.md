# Title updates: the design

Status: Design. Version detection, the per-version address map, hook translation, anchors, data, CLI and testing. Part of [Title updates](title-updates.md).

## 3. Design

### 3.1 Version detection

Add an `ImageId` in `crates/formats/xbe`, computed once per image:

- the SHA-256 of the whole file;
- certificate title ID, version (+0xAC, already parsed), regions;
- entry point, header timestamp;
- the library table: the build and QFE of `D3D8LTCG`, `DSOUND`, `XAPILIB` and the rest.

Classify the image:

1. **Base**: the SHA-256 equals the 1.0 identifier, which is already in the
   repo as `wma::RETAIL_SHA256`. Move it into `xbe` (or a new small crate)
   as the single source of truth. Replace the three title-ID + entry gates and
   the test's copy with `ImageId::is_base()`.
2. **Update candidate**: same title ID, not the base hash. This alone does
   not prove an official TU: another localized retail image or a modified or
   spoofed image can share that ID. Establish local provenance and disc/language
   compatibility before assigning a version label. The candidate needs an
   address map (3.2) built on this machine from the user's base and target
   images. Without one, `h2 play` refuses the image and says how to build the map.
3. **Anything else**: refused, as today.

Also require that every XDK library build in the update equals the base's
(5849). `CDevice` offsets, render-state slot numbering, DirectSound object
layouts and the LTCG conventions depend on that build. If an update relinked
a newer XDK or QFE, refuse it with a clear message until someone audits those
tables. `gfx::RenderStateTable::for_build` is the model for making them
per-build.

A map file records both `ImageId`s, so it can't be applied to a different pair.
Keep newly measured copyrighted-file hashes in the user's private local
manifest and map cache. Public documentation may record version labels,
release dates and metadata identifiers, but must not publish those hashes
or copied image data as distribution pointers. The existing `RETAIL_SHA256`
is not a precedent for adding update fingerprints to the repository.

### 3.2 Per-version address map, built at analysis time

The design adds a crate, `imagemap`. It depends on `xbe`, `analyze` and,
optionally, `symbols`. Its input is the base XBE and the target XBE (both
local), and it produces:

```rust
pub struct AddrMap {
    pub base: ImageId,
    pub target: ImageId,
    pub functions: BTreeMap<u32, FnMatch>,   // base entry -> target entry
    pub data: BTreeMap<u32, DataMatch>,      // base global/table -> target
    pub ambiguous: BTreeMap<u32, Vec<u32>>,  // never resolved automatically
}
pub struct FnMatch { pub target: u32, pub how: Evidence, pub body_hash: u64 }
pub enum Evidence { Exact, XdkName, StringRef, Structural, Propagated }
```

The map is written to `<data or cache root>/imagemap/<target-sha>.toml` and
never committed. It holds only addresses, scores and hashes of normalised
instruction streams.

Matching runs in passes, roughly the way BinDiff- and Diaphora-style differs
work. Those tools are cited as prior art only; we don't use their code.
Every pass enforces one-to-one: a target address matched twice is ambiguous
on both sides.

1. **Fixed anchors.**
   - The entry point.
   - Kernel thunk slots, by ordinal, which gives exact data matches for the
     import table.
   - XDK functions that `crates/recompiler/symbols` finds in both images under the same
     name, with no `ambiguous` or `shared` flag.
   - The data symbols that symbols votes for (the D3D device pointer, state
     offsets).
2. **Exact normalised bodies.** Decode each discovered function with iced-x86.
   Replace every operand that points into the image (rel32 targets,
   absolute data or code addresses, as the analyzer classifies them) with a
   placeholder. Then hash the stream of mnemonics, operand kinds and
   non-address immediates. A hash that is unique on both sides is a match.
   Unchanged engine code, the CRT, Bink, WMA and most XDK code should match
   here, just relocated.
3. **String references.** Functions that reference a string literal with the
   same content, unique on both sides.
4. **Call-graph propagation.** For each matched pair, line up the
   address-ordered list of direct callees. Where the lists are the same
   length and each pair has a compatible shape, propose callee matches. Do
   the same for callers. Repeat until nothing changes.
5. **Structural features**, only for what's still unmatched: a vector of
   block count, edge count, instruction count, call count, loop count,
   imports, string refs, and non-address constants, plus a small-prime
   product over block degrees. Accept a unique feature match only when its
   call-graph neighbourhood agrees.
6. **Data by operand correspondence.** For two functions whose normalised
   bodies are equal, the k-th placeholder in one aligns with the k-th in the
   other. Each pair of aligned data addresses is a vote. Accept a data match
   only if every vote agrees and there are at least two (or one from an
   `Exact` function with an `XdkName` behind it). Jump tables, vtables and
   the `dispatch.rs` spans map the same way, and their slots must map
   through `functions`.

### 3.3 Translating the existing hooks

Refactor so that no crate passes a literal address to `register_override`.

- Each table entry keeps its 1.0 address as a key, together with a class:

  ```rust
  pub struct Anchor {
      pub base: u32,
      pub name: &'static str,
      pub xdk: Option<&'static str>, // XDK name when a signature backs it
      pub class: HookClass,
  }
  pub enum HookClass { Required, Optional }
  ```

- Registration takes `&dyn ImageAddrs`:
  - `fn function(&self, a: &Anchor) -> Placement`
  - `fn data(&self, base: u32) -> Option<u32>`

  `d3d::register`, `dsound::register`, `xwma::register`, `xapi`, `bink-hle`,
  the boot glue, `FrameWait`, `GuestLayout`, `dsound::Layout`,
  `xapi::CacheGlobals`, the loader arena and `engine` all take it.
- For the base image this is the identity map, so 1.0 behaviour and every
  existing test stay byte-for-byte the same.
- `Placement` comes in three kinds:
  - `At(addr)`: install the hook there.
  - `Skip`: the matcher found nothing, or something ambiguous, and the hook
    is `Optional`. The original code runs as generated C (the slow path).
  - `Refuse`: the matcher found nothing, or something ambiguous, and the
    hook is `Required`. Collect every refusal, then fail the boot with a
    report.
- **Unmatched data globals always refuse.** Data has no slow path.
- **Check the bytes at registration.** The map stores each match's
  `body_hash`. When a hook is installed, normalise the target function again
  and compare. This stops a stale or hand-edited map file from placing a
  hook.
- **Conventions.** An `Exact` match proves the register convention and the
  `ret N` are unchanged. Any weaker evidence on an LTCG D3D or engine entry
  can't prove that, so those placements count as `Refuse` unless that
  entry's adapter has an explicit per-version sign-off. Stdcall DSound and
  XAPI entries can accept `XdkName` or `Structural` evidence if the `ret N`
  and argc still match, as checked by the logic now in the `xbe_pins` tests.
- **Picking `Optional`** needs an audit per entry. Candidates are hooks whose
  generated original stays correct against the HLE device:
  - the native-policy XAPI hooks such as power-down;
  - `VOICE_INIT`;
  - `MOVIE_START` skip mode;
  - `FrameWait`, which only paces frames.

  D3D device entry points, port I/O, DSound creation and buffers, the XAPI
  device stack and Bink playback are `Required`.

### 3.4 Library anchors first

Some of this pays for itself even without update files:

- Fill in `xdk` on every hook with a signature behind it: about 130 entries
  (`docs/graphics/d3d.md` "matched" column, `docs/audio/dsound.md`, `docs/runtime/xapi.md` "43
  signature matches"). An `#[ignore]` test with `$H2_XBE` and `H2_XBSYMDB`
  then checks that symbols resolves each name to the table's address. That
  check is a cross-check on the hook tables in its own right.
- Derive the `GuestLayout` globals (`device`, `p_device`, `render_state`,
  `texture_state`) from the data votes symbols already casts, instead of
  writing them in. Keep the literals as the base map's expected values, and
  make a test assert that the two agree.
- Remove the duplicates: `boot::CREATE_DEVICE` and `d3d::DESTROY_RESOURCE`
  should name the `abi.rs` anchors.
- Replace the hard gates that use title ID + entry with `ImageId`, and turn
  `hs` and `dispatch.rs` into "located structurally". Their validators
  (builtin count, descriptor shape, slot targets in code sections) are
  already strong enough to scan the target's `.rdata` for the one candidate
  table that passes. Scanning beats looking the table up through the data
  map.
- **WMA**: allow `VerifiedImage` either for the base hash, or for a target
  image plus a map where every table in `tables.rs` maps to a range whose
  bytes equal the base's range, compared at run time from the two local
  files. The WMA decoder is third-party and probably unchanged.
- **Physical arena**: read the cursor seeds from the startup code through the
  data map, and refuse if they differ from the base. Don't apply the 1.0
  range blindly.

### 3.5 Data: maps, saves, tags

- **Disc maps** don't change with a title update: the update XBE reads them
  from `D:`. `crates/formats/map`'s version 8 / `02.09.27.09809` gate keeps working
  for them.
- **To check once files are available:**
  - Does the update directory carry any `.map` files or replacement shared
    maps? If so, check their header version, build string and tag index
    layout. Run `crates/formats/tags` decoders over every tag, and diff the field
    sets against a disc map of the same name.
  - Downloadable map packs sit beside the update in `TDATA`, under `$c`.
    Answered for the Map Pack disc ([map-pack.md](../game/map-pack.md)): its nine
    maps have the same version 8 header and build string as the disc's, and
    `crates/formats/map` opens them. The update, unlike 1.0, lists them: 1.0 reads
    each package's `patch.lvl`, which only names Lockout, and the update
    reads `patch_v3.lvl`, which names the package's own maps. Still to do:
    run the tag decoders over every tag and diff against a disc map.
  - Does the update XBE change the per-cache-type size limits that
    `map/src/writer.rs:137-142` copied from the 1.0 validator?
  - Saves. Checkpoints are raw snapshots of game-state memory
    (`docs/runtime/checkpoints.md`: the region, a fixed reservation, restore through
    the header check). Any change to engine structures makes a 1.0
    checkpoint meaningless to 1.1, so check the save header for a version
    field and whether the game refuses mismatched saves. Profile and
    settings saves are more likely to carry over. `saves.rs`'s search
    object and name hash are XAPI-level and the title ID is unchanged, so
    the directory layout stays the same.
  - The cache-partition database row is keyed by title ID. Check whether the
    update invalidates the cache partition contents (`xapi/src/cache.rs`).
  - The HaloScript builtin count and table layout (`hs` will tell us
    straight away).
  - Engine struct offsets used by `engine`, `d3d::FrameWait` and
    `bink-hle::WRAPPER_STATE`. Data-map voting finds globals but not field
    offsets. The `Exact` body match on the functions that use them is the
    evidence that the offsets didn't change.
- **Default to one data root per image** (`h2boot-data/<image label>`), so a
  1.0 run and an update run don't share checkpoints. Allow opting in to a
  shared root (`--data-root`) once the save formats are known to agree.

### 3.6 CLI and UX

The proposed CLI uses `h2 image` to list and map images and
`h2 play --image` to select one. The acceptance tests in section 5 take
a local base and update XBE.

- **Where files live**: `<data>/updates/4D530064/<certificate version>/default.xbe`,
  which is where `h2 import-mappack` puts the Map Pack disc's update. Also
  accept `disc/updates/` (already ignored through `/disc/`), where the user
  can copy an HDD's `TDATA/4D530064/$u/` as is, for example
  `disc/updates/<anything>/default.xbe`, and an emulated data root laid out
  like the HDD (`<data>/disk/Partition1/TDATA/4D530064/$u/...`), since `vfs`
  mirrors that layout.
- **`h2 image list`** shows every image in `disc/` and `disc/updates/`: its
  label, certificate version, short hash, XDK builds and status (`base`,
  `mapped`, `needs map`, `refused: <reason>`).
- **`h2 image map [<update.xbe>]`** runs the matcher against the base. The
  base is pulled from the ISO as now and checked against the 1.0 hash. The
  command writes the map and prints a report: per crate, how many hooks are
  placed, skipped or refused, plus the ambiguous and unmatched lists by name.
  It exits non-zero if any `Required` hook is refused. `--explain <name>`
  shows the evidence for one anchor.
- **`h2 play`** gets `--image <base|latest|path|label>` (also `H2_IMAGE`).
  The default stays `base` until an update has been accepted (section 5).
  After that the default can be `latest`: the newest update whose map has
  no refusals, otherwise base, with a one-line note.
  - The XBE and the disc are already separate (`boot::run(xbe_bytes, dylib,
    &RunOpts { disc, .. })`), so the update's bytes feed the build and run
    while `D:` stays the ISO.
  - Out dirs become `h2boot/<image label>` and `h2gen/<image label>`, so
    images don't thrash one dylib.
- **Guest relaunch.** 1.0's own startup code has a title-update path
  (`ApplyTitleUpdate`, `LaunchImageInternal` and the firmware reboot handoff,
  `docs/runtime/xapi.md`). If an update XBE exists in the emulated `TDATA`, the base
  may try to launch it itself. The runner then has to map the launch path to
  that image's prebuilt dylib and map, which means one stop and reload
  through `Outcome::Launch` and `HalReturnToFirmware`. If none is built, it
  stops with a clear error. h2 shouldn't place update files in the emulated
  `TDATA` unless the user selected that image, so `--image base` stays base.
  This needs checking against real files: whether 1.0 looks for an update at
  all, and where.

### 3.7 Testing

**Synthetic matcher tests, with no retail data.** Build pairs of images from
iced-x86-assembled code, with the synthetic XBE builders already used in the
`hs` and `analyze` tests. Cases:

- the same functions moved by an offset, or reordered;
- one function inserted or removed;
- one function body changed, which must not come out `Exact`;
- two identical functions (`ambiguous`, never a guess);
- a callee inlined into its caller, so the callee goes unmatched and the
  caller loses `Exact`;
- callers that swap their call order;
- a global moved, with and without two agreeing votes;
- a conflicting vote, which must be rejected;
- a jump table and a vtable moved;
- a string-ref anchor;
- a name anchor faked through a stub `symbols::Report`;
- one-to-one conflicts.

Add a property test: generate a random synthetic program, relocate its
sections at random, and require the map to equal the shift exactly. Then
apply random body mutations: precision must stay at 100% (nothing mapped
wrongly), and recall gets recorded.

**Placement tests.**

- Under the identity map, every table yields `At(base)`, so the existing
  synthetic tests keep running unchanged.
- A synthetic map with holes yields the right `Skip` and `Refuse` set by
  class.
- A stale `body_hash` refuses.
- An `Exact`-only rule on LTCG entries.

**Base self-map** (`#[ignore]`, `$H2_XBE`): mapping 1.0 against itself must
give the identity for every anchor, with `Exact` evidence.

**Per-version acceptance** (planned disc tests, with paths to both XBEs):

1. `h2 image map` refuses nothing.
2. Run the image-backed pins (dsound and xapi ret sizes, Bink prologues, the
   d3d `round6`/`round7` convention decoders, the `SimpleEncode` table, the
   `hs` catalog), parameterized over image + map instead of 1.0 literals.
3. Boot to the main menu under the game gate with
   `H2_BOOT_STOP_AT_PRESENT` or `menu_cpu`.
4. Enter a level (`00a_introduction`) with a scripted input timeline retuned
   for that image.
5. Save and restore a checkpoint.
6. A menu idle bench, to compare frame cost against 1.0.
