# live-worker

The Live server as a Cloudflare Worker: `live-server`'s routes over D1, R2 and Durable Objects. It is the hosted home of the same service.

## Where it sits

`live`. Uses `live-server` (without its `native` feature) and `live-proto`. Nothing depends on it; `deploy.sh` ships it.

## Layout

- `src/entry.rs`: the Worker's fetch handler.
- `src/hostroom.rs`, `src/relayroom.rs`: the host and relay Durable Objects.
- `src/sqlstore.rs`, `src/dosql.rs`, `src/doretry.rs`: the event log in the object's SQLite storage.
- `src/r2.rs`: blobs in R2.
- `src/settings.rs`, `src/site.rs`: settings and the site.
- `wrangler.toml`, `deploy.sh`, `smoke/`: deploy and smoke checks.

## Tests

`cargo nextest run -p live-worker`. `sqlstore` and `settings` build and test on the host; `entry`, `dosql` and `r2` build for wasm32 only.

## Docs

- [live-worker.md](../../../docs/live/live-worker.md)
- [dedicated-host.md](../../../docs/live/dedicated-host.md)
