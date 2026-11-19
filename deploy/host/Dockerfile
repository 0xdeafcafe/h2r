# syntax=docker/dockerfile:1.7
#
# The dedicated host (docs/live/dedicated-host.md, "Container image"): the
# Linux x86-64 `h2` in the host profile and the game module deploy/host/
# build.sh compiled from the OWNER'S OWN disc. No game data inside: maps and
# shared disc data are fetched at start from the owner's private bundle
# (H2_DATA_URL / H2_DATA_SHA) and cached under /var/cache/h2.
#
# PRIVATE ONLY. The module is compiled from the owner's disc, so the image is
# theirs alone: push it only to a private registry (Cloudflare's container
# registry for the account, or Fly's private registry), never a public one.
#
# Build after build.sh has staged stage/ (see README.md), from this directory:
#   docker build -t h2host:local .
# or from the repository root:
#   docker build -f deploy/host/Dockerfile -t h2host:local deploy/host
#
# The platform is pinned: the staged module is x86-64 ELF, so the image is
# linux/amd64 only (arm64 hosts run it through Rosetta/QEMU emulation).

FROM --platform=linux/amd64 debian:bookworm-slim

# ca-certificates for the HTTPS fetch of the game-data bundle; nothing else.
# glibc 2.36 in bookworm matches the .2.36 cargo-zigbuild pins by default.
# The h2 user owns /var/cache/h2 (the bundle cache kept between containers,
# a volume when the provider mounts one).
RUN set -eux; \
    apt-get update; \
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates; \
    rm -rf /var/lib/apt/lists/*; \
    useradd --system --uid 1000 --user-group \
      --home-dir /var/cache/h2 --create-home --shell /usr/sbin/nologin h2

COPY stage/ /opt/h2/
COPY entrypoint.sh /opt/h2/entrypoint.sh
RUN chmod 0755 /opt/h2/entrypoint.sh /opt/h2/h2

# The game's UDP ports (base 31000: channels +0/+1/+5/+6, QoS +2, co-op +4)
# and the agent socket's TCP port. Ad hoc lifecycle: no HEALTHCHECK, no
# restart — the provider that starts the container owns liveness and stop.
EXPOSE 31000-31006/udp
EXPOSE 31100/tcp

USER h2
WORKDIR /var/cache/h2

ENTRYPOINT ["/opt/h2/entrypoint.sh"]
