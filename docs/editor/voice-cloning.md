# Voice cloning: new lines in a game character's voice

Status: Partly built. The dataset builder, the jobs, the provider and the API are built and tested on invented maps and a fake sidecar; the Python sidecar, the tag-path rules and the subtitle pairing are not yet run against a disc.
Code: `crates/apps/studio/src/voice/dataset.rs` (`build`), `voice/clone.rs` (`CloneVoice`), `voice/sidecar.rs` (`Managed`, `Process`), `crates/apps/studio/sidecar/voice_clone.py`, `app/voices.rs` (`AppLines`, the jobs), `app/announcer.rs` (`App::voice_say` with `Engine::Clone`), `api/voice_clone.rs`, `ui/announcer.rs` (the Voices window). Tests: `voice/dataset/tests.rs`, `voice/tests.rs`, `api/voice_clone/tests.rs`.
Switch: none.

Studio makes new dialogue in the voice of a game character (Master Chief,
the Arbiter, Cortana, Sergeant Johnson, the multiplayer announcer and the
rest) so mod makers can add lines in the original voices. The clone learns
from that character's own lines on the user's disc, paired with their text.
The rule that must hold: disc audio, the datasets built from it and the
model weights stay on the user's machine. Nothing is uploaded, nothing is
committed, and the model runs locally with the network off.

## How it works

