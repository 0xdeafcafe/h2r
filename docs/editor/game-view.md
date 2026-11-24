# The game view: the game's picture inside Studio

Status: Partly built. The frame feed (`render.picture`, GV1) and the Game panel in attached mode (GV2: the ring's picture of the paired game, `game.status`, `game.snapshot`) are built; editor mode, the own game, input and cameras are not, so Studio starts and pairs a separate game ([editor.md](editor.md#play-and-pair)) and the panel shows what that game shows. A game over `tcp://` has no picture in Studio yet: the JPEG stream is not decoded.
Code: built in `crates/extensions/agent/src/render/picture.rs` and `crates/extensions/agent-proto/src/picture.rs` (the frame feed, `render.picture`) and in Studio's `crates/apps/studio/src/play/picture.rs` (the ring reader), `src/ui/game_view.rs` (the panel's texture) and `src/api/game.rs` (`game.status`, `game.snapshot`); planned in Studio's `play` and `ui` modules (launch and input, slices GV3 to GV5) and `crates/apps/cli/src/play_cmd.rs` (editor mode). Tests: `agent::render::picture::tests`, `studio::play::picture::tests`, `studio::api::tests::game_tests`, `headless::tests::the_game_panel_shows_the_picture_and_its_stall`.
Switch: none.

Studio has a **Game** panel that shows a running game's own frames, live.
It has two modes. **Own**: Studio starts a game of its own in editor mode
on the map open in Studio, and the panel is where that game is seen and
played. **Attached**: Studio is paired with a game someone else started,
and the panel shows what that game shows. The game is always another
process; only its frames and Studio's input cross. The rule that must
hold: Studio's own game is the only game Studio's tools and agents ever
change the map of. An attached game is watched, and edited only under the
pairing's existing consent ([live-mirror.md](live-mirror.md)).

## How it works

### Frames

The game publishes its presented frame through a new host method group,
`render.picture`, fed from the frame source the window shell and
`host.screenshot` already read (`crates/apps/app/src/frame.rs`,
`FrameSource`), or from the headless GPU target on a game with no
window.

- **Local: a shared-memory ring.** `render.picture.open {width?, fps?}`
  answers with the path of a memory-mapped file under the cache
  directory, mode 0600: a header (size, format, the newest slot, a
  sequence number per slot) and three RGBA slots. The game reads each
  presented frame back, scaled to the asked size, into the slot after
  the newest and then publishes it. Studio maps the file, takes the
  newest slot whose sequence is even and unchanged across its copy, and
  uploads it to the panel's texture. No frame bytes travel over the
  socket; the socket carries `open`, `close` and a `picture` event per
  frame with its tick, present number and time.
- **Remote: frames on the socket.** A game reached over `tcp://` (the
  other Mac through the lab, the Xbox app) cannot share memory.
  `render.picture.open {transport: "stream"}` switches its connection
  to binary frames, as `live.subscribe` does, and sends each frame there
  as a JPEG, at the asked rate and size (default 15 a second at
  640 wide).
- **Why not a shared GPU surface.** An IOSurface shared between the two
  processes' Metal devices avoids the readback and the upload, but it
  needs raw Metal under `wgpu` on both sides and exists on one
  platform. The ring is measured first; the surface is a later change
  behind the same `open` if a 60 Hz view costs the game too much.
- **Cost.** A reader that stops reading costs the game nothing: the
  game skips the readback while the newest slot is unread for a second,
  and stops when the client closes or drops.

`render.capture` stays the way to take a still for a check: it is a
sealed scene at a tick, not whatever was last presented.

### Own mode: the game in editor mode

`h2 play --editor` and `h2 serve --editor` (one flag, read in
`play_cmd.rs` and `boot`) start a game for a tool to hold:

| Part | Editor mode |
|---|---|
| Start | the level boot straight into `--level`, a disc level or an installed package, with one default player; no attract film, no main menu, no sign-in ([level-boot.md](../runtime/level-boot.md)) |
| Window | none by default: the GPU view backend draws headless and the picture goes to the ring. `--window` gives it its own window as well |
| Session | a local game; Live, System Link, film and input recording are off, so every edit method is allowed |
| Edits | the pairing starts in Edit, as a game Studio launches does |
| Socket | the private socket Studio names (`H2_AGENT=<cache>/editor-<pid>-<n>.sock`), which other clients do not discover as the shared game |
| Pause | paused when Studio says so; never by a focus change of its own |
| Menus | the in-game pause menu is suppressed; Start goes to Studio |
| Code | the fast tier ([cranelift-tier.md](../lifter/cranelift-tier.md)), or clang's module when it is current. It never builds one |

