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

package_deps=(
  "podman"
  "podman-compose"
  "git"
  "yq"
  "jq"
  "openssl"
  "sed"
)

other_deps=(
  "uv"
)

show_deps() {
  cat <<EOF >/dev/stderr
CBSD requires the following dependencies to be installed:

EOF

  for d in "${package_deps[@]}" "${other_deps[@]}"; do
    echo "  - ${d}" >/dev/stderr
  done

}

usage() {
  cat <<EOF >/dev/stderr
usage: $0 [options...]

Options:
  --show-deps   Show required dependencies and exit
  --no-uv       Do not install 'uv' tool
  -h|--help     Show this help message and exit

EOF
}

no_uv=0

while [[ $# -gt 0 ]]; do
  case $1 in
    --show-deps)
      show_deps
      exit 0
      ;;
    --no-uv)
      no_uv=1
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      echo "error: unknown option '$1'" >/dev/stderr
      usage
      exit 1
      ;;
    *)
      echo "error: unexpected positional argument '$1'" >/dev/stderr
      usage
      exit 1
      ;;
  esac
  shift 1
done

[[ $(id -u) -ne 0 ]] && {
  echo "error: this script must be run as root" >/dev/stderr
  exit 1
}

[[ ! -e /etc/os-release ]] &&
  echo "error: unable to find '/etc/os-release', please install dependencies manually." \
    >/dev/stderr && {
  show_deps
  exit 1
}

source /etc/os-release

case "${ID}" in
  fedora | rhel | centos | rocky)

    case ${ID} in
      rhel | centos | rocky)
        dnf install -y epel-release || exit 1
        ;;
    esac

    dnf install -y "${package_deps[@]}" || exit 1

    ;;
  *)
    echo "error: unsupported OS '${ID}', please install dependencies manually." \
      >/dev/stderr
    show_deps
    exit 1
    ;;
esac

[[ $no_uv -eq 0 ]] && {
  curl -LsSf https://astral.sh/uv/install.sh |
    env UV_NO_MODIFY_PATH=1 UV_INSTALL_DIR=/usr/bin sh
}
