#!/bin/bash

# CBS build service daemon (cbsd-rs) — container lifecycle script
# Copyright (C) 2026  Clyso
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
#
# Invoked by the cbsd-rs-<deployment>@<service>.service systemd unit.
# Handles start/stop of server and worker containers via podman.

usage() {
  cat <<EOF >&2
usage: $0 ACTION DEPLOYMENT SERVICE [options...]

Actions:
  start     Start specified component
  stop      Stop specified component

Services:
  server    cbsd-rs server
  worker    cbsd-rs worker (may be named, e.g. worker.host-01)

Options:
  -c|--config DIR       Directory for per-deployment configuration files
  -d|--data DIR         Directory for per-deployment data files
  -h|--help             Show this help message and exit
EOF
}

config_dir="${HOME}/.config/cbsd-rs"
data_dir="${HOME}/.local/share/cbsd-rs"

positional_args=()

while [[ $# -gt 0 ]]; do
  case $1 in
    -h | --help)
      usage
      exit 0
      ;;
    -c | --config)
      [[ -z $2 ]] && {
        echo "error: '--config' requires an argument" >&2
        usage
        exit 1
      }
      config_dir="${2}"
      shift 1
      ;;
    -d | --data)
      [[ -z $2 ]] && {
        echo "error: '--data' requires an argument" >&2
        usage
        exit 1
      }
      data_dir="${2}"
      shift 1
      ;;
    -*)
      echo "error: unknown option: ${1}" >&2
      usage
      exit 1
      ;;
    *)
      positional_args+=("${1}")
      ;;
  esac
  shift 1
done

