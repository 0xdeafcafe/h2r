# The editor's agent API: images, checks and voices

Status: Built. Code: `crates/apps/studio/src/api`, `crates/apps/studio-proto/src`. Tests: in-source unit tests.
Switch: `H2_STUDIO` (the socket path).

Image generation, content checks, the announcer and voice methods. The index is [agent-api.md](agent-api.md).

## Generating images

`image.generate` makes a picture for an image of a bitmap from a prompt
and reference pictures, and the `image.*` calls after it put the
picture into the image through the Textures tool's own edits
(`crates/apps/studio/src/api/image_gen.rs`, `src/app/image_gen.rs`,
`src/imagegen.rs`). The window's Generate button does the same
([editor-guide.md](editor-guide.md#generating-an-image)). A generation
takes a minute or more, so it runs as a job: `image.generate` answers at
once with `{id, state: "running", generator, slot}` (`slot` is
`{bitmap, image}` or null) and `image.result {id?}`
reads the job (the newest without `id`). One job runs at a time and the
last sixteen are kept.

`image.generate`'s params:

- `prompt`: what the picture is.
- `references`: a list, empty for none. Each is a PNG path, or
  `{bitmap, image?}`, an image of the open map's bitmap, exported as a
  PNG into the job's directory for the generator. The references are
  the user's choice: an agent passes the ones the user named (for a menu
  icon, the other icons of the same bitmap), and never picks disc images
  of its own accord.
- `bitmap` and `image` (default 0): the slot, named as `bitmap.*` names
  a bitmap. The picture is fitted to the slot. `image: "new"` (which
  needs `bitmap`) first adds an image to the bitmap (`bitmap.image.add`),
  shaped like image `like` (default 0) or made at `size`, and makes it
  the slot; the answer's `slot` names it. Without a slot, `size
  [w, h]` fits it to that size as 32-bit RGBA; with neither, it is kept
  as it came. A slot and a size together are `INVALID_PARAMS` (a `new`
  slot is the one case that takes both).
- `alpha`: `auto` (the default: the picture's own alpha, except that an
  opaque picture going into an `A8` or `AY8` image takes its
  brightness), `image`, `luma` or `opaque`.
- `out`: a path to copy the fitted PNG to.
- `from`: the job this one regenerates, with the same or a tweaked
  prompt. The result and the provenance name it.

The job gives the source the prompt with the slot's needs added
(`imagegen::compose_prompt`): its size, a transparent background when the
format keeps alpha, and how the references are meant (match their style,
palette, line weight and framing, not their subject). It keeps the
source's picture as `generated.png`, then fits it (`imagegen::fit`). The
picture is resampled with the Textures tool's filter: stretched when the
aspects are within a tenth, else centred on transparent pixels. Then
each pixel is rounded to what the slot's format stores and its decoder
gives back. The uncompressed formats round as `assets::encode` stores
them and `gfx::convert` reads them. The DXT formats encode each 4 by 4
block with `assets::encode::encode_block` and decode it with `gfx::dxt`.
The fitted picture, `fitted.png`, is what the game shows. A slot of a
format that is not a picture (`P8`, bump, float) is `REFUSED`, and so is
a cube map or a volume. A slot with a `bitmap.reshape` asked for is
fitted to the reshape's format.

`image.result` gives `state` (`running`, `done` or `failed`),
`generator`, `elapsed_ms`, `prompt`, `references` (each with its `path`
and `origin`, `file` or `map` with its `bitmap` and `image`), `slot`,
`size`, `dir` (the job's directory, in the cache's `studio/imagegen/`),
`from`, and `applied`, `accepted` (the files written, or null) and
`discarded`. A done job adds `raw {path, width, height}`, `fitted {path,
width, height, format, alpha}`, `revised_prompt` and `out`. A failed one
adds `why`, the source's message, and `error`:

| `error` | Meaning |
|---|---|
| `missing` | the generator is not installed (`codex` not on `PATH`); `why` says how to install it and sign in |
| `unauthenticated` | Codex is not signed in; `why` says to run `codex login` and sign in with ChatGPT |
| `quota` | the ChatGPT account's allowance is used up; `why` has Codex's message, with when it renews |
| `no_image` | the run finished without a picture |
| `failed` | anything else, with the generator's message |

Then:

- `image.apply {id}` writes the fitted picture into its slot as one
  `BitmapAction::Import`, one undo step, after keeping the slot's pixels
  as `original.png` in the job's directory. The picture then shows
  wherever the image is used: the Textures canvas, `bitmap.pixels`, the
  UI Screens tool's pictures and `ui.render` (the disc's picture of the
  bitmap, `UiScreens::show_image`), and a paired game through the
  Textures tool's live sync ([realtime-edits.md](realtime-edits.md)).
  `pair.preview {what: "ui"}` captures the menu with it. A job without a
  slot, or one that failed, is `REFUSED`.
- `image.accept {id, save?}` keeps it, applying it first if it is not in
  its slot. The picture stays as the bitmap's edit and saves with the
  map (`doc.save {doc: "bitmaps"}`), like any texture edit. It writes `provenance.json` in
  the job's directory and logs a line naming the bitmap, the generator,
  the references and the prompt. With `save` (a directory) it copies
  `<bitmap>-<image>.png` (the generator's picture),
  `<bitmap>-<image>.fitted.png` and `<bitmap>-<image>.provenance.json`
  there. A job made to a `size` has no slot: accepting it keeps the art
  and its provenance (`image-<id>.png`, `image-<id>.fitted.png`,
  `image-<id>.provenance.json` under `save`) for an image made later,
  and changes no bitmap.
- `image.discard {id}` takes it out: the import's undo step while it is
  still the newest step of that bitmap, else the original pixels
  imported back.

The provenance names the prompt, the prompt sent, the generator's
rewrite of it, every reference with its origin, the generator (`codex-cli
0.170.0 (ChatGPT image generation)`), the slot, the alpha, the job it
regenerates, and whether it was accepted. Generated art is original
AI-made art and may go in the repository with its provenance; every PNG
there is stored in LFS.

**The source.** `imagegen::ImageSource` makes the picture.
`H2_IMAGEGEN` picks it: `codex` (the default) or `fake`. An OpenAI API key
source, with the key in the Keychain under the service `openai`, is
another implementation of the trait; it is not built.

- **Codex** (`imagegen::codex::CodexCli`) uses ChatGPT's image generation
  on the user's ChatGPT sign-in to the Codex CLI, with no API key. It
  first runs `codex --version` and `codex login status`. Then it runs
  `codex exec --image <ref>... --json --skip-git-repo-check --sandbox
  workspace-write --cd <job dir> --output-last-message <job
  dir>/codex-last-message.txt` once, with the prompt on stdin (`--image`
  takes many values, so a prompt argument after it would be read as an
  image). The prompt asks Codex to make one image with its image
  generation tool and copy it to `out.png` in the job's directory. The
  picture is `out.png`, else a `saved_path` in the run's events, else
  the newest PNG in `$CODEX_HOME/generated_images/<thread id>/` (Codex
  saves each picture there, the thread id from the `thread.started`
  event), else a PNG path in the last message. The events are kept as
  `codex-events.jsonl` in the job's directory. A run may take up to ten
  minutes. Studio never reads Codex's credentials.
- **The fake** (`imagegen::fake::FakeSource`) draws a disc on a
  transparent ground, its colour from the prompt. The tests use it, and
  `H2_IMAGEGEN=fake` tries the flow with no generator.

Into an image the map has (here `mp_games2`'s last icon), then into no
slot, at the icons' size, for an image made later, then into an image
the bitmap did not have, added first:

```sh
h2 studio image.generate '{"prompt":"a radar dish","references":[{"bitmap":"mp_games2","image":0},{"bitmap":"mp_games2","image":3}],"bitmap":"mp_games2","image":10}'
h2 studio image.result '{"id":1}'
h2 studio image.apply '{"id":1}'
h2 studio pair.preview '{"what":"ui"}'
h2 studio image.accept '{"id":1,"save":"art/ui-icons"}'
h2 studio doc.save '{"doc":"bitmaps"}'
h2 studio image.generate '{"prompt":"a radar dish","references":[{"bitmap":"mp_games2","image":0}],"size":[40,40]}'
h2 studio image.accept '{"id":2,"save":"art/ui-icons"}'
h2 studio image.generate '{"prompt":"a radar dish","references":[{"bitmap":"mp_games2","image":0},{"bitmap":"mp_games2","image":3}],"bitmap":"mp_games2","image":"new"}'
h2 studio image.apply '{"id":3}'
h2 studio doc.save '{"doc":"bitmaps"}'
```


## Content checks

An agent building content (the Pogo Stick's sounds, a screen of ours, a
HUD kit element) measures and checks it here before handing it back:
`sound.measure`, `ui.check`, `hud.check`, and `check.run` for all of
them at once.

**`sound.measure`** (`api/sound_measure.rs`) takes one of `tag` (a
`snd!` of the map the Tags tool shows, decoded by the audition as
`sound.read` decodes it), `clip` (one of our WAV clips as the game
embeds them: `pogo/compress`, `pogo/spring`, `pogo/settle` from
`boot::pogo_sound::CLIPS`, `announcer/<medal>` from
`boot::medal_announcer::CLIPS`) or `path` (a WAV file, such as one
`scripts/pogo-sounds.py` just wrote). WAVs are read by the game's
`parse_wav`: 16-bit PCM, one or two channels; a Git LFS pointer is
`FAILED` and names `git lfs pull`. A tag is measured in every
permutation of `range` (default 0), or the one `permutation` names;
the loudest is the one compared. It answers:

- `levels`: `peak_dbfs` (the largest sample, full scale 32768),
  `rms_dbfs` (every sample of every channel), `lufs` (integrated
  loudness), `max_momentary_lufs` (the loudest 400 ms block),
  `duration_ms`, `clipped` (samples at 32767 or -32768), `frames`,
  `rate`, `channels`. Levels are to one decimal place; silence is -120.
- `reference`: the open map's `snd!` tags of the same class (the
  header's `+2` byte, unnamed: [sound.md](../formats/sound.md)), or of
  every class for a clip or file, or of `class`. Up to `limit` (default
  32, at most 512) of them, spread evenly over the map's tag order, are
  decoded (range 0, permutation 0) and measured. It gives the class,
  how many there are (`considered`), `decoded`, `skipped` (a codec
  stream without the game's executable), the `lufs` and `peak_dbfs`
  spreads (`p10`, `p50`, `p90`, `max`, nearest rank) and the three
  loudest. With no map open it gives `why`.
- `verdict`: `loud` when the loudness is over the reference's p90 by
  more than 1 dB, `quiet` when under its p10 by more than 6 dB, else
  `ok`; `no_reference` with nothing decoded. `over_p90_db` and
  `peak_over_p90_db` are the margins, and `text` says it ("louder than
  the p90 of the map's sounds by 7.2 dB").

The loudness is ITU-R BS.1770-4's (`crates/platform/audio/src/meter.rs`,
shared with the game's `audio.meter`): each
channel K-weighted (the high shelf and high pass, their coefficients
derived for the clip's rate from the 48 kHz design, as libebur128 does),
mean square over 400 ms blocks stepped 100 ms, blocks under -70 LUFS
dropped, then blocks more than 10 LU under the remaining mean,
`-0.691 + 10 log10` of the mean of the rest. Each channel weighs 1, so
a mono 997 Hz sine reads its RMS level and a stereo one 3 dB more. A
clip shorter than 400 ms is one block. Clips and tags are compared
sample for sample: the game's gain, distance and mixing are not
applied.

**`ui.check`** (`api/ui_check.rs`) checks our screens (`screen`, and
`state`, narrow it) in every state with the unsaved edits
(`UiScreens::check_state`: the edited docs in scope, the disc's art
and fonts when a disc is open, else the synthetic source). The checks
are the kit's (`boot::ui_kit::findings::of_state`;
[ui-builder.md](../game/ui-builder.md) "The checks"): text overflowing
its rect (`fit`), overlapping texts, widgets outside the title-safe
area (`unsafe`), contrast under 2.5, missing glyphs and unregistered
string ids, art drawn past its rect or off screen, and the design
system's rules. Each finding has `code` `ui.<check>.<kind>`
(`ui.layout.unsafe`, `ui.layout.fit`, `ui.layout.text_overlap`,
`ui.content.missing_glyph`), `severity` (`error`; `warning` for the
design system's `consistency`; `note` for a layout note), `message`,
`target` (`screen/state`), `path` (the screen's file under
`content/ui/screens/`), and the `screen`, `state`, `check`, `kind` and
widget `ids`. A state that does not draw is `ui.draw`. `checked` lists
each state with its count and the checks the source could not run
(`skipped`). `aspect` is `4:3`, `16:9` or `both` (the default); the
host fits a menu uniformly into a centred 4:3 area at any window
aspect ([high-resolution.md](../graphics/high-resolution.md), "HUD and
UI"), so the layout checked is the 640 by 480 one and `aspects` says
which fit each aspect asked is.

**`hud.check`** gives each finding's `code` beside its text:
`off_screen`, `outside_frame`, `art_overlap`, `text_overlap` or
`text_spills` (`hud_edit::checks`).

**`check.run {what?, target?, asset?}`** (`api/checks.rs`) runs every check of
`CHECKS` (or one area's: `models`, `sounds`, `ui`, `hud`, `markers`) and answers
`{findings, counts, ran, skipped}`. A finding is `{area, code,
severity, message, target, path?}`, worst first; `counts` has `error`,
`warning`, `note`, `total` and `by_area`; `ran` names each check with
its count; `skipped` each check that could not run and why, and each
area with no check registered. `target` narrows every check to one
thing; a check it does not fit is skipped. With `asset` (an asset id)
the Problems list is read again and the answer is
`{asset, findings, counts, ran, skipped}` for only the rows about that
asset (the rows of `problems.list {asset}`, with `source`, `asset` and the
rest); `what` and `target` are not used. For a package item, both calls
refresh package checks and open-script diagnostics, which are the sources
that can report a finding on an item. The checks:

| Area | Check | What it checks |
|---|---|---|
| `models` | `model.check` | every biped, vehicle and weapon of the scenario tool's map (or the source's only map), or the tag `target` names, read without the Pogo kit: `model.<code>` for each `model.check` finding (`model.no_first_person`, ...), the tag as `target` and the `tag.set` path in the message; `model.tag_memory` once (`api/models.rs`, `check`) |
| `sounds` | `sound.measure` | each of our clips (or the clip id, id prefix such as `pogo`, or `snd!` tag `target` names) against the open map's sounds: `sound.loud` (a warning, an error past 6 dB over the p90), `sound.clipping`, `sound.unreadable`, `sound.no_reference` (a note) |
| `ui` | `ui.check` | every state of our screens, or `target` (`screen` or `screen/state`) |
| `hud` | `hud.check` | the HUD the HUD tool shows, at its state (`hud.<code>`); skipped with no map open there |
| `hud` | `hud.kit` | every kit HUD (or `target`) in every state and screen mode over the kit's synthetic map, as `h2 hud sheet --synthetic` checks them (`hud_kit::check::all`): `hud.kit.<check>` errors, and a note for each check the synthetic map cannot run |
| `markers` | `markers.check` | the open map's markers (`api/markers_check.rs`): `markers.area.invalid` (error), `markers.hill.corners_and_area` (error), `markers.area.no_floor` and `markers.area.overlap` (warnings, the second naming the pair); no map is no findings |

A check is a row of `CHECKS`: `ContentCheck { area, name, run }`,
`run` being `fn(&mut App, Option<&str>) -> Result<Vec<Finding>,
String>`.

## Announcer

`voice.say`, `voice.list` and `voice.commit` (`api/voice.rs`, over
`app/announcer.rs`) make announcer lines from text with a Fish Audio voice.
The design, the key and the steps are in [announcer.md](announcer.md).

**`voice.say {text, voice?, model?, speed?, out?, clip?, play?,
target_lufs?, fresh?}`** says `text` (at most 500 characters) in `voice`
(a Fish Audio model id; default `86126c4c4dad4911979556b4569802cd`), trims
the silence, measures the line against the open map's retail announcer
lines and brings it to their median loudness (`target_lufs` instead), saves
44.1 kHz mono 16-bit PCM under `<data root>/announcer`, and plays it
(`play` defaults to true). The same text, voice, `model` and `speed` reuse
the earlier file: `cached` is true and the service is not called, so no key
is needed. `fresh` asks again. It answers `file`, `raw` (the voice's own
file in the cache), `levels` and `raw_levels` (as `sound.measure`),
`trimmed_ms`, `gain_db`, `limited` (the -1 dBFS peak ceiling held the gain
back), `target` (`lufs`, `from`: `given`, `retail`, `our clips` or
`fallback`, and `why` when not retail), `verdict` (as `sound.measure`),
`playback`, and with `clip` a `ready` object: the checkout path
`art/announcer/<clip>.wav`, the `CLIPS` entry to add, and whether the clip
exists. With no key in the Keychain the error is `REFUSED`, its message
says to add the key with the exact command, and its `data` is
`{step: "add_key", service, command}`. A bad param is `INVALID_PARAMS`
before the service is asked; the service's errors and a silent line are
`FAILED`.

**`voice.list`** answers the voice source, the default voice, the voices
kept lines used, whether the key item exists (`key.state`: `present`,
`missing` with the command, or `unreadable`; the key is not read), the
cache's directory and size, and the lines most recently said. It asks
nothing of the service.

**`voice.commit {tag, package?, class?, from?}`** adds a saved line
(`from`, else the last one made, by `voice.say` or `voice.clone.say`) to
`package` (default the edit target; `NO_TARGET` with none) as a `sound` item
that writes the `snd!` `tag`. The WAV is copied in unchanged and the
package's build encodes it, the way `import.sound` does (its reply, with
`class` as there). A `from` inside a character's `dataset` or `model`
folder is `REFUSED`, and only a `.wav` is taken, so a dataset clip or a
weight file never reaches a package.

**`voice.commit {clip, from?, repo?, replace?}`** saves a standalone
`sound` item in the edit target. An explicit `repo` exports the WAV to
`art/announcer/<clip>.wav` in that checkout. Use `replace` to overwrite
an exported clip. The export's `next` field gives the steps for
`boot::medal_announcer::CLIPS` and Git. `clip`, `repo` and `replace`
are refused with `tag`.

## Voices

`voice.characters`, `voice.dataset.build`, `voice.dataset.status`,
`voice.clone.train`, `voice.clone.say` and the batch (`voice.say_batch`,
`voice.batch.status`, `voice.batch.run`, `voice.batch.save`) (`api/voice_clone.rs`, over
`app/voices.rs` and `app/announcer.rs`) clone a game character's voice from
the user's own disc and say new lines with it, all on this machine. The
design, the dataset, the sidecar and the models are in
[voice-cloning.md](voice-cloning.md).

**`voice.characters {map?}`** lists the characters Studio knows: `id`,
`name`, the tag-path `rules`, how many dialogue sounds of the source's maps
(or `map`) they pick (`tags`), their `dataset` summary (or null) and
whether they are `trained`. `speakers` lists the groups the maps' dialogue
tags fall into (`kind: code` for the letters at the end of a tag name,
`folder` for the first folders under `sound\dialog\`) with a count and a
sample tag, to check the rules against. Nothing is decoded.

**`voice.dataset.build {character, paths?, maps?, asr?, min_ms?, max_ms?,
keep_radio?, limit?, max_permutations?}`** builds the character's dataset
under `<data root>/voices/<character>`: 24 kHz mono clips and
`manifest.jsonl`. `paths` (pieces of tag paths) replaces the character's
rules and allows a new id. It answers `kept`, `tags`, `decoded`, `dropped`
by reason (`too_short`, `too_long`, `silent`, `clipped`, `radio`,
`decode`) with a few `dropped_examples`, `maps_read`, `maps_failed`, the
`dataset` summary (`lines`, `usable`, `minutes`, `from_subtitle`,
`from_asr`, `pending_asr`, `no_text`, `lufs`, `peak_dbfs`), and `asr_job`:
the background transcription started for clips without a subtitle (`asr`
defaults to true). An unknown character without `paths` is
`INVALID_PARAMS`; no map read, or a job still running, is `REFUSED`.

**`voice.dataset.status {character?, job?}`** answers the `sidecar`
(`backend`, `state`: `installed`, `installing` or `missing`, `dir`,
`trains`), the latest `job` (or `job`'s) and the last few `jobs` (`id`,
`kind`: `install`, `transcribe` or `train`, `state`: `running`, `done` or
`failed`, `stage`, `done`, `total`, `text`, `log`, `result` or `why`), and
each built character's `dataset`, last `build` and `model` (`trained`,
`checkpoint`).

**`voice.clone.train {character, steps?, min_minutes?, backend?}`** starts
the fine-tune on the character's lines with text as a background job
(installing the model first when needed) and answers `{job, state:
"running", lines, minutes}`. It is `REFUSED` under `min_minutes` of speech
(default 10), without a dataset, or while a job runs, and
`INVALID_PARAMS` for a backend that does not train.

**`voice.clone.say {character, text, reference?, speed?, out?, clip?,
play?, target_lufs?, fresh?, backend?}`** says `text` with the clone,
copying the delivery of `reference` (a dataset clip id, or a source tag
with an optional `#<permutation>`; default a subtitled line near 7 s), and
answers as `voice.say` does plus `engine: "clone"`, `character`,
`backend`, `model` (the backend, and the fine-tune's digest when there is
one) and the `reference` used (null for a cached line). `target.from` is
`character`: the median loudness of the character's own lines. Before the
model is installed it starts the install and is `REFUSED` with
`data.step: "install"`; without a dataset, or with no line that has text,
`data.step` is `dataset`. A model failure is `FAILED` with the sidecar's
message. To hear a committed clip in the game, `voice.commit` it and use
`pair.audio` as for any announcer clip.

**`voice.say_batch {character?, rows?, lines?, file?, append?, wait?,
backend?}`** says many lines as the Voices window's **Say all** does, each
row as `voice.clone.say` says a line (cached, levelled, saved under
`<data root>/voices/<character>`, never played). `rows` is a list of
`{text, character?, clip?, tag?}` (`character` defaults to the batch's;
`clip` is the output name; `tag` a `snd!` path for the edit target
package), `lines` one row per line of a text (a tab splits an output name
from the text) and `file` a text file of at most 1 MB. The previous batch
is replaced unless `append`; while rows are queued or running it is
`REFUSED`. A bad param adds nothing. A row with a problem fails at once with
`why`; the others are queued. With `wait` the call returns when every row
has ended; without it the window's frames say one row each, so poll
`voice.batch.status`. It answers `added`, `queued`, `counts` (`idle`,
`queued`, `running`, `done`, `failed`), `active` and `rows`.

**`voice.batch.status {ids?}`** answers the same without asking the model:
each row's `id`, `text`, `character`, `clip`, `tag`, `state` (`idle`,
`queued`, `running`, `done`, `failed` with `why`), and once said its `file`,
`levels` (`lufs`, `peak_dbfs`, `duration_ms`), `verdict` and `cached`;
`saved` or `save_failed` after a save.

**`voice.batch.run {ids?, wait?, backend?}`** says the idle and failed rows
again, or `ids` (a done row too, asking the model again, not the cache).

**`voice.batch.save {ids?, repo?, package?, replace?}`** saves done rows
as `sound` items in the edit target or an explicit `package`. A row's
`tag` or `clip` identifies the item to add or update. A row with neither
fails with a reason. With neither a target nor a destination the call
answers `NO_TARGET`. An explicit `repo` exports untagged rows to
`art/announcer/<clip>.wav`, overwriting an exported clip with `replace`.
The reply contains `saved`, `failed` and each row's result.
