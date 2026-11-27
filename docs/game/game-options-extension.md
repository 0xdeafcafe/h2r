# Game options: the extension

The option extension. Part of [game-options.md](game-options.md).

## 3. The extension

### Where the extension lives

The choice was an extension block the game ignores, or a side table keyed by
the variant. **The block goes in the record's spare bytes** at
`+0x10c..+0x12f`, which every copy of the record already carries: saved
variants (signed by the game), cloud saves, `.h2variant` files, hopper
slots and the editor. A side table would need its own storage, sync and
cleanup beside each of those, and could fall out of step with the record.

Layout (0x24 bytes; all zero means "no extension", which is every retail
variant):

| Offset | Size | Content |
|---|---|---|
| `+0x10c` | 2 | magic `hx` |
| `+0x10e` | 1 | version (1) |
| `+0x10f` | 1 | flags: bit 0 reserved, bit 1 unlimited grenades, bit 2 reserved (reads as off; the superbounce mode comes from a rules script, [superbounce.md](superbounce.md#the-superbounce-game-type)) |
| `+0x110` | 2 | gravity, percent (0 means 100; 10..400) |
| `+0x112` | 2 | damage dealt, percent (0 means 100; 10..400, as the next three) |
| `+0x114` | 2 | shield strength, percent |
| `+0x116` | 2 | health, percent |
| `+0x118` | 2 | movement speed, percent |
| `+0x11a` | 2 | primary weapon, our id (0 means use `+0xd6`) |
| `+0x11c` | 2 | secondary weapon, our id |
| `+0x11e` | 1 | team size (0 means any; at most 16) |
| `+0x11f` | 1 | script mode: 0 none, 1 Firefight, 2 rules script (other values are kept and never applied) |
| `+0x120` | 8 | first 8 bytes of the script definition's SHA-256 |
| `+0x128` | 4 | packed script option values |
| `+0x12c` | 4 | CRC-32 of `+0x10c..+0x12b` |

Our weapon ids are a table in our code naming tags by path (the weapons the
multiplayer maps carry), never a datum, which differs per map. A block with
a bad magic, version or CRC, or a value out of its range, is ignored as a
whole (`variants::ext::Block::Ignored`): the game plays retail rules and
Studio lists it as an issue. The CRC is zlib's CRC-32 over the 0x20 bytes
before it, little-endian. The default options with no script mode encode as all zero, so a
variant whose extras are all put back is a retail record again. A block
with a script mode is never default. `variants::ext::Ext` carries the
script reference (`Script`: mode, prefix, options), so every edit that
decodes and re-encodes the block keeps it
([rules-engine.md](rules-engine.md#carrying-a-script)). Because retail never writes these
bytes, a record from a retail editor is all zero there.

An older build ignores the block and keeps it: it loads, plays and re-saves
the variant with only its retail fields in effect.

### How it reaches every player

The wire drops the spare bytes, so the host sends the block another way,
through Live, as skulls already do (skulls.md "How the host's set reaches
guests"):

- **Custom games and hoppers** (`boot::variant_ext::Transport`, feature `variant-ext-live`,
  on `GAME_ACTIVATE`, the options in EAX, before the game copies them into
  the globals):
  - A **Live host** puts its record's block and the variant's key on its
    session at every multiplayer start, extras or not
    (`NewSession.variant_ext` / `Update`, `{block, key}` in hex;
    `Hle::set_session_variant_ext`). Open advertisements are updated at
    once. Its log says `variant options: sent to guests: ...`.
  - A **Live guest** asks the server for its host's session
    (`Hle::host_variant_ext`) until the key there is its own record's, for
    at most 1.5 s (the host may publish a moment later, and its session may
    still name the last game's variant). It then writes the block into the
    options at `O+0x134+0x10c`, which the game copies to `G+0x13c+0x10c`,
    and applies it (`variant_ext::decide`, "guest, the host's"). Its log
    says `variant options: the host's for this game: ...`, or why none.
  - **The key** is FNV-1a over the record's name (`+4`, 0x40 bytes) and
    engine (`+0x44`), which the wire carries whole. It is not the whole wire
    projection plus the map: which fields the codec sends isn't pinned in
    our code, and a field one side holds differently would make every key
    differ. Two variants with the same name and type
    share a key; the cost is a guest taking the other's block.
  - The server keeps the field as sent, checks its form only (72 and 16
    lowercase hex digits), and lists it; both homes run the same routes.
- **Hoppers:** the extension is already in the hopper slot's record, so the
  host has it the moment it picks the entry; the rest is the custom-game
  path. The server's hopper model and the editor's hopper tool carry it
  untouched (both copy the record whole).
- **Older builds** can join when they advertise the same
  [replication version](ai-networking.md#creature-entity). Variant
  support is a separate capability.
  - A new guest of an older host finds no `variant_ext` on the session and
    logs `variant options: none from the host: the host's build sends no
    variant options (an older build)`. An older host never applies a block,
    so both play the game's own rules and agree.
  - **An older guest of a new host**: the host names it in its
    log. Each console's presence carries its build (the git short hash,
    7 to 12 lowercase hex digits) and its capabilities (`name/version`,
    at most 16 of 32 characters each; this build sends `variant-ext/1`)
    on every `SignedIn` and `PresenceChanged` (`live_proto::PresenceData`;
    the HLE's `ServerOptions.build` and `capabilities`, which boot fills
    from the binary's own build metadata, `app::BuildInfo`, and
    `live_glue::CAPABILITIES`). The server checks their shape and returns
    them on `GET /v1/presence` and in friends lists. **Privacy:** they
    show to every reader, anonymous included, as `state` does: they name
    the console's software, not where its player is, and a host must read
    its guests' whether or not they are friends (the game id and activity
    keep their own rule). At a multiplayer start with options, a Live host
    reads its guests' XUIDs from the squad session's players
    (`variant_ext::session_guests`: mask `S+0x111c`, 16 players at
    `S+0x1120`, `0x13c` each, XUID at `+0`, peer at `+0xc`; docs/game/
    bots.md) and asks the server (`Hle::players_lacking`, at most 0.5 s;
    `live_proto::lacking`). Each guest whose presence lacks
    `variant-ext/1` gets a line: `variant options: <gamertag> plays this
    variant with the game's own rules (older build)`. A player with no
    presence (a bot) is not named. The check takes XUIDs, so the Live
    session's roster can feed it once the HLE writes `PlayerJoined`.
    Not built: the lobby text (log lines only), and telling the older
    player (its build can't).

  **The desync risk.** Halo 2's host simulates and its guests predict. A
  guest playing retail rules against a host with extras predicts wrong
  wherever the rules differ: with gravity it predicts its own jumps and
  every falling object at the wrong height, and the host corrects it
  (rubber-banding, grenades landing short or long on its screen); with
  speed, its own movement snaps back; with damage, shield or health scales,
  it shows hits and kills the host does not agree with until the host's
  update arrives. Unlimited grenades and other weapons are host-side and
  replicate, so they look right. Nothing crashes and the host's result
  stands, but the older player's game feels broken, which is what the
  warning says.
- **System Link**: extras are off. No machine on System Link
  applies a block (`variant_ext::decide` checks the session's network
  class at `+0x18`), so nobody disagrees. The warning is a log line at the
  start (`variant options: ... off: System Link plays the game's own
  rules`); the lobby itself doesn't say it (no lobby text hook of ours
  there).

### Showing them

- **In the game's editor.** Storable values join the retail rows: at load
  we copy each `sily` options block into our page, append our options
  (value and a label id in our private string range, served by the
  `community_variants` string hook) and repoint it, the way
  `community_variants` extends the `goof` block. Respawn gets 1 s and 2 s,
  primary weapon "None", score, time, lives and penalties more steps. The
  game's own editor then shows and stores them; nothing new to draw.
  (`boot::variant_options`.)
- **Extension options** (gravity, multipliers, grenades, other weapons,
  team size) have no setting number the setter can store (its descriptor
  table at `0x470828` has 0x70 entries). They go on a kit page of ours,
  "More Options", opened from a row added to the variant editor's page list
  with the list hooks the pregame dialog's "Halo 1 Options" row uses
  ([multiplayer.md](../halo1/multiplayer.md) "Halo 1 options",
  [ui-screens.md](ui-screens.md)). A opens the shared value picker for
  gravity, the team count, the damage, shield, health and speed scales and
  the two other-weapon rows (a picker of "Not Used" and the table's
  weapons). Unlimited Grenades is a toggle. The page reads and writes the record
  being edited (`boot::variant_page` under "The variant editor's page list").
- **Studio** (`crates/apps/studio/src/ui/variants.rs`): a "More options
  (h2recomp)" section under the pages edits the block through
  `VariantEditor::set_row` (undoable; a drag on one row is one step):
  each extra is the settings-table row a script's `settings` block names
  ([rules-variants.md](rules-variants.md#the-settings-block)), and the
  record names its script (mode 2). It shows only what the game applies (gravity, damage, shield, health and
  speed, unlimited grenades, team count) and the other starting and
  secondary weapon (two choices, `VariantEditor::set_other_weapon`,
  `Action::SetVariantWeapon`; the table has no row for them); a block the
  game would ignore shows a warning and an `Issue::Ext`. `issues()` also
  has our own rule (respawn 0 with cycling).
- **Unlimited grenades**: flag bit 1. After each tick a
  lone machine or a Live host (`Decision::unlimited_grenades`; never a
  guest, never System Link) tops up every player's unit, remote ones too,
  to at least four of each grenade type (`skulls_custom::variant_grenades`,
  the Unlimited Grenades skull's `top_grenades` on the unit's two counts at
  `+0x23e`). The host's counts are the game's and replicate, so a guest
  writes nothing. Row 1 of "More Options" ("Unlimited Grenades: Off",
  left, right and A toggle) and a Studio checkbox.
- **Damage, shield and health** (traced statically).
  `0xd7b80` applies one damage instance; it calls the shield step
  `0xd9110` (stdcall: object datum, ?, damage data, record; `ret 0x10`) at
  `0xd80de` and the body step `0xd8cb0` (record in EDI; stdcall: object
  datum, ?, damage data; `ret 0xc`) at `0xd8184`, each its only caller.
  Both read the amount left to apply at `record+0x1c`: the shield step
  takes `amount × material scale / shield vitality` (`0xd5d80` gives the
  vitality) off the shield and leaves the rest at `+0x1c`; the body step
  reads it through `0xd9020`. Our feature `variant-vitality`
  (`variant_ext::Vitality`, points `SHIELD_DAMAGE` and `BODY_DAMAGE`), for
  an object that is a player's unit, multiplies the amount by damage ÷
  shield before the shield step, puts what the shield left back to the
  game's units after it, and multiplies by damage ÷ health before the
  body step. So a hit takes damage/shield of the game's shield share and
  damage/health of the body's, whether or not the shield took part. It
  applies wherever `Decision::vitality` says: alone, the host and a guest
  with its host's block (a guest predicts the hits too), never System
  Link. Vehicles, AI and objects keep the game's rules. Rows 3 to 5 of
  "More Options" (gravity's steps, 10% to 400%) and Studio sliders. What
  else reads `+0x1c` after the steps (`0xd82e0`, called between them, and
  the damage report) sees the scaled amount; not traced further.
- **Movement speed** (traced statically). The engine's per-player speed
  step `0x15c000` (`ENGINE_PLAYER_SPEEDS`; called from `ENGINE_UPDATE` at
  `0x15c2eb` and from `0x162060`) asks the engine's vtable slot `+0x4c`
  for a target multiplier (Slayer's, King of the Hill's and Territories'
  `0x24dd50` return 1.0; Oddball's `0x2bc3d0` and CTF's and Assault's
  `0x240e30` return 0.75, 1.0 or 1.25 from the speed setting `G+0x234`
  (record `+0xf8`, setting 96) for their carrier; Juggernaut's own
  `0x1924c0` is not read),
  then ramps player `+0x19c` toward it: up by 0.15 over the tick rate a
  tick, straight down to the target when above it, never below 0. Player
  `+0x19c` is read only there and by the player's state export `0x24de80`
  (record `+0x14`, default 1.0), which is what carries it to the unit's
  movement and to guests. Our feature `variant-speed` (`variant_ext::Speed`)
  divides every live player's `+0x19c` by the variant's factor before the
  step and multiplies it back after, so the game's target, ramp and clamp
  run on unscaled values and everything that reads `+0x19c` later sees the
  scaled one (a carrier's 1.25 at 150% is 1.875). It applies wherever
  `Decision::speed` says: alone, the host and a guest with its host's
  block (a guest predicts its own movement), never System Link. Players
  reach the factor over a few seconds from the first tick, as the engine's
  own ramp does. Row 6 of "More Options", a Studio slider and the
  settings-table row `speed` (`ExtField::Speed`, so a script's `settings`
  block and `.h2variant` files carry it). The movement code itself
  (`0xe3700`, the biped update) is not read: that the unit uses the
  exported multiplier rests on the Juggernaut working through the same
  path in retail; measure a unit's speed on a running game before
  trusting the last digit.
- **Team count**: a retail field (setting 93, `+0xb4`)
  with no retail row. Row 2 of "More Options" ("Team Count: Map
  Default", then 2 to 8; 1 is storable but not offered and shows as 2)
  writes the edited record's `+0xb4` (`0x54e554`) rather than the block;
  Studio has a "Team count" box (`record::TEAM_COUNT`, `TEAM_COUNTS`).
  Every build plays it: it is the game's own field. Whether the game's
  wire carries it to guests is not traced (the codec's fields aren't
  pinned); only the host builds the team set, as far as the trace shows.
- **The stats site** records only the engine name (`GameStarted`'s
  `variant`); the extension's summary ("Gravity 50%") is not sent.

### The variant editor's page list (traced, static)

The list already has a row of ours: Skulls, row word 6
(`boot::skulls_mp_row`, docs/game/skulls.md "Choosing them"). "More
Options" is row word 7, added the same way. Confirmed in the code:

- **Screen.** `variant_editing_options`, screen 178: loader `0x23764f`
  (stdcall, the request, `ret 4`; 0x9b4 bytes, pushed by `0x147f6d`),
  constructor `0x23768d` (vtable `0x459ae8`), list handler at screen
  `+0x614`. Reached from Settings, Game Variants (`0x2b1b01`), then
  `0x2387d8` or `0x2ba9d0`, on window 5.
- **List constructor `0x2b74c3`** (handler vtable `0x45bdb8`): six item
  widgets, a datum array named by `0x45be10` with a **fixed** count of 6
  (`push 6`), each row's word (`+2`) set to its index by a loop over the
  array's count. The rows don't come from the `goof` pages, so
  `skulls_mp::variant_count` grows it by name; for two rows it must add 2.
- **Row text `0x2b7581`** (thiscall, `[esp+4]` the item, `ret 8`): a
  table of six (word, string id) pairs through `0x24c75c`; an unknown word
  leaves the text alone, so a `before` hook draws ours.
- **Row chosen `0x2b75e8`** (thiscall, `[esp+8]` the row's handle,
  `ret 8`): words above 5 return at `0x2b7711`. Words 0..5 open page
  `E - 1`, 9, `0x1e + E`, `0xc + E`, 0xb, 0xc (E the engine of the record
  being edited) through `0x2c83e6(page, window 5, slot 4, 1 << player)`;
  the page screen (`0x2c7e5b`, rows filled by `0x2c7f7f` from the `goof`
  page via `0x2369b3`) is retail's and has no room for our options.
- **Panes:** the tag has six; `skulls_mp::skip_pane` covers row 6 only and
  must cover 7 too (`0x230427` doesn't check the index).
- **Opening ours:** `skulls_mp::open_request(mask, [screen+0x74],
  [screen+0x78], loader)`, as the Skulls row does, onto the editor's own
  stack.
- **The record the page edits is at `0x54e4a0`** (confirmed):
  `0x148a8d` zeroes 0x4c dwords (0x130 bytes, one record) from
  `0x54e4a0`; `0x2387c6` copies 32 units of name to `0x54e4a4` (`+4`);
  the row chosen reads the engine at `0x54e4e4` (`+0x44`). The block is at
  `0x54e5ac`. `variant_page::tests::the_edited_record_starts_at_0x54e4a0`
  pins it against the record layout and
  `edited_record_is_where_the_xbe_says` (ignored; `H2_XBE`) against the
  instructions.

**Our row** (`boot::variant_page`): `variant_count` grows the list by the
Skulls row and ours (8 rows with both), `more_row` is word 7 (6 without
Skulls), `skip_pane` keeps the last pane for it. The row's `before` hooks
on `0x2b7581` and `0x2b75e8` draw "More Options" and open our page with
`skulls_mp::open_request` on the editor's own window and slot. The page
(`variant-more-options`, laid out as the skulls page) reads the block at
`0x54e5ac` when it opens and at each row text. A opens the shared picker
with the current choice focused. Confirming a choice stages the edit.
Left and right remain shortcuts, and A on Unlimited Grenades toggles it
directly. `Rows::live_text` writes pending changes before the row redraws,
so the editor saves them with the record. Gravity (row 0) offers 10, 25,
50, 75, Normal, 125, 150, 200, 300 and 400%. A value Studio sets between
steps shows as the nearest. Selecting Normal writes all zeros when the
other options are at their defaults. Rows 1 to 6 are Unlimited Grenades,
Team Count and the damage, shield, health and speed scales
(`variant_page::GRENADES_ROW`, `TEAMS_ROW`, `Scale`). The "variant page"
string family supplies the labels and descriptions.

**A script's options.** A record whose block names a rules script (mode 2
and the hash prefix) gets one row per option of that script after the seven
fixed rows, in the script's declaration order, at most
`variant_page::MAX_OPTION_ROWS` (32). The page finds the script from the
prefix as the lobby does: a compiled-in script, else the game-variant
store's (`variant_page::script_options`). A record that names no script,
or a script this machine lacks, has the seven rows only.

| Option type | Row | A or Left and Right |
|---|---|---|
| `bool` | "Name: On" or "Off" | the picker, or a step |
| `enum` | "Name: Value" | the picker over the values, or a step |
| `int` of at most 16 values | "Name: 3" | the picker, or a step of one |
| wider `int` | "Name: 150" | no picker; a step of a sixteenth of the range |

The row's label and the focused description are the script's own
(`option` line); their text changes with the record, so each row owns two
ids of the "variant page" family (`option_label_id`,
`option_description_id`), each a placeholder of `OPTION_ROOM` bytes that
`Rows::live_text` rewrites where the text changed. The list is built with
room for 32 option rows (`Rows::growing_capacity`) and grows or shrinks
to the record's script once the page has read the block, since the row
count is asked before guest memory is read.

A step writes only that option's field of the block's 32 option bits
(`OptionRow::set`, the bits `Ir::pack_options` lays out) and leaves
every other byte of the block, the script reference and the other
options as they were; the block is written back to the record the way
the fixed rows' edits are. An option with no bits (one value) has a row
and nothing to change.

### The code

- `variants::ext`: `Ext` (gravity, damage, shield, health and speed
  percents, unlimited grenades, two weapon ids of `variants::ext_weapons`,
  team size), `encode`
  (all zero for the default) and `decode` (`Block::None`, `Ext` or
  `Ignored(Why)`: magic, version, CRC, range), `in_effect`, `summary`,
  `crc32`. `GameVariant::ext_block`, `ext_state`, `ext` and `set_ext`
  (refuses out-of-range values). `Schema::validate` reports a block the
  game would ignore as `Issue::Ext`; it is a warning, not refused.
- The block is in every copy of the record, so saved variants (signed
  over the whole record), `.h2variant` files, Studio's undo and hopper
  slots keep it with no further code. The hopper tool's editable fields
  inside the variant are the raw string (`+0x00`), flags (`+0x48`) and
  second dword (`+0x58`), all before `+0x10c`.
- Studio: `Action::SetVariantExt`, `VariantEditor::set_ext`, the "More
  options (h2recomp)" section.
- `live_proto::session::VariantExt` (`{block, key}`, form checks, `bytes`,
  `new`) on `NewSession` and `Update`; the server keeps and lists it (one
  set of routes for both homes). `live-hle`: `LocalSession.variant_ext`,
  `Sessions::set_variant_ext` (updates open advertisements at once, only on
  a change), `Hle::set_session_variant_ext`, `Hle::host_variant_ext`
  (`HostVariantExt`).
- `boot::variant_ext`: `in_game`, `decide`, `key`, `part` (alone, System
  Link, Live host, Live guest), `heard` and `wait_for_host` (the guest's
  poll), the `Transport` feature on `GAME_ACTIVATE`, `Decision`,
  `Vitality`. `skulls_custom`'s tick applies Low Gravity times the
  variant's gravity through `gravity_step`.
- Movement speed: `variant_ext::{Speed, Decision::speed, scale_player_speeds}`,
  `points::ENGINE_PLAYER_SPEEDS`, `Ext::speed_scale`, `ExtField::Speed`
  (`rules::settings`), `variant_page::Scale::Speed`.
- Unlimited grenades: `variant_ext::Decision::unlimited_grenades`,
  `skulls_custom::{player_units, top_grenades, variant_grenades}`.
  Team count: `variants::record::{TEAM_COUNT, TEAM_COUNTS}`. Damage, shield
  and health: `variant_ext::{Vitality, shield_factor, body_factor,
  scale_amount, is_player_unit}`, `engine::points::{SHIELD_DAMAGE,
  BODY_DAMAGE}`. Weapon slots 15, 16 and 20 in Studio:
  `variants::schema::{weapon_slots, weapon_label, add_weapon_slots,
  UNOFFERED_WEAPONS}`, called from `Schema::from_map`.

### Not built

- **Damage through the engine's trait query** (`ENGINE_TRAIT_QUERY`
  `0x24dda0`, engine vtable slot `+0x8c`,
  [notes](../engine/notes/2026-11-21-engine-vtables.md)): its callers
  through the slot (`0x15b28e`, `0x2404ad`, `0x2bc967`) ask trait 1, active
  camo; the reader of traits 2 and 3 is not found, so the scales hook the
  damage steps instead (`0xd9110`, `0xd8cb0`).
- **Our weapons in the game's own editor** and **slots 15, 16 and 20 in
  its rows**: the editor's lists take our labels from fixed strings
  (`boot::variant_options`), and these need names from the disc. The
  paths in `variants::ext_weapons::TABLE` are retail's naming and have not
  been checked against each map's tags (see "Not verified").
- **Team size**: lobby enforcement is not traced. What is: each console
  builds its own player's requested configuration with the team at `+0x7c`
  from its profile (`0x18fd94`), and `0x14be90` (player index on the
  stack, the configuration in EAX; callers `0x8552b`, `0x14c279`,
  `0x152445`) copies it to player `+0xd4`, checks it through `0x14bc80`
  (which sets -1 for a team outside the game's mask) and applies it. A
  limit would clamp `+0x7c` there, counting the players already on each
  team, but every machine applies configurations in its own order, so a
  clamp on one machine can differ from another's and desync the teams;
  the host's choice would have to reach the guests first. No such path is
  traced, so the block's `+0x11e` is stored, shown and sent and nothing
  applies it.
- **Lobby texts** for older guests and for System Link (log lines only).
