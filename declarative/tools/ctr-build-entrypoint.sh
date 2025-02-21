#!/bin/bash

# Helper for building CES in a container
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

ourpath="$(dirname "$(realpath "$0")")"

if [[ -z "${HOME}" ]] || [[ "${HOME}" == "/" ]]; then
  HOME=/builder
  export HOME
fi

mkdir /builder/bin || true

PATH="/builder/bin:$PATH"
export PATH

curl -LsSf https://astral.sh/uv/install.sh |
  UV_INSTALL_DIR=/builder/bin \
    UV_DISABLE_UPDATE=1 \
    UV_NO_MODIFY_PATH=1 \
    sh

cd /builder || exit 1

uv venv --python 3.13 /builder/venv

# shellcheck source=/dev/null
source /builder/venv/bin/activate

uv pip install -r "${ourpath}/requirements.txt" || exit 1

dbg=
[[ -n "${WITH_DEBUG}" ]] && [[ "${WITH_DEBUG}" == "1" ]] && dbg="-d"
# shellcheck disable=2048,SC2086
python3 "${ourpath}"/ces-build.py ${dbg} ctr-build \
  --scratch-dir /builder/scratch \
  --secrets-path /builder/secrets.json \
  --components-dir /builder/components \
  $* || exit 1
