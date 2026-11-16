# Title updates

Status: design only. Nothing here is implemented. We now have an update to
test with: the Multiplayer Map Pack disc carries one, certificate version
`0x303` for our 1.0 disc (section 6, and [map-pack.md](../game/map-pack.md)).
`h2 import-mappack` copies it to `<data>/updates/4D530064/00000303/default.xbe`.

Halo 2 shipped on disc as 1.0. Xbox Live title updates (1.1 and later) don't
patch the disc. They put a new `default.xbe` on the console's hard disk under
`TDATA\4D530064\$u\` (1.0 has that path built in), and that image runs in
place of the disc's. The maps and other data still come from the disc (`D:`),
and downloaded maps from `T:\$c\`. See Sources and formats below for
evidence and the remaining launch-path checks. This page covers:

- what in the repo assumes the 1.0 image;
- what already copes with a different image;
- how to support an update without committing anything taken from it.

The rules don't change. Nothing read from either XBE goes into the repo:
no bytes, tables or instruction sequences. Writing down addresses and names is
fine, as it is everywhere else in `docs/`. Every per-version address map is
worked out on the user's machine from the two XBEs they own.

## Sources and formats

Research checked 2026-11-15. This concerns the original Xbox game. No XBE,
patch, update package or game data was downloaded or examined for this
survey; file-level conclusions still need the user's own inputs.

### Versions and release history

Preservation TU numbers, public 1.x names and executable revision fields
use different numbering. Bungie explicitly called the April 2005 game
**1.1**; Insignia identifies the final TU5 as **1.5**. Research found informal
use of **1.2**, but no authoritative mapping of separate 1.2, 1.3 or 1.4
releases to unique Xbox executables. Identify the actual image rather than
assigning those names to successive dates.
[Bungie's April preview](https://www.halo.bungie.org/bwu/index.html?item=21),
[Insignia's Halo 2 FAQ](https://insignia.live/halo2/).

This chronology combines release reporting with the five generations in
the [preservation catalog](https://consolemods.org/wiki/Xbox:Title_Updates).
It does not imply that every generation had a distinct displayed version.

| Image / preservation generation | Public release | Known changes and naming |
|---|---|---|
| Retail **1.0** | 2004-11-09 North America; 2004-11-11 Europe/Japan | Disc baseline. [Release history](https://en.wikipedia.org/wiki/Halo_2). |
| TU1 | Unknown | Harcroft reportedly recovered an XBE dated **2004-10-14**. This is a reported authoring timestamp, not a verified delivery date; changes unknown. Only the secondary transcription was accessible during research. [Discovery account and original-post reference](https://www.halopedia.org/Halo_2_Auto-Updates). |
| TU2 | **2004-11-17** | Contemporary reporting calls this the first auto-update: shorter matchmaking waits, 480p/HUD framing fixes and Foundation unlocked for everyone. [Contemporary Bungie announcement repost](https://www.neowin.net/news/halo-2-xbox-live-autoupdate/). |
| TU3 / **1.1** | **2005-04-18** | Stronger melee and grenades, shorter grenade fuse, weaker Magnum and some dual-wield combinations; these balance changes concern multiplayer. [Microsoft release notice](https://news.xbox.com/en-us/2005/04/18/halo-2-autoupdate/), [Bungie balance preview](https://www.halo.bungie.org/bwu/index.html?item=21). |
| TU4 / sometimes **1.1b** | **2005-07-20** | Anti-cheating measures targeting modified DLC and a Relic teleporter fix. The 1.1a/1.1b distinction is a secondary history's label, not a verified menu-string progression. [Update history](https://www.halopedia.org/Halo_2_Auto-Updates), [contemporary July coverage](https://halo.bungie.org/newsarchive.html?month=July&year=2005). |
| TU5 / **1.5** | **2007-04-12** | Final Xbox TU. Supports the forthcoming maps, strengthens Banhammer functions and clears learned hosting history and map cache. [Bungie's April 12 announcement, preserved mirror](https://bnetarchive.haloman30.com/news/Blog7d2b-2.html?dfilter=&mode=tags&page=1&tag=22), [displayed label](https://insignia.live/halo2/). |

April 2005 also addressed flag bouncing/through-wall flag grabs, Elite
ghosting, multiplayer vehicle sword flying, PAL/NTSC desynchronization and
standby abuse. Campaign sword flying remained. **Flag bouncing is distinct
from player superbouncing.** Bungie's 2010 farewell still referred to BXR;
there is no established original-Xbox TU removal of BXR or general player
superbounces. Reports of superbounces working in 1.5 are community evidence,
not a changelog or a test here. Changes in Vista or MCC do not establish an
Xbox TU fix.
[Bungie's exploit-fix preview](https://halo.bungie.org/bwu/index.html?item=17),
[Bungie's farewell](https://halo.bungie.org/bwu/index.html?item=272),
[community 1.5 report](https://www.reddit.com/r/halo/comments/1m7onbe/).

Map packs are separate releases: Bonus and Killtacular on **2005-04-25**,
Maptacular on **2005-07-05**, and Blastacular on **2007-04-17**. The retail
Multiplayer Map Pack DVD, released **2005-07-05**, carries the first nine
DLC maps and the then-current cumulative update through TU3. It predates
TU4 and does not supply final 1.5 or Blastacular. Its root `default.xbe`
is an installer menu, not the updated gameplay image; later community
repacked discs have different contents.
[Early DLC chronology](https://www.mobygames.com/game/15499/halo-2/patches/),
[Microsoft's Maptacular announcement](https://news.xbox.com/en-us/2005/07/05/halo-2-maptacular-pack-now-available/),
[retail DVD history](https://www.halopedia.org/Halo_2_Multiplayer_Map_Pack),
[Bungie's Blastacular announcement](https://bnetarchive.haloman30.com/news/Blog7d2b-2.html?dfilter=&mode=tags&page=1&tag=22),
[repack author's explanation](https://digiex.net/threads/halo-2-xbox-360-multiplayer-map-pack-download-all-4-map-packs-with-1-5-update.13961/).

Retail title ID is `4D530064`. The catalog distinguishes English, French,
German, Italian, Spanish, Korean, Traditional Chinese and Japanese disc
versions (`03` through `0a`, respectively), each with five TU generations.
English XBE revisions are `01/02/03/05/08`; other languages use `01`–`05`.
These are catalog identifiers, not menu decimals. Region-free flags do not
establish language/disc interchangeability. The separate Halo 2 Beta uses
`4D530082`, disc version `01`, with four catalogued TU revisions (`01`–`04`),
marked unverified and without dates or changelogs. No standalone demo TU
lineage was established; E3 demos, beta software and later ports are outside
the retail sequence.
[Variant inventory](https://consolemods.org/wiki/Xbox:Title_Updates).

### Delivery and loading

The installed update includes a complete gameplay executable at
`E:\TDATA\4d530064\$u\default.xbe`. Pinecone's own Halo 2 example and scanner
confirm that layout. DLC maps live separately under sibling `$c`; UDATA is
save/profile storage, not the installed TU location. Preserve the whole
`$u` tree: this research did not establish every companion file for every
release or language.
[Pinecone README](https://github.com/MrMilenko/Pinecone),
[Pinecone scanner](https://github.com/MrMilenko/Pinecone/blob/main/fileparse.go),
[installer maintainer's directory description](https://www.xbox-scene.info/forums/topic/213-dlc-halo-2/).

That installed XBE does not establish the network payload format. Insignia
says Xbox updates are binary patches requiring clean source game files;
its Halo 2 FAQ likewise recommends an intact disc or unpatched rip if
updating fails. The public sources do not identify Halo 2's transport codec
as xdelta, IPS or BPS, nor establish the payload format of every historical
TU.
[Insignia's patch-delivery announcement](https://bsky.app/profile/insignia.live/post/3kjbyf7rhov2n),
[Halo 2 troubleshooting](https://insignia.live/halo2/).

The Xbox update flow installs to HDD and relaunches an executable; it is
not a disc rewrite. XboxDev describes `XOnlineTitleUpdate` booting an updater.
The clean nxdk launch implementation records a launch path/data page and
uses the firmware quick-reboot handoff. For h2recomp, the intended input is
the installed update XBE plus the user's disc assets. The exact Halo 2
startup checks and drive mappings still need confirmation with owned files
(section 3.6); these sources do not prove a universal kernel overlay that
silently substitutes every disc XBE.
[Xbox Live API notes](https://xboxdevwiki.net/Xbox_Live),
[nxdk launch implementation](https://github.com/XboxDev/nxdk/blob/master/lib/hal/xbox.c).

### Community patches and preservation

**No verified retail-1.0-to-official-1.1/1.2/1.5 xdelta, IPS or BPS
converter was found.** This is a search result, not proof of absence.
Grimdoomer's Halo 2 HD release v1.1 has separate xdelta inputs for an
already-owned 1.0 XBE and an already-owned 1.5 XBE; those patches are not
interchangeable and do not manufacture official 1.5 from retail. The mod's
v1.1 is its own release number. Farklem's Halo 2 Launcher requires an
existing 1.1 update, while XBX Content Tool signs existing ContentMeta
metadata for a console; neither is an official-version converter.
[HD mod author's release notes](https://github.com/grimdoomer/Halo-2-HD/releases),
[launcher author's release NFO](https://www.xbins.org/nfo.php?file=xboxnfo1248.nfo),
[Content Tool author's README](https://github.com/feudalnate/XBX-Content-Tool).

Preservation maintainers catalog recovered releases and bundle updates
with offline DLC installers. Installer provenance matters: a 2023 release
explicitly included spoofed regional substitutes for then-missing original
TU5 files. A matching title ID or displayed 1.5 is therefore insufficient
proof of an authentic localized release. Archive completeness changes;
consult the current missing-content inventory. Pinecone recognizes local
dumps; XCAT scans and uploads preservation contributions and console
identifiers, so it is a separate opt-in activity from private extraction.
[Maintainer's installer thread](https://digiex.net/threads/xbox-offline-xbox-live-downloadable-content-dlc-installers.675/),
[regional-substitute disclosure](https://www.xbox-scene.info/forums/topic/213-dlc-halo-2/),
[current missing inventory](https://consolemods.org/wiki/Xbox:Unarchived_DLC),
[Pinecone](https://github.com/MrMilenko/Pinecone),
[XCAT documentation](https://consolemods.org/wiki/Xbox:XCAT).

### Getting and checking the user's own files

Back up an existing `$u` directory before installing a newer TU, especially
if preserving an older generation. For current 1.5, the practical route is
the user's own clean game disc and configured Insignia console: Halo 2
receives TU5 automatically. Insignia does not promise selection of older
TUs. Its matchmaking also requires all four map packs, which are separate
from obtaining the executable.
[Insignia's update and DLC requirements](https://insignia.live/halo2/).

Copy the installed `$u` tree privately through an existing dashboard's
binary-mode FTP facility, or read the user's own unlocked HDD/image with a
FATX-aware tool and the owner's required keys. Keep companions and provenance
with `default.xbe`; leave the disc dump unchanged. The user's original map
DVD offers the older offline update described above. Do not substitute its
installer-menu XBE for the gameplay image.
[Transfer guidance from the installer maintainer](https://www.xbox-scene.info/forums/topic/213-dlc-halo-2/),
[xemu's own-HDD imaging guidance](https://xemu.app/docs/required-files/),
[FATXplorer author's HDD-unlocking documentation](https://fatxplorer.eaton-works.com/2024/05/31/fatxplorer-3-0-beta-31-password-recovery-for-og-xbox-western-digital-drives/).

Future h2 tooling could import that tree into ignored `disc/updates/`,
validate XBE bounds/sections, and report title ID, raw certificate version,
region, library builds and timestamps. Header creation, PE and certificate
timestamps are distinct metadata, not public release dates. Preserve disc
and language identity alongside them; metadata alone does not authenticate
an executable. `crates/xbe` currently parses certificate title/version/region;
the expanded importer and timestamp reporting remain design work.
[XBE format reference](https://xboxdevwiki.net/Xbe).

Keep provenance and integrity fingerprints in a private manifest, pairing
the exact local base and update for the address-map design below. If future
tooling accepts a user-supplied delta, it must first recognize its actual
format and validate the source, reconstruct into a new local output, then
check output structure, metadata and independently trusted local integrity
evidence. The complete resulting XBE is the input to image matching;
unknown or incompatible images must not acquire official labels solely
from title ID or filename. Signature authentication must not be claimed
unless implemented. No copyrighted bytes, copied tables, generated matches
or newly measured copyrighted-file hashes belong in this public documentation
or commit.

## 1. Census: what depends on the 1.0 image

The counts below are distinct guest addresses or facts, taken from reading
the code and from searching for hex literals in the image range
`0x11000..0x600000`. Counts marked ~ are approximate: some literals are
comments, and some tables duplicate others. "Runtime" means a wrong value
changes what the game does. "Analysis" means it changes what is discovered
or generated. "Tooling" means CLI inspection only.

### 1.1 Runtime-critical, hard-coded

The HLE hooks are almost all literal addresses, and none of them is checked
against the image before it's installed. `runtime::Dispatch::register_override`
(`crates/runtime/src/dispatch.rs:102-136`) is a plain map insert. If a 1.0
address points somewhere else in an update, one of two things happens without
any error: the hook never fires, and the original code runs as generated C
behind an HLE device that never saw the call, or the hook fires in the middle
of an unrelated function.

| crate | what | distinct | where | how it's matched today |
|---|---|---:|---|---|
| d3d | function overrides: `Entry` literals | 56 | `crates/d3d/src/abi.rs:338-860` (`entries()`) | 63 of all d3d entries named by XDK signature in `docs/graphics/d3d.md`, 28 not |
| d3d | `SetRenderState_*` setters, each with its guest slot | 30 | `abi.rs:117-147` (`render_state_setters!`, `setter_adapters!` at `:288`) | mostly signature-matched; the slot comes from the body's store |
| d3d | `UNIMPLEMENTED` + `PORT_IO` stops | 4 | `abi.rs:152`, `:176`, `stops()` at `:197` | partly |
| d3d | `NATIVE` (report only, never registered) | ~7 | `abi.rs:220` | |
| d3d | LTCG register convention per entry | 91 | the `convention` strings and adapter bodies in `abi.rs` | not checked at run time (`abi.rs:11`) |
| d3d | `GuestLayout::HALO2`: D3D globals, plus one game global (`game_device_pointer`) | 16 | `crates/d3d/src/layout.rs:194-215` | none. Only `simple_encode` is checked against the bytes it points at (`pushbuf.rs:482-510`) |
| d3d | `CDevice` field offsets (`device_field`), `DEVICE_DWORDS`, `RENDER_STATE_SLOTS`, and two stray offsets (`fragments.rs:536`, `display.rs:124`) | ~48 | `layout.rs:69-190` | they depend on the XDK build, not on Halo |
| d3d | render-state slot numbers (5849 numbering) | ~20 | `crates/d3d/src/state.rs:62` | XDK build. `gfx::RenderStateTable::for_build` already handles other builds |
| d3d | `DESTROY_RESOURCE` | 1 | `crates/d3d/src/runtime_glue.rs:328` | duplicates an `abi.rs` entry |
| d3d (game code) | `FrameWait::HALO2`: 1 hook + 6 game globals | 7 | `crates/d3d/src/vblank.rs:235-245` | none |
| dsound | `ENTRIES` stdcall overrides, each with its `argc` | 66 | `crates/dsound/src/overrides.rs:46-114`, registered in `register.rs:581` | 59 named by XDK signature (`docs/audio/dsound.md`), the rest by call-site survey |
| dsound | xWMA codec entries | 6 | `crates/dsound/src/xwma.rs:26-46` | none |
| dsound | `Layout::default`: stream vtable global, effect count, XDK-internal object offsets | ~1 + ~8 | `crates/dsound/src/state.rs:108-168` | none |
| wma | WMA decoder tables and constants read from the image | ~54 | `crates/wma/src/tables.rs:116-217` | gated by the exact SHA-256 of the 1.0 image (`wma/src/lib.rs:20`, `VerifiedImage::load` at `:47`) |
| xapi | overrides (16) and opt-in save hooks (4) | 20 | `crates/xapi/src/register.rs:29-82`, `OVERRIDES` at `:105`, `SAVE_OVERRIDES` at `:157` | 12 of the 16 overrides are XAPI signature matches (`docs/runtime/xapi.md`), the rest inferred |
| xapi | globals and import slot: TLS index, cache rows, gamepad type, the HAL partition-count import slot | 5 | `register.rs` `addr` module, `config.rs:25-46` (`CacheGlobals`, already configurable) | none |
| xapi | game allocator called from Rust (`GAME_ALLOCATE`) | 1 | `register.rs:445` | none |
| bink-hle | Bink API hooks | 11 | `crates/bink-hle/src/addr.rs` | none (third-party, no XDK database) |
| bink-hle | game movie-wrapper functions, plus the `WRAPPER_STATE` global and its offsets | 15 + 1 | `addr.rs`, installed by `lib.rs:65-118` | none |
| boot | `VOICE_INIT` stopgap, `X_MOUNT_UTILITY_DRIVE` (`--no-xapi`), `CREATE_DEVICE` (duplicates d3d) | 3 | `crates/boot/src/game_glue.rs:28`, `xapi_glue.rs:248`, `d3d_glue.rs:311` | none |
| runtime | `HALO2_PHYSICAL_ARENA`, applied to every image | 1 range | `crates/runtime/src/loader.rs:24`, used by `boot/src/lib.rs:353` | none. It was read off the 1.0 startup code |

Runtime total: about 250 hook addresses, about 30 data globals, and about 80
struct offsets or slot numbers. Of the hooks, about 130 have an XDK signature
behind their name (D3D 63, DSound 59, XAPI 12). The rest are Halo game code,
Bink, unnamed D3D or DSound internals, or XAPI helpers named by hand.

### 1.2 Hard-coded but not wired up, or tooling only

| crate | what | distinct | where | status |
|---|---|---:|---|---|
| engine | `OVERRIDES` (tag lookup, datum arrays, locale), each with its 1.0 LTCG convention | 22 | `crates/engine/src/abi.rs:100-252` | nothing registers them yet (`engine/src/runtime_glue.rs:6`); they'd misbehave silently once something does |
| engine | globals: tag index, tag entries, empty string | 3 | `engine/src/tags.rs:17,20`, `locale.rs:18` | same |
| engine | datum array header (0x4c bytes, 15 fields) | 15 | `engine/src/datum.rs:20-81` | engine struct, probably the same across builds |
| hs | catalog tables (functions, count, types, script kinds, externals) | 5 | `crates/hs/src/catalog.rs:8-12` | **fail-closed**: checks title ID, entry, certificate version, builtin count and the dispatch instruction (`:53-57`) |
| hs | native evaluator addresses plus argument, return and float-constant addresses | ~24 | `crates/hs/src/vm/image.rs:65-107` | **fail-closed**: validated at `:133-170` |
| hs | baked counts (911 builtins, 62 types, 0x41d externals, the thread size) | ~6 | `catalog.rs:67-112` | fail-closed through the catalog gate |
| perf | d3d bench entry points | 3 | `crates/perf/src/pushbuf.rs:36-38` | synthetic memory only |

### 1.3 Analysis-time, hard-coded

| crate | what | distinct | where | failure mode |
|---|---|---:|---|---|
| analyze | known retail dispatch tables (lifecycle, HaloScript evaluators, object descriptors, animation codecs, compression callbacks, UI vtables) | ~9 | `crates/analyze/src/dispatch.rs:35` (gate), `retail_tables` up to `:235` | **fail-closed as a unit**: gated on title ID + entry point, every slot checked; another image just loses that coverage |
| analyze | campaign roots | 6 | `dispatch.rs:12` | coverage test and example only |
| lift | `_ftol2` native replacement | 1 | `crates/lift/src/crt.rs:36-53` | gated on title ID + entry, otherwise the generic lift (safe, but slower). No byte check inside the gate |

### 1.4 Stamps, caches and gates that assume one image

| what | key | where | with an update XBE |
|---|---|---|---|
| dylib stamp | fnv64 of the XBE + generator hash + passes + codegen + ABI + cflags | `crates/boot/src/lib.rs:65-95` | rebuilds, never reuses (safe) |
| emit receipt | SHA-256 of the XBE + h2 binary + settings | `crates/cli/src/emit_cache.rs:31-38` | safe |
| fallback unread-target cache | fnv64 of the XBE + source hash | `crates/boot/src/fallback.rs:503-513` | safe |
| object cache | per-source fnv64 | `crates/emit/src/compile.rs:292` | safe to share between images |
| default out dirs `$TMPDIR/h2gen`, `$TMPDIR/h2boot` | none | cli | two images thrash one dylib slot |
| data root `$TMPDIR/h2boot-data` (and `Config::for_title`) | title ID only | `boot/src/lib.rs:378`, `vfs` | 1.0 and the update share saves and the cache partition (see section 4) |
| `h2 play --follow` caches | commit only | `crates/cli/src/follow.rs:1091,1103` | stamp check still applies |
| title ID + entry gates | `0x4d530064` + `0x2d0aee` | `analyze/dispatch.rs:35`, `lift/crt.rs`, `hs/catalog.rs` | probably excluded, since an update relinks and the entry moves. But a build that keeps the entry would be wrongly accepted |
| exact 1.0 SHA-256 | `wma::RETAIL_SHA256` | `crates/wma/src/lib.rs:20`; also `engine/tests/differential.rs:44` | **boot fails**: `boot/src/dsound_glue.rs:136` propagates the `from_xbe` error |
| map header | version 8, build string `02.09.27.09809` | `crates/map/src/reader.rs:54-66` | disc maps still open; any map the update ships with a new build string is refused loudly |
| map writer size limits | per cache type, from the 1.0 validator | `crates/map/src/writer.rs:137-142` | tooling only |

### 1.5 Test-only

31 files read `$H2_XBE` or `disc/default.xbe`, with 35 `#[ignore]` tests
between them. They pin 1.0 facts: ret sizes, prologues, conventions decoded
with iced-x86, table contents.

