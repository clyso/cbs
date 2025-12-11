#!/bin/bash

# CBS - Clyso Build System
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.

[[ ! -e ".git" ]] &&
  echo "error: must be run from repository root" >/dev/stderr && exit 1

if ! podman --version >/dev/null 2>&1; then
  echo "error: podman is not installed" >/dev/stderr
  exit 1
fi

repo_tag="$(git rev-parse --short HEAD 2>/dev/null)"
[[ -z "${repo_tag}" ]] &&
  echo "error: failed to determine git commit hash" >/dev/stderr &&
  exit 1

server_image="cbs/cbsd-server"
worker_image="cbs/cbsd-worker"
registry="harbor.clyso.com"

server_image_tagged="${server_image}:${repo_tag}"
worker_image_tagged="${worker_image}:${repo_tag}"

usage() {
  cat <<EOF >/dev/stderr
usage: $0 [options...]

Options:
  -s|--server IMAGE:TAG     Specify the server image (default: ${server_image_tagged})
  -w|--worker IMAGE:TAG     Specify the worker image  (default: ${worker_image_tagged})
  -r|--registry URL         Specify the container registry (default: ${registry})

  --no-server               Do not build the server image (default: false)
  --no-worker               Do not build the worker image (default: false)
  --push                    Push to registry (default: false)
  --update-latest           Update 'latest' tags in the registry (default: false)
  --force-rebuild           Force rebuild of all intermediate layers (default: false)

EOF
}

build_server=1
build_worker=1
push=0
update_latest=0
force_rebuild=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    -s | --server)
      [[ -z $2 ]] &&
        echo "error: missing argument for '--server'" >/dev/stderr &&
        usage &&
        exit 1
      server_image_tagged="${2}"
      shift 1
      ;;
    -w | --worker)
      [[ -z $2 ]] &&
        echo "error: missing argument for '--worker'" >/dev/stderr &&
        usage &&
        exit 1
      worker_image_tagged="${2}"
      shift 1
      ;;
    -r | --registry)
      [[ -z $2 ]] &&
        echo "error: missing argument for '--registry'" >/dev/stderr &&
        usage &&
        exit 1
      registry="${2}"
      shift 1
      ;;
    --no-server)
      build_server=0
      ;;
    --no-worker)
      build_worker=0
      ;;
    --push)
      push=1
      ;;
    --update-latest)
      update_latest=1
      ;;
    --force-rebuild)
      force_rebuild=1
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: '${1}'" >/dev/stderr
      usage
      exit 1
      ;;
  esac
  shift 1
done

[[ ${build_server} -eq 0 && ${build_worker} -eq 0 ]] &&
  echo "error: at least one of '--no-server' or '--no-worker' must be omitted" >/dev/stderr &&
  usage &&
  exit 1

build_image() {
  target="${1}"
  [[ -z "${target}" ]] &&
    echo "error: missing build target" >/dev/stderr &&
    exit 1

  image_dst="${2}"
  [[ -z "${image_dst}" ]] &&
    echo "error: missing image name" >/dev/stderr &&
    exit 1

  no_cache_arg=""
  [[ $force_rebuild -eq 1 ]] && no_cache_arg="--no-cache"

  if ! podman build -f ./container/ContainerFile.cbsd \
    --target "${target}" ${no_cache_arg} \
    --tag "${image_dst}" \
    .; then
    echo "error: failed to build image '${image_dst}'" >/dev/stderr
    exit 1
  fi
}

server_image_dst="${registry}/${server_image_tagged}"
worker_image_dst="${registry}/${worker_image_tagged}"

server_image_latest="${registry}/${server_image}:latest"
worker_image_latest="${registry}/${worker_image}:latest"

if [[ ${build_server} -eq 1 ]]; then
  echo "building server image: ${server_image_dst}"
  build_image "cbsd-server" "${server_image_dst}"
fi

if [[ ${build_worker} -eq 1 ]]; then
  echo "building worker image: ${worker_image_dst}"
  build_image "cbsd-worker" "${worker_image_dst}"
fi

if [[ $update_latest -eq 1 ]]; then
  if [[ ${build_server} -eq 1 ]]; then
    echo "tagging server image as latest: ${server_image_latest}"
    podman tag "${server_image_dst}" "${server_image_latest}" || (
      echo "error: failed to tag server image '${server_image_latest}'" >/dev/stderr &&
        exit 1
    )
  fi

  if [[ ${build_worker} -eq 1 ]]; then
    echo "tagging worker image as latest: ${worker_image_latest}"
    podman tag "${worker_image_dst}" "${worker_image_latest}" || (
      echo "error: failed to tag worker image '${worker_image_latest}'" >/dev/stderr &&
        exit 1
    )
  fi
fi

if [[ ${push} -eq 1 ]]; then
  echo "checking login to registry '${registry}'"
  if ! podman login --get-login "${registry}" >/dev/null 2>&1; then
    echo "error: not logged in to '${registry}'" >/dev/stderr
    exit 1
  fi

  if [[ ${build_server} -eq 1 ]]; then
    echo "pushing server image: ${server_image_dst}"
    podman push "${server_image_dst}" || (
      echo "error: failed to push server image '${server_image_dst}'" >/dev/stderr &&
        exit 1
    )

    if [[ ${update_latest} -eq 1 ]]; then
      echo "pushing server image latest tag: ${server_image_latest}"
      podman push "${server_image_latest}" || (
        echo "error: failed to push server image latest tag '${server_image_latest}'" >/dev/stderr &&
          exit 1
      )
    fi
  fi

  if [[ ${build_worker} -eq 1 ]]; then
    echo "pushing worker image: ${worker_image_dst}"
    podman push "${worker_image_dst}" || (
      echo "error: failed to push worker image '${worker_image_dst}'" >/dev/stderr &&
        exit 1
    )

    if [[ $update_latest -eq 1 ]]; then
      echo "pushing worker image latest tag: ${worker_image_latest}"
      podman push "${worker_image_latest}" || (
        echo "error: failed to push worker image latest tag '${worker_image_latest}'" >/dev/stderr &&
          exit 1
      )
    fi
  fi

fi
