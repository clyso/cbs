#!/bin/bash

# CES - install dependencies to build Ceph RPMs
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

ceph_install_deps() {
  local ceph_dir="${1}"

  pushd "${ceph_dir}" || exit 1
  ./install-deps.sh || exit 1
  popd || exit 1
}

ceph_install_deps "$@"