- **Image-backed pins** (useful later as per-version validators):
  `analyze/tests/{xbe_pins,dispatch_coverage}.rs`,
  `d3d/tests/{xbe_tables,fog_xbe,round5,round6,round7}.rs`,
  `dsound/tests/{xbe_pins,xwma}.rs`, `xapi/tests/{xbe_pins,save_pins,checkpoints}.rs`,
  `bink-hle/tests/xbe_pins.rs`, `runtime/tests/{loader,memory_ranges}.rs`,
  `lift/tests/{entry,x87}.rs`, `emit/tests/entry.rs`,
  `kernel/tests/{cache_slots,image,net}.rs`, `hs/tests/vm_disc.rs`,
  `engine/tests/differential.rs` (asserts the 1.0 SHA-256),
  `boot/tests/{audio_idle,crt_copy,hosted}.rs`.
- **Synthetic tests that assert 1.0 addresses** (they call handlers at fixed
  addresses, but every byte is synthetic):
  - `d3d/tests/registration.rs` (~29)
  - `d3d/tests/{passes,fragments,recheck_fixes,round5-7}.rs` (10-40 each)
  - `dsound/tests/guest.rs` (~86)
  - `dsound/tests/register.rs` (66)
  - `xapi/tests/{checkpoints,routing,register,host_boot}.rs` (19-54 each)
  - `kernel/tests/cache_slots.rs` (~37)
  - `bink-hle/tests/synthetic.rs` (~22)
  - `engine/src/tests.rs` (pins the override list at `:727`)
  - `hs/src/tests.rs` (builds a synthetic XBE around the catalog anchors)

  These keep working unchanged as long as the 1.0 map is the identity.
