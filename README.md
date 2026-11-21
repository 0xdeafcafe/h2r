<p align="center">
  <img src="docs/images/readme/header.webp" alt="A panorama of Old Mombasa, Delta Approach and Ascension" width="100%">
</p>

# h2recomp

h2recomp recompiles the original Xbox version of Halo 2 to native code.
The recompiler lifts the retail executable from your disc into an
intermediate representation. Cranelift compiles the fast-start image, and
clang builds the optimised image from generated C11. A Rust runtime supplies
the Xbox kernel, graphics and audio services. [Architecture](docs/ARCHITECTURE.md)

<p align="center">
  <img src="docs/images/readme/oldmombasa-4x.jpg" alt="Old Mombasa at 4x render scale" width="32%">
  <img src="docs/images/readme/deltaapproach-4x.jpg" alt="Delta Approach at 4x render scale" width="32%">
  <img src="docs/images/readme/ascension-4x.jpg" alt="Ascension at 4x render scale" width="32%">
</p>

You'll need your own legally obtained copy of the game. The documentation images are rendered from my disc by scripts and kept in Git LFS. [How the README media is made](CONTRIBUTING.md#readme-media).

h2recomp was created under Microsoft's "[Game Content Usage Rules](https://www.xbox.com/en-US/developers/rules)" using assets from Halo 2 and Halo: Combat Evolved, and it is not endorsed by or affiliated with Microsoft. Halo © Microsoft Corporation. This is a free, non-commercial fan project.

## Play it

**1. Get your ISO.** Use an XISO or a full Redump image of your Halo 2
disc. Save it as `disc/halo2.iso` inside the checkout once you've cloned
it below. The checkout ignores `disc/`.

**2. Install the CLI on Mac.** Install Rust through rustup and have clang
on your PATH. The checkout's `rust-toolchain.toml` selects the Rust toolchain.

```sh
git clone https://github.com/0xdeafcafe/h2r
cd h2r
cargo install --locked --path crates/apps/cli
```

```sh
mkdir -p disc
cp -c ~/Downloads/your.iso disc/halo2.iso
```

For the Multiplayer Map Pack and Halo 1, place your disc images beside
it as `mappack.iso` and `halo1.iso`. The player imports them on launch.

**3. Play.**

```sh
h2 play disc/halo2.iso
h2 play disc/halo2.iso --live
h2 play disc/halo2.iso --level ascension
```

The first launch uses Cranelift to build a fast-start image, then clang
builds the full-speed image in the background. The window tells you when
the clang build is ready. Restart to use it. Later launches reuse a
matching cached image in `$TMPDIR/h2boot`.
[Fast-start design](docs/lifter/cranelift-tier.md) · [player options](docs/runtime/app.md)

WASD and the mouse move and look. Enter is A, Escape is Start and Cmd+F
toggles fullscreen. `h2 play --follow` builds new commits in the background
and offers a restart when one is ready.

On macOS, run `just mac-install iso=disc/halo2.iso` after installing the CLI
to open **Join in Halo 2** links from the Live website. It installs
`~/Applications/Halo 2.app` and remembers where your disc is. Run the
recipe again after updating the CLI or moving the disc.

## What you can play and use

On Mac, you can play the Halo 2 campaign, local multiplayer and online
matches. The iPhone and iPad app has touch controls. The Apple TV app
plays its bundled levels at reduced performance.
[Platform guide](docs/runtime/building.md)

