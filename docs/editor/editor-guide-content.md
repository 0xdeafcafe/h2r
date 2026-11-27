# Editor guide: UI screens, HUD and tags

Status: Built. The walkthroughs follow the controls in Studio.

Sections 7 to 9 of the editor guide: UI Screens, the HUD tool, and tag fields, announcer and voices. The index is [editor-guide.md](editor-guide.md).

## 7. UI Screens

**UI Screens** shows the screens h2recomp adds to the game (the Game
Options page before a campaign level, the Halo 1 options, multiplayer
skulls, the co-op screens, the Xbox Live sign-in and account screens)
the way the game draws them, without starting the game. Use it to see a
screen in each of its states, try its buttons, check its layout, and
move, resize or retype its widgets.

- **Screens** (left): one filter at the top narrows both trees. **Our
  screens** groups our screens into folders by the start of their names
  (`forge.*`, `skulls*`, ...; the design system and the layouts first),
  each screen with its named states (a list's first, middle and last row
  focused; the sign-in prompt's pages, such as `code-wrong` or
  `checking`). Click a state to show it. **Retail screens** lists the
  game's own screens from your disc, in folders by their tag path. Those
  are drawn for comparison only. The arrows fold; a click on a folder or
  on **Our screens** / **Retail screens** shows all of it (see the grid
  below). The panel scrolls as a whole, so neither tree is squeezed on a
  short window.
- **Grid** (centre, after clicking a folder or a root): a preview of every
  screen in it and under it, by folder. Each tile has the screen's name
  and a red count when the layout checks found something (a tick when
  they found nothing; retail screens are not checked). **Size** scales
  the tiles; **All states** shows every state of our screens instead of
  the first. Pictures draw as they scroll into view. Click a tile to open
  that screen; the names above its picture take you back to the grid.
- **Preview** (centre): the screen at the game's 640 by 480. **Zoom**
  makes it bigger. **Rects** outlines every widget, **Title safe** the
  area a TV always shows (green), and **Reserved** the areas a screen
  must leave alone (amber). Widgets a check complains about are outlined
  in red. Hover a widget to see its name. With a disc open the pictures
  use the disc's own art and fonts; without one they are placeholders
  ("synthetic"), and the checks that need the art are skipped.
  - **Transport** (the row above the picture): pick **Open** or
    **Close** and press **Play** to watch the screen's transition.
    **Loop** repeats it, the slider scrubs it by tick, **Speed** runs it
    at 0.25 to 2 times, and **Onion** outlines the widgets a moment
    before and after. **Rest** shows the screen settled.
  - **Editing**: click a widget to select it. Drag it to move it, or drag
    one of its eight handles to resize it. It snaps to other widgets'
    edges and centres; hold Shift to place it freely. The arrow keys
    nudge it a unit at a time (Shift: 10). Double-click a text to retype
    it in place: Enter keeps it, Esc cancels. Esc on its own clears the
    selection. **Undo** and **Redo** step back through your edits, and a
    whole drag is one step. Each edit is checked at once, so a widget
    dragged off the title-safe area turns red.
