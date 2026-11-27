# Live admin: the admin API

Part of [Live service: admins, the MOTD editor and the playlist builder](live-admin.md). Covers: 2. The admin API.

## 2. The admin API

The editors speak structured JSON; the server builds the binary with
`live-proto` and publishes it as the same title file
`h2 live motd set` / `hopper import` publish. Reads are public like every
read; writes need a `titles.write` token (operator or admin).

### `GET /v1/titles/{title}/motd`

The published `message_of_the_day.dat` as its editable fields, 404 until
one is published (the embedded server publishes a default at start, see
below). Public on a public server: the dashboard shows the header and the
stats site's "Did You Know?" box cycles the messages from this read.

```json
{ "header": "Halo 2 is back", "messages": ["Welcome, Spartan.", "…"] }
```

### `PUT /v1/titles/{title}/motd`

The same shape, admin-only. The server writes the same text into all nine
language slots (as `motd set` does) and answers
`{"published": {name, sha256, size}, "motd": …}`. Limits as the format
requires: 1..=32 messages, at most 255 UTF-16 units each, and a built file
of at most 32,768 bytes (`0x8000`, the MOTD channel's download buffer,
[live-motd.md §2.2](live-motd.md); a larger file would fail the game's
download and show nothing). The server writes the 16-byte header the game
validates (CRC, version 1, size; `live_proto::title_file`). A refused PUT
leaves the published file as it was. The hopper PUT is the same with
version 4 and a 131,072-byte (`0x20000`) cap.

### When the game shows it

The game fetches the MOTD from title storage once per Xbox Live sign-in
([live-motd.md §2](live-motd.md)): sign-in arms the channel, the resource
manager downloads `9809/message_of_the_day.dat`, validates the header and
only then sets the flag the two screens check. Nothing re-arms the channel
mid-session, so **a MOTD published while the game is signed in shows
after the next sign-in** (sign out and back in, or restart the game). The
editor says so after a publish. The Live main menu shows the header once
per screen visit; the matchmaking screen shows a random message,
re-picked every 15 s.

Files published before the header was traced carry 16 zero bytes there,
which the game rejects. On start, the server (`h2 live serve` and the
embedded one) republishes such a MOTD or hopper with a real header and the
same body (`crates/live/live-server/src/title_headers.rs`); reads through the
admin API accept them meanwhile.

### The default MOTD and keeping what you publish

`h2 play --live-local` without Postgres keeps its log in the live data directory
(`server/log`: `events.jsonl`, `blobs/`, `checkpoints.json`;
`crates/live/live-server/src/filestore.rs`), so accounts, friends, games and
published title files survive a restart; the game reads the MOTD only at
sign-in, so a publish needs to outlive the session. Only if that directory
is held by another running server does it fall back to memory (with a
warning).

When no MOTD has ever been published, the embedded server (on Postgres or
the file store) publishes a default one at start
(`crates/live/live-server/src/default_motd.rs`): a short header and 21 original
"Did you know?" lines and gameplay tips. It never replaces a published one;
edit it like any other.

### `GET /v1/titles/{title}/hopper`

The playlists as JSON - 16 hoppers, each with its records (the entries
within the hopper, the first `record_count` of them):

```json
{ "hoppers": [ {
    "index": 0, "type": 4, "unused": false, "status": 1, "record_count": 2,
    "name": "", "title": "Big Team Battle",
    "description": "Join alone or as a party…", "ranges": [1, 8, 6],
    "dwords": [ …18 numbers… ],
    "records": [ { "id": 1, "string": "", "flags": 1, "dwords": [1, 0],
                   "variant": { "name": "Team Slayer", "engine": 2, "team": true },
                   "builtin": "team_slayer" } ]
  }, … ],
  "variants": [ { "key": "team_slayer", "name": "Team Slayer", "engine": 2, "team": true }, … ],
  "templates": [ { "id": 2, "type": 4, "title": "Team Slayer", "description": "…",
                   "min_players": 6, "max_players": 8, "ranges": [2, 3, 4],
                   "dwords": [ …18… ], "entry": { "id": 50, "variant": "team_slayer" } }, … ],
  "maps": [ { "id": 50, "name": "Lockout" }, … ],
  "position": 1042 }
```

- `unused`: the slot is type 0, which the game reads as no hopper. The file
  always has 16 slots; the built-in lineup fills 11 and leaves 12..16
  unused. Its records are empty.
