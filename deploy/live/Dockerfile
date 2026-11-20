# syntax=docker/dockerfile:1.7
#
# The Live server (API, web dashboard, stats site) as a small image:
# `h2-live-server` (crates/live/live-server, the server without the rest of `h2`)
# and the built web app. No disc and no game art inside: the art is extracted
# on a machine with the disc and mounted at /art (docs/live/live-hosting.md,
# "Docker with a named tunnel").
#
# Build from the repository root:
#   docker build -f deploy/live/Dockerfile -t h2live .
# or let deploy/live/compose.yaml do it.

ARG RUST_IMAGE=rust:1.98.1-slim-trixie
ARG BUN_IMAGE=oven/bun:1
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian13:nonroot

# The web app: web/dist.
FROM ${BUN_IMAGE} AS web
WORKDIR /src/web
COPY web/package.json web/bun.lock web/bunfig.toml ./
RUN bun install --frozen-lockfile
COPY web/ ./
RUN bun run build

# The server binary. Only live-server and its dependencies compile; the rest
# of the workspace is here so cargo can read the workspace and lock file.
FROM ${RUST_IMAGE} AS server
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p live-server --bin h2-live-server \
    && install -D -m 0755 target/release/h2-live-server /out/h2-live-server

# The runtime: glibc and libgcc, no shell, runs as uid 65532.
FROM ${RUNTIME_IMAGE}
COPY --from=server /out/h2-live-server /usr/local/bin/h2-live-server
COPY --from=web /src/web/dist /srv/web
ENV H2_LIVE_WEB=/srv/web \
    H2_LIVE_DATA=/data
ENTRYPOINT ["/usr/local/bin/h2-live-server"]
CMD ["--listen", "127.0.0.1:8074"]
