# asf

A bounded ASF demuxer. It splits the game's WMA streams into packets and
reassembled media objects, with every wire read checked and payload bytes
borrowed from the input. It decodes nothing; `wma` does that.

## Where it sits

Domain `formats`. It depends on no domain above it (`map`, `tags` and
`xiso` only behind the `disc` feature). `wma` and `dsound` use it, and
`cli` uses it for disc inspection.

## Layout

- `src/packet.rs`: `Packet`, `Payload`, `MediaObject` and the `Reassembler`.
- `src/disc.rs`: finds the WMA streams on a disc image (`disc` feature).

## Features

- `disc`: the disc walker, which pulls in `xiso`, `map` and `tags`.

## Tests

`cargo nextest run -p asf`. The unit tests use invented streams.

## Docs

- [docs/audio/wma.md](../../../docs/audio/wma.md): provenance and the
  features that are deliberately unsupported.
