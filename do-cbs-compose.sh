#!/bin/bash

components=()

ourdir="$(dirname "$(realpath "$0")")"
localdir="${ourdir}/_local"
rundir="${localdir}/cbs"

components_dir="${rundir}/components"
vault_cfg="${rundir}/cbs.vault.json"
server_cfg="${rundir}/cbs.config.server.json"
worker_cfg="${rundir}/cbs.config.worker.json"
google_client_secrets="${rundir}/google-client-cbs.json"
cbs_cert="${rundir}/cbs-cert.pem"
cbs_key="${rundir}/cbs-key.pem"

down() {
  PODMAN_COMPOSE_PROVIDER="podman-compose" podman compose \
    -f ./podman-compose.cbs.yaml down
}

up() {
  PODMAN_COMPOSE_PROVIDER="podman-compose" podman compose --verbose \
    --podman-run-args="--rm" -f ./podman-compose.cbs.yaml up --build
}

gen_server_keys() {
  if [[ -e "${cbs_key}" ]] && [[ -e "${cbs_cert}" ]]; then
    echo "PEM key and cert already exist, not overwriting"
    return
  fi

  openssl req -x509 -newkey rsa:4096 -keyout "${cbs_key}" \
    -out "${cbs_cert}" -days 365 -nodes -subj "/CN=cbs" || (
    echo "error: failed to generate self-signed cert" >/dev/stderr
    exit 1
  )
}

set_server_keys() {
  local src_template="${1}"
  local tgt_file="${2}"
  local srv_key="${3}"
  local tkn_key="${4}"

  (jq ".server.secrets.session_secret_key = \"${srv_key}\" |
    .server.secrets.token_secret_key = \"${tkn_key}\"" \
    "${src_template}" >"${tgt_file}") || (
    echo "error: failed to set keys in ${tgt_file}" >/dev/stderr && exit 1
  )
}

