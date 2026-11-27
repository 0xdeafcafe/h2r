# Tag field names: sources and licences

Evidence for the tag field names and descriptions the editor shows
([tag-editing.md](../tag-editing.md)).

## 2026-11-21: the Halo 2 fallback folder and other retail games

Commit: branch `tag-names-xver`, from `c263b9cbc`. Reference revision:
`fd14ef3c46135e9422966c6a64cd926fb0bd23fb`. Method: a read of the
plugin tree and of `src/Blamite/Formats/Engines.xml` at that revision,
and an offline comparison script, kept outside the repository, that
reads our definitions and the plugins. The Rust code was not built or
tested.

### Why the earlier comparison found 14 plugins

`Halo2Xbox` holds only the 14 groups whose Xbox layout differs.
Assembly's engine table gives the retail Xbox build (engine "Halo 2",
build `02.09.27.09809`) `Halo2Xbox` as its plugin folder and `Halo2`
as its `fallbackPlugins`. A group with no `Halo2Xbox` plugin is read
from `Halo2`, the retail Halo 2 Vista folder (98 plugins, roots
`game="Halo2"`). The weapon, equipment, biped, scenery, machine, crate
and damage plugins are there. The earlier comparison opened `Halo2Xbox`
alone, and the reader refused any root but `game="Halo2Xbox"`.

The four possible causes, checked:

| Cause | Finding |
|---|---|
| File names differ from our groups | One case. Assembly writes `_` for characters a Windows file name cannot hold: `_fx_.xml` is `<fx>`. Every other object group uses its own spelling. |
| Shared base layouts | Not the cause. Each `Halo2` plugin is flat and repeats the object, item, unit and device fields. |
| The reader skips them | Yes. `Folder::open` read only `Halo2Xbox` and required its game attribute. |
| A different branch | No. `master` has both folders. |

The root sizes of the `Halo2` plugins for `weap`, `eqip`, `vehi`, `bipd`,
`proj`, `scen`, `mach`, `bloc` and `jpt!` equal our definitions'. The
`Halo2` and `Halo2Xbox` copies of the 14 overridden groups differ in a
few strides and tooltips (in `vehi`, the phantom shape element is `0xf0`
in `Halo2` and `0x2a0` in `Halo2Xbox`).

### The reader

`tag_names::assembly::Folder` indexes `Halo2Xbox` and, for the groups it
lacks, `Halo2` (`FALLBACK`). Each file's root must name its folder's
game. `Folder::game` says which folder a group came from; both carry the
`assembly` source in a merge, as Assembly itself does.

`tag_names::assembly::Hints` indexes the other retail folders in
`HINTS`: `Halo2MCC`, `Halo2A`, `Halo2AMCC`, `Halo3`, `Halo3MCC`, `ODST`,
`ODSTMCC`, `Reach`, `ReachMCC`, `Halo4`, `Halo4MCC`. Each root names its
folder. Halo 2's folders read references, blocks and data references as
8 bytes; the later engines' as 16, 12 and 20. `tag_names::hints` returns
a name from one of them for an unnamed field of ours only where the
structure around it matches:

- the plugin's field starts at our offset with our width: a scalar over
  an unknown word, or a compatible type over a typed field;
- and the element has the plugin element's size with every named field
  of ours lining up, or the nearest named field on each side lines up.

A hint carries its game and is never written as our name.

`REFUSED` names the pre-release and leak-derived folders: `Halo2Alpha`,
`Halo2Beta`, `Halo2E3_04`, `Halo3Alpha`, `Halo3Beta` (and so its
`march_delta` children), `Halo3PreAlpha`, `ReachAlpha`, `ReachBeta`,
`Halo4NetTest`, `HaloOnline`, `ElDewrito`, `ManagedDonkey`, and four
other pre-release spellings kept from the first list. A refused name anywhere in
a path refuses it. The retail names the first list held (`Halo3`,
`Halo2MCC`, `Halo4` and others) are no longer refused.

### Names written into the definitions

A name carries no citation: the definitions and the editor show it as
our name, with no source label. Names are our snake_case, without the plugin's parentheses, units or descriptions.
A `degree` field gets `unit=degrees`. Enumerations and flags get no
value or bit names. A new scalar tiles one or more whole consecutive
unknown spans; a plugin field that crosses a named field or a span
boundary is left out. A plugin reference, string ID, block or data
reference over an unknown span is left out: our schema has no fixup
there.

