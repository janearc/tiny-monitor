#!/usr/bin/env bash
# run-tiny-monitor.sh -- launch the tiny-monitor floating widget against the live
# obs-svc-agg aggregator.
#
# obs-svc-agg serves its snapshot on the container's internal :8090, but on this
# host :8090 is already held by another process, so the container publishes 8090
# on an ephemeral host port. this script resolves that published port (so you
# don't have to look it up), points the widget at it, and runs the release build.
#
# overrides:
#   OBS_AGG_URL=http://host:port/state   skip discovery, use this endpoint
#   OBS_AGG_CONTAINER=name               container to inspect (default obs-svc-agg)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here"

if [ -n "${OBS_AGG_URL:-}" ]; then
  # caller gave an explicit endpoint; trust it.
  url="$OBS_AGG_URL"
else
  # discover the host port the agg container publishes for its internal 8090.
  container="${OBS_AGG_CONTAINER:-obs-svc-agg}"
  port="$(docker port "$container" 8090 2>/dev/null | grep -oE '[0-9]+$' | head -1 || true)"
  if [ -z "$port" ]; then
    echo "run-obs-apple: no published :8090 found for container '$container'." >&2
    echo "  check it is up:   docker ps | grep obs-svc-agg" >&2
    echo "  or set OBS_AGG_URL=http://host:port/state and re-run." >&2
    exit 1
  fi
  url="http://127.0.0.1:${port}/state"
fi

echo "run-obs-apple: aggregator -> ${url}"
# build is a fast no-op when up to date; exec so Ctrl-C reaches the widget.
exec env OBS_AGG_URL="${url}" cargo run --release -- "$@"
