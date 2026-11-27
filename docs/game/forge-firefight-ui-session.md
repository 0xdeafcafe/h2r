# Forge and Firefight UI: the Forge in-session UI

Status: part of [forge-firefight-ui.md](forge-firefight-ui.md); see there for status, labels and the index. This page: the monitor HUD, screens, markers, AI tool and Waves screens.

## The Forge in-session UI

The behaviour (what each button does, the tools, the settings, the
properties by kind) is [forge-editing.md](forge-editing.md). This
section is how each piece is drawn.

### The monitor HUD

A HUD kit `Hud` named `forge`, registered by `forge_glue` and published
when the tool's state changes. One layout:

| Element | Kind | Place | Shows |
|---|---|---|---|
| `forge.reticle` (not built) | Icon, `Art::Widget` of a retail weapon reticle | centre | always in the monitor |
| `forge.prompt` | Prompt | the prompt line | the one action A does: "Press (A) to pick up Beacon", "Press (A) to place Beacon", "Press (X) for the object menu"; while manipulating, the value that is changing ("Heading 45") |
| `forge.notice` | Message | the notification list | a refusal ("The object budget is full."), Saved, Loaded, a setting changed by a button ("Fine movement on"); it fades as a pickup line does |
| `forge.teach` | Training | the training box | the first time a state is reached: hold Y to delete, hold RT to turn, Start for the menu |
| `forge.budget.pill`, `forge.budget.pill.end` | Icon, `Art::GrenadePill` | the grenade row's first place | always |
| `forge.budget.count` | Text, `Font::Lcd`, longest "9999" | on the pill | objects left to place |
| `forge.budget.meter` | Meter, `Art::StripMeter` | under the pill (`rects::below`) | used over limit; `Flash(Pulse::TimeWarning)` from 90% |
| `forge.tool` | Text, `Font::Hud`, longest "VOLUMES" | top right, where a weapon's name line sits | the tool; capitals |
| `forge.rebuild.pill`, `.pill.end`, `.count` | Icon and LCD Text | the grenade row's second place | pending Rebuild edits; hidden at 0 |
| `forge.enemies.pill`, `.pill.end`, `.count` | Icon and LCD Text | the grenade row's third place (`views::forge::ENEMIES_STEP`, 60) | enemies left while a wave test runs; hidden otherwise |
| `forge.nav.target` | Navpoint | over the held, aimed or placed thing; in the Links tool, over the aimed end | while there is one; the number is the distance in metres |
| `forge.nav.from` | Navpoint | over the link's first end | while a link is started |
| `forge.nav.monitor.1`, `.2` | Navpoint | over the nearest two other monitors | while another seat's monitor is up |
| `forge.gizmo.x`, `.y`, `.z` | world mark, `Shape::Segment` in `Style::Axis(0..3)` | from the held or aimed object's origin to its end along each axis, in world space | in the Objects tool while an object is held, placed or aimed at |

- **The retail widgets are off in the monitor, but the motion tracker.**
  A monitor has no shield, weapon or grenades. The kit's gate hook
  answers for the retail widgets of a user whose HUD is in monitor mode
  (`Handle::retail(user, false)`, slice K1), except the motion
  tracker, which stays drawn (decision 3). The grenade row's origins
  (`Origin::NextGrenadePill`) then resolve to the row's first place.
- **No reticle yet.** The kit has no reticle element until a retail
  weapon's reticle widget is chosen from the disc census (**S**): a text
  at the screen's centre is inside the retail reticle's reserved
  rectangle and fails the `retail` check, so the monitor draws none.
- **Copy** is the game's form: sentence case, the button as the font's
  glyph, one line. The count is digits only, as the LCD font has no
  slash. A prompt's button is read from the bindings table
  (`forge_controls::bindings::BINDINGS`), never written in the string.
