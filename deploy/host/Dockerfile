# syntax=docker/dockerfile:1.7
# The game module is compiled from the owner's disc. Use a private registry.
FROM --platform=linux/amd64 debian:bookworm-slim AS rootfs

RUN set -eux; \
    apt-get update; \
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates libgcc-s1; \
    useradd --system --uid 1000 --user-group \
      --home-dir /var/cache/h2 --no-create-home --shell /usr/sbin/nologin h2; \
    install -d -m 0755 -o 1000 -g 1000 /rootfs/var/cache/h2; \
    install -d -m 1777 /rootfs/tmp; \
    install -d /rootfs/bin /rootfs/etc/ssl/certs; \
    install -m 0755 /bin/dash /rootfs/bin/sh; \
    cp /etc/passwd /etc/group /etc/nsswitch.conf /rootfs/etc/; \
    cp /etc/ssl/certs/ca-certificates.crt /rootfs/etc/ssl/certs/

COPY stage/h2 /rootfs/opt/h2/h2
COPY stage/image/libh2game.so stage/image/libh2game.so.stamp /rootfs/opt/h2/image/
COPY --chmod=0755 entrypoint.sh /rootfs/opt/h2/entrypoint.sh

# Copy the ELF interpreter and the transitive libraries of both artifacts
# and the entrypoint shell. Keep absolute loader paths, including symlinks'
# targets as regular files. Docker provides resolv.conf and hosts at run time.
RUN set -eux; \
    test -x /rootfs/opt/h2/h2; \
    test -s /rootfs/opt/h2/image/libh2game.so.stamp; \
    ldd /rootfs/opt/h2/h2 > /elf-deps; \
    ldd /rootfs/opt/h2/image/libh2game.so >> /elf-deps; \
    ldd /rootfs/bin/sh >> /elf-deps; \
    if grep -F 'not found' /elf-deps; then exit 1; fi; \
    awk '$1 ~ /^\// {print $1} $2 == "=>" && $3 ~ /^\// {print $3}' /elf-deps | sort -u > /elf-paths; \
    while IFS= read -r path; do cp -L --parents "$path" /rootfs; done < /elf-paths; \
    cp -L --parents /lib/x86_64-linux-gnu/libnss_files.so.2 /lib/x86_64-linux-gnu/libnss_dns.so.2 /rootfs

FROM scratch
COPY --from=rootfs /rootfs/ /
EXPOSE 31000-31069/udp
EXPOSE 31099-31109/tcp
USER 1000:1000
WORKDIR /var/cache/h2
ENTRYPOINT ["/opt/h2/entrypoint.sh"]