- **Title ID literal**: `xapi/tests/{saves,common/mod,host_boot,checkpoints}.rs`.
  It doesn't change: updates keep the title ID.
- **Scripted input**: `docs/runtime/BOOT.md` boot scripts are keyed to present
  numbers (`START@f310 ...`), and `--input-timeline` uses `at_ticks` guest
  time (`docs/input/input.md`). Neither uses addresses, but both are tuned to how
  long 1.0 takes to boot (see Risks).

## 2. What already copes with a different image

- **Function discovery and the CFG** (`crates/analyze`): discovery starts
  from the entry point and the kernel thunks, then follows calls and tail
  calls, code immediates, data pointers and gap scans. Jump tables are sized
  from their guards. `pointer_seeds.rs` is fully derived; its literals are
  synthetic tests. The generic dispatch scan works on any image. The retail
  tables are extra coverage, and they fail closed.
- **Lift, IR, opt, emit, x87**: no image addresses. Generated symbols
  (`h2fn_XXXXXXXX`) and the `/* 0x... */` comments carry each image's own
  addresses, and perf symbolization reads them back
  (`perf/src/symbolize.rs:153`).
- **Kernel**: imports go by ordinal. The thunk table is decoded from the
  header (`kernel/src/image.rs:127`), the title ID comes from the certificate
  (`kernel/src/lib.rs:381-396`), and so does XAPI's cache-row title ID
  (`xapi/src/cache.rs:165`). `boot/src/stopgaps.rs` and `wrappers.rs` are
  keyed by ordinal, and sections come from the header.
