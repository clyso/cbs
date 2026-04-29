# crt - patch search
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


import re
import uuid
from pathlib import Path

import pydantic

from crt.crtlib.logger import logger as parent_logger
from crt.crtlib.models.common import ManifestPatchEntry
from crt.crtlib.models.discriminator import ManifestPatchEntryWrapper
from crt.crtlib.models.patch import PatchMeta
from crt.crtlib.models.patchset import GitHubPullRequest
from crt.crtlib.paths import patch_meta_dir

logger = parent_logger.getChild("search")


class PatchSearchResult(pydantic.BaseModel):
    """A single patch search result."""

    entry_uuid: uuid.UUID
    title: str
    source: str
    pr_id: int | None = None
    org: str | None = None
    repo: str | None = None


def search_patches(
    repo_path: Path,
    *,
    grep: str | None = None,
    source: str | None = None,
    pr: str | None = None,
    patch_uuid: str | None = None,
) -> list[PatchSearchResult]:
    """Search the patch library under ceph/patches/meta/."""
    meta_dir = patch_meta_dir(repo_path)
    if not meta_dir.exists():
        return []

    results: list[PatchSearchResult] = []
    grep_re = re.compile(grep, re.IGNORECASE) if grep else None

    for meta_path in meta_dir.glob("*.json"):
        try:
            entry_uuid = uuid.UUID(meta_path.stem)
        except ValueError:
            continue

        if patch_uuid and not str(entry_uuid).startswith(patch_uuid):
            continue

        try:
            wrapped = ManifestPatchEntryWrapper.model_validate_json(
                meta_path.read_text(encoding="utf-8")
            )
        except pydantic.ValidationError:
            logger.warning(f"malformed patch meta '{meta_path}', skip")
            continue

        entry: ManifestPatchEntry = wrapped.contents
        result = _entry_to_result(entry)
        if not result:
            continue

        if grep_re and not grep_re.search(result.title):
            continue

        if source and result.source != source:
            continue

        if pr:
            pr_org, pr_repo, pr_id = _parse_pr_ref(pr)
            if not (
                result.org == pr_org
                and result.repo == pr_repo
                and result.pr_id == pr_id
            ):
                continue

        results.append(result)

    return results


def search_patches_in_release(
    repo_path: Path,
    manifest_uuids: set[uuid.UUID],
    *,
    grep: str | None = None,
    patch_uuid: str | None = None,
) -> list[PatchSearchResult]:
    """Search patches referenced by manifests (by their patch UUIDs)."""
    meta_dir = patch_meta_dir(repo_path)
    if not meta_dir.exists():
        return []

    results: list[PatchSearchResult] = []
    grep_re = re.compile(grep, re.IGNORECASE) if grep else None

    for entry_uuid in manifest_uuids:
        if patch_uuid and not str(entry_uuid).startswith(patch_uuid):
            continue

        meta_path = meta_dir / f"{entry_uuid}.json"
        if not meta_path.exists():
            continue

        try:
            wrapped = ManifestPatchEntryWrapper.model_validate_json(
                meta_path.read_text(encoding="utf-8")
            )
        except pydantic.ValidationError:
            continue

        entry: ManifestPatchEntry = wrapped.contents
        result = _entry_to_result(entry)
        if not result:
            continue

        if grep_re and not grep_re.search(result.title):
            continue

        results.append(result)

    return results


def _entry_to_result(entry: ManifestPatchEntry) -> PatchSearchResult | None:
    if isinstance(entry, PatchMeta):
        return PatchSearchResult(
            entry_uuid=entry.entry_uuid,
            title=entry.info.title,
            source=entry.src_version or "unknown",
        )
    elif isinstance(entry, GitHubPullRequest):
        return PatchSearchResult(
            entry_uuid=entry.entry_uuid,
            title=entry.title,
            source=f"{entry.org_name}/{entry.repo_name}",
            pr_id=entry.pull_request_id,
            org=entry.org_name,
            repo=entry.repo_name,
        )
    return None


def _parse_pr_ref(pr_ref: str) -> tuple[str, str, int]:
    """Parse 'org/repo#id' into (org, repo, id)."""
    m = re.match(r"^([\w._-]+)/([\w._-]+)#(\d+)$", pr_ref)
    if not m:
        msg = f"malformed PR reference '{pr_ref}', expected 'org/repo#id'"
        raise ValueError(msg)
    return (m.group(1), m.group(2), int(m.group(3)))
