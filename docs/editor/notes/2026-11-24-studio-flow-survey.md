# How a person moves through Studio: survey

Evidence for [studio-flow.md](../studio-flow.md). Nothing here states what
the code does today; the design page does.

## 2026-11-24: three journeys, read from the code

Commit `12ae05214` (master). Method: static reading only. Nothing was
built, tested or run. The window was read from
`crates/apps/studio/src/ui.rs` (`Studio::frame`, `menu_bar`), the modules
of `src/ui/` and `src/app/shell.rs`, `app/assets.rs`, `app/commands.rs`
and `app/problems.rs`, against [editor-guide.md](../editor-guide.md) and
[studio-redesign.md](../studio-redesign.md). To repeat it: take each
walk-through below in the window and compare each step with the file named.

Each step is marked with what goes wrong there:

- **hidden**: the way in has to be known; nothing on screen leads to it.
- **lost**: what was in front is not carried to the next editor.
- **dead end**: the window stops and the step is done outside it.
- **differs**: the same thing is done another way in another editor.
- **unfindable**: the asset does not say which editor works on it.

### What the window is

One window, a bar of document tabs and a dock (`ui.rs`, `ui/bar.rs`,
`ui/doc_tabs.rs`). What is drawn under the bar is the front document's
`app::Surface`: the map surfaces (Browser, Scene, Tags, Scripts, viewport,
inspector) or one of Variants, Playlists, UI screens, HUD and Textures,
each with its own windows. On top of that float five windows that are not
documents: Sound (`ui/sound.rs`), Animation (`ui/anim.rs`), Voices
(`ui/announcer.rs`), Generate (`ui/image_gen.rs`) and the first-start
page.

Ways in:

| Way in | Reaches | Does not reach |
|---|---|---|
| Browser double-click (`ui/browser.rs` `row`) | a tag, in the editor its group gets; a package's item list | a package item (a row with no action), a variant, a playlist file, a `.rules` script, a screen or HUD of ours, a voice |
| Go to anything (`app::commands::palette`) | maps, tags of the open map, placements, 15 menu commands | packages, items, variants, scripts, playlists, screens, HUDs, the editors of View > Editors, the Voices window |
| View > Editors (`Surface::EDITORS`) | Map, Game variants, Playlists, UI screens, HUD, Textures | the script editor (reached from inside Game variants), Sound, Animation, Voices |
| View > Voices | the Voices window | |
| A reference row in the Tags inspector | the tag it names | anything that is not a tag reference |
| A Problems row (`App::open_problem`) | a tag and its field, a package in the Browser, a tool | |

### Walk-through 1: a new weapon, end to end

| Step | In the window | Problem |
|---|---|---|
| Find a weapon to start from | Browser, Kind: Objects | |
| Copy it into my package | right-click, **Copy into package...** (`Action::CopyAsset`); with no local package the entry is greyed and its tooltip says to make one under `<data>/packages/` | **dead end**: no window makes a package (`package.new` is API only) |
| Open the copy | Browser, Packages root, the package, its items: the item row does nothing on a double-click | **dead end**; the copy is opened from the Tags root by its path |
| Tune it | the weapon opens in the Tags surface: the field panel | |
| Go to its model | the reference row's link opens the `hlmt`, then the `mode`; the Modeling section is in the map inspector | **lost**: the weapon tab is behind; nothing on the model says which weapon brought it. **unfindable**: the model row does not say Modeling edits it |
| Replace the model | a `model` item of the package, written by hand as YAML | **dead end**: no window form |
| Its sounds | follow references to each `snd!`; the Sound window floats only while the Tags surface shows a sound (`ui/sound.rs` `target`) | **differs**: a floating window, not the tab's own editor |
| Its animations | the Animation window, floating over a `jmad`; it saves with **Save to package...** and its own package picker (`ui/anim/save.rs`) | **differs**: every other tag edit goes to the edit target |
| Its HUD | View > Editors > HUD, then pick the map again in the HUD's own list (`ui/hud.rs`) | **hidden**, **lost**: the weapon's `nhdt` reference does not open the HUD editor on that HUD |
| Markers | `markers.*` is API only | **dead end** |
| Check it | **Problems** > **Check** reads every source; the HUD and UI editors have their own **Checks** lists and the script editor its own **Check** | **differs** |
| See it in the game | Preview > **Preview in game** (Cmd+Enter) for the front tab; the result is a job and a log line | **hidden**: the Preview menu is the only control; the document has none |
| What uses it | the inspector's header says "shared by n placements, n tags"; the list is `tag.uses` (API) | **unfindable** from the weapon outward to the packages and variants that name it |

