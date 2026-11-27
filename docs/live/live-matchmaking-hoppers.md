# Live matchmaking: the hopper file

Part of [Matchmaking and custom games over Live](live-matchmaking.md). Covers: 6. The hopper file, field by field.

## 6. The hopper file, field by field

`multiplayer_hopper.dat` is now decoded whole, from the retail game's own
readers and its compiled-in defaults (read statically from the 5849 XBE;
nothing was run). The code is `crates/live/live-proto/src/hopper/`:
`layout.rs` (every field, the validator port, the accessor ports),
`variant.rs` (the game variant an entry carries) and `defaults.rs` (the
lineup a server offers with no file). The envelope is unchanged
([live-motd.md](live-motd.md) §5.1).

Evidence: **confirmed** means a reader
was seen using the field that way; **likely** means a reader was seen but
the meaning is inferred from how it is used; **unused** means only the
game's defaults write it and no reader was found, so we write 0.

### 6.1 Image

```
+0x000000  16 x 0x614     descriptors (copied to 0x00551ae8 by 0x00193ff0)
+0x006140  16 x 0x15cb8   data blocks (read one at a time by 0x001943d0)
```

A descriptor that fails the validator (`0x00193560` and the per-type
check behind `0x001934f0`) is no hopper to the game: every reader goes
through `0x001934f0` first (`0x00192db0`, `0x00192df0`, `0x00192e60`,
`0x001943d0`, `0x001945c0`, `0x00194610`). An all-zero slot is therefore
simply absent.

### 6.2 Descriptor (0x614 bytes)

