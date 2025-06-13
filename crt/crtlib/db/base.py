# crt - db - base database
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


import abc
import uuid
from pathlib import Path
from typing import TypeVar

import pydantic
from crtlib.db import (
    get_gh_prs_loc,
    get_manifests_loc,
    get_patches_by_sha_loc,
    get_patches_by_uuid_loc,
    get_patches_loc,
    get_patchsets_loc,
    manifest_loc,
)
from crtlib.db import logger as parent_logger
from crtlib.db.errors import DBError
from crtlib.errors.manifest import MalformedManifestError, NoSuchManifestError
from crtlib.errors.patchset import (
    MalformedPatchSetError,
    NoSuchPatchSetError,
    PatchSetError,
    PatchSetMismatchError,
)
from crtlib.models.db import DBManifestInfo
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patchset import GitHubPullRequest, PatchSet, PatchSetBase

logger = parent_logger.getChild("basedb")

_T = TypeVar("_T", bound=pydantic.BaseModel)


class BaseDB(abc.ABC):
    """
    Representation of the releases database.

    For a release db root at `$root`, the format is as follows:

    $root/manifests         - stores release manifests as JSON files, identified by
                              their UUIDs.
    $root/patchsets         - stores patchsets for releases, identified by their UUIDs.
    $root/patchsets/gh      - stores pull requests from GitHub, mapping them to the
                              corresponding patch set UUIDs.
    $root/patches/by_uuid   - stores patches information as JSON files, identified by
                              their UUIDs.
    $root/patches/by_sha    - stores patches' SHAs, mapping them to the corresponding
                              patch UUID.

    The release db exists both on-disk and in S3.
    """

    _base_path: Path

    def __init__(self, path: Path) -> None:
        self._base_path = path
        self._init_tree()

    def _init_tree(self) -> None:
        self._base_path.mkdir(exist_ok=True, parents=True)
        self.manifests_path.mkdir(exist_ok=True, parents=True)
        self.gh_prs_path.mkdir(exist_ok=True, parents=True)
        self.patches_by_uuid_path.mkdir(exist_ok=True, parents=True)
        self.patches_by_sha_path.mkdir(exist_ok=True, parents=True)

    def manifest_path(self, _uuid: uuid.UUID) -> Path:
        return Path(manifest_loc(self._base_path, _uuid))

    @property
    def manifests_path(self) -> Path:
        return Path(get_manifests_loc(self._base_path))

    @property
    def patchsets_path(self) -> Path:
        return Path(get_patchsets_loc(self._base_path))

    @property
    def gh_prs_path(self) -> Path:
        return Path(get_gh_prs_loc(self._base_path))

    @property
    def patches_path(self) -> Path:
        return Path(get_patches_loc(self._base_path))

    @property
    def patches_by_uuid_path(self) -> Path:
        return Path(get_patches_by_uuid_loc(self._base_path))

    @property
    def patches_by_sha_path(self) -> Path:
        return Path(get_patches_by_sha_loc(self._base_path))

    def _write_manifest(self, _uuid: uuid.UUID, manifest: pydantic.BaseModel) -> None:
        """Write a manifest to disk."""
        manifest_path = self.manifest_path(_uuid)
        try:
            n = manifest_path.write_text(manifest.model_dump_json(indent=2))
        except Exception as e:
            msg = f"unable to write manifest uuid '{_uuid}' to '{manifest_path}': {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

        logger.debug(f"wrote manifest uuid '{_uuid}' to '{manifest_path}' size {n}")

    def _read_manifest(self, _uuid: uuid.UUID, t: type[_T]) -> _T:
        """Read a manifest from disk, into the provided type `t`."""
        manifest_path = self.manifest_path(_uuid)
        if not manifest_path.exists():
            raise NoSuchManifestError(_uuid)

        try:
            return t.model_validate_json(manifest_path.read_text())
        except pydantic.ValidationError:
            msg = f"malformed manifest uuid '{_uuid}' at '{manifest_path}"
            logger.error(msg)
            raise MalformedManifestError(_uuid, msg=msg) from None

    def list_manifests(self) -> list[tuple[ReleaseManifest, DBManifestInfo]]:
        """Obtain list of manifests (and corresponding db info) from disk."""
        lst: list[tuple[ReleaseManifest, DBManifestInfo]] = []
        for entry in self.manifests_path.glob("*.json"):
            try:
                entry_uuid = uuid.UUID(entry.stem)
            except Exception:  # noqa: S112
                # malformed UUID, ignore.
                continue

            manifest = self.get_manifest(entry_uuid)
            info = self.get_manifest_info(entry_uuid)
            lst.append((manifest, info))

        return lst

    @abc.abstractmethod
    def get_manifest(self, _uuid: uuid.UUID) -> ReleaseManifest:
        pass

    @abc.abstractmethod
    def get_manifest_info(self, _uuid: uuid.UUID) -> DBManifestInfo:
        pass

    @abc.abstractmethod
    def store_manifest(
        self, manifest: ReleaseManifest, *, etag: str | None = None
    ) -> None:
        pass

    def list_patchsets(self) -> list[PatchSet]:
        """List known patch sets."""
        lst: list[PatchSet] = []
        return lst

    def _get_patchset(self, _uuid: uuid.UUID, t: type[_T]) -> _T:
        """Obtain a patch set by its UUID."""
        patchset_path = self.patchsets_path.joinpath(f"{_uuid}.json")
        if not patchset_path.exists():
            msg = f"patch set uuid '{_uuid}' not found"
            logger.warning(msg)
            raise NoSuchPatchSetError(msg=msg)

        try:
            patchset_ctr = PatchSet.model_validate_json(patchset_path.read_text())
        except pydantic.ValidationError:
            msg = f"malformed patch set uuid '{_uuid}'"
            logger.error(msg)
            raise MalformedPatchSetError(msg=msg) from None

        if not isinstance(patchset_ctr.info, t):
            msg = (
                f"patch set uuid '{_uuid}' mismatch, "
                + f"found '{type(patchset_ctr.info)}' expected '{t}'"
            )
            logger.error(msg)
            raise PatchSetMismatchError(msg=msg)

        return patchset_ctr.info

    def get_patchset(self, _uuid: uuid.UUID) -> PatchSetBase:
        """Obtain a patch set by its UUID."""
        return self._get_patchset(_uuid, PatchSetBase)

    def get_patchset_gh_pr(self, org: str, repo: str, pr_id: int) -> GitHubPullRequest:
        """Obtain a GitHub pull request patch set."""
        pr_desc = f"{org}/{repo}/{pr_id}"
        pr_path = self.gh_prs_path.joinpath(pr_desc)
        logger.debug(f"get gh patch set from '{pr_path}'")
        if not pr_path.exists():
            msg = f"gh patch set '{pr_desc}' not found"
            logger.debug(msg)
            raise NoSuchPatchSetError(msg)

        try:
            patchset_uuid = uuid.UUID(pr_path.read_text())
        except Exception as e:
            msg = f"missing uuid for gh patch set '{pr_desc}': {e}"
            logger.error(msg)
            raise PatchSetError(msg=msg) from None

        return self._get_patchset(patchset_uuid, GitHubPullRequest)

    def store_patchset(self, _uuid: uuid.UUID, patchset: PatchSet) -> None:
        """Store a patch set to disk."""
        patchset_path = self.patchsets_path.joinpath(f"{_uuid}.json")
        try:
            _ = patchset_path.write_text(patchset.model_dump_json(indent=2))
        except Exception as e:
            msg = f"unable to write patch set uuid '{_uuid}': {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

    @abc.abstractmethod
    def store_patchset_gh_pr(self, patchset: GitHubPullRequest) -> None:
        """Store a GitHub pull request patch set."""
        pass