### Walk-through 2: an announcer line in a cloned voice

| Step | In the window | Problem |
|---|---|---|
| Find the line to replace | Browser, Kind: Sounds, the announcer's folder; the sound opens with the Sound window | |
| Get to the voice | View > Voices. Nothing on the sound, the Browser or Go to anything names it | **hidden**, **unfindable**: the report this survey answers |
| Carry the sound across | in Voices: pick **Game character**, pick the voice, add a row, open the row's **Replaces** picker and find the same sound again by name | **lost** |
| First use | **Voice setup** is a collapsed header at the bottom of the flow that the first run needs first | **hidden** |
| Say and review | **Say all**, each row plays | |
| Compare with the original | the Sound window shows the original's waveform and levels; the new line's are a text line in Voices | **lost**: two windows, no side by side |
| Keep it | **Keep all done lines**: a row with a tag goes to the edit target, a row with only a name goes to `art/announcer/` of the checkout; the Fish Audio line has **Copy into the checkout** | **differs**: two destinations by row shape, neither named Save |
| Hear it in the game | Preview in game needs the sound's tab in front again | **lost** |
| Find the new line later | it is a `sound` item of the package; the item row opens nothing | **dead end** |

### Walk-through 3: a game type, tested in the Game view, packaged as a mod

| Step | In the window | Problem |
|---|---|---|
| Start a script | View > Editors > Game variants, then **New** in the script editor docked beside the settings (`ui/game_script.rs`) | **hidden**: the script editor has no entry of its own; a second, read-only **Scripts** tab on the map surface shows HaloScript and has a similar name |
| Check it | the editor's own **Check** section | **differs** from Problems |
| Make a variant of it | **New from script...** in the variants list | |
| Play it | F5 or Game > Play starts a game on the open map; the variant is chosen in that game's own menus. **Reload** sends the script to a paired game | **lost**: Play does not know the variant in front |
| Watch it | the Game panel in the bottom dock | |
| Save | the variant saves into the data folder's saved variants; the script saves to a `.rules` file (`scripts/<name>.rules`); neither is in a package | **differs** from the edit target |
| Package it | `package.item.add` (API), or the manifest by hand | **dead end**: no window lists a package's manifest, adds an item, checks, builds or publishes |
| Share it | **Export...** writes a `.h2variant`; `package.publish` is API only | **dead end** for the package |

### What repeats

1. **Several ways in, none complete.** The Browser lists tags and stops
   at package items. Go to anything lists the open map. View > Editors
   lists five surfaces. Voices, the script editor, Sound and Animation
   are in none of the three lists.
2. **A link carries a tag and nothing else.** Tag references are the only
   links. Variant to script, sound to voice, weapon to HUD, item to tag,
   asset to package are not links, so the person finds the second asset
   again by name.
3. **Five floating windows and five surfaces sit beside the tabs.** A
   tab's editor is sometimes the centre, sometimes a floating window,
   sometimes another surface with its own list.
4. **Six places to save**: the edit target, the animation's own package
   picker, the checkout (`art/announcer`, the UI package), the data
   folder (variants, scripts), a playlist file, a map file.
5. **Four checks**: Problems, the HUD's, the screens', the script's.
6. **Six undo buttons** besides Cmd+Z: Tags, Modeling, Textures,
   Animation, the mouth curve, the script's compile.
7. **Packages have no window.** Making, listing, checking, building,
   playing and publishing a package are API calls.
