# Live Worker: build slices and results

## What changed to make it build (slice 1)

### live-server

- **Features.** A `native` feature (on by default) holds everything that
  cannot build for `wasm32-unknown-unknown`:
  - `sqlx` (with `pg.rs`, `sqlite.rs`, `copy.rs`, `check.rs` and `standalone.rs`), `axum-server`, `rustls`, `reqwest` and `tower-http`;
  - object_store's file and S3 stores;
  - tokio's net, fs and multi-thread runtime;
  - axum's `ws`, `tokio`, `http1` and `http2`.

  `cargo check -p live-server --no-default-features --target wasm32-unknown-unknown` passes, and clippy is clean.
- **WebSocket routes.** Stream, ingest and relay are native-only (`http::sockets`). The relay's room logic stays shared.
- **`rt`.** Spawning, sleep, timeout, interval and `Instant` go through `rt`:
  - natively it is tokio;
  - in a Worker it is `spawn_local`, `setTimeout` and `performance.now()`.

  `std::time::Instant` panics in a Worker. A JavaScript timer is not `Send`, so it is wrapped (`send_wrapper`).
- **`ConnectInfo`.** A Worker has no socket, so `ConnectInfo` is never set there. Nothing counts as "this machine", which turns local sign-in and the loopback-only admin details off.
- **Blobs.** `BlobStore::from_store` takes any object store. `put_file` (uploads spooled to a temporary file) fails on a Worker; see slice 4.
- **Shared SQL.** The SQLite statements moved to `sql.rs`, shared by `SqliteStore` and the Worker's store.
- **Cache headers.** Anonymous reads are `public, s-maxage=5` with an ETag; signed reads are `private, no-store`.
- **Public helpers.** `BatchRows`, `BatchDecoder` and `batch_key` are public. The shared store tests are public behind the `conformance` feature.

### live-proto

On wasm32 it uses getrandom's JavaScript backends (0.2 `js` and 0.3 `wasm_js`) and `time`'s `wasm-bindgen` clock.

### crates/live/live-worker

- **`sqlstore`** (named `d1store` in slice 1): a `Store` (`EventStore`, `TitleBlobStore` and `UsageStore`) over `Sql`. It builds and is tested natively against SQLite:
  - it passes the shared store tests for events, blobs and usage (`conformance::run`);
  - its rows are `SqliteStore`'s own, checked in both directions on one file.
- **`dosql`** (slice 2; it replaced slice 1's `d1`): the hub's own storage as `Sql`.
- **`r2`**: R2 as an `ObjectStore`. It covers get, range, head, put (overwrite, create-only, if-match), delete and list; multipart and copy are not done yet.
- **`entry`**: the fetch handler and the `LiveHub` object, which starts `Live` on its first request and answers with the router.
- **`wrangler.toml`**: the local defaults and the `staging`, `cutover` and `production` environments ([Deploy](live-worker-deploy.md#deploy)).

### Building and running on this machine

The repository pins Rust 1.98.1 (rust-toolchain.toml). Homebrew's 1.98.1 has
no wasm32 standard library, so the Worker is built with rustup's 1.98.1
plus the `wasm32-unknown-unknown` target and `worker-build`. That toolchain
lives in a home of its own, so the machine's toolchain is untouched:

```
export RUSTUP_HOME=$TMPDIR/lw2/rustup CARGO_HOME=$TMPDIR/lw2/cargo   # rustup-init, once
rustup target add wasm32-unknown-unknown; cargo install worker-build
cd crates/live/live-worker && worker-build --release     # build/index.js, build/index_bg.wasm
crates/live/live-worker/smoke/run.sh 1000                # wrangler dev, the smoke test, a restart
```

For the install only, `CARGO_HOME` points at the private home; the builds
then run with `RUSTUP_HOME` alone, so cargo keeps the shared registry.

The bundle is 4.4 MB of wasm, 1.35 MB gzipped. Workers allow 3 MB on the
free plan and 10 MB on paid.

`smoke/run.sh` runs `bunx wrangler@4 dev --local`: workerd and miniflare on
this machine, with no account. It writes its own wrangler config and a dev
server key under `$TMPDIR/live-worker-dev`.

`smoke/smoke.ts` uses the web app's own sign-up, token and event code. It
checks, in order:
- the first request (the cold start);
- an anonymous `/v1/snapshot`;
- sign-up;
- a token-signed read (sign-in);
- an event append, read back from the log;
- the stream: a socket subscribed before the append receives the event live,
  a replay from a position, a token-authed reader, a bad token closed with
  4401, and a plain GET refused with 400;
- with `WAKE=S`, a hibernation wake with a socket open;
- game ingest to JSON and packed readers, then game close, alarm-driven archive publication and reads of both stored segments;
- R2: a multipart content upload, a cloud save and a 2 MiB title file;
- the same screenshot HTTP contract as native Live, from proof rejection
  and private preview through approval, reporting and removal;
- warm timings;
- with `--seed N`, 2N more events so a restart has a log to replay.

Measurements for slices 2 to 5 and the replay archive smoke: [notes/2026-11-18-worker-slice-results.md](notes/2026-11-18-worker-slice-results.md).
