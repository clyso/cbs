#!/bin/bash

PODMAN_COMPOSE_PROVIDER="podman-compose" podman compose \
  -f ./podman-compose.cbs.yaml down

[[ ${1} == "down" ]] && exit 0

PODMAN_COMPOSE_PROVIDER="podman-compose" podman compose --verbose \
  --podman-run-args="--rm" -f ./podman-compose.cbs.yaml up --build
