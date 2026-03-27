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
    echo "UI not available yet; creating mock build"
    mkdir ./build
    cat <<EOF >./build/index.html
<!doctype html>
<html lang="">
  <head>
    <title>CBS</title>
  </head>
  <body>
    <p>CBSD UI is not available yet; this is a placeholder page.</p>
  </body>
</html>
EOF
    ;;

  dev)
    # enable once we have the actual UI available
    #
    # yarn dev --port 3000 --host 0.0.0.0
    echo "UI not available yet; run infinite sleep" >&2
    sleep infinity
    ;;
esac
