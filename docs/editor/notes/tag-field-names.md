# Tag field names: sources and licences

Evidence for [polish-plan.md](../polish-plan.md), "Field names and
descriptions".

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
files. The comparison for these mappings covers ten existing group definitions. The scenario
definition is maintained separately.

In a definition's evidence, `A Halo2Xbox <filename>` identifies an
observation from that reference. It can accompany an engine or disc
citation where they establish different facts. New labels use our
snake_case spelling. Descriptions are single-line sentences written
for the editor. New scalar fields tile an entire existing opaque span.
Shared layers contribute their fields to every definition that embeds
them. A converter that addresses an opaque field by its byte-valued
path keeps that path and representation.

### Mapped declarations

Counts are new names and authored descriptions, once at the declaration.
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
| Total | 374 | 461 |

The retail reference supplies no separate plugin for the other object
palettes, including scenery, weapons, equipment, machines, sound scenery
and crates. Those groups inherit the mapped object layer. The vehicle
also supplies the shared unit layer for bipeds. Of the local schema
groups without a definition, `scnr` belongs to the scenario work.
Retail plugins also exist for `DECR`, `PRTM` and `hudg`. The other
groups have no plugin in this retail directory:
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
| unit +0xd8 | Four opaque bytes | String ID | A string would change encoding and fixup behavior; our structural schema has no string there. |
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
