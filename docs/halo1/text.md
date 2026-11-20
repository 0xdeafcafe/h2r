# Halo 1 text: `ustr`, `hmt `, and their conversion to Halo 2

Halo 1 keeps its user-facing text in two tag groups, read here by
`crates/h1text` through the [`h1map`](map-format.md) reader, and
converted into Halo 2 localized text ([localization.md](../formats/localization.md)):
string IDs, locale rows in the `matg` globals, and one `unic` tag per
Halo 1 list. This page covers both sides.

Evidence labels, as in map-format.md:

- **V — verified-on-disc:** checked against the owner's retail Halo 1 disc
  by the ignored tests `real_disc_every_string_list_decodes` (every
  `ustr` and `hmt ` on every map: 10 campaign levels, 13 multiplayer
  maps and `ui.map`, `$H1_ISO`) and
  `real_disc_a30_converts_and_reads_back_through_halo_2`.
- **S — unverified:** not yet independently checked here.
- **Design:** an implementation choice.

No game text, string dumps or community definition files are included in
the crate or its tests; the tests build invented string lists in memory.

## `ustr`: unicode string list

**V** (all 418 tags on the disc, 5,397 strings): the tag is a block
(count, address, unused) of 20-byte elements — the same data-reference
shape the `sbsp` material layout uses — followed by the strings, packed
in element order.

Element (`0x14` bytes):

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `00` | u32 | Byte size of the string, its NUL included; always even, at least 2 | V |
| `04` | u32 | Flags; 0 everywhere on the disc | V |
| `08` | u32 | Offset from the tag's data address, **unpadded** (below) | V |
| `0c` | u32 | Tag-data address of the string's bytes | V |
| `10` | u32 | Unused; 0 everywhere on the disc | V |

The strings are little-endian UTF-16, `size/2 − 1` code units and a NUL.

**V**, the offset field at `08`: it is the sum of the earlier elements'
sizes from the tag start, without padding. Campaign maps store the
strings unpadded, so it equals the address there (a30: every element);
multiplayer and `ui.map` dword-pad the string data, so the address leads
it by 2 per padded string (disc-wide 1,262 elements match exactly, 4,135
lead; e.g. `ui\hud\hud_item_messages` deltas 0, 2, 4, 4, 4, 6…). The
engine uses the address; the reader does too.

## `hmt `: HUD message text

**V** (all 34 tags on the disc, 400 messages): one UTF-16 string blob,
one block of 2-byte elements, one block of named messages. The tag opens
with a 20-byte data reference to the blob (same shape as a `ustr`
element), then two blocks:

