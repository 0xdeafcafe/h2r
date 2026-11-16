# Game variants: record, schema and saved files

`crates/variants` is the game variant editor of the editor suite: the
0x130-byte variant record, the settings schema, saved-variant files and the
egui panel. No game data is in the repository: the schema and the built-in
variants are read from the user's disc at run time, tests use synthetic
bytes, and the checks against the retail XBE and disc are `#[ignore]` tests
that read `$H2_XBE` (and `halo2.iso` beside it, or `$H2_ISO`).

Everything below was read statically from the retail XBE (SHA-256
`03215919…935d`) with a scratch disassembler and from the retail main menu
map with the ignored test. Nothing was run in the game. Each claim is
marked **confirmed** (seen in code or data) or **inferred**.

## The record

The record is the active variant at `G+0x13c` (docs/live-gamestate.md §3),
the whole payload of a saved variant, and the last 0x130 bytes of a hopper
record slot (`crates/live-proto/src/hopper.rs`, `R_VARIANT`). A hopper tool
links a variant by copying `GameVariant::to_bytes()` into the slot.

| Offset | Type | Content | |
|---|---|---|---|
| `+0x00` | u16 | bit 0: built-in flag; nothing else may be set | inferred meaning, confirmed limit |
| `+0x02` | u8 | unknown, preserved | |
| `+0x03` | u8 | built-in description number (0..=100) | confirmed |
| `+0x04` | UTF-16 [32] | name; the last unit is always 0 | confirmed |
| `+0x44` | i32 | game-engine index: 1 CTF, 2 Slayer, 3 Oddball, 4 King, 7 Juggernaut, 8 Territories, 9 Assault | confirmed |
| `+0x48` | u32 | flags: bit 0 team play, other bits are settings (below) | confirmed |
| `+0x4c..+0xf0` | | common settings (below) | confirmed |
| `+0xf0..+0x10c` | | game-type area: its meaning depends on the engine index | confirmed |

### Where each setting lives (confirmed)

The editor and the built-in loader both store a setting with `0x2373be`
(EAX setting, EBX value, ESI record). It calls a per-setting descriptor
from a 0x70-entry pointer table at `0x470828`, which answers a storage kind,
an offset and a bit or scale; then it stores the value:

| Kind | Store |
|---|---|
| 0 | bit of a u16 |
| 1 | bit of a u32 |
| 2 | byte, `value != 0` |
| 3 | byte, `value * (1.0 / scale)` |
| 4 | i16, same |
| 5 | i32, same |
| 6 | f32, same |

Every scale in this build is 1.0, so values are stored as given. Settings
27, 31..37, 48 and 49 have no descriptor. `record::field` is our
encoding of the answers. The ignored test `descriptors_match_the_xbe`
should decode the table from `$H2_XBE` and compare (not written yet; see
"Status").

Which settings belong to which game type comes from the `goof` tag's pages
(below): the type-area settings are Slayer 28..30; Oddball 43..47, 100,
102; King 38..42, 101; CTF 50..58, 106, 107; Assault 59..67, 92, 95, 97,
109, 110; Juggernaut 68..73, 96, 108; Territories 74, 98, 99. Setting 103
is stored but on no page. Settings 1..9 are each type's score to win
(1 CTF, 2 Slayer, 3 Oddball, 4 King, 7 Juggernaut, 8 Territories, 9 Assault)
and all store at `+0x50`.

### The clean-up (confirmed, two limits inferred)

Loading a saved variant runs `0x19d650` on it. It returns 1 only if no
rule changed the record; otherwise it overwrites the whole record with a
default Slayer and the load fails. So an edited record must satisfy every
rule. `record::check` reports the broken ones:

- `+0` only bit 0; `+0x42` (last name unit) 0; engine 1..=4 or 7..=9;
  `+0x48` below bit 15; `+0xb0` 0.
- i32 `+0x4c` 0..6; `+0x50`, `+0x54`, `+0x7c`, `+0x80`, `+0x84`, `+0xac`
  0..0x7fff; `+0x58`, `+0x88`, `+0xa4`, `+0xa8` 0..2; `+0x74`, `+0x78`
  0..16; `+0xb4` 0..8.
- bytes `+0xcc` ≤3; `+0xcd`, `+0xce` ≤7 (inferred from `eax=7`);
  `+0xcf..+0xd1` ≤4; `+0xd2`, `+0xd3` ≤6; `+0xd4` ≤0x13; `+0xd5` ≤3;
  `+0xd6`, `+0xd7` ≤0x14.
- Per engine (jump table at `0x19de54`): CTF forces team play, `+0xf0`
  mask 0xff, i32 `+0xf4` 0..0x7fff, `+0xf8` 0..2, `+0xfc` 0..1, `+0x100`
  0..3, `+0x104` 0..2; Assault: i16 `+0x108`, `+0x10a` 0..15, then the CTF
  rules; Slayer `+0xf0` mask 7 (inferred); Oddball mask 7 (inferred), i16
  `+0xf4` 0..3, `+0xf6` 0..1, `+0xf8` 0..2, `+0xfa` 0..3; King mask 0x1f, i16
  `+0xf4` 0..0x7fff; Juggernaut never team play, mask 0x7f, i16 `+0xf4`
  0..2; Territories i16 `+0xf0` 1..8, `+0xf2` and `+0xf4` 1..0x7fff; Race
  and Headhunter always reset.

## The schema (`goof` and `sily`)

`mainmenu.map` has one `goof` tag (docs/community-gametypes.md).