[[ ${#positional_args[@]} -lt 3 ]] && {
  echo "error: ACTION, DEPLOYMENT, and SERVICE must be specified" >&2
  usage
  exit 1
}

action="${positional_args[0]}"
deployment_name="${positional_args[1]}"
service="${positional_args[2]}"

[[ "${action}" != "start" && "${action}" != "stop" ]] && {
  echo "error: unknown action: ${action}" >&2
  usage
  exit 1
}

# Strip the 'cbsd-rs-' prefix to get the bare deployment name.
# The systemd unit prefix is 'cbsd-rs-<deployment>', so %p produces
# 'cbsd-rs-default' for the 'default' deployment.
real_deployment_name="${deployment_name#cbsd-rs-}"
[[ -z "${real_deployment_name}" || "${real_deployment_name}" == "${deployment_name}" ]] && {
  echo "error: invalid deployment name: ${deployment_name}" >&2
  usage
  exit 1
}
deployment_name="${real_deployment_name}"

service_type="${service%%.*}"
[[ -z "${service_type}" ]] && {
  echo "error: invalid service name '${service}'" >&2
  usage
  exit 1
}
service_id="${service#*.}"
[[ "${service_id}" == "${service}" ]] && {
  service_id=""
}

ctr_name="cbsd-rs-${service_type}.${deployment_name}"
ctr_name+="${service_id:+.${service_id}}"

echo "run '${action}' for '${ctr_name}'"
echo "  deployment:     ${deployment_name}"
echo "  service name:   ${service}"
echo "  service type:   ${service_type}"
echo "  service id:     ${service_id}"
echo "  container name: ${ctr_name}"

source_config() {
  [[ -e "${config_dir}/${deployment_name}/${service}.conf" ]] && {
    # shellcheck source=/dev/null
    source "${config_dir}/${deployment_name}/${service}.conf"
  }
}

# --------------------------------------------------------------------------
# Server
# --------------------------------------------------------------------------

server_start() {
  echo "starting server '${ctr_name}'..."

  server_config_dir="${config_dir}/${deployment_name}/${service}"
  server_data_dir="${data_dir}/${deployment_name}/${service}"
  server_logs_dir="${data_dir}/${deployment_name}/${service}/logs"
  components_dir="${data_dir}/${deployment_name}/components"

  SERVER_BIND_ADDR="127.0.0.1"
  SERVER_BIND_PORT="8080"
  RUST_LOG="info"
  IMAGE="harbor.clyso.com/cbs/cbsd-rs-server:latest"
  source_config

  [[ ! -d "${server_config_dir}" ]] && {
    echo "error: server config directory '${server_config_dir}' does not exist" >&2
    exit 1
  }

  [[ ! -d "${server_data_dir}" ]] && {
    mkdir -p "${server_data_dir}" || {
      echo "error: failed to create server data directory '${server_data_dir}'" >&2
      exit 1
    }
  }

  [[ ! -d "${server_logs_dir}" ]] && {
    mkdir -p "${server_logs_dir}" || {
      echo "error: failed to create server logs directory '${server_logs_dir}'" >&2
      exit 1
    }
  }

  # Components directory is optional — server starts without it (with a warning).
  components_vol=""
  [[ -d "${components_dir}" ]] && {
    components_vol="-v ${components_dir}:/cbs/components:ro"
  }

  # shellcheck disable=SC2086
  podman run \
    -d \
    --replace \
    -p "${SERVER_BIND_ADDR}":"${SERVER_BIND_PORT}":8080 \
    -v "${server_config_dir}":/cbs/config:ro \
    -v "${server_data_dir}":/cbs/data:Z \
    -v "${server_logs_dir}":/cbs/logs:Z \
    ${components_vol} \
    -e CBSD_CONFIG=/cbs/config/server.yaml \
    -e RUST_LOG="${RUST_LOG}" \
    --security-opt label=disable \
    --security-opt seccomp=unconfined \
    --network "cbsd-rs-${deployment_name}" \
    --name "${ctr_name}" \
    "${IMAGE}" || {
    echo "error: failed to start server '${ctr_name}'" >&2
    exit 1
  }
}

server_stop() {
  echo "stopping server '${ctr_name}'..."
  podman stop "${ctr_name}" || {
    echo "error: failed to stop server '${ctr_name}'" >&2
    exit 1
  }
}

# --------------------------------------------------------------------------
# Worker
# --------------------------------------------------------------------------

worker_start() {
  echo "starting worker '${ctr_name}'..."

  worker_config_dir="${config_dir}/${deployment_name}/${service}"
  worker_components_dir="${data_dir}/${deployment_name}/components"
  worker_logs_dir="${data_dir}/${deployment_name}/${service}/logs"

  WORKER_SCRATCH_DIR=
  WORKER_CONTAINERS_DIR=
  WORKER_CCACHE_DIR=
  RUST_LOG="info"
  IMAGE="harbor.clyso.com/cbs/cbsd-rs-worker:latest"
  source_config

  [[ ! -d "${worker_config_dir}" ]] && {
    echo "error: worker config directory '${worker_config_dir}' does not exist" >&2
    exit 1
  }

  [[ ! -d "${worker_components_dir}" ]] && {
    echo "error: worker components directory '${worker_components_dir}' does not exist" >&2
    exit 1
  }

  [[ ! -d "${worker_logs_dir}" ]] && {
    mkdir -p "${worker_logs_dir}" || {
      echo "error: failed to create worker logs directory '${worker_logs_dir}'" >&2
      exit 1
    }
  }

  [[ ! -d "${WORKER_SCRATCH_DIR}" ]] && {
    mkdir -p "${WORKER_SCRATCH_DIR}" || {
      echo "error: failed to create worker scratch directory '${WORKER_SCRATCH_DIR}'" >&2
      exit 1
    }
  }

  [[ ! -d "${WORKER_CONTAINERS_DIR}" ]] && {
    mkdir -p "${WORKER_CONTAINERS_DIR}" || {
      echo "error: failed to create worker containers directory '${WORKER_CONTAINERS_DIR}'" >&2
      exit 1
    }
  }

  [[ ! -d "${WORKER_CCACHE_DIR}" ]] && {
    mkdir -p "${WORKER_CCACHE_DIR}" || {
      echo "error: failed to create worker ccache directory '${WORKER_CCACHE_DIR}'" >&2
      exit 1
    }
  }

  # shellcheck disable=SC2086
  podman run \
    -d \
    --replace \
    -v "${worker_config_dir}":/cbs/config:ro \
    -v "${worker_logs_dir}":/cbs/logs:Z \
    -v "${WORKER_SCRATCH_DIR}":/cbs/scratch:Z \
    -v "${WORKER_CONTAINERS_DIR}":/var/lib/containers:Z \
    -v "${WORKER_CCACHE_DIR}":/cbs/ccache:Z \
    -v "${worker_components_dir}":/cbs/components:ro \
    -v /dev/fuse:/dev/fuse:rw \
    -e CBSD_CONFIG=/cbs/config/worker.yaml \
    -e RUST_LOG="${RUST_LOG}" \
    --cap-add SYS_ADMIN \
    --cap-add MKNOD \
    --security-opt label=disable \
    --security-opt seccomp=unconfined \
    --privileged \
    --network "cbsd-rs-${deployment_name}" \
    --name "${ctr_name}" \
    "${IMAGE}" || {
    echo "error: failed to start worker '${ctr_name}'" >&2
    exit 1
  }
}

worker_stop() {
  echo "stopping worker '${ctr_name}'..."
  podman stop "${ctr_name}" || {
    echo "error: failed to stop worker '${ctr_name}'" >&2
    exit 1
  }
}

# --------------------------------------------------------------------------
# Dispatch
# --------------------------------------------------------------------------

fname=

case "${service_type}" in
  server)
    fname="server_${action}"
    ;;
  worker)
    fname="worker_${action}"
    ;;
  *)
    echo "error: unknown service type '${service_type}'" >&2
    usage
    exit 1
    ;;
esac

${fname}
