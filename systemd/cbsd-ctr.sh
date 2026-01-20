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

usage() {
  cat <<EOF >&2
usage: $0 ACTION DEPLOYMENT SERVICE [options...]

Actions:
  start     Start specified component
  stop      Stop specified component

Options:
  -c|--config DIR       Specify the directory for configuration files
  -d|--data DIR         Specify the directory for data files
  -h|--help             Show this help message and exit
EOF
}

config_dir="${HOME}/.config/cbsd"
data_dir="${HOME}/.local/share/cbsd"

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

real_deployment_name="${deployment_name#cbsd-}"
[[ -z "${deployment_name}" || "${real_deployment_name}" == "${deployment_name}" ]] && {
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

ctr_name="cbsd-${service_type}.${deployment_name}"
ctr_name+="${service_id:+.${service_id}}"

echo "run '${action}' for '${ctr_name}'"
echo "  deployment:     ${deployment_name}"
echo "  service name:   ${service}"
echo "  service type:   ${service_type}"
echo "  service id:     ${service_id}"
echo "  container name: ${ctr_name}"

source_config() {
  [[ -e "${config_dir}/${deployment_name}/${service}.conf" ]] && {
    #shellcheck source=/dev/null
    source "${config_dir}/${deployment_name}/${service}.conf"
  }
}

redis_start() {
  echo "starting redis '${ctr_name}'..."

  redis_data_dir="${data_dir}/${deployment_name}/${service}/data"

  REDIS_BIND_ADDR="127.0.0.1"
  REDIS_PORT="6379"
  source_config

  [[ ! -d "${redis_data_dir}" ]] && {
    mkdir -p "${redis_data_dir}" || {
      echo "error: unable to create redis data directory at '${redis_data_dir}" \
        >&2
      exit 1
    }
  }

  podman run \
    -d \
    --replace \
    -p "${REDIS_BIND_ADDR}":"${REDIS_PORT}":6379 \
    -v "${redis_data_dir}":/data:Z \
    --security-opt label=disable \
    --network "cbsd-${deployment_name}" \
    --userns keep-id \
    --name "${ctr_name}" \
    docker.io/redis:8.4 || {
    echo "error: failed to start redis '${ctr_name}'" >&2
    exit 1
  }
}

redis_stop() {
  echo "stopping redis '${ctr_name}'..."
  podman stop "${ctr_name}" || {
    echo "error: failed to stop redis '${ctr_name}'" >&2
    exit 1
  }
}

server_start() {
  echo "starting server '${ctr_name}'..."

  server_config_dir="${config_dir}/${deployment_name}/${service}"
  server_data_dir="${data_dir}/${deployment_name}/${service}"
  server_logs_dir="${data_dir}/${deployment_name}/${service}/logs"

  DEBUG=0
  SERVER_BIND_ADDR="127.0.0.1"
  SERVER_BIND_PORT="8080"
  IMAGE="harbor.clyso.com/cbs/cbsd-server:latest"
  source_config

  [[ ! -d "${server_config_dir}" ]] && {
    echo "error: server config directory '${server_config_dir}'" \
      "does not exist" >&2
    exit 1
  }

  [[ ! -d "${server_data_dir}" ]] && {
    mkdir -p "${server_data_dir}" || {
      echo "error: failed to create server data directory" \
        "'${server_data_dir}'" >&2
      exit 1
    }
  }

  [[ ! -d "${server_logs_dir}" ]] && {
    mkdir -p "${server_logs_dir}" || {
      echo "error: failed to create server logs directory '${server_logs_dir}'" \
        >&2
      exit 1
    }
  }

  debug_args=""
  [[ $DEBUG -eq 1 ]] && {
    debug_args="--env CBS_DEBUG=1"
  }

  # shellcheck disable=SC2086
  podman run \
    -d \
    --replace \
    -p "${SERVER_BIND_ADDR}":"${SERVER_BIND_PORT}":8080 \
    -v "${server_config_dir}":/cbs/config:ro \
    -v "${server_data_dir}":/cbs/data:Z \
    -v "${server_logs_dir}":/cbs/logs:Z \
    -e CBS_CONFIG=/cbs/config/cbsd.server.config.yaml ${debug_args} \
    --security-opt label=disable \
    --security-opt seccomp=unconfined \
    --privileged \
    --network "cbsd-${deployment_name}" \
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

worker_start() {
  echo "starting worker '${ctr_name}'..."

  worker_config_dir="${config_dir}/${deployment_name}/${service}"
  worker_components_dir="${data_dir}/${deployment_name}/components"

  WORKER_SCRATCH_DIR=
  WORKER_CONTAINERS_DIR=
  WORKER_CCACHE_DIR=
  DEBUG=0
  IMAGE="harbor.clyso.com/cbs/cbsd-worker:latest"
  source_config

  debug_args=""
  [[ $DEBUG -eq 1 ]] && {
    debug_args="--env CBS_DEBUG=1"
  }

  [[ ! -d "${worker_config_dir}" ]] && {
    echo "error: worker config directory '${worker_config_dir}' does not exist" \
      >&2
    exit 1
  }

  [[ ! -d "${worker_components_dir}" ]] && {
    echo "error: worker components directory '${worker_components_dir}'" \
      "does not exist" >&2
    exit 1
  }

  [[ ! -d "${WORKER_SCRATCH_DIR}" ]] && {
    mkdir -p "${WORKER_SCRATCH_DIR}" || {
      echo "error: failed to create worker scratch directory" \
        "'${WORKER_SCRATCH_DIR}'" >&2
      exit 1
    }
  }

  [[ ! -d "${WORKER_CONTAINERS_DIR}" ]] && {
    mkdir -p "${WORKER_CONTAINERS_DIR}" || {
      echo "error: failed to create worker containers directory" \
        "'${WORKER_CONTAINERS_DIR}'" >&2
      exit 1
    }
  }

  [[ ! -d "${WORKER_CCACHE_DIR}" ]] && {
    mkdir -p "${WORKER_CCACHE_DIR}" || {
      echo "error: failed to create worker ccache directory" \
        "'${WORKER_CCACHE_DIR}'" >&2
      exit 1
    }
  }

  # shellcheck disable=SC2086
  podman run \
    -d \
    --replace \
    -v "${worker_config_dir}":/cbs/config:ro \
    -v "${WORKER_SCRATCH_DIR}":/cbs/scratch:Z \
    -v "${WORKER_CONTAINERS_DIR}":/var/lib/containers:Z \
    -v "${WORKER_CCACHE_DIR}":/cbs/ccache:Z \
    -v "${worker_components_dir}":/cbs/components:ro \
    -v /dev/fuse:/dev/fuse:rw \
    -e CBS_CONFIG=/cbs/config/cbsd.worker.config.yaml ${debug_args} \
    --cap-add SYS_ADMIN \
    --cap-add MKNOD \
    --security-opt label=disable \
    --security-opt seccomp=unconfined \
    --privileged \
    --network "cbsd-${deployment_name}" \
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

fname=

case "${service_type}" in
  redis)
    fname="redis_${action}"
    ;;
  server)
    fname="server_${action}"
    ;;
  worker)
    fname="worker_${action}"
    ;;
  *)
    echo "error: unknown service: ${service_type}" >&2
    usage
    exit 1
    ;;
esac

${fname}
