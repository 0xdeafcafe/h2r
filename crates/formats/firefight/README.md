# firefight

Portable Firefight definitions and admission rules for a one-team Slayer match. A definition carries authored tuning and kit class names; the director turns it and what the host observes into host commands, and `script` compiles a definition to a rules script that does the same.

## Where it sits

`formats` domain. Depends on `content` and `rules`. No I/O and no guest types, so Studio, the cli, Live and the host runtime link the same code.

## Layout

- `src/def.rs`, `src/validate.rs`: the definition and its admission rules.
- `src/director.rs`: definition and observations to host commands.
- `src/script.rs`: a definition compiled to a rules script.
- `src/scoring.rs`, `src/wire.rs`, `src/builtin.rs`: scoring, the wire form and the built-in definitions.

## Tests

`cargo nextest run -p firefight`.

## Docs

- [game/firefight.md](../../../docs/game/firefight.md)
