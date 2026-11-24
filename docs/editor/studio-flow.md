# Studio's flow: one way from any asset to its editors and to what it touches

Status: Partly built. The link table and `asset.links` (slice FL1) are built, and so are the ways to Voices (slice FL0): Go to anything lists every editor and the Voices window, and a sound's editor starts a Voices line that replaces it. The rest is design.
Code: `crates/apps/studio/src/app/links.rs` (`links`, `role_of`), `api/assets.rs` (`links`), `crates/apps/studio/src/app/commands.rs` (`palette`), `app/voices.rs` (`Show`, `App::voices_line_for`), `ui/sound.rs` (**New line in Voices**), `ui/announcer.rs` (`window`). Tests: `api/tests/links_tests.rs`, `app/tests.rs` (`go_to_anything_opens_the_voices_window_and_every_editor`, `a_sound_starts_one_voices_row_that_replaces_it`).
Switch: none.

Studio's editors were built one at a time, and each brought its own way
in, its own save, its own check and its own undo
([the survey](notes/2026-11-24-studio-flow-survey.md)). This page is the
one model they share. It sits on the shell of
[studio-redesign.md](studio-redesign.md) (the Browser, document tabs, the
inspector, Problems) and adds what that page leaves open: how a person
gets from one asset to the next. The rule that must hold: **every editor
is reached from the asset it works on, and every asset names what it uses
and what uses it.** No editor is reachable only from a menu.

Studio stays native Rust on Dear ImGui, with the theme of
`crates/apps/studio/src/theme.rs` and the layout of studio-redesign's
"The shell". Nothing here adds a new kind of chrome: the model is one
header strip, one menu and rows in panels that exist.

## How it works

### Everything is an asset

An asset is anything a person makes or changes. Each has an id, a kind, a
row in the Browser and a document tab.

| Asset | Id | Opens as |
|---|---|---|
| a tag | `tag:<group>:<path>` (`app::assets::AssetId::Tag`) | the editor its group gets (`app::kinds::KINDS`) |
| a package | `package:<id>` | the package document |
| a package item (a `.rules` script, a variant, a playlist, a screen, a HUD, a sound file, a model, an edits file) | `item:<package>:<n>` | the editor of what the item builds: the script editor, the variant's settings, the playlist table, the screen or HUD canvas, the sound editor, the model view, the tag the edits touch |
| a voice (a character's clone, the announcer's voice) | `voice:<character>` | the voice document ("Voices and the announcer") |
| a map | its scenario tag | the map editor |

A package item is the same document as the tag it builds. A `sound` item
and the `snd!` it builds are one tab; the header says which package the
tab writes to.

### One way in

There are four ways to an asset, and all four run `Action::OpenAsset`, so
they leave the same tab in front:

1. **The Browser** lists every asset. Its Kind picker
   (`app::assets::KINDS`) has **Game types** (scripts and variants),
   **Playlists** and **Voices** beside the tag kinds, and an item row of
   the Packages root opens on a double-click as a tag row does.
2. **Go to anything** (Cmd/Ctrl+P, `app::commands::palette`) searches the
   same rows the Browser lists (`asset.list`), not only the open map, and
   every command the menus hold.
3. **A link** on another asset ("Links").
4. **A Problems row** (`App::open_problem`).

**View > Editors** and **View > Voices** stay until slice FL4 and FL5
give their documents Browser rows, and then go: a menu entry that opens
an empty editor is a way in that only someone who already knows the
editor uses. Until then Go to anything lists each of them
(`Open editor: <name>`, `Open: Voices`).

The floating Sound, Animation, Voices and Generate windows become the
centre of their document or a pane of it, as studio-redesign's "What goes
away" says. A tab's editor is always in the centre.

### The document header

Every document draws the same strip above its editor (`ui/doc_header.rs`,
one function every surface calls):

```
+------------------------------------------------------------------------------------+
| < >  pogo > Sounds > sound\dialog\multiplayer\flavor\superbounce        [in Browser]|
| snd!  new in pogo   into: pogo *      Preview: (o) Studio ( ) My game  [In game]    |
| [Check 2]  [Undo] [Redo] [Revert]  [Save]              Links v   Open with v        |
+------------------------------------------------------------------------------------+
```

- **Where it is.** Back and forward (Alt+Left, Alt+Right; the history of
  `app/navigation.rs`, kept across every document kind, not only tags),
  then the trail: package, kind, path. Each part of the trail opens the
  Browser there. **In Browser** selects the asset's row.