- **XDK identification** (`crates/symbols`, `docs/lifter/xdk-libraries.md`): it reads
  the library build from the XBE's library table and matches in waves, so it
  would find the same XDK functions in an update. But its only consumer
  today is `h2 symbols` (`cli/src/main.rs:434`). **No hook is currently
  placed through it.** The database is downloaded locally (ODbL, not
  vendored), so it can't be a hard boot dependency. As an analysis-time
  anchor source, though, it's ideal.
- **Fail-closed already**: `hs` (catalog and VM), the analyzer's retail tables,
  `lift`'s `_ftol2`, WMA tables, and the dylib and emit stamps. Of these,
  only WMA turns into a hard boot failure.
- **Data format**: `crates/tags`' ~3,600 hex literals are tag field offsets,
  block sizes and fourccs. The 154 XBE addresses in that crate are provenance
  comments. `crates/map` refuses unknown versions and build strings.

## 3. Design

### 3.1 Version detection

Add an `ImageId` in `crates/xbe`, computed once per image:

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

New crate `crates/imagemap`. It depends on `xbe`, `analyze` and,
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
never committed. It holds only addresses, scores and hashes of normalized
instruction streams.

Matching runs in passes, roughly the way BinDiff- and Diaphora-style differs
work. Those tools are cited as prior art only; we don't use their code.
Every pass enforces one-to-one: a target address matched twice is ambiguous
on both sides.