prepare() {
  local scratch_dir="${1}"
  local google_client_secrets_src="${2}"

  [[ -z "${scratch_dir}" ]] &&
    echo "error: missing scratch dir argument" >/dev/stderr &&
    exit 1

  [[ -z "${google_client_secrets_src}" ]] &&
    echo "error: missing google client secrets source argument" >/dev/stderr &&
    exit 1

  [[ ! -d "${localdir}" ]] && mkdir -p "${localdir}"

  [[ ! -d "${scratch_dir}" ]] &&
    echo "error: scratch dir at ${scratch_dir} not found" >/dev/stderr &&
    exit 1

  [[ ! -d "${rundir}" ]] && mkdir -p "${rundir}"
  [[ ! -e "${rundir}/scratch" ]] &&
    ln -fs "${scratch_dir}" "${rundir}/scratch"

  [[ ! -e "${scratch_dir}"/ccache ]] &&
    mkdir "${scratch_dir}"/ccache
  [[ ! -e "${scratch_dir}"/containers ]] &&
    mkdir "${scratch_dir}"/containers

  [[ ! -e "${rundir}"/data ]] &&
    mkdir "${rundir}"/data

  gen_server_keys

  if [[ ! -e "${server_cfg}" ]] || [[ ! -e "${worker_cfg}" ]]; then
    srv_key=$(openssl rand -hex 32)
    tkn_key=$(openssl rand -hex 32)

    tmp_server_cfg=$(mktemp)

    set_server_keys "${ourdir}"/cbs/cbs.config.server.example.json \
      "${tmp_server_cfg}" "${srv_key}" "${tkn_key}"

    cp "${tmp_server_cfg}" "${server_cfg}" || (
      echo "error: unable to copy server config" >/dev/stderr && exit 1
    )
    cp "${ourdir}"/cbs/cbs.config.worker.example.json "${worker_cfg}" || (
      echo "error: unable to copy worker config" >/dev/stderr && exit 1
    )
  fi

  cp "${google_client_secrets_src}" "${google_client_secrets}" || (
    echo "error: unable to copy google client secrets" >/dev/stderr &&
      exit 1
  )

  if [[ ! -e "${vault_cfg}" ]]; then
    cp "${ourdir}"/cbs/cbs.vault.example.json "${vault_cfg}" || (
      echo "error: unable to copy vault config" >/dev/stderr && exit 1
    )
    cat <<EOF >/dev/stdout

!! please configure the vault access credentials at
  -> ${vault_cfg} <-
!! keep in mind that only one authentication method will be used!

EOF
  fi

  [[ -e "${components_dir}" ]] && (rm -rf "${components_dir}" || (
    echo "error: unable to remove old components dir" >/dev/stderr && exit 1
  ))

  mkdir "${components_dir}" || (
    echo "error: unable to create components dir" >/dev/stderr && exit 1
  )
  for comp_dir in "${components[@]}"; do
    cp -r "${comp_dir}"/* "${components_dir}"/ || (
      echo "error: unable to copy components from '${comp_dir}'" >/dev/stderr &&
        exit 1
    )
    echo "=> using components from '${comp_dir}'"
  done

  available_components=$(ls "${components_dir}")
  [[ -z "${available_components}" ]] &&
    echo "error: no components found in '${components_dir}'" >/dev/stderr &&
    exit 1
  echo "=> available components: ${available_components}"

  echo "=> fully prepared CBS environment in '${rundir}'"
}

check() {
  [[ ! -e "${google_client_secrets}" ]] &&
    echo "error: missing google client secrets at '${google_client_secrets}'" \
      >/dev/stderr &&
    exit 1

  [[ ! -e "${server_cfg}" ]] &&
    echo "error: missing cbs config at '${server_cfg}'" >/dev/stderr &&
    exit 1

  [[ ! -e "${worker_cfg}" ]] &&
    echo "error: missing worker config at '${worker_cfg}'" >/dev/stderr &&
    exit 1

}

usage() {
  cat <<EOF >/dev/stderr
usage: $0 <COMMAND>

Commands:
  prepare <SCRATCH_DIR> [opts]    prepare environment to run CBS
  up                              bring up a CBS podman-compose environment
  down                            bring down a CBS podman-compose environment

Options:
  -h | --help               Show this message

Options for 'prepare':
  -f | --force                    force preparing from scratch
  --google-client-secrets <PATH>  path to google client app secrets file
                                  [required]
  --components <PATH>             path to a directory with components
                                  [multiple] (default: ./components)

EOF
}

[[ $# -eq 0 ]] && usage && exit 1

force=0
google_client_secrets_src=
vault_env_src=

args=()
cmd="${1}"
shift 1

while [[ $# -gt 0 ]]; do
  case $1 in
    -f | --force)
      force=1
      ;;
    --google-client-secrets)
      [[ -z $2 ]] &&
        echo "error: missing 'google-client-secrets' argument" >/dev/stderr &&
        usage &&
        exit 1
      google_client_secrets_src="${2}"
      if [[ ! -e "${google_client_secrets_src}" ]]; then
        echo "error: google client secrets file at '${google_client_secrets_src}' not found" >/dev/stderr
        exit 1
      fi
      shift 1
      ;;
    --components)
      [[ -z $2 ]] &&
        echo "error: missing 'components' argument" >/dev/stderr &&
        usage &&
        exit 1
      [[ ! -d $2 ]] &&
        echo "error: components directory at '${2}' not found" >/dev/stderr &&
        exit 1
      components+=("${2}")
      shift 1
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      echo "error: unknown option '${1}'" >/dev/stderr
      usage
      exit 1
      ;;
    *)
      args+=("${1}")
      ;;
  esac
  shift 1
done

case ${cmd} in
  prepare)
    if [[ ${#args[@]} -ne 1 ]]; then
      echo "error: 'prepare' command requires exactly one argument" >/dev/stderr
      usage
      exit 1
    fi

    [[ -z "${google_client_secrets_src}" ]] &&
      echo "error: missing '--google-client-secrets' argument" >/dev/stderr &&
      usage &&
      exit 1

    if [[ ${force} -eq 1 ]]; then
      echo "=> forcing prepare from scratch"
      rm -rf "${rundir}"
    fi

    [[ "${#components[@]}" -eq 0 ]] &&
      echo "=> using default components from './components'" &&
      components=("./components")

    prepare "${args[0]}" "${google_client_secrets_src}"
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
esac