1. **The dataset** (`voice.dataset.build`). `dataset::build` reads every map
   of the open source through `AppLines`, which browses each map in the
   Tags tool and decodes its sounds with the Sound tool's decoder
   (`App::sound_tags`, `sound_info`, `sound_clip`; ADPCM and the codec
   streams, [rich-editors.md](rich-editors.md)). It keeps the `snd!` tags
   under `sound\dialog\` that the character's rules pick, each tag once
   across maps, and every permutation of the first pitch range.
2. **Cleaning.** Each permutation is made mono, trimmed of silence
   (-50 dBFS, 40 ms kept at the start and 80 ms at the end), and dropped
   when it is shorter than 1 s or longer than 15 s, silent, has more than
   0.1% of its samples at full scale, or is radio-filtered. A clip is
   radio-filtered when less than 1% of its energy is under 250 Hz and less
   than 0.4% is over 4 kHz (`dataset::analyse`, `Filter::radio`), or when a
   folder of its path has `radio` in it. The class byte is kept in the
   manifest but not used: its values are not interpreted yet
   ([sound.md](../formats/sound.md)). The kept clip is low-passed and
   resampled to 24 kHz mono 16-bit (`dataset::resample`), the rate F5-TTS
   takes.
3. **Text.** A sound with one permutation takes the subtitle whose string
   id name is the tag's last path part (`dataset::subtitles_of`, every
   `unic` tag's English entries, cleaned by `clean_text`: stage directions
   in brackets and a leading `Speaker:` taken off). The rest (combat
   chatter, the announcer, sounds with several takes) are marked `pending`
   and transcribed by the local Whisper in a background job; a clip it
   hears nothing in is `none` and never used.
4. **The manifest.** `<data root>/voices/<character>/manifest.jsonl`, one
   line per clip: `id`, `path`, `text`, `text_from` (`subtitle`, `asr`,
   `pending`, `none`), `duration_ms`, `source_tag`, `map`, `permutation`,
   `class`, the source's `lufs` and `peak_dbfs` (trimmed, before
   resampling), and the two band shares. `dataset.json` beside it records
   the rules, the filter and what was dropped and why. A rebuild replaces
   the clips and keeps the Whisper's transcripts of clips with the same id.
5. **The model** runs in a Python sidecar Studio installs on first use
   (below). `voice.clone.say` picks a reference line from the dataset (the
   one named, else a subtitled line of 3 to 10 s nearest 7 s), and the
   model says the new text in that line's voice and delivery, with the
   character's fine-tuned weights when there are some.
6. **The line** then goes the way a Fish Audio line goes
   ([announcer.md](announcer.md), "A line, step by step"): cached by
   request (the backend and the fine-tune's digest are in the key), brought
   to 44.1 kHz, trimmed, brought with one gain to the median loudness of the
   character's own lines (`App::character_target`, the dataset's `lufs`,
   measured by `sound.measure`'s meter), saved under
   `<data root>/voices/<character>/lines` (or `clips/<clip>.wav`), and
   played. `voice.commit` copies a clip into the checkout as it does a Fish
   line.

### Naming a character

`dataset::CHARACTERS` gives each character an id, a name and three kinds of
rule, matched against the lowercase tag path: path prefixes (the announcer:
`sound\dialog\multiplayer\`), folder names under `sound\dialog\`, and
speaker codes, the letters after the last `_` of the tag's last part (a
trailing number allowed). The folders and codes in the table are our guess
at the disc's naming (**S**). `voice.characters` lists the codes and folders
the maps really have, with counts and a sample tag, so the table can be put
right from the first run; until then `voice.dataset.build` takes `paths`
(pieces of tag paths) and a new character id for any speaker.

### Pairing a line with its text

The subtitle a cutscene or mission line shows is believed to be a `unic`
entry whose string id name is the sound's tag name (**S**). A stronger link
is the scenario's scripts, which start a sound and show a subtitle next to
each other; that is not read here. Lines with several takes do not take a
subtitle, since one name captions them all; they go to the Whisper.

### The sidecar

`voice::sidecar` writes the built-in script and the backend's
requirements into `<caches>/voice-clone/<backend>/`, makes a venv there
(uv's Python 3.12 when `uv` is installed, else `python3 -m venv`), installs
the requirements, and runs the script's `prepare` op, which downloads the
model's and the Whisper's weights into `<caches>/voice-clone/hf`
(`HF_HOME`). `installed.json` holds a digest of the script and the
requirements; a Studio with another script installs again. Every other run
sets `HF_HUB_OFFLINE=1` and `python -I`. The protocol is JSON lines on
stdin and stdout (`voice/sidecar.rs` header); stderr is
`sidecar.log`. The install, the transcription and the fine-tune run as
background jobs, one at a time (`app::voices::Voices::start`);
`voice.dataset.status` and the Voices window show their stage, progress
and last log lines.

### The models

| Backend | What | Runs on | Clone | Fine-tune | Licence |
|---|---|---|---|---|---|
| `f5-mlx` (default) | F5-TTS through `f5-tts-mlx` | MLX, Apple Silicon | from a 24 kHz reference of about 5 to 10 s | yes, MLX | code MIT; weights as F5-TTS's base, CC-BY-NC-4.0 (**S**: check the model card) |
| `qwen3-tts` | Qwen3-TTS 1.7B Base through `qwen-tts` | PyTorch, MPS (**S**) | from a 3 s or longer reference | not here | Apache-2.0 |

**Design.** F5-TTS is the default because its MLX port runs natively on a
Mac, clones well from one short clean reference, and has an MLX training
path (`f5_tts_mlx.trainer`), so the fine-tune on 20 to 60 minutes of one
speaker stays on the Mac. Qwen3-TTS has open weights under a looser
licence and a voice-clone API (`Qwen3TTSModel.generate_voice_clone`), but
its examples are CUDA-first and its fine-tune is PyTorch; it is the
switchable second backend (`backend` on `voice.clone.say`). GPT-SoVITS
clones well from minutes of speech but is a PyTorch web-UI project with
its own training flow; it is not wired in.

### The fine-tune

`voice.clone.train` writes the lines with text to `model/train.jsonl` and
starts the sidecar's `train`: the pretrained model, AdamW at 1e-5, the
flow-matching loss of `model(mel_spec, text=, lens=)` as
`f5_tts_mlx.trainer` computes it, gradients clipped at 1, one clip a step
(default `max(2000, 4 × lines)` steps). The trainable parameters are saved
to `model/model.safetensors` and recorded in `model/trained.json`; `say`
loads them over the base model. It needs 10 minutes of speech with text
unless `min_minutes` says otherwise.

## Interfaces

- Agent API: `voice.characters`, `voice.dataset.build`,
  `voice.dataset.status`, `voice.clone.train`, `voice.clone.say`
  ([agent-api.md](agent-api.md), "Voices").
- The Voices window (View > Voices): the engine (the announcer's Fish
  Audio voice, or a character's clone), the character, Build dataset,
  Fine-tune, the job's progress, the reference line, and the line view
  shared with the announcer.
- `voice::sidecar::Launcher` and `Sidecar` are the seam the tests replace
  (`voice::fake::FakeLauncher`); `voice::dataset::LineSource` the one the
  dataset builder reads through (`voice::fake::FakeLines`).

## Rules and limits

- Everything is written under `<data root>/voices/` or the caches dir,
  never in a checkout. `voice.commit` is the only step that copies a line
  into one, and only a line the user asked for.
- No audio, transcript or weight is sent anywhere. The only network the
  clone uses is the install's package and weight downloads.
- One background job at a time; a build or a train while one runs is
  `REFUSED`. A line asked for before the model is installed starts the
  install and is `REFUSED` with `step: install`.
- Sounds whose decode needs the game's executable need it here too
  ([rich-editors.md](rich-editors.md)).
- The window reads the character's manifest every frame it is open.

## Evidence

- The filters, the resampler, the manifest, the reference pick and the
  build over invented maps: **Test** `voice/dataset/tests.rs`.
- The jobs, the install on first use, the clone's line and loudness, the
  fine-tune record: **Test** `api/voice_clone/tests.rs` (fake sidecar).
- The sidecar protocol and the install stamp: **Test** `voice/tests.rs`
  (`the_sidecar_protocol_reads_progress_and_the_answer`,
  `the_sidecar_is_installed_only_by_a_matching_stamp`).
- The `f5-tts-mlx` and `qwen-tts` calls follow their repositories'
  examples and sources (**S** until run).

## Not verified

- The folders and speaker codes of `CHARACTERS` against a disc.
- That subtitle string ids are named as their sounds, and how many lines
  pair that way.
- The radio thresholds on real radio and intercom lines, and Cortana's
  effected voice.
- The sidecar's install, `say`, `transcribe` and `train` on a Mac, and how
  long each takes.
- Qwen3-TTS on MPS.
- How a clone line sounds against the character's own lines.

## Related

- [announcer.md](announcer.md): the line pipeline the clone shares.
- [agent-api.md](agent-api.md), "Voices".
- [tag-workspace.md](tag-workspace.md): the tags extracted from the disc.
- [sound.md](../formats/sound.md): the `snd!` header and its class byte.