| Offset | Field | Evidence |
|---|---|---|
| `00` | Data reference (size, flags, offset-from-tag-start, address, unused) to the UTF-16 blob | V |
| `14` | Block of elements, 2 bytes each | V |
| `20` | Block of messages, `0x40` bytes each | V |
| `28`..`80` | Zero padding (the blob starts at the tag's `+0x80` on every map on the disc) | V |

Element (`2` bytes): `type` (u8), `value` (u8).

- Type 0, **text** (**V**): the value is a length in UTF-16 units, NUL
  included. The lengths of all type-0 elements in tag order sum to
  exactly the blob (checked per tag on every map): the messages consume
  the blob sequentially, in element order.
- Type 1, **glyph** (**V**, **E** at `0x000dfc14`): a
  literal controller glyph, an action bound to a control, or an event
  parameter, by index (the
  shared `ui\hud\hud messages` uses 18, 32, 33, 24…; a30's level
  messages use one glyph, 26). Counts vary per tag, and no type other
  than 0 and 1 occurs on the disc; the blob sums leave no bytes for
  glyphs, so they carry no text of their own.

Message (`0x40` bytes):

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `00` | char[32] | Message name, NUL-padded, ASCII on the disc | V |
| `20` | u16 | Start of this message in the UTF-16 blob, in code units | E `0xdfbd6`, V on a10, a30 and d40 |
| `22` | u16 | First element of the message | V |
| `24` | u8 | Element count | E `0xdfbdf`, V |
| `25`..`40` | — | Zero on the disc | V |

**V:** the messages tile the element block in order — message `i` starts
where message `i−1` ends, at element 0, and together they cover every
element. A message's text is its parts in order: text runs and glyphs
(`HudMessage::plain_text` drops the glyphs).

Example shape (invented text): a message `tutorial_x` takes elements 0–2
(text "Press ", glyph 26, text " to do X"), and the next message takes
element 3, a single text run.

## The scenario's text references

**V**, every campaign map has these three fields; `ui.map`'s scenario
has none of them set:

| Offset | Field | Evidence |
|---|---|---|
| `574` | `ustr`: names scripts give placed objects (`custom_object_names`); null on a10, c20, d20 | V |
| `584` | `ustr`: in-game help text (`player_help_text_<level>` / `ingame help text`) | V |
| `594` | `hmt `: the level's HUD messages | V |

A non-null reference that names a tag of the wrong group is refused.

## Conversion to Halo 2

Halo 2 localized text is one global structure per map
([localization.md](../formats/localization.md)): per language, an index of
`(packed string ID, blob offset)` rows over a NUL-terminated UTF-8-like
blob, described by a descriptor in the `matg` at `+0x188`; a `unic` tag
is `0x34` bytes whose nine `(start, count)` ranges select rows in its
language's index. Halo 1 has only English, and addresses its lists by
tag and index rather than by ID.

`h1text::convert::Locales` builds the Halo 2 side, following the
`Converted` pattern ([models.md](models.md)):

- `Locales::add_ustr` mints one string ID per string, named
  `<Halo 1 tag path>\<index>`, and interns it into the `TagSet` so other
  tags can name the same text; `Locales::add_hmt` names each message
  `<Halo 1 tag path>\<message name>` and drops its glyphs (Halo 2 has
  its own button glyphs; the mapping is a later problem). Rows append to
  one English table: `(key, blob offset)` and the UTF-8 bytes plus NUL.
- Each list returns a `Converted` `unic`: 16 zero build bytes, the
  English range `[start, start + n)`, eight empty ranges. `install`
  places it like any other tag.
- The blob is UTF-8, and the engine's converter narrows four-byte
  sequences to their low 16 bits, so text outside the Basic Multilingual
  Plane would not read back; the converter refuses it (none occurs on
  the disc). Halo 1's `»`, `\r\n` and private-use characters are
  BMP-sized and pass through byte-identical.
- `Locales::patch_matg` writes the English descriptor (row count, blob
  size, index and blob file offsets) into a `matg` block. The offsets
  are plain file offsets of the two raw resources, so the set is built
  once to learn them (`BuildLayout::raw`), the descriptor is patched,
  and the map is built again: only field values changed, so the layout —
  and the raw offsets — stay put (asserted in the tests).

Verified end to end by the ignored disc test: a30's 21 string lists (19
`ustr`, 2 `hmt `) become 21 `unic` tags over 239 rows; every row reads
back through `tags::Localization` byte-identical, and synthetic tests
check the empty string, non-ASCII text, range isolation between two
`unic` tags, and the malformed inputs the reader refuses.

## Campaign integration

**Built.** `h1text::scenario::LevelText` rebuilds the level's `ustr` tags
and mission `hmt ` as `unic` lists under `h1\<source path>`. The base's
English rows keep their indices and bytes. New rows append at the end.
The `matg` describes the rebuilt index and blob with sector-aligned
file offsets. The pinned builder places them in the file prefix, and
the scratch writer relocates them as local resources. The byte extents
are row count times eight and blob size (E `0x1a01e0`).
`scnr +0x1f8` keeps the object-name `unic` reference. The converted
help `ustr` is available as a `unic` list. Mission scripts use the
scenario's `hmt ` index for the help that appears during play.

**Built.** The legacy `hmt ` is rebuilt from
`tags::defs/hmt_.txt` alongside its localized list. Halo 2's help
and objective setters read `scnr +0x20c` for the datum, then the
message pointer at `hmt +0x14` and the element pointer at `+0x0c`
(E `0x24c831`, `0x24c878`). The UTF-16 data reference, elements and
messages are laid out by the builder. Message indices keep their source
order. The compiler resolves tokens case-insensitively and fails the
conversion if a HUD message term cannot be resolved, since the native
setter indexes the array directly. On the ten campaign levels every one
of the 70 help setters and 44 objective setters names the message at
its encoded index. Each objective is one text run.

**Built.** Mission glyphs survive in both representations. Halo 1's
draw takes literal controls 0–17, resolves indices 18–28 through its
input-action table at `0x21bfc0`, and handles the last literal controls
at 29–31 (E `0xdfc14`–`0xdfd27`). The converter uses Halo 2's font
glyphs and input-action substitutions (E `0x22d2ee`, `0x22dcbc`,
`0x22df04`, [button icons](../input/button-icons.md)). The
shared event-message list uses glyphs 32 and up as the argument indices
in the player HUD message entry (E `0xdfc65`–`0xdfd19`).

