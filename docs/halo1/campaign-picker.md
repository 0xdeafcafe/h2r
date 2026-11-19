# More than one campaign in the menus

Status: **built, and confirmed in the game by the owner (2026-11-17).** The level-list records, the
picker, the campaign of a started level and per-campaign progress are
proven by unit tests and an ignored test against the user's disc. Nothing
here has been run in the game. Started 2026-11-16. Part of
[campaign.md](campaign.md) (step 8, §4).

The goal: a campaign picker (Halo 2, Halo 1, ...) in front of the campaign
level list, and progress kept per campaign. With no second campaign
installed, the game is exactly retail: nothing is hooked, nothing is
written and the disc isn't read for it.

Labels: **E** read from the retail XBE with `h2 disasm` (a full linear
disassembly of `.text`), **V** measured on the user's disc (the ignored
test below), **S** inference. Addresses are 1.0's.

## 1. How 1.0 builds its campaign level list

### The data (V)

Three lists in the globals tag (`matg`) of `mainmenu.map`. All three have
free slots, the way the multiplayer list has 38 for downloaded maps
([map-pack.md](../game/map-pack.md) 8.1):

| list | where | element | on the user's disc |
|---|---|---|---|
| start levels: the levels the game can start | `+0x170`, element 0, block `+0` | `0x108`: `+0` campaign id, `+4` map id, `+8` scenario path (`0x100`) | 20 slots: 15 of campaign 1, 5 free |
| level-select campaigns | `+0x178`, element 0, block `+0` | `0xb44`: `+0` campaign id, `+4` nine names (32 UTF-16 units), `+0x244` nine descriptions (128 units) | 4 slots: campaign 1 (no name), 3 free |
| level-select levels | `+0x178`, element 0, block `+8` | `0xb50`: `+0` campaign id, `+4` map id, `+8` picture (`bitm` reference), `+0x10` nine names, `+0x250` nine descriptions | 20 slots: 15 used, 5 free, **each free slot with a picture of its own** |

