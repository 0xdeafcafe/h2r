# Rich editors: shaders, colours, curves, scripts, sounds and recordings

Status: Design, except the seam (X1), "Data that changes length", sound audition (A1), mouth curves, read (A2), Scripts, read (H1), Recordings, read (K1), the curve editor, same length (F1), curve type changes (F2) and Model poses (M1), which are built; each section names the format code it stands on and the gaps in it.
Code: `crates/apps/studio/src/fields.rs` (the panel the rich views join), `crates/apps/studio/src/fields/rich.rs` (the seam), `crates/formats/tags/src/defs/attrs.rs` (`FieldView`), `crates/apps/studio/src/app/tag_doc.rs` (`TagDoc`, the only write path), `crates/apps/studio/src/fields/curve.rs` and `crates/apps/studio/src/api/curves.rs` (F1), `crates/formats/tags/src/shader.rs`, `crates/formats/tags/src/effects/function.rs`, `crates/formats/hs/src/source.rs`, `crates/formats/hs/src/script_doc.rs`, `crates/formats/hs/src/recording.rs`, `crates/apps/studio/src/app/recordings.rs`, `crates/apps/studio/src/api/recordings.rs`, `crates/apps/studio/src/app/scripts.rs`, `crates/apps/studio/src/ui/scripts.rs`, `crates/apps/studio/src/api/scripts.rs`, `crates/tools/h1campaign/src/recordings.rs`, `crates/formats/tags/src/sound.rs`, `crates/apps/studio/src/app/audition.rs`. Tests: `defs/tests.rs`, `registry/tests.rs`, `fields::rich::tests` and `api/tests.rs` (X1), `effects::function::tests` and `api/curves/tests.rs` (F1), `app/tag_data_tests.rs` (X2), `app/audition_tests.rs` and `api/sound/tests.rs` (A1), `app::scripts::tests`, `hs::script_doc::tests` and `hs::source::each_tests` (H1), `hs::recording::tests` and `app/recordings_tests.rs` (K1); each other slice names its own, `crates/apps/studio/src/app/anim.rs`, `crates/apps/studio/src/ui/anim.rs` (M1), `api/anim/tests.rs`, `anim_tests.rs`.
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
through `TagDoc`.

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

### Colours

**What the user does.** Every colour field shows a swatch, the numbers,
and opens a picker dialog on a click: a saturation and value square, a
hue bar, an alpha bar over a checkerboard when the field has alpha, RGB,
HSV and hex entry, the value in the file beside the new one, a row of
recent colours and a saved palette. Copy and paste carry a hex string
between any two colour fields, converting form. A drag is one undo step.

**The data.** The definitions have no colour type. `registry::Hint::Colour`
is set by `registry::hint` only for an array of three floats whose name
contains `colour` or `color`, and `fields.rs` draws that one case with
`color_edit3`. Four-float colours and packed 32-bit colours show as plain
numbers or hex. The `view=` words name the stored form:

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
opening the curve editor below with the strip under the plot. The gap:
`effects::function::Function` evaluates to one scalar and keeps header
bytes `+0xc` to `+0x14` as `unknown_0c`; no code reads which colours a
colour-valued property blends between. Slice C3 starts with that read
from the engine (**E**), and until it lands colour functions show as
scalar curves with the sentence `The colours this curve blends are not
read yet.`

**Preview.** None of its own. A colour change redraws whatever shows the
tag: the Shader preview, the viewport, and a connected game when the
field's effect is known.

**Write.** Fixed-length, in place, through `TagDoc::write`. Nothing is
refused beyond the tag's own read-only reasons.

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

**What the user does.** Opening a `shad` tag shows a **Shader** tab: a
preview on the left, parameters on the right in four folding sections.

- **Bitmaps**: one row per slot with a thumbnail, the bitmap's name,
  **Open** and **Change...** (the tag picker of tag-editing.md, limited
  to `bitm`, with thumbnails in its list).
