# crates/platform

Platform HLE: the Xbox's devices and libraries replaced on the host
(Direct3D 8, DirectSound, XAPI, gamepads, Bink, voice, WMA), and the host
graphics and audio underneath them. A crate here may depend on `formats`
and `runtime` and on other platform crates, and on nothing above
(`scripts/crate-domains.toml`, checked by `scripts/crate-graph.py layers
--check`). The guest-facing crates (`d3d`, `dsound`, `xapi`, `bink-hle`,
`voice`) read guest memory and install overrides into the runtime; the
host-facing ones (`gfx`, `audio`, `input`) know nothing of the guest and
can be reused by a port. See
[docs/dev/crate-structure.md](../../docs/dev/crate-structure.md).

| Crate | What |
|---|---|
| [assets](assets/) | Glue from decoded bitmap tags to `gfx` texture decoding |
| [audio](audio/) | DirectSound-level audio model, Xbox ADPCM, software mixer, host output |
| [bink-hle](bink-hle/) | Overrides for the statically linked Bink 1.x player |
| [d3d](d3d/) | Guest-facing Direct3D 8 (XDK 5849) on top of `gfx` |
| [dsound](dsound/) | Guest-facing DirectSound on top of `audio` |
| [gfx](gfx/) | Xbox-flavoured Direct3D 8 data model and renderer on wgpu |
| [hot-render](hot-render/) | WGSL overrides, tunables and toggles for live renderer iteration |
| [input](input/) | Xbox gamepad model, 4-port device model and host backends |
| [voice](voice/) | Microphone and headphone media objects over host capture and playback |
| [wma](wma/) | Clean-room WMAv2 decoder |
| [xapi](xapi/) | Guest XAPI service replacements |
