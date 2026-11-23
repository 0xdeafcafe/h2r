<p align="center">
  <img src="docs/images/readme/header.webp" alt="A panorama of Old Mombasa, Delta Approach and Ascension" width="100%">
</p>

# h2recomp

Halo 2. The original Xbox one, from 2004. Recompiled into a native Mac app, no emulator in sight.

The XBE gets lifted to C, all 18,700-odd functions of it, and clang compiles the lot into a native library. A Rust runtime stands in for the Xbox underneath: kernel, Direct3D and DirectSound all swapped out, with a native GPU renderer drawing the frame on Metal.

Xbox Live is back, there are bots, there's Forge and Firefight, and Halo 1 runs in the Halo 2 engine.

<p align="center">
  <img src="docs/images/readme/oldmombasa-4x.jpg" alt="Old Mombasa at 4x render scale" width="32%">
  <img src="docs/images/readme/deltaapproach-4x.jpg" alt="Delta Approach at 4x render scale" width="32%">
  <img src="docs/images/readme/ascension-4x.jpg" alt="Ascension at 4x render scale" width="32%">
</p>

## Why it's sick

- **60 and 120 fps** over a game that ticks at 30, any resolution or aspect, ultrawide included, and a render scale up to 6x. [High frame rate](docs/game/high-framerate.md) · [resolution](docs/graphics/high-resolution.md)
- **A native GPU renderer.** Scene lists built in Rust, geometry kept on the GPU between frames, and the game's own draw handlers skipped. On by default. [Renderer](docs/graphics/native-renderer.md)
- **Xbox Live is back.** Microsoft shut it off in 2010, so I wrote a new one. Gamertags, friends, Teams, invites, OptiMatch, Ranked, the custom games browser with filters, and cloud saves. Self-hosted, running at [live.forbes.red](https://live.forbes.red). [Live](docs/live/live.md)
- **One lobby for everything.** Multiplayer, Custom Games, Forge, Firefight and Co-op are tabs in one party lobby. Pick a game with Clankers or without from the same Multiplayer page, set the Clankers up in Custom Games, Forge and Firefight, and tune the match in the Tweakers menu. [Party lobby](docs/game/party-lobby.md)
- **Co-op campaign over Live.** Retail only ever did split screen on one console. [Co-op](docs/live/live-coop.md)
- **Dedicated hosts.** Live matches run on small hosts that start for a match and stop after. The host image carries native code only for what a match actually runs, so it stays tiny. [Dedicated hosts](docs/live/dedicated-host.md)
- **A dashboard and h2 Uplink.** The Live dashboard looks like the original Xbox one. [Uplink](https://live.forbes.red/games/halo2) is the stats site, with proper mid-2000s Bungie.net vibes: service records, carnage reports, the Medal Chest, leaderboards, and a 3D game viewer that watches matches live or replays them after. Medals come from the server's own record of each match, under their official names. [Web app](docs/live/web-app.md) · [3D viewer](docs/live/web-viewer.md)
- **Theater.** Films recorded by dedicated hosts or with `h2 play --film`, played back through the game's own renderer with director cameras and seeking. [Theater](docs/game/theater.md)
- **Clankers.** Bots that join as phantom players, so every controller port stays free for an actual person. A Clanker's gamertag picks its emblem, colours and personality, they hold still through the round intro like a person would, and Clankermatch is a whole matchmaking playlist of them. [Bots](docs/game/bots.md)
- **Forge and Firefight.** Edit a multiplayer map in game as the monitor, save it as a map package and share it over Live. Firefight throws waves of the campaign's AI at you on multiplayer maps. [Forge](docs/game/forge-editing.md) · [Firefight](docs/game/firefight.md)
- **Superbounce.** The engine's famous bug, measured, scored with a medal and a HUD line, and with ghost runs to chase. [Superbounce](docs/game/superbounce.md)
- **Game types as scripts.** A small rules language for new game types, checked against a tick budget before it ships. [Rules engine](docs/game/rules-engine.md)
- **Mods and map packs.** Maps as packages with their own slots, a Mods screen in the game, and joining a Live lobby fetches whatever layers you're missing. The Multiplayer Map Pack installs too. [Maps](docs/game/maps.md) · [Map Pack](docs/game/map-pack.md)
- **13 community game variants** built in, Zombies and SWAT included, plus [Skulls](docs/game/skulls.md) per profile and a few of our own. [Variants](docs/game/community-gametypes.md)
- **Halo 1 in the Halo 2 engine.** All 13 Halo 1 multiplayer maps, and the campaign levels converted with Halo 1's health model. The campaign converts and boots, nobody's played one through yet. [Campaign](docs/halo1/campaign.md) · [multiplayer](docs/halo1/multiplayer.md)
- **An editor suite.** `h2-editor` is a Sapien-style scenario editor: move things around a map in 3D, edit any field of any tag, build game variants and the 16 matchmaking playlists. It reads Halo 1 maps too. Pair it with the running game and your placement edits go straight across. [Editor guide](docs/editor/editor-guide.md) · [paired editing](docs/editor/live-mirror.md)
- **An agent API.** `h2 serve` runs the game with a JSON-RPC socket, and `h2 agent` asks it what's in the level and where the frame time went, presses buttons, waits for things to happen, and saves and restores state. No squinting at screenshots. [Agent API](docs/runtime/agent-api.md)
- **iPhone, iPad, Apple TV and Linux.** Touch controls on iOS, and a Steam Deck build. The Apple TV one is still slow. [iOS](docs/runtime/ios.md) · [tvOS](docs/runtime/tvos.md) · [SteamOS](docs/runtime/steamos.md)

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
cargo install --locked --path crates/apps/h2-tools
```

Then play:

```sh
h2 play disc/halo2.iso
h2 play disc/halo2.iso --live
h2 play disc/halo2.iso --level ascension
```

The first run starts on a quick Cranelift build of the game while clang builds the full-speed one in the background; the window tells you when it's ready, and every run after that reuses it from `$TMPDIR/h2boot`. WASD and the mouse to move and look, Enter is A, Escape is Start, Cmd+F for fullscreen. `--follow` keeps you on the latest master, building each new commit in the background. Every flag is in [docs/runtime/app.md](docs/runtime/app.md). [Fast start](docs/lifter/cranelift-tier.md)

Want **Join in Halo 2** links on the Live site to open the game? `just mac-install iso=disc/halo2.iso` installs `~/Applications/Halo 2.app` and remembers where your disc is.

The editor:

```sh
cargo run --release -p studio -- --disc disc/halo2.iso
```

Poke the running game from a script:

```sh
h2 agent status
h2 agent query '{"kind":["actor"],"near":{"player":0,"radius":30},"fields":["pos","team"]}'
```

`h2 lab` does the same on a second Mac: build a branch there, replay inputs, and bring back frame timings, profiles and logs. [Lab](docs/runtime/lab.md)

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

On Linux, [Buck2](https://buck2.build) builds everything inside a [devenv](https://devenv.sh) shell. Tell it where your disc is in `.buckconfig.local` (an ISO or a `.7z` holding one):

```ini
[h2]
  disc = ~/Downloads/halo2.7z
```

Then `devenv shell` and `buck2 run //platform/linux:play`. A Steam Machine or Steam Deck in Developer Mode gets the game with `buck2 run //platform/steamos:run`. [docs/runtime/steamos.md](docs/runtime/steamos.md) has the details.

## The legal bit

h2recomp is a free, non-commercial fan project. It is not affiliated with, endorsed by or sponsored by Microsoft, Xbox Game Studios, Bungie, 343 Industries or Halo Studios. Halo © Microsoft Corporation. All trademarks belong to their owners.

h2recomp was created under Microsoft's "[Game Content Usage Rules](https://www.xbox.com/en-US/developers/rules)" using assets from Halo 2 and Halo: Combat Evolved, and it is not endorsed by or affiliated with Microsoft.

No game data is in this repository: no executables, maps, textures, sounds or other copyrighted content from Halo 2 or Halo: Combat Evolved. To play, you need your own legally obtained copy of the game, and h2recomp builds everything from that disc image on your machine. The only screenshots here are the docs' own, rendered from my disc by scripts and stored in Git LFS. [How the README media is made](CONTRIBUTING.md#readme-media)

The code here is under the [WTFPL](LICENSE). It covers this repository's own code, never Halo 2: the disc, its maps and everything derived from them stay yours to supply. Fonts bundled with the editor keep their own licences, alongside them in [crates/apps/studio/assets/fonts](crates/apps/studio/assets/fonts).
