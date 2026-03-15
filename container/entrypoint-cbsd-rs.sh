#!/bin/sh
# CBS build service daemon (cbsd-rs) — unified container entrypoint
#
# Copyright (C) 2026  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# Usage: entrypoint-cbsd-rs.sh server|worker|server-dev|worker-dev
#
# server      run cbsd-server (production binary)
# worker      run cbsd-worker (production binary)
# server-dev  run cbsd-server under cargo-watch (auto-reload on source changes)
# worker-dev  run cbsd-worker under cargo-watch (auto-reload on source changes)
#
# Dev modes require the cbsd-rs workspace to be bind-mounted at /cbs/src
# and the .sqlx/ offline query cache to be present (run
# 'cargo sqlx prepare --workspace' on the host first).
#
# Environment:
#   CBSD_CONFIG   path to the YAML config file
#                 (default: /cbs/config/server.yaml or /cbs/config/worker.yaml)
#   RUST_LOG      log filter passed through to the binary

set -e

[ $# -lt 1 ] && {
    echo "error: expected 'server', 'worker', 'server-dev', or 'worker-dev'" >&2
    exit 1
}

mode="$1"

case "${mode}" in
    server)
        exec cbsd-server --config "${CBSD_CONFIG:-/cbs/config/server.yaml}"
        ;;
    worker)
        exec cbsd-worker --config "${CBSD_CONFIG:-/cbs/config/worker.yaml}"
        ;;
    server-dev)
        # Use the committed .sqlx/ offline query cache — no live DB at build time.
        export SQLX_OFFLINE=true
        config="${CBSD_CONFIG:-/cbs/config/server.yaml}"
        exec cargo watch \
            --workdir /cbs/src \
            -w cbsd-server/src \
            -w cbsd-proto/src \
            -x "run --bin cbsd-server -- --config ${config}"
        ;;
    worker-dev)
        export SQLX_OFFLINE=true
        config="${CBSD_CONFIG:-/cbs/config/worker.yaml}"
        exec cargo watch \
            --workdir /cbs/src \
            -w cbsd-worker/src \
            -w cbsd-proto/src \
            -x "run --bin cbsd-worker -- --config ${config}"
        ;;
    *)
        echo "error: unknown mode '${mode}'; expected 'server', 'worker', 'server-dev', or 'worker-dev'" >&2
        exit 1
        ;;
esac
