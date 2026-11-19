# The dedicated-host container image

A Linux x86-64 image that runs one ad-hoc Halo 2 match host
([docs/live/dedicated-host.md](../../docs/live/dedicated-host.md), "Container
image"). Inside: the Linux `h2` (host profile, no GUI) and the game module
`build.sh` compiled from **your own disc**. Not inside: any game data — maps
and shared disc data are fetched at start from your private bundle
(`h2 host bundle`, contract 7) and cached under `/var/cache/h2`.

## Build

On a Mac, from the repository root (the script checks for `cargo-zigbuild`,
`zig` and the rust target, and says what to install rather than installing
it):

```sh
deploy/host/build.sh /path/to/halo2.iso
(cd deploy/host && docker build -t h2host:local .)
```

`build.sh` cross-builds `h2` with `cargo zigbuild --release -p cli
--no-default-features --features host --target x86_64-unknown-linux-gnu.2.36`
(glibc 2.36 = the image's `debian:bookworm-slim`), compiles the Linux module
from the disc with this checkout's macOS `h2` (the long step; skipped when
the staged module is already current), and stages everything into
`deploy/host/stage/`, which is gitignored and is the docker build context.

## Run

```sh
docker run --rm \
  -p 31000-31006:31000-31006/udp -p 31100:31100 \
  -e H2_LEASE=hl-... -e H2_LIVE=https://live.forbes.red \
  -e H2_TOKEN=... -e H2_DATA_URL='https://...' -e H2_DATA_SHA=... \
  h2host:local
```

The entrypoint maps the environment to `h2 host run` (contract 9) and execs
it, so the host process is the container's main process and stops cleanly
with the container.

| variable | required | maps to / default |
|---|---|---|
| `H2_LEASE` | yes | `--lease` |
| `H2_LIVE` | yes | `--live` |
| `H2_TOKEN` | yes | `--token` (never logged) |
| `H2_DATA_URL` | yes | `--data-url` (signed; never logged) |
| `H2_DATA_SHA` | yes | `--data-sha` |
| `H2_REGION` | no | `--region` |
| `H2_AGENT` | no | agent socket, default `tcp://0.0.0.0:31100` |
| `H2_XNET_ADDR` | no | game UDP bind; unset picks this machine's address (must be unicast) |
| `H2_IMAGE` | no | staged module dir, default `/opt/h2/image` |
| `H2_HOST_CACHE` | no | bundle cache, default `/var/cache/h2` (mount a volume here) |
| `H2_DATA` | no | game data root, default `/var/cache/h2/data` |

Ports: `31000-31006/udp` are the game's (base 31000; channels +0/+1/+5/+6,
QoS +2, co-op +4), `31100/tcp` the agent socket. There is no `HEALTHCHECK`
and no restart policy — a host is ad hoc, one lease per container, and the
provider that started it (Durable Object, Fly Machine, `docker run` above)
owns liveness and stop.

## Keep the image private

The module inside is compiled from your disc. The image is for you and your
registry only: push it to Cloudflare's container registry for the account or
Fly's private registry, both of which serve the image to your own deploys
alone — never to a public registry. The same rights questions as the rest of
the project apply to a hosted fleet ([dedicated-host.md][rights]).

[rights]: ../../docs/live/dedicated-host.md