1. **Fixed anchors.**
   - The entry point.
   - Kernel thunk slots, by ordinal, which gives exact data matches for the
     import table.
   - XDK functions that `crates/symbols` finds in both images under the same
     name, with no `ambiguous` or `shared` flag.
   - The data symbols that symbols votes for (the D3D device pointer, state
     offsets).
2. **Exact normalized bodies.** Decode each discovered function with iced-x86.
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
6. **Data by operand correspondence.** For two functions whose normalized
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
  `body_hash`. When a hook is installed, normalize the target function again
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
  from `D:`. `crates/map`'s version 8 / `02.09.27.09809` gate keeps working
  for them.
- **To check once files are available:**
  - Does the update directory carry any `.map` files or replacement shared
    maps? If so, check their header version, build string and tag index
    layout. Run `crates/tags` decoders over every tag, and diff the field
    sets against a disc map of the same name.
  - Downloadable map packs sit beside the update in `TDATA`, under `$c`.
    Answered for the Map Pack disc ([map-pack.md](../game/map-pack.md)): its nine
    maps have the same version 8 header and build string as the disc's, and
    `crates/map` opens them. The update, unlike 1.0, lists them: 1.0 reads
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

Design only: `h2 image`, `h2 play --image`, `H2_IMAGE` and `H2_XBE_UPDATE`
(the last as a test input, section 5) are not in the code. The only update
handling today is `h2 import-mappack` copying the update aside.

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

