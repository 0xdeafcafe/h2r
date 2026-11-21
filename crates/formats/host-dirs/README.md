# host-dirs

The per-user `h2recomp` directories: app data, cache and config, under the
convention of the host. One closed enum, `Convention` (`Apple`, `Windows`,
`Xdg`), and one `cfg!` const, `Convention::HOST`, pick the convention;
everything else is a plain function of the convention and an environment
lookup, so every convention is tested on every host. It has no
dependencies.

## Where it sits

Domain `formats`, so every other domain may use it. `content`, `movies`,
`vfs`, `snapshot`, `d3d`, `bots`, `agent-proto`, `cli` and `xtask` use it.

## Layout

- `src/lib.rs`: `Convention`, `data_dir`, `cache_dir`, `config_dir`, and
  `data`, `cache` and `config` for this host and process.

## Tests

`cargo nextest run -p host-dirs`.

## Docs

- [docs/runtime/storage.md](../../../docs/runtime/storage.md), "Host
  directories": the path table.
