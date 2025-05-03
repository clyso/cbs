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

RUNNER_DIR="/runner"

if [[ -z ${HOME} ]] || [[ ${HOME} == "/" ]]; then
  HOME="${RUNNER_DIR}"
  export HOME
fi

mkdir "${RUNNER_DIR}"/bin || true

PATH="${RUNNER_DIR}/bin:$PATH"
export PATH

curl -LsSf https://astral.sh/uv/install.sh |
  UV_INSTALL_DIR="${RUNNER_DIR}"/bin \
    UV_DISABLE_UPDATE=1 \
    UV_NO_MODIFY_PATH=1 \
    sh

cd "${RUNNER_DIR}" || exit 1

uv venv --python 3.13 "${RUNNER_DIR}"/venv

# shellcheck source=/dev/null
source "${RUNNER_DIR}"/venv/bin/activate

uv pip install -r "${ourpath}/requirements.txt" || exit 1

dbg=
[[ -n ${CBS_DEBUG} ]] && [[ ${CBS_DEBUG} == "1" ]] && dbg="--debug"
# shellcheck disable=2048,SC2086
python3 "${ourpath}"/ces-build.py ${dbg} runner build \
  --scratch-dir "${RUNNER_DIR}"/scratch \
  --secrets-path "${RUNNER_DIR}"/secrets.json \
  --components-dir "${RUNNER_DIR}"/components \
  --containers-dir "${RUNNER_DIR}"/containers \
  $* || exit 1