- **Inspector** (right):
  - **Page**: what the screen shows, and a pad: **Up**, **Down**,
    **Left**, **Right**, **A**, **B**, **X**, **Y**, **Start** and
    **Back**. Each press steps the screen as the game would, and the
    line under the pad says what it did (`Down: Focus(1)`,
    `A: Submit("120000")`). Press as many as you like; picking the state
    again on the left starts over. The screens change nothing on your
    machine: their settings live in a scratch profile.
  - **Checks**: what the layout checks found (text that doesn't fit, text
    on top of other text or art, art off the screen, low contrast, a
    missing button legend). Hover one to outline its widgets in the
    picture. "every check passes" when there is nothing.
  - **Widget**: the hovered widget's details (its name, rectangle, text
    or art), as the exported JSON has them.
  - **Selected**: the selected widget's rectangle as numbers you can
    type, its text, its font, justification and colour, **Copy JSON**
    (the widget as the agent API gives it), **Revert widget**,
    **Revert screen** and **Save**. Our screens save into the checkout
    the editor was started from (`content/ui/screens/<name>.yaml`);
    outside a checkout Save is refused. When a file changes on disk the
    screen reloads it, unless you have unsaved edits to it, in which
    case it says so. A retail screen's edits are changes to your
    `mainmenu` map (open it first), saved with the map. A picture has
    **Generate image...** ([Generating an image](#generating-an-image)).
    Some widgets
    can't be edited here and say why: a row's value comes from its
    setting, rows sit where the list puts them, and a retail screen's
    texts and title belong to the disc's string table and shared header.
  - **Widgets**: every widget of the screen. Hover a row to light its
    widget in the picture; click it to select it.
  - **Export...**: pick a folder. The editor writes the picture
    (`<screen>-<state>.png`), its widgets (`<screen>-<state>.json`) and
    `sheet.png` with every screen in every state. It refuses a folder
    inside a Git checkout, so pictures of your disc's art never end up in
    a repository.

To open the editor straight on a screen: `h2-editor --ui-screen
signin-code/code-wrong --ui-press up,right`.

### Generating an image

**Generate image...** makes a new picture for an image of a bitmap: a
menu icon, a screen's art, a HUD widget's picture or any texture. It is
on a selected picture in **Selected** here, on a bitmap widget in the
HUD tool, and as **Generate...** in the Textures tool's PNG panel. Each
opens the **Generate image** window on that image of the map open in
Studio (for a menu, open your `mainmenu` map first).

1. **Image** is the image the picture goes into; the line under it gives
   its size and format. The picture is made to fit both. **New image**
   beside it (or **New image...** on a selected picture here, or **New
   image, generated...** in the Textures tool) first adds an image to
   the bitmap, the size and format of this one, and generates into
   that: for a bitmap that has fewer images than you need, such as
   `mainmenu`'s eleven game-type icons.
2. Write the **Prompt**: what the picture shows ("a radar dish").
3. Pick the **References**: the images whose style the new one should
   match. For a menu icon, tick the other icons of the same bitmap; they
   are listed as **Image N of this bitmap**. **Add PNG...** adds a file.
   The references are your choice: they go to the generator as they are,
   and the editor never picks any for you.
4. **Alpha** is where transparency comes from. **Auto** keeps the
   picture's own, except that a picture with no transparency going into
   a mask-only image takes it from brightness.
5. Press **Generate**. It takes a minute or more; the window counts the
   seconds and the editor stays usable.
6. **Apply (preview in place)** puts the picture into the image. The
   Textures canvas, this tool's picture and a paired game show it where
   it is used. Not right? **Discard** takes it out, or change the prompt
   or the references and press **Regenerate**.
7. **Accept** keeps it: it is an edit of the bitmap like any other and is
   written when you save the map. **Accept and save art to...** also
   copies the picture and a provenance file (the prompt, the
   references and the generator) into a folder you pick, such as one in
   the repository's `art/`.

The generator is ChatGPT's image generation through the Codex CLI,
signed in with your ChatGPT account. It needs `codex` installed and
`codex login` done once. When it is missing, signed out or out of
allowance the window says so in red, with what to do. Generated pictures
are original AI-made art. The agent API's `image.*` methods do the same
([agent-api.md](agent-api.md#generating-images)).

## 8. HUD

**HUD** shows a map's HUDs the way the game places them on the screen,
from the map's own art and the disc's fonts, and lets you fix their
layout: move a piece, change where it hangs from, swap its art, and save
the map. Use it when something on a HUD sits in the wrong place, such as
a converted Halo 1 map's shield bar over its health bar or a grenade
count spilling out of its box.

- **HUDs** (left):
  - **Map** (the menu at the top): the source's multiplayer maps and
    campaign levels, your packages and **Halo 1 sandbox variants**,
    then **Shared and menu**. Pick a map to open it in the HUD tool.
    The filter matches source maps by display name and file name.
  - **Filter**: one filter under the menu narrows the HUDs and the widgets.
  - **HUDs** (a folding header): every HUD of that map, grouped by who uses it: players and
    bipeds, weapons, vehicles, the globals, and the converted Halo 1
    HUDs. Hover one to see who names it. Click one to show it. A `*`
    marks a HUD with unsaved edits.
  - **Widgets** (a folding header): the HUD's pieces in the order the game
    draws them, art (a box) then text (a page). Click one to select it.
    The two lists share the panel's one scroll, so a short window scrolls
    the panel instead of squeezing a list.
- **HUD** (centre): the HUD on a 640 by 480 screen.
  - The menu at the top picks **fullscreen**, **half screen** or
    **quarter screen** (split screen; each widget has its own placement
    for each). **Zoom** makes it bigger. **Rects** outlines every widget
    (blue: this HUD; amber: a HUD drawn with it; grey: hidden by its
    state flags). **HUD frame** draws the frame the game hangs widgets
    from (green): its corners and centre are the anchors.
  - Click a widget to select it (white outline). Drag it to move it: that
    edits its offset for the screen mode shown, and the whole drag is one
    undo step. Hover to see the HUD pixel under the mouse and the widget
    there.
  - **State**, under the picture: shield and health, grenade counts and
    which type is selected, zoomed, rounds loaded and the magazine, heat
    and battery. Meters fill from these. **Gate widgets by their state
    flags** hides what the game would hide in that state (off: every
    widget is drawn). The game's state words are mostly not known yet, so
    with the gate on you can type them in (Word A to D); the grenade type
    and zoom set the bits that are known. **Inputs** lists the values the
    HUD's widgets read; the ones the editor does not know are 1 until you
    type a value.
- **Inspector** (right):
  - **Widget**: the selected widget's fields, in the same field panel as
    the **Tags** tool (section 9): its name, inputs, the words that show
    and hide it, anchor (which corner of the frame, or the centre),
    flags, bitmap and shader (pick from the map's tags), the sequence,
    offset and registration for each screen mode, and its special type;
    a text widget has its string, fonts and offsets instead of art.
    Hover a field for what it means and where that comes from. A text's
    **Justification** sits above the panel under **HUD fields** (the
    panel shows those bytes as a size it cannot change). Each change is
    an undo step of the HUD tool (typing in one field is one step).
  - **Checks**: art that half covers other art, art off the screen or
    outside the frame, and text that spills out of the art it sits on, or
    over other text. Hover one to outline its widgets in red. Art that
    sits wholly inside another piece (a panel over its meter) is a layer,
    not a problem, and is not listed. Under the checks, notes about what
    could not be drawn (a missing sequence, art that did not decode).
  - **Draw with (weapon held, vehicle seat)**: the game draws the
    player's HUD and the weapon's together. Tick a weapon's or vehicle's
    HUD to draw it with this one (read-only; open it from the list to
    edit it). The checks then also catch the two overlapping.
  - **Placement rules and their proof**: how the editor places
    widgets, each rule marked E (read in the game's code), V (seen on the
    disc or in a frame of the game) or D (assumed, not proven yet). The
    frame's top and right edges and the split-screen frames are D: if a
    widget near those edges looks wrong in the game, that is why.
  - **Save map as...**: writes a new `.map`, the source byte for byte
    except the fields you edited. It never writes over the map it opened.
    Play it with `H2_MAP_DIR=<that folder> h2 play` (section 11); the
    file must have the disc's map name.

What the picture does not show: widgets' animations and flashes, the
shaders' colours (art is drawn in its own colours, text in light blue),
and text other than numbers (a text shows its string ID's name in angle
brackets).

To open the editor straight on a HUD: `h2-editor --hud-map lockout --hud
masterchief`; add `--hud-widget <name>` to select a widget by its name.

## 9. Tags: every field of a tag

**Tags** shows every tag of the open map. Editable fields include a
weapon's numbers, a projectile's flags and a vehicle's weapon reference.
Open a map in **Maps** first (a disc map, one of your
packages, or a converted Halo 1 map), then select the **Tags** tab beside **Scene**.

- **Which map** (the list at the top left) groups the source's maps by
  type, with caches such as `shared`, `single_player_shared` and
  `mainmenu` at the bottom under **Shared and menu**. Browse another
  map's tags read-only or select the row marked **(open map)** to
  edit the Scenario tool's map. Selecting `shared` or
  `single_player_shared` in **Maps** opens its tags here.
- **The tag tree** (left) starts with **Scenario**. Expand Structure,
  Skies, Objects or Other references to see what the level uses. Select
  Globals to inspect the map's globals tag. Each object palette row gives
  the number of placements using that entry. Selecting it opens the tag
  and outlines its placements in the viewport. **All tags** offers
  **Groups** and **Paths**. Search with words such as `weap rifle`, each
  matching the group or name. **Not reachable from the scenario** helps
  find tags outside the walk from the scenario and globals. If that walk
  is incomplete, the section says so and leaves the list empty.
  A `*` marks an edit. `[Pending]` marks edited tag metadata waiting
  to reach a paired game. Double-click a tag to open it as a tab in the
  bar, in the editor its kind gets (a bitmap in **Textures**, the
  scenario in **Map**, the rest here); each tag's tab has its own undo. Shared-cache marks show where the data lives,
  such as `[shared]` for resources and `[in shared]` for tag fields.
- **The fields** (Inspector) are the selected tag's fields, top to bottom.
  Fields we have a name for show it in white; the rest show grey, named
  by where they are (`+0x14`), with their value shown the way it looks
  most likely (a decimal number, a whole number or hex). Right-click a
  grey field to see it another way. Hover a field for its type, where it
  is and what we know about it.
- **Scenario** shows the source fields read-only. Expand a placement or
  palette block and choose **Open … in Scene**. A placement link selects
  the displayed placement if it exists. A palette link selects the
  first placement that uses the displayed entry.
- **Structure BSPs and lightmaps** open in the field inspector. Their
  rows identify the BSP. Field and block edits save to that BSP's
  region, and blocks use their original capacity.
- **Data and bytes** (beside Log) shows the field you last clicked in the
  fields list; moving the mouse does not change it. **Field details**
  explains the field: what it is, its unit, its value in the map file
  when you changed it, with **Reset to file value**, and when the game
  sees a change. **Developer details** inside it has the path for
  scripts, the offset, the address and the bytes. **Bytes** (folded; **Show
  bytes** on a list element opens it) shows the raw bytes of the field's
  part of the tag, the field lit in blue and bytes you changed in orange.
  **Data and resources** lists each block of raw data inside the tag
  with its size, and each resource (pixels, model geometry, sound) with
  the map it is in (`local` for this one, or `shared`,
  `single_player_shared`, `mainmenu`), where and how big. A bitmap has
  **Preview**, which draws its first image.

### Field names

Hover a tag field to read its description in the tooltip and the detail
row. The editor includes these labels. A field with no established name
is labelled by its type and offset (`Value at +0x14`) and sits in the
folded **Unnamed fields** group at the end of its element, which says
what it holds; **Show unnamed fields** opens every group, and **Show
as** picks how an unnamed value reads (Decimal, Whole number or Hex). The
evidence line, when there is one, identifies an engine reader with `E` or
a disc check with `V`. See [Field names](editor.md#field-names).

How to change things:

- **Numbers**: drag left or right, or double-click and type. Angles show
  in degrees.
- **Choices**: click the value to open the picker, type to narrow it and
  pick a named value. **Enter a number...** sets a value no name covers;
  such a value reads `Value 7 (not named)`.
- **Switches (flags)**: the row says how many are on (`2 of 9 on: Loud,
  Quiet`). Open it to tick named switches; the rest are under **Unnamed
  bits**, each `Bit 7`.
- **Colours**: click the swatch.
- **Names (string IDs)**: type a name and press Enter. A name the map
  doesn't have yet is added to it when you save (under 128 characters).
  Undo takes it back. Maps with new names have not been tried in the game
  yet.
- **References to other tags**: the row shows the kind and the tag's
  name (hover for its full path). **Change...** opens the picker: type to
  narrow it and pick a tag, or **No tag**. Only tags of the right kind are
  listed, all of them; when several kinds fit, **Kind** narrows to one.
  **Open** jumps to that tag.
- **Lists inside a tag (blocks)**: open the list to see a row per entry,
  `1  scope`, named by its title or name field; open one entry to edit its
  fields (one at a time). A long list has a filter box. **Open alone**
  shows one entry as the whole panel, with breadcrumbs back to the tag.
  **Add**, **Duplicate** and **Remove** change the list. **Edit > Allow growing full blocks** lets lists in ordinary
  tag data grow beyond their original size. Save writes the longer list
  at the end of the map's tag data. Hover a greyed-out button for the
  reason. Other things in the map that point at
  a list entry by its number are not updated when entries move.
- **Data** (raw bytes inside a tag): edit the hex; the length must stay
  the same.

Some things are shown but cannot be changed, with the reason shown:
counts and addresses (use Add/Remove to change a block) and scenario
fields (use the Scenario tool for placements).

Every change is an undo step (one drag is one step). **Save**
and **Export map...** write your tag edits and placement edits together. The Scenario tool's
`*` shows when the map has unsaved edits of either kind.

### Export and import a tag

**Export...** above the fields writes the selected tag to a file of its
own, a *tag bundle* (`.h2tag`): its fields, its lists, the tags it
points at by name and its names as text. Tick **references** to put the
tags it points at in the same file (and the ones they point at), and
**resources** to copy its pictures and geometry in too (the file gets
large). You can export from any map, the shared ones included. Bundles
hold what is on your disc, so keep them out of Git repositories: the
editor won't write one inside a checkout.

**Import over this tag...** replaces the selected tag of the open map
with a bundle's tag of the same kind; **Import tags...** (with no tag
selected) replaces each tag of the open map that has the same group and
name as one in the bundle. An import is one undo step and is saved with
the map like any other edit. What to know:

- It replaces tags; it can't add a tag the map doesn't have yet.
- A tag it points at must already be in the map (by name), or the import
  stops and says which.
- Lists take the bundle's length. A list longer than the map's needs
  **Edit > Allow growing full blocks**, as when you add to it by hand.
- Raw data inside the tag keeps its length; a bundle with another length
  is refused.
- Pictures and models in the shared maps carry over between maps of the
  same disc. Ones inside the bundle's own map only carry over into that
  map; elsewhere the tag keeps the map's own, and the log says how many.
- Sounds only import into the map they came from.
- If anything is refused, nothing changes.

### Announcer lines

A voice is a document. The Browser's **Voices** kind lists the announcer
and every game character, each with its minutes of speech and whether it
is fine-tuned; double-click one, or find it in **Go to anything** (type
`voices` or `announcer`), to open it. It has two panes, **Setup** and
**Lines**. The announcer's **Lines** pane makes announcer lines from text.
Type the line, give a clip id (such as `superbounce`) if it is for one,
and press **Say it**. The voice says it, Studio trims the silence, brings
it to the loudness of the retail announcer lines of the open map and plays
it. The window shows the level it reached, how far it is from the retail
lines, and where the file is. The same text and voice give the earlier
file back from the cache without asking the service again.

The voice is Fish Audio's, and its key is in your Keychain. The first
time, the window shows the command to add it:

```
security add-generic-password -s fish-audio -a "$USER" -w '<your-fish-audio-key>'
```

Studio caches spoken lines under `<data>/announcer/`. Give the line a
clip id and use the document header's **Save** to make it a sound item of
the edit target. [announcer.md](announcer.md) describes the API export
for bundled art.

### Lines in a game character's voice

Open a character from the **Voices** kind. A voice with no dataset opens
on **Setup**, one that can say lines on **Lines**. **Lines** goes top to
bottom: **Lines** (type or paste many lines, one per line, or import a
text file; each row has an output name and the game sound it replaces),
**Say** (**Say all** says every row of this voice in turn; each row shows
queued, running, done or why it failed, plays, and re-runs on its own),
and **Save** in the header writes the voice's done lines to the edit
target. Buttons that are off say why.

A line for one sound is also made from the sound itself. A dialogue
sound's editor ends with a **Lines** pane: **New line** adds a line that
replaces that sound, said by the voice its path maps to. Type what is
said and press **Say**; the said line's waveform draws under the sound's
own on the same time axis, with its loudness beside it, so the two are
easy to compare. **Save** makes the line a `sound` item of the edit
target. A voice that is not set up says so on the row, and **Open its
Setup** opens the voice on **Setup**.

The first time, open the voice's **Setup**: the model picker, then **Build dataset** reads that
character's lines from your disc, cuts them into clean clips and pairs each
with its subtitle; the lines with no subtitle are transcribed on your Mac.
Studio also installs the voice model (a few GB, once); the window shows the
progress. A line is said in the character's voice, copying the delivery of
one of their own lines (name one under **Reference line**, or leave it to
Studio), and brought to the loudness of their lines on the disc.
**Fine-tune** trains the clone on the whole dataset for a closer voice; it
needs about ten minutes of their speech and takes a while.

Everything stays on your Mac: the clips, the transcripts, the model and
the lines are under `<data>/voices/` and Studio's caches, and nothing is
uploaded. A line with a clip id can be copied into a checkout like an
announcer line. [voice-cloning.md](voice-cloning.md) has the details.
