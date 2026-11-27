# Rich editors: shaders, colours, curves, scripts, sounds and recordings

Status: Design, except the seam (X1), "Data that changes length", sound audition (A1), mouth curves, read (A2), mouth curves, write (A4), Scripts, read (H1), Recordings, read (K1), Recordings, play (K2), Recordings, write (K3), Scripts, write (H3), the curve editor, same length (F1), curve type changes (F2), Model poses (M1), the colour picker (C1) and the colour forms of `obje`, `deca`, `lens` and `sky ` (C2), the Shader tab (S1), shader animation (S2), shader code, read (S4) and WGSL overrides (S5), which are built; each section names the format code it stands on and the gaps in it.
Code: `crates/apps/studio/src/fields.rs` (the panel the rich views join), `crates/apps/studio/src/fields/rich.rs` (the seam), `crates/formats/tags/src/defs/attrs.rs` (`FieldView`), `crates/apps/studio/src/app/tag_doc.rs` (`TagDoc`, the only write path), `crates/tools/scenario-edit/src/scenario/data.rs` (the scenario's data that changes length), `crates/apps/studio/src/fields/colour.rs` and `crates/apps/studio/src/api/colours.rs` (C1), `crates/apps/studio/src/fields/curve.rs` and `crates/apps/studio/src/api/curves.rs` (F1), `crates/formats/tags/src/shader.rs`, `crates/formats/tags/src/effects/function.rs`, `crates/formats/hs/src/source.rs`, `crates/formats/hs/src/script_doc.rs`, `crates/formats/hs/src/recording.rs`, `crates/formats/hs/src/recording_edit.rs`, `crates/apps/studio/src/app/recordings.rs`, `crates/apps/studio/src/api/recordings.rs`, `crates/apps/studio/src/app/scripts.rs`, `crates/apps/studio/src/app/script_edit.rs`, `crates/formats/hs/src/table_writes.rs`, `crates/apps/studio/src/ui/scripts.rs`, `crates/apps/studio/src/api/scripts.rs`, `crates/tools/h1campaign/src/recordings.rs`, `crates/formats/tags/src/sound.rs`, `crates/apps/studio/src/app/audition.rs`, `crates/apps/studio/src/app/shader.rs`, `crates/apps/studio/src/fields/shader.rs`, `crates/apps/studio/src/api/shader.rs` and `crates/tools/scenario-edit/src/native_view/preview.rs` (`material`, S1) and `crates/tools/scenario-edit/src/native_view/code.rs` (`pass_code`, S4). `linked_code`, `check_override`, S4, S5) and `crates/apps/studio/src/app/shader_override.rs` (S5). S4) S1). and `crates/tools/scenario-edit/src/native_view/animate.rs` (`value_at`, S2). Tests: `defs/tests.rs`, `registry/tests.rs`, `fields::rich::tests` and `api/tests.rs` (X1), `effects::function::tests` and `api/curves/tests.rs` (F1), `app/tag_data_tests.rs` and scenario-edit's `data_tests.rs` (X2), `app/audition_tests.rs` and `api/sound/tests.rs` (A1), `app::scripts::tests`, `app/script_edit_tests.rs`, `hs/src/table_writes_tests.rs` and scenario-edit's `the_script_tables_writes_save_with_the_scenario` (H3), `hs::script_doc::tests` and `hs::source::each_tests` (H1), `hs::recording::tests`, `hs::recording_edit::tests` (`recording_edit_tests.rs`) and `app/recordings_tests.rs` (K1, K3), `fields/colour/tests.rs` and `api/colours/tests.rs` (C1), `scenario-edit`'s `the_recordings_blocks_writes_save_with_the_scenario` (K3), `app/shader_tests.rs`, `api/tests/shader_tests.rs` and `native_view::tests` (S1), `native_view::code::tests` and `shader_code_lists_both_programs_of_a_pass` (S4); `app/shader_override_tests.rs`, `api/tests/shader_tests.rs` (`shader_override_*`, `shader_push_*`) and `play/tests/shader_override.rs` (S5); (S1); `native_view::animate::tests` and `the_preview_clock_is_read_and_checked` (S2); each other slice names its own, `crates/apps/studio/src/app/anim.rs`, `crates/apps/studio/src/ui/anim.rs` (M1), `api/anim/tests.rs`, `anim_tests.rs`.
Switch: none.

A rich editor shows a tag, or part of one, as the thing it is: a material
with a preview, a colour with a picker, a curve with handles, a script as
source, a line of dialogue with its waveform and mouth track. It is a
second view of the same document as the field panel
([tag-editing.md](tag-editing.md)), never a second document. The one rule
that must hold: every byte a rich editor changes goes through `TagDoc` or
the Scenario session as an ordinary edit, so undo, save, packages, the
Pending tab and `tag.fields` see it exactly as if the fields had been
typed, and anything the editor cannot write it refuses by name.

The editor is Dear ImGui (`dear_imgui_rs`) on wgpu, native Rust
([editor.md](editor.md)).

## How it works

### One seam for every rich view

`fields::rich` (`crates/apps/studio/src/fields/rich.rs`) holds the
seam: the tag-view table and the tab logic. A field view is the
`view=` attribute the definitions parse (`tags::defs::FieldView`, carried
as `registry::Field::view`). A view is one of two shapes:

| Shape | Attached by | Shown as | Examples |
|---|---|---|---|
| Field view | a `view=<word>` attribute on the field's definition line (below) | the row's own widget; a button on the row opens a dialog for the full editor | colour, function curve, mouth curve |
| Tag view | the tag's group | a tab beside **Fields** in the tag's panel, and a window of its own from **Open alone** | Shader, Sound, Scripts, Recordings |

`view=` joins the definition attributes of tag-editing.md's slice T4
(`label`, `unit`, `axes`, `title`, `applies`). Its word list is closed and
the parser refuses any other word: `rgb`, `argb`, `rgb8`, `argb8`,
`xrgb8`, `hdr_rgb`, `function`. Each line carries its evidence as a name
does ([tag-definitions.md](../formats/tag-definitions.md)).

`tag.fields` rows carry the field's word in `view` (null for a field with
none), and `tag.view` gives `tabs` and `tab` and takes `tab`. The parser
refuses a word outside the list, and a word on a type it does not fit:
`rgb` and `hdr_rgb` need `f32[3]`, `argb` `f32[4]`, the three packed forms
a `u32`, `function` a `data` reference.

A tag view is one line in `fields::rich::TAG_VIEWS` (`id`, `label`,
`groups`, `draw`). A tag whose group has a view shows **Fields** and the
view's tab as a row of buttons over the panel (`rich::tab_bar`, in
`ui/tags.rs`); the chosen tab is `fields::Settings::tab`, not saved, and a
tab the selected tag's group lacks shows **Fields**. A group with no view
has no bar. The table is empty until a slice that adds a view.

A view reads and writes through three things only:

- `TagDoc::read` and `TagDoc::write(TagWrite)` for fixed-length bytes,
  `TagAction::Seal` to end a drag as one undo step;
- `TagDoc::block_op` for list changes, under `TagDoc::block_refusal`;
- `TagDoc::resize_data` (`TagAction::ResizeData`) for a data reference whose length
  changes.

The Scripts and Recordings views edit the scenario tag, which
`tag_doc::SCENARIO_OWNED` gives to the Scenario tool. They go through new
operations on the scenario session (`scenario_edit::edit::Op`), not
through `TagDoc`; a data reference there changes length through
`Session::resize_data` (below).

A row that has a field view keeps its plain form: **Show as fields** on
the dialog and `tag.fields` always give the underlying values. A tag view
that cannot model a tag (a layout it does not recognise, a refused
decode) shows the reason and a **Show fields** button; it never draws a
partial picture as if it were whole.

### Data that changes length

`TagDoc::resize_data` (`crates/apps/studio/src/app/tag_doc.rs`) gives the
data reference at a named `Kind::Data` field a payload of any length,
as one undo step. It follows the rule grown blocks already use, treating
the reference as a block of one-byte elements:

- A payload no longer than the source's, or than the room an earlier
  growth made, is written in place and the size field patched; the tail
  stays as dead bytes.
- A longer payload gets a provisional address past the end of the
  metadata. On save it is appended through `MapEditor::append_metadata`
  and its `(size, address)` pair patched, carried in the save's change
  list as a `scenario_edit::TagGrown` (`root` the pair, `count` the
  length). The old bytes stay dead.
- It is allowed only while **Edit > Allow growing full blocks** is on
  (`DATA_GROWTH_REFUSAL`). That the metadata ends the file and the cache
  size limit hold is checked by the save, as for block growth
  ([scenario-editor.md](scenario-editor.md) §6).
- A data reference in a BSP region (`DATA_BSP_REFUSAL`), in a tag with
  no data in the map, in a browsed map, or inside a grown block's
  elements (`DATA_NESTED_GROWTH`) is refused. `TagDoc::data_refusal`
  gives the reason without writing.

