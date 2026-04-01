#!/bin/bash
# CBS build service daemon (cbsd-rs) — production image builder
# Copyright (C) 2026  Clyso
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# Builds production container images for cbsd-rs server, worker, and UI.
# Embeds the current git SHA into the binaries via --build-arg.
#
# Usage (from repository root):
#   ./container/build-cbsd-rs.sh server
#   ./container/build-cbsd-rs.sh worker
#   ./container/build-cbsd-rs.sh ui
#   ./container/build-cbsd-rs.sh all

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

[[ ! -e "${REPO_ROOT}/.git" ]] && {
  echo "error: must be run from repository root" >&2
  exit 1
}

if ! podman --version >/dev/null 2>&1; then
  echo "error: podman is not installed" >&2
  exit 1
fi

CONTAINERFILE="${SCRIPT_DIR}/ContainerFile.cbsd-rs"
GIT_VERSION="$(git -C "${REPO_ROOT}" describe --always --match='' 2>/dev/null || echo unknown)"

registry="harbor.clyso.com"
image_org="cbs"
server_image=
worker_image=
ui_image=

usage() {
  cat <<EOF >&2
usage: $0 TARGET [options...]

Targets:
  server    Build the production server image
  worker    Build the production worker image
  ui        Build the production UI image
  all       Build server, worker, and UI images

Options:
  --tag TAG               Image tag (default: git describe output)
  -r|--registry URL       Container registry (default: ${registry})
  --server-image IMAGE    Override server image name (default: ${image_org}/cbsd-rs-server)
  --worker-image IMAGE    Override worker image name (default: ${image_org}/cbsd-rs-worker)
  --ui-image IMAGE        Override UI image name (default: ${image_org}/cbsd-rs-ui)
  --push                  Push images after building
  --update-latest         Also tag and push images as :latest
  --force-rebuild         Force rebuild without layer cache
  -h|--help               Show this help
EOF
}

TAG="${GIT_VERSION}"
PUSH=0
UPDATE_LATEST=0
FORCE_REBUILD=0
TARGETS=()

while [[ $# -gt 0 ]]; do
  case $1 in
    server|worker|ui|all)
      TARGETS+=("$1")
      ;;
    --tag)
      TAG="${2:?--tag requires a value}"
      shift
      ;;
    -r|--registry)
      registry="${2:?--registry requires a value}"
      shift
      ;;
    --server-image)
      server_image="${2:?--server-image requires a value}"
      shift
      ;;
    --worker-image)
      worker_image="${2:?--worker-image requires a value}"
      shift
      ;;
    --ui-image)
      ui_image="${2:?--ui-image requires a value}"
      shift
      ;;
    --push)
      PUSH=1
      ;;
    --update-latest)
      UPDATE_LATEST=1
      ;;
    --force-rebuild)
      FORCE_REBUILD=1
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

# Resolve image names: override > default (org/cbsd-rs-<target>)
resolve_image_name() {
  local target="$1"
  local override=

  case "${target}" in
    server) override="${server_image}" ;;
    worker) override="${worker_image}" ;;
    ui)     override="${ui_image}" ;;
  esac

  if [[ -n "${override}" ]]; then
    echo "${override}"
  else
    echo "${image_org}/cbsd-rs-${target}"
  fi
}

build_image() {
  local target="$1"
  local image_name
  image_name="$(resolve_image_name "${target}")"
  local image_tagged="${registry}/${image_name}:${TAG}"
  local image_latest="${registry}/${image_name}:latest"

  local no_cache_arg=()
  [[ ${FORCE_REBUILD} -eq 1 ]] && no_cache_arg=("--no-cache")

  echo "=> Building ${image_tagged} (git: ${GIT_VERSION})"
  podman build \
    -f "${CONTAINERFILE}" \
    --target "cbsd-rs-${target}" \
    --build-arg "GIT_VERSION=${GIT_VERSION}" \
    "${no_cache_arg[@]}" \
    -t "${image_tagged}" \
    "${REPO_ROOT}" || {
    echo "error: failed to build image '${image_tagged}'" >&2
    exit 1
  }

  echo "=> Built ${image_tagged}"

  if [[ ${UPDATE_LATEST} -eq 1 ]]; then
    echo "=> Tagging ${image_latest}"
    podman tag "${image_tagged}" "${image_latest}" || {
      echo "error: failed to tag image '${image_latest}'" >&2
      exit 1
    }
  fi

  if [[ ${PUSH} -eq 1 ]]; then
    echo "=> Pushing ${image_tagged}"
    podman push "${image_tagged}" || {
      echo "error: failed to push image '${image_tagged}'" >&2
      exit 1
    }

    if [[ ${UPDATE_LATEST} -eq 1 ]]; then
      echo "=> Pushing ${image_latest}"
      podman push "${image_latest}" || {
        echo "error: failed to push image '${image_latest}'" >&2
        exit 1
      }
    fi
  fi
}

# Verify registry login before attempting to push
if [[ ${PUSH} -eq 1 ]]; then
  echo "=> Checking login to registry '${registry}'"
  if ! podman login --get-login "${registry}" >/dev/null 2>&1; then
    echo "error: not logged in to '${registry}'" >&2
    exit 1
  fi
fi

for target in "${TARGETS[@]}"; do
  if [[ "${target}" == "all" ]]; then
    build_image server
    build_image worker
    build_image ui
  else
    build_image "${target}"
  fi
done