- **Any resolution or aspect.** Native follows the window or display in physical pixels and keeps its aspect within a 4K pixel budget. Set a fixed width and height in Video settings. The native GPU renderer supports ultrawide views, and the game redraws during live window resize. [Resolution and aspect](docs/graphics/high-resolution.md) · [renderer](docs/graphics/native-renderer.md)
- **Higher frame rates.** The game simulates at 30 Hz. The player offers 60 and 120 fps presentation, with in-between frame work in progress. [High frame rate](docs/game/high-framerate.md) · [measured performance](docs/features.md#speed)
- **Classic maps and modes.** Import the Multiplayer Map Pack from your disc. Convert all 13 Halo 1 multiplayer maps and the Halo 1 campaign from your Halo 1 disc. Campaign conversion and boot work, with full playthroughs still unverified. Custom games include community variants such as Zombies and SWAT. [Map Pack](docs/game/map-pack.md) · [Halo 1 campaign](docs/halo1/campaign.md) · [Halo 1 multiplayer](docs/halo1/multiplayer.md) · [variants](docs/game/community-gametypes.md)
- **Clankers and Firefight.** Clankers join hosted matches as network peers. Firefight and networking for campaign-style AI are in progress. [Clankers](docs/game/bots.md)
- **Theater.** Record films on a dedicated host or with `h2 play --film`. The in-game library opens host films with the game's renderer, director cameras and playback controls. Seeking uses compatible keyframes when available, and replay tools report timing, tape size and determinism checks. Real-host verification is still pending. [Theater](docs/game/theater.md) · [film format](docs/game/film-format.md)
- **Forge.** `h2 forge check` validates a Forge document against your disc, and `h2 forge build` turns it into a local map package. The in-game monitor and editing tools are planned. [Forge design](docs/game/forge-design.md)
- **Skulls and settings.** Campaign skulls are saved per profile, and the game has settings screens for video, controls, field of view and voice. [Skulls](docs/game/skulls.md) · [settings](docs/game/settings-menu.md)

## Live, and the stats site

Xbox Live for the original Xbox shut down in 2010, so I wrote a new one. It runs at **[live.forbes.red](https://live.forbes.red)**, and `h2 play --live` signs you in to it.

Live has gamertags, friends, teams, invites, matchmaking with served
playlists, online campaign co-op and cloud saves. The **Custom Game Browser**
lists joinable games. Press Y to filter by map category, variant, open
seats, Clankers or co-op difficulty. Dedicated hosting is in progress.
[Browser and filters](docs/live/live-custom-games.md) · [co-op](docs/live/live-coop.md)

There's a web app beside it: a dashboard in the style of the original Xbox one, and a stats site in the style of Bungie.net's Halo 2 and 3 stats pages. It shows who's online, games as they happen on a live top-down map with the scoreboard and kill feed, replays of finished games and the ranks.

You can run your own too: `h2 live serve` on a Mac, Docker behind a Cloudflare Tunnel, or Cloudflare Workers. [Live](docs/live/live.md) · [hosting](docs/live/live-hosting.md) · [Workers](docs/live/live-worker.md) · [web app](web/README.md)

## The editor and the agent API

`h2-editor` is a native map and variant editor in the spirit of Sapien.
Open a map from your disc, edit placements in 3D, inspect and edit tags
and HUDs, and make variants and playlists. A paired game can show live
world state and accept supported edits during offline play. The editor
also reads Halo 1 maps. [Editor guide](docs/editor/editor-guide.md) ·
[paired editing](docs/editor/live-mirror.md)

```sh
cargo run --release -p studio -- --disc disc/halo2.iso
```

The agent API is how a script, a test or a coding agent reads and drives the running game without squinting at screenshots. `h2 serve` runs the game with a JSON-RPC socket, and `h2 agent` asks it what's in the level, where the frame time goes and what the menus show, presses buttons, waits for things to happen, and saves and restores state. [Agent API](docs/runtime/agent-api.md) · [the editor's](docs/editor/agent-api.md)

```sh
h2 agent status
h2 agent query '{"kind":["actor"],"near":{"player":0,"radius":30},"fields":["pos","team"]}'
```

`h2 lab` uses the agent API over SSH to build a branch and run it on a
second Mac, replay inputs, and collect frame timings, profiles and logs.
[Lab guide](docs/runtime/lab.md)

## Rendering

The native renderer builds scene lists in Rust and draws them with wgpu.
The Direct3D path handles draws the native renderer does not cover.
[Native renderer](docs/graphics/native-renderer.md)

<p align="center">
  <img src="docs/images/readme/render-pipeline.svg" alt="Render pipeline before and after: guest front-end, D3D translate, two copies; then native scene lists and one native render" width="100%">
</p>

## Building from source

The root `justfile` builds every target from your disc (`brew install just`):

```sh
just doctor          # what each target needs and what's missing
just mac-play        # build and play on this Mac
just tvos            # the Apple TV app; then just tvos-install
just ios maps=all    # the iPhone/iPad app with every map and the map pack
just check           # fmt, clippy and tests, as before a push
```

`cargo h2 <command>` runs the CLI straight from the checkout. Every recipe, the full command list and Ghidra are in [docs/runtime/building.md](docs/runtime/building.md), and [CONTRIBUTING.md](CONTRIBUTING.md) has the rules (the main one: no game data, ever).

## Docs

[docs/README.md](docs/README.md) is the index, every doc grouped by area. Good places to start:

- [ARCHITECTURE.md](docs/ARCHITECTURE.md): the design, end to end
- [features.md](docs/features.md): every feature with its state, the speed numbers and the roadmap
- [LIFTER.md](docs/lifter/LIFTER.md): x86 analysis, lifting and code generation
- [native-renderer.md](docs/graphics/native-renderer.md): the GPU renderer
- [live.md](docs/live/live.md): the Xbox Live replacement
- [editor-guide.md](docs/editor/editor-guide.md) and [agent-api.md](docs/runtime/agent-api.md): the tools
