#!/bin/bash
# CBS build service daemon (cbsd-rs) — production image builder
# Copyright (C) 2026  Clyso
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# Builds production container images for cbsd-rs server and worker.
# Embeds the current git SHA into the binaries via --build-arg.
#
# Usage (from repository root):
#   ./container/build-cbsd-rs.sh server
#   ./container/build-cbsd-rs.sh worker
#   ./container/build-cbsd-rs.sh all

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

CONTAINERFILE="${SCRIPT_DIR}/ContainerFile.cbsd-rs"
GIT_VERSION="$(git -C "${REPO_ROOT}" describe --always --match='' 2>/dev/null || echo unknown)"

# Default image name prefix
IMAGE_PREFIX="${CBS_IMAGE_PREFIX:-cbsd-rs}"

usage() {
    cat <<EOF >&2
usage: $0 TARGET [options...]

Targets:
  server    Build the production server image
  worker    Build the production worker image
  all       Build both server and worker images

Options:
  --tag TAG     Image tag (default: latest)
  --push        Push images after building
  -h|--help     Show this help
EOF
}

TAG="latest"
PUSH=0
TARGETS=()

while [[ $# -gt 0 ]]; do
    case $1 in
        server|worker|all)
            TARGETS+=("$1")
            ;;
        --tag)
            TAG="${2:?--tag requires a value}"
            shift
            ;;
        --push)
            PUSH=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            usage
            exit 1
            ;;
    esac
    shift
done

if [[ ${#TARGETS[@]} -eq 0 ]]; then
    echo "error: no target specified" >&2
    usage
    exit 1
fi

build_image() {
    local target="$1"
    local image_name="${IMAGE_PREFIX}-${target}:${TAG}"

    echo "=> Building ${image_name} (git: ${GIT_VERSION})"
    podman build \
        -f "${CONTAINERFILE}" \
        --target "cbsd-rs-${target}" \
        --build-arg "GIT_VERSION=${GIT_VERSION}" \
        -t "${image_name}" \
        "${REPO_ROOT}"

    echo "=> Built ${image_name}"

    if [[ ${PUSH} -eq 1 ]]; then
        echo "=> Pushing ${image_name}"
        podman push "${image_name}"
    fi
}

for target in "${TARGETS[@]}"; do
    if [[ "${target}" == "all" ]]; then
        build_image server
        build_image worker
    else
        build_image "${target}"
    fi
done
