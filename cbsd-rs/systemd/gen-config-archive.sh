#!/bin/bash

[[ ! -e ".git" ]] && {
  echo "error: must be run from the repository's root" >&2
  exit 1
}

archive_path="./cbsd-rs-config.tar"
systemd_path="$(realpath ./cbsd-rs/systemd)"

usage() {
  cat <<EOF >&2
usage: $0 [options]

Options:
  --component PATH        Specify component to be included in the archive.
                          Can be specified multiple times.
                          (default: ./components/*)
  -o | --output PATH      Specify resulting archive path.
                          (default: ${archive_path})
  -h | --help             Shows this message.

EOF
}

component_paths=()

while [[ $# -gt 0 ]]; do
  case ${1} in
    --component)
      [[ -z "${2}" ]] && {
        echo "error: '--component' requires a PATH" >&2
        exit 1
      }
      comp_path="$(realpath "${2}")"
      [[ ! -d "${comp_path}" ]] && {
        echo "error: path at '${2}' is not a directory" >&2
        exit 1
      }
      component_paths+=("${comp_path}")
      shift 1
      ;;
    -o | --output)
      [[ -z "${2}" ]] && {
        echo "error: '--output' requires a PATH" >&2
        exit 1
      }
      archive_path="${2}"
      shift 1
      ;;
    --help | -h)
      usage
      exit 0
      ;;
    -*)
      echo "error: unknown option '${1}'" >&2
      exit 1
      ;;
    *)
      echo "error: unknown argument '${1}'" >&2
      exit 1
      ;;
  esac
  shift 1
done

archive_path="$(realpath "${archive_path}")"

[[ -e "${archive_path}" ]] && {
  echo "error: archive path at '${archive_path}' already exists" >&2
  exit 1
}

if [[ ${#component_paths[@]} -eq 0 ]]; then
  [[ ! -d "./components" ]] && {
    echo "error: missing 'components' directory" >&2
    exit 1
  }

  for c in ./components/*; do
    component_paths+=("$(realpath "${c}")")
  done
fi

for comp_path in "${component_paths[@]}"; do
  res="$(find "${comp_path}" -name 'cbs.component.yaml' -type f -print -quit)"
  [[ -z "${res}" ]] && {
    echo "error: component not found at '${comp_path}'" >&2
    exit 1
  }
done

[[ ! -d "${systemd_path}" ]] && {
  echo "error: missing systemd path at '${systemd_path}'" >&2
  exit 1
}

archive_dir="$(mktemp -d --suffix='-cbsd-rs')"

cleanup() {
  [[ -e "${archive_dir}" ]] && rm -fr "${archive_dir}"
}

trap cleanup EXIT SIGINT SIGTERM

mkdir "${archive_dir}"/{components,systemd} || {
  echo "error: unable to create archive directories" >&2
  rm -fr "${archive_dir}"
  exit 1
}

cp -r "${systemd_path}"/* "${archive_dir}/systemd" || {
  echo "error: unable to copy systemd files from '${systemd_path}'" >&2
  exit 1
}

for comp in "${component_paths[@]}"; do
  comp_name="$(basename "${comp}")"
  echo "> archiving '${comp_name}' at '${comp}'"
  cp -r "${comp}" "${archive_dir}/components/${comp_name}" || {
    echo "error: unable to copy component from '${comp_path}'" >&2
    exit 1
  }
done

tar -C "${archive_dir}" -cf "${archive_path}" components systemd || {
  echo "error: unable to create archive at '${archive_path}'" >&2
  exit 1
}