- `variants`: the built-in game variants the server can write into an
  entry by key: the record of one of the default lineup's scripts
  (`rules::builtin::MATCHMAKING`, built by `variants::portable::record_for`
  in `live_proto::hopper::defaults`, never from a disc), so each is mode 2
  and `scripted` is true. `builtin` on a record names the one its variant
  is byte for byte, if any. A record's `variant.scripted` says whether its
  variant runs a rules script; the builder offers scripted variants only.
- `templates`: the default lineup's human playlists, each a kind of
  playlist a new one is made from: its type, sizes, waits and skill table
  (the descriptor the game's validator wants, which the JSON fields alone
  cannot build) and a first entry that suits it. `ranges` and `dwords` are
  that descriptor's, so a builder's copy of a new playlist is whole.
- `maps`: the retail multiplayer maps by entry id, the map list when the
  owner's catalogue (below) is not there.

For Halo 2, a server with nothing published serves the built-in lineup
([live-matchmaking.md §6.8](live-matchmaking.md)) here and from the file
endpoint; other titles 404 until a file is imported. What the fields are
(the full layout is [live-matchmaking.md §6](live-matchmaking.md)):

- `title` / `description`: what the playlist list shows, the English one
  of nine per-language UTF-8 slots (`+0x0c`, 16 bytes each; `+0x9c`, 128
  bytes each). A write puts the text in all nine. At most 15 and 127 bytes.
- `type` 1..5: 1 free for all, 2 ranked free for all, 3 unranked teams,
  4 ranked teams, 5 clan match. `status` 0..3 (0 lets the game check the
  maps itself; 1 ok, 2 error, 3 none). The three `ranges` are the first
  three type parameters (`+0x5ec`): fewest and most players for types 1
  and 2; teams, fewest and most a team for 3..5. The descriptor's dword
  array carries the rest (§6.2 there).
- `record_count` is the data block's entry count (`+0x44`). The
  descriptor's `+0x08` is the playlist's stats id, which the server keeps
  as it is.
- `name`: the raw ASCII field at `+0x51b`. It overlaps a 128-byte table at
  `+0x51c` that the game validates (`0x00193560`: each byte `i..=0x7f`), so
  it normally reads empty and must not be changed; the server writes it
  only when it differs from what is there.
- Per record: `id` is a **map**: a multiplayer level id from the globals
  tag (`matg +0x178`, the lookup `0x0019c5f0`); an id that is not one
  marks the hopper as an error. `dwords[0]` (`+0x248`) is the record's
  **weight**: session setup (`0x00192eb0`) picks a record at random with
  probability weight / total, clamped to 1..1000. The rest of the slot from
  `+0x24c` is the record's **game variant**, 0x130 bytes that session setup
  copies out whole; `variant` decodes its name, game-engine index (1 CTF,
  2 Slayer, 3 Oddball, 4 King, 7 Juggernaut, 8 Territories, 9 Assault) and
  team flag, read-only. `string`, `flags` and `dwords[1]` lie inside the
  variant (`string` over its first 0x48 bytes, name and engine included;
  `flags` is its engine flags; `dwords[1]` its join-in-progress setting);
  the server writes `string` only when it changes, so a round trip keeps
  the variant whole.

Every byte the JSON does not name is preserved; `live_proto::hopper`
(`Descriptor`, `Rules`, `Entry`, `VariantRecord`) reads and writes all of
them.

### `PUT /v1/titles/{title}/hopper`

Admin-only. The body lists only the hoppers that changed; within one, only
the fields given change:

```json
{ "hoppers": [ { "index": 0, "name": "Big Team", "record_count": 3,
    "records": [
      { "id": 2, "string": "two first", "flags": 0, "dwords": [0, 0], "from": 1 },
      { "id": 1, "string": "one",     "flags": 0, "dwords": [0, 0] },
      { "id": 3, "string": "a copy",  "flags": 0, "dwords": [0, 0], "from": 1 },
      { "id": 4, "string": "",        "flags": 1, "dwords": [1, 0], "from": 0, "from_hopper": 5 }
    ] } ] }
```

`type` is the field's name, as the GET has it (`hopper_type` is read
too; before this the server read only `hopper_type`, so a type edit from
the builder was silently dropped).

