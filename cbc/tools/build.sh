#!/bin/bash

# CBC - CBS client
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

set -e

# Get the directory where this script is located (cbs/cbc/tools)
scriptdir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Go to the 'cbc' package root (one level up from tools)
project_root="$(realpath "${scriptdir}/..")"
cd "${project_root}"

echo ">>> Building cbc binary from ${project_root}..."

# Run PyInstaller via uv
# Adjust 'main.py' if your entry point is named differently (e.g. cli.py)
uv run --with pyinstaller pyinstaller \
  --clean \
  --onefile \
  --name cbc \
  --hidden-import cbsdcore \
  --paths ../cbsdcore \
  --distpath dist \
  --workpath build \
  src/cbc/__main__.py

echo ">>> Build Complete!"
echo ">>> Cleanup..."
rm -fr "${project_root}/build" "${project_root}/cbc.spec"
echo ">>> Binary location: $(realpath "${project_root}/dist/cbc")"
