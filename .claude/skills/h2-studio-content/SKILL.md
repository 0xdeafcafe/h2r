---
name: h2-studio-content
description: Use when building or changing game content (a model, a weapon such as the Pogo Stick from content/pogo, an object, a sound, a UI screen or a HUD piece) so it is seen before the user plays it. Covers Studio (h2-editor) as the content workbench, its renders (model.render, ui.render, hud.render), check.run, and pair.preview and pair.audio on a running game, and what a content lane's report attaches.
---

# Build content through Studio

Content written blind turns up broken in play: a weapon that turns the
screen black, a sound far too loud. Studio (`h2-editor`,
`crates/apps/studio`) is the workbench for models, weapons, objects,
sounds, UI screens and HUD pieces. Open the content in Studio, render
it, check it, then preview it in a running game through the pairing.
The method shapes are in the [editor API](../../../docs/editor/agent-api.md).
For the game's own API, see the [agent API skill](../h2-agent-api/SKILL.md).

Studio is a client of the game. It does not build an image or start a
game for you. Use the game the user or coordinator has running (`h2
agent games`). If no game is running, do the Studio steps, and say in the
report that the in-game preview did not run.

## Find Studio and check what it offers

```sh
h2 studio status
h2 studio session.methods
```

The window listens at `~/Library/Caches/h2recomp/studio.sock` (or
`$H2_STUDIO`). With no window, `h2-editor --serve --map <file.map>` runs
the same API headless, pairing included. Check `session.methods` before
you rely on a method: an entry with `"status": "planned"` answers
`NOT_YET`.

## Open, edit and render

Open the map that carries the content, then edit it through the API.
The window's own commands make the edits, so the user sees them land.
Tag fields go through `tag.set` and `tag.block.*`, models through the
Modeling tool's `geometry.*` and `mesh.*`, textures through `bitmap.*`,
UI screens through `ui.*`, and HUDs through `hud.*`.

```sh
h2 studio source ~/maps/ascension.map
h2 studio tags weap pogo
h2 studio tag.set '{"tag":"<tag>","path":"<field>","value":<value>}'
```

Render it before anything else. Look at every PNG you cite: a render you
have not opened proves nothing. The calls below show the usual params;
`session.methods` has each method's exact shape and an example.

```sh
h2 studio model.render '{"tag":"<hlmt or weap>","out":"/tmp/pogo-model.png"}'
h2 studio ui.render '{"out":"/tmp/screen.png"}'
h2 studio hud.render '{"out":"/tmp/hud.png"}'
```

## Check it

`check.run` runs the checks that apply to the open content (a model's
geometry and materials, a sound's level, a screen's layout and a HUD's
widgets). Each finding comes with its reason. Fix the findings or say in
the report why one stands. `sound.measure` gives a sound tag's own level
(peak and loudness) without a game.

```sh
h2 studio check.run '{}'
h2 studio sound.measure '{"tag":"<snd! tag>"}'
```

## Preview it in the running game

Pair Studio with the game first, then preview. Pending edits reach the
game through the pairing's own edit path before the preview runs.

```sh
h2 studio pair.list
h2 studio pair.connect '{"socket":"<socket from pair.list>"}'
h2 studio pair.status
```

`pair.preview` answers at once with a job id. Poll `pair.result` until
`state` is `done` or `failed`.

```sh
h2 studio pair.preview '{"what":"weapon","target":"objects\\weapons\\multiplayer\\pogo\\unsc\\pogo_unsc","out":"/tmp/pogo-game.png"}'
h2 studio pair.result '{"id":1}'
h2 studio pair.preview '{"what":"object","target":"<tag name>","distance":3}'
h2 studio pair.preview '{"what":"hud","target":"superbounce"}'
h2 studio pair.preview '{"what":"ui","target":"<screen name>"}'
```

Read the result, not only its state. `warnings` names what a player
would see as broken: a black frame, a weapon not held, no first-person
model drawn, a Pogo stick with no rider or a chase camera that is not
drawn, a spawned object that is gone, another screen shown, or edits
still waiting in Pending. Open `capture.path`. `capture.black` is true
when nearly every pixel is dark.

`pair.preview` reads a menu screen but does not open one. Drive the menu
with the game's `ui.press` (see the agent API skill), then preview `ui`.

For a sound in the game, make the sound and sample the mixer:

```sh
h2 studio pair.audio '{"seconds":3,"call":{"method":"pogo.test","params":{"trick":"backflip"}}}'
h2 studio pair.result
```

`audio.peak_db` and `audio.loudest` are mixer gains. A voice at 0 dB is
as loud as its sample. Read the sample's own level with `sound.measure`.

## Report

A content lane's report attaches the evidence:

- the render paths (`model.render`, `ui.render`, `hud.render`) and what
  each shows;
- the `check.run` output, with each finding fixed or explained;
- each `pair.preview` and `pair.audio` result: the job id, `capture.path`,
  the reads that matter (`held`, `first_person.drawn`, `pogo.camera`,
  `audio.peak_db`) and every warning;
- or, where no game was running, that the in-game preview did not run.

If Studio cannot render, check or preview the content, extend its API
(see [AGENTS.md](../../../AGENTS.md)). Do not route around it.
