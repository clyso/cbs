#!/bin/sh

[ $# -lt 1 ] && echo "error: missing arguments ('worker', 'server')" >&2 && exit 1

if ! /usr/bin/uv --help >/dev/null 2>&1; then
  echo "error: uv is not installed. Please install uv to proceed."
  exit 1
fi

uv sync --all-packages --no-dev --no-cache || (
  echo "error: 'uv sync' command failed." >&2 && exit 1
)

cd /cbs/server/cbsd || (
  echo "error: failed to change directory to /cbs/server/cbsd." >&2 && exit 1
)

if [ "${1}" = "server" ]; then
  uv run --no-sync \
    uvicorn --factory --host 0.0.0.0 --port 8080 \
    --ssl-keyfile /cbs/config/cbs.key.pem \
    --ssl-certfile /cbs/config/cbs.cert.pem \
    cbs-server:factory || (echo "error: failed to start server." >&2 && exit 1)

elif [ "${1}" = "worker" ]; then
  uv run --no-sync \
    celery -A cbslib.worker worker \
    -E --loglevel=info --concurrency=1 || (echo "error: failed to start worker." >&2 && exit 1)

else
  echo "error: invalid argument '${1}'. Expected 'worker' or 'server'." >&2
  exit 1
fi
