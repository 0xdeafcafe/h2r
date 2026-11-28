# HUD kit: how it works

The mechanism: how the kit builds and draws the HUD. Part of [hud-kit.md](hud-kit.md).

## How it works

### The model (`crates/rewrites/hud-kit-core`)

The model, the HUDs' states, the template resolver and the record
compiler are in `hud-kit-core` (`rewrites` domain, guest-free), which
both the offline preview (`crates/tools/hud-kit`) and the game's runtime
use; `hud_edit::Mode` is its `Mode`. A HUD of ours is a `Hud`: a name
and a fixed list of elements. A state
is a `Values`: per element its text, fill, angle, style and whether it
shows. One layout per HUD: a state never adds, removes or moves an
element (the UI kit's rule, for the same reasons: the runtime only
answers hooks, and the checks cover every state).

### HUDs as data

A HUD's elements (its layout) are YAML, the `hud` item of a package
([studio-redesign.md](../editor/studio-redesign.md), "Contents"); its
states (`View`, `values`, the preview's samples) are Rust, in
`hud-kit-core/src/views/`. The YAML is `name` and `elements`, each with an
`id`, a `kind`, and what the kind takes (`art`, `font`, `justify`,
`longest`, `glyphs`, `shape`), `at` (anchor, the offsets for each screen
mode, registration, origin), `over`, `hide_when_d` and `outside`; the
keys and what each kind needs are in `hud-kit-core/src/data.rs`, which
reads one into a `Hud` and refuses an unknown key or an incomplete element
by name. The first-party HUDs are in `content/superbounce/huds/`
(`superbounce`, `superbounce-readout`, `pill-block`), `content/forge/huds/`
(`forge`, `forge-readout`), `content/modes/huds/` (`firefight`) and
`content/core/huds/` (`theater`, [the playback HUD](theater.md#the-playback-hud)); `data::BUILTIN`
embeds them and each `views::*::hud()` reads its own, so the preview, the
checks and the runtime all see the YAML. The `hud` builder
(`crates/tools/workspace/src/hud.rs`) refuses a source that is not a HUD
and adds no tag: the game compiles a HUD's records against the map it
loads, so a package of HUDs alone leaves every map unchanged
(`workspace::changes_map`). `workspace::huds` lists a set's HUDs.
`hud-kit-core/src/expected.rs` keeps the Rust each HUD was built with and
`data_tests.rs` holds the YAML to it element by element.

```rust
pub struct Hud { pub name: &'static str, pub elements: Vec<Element> }

pub struct Element {
    pub id: &'static str,
    pub kind: Kind,
    pub at: Place,
    /// Drawn on that element by design (a count on its pill): the fit and
    /// own checks let the two overlap.
    pub over: Option<&'static str>,
    /// Word D bits that hide it (as a retail widget's `hide_when_d`): ours
    /// gives way while the game draws a retail widget in its place.
    pub hide_when_d: u16,
    /// Pixels it may sit outside the HUD frame, in the screen's margin
    /// (the `frame` check); 0 for all but the charge segments.
    pub outside: i16,
}

pub enum Kind {
    /// A line in a HUD font. `longest` is the widest text any state gives it.
    Text { font: Font, justify: Justify, longest: &'static str },
    /// A strip that fills from the left with `Value::fill`, 0 to 1.
    Meter { art: Art },
    /// A sprite of a retail HUD bitmap.
    Icon { art: Art },
    /// An icon turned to `Value::angle` about its centre.
    Pointer { art: Art },
    /// A picture of ours that fills with `Value::fill` in steps: a text
    /// widget whose one character is the picture for the fill ("Gauges").
    Gauge { glyphs: Glyphs },
    /// The channels (below).
    Message { longest: &'static str },
    Prompt { longest: &'static str },
    Training { longest: &'static str },
    Navpoint { longest: &'static str },
    WorldMark { shape: Shape, longest: &'static str },
}

/// Anchor and offsets in HUD pixels, per screen mode. `None` hides the
/// element in that mode.
pub struct Place {
    pub anchor: Anchor,
    pub full: (i16, i16),
    pub half: Option<(i16, i16)>,
    pub quarter: Option<(i16, i16)>,
    pub registration: [f32; 2],
    pub origin: Origin,
    /// What the half and quarter offsets are from, when not `origin`.
    pub split_origin: Option<Origin>,
}

/// What the offsets are from: the anchor's corner, or where the player's
/// grenade row puts a next pill or a next count (`templates::GrenadeRow`).
/// A place can be from its anchor fullscreen and from the row in split
/// screen (`Place::split_from`); a widget has one anchor, so the place's
/// is then the row's (`records::placed`).
pub enum Origin { Anchor, NextGrenadePill, NextGrenadeCount }

pub enum Font { Hud, Lcd }            // a text widget's font index 0 and 1
pub enum Art {
    StripMeter,                       // the sword's battery strip and its shader
    StripMeterEnd(u8),                // the strip's last n columns, cropped, with its blue ramp
    WaypointArrow,                    // hudg arrow_assets[0]
    NavpointArrow,                    // hudg arrow_assets[1], the navpoints' arrow
    GrenadePill(PillPart),            // a part of the player's frag pill (shown on word A 0x04 alone)
    Widget { hud: HudRef, name: &'static str }, // a retail widget's bitmap, shader and sequence, by its name string
}
pub enum HudRef { Player, Named(&'static str) } // the player's nhdt, or one by tag name
pub enum PillPart { Body, End }       // the pill's first PILL_BODY (20) pixels, its last PILL_END (7)
/// A gauge's pictures: `steps + 1` characters of ours in the fonts, from
/// `first`, the empty one, to the full one, each `size` pixels.
pub struct Glyphs { pub first: u16, pub steps: u8, pub size: (i16, i16) }
pub enum Style { Normal, Dim, Inactive, Flash(Pulse), Go, Team(u8) } // Go, Team: a world mark's alone
pub enum Pulse { Help, Objective, TimeWarning, TimeExpired } // the hudg records

pub struct Value {
    pub shown: bool, pub text: String, pub fill: f32, pub angle: f32, pub style: Style, pub since_tick: u32,
    pub motion: Motion, // how the fill and visibility move after a change, on wall time (below)
    pub from: f32,      // the runtime's: the fill shown when the value changed
}
pub enum Motion { None, Ease, Ending, RunOut, Blink, Arrive }
pub struct Values(pub Vec<(&'static str, Value)>);

pub trait Describe {
    fn hud(&self) -> Hud;
    /// The named states the preview sheet draws.
    fn states(&self) -> Vec<(&'static str, Values)>;
    /// Why the HUD is not drawn on a campaign map, if it is not.
    fn multiplayer_only(&self) -> Option<&'static str> { None }
    /// The sample poses of the HUD's world marks in a state, for the
    /// preview and the checks.
    fn world(&self, _state: &str) -> Vec<(&'static str, Pose)> { Vec::new() }
    /// A monitor's HUD: the retail widgets but MONITOR_KEEPS are off for
    /// its user, and its grenade row origins are the row's first place.
    fn monitor(&self) -> bool { false }
}
```

`records::compile(&Hud, &Templates) -> Result<Compiled, Error>` turns
the elements into widget records: `0x64` bytes per meter, icon and
pointer and `0x54` per text and gauge, with the element id of each, bitmaps first
and texts after, as the game draws them (E `0x0013a532`). It writes the
fields at the offsets of the `nhdt` definition (`tags::defs`) that
`hud_edit::layout` reads, and `hud-kit`'s tests read every compiled
widget back through `hud_edit::model::Widget`. `Templates` holds
what must come from the loaded map: for each `Art` the bitmap and shader
datums and the sequences, and for text the shader and string ID of a
retail text widget of the same `nhdt` (the grenade count: the first text
widget whose first input is 20). `templates::resolve` reads the map
through a `MapView`: `hud_kit::templates::EditView` over the HUD
editor's model and renderer for the preview, and
`hud_kit_core::map_file::MapFile` over the map file and its resources
for the runtime; the disc test resolves both on every map and compares
them. The one difference is deliberate: `MapFile` resolves no arrow
(`map_file::NO_ARROWS`), as the runtime's navpoint is the game's own and
the runtime checks only the widgets' templates
(`Templates::missing_widgets`). The strip meter is found by the names
`h1objects::health_hud` uses (`disintegrator_meter`, `ammo_meter`,
sequence 0). An element placed from the grenade row takes its anchor
and offsets from the row's (`compile::placed`): the next pill or count
(below, "The pill"), plus the element's own offsets. An `ArtTemplate`
may carry a crop, `[u0, u1, v0, v1]` of its sprite: the part a
`GrenadePill` part draws. The crop is not in the widget record; the
`Compiled` widget carries it, the preview draws it
(`hud_edit::render::Answers::crop`, `Sprite::cropped`) and the runtime
narrows the quad's texture rectangle to it. A cropped meter's template
also carries its blue ramp, `[b0, b1]` (`templates::ramp`): the blue of
the crop's first column and of its last plus one column's step, read
from the strip's middle row; `records::Widget::input(fill)` is
`b0 + fill × (b1 − b0)`, the `hud_input1` the preview draws and the
runtime gives. The same `Compiled` bytes go
to the game and to the preview, so what is checked offline is what the
game is given.

What the bytes leave to the runtime's hooks: the four show words are
zero, so the retail gate (E `0x0013ac87`) never draws a kit widget on its
own; every input is 0 ("zero"), and the input hook gives `hud_input1`;
a text's string is the template's, and the string hook gives the text,
for a gauge the character of its picture (`records::Widget::text`).
A text hidden in a screen mode has the font byte -1 there, so the game's
own text draw skips it (E `0x0022a85b`). A hidden mode keeps the
fullscreen offset.

**Motion** (`hud_kit_core::motion`). A HUD's logic stays per tick: it
publishes a target fill and a `Motion` at 30 Hz. The draw hooks run once
per drawn HUD, and each asks the curve at the milliseconds since the
value changed, so a curve is the same at 30, 60 or 120 frames a second,
only sampled more often (docs/game/high-framerate.md). The curves are
pure functions of those milliseconds: `fill_at`, `shown_at` and
`scale_at`, and `animate(&Value, ms, scaled)` for a whole value.

| Motion | Fill | Shown | Scale |
|---|---|---|---|
| `None` | the fill | yes | 1 |
| `Ease` | from `from` to the fill in 200 ms, smoothstep | yes | 1 |
| `Ending` | from `from` to full in 130 ms | off 290 to 360 ms; gone at 600 ms (at 430 ms unscaled) | 130 to 290 ms a swell to 1.12 wide and 1.30 tall and back (a half sine); from 430 ms the width closes to 0 by 600 ms, the height kept |
| `RunOut` | from `from` to 0 in 200 ms | gone at 300 ms | 1 |
| `Blink` | the fill | on and off in sixths of 500 ms, three times on; gone at 500 ms | 1 |
| `Arrive` | the fill | on and off in sixths of 500 ms, three times on; shown from 500 ms for good | 1 |

Every step is at least 60 ms, so at 30 frames a second a blink or the
swell still gets two frames. `motion::restamp` decides a stamp: a change
of shown, fill or motion is stamped from the fill the old value was
showing then (0 if it was not showing); the same value again keeps its
stamp and its `from`. Text and style alone are no change.

Six more elements are not widgets. Five are channels into the game's
message, training and navpoint parts, one text each, and the sixth is a
world mark, drawn by the window ("World marks"):

| Element | What the player sees | Through |
|---|---|---|
| `Message` | a notification in the pickup list, top left, that fades | the notification queue (`0x0024cbbf`) |
| `Prompt` | the prompt line, top left, as "Hold X to pick up" | the user's prompt fields (`+0x220`, `+0x440`, `+0x444`), while the game shows no prompt of its own |
| `Training` | the framed box in the middle of the screen | the scripted training text (`+0x6c0`, `+0x6c4`) |
| `Banner` | one line centred above the frame's bottom, over the respawn timer, for three seconds, fading | each local user's timed centred text (`+0x454`, `+0x4d8`, `+0x4dc`), when the text that user's values show changes |
| `Navpoint` | the game's arrow pointing down at a point of the world and its waypoint marker above | the navpoint list (`0x0024da7d`, below in "The navpoint") |
| `WorldMark` | lines lying on the level's geometry, a label over them | the window's world-line pass (`hud_kit_world`, below in "World marks") |

Their texts are UTF-16 with the font's button glyphs or action tokens.
A channel, like a widget, shows only in the screen modes its place has:
`Element::channel` gives it `Place::GAME`, fullscreen only.

A notification is an event, not a state: two bounces are two lines with
the same text. So a `Message` is sent with `Handle::notify(user, text)`,
once per line, for a local user, and not by `publish`. Its element is in
the `Hud` for the checks and the preview, whose states give it a text
(`Value::text`); `longest` is the widest line it is ever sent. A text
of at most 62 UTF-16 units and one line is sent whole; a longer one, or
one with a line break, is refused by the `prompt` check and by
`Handle::notify` (`hud_kit_core::model::message_fault`; E `0x0024cbee`
keeps 62). A sender with no HUD of its own, such as a rules script's `message`
([rules-world-api.md](rules-world-api.md#the-show-channel)), queues the
same line through `hud_kit::notify(user, text)`.

A `Banner` is a state, unlike a notification: the HUD publishes its
text while the line should show (a wave's number while the wave is
being placed), and the runtime writes the line each time the shown text
changes. The game draws it for `model::BANNER_TICKS` (90, three seconds;
Design), fading, and then it is gone; the same text published again
keeps the line it started, and a hidden value lets it run out. A text of
at most 62 UTF-16 units (`model::BANNER_UNITS`: the game's copy keeps
one fewer than the 63 it is given) and one line is written whole, as a notification's; a
longer one, or a line break, is refused by the `prompt` check and not
written (`model::banner_fault`). The preview draws it centred across the
frame with its top 40 pixels above the frame's bottom
(`preview::BANNER_UP`), in the notifications' colour.

### World marks

A world mark is an element of a HUD that lies on the level's geometry
and not on the screen: a landing marker, a box round a place to land in.
It is in the `Hud` with the widgets and the channels, so a state shows
or hides it, styles it and gives it a label like any other element, and
the checks and the preview cover it. `hud_kit_core::world` has its
shapes and its pose:

```rust
pub enum Shape { Pipper, Bracket, Arrow, Ticks, Bar, Line, Segment }

pub struct Pose {
    pub at: Vec3,          // the point on the geometry
    pub along: [f32; 2],   // the heading, flat: a bracket's long side, an arrow's way
    pub half: [f32; 2],    // the size, world units
    pub strength: f32,     // 0 to 1: the lines' alpha, for a mark that fades
    pub to: Option<Vec3>,  // a Segment's far end
}
```

| Shape | Lines | `half` |
|---|---|---|
| `Pipper` | a ring of 12 segments with a cross at its centre | the ring's radius, unused |
| `Bracket` | the four corner brackets of a box, each arm half the box's shorter half | along the heading, across it |
| `Arrow` | a shaft and a head's two wings, pointing along the heading | half the length, the wings' reach |
| `Ticks` | three dashes from the point along the heading | the run they take, unused |
| `Bar` | a line across the heading through the point; with a band, its two ends as lines half as long either side along the heading | the band's half along the heading (0: none), the line's half across |
| `Line` | one segment through the point along the heading, an edge of an outline | half its length, unused |
| `Segment` | one segment from the point to `Pose::to`, at any slope and not lifted: an axis of a gizmo | unused |

`world::edges(shape, &pose)` gives the line segments in world space,
flat and 0.02 above the pose's point (a `Segment` is the pose's two points as given). The game and the preview draw the
same segments: the game through the window's world-line pass, from each
view's camera, under the HUD and with no depth test
([superbounce.md](superbounce.md#the-superbounce-game-type), the pass
the mode's own marks use); the preview projected onto its canvas ("The
preview and the checks").

A mark is drawn in the user's HUD colour. `Dim` halves its alpha and
`Inactive` greys it, as for a widget, and the pose's strength scales the
alpha again. `Team(n)` draws it in the colour the game gives team `n`
("Colours", "A team's colour"): read from the loaded map's `mulg` each
present, and only while a shown mark has the style
(`hud_kit_world::colour`, `Teams`). Where the map has no colour for the
team the mark is in the user's HUD colour. The preview has no map to read
it from and draws the stand-ins `preview::PREVIEW_TEAMS` (**S**: not the
game's, apart enough to tell on the sheet). The `world` check refuses
`Team` on any other kind, and a team of `world::TEAMS` (8) or more. `Axis(n)` (`0..world::AXES`) draws in the colour of axis X, Y or Z
(`world::AXIS_COLOURS`, ours), for a gizmo's three segments; the `world`
check refuses it on any other kind and an axis of 3 or more. One
more style is a world mark's alone: `Go`, a
green (`world::GO_GREEN`, [0.35, 0.95, 0.45]) for a mark that says "this
will work": the approach guide's "this fall will bounce"
([superbounce.md](superbounce.md#the-approach-guide)). The `world` check refuses `Go` on any other kind. The
value's text is the mark's label, centred over its point in the game, and in the preview below the lines (the guide's drift arrow is on the other side), in the HUD font
(`model::WORLD_FONT`); `longest` is the widest label any state gives it.

A pose is the runtime's, each tick, as a navpoint's point is:
`Handle::world_mark(id, Option<Pose>)`. For the preview and the checks a
HUD gives sample poses per state, `Describe::world(state)`: world units
in a scene of the state's own. A mark shows only in the screen modes its
place has, like a channel: `Element::channel` gives it `Place::GAME`,
fullscreen only.

`crates/extensions/game/src/hud_kit_world.rs` is the game's side: a
world-line source (`hud_kit_world::register`, windowed hosts) that each
present takes the marks to draw now (`hud_kit::world_marks_now(mode)`:
the world mark elements of the registered HUDs whose value is shown,
that have a pose and whose place has the screen mode) and gives the
pass their lines (`hud_kit_world::lines`: 2.5 pixels wide at 1080p with
a glow, the label 0.25 under the point). One label a seam: of the marks within 0.75 of each other only the first with a label keeps it. A label that projects into the HUD's top-left or bottom-left block is left out (`hud_kit_world::HUD_BLOCKS`, `label_under_hud`, asked by the pass that projects the anchors). With no pose set it costs one
atomic load (`hud_kit::any_world_marks`). Nothing is written to guest
memory.

### The runtime (`crates/extensions/game/src/hud_kit.rs`)

A feature registers its `Describe` once (`hud_kit::register`, which
returns a `Handle`), then publishes a `Values` whenever its state changes
(`Handle::publish`), and, for a split-screen player whose widgets differ,
that local user's own (`Handle::publish_for(user, Some(values))`, `None`
to draw the shared ones again), the world point of each of its navpoints
(`Handle::navpoint` for its first, `Handle::navpoint_at(id, point)` for
any) and the pose of each of its world marks
(`Handle::world_mark(id, pose)`, "World marks"). The feature `hud-kit`
does the rest:

1. **Install**, before `GAME_ACTIVATE` and after `hud_swap`: the loaded
   map is opened through `native_map::open` (builds with the `gpu`
   feature; others install nothing and say so), and for every `nhdt` a
   `bipd`'s HUD interfaces name (`hud_kit_core::map_file::player_huds`)
   the templates are resolved through `MapFile` and every registered HUD
   is compiled. A HUD that is multiplayer only is not joined on another
   map, and one with a template missing is not joined; both are logged
   with the reason. `install_records` copies each root's two widget
   blocks into the kit's memory (`Builder::guest_alloc`, `REGION_LEN`)
   with the kit's records after them and points the root's two block
   fields at the copies. The retail records are byte for byte the same
   and draw first. What each root held is kept at the head of that
   memory (`hud_kit::Header`), so the next install puts it back first
   and leaves one copy, after a machine snapshot too; a root the game
   reloaded is left alone. With no HUD registered nothing is touched.
   A live edit of a retail record (the agent's `edit.*`) reaches the
   copy the game draws: the hot-edit glue's tag-write notice calls
   `hud_kit::tag_written`, and `recopy` copies each retail block the
   edit overlaps into its copy again
   ([realtime-edits.md](../editor/realtime-edits.md), "Realtime
   coverage"; test `an_edited_retail_record_is_copied_again`).
2. **Gate** (the Rust walk's seam, `render_bridge::native_hud::seam`):
   a widget in the kit's memory is drawn while its value is shown, in a
   style the runtime draws, in a screen mode its place has (one local
   view fullscreen, two half, more quarter: `player_globals +8`), and
   none of its element's `hide_when_d` bits is set in the walk's word D
   (the game's state word D, its low 16 bits). The
   retail gate is skipped for it; a retail widget costs two loads and
   one more, of the monitor users' bits. For a local user in monitor
   mode (`Handle::retail(user, false)`) every widget that is not the
   kit's is hidden, the player `nhdt`'s retail records and the weapon's,
   but the motion tracker's: the bitmap widgets named in
   `model::MONITOR_KEEPS` (`motion_sensor`, V: docs/halo1/hud.md, "The
   motion sensor's centre"), whose records' addresses the install keeps
   in `Table::kept` (`hud_kit::kept_records`: the copy when the root
   points at it, else the tag's own). The tracker's blips are not a
   widget (`0x1a2823`) and draw as before. Another user's widgets are
   left to the game's gate (`hud_kit::retail_gate`; tests
   `a_monitor_user_draws_no_retail_widget_but_the_motion_tracker`,
   `the_kept_records_follow_the_root`). Each install shows every user's
   retail widgets again.
   The gate, the inputs, the string and the scale read the values of
   the local user whose HUD is being drawn: `[0x004b9ed8]`, which
   `UI_HUD_DRAW` reads and hands to its per-user draws (E `0x001917ed`,
   `0x0019186a`; `hud_kit::draw_user`): that user's own values if it has
   them, else the shared ones (`hud_kit::values_for`). The channels read
   the shared values alone.
3. **Inputs** (the seam): `hud_input1` is
   `records::Widget::input(fill)`, the other three 0.

   The gate and the inputs read a value with a motion as it draws now:
   `Handle::publish` stamps a changed one with a host `Instant`
   (`hud_kit::stamp_values`, through `motion::restamp`), and
   `hud_kit::animated` puts it on its curve at the milliseconds since.
4. **Scale** (the seam, before the widget's effects `0x0022bcaf`): a kit
   bitmap widget's basis factors, across and down (the two
   `h1_hud_text::weapon`'s split-screen scale sets), are multiplied by
   `motion::scale_at` (`hud_kit::scale_factors`). A factor that is not a
   number is left at 1. The first time the seam is asked for a kit
   widget it sets `hud_kit::EFFECTS_SEEN`: the effects run for a widget
   with an empty effects block, and the scale applies. Until then the curves run
   unscaled and the ending is the plain one (sweep, blink, gone at
   430 ms); a crop-based close was not chosen, as it would need the
   sprite hook to narrow from both sides.
5. **String** (the seam): the value's text, UTF-16, in the element's
   engine font for the mode; the walk writes the four inputs to the HUD
   material's (`0x004e6980`), as the text draw does. A gauge's
   text is the one character of its picture for the value's fill as it
   draws now (`records::Widget::text`), so an eased fill sweeps through
   the steps at any frame rate.
6. **Crop** (the seam's sprite): for a cropped widget the seam
   picks the sprite as the game does (`hud_kit::sprite_pick`: the mode's
   sequence and its first sprite; a sequence with no sprites, its
   `first_image` whole; no such sequence, image 0 whole; then the
   image's width and height) and narrows its bounds to the crop
   (`hud_kit::narrow`). A sprite-less sequence is cropped too: the
   battery strip and the grenade panel are drawn as whole images, and
   left to the game they drew 92 and 103 pixels wide, the meter across
   the full strip and the pill over the retail pills' art. The crop is applied to the sprite's bounds,
   not to the quad's corners at `HUD_SHADER_QUAD`, whose order is not
   read (decoding 7).
7. **Channels**, at the tick's end (`GAME_TICK`, after) and in the prompt
   hook:
   - `Prompt`: the first shown prompt text is written to the first local
     player's prompt fields (`+0x220`, `+0x440`, `+0x444`, the fade
     bytes `+0x44e`, `+0x44f`) in a hook on `HUD_INTERACTION_VISIBLE`, at
     the interaction code's call, only while the game shows no prompt of
     its own (`hud_kit::show_prompt`, as `h1_hud_text/prompts.rs` does).
   - `Training`: the first shown training text is the kit's string id
     (family `hud kit` in `ui_kit::ids`), set with the show byte in the
     scripted training slot (`[0x0051e9c0] +0x6c0`, `+0x6c4`) unless the
     game shows a scripted text of its own; the string lookup
     (`0x001a0180`, `community_variants`' override) answers the id with
     the text, each `\n` written `\r\n` as in the game's own strings (a
     lone `\n` draws no break and no space; `hud_kit::write_training`,
     `hud_kit::text`, `hud_kit::game_line_breaks`).
   - `Navpoint`: at install the scenario's point block, the tag block
     at `scnr +0x1e0` (count, then the address at `+0x1e4`, which is what
     the draw reads, E `0x0024db5e`; `0x38`-byte points), is copied into
     the kit's second allocation with four points after it
     (`KIT_POINTS`, as many as a player's navpoint list has slots), and
     the scenario points at the copy (`hud_kit::install_point`, kept and
     put back as the widget blocks are, and only while the loaded
     scenario is the one it changed). The kit's navpoints are the
     `Navpoint` elements of the registered HUDs, in registration order,
     the first four (`hud_kit::navpoints_now`); a HUD starts after the
     navpoints of the earlier HUDs it can show with, and starts again at
     the first slot after those it never shows with
     (`Describe::never_with`, `hud_kit::slot_bases`); an element keeps its
     place in that list whatever shows, so it keeps its point. Each tick
     each point is set to its element's navpoint and one slot of the
     first local player's navpoint list is kept on it: the slot already
     that point's, else the first the game's draw passes (`+0` or `+8`
     -1, or kind 15; the draw walks all four slots and no slot ends the
     list, E `0x0024dc21`..`0x0024dc3e`) (`hud_kit::write_navpoint_n`).
     The game's own slots are not touched, so a navpoint of the kit's
     with no slot free is not drawn that tick. Without a point, or once
     the loaded scenario no longer points at the copy, the slot is
     skipped (kind 15): the draw does not check a kind-0 slot's index
     against the block's count. The game draws its own arrow and marker
     there; no number is drawn with it.
   - `Banner`: for each local user (users 0 to the local view count
     less one, `hud_kit::local_users`), the first shown banner of the
     registered HUDs whose place has the screen mode, read from the
     values that user's HUD draws, its own (`Handle::publish_for`) or
     the shared ones (`hud_kit::banner_for`), compared with that user's
     last tick's (`hud_kit::banner_due`): a new text is written into the
     user's timed line with the fields as `0x0024cc73` leaves them
     and a duration of `BANNER_TICKS` (`hud_kit::write_banner`). With no
     game time (`[0x00510c54]`) or no HUD state yet nothing is written.
   - `Message`: `Handle::notify(user, text)` queues the line (a user
     under 4, a text `message_fault` passes; else refused and logged).
     At the top of the next tick a before-hook on `GAME_TICK` writes
     each queued text, UTF-16 and zero ended, into a 128-byte slot of
     the kit's third allocation (`NOTES_LEN`, 32 slots, so 32 lines a
     tick at most; the rest wait), and calls the insert `0x0024cbee`
     for each with EAX the user and the slot's address its stack
     argument ("Game events and medals"), then runs the tick itself and
     answers for it, as `skulls_custom` sends its messages. A flag passes
     the hook while that tick runs. It is in the first group, after the
     agent's calls, our skulls', the map select's and the dedicated
     lobby's hooks there (`hud_kit::stage_notes`, `notify_call`).

The styles the runtime draws are `Normal`, and `Flash` as `Normal`; a
widget in `Dim` or `Inactive` is not drawn until the colour hook
(slice 4). The draw hooks are `context_only` and read one shared table. The feature is presentation-only and
snapshot-stateless: what the roots and the scenario held is in guest
memory, and the draw table is rebuilt at each install (a snapshot loaded
mid-game draws no kit widget until the next game starts).

### Anchoring next to retail art

An element is placed in HUD pixels from an anchor, like the retail
widget beside it. The crate has the retail rectangles as named
constants, checked on the disc: `TRACKER`, `SHIELD_BAND`, `H1_HEALTH`,
`PROMPT_LINE`, `TRAINING_BOX`. `rects::above(SHIELD_BAND, gap, height)`
and the like give offsets from them, so "above the motion tracker" is written
once and is right in each screen mode.

Something drawn over the tracker that no retail widget can draw (a
polygon, a map) is not a kit element. The superbounce bounce view's
blips and map are host marks in the image's HUD space
(`gfx::world_lines::HudMark`), placed at `TRACKER` from the HUD frame's
bottom-left corner and moved by the host's fit of the bottom-left group
(`render_aspect::hud_bottom_left_fit`)
([superbounce.md](superbounce.md#the-bounce-view)).

### Direction arrows

A direction on the HUD is a character. Every HUD font of a served map
carries eight arrows at U+E2B0 to U+E2B7, the first pointing up and each
the last turned 45 degrees clockwise (`crates/formats/font/src/arrows.rs`:
`glyph_for(angle)`, the angle in radians clockwise from up). They are
drawn in code, white, as tall as the font's capitals, and added where the
gear and the medal glyphs are (`bot_mark::GearRewrite` at load;
`hud-edit`'s `Fonts::as_served` for the preview and the checks), so a
text widget draws one in the text's colour as it draws a letter. The
disc's fonts have no arrows of their own.

A `Pointer` element is not used for a direction: the runtime does not
turn a widget.

### Gauges

A meter is a retail bitmap filled by its shader, so it is a rectangle:
a strip, or a crop of one. A shape no retail bitmap has, as an arc round
the motion tracker, is a `Gauge`: a text widget in font byte 0
(`GAUGE_FONT`, engine font 6) whose string is one character, the picture
of the shape at the value's fill. The pictures are drawn in code and
served in every font file by the font rewrite, as the direction arrows
are, so the game's text draw draws them in the HUD's text colour, in the
game's own batches, hidden with the HUD.

| | A bitmap of ours in the map | A host mark (`gfx::world_lines::HudMark`) | Glyphs of ours in the fonts |
|---|---|---|---|
| Gets into the game by | a new `bitm` in every level's map: the map decomposed and rebuilt at load (`tags::rgba_bitmap`, as mainmenu's level pictures are) | the window's own pass | the font rewrite the arrows and the medals use (`bot_mark::GearRewrite`, `font::patch`) |
| Fill | the meter shader, by a blue ramp along the shape | drawn per frame | a picture per step |
| Colour, hiding with the HUD, films, the headless host | the game's | not the game's ("Where the kit draws") | the game's |

A `Glyphs` names the run: `first`, the empty picture's UTF-16 unit, then
one unit a step to the full one at `first + steps`, each `size` pixels.
`Glyphs::step` picks the nearest step for a fill, and
`records::Widget::text` is the string for a value. The fill keeps its
`Motion`: the runtime asks the curve first and picks the step after.
A picture has no offset across and sits as far above the baseline as the
font's ascent, so its top-left corner is the widget's point in any font
(`font::ring::replacements`), and the widget is justified from its
start.

The pictures the kit has are the charge ring's, `font::ring`: nine arcs,
seven steps each, at U+E2C0 to U+E2FE, after the arrows ("The charge
segments"), and the Firefight HUD's skulls, `font::skulls`: seven slots
of two steps (off, faint; on, lit), 14 pixels square, at U+E600 to
U+E60D (slot `s`'s step `k` at `0xE600 + 2s + k`). Each is a skull's glyph,
the strokes of `font::strokes` (`game::skull_glyphs`) rasterised, so the
preview, the checks (`hud-edit`'s served fonts) and the game's font rewrite
(`bot_mark::GearRewrite`) all carry the same pictures. `Glyphs::picture`
asks the ring first and the skulls after.

### The Firefight HUD

`views::firefight` (`content/modes/huds/firefight.hud.yaml`,
[firefight.md](firefight.md), "The HUD") is one layout of twenty-one
elements at the frame's bottom right, above the retail score plate, in the
HUD font, offsets from anchor 3 (bottom right) in 14 pixel lines. What it
shows follows Halo 3: ODST's Firefight HUD; the pieces are Halo 2's and
pictures of ours.

| Element | Kind | Place (full) | Screens |
|---|---|---|---|
| `ff.score`, `ff.lives`, `ff.progress` | text, right-justified | (0, -58), (0, -72), (0, -86) | score not in a quarter; lives and progress one line lower there |
| `ff.enemies` | meter, `Art::StripMeter` (92 by 8), registered at its right edge | (0, -94) | not in a quarter |
| `ff.set` | text, right-justified | (-72, -110) | one line lower in a quarter |
| `ff.round.0` to `ff.round.2` | gauge of `font::skulls::Pip::Round` (8 by 10), two steps | (-(66 - 9 i), -108) | one line lower in a quarter |
| `ff.wave.0` to `ff.wave.4` | gauge of `font::skulls::Pip::Wave` (6 by 10), two steps | (-(34 - 7 i), -108) | one line lower in a quarter |
| `ff.skull.0` to `ff.skull.6` | gauge of `font::skulls`, two steps | (-(18 i + 15), -125); half (-(14 i + 14), -124) | not in a quarter |
| `ff.banner` | `Banner`, the game's timed centred line | the game's | not in a quarter |

A skull on is `Value::fill(1.0)` with `Motion::Arrive`, so it blinks three
times when it first shows and stays lit; off is hidden, so a slot never
moves. A pip is `Value::fill(1.0)` with `Motion::Arrive` when lit,
`Value::fill(0.0)` (the faint picture) for a round or wave to come, and
hidden where the definition has none. The last life's line has
`Motion::Arrive` too, no lives left is `Style::Flash(TimeWarning)`, and the
meter eases. The banner is the game's line, so the game fades it. `State` is
what the game gives one local user (`game::firefight::hud::state_of`, from
the layout for that user's screen: full for one local user, half for two,
quarter for three or four); `values` turns it into the kit's `Values`.

### Cost

S, to be measured with `profile` on the lab Mac: one address-range test
per retail widget on hook points other features already use,
and for a HUD of ten elements about ten more HUD quads and five text
draws a frame, in the game's own batches. No host texture, no upload.
