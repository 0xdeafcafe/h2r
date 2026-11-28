# Every tag kind: an editor, a preview and three live views

Status: In progress. LE1 (the kind registry, `tag.kinds`, the fan-out and `class` in Pending), LE2 (the subject: `tag.open`, its place in a level and on the stage's pad, the respawn after a `NextSpawn` write, the retrigger of a played sound, the unread mark) and LE3 (bitmaps: the encoder and its Raw delivery, `import.bitmap` as a new `bitm`; sequences and sprites are edited in the inspector) are built, and so are the editors named as built below. The stage's swatch objects, the looping effect and the retrigger of an effect or animation (they need a game method), the build and reload after a `Reload` write (GV6), Host changes from the HUD, UI Screens and Rules tools, and the missing editors are design.
Code: built in `crates/apps/studio/src/app/kinds.rs` (the registry), `crates/apps/studio/src/app/live.rs` (the fan-out), `crates/apps/studio/src/app/tag_doc.rs` (`TagDoc`, the write path), `crates/apps/studio/src/play/tag_sync.rs` (`Effect`, `Play::fan_out`) `crates/apps/studio/src/app/bitmap_import.rs` (`import.bitmap`), `crates/apps/studio/src/app/subject.rs` (the subject's place), `crates/apps/studio/src/play/subject.rs` (the object and its follow-ups in the own game), `crates/tools/content-kit/src/stage` (`subject_place`, the stage's pad) and `crates/apps/studio/src/api/tags.rs` (`tag.kinds`, `tag.open`, `tag.subject`); the views' table in `crates/apps/studio/src/fields/rich.rs` (planned). Tests: `app::kinds::tests`, `app::live::tests`, `play::tag_sync::fan_out_tests`, `app::subject::tests`, `play::subject::tests`, `api::tags::tests`, `content_kit::stage::tests`, each editor's own.
Switch: none.

Studio is where any piece of content is picked and edited in place: a
model, a bitmap, a shader, a colour gradient, a UV layout, a level's
geometry, a screen, a HUD, a sound. This page is the layer that makes
that true for every tag group rather than for the ones that happen to
have a tool: each group has an entry saying which editor opens it, what
draws its preview, and how each of its edits reaches the three places a
change is seen. The rule that must hold: an editor is a view of the one
document (`TagDoc`, or the scenario session), every byte goes through
it, and the three views are fed from that one write, so they cannot
disagree about what was changed.

How far each group's edits reach into a running game is surveyed and
sized on [realtime-edits.md](realtime-edits.md) and its note
`notes/2026-11-23-realtime-survey.md` (new); this page keeps to the
editors and the routing and does not repeat that audit.

It gathers the editors designed in [tag-editing.md](tag-editing.md),
[rich-editors.md](rich-editors.md), [modeling.md](modeling.md),
[hud-editor.md](hud-editor.md) and [scenario-editor.md](scenario-editor.md);
their slices stay on those pages and are cited here by page and number.

## How it works

### The three views

| View | What draws it | Fed by |
|---|---|---|
| **Preview** | the editor's own picture, by Studio's renderer (`native-render` loaders, the HUD and UI previews, the waveform) | the document, re-read after each write |
| **Game** | Studio's own game in editor mode, in the Game panel ([game-view.md](game-view.md)) | the live route below, with no click |
| **Attached** | a paired game someone else runs, in the Game panel's attached mode and in its own window | the same route, under the pairing's View and Edit and its Pending list ([live-mirror.md](live-mirror.md)) |

### One write, three deliveries

Every write path hands a `Change {tag, ranges, class}` to
`app::live::Fanout` (`App::live`):

- `TagDoc`'s `write`, `block_op`, `resize_data` and `set_string`, and
  its `undo` and `redo`, each report one with the bytes written
  (`TagDoc::take_changes`). A field set is Field; a block's count, a
  data length or a new string is Shape. The frames of one drag are one
  change.
- The scenario session and the raw documents (the Modeling tool's
  geometry, the Textures tool's pixels) are seen by their revisions
  each frame (`play::tag_sync::see_documents`): a block's layout
  revision is Shape, another scenario edit Field, a raw document's
  revision Raw. Opening a document is not a write.
- A host source hands in `Change::host(source, params)`.

Once a frame `Play::fan_out` takes the changes, `app::live::route`
gives each view its route, and the game side carries the deliveries
out. The class decides the route:

| Class | What changed | Preview | Game and attached |
|---|---|---|---|
| **Field** | fixed bytes of a tag the game holds | re-read | `edit.batch`, at the next tick. When it shows is the field's `Effect` (`applies=`: `Now`, `NextUse`, `NextSpawn`, `Reload`, `NotKnown`) |
| **Raw** | pixels, vertices, indices, sound samples | re-read | the raw edits ([realtime-edits.md](realtime-edits.md)), at the next frame |
| **Shape** | a block grown or shrunk, data resized, a tag added, a string added | re-read | a build of the project and a load of it ([tag-workspace.md](tag-workspace.md)); a block the game can move goes by the edit arena instead |
| **Host** | content the game draws from host data: HUD kit records, our screens' YAML, a rules script, a host sound | re-read | the host's own reload: `rules.reload`, the UI and HUD kit reloads, `render.reload` for shader overrides |

After a Field delivery to the subject's own tag the fan-out makes the
change visible where the game would otherwise keep showing the old
thing, in the own game only (`play::subject`; each call waits for the
write's batch to be out, and a write to any other tag queues nothing):

- `NextSpawn`: the subject's object is respawned
  (`world.object.respawn`), once however many fields were written, and
  nothing else is.
- `NextUse`: the subject's use is triggered again: a sound is played
  again. An object has no use to trigger, and an effect or an animation
  has no game method yet, so the subject's `failure` says so and no call
  is made.
- `NotKnown`: sent as a Field write and shown with a mark saying the
  game may not have read it (`unread` in `tag.subject`).
- `Reload` and Shape: the build and reload of [game-view.md](game-view.md),
  GV6. A Shape change is built and reloaded by the fan-out; a Field
  write that is only read at reload stays a Pending row until GV6.

In an attached game none of these is done for the user: they are rows
in Pending, as now.

### Own and attached

The own game is a game Studio started (`Play::own_game`: the paired
endpoint is the child's). Every other paired game is attached. Each
change makes one Preview re-read and, with a game paired, one game
delivery or one Pending row:

| Class | Own game | Attached, in Edit | Attached, in View |
|---|---|---|---|
| Field, Raw | one send of what the documents hold, with no click | the pairing's own push | a Pending row |
| Shape | that send, then one play-copy reload (`map.reload`) when a row of class `shape` stays After reload | a Pending row | a Pending row |
| Host | the source's reload method | the same | a Pending row, sent when Edit turns on |

A Host source the game has no reload for (the UI, the HUD kit and host
sounds have none in the game's agent API) is a Pending row in every
game, saying so (`app::live::Host::NO_RELOAD`). No tool emits a Host
change yet: the Rules tool keeps its own `rules.reload` button.

Every Pending row carries its `class` beside its `effect`
(`play::tag_sync::row`): `raw` for `geometry`, `bitmaps` and
`collision` rows, `shape` for a block layout or a new map string,
`host` for a held Host change, else `field`.

### The subject

The tag being edited is the **subject**. The Preview frames it. The own
game shows it too:

- with a level open, an object-like subject is placed at the viewport
  camera's focus, on the ground under it, as an unlinked preview: the
  game's own `sim.spawn`, the ownerless placement a scenario preview
  makes, so nothing is written to the level (`app::subject::place`,
  `Scene::Level`);
- with no level open (a tag opened from the tree), the own game runs
  the [content stage](content-stage.md) and the subject stands on the
  flat bay's subject pad (`content_kit::stage::subject_place`, `Scene::Stage`),
  facing the spawn: an object on the pad, a sound played (`audio.play`,
  at the player's unit). A shader or bitmap on a swatch object of the
  stage, and an effect or light looping at the pad, are design: the
  stage has no swatch object and the game has no method to start a
  looping effect, so the subject's `note` says so.

An object-like subject that is a model (`hlmt`, `mode`, `coll`,
`phmo`) stands as the first scenery, weapon, vehicle or equipment tag
that uses it (`app::subject::spawn_tag`); with none, `refusal` says so.
The subject is the tag in front, else the one selected in the Tags tool
(`App::subject`); a new subject deletes the object of the old one
(`world.object.delete`) before its own is made, so one object stands.

So every kind has a game view even where Studio's renderer cannot draw
it. For those kinds (effects, particles, decals, lens flares) the Game
panel is the preview, and the editor's Preview tab says so.

### The registry

`app::kinds::KINDS` has one row per tag group: the label, the editor (a
tag view of `fields::rich`, a tool, or the field panel), the preview
provider, the subject rule, and the classes its edits take. A group
with no row gets the field panel, no preview, and Field and Shape
routes. `tag.kinds` lists the table, so which kinds lack an editor or a
preview is a query and not a guess.

### The kinds

| Kind (groups) | Editor today | Preview today | Missing | Slice |
|---|---|---|---|---|
| Any tag | the field panel, typed by `tags::defs`, names and units | none | the designed panel | tag-editing T slices |
| Level placements (`scnr`) | the Scenario tool: blocks, volumes, gizmo | the viewport | AI, scripts and recordings as editors | scenario-editor; rich-editors H, K |
| Level geometry (`sbsp`), collision, portals, lightmaps (`ltmp`) | the Modeling tool on the mesh kernel | the viewport, collision and nav layers | writes per target | modeling; modeling-targets |
| Render models (`mode`), collision and physics models (`coll`, `phmo`) | the Modeling tool (open, read, check) | the viewport, bind pose, first variant | variants and regions, markers, posed preview, the write | rich-editors M1; LE5 |
| Objects (`hlmt`, `weap`, `vehi`, `bipd`, `scen`, `eqip`, `proj`) | the field panel; `model.*` reads and renders | `model.render` | one object editor: model, variants, markers, attachments, the tuning sheet with units | LE5 |
| UV layouts (in `mode` and `sbsp` sections) | none | none | a UV view on the mesh kernel: islands over the bitmap, move, scale, pin; Raw writes | LE4 |
| Bitmaps (`bitm`) | `bitmap-edit`'s canvas (paint, PNG in and out) and the sprite editor (sequences, sprites) | thumbnails | none | modeling, "Texture editing"; LE3 |
| Shaders (`shad`, `stem`, `spas`) | the field panel | none | the Shader tab and its preview, animation, code | rich-editors S1 to S6 |
| Colours and gradients (colour fields, colour functions, colour tables) | the field panel | none | the picker, gradient strips, a table editor | rich-editors C1 to C3; LE6 |
| Curves (function data) | the field panel | none | the curve editor | rich-editors F1, F2 |
| Effects, particles, contrails, lights, lens flares, decals (`effe`, `prt3`, `ligh`, `lens`, `deca` and their kin) | the field panel | none | grouped editors on the curves and colours; the game view as preview | LE7 |
| Sky and fog (`sky `, `fog `) | the field panel | the viewport draws the sky's sun and fog from the tag document, so an edit shows on the next frame | a tag view with the dome and the fog curve | LE8 |
| Animations (`jmad`) | model pose, timeline and curve edits, save to an animation package | live metadata writes flush the decoded clips. New raw streams take effect on map load | retarget, import and paired-game preview | rich-editors M1, [studio-redesign.md](studio-redesign.md) AN1 to AN6 |
| Sounds (`snd!`, `lsnd`, `ugh!`) | audition: waveform, play, scrub | the waveform | import, distances, waveform edits, drawing the mouth curve | rich-editors A2 to A5; workspace W7 |
| HUD (`nhdt`, HUD kit) | the HUD tool | its preview | what "Not modelled" lists | hud-editor |
| Screens (`wgit`, `wgtz`, our YAML) | the UI Screens tool | its preview and sheets | the levels' screens and dialogs | ui-in-maps |
| Scripts (in `scnr`) | Scripts, read | none | the compiler and the write | rich-editors H2 to H4 |
| Strings (`unic`), fonts | none | none | the viewer, then the writer | rich-editors N1 |
| Game types, playlists | the Variants, Playlists and Rules tools | the check reports | nothing here | editor-guide |

## Interfaces

- `tag.kinds {group?}` (read): the registry: its rows, the default,
  and every group the registry or the open map has with its editor,
  preview, subject rule and classes; `target` says where the game
  view's changes go now ([agent-api.md](agent-api.md)).
- `tag.open {tag}` (act): opens the tag in its editor and makes it the
  subject; the reply names the editor, the preview and the subject's
  place in the own game. A sound subject is played.
- `tag.subject` (read): the subject's place, the object standing for
  it, the follow-ups owed after a write, the unread fields and the last
  refusal ([agent-api.md](agent-api.md)).
- `tag.preview {tag, out, camera?}` (act): the Preview as a PNG, by
  the kind's provider; `NO_PREVIEW` with the reason for a kind that
  has only the game view.
- `mirror.pending` rows carry `class` (`field`, `raw`, `shape`,
  `host`) beside `effect`.
- Each editor's own namespace is on its page (`shader.*`, `function.*`,
  `sound.*`, `hud.*`, `mesh.*`, `model.*`).

## Sky and fog in the viewport

`ui::sky::live_atmosphere` reads the first of the open scenario's skies
that decodes from the tag document, edits applied, with
`scenario_edit::scene::atmosphere_in_doc`: the sun is the first light
with parameters, the fog the first `fog_a` element. The viewport calls
`ui::sky::apply` whenever the document's revision moves, and
`Viewport::set_atmosphere` swaps the sun and fog words the scene shader
draws with; nothing is uploaded again. A sky that does not decode leaves
the scene's own atmosphere. The own game reads the same bytes through
the Field route. The native renderer's world lighting is not changed by
this path.

## Rules and limits

- An editor never keeps a second copy of a tag. A view that cannot
  model a tag shows the reason and the fields.
- The fan-out acts on the own game only. An attached game's map is
  changed only as [live-mirror.md](live-mirror.md) allows.
- A Preview drawn by Studio's renderer is not the game's picture. Where
  they differ, the Game panel is right.
- A kind's `applies=` lines carry evidence like any definition line.
  A field is `NotKnown` until a line says when the game reads it.

## Not verified

- That a swatch object of the stage can show any shader or bitmap as
  its subject without a reload.
- Each group's reach into the game: see the survey.

## Build slices

| # | Slice | Owns | Tests | Accepted when |
|---|---|---|---|---|
| LE1 | The registry and the fan-out: `KINDS`, `Change` and its classes from every write path, routes for Field, Raw, Shape and Host, `tag.kinds`, `class` in Pending | `studio/src/app/{kinds.rs, live.rs}`, `studio/src/app/tag_doc.rs`, `studio/src/play/tag_sync.rs`, `studio/src/api/tags.rs`, [agent-api.md](agent-api.md) | on the fake game: one write of each class makes its one delivery; an attached game gets a Pending row where the own game gets the delivery; every group has a row or the default | `tag.kinds` lists every group with its editor and preview |
| LE2 | The subject: `tag.open`, the preview placement in a level, the stage pad with no level, respawn and retrigger after `NextSpawn` and `NextUse` | `studio/src/app/subject.rs`, `studio/src/play/subject.rs`, `content-kit` (its `stage` module) (the subject pad; the swatch objects are design) | the subject's place for each rule; a `NextSpawn` write queues one respawn of the subject and of nothing else | a weapon field changed in the panel shows on the weapon standing in the Game panel |
| LE3 (built) | Bitmaps: the encoder and mips, import a PNG as a new `bitm` (`import.bitmap`), sequences and sprites (`bitmap.sprite.*`), Raw delivery | `crates/tools/bitmap-edit`, `studio/src/app/{textures.rs, bitmap_import.rs}`, [modeling.md](modeling.md) | encode then decode within the format's error (`assets::encode`); a painted pixel is one Raw write (`play::raw_sync::tests`); an import is a Shape change (`api::tests::import_tests`); a sprite edit is one undo step and moves the tag on save (`bitmaps_tests::sprites_are_added_edited_removed_undone_and_saved`) | a stroke on a bitmap shows on the model in the Game panel as it is drawn |
| LE4 | UV layouts: the UV view, island select, move, scale and pin, Raw vertex writes | `crates/tools/mesh-edit` (UV ops), `studio/src/ui/uv.rs`, `studio/src/api/mesh.rs` | an island moved changes only its vertices' UVs; one drag is one undo step | a moved island slides the texture in the Preview and the Game panel |
| LE5 | The object editor: model, variants and regions, markers, attachments, the tuning sheet; `model.*` as its API | `studio/src/app/object.rs`, `studio/src/ui/object.rs`, `studio/src/api/models.rs` | an invented object: a variant picked changes the Preview's sections; a marker moved is a Field write | a variant picked shows on the subject in the Game panel after its respawn |
| LE6 | Colour tables and gradients as editors over rich-editors C1, C3 and F1 | `studio/src/fields/rich` (the table view), `tags::effects::function` | a table edited through the view equals the same `tag.set` calls | a gradient stop dragged recolours the Preview and the game |
| LE7 | Effects, particles, lights, flares, decals: grouped editors, the game view as preview, the loop at the pad | `studio/src/app/effects.rs`, `studio/src/ui/effects.rs`, `studio/src/play/subject.rs` | the group's fields listed by part; a `NextUse` write retriggers the loop once | an effect's colour curve changed shows on the looping effect |
| LE8 | Sky and fog | `studio/src/ui/sky.rs`, the viewport's sky | a sun direction changed moves the viewport's sun | the game's sky follows |

LE1 needs nothing. LE2 needs LE1, the game view's GV3 and the stage's
S2. LE3 to LE8 need LE1 and are independent of each other; each also
needs the slice its row cites on another page, and the realtime
slices of [realtime-edits.md](realtime-edits.md) for what its edits can
reach.

## Related

- [game-view.md](game-view.md): the Game panel and the own game
- [tag-editing.md](tag-editing.md), [rich-editors.md](rich-editors.md), [modeling.md](modeling.md), [modeling-targets.md](modeling-targets.md), [hud-editor.md](hud-editor.md), [scenario-editor.md](scenario-editor.md): the editors
- [live-mirror.md](live-mirror.md), [realtime-edits.md](realtime-edits.md): the routes into a game
- [tag-workspace.md](tag-workspace.md): builds, for Shape changes
- [content-stage.md](content-stage.md): where a subject stands with no level open
