#!/bin/bash
# CBS - Clyso Build System
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.

config_dir="${CBSD_CONFIG:-/cbs/config}"

[[ -z "${config_dir}" ]] && {
  echo "error: missing config directory" >&2
  exit 1
}

[[ ! -d "${config_dir}" ]] && {
  echo "error: config dir at '${config_dir}' does not exist" >&2
  exit 1
}

ssl_args=

[[ -e "${config_dir}/cbs.key.pem" && -e "${config_dir}/cbs.cert.pem" ]] && {
  ssl_args=(
    "--ssl-keyfile"
    "${config_dir}/cbs.key.pem"
    "--ssl-certfile"
    "${config_dir}/cbs.cert.pem"
  )
}

# shellcheck disable=SC2048,SC2086
uv run --no-sync \
  uvicorn \
  --factory \
  --host "0.0.0.0" \
  --port "8080" \
  ${ssl_args[*]} \
  cbs-server:factory
