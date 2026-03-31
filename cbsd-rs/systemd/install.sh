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
_ERRORMARK="\U1F4A5"

_REPO_NAME="clyso/cbs"

_cleanup_dirs=()

cleanup() {
  for dir in "${_cleanup_dirs[@]}"; do
    [[ -n "${dir}" && -d "${dir}" ]] && {
      rm -fr "${dir}" || {
        err "failed to remove temporary directory '${dir}'"
        exit 1
      }
    }
  done
}

trap cleanup EXIT SIGINT SIGTERM

print_boxed() {
  in_str="${1}"
  title="${2}"

  longest_line=0
  orig_IFS=${IFS}
  IFS=$'\n'
  for ln in ${in_str}; do
    [[ ${#ln} -gt ${longest_line} ]] && longest_line=${#ln}
  done
  IFS=${orig_IFS}

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

err() {
  echo -e "${_ERRORMARK} error: $*" >&2
}

success() {
  echo -e "${_CHECKMARK} $*"
}

info() {
  echo -e "${_INFOMARK} $*"
}

warn() {
  echo -e "${_WARNMARK} warning: $*" >&2
}

extract() {
  local dest="${1}"
  local archive="${2}"

  [[ -z "${dest}" ]] && {
    err "extract: destination path must be specified"
    exit 1
  }

  [[ ! -d "${dest}" ]] && {
    err "extract: destination directory does not exist: ${dest}"
    exit 1
  }

  [[ -z "${archive}" ]] && {
    err "extract: archive path must be specified"
    exit 1
  }

  [[ ! -f "${archive}" ]] && {
    err "extract: archive file does not exist: ${archive}"
    exit 1
  }

  tar -C "${dest}" -xf "${archive}" || {
    err "extract: failed to extract archive '${archive}' to '${dest}'"
    exit 1
  }
}

download() {
  local path="${1}"
  local version="${2}"

  [[ ! -d "${path}" ]] && {
    err "download: destination path does not exist: ${path}"
    exit 1
  }

  rel_str="latest"
  [[ -n "${version}" ]] && rel_str="tags/${version}"

  archive_url="$(
    curl -sL \
      -H "Accept: application/vnd.github+json" \
      -H "X-GitHub-Api-Version: 2026-03-10" \
      "https://api.github.com/repos/${_REPO_NAME}/releases/${rel_str}" |
      jq -r '.assets[] |
             select(.name == "cbsd-rs-config.tar") |
             .browser_download_url' 2>/dev/null
  )"

  [[ -z "${archive_url}" ]] && {
    err "failed to find archive URL for '${rel_str}'"
    exit 1
  }

  tmp_dir="$(mktemp -d --suffix='-cbsd-rs-download')"
  _cleanup_dirs+=("${tmp_dir}")

  dst_archive_path="${tmp_dir}/cbsd-rs-config.tar"

  curl -s -o "${dst_archive_path}" -L "${archive_url}" || {
    err "failed to download archive from '${archive_url}'"
    exit 1
  }

  [[ ! -e "${dst_archive_path}" ]] && {
    err "downloaded archive not found at '${dst_archive_path}'"
    exit 1
  }

  extract "${path}" "${dst_archive_path}"

  rm -fr "${tmp_dir}" || {
    err "failed to remove temporary download directory '${tmp_dir}'"
    exit 1
  }
}

usage() {
  cat <<EOF >&2
usage: $0 [SERVICE] [options...]

ADDRESS must be in the form 'DOMAIN:PORT'; e.g., 'cbsd.example.tld:443'.

Services:
  server    cbsd-rs server
  worker    cbsd-rs worker
  nginx     prepare nginx config for server and UI

Options:
  --address HOST:PORT     Specify server address
                          Required for worker and nginx
  --config DIR            Directory for configuration files
  --data DIR              Directory for data files
  -n | --name NAME        Instance name for the service (worker only)
  -d | --deployment NAME  Deployment name (default: default)
  -a | --archive PATH     Path to a tarball archive containing
                          unit and config files
  --no-systemd            Only installs the components, not systemd units
  --download              Download the archive containing unit and config files
  --cbs-version VERSION   Version of the archive to download (default: latest)
  -h | --help             Show this help message and exit
EOF
}

systemd_dir="${HOME}/.config/systemd/user"
config_dir="${HOME}/.config/cbsd-rs"
data_dir="${HOME}/.local/share/cbsd-rs"
deployment="default"
service_name=
archive_path=
do_no_systemd=0
do_download=0
download_version=""
address=

positional_args=()

while [[ $# -gt 0 ]]; do
  case $1 in
    -h | --help)
      usage
      exit 0
      ;;
    --address)
      [[ -z $2 ]] && {
        err "'--address' requires an argument"
        usage
        exit 1
      }
      address="${2}"
      shift 1
      ;;
    -d | --deployment)
      [[ -z $2 ]] && {
        err "'--deployment' requires an argument"
        usage
        exit 1
      }
      deployment="${2}"
      shift 1
      ;;
    -n | --name)
      [[ -z $2 ]] && {
        err "'--name' requires an argument"
        usage
        exit 1
      }
      service_name="${2}"
      shift 1
      ;;
    -a | --archive)
      [[ -z $2 ]] && {
        err "'--archive' requires an argument"
        usage
        exit 1
      }
      archive_path="${2}"
      shift 1
      ;;
    --no-systemd)
      do_no_systemd=1
      ;;
    --download)
      do_download=1
      ;;
    --cbs-version)
      [[ -z $2 ]] && {
        err "'--cbs-version' requires an argument"
        usage
        exit 1
      }
      download_version="${2}"
      shift 1
      ;;
    --config)
      [[ -z $2 ]] && {
        err "'--config' requires an argument"
        usage
        exit 1
      }
      config_dir="${2}"
      shift 1
      ;;
    --data)
      [[ -z $2 ]] && {
        err "'--data' requires an argument"
        usage
        exit 1
      }
      data_dir="${2}"
      shift 1
      ;;
    -*)
      err "unknown option: $1"
      usage
      exit 1
      ;;
    *)
      positional_args+=("$1")
      ;;
  esac
  shift 1
