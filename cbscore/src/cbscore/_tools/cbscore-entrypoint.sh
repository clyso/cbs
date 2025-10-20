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

RUNNER_PATH="/runner"
CBSCORE_PATH="${RUNNER_PATH}/cbscore"

if [[ -z ${HOME} ]] || [[ ${HOME} == "/" ]]; then
  HOME="${RUNNER_PATH}"
  export HOME
fi

mkdir "${RUNNER_PATH}"/bin || true

PATH="${RUNNER_PATH}/bin:$PATH"
export PATH

echo "PATH: ${PATH}"

curl -LsSf https://astral.sh/uv/install.sh |
  UV_INSTALL_DIR="${RUNNER_PATH}"/bin \
    UV_DISABLE_UPDATE=1 \
    UV_NO_MODIFY_PATH=1 \
    sh

cd "${RUNNER_PATH}" || exit 1

export VIRTUAL_ENV="${RUNNER_PATH}/venv"
uv venv --python 3.13 "${RUNNER_PATH}"/venv

# shellcheck source=/dev/null
source "${RUNNER_PATH}"/venv/bin/activate

PATH="/root/.local/bin:$PATH"
export PATH

uv --directory "${CBSCORE_PATH}" \
  tool install \
  --no-cache \
  --python 3.13 \
  . || exit 1

dbg=
[[ -n ${CBS_DEBUG} ]] && [[ ${CBS_DEBUG} == "1" ]] && dbg="--debug"
# shellcheck disable=2048,SC2086
cbsbuild --vault "${RUNNER_PATH}/cbs-build.vault.json" ${dbg} \
  runner build \
  --scratch-dir "${RUNNER_PATH}"/scratch \
  --secrets-path "${RUNNER_PATH}"/secrets.json \
  --components-dir "${RUNNER_PATH}"/components \
  $* || exit 1