- **Colours**: the pixel constants, each a `argb8` colour row.
- **Vectors**: the vertex constants, each four numbers; a slot the
  template binds as a texture transform shows as scale and offset.
- **Animation**: one row per animated property with its curve and what it
  drives.

A header names the template (`stem`) with **Open**, the level of detail
shown, and every refusal of the preview in words. Each parameter row has
a `Used by` line from the template's binding: which pass and which
register or stage reads it.

**The data.** `crates/formats/tags/src/shader.rs`, decode only:
`ShaderInstance` and `ShaderPostprocess` (`bitmaps`, `colors`, `vectors`,
`properties`, `animations`, `lods`), `ShaderTemplate`,
`TemplatePostprocess`, `ParameterBinding`, `ShaderPass`, `PassPostprocess`;
`tags::material::ShaderLibrary::material` resolves an instance to
`ResolvedPass` values. The fields the editor writes are named in
`crates/formats/tags/src/defs/shad.txt`, and the bitmap, colour and vector
slots carry `applies=frame`. Gaps:

- **Parameter names.** Slots bind by index (`ParameterBinding::source`).
  The template's parameter and category blocks (`stem` root `+0x40`,
  `+0x4c`) are not read ([shader-instances.md](../formats/shader-instances.md),
  "Open items"). Until slice S3 reads them, a slot is labelled by its
  kind, index and `Used by` line (`Bitmap 2 · stage 1 of pass 3`), and
  the tab says once `This template's parameter names are not read yet.`
- **`pixl`, `slit`.** No code parses either. The light response
  reference shows as a plain reference row.
- **`vrtx`.** A definition only (`defs/vrtx.txt`); no typed struct.

**Preview.** The editor's native view: `scenario_edit::native_view` on
`native_render`, the game's own material path (`layers_load::plan_without_game`,
`program::compile`). The preview draws the resolved material on a sphere,
a flat quad or the selected object's own mesh (a row that opens a
picker), under the map's lighting when a map is open and a fixed key
light otherwise. An edit re-plans the one material and shows on the next
frame; the Scene viewport shows the same change on the map. Animated
properties run on a preview clock with pause and scrub. What the native
view refuses, the preview refuses in the same words, listed in the
header: layer 1 (`native_view::LAYER_1_UNCHECKED`), bitmap, render-state
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

1. **Read it** (slice S4). A **Code** section per pass shows the vertex
   program's disassembly (`vsh::disassemble`), the combiner listing
   (`psh::disassemble`), the translated WGSL (`vsh::wgsl::translate`,
   `psh::wgsl::translate`) and the translator's warnings.
