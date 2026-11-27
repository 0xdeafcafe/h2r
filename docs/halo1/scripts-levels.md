# Halo 1 scripts: level data and dialogue

Level data the scripts name and scripted dialogue, split from [scripts.md](scripts.md).

| Section |
|---|
| [Level data the scripts name](#level-data-the-scripts-name) |
| [Scripted dialogue](#scripted-dialogue) |

Part of [scripts.md](scripts.md).

## Level data the scripts name

Script atoms of these types index scenario blocks, so the converter writes
Halo 1's blocks into Halo 2's in Halo 1's order and each atom keeps its
index. `h1scripts::level` reads and writes them; `h1scripts::seats` builds
the seat-mapping table. Offsets come from the surveys in
`crates/tools/h1campaign/tests/scripts/survey.rs` (they print offsets, strides and counts
only): for every script atom of a type, the block whose element at the
atom's index holds the atom's token as its name.

| Data | Halo 1 `scnr` (stride, name) | Halo 2 `scnr` (stride, name) | Atom value (Halo 2) |
|---|---|---|---|
| cutscene flags | `0x4e4` (`0x5c`, `+4`); position `+0x24`, facing `+0x30` | `0x1e0` (`0x38`, `+4`); position `+0x24`, facing `+0x30` | index |
| camera points | `0x4f0` (`0x68`, `+4`); position `+0x28`, orientation `+0x34` | `0x1e8` (`0x40`, `+4`); position `+0x28`, orientation `+0x34` | index |
| titles | `0x4fc` (`0x60`, `+4`) | `0x1f0` (`0x24`, string ID `+0`) | index |
| device groups | `0x288` (`0x34`, `+0`); initial value `+0x20`, flags `+0x24` | `0xa0` (`0x28`, `+0`); same fields | datum `(0xe564 + i) << 16 \| i` |
| recorded animations | `0x36c` (`0x40`, `+0`); stream data reference `+0x2c` | `0x110` (`0x34`, `+0`); stream data reference `+0x2c` | index |
| seat mappings | (seat text in the script) | `0x228` (8: unit `vehi` datum, seat mask) | `count << 16 \| first` |

Evidence: Halo 1, a30 (every flag, camera point, title and device group
token at its index). Halo 2, 01b, 03a and 05a (the same, with the schema's
strides). Two of 03a's six camera points put the position at `+0x24`
instead; the two its scripts use (`e8_prediction`, `e1_prediction`) have it
at `+0x28` and the orientation after it, the layout written here. Titles:
Halo 1 stores the displayed-text index at title `+0x30` in its scenario
help list (`scnr +0x584`). The script token names the title record. Its
Halo 2 record uses the localized string ID at that index, from the
`unic` referenced at `scnr +0x200`. Bounds, justification and both
colours come from the Halo 1 title. The three durations are converted
from 30 Hz ticks to seconds. The chapter font is 9 on 01b, 03a and 05a.

**Seats.** A unit's seats block is `vehi +0x1c8`, `0xb0` each, the label
string ID at `+4` (V: every `vehi` on 05a). A mapping element is a unit
definition and a mask of its seats; retail matches a seat when its label
starts with the script text (05a: `pelican_p` is the pelican's bits 1–10,
`pelican_p_l01` to `pelican_p_r05`). Halo 1 names roles on Halo 1's vehicles
(`driver`, `gunner`, `passenger`, `p-riderlf`), and the converted level
drives Halo 2's vehicles, so `seats::h2_label_matches` maps roles to Halo
2's label suffixes: driver `_d`, gunner `_g`, passenger `_p`, rider or
passenger with a side `_p_l…`/`_p_r…`, cargo `_lc`; boarding seats (`_b_`)
never; other text by retail's prefix rule. One run of elements per text,
over every `vehi` in the converted set.

**Navpoints** take Halo 2's `default` (0): Halo 1's navpoint types
(`default_red`, ...) are not mapped to Halo 2's. **Tags** (looping sounds,
effects, damage, animation graphs) resolve by the Halo 1 tag's name among
the converted tags of the Halo 2 group. **Built.** The campaign installs every source `lsnd`
before script compilation and retains it by name through the final map
build. [Loop conversion and disc audit](sounds.md#campaign-loops-and-cache-slots-built).

**AI.** Most `ai` hooks left were squads `h1ai` dropped for having no
starting locations. Retail Halo 2 keeps such squads (06b: 28 of 170;
`h1campaign`'s `squad_flag_bits` survey), so `h1ai` keeps them, with no
locations, no actors and no order, and `h1campaign` keeps only those the
scripts name. The campaign retry ladder preserves model materials at
every step (campaign.md).
Each referenced platoon becomes a child squad group of its encounter.
A placement or actor list receives the encoded group index. Migration
into a platoon uses the compatible destination squads chosen by
`h1ai::control`, including empty squads. On a30, the first dropship
places and loads the same landing platoon containing Grunts and Elites.

**a30 mission flow.** Halo 1's 139 script definitions and 108 globals
keep their indices. Each scenario-global reference takes its source type
from its declaration. On disc, a30 contains references with opcode 0 and
set targets with opcode 0xffff. The initial placement helper runs before
the authored startup body and places only `mid_cliff_1`. The opening
cutscene, both chapter titles, checkpoint waits, the eleven mission wakes
and all three extraction branches are checked in the converted-map disc
test. The extraction branches each call `game_time` three times. Two of
the calls participate in waits. The clock and danger gates come from the
unit/miscellaneous builtin family, while the recording, vehicle and HUD
operations use their respective families. Mission flow adds no builtin slots.

**Built.** In `cutscene_intro`, a30 requests convolution with operands
`(1, 1, 3.0, 0.0, 7.0)`. The compiler emits the same checkpointed
parameters and simulation-clock timestamp as on the other levels. The
renderer reduces the radius from three to zero over 210 simulation ticks.
The a30 disc test reads those operands from the converted intro. The
driving hints in `objective_cave` use the HUD text family's
`display_scenario_help`.

**Checkpoint storage.** At `0x208ff0` retail allocates 320 script threads
(stride `0x418`) and 3,072 global cells (stride 8) in game state. The
first 1,053 cells belong to engine globals. The a30 AI registries alone
require 1,896 cells, and its 59 conversations add worker state. The
combined scenario exceeds the retail cell allocation.

At `0x2090a0`, before the interpreter clears threads and initializes the
new map, `engine::hs_pool` sizes both arrays. For an oversized table the
global values occupy the tail of the thread allocation and their bitmap
uses the original global allocation. Both headers stay at their original
addresses. External values, salts and occupancy survive the transition.
The remaining bytes hold the maximum number of whole thread records and
their bitmap. The converter checks that the persistent scripts leave a
free thread. A retail-sized scenario restores the original layout. All
values and array pointers are in the game state saved by checkpoints.

**Syntax storage.** Halo 1 holds up to 19,001 nodes per scenario, and a10
uses 18,735 slots (17,516 live). Halo 2 compiles them about one to one,
but the generated command scripts, AI controls and conversations
roughly double a10: 36,845 nodes with neither AI controls nor
conversations, 41,229 for the whole converted level (both before the
shared initializer roots below). Retail's pool is smaller. At `0x20a700`
retail allocates the syntax array header and an occupancy bitmap for `0x9000` slots in one `0x124c`-byte block from the
system heap (`push 124Ch`, `push 9000h`). The nodes themselves are not
copied: at each map load `0x20a750` passes `scnr+0x238`'s count and
address to `0x16b6b0`, which makes the count the capacity and sets or
clears one bitmap bit per slot. A larger scenario overruns the block.

`engine::hs_syntax` runs before `0x20a750`. When the scenario has more than
`0x9000` slots, the header's bitmap pointer (`+0x48`) moves to an
`0x2000`-byte bitmap in the feature arena. A retail-sized scenario gets
the original bitmap back. Nothing else changes: the attach still sets the
capacity and rebuilds occupancy from the salts. Retail's own capacity
already exceeds `0x7fff`, so the interpreter reads node indices as unsigned
16-bit. The converter's limit (`h1scripts::compile::MAX_NODES`,
`hs::MAX_NODES`) is `0xffff` slots. Slot `0xffff` is excluded because its
salt (`0x8000 | slot`) would make its datum `0xffffffff`, which means none.
The header and bitmap come from the default heap (`*0x468758`), not game
state, so checkpoints neither save nor restore them; the arena is guest
RAM in machine snapshots.

Generated globals with the same literal initial value share one root node
(command-list actor banks, conversation state). A root has no sibling, and
each global keeps its own value cell.

## Scripted dialogue

**Built.** `compile_with_conversations` lowers `ai_conversation`,
`ai_conversation_stop`, `ai_conversation_status`, `ai_conversation_line`
and `ai_conversation_advance` against the decoded scenario block
([ai.md](ai.md)). Generated static calls start, stop or advance a
continuous worker per scene. Status and line calls read its globals. All
campaign dialogue operands are literal scenario indices, checked against
their original names. Original script and global indices stay stable.

Halo 1's start evaluator (`0x000f2fb0`) returns whether the request was
accepted. Its status evaluator reads 0 idle, 1 trying, 2 assembling,
3 playing, 4 waiting for advance, 5 failed to start, 6 complete and 7
aborted (`0x00142770` and `0x00142809`). The generated script exposes that
scale. It reports 2 once a cast is ready and through any wait for the
first speaker to assemble.
The line query starts at -1 while the request is pending, counts lines from
zero and returns 999 when idle or terminal, as at `0x00142710`.
Pauses and advance remain asynchronous. A stop followed by a new start
cannot resume playback from the cancelled request.

**Built.** `ai_dialogue_triggers` renames to `ai_dialogue_enable`.
Halo 1's evaluator (`0x000f0880`) writes the boolean to its AI globals
at `+0x10`. Halo 2's named evaluator (281, `0x002a4dc0`) writes the
counterpart at `+0x20`. This controls spontaneous AI speech.

**Built.** `unit_set_emotion` uses Halo 2's unnamed `(unit, short) -> void`
builtin 218. Halo 1 writes the expression selector at `0x000f15a5`.
Halo 2's evaluator at `0x002a3b70` forwards the numeric selector to
`0x0010e9f0`, the same facial expression setter used by its named
`unit_set_emotional_state`. The original numeric argument, including the
reset value -1, is preserved.

The synthetic tests check the generated roots, speaker references and sound
datums. The ignored disc tests check every campaign scene against Halo 2's
retail catalogue and exercise playback, hold, advance, stop/restart and
simultaneous voices sharing one tag on the Rust HS scheduler. Boot unit
tests cover the native call ABI and the cast rules.

```sh
cargo test -p boot -p h1ai -p h1scripts --lib conversation
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso \
  cargo test -p h1scripts --test conversations_disc -- --ignored
```
