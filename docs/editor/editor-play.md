# The editor for developers: play, variants and playlists

Status: Built.
Switch: none.

Play and pair, embedded play, game variants and playlists. The index is [editor.md](editor.md).

## Play and pair

The **Game** panel (a tab beside the log, scenario tool) starts the game
on the map being edited, or attaches to a game already running, through
the game's agent socket ([agent-api.md](../runtime/agent-api.md)). The
code is `studio::play` (`play.rs`, `play/{launch,link,sync}.rs`) and
`ui/play.rs`; nothing of it runs the game in the editor's process.

**Play** (F5) takes the open document into the game. With a matching map
it sends pending pokes, resumes a paused game and raises its window if
the game lists `editor.focus`. Structural edits require a freshly written
play copy. The editor calls `map.reload {file}` when available, relaunches
its own game otherwise, or leaves the edits pending for a manual load in
a foreign game. F5 leaves the Edit switch as set. A foreign game asks before receiving edits or switching
maps. The other choice in the map prompt opens the game's map here.
`play::playtest::decide` supplies the bar, Game menu and socket API.

`game.start` ([agent-api.md](agent-api.md)) and a launching F5 are the same
start (`Play::start_own`): the game runs with `--editor`, no window of its
own, and is shown in the Game panel ([game-view.md](game-view.md), "Own
mode").

**Stop** (Shift+F5) closes the game the editor started. With no game,
Play launches the copy in editor mode, with no window of its own:

```
H2_AGENT=<cache>/h2recomp/editor-<pid>-<n>.sock H2_MAP_DIR=$TMPDIR/h2-editor-play-<pid>/maps \
  h2 play <disc> --data <data root> --level <map> --title 'Halo 2 - <map> (h2-editor)' --editor
```

- `h2` is the path in Game > Advanced, else `$H2_BIN`, else the `h2` beside the
  editor's executable (a checkout's `target/<profile>/h2`), else
  `~/.cargo/bin/h2`, else `h2` on the `PATH`, else the lab host's copy
  (`launch::find_h2`). The editor never builds it.
- The disc is the open image or the extracted disc containing the open
  `maps` folder or file. Otherwise Play uses `H2_DISC` or the repository's
  `disc/halo2.iso` (`launch::source_disc`, `editor_core::default_disc`).
  Before stopping its running game, the editor resolves the launch level
  with the same `level_boot::resolve_in` as `h2 play`. For a Map Pack map
  this is still the Halo 2 disc: `h2 play` installs the map pack itself from
  `mappack.iso` beside that disc (the `map-pack` package's `convert` item,
  `package_prepare::before_boot`) and the
  editor's copy plays through the override. When the open pack is
  elsewhere, the editor logs that `h2 play` will not install it (it does
  not run `h2 import-mappack` itself).
- A package opens under its manifest's `base.map`. If the document name
  does not resolve, `launch::write_launch_copy` tries the internal cache
  name and scenario path. It writes the edited bytes under the resolved
  level's file name for `H2_MAP_DIR`. The document stays unsaved. When
  the cache name and scenario resolve to different levels, or none of the
  names resolves, Play keeps the running game and explains how to load
  the map from the game's map list.
  [Map packages](../game/maps.md#8-saving-packages-from-the-editor)
- `--level` boots the level with one default player
  ([level-boot.md](../runtime/level-boot.md)).
- **Spawn** offers the level's own spawn, the editor camera, the selected
  placement and **Where I was**. The last becomes the default once the
  stream supplies a local player on this map. After a reload the editor
  checks the copy's checksum and the requested activation (a new epoch on
  hosts without the activation counter), then waits for gameplay and
  at least 30 ticks before restoring position, yaw and pitch with `sim.state.set`.
- **Game > Pause the game while I edit** pauses on editor window focus.
  It defaults to off. F5 resumes a paused game.
- The game's output goes to `play-<n>.log` in the scratch directory.
  Game > Advanced shows the path. The editor waits up to 20 minutes for
  the socket because `h2 play` may build its image first.

**Pair** attaches to a running game: one it found, or a path typed (or
`tcp://HOST:PORT`). `play::pair::Finder` looks every second on its
own thread: the agents directory (`agent_proto::discover`, where `h2 play`
opens `<pid>.sock` unless `H2_AGENT` is set; leftover sockets nothing
answers on are removed) and the sockets **Find** lists (`$H2_AGENT`, the
default socket, and every socket in the agent's cache directory, where
Play puts its own). Each is asked `session.status`. Click the game link
pill to pair with the single discovered game or choose from the list
when several are running. The list shows each game's map and phase.
It also offers **Unpair** and **Auto-pair**. Auto-pair defaults to on and
chooses the editor's own game, the sole game, or the only game on the open
map (`play::auto::choose`). A deliberate Unpair skips the process across finder scans until it exits.
If no PID is known, the socket stays unpaired until the user pairs it again. An empty path in Game > Advanced pairs with the default socket.
The game is taken to hold the map as the editor shows it at that moment.