| Offset | Type | Field | Readers | Evidence |
|---|---|---|---|---|
| `+0x000` | i32 | hopper type 1..5 (§6.3) | validator; `0x00193470`/`90`/`b0`; every per-type accessor | confirmed |
| `+0x004` | i32 | status: 0 not checked yet, 1 ok, 2 error, 3 no hopper; validator 0..3 | `0x00192df0` (0 → `0x00194660` checks the entries' maps and writes 1 or 2), `0x00065530` wants 1 | confirmed |
| `+0x008` | i32 | playlist id, validator 1..=100; unique per playlist (the game's defaults use 1, 2, 3, 4, 25, 26, 5) | `0x000741f0` reads it with `+0x20` and `+0x40` beside it as stats ids | range confirmed, meaning likely |
| `+0x00c` | 9 x 16 B | title per language, UTF-8, NUL-terminated | `0x001944c0`; written by `0x001936e0` | confirmed |
| `+0x09c` | 9 x 128 B | description per language (the ninth ends at `+0x51b`) | `0x00194540` | confirmed |
| `+0x51c` | 128 x i8 | skill ceiling: entry `i` is the highest skill matched with skill `i`; validator `i <= entry <= 0x7f` | `0x001931a0` walks it down for the lowest skill matched (used by `0x00073530` for the session's skill range) | range confirmed, meaning likely |
| `+0x59c` | u32 | search timeout, ms: a team arrangement older than this is dropped | `0x00071c7a` (elapsed + it vs now) | likely |
| `+0x5a0` | u32 | base wait, ms: the defaults' input to the start-wait table | none (`0x001931d0` indexes from `+0x5a4`) | unused |
| `+0x5a4` | 16 x u32 | start wait for 1..16 players, ms; `0x7fffffff` never | `0x001931d0` (answers never outside the type's player range, 0 at the most players, else the table); `0x000719f5` compares it with time searched and shows seconds left (/1000) | confirmed |
| `+0x5e4` | i32 | minimum rank: a party member below it cannot search | `0x00066150` (per-player signed byte, error 0xf); `0x002b2090` (a profile dword at `0x0054eae4 + 0xc70 n`) | reader confirmed, meaning likely |
| `+0x5e8` | u8 | list the hopper whatever the rank | `0x002b2070` | likely |
| `+0x5e9` | 3 B | | none | unused |
| `+0x5ec` | 10 x u32 | type parameters (§6.4) | validator, accessors `0x001931d0`..`0x001934d0`, `0x0006620d` | confirmed |

The older notes' "ASCII name at `+0x51b`" and "record count at `+0x08`"
were wrong: there is no name (the bytes are the last description byte and
the skill table), and the record count is the data block's own.

### 6.3 Hopper types

| Type | Meaning | Party limit (`0x00193250`) | Smallest party (`0x0006620d`) |
|---|---|---|---|
| 1 | free for all, unranked | most players | 1 |
| 2 | free for all, ranked | 1 (alone) | 1 |
| 3 | teams, unranked | teams x most per team | 1 |
| 4 | teams, ranked | `[5]` | `[4]` |
| 5 | clan match: a party is a team | most per team | fewest per team |

Ranked is {2, 4, 5} (`0x00193470`), unranked {1, 3} (`0x00193490`),
teams {3, 4, 5} (`0x001934b0`). The playlist screen's pass at
`0x00065530` (the "types {2, 4, 5}" filter of §1.5) walks the ranked
hoppers to find a player's best skill. A party outside the limits gets
error 6 (too small) or 7 (too big) from `0x00066050`.

### 6.4 Type parameters (`+0x5ec`, ten dwords)

| Type | `[0]` | `[1]` | `[2]` | `[3]` | `[4]`/`[5]` | `[6]` | `[7]`/`[8]` | `[9]` |
|---|---|---|---|---|---|---|---|---|
| 1, 2 | fewest players | most players | | | | | | |
| 3 | teams | fewest a team | most a team | u8: uneven teams allowed | | | | |
| 4 | teams | fewest a team | most a team | largest size difference | party size | u8: keep parties together | party sizes kept as one team | unused |
| 5 | teams | fewest a team | most a team | largest size difference | | | | |

Validator rules (`0x00193610`, `0x00193630`, `0x001936a0`), signed:

- 1, 2: `1 <= [0] <= [1] <= 16`.
- 3, 5: `2 <= [0] <= 16`, `1 <= [1] <= [2] <= 16 / [0]`.
- 4: `2 <= [0] <= 16`, `2 <= [1] <= [2] <= 16 / [0]`,
  `1 <= [4] <= [5] <= [2]`, `0 <= [3] <= [2] - [1]`.

Readers: fewest/most players `0x001932c0`/`0x00193300` (teams: `[0]`
times the team size), team size `0x00193340`/`0x00193370`, teams
`0x00193440`, size difference `0x00193400` (type 3 reads `[3]` as a byte
and answers 0 or 1), party-as-team `0x001934d0` with
`0x001933d0`/`0x001933a0` (type 5 always, with the team size; type 4 when
`[6]`, with `[7]`/`[8]`). These feed team assembly (`0x00071730` →
`0x00091fc0`). The game's defaults write `[9] = 2` on one hopper; nothing
reads it.

### 6.5 Data block (0x15cb8 bytes) and entry (0x37c bytes)

| Offset | Type | Field | Readers | Evidence |
|---|---|---|---|---|
| block `+0x00` | 0x44 B | | none | unused |
| block `+0x44` | i32 | entry count; 0 leaves nothing to pick, the game has no upper check but the block holds 100 | `0x00194660`, `0x00192eb0` | confirmed |
| block `+0x48` | 100 x 0x37c | entries | | |
| entry `+0x000` | u32 | | none | unused |
| entry `+0x004` | u32 | map id: a level in the globals tag's multiplayer list (`0x0019c5f0`); an unknown one marks the whole hopper status 2 | `0x00194660`, `0x00192eb0` | confirmed |
| entry `+0x008` | 9 x 32 u16 | map name per language (the defaults copy a compiled-in English name into all nine) | none | unused |
| entry `+0x248` | i32 | weight: session setup picks an entry at random by weight, each clamped to 1..=1000 (0 when the map is unknown) | `0x00192f12` | confirmed |
| entry `+0x24c` | 0x130 B | game variant, the saved-variant record ([variants.md](../editor/variants.md)); copied out whole with the map id for the entry picked | `0x00192fc0` | confirmed |

Quickmatch's start (`0x00231066`), the playlist screen's press
(`0x002b2181`) and the custom-game list
(`0x002b20a4`, `0x002b2198`) read a block only to see that it loads.

The game's default entries build each variant from a stock record per
game type (`0x0019d220`) plus a few changes per built-in matchmaking
variant (`0x00212f40`, 28 of them), then clear settings 12..14
(`0x00193010`). `variant.rs` ports the stock record and the clean-up
limits (`0x0019d650`) a variant is held to.

Our default entries write the same records as scripts. Each of the ten
built-in matchmaking variants is a `uses` script of its type
(`rules::builtin::MATCHMAKING`): the settings block holds the changes and
the three settings the defaults writer clears (`setting_twelve`,
`setting_thirteen`, `join_in_progress`). `defaults::variants` builds the
record of each with `variants::portable::record_for`, so every default
entry is mode 2 and names its script by hash. Live and the game both find
the script by that prefix among their compiled-in scripts.

A hopper takes
scripted variants only ([live-admin.md](live-admin.md), "`PUT /v1/titles/{title}/hopper`").

With a profile signed in to local Live at the Xbox Live menu, run
`python3 scripts/repro-mm-hopper.py` against the default lineup. It chooses
Clanker Doubles, checks descriptor 10 and phantom membership in
`live.matchmaking.trace`, and waits for gameplay. Pass `--socket PATH` to
select the running game.

### 6.6 The game's own defaults

With no file, `0x001940d0` builds seven hoppers (`0x00193c70`): free for
all (type 2, 4..8 players), small team (type 4, 2 x 3..4, parties 1..4
kept together at 3..4), head to head (type 2, 2..2), big team (type 4,
2 x 5..8, parties 1..5), two clan matches (type 5, 2 x 3..4 and 2 x 6..8)
and small team unranked (type 3, 2 x 2..4, uneven teams). Waits are 30 s
base with a 150 s timeout (120 s for head to head and the clan matches),
45 s and 300 s for big team. Titles come from the
`matchmaking_hopper_descriptions` strings (`ffa_title` ...
`small_team_unranked_description`, string ids `0x7a4`..`0x7b1`).

### 6.7 What a session advertises as its hopper

The **descriptor index** (0..15). The matchmaking state keeps the
selected hopper as an index (`[state + 0x49c8]`), passes it straight to
`0x00192e60` (which indexes the 16 descriptors), and writes it into the
session description at `+0x70` (`0x00073530`). The playlist id (`+0x08`)
is separate and goes to stats; an entry's "record id" is a map id.

So
`hopper_id` in sessions and pins (`PUT /v1/hoppers/{hopper_id}/pins`,
[maps.md](../game/maps.md)) is the index, which is what the web builder's pins use
(`HopperPins` with `h().index`).

### 6.8 The default set

`HopperFile::default_set()` builds a complete file from nothing: sixteen
playlists in hoppers 0..15, playlist ids 1..16 (eight human playlists, then
eight clanker ones), status 0, the game's skill ceiling and wait/timeout
values, the game's start-wait table, maps from the Halo 2 disc and
Multiplayer Map Pack, and variants that are the records of scripts (6.5,
6.6). Clanker Big Team Battle combines Slayer and the big-map objective
variants in one playlist rather than having a separate Big Team Objective.

| # | Title | Kind | Type | Size | Entries |
|---|---|---|---|---|---|
| 0 | Rumble Pit | Human, ranked | 2 | 4..8 players, alone | Rumble Slayer, Oddball, King of the Hill |
| 1 | Team Slayer | Human, ranked | 4 | 2 x 3..4, parties 1..4 (3..4 kept as a team) | Team Slayer |
| 2 | Team Skirmish | Human, ranked | 4 | 2 x 2, parties 1..2 (2 kept as a team) | Team Slayer, Teamball |
| 3 | Team Snipers | Human, ranked | 4 | 2 x 3..4, parties 1..4 | Team Snipers |
| 4 | Big Team Battle | Human, ranked | 4 | 2 x 5..8, parties 1..5 | Team Slayer, Classic CTF, Assault, Territories |
| 5 | Team Objective | Human, ranked | 4 | 2 x 3..4, parties 1..4 (3..4 kept as a team) | Classic CTF, Assault, Territories, Team King, Teamball |
| 6 | Free for All | Human, unranked | 1 | 2..8, parties welcome | Rumble Slayer, Oddball, King of the Hill |
| 7 | Unranked Teams | Human, unranked | 3 | 2 x 2..4, uneven allowed | Team Slayer, Classic CTF, Team King, Teamball |
| 8 | Clanker Rumble | Clanker, ranked | 1 | 2..8, the rest bots | Rumble Slayer, Oddball, King of the Hill |
| 9 | Clanker Slayer | Clanker, ranked | 3 | 2 x 1..4, uneven allowed, the rest bots | Team Slayer |
| 10 | Clanker Doubles | Clanker, ranked | 3 | 2 x 1..2, uneven allowed, the rest bots | Team Slayer, Teamball |
| 11 | Clanker Snipers | Clanker, ranked | 3 | 2 x 1..4, uneven allowed, the rest bots | Team Snipers |
| 12 | Clanker BTB | Clanker, ranked | 3 | 2 x 1..8, uneven allowed, the rest bots | Team Slayer, Classic CTF, Assault, Territories |
| 13 | BTB Heavies | Clanker, ranked | 3 | 2 x 1..8, uneven allowed, the rest bots | Team Heavies |
| 14 | Solo Training | Clanker, unranked | 1 | 2..8, the rest bots | Rumble Slayer, Oddball, King of the Hill |
| 15 | Team Training | Clanker, unranked | 3 | 2 x 1..4, uneven allowed, the rest bots | Team Slayer, Classic CTF, Team King, Teamball |

Hoppers 8..15 are **clanker hoppers** (Clankermatch, docs/game/bots.md
§8): one human or a party, and bots the host runs in every other seat. Titles shared with human modes carry a Clanker prefix in the combined picker and
fit the descriptor's 16-byte title slot (15 bytes and a NUL). Their
descriptions say **Ranked Clanker** or **Unranked Clanker**.

Every clanker
descriptor is an unranked type because the ranked types' rules are for
filling a game with people (ranked teams need two a team and bound parties;
ranked free for all is solo); a lone human with bots needs a team of one
and uneven teams while the bots join.

Ranking is a separate server-side mark, not the descriptor type:
`Playlist::clankers` holds `Clankers { ranked, ranks_as }` (bots.md §9.4
item 1). Hoppers 8..13 have `ranked: true` and each keeps a standing of its
own (`ranks_as` unset), separate from the human playlists. Training hoppers
14 and 15 have `ranked: false` and never change a standing.

`GET /v1/hoppers/clankers` (read like `/v1/hoppers/pins`) serves the marks
as `ClankerHoppers` (`crates/live/live-proto/src/hopper/clankers.rs`):
`{"hoppers": [{"hopper_id", "playlist_id", "players", "teams", "ranked",
"ranks_as"?}]}`, by descriptor index, for the hopper file served now. A
published file keeps a mark only where it has the default's playlist id at
that index; with no file, or one that does not parse, the list is empty.
live-hle reads it for the Clankermatch row's filter, the search and the
lobby fill. A game recorded with `kind: "clankermatch"` (and the hopper's
`playlist_id`) is its own kind in the read model and the stats totals
(`clankermatch`, apart from `matchmaking`) and counts in its playlist and
every career number.

When its mark is ranked it also ranks like a ranked
matchmaking game (§8.4, the same arbitration round) in its clanker hopper's
playlist, or `ranks_as`'s; the game's stats read (§8.3) answers a ranked
clanker hopper's playlist id with that standing and an unranked one's with
no rows.

Tests check it against the validator port, the per-entry checks
(`HopperFile::problems`: base-disc and Map Pack map ids, weights, the
variant clean-up), the base-disc compatibility rotation, a byte-exact
build → parse → build, and that every field reads back from its offset. The
server serves it while no file is published (§6.9);
`h2 live hopper defaults --out FILE [--raw]` writes it.

### 6.9 Serving

`Live::file` answers Halo 2's `multiplayer_hopper.dat` with the default
set while none is published (`crates/live/live-server/src/default_hopper.rs`),
so the game's title-storage download, `GET /v1/titles/{title}/hopper` and
the builder's `PUT` (which then publishes the edited default) all work on
a fresh server. Nothing is written to the log until someone publishes;
`h2 live hopper import` still replaces it with the owner's own file.

### 6.10 Building playlists, and custom maps in them

The 16 slots are fixed; a slot of type 0 is no hopper. The default set fills
every slot, so a new default playlist first clears the slot it replaces. The
web builder (`web/src/admin/HopperBuilder.tsx`) and the editor's playlist
tool (`hopper_edit::model`) work the same way:

- **A new playlist** is made in an unused slot from a template
  (`defaults::templates`, the eight human playlists of §6.8): its whole
  descriptor (type, sizes, waits, timeout, skill ceiling, a free playlist
  id, `HopperFile::create_from`) and one entry, the template's first. The
  raw fields cannot build a descriptor the validator takes (the skill
  ceiling and most type parameters are not in them), which is why a title
  typed into an unused slot alone fails with "type must be 1 to 5" and
  "1 to 100 entries, got 0". "Change kind" applies another template to a
  playlist, keeping its text and entries. A playlist can be cleared back to
  an unused slot (`HopperFile::clear_hopper`).
- **Only changed playlists are checked**, in both editors and on the
  server, so untouched unused slots never block a publish; each problem is
  shown at its playlist with a revert or discard action.
- **An entry's game variant** is picked per entry: a built-in one
  (`defaults::builtin_variants`, keyed; the server writes its 0x130 bytes)
  or one already in the file that runs a rules script (its slot is copied
  with `from`/`from_hopper`). A variant with no script is not offered. The map is a Halo 2 disc map or a Multiplayer Map Pack
  map from the catalogue. The builder labels Map Pack maps and requires at
  least one disc map in the rotation.
- **Custom maps** are allowed per playlist by pins (`HopperPinsSet`,
  [maps.md §5](../game/maps.md)); the builder's "Custom maps allowed"
  section says, for each pinned map, whether the playlist has an entry on
  its base map and offers to add one.

What a playlist cannot do yet: **make a game play a custom map.** An
entry is a map id and a variant; the format has no room for a `MapId`, and
session setup (`0x00192e60`/`0x00192eb0`) picks an entry and loads
`maps\<base>.map`. A pin permits a custom content ID in the session
advertisement. Custom games pick the package's separate slot under Mods.

For a
playlist entry to choose a custom map, three pieces are missing:

1. **Data:** a per-entry binding kept by the server beside the file (for
   example pins extended to `(hopper_id, entry index) -> MapId`, signed
   and replayed like `HopperPinsSet`), served with the pins.
2. **Runtime:** a hook at the host's entry pick (after `0x00192eb0`
   chooses the entry, before the map load) that reads the binding, makes
   the package ready through `MapGate` (download, base check) and selects
   its content offer for that game.
3. **Fallback:** what a host that cannot get the package does (play the
   base, or skip to another entry), and that every peer agrees before the
   load, since lockstep diverges on any byte.