**Per-version acceptance** (`#[ignore]`, `H2_XBE` plus `H2_XBE_UPDATE`):

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

## 4. Effort and ordering

The estimates are focused engineering days, with wide error bars on everything
after files arrive.

**Before the user has update files:**

| step | work | estimate |
|---|---|---:|
| A | `ImageId`, one base hash, replace the three title-ID + entry gates and the duplicate hashes | 0.5 d |
| B | `Anchor` + `ImageAddrs` refactor across d3d, dsound, xwma, xapi, bink-hle, boot glue, engine, `GuestLayout`, `FrameWait`, `dsound::Layout`, `CacheGlobals`, loader arena. Identity map, no behaviour change | 2-3 d |
| C | Fill in `xdk` names, add the symbols cross-check test, derive `GuestLayout` globals from symbols' data votes, remove the duplicate addresses | 1-2 d |
| D | `crates/imagemap`: normalizer, passes 1-6, one-to-one solver, report, synthetic and property tests | 4-6 d |
| E | Byte check at registration, `HookClass` audit for every entry | 1 d |
| F | Structural location for the `hs` catalog and the retail dispatch tables; the WMA range-equality path | 1-2 d |
| G | CLI: `h2 image list/map`, `--image`, per-image out and data dirs, launch-path mapping | 1-2 d |

