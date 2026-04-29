# crt - centralized path construction
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


from pathlib import Path

import yaml

CONFIG_FILENAME = "crt.config.yaml"
DEFAULT_COMPONENT = "ceph"


def component_dir(repo: Path) -> Path:
    config_path = repo / CONFIG_FILENAME
    if not config_path.exists():
        return repo / DEFAULT_COMPONENT

    raw: object = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    if isinstance(raw, dict):
        component = raw.get("component")
        if isinstance(component, str) and component:
            return repo / component

    return repo / DEFAULT_COMPONENT


# -- Release paths (on main) --


def releases_dir(repo: Path, ns: str, channel: str) -> Path:
    return component_dir(repo) / "releases" / ns / channel


def release_path(repo: Path, ns: str, channel: str, name: str) -> Path:
    return releases_dir(repo, ns, channel) / f"{name}.json"


# -- Manifest paths (on release branch) --


def manifest_dir(repo: Path, ns: str, channel: str) -> Path:
    return component_dir(repo) / "manifests" / ns / channel


def manifest_by_name_dir(repo: Path, ns: str, channel: str) -> Path:
    return manifest_dir(repo, ns, channel) / "by_name"


# -- Patch paths (shared, on main, inherited by release branches) --


def patches_dir(repo: Path) -> Path:
    return component_dir(repo) / "patches"


def patch_file(repo: Path, patch_uuid: str) -> Path:
    return patches_dir(repo) / f"{patch_uuid}.patch"


def patch_meta_dir(repo: Path) -> Path:
    return patches_dir(repo) / "meta"


def patch_meta_file(repo: Path, patch_uuid: str) -> Path:
    return patch_meta_dir(repo) / f"{patch_uuid}.json"


def patch_index_dir(repo: Path, org: str, repo_name: str) -> Path:
    return patches_dir(repo) / "index" / org / repo_name


def patch_index_pr_dir(repo: Path, org: str, repo_name: str, pr_id: int) -> Path:
    return patch_index_dir(repo, org, repo_name) / str(pr_id)


# -- Published patch trees (on release branch) --


def published_dir(repo: Path, ns: str, channel: str) -> Path:
    return component_dir(repo) / "published" / ns / channel


# -- Release notes (on release branch) --


def release_notes_dir(repo: Path, ns: str, channel: str) -> Path:
    return repo / "release-notes" / ns / channel


# -- Branch names --


def release_branch_name(ns: str, channel_version: str) -> str:
    return f"release/{ns}/{channel_version}"
