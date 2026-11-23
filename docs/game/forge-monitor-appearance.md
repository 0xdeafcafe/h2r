# Forge monitor appearance

Status: Partly built (ML1 to ML7). The look, its profile keys, its refusals, the census, the marker's projection, the offline agent methods, the presentation body (ML4: the host model list and a seat's draw — pure, the renderer's object pass does not consume the draws yet, so a disc model still shows as the marker), the session channel (ML7: the hello's declaration, the host's table, `look` and `pose` on the session's lanes, the refusals, the join reset, interpolation and staleness) and the look's page (ML6: `forge.look`'s rows and locks, the `Scene3d` preview widget, the Forge menu row) exist; the film records (ML8), the lobby row (ML9) and a real body (ML10) do not.
Code: `crates/extensions/forge/src/look.rs` and `look/` (the look, `census`, `saved`), `crates/apps/cli/src/forge_looks.rs` (the disc reader behind `h2 forge looks`), `crates/extensions/forge/src/look/monitors.rs` (the seat's look and the agent methods' rules), `crates/extensions/game/src/forge_glue/marker.rs` (the marker), `crates/extensions/game/src/forge_controls/render.rs` (the parked biped and the monitor's view), `crates/extensions/forge/src/camera.rs` (the pose), `crates/extensions/game/src/ui_kit/forge.rs` (the pages). Tests: `look/tests.rs`, `forge_looks.rs`, `forge_glue/marker.rs` and `crates/extensions/agent/src/forge/tests.rs`; each later slice names its own.
Switch: `forge`, listed in [switches.md](../runtime/switches.md). No switch of its own.

What a player looks like while they fly as a monitor in a Forge session:
the model, its colours, its tint and its light, how the player picks
them, how the other machines in the session draw the same thing, and how
a film shows it. It sits on the two monitor bodies of
[forge-features.md](forge-features.md), "Monitor mode and bodies", and on
the session of [forge-design.md](forge-design.md). The rule that must
hold: **a look is presentation**. It is never read by the tick, never
written to guest memory and never part of the sim hash, so a machine
that draws it differently, or not at all, runs the same game. The one
exception is a real body (slice F10), whose look is a log entry and is
described apart.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## How it works

### What is drawn today

- Entering the monitor parks the local player's biped: it is marked
  undamageable and left out of the render list, and the view is the
  private world view the director camera uses (**Code**,
  `forge_controls/render.rs`: `park`, `HIDDEN`, `draw_world_view`).
- The monitor is a `forge::camera::Camera`: a position, a yaw and a
  pitch. It is published to the input bridge and to `forge.status`
  (**Code**, `forge_controls.rs`: `publish`, `eye`).
- The only thing drawn at a world position is the Objects tool's gizmo,
  projected to 2D through the frame's camera and drawn by the
  compositor (**Code**, `forge_glue/gizmo.rs`: `Axes::from_frame`).
- A session is local. No other machine exists to see a monitor, and no
  film record holds its pose.

So a look has nothing to replace. It adds a drawn body where there is
only a camera.

### The look

One value per player, the same shape for every use (the picker, the
profile, the network, the film):

```
MonitorLook
  version: 1
  model: key            "marker", or a census key (below)
  variant: name?        one of the model's named variants; absent = its default
  primary: 0..17        palette index
  secondary: 0..17      palette index
  tint: 0..3            none, light, medium, strong: how far the lit colour is mixed towards primary
  light: 0..2           off, dim, bright
  light_colour: 0..17   palette index
```

- `forge::look` (in `crates/extensions/forge`) owns the type, its
  bounds, its canonical bytes (canonical JSON through
  `content::canonicalize`, as the document, with the variant left out
  when absent) and `validate`. It has no guest, GPU or disc dependency.
- A model key and a variant name are 1 to 32 and 1 to 64 characters of
  `a-z`, `0-9` and `_`. The marker has no variant, tint or light, so a
  marker look with any of them is `monitor_look_invalid`; choosing the
  marker from a disc model clears them.
- A look is changed by a `Patch` of named fields (`look::apply`): the
  bounds, then the catalogue, then the locks. A field the model locks is
  refused as `monitor_look_locked` when the patch would change it.
- Palette indices are the retail player palette's: 18 colours
  (**Code**, `live_telemetry::layout::PALETTE_COLOURS`). Their RGB is
  read from the loaded map's globals at run time, the block
  `pad_lightbar.rs` reads (`MATG_PLAYER_COLOURS`, **Code**). No colour
  value is in this repository.
- A look names its model by key, never by bytes. Nothing in it can hold
  a file.

### Which model

There are three sources, in the order they are built.

| Source | What it is | Needs | Seen by a machine without it |
|---|---|---|---|
| **Marker** | a ring and the player's name, projected to 2D and drawn by the compositor, as the gizmo is | nothing: no disc tag, no renderer feature | cannot happen |
| **Disc model** | the render model of a flying biped on a campaign map of the user's own disc, read by the host process and drawn by the native renderer at the monitor's pose | that campaign map on this machine; the native renderer | the choice is refused (below) |
| **Real body** (F10) | the biped itself, transplanted into the map; the player's unit | F10, tag headroom, a Rebuild | the document is refused at the gate (forge-design.md, "Saving and sharing on Live") |

A disc model is **not** a transplant. The tag stays in its own map file;
the host process reads it from there, as the Live art extractor reads
campaign bipeds (**Code**, `crates/apps/cli/src/live_models.rs`,
`build_ai_bipeds`) and as Theater's independent view draws posed models
without the guest (**Code**, `theater/host_view.rs`, `GpuModels`,
`native_render::instances::ObjectInput`). So it costs no tag headroom,
needs no Rebuild and works on a map with no kit.

**The census.** When a session opens, `forge::look::census` lists the
models this machine can draw:

1. Walk the campaign maps the disc mounts. For each `bipd` tag, take
   its species key by `species_of` (the tag's directory name). Every
   campaign biped is a candidate, flying or not, and the player
   bipeds are included. A tag whose directory is not a key, and the key
   `marker`, are listed as skipped with the reason.
2. Where several tags share a key, pick one by `choose_bipeds`' rule
   (the tag named like its directory, else the one on the most maps,
   else the shortest name). `species_of` and `choose_bipeds` are
   `forge::look::census`'s; `live_models` calls them, so the Live art
   extractor and the census agree on a species.
3. Resolve it: `Context::resolve_object` gives the `hlmt`, its render
   model and its variants ([objects.md](../formats/objects.md),
   "Resolver").
4. Record the entry: key, display name (the tag's base name, read at run
   time), source map and that map file's SHA-256, the variant names,
   whether any material has a change-colour map, whether the object
   attaches a light, and the node and triangle counts.

The rules, the cache and the report are `forge::look::census`, over a
`Disc` trait (campaign maps with their hashes, the biped tags of a map,
one tag described). `crates/apps/cli/src/forge_looks.rs` implements it
over the user's disc: `Context::resolve_object` for the variants and the
attached light, `live_models::read_source` for the nodes, triangles and
change-colour maps. A tag or variant the census cannot list is a
`skipped` row with its reason, never dropped.

The census is cached under `<data root>/cache` by the set of source map
hashes (`forge-looks-<key>.json`), and the latest one is also written as
`forge-looks.json`, the file the game reads as its catalogue. A game
with no `forge-looks.json` has an empty catalogue and says why (the
`catalogue_note` of `forge.look`). An entry is identified across
machines by `(key, source map SHA-256)`, never by a tag index.

**Variants.** A model with more than one named variant gets a
**Variant** row. A variant's sections come from
`native_render::model_load::pick_sections`. Where a variant region
lists several permutations the look takes the one with the highest
probability, the first on a tie (**Design**; the game draws by
`0x0016dad0`, which is not reproduced, objects.md). Damage states are
never chosen.

**The marker.** `forge_glue::marker::Marker` is the ring: its centre is
the monitor's position projected through the frame's world camera by the
function the gizmo's axes use (`gizmo::pixel_of`), its radius is a fixed
world radius projected the same way, and its label point sits above the
ring. `Marker::paint` draws the ring into the HUD canvas behind the
reticle and panels, in the primary colour read from the map's player
colours (`pad_lightbar::palette_rgb`). The name is drawn by the
compositor's text path at the label point. Nothing draws a marker yet:
a monitor never draws its own, and no other seat exists until the
session channel (ML7).

**Pose.** The body sits at the camera's position, turned to its yaw and
pitch, in the model's rest pose, with a slow vertical bob computed from
the render clock. A monitor never draws its own body in its own view:
the camera is inside it.

### Colours, tint and light

| Part | Marker | Disc model |
|---|---|---|
| Primary, secondary | the ring and the name | the model's change colours, where the census found a change-colour map; otherwise the rows are locked with "This model has no colour areas" |
| Tint | none | the lit colour mixed towards primary by a fixed alpha a step (`ObjectInput::highlight`, the editor's selection mix, **Code**) |
| Light | none | a host light at the body's origin: the palette colour, one of two fixed radii, no shadow |

- Change colours are drawn as the game tints them: per channel, one
  where the change-colour map is empty and the colour where it is full,
  red primary and green secondary ([live-art.md](../live/live-art.md),
  "Vehicle colours"). Only primary and secondary are used.
- The light is the renderer's, not the game's. It is a candidate added
  to the scene's light list after extraction with no light datum and no
  `ligh` tag (`frame_scene::LightCandidate` holds the position and
  colours a candidate needs, **Code**). It lights the world and other
  objects on machines that draw it and changes nothing in the sim. It
  casts no shadow and is skipped first when a view is over its light
  budget.
- In a team session the host may set **Monitor colours: team**. Primary
  is then the team's colour from the map's multiplayer globals (the
  block `pad_lightbar.rs` reads, **Code**) and the Primary row is
  locked with that reason.

### Choosing it

The entry is a row, **Monitor look**, in two places: the Forge menu
(Start while a monitor, the session's pause screen; **Code**,
`ui_kit/forge/dialogs.rs`, `Parent::Menu`) and the Forge lobby (F14).
Both open the same page, `forge.look`, a `Dialog` on the large base
drawn `Over` by the compositor, as the Forge dialogs and Theater's
pause menu are ([ui-builder.md](ui-builder.md), "Screen kinds", "The
value picker").

```text
 +----------------------------------------------------------+
 | MONITOR LOOK                                             |
 | Model             Sentinel                               |
 | Variant           Default                                |
 | Primary colour    <colour>              (more rows scroll)|
 | +------------------------------------------------------+ |
 | |                      preview                         | |
 | +------------------------------------------------------+ |
 | (A) Change   (X) Reset   (B) Done                        |
 +----------------------------------------------------------+
```

- **Rows.** The list shows three rows at a time and scrolls through the seven, over the preview strip (`ui_kit/forge/dialogs.rs`, `Parent::Look`): the large panel holds no more. Each row shows its value in the second column; a colour's
  name is the game's own string for that palette index, read at run
  time. A opens the
  kit's value picker over the page (`ui_kit::picker::offer_list`) with
  the row's values and one line of description. No row steps sideways
  and none draws arrows.
- **Locked rows.** A row that cannot be changed shows its value and its
  reason as the description: no colour areas, team colours, a model
  with one variant. A model the census lists but a peer cannot draw
  stays in the Model picker, greyed, with the refusal's sentence.
- **Preview.** A new `View` widget, `Scene3d {id, rect}`: the look drawn
  by the native renderer into an offscreen target (the model, its
  colours, tint and light, a slow turn, a neutral backdrop) and placed
  in the rect by the compositor. `h2 ui preview forge.look` draws the
  rect with the model's name in it, since the offline preview has no
  GPU scene; the kit's checks treat the widget as art.
- **Preview follows focus.** While a picker is open the preview shows
  the focused value, not the held one. `Pick` gains an `on_focus`
  callback for it. B restores the held value; A sets it.
- **Applying.** A in a picker sends the changed look. Offline it is set
  at once. In a network session it is a proposal to the host, and the
  row keeps the old value until the host answers; a refusal shows its
  sentence in the dialog's description line and in the HUD strip.
- **The game's Appearance page.** When the player's look is the Forge
  monitor, the page's own model preview draws the monitor beside the
  Spartan (ML4b, decision 8).
- **Reset** (X) is the default look: the marker, the seat's colour, no
  tint, no light.

### Where it is saved

Per profile, in `settings.toml`, through the store's per-profile values
(`settings::Store::profile_get` and `profile_set`, **Code**;
[settings-menu.md](settings-menu.md), "Settings as a schema"):

```toml
[profiles."<name>".forge]
monitor_model = "sentinel"
monitor_source = "<source map SHA-256>"
monitor_variant = ""
monitor_primary = 3
monitor_secondary = 0
monitor_tint = 1
monitor_light = 1
monitor_light_colour = 3
```

- The five numeric keys (`monitor_primary`, `monitor_secondary`,
  `monitor_tint`, `monitor_light`, `monitor_light_colour`) are
  `settings::Def`s with `Scope::View`: they change presentation only.
  Tint and light are choices (`none light medium strong`, `off dim
  bright`) and are written by name. They are in `forge::look::saved::DEFS`
  and listed in `setting_defs::ALL`, with no menu row, environment
  variable or flag.
- The three names (`monitor_model`, `monitor_source`, `monitor_variant`)
  are read and written as text through `Store::profile_text` and
  `profile_text_set`, because a `Def` holds only a bool or a whole
  number. An empty text is "none".
- A missing key is its default: the colours are the seat's colour, tint
  and light are none and off.
- The file follows the account through cloud saves with the other
  per-profile settings ([live-cloud-saves.md](../live/live-cloud-saves.md)).
- A saved look whose model this machine's census lacks, or has from
  another source map, is not replaced. It is kept in the file, the
  session starts with the marker in the saved colours, and the HUD strip
  and the page say why once ("Sentinel needs a campaign map this install
  does not have"). A variant the model no longer has is said the same
  way. An out-of-range value is reported by key, as any setting is, and
  a bad name is reported by key; neither is rewritten.

### Other players

A look and a pose reach the other machines on the Forge session's own
channel, beside the edit log (forge-design.md, "The edit log and
multiplayer"). They are not log entries: the log is the document's
history, and a look edits no map.

```
look:   {seat, look, base?}             peer -> host, reliable ordered
look:   {seat, look}                    host -> every peer, reliable ordered
pose:   {seat, position, yaw, pitch, monitor: bool, held: OpId?}
                                        peer -> host -> peers, unreliable, at most 15 a second
```

- **The host owns the table** of seat to look. A peer proposes; the host
  validates and sends the accepted look to everyone, the proposer
  included. A machine draws only looks the host sent.
- **What each machine can draw** is declared in the session hello: the
  look version, whether the native renderer is on, and the SHA-256 of
  each census source map it holds. The host keeps these per machine.
- **A choice everyone cannot draw is refused**, to its author, with a
  named reason and the machine it failed on. The seat keeps its
  previous look.

| Refusal | When |
|---|---|
| `monitor_look_invalid` `{field, reason}` | a field out of bounds, an unknown key, a model or variant the catalogue lacks |
| `monitor_look_source_missing` `{machine, map}` | a machine in the session does not hold the model's source map at that hash |
| `monitor_look_renderer` `{machine}` | a machine in the session runs without the native renderer |
| `monitor_look_locked` `{field, reason}` | the look changes a field the model or the session locks: the host's team colours lock primary, a model with no colour areas locks both colours, one variant locks the variant, the marker locks variant, tint and light |
| `monitor_look_version` | at join: the joiner's look version is not the host's; the join is refused before the map loads |
| `monitor_look_no_session` | a set outside a Forge session through the agent API (the menu writes the profile instead) |

- **A joiner who cannot draw a look in use** does not join into a
  session that silently differs. The host resets each affected seat to
  the marker with its colours kept, sends that look to everyone, and
  tells the affected player in the HUD strip: "Monitor look reset:
  \<gamertag\> does not have \<map\>". The saved profile value is left
  alone, so the look returns in the next session.
- **Poses** are interpolated between samples on the render clock and
  dropped when stale by two seconds; the body then fades out, and the
  seat shows as "not seen" in `forge.status`. `monitor: false` (the
  player is back in their biped) hides the body at once.
- **Both network modes** carry the channel unchanged. Synchronous (F5)
  and host-replicated (F15) differ in how the sim is kept equal; the
  look is in neither.
- **Retail-compatible sessions are untouched.** A Forge session is not
  one: every machine in it runs Forge. A match on a published map has no
  monitors, sends no look or pose message and gives every player their
  own biped (forge-design.md, "Rules and limits").

### A real body

With slice F10 a monitor can be a transplanted biped. Its look is then
part of the game:

- The model is the `transplant` recipe and the unit swap. Choosing it is
  a Rebuild, offered in the same Model picker under a **Bodies** heading
  with the pending badge.
- Its colours are the unit's change colours in guest memory, which the
  object update evaluates (`OBJECT_CHANGE_COLORS`, **Code**,
  `crates/hooking/guest-layout/src/points.rs`). Setting them is a Forge
  `body` op in the log, applied by every machine at the entry's tick, so
  the sim hash stays equal.
- Its light is whatever the biped's own tags attach. The look's `light`
  and `tint` rows are locked for a real body.
- The presentation body is not drawn for a seat that has a real one.

### Theater

A film of a Forge session already records each log entry under a Forge
tag (forge-design.md, "Films and determinism"). That tag's payload
starts with a subtype byte: 0 is a log entry, 1 a look, 2 a pose.

- The recorder writes a look record when the host's table changes and a
  pose record for every monitor seat at 15 a second, its own included.
  Each seek checkpoint repeats the table and each seat's last pose, as
  tag 3's baselines do ([film-format.md](film-format.md), "Tag 3
  presentation records").
- Playback draws monitors from these records with the code the live
  session uses. They are outside the gameplay digest, so a film
  verifies the same with or without them.
- `theater.camera {mode: "follow" | "orbit", seat}` on a monitor seat
  follows its body; first person on that seat is the monitor's own
  camera pose, with no body drawn.
- A playback machine that cannot draw a recorded look says so and draws
  the marker in that look's colours: `theater.compat` gains the reason
  `monitor_look_source` with the seat and the map, the film's detail
  page lists it, and the Theater bar shows it once when the seat first
  appears. Playback is not refused, because the simulation is whole.
- A real body needs nothing here: it is a unit in the film's own state.

## Interfaces

**Agent API** (game), in [agent-api.md](../runtime/agent-api.md),
"Local Forge session". Every look test, lane and tool reads and sets
looks through these. They are in `crates/extensions/agent/src/forge/local.rs`
and ride the `forge` feature, like the rest of that section.

| Method | Access | Parameters | Result |
|---|---|---|---|
| `forge.look` | read | `{seat?}` | `{session, look, drawn_as, saved, catalogue: [{key, name, source, sha256, variants, colours, light, available, refusal?}], locks}`; with no session, the profile's saved look and this machine's catalogue |
| `forge.look.set` | act | any of the look's fields; `reset: true`; `save: true` | `{look, changed, saved, drawn_as}` as accepted, or the refusal with its code and fields |
| `forge.status.monitors` | read | | `{monitors: [{seat, name, monitor, pose, look, drawn_as, peers_see, seen_ticks_ago}]}` |
| `forge.look.preview` | act | `{look, path?, size?}` | `{path, width, height}`: the preview target as a PNG under the data root, `NO_GPU` where there is none (ML6) |

`drawn_as` is `model`, `marker` or `hidden`, with the reason when it is
not what the look asks for. `peers_see` is the same for another
machine. A set changes the session's look; it writes the profile only
with `save: true`, so a test does not rewrite the player's settings. The
set methods speak for the profile signed in on local slot 0 and take its
primary colour as the seat's colour; with no profile that is the
`default` profile and palette colour 0. `ui.screen` reports the `forge.look` page
and its `Scene3d` widget by id like any kit page.

**CLI.** `h2 forge looks [--disc <iso>]` prints the census. `h2 ui
preview forge.look` draws the page's states.

**Settings.** The eight per-profile keys above, in `boot::setting_defs`.

**Session channel.** The `look` and `pose` messages and the hello's three
fields, in the F5 transport.

**Film.** Subtypes 1 and 2 of the Forge tag, in
[film-format.md](film-format.md) when built.

**Renderer.** A host model list beside the map's models (a model named
by census entry, not by tag datum), presentation instances and light
candidates appended to the `FrameScene` after extraction
([framescene.md](../graphics/framescene.md) 4.5), and the offscreen
preview target.

**Live.** None. No roster field, event or route carries a look.

## Rules and limits

- A look is never read by the tick, written to guest memory, saved in a
  checkpoint or snapshot, or folded into a hash. `ForgeExt` holds none
  of it.
- A look is not in the Forge document. A published map carries no
  monitor and no look.
- No byte of a model leaves the machine: a look names a key and a
  source map hash, and each machine draws from its own disc.
- No quiet fallback. Every case where a machine draws something other
  than the chosen look has a named reason shown to the affected player:
  the refusals above, the join reset, the saved look this install
  cannot draw, and the Theater reason.
- Imported models ([forge-features.md](forge-features.md), "Character
  import") are not look sources in this design. They are skins on a
  chassis, and a monitor has no chassis until F10.
- The census asks only campaign maps, so it never offers a model from a
  multiplayer map.
- At most 16 presentation bodies and 16 host lights exist, one a seat.
- The look is not sent to Live and does not appear on a stats page or
  in the web viewer. A shared film carries it in its Forge records,
  which only the game's Theater reads.

## Slices

Sizes as forge-design.md, "Features": **S** under 800 lines with tests,
**M** under 2,500, **L** more. Lanes write code and tests on invented
data.

| # | Slice | Size | Needs | Acceptance |
|---|---|---|---|---|
| **ML1** | `forge::look`: the type, bounds, canonical bytes, `validate`, the refusal codes, the eight `Def`s and the profile read and write | S | nothing | every bound has a refusing case; canonical bytes have golden vectors; a profile round-trips through `settings::Store`; an unknown saved model is kept in the file and reported |
| **ML2** | The census and `h2 forge looks`: species rule, source hashes, variants, colour and light capability, the cache | M | ML1 | on invented maps: one entry a key, the `choose_bipeds` tie rules, a variant with a weighted region picks by the stated rule, an entry's identity is unchanged when tag indices move |
| **ML3** | The marker and `forge.look`, `forge.look.set`, `forge.status.monitors` offline | S | ML1 | over a fake game: set, reset and each local refusal; the marker's projection matches the gizmo's for the same frame |
| **ML4** | Disc models drawn: the host model list, a presentation instance at a pose, change colours, tint, the bob, hidden in the owner's view | L | ML2 | a scene built from an invented model holds one extra instance at the pose with the expected colour inputs; the owner's view has none; the sim hash of a run with and without a look is equal |
| **ML4b** | The game's own Appearance page: when the player's look is the Forge monitor, the monitor is drawn beside the Spartan in the page's model preview | M | ML4 | in a widget view built from invented data, a monitor look adds one presentation instance beside the biped with the look's colours; any other look, or none, leaves the widget's draws as retail; the sim hash of a run with and without it is equal |
| **ML5** | The host light | M | ML4 | a candidate with no datum is in the view's light list, last in budget order, with the palette colour; no shadow map is made for it |
| **ML6** | The page: `forge.look`, locked rows, the `Scene3d` widget, `Pick::on_focus`, the Forge menu row, `forge.look.preview` | M | ML3; ML4 for a model in the preview | `Page` tables for every row and lock; the kit's checks pass in every state; focus changes the preview and B restores it |
| **ML7** | The session channel: hello fields, the host's table, `look` and `pose`, the refusals, the join reset, interpolation and staleness | M | F5, ML3 | a fake host and two peers: a look one peer lacks is refused naming it; a joiner without a source resets the seat with the notice; reordered poses never move a body backwards |
| **ML8** | Film and Theater: subtypes 1 and 2, checkpoints, playback draw, follow and first person on a monitor seat, the `theater.compat` reason | M | F5's film record, ML4 | a recorded session replays to the same table and poses at every tick after a seek; the gameplay digest is unchanged; a player without the source reports the reason |
| **ML9** | The lobby row and team colours | S | F14, ML6 | the lobby opens the page; the team lock holds and names itself |
| **ML10** | A real body's look: the Bodies heading, the `body` op's colours, locked rows | M | F10, ML6 | the op writes the unit's change colours over a fake game and folds through the log; two fake peers hash alike |

ML1, ML2 and ML3 are built. ML4 is the first that needs the
native renderer's object path; ML4b follows it. ML7 and ML8 wait for F5; ML9 for F14;
ML10 for F10.

## Decisions

The owner's choices, each with the alternative not taken.

1. **A presentation body, not a real body first.** A disc model is drawn
   by the host process from the disc (no transplant, no Rebuild, ML4).
   Waiting for F10's transplanted biped is not taken; F10 adds a real
   body beside it.
2. **A peer that cannot draw a look.** The choice is refused, and a
   joiner who cannot draw a look in use resets that seat on join with a
   notice. Refusing the peer's join is not taken.
3. **Which models.** Any campaign biped the census finds, not only
   flying machines. Narrowing to the Sentinel family and the monitor is
   not taken.
4. **Default colours.** The seat's colour. The profile's own colours,
   pending validation of their location
   ([live-gamestate.md](../live/live-gamestate.md), "Local player slots
   and profile appearance"), are not used.
5. **The light.** A real light on the world (ML5). A glow on the body
   only is not taken.
6. **Theater without the source.** The film plays with the marker and a
   named reason. Refusing the film is not taken.
7. **Live.** Nothing is sent. A monitor in the web viewer from the art
   pack's per-species models (live-art.md, "Player and weapon models")
   is not taken.
8. **The Appearance page.** On the game's own Appearance page, when the
   player's look is the Forge monitor, the monitor is drawn beside the
   Spartan in the preview (ML4b). Showing the look only on the Monitor
   look page is not taken.

## Evidence

- **Code**: the monitor is a camera and the biped is parked and hidden,
  `forge_controls/render.rs` and `forge/src/camera.rs`.
- **Code**: campaign maps are read for one biped a species by directory
  key, `live_models::species_of`, `choose_bipeds`, `build_ai_bipeds`;
  Sentinels are among the campaign characters the extractor poses as
  bipeds (live-art.md's list of characters with no rifle stance).
- **V**: the `hlmt` variant layout and the default-variant rule, with
  its census of the retail disc, objects.md, "`hlmt` object models" and
  "Validation on the retail disc".
- **Code**: `pick_sections` picks a variant's sections;
  `ObjectInput::highlight` mixes the lit colour towards a colour.
- **Code**: models are drawn without the guest in Theater's independent
  view, `theater/host_view.rs`.
- **Code**: the palette blocks in the map's globals, `pad_lightbar.rs`.
- **Code**: per-profile values, `crates/formats/settings/src/store.rs`.
- **S**: that the monitor and each Sentinel is a `bipd` on a campaign
  map, and which maps hold them (forge-features.md). `h2 forge looks` on
  the owner's disc verifies it; its counts go in a note.
- **S**: that the Sentinel and monitor shaders have change-colour maps.
  The census reports it for each model; a model without one keeps its
  disc colours and offers tint only.
- **S**: which shader parameters take the change colours in the native
  object path ([agent-api.md](../editor/agent-api.md), "`model.*`", "To
  establish first"). ML4 needs it; without it colours are locked and
  tint stands in by the player's choice, not by default.
- **S**: that the two monitor characters differ by a variant, a
  permutation or a shader (forge-features.md). The census's variant
  list answers it.
- **Design**: the look's fields, the marker, the host's table, the
  refusals, the join reset and the film subtypes.

Gaps, named:

- The retail models carry no idle pose a rest pose shows well. An idle
  clip from the model's animation graph is not in these slices.
- The Enforcer is a vehicle, not a biped (live-art.md, "Pipeline"), so
  the species rule does not offer it.
- A light candidate without a guest record has no path in the native
  light walk today.
- The native object path names a model by tag datum and map epoch
  (framescene.md 4.2). A model from another map file needs the host
  model list.
- A machine without the native renderer draws the marker only, and
  holds every other seat in its session to it.

## Not verified

- Every **S** above.
- That a presentation instance appended after extraction draws with
  correct depth against the world, the first-person pass and
  transparents.
- How a disc model looks with the lighting the scene gives an instance
  that has no object lighting record.
- The cost of 16 host lights in a view.
- The `Scene3d` rect composited over a kit dialog in game, and over the
  Forge lobby.
- Pose traffic and smoothness at 15 a second on a real link.
- The census's time on a full disc.

## Related

- [forge-design.md](forge-design.md): the session, the log, the two
  network modes, films, the slices F5, F10, F14 and F15.
- [forge-features.md](forge-features.md): "Monitor mode and bodies",
  "Character import".
- [forge.md](forge.md): retail evidence.
- [ui-builder.md](ui-builder.md): pages, the Dialog kind, the value
  picker.
- [settings-menu.md](settings-menu.md): per-profile settings.
- [objects.md](../formats/objects.md): object models and variants.
- [framescene.md](../graphics/framescene.md),
  [native-objects.md](../graphics/native-objects.md): the scene and the
  object path.
- [film-format.md](film-format.md), [theater-design.md](theater-design.md):
  tagged records and the player.
- [live-art.md](../live/live-art.md): the per-species models and
  change-colour rules.
