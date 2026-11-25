# The Announcer: announcer lines from text, in Studio

Status: Built, unproven against the live service (the tests use a fake voice).
Code: `crates/apps/studio/src/voice.rs` (the `VoiceSource` trait, the request, the cache), `voice/fish.rs` (Fish Audio), `voice/key.rs` (the Keychain), `voice/audio.rs` (trim, normalise, WAV), `crates/apps/studio/src/app/announcer.rs` (the pipeline), `api/voice.rs` (`voice.*`), `ui/voices.rs` and `ui/voices/lines.rs` (the voice document). Tests: `voice/tests.rs`, `api/voice/tests.rs`.
Switch: none. The key is read from the Keychain; there is no environment variable.

Studio makes an announcer line from text: a voice says it, Studio trims
it, brings it to the loudness of the retail announcer, saves it in the
format of our announcer clips (`art/announcer/README.md`), plays it and
says how it sits against the retail lines. The announcer's voice document (the Browser's **Voices** kind, or Go to
anything; the announcer engine) and the agent API (`voice.say`, `voice.list`,
`voice.commit`, in [agent-api.md](agent-api.md)) go through the same `App`
methods. The same pipeline says a game character's lines with a local
clone (`Engine::Clone`, `voice.clone.say`); that engine, its dataset and
its model are in [voice-cloning.md](voice-cloning.md).

## The voice

The default voice is the Fish Audio model
[86126c4c4dad4911979556b4569802cd](https://fish.audio/m/86126c4c4dad4911979556b4569802cd/),
sent as `reference_id`. `voice` on a call picks another model id.

`voice::fish::Fish` asks `POST https://api.fish.audio/v1/tts` with a bearer
key and a JSON body (`text`, `reference_id`, `format: pcm`, `sample_rate:
44100`, `latency: normal`, `normalize: true`, and `prosody.speed` when the
speed is not 1). `pcm` is raw 16-bit mono, streamed in chunks, so there is
no WAV header to trust. The `model` header is sent only when the call names
a TTS model; the service's default stands otherwise. Fish Audio's docs
state no rate limits, so Studio does not retry: a 401 (no permission), 402
(no payment), 429 or 503 comes back as `FAILED` with the status and the
service's own message (cut to 200 characters, the key taken out).

Everything the service sees is behind `VoiceSource`: `say(&Request)` and
`ready()`. The tests use `voice::fake::Fake`, a synthetic tone between two
silences that counts its calls; in a test build `App` starts with the fake,
so no test can reach the service.

## The key

The key is a macOS Keychain item with service `fish-audio`. `Keychain`
runs `security find-generic-password -s fish-audio -w` when a line has to
be asked of the service, never for a cached one. The key is held in a
`Secret` whose `Debug` and `Display` print `[key]`, sent as the bearer
header and dropped. It is not logged, not in an error message and not
written to disk. `voice.list` checks that the item exists without reading
it.

With no item the call is refused (`REFUSED`) and the error says to add the
key. Its `data` is `{step: "add_key", service, command}`, and the Announcer
window shows the command. Make a key at https://fish.audio/app/api-keys/
and run once, in a terminal:

```
security add-generic-password -s fish-audio -a "$USER" -w '<your-fish-audio-key>'
```

Leaving the key off after `-w` makes `security` ask for it, which keeps it
out of the shell history. A Keychain that cannot be read (access denied, no
`security` on the host) is `REFUSED` with the reason.

## A line, step by step

1. **Ask.** The request is the text, voice, model and speed. Its SHA-256
   is the cache key. `<data root>/announcer/cache/<key>.wav` (44.1 kHz mono
   16-bit, as the voice said it) and `<key>.json` (what was asked) hold an
   earlier answer; a hit makes no call and needs no key. `fresh` asks again
   and replaces the entry. Only an answer that passes the trim and the
   normalise below is kept: a silent line is an error and is never served
   from the cache.
2. **Trim.** Samples under -50 dBFS are silence. 20 ms of it stays at the
   start and 60 ms at the end.
3. **Measure and normalise.** One gain brings the line's integrated
   loudness (BS.1770-4, `sound.measure`'s meter) to the target: the median
   of the open map's retail announcer lines (`sound\dialog\multiplayer\flavor`
   tags, up to 16 measured, at least 3), else the median of our own
   announcer clips, else -14.5 LUFS, the level they were mixed to.
   `target_lufs` overrides all three. The sample peak is held at -1 dBFS:
   a line that would pass it gets less gain and `limited` is true. The
   reply's `verdict` is `sound.measure`'s, against the same lines.
4. **Save.** 44.1 kHz mono 16-bit PCM WAV, the format `parse_wav` and the
   game's announcer read. `out` names the file (a `.wav`, relative to
   `<data root>/announcer`); with `clip` it is `clips/<clip>.wav`; otherwise
   `lines/<text>-<key>.wav`.
5. **Play** through the Sound tool's audition output (`App::sound_play_clip`),
   unless `play` is false. No audio output is reported in `playback`; the
   line is still made.

## Where audio goes, and how a clip gets into the repo

Generated audio is under the data root (`--data`, else the one `h2 play`
uses) in `announcer/`, outside Git. `voice.say` never writes into the
checkout.

Committing is a separate, explicit step. The announcer's clips are
`art/announcer/<id>.wav` (Git LFS), embedded by
`boot::medal_announcer::CLIPS`. `voice.say` with `clip: "<id>"` says what
that takes (`ready`). `voice.commit {tag}` adds the saved line to the edit target as a
`sound` item of that `snd!`, and a dataset or weights path is refused.
The document header's **Save** and `voice.commit {clip}` make a
standalone `sound` item of the edit target. For bundled announcer art,
`voice.commit {clip, repo}` exports a clip to
`art/announcer/<clip>.wav` in the specified checkout. An existing
export needs `replace`. The reply gives the `CLIPS` entry to add, and a medal
needs its id in `hud_kit_core::huds::superbounce::MEDALS` as well
([superbounce.md](../game/superbounce.md), "Medals"). `git add` and the
commit are the user's.

## What is not here

- The window waits while the voice is asked; there is no progress or
  cancel.
- Only Fish Audio among services, only the Keychain, only macOS. The other
  engine is local ([voice-cloning.md](voice-cloning.md)).
- No per-line voice settings beyond `speed` and `model`.
