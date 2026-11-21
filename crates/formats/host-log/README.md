# host-log

An append-only log of host commands for Forge and Firefight. Each entry
carries its sequence, apply tick, payload and predecessor hash. Forge
also supplies the authenticated editor in the optional `author` field.
The director omits it.

## Where it sits

The `formats` domain. The crate uses `content` for SHA-256 and integer-only
canonical JSON. A payload implements `serde::Serialize` to be hashed.
Schemas encode fractional values as strings or integer bit patterns.
Each caller owns command validation, authorisation and input limits.

## Chain

- `LogHead::genesis` starts at sequence and tick zero, with the caller's
  starting document or session digest. `Log::new` also accepts a trusted
  checkpoint for catch-up or replay.
- `Log::push` checks the next sequence, predecessor and nondecreasing tick
  before advancing the head. It hashes the canonical UTF-8 JSON of the full
  entry, including the lowercase hexadecimal predecessor hash and the
  author when present. Authored entries use Forge F1's wire shape and hash.
- `Log::since(seq)` borrows the retained entries strictly after that
  sequence. A sequence before the checkpoint yields the whole retained
  suffix.
- `Log::verify` recomputes the chain from its starting checkpoint and
  compares it with a trusted final head. This detects changes to the final
  entry and lost tail entries as well as broken links.

## Tests

`cargo test --locked -p host-log` covers the chain, checkpoint catch-up,
refusals and canonical hash vectors, including Forge F1's authored entry.

## Docs

- [AI over the network](../../../docs/game/ai-networking.md)
- [Forge design](../../../docs/game/forge-design.md)