done

our_dir=

[[ -n "${download_version}" && ${do_download} -eq 0 ]] && {
  err "--cbs-version can only be used with --download"
  usage
  exit 1
}

[[ ${do_download} -eq 1 && -n "${archive_path}" ]] && {
  err "cannot specify both --download and --archive"
  usage
  exit 1
}

[[ ${do_download} -eq 0 && -z "${archive_path}" && ! -e ".git" ]] && {
  err "no source of unit and config files specified"
  cat <<EOF >&2
either run from a repository checkout or provide an archive
with --archive or --download
EOF
  exit 1
}

[[ ${do_download} -eq 1 ]] && {
  jq --version >/dev/null 2>&1 || {
    err "jq is required to use --download"
    exit 1
  }
} && {
  curl --version >/dev/null 2>&1 || {
    err "curl is required to use --download"
    exit 1
  }
}

[[ -n "${archive_path}" && ! -f "${archive_path}" ]] && {
  err "specified archive file does not exist: ${archive_path}"
  exit 1
}

src_systemd_dir=
src_components_dir=

[[ ${do_download} -eq 1 || -n "${archive_path}" ]] && {
  info "preparing unit and config files from specified source..."
  our_dir="$(mktemp -d --suffix='-cbsd-rs-config')"
  _cleanup_dirs+=("${our_dir}")

  src_systemd_dir="${our_dir}/systemd"
  src_components_dir="${our_dir}/components"
}

if [[ ${do_download} -eq 1 ]]; then
  download "${our_dir}" "${download_version}"

elif [[ -n "${archive_path}" ]]; then
  extract "${our_dir}" "${archive_path}"

else
  our_dir="${PWD}"
  src_systemd_dir="${our_dir}/cbsd-rs/systemd"
  src_components_dir="${our_dir}/components"
fi