**Built.** The campaign converter keeps the shared templates from
`hudg +0xfc` and binds them to the native interaction and pickup string
fields of the rebuilt objects. At `0x13925f` the game already chose the
object and action. The hook uses that string ID to select a template, then
expands the template with its argument rows. Vehicle entry uses the seat
and vehicle names from `hudg +0xc0`, the vehicle's `+0x13c` and the seat's
`+0xec` (E `0xe2d28`, `0xe2fc3`). Vehicle flip uses the vehicle name.

**Built.** Weapon pickup, swap and switch reminders use the silhouette
selected by the weapon's `wphi +0x13c` in the `hudg +0xa4` bitmap
(E `0xe2ed5`, `0xe2f5e`, `0xe31e7`, `0xdf520`). Each pickup weapon
with an authored icon on the ten Xbox campaign maps uses a static
sprite. The converter crops its UV rectangle and stores ARGB4444 pixels
and glyph metrics in the rebuilt `hmt `. Halo 2 measures and advances
the sprite inline with the text and button glyphs. The hooks supply
metrics to the font cache (E `0x140b20`, `0x140b80`, `0x141020`)
and pixels to the existing atlas upload (E `0x140870`, `0x140960`,
`0x1f85f`). The cache key includes the pixels and metrics so a changed
map sprite is loaded separately. A weapon with no authored sprite uses
its label. The a30 disc test compares the rifle prompt's glyph pixels
against its source atlas rectangle.

**Built.** On the native interaction fallback (E `0x191976`),
`exit_vehicle` takes the name of the local unit's live parent vehicle
while it occupies a valid seat, local input is enabled, and a script has
neither claimed Action nor reserved it until release. In a10 the cryo
lesson supplies its own help row, `tutorial_action_1`. Halo 1's action 3 reads that parent's
icon-message row (E `0xe2dd4`, `0xe25d0`). `swap_powerup` uses the
held and nearby powerup names in that order (E `0xe2d77`). The nearby
candidate comes from Halo 2's pickup scan (E `0x1508e0`, `0x150e30`),
and is eligible only when the unit is on foot and its held-equipment
slot (E `0xccb80`) names a live powerup of a different type. The Xbox
Halo 1 candidate code compares the incoming type with itself at
`0xf5939`, leaving action 5 unreachable through that comparison.
The converted trigger compares the two equipment definitions. It uses
the source equipment's pickup text when the icon-message row is the
placeholder. The interact press uses the native object attachment and
detachment paths (E `0x151c90`, `0xb8ee0`, `0xb9a50`) to exchange
the held and offered items. The ordinary automatic pickups still use
their native scan path when the held slot is empty.

The source control placement's `+0x34` holds a one-based custom phrase
index (E `0x118f16`, `0xe2e30`). The converter keeps that argument
through placement remapping. The interaction hook reads the selected
object's placement index at `+0x1a` (E `0x191cf0`, `0xb7c87`), so two
instances of the same switch can show different prompts. Pickup
notifications use the source item row at `+0x180` (E `0xe0147`) and
the next row's `%d` becomes the queued-count substitution U+E422
(E `0x24caf4`, `0x24d472`, `0x22d9e0`). Counted ammo and grenades
use the native queue's singular/plural selection and accumulation.

## Script control and the native HUD draw

**Built.** Every slot in the first table already has a native evaluator.
`h1scripts::hud::BUILTINS` checks the evaluator address and signature
in the retail Xbox XBE when naming the slot. A static survey of all
30 retail Halo 2 scenarios found no calls to these slots or 657–663.

| Halo 1 builtin | Halo 2 slot | Evidence |
|---|---:|---|
| `show_hud` | 625 | E `0x2aa5e0` sets the HUD visibility byte |
| `show_hud_help_text` | 626 | E `0x2aa640` sets the legacy help flag and returns the Boolean |
| `enable_hud_help_flash`, `hud_help_flash_restart` | 628, 629 | E `0x2aa700`, `0x2aa770` write the flag and timestamp |
| `hud_clear_messages` | 664 | E `0x2aae20` clears the native display queue through `0x24cdaf` |
| `hud_set_help_text`, `hud_set_objective_text` | 665, 666 | E `0x2aae40`, `0x2aae90` pass the short index to `0x24c831`, `0x24c878` |
| `hud_set_timer_time`, `hud_set_timer_warning_time`, `hud_set_timer_position` | 667–669 | E `0x2aaee0`–`0x2ab002` write duration, threshold and anchor |
| `show_hud_timer`, `pause_hud_timer`, `hud_get_timer_ticks` | 670–672 | E `0x2ab010`–`0x2ab12c` manage visibility, pause and remaining ticks |

