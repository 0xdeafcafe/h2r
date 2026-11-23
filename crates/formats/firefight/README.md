# firefight

Portable Firefight definitions and admission rules for a one-team Slayer match. A definition carries authored tuning and kit class names; `script` compiles it to a rules script whose running (`ScriptDirector`) is the director: it turns what the host observes into host commands.

## Where it sits

`formats` domain. Depends on `content` and `rules`. No I/O and no guest types, so Studio, the cli, Live and the host runtime link the same code.

## Layout

- `src/def.rs`, `src/validate.rs`: the definition and its admission rules.
- `src/director.rs`: the director's interface types (settings, observations, commands, state).
- `src/script.rs`: a definition compiled to a rules script, and the `ScriptDirector` that runs it.
- `src/scoring.rs`, `src/wire.rs`, `src/builtin.rs`: scoring, the wire form and the built-in definitions.

## Tests

`cargo nextest run -p firefight`.

## Docs

- [game/firefight.md](../../../docs/game/firefight.md)