Sources per definition:

| Definition | Sources, in order | Checked against |
|---|---|---|
| `obje` | `Halo2Xbox vehi.xml`, `Halo2 scen.xml` | `Halo2` `weap`, `bipd`, `mach`, `bloc` |
| `item` | `Halo2 weap.xml` | `Halo2` `eqip`, `garb` |
| `unit` | `Halo2Xbox vehi.xml`, `Halo2 bipd.xml` | each other |
| `devi` | `Halo2 mach.xml` | `Halo2 ctrl.xml` |
| `weap`, `eqip`, `bipd`, `proj`, `scen`, `mach`, `ctrl`, `bloc`, `jpt!`, `ssce`, `crea`, `garb` | `Halo2 <group>.xml` | |
| `vehi` | `Halo2Xbox vehi.xml` | |

"Checked against": a shared-layer field is taken only where every other
plugin with a field at that offset agrees on its type, width and stride.
No other game's plugin supplied a name to the definitions.

Fields the Halo 1 converter writes are named too, against the earlier
rule that such a field keeps its byte-valued path: the owner decided to
name them. The converter writes the same bytes under the new names,
raw words over typed fields (`Record::set` with `Val::Bytes`,
[tag-definitions.md](../../formats/tag-definitions.md), "Raw bytes into
typed fields"). A word that names no field start is addressed by its
name and offset (`pitch_range+0x4`) or by its element offset (`@0x38`),
as the `weap` triggers, `vehi` phantom and `obje` change colours do in
their loops. Every path in `crates/tools/h1objects`, its tests, the
`h1campaign` disc test and the studio's chain labels moved with the
names.

### Counts

Counted by reading, as the registry counts them
(`Layout::coverage`): each embedded struct's fields once per group, a
block's element once, an unknown span as its words (`registry::fill`),
a typed field whose name ends in its offset as unnamed. A multi-word
field (a float range, a point) counts once, so a group's total falls
where words merge.

| Group | Named before | Unnamed before | Named after | Unnamed after |
|---|---:|---:|---:|---:|
| `weap` | 60 | 244 | 244 | 56 |
| `vehi` | 111 | 374 | 309 | 146 |
| `bipd` | 46 | 305 | 256 | 75 |
| `eqip` | 33 | 73 | 85 | 8 |
| `proj` | 55 | 87 | 119 | 11 |
| `scen` | 27 | 54 | 62 | 8 |
| `mach` | 41 | 63 | 83 | 9 |
| `ctrl` | 40 | 63 | 83 | 8 |
| `bloc` | 27 | 53 | 60 | 8 |
| `jpt!` | 4 | 59 | 55 | 9 |
| `ssce` | 27 | 56 | 59 | 11 |
| `crea` | 27 | 180 | 103 | 92 |
| `garb` | 30 | 114 | 81 | 50 |

Still unnamed where the plugin names a field: `weap` +0x210 and +0x214
(two plugin fields share one name). `jpt!` +0x14 and +0x8c and the
`weap` barrel words +0x38, +0x3c, +0x48..+0x54 and +0x70..+0x78 stay
unknown spans.

The shared layers count in every group that embeds them.
`H2_FIELD_NAMES` with the ignored comparison test prints the registry's
own counts.

### Disagreements found

| Definition and offset | Ours | `Halo2` plugin | Kept because |
|---|---|---|---|
| weap +0x130, +0x284 | Unknown words | String IDs | No string fixup in our schema. |
| bipd `block_1c8` | Element `0xb4` | Element `0xb0` | The schema infers `0xb4` in `bipd`; objects.md and the `vehi` element have `0xb0`. Correcting the stride is schema work. |

## 2026-11-21: retail reference and checked-in mappings

On 2026-11-21 the owner of this repository, one of Assembly's creators,
gave permission to use Assembly's retail Xbox plugins as a reference for
this repository's own field names and descriptions. Assembly is GPL-3.0.
Only `src/Assembly/Plugins/Halo2Xbox` is read. No plugin file or verbatim
description, comment or option list is committed. This decision supersedes
the runtime-folder design in the entry below. [Assembly](https://github.com/XboxChaos/Assembly)

Reference revision: `fd14ef3c46135e9422966c6a64cd926fb0bd23fb`.
Method: a depth-one, blob-filtered checkout restricted to that directory,
compared by root offset, block stride and element offset with the tag
definitions and relocation schema. The retail directory contains 14 XML
files. The comparison for these mappings covers 13 groups. The scenario
definition is maintained separately.

At this entry's commit, `A Halo2Xbox <filename>` in a definition's
evidence identified an observation from that reference; the entry above
drops the label. New labels use our
snake_case spelling. Descriptions are single-line sentences written
for the editor. New scalar fields tile an entire existing opaque span.
Shared layers contribute their fields to every definition that embeds
them.

### Mapped declarations

Counts are new names and authored descriptions, once at the declaration.
The `DECR` count includes two shared mesh records also used by `PRTM`.
Descriptions include those added to existing names and retained byte
spans. The `itmc` example is based on its existing engine evidence.

| Group or shared layer | New names | Descriptions |
|---|---:|---:|
| Shared `obje` (`vehi`) | 8 | 27 |
| Shared `unit` (`vehi`) | 16 | 35 |
| `vehi` suffix | 45 | 63 |
| `bitm` | 34 | 38 |
| `jmad` | 48 | 63 |
| `shad` | 3 | 13 |
| `sbsp` | 151 | 151 |
| `skin` | 10 | 10 |
| `spas` | 11 | 11 |
| `vrtx` | 0 | 1 |
| `wgit` | 12 | 12 |
| `wigl` | 36 | 36 |
| `itmc` (engine evidence only) | 0 | 1 |
| `DECR` | 42 | 42 |
| `PRTM` | 49 | 49 |
| `hudg` | 126 | 126 |
| Total | 591 | 678 |

The retail reference supplies no separate plugin for the other object
palettes, including scenery, weapons, equipment, machines, sound scenery
and crates. Those groups inherit the mapped object layer. The vehicle
also supplies the shared unit layer for bipeds. Of the local schema
groups without a definition, `scnr` belongs to the scenario work.
The remaining groups have no plugin in this retail directory:
`<fx>`, `adlg`, `bsdt`, `colo`, `gldf`, `mdlg`, `mulg`, `phys`,
`sfx+`, `sncl`, `snmx`, `spk!`, `styl`, `weat`.

### Disagreements retained in the local layout

A path names a block element or descends by its offset. Numeric paths
use hexadecimal byte offsets. Offsets grouped on one line share the
same decision. A disagreement at a block header applies to its whole
subtree.

#### Vehicles and shared object layers

| Definition and offset | Ours | Retail reference | Reason for retaining ours |
|---|---|---|---|
| obje +0x24 | Three separate words | Three-float point | Crosses our field boundaries. |
| obje +0x70, +0x78, +0x80, +0x88 | Two words at each offset | Two-float ranges | Each range crosses a word boundary. |
| obje attachments +0xc | Signed 16-bit index | 16-bit enumeration | The engine reads the signed index, including -1. |
| obje +0xa4 | Eight opaque bytes | Block with 0x50-byte elements | Empty on examined retail objects. No independently established element stride. |
| obje change_colors.block_0 +0x4, +0x10 and change_colors.block_8 +0x8, +0x14 | Three separate words in each triplet | RGB float triplets | The converter populates the three components independently by their word paths. |
| unit +0xd8 | Four opaque bytes | String ID | A string would change encoding and fixup behaviour; our structural schema has no string there. |
| unit +0xe0, +0xf0 and block_198 +0x4 | Two, three and three words respectively | Angular range and two three-float vectors | These fields cross our word boundaries. |
| unit block_1a8 +0x4 | Two words | Tag reference | The reference straddles our fields and lacks a matching schema fixup. |
| vehi block_1c8 +0x24; +0x44, +0x4c, +0x6c, +0x88 | Three words; four pairs of words | Three-float vector; two float ranges and two angular ranges | They straddle our fields. The converter writes the camera angle bounds as individual words. |
| vehi +0x1ec | Unsigned 32-bit vehicle flags | 32-bit flag set | The vehicle builder uses the existing integer representation. |
| vehi block_2f8 +0x70, +0x80, +0x90, +0xa0, +0xb0, +0xc0, +0xd0, +0xe0 | Three words at each offset | Three-float sphere positions | These span separate fields. The vehicle builder writes each coordinate by word offset. |

#### Images, shaders and animations

| Group and offset path | Our representation | Retail reference | Reason retained |
|---|---|---|---|
| bitm 3c/0..1f, 3c/34/18..1f | Several separate observed byte spans | A 32-byte string, an XY float pair respectively | Each crosses existing field boundaries. |
| bitm 44/0 | u32 signature | Four ASCII bytes | Existing builder writes the group word. |
| bitm 44/(4,6,8,14,34) | Unsigned width, height, depth, mip count and LOD size | Signed integers of the same widths | Image decoding and layout checks use unsigned quantities. |
| bitm 44/c | u16 format code | enum16 | Pixel-format mapping uses the numeric code. |
| bitm 44/(1c,20,24,38,3c) | Three raw resource locations and two linked raw sizes | Signed 32-bit integers | Relocation needs the resource relationships. |
| jmad c/(a,c,10) | u16 flags, u32 flags, then opaque bytes through +20 | Two flag bytes, a three-float vector extending from +c into the opaque bytes, then two floats | Existing node decoder and field boundaries. |
| jmad 14/8, 1c/8, 2c/11, 2c/4c/0 | u16 sound/effect flags, u8 frame-info code and i16 event code | flags16, enum8 and enum16 respectively | Existing event and movement decoders use numeric codes. |
| jmad 2c/4, 2c/(16,24,28) | A schema word, a u16 flag word, a u32 resource index and a u32 byte offset | i32, two flag bytes, two i16s and an i32 respectively | Engine fetch and resource lookup establish the field boundaries. |
| jmad 2c/(34,38,3c,3d,42,48) | Unsigned size and address fields (32, 32, 8, 8, 16, 32 bits) | The first two words unspecified, remaining sizes signed | Animation stream decoder sizes buffers using these fields. |
| jmad 2c/64/(4,8,c,10,14) | Five separate opaque words | Packed quaternion across two words, translation across three | Neither fits one observed span. |
| jmad 34/4/4/24/0 and 34/4/4/2c/0 | u32 resource index | i32 | Existing preload index type. |
| jmad 88..ab | One zero-filled span | Five mask words, a data reference and a block header | Neither pointer is in our relocation schema. |
| jmad ac/(4,8,c,10) | A raw size linked to the resource location, followed by two unsigned words | Signed size, ordinary unsigned offset, two signed words | Resource relocator and runtime own these words. |
| jmad b4/(0,8) | Four packed i16 rotation components, then a three-component translation | Two individual floats followed by three individual floats | Retail pose decoder reads a packed quaternion and a translation vector. |
| shad c/(34,3c) | Specular texture and colour | Translucency texture and lightmap transparency colour | Existing names have evidence from the corresponding bitmap paths and the observed nonzero colours. |
| shad 18,4c | Two separate eight-byte padding regions | Block headers with 0x28- and 0x4-byte elements respectively | Neither header appears in the retail relocation schema. |
| shad 20/c/0, 20/14/0 | A packed u32 colour and four float components | A colour type and a three-float vector plus one float | Existing draw path packs pixel constants and copies four vertex components. |
| shad 20/1c/(0,4), 20/24/0, 20/2c/0, 20/34/(0,2,4,6,8), 20/4c/0 | Unsigned mask and packed range indices | Signed integers of matching widths | Engine extracts indices and counts from the packed words. |
| shad 20/54/(0,1,2,3) | Four separately read unsigned bytes | A signed 16-bit field followed by two signed bytes | Existing animation bindings address each byte. |
| shad 2c/2, 3e, 40 | Unsigned image index and two integer shader inputs | Signed prediction index and two enum selectors | Existing image occurrence check and engine integer loads. |

#### Structure BSP

| Group and path | Our layout | Retail reference | Reason |
|---|---|---|---|
| sbsp `c, 5c, 9c, a4, c4/18, c4/38, c4/6c/18, 104, 138, 1d0/48/20, 234/10` | Block strides `8, 3, 19, 17, 3, 2, 6, 8, 11, 11, 3` | Strides `14, 24, b0, 20, 14, 14, 44, 68, c8, 20, 2c` | A stride change moves every element after the first. The engine readers cited in sbsp.txt independently use larger strides at 9c, c4/18 and 138. Correcting the schema is separate work. |
| sbsp `14/{0,8,10,18,20,28,30,38}, 2c, 4c, c4/{0,8,10,28,40}, 224/{24,2c}` | Data buffers | Blocks with strides `8,10,4,4,10,8,c,10; 8,8; 8,10,4,c,4; 1,1` | Our relocation and buffer readers keep their established representation. |
| sbsp `8, c4/48` | Unsigned 32-bit checksum | Signed 32-bit value | Preserve the existing checksum interpretation. |
| sbsp `8c/10/{0,c}` | Three floats for a normal and one for distance | One 16-byte plane | Keep the plane components that the weather-volume reader consumes. |
| sbsp `ec/{0,10,14}` | Unknown 0x10, raw word, unknown 0x28 | A 32-byte string crossing both boundaries, followed by rotation and translation | The reference string spans our raw field. |
| sbsp `140/4c, 224/c/0, 234/18/14` | Schema words | Signed 32-bit integers | Preserve the schema's word type. |
| sbsp `214/{10,14,18,1c}` | Raw, raw, raw-size, raw | Signed, unsigned, unsigned and unsigned 32-bit integers | Our resource handling uses these annotations. |
| sbsp `214/{28,2c}` | Word then unknown 0x80 | Eight-byte tag reference starting at 28 and crossing 2c | The reference straddles the two fields. |
| sbsp `84/28, fc/0, 1d8` | Unknown ranges of 0x60, 0x14 and 8 bytes | Tag references at child 4c, child 0 and 8, and root 1d8 respectively | The schema provides no corresponding tag fixup. |
| sbsp `94/0, bc, c4/20, c4/6c/0, c4/6c/20, cc, 1d0/40/0, 1d0/40/38, 1d0/50/38, 214/8` | Unknown ranges of 0x24, 8, 8, 0x10, 8, 8, 0x28, 8, 8 and 8 bytes | Blocks at `94/{0,8,10,18}, bc, c4/20, c4/6c/{0,8,20}, cc, 1d0/40/{20,38}, 1d0/50/38, 214/8` | The schema provides no block fixup at these offsets. |
| sbsp `e4` | Unknown 8 bytes | Data reference | No data fixup at this offset in our schema. |

At root offsets `0, 24, 10c, 118, 120, 128, 130`, two numeric words do not establish a block fixup. Object IDs at `21c/0/0` and `22c/0` lack evidence of a relocatable datum.

#### Interface and shader programs

| Group | Element and offset | Ours | Retail reference | Reason |
|---|---|---|---|---|
| skin | `item_animations` 0x4 | u32 | i32 | Retain the registry field type |
| skin | `text_blocks` 0x4 | u16 | enum16 | Retain the registry field type |
| skin | `text_blocks` 0xa | i16 | enum16 | Retain the registry field type |
| skin | `text_blocks` 0xc | unknown | f32[4] at 0xc | Crosses a definition boundary |
| skin | `text_blocks` 0x10 | f32[3] | f32[4] | Retain existing field boundaries |
| skin | `bitmaps` 0x4 | u16 | enum16 | Retain the registry field type |
| skin | `bitmaps` 0x8 | i16 | enum16 | Retain the registry field type |
| skin | `shader_bitmaps` 0x4 | u16 | enum16 | Retain the registry field type |
| skin | `tables` 0x10 | i8 | enum8 | Retain the registry field type |
| spas | `root` 0x0 | unknown | dataref | No matching registry fixup |
| spas | `root` 0x8 | unknown | tagblock (stride 0x2c) | No matching registry fixup |
| spas | `root` 0x14 | unknown | tagblock (stride 0x74) | No matching registry fixup |
| spas | `postprocess.implementations` 0x0, 0x2, 0x4, 0xf6, 0x11c, 0x11e, 0x120, 0x122, 0x124 | u16 | i16 | Retain the registry field type |
| spas | `postprocess.implementations` 0x6 | u8[0xd6] | i16 + undefined | Retain existing field boundaries |
| spas | `postprocess.implementations` 0xdc | word | undefined | Retain the registry field type |
| spas | `postprocess.implementations` 0xe0 | u8[0x16] | undefined + i16 | Retain existing field boundaries |
| spas | `postprocess.implementations` 0x104, 0x10c | u32[2] | undefined (multiple fields) | Retain existing field boundaries |
| spas | `postprocess.implementations` 0x114 | u32 | undefined | Retain the registry field type |
| spas | `postprocess.texture_bindings` 0x0, 0x1, 0x2 | u8 | i8 | Retain the registry field type |
| spas | `postprocess.texture_bindings` 0x3 | flags8 | i8 | Retain the registry field type |
| spas | `postprocess.render_states` 0x0 | u8 | i8 | Retain the registry field type |
| spas | `postprocess.render_states` 0x1 | u32 | i32 | Retain the registry field type |
| spas | `postprocess.stage_states` 0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6 | u8 | undefined | Retain existing field boundaries |
| spas | `postprocess.stage_states` 0x7 | unknown | undefined at 0x4 | Crosses a definition boundary |
| spas | `postprocess.stage_states` 0x8, 0xc, 0x10 | u32 | undefined | Retain the registry field type |
| spas | `postprocess.stage_states` 0x14 | f32 | undefined | Retain the registry field type |
| spas | `postprocess` 0x20 | block stride 0x4 | block stride 0x3 | The retail registry supplies the stride |
| spas | `postprocess.variants` 0x2 | flags8 | i16 | Retain existing field boundaries |
| spas | `postprocess.variants` 0x3 | unknown | i16 at 0x2 | Crosses a definition boundary |
| spas | `postprocess.variants` 0x4 | u16 | i16 | Retain the registry field type |
| spas | `postprocess.stages` 0x0, 0x4, 0x8, 0xc | u32 | undefined | Retain the registry field type |
| spas | `postprocess.engine_bindings` 0x0 | u8[4] | i16 + i8 | Retain existing field boundaries |
| spas | `postprocess` 0x40, 0x48 | unknown | tagblock (stride 0x7) | No matching registry fixup |
| vrtx | `programs` 0x14 | unknown | dataref | No matching registry fixup |
| wgit | `root` 0x4 | i16 | enum16 | Retain the registry field type |
| wgit | `panes` 0x2 | u16 | enum16 | Retain the registry field type |
| wgit | `panes.buttons` 0x4 | u16 | enum16 | Retain the registry field type |
| wgit | `panes.buttons` 0xa | i16 | enum16 | Retain the registry field type |
| wgit | `panes.buttons` 0xc | unknown | f32[4] at 0xc | Crosses a definition boundary |
| wgit | `panes.buttons` 0x10 | f32[3] | f32[4] | Retain existing field boundaries |
| wgit | `panes.lists` 0x4 | i16 | enum16 | Retain the registry field type |
| wgit | `panes.lists` 0xc | u16 | enum16 | Retain the registry field type |
| wgit | `panes.lists` 0x10 | unknown | tagblock (stride 0x14) | No matching registry fixup |
| wgit | `panes` 0x14, 0x34 | unknown | tagblock (stride 0x28) | No matching registry fixup |
| wgit | `panes.text_blocks` 0x4 | u16 | enum16 | Retain the registry field type |
| wgit | `panes.text_blocks` 0xa | i16 | enum16 | Retain the registry field type |
| wgit | `panes.text_blocks` 0xc | unknown | f32[4] at 0xc | Crosses a definition boundary |
| wgit | `panes.text_blocks` 0x10 | f32[3] | f32[4] | Retain existing field boundaries |
| wgit | `panes.bitmaps` 0x4 | u16 | enum16 | Retain the registry field type |
| wgit | `panes.bitmaps` 0x8 | i16 | enum16 | Retain the registry field type |
| wgit | `panes.bitmaps` 0x2c | unknown | stringid | No matching registry fixup |
| wgit | `panes.model_scenes` 0x4 | u16 | enum16 | Retain the registry field type |
| wgit | `panes.model_scenes` 0x40, 0x44, 0x48 | unknown | stringid | No matching registry fixup |
| wgit | `panes` 0x3c | unknown | tagblock (stride 0x24) | No matching registry fixup |
| wgit | `panes.tables` 0x10 | i8 | enum8 | Retain the registry field type |
| wgit | `root` 0x28 | u16 | enum16 | Retain the registry field type |
| wigl | `animations` 0x4, 0x10, 0x1c | u32 | i32 | Retain the registry field type |
| wigl | `animations.primary_keyframes` 0x8, 0x10 | f32 | f32[3] | Retain existing field boundaries |
| wigl | `animations.primary_keyframes` 0xc | raw | f32[3] | Retain existing field boundaries |
| wigl | `animations` 0x20 | i16 | enum16 | Retain the registry field type |
| wigl | `root` 0x128 | unknown | tagblock (stride 0x18) | No matching registry fixup |
| wigl | `root` 0x150 | unknown | tagref | No matching registry fixup |
| wigl | `root` 0x160 | i16[4] | enum16 (multiple fields) | Retain existing field boundaries |
| vrtx | root +0x0 | opaque 4 bytes | enum16 and a 2-byte unlabelled value | Vertex program builders write this field as one byte array. |

#### HUD globals

| Group | Path/offset | Ours | Reference | Reason |
|---|---|---|---|---|
| hudg | root +0x48, +0x50 | unknown bytes | tag references | The retail schema has no fixups for these font slots. |
| hudg | root +0x84, +0x94, +0xc4 | unknown bytes | tag references | The retail schema has no fixups for these message resources. |
| hudg | root +0xa4 | word | packed colour | The schema classifies this slot as a word and its relocation behaviour takes precedence. |
| hudg | root +0x380, +0x390 | unknown bytes | tag references | The retail schema has no fixups for the results-screen image and checkpoint audio. |

#### Decorator and particle models

| Group and location | Kept | Reference | Reason |
|---|---|---|---|
| DECR root +0x30, PRTM root +0x98 | Eight opaque bytes | Block with a 4-byte entry | The registry has no block relocation at either address. |
| DECR +0x38/+0x3c, PRTM +0xa0/+0xa4 | Raw resource address and its associated size | Signed offset and unsigned byte count | The schema pairs these fields for resource relocation. |
| DECR resources (+0x48) and PRTM resources (+0xb0), element +0x04 and +0x06 | Two unsigned 16-bit fields | Two signed 16-bit fields | Engine reads in the shared `block_resource` definition in mode.txt give their offsets, sizes and sentinel. |
| PRTM root +0x60 and +0x68 | Two opaque 8-byte regions (one 0x10 span) | Two tag references | Neither is a group/datum relocation in the schema. |
| PRTM +0x78/+0x30 (emitter element) +0x00 | Eight opaque bytes | Tag reference | No tag relocation here in the PRTM schema. |
| PRTM root +0x20, +0x30, +0x40, +0x50; emitter +0x08, +0x18, +0x28, +0x38, +0x48, +0x58, +0x68, +0x7c, +0x8c; at each record's +0, +2, +4, +6 | i16, i16, u16, i16 in the shared `property` | Four 16-bit enumerations | The established effect function uses signed inputs and a numeric modifier. Its data relocation at +8 agrees. |

To repeat the comparison, set the directory for the ignored test as described in [switches.md](../../runtime/switches.md#tools) and run:

```sh
cargo test -p tag-names every_plugin_merges_against_our_definitions -- --ignored --nocapture
```

## 2026-11-21: which sources may be read, and how

Commit: the branch that adds `crates/formats/tag-names`. Method: each
project's public repository page and licence, read on this date; no
file from either source is in this repository.

| Source | Where | Licence | Used |
|---|---|---|---|
| This repository's definitions, `crates/formats/tags/src/defs/*.txt` | here | MIT or Apache-2.0 | field names and `doc` text written here, each with its evidence |
| Assembly's plugins, folder `Halo2Xbox` | <https://github.com/XboxChaos/Assembly>, `src/Assembly/Plugins/Halo2Xbox` | GPL-3.0 (the repository's licence; the plugin files carry no licence line of their own) | read at run time from the user's own copy; names, types, enum options, bit names and comments are shown in the inspector and never written into this repository |
| Halo 2 editing kit for MCC | the user's install of the kit | the kit's end-user licence | read at run time from the user's install; the reader is not built |

What follows from the licences:

- GPL-3.0 text cannot be relicensed as MIT or Apache-2.0, so no plugin
  file, and no list of names or comments taken from one, is committed.
- A name in `defs/*.txt` is ours when the field's evidence is in this
  repository. A name is never copied from a plugin to save writing the
  evidence.
- Tests use invented plugins. A test that reads a real plugin folder is
  `#[ignore]`d and prints to the terminal only.

Folders of Assembly's plugin tree that are not read: every folder but
`Halo2Xbox`. The tree also holds plugins for pre-release builds, which
this project does not use; the reader refuses them by name
(`tag_names::assembly::REFUSED`).

The plugin format, as read from one `Halo2Xbox` plugin: a `plugin`
root with `game` and `baseSize`; one element per field, named by type
(`int16`, `enum16`, `undefined`, `dataref`, `tagblock` and others),
with `name`, `offset` and `visible`; `tagblock` carries `elementSize`
and nests its fields; an enum nests `option` elements with `name` and
`value`; a `revisions` element lists authors.

To reproduce: open the repository page and its licence; open one
plugin of the folder.

Where our definitions and a plugin disagree, the difference is listed
by `tag.names` and by the opt-in test
`every_plugin_merges_against_our_definitions`; settled differences are
added below with the evidence that settled them.