With a game connected, a resized data reference has the `reload` effect.

The scenario tag belongs to the Scenario tool, so its data references
change length through the session's form, `Session::resize_data(pair,
bytes, label)` (`crates/tools/scenario-edit/src/session.rs`, over
`scenario/data.rs`). `pair` is the virtual address of the reference's
`(size, address)` pair. The edit is one undo step in the session's
history (`edit::Op::Data`), which keeps each edited reference's whole
payload; `Scenario::changes` turns it into the save's writes by the same
rule:

- A payload no longer than the source's, whose source lies in the
  scenario's payload, is written over the source bytes and the size
  patched; the tail stays as dead bytes.
- A longer payload, or any change to data outside the payload (one an
  earlier save appended), is a `TagGrown` at the pair: appended to the
  metadata on save, the pair pointed at the copy, the old bytes left
  unreferenced. `Session::data_refusal` refuses it while **Allow growing
  full blocks** is off (`DATA_GROWTH_REFUSAL`).
- A pair outside the scenario's payload (`DATA_OUTSIDE`), a pair or data
  inside a block the Scenario tool edits element by element or a block's
  root pair (`DATA_OWNED`), and data that runs past the payload's end
  (`DATA_RUNS_PAST`) are refused.
- Giving a reference its source bytes again is no change; undo puts back
  the payload the step replaced, and with it the pair.

### Colours

**What the user does.** Every colour field shows a swatch and its
stored numbers (three or four floats, or the packed word in hex), and a
click on the swatch opens the picker dialog: a saturation and value
square, a hue bar, an alpha bar over a checkerboard when the field has
alpha, RGB, HSV and hex entry, the value in the file beside the new one
(the picker's reference colour, and both as `#aarrggbb` under it), a row
of recent colours and a saved palette (**Save colour**, **Remove from
saved**; a click on either row applies that colour). The swatch's
right-click menu has **Copy** and **Paste**, which carry a `#aarrggbb`
string between any two colour fields, converting form: a form without
alpha drops it, and an `hdr_rgb` field keeps its intensity. A drag is one
undo step and puts the colour at the head of the recent row. The recent
colours (12) and the saved palette (24) are `fields::Settings`, saved
with the layout as `RecentColour=` and `SavedColour=` lines of the
`[Studio][Tags]` ini section; the copied colour is not saved.