`play::launch::Launch` gains the form. The map is the open project's last
build ([tag-workspace.md](tag-workspace.md), "Projects"): a package by its
id, or a play copy for a disc level, as Play does now.

**Cameras.** The panel's toolbar picks one:

- **Player**: the game's own first-person view, with the HUD.
- **Free**: a private world view flown from the panel
  (`render.view`, [content-stage.md](content-stage.md), S4). The player
  stays where it is.
- **Editor**: the free view driven by Studio's viewport camera every
  frame, so the viewport and the Game panel show the same eye, one
  drawn by Studio's renderer and one by the game's.

**Edits.** Every edit made in any Studio tool reaches the own game by
the route [live-editors.md](live-editors.md) gives its kind: fields at
the next tick, pixels and vertices at the next frame, and a structural
change by a build and a reload that Studio starts itself, restoring the
player's place (`play::playtest`, which does this for F5 today). In own
mode nothing waits in Pending for a click.

### Attached mode

The pairing is the existing one (`pair.*`, auto-pair, View by default).
When the paired game offers `render.picture.open`, the pairing opens its
picture (`Bridge.picture`, `play::picture::Picture`): a connection of its
own asks for the ring, and the panel maps it. The panel polls the ring once
a frame while it is drawn, takes the newest frame, uploads it to a texture
(`ui::game_view`) and draws it above the session controls, under the
game's map, phase and the link plate's state. Under it, the size, rate,
dropped frames and the frame's tick; a ring that has published nothing for
two seconds says how long, and keeps the last frame. A hidden panel does
not poll, and the game stops reading frames back a second after the newest
one went unread. A game over `tcp://` has no ring and the panel says why
(the stream's JPEG frames are not decoded yet). The picture ends with the
pairing and is opened again after the game closed it. The game keeps its
own window and its own input. Studio does not load maps into it; what F5
may do to a foreign game is unchanged ([editor.md](editor.md#play-and-pair)).

The panel has a switch between the two when both exist. Starting the own
game does not unpair the attached one: the mirror and the picture follow
the panel's mode.

### Input

Input goes to the own game only, and only while the panel holds it.

- A click in the panel captures the keyboard, the mouse and the first
  gamepad; the status line says so; Escape (or the guide button)
  releases. While captured, Studio's shortcuts are off.
- `play::input` turns the captured devices into the game's pad at the
  game's tick rate, with the same bindings as the game's window
  (`crates/apps/app`'s mapping, moved to a crate both use), and sends
  each state with `input.pad` ([agent-api.md](../runtime/agent-api.md#input-act-like-a-player)).
  Mouse movement is sent as look deltas, as the window sends them.
- In **Free** and **Editor** cameras the captured input flies the view
  and the player gets none.
- An attached game takes no input from Studio. A user who wants to
  drive it uses its window.

## Interfaces

The game ([agent-api.md](../runtime/agent-api.md)):

| Method | Access | Params | Result |
|---|---|---|---|
| `render.picture.open` | read | `{width?, fps?, transport?}` | `{id, transport, path?, size, format, fps}`; then `picture` events |
| `render.picture.close` | read | `{id}` | `{closed}` |

Studio ([agent-api.md](agent-api.md)):

| Method | Access | Does |
|---|---|---|
| `game.status` | read | the own game (`none`, `starting`, `running`, its map, tier and socket), the attached game, the panel's mode and camera, the picture's state, rate, age, drops and tick lag (built for the attached game; `own` is always `none`) |
| `game.start`, `game.stop` | act | start the own game on the open document, or stop it |
| `game.mode {own \| attached}` | act | which game the panel and the mirror follow |
| `game.camera {player \| free \| editor}` | act | the own game's camera |
| `game.snapshot {out}` | act | the panel's newest frame as a PNG, refused in a Git checkout (built for the attached game's picture) |

`pair.play` and F5 start the own game through `game.start`.

## Rules and limits

- One own game per Studio. A headless Studio (`h2-editor --serve`) has
  one too, with no panel: `stage.*` and `game.snapshot` use it.
- Studio's tools and agents load maps only into the own game. The
  shared `h2 serve` a user started is never taken over.
- Studio never builds a game image. With none current, `game.start`
  refuses with the command that builds one.
- The ring's file holds frames of the user's game and lives under the
  cache directory, never in a Git checkout.
- Editor mode is a start-up flag. A game not started with it is never
  switched into it.

## Evidence

- The window shell reads its newest published frame back for
  `host.screenshot` (**Code**, `crates/apps/app/src/shell.rs`), and the
  same handoff feeds `render.picture` (**Code**, `crates/apps/app/src/picture.rs`).
- The ring's torn-slot guard, the idle skip, the file's removal on close
  and on a dropped connection, and the stream's JPEG frames are tested
  over a synthetic frame source (**Code**,
  `crates/extensions/agent/src/render/picture/tests.rs`,
  `crates/extensions/agent/src/exec/picture.rs`); `h2 agent picture` is the
  client ([agent-api.md](../runtime/agent-api.md#picture-feed-renderpicture)).
- Studio starts `h2 play --level` with a private agent socket and a
  play copy, and restores the player's place after a reload (**Code**,
  `crates/apps/studio/src/play/launch.rs`, `play/playtest.rs`).
- `input.pad` drives a player on a headless or windowed game (**Code**,
  [agent-api.md](../runtime/agent-api.md#input-act-like-a-player)).
- The panel reads the ring, counts its frames and drops, and shows a
  stall, over an invented ring (**Code**,
  `crates/apps/studio/src/play/picture/tests.rs`,
  `crates/apps/studio/src/api/tests/game_tests.rs`, `headless.rs`).
- The readback's cost at 60 Hz, and the panel's latency, are not
  measured; [the note](notes/game-view.md) says how.

## Not verified

- That the level boot starts an installed multiplayer package by name.
- The game's frame time with the ring open at full rate.
- That a headless GPU game presents at the display rate with no window.
- Mouse look through `input.pad` at the window's feel.

## Build slices

| # | Slice | Owns | Tests | Accepted when |
|---|---|---|---|---|
| GV1 | `render.picture`: the ring, the stream, the events, the idle skip | `crates/extensions/agent/src/render/picture.rs`, `agent-proto/src/picture.rs`, boot's frame glue, [agent-api.md](../runtime/agent-api.md) | a synthetic frame source: a reader never sees a torn slot; an unread ring stops readbacks; close and drop free the file; the stream's frames decode | a client program shows a running game's picture |
| GV2 | The Game panel in attached mode: map the ring, upload, mode and status, `game.status`, `game.snapshot`, the measured cost | `studio/src/play/picture.rs`, `studio/src/ui/game_view.rs`, `studio/src/api/game.rs`, `docs/editor/notes/game-view.md` | a fake ring drives the panel in the headless window; a stalled ring shows its age | the panel shows the paired game; the note has the cost and latency |
| GV3 | Editor mode and the own game: the flag, the headless picture, the level boot of a package, `game.start` and `game.stop`, F5 through them | `crates/apps/cli/src/play_cmd.rs`, `crates/apps/boot` (the mode), `crates/extensions/game/src/level_boot`, `studio/src/play/launch.rs`, [switches.md](../runtime/switches.md), [editor.md](editor.md) | the launch line; the mode's refusals (Live, recording); a package level resolves | `game.start` on the stage shows it in the panel with no menu on the way |
| GV4 | Input: capture and release, the shared bindings, `input.pad` at tick rate | `studio/src/play/input.rs`, a bindings crate out of `crates/apps/app`, the panel | captured keys make the pad states the window makes; release on Escape, on focus loss and on a dropped link | the own game is played from the panel |
| GV5 | Cameras: free and editor-synced, on `render.view` | `studio/src/play/camera.rs`, the panel's toolbar, `game.camera` | the viewport's eye reaches the game each frame; switching to Player restores the HUD view | the viewport and the panel show the same eye |
| GV6 | Own-game edits without Pending: structural changes build and reload on their own | `studio/src/play/{sync.rs, playtest.rs}` | on the fake game: a structural edit queues one build and one reload, debounced, and restores the place | a placement added in Studio is in the panel seconds later with no click |

GV1 needs nothing. GV2 and GV3 need GV1. GV4 needs GV3. GV5 needs GV3
and the stage's S4. GV6 needs GV3 and the workspace's W5.

## Related

- [editor.md](editor.md#play-and-pair): starting and pairing a game
- [live-mirror.md](live-mirror.md): the pairing, View and Edit, the world stream
- [live-editors.md](live-editors.md): how each kind of edit reaches the game
- [content-stage.md](content-stage.md): the stage, which runs on the own game
- [tag-workspace.md](tag-workspace.md): the builds the own game loads
