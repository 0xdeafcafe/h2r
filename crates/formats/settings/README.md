# settings

Our host settings as a schema. A setting is one `Def`: its key in
`settings.toml`, label and help, allowed values, default, the environment
variable and flag that override it, and whether a change applies at once
or at next launch. A `Store` loads every `Def` from the file, the
environment and the flags, and hands out `Setting<T>` handles whose read
is one atomic load. It is the only writer of `settings.toml`.

## Where it sits

Domain `formats`; no workspace dependencies. `h2host`, `boot`, `cli`,
`game`, `agent` and `cloud-saves` use it.

## Layout

- `src/def.rs`: `Def`, `Kind`, `Apply`, `Scope`, `Tier`, `Value`.
- `src/store.rs`: `Store`, `Setting<T>`, `Source`, `Outcome`.
- `src/file.rs`: `settings.toml` and profiles (`path_in`, `write_value`,
  `write_profile_value`).

## Tests

`cargo nextest run -p settings`.

## Docs

- [docs/engine/feature-engine.md](../../../docs/engine/feature-engine.md)
- [docs/runtime/switches.md](../../../docs/runtime/switches.md): the
  switches and variables the settings map to.
