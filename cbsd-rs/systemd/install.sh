#!/bin/bash

# CBS build service daemon (cbsd-rs) — systemd user-service installer
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
# Installs cbsd-rs systemd user services for a named deployment.
# Services are managed as podman containers via cbsd-rs-ctr.sh.
#
# Usage:
#   ./install.sh                          Install server + worker
#   ./install.sh server                   Install server only
#   ./install.sh worker                   Install a worker
#   ./install.sh worker -n host-01        Install a named worker instance

_CHECKMARK="\u2713"
_INFOMARK="\u2139"
_WARNMARK="\u26A0"

print_boxed() {
  in_str="${1}"
  title="${2}"

  longest_line=0
  IFS=$'\n'
  for ln in ${in_str}; do
    [[ ${#ln} -gt ${longest_line} ]] && longest_line=${#ln}
  done

  [[ -n "${title}" && ${#title} -gt ${longest_line} ]] &&
    longest_line=$((${#title} + 4))

  longest_line=$((longest_line + (longest_line % 2)))
  horizontal_len=$((longest_line + 2))

  bottom_horizontal=$(printf "\u2500%.0s" $(seq 1 ${horizontal_len}))
  if [[ -z "${title}" ]]; then
    top_horizontal="${bottom_horizontal}"
  else
    extra_title_padding=4
    padding_len=$(((longest_line - ${#title} - extra_title_padding) / 2))
    padding_chrs=$(printf "\u2500%.0s" $(seq 1 ${padding_len}))
    top_horizontal=$(printf "\u2500\u2500%s %s %s\u2500\u2500" \
      "${padding_chrs}" "${title}" "${padding_chrs}")
  fi

  top_left_corner_chr="$(printf '\u250C')"
  top_right_corner_chr="$(printf '\u2510')"
  bottom_left_corner_chr="$(printf '\u2514')"
  bottom_right_corner_chr="$(printf '\u2518')"
  box_vertical_chr="$(printf '\u2502')"

  printf "%s%s%s\n" \
    "${top_left_corner_chr}" "${top_horizontal}" "${top_right_corner_chr}"

  while IFS= read -r ln || [ -n "${ln}" ]; do
    printf "%s %-${longest_line}s %s\n" \
      "${box_vertical_chr}" "${ln}" "${box_vertical_chr}"
  done <<<"${in_str}"

  printf "%s%s%s\n" \
    "${bottom_left_corner_chr}" "${bottom_horizontal}" "${bottom_right_corner_chr}"
}

# Must be run from the repository root (where .git exists)
[[ ! -e ".git" ]] &&
  echo "warning: this script is intended to be run from the CBS source tree root" >&2 &&
  exit 1

usage() {
  cat <<EOF >&2
usage: $0 [SERVICE] [options...]

Services:
  server    cbsd-rs server
  worker    cbsd-rs worker

Options:
  --config DIR          Directory for configuration files
  --data DIR            Directory for data files
  -n|--name NAME        Instance name for the service (worker only)
  -d|--deployment NAME  Deployment name (default: default)
  -h|--help             Show this help message and exit
EOF
}

base_dir="${PWD}"
our_dir="$(dirname "$0")"
systemd_dir="${HOME}/.config/systemd/user"
config_dir="${HOME}/.config/cbsd-rs"
data_dir="${HOME}/.local/share/cbsd-rs"
deployment="default"
service_name=

positional_args=()

while [[ $# -gt 0 ]]; do
  case $1 in
    -h | --help)
      usage
      exit 0
      ;;
    -d | --deployment)
      [[ -z $2 ]] && {
        echo "error: '--deployment' requires an argument" >&2
        usage
        exit 1
      }
      deployment="${2}"
      shift 1
      ;;
    -n | --name)
      [[ -z $2 ]] && {
        echo "error: '--name' requires an argument" >&2
        usage
        exit 1
      }
      service_name="${2}"
      shift 1
      ;;
    --config)
      [[ -z $2 ]] && {
        echo "error: '--config' requires an argument" >&2
        usage
        exit 1
      }
      config_dir="${2}"
      shift 1
      ;;
    --data)
      [[ -z $2 ]] && {
        echo "error: '--data' requires an argument" >&2
        usage
        exit 1
      }
      data_dir="${2}"
      shift 1
      ;;
    -*)
      echo "error: unknown option: $1" >&2
      usage
      exit 1
      ;;
    *)
      positional_args+=("$1")
      ;;
  esac
  shift 1
done

do_server=0
do_worker=0

if [[ ${#positional_args[@]} -eq 0 ]]; then
  echo -e "${_INFOMARK} installing all services for deployment '${deployment}'"
  do_server=1
  do_worker=1
else
  case "${positional_args[0]}" in
    server)
      do_server=1
      ;;
    worker)
      do_worker=1
      ;;
    *)
      echo "error: unknown service: ${positional_args[0]}" >&2
      usage
      exit 1
      ;;
  esac
fi

deployment_config_dir="${config_dir}/${deployment}"
deployment_data_dir="${data_dir}/${deployment}"

[[ ! -d "${deployment_config_dir}" ]] && {
  mkdir -p "${deployment_config_dir}" || {
    echo "error: failed to create config directory: ${deployment_config_dir}" >&2
    exit 1
  }
}

[[ ! -d "${deployment_data_dir}" ]] && {
  mkdir -p "${deployment_data_dir}" || {
    echo "error: failed to create data directory: ${deployment_data_dir}" >&2
    exit 1
  }
}

# Install the container lifecycle script to the data directory
cp "${our_dir}/cbsd-rs-ctr.sh" \
  "${data_dir}/cbsd-rs-ctr.sh" || {
  echo "error: failed to install cbsd-rs-ctr.sh to ${data_dir}" >&2
  exit 1
}

[[ ! -d "${systemd_dir}" ]] && {
  mkdir -p "${systemd_dir}" || {
    echo "error: failed to create systemd user directory: ${systemd_dir}" >&2
    exit 1
  }
}

# Install per-deployment service and target unit files (once per deployment)
if [[ ! -e "${systemd_dir}/cbsd-rs-${deployment}@.service" ]]; then

  cp "${our_dir}/templates/systemd/cbsd-rs-.service.in" \
    "${systemd_dir}/cbsd-rs-${deployment}@.service" || {
    echo "error: failed to install cbsd-rs service file for ${deployment}" >&2
    exit 1
  }

  sed -i "s|{{deployment}}|${deployment}|g;
    s|{{cbsd_rs_data}}|${data_dir}|g;
    s|{{cbsd_rs_config}}|${config_dir}|g" \
    "${systemd_dir}/cbsd-rs-${deployment}@.service" || {
    echo "error: failed to configure cbsd-rs service file for ${deployment}" >&2
    exit 1
  }

  cp "${our_dir}/templates/systemd/cbsd-rs-.target.in" \
    "${systemd_dir}/cbsd-rs-${deployment}.target" || {
    echo "error: failed to install cbsd-rs target file for ${deployment}" >&2
    exit 1
  }

  sed -i "s|{{deployment}}|${deployment}|g" \
    "${systemd_dir}/cbsd-rs-${deployment}.target" || {
    echo "error: failed to configure cbsd-rs target file for ${deployment}" >&2
    exit 1
  }

fi

# Install the network service and top-level target (once per system)
if [[ ! -e "${systemd_dir}/cbsd-rs-network@.service" ]]; then

  cp "${our_dir}/templates/systemd/cbsd-rs-network@.service" \
    "${systemd_dir}/cbsd-rs-network@.service" || {
    echo "error: failed to install cbsd-rs-network service file" >&2
    exit 1
  }

  echo -e "${_INFOMARK} enabling cbsd-rs network for deployment '${deployment}'..."
  systemctl --user enable "cbsd-rs-network@${deployment}" || {
    echo "error: unable to enable cbsd-rs network for deployment '${deployment}'" >&2
    exit 1
  }
  echo -e "${_CHECKMARK} cbsd-rs network enabled for deployment '${deployment}'"

fi

[[ ! -e "${systemd_dir}/cbsd-rs.target" ]] && {
  cp "${our_dir}/templates/systemd/cbsd-rs.target" \
    "${systemd_dir}/cbsd-rs.target" || {
    echo "error: failed to install cbsd-rs.target" >&2
    exit 1
  }
}

# Install logrotate config and timer (once per deployment)
if [[ ! -e "${deployment_data_dir}/logrotate.conf" ]]; then

  cp "${our_dir}/templates/config/logrotate.conf.in" \
    "${deployment_data_dir}/logrotate.conf" || {
    echo "error: failed to install logrotate config for ${deployment}" >&2
    exit 1
  }

  sed -i "s|{{deployment}}|${deployment}|g;
    s|{{cbsd_rs_data}}|${data_dir}|g" \
    "${deployment_data_dir}/logrotate.conf" || {
    echo "error: failed to configure logrotate config for ${deployment}" >&2
    exit 1
  }

fi

if [[ ! -e "${systemd_dir}/cbsd-rs-logrotate@.timer" ]]; then

  cp "${our_dir}/templates/systemd/cbsd-rs-logrotate@.timer" \
    "${systemd_dir}/cbsd-rs-logrotate@.timer" || {
    echo "error: failed to install logrotate timer" >&2
    exit 1
  }

  cp "${our_dir}/templates/systemd/cbsd-rs-logrotate@.service" \
    "${systemd_dir}/cbsd-rs-logrotate@.service" || {
    echo "error: failed to install logrotate service" >&2
    exit 1
  }

  sed -i "s|{{cbsd_rs_data}}|${data_dir}|g" \
    "${systemd_dir}/cbsd-rs-logrotate@.service" || {
    echo "error: failed to configure logrotate service" >&2
    exit 1
  }

  echo -e "${_INFOMARK} enabling logrotate timer for deployment '${deployment}'..."
  systemctl --user enable "cbsd-rs-logrotate@${deployment}.timer" || {
    echo "error: unable to enable logrotate timer for deployment '${deployment}'" >&2
    exit 1
  }
  echo -e "${_CHECKMARK} logrotate timer enabled for deployment '${deployment}'"

fi

enable_service() {
  svc_name="${1}"
  systemctl --user enable "cbsd-rs-${deployment}@${svc_name}.service" || {
    echo "error: failed to enable service '${svc_name}' for deployment '${deployment}'" >&2
    exit 1
  }
}

# --------------------------------------------------------------------------
# Install server
# --------------------------------------------------------------------------

install_server() {
  echo -e "${_INFOMARK} installing server service for deployment '${deployment}'..."

  old_config_warning=
  dst_server_conf="${deployment_config_dir}/server.conf"
  [[ -e "${dst_server_conf}" ]] && {
    echo -e "${_WARNMARK} warning: found existing server.conf at ${dst_server_conf}, moving to '.old'"
    mv "${dst_server_conf}" "${dst_server_conf}.old" || {
      echo "error: unable to move '${dst_server_conf}' to '.old'" >&2
      exit 1
    }
    old_config_warning="$(
      cat <<EOF

Old config file can be found at:
  ${dst_server_conf}.old

EOF
    )"
  }

  cp "${our_dir}/templates/config/server.conf.in" \
    "${dst_server_conf}" || {
    echo "error: failed to install server config for deployment '${deployment}'" >&2
    exit 1
  }

  [[ ! -d "${deployment_config_dir}/server" ]] && {
    mkdir -p "${deployment_config_dir}/server" || {
      echo "error: failed to create server config directory" >&2
      exit 1
    }
  }

  enable_service "server"
  echo -e "${_CHECKMARK} server service installed and enabled"

  print_boxed "$(
    cat <<EOF

CBS-RS service 'server' installed for deployment '${deployment}'.

This service *requires* further configuration before it can be started.

systemd unit configuration can be found at:
  ${dst_server_conf}
${old_config_warning}

CBSD-RS server configuration must exist at:
  ${deployment_config_dir}/server/server.yaml

Copy and adapt cbsd-rs/config/server.yaml.example — paths inside the
configuration file must point to container-internal paths (/cbs/...).

Data files (SQLite DB, build logs) are stored in:
  ${deployment_data_dir}/server/

EOF
  )" "server"
}

# --------------------------------------------------------------------------
# Install worker
# --------------------------------------------------------------------------

install_worker() {
  echo -e "${_INFOMARK} installing worker service for deployment '${deployment}'..."

  svc_name="worker"
  svc_name+="${service_name:+.${service_name}}"

  old_config_warning=
  dst_worker_conf="${deployment_config_dir}/${svc_name}.conf"
  [[ -e "${dst_worker_conf}" ]] && {
    echo -e "${_WARNMARK} warning: found existing ${svc_name}.conf at ${dst_worker_conf}, moving to '.old'"
    mv "${dst_worker_conf}" "${dst_worker_conf}.old" || {
      echo "error: unable to move '${dst_worker_conf}' to '.old'" >&2
      exit 1
    }
    old_config_warning="$(
      cat <<EOF

Old config file can be found at:
  ${dst_worker_conf}.old

EOF
    )"
  }

  cp "${our_dir}/templates/config/worker.conf.in" \
    "${dst_worker_conf}" || {
    echo "error: failed to install worker config for deployment '${deployment}'" >&2
    exit 1
  }

  sed -i "s|{{cbsd_rs_data}}|${data_dir}|g;
    s|{{deployment}}|${deployment}|g;
    s|{{svc_name}}|${svc_name}|g" \
    "${dst_worker_conf}" || {
    echo "error: failed to configure worker config for deployment '${deployment}'" >&2
    exit 1
  }

  # Install component definitions into the deployment data directory
  components_dir="${deployment_data_dir}/components"
  [[ ! -d "${components_dir}" ]] && {
    mkdir -p "${components_dir}" || {
      echo "error: failed to create worker components directory" >&2
      exit 1
    }
  }

  [[ -d "${base_dir}/components" ]] && {
    cp -R "${base_dir}/components/"* \
      "${deployment_data_dir}/components/" || {
      echo "error: failed to install worker components for deployment '${deployment}'" >&2
      exit 1
    }
  }

  [[ ! -d "${deployment_config_dir}/${svc_name}" ]] && {
    mkdir -p "${deployment_config_dir}/${svc_name}" || {
      echo "error: failed to create worker config directory" >&2
      exit 1
    }
  }

  enable_service "${svc_name}"
  echo -e "${_CHECKMARK} worker service installed and enabled"

  print_boxed "$(
    cat <<EOF

CBS-RS service '${svc_name}' installed for deployment '${deployment}'.

This service *requires* further configuration before it can be started.

systemd unit configuration can be found at:
  ${dst_worker_conf}

Review WORKER_SCRATCH_DIR, WORKER_CONTAINERS_DIR, and WORKER_CCACHE_DIR
in that file and adjust if the defaults do not suit your host layout.
${old_config_warning}

CBSD-RS worker configuration must exist at:
  ${deployment_config_dir}/${svc_name}/worker.yaml

Copy and adapt cbsd-rs/config/worker.yaml.example. The server-url must
use the server's container name on the cbsd-rs-${deployment} network:
  server-url: "ws://cbsd-rs-server.${deployment}:8080/api/ws/worker"

Register this worker via the server REST API to obtain a worker-token:
  curl -X POST http://<server-host>:8080/api/admin/workers \\
    -H "Authorization: Bearer <admin-token>" \\
    -H "Content-Type: application/json" \\
    -d '{"name": "<worker-name>", "arch": "x86_64"}'

Set the returned worker-token in worker.yaml before starting the service.

Component files are installed to:
  ${deployment_data_dir}/components/

EOF
  )" "worker"
}

[[ ${do_server} -eq 1 ]] && {
  install_server
}

[[ ${do_worker} -eq 1 ]] && {
  install_worker
}

systemctl --user daemon-reload || {
  echo "error: failed to reload systemd user daemon" >&2
  exit 1
}