The code is `fields::colour` (`crates/apps/studio/src/fields/colour.rs`):
`form` (a field's form), `decode` and `encode` (stored bytes and the
picker's colour), `held` (a colour as the form holds it), `pick` and
`finish` (the dialog's writes and the seal that ends a drag), and
`from_json` (the values `tag.set` takes).

**The data.** The definitions have no colour type. A field is a colour
when it has a colour `view=` word, or when it is an array of three
floats whose name contains `colour` or `color` (`registry::Hint::Colour`,
set by `registry::hint`), which is shown as the `rgb` form. Either way
its row reads `Colour` and draws the swatch. The `view=` words name the
stored form:

| Word | Stored as | Picker |
|---|---|---|
| `rgb` | `f32[3]`, 0 to 1 | no alpha |
| `argb` | `f32[4]`, alpha first | alpha bar |
| `rgb8`, `xrgb8` | packed 32-bit, top byte unused | no alpha; the top byte is kept as it is |
| `argb8` | packed 32-bit, alpha in the top byte | alpha bar |
| `hdr_rgb` | `f32[3]`, values above 1 allowed | an **Intensity** number beside the picker; the swatch shows the colour at intensity 1 with a `×2.5` mark |

A byte form shows the value after quantisation, so the picker never
shows a colour the field cannot hold. The picker shows stored values and
does no colour-space conversion (**Design**). The name match stays for
three-float fields that have no `view=`, as built. A field gets a word
only with evidence of its form: the engine's reader (**E**), a disc check
(**V**) or the retail reference of
[notes/tag-field-names.md](notes/tag-field-names.md).

**Gradients.** Where a tag holds a colour that changes over an input, the
row is a gradient strip: the colour function sampled left to right,
opening the curve editor below with the strip under the plot. The reader (**E**, the engine's colour
property at `0x0013bc00`, `docs/engine/object-update.md`) is
`Function::output_colour`: the flags' high nibble picks grey, one to four
packed ARGB stops (the two range words and the two words of header
`+0xc`) or white, blended in 14-bit fractions. `function.colours` returns
the mode, stops and a sampled strip ([agent-api.md](agent-api.md)). The
header does not say a function is a colour, so a field's `view=` will
name it; until a definition does, colour functions show as scalar curves
with the sentence `The colours this curve blends are not read yet.`

**Preview.** None of its own. A colour change redraws whatever shows the
tag: the Shader preview, the viewport, and a connected game when the
field's effect is known.

**Write.** Fixed-length, in place, through `TagDoc::write` with the
field panel's merge key, sealed when the mouse is let go. A pick from the
recent or saved row, a paste and a `tag.set` are one undo step each.
Nothing is refused beyond the tag's own read-only reasons. `tag.set`
takes `"#aarrggbb"`, `"#rrggbb"` or `{rgba, intensity?}` on a colour
field and refuses channels outside 0 to 1, an intensity below 0, and an
intensity on a form other than `hdr_rgb`; numbers still set the stored
values as for any field.

**Forms on the definitions (C2).** `view=rgb` is on `obje`'s
`change_colors` bounds (both blocks: the retail reference's RGB float
triplets, and the engine's interpolation at `0xbe9a7` for the functions
block), `deca`'s `colour_lower` and `colour_upper` (**V** 0 to 1),
`lens` `reflections[].tint_colour` (**V** 0 to 1) and `sky `
`fog_a[].color` (**E** `0x001301c0` clamps it to 0 to 1). The `defs`
suite checks that every `view=` line cites **E**, **V** or the retail
reference (`builtin_view_lines_cite_their_source`). No packed or
four-float colour has a word yet: `hudg`'s packed colours and `jpt!`'s
four floats wait for the engine read of their byte order.

### Curves (function data)

**What the user does.** A function field shows a small plot of its curve.
The dialog shows the plot large with the input along the bottom, the
output range at the side, both sets when the function is ranged and a
slider for the range input that blends them. The type is a row that opens
a picker (Constant, Transition, Periodic, Linear, Linear key, Spline,
Spline 2, Exponent). Each type has its own handles: end values and the
easing shape for a transition; wave, frequency, phase, minimum and
maximum for a periodic; the four keys for a linear key. Shape and wave
are picker rows.

**The data.** `crates/formats/tags/src/effects/function.rs`:
`Function::parse` and `Function::to_bytes` round-trip byte for byte, and
`Function::evaluate` with `Tables` ports the engine's evaluator
(`0x0013b390`, **E**), so the plot is the game's curve, not an
approximation. `Property::evaluate` maps the result onto the header
range. Function fields are data references named `function*` in the
definitions (`effe`, `ligh`, `lens`, `prt3`, `shad`, `pmov`, `obje`);
`view=function` marks them. Types 6 and 8 have no evaluator in the
engine and an unknown type has no layout, so all three are shown
read-only with `The game evaluates this type as 0.` The per-type payload
has typed accessors: `Function::set_values` reads a set's own words
(`Set`, named as the editor's handles) and `Function::write_set` writes
them, keeping every word the type does not name, so a typed edit
round-trips through `to_bytes`. The row's small plot and the dialog are
`fields/curve.rs`; the plot samples `Function::evaluate` over `[0, 1]`
(the two sets' curves faint when ranged, a wave without its engine
tables says so), and every write is a same-length byte write at the
data's start through `TagDoc::write`, a drag one undo step. The agent
API's `function.*` (`api/curves.rs`) reads and writes the same blob
through the Tags tool's own command; `function.sample` returns
`Function::evaluate`'s own values.

**Write.** Same type: same length, in place. A type change alters the
length (`FunctionType::blob_len`): `Function::with_kind` rebuilds the
blob at the new length, keeping the flags, the output range and the
header's unread bytes and giving each set a plain default (a transition
from 0 to 1, a periodic cosine of one cycle, a line of slope 1, a ramp,
a cubic that is `t`, an exponent that is `x`, a constant of 0), and the
picker and `function.set`'s `type` write it through
`TagDoc::resize_data` as one undo step. A longer blob needs growth on
("Data that changes length", above) and is refused by name
when it is off; a type the game evaluates as 0 is not offered and is
refused by name. Turning **Ranged** on or off keeps the length (both sets
are always present).

### Shaders

**What the user does.** Opening a `shad` tag shows a **Shader** tab
(`fields::shader`, a view of `fields::rich::TAG_VIEWS`): the header, the
preview, then the parameters in four folding sections.

- **Bitmaps**: one row per slot with a thumbnail, the bitmap's name,
  **Open** and **Change...**. **Change...** opens a bitmap picker dialog:
  a search, **No bitmap**, then the map's `bitm` tags, each with its
  thumbnail (decoded a few a frame as rows scroll into view). A pick is
  `tag.set`'s write at the slot's path, one undo step.
- **Colours**: the pixel constants, each an `argb8` colour row
  (`shad.txt` names the view); the swatch opens the colour dialog of
  "Colours".
- **Vectors**: the vertex constants, each four numbers, with **Edit in
  Fields**, which shows the field in the Fields tab. The tab does not edit
  them itself.
- **Animation** has one row per animated
  property with its input (an object function or game time and its
  period), its curve as a small plot with the clock's place marked and
  **Edit curve...** (the curve dialog of "Curves", writing the property's
  `function` data), its value at the clock, and what it drives. A
  property driven by an object function, or with a period that is not
  positive, says why it has no value. A periodic wave that needs an engine
  lookup table names the wave (`native_view::animate`). A property
  that animates a bitmap frame, render state or stage state names the
  refusal. Texture-slot transform properties can feed vertex constants.

A header names the template (`stem`) with **Open**, the level of detail
shown (0, the one a near object draws), why the tag is read-only when it
is, and what did not read: a binding to a slot the shader lacks is named
(`the template binds colour 0, which the shader does not have`). Each
parameter row has a `Used by` line from the template's binding
(`app::shader`): which pass, which implementation, and which texture
stage, combiner constant (`final combiner c0`, `combiner stage 3 c1`),
vertex default (`vN`) or vertex constant (`c[18+N]`) reads it. A slot no
binding reads at that level of detail says so.

The tab and the API read the shown map with the document's edits laid
over it (`App::open_shown_map_edited`, `tag_browse::Patched`), so an edit
shows in the read and the preview at once. A block the document has
grown is not in that overlay; the read shows the map's own.

**The data.** `crates/formats/tags/src/shader.rs`, decode only:
`ShaderInstance` and `ShaderPostprocess` (`bitmaps`, `colors`, `vectors`,
`properties`, `animations`, `lods`), `ShaderTemplate`,
`TemplatePostprocess`, `ParameterBinding`, `ShaderPass`, `PassPostprocess`;
`tags::material::ShaderLibrary::material` resolves an instance to
`ResolvedPass` values. The fields the editor writes are named in
`crates/formats/tags/src/defs/shad.txt`, and the bitmap, colour and vector
slots carry `applies=frame`. Gaps:

- **Parameter names.** Slots bind by index (`ParameterBinding::source`).
  The template's properties and categories blocks (`stem` root `+0x10`,
  `+0x18`, per Assembly's plugin) are not read, and a retail cache may keep
  no names in them ([shader-instances.md](../formats/shader-instances.md),
  "Parameter names"). Until slice S3 reads them, a slot is labelled by its
  kind, index and `Used by` line (`Bitmap 2 · stage 1 of pass 3`), and
  the tab says once `This template's parameter names are not read yet.`
- **`pixl`, `slit`.** No code parses either. The light response
  reference shows as a plain reference row.
- **`vrtx`.** A definition only (`defs/vrtx.txt`); no typed struct.

**Preview.** The editor's native view: `scenario_edit::native_view` on
`native_render`, the game's own material path (`layers_load::plan_without_game`,
`program::compile`). `native_view::preview::material` builds a world of
one section whose material is the shader, with no lightmap, and draws it
through `NativeWorld` as the viewport's Game mode does, unlit: the base
look, then each layer pass the plan accepts. The shape is a sphere (the
default), a flat square, or an object tag's model in its rest pose,
chosen in a picker dialog under the preview. An edit re-plans the one
material and shows on the next frame; the Scene viewport shows the same
change on the map. **Play**, **Pause** and the slider above the preview
set the clock in seconds. It loops over the longest period. Playing or
scrubbing updates the drawn layer passes' pixel and vertex constants
from the same clock that drives the curve readout. The tab keeps the
geometry, programs and textures on the GPU between samples.
`preview::material_at` and `shader.preview` take a time for a single
picture. The preview lists the native view's refusals, including
layer 1 (`native_view::LAYER_1_UNCHECKED`), bitmap-frame, render-state
and stage-state animations and object-function inputs
(`native_render::layer_pass::LayerRefusal`), and the registers named in
`program::CAMERA_GIVEN`. A refused pass is left out and counted, never
replaced by a flat colour. With a game connected in Edit, the same
writes reach it through `edit.batch` and show on its next frame
([live-mirror.md](live-mirror.md), "How an edit applies"). This preview
needs the native view ([editor.md](editor.md), "Lighting").

**Write.**

| Edit | How | Refused when |
|---|---|---|
| A bitmap, a colour, a vector | fixed-length, in place | the tag is read-only (shared cache, browsed map) |
| A curve of an animated property | as "Curves" | as "Curves" |
| Animate a parameter that is not animated, or stop animating one | not written | always, until the postprocess block is rebuilt whole: `Adding an animation changes the shader's compiled tables, which the editor does not rebuild.` |
| Change the template | the reference, in place | the new template's bitmap, colour and vector counts or its bindings differ from the old one's (`TemplatePostprocess` compared), with the differing counts in the message |
| A new shader | **Duplicate tag** is not a tag-level operation the map writer has | always; named |

Most retail shaders, templates and passes have their data in a shared
cache, which the editor shows and does not save ([editor.md](editor.md),
"Browsing other maps and where tags live"). The tab says so in its header
and stays a viewer with a working preview.

**Shader code.** Three levels, in order of cost:

1. **Read it** (built, slice S4; `native_view::code`). A **Code** section
   per pass (read only, implementation 0 of the pass; `shader.code` takes
   any) shows the vertex program's disassembly (`vsh::disassemble`), the combiner listing
   (`psh::disassemble`), the translated WGSL (`vsh::wgsl::translate`,
   `psh::wgsl::translate`) and the translator's warnings.
2. **Override the translation** (built, slice S5; `app::shader_override`).
   The game replaces a translated program with a WGSL file from a watched
   directory (`crates/platform/hot-render`, `render.dump`, `render.reload`,
   [hot-iteration.md](../graphics/hot-iteration.md)). Each pass's Code
   section has an **Override** box. Its text starts from the pass's vertex
   and pixel programs linked into one module (`code::linked_code`:
   `shaders::link` with default options, `vs_main` and `fs_main`), and is
   checked as the game parses it (`code::check_override`: naga, then the
   two entry points), with the line and column of a compile error under the
   text. The game's own program carries the draw's options, so for an exact
   start `render.dump` writes its translation. **Push** writes the text as
   `<program>_studio.wgsl` in the override directory (the game's default,
   on this machine) and sends `render.reload {dir}` on the game link;
   **Revert** deletes the file and reloads, so the program draws its
   translation; **Discard edits** drops the draft. A push shows as queued,
   sent, `loaded` (the game lists the program) or refused with the game's
   words; whether the program takes the text (the game compares its
   interface with the program's, `hot_render::iface::check`) is
   `render.overrides`' `applied`. The box says that the override changes
   how our renderer draws and is never written into a map.
   - *Program id.* The game's id is the pipeline cache's hash of a key with
     draw-time options (`d3d::pipeline_store::program_id`), which the
     editor cannot derive; the user names it from `render.programs`.
   - *Preview.* The editor's own preview draws materials through the
     native renderer, not WGSL, so an override is previewed by the game
     it is pushed to. A push with no game paired, or to a game without
     `render.reload`, is refused by name and writes no file.
   - *Where.* The file and the directory are on this machine; a game on
     another machine does not see them.
3. **Write Xbox programs into the map** (slice S6). Feasible, with
   limits:
   - *Vertex programs.* `vsh::encode` builds microcode from `Inst` values
     and `vsh::decode` reads it back, but nothing parses text. An
     assembler over the disassembler's syntax closes that, tested by
     `assemble(disassemble(p)) == p`. Hard limits: `vsh::MAX_INSTRUCTIONS`
     (136) and `CONSTANT_COUNT` (192). State programs and read/write
     programs have no translation (`TranslateError::StateProgram`), so
     they have no preview and are refused.
   - *Pixel programs.* A combiner definition is a fixed-length record
     held verbatim in the pass (`shader::Implementation`), so a write is
     in place. `PixelShaderDef::decode` gives `Combiners` one way only;
     the slice adds the inverse and a parser for our listing syntax.
     Features the translator drops (`dot_zw`, brdf, unsupported dot
     mappings) are refused on edit because the preview would lie.
   - *Storage.* A vertex program is data in a `vrtx` tag shared by every
     pass that names it; a longer program needs `TagDoc::resize_data`.
     Almost all of them live in the shared cache, where the editor does
     not save, so on a retail map this path mostly refuses until tags can
     be copied into the open map.

### Scripts

**What the user does.** With a map open, the Map tool's **Scripts** tab
lists the scenario's globals (name, type) and scripts (name, kind,
return type), with a search over names and source, and counts the free
node slots against `hs::RETAIL_NODES`. Selecting one shows its source,
decompiled, in the Inspector, with the definitions that use it under
**Used by**. Names in the source are links: an object name selects its
placement in Scene, a trigger volume name selects the volume, a tag the
scripts reference opens in Tags, and a script or global shows its
definition. A link that goes nowhere (no object of that name, two
volumes with one name) is drawn in the warning colour and its tooltip
says why; clicking it logs the same sentence. **Builtins** lists every
builtin of the executable as its call form, `(name <short> <object>)`,
with where the name comes from (below); an unnamed one is
`(#builtin:<index> ...)`. **Script documentation** picks the listing
builtin names may come from, with a file dialog, and says what it
named. **Edit** puts the shown definition's source in a text box; **Compile**
writes it into the map's tables as one undo step, **Check** says what it
would do without writing, and **Undo compile** and **Redo compile** step
through the compiles. A definition the map has is replaced where it is; a
new one is added at the end of its table; no other definition's nodes
change. The result shows under the buttons: the definitions, nodes used,
slots reused and added, new string bytes, free nodes left, and a line
for each problem with its line and column. **Open in VS Code** and **Open
in Zed** write every global and script to a working copy
(`scripts/<map>/<map>.hsc` under the data root, with the `h2hs.json`
context `h2 hs lsp` checks it against) and open it there; each save comes
back to the tab as the definitions that differ from the map's and the
file's problems (`app/external.rs`;
[editor-guide.md, "Scripts in VS Code and Zed"](editor-guide.md#scripts-in-vs-code-and-zed)).
**Compile working copy** compiles the definitions the file changed or
added (the ones it holds unchanged are not recompiled, so their nodes are
not touched). Removing a definition from the file removes nothing from the
map.

**The data.** `crates/formats/hs`: `Scenario::read` decodes the node
table, string data, scripts, globals and tag references (`wire::Node`,
`Script`, `Global`, `Reference`, each with `decode` and `encode`);
`Scenario::decompile` prints Lisp-style source and
`Scenario::decompile_each` prints each global and script alone with the
byte span of every atom and call name (`Token`); `Catalog::from_xbe`
reads the builtin table and value types from the game's executable;
`cast::castable` and `Scenario::validate` hold the type rules
([haloscript.md](../formats/haloscript.md)). The source is a
reconstruction: comments, layout and the original file split are not in
the map.

**Where the tab reads from** (`app/scripts.rs`, `Loaded::read`). The
tables come from the map the scenario session has open; they are read
when the tab is first drawn for a map, or when an API call or **Scripts**
needs them. The executable is the source's own `default.xbe`
(`editor_core::MapSource::executable`): the root of a disc image, or for
a folder of maps or a loose map the file beside it, then the one in the
folder above (an extracted disc's `maps`), in any case. It is read at
run time and never copied or committed. Without it the tab shows
`Scripts need the game's executable to name functions and types.`
followed by every place it looked, and nothing else; an executable that
is not the supported one is refused in the same sentence.

**Links** (`classify`, `Loaded::resolve`). A call's name is a script
link when the call is a script call. A global atom names a scenario
global (external globals are not links). A symbol literal of an object
name type (`object_name` to `scenery_name`) is an object link, resolved
by its text through the scenario's object names block
(`scenario_edit::Names::name`) to the placement block whose object type
the name holds. A symbol of the executable's `trigger_volume` type is a
volume link, resolved by name among the trigger volumes. A symbol whose
value is the id of a tag in the scripts' tag references is a tag link.
Links are resolved against the session's scenario as it is now, so a
removed placement shows as a link that goes nowhere. Other scenario
names (AI, cutscene flags and camera points, starting profiles) are not
links.

**Builtin names** (`hs::Naming`). A builtin is named by the calls in
the open map's scripts (`Catalog::observe`). For the others, the user
may point the tab at a script documentation listing: the `hs_doc` text
of the retail editing kit for Halo 2 on MCC, read from the user's own
copy each time it is chosen or the map opens, never stored, logged or
committed. An entry names a builtin only when the builtin's return type
and parameter types, in order, are the entry's, and the pairing is
unambiguous ([haloscript.md](../formats/haloscript.md), "Builtin names
from a documentation listing"). Every other builtin stays
`#builtin:<index>`; nothing guesses a name. The listing is chosen with
**Choose...**, `h2-editor --hs-doc <file>` or `script.doc`. Projects
built from leaked builds are not a source for any name.

**The compiler** (`hs::compile`, `crates/formats/hs/src/compiler.rs`;
[haloscript.md, "Compiling source"](../formats/haloscript.md#compiling-source)).
It takes source as the decompiler prints it or a person writes it, and
gives a copy of the scenario's tables with those definitions in it, or
every problem as a `Message` with its line, column, span and a stable
code (`syntax.open`, `def.kind`, `call.arity`, `type.cast`,
`name.unresolved`, `nodes.limit`, ...).

| Part | How |
|---|---|
| Parser | the working copy's tree (`ide::syntax`), with byte spans for messages; several body expressions are joined by the map's `begin` |
| Function names | the names the map's own calls spell, then `hs::Naming` (or the catalogue's). A builtin nothing names is called as `(#builtin:<index> ...)`; nothing guesses a name |
| Game globals | names only as the map's scripts already use them |
| Types and casts | a node takes its parent's type: a fixed builtin's parameter, a script's return type, a global's type. Arguments of a builtin that reads its own (flags bit 1) take the types this map's calls of it give that position, else their own. A value of another type needs `cast::castable` |
| Literals | numbers, booleans, `none`, strings and `#real-bits:` by their form; a script by its index; other names (objects, volumes, AI, enums, tags) as this map's scripts already use them, then through the caller's `Options::resolve`; otherwise an error naming the type |
| Slots | the replaced definitions' nodes are freed (those no kept definition reaches); new nodes take free slots first, then new slots, up to `hs::RETAIL_NODES` (or the scenario's own size); past it the compile is refused with both counts |
| Strings | a token the string data holds keeps its offset; a new one is appended |
| Check | the new tables pass `Scenario::validate` or nothing is returned |

The compiler's acceptance is a round trip: for every script,
`compile(decompile(s))` gives nodes equal to the source's in opcode,
type, flags, data and shape (`hs::first_difference`). The synthetic form
is in `compiler/tests.rs`; the disc form is `tests/compile_disc.rs`,
`#[ignore]`d, and reports the share of retail definitions that round-trip
as the evidence that the type rules match retail.

**Preview.** There is no editor-side run. `hs::vm` evaluates against
`vm::mock` only and the game's scripts run on the retail interpreter
([hs_vm.md](../engine/hs_vm.md)). With a game connected the tool shows
live threads and globals beside the source (`hs.threads`, `hs.globals`).
A changed script has the `reload` effect. The expression box under the
search (**Evaluate in game**, `script.eval`) sends one expression to the
paired game's `hs.eval`, which compiles it against the loaded map and runs
it at a tick boundary over a snapshot of the map's globals; the last answer
shows under the box. It does not run builtins that act on the game's
objects, and it writes nothing back
([runtime/agent-api.md](../runtime/agent-api-input.md#hseval)).

**Write** (`crates/apps/studio/src/app/script_edit.rs`,
`crates/formats/hs/src/table_writes.rs`). `App::script_compile(source,
only, apply)` hands the source to `hs::compile` with the tool's catalogue
and builtin names, so a map with one changed script differs only in that
script's nodes. The tables it returns go to `hs::table_writes::writes`,
which says how the save writes each of the five the scenario header
describes (nodes `+0x238`, string data `+0x1b0`, scripts `+0x1b8`,
globals `+0x1c0`, tag references `+0x1c8`):

- A table with no more elements than the map's is written in place: one
  patch per run of changed bytes, and its count when that changed. The old
  nodes of an edited script are freed (salt zero) and new nodes take free
  slots first, so an edit that fits is node patches and nothing else.
- A table with more elements (more nodes than free slots, a new string, a
  new script or global) is written whole to a new place: a
  `scenario_edit::TagGrown` at the header's pair, appended on save and the
  pair pointed at the copy, the old bytes left unreferenced. Existing
  string offsets never move, because new tokens are only appended. The
  node count may not pass `hs::RETAIL_NODES` (or the scenario's own
  size); past it the compile is refused with both counts, in the
  compiler's `nodes.limit` message.
- The session holds the writes beside the scenario's own
  (`Session::set_script_changes`, `Scenario::set_script_changes`): they
  save with the map, count as unsaved, and a grown table is refused while
  **Edit > Allow growing full blocks** is off
  (`SCRIPT_GROWTH_REFUSAL`).
- Before anything is held, the compile passes `Scenario::validate` (in
  `hs::compile`), and the new tables pass `Scenario::validate_graph` and
  the codec's symmetry (`Scenario::verify_written`: written out, read
  back, and every table encodes to the same bytes). A failure leaves the
  document as it was.
- The tool's tables become the compiler's (`Loaded::rebuilt`): the
  source, links, free node count and **Used by** show the edit. The
  working copy's differences are computed again. The undo stack
  (`script_edit::Doc`) is the tables before each compile, and the writes
  are always the diff of the map's tables against the current ones, so
  undoing to the start clears them.
- Names the map's scripts do not already use (an object, a trigger volume,
  AI, an enum) do not resolve: the compile says `name.unresolved`.
  `Options::resolve` is not given the scenario's blocks, because how a
  scenario name's value is encoded in a node is not established (**E**);
  nothing guesses it.
- Removing a script or global is not a compile operation, so a removal
  another script's call depends on cannot happen. A scripted map already
  refuses the removal of a trigger volume
  (`scenario_edit::scenario::is_scripted`); naming the scripts that use
  it in that refusal is not built.

### Sounds and facial animation

**Where the game keeps facial animation.** In the sound, not the
animation graph (**E**, [sounds.md](../halo1/sounds.md)):

- A `snd!` tag indexes the map's sound gestalt (`ugh!`). Each permutation
  holds a signed index of a descriptor in the gestalt's *extra info*
  resource.
- A descriptor is four numbers: mouth offset, mouth length,
  facial-sequence offset, facial-sequence length.
- The **mouth curve** is one byte per 30 Hz tick, the jaw aperture scaled
  by 1/255 (`0x218e50`).
- A speaking unit whose sound class enables lip sync (`0x18c630`) samples
  the curve by audio time and blends frame zero of its `talk` overlay
  from the model animation graph by that weight (`0x1cc150`). The unit's
  `lip_sync_onset_weight` and `lip_sync_release_weight` shape it.
- The **facial sequence** is optional second data in the same resource:
  weight keys per frame of a facial pose animation, of which the engine
  blends the 12 largest as overlays (**E** `0x10de40`, `0x10e530`; slice
  A5, [the note](../formats/notes/2026-11-24-facial-sequences.md)).
  Whether any retail sound has one is not known (**S**). Nothing below
  reads or edits it.

So facial animation in Halo 2 is a jaw track per line of dialogue plus
one authored overlay per character, and an editor for it is a sound
editor with a face preview.

**What the user does.** Opening a `snd!` tag shows a **Sound** tab: pitch
ranges and permutations as a list; for the selected permutation the
waveform, **Play**, a scrub bar, and under the waveform the mouth curve
as a filled track on the same time axis. A **Preview on** row opens a
picker of the map's bipeds; the model draws in the native view with its
`talk` overlay driven by the curve while the sound plays or is scrubbed.
Editing the curve is drawing on the track; **From the sound** fills it
from the sound's loudness envelope, with a gain and a smoothing number
(**Design**: our own generator; it claims nothing about how the retail
curves were made). A `udlg` tag shows its vocalisations as a list that
opens each sound here.

**The data.** `crates/formats/tags/src/sound.rs` (`SoundHeader`,
`Gestalt::read`, `Gestalt::mouth_curve`, `Sound`) reads pitch ranges,
permutations, chunks and the extra-info mouth curves.
Decoders: `audio::adpcm::decode`, and `crates/platform/wma` which needs
codec tables from a verified `default.xbe` (`wma::VerifiedImage`). Gaps:

- the mouth curves, read (slice A2, built): the extra-info index is named
  in `defs/snd_.txt`, the block, its descriptors and its locators in
  `defs/ugh_.txt`, and `Gestalt::mouth_curve` reads a permutation's curve
  out of the resource by its two locators - one aperture byte per 30 Hz
  tick, `None` for an unvoiced permutation (no extra info, a negative
  descriptor index, or a zero-length entry). The Sound window draws the
  selected permutation's curve as a filled track under the waveform on
  the same time axis, and `sound.mouth` reads it; both show it even when
  the audio is not decoded;
- audition (slice A1, built): `crates/apps/studio/src/app/audition.rs`
  decodes a permutation (`audio::adpcm`, or `wma` with tables from the
  source's executable: the disc's `default.xbe`, or the one beside a loose
  map, or `H2_XBE`) and plays it through `audio::output::DeviceSink` on an
  audio thread of its own, the game's own sink. The device opens on the
  first play; a machine without one gets the reason, not silence. The
  **Sound** window (`ui/sound.rs`, floating until the tag view tabs of X1
  exist) lists the permutations, draws the waveform, plays, stops and
  plays from a click on the waveform. A permutation whose compression is
  neither Xbox ADPCM nor a codec stream is refused by name. The curve's
  drawing is slice A4 (below);
- models posed from their animation graphs: slice M1 (built) samples a
  clip with `tags::anim_sample::AnimationClip` and draws the posed node
  matrices in place of `native_view`'s `rest_nodes` (the Animation
  window, `anim.pose`), so the face preview has its model;
- the face preview (slice A3, built): `crates/apps/studio/src/app/anim/face.rs`.
  **Preview on** under the mouth track opens a picker of the map's
  bipeds; the picked one draws with its graph's `talk` overlay, frame
  zero, at the weight of the playhead's tick (its byte over 255, 0 past
  the curve's end), redrawn when the byte changes, so it follows play
  and a click on the waveform or the track. The overlay is looked up as
  the engine looks one up, mode, weapon class and weapon type each exact
  then `any`, from `combat:any:any` (**Design**: a model in the editor
  has no mode or weapon; a converted biped keeps `talk` there). It
  blends over the `idle` action the same lookup finds, at frame zero,
  else the rest pose. The unit's onset and release weights are not
  applied, and a `talk` in an inherited graph is refused by name.
  `sound.face` gives the same pose and picture;
- a WMA sound without the executable is refused: `This sound needs the
  game's executable to decode.` The mouth track still shows.

**Preview.** Audio through the host mixer; the face in the editor's
native view. The game is not needed. Hearing a line in the running game
with the character speaking is a script call and belongs to `hs.eval`, which
runs builtins that need the game's objects only once the guest-backed engine
exists.

**Write.**

| Edit | How | Refused when |
|---|---|---|
| Redraw a curve at its length | the bytes, in place in the map's raw data, through `MapEditor::patch_raw` (modeling.md slice 3) | the sound's data is in a shared cache; the permutation is a split chain (no retail example of its segment timing) |
| Give a curve to a permutation that has none, or change its length | not written | always, until raw data can be appended (`MapEditor::append_raw`, modeling.md slice 7) and the descriptor list grown; named |
| The audio itself | not written | always: replacing samples is an encoder and a gestalt rebuild, outside this design |

A curve's length follows the sound's length, so a redraw never needs
another length.

The write is built (slice A4, `crates/apps/studio/src/app/sound_mouth.rs`).
`Gestalt::mouth_span` gives, beside the curve, where its bytes are: the
extra-info resource's raw pointer and size and the curve's offset in it.
A redraw is a `RawPatch` of exactly those bytes, handed to the map's save
as the session's sound changes (`Scenario::set_sound_changes`, merged with
the other tools' raw patches), so a saved map differs from its source in
the curve's bytes and the checksum only. The edits are kept until the save,
shown by every read (`sound.mouth`, `sound.read`, the track, the face
preview), and each is one step of the sound's document beside the sound's
own writes: **Undo** in the window takes the document's newest step as
Cmd+Z does (`doc.undo` on the sound's tab), while `undo` in the API takes
the last redraw of the shown map; `revert` gives the file's curve back,
also one step ([studio-flow.md](studio-flow.md), "Shared patterns"). A
curve drawn back to the file's values is no edit. Two permutations that
share a descriptor share its bytes, so the last drawing wins for both. A
refusal (`check_writable`) names the cause. On the Sound window, a drag
along the track draws (one stroke is one write), **From the sound** fills
the curve from the decoded audio at the window's **Gain** and
**Smoothing**, and **Revert** and **Undo** sit beside it; the waveform
keeps click-to-play.
**From the sound** is our own generator: the RMS of each 30 Hz tick,
scaled so the loudest is the gain (0 to 4), then smoothed forward (0 to
0.95); it needs the audio decoded, and keeps the stored length. The Halo 1 converter is the one existing writer of
curves and stays the reference for the resource's layout.

### Recordings

"Recordings" here are the scenario's recorded animations: named unit
control streams a script plays on a unit (`recording_play` and its
relatives, identified by position in `h1campaign::scripts::identified`).
The game's films and the input logs are other things, with their own
pages ([film-format.md](../game/film-format.md),
[input-film.md](../game/input-film.md)), and are not edited here.

**What they are worth.** The disc's scenarios leave the block empty
(`defs/recorded_animation.txt`), and no retail script plays one. The
converted Halo 1 campaign carries them
([campaign.md](../halo1/campaign.md)). The editor is for converted and
custom maps; on a retail map it shows `This map has no recordings.`

**What the user does.** The Scripts tab's **Recordings** section lists
the map's recordings (name, length in ticks, event count, or `does not
read`). Selecting one shows its stream and control versions, the scripts
that play it, the unit control state it starts from, and every event as a
line: tick, type name, values. A stream that does not read shows the
reason with the offset of the event that broke it; a map with an empty
block shows `This map has no recordings.`. Without the executable the
list still shows and says that the scripts that play each need it. With
a game paired, **Play on unit** plays the recording on the first
player's unit (`recording.play`) and the Scene viewport draws the path
that unit takes through the live mirror as a line, kept until the next
play (`recording.path`). Stream edits, **Remove** and undo are the API's
(`recording.write`, `recording.remove`, `recording.undo`); the tool has no
controls for them, and the lanes and the scrub bar are not built.

**The data.** The element is `defs/recorded_animation.txt` (name,
version, control version, ticks, the stream). `hs::recording` reads the
block (`read` over a map, `decode` over a scenario payload; the count
and sizes are bounded by `MAX_RECORDINGS` and `MAX_STREAM`, which are
sanity limits, not the engine's) and frames and types a stream:
`walk` frames it into `Event` values and checks payload lengths against
the handler table (24 Halo 2 types), and `decode_stream` gives the
initial state by field and each payload as named values (`Value`:
unsigned, signed or floats, by the field's width), keeping the payload
bytes beside them. `h1campaign::recordings` re-exports the walker and
holds the converter and the evidence for the layout;
`map::recording` and `crates/extensions/game/src/h1_recordings.rs` hold
the converted-campaign extensions. Two readings are by width only: the
eight, twelve and four-byte unit control fields are shown as floats
(throttle two, facing, aiming and looking three) or integers, and the
byte and short deltas (types 7 to 22) show their `channel` (the type's
position in its run) and two values: which control each channel moves
is not read. Only version 4 streams are read. Halo 2's player reads the stream; a Halo 1 stream in a
converted map has already been rewritten to Halo 2's widths.

**Scripts that play them.** A script names a recording with a value of
type `cutscene_recording`: by its spelling, matched without case, or by
a number, the recording's index. `Entry::recordings` collects them from
each script's source, `Loaded::recording_uses` matches them to a row, and
`script.uses {kind: "recording"}` and the `played_by` of `recording.list`
give the result.

**Preview.** A recording holds control input, not positions: where the
unit goes depends on the simulation. The editor does not integrate
movement itself, because a path drawn from a private approximation would
be wrong and look right. The timeline needs no game; the path on the map
needs one. Playing is the game's `recording.play`: it finds the recording
by name in the loaded scenario's block (`scnr +0x110`) and calls the
engine's recording player (`0x1fb360`: EAX the unit, CX the index, the
play flags on the stack, which the evaluators of `recording_play`,
`_and_delete` and `_and_hover` push as 0, 8 and `0x10`). The editor
(`api::recordings::play_on`) makes the call on a connection of its own,
then `play::recording_path::RecordingPath` keeps the played player's
unit position from each sample the object stream delivers
([live-mirror.md](live-mirror.md), "Players, objects and poses"), one
per new tick, to 20,000 points; a new epoch ends the path. A paused game
is refused, because the call answers on the next tick.

**Re-recording from input.** Feasible, as the inverse of playback: the
game samples a player's unit control each tick and the editor encodes
the changes as events. It needs a game method (`recording.capture`,
slice K4) that reads the same control fields the handlers write, the K3
encoder, and a start: the unit placement the script plays it on, where
the player is put before capture starts. The check is that a captured
stream played back on the same unit from the same start reaches the same
place in a deterministic run (**S** until run).

**Write.** `hs::recording_edit::Stream` parses a stream into events that
encode back to the same bytes: it keeps the initial state's padding, the
form each delay was written in and any bytes after the end event, so an
unedited stream encodes to itself. The edits keep every event that is not
named at its tick: removing one gives its delay to the next, inserting one
takes its delay from the one it goes before, moving one takes and gives
the difference (the end event, with nothing after it, sets the length).
The end event cannot be removed or inserted, a payload has its type's
length, an initial field its width, and the result must walk clean
(`Stream::check`). `payload_from` builds a payload from the names
`recording.read` gives. The block's tick count becomes the stream's.

`app::recordings::Doc` holds the block as the source has it and the
entries as edited, with an undo stack of entry snapshots. Each edit
(`App::recording_write`, `recording_remove`, `recording_step`) replaces
the entries and hands the scenario session the diff
(`hs::recording_edit::changes`, `Session::set_recording_changes`): the
element bytes that differ, a stream no longer than its source's written
over it with the size patched, and a longer stream as a
`scenario_edit::TagGrown` at the slot's `(size, address)` pair, appended
to the metadata on save behind **Allow growing full blocks**. The old
streams stay as dead bytes. Nothing is written by the tag editor, which
does not own the scenario tag's bytes; the diff counts as an unsaved
change and goes out with the placement edits. A refused diff changes
nothing.

**Remove** moves the later elements down and lowers the count. It is
refused when a script plays the recording, when a script plays a later
one by its number (which would then name another), and when the scripts do
not read (without the executable, who plays it cannot be known).
Recordings are not added: the block keeps the element room it has.

On a converted Halo 1 map the conversion owns the scenario, so edits
should save as an overlay the converter applies, the way
[modeling.md](modeling.md), "Converted Halo 1 maps" does for geometry.
That overlay is not built: the edits above are written into the converted
map itself, and a reconversion discards them.

### Strings

`strings.list {tag?, language?, query?, limit?, offset?}` lists the open
map's `unic` strings (`api/strings.rs`, over `tags::Localization`;
[agent-api.md](agent-api.md)). A `unic` tag holds no text: it selects, for
each of the nine languages, a span of that language's locale table, and a
string key can repeat across tags with different text. The list therefore
groups by tag and key: one row per key of a tag, its identifier name
resolved through the map's string list, and its text in each language the
key has. `language` narrows the languages shown, `query` searches names,
IDs and the shown texts, and `tag` narrows to one tag. Text is decoded the
way the engine does, so a lossy character is U+FFFD. The write stays
unbuilt for the reason in the table above. There is no Strings tool view
yet; the method is the viewer.

### Other editors worth building

| Editor | Payoff | Stands on | Blocked by |
|---|---|---|---|
| **Model poses and animations**: pick an animation, scrub it on the model | high: it is the preview every character, weapon and vehicle edit wants, and the face preview needs it | `tags::anim::AnimationGraph`, `anim_sample` (every retail codec), the native view | the poses are built (slice M1); editing animation data is the animation editor of [studio-redesign.md](studio-redesign.md) |
| **Sound audition**: play any sound, loop or dialogue line | high for little work | as "Sounds" | nothing; slice A1 |
| **Strings**: every `unic` string by name and language, searched | medium: the first thing a map author changes | `tags::localization` (read only) | writing: string bytes live in the cache's language tables, outside the metadata, and no writer exists. Slice N1 is the read, `strings.list` ([below](#strings)); the write is named, not sliced |
| **Effects, particles, lights and flares** with a preview | high, but large | `tags::effects::{effect, particle, movement, contrail, light, lens}`, curves and colours from this page | a preview needs effect drawing in the native view, which is the game renderer's work first |
| **Sky and fog** | medium | `tags::lighting`; the viewport already uses the sky's colour, sun and fog | nothing beyond colours and the field panel; no slice of its own |
| **Damage, ballistics, weapon tuning** | medium | definitions only | these are number sheets: tag-editing's units and names serve them better than a custom view |
| **AI squads, zones and orders** | high | nothing | the engine's AI readers are not disassembled ([scenario-editor.md](scenario-editor.md) §6) |
| **Camera tracks and cinematics** | low until scripts and recordings exist | `defs/trak.txt` | scripts first |

## Interfaces

### The editor's socket API

Each tool is an `api::Provider` (`crates/apps/studio/src/api/seams.rs`)
and its edits are the tool's own actions through `App::perform`, so the
window and the API stay one path. All are listed by `session.methods`
and documented in [agent-api.md](agent-api.md) when built. `model.*` is
already a namespace there (player models and looks), so poses use `anim.*`.

| Method | Params | Result |
|---|---|---|
| `tag.fields` (extended) | as tag-editing.md | a row with a field view gains `view` (its word); a colour row gains `colour: {form, by, hex, rgba, intensity?}`; a function row gains `function: {type, ranged, range, length}` |
| `tag.set` (extended) | a colour row's `value` may be `"#aarrggbb"`, `"#rrggbb"` or `{rgba, intensity?}` | as built |
| `tag.colours` | `{tag}` | every colour row of the tag, every list walked as `tag.find` walks: `{colours: [{path, label, form, by, hex, rgba, intensity?}], truncated}` |
| `function.read` | `{tag, path}` | `{type, ranged, sub_types, range, length, sets: [...], editable, read_only}` |
| `function.set` | `{tag, path, type?, ranged?, range?, sets?}` | the new `function.read`; one undo step; refusals as "Curves" |
| `function.sample` | `{tag, path, inputs: [x], range_input?}` | `{outputs}` from `Function::evaluate` |
| `shader.read` | `{tag, time?}` | `{tag, template, lod, lods, bitmaps, colours, vectors, time, clock_length, animations, passes, used_by, names_known, names, read_only, refusals}`; each slot `{kind, index, path, label, value, used_by}`, each animation `{index, input, period, function_bytes, path, drives, kinds, refusal, value, no_value}` |
| `shader.set` | `{tag, slot: {kind: bitmap\|colour\|vector, index}, value}` | the slot as `shader.read` lists it; the write, undo step and refusals are the `tag.set` it stands for |
| `shader.preview` | `{tag, shape?: sphere\|quad\|object, object?, size?, time?, out?}` | `{png, width, height, covered, shape, time, animation: [{index, value, no_value, drives}], passes_drawn, refusals: [{layer, passes, why}], base, unresolved, notes}`; with no renderer `png` is null and the counts still answer |
| `shader.code` | `{tag, pass?, implementation?}` | `{tag, pass, passes, layer, pass_in_layer, pass_tag, implementation, implementations, vertex: {classification, listing, wgsl, warnings}, pixel: {listing, wgsl, warnings}}` |
| `script.list` | `{filter?}` | `{scripts: [{name, kind, type, nodes}], globals, free_nodes, node_limit}` |
| `script.source` | `{name?}` | `{name, item, kind, type, source, links: [{span, kind, name, target, why?}]}` |
| `script.follow` | `{name?, link}` | `{kind, name, target, tool, log}`: what a click on the link does |
| `script.compile` | `{source?, working?, name?, apply?}` | `{ok, applied, definitions, nodes, freed, reused, grown, new_strings, grows, free_nodes, depth}`, or `{ok: false, errors: [{line, column, span, code, text}]}`; with `apply`, one undo step |
| `script.undo` | `{redo?}` | `{changed, free_nodes, depth}` |
| `script.eval` | `{expression, type?}` | the paired game's `hs.eval`: `{ok, type, type_index, value, bits}`, or `{ok: false, errors, error}`; `REFUSED` without a paired, running game that lists it |
| `script.uses` | `{kind, name}` | `{uses: [{name, item, count}]}`: the definitions that name a script, global, object name, volume, tag or recording |
| `script.builtins` | `{filter?, unnamed?}` | `{builtins: [{index, name, label, from, returns, parameters, own_arguments, call, usage}], named_by_map, named_by_doc, unnamed, doc}` |
| `script.doc` | `{path}` | `{path, doc, log}`: names builtins from the listing at `path` (`null`: none) |
| `script.external` | `{name?, editor?, launch?}` | `{path, context, line, editor, commands, launched, working}`: writes the working copy and opens it in `code`, `zed` or any editor (`auto`) at the definition |
| `script.working` | `{}` | `{open, map, path, edited, changed, added, removed, problems, stale, summary}`: the working copy read and checked now; `{open: false}` with none |
| `sound.read` | `{tag, range?, permutation?, bins?}` | the header, every pitch range's permutations `{encoded_bytes, chunks}`, for one permutation `{channels, rate, frames, duration_ms, peaks}` or its `refusal`, its `mouth` `{ticks, curve}` either way, and `playback` |
| `sound.play` | `{tag, range?, permutation?, from_ms?}` | `{playing, frames, rate, channels, duration_ms, from_ms, output}`; `output` is `headless` when `Audition::headless` is set (tests, the API's own examples), which plays to nothing in real time |
| `sound.stop` | `{}` | `{stopped}` |
| `sound.mouth` | `{tag, range?, permutation?}`; `curve?`, `from_sound?: {gain, smoothing}`, `revert?` or `undo?` to write (A4) | `{ticks, curve, edited}` (0 to 1 per tick; `curve` null when unvoiced), `written` after a write |
| `sound.face` | `{tag?, range?, permutation?, at_ms?}` or `{weight?}`, with `on?` (the object), `overlay?` (in place of `talk`), `size?`, `out?` | `{weight, at_ms, tick, ticks, byte, voiced}`; with `on`, `talk`, `base` and `anim.pose`'s answer and PNG |
| `anim.list`, `anim.pose` | `{tag}`; `{tag, animation, frame?, overlay?: {animation, weight}, model?, size?, out?}` | the animations with types, frame counts and codecs, and the models that use the graph; the posed node transforms (local and world, which channels the animation supplied), and the posed model drawn to a PNG |
| `recording.list` | none | `{recordings: [{name, ticks, version, control_version, stream_bytes, events, problem?, played_by}], note?}` |
| `recording.read` | `{name, limit?, offset?}` | `{initial: {fields, trailer}, events: [{tick, at, delay, kind, name, fields, payload}], total, truncated, ...}`; a stream that does not read is `REFUSED` with the offset |
| `recording.write` | `{name, initial?, trailer?, events?: [{op: set\|insert\|remove, index, delay?, payload?\|fields?, kind?}]}` | `{name, ticks, stream_bytes, events, grows}`; all or nothing, one undo step |
| `recording.remove` | `{name}` | `{removed, recordings}`; `REFUSED` while a script plays it |
| `recording.undo` | `{redo?}` | `{changed}` |
| `recording.play` | `{name, unit?: "player.<n>", mode?: play\|delete\|hover}` | the game's answer `{player, recording, index, unit}` and `path`; `REFUSED` without a paired, running game that lists it, with the game's reason when it refuses |
| `recording.path` | `{clear?, limit?}` | `{recording, player, count, points, truncated}`: the unit positions kept since the last play |
| `recording.capture` | `{name, unit, start?, stop?}` (K4) | forwarded to the paired game; `REFUSED` without one |

### The game's agent API

Three methods are new on the game's side
([agent-api.md](../runtime/agent-api.md)); each is used by more than one
slice and by later tests, so each is an expansion, not a one-off:

| Method | Class | Does | Needed by |
|---|---|---|---|
| `hs.eval` (built) | read | compiles one expression against the loaded map (`hs::compile`) and runs it on `hs::vm` over a snapshot of the scripts and global cells at a tick boundary; `{type, type_index, value, bits}`, the compiler's problems with places, or the evaluator's refusal; builtins that act on the game are refused by name | H4; any test that drives a scripted map |
| `recording.play` (built) | act | plays a named recording on a player's unit by the engine's own entry | K2 |
| `recording.capture` | act | records a unit's control per tick into events until stopped | K4 |

Shader, colour and curve edits use the built `edit.batch`; the WGSL path
uses the built `render.dump`, `render.reload` and `render.overrides`.

## Rules and limits

- One document. A rich view holds no state that is not in `TagDoc` or
  the scenario session, apart from what it is showing (the selected
  permutation, the preview clock, the scroll). Closing and reopening a
  view loses nothing.
- Fields stay true. Any tag a rich view shows can be opened as fields,
  and `tag.fields` never differs from what a view wrote.
- A preview is the game's picture or nothing. Shader previews draw only
  through `native_render`; curves plot `Function::evaluate`; the face
  uses the sampled overlay. A part that cannot be drawn exactly is left
  out and named in the view's header.
- No quiet fallbacks. An unknown function type, an unnamed builtin, an
  unresolved script name, an undecodable sound, a shared-cache tag, a
  refused pass: each has its sentence, and none is replaced by a default.
- Size changes go one way: `resize_data` or block growth, both behind
  **Allow growing full blocks**, both refused when the metadata does not
  end the file. Nothing rebuilds a map.
- Choices open a picker dialog (function type, wave, preview shape,
  preview model, unit to play on). No arrow rows and no combos.
- Every view follows "Panels on short windows" in
  [tag-editing.md](tag-editing.md): one scrolling body; the preview
  picture and the timeline are surfaces with a height from their own
  content, not from the leftover height; filters and commands at the top.
- Public repository. Tests build tags, scripts, sounds and streams from
  invented data (`map::build::TagSet`, the `hs` crate's synthetic
  fixtures). Decompiled source, sound samples, curves and programs from
  the disc are never committed, logged or used as fixtures.
- Names for fields, builtins and parameters come from the engine, the
  disc or the permitted retail reference
  ([notes/tag-field-names.md](notes/tag-field-names.md)). A builtin's
  name may also come from the retail editing kit's script documentation,
  read from the user's copy at run time and accepted only where the
  signature matches. Projects built from leaked builds are not a source
  for anything.

## Slices

Sizes are modeling.md's: S under 800 lines with tests, M under 2,500, L
more. T, P and "modeling" numbers are slices of
[tag-editing.md](tag-editing.md), [editor.md](editor.md) and
[modeling.md](modeling.md). Ordered by payoff for the work; a lane takes
the first one whose needs are met.

| # | Slice | Size | Needs | Accepted when |
|---|---|---|---|---|
| X1 (built) | **The seam**: `fields::rich`, the `view=` attribute and its parser, tag-view tabs, `view` on `tag.fields` rows | S | T2, T4 | an invented group with one `view=` word of each kind lists them; an unknown word fails the `defs` suite |
| C1 (built) | **Colour picker**: the five forms, the dialog, recent and saved colours, hex in `tag.set`, `tag.colours` | M | X1, T6 | headless: pick a colour through the dialog on an invented tag of each form; the bytes equal the picked value after quantisation; a drag is one undo step |
| C2 (built for `obje`, `deca`, `lens`, `sky `; repeatable) | **Colour forms for a group** (repeatable): `view=` on a group's colour fields with evidence | S each | X1 | the group's colour rows in `tag.colours` rise; each line cites its source |
| A1 | **Sound audition**: decode, the host output, waveform, play and scrub, `sound.read`, `sound.play` | M | X1 | an invented ADPCM sound plays through the headless sink to its sample count; a WMA sound without the executable is refused by name |
| F1 | **Curve editor, same length**: typed payload accessors, the plot, handles, `function.*` | M | X1 | `function.sample` equals `Function::evaluate` for every type on invented blobs; an edit round-trips through `to_bytes`; types 6 and 8 are read-only |
| H1 | **Scripts, read**: the Scripts tool, the catalogue from the source's executable, source with links, `script.list`, `script.source`, `script.uses` | M | P4 | an invented scenario's scripts list and decompile; a link to an object name selects the placement; without the executable the tool refuses by name |
| M1 (built) | **Model poses**: `anim.list`, `anim.pose`, an animation picker and scrub on a model in the native view | M | P14 | an invented two-bone model posed at a frame differs from the rest pose where the clip moves it and nowhere else |
| S1 (built) | **Shader tab**: slots with `Used by`, bitmap thumbnails in rows and in the picker, the preview on sphere, quad and object, refusals listed, `shader.read`, `shader.set`, `shader.preview` | L | C1, P15 | on an invented material: `shader.set` equals the `tag.set` it stands for; a changed colour changes `shader.preview` pixels; a refused pass is counted and absent |
| S2 (built) | **Shader animation**: animated properties as curves, the preview clock | S | S1, F1 | a periodic property changes the preview between two clock times; a refused animation kind is named |
| S4 (built) | **Shader code, read**: listings, WGSL and warnings per pass, `shader.code` | S | S1 | an invented pass lists both programs; a state program shows its refusal |
| A2 (built) | **Mouth curves, read**: the extra-info names in `defs`, `Gestalt::mouth_curve`, the track under the waveform, `sound.mouth` read | S | A1 | an invented gestalt with one voiced and one unvoiced permutation gives one curve and one null |
| A3 (built) | **Face preview**: the `talk` overlay by the curve on a picked model, in step with play and scrub | S | A2, M1 | `anim.pose` with the overlay at weight 0 and 1 gives two pictures; the preview at a tick uses that tick's byte |
| X2 (built) | **Data that changes length**: `TagDoc::resize_data`, the grown-data entry in the save, the session's form for the scenario | M | nothing | a shorter and a longer invented payload save and read back; growth off refuses; the old bytes are unreferenced; undo restores the pair |
| F2 (built) | **Curve type changes** | S | F1, X2 | changing type saves at the new `blob_len` and reads back equal |
| S3 | **Template parameter names**: read `stem`'s properties and categories blocks (`+0x10`, `+0x18`), names on slots. Waits on a disc check that the blocks are kept and on the source-index to parameter mapping | S | S1 | an invented template names its slots; a template without the blocks shows the not-read sentence |
| C3 (read built; strips wait on a `view=` word) | **Colour functions**: the read of which colours a function blends (**E**), gradient strips | M | F1, C1 | an invented colour property draws its strip; the note records the reader |
| H2 (built) | **Script compiler**: `hs::compile` (parser, names, types and casts, literals, allocator), messages with positions | L | H1 | `compile(decompile(s))` equals the nodes for every construct of the synthetic fixtures; each error class has a test; the `#[ignore]`d disc run reports the share of scripts that round-trip |
| H3 (built) | **Scripts, write**: Edit and Compile in the tool, the session ops, free-slot reuse, growth, strings, the three validations, `script.compile` | M | H2, X2 | one changed script changes only its nodes and new strings in the saved map; a compile past the node limit is refused with both counts; undo restores the table |
| A4 (built) | **Mouth curves, write**: drawing, **From the sound**, the in-place raw write | M | A2; modeling 3 | a redrawn invented curve changes only its bytes and the checksum; a shared-cache sound and a split chain are refused |
| K1 (built) | **Recordings, read**: the stream crate, typed events, the tool and timeline, `recording.list`, `recording.read` | M | H1 | every Halo 2 event type decodes from an invented stream; a bad type or length is an error with its offset |
| K2 (built) | **Recordings, play**: the game's `recording.play`, the path in Scene | M | K1; live-mirror | on the fake game, **Play on unit...** sends the call and the streamed positions draw a path |
| H4 (built) | **`hs.eval`** in the game, and an expression box in the Scripts tool | M | H1 | the agent crate's test evaluates an invented expression through the evaluator; a parse error returns the compiler's text with its place |
| K3 (built) | **Recordings, write**: the encoder, event edits, `recording.write`, the converted-map overlay | M | K1, X2 | `encode(decode(s)) == s`; an edited stream saves and walks clean; removing a played recording is refused |
| S5 (built) | **WGSL overrides from the editor** | S | S4, P12 | on the fake game, an edited file reaches `render.reload` and a compile error shows under the text |
| N1 (built) | **Strings, read**: every string by name and language, searched | S | nothing | an invented table lists and searches in each language |
| K4 | **Re-record from input**: the game's `recording.capture`, the capture flow | L | K2, K3 | a capture on the fake game encodes to a stream that walks clean; the determinism check is in "Not verified" |
| S6 | **Xbox shader programs, write**: the vertex assembler, the combiner encoder and parser, the writes | L | S4, X2 | `assemble(disassemble(p)) == p` and `encode(decode(d)) == d` on invented programs; over-limit and untranslatable programs are refused |
| A5 (built) | **Facial sequences**: the engine read of the second extra-info payload, written up as a note | S | A2 | the note states the layout with addresses, or that the disc has none |

X1, A1, H1, M1, N1 and X2 need nothing from each other and can run side
by side. The first seven rows are the shortest road to something a user
sees in each area: colours everywhere, sounds that play, curves, scripts
that can be read, and models that move.

## Evidence

- **Code**: every "have" and "gap" above is read from the file it names.
  The colour hint is `crates/formats/tags/src/registry.rs` (`hint`); the
  function round trip is `effects/function.rs`; the vertex encoder is the
  `encode` module of `crates/formats/shaders/src/vsh/mod.rs`; the compiler is
  `crates/formats/hs/src/compiler.rs` (`compile`).
- **E**: the mouth curve and its sampler, as cited in
  [sounds.md](../halo1/sounds.md); the function evaluator, as cited in
  [effects.md](../formats/effects.md); the recording decoder loop and
  handler table, in the module comment of
  `crates/tools/h1campaign/src/recordings.rs`.
- **V**: the disc's scenarios hold no recordings
  (`defs/recorded_animation.txt`); the builtin name counts and the
  decompiler's node-count check are in
  [haloscript.md](../formats/haloscript.md).
- **S**: that node types can be inferred well enough to round-trip every
  disc script (the `#[ignore]`d run of H2 verifies it); that the facial
  sequence is used by any retail sound (its layout is **E**, A5); that a colour property keeps
  its colours beside its function (C3); that the `stem` parameter blocks
  hold names usable as labels (S3).
- **Design**: the seam, the `view=` words, `resize_data`, the refusal
  sentences, the loudness generator, the preview shapes, and the order of
  the slices.

## Not verified

- Whether a retail material's preview on a sphere matches the game's
  picture of the same shader on its own mesh, pass for pass.
  `shader.preview` against `render.capture` on the owner's disc shows it.
- Whether the game picks up an in-place mouth-curve edit on reload with
  no other change to the gestalt.
- Whether a grown node block or grown `script_strings` loads: the
  engine's headroom for tag memory is the open question of
  [scenario-editor.md](scenario-editor.md) §6, and the script node pool
  has its own limit.
- Whether a captured recording replays to the same place. It needs a
  deterministic run on a converted map.
- How many disc scripts the compiler round-trips, and which constructs
  the rest use.
- Whether decoding and drawing a long dialogue line's waveform is quick
  enough to do on selection without a pause.

## Related

- [tag-editing.md](tag-editing.md): the row model, pickers and short
  windows this design builds on.
- [editor.md](editor.md): the document, what can be written, shared
  caches.
- [editor.md](editor.md): the native view the previews draw in.
- [modeling.md](modeling.md): `patch_raw`, `append_raw`, and converted
  maps.
- [live-mirror.md](live-mirror.md): how edits reach a game.
- [agent-api.md](agent-api.md): the socket API.
- [shader-instances.md](../formats/shader-instances.md),
  [shaders.md](../graphics/shaders.md),
  [hot-iteration.md](../graphics/hot-iteration.md): shader layouts,
  programs and overrides.
- [haloscript.md](../formats/haloscript.md),
  [hs_vm.md](../engine/hs_vm.md): script tables, the decompiler and the VM.
- [sound.md](../formats/sound.md), [sounds.md](../halo1/sounds.md): the
  sound tags and the lip-sync read.
- [effects.md](../formats/effects.md): function data.