- **No legend bar.** The full control list is the **Controls** page.
- **The feed** (`forge_hud.rs`): the editor publishes the kit's `View`
  each tick (`Editor::monitor_hud`), the tool, what the hand has, the
  budget, the rebuilds pending, and the heading while a held thing is
  turned with RT. The feed publishes the values when they change and
  sends the session's notice once, as a notification: a sentence the
  list cannot keep whole sends its first sentence, and one with no short
  first sentence none (`forge_hud::notice_line`). A hint is shown the
  first time its state is reached, for seven seconds, and not again
  (`forge_hud::Teaching`); the Links tool has none. The prompt names a
  name over 24 characters by its verb alone.
- The agent API's `forge.status` carries the same values; `hud.kit`
  reports what the gate drew.

### The screens

Each is a list class of ours on a retail screen tag
(`forge_screens::sheet::Look`): the large dialog, `error_dialog_large`
(screen 240), laid out as the value picker for the object menu and
Tools and with a body for Link and the questions; the in-game Controller
Settings (screen 200) for the list pages. Both are in `shared.map`
(`each_base_resolves_in_shared`). A screen asked for opens at the next
UI update for player one, once nothing is pending on the dialogs'
window (`forge_screens::step`): a dialog class over the screen beneath,
a list page through the game's screen opener. While a screen is up, ours
or the game's (GAME MENU, the picker, the keyboard), the monitor's
camera and tool take no input (`forge_screens::ui_up`, published to
`forge_controls::State`, `screen`).

- **Who builds what.** The session builds the sheets that need its
  document: the object menu, Properties, Link and the questions
  (`forge_screens::open`). The others are built from what the session
  publishes each tick (`forge_screens::Facts`): Tools, Tool settings,
  the Forge menu and Controls (`forge_screens::request`). One screen of
  ours opens at a time; a second is refused by name.
- **Guest memory.** Every screen's texts and the Forge menu's strip share one
  64 KiB guest allocation (`forge_screens::TEXT_POOL_LEN`,
  `ui_kit::screen::TextPool`). A text slot takes 0x20 to 0x100 bytes of it,
  as its text needs, so a screen costs what it shows, not its whole
  family at 0x100 bytes a slot; a full pool truncates the new text.
- **Choices.** A press sends a `sheet::Choice` to its screen's outbox,
  which the session drains each tick. GAME MENU's Leave Game asks first
  while edits are not saved (`forge_screens::asks_before_leaving`, the
  one press of GAME MENU that Forge takes); Yes stays in
  `forge_screens`: it leaves as the title leaves a game
  (`host_lobby::LEAVE_SESSIONS`, then the main menu).