A free slot has campaign id and map id `-1`. The `+0x178` element's block
`+0x10` is the multiplayer list. Campaign 1's map ids are `100 * chapter
+ n`: `0x1` The Heretic, `0x65` Armory, `0x69` Cairo Station, `0x12d`
Outskirts, ... `0x325` The Great Journey. Ids don't follow play order
everywhere: High Charity is `0x321` and Uprising `0x2c1`.

### The code (E)

| Address | What |
|---|---|
| `0x19c190` | refreshes the UI level lists (through the function table at `0x43ca94`, no direct caller): `patch.lvl` (`0x19c120`), then **`0x19c6d0([0x4ee4e8], 1)`**, then the multiplayer list `0x19c7e0([0x4ee4e4])` |
| `0x19c6d0` | stdcall `(array, campaign)`, `ret 8`: walks the start list, gathers the map ids (not `-1`) whose campaign id matches into a **20-dword stack buffer with no bound**, sorts them rising (`0x13de30`, comparator `0x19c7c0`), clears the data array (`0x16b7a0`) and adds one element a map id (`0x16b840`; the id at element `+4`) |
| `0x19c670` | returns the campaign data array `[0x4ee4e8]` (when the globals have UI level data) |
| `0x2c4ea0` | the level select screen's constructor (screen 11, thiscall, `ret 8`): **writes `0x54e7c0 = 1`**, takes the highest progress over the signed-in profiles (`0x190565`), then adds each map id of `0x4ee4e8` in order and stops after the first one above that progress (so: the finished levels and the next one). Each row's record comes from **`0x19c1f0(map id)`, which matches the map id alone**, and its picture through `0x23625d`. With its second argument set (the lobby's), ids below `0x69` are skipped and the progress is at least `0x68` |
| `0x2b130a` | the main menu's level-select loader (constructor flag 0); opened at `0x2b16d4` through `0x149f49` |
| `0x2b136d` | the lobby's (flag 1); opened at `0x2bb923` (Game Setup's level row) |
| `0x23643f` | the menu's campaign start: **`0x19c270(0x54e7c0, 0x54e7c4)`** for the level (through `0x148d61`), the scenario path copied into the options (`+0x1c`), campaign id `0x54e7c0` and map id `0x54e7c4` into `+0x14` and `+0x18` ([level-boot.md](../runtime/level-boot.md)) |
| `0x19c270` | `(campaign, map)` to the start record: both ids compared |
| `0x19c320` | scenario path to the start record, by basename |
| `0x19c440` | `(campaign, map)` to the next level: the smallest map id above it **in the same campaign** |
| `0x19c4e0` | a campaign's largest map id |
| `0x163610` | after a level is won, the next level's path from the active options' campaign and map ids (by path when the campaign id is `-1`) |
| `0x199a92` | Campaign "continue": **writes `0x54e7c0 = 1`**, map = the profile's last level (at least `0x69`; `0x69` when campaign 1 has no such level) |

Writers of the selected campaign `0x54e7c0`: `1` at `0x2c4f25`,
`0x199a96` and `0x2b17d0`; `-1` at `0x199a5e` and `0x230b4a`; a reset at
`0x14781f`. Nothing ever writes another campaign id.

**So:** looking a level up, starting it and finding the next one are keyed
by campaign id, and a second campaign id works there as data alone. Three
things assume campaign 1 and need code: the list build asks for campaign 1
only; every writer of the selection writes 1; and the level select finds
a level's record by map id alone, so **map ids must be unique across all
campaigns**. Two limits come from the code: at most 20 levels a campaign
(`0x19c6d0`'s buffer), and map ids at least `0x69` if the lobby's level
select is to show them. Nothing validates the records (no version, no
CRC, unlike `patch.lvl`): the only checks are the id compares above, and
the tests assert exactly those.

## 2. How progress is kept (E)

The local player slots are 4 x `0xc70` at `0x54e8e0`; `+0` bit 4 means
signed in, and `+0x18` holds the profile's `0x1e0`-byte block, which is
what is saved (`0x2153dd` writes `0x1e0` bytes through `0x2173d0`, the
save core of [xapi.md](../runtime/xapi.md); the container is content type
0, likely as for saved variants). Readers copy it with `0x18fc44`;
setters change the copy and hand it to `0x18fcc4`, which in a game only
updates the slot and marks it dirty (`0x18fb34`, flag `0x40`), and
otherwise saves at once (`0x18fd20`).

| Field | What | Written by |
|---|---|---|
| `+0xec`, 4 dwords | the highest map id **finished**, per difficulty (0 easy .. 3 legendary) | `0x19043f` (stdcall `(slot, difficulty)`, map id in ESI, `ret 8`), only raising it; called for each slot by `0x1388e0` when a campaign game is won, with the options' map id and difficulty (`+0x18`, `+0x12a`) |
| `+0x5c` | the last level **started**, a map id | `0x19040d` (map id in EAX): every signed-in slot, and `0x510840` for no profile; called at launch (`0x18f0a0`, campaign options only) and by the level select's confirm (`0x2c5143`) |

`0x190565` answers the highest `+0xec` over all difficulties and signed-in
profiles (`-1` none); `0x1a06f0` the same for one profile, counting only
ids the level select has a record for. The difficulty isn't stored (only
the selection `0x54e7c8`). **No campaign id is stored anywhere**: progress
is a bare map id compared with `>`, so a second campaign's levels, whose
ids must sit above Halo 2's, would unlock every Halo 2 level the moment
one is finished. Per-campaign progress therefore can't live in the
profile.

## 3. Where a campaign level's map comes from (E)

`0x214db0` turns a scenario path into the file the cache loader opens: a
path starting `t:\` or `w:\` becomes `<path>.map`; anything else becomes
**`d:\maps\<basename>.map`**. The menu's start copies the start record's
path into the options, so a level's map file is named by the basename of
its record's path.

Halo 1's converted maps go in `<data>/campaigns/halo1/`; `h2 import
halo1`, or `h2 play` with `halo1.iso` beside the disc, writes them there
([campaign.md](campaign.md) §10).
Choices:

1. **The VFS serves them as `maps\<name>.map`** (chosen). The records keep
   `scenarios\solo\<map>\<map>`; the game copies the map into a cache slot
   exactly as it does a Halo 2 level. The VFS already serves `maps\` from
   packages ([maps.md](../game/maps.md)); it gains names the disc lacks, from an
   installed campaign's folder. Halo 1's names (`a10` ... `d40`) don't
   meet any Halo 2 name, and the manifest refuses a map named twice.
   **Built:** `vfs::CampaignMapsBackend`, directly over the disc (so map
   packages still apply over it). A level whose map the disc lacks is
   served from `<data>/campaigns/<key>/<map>.map` with the disc's
   `mainmenu.map` metadata resized; a name the disc has is never shadowed.
   A level is listed only when one of the two exists.
2. A `t:\` path (the title's own data partition, as Map Pack packages
   are). No VFS change, but the maps would have to live under the emulated
   hard disk, and they would load through the content path rather than the
   one Halo 2's own levels use. Rejected.

## What we do

### A campaign on disk

`<data>/campaigns/<key>/campaign.toml` (`content::campaign_list`):

```toml
format = 1

