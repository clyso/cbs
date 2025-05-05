#!/bin/bash

ourdir="$(dirname "$(realpath "$0")")"

cbs_cfg="${ourdir}/local/cbs-config.server.json"
worker_cfg="${ourdir}/local/cbs-config.worker.json"
google_client_secrets="${ourdir}/local/google-client-cbs.json"
cbs_cert="${ourdir}/local/cbs-cert.pem"
cbs_key="${ourdir}/local/cbs-key.pem"

down() {
  PODMAN_COMPOSE_PROVIDER="podman-compose" podman compose \
    -f ./podman-compose.cbs.yaml down
}

up() {
  PODMAN_COMPOSE_PROVIDER="podman-compose" podman compose --verbose \
    --podman-run-args="--rm" -f ./podman-compose.cbs.yaml up --build
}

prepare() {
  local scratch_dir="${1}"

  [[ ! -d "${scratch_dir}" ]] &&
    echo "error: scratch dir at ${scratch_dir} not found" >/dev/stderr &&
    exit 1

  [[ ! -d "${ourdir}/local" ]] && mkdir "${ourdir}"/local
  [[ ! -e "${ourdir}/local/scratch" ]] &&
    ln -fs "${scratch_dir}" "${ourdir}"/local/scratch

  [[ ! -e "${scratch_dir}"/ccache ]] &&
    mkdir "${scratch_dir}"/ccache
  [[ ! -e "${scratch_dir}"/containers ]] &&
    mkdir "${scratch_dir}"/containers

  [[ ! -e "${ourdir}"/local/data ]] &&
    mkdir "${ourdir}"/local/data

  if [[ ! -e "${cbs_cert}" ]] || [[ ! -e "${cbs_key}" ]]; then
    pushd "${ourdir}/local" >/dev/null || true
    "${ourdir}"/cbs/gen-reqs.sh
    popd >/dev/null || true
  fi
}

check() {
  [[ ! -e "${worker_cfg}" ]] &&
    echo "error: missing cbs worker config at '${worker_cfg}'" >/dev/stderr &&
    exit 1

  [[ ! -e "${google_client_secrets}" ]] &&
    echo "error: missing google client secrets at '${google_client_secrets}'" \
      >/dev/stderr &&
    exit 1

  [[ ! -e "${cbs_cfg}" ]] &&
    echo "error: missing cbs config at '${cbs_cfg}'" >/dev/stderr &&
    exit 1
}

usage() {
  cat <<EOF >/dev/stderr
usage: $0 <COMMAND>

Commands:
  prepare <SCRATCH_DIR>     prepare environment to run CBS
  up                        bring up a CBS podman-compose environment
  down                      bring down a CBS podman-compose environment

Options:
  -h | --help               Show this message

EOF
}

[[ $# -eq 0 ]] && usage && exit 1

while [[ $# -gt 0 ]]; do
  case $1 in
    prepare)
      [[ -z $2 ]] &&
        echo "error: missing 'scratch_dir' argument" >/dev/stderr &&
        usage &&
        exit 1
      prepare "${2}"
      shift 1
      ;;
    up)
      check
      down
      up
      ;;
    down)
      down
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument '${1}'" >/dev/stderr
      usage
      exit 1
      ;;
  esac
  shift 1
done
