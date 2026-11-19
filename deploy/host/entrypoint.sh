#!/bin/sh
# Container entrypoint for the dedicated host (docs/live/dedicated-host.md,
# "Container image"): maps the provider's environment to `h2 host run`
# (contract 9) and execs it, so the game process is the container's main
# process and receives its stop signal directly. The host is ad hoc — started
# for one lease, exits when the match ends — so there is no health check and
# no restart policy here; the provider that started the container owns both.
#
# Required (set by the provider starting the container):
#   H2_LEASE      the lease id this host serves        (--lease)
#   H2_LIVE       where it registers, e.g. https://live.forbes.red (--live)
#   H2_TOKEN      the host credential for this lease   (--token)
#   H2_DATA_URL   signed URL of the owner's game-data bundle (--data-url)
#   H2_DATA_SHA   that bundle's sha256                 (--data-sha)
#
# Optional:
#   H2_REGION     provider region code                 (--region)
#   H2_AGENT      agent socket for diagnostics (default tcp://0.0.0.0:31100,
#                 the port the image EXPOSEs on tcp)
#   H2_XNET_ADDR  the game's UDP bind, IPv4:port (default: the HLE picks this
#                 machine's address; must be unicast, so 0.0.0.0 is refused —
#                 set it only if that pick is wrong in your network). Base
#                 31000 uses the UDP ports the image EXPOSEs, 31000-31006.
#   H2_IMAGE      the staged module dir (default /opt/h2/image)
#   H2_HOST_CACHE bundle cache, kept between containers (default /var/cache/h2)
#   H2_DATA       the game's data root (default $H2_HOST_CACHE/data)
set -eu

: "${H2_LEASE:?H2_LEASE (the lease id this host serves) is required}"
: "${H2_LIVE:?H2_LIVE (where the host registers) is required}"
: "${H2_TOKEN:?H2_TOKEN (the host credential for this lease) is required}"
: "${H2_DATA_URL:?H2_DATA_URL (the signed game-data bundle URL) is required}"
: "${H2_DATA_SHA:?H2_DATA_SHA (the game-data bundle sha256) is required}"

H2_BIN=${H2_BIN:-/opt/h2/h2}
H2_IMAGE=${H2_IMAGE:-/opt/h2/image}
H2_HOST_CACHE=${H2_HOST_CACHE:-/var/cache/h2}
H2_DATA=${H2_DATA:-$H2_HOST_CACHE/data}
H2_AGENT=${H2_AGENT:-tcp://0.0.0.0:31100}
export H2_IMAGE H2_HOST_CACHE H2_DATA H2_AGENT

mkdir -p "$H2_HOST_CACHE" "$H2_DATA"

# Never log H2_TOKEN or H2_DATA_URL: the token is the host's credential and
# the data URL is signed for this host.
echo "h2 host: lease $H2_LEASE at $H2_LIVE, data sha256 $H2_DATA_SHA, cache $H2_HOST_CACHE"

set -- host run \
  --lease "$H2_LEASE" \
  --live "$H2_LIVE" \
  --token "$H2_TOKEN" \
  --data-url "$H2_DATA_URL" \
  --data-sha "$H2_DATA_SHA"
if [ -n "${H2_REGION:-}" ]; then
  set -- "$@" --region "$H2_REGION"
fi

exec "$H2_BIN" "$@"