2. **Override the translation** (slice S5). The game already replaces a
   translated program with a WGSL file from a watched directory
   (`crates/platform/hot-render`, `render.dump`, `render.reload`,
   [hot-iteration.md](../graphics/hot-iteration.md)). The Code section
   edits such a file with the compile error under the text and sends it
   to the connected game. It changes how our renderer draws, is saved in
   the override directory, and is never written into a map; the section
   says so.
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
named. **Edit** and **Compile** (slice H3) are not built: the tab is read
only. **Open in VS Code** and **Open in Zed** write every global and
script to a working copy (`scripts/<map>/<map>.hsc` under the data root,
with the `h2hs.json` context `h2 hs lsp` checks it against) and open it
there; each save comes back to the tab as the definitions that differ
from the map's and the file's problems (`app/external.rs`;
[editor-guide.md, "Scripts in VS Code and Zed"](editor-guide.md#scripts-in-vs-code-and-zed)).
Those edits reach the map once the compiler below exists.

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

**What the compiler needs.** `hs::compile` (new) does not exist;
`source::source_counts` only tokenises. It needs:

| Need | Have | Gap |
|---|---|---|
| A parser for the decompiler's own output | the tokeniser | the tree builder, with positions for messages |
| Function name to builtin index, with parameter and return types | `Catalog`: types and addresses for every builtin; names for those some scenario calls (`Catalog::observe`), plus `h1campaign::scripts::identified`, plus those a documentation listing names by signature (`hs::Naming`) | a builtin nothing names cannot be called by name. It is callable as `(#builtin:<index> ...)` and listed as such; nothing guesses a name |
| External global name to index | `Catalog::external_global_types` (types only) | names only as observed in scenarios, as for builtins |
| Type checking and casts | `cast::castable`, the VM's `HsType` | inference of each node's type from its parent, as the engine's compiler does (**S**: the retail node types of every disc script are the check, below) |
| Literal to node data, per value type | numbers, booleans, strings; script and global references; tag references through `script_tag_references` | names of scenario things (object names, trigger volumes, cutscene flags and camera points, starting profiles, AI) resolve through scenario blocks, and the AI blocks have no reader ([scenario-editor.md](scenario-editor.md) §6). A name the compiler cannot resolve from a block resolves only if this map's scripts already use it (the pair of type and token is harvested from the existing nodes); otherwise it is an error naming the type |
| Node salts and links | `wire::Datum`, `Scenario::validate_graph` | the allocator: free slots first, then new slots |
| The executable | `Catalog::from_xbe`; the Scripts tab reads `default.xbe` from the source (`MapSource::executable`) and refuses without it | nothing |

The compiler's acceptance is a round trip: for every script,
`compile(decompile(s))` gives nodes equal to the source's in opcode,
type, flags, data and shape. The synthetic form of that test is in the
slice; the disc form is `#[ignore]`d and is the evidence that the type
inference matches retail.

**Preview.** There is no editor-side run. `hs::vm` evaluates against
`vm::mock` only and nothing in the game calls it
([hs_vm.md](../engine/hs_vm.md)). With a game connected the tool shows
live threads and globals beside the source (`hs.threads`, `hs.globals`).
A changed script has the `reload` effect. Evaluating an expression in the
running game needs a game method that does not exist (`hs.eval`, slice
H4), and is not assumed.

**Write.** Scripts compile one at a time. An unedited script's nodes are
not touched, so a map with one changed script differs only in that
script's nodes.

- The old nodes of an edited script are freed (salt kept, marked free as
  the source's free slots are); new nodes take free slots first.
- More nodes than free slots grows the node block as any block grows
  (appended on save, **Allow growing full blocks** on), up to
  `hs::scenario::RETAIL_NODES`. Past that the compile is refused with
  both counts.
- New tokens are appended to `script_strings` through the scenario
  session's form of `resize_data`; existing offsets never move.
- A new script or global adds an element to its block. Removing a script
  or a global another script uses is refused, naming the user.
- Before the edit is accepted, the whole table passes
  `Scenario::validate_graph`, `Scenario::validate` and
  `verify_codec_symmetry`. A failure rejects the edit and leaves the
  document as it was.
- A scripted map already refuses the removal of a trigger volume
  (`scenario_edit::scenario::is_scripted`). With the Scripts tool the
  refusal names the scripts that use it.

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
  out of the resource by its two locators — one aperture byte per 30 Hz
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
  drawing is A4;
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
with the character speaking is a script call and belongs to `hs.eval`.

**Write.**

| Edit | How | Refused when |
|---|---|---|
| Redraw a curve at its length | the bytes, in place in the map's raw data, through `MapEditor::patch_raw` (modeling.md slice 3) | the sound's data is in a shared cache; the permutation is a split chain (no retail example of its segment timing) |
| Give a curve to a permutation that has none, or change its length | not written | always, until raw data can be appended (`MapEditor::append_raw`, modeling.md slice 7) and the descriptor list grown; named |
| The audio itself | not written | always: replacing samples is an encoder and a gestalt rebuild, outside this design |

A curve's length follows the sound's length, so a redraw never needs
another length. The Halo 1 converter is the one existing writer of
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
list still shows and says that the scripts that play each need it. The
lanes, the scrub bar and **Play on unit...** are not built (slice K2 adds
the play); with a game connected the Scene viewport would draw the unit
moving through the live mirror and keep the path as a line.

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
is not read. Gaps: nothing encodes a stream (slice K3), and only version 4
streams are read. Halo 2's player reads the stream; a Halo 1 stream in a
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
needs one. Playing needs a game method (`recording.play`, slice K2); the
positions come back over the mirror's object stream
([live-mirror.md](live-mirror.md), "Players, objects and poses").

**Re-recording from input.** Feasible, as the inverse of playback: the
game samples a player's unit control each tick and the editor encodes
the changes as events. It needs a game method (`recording.capture`,
slice K4) that reads the same control fields the handlers write, the K3
encoder, and a start: the unit placement the script plays it on, where
the player is put before capture starts. The check is that a captured
stream played back on the same unit from the same start reaches the same
place in a deterministic run (**S** until run).

**Write.** Through the scenario session. A stream changes length with
every edit, so it uses the session's `resize_data`; a new recording adds
a block element. Renaming or removing one a script plays is refused,
naming the script. On a converted Halo 1 map the conversion owns the
scenario, so edits save as an overlay the converter applies, the way
[modeling.md](modeling.md), "Converted Halo 1 maps" does for geometry.

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
| `tag.fields` (extended) | as tag-editing.md | a row with a field view gains `view: {kind, form?}`; a colour row gains `colour: {hex, rgba, intensity?}`; a function row gains `function: {type, ranged, range, length}` |
| `tag.set` (extended) | a colour row's `value` may be `"#aarrggbb"` or `{rgba, intensity?}` | as built |
| `tag.colours` | `{tag}` | every colour row of the tag: `{path, label, form, hex}` |
| `function.read` | `{tag, path}` | `{type, ranged, sub_types, range, length, sets: [...], editable, read_only}` |
| `function.set` | `{tag, path, type?, ranged?, range?, sets?}` | the new `function.read`; one undo step; refusals as "Curves" |
| `function.sample` | `{tag, path, inputs: [x], range_input?}` | `{outputs}` from `Function::evaluate` |
| `shader.read` | `{tag}` | `{template, lod, bitmaps, colours, vectors, animations, used_by, names_known, refusals}` |
| `shader.set` | `{tag, slot: {kind, index}, value}` | the row; equal to the `tag.set` it stands for |
| `shader.preview` | `{tag, shape?, object?, time?, size?}` | `{png, passes_drawn, refusals}` |
| `shader.code` | `{tag, pass}` | `{vertex: {listing, wgsl, warnings}, pixel: {listing, wgsl, warnings}}` |
| `script.list` | `{filter?}` | `{scripts: [{name, kind, type, nodes}], globals, free_nodes, node_limit}` |
| `script.source` | `{name?}` | `{name, item, kind, type, source, links: [{span, kind, name, target, why?}]}` |
| `script.follow` | `{name?, link}` | `{kind, name, target, tool, log}`: what a click on the link does |
| `script.compile` | `{name, source, apply?}` | `{ok, errors: [{line, column, text}], nodes, new_strings, grows}`; with `apply`, one undo step |
| `script.uses` | `{kind, name}` | `{uses: [{name, item, count}]}`: the definitions that name a script, global, object name, volume, tag or recording |
| `script.builtins` | `{filter?, unnamed?}` | `{builtins: [{index, name, label, from, returns, parameters, own_arguments, call, usage}], named_by_map, named_by_doc, unnamed, doc}` |
| `script.doc` | `{path}` | `{path, doc, log}`: names builtins from the listing at `path` (`null`: none) |
| `script.external` | `{name?, editor?, launch?}` | `{path, context, line, editor, commands, launched, working}`: writes the working copy and opens it in `code`, `zed` or any editor (`auto`) at the definition |
| `script.working` | `{}` | `{open, map, path, edited, changed, added, removed, problems, stale, summary}`: the working copy read and checked now; `{open: false}` with none |
| `sound.read` | `{tag, range?, permutation?, bins?}` | the header, every pitch range's permutations `{encoded_bytes, chunks}`, for one permutation `{channels, rate, frames, duration_ms, peaks}` or its `refusal`, its `mouth` `{ticks, curve}` either way, and `playback` |
| `sound.play` | `{tag, range?, permutation?, from_ms?}` | `{playing, frames, rate, channels, duration_ms, from_ms, output}`; `output` is `headless` when `Audition::headless` is set (tests, the API's own examples), which plays to nothing in real time |
| `sound.stop` | `{}` | `{stopped}` |
| `sound.mouth` | `{tag, range?, permutation?}`; `curve?` or `from_sound?: {gain, smoothing}` to write are A4 | `{ticks, curve}` (0 to 1 per tick; `curve` null when unvoiced) |
| `sound.face` | `{tag?, range?, permutation?, at_ms?}` or `{weight?}`, with `on?` (the object), `overlay?` (in place of `talk`), `size?`, `out?` | `{weight, at_ms, tick, ticks, byte, voiced}`; with `on`, `talk`, `base` and `anim.pose`'s answer and PNG |
| `anim.list`, `anim.pose` | `{tag}`; `{tag, animation, frame?, overlay?: {animation, weight}, model?, size?, out?}` | the animations with types, frame counts and codecs, and the models that use the graph; the posed node transforms (local and world, which channels the animation supplied), and the posed model drawn to a PNG |
| `recording.list` | none | `{recordings: [{name, ticks, version, control_version, stream_bytes, events, problem?, played_by}], note?}` |
| `recording.read` | `{name, limit?, offset?}` | `{initial: {fields, trailer}, events: [{tick, at, delay, kind, name, fields, payload}], total, truncated, ...}`; a stream that does not read is `REFUSED` with the offset |
| `recording.write` | `{name, initial?, events}` | the new `recording.read`; one undo step |
| `recording.play`, `recording.capture` | `{name, unit}`; `{name, unit, start?, stop?}` | forwarded to the paired game; `NOT_PAIRED` without one |

### The game's agent API

Three methods are new on the game's side
([agent-api.md](../runtime/agent-api.md)); each is used by more than one
slice and by later tests, so each is an expansion, not a one-off:

| Method | Class | Does | Needed by |
|---|---|---|---|
| `hs.eval` | act | evaluates one expression through the game's own script evaluator on the game thread; `{value, type}` or the evaluator's error | H4; any test that drives a scripted map |
| `recording.play` | act | plays a named recording on a unit by the engine's own entry | K2 |
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
| C1 | **Colour picker**: the five forms, the dialog, recent and saved colours, hex in `tag.set`, `tag.colours` | M | X1, T6 | headless: pick a colour through the dialog on an invented tag of each form; the bytes equal the picked value after quantisation; a drag is one undo step |
| C2 | **Colour forms for a group** (repeatable): `view=` on a group's colour fields with evidence | S each | X1 | the group's colour rows in `tag.colours` rise; each line cites its source |
| A1 | **Sound audition**: decode, the host output, waveform, play and scrub, `sound.read`, `sound.play` | M | X1 | an invented ADPCM sound plays through the headless sink to its sample count; a WMA sound without the executable is refused by name |
| F1 | **Curve editor, same length**: typed payload accessors, the plot, handles, `function.*` | M | X1 | `function.sample` equals `Function::evaluate` for every type on invented blobs; an edit round-trips through `to_bytes`; types 6 and 8 are read-only |
| H1 | **Scripts, read**: the Scripts tool, the catalog from the source's executable, source with links, `script.list`, `script.source`, `script.uses` | M | P4 | an invented scenario's scripts list and decompile; a link to an object name selects the placement; without the executable the tool refuses by name |
| M1 (built) | **Model poses**: `anim.list`, `anim.pose`, an animation picker and scrub on a model in the native view | M | P14 | an invented two-bone model posed at a frame differs from the rest pose where the clip moves it and nowhere else |
| S1 | **Shader tab**: slots with `Used by`, bitmap thumbnails in rows and in the picker, the preview on sphere, quad and object, refusals listed, `shader.read`, `shader.set`, `shader.preview` | L | C1, P15 | on an invented material: `shader.set` equals the `tag.set` it stands for; a changed colour changes `shader.preview` pixels; a refused pass is counted and absent |
| S2 | **Shader animation**: animated properties as curves, the preview clock | S | S1, F1 | a periodic property changes the preview between two clock times; a refused animation kind is named |
| S4 | **Shader code, read**: listings, WGSL and warnings per pass, `shader.code` | S | S1 | an invented pass lists both programs; a state program shows its refusal |
| A2 (built) | **Mouth curves, read**: the extra-info names in `defs`, `Gestalt::mouth_curve`, the track under the waveform, `sound.mouth` read | S | A1 | an invented gestalt with one voiced and one unvoiced permutation gives one curve and one null |
| A3 (built) | **Face preview**: the `talk` overlay by the curve on a picked model, in step with play and scrub | S | A2, M1 | `anim.pose` with the overlay at weight 0 and 1 gives two pictures; the preview at a tick uses that tick's byte |
| X2 | **Data that changes length**: `TagDoc::resize_data`, the grown-data entry in the save, the session's form for the scenario | M | nothing | a shorter and a longer invented payload save and read back; growth off refuses; the old bytes are unreferenced; undo restores the pair |
| F2 (built) | **Curve type changes** | S | F1, X2 | changing type saves at the new `blob_len` and reads back equal |
| S3 | **Template parameter names**: read `stem`'s parameter and category blocks (**E** first), names on slots | S | S1 | an invented template names its slots; a template without the blocks shows the not-read sentence |
| C3 | **Colour functions**: the read of which colours a function blends (**E**), gradient strips | M | F1, C1 | an invented colour property draws its strip; the note records the reader |
| H2 | **Script compiler**: `hs::compile` (parser, names, types and casts, literals, allocator), messages with positions | L | H1 | `compile(decompile(s))` equals the nodes for every construct of the synthetic fixtures; each error class has a test; the `#[ignore]`d disc run reports the share of scripts that round-trip |
| H3 | **Scripts, write**: Edit and Compile in the tool, the session ops, free-slot reuse, growth, strings, the three validations, `script.compile` | M | H2, X2 | one changed script changes only its nodes and new strings in the saved map; a compile past the node limit is refused with both counts; undo restores the table |
| A4 | **Mouth curves, write**: drawing, **From the sound**, the in-place raw write | M | A2; modeling 3 | a redrawn invented curve changes only its bytes and the checksum; a shared-cache sound and a split chain are refused |
| K1 (built) | **Recordings, read**: the stream crate, typed events, the tool and timeline, `recording.list`, `recording.read` | M | H1 | every Halo 2 event type decodes from an invented stream; a bad type or length is an error with its offset |
| K2 | **Recordings, play**: the game's `recording.play`, the path in Scene | M | K1; live-mirror | on the fake game, **Play on unit...** sends the call and the streamed positions draw a path |
| H4 | **`hs.eval`** in the game, and an expression box in the Scripts tool | M | H1 | the agent crate's test evaluates an invented expression through the evaluator entry; a parse error returns the evaluator's text |
| K3 | **Recordings, write**: the encoder, event edits, `recording.write`, the converted-map overlay | M | K1, X2 | `encode(decode(s)) == s`; an edited stream saves and walks clean; removing a played recording is refused |
| S5 | **WGSL overrides from the editor** | S | S4, P12 | on the fake game, an edited file reaches `render.reload` and a compile error shows under the text |
| N1 | **Strings, read**: every string by name and language, searched | S | nothing | an invented table lists and searches in each language |
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
  `encode` module of `crates/formats/shaders/src/vsh/mod.rs`; the missing
  compiler is `crates/formats/hs/src/source.rs` (`source_counts`).
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
