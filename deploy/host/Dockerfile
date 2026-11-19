# syntax=docker/dockerfile:1.7
# The game module is compiled from the owner's disc. Use a private registry.
FROM --platform=linux/amd64 debian:bookworm-slim

RUN set -eux; \
    apt-get update; \
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates; \
    rm -rf /var/lib/apt/lists/*; \
    useradd --system --uid 1000 --user-group \
      --home-dir /var/cache/h2 --create-home --shell /usr/sbin/nologin h2

COPY stage/ /opt/h2/
COPY --chmod=0755 entrypoint.sh /opt/h2/entrypoint.sh
RUN test -x /opt/h2/h2 && test -s /opt/h2/image/libh2game.so \
    && test -s /opt/h2/image/libh2game.so.stamp

EXPOSE 31000-31006/udp
EXPOSE 31100/tcp

USER h2
WORKDIR /var/cache/h2
ENTRYPOINT ["/opt/h2/entrypoint.sh"]