- **What it is.** The kind, the origin badge the Browser shows (on the
  disc, **edited by** or **new in** a package), and the edit target the
  next edit goes to.
- **Preview, Check, Undo, Redo, Revert, Save** ("Shared patterns").
- **Links** and **Open with**.

### Links

`app/links.rs` answers one question for any asset: what it uses, what
uses it, and which editors open it. `asset.links {asset}` is the same
answer over the API, and the header's two menus and the inspector's
**Where used** section draw it.

```
{ "asset": "tag:weap:objects\\weapons\\...\\pogo_unsc",
  "uses":    [{ "asset", "role", "via" }],
  "used_by": [{ "asset", "role", "via" }],
  "editors": [{ "id", "label", "why_not" }] }
```

`role` is the plain word the menu groups by (Model, Animations, Sounds,
HUD, Projectile, Script, Voice, Package); `via` is the field path or the
manifest key the link was read from.

| From | Lists | Read from |
|---|---|---|
| any tag | **uses**: the tags it references, grouped by role | the tag's file in the tree; `via` is `node <n> +<offset>`; the role is the kind of the target (`app::links::role_of`) |
| any tag | **used by**: the tags that reference it | a scan of every tag file of the tree |
| any tag | **used by**: the packages that edit or add it (`via` is `edits` or `adds`) | the edit lines and items of the set (`app::assets::overlay`) |
| a `variant` item | **uses**: the `rules` item of its script | the files' stem up to the first `.`, the key the script registry gives both |
| a `rules` item | **used by**: the `variant` items of its script | the same key |
| a dialogue sound | **uses**: the voice that says it (`voice:<character>`) | the character whose rules pick its path (`voice::dataset::CHARACTERS`) |
| an `edits`, `tag`, `bitmap`, `sound` or `model` item | **uses**: the tags it edits or adds; **used by**: its package | the item's source and tag |
| a package | **uses**: its items and the packages it `requires`; **used by**: the packages that require it | the manifest |

Not built yet: a voice's lines (needs the voice document, FL5), a
variant `.h2variant` record's script prefix, and the bitmaps and strings
of a screen or HUD. A file that does not decode is named in `problems`
and the rest of the answer stands.

**Used by** is the same table read backwards, from a scan of the tree's
tags and of the set's manifests. A link menu lists at most 20 rows per role and ends with
**Show all in the Browser**, which sets the Browser's search to the
query that lists them.

**Open with** lists the editors of the front asset: its kind's own
editor, **Fields** (the field panel, which every tag has), and the
editors of other kinds that work on it. For a dialogue sound that is
**Voices**; for a variant, **Script**; for a weapon, **Model** and
**HUD**. An editor that cannot open says why in the row (`why_not`), so
the menu also answers "why is there no editor for this".

Following a link pushes the navigation history, so Back returns to the
weapon from its model with the weapon's selection as it was.

### Shared patterns

Each of the four is one control in the header, one keyboard shortcut and
one API method, whatever the kind.