`records`, when present, replaces the whole list (1..100 entries;
`record_count` must then match or be left out). Each entry keeps its
unknown bytes - the template blobs the game fills in - from the slot named
by `from`: the entry's position in the hopper as it stands now. Absent
means "this position", so an edit in place needs nothing; a reorder names
where the entry came from, a copy names its source, a removal is simply a
shorter list. `from_hopper` makes `from` name a slot of another hopper (as
the file stood before this patch): that is how an entry, game variant and
all, is copied between playlists. `title` and `description` are optional
like every other field. Nothing the builder cannot edit is ever lost, and the
published file re-inflates to the same 0x162cc0 image with the edits
applied. The reply is `{"published": …, "hoppers": …, "position": …}`, the
fresh state.

New playlists, removals and built-in variants:

```json
{ "hoppers": [
    { "index": 11, "template": 2, "title": "Modtacular",
      "records": [ { "id": 50, "string": "", "flags": 0, "dwords": [5, 0], "variant": "team_slayer" },
                   { "id": 20, "string": "", "flags": 0, "dwords": [1, 0], "variant": "classic_ctf" } ] },
    { "index": 10, "clear": true } ] }
```

- `template` (an id from the GET's `templates`) writes that template's
  whole descriptor first, with the lowest playlist id (1..100) no other
  playlist has, then the other fields on top. It is how an unused slot
  becomes a playlist: a patch naming an unused slot without one is refused
  ("an unused slot: create it from a template first"), and so is a new
  playlist with no entries. On a playlist it changes its kind.
- `clear: true` zeroes the hopper's descriptor and entries, making it an
  unused slot; nothing else may be sent with it. Pins on that index stay
  (they are the server's, by index) until changed.
- A record's `variant` (a key from `variants`) builds the entry afresh: the
  map `id`, the weight `dwords[0]`, that variant, every other byte zero, as
  the built-in lineup writes its entries. `string`, `flags` and
  `dwords[1]` lie inside the variant and are ignored; `from` may not be
  given with it.
- A playlist the game's validator took before the patch (or one made from
  a template) must still pass it after (`check_descriptor`: type, sizes,
  skill table, playlist id), so a raw `type` that does not fit the range
  fields is refused with the validator's reasons.
- Only the hoppers named are checked: unused slots elsewhere never block a
  publish.
- Every entry of a named playlist runs a rules script (mode 2): a hopper
  plays scripted variants only, so an entry whose variant is mode 0 (its
  own game type), mode 1 (Firefight) or a mode the server does not know is
  refused by playlist, entry and variant name. The same check runs in the
  map gate ([`blocked_pairs`] lists such an entry as blocked) and, for a
  whole file, on `PUT /v1/titles/{title}/files/multiplayer_hopper.dat`
  (`h2 live hopper import`): the entry checks of this list that concern
  scripts apply, so a disc image of the game's own hoppers (every entry
  mode 0) is not importable; the map pairs are not checked at import, as
  a map's census may be set after the file.
- Each entry names, by its hash prefix, one saved or built-in script (the
  default lineup's scripts included), and the
  playlist's kind takes that script's capabilities (the script policy,
  [rules-engine.md, "Roles, powers and capabilities"](../game/rules-engine.md#roles-powers-and-capabilities)):
  ranked types (2, 4 and 5) refuse every `forge.*` capability, and the
  unranked ones keep no deny list, so they refuse none. A refusal names
  the playlist, the entry and the capability; a prefix no script has is
  refused too.

This is `apply_patch`, `check_scripts` and `check_file` in
`crates/live/live-server/src/admin.rs`; the Worker serves the same
router, so both homes check the same way.

### Importing the base image

```
h2 live hopper import <path> [--title 4D530064] [--server URL] [--data DIR]
```

`path` is the owner's own `n:\multiplayer_hopper.dat` (the raw image the
game caches) or a download envelope; the command shows the 16 playlists it
found and publishes the file. Game data never goes in the repo - only to
the owner's server. Until one is imported or edited, Halo 2's hopper
endpoints serve the built-in lineup.

```
h2 live hopper defaults --out FILE [--raw] [--force]
```

writes that built-in lineup (eight playlists and three Clankermatch ones
on retail maps, built from the format alone) as a download envelope, or
with `--raw` as the raw image the game caches, to edit offline or import
elsewhere.