[campaign]
key = "halo1"                 # = the folder's name
name = "Halo: Combat Evolved"
description = "..."           # optional
order = 10                    # optional: the picker's order (then key)

[[level]]                     # play order; 1 to 20
map = "a10"                   # maps\a10.map
name = "The Pillar of Autumn"
description = "..."           # optional
scenario = "..."              # optional; default scenarios\solo\<map>\<map>
picture = "01b_spacestation"  # optional: a Halo 2 level whose picture it shows
```

`campaign_list::HALO1` is Halo 1's: a10, a30, a50, b30, b40, c10, c20,
c40, d20, d40 with their names; their pictures are Halo 1's own ("Pictures", below). `campaign_list::FIXTURE`
is a **test campaign** of three retail Halo 2 levels (Outskirts, Delta
Halo, Gravemind, named "(2)", each with its own picture), so the picker
can be seen before any converted map exists. At boot
(`campaigns::boot_state`) a campaign is listed when its manifest parses,
its key is its folder's name, and at least one level has a map (in its
folder, or on the disc); levels with no map yet are left out and the log
says how many.

### Ids

Campaign ids follow the largest the disc uses (so Halo 1 is 2 when it is
the only one); map ids are `0x1000 * (n + 1) + 1 + level` for the `n`th
campaign in picker order: rising in play order, far above Halo 2's
(`0x1`-`0x325`) and at least `0x69`. `campaign_list::check` refuses a
shared map id or more than 20 levels in a campaign; `check_built` refuses
ids out of order or below `0x69`.

### The lists (`campaigns::extend`)

Our records go into the free slots first; when those run out (Halo 1's ten
levels need more than five), the whole list is copied into our guest
memory with ours after the retail ones and the block repointed. Retail
records keep their places. A level-select record takes its `picture`
level's picture, else its free slot's own, else the list's first. It runs
before each hooked reader, so it follows map loads (each load brings the
tag back from the file); it does nothing when the lists already hold our
first campaign, and nothing when the loaded map has no list. A campaign
level's own globals have a start list too, which the next-level lookup
reads after a level is won; the hook there extends it the same way.

### Pictures (`campaign_cards`)

The level select's pictures are mainmenu's
`ui\code_global_bitmaps\campaign0` to `campaign19`, one per list slot: 220x207 linear `ARGB8888`
(rows padded to 896 bytes, LOD `0x2d480`). The last five belong to the
free slots, so they are not reused.

- **At import**, `h2 import halo1` writes each level's Halo 1 picture as
  `<data>/campaigns/halo1/cards/<level>.png`. The picture is the top-left
  139x113 of its `ui\shell\bitmaps\sp_levels` image
  (`h1convert::ui::campaign_card`).
- **At boot**, `campaign_cards::CardRewrite` reads the cards of the
  levels listed.
- **When the VFS first serves `maps\mainmenu.map`**, CardRewrite adds one
  `bitm` per card. It is a `vfs::Rewrite`, chained after the button
  icons' font rewrite (`vfs::Chain`).
  - The disc's map is decomposed with every tag where it was
    (`TagSet::from_map`), and the index is given room for more tags.
  - Each card is scaled to cover 220x207: centred, the overflow cropped,
    bilinear.
  - Each card becomes a bitmap shaped like `campaign0`
    (`tags::rgba_bitmap`): its header and image record copied, then its
    own pixels as a new raw resource, its own datum, and cache handles
    `-1`. The tag is named `ui\code_global_bitmaps\<key>_<level>`.
  - Tag memory grows one page, from `0x1d3000` to `0x1d4000`. That is
    past the builder's main-menu bound, which is only the largest retail
    main menu (map-format.md: no loader constant, the arena limits it, and
    the arena holds a campaign map's `0xc24000`). So the bound is lifted
    for this rebuild.
  - The map is built again with a fresh checksum. The header checksum's
    one reader is the cache copy's compare (`0x214618`), and it passes.
  - The datums go into `State::cards`.
- `campaigns::extend_with_cards` gives a level with a card that picture,
  ahead of its `picture` level's.

**The frame is in the pixels (V, 2026-11-18).** Halo 2's level pictures
show a pale border with the bottom-right corner cut off. The screen draws
none of it: the level select's picture widget is one plain bitmap block
(`[177 39 -30 259]`, `h2 ui mainmenu load_level`), and every used picture
(`campaign0` to `campaign14`) carries the frame itself. All fifteen have
the same alpha, pixel for pixel: 834 opaque border pixels (a one-pixel
rim and the diagonal of the cut, `(154, 171, 189)`, `campaign14` a little
lighter), 136 clear pixels past the cut, and the picture at alpha 191, so
it is drawn a quarter see-through. The free slots' five are fully clear.
So a card goes in the same frame: `campaign_cards::framed` takes
`campaign0`'s own pixels from the disc's mainmenu at rewrite time
(`Template::rgba`) and, pixel by pixel, keeps the template's where it is
opaque (the border), clears where it is clear (the corner), and puts the
card's colour at the template's alpha elsewhere. Nothing of the frame is
in the repository. A template with no frame (`has_frame`: no opaque
pixels, or no more part see-through than opaque ones) leaves the cards
plain, and the log says so.

**Descriptions.** Each Halo 1 level has a one-line description in
`HALO1`, written for this project (not the game's text), which goes to
the record's `+0x250` in all nine languages, where the level select reads
Halo 2's own. An import made before they existed keeps its
`campaign.toml` (the import never rewrites one it finds), so
`campaigns::installed` fills a manifest's missing descriptions from the
built-in one (`CampaignManifest::fill_from`); a description the user
wrote is kept.

**Only the first level listed.** This is retail's unlock rule, working
per campaign, not a bug. The level select's constructor (`0x2c4ea0`) adds
the levels in id order up to and including the first one above the
progress it is given. For one of our campaigns `PROGRESS_MAX` answers our
own progress (`progress.toml`), which counts a level only when it is
**won** (`PROGRESS_RAISE`, from the game-won path). With nothing won it
answers `-1`, and the list is the first level alone, as Halo 2's is on a
new profile. On the user's machine (2026-11-18) `progress.toml` has
`finished = [-1, -1, -1, -1]` and `last = 4097` for Halo 1: The Pillar of
Autumn was started, not finished. Finishing it lists Halo next, and so
on.
- Retail tags keep their bytes and addresses. On the user's disc the map
  stays under the rewrite's 64 MiB limit (the boot disc test). If the
  rewrite fails, mainmenu is served as it is and levels keep the pictures
  they had.

### Hooks (`campaign_menu::Campaigns`, feature `campaigns`)

| Point | Kind | What |
|---|---|---|
| `CAMPAIGN_LEVEL_LIST` `0x19c6d0` | before | the lists; the campaign argument becomes the picked campaign (1 until the picker says otherwise) |
| `LEVEL_SELECT_NEW` `0x2c4ea0` | before, after | the lists; after, `0x54e7c0` is the picked campaign instead of the constructor's 1 |
| `PROGRESS_MAX` `0x190565` | after | while the level select is being built for one of our campaigns: our progress for the signed-in profiles (`-1`: the first level only) |
| `CAMPAIGN_START` `0x23643f` | before | the lists; `0x54e7c0` becomes the campaign the selected map id belongs to |
| `NEXT_LEVEL` `0x19c440` | before | the lists (in the level's own globals): the game-won path (`0x18e960`, through `0x1388e0`) and `0x163610` find the next level here; the campaign argument becomes the map's campaign (`0x24a9e9` passes 1) |
| `CAMPAIGN_LEVEL_LOOKUP` `0x19c270` | before | the lists; the campaign argument becomes the map's campaign (`State::lookup_campaign`): the level select's press (`0x2c5118`) and Continue (`0x199ab7`, `0x199ac5`) pass 1 for any map |
| `PROGRESS_RAISE` `0x19043f` | before | a map id of ours: kept in our progress for that slot's profile, and the original skipped, so the profile keeps Halo 2's progress only |
| `LAST_LEVEL_SET` `0x19040d` | before | a map id of ours: kept as that campaign's last level for each signed-in profile; the original skipped |
| `SCREEN_REQUEST` `0x149f49` | before | the main menu's level-select request (return address `0x2b16f3`, loader `0x2b130a`): the loader becomes the picker's |

### The picker (`campaign_menu::PickerRows`, screen `campaigns`)

Our own list class ([feature-engine.md](../engine/feature-engine.md) "UI
step b", as the Skulls screen is), titled "Campaign": the campaigns whose
`order` is below 20 (`campaign_list::HALO2_ORDER`; Halo 1's is 10), then
"Halo 2", then the rest, so Halo: Combat Evolved is first and Halo 2
second. A press sets the picked campaign, rebuilds the level
select's list with the game's own `0x19c6d0([0x4ee4e8], id)`, and opens
the retail level select (`0x2b130a`). This needed one new press result in
`ui::Chosen`, `Retail { calls, loader }`, made by
`list_class::open_retail`. The labels are our strings (`ID_BASE +
0x0200_0000`), served with the layouts'.

### The picker's detail pane (`campaign_info`, 2026-11-18)

The picker is laid out as the level select is (its list, bracket, picture
box and description text: `skulls_menu::LIST_LAYOUT`, `FRAME`,
`DESCRIPTION_TEXT`, as Game Options is). For the focused campaign:

- **the picture**: its first level's, from the level select's records in
  the loaded map (`campaigns::first_pictures`; Halo 1's is a10's framed
  card). The widget is a copy of the level select's own picture block
  (`campaign_menu::PICTURE`); a bitmap widget reads its block's `bitm`
  reference (`[widget + 0x70] + 0x1c`) at every draw (`0x2b05bb`,
  `0x2b0b26`, E), so on a focus change `list_class::set_picture` writes
  the row's datum there (`Rows::picture`);
- **the text** (`campaign_info::Info::text`): an original line about the
  campaign, the release year and studio as plain facts (Halo 2: 2004,
  Bungie; Halo 1: 2001, Bungie), the level count, the progress of the
  signed-in profiles (levels the level select lists of the total, and
  the hardest difficulty the last level was won on), and for Halo 1 the
  Assets and HUD options. Halo 2's progress lives in the profile and isn't
  shown.

The texts are our strings (family `campaign picker info`), installed as
placeholders of `INFO_ROOM` bytes and rewritten in place while the picker
is up (`Rows::live_text`), so progress and options are current. The
profiles and pictures are read as the picker opens (the `SCREEN_REQUEST`
hook already there: no new hook, no order changed).

The kit describes it (`Describe for PickerRows`, screen `campaigns` in
`ui_kit::screens`), so `h2 ui preview campaigns` and the sheet draw it;
the preview's picture box shows the level select's own placeholder bitmap,
since the cards exist only in the rewritten mainmenu.

### Progress (`campaigns::Progress`)

`<data>/campaigns/progress.toml`, per profile name (the skulls'
convention: a slot's profile name, `default` when no one is signed in),
per campaign key:

```toml
[Chief.halo1]
finished = [-1, 4099, -1, -1]   # highest map id finished, easy .. legendary
last = 4100                     # last level started
```

Written whole through a rename when it changes; an unreadable file is
reported and never overwritten.

### The lookups by `(campaign, map)`

Every 1.0 caller of the start-record lookup `0x19c270` and the next-level
lookup `0x19c440` (found by scanning the XBE for calls to them):

| Call | Campaign passed | Map |
|---|---|---|
| `0x2c5118` (level select, a press) | **1** | the pressed level's |
| `0x199ab7`, `0x199ac5` (Continue) | **1** (`0x54e7c0` set to 1) | the profile's progress |
| `0x148d6d` | `0x54e7c0` | `0x54e7c4` |
| `0x18e9ec`, `0x18e9da` (game won) | the game options' | the next level |
| `0x163664`, `0x163658` (`0x163610`) | the level's own | its next |
| `0x19c98a` (`0x19c960`) | its caller's | its caller's |
| `0x24a9e9` (`0x19c440`) | **1** | `[ebp+0x68]` |

The ones passing 1 find no record for one of our levels: picking Halo CE
and pressing a level faulted at `0x2c5125` reading the null record. Map ids
are unique across campaigns, so both lookups are hooked at entry and the
campaign argument becomes the one that owns the map
(`campaigns::State::lookup_campaign`); a retail map asked under one of
ours goes back to 1. The MP lookup `0x19c5f0` takes a map id alone and
reads the multiplayer list, which campaign ids never reach.

## Tests

- `content` `campaign_list::tests` (9): the Halo 1 manifest's levels and
  order; ids after retail and rising; several campaigns ordered by `order`
  then key and never sharing an id; campaign ids after the disc's largest;
  repeats and taken ids refused; manifest validation (format, key, names,
  20 levels, duplicate maps, scenario paths that don't end in the map's
  name, pictures); the shared-id and 20-level rules; records read back
  field for field; names cut to fit.
- `boot` `campaigns::tests` (11, over a fake guest with retail-shaped
  lists): a small campaign goes into the free slots and nothing moves; a
  campaign past the free slots is copied into our memory and the blocks
  repointed; two campaigns keep apart; a level map's start list alone;
  no globals or no list; a full region fails without repointing; the
  start takes the campaign of the selected level; progress per difficulty
  and profile, persisted, never lowered, an unreadable file left alone;
  installed campaigns and the reasons others aren't; the picker's rows.
  Each extension is checked through transcriptions of the game's readers
  (`0x19c270`, `0x19c6d0` with its 20-entry limit, `0x19c440`, `0x19c1f0`).
- `tags` `rgba_bitmap::tests` (3): BGRA rows padded to 64 bytes and the
  LOD to 128 (220x207 gives the retail `0x2d480`); a new bitmap takes the
  template's shape and its own pixels on a decomposed map, the template
  untouched; a bad template refused.
- `boot` `campaign_cards::tests` (4, and 1 ignored): cover scaling; cards
  into a mainmenu-shaped map with every retail tag's bytes kept; the
  rewrite wants mainmenu only and fills the table; cards read per
  campaign folder. The ignored test, `the_disc_s_mainmenu_takes_ten_cards`,
  puts ten cards into the disc's mainmenu: every retail tag is unchanged,
  each card is a 220x207 `0x2d480`-byte LOD holding the scaled pixels,
  the checksum is good, tag memory grows by at most two pages (it grows
  by one), and the map is under 64 MiB (61.6 MB). The rebuild takes 31 ms.
  `H2_DISC=disc/halo2.iso cargo test -p boot --lib campaign_cards --
  --ignored`.
- `vfs` `rewrite::tests::a_chain_rewrites_what_either_wants_one_after_the_other`.
- `vfs` `campaign_maps::tests` (2): a map the disc lacks is served from
  its campaign folder (stat, read, listing, host file, `MapId`); the
  disc's own map is never shadowed; a missing host file stays missing.
- `boot` `campaigns::tests::every_lookup_caller_finds_a_halo1_level`: each
  call in the table above, with a Halo 1 map id and the campaign it passes,
  finds nothing without the rewrite and Halo 1's record or next level with
  it; `the_picker_lists_halo_1_then_halo_2_then_the_rest`.
- `boot` `campaign_menu::tests` (2): a press picks the campaign and asks
  for `0x19c6d0([0x4ee4e8], id)` then the level select; the hooks are on
  the plan where the table says.
- `boot` `campaigns::tests::real_disc_lists_take_both_campaigns`
  (ignored): `mainmenu.map`'s real lists, laid into a fake guest at their
  own addresses, take Halo 1 and the test campaign (5 into free slots, 8
  copied); every level resolves through the transcribed readers, the
  fixture's levels show their Halo 2 level's picture, and Halo 2's own
  lookups, list and next levels read exactly as before. Passes on the
  user's disc. `H2_DISC=disc/halo2.iso cargo test -p boot --lib
  campaigns::tests::real_disc -- --ignored --nocapture`

## To check in game

Only a run can show the screen flow, the pictures, and that the engine
starts and chains levels from records it was never shipped with.

1. Install the test campaign: copy `FIXTURE` from
   `crates/content/src/campaign_list.rs` to
   `<data>/campaigns/test/campaign.toml` (the data root `h2 play` uses,
   `~/Library/Application Support/h2recomp/data` by default).
2. `h2 play`. The console says `campaigns: test is campaign 2 with 3
   levels (map ids 0x1001..=0x1003)`.
3. Sign in with a profile, choose **Campaign** in the main menu and go
   to where the level list would open. Expected: `campaigns: opening the
   campaign picker before the level select`, and a "Campaign" screen with
   **Halo 2** and **Second Campaign (test)**.
4. Pick **Halo 2**: the retail level list, unchanged. Back out, pick
   **Second Campaign (test)**: `campaigns: level lists extended ...`, then
   a level list with **Outskirts (2)** only (nothing finished yet), with
   Outskirts' picture.
5. Choose it and a difficulty. Expected: `campaigns: starting a level of
   campaign 2`, and Outskirts loads.
6. Finish it (or use the level's own end), and expect `campaigns: <you>
   finished test level 0x1001 on difficulty N`, the next level
   (Delta Halo) to follow, and afterwards both levels listed under the test
   campaign while Halo 2's list is as before. `progress.toml` has the
   entry.
7. Remove the folder and start again: no `campaigns:` lines, the main
   menu as retail.

## Open questions

- Whether the cache-slot copier and the header checks accept a converted
  map served this way: the same path as a Halo 2 level, but the first
  converted map will tell.
- **The lobby's level select** (`0x2b136d`, co-op) reads the same data
  array: after picking Halo 1 in the main menu, it lists Halo 1 until
  Halo 2 is picked again or the main menu reloads. The lobby gets no
  picker yet.
- **Continue** (`0x199a92`) resumes Halo 2 only: our campaigns' last level
  is kept but nothing reads it yet.
- Where a level ends the campaign: Halo 2's last level is followed by the
  credits through scripts; a campaign's last level (no next id) returns to
  the menu as `0x163610` returns 0. Unchecked in a run.
- Whether the free slots' own pictures are blank placeholders or images.
  Halo 1's levels no longer use them ("Pictures").
- Whether the engine accepts mainmenu with its index tail cleared and one
  page more tag memory than retail's. The
  converted maps already do (sounds.md, "Open questions"). Only a run
  can show the pictures.
- The campaign record's name: campaign 1 has none on the user's disc; we
  fill ours, and nothing is known to read them.
