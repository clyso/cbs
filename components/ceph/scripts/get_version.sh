#!/bin/bash

get_version() {
  version="$(git describe --long --match 'v*-ces-v*' 2>/dev/null | sed s/^v//)"
  [[ -z ${version} ]] &&
    version="$(git describe --long --match 'v*' 2>/dev/null | sed s/^v//)"
  echo "${version}"
}

get_version
