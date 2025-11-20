#!/bin/bash

components=()

ourdir="$(dirname "$(realpath "$0")")"
localdir="${ourdir}/_local"
rundir="${localdir}/cbs"
configdir="${rundir}/config"
worker_cfg_dir="${configdir}/worker"
server_cfg_dir="${configdir}/server"

components_dir="${rundir}/components"
cbscore_cfg="${worker_cfg_dir}/cbscore.config.yaml"
worker_cfg="${worker_cfg_dir}/cbs.worker.config.yaml"
server_cfg="${server_cfg_dir}/cbs.server.config.yaml"
google_client_secrets="${server_cfg_dir}/google-client-cbs.json"
cbs_cert="${server_cfg_dir}/cbs.cert.pem"
cbs_key="${server_cfg_dir}/cbs.key.pem"

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

  (yq ".server.secrets.session-secret-key = \"${srv_key}\" |
    .server.secrets.token-secret-key = \"${tkn_key}\"" \
    "${src_template}" >"${tgt_file}") || (
    echo "error: failed to set keys in ${tgt_file}" >/dev/stderr && exit 1
  )
}

prepare() {
  local scratch_dir="${1}"
  local google_client_secrets_src="${2}"
  local secrets_file_src="${3}"

  [[ -z "${scratch_dir}" ]] &&
    echo "error: missing scratch dir argument" >/dev/stderr &&
    exit 1

  [[ -z "${google_client_secrets_src}" ]] &&
    echo "error: missing google client secrets source argument" >/dev/stderr &&
    exit 1

  [[ -z "${secrets_file_src}" ]] &&
    echo "error: missing secrets file source argument" >/dev/stderr &&
    exit 1

  if ! cbsbuild --help >&/dev/null; then
    echo "error: 'cbsbuild' tool not found in PATH" >/dev/stderr
    exit 1
  fi

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

  [[ ! -e "${configdir}" ]] &&
    mkdir -p "${configdir}"

  [[ ! -e "${worker_cfg_dir}" ]] &&
    mkdir -p "${worker_cfg_dir}"

  [[ ! -e "${server_cfg_dir}" ]] &&
    mkdir -p "${server_cfg_dir}"

  if ! yq --help 2>/dev/null; then
    echo "error: 'yq' not available in PATH" >/dev/stderr
    exit 1
  fi

  cbsbuild config init-vault --vault "${configdir}/cbs.vault.yaml" || (
    echo "error: unable to init vault config" >/dev/stderr && exit 1
  )

  cbsbuild -c "${cbscore_cfg}" config init \
    --components /cbs/components \
    --scratch /cbs/scratch \
    --containers-scratch /var/lib/containers \
    --ccache /cbs/ccache \
    --vault /cbs/config/cbs.vault.yaml \
    --secrets /cbs/config/secrets.yaml || (
    echo "error: unable to init cbscore config at '${cbscore_cfg}'" >/dev/stderr &&
      exit 1
  )

  [[ ! -e "${worker_cfg_dir}/secrets.yaml" ]] && (
    cp "${secrets_file_src}" "${worker_cfg_dir}/secrets.yaml" || (
      echo "error: unable to copy secrets file to worker config dir" >/dev/stderr &&
        exit 1
    )
  ) && echo "=> copied secrets file to worker config dir"

  gen_server_keys

  if [[ ! -e "${server_cfg}" ]] || [[ ! -e "${worker_cfg}" ]]; then
    srv_key=$(openssl rand -hex 32)
    tkn_key=$(openssl rand -hex 32)

    tmp_server_cfg=$(mktemp)

    set_server_keys "${ourdir}"/cbs/cbs.server.config.example.yaml \
      "${tmp_server_cfg}" "${srv_key}" "${tkn_key}"

    cp "${tmp_server_cfg}" "${server_cfg}" || (
      echo "error: unable to copy server config" >/dev/stderr && exit 1
    )
    cp "${ourdir}"/cbs/cbs.worker.config.example.yaml "${worker_cfg}" || (
      echo "error: unable to copy worker config" >/dev/stderr && exit 1
    )
  fi

  cp "${google_client_secrets_src}" "${google_client_secrets}" || (
    echo "error: unable to copy google client secrets" >/dev/stderr &&
      exit 1
  )

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
  -h | --help                     Show this message

Options for 'prepare':
  -f | --force                    force preparing from scratch
  --google-client-secrets <PATH>  path to google client app secrets file
                                  [required]
  --components <PATH>             path to a directory with components
                                  [multiple] (default: ./components)
  --secrets <PATH>                path to secrets.yaml file [required]

EOF
}

[[ $# -eq 0 ]] && usage && exit 1

force=0
google_client_secrets_src=
secrets_file_src=

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
    --secrets)
      [[ -z $2 ]] &&
        echo "error: missing 'secrets' argument" >/dev/stderr &&
        usage &&
        exit 1
      [[ ! -e $2 ]] &&
        echo "error: secrets file at '${2}' not found" >/dev/stderr &&
        exit 1
      secrets_file_src="${2}"
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

    prepare "${args[0]}" "${google_client_secrets_src}" "${secrets_file_src}"
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
