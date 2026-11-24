# firefight

Portable Firefight definitions and admission rules for a one-team Slayer match. A definition carries authored tuning and kit class names; `ScriptDirector` loads it as data for `content/modes/rules/firefight.rules`, which turns the host’s observations into commands.

## Where it sits

`formats` domain. Depends on `content` and `rules`. No I/O and no guest types, so Studio, the cli, Live and the host runtime link the same code.

## Layout

- `src/def.rs`, `src/validate.rs`: the definition and its admission rules.
- `src/director.rs`: the director's interface types (settings, observations, commands, state).
- `src/script.rs`: the wave-set data loader and the `ScriptDirector` that runs the script.
- `src/scoring.rs`, `src/wire.rs`, `src/builtin.rs`: scoring, the wire form and the built-in definitions (the files of `content/modes/firefight/`).

## Tests

`cargo nextest run -p firefight`.

## Docs

- [game/firefight.md](../../../docs/game/firefight.md)
