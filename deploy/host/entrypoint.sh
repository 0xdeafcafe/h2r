#!/bin/sh
set -eu

: "${H2_LEASE:?H2_LEASE (lease ID) is required}"
: "${H2_LIVE:?H2_LIVE (Live URL) is required}"
: "${H2_TOKEN:?H2_TOKEN (lease credential) is required}"
: "${H2_DATA_URL:?H2_DATA_URL (signed bundle URL) is required}"
: "${H2_DATA_SHA:?H2_DATA_SHA (bundle SHA-256) is required}"

export H2_IMAGE="${H2_IMAGE:-/opt/h2/image}"
export H2_HOST_CACHE="${H2_HOST_CACHE:-/var/cache/h2}"
mkdir -p "$H2_HOST_CACHE"

set -- host run \
  --lease "$H2_LEASE" \
  --live "$H2_LIVE" \
  --token "$H2_TOKEN" \
  --data-url "$H2_DATA_URL" \
  --data-sha "$H2_DATA_SHA"
if [ -n "${H2_REGION:-}" ]; then
  set -- "$@" --region "$H2_REGION"
fi

exec /opt/h2/h2 "$@"