| | Control | Does | Replaces |
|---|---|---|---|
| **Preview** | the target switch and **In game** (Cmd/Ctrl+Enter) in the header; the kind's own picture is the centre of the document | `preview.target`, `pair.preview` for the front document (`app/preview_switch.rs`); a variant or a script previews by starting or reloading Studio's own game with it (`game.start`, `rules.reload`), and the Game panel comes forward with the result | the Preview menu as the only control; the script editor's **Reload**; choosing the variant in the game's menus |
| **Check** | **Check** in the header, with the count of this asset's findings | `check.run` narrowed to the asset, and the kind's own check (`ui.check`, `hud.check`, `package.check`, the script's analysis); the findings go to Problems, and the editor shows Problems' rows for this asset, drawn by the same row function | the HUD's and the screens' **Checks** lists and the script editor's **Check** section as private lists |
| **Apply** | an edit applies to the document at once. **Save** (Cmd/Ctrl+S) writes the document to the edit target ("into: pogo"), and only there | `app::edit_target`; a variant, a script and a playlist save as `variant`, `rules` and `playlist` items of the target; a voice line saves as a `sound` item | the animation editor's own package picker (`ui/anim/save.rs`), **Keep all done lines** and **Copy into the checkout** as separate destinations, the data folder's loose variants and scripts |
| **Undo** | Cmd/Ctrl+Z, and **Undo**, **Redo** in the header, on the front document's stack (`Action::UndoDoc`, `App::doc_can`) | every editor's steps are steps of its document | the private **Undo** buttons of Modeling, Textures, Animation, the mouth curve and the script's compile |

**Revert** puts the document back to the edit target's saved state and
is one undo step. **Export...** (a `.map`, a `.h2variant`, a WAV, a PNG)
is the one way a file leaves the edit target, and is in the File menu
for every kind that has one.

A kind with no picture of its own says so in the centre and offers **In
game**. A control that cannot run is drawn disabled with its reason on
hover, as `ui/announcer.rs`'s `action` does.

### Voices and the announcer

Voices are assets, and making a line is part of editing a sound.

- **A voice is a document.** The Browser's **Voices** kind lists each
  character of `voice::dataset::CHARACTERS` and the announcer's Fish
  Audio voice, each row with its minutes of speech and whether it is
  fine-tuned (`ui/announcer.rs` `VoiceInfo`). Opening one gives the voice
  document: **Setup** (dataset, model, fine-tune, the jobs' progress)
  first while the voice is not ready, then **Lines** (the batch of
  `app/voice_batch.rs` with this voice as its default). Its **Links**
  list the lines it made and the sounds they replace.
- **A dialogue sound has a Lines pane.** The sound document is the
  waveform and the mouth curve with a **Lines** pane beside the
  permutations. The pane is the batch's rows whose target is this sound.
  **New line** adds a row with the sound as its target and the voice the
  sound's path maps to; the said line draws its waveform and levels on
  the same axis as the original, under it. **Save** makes it a `sound`
  item of the edit target.
- **The announcer's own clips** (`art/announcer/`, the medal lines) are
  `sound` items of the package that ships them, so they are rows under
  Sounds and open in the same document with the same Lines pane. A line
  with a clip id and no game sound saves as a new item of the edit
  target.

Until the voice document exists (FL5), the window is the floating Voices
window, reached three ways: **View > Voices**; Go to anything (`Open:
Voices`, also found by "announcer", "clone" and "dialogue"); and **New
line in Voices** at the top of the sound editor, which runs
`App::voices_line_for`: the batch gets a row that replaces the sound in
front (one row per sound, however often it is pressed), and the window
opens in front on the character batch (`app::voices::Show::Lines`).

### The three journeys in this model

**A new weapon.** Browser > Objects > a weapon > **New from this...**
makes the copy in the edit target (`asset.copy`), asking for a package
first when there is none (the edit target picker, with **New
package...**). The copy opens. **Links** lists its model, animations,
sounds, HUD and projectile by role; each opens its own editor, and Back
returns. **Check** counts the weapon's findings and those of what it
uses. **In game** holds it in Studio's own game. **Save** writes every
changed document to the target. The package document builds and
publishes it.

**An announcer line in a cloned voice.** Browser > Sounds > the line.
**New line** in its Lines pane; type the text; **Say**; compare the two
waveforms; **Save**; **In game**. A voice that is not set up says so in
the row, and its link opens the voice document's Setup.

**A game type.** Browser > Game types > **New script...** opens the
script editor as a document of the edit target. **Open with > Variant**
makes or opens a variant of it. **In game** starts Studio's own game on
the variant and brings the Game panel forward; a save reloads it.
**Save** makes both items of the package. The package document's
**Check**, **Build**, **Play** and **Publish...** finish it.

## Interfaces

- `asset.links {asset}` (built): the link table above, in
  `crates/apps/studio/src/api/assets.rs`; it also answers `problems`,
  the files it could not read.
- `asset.list` gains the kinds `Game types`, `Playlists` and `Voices` and
  rows for `voice:<character>`.
- `shell.open {asset}` takes `item:` and `voice:` ids.
- `check.run` takes `asset` and answers only that asset's findings.
- `ui.palette` lists every asset `asset.list` does, and every command.
- `Action::ShowVoices` and `App::voices_line_for` (built) are the window's
  and the sound editor's way to the Voices window.

The window has no action the API lacks; each slice's test drives it
through the method.

## Slices

One lane each. S is under 800 lines with tests, M under 2,500. Every
test is headless on invented data. Paths are under
`crates/apps/studio/src/`. Rows are in build order; FL1, FL3 and FL8 need
nothing unbuilt and run together.

| # | Slice | Size | Needs | Files | Accepted when |
|---|---|---|---|---|---|
| FL0 (built) | **Ways to Voices**: the editors and Voices in Go to anything; **New line in Voices** on a sound | S | nothing | `app/commands.rs`, `app/voices.rs`, `app.rs`, `ui/sound.rs`, `ui/announcer.rs` | `go_to_anything_opens_the_voices_window_and_every_editor`; `a_sound_starts_one_voices_row_that_replaces_it` |
| FL1 (built) | **The link table**: `app/links.rs`, `asset.links`; tag references by role, packages of a tag, variant and script, sound and voice, package and items | M | nothing | `app/links.rs` (new), `api/assets.rs`, `api/tests/links_tests.rs` (new) | on an invented map and package: a weapon lists its model under Model and its sound under Sounds; the model lists the weapon under Used by; a variant lists its script and the script the variant; a tag an edit line touches lists the package; a sound under an invented character's path lists that voice |
| FL2 | **The document header**: the strip on every surface, with the trail, the origin, the edit target, **Links** and **Open with** drawn from FL1 | M | FL1 | `ui/doc_header.rs` (new), `ui.rs`, `ui/tags.rs`, `ui/variants.rs`, `ui/playlists.rs`, `ui/ui_screens.rs`, `ui/hud.rs`, `ui/textures.rs`, `headless.rs` | each kind's document draws the header at 1000 by 420 with no stray child window; the header's text for an invented weapon names its package and its roles; following a link and going Back returns the first tab with its selection |
| FL3 | **Go to anything over everything**: the rows of `asset.list` for every root, packages and items, and every menu command | S | nothing | `app/commands.rs`, `ui/palette.rs`, `api/ui_screens.rs`, `app/tests.rs` | with no map open, a query finds a tag of an invented tree, a package, an item and a command, each under its kind; running each opens its document |
| FL4 | **Items open**: item rows open the editor of what they build; the **Game types** and **Playlists** kinds; scripts, variants and playlists as tabs with `item:` ids; **View > Editors** removed | M | FL2 | `app/assets.rs`, `ui/browser.rs`, `app/shell.rs`, `app/rules_edit.rs`, `ui.rs`, `api/shell.rs` | a double-click on an invented package's `rules`, `variant`, `sound` and `edits` items opens the script editor, the variant's settings, the sound editor and the edited tag; `shell.open {asset: "item:..."}` leaves the same tabs |
| FL5 | **Voices as documents**: `Doc::Voice`, the **Voices** kind, the voice document (Setup, Lines), the sound document's Lines pane with the two waveforms; the floating window and **View > Voices** removed | M | FL2, FL4 | `ui/announcer.rs` split into `ui/voices/{setup,lines}.rs`, `ui/sound.rs`, `app/shell.rs`, `app/assets.rs`, `app/voice_batch.rs`, `api/voice_clone.rs` | with the fake sidecar: the Voices kind lists every character with its dataset state; a line said from an invented sound's Lines pane is a row whose target is that sound and whose levels draw under the original's; a voice with no dataset opens on Setup |
| FL6 | **One check**: `check.run {asset}`, **Check** in the header with the count, the editors' lists drawn from Problems | M | FL2 | `app/problems.rs`, `ui/problems.rs`, `api/checks.rs`, `ui/hud.rs`, `ui/ui_screens.rs`, `ui/game_script.rs` | a finding on an invented HUD shows once, in Problems and in the HUD document, with the same text; the header's count for an asset equals `problems.list` filtered to it; a script error is a Problems row that opens its line |
| FL7 | **One save**: every document saves to the edit target; variants, scripts and playlists as items; voice lines as `sound` items only; the animation save picker removed | M | FL4 | `app/edit_target.rs`, `app/package_doc.rs`, `app/rules_edit.rs`, `app/voice_batch.rs`, `ui/anim/save.rs` (deleted), `ui/variants.rs`, `ui/playlists.rs` | with a target set, saving an invented variant, script, playlist, animation and voice line each adds or updates one item of the target and writes nothing outside `<data>/packages/`; with none, each asks through the one picker, and the API answers `NO_TARGET` |
| FL8 | **One undo**: the editors' own Undo buttons run `Action::UndoDoc` on the front document; the mouth curve, Modeling, Textures and Animation steps are steps of their tab | S | nothing | `app/shell.rs` (`undo_redo_doc`, `doc_can`), `ui/sound.rs`, `ui/modeling.rs`, `ui/textures.rs`, `ui/anim/edit.rs`, `app/sound_mouth.rs` | a mouth redraw and a field edit on one invented sound undo in the order made with Cmd+Z; two documents keep two stacks; no `ui/` module holds an undo button that is not `UndoDoc` |
| FL9 | **One preview**: the target switch and **In game** in the header; a variant or script in front starts or reloads Studio's own game with it and brings the Game panel forward | M | FL2 | `ui/preview_switch.rs`, `app/preview_switch.rs`, `api/preview.rs`, `play.rs`, `ui/game_script.rs` | on the fake game: **In game** on an invented variant sends the start with that variant, and on its script sends `rules.reload`; each kind's plan is the one `pair.preview` lists; the Preview menu and the header leave the same state |
| FL10 | **The package document**: the manifest, the items, **Check**, **Build**, **Play**, **Publish...**, **Export...**, **Set as edit target**, **New package...**, and **Add to package** from any header | M | FL4 | `ui/package.rs` (new), `app/package_doc.rs`, `ui/browser.rs`, `ui.rs` | every button runs the `package.*` method studio-redesign's "Studio's side" names, shown by the same result; a new package made in the window is the edit target and is listed by `package.list` |
| FL11 | **New from this**: **New from this...** on a Browser row and in the header (the window form of `asset.copy`), **Import sound...** (`import.sound`) and **Add model...** (a `model` item), each opening the new document | M | FL10 | `ui/browser.rs`, `ui/doc_header.rs`, `app/assets.rs`, `api/import.rs` | a weapon copied from an invented one opens as a tab marked **new in** the target and holds no bytes of its source; an imported WAV opens in the sound editor |
| FL12 | **The guide by journey**: [editor-guide.md](editor-guide.md) and [editor.md](editor.md) describe the header, links and the four patterns, with the three journeys as walkthroughs | S | FL5 to FL10 | `docs/editor/editor-guide.md`, `docs/editor/editor.md`, `headless.rs` (the screenshots list) | `scripts/docs-check.py docs/editor` is clean; the screenshots regenerate from documents |

## Rules and limits

- An editor with no asset row is not added. A new editor names its asset
  kind, its Browser row and its links in the slice that builds it.
- A link is read, never stored: the tables come from tag references,
  manifests and the batch each time, so a link cannot go stale.
- One save destination. A file written outside the edit target is an
  **Export**.
- The header is one function. An editor does not draw its own Save,
  Check, Undo or Preview control.
- Link menus are capped at 20 rows per role; the Browser holds the rest.
- Voice datasets and weights are never assets of a package
  ([voice-cloning.md](voice-cloning.md), "Rules and limits"): a voice's
  row is local to the machine, and only its lines save.
- No disc bytes cross a link: **New from this** writes a `like:` tag, as
  `asset.copy` does.

## Evidence

- The entry points, the saves, the checks and the undo buttons of the
  window before this design: **Code**, listed with their files in
  [the survey](notes/2026-11-24-studio-flow-survey.md).
- Go to anything lists the editors and Voices, and running the entry
  asks for the window: **Test**
  `go_to_anything_opens_the_voices_window_and_every_editor`.
- A sound's **New line in Voices** makes one row per sound and asks for
  the batch: **Test** `a_sound_starts_one_voices_row_that_replaces_it`.
- The header, the link roles, the four patterns and the voice document:
  **Design**.

## Not verified

- That the Voices window comes to the front over a docked layout when
  asked for from the sound editor (the window is focused by
  `ui.set_window_focus`, as the Game panel is).
- Whether a role per reference is right from the target's kind alone: a
  weapon's first-person and third-person models are both Model.
- How long `asset.links`' reverse scan of manifests takes on the full
  set.
- That starting Studio's own game on a variant needs nothing from the
  game beyond `game.start`.

## Related

- [studio-redesign.md](studio-redesign.md): the shell, the Browser,
  packages and the edit target this page moves through.
- [live-editors.md](live-editors.md): the kind registry and the preview
  targets.
- [announcer.md](announcer.md), [voice-cloning.md](voice-cloning.md): the
  line pipeline and the clone.
- [game-view.md](game-view.md): the Game panel and Studio's own game.
- [agent-api.md](agent-api.md): the methods named here.
- [notes/2026-11-24-studio-flow-survey.md](notes/2026-11-24-studio-flow-survey.md):
  the journeys as they are in the window.