**Built.** `boot::h1_hud_text` restores the link between selection and
drawing. Halo 2's legacy display already tests an objective's hold time
and the help visibility flag at `0x24d02c` and `0x24d045`. Its text
pass copies an inline UTF-16 buffer at `0x24d2b7`. The hook opens that
path for the selected message and supplies a full-length buffer to the
substitution, measurement and draw calls. Halo 2 draws it with its own font and
current controller bindings. Objective messages have priority during
their native hold time. Help flashing applies Halo 1's `hudg +0xd0`
record, sampled from Halo 2's script-controlled tick (`+0x1380` in the
HUD state). The original help draw passes that record to its flash
curve at `0xdf9d3` and the curve is at `0xe04b0`.

**Built.** The countdown uses the final text pass of the legacy draw
(`0x24d67f`, `0x24d698`, `0x24d6bb`) while the timer is shown. The
text takes the script's corner and offsets. A warning colour starts at
the native threshold. This pass carries the countdown in place of its
normal auxiliary label while the timer is up.

**Built.** `h1scripts::hud::CONTROL_BUILTINS` reserves the following
retired entries. Their retail signature is `(boolean) -> void` and
they share evaluator `0x2ab130`. The hook evaluates the argument with
the retail VM at `0x209d50` and returns through `0x209ae0`. It runs
only when the scenario names a rebuilt mission HUD. The slots are
independent of AI's slot 300.

| Slot | Control |
|---:|---|
| 657 | `hud_show_health` |
| 658 | `hud_blink_health` |
| 659 | `hud_show_shield` |
| 660 | `hud_blink_shield` |
| 661 | `hud_show_motion_sensor` |
| 662 | `hud_blink_motion_sensor` |
| 663 | `hud_show_crosshair` |
| 903 | `display_scenario_help` |

**Built.** Slot 903 has the retail discarded `(short) -> void`
signature and shares the argument evaluator. The Halo 1 evaluator
`0xf2f30` passes a help-list row to `0xd0930`. The converter appends the
scenario's help rows to its HUD tag, with their own base index so the
mission indices still address the original messages. The evaluator shows
the selected row in the native HUD help panel. The two a30 cave rows use
`%move`, `%look` and `%left-stick`. The first pair follows the current
player's thumbstick layout at `0x51ea18 + 0x1c * port + 0x18`
(E `0x22df4c`). The last token always shows the physical left stick.

The flags reflect the stores in Halo 1's evaluators
(`0xee820`–`0xee580`). The widget gate at `0x13ac87` matches string
IDs recorded at conversion. The radar blip draw at `0x1a2823` follows
the motion-sensor visibility gate.

**Built.** The campaign health and shield panels and the two radar
layers keep their original bitmap alpha. Each layer carries its own
colour pair and flash timing from `unhi`, following the static draw at
`0xe18ed` and the flash curve at `0xe04b0`. The sampled ARGB goes into
the native widget shader's active combiner constants at draw time
(E `0x22a7dd`, `0x18ee0`, `0x1b050`). The material cache is restored
after the native material upload. The fill and blip passes keep the separate
widget behaviour seen in Halo 1 at `0xdd2ea`, `0xdd52e` and
`0xdd690`.

The retail HUD readers consume the first `0x18` bytes of the rebuilt
`hmt `. The rest holds converter metadata for the message lists and
widget controls. The hooks check the marker and source call site. The
simulation thread writes the control bits and blink start ticks from
scripts. The render thread reads the resulting guest state and uses
the native draw calls.

The code and regression tests are ready for the integration pass. The
evidence here comes from static inspection of the two discs and XBEs.
## Commands

The campaign import installs the converted lists. `h2 h1 tags` lists
source `ustr` and `hmt ` tags. `h2 strings` reads the localized lists
in the converted map.

## Verifying on the disc

```
H1_ISO=/path/to/halo1.iso cargo test -p h1text -- --ignored --nocapture
```

`real_disc_every_string_list_decodes` prints, per map, the `ustr`/`hmt `
counts, string and message totals, and glyph counts; disc-wide:
**418 `ustr` tags, 5,397 strings; 34 `hmt ` tags, 400 messages; 0
failures.** `real_disc_a30_converts_and_reads_back_through_halo_2`
prints the unic/row counts and the strings-checked count (239).

The integration test checks the native and retired builtin signatures,
loads converted a30 and d40 back from pinned and fresh builds, checks
the base locale rows and new mission rows, and resolves the script
message atoms and countdown calls.

```sh
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1campaign --test disc campaign_hud_text_and_builtins_round_trip -- --ignored
```