| Screen | Opened by | Built as | Slice |
|---|---|---|---|
| Object menu | X with nothing aimed at | `kit.picker.game` twice: groups, then the group's objects | U4 |
| Object actions | X on the thing aimed at or held | a [floating menu](ui-screens.md#the-floating-menu) at the object (`forge.object-actions`, `forge_screens::object_menu`): Edit, Duplicate, Delete, Properties | U4 |
| Properties | the Properties row of Object actions; X on a thing being placed | a list page on the in-game Controller Settings host; rows from a table by kind | U4 |
| Tools | D-pad Right | `kit.picker.game` over the tools that are built | U4 |
| Tool settings | D-pad Down; a row of the Forge menu | a list page on the same host, a row per setting | U4, rows in P1 |
| Overlays | Click the right stick | a list page on the same host as Tool settings, a row per overlay | |
| Link | X in the Links tool | a large dialog with a body and two rows | U4 |
| Questions | delete with links, overwrite, leave unsaved | the same dialog with Yes and No | U4 |
| Forge menu | Back, while the player flies the monitor | `forge.menu`, a list page on the Controller Settings host with its pages as tabs | U4 |
| Controls | a row of the Forge menu | a list page made from the bindings table | U4 |
| Coordinates | a row of Properties | a list page on the same host; A opens the keyboard | P1 |
| Placed objects | the row of the Forge menu's Objects tab | `kit.picker.game` over the objects placed, each flying the monitor to it | built |
| Map limits | the Forge menu's Limits tab | the rows of `forge.map-limits`, read-only | built (P4) |
| Waves, Round, Wave, Squad | **Waves...** on the Forge menu's Waves tab | list pages on the Controller Settings host, value rows opening `kit.picker.game` ("The Waves screens") | built (FA5) |
| Rounds, Squads, Problems | a row of Waves or Wave | the large dialog, a row an item, each opening its page | built (FA5) |
| Drop | X on a drop zone in the AI tool | a list page on the same host, on the Wave screen (`waves::Page::Drop`), with the squads that land there on the Squads list (`Page::DropSquads`) | built (FA7) |
| Enemy types | a row of the Forge menu's Waves tab | a list page on the Controller Settings host, a row a class, each On or Off | built (FA5, FA6) |

- **Object menu.** Each picker has its six rows, scroll arrows and
  description block. The groups come from a table
  ([forge-editing.md](forge-editing.md), "The object menu"); a group
  with nothing on this map is absent. The description is the object's
  line: what it is, how many are placed, how many are left. A on an
  object closes both and holds a new one. The picker opens on the group
  and object last chosen.
- **Properties.** The object's name is the header. A value row has its
  value in the skin's value column and A opens the picker; an action row
  (**Coordinates...**, **Reset orientation**, **Duplicate**) acts. No
  row draws arrows. Coordinates and Heading are present for every object,
  Team for starting points and goals, Role and Order for goals, and
  Respawn time for item spawners (`properties(group, kind)`), so a later
  slice adds rows to the table and no screen.
- **Tool settings.** Rows Movement snap, Rotation snap and Axes, each a
  picker (`Tool::set_movement_step`, `Tool::set_rotation_step`,
  `Tool::set_axes`, `sheet::SETTINGS`). Fine movement is the stick click's
  toggle.
- **Link.** The body lists the two ends and what the link writes, with
  rows **Remove link** and **Done**: retail's two-button question.
- **Questions.** **Yes** and **No**; No has the focus.
- **Forge menu.** `forge.menu`, titled FORGE: Back's screen while the
  player flies the monitor, B closing it. Its pages are tabs across the
  top, stepped with LB and RB (Left and Right do the same, for a
  keyboard), round the ends:

  | Tab | Rows |
  |---|---|
  | Options | **Rebuild map** (the pending count as its value), **Tool settings...**, **Controls**, **Generate nav** |
  | Map variants | **Save**, **Save as**, **Load**, **Map variants...** |
  | Waves | **Test waves** (**Stop waves** while a test runs), **Waves...** while the session publishes a wave set, **Enemy types...** while the recipe names classes, **Build AI ground** |
  | Objects | **Placed objects**, the picker that flies the monitor to a group or an object |
  | Limits | the Map limits rows, each a fact |

  ```text
   FORGE
   (LB) OPTIONS  Map variants  Waves  Objects  Limits (RB)
  +----------------------------------------------------+
  | Rebuild map  2                                     |
  | Tool settings...                                   |
  | Controls                                           |
  | Generate nav                                       |
  +----------------------------------------------------+
   (A) SELECT  (B) BACK
  ```

  The strip is the class's panel (`menu::panel`, `ListClass::panel`):
  each tab has a bright and a muted text in the same place, the open
  tab's name bright and in capitals, as Theater's library tabs are
  drawn, with the bumpers' glyphs at its ends. A step sets the tab's
  sheet in place (`ForgeScreen::turn_tab`, `menu::sheet`) and the list
  grows or shrinks to its rows at the next draw (`Rows::growing_capacity`,
  `Rows::live_text`), so the screen never closes between tabs; the
  class's event hook takes the bumpers before the list sees them
  (`menu::turn_of`). The menu opens on the tab it was left on.

  It is the monitor's alone. Back in the player's own body is the
  game's scoreboard, a request for the menu there is refused by name,
  and a swap back to the body drops a menu asked for and closes one
  that is up (`forge_screens::publish`, `Facts::monitor`). GAME MENU
  (Start) keeps the game's own rows in a Forge game.

  Load opens the picker over this map's saved variants and closes the
  menu with the pick, then asks before it replaces unsaved edits; Save
  opens the keyboard for a session with no name, as Save as does
  (`Act::SaveAs`), then the replace question when the name is taken
  ([forge-map-variants-ui.md](forge-map-variants-ui.md), "Saving,
  loading and renaming"). Rebuild map builds the map from the document
  and reloads onto it ([forge-design.md](forge-design.md), "Rebuild
  map"). **Build AI ground**, **Test waves** and **Generate nav** act
  and close the menu ([forge-map-variants.md](forge-map-variants.md),
  "AI ground"; [forge-nav.md](forge-nav.md)).

  A row that opens a page opens it above the menu, so B comes back. In
  the design **Build AI ground** has the ground's state as its value
  (Not built, Current, Out of date). Monitor look... joins when it is
  built; Players (F5), Map settings and Publish (F14) join in their
  slices. The object menu's last row is **AI and objectives**, the
  ground labels (`sheet::objects_with_ai`).
- **Controls.** A row a control of the bindings table, its pad button
  as the value and its keys in the description; up and down, the tools
  and their settings, and the two menus share a row
  (`bindings::PAIRS`), so the page fits the in-game list's sixteen
  lines.
- **Coordinates.** Six rows, X to Roll. A opens `Effect::Keyboard` with
  the value; left and right nudge.

### Markers in the world

- The held, placed or aimed object carries `forge.nav.target`, the game's
  arrow and marker, with the distance in whole metres as its number.
  In the Links tool the target is the aimed end, and a link's first end
  carries `forge.nav.from` while it is started.
- Each other seat's monitor is a navpoint (`forge.nav.monitor.1` and
  `.2`, the nearest two, nearest first). The game draws a navpoint in one
  colour, so a seat is told apart by where it is, not by its colour. The
  game's list has four slots, which the four navpoints fill. A HUD that
  never shows with another (`Describe::never_with`) starts its navpoints
  at the first slot, so the monitor's four do not wait behind a biped's
  (`hud_kit::slot_bases`).
- **The axis gizmo** (decision 2) is three HUD kit world marks of
  `Shape::Segment`: a line between two world points (`Pose::at` and
  `Pose::to`), drawn by the window's world-line pass as every world mark
  is, in the three axis colours (`Style::Axis`, `world::AXIS_COLOURS`).
  `forge_glue/gizmo.rs` (`Axes::from_frame`) gives the ends each tick, in
  the world's or the object's basis as the Axes setting says, an arm a
  seventh of the object's distance from the eye, between 0.05 and 0.5
  world units. It hides with the HUD and while a Forge screen is up. No
  bounding sphere's outline is drawn: retail has none.
- `forge_hud::Points` holds the world points each tick; the editor
  leaves out the markers, the rings (other monitors) and the gizmo that
  `forge_hud::Show` turns off; the session turns them on all (the
  Overlays setting of [forge-editing.md](forge-editing.md) sets them).

### The AI tool

Built (FA7, `forge::tool::Tool::ai`, `forge_glue/editing.rs`). The AI tool
is the monitor's tool for a Firefight map: it aims at ground markers
only, draws the built ground, and opens the waves. Everything it writes
is a marker or the wave set ([forge-firefight.md](forge-firefight.md)).

**The palette group.** The object menu's last group is **AI and
objectives** in every tool (`sheet::objects_with_ai`); in the AI tool X
with nothing aimed at opens the object menu focused on it. The Waves
screens open from the Forge menu's Waves tab. Its rows are the
labels the session's game type reads, each with how many are placed as
its value and one line of help:

| Row | Places | Help |
|---|---|---|
| Drop zone | an `ff_drop` marker facing the way the monitor looks | "Where a wave arrives. Twelve at most." |
| Base | an `ff_base` marker for the team the placed ones leave short, red then blue | "What the enemy pushes at, and where a team spawns." |
| Cover | an `ff_cover` marker with a facing | "A place the enemy fires from." |

The Siege labels follow them as they do today. A marker is held, placed,
picked up, turned and deleted as an object is: A places and picks up, RT
turns, hold Y deletes ([forge-editing.md](forge-editing.md),
"Controls"). A thirteenth drop zone is refused with the notice "A map
has 12 drop zones at most."

**What the tool draws.** Each marker as its game type marker (a drop
green, a cover yellow, a base its team's colour), a marker the last
build could not honour in orange ([forge-map-variants.md](forge-map-variants.md),
"AI ground"), and, in this tool only, the built ground's sectors in the
Nav tool's sector style (`forge_glue/nav_world.rs`, `sector_lines`). The
sectors are `GroundReport::sectors`, each a polygon of the built map
(`forge_build::ground::sectors_of`); they are drawn only while the
ground is current, with the Markers overlay on: a stale ground draws
none, since they are no longer what a build gives. The build's problems
are orange markers in every tool.

**The readout** (the bottom-left readout,
[forge-map-variants-ui.md](forge-map-variants-ui.md)) names what is
aimed at: "Drop zone 3" and "2 squads land here", "Red base", "Cover".
A drop zone's number is its zone in the wave set, counted from 0 as the
Squad page's Drop picker counts it, so the two agree. The prompt line is
the Objects tool's with the marker's name: "Press (A) to pick up Drop
zone 3". `forge.status` gives `aimed_drop` (`zone`, `squads`) and `tool:
"ai"`.

**Placing a squad.** A squad is a row of a wave, and it is placed by
giving it a drop zone. X on a drop zone opens the DROP page:

| Row | Value | A |
|---|---|---|
| Squads that land here | the count | the Squads list of them ("Round 1 Wave 2: Elites"), each opening its SQUAD page |
| Add squad here... | | a picker of the waves ("Round 1 Wave 1" to the last); the choice adds a copy of the wave's last squad with Drop set to this zone (`Edit::AddSquadAt`), and the squad is reached from the list above |
| Coordinates... | | the Coordinates page, as for any thing |

A squad whose Drop is **Any drop** belongs to no zone and is reached
from its wave. Moving a drop zone keeps its squads; deleting it sends
them to Any drop, and the notice says so ("2 squads now use any drop.").

**The ground's state** is not a HUD element. It is the value of the
**Build AI ground** row, and a notice the first time an edit makes a
built ground stale: "AI ground is out of date." **Test waves** builds
it.

### The Waves screens

Built (FA5), in `crates/extensions/game/src/forge_screens/waves.rs`. Four
list pages on the in-game Controller Settings host, each within its
sixteen lines, opened one above the other so B goes back. A row with a
set of values opens `kit.picker.game`; the name opens the keyboard. A
list that can outgrow the page (the rounds, a wave's squads, the
problems) is a page of its own on the large dialog, which scrolls: a row
an item, each opening its page, and the list's **Add** row last.

- **Built from facts.** The session publishes the set it plays and what
  the pages read with it each tick (`waves::Facts`: the set, whether it
  was edited since the load, the ground's classes, the drops placed, the
  problems, the Start from sets). A page is built from them when its row
  is pressed (`waves::open`), and the hub keeps the page each screen
  shows.
- **Each press is one edit.** A value sends `Choice::Waves` with a
  `waves::Edit`; the editor applies it to the set it holds
  (`waves::apply`, which checks the result as `forge_doc::waves::check`
  does) and makes one `Session::set_waves` with the whole definition
  (`Editor::edit_waves`). A change the validator refuses changes nothing
  and its sentence is the notice. A change of the published set, or a
  refusal (`waves::stale`), builds each page shown again from the set
  (`waves::refresh`), so a refused value goes back; a page whose round,
  wave or squad is gone closes.
- **Zones** are the definition's, numbered from 0 as the kit numbers its
  `ff_drop_NN` zones, `forge.waves` lists them and the problems' sentences
  say them.

**WAVES** (`forge.waves`), from **Waves...** on the Forge menu. The
header is the set's name.

| Row | Value | A |
|---|---|---|
| Name | the set's name | the keyboard |
| Start from... | | a picker: Holdout, Attrition, Last Stand, Crossfire, then the sets the `modes` package keeps for the session's base map (`firefight::builtin::map_sets`); over a set edited since the load it asks first (`waves::start_from`) |
| Teams | One team, Two teams | picker; two teams score a player kill as the built-in sets do |
| Starting lives | 0 to 30 | picker |
| Bonus lives a round | 0 to 10 | picker |
| Shared lives | On, Off | picker |
| Enemy limit | the live enemies allowed with four players | a picker of steps of four up to the host table's budget for four ([ai-networking.md](ai-networking.md#cpu-on-the-host)); every tier scales with it, each within the table's budget for its players |
| Rounds... | the count | the Rounds list (`forge.rounds`): a row a round, its line its waves and skull ("5 waves, Catch."), each opening ROUND, then **Add round**, a copy of the last without its skull |
| Problems | the count; absent at 0 | the Problems list (`forge.wave-problems`): a row a problem ("Round 1, wave 3, squad 2", its line "No Grunts on this map."), each opening its squad |
| Test waves | | starts the test and closes the page; **Stop waves** while one runs |
| Reset waves | | the set as the map was loaded, after a question; absent while the set is as loaded |

**ROUND n** (`forge.round`).

| Row | Value | A |
|---|---|---|
| Skull | None, or the skull the round switches on | a picker of the nine Firefight skulls no other round uses |
| Wave 1 to Wave 4, Boss wave | the wave's enemies with one and with four players ("6 to 14 enemies"); its squads as its help | WAVE |
| Copy from... | | a picker of the other rounds; this round takes their waves, and keeps its skull |
| Remove round | | a question; absent from the only round |

**WAVE n** (`forge.wave`).

| Row | Value | A |
|---|---|---|
| Starts after | 0 to 300 seconds | picker; the gap before the wave |
| Squads... | the count | the Squads list (`forge.squads`): a row a squad, its line "Elites, drop zone 3. 1 for each player, 1 to 5.", each opening SQUAD, then **Add squad**, a copy of the last |
| Copy from... | | a picker of the other waves of the round; this wave takes their squads and gap |
| Clear wave | | a question; the wave keeps its first squad; absent with one squad |

The Squads row's help is the wave's enemies against the limit at one,
two and four players ("6, 10 and 14 of 16, 24 and 32 enemies with one,
two and four players.", `waves::wave_help`).

**SQUAD** (`forge.squad`).

| Row | Value | A |
|---|---|---|
| Enemy | the class's name | a picker of the classes the built ground has (`forge_glue::ground::wave_classes`: the last build's classes, and before a build the Elite and the classes the recipe asks for); a class named the first time takes its points from a built-in set |
| For each player | 1 to 8 | picker |
| At least | 1 to 32 | picker; At most rises with it |
| At most | 1 to 32 | picker; At least falls with it |
| Drop | Any drop, or Drop zone n | a picker: Any drop, then each placed drop zone by number; a zone the squad prefers that is not placed is its value still |
| Duplicate | | the same squad again in this wave, after it; absent at 16 squads |
| Remove squad | | absent from a wave's only squad |

A squad has no weapons row: the ground authors one weapon a class, and
the row arrives when the weapon tier is applied
([firefight.md](firefight.md), "Guest boundary"). Scoring and the skull
multipliers are the template's (**Start from...**) and have no page.

**The questions** are the Forge question dialog's (`Ask::Waves`): Reset
waves, Remove round and Clear wave are asked from their rows; Start
from's opens over WAVES (`forge_screens::ask_over`), as its choice comes
from the picker.

**Enemy types** (`forge.enemy-types`) is a list page off the Forge menu, a
row a class of the retail class table past the Elite (which is always
built), each On or Off in a picker with its tag cost from the last build
as its help ("Grunt. 212 KB.", "Hunter. Cut, no room."). A change marks
the ground stale (`Session::set_ground_classes`); the classes arrive at
the next build. The rows come from `forge_glue::ground::enemy_types`, and
The Forge menu's Waves tab has the Enemy types row whenever they do.