`Play::link_view` supplies the pill in the bar and the Game panel, and
the `link` in `pair.status`.

| Pill | Meaning |
|---|---|
| No game | Play is available when a map is open. |
| `Halo 2 is running <map>` | Click to pair. |
| Connecting… | Click for **Cancel**. |
| Reconnecting… | The editor retries the discovered or explicitly named socket for up to 30 seconds. Click for **Cancel**. A matching epoch and checksum preserve acknowledged edits. An interrupted write requires a fresh journal check. |
| `Viewing <map> · <phase>` | The editor reads the running game in View mode. |
| `Editing <map> live` | The Edit switch is on and tag and placement edits can reach the game. The viewport frame and the inspector's Live pill are green. |
| Edits stay here | The Edit switch is on and the game is blocking writes. Hover for the reason. The frame and Live pill are amber. |
| `Game runs <map>` | The game is on another map. Click for `Open <map>`. |
| Game lost | Hover for the reason or click for **Pair again**. |

Pairing with an already-running game starts in View. Play starts its own
game in Edit. Use the **Edit** switch in the bar or Game panel to change
mode. The Game panel's Session section shows the map, phase, tick,
session kind, stream rate, delay and counters. Player holds the camera,
teleport and spawn controls.

While paired, `play::Play` keeps a control connection for commands and
status, and a second connection for the binary live mirror.

- **Live objects** use their definition's model in the bind pose.
  Player labels show the name and team, and an aim line follows the
  unit's targeting vector. The viewport interpolates streamed transforms
  between game ticks ([live-mirror.md](live-mirror.md)).
- **Edits sent live.** The editor compares changed tag and placement fields
  with the bytes it expects in the loaded map. Edit sends these fields in
  one conditional `edit.batch`. Undo and redo use the same path. A conflict
  leaves the entire batch unapplied. The inspector and tree mark refused
  fields in red and conflicts in amber. The bar and the game's editor
  plate show the largest issue count. Clicking the bar's count opens
  **Pending**. On a conflicted field, choose **Take the game's value**
  to put the game's value in the document or **Overwrite** to send the
  document's value. The field badge shows when the game reads it.
  Moving or turning a placement also moves its linked live object. Deleting
  a placement deletes that object. **Respawn now** applies creation-time
  edits using the loaded placement. Pending shows when a changed block or
  an appended placement needs **Reload**.
- **Go to player** puts the camera's focus on player one, looking the way
  they face. **Teleport player here** moves them to the ground under the
  camera's focus; **T** over the viewport moves them to the point under
  the cursor (`sim.state.set`).
- **Pause**, **Step** (1 to 600 ticks, set in Game > Advanced) and **Resume** (`session.claim`,
  `pause` with the 10-minute limit, `step`, `resume`, then `release`, so
  other clients may act again). In `h2 play` a pause also stops the
  window's event loop (agent-api.md).
- **Co-op.** In a synchronous session (co-op host or guest), as a
  multiplayer client, or when the session can't be read, the editor
  sends no edits, no teleport and no spawn (`SessionKind::writes`, the
  rule `sim.state.set` already enforces in the game): a write on one
  machine desyncs the others.

Tests (`play::tests`, against the agent crate's server and executor with
an invented game behind them, no game data): pairing and streamed
objects, Go to player, tag and placement edits in one `edit.batch`,
both choices when the game's copy differed, undo, an added
entry as a reload, teleport and the spawn, no writes in co-op or with
another map loaded, pause/step/resume and control given back, a lost game,
nothing listening, and the play copy of the map (an edit in it, the map
still unsaved). `play::sync::tests` and `play::launch::tests` cover the
diff and the command line.

Not yet:

- **Checking the game's copy.** Pairing trusts that the game runs the map
  as the editor shows it; a field whose `old` differs is reported once.
  Reading the game's placements back (`edit.get` per block) would show
  the difference before the first edit.

### Embedded play (not built)

The game's picture inside Studio is designed in
[game-view.md](game-view.md): a Game panel fed by a shared-memory ring
of the game's frames, an own game in editor mode and an attached mode,
with the game always a separate process.

## Game variants

The variant editor of [variants.md](variants.md), in the same windows:

- **Variants** (the outliner's place): New (pick a game type; a new
  variant has the type's defaults), the saved variants, the built-ins.
- **Settings** (centre): the open variant's name and its settings in the
  game's pages, each a combo of the values the game offers, a checkbox or
  a bounded number; a setting changed from the type's default is bold,
  and its help and default are on hover. Built-ins are read-only, as in
  the game: Copy makes a named, editable copy.
- **Inspector**: Revert, Delete for an imported game save, Export... (a portable `.h2variant` file), Import... (a
  file, or pasted text), whether the game would accept it, and the
  changes from the type's defaults.

The schema comes from the disc's `mainmenu` map (the built-in model
without a disc); save names come from the XBE (`H2_XBE`, else
`default.xbe` beside the disc image) (`editor_core::variant_sources`).
The document header's **Save** writes a `variant` item in the edit
target. Use **Export...** to share a portable variant file. Typing a
name is one undo step.

## Playlists

The native form of the web admin's playlist builder (docs/live/live-admin.md
§3, `web/src/admin/HopperBuilder.tsx`), working on a file instead of the
server's copy:

- **Playlists** (the outliner's place): New (an all-zero image, every
  playlist empty), Default lineup (the playlists a Live server serves
  before any file is published), Open..., then the 16
  playlists (a mark on those that would not save). Open reads either form
  of `multiplayer_hopper.dat`: the raw 0x162cc0-byte image the console
  caches, or the download envelope (header, size, zlib) the server
  publishes. The document header's **Save** writes that form as a
  `playlist` item in the edit target. **File > Export playlist file...**
  writes a copy. An envelope larger than the
  game's 128 KiB download buffer is refused (the game would keep its
  built-in playlists).
- **Entries** (centre): the playlist's title (15 bytes) and description
  (127 bytes) and its entries: the map (a picker of the disc's
  multiplayer maps, by name; numbers without a disc), the game variant,
  Edit, the weight (1..1000) and the resulting chance, and move,
  duplicate, remove; Add an entry (copies the last). A playlist keeps
  1..100 entries. Validation errors are listed below.
- **Inspector**: start from a historical preset, the variant picker's
  sources (load a `.h2variant` file, re-read the saves), and **Advanced:
  raw fields** (the raw name, type, status, the three range fields, each
  entry's raw string, flags and second dword).

**Game variants.** Each entry shows its game variant (name, game type,
teams), decoded from the 0x130 bytes its slot will carry, with
"(picked)" once changed. Its combo offers the default lineup's
matchmaking variants (the records of scripts,
`live_proto::hopper::defaults`), your saved variants (the runtime's save
storage, re-read with **Refresh saved**) and `.h2variant` files loaded
this session (typed path and **Load**, or **Browse...**), each only when
it runs a rules script (mode 2): a hopper plays scripted variants only,
so the disc's built-in variants are not offered and
`model::set_variant` refuses a record with no script. Picking one copies its `GameVariant::to_bytes()` over
the slot's `+0x24c..+0x37c`; the map id, weight and every byte before
the variant stay as they were. A variant the game's clean-up would
change (`variants::record::check`) is disabled with the reasons on hover,
and is refused if picked another way. **Edit** opens a copy of the
entry's variant in the variant tool, as a new unsaved variant there (the
guard asks first when that tool has unsaved edits); to use the edited
variant, save it there and pick it from **Saved**.

**Start from a preset**: the web builder's historical lineups, resolved
against the disc's maps and the entries already in the file (each new
entry copies a whole published entry of the same game type, preferring
the named variant, then the same team play). What does not resolve is
listed as unavailable.

Each change is one undo step (typing in a field or dragging a weight is
one), and undo walks back past a save.

The editing logic (`crates/tools/hopper-edit/src/model.rs`, driven through
`hopper_edit::HopperSession`) mirrors the server's `apply_patch`: every
entry remembers the slot its bytes come from, and writing back copies
that whole 0x37c-byte slot (game variant and unknown blobs) before
setting the edited fields, so reorders and copies lose nothing. It is
stricter about untouched bytes than the server: an unchanged playlist is
not rewritten at all, and in a changed one the title and description
(written to all nine language slots) are rewritten only if they changed.
An unedited file saves to the identical image. Only changed playlists are
validated, with the server's rules.

A picked game variant is kept as the whole 0x130-byte record beside the
entry's source slot; `model::slot_bytes` builds the slot (source bytes,
then the picked variant over the tail, then the editable fields) both
for the save and for what the table shows. The raw string, flags and
second dword of **Advanced: raw fields** lie inside the variant (at its
`+0x00`, `+0x48` and `+0x58`), so picking re-reads them from the new
record, and editing them afterwards edits the picked variant: the save
then checks the result with the clean-up rules again. Variants that came
with the file are written back as they are, never judged.

Map names come from the disc's main menu map (the globals' multiplayer
level list, as `h2 live assets extract` reads it for `catalog.json`).
The presets' playlist texts are the preset's own; the disc's
`matchmaking_hopper_descriptions` strings are not read yet.

To put a saved file on a Live server, run `h2 live hopper import <file>`
on the server's machine (docs/live/live-admin.md); the editor does not
publish by itself.