- `+0x18`: pages, 0x18 bytes each: `+0` page id, `+8` count and `+0xc`
  address of 8-byte `sily` references. Confirmed (`0x2369b3`, `0x236a3a`,
  retail tag). The page ids in the retail tag: `engine - 1` match options
  (rounds, score, time and three more), 9 player options, 11 vehicles, 12
  weapons, `12 + engine` the type's own options, `21 + engine` a summary page
  that repeats settings from the others, `31 + engine` team options. Page
  titles are not in the tag (the UI screens carry them); ours are symbolic.
- `+0x20`: the built-in names' `unic` reference (datum at `+0x24`).
- `+0x28`: the built-in variants (docs/community-gametypes.md).
- `+0x24 + 0x14 * content type`: each game type's defaults, a block of
  `(setting, value)` pairs that `0x1489b5` applies through the setter to a
  fresh record, indexed by the save-content type (below); the Race and
  Headhunter slots are empty. Confirmed. Before that, `0x19d220` sets up
  the record; that it starts zeroed is **inferred**.

A `sily` tag: `+0` setting number, `+8` its `unic` reference (datum
`+0xc`), `+0x10`, `+0x14`, `+0x18` three string ids (title and help
inferred from the order), `+0x1c` the options block of 0xc-byte options
(`+0` default flag, `+4` value, `+8` label string id). Confirmed layout.

Without a disc, `Schema::fallback` lists every setting the record stores
with our names, any value the record accepts, and no built-ins.

## Saved variants

### The container (confirmed)

A saved variant is an XAPI save game (docs/xapi.md): a directory
`U:\<12 hex digits>\` named by the hash of the save name, with
`SaveMeta.xbx` (`FEFF`, `Name=<save name>` CR LF, UTF-16LE). The game
creates it with `0x2ad020` → XCreateSaveGame, CREATE_NEW and flags 0 (no
`NoCopy`). On our runtime `U:\` is
`<data root>/disk/Partition1/UDATA/4D530064/`, by default
`~/Library/Application Support/h2recomp/disk/Partition1/UDATA/4D530064/`.

The save name is `<content-type label>: <variant name>`, at most 0x80
units (`0x216da0`). The label is per language, compiled into the XBE (the
per-type functions behind `0x217a80`, e.g. `0x2176e0`), not in the tags. The
listing does not need the prefix: it strips a known label (`0x216e50`) and
takes the game type from the file present. But the game rebuilds the name
when it re-saves or deletes, and XCreateSaveGame/XDeleteSaveGame compare
`Name`, so an editor must write the game's exact name or the game cannot
later overwrite or delete the save (inferred).

### The file (confirmed)

One file per save directory, no extension, named by the content type
(`0x216b60`: a table of 11 ASCII names, read from the XBE at run time).
Content types: 0 profile, 1 Slayer, 2 King, 3 Race, 4 Oddball,
5 Juggernaut, 6 Headhunter, 7 CTF, 8 Assault, 9 Territories, 10 playlist.
The listing tries each type's file name in turn (`0x216537`); the first
that exists gives the type.

| Offset | Size | Content |
|---|---|---|
| `0x000` | 0x130 | the record, as in memory, after the clean-up (`0x212ec0`) |
| `0x130` | 0x14 | signature |

No header, magic or version. Writing: `0x2ae090` writes the data at 0,
signs it and writes the signature right after.

### The signature (confirmed; the key on our runtime unverified)

`XCalculateSignatureBegin(0)` (roamable; both the writer `0x217460` and the
reader `0x2163aa` pass 0), Update over the 0x130 bytes, End:

    signature = HMAC-SHA1(key = XboxSignatureKey (16 bytes), record)

`XCalculateSignatureBeginEx` (`0x2d65a7`) takes the key from kernel export
325 `XboxSignatureKey` (via `[0x411660]`) when the title id is the
certificate's own, else from `XboxAlternateSignatureKeys`. End's roamable
path (`0x2d64c0`) is a plain HMAC finish. The non-roamable path (not used
here) adds a second HMAC with the hard-disk key.

Our kernel never fills export 325 (crates/kernel/STOPS.md), so on our
runtime the key is 16 zero bytes. **Unverified**: no save written by the
game under our runtime has been compared yet. To check: save one variant
in the game, then run the editor's "verify" on it (or the ignored test
`saved_variant_verifies` with `H2_SAVED_VARIANT=<path to the file>`). A
real console derives the key from the certificate's signature key with a
console secret, so its saves cannot be re-signed here; the key is a
parameter of the codec.

### Loading (confirmed)

`0x2adf70` reads exactly 0x130 bytes (else error 0x26), signs them, reads
the next 0x14 bytes and compares. Only a match marks the load good. The
size is never checked, so trailing bytes are ignored (inferred). Then
`0x212bc0` runs the clean-up on a copy and fails if it changed anything.
What the UI shows for a failed load was not traced.

### Copy-on-edit (confirmed)

Built-ins carry bit 21 in their content handle and are copied from the
content list, never read from disk (`0x2162c0`). Editing one (`0x212b2c`)
clears record bit 0 of `+0`, runs the clean-up, sets the new name (at most
0x1f units), zeroes `+0x42` and saves it as an ordinary variant.

## Portable files

Planned (not written yet): `.h2variant`, our sharing format, JSON with a format tag, version, the
name and game type for people, and the 0x130 record as hex (the source of
truth). Importing checks the length and the clean-up rules; the game never
sees this file, so it carries no signature.

## Status

Done: the record model and lossless codec (`record`), the clean-up rules,
SHA-1/HMAC, the schema reader for `goof`/`sily` with page grouping and
defaults (`schema`), all from the facts above.

Not done (the job stopped when the shared disk fell under 10 GiB): the
saved-file codec and store (`save`), the XBE readers for the content-type
file names and labels, the portable format, the panel, the
`descriptors_match_the_xbe` test, and running the crate's tests after the
last edits to `record::fallback_owner` and the schema's page, `sily` and
defaults offsets.
