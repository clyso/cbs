#!/bin/bash

ourdir="$(dirname "$(realpath "$0")")"
localdir="${ourdir}/_local"
rundir="${localdir}/cbs"

server_cfg="${rundir}/cbs-config.server.json"
worker_cfg="${rundir}/cbs-config.worker.json"
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

gen_keys() {
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

set_keys() {
  local src_template="${1}"
  local tgt_file="${2}"
  local srv_key="${3}"
  local tkn_key="${4}"

  (jq ".secrets.server.session_secret_key = \"${srv_key}\" |
    .secrets.server.token_secret_key = \"${tkn_key}\"" \
    "${src_template}" >"${tgt_file}") || (
    echo "error: failed to set keys in ${tgt_file}" >/dev/stderr && exit 1
  )
}

set_vault_creds() {
  local src_file="${1}"

  [[ -z "${VAULT_ADDR}" ]] && "error: missing VAULT_ADDR" >/dev/stderr && exit 1
  [[ -z "${VAULT_ROLE_ID}" ]] &&
    "error: missing VAULT_ROLE_ID" >/dev/stderr &&
    exit 1
  [[ -z "${VAULT_SECRET_ID}" ]] &&
    "error: missing VAULT_SECRET_ID" >/dev/stderr &&
    exit 1
  [[ -z "${VAULT_TRANSIT}" ]] && echo "error: missing VAULT_TRANSIT" >/dev/stderr && exit 1

  tmp_file=$(mktemp)

  (jq ".secrets.vault.addr = \"${VAULT_ADDR}\" |
    .secrets.vault.role_id = \"${VAULT_ROLE_ID}\" |
    .secrets.vault.secret_id = \"${VAULT_SECRET_ID}\" |
    .secrets.vault.transit = \"${VAULT_TRANSIT}\"" \
    "${src_file}" >"${tmp_file}") || (
    echo "error: failed to set vault creds in ${src_file}" >/dev/stderr && exit 1
  )
  cp "${tmp_file}" "${src_file}" || (echo "error: unable to copy vault creds" >/dev/stderr && exit 1)
  rm "${tmp_file}"
}

prepare() {
  local scratch_dir="${1}"
  local google_client_secrets_src="${2}"
  local vault_env_src="${3}"

  [[ -z "${scratch_dir}" ]] &&
    echo "error: missing scratch dir argument" >/dev/stderr &&
    exit 1

  [[ -z "${google_client_secrets_src}" ]] &&
    echo "error: missing google client secrets source argument" >/dev/stderr &&
    exit 1

  [[ -z "${vault_env_src}" ]] &&
    echo "error: missing vault env source argument" >/dev/stderr &&
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

  gen_keys

  if [[ ! -e "${server_cfg}" ]] || [[ ! -e "${worker_cfg}" ]]; then
    srv_key=$(openssl rand -hex 32)
    tkn_key=$(openssl rand -hex 32)

    tmp_server_cfg=$(mktemp)
    tmp_worker_cfg=$(mktemp)

    set_keys "${ourdir}"/cbs/cbs-config.server.example.json \
      "${tmp_server_cfg}" "${srv_key}" "${tkn_key}"
    set_keys "${ourdir}"/cbs/cbs-config.worker.example.json \
      "${tmp_worker_cfg}" "${srv_key}" "${tkn_key}"

    # shellcheck disable=SC1090
    source "${vault_env_src}"

    set_vault_creds "${tmp_server_cfg}"
    set_vault_creds "${tmp_worker_cfg}"

    cp "${tmp_server_cfg}" "${server_cfg}" || (
      echo "error: unable to copy server config" >/dev/stderr && exit 1
    )
    cp "${tmp_worker_cfg}" "${worker_cfg}" || (
      echo "error: unable to copy worker config" >/dev/stderr && exit 1
    )
    rm "${tmp_server_cfg}" "${tmp_worker_cfg}"
  fi

  cp "${google_client_secrets_src}" "${google_client_secrets}" || (
    echo "error: unable to copy google client secrets" >/dev/stderr &&
      exit 1
  )

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
  --vault-env <PATH>              path to vaul creds environment file
                                  [required]

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
    --vault-env)
      [[ -z $2 ]] &&
        echo "error: missing 'vault-env' argument" >/dev/stderr &&
        usage &&
        exit 1
      vault_env_src="${2}"
      if [[ ! -e "${vault_env_src}" ]]; then
        echo "error: vault env file at '${vault_env_src}' not found" >/dev/stderr
        exit 1
      fi
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

    prepare "${args[0]}" "${google_client_secrets_src}" "${vault_env_src}"
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
