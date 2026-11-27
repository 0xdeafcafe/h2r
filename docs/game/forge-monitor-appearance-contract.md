# Forge monitor appearance: interfaces, slices and evidence

Interfaces, rules, slices, decisions, evidence and open items. Part of [forge-monitor-appearance.md](forge-monitor-appearance.md).

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
| **ML3** | The marker and `forge.look`, `forge.look.set`, `forge.status.monitors` offline | S | ML1 | over a fake game: set, reset and each local refusal; the navpoint over another seat's monitor is at its pose |
| **ML4** | Disc models drawn: the host model list, a presentation instance at a pose, change colours, tint, the bob, hidden in the owner's view | L | ML2 | a scene built from an invented model holds one extra instance at the pose with the expected colour inputs; the owner's view has none; the sim hash of a run with and without a look is equal |
| **ML4b** | The game's own Appearance page: when the player's look is the Forge monitor, the monitor is drawn beside the Spartan in the page's model preview | M | ML4 | in a widget view built from invented data, a monitor look adds one presentation instance beside the biped with the look's colours; any other look, or none, leaves the widget's draws as retail; the sim hash of a run with and without it is equal |
| **ML5** | The host light | M | ML4 | a candidate with no datum is in the view's light list, last in budget order, with the palette colour; no shadow map is made for it |
| **ML6** | The page: `forge.look`, locked rows, the `Scene3d` widget, `Pick::on_focus`, the Forge menu row, `forge.look.preview` | M | ML3; ML4 for a model in the preview | `Page` tables for every row and lock; the kit's checks pass in every state; focus changes the preview and B restores it |
| **ML7** | The session channel: hello fields, the host's table, `look` and `pose`, the refusals, the join reset, interpolation and staleness | M | F5, ML3 | a fake host and two peers: a look one peer lacks is refused naming it; a joiner without a source resets the seat with the notice; reordered poses never move a body backwards |
| **ML8** | Film and Theater: subtypes 1 and 2, checkpoints, playback draw, follow and first person on a monitor seat, the `theater.compat` reason | M | F5's film record, ML4 | a recorded session replays to the same table and poses at every tick after a seek; the gameplay digest is unchanged; a player without the source reports the reason |
| **ML9** | The lobby row and team colours | S | F14, ML6 | the lobby opens the page; the team lock holds and names itself |
| **ML10** | A real body's look: the Bodies heading, the `body` op's colours, locked rows | M | F10, ML6 | the op writes the unit's change colours over a fake game and folds through the log; two fake peers hash alike |

ML1 to ML5, ML7 and ML8 are built; ML6's rows are, and its screen is not
(the screen kind in `forge_screens`, `Pick::on_focus`, the Forge menu
row, the `Scene3d` draw in a dialog and `forge.look.preview`). The
bodies stage, the Appearance page's picture and the host light are
drawn code that no run has seen. A body draws from a campaign map's
model read at run time, in the model's default variant and without
change colours; a model this install cannot read draws the marker, and
so does a seat whose model has not loaded. ML8 records and replays the
look, and a playback machine draws the marker for a look whose source map
it does not hold.
ML9 waits for F14; ML10 for F10.

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
- A host light lights the structure and its placements, not objects:
  the passes' model parts are chosen from guest lists a host light has
  not. The light's near radius is zero and its far radius the look's, an
  attenuation the retail `ligh` data does not give a body.
- The native object path names a model by tag datum and map epoch
  (framescene.md 4.2). The bodies stage does not use it: it holds its own
  models, read from the entry's map, so a body is drawn by the simple
  skinned pass (`model_gpu`), not the game's lit object program. A body
  has no object lighting and takes the pass's fixed ambient and one
  directional light.
- A machine without the native renderer draws the marker only, and
  holds every other seat in its session to it.

## Not verified

- Every **S** above.
- That a body of the bodies stage draws with correct depth against the
  world, the first-person pass and transparents, in the markers' place
  (after the post passes, before the overlay), at every internal
  resolution.
- That a model's node frames under the pose (`bodies::nodes`) put every
  biped model upright and facing along its yaw, and which models a
  census entry's tag rule reads.
- The Appearance screen's name. The picture draws only when the active
  screen is `monitor_model::APPEARANCE_SCREEN`, which is `None`, so it
  draws nowhere yet and no other screen's model widget (the lobby's
  players, the profile preview) gets it. To set it: open the page in a
  running game, run `h2 agent ui.stack`, read the top screen's tag name,
  put it in `APPEARANCE_SCREEN` and give `monitor_model::gpu::active_screen`
  a reader of the same screen.
- That the Appearance page's picture shows in the widget's right share
  at the page's own widths, and which model widgets offer a view.
- That the models read in the background do so in the mounted game
  without a hitch, and what the first read costs.
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