That's about 11-17 days. A, B and D are the critical path. B and C are worth
doing anyway: they make every hook auditable against the image.

**Once files arrive:**

| step | work | estimate |
|---|---|---:|
| H | Run the map, triage refusals and ambiguities, audit XDK builds | 1-3 d |
| I | Data checks from 3.5 (shipped maps, saves, cache, hs) | 1 d |
| J | Port the image pins to the per-version form | 1 d |
| K | Boot to menu, then a level: fix whatever moved that the matcher couldn't prove | 2-5 d, unknown |
| L | Retune input scripts, benches, docs | 1 d |

That's about 6-11 days. If the update relinked a different XDK, add the
per-build D3D and DSound layout work on top, probably another week.

## 5. Risks

- **Inlined or rewritten hot functions.** Game-code hooks (`FrameWait`, the
  movie wrappers, the voice init, the engine overrides once registered) have
  no library identity. If the update changed or inlined one, its hook goes
  unmatched. `Optional` hooks fall back to the generated original at some
  speed cost. `Required` ones need manual work. One changed basic block
  takes a function out of `Exact`, and the LTCG rule then refuses it. That
  is the intent: it's conservative, at the cost of more manual triage.
- **LTCG relinks.** Re-running link-time code generation over unchanged D3D
  source can still change register allocation, and with it every adapter's
  convention. `Exact`-only placement for LTCG entries is the guard. Without
  it, adapters would read the wrong registers without any error.
