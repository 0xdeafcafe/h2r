<p align="center">
  <img src="docs/images/readme/header.webp" alt="Ascension, Delta Approach and Old Mombasa at 4x render scale, blended into one banner" width="100%">
</p>

# h2recomp

Halo 2. The original Xbox one, from 2004. Recompiled into a native Mac app, no emulator in sight.

The XBE gets lifted to C, all 18,700-odd functions of it, and clang compiles the lot into a native library. A Rust runtime stands in for the Xbox underneath: kernel, Direct3D and DirectSound all swapped out, with a native GPU renderer drawing the frame on Metal.

Then I kept going. Xbox Live is back, there are bots, and Halo 1 runs in the Halo 2 engine.

<p align="center">
  <img src="docs/images/readme/montage.webp" alt="Old Mombasa, Delta Approach and Ascension at 4x render scale, crossfading" width="100%">
</p>

## Why it's sick

- **60 and 120 fps** over a game that ticks at 30, and a render scale up to 6x. [High frame rate](docs/game/high-framerate.md) · [render scale](docs/graphics/high-resolution.md)
- **A native GPU renderer.** Scene lists built in Rust, geometry kept on the GPU between frames, and the game's own draw handlers skipped. On by default. [Renderer](docs/graphics/native-renderer.md)
- **Xbox Live is back.** Microsoft shut it off in 2010, so I wrote a new one. Gamertags, friends, Teams, invites, OptiMatch, Ranked, the custom games browser and cloud saves. Self-hosted, running at [live.forbes.red](https://live.forbes.red). [Live](docs/live/live.md)
- **Co-op campaign over Live.** Retail only ever did split screen on one console. [Co-op](docs/live/live-coop.md)
- **A dashboard and h2 Uplink.** The Live dashboard looks like the original Xbox one. [Uplink](https://live.forbes.red/games/halo2) is the stats site, with proper mid-2000s Bungie.net vibes: service records, carnage reports, the Medal Chest, leaderboards, and a 3D game viewer that watches matches live or replays them after. [Web app](docs/live/web-app.md) · [3D viewer](docs/live/web-viewer.md)
- **Theater.** Films recorded by dedicated hosts, played back through the game's own renderer with director cameras. Early days, but it works. [Theater](docs/game/theater.md)
- **Clankers.** Bots that join as phantom players, so every controller port stays free for an actual person. A Clanker's gamertag picks its emblem and colours, and Clankermatch is a whole matchmaking playlist of them. Personalities derived from the name are coming. [Bots](docs/game/bots.md)
- **Mods and map packs.** Maps as packages with their own slots, a Mods screen in the game, and joining a Live lobby fetches whatever layers you're missing. The Multiplayer Map Pack installs too. [Maps](docs/game/maps.md) · [Map Pack](docs/game/map-pack.md)
- **13 community game variants** built in, Zombies and SWAT included, plus [Skulls](docs/game/skulls.md) per profile and a few of our own. [Variants](docs/game/community-gametypes.md)
- **Halo 1 in the Halo 2 engine.** All 13 Halo 1 multiplayer maps, and the campaign levels converted with Halo 1's health model. The campaign converts and boots, nobody's played one through yet. [Campaign](docs/halo1/campaign.md) · [multiplayer](docs/halo1/multiplayer.md)
- **An editor suite.** `h2-editor` is a Sapien-style scenario editor: move things around a map in 3D, edit any field of any tag, build game variants and the 16 matchmaking playlists. Pair it with the running game and your placement edits go straight across. [Editor guide](docs/editor/editor-guide.md)
- **An agent API.** `h2 serve` runs the game with a JSON-RPC socket, and `h2 agent` asks it what's in the level and where the frame time went, presses buttons, waits for things to happen, and saves and restores state. No squinting at screenshots. [Agent API](docs/runtime/agent-api.md)
- **iPhone, iPad and Apple TV** apps too, with touch controls on iOS. The Apple TV one is still slow. [iOS](docs/runtime/ios.md) · [tvOS](docs/runtime/tvos.md)

Everything, with what's only partly done and the measured speed numbers, is in [docs/features.md](docs/features.md).

## Run it

You need:

- A Halo 2 disc image of your own. XISO or a full Redump image both work.
- A Mac with Apple silicon.
- Rust via rustup (`rust-toolchain.toml` pins the version) and clang on your `PATH`.

Put the image in `disc/`, which is git-ignored. `cp -c` clones it without using any extra space. Got the Multiplayer Map Pack or Halo 1 discs too? Drop them in beside it as `mappack.iso` and `halo1.iso` and they're imported on the first run.

```sh
git clone https://github.com/0xdeafcafe/h2r && cd h2r
mkdir -p disc && cp -c ~/Downloads/your.iso disc/halo2.iso
cargo install --locked --path crates/apps/cli
```

Then play:

```sh
h2 play disc/halo2.iso                          # window, sound, keyboard, mouse and pads
h2 play disc/halo2.iso --live                   # signed in to live.forbes.red
h2 play disc/halo2.iso --level 03a_oldmombasa   # straight into a level, no menus
```

The first run builds the game library into `$TMPDIR/h2boot`, which takes a while. Every run after that reuses it. WASD and the mouse to move and look, Enter is A, Escape is Start, Cmd+F for fullscreen. `--follow` keeps you on the latest master, building each new commit in the background. Every flag is in [docs/runtime/app.md](docs/runtime/app.md).

The editor:

```sh
cargo run --release -p studio -- --disc disc/halo2.iso
```

Poke the running game from a script:

```sh
h2 agent status
h2 agent query '{"kind":["actor"],"near":{"player":0,"radius":30},"fields":["pos","team"]}'
```

Want your own Live? `h2 live serve` on a Mac, Docker behind a Cloudflare Tunnel, or Cloudflare Workers. [Hosting](docs/live/live-hosting.md) · [Workers](docs/live/live-worker.md) · [web app](web/README.md)

## How it works

The XBE is analysed, every function is lifted from x86 to an IR and emitted as C11, and clang builds that into a library. Nobody edits the generated code by hand, it's always regenerated. The Rust runtime underneath replaces the Xbox kernel, XAPI, Direct3D 8 and DirectSound on a deterministic logical clock, and renders through wgpu on Metal. The native renderer skips the game's own draw handlers and falls back to the D3D path for anything it can't take yet. Bit by bit, the hot parts of the game get rewritten in Rust, overriding the lifted function at the same address.

<p align="center">
  <img src="docs/images/readme/render-pipeline.svg" alt="Render pipeline before and after: guest front-end, D3D translate, two copies; then native scene lists and one native render" width="100%">
</p>

The deep dives:

- [ARCHITECTURE.md](docs/ARCHITECTURE.md): the whole design, end to end
- [LIFTER.md](docs/lifter/LIFTER.md): x86 to IR to C11
- [native-renderer.md](docs/graphics/native-renderer.md): the GPU renderer
- [live.md](docs/live/live.md): the Xbox Live replacement
- [docs/README.md](docs/README.md): every doc, grouped by area

## Building everything

The root `justfile` builds every target (`brew install just`):

```sh
just doctor          # what each target needs and what's missing
just mac-play        # build and play on this Mac
just tvos            # the Apple TV app; then just tvos-install
just ios maps=all    # the iPhone/iPad app with every map and the map pack
just check           # fmt, clippy and tests, as before a push
```

`cargo h2 <command>` runs the CLI straight from the checkout. Every recipe is in [docs/runtime/building.md](docs/runtime/building.md), and [CONTRIBUTING.md](CONTRIBUTING.md) has the rules. The main one: no game data, ever.

## The legal bit

h2recomp is a free, non-commercial fan project. It is not affiliated with, endorsed by or sponsored by Microsoft, Xbox Game Studios, Bungie, 343 Industries or Halo Studios. Halo © Microsoft Corporation. All trademarks belong to their owners.

h2recomp was created under Microsoft's "[Game Content Usage Rules](https://www.xbox.com/en-US/developers/rules)" using assets from Halo 2 and Halo: Combat Evolved, and it is not endorsed by or affiliated with Microsoft.

No game data is in this repository: no executables, maps, textures, sounds or other copyrighted content from Halo 2 or Halo: Combat Evolved. To play, you need your own legally obtained copy of the game, and h2recomp builds everything from that disc image on your machine. The only screenshots here are the docs' own, rendered by [scripts/readme-screenshots.sh](scripts/readme-screenshots.sh) and stored in Git LFS.

The source code is licensed under MIT OR Apache-2.0, at your option (`license` in [Cargo.toml](Cargo.toml)). Fonts bundled with the editor keep their own licences, alongside them in [crates/apps/studio/assets/fonts](crates/apps/studio/assets/fonts).
