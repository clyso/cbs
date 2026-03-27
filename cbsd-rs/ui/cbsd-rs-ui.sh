#!/bin/sh

set -e

[ $# -lt 1 ] && {
  echo "error: expected 'prod' or 'dev' as arguments" >&2
  exit 1
}

case "${1}" in
  prod | dev) ;;
  *)
    echo "error: unknown mode '${1}'; expected 'prod' or 'dev'" >&2
    exit 1
    ;;
esac

# enable once we have the actual UI available
#
# yarn install

case "${1}" in
  prod)
    # enable once we have the actual UI available
    #
    # yarn build
    echo "UI not available yet; using default index.html"
    mkdir ./build || true
    cp ./index.html ./build/index.html
    ;;

  dev)
    # enable once we have the actual UI available
    #
    # yarn dev --port 3000 --host 0.0.0.0
    echo "UI not available yet; serving default index.html"
    mkdir ./build || true
    cp ./index.html ./build/index.html
    npm install http-server -g
    http-server -p 3000 -a 0.0.0.0 ./build
    ;;
esac
