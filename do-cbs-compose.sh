#!/bin/bash

PODMAN_COMPOSE_PROVIDER="podman-compose" podman compose \
	-f ./podman-compose.cbs.yaml down
PODMAN_COMPOSE_PROVIDER="podman-compose" podman compose --verbose \
	--podman-run-args="--rm" -f ./podman-compose.cbs.yaml up --build