- **Code tied to 1.0 behaviour beyond hook addresses.**
  - The d3d device model depends on how 1.0 game code writes push buffers
    and device fields inline (`docs/graphics/d3d.md`: the inline render-state stores,
    `MakeRequestedSpace` callers, inline `BEGIN_END`).
  - The `_ftol2` replacement, the `hs` VM natives, and `engine` contracts
    proven by differential tests against 1.0.
  - Any native rewrite of hot functions coming from in-flight work
    (`hot-overrides`).

  All of it should register through anchors, so it either gets proven
  unchanged (`Exact`) or gets dropped. A differential run of the original
  generated function against the native one, on the update image, is the
  per-version proof for anything that isn't `Exact`.
- **Logical-time determinism.** Logical time advances on the 60 Hz vblank
  clock and audio quanta, and scripts are keyed to present counts or
  `at_ticks`. An update that does different work per frame, or loads for
  longer, shifts every scripted timeline and menu bench, so input scripts
  and bench baselines have to be per image. Golden traces (`docs/lifter/golden-trace.md`)
  are also per image by construction. Comparing 1.0 and 1.1 runs for a
  regression only works at the level of state (menu reached, level loaded),
  not frame-exactly.
- **False positives in the matcher.** A wrong mapping is worse than none.
  Enforcing one-to-one, requiring agreement for data votes, checking bytes at
  registration and gating LTCG entries on `Exact` keep precision first. The
  property test measures it.
- **Shared persistent state.** 1.0 and the update share UDATA and the cache
  partition under one title ID. Keeping a data root per image avoids
  corrupting checkpoints until the save formats are known.
- **Kernel imports.** An update may import ordinals that 1.0 doesn't.
  `kernel` already has a supplemental-export bridge (`kernel/src/lib.rs:237`),
  so make `h2 image list` report unknown ordinals up front. The `0x303`
  update adds one: ordinal 270, `RtlCompareString`.

## 6. The first update we have: `0x303`

From the Multiplayer Map Pack disc ([map-pack.md](../game/map-pack.md) has the
details). The disc carries eight signed copies, `default.xbe.03` to `.10`,
for base discs with certificate versions 3 to 10. They're one build: they
differ only in the header signature and certificate, and each one's version
is `0x300` plus its base's. Our 1.0 disc is version 3, so its update is
`default.xbe.03`, version `0x303`.

What it means for the design above:

- **Same XDK.** Every library is 1.0.5849, the same set as 1.0. The
  per-build layout risk in 3.1 doesn't apply to this update: `CDevice`
  offsets, render-state numbering, DirectSound layouts and the library code
  should all be the same, only relocated.
- **Everything moved.** The entry point is `0x2d6f2e` (1.0: `0x2d0aee`), the
  thunk table `0x417b60` (`0x411520`), the image `0x57d2a0` bytes
  (`0x5754c0`). The title-ID + entry gates (1.4) correctly reject it, and
  every literal hook address in section 1.1 is likely wrong: the address map is
  needed.
- **One new kernel import**, ordinal 270 (Risks, "Kernel imports").
- **Version label.** Under the precedent in 3.1, the update can be named
  `0x303` (certificate version) in code, keyed by its hash once measured on
  the user's machine. The hash isn't recorded here.
- **The update path is confirmed.** 1.0 builds
  `\Device\Harddisk0\Partition1\TDATA\<title>\$u\<file>` itself, so the
  relaunch in 3.6 is real. Keep the update out of the emulated `$u` until
  `--image` selects it, as `h2 import-mappack` does.

Next steps, in the order of section 4: A to G can now be tested against a
real update (`H2_XBE_UPDATE=<data>/updates/4D530064/00000303/default.xbe`),
and H can start.
