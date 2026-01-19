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

_CHECKMARK="\u2713"
_INFOMARK="\u2139"

print_boxed() {
  in_str="${1}"
  title="${2}"

  longest_line=0
  IFS=$'\n'
  for ln in ${in_str}; do
    [[ ${#ln} -gt ${longest_line} ]] && longest_line=${#ln}
  done

  [[ -n "${title}" && ${#title} -gt ${longest_line} ]] &&
    longest_line=$((${#title} + 4)) # add extra padding

  longest_line=$((longest_line + (longest_line % 2)))
  horizontal_len=$((longest_line + 2))

  bottom_horizontal=$(printf "\u2500%.0s" $(seq 1 ${horizontal_len}))
  if [[ -z "${title}" ]]; then
    top_horizontal="${bottom_horizontal}"
  else
    extra_title_padding=4 # 4 additional chars before title line
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

[[ ! -e ".git" ]] &&
  echo "warning: this script is intended to be run from the CBS source tree root" >/dev/stderr &&
  exit 1

usage() {
  cat <<EOF >/dev/stderr
usage: $0 [SERVICE] [options...]

Services:
  redis     redis server for CBSD usage
  server    CBSD server
  worker    CBSD worker

Options:
  --config DIR          Specify the directory for configuration files
  --data DIR            Specify the directory for data files
  -n|--name NAME        Specify the service's instance name
  -d|--deployment NAME  Specify the deployment name (default: default)
  -h|--help             Show this help message and exit
EOF
}

base_dir="${PWD}"
our_dir="$(dirname "$0")"
systemd_dir="${HOME}/.config/systemd/user"
config_dir="${HOME}/.config/cbsd"
data_dir="${HOME}/.local/share/cbsd"
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
        echo "error: '--deployment' requires an argument" >/dev/stderr
        usage
        exit 1
      }
      deployment="${2}"
      shift 1
      ;;
    -n | --name)
      [[ -z $2 ]] && {
        echo "error: '--name' requires an argument" >/dev/stderr
        usage
        exit 1
      }
      service_name="${2}"
      shift 1
      ;;
    --config)
      [[ -z $2 ]] && {
        echo "error: '--config' requires an argument" >/dev/stderr
        usage
        exit 1
      }
      config_dir="${2}"
      shift 1
      ;;
    --data)
      [[ -z $2 ]] && {
        echo "error: '--data' requires an argument" >/dev/stderr
        usage
        exit 1
      }
      data_dir="${2}"
      shift 1
      ;;
    -*)
      echo "error: unknown option: $1" >/dev/stderr
      usage
      exit 1
      ;;
    *)
      positional_args+=("$1")
      ;;
  esac
  shift 1
done

do_redis=0
do_server=0
do_worker=0

if [[ ${#positional_args[@]} -eq 0 ]]; then
  echo -e "${_INFOMARK} all services for deployment '${deployment}'"
  do_redis=1
  do_server=1
  do_worker=1

else
  case "${positional_args[0]}" in
    redis)
      do_redis=1
      ;;
    server)
      do_server=1
      ;;
    worker)
      do_worker=1
      ;;
    *)
      echo "error: unknown service: ${positional_args[0]}" >/dev/stderr
      usage
      exit 1
      ;;
  esac
fi

deployment_config_dir="${config_dir}/${deployment}"
deployment_data_dir="${data_dir}/${deployment}"

[[ ! -d "${deployment_config_dir}" ]] && {
  mkdir -p "${deployment_config_dir}" ||
    {
      echo "error: failed to create config directory: ${deployment_config_dir}" >/dev/stderr
      exit 1
    }
}

[[ ! -d "${deployment_data_dir}" ]] && {
  mkdir -p "${deployment_data_dir}" ||
    {
      echo "error: failed to create data directory: ${deployment_data_dir}" >/dev/stderr
      exit 1
    }
}

cp "${our_dir}/cbsd-ctr.sh" \
  "${data_dir}/cbsd-ctr.sh" || {
  echo "error: failed to install cbsd-ctr.sh to ${data_dir}" >/dev/stderr
  exit 1
}

[[ ! -d "${systemd_dir}" ]] && {
  mkdir -p "${systemd_dir}" ||
    {
      echo "error: failed to create systemd user directory: ${systemd_dir}" >/dev/stderr
      exit 1
    }
}

if [[ ! -e "${systemd_dir}/cbsd-${deployment}@.service" ]]; then

  cp "${our_dir}/templates/systemd/cbsd-.service.in" \
    "${systemd_dir}/cbsd-${deployment}@.service" ||
    {
      echo "error: failed to install cbsd service file for ${deployment}" >/dev/stderr
      exit 1
    }

  sed -i "s|{{deployment}}|${deployment}|g;
    s|{{cbsd_data}}|${data_dir}|g;
    s|{{cbsd_config}}|${config_dir}|g" \
    "${systemd_dir}/cbsd-${deployment}@.service" || {
    echo "error: failed to configure cbsd service file for ${deployment}" >/dev/stderr
    exit 1
  }

  cp "${our_dir}/templates/systemd/cbsd-.target.in" \
    "${systemd_dir}/cbsd-${deployment}.target" ||
    {
      echo "error: failed to install cbsd target file for ${deployment}" >/dev/stderr
      exit 1
    }

  sed -i "s|{{deployment}}|${deployment}|g" "${systemd_dir}/cbsd-${deployment}.target" || {
    echo "error: failed to configure cbsd target file for ${deployment}" >/dev/stderr
    exit 1
  }

fi

[[ ! -e "${systemd_dir}/cbsd.target" ]] && {
  cp "${our_dir}/templates/systemd/cbsd.target" \
    "${systemd_dir}/cbsd.target" ||
    {
      echo "error: failed to install cbsd target file" >/dev/stderr
      exit 1
    }
}

enable_service() {
  svc_name="${1}"
  systemctl --user enable "cbsd-${deployment}@${svc_name}.service" || {
    echo "error: failed to enable cbsd service '${svc_name}' for" \
      "deployment '${deployment}'" >/dev/stderr
    exit 1
  }
}

install_redis() {
  echo -e "${_INFOMARK} installing redis service for" \
    "deployment '${deployment}'..."

  cp "${our_dir}/templates/config/redis.conf.in" \
    "${deployment_config_dir}/redis.conf" ||
    {
      echo "error: failed to install redis config for" \
        "deployment '${deployment}'" >/dev/stderr
      exit 1
    }

  enable_service "redis"
  echo -e "${_CHECKMARK} redis service installed and enabled"
}

install_server() {
  echo -e "${_INFOMARK} installing server service for" \
    "deployment '${deployment}'..."

  cp "${our_dir}/templates/config/server.conf.in" \
    "${deployment_config_dir}/server.conf" ||
    {
      echo "error: failed to install server config for" \
        "deployment '${deployment}'" >/dev/stderr
      exit 1
    }

  [[ ! -d "${deployment_config_dir}/server" ]] && {
    mkdir -p "${deployment_config_dir}/server" ||
      {
        echo "error: failed to create server config directory" \
          "for deployment '${deployment}'" >/dev/stderr
        exit 1
      }
  }

  enable_service "server"
  echo -e "${_CHECKMARK} server service installed and enabled"

  print_boxed "$(
    cat <<EOF

CBS service 'server' installed for deployment '${deployment}'.

This service *requires* further configuration before it can be started.

systemd unit configuration can be found at:
  ${deployment_config_dir}/server.conf

CBSD server configuration must exist in:
  ${deployment_config_dir}/server/

Please ensure the appropriate configuration is set up before starting the
service. Check the example configuration file under 'cbsd/' in the CBS
repository.

NOTE: Keep in mind that paths in the configuration file must be pointing to
'/cbs/config' and not the local filesystem config directory.

CBSD server data files are kept in:
  ${deployment_data_dir}/server/

EOF
  )" "server"

}

install_worker() {
  echo -e "${_INFOMARK} installing worker service" \
    "for deployment '${deployment}'..."

  svc_name="worker"
  svc_name+="${service_name:+.${service_name}}"

  cp "${our_dir}/templates/config/worker.conf.in" \
    "${deployment_config_dir}/${svc_name}.conf" ||
    {
      echo "error: failed to install worker config" \
        "for deployment '${deployment}'" >/dev/stderr
      exit 1
    }

  sed -i "s|{{cbsd_data}}|${data_dir}|g;
    s|{{deployment}}|${deployment}|g;
    s|{{svc_name}}|${svc_name}|g" \
    "${deployment_config_dir}/${svc_name}.conf" || {
    echo "error: failed to configure worker config" \
      "for deployment '${deployment}'" >/dev/stderr
    exit 1
  }

  # install components to deployment's data directory
  components_dir="${deployment_data_dir}/components"
  [[ ! -d "${components_dir}" ]] && {
    mkdir -p "${components_dir}" ||
      {
        echo "error: failed to create worker components directory" \
          "for deployment '${deployment}'" >/dev/stderr
        exit 1
      }
  }

  cp -R "${base_dir}/components/"* \
    "${deployment_data_dir}/components/" ||
    {
      echo "error: failed to install worker components" \
        "for deployment '${deployment}'" >/dev/stderr
      exit 1
    }

  [[ ! -d "${deployment_config_dir}/${svc_name}" ]] && {
    mkdir -p "${deployment_config_dir}/${svc_name}" ||
      {
        echo "error: failed to create worker config directory" \
          "for deployment '${deployment}'" >/dev/stderr
        exit 1
      }
  }

  enable_service "${svc_name}"
  echo -e "${_CHECKMARK} worker service installed and enabled"

  print_boxed "$(
    cat <<EOF

CBS service '${svc_name}' installed for deployment '${deployment}'.

This service *requires* further configuration before it can be started.

systemd unit configuration can be found at:
  ${deployment_config_dir}/${svc_name}.conf

Consider editing this file to adjust paths for the worker's scratch,
containers, and ccache directories.

CBSD worker configuration must exist in:
  ${deployment_config_dir}/${svc_name}/

Please ensure the appropriate configuration is set up before starting the
service. Consider running the 'cbsbuild' tool to configure the worker:

  cbsbuild config init --for-systemd-install \\
    --systemd-deployment ${deployment} [opts...]

CBSD worker component files are kept in:
  ${deployment_data_dir}/components/

Additional component files may be added to this directory as needed.

EOF
  )" "worker"
}

[[ ${do_redis} -eq 1 ]] && {
  install_redis
}

[[ ${do_server} -eq 1 ]] && {
  install_server
}

[[ ${do_worker} -eq 1 ]] && {
  install_worker
}

systemctl --user daemon-reload || {
  echo "error: failed to reload systemd user daemon" >/dev/stderr
  exit 1
}