[[ -z "${src_systemd_dir}" || ! -d "${src_systemd_dir}" ]] && [[ ${do_no_systemd} -eq 0 ]] && {
  err "source systemd directory does not exist: ${src_systemd_dir}"
  exit 1
}

[[ -z "${src_components_dir}" || ! -d "${src_components_dir}" ]] && {
  err "source components directory does not exist: ${src_components_dir}"
  exit 1
}

die_on_no_files() {
  local path="${1}"

  local extra=()
  [[ -n "${2}" ]] && extra=("-iname" "${2}")

  local cmd=("find" "${path}" "${extra[@]}" "-type" "f" "-print" "-quit")

  [[ -z "$("${cmd[@]}" 2>/dev/null)" ]] && {
    what=""
    [[ -n "${2}" ]] && what=" matching '${2}'"
    err "no files found in ${path}${what}"
    exit 1
  }
}

[[ ${do_no_systemd} -eq 0 ]] && die_on_no_files "${src_systemd_dir}" "cbsd-rs.*"
die_on_no_files "${src_components_dir}" "cbs.component.yaml"

do_server=0
do_worker=0
do_nginx=0

[[ ${do_no_systemd} -eq 1 && ${#positional_args[@]} -gt 0 ]] && {
  err "services cannot be specified when using --no-systemd"
  usage
  exit 1
}

if [[ ${#positional_args[@]} -eq 0 && ${do_no_systemd} -eq 0 ]]; then
  info "installing all services for deployment '${deployment}'"
  do_server=1
  do_worker=1
  do_nginx=1
else
  for what in "${positional_args[@]}"; do
    case "${what}" in
      server)
        do_server=1
        ;;
      worker)
        do_worker=1
        ;;
      nginx)
        do_nginx=1
        ;;
      *)
        err "unknown service: ${what}"
        usage
        exit 1
        ;;
    esac
  done
fi

[[ $do_worker -eq 1 || $do_nginx -eq 1 ]] && [[ -z "${address}" ]] && {
  err "'--address' must be specified for worker and nginx"
  usage
  exit 1
}

[[ -n "${service_name}" && ${do_worker} -eq 0 ]] && {
  err "instance name cannot be specified for server service"
  usage
  exit 1
}

deployment_config_dir="${config_dir}/${deployment}"
deployment_data_dir="${data_dir}/${deployment}"

[[ ! -d "${deployment_config_dir}" ]] && {
  mkdir -p "${deployment_config_dir}" || {
    err "failed to create config directory: ${deployment_config_dir}"
    exit 1
  }
}

[[ ! -d "${deployment_data_dir}" ]] && {
  mkdir -p "${deployment_data_dir}" || {
    err "failed to create data directory: ${deployment_data_dir}"
    exit 1
  }
}

install_components() {
  local src="${1}"
  local dst="${2}"

  [[ ! -d "${src}" ]] && {
    err "components source directory does not exist: ${src}"
    exit 1
  }

  [[ ! -d "${dst}" ]] && {
    err "components destination directory does not exist: ${dst}"
    err "has the server been installed for this deployment?"
    exit 1
  }

  cp -r "${src}"/* "${dst}" || {
    err "failed to copy components from '${src}' to '${dst}'"
    exit 1
  }
}

if [[ ${do_no_systemd} -eq 1 ]]; then
  info "installing components only, skipping systemd unit installation"
  install_components "${src_components_dir}" "${deployment_data_dir}/components"
  success "components installed to ${deployment_data_dir}/components"
  exit 0
fi

# Install the container lifecycle script to the data directory
cp "${src_systemd_dir}/cbsd-rs-ctr.sh" \
  "${data_dir}/cbsd-rs-ctr.sh" || {
  err "failed to install cbsd-rs-ctr.sh to ${data_dir}"
  exit 1
}

[[ ! -d "${systemd_dir}" ]] && {
  mkdir -p "${systemd_dir}" || {
    err "failed to create systemd user directory: ${systemd_dir}"
    exit 1
  }
}

# Install per-deployment service and target unit files (once per deployment)
if [[ ! -e "${systemd_dir}/cbsd-rs-${deployment}@.service" ]]; then

  cp "${src_systemd_dir}/templates/systemd/cbsd-rs-.service.in" \
    "${systemd_dir}/cbsd-rs-${deployment}@.service" || {
    err "failed to install cbsd-rs service file for ${deployment}"
    exit 1
  }

  sed -i "s|{{deployment}}|${deployment}|g;
    s|{{cbsd_rs_data}}|${data_dir}|g;
    s|{{cbsd_rs_config}}|${config_dir}|g" \
    "${systemd_dir}/cbsd-rs-${deployment}@.service" || {
    err "failed to configure cbsd-rs service file for ${deployment}"
    exit 1
  }

  cp "${src_systemd_dir}/templates/systemd/cbsd-rs-.target.in" \
    "${systemd_dir}/cbsd-rs-${deployment}.target" || {
    err "failed to install cbsd-rs target file for ${deployment}"
    exit 1
  }

  sed -i "s|{{deployment}}|${deployment}|g" \
    "${systemd_dir}/cbsd-rs-${deployment}.target" || {
    err "failed to configure cbsd-rs target file for ${deployment}"
    exit 1
  }

fi

[[ ! -e "${systemd_dir}/cbsd-rs.target" ]] && {
  cp "${src_systemd_dir}/templates/systemd/cbsd-rs.target" \
    "${systemd_dir}/cbsd-rs.target" || {
    err "failed to install cbsd-rs.target"
    exit 1
  }
}

# Install logrotate config and timer (once per deployment)
if [[ ! -e "${deployment_data_dir}/logrotate.conf" ]]; then

  cp "${src_systemd_dir}/templates/config/logrotate.conf.in" \
    "${deployment_data_dir}/logrotate.conf" || {
    err "failed to install logrotate config for ${deployment}"
    exit 1
  }

  sed -i "s|{{deployment}}|${deployment}|g;
    s|{{cbsd_rs_data}}|${data_dir}|g" \
    "${deployment_data_dir}/logrotate.conf" || {
    err "failed to configure logrotate config for ${deployment}"
    exit 1
  }

fi

if [[ ! -e "${systemd_dir}/cbsd-rs-logrotate@.timer" ]]; then

  cp "${src_systemd_dir}/templates/systemd/cbsd-rs-logrotate@.timer" \
    "${systemd_dir}/cbsd-rs-logrotate@.timer" || {
    err "failed to install logrotate timer"
    exit 1
  }

  cp "${src_systemd_dir}/templates/systemd/cbsd-rs-logrotate@.service" \
    "${systemd_dir}/cbsd-rs-logrotate@.service" || {
    err "failed to install logrotate service"
    exit 1
  }

  sed -i "s|{{cbsd_rs_data}}|${data_dir}|g" \
    "${systemd_dir}/cbsd-rs-logrotate@.service" || {
    err "failed to configure logrotate service"
    exit 1
  }

  info "enabling logrotate timer for deployment '${deployment}'..."
  systemctl --user enable "cbsd-rs-logrotate@${deployment}.timer" || {
    err "unable to enable logrotate timer for deployment '${deployment}'"
    exit 1
  }
  success "logrotate timer enabled for deployment '${deployment}'"

fi

enable_service() {
  local svc_name="${1}"
  systemctl --user enable "cbsd-rs-${deployment}@${svc_name}.service" || {
    err "failed to enable service '${svc_name}' for deployment '${deployment}'"
    exit 1
  }
}

# --------------------------------------------------------------------------
# Install server
# --------------------------------------------------------------------------

install_server() {
  info "installing server service for deployment '${deployment}'..."

  old_config_warning=
  dst_server_conf="${deployment_config_dir}/server.conf"
  [[ -e "${dst_server_conf}" ]] && {
    warn "found existing server.conf at ${dst_server_conf}, moving to '.old'"
    mv "${dst_server_conf}" "${dst_server_conf}.old" || {
      err "unable to move '${dst_server_conf}' to '.old'"
      exit 1
    }
    old_config_warning="$(
      cat <<EOF

Old config file can be found at:
  ${dst_server_conf}.old
EOF
    )"
  }

  cp "${src_systemd_dir}/templates/config/server.conf.in" \
    "${dst_server_conf}" || {
    err "failed to install server config for deployment '${deployment}'"
    exit 1
  }

  [[ ! -d "${deployment_config_dir}/server" ]] && {
    mkdir -p "${deployment_config_dir}/server" || {
      err "failed to create server config directory"
      exit 1
    }
  }

  # Install component definitions into the deployment data directory
  components_dir="${deployment_data_dir}/components"
  [[ ! -d "${components_dir}" ]] && {
    mkdir -p "${components_dir}" || {
      err "failed to create worker components directory"
      exit 1
    }
  }

  srv_cfg_yaml="${deployment_config_dir}/server/server.yaml"
  [[ -e "${srv_cfg_yaml}" ]] && {
    warn "found existing server.yaml at ${srv_cfg_yaml}, moving to '.old'"
    mv "${srv_cfg_yaml}" "${srv_cfg_yaml}.old" || {
      err "unable to move '${srv_cfg_yaml}' to '.old'"
      exit 1
    }
    old_config_warning+="$(
      cat <<EOF

Old server.yaml config can be found at:
${srv_cfg_yaml}.old
EOF
    )"
  }
  cp "${src_systemd_dir}/templates/config/server.yaml.in" \
    "${deployment_config_dir}/server/server.yaml" || {
    err "failed to install server.yaml config for deployment '${deployment}'"
    exit 1
  }

  [[ -d "${src_components_dir}" ]] && {
    install_components "${src_components_dir}" "${deployment_data_dir}/components"
  }

  enable_service "server"
  success "server service installed and enabled"

  print_boxed "$(
    cat <<EOF

CBS service 'server' installed for deployment '${deployment}'.

This service *requires* further configuration before it can be started.

systemd unit configuration can be found at:
  ${dst_server_conf}
${old_config_warning}

CBSD server configuration available at
  ${deployment_config_dir}/server/server.yaml

The configuration file must be modified to reflect your deployment.
In particular, the following fields must be properly configured from their
dummy defaults:
  - secrets.token-secret-key
  - oauth.allowed-domains
  - seed.seed-admin

Data files (SQLite DB, build logs) are stored in:
  ${deployment_data_dir}/server/

nginx example configuration file available at
  ${deployment_data_dir}/nginx.cbsd.conf

Ensure the nginx configuration file reflects your deployment.
In particular,
  - the 'listen' address/port entries for SSL
  - the 'ssl_certificate' and 'ssl_certificate_key' entries for your
    TLS certificate and key

The nginx configuration file must be appropriately placed in nginx's
config directory; e.g., in /etc/nginx/vhosts.d/ or /etc/nginx/conf.d/,
and nginx must be reloaded after.

EOF
  )" "server"
}

# --------------------------------------------------------------------------
# Install worker
# --------------------------------------------------------------------------

install_worker() {
  info "installing worker service for deployment '${deployment}'..."

  svc_name="worker"
  svc_name+="${service_name:+.${service_name}}"

  old_config_warning=
  dst_worker_conf="${deployment_config_dir}/${svc_name}.conf"
  [[ -e "${dst_worker_conf}" ]] && {
    warn "found existing ${svc_name}.conf at ${dst_worker_conf}, moving to '.old'"
    mv "${dst_worker_conf}" "${dst_worker_conf}.old" || {
      err "unable to move '${dst_worker_conf}' to '.old'"
      exit 1
    }
    old_config_warning="$(
      cat <<EOF

Old config file can be found at:
  ${dst_worker_conf}.old

EOF
    )"
  }

  cp "${src_systemd_dir}/templates/config/worker.conf.in" \
    "${dst_worker_conf}" || {
    err "failed to install worker config for deployment '${deployment}'"
    exit 1
  }

  sed -i "s|{{cbsd_rs_data}}|${data_dir}|g;
    s|{{deployment}}|${deployment}|g;
    s|{{svc_name}}|${svc_name}|g" \
    "${dst_worker_conf}" || {
    err "failed to configure worker config for deployment '${deployment}'"
    exit 1
  }

  [[ ! -d "${deployment_config_dir}/${svc_name}" ]] && {
    mkdir -p "${deployment_config_dir}/${svc_name}" || {
      err "failed to create worker config directory"
      exit 1
    }
  }

  cfg_yaml="${deployment_config_dir}/${svc_name}/worker.yaml"
  [[ -e "${cfg_yaml}" ]] && {
    warn "found existing worker.yaml at ${cfg_yaml}, moving to '.old'"
    mv "${cfg_yaml}" "${cfg_yaml}.old" || {
      err "unable to move '${cfg_yaml}' to '.old'"
      exit 1
    }
    old_config_warning+="$(
      cat <<EOF

Old worker.yaml config can be found at:
${cfg_yaml}.old
EOF
    )"
  }

  cp "${src_systemd_dir}/templates/config/worker.yaml.in" "${cfg_yaml}" || {
    err "failed to install worker.yaml config for deployment '${deployment}'"
    exit 1
  }

  sed -i "s|{{address}}|${address}|g" "${cfg_yaml}" || {
    err "failed to configure worker.yaml config for deployment '${deployment}'"
    exit 1
  }

  enable_service "${svc_name}"
  success "worker service installed and enabled"

  print_boxed "$(
    cat <<EOF

CBS service '${svc_name}' installed for deployment '${deployment}'.

This service *requires* further configuration before it can be started.

systemd unit configuration can be found at:
  ${dst_worker_conf}

Review WORKER_SCRATCH_DIR, WORKER_CONTAINERS_DIR, and WORKER_CCACHE_DIR
in that file and adjust if the defaults do not suit your host layout.
${old_config_warning}

CBSD worker configuration available at
  ${deployment_config_dir}/${svc_name}/worker.yaml

Register this worker via the server REST API to obtain a worker-token:
  curl -X POST https://${address}/api/admin/workers \\
    -H "Authorization: Bearer <admin-token>" \\
    -H "Content-Type: application/json" \\
    -d '{"name": "${service_name:-<worker-name>}", "arch": "x86_64"}'

Or through the CLI:
  cbc worker register ${service_name:-<worker-name>} x86_64

The configuration file must be modified to reflect your deployment.
The 'worker-token' configuration field must contain the token resulting
from registering the worker.

Component files are installed to:
  ${deployment_data_dir}/components/

EOF
  )" "worker"
}

install_nginx() {
  info "creating nginx config for deployment '${deployment}'..."

  old_config_warning=
  dst_nginx_conf="${deployment_data_dir}/nginx.cbsd.conf"
  [[ -e "${dst_nginx_conf}" ]] && {
    warn "found existing nginx.cbsd.conf at ${dst_nginx_conf}, moving to '.old'"
    mv "${dst_nginx_conf}" "${dst_nginx_conf}.old" || {
      err "unable to move '${dst_nginx_conf}' to '.old'"
      exit 1
    }
    old_config_warning="$(
      cat <<EOF

Old nginx config file can be found at:
  ${dst_nginx_conf}.old
EOF
    )"
  }

  cp "${src_systemd_dir}/templates/config/nginx.cbsd.conf.in" \
    "${dst_nginx_conf}" || {
    err "failed to prepare server's nginx config for deployment '${deployment}'"
    exit 1
  }

  sed -i "s|{{domain}}|${address%%:*}|g" "${dst_nginx_conf}" || {
    err "failed to configure server's nginx config for deployment '${deployment}'"
    exit 1
  }

  info "nginx config created at '${dst_nginx_conf}'"
  [[ -n "${old_config_warning}" ]] && echo -e "${old_config_warning}"
}

[[ ${do_server} -eq 1 ]] && {
  install_server
}

[[ ${do_worker} -eq 1 ]] && {
  install_worker
}

[[ ${do_nginx} -eq 1 ]] && {
  install_nginx
}

systemctl --user daemon-reload || {
  err "failed to reload systemd user daemon"
  exit 1
}
