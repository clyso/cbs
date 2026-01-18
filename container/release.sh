#!/bin/bash

# CBS - Clyso Build System
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

_CHECKMARK="\u2713"
_INFOMARK="\u2139"

[[ ! -e ".git" ]] &&
  echo "error: must be run from the repository's root" >/dev/stderr && exit 1

default_remote="$(git remote -v 2>/dev/null | grep 'push' | head -n 1 | cut -f1)"

usage() {
  default_remote_str="${default_remote:-N/A}"
  cat <<EOF >/dev/stderr
usage: $0 <version> [options...]

Add an annotated tag to the current branch with the provided version.
The tag will be pushed to the specified upstream repository, or the first
available remote.

Versions must always be in the format 'vMAJOR.minor.patch', where 'MAJOR',
'minor', and 'patch', are integers greater or equal to zero.

Options:
  -r | --remote NAME    Name of remote to push to (default: ${default_remote_str})
  -h | --help           Show this message

EOF
}

push_remote="${default_remote}"
positional_args=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    -r | --remote)
      [[ -z $2 ]] &&
        echo "error: missing argument for '--remote'" >/dev/stderr &&
        exit 1
      push_remote="${2}"
      shift 1
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      echo "error: unknown argument '${1}'" >/dev/stderr
      exit 1
      ;;
    *)
      positional_args+=("${1}")
      ;;
  esac

  shift 1
done

cur_branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null)"
[[ -z "${cur_branch}" ]] &&
  echo "error: unable to obtain current git branch" >/dev/stderr &&
  exit 1

[[ "${cur_branch}" != "main" ]] &&
  echo "error: can only release while on 'main' branch" >/dev/stderr &&
  exit 1

[[ -z "${push_remote}" ]] &&
  echo "error: missing push remote" >/dev/stderr &&
  usage &&
  exit 1

[[ ${#positional_args[@]} -eq 0 ]] &&
  echo "error: missing 'version' argument" >/dev/stderr &&
  usage &&
  exit 1

[[ ${#positional_args[@]} -gt 1 ]] &&
  echo "error: too many arguments provided" >/dev/stderr &&
  usage &&
  exit 1

version="${positional_args[0]}"

[[ ! "${version}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] &&
  echo "error: malformed version, should be in format vMAJOR.minor.patch" >/dev/stderr &&
  exit 1

tag_found="$(git tag -l "${version}" 2>/dev/null)"
[[ -n "${tag_found}" ]] && [[ "${tag_found}" == "${version}" ]] &&
  echo "error: version '${version}' already exists" >/dev/stderr &&
  exit 1

git_user="$(git config user.name 2>/dev/null)"
git_email="$(git config user.email 2>/dev/null)"
git_signing_key="$(git config user.signingkey 2>/dev/null)"

[[ -z "${git_user}" ]] &&
  echo "error: missing git user name, must be set before releasing" >/dev/null &&
  exit 1

[[ -z "${git_email}" ]] &&
  echo "error: missing git user email, must be set before releasing" >/dev/null &&
  exit 1

[[ -z "${git_signing_key}" ]] &&
  echo "error: missing git user signing key, must be set before releasing" >/dev/null &&
  exit 1

echo -e "${_INFOMARK} update current main branch from remote '${push_remote}'"
if ! git remote update "${push_remote}" >/dev/null 2>&1; then
  echo "error: unable to update remote '${push_remote}'"
  exit 1
fi

if ! git pull "${push_remote}" main:main >/dev/null 2>&1; then
  echo "error: unable to pull latest remote main from remote '${push_remote}'"
  exit 1
fi

tmp_msg_file="$(mktemp)"
cat <<EOF >"${tmp_msg_file}"
Release CBS ${version}

Signed-off-by: ${git_user} <${git_email}>
EOF

if ! git tag -s -F "${tmp_msg_file}" "${version}"; then
  echo "error: unable to tag with version '${version}'"
  rm "${tmp_msg_file}"
  exit 1
fi

rm "${tmp_msg_file}"

echo -e "${_CHECKMARK} created tag '${version}'"
echo "${_INFOMARK} pushing to remote '${push_remote}'..."

if ! git push "${push_remote}" tag "${version}"; then
  echo "error: unable to push tag '${version}' to remote '${push_remote}'" >/dev/null
  exit 1
fi

echo -e "${_CHECKMARK} pushed tag '${version}' to '${push_remote}'"
exit 0
